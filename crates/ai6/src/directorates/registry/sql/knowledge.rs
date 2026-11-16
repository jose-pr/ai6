//! Knowledge in SQL: the revisions of a Report beside its row, and search through the backend's
//! own text index.
//!
//! SQLite keeps a Report's words in the FTS5 table `ai6_reports_search`, PostgreSQL in a
//! generated `tsvector`. Either only narrows what a search looks at: nothing here ranks or
//! counts, since a database's statistics would rest on Reports the asker may not read. A Report
//! with no words indexed (one stored before the index existed) is always a candidate.

use std::io;

use super::db::{Dialect, Value};
use super::reports::{known_held, put};
use super::{SqlRegistry, from_json, to_json};
use crate::directorates::registry::knowledge::{
    KnowledgeStore, ReportQuery, ReportWriteError, check_room, replacing, unforgettable,
};
use crate::directorates::registry::reports::{Report, ReportScope, parse_report, report_bytes};
use crate::directorates::registry::store::validate_registry_key;

impl KnowledgeStore for SqlRegistry {
    fn write_report(&self, next: &Report, base: u64) -> Result<Report, ReportWriteError> {
        let limits = self.current_limits().reports.clone();
        next.validate(&limits)?;
        self.write(|tx, revision| {
            let current = tx
                .query_opt(
                    "SELECT record FROM ai6_reports WHERE id = $1",
                    &[next.id.as_str().into()],
                )?
                .ok_or(ReportWriteError::Missing)?;
            let current = parse_report(current.text(0)?.as_bytes(), &limits)?;
            let stored = replacing(&limits, next, &current, base)?;
            let held = known_held(tx, &stored.scope.to_string())?;
            check_room(&limits, &stored, held, Some(current.size()))?;
            if limits.versions.allows(1u64) {
                tx.execute(
                    "INSERT INTO ai6_report_versions (id, report_revision, record, revision) \
                     VALUES ($1, $2, $3, $4)",
                    &[
                        current.id.as_str().into(),
                        Value::Int(i64::try_from(current.revision).map_err(io::Error::other)?),
                        to_json(&current)?.into(),
                        revision.into(),
                    ],
                )?;
            }
            let kept = tx.query(
                "SELECT report_revision FROM ai6_report_versions WHERE id = $1 \
                 ORDER BY report_revision DESC",
                &[current.id.as_str().into()],
            )?;
            for (index, row) in kept.iter().enumerate() {
                if !limits.versions.allows(index + 1) {
                    tx.execute(
                        "DELETE FROM ai6_report_versions WHERE id = $1 AND report_revision = $2",
                        &[current.id.as_str().into(), Value::Int(row.int(0)?)],
                    )?;
                }
            }
            let text =
                String::from_utf8(report_bytes(&stored, &limits)?).map_err(io::Error::other)?;
            put(self, tx, &stored, text, revision)?;
            Ok(stored)
        })
    }

    fn report_versions(&self, id: &str) -> io::Result<Vec<Report>> {
        if validate_registry_key(id).is_err() {
            return Ok(Vec::new());
        }
        let rows = self.read(|tx| {
            tx.query(
                "SELECT record FROM ai6_report_versions WHERE id = $1 \
                 ORDER BY report_revision DESC",
                &[id.into()],
            )
        })?;
        Ok(rows
            .iter()
            .filter_map(|row| from_json::<Report>(&row.text(0).ok()?, "report").ok())
            .collect())
    }

    fn forget_report(&self, id: &str, base: Option<u64>) -> Result<Report, ReportWriteError> {
        if validate_registry_key(id).is_err() {
            return Err(ReportWriteError::Missing);
        }
        let limits = self.current_limits().reports.clone();
        self.write(|tx, _| {
            let found = tx
                .query_opt("SELECT record FROM ai6_reports WHERE id = $1", &[id.into()])?
                .ok_or(ReportWriteError::Missing)?;
            let report = parse_report(found.text(0)?.as_bytes(), &limits)?;
            if let Some(refusal) = unforgettable(&report) {
                return Err(refusal);
            }
            if base.is_some_and(|base| base != report.revision) {
                return Err(ReportWriteError::Changed {
                    revision: report.revision,
                });
            }
            tx.execute(
                "DELETE FROM ai6_report_versions WHERE id = $1",
                &[id.into()],
            )?;
            tx.execute(
                "DELETE FROM ai6_report_links WHERE report_id = $1",
                &[id.into()],
            )?;
            tx.execute("DELETE FROM ai6_reports WHERE id = $1", &[id.into()])?;
            #[cfg(feature = "sqlite")]
            if self.db.dialect() == Dialect::Sqlite {
                tx.execute("DELETE FROM ai6_reports_search WHERE id = $1", &[id.into()])?;
            }
            Ok(report)
        })
    }

    fn report_scopes(&self) -> io::Result<Vec<ReportScope>> {
        let rows = self.read(|tx| tx.query("SELECT DISTINCT scope FROM ai6_reports", &[]))?;
        Ok(rows
            .iter()
            .filter_map(|row| row.text(0).ok()?.parse().ok())
            .collect())
    }

    fn report_candidates(
        &self,
        scopes: &[ReportScope],
        query: &ReportQuery,
    ) -> io::Result<Vec<Report>> {
        if scopes.is_empty() {
            return Ok(Vec::new());
        }
        let limits = self.current_limits().reports.clone();
        let mut params: Vec<Value> = scopes
            .iter()
            .map(|scope| scope.to_string().into())
            .collect();
        let places: Vec<String> = (1..=scopes.len())
            .map(|index| format!("${index}"))
            .collect();
        let mut narrowed = String::new();
        if let Some(kind) = &query.kind {
            params.push(kind.as_str().into());
            narrowed.push_str(&format!(" AND kind = ${}", params.len()));
        }
        if !query.terms.is_empty() {
            // Each term is letters and digits alone, so it cannot be read as query syntax.
            match self.db.dialect() {
                #[cfg(feature = "sqlite")]
                Dialect::Sqlite => {
                    let any: Vec<String> = query
                        .terms
                        .iter()
                        .map(|term| format!("\"{term}\""))
                        .collect();
                    params.push(any.join(" OR ").into());
                    narrowed.push_str(&format!(
                        " AND (id IN (SELECT id FROM ai6_reports_search \
                         WHERE ai6_reports_search MATCH ${}) \
                         OR id NOT IN (SELECT id FROM ai6_reports_search))",
                        params.len()
                    ));
                }
                #[cfg(feature = "postgres")]
                Dialect::Postgres => {
                    let any: Vec<String> =
                        query.terms.iter().map(|term| format!("'{term}'")).collect();
                    params.push(any.join(" | ").into());
                    narrowed.push_str(&format!(
                        " AND (search @@ to_tsquery('simple', ${}) OR words = '')",
                        params.len()
                    ));
                }
            }
        }
        let rows = self.read(|tx| {
            tx.query(
                &format!(
                    "SELECT record FROM ai6_reports WHERE scope IN ({}){narrowed}",
                    places.join(", ")
                ),
                &params,
            )
        })?;
        Ok(rows
            .iter()
            .filter_map(|row| parse_report(row.text(0).ok()?.as_bytes(), &limits).ok())
            .collect())
    }
}
