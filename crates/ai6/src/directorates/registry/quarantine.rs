//! Quarantine records: Agents stopped everywhere until an operator releases them.
//!
//! A record names an Agent and the Operation it applies to (or every Operation). Every
//! Station reads the same records, so a quarantine raised on one Station stops the Agent on
//! the others. The incidents that justify a quarantine are kept per Agent for a few days, and
//! each new quarantine is counted on the Agent's record ([`super::agent_records`]).

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::agent_records::{AgentOutcome, AgentOutcomeReport};
use super::store::{FileRegistry, invalid_data, registry_lock, safe_component, write_atomic};
use crate::directorates::medical::quarantine::Incident;

const ALL_OPERATIONS: &str = "_all";
pub(super) const MAX_RECORD_BYTES: u64 = 16 * 1024;
pub(super) const MAX_QUARANTINES: usize = 10_000;
const MAX_INCIDENTS: usize = 256;
/// Incidents older than this are dropped; no policy window is longer in practice.
const KEEP_INCIDENTS_SECS: f64 = 7.0 * 24.0 * 3600.0;

/// An Agent stopped by Medical or an operator.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quarantine {
    /// The Agent codename.
    pub agent: String,
    /// The Operation it applies to; `None` is every Operation.
    pub operation: Option<String>,
    /// Why it was quarantined.
    pub reason: String,
    /// Who quarantined it: an operator, or `station:ID` for an automatic trigger.
    pub by: String,
    /// When, as Unix seconds.
    pub at: f64,
}

impl Quarantine {
    /// A quarantine of `agent` in `operation` (every Operation when `None`).
    pub fn new(
        agent: impl Into<String>,
        operation: Option<String>,
        reason: impl Into<String>,
        by: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            agent: agent.into(),
            operation,
            reason: reason.into(),
            by: by.into(),
            at,
        }
    }

    /// Whether this record stops `agent` working in `operation`.
    pub fn covers(&self, agent: &str, operation: Option<&str>) -> bool {
        self.agent == agent
            && match (&self.operation, operation) {
                (None, _) => true,
                (Some(own), Some(asked)) => own == asked,
                (Some(_), None) => false,
            }
    }
}

impl FileRegistry {
    fn quarantine_dir(&self) -> PathBuf {
        self.root.join("quarantine")
    }

    fn quarantine_path(&self, agent: &str, operation: Option<&str>) -> PathBuf {
        self.quarantine_dir().join(format!(
            "{}--{}.json",
            safe_component(agent),
            safe_component(operation.unwrap_or(ALL_OPERATIONS))
        ))
    }
}

/// Quarantined Agents and the incidents that lead to quarantine.
pub trait QuarantineStore {
    /// Quarantines an Agent. Quarantining again replaces the reason; the first record's
    /// time is kept.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty Agent or reason, or when the Registry cannot be written.
    fn quarantine_agent(&self, record: &Quarantine) -> io::Result<Quarantine>;

    /// Lifts a quarantine; `false` when there was none.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn release_quarantine(&self, agent: &str, operation: Option<&str>) -> io::Result<bool>;

    /// Lists every quarantine, Agent first.
    ///
    /// # Errors
    ///
    /// Returns an error when a record cannot be read.
    fn quarantines(&self) -> io::Result<Vec<Quarantine>>;

    /// Whether `agent` is quarantined for `operation`, by a record for that Operation or one
    /// for every Operation.
    ///
    /// # Errors
    ///
    /// Returns an error when the records cannot be read; callers that gate work on this treat
    /// that as quarantined.
    fn is_quarantined(&self, agent: &str, operation: Option<&str>) -> io::Result<bool>;

    /// Adds an incident to an Agent's history and returns the history, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the history cannot be read or written.
    fn record_incident(&self, agent: &str, incident: Incident) -> io::Result<Vec<Incident>>;
}

impl QuarantineStore for FileRegistry {
    fn quarantine_agent(&self, record: &Quarantine) -> io::Result<Quarantine> {
        validate_quarantine(record)?;
        let _lock = registry_lock(&self.root)?;
        let path = self.quarantine_path(&record.agent, record.operation.as_deref());
        if let Some(existing) = read_record(&path)? {
            return Ok(existing);
        }
        if fs::read_dir(self.quarantine_dir())
            .is_ok_and(|entries| entries.count() >= MAX_QUARANTINES)
        {
            return Err(io::Error::other("too many quarantine records"));
        }
        let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
        write_atomic(&path, &bytes)?;
        // A full or unreadable Agent record never keeps a quarantine from being stored.
        let _ = self.count_outcome_locked(&quarantine_report(record));
        Ok(record.clone())
    }

