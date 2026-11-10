//! What one connection knows of its account's usage: the report a client reads with
//! `_coqui_acp/usage`, kept up to date from the harness's events.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use std::time::{SystemTime, UNIX_EPOCH};

use coqui_acp_api::usage::{Bucket, LimitHit, Refill, Spend, Status};
use coqui_acp_api::{Coverage, UsageReport, UsageSupport};

/// Unix seconds now.
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

/// The report of one account as a connection or a host sees it: its buckets, the limit last hit and
/// each session's spend, with the rules that merge what a reader reports. A bucket replaces the earlier
/// of its id and keeps what it leaves unknown; a change bumps the revision and a repeat moves only
/// `observed_at`; a limit exhausts the bucket it names until its reset passes or a later reading shows
/// room.
#[derive(Debug)]
pub struct UsageTracker {
    support: UsageSupport,
    notify: bool,
    report: UsageReport,
    spends: BTreeMap<String, Spend>,
}

impl UsageTracker {
    /// A report of what `support` says is reported, for `account` when one is known.
    #[must_use]
    pub fn new(support: UsageSupport, account: Option<String>) -> Self {
        let mut report = UsageReport::new();
        report.sources.clone_from(&support.sources);
        report.account = account;
        Self {
            support,
            notify: false,
            report,
            spends: BTreeMap::new(),
        }
    }

    /// Whether the harness reports usage at all.
    #[must_use]
    pub fn reports(&self) -> bool {
        self.support.reports()
    }

    /// Whether the client asked for `_coqui_acp/usage_update`.
    #[must_use]
    pub const fn notifies(&self) -> bool {
        self.notify
    }

    /// What the client said at `initialize`. A host's own account, set in the settings, stays.
    pub fn client(&mut self, notify: bool, account: Option<String>) {
        self.notify = notify;
        if self.report.account.is_none() {
            self.report.account = account;
        }
    }

    /// The report as of `now`, for `session` when one is named: with that session's spend, and
    /// without a limit whose reset has passed.
    #[must_use]
    pub fn snapshot(&self, session: Option<&str>, now: i64) -> UsageReport {
        let mut report = self.report.clone();
        if report
            .limit
            .as_ref()
            .is_some_and(|hit| hit.resets_at.is_some_and(|at| at <= now))
        {
            report.limit = None;
        }
        if self.support.spend {
            report.spend = session.and_then(|session| self.spends.get(session).cloned());
        }
        report
    }

    /// Takes the buckets a harness reported: one replaces the earlier of its id, keeping what the
    /// new one leaves unknown. Answers whether the report changed, other than in when it was read.
    pub fn buckets(&mut self, incoming: Vec<Bucket>, now: i64) -> bool {
        let mut changed = false;
        for mut bucket in incoming {
            if bucket.observed_at == 0 {
                bucket.observed_at = now;
            }
            let id = bucket.id.clone();
            let room = matches!(bucket.status, Status::Ok | Status::Warning);
            match self.report.buckets.iter_mut().find(|held| held.id == id) {
                Some(held) => {
                    let merged = merge(held, bucket);
                    let mut probe = merged.clone();
                    probe.observed_at = held.observed_at;
                    changed |= probe != *held;
                    *held = merged;
                }
                None => {
                    self.report.buckets.push(bucket);
                    changed = true;
                }
            }
            if room
                && self
                    .report
                    .limit
                    .as_ref()
                    .is_some_and(|hit| hit.bucket.as_deref() == Some(id.as_str()))
            {
                self.report.limit = None;
                changed = true;
            }
        }
        self.settle(changed)
    }

    /// Takes a complete reading of the buckets whose ids start with `prefix`: each replaces the held
    /// bucket of its id outright, so nothing of an earlier reading (a reset that has passed) comes
    /// back, and a held bucket of that prefix the reading leaves out is dropped. Buckets of other ids
    /// are not touched. Answers whether the report changed other than in when it was read.
    pub fn replace(&mut self, prefix: &str, incoming: Vec<Bucket>, now: i64) -> bool {
        let before = self.report.buckets.len();
        self.report.buckets.retain(|held| {
            !held.id.starts_with(prefix) || incoming.iter().any(|bucket| bucket.id == held.id)
        });
        let mut changed = self.report.buckets.len() != before;
        for mut bucket in incoming {
            if bucket.observed_at == 0 {
                bucket.observed_at = now;
            }
            let room = matches!(bucket.status, Status::Ok | Status::Warning);
            let id = bucket.id.clone();
            match self.report.buckets.iter_mut().find(|held| held.id == id) {
                Some(held) => {
                    let mut probe = bucket.clone();
                    probe.observed_at = held.observed_at;
                    changed |= probe != *held;
                    *held = bucket;
                }
                None => {
                    self.report.buckets.push(bucket);
                    changed = true;
                }
            }
            if room && self.limit_names(&id) {
                self.report.limit = None;
                changed = true;
            }
        }
        self.settle(changed)
    }

    /// Whether the report's limit names the bucket `id`.
    fn limit_names(&self, id: &str) -> bool {
        self.report
            .limit
            .as_ref()
            .is_some_and(|hit| hit.bucket.as_deref() == Some(id))
    }

