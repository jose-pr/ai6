//! Missions' keys and handles: the one rule that names a brief's Mission where the brief enters
//! the Registry.
//!
//! A Mission, and each run of it, is keyed by a UID: a UUIDv7 in lower-case text, which every
//! Registry key, relation and route uses and which never changes. What people see is its
//! handle: the Operation and either a number from the Operation's counter or the tracker's code
//! (`ai6-42`, `ai6-nightly-43`, `ai6-gh-63`, `ai6-AI6-64`); a run's is `<handle>.<attempt>`. A
//! handle can be renamed, and nothing reads a code or a number back out of one.
//!
//! The Registry keeps a book of every handle it gave, current and former, each naming its
//! Mission's UID, with the [`MissionEntry`] it was given for when that entry can come again (a
//! re-polled item, a Sleeper's round, a review, a repeated approval), so the same entry always
//! names the same Mission, on any Station, however its brief moved since. A handle is never
//! given to a second Mission, and no two differ only by case, since a handle names a branch.

use std::io;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::limits::BriefLimits;
use super::store::hex_digest;

/// What stands for the Operation in the handle of a brief that names none.
const NO_OPERATION: &str = "mission";

/// Separates the parts of an entry's name in the book. Operation, instance and Sleeper names
/// hold no control character, and a SQL script can write this one with `chr(31)`.
const SEPARATOR: char = '\u{1f}';

/// The most bytes a handle can have, whatever `handle_bytes` says: it names a Git branch, whose
/// ref is a file, 255 bytes on every platform here, with room for a branch template's suffix.
const BRANCH_ROOM: usize = 240;

/// A Mission's key and the handle it shows.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionHandle {
    /// The Mission's UID: its Registry key, which every relation uses.
    pub mission_id: String,
    /// What the Mission is called now.
    pub handle: String,
}

impl MissionHandle {
    /// The Mission `mission_id`, called `handle`.
    pub fn new(mission_id: impl Into<String>, handle: impl Into<String>) -> Self {
        Self {
            mission_id: mission_id.into(),
            handle: handle.into(),
        }
    }
}

/// A new UID for a Mission or a run: a UUIDv7 in lower-case text, so UIDs sort by when they
/// were made.
pub fn new_uid() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Whether `text` is a UID as AI6 writes one: a UUID, hyphenated and in lower case.
pub fn is_uid(text: &str) -> bool {
    uuid::Uuid::parse_str(text).is_ok_and(|uid| uid.hyphenated().to_string() == text)
}

/// A run's handle: its Mission's handle when the run opened, and its attempt, `ai6-42.2`.
pub fn run_handle(mission_handle: &str, attempt: u32) -> String {
    format!("{mission_handle}.{attempt}")
}

/// Whether `handle` can name a Mission: only ASCII letters, digits, `_` and `-`, so it is valid
/// in a file name on any platform, in a Git branch and in a URL without escaping; neither
/// starting nor ending with `-`; within 240 bytes; and not a UID, which routes take as well.
pub fn valid_handle(handle: &str) -> bool {
    !handle.is_empty()
        && handle.len() <= BRANCH_ROOM
        && handle
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        && !handle.starts_with('-')
        && !handle.ends_with('-')
        && !is_uid(handle)
}

/// Checks a handle a person chose, within `[registry.limits.briefs] handle_bytes`.
///
/// # Errors
///
/// `InvalidInput`, saying what is wrong, for a handle [`valid_handle`] refuses or one longer
/// than the limit.
pub(super) fn check_handle(handle: &str, limits: &BriefLimits) -> io::Result<()> {
    if !valid_handle(handle) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a handle is ASCII letters, digits, '_' and '-', neither starting nor ending with \
             '-', and is not a UID",
        ));
    }
    if handle.len() > id_bytes(limits) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a handle is at most {} bytes", id_bytes(limits)),
        ));
    }
    Ok(())
}

/// Where a brief enters, which decides its Mission's handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionEntry {
    operation: Option<String>,
    kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EntryKind {
    Person,
    Proposal {
        id: String,
    },
    Review {
        id: String,
    },
    Sleeper {
        name: String,
        window: String,
        station: Option<String>,
    },
    Standing {
        name: String,
        station: Option<String>,
    },
    FollowUp {
        instance: String,
        item: String,
    },
    Source {
        instance: String,
        item: String,
        code: Option<String>,
        tag: Option<String>,
    },
}

impl MissionEntry {
    /// A person's brief, from `ai6ctl`, the app or Moneypenny: `<operation>-<n>`. Each entry is
    /// a new Mission.
    pub fn person(operation: Option<&str>) -> Self {
        Self::of(operation, EntryKind::Person)
    }

    /// The Mission an approved proposal becomes: `<operation>-<n>`, the same one however often
    /// it is approved.
    pub fn proposal(operation: &str, proposal_id: &str) -> Self {
        Self::of(
            Some(operation),
            EntryKind::Proposal {
                id: proposal_id.to_owned(),
            },
        )
    }

    /// A review brief: `<operation>-<n>`, the same one for the same review ID.
    pub fn review(operation: &str, review_id: &str) -> Self {
        Self::of(
            Some(operation),
            EntryKind::Review {
                id: review_id.to_owned(),
            },
        )
    }

    /// One round of a Sleeper: `<operation>-<sleeper>-<n>`, from the Operation's counter, the
    /// same one for the same window and Station.
    pub fn sleeper(operation: &str, name: &str, window: &str, station: Option<&str>) -> Self {
        Self::of(
            Some(operation),
            EntryKind::Sleeper {
                name: name.to_owned(),
                window: window.to_owned(),
                station: station.map(str::to_owned),
            },
        )
    }

    /// A standing Sleeper's one Mission, per Station when it holds a round on each:
    /// `<operation>-<sleeper>-<n>`, the same one for every window, each window an attempt of it.
    pub fn standing(operation: &str, name: &str, station: Option<&str>) -> Self {
        Self::of(
            Some(operation),
            EntryKind::Standing {
                name: name.to_owned(),
                station: station.map(str::to_owned),
            },
        )
    }

    /// Follow-up work on an item of a Source instance that already has a Mission: the item's
    /// Mission's handle now and a follow-up number from 2, `<operation>-<tag>-<code>-<k>`. Each
    /// entry is a new Mission; allotting one for an item with no Mission is refused.
    pub fn follow_up(operation: &str, instance: &str, item: &str) -> Self {
        Self::of(
            Some(operation),
            EntryKind::FollowUp {
                instance: instance.to_owned(),
                item: item.to_owned(),
            },
        )
    }

    /// An item of a Source instance: `<operation>-<tag>-<code>`, or `<operation>-<code>` for a
    /// code that names its own project, or `<operation>-<instance>-<item>` when the provider
    /// gives no code. `tag` and `code` count only the first time the item is seen: the item
    /// keeps the Mission it was given if the Operation's instances change later.
    pub fn source(
        operation: &str,
        instance: &str,
        item: &str,
        code: Option<&str>,
        tag: Option<&str>,
    ) -> Self {
        Self::of(
            Some(operation),
            EntryKind::Source {
                instance: instance.to_owned(),
                item: item.to_owned(),
                code: code.map(str::to_owned),
                tag: tag.map(str::to_owned),
            },
        )
    }

