//! Memories: rows of text with a scope, searched by word.
//!
//! An entry belongs to one [`MemoryScope`]: a person, an Operation or the global one. Its
//! identity is a generated UUID, set once; names and summaries may repeat. A write names the
//! revision it was made from and is refused when the entry has moved on. Prior versions are kept
//! up to `[registry.limits.memories] versions`, and forgetting an entry removes them.
//!
//! Who may see or change an entry is not decided here: `directorates::m::memories` does, and every
//! read goes through it. What a store offers a search is [`MemoryStore::memory_candidates`], every
//! entry of the scopes holding one of the words, whoever may see it; a database's text index only
//! narrows that set. [`matching`] then ranks what is left once the caller has filtered it, so a
//! search answers the same on every backend and no score rests on an entry the reader may not see.
//!
//! What an Agent may not write at once waits as a [`MemoryProposal`] for a person.
//!
//! The file Registry keeps `memories/<scope digest>/<id>.json`, prior versions in
//! `memories/<scope digest>/versions/<id>/<revision>.json` and proposals in
//! `memories/<scope digest>/proposals/<id>.json`. A scope's directory is a digest so that two
//! scopes differing only by case stay apart on a filesystem that folds case.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::limits::MemoryLimits;
use super::store::{FileRegistry, hex_digest, invalid_data};

/// Whose an entry is.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemoryScope {
    /// One person's, by ID: what Moneypenny remembers of them.
    Person(String),
    /// An Operation's knowledge base.
    Operation(String),
    /// The knowledge base everyone shares.
    Global,
}

impl MemoryScope {
    /// The scope `text` names: `person:<id>`, `operation:<id>` or `global`.
    pub fn parse(text: &str) -> Option<Self> {
        if text == "global" {
            return Some(Self::Global);
        }
        match text.split_once(':')? {
            ("person", id) if !id.is_empty() => Some(Self::Person(id.to_owned())),
            ("operation", id) if !id.is_empty() => Some(Self::Operation(id.to_owned())),
            _ => None,
        }
    }
}

impl fmt::Display for MemoryScope {
    /// `person:<id>`, `operation:<id>` or `global`.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Person(id) => write!(formatter, "person:{id}"),
            Self::Operation(id) => write!(formatter, "operation:{id}"),
            Self::Global => formatter.write_str("global"),
        }
    }
}

impl Serialize for MemoryScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MemoryScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("{text:?} is not a memory scope")))
    }
}

/// Who wrote an entry, and whether what wrote it had read outside text.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySource {
    /// `person:<id>`, `agent:<codename>`, or `moneypenny:<person id>` for what she wrote for them.
    pub by: String,
    /// The Mission Session or the conversation it was written from, when one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// How that Session was exposed when it wrote this; empty means it was not. An entry with any
    /// exposes whoever reads it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exposures: Vec<String>,
}

impl MemorySource {
    /// Written by `by`, from `session` when there is one, with no mark.
    pub fn new(by: impl Into<String>, session: Option<String>) -> Self {
        Self {
            by: by.into(),
            session,
            exposures: Vec::new(),
        }
    }

    /// This source, marked with `exposures`.
    #[must_use]
    pub fn exposed_by(mut self, exposures: impl IntoIterator<Item = String>) -> Self {
        self.exposures = exposures.into_iter().collect();
        self
    }

    /// Whether what wrote it was exposed.
    pub fn exposed(&self) -> bool {
        !self.exposures.is_empty()
    }
}

/// An entry without its content: what a listing and a starting context hold.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryHeader {
    /// A generated UUID, what every tool and route names it by.
    pub id: String,
    /// Whose it is.
    pub scope: MemoryScope,
    /// An optional name; two entries may share one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// One line saying what it is for; two entries may share one.
    pub summary: String,
    /// When it was made, as Unix seconds.
    pub created_at: f64,
    /// When it last changed, as Unix seconds.
    pub updated_at: f64,
    /// Who last changed it: as [`MemorySource::by`].
    pub updated_by: String,
    /// Who wrote what it holds now.
    pub source: MemorySource,
    /// Narrows who can see it at all: a reader needs the scope's read and to be read into one of
    /// these. Empty asks nothing more.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compartments: Vec<String>,
    /// How many times it has been written; a write names the one it was made from.
    pub revision: u64,
    /// Bytes of its name, summary and content.
    pub bytes: u64,
}

/// One entry.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// All but the text.
    pub header: MemoryHeader,
    /// The text.
    pub content: String,
}

impl MemoryEntry {
    /// A new entry of `scope` with a fresh ID, written by `source` at `at`.
    pub fn new(
        scope: MemoryScope,
        name: Option<String>,
        summary: impl Into<String>,
        content: impl Into<String>,
        source: MemorySource,
        at: f64,
    ) -> Self {
        Self::with_id(
            Uuid::new_v4().to_string(),
            scope,
            name,
            summary,
            content,
            source,
            at,
        )
    }

    /// An entry with this ID: a change to the entry `id`, for [`MemoryStore::write_memory`].
    pub fn with_id(
        id: String,
        scope: MemoryScope,
        name: Option<String>,
        summary: impl Into<String>,
        content: impl Into<String>,
        source: MemorySource,
        at: f64,
    ) -> Self {
        let updated_by = source.by.clone();
        let mut entry = Self {
            header: MemoryHeader {
                id,
                scope,
                name,
                summary: summary.into(),
                created_at: at,
                updated_at: at,
                updated_by,
                source,
                compartments: Vec::new(),
                revision: 1,
                bytes: 0,
            },
            content: content.into(),
        };
        entry.header.bytes = entry.size();
        entry
    }

