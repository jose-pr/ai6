//! The tools an Operation's watch decides with: what its Missions stand at, what a verifying one
//! was to deliver and what the probes found, and the decision itself.
//!
//! The watch is the Operation's 00 Agent in charge, so it verifies and the Station only provides.
//! A decision takes the path a person's does ([`decisions::decide`]): audited, recorded as the
//! attempt's verdict, on the timeline, and applied by the Mission's Station, which counts it on
//! the scorecard. A watch whose clearance has been lowered to `desk` recommends instead, and a
//! person confirms. A person's decision replaces a watch's, and a watch never replaces a person's.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::super::{DaemonState, agents, verification};
use crate::directorates::m::memories::fenced;
use crate::directorates::m::mission::MissionState;
use crate::directorates::registry::decisions::{self, VerifyingDecision};
use crate::directorates::registry::sessions::{OutcomeVerdict, SessionRecord};
use crate::directorates::registry::store::AuditRecord;

/// The prefix of the name a watch's decisions are recorded under.
const BY_PREFIX: &str = "watch:operation:";

/// An Operation's watch at work on its Missions.
#[derive(Clone)]
pub(in crate::station::server) struct DecisionDesk {
    state: DaemonState,
    operation: String,
    agent: String,
    /// The watch's Session name, which its proposals name as their parent.
    key: String,
}

impl DecisionDesk {
    pub(in crate::station::server) fn new(
        state: DaemonState,
        operation: String,
        agent: String,
        key: String,
    ) -> Self {
        Self {
            state,
            operation,
            agent,
            key,
        }
    }

    pub(in crate::station::server) fn key(&self) -> &str {
        &self.key
    }

    fn by(&self) -> String {
        format!("{BY_PREFIX}{}", self.operation)
    }

    /// The latest attempt at the Mission `name` names (a UID or a handle), when the Operation is
    /// the watch's.
    fn latest(&self, name: &str) -> Result<SessionRecord, String> {
        let intake = &self.state.intake;
        let latest = match intake.latest_session(name).ok().flatten() {
            Some(record) => Some(record),
            None => {
                let handle = intake
                    .resolve_mission(name)
                    .map_err(|error| error.to_string())?
                    .ok_or("no Mission goes by that name")?;
                intake
                    .latest_session(&handle.mission_id)
                    .map_err(|error| error.to_string())?
            }
        };
        latest
            .filter(|record| record.operation.as_deref() == Some(self.operation.as_str()))
            .ok_or_else(|| "that is not a Mission of your Operation".to_owned())
    }

