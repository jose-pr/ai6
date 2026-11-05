//! The Guatiao contract for launching one agent Session. The Session a handler returns, and the
//! events it reports, are `coqui_acp_provider`'s agent session kind, shared with every host of
//! agent providers.

use coqui_acp_provider::AgentSession;
use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, ToValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::directorates::m::identity::{MissionId, SessionId};
use crate::directorates::q::terminal::{AgentTerminal, HarnessStdio};

/// The settings an `AgentHandler` provider is built with: an Agent's `handler_settings` as TOML
/// text. The provider validates them and refuses unknown keys. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct HandlerConfig {
    /// The settings table as TOML.
    pub settings_toml: String,
}

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
    /// The clearance selected by AI6; the handler maps it to harness policy. A clearance it does
    /// not know counts as `desk` ([`clearance_runs_commands`]).
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
    /// What the Session's overlays hand the harness beside their text and environment, which are
    /// in `initial_context` and `environment` already.
    #[map(default)]
    #[serde(default)]
    pub overlays: SessionOverlays,
}

/// A skill of an overlay issued to a Session.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct OverlaySkill {
    /// The skill's name, its directory under `skills/`.
    pub name: String,
    /// The overlay that issues it.
    pub overlay: String,
    /// What its front matter says it is for.
    pub description: String,
    /// The skill's directory on the harness's host, absolute and read-only; `SKILL.md` is in it.
    pub path: String,
    /// Whether a handler whose harness loads skills may do so. `false` when the overlay's harness
    /// hint turns native delivery off: the routing line is then the delivery.
    #[map(default)]
    #[serde(default)]
    pub native: bool,
}

/// What a Session's overlays hand its harness. Empty for a Session with none.
#[derive(
    ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default,
)]
#[map(crate = ::guatiao)]
pub struct SessionOverlays {
    /// Each overlay's root on the harness's host, absolute and read-only. A handler whose harness
    /// refuses to read outside its workspaces allows these.
    #[map(default)]
    #[serde(default)]
    pub roots: Vec<String>,
    /// The skills, each once, in composition order.
    #[map(default)]
    #[serde(default)]
    pub skills: Vec<OverlaySkill>,
    /// A read-only directory holding the skills whose `native` is set as a harness plugin:
    /// `skills/<name>/` and a manifest `.claude-plugin/plugin.json` naming the plugin `ai6`.
    #[serde(default)]
    pub plugin_dir: Option<String>,
}

impl SessionOverlays {
    /// The routing lines for the skills a handler does not load itself: those whose `native` is
    /// unset when `loads_skills`, else every one. A skill is a file an Agent can read, so a harness
    /// with no skills of its own is told where each one is. `None` when there is none to name.
    pub fn routing(&self, loads_skills: bool) -> Option<String> {
        let lines: Vec<String> = self
            .skills
            .iter()
            .filter(|skill| !(loads_skills && skill.native))
            .map(|skill| {
                format!(
                    "- Skill `{}` (overlay `{}`): {} Read {}/SKILL.md when the task calls for it.",
                    skill.name,
                    skill.overlay,
                    skill.description.trim(),
                    skill.path.trim_end_matches(['/', '\\'])
                )
            })
            .collect();
        (!lines.is_empty()).then(|| format!("Skills you can use:\n{}", lines.join("\n")))
    }

    /// `context` with the routing lines for the skills a handler does not load itself, after it.
    pub fn with_routing(&self, context: &[String], loads_skills: bool) -> Vec<String> {
        context
            .iter()
            .cloned()
            .chain(self.routing(loads_skills))
            .collect()
    }
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
    /// The Station's own directories (its state, identity and secrets directories) the
    /// interpreters may never read, besides those a handler denies itself.
    #[serde(default)]
    pub station_paths: Vec<String>,
}

