//! Memories in SQL: one row per entry, its words in the backend's own text index.
//!
//! SQLite keeps them in the FTS5 table `ai6_memories_search`, PostgreSQL in a generated `tsvector`.
//! Either only narrows what a search looks at: nothing here ranks or counts, since a database's
//! statistics would rest on entries the reader may not see.

use std::io;

use super::SqlRegistry;
use super::db::{Dialect, Row, Tx, Value};
use crate::directorates::registry::limits::MemoryLimits;
use crate::directorates::registry::memories::{
    MemoryEntry, MemoryError, MemoryHeader, MemoryProposal, MemoryScope, MemorySource, MemoryStore,
    PROPOSALS_FULL, index_words, is_memory_id, newest, oldest, replacing, room_for,
};
use crate::directorates::registry::store::invalid_data;

const HEADER: &str = "id, scope, name, summary, created_at, updated_at, updated_by, source, \
                      compartments, entry_revision, bytes";

/// The columns of [`HEADER`], then the content when there is one.
fn header_of(row: &Row) -> io::Result<MemoryHeader> {
    let scope = row.text(1)?;
    let source: MemorySource =
        serde_json::from_str(&row.text(7)?).map_err(|error| invalid_data(error.to_string()))?;
    let compartments: Vec<String> =
        serde_json::from_str(&row.text(8)?).map_err(|error| invalid_data(error.to_string()))?;
    Ok(MemoryHeader {
        id: row.text(0)?,
        scope: MemoryScope::parse(&scope)
            .ok_or_else(|| invalid_data(format!("{scope:?} is not a memory scope")))?,
        name: row.opt_text(2)?,
        summary: row.text(3)?,
        created_at: row.real(4)?,
        updated_at: row.real(5)?,
        updated_by: row.text(6)?,
        source,
        compartments,
        revision: u64::try_from(row.int(9)?).map_err(|error| invalid_data(error.to_string()))?,
        bytes: u64::try_from(row.int(10)?).map_err(|error| invalid_data(error.to_string()))?,
    })
}

fn entry_of(row: &Row) -> io::Result<MemoryEntry> {
    Ok(MemoryEntry {
        header: header_of(row)?,
        content: row.text(11)?,
    })
}

/// The entry `id` as this transaction sees it.
fn find(tx: &mut Tx<'_>, id: &str) -> io::Result<Option<MemoryEntry>> {
    tx.query_opt(
        &format!("SELECT {HEADER}, content FROM ai6_memories WHERE id = $1"),
        &[id.into()],
    )?
    .map(|row| entry_of(&row))
    .transpose()
}

fn to_json(value: &impl serde::Serialize) -> io::Result<String> {
    serde_json::to_string(value).map_err(io::Error::other)
}

impl SqlRegistry {
    /// Keeps `entry`'s words where its backend looks for them, in the transaction writing it.
    fn set_search_words(&self, tx: &mut Tx<'_>, entry: &MemoryEntry) -> io::Result<()> {
        let id = entry.header.id.as_str();
        let words = index_words(entry, self.limits.memories.index_bytes);
        match self.db.dialect() {
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => {
                tx.execute(
                    "DELETE FROM ai6_memories_search WHERE id = $1",
                    &[id.into()],
                )?;
                tx.execute(
                    "INSERT INTO ai6_memories_search (id, words) VALUES ($1, $2)",
                    &[id.into(), words.into()],
                )?;
            }
            #[cfg(feature = "postgres")]
            Dialect::Postgres => {
                tx.execute(
                    "UPDATE ai6_memories SET words = $2 WHERE id = $1",
                    &[id.into(), words.into()],
                )?;
            }
        }
        Ok(())
    }

    /// Stores `entry` as its row, new or over the old one.
    fn put_row(&self, tx: &mut Tx<'_>, entry: &MemoryEntry, revision: i64) -> io::Result<()> {
        let header = &entry.header;
        let (extra_column, extra_value) = match self.db.dialect() {
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => ("", ""),
            #[cfg(feature = "postgres")]
            Dialect::Postgres => (", words", ", ''"),
        };
        tx.execute(
            &format!(
                "INSERT INTO ai6_memories (id, scope, name, summary, content, created_at, \
                 updated_at, updated_by, source, compartments, entry_revision, bytes, revision\
                 {extra_column}) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13{extra_value}) \
                 ON CONFLICT (id) DO UPDATE SET name = excluded.name, summary = excluded.summary, \
                 content = excluded.content, updated_at = excluded.updated_at, \
                 updated_by = excluded.updated_by, source = excluded.source, \
                 compartments = excluded.compartments, \
                 entry_revision = excluded.entry_revision, bytes = excluded.bytes, \
                 revision = excluded.revision"
            ),
            &[
                header.id.as_str().into(),
                header.scope.to_string().into(),
                header.name.clone().into(),
                header.summary.as_str().into(),
                entry.content.as_str().into(),
                header.created_at.into(),
                header.updated_at.into(),
                header.updated_by.as_str().into(),
                to_json(&header.source)?.into(),
                to_json(&header.compartments)?.into(),
                Value::Int(i64::try_from(header.revision).map_err(io::Error::other)?),
                Value::Int(i64::try_from(header.bytes).map_err(io::Error::other)?),
                revision.into(),
            ],
        )?;
        self.set_search_words(tx, entry)
    }

