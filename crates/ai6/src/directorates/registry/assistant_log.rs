//! Moneypenny's conversation log: every entry of every conversation as Universal Exports kept
//! it, for troubleshooting and audit, and so a conversation outlives a gateway restart.
//!
//! One record per entry: who said it or what it records, its text as sent, its data, and for
//! what a Station's harness reported, the turn that ran it (Station, provider Session, Agent,
//! route, pool account, model, tokens). Universal Exports keeps no secret in an entry: tool
//! tokens are never part of one. Retention is the gateway's: [`AssistantLogStore::prune_assistant_log`]
//! drops entries by age and keeps the newest that fit a size.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::accounts::AccountRef;
use super::limits::AssistantLogLimits;
use super::store::{FileRegistry, hex_digest, safe_component};
use crate::bounds::Limit;
use crate::directorates::m::roster::RouteRef;

/// The turn a harness's entry came from: where it ran and on what.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantTurn {
    /// The Station that ran the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    /// The provider Session of the harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The roster Agent that is Moneypenny.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The route the Session ran on, when her Agent has routes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<RouteRef>,
    /// The pool account the harness held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountRef>,
    /// The model, as the route or the Agent names it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tokens the harness reported for the events of this report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
}

/// One entry of a conversation's log.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantLogEntry {
    /// A time-ordered unique ID.
    pub id: String,
    /// The conversation.
    pub conversation: String,
    /// The person the conversation is for.
    pub person: String,
    /// When it happened, as Unix seconds.
    pub at: f64,
    /// What it records: an entry the app shows (`person`, `moneypenny`, `tools`, `error`,
    /// `proposal`, `view`, `navigation`, `page`, `notice`) or one only the log keeps
    /// (`tool_call`, `tool_result`).
    pub kind: String,
    /// Its text, as sent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Its data: the app's data for the entry, a tool call's arguments, a result's size and
    /// status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// The turn a harness's entry came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<AssistantTurn>,
    /// The size of the entry before it was cut to `[registry.limits.assistant_log]
    /// entry_bytes`, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_from: Option<u64>,
}

impl AssistantLogEntry {
    /// An entry of `kind` with `text`, with a fresh time-ordered ID.
    pub fn new(
        conversation: impl Into<String>,
        person: impl Into<String>,
        kind: impl Into<String>,
        text: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            conversation: conversation.into(),
            person: person.into(),
            at,
            kind: kind.into(),
            text: text.into(),
            data: None,
            turn: None,
            cut_from: None,
        }
    }

    /// Adds its data.
    #[must_use]
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Names the turn it came from.
    #[must_use]
    pub fn with_turn(mut self, turn: AssistantTurn) -> Self {
        self.turn = Some(turn);
        self
    }

    fn validate(&self) -> io::Result<()> {
        let invalid = |what: &str| {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("assistant log entry {what} is invalid"),
            ))
        };
        let identity = |value: &str| {
            !value.is_empty() && value.len() <= 256 && !value.contains(char::is_control)
        };
        if self.id.is_empty()
            || self.id.len() > 128
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return invalid("ID");
        }
        if !identity(&self.conversation) || !identity(&self.person) {
            return invalid("conversation or person");
        }
        if !self.at.is_finite() || self.at <= 0.0 {
            return invalid("time");
        }
        if self.kind.is_empty()
            || self.kind.len() > 32
            || !self
                .kind
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        {
            return invalid("kind");
        }
        Ok(())
    }

    /// When it happened in whole microseconds: entries sort by this, then by ID.
    pub(super) fn micros(&self) -> u64 {
        (self.at * 1_000_000.0) as u64
    }

    /// The stored form of a checked entry. One past `limits` keeps its turn, its text cut to
    /// half the bound and no data, and says how large it was.
    pub(super) fn to_record(&self, limits: &AssistantLogLimits) -> io::Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        if limits.entry_bytes.allows(bytes.len()) {
            return Ok(bytes);
        }
        let mut cut = self.clone();
        cut.data = None;
        let room = limits.entry_bytes.or_max() / 2;
        let mut end = cut.text.len().min(room);
        while !cut.text.is_char_boundary(end) {
            end -= 1;
        }
        cut.text.truncate(end);
        cut.cut_from = Some(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        let bytes = serde_json::to_vec(&cut).map_err(io::Error::other)?;
        if !limits.entry_bytes.allows(bytes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "assistant log entry exceeds the size limit even when cut",
            ));
        }
        Ok(bytes)
    }

    /// Reads a stored entry, `None` when it is out of bounds or invalid.
    pub(super) fn from_record(bytes: &[u8], limits: &AssistantLogLimits) -> Option<Self> {
        if !limits.entry_bytes.allows(bytes.len()) {
            return None;
        }
        serde_json::from_slice::<Self>(bytes)
            .ok()
            .filter(|entry| entry.validate().is_ok())
    }
}

