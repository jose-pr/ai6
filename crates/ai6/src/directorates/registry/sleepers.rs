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
use super::limits::SleeperLimits;
use super::logbook::station_key;
use super::mission_ids::MissionEntry;
use super::store::{
    Brief, FileRegistry, Reentry, hex_digest, invalid_data, safe_component, unix_time,
    validate_station_id,
};
use super::timeline::{TimelineEvent, record_event_locked};
use crate::directorates::registry::backend::*;

/// The Brief `source` of every brief a sleeper queues.
pub const SLEEPER_BRIEF_SOURCE: &str = "sleeper";

/// A standing order, as written under `sleepers.<name>`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    guatiao::ToValue,
    guatiao::FromValue,
    guatiao::Schema,
    guatiao_intake::Form,
)]
#[serde(deny_unknown_fields)]
#[map(deny_unknown_fields)]
pub struct Sleeper {
    /// When it runs, as five cron fields in UTC: minute, hour, day of month, month, day of week.
    #[schema(title = "Schedule")]
    #[form(placeholder = "0 6 * * 1")]
    pub schedule: String,
    /// What each brief it queues is called.
    #[schema(title = "Brief title")]
    pub title: String,
    /// What each brief asks for.
    #[serde(default)]
    #[schema(title = "Instructions")]
    #[map(default)]
    #[form(widget = "textarea")]
    pub prompt: String,
    /// The Agent that takes it, when it is always the same one.
    #[serde(default)]
    #[schema(title = "Agent", format = "record:agents")]
    pub agent: Option<String>,
    /// The role it asks for, when no one Agent is named.
    #[serde(default)]
    #[schema(title = "Role", format = "record:roles")]
    pub role: Option<String>,
    /// Among briefs waiting, the highest goes first.
    #[serde(default)]
    #[schema(title = "Priority")]
    #[map(default)]
    pub priority: i64,
    /// The Stations its briefs are limited to; empty adds no limit. It can only narrow the
    /// Operation's own list.
    #[serde(default)]
    #[schema(title = "Stations", format = "reported:stations")]
    #[map(default)]
    pub stations: Vec<String>,
    /// Queue one brief for every live Station that may take it: a round of all Stations.
    #[serde(default)]
    #[schema(title = "A brief for each Station")]
    #[map(default)]
    pub each_station: bool,
    /// One standing Mission for the Sleeper (one per Station with `each_station`), each window
    /// a new attempt of it, instead of a Mission per window. A window that comes while an
    /// attempt is in flight is skipped.
    #[serde(default, skip_serializing_if = "is_false")]
    #[schema(title = "One standing Mission")]
    #[map(default)]
    pub standing: bool,
    /// Each window's attempt resumes the last attempt's provider Session when its handler can,
    /// else starts a new one. Needs `standing`.
    #[serde(default, skip_serializing_if = "is_false")]
    #[schema(title = "Resume the last attempt's Session")]
    #[map(default)]
    pub resume: bool,
}

/// Leaves an unset flag out of a definition, so its fingerprint, and the approval that names
/// it, stay as they were before the flag existed.
fn is_false(value: &bool) -> bool {
    !*value
}