    /// Your Operation's Missions that are not settled: the latest attempt of each, with its state.
    pub(in crate::station::server) fn missions(&self) -> Result<String, String> {
        let mut latest: BTreeMap<String, SessionRecord> = BTreeMap::new();
        for record in self
            .state
            .intake
            .sessions()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|record| record.operation.as_deref() == Some(self.operation.as_str()))
        {
            match latest.get(&record.mission_id) {
                Some(held) if held.attempt >= record.attempt => {}
                _ => {
                    latest.insert(record.mission_id.clone(), record);
                }
            }
        }
        let rows: Vec<Value> = latest
            .into_values()
            .filter(|record| !record.status.is_closed())
            .map(|record| {
                json!({
                    "mission": record.mission_id,
                    "handle": record.handle,
                    "attempt": record.attempt,
                    "state": record.result.as_ref().map(|result| result.state.as_str()),
                    "agent": record.launch.as_ref().and_then(|launch| launch.agent.as_deref()),
                    "decided": record.verdict.is_some(),
                    "recommended": record.recommendation.is_some(),
                })
            })
            .collect();
        Ok(fenced("operation_missions", &Value::Array(rows)))
    }

    /// One Mission's latest attempt: its brief, its final report, what it was to deliver, what
    /// the probes found and what has been decided.
    pub(in crate::station::server) fn detail(&self, name: &str) -> Result<String, String> {
        let record = self.latest(name)?;
        Ok(fenced(
            "mission_detail",
            &super::attempt_detail(&self.state, &record),
        ))
    }

    /// What the Mission was to deliver.
    pub(in crate::station::server) fn deliverables(&self, name: &str) -> Result<String, String> {
        let record = self.latest(name)?;
        Ok(fenced(
            "deliverables",
            &json!({
                "mission": record.mission_id,
                "attempt": record.attempt,
                "deliverables": record.deliverables,
            }),
        ))
    }

    /// Reads every deliverable of the verifying attempt back and keeps what was found as its
    /// evidence. A Mission another Station holds is asked of that Station, which answers on its
    /// next pass.
    pub(in crate::station::server) fn probe(&self, name: &str) -> Result<String, String> {
        let record = self.latest(name)?;
        decisions::check_verifying(&Some(record.clone()), &self.operation)
            .map_err(|error| error.to_string())?;
        let now = unix_seconds();
        let audit = AuditRecord {
            id: uuid::Uuid::new_v4().to_string(),
            actor: self.by(),
            action: "mission.probe".to_owned(),
            operation: self.operation.clone(),
            resource: Some(format!("mission:{}", record.mission_id)),
            target: record.mission_id.clone(),
            at: now,
        };
        self.state
            .intake
            .record_audit(&audit)
            .map_err(|error| error.to_string())?;
        let here = self.state.missions.lock().ok().and_then(|missions| {
            missions
                .get(&record.mission_id)
                .map(|mission| mission.state)
        }) == Some(MissionState::Verifying);
        if !here {
            self.state
                .intake
                .request_probe(&self.operation, &record.mission_id, now)
                .map_err(|error| error.to_string())?;
            return Ok(json!({
                "asked": true,
                "note": "the Station holding this Mission reads its deliverables on its next \
                         pass; mission_detail then shows what it found",
            })
            .to_string());
        }
        let found = verification::probe_and_record(&self.state, &record.mission_id)?;
        let evidence = self
            .latest(&record.mission_id)
            .map(|record| record.evidence)
            .unwrap_or_default();
        let fresh = evidence.len().saturating_sub(found);
        Ok(fenced(
            "deliverable_probe",
            &json!({ "mission": record.mission_id, "found": &evidence[fresh..] }),
        ))
    }

    /// Decides the verifying Mission `done` or `failed`, or redispatches it with `amendment`.
    /// Held at `desk` clearance, the watch records a recommendation a person confirms instead.
    pub(in crate::station::server) fn decide(
        &self,
        name: &str,
        word: &str,
        reason: &str,
        amendment: Option<&str>,
    ) -> Result<String, String> {
        let verdict = VerifyingDecision::verdict_named(word)
            .ok_or("a decision is done, failed or redispatch")?;
        if reason.trim().is_empty() {
            return Err("a decision needs its reason".to_owned());
        }
        if amendment.is_some() && verdict != OutcomeVerdict::Redispatch {
            return Err("only a redispatch takes an amendment".to_owned());
        }
        let record = self.latest(name)?;
        if let Some(by) = record
            .verdict
            .as_ref()
            .map(|given| given.by.as_str())
            .filter(|by| !by.starts_with(BY_PREFIX))
        {
            return Err(format!("{by} has already decided this Mission"));
        }
        let clearance = agents::effective(&self.state, Some(&self.operation), &self.agent)
            .map_err(|error| error.to_string())?
            .clearance;
        let by = self.by();
        let at = unix_seconds();
        let decision = VerifyingDecision::new(
            &self.operation,
            &record.mission_id,
            verdict,
            reason,
            &by,
            at,
        )
        .with_amendment(amendment);
        let store = self.state.intake.as_ref();
        if clearance == "desk" {
            decisions::recommend(store, &decision).map_err(|error| error.to_string())?;
            return Ok(json!({
                "recommended": word,
                "settled": false,
                "note": "your clearance is desk, so this is a recommendation a person confirms",
            })
            .to_string());
        }
        decisions::decide(store, &decision).map_err(|error| error.to_string())?;
        Ok(json!({
            "decided": word,
            "attempt": record.attempt,
            "note": "the Mission's Station settles it, or queues the next attempt, on its next pass",
        })
        .to_string())
    }
}

fn unix_seconds() -> f64 {
    super::super::unix_now()
}
