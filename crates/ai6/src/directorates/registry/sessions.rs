//! Session records: the shared, fenced record of every Agent Session.
//!
//! A Station's own Mission book dies with the Station; this record lives in the Registry so
//! another Station can see what was running and where. Whoever acts on a Session must hold
//! its lease, and every write presents the fencing token the lease was granted with. A token
//! is taken from the Headquarters election term and always grows past the previous holder's,
//! so a Station that lost its lease while paused cannot write after a new holder took over.

use std::fmt;
use std::fs;
use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::directorates::medical::exposure::Exposure;

use super::store::{
    FileRegistry, invalid_data, read_headquarters, registry_lock, unix_time, write_atomic,
};

const MAX_SESSION_RECORD_BYTES: u64 = 65_536;
const MAX_SESSIONS: usize = 100_000;
const MAX_SESSION_ID_BYTES: usize = 128;
const MAX_EXPOSURES_PER_SESSION: usize = 32;

/// Where a Session stands, as far as the Registry knows.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// The holder is running the provider Session.
    Active,
    /// A provider limit paused the Session until `parked_until`.
    Parked,
    /// The Session finished; it can no longer be taken or written.
    Ended,
}

/// One Agent Session as stored in the shared Registry.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    /// AI6's identifier for this Session, distinct from the provider's.
    pub session_id: String,
    /// The Mission the Session works for.
    pub mission_id: String,
    /// The provider-issued Session ID, once the provider has created it.
    pub provider_session_id: Option<String>,
    /// The Station that holds the lease and supervises the Session.
    pub station: String,
    /// The Theater the harness runs in, when known.
    pub theater: Option<String>,
    /// Which try at the Mission this is, starting at 1.
    pub attempt: u32,
    /// The AI6 ID of the Session this one replaced, when it is a later attempt.
    pub predecessor: Option<String>,
    /// Current status.
    pub status: SessionStatus,
    /// When a parked Session may resume, as Unix seconds.
    pub parked_until: Option<f64>,
    /// When the holder's lease lapses, as Unix seconds.
    pub lease_expires: f64,
    /// The fencing token of the current lease; it only grows.
    pub fence: u64,
    /// The untrusted input the Session has read; it only grows and a later attempt at the
    /// Mission inherits it. Empty in records written before exposure was tracked.
    #[serde(default)]
    pub exposure: Vec<Exposure>,
}

impl SessionRecord {
    /// Whether the lease is still live at `now`, in Unix seconds.
    pub fn leased_at(&self, now: f64) -> bool {
        self.status != SessionStatus::Ended && self.lease_expires >= now
    }
}

/// A Session to record, built with defaults for the optional parts.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSession {
    session_id: String,
    mission_id: String,
    station: String,
    provider_session_id: Option<String>,
    theater: Option<String>,
    attempt: u32,
    predecessor: Option<String>,
    exposure: Vec<Exposure>,
}

impl NewSession {
    /// Starts a first attempt held by `station`.
    pub fn new(
        session_id: impl Into<String>,
        mission_id: impl Into<String>,
        station: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            mission_id: mission_id.into(),
            station: station.into(),
            provider_session_id: None,
            theater: None,
            attempt: 1,
            predecessor: None,
            exposure: Vec::new(),
        }
    }

    /// Records the exposure the Session starts with, inherited from its Mission.
    pub fn with_exposure(mut self, exposure: Vec<Exposure>) -> Self {
        self.exposure = exposure;
        self
    }

    /// Records the provider's Session ID when it is already known.
    pub fn with_provider_session(mut self, id: impl Into<String>) -> Self {
        self.provider_session_id = Some(id.into());
        self
    }

    /// Records the Theater the harness runs in.
    pub fn with_theater(mut self, theater: impl Into<String>) -> Self {
        self.theater = Some(theater.into());
        self
    }

    /// Marks this as a later attempt that replaces the Session `predecessor`.
    pub fn with_attempt(mut self, attempt: u32, predecessor: Option<String>) -> Self {
        self.attempt = attempt.max(1);
        self.predecessor = predecessor;
        self
    }
}

