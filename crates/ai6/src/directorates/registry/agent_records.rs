//! Agent records: each Agent's track record in each Operation.
//!
//! A Station adds an outcome when one of its Missions finishes, stalls or loses its Session,
//! and when an incident is charged to the Agent: a quarantine, a budget stop or a refused green
//! ink request. Per Operation a record holds counts, the time of each kind's latest outcome,
//! counts by the clearance the Agent worked at, and the models that occupied the codename with
//! what each did; the Missions, Sessions and audit records stay the detail. Promotion and the
//! operator choosing a backup read them.
//!
//! Each report carries a key naming the event (a Mission attempt and its outcome, a
//! quarantine), and a key already counted is not counted again, so every Station may report
//! what it saw.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::store::{FileRegistry, invalid_data, registry_lock, safe_component, write_atomic};

/// The Operation key of outcomes outside any Operation, and of quarantines in every Operation.
pub const NO_OPERATION: &str = "";
pub(super) const MAX_RECORD_BYTES: u64 = 1024 * 1024;
const MAX_OPERATIONS: usize = 128;
const MAX_CLEARANCES: usize = 8;
const MAX_COUNTED: usize = 256;
const MAX_MODEL_HISTORY: usize = 16;
const MAX_KEY_BYTES: usize = 512;
const MAX_AGENT_BYTES: usize = 256;

/// What happened that counts toward an Agent's record.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOutcome {
    /// A Mission finished its work.
    Done,
    /// A Mission ended unsuccessfully.
    Failed,
    /// A Mission's commits broke its Operation's cover.
    Blown,
    /// A Mission was withdrawn.
    Abandoned,
    /// The Watchdog stopped a Session that made no progress or looped.
    Stalled,
    /// A Session was lost with its Station or runner: the infrastructure failed, not the Agent.
    Lost,
    /// Medical or an operator quarantined the Agent.
    Quarantined,
    /// A budget limit stopped one of its Sessions.
    BudgetStop,
    /// A person refused a step it asked green ink for.
    GreenInkRejected,
}

impl AgentOutcome {
    /// Every outcome, in declaration order.
    pub const ALL: [Self; 9] = [
        Self::Done,
        Self::Failed,
        Self::Blown,
        Self::Abandoned,
        Self::Stalled,
        Self::Lost,
        Self::Quarantined,
        Self::BudgetStop,
        Self::GreenInkRejected,
    ];

    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Blown => "blown",
            Self::Abandoned => "abandoned",
            Self::Stalled => "stalled",
            Self::Lost => "lost",
            Self::Quarantined => "quarantined",
            Self::BudgetStop => "budget_stop",
            Self::GreenInkRejected => "green_ink_rejected",
        }
    }

    /// The outcome its wire spelling names.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == name)
    }

    /// Whether it is an incident: a blown cover, a stall, a quarantine, a budget stop or a
    /// refused green ink request. A failure, an abandoned Mission or a lost Session is not.
    pub fn is_incident(self) -> bool {
        !matches!(
            self,
            Self::Done | Self::Failed | Self::Abandoned | Self::Lost
        )
    }

    /// Whether it is the outcome of a Session the Agent ran, so it names the model it ran.
    pub fn is_session_outcome(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Blown | Self::Abandoned | Self::Stalled | Self::Lost
        )
    }
}

/// Counts of an Agent's outcomes.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentTally {
    /// Missions done.
    pub done: u64,
    /// Missions failed.
    pub failed: u64,
    /// Missions whose cover was blown.
    pub blown: u64,
    /// Missions abandoned.
    pub abandoned: u64,
    /// Sessions stopped as stalled or looping.
    pub stalled: u64,
    /// Sessions lost with their Station or runner.
    pub lost: u64,
    /// Quarantines.
    pub quarantined: u64,
    /// Budget stops.
    pub budget_stops: u64,
    /// Refused green ink requests.
    pub green_ink_rejected: u64,
    /// When each kind of outcome last happened, as Unix seconds.
    pub last: BTreeMap<AgentOutcome, f64>,
}

impl AgentTally {
    /// How many times `outcome` was counted.
    pub fn count(&self, outcome: AgentOutcome) -> u64 {
        match outcome {
            AgentOutcome::Done => self.done,
            AgentOutcome::Failed => self.failed,
            AgentOutcome::Blown => self.blown,
            AgentOutcome::Abandoned => self.abandoned,
            AgentOutcome::Stalled => self.stalled,
            AgentOutcome::Lost => self.lost,
            AgentOutcome::Quarantined => self.quarantined,
            AgentOutcome::BudgetStop => self.budget_stops,
            AgentOutcome::GreenInkRejected => self.green_ink_rejected,
        }
    }

