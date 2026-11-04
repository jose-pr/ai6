//! Who may see and change a memory, asked at every call.
//!
//! Every read goes through this module and every one through `visible`, whether it is a read of
//! one entry, a listing, a search or a starting context, so what a caller cannot see is the same
//! everywhere. An entry the caller may not see answers as an ID that does not exist. A search
//! filters before it ranks or counts: nothing it answers rests on an entry it hid.
//!
//! An entry can be protected by compartments: to see it at all a caller needs the scope's read and
//! to be read into one of them, or to be a chief. A compartment that is unknown, or cannot be read,
//! reads nobody in. Only someone read into a compartment adds it to an entry or removes it, and
//! nobody hides an entry from themselves.
//!
//! An entry written by an exposed Session stays marked, and whoever reads it is told: a Session is
//! exposed by it, as by a Cable. A person's own edit clears the mark, audited. What an Agent may not
//! write at once becomes a [`MemoryProposal`] a person decides; it keeps who asked and their mark.
//!
//! Universal Exports asks as a person, and as Moneypenny for one; a Station asks as a Session. Each
//! says what it holds by implementing [`MemoryCaller`], read afresh for the call.

use std::collections::BTreeSet;
use std::io;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::bounds::Limit;
use crate::directorates::medical::exposure::memory_source;
use crate::directorates::registry::limits::MemoryLimits;
use crate::directorates::registry::memories::{
    MemoryEntry, MemoryError, MemoryHeader, MemoryProposal, MemoryScope, MemorySource, MemoryStore,
    matching, search_terms,
};

/// The audit action of a change to an entry.
pub const AUDIT_WRITE: &str = "memory.write";
/// The audit action of forgetting an entry.
pub const AUDIT_FORGET: &str = "memory.forget";
/// The audit action of a read of another person's memory.
pub const AUDIT_READ: &str = "memory.read";
/// The audit action of a change to an entry's compartments.
pub const AUDIT_COMPARTMENTS: &str = "memory.compartments";
/// The audit action of a person's edit that cleared an entry's mark.
pub const AUDIT_MARK_CLEARED: &str = "memory.mark_cleared";
/// The audit action of a proposal kept for a person to decide.
pub const AUDIT_PROPOSED: &str = "memory.proposed";
/// The audit action of a person approving a proposal.
pub const AUDIT_APPROVED: &str = "memory.proposal.approved";
/// The audit action of a person rejecting a proposal.
pub const AUDIT_REJECTED: &str = "memory.proposal.rejected";
/// The audit resource of a person's memory.
pub const PERSON_RESOURCE: &str = "config:assistant";
/// The audit resource of the global memory, where `memory.write` is granted.
pub const GLOBAL_RESOURCE: &str = "config:memory";

/// What a caller may do to a scope's entries.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryWriteRight {
    /// Change them at once.
    Write,
    /// Ask a person to.
    Propose,
    /// Nothing.
    Refused,
}

/// Who is asking, and what they hold now. Implementations read what they hold for this call and
/// fail closed: what cannot be read is not held.
pub trait MemoryCaller {
    /// Who a write is recorded as: [`MemorySource::by`].
    fn by(&self) -> String;

    /// The Session or conversation a write comes from.
    fn session(&self) -> Option<String>;

    /// Whether the caller has the scope's read.
    fn reads_scope(&self, scope: &MemoryScope) -> bool;

    /// What the caller may do to the scope's entries.
    fn writes_scope(&self, scope: &MemoryScope) -> MemoryWriteRight;

    /// Whether the caller is read into `compartment`. One that is unknown or cannot be read
    /// answers `false`.
    fn read_in(&self, compartment: &str) -> bool;

    /// Whether the caller sees an entry whatever its compartments: a chief.
    fn sees_all_compartments(&self) -> bool;

    /// Whether a person is acting for themselves, so an edit clears a mark and a proposal can be
    /// decided: never a Session, nor Moneypenny for a person.
    fn acts_in_person(&self) -> bool;

    /// How what is asking has been exposed, which a write it makes carries.
    fn exposures(&self) -> Vec<String>;

    /// Tells the caller it was shown what an exposed Session wrote; `source` says which.
    fn saw_marked(&self, source: &str);

    /// Bytes of one exposure source (`[registry.limits.sessions] exposure_source_bytes`).
    fn mark_bytes(&self) -> Limit;

    /// The time now, as Unix seconds.
    fn now(&self) -> f64;

    /// Records `action` on `target` in `scope`, before it happens.
    ///
    /// # Errors
    ///
    /// Why it could not be recorded; nothing then happens.
    fn audit(&self, action: &str, scope: &MemoryScope, target: &str) -> Result<(), String>;
}

/// Whether `compartments` let `caller` see what they protect.
fn admits(caller: &dyn MemoryCaller, compartments: &[String]) -> bool {
    compartments.is_empty()
        || caller.sees_all_compartments()
        || compartments
            .iter()
            .any(|compartment| caller.read_in(compartment))
}

/// The one question every read asks: whether `caller` may know of this entry at all.
fn visible(caller: &dyn MemoryCaller, header: &MemoryHeader) -> bool {
    caller.reads_scope(&header.scope) && admits(caller, &header.compartments)
}

/// Tells `caller` of each of `headers` an exposed Session wrote.
fn note_marks<'a>(caller: &dyn MemoryCaller, headers: impl Iterator<Item = &'a MemoryHeader>) {
    for header in headers.filter(|header| header.source.exposed()) {
        caller.saw_marked(&memory_source(&header.id, caller.mark_bytes()));
    }
}

/// Why a call did nothing, in the words the caller sees. [`Missing`](Self::Missing) is one answer
/// for an entry that is not there and one the caller may not see.
#[non_exhaustive]
#[derive(Debug)]
pub enum MemoryRefusal {
    /// There is no such entry, as far as the caller may know.
    Missing,
    /// The caller may not do this.
    Forbidden(String),
    /// What was asked is out of bounds.
    Invalid(String),
    /// The scope holds as much as it may.
    Full(String),
    /// The entry has moved on from the revision the write was made from.
    Changed {
        /// The revision it is at.
        revision: u64,
    },
    /// The store or the audit log failed.
    Failed(String),
}

