//! What a provider of the `acp_harness` kind needs to front a harness that [`serve`] serves: the channel
//! the host talks over, ended the way the kind says, and the usage a provider keeps for its `usage` method.
//!
//! [`Serving::open`] runs `serve` on a thread of its own over a [`Channel`]: each message the host sends is a line `serve`
//! reads, each line it writes a message handed to the host's sink. The harness starts its programs through the host's
//! spawner, which is destroyed before the sink is told the connection has closed.
//!
//! This module holds the one piece of process-wide state of the crate: the threads it started, which a library that
//! provides the kind has to be able to wait for before it is unloaded ([`threads_done`]).

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::io::{self, Read, Write};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::usage::Bucket;
use coqui_acp_api::{
    Channel, ChannelSink, HarnessNotice, ProcessSpawner, UsageReport, UsageSupport,
};
use serde_json::Value;

use crate::acp::{Harness, Settings, UsageTracker, serve};
use crate::process::{Launch, Spawn, Spawned};
use crate::spawner::ThroughSpawner;

/// How many messages wait on either side of `serve` before the one handing them over waits too.
const QUEUE: usize = 4;

/// The threads this library started, joined when it is unloaded.
static THREADS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Has this library wait for `thread` before it is unloaded, as it does for the threads of its connections: a provider's own
/// worker thread (a sign-in's) is handed here.
pub fn track(thread: JoinHandle<()>) {
    let mut threads = THREADS.lock().unwrap_or_else(PoisonError::into_inner);
    let (done, running): (Vec<_>, Vec<_>) = threads.drain(..).partition(|t| t.is_finished());
    *threads = running;
    for finished in done {
        let _ = finished.join();
    }
    threads.push(thread);
}

/// What a library's `unload` hook answers: [`Status::GUATIAO_ERR_BUSY`] while a thread a connection started still runs,
/// else the threads are joined and it is `GUATIAO_OK`.
#[must_use]
pub fn threads_done() -> Status {
    let mut threads = THREADS.lock().unwrap_or_else(PoisonError::into_inner);
    if threads.iter().any(|t| !t.is_finished()) {
        return Status::GUATIAO_ERR_BUSY;
    }
    for thread in threads.drain(..) {
        let _ = thread.join();
    }
    Status::GUATIAO_OK
}

/// What runs once `serve` has returned, before the spawner is let go.
type Ending = Box<dyn FnOnce() + Send>;

/// How a connection is made: the limits of the channel and of `serve`.
#[must_use]
pub struct Serving {
    settings: Settings,
    max_message_bytes: usize,
    ending: Option<Ending>,
}

impl core::fmt::Debug for Serving {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Serving")
            .field("settings", &self.settings)
            .field("max_message_bytes", &self.max_message_bytes)
            .finish_non_exhaustive()
    }
}