    /// This entry behind `compartments`, sorted and without repeats.
    #[must_use]
    pub fn behind(mut self, compartments: impl IntoIterator<Item = String>) -> Self {
        self.header.compartments = compartments
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self
    }

    /// Bytes of its name, summary and content.
    pub(super) fn size(&self) -> u64 {
        (self.header.name.as_deref().map_or(0, str::len)
            + self.header.summary.len()
            + self.content.len()) as u64
    }

    /// Checks the entry's bounds: a UUID, a one-line summary, a name and an author of one line, no
    /// NUL in the content, each within `limits`.
    ///
    /// # Errors
    ///
    /// A message for the writer, naming what is wrong.
    pub fn validate(&self, limits: &MemoryLimits) -> Result<(), String> {
        let header = &self.header;
        if !is_memory_id(&header.id) {
            return Err("an entry's ID is a generated UUID".to_owned());
        }
        let line = |text: &str, bound: crate::bounds::Limit| {
            !text.trim().is_empty() && bound.allows(text.len()) && !text.contains(char::is_control)
        };
        if !line(&header.summary, limits.summary_bytes) {
            return Err(format!(
                "a summary is one line of 1 to {} bytes",
                limits.summary_bytes
            ));
        }
        if !header
            .name
            .as_deref()
            .is_none_or(|name| line(name, limits.name_bytes))
        {
            return Err(format!(
                "a name is one line of 1 to {} bytes",
                limits.name_bytes
            ));
        }
        if !line(&header.updated_by, limits.name_bytes)
            || !line(&header.source.by, limits.name_bytes)
        {
            return Err(format!(
                "who wrote an entry is one line of 1 to {} bytes",
                limits.name_bytes
            ));
        }
        if !limits.content_bytes.allows(self.content.len()) {
            return Err(format!(
                "an entry holds at most {} bytes",
                limits.content_bytes
            ));
        }
        // PostgreSQL text cannot hold it.
        if self.content.contains('\0') {
            return Err("an entry's content is text, with no NUL".to_owned());
        }
        if !limits.compartments.allows(header.compartments.len())
            || !header
                .compartments
                .iter()
                .all(|compartment| line(compartment, limits.name_bytes))
        {
            return Err(format!(
                "an entry names at most {} compartments",
                limits.compartments
            ));
        }
        if !header.created_at.is_finite() || !header.updated_at.is_finite() {
            return Err("an entry's times are not numbers".to_owned());
        }
        Ok(())
    }
}

/// A change an Agent asked for, waiting for a person. It carries who asked and how exposed they
/// were, so an entry made from it stays marked.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryProposal {
    /// A generated UUID.
    pub id: String,
    /// The scope it changes.
    pub scope: MemoryScope,
    /// The entry it changes, and the revision it was made from; `None` for a new entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<(String, u64)>,
    /// It forgets the entry, rather than writing it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forget: bool,
    /// The entry's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The entry's one-line summary.
    pub summary: String,
    /// The entry's whole new text.
    pub content: String,
    /// The compartments it would set; `None` leaves them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compartments: Option<Vec<String>>,
    /// Who asked.
    pub source: MemorySource,
    /// When, as Unix seconds.
    pub at: f64,
}

impl MemoryProposal {
    /// A proposal with a fresh ID, of a new entry, or with `target` of a change to that entry at
    /// that revision.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope: MemoryScope,
        target: Option<(String, u64)>,
        name: Option<String>,
        summary: impl Into<String>,
        content: impl Into<String>,
        compartments: Option<Vec<String>>,
        source: MemorySource,
        at: f64,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            scope,
            target,
            forget: false,
            name,
            summary: summary.into(),
            content: content.into(),
            compartments,
            source,
            at,
        }
    }

    /// Checks its bounds, as the entry it would make.
    ///
    /// # Errors
    ///
    /// A message for the writer, naming what is wrong.
    pub fn validate(&self, limits: &MemoryLimits) -> Result<(), String> {
        if !is_memory_id(&self.id)
            || self
                .target
                .as_ref()
                .is_some_and(|(id, _)| !is_memory_id(id))
        {
            return Err("a proposal and the entry it changes are named by UUIDs".to_owned());
        }
        MemoryEntry::with_id(
            self.id.clone(),
            self.scope.clone(),
            self.name.clone(),
            self.summary.clone(),
            self.content.clone(),
            self.source.clone(),
            self.at,
        )
        .behind(self.compartments.clone().unwrap_or_default())
        .validate(limits)
    }
}

/// Whether `id` is a UUID in the one form this makes them: lower case, with hyphens. Anything
/// else names no entry, so it cannot climb out of a directory or match by case.
pub fn is_memory_id(id: &str) -> bool {
    Uuid::parse_str(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id)
}

/// Why a write to the memory store did not happen.
#[non_exhaustive]
#[derive(Debug)]
pub enum MemoryError {
    /// The entry is out of bounds; the message is for the writer.
    Invalid(String),
    /// Its scope holds as much as it may; the message says nothing of what it holds.
    Full(String),
    /// There is no such entry, or none in this scope.
    Missing,
    /// It has moved on from the revision the write was made from.
    Changed {
        /// The revision it is at.
        revision: u64,
    },
    /// The store failed.
    Io(io::Error),
}

impl fmt::Display for MemoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) | Self::Full(reason) => formatter.write_str(reason),
            Self::Missing => formatter.write_str("no such entry"),
            Self::Changed { revision } => {
                write!(formatter, "the entry has moved on to revision {revision}")
            }
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<io::Error> for MemoryError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Memories: the entries of each scope.
pub trait MemoryStore {
    /// The bounds it holds entries to.
    fn memory_limits(&self) -> &MemoryLimits;

