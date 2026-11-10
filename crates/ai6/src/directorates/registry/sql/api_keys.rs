//! Personal API keys in SQL: one row per key, holding its record as JSON beside the owner.

use std::io;

use super::SqlRegistry;
use crate::directorates::registry::api_keys::{
    ApiKeyRecord, ApiKeyStore, api_key_bytes, parse_api_key, touches,
};
use crate::directorates::registry::store::{invalid_data, validate_registry_key};

impl ApiKeyStore for SqlRegistry {
    fn put_api_key(&self, key: &ApiKeyRecord) -> io::Result<()> {
        key.validate(&self.limits.api_keys)?;
        let text = String::from_utf8(api_key_bytes(key, &self.limits.api_keys)?)
            .map_err(io::Error::other)?;
        self.write(|tx, revision| {
            let exists = tx
                .query_opt(
                    "SELECT id FROM ai6_api_keys WHERE id = $1",
                    &[key.id.as_str().into()],
                )?
                .is_some();
            if !exists {
                let count = tx
                    .query_opt("SELECT count(*) FROM ai6_api_keys", &[])?
                    .map_or(Ok(0), |row| row.int(0))?;
                if !self.limits.api_keys.keys.allows(count + 1) {
                    return Err(invalid_data("too many API keys"));
                }
            }
            tx.execute(
                "INSERT INTO ai6_api_keys (id, owner, record, revision) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (id) \
                 DO UPDATE SET owner = excluded.owner, record = excluded.record, \
                 revision = excluded.revision",
                &[
                    key.id.as_str().into(),
                    key.owner.as_str().into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn api_key(&self, id: &str) -> io::Result<Option<ApiKeyRecord>> {
        validate_registry_key(id)?;
        let row = self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_api_keys WHERE id = $1",
                &[id.into()],
            )
        })?;
        row.map(|row| parse_api_key(row.text(0)?.as_bytes(), &self.limits.api_keys))
            .transpose()
    }

    fn api_keys(&self, owner: Option<&str>) -> io::Result<Vec<ApiKeyRecord>> {
        let rows = self.read(|tx| match owner {
            Some(owner) => tx.query(
                "SELECT record FROM ai6_api_keys WHERE owner = $1",
                &[owner.into()],
            ),
            None => tx.query("SELECT record FROM ai6_api_keys", &[]),
        })?;
        let mut keys = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(key) = parse_api_key(row.text(0)?.as_bytes(), &self.limits.api_keys) {
                keys.push(key);
            }
        }
        keys.sort_by(|left, right| {
            left.created_at
                .total_cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(keys)
    }

    fn remove_api_key(&self, id: &str) -> io::Result<Option<ApiKeyRecord>> {
        validate_registry_key(id)?;
        self.write(|tx, _| {
            let Some(row) = tx.query_opt(
                "SELECT record FROM ai6_api_keys WHERE id = $1",
                &[id.into()],
            )?
            else {
                return Ok(None);
            };
            let key = parse_api_key(row.text(0)?.as_bytes(), &self.limits.api_keys)?;
            tx.execute("DELETE FROM ai6_api_keys WHERE id = $1", &[id.into()])?;
            Ok(Some(key))
        })
    }

    fn touch_api_key(&self, id: &str, at: f64, within: f64) -> io::Result<bool> {
        validate_registry_key(id)?;
        self.write(|tx, revision| {
            let Some(row) = tx.query_opt(
                "SELECT record FROM ai6_api_keys WHERE id = $1",
                &[id.into()],
            )?
            else {
                return Ok(false);
            };
            let mut key = parse_api_key(row.text(0)?.as_bytes(), &self.limits.api_keys)?;
            if !touches(&key, at, within) {
                return Ok(false);
            }
            key.last_used_at = Some(at);
            let text = String::from_utf8(api_key_bytes(&key, &self.limits.api_keys)?)
                .map_err(io::Error::other)?;
            tx.execute(
                "UPDATE ai6_api_keys SET record = $1, revision = $2 WHERE id = $3",
                &[text.into(), revision.into(), id.into()],
            )?;
            Ok(true)
        })
    }
}