    /// Takes a session's spend, as running totals.
    pub fn spend(&mut self, session: &str, mut spend: Spend) -> bool {
        if !self.support.spend {
            return false;
        }
        session.clone_into(&mut spend.session_id);
        if self.spends.get(session) == Some(&spend) {
            return false;
        }
        self.spends.insert(session.into(), spend);
        self.settle(true)
    }

    /// Takes a refusal: it is the report's limit, and the bucket it names is exhausted.
    pub fn limit(&mut self, mut hit: LimitHit, now: i64) -> bool {
        stamp(&mut hit, now);
        if self.report.limit.as_ref() == Some(&hit) {
            return false;
        }
        if let Some(id) = &hit.bucket {
            match self.report.buckets.iter_mut().find(|held| &held.id == id) {
                Some(held) => {
                    held.status = Status::Exhausted;
                    held.observed_at = held.observed_at.max(hit.observed_at);
                }
                None => {
                    let mut bucket = Bucket::new(id.clone());
                    bucket.status = Status::Exhausted;
                    bucket.source = hit.source.clone();
                    bucket.observed_at = if hit.observed_at == 0 {
                        now
                    } else {
                        hit.observed_at
                    };
                    bucket.refill = Refill::FixedWindow {
                        length: None,
                        resets_at: hit.resets_at,
                    };
                    self.report.buckets.push(bucket);
                }
            }
        }
        self.report.limit = Some(hit);
        self.settle(true)
    }

    /// Bumps the revision when something changed, and keeps `observed_at` the newest bucket's.
    fn settle(&mut self, changed: bool) -> bool {
        self.report.observed_at = self
            .report
            .buckets
            .iter()
            .map(|bucket| bucket.observed_at)
            .max()
            .unwrap_or(0);
        if changed {
            self.report.revision += 1;
        }
        changed
    }
}

/// Dates a refusal that was not dated. `retry_after` is the harness's own word (a 429's header) and is
/// never worked out from `resets_at`.
pub(crate) fn stamp(hit: &mut LimitHit, now: i64) {
    if hit.observed_at == 0 {
        hit.observed_at = now;
    }
}

/// `new` over `old`: what `new` leaves unknown, `old` keeps.
fn merge(old: &Bucket, new: Bucket) -> Bucket {
    let mut merged = new;
    if merged.label.is_none() {
        merged.label.clone_from(&old.label);
    }
    if merged.models == Coverage::Unknown {
        merged.models = old.models.clone();
    }
    merged.used = merged.used.or(old.used);
    merged.limit = merged.limit.or(old.limit);
    merged.remaining = merged.remaining.or(old.remaining);
    merged.refill = match (merged.refill, &old.refill) {
        (Refill::Unknown, kept) => kept.clone(),
        (
            Refill::FixedWindow { length, resets_at },
            Refill::FixedWindow {
                length: old_length,
                resets_at: old_resets,
            },
        ) => Refill::FixedWindow {
            length: length.or(*old_length),
            resets_at: resets_at.or(*old_resets),
        },
        (other, _) => other,
    };
    if merged.status == Status::Unknown {
        merged.status = old.status.clone();
    }
    merged.observed_at = merged.observed_at.max(old.observed_at);
    if merged.evidence.is_none() {
        merged.evidence.clone_from(&old.evidence);
    }
    merged
}

#[cfg(test)]
mod tests {
    use coqui_acp_api::Source;

    use super::*;

    fn support() -> UsageSupport {
        UsageSupport::new(alloc::vec![Source::Reported], true)
    }

    fn five_hour(used: u64) -> Bucket {
        let mut bucket = Bucket::new("five_hour");
        bucket.limit = Some(1000);
        bucket.used = Some(used);
        bucket.status = Status::Ok;
        bucket.refill = Refill::FixedWindow {
            length: Some(core::time::Duration::from_secs(18_000)),
            resets_at: Some(1_000_000),
        };
        bucket.observed_at = 100;
        bucket
    }

    #[test]
    fn a_change_bumps_the_revision_and_a_repeat_does_not() {
        let mut tracker = UsageTracker::new(support(), None);
        assert!(tracker.buckets(alloc::vec![five_hour(620)], 100));
        assert_eq!(tracker.snapshot(None, 100).revision, 1);
        let mut again = five_hour(620);
        again.observed_at = 200;
        assert!(!tracker.buckets(alloc::vec![again], 200));
        let report = tracker.snapshot(None, 200);
        assert_eq!((report.revision, report.observed_at), (1, 200));
        assert!(tracker.buckets(alloc::vec![five_hour(700)], 300));
        assert_eq!(tracker.snapshot(None, 300).revision, 2);
    }

