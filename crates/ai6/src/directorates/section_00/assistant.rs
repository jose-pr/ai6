//! The assistant Session contract between Universal Exports and a Station.
//!
//! Moneypenny, the operator's assistant, runs as an Agent Session on a Station through the
//! same AgentHandler providers as every Agent, started with [`AgentStart::assistant`]. Its
//! tools are not on the Station: Universal Exports serves them on an MCP endpoint of its own,
//! authenticated by a token bound to the person and the conversation, and every tool runs as
//! that person. The Station takes [`AssistantJob`]s from Universal Exports (one per message
//! the person sends), hands the harness the endpoint's URL and token in its environment, and
//! reports what the harness says as [`AssistantReport`]s. A job may instead carry a
//! [`MemoryOp`]: the person reading or changing her memory of them from the app, which the
//! Station makes in a clone of the person's repository without any harness.
//!
//! [`AgentStart::assistant`]: super::agent::AgentStart::assistant

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::agent::{AssistantMode, AssistantNetwork, McpServerRef};
use crate::directorates::m::assistant::{RepoKind, file_safe};

/// The prefix of the environment variables holding the assistant's MCP endpoint
/// (`AI6_MONEYPENNY_MCP_URL`, `AI6_MONEYPENNY_MCP_TOKEN`).
pub const ASSISTANT_MCP_ENV_PREFIX: &str = "AI6_MONEYPENNY_MCP";
/// The environment variable holding the URL of the assistant's MCP endpoint.
pub const ASSISTANT_MCP_URL_ENV: &str = "AI6_MONEYPENNY_MCP_URL";
/// The environment variable holding the bearer token of the assistant's MCP endpoint.
pub const ASSISTANT_MCP_TOKEN_ENV: &str = "AI6_MONEYPENNY_MCP_TOKEN";
/// The name the harness knows the endpoint by: Claude Code calls its tools
/// `mcp__moneypenny__<tool>`.
pub const ASSISTANT_MCP_SERVER: &str = "moneypenny";
/// The prefix of the environment variables holding the Station's Cable endpoint for an
/// assistant (`AI6_ASSISTANT_CABLE_MCP_URL`, `AI6_ASSISTANT_CABLE_MCP_TOKEN`).
pub const ASSISTANT_CABLE_MCP_ENV_PREFIX: &str = "AI6_ASSISTANT_CABLE_MCP";
/// The environment variable holding the URL of an assistant's Cable endpoint.
pub const ASSISTANT_CABLE_MCP_URL_ENV: &str = "AI6_ASSISTANT_CABLE_MCP_URL";
/// The environment variable holding the bearer token of an assistant's Cable endpoint.
pub const ASSISTANT_CABLE_MCP_TOKEN_ENV: &str = "AI6_ASSISTANT_CABLE_MCP_TOKEN";
/// The name the harness knows an assistant's Cable endpoint by (`mcp__cables__<tool>`).
pub const ASSISTANT_CABLE_MCP_SERVER: &str = "cables";
/// The most Operations a job grants Cable actions in.
pub const MAX_ASSISTANT_CABLE_GRANTS: usize = 64;

impl AssistantMode {
    /// The mode of a Moneypenny Session that has only her own tools: the Universal Exports
    /// endpoint, no interpreter and no skill.
    pub fn moneypenny() -> Self {
        Self {
            mcp_servers: vec![McpServerRef {
                name: ASSISTANT_MCP_SERVER.to_owned(),
                env_prefix: ASSISTANT_MCP_ENV_PREFIX.to_owned(),
            }],
            interpreters: Vec::new(),
            deny_read: Vec::new(),
            unsandboxed: false,
            skills: Vec::new(),
            network: AssistantNetwork::default(),
            files: Vec::new(),
            read_only: Vec::new(),
            context_file: None,
        }
    }

    /// This mode with the Station's Cable endpoint as a further server,
    /// [`ASSISTANT_CABLE_MCP_SERVER`], reached through [`ASSISTANT_CABLE_MCP_ENV_PREFIX`].
    #[must_use]
    pub fn with_cables(mut self) -> Self {
        self.mcp_servers.push(McpServerRef {
            name: ASSISTANT_CABLE_MCP_SERVER.to_owned(),
            env_prefix: ASSISTANT_CABLE_MCP_ENV_PREFIX.to_owned(),
        });
        self
    }

    /// Checks the parts: at least one server; server names of letters, digits, `-` and `_`;
    /// environment prefixes of capitals, digits and `_`; interpreter and skill names of
    /// letters, digits, `.`, `-` and `_`; a known network policy and hosts as [`host_pattern`]
    /// accepts them; writable directories of one to eight `/`-separated names of letters,
    /// digits, `.`, `-` and `_`, none starting with `.`; a context file and unreadable paths
    /// given as absolute paths. A handler checks this before building settings from the
    /// names.
    ///
    /// # Errors
    ///
    /// A message naming the first part out of bounds.
    pub fn validate(&self) -> Result<(), String> {
        let word = |value: &str, extra: &[char]| {
            !value.is_empty()
                && value.len() <= 64
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
        };
        if self.mcp_servers.is_empty() {
            return Err("an assistant needs at least one MCP server".to_owned());
        }
        for server in &self.mcp_servers {
            if !word(&server.name, &['-', '_']) {
                return Err(format!("MCP server name {:?} is not allowed", server.name));
            }
            if !word(&server.env_prefix, &['_'])
                || server.env_prefix.chars().any(|c| c.is_ascii_lowercase())
            {
                return Err(format!(
                    "MCP environment prefix {:?} is not allowed",
                    server.env_prefix
                ));
            }
        }
        if let Some(name) = self
            .interpreters
            .iter()
            .find(|name| !interpreter_name(name))
        {
            return Err(format!("{name:?} is not an allowed interpreter name"));
        }
        for name in &self.skills {
            if !word(name, &['.', '-', '_']) {
                return Err(format!(
                    "{name:?} is not an allowed interpreter or skill name"
                ));
            }
        }
        check_network(&self.network)?;
        for path in &self.deny_read {
            if path.chars().any(char::is_control) || !std::path::Path::new(path).is_absolute() {
                return Err(format!("{path:?} is not an absolute path"));
            }
        }
        for path in self.files.iter().chain(&self.read_only) {
            let names: Vec<&str> = path.split('/').collect();
            if names.len() > 8 || !names.iter().all(|name| file_safe(name)) {
                return Err(format!(
                    "{path:?} is not a path in the assistant's working directory"
                ));
            }
        }
        if let Some(file) = &self.context_file
            && (file.chars().any(char::is_control) || !std::path::Path::new(file).is_absolute())
        {
            return Err(format!("context file {file:?} is not an absolute path"));
        }
        Ok(())
    }
}

