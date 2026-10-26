//! Sleepers: standing orders that queue a brief on a schedule.
//!
//! An Operation lists them under `sleepers.<name>` in `operations.toml`. A sleeper fires
//! only after an operator authorizes its exact definition, and it fires once per scheduled
//! window: the brief's key is derived from the window, so any Station that evaluates the
//! same window, before or after a Headquarters failover, submits the same key and the
//! Registry accepts it once.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::bona_fides::{Origin, Provenance, now_secs};
use super::store::{
    Brief, FileRegistry, hex_digest, invalid_data, registry_lock, safe_component, unix_time,
    write_atomic,
};
use super::timeline::{TimelineEvent, record_event_locked};
use crate::directorates::registry::backend::*;

/// The Brief `source` of every brief a sleeper queues.
pub const SLEEPER_BRIEF_SOURCE: &str = "sleeper";

/// A window older than this is skipped rather than fired late.
pub const MAX_WINDOW_AGE_SECS: i64 = 24 * 60 * 60;

const MAX_PROMPT_BYTES: usize = 16 * 1024;
const LOOKBACK_DAYS: i64 = 366;

/// A standing order, as written under `sleepers.<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sleeper {
    /// Five-field cron expression (minute hour day-of-month month day-of-week) in UTC.
    pub schedule: String,
    /// The brief's title.
    pub title: String,
    /// The brief's instructions. Written by the operator, so not fenced as untrusted text.
    #[serde(default)]
    pub prompt: String,
    /// Explicit Agent identity, when assigned.
    #[serde(default)]
    pub agent: Option<String>,
    /// Requested Agent role, when not explicitly assigned.
    #[serde(default)]
    pub role: Option<String>,
    /// Higher values sort first.
    #[serde(default)]
    pub priority: i64,
}

impl Sleeper {
    /// Checks the definition without any Registry; the roster calls this when it loads.
    ///
    /// # Errors
    ///
    /// Returns a message for a bad cron expression, an empty title or an oversized prompt.
    pub fn validate(&self) -> Result<(), String> {
        Cron::parse(&self.schedule).map_err(|e| format!("schedule: {e}"))?;
        if self.title.trim().is_empty() {
            return Err("title must not be empty".to_owned());
        }
        if self.prompt.len() > MAX_PROMPT_BYTES {
            return Err(format!("prompt is over {MAX_PROMPT_BYTES} bytes"));
        }
        Ok(())
    }

    /// A digest of everything an authorization covers; editing the order changes it.
    pub fn fingerprint(&self) -> String {
        hex_digest(&Sha256::digest(
            serde_json::to_vec(self).unwrap_or_default(),
        ))
    }

    /// The latest scheduled minute (Unix seconds) at or before `now` that may still fire:
    /// after `authorized_at` and no older than [`MAX_WINDOW_AGE_SECS`].
    pub fn due_window(&self, now: i64, authorized_at: i64) -> Option<i64> {
        let window = Cron::parse(&self.schedule).ok()?.latest_at_or_before(now)?;
        (window > authorized_at && now - window <= MAX_WINDOW_AGE_SECS).then_some(window)
    }
}

/// An operator's approval of one exact sleeper definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SleeperAuthorization {
    /// The [`Sleeper::fingerprint`] that was approved.
    pub fingerprint: String,
    /// Who approved it.
    pub authorized_by: String,
    /// When, in Unix seconds; windows at or before this never fire.
    pub at: i64,
}

/// What offering a window to the Registry did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fire {
    /// The window's brief was queued.
    Queued,
    /// The window's brief already exists, queued, running or finished.
    AlreadyFired,
}

/// A parsed five-field cron expression, evaluated in UTC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u32,
    days: u32,
    months: u16,
    weekdays: u8,
    any_day: bool,
    any_weekday: bool,
}

/// Why a cron expression was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronError(String);

impl fmt::Display for CronError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CronError {}

fn bad(message: impl Into<String>) -> CronError {
    CronError(message.into())
}

