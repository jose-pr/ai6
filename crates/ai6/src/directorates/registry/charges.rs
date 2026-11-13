//! Charge records: each charge's standing Run, the Station that runs it, and what it has done.
//!
//! A charge is not a Mission, so its Run is recorded here and never among the Run records:
//! this record names the charge's scope (`m`, `operation:<codename>`, `station:<id>`) where a
//! Mission's Run record names the Mission. It keeps the Run the charge resumes,
//! its logbook (what a fresh Run is handed when the old one cannot be resumed), when it last
//! woke and when each turn of the last hour began, what it has been told of, and the controls
//! people asked for.
//!
//! The Station running a charge holds its lease as a Station holds a Run's: it claims the
//! record, renews it, and presents its fencing token with every write. A lease that lapses lets
//! another Station take the charge over with a higher token, so the one that went dark cannot
//! write after it. Leases lapse on the Registry's clock (the database's for SQL).

use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use guatiao::{FromValue, Schema, ToValue};
use serde::{Deserialize, Serialize};

use super::store::{FileRegistry, hex_digest, invalid_data, unix_time, validate_station_id};
use crate::bounds::Limit;
use crate::directorates::m::charges::ChargeScope;

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;

/// The window a charge's turns are counted over, in seconds.
pub const TURN_WINDOW_SECS: f64 = 3_600.0;

/// `[registry.limits.charges]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ChargeLimits {
    /// Charge records the Registry keeps.
    #[schema(title = "Charge records")]
    #[map(default = ChargeLimits::default().records)]
    pub records: Limit,
    /// Bytes of one charge's record.
    #[schema(title = "Largest charge record", format = "bytes")]
    #[map(default = ChargeLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Logbook entries kept per charge, and handed to a fresh Run; past it the oldest go.
    #[schema(title = "Logbook entries per charge")]
    #[map(default = ChargeLimits::default().logbook)]
    pub logbook: Limit,
    /// Bytes of one logbook entry's text, or of a control's note; a longer entry is cut.
    #[schema(title = "Longest logbook entry", format = "bytes")]
    #[map(default = ChargeLimits::default().entry_bytes)]
    pub entry_bytes: Limit,
    /// Events a charge remembers being told of, so it is not told twice; past it the oldest go.
    #[schema(title = "Events remembered per charge")]
    #[map(default = ChargeLimits::default().seen)]
    pub seen: Limit,
    /// Controls waiting for a charge's Station; past it a control is refused.
    #[schema(title = "Controls waiting per charge")]
    #[map(default = ChargeLimits::default().controls)]
    pub controls: Limit,
}

impl Default for ChargeLimits {
    fn default() -> Self {
        Self {
            records: Limit::at(1_024),
            record_bytes: Limit::at(MIB),
            logbook: Limit::at(100),
            entry_bytes: Limit::at(2 * KIB),
            seen: Limit::at(1_024),
            controls: Limit::at(32),
        }
    }
}

/// One line of a charge's logbook.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChargeLogEntry {
    /// When it was written, as Unix seconds.
    pub at: f64,
    /// What it records: `turn`, `started`, `resumed`, `fresh`, `quota`, `station`, `agent`.
    pub kind: String,
    /// What happened, in a line or a few. A turn's quotes what the charge said, which may quote
    /// untrusted input.
    pub text: String,
}

impl ChargeLogEntry {
    /// An entry of `kind` saying `text`, written at `at`.
    pub fn new(kind: impl Into<String>, text: impl Into<String>, at: f64) -> Self {
        Self {
            at,
            kind: kind.into(),
            text: text.into(),
        }
    }
}

/// A person's request that a charge wake, waiting for the Station that runs it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChargeControl {
    /// A time-ordered unique ID.
    pub id: String,
    /// Who asked, as Universal Exports or the operator named them.
    pub by: String,
    /// What they said, if anything: the charge is given it with the wake.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// When they asked, as Unix seconds.
    pub at: f64,
}

