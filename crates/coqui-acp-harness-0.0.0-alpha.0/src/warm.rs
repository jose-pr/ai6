//! Agent processes kept ready ahead of the session that will use them.
//!
//! A [`WarmPool`] holds started, initialised agents (whatever a translator calls one) under a [`WarmKey`] that names everything
//! fixed when the process starts. A `session/new` whose key matches takes one with [`WarmPool::take`] and a replacement is
//! started behind it; a key that matches none starts a process as it always did. How many to keep ahead and for how long is the
//! host's, a [`WarmPolicy`]; [`drop_all`] stops every warm process of every pool, for a host under memory pressure or ending.
//!
//! A warm process holds no session and is shared by every provider instance of the process that names the same key.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::time::Duration;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Instant;

use crate::event::Event;
use crate::process::Launch;

/// How often a pool looks for a warm process that has exited, which it then forgets.
const POLL: Duration = Duration::from_secs(1);

/// The most events a warm process may say before it is taken; past it the process is not handed out.
const HELD_EVENTS: usize = 1024;

/// How many warm processes to keep ahead of `session/new`, and for how long. The default keeps none.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct WarmPolicy {
    /// How many to keep ahead for each key; 0 turns warming off (the default): every session starts its own process.
    pub ahead: usize,
    /// How long a warm process may wait unused before it is stopped. Default 5 minutes; zero stops one at the next look.
    pub idle: Duration,
    /// Where the processes run. Instances share warm processes only when their scopes are equal, so a host that starts
    /// programs through more than one spawner (another machine, a container) gives each its own. Default empty.
    pub scope: String,
}

impl WarmPolicy {
    /// Keeps `ahead` processes for each key, each for at most `idle` unused, in the empty scope.
    #[must_use]
    pub const fn new(ahead: usize, idle: Duration) -> Self {
        Self {
            ahead,
            idle,
            scope: String::new(),
        }
    }

    /// Names where the processes run, as [`scope`](Self::scope) says.
    #[must_use]
    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = scope.into();
        self
    }
}

impl Default for WarmPolicy {
    fn default() -> Self {
        Self::new(0, Duration::from_secs(300))
    }
}

/// Everything that fixes a process when it starts: the policy's scope, the program, its arguments, directory and
/// environment, and any more a translator adds with [`part`](Self::part). Two keys are equal when a process started for one
/// would be the same as one started for the other.
///
/// `Debug` shows the program and the names of the variables, never their values, which may be credentials.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WarmKey {
    scope: String,
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    env: BTreeMap<OsString, OsString>,
    remove_env: Vec<OsString>,
    clear_env: bool,
    parts: Vec<Vec<u8>>,
}

impl core::fmt::Debug for WarmKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WarmKey")
            .field("scope", &self.scope)
            .field("program", &self.program)
            .field("cwd", &self.cwd)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl WarmKey {
    /// The key of a process started as `launch` says, in `policy`'s scope.
    #[must_use]
    pub fn new(policy: &WarmPolicy, launch: &Launch) -> Self {
        let mut remove_env = launch.remove_env.clone();
        remove_env.sort();
        remove_env.dedup();
        Self {
            scope: policy.scope.clone(),
            program: launch.program.clone(),
            args: launch.args.clone(),
            cwd: launch.cwd.clone(),
            env: launch.env.clone(),
            remove_env,
            clear_env: launch.clear_env,
            parts: Vec::new(),
        }
    }

    /// Leaves the working directory out, for a program that is told its directory later.
    #[must_use]
    pub fn without_cwd(mut self) -> Self {
        self.cwd = None;
        self
    }

    /// Adds what else fixes the process: the contents of a file named in its arguments, say.
    #[must_use]
    pub fn part(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.parts.push(bytes.into());
        self
    }
}

/// What a [`WarmPool`] keeps: a started process that is waiting for its session.
pub trait Warmed: Send + 'static {
    /// Whether it can still be handed out: false once its process has exited or it has been spoilt.
    fn ready(&self) -> bool;

    /// Ends the process and everything it started.
    fn stop(&self);
}

/// Starts one warm process, `None` when it could not be started. It runs on a thread of the pool's, and starts the process
/// through the spawner the host gave.
pub type Maker<T> = Arc<dyn Fn() -> Option<T> + Send + Sync>;