impl std::fmt::Display for MemoryRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => formatter.write_str("There is no such entry."),
            Self::Forbidden(reason)
            | Self::Invalid(reason)
            | Self::Full(reason)
            | Self::Failed(reason) => formatter.write_str(reason),
            Self::Changed { revision } => write!(
                formatter,
                "The entry changed since it was read, so nothing was saved: it is at revision \
                 {revision}. Read it again and make the change from what it holds now."
            ),
        }
    }
}

impl std::error::Error for MemoryRefusal {}

impl From<io::Error> for MemoryRefusal {
    fn from(error: io::Error) -> Self {
        Self::Failed(format!("The memory could not be reached: {error}"))
    }
}

impl From<MemoryError> for MemoryRefusal {
    fn from(error: MemoryError) -> Self {
        match error {
            MemoryError::Invalid(reason) => Self::Invalid(reason),
            MemoryError::Full(reason) => Self::Full(reason),
            MemoryError::Missing => Self::Missing,
            MemoryError::Changed { revision } => Self::Changed { revision },
            MemoryError::Io(error) => error.into(),
        }
    }
}

/// The entry `id`, when the caller may see it.
///
/// # Errors
///
/// When the store cannot be read.
pub fn read_entry(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    id: &str,
) -> Result<MemoryEntry, MemoryRefusal> {
    let entry = find(store, caller, id)?;
    note_marks(caller, std::iter::once(&entry.header));
    Ok(entry)
}

/// The entry `id`, when the caller may see it, without telling them of a mark.
fn find(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    id: &str,
) -> Result<MemoryEntry, MemoryRefusal> {
    let entry = store.memory(id)?.ok_or(MemoryRefusal::Missing)?;
    if !visible(caller, &entry.header) {
        return Err(MemoryRefusal::Missing);
    }
    Ok(entry)
}

/// The prior versions of the entry `id`, newest first, when the caller may see the entry.
///
/// # Errors
///
/// When the store cannot be read.
pub fn prior_versions(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    id: &str,
) -> Result<Vec<MemoryEntry>, MemoryRefusal> {
    let versions = {
        read_entry(store, caller, id)?;
        store.memory_versions(id)?
    };
    note_marks(caller, versions.iter().map(|version| &version.header));
    Ok(versions)
}

/// What the caller may see of `scope`, newest change first.
///
/// # Errors
///
/// When the store cannot be read.
pub fn list_entries(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    scope: &MemoryScope,
) -> Result<Vec<MemoryHeader>, MemoryRefusal> {
    if !caller.reads_scope(scope) {
        return Ok(Vec::new());
    }
    let listed: Vec<MemoryHeader> = store
        .memory_headers(scope)?
        .into_iter()
        .filter(|header| visible(caller, header))
        .collect();
    note_marks(caller, listed.iter());
    Ok(listed)
}

/// What a search found.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySearch {
    /// The entries it answers, best first, at most `search_results`.
    pub hits: Vec<MemoryHeader>,
    /// How many the caller may see matched, whether or not all are in `hits`.
    pub total: usize,
}

/// Searches `scopes` for the words of `query`: what the caller may see, then ranked, then counted,
/// then cut to `limits.search_results`. With no words it is every entry, newest change first.
///
/// # Errors
///
/// A query past `limits.query_bytes`, or a store that cannot be read.
pub fn search_entries(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    scopes: &[MemoryScope],
    query: &str,
    limits: &MemoryLimits,
) -> Result<MemorySearch, MemoryRefusal> {
    if !limits.query_bytes.allows(query.len()) {
        return Err(MemoryRefusal::Invalid(format!(
            "A search asks for at most {} bytes.",
            limits.query_bytes
        )));
    }
    let readable: Vec<MemoryScope> = scopes
        .iter()
        .filter(|scope| caller.reads_scope(scope))
        .cloned()
        .collect();
    let terms = search_terms(query);
    let seen: Vec<MemoryEntry> = store
        .memory_candidates(&readable, &terms)?
        .into_iter()
        .filter(|entry| visible(caller, &entry.header))
        .collect();
    let ranked = matching(seen, &terms);
    let total = ranked.len();
    let hits: Vec<MemoryHeader> = ranked
        .into_iter()
        .take(limits.search_results.or_max())
        .map(|entry| entry.header)
        .collect();
    note_marks(caller, hits.iter());
    Ok(MemorySearch { hits, total })
}

/// What a write changes.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryTarget {
    /// A new entry of this scope.
    New(MemoryScope),
    /// The entry `id`, made from `revision`.
    Existing {
        /// Its ID.
        id: String,
        /// The revision the write was made from.
        revision: u64,
    },
}

/// A write to one entry.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryChange {
    /// What it changes.
    pub target: MemoryTarget,
    /// The entry's name; `None` leaves it without one.
    pub name: Option<String>,
    /// The entry's one-line summary.
    pub summary: String,
    /// The entry's whole new text.
    pub content: String,
    /// The compartments protecting it; `None` leaves them as they are, and a new entry without.
    pub compartments: Option<Vec<String>>,
    /// Who wrote it, when that is not the caller: the Agent whose proposal a person approved.
    source: Option<MemorySource>,
}

impl MemoryChange {
    /// A change of `target` to this name, summary and text.
    pub fn new(
        target: MemoryTarget,
        name: Option<String>,
        summary: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            target,
            name,
            summary: summary.into(),
            content: content.into(),
            compartments: None,
            source: None,
        }
    }

    /// This change setting `compartments`, when given.
    #[must_use]
    pub fn with_compartments(mut self, compartments: Option<Vec<String>>) -> Self {
        self.compartments = compartments;
        self
    }
}

