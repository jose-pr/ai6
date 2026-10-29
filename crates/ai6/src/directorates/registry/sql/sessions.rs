//! Session records and their fenced leases in `ai6_sessions`.

use std::io;
use std::time::Duration;

use super::SqlRegistry;
use super::db::Tx;
use super::stations::headquarters_in;
use crate::directorates::medical::exposure::Exposure;
use crate::directorates::registry::limits::SessionLimits;
use crate::directorates::registry::sessions::{
    NewSession, SessionError, SessionRecord, SessionStore, SessionUpdate, apply_fenced, latest,
    lose, merged_exposure, opened, parse_session, session_json, take, validate_new_session,
    validate_session_id,
};
use crate::directorates::registry::store::{invalid_data, validate_station_id};

fn term(tx: &mut Tx<'_>) -> io::Result<u64> {
    Ok(headquarters_in(tx)?.map_or(0, |lease| lease.term()))
}

fn session_in(
    tx: &mut Tx<'_>,
    id: &str,
    limits: &SessionLimits,
) -> io::Result<Option<SessionRecord>> {
    tx.query_opt(
        "SELECT record FROM ai6_sessions WHERE session_id = $1",
        &[id.into()],
    )?
    .map(|row| parse_session(row.text(0)?.as_bytes(), limits))
    .transpose()
}

fn put_session(
    tx: &mut Tx<'_>,
    record: &SessionRecord,
    revision: i64,
    limits: &SessionLimits,
) -> io::Result<()> {
    let text = String::from_utf8(session_json(record, limits)?)
        .map_err(|_| invalid_data("Session record is not UTF-8"))?;
    tx.execute(
        "INSERT INTO ai6_sessions (session_id, mission_id, record, revision) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (session_id) \
         DO UPDATE SET mission_id = excluded.mission_id, record = excluded.record, \
         revision = excluded.revision",
        &[
            record.session_id.as_str().into(),
            record.mission_id.as_str().into(),
            text.into(),
            revision.into(),
        ],
    )?;
    Ok(())
}

fn records(
    tx: &mut Tx<'_>,
    mission: Option<&str>,
    limits: &SessionLimits,
) -> io::Result<Vec<SessionRecord>> {
    let rows = match mission {
        Some(mission) => tx.query(
            "SELECT record FROM ai6_sessions WHERE mission_id = $1 ORDER BY session_id",
            &[mission.into()],
        )?,
        None => tx.query("SELECT record FROM ai6_sessions ORDER BY session_id", &[])?,
    };
    rows.into_iter()
        .map(|row| parse_session(row.text(0)?.as_bytes(), limits))
        .collect()
}

/// When a change happens: at a given time, or now on the database's clock.
fn when(tx: &mut Tx<'_>, at: Option<f64>) -> io::Result<f64> {
    at.map_or_else(|| tx.now(), Ok)
}

