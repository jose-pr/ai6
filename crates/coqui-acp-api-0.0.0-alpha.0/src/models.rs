//! What crosses the `acp_harness` kind besides the channel and the spawner: plain data, the same in
//! every language.
//!
//! What a host sends (`StartRequest`, `Requirement`, `SessionQuery`, `SessionImport`, `SessionExport`) refuses a key it does
//! not declare, so a field a newer host adds is never silently dropped by an older provider. What
//! a provider answers is read leniently.

use alloc::string::String;
use alloc::vec::Vec;

use guatiao::{FromValue, Schema, Secret, ToValue};

use crate::environment::Environment;
use crate::requirement::Requirement;
use crate::usage::UsageSupport;

/// What a harness's `initialize` will advertise, known before anything starts. `initialize`'s own
/// answer governs; this is what a host shows before it has one.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct Takes {
    /// Images in a prompt.
    #[map(default)]
    pub images: bool,
    /// Audio in a prompt.
    #[map(default)]
    pub audio: bool,
    /// Resources' content in a prompt.
    #[map(default)]
    pub embedded_context: bool,
    /// MCP servers reached over HTTP.
    #[map(default)]
    pub mcp_http: bool,
    /// MCP servers reached over server-sent events.
    #[map(default)]
    pub mcp_sse: bool,
    /// `session/load`: a session reopened with its history replayed.
    #[map(default)]
    pub load: bool,
    /// `session/resume`: a session reopened without its history replayed.
    #[map(default)]
    pub resume: bool,
    /// More directories than the working one.
    #[map(default)]
    pub additional_directories: bool,
    /// A system prompt appended to the harness's own.
    #[map(default)]
    pub system_prompt: bool,
}

impl Takes {
    /// Takes nothing beyond text prompts and stdio MCP servers.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            images: false,
            audio: false,
            embedded_context: false,
            mcp_http: false,
            mcp_sse: false,
            load: false,
            resume: false,
            additional_directories: false,
            system_prompt: false,
        }
    }
}

/// Who a harness is, from [`AcpHarness::describe`](crate::AcpHarness::describe), as configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct HarnessInfo {
    /// Its name for people, such as `Claude Code`.
    pub title: String,
    /// The ACP versions its `initialize` answers, lowest first.
    pub versions: Vec<u16>,
    /// What its `initialize` will advertise.
    #[map(default)]
    pub takes: Takes,
    /// Whether a session held to requirements can be reopened (`session/load` or `session/resume`)
    /// on a later connection started with fewer, and then have what that connection allows.
    #[map(default)]
    pub resumes_wider: bool,
    /// What usage it reports, and so whether its `initialize` advertises the usage extension.
    #[map(default)]
    pub usage: UsageSupport,
}

impl HarnessInfo {
    /// A harness named `title` for people, answering the ACP `versions`, taking nothing more and
    /// reporting no usage.
    #[must_use]
    pub fn new(title: impl Into<String>, versions: Vec<u16>) -> Self {
        Self {
            title: title.into(),
            versions,
            takes: Takes::new(),
            resumes_wider: false,
            usage: UsageSupport::none(),
        }
    }
}

/// What a host starts one connection with: what the provider cannot learn from ACP, on top of the
/// provider's own configuration. Everything in it only narrows.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct StartRequest {
    /// Variables for what the harness runs, over its configuration's. A provider shows none of
    /// them in an answer, a refusal, an error or a log.
    #[map(default)]
    pub environment: Secret<Environment>,
    /// The directories every session's `cwd` and additional directories must lie in, compared as
    /// [`WorkingRoots`](crate::WorkingRoots) says; empty adds none.
    #[map(default)]
    pub working_roots: Vec<String>,
    /// What the connection is held to. A name may come once.
    #[map(default)]
    pub requirements: Vec<Requirement>,
    /// The host's id for the account the connection runs on, echoed into every usage report of
    /// the connection and never interpreted. It picks no credentials: those stay in
    /// `environment`.
    pub account: Option<String>,
}

impl StartRequest {
    /// A request adding nothing to the provider's configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A request to fill in, starting from [`new`](Self::new).
    pub fn builder() -> StartRequestBuilder {
        StartRequestBuilder(Self::new())
    }
}

/// Builds a [`StartRequest`]; from [`StartRequest::builder`].
#[derive(Debug, Clone)]
#[must_use]
pub struct StartRequestBuilder(StartRequest);

impl StartRequestBuilder {
    /// The variables for what the harness runs.
    pub fn environment(mut self, environment: Environment) -> Self {
        self.0.environment = Secret::new(environment);
        self
    }

    /// Adds a working root.
    pub fn working_root(mut self, root: impl Into<String>) -> Self {
        self.0.working_roots.push(root.into());
        self
    }

