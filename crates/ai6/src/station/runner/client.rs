//! The Station's side of a runner: how it reaches one, and the [`AgentSession`] through which
//! it drives a Session the runner supervises, over ACP.
//!
//! On the Station's own host the connection is the runner's loopback port. On another host it is
//! `ai6 runner connect` on a terminal the Theater's transport opens: it authenticates with the
//! runner's token and relays the stream.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use guatiao::Bytes;
use guatiao::Status;
use guatiao::library::{Object, ProviderError};

use super::{Launch, RunnerHandoff, SETTINGS_FILE, SessionReport};
use crate::directorates::m::identity::SessionId;
use crate::directorates::m::theaters::{RunnerSettings, Theater, TheaterAccess};
use crate::directorates::q::terminal::{STREAM_COLS, Spec, TerminalSession};
use crate::directorates::q::transport::{RunSpec, TransportClient, describe};
use crate::directorates::section_00::acp::{
    AcpOptions, AcpSession, Authenticate, Exchange, ExtensionHandler, HandlerSpec, LAUNCH_META_KEY,
    LaunchMeta, RUNNER_TOKEN_METHOD, ResumeMeta, RpcFailure, TerminalIo, UNBOUNDED_LINE_BYTES,
    UNBOUNDED_MS, duration_of,
};
use crate::directorates::section_00::agent::provider_session_id;
use crate::station::limits::RunnerLimits;
use crate::station::reach::ReachWatch;
use coqui_acp::lines::{IoLines, over_io};
use coqui_acp_provider::{AgentEvent, AgentMessage, AgentSession};

/// The line `ai6 runner connect` prints once its terminal is raw and it is authenticated, so
/// the Station writes nothing a cooked terminal would echo.
pub(crate) const READY_LINE: &str = "ai6-runner-connect-ready";

/// The JSON-RPC code of a resource that was not found.
const NOT_FOUND: i32 = -32002;

/// How the Station reaches the runner.
enum Reach {
    /// On the Station's own host: the runner's loopback port.
    Local {
        state_dir: PathBuf,
        command: PathBuf,
    },
    /// On a host a transport reaches: `ai6 runner connect` on a terminal there.
    Transport {
        client: Arc<TransportClient>,
        command: String,
        state_dir: Option<String>,
    },
}

/// The bounds on talking to a runner, from the Theater's runner settings.
#[derive(Debug, Clone, Copy)]
struct Timeouts {
    connect_ms: u64,
    initialize_ms: u64,
    request_ms: u64,
}

impl Timeouts {
    fn of(settings: &RunnerSettings) -> Self {
        Self {
            connect_ms: settings.connect_timeout_ms.unwrap_or(30_000),
            initialize_ms: settings.initialize_timeout_ms.unwrap_or(20_000),
            request_ms: settings.request_timeout_ms.unwrap_or(30_000),
        }
    }
}

/// One runner a Station talks to.
pub(crate) struct RunnerLink {
    reach: Reach,
    plugin_paths: Vec<String>,
    terminal: Option<String>,
    timeouts: Timeouts,
    /// The pins and profile the Station resolved, which a runner it starts on its own host
    /// checks its libraries against.
    pins: Option<(crate::PinSet, crate::PluginProfile, crate::KeepVerified)>,
    /// The `[runner]` and `[providers.<id>]` tables, which the link runs by and hands a runner
    /// it starts.
    settings: RunnerHandoff,
}

/// Why a request got no answer from the runner: the link is down, or no runner serves.
#[derive(Debug, Clone)]
pub(crate) struct Unreachable(pub(crate) String);

/// Why a Session could not be attached to.
pub(crate) enum AttachError {
    /// The runner does not have it.
    NoSession(String),
    /// The runner could not be reached.
    Unreachable(String),
}

/// A connection to a runner, before any ACP is spoken over it.
struct Connection {
    lines: IoLines,
    authenticate: Option<Authenticate>,
    on_close: Option<Box<dyn FnOnce() + Send>>,
}

fn token_auth(token: &str) -> Authenticate {
    Authenticate {
        method: RUNNER_TOKEN_METHOD.to_owned(),
        meta: serde_json::Map::from_iter([("token".to_owned(), token.trim().into())]),
    }
}