    /// When `outcome` last happened, as Unix seconds.
    pub fn last(&self, outcome: AgentOutcome) -> Option<f64> {
        self.last.get(&outcome).copied()
    }

    /// The latest time any of `outcomes` happened.
    pub fn latest(&self, outcomes: &[AgentOutcome]) -> Option<f64> {
        outcomes
            .iter()
            .filter_map(|outcome| self.last(*outcome))
            .reduce(f64::max)
    }

    /// Failed and blown Missions as a share of the Missions that ended done, failed or blown;
    /// `None` before any did. Lost Sessions do not count.
    pub fn failure_rate(&self) -> Option<f64> {
        let failures = self.failed + self.blown;
        let ended = self.done + failures;
        (ended > 0).then(|| failures as f64 / ended as f64)
    }

    fn add(&mut self, outcome: AgentOutcome, at: f64) {
        let count = match outcome {
            AgentOutcome::Done => &mut self.done,
            AgentOutcome::Failed => &mut self.failed,
            AgentOutcome::Blown => &mut self.blown,
            AgentOutcome::Abandoned => &mut self.abandoned,
            AgentOutcome::Stalled => &mut self.stalled,
            AgentOutcome::Lost => &mut self.lost,
            AgentOutcome::Quarantined => &mut self.quarantined,
            AgentOutcome::BudgetStop => &mut self.budget_stops,
            AgentOutcome::GreenInkRejected => &mut self.green_ink_rejected,
        };
        *count = count.saturating_add(1);
        let last = self.last.entry(outcome).or_insert(at);
        *last = last.max(at);
    }
}

/// One model an Agent's codename was occupied by in an Operation, and what it did there.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelTenure {
    /// The model its Sessions were launched with; `None` is the handler's default.
    pub model: Option<String>,
    /// The first outcome counted with it, as Unix seconds.
    pub from: f64,
    /// The first outcome counted with the next model; `None` while it is current.
    pub to: Option<f64>,
    /// The outcomes of its Sessions.
    #[serde(default)]
    pub tally: AgentTally,
}

/// An Agent's record in one Operation.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperationRecord {
    /// Every outcome there.
    pub tally: AgentTally,
    /// Outcomes there by the clearance the Agent worked at.
    pub by_clearance: BTreeMap<String, AgentTally>,
    /// When its latest Mission there ended done, failed, blown or abandoned, as Unix seconds.
    pub last_mission_at: Option<f64>,
    /// The models that occupied the codename there, oldest first; a new one starts with the
    /// first Session outcome counted with another model.
    pub model_history: Vec<ModelTenure>,
}

impl OperationRecord {
    /// What the Sessions of `model` did here while it is the current occupant; an empty tally
    /// when another model occupies the codename now.
    pub fn tally_of_model(&self, model: Option<&str>) -> AgentTally {
        self.model_history
            .last()
            .filter(|tenure| tenure.model.as_deref() == model)
            .map(|tenure| tenure.tally.clone())
            .unwrap_or_default()
    }

    fn note_model(&mut self, model: Option<String>, at: f64) -> &mut ModelTenure {
        let current = self
            .model_history
            .last()
            .is_some_and(|tenure| tenure.model == model);
        if !current {
            if let Some(previous) = self.model_history.last_mut() {
                previous.to = Some(at);
            }
            self.model_history.push(ModelTenure {
                model,
                from: at,
                to: None,
                tally: AgentTally::default(),
            });
            if self.model_history.len() > MAX_MODEL_HISTORY {
                self.model_history
                    .drain(..self.model_history.len() - MAX_MODEL_HISTORY);
            }
        }
        self.model_history
            .last_mut()
            .expect("a tenure was just ensured")
    }
}

/// An Agent's track record.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentRecord {
    /// The Agent codename.
    pub agent: String,
    /// Its record per Operation; [`NO_OPERATION`] holds outcomes outside any Operation and
    /// quarantines that apply to every Operation.
    pub by_operation: BTreeMap<String, OperationRecord>,
    /// The keys of the latest reports counted, newest last, so a repeated report is not
    /// counted twice.
    pub counted: Vec<String>,
}

impl AgentRecord {
    /// An empty record for `agent`.
    pub fn new(agent: impl Into<String>) -> Self {
        Self {
            agent: agent.into(),
            ..Self::default()
        }
    }

    /// The Agent's record in `operation`, or an empty one.
    pub fn in_operation(&self, operation: &str) -> OperationRecord {
        self.by_operation
            .get(operation)
            .cloned()
            .unwrap_or_default()
    }