    /// Adds a requirement.
    pub fn requirement(mut self, requirement: impl Into<Requirement>) -> Self {
        self.0.requirements.push(requirement.into());
        self
    }

    /// The host's id for the account.
    pub fn account(mut self, account: impl Into<String>) -> Self {
        self.0.account = Some(account.into());
        self
    }

    /// The request.
    #[must_use]
    pub fn build(self) -> StartRequest {
        self.0
    }
}

/// One model a harness says an account may use.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct ModelInfo {
    /// The harness's own identifier or alias for it.
    pub id: String,
    /// What the harness shows a person, when it has a name apart from the identifier.
    pub name: Option<String>,
    /// The harness's one-line description, when it gives one.
    pub description: Option<String>,
}

impl ModelInfo {
    /// The model `id`, with no name or description.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            description: None,
        }
    }
}

/// What a harness can say about the models an account may use, with no model turn. A harness that
/// cannot list says so; it never guesses.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct ModelListing {
    /// Whether the harness listed them; when not, `models` is empty.
    pub known: bool,
    /// Why it could not, when `known` is false.
    pub unknown_because: Option<String>,
    /// The models, in the harness's order.
    #[map(default)]
    pub models: Vec<ModelInfo>,
}

impl ModelListing {
    /// A listing the harness could not give, with the reason.
    #[must_use]
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            known: false,
            unknown_because: Some(reason.into()),
            models: Vec::new(),
        }
    }

    /// The models a harness listed.
    #[must_use]
    pub fn of(models: Vec<ModelInfo>) -> Self {
        Self {
            known: true,
            unknown_because: None,
            models,
        }
    }
}

/// One session a harness keeps, for [`AcpHarness::session_record`](crate::AcpHarness::session_record).
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct SessionQuery {
    /// The session's id as the provider reported it: a UUID.
    pub session_id: String,
    /// The directory the session worked in.
    pub working_directory: String,
    /// The variables its connection was started with.
    #[map(default)]
    pub environment: Secret<Environment>,
}

impl SessionQuery {
    /// The session `session_id`, which worked in `working_directory`, with no environment.
    #[must_use]
    pub fn new(session_id: impl Into<String>, working_directory: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            working_directory: working_directory.into(),
            environment: Secret::default(),
        }
    }

    /// The same query, with the variables its connection was started with.
    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = Secret::new(environment);
        self
    }
}

/// A session to seed, for [`AcpHarness::import_session`](crate::AcpHarness::import_session): at the
/// host's UUID, from a history, so a later connection resumes it by that id.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct SessionImport {
    /// The id the session is seeded at, created by the host: a UUID no session of the harness has.
    pub session_id: String,
    /// The directory the session will work in.
    pub working_directory: String,
    /// The history, as JSON lines in the standard session-history format, whose first line is a
    /// header carrying its version; empty seeds a session with no history.
    #[map(default)]
    pub history: String,
    /// The variables the session's connections will be started with.
    #[map(default)]
    pub environment: Secret<Environment>,
}

impl SessionImport {
    /// Seeds `session_id`, to work in `working_directory`, from `history`, with no environment.
    #[must_use]
    pub fn new(
        session_id: impl Into<String>,
        working_directory: impl Into<String>,
        history: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            working_directory: working_directory.into(),
            history: history.into(),
            environment: Secret::default(),
        }
    }

    /// The same import, with the variables the session's connections will be started with.
    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = Secret::new(environment);
        self
    }
}

/// One page of a session's history to export, for
/// [`AcpHarness::export_session`](crate::AcpHarness::export_session).
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct SessionExport {
    /// The session's id as the provider reported it: a UUID.
    pub session_id: String,
    /// The directory the session worked in.
    pub working_directory: String,
    /// The variables its connection was started with.
    #[map(default)]
    pub environment: Secret<Environment>,
    /// Where to go on from: the `next` of the page before, as the provider gave it; absent for the
    /// first page.
    pub after: Option<String>,
    /// The most bytes a page holds: the caller's memory bound, never exceeded. A next line longer
    /// than it, line feed included, is refused, saying how long it is (see
    /// [`LINE_TOO_LARGE`](crate::LINE_TOO_LARGE)).
    #[map(default = DEFAULT_PAGE_BYTES)]
    pub max_bytes: u64,
}

/// [`SessionExport::max_bytes`] when a host gives none: 1 MiB.
pub const DEFAULT_PAGE_BYTES: u64 = 1 << 20;

/// How the refusal of a page too small for its next line begins: `GUATIAO_ERR_BAD_VALUE` with the
/// message `line too large: <N> bytes`, N being the line's length with its line feed, so a host
/// can ask again with a `max_bytes` of at least N, or stop and say why.
pub const LINE_TOO_LARGE: &str = "line too large: ";

