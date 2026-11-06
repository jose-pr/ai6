//! The field runner: `ai6 runner`, the AI6 binary's Theater mode.
//!
//! A runner supervises agent harnesses on the Theater they run on, so a Session outlives the
//! Station that started it. It loads the same AgentHandler and Terminal providers a Station
//! does and has no scheduler, Registry or roster: a Station starts Sessions on it, reads their
//! events and sends them messages over the Agent Client Protocol (ACP) on a loopback port. A
//! connection authenticates with the `ai6-runner-token` method, the bearer token kept in the
//! state directory. A Station that reaches the Theater through a transport opens `ai6 runner
//! connect` on a terminal there, which authenticates for it and relays the stream.
//!
//! Every event a harness emits is written to disk with a sequence number before any Station
//! can read it. A Station attaches to a Session by key with `session/load`, and the runner
//! replays the events after the one the Station last took, as `_ai6/event` notifications that
//! carry their sequence number; the Station acknowledges what it took with `_ai6/ack`. A
//! Station that comes back, or another one, loads again and replays what it missed; a newer
//! connection to a Session closes the older. A runner that restarts has lost the harnesses it
//! supervised; it ends their Sessions with a `lost` event, which a Station takes as the
//! Session lost.
//!
//! A Session the Station asks Cables for gets a Cable endpoint on the runner's loopback, which
//! carries the harness's requests over the attached Station's connection (`cable`).
//!
//! The state directory holds `runner.json` (where the runner listens, while it runs),
//! `token`, `runner.lock`, `settings.json` (the [`RunnerHandoff`] it runs by) and
//! `sessions/<key>/` with `events.jsonl`, `cursor` and `session.json`.

mod cable;
pub(crate) mod client;
mod spool;
mod wire;

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use guatiao::library::Object;
use serde::{Deserialize, Serialize};

use crate::directorates::m::identity::SessionId;
use crate::directorates::q::terminal::{HarnessStdio, TerminalBridge};
use crate::directorates::section_00::acp::HandlerSpec;
use crate::directorates::section_00::acp::ends_turn;
use crate::directorates::section_00::agent::{AgentResume, AgentStart, checked_session_id};
use crate::station::limits::{ApiLimits, RunnerLimits};
use crate::station::plugins::Plugins;
use coqui_acp_provider::{AgentEvent, AgentSession};

#[cfg(feature = "cables-mcp")]
pub(crate) use cable::URL_KEY as CABLE_URL_KEY;
#[cfg(feature = "cables-mcp")]
pub(crate) use cable::{CableTunnel, TOKEN_KEY as CABLE_TOKEN_KEY};

const INFO_FILE: &str = "runner.json";
const TOKEN_FILE: &str = "token";
const LOCK_FILE: &str = "runner.lock";
const SESSIONS_DIR: &str = "sessions";
const SESSION_FILE: &str = "session.json";
/// The file in the state directory holding the [`RunnerHandoff`] the runner serves by, which
/// `ai6 runner connect` reads too.
pub const SETTINGS_FILE: &str = "settings.json";

/// The kind of the event that tells a connection another one took its Session over.
pub(crate) const DETACHED_EVENT: &str = "detached";
/// The exit status of `ai6 runner connect` when no runner answers it.
pub const CONNECT_NOT_RUNNING: u8 = 3;

pub use client::connect_stdio;

/// Puts this process's standard input in raw mode when it is a terminal, so ACP lines are not
/// echoed or rewritten on their way through it.
#[cfg(unix)]
fn raw_terminal() {
    use rustix::termios::{OptionalActions, isatty, tcgetattr, tcsetattr};
    let stdin = std::io::stdin();
    if isatty(&stdin)
        && let Ok(mut settings) = tcgetattr(&stdin)
    {
        settings.make_raw();
        let _ = tcsetattr(&stdin, OptionalActions::Now, &settings);
    }
}

#[cfg(not(unix))]
fn raw_terminal() {}

/// The kind of the event a restarted runner ends a Session it lost with: the harness is gone,
/// so the Station treats the Session as lost rather than failed.
pub(crate) const LOST_EVENT: &str = "lost";

/// Where a runner listens and what it was built from, as `runner.json` records it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerInfo {
    /// The loopback address the runner serves ACP on, `host:port`.
    pub address: String,
    /// The runner's process ID.
    pub pid: u32,
    /// The AI6 version of the runner.
    pub version: String,
}

impl RunnerInfo {
    /// Reads `runner.json` from a runner's state directory; absent while no runner serves it.
    pub fn read(state_dir: &Path) -> io::Result<Self> {
        let text = fs::read(state_dir.join(INFO_FILE))?;
        serde_json::from_slice(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

/// What a Station hands a runner it starts: the `[runner]` table and the `[providers.<id>]`
/// tables of its `station` record. `ai6 runner serve --settings PATH` reads it as JSON; a key
/// it does not name keeps its default.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, guatiao::ToValue, guatiao::FromValue)]
#[map(deny_unknown_fields)]
pub struct RunnerHandoff {
    /// How the runner serves.
    #[map(default)]
    pub runner: RunnerLimits,
    /// The Guatiao configuration each provider is built with when a request gives none.
    #[map(with = guatiao_serde::adapt::object, default)]
    pub providers: BTreeMap<String, serde_json::Value>,
}

impl RunnerHandoff {
    /// The settings `runner` and `providers`.
    pub fn new(runner: RunnerLimits, providers: BTreeMap<String, serde_json::Value>) -> Self {
        Self { runner, providers }
    }

    /// The settings as the JSON [`parse`](Self::parse) reads.
    ///
    /// # Errors
    ///
    /// A provider's configuration has no JSON form.
    pub fn to_json_bytes(&self) -> io::Result<Vec<u8>> {
        let document = crate::directorates::m::records::to_json(self)
            .ok_or_else(|| io::Error::other("the runner's settings have no JSON form"))?;
        serde_json::to_vec_pretty(&document).map_err(io::Error::other)
    }

    /// Reads the settings in `path`.
    ///
    /// # Errors
    ///
    /// The file cannot be read, or is not settings JSON.
    pub fn read(path: &Path) -> io::Result<Self> {
        Self::parse(&fs::read(path)?)
    }

    /// Reads settings JSON from `bytes`.
    ///
    /// # Errors
    ///
    /// `InvalidData` for anything but settings JSON with acceptable values.
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        let invalid = |reason: String| io::Error::new(io::ErrorKind::InvalidData, reason);
        let document: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|error| invalid(error.to_string()))?;
        let settings: Self =
            crate::directorates::m::records::from_json(&document).map_err(invalid)?;
        if settings.runner.replay_batch == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "runner.replay_batch: at least 1",
            ));
        }
        crate::station::plugins::provider_configs(&settings.providers)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(settings)
    }

    /// The settings recorded in the state directory `state_dir`, or the defaults when there
    /// are none.
    ///
    /// # Errors
    ///
    /// The file exists but cannot be read or parsed.
    pub fn in_state_dir(state_dir: &Path) -> io::Result<Self> {
        match Self::read(&state_dir.join(SETTINGS_FILE)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            read => read,
        }
    }
}

