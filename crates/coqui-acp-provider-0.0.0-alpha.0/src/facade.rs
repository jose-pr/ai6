//! Any [`AgentProvider`] as an ACP agent.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::time::Duration;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};

use agent_client_protocol::{
    Agent, Client, ConnectTo, ConnectionTo, Responder, UntypedMessage, on_receive_notification,
    on_receive_request,
};
use futures::channel::oneshot;
use futures::future::{self, Either};
use serde_json::{Map, Value};

use crate::kind::AgentProvider;

mod connection;
mod operation;
mod options;
mod updates;

use connection::Connection;
pub use options::{OptionTarget, OptionValue, StartOption};

/// The `_meta` key of `session/new` and `session/resume` that carries what ACP has no field for:
/// `{"context": [text], "environment": {name: text}, "model": text, "effort": text, "meta": any}`, each
/// optional. An unknown key is refused.
pub const LAUNCH_META_KEY: &str = "dev.boriken.coqui.acp/launch";

/// The `agentCapabilities._meta` key with which the facade says it sends [`EVENT_NOTIFICATION`].
pub const EVENTS_CAPABILITY: &str = "dev.boriken.coqui.acp/events";

/// The notification for what ACP has no update for: `{"sessionId", "providerSessionId", "event"}`, with
/// the [`AgentEvent`](crate::AgentEvent) as JSON. No `event` says the provider's identity is now known.
pub const EVENT_NOTIFICATION: &str = "_coqui_acp/event";

/// The `agentCapabilities._meta` key with which the facade says it answers [`SESSION_OPTIONS_METHOD`].
pub const SESSION_OPTIONS_CAPABILITY: &str = "dev.boriken.coqui.acp/session_options";

/// The request for what a session can be started with, before any session: `{}`, answered
/// `{"configOptions": [SessionConfigOption], "modes": SessionModeState, "unknown": {part: reason}}`, from
/// [`Settings::options`]; a part the host did not declare is left out, and `unknown` says so.
pub const SESSION_OPTIONS_METHOD: &str = "_coqui_acp/session_options";

/// Judges the `_meta` of an `authenticate` request.
pub type Credential = dyn Fn(&Map<String, Value>) -> bool + Send + Sync;

/// A method every client must authenticate with before it opens or resumes a session.
#[derive(Clone)]
pub struct FacadeAuth {
    /// The advertised method id.
    pub method: String,
    /// A name for people.
    pub name: String,
    /// Whether the `authenticate` request's `_meta` proves the client.
    pub check: Arc<Credential>,
}

impl fmt::Debug for FacadeAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FacadeAuth")
            .field("method", &self.method)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// The limits of one connection. Each default is safe for a client the host does not fully trust.
