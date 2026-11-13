//! A harness served as an Agent Client Protocol agent, in version 1's shapes or version 2's, whichever the client
//! asks for.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::time::Duration;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;

use serde_json::{Map, Value};

use coqui_acp_api::{Bucket, UsageSupport};

use crate::event::{Content, Event};

mod connection;
mod parse;
mod session;
mod shape;
mod usage;

pub use connection::serve;
pub use usage::UsageTracker;
#[cfg(feature = "antigravity")]
pub(crate) use usage::now;

/// `path` when it is absolute by this platform's rules: a drive or UNC path on Windows, `/…` elsewhere. `what` names it
/// in the refusal.
///
/// # Errors
///
/// The refusal, for a relative path or one with a NUL character.
pub fn absolute_path(path: &str, what: &str) -> Result<PathBuf, String> {
    parse::absolute(path, what)
}

/// `path` with every link in the part of it that exists resolved, and the rest as written: what a check of where a path
/// leads has to compare.
#[must_use]
pub fn resolved_path(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(found) = std::fs::canonicalize(existing) {
            let mut resolved = found;
            for part in rest.iter().rev() {
                resolved.push(part);
            }
            return resolved;
        }
        let (Some(parent), Some(part)) = (existing.parent(), existing.file_name()) else {
            return path.to_path_buf();
        };
        rest.push(part);
        existing = parent;
    }
}

/// Whether `path`, followed through its links, lies in one of `roots`, followed through theirs.
#[must_use]
pub fn within_roots(path: &Path, roots: &[PathBuf]) -> bool {
    // How it is written decides first: resolving a network or device path would open a connection to its host.
    if parse::network(path) {
        return false;
    }
    let plain = path
        .components()
        .all(|part| !matches!(part, Component::ParentDir | Component::CurDir));
    if !plain {
        return false;
    }
    let resolved = resolved_path(path);
    roots
        .iter()
        .any(|root| resolved.starts_with(resolved_path(root)))
}

/// Whether a host's narrowing takes `path`: inside `roots` unless there are none, and not on the network, as written or
/// where its links lead, unless `network_paths`.
///
/// # Errors
///
/// The refusal.
pub fn path_admitted(path: &Path, roots: &[PathBuf], network_paths: bool) -> Result<(), String> {
    if !network_paths && (parse::network(path) || parse::network_resolved(&resolved_path(path))) {
        return Err("a network or device path is not taken".into());
    }
    if !roots.is_empty() && !within_roots(path, roots) {
        return Err("the directory is outside the working roots".into());
    }
    Ok(())
}

/// Whether a working directory the person chose, a session's `cwd` or one of its additional directories, is taken: inside
/// `roots` unless there are none, and in no device namespace.
///
/// On Windows a folder on a share (`\\server\share\work`) is an ordinary absolute path: held to the roots by its parts,
/// without regard to case or which separator it is written with, with a `.` or `..` part refused, and not resolved first
/// when there are no roots to compare it with. A path written as a local one whose links lead to a share is still refused
/// unless `network_paths`, which also still covers what a harness takes from a request outside these two fields. Elsewhere
/// the rule is [`path_admitted`]'s.
///
/// # Errors
///
/// The refusal.
pub fn directory_admitted(
    path: &Path,
    roots: &[PathBuf],
    network_paths: bool,
) -> Result<(), String> {
    if !cfg!(windows) {
        return path_admitted(path, roots, network_paths);
    }
    if parse::device(path) {
        return Err("a device path is not a directory".into());
    }
    if !parse::network(path) {
        return path_admitted(path, roots, network_paths);
    }
    if parse::dotted(path) {
        return Err("the directory has a . or .. part".into());
    }
    let text = path.to_string_lossy();
    let roots: Vec<String> = roots
        .iter()
        .map(|root| root.to_string_lossy().into_owned())
        .collect();
    coqui_acp_api::WorkingRoots::new(roots)
        .admits(&text)
        .map_err(|refusal| refusal.to_string())
}

/// The `_meta` key of a turn's answer, or of a version 2 `idle`, that says a quota refused the turn: a
/// [`LimitHit`](coqui_acp_api::LimitHit) as JSON.
pub use coqui_acp_api::LIMIT_META_KEY;

