//! Reports in SQL: one row per Report holding its record as JSON beside what a list filters and
//! orders by, one row per link, and one row per rated source.

use std::io;

use super::db::{Tx, Value};
use super::{SqlRegistry, from_json, to_json};
use crate::directorates::registry::reports::{
    Report, ReportChange, ReportClearance, ReportFilter, ReportSource, ReportState, ReportStore,
    apply_change, check_rate, closed_before, parse_report, report_bytes, window_start,
};
use crate::directorates::registry::store::{invalid_data, validate_registry_key};

/// Writes `report`'s row and its links, replacing what it had.
fn put(tx: &mut Tx<'_>, report: &Report, text: String, revision: i64) -> io::Result<()> {
    tx.execute(
        "DELETE FROM ai6_report_links WHERE report_id = $1",
        &[report.id.as_str().into()],
    )?;
    tx.execute(
        "DELETE FROM ai6_reports WHERE id = $1",
        &[report.id.as_str().into()],
    )?;
    tx.execute(
        "INSERT INTO ai6_reports \
         (id, scope, state, kind, session_id, source_key, priority, deadline, credibility, \
          created, updated, record, revision) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        &[
            report.id.as_str().into(),
            report.scope.to_string().into(),
            report.state.as_str().into(),
            report.kind.as_str().into(),
            report.filer.session().unwrap_or_default().into(),
            report.filer.source_key().into(),
            report.priority.as_deref().unwrap_or_default().into(),
            report.deadline.map_or(Value::Null, Value::from),
            report
                .credibility
                .map_or(Value::Null, |credibility| i64::from(credibility).into()),
            report.created.into(),
            report.updated.into(),
            text.into(),
            revision.into(),
        ],
    )?;
    for link in &report.links {
        tx.execute(
            "INSERT INTO ai6_report_links (report_id, kind, target) VALUES ($1, $2, $3) \
             ON CONFLICT (report_id, kind, target) DO NOTHING",
            &[
                report.id.as_str().into(),
                link.kind.as_str().into(),
                link.id.as_str().into(),
            ],
        )?;
    }
    Ok(())
}

