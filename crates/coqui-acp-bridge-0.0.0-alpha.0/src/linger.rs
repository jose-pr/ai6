//! Keeping an agent through a reconnect: when a link ends its agent waits for the same proven peer, for a time, with what it writes
//! meanwhile held in a bounded buffer. What the buffer has to drop is said on the next link, since nothing here replays silently.

use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use coqui_transport::Channel;

use crate::agent::{Agent, Output};
use crate::relay::{LinkEnd, relay_holding};

/// The method of the one message the bridge writes itself: a notification, first on a link, with `lines` and `bytes` it dropped while
/// nobody was connected. ACP has no message for this, and has clients ignore an unknown notification whose method starts with `_`.
pub const OUTPUT_LOST: &str = "_dev.boriken.coqui.acp/output_lost";

/// The most [`Settings::buffer`] can hold for one agent, 256 MiB; a larger setting holds this much.
pub const MAX_BUFFER: usize = 256 * 1024 * 1024;

/// How often a kept agent's watcher looks up from its output.
const POLL: Duration = Duration::from_millis(50);
/// How often a kept agent's watcher asks whether its peer is still accepted.
const CHECK: Duration = Duration::from_secs(1);
/// What holding one line costs beyond its bytes: its place in the queue.
const LINE_OVERHEAD: usize = core::mem::size_of::<Vec<u8>>();

/// What a [`Keeper`] keeps, and for how long.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Settings {
    /// How long an agent waits once its link has ended.
    pub keep: Duration,
    /// Agents kept for one peer. The bridge reads no ACP, so it cannot tell which agent holds the session a peer wants: past one,
    /// a peer can be given the wrong one. Past this, of that peer's agents the one started last ends, since a peer has two only when a
    /// connection came while an earlier link was still up, and the earlier holds the session that link was in. Default 1.
    pub per_peer: NonZeroUsize,
    /// Agents kept for everyone together, past which the one kept longest ends. Default 4.
    pub total: NonZeroUsize,
    /// Bytes to hold of a kept agent's output, each line counted with what holding it takes, past which the oldest lines are dropped
    /// and said to be. At most [`MAX_BUFFER`]. Default 16 MiB.
    pub buffer: usize,
    /// How long a new connection waits for a link of the same peer to end, so that a reload faster than the old link's end still
    /// finds its agent. It decides what a second connection beside a live link waits; one too long for the clock waits until that
    /// link ends. Default 500 ms.
    pub settle: Duration,
}

impl Settings {
    /// Keeps for `keep`, with the defaults above.
    #[must_use]
    pub const fn new(keep: Duration) -> Self {
        Self {
            keep,
            per_peer: NonZeroUsize::MIN,
            total: NonZeroUsize::new(4).expect("four is not zero"),
            buffer: 16 * 1024 * 1024,
            settle: Duration::from_millis(500),
        }
    }
}

/// Output held for the next link: always the newest lines in order, with a count of what went before them.
#[derive(Debug)]
pub(crate) struct Backlog {
    limit: usize,
    held: usize,
    lines: VecDeque<Vec<u8>>,
    lost_lines: u64,
    lost_bytes: u64,
}

/// What holding `line` takes: its allocation and its place in the queue.
const fn cost(line: &Vec<u8>) -> usize {
    line.capacity().saturating_add(LINE_OVERHEAD)
}

impl Backlog {
    pub(crate) const fn new(limit: usize) -> Self {
        Self {
            limit,
            held: 0,
            lines: VecDeque::new(),
            lost_lines: 0,
            lost_bytes: 0,
        }
    }

    /// Holds a line. The newest are the ones kept, since those hold the answer that ends a turn; a line over the limit by itself goes
    /// with all before it, so what is held stays unbroken.
    pub(crate) fn push(&mut self, mut line: Vec<u8>) {
        line.shrink_to_fit();
        if cost(&line) > self.limit {
            while self.drop_oldest() {}
            self.note_lost(&line);
            return;
        }
        self.held += cost(&line);
        self.lines.push_back(line);
        while self.held > self.limit {
            self.drop_oldest();
        }
    }

