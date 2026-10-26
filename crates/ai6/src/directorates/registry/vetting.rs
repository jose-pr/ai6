//! Held briefs and vetting clearances.
//!
//! A brief that vetting holds leaves the queue for `held/KEY.json` with the [`Vetting`] that
//! held it, where no Station claims it. An operator releases it back to the queue, with a
//! [`Clearance`] for the kinds it was held for so the same checks do not hold it again, or
//! refuses it, which files it as failed with a `refused` outcome (a Source still gets its CX
//! report). A clearance is kept apart from the brief (`vetting/KEY.json`), so a brief's own
//! text cannot carry one, and it is bound to the brief's content ([`content_digest`]): once
//! the brief changes, it no longer clears anything. It also records a Chief of Staff review
//! that found nothing.
//!
//! Each change is safe to repeat after a crash part-way through it, and a held record left
//! beside a brief already claimed or finished is dropped rather than queued again.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::store::{
    Brief, FileRegistry, MAX_BRIEF_BYTES, finish_locked, invalid_data, registry_lock,
    validate_registry_key, write_atomic,
};
use crate::directorates::m::vetting::{FindingKind, Vetting, content_digest};

/// The most briefs one Operation holds at once; past it a brief stays queued.
pub const MAX_HELD_PER_OPERATION: usize = 1_000;
/// The most held briefs the Registry reads in one listing, across Operations.
pub const MAX_HELD_BRIEFS: usize = 10_000;
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

    /// Whether the rate is all it was held for.
    pub fn held_only_for_rate(&self) -> bool {
        self.vetting.kinds() == BTreeSet::from([FindingKind::Rate])
    }
}

/// Kinds of finding a brief no longer stops for, while its content stays as it was: released
/// by an operator, or a review that found nothing.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clearance {
    /// The kinds cleared.
    pub kinds: BTreeSet<FindingKind>,
    /// The [`content_digest`] of the brief they were cleared for.
    pub digest: String,
    /// Who cleared the last of them: a person, or `review:AGENT`.
    pub by: String,
    /// When, as Unix seconds.
    pub at: f64,
}

impl Clearance {
    /// The kinds this clearance clears for `brief` as it reads now: none once it changed.
    pub fn kinds_for(&self, brief: &Brief) -> BTreeSet<FindingKind> {
        if self.digest == content_digest(brief) {
            self.kinds.clone()
        } else {
            BTreeSet::new()
        }
    }
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

/// Adds `kinds` to an existing clearance of the same content, or starts one.
pub(super) fn merge_clearance(
    existing: Option<Clearance>,
    digest: &str,
    kinds: &[FindingKind],
    by: &str,
    at: f64,
) -> Clearance {
    let mut clearance = existing
        .filter(|existing| existing.digest == digest)
        .unwrap_or(Clearance {
            kinds: BTreeSet::new(),
            digest: digest.to_owned(),
            by: String::new(),
            at,
        });
    clearance.kinds.extend(kinds.iter().copied());
    clearance.by = by.to_owned();
    clearance.at = at;
    clearance
}

pub(super) fn already_decided() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        "the brief was already claimed or finished",
    )
}

pub(super) fn too_many_held(operation: &str) -> io::Error {
    invalid_data(format!(
        "Operation {operation:?} already holds {MAX_HELD_PER_OPERATION} briefs"
    ))
}

/// Held briefs and the clearances that keep vetting from holding a brief again.
pub trait VettingStore {
    /// Moves a queued brief to the held briefs of `operation` with the vetting that held it.
    /// `None` when the brief is no longer queued.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a key or Operation that is not a Registry key; `InvalidData` when
    /// the Operation already holds [`MAX_HELD_PER_OPERATION`]; I/O errors otherwise.
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

    /// Returns a held brief to the queue, cleared of the kinds it was held for. Releasing a
    /// brief already released returns it again.
    ///
    /// # Errors
    ///
    /// `NotFound` when the Operation neither holds nor released such a brief;
    /// `AlreadyExists` when it was claimed or finished meanwhile; `InvalidInput` for a bad key
    /// or actor; I/O errors otherwise.
    fn release_held(&self, operation: &str, key: &str, by: &str, at: f64) -> io::Result<Brief>;