fn field(text: &str, min: u32, max: u32, name: &str) -> Result<(u64, bool), CronError> {
    let mut bits = 0_u64;
    let mut star = false;
    for item in text.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step
                    .parse()
                    .ok()
                    .filter(|step| *step > 0)
                    .ok_or_else(|| bad(format!("{name}: bad step in {item:?}")))?;
                (range, step)
            }
            None => (item, 1),
        };
        let (low, high) = if range == "*" {
            star = star || step == 1;
            (min, max)
        } else if let Some((low, high)) = range.split_once('-') {
            (number(low, name)?, number(high, name)?)
        } else {
            let value = number(range, name)?;
            (value, if item.contains('/') { max } else { value })
        };
        if low < min || high > max || low > high {
            return Err(bad(format!("{name}: {item:?} is outside {min}-{max}")));
        }
        let mut value = low;
        while value <= high {
            bits |= 1 << value;
            value += step;
        }
    }
    Ok((bits, star))
}

fn number(text: &str, name: &str) -> Result<u32, CronError> {
    text.parse()
        .map_err(|_| bad(format!("{name}: {text:?} is not a number")))
}

impl Cron {
    /// Parses `minute hour day-of-month month day-of-week`. Each field takes `*`, numbers,
    /// `a-b` ranges, `,` lists and `/step`. Day-of-week is 0-7 (0 and 7 are Sunday). When
    /// both day fields are restricted, a day matches if either does.
    ///
    /// # Errors
    ///
    /// Returns [`CronError`] for the wrong field count or a value out of range.
    pub fn parse(expression: &str) -> Result<Self, CronError> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(bad("expected five fields: minute hour day month weekday"));
        };
        let (minutes, _) = field(minute, 0, 59, "minute")?;
        let (hours, _) = field(hour, 0, 23, "hour")?;
        let (days, any_day) = field(day, 1, 31, "day")?;
        let (months, _) = field(month, 1, 12, "month")?;
        let (mut weekdays, any_weekday) = field(weekday, 0, 7, "weekday")?;
        if weekdays & (1 << 7) != 0 {
            weekdays = (weekdays & !(1 << 7)) | 1;
        }
        Ok(Self {
            minutes,
            hours: hours as u32,
            days: days as u32,
            months: months as u16,
            weekdays: weekdays as u8,
            any_day,
            any_weekday,
        })
    }

    fn day_matches(&self, month: u32, day: u32, weekday: u32) -> bool {
        if self.months & (1 << month) == 0 {
            return false;
        }
        let day_ok = self.days & (1 << day) != 0;
        let weekday_ok = self.weekdays & (1 << weekday) != 0;
        match (self.any_day, self.any_weekday) {
            (false, false) => day_ok || weekday_ok,
            (false, true) => day_ok,
            (true, false) => weekday_ok,
            (true, true) => true,
        }
    }

    /// The latest scheduled minute at or before `now` (Unix seconds), looking back a year.
    pub fn latest_at_or_before(&self, now: i64) -> Option<i64> {
        let today = now.div_euclid(86_400);
        let minute_now = now.rem_euclid(86_400) / 60;
        for back in 0..=LOOKBACK_DAYS {
            let day = today - back;
            let (_, month, date) = civil_from_days(day);
            let weekday = (day + 4).rem_euclid(7) as u32; // 1970-01-01 was a Thursday
            if !self.day_matches(month, date, weekday) {
                continue;
            }
            let last = if back == 0 { minute_now } else { 1439 };
            for minute_of_day in (0..=last).rev() {
                let (hour, minute) = (minute_of_day / 60, minute_of_day % 60);
                if self.hours & (1 << hour) != 0 && self.minutes & (1 << minute) != 0 {
                    return Some(day * 86_400 + minute_of_day * 60);
                }
            }
        }
        None
    }
}

/// Days since 1970-01-01 to (year, month, day) in the proleptic Gregorian calendar.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `20261025T0300Z` for a window start.
pub fn window_label(window: i64) -> String {
    let (year, month, day) = civil_from_days(window.div_euclid(86_400));
    let minute = window.rem_euclid(86_400) / 60;
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}Z",
        minute / 60,
        minute % 60
    )
}