/// The id of the plan of a session in version 2, where plans have ids; a harness has one plan per session.
pub const PLAN_ID: &str = "plan";

/// Who the agent is, for `initialize`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AgentInfo {
    /// Its name, such as `coqui-acp-claude`.
    pub name: String,
    /// Its name for people, such as `Claude Code`.
    pub title: String,
    /// Its version.
    pub version: String,
}

impl AgentInfo {
    /// Who the agent is.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        title: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            title: title.into(),
            version: version.into(),
        }
    }
}

/// What the harness takes, which `initialize` advertises and requests are checked against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Takes {
    /// Images in a prompt.
    pub images: bool,
    /// Resources' content in a prompt.
    pub embedded_context: bool,
    /// MCP servers reached over HTTP.
    pub mcp_http: bool,
    /// MCP servers reached over server-sent events, which version 2 does not have.
    pub mcp_sse: bool,
    /// Loading a session with its history replayed.
    pub load: bool,
    /// More directories than the working one.
    pub additional_directories: bool,
    /// A system prompt appended to the harness's own.
    pub system_prompt: bool,
}

impl Takes {
    /// Takes nothing beyond the baseline: text prompts, stdio MCP servers, resume and close.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            images: false,
            embedded_context: false,
            mcp_http: false,
            mcp_sse: false,
            load: false,
            additional_directories: false,
            system_prompt: false,
        }
    }

    /// Images in a prompt.
    #[must_use]
    pub const fn images(mut self, takes: bool) -> Self {
        self.images = takes;
        self
    }

    /// Resources' content in a prompt.
    #[must_use]
    pub const fn embedded_context(mut self, takes: bool) -> Self {
        self.embedded_context = takes;
        self
    }

    /// MCP servers over HTTP.
    #[must_use]
    pub const fn mcp_http(mut self, takes: bool) -> Self {
        self.mcp_http = takes;
        self
    }

    /// MCP servers over server-sent events.
    #[must_use]
    pub const fn mcp_sse(mut self, takes: bool) -> Self {
        self.mcp_sse = takes;
        self
    }

    /// Loading a session with its history.
    #[must_use]
    pub const fn load(mut self, takes: bool) -> Self {
        self.load = takes;
        self
    }

    /// More directories.
    #[must_use]
    pub const fn additional_directories(mut self, takes: bool) -> Self {
        self.additional_directories = takes;
        self
    }

    /// An appended system prompt.
    #[must_use]
    pub const fn system_prompt(mut self, takes: bool) -> Self {
        self.system_prompt = takes;
        self
    }
}

/// How an MCP server is reached.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpTransport {
    /// A program the harness starts.
    Stdio {
        /// The program, an absolute path.
        command: String,
        /// Its arguments.
        args: Vec<String>,
        /// Its environment, by name.
        env: Vec<(String, String)>,
    },
    /// Streamable HTTP.
    Http {
        /// The endpoint.
        url: String,
        /// Headers, by name.
        headers: Vec<(String, String)>,
    },
    /// Server-sent events.
    Sse {
        /// The endpoint.
        url: String,
        /// Headers, by name.
        headers: Vec<(String, String)>,
    },
}

/// `url` without what may be a credential: the user and password before its host, its query and its fragment.
fn address(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or_default()
}

/// [`address`], also without the user and password.
fn shown_address(url: &str) -> String {
    let url = address(url);
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let host = authority.rsplit('@').next().unwrap_or_default();
    alloc::format!("{scheme}://{host}{path}")
}

impl fmt::Debug for McpTransport {
    /// Names the environment and headers and counts the arguments, never showing their values, which may be credentials,
    /// and shows an address without its user, query or fragment.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = |pairs: &[(String, String)]| {
            pairs
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>()
        };
        match self {
            Self::Stdio { command, args, env } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", &args.len())
                .field("env", &names(env))
                .finish(),
            Self::Http { url, headers } => f
                .debug_struct("Http")
                .field("url", &shown_address(url))
                .field("headers", &names(headers))
                .finish(),
            Self::Sse { url, headers } => f
                .debug_struct("Sse")
                .field("url", &shown_address(url))
                .field("headers", &names(headers))
                .finish(),
        }
    }
}

