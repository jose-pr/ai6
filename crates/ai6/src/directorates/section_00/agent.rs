//! The Guatiao contract for launching and driving one agent Session.

use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, ToValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::directorates::m::identity::{MissionId, SessionId};
use crate::directorates::q::terminal::{AgentTerminal, HarnessStdio};

/// The first instruction for one agent operation in a Mission.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AgentStart {
    /// AI6's identifier for the unit of work; a Mission may have several Sessions.
    pub mission_id: MissionId,
    /// The configured Agent codename.
    pub agent_id: String,
    /// The task prompt sent to the agent.
    pub prompt: String,
    /// Context to establish before the task prompt is sent.
    pub initial_context: Vec<String>,
    /// The Operation checkout or Mission safe house.
    pub working_directory: String,
    /// The clearance selected by AI6; the handler maps it to harness policy.
    pub clearance: String,
    /// Environment values issued for this provider operation.
    pub environment: BTreeMap<String, String>,
    /// The requested model, when the Agent roster specifies one.
    pub model: Option<String>,
    /// The requested reasoning effort, when supported by the handler.
    pub effort: Option<String>,
    /// Spend, in millionths of a dollar, after which the harness should stop on its own, when
    /// it can; AI6 enforces its budgets itself regardless.
    pub max_cost_micro_usd: Option<u64>,
    /// Whether the harness may load the settings its working directory carries (hooks, MCP
    /// servers, permissions), as AI6 decided from the Operation's harness policy. `false`
    /// keeps only the user's own settings and those AI6 passes itself.
    #[serde(default)]
    pub project_settings: bool,
    /// Whether it may also connect the MCP servers the working directory's `.mcp.json`
    /// names; `false` keeps only the servers AI6 passes.
    #[serde(default)]
    pub project_mcp: bool,
    /// Whether the Session only reads its prompt and answers: the handler starts the harness
    /// with every tool it can switch off switched off, no MCP server, no settings of the
    /// working directory, and a sandbox that cannot write or reach the network where the
    /// harness has one. AI6 asks for it for a Chief of Staff review, which reads untrusted text.
    #[serde(default)]
    pub answer_only: bool,
    /// Set when the Session is Moneypenny, the operator's assistant: as [`answer_only`], no
    /// built-in tool, no settings of the working directory and no web, except for what the
    /// mode allows, which is only the MCP servers it names. Takes precedence over
    /// `answer_only`.
    ///
    /// [`answer_only`]: AgentStart::answer_only
    #[serde(default)]
    pub assistant: Option<AssistantMode>,
}

/// What an assistant Session may use besides answering: its parts, so a handler builds the
/// harness's settings from them rather than from one fixed set.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AssistantMode {
    /// The MCP servers the harness connects to and may call every tool of, and no other.
    pub mcp_servers: Vec<McpServerRef>,
    /// Interpreters the harness may run inside its own sandbox (`python3`); empty means no
    /// shell at all. A handler that cannot confine them refuses to start, unless
    /// `unsandboxed` lets them run without it.
    #[serde(default)]
    pub interpreters: Vec<String>,
    /// Run the interpreters without the harness's sandbox, where it has none: no file or
    /// network limit applies to them. A weakening the Station reports; `false` refuses
    /// interpreters a handler cannot confine.
    #[serde(default)]
    pub unsandboxed: bool,
    /// Skills the harness may load, by name, each installed in `skills_dir`; empty means none.
    /// A handler that cannot limit skills to a list refuses a non-empty one.
    #[serde(default)]
    pub skills: Vec<String>,
    /// The absolute directory the Station installed `skills` in, laid out as a plugin of the
    /// harness: `skills/<name>/SKILL.md` with each skill's files, and a manifest
    /// `.claude-plugin/plugin.json` naming the plugin `ai6`. Required with `skills`.
    #[serde(default)]
    pub skills_dir: Option<String>,
    /// Whether a skill's injected commands (`` !`command` ``) may run, through the harness's
    /// permission rules; `false` has the harness replace them with a notice.
    #[serde(default)]
    pub skill_commands: bool,
    /// What the harness's own web tools and its interpreters may reach besides the MCP
    /// servers, which are always reachable.
    #[serde(default)]
    pub network: AssistantNetwork,
    /// Directories, relative to the working directory (`personal`), under which the harness
    /// may create and change files; it may read anything in the working directory. Empty
    /// means no file tool at all. A handler that cannot confine its file tools to them
    /// refuses a non-empty list.
    #[serde(default)]
    pub files: Vec<String>,
    /// Paths, relative to the working directory (`shared`, `CLAUDE.md`), the interpreters may
    /// not write although the rest of the working directory is theirs to write.
    #[serde(default)]
    pub read_only: Vec<String>,
    /// A file outside the working directory whose text the harness gets as part of its system
    /// prompt: standing context too large for a command line. A handler that cannot pass a
    /// file refuses it.
    #[serde(default)]
    pub context_file: Option<String>,
}