impl RunnerLink {
    /// The runner of `theater`, reached through `transport` unless the Theater is the
    /// Station's own host, where it keeps its state in `runner` under `station_state` unless
    /// the Theater says otherwise. `None` when the Theater declares no runner.
    pub(crate) fn for_theater(
        theater: &Theater,
        transport: Option<Arc<TransportClient>>,
        station_state: &Path,
    ) -> Result<Option<Self>, String> {
        let Some(settings) = &theater.runner else {
            return Ok(None);
        };
        let reach = match &theater.access {
            TheaterAccess::Local => Reach::Local {
                state_dir: match &settings.state_dir {
                    Some(dir) => PathBuf::from(dir),
                    None => station_state.join("runner"),
                },
                command: match &settings.command {
                    Some(command) => PathBuf::from(command),
                    None => std::env::current_exe()
                        .map_err(|error| format!("this Station's own binary: {error}"))?,
                },
            },
            _ => Reach::Transport {
                client: transport.ok_or_else(|| {
                    format!(
                        "this Station has no transport to Theater {:?}, so it cannot reach its runner",
                        theater.id
                    )
                })?,
                command: settings.command.clone().unwrap_or_else(|| "ai6".to_owned()),
                state_dir: settings.state_dir.clone(),
            },
        };
        Ok(Some(Self::with_reach(reach, settings)))
    }

    /// A runner on this host serving `state_dir`, started (if ever) with `command`.
    #[cfg(test)]
    pub(crate) fn local(state_dir: PathBuf, command: PathBuf) -> Self {
        Self::with_reach(
            Reach::Local { state_dir, command },
            &RunnerSettings::default(),
        )
    }

    fn with_reach(reach: Reach, settings: &RunnerSettings) -> Self {
        Self {
            reach,
            plugin_paths: settings.plugin_paths.clone(),
            terminal: settings.terminal.clone(),
            timeouts: Timeouts::of(settings),
            pins: None,
            settings: RunnerHandoff::default(),
        }
    }

    /// Runs by `settings`, and hands them to a runner it starts.
    #[must_use]
    pub(crate) fn with_settings(mut self, settings: RunnerHandoff) -> Self {
        self.settings = settings;
        self
    }

    /// Starts a runner on the Station's own host checking its libraries against `pins` under
    /// `profile`, written to `plugins.toml` in the runner's state directory, and keeping old
    /// verified copies as `keep` says.
    #[must_use]
    pub(crate) fn with_pins(
        mut self,
        pins: crate::PinSet,
        profile: crate::PluginProfile,
        keep: crate::KeepVerified,
    ) -> Self {
        self.pins = Some((pins, profile, keep));
        self
    }

    /// The terminal provider pinned for the runner, if any.
    pub(crate) fn terminal(&self) -> Option<&str> {
        self.terminal.as_deref()
    }

    /// Opens a connection to the runner.
    fn connect(&self) -> Result<Connection, Unreachable> {
        match &self.reach {
            Reach::Local { state_dir, .. } => {
                let not_running = |why: String| {
                    Unreachable(format!("no runner serves {} ({why})", state_dir.display()))
                };
                let info = super::RunnerInfo::read(state_dir)
                    .map_err(|error| not_running(error.to_string()))?;
                let token = std::fs::read_to_string(state_dir.join(super::TOKEN_FILE))
                    .map_err(|error| not_running(error.to_string()))?;
                let address = info
                    .address
                    .parse()
                    .map_err(|error: std::net::AddrParseError| not_running(error.to_string()))?;
                let stream = std::net::TcpStream::connect_timeout(
                    &address,
                    Duration::from_millis(self.timeouts.connect_ms),
                )
                .map_err(|error| not_running(error.to_string()))?;
                let reader = stream
                    .try_clone()
                    .map_err(|error| not_running(error.to_string()))?;
                Ok(Connection {
                    lines: over_io(reader, stream, 0, UNBOUNDED_LINE_BYTES)
                        .map_err(|error| not_running(error.to_string()))?
                        .0,
                    authenticate: Some(token_auth(&token)),
                    on_close: None,
                })
            }
            Reach::Transport {
                client,
                command,
                state_dir,
            } => {
                let mut argv = vec![command.clone(), "runner".to_owned(), "connect".to_owned()];
                if let Some(dir) = state_dir {
                    argv.extend(["--state-dir".to_owned(), dir.clone()]);
                }
                let spec = Spec {
                    argv,
                    cwd: None,
                    env: None,
                    rows: 24,
                    cols: STREAM_COLS,
                };
                let process: Arc<Object<dyn TerminalSession>> = Arc::new(
                    client
                        .open_terminal(&spec)
                        .map_err(|error| Unreachable(describe(&error)))?,
                );
                let timeout = Duration::from_millis(self.timeouts.connect_ms);
                let poll_ms =
                    i64::try_from(millis(self.settings.runner.terminal_poll.0)).unwrap_or(i64::MAX);
                if let Err(why) = await_ready(&process, timeout, poll_ms) {
                    let _ = process.kill();
                    return Err(Unreachable(why));
                }
                let io = TerminalIo::new(process.clone(), poll_ms);
                let (lines, _) = over_io(io.clone(), io.clone(), 0, UNBOUNDED_LINE_BYTES)
                    .map_err(|error| Unreachable(error.to_string()))?;
                let stopper = io;
                Ok(Connection {
                    lines,
                    authenticate: None,
                    on_close: Some(Box::new(move || {
                        let _ = process.kill();
                        stopper.stop();
                    })),
                })
            }
        }
    }