impl SqlRegistry {
    /// Changes one Session record under the Registry clock at `at` (the database's clock when
    /// `None`); `change` may read the Headquarters term in the same transaction.
    fn change_session(
        &self,
        id: &str,
        at: Option<f64>,
        change: impl FnOnce(&mut SessionRecord, &mut Tx<'_>, f64) -> Result<(), SessionError>,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(id, &self.limits.sessions)?;
        self.write(|tx, revision| {
            let mut record =
                session_in(tx, id, &self.limits.sessions)?.ok_or(SessionError::NotFound)?;
            let now = when(tx, at)?;
            change(&mut record, tx, now)?;
            put_session(tx, &record, revision, &self.limits.sessions)?;
            Ok(record)
        })
    }

    fn open_session_when(
        &self,
        new: NewSession,
        ttl: Duration,
        at: Option<f64>,
    ) -> Result<SessionRecord, SessionError> {
        validate_new_session(&new, &self.limits.sessions)?;
        self.write(|tx, revision| {
            if session_in(tx, &new.session_id, &self.limits.sessions)?.is_some() {
                return Err(SessionError::Exists);
            }
            let count = tx
                .query_opt("SELECT count(*) FROM ai6_sessions", &[])?
                .map_or(Ok(0), |row| row.int(0))?;
            if !self.limits.sessions.records.allows(count + 1) {
                return Err(io::Error::other("too many Session records").into());
            }
            let now = when(tx, at)?;
            let record = opened(new, ttl, now, term(tx)?);
            put_session(tx, &record, revision, &self.limits.sessions)?;
            Ok(record)
        })
    }

    fn take_session_when(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
        at: Option<f64>,
    ) -> Result<SessionRecord, SessionError> {
        validate_station_id(station)?;
        self.change_session(id, at, |record, tx, now| {
            take(record, station, ttl, now, || term(tx))
        })
    }

    fn write_fenced_when(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        at: Option<f64>,
        renew: Option<Duration>,
        update: SessionUpdate,
    ) -> Result<SessionRecord, SessionError> {
        self.change_session(id, at, |record, _, now| {
            apply_fenced(
                record,
                station,
                fence,
                now,
                renew,
                update,
                &self.limits.sessions,
            )
        })
    }

    fn lose_session_when(
        &self,
        id: &str,
        reason: &str,
        at: Option<f64>,
    ) -> Result<SessionRecord, SessionError> {
        self.change_session(id, at, |record, tx, now| {
            lose(record, reason, now, || term(tx), &self.limits.sessions)
        })
    }

    #[cfg(test)]
    pub(crate) fn open_session_at(
        &self,
        new: NewSession,
        ttl: Duration,
        now: f64,
    ) -> Result<SessionRecord, SessionError> {
        self.open_session_when(new, ttl, Some(now))
    }

    #[cfg(test)]
    pub(crate) fn take_session_at(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
        now: f64,
    ) -> Result<SessionRecord, SessionError> {
        self.take_session_when(id, station, ttl, Some(now))
    }

    #[cfg(test)]
    pub(crate) fn write_fenced(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        now: f64,
        renew: Option<Duration>,
        update: SessionUpdate,
    ) -> Result<SessionRecord, SessionError> {
        self.write_fenced_when(id, station, fence, Some(now), renew, update)
    }

    #[cfg(test)]
    pub(crate) fn lose_session_at(
        &self,
        id: &str,
        reason: &str,
        now: f64,
    ) -> Result<SessionRecord, SessionError> {
        self.lose_session_when(id, reason, Some(now))
    }
}

impl SessionStore for SqlRegistry {
    fn open_session(&self, new: NewSession, ttl: Duration) -> Result<SessionRecord, SessionError> {
        self.open_session_when(new, ttl, None)
    }

    fn session(&self, id: &str) -> io::Result<Option<SessionRecord>> {
        validate_session_id(id, &self.limits.sessions)?;
        self.read(|tx| session_in(tx, id, &self.limits.sessions))
    }

    fn mission_exposure(&self, mission_id: &str) -> io::Result<Vec<Exposure>> {
        Ok(merged_exposure(self.read(|tx| {
            records(tx, Some(mission_id), &self.limits.sessions)
        })?))
    }

    fn sessions(&self) -> io::Result<Vec<SessionRecord>> {
        self.read(|tx| records(tx, None, &self.limits.sessions))
    }

    fn take_session(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
    ) -> Result<SessionRecord, SessionError> {
        self.take_session_when(id, station, ttl, None)
    }

    fn renew_session(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        ttl: Duration,
    ) -> Result<SessionRecord, SessionError> {
        self.write_fenced_when(
            id,
            station,
            fence,
            None,
            Some(ttl),
            SessionUpdate::default(),
        )
    }

    fn update_session(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        update: SessionUpdate,
    ) -> Result<SessionRecord, SessionError> {
        self.write_fenced_when(id, station, fence, None, None, update)
    }

    fn lose_session(&self, id: &str, reason: &str) -> Result<SessionRecord, SessionError> {
        self.lose_session_when(id, reason, None)
    }

    fn latest_session(&self, mission_id: &str) -> io::Result<Option<SessionRecord>> {
        Ok(latest(self.read(|tx| {
            records(tx, Some(mission_id), &self.limits.sessions)
        })?))
    }
}