    fn release_quarantine(&self, agent: &str, operation: Option<&str>) -> io::Result<bool> {
        let _lock = registry_lock(&self.root)?;
        match fs::remove_file(self.quarantine_path(agent, operation)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn quarantines(&self) -> io::Result<Vec<Quarantine>> {
        let entries = match fs::read_dir(self.quarantine_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut records = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && let Some(record) = read_record(&path)?
            {
                records.push(record);
            }
        }
        records.sort_by(|left, right| {
            (&left.agent, &left.operation).cmp(&(&right.agent, &right.operation))
        });
        Ok(records)
    }

    fn is_quarantined(&self, agent: &str, operation: Option<&str>) -> io::Result<bool> {
        Ok(self
            .quarantines()?
            .iter()
            .any(|record| record.covers(agent, operation)))
    }

    fn record_incident(&self, agent: &str, incident: Incident) -> io::Result<Vec<Incident>> {
        let _lock = registry_lock(&self.root)?;
        let path = self
            .quarantine_dir()
            .join("incidents")
            .join(format!("{}.json", safe_component(agent)));
        let mut history: Vec<Incident> = match fs::read(&path) {
            Ok(bytes) if bytes.len() as u64 <= MAX_RECORD_BYTES * 16 => {
                serde_json::from_slice(&bytes)
                    .map_err(|error| invalid_data(format!("invalid incident history: {error}")))?
            }
            Ok(_) => return Err(invalid_data("incident history exceeds the size limit")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        add_incident(&mut history, incident);
        let bytes = serde_json::to_vec(&history).map_err(io::Error::other)?;
        write_atomic(&path, &bytes)?;
        Ok(history)
    }
}

/// Adds `incident` to an Agent's history, dropping incidents too old to count and the
/// oldest past the bound.
pub(super) fn add_incident(history: &mut Vec<Incident>, incident: Incident) {
    let cutoff = incident.at - KEEP_INCIDENTS_SECS;
    history.retain(|known| known.at >= cutoff);
    history.push(incident);
    if history.len() > MAX_INCIDENTS {
        history.drain(..history.len() - MAX_INCIDENTS);
    }
}

/// The outcome a new quarantine counts on its Agent's record: in its Operation, or under
/// [`NO_OPERATION`](super::agent_records::NO_OPERATION) when it applies to every Operation.
pub(super) fn quarantine_report(record: &Quarantine) -> AgentOutcomeReport {
    AgentOutcomeReport::new(
        record.agent.clone(),
        record.operation.clone(),
        AgentOutcome::Quarantined,
        format!(
            "quarantine#{}#{}",
            record.operation.as_deref().unwrap_or(ALL_OPERATIONS),
            record.at
        ),
        record.at,
    )
}

/// Checks a quarantine before it is stored.
pub(super) fn validate_quarantine(record: &Quarantine) -> io::Result<()> {
    if record.agent.trim().is_empty() || record.reason.trim().is_empty() {
        return Err(invalid_data("a quarantine needs an Agent and a reason"));
    }
    Ok(())
}

fn read_record(path: &std::path::Path) -> io::Result<Option<Quarantine>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid_data("quarantine record exceeds the size limit"));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| invalid_data(format!("invalid quarantine record: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::medical::quarantine::IncidentKind;
    use crate::directorates::registry::agent_records::NO_OPERATION;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_quarantine_covers_its_operation_or_all_and_release_lifts_it,
        incidents_accumulate_per_agent_and_old_ones_are_dropped,
    );

    fn a_quarantine_covers_its_operation_or_all_and_release_lifts_it(store: &dyn Store) {
        let registry = store.handle();
        assert!(!registry.is_quarantined("007", Some("G")).unwrap());
        registry
            .quarantine_agent(&Quarantine::new(
                "007",
                Some("G".into()),
                "looping",
                "ops",
                10.0,
            ))
            .unwrap();
        assert!(registry.is_quarantined("007", Some("G")).unwrap());
        assert!(!registry.is_quarantined("007", Some("L")).unwrap());
        assert!(!registry.is_quarantined("008", Some("G")).unwrap());
        registry
            .quarantine_agent(&Quarantine::new("009", None, "turned", "ops", 11.0))
            .unwrap();
        assert!(registry.is_quarantined("009", Some("anything")).unwrap());
        assert!(registry.is_quarantined("009", None).unwrap());

        let again = registry
            .quarantine_agent(&Quarantine::new(
                "007",
                Some("G".into()),
                "other",
                "x",
                99.0,
            ))
            .unwrap();
        assert_eq!((again.reason.as_str(), again.at), ("looping", 10.0));
        assert_eq!(registry.quarantines().unwrap().len(), 2);
        let counted = |agent: &str, operation: &str| {
            registry
                .agent_record(agent)
                .unwrap()
                .map_or(0, |record| record.in_operation(operation).quarantined)
        };
        assert_eq!(counted("007", "G"), 1, "quarantining again counts nothing");
        assert_eq!(counted("009", NO_OPERATION), 1);
        assert!(registry.release_quarantine("007", Some("G")).unwrap());
        assert!(!registry.release_quarantine("007", Some("G")).unwrap());
        assert!(!registry.is_quarantined("007", Some("G")).unwrap());
    }

    fn incidents_accumulate_per_agent_and_old_ones_are_dropped(store: &dyn Store) {
        let registry = store.handle();
        let incident = |at| Incident {
            kind: IncidentKind::Stall,
            at,
            operation: None,
            mission: "m".to_owned(),
        };
        registry.record_incident("007", incident(1_000.0)).unwrap();
        registry.record_incident("008", incident(1_001.0)).unwrap();
        let history = registry.record_incident("007", incident(2_000.0)).unwrap();
        assert_eq!(history.len(), 2);
        let history = registry
            .record_incident("007", incident(2_000.0 + KEEP_INCIDENTS_SECS + 1.0))
            .unwrap();
        assert_eq!(history.len(), 1, "incidents past the retention are dropped");
    }
}
