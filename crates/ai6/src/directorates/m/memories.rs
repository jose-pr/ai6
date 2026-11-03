//! Who may see and change a memory, asked at every call.
//!
//! Every read goes through this module and every one through [`visible`], whether it is a read of
//! one entry, a listing, a search or a starting context, so what a caller cannot see is the same
//! everywhere. An entry the caller may not see answers as an ID that does not exist. A search
//! filters before it ranks or counts: nothing it answers rests on an entry it hid.
//!
//! Universal Exports asks as a person, and as Moneypenny for one; a Station asks as a Session. Each
//! says what it holds by implementing [`MemoryCaller`], read afresh for the call.

use std::io;

use serde_json::{Value, json};

use crate::bounds::Limit;
use crate::directorates::registry::limits::MemoryLimits;
use crate::directorates::registry::memories::{
    MemoryEntry, MemoryError, MemoryHeader, MemoryScope, MemorySource, MemoryStore, matching,
    search_terms,
};

/// The audit action of a change to an entry, or the read of another person's.
pub const AUDIT_WRITE: &str = "memory.write";
/// The audit action of forgetting an entry.
pub const AUDIT_FORGET: &str = "memory.forget";
/// The audit action of a read of another person's memory.
pub const AUDIT_READ: &str = "memory.read";
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

    /// The time now, as Unix seconds.
    fn now(&self) -> f64;

    /// Records `action` on `target` in `scope`, before it happens.
    ///
    /// # Errors
    ///
    /// Why it could not be recorded; nothing then happens.
    fn audit(&self, action: &str, scope: &MemoryScope, target: &str) -> Result<(), String>;
}

/// The one question every read asks: whether `caller` may know of this entry at all.
fn visible(caller: &dyn MemoryCaller, header: &MemoryHeader) -> bool {
    caller.reads_scope(&header.scope)
        && (header.compartments.is_empty()
            || caller.sees_all_compartments()
            || header
                .compartments
                .iter()
                .any(|compartment| caller.read_in(compartment)))
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
    read_entry(store, caller, id)?;
    Ok(store.memory_versions(id)?)
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
    Ok(store
        .memory_headers(scope)?
        .into_iter()
        .filter(|header| visible(caller, header))
        .collect())
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
        }
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

/// Makes `change` as the caller, audited first.
///
/// # Errors
///
/// [`MemoryRefusal`]: an entry the caller may not see is [`MemoryRefusal::Missing`], a scope they
/// may not write is [`MemoryRefusal::Forbidden`].
pub fn write_entry(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    change: MemoryChange,
) -> Result<MemoryWritten, MemoryRefusal> {
    let source = MemorySource::new(caller.by(), caller.session());
    let at = caller.now();
    let (next, base, before) = match change.target {
        MemoryTarget::New(scope) => (
            MemoryEntry::new(
                scope,
                change.name,
                change.summary,
                change.content,
                source,
                at,
            ),
            None,
            None,
        ),
        MemoryTarget::Existing { id, revision } => {
            let current = read_entry(store, caller, &id)?;
            let next = MemoryEntry::with_id(
                id,
                current.header.scope.clone(),
                change.name,
                change.summary,
                change.content,
                source,
                at,
            )
            .behind(current.header.compartments.clone());
            (next, Some(revision), Some(current.content))
        }
    };
    let scope = next.header.scope.clone();
    if caller.writes_scope(&scope) != MemoryWriteRight::Write {
        return Err(MemoryRefusal::Forbidden(format!(
            "You may not change the memory of {scope}."
        )));
    }
    next.validate(store.memory_limits())
        .map_err(MemoryRefusal::Invalid)?;
    caller
        .audit(AUDIT_WRITE, &scope, &audit_target(&next.header.id, caller))
        .map_err(MemoryRefusal::Failed)?;
    let entry = store.write_memory(next, base)?;
    Ok(MemoryWritten { entry, before })
}

/// Forgets the entry `id` as the caller, audited first; `revision`, when given, is the one it was
/// read at. Its prior versions go with it.
///
/// # Errors
///
/// As [`write_entry`].
pub fn forget_entry(
    store: &dyn MemoryStore,
    caller: &dyn MemoryCaller,
    id: &str,
    revision: Option<u64>,
) -> Result<MemoryEntry, MemoryRefusal> {
    let current = read_entry(store, caller, id)?;
    let scope = current.header.scope;
    if caller.writes_scope(&scope) != MemoryWriteRight::Write {
        return Err(MemoryRefusal::Forbidden(format!(
            "You may not change the memory of {scope}."
        )));
    }
    caller
        .audit(AUDIT_FORGET, &scope, &audit_target(id, caller))
        .map_err(MemoryRefusal::Failed)?;
    Ok(store.forget_memory(id, revision)?)
}

/// What an audit record names: the entry, and the Session or conversation it came from.
fn audit_target(id: &str, caller: &dyn MemoryCaller) -> String {
    match caller.session() {
        Some(session) => format!("{id} from {session}"),
        None => id.to_owned(),
    }
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

    /// A caller holding exactly what a test gives it, recording what it audits.
    #[derive(Default)]
    struct Holds {
        reads: BTreeSet<String>,
        writes: BTreeSet<String>,
        read_in: BTreeSet<String>,
        chief: bool,
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
            } else {
                MemoryWriteRight::Refused
            }
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

    fn new_in(scope: &MemoryScope, summary: &str, content: &str) -> MemoryChange {
        MemoryChange::new(MemoryTarget::New(scope.clone()), None, summary, content)
    }

    #[test]
    fn one_person_finds_nothing_of_anothers_by_any_read() {
        let (_dir, registry) = store();
        let theirs = write_entry(
            &registry,
            &Holds::of(&bob()),
            new_in(&bob(), "bob's", "secret"),
        )
        .unwrap()
        .entry
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
        let written = write_entry(&registry, &caller, new_in(&ana(), "s", "one")).unwrap();
        let id = written.entry.header.id.clone();
        assert_eq!(written.before, None);
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
        let written = write_entry(&registry, &caller, change).unwrap();
        assert_eq!(written.before.as_deref(), Some("one"));
        assert_eq!(written.entry.header.revision, 2);
        forget_entry(&registry, &caller, &id, Some(2)).unwrap();
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
}