    /// Makes `calls` on a connection of its own; each answers by itself.
    fn exchange(
        &self,
        calls: Vec<(String, serde_json::Value)>,
    ) -> Result<Vec<Result<serde_json::Value, RpcFailure>>, Unreachable> {
        let connection = self.connect()?;
        let mut exchange = Exchange::new()
            .client_info("ai6", env!("CARGO_PKG_VERSION"))
            .initialize_timeout(duration_of(self.timeouts.initialize_ms))
            .request_timeout(duration_of(self.timeouts.request_ms));
        if let Some(authenticate) = connection.authenticate {
            exchange = exchange.authenticate(authenticate);
        }
        let answered = exchange.run(connection.lines, calls);
        if let Some(on_close) = connection.on_close {
            on_close();
        }
        answered.map_err(|error| Unreachable(error.to_string()))
    }

    /// Whether a runner answers.
    pub(crate) fn probe(&self) -> Result<(), Unreachable> {
        self.exchange(Vec::new()).map(drop)
    }

    /// Starts the runner unless one already answers.
    pub(crate) fn ensure(&self) -> Result<(), String> {
        let not_running = match self.probe() {
            Ok(()) => return Ok(()),
            Err(Unreachable(why)) => why,
        };
        let mut arguments: Vec<String> = Vec::new();
        for path in &self.plugin_paths {
            arguments.extend(["--plugin-path".to_owned(), path.clone()]);
        }
        if let Some(terminal) = &self.terminal {
            arguments.extend(["--terminal".to_owned(), terminal.clone()]);
        }
        match &self.reach {
            Reach::Local { state_dir, command } => {
                let mut arguments: Vec<std::ffi::OsString> =
                    arguments.into_iter().map(Into::into).collect();
                arguments.extend(["--state-dir".into(), state_dir.as_os_str().to_owned()]);
                let settings = state_dir.join(SETTINGS_FILE);
                let text = self
                    .settings
                    .to_json_bytes()
                    .map_err(|error| error.to_string())?;
                std::fs::create_dir_all(state_dir)
                    .and_then(|()| std::fs::write(&settings, text))
                    .map_err(|error| format!("{}: {error}", settings.display()))?;
                arguments.extend(["--settings".into(), settings.into_os_string()]);
                if let Some((pins, profile, keep)) = &self.pins {
                    let path = state_dir.join("plugins.toml");
                    let text = toml::to_string(pins).map_err(|error| error.to_string())?;
                    std::fs::create_dir_all(state_dir)
                        .and_then(|()| std::fs::write(&path, text))
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                    arguments.extend([
                        "--pins".into(),
                        path.into_os_string(),
                        "--plugin-profile".into(),
                        profile.as_str().into(),
                        "--keep-verified".into(),
                        keep.to_string().into(),
                    ]);
                }
                super::spawn_detached(command, &arguments, state_dir, &self.settings.runner)
                    .map_err(|error| {
                        format!("the runner could not start ({not_running}): {error}")
                    })?;
            }
            Reach::Transport {
                client,
                command,
                state_dir,
            } => {
                let mut argv = vec![
                    command.clone(),
                    "runner".to_owned(),
                    "serve".to_owned(),
                    "--detach".to_owned(),
                ];
                if let Some(dir) = state_dir {
                    argv.extend(["--state-dir".to_owned(), dir.clone()]);
                }
                argv.extend(arguments);
                // The settings go over standard input, so nothing is written on the Theater
                // before the runner itself records them in its state directory.
                argv.extend(["--settings".to_owned(), "-".to_owned()]);
                let settings = self
                    .settings
                    .to_json_bytes()
                    .map_err(|error| error.to_string())?;
                let output = client
                    .run(&RunSpec {
                        argv,
                        stdin: Some(Bytes(settings)),
                        timeout_ms: millis(
                            self.settings.runner.start_wait.0 + self.settings.runner.start_margin.0,
                        ),
                        ..RunSpec::default()
                    })
                    .map_err(|error| describe(&error))?;
                if output.code != Some(0) {
                    return Err(format!(
                        "`{command} runner serve --detach` failed ({not_running}): {}",
                        String::from_utf8_lossy(&output.stderr.0).trim()
                    ));
                }
            }
        }
        self.probe()
            .map_err(|Unreachable(why)| format!("the runner started but does not answer: {why}"))
    }