/// How `ai6 runner serve` runs.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    state_dir: PathBuf,
    listen: SocketAddr,
    plugin_paths: Vec<PathBuf>,
    plugin_policy: crate::station::plugins::PluginPolicy,
    terminal: Option<String>,
    settings: RunnerHandoff,
}

impl RunnerConfig {
    /// A runner keeping its state in `state_dir`, on a loopback port the host chooses.
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: state_dir.into(),
            listen: SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), 0),
            plugin_paths: Vec::new(),
            plugin_policy: crate::station::plugins::PluginPolicy::default(),
            terminal: None,
            settings: RunnerHandoff::default(),
        }
    }

    /// Serves within `limits` (default [`RunnerLimits::default`]).
    #[must_use]
    pub fn with_limits(mut self, limits: RunnerLimits) -> Self {
        self.settings.runner = limits;
        self
    }

    /// Builds provider `<id>` with `configs[<id>]` when a request gives no configuration of
    /// its own.
    #[must_use]
    pub fn with_provider_configs(mut self, configs: BTreeMap<String, serde_json::Value>) -> Self {
        self.settings.providers = configs;
        self
    }

    /// Listens on `address`, which must be a loopback address.
    #[must_use]
    pub fn with_listen(mut self, address: SocketAddr) -> Self {
        self.listen = address;
        self
    }

    /// Loads AgentHandler and Terminal providers from these directories or library files.
    #[must_use]
    pub fn with_plugin_paths(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.plugin_paths = paths.into_iter().collect();
        self
    }

    /// Sets how provider libraries are checked before they load (default: `report`, no
    /// pins), as the Station's own policy does.
    #[must_use]
    pub fn with_plugin_policy(mut self, policy: crate::station::plugins::PluginPolicy) -> Self {
        self.plugin_policy = policy;
        self
    }

    /// Pins the terminal provider harnesses run in, unless a Station names one per Session.
    #[must_use]
    pub fn with_terminal(mut self, terminal: impl Into<String>) -> Self {
        self.terminal = Some(terminal.into());
        self
    }
}

/// The runner's state directory when none is given: `runner` in AI6's state directory.
pub fn default_state_dir() -> io::Result<PathBuf> {
    crate::bootstrap::platform_state_dir()
        .map(|dir| dir.join("runner"))
        .map_err(io::Error::from)
}

/// Serves the runner until `shutdown` resolves. The harnesses it supervises end with it.
pub async fn serve<F>(config: RunnerConfig, shutdown: F) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let configs = crate::station::plugins::provider_configs(&config.settings.providers)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let plugins = Arc::new(
        Plugins::scan_with(&config.plugin_paths, &config.plugin_policy)
            .with_provider_configs(configs),
    );
    crate::station::plugins::log_scan(plugins.scan_summary());
    serve_with_plugins(config, plugins, shutdown).await
}

pub(crate) async fn serve_with_plugins<F>(
    config: RunnerConfig,
    plugins: Arc<Plugins>,
    shutdown: F,
) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    if !config.listen.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a runner listens on a loopback address only; a Station reaches it through the Theater's transport",
        ));
    }
    fs::create_dir_all(config.state_dir.join(SESSIONS_DIR))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(config.state_dir.join(LOCK_FILE))?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("another runner serves {}", config.state_dir.display()),
        )
    })?;
    let token = ensure_token(&config.state_dir)?;
    write_atomically(
        &config.state_dir.join(SETTINGS_FILE),
        &config.settings.to_json_bytes()?,
    )?;
    let limits = config.settings.runner;
    let runner = Runner {
        inner: Arc::new(Inner {
            state_dir: config.state_dir.clone(),
            plugins,
            terminal: config.terminal,
            sessions: Mutex::new(load_sessions(&config.state_dir, &limits)?),
            bindings: Mutex::new(BTreeMap::new()),
            streams: Mutex::new(BTreeMap::new()),
            limits,
            stopping: AtomicBool::new(false),
            _lock: lock,
        }),
    };
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let info = RunnerInfo {
        address: listener.local_addr()?.to_string(),
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    write_atomically(
        &config.state_dir.join(INFO_FILE),
        &serde_json::to_vec_pretty(&info).map_err(io::Error::other)?,
    )?;
    #[cfg(test)]
    live::register(&config.state_dir, &runner.inner);
    let handle = tokio::runtime::Handle::current();
    let _closing = CloseConnections(runner.clone());
    let accepting = async {
        let mut accepted = 0u64;
        loop {
            let (stream, _) = listener.accept().await?;
            let stream = stream.into_std()?;
            stream.set_nonblocking(false)?;
            accepted += 1;
            let id = accepted;
            if let Ok(kept) = stream.try_clone() {
                lock_of(&runner.inner.streams).insert(id, kept);
            }
            let (runner, token, handle) = (runner.clone(), token.clone(), handle.clone());
            std::thread::spawn(move || {
                wire::serve_connection(runner.clone(), token, handle, stream);
                // Clones of the socket outlive the connection, and a peer that half-closed
                // waits for this end: only a shutdown tells it the connection is over.
                if let Some(kept) = lock_of(&runner.inner.streams).remove(&id) {
                    let _ = kept.shutdown(std::net::Shutdown::Both);
                }
            });
        }
        #[allow(unreachable_code)]
        Ok::<(), io::Error>(())
    };
    tokio::pin!(accepting);
    let shutdown = shutdown;
    tokio::pin!(shutdown);
    let served = tokio::select! {
        () = &mut shutdown => Ok(()),
        failed = &mut accepting => failed,
    };
    runner.inner.stopping.store(true, Ordering::Release);
    let sessions: Vec<_> = lock_of(&runner.inner.sessions).values().cloned().collect();
    for hosted in sessions {
        hosted.close();
    }
    if RunnerInfo::read(&config.state_dir).is_ok_and(|current| current == info) {
        let _ = fs::remove_file(config.state_dir.join(INFO_FILE));
    }
    served
}

