//! Each Session's context: what its harness keeps of the Session (Claude Code's transcript,
//! Codex's rollout, the Antigravity CLI's), filed by the Station when a Mission's Session ends
//! and after each of Moneypenny's turns, so a person can read what the agent was given and
//! did. A harness that keeps no record of its own gets the Station's event stream instead, and
//! the record says which it is.
//!
//! One record per provider Session, replaced each time it is filed. A context past
//! `[registry.limits.contexts] context_bytes` keeps its newest part, from a line start, and
//! says how large it was; past `records`, the oldest filed go first.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::limits::ContextLimits;
use super::store::{FileRegistry, hex_digest, invalid_data, safe_component};
use crate::bounds::Limit;

/// The format of a context the Station filed from its own event stream: one JSON object per
/// line, each an event as the handler reported it.
pub const EVENTS_FORMAT: &str = "ai6-events-jsonl";

/// What a context belongs to.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextOwner {
    /// A Mission's Session.
    Mission {
        /// The Operation.
        operation: String,
        /// The Mission, by UID.
        mission: String,
    },
    /// One of Moneypenny's conversations.
    Conversation {
        /// The conversation.
        conversation: String,
    },
}

/// Where a context came from.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSource {
    /// The harness's own record of the Session.
    Harness,
    /// The Station's event stream, for a harness that keeps none it can name.
    Events,
}

/// One Session's context.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionContext {
    /// The provider Session.
    pub session: String,
    /// What it belongs to.
    pub owner: ContextOwner,
    /// The Station that filed it.
    pub station: String,
    /// The AgentHandler provider that ran the Session.
    pub handler: String,
    /// The format of `content`, as the handler names it (`claude-code-jsonl`,
    /// `codex-rollout-jsonl`, `antigravity-jsonl`), or [`EVENTS_FORMAT`].
    pub format: String,
    /// Where it came from.
    pub source: ContextSource,
    /// When it was filed, as Unix seconds.
    pub filed_at: f64,
    /// The size of the whole record before its oldest part was cut, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_from: Option<u64>,
    /// The context as kept.
    #[serde(skip)]
    pub content: String,
}

impl SessionContext {
    /// The context of `session`, filed now by `station` from `source`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session: impl Into<String>,
        owner: ContextOwner,
        station: impl Into<String>,
        handler: impl Into<String>,
        format: impl Into<String>,
        source: ContextSource,
        filed_at: f64,
        content: impl Into<String>,
    ) -> Self {
        Self {
            session: session.into(),
            owner,
            station: station.into(),
            handler: handler.into(),
            format: format.into(),
            source,
            filed_at,
            cut_from: None,
            content: content.into(),
        }
    }

    /// Keeps the newest part of `content` that fits `limit` bytes, from the start of a line
    /// when one starts within it, and records the size it had; nothing changes when it fits.
    #[must_use]
    pub fn cut_to(mut self, limit: Limit) -> Self {
        if limit.allows(self.content.len()) {
            return self;
        }
        let whole = self.content.len();
        let mut start = whole - limit.or_max().min(whole);
        while !self.content.is_char_boundary(start) {
            start += 1;
        }
        if start > 0
            && self.content.as_bytes()[start - 1] != b'\n'
            && let Some(line) = self.content[start..].find('\n')
        {
            start += line + 1;
        }
        self.content.drain(..start);
        self.cut_from = Some(
            self.cut_from
                .unwrap_or(u64::try_from(whole).unwrap_or(u64::MAX)),
        );
        self
    }

    /// The conversation it belongs to, when it is one of Moneypenny's.
    pub fn conversation(&self) -> Option<&str> {
        match &self.owner {
            ContextOwner::Conversation { conversation } => Some(conversation),
            ContextOwner::Mission { .. } => None,
        }
    }

    /// The Operation it belongs to, when it is a Mission's.
    pub fn operation(&self) -> Option<&str> {
        match &self.owner {
            ContextOwner::Mission { operation, .. } => Some(operation),
            ContextOwner::Conversation { .. } => None,
        }
    }

    pub(super) fn validate(&self) -> io::Result<()> {
        let identity = |value: &str| {
            !value.is_empty() && value.len() <= 256 && !value.contains(char::is_control)
        };
        let owned = match &self.owner {
            ContextOwner::Mission { operation, mission } => {
                identity(operation) && identity(mission)
            }
            ContextOwner::Conversation { conversation } => identity(conversation),
        };
        if !identity(&self.session)
            || !owned
            || !identity(&self.station)
            || !identity(&self.handler)
            || !identity(&self.format)
            || !self.filed_at.is_finite()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a Session context needs a Session, an owner, a Station, a handler, a format \
                 and a time",
            ));
        }
        Ok(())
    }

    /// When it was filed in whole microseconds.
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    pub(super) fn micros(&self) -> i64 {
        (self.filed_at.max(0.0) * 1_000_000.0) as i64
    }
}