/// Whether `host` may name a host an assistant reaches: a lowercase DNS name of at least two
/// labels (1 to 63 letters, digits and `-`, not starting or ending with `-`), 253 bytes at
/// most, optionally `*.` before it for every name under it. A bare `*`, a wildcard elsewhere,
/// an address or a dotless name (which Claude Code's WebFetch refuses) is not one.
pub fn host_pattern(host: &str) -> bool {
    let name = host.strip_prefix("*.").unwrap_or(host);
    let labels: Vec<&str> = name.split('.').collect();
    name.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && !labels
            .last()
            .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether `name` may name an interpreter: 1 to 64 letters, digits, `.`, `-` and `_`, so it
/// is a program name and never a path or an argument.
pub fn interpreter_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with(['.', '-'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// Checks a network policy: `limited`, `denied` or `open`, with hosts as [`host_pattern`]
/// accepts them.
fn check_network(network: &AssistantNetwork) -> Result<(), String> {
    if !matches!(network.policy.as_str(), "limited" | "denied" | "open") {
        return Err(format!(
            "network policy {:?} is not limited, denied or open",
            network.policy
        ));
    }
    for host in &network.allowed_hosts {
        if !host_pattern(host) {
            return Err(format!("{host:?} is not a host name"));
        }
    }
    Ok(())
}

/// Whether a [`EVENT_TOOL`] event's `tool` brings text from outside AI6 into the conversation:
/// Claude Code's web tools always, and a script (`Bash`) when `scripts_reach_hosts`
/// ([`AssistantReach::scripts_reach_hosts`]). Text it brings in exposes the conversation.
pub fn outside_tool(tool: &str, scripts_reach_hosts: bool) -> bool {
    matches!(tool, "WebFetch" | "WebSearch") || (tool == "Bash" && scripts_reach_hosts)
}

/// How strict a network policy is: `denied` before `limited` before `open`; an unknown one
/// counts as `denied`.
fn policy_rank(policy: &str) -> u8 {
    match policy {
        "limited" => 1,
        "open" => 2,
        _ => 0,
    }
}

/// What a conversation's harness may run and reach besides its MCP servers, as Universal
/// Exports works it out for the person from the roster's `[assistant]` and its narrowing
/// tables. The default runs nothing and reaches nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantReach {
    /// The interpreters her scripts may run (`python3`); empty, she runs no script.
    pub interpreters: Vec<String>,
    /// What her web tools and scripts may reach.
    pub network: AssistantNetwork,
    /// Whether her scripts may run without a sandbox where the harness has none; a weakening.
    pub unsandboxed_scripts: bool,
}

impl AssistantReach {
    /// This reach, never wider than `cap`: the interpreters both name, the stricter network
    /// policy (`denied` < `limited` < `open`) with the hosts both reach, and unsandboxed
    /// scripts only when both allow them.
    #[must_use]
    pub fn capped_by(&self, cap: &AssistantReach) -> AssistantReach {
        let interpreters = self
            .interpreters
            .iter()
            .filter(|name| cap.interpreters.contains(name))
            .cloned()
            .collect();
        let (mine, theirs) = (&self.network, &cap.network);
        let rank = policy_rank(&mine.policy).min(policy_rank(&theirs.policy));
        let network = match rank {
            2 => AssistantNetwork {
                policy: "open".to_owned(),
                allowed_hosts: Vec::new(),
            },
            1 => {
                // An `open` side reaches every host the other lists.
                let hosts = match (mine.open(), theirs.open()) {
                    (true, _) => theirs.allowed_hosts.clone(),
                    (_, true) => mine.allowed_hosts.clone(),
                    _ => mine
                        .allowed_hosts
                        .iter()
                        .filter(|host| theirs.allowed_hosts.contains(host))
                        .cloned()
                        .collect(),
                };
                AssistantNetwork {
                    policy: "limited".to_owned(),
                    allowed_hosts: hosts,
                }
            }
            _ => AssistantNetwork {
                policy: "denied".to_owned(),
                allowed_hosts: Vec::new(),
            },
        };
        AssistantReach {
            interpreters,
            network,
            unsandboxed_scripts: self.unsandboxed_scripts && cap.unsandboxed_scripts,
        }
    }

    /// Whether her scripts may reach a host: a network policy that reaches one, or scripts let
    /// run without a sandbox, which no policy limits.
    pub fn scripts_reach_hosts(&self) -> bool {
        !self.network.reaches_nothing() || self.unsandboxed_scripts
    }

    /// Checks the reach: interpreters as [`interpreter_name`] accepts them, and a network as
    /// [`AssistantMode::validate`] wants it.
    ///
    /// # Errors
    ///
    /// A message naming the first part out of bounds.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(name) = self
            .interpreters
            .iter()
            .find(|name| !interpreter_name(name))
        {
            return Err(format!("{name:?} is not an interpreter name"));
        }
        check_network(&self.network)
    }
}

/// The endpoint's path on Universal Exports, relative to its origin.
pub const ASSISTANT_MCP_PATH: &str = "v1/moneypenny/mcp";
/// The longest message a job carries, in bytes.
pub const MAX_ASSISTANT_MESSAGE_BYTES: usize = 32 * 1024;
/// The most initial context a job carries, in bytes, all items together.
pub const MAX_ASSISTANT_CONTEXT_BYTES: usize = 24 * 1024;
/// The longest text one reported event carries, in bytes; longer text is cut.
pub const MAX_ASSISTANT_EVENT_BYTES: usize = 64 * 1024;
/// The most events one report carries.
pub const MAX_ASSISTANT_EVENTS: usize = 64;

/// A Station took the job: the harness has the message. `text` is unset.
pub const EVENT_ACCEPTED: &str = "accepted";
/// A Station refused the job; `text` says why, in AI6's words. Universal Exports may offer
/// the job to another Station.
pub const EVENT_REJECTED: &str = "rejected";
/// Text the assistant wrote.
pub const EVENT_TEXT: &str = "text";
/// The assistant called tools, named in [`AssistantEvent::tools`].
pub const EVENT_TOOL: &str = "tool";
/// The assistant finished answering the message; `text` is its final answer when the
/// harness reports one.
pub const EVENT_COMPLETED: &str = "completed";
/// The harness stopped before finishing; `text` says why when it can.
pub const EVENT_FAILED: &str = "failed";
/// The model's provider refused more work until a quota resets.
pub const EVENT_LIMIT: &str = "limit";
/// The assistant stopped answering because the person interrupted it; the harness stays up
/// for the next message.
pub const EVENT_INTERRUPTED: &str = "interrupted";
/// The Station closed the conversation's harness (idle, closed by request, or stopping). The
/// provider Session can be resumed by a later job.
pub const EVENT_CLOSED: &str = "closed";
/// The assistant changed her memory (`memory/` of the personal repository) in a turn: the
/// change was set aside, `text` is its unified diff and [`AssistantEvent::change`] the files.
/// Universal Exports answers with an [`AssistantCommit`] of [`COMMIT_MEMORY`], or a proposal
/// first.
pub const EVENT_MEMORY_CHANGES: &str = "memory_changes";
/// Something the person and the assistant should know about her workspace, in `text`: work
/// saved to a branch of its own because `main` refused it, or a repository that could not be
/// opened.
pub const EVENT_NOTICE: &str = "notice";
/// The assistant asked to send a Cable while exposed: nothing was sent, and
/// [`AssistantEvent::cable`] is what waits for the person's confirmation.
pub const EVENT_CABLE_PROPOSAL: &str = "cable_proposal";
/// A Cable left on the person's behalf; [`AssistantEvent::cable`] says which.
pub const EVENT_CABLE_SENT: &str = "cable_sent";

/// An [`AssistantCommit`] to the personal repository's `memory/`.
pub const COMMIT_MEMORY: &str = "memory";
/// An [`AssistantCommit`] to a shared repository.
pub const COMMIT_SHARED: &str = "shared";
/// The most bytes of file content one memory change or commit carries.
pub const MAX_ASSISTANT_CHANGE_BYTES: usize = 64 * 1024;
/// The most files one memory change or commit carries.
pub const MAX_ASSISTANT_CHANGE_FILES: usize = 32;
/// The most files a [`MemoryAction::List`] answer names, within [`MAX_MEMORY_LIST_BYTES`] of
/// paths; [`MemoryAnswer::more`] says there are others.
pub const MAX_MEMORY_FILES: usize = 1_000;
/// The most bytes of paths a [`MemoryAction::List`] answer names, so the report that carries
/// it stays well within what Universal Exports takes.
pub const MAX_MEMORY_LIST_BYTES: usize = 128 * 1024;

/// Where a person's conversation with the assistant keeps its work, sent with each message
/// when the deployment gives people personal repositories (`[assistant] personal_repo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantWorkspace {
    /// The person's personal repository: her working directory and memory for them.
    pub personal: String,
    /// The shared repositories the person reads, by the name each is checked out under (a
    /// directory group, or `org`).
    #[serde(default)]
    pub shared: BTreeMap<String, String>,
    /// The name commits are authored with: the person's.
    pub author_name: String,
    /// The email commits are authored with: the person's.
    pub author_email: String,
}