/// Starts `program runner serve <arguments>` in the background, detached from the caller's
/// terminal and session, and answers once it serves `state_dir`, looking every
/// `limits.start_poll` and giving up after `limits.start_wait`.
///
/// Its output goes to `runner.log` in the state directory. On Windows it also inherits the
/// caller's inheritable handles, such as an output pipe, and holds them while it runs.
pub fn spawn_detached(
    program: &Path,
    arguments: &[std::ffi::OsString],
    state_dir: &Path,
    limits: &RunnerLimits,
) -> io::Result<RunnerInfo> {
    fs::create_dir_all(state_dir)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir.join("runner.log"))?;
    let mut command = std::process::Command::new(program);
    command
        .arg("runner")
        .arg("serve")
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group: a hangup or Ctrl-C for the caller's terminal is not its.
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = command.spawn()?;
    let start_wait = limits.start_wait.0;
    let deadline = std::time::Instant::now() + start_wait;
    loop {
        if let Ok(info) = RunnerInfo::read(state_dir)
            && info.pid == child.id()
        {
            return Ok(info);
        }
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "the runner exited with {status}; see {}",
                state_dir.join("runner.log").display()
            )));
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "the runner did not start within {} seconds",
                    start_wait.as_secs_f64()
                ),
            ));
        }
        std::thread::sleep(limits.start_poll.0);
    }
}

/// What a Station asks a runner to launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Launch {
    Start(AgentStart),
    Resume(AgentResume),
}

/// A Session as a runner reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SessionReport {
    pub(crate) key: String,
    pub(crate) handler: String,
    pub(crate) terminal: String,
    pub(crate) session_id: Option<SessionId>,
    /// The harness is gone; no event will follow the last one.
    pub(crate) ended: bool,
    /// The newest event's sequence number.
    pub(crate) last: u64,
    /// The last sequence number a Station acknowledged.
    pub(crate) cursor: u64,
}

#[derive(Serialize, Deserialize)]
struct SessionFile {
    handler: String,
    terminal: String,
    session_id: Option<SessionId>,
    ended: bool,
    created_at: u64,
}

#[derive(Clone)]
struct Runner {
    inner: Arc<Inner>,
}

struct Inner {
    state_dir: PathBuf,
    plugins: Arc<Plugins>,
    terminal: Option<String>,
    sessions: Mutex<BTreeMap<String, Arc<Hosted>>>,
    /// The connection each Session is bound to.
    bindings: Mutex<BTreeMap<String, Arc<wire::Binding>>>,
    /// The connections being served, by the order they came in, closed when the runner stops.
    streams: Mutex<BTreeMap<u64, std::net::TcpStream>>,
    limits: RunnerLimits,
    stopping: AtomicBool,
    _lock: File,
}

/// A running harness, shared by its pump and the requests that reach it.
type Harness = Arc<Mutex<Object<dyn AgentSession>>>;

/// One Session the runner supervises, or supervised before it restarted.
struct Hosted {
    key: String,
    dir: PathBuf,
    handler: String,
    terminal: String,
    created_at: u64,
    harness: Mutex<Option<Harness>>,
    /// Requests queued for the harness lock; the pump lets them go first.
    waiting: AtomicUsize,
    closing: AtomicBool,
    ended: AtomicBool,
    session_id: Mutex<Option<SessionId>>,
    spool: Mutex<spool::Spool>,
    cable: Mutex<Option<cable::CableProxy>>,
}

/// The way a Session's Cable relay reaches the Station: the connection bound to it now.
struct BoundStation {
    runner: std::sync::Weak<Inner>,
    key: String,
}

impl cable::Route for BoundStation {
    fn open(&self, request: &cable::Request) -> Option<cable::Opened> {
        let inner = self.runner.upgrade()?;
        let binding = lock_of(&inner.bindings).get(&self.key).cloned()?;
        binding.open_cable(request)
    }
}

fn lock_of<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let partial = path.with_extension("tmp");
    fs::write(&partial, bytes)?;
    fs::rename(&partial, path)
}

/// The API token, created on first start and readable only by the runner's user.
fn ensure_token(state_dir: &Path) -> io::Result<String> {
    ensure_token_at(&state_dir.join(TOKEN_FILE))
}

/// The bearer token kept in the file `path`: read when it holds one, else made from 256 random
/// bits and written there, readable only by this user on Unix.
pub(crate) fn ensure_token_at(path: &Path) -> io::Result<String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Ok(token) = fs::read_to_string(path)
        && !token.trim().is_empty()
    {
        return Ok(token.trim().to_owned());
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(token.as_bytes())?;
    Ok(token)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && !key.starts_with('.')
}

/// The Sessions in the state directory. One that was live when its runner stopped ends now,
/// with an event saying why; ended ones older than `limits.keep_ended` are removed.
fn load_sessions(
    state_dir: &Path,
    limits: &RunnerLimits,
) -> io::Result<BTreeMap<String, Arc<Hosted>>> {
    let mut sessions = BTreeMap::new();
    let now = unix_seconds();
    for entry in fs::read_dir(state_dir.join(SESSIONS_DIR))? {
        let entry = entry?;
        let key = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.path();
        if !valid_key(&key) || !dir.is_dir() {
            continue;
        }
        let Ok(file) = fs::read(dir.join(SESSION_FILE))
            .map_err(io::Error::other)
            .and_then(|bytes| {
                serde_json::from_slice::<SessionFile>(&bytes).map_err(io::Error::other)
            })
        else {
            continue;
        };
        if file.ended
            && now.saturating_sub(file.created_at) as f64 > limits.keep_ended.0.as_secs_f64()
        {
            let _ = fs::remove_dir_all(&dir);
            continue;
        }
        let hosted = Hosted {
            key: key.clone(),
            dir: dir.clone(),
            handler: file.handler,
            terminal: file.terminal,
            created_at: file.created_at,
            harness: Mutex::new(None),
            waiting: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            ended: AtomicBool::new(true),
            session_id: Mutex::new(file.session_id),
            spool: Mutex::new(spool::Spool::open(&dir)?),
            cable: Mutex::new(None),
        };
        if !file.ended {
            let _ = lock_of(&hosted.spool).append(event(
                LOST_EVENT,
                "the runner supervising this Session restarted, so its harness is gone",
            ));
            hosted.persist();
        }
        sessions.insert(key, Arc::new(hosted));
    }
    Ok(sessions)
}

