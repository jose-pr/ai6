//! Sealed values and Stations' sealing keys: one row each, written in the transaction that
//! serializes every Registry writer.

use std::collections::BTreeMap;
use std::io;

use super::SqlRegistry;
use crate::directorates::registry::sealed::{
    SealedCopy, SealedExpect, SealedRecord, SealedStore, SealedWriteError, StationSealingKey,
    next_revision, parse_sealed, sealed_bytes, validate_name,
};
use crate::directorates::registry::store::invalid_data;

impl SealedStore for SqlRegistry {
    fn publish_sealing_key(&self, key: &StationSealingKey) -> io::Result<()> {
        validate_name(&key.station)?;
        let text = serde_json::to_string(key).map_err(io::Error::other)?;
        self.write(|tx, revision| {
            tx.execute(
                "INSERT INTO ai6_sealing_keys (station, record, revision) VALUES ($1, $2, $3) \
                 ON CONFLICT (station) DO UPDATE SET record = excluded.record, \
                 revision = excluded.revision",
                &[key.station.as_str().into(), text.into(), revision.into()],
            )?;
            Ok(())
        })
    }

    fn sealing_key(&self, station: &str) -> io::Result<Option<StationSealingKey>> {
        validate_name(station)?;
        self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_sealing_keys WHERE station = $1",
                &[station.into()],
            )?
            .map(|row| {
                serde_json::from_str(&row.text(0)?)
                    .map_err(|_| invalid_data("a sealing key's record cannot be read"))
            })
            .transpose()
        })
    }

    fn sealed_value(&self, name: &str) -> io::Result<Option<SealedRecord>> {
        validate_name(name)?;
        let limits = &self.limits.accounts;
        self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_sealed_values WHERE name = $1",
                &[name.into()],
            )?
            .map(|row| parse_sealed(row.text(0)?.as_bytes(), limits))
            .transpose()
        })
    }

    fn sealed_revisions(&self) -> io::Result<BTreeMap<String, u64>> {
        let limits = &self.limits.accounts;
        self.read(|tx| {
            tx.query("SELECT record FROM ai6_sealed_values", &[])?
                .into_iter()
                .map(|row| {
                    let record = parse_sealed(row.text(0)?.as_bytes(), limits)?;
                    Ok((record.name, record.revision))
                })
                .collect()
        })
    }

    fn store_sealed(
        &self,
        name: &str,
        copies: &BTreeMap<String, SealedCopy>,
        writer: &str,
        expect: SealedExpect,
        now: f64,
    ) -> Result<u64, SealedWriteError> {
        validate_name(name)?;
        let limits = &self.limits.accounts;
        self.write(|tx, clock| {
            let held = tx
                .query_opt(
                    "SELECT record FROM ai6_sealed_values WHERE name = $1",
                    &[name.into()],
                )?
                .map(|row| parse_sealed(row.text(0)?.as_bytes(), limits))
                .transpose()?
                .map(|record| record.revision);
            let revision = next_revision(held, expect)?;
            let record = SealedRecord {
                name: name.to_owned(),
                revision,
                copies: copies.clone(),
                written_by: writer.to_owned(),
                written_at: now,
            };
            let text = String::from_utf8(sealed_bytes(&record, limits)?)
                .map_err(|_| invalid_data("a sealed value's record is not UTF-8"))?;
            tx.execute(
                "INSERT INTO ai6_sealed_values (name, record, revision) VALUES ($1, $2, $3) \
                 ON CONFLICT (name) DO UPDATE SET record = excluded.record, \
                 revision = excluded.revision",
                &[name.into(), text.into(), clock.into()],
            )?;
            Ok(revision)
        })
    }

    fn remove_sealed(&self, name: &str) -> io::Result<bool> {
        validate_name(name)?;
        self.write(|tx, _| {
            Ok(tx.execute(
                "DELETE FROM ai6_sealed_values WHERE name = $1",
                &[name.into()],
            )? > 0)
        })
    }
}
