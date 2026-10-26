//! Held briefs and vetting clearances.
//!
//! A brief that vetting holds leaves the queue for `held/KEY.json` with the [`Vetting`] that
//! held it, where no Station claims it. An operator releases it back to the queue, with a
//! [`Clearance`] for the kinds it was held for so the same checks do not hold it again, or
//! refuses it, which files it as failed with a `refused` outcome (a Source still gets its CX
//! report). A clearance is kept apart from the brief (`vetting/KEY.json`), so a brief's own
//! text cannot carry one; it also records a Chief of Staff review that found nothing.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::store::{
    Brief, FileRegistry, MAX_BRIEF_BYTES, finish_locked, invalid_data, registry_lock,
    validate_registry_key, write_atomic,
};
use crate::directorates::m::vetting::{FindingKind, Vetting};

/// The most briefs held at once across the Registry; past it a brief stays queued.
pub const MAX_HELD_BRIEFS: usize = 1_000;
/// The most held briefs one listing returns.
pub const MAX_HELD_RESULTS: usize = 500;
pub(super) const MAX_HELD_RECORD_BYTES: u64 = MAX_BRIEF_BYTES + 64 * 1024;
const MAX_CLEARANCE_BYTES: u64 = 4_096;
const MAX_REASON_BYTES: usize = 1_024;
const MAX_ACTOR_BYTES: usize = 256;

/// A brief vetting held, waiting for an operator.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeldBrief {
    /// The Operation it belongs to, by codename.
    pub operation: String,
    /// The brief as it was queued.
    pub brief: Brief,
    /// Why it is held.
    pub vetting: Vetting,
    /// When it was held, as Unix seconds.
    pub held_at: f64,
}

impl HeldBrief {
    pub(super) fn validate(&self) -> io::Result<()> {
        validate_registry_key(&self.operation)?;
        self.brief.validate()?;
        if !self.held_at.is_finite() || self.held_at < 0.0 {
            return Err(invalid_data("held brief time is invalid"));
        }
        Ok(())
    }
}

/// Kinds of finding a brief no longer stops for: released by an operator, or a review that
/// found nothing.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clearance {
    /// The kinds cleared.
    pub kinds: BTreeSet<FindingKind>,
    /// Who cleared the last of them: a person, or `review:AGENT`.
    pub by: String,
    /// When, as Unix seconds.
    pub at: f64,
}

/// The outcome filed for a held brief an operator refused.
pub(super) fn refused_outcome(
    held: &HeldBrief,
    by: &str,
    reason: &str,
    at: f64,
) -> serde_json::Value {
    serde_json::json!({
        "state": "refused",
        "mission_id": held.brief.key(),
        "reason": reason,
        "refused_by": by,
        "refused_at": at,
        "vetting": held.vetting,
    })
}

pub(super) fn check_actor(by: &str, reason: &str) -> io::Result<()> {
    if by.trim().is_empty() || by.len() > MAX_ACTOR_BYTES || by.contains(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the actor must be 1 to 256 bytes without control characters",
        ));
    }
    if reason.len() > MAX_REASON_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the reason must be at most 1024 bytes",
        ));
    }
    Ok(())
}

pub(super) fn check_limit(limit: usize) -> io::Result<()> {
    if limit == 0 || limit > MAX_HELD_RESULTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("held brief result limit must be between 1 and {MAX_HELD_RESULTS}"),
        ));
    }
    Ok(())
}

/// Adds `kinds` to an existing clearance, or starts one.
pub(super) fn merge_clearance(
    existing: Option<Clearance>,
    kinds: &[FindingKind],
    by: &str,
    at: f64,
) -> Clearance {
    let mut clearance = existing.unwrap_or(Clearance {
        kinds: BTreeSet::new(),
        by: String::new(),
        at,
    });
    clearance.kinds.extend(kinds.iter().copied());
    clearance.by = by.to_owned();
    clearance.at = at;
    clearance
}

/// Held briefs and the clearances that keep vetting from holding a brief again.
pub trait VettingStore {
    /// Moves a queued brief to the held briefs of `operation` with the vetting that held it.
    /// `None` when the brief is no longer queued.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a key or Operation that is not a Registry key; `InvalidData` when
    /// [`MAX_HELD_BRIEFS`] are already held; I/O errors otherwise.
    fn hold_brief(
        &self,
        operation: &str,
        key: &str,
        vetting: &Vetting,
        at: f64,
    ) -> io::Result<Option<HeldBrief>>;

