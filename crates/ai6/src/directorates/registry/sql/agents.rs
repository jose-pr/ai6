//! Agent records in SQL: one row per codename, holding the record as JSON.

use std::io;

use super::SqlRegistry;
use super::db::Tx;
use crate::directorates::registry::agent_records::{
    AgentOutcomeReport, AgentRecord, AgentRecordStore, parse_record, record_bytes,
};

/// Counts `report` into its Agent's record within a write transaction at `revision`.
pub(super) fn count_in(
    tx: &mut Tx<'_>,
    revision: i64,
    report: &AgentOutcomeReport,
) -> io::Result<AgentRecord> {
    let mut record = match tx.query_opt(
        "SELECT record FROM ai6_agent_records WHERE agent = $1",
        &[report.agent.as_str().into()],
    )? {
        Some(row) => parse_record(row.text(0)?.as_bytes())?,
        None => AgentRecord::new(report.agent.clone()),
    };
    if record.count(report)? {
        let text = String::from_utf8(record_bytes(&record)?).map_err(io::Error::other)?;
        tx.execute(
            "INSERT INTO ai6_agent_records (agent, record, revision) VALUES ($1, $2, $3) \
             ON CONFLICT (agent) DO UPDATE SET record = excluded.record, \
             revision = excluded.revision",
            &[report.agent.as_str().into(), text.into(), revision.into()],
        )?;
    }
    Ok(record)
}

impl AgentRecordStore for SqlRegistry {
    fn record_agent_outcome(&self, report: &AgentOutcomeReport) -> io::Result<AgentRecord> {
        self.write(|tx, revision| count_in(tx, revision, report))
    }

    fn agent_record(&self, agent: &str) -> io::Result<Option<AgentRecord>> {
        self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_agent_records WHERE agent = $1",
                &[agent.into()],
            )?
            .map(|row| parse_record(row.text(0)?.as_bytes()))
            .transpose()
        })
    }

    fn agent_records(&self) -> io::Result<Vec<AgentRecord>> {
        let rows = self.read(|tx| tx.query("SELECT record FROM ai6_agent_records", &[]))?;
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(record) = parse_record(row.text(0)?.as_bytes()) {
                records.push(record);
            }
        }
        records.sort_by(|left, right| left.agent.cmp(&right.agent));
        Ok(records)
    }
}