/// A memory change the desk set aside, or the files of a commit: each path, relative to the
/// repository, with its new text, or `None` to remove it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantChange {
    /// The SHA-256 of the files and their bases, as [`AssistantChange::digest_over`] makes
    /// it, in hex.
    pub digest: String,
    /// The files.
    pub files: BTreeMap<String, Option<String>>,
    /// What each file held when the change was made from it, for the commit to refuse a
    /// file changed since: the hex SHA-256 of its text, or `None` when it did not exist. A
    /// file not named here is written whatever it holds.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub base: BTreeMap<String, Option<String>>,
}

impl AssistantChange {
    /// The change of `files`, with its digest, whatever they hold now.
    pub fn new(files: BTreeMap<String, Option<String>>) -> Self {
        Self::based(files, BTreeMap::new())
    }

    /// The change of `files` made from `base` ([`AssistantChange::base`]), with its digest.
    pub fn based(
        files: BTreeMap<String, Option<String>>,
        base: BTreeMap<String, Option<String>>,
    ) -> Self {
        Self {
            digest: Self::digest_over(&files, &base),
            files,
            base,
        }
    }

    /// The SHA-256, in hex, over each path and its text (or removal), in path order.
    pub fn digest_of(files: &BTreeMap<String, Option<String>>) -> String {
        Self::digest_over(files, &BTreeMap::new())
    }

    /// [`AssistantChange::digest_of`], then each base, in path order, when there are any.
    pub fn digest_over(
        files: &BTreeMap<String, Option<String>>,
        base: &BTreeMap<String, Option<String>>,
    ) -> String {
        let mut hasher = Sha256::new();
        let mut entry = |path: &str, text: &Option<String>| {
            hasher.update((path.len() as u64).to_be_bytes());
            hasher.update(path.as_bytes());
            match text {
                Some(text) => {
                    hasher.update([1]);
                    hasher.update((text.len() as u64).to_be_bytes());
                    hasher.update(text.as_bytes());
                }
                None => hasher.update([0]),
            }
        };
        for (path, text) in files {
            entry(path, text);
        }
        if !base.is_empty() {
            entry("base", &None);
            for (path, digest) in base {
                entry(path, digest);
            }
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Checks the change: at most [`MAX_ASSISTANT_CHANGE_FILES`] files and
    /// [`MAX_ASSISTANT_CHANGE_BYTES`] of text, each path one to eight `/`-separated names of
    /// letters, digits, `.`, `-` and `_` not starting with `.`, under `memory/` when `memory`,
    /// and the digest matching.
    ///
    /// # Errors
    ///
    /// A message naming what is out of bounds.
    pub fn validate(&self, memory: bool) -> Result<(), String> {
        if self.files.is_empty() || self.files.len() > MAX_ASSISTANT_CHANGE_FILES {
            return Err(format!(
                "a change has 1 to {MAX_ASSISTANT_CHANGE_FILES} files"
            ));
        }
        let bytes: usize = self.files.values().flatten().map(String::len).sum();
        if bytes > MAX_ASSISTANT_CHANGE_BYTES {
            return Err(format!(
                "a change holds at most {MAX_ASSISTANT_CHANGE_BYTES} bytes of text"
            ));
        }
        for path in self.files.keys() {
            if !repository_path(path) || (memory && !path.starts_with("memory/")) {
                return Err(format!("{path:?} is not a file this change may write"));
            }
        }
        for (path, digest) in &self.base {
            let hex = |digest: &String| {
                digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
            };
            if !self.files.contains_key(path) || !digest.as_ref().is_none_or(hex) {
                return Err(format!("the base of {path:?} is not one of its files'"));
            }
        }
        if Self::digest_over(&self.files, &self.base) != self.digest {
            return Err("the change's digest does not match its files".to_owned());
        }
        Ok(())
    }
}

/// Whether `path` is a file path inside a repository that names no hidden or parent
/// directory: one to eight `/`-separated names of letters, digits, `.`, `-` and `_`, none
/// starting with `.`.
pub fn repository_path(path: &str) -> bool {
    let names: Vec<&str> = path.split('/').collect();
    names.len() <= 8 && names.iter().all(|name| file_safe(name))
}

/// A commit Universal Exports asks a Station to make for a person: a memory change she made
/// and the person let through, or a change to a shared repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantCommit {
    /// [`COMMIT_MEMORY`] or [`COMMIT_SHARED`].
    pub kind: String,
    /// The repository: the person's personal repository, or a shared one.
    pub repo: String,
    /// What to write.
    pub change: AssistantChange,
    /// The commit message, its trailer included.
    pub message: String,
    /// The author name: the person's.
    pub author_name: String,
    /// The author email: the person's.
    pub author_email: String,
}

impl AssistantCommit {
    /// Checks the commit: a known kind, a repository of a known kind, the change as
    /// [`AssistantChange::validate`] checks it, a message of 1 to 4,096 bytes and an author
    /// without control characters or `<`, `>`.
    ///
    /// # Errors
    ///
    /// A message naming what is out of bounds.
    pub fn validate(&self) -> Result<(), String> {
        let memory = match self.kind.as_str() {
            COMMIT_MEMORY => true,
            COMMIT_SHARED => false,
            other => return Err(format!("{other:?} is not a kind of commit")),
        };
        RepoKind::of(&self.repo)?;
        self.change.validate(memory)?;
        if self.message.trim().is_empty() || self.message.len() > 4_096 {
            return Err("a commit message is 1 to 4096 bytes".to_owned());
        }
        author(&self.author_name, &self.author_email)
    }
}

/// Checks a commit author: 1 to 256 bytes each, no control characters, `<` or `>`.
fn author(name: &str, email: &str) -> Result<(), String> {
    for value in [name, email] {
        if !valid_identity(value) || value.contains(['<', '>']) {
            return Err(format!("{value:?} cannot author a commit"));
        }
    }
    Ok(())
}

/// A look at, or a change to, the person's memory (`memory/` of their personal repository)
/// that Universal Exports asks a Station to make for the person, from the app. The Station
/// works in a clone of its own, which goes afterwards, and answers with a [`MemoryAnswer`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryOp {
    /// The person's personal repository; the Station takes only the one its own roster gives
    /// the job's `person_id`.
    pub repo: String,
    /// What to do.
    pub action: MemoryAction,
}