impl Sleeper {
    /// Checks the definition without any Registry; the roster calls this when it loads. The
    /// Registry also checks the prompt's size when the sleeper is authorized.
    ///
    /// # Errors
    ///
    /// Returns a message for a bad cron expression, an empty title, a `stations` entry that is
    /// not a Station ID, or `resume` without `standing`.
    pub fn validate(&self) -> Result<(), String> {
        Cron::parse(&self.schedule).map_err(|e| format!("schedule: {e}"))?;
        if self.title.trim().is_empty() {
            return Err("title must not be empty".to_owned());
        }
        if self.resume && !self.standing {
            return Err("resume needs standing = true".to_owned());
        }
        if let Some(station) = self
            .stations
            .iter()
            .find(|station| validate_station_id(station).is_err())
        {
            return Err(format!(
                "stations entry {station:?} is not a valid Station ID"
            ));
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
    /// after `authorized_at` and no older than `limits.window_age`; an older window is
    /// skipped rather than fired late.
    pub fn due_window(&self, now: i64, authorized_at: i64, limits: &SleeperLimits) -> Option<i64> {
        let window = Cron::parse(&self.schedule)
            .ok()?
            .latest_at_or_before(now, limits.lookback_days)?;
        (window > authorized_at && limits.window_age.allows_secs((now - window) as f64))
            .then_some(window)
    }

    /// Checks the prompt against `limits`.
    pub(super) fn check_size(&self, limits: &SleeperLimits) -> Result<(), String> {
        if limits.prompt_bytes.allows(self.prompt.len()) {
            Ok(())
        } else {
            Err(format!("prompt is over {} bytes", limits.prompt_bytes))
        }
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
    /// A standing Sleeper's attempt of an earlier window is still queued, held or running, so
    /// this window is skipped.
    InFlight,
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

    /// The latest scheduled minute at or before `now` (Unix seconds), looking back
    /// `lookback_days` days.
    pub fn latest_at_or_before(&self, now: i64, lookback_days: u32) -> Option<i64> {
        let today = now.div_euclid(86_400);
        let minute_now = now.rem_euclid(86_400) / 60;
        for back in 0..=i64::from(lookback_days) {
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
pub(super) fn civil_from_days(days: i64) -> (i64, u32, u32) {
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

/// Builds the brief a sleeper queues for one window. `station` names the Station of one
/// round of an [`each_station`](Sleeper::each_station) sleeper: the brief is narrowed to it
/// and its key names it, so each Station's round is queued once.
pub fn sleeper_brief(
    operation: &str,
    name: &str,
    sleeper: &Sleeper,
    window: i64,
    station: Option<&str>,
) -> Brief {
    let label = window_label(window);
    let mut data = BTreeMap::new();
    let mut origin = serde_json::json!({ "operation": operation, "name": name, "window": label });
    let mut id = format!(
        "{}.{}.{label}",
        safe_component(operation),
        safe_component(name)
    );
    if let Some(station) = station {
        origin["station"] = serde_json::json!(station);
        id.push('.');
        id.push_str(&station_key(station));
    }
    if sleeper.standing {
        origin["standing"] = serde_json::json!(true);
    }
    if sleeper.resume {
        origin["resume"] = serde_json::json!(true);
    }
    data.insert("sleeper".to_owned(), origin);
    Brief {
        title: sleeper.title.clone(),
        body: sleeper.prompt.clone(),
        id,
        mission_id: None,
        handle: None,
        name: None,
        source: SLEEPER_BRIEF_SOURCE.to_owned(),
        operation: Some(operation.to_owned()),
        agent: sleeper.agent.clone(),
        role: sleeper.role.clone(),
        tags: Vec::new(),
        stations: match station {
            Some(station) => vec![station.to_owned()],
            None => sleeper.stations.clone(),
        },
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

    /// Queues the brief for one `(window, station)` round: the window's brief, or with a
    /// `station` that Station's round in it (see [`sleeper_brief`]), unless it already
    /// exists in any Registry state. Its
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
        round: (i64, Option<&str>),
        received_by: &str,
    ) -> io::Result<Fire>;
}

/// Where one round of a sleeper enters: the window's own Mission, or for a standing Sleeper the
/// one Mission all its windows are attempts of.
pub(super) fn round_entry(
    operation: &str,
    name: &str,
    sleeper: &Sleeper,
    (window, station): (i64, Option<&str>),
) -> MissionEntry {
    if sleeper.standing {
        MissionEntry::standing(operation, name, station)
    } else {
        MissionEntry::sleeper(operation, name, &window_label(window), station)
    }
}

/// The window a Sleeper's brief was queued for.
pub fn window_of(brief: &Brief) -> Option<&str> {
    brief.data.get("sleeper")?.get("window")?.as_str()
}

/// Queues one round of a sleeper in `registry`, as [`SleeperStore::fire_sleeper`] says.
pub(super) fn fire_round<R: BriefStore + ?Sized>(
    registry: &R,
    (operation, name, sleeper): (&str, &str, &Sleeper),
    round: (i64, Option<&str>),
    received_by: &str,
) -> io::Result<Fire> {
    let brief = fired_brief(operation, name, sleeper, round, received_by);
    let entry = round_entry(operation, name, sleeper, round);
    let enter = || match registry.enter_brief(&brief, &entry) {
        Ok(_) => Ok(Fire::Queued),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(Fire::AlreadyFired),
        Err(error) => Err(error),
    };
    if !sleeper.standing {
        return enter();
    }
    let label = window_label(round.0);
    let this_window = |filed: &Brief| window_of(filed) == Some(label.as_str());
    match registry.reenter_brief(&brief, &entry, &|filed| !this_window(filed))? {
        Reentry::Queued(_) => Ok(Fire::Queued),
        Reentry::Unchanged(filed)
            if this_window(&filed.brief) || matches!(filed.state.as_str(), "done" | "failed") =>
        {
            Ok(Fire::AlreadyFired)
        }
        Reentry::Unchanged(_) => Ok(Fire::InFlight),
        _ => enter(),
    }
}

/// The brief a sleeper queues for one window and round, with its provenance.
pub(super) fn fired_brief(
    operation: &str,
    name: &str,
    sleeper: &Sleeper,
    (window, station): (i64, Option<&str>),
    received_by: &str,
) -> Brief {
    let mut brief = sleeper_brief(operation, name, sleeper, window, station);
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
        sleeper
            .check_size(&self.limits.sleepers)
            .map_err(invalid_data)?;
        let _lock = self.lock()?;
        let record = SleeperAuthorization {
            fingerprint: sleeper.fingerprint(),
            authorized_by: authorized_by.to_owned(),
            at,
        };
        let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
        self.write_atomic(&self.sleeper_path(operation, name), &bytes)?;
        let event = TimelineEvent::new("sleeper", "authorized", operation, at as f64)
            .actor(authorized_by)
            .detail("sleeper", name);
        // The approval stands either way; its timeline entry is a record, not a condition.
        let _ = record_event_locked(self, &event);
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
        let _lock = self.lock()?;
        match fs::remove_file(self.sleeper_path(operation, name)) {
            Ok(()) => {
                if let Ok(at) = unix_time() {
                    let event = TimelineEvent::new("sleeper", "revoked", operation, at)
                        .detail("sleeper", name);
                    let _ = record_event_locked(self, &event);
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
        round: (i64, Option<&str>),
        received_by: &str,
    ) -> io::Result<Fire> {
        fire_round(self, (operation, name, sleeper), round, received_by)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_window_fires_once_however_many_stations_evaluate_it,
        approval_covers_one_exact_definition,
        each_station_s_round_is_its_own_brief_narrowed_to_it,
        a_standing_sleeper_s_windows_are_attempts_of_one_mission,
        a_standing_sleeper_past_the_attempt_limit_still_requeues_a_lapse,
    );

    /// 2026-10-25T10:00:00Z
    const NOW: i64 = 1_792_922_400;

    fn a_standing_sleeper_s_windows_are_attempts_of_one_mission(store: &dyn Store) {
        let headquarters = store.handle();
        let takeover = store.handle();
        let standing = Sleeper {
            standing: true,
            ..nightly()
        };
        let window = |day: i64| at("2026-10-25") + 3 * 3600 + day * 86_400;
        let fire = |registry: &dyn RegistryBackend, at: i64, station: Option<&str>| {
            registry
                .fire_sleeper("G", "deps", &standing, (at, station), "hq")
                .unwrap()
        };
        assert_eq!(fire(headquarters.as_ref(), window(0), None), Fire::Queued);
        assert_eq!(fire(takeover.as_ref(), window(0), None), Fire::AlreadyFired);
        assert_eq!(
            fire(takeover.as_ref(), window(1), None),
            Fire::InFlight,
            "the next window comes while the first attempt is queued"
        );
        let first = headquarters.list_new().unwrap().remove(0);
        assert_eq!(first.shown_handle(), "G-deps-1");
        assert_eq!(window_of(&first), Some(window_label(window(0)).as_str()));
        let key = first.key();

        for attempt in 1..=3 {
            let lease = headquarters
                .claim_with_lease(None, "hq", crate::DEFAULT_LEASE, BTreeMap::new())
                .unwrap()
                .unwrap();
            assert_eq!((lease.key(), lease.attempt()), (key.as_str(), 1));
            let claimed = headquarters.read_claimed(&key).unwrap().unwrap();
            assert_eq!(
                window_of(&claimed),
                Some(window_label(window(i64::from(attempt) - 1)).as_str())
            );
            assert_eq!(claimed.provenance.as_ref().unwrap().origin, Origin::Sleeper);
            assert_eq!(
                fire(takeover.as_ref(), window(i64::from(attempt) - 1), None),
                Fire::AlreadyFired
            );
            headquarters
                .finish(
                    &claimed,
                    &serde_json::json!({"state": "done"}),
                    true,
                    None,
                    None,
                )
                .unwrap();
            assert_eq!(
                fire(takeover.as_ref(), window(i64::from(attempt) - 1), None),
                Fire::AlreadyFired,
                "a finished window does not fire again"
            );
            if attempt < 3 {
                assert_eq!(
                    fire(takeover.as_ref(), window(i64::from(attempt)), None),
                    Fire::Queued
                );
                let queued = headquarters.list_new().unwrap();
                assert_eq!(queued.len(), 1);
                assert_eq!(
                    (queued[0].key(), queued[0].shown_handle()),
                    (key.clone(), "G-deps-1".to_owned())
                );
            }
        }
        // Rounds on each Station are a standing Mission each.
        for station in ["station-a", "station-b"] {
            assert_eq!(
                fire(headquarters.as_ref(), window(5), Some(station)),
                Fire::Queued
            );
        }
        let mut rounds: Vec<String> = headquarters
            .list_new()
            .unwrap()
            .iter()
            .map(Brief::shown_handle)
            .collect();
        rounds.sort();
        assert_eq!(rounds, ["G-deps-2", "G-deps-3"]);
    }

    fn a_standing_sleeper_past_the_attempt_limit_still_requeues_a_lapse(store: &dyn Store) {
        use crate::directorates::registry::store::LeaseRecovery;
        use crate::directorates::registry::suite::lapse;
        const LIMIT: u32 = 2;
        let registry = store.handle();
        let standing = Sleeper {
            standing: true,
            ..nightly()
        };
        let window = |day: i64| at("2026-10-25") + 3 * 3600 + day * 86_400;
        let fire = |day: i64| {
            registry
                .fire_sleeper("G", "deps", &standing, (window(day), None), "hq")
                .unwrap()
        };
        let claim = || {
            registry
                .claim_with_lease(None, "hq", crate::DEFAULT_LEASE, BTreeMap::new())
                .unwrap()
                .expect("the window's attempt is queued")
        };
        for day in 0..=i64::from(LIMIT) {
            assert_eq!(fire(day), Fire::Queued);
            let lease = claim();
            assert_eq!(lease.attempt(), 1, "window {day}");
            let claimed = registry.read_claimed(lease.key()).unwrap().unwrap();
            registry
                .finish(
                    &claimed,
                    &serde_json::json!({"state": "done"}),
                    true,
                    None,
                    None,
                )
                .unwrap();
        }

        assert_eq!(fire(i64::from(LIMIT) + 1), Fire::Queued);
        let lease = claim();
        let key = lease.key().to_owned();
        let lapsed = lapse(registry.as_ref(), &key, "hq");
        assert_eq!(
            registry.recover(&lapsed, LIMIT, "other").unwrap(),
            Some(LeaseRecovery::Requeued),
            "a window's first lapse, however many windows came before"
        );
        let again = claim();
        assert_eq!((again.key(), again.attempt()), (key.as_str(), 2));
        let lapsed = lapse(registry.as_ref(), &key, "hq");
        assert_eq!(
            registry.recover(&lapsed, LIMIT, "other").unwrap(),
            Some(LeaseRecovery::Failed),
            "the limit still bounds the lapses of one attempt"
        );
    }

    #[test]
    fn standing_and_resume_leave_an_existing_approval_standing_unless_set() {
        let plain = nightly();
        let json = serde_json::to_string(&plain).unwrap();
        assert!(
            !json.contains("standing") && !json.contains("resume"),
            "an existing definition fingerprints as it did: {json}"
        );
        let standing = Sleeper {
            standing: true,
            ..nightly()
        };
        assert_ne!(standing.fingerprint(), plain.fingerprint());
        let resume = Sleeper {
            resume: true,
            ..nightly()
        };
        assert!(resume.validate().unwrap_err().contains("standing"));
        let both = Sleeper {
            resume: true,
            ..standing
        };
        assert!(both.validate().is_ok());
        let brief = sleeper_brief("G", "deps", &both, NOW, None);
        assert_eq!(brief.data["sleeper"]["standing"], true);
        assert_eq!(brief.data["sleeper"]["resume"], true);
    }

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
            .latest_at_or_before(now, 366)
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
            Cron::parse("0 0 30 2 *")
                .unwrap()
                .latest_at_or_before(NOW, 366),
            None
        );
    }

    pub(in crate::directorates::registry) fn nightly() -> Sleeper {
        Sleeper {
            schedule: "0 3 * * *".to_owned(),
            title: "Update dependencies".to_owned(),
            prompt: "Update the lockfile and open a branch.".to_owned(),
            agent: None,
            role: Some("maintenance".to_owned()),
            priority: 1,
            stations: Vec::new(),
            each_station: false,
            standing: false,
            resume: false,
        }
    }

    #[test]
    fn round_fields_change_the_fingerprint() {
        let before = nightly().fingerprint();
        let rounds = Sleeper {
            each_station: true,
            ..nightly()
        };
        let narrowed = Sleeper {
            stations: vec!["station-a".to_owned()],
            ..nightly()
        };
        assert_ne!(rounds.fingerprint(), before);
        assert_ne!(narrowed.fingerprint(), before);
        assert_ne!(rounds.fingerprint(), narrowed.fingerprint());
        let bad = Sleeper {
            stations: vec![String::new()],
            ..nightly()
        };
        assert!(bad.validate().unwrap_err().contains("stations"));
    }

    #[test]
    fn a_narrowed_sleeper_s_brief_carries_its_stations() {
        let narrowed = Sleeper {
            stations: vec!["station-a".to_owned(), "station-b".to_owned()],
            ..nightly()
        };
        let brief = sleeper_brief("G", "deps", &narrowed, NOW, None);
        assert_eq!(brief.stations, ["station-a", "station-b"]);
        assert_eq!(brief.id, "G.deps.20261025T1000Z");
        // Odd Station IDs that read alike as a key still make distinct rounds.
        let one = sleeper_brief("G", "deps", &nightly(), NOW, Some("a b"));
        let other = sleeper_brief("G", "deps", &nightly(), NOW, Some("a/b"));
        assert_ne!(one.id, other.id);
        assert_eq!(one.stations, ["a b"]);
        assert_eq!(one.data["sleeper"]["station"], "a b");
    }

    fn each_station_s_round_is_its_own_brief_narrowed_to_it(store: &dyn Store) {
        let headquarters = store.handle();
        let takeover = store.handle();
        let rounds = Sleeper {
            each_station: true,
            ..nightly()
        };
        let window = rounds
            .due_window(NOW, 0, &SleeperLimits::default())
            .unwrap();
        for station in ["station-a", "station-b"] {
            assert_eq!(
                headquarters
                    .fire_sleeper("G", "deps", &rounds, (window, Some(station)), "hq")
                    .unwrap(),
                Fire::Queued
            );
            assert_eq!(
                takeover
                    .fire_sleeper("G", "deps", &rounds, (window, Some(station)), "hq-2")
                    .unwrap(),
                Fire::AlreadyFired,
                "a takeover Headquarters fires no second round for {station}"
            );
        }
        let mut queued = headquarters.list_new().unwrap();
        queued.sort_by_key(Brief::shown_handle);
        assert_eq!(
            queued.iter().map(Brief::shown_handle).collect::<Vec<_>>(),
            ["G-deps-1", "G-deps-2"]
        );
        assert_eq!(queued[0].stations, ["station-a"]);
        assert_eq!(queued[1].stations, ["station-b"]);
        assert!(queued.iter().all(|brief| brief.provenance.is_some()));
    }

    fn a_window_fires_once_however_many_stations_evaluate_it(store: &dyn Store) {
        let first_hq = store.handle();
        let second_hq = store.handle();
        let sleeper = nightly();
        let window = sleeper
            .due_window(NOW, 0, &SleeperLimits::default())
            .unwrap();

        assert_eq!(
            first_hq
                .fire_sleeper("G", "deps", &sleeper, (window, None), "station-a")
                .unwrap(),
            Fire::Queued
        );
        assert_eq!(
            second_hq
                .fire_sleeper("G", "deps", &sleeper, (window, None), "station-a")
                .unwrap(),
            Fire::AlreadyFired
        );
        let queued = first_hq.list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].shown_handle(), "G-deps-1");
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
                .fire_sleeper("G", "deps", &sleeper, (window, None), "station-a")
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
                .fire_sleeper("G", "deps", &sleeper, (window, None), "station-a")
                .unwrap(),
            Fire::AlreadyFired
        );
        assert!(second_hq.list_new().unwrap().is_empty());

        // The next night is a new window.
        let next = sleeper
            .due_window(NOW + 86_400, 0, &SleeperLimits::default())
            .unwrap();
        assert_eq!(
            second_hq
                .fire_sleeper("G", "deps", &sleeper, (next, None), "station-a")
                .unwrap(),
            Fire::Queued
        );
    }

    #[test]
    fn only_windows_after_the_approval_and_within_a_day_may_fire() {
        let sleeper = nightly();
        let three = at("2026-10-25") + 3 * 3600;
        assert_eq!(
            sleeper.due_window(NOW, three, &SleeperLimits::default()),
            None,
            "approved after the 03:00 window"
        );
        assert_eq!(
            sleeper.due_window(NOW, three - 1, &SleeperLimits::default()),
            Some(three)
        );
        assert_eq!(
            sleeper.due_window(three + 24 * 60 * 60 + 60, 0, &SleeperLimits::default()),
            Some(three + 86_400),
            "the next night's window is the latest"
        );
        let weekly = Sleeper {
            schedule: "0 3 * * 0".to_owned(),
            ..nightly()
        };
        assert_eq!(
            weekly.due_window(NOW + 3 * 86_400, 0, &SleeperLimits::default()),
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