/// One charge as the Registry keeps it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChargeRecord {
    /// What the charge keeps: `m`, `operation:<codename>` or `station:<id>`.
    pub scope: String,
    /// The Agent that keeps it, as its table named it when its Station last claimed it.
    pub agent: String,
    /// The Station that holds its lease and runs its Run.
    pub station: String,
    /// The fencing token of the current lease; it only grows.
    pub fence: u64,
    /// When the holder's lease lapses, as Unix seconds.
    pub lease_expires: f64,
    /// When the record was made, as Unix seconds.
    pub opened_at: f64,
    /// The Run the charge resumes, once a harness has issued one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// The handler that issued it: another handler cannot resume it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
    /// When it last woke, as Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_wake: Option<f64>,
    /// When each of its turns in the last hour began, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<f64>,
    /// What it did, oldest first and bounded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logbook: Vec<ChargeLogEntry>,
    /// The events it has been told of, by key, oldest first and bounded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seen: Vec<String>,
    /// Controls waiting for its Station, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<ChargeControl>,
}

impl ChargeRecord {
    /// The turns it began after `since` (Unix seconds).
    pub fn turns_since(&self, since: f64) -> usize {
        self.turns.iter().filter(|at| **at > since).count()
    }
}

/// What the holder of a charge writes to its record.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChargeUpdate {
    /// The Run and the handler that issued it; `Some(None)` forgets both.
    pub run: Option<Option<(String, String)>>,
    /// When it woke.
    pub last_wake: Option<f64>,
    /// When a turn began: counted towards its hour.
    pub turn_at: Option<f64>,
    /// Lines for its logbook.
    pub log: Vec<ChargeLogEntry>,
    /// What it has been told of now, replacing what it had.
    pub seen: Option<Vec<String>>,
    /// The controls its Station took, by ID.
    pub taken_controls: Vec<String>,
}

impl ChargeUpdate {
    /// Records the Run `run`, issued by `handler`.
    #[must_use]
    pub fn run(mut self, handler: impl Into<String>, run: impl Into<String>) -> Self {
        self.run = Some(Some((handler.into(), run.into())));
        self
    }

    /// Forgets the Run.
    #[must_use]
    pub fn forget_run(mut self) -> Self {
        self.run = Some(None);
        self
    }

    /// Records a wake at `at`, and with `turn` a turn begun then.
    #[must_use]
    pub fn woke(mut self, at: f64, turn: bool) -> Self {
        self.last_wake = Some(at);
        if turn {
            self.turn_at = Some(at);
        }
        self
    }

    /// Adds a line to the logbook.
    #[must_use]
    pub fn log(mut self, entry: ChargeLogEntry) -> Self {
        self.log.push(entry);
        self
    }

    /// Replaces what it has been told of.
    #[must_use]
    pub fn seen(mut self, seen: Vec<String>) -> Self {
        self.seen = Some(seen);
        self
    }

    /// Removes the controls taken, by ID.
    #[must_use]
    pub fn took_controls(mut self, ids: Vec<String>) -> Self {
        self.taken_controls = ids;
        self
    }
}

/// Why a charge record was not claimed or written.
#[derive(Debug)]
#[non_exhaustive]
pub enum ChargeError {
    /// There is no record of that charge.
    NotFound,
    /// Another Station holds a live lease on it.
    HeldBy(String),
    /// The writer no longer holds the lease it presents.
    Stale,
    /// The record or the request is invalid, or the Registry failed.
    Io(io::Error),
}

impl fmt::Display for ChargeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("no such charge"),
            Self::HeldBy(station) => write!(formatter, "the charge is held by Station {station:?}"),
            Self::Stale => formatter.write_str("the charge's lease is held by another"),
            Self::Io(error) => write!(formatter, "charge record: {error}"),
        }
    }
}

impl std::error::Error for ChargeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ChargeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Charge records and their leases.
pub trait ChargeStore {
    /// Takes the lease on the charge of `scope` for `station` until `ttl` from now, making its
    /// record when there is none, or renews it when `station` holds it already. A lapsed lease of
    /// another Station is taken over with a higher fencing token, which the logbook records. A
    /// record whose Agent is not `agent` takes `agent` and forgets its Run, which
    /// was another Agent's.
    ///
    /// # Errors
    ///
    /// `HeldBy` while another Station's lease is live; `Io` for an invalid scope, Agent or
    /// Station, too many records, or a Registry failure.
    fn claim_charge(
        &self,
        scope: &str,
        agent: &str,
        station: &str,
        ttl: Duration,
    ) -> Result<ChargeRecord, ChargeError>;