/// An MCP server the client gives a session.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpServer {
    /// Its name, unique in the session.
    pub name: String,
    /// How it is reached.
    pub transport: McpTransport,
}

impl McpServer {
    /// A server.
    #[must_use]
    pub fn new(name: impl Into<String>, transport: McpTransport) -> Self {
        Self {
            name: name.into(),
            transport,
        }
    }
}

impl fmt::Debug for McpServer {
    /// As [`McpTransport`]'s.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("McpServer");
        debug.field("name", &self.name);
        match &self.transport {
            McpTransport::Stdio { command, args, env } => debug
                .field("command", command)
                .field("args", &args.len())
                .field("env", &env.iter().map(|(name, _)| name).collect::<Vec<_>>()),
            McpTransport::Http { url, headers } | McpTransport::Sse { url, headers } => {
                debug.field("url", &shown_address(url)).field(
                    "headers",
                    &headers.iter().map(|(name, _)| name).collect::<Vec<_>>(),
                )
            }
        };
        debug.finish()
    }
}

/// Which session to open.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Open {
    /// A new one.
    New,
    /// An earlier one by the harness's id, its history not replayed.
    Resume(String),
    /// An earlier one by the harness's id, its history replayed as events before `open` returns.
    Load(String),
}

/// What a session is opened with.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenRequest {
    /// The working directory, absolute.
    pub cwd: PathBuf,
    /// Which session.
    pub open: Open,
    /// MCP servers to start for it.
    pub mcp_servers: Vec<McpServer>,
    /// Text appended to the harness's system prompt.
    pub system_prompt: Option<String>,
    /// More directories it may work in, absolute.
    pub additional_directories: Vec<PathBuf>,
    /// [`Settings::working_roots`]: what a harness that finds a session by its id alone checks that session against.
    pub working_roots: Vec<PathBuf>,
    /// [`Settings::network_paths`]: for what a harness takes outside this request's fields.
    pub network_paths: bool,
    /// The request's `_meta`, for what ACP has no field for: the client's, untrusted.
    pub meta: Map<String, Value>,
}

impl OpenRequest {
    /// Whether the host's narrowing takes a path the harness read from this request's other parts, as `cwd` and
    /// `additionalDirectories` are checked.
    ///
    /// # Errors
    ///
    /// The refusal.
    pub fn path_admitted(&self, path: &Path) -> Result<(), String> {
        path_admitted(path, &self.working_roots, self.network_paths)
    }

    /// A new session in `cwd`, with nothing else.
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            open: Open::New,
            mcp_servers: Vec::new(),
            system_prompt: None,
            working_roots: Vec::new(),
            network_paths: false,
            additional_directories: Vec::new(),
            meta: Map::new(),
        }
    }
}

/// One value of a [`ConfigOption`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OptionValue {
    /// Its id, which `session/set_config_option` names.
    pub id: String,
    /// Its name for people.
    pub name: String,
    /// What it means.
    pub description: Option<String>,
}

impl OptionValue {
    /// A value.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description,
        }
    }
}

/// A choice the session offers, such as its model or effort, as an ACP select.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConfigOption {
    /// Its id.
    pub id: String,
    /// Its name for people.
    pub name: String,
    /// What it chooses.
    pub description: Option<String>,
    /// ACP's category: `model`, `thought_level` or `mode`.
    pub category: Option<String>,
    /// The value in use.
    pub current: String,
    /// The values offered.
    pub values: Vec<OptionValue>,
}

impl ConfigOption {
    /// An option with no description or category.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        current: impl Into<String>,
        values: Vec<OptionValue>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            category: None,
            current: current.into(),
            values,
        }
    }

    /// Sets the category.
    #[must_use]
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    /// Sets the description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Why a harness could not do what was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HarnessError {
    /// The request asks for what the harness cannot do: answered invalid params.
    Refused(String),
    /// The harness failed, or is gone: answered internal error.
    Failed(String),
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(why) => write!(f, "refused: {why}"),
            Self::Failed(why) => write!(f, "failed: {why}"),
        }
    }
}