impl Serving {
    /// A connection served under `settings`, taking messages of at most `max_message_bytes`; one byte more ends it with
    /// `GUATIAO_ERR_BAD_VALUE`.
    pub fn new(mut settings: Settings, max_message_bytes: usize) -> Self {
        settings.max_line = u64::try_from(max_message_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        Self {
            settings,
            max_message_bytes,
            ending: None,
        }
    }

    /// Runs `ending` once, on the serving thread, after `serve` has returned and the harness and its programs are
    /// gone, and before the spawner is let go and the sink told the connection has closed: the place for a last look at
    /// an account's files and for destroying what the connection holds besides the spawner.
    pub fn ending(mut self, ending: impl FnOnce() + Send + 'static) -> Self {
        self.ending = Some(Box::new(ending));
        self
    }

    /// Starts a connection: `harness` is built from the `Spawn` that starts programs through `spawner`, and `serve` runs
    /// it, sending what it says to `sink`. Returns once the channel takes messages.
    ///
    /// `spawner` and `sink` are destroyed in every case, `spawner` before `sink`'s `closed` is called.
    ///
    /// # Errors
    ///
    /// `harness` refused, or no thread could be started.
    pub fn open(
        self,
        spawner: Object<dyn ProcessSpawner>,
        sink: Object<dyn ChannelSink>,
        harness: impl FnOnce(Arc<dyn Spawn>) -> Result<Arc<dyn Harness>, ProviderError>,
    ) -> Result<Object<dyn Channel>, ProviderError> {
        self.open_shared(Arc::new(spawner), sink, harness)
    }

    /// [`open`](Self::open) for a spawner someone else holds too, such as the file methods of an account's hold: it is
    /// destroyed when the last holder lets go, so they let go in [`ending`](Self::ending).
    ///
    /// # Errors
    ///
    /// `harness` refused, or no thread could be started.
    pub fn open_shared(
        mut self,
        spawner: Arc<Object<dyn ProcessSpawner>>,
        sink: Object<dyn ChannelSink>,
        harness: impl FnOnce(Arc<dyn Spawn>) -> Result<Arc<dyn Harness>, ProviderError>,
    ) -> Result<Object<dyn Channel>, ProviderError> {
        // A notice goes to the host through the spawner, which the connection must not outlive: the route holds it weakly,
        // so that the spawner is destroyed when its holders let go, and a notice after that is nobody's.
        let noticing = Arc::downgrade(&spawner);
        self.settings.on_notice.get_or_insert_with(|| {
            Arc::new(move |kind, text: &str| {
                if let Some(spawner) = noticing.upgrade() {
                    let _ = spawner.notice(HarnessNotice::new(kind, text));
                }
            })
        });
        let lent = Arc::new(Lent {
            spawner: Mutex::new(Some(ThroughSpawner::shared(spawner))),
        });
        let harness = harness(Arc::clone(&lent) as Arc<dyn Spawn>)?;
        let shared = Arc::new(Shared::default());
        let Self {
            settings,
            max_message_bytes,
            ending,
        } = self;
        let served = Arc::clone(&shared);
        let serving = std::thread::Builder::new()
            .name("coqui-acp-serve".into())
            .spawn(move || {
                let result = serve(
                    harness,
                    settings,
                    Input {
                        shared: Arc::clone(&served),
                        part: Vec::new(),
                        at: 0,
                    },
                    Output {
                        shared: Arc::clone(&served),
                        line: Vec::new(),
                    },
                );
                if let Some(ending) = ending {
                    ending();
                }
                lent.release();
                let mut state = served.lock();
                if state.status.is_none() {
                    state.status = Some(match result {
                        Ok(()) => (Status::GUATIAO_OK.into(), String::new()),
                        Err(error) => (Status::GUATIAO_ERR_INTERNAL.into(), error.to_string()),
                    });
                }
                state.served = true;
                drop(state);
                served.changed.notify_all();
            })
            .map_err(|_| {
                ProviderError::new(Status::GUATIAO_ERR_INTERNAL, "no thread to serve on")
            })?;
        track(serving);
        let delivered = Arc::clone(&shared);
        let delivering = std::thread::Builder::new()
            .name("coqui-acp-deliver".into())
            .spawn(move || deliver(sink, &delivered));
        match delivering {
            Ok(thread) => track(thread),
            Err(_) => {
                shared.end(
                    Status::GUATIAO_ERR_INTERNAL.into(),
                    "no thread to deliver on",
                );
                return Err(ProviderError::new(
                    Status::GUATIAO_ERR_INTERNAL,
                    "no thread to deliver on",
                ));
            }
        }
        Ok(Link {
            shared,
            max_message_bytes,
        }
        .into_object())
    }
}

/// The spawner of the connection, until the connection has ended and lets it go.
struct Lent {
    spawner: Mutex<Option<ThroughSpawner>>,
}

impl Lent {
    fn release(&self) {
        drop(
            self.spawner
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
    }
}

impl Spawn for Lent {
    fn spawn(&self, launch: &Launch) -> io::Result<Spawned> {
        let held = self.spawner.lock().unwrap_or_else(PoisonError::into_inner);
        match held.as_ref() {
            Some(spawner) => spawner.spawn(launch),
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the connection has ended",
            )),
        }
    }
}

#[derive(Default)]
struct State {
    /// Lines for `serve` to read, each ending in a line feed.
    inbound: VecDeque<Vec<u8>>,
    /// Lines `serve` wrote, for the sink.
    outbound: VecDeque<Vec<u8>>,
    /// No more messages are taken: the host closed, or the connection failed.
    ended: bool,
    /// The sink refused a message: what `serve` writes is dropped.
    dead: bool,
    /// How the connection ended, once it has.
    status: Option<(u32, String)>,
    /// `serve` has returned and its harness and spawner are let go.
    served: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ends the connection with `status` and why, unless it ended: nothing more is taken and what is queued is dropped.
    fn end(&self, status: u32, reason: &str) {
        let mut state = self.lock();
        state.ended = true;
        state.inbound.clear();
        state
            .status
            .get_or_insert_with(|| (status, reason.to_string()));
        drop(state);
        self.changed.notify_all();
    }
}

/// The host's end of the connection.
struct Link {
    shared: Arc<Shared>,
    max_message_bytes: usize,
}