    /// Applies `update` to the charge of `scope` if `station` still holds it with `fence`, and
    /// with `renew` extends its lease that far from now.
    ///
    /// # Errors
    ///
    /// `NotFound`; `Stale` when another Station took it or `fence` is not the current token;
    /// `Io` for a record out of bounds or a Registry failure.
    fn update_charge(
        &self,
        scope: &str,
        station: &str,
        fence: u64,
        renew: Option<Duration>,
        update: ChargeUpdate,
    ) -> Result<ChargeRecord, ChargeError>;

    /// Asks the charge of `scope` to wake, for `by`, with `note`; its Station gives the note to the
    /// charge with the wake. It needs no lease: a person's word, not a holder's write.
    ///
    /// # Errors
    ///
    /// `NotFound` when the charge has never been claimed; `Io` for an empty or invalid `by`, a
    /// note over its bound, too many controls waiting, or a Registry failure.
    fn request_charge_wake(
        &self,
        scope: &str,
        by: &str,
        note: &str,
    ) -> Result<ChargeControl, ChargeError>;

    /// The record of the charge of `scope`.
    ///
    /// # Errors
    ///
    /// For an invalid scope, or when the record cannot be read.
    fn charge(&self, scope: &str) -> io::Result<Option<ChargeRecord>>;