/// The assistant's network policy. `limited` with no hosts, the default, reaches nothing
/// but its MCP servers.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AssistantNetwork {
    /// `limited` (only `allowed_hosts`), `denied` (nothing) or `open` (anything; a weakening).
    pub policy: String,
    /// Host names a `limited` policy reaches; ignored otherwise.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

impl Default for AssistantNetwork {
    fn default() -> Self {
        Self {
            policy: "limited".to_owned(),
            allowed_hosts: Vec::new(),
        }
    }
}

impl AssistantNetwork {
    /// Whether the policy reaches nothing besides the MCP servers: `denied`, or `limited`
    /// with no host.
    pub fn reaches_nothing(&self) -> bool {
        self.policy == "denied" || (self.policy == "limited" && self.allowed_hosts.is_empty())
    }

    /// Whether the policy reaches any host at all: `open`.
    pub fn open(&self) -> bool {
        self.policy == "open"
    }
}

/// An MCP server an assistant Session connects to, found in its environment: the URL in
/// `<env_prefix>_URL` and the bearer token in `<env_prefix>_TOKEN`, so the token never
/// appears in a command line.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct McpServerRef {
    /// The name the harness knows the server by (`moneypenny`); its tools are
    /// `mcp__<name>__<tool>` in Claude Code.
    pub name: String,
    /// The prefix of the two environment variables (`AI6_MONEYPENNY_MCP`).
    pub env_prefix: String,
}

/// How AI6 resumes a provider-issued Session.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AgentResume {
    /// The Mission this Session is working on.
    pub mission_id: MissionId,
    /// The provider-issued identity of the operation to resume.
    pub session_id: SessionId,
    /// A new message to submit after resuming, if any.
    pub message: Option<String>,
    /// The Mission checkout or safe house containing the saved Session.
    pub working_directory: String,
    /// Environment values issued for the resumed provider operation.
    pub environment: BTreeMap<String, String>,
    /// The requested model when the harness supports changing it on resume.
    pub model: Option<String>,
    /// The requested reasoning effort when the harness supports it on resume.
    pub effort: Option<String>,
    /// Spend, in millionths of a dollar, after which the harness should stop on its own, when
    /// it can; AI6 enforces its budgets itself regardless.
    pub max_cost_micro_usd: Option<u64>,
    /// Whether the harness may load the settings its working directory carries, decided
    /// again on every resume.
    #[serde(default)]
    pub project_settings: bool,
    /// Whether it may also connect the MCP servers the working directory's `.mcp.json`
    /// names, decided again on every resume.
    #[serde(default)]
    pub project_mcp: bool,
    /// Whether the Session only reads and answers, as [`AgentStart::answer_only`] started it.
    #[serde(default)]
    pub answer_only: bool,
    /// The assistant mode, as [`AgentStart::assistant`]; decided again on every resume.
    #[serde(default)]
    pub assistant: Option<AssistantMode>,
    /// The standing part of [`AgentStart::initial_context`] (the Agent's context, its role's
    /// purpose and policy, an assistant's instructions; never a recap of the conversation the
    /// Session already holds), for a harness that may not keep its start-up context in the
    /// Session it resumes. Claude Code gets it again as `--append-system-prompt`, which it
    /// uses once the recorded prompt is gone (a compacted Session); Codex keeps a thread's
    /// instructions and ignores it.
    #[serde(default)]
    pub initial_context: Vec<String>,
}

/// A message AI6 sends to an already running Agent Session.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AgentMessage {
    /// Message text, kept separate from the initial context.
    pub text: String,
}

/// The event kind a handler emits when the provider refuses further work until a quota
/// resets. The event carries the details in [`AgentEvent::limit`].
pub const LIMIT_EVENT: &str = "limit";

/// A provider usage or rate limit that stopped the Session.
///
/// A handler emits it as an [`AgentEvent`] of kind [`LIMIT_EVENT`] in place of the
/// `completed` or `failed` event the same output would otherwise produce.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[map(crate = ::guatiao)]
pub struct UsageLimit {
    /// When the provider says the quota resets, as Unix seconds. `None` when it did not say.
    pub resets_at: Option<i64>,
    /// Which window was exhausted (`five_hour`, `weekly`, ...) when the provider names it.
    pub window: Option<String>,
}

/// Tokens and cost a provider reported for a Session, as the provider reports them.
///
/// Money is an integer count of millionths of a dollar so records never carry floats.
#[derive(
    ToValue, FromValue, Schema, Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