    /// Launches a harness on the runner under `key`, and answers what it reports with the
    /// Session that drives it. With `cables`, the runner gives the harness a Cable endpoint and
    /// `cables` answers what it relays.
    pub(crate) fn launch(
        &self,
        key: &str,
        handler: &HandlerSpec,
        launch: Launch,
        cables: Option<Arc<dyn ExtensionHandler>>,
    ) -> Result<(SessionReport, AcpSession), String> {
        let connection = self.connect().map_err(|Unreachable(why)| why)?;
        let relayed = cables.is_some();
        let mut options = AcpOptions {
            authenticate: connection.authenticate,
            initialize_timeout_ms: self.timeouts.initialize_ms,
            request_timeout_ms: self.timeouts.request_ms,
            start_timeout_ms: UNBOUNDED_MS,
            close_grace_ms: UNBOUNDED_MS,
            close_on_drop: false,
            fail_on_disconnect: false,
            extensions: cables,
            ..AcpOptions::default()
        };
        match launch {
            Launch::Start(start) => {
                options.cwd = start.working_directory.clone();
                options.prompt = Some(start.prompt.clone());
                options.meta.insert(
                    LAUNCH_META_KEY.to_owned(),
                    LaunchMeta::from_start(&start)
                        .with_handler(handler.clone())
                        .with_terminal(self.terminal.clone())
                        .with_key(key)
                        .with_cables(relayed)
                        .to_value(),
                );
            }
            Launch::Resume(resume) => {
                options.cwd = resume.working_directory.clone();
                options.resume = Some(resume.session_id.to_string());
                options.meta.insert(
                    LAUNCH_META_KEY.to_owned(),
                    ResumeMeta::from_resume(&resume)
                        .with_handler(handler.clone())
                        .with_terminal(self.terminal.clone())
                        .with_key(key)
                        .with_cables(relayed)
                        .to_value(),
                );
            }
        }
        let session = AcpSession::connect(connection.lines, options, connection.on_close)
            .map_err(|error| error.to_string())?;
        let info = session.start_info().unwrap_or_default();
        let report = SessionReport {
            key: key.to_owned(),
            handler: handler.id.clone(),
            terminal: info
                .get("terminal")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            session_id: provider_session_id(session.session_id())
                .map_err(|error| error.to_string())?,
            ended: false,
            last: info
                .get("last")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            cursor: 0,
        };
        Ok((report, session))
    }

