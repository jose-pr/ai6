//! Charge records and their fenced leases in `ai6_charges`, one row per scope.

use std::io;
use std::time::Duration;

use super::SqlRegistry;
use super::db::Tx;
use crate::directorates::registry::charges::{
    ChargeControl, ChargeError, ChargeRecord, ChargeStore, ChargeUpdate, add_control, apply, claim,
    parse_record, record_json, validate_scope,
};
use crate::directorates::registry::limits::RegistryLimits;

fn charge_in(
    tx: &mut Tx<'_>,
    scope: &str,
    limits: &RegistryLimits,
) -> io::Result<Option<ChargeRecord>> {
    tx.query_opt(
        "SELECT record FROM ai6_charges WHERE scope = $1",
        &[scope.into()],
    )?
    .map(|row| parse_record(row.text(0)?.as_bytes(), &limits.charges))
    .transpose()
}

fn put_charge(
    tx: &mut Tx<'_>,
    record: &ChargeRecord,
    revision: i64,
    limits: &RegistryLimits,
) -> io::Result<()> {
    let text = record_json(record, &limits.charges)?;
    tx.execute(
        "INSERT INTO ai6_charges (scope, record, revision) VALUES ($1, $2, $3) \
         ON CONFLICT (scope) DO UPDATE SET record = excluded.record, revision = excluded.revision",
        &[record.scope.as_str().into(), text.into(), revision.into()],
    )?;
    Ok(())
}

impl SqlRegistry {
    /// Changes the record of `scope` on the database's clock.
    fn change_charge<T>(
        &self,
        scope: &str,
        change: impl FnOnce(&mut ChargeRecord, f64) -> Result<T, ChargeError>,
    ) -> Result<(T, ChargeRecord), ChargeError> {
        validate_scope(scope)?;
        self.write(|tx, revision| {
            let mut record = charge_in(tx, scope, &self.limits)?.ok_or(ChargeError::NotFound)?;
            let now = tx.now()?;
            let done = change(&mut record, now)?;
            put_charge(tx, &record, revision, &self.limits)?;
            Ok((done, record))
        })
    }
}

impl ChargeStore for SqlRegistry {
    fn claim_charge(
        &self,
        scope: &str,
        agent: &str,
        station: &str,
        ttl: Duration,
    ) -> Result<ChargeRecord, ChargeError> {
        validate_scope(scope)?;
        self.write(|tx, revision| {
            let existing = charge_in(tx, scope, &self.limits)?;
            if existing.is_none() {
                let count = tx
                    .query_opt("SELECT count(*) FROM ai6_charges", &[])?
                    .map_or(Ok(0), |row| row.int(0))?;
                if !self.limits.charges.records.allows(count + 1) {
                    return Err(io::Error::other("too many charge records").into());
                }
            }
            let now = tx.now()?;
            let record = claim(
                existing,
                (scope, agent, station),
                ttl,
                now,
                &self.limits.charges,
            )?;
            put_charge(tx, &record, revision, &self.limits)?;
            Ok(record)
        })
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
        self.read(|tx| charge_in(tx, scope, &self.limits))
    }

    fn charges(&self) -> io::Result<Vec<ChargeRecord>> {
        self.read(|tx| {
            tx.query("SELECT record FROM ai6_charges ORDER BY scope", &[])?
                .into_iter()
                .map(|row| parse_record(row.text(0)?.as_bytes(), &self.limits.charges))
                .collect()
        })
    }
}
