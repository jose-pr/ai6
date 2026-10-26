//! Agent records and clearance grants in SQL: one row per codename, and one per Agent and
//! Operation, each holding its record as JSON.

use std::io;

use super::SqlRegistry;
use super::db::Tx;
use crate::directorates::registry::agent_records::{
    AgentOutcomeReport, AgentRecord, AgentRecordStore, parse_record, record_bytes,
};
use crate::directorates::registry::clearance::{
    ClearanceGrant, ClearanceStore, MAX_GRANTS, grant_bytes, parse_grant,
};
use crate::directorates::registry::store::{invalid_data, validate_registry_key};

/// The Agent's grant in `operation`, read within a transaction.
fn grant_in(tx: &mut Tx<'_>, operation: &str, agent: &str) -> io::Result<Option<ClearanceGrant>> {
    tx.query_opt(
        "SELECT record FROM ai6_clearance_grants WHERE operation = $1 AND agent = $2",
        &[operation.into(), agent.into()],
    )?
    .map(|row| parse_grant(row.text(0)?.as_bytes()))
    .transpose()
}

/// Removes `agent`'s raises in `operation` (every Operation when `None`) within a write
/// transaction; lowers stay. Returns how many were removed.
pub(super) fn remove_raises_in(
    tx: &mut Tx<'_>,
    agent: &str,
    operation: Option<&str>,
) -> io::Result<usize> {
    let rows = match operation {
        Some(operation) => tx.query(
            "SELECT operation, record FROM ai6_clearance_grants WHERE agent = $1 AND operation = $2",
            &[agent.into(), operation.into()],
        )?,
        None => tx.query(
            "SELECT operation, record FROM ai6_clearance_grants WHERE agent = $1",
            &[agent.into()],
        )?,
    };
    let mut removed = 0;
    for row in rows {
        let operation = row.text(0)?;
        // An unreadable grant is removed too: it could be a raise.
        if parse_grant(row.text(1)?.as_bytes()).is_ok_and(|grant| !grant.raises()) {
            continue;
        }
        removed += tx.execute(
            "DELETE FROM ai6_clearance_grants WHERE operation = $1 AND agent = $2",
            &[operation.as_str().into(), agent.into()],
        )?;
    }
    Ok(removed as usize)
}

impl ClearanceStore for SqlRegistry {
    fn put_clearance_grant(&self, grant: &ClearanceGrant) -> io::Result<()> {
        grant.validate()?;
        let text = String::from_utf8(grant_bytes(grant)?).map_err(io::Error::other)?;
        self.write(|tx, revision| {
            let exists = tx
                .query_opt(
                    "SELECT agent FROM ai6_clearance_grants WHERE operation = $1 AND agent = $2",
                    &[grant.operation.as_str().into(), grant.agent.as_str().into()],
                )?
                .is_some();
            if !exists {
                let count = tx
                    .query_opt("SELECT count(*) FROM ai6_clearance_grants", &[])?
                    .map_or(Ok(0), |row| row.int(0))?;
                if count >= MAX_GRANTS as i64 {
                    return Err(invalid_data("too many clearance grants"));
                }
            }
            tx.execute(
                "INSERT INTO ai6_clearance_grants (operation, agent, record, revision) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (operation, agent) \
                 DO UPDATE SET record = excluded.record, revision = excluded.revision",
                &[
                    grant.operation.as_str().into(),
                    grant.agent.as_str().into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            Ok(())
        })
    }

    fn replace_clearance_grant(
        &self,
        expected: Option<&ClearanceGrant>,
        grant: &ClearanceGrant,
    ) -> io::Result<bool> {
        grant.validate()?;
        let text = String::from_utf8(grant_bytes(grant)?).map_err(io::Error::other)?;
        self.write(|tx, revision| {
            let current = grant_in(tx, &grant.operation, &grant.agent)?;
            if current.as_ref() != expected {
                return Ok(false);
            }
            if current.is_none() {
                let count = tx
                    .query_opt("SELECT count(*) FROM ai6_clearance_grants", &[])?
                    .map_or(Ok(0), |row| row.int(0))?;
                if count >= MAX_GRANTS as i64 {
                    return Err(invalid_data("too many clearance grants"));
                }
            }
            tx.execute(
                "INSERT INTO ai6_clearance_grants (operation, agent, record, revision) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT (operation, agent) \
                 DO UPDATE SET record = excluded.record, revision = excluded.revision",
                &[
                    grant.operation.as_str().into(),
                    grant.agent.as_str().into(),
                    text.into(),
                    revision.into(),
                ],
            )?;
            Ok(true)
        })
    }

    fn clearance_grant(&self, operation: &str, agent: &str) -> io::Result<Option<ClearanceGrant>> {
        validate_registry_key(operation)?;
        validate_registry_key(agent)?;
        self.read(|tx| {
            tx.query_opt(
                "SELECT record FROM ai6_clearance_grants WHERE operation = $1 AND agent = $2",
                &[operation.into(), agent.into()],
            )?
            .map(|row| parse_grant(row.text(0)?.as_bytes()))
            .transpose()
        })
    }

    fn clearance_grants(&self, operation: &str) -> io::Result<Vec<ClearanceGrant>> {
        validate_registry_key(operation)?;
        let rows = self.read(|tx| {
            tx.query(
                "SELECT record FROM ai6_clearance_grants WHERE operation = $1",
                &[operation.into()],
            )
        })?;
        let mut grants = Vec::with_capacity(rows.len());
        for row in rows {
            if let Ok(grant) = parse_grant(row.text(0)?.as_bytes()) {
                grants.push(grant);
            }
        }
        grants.sort_by(|left, right| left.agent.cmp(&right.agent));
        Ok(grants)
    }

    fn remove_clearance_grant_if(&self, expected: &ClearanceGrant) -> io::Result<bool> {
        validate_registry_key(&expected.operation)?;
        validate_registry_key(&expected.agent)?;
        self.write(|tx, _| {
            if grant_in(tx, &expected.operation, &expected.agent)?.as_ref() != Some(expected) {
                return Ok(false);
            }
            tx.execute(
                "DELETE FROM ai6_clearance_grants WHERE operation = $1 AND agent = $2",
                &[
                    expected.operation.as_str().into(),
                    expected.agent.as_str().into(),
                ],
            )?;
            Ok(true)
        })
    }

    fn remove_clearance_grant(
        &self,
        operation: &str,
        agent: &str,
    ) -> io::Result<Option<ClearanceGrant>> {
        validate_registry_key(operation)?;
        validate_registry_key(agent)?;
        self.write(|tx, _| {
            let Some(row) = tx.query_opt(
                "SELECT record FROM ai6_clearance_grants WHERE operation = $1 AND agent = $2",
                &[operation.into(), agent.into()],
            )?
            else {
                return Ok(None);
            };
            let grant = parse_grant(row.text(0)?.as_bytes())?;
            tx.execute(
                "DELETE FROM ai6_clearance_grants WHERE operation = $1 AND agent = $2",
                &[operation.into(), agent.into()],
            )?;
            Ok(Some(grant))
        })
    }
}

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