/// A write that happened.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryWritten {
    /// The entry as stored.
    pub entry: MemoryEntry,
    /// The text it held before, for an entry that was there.
    pub before: Option<String>,
}

/// What a write came to.
#[derive(Debug, Clone, PartialEq)]
pub enum MemoryOutcome {
    /// It is kept.
    Kept(MemoryWritten),
    /// It waits for a person.
    Proposed(MemoryProposal),
}

/// What a forget came to.
#[derive(Debug, Clone, PartialEq)]
pub enum MemoryForgotten {
    /// It is gone, with its prior versions.
    Forgotten(MemoryEntry),
    /// It waits for a person.
    Proposed(MemoryProposal),
}

/// Refuses a change of an entry's compartments from `before` to `after` the caller may not make:
/// adding or removing one it is not read into, or hiding the entry from itself. A chief makes any.
fn check_compartments(
    caller: &dyn MemoryCaller,
    before: &[String],
    after: &[String],
) -> Result<(), MemoryRefusal> {
    if caller.sees_all_compartments() {
        return Ok(());
    }
    let (before_set, after_set): (BTreeSet<&String>, BTreeSet<&String>) =
        (before.iter().collect(), after.iter().collect());
    if before_set
        .symmetric_difference(&after_set)
        .any(|compartment| !caller.read_in(compartment))
    {
        return Err(MemoryRefusal::Forbidden(
            "You add or remove only a compartment you are read into.".to_owned(),
        ));
    }
    if before_set != after_set && !admits(caller, after) {
        return Err(MemoryRefusal::Forbidden(
            "That would hide the entry from you.".to_owned(),
        ));
    }
    Ok(())
}

/// Makes `change` as the caller, audited first: kept at once, or kept as a proposal for a person
/// when that is all the caller may do. A person's own edit clears a mark, audited; anyone else's
/// keeps it, and adds how they were exposed.
///
/// # Errors
///
/// [`MemoryRefusal`]: an entry the caller may not see is [`MemoryRefusal::Missing`], a scope they
/// may not write or a compartment they may not set is [`MemoryRefusal::Forbidden`].
pub fn write_entry(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    change: MemoryChange,
) -> Result<MemoryOutcome, MemoryRefusal> {
    let at = caller.now();
    let (scope, current, revision) = match &change.target {
        MemoryTarget::New(scope) => (scope.clone(), None, None),
        MemoryTarget::Existing { id, revision } => {
            let current = find(store, caller, id)?;
            (current.header.scope.clone(), Some(current), Some(*revision))
        }
    };
    let right = caller.writes_scope(&scope);
    if right == MemoryWriteRight::Refused {
        return Err(MemoryRefusal::Forbidden(format!(
            "You may not change the memory of {scope}."
        )));
    }
    let before: Vec<String> = current
        .as_ref()
        .map(|entry| entry.header.compartments.clone())
        .unwrap_or_default();
    let after = change
        .compartments
        .clone()
        .unwrap_or_else(|| before.clone());
    check_compartments(caller, &before, &after)?;

    let marks = current
        .as_ref()
        .filter(|_| !caller.acts_in_person())
        .map(|entry| entry.header.source.exposures.clone())
        .unwrap_or_default();
    let mut exposures = caller.exposures();
    exposures.extend(marks);
    exposures.sort_unstable();
    exposures.dedup();
    let source = match change.source {
        Some(source) => source,
        None => MemorySource::new(caller.by(), caller.session()).exposed_by(exposures),
    };

    if right == MemoryWriteRight::Propose {
        let proposal = MemoryProposal {
            id: Uuid::new_v4().to_string(),
            scope: scope.clone(),
            target: current
                .as_ref()
                .map(|entry| entry.header.id.clone())
                .zip(revision),
            forget: false,
            name: change.name,
            summary: change.summary,
            content: change.content,
            compartments: change.compartments,
            source,
            at,
        };
        return propose(store, caller, proposal);
    }

    let next = match &current {
        Some(entry) => MemoryEntry::with_id(
            entry.header.id.clone(),
            scope.clone(),
            change.name,
            change.summary,
            change.content,
            source,
            at,
        ),
        None => MemoryEntry::new(
            scope.clone(),
            change.name,
            change.summary,
            change.content,
            source,
            at,
        ),
    }
    .behind(after.clone());
    // Who wrote what it holds may be the Agent whose proposal this carries out; who changed it last
    // is always the caller.
    let mut next = next;
    next.header.updated_by = caller.by();
    next.validate(store.memory_limits())
        .map_err(MemoryRefusal::Invalid)?;
    let id = next.header.id.clone();
    caller
        .audit(AUDIT_WRITE, &scope, &audit_target(&id, caller))
        .map_err(MemoryRefusal::Failed)?;
    if before != next.header.compartments {
        caller
            .audit(
                AUDIT_COMPARTMENTS,
                &scope,
                &format!("{id} [{}] to [{}]", before.join(","), after.join(",")),
            )
            .map_err(MemoryRefusal::Failed)?;
    }
    if current
        .as_ref()
        .is_some_and(|entry| entry.header.source.exposed())
        && !next.header.source.exposed()
    {
        caller
            .audit(AUDIT_MARK_CLEARED, &scope, &id)
            .map_err(MemoryRefusal::Failed)?;
    }
    let entry = store.write_memory(next, revision)?;
    Ok(MemoryOutcome::Kept(MemoryWritten {
        entry,
        before: current.map(|entry| entry.content),
    }))
}

/// Keeps `proposal`, audited first.
fn propose(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    proposal: MemoryProposal,
) -> Result<MemoryOutcome, MemoryRefusal> {
    caller
        .audit(
            AUDIT_PROPOSED,
            &proposal.scope,
            &audit_target(&proposal.id, caller),
        )
        .map_err(MemoryRefusal::Failed)?;
    store.propose_memory(&proposal)?;
    Ok(MemoryOutcome::Proposed(proposal))
}