/// Changes a lease holder may make to a Session record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionUpdate {
    provider_session_id: Option<String>,
    status: Option<SessionStatus>,
    parked_until: Option<f64>,
    exposure: Vec<Exposure>,
}

impl SessionUpdate {
    /// Adds exposures the record does not have yet; an exposure is never removed.
    pub fn exposed(mut self, exposure: Vec<Exposure>) -> Self {
        self.exposure = exposure;
        self
    }

    /// Records the provider's Session ID.
    pub fn provider_session(mut self, id: impl Into<String>) -> Self {
        self.provider_session_id = Some(id.into());
        self
    }

    /// Sets the status. `Active` clears `parked_until`.
    pub fn status(mut self, status: SessionStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Sets when a parked Session may resume, in Unix seconds.
    pub fn parked_until(mut self, until: f64) -> Self {
        self.parked_until = Some(until);
        self
    }
}

/// Why a Session record could not be read or written by a Station.
#[non_exhaustive]
#[derive(Debug)]
pub enum SessionError {
    /// No record with this ID exists.
    NotFound,
    /// A record with this ID already exists.
    Exists,
    /// The Session finished and is closed to every Station.
    Ended,
    /// Another Station holds a live lease on the Session.
    HeldBy(String),
    /// The Station's lease or fencing token is no longer the current one.
    Stale,
    /// The Registry could not be read or written.
    Io(io::Error),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("no such Session record"),
            Self::Exists => f.write_str("a Session record with this ID already exists"),
            Self::Ended => f.write_str("the Session has ended"),
            Self::HeldBy(station) => write!(f, "Station {station:?} holds the Session lease"),
            Self::Stale => f.write_str("the Session lease or fencing token is stale"),
            Self::Io(error) => write!(f, "Session record: {error}"),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for SessionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn validate_session_id(id: &str) -> io::Result<()> {
    let valid = !id.is_empty()
        && id.len() <= MAX_SESSION_ID_BYTES
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Session ID must be 1-128 letters, digits, '-', '_' or '.' and not start with '.'",
        ))
    }
}

impl FileRegistry {
    fn session_path(&self, id: &str) -> std::path::PathBuf {
        self.root.join("sessions").join(format!("{id}.json"))
    }

    fn headquarters_term(&self) -> io::Result<u64> {
        let path = self.root.join("headquarters").join("lease.json");
        Ok(read_headquarters(&path)?.map_or(0, |lease| lease.term()))
    }

    /// Records a new Session held by its Station and returns it with its fencing token.
    ///
    /// # Errors
    ///
    /// `Exists` when the ID is taken; `Io` for an invalid ID, record or Registry failure.
    pub fn open_session(
        &self,
        new: NewSession,
        ttl: Duration,
    ) -> Result<SessionRecord, SessionError> {
        self.open_session_at(new, ttl, unix_time()?)
    }

    pub(crate) fn open_session_at(
        &self,
        new: NewSession,
        ttl: Duration,
        now: f64,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(&new.session_id)?;
        super::store::validate_station_id(&new.station)?;
        let _lock = registry_lock(&self.root)?;
        let path = self.session_path(&new.session_id);
        if path.exists() {
            return Err(SessionError::Exists);
        }
        if fs::read_dir(self.root.join("sessions"))
            .is_ok_and(|entries| entries.count() >= MAX_SESSIONS)
        {
            return Err(io::Error::other("too many Session records").into());
        }
        let record = SessionRecord {
            session_id: new.session_id,
            mission_id: new.mission_id,
            provider_session_id: new.provider_session_id,
            station: new.station,
            theater: new.theater,
            attempt: new.attempt,
            predecessor: new.predecessor,
            status: SessionStatus::Active,
            parked_until: None,
            lease_expires: now + ttl.as_secs_f64(),
            fence: self.headquarters_term()?.max(1),
            exposure: new.exposure,
        };
        write_session(&path, &record)?;
        Ok(record)
    }