    fn of(operation: Option<&str>, kind: EntryKind) -> Self {
        Self {
            operation: operation.map(str::to_owned),
            kind,
        }
    }

    /// The entry's name in the book; `None` for an entry that never comes again.
    pub(super) fn book_name(&self) -> Option<String> {
        let operation = self.operation.as_deref().unwrap_or_default();
        let parts: Vec<&str> = match &self.kind {
            EntryKind::Person | EntryKind::FollowUp { .. } => return None,
            EntryKind::Proposal { id } => vec!["proposal", operation, id],
            EntryKind::Review { id } => vec!["review", operation, id],
            EntryKind::Sleeper {
                name,
                window,
                station,
            } => match station {
                Some(station) => vec!["sleeper", operation, name, window, station],
                None => vec!["sleeper", operation, name, window],
            },
            EntryKind::Standing { name, station } => match station {
                Some(station) => vec!["standing", operation, name, station],
                None => vec!["standing", operation, name],
            },
            EntryKind::Source { instance, item, .. } => vec!["source", operation, instance, item],
        };
        Some(parts.join(&SEPARATOR.to_string()))
    }

    /// The label the handle starts with, and the counter's name once folded.
    fn label(&self) -> String {
        let label = part(self.operation.as_deref().unwrap_or_default());
        if label.is_empty() {
            NO_OPERATION.to_owned()
        } else {
            label
        }
    }

    fn counted(&self) -> bool {
        !matches!(self.kind, EntryKind::Source { .. })
    }

    fn shaped(&self, number: Option<u64>) -> String {
        let label = self.label();
        let number = number.unwrap_or_default();
        match &self.kind {
            EntryKind::Person
            | EntryKind::Proposal { .. }
            | EntryKind::Review { .. }
            | EntryKind::FollowUp { .. } => format!("{label}-{number}"),
            EntryKind::Sleeper { name, .. } | EntryKind::Standing { name, .. } => {
                format!("{label}-{}-{number}", part(name))
            }
            EntryKind::Source {
                instance,
                item,
                code,
                tag,
            } => match (code, tag) {
                (Some(code), Some(tag)) => format!("{label}-{}-{}", part(tag), part(code)),
                (Some(code), None) => format!("{label}-{}", part(code)),
                (None, _) => format!("{label}-{}-{}", part(instance), part(item)),
            },
        }
    }
}

/// A name as it may stand in a handle: only ASCII letters, digits, `_` and `-`. A `.` goes too:
/// it is what makes `..`, a trailing `.lock` and a trailing dot, and it separates a run's
/// attempt.
fn part(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    let mut replacing = false;
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
            safe.push(character);
            replacing = false;
        } else if !replacing {
            safe.push('_');
            replacing = true;
        }
    }
    safe.trim_matches('-').to_owned()
}

/// `handle` within `bytes`: its start and a short hash of the whole of it, when it is longer.
fn fit(handle: String, bytes: usize) -> String {
    if handle.len() <= bytes {
        return handle;
    }
    let start = handle[..bytes.saturating_sub(9)].trim_end_matches('-');
    format!("{start}-{}", short_hash(&handle))
}

fn short_hash(text: &str) -> String {
    hex_digest(&Sha256::digest(text.as_bytes()))[..8].to_owned()
}

/// The longest handle `limits` allow.
fn id_bytes(limits: &BriefLimits) -> usize {
    limits.handle_bytes.or_max().min(BRANCH_ROOM)
}

/// A handle as the book compares it: two that differ only by case are one branch once fetched
/// on Windows or macOS.
pub(super) fn folded(handle: &str) -> String {
    handle.to_ascii_lowercase()
}

/// The name of an entry's file, for a backend that cannot hold it as a key.
pub(super) fn book_digest(name: &str) -> String {
    hex_digest(&Sha256::digest(name.as_bytes()))
}

/// What a backend keeps for the rule, read and written under its one writer's lock.
pub(super) trait MissionBook {
    /// The Mission an entry names, with the handle it goes by now.
    fn entered(&mut self, name: &str) -> io::Result<Option<MissionHandle>>;
    /// The UID of the Mission `handle` was given to, compared without case, current or former.
    fn holder(&mut self, handle: &str) -> io::Result<Option<String>>;
    /// The next number of the counter `name`, written before it is used, so it is never reused.
    fn next_number(&mut self, name: &str) -> io::Result<u64>;
    /// Records `mission`'s handle as given to it, and for the entry `name`, which then names
    /// this handle, when the entry can come again.
    fn issue(&mut self, mission: &MissionHandle, name: Option<&str>) -> io::Result<()>;
    /// The entry `handle` was given for, when it can come again.
    fn entry_of(&mut self, handle: &str) -> io::Result<Option<String>>;
}

/// Whether the book or a route could mistake `handle` for another Mission.
fn taken(book: &mut dyn MissionBook, handle: &str) -> io::Result<bool> {
    Ok(is_uid(handle) || book.holder(handle)?.is_some())
}

/// The Mission for `entry`: the one it already names, or a new UID with a handle by the rule.
///
/// A number another Mission's handle already reads as is skipped, and never used. A code whose
/// handle is taken, by `ai6-gh`'s Mission 63 for `ai6`'s GitHub item 63, adds a short hash of
/// the entry.
pub(super) fn allot(
    book: &mut dyn MissionBook,
    entry: &MissionEntry,
    limits: &BriefLimits,
) -> io::Result<MissionHandle> {
    if let EntryKind::FollowUp { instance, item } = &entry.kind {
        return follow_up(book, entry.operation.as_deref(), (instance, item), limits);
    }
    let name = entry.book_name();
    if let Some(name) = &name
        && let Some(entered) = book.entered(name)?
    {
        return Ok(entered);
    }
    let bytes = id_bytes(limits);
    let handle = if entry.counted() {
        let counter = folded(&fit(entry.label(), bytes));
        loop {
            let handle = fit(entry.shaped(Some(book.next_number(&counter)?)), bytes);
            if !taken(book, &handle)? {
                break handle;
            }
        }
    } else {
        let name = name.as_deref().unwrap_or_default();
        let handle = fit(entry.shaped(None), bytes);
        if taken(book, &handle)? {
            let handle = format!(
                "{}-{}",
                &handle[..handle.len().min(bytes.saturating_sub(9))],
                short_hash(name)
            );
            if taken(book, &handle)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("handle {handle} is taken by another entry"),
                ));
            }
            handle
        } else {
            handle
        }
    };
    let mission = MissionHandle::new(new_uid(), handle);
    book.issue(&mission, name.as_deref())?;
    Ok(mission)
}