    /// Keeps `entry` and answers it as stored. With `base` `None` it is new, at revision 1: an ID
    /// already taken is [`MemoryError::Changed`]. With `base` it replaces the entry of its ID in
    /// its scope, when that is at revision `base`, keeps what it held as a prior version, and is
    /// at the next revision; its `created_at` stays the first.
    ///
    /// # Errors
    ///
    /// [`MemoryError`]: out of bounds, past the scope's limits, not there, moved on, or the store
    /// failing.
    fn write_memory(
        &self,
        entry: MemoryEntry,
        base: Option<u64>,
    ) -> Result<MemoryEntry, MemoryError>;

    /// The entry `id`, whoever's it is. The caller decides who may see it.
    ///
    /// # Errors
    ///
    /// I/O errors. An ID that is not one answers `None`.
    fn memory(&self, id: &str) -> io::Result<Option<MemoryEntry>>;

    /// Every entry of `scope` without its content, newest change first. Unreadable entries are
    /// skipped.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn memory_headers(&self, scope: &MemoryScope) -> io::Result<Vec<MemoryHeader>>;

    /// Every entry of `scopes` that may hold one of `terms`, or every one when there are none: at
    /// least the entries [`matching`] would take, whoever may see them. The caller filters and
    /// then ranks, so neither a count nor a score rests on an entry it hid.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn memory_candidates(
        &self,
        scopes: &[MemoryScope],
        terms: &[String],
    ) -> io::Result<Vec<MemoryEntry>>;

    /// The prior versions of the entry `id`, newest first.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn memory_versions(&self, id: &str) -> io::Result<Vec<MemoryEntry>>;

    /// Removes the entry `id` and its prior versions, when it is at revision `base` if one is
    /// given. Answers what it held.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Missing`], [`MemoryError::Changed`], or the store failing.
    fn forget_memory(&self, id: &str, base: Option<u64>) -> Result<MemoryEntry, MemoryError>;

    /// Keeps a proposal until a person decides it.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] out of bounds, [`MemoryError::Full`] past
    /// `[registry.limits.memories] proposals` waiting in its scope, or the store failing.
    fn propose_memory(&self, proposal: &MemoryProposal) -> Result<(), MemoryError>;

    /// What waits in `scope`, oldest first. Unreadable proposals are skipped.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn memory_proposals(&self, scope: &MemoryScope) -> io::Result<Vec<MemoryProposal>>;

    /// The proposal `id`, whoever's it is. The caller decides who may see it.
    ///
    /// # Errors
    ///
    /// I/O errors. An ID that is not one answers `None`.
    fn memory_proposal(&self, id: &str) -> io::Result<Option<MemoryProposal>>;

    /// Removes the proposal `id`, once it is decided; `false` when it was not there.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn drop_memory_proposal(&self, id: &str) -> io::Result<bool>;
}

/// The words of `text`, lower case: every run of letters and digits.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

/// The distinct words of a search, in the order first asked.
pub fn search_terms(query: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    words(query)
        .filter(|word| seen.insert(word.clone()))
        .collect()
}

/// The distinct words of `entry` a database's text index holds, within `limit` bytes.
#[cfg_attr(not(any(feature = "sqlite", feature = "postgres")), allow(dead_code))]
pub(super) fn index_words(entry: &MemoryEntry, limit: crate::bounds::Limit) -> String {
    let distinct: BTreeSet<String> = words(entry.header.name.as_deref().unwrap_or_default())
        .chain(words(&entry.header.summary))
        .chain(words(&entry.content))
        .collect();
    let mut index = String::new();
    for word in distinct {
        if !limit.allows(index.len() + word.len() + 1) {
            break;
        }
        index.push_str(&word);
        index.push(' ');
    }
    index
}

/// `entries` holding at least one of `terms`, best first; all of them, newest change first, when
/// there are no terms. An entry ranks by how many of the terms it holds, then how many of those
/// are in its name or summary, then its newest change, then its ID. An entry holding none is left
/// out. Run on what the reader may see, this is every backend's search.
pub fn matching(entries: Vec<MemoryEntry>, terms: &[String]) -> Vec<MemoryEntry> {
    let mut ranked: Vec<(usize, usize, MemoryEntry)> = entries
        .into_iter()
        .filter_map(|entry| {
            let titled: BTreeSet<String> = words(entry.header.name.as_deref().unwrap_or_default())
                .chain(words(&entry.header.summary))
                .collect();
            let body: BTreeSet<String> = words(&entry.content).collect();
            let in_title = terms.iter().filter(|term| titled.contains(*term)).count();
            let anywhere = terms
                .iter()
                .filter(|term| titled.contains(*term) || body.contains(*term))
                .count();
            (terms.is_empty() || anywhere > 0).then_some((anywhere, in_title, entry))
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then(right.1.cmp(&left.1))
            .then(
                right
                    .2
                    .header
                    .updated_at
                    .total_cmp(&left.2.header.updated_at),
            )
            .then_with(|| left.2.header.id.cmp(&right.2.header.id))
    });
    ranked.into_iter().map(|(_, _, entry)| entry).collect()
}