    /// Reads one Session record.
    pub fn session(&self, id: &str) -> io::Result<Option<SessionRecord>> {
        validate_session_id(id)?;
        let _lock = registry_lock(&self.root)?;
        read_session(&self.session_path(id))
    }

    /// Every way any Session of a Mission was exposed, earliest first, one entry per source.
    ///
    /// A later attempt inherits this, so exposure cannot be shed by starting again.
    ///
    /// # Errors
    ///
    /// Returns an error when the Session records cannot be read.
    pub fn mission_exposure(&self, mission_id: &str) -> io::Result<Vec<Exposure>> {
        let mut merged: Vec<Exposure> = Vec::new();
        for record in self
            .sessions()?
            .into_iter()
            .filter(|record| record.mission_id == mission_id)
        {
            for exposure in record.exposure {
                match merged
                    .iter_mut()
                    .find(|known| known.source == exposure.source)
                {
                    Some(known) => known.at = known.at.min(exposure.at),
                    None => merged.push(exposure),
                }
            }
        }
        merged.sort_by(|left, right| left.at.total_cmp(&right.at));
        Ok(merged)
    }

    /// Lists every Session record in stable ID order.
    pub fn sessions(&self) -> io::Result<Vec<SessionRecord>> {
        let _lock = registry_lock(&self.root)?;
        let mut records = Vec::new();
        let entries = match fs::read_dir(self.root.join("sessions")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(records),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && let Some(record) = read_session(&path)?
            {
                records.push(record);
            }
        }
        records.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        Ok(records)
    }

    /// Takes the lease on a Session for `station`, or renews it when `station` already holds it.
    ///
    /// A different holder's live lease is respected. A lapsed one is taken over with a fencing
    /// token above the previous holder's, which makes that holder's writes stale.
    ///
    /// # Errors
    ///
    /// `NotFound`, `Ended`, `HeldBy` while another Station's lease is live, or `Io`.
    pub fn take_session(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
    ) -> Result<SessionRecord, SessionError> {
        self.take_session_at(id, station, ttl, unix_time()?)
    }

    pub(crate) fn take_session_at(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
        now: f64,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(id)?;
        super::store::validate_station_id(station)?;
        let _lock = registry_lock(&self.root)?;
        let path = self.session_path(id);
        let mut record = read_session(&path)?.ok_or(SessionError::NotFound)?;
        if record.status == SessionStatus::Ended {
            return Err(SessionError::Ended);
        }
        if record.station != station {
            if record.leased_at(now) {
                return Err(SessionError::HeldBy(record.station));
            }
            record.station = station.to_owned();
            record.fence = self.headquarters_term()?.max(record.fence + 1);
        }
        record.lease_expires = now + ttl.as_secs_f64();
        write_session(&path, &record)?;
        Ok(record)
    }

    /// Extends the lease `station` holds on a Session, if it still holds it with `fence`.
    ///
    /// # Errors
    ///
    /// `Stale` when another Station took the Session or `fence` is not the current token.
    pub fn renew_session(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        ttl: Duration,
    ) -> Result<SessionRecord, SessionError> {
        self.write_fenced(
            id,
            station,
            fence,
            unix_time()?,
            Some(ttl),
            SessionUpdate::default(),
        )
    }

    /// Applies `update` to a Session if `station` still holds it with `fence`.
    ///
    /// # Errors
    ///
    /// `Stale` when another Station took the Session or `fence` is not the current token.
    pub fn update_session(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        update: SessionUpdate,
    ) -> Result<SessionRecord, SessionError> {
        self.write_fenced(id, station, fence, unix_time()?, None, update)
    }