impl core::error::Error for HarnessError {}

/// Where a session's events go. Cloned freely; sending after the session is gone does nothing.
#[derive(Clone, Debug)]
pub struct EventSink(pub(crate) mpsc::SyncSender<session::Item>);

impl EventSink {
    /// Sends `event`, waiting while [`Settings::max_queued_events`] are queued for a client that has not read them.
    /// Answers `false` once the session is gone.
    pub fn send(&self, event: Event) -> bool {
        self.0.send(session::Item::Event(Box::new(event))).is_ok()
    }
}

/// A coding agent's command line, which [`serve`] serves over ACP.
pub trait Harness: Send + Sync + 'static {
    /// Who the agent is.
    fn info(&self) -> AgentInfo;

    /// What it takes.
    fn takes(&self) -> Takes;

    /// What usage it reports. When it reports any, `initialize` advertises the usage extension and
    /// `_coqui_acp/usage` is answered from the [`Event::Buckets`], [`Event::Usage`]'s spend and
    /// [`TurnEnd::Limited`](crate::event::TurnEnd::Limited) it sends. The default, for a harness
    /// that reports none: none, which is a true answer.
    fn usage(&self) -> UsageSupport {
        UsageSupport::none()
    }

    /// Reads the account's usage with no session open and no turn, for `_coqui_acp/usage` when
    /// [`UsageSupport::needs_turn`] is false and no open session could answer. Called on every such
    /// request, so a host's polling sets how often; it may start the harness for the read. `None`,
    /// the default, leaves the answer to what the connection has seen.
    fn read_usage(&self) -> Option<Vec<Bucket>> {
        None
    }

    /// Opens a session, sending what it reports to `events`. For [`Open::Load`] the history is sent before this
    /// returns. It returns once the harness's session id is known.
    ///
    /// # Errors
    ///
    /// The harness could not start, or refused the request.
    fn open(
        &self,
        request: OpenRequest,
        events: EventSink,
    ) -> Result<Box<dyn Conversation>, HarnessError>;
}

/// An open session of a harness. Any method may be called while another runs.
pub trait Conversation: Send + Sync {
    /// The harness's id for the session.
    fn session_id(&self) -> String;

    /// The choices the session offers, at their current values.
    fn options(&self) -> Vec<ConfigOption>;

    /// Reads the account's usage from the running harness, with no turn. `None`, the default,
    /// when it cannot; [`Harness::read_usage`] is then asked.
    fn read_usage(&self) -> Option<Vec<Bucket>> {
        None
    }

    /// Starts a turn. Its events and its end go to the session's [`EventSink`].
    ///
    /// # Errors
    ///
    /// The prompt could not be sent, or holds what the harness does not take.
    fn prompt(&self, content: Vec<Content>) -> Result<(), HarnessError>;

    /// Interrupts the running turn, which then ends as cancelled; nothing when none runs.
    ///
    /// # Errors
    ///
    /// The request could not be sent.
    fn interrupt(&self) -> Result<(), HarnessError>;

    /// Sets the option `id` to `value`, for the turns that follow, and answers every option.
    ///
    /// # Errors
    ///
    /// No such option or value, or the harness refused.
    fn set_option(&self, id: &str, value: &str) -> Result<Vec<ConfigOption>, HarnessError>;

    /// Ends the harness. Returns once it has exited, or after a short wait when that cannot be told.
    fn close(&self);
}

/// What [`Settings::on_notice`] calls with each notice.
pub type NoticeHook = Arc<dyn Fn(&str) + Send + Sync>;

/// What [`Settings::on_usage`] calls with the connection's report each time it changes.
pub type UsageHook = Arc<dyn Fn(&coqui_acp_api::UsageReport) + Send + Sync>;

