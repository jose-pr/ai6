//! Mission IDs: the one rule that names a brief's Mission where the brief enters the Registry.
//!
//! An ID names its Operation and either a number from the Operation's counter or the tracker's
//! code: `ai6-42`, `ai6-nightly-43`, `ai6-gh-63`, `ai6-AI6-64`. The ID is chosen once, kept on
//! the brief, and is opaque from then on: nothing reads a code or a number back out of it.
//!
//! The Registry keeps every ID it issued, with the [`MissionEntry`] it was issued to when that
//! entry can come again (a re-polled item, a Sleeper's round, a review, a repeated approval), so
//! the same entry always gets the same ID, on any Station, however its brief moved since. No two
//! IDs differ only by case, since an ID becomes a file name and a branch.

use std::io;

use sha2::{Digest, Sha256};

use super::limits::BriefLimits;
use super::store::hex_digest;

/// What stands for the Operation in the ID of a brief that names none.
const NO_OPERATION: &str = "mission";

/// Separates the parts of an entry's name in the book. Operation, instance and Sleeper names
/// hold no control character, and a SQL script can write this one with `chr(31)`.
const SEPARATOR: char = '\u{1f}';

/// The most bytes a Mission ID can have, whatever `mission_id_bytes` says: it is a file name,
/// 255 bytes on every platform here, with a suffix of up to 15 (`.outcome.json`, `.N.json`).
const FILE_NAME_ROOM: usize = 240;

/// Where a brief enters, which decides its Mission ID.
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

    /// An item of a Source instance: `<operation>-<tag>-<code>`, or `<operation>-<code>` for a
    /// code that names its own project, or `<operation>-<instance>-<item>` when the provider
    /// gives no code. `tag` and `code` count only the first time the item is seen: the item
    /// keeps the ID it was given if the Operation's instances change later.
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
            EntryKind::Person => return None,
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
            EntryKind::Source { instance, item, .. } => vec!["source", operation, instance, item],
        };
        Some(parts.join(&SEPARATOR.to_string()))
    }

    /// The label the ID starts with, and the counter's name once folded.
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
            EntryKind::Person | EntryKind::Proposal { .. } | EntryKind::Review { .. } => {
                format!("{label}-{number}")
            }
            EntryKind::Sleeper { name, .. } => format!("{label}-{}-{number}", part(name)),
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

/// A name as it may stand in an ID: only ASCII letters, digits, `_` and `-`, so it is valid in a
/// file name on any platform, in a Git branch and in a URL without escaping. A `.` goes too: it
/// is what makes `..`, a trailing `.lock` and a trailing dot, and it separates a run's attempt.
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

/// `id` within `bytes`: its start and a short hash of the whole of it, when it is longer.
fn fit(id: String, bytes: usize) -> String {
    if id.len() <= bytes {
        return id;
    }
    let start = id[..bytes.saturating_sub(9)].trim_end_matches('-');
    format!("{start}-{}", short_hash(&id))
}

fn short_hash(text: &str) -> String {
    hex_digest(&Sha256::digest(text.as_bytes()))[..8].to_owned()
}

/// The longest ID `limits` allow.
fn id_bytes(limits: &BriefLimits) -> usize {
    limits.mission_id_bytes.or_max().min(FILE_NAME_ROOM)
}

/// An ID as the book compares it: two IDs that differ only by case are one file on Windows and
/// macOS, and one branch once fetched there.
pub(super) fn folded(id: &str) -> String {
    id.to_ascii_lowercase()
}

/// The name of an entry's file, for a backend that cannot hold it as a key.
pub(super) fn book_digest(name: &str) -> String {
    hex_digest(&Sha256::digest(name.as_bytes()))
}

/// What a backend keeps for the rule, read and written under its one writer's lock.
pub(super) trait MissionBook {
    /// The ID an entry was given.
    fn entered(&mut self, name: &str) -> io::Result<Option<String>>;
    /// Whether `id` was issued, compared without case, or a brief already goes by it.
    fn taken(&mut self, id: &str) -> io::Result<bool>;
    /// The next number of the counter `name`, written before it is used, so it is never reused.
    fn next_number(&mut self, name: &str) -> io::Result<u64>;
    /// Records `id` as issued to the entry `name`.
    fn issue(&mut self, id: &str, name: Option<&str>) -> io::Result<()>;
}