    #[test]
    fn a_sparse_update_keeps_the_fields_it_lacks() {
        let mut tracker = UsageTracker::new(support(), None);
        tracker.buckets(alloc::vec![five_hour(620)], 100);
        let mut sparse = Bucket::new("five_hour");
        sparse.used = Some(640);
        sparse.status = Status::Warning;
        sparse.observed_at = 150;
        tracker.buckets(alloc::vec![sparse], 150);
        let report = tracker.snapshot(None, 150);
        let held = &report.buckets[0];
        assert_eq!((held.used, held.limit), (Some(640), Some(1000)));
        assert_eq!(held.status, Status::Warning);
        assert!(matches!(
            held.refill,
            Refill::FixedWindow {
                resets_at: Some(1_000_000),
                ..
            }
        ));
    }

    #[test]
    fn a_limit_exhausts_its_bucket_until_room_or_its_reset() {
        let mut tracker = UsageTracker::new(support(), Some("a".into()));
        tracker.buckets(alloc::vec![five_hour(620)], 100);
        let mut hit = LimitHit::new(200);
        hit.bucket = Some("five_hour".into());
        hit.resets_at = Some(900);
        assert!(tracker.limit(hit.clone(), 200));
        assert!(!tracker.limit(hit, 200), "the same refusal changes nothing");
        let report = tracker.snapshot(None, 200);
        assert_eq!(report.buckets[0].status, Status::Exhausted);
        assert!(report.limit.is_some());
        assert!(
            tracker.snapshot(None, 900).limit.is_none(),
            "its reset passed"
        );
        tracker.buckets(alloc::vec![five_hour(10)], 950);
        assert!(
            tracker.snapshot(None, 800).limit.is_none(),
            "room was reported"
        );
    }

    #[test]
    fn spend_is_kept_per_session_and_only_for_a_harness_that_reports_it() {
        let mut tracker = UsageTracker::new(support(), None);
        let mut spend = Spend::new("");
        spend.input = 5;
        assert!(tracker.spend("s1", spend.clone()));
        assert!(!tracker.spend("s1", spend));
        assert_eq!(
            tracker
                .snapshot(Some("s1"), 0)
                .spend
                .map(|s| (s.session_id, s.input)),
            Some(("s1".into(), 5))
        );
        assert!(tracker.snapshot(Some("s2"), 0).spend.is_none());
        assert!(tracker.snapshot(None, 0).spend.is_none());
        let mut bare = UsageTracker::new(
            UsageSupport::new(alloc::vec![Source::Reported], false),
            None,
        );
        assert!(!bare.spend("s1", Spend::new("")));
    }

    #[test]
    fn a_hit_is_dated_and_keeps_the_wait_it_came_with_and_never_gains_one() {
        let mut hit = LimitHit::new(0);
        hit.resets_at = Some(1_300);
        stamp(&mut hit, 1_000);
        assert_eq!((hit.observed_at, hit.retry_after), (1_000, None));

        let mut told = LimitHit::new(900);
        told.resets_at = Some(5_000);
        told.retry_after = Some(core::time::Duration::from_secs(7));
        stamp(&mut told, 1_000);
        assert_eq!(
            (told.observed_at, told.retry_after),
            (900, Some(core::time::Duration::from_secs(7)))
        );
    }

    #[test]
    fn a_replacing_reading_drops_the_buckets_it_leaves_out_and_those_of_other_ids_stay() {
        let mut tracker = UsageTracker::new(support(), None);
        let mut other = five_hour(10);
        other.id = "five_hour".into();
        let mut split = five_hour(0);
        split.id = "agy:b".into();
        split.status = Status::Exhausted;
        let mut kept = five_hour(0);
        kept.id = "agy:a".into();
        tracker.buckets(alloc::vec![other, split, kept.clone()], 100);

        kept.observed_at = 200;
        assert!(tracker.replace("agy:", alloc::vec![kept], 200));
        let ids: Vec<_> = tracker
            .snapshot(None, 200)
            .buckets
            .into_iter()
            .map(|bucket| bucket.id)
            .collect();
        assert_eq!(ids, ["five_hour", "agy:a"]);
    }

    #[test]
    fn a_replacing_reading_does_not_bring_back_a_reset_that_passed() {
        let mut tracker = UsageTracker::new(support(), None);
        let mut used = five_hour(400);
        used.id = "agy:a".into();
        tracker.replace("agy:", alloc::vec![used.clone()], 100);

        used.refill = Refill::FixedWindow {
            length: Some(core::time::Duration::from_secs(18_000)),
            resets_at: None,
        };
        used.observed_at = 2_000_000;
        assert!(tracker.replace("agy:", alloc::vec![used], 2_000_000));
        assert!(matches!(
            tracker.snapshot(None, 2_000_000).buckets[0].refill,
            Refill::FixedWindow {
                resets_at: None,
                ..
            }
        ));
    }

    #[test]
    fn a_replacing_reading_that_shows_room_clears_the_limit() {
        let mut tracker = UsageTracker::new(support(), None);
        let mut bucket = five_hour(0);
        bucket.id = "agy:a".into();
        tracker.replace("agy:", alloc::vec![bucket.clone()], 100);
        let mut hit = LimitHit::new(150);
        hit.bucket = Some("agy:a".into());
        tracker.limit(hit, 150);
        bucket.observed_at = 300;
        tracker.replace("agy:", alloc::vec![bucket], 300);
        assert!(tracker.snapshot(None, 300).limit.is_none());
    }
}