    /// Keeps `previous` as a prior version and drops the oldest past `[..] versions`.
    fn keep_version(
        &self,
        tx: &mut Tx<'_>,
        previous: &MemoryEntry,
        revision: i64,
    ) -> io::Result<()> {
        let keep = self.limits.memories.versions;
        let id = previous.header.id.as_str();
        if keep.allows(1u64) {
            tx.execute(
                "INSERT INTO ai6_memory_versions (id, entry_revision, record, revision) \
                 VALUES ($1, $2, $3, $4)",
                &[
                    id.into(),
                    Value::Int(i64::try_from(previous.header.revision).map_err(io::Error::other)?),
                    to_json(previous)?.into(),
                    revision.into(),
                ],
            )?;
        }
        let kept = tx.query(
            "SELECT entry_revision FROM ai6_memory_versions WHERE id = $1 \
             ORDER BY entry_revision DESC",
            &[id.into()],
        )?;
        for (index, row) in kept.iter().enumerate() {
            if !keep.allows(index + 1) {
                tx.execute(
                    "DELETE FROM ai6_memory_versions WHERE id = $1 AND entry_revision = $2",
                    &[id.into(), Value::Int(row.int(0)?)],
                )?;
            }
        }
        Ok(())
    }
}

impl MemoryStore for SqlRegistry {
    fn memory_limits(&self) -> &MemoryLimits {
        &self.limits.memories
    }

    fn write_memory(
        &self,
        mut entry: MemoryEntry,
        base: Option<u64>,
    ) -> Result<MemoryEntry, MemoryError> {
        let limits = self.limits.memories.clone();
        entry.validate(&limits).map_err(MemoryError::Invalid)?;
        entry.header.bytes = entry.size();
        self.write(|tx, revision| {
            let existing = find(tx, &entry.header.id)?;
            let totals = tx
                .query_opt(
                    "SELECT count(*), CAST(coalesce(sum(bytes), 0) AS BIGINT) FROM ai6_memories \
                     WHERE scope = $1",
                    &[entry.header.scope.to_string().into()],
                )?
                .ok_or_else(|| invalid_data("a scope could not be counted"))?;
            let count = u64::try_from(totals.int(0)?).unwrap_or(u64::MAX);
            let bytes = u64::try_from(totals.int(1)?).unwrap_or(u64::MAX);
            let stored = match (existing, base) {
                (Some(current), None) => {
                    return Err(MemoryError::Changed {
                        revision: current.header.revision,
                    });
                }
                (None, Some(_)) => return Err(MemoryError::Missing),
                (None, None) => {
                    room_for(&entry, count, bytes, None, &limits)?;
                    let mut first = entry.clone();
                    first.header.revision = 1;
                    first
                }
                (Some(current), Some(base)) => {
                    let next = replacing(entry.clone(), &current, base)?;
                    room_for(&next, count, bytes, Some(current.header.bytes), &limits)?;
                    self.keep_version(tx, &current, revision)?;
                    next
                }
            };
            self.put_row(tx, &stored, revision)?;
            Ok(stored)
        })
    }

    fn memory(&self, id: &str) -> io::Result<Option<MemoryEntry>> {
        if !is_memory_id(id) {
            return Ok(None);
        }
        self.read(|tx| find(tx, id))
    }

    fn memory_headers(&self, scope: &MemoryScope) -> io::Result<Vec<MemoryHeader>> {
        let rows = self.read(|tx| {
            tx.query(
                &format!("SELECT {HEADER} FROM ai6_memories WHERE scope = $1"),
                &[scope.to_string().into()],
            )
        })?;
        let mut headers: Vec<MemoryHeader> =
            rows.iter().filter_map(|row| header_of(row).ok()).collect();
        newest(&mut headers);
        Ok(headers)
    }