#[derive(Clone)]
#[non_exhaustive]
pub struct Settings {
    /// Sessions one connection may hold, closed sessions whose provider has not returned included: a new
    /// one past this is refused. Default 8.
    pub max_sessions: usize,
    /// The timeout given to each `next_event` call. Default 100 ms.
    pub poll: Duration,
    /// How long `start` or `resume` may take before the turn fails and the session is ended. If it returns
    /// later what it returned is closed. Default 60 s.
    pub start_timeout: Duration,
    /// How long any other provider call may take past its own timeout before the session is ended. A
    /// thread stuck in a provider cannot be stopped, so it stays, and its session keeps its place in
    /// `max_sessions`. Default 30 s.
    pub call_timeout: Duration,
    /// How long a cancelled turn may take to end before its prompt is answered `cancelled` anyway. Default
    /// 10 s.
    pub cancel_grace: Duration,
    /// How long `session/close`, or a connection ending, waits for a provider's `close`. Default 10 s.
    pub close_timeout: Duration,
    /// The largest prompt taken. Default 8 MiB.
    pub max_prompt_bytes: usize,
    /// What a provider may send in one turn, or between turns, as the lines it becomes, before the session is
    /// ended. Default 64 MiB.
    pub max_turn_bytes: usize,
    /// What one connection may send in all, as lines, before it is ended. The protocol library queues what
    /// goes out without limit and says nothing of what the client has read, so this is what bounds the
    /// memory a client that never reads can cost. Default 256 MiB.
    pub max_connection_bytes: usize,
    /// The most a text or an event takes in one line, as JSON: a longer text is split, and an event cut, since
    /// a line transport ends at its line limit. A line is at most this, twice `max_label_bytes`, and 5 KiB.
    /// Default 1 MiB.
    pub max_update_bytes: usize,
    /// The most a tool's title or name, a failure text or an identity takes, as JSON: past it the text is
    /// cut. Default 4 KiB.
    pub max_label_bytes: usize,
    /// Failed `authenticate` requests before the connection is ended: from then on every request on it is
    /// refused unread. Default 5.
    pub max_auth_failures: u32,
    /// The directories a session may work in, compared as written; a `cwd` with `..` or `.` in it is
    /// refused. Empty, the default, takes any absolute path, a network share's included.
    pub working_roots: Vec<PathBuf>,
    /// Environment values given to every `start` and `resume`.
    pub environment: BTreeMap<String, String>,
    /// The environment names a client may set through [`LAUNCH_META_KEY`], over `environment`; any other is
    /// refused. A client that can set `PATH` runs what it likes, so the default is none.
    pub client_environment: Vec<String>,
    /// What a client may choose before a session starts, each from its own list: `session/new` and
    /// `session/resume` answer them as `configOptions`, one of category `mode` also as `modes`, and
    /// `session/set_config_option` and `session/set_mode` change them until the provider starts. Empty, the
    /// default, offers none, and a launch's `model` and `effort` pass as the client wrote them.
    pub options: Vec<StartOption>,
}

impl fmt::Debug for Settings {
    /// Names the environment and shows none of its values, which may be credentials.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("max_sessions", &self.max_sessions)
            .field("poll", &self.poll)
            .field("start_timeout", &self.start_timeout)
            .field("call_timeout", &self.call_timeout)
            .field("cancel_grace", &self.cancel_grace)
            .field("close_timeout", &self.close_timeout)
            .field("max_prompt_bytes", &self.max_prompt_bytes)
            .field("max_turn_bytes", &self.max_turn_bytes)
            .field("max_connection_bytes", &self.max_connection_bytes)
            .field("max_update_bytes", &self.max_update_bytes)
            .field("max_label_bytes", &self.max_label_bytes)
            .field("max_auth_failures", &self.max_auth_failures)
            .field("working_roots", &self.working_roots)
            .field("environment", &self.environment.keys())
            .field("client_environment", &self.client_environment)
            .field("options", &self.options)
            .finish()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_sessions: 8,
            poll: Duration::from_millis(100),
            start_timeout: Duration::from_secs(60),
            call_timeout: Duration::from_secs(30),
            cancel_grace: Duration::from_secs(10),
            close_timeout: Duration::from_secs(10),
            max_prompt_bytes: 8 * 1024 * 1024,
            max_turn_bytes: 64 * 1024 * 1024,
            max_connection_bytes: 256 * 1024 * 1024,
            max_update_bytes: 1024 * 1024,
            max_label_bytes: 4 * 1024,
            max_auth_failures: 5,
            working_roots: Vec::new(),
            environment: BTreeMap::new(),
            client_environment: Vec::new(),
            options: Vec::new(),
        }
    }
}

/// Why a connection ended badly.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The connection failed, in the protocol library's words.
    Connection(String),
    /// A thread could not be started.
    Thread(std::io::Error),
    /// This many sessions' providers had not returned from `close` when the connection ended. Their
    /// threads stay until they do.
    Stuck(usize),
    /// [`Settings::options`] cannot be served, and why: an id or a value twice, a default not among the
    /// values, two options with one target, or two of category `mode`.
    Settings(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(why) => write!(f, "the ACP connection failed: {why}"),
            Self::Thread(_) => f.write_str("a thread could not be started"),
            Self::Stuck(count) => write!(f, "{count} sessions did not close in time"),
            Self::Settings(why) => write!(f, "the settings cannot be served: {why}"),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Thread(error) => Some(error),
            _ => None,
        }
    }
}