    /// Attaches to Session `key`, asking the runner to replay what followed event `after`;
    /// `cables` answers the Cable requests the runner relays over the connection.
    pub(crate) fn attach(
        &self,
        key: &str,
        after: u64,
        cables: Option<Arc<dyn ExtensionHandler>>,
    ) -> Result<AcpSession, AttachError> {
        let connection = self
            .connect()
            .map_err(|Unreachable(why)| AttachError::Unreachable(why))?;
        let options = AcpOptions {
            cwd: ".".to_owned(),
            resume: Some(key.to_owned()),
            reattach: Some(after),
            authenticate: connection.authenticate,
            initialize_timeout_ms: self.timeouts.initialize_ms,
            request_timeout_ms: self.timeouts.request_ms,
            start_timeout_ms: UNBOUNDED_MS,
            close_grace_ms: UNBOUNDED_MS,
            close_on_drop: false,
            fail_on_disconnect: false,
            extensions: cables,
            ..AcpOptions::default()
        };
        AcpSession::connect(connection.lines, options, connection.on_close).map_err(|error| {
            let why = error.to_string();
            if why.to_lowercase().contains("not found") {
                AttachError::NoSession(why)
            } else {
                AttachError::Unreachable(why)
            }
        })
    }

    /// What the runner knows of Session `key`; `None` when it has no such Session.
    pub(crate) fn status(&self, key: &str) -> Result<Option<SessionReport>, String> {
        let mut answers = self
            .exchange(vec![(
                "_ai6/session_status".to_owned(),
                serde_json::json!({"key": key}),
            )])
            .map_err(|Unreachable(why)| why)?;
        match answers.pop() {
            Some(Ok(value)) => serde_json::from_value(value)
                .map(Some)
                .map_err(|error| error.to_string()),
            Some(Err(RpcFailure { code, .. })) if code == NOT_FOUND => Ok(None),
            Some(Err(failure)) => Err(format!("the runner refused: {}", failure.message)),
            None => Err("the runner gave no answer".to_owned()),
        }
    }

    /// Ends Session `key` on the runner, attached or not.
    pub(crate) fn close_session(&self, key: &str) -> Result<(), String> {
        let mut answers = self
            .exchange(vec![(
                "session/close".to_owned(),
                serde_json::json!({"sessionId": key}),
            )])
            .map_err(|Unreachable(why)| why)?;
        match answers.pop() {
            Some(Ok(_)) => Ok(()),
            Some(Err(RpcFailure { code, .. })) if code == NOT_FOUND => Ok(()),
            Some(Err(failure)) => Err(format!("the runner refused: {}", failure.message)),
            None => Err("the runner gave no answer".to_owned()),
        }
    }
}

/// A duration as the whole milliseconds a request takes.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Reads the terminal until `ai6 runner connect` says it is ready, or `timeout` passes, each
/// read waiting up to `poll_ms`.
fn await_ready(
    process: &Arc<Object<dyn TerminalSession>>,
    timeout: Duration,
    poll_ms: i64,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut seen = Vec::new();
    let mut chunk = vec![0u8; 4096];
    while Instant::now() < deadline {
        match process.read(&mut chunk, poll_ms) {
            Ok(crate::directorates::q::terminal::READ_TIMED_OUT) => {}
            Ok(0) => {
                let said = String::from_utf8_lossy(&seen).trim().to_owned();
                return Err(if said.is_empty() {
                    "`ai6 runner connect` ended before it was ready".to_owned()
                } else {
                    said
                });
            }
            Ok(count) => {
                seen.extend_from_slice(&chunk[..usize::try_from(count).unwrap_or(0)]);
                if String::from_utf8_lossy(&seen)
                    .lines()
                    .any(|line| line.trim() == READY_LINE)
                {
                    return Ok(());
                }
            }
            Err(error) => return Err(describe(&error)),
        }
    }
    Err(format!(
        "`ai6 runner connect` was not ready within {} ms",
        timeout.as_millis()
    ))
}

fn refuse(message: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &message.to_string())
}

/// A Session on a runner, driven over ACP.
///
/// A connection that drops costs only time: the next `next_event` that can reach the runner
/// attaches again and the runner replays what followed the last event taken. A runner out of
/// reach past its [`ReachWatch`] grace, one that no longer has the Session, or one that
/// restarted without its harness gives the Session up as lost through the watch. A Station that
/// another Station's connection supersedes stops reading and says so.
pub(crate) struct RemoteSession {
    link: Arc<RunnerLink>,
    key: String,
    inner: Mutex<Inner>,
    detached: Arc<AtomicBool>,
    /// Answers the Cable requests the runner relays, on every connection to it.
    cables: Option<Arc<dyn ExtensionHandler>>,
    reach: ReachWatch,
}