    /// The Operation's held briefs, oldest first, at most `limit` (1 to 500).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an Operation that is not a Registry key or a limit out of range;
    /// I/O errors otherwise. Unreadable records are skipped.
    fn held_briefs(&self, operation: &str, limit: usize) -> io::Result<Vec<HeldBrief>>;

    /// Returns a held brief to the queue, cleared of the kinds it was held for.
    ///
    /// # Errors
    ///
    /// `NotFound` when the Operation holds no such brief; `InvalidInput` for a bad key or
    /// actor; I/O errors otherwise.
    fn release_held(&self, operation: &str, key: &str, by: &str, at: f64) -> io::Result<Brief>;

    /// Files a held brief as failed with a `refused` outcome naming `by` and `reason`.
    ///
    /// # Errors
    ///
    /// As [`VettingStore::release_held`], and `InvalidInput` for a reason over 1024 bytes.
    fn refuse_held(
        &self,
        operation: &str,
        key: &str,
        by: &str,
        reason: &str,
        at: f64,
    ) -> io::Result<HeldBrief>;

    /// What a queued or held brief is cleared of, if anything.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a bad key; I/O errors otherwise.
    fn clearance(&self, key: &str) -> io::Result<Option<Clearance>>;

    /// Clears a queued brief of `kinds`, adding to what it was cleared of before.
    ///
    /// # Errors
    ///
    /// `NotFound` when no such brief is queued; `InvalidInput` for a bad key or actor; I/O
    /// errors otherwise.
    fn clear_brief(
        &self,
        key: &str,
        kinds: &[FindingKind],
        by: &str,
        at: f64,
    ) -> io::Result<Clearance>;
}

impl FileRegistry {
    fn held_path(&self, key: &str) -> PathBuf {
        self.root.join("held").join(format!("{key}.json"))
    }

    fn clearance_path(&self, key: &str) -> PathBuf {
        self.root.join("vetting").join(format!("{key}.json"))
    }

    fn held_in(&self, operation: &str, key: &str) -> io::Result<HeldBrief> {
        match read_held(&self.held_path(key))? {
            Some(held) if held.operation == operation && held.brief.key() == key => Ok(held),
            _ => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the Operation holds no such brief",
            )),
        }
    }
}

