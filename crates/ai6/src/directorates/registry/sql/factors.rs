//! Factor documents in SQL: one row per family and key, each holding its text.

use std::io;

use super::SqlRegistry;
use crate::directorates::registry::factors::{
    FactorChange, FactorEdit, FactorKeep, FactorStore, validate_factor_key,
};
use crate::directorates::registry::store::{invalid_data, validate_registry_key};

impl FactorStore for SqlRegistry {
    fn factor_get(&self, family: &str, key: &str) -> io::Result<Option<String>> {
        validate_registry_key(family)?;
        validate_factor_key(key)?;
        self.read(|tx| {
            tx.query_opt(
                "SELECT text FROM ai6_factors WHERE family = $1 AND doc_key = $2",
                &[family.into(), key.into()],
            )?
            .map(|row| row.text(0))
            .transpose()
        })
    }

    fn factor_documents(&self, family: &str) -> io::Result<Vec<(String, String)>> {
        validate_registry_key(family)?;
        let rows = self.read(|tx| {
            tx.query(
                "SELECT doc_key, text FROM ai6_factors WHERE family = $1 ORDER BY doc_key",
                &[family.into()],
            )
        })?;
        rows.into_iter()
            .map(|row| Ok((row.text(0)?, row.text(1)?)))
            .collect()
    }

    fn factor_update(&self, family: &str, key: &str, edit: &mut FactorEdit<'_>) -> io::Result<()> {
        validate_registry_key(family)?;
        validate_factor_key(key)?;
        self.write(|tx, revision| {
            let held = tx
                .query_opt(
                    "SELECT text FROM ai6_factors WHERE family = $1 AND doc_key = $2",
                    &[family.into(), key.into()],
                )?
                .map(|row| row.text(0))
                .transpose()?;
            match edit(held.as_deref())? {
                FactorChange::Keep => Ok(()),
                FactorChange::Put(text) => {
                    if !self.current_limits().tokens.record_bytes.allows(text.len()) {
                        return Err(invalid_data("a factor document exceeds the size limit"));
                    }
                    tx.execute(
                        "INSERT INTO ai6_factors (family, doc_key, text, revision) \
                         VALUES ($1, $2, $3, $4) ON CONFLICT (family, doc_key) \
                         DO UPDATE SET text = excluded.text, revision = excluded.revision",
                        &[family.into(), key.into(), text.into(), revision.into()],
                    )?;
                    Ok(())
                }
                FactorChange::Remove => {
                    tx.execute(
                        "DELETE FROM ai6_factors WHERE family = $1 AND doc_key = $2",
                        &[family.into(), key.into()],
                    )?;
                    Ok(())
                }
            }
        })
    }

    fn factor_retain(&self, family: &str, keep: &mut FactorKeep<'_>) -> io::Result<usize> {
        validate_registry_key(family)?;
        self.write(|tx, _| {
            let rows = tx.query(
                "SELECT doc_key, text FROM ai6_factors WHERE family = $1",
                &[family.into()],
            )?;
            let mut removed = 0;
            for row in rows {
                let (key, text) = (row.text(0)?, row.text(1)?);
                if !keep(&key, &text) {
                    tx.execute(
                        "DELETE FROM ai6_factors WHERE family = $1 AND doc_key = $2",
                        &[family.into(), key.as_str().into()],
                    )?;
                    removed += 1;
                }
            }
            Ok(removed)
        })
    }
}