/// Serves any [`AgentProvider`] as an ACP agent: one provider session per ACP session, started on its
/// first `session/prompt`. Sessions belong to the connection that opened them and end with it.
#[derive(Clone)]
pub struct AcpFacade {
    provider: Arc<dyn AgentProvider>,
    /// Shared by every clone and connection, so no identity is held twice.
    held: Arc<Mutex<BTreeSet<String>>>,
    auth: Option<FacadeAuth>,
    settings: Settings,
    name: String,
    version: String,
}

impl fmt::Debug for AcpFacade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcpFacade")
            .field("auth", &self.auth)
            .field("name", &self.name)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl AcpFacade {
    /// A facade over `provider`, with the default [`Settings`] and no authentication.
    #[must_use]
    pub fn new(provider: Arc<dyn AgentProvider>) -> Self {
        Self {
            provider,
            held: Arc::new(Mutex::new(BTreeSet::new())),
            auth: None,
            settings: Settings::default(),
            name: env!("CARGO_PKG_NAME").to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Requires clients to authenticate before opening a session.
    #[must_use]
    pub fn with_auth(mut self, auth: FacadeAuth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Replaces the limits.
    #[must_use]
    pub fn with_settings(mut self, settings: Settings) -> Self {
        self.settings = settings;
        self
    }

    /// The name and version `initialize` answers with; by default this crate's.
    #[must_use]
    pub fn with_agent_info(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.name = name.into();
        self.version = version.into();
        self
    }

    /// Serves one connection on this thread until its transport closes, then closes its sessions.
    ///
    /// # Errors
    ///
    /// [`Error::Connection`] when the transport or the protocol failed; [`Error::Thread`] when the
    /// connection's supervising thread could not start; [`Error::Stuck`] when a provider's `close` had not
    /// returned within [`Settings::close_timeout`]; [`Error::Settings`] before anything is served, when
    /// [`Settings::options`] cannot be.
    pub fn run(&self, transport: impl ConnectTo<Agent> + 'static) -> Result<(), Error> {
        options::check(&self.settings.options).map_err(Error::Settings)?;
        let (stop, stopped) = oneshot::channel::<()>();
        let connection = Connection::new(self, stop);
        let supervisor = connection.supervise().map_err(Error::Thread)?;
        let for_requests = connection.clone();
        let for_notifications = connection.clone();
        let served = futures::executor::block_on(
            Agent
                .builder()
                .name(self.name.clone())
                .on_receive_request(
                    async move |message: UntypedMessage,
                                responder: Responder<Value>,
                                cx: ConnectionTo<Client>| {
                        for_requests.request(&message, responder, cx);
                        Ok(())
                    },
                    on_receive_request!(),
                )
                .on_receive_notification(
                    async move |message: UntypedMessage, _cx: ConnectionTo<Client>| {
                        for_notifications.notification(&message);
                        Ok(())
                    },
                    on_receive_notification!(),
                )
                .connect_with(transport, async move |cx: ConnectionTo<Client>| {
                    let closed = core::pin::pin!(cx.incoming_closed());
                    match future::select(closed, stopped).await {
                        Either::Left(_) | Either::Right(_) => Ok(()),
                    }
                }),
        );
        let stuck = connection.shut_down();
        let _ = supervisor.join();
        served.map_err(|error| Error::Connection(error.to_string()))?;
        match stuck {
            0 => Ok(()),
            count => Err(Error::Stuck(count)),
        }
    }

    /// [`run`](Self::run) on a thread of its own.
    ///
    /// # Errors
    ///
    /// [`Error::Thread`] when the thread could not start; the handle answers what `run` does.
    pub fn serve(
        &self,
        transport: impl ConnectTo<Agent> + 'static,
    ) -> Result<JoinHandle<Result<(), Error>>, Error> {
        let facade = self.clone();
        thread::Builder::new()
            .name("coqui-acp-facade".to_string())
            .spawn(move || facade.run(transport))
            .map_err(Error::Thread)
    }
}