struct Slot<T> {
    held: Vec<(Instant, T)>,
    /// How many are being started.
    starting: usize,
    ahead: usize,
    idle: Duration,
}

struct State<T> {
    slots: HashMap<WarmKey, Slot<T>>,
    /// Counts [`WarmPool::drop_all`]s: a process started before one is stopped when it arrives.
    generation: u64,
    reaping: bool,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Warm processes of one kind, by [`WarmKey`]. Keep one in a `static` and let every provider instance of the process use it.
pub struct WarmPool<T: Warmed> {
    shared: Arc<Shared<T>>,
}

impl<T: Warmed> core::fmt::Debug for WarmPool<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WarmPool").finish_non_exhaustive()
    }
}

impl<T: Warmed> Default for WarmPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A pool as [`drop_all`] reaches it, whatever it holds.
trait Clearing: Send + Sync {
    fn clear(&self);
}

static POOLS: Mutex<Vec<Weak<dyn Clearing>>> = Mutex::new(Vec::new());

/// Stops every warm process of every pool in this process, and any being started when this is called as it arrives. For a host
/// under memory pressure, or ending.
pub fn drop_all() {
    let pools: Vec<Arc<dyn Clearing>> = {
        let mut pools = locked(&POOLS);
        pools.retain(|pool| pool.strong_count() > 0);
        pools.iter().filter_map(Weak::upgrade).collect()
    };
    for pool in pools {
        pool.clear();
    }
}

impl<T: Warmed> Clearing for Shared<T> {
    fn clear(&self) {
        let stopping: Vec<T> = {
            let mut state = locked(&self.state);
            state.generation += 1;
            state
                .slots
                .drain()
                .flat_map(|(_, slot)| slot.held)
                .map(|(_, item)| item)
                .collect()
        };
        self.changed.notify_all();
        for item in stopping {
            item.stop();
        }
    }
}

impl<T: Warmed> WarmPool<T> {
    /// An empty pool, which [`drop_all`] reaches.
    #[must_use]
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                slots: HashMap::new(),
                generation: 0,
                reaping: false,
            }),
            changed: Condvar::new(),
        });
        let registered: Arc<dyn Clearing> = shared.clone();
        locked(&POOLS).push(Arc::downgrade(&registered));
        Self { shared }
    }

    /// Takes a warm process for `key` when one is ready. With `policy.ahead` 0 it answers `None`. A process that has exited is
    /// never handed out. It starts nothing: [`fill`](Self::fill) after the session has what it needs, so that the replacement
    /// does not compete with a start the person waits for.
    pub fn take(&self, key: &WarmKey, policy: &WarmPolicy) -> Option<T> {
        if policy.ahead == 0 {
            return None;
        }
        let (taken, dead) = {
            let mut state = locked(&self.shared.state);
            let slot = slot_of(&mut state, key, policy);
            let dead = purge(slot);
            ((!slot.held.is_empty()).then(|| slot.held.remove(0).1), dead)
        };
        for item in dead {
            item.stop();
        }
        taken
    }

    /// Starts warm processes for `key` until `policy.ahead` are ready or being started, each on a thread of its own: for a
    /// host that knows what a session will ask for before it asks.
    pub fn fill(&self, key: &WarmKey, policy: &WarmPolicy, make: &Maker<T>) {
        if policy.ahead == 0 {
            return;
        }
        let (missing, generation, dead, reaper) = {
            let mut state = locked(&self.shared.state);
            let generation = state.generation;
            let slot = slot_of(&mut state, key, policy);
            let dead = purge(slot);
            let missing = slot.ahead.saturating_sub(slot.held.len() + slot.starting);
            slot.starting += missing;
            let reaper = !state.reaping && missing > 0;
            state.reaping |= reaper;
            (missing, generation, dead, reaper)
        };
        for item in dead {
            item.stop();
        }
        if reaper {
            let shared = Arc::clone(&self.shared);
            if thread::Builder::new()
                .name("coqui-acp-warm-reaper".into())
                .spawn(move || reap(&shared))
                .is_err()
            {
                locked(&self.shared.state).reaping = false;
            }
        }
        for _ in 0..missing {
            let shared = Arc::clone(&self.shared);
            let owned = key.clone();
            let make = Arc::clone(make);
            let started = thread::Builder::new()
                .name("coqui-acp-warm".into())
                .spawn(move || {
                    let made = make();
                    arrive(&shared, &owned, generation, made);
                });
            if started.is_err() {
                let mut state = locked(&self.shared.state);
                if state.generation == generation
                    && let Some(slot) = state.slots.get_mut(key)
                {
                    slot.starting = slot.starting.saturating_sub(1);
                }
            }
        }
    }

    /// How many processes are warm and ready for `key`: those being started and those that have exited are not counted.
    #[must_use]
    pub fn ready(&self, key: &WarmKey) -> usize {
        let (count, dead) = {
            let mut state = locked(&self.shared.state);
            match state.slots.get_mut(key) {
                Some(slot) => {
                    let dead = purge(slot);
                    (slot.held.len(), dead)
                }
                None => (0, Vec::new()),
            }
        };
        for item in dead {
            item.stop();
        }
        count
    }

    /// Stops every warm process of this pool, and any being started as it arrives. [`drop_all`] does it for every pool.
    pub fn drop_all(&self) {
        self.shared.clear();
    }
}

