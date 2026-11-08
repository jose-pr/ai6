//! Watch records and their fenced leases in `ai6_watches`, one row per scope.

use std::io;
use std::time::Duration;

use super::SqlRegistry;
use super::db::Tx;
use crate::directorates::registry::limits::RegistryLimits;
use crate::directorates::registry::watches::{
    WatchControl, WatchError, WatchRecord, WatchStore, WatchUpdate, add_control, apply, claim,
    parse_record, record_json, validate_scope,
};

fn watch_in(
    tx: &mut Tx<'_>,
    scope: &str,
    limits: &RegistryLimits,
) -> io::Result<Option<WatchRecord>> {
    tx.query_opt(
        "SELECT record FROM ai6_watches WHERE scope = $1",
        &[scope.into()],
    )?
    .map(|row| parse_record(row.text(0)?.as_bytes(), &limits.watches))
    .transpose()
}

fn put_watch(
    tx: &mut Tx<'_>,
    record: &WatchRecord,
    revision: i64,
    limits: &RegistryLimits,
) -> io::Result<()> {
    let text = record_json(record, &limits.watches)?;
    tx.execute(
        "INSERT INTO ai6_watches (scope, record, revision) VALUES ($1, $2, $3) \
         ON CONFLICT (scope) DO UPDATE SET record = excluded.record, revision = excluded.revision",
        &[record.scope.as_str().into(), text.into(), revision.into()],
    )?;
    Ok(())
}

impl SqlRegistry {
    /// Changes the record of `scope` on the database's clock.
    fn change_watch<T>(
        &self,
        scope: &str,
        change: impl FnOnce(&mut WatchRecord, f64) -> Result<T, WatchError>,
    ) -> Result<(T, WatchRecord), WatchError> {
        validate_scope(scope)?;
        self.write(|tx, revision| {
            let mut record = watch_in(tx, scope, &self.limits)?.ok_or(WatchError::NotFound)?;
            let now = tx.now()?;
            let done = change(&mut record, now)?;
            put_watch(tx, &record, revision, &self.limits)?;
            Ok((done, record))
        })
    }
}

impl WatchStore for SqlRegistry {
    fn claim_watch(
        &self,
        scope: &str,
        agent: &str,
        station: &str,
        ttl: Duration,
    ) -> Result<WatchRecord, WatchError> {
        validate_scope(scope)?;
        self.write(|tx, revision| {
            let existing = watch_in(tx, scope, &self.limits)?;
            if existing.is_none() {
                let count = tx
                    .query_opt("SELECT count(*) FROM ai6_watches", &[])?
                    .map_or(Ok(0), |row| row.int(0))?;
                if !self.limits.watches.records.allows(count + 1) {
                    return Err(io::Error::other("too many watch records").into());
                }
            }
            let now = tx.now()?;
            let record = claim(
                existing,
                (scope, agent, station),
                ttl,
                now,
                &self.limits.watches,
            )?;
            put_watch(tx, &record, revision, &self.limits)?;
            Ok(record)
        })
    }

    fn update_watch(
        &self,
        scope: &str,
        station: &str,
        fence: u64,
        renew: Option<Duration>,
        update: WatchUpdate,
    ) -> Result<WatchRecord, WatchError> {
        let limits = self.limits.watches.clone();
        self.change_watch(scope, |record, now| {
            apply(record, (station, fence), renew, update, now, &limits)
        })
        .map(|((), record)| record)
    }

    fn request_watch_wake(
        &self,
        scope: &str,
        by: &str,
        note: &str,
    ) -> Result<WatchControl, WatchError> {
        let limits = self.limits.watches.clone();
        self.change_watch(scope, |record, now| {
            add_control(record, by, note, now, &limits)
        })
        .map(|(control, _)| control)
    }

    fn watch(&self, scope: &str) -> io::Result<Option<WatchRecord>> {
        validate_scope(scope)?;
        self.read(|tx| watch_in(tx, scope, &self.limits))
    }

    fn watches(&self) -> io::Result<Vec<WatchRecord>> {
        self.read(|tx| {
            tx.query("SELECT record FROM ai6_watches ORDER BY scope", &[])?
                .into_iter()
                .map(|row| parse_record(row.text(0)?.as_bytes(), &self.limits.watches))
                .collect()
        })
    }
}