    /// Counts `report` into the record; `false` when its key was already counted.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a report out of bounds or for another Agent; `InvalidData` when the
    /// record would hold more than 128 Operations or 8 clearances in one.
    pub fn count(&mut self, report: &AgentOutcomeReport) -> io::Result<bool> {
        report.validate()?;
        if report.agent != self.agent {
            return Err(invalid_input("the report is for another Agent"));
        }
        if self.counted.contains(&report.key) {
            return Ok(false);
        }
        let operation = report.operation.as_deref().unwrap_or(NO_OPERATION);
        let new_clearance = |record: &OperationRecord| {
            report.clearance.as_deref().is_some_and(|clearance| {
                !record.by_clearance.contains_key(clearance)
                    && record.by_clearance.len() >= MAX_CLEARANCES
            })
        };
        match self.by_operation.get(operation) {
            None if self.by_operation.len() >= MAX_OPERATIONS => {
                return Err(invalid_data("the Agent record holds too many Operations"));
            }
            Some(record) if new_clearance(record) => {
                return Err(invalid_data("the Agent record holds too many clearances"));
            }
            _ => {}
        }
        let record = self.by_operation.entry(operation.to_owned()).or_default();
        record.tally.add(report.outcome, report.at);
        if let Some(clearance) = &report.clearance {
            record
                .by_clearance
                .entry(clearance.clone())
                .or_default()
                .add(report.outcome, report.at);
        }
        if matches!(
            report.outcome,
            AgentOutcome::Done
                | AgentOutcome::Failed
                | AgentOutcome::Blown
                | AgentOutcome::Abandoned
        ) {
            record.last_mission_at = Some(
                record
                    .last_mission_at
                    .map_or(report.at, |at| at.max(report.at)),
            );
        }
        if report.outcome.is_session_outcome() {
            record
                .note_model(report.model.clone(), report.at)
                .tally
                .add(report.outcome, report.at);
        }
        self.counted.push(report.key.clone());
        if self.counted.len() > MAX_COUNTED {
            self.counted.drain(..self.counted.len() - MAX_COUNTED);
        }
        Ok(true)
    }
}

/// One outcome to count, as handed to [`AgentRecordStore::record_agent_outcome`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct AgentOutcomeReport {
    /// The Agent codename.
    pub agent: String,
    /// The Operation it happened in; `None` is outside any Operation, or every Operation.
    pub operation: Option<String>,
    /// What happened.
    pub outcome: AgentOutcome,
    /// Names the event, such as `MISSION#ATTEMPT#OUTCOME`; a key counted before is not counted
    /// again.
    pub key: String,
    /// The clearance the Agent worked at, when known.
    pub clearance: Option<String>,
    /// The model the Session was launched with; `None` is the handler's default.
    pub model: Option<String>,
    /// When, as Unix seconds.
    pub at: f64,
}

impl AgentOutcomeReport {
    /// A report of `outcome` for `agent` in `operation`, named by `key`, at `at`.
    pub fn new(
        agent: impl Into<String>,
        operation: Option<String>,
        outcome: AgentOutcome,
        key: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            agent: agent.into(),
            operation,
            outcome,
            key: key.into(),
            clearance: None,
            model: None,
            at,
        }
    }

    /// Sets the clearance the Agent worked at.
    #[must_use]
    pub fn at_clearance(mut self, clearance: impl Into<String>) -> Self {
        self.clearance = Some(clearance.into());
        self
    }

    /// Sets the model the Agent was configured with.
    #[must_use]
    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    fn validate(&self) -> io::Result<()> {
        let bounded = |text: &str, limit: usize| {
            !text.trim().is_empty() && text.len() <= limit && !text.contains(char::is_control)
        };
        if !bounded(&self.agent, MAX_AGENT_BYTES)
            || !bounded(&self.key, MAX_KEY_BYTES)
            || self
                .operation
                .as_deref()
                .is_some_and(|operation| !bounded(operation, MAX_AGENT_BYTES))
            || self
                .clearance
                .as_deref()
                .is_some_and(|clearance| !bounded(clearance, 16))
            || self
                .model
                .as_deref()
                .is_some_and(|model| !bounded(model, MAX_AGENT_BYTES))
            || !self.at.is_finite()
            || self.at < 0.0
        {
            return Err(invalid_input(
                "an Agent outcome needs an Agent, a key and a time, each in bounds",
            ));
        }
        Ok(())
    }
}