    pub(crate) fn write_fenced(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        now: f64,
        renew: Option<Duration>,
        update: SessionUpdate,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(id)?;
        let _lock = registry_lock(&self.root)?;
        let path = self.session_path(id);
        let mut record = read_session(&path)?.ok_or(SessionError::NotFound)?;
        if record.station != station || record.fence != fence {
            return Err(SessionError::Stale);
        }
        if record.status == SessionStatus::Ended {
            return Err(SessionError::Ended);
        }
        if let Some(ttl) = renew {
            record.lease_expires = now + ttl.as_secs_f64();
        }
        if let Some(provider) = update.provider_session_id {
            record.provider_session_id = Some(provider);
        }
        for exposure in update.exposure {
            if record.exposure.len() < MAX_EXPOSURES_PER_SESSION
                && !record
                    .exposure
                    .iter()
                    .any(|known| known.source == exposure.source)
            {
                record.exposure.push(exposure);
            }
        }
        if let Some(status) = update.status {
            record.status = status;
            record.parked_until = match status {
                SessionStatus::Parked => update.parked_until.or(record.parked_until),
                _ => None,
            };
        }
        write_session(&path, &record)?;
        Ok(record)
    }
}

fn read_session(path: &std::path::Path) -> io::Result<Option<SessionRecord>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if bytes.len() as u64 > MAX_SESSION_RECORD_BYTES {
        return Err(invalid_data("Session record exceeds the size limit"));
    }
    let record: SessionRecord = serde_json::from_slice(&bytes)
        .map_err(|error| invalid_data(format!("invalid Session record: {error}")))?;
    if !record.lease_expires.is_finite() || record.attempt == 0 {
        return Err(invalid_data("invalid Session record"));
    }
    Ok(Some(record))
}