    /// Files a held brief as failed with a `refused` outcome naming `by` and `reason`.
    /// Refusing a brief already refused returns it again.
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
    ) -> io::Result<Brief>;

    /// Returns a held brief to the queue without clearing anything, so it is vetted again:
    /// how a hold for the rate expires. `false` when it is not held, or was claimed or
    /// finished (its held record is then dropped).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a bad key; I/O errors otherwise.
    fn requeue_held(&self, operation: &str, key: &str) -> io::Result<bool>;

    /// What a queued or held brief is cleared of, if anything, for the content it had then.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a bad key; I/O errors otherwise.
    fn clearance(&self, key: &str) -> io::Result<Option<Clearance>>;

    /// Clears a queued brief of `kinds` for its content `digest`, adding to what that content
    /// was cleared of before. `None`, clearing nothing, when the brief no longer has that
    /// content.
    ///
    /// # Errors
    ///
    /// `NotFound` when no such brief is queued; `InvalidInput` for a bad key or actor; I/O
    /// errors otherwise.
    fn clear_brief(
        &self,
        key: &str,
        digest: &str,
        kinds: &[FindingKind],
        by: &str,
        at: f64,
    ) -> io::Result<Option<Clearance>>;

    /// Promotes a pending Mission proposal (as
    /// [`BriefStore::promote_mission_proposal`](super::store::BriefStore::promote_mission_proposal))
    /// and clears the queued brief of `kinds`, in one step: no intake round sees the brief
    /// uncleared.
    ///
    /// # Errors
    ///
    /// As `promote_mission_proposal`, and `InvalidInput` for a bad actor.
    fn promote_cleared(
        &self,
        operation: &str,
        proposal_id: &str,
        received_by: &str,
        kinds: &[FindingKind],
        by: &str,
        at: f64,
    ) -> io::Result<Brief>;
}

impl FileRegistry {
    fn held_path(&self, key: &str) -> PathBuf {
        self.root.join("held").join(format!("{key}.json"))
    }

    fn clearance_path(&self, key: &str) -> PathBuf {
        self.root.join("vetting").join(format!("{key}.json"))
    }

    fn queued_path(&self, key: &str) -> PathBuf {
        self.root.join("new").join(format!("{key}.json"))
    }

