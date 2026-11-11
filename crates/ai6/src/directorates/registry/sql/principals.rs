//! Principals in SQL: one row per person, holding the record as JSON beside the ID.

use std::io;

use super::SqlRegistry;
use crate::directorates::registry::principals::{
    PrincipalRecord, PrincipalStore, merged, parse_principal, principal_bytes, sorted,
};
use crate::directorates::registry::store::invalid_data;

impl PrincipalStore for SqlRegistry {
    fn note_principal(&self, seen: &PrincipalRecord) -> io::Result<bool> {
        let limits = &self.limits.principals;
        seen.validate(limits)?;
        self.write(|tx, revision| {
            let existing = tx
                .query_opt(
                    "SELECT record FROM ai6_principals WHERE id = $1",
                    &[seen.id.as_str().into()],
                )?
                .map(|row| parse_principal(row.text(0)?.as_bytes(), limits))
                .transpose()?;
            let fresh = existing.is_none();
            let Some(next) = merged(existing, seen, limits) else {
                return Ok(false);
            };
            if fresh {
                let count = tx
                    .query_opt("SELECT count(*) FROM ai6_principals", &[])?
                    .map_or(Ok(0), |row| row.int(0))?;
                if !limits.principals.allows(count + 1) {
                    return Err(invalid_data("too many principals"));
                }
            }
            let text =
                String::from_utf8(principal_bytes(&next, limits)?).map_err(io::Error::other)?;
            tx.execute(
                "INSERT INTO ai6_principals (id, record, revision) VALUES ($1, $2, $3) \
                 ON CONFLICT (id) DO UPDATE SET record = excluded.record, \
                 revision = excluded.revision",
                &[seen.id.as_str().into(), text.into(), revision.into()],
            )?;
            Ok(true)
        })
    }

    fn principal(&self, id: &str) -> io::Result<Option<PrincipalRecord>> {
        let row = self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_principals WHERE id = $1",
                &[id.into()],
            )
        })?;
        row.map(|row| parse_principal(row.text(0)?.as_bytes(), &self.limits.principals))
            .transpose()
    }

    fn principals(&self) -> io::Result<Vec<PrincipalRecord>> {
        let rows = self.read(|tx| tx.query("SELECT record FROM ai6_principals", &[]))?;
        let mut people = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(person) = parse_principal(row.text(0)?.as_bytes(), &self.limits.principals) {
                people.push(person);
            }
        }
        Ok(sorted(people))
    }
}