    fn memory_candidates(
        &self,
        scopes: &[MemoryScope],
        terms: &[String],
    ) -> io::Result<Vec<MemoryEntry>> {
        if scopes.is_empty() {
            return Ok(Vec::new());
        }
        let mut params: Vec<Value> = scopes
            .iter()
            .map(|scope| scope.to_string().into())
            .collect();
        let places: Vec<String> = (1..=scopes.len())
            .map(|index| format!("${index}"))
            .collect();
        let narrowed = if terms.is_empty() {
            String::new()
        } else {
            let at = scopes.len() + 1;
            // Each term is letters and digits alone, so it cannot be read as query syntax.
            match self.db.dialect() {
                #[cfg(feature = "sqlite")]
                Dialect::Sqlite => {
                    let any: Vec<String> = terms.iter().map(|term| format!("\"{term}\"")).collect();
                    params.push(any.join(" OR ").into());
                    format!(
                        " AND id IN (SELECT id FROM ai6_memories_search \
                         WHERE ai6_memories_search MATCH ${at})"
                    )
                }
                #[cfg(feature = "postgres")]
                Dialect::Postgres => {
                    let any: Vec<String> = terms.iter().map(|term| format!("'{term}'")).collect();
                    params.push(any.join(" | ").into());
                    format!(" AND search @@ to_tsquery('simple', ${at})")
                }
            }
        };
        let rows = self.read(|tx| {
            tx.query(
                &format!(
                    "SELECT {HEADER}, content FROM ai6_memories WHERE scope IN ({}){narrowed}",
                    places.join(", ")
                ),
                &params,
            )
        })?;
        Ok(rows.iter().filter_map(|row| entry_of(row).ok()).collect())
    }

    fn memory_versions(&self, id: &str) -> io::Result<Vec<MemoryEntry>> {
        if !is_memory_id(id) {
            return Ok(Vec::new());
        }
        let rows = self.read(|tx| {
            tx.query(
                "SELECT record FROM ai6_memory_versions WHERE id = $1 \
                 ORDER BY entry_revision DESC",
                &[id.into()],
            )
        })?;
        Ok(rows
            .iter()
            .filter_map(|row| serde_json::from_str(&row.text(0).ok()?).ok())
            .collect())
    }

    fn forget_memory(&self, id: &str, base: Option<u64>) -> Result<MemoryEntry, MemoryError> {
        if !is_memory_id(id) {
            return Err(MemoryError::Missing);
        }
        self.write(|tx, _| {
            let entry = find(tx, id)?.ok_or(MemoryError::Missing)?;
            if base.is_some_and(|base| base != entry.header.revision) {
                return Err(MemoryError::Changed {
                    revision: entry.header.revision,
                });
            }
            tx.execute(
                "DELETE FROM ai6_memory_versions WHERE id = $1",
                &[id.into()],
            )?;
            tx.execute("DELETE FROM ai6_memories WHERE id = $1", &[id.into()])?;
            #[cfg(feature = "sqlite")]
            if self.db.dialect() == Dialect::Sqlite {
                tx.execute(
                    "DELETE FROM ai6_memories_search WHERE id = $1",
                    &[id.into()],
                )?;
            }
            Ok(entry)
        })
    }

    fn propose_memory(&self, proposal: &MemoryProposal) -> Result<(), MemoryError> {
        let limits = self.limits.memories.clone();
        proposal.validate(&limits).map_err(MemoryError::Invalid)?;
        let record = to_json(proposal)?;
        self.write(|tx, revision| {
            let waiting = tx
                .query_opt(
                    "SELECT count(*) FROM ai6_memory_proposals WHERE scope = $1",
                    &[proposal.scope.to_string().into()],
                )?
                .ok_or_else(|| invalid_data("a scope's proposals could not be counted"))?
                .int(0)?;
            if !limits
                .proposals
                .allows(u64::try_from(waiting).unwrap_or(u64::MAX).saturating_add(1))
            {
                return Err(MemoryError::Full(PROPOSALS_FULL.to_owned()));
            }
            tx.execute(
                "INSERT INTO ai6_memory_proposals (id, scope, record, revision) \
                 VALUES ($1, $2, $3, $4)",
                &[
                    proposal.id.as_str().into(),
                    proposal.scope.to_string().into(),
                    record.as_str().into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn memory_proposals(&self, scope: &MemoryScope) -> io::Result<Vec<MemoryProposal>> {
        let rows = self.read(|tx| {
            tx.query(
                "SELECT record FROM ai6_memory_proposals WHERE scope = $1",
                &[scope.to_string().into()],
            )
        })?;
        let mut found: Vec<MemoryProposal> = rows
            .iter()
            .filter_map(|row| serde_json::from_str(&row.text(0).ok()?).ok())
            .collect();
        oldest(&mut found);
        Ok(found)
    }

    fn memory_proposal(&self, id: &str) -> io::Result<Option<MemoryProposal>> {
        if !is_memory_id(id) {
            return Ok(None);
        }
        self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_memory_proposals WHERE id = $1",
                &[id.into()],
            )
        })?
        .map(|row| {
            serde_json::from_str(&row.text(0)?)
                .map_err(|error| invalid_data(format!("invalid proposal: {error}")))
        })
        .transpose()
    }

    fn drop_memory_proposal(&self, id: &str) -> io::Result<bool> {
        if !is_memory_id(id) {
            return Ok(false);
        }
        self.write(|tx, _| {
            Ok::<_, io::Error>(
                tx.execute(
                    "DELETE FROM ai6_memory_proposals WHERE id = $1",
                    &[id.into()],
                )? > 0,
            )
        })
    }
}