struct Inner {
    session: Option<AcpSession>,
    session_id: Option<SessionId>,
    /// The newest event taken, which a re-attach asks the runner to continue after; `u64::MAX`
    /// until one is, to continue from what the runner last had acknowledged.
    after: u64,
    unreachable: bool,
    superseded: bool,
}

impl RemoteSession {
    /// Drives `session`, a Session `link`'s runner supervises under `key`, noting in `reach`
    /// whether the runner answers.
    pub(crate) fn drive(
        link: Arc<RunnerLink>,
        key: impl Into<String>,
        session: Option<AcpSession>,
        session_id: Option<SessionId>,
        detached: Arc<AtomicBool>,
        reach: ReachWatch,
    ) -> Self {
        Self {
            link,
            key: key.into(),
            inner: Mutex::new(Inner {
                after: if session.is_some() { 0 } else { u64::MAX },
                session,
                session_id,
                unreachable: false,
                superseded: false,
            }),
            detached,
            cables: None,
            reach,
        }
    }

    /// Answers the harness's Cable requests with `cables` on each connection made from now on;
    /// a Session launched with them was given them at its launch.
    #[must_use]
    pub(crate) fn with_cables(mut self, cables: Option<Arc<dyn ExtensionHandler>>) -> Self {
        self.cables = cables;
        self
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What the Station is handed for an event: the event a restarted runner ends the Session
    /// with is taken as the Session lost instead, and the notice that another connection took
    /// the Session over ends this one's reading.
    fn take(&self, inner: &mut Inner, event: AgentEvent) -> Option<AgentEvent> {
        if event.kind == super::LOST_EVENT {
            self.reach.lose(
                event
                    .text
                    .unwrap_or_else(|| "its runner lost the harness".to_owned()),
            );
            return None;
        }
        if event.kind == super::DETACHED_EVENT {
            inner.superseded = true;
            inner.session = None;
        }
        Some(event)
    }

    /// Asks the runner, through the Theater's transport, whether it still has the Session.
    fn probe(&self) -> Result<(), String> {
        match self.link.status(&self.key) {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(format!("the runner no longer has Session {:?}", self.key)),
            Err(why) => Err(why),
        }
    }
}

impl AgentSession for RemoteSession {
    fn session_id(&self) -> Result<Option<String>, ProviderError> {
        let inner = self.inner();
        Ok(inner
            .session
            .as_ref()
            .and_then(|session| session.session_id().ok().flatten())
            .or_else(|| inner.session_id.clone().map(String::from)))
    }

    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
        match &self.inner().session {
            Some(session) => session.send_message(message),
            None => Err(refuse("the runner is unreachable")),
        }
    }

    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
        let wait = u64::try_from(timeout_ms)
            .unwrap_or(0)
            .min(millis(self.link.settings.runner.read_wait.0));
        let mut inner = self.inner();
        if inner.superseded || self.reach.lost().is_some() {
            drop(inner);
            std::thread::sleep(Duration::from_millis(wait));
            return Ok(None);
        }
        if let Some(session) = inner.session.take() {
            let event = match session.next_event(i64::try_from(wait).unwrap_or(i64::MAX)) {
                Ok(event) => event,
                Err(error) => {
                    inner.session = Some(session);
                    return Err(error);
                }
            };
            let seen = session.last_seq();
            if seen > 0 {
                inner.after = if inner.after == u64::MAX {
                    seen
                } else {
                    inner.after.max(seen)
                };
            }
            let lost = session.connection_lost();
            if !lost {
                inner.session = Some(session);
            }
            if let Some(event) = event {
                return Ok(self.take(&mut inner, event));
            }
            if lost {
                inner.unreachable = self.reach.missed(|| self.probe());
                if inner.unreachable {
                    eprintln!(
                        "ai6: runner Session {:?} is unreachable; waiting up to {}s for it",
                        self.key,
                        self.reach.grace().as_secs()
                    );
                }
            }
            return Ok(None);
        }
        match self
            .link
            .attach(&self.key, inner.after, self.cables.clone())
        {
            Ok(session) => {
                self.reach.reached();
                inner.session = Some(session);
                if std::mem::take(&mut inner.unreachable) {
                    eprintln!("ai6: runner Session {:?} is reachable again", self.key);
                }
            }
            Err(AttachError::NoSession(why)) => self.reach.lose(format!(
                "the runner on its Theater no longer has Session {:?} ({why})",
                self.key
            )),
            Err(AttachError::Unreachable(why)) => {
                if self.reach.missed(|| self.probe()) {
                    inner.unreachable = true;
                    eprintln!(
                        "ai6: runner Session {:?} is unreachable; waiting up to {}s for it: {why}",
                        self.key,
                        self.reach.grace().as_secs()
                    );
                }
                drop(inner);
                std::thread::sleep(Duration::from_millis(
                    wait.max(millis(self.link.settings.runner.retry_pause.0)),
                ));
            }
        }
        Ok(None)
    }

    fn interrupt(&self) -> Result<(), ProviderError> {
        if self.detached.load(Ordering::Acquire) {
            return Ok(());
        }
        match &self.inner().session {
            Some(session) => session.interrupt(),
            None => Err(refuse("the runner is unreachable")),
        }
    }

    fn close(&self) -> Result<(), ProviderError> {
        if self.detached.load(Ordering::Acquire) {
            return Ok(());
        }
        let session = self.inner().session.take();
        match session {
            Some(session) => session.close(),
            None => self.link.close_session(&self.key).map_err(refuse),
        }
    }
}