impl VettingStore for FileRegistry {
    fn hold_brief(
        &self,
        operation: &str,
        key: &str,
        vetting: &Vetting,
        at: f64,
    ) -> io::Result<Option<HeldBrief>> {
        validate_registry_key(operation)?;
        validate_registry_key(key)?;
        let _lock = registry_lock(&self.root)?;
        let queued = self.root.join("new").join(format!("{key}.json"));
        let brief = match fs::read(&queued) {
            Ok(bytes) => serde_json::from_slice::<Brief>(&bytes)
                .map_err(|error| invalid_data(error.to_string()))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if count_json(&self.root.join("held"))? >= MAX_HELD_BRIEFS {
            return Err(invalid_data(format!(
                "the Registry already holds {MAX_HELD_BRIEFS} briefs"
            )));
        }
        let held = HeldBrief {
            operation: operation.to_owned(),
            brief,
            vetting: vetting.clone(),
            held_at: at,
        };
        held.validate()?;
        let bytes = serde_json::to_vec_pretty(&held).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_HELD_RECORD_BYTES {
            return Err(invalid_data("held brief exceeds the size limit"));
        }
        write_atomic(&self.held_path(key), &bytes)?;
        fs::remove_file(&queued)?;
        Ok(Some(held))
    }

    fn held_briefs(&self, operation: &str, limit: usize) -> io::Result<Vec<HeldBrief>> {
        validate_registry_key(operation)?;
        check_limit(limit)?;
        let mut held = Vec::new();
        for path in json_files(&self.root.join("held"))? {
            if let Ok(Some(record)) = read_held(&path)
                && record.operation == operation
                && path.file_stem().and_then(|stem| stem.to_str()) == Some(&record.brief.key())
            {
                held.push(record);
            }
        }
        held.sort_by(|left, right| {
            left.held_at
                .total_cmp(&right.held_at)
                .then_with(|| left.brief.key().cmp(&right.brief.key()))
        });
        held.truncate(limit);
        Ok(held)
    }

    fn release_held(&self, operation: &str, key: &str, by: &str, at: f64) -> io::Result<Brief> {
        validate_registry_key(operation)?;
        validate_registry_key(key)?;
        check_actor(by, "")?;
        let _lock = registry_lock(&self.root)?;
        let held = self.held_in(operation, key)?;
        let kinds = held.vetting.kinds().into_iter().collect::<Vec<_>>();
        let clearance = merge_clearance(read_clearance(&self.clearance_path(key))?, &kinds, by, at);
        write_atomic(
            &self.clearance_path(key),
            &serde_json::to_vec(&clearance).map_err(io::Error::other)?,
        )?;
        write_atomic(
            &self.root.join("new").join(format!("{key}.json")),
            &serde_json::to_vec_pretty(&held.brief).map_err(io::Error::other)?,
        )?;
        fs::remove_file(self.held_path(key))?;
        Ok(held.brief)
    }

    fn refuse_held(
        &self,
        operation: &str,
        key: &str,
        by: &str,
        reason: &str,
        at: f64,
    ) -> io::Result<HeldBrief> {
        validate_registry_key(operation)?;
        validate_registry_key(key)?;
        check_actor(by, reason)?;
        let _lock = registry_lock(&self.root)?;
        let held = self.held_in(operation, key)?;
        finish_locked(
            &self.root,
            &held.brief,
            &refused_outcome(&held, by, reason, at),
            false,
            None,
        )?;
        fs::remove_file(self.held_path(key))?;
        Ok(held)
    }

    fn clearance(&self, key: &str) -> io::Result<Option<Clearance>> {
        validate_registry_key(key)?;
        read_clearance(&self.clearance_path(key))
    }

    fn clear_brief(
        &self,
        key: &str,
        kinds: &[FindingKind],
        by: &str,
        at: f64,
    ) -> io::Result<Clearance> {
        validate_registry_key(key)?;
        check_actor(by, "")?;
        let _lock = registry_lock(&self.root)?;
        if !self.root.join("new").join(format!("{key}.json")).is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no such brief is queued",
            ));
        }
        let clearance = merge_clearance(read_clearance(&self.clearance_path(key))?, kinds, by, at);
        write_atomic(
            &self.clearance_path(key),
            &serde_json::to_vec(&clearance).map_err(io::Error::other)?,
        )?;
        Ok(clearance)
    }
}

fn read_held(path: &Path) -> io::Result<Option<HeldBrief>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if bytes.len() as u64 > MAX_HELD_RECORD_BYTES {
        return Err(invalid_data("held brief exceeds the size limit"));
    }
    let held: HeldBrief =
        serde_json::from_slice(&bytes).map_err(|error| invalid_data(error.to_string()))?;
    held.validate()?;
    Ok(Some(held))
}

fn read_clearance(path: &Path) -> io::Result<Option<Clearance>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if bytes.len() as u64 > MAX_CLEARANCE_BYTES {
        return Err(invalid_data("clearance exceeds the size limit"));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| invalid_data(error.to_string()))
}