/// Builds the brief a sleeper queues for one window.
pub fn sleeper_brief(operation: &str, name: &str, sleeper: &Sleeper, window: i64) -> Brief {
    let label = window_label(window);
    let mut data = BTreeMap::new();
    data.insert(
        "sleeper".to_owned(),
        serde_json::json!({ "operation": operation, "name": name, "window": label }),
    );
    Brief {
        title: sleeper.title.clone(),
        body: sleeper.prompt.clone(),
        id: format!(
            "{}.{}.{label}",
            safe_component(operation),
            safe_component(name)
        ),
        source: SLEEPER_BRIEF_SOURCE.to_owned(),
        operation: Some(operation.to_owned()),
        agent: sleeper.agent.clone(),
        role: sleeper.role.clone(),
        tags: Vec::new(),
        stations: Vec::new(),
        priority: sleeper.priority,
        url: None,
        data,
        created: label,
        version: 1,
        provenance: None,
    }
}

impl FileRegistry {
    fn sleeper_path(&self, operation: &str, name: &str) -> std::path::PathBuf {
        self.root.join("sleepers").join(format!(
            "{}--{}.authorized.json",
            safe_component(operation),
            safe_component(name)
        ))
    }
}

/// Sleeper authorizations and the windows they have fired.
pub trait SleeperStore {
    /// Records an operator's approval of this exact definition.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid sleeper or when the Registry cannot be written.
    fn authorize_sleeper(
        &self,
        operation: &str,
        name: &str,
        sleeper: &Sleeper,
        authorized_by: &str,
        at: i64,
    ) -> io::Result<()>;

    /// Returns the stored approval, if any.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists but cannot be read.
    fn sleeper_authorization(
        &self,
        operation: &str,
        name: &str,
    ) -> io::Result<Option<SleeperAuthorization>>;

    /// Withdraws an approval; returns whether one existed.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn revoke_sleeper(&self, operation: &str, name: &str) -> io::Result<bool>;

    /// Queues the brief for one window unless it already exists in any Registry state. Its
    /// [`Provenance`] names the sleeper, its definition's fingerprint and `received_by`, the
    /// Station that fired it.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid brief or Registry I/O failures.
    fn fire_sleeper(
        &self,
        operation: &str,
        name: &str,
        sleeper: &Sleeper,
        window: i64,
        received_by: &str,
    ) -> io::Result<Fire>;
}

/// The brief a sleeper queues for one window, with its provenance.
pub(super) fn fired_brief(
    operation: &str,
    name: &str,
    sleeper: &Sleeper,
    window: i64,
    received_by: &str,
) -> Brief {
    let mut brief = sleeper_brief(operation, name, sleeper, window);
    brief.provenance = Some(
        Provenance::new(Origin::Sleeper, &brief, received_by, now_secs())
            .with_principal(name)
            .with_reference(sleeper.fingerprint()),
    );
    brief
}

impl SleeperStore for FileRegistry {
    fn authorize_sleeper(
        &self,
        operation: &str,
        name: &str,
        sleeper: &Sleeper,
        authorized_by: &str,
        at: i64,
    ) -> io::Result<()> {
        sleeper.validate().map_err(invalid_data)?;
        let _lock = registry_lock(&self.root)?;
        let record = SleeperAuthorization {
            fingerprint: sleeper.fingerprint(),
            authorized_by: authorized_by.to_owned(),
            at,
        };
        let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
        write_atomic(&self.sleeper_path(operation, name), &bytes)?;
        let event = TimelineEvent::new("sleeper", "authorized", operation, at as f64)
            .actor(authorized_by)
            .detail("sleeper", name);
        // The approval stands either way; its timeline entry is a record, not a condition.
        let _ = record_event_locked(self.root(), &event);
        Ok(())
    }