/// What a [`MemoryOp`] does. Every path is relative to the repository and under `memory/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryAction {
    /// Lists the files under `memory/` with their sizes.
    List {},
    /// Reads one file's text.
    Read {
        /// The file.
        path: String,
    },
    /// Writes one file and commits it to `main`.
    Write {
        /// The file.
        path: String,
        /// Its new text.
        text: String,
        /// The hex SHA-256 of what the file must hold on `main` for the write to go ahead, as
        /// [`MemoryAnswer::digest`] of a read gave it; `None` writes whatever it holds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
        /// The commit to make.
        commit: MemoryCommit,
    },
    /// Removes one file and commits the removal to `main`: "forget".
    Forget {
        /// The file.
        path: String,
        /// The commit to make.
        commit: MemoryCommit,
    },
}

/// The commit a [`MemoryAction::Write`] or [`MemoryAction::Forget`] makes: authored and
/// committed by the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryCommit {
    /// The commit message, its trailer included.
    pub message: String,
    /// The author name: the person's.
    pub author_name: String,
    /// The author email: the person's.
    pub author_email: String,
}

/// A Station's answer to a [`MemoryOp`], on the job's `accepted` or `rejected` event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryAnswer {
    /// The commit `main` was read at, or the one a change made; `None` for a repository with
    /// no commit yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The files a listing found, by path relative to the repository, in path order; at most
    /// [`MAX_MEMORY_FILES`] and [`MAX_MEMORY_LIST_BYTES`] of paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<MemoryFile>,
    /// The listing found more files than it names.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
    /// The text a read found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The hex SHA-256 of the text read or written: the `base` of a later write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Refused: the file is not there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub missing: bool,
    /// Refused: the file no longer holds the write's `base`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub changed: bool,
}

/// One file of a memory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryFile {
    /// The path, relative to the repository.
    pub path: String,
    /// Its size in bytes.
    pub bytes: u64,
}

/// Whether `path` names a file of the person's memory: a [`repository_path`] under `memory/`.
pub fn memory_path(path: &str) -> bool {
    path.strip_prefix("memory/")
        .is_some_and(|rest| !rest.is_empty() && repository_path(path))
}

impl MemoryOp {
    /// Checks the operation: a repository of a known kind, paths as [`memory_path`] wants
    /// them, text within [`MAX_ASSISTANT_CHANGE_BYTES`], a base of 64 hex digits, and a commit
    /// as [`AssistantCommit::validate`] wants its message and author.
    ///
    /// # Errors
    ///
    /// A message naming what is out of bounds.
    pub fn validate(&self) -> Result<(), String> {
        RepoKind::of(&self.repo)?;
        let (path, commit) = match &self.action {
            MemoryAction::List {} => return Ok(()),
            MemoryAction::Read { path } => (path, None),
            MemoryAction::Write {
                path,
                text,
                base,
                commit,
            } => {
                if text.len() > MAX_ASSISTANT_CHANGE_BYTES {
                    return Err(format!(
                        "a memory file holds at most {MAX_ASSISTANT_CHANGE_BYTES} bytes of text"
                    ));
                }
                if base.as_ref().is_some_and(|base| {
                    base.len() != 64 || !base.bytes().all(|b| b.is_ascii_hexdigit())
                }) {
                    return Err("a write's base is not a SHA-256 in hex".to_owned());
                }
                (path, Some(commit))
            }
            MemoryAction::Forget { path, commit } => (path, Some(commit)),
        };
        if !memory_path(path) {
            return Err(format!("{path:?} is not a file of the person's memory"));
        }
        if let Some(commit) = commit {
            if commit.message.trim().is_empty() || commit.message.len() > 4_096 {
                return Err("a commit message is 1 to 4096 bytes".to_owned());
            }
            author(&commit.author_name, &commit.author_email)?;
        }
        Ok(())
    }
}

impl AssistantWorkspace {
    /// Checks the workspace: repositories of a known kind, shared names as directory names,
    /// and an author as [`AssistantCommit::validate`] wants one.
    ///
    /// # Errors
    ///
    /// A message naming what is out of bounds.
    pub fn validate(&self) -> Result<(), String> {
        RepoKind::of(&self.personal)?;
        for (name, repo) in &self.shared {
            if !file_safe(name) {
                return Err(format!("{name:?} cannot name a shared checkout"));
            }
            RepoKind::of(repo)?;
        }
        author(&self.author_name, &self.author_email)
    }
}