/// A message as one line: a raw line feed or carriage return in one that is not JSON is replaced by a character no
/// JSON text holds, so it stays one line and stays invalid; in JSON they are only white space.
fn line_of(bytes: &[u8]) -> Vec<u8> {
    let mut line = if bytes.iter().any(|b| matches!(b, b'\n' | b'\r')) {
        match serde_json::from_slice::<Value>(bytes) {
            Ok(value) => serde_json::to_vec(&value).unwrap_or_else(|_| bytes.to_vec()),
            Err(_) => bytes
                .iter()
                .map(|b| if matches!(b, b'\n' | b'\r') { 0 } else { *b })
                .collect(),
        }
    } else {
        bytes.to_vec()
    };
    line.push(b'\n');
    line
}

impl Channel for Link {
    fn send(&self, bytes: &[u8]) -> Result<(), ProviderError> {
        if bytes.len() > self.max_message_bytes {
            const WHY: &str = "the message is larger than this provider takes";
            self.shared.end(Status::GUATIAO_ERR_BAD_VALUE.into(), WHY);
            return Err(ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, WHY));
        }
        let line = line_of(bytes);
        let mut state = self.shared.lock();
        loop {
            if state.ended {
                return Err(ProviderError::new(
                    Status::GUATIAO_ERR_GONE,
                    "the connection is closed",
                ));
            }
            // The intake also waits while the sink holds the answers back, so a host that does not read
            // is not given more work to answer.
            if state.inbound.len() < QUEUE && state.outbound.len() < QUEUE {
                break;
            }
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.inbound.push_back(line);
        drop(state);
        self.shared.changed.notify_all();
        Ok(())
    }

    fn close(&self, status: u32, reason: &str) {
        self.shared.end(status, reason);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.shared.end(Status::GUATIAO_OK.into(), "");
    }
}

/// What `serve` reads: the host's messages, a line each.
struct Input {
    shared: Arc<Shared>,
    part: Vec<u8>,
    at: usize,
}

