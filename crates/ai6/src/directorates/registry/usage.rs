//! The usage ledger: what every Station's Sessions have consumed, per Mission.
//!
//! One file per Mission (`usage/OPERATION/MISSION.json`) holds each Session's totals and
//! the Mission's cost per UTC day. Writes happen under the Registry lock, so concurrent
//! Stations never lose an addition, and every Station reads the same spend.

use std::fs;
use std::io;
use std::path::PathBuf;

use super::store::{FileRegistry, invalid_data, registry_lock, safe_component, write_atomic};
use crate::directorates::medical::usage::{MissionUsage, UsageTotals, utc_day};

const MAX_LEDGER_BYTES: u64 = 1024 * 1024;
const KEPT_DAYS: usize = 8;
const NO_OPERATION: &str = "_";

impl FileRegistry {
    fn usage_path(&self, operation: Option<&str>, mission: &str) -> PathBuf {
        self.root
            .join("usage")
            .join(safe_component(operation.unwrap_or(NO_OPERATION)))
            .join(format!("{}.json", safe_component(mission)))
    }
}

/// Usage ledgers per Mission and spending per Operation and day.
pub trait UsageStore {
    /// Adds `delta` to a Session's totals in its Mission's ledger and returns the Mission's
    /// usage afterwards. `now` (Unix seconds) picks the UTC day the cost counts toward.
    ///
    /// # Errors
    ///
    /// Returns an error when the ledger cannot be read or written.
    fn record_usage(
        &self,
        operation: Option<&str>,
        mission: &str,
        session: &str,
        delta: &UsageTotals,
        now: i64,
    ) -> io::Result<MissionUsage>;

    /// Returns a Mission's recorded usage, or `None` when nothing was recorded.
    ///
    /// # Errors
    ///
    /// Returns an error when the ledger exists but cannot be read.
    fn mission_usage(
        &self,
        operation: Option<&str>,
        mission: &str,
    ) -> io::Result<Option<MissionUsage>>;

    /// Returns what an Operation's Missions spent on the UTC day containing `now`, in
    /// micro-dollars.
    ///
    /// # Errors
    ///
    /// Returns an error when the Operation's ledger directory cannot be read.
    fn operation_cost_on_day(&self, operation: Option<&str>, now: i64) -> io::Result<u64>;
}

impl UsageStore for FileRegistry {
    fn record_usage(
        &self,
        operation: Option<&str>,
        mission: &str,
        session: &str,
        delta: &UsageTotals,
        now: i64,
    ) -> io::Result<MissionUsage> {
        let _lock = registry_lock(&self.root)?;
        let path = self.usage_path(operation, mission);
        let mut usage = read_ledger(&path)?.unwrap_or_default();
        add_usage(&mut usage, session, delta, now);
        let bytes = serde_json::to_vec(&usage).map_err(io::Error::other)?;
        write_atomic(&path, &bytes)?;
        Ok(usage)
    }

    fn mission_usage(
        &self,
        operation: Option<&str>,
        mission: &str,
    ) -> io::Result<Option<MissionUsage>> {
        read_ledger(&self.usage_path(operation, mission))
    }

    fn operation_cost_on_day(&self, operation: Option<&str>, now: i64) -> io::Result<u64> {
        let directory = self
            .root
            .join("usage")
            .join(safe_component(operation.unwrap_or(NO_OPERATION)));
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        let mut total = 0_u64;
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if let Some(usage) = read_ledger(&path)? {
                total = total.saturating_add(cost_on_day(&usage, now));
            }
        }
        Ok(total)
    }
}

/// Adds `delta` to a Session's totals and the Mission's, and its cost to the UTC day of
/// `now`, keeping the last few days.
pub(super) fn add_usage(usage: &mut MissionUsage, session: &str, delta: &UsageTotals, now: i64) {
    usage.total.add(delta);
    usage
        .sessions
        .entry(session.to_owned())
        .or_default()
        .add(delta);
    if delta.cost_micro_usd > 0 {
        let day = usage.daily_cost_micro_usd.entry(utc_day(now)).or_default();
        *day = day.saturating_add(delta.cost_micro_usd);
        while usage.daily_cost_micro_usd.len() > KEPT_DAYS {
            usage.daily_cost_micro_usd.pop_first();
        }
    }
}

/// What a ledger holds for the UTC day of `now`.
pub(super) fn cost_on_day(usage: &MissionUsage, now: i64) -> u64 {
    usage
        .daily_cost_micro_usd
        .get(&utc_day(now))
        .copied()
        .unwrap_or_default()
}

fn read_ledger(path: &std::path::Path) -> io::Result<Option<MissionUsage>> {
    match fs::read(path) {
        Ok(bytes) => parse_ledger(&bytes).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Reads a stored ledger, refusing one over the size limit.
pub(super) fn parse_ledger(bytes: &[u8]) -> io::Result<MissionUsage> {
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return Err(invalid_data("usage ledger exceeds the size limit"));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| invalid_data(format!("invalid usage ledger: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(additions_accumulate_per_session_and_mission_and_day,);

    fn totals(input: u64, cost: u64) -> UsageTotals {
        UsageTotals {
            input,
            cost_micro_usd: cost,
            ..UsageTotals::default()
        }
    }

    fn additions_accumulate_per_session_and_mission_and_day(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(registry.mission_usage(Some("op"), "m1").unwrap(), None);
        registry
            .record_usage(Some("op"), "m1", "s1", &totals(100, 1_000), 1_790_000_000)
            .unwrap();
        registry
            .record_usage(Some("op"), "m1", "s2", &totals(50, 500), 1_790_000_100)
            .unwrap();
        let usage = registry
            .record_usage(Some("op"), "m1", "s1", &totals(10, 100), 1_790_086_400)
            .unwrap();
        assert_eq!(usage.total.input, 160);
        assert_eq!(usage.sessions["s1"].input, 110);
        assert_eq!(usage.sessions["s2"].cost_micro_usd, 500);
        assert_eq!(usage.daily_cost_micro_usd["2026-09-21"], 1_500);
        assert_eq!(usage.daily_cost_micro_usd["2026-09-22"], 100);
        assert_eq!(
            registry.mission_usage(Some("op"), "m1").unwrap(),
            Some(usage)
        );

        registry
            .record_usage(Some("op"), "m2", "s3", &totals(1, 70), 1_790_000_000)
            .unwrap();
        assert_eq!(
            registry
                .operation_cost_on_day(Some("op"), 1_790_000_000)
                .unwrap(),
            1_570
        );
        assert_eq!(
            registry
                .operation_cost_on_day(Some("other"), 1_790_000_000)
                .unwrap(),
            0
        );
    }
}