/// One message for the assistant of one conversation, queued for a Station.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantJob {
    /// Universal Exports' identity for the job.
    pub job_id: String,
    /// The conversation the message belongs to; the Station keeps one harness per
    /// conversation.
    pub conversation_id: String,
    /// The roster Agent that is the assistant; its handler, model, effort and environment
    /// apply as for any Agent.
    pub agent_id: String,
    /// What the person wrote, or what the app reports on the person's behalf.
    pub message: String,
    /// The assistant's standing context (her instructions, the field dictionary): given to a
    /// harness started for this job before the message, and to a resumed one again
    /// ([`AgentResume::initial_context`]); unused when the conversation's harness is running.
    ///
    /// [`AgentResume::initial_context`]: crate::directorates::section_00::agent::AgentResume::initial_context
    #[serde(default)]
    pub context: Vec<String>,
    /// A recap of the conversation so far, for a harness that starts afresh; a resumed one
    /// has the conversation already and is not given it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recap: Option<String>,
    /// The bearer token of the conversation's MCP endpoint, bound to the person and the
    /// conversation. Never logged.
    pub mcp_token: String,
    /// When `mcp_token` stops being accepted, as Unix seconds. A harness started with an older
    /// token is restarted (resumed) with this one before the token it has expires.
    pub token_expires_at: f64,
    /// The provider Session the conversation last ran, which a new harness resumes.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Close the conversation's harness instead of sending a message; `message` is empty.
    #[serde(default)]
    pub close: bool,
    /// Stop the answer the conversation's harness is giving instead of sending a message;
    /// `message` is empty. A harness that is not answering is left as it is.
    #[serde(default)]
    pub interrupt: bool,
    /// The conversation has read text written outside AI6's own records, as Universal Exports
    /// knows it when the job is queued (`Conversation.exposed` there is the one record of it):
    /// a memory change then waits for the person, and so does whatever else waits on exposure.
    #[serde(default)]
    pub exposed: bool,
    /// The person the conversation is for. Required with `workspace` and `commit`: the
    /// Station fills the person's repository from its own roster and takes only that one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<String>,
    /// Where the conversation keeps its work; `None` keeps nothing, in an empty directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<AssistantWorkspace>,
    /// Make this commit instead of sending a message; `message` is empty. The Station answers
    /// `accepted` with where the commit landed, or `rejected` with why it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<AssistantCommit>,
    /// The Operations where the person may send or read Cables, at most
    /// [`MAX_ASSISTANT_CABLE_GRANTS`]; empty, the harness has no Cable endpoint. Requires
    /// `person_id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cables: Vec<AssistantCableGrant>,
    /// Send the Cable held for the person's confirmation instead of a message; `message` is
    /// empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cable_confirm: Option<AssistantCableDecision>,
    /// Drop the Cable held for the person's confirmation; `message` is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cable_discard: Option<AssistantCableDecision>,
    /// Look at or change the person's memory instead of sending a message; `message` is
    /// empty, and no harness is involved. The Station answers with a [`MemoryAnswer`] on its
    /// `accepted` or `rejected` event. Requires `person_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryOp>,
    /// What the harness may run and reach for this person; the Station caps it by its own
    /// roster's `[assistant]` and never widens it. The default runs and reaches nothing.
    #[serde(default)]
    pub reach: AssistantReach,
}

/// What a person may do with Cables in one Operation, through the assistant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantCableGrant {
    /// The Operation.
    pub operation: String,
    /// The person holds `cable.send` there.
    pub send: bool,
    /// The person holds `cable.read` there.
    pub read: bool,
}

/// The person's answer to a Cable the assistant was holding for confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantCableDecision {
    /// The desk's ID for the held Cable, as its [`EVENT_CABLE_PROPOSAL`] reported it.
    pub id: String,
    /// The digest that event reported; the desk sends only the Cable it names.
    pub digest: String,
}

/// A Cable an assistant proposed or sent on the person's behalf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantCable {
    /// The desk's ID for a held Cable; for one sent at once, the relay's Cable ID.
    pub id: String,
    /// The Operation it is sent in.
    pub operation: String,
    /// Its audience, as `<scope>:<name>` (`agent:007`, `station:hq`, `global`), or
    /// `reply:<cable id>` for a reply.
    pub target: String,
    /// Who it goes to: the audience of a send, `agent:<sender>` of the Cable a reply answers.
    pub recipient: String,
    /// The lifetime asked for, in seconds; `None` is the relay's default (15 minutes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u32>,
    /// The body, for a proposal; a sent Cable's event does not repeat it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// [`AssistantCable::request_digest`] of the request, held or sent.
    pub digest: String,
    /// The relay's Cable ID, once sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cable_id: Option<String>,
}

impl AssistantJob {
    /// Checks the job's bounds: identities of 1 to 256 bytes without control characters, a
    /// message within [`MAX_ASSISTANT_MESSAGE_BYTES`] (empty only to close, interrupt, commit,
    /// confirm or discard a held Cable, or look at or change the person's memory, of which a
    /// job does one), context within [`MAX_ASSISTANT_CONTEXT_BYTES`], a token of 32 to 512
    /// bytes, a workspace, commit and memory operation as their own `validate` checks them,
    /// and Cable grants for at most [`MAX_ASSISTANT_CABLE_GRANTS`] distinct Operations, with
    /// the person they are for.
    ///
    /// # Errors
    ///
    /// A message naming the first field out of bounds.
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("job_id", self.job_id.as_str()),
            ("conversation_id", self.conversation_id.as_str()),
            ("agent_id", self.agent_id.as_str()),
        ] {
            if !valid_identity(value) {
                return Err(format!(
                    "{name} is empty, too long or has control characters"
                ));
            }
        }
        if let Some(session) = &self.session_id
            && !valid_identity(session)
        {
            return Err("session_id is empty, too long or has control characters".to_owned());
        }
        let controls = [
            self.close,
            self.interrupt,
            self.commit.is_some(),
            self.cable_confirm.is_some(),
            self.cable_discard.is_some(),
            self.memory.is_some(),
        ];
        if controls.iter().filter(|set| **set).count() > 1 {
            return Err(
                "a job closes, interrupts, commits, confirms or discards a Cable, or looks at \
                 or changes the person's memory, only one"
                    .to_owned(),
            );
        }
        let control = controls.contains(&true);
        if self.message.len() > MAX_ASSISTANT_MESSAGE_BYTES
            || (self.message.trim().is_empty() && !control)
        {
            return Err("message is empty or too long".to_owned());
        }
        if let Some(person) = &self.person_id
            && !valid_identity(person)
        {
            return Err("person_id is empty, too long or has control characters".to_owned());
        }
        if (self.workspace.is_some()
            || self.commit.is_some()
            || self.memory.is_some()
            || !self.cables.is_empty())
            && self.person_id.is_none()
        {
            return Err(
                "a workspace, a commit, a memory operation or Cable grants need person_id"
                    .to_owned(),
            );
        }
        for decision in self.cable_confirm.iter().chain(&self.cable_discard) {
            if !valid_identity(&decision.id) || !valid_identity(&decision.digest) {
                return Err("a Cable decision's id or digest is not valid".to_owned());
            }
        }
        if self.cables.len() > MAX_ASSISTANT_CABLE_GRANTS {
            return Err(format!(
                "Cable grants name more than {MAX_ASSISTANT_CABLE_GRANTS} Operations"
            ));
        }
        let mut operations = std::collections::BTreeSet::new();
        for grant in &self.cables {
            if !valid_identity(&grant.operation) || !operations.insert(grant.operation.as_str()) {
                return Err("a Cable grant's Operation is not valid or repeated".to_owned());
            }
        }
        if let Some(workspace) = &self.workspace {
            workspace
                .validate()
                .map_err(|reason| format!("workspace: {reason}"))?;
        }
        if let Some(commit) = &self.commit {
            commit
                .validate()
                .map_err(|reason| format!("commit: {reason}"))?;
        }
        if let Some(memory) = &self.memory {
            memory
                .validate()
                .map_err(|reason| format!("memory: {reason}"))?;
        }
        self.reach
            .validate()
            .map_err(|reason| format!("reach: {reason}"))?;
        let recap = self.recap.as_ref().map_or(0, String::len);
        if self.context.iter().map(String::len).sum::<usize>() + recap > MAX_ASSISTANT_CONTEXT_BYTES
        {
            return Err("context is too long".to_owned());
        }
        if !(32..=512).contains(&self.mcp_token.len())
            || self
                .mcp_token
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err("mcp_token is not a valid token".to_owned());
        }
        if !self.token_expires_at.is_finite() || self.token_expires_at <= 0.0 {
            return Err("token_expires_at is not a time".to_owned());
        }
        Ok(())
    }
}