/// A new Mission following up `item` of `instance`: the handle the item's Mission goes by now
/// and the item's next follow-up number, from 2 since the item's own Mission is its first.
fn follow_up(
    book: &mut dyn MissionBook,
    operation: Option<&str>,
    (instance, item): (&str, &str),
    limits: &BriefLimits,
) -> io::Result<MissionHandle> {
    let operation = operation.unwrap_or_default();
    let ticket = MissionEntry::source(operation, instance, item, None, None)
        .book_name()
        .unwrap_or_default();
    let base = book.entered(&ticket)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("item {item:?} of {instance:?} has no Mission in Operation {operation:?}"),
        )
    })?;
    // `.` is in no Operation's counter, so this one is the item's alone.
    let counter = format!("follow-up.{}", book_digest(&ticket));
    let bytes = id_bytes(limits);
    loop {
        let next = book.next_number(&counter)?.saturating_add(1);
        let handle = fit(format!("{}-{next}", base.handle), bytes);
        if !taken(book, &handle)? {
            let mission = MissionHandle::new(new_uid(), handle);
            book.issue(&mission, None)?;
            return Ok(mission);
        }
    }
}

/// Renames `mission`, which goes by `mission.handle` now, to `handle`. A handle another Mission
/// was ever given is refused; one of its own former handles can be taken back. The old handle
/// stays in the book, naming this Mission, and an entry that named it names the new one.
///
/// # Errors
///
/// `InvalidInput` for a malformed handle, `AlreadyExists` for one that is taken.
pub(super) fn rename(
    book: &mut dyn MissionBook,
    mission: &MissionHandle,
    handle: &str,
    limits: &BriefLimits,
) -> io::Result<MissionHandle> {
    check_handle(handle, limits)?;
    if book
        .holder(handle)?
        .is_some_and(|holder| holder != mission.mission_id)
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("handle {handle} is taken by another Mission"),
        ));
    }
    let entry = book.entry_of(&mission.handle)?;
    let renamed = MissionHandle::new(mission.mission_id.clone(), handle);
    book.issue(&renamed, entry.as_deref())?;
    Ok(renamed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::backend::*;
    use crate::directorates::registry::limits::RegistryLimits;
    use crate::directorates::registry::sessions::NewSession;
    use crate::directorates::registry::sources::{Ingest, SourceItem};
    use crate::directorates::registry::store::{
        Brief, DEFAULT_LEASE, FileRegistry, LeaseRecovery, MissionProposal, Reentry,
    };
    use crate::directorates::registry::suite::{Store, backend_tests, brief};
    use std::collections::{BTreeMap, BTreeSet};

    backend_tests!(
        concurrent_people_get_distinct_consecutive_numbers,
        each_entry_point_yields_its_shape,
        a_repeated_entry_keeps_its_mission_and_a_number_is_never_reused,
        a_re_polled_item_keeps_its_mission_and_creates_nothing,
        jira_and_github_codes_shape_the_handle,
        an_item_keeps_its_handle_when_its_instance_stops_being_the_only_one,
        odd_codes_become_valid_as_a_branch_a_path_and_a_url_part,
        operations_whose_handles_would_collide_never_share_one,
        a_brief_keeps_its_mission_through_a_requeue_and_a_hold,
        a_proposal_gets_one_number_however_often_it_is_approved,
        a_dropped_record_cannot_name_a_path,
        a_finished_brief_from_elsewhere_owes_no_report,
        a_rename_keeps_the_uid_and_every_handle_finds_it,
        a_rename_reaches_the_brief_in_every_state,
        a_taken_or_malformed_handle_is_refused,
        a_former_handle_is_never_given_again,
        a_renamed_item_keeps_its_new_handle_when_polled_again,
        run_handles_follow_the_current_handle_only_for_new_runs,
        follow_ups_take_the_items_handle_and_a_number_from_two,
        a_reopened_item_has_its_own_count_of_lapses,
        a_brief_never_reentered_keeps_its_count_of_lapses,
    );

    /// Claims the only queued brief as `attempt`, lets its lease lapse and recovers it under a
    /// limit of two claims.
    fn lapse_claim(
        registry: &dyn crate::directorates::registry::suite::TestRegistry,
        attempt: u32,
    ) -> LeaseRecovery {
        let lease = registry
            .claim_with_lease(None, "hq", DEFAULT_LEASE, BTreeMap::new())
            .unwrap()
            .expect("a brief is queued");
        assert_eq!(lease.attempt(), attempt);
        let lapsed = crate::directorates::registry::suite::lapse(registry, lease.key(), "hq");
        registry
            .recover(&lapsed, 2, "other")
            .unwrap()
            .expect("an expired lease is recovered")
    }

    fn a_reopened_item_has_its_own_count_of_lapses(store: &dyn Store) {
        let registry = store.handle();
        let item = source_item("o/r#7", Some("7"), Some("gh"));
        registry
            .ingest_source_item("G", "github", &item, "hq")
            .unwrap();
        assert_eq!(lapse_claim(registry.as_ref(), 1), LeaseRecovery::Requeued);
        let lease = registry
            .claim_with_lease(None, "hq", DEFAULT_LEASE, BTreeMap::new())
            .unwrap()
            .unwrap();
        assert_eq!(lease.attempt(), 2, "the first attempt spent its count");
        let claimed = registry.read_claimed(lease.key()).unwrap().unwrap();
        registry
            .finish(&claimed, &serde_json::json!({}), true, None, Some("hq"))
            .unwrap();

        let reopened = crate::directorates::registry::sources::reopened_brief(
            ("G", "github"),
            &item,
            &[],
            &Default::default(),
            "hq",
        );
        let entry = crate::directorates::registry::sources::source_entry("G", "github", &item);
        assert!(matches!(
            registry
                .reenter_brief(&reopened, &entry, &|_| true)
                .unwrap(),
            Reentry::Queued(_)
        ));
        assert_eq!(
            registry.filed_brief(lease.key()).unwrap().unwrap().attempts,
            0,
            "the next attempt has made no claim yet"
        );
        assert_eq!(
            lapse_claim(registry.as_ref(), 1),
            LeaseRecovery::Requeued,
            "its own full count"
        );
        assert_eq!(lapse_claim(registry.as_ref(), 2), LeaseRecovery::Failed);
    }

    fn a_brief_never_reentered_keeps_its_count_of_lapses(store: &dyn Store) {
        let registry = store.handle();
        let entered = registry
            .enter_brief(&brief("once"), &MissionEntry::person(Some("G")))
            .unwrap();
        assert_eq!(lapse_claim(registry.as_ref(), 1), LeaseRecovery::Requeued);
        assert_eq!(lapse_claim(registry.as_ref(), 2), LeaseRecovery::Failed);
        let filed = registry.filed_brief(&entered.key()).unwrap().unwrap();
        assert_eq!((filed.state.as_str(), filed.attempts), ("failed", 2));
    }

    fn follow_ups_take_the_items_handle_and_a_number_from_two(store: &dyn Store) {
        let registry = store.handle();
        registry
            .ingest_source_item(
                "G",
                "github",
                &source_item("o/r#31", Some("31"), Some("gh")),
                "hq",
            )
            .unwrap();
        let item = registry.list_new().unwrap()[0].key();
        let follow_up = |registry: &dyn RegistryBackend| {
            registry
                .allot_mission_id(&MissionEntry::follow_up("G", "github", "o/r#31"))
                .unwrap()
        };
        let second = follow_up(registry.as_ref());
        assert_eq!(second.handle, "G-gh-31-2");
        assert_ne!(second.mission_id, item);
        assert_eq!(follow_up(store.handle().as_ref()).handle, "G-gh-31-3");
        registry.rename_mission(&item, "G-crash").unwrap();
        assert_eq!(
            follow_up(registry.as_ref()).handle,
            "G-crash-4",
            "the item's handle as it is now"
        );
        assert_eq!(
            registry
                .allot_mission_id(&MissionEntry::follow_up("G", "github", "o/r#99"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound,
            "an item with no Mission has nothing to follow up"
        );
        assert_eq!(
            registry
                .allot_mission_id(&MissionEntry::follow_up("H", "github", "o/r#31"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound,
            "another Operation's item"
        );
        assert_eq!(person(registry.as_ref(), Some("G")), "G-1");
        assert_eq!(registry.resolve_mission("G-gh-31-2").unwrap(), Some(second));
    }

    fn person(registry: &dyn RegistryBackend, operation: Option<&str>) -> String {
        registry
            .allot_mission_id(&MissionEntry::person(operation))
            .unwrap()
            .handle
    }

    fn handle_of(brief: &Brief) -> String {
        brief.handle.clone().expect("an entered brief has a handle")
    }

    fn numbered(operation: &str, count: usize, threads: usize, store: &dyn Store) -> Vec<String> {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                let registry = store.handle();
                let operation = operation.to_owned();
                std::thread::spawn(move || {
                    (0..count)
                        .map(|_| person(registry.as_ref(), Some(&operation)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    }

    fn concurrent_people_get_distinct_consecutive_numbers(store: &dyn Store) {
        let handles = numbered("ops", 10, 6, store);
        let distinct: BTreeSet<_> = handles.iter().cloned().collect();
        assert_eq!(distinct.len(), 60, "{handles:?}");
        let expected: BTreeSet<_> = (1..=60).map(|number| format!("ops-{number}")).collect();
        assert_eq!(distinct, expected);
    }

    fn source_item(id: &str, code: Option<&str>, tag: Option<&str>) -> SourceItem {
        SourceItem {
            id: id.to_owned(),
            title: "Fix the build".to_owned(),
            body: "It broke.".to_owned(),
            url: None,
            labels: Vec::new(),
            author: None,
            updated: "2026-11-02T00:00:00Z".to_owned(),
            code: code.map(str::to_owned),
            code_tag: tag.map(str::to_owned),
        }
    }

    fn proposal_of(operation: &str, key: &str) -> MissionProposal {
        let mut proposed = brief("ignored");
        proposed.title = "Refactor it".to_owned();
        MissionProposal::try_new(
            operation,
            crate::MissionId::try_new("parent".to_owned()).unwrap(),
            "007",
            crate::SessionId::try_new("session".to_owned()).unwrap(),
            key,
            proposed,
            &Default::default(),
        )
        .unwrap()
    }

    fn each_entry_point_yields_its_shape(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(person(registry.as_ref(), Some("ai6")), "ai6-1");
        assert_eq!(person(registry.as_ref(), None), "mission-1");

        let sleeper = crate::directorates::registry::sleepers::tests::nightly();
        registry
            .fire_sleeper("ai6", "nightly", &sleeper, (1_792_922_400, None), "hq")
            .unwrap();
        let round = &registry.list_new().unwrap()[0];
        assert_eq!(handle_of(round), "ai6-nightly-2");
        assert!(is_uid(&round.key()), "{}", round.key());

        let proposal = proposal_of("ai6", "one");
        registry.submit_mission_proposal(&proposal).unwrap();
        let promoted = registry
            .promote_mission_proposal("ai6", &proposal.id, "hq")
            .unwrap();
        assert_eq!(handle_of(&promoted), "ai6-3");
        assert_eq!(promoted.source, "ai6-agent-proposal");
        assert_eq!(promoted.id, proposal.id);

        let mut review = brief("review-1");
        review.operation = Some("ai6".to_owned());
        let review = registry
            .enter_brief(&review, &MissionEntry::review("ai6", "review-1"))
            .unwrap();
        assert_eq!(handle_of(&review), "ai6-4");

        registry
            .ingest_source_item(
                "ai6",
                "jira",
                &source_item("AI6-64", Some("AI6-64"), None),
                "hq",
            )
            .unwrap();
        let queued = registry.list_new().unwrap();
        assert!(queued.iter().any(|brief| handle_of(brief) == "ai6-AI6-64"));
        // A person's next number comes after everything the counter gave.
        assert_eq!(person(registry.as_ref(), Some("ai6")), "ai6-5");
        let keys: BTreeSet<String> = queued.iter().map(Brief::key).collect();
        assert_eq!(
            keys.len(),
            queued.len(),
            "every Mission has a UID of its own"
        );
    }

    fn a_repeated_entry_keeps_its_mission_and_a_number_is_never_reused(store: &dyn Store) {
        let registry = store.handle();
        let entry = MissionEntry::sleeper("G", "deps", "20261025T0300Z", Some("a"));
        let first = registry.allot_mission_id(&entry).unwrap();
        assert_eq!(first.handle, "G-deps-1");
        assert!(is_uid(&first.mission_id));
        assert_eq!(registry.allot_mission_id(&entry).unwrap(), first);
        // Another handle, as another Station opens it, reads the same book.
        assert_eq!(
            store.handle().allot_mission_id(&entry).unwrap(),
            first,
            "a second Station gives the entry a second Mission"
        );
        assert_eq!(
            registry.entered_mission_id(&entry).unwrap(),
            Some(first.mission_id.clone())
        );
        let other = MissionEntry::sleeper("G", "deps", "20261025T0300Z", Some("b"));
        assert_eq!(registry.entered_mission_id(&other).unwrap(), None);
        assert_eq!(
            registry.allot_mission_id(&other).unwrap().handle,
            "G-deps-2"
        );

        // A brief that entered and finished leaves its number spent.
        let entered = registry
            .enter_brief(&brief("gone"), &MissionEntry::person(Some("G")))
            .unwrap();
        assert_eq!(handle_of(&entered), "G-3");
        let claimed = registry.claim_new(Some(&entered.key())).unwrap().unwrap();
        registry
            .finish(&claimed, &serde_json::json!({}), true, None, None)
            .unwrap();
        assert_eq!(person(registry.as_ref(), Some("G")), "G-4");
    }

    fn a_re_polled_item_keeps_its_mission_and_creates_nothing(store: &dyn Store) {
        let registry = store.handle();
        let item = source_item("o/r#63", Some("63"), Some("gh"));
        let ingest = |item: &SourceItem| {
            registry
                .ingest_source_item("ai6", "github", item, "station-a")
                .unwrap()
        };
        assert_eq!(ingest(&item), Ingest::Queued);
        assert_eq!(ingest(&item), Ingest::Unchanged);
        assert_eq!(registry.list_new().unwrap().len(), 1);
        let queued = registry.list_new().unwrap().remove(0);
        assert_eq!(handle_of(&queued), "ai6-gh-63");
        let key = queued.key();

        let mut changed = item.clone();
        changed.body = "It broke again.".to_owned();
        assert_eq!(ingest(&changed), Ingest::Updated);
        assert_eq!(registry.list_new().unwrap()[0].key(), key);

        let lease = registry
            .claim_with_lease(None, "station-a", DEFAULT_LEASE, BTreeMap::new())
            .unwrap()
            .unwrap();
        assert_eq!(lease.key(), key);
        assert_eq!(ingest(&changed), Ingest::InFlight);
        let claimed = registry.read_claimed(lease.key()).unwrap().unwrap();
        registry
            .finish(
                &claimed,
                &serde_json::json!({}),
                true,
                None,
                Some("station-a"),
            )
            .unwrap();
        assert_eq!(ingest(&changed), Ingest::Finished);
        assert!(registry.list_new().unwrap().is_empty());

        // Reopened, the item is the same Mission's next attempt, and only from a finished brief.
        let reopened = crate::directorates::registry::sources::reopened_brief(
            ("ai6", "github"),
            &changed,
            &[],
            &Default::default(),
            "station-a",
        );
        let entry = crate::directorates::registry::sources::source_entry("ai6", "github", &item);
        let Reentry::Queued(queued) = registry
            .reenter_brief(&reopened, &entry, &|_| true)
            .unwrap()
        else {
            panic!("a finished brief is queued again");
        };
        assert_eq!(
            (queued.key(), handle_of(&queued)),
            (key.clone(), "ai6-gh-63".to_owned())
        );
        assert!(matches!(
            registry.reenter_brief(&reopened, &entry, &|_| true).unwrap(),
            Reentry::Unchanged(filed) if filed.state == "new"
        ));
        assert_eq!(ingest(&changed), Ingest::Unchanged);
        let again = registry
            .claim_with_lease(None, "station-a", DEFAULT_LEASE, BTreeMap::new())
            .unwrap()
            .unwrap();
        assert_eq!(
            (again.key(), again.attempt()),
            (key.as_str(), 1),
            "the next attempt counts its claims from one"
        );
        let claimed = registry.read_claimed(again.key()).unwrap().unwrap();
        assert!(crate::directorates::registry::sources::reportable(
            &claimed, "github"
        ));
        registry
            .finish(
                &claimed,
                &serde_json::json!({}),
                true,
                None,
                Some("station-a"),
            )
            .unwrap();
        assert_eq!(
            registry.unreported_source_briefs().unwrap().len(),
            1,
            "the next attempt owes its own report"
        );
        assert!(matches!(
            registry.reenter_brief(&reopened, &entry, &|_| false).unwrap(),
            Reentry::Unchanged(filed) if filed.state == "done" && filed.attempts == 1
        ));
        let unknown = crate::directorates::registry::sources::source_entry(
            "ai6",
            "github",
            &source_item("o/r#64", Some("64"), Some("gh")),
        );
        assert_eq!(
            registry
                .reenter_brief(&reopened, &unknown, &|_| true)
                .unwrap(),
            Reentry::Unknown
        );
        // The same number in another Operation is another Mission.
        assert_eq!(
            registry
                .ingest_source_item("other", "github", &item, "station-a")
                .unwrap(),
            Ingest::Queued
        );
        assert_eq!(handle_of(&registry.list_new().unwrap()[0]), "other-gh-63");
    }

    fn jira_and_github_codes_shape_the_handle(store: &dyn Store) {
        let registry = store.handle();
        for (instance, item) in [
            ("jira", source_item("AI6-64", Some("AI6-64"), None)),
            ("github", source_item("o/r#63", Some("63"), Some("gh"))),
            (
                "backend",
                source_item("o/b#63", Some("63"), Some("backend")),
            ),
            ("board", source_item("board/7", None, None)),
        ] {
            registry
                .ingest_source_item("ai6", instance, &item, "hq")
                .unwrap();
        }
        let handles: BTreeSet<_> = registry.list_new().unwrap().iter().map(handle_of).collect();
        assert_eq!(
            handles,
            BTreeSet::from(
                [
                    "ai6-AI6-64",
                    "ai6-gh-63",
                    "ai6-backend-63",
                    "ai6-board-board_7"
                ]
                .map(str::to_owned)
            )
        );
    }

    fn an_item_keeps_its_handle_when_its_instance_stops_being_the_only_one(store: &dyn Store) {
        let registry = store.handle();
        let alone = source_item("o/r#63", Some("63"), Some("gh"));
        let beside = source_item("o/r#63", Some("63"), Some("github"));
        registry
            .ingest_source_item("ai6", "github", &alone, "hq")
            .unwrap();
        assert_eq!(
            registry
                .ingest_source_item("ai6", "github", &beside, "hq")
                .unwrap(),
            Ingest::Unchanged
        );
        assert_eq!(registry.list_new().unwrap().len(), 1);
        assert_eq!(handle_of(&registry.list_new().unwrap()[0]), "ai6-gh-63");
    }

    /// What Git's `check-ref-format` refuses in a branch, what a Windows or case-insensitive
    /// file system cannot hold, and what a URL would have to escape.
    fn assert_valid_everywhere(handle: &str) {
        assert!(
            !handle.is_empty() && handle.len() <= BRANCH_ROOM,
            "{handle}"
        );
        assert!(
            handle.chars().all(
                |character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            ),
            "{handle}"
        );
        assert!(!handle.starts_with('-'), "{handle}");
        assert!(!handle.ends_with('-'), "{handle}");
        assert!(!handle.contains(".."), "{handle}");
        assert!(!handle.ends_with(".lock"), "{handle}");
        assert!(!handle.ends_with('.'), "{handle}");
        assert!(valid_handle(handle), "{handle}");
    }

    fn odd_codes_become_valid_as_a_branch_a_path_and_a_url_part(store: &dyn Store) {
        let registry = store.handle();
        let long = "x".repeat(400);
        let uid_code = new_uid();
        for (index, code) in [
            "a/b c",
            "..",
            "x..y",
            "fix.lock",
            "-leading",
            "trailing.",
            "../../etc/passwd",
            "CON",
            "a\\b:c*d?e\"f<g>h|i",
            "\u{202e}txt.exe",
            "日本語",
            "@{1}",
            "a\nb",
            long.as_str(),
            uid_code.as_str(),
        ]
        .into_iter()
        .enumerate()
        {
            let item = source_item(&format!("item-{index}"), Some(code), None);
            registry
                .ingest_source_item("ops", "github", &item, "hq")
                .unwrap();
        }
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 15);
        for brief in &queued {
            assert_valid_everywhere(&handle_of(brief));
            assert!(
                handle_of(brief).len() <= DEFAULT_ID_BYTES,
                "{}",
                handle_of(brief)
            );
        }
        let cut = queued
            .iter()
            .map(handle_of)
            .find(|handle| handle.contains("xxxx"))
            .unwrap();
        assert!(cut.starts_with("ops-xxx"), "{cut}");
        assert_eq!(
            registry
                .ingest_source_item(
                    "ops",
                    "github",
                    &source_item("item-13", Some(&long), None),
                    "hq"
                )
                .unwrap(),
            Ingest::Unchanged,
            "a long code is cut the same way each time"
        );
        // An Operation's own odd name is made safe the same way.
        for operation in ["-odd.name", "a b", "a..b", "x.lock", "..", "日本語"] {
            assert_valid_everywhere(&person(registry.as_ref(), Some(operation)));
        }
    }

    fn operations_whose_handles_would_collide_never_share_one(store: &dyn Store) {
        let registry = store.handle();
        // Two Operations a case-insensitive file system cannot tell apart.
        let upper = person(registry.as_ref(), Some("OPS"));
        let lower = person(registry.as_ref(), Some("ops"));
        assert_eq!((upper.as_str(), lower.as_str()), ("OPS-1", "ops-2"));

        // Operation `ai6-gh`'s Mission 63 and Operation `ai6`'s GitHub item 63.
        let mut numbers = Vec::new();
        for _ in 0..63 {
            numbers.push(person(registry.as_ref(), Some("ai6-gh")));
        }
        assert_eq!(numbers.last().unwrap(), "ai6-gh-63");
        let item = source_item("o/r#63", Some("63"), Some("gh"));
        assert_eq!(
            registry
                .ingest_source_item("ai6", "github", &item, "hq")
                .unwrap(),
            Ingest::Queued,
            "another Operation's Mission must not read as this item finished"
        );
        let handle = handle_of(&registry.list_new().unwrap()[0]);
        assert!(
            handle.starts_with("ai6-gh-63-") && handle.len() > 10,
            "{handle}"
        );
        assert_eq!(
            registry
                .ingest_source_item("ai6", "github", &item, "hq")
                .unwrap(),
            Ingest::Unchanged
        );
        assert_eq!(handle_of(&registry.list_new().unwrap()[0]), handle);

        // Sleeper `b` of Operation `a` and the Missions of Operation `a-b`.
        let plain = person(registry.as_ref(), Some("a-b"));
        assert_eq!(plain, "a-b-1");
        let round = MissionEntry::sleeper("a", "b", "w", None);
        assert_eq!(registry.allot_mission_id(&round).unwrap().handle, "a-b-2");
    }

    fn a_brief_keeps_its_mission_through_a_requeue_and_a_hold(store: &dyn Store) {
        let registry = store.handle();
        let mut asked = brief("asked");
        asked.operation = Some("G".to_owned());
        let entered = registry
            .enter_brief(&asked, &MissionEntry::person(Some("G")))
            .unwrap();
        let key = entered.key();
        assert_eq!(handle_of(&entered), "G-1");
        assert_eq!(entered.mission_id.as_deref(), Some(key.as_str()));
        let vetting = crate::directorates::m::vetting::Vetting::pass();
        assert!(
            registry
                .hold_brief("G", &key, &vetting, 1.0)
                .unwrap()
                .is_some()
        );
        assert!(registry.requeue_held("G", &key).unwrap());
        assert_eq!(registry.list_new().unwrap()[0].key(), key);

        let lease = registry
            .claim_with_lease(Some(&key), "dead", DEFAULT_LEASE, BTreeMap::new())
            .unwrap()
            .unwrap();
        let lapsed =
            crate::directorates::registry::suite::lapse(registry.as_ref(), lease.key(), "dead");
        assert!(registry.recover(&lapsed, 5, "hq").unwrap().is_some());
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].key(), key);
        assert_eq!(handle_of(&queued[0]), "G-1");
        let again = registry
            .claim_with_lease(Some(&key), "hq", DEFAULT_LEASE, BTreeMap::new())
            .unwrap()
            .unwrap();
        assert_eq!(again.attempt(), 2);
        assert_eq!(person(registry.as_ref(), Some("G")), "G-2");
    }

    fn a_proposal_gets_one_number_however_often_it_is_approved(store: &dyn Store) {
        let registry = store.handle();
        let proposal = proposal_of("G", "retry");
        registry.submit_mission_proposal(&proposal).unwrap();
        let first = registry
            .promote_mission_proposal("G", &proposal.id, "hq")
            .unwrap();
        assert_eq!(handle_of(&first), "G-1");
        let again = store
            .handle()
            .promote_mission_proposal("G", &proposal.id, "other")
            .unwrap();
        assert_eq!(again.key(), first.key());
        assert_eq!(handle_of(&again), "G-1");
        assert_eq!(again.provenance, first.provenance);
        assert_eq!(registry.list_new().unwrap().len(), 1);
        assert_eq!(person(registry.as_ref(), Some("G")), "G-2");
    }

    fn a_dropped_record_cannot_name_a_path(store: &dyn Store) {
        let registry = store.handle();
        registry.drop_brief(
            "escape",
            r#"{"title":"Evil","id":"x","mission_id":"../../escape"}"#,
        );
        registry.drop_brief("y", r#"{"title":"Early","id":"y","handle":"G-1"}"#);
        let uid = new_uid();
        registry.drop_brief(
            &uid,
            &format!(r#"{{"title":"Odd","id":"z","mission_id":"{uid}","handle":"../x"}}"#),
        );
        assert!(
            registry.list_new().unwrap().is_empty(),
            "a Mission ID that is no Registry key, or a handle without one or that is no handle, \
             makes the record invalid"
        );
        registry.drop_brief("G-1", r#"{"title":"Early","id":"y","mission_id":"G-1"}"#);
        let allotted = registry
            .allot_mission_id(&MissionEntry::person(Some("G")))
            .unwrap();
        assert_eq!(
            allotted.handle, "G-1",
            "a key a dropped brief goes by takes no handle"
        );
        assert_eq!(
            registry.resolve_mission("G-1").unwrap(),
            Some(allotted),
            "a handle names its Mission before a dropped key does"
        );
    }

    fn a_finished_brief_from_elsewhere_owes_no_report(store: &dyn Store) {
        let registry = store.handle();
        let asked = registry
            .enter_brief(&brief("asked"), &MissionEntry::person(Some("source_x")))
            .unwrap();
        registry
            .ingest_source_item("G", "github", &source_item("o/r#1", None, None), "hq")
            .unwrap();
        for brief in registry.list_new().unwrap() {
            registry
                .finish(&brief, &serde_json::json!({}), true, None, None)
                .unwrap();
        }
        let listed: Vec<String> = registry
            .unreported_source_briefs()
            .unwrap()
            .iter()
            .map(|finished| handle_of(&finished.brief))
            .collect();
        assert_eq!(
            listed,
            ["G-github-o_r_1"],
            "{} owes nothing",
            handle_of(&asked)
        );
    }

    fn a_rename_keeps_the_uid_and_every_handle_finds_it(store: &dyn Store) {
        let registry = store.handle();
        let entered = registry
            .enter_brief(&brief("asked"), &MissionEntry::person(Some("G")))
            .unwrap();
        let uid = entered.key();
        let renamed = registry.rename_mission(&uid, "G-build-fix").unwrap();
        assert_eq!(renamed, MissionHandle::new(uid.clone(), "G-build-fix"));
        let current = Some(renamed.clone());
        assert_eq!(registry.resolve_mission(&uid).unwrap(), current);
        assert_eq!(registry.resolve_mission("G-1").unwrap(), current);
        assert_eq!(registry.resolve_mission("g-1").unwrap(), current);
        assert_eq!(registry.resolve_mission("g-BUILD-fix").unwrap(), current);
        // Another Station reads the same book.
        assert_eq!(store.handle().resolve_mission("G-1").unwrap(), current);
        assert_eq!(registry.resolve_mission("G-2").unwrap(), None);
        assert_eq!(registry.resolve_mission(&new_uid()).unwrap(), None);
        assert_eq!(registry.resolve_mission("../x").unwrap(), None);
        let queued = registry.list_new().unwrap();
        assert_eq!(queued[0].key(), uid);
        assert_eq!(handle_of(&queued[0]), "G-build-fix");
        // A brief dropped by hand goes by its key, and has no handle to rename.
        registry.submit_brief(&brief("manual")).unwrap();
        assert_eq!(
            registry.resolve_mission("manual--manual").unwrap(),
            Some(MissionHandle::new("manual--manual", "manual--manual"))
        );
        assert_eq!(
            registry
                .rename_mission("manual--manual", "G-x")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            registry
                .rename_mission(&new_uid(), "G-y")
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    fn a_rename_reaches_the_brief_in_every_state(store: &dyn Store) {
        let registry = store.handle();
        registry
            .ingest_source_item(
                "G",
                "github",
                &source_item("o/r#7", Some("7"), Some("gh")),
                "hq",
            )
            .unwrap();
        let key = registry.list_new().unwrap()[0].key();
        let vetting = crate::directorates::m::vetting::Vetting::pass();
        registry.hold_brief("G", &key, &vetting, 1.0).unwrap();
        registry.rename_mission(&key, "G-held").unwrap();
        let held = registry.held_briefs("G", 10).unwrap();
        assert_eq!(handle_of(&held[0].brief), "G-held");
        assert!(registry.requeue_held("G", &key).unwrap());
        let queued = registry.list_new().unwrap().remove(0);
        assert_eq!(handle_of(&queued), "G-held");
        assert!(
            queued.provenance.is_some(),
            "a rename leaves what the brief says, and its provenance, as they were"
        );

        let lease = registry
            .claim_with_lease(Some(&key), "hq", DEFAULT_LEASE, BTreeMap::new())
            .unwrap()
            .unwrap();
        registry.rename_mission(&key, "G-claimed").unwrap();
        let claimed = registry.read_claimed(lease.key()).unwrap().unwrap();
        assert_eq!(handle_of(&claimed), "G-claimed");
        assert!(claimed.provenance.is_some());
        registry
            .finish(&claimed, &serde_json::json!({}), true, None, Some("hq"))
            .unwrap();
        registry.rename_mission(&key, "G-done").unwrap();
        let filed = registry.filed_brief(&key).unwrap().unwrap();
        assert_eq!(filed.state, "done");
        assert_eq!(handle_of(&filed.brief), "G-done");
        for former in ["G-gh-7", "G-held", "G-claimed", "G-done"] {
            assert_eq!(
                registry.resolve_mission(former).unwrap(),
                Some(MissionHandle::new(key.clone(), "G-done")),
                "{former}"
            );
        }
        let unreported = registry.unreported_source_briefs().unwrap();
        assert_eq!(handle_of(&unreported[0].brief), "G-done");
        assert!(crate::directorates::registry::sources::reportable(
            &unreported[0].brief,
            "github"
        ));
    }

    fn a_taken_or_malformed_handle_is_refused(store: &dyn Store) {
        let registry = store.handle();
        let first = registry
            .enter_brief(&brief("one"), &MissionEntry::person(Some("G")))
            .unwrap();
        let second = registry
            .enter_brief(&brief("two"), &MissionEntry::person(Some("G")))
            .unwrap();
        registry.rename_mission(&first.key(), "G-first").unwrap();
        for taken in ["G-first", "g-FIRST", "G-1"] {
            assert_eq!(
                registry
                    .rename_mission(&second.key(), taken)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::AlreadyExists,
                "{taken}"
            );
        }
        let long = "y".repeat(DEFAULT_ID_BYTES + 1);
        for malformed in [
            "",
            "a b",
            "-lead",
            "trail-",
            "a.b",
            "a/b",
            "ünï",
            long.as_str(),
            first.key().as_str(),
        ] {
            assert_eq!(
                registry
                    .rename_mission(&second.key(), malformed)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput,
                "{malformed:?}"
            );
        }
        assert_eq!(
            handle_of(&registry.filed_brief(&second.key()).unwrap().unwrap().brief),
            "G-2"
        );
        // A Mission may take back a handle of its own.
        registry.rename_mission(&first.key(), "G-1").unwrap();
        assert_eq!(
            registry.resolve_mission("G-first").unwrap().unwrap().handle,
            "G-1"
        );
    }

    fn a_former_handle_is_never_given_again(store: &dyn Store) {
        let registry = store.handle();
        let entered = registry
            .enter_brief(&brief("one"), &MissionEntry::person(Some("G")))
            .unwrap();
        registry.rename_mission(&entered.key(), "G-2").unwrap();
        assert_eq!(
            person(registry.as_ref(), Some("G")),
            "G-3",
            "a handle a rename took is skipped by the counter"
        );
        let other = registry
            .enter_brief(&brief("two"), &MissionEntry::person(Some("G")))
            .unwrap();
        assert_eq!(handle_of(&other), "G-4");
        assert_eq!(
            registry
                .rename_mission(&other.key(), "G-1")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists,
            "a former handle stays with its Mission"
        );
    }

    fn a_renamed_item_keeps_its_new_handle_when_polled_again(store: &dyn Store) {
        let registry = store.handle();
        let item = source_item("o/r#9", Some("9"), Some("gh"));
        registry
            .ingest_source_item("G", "github", &item, "hq")
            .unwrap();
        let key = registry.list_new().unwrap()[0].key();
        registry.rename_mission(&key, "G-nine").unwrap();
        assert_eq!(
            registry
                .ingest_source_item("G", "github", &item, "hq")
                .unwrap(),
            Ingest::Unchanged
        );
        let mut changed = item.clone();
        changed.body = "Still broken.".to_owned();
        assert_eq!(
            store
                .handle()
                .ingest_source_item("G", "github", &changed, "hq")
                .unwrap(),
            Ingest::Updated
        );
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(
            (queued[0].key(), handle_of(&queued[0])),
            (key, "G-nine".to_owned())
        );
        assert!(queued[0].provenance.is_some());
    }

    fn run_handles_follow_the_current_handle_only_for_new_runs(store: &dyn Store) {
        let registry = store.handle();
        let entered = registry
            .enter_brief(&brief("one"), &MissionEntry::person(Some("G")))
            .unwrap();
        let uid = entered.key();
        let open = |attempt: u32, handle: &str| {
            registry.open_session(
                NewSession::new(new_uid(), &uid, "hq")
                    .with_handle(handle)
                    .with_attempt(attempt, None),
                DEFAULT_LEASE,
            )
        };
        let first = open(1, "G-1").unwrap();
        assert!(is_uid(&first.session_id));
        assert_eq!(first.handle.as_deref(), Some("G-1.1"));
        assert!(
            matches!(
                open(1, "G-1"),
                Err(crate::directorates::registry::sessions::SessionError::Exists)
            ),
            "one record per attempt, whatever its UID"
        );
        let renamed = registry.rename_mission(&uid, "G-fix").unwrap();
        let second = open(2, &renamed.handle).unwrap();
        assert_eq!(second.handle.as_deref(), Some("G-fix.2"));
        assert_eq!(
            registry
                .session(&first.session_id)
                .unwrap()
                .unwrap()
                .handle
                .as_deref(),
            Some("G-1.1"),
            "a run keeps the handle it opened with"
        );
        let next = NewSession::new(new_uid(), &uid, "hq")
            .with_handle("G-fix")
            .with_attempt(2, None)
            .next_attempt()
            .unwrap();
        let third = registry.open_session(next, DEFAULT_LEASE).unwrap();
        assert_eq!(
            (third.attempt, third.handle.as_deref()),
            (3, Some("G-fix.3"))
        );
        assert_ne!(third.session_id, second.session_id);
    }

    #[test]
    fn a_deleted_brief_does_not_free_its_number_on_a_file_registry() {
        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        let entered = registry
            .enter_brief(&brief("gone"), &MissionEntry::person(Some("G")))
            .unwrap();
        assert_eq!(handle_of(&entered), "G-1");
        let claimed = registry.claim_new(Some(&entered.key())).unwrap().unwrap();
        registry
            .finish(&claimed, &serde_json::json!({}), true, None, None)
            .unwrap();
        std::fs::remove_dir_all(root.path().join("done")).unwrap();
        assert_eq!(person(&registry, Some("G")), "G-2");
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn a_deleted_brief_does_not_free_its_number_on_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.db");
        let registry = crate::directorates::registry::sql::SqlRegistry::open(&format!(
            "sqlite:{}",
            path.display()
        ))
        .unwrap();
        let entered = registry
            .enter_brief(&brief("gone"), &MissionEntry::person(Some("G")))
            .unwrap();
        assert_eq!(handle_of(&entered), "G-1");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("DELETE FROM ai6_briefs", [])
            .unwrap();
        assert!(registry.list_new().unwrap().is_empty());
        assert_eq!(person(&registry, Some("G")), "G-2");
    }

    /// The listing is cut at a row limit, so a finished brief that owes nothing must not count
    /// against one that does.
    #[cfg(feature = "sqlite")]
    #[test]
    fn finished_briefs_that_owe_nothing_do_not_crowd_out_a_report() {
        let dir = tempfile::tempdir().unwrap();
        let mut limits = RegistryLimits::default();
        limits.briefs.records = crate::bounds::Limit::at(1);
        let registry = crate::directorates::registry::sql::SqlRegistry::open_with(
            &format!("sqlite:{}", dir.path().join("registry.db").display()),
            &crate::directorates::registry::sql::SqlOptions::default().with_limits(limits),
        )
        .unwrap();
        for _ in 0..12 {
            let entered = registry
                .enter_brief(&brief("asked"), &MissionEntry::person(Some("A")))
                .unwrap();
            registry
                .finish(&entered, &serde_json::json!({}), true, None, None)
                .unwrap();
        }
        registry
            .ingest_source_item("Z", "github", &source_item("o/r#1", None, None), "hq")
            .unwrap();
        let queued = registry.list_new().unwrap();
        registry
            .finish(&queued[0], &serde_json::json!({}), true, None, None)
            .unwrap();
        let listed = registry.unreported_source_briefs().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(handle_of(&listed[0].brief), "Z-github-o_r_1");
    }

    const DEFAULT_ID_BYTES: usize = 120;

    #[test]
    fn the_limit_on_a_handle_is_a_setting() {
        let limits = RegistryLimits::default();
        assert_eq!(id_bytes(&limits.briefs), DEFAULT_ID_BYTES);
        let handle = fit("y".repeat(400), DEFAULT_ID_BYTES);
        assert_eq!(handle.len(), DEFAULT_ID_BYTES);
        assert_eq!(run_handle(&handle, 4_000_000), format!("{handle}.4000000"));

        let root = tempfile::tempdir().unwrap();
        let mut narrow = RegistryLimits::default();
        narrow.briefs.handle_bytes = crate::bounds::Limit::at(16);
        let registry = FileRegistry::open(root.path()).with_limits(narrow);
        let allotted = registry
            .allot_mission_id(&MissionEntry::person(Some(&"operation".repeat(9))))
            .unwrap();
        assert!(allotted.handle.len() <= 16, "{}", allotted.handle);
        let entered = registry
            .enter_brief(&brief("x"), &MissionEntry::person(Some("G")))
            .unwrap();
        assert_eq!(
            registry
                .rename_mission(&entered.key(), &"z".repeat(17))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        registry
            .rename_mission(&entered.key(), &"z".repeat(16))
            .unwrap();
    }

    #[test]
    fn a_uid_is_a_lower_case_uuid_v7_and_never_a_handle() {
        let uid = new_uid();
        assert!(is_uid(&uid));
        assert_eq!(uuid::Uuid::parse_str(&uid).unwrap().get_version_num(), 7);
        assert!(!is_uid(&uid.to_ascii_uppercase()));
        assert!(!is_uid(&uid.replace('-', "")));
        assert!(!is_uid("ai6-42"));
        assert!(!valid_handle(&uid));
        assert!(valid_handle(&uid.to_ascii_uppercase()));
        assert!(new_uid() > uid, "UIDs sort by when they were made");
        assert_eq!(run_handle("ai6-42", 2), "ai6-42.2");
    }

    #[test]
    fn an_unreadable_counter_refuses_rather_than_starting_again() {
        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        assert_eq!(person(&registry, Some("G")), "G-1");
        let counter = root.path().join("mission-ids").join("counters").join("g");
        std::fs::write(&counter, "not a number").unwrap();
        assert!(
            registry
                .allot_mission_id(&MissionEntry::person(Some("G")))
                .is_err()
        );
    }

    /// Set to a Registry directory, the test below allots for real, in this process, and writes
    /// what it got beside it. Each process is the test binary, started by the test.
    const CHILD_ROOT: &str = "AI6_TEST_MISSION_COUNTER_ROOT";

    #[test]
    fn two_processes_share_one_file_counter() {
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let registry = FileRegistry::open(&root);
            let handles: Vec<String> = (0..25).map(|_| person(&registry, Some("ops"))).collect();
            let out = std::path::Path::new(&root).join(format!("ids-{}.txt", std::process::id()));
            std::fs::write(out, handles.join("\n")).unwrap();
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let children: Vec<_> = (0..3)
            .map(|_| {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "directorates::registry::mission_ids::tests::two_processes_share_one_file_counter",
                        "--nocapture",
                    ])
                    .env(CHILD_ROOT, root.path())
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        let mut handles = BTreeSet::new();
        let mut files = 0;
        for entry in std::fs::read_dir(root.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "txt") {
                files += 1;
                for line in std::fs::read_to_string(path).unwrap().lines() {
                    assert!(handles.insert(line.to_owned()), "{line} was given twice");
                }
            }
        }
        assert_eq!(files, 3, "a child process ran no test");
        let expected: BTreeSet<_> = (1..=75).map(|number| format!("ops-{number}")).collect();
        assert_eq!(handles, expected);
    }
}