impl<T: Warmed> Drop for WarmPool<T> {
    fn drop(&mut self) {
        self.shared.clear();
    }
}

/// The slot of `key`, made when there is none, with `policy`'s numbers.
fn slot_of<'a, T>(state: &'a mut State<T>, key: &WarmKey, policy: &WarmPolicy) -> &'a mut Slot<T> {
    let slot = state.slots.entry(key.clone()).or_insert_with(|| Slot {
        held: Vec::new(),
        starting: 0,
        ahead: policy.ahead,
        idle: policy.idle,
    });
    slot.ahead = policy.ahead;
    slot.idle = policy.idle;
    slot
}

/// Takes out the processes that cannot be handed out any more, for the caller to stop with no lock held.
fn purge<T: Warmed>(slot: &mut Slot<T>) -> Vec<T> {
    let mut dead = Vec::new();
    let mut index = 0;
    while index < slot.held.len() {
        if slot.held[index].1.ready() {
            index += 1;
        } else {
            dead.push(slot.held.remove(index).1);
        }
    }
    dead
}

/// A started process arrives: kept, unless a drop of everything came while it started.
fn arrive<T: Warmed>(shared: &Shared<T>, key: &WarmKey, generation: u64, made: Option<T>) {
    let rejected = {
        let mut state = locked(&shared.state);
        let current = state.generation == generation;
        match state.slots.get_mut(key) {
            Some(slot) if current => {
                slot.starting = slot.starting.saturating_sub(1);
                if let Some(item) = made {
                    slot.held.push((Instant::now(), item));
                }
                None
            }
            _ => made,
        }
    };
    shared.changed.notify_all();
    if let Some(item) = rejected {
        item.stop();
    }
}