impl fmt::Debug for AssistantJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AssistantJob")
            .field("job_id", &self.job_id)
            .field("conversation_id", &self.conversation_id)
            .field("agent_id", &self.agent_id)
            .field("message_bytes", &self.message.len())
            .field("context_items", &self.context.len())
            .field("recap_bytes", &self.recap.as_ref().map(String::len))
            .field("mcp_token", &"[REDACTED]")
            .field("token_expires_at", &self.token_expires_at)
            .field("session_id", &self.session_id)
            .field("close", &self.close)
            .field("interrupt", &self.interrupt)
            .field("person_id", &self.person_id)
            .field("workspace", &self.workspace.is_some())
            .field(
                "commit",
                &self.commit.as_ref().map(|commit| commit.kind.as_str()),
            )
            .field("cables", &self.cables)
            .field("cable_confirm", &self.cable_confirm)
            .field("cable_discard", &self.cable_discard)
            .field(
                "memory",
                &self.memory.as_ref().map(|memory| match &memory.action {
                    MemoryAction::List {} => "list",
                    MemoryAction::Read { .. } => "read",
                    MemoryAction::Write { .. } => "write",
                    MemoryAction::Forget { .. } => "forget",
                }),
            )
            .field("reach", &self.reach)
            .finish()
    }
}

impl AssistantCable {
    /// SHA-256, lowercase hex, of the Operation, target, recipient, lifetime and body, each
    /// prefixed with its length in bytes, so a Station and Universal Exports agree on what the
    /// person confirms whatever their JSON settings.
    pub fn request_digest(&self) -> String {
        use sha2::{Digest, Sha256};

        let ttl = self
            .ttl_seconds
            .map(|ttl| ttl.to_string())
            .unwrap_or_default();
        let mut hasher = Sha256::new();
        for part in [
            self.operation.as_str(),
            self.target.as_str(),
            self.recipient.as_str(),
            ttl.as_str(),
            self.body.as_deref().unwrap_or_default(),
        ] {
            hasher.update(format!("{}:", part.len()).as_bytes());
            hasher.update(part.as_bytes());
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// What a Station reports about one conversation's harness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantReport {
    /// The reporting Station; Stations share one credential, so this is its own word.
    pub station_id: String,
    /// The conversation the events belong to.
    pub conversation_id: String,
    /// The job the events answer, when they answer one.
    #[serde(default)]
    pub job_id: Option<String>,
    /// The provider Session the harness runs, once the provider has issued it.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The events, oldest first; at most [`MAX_ASSISTANT_EVENTS`].
    pub events: Vec<AssistantEvent>,
}

/// One thing the assistant's harness did, in provider-neutral terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantEvent {
    /// One of the `EVENT_*` kinds; a Station may report others, which Universal Exports
    /// ignores.
    pub kind: String,
    /// The event's text, at most [`MAX_ASSISTANT_EVENT_BYTES`].
    #[serde(default)]
    pub text: Option<String>,
    /// The tools a [`EVENT_TOOL`] event called, as the harness names them.
    #[serde(default)]
    pub tools: Vec<String>,
    /// The memory change an [`EVENT_MEMORY_CHANGES`] event set aside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<AssistantChange>,
    /// The Cable of an [`EVENT_CABLE_PROPOSAL`] or [`EVENT_CABLE_SENT`] event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cable: Option<AssistantCable>,
    /// The answer to a memory job ([`AssistantJob::memory`]), on its `accepted` or `rejected`
    /// event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryAnswer>,
}

impl AssistantEvent {
    /// An event of `kind` with `text`, cut to [`MAX_ASSISTANT_EVENT_BYTES`] on a character
    /// boundary.
    pub fn new(kind: &str, text: Option<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            text: text.map(|text| cut(text, MAX_ASSISTANT_EVENT_BYTES)),
            tools: Vec::new(),
            change: None,
            cable: None,
            memory: None,
        }
    }
}