fn event(kind: &str, text: &str) -> AgentEvent {
    AgentEvent {
        kind: kind.to_owned(),
        text: Some(text.to_owned()),
        data: None,
        limit: None,
        usage: None,
        tools: Vec::new(),
        quota: Vec::new(),
    }
}

impl Hosted {
    fn report(&self) -> SessionReport {
        let spool = lock_of(&self.spool);
        SessionReport {
            key: self.key.clone(),
            handler: self.handler.clone(),
            terminal: self.terminal.clone(),
            session_id: lock_of(&self.session_id).clone(),
            ended: self.ended.load(Ordering::Acquire),
            last: spool.last(),
            cursor: spool.cursor(),
        }
    }

    fn persist(&self) {
        let file = SessionFile {
            handler: self.handler.clone(),
            terminal: self.terminal.clone(),
            session_id: lock_of(&self.session_id).clone(),
            ended: self.ended.load(Ordering::Acquire),
            created_at: self.created_at,
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&file)
            && let Err(error) = write_atomically(&self.dir.join(SESSION_FILE), &bytes)
        {
            eprintln!(
                "ai6 runner: Session {:?} record not written: {error}",
                self.key
            );
        }
    }

    fn harness(&self) -> Option<Harness> {
        lock_of(&self.harness).clone()
    }

    /// Runs `action` on the harness ahead of the pump's next poll.
    fn with_harness<T>(
        &self,
        action: impl FnOnce(&Object<dyn AgentSession>) -> Result<T, String>,
    ) -> Result<T, String> {
        let harness = self
            .harness()
            .ok_or_else(|| "the Session's harness is no longer running".to_owned())?;
        self.waiting.fetch_add(1, Ordering::AcqRel);
        let guard = harness.lock();
        self.waiting.fetch_sub(1, Ordering::AcqRel);
        let guard = guard.map_err(|_| "the harness lock was poisoned".to_owned())?;
        action(&guard)
    }

    fn append(&self, event: AgentEvent) {
        if let Err(error) = lock_of(&self.spool).append(event) {
            eprintln!(
                "ai6 runner: Session {:?} event not spooled: {error}",
                self.key
            );
        }
    }

    fn end(&self) {
        if !self.ended.swap(true, Ordering::AcqRel) {
            self.persist();
        }
    }

    /// Stops the harness, if it still runs, and ends the Session.
    fn close(&self) {
        self.closing.store(true, Ordering::Release);
        if let Some(harness) = lock_of(&self.harness).take() {
            self.waiting.fetch_add(1, Ordering::AcqRel);
            let guard = harness.lock();
            self.waiting.fetch_sub(1, Ordering::AcqRel);
            if let Ok(guard) = guard {
                let _ = guard.close();
            }
        }
        lock_of(&self.cable).take();
        self.end();
    }

    /// Moves the harness's events to the spool until it ends or the Session is closed.
    fn pump(self: Arc<Self>, harness: Harness, poll_ms: i64) {
        loop {
            if self.closing.load(Ordering::Acquire) {
                break;
            }
            while self.waiting.load(Ordering::Acquire) > 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            let result = {
                let Ok(guard) = harness.lock() else {
                    self.append(event("failed", "the harness lock was poisoned"));
                    break;
                };
                let polled = guard.next_event(poll_ms);
                match checked_session_id(guard.session_id().ok().flatten()) {
                    Ok(session_id) => (polled, session_id),
                    Err(why) => {
                        self.append(event("failed", &why));
                        break;
                    }
                }
            };
            if let (_, Some(session_id)) = &result {
                let mut known = lock_of(&self.session_id);
                if known.as_ref() != Some(session_id) {
                    *known = Some(session_id.clone());
                    drop(known);
                    self.persist();
                }
            }
            match result.0 {
                Ok(Some(event)) => {
                    let last = ends_turn(&event);
                    self.append(event);
                    if last {
                        break;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    if !self.closing.load(Ordering::Acquire) {
                        self.append(event(
                            "failed",
                            &crate::directorates::q::transport::describe(&error),
                        ));
                    }
                    break;
                }
            }
        }
        // The harness has said its last word; a Station closes it only after reading that,
        // so a harness that keeps running is stopped here instead.
        if !self.closing.load(Ordering::Acquire) {
            let _ = harness.lock().map(|guard| guard.close());
            lock_of(&self.harness).take();
        }
        self.end();
    }
}

/// The bearer token an API takes, and how much of a refused request's body it reads before
/// answering.
#[derive(Clone)]
pub(crate) struct Bearer {
    token: String,
    drain: usize,
}

impl Bearer {
    /// `token`, draining refused bodies up to `limits.refused_body_drain_bytes`.
    pub(crate) fn new(token: String, limits: &ApiLimits) -> Self {
        Self {
            token,
            drain: limits.refused_body_drain_bytes.or_max(),
        }
    }
}

/// Lets an HTTP request through only with `Authorization: Bearer TOKEN`, compared in constant
/// time. The Station's own HTTP service uses it; a runner authenticates ACP connections itself.
pub(crate) async fn authorize(
    axum::extract::State(Bearer { token, drain }): axum::extract::State<Bearer>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    use subtle::ConstantTimeEq as _;
    let supplied = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if supplied.len() != token.len() || !bool::from(supplied.as_bytes().ct_eq(token.as_bytes())) {
        // Read a small body before answering: a socket closed with unread data is reset, and
        // a Windows client then sees an aborted connection instead of the 401.
        let _ = axum::body::to_bytes(request.into_body(), drain).await;
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({"error": "a valid bearer token is required"})),
        )
            .into_response();
    }
    next.run(request).await
}

/// The runners serving in this process, so a test can cut their connections as a dead host does.
#[cfg(test)]
mod live {
    use super::*;

    static RUNNERS: Mutex<Vec<(PathBuf, std::sync::Weak<Inner>)>> = Mutex::new(Vec::new());

    pub(super) fn register(state_dir: &Path, inner: &Arc<Inner>) {
        lock_of(&RUNNERS).push((state_dir.to_path_buf(), Arc::downgrade(inner)));
    }