/// Each Session's harness context.
pub trait SessionContextStore {
    /// Files `context` for its Session, in place of any filed before, cut to
    /// `[registry.limits.contexts] context_bytes`; the oldest filed go past `records`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a context without a Session, owner, Station, handler, format or
    /// time; I/O errors otherwise.
    fn file_session_context(&self, context: &SessionContext) -> io::Result<()>;

    /// The context filed for `session`, if any.
    ///
    /// # Errors
    ///
    /// Returns an error when it cannot be read.
    fn session_context(&self, session: &str) -> io::Result<Option<SessionContext>>;

    /// The contexts filed for Moneypenny's `conversation`, oldest filed first.
    ///
    /// # Errors
    ///
    /// Returns an error when they cannot be read.
    fn conversation_contexts(&self, conversation: &str) -> io::Result<Vec<SessionContext>>;
}

/// A Session ID as a file name; IDs that differ only in characters a name cannot hold get a
/// digest of the whole ID.
fn session_key(session: &str) -> String {
    let safe = safe_component(session);
    if safe == session {
        return safe;
    }
    let digest = hex_digest(&Sha256::digest(session.as_bytes()));
    format!("{safe}-{}", &digest[..12])
}

impl FileRegistry {
    fn contexts_root(&self) -> PathBuf {
        self.root().join("contexts")
    }

    /// Every context's record without its content, with the path of its record.
    fn context_records(&self) -> io::Result<Vec<(PathBuf, SessionContext)>> {
        let entries = match fs::read_dir(self.contexts_root()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut records = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if let Ok(Some(record)) = read_record(&path) {
                records.push((path, record));
            }
        }
        Ok(records)
    }
}