/// Forgets the entry `id` as the caller, audited first; `revision`, when given, is the one it was
/// read at. Its prior versions go with it. Where the caller may only propose, it is a proposal.
///
/// # Errors
///
/// As [`write_entry`].
pub fn forget_entry(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    id: &str,
    revision: Option<u64>,
) -> Result<MemoryForgotten, MemoryRefusal> {
    let current = find(store, caller, id)?;
    let scope = current.header.scope.clone();
    match caller.writes_scope(&scope) {
        MemoryWriteRight::Refused => Err(MemoryRefusal::Forbidden(format!(
            "You may not change the memory of {scope}."
        ))),
        MemoryWriteRight::Propose => {
            let proposal = MemoryProposal {
                id: Uuid::new_v4().to_string(),
                scope,
                target: Some((
                    current.header.id.clone(),
                    revision.unwrap_or(current.header.revision),
                )),
                forget: true,
                name: current.header.name.clone(),
                summary: current.header.summary.clone(),
                content: String::new(),
                compartments: None,
                source: MemorySource::new(caller.by(), caller.session())
                    .exposed_by(caller.exposures()),
                at: caller.now(),
            };
            match propose(store, caller, proposal)? {
                MemoryOutcome::Proposed(proposal) => Ok(MemoryForgotten::Proposed(proposal)),
                MemoryOutcome::Kept(_) => Err(MemoryRefusal::Failed(
                    "It was neither forgotten nor proposed.".to_owned(),
                )),
            }
        }
        MemoryWriteRight::Write => {
            caller
                .audit(AUDIT_FORGET, &scope, &audit_target(id, caller))
                .map_err(MemoryRefusal::Failed)?;
            Ok(MemoryForgotten::Forgotten(
                store.forget_memory(id, revision)?,
            ))
        }
    }
}

/// Whether `caller` may know of `proposal`: the scope's read and the compartments it would set, or
/// those of the entry it changes.
fn proposal_visible(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    proposal: &MemoryProposal,
) -> bool {
    if !caller.reads_scope(&proposal.scope) {
        return false;
    }
    let held = proposal
        .target
        .as_ref()
        .and_then(|(id, _)| store.memory(id).ok().flatten())
        .map(|entry| entry.header.compartments)
        .unwrap_or_default();
    admits(caller, &held) && admits(caller, proposal.compartments.as_deref().unwrap_or_default())
}

/// What waits in `scope` for a person like `caller` to decide, oldest first: nothing for anyone
/// but a person who may change the scope, and only what they may see.
///
/// # Errors
///
/// When the store cannot be read.
pub fn list_proposals(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    scope: &MemoryScope,
) -> Result<Vec<MemoryProposal>, MemoryRefusal> {
    if !caller.acts_in_person() || caller.writes_scope(scope) != MemoryWriteRight::Write {
        return Ok(Vec::new());
    }
    Ok(store
        .memory_proposals(scope)?
        .into_iter()
        .filter(|proposal| proposal_visible(store, caller, proposal))
        .collect())
}

/// Decides the proposal `id` as a person who may change its scope, audited first: approved, it is
/// carried out with who asked as its source and their mark on it, and the entry it wrote answers;
/// rejected, it is dropped. A proposal the caller may not see answers as one that does not exist,
/// and an approval is held to what the person may do: a compartment they are not read into is
/// refused. One whose entry has moved on is refused and left for them to reject.
///
/// # Errors
///
/// [`MemoryRefusal`].
pub fn decide_proposal(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    id: &str,
    approve: bool,
) -> Result<Option<MemoryEntry>, MemoryRefusal> {
    let proposal = store.memory_proposal(id)?.ok_or(MemoryRefusal::Missing)?;
    if !caller.acts_in_person() || !proposal_visible(store, caller, &proposal) {
        return Err(MemoryRefusal::Missing);
    }
    if caller.writes_scope(&proposal.scope) != MemoryWriteRight::Write {
        return Err(MemoryRefusal::Forbidden(format!(
            "You may not change the memory of {}.",
            proposal.scope
        )));
    }
    let scope = proposal.scope.clone();
    if !approve {
        caller
            .audit(AUDIT_REJECTED, &scope, id)
            .map_err(MemoryRefusal::Failed)?;
        store.drop_memory_proposal(id)?;
        return Ok(None);
    }
    caller
        .audit(AUDIT_APPROVED, &scope, id)
        .map_err(MemoryRefusal::Failed)?;
    let written = if proposal.forget {
        let (entry, revision) = proposal.target.as_ref().ok_or(MemoryRefusal::Missing)?;
        forget_entry(store, caller, entry, Some(*revision))?;
        None
    } else {
        let target = match proposal.target {
            Some((id, revision)) => MemoryTarget::Existing { id, revision },
            None => MemoryTarget::New(scope),
        };
        let change = MemoryChange {
            target,
            name: proposal.name,
            summary: proposal.summary,
            content: proposal.content,
            compartments: proposal.compartments,
            source: Some(proposal.source),
        };
        match write_entry(store, caller, change)? {
            MemoryOutcome::Kept(written) => Some(written.entry),
            MemoryOutcome::Proposed(_) => None,
        }
    };
    store.drop_memory_proposal(id)?;
    Ok(written)
}

/// What an audit record names: the entry, and the Session or conversation it came from.
fn audit_target(id: &str, caller: &dyn MemoryCaller) -> String {
    match caller.session() {
        Some(session) => format!("{id} from {session}"),
        None => id.to_owned(),
    }
}

/// `text` on one line within `limit` bytes, for an audit record.
pub fn bounded(text: &str, limit: Limit) -> String {
    let line: String = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let mut end = line.len().min(limit.or_max());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_owned()
}

/// `value` fenced as data for a model: `<` is escaped inside the JSON, so nothing in an entry can
/// close the element.
pub fn fenced(tool: &str, value: &Value) -> String {
    let body = serde_json::to_string_pretty(value)
        .unwrap_or_default()
        .replace('<', "\\u003c");
    format!("<ai6-data tool=\"{tool}\">\n{body}\n</ai6-data>")
}

