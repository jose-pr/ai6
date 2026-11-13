//! Deciding a Mission whose latest run is verifying: the one path a person (through the
//! gateway) and an Operation's charge both take.
//!
//! A decision is audited first, then recorded as the run's verdict, then put on the
//! Operation's timeline. A charge held at `desk` clearance does not decide: it recommends, which
//! is audited and timelined the same way and settles nothing.

use super::backend::RegistryBackend;
use super::runs::{MissionVerdict, OutcomeVerdict, RunError, RunRecord};
use super::store::AuditRecord;
use super::timeline::TimelineEvent;

/// What `by` found of the Mission's verifying run.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct VerifyingDecision<'a> {
    /// The Operation the Mission belongs to.
    pub operation: &'a str,
    /// The Mission's UID.
    pub mission: &'a str,
    /// `Confirmed` (done), `FailedVerification` (failed) or `Redispatch`.
    pub verdict: OutcomeVerdict,
    /// Why; the next run of a redispatch is told it.
    pub reason: &'a str,
    /// For a redispatch, what the next run is told beyond the brief and the reason.
    pub amendment: Option<&'a str>,
    /// Who decided: a person ID, or `charge:operation:<codename>`.
    pub by: &'a str,
    /// When, as Unix seconds.
    pub at: f64,
}

impl<'a> VerifyingDecision<'a> {
    /// A decision of `by`, who found `verdict` for `reason`.
    pub fn new(
        operation: &'a str,
        mission: &'a str,
        verdict: OutcomeVerdict,
        reason: &'a str,
        by: &'a str,
        at: f64,
    ) -> Self {
        Self {
            operation,
            mission,
            verdict,
            reason,
            amendment: None,
            by,
            at,
        }
    }

    /// This decision with what a redispatch tells the next run.
    #[must_use]
    pub fn with_amendment(mut self, amendment: Option<&'a str>) -> Self {
        self.amendment = amendment;
        self
    }

    /// The verdict a decision word names: `done`, `failed` or `redispatch`.
    pub fn verdict_named(word: &str) -> Option<OutcomeVerdict> {
        match word {
            "done" => Some(OutcomeVerdict::Confirmed),
            "failed" => Some(OutcomeVerdict::FailedVerification),
            "redispatch" => Some(OutcomeVerdict::Redispatch),
            _ => None,
        }
    }

    /// The word of the verdict: `done`, `failed` or `redispatch`.
    pub fn word(&self) -> &'static str {
        match self.verdict {
            OutcomeVerdict::Confirmed => "done",
            OutcomeVerdict::Redispatch => "redispatch",
            _ => "failed",
        }
    }

    fn verdict_record(&self) -> MissionVerdict {
        MissionVerdict::new(self.verdict, self.by, self.at, Some(self.reason.to_owned()))
            .with_amendment(self.amendment.map(str::to_owned))
    }
}

/// Why `latest`, the latest run at a Mission, is not one `operation` may decide, recommend
/// on or probe: it is not the Operation's, or not verifying.
///
/// # Errors
///
/// `NotFound` for no run or another Operation's; `Unsettled` with its state otherwise.
pub fn check_verifying(latest: &Option<RunRecord>, operation: &str) -> Result<(), RunError> {
    match latest {
        Some(record) if record.operation.as_deref() == Some(operation) => {
            let state = record
                .result
                .as_ref()
                .map_or("unrecorded", |result| result.state.as_str());
            if state == "verifying" && !record.status.is_closed() {
                Ok(())
            } else {
                Err(RunError::Unsettled(state.to_owned()))
            }
        }
        _ => Err(RunError::NotFound),
    }
}

fn audit(decision: &VerifyingDecision<'_>, action: &str) -> AuditRecord {
    AuditRecord {
        id: uuid::Uuid::new_v4().to_string(),
        actor: decision.by.to_owned(),
        action: action.to_owned(),
        operation: decision.operation.to_owned(),
        resource: Some(format!("mission:{}", decision.mission)),
        target: format!("{} {}", decision.mission, decision.word()),
        at: decision.at,
    }
}

/// The timeline entry of an applied decision. One that cannot be written is left out: the audit
/// record and the verdict stand.
fn timeline(
    store: &dyn RegistryBackend,
    decision: &VerifyingDecision<'_>,
    kind: &str,
    record: &RunRecord,
) {
    let event = TimelineEvent::new(kind, decision.word(), decision.operation, decision.at)
        .mission(record.mission_id.clone())
        .run(record.id.clone())
        .actor(decision.by);
    let _ = store.record_event(&event);
}

/// Decides the verifying run: audited, recorded as its verdict, and on the timeline. The
/// Mission's Station then settles it done or failed, or queues the next run. A later decision
/// replaces this one until then.
///
/// # Errors
///
/// `NotFound`, or `Unsettled` when the latest run is not verifying; `Io` for a decision out of
/// bounds, an audit that cannot be written, or a Registry failure.
pub fn decide(
    store: &dyn RegistryBackend,
    decision: &VerifyingDecision<'_>,
) -> Result<RunRecord, RunError> {
    check_verifying(&store.latest_run(decision.mission)?, decision.operation)?;
    // Audited first: a decision that cannot be audited is not given.
    store.record_audit(&audit(decision, "mission.verify"))?;
    let record = store.record_verdict(
        decision.operation,
        decision.mission,
        &decision.verdict_record(),
    )?;
    timeline(store, decision, "verify", &record);
    Ok(record)
}

/// Records what a charge at `desk` clearance advises for the verifying run, as `decide` does a
/// decision, except that nothing is settled: a person confirms it by deciding the same way.
///
/// # Errors
///
/// As [`decide`].
pub fn recommend(
    store: &dyn RegistryBackend,
    decision: &VerifyingDecision<'_>,
) -> Result<RunRecord, RunError> {
    check_verifying(&store.latest_run(decision.mission)?, decision.operation)?;
    store.record_audit(&audit(decision, "mission.recommend"))?;
    let record = store.record_recommendation(
        decision.operation,
        decision.mission,
        &decision.verdict_record(),
    )?;
    timeline(store, decision, "recommend", &record);
    Ok(record)
}