    /// The state a brief left the queue for, if it was claimed or finished.
    fn decided(&self, key: &str) -> Option<&'static str> {
        ["claimed", "done", "failed"]
            .into_iter()
            .find(|state| self.root.join(state).join(format!("{key}.json")).is_file())
    }

    fn held_by(&self, operation: &str, key: &str) -> io::Result<Option<HeldBrief>> {
        Ok(read_held(&self.held_path(key))?
            .filter(|held| held.operation == operation && held.brief.key() == key))
    }

    fn remove_held(&self, key: &str) -> io::Result<()> {
        match fs::remove_file(self.held_path(key)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn write_clearance(&self, key: &str, clearance: &Clearance) -> io::Result<()> {
        write_atomic(
            &self.clearance_path(key),
            &serde_json::to_vec(clearance).map_err(io::Error::other)?,
        )
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
        let queued = self.queued_path(key);
        let brief = match fs::read(&queued) {
            Ok(bytes) => serde_json::from_slice::<Brief>(&bytes)
                .map_err(|error| invalid_data(error.to_string()))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let held_here = held_records(&self.root.join("held"))?
            .iter()
            .filter(|held| held.operation == operation && held.brief.key() != key)
            .count();
        if held_here >= MAX_HELD_PER_OPERATION {
            return Err(too_many_held(operation));
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
        // Written before the queued copy goes, so a crash in between holds it twice, which
        // the next hold or release resolves, and never loses it.
        write_atomic(&self.held_path(key), &bytes)?;
        fs::remove_file(&queued)?;
        Ok(Some(held))
    }

    fn held_briefs(&self, operation: &str, limit: usize) -> io::Result<Vec<HeldBrief>> {
        validate_registry_key(operation)?;
        check_limit(limit)?;
        let mut held = held_records(&self.root.join("held"))?
            .into_iter()
            .filter(|held| held.operation == operation)
            .collect::<Vec<_>>();
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
        if self.decided(key).is_some() {
            self.remove_held(key)?;
            return Err(already_decided());
        }
        let Some(held) = self.held_by(operation, key)? else {
            // Released already: the brief is queued with a clearance for what it is.
            if let Some(brief) = read_brief(&self.queued_path(key))?
                && read_clearance(&self.clearance_path(key))?
                    .is_some_and(|clearance| !clearance.kinds_for(&brief).is_empty())
            {
                return Ok(brief);
            }
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the Operation holds no such brief",
            ));
        };
        let kinds = held.vetting.kinds().into_iter().collect::<Vec<_>>();
        let digest = content_digest(&held.brief);
        let clearance = merge_clearance(
            read_clearance(&self.clearance_path(key))?,
            &digest,
            &kinds,
            by,
            at,
        );
        self.write_clearance(key, &clearance)?;
        write_atomic(
            &self.queued_path(key),
            &serde_json::to_vec_pretty(&held.brief).map_err(io::Error::other)?,
        )?;
        self.remove_held(key)?;
        Ok(held.brief)
    }

    fn refuse_held(
        &self,
        operation: &str,
        key: &str,
        by: &str,
        reason: &str,
        at: f64,
    ) -> io::Result<Brief> {
        validate_registry_key(operation)?;
        validate_registry_key(key)?;
        check_actor(by, reason)?;
        let _lock = registry_lock(&self.root)?;
        match self.decided(key) {
            Some("failed") if refused(&self.root, key) => {
                self.remove_held(key)?;
                return read_brief(&self.root.join("failed").join(format!("{key}.json")))?
                    .ok_or_else(already_decided);
            }
            Some(_) => {
                self.remove_held(key)?;
                return Err(already_decided());
            }
            None => {}
        }
        let held = self.held_by(operation, key)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "the Operation holds no such brief")
        })?;
        finish_locked(
            &self.root,
            &held.brief,
            &refused_outcome(&held, by, reason, at),
            false,
            None,
        )?;
        // A copy left queued by an interrupted hold must not run after the refusal.
        if let Err(error) = fs::remove_file(self.queued_path(key))
            && error.kind() != io::ErrorKind::NotFound
        {
            return Err(error);
        }
        self.remove_held(key)?;
        Ok(held.brief)
    }

    fn requeue_held(&self, operation: &str, key: &str) -> io::Result<bool> {
        validate_registry_key(operation)?;
        validate_registry_key(key)?;
        let _lock = registry_lock(&self.root)?;
        let Some(held) = self.held_by(operation, key)? else {
            return Ok(false);
        };
        if self.decided(key).is_some() {
            self.remove_held(key)?;
            return Ok(false);
        }
        write_atomic(
            &self.queued_path(key),
            &serde_json::to_vec_pretty(&held.brief).map_err(io::Error::other)?,
        )?;
        self.remove_held(key)?;
        Ok(true)
    }

    fn clearance(&self, key: &str) -> io::Result<Option<Clearance>> {
        validate_registry_key(key)?;
        read_clearance(&self.clearance_path(key))
    }

    fn clear_brief(
        &self,
        key: &str,
        digest: &str,
        kinds: &[FindingKind],
        by: &str,
        at: f64,
    ) -> io::Result<Option<Clearance>> {
        validate_registry_key(key)?;
        check_actor(by, "")?;
        let _lock = registry_lock(&self.root)?;
        let brief = read_brief(&self.queued_path(key))?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such brief is queued"))?;
        if content_digest(&brief) != digest {
            return Ok(None);
        }
        let clearance = merge_clearance(
            read_clearance(&self.clearance_path(key))?,
            digest,
            kinds,
            by,
            at,
        );
        self.write_clearance(key, &clearance)?;
        Ok(Some(clearance))
    }

    fn promote_cleared(
        &self,
        operation: &str,
        proposal_id: &str,
        received_by: &str,
        kinds: &[FindingKind],
        by: &str,
        at: f64,
    ) -> io::Result<Brief> {
        validate_registry_key(operation)?;
        validate_registry_key(proposal_id)?;
        check_actor(by, "")?;
        let _lock = registry_lock(&self.root)?;
        let brief = self.promote_locked(operation, proposal_id, received_by)?;
        let key = brief.key();
        if self.queued_path(&key).is_file() {
            let digest = content_digest(&brief);
            let clearance = merge_clearance(
                read_clearance(&self.clearance_path(&key))?,
                &digest,
                kinds,
                by,
                at,
            );
            self.write_clearance(&key, &clearance)?;
        }
        Ok(brief)
    }
}

