//! The field runner: `ai6 runner`, the AI6 binary's Theater mode.
//!
//! A runner supervises agent harnesses on the Theater they run on, so a Session outlives the
//! Station that started it. It loads the same AgentHandler and Terminal providers a Station
//! does and has no scheduler, Registry or roster: a Station starts Sessions on it, reads their
//! events and sends them messages over an HTTP API on a loopback port, protected by a bearer
//! token kept in the state directory. A Station that reaches the Theater through a transport
//! runs `ai6 runner call` there, which relays one request.
//!
//! Every event a harness emits is written to disk with a sequence number before any Station
//! can read it. A Station acknowledges what it has read by asking for the events after it, so
//! a Station that comes back, or another one, asks from the last acknowledgement and replays
//! what it missed. A runner that restarts has lost the harnesses it supervised; it ends their
//! Sessions with a `lost` event, which a Station takes as the Session lost.
//!
//! The state directory holds `runner.json` (where the runner listens, while it runs),
//! `token`, `runner.lock` and `sessions/<key>/` with `events.jsonl`, `cursor` and
//! `session.json`.

mod cable;
pub(crate) mod client;
mod spool;

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{StatusCode, header::AUTHORIZATION};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use guatiao::library::Object;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::Notify;

use crate::directorates::m::identity::SessionId;
use crate::directorates::q::terminal::{HarnessStdio, TerminalBridge};
use crate::directorates::section_00::agent::{
    AgentEvent, AgentMessage, AgentResume, AgentSession, AgentStart, LIMIT_EVENT,
};
use crate::station::plugins::Plugins;

#[cfg(feature = "cables-mcp")]
pub(crate) use cable::URL_KEY as CABLE_URL_KEY;
pub(crate) use cable::Upstream;
pub(crate) use spool::Spooled;

const INFO_FILE: &str = "runner.json";
const TOKEN_FILE: &str = "token";
const LOCK_FILE: &str = "runner.lock";
const SESSIONS_DIR: &str = "sessions";
const SESSION_FILE: &str = "session.json";
/// Most events one read returns.
const PAGE: usize = 256;
/// The longest a read waits for a new event.
const MAX_WAIT: Duration = Duration::from_secs(30);
/// How long the pump waits for one event before it checks whether to stop.
const POLL_MS: i64 = 250;
/// Ended Sessions older than this are removed when a runner starts.
const KEEP_ENDED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The exit status of `ai6 runner call` when no runner answers it.
pub const CALL_NOT_RUNNING: u8 = 3;
/// The kind of the event a restarted runner ends a Session it lost with: the harness is gone,
/// so the Station treats the Session as lost rather than failed.
pub(crate) const LOST_EVENT: &str = "lost";

/// Where a runner listens and what it was built from, as `runner.json` records it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerInfo {
    /// The base URL of the runner's API, on a loopback address.
    pub url: String,
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

/// How `ai6 runner serve` runs.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    state_dir: PathBuf,
    listen: SocketAddr,
    plugin_paths: Vec<PathBuf>,
    plugin_policy: crate::station::plugins::PluginPolicy,
    terminal: Option<String>,
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
        }
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
    crate::station::safehouse::state_dir()
        .map(|dir| dir.join("runner"))
        .map_err(io::Error::other)
}