/// A proposal as routes show it.
pub fn proposal_json(proposal: &MemoryProposal) -> Value {
    json!({
        "id": proposal.id,
        "status": "proposed",
        "scope": proposal.scope.to_string(),
        "entry": proposal.target.as_ref().map(|(id, _)| id),
        "revision": proposal.target.as_ref().map(|(_, revision)| revision),
        "forget": proposal.forget,
        "name": proposal.name,
        "summary": proposal.summary,
        "content": proposal.content,
        "compartments": proposal.compartments,
        "source": {
            "by": proposal.source.by,
            "session": proposal.source.session,
            "exposed": proposal.source.exposed(),
            "exposures": proposal.source.exposures,
        },
        "at": proposal.at,
    })
}

/// An entry's header as tools, routes and a starting context show it: never its content.
pub fn header_json(header: &MemoryHeader) -> Value {
    json!({
        "id": header.id,
        "scope": header.scope.to_string(),
        "name": header.name,
        "summary": header.summary,
        "created_at": header.created_at,
        "updated_at": header.updated_at,
        "updated_by": header.updated_by,
        "source": {
            "by": header.source.by,
            "session": header.source.session,
            "exposed": header.source.exposed(),
            "exposures": header.source.exposures,
        },
        "compartments": header.compartments,
        "revision": header.revision,
        "bytes": header.bytes,
    })
}

/// An entry as tools and routes show it: its header and its text.
pub fn entry_json(entry: &MemoryEntry) -> Value {
    let mut value = header_json(&entry.header);
    value["content"] = Value::from(entry.content.as_str());
    value
}

