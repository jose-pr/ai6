//! Station check-ins and settings, and the Headquarters lease.

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use super::SqlRegistry;
use super::db::Tx;
use crate::directorates::registry::store::{
    HeadquartersLease, MAX_STATION_RECORD_BYTES, MAX_STATIONS, StationRecord, StationRecordFile,
    StationSettings, StationStore, check_election, check_in_record, check_settings, elected,
    invalid_data, unix_time, validate_station_id,
};

/// The Headquarters lease, read in the transaction that may replace it.
pub(super) fn headquarters_in(tx: &mut Tx<'_>) -> io::Result<Option<HeadquartersLease>> {
    let Some(row) = tx.query_opt(
        "SELECT station, expires, term FROM ai6_headquarters WHERE id = 1",
        &[],
    )?
    else {
        return Ok(None);
    };
    let station = row.text(0)?;
    let expires = row.real(1)?;
    validate_station_id(&station)?;
    if !expires.is_finite() {
        return Err(invalid_data("invalid Headquarters lease expiry"));
    }
    Ok(Some(HeadquartersLease {
        station,
        expires,
        term: u64::try_from(row.int(2)?).map_err(|_| invalid_data("invalid Headquarters term"))?,
    }))
}

impl StationStore for SqlRegistry {
    fn check_in(&self, station: &str, info: BTreeMap<String, serde_json::Value>) -> io::Result<()> {
        let record = String::from_utf8(check_in_record(station, info)?)
            .map_err(|_| invalid_data("station record is not UTF-8"))?;
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_stations (station, record, revision) VALUES ($1, $2, $3) \
                 ON CONFLICT (station) DO UPDATE SET record = excluded.record, \
                 revision = excluded.revision",
                &[station.into(), record.into(), revision.into()],
            )?;
            Ok(())
        })
    }

    fn stations(&self, within: Duration) -> io::Result<Vec<StationRecord>> {
        let rows = self.read(|tx| {
            tx.query(
                "SELECT record FROM ai6_stations ORDER BY station LIMIT $1",
                &[(MAX_STATIONS + 1).into()],
            )
        })?;
        if rows.len() > MAX_STATIONS {
            return Err(invalid_data(format!(
                "Registry has more than {MAX_STATIONS} station records"
            )));
        }
        let now = unix_time()?;
        let window = within.as_secs_f64();
        let mut stations = Vec::with_capacity(rows.len());
        for row in rows {
            let text = row.text(0)?;
            if text.len() as u64 > MAX_STATION_RECORD_BYTES {
                continue;
            }
            let Ok(file) = serde_json::from_str::<StationRecordFile>(&text) else {
                continue;
            };
            let Ok(record) = file.into_station_record() else {
                continue;
            };
            if now - record.seen() <= window {
                stations.push(record);
            }
        }
        Ok(stations)
    }

    fn station_settings(&self, station: &str) -> io::Result<Option<StationSettings>> {
        validate_station_id(station)?;
        let capacity = self.read(|tx| {
            tx.query_opt(
                "SELECT max_concurrent_missions FROM ai6_station_settings WHERE station = $1",
                &[station.into()],
            )?
            .map(|row| row.int(0))
            .transpose()
        })?;
        capacity
            .map(|capacity| {
                usize::try_from(capacity)
                    .ok()
                    .and_then(|capacity| StationSettings::try_new(station, capacity).ok())
                    .ok_or_else(|| invalid_data("Station settings are invalid"))
            })
            .transpose()
    }

    fn set_station_settings(&self, station: &str, settings: &StationSettings) -> io::Result<()> {
        check_settings(station, settings)?;
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_station_settings (station, max_concurrent_missions, revision) \
                 VALUES ($1, $2, $3) ON CONFLICT (station) DO UPDATE SET \
                 max_concurrent_missions = excluded.max_concurrent_missions, \
                 revision = excluded.revision",
                &[
                    station.into(),
                    settings.max_concurrent_missions().into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn elect(&self, station: &str, ttl: Duration) -> io::Result<bool> {
        check_election(station, ttl)?;
        self.write(|tx, revision| {
            let current = headquarters_in(tx)?;
            let Some(lease) = elected(current.as_ref(), station, ttl, unix_time()?) else {
                return Ok(false);
            };
            tx.execute(
                "INSERT INTO ai6_headquarters (id, station, expires, term, revision) \
                 VALUES (1, $1, $2, $3, $4) ON CONFLICT (id) DO UPDATE SET \
                 station = excluded.station, expires = excluded.expires, term = excluded.term, \
                 revision = excluded.revision",
                &[
                    lease.station().into(),
                    lease.expires().into(),
                    i64::try_from(lease.term())
                        .map_err(|_| invalid_data("Headquarters term overflow"))?
                        .into(),
                    revision.into(),
                ],
            )?;
            Ok(true)
        })
    }

    fn headquarters(&self) -> io::Result<Option<HeadquartersLease>> {
        self.read(headquarters_in)
    }
}