    fn drop_oldest(&mut self) -> bool {
        let Some(line) = self.lines.pop_front() else {
            return false;
        };
        self.held -= cost(&line);
        self.note_lost(&line);
        true
    }

    fn note_lost(&mut self, line: &[u8]) {
        self.lost_lines = self.lost_lines.saturating_add(1);
        self.lost_bytes = self
            .lost_bytes
            .saturating_add(u64::try_from(line.len()).unwrap_or(u64::MAX));
    }

    /// Whether there is nothing to send, not even a notice.
    fn is_empty(&self) -> bool {
        self.lost_lines == 0 && self.lines.is_empty()
    }

    /// What goes on the wire next: the notice of what was lost, then the lines.
    pub(crate) fn next(&self) -> Option<Cow<'_, [u8]>> {
        if self.lost_lines > 0 {
            let notice = format!(
                r#"{{"jsonrpc":"2.0","method":"{OUTPUT_LOST}","params":{{"lines":{},"bytes":{}}}}}"#,
                self.lost_lines, self.lost_bytes
            );
            return Some(Cow::Owned(notice.into_bytes()));
        }
        self.lines
            .front()
            .map(|line| Cow::Borrowed(line.as_slice()))
    }

    /// What [`Self::next`] gave has been sent.
    pub(crate) fn sent(&mut self) {
        if self.lost_lines > 0 {
            self.lost_lines = 0;
            self.lost_bytes = 0;
        } else if let Some(line) = self.lines.pop_front() {
            self.held -= cost(&line);
        }
    }
}

struct Kept {
    agent: Agent,
    backlog: Backlog,
    /// When the agent started, in the order agents start here.
    started: u64,
}

impl Kept {
    /// Whether the agent can still say anything. Once it cannot, what it wrote that still waited is moved to the backlog.
    fn running(&mut self) -> bool {
        if self.agent.output_open() {
            return true;
        }
        while let Ok(Output::Line(line)) = self.agent.next_output(Duration::ZERO) {
            self.backlog.push(line);
        }
        false
    }
}

struct Parked {
    id: u64,
    peer: String,
    started: u64,
    leave: Arc<AtomicBool>,
    watcher: JoinHandle<Option<Kept>>,
}

/// What a peer gets back.
enum Taken {
    /// Its agent, still running.
    Agent(Box<Kept>),
    /// What an agent that has since ended wrote while nobody was connected.
    Output(Backlog),
}

#[derive(Default)]
struct State {
    /// Oldest first.
    parked: Vec<Parked>,
    /// Links up, by peer, counted from their arrival until their agent is dealt with.
    up: BTreeMap<String, usize>,
    next: u64,
    started: u64,
}

type Accepted = Box<dyn Fn(&str) -> bool + Send + Sync>;

struct Inner {
    settings: Settings,
    accepted: Accepted,
    report: Box<dyn Fn(&str) + Send + Sync>,
    state: Mutex<State>,
    changed: Condvar,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The agents waiting for their peers. Clones share them. Each is ended by its own time, by making room, by its peer no longer being
/// accepted, or by being dropped with its [`Agent`]; one left behind by this process ending is what [`Agent::guarded`] is for.
#[derive(Clone)]
pub struct Keeper(Arc<Inner>);

impl core::fmt::Debug for Keeper {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Keeper")
            .field("settings", &self.0.settings)
            .finish_non_exhaustive()
    }
}

impl Keeper {
    /// Keeps by `settings`. `accepted` says whether a peer may still have its agent: it is asked about once a second while one is kept,
    /// and again before one is given back, and an agent whose peer is no longer accepted ends. It must not block for long. `report`
    /// hears, in a sentence, what became of each agent.
    pub fn new(
        mut settings: Settings,
        accepted: impl Fn(&str) -> bool + Send + Sync + 'static,
        report: impl Fn(&str) + Send + Sync + 'static,
    ) -> Self {
        settings.buffer = settings.buffer.min(MAX_BUFFER);
        Self(Arc::new(Inner {
            settings,
            accepted: Box::new(accepted),
            report: Box::new(report),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        }))
    }