/// The Mission ID for `entry`: the one it was given, or a new one by the rule.
///
/// A number another entry's ID already reads as is skipped, and never used. A code whose ID is
/// taken, by `ai6-gh`'s Mission 63 for `ai6`'s GitHub item 63, adds a short hash of the entry.
pub(super) fn allot(
    book: &mut dyn MissionBook,
    entry: &MissionEntry,
    limits: &BriefLimits,
) -> io::Result<String> {
    let name = entry.book_name();
    if let Some(name) = &name
        && let Some(id) = book.entered(name)?
    {
        return Ok(id);
    }
    let bytes = id_bytes(limits);
    let id = if entry.counted() {
        let counter = folded(&fit(entry.label(), bytes));
        loop {
            let id = fit(entry.shaped(Some(book.next_number(&counter)?)), bytes);
            if !book.taken(&id)? {
                break id;
            }
        }
    } else {
        let name = name.as_deref().unwrap_or_default();
        let id = fit(entry.shaped(None), bytes);
        if book.taken(&id)? {
            let id = format!(
                "{}-{}",
                &id[..id.len().min(bytes.saturating_sub(9))],
                short_hash(name)
            );
            if book.taken(&id)? {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("Mission ID {id} is taken by another entry"),
                ));
            }
            id
        } else {
            id
        }
    };
    book.issue(&id, name.as_deref())?;
    Ok(id)
}