/// Agent records: what each Agent has done, per Operation and clearance.
pub trait AgentRecordStore {
    /// Counts one outcome into its Agent's record and returns the record. A report whose key
    /// was counted before leaves the record as it is.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a report out of bounds; `InvalidData` for a full or unreadable
    /// record; I/O errors otherwise.
    fn record_agent_outcome(&self, report: &AgentOutcomeReport) -> io::Result<AgentRecord>;

    /// One Agent's record, if it has one.
    ///
    /// # Errors
    ///
    /// `InvalidData` for an unreadable record; I/O errors otherwise.
    fn agent_record(&self, agent: &str) -> io::Result<Option<AgentRecord>>;

    /// Every Agent's record, by codename. Unreadable records are skipped.
    ///
    /// # Errors
    ///
    /// I/O errors listing the records.
    fn agent_records(&self) -> io::Result<Vec<AgentRecord>>;
}

impl FileRegistry {
    fn agent_records_dir(&self) -> PathBuf {
        self.root.join("agent-records")
    }

    fn agent_record_path(&self, agent: &str) -> PathBuf {
        self.agent_records_dir()
            .join(format!("{}.json", safe_component(agent)))
    }
}

impl FileRegistry {
    /// Counts `report` into its Agent's record; the caller holds the Registry lock.
    pub(super) fn count_outcome_locked(
        &self,
        report: &AgentOutcomeReport,
    ) -> io::Result<AgentRecord> {
        report.validate()?;
        let path = self.agent_record_path(&report.agent);
        let mut record = read_record(&path, &report.agent)?
            .unwrap_or_else(|| AgentRecord::new(report.agent.clone()));
        if record.count(report)? {
            write_atomic(&path, &record_bytes(&record)?)?;
        }
        Ok(record)
    }
}

impl AgentRecordStore for FileRegistry {
    fn record_agent_outcome(&self, report: &AgentOutcomeReport) -> io::Result<AgentRecord> {
        report.validate()?;
        let _lock = registry_lock(&self.root)?;
        self.count_outcome_locked(report)
    }

    fn agent_record(&self, agent: &str) -> io::Result<Option<AgentRecord>> {
        read_record(&self.agent_record_path(agent), agent)
    }

    fn agent_records(&self) -> io::Result<Vec<AgentRecord>> {
        let entries = match fs::read_dir(self.agent_records_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut records = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            if let Ok(bytes) = fs::read(&path)
                && let Ok(record) = parse_record(&bytes)
                && path.file_stem().and_then(|stem| stem.to_str())
                    == Some(safe_component(&record.agent).as_str())
            {
                records.push(record);
            }
        }
        records.sort_by(|left, right| left.agent.cmp(&right.agent));
        Ok(records)
    }
}

/// The stored form of a record, refused past the size limit.
pub(super) fn record_bytes(record: &AgentRecord) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid_data("the Agent record exceeds the size limit"));
    }
    Ok(bytes)
}

/// A stored record, checked against the size limit.
pub(super) fn parse_record(bytes: &[u8]) -> io::Result<AgentRecord> {
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid_data("the Agent record exceeds the size limit"));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| invalid_data(format!("invalid Agent record: {error}")))
}

fn read_record(path: &std::path::Path, agent: &str) -> io::Result<Option<AgentRecord>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let record = parse_record(&bytes)?;
    // Codenames that differ only in characters a file name cannot hold share a file.
    if record.agent != agent {
        return Ok(None);
    }
    Ok(Some(record))
}