    /// Closes every connection to the runner serving `state_dir`; it still listens.
    pub(crate) fn sever_connections(state_dir: &Path) {
        for (dir, inner) in lock_of(&RUNNERS).iter() {
            if dir == state_dir
                && let Some(inner) = inner.upgrade()
            {
                inner.close_connections();
            }
        }
    }
}

#[cfg(test)]
pub(crate) use live::sever_connections;

/// Closes every connection when the runner stops serving, however it stops.
struct CloseConnections(Runner);

impl Drop for CloseConnections {
    fn drop(&mut self) {
        self.0.inner.close_connections();
    }
}

impl Inner {
    fn close_connections(&self) {
        for stream in std::mem::take(&mut *lock_of(&self.streams)).into_values() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Runner {
    fn hosted(&self, key: &str) -> Option<Arc<Hosted>> {
        lock_of(&self.inner.sessions).get(key).cloned()
    }

    /// The terminal provider for a harness that talks over `stdio`: the Station's request or
    /// the runner's pin, when that suits the harness, else one chosen among the available
    /// providers, as a Station chooses.
    fn terminal(&self, requested: Option<&str>, stdio: HarnessStdio) -> Result<String, String> {
        self.inner
            .plugins
            .choose_terminal(stdio, requested.or(self.inner.terminal.as_deref()))
            .map_err(|refusal| format!("this runner cannot start the harness: {refusal}"))
    }

    /// Starts or resumes the harness of Session `key` on `handler`, supervised from now on,
    /// with a Cable endpoint relayed to the attached Station when `cables` asks for one. The
    /// Session exists, with its failure in its spool, even when the harness does not start.
    async fn launch(
        &self,
        key: &str,
        handler: &HandlerSpec,
        pinned: Option<&str>,
        mut launch: Launch,
        cables: bool,
    ) -> Result<Arc<Hosted>, String> {
        if self.inner.stopping.load(Ordering::Acquire) {
            return Err("the runner is stopping".to_owned());
        }
        if !valid_key(key) {
            return Err("a Session key uses 1 to 128 letters, digits, '.', '_' or '-'".to_owned());
        }
        let settings = match &handler.config {
            serde_json::Value::Object(settings) => settings.clone(),
            _ => serde_json::Map::new(),
        };
        let built = self
            .inner
            .plugins
            .handler_with(&handler.id, &settings)
            .map_err(|error| error.to_string())?;
        for line in built.weakenings().unwrap_or_default() {
            eprintln!("ai6: Session {key:?} is weakened: {line}");
        }
        let stdio = built.stdio().map_err(|error| {
            format!(
                "handler `{}` did not declare its harness's stdio: {error}",
                handler.id
            )
        })?;
        let terminal = self.terminal(pinned, stdio)?;
        let provider = self
            .inner
            .plugins
            .terminal(&terminal, None)
            .map_err(|error| error.to_string())?;
        let dir = self.inner.state_dir.join(SESSIONS_DIR).join(key);
        let hosted = {
            let mut sessions = lock_of(&self.inner.sessions);
            if sessions.contains_key(key) || dir.exists() {
                return Err(format!("Session {key:?} exists"));
            }
            let spool = spool::Spool::open(&dir).map_err(|error| error.to_string())?;
            let hosted = Arc::new(Hosted {
                key: key.to_owned(),
                dir,
                handler: handler.id.clone(),
                terminal,
                created_at: unix_seconds(),
                harness: Mutex::new(None),
                waiting: AtomicUsize::new(0),
                closing: AtomicBool::new(false),
                ended: AtomicBool::new(false),
                session_id: Mutex::new(None),
                spool: Mutex::new(spool),
                cable: Mutex::new(None),
            });
            sessions.insert(key.to_owned(), hosted.clone());
            hosted
        };
        hosted.persist();

        let environment = match &mut launch {
            Launch::Start(start) => &mut start.environment,
            Launch::Resume(resume) => &mut resume.environment,
        };
        // The harness talks to the runner's relay, which reaches whichever Station is attached.
        // A Cable endpoint a Station named itself is not one the harness can reach from here.
        environment.remove(cable::URL_KEY);
        environment.remove(cable::TOKEN_KEY);
        if cables {
            let route = Arc::new(BoundStation {
                runner: Arc::downgrade(&self.inner),
                key: key.to_owned(),
            });
            match cable::CableProxy::start(key, route, &self.inner.limits).await {
                Ok(proxy) => {
                    proxy.add_environment(environment);
                    *lock_of(&hosted.cable) = Some(proxy);
                }
                Err(error) => {
                    hosted.append(event("failed", &format!("Cable relay failed: {error}")));
                    hosted.end();
                    return Err(error.to_string());
                }
            }
        }
        let terminal_handle = TerminalBridge::agent_terminal(provider);
        let started = tokio::task::spawn_blocking(move || match launch {
            Launch::Start(start) => built.start(start, terminal_handle),
            Launch::Resume(resume) => built.resume(resume, terminal_handle),
        })
        .await;
        let harness = match started {
            Ok(Ok(harness)) => harness,
            Ok(Err(error)) => {
                let reason = crate::directorates::q::transport::describe(&error);
                hosted.append(event("failed", &reason));
                hosted.close();
                return Err(reason);
            }
            Err(error) => {
                hosted.append(event("failed", &error.to_string()));
                hosted.close();
                return Err(error.to_string());
            }
        };
        match checked_session_id(harness.session_id().ok().flatten()) {
            Ok(Some(session_id)) => {
                *lock_of(&hosted.session_id) = Some(session_id);
                hosted.persist();
            }
            Ok(None) => {}
            Err(why) => {
                let _ = harness.close();
                hosted.append(event("failed", &why));
                hosted.close();
                return Err(why);
            }
        }
        let harness = Arc::new(Mutex::new(harness));
        *lock_of(&hosted.harness) = Some(harness.clone());
        let pump = hosted.clone();
        let poll_ms = i64::try_from(self.inner.limits.pump_poll.0.as_millis()).unwrap_or(i64::MAX);
        std::thread::Builder::new()
            .name(format!("ai6-runner-{}", hosted.key))
            .spawn(move || pump.pump(harness, poll_ms))
            .map_err(|error| {
                hosted.close();
                error.to_string()
            })?;
        Ok(hosted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_on_disk(state_dir: &Path, key: &str, ended: bool, created_at: u64) -> PathBuf {
        let dir = state_dir.join(SESSIONS_DIR).join(key);
        let mut spool = spool::Spool::open(&dir).unwrap();
        spool.append(event("text", "working on it")).unwrap();
        let file = SessionFile {
            handler: "claude".to_owned(),
            terminal: "pty".to_owned(),
            session_id: Some(SessionId::try_new("provider-1").unwrap()),
            ended,
            created_at,
        };
        fs::write(dir.join(SESSION_FILE), serde_json::to_vec(&file).unwrap()).unwrap();
        dir
    }

    #[test]
    fn a_settings_file_a_station_wrote_keeps_loading_and_what_is_written_reads_back() {
        let settings = RunnerHandoff::parse(
            br#"{"runner": {"pump_poll": 0.25, "replay_batch": 8, "cable_request_bytes": null},
                "providers": {"pty": {"scrollback": 10, "shell": null}}}"#,
        )
        .unwrap();
        assert_eq!(settings.runner.replay_batch, 8);
        assert_eq!(
            settings.runner.cable_request_bytes,
            crate::bounds::Limit::UNBOUNDED
        );
        assert_eq!(
            settings.runner.keep_ended,
            RunnerLimits::default().keep_ended
        );
        assert_eq!(settings.providers["pty"]["shell"], serde_json::Value::Null);
        assert_eq!(
            RunnerHandoff::parse(&settings.to_json_bytes().unwrap()).unwrap(),
            settings
        );
        let refusal = RunnerHandoff::parse(br#"{"runner": {"pump_poll": "hunter2"}}"#)
            .unwrap_err()
            .to_string();
        assert!(refusal.contains("runner.pump_poll"), "{refusal}");
        assert!(!refusal.contains("hunter2"), "{refusal}");
    }

    #[test]
    fn a_restarted_runner_ends_the_sessions_it_lost_and_keeps_their_events() {
        let state_dir = tempfile::tempdir().unwrap();
        session_on_disk(state_dir.path(), "live", false, unix_seconds());
        session_on_disk(state_dir.path(), "done", true, unix_seconds());
        let old = session_on_disk(state_dir.path(), "old", true, 1);

        let sessions = load_sessions(state_dir.path(), &RunnerLimits::default()).unwrap();
        assert_eq!(
            sessions.keys().map(String::as_str).collect::<Vec<_>>(),
            ["done", "live"]
        );
        assert!(!old.exists(), "a Session ended long ago is removed");

        let live = sessions["live"].report();
        assert!(live.ended);
        assert_eq!(live.last, 2);
        let events = lock_of(&sessions["live"].spool).after(0, 10);
        assert_eq!(events[1].event.kind, LOST_EVENT);
        assert!(
            events[1]
                .event
                .text
                .as_deref()
                .is_some_and(|text| text.contains("restarted"))
        );
        // Ended once is enough: loading again adds nothing.
        drop(sessions);
        let again = load_sessions(state_dir.path(), &RunnerLimits::default()).unwrap();
        assert_eq!(again["live"].report().last, 2);
        assert_eq!(again["done"].report().last, 1);
    }

    fn choice_runner(state_dir: &Path, with_pipe: bool, pinned: Option<&str>) -> Runner {
        Runner {
            inner: Arc::new(Inner {
                state_dir: state_dir.to_path_buf(),
                plugins: Arc::new(crate::station::plugins::testing::terminal_choice(with_pipe)),
                terminal: pinned.map(str::to_owned),
                sessions: Mutex::new(BTreeMap::new()),
                bindings: Mutex::new(BTreeMap::new()),
                streams: Mutex::new(BTreeMap::new()),
                limits: RunnerLimits::default(),
                stopping: AtomicBool::new(false),
                _lock: File::create(state_dir.join(LOCK_FILE)).unwrap(),
            }),
        }
    }

    fn start_request(key: &str) -> AgentStart {
        AgentStart {
            mission_id: crate::MissionId::try_new(key).unwrap(),
            agent_id: "007".to_owned(),
            prompt: "work".to_owned(),
            initial_context: Vec::new(),
            working_directory: ".".to_owned(),
            clearance: "standard".to_owned(),
            environment: BTreeMap::new(),
            model: None,
            effort: None,
            max_cost_micro_usd: None,
            project_settings: false,
            project_mcp: false,
            answer_only: false,
            assistant: None,
            overlays: Default::default(),
        }
    }

    async fn start(runner: &Runner, key: &str, handler: &str) -> Result<Arc<Hosted>, String> {
        runner
            .launch(
                key,
                &HandlerSpec::new(handler, serde_json::Value::Null),
                None,
                Launch::Start(start_request(key)),
                false,
            )
            .await
    }

    /// A runner chooses the terminal as a Station does, from what the handler declares: the
    /// mock handlers refuse to start, after the choice, and the Session records it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_runner_gives_each_handler_a_terminal_that_suits_it() {
        let state_dir = tempfile::tempdir().unwrap();
        let runner = choice_runner(state_dir.path(), true, None);
        for (key, handler, terminal) in
            [("json", "json-agent", "pipe"), ("tty", "tty-agent", "pty")]
        {
            let reason = start(&runner, key, handler).await.err().unwrap();
            assert!(reason.contains(crate::station::plugins::testing::REFUSED_START));
            let recorded = lock_of(&runner.inner.sessions)[key].report().terminal;
            assert_eq!(recorded, terminal);
        }

        let pty_only = tempfile::tempdir().unwrap();
        let runner = choice_runner(pty_only.path(), false, None);
        let reason = start(&runner, "json", "json-agent").await.err().unwrap();
        assert!(reason.contains("without a terminal (`pipe`)"), "{reason}");
        assert!(runner.hosted("json").is_none(), "nothing was started");

        let pinned = tempfile::tempdir().unwrap();
        let runner = choice_runner(pinned.path(), true, Some("pty"));
        let reason = start(&runner, "json", "json-agent").await.err().unwrap();
        assert!(reason.contains("`pty` is pinned"), "{reason}");
        let reason = start(&runner, "tty", "tty-agent").await.err().unwrap();
        assert!(reason.contains(crate::station::plugins::testing::REFUSED_START));
    }

    #[test]
    fn session_keys_cannot_leave_the_sessions_directory() {
        for key in ["mission-1", "a.b_c", "0f3a9c"] {
            assert!(valid_key(key), "{key}");
        }
        for key in ["", "..", ".hidden", "a/b", "a\\b", "a b", &"k".repeat(129)] {
            assert!(!valid_key(key), "{key}");
        }
    }

    use crate::directorates::section_00::acp::{
        AcpOptions, AcpSession, Authenticate, Exchange, LAUNCH_META_KEY, LaunchMeta,
        RUNNER_TOKEN_METHOD, UNBOUNDED_LINE_BYTES,
    };
    use coqui_acp::lines::{IoLines, over_io};

    /// A runner serving a directory on a loopback port, stopped when `stop` is dropped.
    struct Serving {
        dir: tempfile::TempDir,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        task: Option<tokio::task::JoinHandle<io::Result<()>>>,
    }

    impl Serving {
        async fn start() -> Self {
            Self::start_within(RunnerLimits::default()).await
        }

        async fn start_within(limits: RunnerLimits) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let task = tokio::spawn(serve_with_plugins(
                RunnerConfig::new(dir.path()).with_limits(limits),
                Arc::new(crate::station::plugins::testing::terminal_choice(true)),
                async move {
                    let _ = stopped.await;
                },
            ));
            let path = dir.path().to_path_buf();
            tokio::task::spawn_blocking(move || {
                for _ in 0..500 {
                    if RunnerInfo::read(&path).is_ok() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                panic!("the runner did not start");
            })
            .await
            .unwrap();
            Self {
                dir,
                stop: Some(stop),
                task: Some(task),
            }
        }

        fn address(&self) -> String {
            RunnerInfo::read(self.dir.path()).unwrap().address
        }

        fn token(&self) -> String {
            fs::read_to_string(self.dir.path().join(TOKEN_FILE))
                .unwrap()
                .trim()
                .to_owned()
        }

        fn transport(&self) -> IoLines {
            let stream = std::net::TcpStream::connect(self.address()).unwrap();
            over_io(stream.try_clone().unwrap(), stream, 0, UNBOUNDED_LINE_BYTES)
                .unwrap()
                .0
        }

        fn authenticate(&self, token: &str) -> Authenticate {
            Authenticate {
                method: RUNNER_TOKEN_METHOD.to_owned(),
                meta: serde_json::Map::from_iter([("token".to_owned(), token.into())]),
            }
        }

        fn options(&self, key: &str) -> AcpOptions {
            let launch = LaunchMeta::from_start(&start_request(key))
                .with_handler(HandlerSpec::new("stream-agent", serde_json::Value::Null))
                .with_key(key);
            let mut options = AcpOptions {
                cwd: ".".to_owned(),
                prompt: Some("work".to_owned()),
                authenticate: Some(self.authenticate(&self.token())),
                close_on_drop: false,
                ..AcpOptions::default()
            };
            options
                .meta
                .insert(LAUNCH_META_KEY.to_owned(), launch.to_value());
            options
        }

        fn attach_options(&self, key: &str, after: u64) -> AcpOptions {
            AcpOptions {
                cwd: ".".to_owned(),
                resume: Some(key.to_owned()),
                reattach: Some(after),
                authenticate: Some(self.authenticate(&self.token())),
                close_on_drop: false,
                ..AcpOptions::default()
            }
        }

        async fn stop(mut self) {
            drop(self.stop.take());
            self.task.take().unwrap().await.unwrap().unwrap();
        }
    }

    fn events_until(session: &AcpSession, wanted: usize) -> Vec<coqui_acp_provider::AgentEvent> {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut seen = Vec::new();
        while seen.len() < wanted && std::time::Instant::now() < deadline {
            if let Some(event) = session.next_event(100).unwrap() {
                seen.push(event);
            }
        }
        seen
    }

    fn texts(events: &[coqui_acp_provider::AgentEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| event.text.clone())
            .collect()
    }

    use coqui_acp_provider::AgentMessage;

    #[tokio::test(flavor = "multi_thread")]
    async fn a_session_runs_over_acp_and_a_dropped_connection_replays_the_spool_once() {
        let serving = Serving::start().await;
        let (options, attach_first, attach_second, transport, t2, t3) = (
            serving.options("k1"),
            serving.attach_options("k1", 1),
            serving.attach_options("k1", 3),
            serving.transport(),
            serving.transport(),
            serving.transport(),
        );
        tokio::task::spawn_blocking(move || {
            let session = AcpSession::connect(transport, options, None).unwrap();
            assert_eq!(texts(&events_until(&session, 1)), ["hello work"]);
            assert_eq!(
                session.session_id().unwrap().unwrap().as_str(),
                "stream-session-1"
            );
            session
                .send_message(AgentMessage {
                    text: "a".to_owned(),
                })
                .unwrap();
            assert_eq!(texts(&events_until(&session, 1)), ["echo:a"]);
            // The connection drops in the middle of the turn.
            drop(session);

            // Another connection takes the Session by key from what it had taken (1 event):
            // the spool replays what followed, once, and the Session goes on live.
            let again = AcpSession::connect(t2, attach_first, None).unwrap();
            assert_eq!(texts(&events_until(&again, 1)), ["echo:a"]);
            again
                .send_message(AgentMessage {
                    text: "b".to_owned(),
                })
                .unwrap();
            assert_eq!(texts(&events_until(&again, 1)), ["echo:b"]);
            assert!(again.next_event(300).unwrap().is_none(), "no repeats");
            drop(again);

            // Asked from the end, nothing is replayed.
            let last = AcpSession::connect(t3, attach_second, None).unwrap();
            assert!(last.next_event(300).unwrap().is_none());
            last.send_message(AgentMessage {
                text: "finish".to_owned(),
            })
            .unwrap();
            let done = events_until(&last, 1);
            assert_eq!(done[0].kind, "completed");
            last.close().unwrap();
        })
        .await
        .unwrap();
        serving.stop().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_wrong_token_is_refused_before_any_session() {
        let serving = Serving::start().await;
        let (mut options, transport) = (serving.options("k2"), serving.transport());
        options.authenticate = Some(serving.authenticate("wrong"));
        let (refused, probe) = (serving.transport(), serving.token());
        let outcome = tokio::task::spawn_blocking(move || {
            let error = AcpSession::connect(transport, options, None).err().unwrap();
            // Without authenticating at all, a session request is refused too.
            let mut bare = AcpOptions {
                cwd: ".".to_owned(),
                prompt: Some("work".to_owned()),
                ..AcpOptions::default()
            };
            bare.meta.insert(
                LAUNCH_META_KEY.to_owned(),
                LaunchMeta::from_start(&start_request("k2"))
                    .with_handler(HandlerSpec::new("stream-agent", serde_json::Value::Null))
                    .with_key("k2")
                    .to_value(),
            );
            let bare_error = AcpSession::connect(refused, bare, None).err().unwrap();
            (error.to_string(), bare_error.to_string(), probe)
        })
        .await
        .unwrap();
        assert!(
            outcome.0.contains("authentication") || outcome.0.contains("token"),
            "{}",
            outcome.0
        );
        assert!(!outcome.1.is_empty());
        assert!(
            lock_of(&serving_runner_sessions(&serving)).is_empty(),
            "no Session was made"
        );
        serving.stop().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_connection_its_peer_finished_is_closed_while_the_runner_serves_on() {
        let serving = Serving::start().await;
        let address = serving.address();
        tokio::task::spawn_blocking(move || {
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(20)))
                .unwrap();
            // A runner that kept the socket open makes this read time out, which is an error.
            std::io::Read::read_to_end(&mut stream, &mut Vec::new()).unwrap();
        })
        .await
        .unwrap();
        serving.stop().await;
    }

    fn serving_runner_sessions(serving: &Serving) -> Mutex<Vec<String>> {
        // Sessions live on disk: a Session made would have a directory.
        let dir = serving.dir.path().join(SESSIONS_DIR);
        Mutex::new(
            fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .filter_map(|entry| entry.ok())
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default(),
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_second_connection_supersedes_the_first() {
        let serving = Serving::start().await;
        let (options, attach, first, second) = (
            serving.options("k3"),
            serving.attach_options("k3", 0),
            serving.transport(),
            serving.transport(),
        );
        tokio::task::spawn_blocking(move || {
            let older = AcpSession::connect(first, options, None).unwrap();
            assert_eq!(texts(&events_until(&older, 1)), ["hello work"]);
            let newer = AcpSession::connect(second, attach, None).unwrap();
            // The newer one gets the Session (and what the older took, replayed from its cursor).
            newer
                .send_message(AgentMessage {
                    text: "x".to_owned(),
                })
                .unwrap();
            assert!(texts(&events_until(&newer, 2)).contains(&"echo:x".to_owned()));
            // The older is told why it ends, then loses its connection.
            let told = events_until(&older, 1);
            assert_eq!(told[0].kind, "detached");
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while !older.connection_lost() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the older stays connected"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        })
        .await
        .unwrap();
        serving.stop().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_superseded_connection_stays_open_for_its_close_wait() {
        const CLOSE_WAIT_SECS: u64 = 3;
        let serving = Serving::start_within(RunnerLimits {
            superseded_close_wait: crate::bounds::Seconds::new(CLOSE_WAIT_SECS),
            ..RunnerLimits::default()
        })
        .await;
        let (options, attach, first, second) = (
            serving.options("k5"),
            serving.attach_options("k5", 0),
            serving.transport(),
            serving.transport(),
        );
        tokio::task::spawn_blocking(move || {
            let older = AcpSession::connect(first, options, None).unwrap();
            assert_eq!(texts(&events_until(&older, 1)), ["hello work"]);
            let _newer = AcpSession::connect(second, attach, None).unwrap();
            assert_eq!(events_until(&older, 1)[0].kind, "detached");
            let told = std::time::Instant::now();
            let deadline = told + Duration::from_secs(20);
            while !older.connection_lost() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the older stays connected"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            // A second less, for how late the test may have read the notice.
            assert!(
                told.elapsed() >= Duration::from_secs(CLOSE_WAIT_SECS - 1),
                "closed {:?} after the notice",
                told.elapsed()
            );
        })
        .await
        .unwrap();
        serving.stop().await;
    }

    #[test]
    fn a_starting_runner_is_looked_for_at_the_start_poll() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let program = {
            use std::os::unix::fs::PermissionsExt;
            let program = dir.path().join("stay");
            fs::write(&program, "#!/bin/sh\nexec sleep 5\n").unwrap();
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            program
        };
        #[cfg(windows)]
        let program = {
            let program = dir.path().join("stay.cmd");
            fs::write(&program, "@ping -n 5 127.0.0.1 >nul\r\n").unwrap();
            program
        };
        let poll = Duration::from_secs(2);
        let limits = RunnerLimits {
            start_wait: crate::bounds::Seconds(Duration::from_millis(100)),
            start_poll: crate::bounds::Seconds(poll),
            ..RunnerLimits::default()
        };
        let started = std::time::Instant::now();
        let error = spawn_detached(&program, &[], dir.path(), &limits).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        assert!(
            started.elapsed() >= poll,
            "gave up {:?} in",
            started.elapsed()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_runner_listens_on_loopback_only_and_one_serves_a_directory() {
        let state_dir = tempfile::tempdir().unwrap();
        let open = RunnerConfig::new(state_dir.path()).with_listen("0.0.0.0:0".parse().unwrap());
        let refused = serve_with_plugins(open, Arc::new(Plugins::scan(&[])), async {}).await;
        assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::InvalidInput);

        let serving = Serving::start().await;
        let second = serve_with_plugins(
            RunnerConfig::new(serving.dir.path()),
            Arc::new(Plugins::scan(&[])),
            async {},
        )
        .await;
        assert_eq!(second.unwrap_err().kind(), io::ErrorKind::AddrInUse);

        // With the right token the runner answers, and says it has no such Session.
        let (transport, auth) = (serving.transport(), serving.authenticate(&serving.token()));
        let answered = tokio::task::spawn_blocking(move || {
            Exchange::new().authenticate(auth).run(
                transport,
                vec![(
                    "_ai6/session_status".to_owned(),
                    serde_json::json!({"key": "none"}),
                )],
            )
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(answered.len(), 1);
        assert_eq!(answered[0].as_ref().unwrap_err().code, -32002);

        // A wrong token is refused.
        let (transport, auth) = (serving.transport(), serving.authenticate("wrong"));
        let refused = tokio::task::spawn_blocking(move || {
            Exchange::new().authenticate(auth).run(
                transport,
                vec![(
                    "_ai6/session_status".to_owned(),
                    serde_json::json!({"key": "none"}),
                )],
            )
        })
        .await
        .unwrap();
        assert!(refused.is_err(), "{refused:?}");

        let path = serving.dir.path().to_path_buf();
        serving.stop().await;
        assert!(
            RunnerInfo::read(&path).is_err(),
            "a runner that stops says so by removing runner.json"
        );
    }
}