/// The ID of a run of a Mission, `<mission>.<attempt>`, within `bytes`: a Session record's ID.
/// A longer Mission ID keeps its start and adds a short hash of the whole.
pub fn run_id(mission_id: &str, attempt: u32, bytes: usize) -> String {
    let safe: String = mission_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim_start_matches('.');
    let suffix = format!(".{attempt}");
    if safe == mission_id && mission_id.len() + suffix.len() <= bytes {
        return format!("{mission_id}{suffix}");
    }
    let room = bytes.saturating_sub(suffix.len() + 9).min(safe.len());
    let mut start = room;
    while !safe.is_char_boundary(start) {
        start -= 1;
    }
    format!("{}-{}{suffix}", &safe[..start], short_hash(mission_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::backend::*;
    use crate::directorates::registry::limits::RegistryLimits;
    use crate::directorates::registry::sources::{Ingest, SourceItem};
    use crate::directorates::registry::store::{
        Brief, DEFAULT_LEASE, FileRegistry, MissionProposal,
    };
    use crate::directorates::registry::suite::{Store, backend_tests, brief};
    use std::collections::{BTreeMap, BTreeSet};

    backend_tests!(
        concurrent_people_get_distinct_consecutive_numbers,
        each_entry_point_yields_its_shape,
        a_repeated_entry_keeps_its_id_and_a_number_is_never_reused,
        a_re_polled_item_keeps_its_id_and_creates_nothing,
        jira_and_github_codes_shape_the_id,
        an_item_keeps_its_id_when_its_instance_stops_being_the_only_one,
        odd_codes_become_valid_as_a_branch_a_path_and_a_url_part,
        operations_whose_ids_would_collide_never_share_one,
        a_brief_keeps_its_id_through_a_requeue_and_a_hold,
        a_proposal_gets_one_number_however_often_it_is_approved,
        a_dropped_record_cannot_name_a_path_or_take_a_number,
        a_finished_brief_from_elsewhere_owes_no_report,
    );

    fn person(registry: &dyn RegistryBackend, operation: Option<&str>) -> String {
        registry
            .allot_mission_id(&MissionEntry::person(operation))
            .unwrap()
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
        let ids = numbered("ops", 10, 6, store);
        let distinct: BTreeSet<_> = ids.iter().cloned().collect();
        assert_eq!(distinct.len(), 60, "{ids:?}");
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
        assert_eq!(registry.list_new().unwrap()[0].key(), "ai6-nightly-2");

        let proposal = proposal_of("ai6", "one");
        registry.submit_mission_proposal(&proposal).unwrap();
        let promoted = registry
            .promote_mission_proposal("ai6", &proposal.id, "hq")
            .unwrap();
        assert_eq!(promoted.key(), "ai6-3");
        assert_eq!(promoted.source, "ai6-agent-proposal");
        assert_eq!(promoted.id, proposal.id);

        let mut review = brief("review-1");
        review.operation = Some("ai6".to_owned());
        let review = registry
            .enter_brief(&review, &MissionEntry::review("ai6", "review-1"))
            .unwrap();
        assert_eq!(review.key(), "ai6-4");

        registry
            .ingest_source_item(
                "ai6",
                "jira",
                &source_item("AI6-64", Some("AI6-64"), None),
                "hq",
            )
            .unwrap();
        let queued = registry.list_new().unwrap();
        assert!(queued.iter().any(|brief| brief.key() == "ai6-AI6-64"));
        // A person's next number comes after everything the counter gave.
        assert_eq!(person(registry.as_ref(), Some("ai6")), "ai6-5");
    }

    fn a_repeated_entry_keeps_its_id_and_a_number_is_never_reused(store: &dyn Store) {
        let registry = store.handle();
        let entry = MissionEntry::sleeper("G", "deps", "20261025T0300Z", Some("a"));
        let first = registry.allot_mission_id(&entry).unwrap();
        assert_eq!(first, "G-deps-1");
        assert_eq!(registry.allot_mission_id(&entry).unwrap(), first);
        // Another handle, as another Station opens it, reads the same book.
        assert_eq!(
            store.handle().allot_mission_id(&entry).unwrap(),
            first,
            "a second Station gives the entry a second number"
        );
        assert_eq!(registry.entered_mission_id(&entry).unwrap(), Some(first));
        let other = MissionEntry::sleeper("G", "deps", "20261025T0300Z", Some("b"));
        assert_eq!(registry.entered_mission_id(&other).unwrap(), None);
        assert_eq!(registry.allot_mission_id(&other).unwrap(), "G-deps-2");

        // A brief that entered and is deleted leaves its number spent.
        let entered = registry
            .enter_brief(&brief("gone"), &MissionEntry::person(Some("G")))
            .unwrap();
        assert_eq!(entered.key(), "G-3");
        let claimed = registry.claim_new(Some("G-3")).unwrap().unwrap();
        registry
            .finish(&claimed, &serde_json::json!({}), true, None, None)
            .unwrap();
        assert_eq!(person(registry.as_ref(), Some("G")), "G-4");
    }

    fn a_re_polled_item_keeps_its_id_and_creates_nothing(store: &dyn Store) {
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
        assert_eq!(registry.list_new().unwrap()[0].key(), "ai6-gh-63");

        let mut changed = item.clone();
        changed.body = "It broke again.".to_owned();
        assert_eq!(ingest(&changed), Ingest::Updated);
        assert_eq!(registry.list_new().unwrap()[0].key(), "ai6-gh-63");

        let lease = registry
            .claim_with_lease(None, "station-a", DEFAULT_LEASE, BTreeMap::new())
            .unwrap()
            .unwrap();
        assert_eq!(lease.key(), "ai6-gh-63");
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
        // The same number in another Operation is another Mission.
        assert_eq!(
            registry
                .ingest_source_item("other", "github", &item, "station-a")
                .unwrap(),
            Ingest::Queued
        );
        assert_eq!(registry.list_new().unwrap()[0].key(), "other-gh-63");
    }

    fn jira_and_github_codes_shape_the_id(store: &dyn Store) {
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
        let keys: BTreeSet<_> = registry
            .list_new()
            .unwrap()
            .iter()
            .map(Brief::key)
            .collect();
        assert_eq!(
            keys,
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

    fn an_item_keeps_its_id_when_its_instance_stops_being_the_only_one(store: &dyn Store) {
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
        assert_eq!(registry.list_new().unwrap()[0].key(), "ai6-gh-63");
    }

    /// What Git's `check-ref-format` refuses in a branch, what a Windows or case-insensitive
    /// file system cannot hold, and what a URL would have to escape.
    fn assert_valid_everywhere(id: &str) {
        assert!(!id.is_empty() && id.len() <= FILE_NAME_ROOM, "{id}");
        assert!(
            id.chars().all(
                |character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            ),
            "{id}"
        );
        assert!(!id.starts_with('-'), "{id}");
        assert!(!id.ends_with('-'), "{id}");
        assert!(!id.contains(".."), "{id}");
        assert!(!id.ends_with(".lock"), "{id}");
        assert!(!id.ends_with('.'), "{id}");
    }

    fn odd_codes_become_valid_as_a_branch_a_path_and_a_url_part(store: &dyn Store) {
        let registry = store.handle();
        let long = "x".repeat(400);
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
        ]
        .into_iter()
        .enumerate()
        {
            let item = source_item(&format!("item-{index}"), Some(code), Some("gh"));
            registry
                .ingest_source_item("ops", "github", &item, "hq")
                .unwrap();
        }
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 14);
        for brief in &queued {
            assert_valid_everywhere(&brief.key());
            assert!(brief.key().len() <= DEFAULT_ID_BYTES, "{}", brief.key());
        }
        let cut = queued
            .iter()
            .find(|brief| brief.key().contains("xxxx"))
            .unwrap()
            .key();
        assert!(cut.starts_with("ops-gh-xxx"), "{cut}");
        assert_eq!(
            registry
                .ingest_source_item(
                    "ops",
                    "github",
                    &source_item("item-13", Some(&long), Some("gh")),
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

    fn operations_whose_ids_would_collide_never_share_one(store: &dyn Store) {
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
        let key = registry.list_new().unwrap()[0].key();
        assert!(key.starts_with("ai6-gh-63-") && key.len() > 10, "{key}");
        assert_eq!(
            registry
                .ingest_source_item("ai6", "github", &item, "hq")
                .unwrap(),
            Ingest::Unchanged
        );
        assert_eq!(registry.list_new().unwrap()[0].key(), key);

        // Sleeper `b` of Operation `a` and the Missions of Operation `a-b`.
        let plain = person(registry.as_ref(), Some("a-b"));
        assert_eq!(plain, "a-b-1");
        let round = MissionEntry::sleeper("a", "b", "w", None);
        assert_eq!(registry.allot_mission_id(&round).unwrap(), "a-b-2");
    }

    fn a_brief_keeps_its_id_through_a_requeue_and_a_hold(store: &dyn Store) {
        let registry = store.handle();
        let mut asked = brief("asked");
        asked.operation = Some("G".to_owned());
        let entered = registry
            .enter_brief(&asked, &MissionEntry::person(Some("G")))
            .unwrap();
        let key = entered.key();
        assert_eq!(key, "G-1");
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
        assert_eq!(queued[0].mission_id.as_deref(), Some("G-1"));
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
        assert_eq!(first.key(), "G-1");
        let again = store
            .handle()
            .promote_mission_proposal("G", &proposal.id, "other")
            .unwrap();
        assert_eq!(again.key(), "G-1");
        assert_eq!(again.provenance, first.provenance);
        assert_eq!(registry.list_new().unwrap().len(), 1);
        assert_eq!(person(registry.as_ref(), Some("G")), "G-2");
    }

    fn a_dropped_record_cannot_name_a_path_or_take_a_number(store: &dyn Store) {
        let registry = store.handle();
        registry.drop_brief(
            "escape",
            r#"{"title":"Evil","id":"x","mission_id":"../../escape"}"#,
        );
        assert!(
            registry.list_new().unwrap().is_empty(),
            "a Mission ID that is no Registry key makes the record invalid"
        );
        registry.drop_brief("G-1", r#"{"title":"Early","id":"y","mission_id":"G-1"}"#);
        assert_eq!(
            person(registry.as_ref(), Some("G")),
            "G-2",
            "a number a dropped brief goes by is skipped"
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
            .map(|finished| finished.brief.key())
            .collect();
        assert_eq!(listed, ["G-github-o_r_1"], "{} owes nothing", asked.key());
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
        assert_eq!(listed[0].brief.key(), "Z-github-o_r_1");
    }

    const DEFAULT_ID_BYTES: usize = 120;

    #[test]
    fn the_limit_on_an_id_is_a_setting_and_the_default_leaves_a_run_room() {
        let limits = RegistryLimits::default();
        assert_eq!(id_bytes(&limits.briefs), DEFAULT_ID_BYTES);
        let long = "y".repeat(400);
        let id = fit(long, DEFAULT_ID_BYTES);
        let run = run_id(&id, 4_000_000, 128);
        assert!(run.starts_with(&id), "{run}");
        assert!(run.ends_with(".4000000"), "{run}");
        assert!(run.len() <= 128);

        let root = tempfile::tempdir().unwrap();
        let mut narrow = RegistryLimits::default();
        narrow.briefs.mission_id_bytes = crate::bounds::Limit::at(16);
        let registry = FileRegistry::open(root.path()).with_limits(narrow);
        let id = registry
            .allot_mission_id(&MissionEntry::person(Some(&"operation".repeat(9))))
            .unwrap();
        assert!(id.len() <= 16, "{id}");
    }

    #[test]
    fn a_run_is_the_mission_and_its_attempt() {
        assert_eq!(run_id("ai6-42", 1, 128), "ai6-42.1");
        assert_eq!(run_id("ai6-42", 2, 128), "ai6-42.2");
        assert_eq!(
            run_id("universal-exports--00000000-0000", 3, 128),
            "universal-exports--00000000-0000.3"
        );
        // An ID a Station was handed directly can hold anything but a control character.
        let odd = run_id("a/b c", 1, 128);
        assert!(odd.starts_with("a_b_c-") && odd.ends_with(".1"), "{odd}");
        assert_ne!(odd, run_id("a b/c", 1, 128));
        let dotted = run_id(".hidden", 1, 128);
        assert!(!dotted.starts_with('.'), "{dotted}");
        let long = run_id(&"z".repeat(300), 12, 128);
        assert!(long.len() <= 128 && long.ends_with(".12"), "{long}");
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
            let ids: Vec<String> = (0..25).map(|_| person(&registry, Some("ops"))).collect();
            let out = std::path::Path::new(&root).join(format!("ids-{}.txt", std::process::id()));
            std::fs::write(out, ids.join("\n")).unwrap();
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
        let mut ids = BTreeSet::new();
        let mut files = 0;
        for entry in std::fs::read_dir(root.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "txt") {
                files += 1;
                for line in std::fs::read_to_string(path).unwrap().lines() {
                    assert!(ids.insert(line.to_owned()), "{line} was given twice");
                }
            }
        }
        assert_eq!(files, 3, "a child process ran no test");
        let expected: BTreeSet<_> = (1..=75).map(|number| format!("ops-{number}")).collect();
        assert_eq!(ids, expected);
    }
}