fn json_files(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            if files.len() > MAX_HELD_BRIEFS {
                return Err(invalid_data(format!(
                    "the Registry holds more than {MAX_HELD_BRIEFS} briefs"
                )));
            }
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn count_json(directory: &Path) -> io::Result<usize> {
    json_files(directory).map(|files| files.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::directorates::m::vetting::Finding;
    use crate::directorates::registry::suite::{Store, backend_tests, brief};

    backend_tests!(
        a_held_brief_leaves_the_queue_and_a_release_places_it_once_cleared,
        a_refused_brief_is_filed_as_failed_with_its_reason,
        clearances_accumulate_and_need_a_queued_brief,
        held_briefs_are_listed_per_operation_and_block_resubmission,
    );

    fn duplicate() -> Vetting {
        Vetting::from_findings(vec![
            Finding::new(FindingKind::Duplicate, "the same title as other work")
                .related("mission-1"),
        ])
    }

    fn a_held_brief_leaves_the_queue_and_a_release_places_it_once_cleared(store: &dyn Store) {
        let registry = store.handle();
        registry.submit_brief(&brief("a")).unwrap();
        let held = registry
            .hold_brief("G", "manual--a", &duplicate(), 10.0)
            .unwrap()
            .unwrap();
        assert_eq!(held.vetting, duplicate());
        assert!(registry.list_new().unwrap().is_empty());
        assert!(
            registry
                .claim_with_lease(None, "s", Duration::from_secs(60), Default::default())
                .unwrap()
                .is_none(),
            "a held brief is never claimed"
        );
        assert_eq!(
            registry
                .hold_brief("G", "manual--a", &duplicate(), 11.0)
                .unwrap(),
            None,
            "only a queued brief is held"
        );
        assert_eq!(
            registry
                .release_held("L", "manual--a", "ops", 12.0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound,
            "another Operation cannot release it"
        );
        let released = registry
            .release_held("G", "manual--a", "ops", 12.0)
            .unwrap();
        assert_eq!(released, brief("a"));
        assert_eq!(registry.list_new().unwrap(), [brief("a")]);
        let clearance = registry.clearance("manual--a").unwrap().unwrap();
        assert_eq!(clearance.kinds, BTreeSet::from([FindingKind::Duplicate]));
        assert_eq!(clearance.by, "ops");
        assert_eq!(
            registry
                .release_held("G", "manual--a", "ops", 13.0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        let lease = registry
            .claim_with_lease(None, "s", Duration::from_secs(60), Default::default())
            .unwrap()
            .unwrap();
        assert_eq!(lease.key(), "manual--a");
        assert!(registry.list_new().unwrap().is_empty(), "placed once");
    }

    fn a_refused_brief_is_filed_as_failed_with_its_reason(store: &dyn Store) {
        let registry = store.handle();
        registry.submit_brief(&brief("r")).unwrap();
        registry
            .hold_brief("G", "manual--r", &duplicate(), 10.0)
            .unwrap();
        assert_eq!(
            registry
                .refuse_held("G", "manual--r", "ops", &"x".repeat(2_000), 11.0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        let refused = registry
            .refuse_held("G", "manual--r", "ops", "repeats mission-1", 11.0)
            .unwrap();
        assert_eq!(refused.brief, brief("r"));
        assert!(registry.held_briefs("G", 10).unwrap().is_empty());
        assert_eq!(
            registry.submit_brief(&brief("r")).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists,
            "a refused brief is finished"
        );
    }

    fn clearances_accumulate_and_need_a_queued_brief(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(
            registry
                .clear_brief("manual--c", &[FindingKind::Review], "review:tanner", 1.0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        registry.submit_brief(&brief("c")).unwrap();
        assert_eq!(registry.clearance("manual--c").unwrap(), None);
        registry
            .clear_brief("manual--c", &[FindingKind::Review], "review:tanner", 1.0)
            .unwrap();
        let clearance = registry
            .clear_brief("manual--c", &[FindingKind::Rate], "ops", 2.0)
            .unwrap();
        assert_eq!(
            clearance.kinds,
            BTreeSet::from([FindingKind::Rate, FindingKind::Review])
        );
        assert_eq!(registry.clearance("manual--c").unwrap(), Some(clearance));
    }

    fn held_briefs_are_listed_per_operation_and_block_resubmission(store: &dyn Store) {
        let registry = store.handle();
        for (id, operation, at) in [("one", "G", 3.0), ("two", "G", 1.0), ("three", "L", 2.0)] {
            registry.submit_brief(&brief(id)).unwrap();
            registry
                .hold_brief(operation, &format!("manual--{id}"), &duplicate(), at)
                .unwrap();
        }
        let keys = |operation| {
            registry
                .held_briefs(operation, 10)
                .unwrap()
                .into_iter()
                .map(|held| held.brief.key())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys("G"), ["manual--two", "manual--one"]);
        assert_eq!(keys("L"), ["manual--three"]);
        assert_eq!(registry.held_briefs("G", 1).unwrap().len(), 1);
        assert!(registry.held_briefs("G", 0).is_err());
        assert_eq!(
            registry.submit_brief(&brief("one")).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }
}