/// Stops what has waited past its idle time or has exited, until nothing is kept or started.
fn reap<T: Warmed>(shared: &Shared<T>) {
    let mut state = locked(&shared.state);
    loop {
        let now = Instant::now();
        let mut stale = Vec::new();
        let mut next: Option<Duration> = None;
        state.slots.retain(|_, slot| {
            let idle = slot.idle;
            let mut kept = Vec::new();
            for (since, item) in slot.held.drain(..) {
                let waited = now.saturating_duration_since(since);
                if item.ready() && waited < idle {
                    let left = idle - waited;
                    next = Some(next.map_or(left, |next| next.min(left)));
                    kept.push((since, item));
                } else {
                    stale.push(item);
                }
            }
            slot.held = kept;
            !slot.held.is_empty() || slot.starting > 0
        });
        if !stale.is_empty() {
            drop(state);
            for item in stale {
                item.stop();
            }
            state = locked(&shared.state);
            continue;
        }
        if state.slots.is_empty() {
            state.reaping = false;
            return;
        }
        let wait = next.map_or(POLL, |next| next.min(POLL));
        state = shared
            .changed
            .wait_timeout(state, wait)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/// A sink that holds what a warm process says until a session takes it, then hands it all over and passes the rest on.
///
/// Give [`sink`](Self::sink) to the process when it starts, and [`connect`](Self::connect) the session's own when it is taken.
#[derive(Clone)]
pub struct Relay {
    state: Arc<Mutex<Relayed>>,
}

enum Relayed {
    Holding {
        events: Vec<Event>,
        overflowed: bool,
    },
    Connected(Arc<dyn Fn(Event) -> bool + Send + Sync>),
}

impl core::fmt::Debug for Relay {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Relay").finish_non_exhaustive()
    }
}

impl Default for Relay {
    fn default() -> Self {
        Self::new()
    }
}

impl Relay {
    /// A relay that holds.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(Relayed::Holding {
                events: Vec::new(),
                overflowed: false,
            })),
        }
    }

    /// The sink to start the process with.
    pub fn sink(&self) -> impl Fn(Event) -> bool + Send + Sync + 'static {
        let state = Arc::clone(&self.state);
        move |event| {
            let sink = {
                let mut state = locked(&state);
                match &mut *state {
                    Relayed::Holding { events, overflowed } => {
                        if events.len() < HELD_EVENTS {
                            events.push(event);
                        } else {
                            *overflowed = true;
                        }
                        return true;
                    }
                    Relayed::Connected(sink) => Arc::clone(sink),
                }
            };
            sink(event)
        }
    }

    /// Whether the process said more than the relay holds while it waited, so that it is not the process a session expects.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        matches!(
            &*locked(&self.state),
            Relayed::Holding {
                overflowed: true,
                ..
            }
        )
    }

    /// Hands what was held to `sink`, in order, and has it called with everything the process says from now on.
    pub fn connect(&self, sink: impl Fn(Event) -> bool + Send + Sync + 'static) {
        let sink: Arc<dyn Fn(Event) -> bool + Send + Sync> = Arc::new(sink);
        let mut state = locked(&self.state);
        if let Relayed::Holding { events, .. } = &mut *state {
            let mut open = true;
            for event in core::mem::take(events) {
                if open {
                    open = sink(event);
                }
            }
        }
        *state = Relayed::Connected(sink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Toy {
        alive: Arc<AtomicBool>,
        stopped: Arc<AtomicBool>,
    }

    impl Warmed for Toy {
        fn ready(&self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
        fn stop(&self) {
            self.stopped.store(true, Ordering::SeqCst);
            self.alive.store(false, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct Made {
        count: AtomicUsize,
        flags: Mutex<Vec<(Arc<AtomicBool>, Arc<AtomicBool>)>>,
    }

    fn maker(made: &Arc<Made>) -> Maker<Toy> {
        let made = Arc::clone(made);
        Arc::new(move || {
            made.count.fetch_add(1, Ordering::SeqCst);
            let alive = Arc::new(AtomicBool::new(true));
            let stopped = Arc::new(AtomicBool::new(false));
            locked(&made.flags).push((Arc::clone(&alive), Arc::clone(&stopped)));
            Some(Toy { alive, stopped })
        })
    }

    fn key(program: &str) -> WarmKey {
        WarmKey::new(&WarmPolicy::default(), &Launch::new(program))
    }

    fn until(what: &str, done: impl Fn() -> bool) {
        let limit = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < limit, "{what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_pool_with_nothing_ahead_starts_nothing() {
        let pool = WarmPool::new();
        let made = Arc::new(Made::default());
        let policy = WarmPolicy::default();
        assert!(pool.take(&key("a"), &policy).is_none());
        pool.fill(&key("a"), &policy, &maker(&made));
        thread::sleep(Duration::from_millis(50));
        assert_eq!(made.count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_taken_process_is_replaced_and_another_key_is_not_served_by_it() {
        let pool = WarmPool::new();
        let made = Arc::new(Made::default());
        let policy = WarmPolicy::new(1, Duration::from_secs(60));
        let make = maker(&made);
        assert!(
            pool.take(&key("a"), &policy).is_none(),
            "nothing was warm yet"
        );
        pool.fill(&key("a"), &policy, &make);
        until("the first starts", || pool.ready(&key("a")) == 1);
        assert!(pool.take(&key("b"), &policy).is_none());
        assert!(pool.take(&key("a"), &policy).is_some());
        assert_eq!(pool.ready(&key("a")), 0);
        pool.fill(&key("a"), &policy, &make);
        until("a replacement starts", || pool.ready(&key("a")) == 1);
        assert_eq!(made.count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_process_that_exited_is_not_handed_out_or_counted() {
        let pool = WarmPool::new();
        let made = Arc::new(Made::default());
        let policy = WarmPolicy::new(1, Duration::from_secs(60));
        let make = maker(&made);
        pool.fill(&key("a"), &policy, &make);
        until("it starts", || pool.ready(&key("a")) == 1);
        locked(&made.flags)[0].0.store(false, Ordering::SeqCst);
        assert_eq!(pool.ready(&key("a")), 0);
        assert!(
            locked(&made.flags)[0].1.load(Ordering::SeqCst),
            "and it is stopped"
        );
        assert!(pool.take(&key("a"), &policy).is_none());
        pool.fill(&key("a"), &policy, &make);
        until("a live one replaces it", || pool.ready(&key("a")) == 1);
    }

    #[test]
    fn an_idle_process_is_stopped_and_drop_all_stops_the_rest() {
        let pool = WarmPool::new();
        let made = Arc::new(Made::default());
        let make = maker(&made);
        pool.fill(
            &key("a"),
            &WarmPolicy::new(1, Duration::from_millis(50)),
            &make,
        );
        until("it starts", || locked(&made.flags).len() == 1);
        until("it is stopped for idling", || {
            locked(&made.flags)[0].1.load(Ordering::SeqCst)
        });
        assert_eq!(pool.ready(&key("a")), 0);
        let policy = WarmPolicy::new(2, Duration::from_secs(60));
        pool.fill(&key("b"), &policy, &make);
        until("two start", || pool.ready(&key("b")) == 2);
        pool.drop_all();
        assert_eq!(pool.ready(&key("b")), 0);
        assert!(
            locked(&made.flags)[1..]
                .iter()
                .all(|flags| flags.1.load(Ordering::SeqCst))
        );
    }

    #[test]
    fn a_process_still_starting_when_everything_is_dropped_is_stopped_on_arrival() {
        let pool = WarmPool::new();
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let stopped = Arc::new(AtomicBool::new(false));
        let made_stopped = Arc::clone(&stopped);
        let make: Maker<Toy> = Arc::new(move || {
            let _ = locked(&gate).recv();
            Some(Toy {
                alive: Arc::new(AtomicBool::new(true)),
                stopped: Arc::clone(&made_stopped),
            })
        });
        pool.fill(
            &key("a"),
            &WarmPolicy::new(1, Duration::from_secs(60)),
            &make,
        );
        pool.drop_all();
        release.send(()).unwrap();
        until("it is stopped", || stopped.load(Ordering::SeqCst));
        assert_eq!(pool.ready(&key("a")), 0);
    }

    #[test]
    fn keys_differ_by_scope_directory_environment_and_parts() {
        let launch = Launch::new("p").arg("x");
        let base = WarmKey::new(&WarmPolicy::default(), &launch);
        assert_eq!(base, WarmKey::new(&WarmPolicy::default(), &launch));
        assert_ne!(
            base,
            WarmKey::new(&WarmPolicy::default().scope("far"), &launch)
        );
        assert_ne!(
            base,
            WarmKey::new(&WarmPolicy::default(), &launch.clone().cwd("/w"))
        );
        assert_eq!(
            base,
            WarmKey::new(&WarmPolicy::default(), &launch.clone().cwd("/w")).without_cwd()
        );
        assert_ne!(base, base.clone().part("m"));
        let mut with_env = launch.clone();
        with_env.env.insert("A".into(), "1".into());
        let keyed = WarmKey::new(&WarmPolicy::default(), &with_env);
        assert_ne!(base, keyed);
        assert!(!format!("{keyed:?}").contains('1'), "no value is shown");
    }

    #[test]
    fn a_relay_holds_then_hands_over_in_order_and_passes_the_rest_on() {
        let relay = Relay::new();
        let sink = relay.sink();
        assert!(sink(Event::Notice {
            severity: crate::event::Severity::Info,
            title: "one".into()
        }));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let told = Arc::clone(&seen);
        relay.connect(move |event| {
            if let Event::Notice { title, .. } = event {
                locked(&told).push(title);
            }
            true
        });
        sink(Event::Notice {
            severity: crate::event::Severity::Info,
            title: "two".into(),
        });
        assert_eq!(*locked(&seen), ["one", "two"]);
        assert!(!relay.overflowed());
    }
}