/// Newest change first, then by ID, so a listing is stable.
pub(super) fn newest(headers: &mut [MemoryHeader]) {
    headers.sort_by(|left, right| {
        right
            .updated_at
            .total_cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

/// What a full queue of proposals says, telling nothing of what they ask.
pub(super) const PROPOSALS_FULL: &str =
    "too many proposals wait for a person here, so this one was not kept";

/// Oldest first, then by ID.
pub(super) fn oldest(proposals: &mut [MemoryProposal]) {
    proposals.sort_by(|left, right| {
        left.at
            .total_cmp(&right.at)
            .then_with(|| left.id.cmp(&right.id))
    });
}

/// `next` as it is stored when it replaces `current` at revision `base`, or why not.
pub(super) fn replacing(
    mut next: MemoryEntry,
    current: &MemoryEntry,
    base: u64,
) -> Result<MemoryEntry, MemoryError> {
    if current.header.scope != next.header.scope {
        return Err(MemoryError::Missing);
    }
    if current.header.revision != base {
        return Err(MemoryError::Changed {
            revision: current.header.revision,
        });
    }
    next.header.created_at = current.header.created_at;
    next.header.revision = base + 1;
    next.header.bytes = next.size();
    Ok(next)
}

/// Whether a scope holding `count` entries of `bytes` in all may take `entry`, in place of
/// `replaced` bytes when it replaces one, or a message that tells nothing of what it holds.
pub(super) fn room_for(
    entry: &MemoryEntry,
    count: u64,
    bytes: u64,
    replaced: Option<u64>,
    limits: &MemoryLimits,
) -> Result<(), MemoryError> {
    let after = bytes
        .saturating_sub(replaced.unwrap_or(0))
        .saturating_add(entry.header.bytes);
    if (replaced.is_none() && !limits.entries.allows(count.saturating_add(1)))
        || !limits.scope_bytes.allows(after)
    {
        return Err(MemoryError::Full(
            "this memory is full, so nothing was kept: forget something first".to_owned(),
        ));
    }
    Ok(())
}

impl FileRegistry {
    fn memories_root(&self) -> PathBuf {
        self.root.join("memories")
    }

    fn memory_scope_dir(&self, scope: &MemoryScope) -> PathBuf {
        let digest = Sha256::digest(scope.to_string().as_bytes());
        self.memories_root().join(hex_digest(&digest))
    }

    /// Every entry file in `dir`, as read; unreadable ones are skipped. A file that says it is
    /// another scope's, or another ID's, is not an entry here.
    fn scope_entries(&self, scope: &MemoryScope) -> io::Result<Vec<MemoryEntry>> {
        let dir = self.memory_scope_dir(scope);
        let mut found = Vec::new();
        for path in entry_files(&dir)? {
            if let Ok(bytes) = fs::read(&path)
                && let Ok(entry) = serde_json::from_slice::<MemoryEntry>(&bytes)
                && entry.header.scope == *scope
                && path.file_stem().and_then(|stem| stem.to_str()) == Some(entry.header.id.as_str())
            {
                found.push(entry);
            }
        }
        Ok(found)
    }

    fn write_entry(&self, entry: &MemoryEntry) -> io::Result<()> {
        let bytes = serde_json::to_vec(entry).map_err(io::Error::other)?;
        let dir = self.memory_scope_dir(&entry.header.scope);
        self.write_atomic(&dir.join(format!("{}.json", entry.header.id)), &bytes)
    }

    /// The directory of an entry's prior versions.
    fn versions_dir(&self, scope: &MemoryScope, id: &str) -> PathBuf {
        self.memory_scope_dir(scope).join("versions").join(id)
    }

    fn proposals_dir(&self, scope: &MemoryScope) -> PathBuf {
        self.memory_scope_dir(scope).join("proposals")
    }

    /// The file of the proposal `id`, in whichever scope holds it.
    fn proposal_file(&self, id: &str) -> io::Result<Option<PathBuf>> {
        if !is_memory_id(id) {
            return Ok(None);
        }
        let scopes = match fs::read_dir(self.memories_root()) {
            Ok(scopes) => scopes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        for scope in scopes {
            let path = scope?.path().join("proposals").join(format!("{id}.json"));
            if path.exists() {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    /// Keeps `previous` as a prior version and drops the oldest past `[..] versions`.
    fn keep_version(&self, previous: &MemoryEntry) -> io::Result<()> {
        let keep = self.limits.memories.versions;
        let dir = self.versions_dir(&previous.header.scope, &previous.header.id);
        if keep.allows(1u64) {
            let bytes = serde_json::to_vec(previous).map_err(io::Error::other)?;
            self.write_atomic(
                &dir.join(format!("{}.json", previous.header.revision)),
                &bytes,
            )?;
        }
        let mut kept: BTreeMap<u64, PathBuf> = BTreeMap::new();
        for path in entry_files(&dir)? {
            if let Some(revision) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse().ok())
            {
                kept.insert(revision, path);
            }
        }
        while !keep.allows(kept.len()) {
            let Some((_, oldest)) = kept.pop_first() else {
                break;
            };
            fs::remove_file(oldest)?;
        }
        Ok(())
    }
}

/// The `.json` files directly in `dir`; none when it is not there.
fn entry_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            files.push(path);
        }
    }
    Ok(files)
}

impl MemoryStore for FileRegistry {
    fn memory_limits(&self) -> &MemoryLimits {
        &self.limits.memories
    }

    fn write_memory(
        &self,
        mut entry: MemoryEntry,
        base: Option<u64>,
    ) -> Result<MemoryEntry, MemoryError> {
        let limits = &self.limits.memories;
        entry.validate(limits).map_err(MemoryError::Invalid)?;
        entry.header.bytes = entry.size();
        let _lock = self.lock()?;
        let existing = self.memory(&entry.header.id)?;
        let present = self.scope_entries(&entry.header.scope)?;
        let bytes: u64 = present.iter().map(|one| one.header.bytes).sum();
        let count = present.len() as u64;
        match (existing, base) {
            (Some(current), None) => Err(MemoryError::Changed {
                revision: current.header.revision,
            }),
            (None, Some(_)) => Err(MemoryError::Missing),
            (None, None) => {
                room_for(&entry, count, bytes, None, limits)?;
                entry.header.revision = 1;
                self.write_entry(&entry)?;
                Ok(entry)
            }
            (Some(current), Some(base)) => {
                let next = replacing(entry, &current, base)?;
                room_for(&next, count, bytes, Some(current.header.bytes), limits)?;
                self.keep_version(&current)?;
                self.write_entry(&next)?;
                Ok(next)
            }
        }
    }

    fn memory(&self, id: &str) -> io::Result<Option<MemoryEntry>> {
        if !is_memory_id(id) {
            return Ok(None);
        }
        let scopes = match fs::read_dir(self.memories_root()) {
            Ok(scopes) => scopes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        for scope in scopes {
            let path = scope?.path().join(format!("{id}.json"));
            match fs::read(&path) {
                Ok(bytes) => {
                    let entry: MemoryEntry = serde_json::from_slice(&bytes)
                        .map_err(|error| invalid_data(format!("invalid entry: {error}")))?;
                    return Ok((entry.header.id == id).then_some(entry));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    fn memory_headers(&self, scope: &MemoryScope) -> io::Result<Vec<MemoryHeader>> {
        let mut headers: Vec<MemoryHeader> = self
            .scope_entries(scope)?
            .into_iter()
            .map(|entry| entry.header)
            .collect();
        newest(&mut headers);
        Ok(headers)
    }

    fn memory_candidates(
        &self,
        scopes: &[MemoryScope],
        _terms: &[String],
    ) -> io::Result<Vec<MemoryEntry>> {
        // No index here: the scan is [`matching`] itself.
        let mut found = Vec::new();
        for scope in scopes {
            found.extend(self.scope_entries(scope)?);
        }
        Ok(found)
    }

    fn memory_versions(&self, id: &str) -> io::Result<Vec<MemoryEntry>> {
        let Some(entry) = self.memory(id)? else {
            return Ok(Vec::new());
        };
        let mut versions = Vec::new();
        for path in entry_files(&self.versions_dir(&entry.header.scope, id))? {
            if let Ok(bytes) = fs::read(&path)
                && let Ok(version) = serde_json::from_slice::<MemoryEntry>(&bytes)
            {
                versions.push(version);
            }
        }
        versions.sort_by_key(|version| std::cmp::Reverse(version.header.revision));
        Ok(versions)
    }

    fn forget_memory(&self, id: &str, base: Option<u64>) -> Result<MemoryEntry, MemoryError> {
        let _lock = self.lock()?;
        let entry = self.memory(id)?.ok_or(MemoryError::Missing)?;
        if base.is_some_and(|base| base != entry.header.revision) {
            return Err(MemoryError::Changed {
                revision: entry.header.revision,
            });
        }
        let versions = self.versions_dir(&entry.header.scope, id);
        match fs::remove_dir_all(&versions) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        let dir = self.memory_scope_dir(&entry.header.scope);
        fs::remove_file(dir.join(format!("{id}.json")))?;
        Ok(entry)
    }

    fn propose_memory(&self, proposal: &MemoryProposal) -> Result<(), MemoryError> {
        let limits = &self.limits.memories;
        proposal.validate(limits).map_err(MemoryError::Invalid)?;
        let bytes = serde_json::to_vec(proposal).map_err(io::Error::other)?;
        let _lock = self.lock()?;
        let dir = self.proposals_dir(&proposal.scope);
        if !limits.proposals.allows(entry_files(&dir)?.len() + 1) {
            return Err(MemoryError::Full(PROPOSALS_FULL.to_owned()));
        }
        self.write_atomic(&dir.join(format!("{}.json", proposal.id)), &bytes)?;
        Ok(())
    }

    fn memory_proposals(&self, scope: &MemoryScope) -> io::Result<Vec<MemoryProposal>> {
        let mut found = Vec::new();
        for path in entry_files(&self.proposals_dir(scope))? {
            if let Ok(bytes) = fs::read(&path)
                && let Ok(proposal) = serde_json::from_slice::<MemoryProposal>(&bytes)
                && proposal.scope == *scope
            {
                found.push(proposal);
            }
        }
        oldest(&mut found);
        Ok(found)
    }

    fn memory_proposal(&self, id: &str) -> io::Result<Option<MemoryProposal>> {
        let Some(path) = self.proposal_file(id)? else {
            return Ok(None);
        };
        let bytes = fs::read(path)?;
        let proposal: MemoryProposal = serde_json::from_slice(&bytes)
            .map_err(|error| invalid_data(format!("invalid proposal: {error}")))?;
        Ok((proposal.id == id).then_some(proposal))
    }

    fn drop_memory_proposal(&self, id: &str) -> io::Result<bool> {
        let _lock = self.lock()?;
        let Some(path) = self.proposal_file(id)? else {
            return Ok(false);
        };
        fs::remove_file(path)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounds::Limit;
    use crate::directorates::registry::backend::RegistryBackend;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        two_entries_may_share_a_name_and_a_summary,
        scopes_are_kept_apart,
        a_write_from_an_old_revision_is_refused,
        what_a_scope_may_hold_is_limited,
        a_search_ranks_a_match_over_a_non_match,
        a_search_finds_a_word_only_as_a_word,
        a_search_finds_every_word_at_any_length_of_query,
        prior_versions_are_kept_to_the_limit_and_go_with_a_forget,
        an_id_that_is_not_one_names_nothing,
        proposals_wait_per_scope_within_their_limit,
    );

    fn person(id: &str) -> MemoryScope {
        MemoryScope::Person(id.to_owned())
    }

    fn entry(scope: MemoryScope, name: Option<&str>, summary: &str, content: &str) -> MemoryEntry {
        MemoryEntry::new(
            scope,
            name.map(str::to_owned),
            summary,
            content,
            MemorySource::new("person:ana", None),
            10.0,
        )
    }

    fn found(registry: &dyn RegistryBackend, scopes: &[MemoryScope], query: &str) -> Vec<String> {
        let terms = search_terms(query);
        matching(registry.memory_candidates(scopes, &terms).unwrap(), &terms)
            .into_iter()
            .map(|entry| entry.header.id)
            .collect()
    }

    fn two_entries_may_share_a_name_and_a_summary(store: &dyn Store) {
        let registry = store.handle();
        let first = registry
            .write_memory(
                entry(person("ana"), Some("prefs"), "How she likes it", "tables"),
                None,
            )
            .unwrap();
        let second = registry
            .write_memory(
                entry(person("ana"), Some("prefs"), "How she likes it", "charts"),
                None,
            )
            .unwrap();
        assert_ne!(first.header.id, second.header.id);
        assert_eq!(first.header.revision, 1);
        assert_eq!(registry.memory_headers(&person("ana")).unwrap().len(), 2);
        assert_eq!(
            registry.memory(&second.header.id).unwrap().unwrap().content,
            "charts"
        );
        assert_eq!(
            registry.memory_headers(&person("ana")).unwrap()[0].bytes,
            5 + 16 + 6,
            "a header says its size and holds no content"
        );
        // Taking an ID already there changes nothing.
        let mut again = second.clone();
        again.content = "over it".to_owned();
        assert!(matches!(
            registry.write_memory(again, None),
            Err(MemoryError::Changed { revision: 1 })
        ));
        for bad in [
            entry(person("ana"), None, "", "x"),
            entry(person("ana"), None, "two\nlines", "x"),
            entry(person("ana"), Some(""), "s", "x"),
            entry(person("ana"), None, "s", "nul\0"),
        ] {
            assert!(
                matches!(
                    registry.write_memory(bad, None),
                    Err(MemoryError::Invalid(_))
                ),
                "out of bounds"
            );
        }
        let mut not_an_id = entry(person("ana"), None, "s", "x");
        not_an_id.header.id = "../../elsewhere".to_owned();
        assert!(matches!(
            registry.write_memory(not_an_id, None),
            Err(MemoryError::Invalid(_))
        ));
    }

    fn scopes_are_kept_apart(store: &dyn Store) {
        let registry = store.handle();
        let hers = registry
            .write_memory(entry(person("ana"), None, "ana's own", "x"), None)
            .unwrap();
        // Two people whose IDs differ only by case stay two.
        let other = registry
            .write_memory(entry(person("ANA"), None, "another's", "y"), None)
            .unwrap();
        let ops = registry
            .write_memory(
                entry(MemoryScope::Operation("ana".to_owned()), None, "op", "z"),
                None,
            )
            .unwrap();
        let everyone = registry
            .write_memory(entry(MemoryScope::Global, None, "all", "w"), None)
            .unwrap();
        for (scope, id) in [
            (person("ana"), &hers.header.id),
            (person("ANA"), &other.header.id),
            (MemoryScope::Operation("ana".to_owned()), &ops.header.id),
            (MemoryScope::Global, &everyone.header.id),
        ] {
            let listed = registry.memory_headers(&scope).unwrap();
            assert_eq!(
                listed.iter().map(|header| &header.id).collect::<Vec<_>>(),
                [id],
                "{scope}"
            );
        }
        assert!(
            registry
                .memory_headers(&person("nobody"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            found(registry.as_ref(), &[person("ana")], "ana"),
            std::slice::from_ref(&hers.header.id)
        );
        assert_eq!(
            found(registry.as_ref(), &[person("ANA")], "another"),
            std::slice::from_ref(&other.header.id)
        );
        assert!(found(registry.as_ref(), &[person("ana")], "another").is_empty());
        // An entry cannot be moved to another scope by naming its ID there.
        let mut moved = hers.clone();
        moved.header.scope = person("bob");
        assert!(matches!(
            registry.write_memory(moved, Some(1)),
            Err(MemoryError::Missing)
        ));
        assert_eq!(registry.memory(&hers.header.id).unwrap().unwrap(), hers);
        assert!(registry.memory_headers(&person("bob")).unwrap().is_empty());
    }

    fn a_write_from_an_old_revision_is_refused(store: &dyn Store) {
        let registry = store.handle();
        let first = registry
            .write_memory(entry(person("ana"), None, "s", "one"), None)
            .unwrap();
        let mut second = first.clone();
        second.content = "two".to_owned();
        second.header.created_at = 99.0;
        let second = registry.write_memory(second, Some(1)).unwrap();
        assert_eq!(second.header.revision, 2);
        assert_eq!(second.header.created_at, 10.0, "made when it was made");
        let mut late = first.clone();
        late.content = "late".to_owned();
        assert!(matches!(
            registry.write_memory(late, Some(1)),
            Err(MemoryError::Changed { revision: 2 })
        ));
        assert_eq!(
            registry.memory(&first.header.id).unwrap().unwrap().content,
            "two"
        );
        assert!(matches!(
            registry.forget_memory(&first.header.id, Some(1)),
            Err(MemoryError::Changed { revision: 2 })
        ));
        assert!(registry.memory(&first.header.id).unwrap().is_some());
        let gone = entry(person("ana"), None, "s", "gone");
        assert!(matches!(
            registry.write_memory(gone, Some(1)),
            Err(MemoryError::Missing)
        ));
    }

    fn what_a_scope_may_hold_is_limited(store: &dyn Store) {
        let registry = store.handle();
        let limits = registry.limits().memories.clone();
        // An entry over the content limit is refused for what it is.
        let huge = "x".repeat(limits.content_bytes.or_max() + 1);
        assert!(matches!(
            registry.write_memory(entry(person("ana"), None, "s", &huge), None),
            Err(MemoryError::Invalid(_))
        ));
        // Filling a scope to its total stops the next, and another scope is not touched.
        let large = "y".repeat(limits.content_bytes.or_max());
        let fits = limits.scope_bytes.or_max() / limits.content_bytes.or_max();
        let mut kept = Vec::new();
        for _ in 0..fits {
            let stored = registry.write_memory(entry(person("ana"), None, "s", &large), None);
            match stored {
                Ok(stored) => kept.push(stored),
                Err(MemoryError::Full(_)) => break,
                Err(error) => panic!("{error}"),
            }
        }
        assert!(
            matches!(
                registry.write_memory(entry(person("ana"), None, "s", &large), None),
                Err(MemoryError::Full(_))
            ),
            "the total stops it"
        );
        assert!(
            registry
                .write_memory(entry(person("bob"), None, "s", &large), None)
                .is_ok()
        );
        // A forget makes room.
        let first = kept.remove(0);
        registry.forget_memory(&first.header.id, None).unwrap();
        assert!(
            registry
                .write_memory(entry(person("ana"), None, "s", &large), None)
                .is_ok()
        );
    }

    fn a_search_ranks_a_match_over_a_non_match(store: &dyn Store) {
        let registry = store.handle();
        let scopes = [person("ana")];
        let write = |name: Option<&str>, summary: &str, content: &str| {
            registry
                .write_memory(entry(person("ana"), name, summary, content), None)
                .unwrap()
                .header
                .id
        };
        let none = write(None, "Nothing here", "unrelated words");
        let body = write(None, "Something else", "she prefers tables in reports");
        let title = write(None, "Prefers tables", "details below");
        let both = write(Some("tables"), "Reports", "she prefers tables");
        // All three hold both words; more of them in the name or summary ranks first.
        assert_eq!(
            found(registry.as_ref(), &scopes, "prefers tables"),
            [title.clone(), both.clone(), body.clone()]
        );
        let any = found(registry.as_ref(), &scopes, "Prefers ZEBRA");
        assert_eq!(any.len(), 3, "one word of two is a match: {any:?}");
        assert!(!any.contains(&none));
        assert!(found(registry.as_ref(), &scopes, "zebra").is_empty());
        // No words at all lists everything, newest first.
        assert_eq!(found(registry.as_ref(), &scopes, " ... ").len(), 4);
    }

    fn a_search_finds_a_word_only_as_a_word(store: &dyn Store) {
        let registry = store.handle();
        let scopes = [person("ana")];
        let hyphen = registry
            .write_memory(
                entry(
                    person("ana"),
                    None,
                    "Host",
                    "reach api-gateway.example.com:8443",
                ),
                None,
            )
            .unwrap()
            .header
            .id;
        let accent = registry
            .write_memory(entry(person("ana"), None, "Café", "Mañana"), None)
            .unwrap()
            .header
            .id;
        for query in ["gateway", "EXAMPLE", "api-gateway", "8443", "com"] {
            assert_eq!(
                found(registry.as_ref(), &scopes, query),
                std::slice::from_ref(&hyphen),
                "{query}"
            );
        }
        assert!(found(registry.as_ref(), &scopes, "gate").is_empty());
        assert_eq!(
            found(registry.as_ref(), &scopes, "mañana"),
            std::slice::from_ref(&accent)
        );
        assert_eq!(found(registry.as_ref(), &scopes, "CAFÉ"), [accent]);
        // Syntax of an index's own query language is only words.
        for query in ["\"", "a' | 'b", "NOT", "*", "col:x", "x OR", "(", "\\"] {
            let _ = found(registry.as_ref(), &scopes, query);
        }
    }

    fn a_search_finds_every_word_at_any_length_of_query(store: &dyn Store) {
        let registry = store.handle();
        let id = registry
            .write_memory(entry(person("ana"), None, "needle", "x"), None)
            .unwrap()
            .header
            .id;
        let many: Vec<String> = (0..2_000).map(|n| format!("w{n}")).collect();
        let query = format!("{} needle", many.join(" "));
        assert_eq!(found(registry.as_ref(), &[person("ana")], &query), [id]);
    }

    fn prior_versions_are_kept_to_the_limit_and_go_with_a_forget(store: &dyn Store) {
        let registry = store.handle();
        let keep = registry.limits().memories.versions.or_max();
        let mut current = registry
            .write_memory(entry(person("ana"), None, "s", "v1"), None)
            .unwrap();
        let other = registry
            .write_memory(entry(person("ana"), None, "s", "other"), None)
            .unwrap();
        let mut second = other.clone();
        second.content = "other two".to_owned();
        registry.write_memory(second, Some(1)).unwrap();
        for revision in 1..=(keep as u64 + 2) {
            let mut next = current.clone();
            next.content = format!("v{}", revision + 1);
            current = registry.write_memory(next, Some(revision)).unwrap();
        }
        let versions = registry.memory_versions(&current.header.id).unwrap();
        assert_eq!(versions.len(), keep);
        assert_eq!(
            versions[0].content,
            format!("v{}", keep + 2),
            "newest first"
        );
        assert_eq!(
            versions.last().unwrap().header.revision,
            current.header.revision - keep as u64
        );
        let forgotten = registry.forget_memory(&current.header.id, None).unwrap();
        assert_eq!(forgotten.header.id, current.header.id);
        assert!(registry.memory(&current.header.id).unwrap().is_none());
        assert!(
            registry
                .memory_versions(&current.header.id)
                .unwrap()
                .is_empty(),
            "nothing of it is left"
        );
        assert_eq!(
            registry.memory_versions(&other.header.id).unwrap().len(),
            1,
            "another entry keeps its own"
        );
        assert!(matches!(
            registry.forget_memory(&current.header.id, None),
            Err(MemoryError::Missing)
        ));
        // Nothing of it can be found by what it said.
        assert!(found(registry.as_ref(), &[person("ana")], "v3").is_empty());
    }

    fn an_id_that_is_not_one_names_nothing(store: &dyn Store) {
        let registry = store.handle();
        let written = registry
            .write_memory(entry(person("ana"), None, "s", "x"), None)
            .unwrap();
        for id in [
            "",
            "..",
            "../x",
            "not-a-uuid",
            written.header.id.to_uppercase().as_str(),
            written.header.id.replace('-', "").as_str(),
        ] {
            assert!(registry.memory(id).unwrap().is_none(), "{id}");
            assert!(registry.memory_versions(id).unwrap().is_empty(), "{id}");
            assert!(
                matches!(registry.forget_memory(id, None), Err(MemoryError::Missing)),
                "{id}"
            );
        }
        assert!(registry.memory(&written.header.id).unwrap().is_some());
    }

    fn proposal(scope: MemoryScope, summary: &str) -> MemoryProposal {
        MemoryProposal {
            id: Uuid::new_v4().to_string(),
            scope,
            target: None,
            forget: false,
            name: None,
            summary: summary.to_owned(),
            content: "text".to_owned(),
            compartments: None,
            source: MemorySource::new("agent:007", Some("G-1".to_owned()))
                .exposed_by(["tool:WebFetch".to_owned()]),
            at: 10.0,
        }
    }

    fn proposals_wait_per_scope_within_their_limit(store: &dyn Store) {
        let registry = store.handle();
        let operation = MemoryScope::Operation("G".to_owned());
        assert!(registry.memory_proposals(&operation).unwrap().is_empty());
        let first = proposal(operation.clone(), "first");
        registry.propose_memory(&first).unwrap();
        let mut later = proposal(operation.clone(), "later");
        later.at = 20.0;
        registry.propose_memory(&later).unwrap();
        registry
            .propose_memory(&proposal(MemoryScope::Global, "elsewhere"))
            .unwrap();
        let waiting = registry.memory_proposals(&operation).unwrap();
        assert_eq!(
            waiting
                .iter()
                .map(|one| one.summary.as_str())
                .collect::<Vec<_>>(),
            ["first", "later"]
        );
        assert_eq!(
            waiting[0].source.exposures,
            ["tool:WebFetch"],
            "its mark stays"
        );
        assert_eq!(registry.memory_proposal(&first.id).unwrap().unwrap(), first);
        assert!(registry.memory_proposal("not-an-id").unwrap().is_none());
        assert!(registry.drop_memory_proposal(&first.id).unwrap());
        assert!(!registry.drop_memory_proposal(&first.id).unwrap());
        assert!(registry.memory_proposal(&first.id).unwrap().is_none());
        assert_eq!(registry.memory_proposals(&operation).unwrap().len(), 1);

        let mut bad = proposal(operation.clone(), "two\nlines");
        assert!(matches!(
            registry.propose_memory(&bad),
            Err(MemoryError::Invalid(_))
        ));
        bad.summary = "s".to_owned();
        bad.target = Some(("../x".to_owned(), 1));
        assert!(matches!(
            registry.propose_memory(&bad),
            Err(MemoryError::Invalid(_))
        ));
        // Past the limit the next is refused, in that scope alone.
        let room = registry.limits().memories.proposals.or_max();
        for index in 1..room {
            registry
                .propose_memory(&proposal(operation.clone(), &format!("{index}")))
                .unwrap();
        }
        assert!(matches!(
            registry.propose_memory(&proposal(operation.clone(), "one too many")),
            Err(MemoryError::Full(_))
        ));
        assert!(
            registry
                .propose_memory(&proposal(MemoryScope::Global, "another scope"))
                .is_ok()
        );
    }

    #[test]
    fn a_file_registry_keeps_no_version_when_told_to_keep_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut limits = crate::directorates::registry::limits::RegistryLimits::default();
        limits.memories.versions = Limit::at(0);
        let registry = FileRegistry::open(dir.path()).with_limits(limits);
        let first = registry
            .write_memory(entry(person("ana"), None, "s", "one"), None)
            .unwrap();
        let mut next = first.clone();
        next.content = "two".to_owned();
        let next = registry.write_memory(next, Some(1)).unwrap();
        assert!(
            registry
                .memory_versions(&next.header.id)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_scope_written_as_text_reads_back_as_itself() {
        for (text, scope) in [
            ("person:ana", person("ana")),
            ("person:a:b", person("a:b")),
            ("operation:G", MemoryScope::Operation("G".to_owned())),
            ("global", MemoryScope::Global),
        ] {
            assert_eq!(MemoryScope::parse(text), Some(scope.clone()));
            assert_eq!(scope.to_string(), text);
            let json = serde_json::to_string(&scope).unwrap();
            assert_eq!(serde_json::from_str::<MemoryScope>(&json).unwrap(), scope);
        }
        for bad in [
            "",
            "person:",
            "operation:",
            "global:x",
            "station:hq",
            "person",
        ] {
            assert_eq!(MemoryScope::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_index_holds_each_word_once_within_its_bound() {
        let written = entry(
            person("ana"),
            Some("Tables"),
            "tables TABLES",
            "Charts, tables.",
        );
        assert_eq!(index_words(&written, Limit::UNBOUNDED), "charts tables ");
        assert_eq!(index_words(&written, Limit::at(7)), "charts ");
        assert_eq!(index_words(&written, Limit::at(0)), "");
    }
}