/// Serves the runner until `shutdown` resolves. The harnesses it supervises end with it.
pub async fn serve<F>(config: RunnerConfig, shutdown: F) -> io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let plugins = Arc::new(Plugins::scan_with(
        &config.plugin_paths,
        &config.plugin_policy,
    ));
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
    let runner = Runner {
        inner: Arc::new(Inner {
            state_dir: config.state_dir.clone(),
            plugins,
            terminal: config.terminal,
            sessions: Mutex::new(load_sessions(&config.state_dir)?),
            stopping: AtomicBool::new(false),
            _lock: lock,
        }),
    };
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    let info = RunnerInfo {
        url: format!("http://{}", listener.local_addr()?),
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    write_atomically(
        &config.state_dir.join(INFO_FILE),
        &serde_json::to_vec_pretty(&info).map_err(io::Error::other)?,
    )?;
    let app = Router::new()
        .route("/v1/health", get(health))
        .route("/v1/sessions", get(list_sessions).post(start_session))
        .route("/v1/sessions/{key}", get(session_status))
        .route("/v1/sessions/{key}/events", get(read_events))
        .route("/v1/sessions/{key}/messages", post(send_message))
        .route("/v1/sessions/{key}/interrupt", post(interrupt))
        .route("/v1/sessions/{key}/close", post(close))
        .route("/v1/sessions/{key}/cable", put(set_cable))
        .layer(middleware::from_fn_with_state(token, authorize))
        .with_state(runner.clone());
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
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

/// Sends one request to the runner serving `state_dir` and answers its status and JSON body.
///
/// This is what `ai6 runner call` does for a Station that reaches the Theater through a
/// transport, and what a Station on the same host does itself.
pub fn call(
    state_dir: &Path,
    method: &str,
    path: &str,
    body: Option<&serde_json::Value>,
) -> Result<(u16, serde_json::Value), String> {
    let not_running = |why: String| format!("no runner serves {} ({why})", state_dir.display());
    let info = RunnerInfo::read(state_dir).map_err(|error| not_running(error.to_string()))?;
    let token = fs::read_to_string(state_dir.join(TOKEN_FILE))
        .map_err(|error| not_running(error.to_string()))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(MAX_WAIT + Duration::from_secs(30)))
        .build()
        .into();
    let request = ureq::http::Request::builder()
        .method(method)
        .uri(format!("{}{path}", info.url))
        .header("Authorization", format!("Bearer {}", token.trim()));
    let response = match body {
        Some(body) => {
            let bytes = serde_json::to_vec(body).map_err(|error| error.to_string())?;
            request
                .header("Content-Type", "application/json")
                .body(bytes)
                .map(|request| agent.run(request))
        }
        None => request.body(()).map(|request| agent.run(request)),
    };
    let mut response = response
        .map_err(|error| format!("invalid runner request: {error}"))?
        .map_err(|error| not_running(error.to_string()))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("the runner's answer could not be read: {error}"))?;
    let body = if text.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&text)
            .map_err(|error| format!("the runner answered something other than JSON: {error}"))?
    };
    Ok((status, body))
}