impl ReportStore for SqlRegistry {
    fn file_report(&self, report: &Report) -> io::Result<()> {
        let limits = &self.limits.reports;
        report.validate(limits)?;
        let text = String::from_utf8(report_bytes(report, limits)?).map_err(io::Error::other)?;
        let scope = report.scope.to_string();
        self.write(|tx, revision| {
            if tx
                .query_opt(
                    "SELECT id FROM ai6_reports WHERE id = $1",
                    &[report.id.as_str().into()],
                )?
                .is_some()
            {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "a report with this ID exists",
                ));
            }
            let recent = tx
                .query(
                    "SELECT session_id FROM ai6_reports \
                     WHERE scope = $1 AND created >= $2 AND session_id <> ''",
                    &[
                        scope.as_str().into(),
                        window_start(limits, report.created).into(),
                    ],
                )?
                .iter()
                .map(|row| row.text(0))
                .collect::<io::Result<Vec<_>>>()?;
            check_rate(limits, report, &recent)?;
            if let Some(before) = closed_before(limits, report.created) {
                let stale = [before.into(), ReportState::Closed.as_str().into()];
                tx.execute(
                    "DELETE FROM ai6_report_links WHERE report_id IN \
                     (SELECT id FROM ai6_reports WHERE updated < $1 AND state = $2)",
                    &stale,
                )?;
                tx.execute(
                    "DELETE FROM ai6_reports WHERE updated < $1 AND state = $2",
                    &stale,
                )?;
            }
            let count = tx
                .query_opt("SELECT count(*) FROM ai6_reports", &[])?
                .map_or(Ok(0), |row| row.int(0))?;
            if !limits.reports.allows(count + 1) {
                return Err(invalid_data("too many reports"));
            }
            put(tx, report, text, revision)
        })
    }

    fn report(&self, id: &str) -> io::Result<Option<Report>> {
        validate_registry_key(id)?;
        self.read(|tx| {
            tx.query_opt("SELECT record FROM ai6_reports WHERE id = $1", &[id.into()])?
                .map(|row| from_json(&row.text(0)?, "report"))
                .transpose()
        })
    }

    fn reports(&self, filter: &ReportFilter) -> io::Result<Vec<Report>> {
        let limits = &self.limits.reports;
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        let mut bind = |clause: &str, value: Value| {
            params.push(value);
            clauses.push(clause.replace('?', &format!("${}", params.len())));
        };
        if let Some(scope) = &filter.scope {
            bind("r.scope = ?", scope.to_string().into());
        }
        if let Some(state) = filter.state {
            bind("r.state = ?", state.as_str().into());
        }
        if let Some(kind) = &filter.kind {
            bind("r.kind = ?", kind.as_str().into());
        }
        if let Some(mission) = &filter.mission {
            bind(
                "r.id IN (SELECT report_id FROM ai6_report_links WHERE kind = 'mission' AND target = ?)",
                mission.as_str().into(),
            );
        }
        let filtered = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {} ", clauses.join(" AND "))
        };
        // The deployment's priorities, most urgent first; one not in the list, or none, last.
        let mut arms = String::new();
        for (rank, priority) in filter.priorities.iter().enumerate() {
            params.push(priority.as_str().into());
            arms.push_str(&format!("WHEN ${} THEN {rank} ", params.len()));
        }
        let priority = if arms.is_empty() {
            "0".to_owned()
        } else {
            format!("CASE r.priority {arms}ELSE {} END", filter.priorities.len())
        };
        params.push(
            i64::try_from(filter.page(limits))
                .unwrap_or(i64::MAX)
                .into(),
        );
        let sql = format!(
            "SELECT r.record FROM ai6_reports r \
             LEFT JOIN ai6_report_sources s ON s.source_key = r.source_key \
             {filtered}ORDER BY {priority}, COALESCE(r.deadline, 1e300), \
             COALESCE(r.credibility, 7), COALESCE(s.reliability, 'F'), r.created, r.id \
             LIMIT ${}",
            params.len()
        );
        let rows = self.read(|tx| tx.query(&sql, &params))?;
        let mut reports = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(report) = parse_report(row.text(0)?.as_bytes(), limits) {
                reports.push(report);
            }
        }
        Ok(reports)
    }

    fn change_report(&self, id: &str, change: ReportChange<'_>) -> io::Result<Report> {
        validate_registry_key(id)?;
        let limits = &self.limits.reports;
        self.write(|tx, revision| {
            let stored = tx
                .query_opt("SELECT record FROM ai6_reports WHERE id = $1", &[id.into()])?
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such report"))?;
            let stored = parse_report(stored.text(0)?.as_bytes(), limits)?;
            let changed = apply_change(limits, &stored, change)?;
            let text =
                String::from_utf8(report_bytes(&changed, limits)?).map_err(io::Error::other)?;
            put(tx, &changed, text, revision)?;
            Ok(changed)
        })
    }

    fn report_source(&self, key: &str) -> io::Result<ReportSource> {
        ReportSource::new(key).validate(&self.limits.reports)?;
        let found = self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_report_sources WHERE source_key = $1",
                &[key.into()],
            )?
            .map(|row| from_json::<ReportSource>(&row.text(0)?, "report source"))
            .transpose()
        })?;
        Ok(found
            .filter(|source| source.key == key)
            .unwrap_or_else(|| ReportSource::new(key)))
    }

    fn put_report_source(&self, source: &ReportSource) -> io::Result<()> {
        source.validate(&self.limits.reports)?;
        let text = to_json(source)?;
        self.write(|tx, revision| {
            tx.execute(
                "DELETE FROM ai6_report_sources WHERE source_key = $1",
                &[source.key.as_str().into()],
            )?;
            tx.execute(
                "INSERT INTO ai6_report_sources (source_key, reliability, record, revision) \
                 VALUES ($1, $2, $3, $4)",
                &[
                    source.key.as_str().into(),
                    source.reliability.as_str().into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn report_clearances(&self) -> io::Result<Vec<ReportClearance>> {
        let limits = &self.limits.reports;
        let rows = self.read(|tx| {
            tx.query(
                "SELECT record FROM ai6_report_clearances ORDER BY subject",
                &[],
            )
        })?;
        let mut clearances = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(clearance) = from_json::<ReportClearance>(&row.text(0)?, "report clearance")
                && clearance.validate(limits).is_ok()
            {
                clearances.push(clearance);
            }
        }
        Ok(clearances)
    }

    fn put_report_clearance(&self, clearance: &ReportClearance) -> io::Result<()> {
        clearance.validate(&self.limits.reports)?;
        let text = to_json(clearance)?;
        self.write(|tx, revision| {
            tx.execute(
                "DELETE FROM ai6_report_clearances WHERE subject = $1",
                &[clearance.subject.as_str().into()],
            )?;
            tx.execute(
                "INSERT INTO ai6_report_clearances (subject, record, revision) VALUES ($1, $2, $3)",
                &[
                    clearance.subject.as_str().into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn revoke_report_clearance(&self, subject: &str) -> io::Result<bool> {
        self.write(|tx, _| {
            let removed = tx.execute(
                "DELETE FROM ai6_report_clearances WHERE subject = $1",
                &[subject.into()],
            )?;
            Ok(removed > 0)
        })
    }
}