/// One conversation of the log, as a listing shows it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantConversation {
    /// The conversation.
    pub conversation: String,
    /// The person it is for.
    pub person: String,
    /// When its oldest kept entry happened, as Unix seconds.
    pub first_at: f64,
    /// When its newest entry happened, as Unix seconds.
    pub last_at: f64,
    /// Entries kept.
    pub entries: u64,
    /// Bytes of those entries as stored.
    pub bytes: u64,
}

/// Moneypenny's conversation log.
pub trait AssistantLogStore {
    /// Keeps `entry`; one past `[registry.limits.assistant_log] entry_bytes` is kept cut.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an entry with an invalid ID, identity, time or kind; I/O errors
    /// otherwise.
    fn log_assistant_entry(&self, entry: &AssistantLogEntry) -> io::Result<()>;

    /// The entries of `conversation`, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the log cannot be read; an unreadable entry is skipped.
    fn assistant_log(&self, conversation: &str) -> io::Result<Vec<AssistantLogEntry>>;

    /// The conversations in the log, `person`'s only when named, the most recent first.
    ///
    /// # Errors
    ///
    /// Returns an error when the log cannot be read.
    fn assistant_conversations(
        &self,
        person: Option<&str>,
    ) -> io::Result<Vec<AssistantConversation>>;

    /// Drops the entries that happened before `before` (Unix seconds), then the oldest of
    /// those left past `bytes` in all; returns how many went.
    ///
    /// # Errors
    ///
    /// Returns an error when the log cannot be read or changed.
    fn prune_assistant_log(&self, before: Option<f64>, bytes: Limit) -> io::Result<u64>;
}

/// A conversation ID as a directory name; IDs that differ only in characters a name cannot
/// hold get a digest of the whole ID, so two conversations never share one.
fn conversation_key(conversation: &str) -> String {
    let safe = safe_component(conversation);
    if safe == conversation {
        return safe;
    }
    let digest = hex_digest(&Sha256::digest(conversation.as_bytes()));
    format!("{safe}-{}", &digest[..12])
}

/// One stored entry: its directory, its file, when it happened and its size.
struct Stored {
    dir: PathBuf,
    path: PathBuf,
    micros: u64,
    bytes: u64,
}

impl FileRegistry {
    fn assistant_log_root(&self) -> PathBuf {
        self.root().join("assistant-log")
    }

    /// Every stored entry, by conversation directory, oldest first within each.
    fn assistant_log_files(&self) -> io::Result<BTreeMap<PathBuf, Vec<Stored>>> {
        let mut found = BTreeMap::new();
        for dir in directories(&self.assistant_log_root())? {
            found.insert(dir.clone(), entry_files(&dir)?);
        }
        Ok(found)
    }
}