    /// A new connection from `peer`, proven. Waits up to [`Settings::settle`] while a link of this peer is up, so it finds what that
    /// link leaves. Hold what it returns until the connection's agent has been dealt with.
    #[must_use]
    pub fn arrive(&self, peer: &str) -> Arrival {
        // None: longer than the clock can count, which is waiting for as long as it takes.
        let until = Instant::now().checked_add(self.0.settings.settle);
        let mut state = self.0.state();
        while state.up.get(peer).is_some_and(|links| *links > 0) {
            state = match until {
                Some(until) => {
                    let left = until.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    self.0
                        .changed
                        .wait_timeout(state, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self
                    .0
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
            };
        }
        *state.up.entry(peer.to_owned()).or_default() += 1;
        Arrival {
            keeper: self.clone(),
            peer: peer.to_owned(),
        }
    }

    /// The earliest started of what is kept for `peer`: an agent still running, or else what one that has ended wrote.
    fn take(&self, peer: &str) -> Option<Taken> {
        loop {
            let parked = {
                let mut state = self.0.state();
                let found = state
                    .parked
                    .iter()
                    .enumerate()
                    .filter(|(_, parked)| parked.peer == peer)
                    .min_by_key(|(_, parked)| parked.started)
                    .map(|(at, _)| at)?;
                state.parked.remove(found)
            };
            parked.leave.store(true, Ordering::SeqCst);
            let Ok(Some(mut kept)) = parked.watcher.join() else {
                continue;
            };
            if !(self.0.accepted)(peer) {
                drop(kept);
                (self.0.report)(&format!(
                    "{peer}'s kept agent is gone: its peer is no longer accepted"
                ));
                return None;
            }
            if kept.running() {
                return Some(Taken::Agent(Box::new(kept)));
            }
            if !kept.backlog.is_empty() {
                return Some(Taken::Output(kept.backlog));
            }
        }
    }

    /// The next number in the order agents start.
    fn started(&self) -> u64 {
        let mut state = self.0.state();
        state.started += 1;
        state.started
    }

    /// Keeps an agent for `peer`, ending one where a bound is reached: of the peer's own the one started last, which may be this one, and
    /// of everyone's the one kept longest.
    fn park(&self, peer: &str, kept: Kept) {
        if !(self.0.accepted)(peer) {
            drop(kept);
            (self.0.report)(&format!(
                "{peer}'s agent was not kept: its peer is no longer accepted"
            ));
            return;
        }
        let settings = self.0.settings;
        let mut evicted = Vec::new();
        let mut refused = false;
        {
            let mut state = self.0.state();
            while !refused {
                let latest = state
                    .parked
                    .iter()
                    .enumerate()
                    .filter(|(_, parked)| parked.peer == peer)
                    .map(|(at, parked)| (at, parked.started))
                    .collect::<Vec<_>>();
                if latest.len() < settings.per_peer.get() {
                    break;
                }
                match latest.iter().max_by_key(|(_, started)| *started) {
                    Some(&(at, started)) if started > kept.started => {
                        evicted.push(state.parked.remove(at));
                    }
                    _ => refused = true,
                }
            }
            if refused {
                drop(state);
                drop(kept);
                (self.0.report)(&format!(
                    "{peer}'s agent was not kept: an agent of that peer started earlier holds its place"
                ));
                self.end(evicted);
                return;
            }
            while state.parked.len() >= settings.total.get() {
                evicted.push(state.parked.remove(0));
            }
            state.next += 1;
            let id = state.next;
            let started = kept.started;
            let leave = Arc::new(AtomicBool::new(false));
            let keeper = self.clone();
            let (told, owner) = (Arc::clone(&leave), peer.to_owned());
            // A closed process table is the only way this fails, and the closure's drop then ends the agent.
            let until = Instant::now()
                .checked_add(settings.keep)
                .unwrap_or_else(Instant::now);
            let watcher = thread::Builder::new()
                .name("coqui-acp-bridge-kept".to_owned())
                .spawn(move || keeper.watch(id, &owner, kept, &told, until));
            match watcher {
                Ok(watcher) => state.parked.push(Parked {
                    id,
                    peer: peer.to_owned(),
                    started,
                    leave,
                    watcher,
                }),
                Err(error) => {
                    (self.0.report)(&format!("{peer}'s agent could not be kept: {error}"))
                }
            }
        }
        self.0.changed.notify_all();
        self.end(evicted);
    }

    fn end(&self, evicted: Vec<Parked>) {
        for parked in evicted {
            parked.leave.store(true, Ordering::SeqCst);
            drop(parked.watcher.join());
            (self.0.report)(&format!(
                "{}'s kept agent was ended to make room",
                parked.peer
            ));
        }
    }

    /// Takes a kept agent's output into its buffer until it is wanted, or has to go. Whoever removed the entry is waiting for it. An
    /// agent that ends with output held is collected, and what it wrote waits out the time for its peer.
    fn watch(
        &self,
        id: u64,
        peer: &str,
        mut kept: Kept,
        leave: &AtomicBool,
        until: Instant,
    ) -> Option<Kept> {
        let mut check = Instant::now() + CHECK;
        let mut ended = false;
        let why = loop {
            if leave.load(Ordering::SeqCst) {
                return Some(kept);
            }
            let now = Instant::now();
            if now >= until {
                break "its time ran out";
            }
            if now >= check {
                if !(self.0.accepted)(peer) {
                    break "its peer is no longer accepted";
                }
                check = now + CHECK;
            }
            if ended {
                thread::sleep(POLL);
                continue;
            }
            match kept.agent.next_output(POLL) {
                Ok(Output::Line(line)) => kept.backlog.push(line),
                Ok(Output::TooLong) => break "it wrote a line over the limit",
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) if kept.backlog.is_empty() => break "it ended",
                Err(RecvTimeoutError::Disconnected) => {
                    ended = true;
                    let _ = kept.agent.stop();
                    (self.0.report)(&format!(
                        "{peer}'s kept agent ended; what it wrote waits for the next link"
                    ));
                }
            }
        };
        let mine = {
            let mut state = self.0.state();
            let found = state.parked.iter().position(|parked| parked.id == id);
            found.map(|found| state.parked.remove(found))
        };
        if mine.is_none() {
            return Some(kept);
        }
        drop(kept);
        (self.0.report)(&format!("{peer}'s kept agent is gone: {why}"));
        None
    }
}

/// A connection a [`Keeper`] knows of. Dropping it says this connection's agent has been dealt with.
#[derive(Debug)]
pub struct Arrival {
    keeper: Keeper,
    peer: String,
}

impl Arrival {
    /// Relays over `channel` until the link ends, with the agent kept for this peer or else one from `start`. What was kept for this
    /// peer goes first, including what a kept agent that has since ended wrote. Afterwards the agent is kept again, or ended if the
    /// link's end leaves nothing to come back to.
    ///
    /// # Errors
    ///
    /// `start` failed.
    pub fn serve<C: Channel + ?Sized>(
        &self,
        channel: &C,
        start: impl FnOnce() -> io::Result<Agent>,
    ) -> io::Result<LinkEnd> {
        let inner = &self.keeper.0;
        let mut kept = match self.keeper.take(&self.peer) {
            Some(Taken::Agent(kept)) => {
                let kept = *kept;
                (inner.report)(&format!(
                    "{} got its agent back, {} lines of its output lost",
                    self.peer, kept.backlog.lost_lines
                ));
                kept
            }
            Some(Taken::Output(backlog)) => {
                (inner.report)(&format!(
                    "{} gets what its ended agent wrote, then a fresh agent",
                    self.peer
                ));
                Kept {
                    agent: start()?,
                    backlog,
                    started: self.keeper.started(),
                }
            }
            None => Kept {
                agent: start()?,
                backlog: Backlog::new(inner.settings.buffer),
                started: self.keeper.started(),
            },
        };
        let end = relay_holding(channel, &kept.agent, &mut kept.backlog);
        if end.keeps_agent() {
            self.keeper.park(&self.peer, kept);
        } else {
            let _ = kept.agent.stop();
        }
        Ok(end)
    }
}

impl Drop for Arrival {
    fn drop(&mut self) {
        let inner = &self.keeper.0;
        {
            let mut state = inner.state();
            if let Some(links) = state.up.get_mut(&self.peer) {
                *links -= 1;
                if *links == 0 {
                    state.up.remove(&self.peer);
                }
            }
        }
        inner.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(backlog: &mut Backlog) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(message) = backlog.next() {
            out.push(message.into_owned());
            backlog.sent();
        }
        out
    }