    fn sleeper_authorization(
        &self,
        operation: &str,
        name: &str,
    ) -> io::Result<Option<SleeperAuthorization>> {
        match fs::read(self.sleeper_path(operation, name)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| invalid_data(e.to_string())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn revoke_sleeper(&self, operation: &str, name: &str) -> io::Result<bool> {
        let _lock = registry_lock(&self.root)?;
        match fs::remove_file(self.sleeper_path(operation, name)) {
            Ok(()) => {
                if let Ok(at) = unix_time() {
                    let event = TimelineEvent::new("sleeper", "revoked", operation, at)
                        .detail("sleeper", name);
                    let _ = record_event_locked(self.root(), &event);
                }
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn fire_sleeper(
        &self,
        operation: &str,
        name: &str,
        sleeper: &Sleeper,
        window: i64,
        received_by: &str,
    ) -> io::Result<Fire> {
        match self.submit_brief(&fired_brief(operation, name, sleeper, window, received_by)) {
            Ok(_) => Ok(Fire::Queued),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(Fire::AlreadyFired),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_window_fires_once_however_many_stations_evaluate_it,
        approval_covers_one_exact_definition,
    );

    /// 2026-10-25T10:00:00Z
    const NOW: i64 = 1_792_922_400;

    fn at(day: &str) -> i64 {
        // Midnight of an ISO date, through the same calendar code the schedule uses.
        let parts: Vec<i64> = day.split('-').map(|p| p.parse().unwrap()).collect();
        let (y, m, d) = (parts[0], parts[1], parts[2]);
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let mp = if m > 2 { m - 3 } else { m + 9 };
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        (era * 146_097 + doe - 719_468) * 86_400
    }

    fn label(cron: &str, now: i64) -> Option<String> {
        Cron::parse(cron)
            .unwrap()
            .latest_at_or_before(now)
            .map(window_label)
    }

    #[test]
    fn the_calendar_helper_agrees_with_the_fixture_time() {
        assert_eq!(at("2026-10-25") + 10 * 3600, NOW);
        assert_eq!(window_label(NOW), "20261025T1000Z");
    }

    #[test]
    fn daily_and_stepped_schedules_find_their_latest_window() {
        assert_eq!(label("0 3 * * *", NOW).as_deref(), Some("20261025T0300Z"));
        assert_eq!(
            label("0 3 * * *", at("2026-10-25") + 2 * 3600 + 59 * 60).as_deref(),
            Some("20261024T0300Z")
        );
        assert_eq!(
            label("*/15 * * * *", NOW + 14 * 60).as_deref(),
            Some("20261025T1000Z")
        );
        assert_eq!(
            label("*/15 * * * *", NOW + 15 * 60).as_deref(),
            Some("20261025T1015Z")
        );
        assert_eq!(
            label("30 9,17 * * *", NOW).as_deref(),
            Some("20261025T0930Z")
        );
    }

    #[test]
    fn weekday_month_and_year_boundaries() {
        // 2026-10-25 is a Sunday; the previous weekday morning is Friday the 23rd.
        assert_eq!(label("0 9 * * 1-5", NOW).as_deref(), Some("20261023T0900Z"));
        assert_eq!(label("0 9 * * 7", NOW).as_deref(), Some("20261025T0900Z"));
        assert_eq!(label("0 0 1 * *", NOW).as_deref(), Some("20261001T0000Z"));
        assert_eq!(
            label("0 0 29 2 *", NOW),
            None,
            "the last leap day is over a year back"
        );
        assert_eq!(label("0 0 1 1 *", NOW).as_deref(), Some("20260101T0000Z"));
    }

    #[test]
    fn a_restricted_day_and_weekday_match_either() {
        // The 1st of the month, or any Monday.
        assert_eq!(label("0 0 1 * 1", NOW).as_deref(), Some("20261019T0000Z"));
    }

    #[test]
    fn bad_expressions_are_refused() {
        for bad in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "*/0 * * * *",
            "a * * * *",
            "5-1 * * * *",
        ] {
            assert!(Cron::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            Cron::parse("0 0 30 2 *").unwrap().latest_at_or_before(NOW),
            None
        );
    }

    fn nightly() -> Sleeper {
        Sleeper {
            schedule: "0 3 * * *".to_owned(),
            title: "Update dependencies".to_owned(),
            prompt: "Update the lockfile and open a branch.".to_owned(),
            agent: None,
            role: Some("maintenance".to_owned()),
            priority: 1,
        }
    }

    fn a_window_fires_once_however_many_stations_evaluate_it(store: &dyn Store) {
        let first_hq = store.handle();
        let second_hq = store.handle();
        let sleeper = nightly();
        let window = sleeper.due_window(NOW, 0).unwrap();

        assert_eq!(
            first_hq
                .fire_sleeper("G", "deps", &sleeper, window, "station-a")
                .unwrap(),
            Fire::Queued
        );
        assert_eq!(
            second_hq
                .fire_sleeper("G", "deps", &sleeper, window, "station-a")
                .unwrap(),
            Fire::AlreadyFired
        );
        let queued = first_hq.list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].key(), "sleeper--G.deps.20261025T0300Z");
        assert_eq!(queued[0].role.as_deref(), Some("maintenance"));
        assert_eq!(queued[0].operation.as_deref(), Some("G"));
        let provenance = queued[0]
            .provenance
            .as_ref()
            .expect("a sleeper brief's origin");
        assert_eq!(provenance.origin, Origin::Sleeper);
        assert_eq!(provenance.principal.as_deref(), Some("deps"));
        assert_eq!(
            provenance.reference.as_deref(),
            Some(sleeper.fingerprint().as_str())
        );

        // Running and finished briefs still count as fired.
        let brief = first_hq.claim_new(None).unwrap().unwrap();
        assert_eq!(
            second_hq
                .fire_sleeper("G", "deps", &sleeper, window, "station-a")
                .unwrap(),
            Fire::AlreadyFired
        );
        first_hq
            .finish(
                &brief,
                &serde_json::json!({"state": "done"}),
                true,
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            second_hq
                .fire_sleeper("G", "deps", &sleeper, window, "station-a")
                .unwrap(),
            Fire::AlreadyFired
        );
        assert!(second_hq.list_new().unwrap().is_empty());

        // The next night is a new window.
        let next = sleeper.due_window(NOW + 86_400, 0).unwrap();
        assert_eq!(
            second_hq
                .fire_sleeper("G", "deps", &sleeper, next, "station-a")
                .unwrap(),
            Fire::Queued
        );
    }

    #[test]
    fn only_windows_after_the_approval_and_within_a_day_may_fire() {
        let sleeper = nightly();
        let three = at("2026-10-25") + 3 * 3600;
        assert_eq!(
            sleeper.due_window(NOW, three),
            None,
            "approved after the 03:00 window"
        );
        assert_eq!(sleeper.due_window(NOW, three - 1), Some(three));
        assert_eq!(
            sleeper.due_window(three + MAX_WINDOW_AGE_SECS + 60, 0),
            Some(three + 86_400),
            "the next night's window is the latest"
        );
        let weekly = Sleeper {
            schedule: "0 3 * * 0".to_owned(),
            ..nightly()
        };
        assert_eq!(
            weekly.due_window(NOW + 3 * 86_400, 0),
            None,
            "a window over a day old is skipped"
        );
    }

    fn approval_covers_one_exact_definition(store: &dyn Store) {
        let registry = store.handle();
        let sleeper = nightly();
        assert!(
            registry
                .sleeper_authorization("G", "deps")
                .unwrap()
                .is_none()
        );
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", 100)
            .unwrap();
        let stored = registry
            .sleeper_authorization("G", "deps")
            .unwrap()
            .unwrap();
        assert_eq!(stored.fingerprint, sleeper.fingerprint());
        assert_eq!(stored.at, 100);
        let edited = Sleeper {
            prompt: "Something else.".to_owned(),
            ..nightly()
        };
        assert_ne!(edited.fingerprint(), stored.fingerprint);
        assert!(registry.revoke_sleeper("G", "deps").unwrap());
        assert!(!registry.revoke_sleeper("G", "deps").unwrap());
        let events = registry.timeline_events("G").unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| (event.action.as_str(), event.detail["sleeper"].as_str()))
                .collect::<Vec<_>>(),
            [("authorized", "deps"), ("revoked", "deps")]
        );
        assert_eq!(events[0].actor.as_deref(), Some("operator"));
        assert!(
            registry
                .authorize_sleeper(
                    "G",
                    "bad",
                    &Sleeper {
                        schedule: "nope".to_owned(),
                        ..nightly()
                    },
                    "o",
                    1
                )
                .is_err()
        );
    }
}