/// Starts `program runner serve <arguments>` in the background, detached from the caller's
/// terminal and session, and answers once it serves `state_dir`.
///
/// Its output goes to `runner.log` in the state directory. On Windows it also inherits the
/// caller's inheritable handles, such as an output pipe, and holds them while it runs.
pub fn spawn_detached(
    program: &Path,
    arguments: &[std::ffi::OsString],
    state_dir: &Path,
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
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
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
                "the runner did not start within 30 seconds",
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What a Station asks a runner to launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Launch {
    Start(AgentStart),
    Resume(AgentResume),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StartRequest {
    /// The Station's name for the Session on this runner; letters, digits, `.`, `_`, `-`.
    pub(crate) key: String,
    pub(crate) handler: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) terminal: Option<String>,
    pub(crate) launch: Launch,
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub(crate) struct EventsQuery {
    /// Acknowledges every event up to this one and reads those after it; the last
    /// acknowledged one when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) after: Option<u64>,
    /// Waits this long for an event when there is none; `0` answers at once.
    #[serde(default)]
    pub(crate) timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EventsPage {
    pub(crate) events: Vec<Spooled>,
    pub(crate) session_id: Option<SessionId>,
    /// The Session has ended and the page holds its last event, or it had none left.
    pub(crate) complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Health {
    version: String,
    pid: u32,
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
    changed: Notify,
    cable: Mutex<Option<cable::CableProxy>>,
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
    let path = state_dir.join(TOKEN_FILE);
    if let Ok(token) = fs::read_to_string(&path)
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
    options.open(&path)?.write_all(token.as_bytes())?;
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
/// with an event saying why; ended ones past [`KEEP_ENDED`] are removed.
fn load_sessions(state_dir: &Path) -> io::Result<BTreeMap<String, Arc<Hosted>>> {
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
        if file.ended && now.saturating_sub(file.created_at) > KEEP_ENDED.as_secs() {
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
            changed: Notify::new(),
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
        self.changed.notify_waiters();
    }

    fn end(&self) {
        if !self.ended.swap(true, Ordering::AcqRel) {
            self.persist();
        }
        self.changed.notify_waiters();
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
    fn pump(self: Arc<Self>, harness: Harness) {
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
                let polled = guard.next_event(POLL_MS);
                let session_id = guard.session_id().ok().flatten();
                (polled, session_id)
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
                    let last = matches!(event.kind.as_str(), "completed" | "failed" | LIMIT_EVENT);
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

#[derive(Serialize)]
struct ApiError {
    error: String,
}

/// A refusal: its status and `{"error": "..."}`.
struct Failure(StatusCode, String);

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (self.0, Json(ApiError { error: self.1 })).into_response()
    }
}

fn failure(status: StatusCode, message: impl Into<String>) -> Failure {
    Failure(status, message.into())
}

async fn authorize(State(token): State<String>, request: Request, next: Next) -> Response {
    let supplied = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if supplied.len() != token.len() || !bool::from(supplied.as_bytes().ct_eq(token.as_bytes())) {
        return failure(StatusCode::UNAUTHORIZED, "a valid bearer token is required")
            .into_response();
    }
    next.run(request).await
}

async fn health() -> Json<Health> {
    Json(Health {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: std::process::id(),
    })
}

impl Runner {
    fn hosted(&self, key: &str) -> Result<Arc<Hosted>, Failure> {
        lock_of(&self.inner.sessions)
            .get(key)
            .cloned()
            .ok_or_else(|| failure(StatusCode::NOT_FOUND, format!("no Session {key:?}")))
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
}

async fn list_sessions(State(runner): State<Runner>) -> Json<Vec<SessionReport>> {
    let sessions: Vec<_> = lock_of(&runner.inner.sessions).values().cloned().collect();
    Json(sessions.iter().map(|hosted| hosted.report()).collect())
}

async fn session_status(
    State(runner): State<Runner>,
    UrlPath(key): UrlPath<String>,
) -> Result<Json<SessionReport>, Failure> {
    Ok(Json(runner.hosted(&key)?.report()))
}

async fn start_session(
    State(runner): State<Runner>,
    Json(request): Json<StartRequest>,
) -> Result<(StatusCode, Json<SessionReport>), Failure> {
    if runner.inner.stopping.load(Ordering::Acquire) {
        return Err(failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "the runner is stopping",
        ));
    }
    if !valid_key(&request.key) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "a Session key uses 1 to 128 letters, digits, '.', '_' or '-'",
        ));
    }
    let handler = runner
        .inner
        .plugins
        .handler(&request.handler, None)
        .map_err(|error| failure(StatusCode::CONFLICT, error.to_string()))?;
    let stdio = handler.stdio().map_err(|error| {
        failure(
            StatusCode::BAD_GATEWAY,
            format!(
                "handler `{}` did not declare its harness's stdio: {error}",
                request.handler
            ),
        )
    })?;
    let terminal = runner
        .terminal(request.terminal.as_deref(), stdio)
        .map_err(|error| failure(StatusCode::CONFLICT, error))?;
    let provider = runner
        .inner
        .plugins
        .terminal(&terminal, None)
        .map_err(|error| failure(StatusCode::CONFLICT, error.to_string()))?;
    let dir = runner.inner.state_dir.join(SESSIONS_DIR).join(&request.key);
    let hosted = {
        let mut sessions = lock_of(&runner.inner.sessions);
        if sessions.contains_key(&request.key) || dir.exists() {
            return Err(failure(
                StatusCode::CONFLICT,
                format!("Session {:?} exists", request.key),
            ));
        }
        let spool = spool::Spool::open(&dir)
            .map_err(|error| failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        let hosted = Arc::new(Hosted {
            key: request.key.clone(),
            dir,
            handler: request.handler.clone(),
            terminal,
            created_at: unix_seconds(),
            harness: Mutex::new(None),
            waiting: AtomicUsize::new(0),
            closing: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            session_id: Mutex::new(None),
            spool: Mutex::new(spool),
            changed: Notify::new(),
            cable: Mutex::new(None),
        });
        sessions.insert(request.key.clone(), hosted.clone());
        hosted
    };
    hosted.persist();

    let mut launch = request.launch;
    let environment = match &mut launch {
        Launch::Start(start) => &mut start.environment,
        Launch::Resume(resume) => &mut resume.environment,
    };
    // The harness talks to the runner's relay, which a re-attaching Station repoints.
    if let Some(upstream) = Upstream::from_environment(environment) {
        match cable::CableProxy::start(Some(upstream)).await {
            Ok(proxy) => {
                proxy.add_environment(environment);
                *lock_of(&hosted.cable) = Some(proxy);
            }
            Err(error) => {
                hosted.append(event("failed", &format!("Cable relay failed: {error}")));
                hosted.end();
                return Err(failure(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    error.to_string(),
                ));
            }
        }
    }
    let terminal_handle = TerminalBridge::agent_terminal(provider);
    let started = tokio::task::spawn_blocking(move || match launch {
        Launch::Start(start) => handler.start(start, terminal_handle),
        Launch::Resume(resume) => handler.resume(resume, terminal_handle),
    })
    .await;
    let harness = match started {
        Ok(Ok(harness)) => harness,
        Ok(Err(error)) => {
            let reason = crate::directorates::q::transport::describe(&error);
            hosted.append(event("failed", &reason));
            hosted.close();
            return Err(failure(StatusCode::BAD_GATEWAY, reason));
        }
        Err(error) => {
            hosted.append(event("failed", &error.to_string()));
            hosted.close();
            return Err(failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                error.to_string(),
            ));
        }
    };
    if let Ok(Some(session_id)) = harness.session_id() {
        *lock_of(&hosted.session_id) = Some(session_id);
        hosted.persist();
    }
    let harness = Arc::new(Mutex::new(harness));
    *lock_of(&hosted.harness) = Some(harness.clone());
    let pump = hosted.clone();
    std::thread::Builder::new()
        .name(format!("ai6-runner-{}", hosted.key))
        .spawn(move || pump.pump(harness))
        .map_err(|error| {
            hosted.close();
            failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        })?;
    Ok((StatusCode::CREATED, Json(hosted.report())))
}

async fn read_events(
    State(runner): State<Runner>,
    UrlPath(key): UrlPath<String>,
    Query(query): Query<EventsQuery>,
) -> Result<Json<EventsPage>, Failure> {
    let hosted = runner.hosted(&key)?;
    let page = |after: Option<u64>| {
        let mut spool = lock_of(&hosted.spool);
        let after = after.unwrap_or_else(|| spool.cursor()).min(spool.last());
        if let Err(error) = spool.acknowledge(after) {
            eprintln!("ai6 runner: Session {key:?} cursor not written: {error}");
        }
        let events = spool.after(after, PAGE);
        let through = events.last().map_or(after, |spooled| spooled.seq);
        let complete = hosted.ended.load(Ordering::Acquire) && through >= spool.last();
        (after, events, complete)
    };
    let (after, mut events, mut complete) = page(query.after);
    let wait = Duration::from_millis(query.timeout_ms).min(MAX_WAIT);
    if events.is_empty() && !complete && !wait.is_zero() {
        let _ = tokio::time::timeout(wait, async {
            loop {
                let notified = hosted.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let (_, now, done) = page(Some(after));
                if !now.is_empty() || done {
                    return;
                }
                notified.await;
            }
        })
        .await;
        (_, events, complete) = page(Some(after));
    }
    Ok(Json(EventsPage {
        events,
        session_id: lock_of(&hosted.session_id).clone(),
        complete,
    }))
}

async fn send_message(
    State(runner): State<Runner>,
    UrlPath(key): UrlPath<String>,
    Json(message): Json<AgentMessage>,
) -> Result<StatusCode, Failure> {
    let hosted = runner.hosted(&key)?;
    tokio::task::spawn_blocking(move || {
        hosted.with_harness(|harness| {
            harness
                .send_message(message)
                .map_err(|error| crate::directorates::q::transport::describe(&error))
        })
    })
    .await
    .map_err(|error| failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
    .map_err(|error| failure(StatusCode::CONFLICT, error))?;
    Ok(StatusCode::ACCEPTED)
}

async fn interrupt(
    State(runner): State<Runner>,
    UrlPath(key): UrlPath<String>,
) -> Result<StatusCode, Failure> {
    let hosted = runner.hosted(&key)?;
    tokio::task::spawn_blocking(move || {
        hosted.with_harness(|harness| {
            harness
                .interrupt()
                .map_err(|error| crate::directorates::q::transport::describe(&error))
        })
    })
    .await
    .map_err(|error| failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
    .map_err(|error| failure(StatusCode::CONFLICT, error))?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Debug, Default, Deserialize)]
struct CloseQuery {
    /// The last event the Station took, acknowledged before the Session closes.
    after: Option<u64>,
}

async fn close(
    State(runner): State<Runner>,
    UrlPath(key): UrlPath<String>,
    Query(query): Query<CloseQuery>,
) -> Result<Json<SessionReport>, Failure> {
    let hosted = runner.hosted(&key)?;
    if let Some(after) = query.after
        && let Err(error) = lock_of(&hosted.spool).acknowledge(after)
    {
        eprintln!("ai6 runner: Session {key:?} cursor not written: {error}");
    }
    let closed = hosted.clone();
    tokio::task::spawn_blocking(move || closed.close())
        .await
        .map_err(|error| failure(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Json(hosted.report()))
}

async fn set_cable(
    State(runner): State<Runner>,
    UrlPath(key): UrlPath<String>,
    Json(upstream): Json<Upstream>,
) -> Result<StatusCode, Failure> {
    let hosted = runner.hosted(&key)?;
    match lock_of(&hosted.cable).as_ref() {
        Some(proxy) => {
            proxy.set_upstream(upstream);
            Ok(StatusCode::NO_CONTENT)
        }
        None => Err(failure(
            StatusCode::CONFLICT,
            "the Session has no Cable endpoint",
        )),
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
    fn a_restarted_runner_ends_the_sessions_it_lost_and_keeps_their_events() {
        let state_dir = tempfile::tempdir().unwrap();
        session_on_disk(state_dir.path(), "live", false, unix_seconds());
        session_on_disk(state_dir.path(), "done", true, unix_seconds());
        let old = session_on_disk(state_dir.path(), "old", true, 1);

        let sessions = load_sessions(state_dir.path()).unwrap();
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
        let again = load_sessions(state_dir.path()).unwrap();
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
                stopping: AtomicBool::new(false),
                _lock: File::create(state_dir.join(LOCK_FILE)).unwrap(),
            }),
        }
    }

    async fn start(runner: &Runner, key: &str, handler: &str) -> Result<SessionReport, Failure> {
        let request = StartRequest {
            key: key.to_owned(),
            handler: handler.to_owned(),
            terminal: None,
            launch: Launch::Start(AgentStart {
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
            }),
        };
        start_session(State(runner.clone()), Json(request))
            .await
            .map(|(_, Json(report))| report)
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
            let Failure(status, reason) = start(&runner, key, handler).await.unwrap_err();
            assert_eq!(status, StatusCode::BAD_GATEWAY, "{reason}");
            assert!(reason.contains(crate::station::plugins::testing::REFUSED_START));
            let recorded = lock_of(&runner.inner.sessions)[key].report().terminal;
            assert_eq!(recorded, terminal);
        }

        let pty_only = tempfile::tempdir().unwrap();
        let runner = choice_runner(pty_only.path(), false, None);
        let Failure(status, reason) = start(&runner, "json", "json-agent").await.unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(reason.contains("without a terminal (`pipe`)"), "{reason}");
        assert!(runner.hosted("json").is_err(), "nothing was started");

        let pinned = tempfile::tempdir().unwrap();
        let runner = choice_runner(pinned.path(), true, Some("pty"));
        let Failure(status, reason) = start(&runner, "json", "json-agent").await.unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(reason.contains("`pty` is pinned"), "{reason}");
        let Failure(_, reason) = start(&runner, "tty", "tty-agent").await.unwrap_err();
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

    #[tokio::test(flavor = "multi_thread")]
    async fn a_runner_listens_on_loopback_only_and_one_serves_a_directory() {
        let state_dir = tempfile::tempdir().unwrap();
        let open = RunnerConfig::new(state_dir.path()).with_listen("0.0.0.0:0".parse().unwrap());
        let refused = serve_with_plugins(open, Arc::new(Plugins::scan(&[])), async {}).await;
        assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::InvalidInput);

        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let first = tokio::spawn(serve_with_plugins(
            RunnerConfig::new(state_dir.path()),
            Arc::new(Plugins::scan(&[])),
            async move {
                let _ = stopped.await;
            },
        ));
        let dir = state_dir.path().to_path_buf();
        let health = tokio::task::spawn_blocking(move || {
            for _ in 0..500 {
                if let Ok(answer) = call(&dir, "GET", "/v1/health", None) {
                    return answer;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("the runner did not answer");
        })
        .await
        .unwrap();
        assert_eq!(health.0, 200);
        assert_eq!(health.1["pid"], std::process::id());

        let second = serve_with_plugins(
            RunnerConfig::new(state_dir.path()),
            Arc::new(Plugins::scan(&[])),
            async {},
        )
        .await;
        assert_eq!(second.unwrap_err().kind(), io::ErrorKind::AddrInUse);

        // A wrong token is refused; the right one is in the state directory, not elsewhere.
        let info = RunnerInfo::read(state_dir.path()).unwrap();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let url = format!("{}/v1/sessions", info.url);
        let status = tokio::task::spawn_blocking(move || {
            agent
                .get(&url)
                .header("Authorization", "Bearer wrong")
                .call()
                .unwrap()
                .status()
                .as_u16()
        })
        .await
        .unwrap();
        assert_eq!(status, 401);

        drop(stop);
        first.await.unwrap().unwrap();
        assert!(
            RunnerInfo::read(state_dir.path()).is_err(),
            "a runner that stops says so by removing runner.json"
        );
        let dir = state_dir.path().to_path_buf();
        let gone = tokio::task::spawn_blocking(move || call(&dir, "GET", "/v1/health", None))
            .await
            .unwrap();
        assert!(gone.unwrap_err().contains("no runner serves"));
    }
}