fn directories(root: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// The entry files of one conversation's directory, oldest first.
fn entry_files(dir: &Path) -> io::Result<Vec<Stored>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(micros) = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| name.ends_with(".json"))
            .and_then(|name| name.split('-').next())
            .and_then(|micros| micros.parse::<u64>().ok())
        else {
            continue;
        };
        if !entry.file_type()?.is_file() {
            continue;
        }
        files.push(Stored {
            dir: dir.to_path_buf(),
            path,
            micros,
            bytes: entry.metadata()?.len(),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

impl AssistantLogStore for FileRegistry {
    fn log_assistant_entry(&self, entry: &AssistantLogEntry) -> io::Result<()> {
        let bytes = entry.to_record(&self.limits.assistant_log)?;
        let dir = self
            .assistant_log_root()
            .join(conversation_key(&entry.conversation));
        let _lock = self.lock()?;
        fs::create_dir_all(&dir)?;
        self.write_atomic(
            &dir.join(format!("{:020}-{}.json", entry.micros(), entry.id)),
            &bytes,
        )
    }

    fn assistant_log(&self, conversation: &str) -> io::Result<Vec<AssistantLogEntry>> {
        let dir = self
            .assistant_log_root()
            .join(conversation_key(conversation));
        let mut entries = Vec::new();
        for stored in entry_files(&dir)? {
            let Ok(bytes) = fs::read(&stored.path) else {
                continue;
            };
            if let Some(entry) = AssistantLogEntry::from_record(&bytes, &self.limits.assistant_log)
                && entry.conversation == conversation
            {
                entries.push(entry);
            }
        }
        entries.sort_by(|a, b| (a.micros(), &a.id).cmp(&(b.micros(), &b.id)));
        Ok(entries)
    }

    fn assistant_conversations(
        &self,
        person: Option<&str>,
    ) -> io::Result<Vec<AssistantConversation>> {
        let limits = &self.limits.assistant_log;
        let read = |stored: &Stored| {
            fs::read(&stored.path)
                .ok()
                .and_then(|bytes| AssistantLogEntry::from_record(&bytes, limits))
        };
        let mut conversations = Vec::new();
        for files in self.assistant_log_files()?.values() {
            let (Some(first), Some(last)) = (
                files.iter().find_map(read),
                files.iter().rev().find_map(read),
            ) else {
                continue;
            };
            if person.is_some_and(|person| person != first.person) {
                continue;
            }
            conversations.push(AssistantConversation {
                conversation: first.conversation,
                person: first.person,
                first_at: first.at,
                last_at: last.at,
                entries: u64::try_from(files.len()).unwrap_or(u64::MAX),
                bytes: files.iter().map(|stored| stored.bytes).sum(),
            });
        }
        conversations.sort_by(|a, b| b.last_at.total_cmp(&a.last_at));
        Ok(conversations)
    }

    fn prune_assistant_log(&self, before: Option<f64>, bytes: Limit) -> io::Result<u64> {
        let _lock = self.lock()?;
        let mut stored: Vec<Stored> = self
            .assistant_log_files()?
            .into_values()
            .flatten()
            .collect();
        let cutoff = before.map(|before| (before.max(0.0) * 1_000_000.0) as u64);
        // Newest first: the oldest are the ones past the size.
        stored.sort_by(|a, b| b.micros.cmp(&a.micros).then_with(|| b.path.cmp(&a.path)));
        let mut kept = 0_u64;
        let mut removed = 0_u64;
        let mut touched = Vec::new();
        for entry in stored {
            let old = cutoff.is_some_and(|cutoff| entry.micros < cutoff);
            if !old {
                kept = kept.saturating_add(entry.bytes);
            }
            if old || !bytes.allows(usize::try_from(kept).unwrap_or(usize::MAX)) {
                fs::remove_file(&entry.path)?;
                removed += 1;
                touched.push(entry.dir);
            }
        }
        touched.dedup();
        for dir in touched {
            // Only an empty directory goes.
            let _ = fs::remove_dir(dir);
        }
        Ok(removed)
    }
}

/// The stored form of an entry, for a SQL row.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub(super) fn entry_text(
    entry: &AssistantLogEntry,
    limits: &AssistantLogLimits,
) -> io::Result<String> {
    String::from_utf8(entry.to_record(limits)?)
        .map_err(|_| super::store::invalid_data("assistant log entry is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_conversation_s_entries_come_back_in_order_with_their_turn,
        conversations_are_listed_per_person_newest_first,
        pruning_drops_by_age_then_keeps_the_newest_that_fit,
        an_entry_past_the_bound_is_kept_cut_and_a_bad_one_refused,
    );

    fn turn() -> AssistantTurn {
        AssistantTurn {
            station: Some("hq".into()),
            session: Some("provider-1".into()),
            agent: Some("moneypenny".into()),
            route: Some(RouteRef::new(
                "claude",
                Some("sonnet".into()),
                Some("team".into()),
            )),
            account: Some(AccountRef::new("team", "alice")),
            model: Some("sonnet".into()),
            tokens: Some(1_200),
        }
    }

    fn a_conversation_s_entries_come_back_in_order_with_their_turn(store: &dyn Store) {
        let registry = store.handle();
        let asked = AssistantLogEntry::new("c-1", "eve", "person", "What runs?", 10.0);
        let call = AssistantLogEntry::new("c-1", "eve", "tool_call", "list_missions", 10.5)
            .with_data(serde_json::json!({"arguments": {"operation": "G"}}))
            .with_turn(turn());
        let answer = AssistantLogEntry::new("c-1", "eve", "moneypenny", "Two Missions.", 11.0)
            .with_turn(turn());
        let other = AssistantLogEntry::new("c-2", "eve", "person", "Hello", 9.0);
        for entry in [&answer, &asked, &other, &call] {
            registry.log_assistant_entry(entry).unwrap();
        }
        let same = (0..5)
            .map(|index| AssistantLogEntry::new("c-3", "eve", "notice", format!("{index}"), 5.0))
            .collect::<Vec<_>>();
        for entry in &same {
            registry.log_assistant_entry(entry).unwrap();
        }
        assert_eq!(
            store.handle().assistant_log("c-1").unwrap(),
            [asked, call, answer]
        );
        assert_eq!(
            registry.assistant_log("c-3").unwrap(),
            same,
            "one second keeps its order"
        );
        assert!(registry.assistant_log("c-9").unwrap().is_empty());
        let read = registry.assistant_log("c-1").unwrap();
        let account = read[2].turn.as_ref().and_then(|turn| turn.account.clone());
        assert_eq!(account, Some(AccountRef::new("team", "alice")));
    }

    fn conversations_are_listed_per_person_newest_first(store: &dyn Store) {
        let registry = store.handle();
        for (conversation, person, at) in [
            ("c-1", "eve", 10.0),
            ("c-1", "eve", 30.0),
            ("c-2", "eve", 20.0),
            ("c/3", "bob", 40.0),
        ] {
            registry
                .log_assistant_entry(&AssistantLogEntry::new(
                    conversation,
                    person,
                    "person",
                    "hi",
                    at,
                ))
                .unwrap();
        }
        let all = registry.assistant_conversations(None).unwrap();
        let order: Vec<_> = all.iter().map(|c| c.conversation.as_str()).collect();
        assert_eq!(order, ["c/3", "c-1", "c-2"]);
        let eve = registry.assistant_conversations(Some("eve")).unwrap();
        assert_eq!(eve.len(), 2);
        assert_eq!(
            (eve[0].first_at, eve[0].last_at, eve[0].entries),
            (10.0, 30.0, 2)
        );
        assert!(eve[0].bytes > 0);
        assert!(
            registry
                .assistant_conversations(Some("ann"))
                .unwrap()
                .is_empty()
        );
    }

    fn pruning_drops_by_age_then_keeps_the_newest_that_fit(store: &dyn Store) {
        let registry = store.handle();
        for at in 1..=6 {
            registry
                .log_assistant_entry(&AssistantLogEntry::new(
                    if at % 2 == 0 { "even" } else { "odd" },
                    "eve",
                    "person",
                    "x".repeat(100),
                    f64::from(at) * 100.0,
                ))
                .unwrap();
        }
        assert_eq!(
            registry
                .prune_assistant_log(None, Limit::UNBOUNDED)
                .unwrap(),
            0
        );
        assert_eq!(
            registry
                .prune_assistant_log(Some(250.0), Limit::UNBOUNDED)
                .unwrap(),
            2,
            "100 and 200 are older"
        );
        let one = registry.assistant_log("odd").unwrap()[0].clone();
        let size = serde_json::to_vec(&one).unwrap().len();
        // Room for two and a half entries: the two newest stay.
        let removed = registry
            .prune_assistant_log(None, Limit::at(size as u64 * 5 / 2))
            .unwrap();
        assert_eq!(removed, 2);
        let kept: Vec<f64> = ["odd", "even"]
            .iter()
            .flat_map(|conversation| registry.assistant_log(conversation).unwrap())
            .map(|entry| entry.at)
            .collect();
        assert_eq!(kept, [500.0, 600.0]);
        assert_eq!(registry.assistant_conversations(None).unwrap().len(), 2);
    }

    fn an_entry_past_the_bound_is_kept_cut_and_a_bad_one_refused(store: &dyn Store) {
        let registry = store.handle();
        let bound = AssistantLogLimits::default().entry_bytes.or_max();
        let long = AssistantLogEntry::new("c-1", "eve", "moneypenny", "y".repeat(bound + 10), 1.0)
            .with_data(serde_json::json!({"big": "z".repeat(bound)}));
        registry.log_assistant_entry(&long).unwrap();
        let kept = &registry.assistant_log("c-1").unwrap()[0];
        assert!(kept.data.is_none());
        assert!(kept.text.len() <= bound / 2);
        assert!(kept.cut_from.is_some_and(|size| size > bound as u64));
        for bad in [
            AssistantLogEntry::new("", "eve", "person", "x", 1.0),
            AssistantLogEntry::new("c", "eve", "Person", "x", 1.0),
            AssistantLogEntry::new("c", "eve\n", "person", "x", 1.0),
            AssistantLogEntry::new("c", "eve", "person", "x", 0.0),
        ] {
            assert_eq!(
                registry.log_assistant_entry(&bad).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}