/// The limits of one connection. Every one is a setting with a default, and any value is taken.
#[derive(Clone)]
#[non_exhaustive]
pub struct Settings {
    /// The longest line read from the client; a longer one ends the connection. Default 64 MiB.
    pub max_line: u64,
    /// Sessions one connection may hold; another is refused. Default 8.
    pub max_sessions: usize,
    /// Requests being worked on at once; another is refused. Default 64.
    pub max_requests: usize,
    /// The largest prompt taken, as the JSON of its blocks. Default 16 MiB.
    pub max_prompt_bytes: usize,
    /// The highest ACP version answered: 2, or 1 to speak version 1 to every client. Default 2.
    pub max_version: u8,
    /// How long a cancelled turn may take to end before it is answered `cancelled` anyway. Default 10 s.
    pub cancel_grace: Duration,
    /// How long a permission request waits for the client before it is answered cancelled; `None` waits for as long
    /// as the person takes. Default `None`.
    pub permission_timeout: Option<Duration>,
    /// The tool calls a session keeps track of, for version 1's `tool_call` and its updates; past it the oldest is
    /// forgotten, and a later report of it is a new `tool_call`. Default 4096.
    pub max_tools: usize,
    /// Events of a session queued for the client; past it the harness's reader waits, so a client that does not
    /// read holds the harness back instead of filling memory. The same many are kept of what a session says before its
    /// answer has gone; past it they are dropped, and the client is told how many. Default 4096.
    pub max_queued_events: usize,
    /// Version 1 sends a tool call's content whole with each piece: the bytes kept of each, past which more is left out
    /// and the client told so. Default 1 MiB.
    pub max_tool_content_bytes: usize,
    /// The same for how many pieces. Default 256.
    pub max_tool_content_items: usize,
    /// A session's questions open at once; past it one is answered cancelled without being asked. Each open question
    /// holds a thread. Default 64.
    pub max_open_questions: usize,
    /// Whether a client may give a session MCP servers; when not, a request with any is refused. Default true.
    pub client_mcp_servers: bool,
    /// The directories a session's `cwd`, more directories and what a harness reads from the request must lie in, links
    /// followed as far as the path exists; empty takes any. Default empty.
    pub working_roots: Vec<PathBuf>,
    /// Whether a path may be a network or device path (`\\host\share`, `//host/share`, `\\?\…`, `\\.\…`); on
    /// Windows a process started in one authenticates to the host. Default false.
    pub network_paths: bool,
    /// Called with each notice the harness gives, such as a warning about its version, for a host that logs them.
    pub on_notice: Option<NoticeHook>,
    /// Called with the connection's report, as `_coqui_acp/usage` answers it with no session, each time something other than
    /// when it was read changes, for a host that keeps what its connections learned. Called with no lock held. Default none.
    pub on_usage: Option<UsageHook>,
    /// The host's id for the account the connection runs on, echoed into every usage report and never
    /// interpreted; over the one the client names in its capabilities. Default none.
    pub account: Option<String>,
}

impl Settings {
    /// The defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_line: 64 * 1024 * 1024,
            max_sessions: 8,
            max_requests: 64,
            max_prompt_bytes: 16 * 1024 * 1024,
            max_version: 2,
            cancel_grace: Duration::from_secs(10),
            permission_timeout: None,
            max_tools: 4096,
            max_queued_events: 4096,
            max_open_questions: 64,
            max_tool_content_bytes: 1024 * 1024,
            max_tool_content_items: 256,
            client_mcp_servers: true,
            working_roots: Vec::new(),
            network_paths: false,
            on_notice: None,
            on_usage: None,
            account: None,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Settings")
            .field("max_line", &self.max_line)
            .field("max_sessions", &self.max_sessions)
            .field("max_requests", &self.max_requests)
            .field("max_prompt_bytes", &self.max_prompt_bytes)
            .field("max_version", &self.max_version)
            .field("cancel_grace", &self.cancel_grace)
            .field("permission_timeout", &self.permission_timeout)
            .field("max_tools", &self.max_tools)
            .field("max_queued_events", &self.max_queued_events)
            .field("max_open_questions", &self.max_open_questions)
            .field("max_tool_content_bytes", &self.max_tool_content_bytes)
            .field("max_tool_content_items", &self.max_tool_content_items)
            .field("client_mcp_servers", &self.client_mcp_servers)
            .field("working_roots", &self.working_roots)
            .field("network_paths", &self.network_paths)
            .field("on_notice", &self.on_notice.is_some())
            .field("on_usage", &self.on_usage.is_some())
            .field("account", &self.account)
            .finish()
    }
}