fn invalid_input(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        outcomes_count_once_per_key_by_operation_and_clearance,
        a_change_of_model_opens_a_new_tenure,
    );

    fn report(outcome: AgentOutcome, key: &str, at: f64) -> AgentOutcomeReport {
        AgentOutcomeReport::new("007", Some("G".into()), outcome, key, at)
            .at_clearance("field")
            .with_model(Some("opus".into()))
    }

    fn outcomes_count_once_per_key_by_operation_and_clearance(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(registry.agent_record("007").unwrap(), None);
        registry
            .record_agent_outcome(&report(AgentOutcome::Done, "m-1#1#done", 10.0))
            .unwrap();
        registry
            .record_agent_outcome(&report(AgentOutcome::Done, "m-2#1#done", 20.0))
            .unwrap();
        registry
            .record_agent_outcome(&report(AgentOutcome::Stalled, "m-3#1#stalled", 15.0))
            .unwrap();
        registry
            .record_agent_outcome(&report(AgentOutcome::Lost, "m-3#1#lost", 16.0))
            .unwrap();
        let again = registry
            .record_agent_outcome(&report(AgentOutcome::Done, "m-2#1#done", 30.0))
            .unwrap();
        let record = again.in_operation("G");
        let tally = &record.tally;
        assert_eq!(
            (tally.done, tally.stalled, tally.lost, tally.failed),
            (2, 1, 1, 0)
        );
        assert_eq!(tally.last(AgentOutcome::Done), Some(20.0));
        assert_eq!(record.by_clearance["field"].done, 2);
        assert_eq!(record.last_mission_at, Some(20.0));
        assert_eq!(
            tally.failure_rate(),
            Some(0.0),
            "a lost Session is no failure"
        );

        // Another Operation's work stays in its own record.
        registry
            .record_agent_outcome(
                &AgentOutcomeReport::new(
                    "007",
                    Some("L".into()),
                    AgentOutcome::Failed,
                    "l-1#1#failed",
                    60.0,
                )
                .at_clearance("00")
                .with_model(Some("haiku".into())),
            )
            .unwrap();
        registry
            .record_agent_outcome(&AgentOutcomeReport::new(
                "007",
                None,
                AgentOutcome::Quarantined,
                "quarantine#_all#40",
                40.0,
            ))
            .unwrap();
        registry
            .record_agent_outcome(&AgentOutcomeReport::new(
                "008",
                Some("G".into()),
                AgentOutcome::Failed,
                "m-4#1#failed",
                50.0,
            ))
            .unwrap();
        let record = registry.agent_record("007").unwrap().unwrap();
        let g = record.in_operation("G");
        assert_eq!(record.in_operation(NO_OPERATION).tally.quarantined, 1);
        assert_eq!(g.tally.quarantined, 0);
        assert!(!g.by_clearance.contains_key("00"));
        assert_eq!(g.last_mission_at, Some(20.0));
        assert_eq!(g.model_history.len(), 1);
        assert_eq!(record.in_operation("L").tally.failed, 1);
        let agents = registry
            .agent_records()
            .unwrap()
            .into_iter()
            .map(|record| record.agent)
            .collect::<Vec<_>>();
        assert_eq!(agents, ["007", "008"]);

        let bad = AgentOutcomeReport::new("007", None, AgentOutcome::Done, " ", 1.0);
        assert!(registry.record_agent_outcome(&bad).is_err());
    }

    fn a_change_of_model_opens_a_new_tenure(store: &dyn Store) {
        let registry = store.handle();
        registry
            .record_agent_outcome(&report(AgentOutcome::Done, "m-1#1#done", 10.0))
            .unwrap();
        registry
            .record_agent_outcome(&report(AgentOutcome::Failed, "m-2#1#failed", 20.0))
            .unwrap();
        let record = registry
            .record_agent_outcome(
                &report(AgentOutcome::Done, "m-3#1#done", 30.0).with_model(Some("sonnet".into())),
            )
            .unwrap();
        let g = record.in_operation("G");
        let tenures: Vec<_> = g
            .model_history
            .iter()
            .map(|tenure| {
                (
                    tenure.model.as_deref(),
                    tenure.from,
                    tenure.to,
                    tenure.tally.done,
                )
            })
            .collect();
        assert_eq!(
            tenures,
            [
                (Some("opus"), 10.0, Some(30.0), 1),
                (Some("sonnet"), 30.0, None, 1),
            ]
        );
        assert_eq!(g.tally_of_model(Some("sonnet")).done, 1);
        assert_eq!(
            g.tally_of_model(Some("opus")).done,
            0,
            "opus is not the occupant now"
        );
        // An incident charged to the Agent does not say which model it ran.
        let record = registry
            .record_agent_outcome(
                &AgentOutcomeReport::new(
                    "007",
                    Some("G".into()),
                    AgentOutcome::BudgetStop,
                    "budget#m-3#1",
                    31.0,
                )
                .with_model(Some("haiku".into())),
            )
            .unwrap();
        let g = record.in_operation("G");
        assert_eq!(g.model_history.len(), 2);
        assert_eq!(g.tally.budget_stops, 1);
    }

    #[test]
    fn outcomes_round_trip_by_name_and_incidents_are_named() {
        for outcome in AgentOutcome::ALL {
            assert_eq!(AgentOutcome::from_name(outcome.as_str()), Some(outcome));
            let json = serde_json::to_string(&outcome).unwrap();
            assert_eq!(json, format!("\"{}\"", outcome.as_str()));
        }
        assert!(!AgentOutcome::Failed.is_incident());
        assert!(!AgentOutcome::Lost.is_incident());
        assert!(AgentOutcome::GreenInkRejected.is_incident());
    }
}