    #[test]
    fn what_fits_comes_back_in_order_with_no_notice() {
        let mut backlog = Backlog::new(10 + 2 * LINE_OVERHEAD);
        backlog.push(b"one".to_vec());
        backlog.push(b"two".to_vec());
        assert_eq!(drain(&mut backlog), [b"one".to_vec(), b"two".to_vec()]);
    }

    #[test]
    fn past_the_limit_the_oldest_go_and_the_notice_comes_first_with_their_count() {
        let mut backlog = Backlog::new(6 + 3 * LINE_OVERHEAD);
        for line in [&b"aaa"[..], b"bbb", b"cc", b"d"] {
            backlog.push(line.to_vec());
        }
        let out = drain(&mut backlog);
        let notice: serde_json::Value = serde_json::from_slice(&out[0]).unwrap();
        assert_eq!(notice["jsonrpc"], "2.0");
        assert_eq!(notice["method"], OUTPUT_LOST);
        assert!(notice.get("id").is_none(), "a notification has no id");
        assert_eq!(notice["params"]["lines"], 1);
        assert_eq!(notice["params"]["bytes"], 3);
        assert_eq!(out[1..], [b"bbb".to_vec(), b"cc".to_vec(), b"d".to_vec()]);
    }

    #[test]
    fn a_line_over_the_limit_goes_with_all_before_it_so_what_is_held_is_unbroken() {
        let mut backlog = Backlog::new(4 + LINE_OVERHEAD);
        backlog.push(b"aa".to_vec());
        backlog.push(b"toolong".to_vec());
        backlog.push(b"bb".to_vec());
        let out = drain(&mut backlog);
        let notice: serde_json::Value = serde_json::from_slice(&out[0]).unwrap();
        assert_eq!(notice["params"]["lines"], 2);
        assert_eq!(notice["params"]["bytes"], 9);
        assert_eq!(out[1..], [b"bb".to_vec()]);
    }