/// Whether `value` is 1 to 256 bytes without control characters.
pub fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn cut(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> AssistantJob {
        AssistantJob {
            job_id: "job-1".to_owned(),
            conversation_id: "conversation-1".to_owned(),
            agent_id: "moneypenny".to_owned(),
            message: "Which Stations are busy?".to_owned(),
            context: vec!["You are Moneypenny.".to_owned()],
            recap: None,
            mcp_token: "t".repeat(43),
            token_expires_at: 1_800_000_000.0,
            session_id: None,
            close: false,
            interrupt: false,
            exposed: false,
            person_id: None,
            workspace: None,
            commit: None,
            cables: Vec::new(),
            cable_confirm: None,
            cable_discard: None,
            memory: None,
            reach: AssistantReach::default(),
        }
    }

    fn memory_commit() -> MemoryCommit {
        MemoryCommit {
            message: "Edit memory/prefs.md\n\nVia: Universal Exports".to_owned(),
            author_name: "Eve".to_owned(),
            author_email: "eve@example.invalid".to_owned(),
        }
    }

    fn reach(interpreters: &[&str], policy: &str, hosts: &[&str]) -> AssistantReach {
        AssistantReach {
            interpreters: interpreters.iter().map(|name| (*name).to_owned()).collect(),
            network: AssistantNetwork {
                policy: policy.to_owned(),
                allowed_hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
            },
            unsandboxed_scripts: false,
        }
    }

    #[test]
    fn a_memory_job_names_its_person_and_one_file_under_memory() {
        let op = |action: MemoryAction| AssistantJob {
            message: String::new(),
            person_id: Some("eve".to_owned()),
            memory: Some(MemoryOp {
                repo: "/srv/mp/eve.git".to_owned(),
                action,
            }),
            ..job()
        };
        let write = |path: &str| MemoryAction::Write {
            path: path.to_owned(),
            text: "tables".to_owned(),
            base: None,
            commit: memory_commit(),
        };
        for action in [
            MemoryAction::List {},
            MemoryAction::Read {
                path: "memory/MEMORY.md".to_owned(),
            },
            write("memory/people/ana.md"),
            MemoryAction::Forget {
                path: "memory/prefs.md".to_owned(),
                commit: memory_commit(),
            },
        ] {
            let job = op(action);
            assert_eq!(job.validate(), Ok(()), "{job:?}");
            let encoded = serde_json::to_value(&job).unwrap();
            assert_eq!(
                serde_json::from_value::<AssistantJob>(encoded).unwrap(),
                job
            );
        }
        let mut unnamed = op(MemoryAction::List {});
        unnamed.person_id = None;
        assert!(unnamed.validate().is_err(), "whose memory?");
        let mut both = op(MemoryAction::List {});
        both.close = true;
        assert!(both.validate().is_err(), "a job does one thing");
        for path in [
            "memory/",
            "memory",
            "notes/a.md",
            "memory/../x.md",
            "memory/.hidden",
            "memory/a\\b.md",
            "memory/a\u{7}.md",
            "/memory/a.md",
            "memory//a.md",
        ] {
            assert!(op(write(path)).validate().is_err(), "{path}");
        }
        let large = op(MemoryAction::Write {
            path: "memory/big.md".to_owned(),
            text: "x".repeat(MAX_ASSISTANT_CHANGE_BYTES + 1),
            base: None,
            commit: memory_commit(),
        });
        assert!(large.validate().is_err());
        let based = |base: &str| {
            op(MemoryAction::Write {
                path: "memory/a.md".to_owned(),
                text: String::new(),
                base: Some(base.to_owned()),
                commit: memory_commit(),
            })
        };
        assert_eq!(based(&"a".repeat(64)).validate(), Ok(()));
        assert!(based("abc").validate().is_err());
        let mut forged = memory_commit();
        forged.author_email = "eve>@example.invalid".to_owned();
        assert!(
            op(MemoryAction::Forget {
                path: "memory/a.md".to_owned(),
                commit: forged,
            })
            .validate()
            .is_err()
        );
        let unknown =
            serde_json::json!({"repo": "/srv/mp/eve.git", "action": {"kind": "list", "x": 1}});
        assert!(serde_json::from_value::<MemoryOp>(unknown).is_err());
        let answer = MemoryAnswer {
            missing: true,
            ..MemoryAnswer::default()
        };
        assert_eq!(
            serde_json::to_value(&answer).unwrap(),
            serde_json::json!({"missing": true}),
            "an answer carries only what it says"
        );
    }

    #[test]
    fn a_reach_capped_by_another_is_never_wider_than_either() {
        let wide = reach(&["python3", "node"], "limited", &["a.example", "b.example"]);
        let narrow = reach(&["python3"], "limited", &["b.example", "c.example"]);
        assert_eq!(
            wide.capped_by(&narrow),
            reach(&["python3"], "limited", &["b.example"])
        );
        let open = reach(&["python3"], "open", &[]);
        assert_eq!(
            open.capped_by(&wide),
            reach(&["python3"], "limited", &["a.example", "b.example"]),
            "an open side reaches what the limited side lists"
        );
        assert_eq!(open.capped_by(&open), open);
        let denied = reach(&[], "denied", &[]);
        assert_eq!(wide.capped_by(&denied), denied);
        assert_eq!(denied.capped_by(&open), denied);
        let unknown = reach(&["python3"], "everything", &["a.example"]);
        assert_eq!(
            open.capped_by(&unknown).network.policy,
            "denied",
            "an unknown policy counts as the strictest"
        );
        let mut loose = open.clone();
        loose.unsandboxed_scripts = true;
        assert!(!loose.capped_by(&open).unsandboxed_scripts);
        assert!(loose.capped_by(&loose).unsandboxed_scripts);
        assert_eq!(AssistantReach::default().validate(), Ok(()));
        assert!(AssistantReach::default().network.reaches_nothing());
    }

    #[test]
    fn hosts_and_interpreters_are_names_never_patterns_or_paths() {
        for host in [
            "example.com",
            "*.example.com",
            "a-b.c1.example",
            "xn--e1a.example",
        ] {
            assert!(host_pattern(host), "{host}");
        }
        for host in [
            "*",
            "localhost",
            "Example.com",
            "*example.com",
            "a.*.example",
            "example.*",
            "10.0.0.1",
            "-a.example",
            "a..example",
            "a.example:443",
            "a.example/path",
            "",
        ] {
            assert!(!host_pattern(host), "{host:?}");
        }
        for name in ["python3", "python3.12", "node", "Rscript"] {
            assert!(interpreter_name(name), "{name}");
        }
        for name in [
            "",
            "/usr/bin/python3",
            "python3 -c",
            "-c",
            ".hidden",
            "a\\b",
            "sh;rm",
        ] {
            assert!(!interpreter_name(name), "{name:?}");
        }
        let mut wild = job();
        wild.reach = reach(&["python3"], "limited", &["*"]);
        assert!(wild.validate().unwrap_err().contains("host"));
        let mut mode = AssistantMode::moneypenny();
        mode.network.allowed_hosts.push("*".to_owned());
        assert!(mode.validate().is_err(), "a bare wildcard is not a host");
        let mut mode = AssistantMode::moneypenny();
        mode.deny_read.push("relative/home".to_owned());
        assert!(mode.validate().is_err());
    }

    #[test]
    fn cable_grants_are_bounded_and_name_their_person() {
        let grant = |operation: &str| AssistantCableGrant {
            operation: operation.to_owned(),
            send: true,
            read: false,
        };
        let mut granted = job();
        granted.cables = vec![grant("G")];
        assert!(granted.validate().is_err(), "whose grants?");
        granted.person_id = Some("person-007".to_owned());
        assert_eq!(granted.validate(), Ok(()));
        granted.cables.push(grant("G"));
        assert!(granted.validate().is_err(), "an Operation once");
        granted.cables = (0..=MAX_ASSISTANT_CABLE_GRANTS)
            .map(|index| grant(&format!("op-{index}")))
            .collect();
        assert!(granted.validate().is_err());
        let encoded = serde_json::to_value(job()).unwrap();
        assert!(encoded.get("cables").is_none());

        let mut confirm = job();
        confirm.message = String::new();
        confirm.cable_confirm = Some(AssistantCableDecision {
            id: "held-1".to_owned(),
            digest: "d".repeat(64),
        });
        assert_eq!(
            confirm.validate(),
            Ok(()),
            "a confirmation carries no message"
        );
        confirm.close = true;
        assert!(confirm.validate().is_err());
        confirm.close = false;
        confirm.cable_discard = confirm.cable_confirm.clone();
        assert!(confirm.validate().is_err());
    }

    fn change() -> AssistantChange {
        AssistantChange::new(BTreeMap::from([(
            "memory/prefs.md".to_owned(),
            Some("tables".to_owned()),
        )]))
    }

    fn commit() -> AssistantCommit {
        AssistantCommit {
            kind: COMMIT_MEMORY.to_owned(),
            repo: "/srv/mp/eve.git".to_owned(),
            change: change(),
            message: "moneypenny: remember\n\nVia: Moneypenny (conversation c-1)".to_owned(),
            author_name: "Eve".to_owned(),
            author_email: "eve@example.invalid".to_owned(),
        }
    }

    #[test]
    fn a_commit_job_carries_one_bounded_change_and_nothing_else() {
        let committing = AssistantJob {
            message: String::new(),
            commit: Some(commit()),
            person_id: Some("eve".to_owned()),
            ..job()
        };
        assert_eq!(committing.validate(), Ok(()));
        let mut both = committing.clone();
        both.close = true;
        assert!(both.validate().is_err(), "a job does one thing");

        let mut forged = commit();
        forged.change.files.insert("memory/x.md".to_owned(), None);
        assert!(forged.validate().unwrap_err().contains("digest"));
        let mut outside = commit();
        outside.change = AssistantChange::new(BTreeMap::from([(
            "notes/a.md".to_owned(),
            Some("a".to_owned()),
        )]));
        assert!(
            outside.validate().is_err(),
            "a memory commit writes memory/ only"
        );
        outside.kind = COMMIT_SHARED.to_owned();
        assert_eq!(outside.validate(), Ok(()));
        for path in [".git/config", "memory/../x", "/etc/passwd", "a\\b"] {
            let mut bad = commit();
            bad.kind = COMMIT_SHARED.to_owned();
            bad.change = AssistantChange::new(BTreeMap::from([(path.to_owned(), None)]));
            assert!(bad.validate().is_err(), "{path}");
        }
        let mut large = commit();
        large.change = AssistantChange::new(BTreeMap::from([(
            "memory/big.md".to_owned(),
            Some("x".repeat(MAX_ASSISTANT_CHANGE_BYTES + 1)),
        )]));
        assert!(large.validate().is_err());
        let mut author = commit();
        author.author_name = "Eve <evil@example.invalid>".to_owned();
        assert!(author.validate().is_err());

        let mut working = job();
        working.workspace = Some(AssistantWorkspace {
            personal: "ext::sh".to_owned(),
            shared: BTreeMap::new(),
            author_name: "Eve".to_owned(),
            author_email: "eve@example.invalid".to_owned(),
        });
        assert!(working.validate().is_err(), "a remote helper is refused");
        let encoded = serde_json::to_value(&committing).unwrap();
        assert_eq!(
            serde_json::from_value::<AssistantJob>(encoded).unwrap(),
            committing
        );
    }

    #[test]
    fn a_held_cables_digest_covers_everything_the_person_confirms() {
        let cable = AssistantCable {
            id: "held-1".to_owned(),
            operation: "G".to_owned(),
            target: "agent:008".to_owned(),
            recipient: "agent:008".to_owned(),
            ttl_seconds: Some(600),
            body: Some("Deploy now.".to_owned()),
            digest: String::new(),
            cable_id: None,
        };
        let digest = cable.request_digest();
        assert_eq!(digest.len(), 64);
        let changed = [
            AssistantCable {
                operation: "H".to_owned(),
                ..cable.clone()
            },
            AssistantCable {
                recipient: "agent:009".to_owned(),
                ..cable.clone()
            },
            AssistantCable {
                ttl_seconds: None,
                ..cable.clone()
            },
            AssistantCable {
                body: Some("Deploy now!".to_owned()),
                ..cable.clone()
            },
            // Moving a byte between fields is a different request too.
            AssistantCable {
                target: "agent:00".to_owned(),
                recipient: "8agent:008".to_owned(),
                ..cable.clone()
            },
        ];
        for other in changed {
            assert_ne!(other.request_digest(), digest, "{other:?}");
        }
        assert_eq!(
            AssistantCable {
                id: "held-2".to_owned(),
                ..cable.clone()
            }
            .request_digest(),
            digest,
            "the desk's ID is not part of the request"
        );
    }

    #[test]
    fn a_job_is_bounded_and_its_token_never_printed() {
        assert_eq!(job().validate(), Ok(()));
        let shown = format!("{:?}", job());
        assert!(!shown.contains(&"t".repeat(43)), "{shown}");
        let mut blank = job();
        blank.message = "  ".to_owned();
        assert!(blank.validate().is_err());
        blank.close = true;
        assert_eq!(blank.validate(), Ok(()), "a close carries no message");
        blank.interrupt = true;
        assert!(blank.validate().is_err(), "a job closes or interrupts");
        blank.close = false;
        assert_eq!(blank.validate(), Ok(()), "nor does an interrupt");
        let mut long = job();
        long.message = "x".repeat(MAX_ASSISTANT_MESSAGE_BYTES + 1);
        assert!(long.validate().is_err());
        let mut token = job();
        token.mcp_token = "short".to_owned();
        assert!(token.validate().is_err());
        let mut control = job();
        control.conversation_id = "a\nb".to_owned();
        assert!(control.validate().is_err());
    }

    #[test]
    fn writable_directories_stay_inside_the_working_directory() {
        let with = |files: &[&str]| AssistantMode {
            files: files.iter().map(|path| (*path).to_owned()).collect(),
            ..AssistantMode::moneypenny()
        };
        assert_eq!(
            with(&["personal", "scratch", "personal/notes"]).validate(),
            Ok(())
        );
        for bad in [
            "",
            "/etc",
            "..",
            "personal/../shared",
            "personal/.git",
            ".claude",
            "a\\b",
            "C:",
            "a//b",
        ] {
            assert!(with(&[bad]).validate().is_err(), "{bad:?}");
        }
        let mut relative = AssistantMode::moneypenny();
        relative.context_file = Some("context.md".to_owned());
        assert!(relative.validate().is_err());
        relative.context_file = Some(
            std::env::temp_dir()
                .join("context.md")
                .to_string_lossy()
                .into_owned(),
        );
        assert_eq!(relative.validate(), Ok(()));
    }

    #[test]
    fn event_text_is_cut_on_a_character_boundary() {
        let text = "é".repeat(MAX_ASSISTANT_EVENT_BYTES);
        let event = AssistantEvent::new(EVENT_TEXT, Some(text));
        let kept = event.text.unwrap();
        assert!(kept.len() <= MAX_ASSISTANT_EVENT_BYTES);
        assert!(kept.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_job_round_trips_and_refuses_unknown_fields() {
        let encoded = serde_json::to_value(job()).unwrap();
        let decoded: AssistantJob = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded, job());
        let mut extra = encoded;
        extra["shell"] = serde_json::json!(true);
        assert!(serde_json::from_value::<AssistantJob>(extra).is_err());
    }
}