/// What a model is given of `headers`, fenced as data, within `bytes`: the newest that fit, and how
/// many did not. `None` when there is nothing to say.
pub fn summaries_for_a_model(
    headers: &[MemoryHeader],
    bytes: Limit,
    fence: impl Fn(&Value) -> String,
) -> Option<String> {
    if headers.is_empty() || bytes == Limit::at(0) {
        return None;
    }
    let brief = |header: &MemoryHeader| {
        json!({
            "id": header.id,
            "scope": header.scope.to_string(),
            "name": header.name,
            "summary": header.summary,
            "revision": header.revision,
            "exposed": header.source.exposed(),
        })
    };
    let mut told: Vec<Value> = Vec::new();
    let mut said = None;
    for header in headers {
        told.push(brief(header));
        let more = headers.len() - told.len();
        let text = fence(&json!({"entries": told, "not_shown": more}));
        if !bytes.allows(text.len()) {
            told.pop();
            break;
        }
        said = Some(text);
    }
    said
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    use super::*;
    use crate::directorates::registry::store::FileRegistry;

    /// A caller holding exactly what a test gives it, recording what it audits and is shown.
    #[derive(Default)]
    struct Holds {
        reads: BTreeSet<String>,
        writes: BTreeSet<String>,
        proposes: BTreeSet<String>,
        read_in: BTreeSet<String>,
        chief: bool,
        agent: bool,
        exposed: Vec<String>,
        seen: RefCell<Vec<String>>,
        audited: RefCell<Vec<(String, String)>>,
        audit_fails: bool,
    }

    impl Holds {
        fn of(scope: &MemoryScope) -> Self {
            Self {
                reads: [scope.to_string()].into(),
                writes: [scope.to_string()].into(),
                ..Self::default()
            }
        }
    }

    impl MemoryCaller for Holds {
        fn by(&self) -> String {
            "person:ana".to_owned()
        }

        fn session(&self) -> Option<String> {
            None
        }

        fn reads_scope(&self, scope: &MemoryScope) -> bool {
            self.reads.contains(&scope.to_string())
        }

        fn writes_scope(&self, scope: &MemoryScope) -> MemoryWriteRight {
            if self.writes.contains(&scope.to_string()) {
                MemoryWriteRight::Write
            } else if self.proposes.contains(&scope.to_string()) {
                MemoryWriteRight::Propose
            } else {
                MemoryWriteRight::Refused
            }
        }

        fn acts_in_person(&self) -> bool {
            !self.agent
        }

        fn exposures(&self) -> Vec<String> {
            let mut all = self.exposed.clone();
            all.extend(self.seen.borrow().iter().cloned());
            all
        }

        fn saw_marked(&self, source: &str) {
            self.seen.borrow_mut().push(source.to_owned());
        }

        fn mark_bytes(&self) -> Limit {
            Limit::at(128)
        }

        fn read_in(&self, compartment: &str) -> bool {
            self.read_in.contains(compartment)
        }

        fn sees_all_compartments(&self) -> bool {
            self.chief
        }

        fn now(&self) -> f64 {
            100.0
        }

        fn audit(&self, action: &str, scope: &MemoryScope, target: &str) -> Result<(), String> {
            if self.audit_fails {
                return Err("the log is gone".to_owned());
            }
            self.audited
                .borrow_mut()
                .push((action.to_owned(), format!("{scope} {target}")));
            Ok(())
        }
    }

    fn ana() -> MemoryScope {
        MemoryScope::Person("ana".to_owned())
    }

    fn bob() -> MemoryScope {
        MemoryScope::Person("bob".to_owned())
    }

    fn store() -> (tempfile::TempDir, FileRegistry) {
        let dir = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(dir.path());
        (dir, registry)
    }

    fn written(outcome: MemoryOutcome) -> MemoryWritten {
        match outcome {
            MemoryOutcome::Kept(written) => written,
            MemoryOutcome::Proposed(proposal) => panic!("proposed: {proposal:?}"),
        }
    }

    fn kept(outcome: MemoryOutcome) -> MemoryEntry {
        written(outcome).entry
    }

    fn global() -> MemoryScope {
        MemoryScope::Global
    }

    /// One reading the global memory and writing it, read into `compartments`.
    fn global_reader(compartments: &[&str]) -> Holds {
        Holds {
            reads: [global().to_string()].into(),
            writes: [global().to_string()].into(),
            read_in: compartments.iter().map(|name| (*name).to_owned()).collect(),
            ..Holds::default()
        }
    }

    fn new_in(scope: &MemoryScope, summary: &str, content: &str) -> MemoryChange {
        MemoryChange::new(MemoryTarget::New(scope.clone()), None, summary, content)
    }

    #[test]
    fn one_person_finds_nothing_of_anothers_by_any_read() {
        let (_dir, registry) = store();
        let theirs = kept(
            write_entry(
                &registry,
                &Holds::of(&bob()),
                new_in(&bob(), "bob's", "secret"),
            )
            .unwrap(),
        )
        .header
        .id;
        let caller = Holds::of(&ana());
        assert!(matches!(
            read_entry(&registry, &caller, &theirs),
            Err(MemoryRefusal::Missing)
        ));
        assert!(matches!(
            read_entry(&registry, &caller, "6f1e4a6a-9c55-4a0c-9d5c-2f8a0b6c1e11"),
            Err(MemoryRefusal::Missing)
        ));
        assert!(matches!(
            prior_versions(&registry, &caller, &theirs),
            Err(MemoryRefusal::Missing)
        ));
        assert!(list_entries(&registry, &caller, &bob()).unwrap().is_empty());
        let limits = MemoryLimits::default();
        let found = search_entries(&registry, &caller, &[bob()], "secret", &limits).unwrap();
        assert_eq!((found.hits.len(), found.total), (0, 0));
        // Nor can it be changed or forgotten by naming its ID, and each answers as a missing one.
        let change = MemoryChange::new(
            MemoryTarget::Existing {
                id: theirs.clone(),
                revision: 1,
            },
            None,
            "mine now",
            "mine",
        );
        assert!(matches!(
            write_entry(&registry, &caller, change),
            Err(MemoryRefusal::Missing)
        ));
        assert!(matches!(
            forget_entry(&registry, &caller, &theirs, None),
            Err(MemoryRefusal::Missing)
        ));
        assert!(matches!(
            write_entry(&registry, &caller, new_in(&bob(), "into theirs", "x")),
            Err(MemoryRefusal::Forbidden(_))
        ));
        assert_eq!(registry.memory(&theirs).unwrap().unwrap().content, "secret");
        assert!(
            caller.audited.borrow().is_empty(),
            "nothing refused is recorded as done"
        );
    }

    #[test]
    fn a_write_is_on_the_record_before_it_happens_and_not_without_it() {
        let (_dir, registry) = store();
        let caller = Holds::of(&ana());
        let first = written(write_entry(&registry, &caller, new_in(&ana(), "s", "one")).unwrap());
        let id = first.entry.header.id.clone();
        assert_eq!(first.before, None);
        assert_eq!(
            caller.audited.borrow().as_slice(),
            [(AUDIT_WRITE.to_owned(), format!("person:ana {id}"))]
        );
        let change = MemoryChange::new(
            MemoryTarget::Existing {
                id: id.clone(),
                revision: 1,
            },
            None,
            "s",
            "two",
        );
        let silent = Holds {
            audit_fails: true,
            ..Holds::of(&ana())
        };
        assert!(matches!(
            write_entry(&registry, &silent, change.clone()),
            Err(MemoryRefusal::Failed(_))
        ));
        assert!(matches!(
            forget_entry(&registry, &silent, &id, None),
            Err(MemoryRefusal::Failed(_))
        ));
        assert_eq!(registry.memory(&id).unwrap().unwrap().content, "one");
        let again = written(write_entry(&registry, &caller, change).unwrap());
        assert_eq!(again.before.as_deref(), Some("one"));
        assert_eq!(again.entry.header.revision, 2);
        assert!(matches!(
            forget_entry(&registry, &caller, &id, Some(2)).unwrap(),
            MemoryForgotten::Forgotten(_)
        ));
        assert_eq!(
            caller.audited.borrow().last().unwrap(),
            &(AUDIT_FORGET.to_owned(), format!("person:ana {id}"))
        );
    }

    #[test]
    fn a_write_out_of_bounds_is_refused_before_it_is_recorded() {
        let (_dir, registry) = store();
        let caller = Holds::of(&ana());
        assert!(matches!(
            write_entry(&registry, &caller, new_in(&ana(), "two\nlines", "x")),
            Err(MemoryRefusal::Invalid(_))
        ));
        assert!(caller.audited.borrow().is_empty());
    }

    #[test]
    fn a_search_past_its_bound_is_refused_and_its_answer_is_cut_after_the_count() {
        let (_dir, registry) = store();
        let caller = Holds::of(&ana());
        for summary in ["needle one", "needle two", "needle three"] {
            write_entry(&registry, &caller, new_in(&ana(), summary, "x")).unwrap();
        }
        let limits = MemoryLimits {
            search_results: Limit::at(2),
            query_bytes: Limit::at(8),
            ..MemoryLimits::default()
        };
        let found = search_entries(&registry, &caller, &[ana()], "needle", &limits).unwrap();
        assert_eq!((found.hits.len(), found.total), (2, 3));
        assert!(matches!(
            search_entries(&registry, &caller, &[ana()], "too long a query", &limits),
            Err(MemoryRefusal::Invalid(_))
        ));
    }

    #[test]
    fn a_model_is_given_summaries_within_their_bound_and_told_how_many_more() {
        let (_dir, registry) = store();
        let caller = Holds::of(&ana());
        for index in 0..5 {
            let mut entry = new_in(&ana(), &format!("summary {index}"), "the content itself");
            entry.name = Some("n".to_owned());
            write_entry(&registry, &caller, entry).unwrap();
        }
        let headers = list_entries(&registry, &caller, &ana()).unwrap();
        let fence = |value: &Value| value.to_string();
        let all = summaries_for_a_model(&headers, Limit::UNBOUNDED, fence).unwrap();
        assert!(all.contains("summary 0") && all.contains("summary 4"));
        assert!(!all.contains("the content itself"), "{all}");
        assert!(all.contains("\"not_shown\":0"), "{all}");
        let some = summaries_for_a_model(&headers, Limit::at(all.len() as u64 / 2), fence).unwrap();
        assert!(some.len() <= all.len() / 2);
        assert!(!some.contains("\"not_shown\":0"), "{some}");
        assert_eq!(summaries_for_a_model(&headers, Limit::at(10), fence), None);
        assert_eq!(summaries_for_a_model(&[], Limit::UNBOUNDED, fence), None);
    }

    #[test]
    fn a_protected_entry_answers_as_a_missing_id_to_who_is_not_read_in() {
        let (_dir, registry) = store();
        let holder = global_reader(&["KESTREL"]);
        let protected = kept(
            write_entry(
                &registry,
                &holder,
                new_in(&global(), "secret needle", "the secret")
                    .with_compartments(Some(vec!["KESTREL".to_owned()])),
            )
            .unwrap(),
        );
        assert_eq!(protected.header.compartments, ["KESTREL"]);
        kept(write_entry(&registry, &holder, new_in(&global(), "open needle", "x")).unwrap());
        let limits = MemoryLimits::default();

        for outsider in [global_reader(&[]), global_reader(&["OTHER"])] {
            let id = protected.header.id.as_str();
            let missing = "6f1e4a6a-9c55-4a0c-9d5c-2f8a0b6c1e11";
            for asked in [id, missing] {
                assert!(matches!(
                    read_entry(&registry, &outsider, asked),
                    Err(MemoryRefusal::Missing)
                ));
                assert!(matches!(
                    prior_versions(&registry, &outsider, asked),
                    Err(MemoryRefusal::Missing)
                ));
                assert!(matches!(
                    forget_entry(&registry, &outsider, asked, None),
                    Err(MemoryRefusal::Missing)
                ));
                let change = MemoryChange::new(
                    MemoryTarget::Existing {
                        id: asked.to_owned(),
                        revision: 1,
                    },
                    None,
                    "taken",
                    "taken",
                );
                assert!(matches!(
                    write_entry(&registry, &outsider, change),
                    Err(MemoryRefusal::Missing)
                ));
            }
            let listed = list_entries(&registry, &outsider, &global()).unwrap();
            assert_eq!(listed.len(), 1);
            let found =
                search_entries(&registry, &outsider, &[global()], "needle", &limits).unwrap();
            assert_eq!((found.hits.len(), found.total), (1, 1), "nothing counts it");
            let found =
                search_entries(&registry, &outsider, &[global()], "secret", &limits).unwrap();
            assert_eq!(found.total, 0);
            assert!(outsider.audited.borrow().is_empty());
        }
        // A chief sees it, whatever it is read into.
        let chief = Holds {
            chief: true,
            ..global_reader(&[])
        };
        assert!(read_entry(&registry, &chief, &protected.header.id).is_ok());
        assert_eq!(
            search_entries(&registry, &chief, &[global()], "needle", &limits)
                .unwrap()
                .total,
            2
        );
    }

    #[test]
    fn only_one_read_in_sets_a_compartment_and_nobody_hides_an_entry_from_themselves() {
        let (_dir, registry) = store();
        let holder = global_reader(&["MINE"]);
        for names in [&["THEIRS"][..], &["NO-SUCH"][..], &["MINE", "THEIRS"][..]] {
            let change = new_in(&global(), "s", "c")
                .with_compartments(Some(names.iter().map(|name| (*name).to_owned()).collect()));
            assert!(
                matches!(
                    write_entry(&registry, &holder, change),
                    Err(MemoryRefusal::Forbidden(_))
                ),
                "{names:?}"
            );
        }
        assert!(holder.audited.borrow().is_empty());
        let entry = kept(
            write_entry(
                &registry,
                &holder,
                new_in(&global(), "s", "c").with_compartments(Some(vec!["MINE".to_owned()])),
            )
            .unwrap(),
        );
        assert_eq!(
            holder
                .audited
                .borrow()
                .iter()
                .filter(|(action, _)| action == AUDIT_COMPARTMENTS)
                .count(),
            1
        );
        // Another's compartment on it stays, and is neither added nor removed by this one.
        let both = entry
            .clone()
            .behind(["MINE".to_owned(), "THEIRS".to_owned()]);
        registry.write_memory(both, Some(1)).unwrap();
        let change = |names: &[&str], revision: u64| {
            MemoryChange::new(
                MemoryTarget::Existing {
                    id: entry.header.id.clone(),
                    revision,
                },
                None,
                "s",
                "edited",
            )
            .with_compartments(Some(names.iter().map(|name| (*name).to_owned()).collect()))
        };
        for names in [&["MINE"][..], &["THEIRS"][..], &[][..]] {
            assert!(
                matches!(
                    write_entry(&registry, &holder, change(names, 2)),
                    Err(MemoryRefusal::Forbidden(_))
                ),
                "{names:?}"
            );
        }
        let kept_both = kept(
            write_entry(
                &registry,
                &holder,
                MemoryChange::new(
                    MemoryTarget::Existing {
                        id: entry.header.id.clone(),
                        revision: 2,
                    },
                    None,
                    "s",
                    "edited",
                ),
            )
            .unwrap(),
        );
        assert_eq!(kept_both.header.compartments, ["MINE", "THEIRS"]);
        // A chief sets any, a compartment it holds or not.
        let chief = Holds {
            chief: true,
            ..global_reader(&[])
        };
        assert!(write_entry(&registry, &chief, change(&["NO-SUCH"], 3)).is_ok());
    }

    #[test]
    fn a_mark_stays_through_an_agents_edit_and_goes_with_a_persons_audited() {
        let (_dir, registry) = store();
        let operation = MemoryScope::Operation("G".to_owned());
        let agent = Holds {
            agent: true,
            exposed: vec!["tool:WebFetch".to_owned()],
            ..Holds::of(&operation)
        };
        let poisoned =
            kept(write_entry(&registry, &agent, new_in(&operation, "s", "bad")).unwrap());
        assert_eq!(poisoned.header.source.exposures, ["tool:WebFetch"]);
        assert_eq!(poisoned.header.source.by, "person:ana");

        // A clean Agent reading it is told, and its own edit keeps the mark and adds what it saw.
        let reader = Holds {
            agent: true,
            ..Holds::of(&operation)
        };
        read_entry(&registry, &reader, &poisoned.header.id).unwrap();
        let mark = format!("memory:{}", poisoned.header.id);
        assert_eq!(reader.seen.borrow().as_slice(), std::slice::from_ref(&mark));
        let edit = MemoryChange::new(
            MemoryTarget::Existing {
                id: poisoned.header.id.clone(),
                revision: 1,
            },
            None,
            "s",
            "still bad",
        );
        let edited = kept(write_entry(&registry, &reader, edit).unwrap());
        assert_eq!(
            edited.header.source.exposures,
            [mark.clone(), "tool:WebFetch".to_owned()]
        );
        // A listing and a search tell a reader too, of what they show.
        let lister = Holds {
            agent: true,
            ..Holds::of(&operation)
        };
        list_entries(&registry, &lister, &operation).unwrap();
        assert_eq!(lister.seen.borrow().as_slice(), std::slice::from_ref(&mark));
        let searcher = Holds {
            agent: true,
            ..Holds::of(&operation)
        };
        search_entries(
            &registry,
            &searcher,
            std::slice::from_ref(&operation),
            "zebra",
            &MemoryLimits::default(),
        )
        .unwrap();
        assert!(
            searcher.seen.borrow().is_empty(),
            "nothing shown, nothing told"
        );

        // A person's edit clears it, on the record.
        let person = Holds::of(&operation);
        let edit = MemoryChange::new(
            MemoryTarget::Existing {
                id: poisoned.header.id.clone(),
                revision: 2,
            },
            None,
            "s",
            "checked",
        );
        let cleared = kept(write_entry(&registry, &person, edit).unwrap());
        assert!(!cleared.header.source.exposed());
        assert!(
            person
                .audited
                .borrow()
                .iter()
                .any(|(action, target)| action == AUDIT_MARK_CLEARED
                    && target.contains(&poisoned.header.id))
        );
    }

    #[test]
    fn a_proposal_keeps_its_source_and_only_a_person_who_may_write_decides_it() {
        let (_dir, registry) = store();
        let operation = MemoryScope::Operation("G".to_owned());
        let agent = Holds {
            agent: true,
            exposed: vec!["issue:github".to_owned()],
            reads: [operation.to_string()].into(),
            proposes: [operation.to_string()].into(),
            ..Holds::default()
        };
        let MemoryOutcome::Proposed(proposal) =
            write_entry(&registry, &agent, new_in(&operation, "worth it", "text")).unwrap()
        else {
            panic!("an Agent proposes");
        };
        assert!(registry.memory_headers(&operation).unwrap().is_empty());
        assert_eq!(proposal.source.exposures, ["issue:github"]);
        assert_eq!(
            agent.audited.borrow().as_slice(),
            [(
                AUDIT_PROPOSED.to_owned(),
                format!("operation:G {}", proposal.id)
            )]
        );

        // Not the Agent, nor a reader who may not write, nor one who may not see it.
        let reader = Holds {
            reads: [operation.to_string()].into(),
            ..Holds::default()
        };
        for who in [&agent, &reader] {
            assert!(
                list_proposals(&registry, who, &operation)
                    .unwrap()
                    .is_empty()
            );
            assert!(decide_proposal(&registry, who, &proposal.id, true).is_err());
        }
        let hidden = MemoryProposal::new(
            operation.clone(),
            None,
            None,
            "behind KESTREL",
            "text",
            Some(vec!["KESTREL".to_owned()]),
            MemorySource::new("agent:007", None),
            2.0,
        );
        registry.propose_memory(&hidden).unwrap();
        let person = Holds::of(&operation);
        let waiting = list_proposals(&registry, &person, &operation).unwrap();
        assert_eq!(
            waiting
                .iter()
                .map(|one| one.id.as_str())
                .collect::<Vec<_>>(),
            [proposal.id.as_str()]
        );
        assert!(matches!(
            decide_proposal(&registry, &person, &hidden.id, true),
            Err(MemoryRefusal::Missing)
        ));

        // Approved, it is kept with who asked as its source, still marked.
        let entry = decide_proposal(&registry, &person, &proposal.id, true)
            .unwrap()
            .unwrap();
        assert_eq!(entry.header.source.by, "person:ana");
        assert_eq!(entry.header.source.exposures, ["issue:github"]);
        assert_eq!(entry.header.updated_by, "person:ana");
        assert!(registry.memory_proposal(&proposal.id).unwrap().is_none());
        assert!(
            person
                .audited
                .borrow()
                .iter()
                .any(|(action, _)| action == AUDIT_APPROVED)
        );
        // Rejected, it goes and nothing is kept.
        let MemoryOutcome::Proposed(again) =
            write_entry(&registry, &agent, new_in(&operation, "no", "text")).unwrap()
        else {
            panic!("an Agent proposes");
        };
        assert_eq!(
            decide_proposal(&registry, &person, &again.id, false).unwrap(),
            None
        );
        assert!(registry.memory_proposal(&again.id).unwrap().is_none());
        assert_eq!(registry.memory_headers(&operation).unwrap().len(), 1);
        // A forget proposed by an Agent waits too.
        let MemoryForgotten::Proposed(forgetting) =
            forget_entry(&registry, &agent, &entry.header.id, None).unwrap()
        else {
            panic!("an Agent proposes a forget");
        };
        assert!(forgetting.forget);
        assert!(registry.memory(&entry.header.id).unwrap().is_some());
        decide_proposal(&registry, &person, &forgetting.id, true).unwrap();
        assert!(registry.memory(&entry.header.id).unwrap().is_none());
    }
}