/// A context's record (`.json`), without its content.
fn read_record(path: &Path) -> io::Result<Option<SessionContext>> {
    match fs::read(path) {
        Ok(bytes) if bytes.len() <= 64 * 1024 => serde_json::from_slice::<SessionContext>(&bytes)
            .map(Some)
            .map_err(|error| invalid_data(format!("invalid Session context: {error}"))),
        Ok(_) => Err(invalid_data("a Session context's record is too large")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The content beside a record, within `limits`.
fn read_content(record: &Path, limits: &ContextLimits) -> io::Result<String> {
    let path = record.with_extension("log");
    let bytes = fs::read(path)?;
    if !limits.context_bytes.allows(bytes.len()) {
        return Err(invalid_data("a Session context exceeds the size limit"));
    }
    String::from_utf8(bytes).map_err(|_| invalid_data("a Session context is not UTF-8"))
}

impl SessionContextStore for FileRegistry {
    fn file_session_context(&self, context: &SessionContext) -> io::Result<()> {
        context.validate()?;
        let limits = &self.limits.contexts;
        let context = context.clone().cut_to(limits.context_bytes);
        let root = self.contexts_root();
        let record = root.join(format!("{}.json", session_key(&context.session)));
        let _lock = self.lock()?;
        fs::create_dir_all(&root)?;
        // The content first: a record always has the content it names.
        self.write_atomic(&record.with_extension("log"), context.content.as_bytes())?;
        self.write_atomic(
            &record,
            &serde_json::to_vec(&context).map_err(io::Error::other)?,
        )?;
        let mut records = self.context_records()?;
        let excess = records.len().saturating_sub(limits.records.or_max());
        if excess > 0 {
            records.sort_by(|a, b| a.1.filed_at.total_cmp(&b.1.filed_at));
            for (path, _) in records.into_iter().take(excess) {
                let _ = fs::remove_file(&path);
                let _ = fs::remove_file(path.with_extension("log"));
            }
        }
        Ok(())
    }

    fn session_context(&self, session: &str) -> io::Result<Option<SessionContext>> {
        let path = self
            .contexts_root()
            .join(format!("{}.json", session_key(session)));
        let Some(mut record) = read_record(&path)? else {
            return Ok(None);
        };
        if record.session != session {
            return Ok(None);
        }
        record.content = read_content(&path, &self.limits.contexts)?;
        Ok(Some(record))
    }

    fn conversation_contexts(&self, conversation: &str) -> io::Result<Vec<SessionContext>> {
        let mut found = Vec::new();
        for (path, mut record) in self.context_records()? {
            if record.conversation() != Some(conversation) {
                continue;
            }
            record.content = read_content(&path, &self.limits.contexts)?;
            found.push(record);
        }
        found.sort_by(|a, b| a.filed_at.total_cmp(&b.filed_at));
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_context_is_filed_replaced_and_read_by_session,
        a_conversation_s_contexts_are_its_own,
        a_context_without_its_parts_is_refused,
    );

    fn mission(session: &str, content: &str, at: f64) -> SessionContext {
        SessionContext::new(
            session,
            ContextOwner::Mission {
                operation: "G".into(),
                mission: "0195-uid".into(),
            },
            "hq",
            "claude",
            "claude-code-jsonl",
            ContextSource::Harness,
            at,
            content,
        )
    }

    fn conversation(session: &str, conversation: &str, at: f64) -> SessionContext {
        SessionContext::new(
            session,
            ContextOwner::Conversation {
                conversation: conversation.into(),
            },
            "hq",
            "acp",
            EVENTS_FORMAT,
            ContextSource::Events,
            at,
            format!("{{\"kind\":\"text\",\"at\":{at}}}\n"),
        )
    }

    fn a_context_is_filed_replaced_and_read_by_session(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.session_context("provider-1").unwrap().is_none());
        registry
            .file_session_context(&mission("provider-1", "{\"a\":1}\n", 1.0))
            .unwrap();
        let newer = mission("provider-1", "{\"a\":1}\n{\"b\":2}\n", 2.0);
        registry.file_session_context(&newer).unwrap();
        let read = store
            .handle()
            .session_context("provider-1")
            .unwrap()
            .unwrap();
        assert_eq!(read, newer);
        assert_eq!(read.operation(), Some("G"));
        assert!(registry.session_context("provider/1").unwrap().is_none());
    }

    fn a_conversation_s_contexts_are_its_own(store: &dyn Store) {
        let registry = store.handle();
        registry
            .file_session_context(&conversation("s-2", "c-1", 20.0))
            .unwrap();
        registry
            .file_session_context(&conversation("s-1", "c-1", 10.0))
            .unwrap();
        registry
            .file_session_context(&conversation("s-3", "c-2", 30.0))
            .unwrap();
        registry
            .file_session_context(&mission("s-4", "x", 40.0))
            .unwrap();
        let mine = registry.conversation_contexts("c-1").unwrap();
        let sessions: Vec<_> = mine
            .iter()
            .map(|context| context.session.as_str())
            .collect();
        assert_eq!(sessions, ["s-1", "s-2"]);
        assert_eq!(mine[0].source, ContextSource::Events);
        assert!(mine[0].content.contains("\"kind\":\"text\""));
        assert!(registry.conversation_contexts("c-9").unwrap().is_empty());
    }

    fn a_context_without_its_parts_is_refused(store: &dyn Store) {
        let registry = store.handle();
        let mut bad = mission("provider-1", "x", 1.0);
        bad.format = String::new();
        assert_eq!(
            registry.file_session_context(&bad).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let bad = mission("provider\n1", "x", 1.0);
        assert!(registry.file_session_context(&bad).is_err());
        assert!(registry.session_context("provider-1").unwrap().is_none());
    }

    #[test]
    fn a_long_context_keeps_its_newest_lines_and_says_how_large_it_was() {
        let content = "{\"turn\":1}\n{\"turn\":2}\n{\"turn\":3}\n";
        let kept = mission("s", content, 1.0).cut_to(Limit::at(15));
        assert_eq!(kept.content, "{\"turn\":3}\n");
        assert_eq!(kept.cut_from, Some(content.len() as u64));
        let whole = mission("s", content, 1.0).cut_to(Limit::UNBOUNDED);
        assert_eq!((whole.content.as_str(), whole.cut_from), (content, None));
        // One line longer than the bound keeps its end.
        let kept = mission("s", &"é".repeat(20), 1.0).cut_to(Limit::at(9));
        assert!(kept.content.len() <= 9 && kept.content.chars().all(|c| c == 'é'));
    }

    #[test]
    fn the_oldest_contexts_go_past_the_bound() {
        let directory = tempfile::tempdir().unwrap();
        let mut limits = crate::directorates::registry::limits::RegistryLimits::default();
        limits.contexts.records = Limit::at(2);
        let registry = FileRegistry::open(directory.path()).with_limits(limits);
        for (session, at) in [("s-1", 1.0), ("s-2", 2.0), ("s-3", 3.0)] {
            registry
                .file_session_context(&conversation(session, "c-1", at))
                .unwrap();
        }
        let kept: Vec<_> = registry
            .conversation_contexts("c-1")
            .unwrap()
            .into_iter()
            .map(|context| context.session)
            .collect();
        assert_eq!(kept, ["s-2", "s-3"]);
    }
}