    /// Every charge record, by scope.
    ///
    /// # Errors
    ///
    /// When a record cannot be read.
    fn charges(&self) -> io::Result<Vec<ChargeRecord>>;
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

/// Checks a scope as the Registry keys it.
pub(super) fn validate_scope(scope: &str) -> io::Result<()> {
    scope.parse::<ChargeScope>().map(drop).map_err(invalid_input)
}

fn validate_agent(agent: &str) -> io::Result<()> {
    if agent.trim().is_empty() || agent.len() > 256 || agent.chars().any(char::is_control) {
        return Err(invalid_input(
            "a charge's Agent must be 1-256 bytes with no control characters",
        ));
    }
    Ok(())
}

/// `text` cut to `limit` bytes on a character boundary.
fn cut(mut text: String, limit: Limit) -> String {
    let room = limit.or_max();
    if text.len() > room {
        let mut end = room;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Keeps the newest `limit` of `items`.
fn keep_newest<T>(items: &mut Vec<T>, limit: Limit) {
    let room = limit.or_max();
    if items.len() > room {
        items.drain(..items.len() - room);
    }
}

fn append_log(record: &mut ChargeRecord, entries: Vec<ChargeLogEntry>, limits: &ChargeLimits) {
    for mut entry in entries {
        entry.text = cut(entry.text, limits.entry_bytes);
        record.logbook.push(entry);
    }
    keep_newest(&mut record.logbook, limits.logbook);
}

/// The charge of `scope` claimed by `station` at `now`: `existing` taken, renewed or taken over,
/// or a new record.
pub(super) fn claim(
    existing: Option<ChargeRecord>,
    (scope, agent, station): (&str, &str, &str),
    ttl: Duration,
    now: f64,
    limits: &ChargeLimits,
) -> Result<ChargeRecord, ChargeError> {
    validate_scope(scope)?;
    validate_agent(agent)?;
    validate_station_id(station)?;
    let lease_expires = now + ttl.as_secs_f64();
    let Some(mut record) = existing else {
        return Ok(ChargeRecord {
            scope: scope.to_owned(),
            agent: agent.to_owned(),
            station: station.to_owned(),
            fence: 1,
            lease_expires,
            opened_at: now,
            provider_id: None,
            handler: None,
            last_wake: None,
            turns: Vec::new(),
            logbook: Vec::new(),
            seen: Vec::new(),
            controls: Vec::new(),
        });
    };
    let mut log = Vec::new();
    if record.station != station {
        if now < record.lease_expires {
            return Err(ChargeError::HeldBy(record.station));
        }
        log.push(ChargeLogEntry::new(
            "station",
            format!(
                "Station {station} took the charge over from Station {}, whose lease lapsed",
                record.station
            ),
            now,
        ));
        record.station = station.to_owned();
        record.fence = record.fence.saturating_add(1);
    }
    if record.agent != agent {
        log.push(ChargeLogEntry::new(
            "agent",
            format!(
                "Agent {agent} keeps the charge in place of {}; its Run starts fresh",
                record.agent
            ),
            now,
        ));
        record.agent = agent.to_owned();
        record.provider_id = None;
        record.handler = None;
    }
    record.lease_expires = lease_expires;
    append_log(&mut record, log, limits);
    Ok(record)
}

/// `update` applied to `record` by `station` presenting `fence` at `now`.
pub(super) fn apply(
    record: &mut ChargeRecord,
    (station, fence): (&str, u64),
    renew: Option<Duration>,
    update: ChargeUpdate,
    now: f64,
    limits: &ChargeLimits,
) -> Result<(), ChargeError> {
    if record.station != station || record.fence != fence {
        return Err(ChargeError::Stale);
    }
    if let Some(run) = update.run {
        match run {
            Some((handler, run)) => {
                if run.is_empty() || run.len() > 512 || run.contains(char::is_control) {
                    return Err(invalid_input("a charge's provider ID is invalid").into());
                }
                record.handler = Some(handler);
                record.provider_id = Some(run);
            }
            None => {
                record.handler = None;
                record.provider_id = None;
            }
        }
    }
    if let Some(at) = update.last_wake {
        record.last_wake = Some(at);
    }
    if let Some(at) = update.turn_at {
        record.turns.push(at);
    }
    record.turns.retain(|at| *at > now - TURN_WINDOW_SECS);
    append_log(record, update.log, limits);
    if let Some(mut seen) = update.seen {
        keep_newest(&mut seen, limits.seen);
        record.seen = seen;
    }
    record
        .controls
        .retain(|control| !update.taken_controls.contains(&control.id));
    if let Some(ttl) = renew {
        record.lease_expires = now + ttl.as_secs_f64();
    }
    Ok(())
}

/// A control from `by` with `note`, added to `record` at `now`.
pub(super) fn add_control(
    record: &mut ChargeRecord,
    by: &str,
    note: &str,
    now: f64,
    limits: &ChargeLimits,
) -> Result<ChargeControl, ChargeError> {
    if by.trim().is_empty() || by.len() > 256 || by.chars().any(char::is_control) {
        return Err(invalid_input("who asked a charge to wake must be 1-256 bytes").into());
    }
    if !limits.entry_bytes.allows(note.len())
        || note
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(invalid_input("a control's note is over its bound or holds controls").into());
    }
    if !limits.controls.allows(record.controls.len() + 1) {
        return Err(invalid_input("too many controls wait for this charge").into());
    }
    let control = ChargeControl {
        id: uuid::Uuid::now_v7().to_string(),
        by: by.to_owned(),
        note: note.to_owned(),
        at: now,
    };
    record.controls.push(control.clone());
    Ok(control)
}

/// The stored form of `record`, refused past its bound.
pub(super) fn record_json(record: &ChargeRecord, limits: &ChargeLimits) -> io::Result<String> {
    validate_scope(&record.scope)?;
    let text = serde_json::to_string(record).map_err(io::Error::other)?;
    if !limits.record_bytes.allows(text.len()) {
        return Err(invalid_data("charge record exceeds the size limit"));
    }
    Ok(text)
}

/// Reads a stored record.
pub(super) fn parse_record(text: &[u8], limits: &ChargeLimits) -> io::Result<ChargeRecord> {
    if !limits.record_bytes.allows(text.len()) {
        return Err(invalid_data("charge record exceeds the size limit"));
    }
    serde_json::from_slice(text)
        .map_err(|error| invalid_data(format!("invalid charge record: {error}")))
}

impl FileRegistry {
    fn charges_dir(&self) -> PathBuf {
        self.root.join("charges")
    }

    fn charge_path(&self, scope: &str) -> PathBuf {
        self.charges_dir()
            .join(format!("{}.json", hex_digest(scope.as_bytes())))
    }

    fn read_charge(&self, scope: &str) -> io::Result<Option<ChargeRecord>> {
        match fs::read(self.charge_path(scope)) {
            Ok(bytes) => parse_record(&bytes, &self.limits.charges).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn write_charge(&self, record: &ChargeRecord) -> io::Result<()> {
        let text = record_json(record, &self.limits.charges)?;
        self.write_atomic(&self.charge_path(&record.scope), text.as_bytes())
    }

    /// Changes the record of `scope` under the directory lock.
    fn change_charge<T>(
        &self,
        scope: &str,
        change: impl FnOnce(&mut ChargeRecord, f64) -> Result<T, ChargeError>,
    ) -> Result<(T, ChargeRecord), ChargeError> {
        validate_scope(scope)?;
        let _lock = self.lock()?;
        let mut record = self.read_charge(scope)?.ok_or(ChargeError::NotFound)?;
        let done = change(&mut record, unix_time()?)?;
        self.write_charge(&record)?;
        Ok((done, record))
    }
}

impl ChargeStore for FileRegistry {
    fn claim_charge(
        &self,
        scope: &str,
        agent: &str,
        station: &str,
        ttl: Duration,
    ) -> Result<ChargeRecord, ChargeError> {
        validate_scope(scope)?;
        let limits = &self.limits.charges;
        let _lock = self.lock()?;
        let existing = self.read_charge(scope)?;
        if existing.is_none()
            && fs::read_dir(self.charges_dir())
                .is_ok_and(|entries| !limits.records.allows(entries.count() + 1))
        {
            return Err(io::Error::other("too many charge records").into());
        }
        let record = claim(existing, (scope, agent, station), ttl, unix_time()?, limits)?;
        self.write_charge(&record)?;
        Ok(record)
    }

    fn update_charge(
        &self,
        scope: &str,
        station: &str,
        fence: u64,
        renew: Option<Duration>,
        update: ChargeUpdate,
    ) -> Result<ChargeRecord, ChargeError> {
        let limits = self.limits.charges.clone();
        self.change_charge(scope, |record, now| {
            apply(record, (station, fence), renew, update, now, &limits)
        })
        .map(|((), record)| record)
    }

    fn request_charge_wake(
        &self,
        scope: &str,
        by: &str,
        note: &str,
    ) -> Result<ChargeControl, ChargeError> {
        let limits = self.limits.charges.clone();
        self.change_charge(scope, |record, now| {
            add_control(record, by, note, now, &limits)
        })
        .map(|(control, _)| control)
    }

    fn charge(&self, scope: &str) -> io::Result<Option<ChargeRecord>> {
        validate_scope(scope)?;
        self.read_charge(scope)
    }

    fn charges(&self) -> io::Result<Vec<ChargeRecord>> {
        let entries = match fs::read_dir(self.charges_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut records = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            records.push(parse_record(&fs::read(&path)?, &self.limits.charges)?);
        }
        records.sort_by(|left, right| left.scope.cmp(&right.scope));
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_charge_is_claimed_renewed_and_taken_over_only_once_its_lease_lapses,
        only_the_holder_writes_and_the_logbook_keeps_the_newest,
        a_control_waits_until_the_holder_takes_it,
    );

    const LONG: Duration = Duration::from_secs(600);

    fn a_charge_is_claimed_renewed_and_taken_over_only_once_its_lease_lapses(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.charge("operation:G").unwrap().is_none());
        let first = registry
            .claim_charge("operation:G", "00", "station-a", LONG)
            .unwrap();
        assert_eq!((first.station.as_str(), first.fence), ("station-a", 1));
        let renewed = registry
            .claim_charge("operation:G", "00", "station-a", LONG)
            .unwrap();
        assert_eq!(renewed.fence, 1, "the holder renews under its own token");
        assert!(matches!(
            registry.claim_charge("operation:G", "00", "station-b", LONG),
            Err(ChargeError::HeldBy(station)) if station == "station-a"
        ));
        registry
            .update_charge(
                "operation:G",
                "station-a",
                1,
                Some(Duration::ZERO),
                ChargeUpdate::default().run("mock", "provider-1"),
            )
            .unwrap();
        let taken = wait_for_takeover(registry.as_ref(), "operation:G", "station-b");
        assert_eq!(taken.fence, 2);
        assert_eq!(taken.provider_id.as_deref(), Some("provider-1"));
        assert_eq!(taken.logbook.last().unwrap().kind, "station");
        assert!(matches!(
            registry.update_charge("operation:G", "station-a", 1, None, ChargeUpdate::default()),
            Err(ChargeError::Stale)
        ));
        let other_agent = registry
            .claim_charge("operation:G", "M", "station-b", LONG)
            .unwrap();
        assert_eq!(
            other_agent.provider_id, None,
            "another Agent's Run"
        );
        assert!(registry.claim_charge("theater:x", "00", "a", LONG).is_err());
        assert_eq!(
            registry
                .charges()
                .unwrap()
                .iter()
                .map(|record| record.scope.as_str())
                .collect::<Vec<_>>(),
            ["operation:G"]
        );
    }

    fn wait_for_takeover<R: ChargeStore + ?Sized>(
        registry: &R,
        scope: &str,
        station: &str,
    ) -> ChargeRecord {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match registry.claim_charge(scope, "00", station, LONG) {
                Ok(record) => return record,
                Err(ChargeError::HeldBy(_)) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("{error}"),
            }
        }
    }

    fn only_the_holder_writes_and_the_logbook_keeps_the_newest(store: &dyn Store) {
        let registry = store.handle();
        let record = registry.claim_charge("m", "M", "hq", LONG).unwrap();
        let limits = ChargeLimits::default();
        let mut update = ChargeUpdate::default().woke(100.0, true);
        for at in 0..limits.logbook.or_max() + 5 {
            update = update.log(ChargeLogEntry::new("turn", format!("turn {at}"), 1.0));
        }
        update = update.log(ChargeLogEntry::new(
            "turn",
            "x".repeat(10 * KIB as usize),
            1.0,
        ));
        let written = registry
            .update_charge("m", "hq", record.fence, None, update)
            .unwrap();
        assert_eq!(written.logbook.len(), limits.logbook.or_max());
        assert_eq!(written.logbook[0].text, "turn 6", "the oldest go");
        assert_eq!(
            written.logbook.last().unwrap().text.len(),
            limits.entry_bytes.or_max(),
            "a long entry is cut"
        );
        assert_eq!(written.last_wake, Some(100.0));
        assert!(
            written.turns.is_empty(),
            "a turn older than the hour is not kept"
        );
        assert!(matches!(
            registry.update_charge("m", "elsewhere", record.fence, None, ChargeUpdate::default()),
            Err(ChargeError::Stale)
        ));
        assert!(matches!(
            registry.update_charge("m", "hq", record.fence + 1, None, ChargeUpdate::default()),
            Err(ChargeError::Stale)
        ));
        assert!(matches!(
            registry.update_charge("station:x", "hq", 1, None, ChargeUpdate::default()),
            Err(ChargeError::NotFound)
        ));
    }

    fn a_control_waits_until_the_holder_takes_it(store: &dyn Store) {
        let registry = store.handle();
        assert!(matches!(
            registry.request_charge_wake("station:s", "person-1", ""),
            Err(ChargeError::NotFound)
        ));
        let record = registry
            .claim_charge("station:s", "chief", "s", LONG)
            .unwrap();
        let control = registry
            .request_charge_wake("station:s", "person-1", "look at the disk")
            .unwrap();
        assert!(registry.request_charge_wake("station:s", " ", "").is_err());
        let waiting = registry.charge("station:s").unwrap().unwrap();
        assert_eq!(waiting.controls.len(), 1);
        let kept = &waiting.controls[0];
        assert_eq!(
            (&kept.id, &kept.by, &kept.note),
            (&control.id, &control.by, &control.note)
        );
        // A time read back from JSON may differ in its last place.
        assert!((kept.at - control.at).abs() < 1e-6);
        let taken = registry
            .update_charge(
                "station:s",
                "s",
                record.fence,
                None,
                ChargeUpdate::default().took_controls(vec![control.id]),
            )
            .unwrap();
        assert!(taken.controls.is_empty());
    }
}
