//! The assistant Session contract between Universal Exports and a Station.
//!
//! Moneypenny, the operator's assistant, runs as an Agent Session on a Station through the
//! same AgentHandler providers as every Agent, started with [`AgentStart::assistant`]. Its
//! tools are not on the Station: Universal Exports serves them on an MCP endpoint of its own,
//! authenticated by a token bound to the person and the conversation, and every tool runs as
//! that person. The Station takes [`AssistantJob`]s from Universal Exports (one per message
//! the person sends), hands the harness the endpoint's URL and token in its environment, and
//! reports what the harness says as [`AssistantReport`]s.
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
            skills: Vec::new(),
            network: AssistantNetwork::default(),
            files: Vec::new(),
            context_file: None,
        }
    }

    /// Checks the parts: at least one server; server names of letters, digits, `-` and `_`;
    /// environment prefixes of capitals, digits and `_`; interpreter and skill names of
    /// letters, digits, `.`, `-` and `_`; writable directories of one to eight `/`-separated
    /// names of letters, digits, `.`, `-` and `_`, none starting with `.`; a context file
    /// given as an absolute path. A handler checks this before building settings from the
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
        for name in self.interpreters.iter().chain(&self.skills) {
            if !word(name, &['.', '-', '_']) {
                return Err(format!(
                    "{name:?} is not an allowed interpreter or skill name"
                ));
            }
        }
        if !matches!(self.network.policy.as_str(), "limited" | "denied" | "open") {
            return Err(format!(
                "network policy {:?} is not limited, denied or open",
                self.network.policy
            ));
        }
        for host in &self.network.allowed_hosts {
            if !word(host, &['.', '-', '*']) || host.len() > 253 {
                return Err(format!("{host:?} is not a host name"));
            }
        }
        for path in &self.files {
            let names: Vec<&str> = path.split('/').collect();
            if names.len() > 8 || !names.iter().all(|name| file_safe(name)) {
                return Err(format!(
                    "{path:?} is not a directory the assistant may write"
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

/// An [`AssistantCommit`] to the personal repository's `memory/`.
pub const COMMIT_MEMORY: &str = "memory";
/// An [`AssistantCommit`] to a shared repository.
pub const COMMIT_SHARED: &str = "shared";
/// The most bytes of file content one memory change or commit carries.
pub const MAX_ASSISTANT_CHANGE_BYTES: usize = 64 * 1024;
/// The most files one memory change or commit carries.
pub const MAX_ASSISTANT_CHANGE_FILES: usize = 32;

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
    /// The conversation has read text others wrote, so a memory change waits for the person.
    #[serde(default)]
    pub exposed: bool,
}

/// A memory change the desk set aside, or the files of a commit: each path, relative to the
/// repository, with its new text, or `None` to remove it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantChange {
    /// The SHA-256 of the files, as [`AssistantChange::digest_of`] makes it, in hex.
    pub digest: String,
    /// The files.
    pub files: BTreeMap<String, Option<String>>,
}

impl AssistantChange {
    /// The change of `files`, with its digest.
    pub fn new(files: BTreeMap<String, Option<String>>) -> Self {
        Self {
            digest: Self::digest_of(&files),
            files,
        }
    }

    /// The SHA-256, in hex, over each path and its text (or removal), in path order.
    pub fn digest_of(files: &BTreeMap<String, Option<String>>) -> String {
        let mut hasher = Sha256::new();
        for (path, text) in files {
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
        if Self::digest_of(&self.files) != self.digest {
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
    /// Where the conversation keeps its work; `None` keeps nothing, in an empty directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<AssistantWorkspace>,
    /// Make this commit instead of sending a message; `message` is empty. The Station answers
    /// `accepted` with where the commit landed, or `rejected` with why it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<AssistantCommit>,
}

impl AssistantJob {
    /// Checks the job's bounds: identities of 1 to 256 bytes without control characters, a
    /// message within [`MAX_ASSISTANT_MESSAGE_BYTES`] (empty only to close, interrupt or
    /// commit, of which a job does one), context within [`MAX_ASSISTANT_CONTEXT_BYTES`], a
    /// token of 32 to 512 bytes, and a workspace and commit as their own `validate` checks
    /// them.
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
        let controls = [self.close, self.interrupt, self.commit.is_some()];
        if controls.iter().filter(|set| **set).count() > 1 {
            return Err("a job closes, interrupts or commits, only one".to_owned());
        }
        let control = controls.contains(&true);
        if self.message.len() > MAX_ASSISTANT_MESSAGE_BYTES
            || (self.message.trim().is_empty() && !control)
        {
            return Err("message is empty or too long".to_owned());
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
            .field("workspace", &self.workspace.is_some())
            .field(
                "commit",
                &self.commit.as_ref().map(|commit| commit.kind.as_str()),
            )
            .finish()
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
            workspace: None,
            commit: None,
        }
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
            exposed: false,
        });
        assert!(working.validate().is_err(), "a remote helper is refused");
        let encoded = serde_json::to_value(&committing).unwrap();
        assert_eq!(
            serde_json::from_value::<AssistantJob>(encoded).unwrap(),
            committing
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