impl Read for Input {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.at >= self.part.len() {
            let mut state = self.shared.lock();
            loop {
                if let Some(line) = state.inbound.pop_front() {
                    self.part = line;
                    self.at = 0;
                    drop(state);
                    self.shared.changed.notify_all();
                    break;
                }
                if state.ended {
                    return Ok(0);
                }
                state = self
                    .shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
        let n = buf.len().min(self.part.len() - self.at);
        buf[..n].copy_from_slice(&self.part[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// What `serve` writes: lines, handed to the delivery thread, each waiting for room when the sink is slow.
struct Output {
    shared: Arc<Shared>,
    line: Vec<u8>,
}

impl Write for Output {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.line.extend_from_slice(buf);
        while let Some(end) = self.line.iter().position(|b| *b == b'\n') {
            let mut message: Vec<u8> = self.line.drain(..=end).collect();
            message.pop();
            if message.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let mut state = self.shared.lock();
            while state.outbound.len() >= QUEUE && !state.dead {
                state = self
                    .shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if !state.dead {
                state.outbound.push_back(message);
            }
            drop(state);
            self.shared.changed.notify_all();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The only caller of the sink: hands over every line `serve` wrote, and when `serve` has ended and the lines are all
/// handed over, says the connection has closed.
fn deliver(mut sink: Object<dyn ChannelSink>, shared: &Shared) {
    loop {
        let message = {
            let mut state = shared.lock();
            loop {
                if let Some(message) = state.outbound.pop_front() {
                    break Some(message);
                }
                if state.served {
                    break None;
                }
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        shared.changed.notify_all();
        let Some(message) = message else { break };
        if shared.lock().dead {
            continue;
        }
        if let Err(refused) = sink.receive(&message) {
            let mut state = shared.lock();
            state.dead = true;
            state.ended = true;
            state.inbound.clear();
            state.outbound.clear();
            state.status = Some((u32::from(refused.status), refused.message().to_owned()));
            drop(state);
            shared.changed.notify_all();
        }
    }
    let (status, reason) = shared
        .lock()
        .status
        .clone()
        .unwrap_or((Status::GUATIAO_OK.into(), String::new()));
    sink.closed(status, &reason);
}

/// What a provider keeps of the usage its connections learned, by account, for the kind's `usage` method: the buckets
/// and limits each connection's report held, merged as a connection merges them, so a repeat of figures already held
/// changes nothing and the revision only grows.
#[derive(Debug, Default)]
pub struct Reports {
    accounts: Mutex<BTreeMap<String, UsageTracker>>,
}

impl Reports {
    /// Nothing seen.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn held(&self) -> MutexGuard<'_, BTreeMap<String, UsageTracker>> {
        self.accounts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the report a connection on `account` has now.
    pub fn observe(&self, support: &UsageSupport, account: Option<&str>, report: &UsageReport) {
        let now = unix_now();
        let mut held = self.held();
        let tracker = held
            .entry(account.unwrap_or_default().to_string())
            .or_insert_with(|| UsageTracker::new(support.clone(), account.map(str::to_string)));
        tracker.buckets(report.buckets.clone(), now);
        if let Some(hit) = &report.limit {
            tracker.limit(hit.clone(), now);
        }
    }

    /// Takes buckets a reading of `account` gave, and answers the report with them.
    pub fn read(
        &self,
        support: &UsageSupport,
        account: Option<&str>,
        buckets: Vec<Bucket>,
    ) -> UsageReport {
        let now = unix_now();
        let mut held = self.held();
        let tracker = held
            .entry(account.unwrap_or_default().to_string())
            .or_insert_with(|| UsageTracker::new(support.clone(), account.map(str::to_string)));
        tracker.buckets(buckets, now);
        tracker.snapshot(None, now)
    }

    /// The report of `account` as far as it is known: what its connections and readings gave, else an empty one that
    /// names the sources `support` lists.
    #[must_use]
    pub fn report(&self, support: &UsageSupport, account: Option<&str>) -> UsageReport {
        let now = unix_now();
        match self.held().get(account.unwrap_or_default()) {
            Some(tracker) => tracker.snapshot(None, now),
            None => {
                UsageTracker::new(support.clone(), account.map(str::to_string)).snapshot(None, now)
            }
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

/// Boxes a harness for [`Serving::open`].
#[must_use]
pub fn shared(harness: impl Harness) -> Arc<dyn Harness> {
    let boxed: Box<dyn Harness> = Box::new(harness);
    Arc::from(boxed)
}

/// A harness's `Takes` as the kind's: the same, and `session/resume`, which every harness served here answers.
#[must_use]
pub fn takes_of(takes: &crate::acp::Takes) -> coqui_acp_api::Takes {
    let mut kind = coqui_acp_api::Takes::default();
    kind.images = takes.images;
    kind.embedded_context = takes.embedded_context;
    kind.mcp_http = takes.mcp_http;
    kind.mcp_sse = takes.mcp_sse;
    kind.load = takes.load;
    kind.resume = true;
    kind.additional_directories = takes.additional_directories;
    kind.system_prompt = takes.system_prompt;
    kind
}

/// A JSON Schema, or any JSON, as a Guatiao value: what a catalog's `settings` and `inputs` hold.
///
/// # Panics
///
/// `json` holds a number Guatiao cannot, which no schema written by hand does.
#[must_use]
pub fn schema(json: &Value) -> coqui_acp_api::guatiao::Value {
    guatiao_serde::from_serde(json.clone()).expect("a schema is a value")
}

/// The schema of a setting that is a credential: text, marked `writeOnly` and `x-sensitive`, with no default.
#[must_use]
pub fn credential() -> Value {
    serde_json::json!({"type": "string", "writeOnly": true, "x-sensitive": true})
}

/// Whether a provider that applies nothing to the harness's environment can take `account`: why not, when it cannot.
///
/// Only [`HARNESS_LOGIN`](coqui_acp_api::HARNESS_LOGIN) hands nothing over and clears nothing, so only it is taken; any
/// other kind the catalog lists is refused with the reason, as the kind says for one that cannot be applied.
///
/// # Errors
///
/// `GUATIAO_ERR_BAD_VALUE`: the id is not an account id, or the kind is not one the catalog lists.
pub fn account_refusal(
    catalog: &coqui_acp_api::AccountCatalog,
    account: &coqui_acp_api::AccountRef,
) -> Result<Option<String>, ProviderError> {
    account.check_id()?;
    let Some(kind) = catalog.kind(&account.kind) else {
        return Err(ProviderError::new(
            Status::GUATIAO_ERR_BAD_VALUE,
            "this harness takes no account of that kind",
        ));
    };
    Ok((kind.id != coqui_acp_api::HARNESS_LOGIN).then(|| {
        alloc::format!(
            "this provider does not apply an account of kind `{}` yet: only `{}` is taken",
            kind.id,
            coqui_acp_api::HARNESS_LOGIN
        )
    }))
}

/// A Guatiao value as JSON; `null` when it holds something JSON cannot: what a request's settings are read as.
#[must_use]
pub fn to_json(value: &coqui_acp_api::guatiao::Value) -> Value {
    serde_json::to_value(guatiao_serde::Serializable::from(value)).unwrap_or(Value::Null)
}