/// The assistant's network policy. `limited` with no hosts, the default, reaches nothing
/// but its MCP servers.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct AssistantNetwork {
    /// `limited` (only `allowed_hosts`), `denied` (nothing) or `open` (everything the Station
    /// reaches, the Station itself included; a weakening).
    pub policy: String,
    /// Host names a `limited` policy reaches; ignored otherwise.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// What neither policy may reach, though `allowed_hosts` or `open` would: categories,
    /// networks, addresses and host names as [`DenyEntry`](super::denied::DenyEntry) reads
    /// them. Empty, the default, denies nothing. A handler that cannot keep a part of it away
    /// from the harness's tools refuses to start.
    #[serde(default)]
    pub denied: Vec<String>,
}

impl Default for AssistantNetwork {
    fn default() -> Self {
        Self {
            policy: "limited".to_owned(),
            allowed_hosts: Vec::new(),
            denied: Vec::new(),
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

/// Whether a Session at `clearance` may have a command tool: `field` and `00`. Anything else,
/// `desk` and a clearance this build does not know alike, may not, because a command reaches
/// whatever the harness's OS user can. A handler that cannot keep commands from a Session refuses
/// to start it.
pub fn clearance_runs_commands(clearance: &str) -> bool {
    crate::directorates::m::approvals::clearance_at_least(clearance, "field")
}

/// The clearance of a Mission's Session held to reading and searching until its plan passes a
/// review: no tool that changes anything but AI6's own `submit_plan`. Only AI6 sets it; an Agent
/// cannot be configured at it.
pub const READ_CLEARANCE: &str = "read";

/// Whether a Session at `clearance` is held to reading ([`READ_CLEARANCE`]).
pub fn clearance_reads_only(clearance: &str) -> bool {
    clearance == READ_CLEARANCE
}

/// The provider-issued Session ID a Session reported, as AI6 records it. One that is no valid
/// identity fails the Session, with this sentence.
#[cfg_attr(not(feature = "daemon"), allow(dead_code))]
pub(crate) fn checked_session_id(reported: Option<String>) -> Result<Option<SessionId>, String> {
    reported
        .map(|id| {
            SessionId::try_from(id).map_err(|error| {
                format!("the provider reported a Session ID AI6 cannot record: {error}")
            })
        })
        .transpose()
}

/// [`checked_session_id`] of what `session_id` answered; the provider's own error passes as it is.
#[cfg_attr(not(feature = "daemon"), allow(dead_code))]
pub(crate) fn provider_session_id(
    reported: Result<Option<String>, ProviderError>,
) -> Result<Option<SessionId>, ProviderError> {
    checked_session_id(reported?)
        .map_err(|why| ProviderError::new(guatiao::Status::GUATIAO_ERR_BAD_VALUE, &why))
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
    /// The clearance the Agent works at now, decided again on every resume, so that a Session
    /// resumed after a change gets what the new clearance allows. Absent counts as `desk`
    /// ([`clearance_runs_commands`]).
    #[serde(default)]
    pub clearance: String,
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
    /// What the Session's overlays hand the harness, delivered again for this launch.
    #[map(default)]
    #[serde(default)]
    pub overlays: SessionOverlays,
}

/// One model a harness says an account may use.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[map(crate = ::guatiao)]
pub struct ModelInfo {
    /// What a roster names it (`model = "..."`): the harness's own identifier or alias.
    pub id: String,
    /// What the harness shows a person, when it has a name apart from the identifier.
    pub name: Option<String>,
    /// The harness's one-line description, when it gives one.
    pub description: Option<String>,
}

/// What a handler can say about the models an account may use, with no model turn.
///
/// A handler that cannot list says so ([`ModelListing::unknown`]); it never guesses.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[map(crate = ::guatiao)]
pub struct ModelListing {
    /// Whether the harness listed them. `false`: unknown, and `models` is empty.
    pub known: bool,
    /// Why it could not, when `known` is `false`.
    pub unknown_because: Option<String>,
    /// The models, in the order the harness gave them.
    pub models: Vec<ModelInfo>,
}

impl ModelListing {
    /// A listing the harness could not give, with the reason.
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            known: false,
            unknown_because: Some(reason.into()),
            models: Vec::new(),
        }
    }

    /// The models a harness listed.
    pub fn of(models: Vec<ModelInfo>) -> Self {
        Self {
            known: true,
            unknown_because: None,
            models,
        }
    }
}

/// The Session whose harness record [`AgentHandler::session_record`] is asked for: its
/// provider-issued ID, and the working directory and environment its harness was started with.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct SessionRecordQuery {
    /// The provider-issued Session ID.
    pub session_id: String,
    /// The directory the harness ran in.
    pub working_directory: String,
    /// The environment the harness was started with.
    pub environment: BTreeMap<String, String>,
}