    #[test]
    fn a_notice_not_yet_sent_stays_until_it_is() {
        let mut backlog = Backlog::new(1);
        backlog.push(b"gone".to_vec());
        let first = backlog.next().unwrap().into_owned();
        assert_eq!(backlog.next().unwrap().into_owned(), first);
        backlog.sent();
        assert!(backlog.next().is_none());
    }

    #[test]
    fn with_no_room_every_line_is_counted_and_none_held() {
        let mut backlog = Backlog::new(0);
        backlog.push(b"a".to_vec());
        backlog.push(b"bc".to_vec());
        let out = drain(&mut backlog);
        assert_eq!(out.len(), 1);
        let notice: serde_json::Value = serde_json::from_slice(&out[0]).unwrap();
        assert_eq!(notice["params"]["lines"], 2);
        assert_eq!(notice["params"]["bytes"], 3);
    }

    #[test]
    fn each_line_costs_what_holding_it_takes_not_only_its_bytes() {
        let mut backlog = Backlog::new(1000);
        for _ in 0..100 {
            backlog.push(b"x".to_vec());
        }
        let out = drain(&mut backlog);
        let notice: serde_json::Value = serde_json::from_slice(&out[0]).unwrap();
        assert_eq!(notice["method"], OUTPUT_LOST);
        let lost = notice["params"]["lines"].as_u64().unwrap();
        assert_eq!(lost + u64::try_from(out.len() - 1).unwrap(), 100);
        assert!(out.len() - 1 < 1000 / core::mem::size_of::<Vec<u8>>());
    }

    #[test]
    fn the_defaults_keep_one_agent_a_peer_and_four_in_all() {
        let settings = Settings::new(Duration::from_secs(1));
        assert_eq!(settings.per_peer.get(), 1);
        assert_eq!(settings.total.get(), 4);
    }
}