/// Connects to the runner serving `state_dir` and authenticates with its token: the stream, and
/// a reader over it that holds whatever the runner has sent since.
pub(crate) fn authenticated_stream(
    state_dir: &Path,
    limits: &RunnerLimits,
) -> Result<(std::net::TcpStream, std::io::BufReader<std::net::TcpStream>), String> {
    let not_running = |why: String| format!("no runner serves {} ({why})", state_dir.display());
    let info =
        super::RunnerInfo::read(state_dir).map_err(|error| not_running(error.to_string()))?;
    let token = std::fs::read_to_string(state_dir.join(super::TOKEN_FILE))
        .map_err(|error| not_running(error.to_string()))?;
    let address = info
        .address
        .parse()
        .map_err(|error: std::net::AddrParseError| not_running(error.to_string()))?;
    let stream = std::net::TcpStream::connect_timeout(&address, limits.connect_timeout.0)
        .map_err(|error| not_running(error.to_string()))?;
    let mut writer = stream.try_clone().map_err(|error| error.to_string())?;
    let mut reader =
        std::io::BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
    let authenticate = serde_json::json!({
        "jsonrpc": "2.0", "id": "ai6-connect", "method": "authenticate",
        "params": {"methodId": RUNNER_TOKEN_METHOD, "_meta": {"token": token.trim()}},
    });
    writeln!(writer, "{authenticate}").map_err(|error| error.to_string())?;
    // On the handle that reads: on Windows a clone of a socket keeps its own read timeout.
    reader
        .get_ref()
        .set_read_timeout(Some(limits.answer_wait.0))
        .map_err(|error| error.to_string())?;
    let mut answer = String::new();
    reader.read_line(&mut answer).map_err(|error| {
        format!(
            "the runner did not answer the authentication within {} s: {error}",
            limits.answer_wait.0.as_secs_f64()
        )
    })?;
    // The stream goes on to carry a Session, which may be silent for as long as it likes.
    reader
        .get_ref()
        .set_read_timeout(None)
        .map_err(|error| error.to_string())?;
    let answer: serde_json::Value = serde_json::from_str(answer.trim())
        .map_err(|_| "the runner did not answer the authentication".to_owned())?;
    if answer.get("error").is_some() {
        return Err("the runner refused the token".to_owned());
    }
    Ok((stream, reader))
}