/// Where a harness keeps its own record of one Session, on the host it ran on, and its format.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct SessionRecordFile {
    /// The file's absolute path.
    pub path: String,
    /// What the file holds, as a short name the app knows (`claude-code-jsonl`,
    /// `codex-rollout-jsonl`, `antigravity-jsonl`).
    pub format: String,
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

    /// Each way this handler's settings are looser than its default, one line each, for the
    /// Station to report. The default: none.
    fn weakenings(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    /// Why this handler, with its settings, would refuse a Mission's Session at `clearance`, in a
    /// sentence; `None` when it holds such a Session to what the clearance promises. A route of
    /// an Agent that would be refused is passed over. The default: it can hold any.
    fn clearance_refusal(&self, clearance: String) -> Result<Option<String>, ProviderError> {
        let _ = clearance;
        Ok(None)
    }

    /// Why this handler cannot hold a Mission's Session to reading at [`READ_CLEARANCE`] and then
    /// give it the Mission's tools on a resume of the same Session, in a sentence; `None` when it
    /// can. The default: it cannot say it can, so a Mission whose plan is reviewed does not start
    /// on it.
    fn read_only_refusal(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(
            "its handler cannot hold a Session to reading and then give it the Mission's tools"
                .to_owned(),
        ))
    }

    /// The models the account behind `environment` (the variables a Session would start with)
    /// may use, if the harness can say without a model turn. The default: it cannot, and says
    /// so. A handler never puts a credential from `environment` in its answer.
    fn models(&self, environment: BTreeMap<String, String>) -> Result<ModelListing, ProviderError> {
        let _ = environment;
        Ok(ModelListing::unknown(
            "this handler has no way to list models",
        ))
    }

    /// Where the harness keeps its own record of the Session `query` names, when it keeps one
    /// and the file is there; the Station reads it as that Session's context. The default:
    /// none, and the Station keeps its event stream instead.
    fn session_record(
        &self,
        query: SessionRecordQuery,
    ) -> Result<Option<SessionRecordFile>, ProviderError> {
        let _ = query;
        Ok(None)
    }

    /// The program the harness is started as, a name looked up on the Session's `PATH` or a
    /// path, so a Station can say a harness cannot be found before it asks a terminal to start
    /// it. The default: `None`, and the Station does not look.
    fn program(&self) -> Result<Option<String>, ProviderError> {
        Ok(None)
    }

    /// Why the harness cannot be shown images, in a sentence; `None` when it reads a message in
    /// the [content form](super::content), and gives its images to the model with the text. The
    /// default: it cannot.
    fn image_input(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some("this handler gives its harness no image".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A changed required signature affects every built provider; the Session's table is the
    // shared kind's, whose header names the same value.
    #[test]
    fn floor_hashes_are_pinned() {
        assert_eq!(AgentHandlerVtable::FLOOR_HASH, 2369958161);
        assert_eq!(
            coqui_acp_provider::AgentSessionVtable::FLOOR_HASH,
            1414216972
        );
    }

    #[test]
    fn a_provider_session_id_ai6_cannot_record_fails_the_session() {
        assert_eq!(checked_session_id(None), Ok(None));
        assert_eq!(
            checked_session_id(Some("provider-1".to_owned())),
            Ok(Some(SessionId::try_new("provider-1").unwrap()))
        );
        for bad in [String::new(), "a\nb".to_owned(), "x".repeat(10_000)] {
            let refused = checked_session_id(Some(bad)).unwrap_err();
            assert!(refused.contains("cannot record"), "{refused}");
        }
        let refused = provider_session_id(Ok(Some(String::new()))).unwrap_err();
        assert!(refused.to_string().contains("cannot record"), "{refused}");
    }
}