/// Whether the brief `key` was filed as failed with a `refused` outcome.
fn refused(root: &Path, key: &str) -> bool {
    fs::read(root.join("failed").join(format!("{key}.outcome.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|outcome| outcome["state"] == "refused")
}

fn read_brief(path: &Path) -> io::Result<Option<Brief>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| invalid_data(error.to_string())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
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

/// Every readable held record, skipping files that are not one or not named by their key.
fn held_records(directory: &Path) -> io::Result<Vec<HeldBrief>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || path.extension().is_none_or(|extension| extension != "json")
        {
            continue;
        }
        if records.len() >= MAX_HELD_BRIEFS {
            return Err(invalid_data(format!(
                "the Registry holds more than {MAX_HELD_BRIEFS} briefs"
            )));
        }
        if let Ok(Some(held)) = read_held(&path)
            && path.file_stem().and_then(|stem| stem.to_str()) == Some(&held.brief.key())
        {
            records.push(held);
        }
    }
    Ok(records)
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
        clearances_accumulate_for_one_content_and_need_a_queued_brief,
        held_briefs_are_listed_per_operation_and_block_resubmission,
        a_pending_proposal_keeps_its_vetting_across_a_repeated_submission,
        a_promotion_clears_the_brief_in_the_same_step,
        a_brief_claimed_or_finished_is_never_released_or_refused,
        a_held_brief_requeued_is_vetted_again_without_a_clearance,
    );

    fn proposal(key: &str) -> crate::directorates::registry::store::MissionProposal {
        use crate::directorates::m::identity::{MissionId, SessionId};
        crate::directorates::registry::store::MissionProposal::try_new(
            "G",
            MissionId::try_new("parent").unwrap(),
            "coordinator",
            SessionId::try_new("session-1").unwrap(),
            key,
            brief("child"),
        )
        .unwrap()
    }

    fn a_pending_proposal_keeps_its_vetting_across_a_repeated_submission(store: &dyn Store) {
        let registry = store.handle();
        let proposal = proposal("retry-1");
        registry.submit_mission_proposal(&proposal).unwrap();
        let updated = registry
            .set_proposal_vetting("G", &proposal.id, &duplicate())
            .unwrap();
        assert_eq!(updated.vetting, Some(duplicate()));
        registry.submit_mission_proposal(&proposal).unwrap();
        let listed = registry.list_mission_proposals("G", 10).unwrap();
        assert_eq!(
            listed[0].vetting,
            Some(duplicate()),
            "a retry keeps the vetting"
        );
        registry
            .promote_mission_proposal("G", &proposal.id, "hq")
            .unwrap();
        assert_eq!(
            registry
                .set_proposal_vetting("G", &proposal.id, &Vetting::pass())
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound,
            "only a pending proposal is vetted"
        );
    }

    fn a_promotion_clears_the_brief_in_the_same_step(store: &dyn Store) {
        let registry = store.handle();
        let proposal = proposal("retry-2");
        registry.submit_mission_proposal(&proposal).unwrap();
        let brief = registry
            .promote_cleared(
                "G",
                &proposal.id,
                "hq",
                &[FindingKind::Screening],
                "ops",
                5.0,
            )
            .unwrap();
        let clearance = registry.clearance(&brief.key()).unwrap().unwrap();
        assert_eq!(
            clearance.kinds_for(&brief),
            BTreeSet::from([FindingKind::Screening])
        );
        assert_eq!(clearance.by, "ops");
        assert_eq!(registry.list_new().unwrap(), [brief]);
    }

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
        assert_eq!(
            clearance.kinds_for(&brief("a")),
            BTreeSet::from([FindingKind::Duplicate])
        );
        assert_eq!(clearance.by, "ops");
        assert_eq!(
            registry
                .release_held("G", "manual--a", "ops", 13.0)
                .unwrap(),
            brief("a"),
            "releasing again is harmless"
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
        assert_eq!(refused, brief("r"));
        assert!(registry.held_briefs("G", 10).unwrap().is_empty());
        assert_eq!(
            registry
                .refuse_held("G", "manual--r", "ops", "again", 12.0)
                .unwrap(),
            brief("r"),
            "refusing again is harmless"
        );
        assert_eq!(
            registry.submit_brief(&brief("r")).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists,
            "a refused brief is finished"
        );
    }

    fn clearances_accumulate_for_one_content_and_need_a_queued_brief(store: &dyn Store) {
        let registry = store.handle();
        let digest = content_digest(&brief("c"));
        assert_eq!(
            registry
                .clear_brief(
                    "manual--c",
                    &digest,
                    &[FindingKind::Review],
                    "review:t",
                    1.0
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        registry.submit_brief(&brief("c")).unwrap();
        assert_eq!(registry.clearance("manual--c").unwrap(), None);
        assert_eq!(
            registry
                .clear_brief(
                    "manual--c",
                    "other",
                    &[FindingKind::Review],
                    "review:t",
                    1.0
                )
                .unwrap(),
            None,
            "a clearance for other content clears nothing"
        );
        registry
            .clear_brief(
                "manual--c",
                &digest,
                &[FindingKind::Review],
                "review:t",
                1.0,
            )
            .unwrap();
        let clearance = registry
            .clear_brief("manual--c", &digest, &[FindingKind::Rate], "ops", 2.0)
            .unwrap()
            .unwrap();
        assert_eq!(
            clearance.kinds,
            BTreeSet::from([FindingKind::Rate, FindingKind::Review])
        );
        assert_eq!(
            registry.clearance("manual--c").unwrap(),
            Some(clearance.clone())
        );
        let mut edited = brief("c");
        edited.body = "changed".to_owned();
        assert!(
            clearance.kinds_for(&edited).is_empty(),
            "bound to the content"
        );
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

    fn a_brief_claimed_or_finished_is_never_released_or_refused(store: &dyn Store) {
        let registry = store.handle();
        registry.submit_brief(&brief("x")).unwrap();
        registry
            .hold_brief("G", "manual--x", &duplicate(), 1.0)
            .unwrap();
        registry
            .refuse_held("G", "manual--x", "ops", "no", 2.0)
            .unwrap();
        assert_eq!(
            registry
                .release_held("G", "manual--x", "ops", 3.0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists,
            "a refused brief cannot be released"
        );
        assert!(registry.list_new().unwrap().is_empty());
    }

    fn a_held_brief_requeued_is_vetted_again_without_a_clearance(store: &dyn Store) {
        let registry = store.handle();
        registry.submit_brief(&brief("q")).unwrap();
        let rate = Vetting::from_findings(vec![Finding::new(FindingKind::Rate, "too many")]);
        let held = registry
            .hold_brief("G", "manual--q", &rate, 1.0)
            .unwrap()
            .unwrap();
        assert!(held.held_only_for_rate());
        assert!(!registry.requeue_held("L", "manual--q").unwrap());
        assert!(registry.requeue_held("G", "manual--q").unwrap());
        assert_eq!(registry.list_new().unwrap(), [brief("q")]);
        assert_eq!(registry.clearance("manual--q").unwrap(), None);
        assert!(!registry.requeue_held("G", "manual--q").unwrap());
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;

    /// A crash between writing the held record and removing the queued copy leaves both; the
    /// next hold resolves it, and a release after the copy was claimed queues nothing.
    #[test]
    fn an_interrupted_hold_is_resolved_and_never_runs_twice() {
        use crate::directorates::registry::backend::*;
        use crate::directorates::registry::suite::brief;
        use std::time::Duration;

        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        registry.submit_brief(&brief("crash")).unwrap();
        let vetting = Vetting::from_findings(vec![crate::directorates::m::vetting::Finding::new(
            FindingKind::Duplicate,
            "same",
        )]);
        registry
            .hold_brief("G", "manual--crash", &vetting, 1.0)
            .unwrap();
        // The crash: the queued copy is back beside the held record.
        write_atomic(
            &registry.queued_path("manual--crash"),
            &serde_json::to_vec(&brief("crash")).unwrap(),
        )
        .unwrap();
        assert!(
            registry
                .hold_brief("G", "manual--crash", &vetting, 2.0)
                .unwrap()
                .is_some()
        );
        assert!(registry.list_new().unwrap().is_empty());

        write_atomic(
            &registry.queued_path("manual--crash"),
            &serde_json::to_vec(&brief("crash")).unwrap(),
        )
        .unwrap();
        let lease = registry
            .claim_with_lease(None, "s", Duration::from_secs(60), Default::default())
            .unwrap()
            .unwrap();
        assert_eq!(lease.key(), "manual--crash");
        assert_eq!(
            registry
                .release_held("G", "manual--crash", "ops", 3.0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(
            registry.held_briefs("G", 10).unwrap().is_empty(),
            "stale record dropped"
        );
        assert!(
            registry.list_new().unwrap().is_empty(),
            "nothing queued twice"
        );
    }

    #[test]
    fn an_operation_holds_at_most_its_own_limit() {
        use crate::directorates::registry::suite::brief;
        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        let vetting = Vetting::from_findings(vec![crate::directorates::m::vetting::Finding::new(
            FindingKind::Duplicate,
            "same",
        )]);
        for index in 0..MAX_HELD_PER_OPERATION {
            let held = HeldBrief {
                operation: "G".to_owned(),
                brief: brief(&format!("h{index}")),
                vetting: vetting.clone(),
                held_at: 1.0,
            };
            write_atomic(
                &registry.held_path(&held.brief.key()),
                &serde_json::to_vec(&held).unwrap(),
            )
            .unwrap();
        }
        use crate::directorates::registry::backend::*;
        registry.submit_brief(&brief("g")).unwrap();
        registry.submit_brief(&brief("l")).unwrap();
        assert!(
            registry
                .hold_brief("G", "manual--g", &vetting, 2.0)
                .is_err(),
            "G is full"
        );
        assert!(
            registry
                .hold_brief("L", "manual--l", &vetting, 2.0)
                .unwrap()
                .is_some(),
            "another Operation is not"
        );
    }
}