fn write_session(path: &std::path::Path, record: &SessionRecord) -> io::Result<()> {
    let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_SESSION_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Session record exceeds the size limit",
        ));
    }
    write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::store::DEFAULT_LEASE;

    const NOW: f64 = 1_800_000_000.0;
    const TTL: Duration = Duration::from_secs(60);

    fn registry() -> (tempfile::TempDir, FileRegistry) {
        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        (root, registry)
    }

    fn new_session() -> NewSession {
        NewSession::new("s-1", "mission-1", "station-a").with_theater("local")
    }

    #[test]
    fn two_stations_sharing_a_registry_see_the_same_record() {
        let (root, first) = registry();
        let second = FileRegistry::open(root.path());
        let opened = first
            .open_session_at(new_session().with_provider_session("provider-1"), TTL, NOW)
            .unwrap();

        let seen = second.session("s-1").unwrap().unwrap();
        assert_eq!(seen, opened);
        assert_eq!(seen.station, "station-a");
        assert_eq!(seen.theater.as_deref(), Some("local"));
        assert_eq!(seen.provider_session_id.as_deref(), Some("provider-1"));
        assert_eq!(seen.attempt, 1);
        assert_eq!(second.sessions().unwrap(), vec![seen]);
        assert!(matches!(
            second.open_session_at(new_session(), TTL, NOW),
            Err(SessionError::Exists)
        ));
    }

    #[test]
    fn a_later_attempt_links_to_its_predecessor() {
        let (_root, registry) = registry();
        registry.open_session_at(new_session(), TTL, NOW).unwrap();
        let next = registry
            .open_session_at(
                NewSession::new("s-2", "mission-1", "station-a")
                    .with_attempt(2, Some("s-1".to_owned())),
                TTL,
                NOW,
            )
            .unwrap();
        assert_eq!(
            (next.attempt, next.predecessor.as_deref()),
            (2, Some("s-1"))
        );
    }

    #[test]
    fn a_stale_fencing_token_is_refused() {
        let (_root, registry) = registry();
        let first = registry.open_session_at(new_session(), TTL, NOW).unwrap();

        // The first holder stops renewing; the second takes the lapsed lease.
        let later = NOW + 120.0;
        let taken = registry
            .take_session_at("s-1", "station-b", TTL, later)
            .unwrap();
        assert_eq!(taken.station, "station-b");
        assert!(taken.fence > first.fence);

        // Its old token, and even the right Station name with the old token, are refused.
        let update = SessionUpdate::default().provider_session("late-write");
        for (station, fence) in [("station-a", first.fence), ("station-b", first.fence)] {
            assert!(matches!(
                registry.write_fenced("s-1", station, fence, later, None, update.clone()),
                Err(SessionError::Stale)
            ));
        }
        assert!(matches!(
            registry.write_fenced("s-1", "station-a", taken.fence, later, None, update.clone()),
            Err(SessionError::Stale)
        ));
        let record = registry.session("s-1").unwrap().unwrap();
        assert_eq!(record.provider_session_id, None);

        // The current holder's token still works.
        let written = registry
            .write_fenced("s-1", "station-b", taken.fence, later, None, update)
            .unwrap();
        assert_eq!(written.provider_session_id.as_deref(), Some("late-write"));
    }

    #[test]
    fn a_live_lease_cannot_be_taken_but_the_holder_renews_it() {
        let (_root, registry) = registry();
        let opened = registry.open_session_at(new_session(), TTL, NOW).unwrap();
        assert!(matches!(
            registry.take_session_at("s-1", "station-b", TTL, NOW + 30.0),
            Err(SessionError::HeldBy(holder)) if holder == "station-a"
        ));
        let renewed = registry
            .take_session_at("s-1", "station-a", TTL, NOW + 30.0)
            .unwrap();
        assert_eq!(renewed.fence, opened.fence);
        assert!(renewed.lease_expires > opened.lease_expires);
        assert!(matches!(
            registry.take_session_at("s-1", "station-b", TTL, NOW + 80.0),
            Err(SessionError::HeldBy(_))
        ));
        assert!(
            registry
                .take_session_at("s-1", "station-b", TTL, NOW + 200.0)
                .is_ok()
        );
    }

    #[test]
    fn the_fencing_token_follows_the_headquarters_term_and_only_grows() {
        let (_root, registry) = registry();
        registry.elect("hq-a", DEFAULT_LEASE).unwrap();
        let first = registry.open_session_at(new_session(), TTL, NOW).unwrap();
        assert_eq!(
            first.fence,
            registry.headquarters().unwrap().unwrap().term()
        );

        // A takeover inside the same election term still gets a larger token.
        let taken = registry
            .take_session_at("s-1", "station-b", TTL, NOW + 500.0)
            .unwrap();
        assert!(taken.fence > first.fence);
        let again = registry
            .take_session_at("s-1", "station-c", TTL, NOW + 1_000.0)
            .unwrap();
        assert!(again.fence > taken.fence);
    }

    #[test]
    fn status_changes_are_fenced_and_an_ended_session_is_closed() {
        let (_root, registry) = registry();
        let opened = registry.open_session_at(new_session(), TTL, NOW).unwrap();
        let parked = registry
            .write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW,
                None,
                SessionUpdate::default()
                    .status(SessionStatus::Parked)
                    .parked_until(NOW + 3_600.0),
            )
            .unwrap();
        assert_eq!(parked.status, SessionStatus::Parked);
        assert_eq!(parked.parked_until, Some(NOW + 3_600.0));
        let active = registry
            .write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW,
                None,
                SessionUpdate::default().status(SessionStatus::Active),
            )
            .unwrap();
        assert_eq!(active.parked_until, None);
        registry
            .write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW,
                None,
                SessionUpdate::default().status(SessionStatus::Ended),
            )
            .unwrap();
        assert!(matches!(
            registry.take_session_at("s-1", "station-b", TTL, NOW + 1_000.0),
            Err(SessionError::Ended)
        ));
        assert!(matches!(
            registry.write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW,
                None,
                SessionUpdate::default()
            ),
            Err(SessionError::Ended)
        ));
    }

    #[test]
    fn unsafe_ids_are_rejected_before_touching_the_filesystem() {
        let (_root, registry) = registry();
        for id in ["", "../escape", "a/b", ".hidden", &"x".repeat(129)] {
            assert!(matches!(
                registry.open_session_at(NewSession::new(id, "m", "station-a"), TTL, NOW),
                Err(SessionError::Io(_))
            ));
        }
        assert!(registry.session("../escape").is_err());
    }
}