#[map(crate = ::guatiao)]
pub struct TokenUsage {
    /// Prompt tokens that were not served from the cache.
    pub input: u64,
    /// Tokens the model generated.
    pub output: u64,
    /// Prompt tokens read from the provider's cache.
    pub cache_read: u64,
    /// Prompt tokens written to the provider's cache.
    pub cache_write: u64,
    /// Cost in millionths of a dollar; `0` when the provider reports no cost.
    pub cost_micro_usd: u64,
    /// Whether the figures are running totals since the provider process started (`true`)
    /// or only what the last step added (`false`).
    pub cumulative: bool,
}

/// How much of one provider quota window the Session's account has used, as the provider
/// reported it. A handler attaches one per window to the event that carried the report.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[map(crate = ::guatiao)]
pub struct QuotaReport {
    /// The window (`five_hour`, `weekly`, ...) as the provider names it, or its position
    /// (`primary`, `secondary`) when it gives no name.
    pub window: String,
    /// Thousandths of the window used, when the provider reports a figure.
    pub used_permille: Option<u16>,
    /// When the window resets, as Unix seconds, when the provider says.
    pub resets_at: Option<i64>,
    /// The provider's verdict for the window (`allowed`, `allowed_warning`, `rejected`), when
    /// it reports one.
    pub status: Option<String>,
}

/// One provider event returned to the daemon.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[map(crate = ::guatiao)]
pub struct AgentEvent {
    /// Provider-neutral event name such as `text`, `blocked`, `tool`, `completed` or `limit`.
    pub kind: String,
    /// Human-readable event text when the provider supplies it.
    pub text: Option<String>,
    /// Provider data that has no text representation.
    pub data: Option<Vec<u8>>,
    /// The limit that stopped the Session; set only on a `limit` event.
    pub limit: Option<UsageLimit>,
    /// Token usage and cost the provider reported with this event.
    pub usage: Option<TokenUsage>,
    /// Names of the tools the event calls, as the harness names them (`WebFetch`).
    pub tools: Vec<String>,
    /// Quota windows the provider reported with this event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quota: Vec<QuotaReport>,
}

/// Starts or resumes the configured Agent implementation.
#[guatiao::kind(name = "ai6_agent_handler")]
pub trait AgentHandler: Send + Sync {
    /// Starts an Agent Session with context established before its task prompt.
    ///
    /// `terminal` is an AI6-owned object bridge over its selected terminal provider; the
    /// handler selects its command and protocol while the provider owns process lifetime.
    fn start(
        &self,
        request: AgentStart,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError>;

    /// Reattaches to the same provider-issued Session ID after an interruption.
    fn resume(
        &self,
        request: AgentResume,
        terminal: Object<dyn AgentTerminal>,
    ) -> Result<Object<dyn AgentSession>, ProviderError>;

    /// How the harness talks over its standard input and output, which decides the terminal
    /// provider the Station gives it; the default is [`HarnessStdio::Terminal`]. A handler
    /// declares a requirement here, never a provider.
    fn stdio(&self) -> Result<HarnessStdio, ProviderError> {
        Ok(HarnessStdio::Terminal)
    }

    /// Whether the harness can run an assistant's interpreters inside its own sandbox on this
    /// Station, started with `environment` (as the Station would give it): `None` when it
    /// can, else why not, in a sentence. The default: it cannot.
    fn assistant_scripts(
        &self,
        environment: BTreeMap<String, String>,
    ) -> Result<Option<String>, ProviderError> {
        let _ = environment;
        Ok(Some(
            "this handler runs no interpreter for an assistant".to_owned(),
        ))
    }

    /// Whether the harness can load exactly an assistant's [`AssistantMode::skills`] and no
    /// other skill: `None` when it can, else why not, in a sentence; a Station installs no
    /// skill for a handler that cannot. The default: it cannot.
    fn assistant_skills(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(
            "this handler loads no skill for an assistant".to_owned(),
        ))
    }
}

/// One provider-owned, resumable agent operation.
#[guatiao::kind(object, name = "ai6_agent_session")]
pub trait AgentSession: Send + Sync {
    /// The provider-issued Session ID, once the provider has created it.
    fn session_id(&self) -> Result<Option<SessionId>, ProviderError>;

    /// Submits a message to the active operation.
    fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError>;

    /// Reads one event, or returns `None` when `timeout_ms` expires.
    ///
    /// A handler must report terminal process exit as a `completed` or `failed` event, the
    /// `failed` text saying how the harness ended and what it wrote last; returning `None`
    /// permanently after end-of-file leaves the daemon waiting forever. A terminal may keep its
    /// output open after the harness exits, so a handler asks the terminal's `exit_status` when
    /// a read times out.
    fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError>;

    /// Requests that the provider interrupt the operation.
    fn interrupt(&self) -> Result<(), ProviderError>;

    /// Closes the operation and releases provider resources. It returns once the harness has
    /// exited, or after a short wait when the terminal cannot tell, since the caller may remove
    /// the harness's working directory next.
    fn close(&self) -> Result<(), ProviderError>;
}