/// The size a [`LINE_TOO_LARGE`] refusal names, in bytes; `None` for any other message.
#[must_use]
pub fn line_too_large(message: &str) -> Option<u64> {
    message
        .strip_prefix(LINE_TOO_LARGE)?
        .split_once(" bytes")
        .and_then(|(size, _)| size.parse().ok())
}

impl SessionExport {
    /// The first page of the history of `session_id`, which worked in `working_directory`, at most
    /// [`DEFAULT_PAGE_BYTES`], with no environment.
    #[must_use]
    pub fn new(session_id: impl Into<String>, working_directory: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            working_directory: working_directory.into(),
            environment: Secret::default(),
            after: None,
            max_bytes: DEFAULT_PAGE_BYTES,
        }
    }

    /// The same export, with the variables its connection was started with.
    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = Secret::new(environment);
        self
    }

    /// The page after the one whose `next` was `after`.
    #[must_use]
    pub fn after(mut self, after: impl Into<String>) -> Self {
        self.after = Some(after.into());
        self
    }

    /// Pages of at most `bytes`.
    #[must_use]
    pub const fn max_bytes(mut self, bytes: u64) -> Self {
        self.max_bytes = bytes;
        self
    }
}

/// One page of an exported history, from
/// [`AcpHarness::export_session`](crate::AcpHarness::export_session).
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct ExportPage {
    /// Whole JSON lines, each ended by a line feed. Only the first page starts with the header.
    pub text: String,
    /// What to ask for the next page with ([`SessionExport::after`]); absent when the export is
    /// complete.
    pub next: Option<String>,
}

impl ExportPage {
    /// A page holding `text`, followed by the page `next` names, if any.
    #[must_use]
    pub fn new(text: impl Into<String>, next: Option<String>) -> Self {
        Self {
            text: text.into(),
            next,
        }
    }
}

/// What a seed was made from and what it left behind, from
/// [`AcpHarness::import_session`](crate::AcpHarness::import_session), for the host's record of how a
/// run started.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct ImportReport {
    /// Where in the history the seed starts (a compaction boundary, or the first line after the
    /// header); absent for an empty history.
    pub seeded_from: Option<SeedPoint>,
    /// What could not be carried into the session.
    #[map(default)]
    pub losses: Vec<HistoryLoss>,
    /// Whether the harness compacted the history to fit its window.
    #[map(default)]
    pub compacted: bool,
}

impl ImportReport {
    /// A seed with no history: nothing seeded from, nothing lost, nothing compacted.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// A position in a history.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct SeedPoint {
    /// Its line, counting the header as line 1.
    pub line: u64,
    /// The digest the history gives that line, when it gives one.
    pub digest: Option<String>,
}

impl SeedPoint {
    /// Line `line`, with no digest.
    #[must_use]
    pub const fn line(line: u64) -> Self {
        Self { line, digest: None }
    }
}

/// One kind of thing an import could not carry, as the standard format's loss report names it.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct HistoryLoss {
    /// What was lost, in the loss report's vocabulary: `thinking` (signed thinking or encrypted
    /// reasoning dropped), `tool_as_note` (a tool call with no equivalent, kept as a note),
    /// `attachment_by_digest` (an attachment carried by its digest only), `replayed_as_prompt` (the
    /// history given as context in a prompt rather than as the harness's own session).
    pub kind: String,
    /// How many times.
    pub count: u64,
    /// More, for people, when there is more to say.
    pub detail: Option<String>,
}

impl HistoryLoss {
    /// `count` losses of `kind`.
    #[must_use]
    pub fn new(kind: impl Into<String>, count: u64) -> Self {
        Self {
            kind: kind.into(),
            count,
            detail: None,
        }
    }
}

/// Where a harness keeps its own record of one session, and in what format.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct SessionRecordFile {
    /// The file's absolute path, where the harness's agent runs.
    pub path: String,
    /// What the file holds, as a short name a reader knows (`claude-code-jsonl`,
    /// `codex-rollout-jsonl`).
    pub format: String,
}

impl SessionRecordFile {
    /// The record at `path`, in `format`.
    #[must_use]
    pub fn new(path: impl Into<String>, format: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            format: format.into(),
        }
    }
}

/// A setting of a configured harness that differs from its default in a way its settings' schema
/// cannot show, as plain data for the host to show or log; never refused for it.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct NonDefault {
    /// The setting's name, as the provider spells it (`allowed_tools[2]`).
    pub setting: String,
    /// Its value, as text; never a credential.
    pub value: String,
    /// The default value, as text.
    pub default: String,
}

impl NonDefault {
    /// A non-default `setting`, now `value`, against `default`.
    #[must_use]
    pub fn new(
        setting: impl Into<String>,
        value: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        Self {
            setting: setting.into(),
            value: value.into(),
            default: default.into(),
        }
    }
}