/// What `ai6 runner connect` does: relays this process's standard input and output to the
/// runner serving `state_dir` and authenticates for the caller first, reaching its port within
/// `limits.connect_timeout`. A terminal is put in raw mode, then the ready line is printed.
///
/// # Errors
///
/// A sentence for the operator, when no runner serves or it refuses the token.
pub fn connect_stdio(state_dir: &Path, limits: &RunnerLimits) -> Result<(), String> {
    let (stream, mut reader) = authenticated_stream(state_dir, limits)?;
    let writer = stream;
    super::raw_terminal();
    println!("{READY_LINE}");
    let _ = std::io::stdout().flush();
    let mut to_runner = writer;
    let input = std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buffer = [0u8; 8192];
        while let Ok(count) = stdin.read(&mut buffer) {
            if count == 0 || to_runner.write_all(&buffer[..count]).is_err() {
                break;
            }
        }
        let _ = to_runner.shutdown(std::net::Shutdown::Write);
    });
    let mut stdout = std::io::stdout().lock();
    let mut buffer = [0u8; 8192];
    while let Ok(count) = reader.read(&mut buffer) {
        if count == 0 || stdout.write_all(&buffer[..count]).is_err() || stdout.flush().is_err() {
            break;
        }
    }
    drop(input);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Says it is ready on the first read, and records how long each read may wait.
    struct RecordingTerminal {
        waits: Arc<Mutex<Vec<i64>>>,
    }

    impl TerminalSession for RecordingTerminal {
        fn pid(&self) -> i64 {
            0
        }

        fn read(&self, dst: &mut [u8], timeout_ms: i64) -> Result<i64, ProviderError> {
            self.waits.lock().unwrap().push(timeout_ms);
            let line = format!("{READY_LINE}\n");
            dst[..line.len()].copy_from_slice(line.as_bytes());
            Ok(i64::try_from(line.len()).unwrap())
        }

        fn write(&self, _data: &[u8]) -> Result<(), ProviderError> {
            Ok(())
        }

        fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
            Ok(())
        }

        fn kill(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn exit_status(&self, _timeout_ms: i64) -> Result<guatiao::Value, ProviderError> {
            Err(refuse("not used"))
        }
    }

    /// A runner that accepts and then says nothing is given up on, whatever the transport does.
    #[test]
    fn a_runner_that_accepts_and_stays_silent_is_given_up_on() {
        let dir = tempfile::tempdir().unwrap();
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        std::fs::write(
            dir.path().join(super::super::INFO_FILE),
            serde_json::to_vec(&super::super::RunnerInfo {
                address: silent.local_addr().unwrap().to_string(),
                pid: 1,
                version: "test".to_owned(),
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(dir.path().join(super::super::TOKEN_FILE), "token").unwrap();
        let limits = RunnerLimits {
            answer_wait: crate::bounds::Seconds(Duration::from_millis(100)),
            ..RunnerLimits::default()
        };
        let path = dir.path().to_path_buf();
        let (done, answer) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(authenticated_stream(&path, &limits).map(|_| ()));
        });
        let error = answer
            .recv_timeout(Duration::from_secs(5))
            .expect("the read of the answer is bounded")
            .unwrap_err();
        assert!(error.contains("did not answer"), "{error}");
        drop(silent);
    }

    #[test]
    fn the_ready_line_is_read_at_the_terminal_poll() {
        let waits = Arc::new(Mutex::new(Vec::new()));
        let terminal = RecordingTerminal {
            waits: waits.clone(),
        };
        await_ready(
            &Arc::new(terminal.into_object()),
            Duration::from_secs(30),
            1234,
        )
        .unwrap();
        assert_eq!(*waits.lock().unwrap(), [1234]);
    }

    #[test]
    fn an_unreachable_runner_is_tried_again_after_the_retry_pause() {
        let dir = tempfile::tempdir().unwrap();
        let pause = Duration::from_secs(2);
        let link = RunnerLink::local(dir.path().join("none"), PathBuf::from("no-such-program"))
            .with_settings(RunnerHandoff::new(
                RunnerLimits {
                    retry_pause: crate::bounds::Seconds(pause),
                    ..RunnerLimits::default()
                },
                std::collections::BTreeMap::new(),
            ));
        let session = RemoteSession::drive(
            Arc::new(link),
            "k",
            None,
            None,
            Arc::new(AtomicBool::new(false)),
            ReachWatch::default(),
        );
        let started = Instant::now();
        assert!(session.next_event(0).unwrap().is_none());
        assert!(
            started.elapsed() >= pause,
            "tried again after {:?}",
            started.elapsed()
        );
    }
}
