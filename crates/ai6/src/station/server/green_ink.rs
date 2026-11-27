//! The Station's side of green ink: an Agent's request reaches the Registry, its Mission
//! waits with the Watchdog paused, and the decision goes back to the Agent.
//!
//! Decisions are made elsewhere (Universal Exports writes them to the Registry), so the
//! scheduler looks for them: a decision reaches a live Run as a message, or resumes a
//! Run that ended its turn while it waited. A request nobody decides expires as refused; a
//! person may re-arm it, and the Mission waits for it again.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;
use tokio::time::Instant;

use super::{
    DaemonState, ResumeMission, publish_lifecycle, queue_event, resume_mission, sync_run_record,
};
use crate::directorates::m::approvals::{ActionScope, GREEN_INK_REASON, Requirement, StepRule};
use crate::directorates::m::mission::{Mission, MissionState};
use crate::directorates::m::roster::Roster;
use crate::directorates::registry::agent_records::AgentOutcome;
use crate::directorates::registry::approvals::{
    Approval, ApprovalLink, ApprovalRequest, ApprovalStatus, Decision,
};
use crate::directorates::registry::logbook::Severity;
use crate::directorates::registry::reports::ReportFiler;
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::{MissionId, RunId};
use coqui_acp_provider::{AgentEvent, AgentMessage, AgentSession};

/// The event kind that reports a request waiting for green ink, and its decision.
pub(super) const GREEN_INK_EVENT: &str = "green_ink";

/// The timeline and event kind for green ink.
const TIMELINE_KIND: &str = "green_ink";

/// What an Agent asks for through `request_approval`.
pub(super) struct Asked {
    pub(super) operation: String,
    pub(super) mission: String,
    pub(super) run: String,
    pub(super) agent: String,
    pub(super) kind: String,
    pub(super) target: String,
    pub(super) reason: String,
    pub(super) links: Vec<ApprovalLink>,
    pub(super) idempotency_key: String,
    /// Whether the Run's exposure keeps it from class authorisations.
    pub(super) exposed: bool,
}

/// What `request_approval` answers.
#[derive(Debug, Serialize)]
pub(super) struct Answer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) request_id: Option<String>,
    /// `approved`, `pending`, `rejected`, `expired`, `revoked` or `not_required`.
    pub(super) status: &'static str,
    /// When an approval stops covering the step, or when a pending request expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// A Run's way to ask its Station for green ink.
#[derive(Clone)]
pub(super) struct Desk {
    state: DaemonState,
}

impl Desk {
    pub(super) fn new(state: DaemonState) -> Self {
        Self { state }
    }

    /// `[approvals] checked_steps`.
    pub(super) fn checked_steps(&self) -> crate::bounds::Limit {
        self.state.station.limits().approvals.checked_steps
    }

    /// Whether `agent` may propose Missions in `operation` now: it works at `00` there.
    pub(super) fn may_propose(&self, operation: &str, agent: &str) -> bool {
        super::agents::effective(&self.state, Some(operation), agent)
            .is_ok_and(|agent| agent.clearance == "00")
    }

    /// Records a request and answers at once: approved by a class authorisation, not
    /// required by the Operation's policy, or pending while the Mission waits for a person.
    ///
    /// # Errors
    ///
    /// A message the Agent may read: the step is out of bounds, the Agent may not ask, or the
    /// Registry could not be written.
    pub(super) fn request(&self, asked: Asked) -> Result<Answer, String> {
        self.ask(asked, false)
    }

    /// As [`Desk::request`], except that a step the Operation's policy would let the Agent
    /// take without asking still waits for a person: for what the Station itself never does
    /// unattended, whatever the policy says.
    ///
    /// # Errors
    ///
    /// As [`Desk::request`].
    pub(super) fn request_always(&self, asked: Asked) -> Result<Answer, String> {
        self.ask(asked, true)
    }

    fn ask(&self, asked: Asked, always: bool) -> Result<Answer, String> {
        let state = &self.state;
        let now = state.clock.now() as f64;
        let operation = state
            .roster
            .operation(&asked.operation)
            .map_err(|_| "the Operation is not configured on this Station".to_owned())?;
        let mission = state
            .missions
            .lock()
            .ok()
            .and_then(|missions| missions.get(&asked.mission).cloned())
            .ok_or("this Station has no such Mission")?;
        let clearance = super::agents::effective(state, Some(&asked.operation), &asked.agent)
            .map_err(|_| "the Agent is not configured for this Operation".to_owned())?
            .clearance;
        let scope = ActionScope::try_new(&asked.kind, &asked.target)
            .and_then(|scope| {
                scope
                    .check_size(&state.station.limits().approvals)
                    .map(|()| scope)
            })
            .map_err(|e| e.to_string())?;
        let policy = &operation.green_ink;
        let parked = match policy.rule(&scope) {
            Some((StepRule::Deny, entry)) => {
                return Err(format!(
                    "the Operation denies {} {} (green_ink deny {entry:?}); the step is not asked for",
                    scope.kind, scope.target
                ));
            }
            Some((StepRule::Park, _)) => true,
            _ => false,
        };
        let (always, exposed) = (always || parked, asked.exposed || parked);
        match policy.requirement(&clearance, &scope) {
            Requirement::NotRequired if always => {}
            Requirement::NotRequired => {
                return Ok(Answer {
                    request_id: None,
                    status: "not_required",
                    expires_at: None,
                    message: Some("the Operation's policy lets this Agent take the step".into()),
                });
            }
            Requirement::GreenInk => {}
            _ if !policy.requesters().any(|known| known == clearance) => {
                return Err(format!("a {clearance} Agent may not ask for green ink"));
            }
            _ => {
                return Err(format!(
                    "the Operation does not govern {:?}; ask for one of: {}",
                    scope.kind,
                    policy.kinds().collect::<Vec<_>>().join(", ")
                ));
            }
        }
        let request = ApprovalRequest::try_new(
            &asked.operation,
            MissionId::try_new(&asked.mission).map_err(|e| e.to_string())?,
            RunId::try_new(&asked.run).map_err(|e| e.to_string())?,
            &asked.agent,
            scope,
            asked.reason,
            &asked.idempotency_key,
            &state.intake.limits().approvals,
        )
        .map_err(|e| e.to_string())?
        .with_links(asked.links)
        .expiring(now, policy.request_ttl());
        let registry = &state.intake;
        let unavailable = |error: std::io::Error| format!("the request was not recorded: {error}");
        if let Some(status) = registry
            .approval(&asked.operation, &request.id, now)
            .map_err(unavailable)?
        {
            if matches!(status, ApprovalStatus::Pending(_)) {
                hold(state, &asked.mission);
            }
            return Ok(answer_at(&status, now));
        }
        if let Some(earlier) =
            self.earlier(&asked.operation, &asked.mission, &request.scope, now)?
        {
            return Ok(earlier);
        }
        if !exposed
            && let Some(approval) = registry
                .approve_by_class(&request, mission.role_id.as_deref(), &clearance, now)
                .map_err(unavailable)?
        {
            record_class_use(state, &approval);
            return Ok(answer(&ApprovalStatus::Decided(approval)));
        }
        let status = registry.request_approval(&request).map_err(unavailable)?;
        record_event(state, &request, "requested", &request.agent);
        audit(
            state,
            "green_ink.requested",
            &format!("agent:{}", request.agent),
            &request,
            now,
        );
        queue_event(
            state,
            &asked.mission,
            AgentEvent {
                kind: GREEN_INK_EVENT.to_owned(),
                text: Some(format!(
                    "waiting for green ink to {} {} ({})",
                    request.scope.kind, request.scope.target, request.id
                )),
                data: None,
                limit: None,
                tools: Vec::new(),
                report: None,
            },
        );
        hold(state, &asked.mission);
        Ok(answer(&status))
    }
    /// The answer an earlier request for the same step gives, if one still stands: it waits, it
    /// is approved and live, or a person rejected it and nobody reopened it. A second request
    /// for the step is never filed in its place.
    fn earlier(
        &self,
        operation: &str,
        mission: &str,
        scope: &ActionScope,
        now: f64,
    ) -> Result<Option<Answer>, String> {
        let state = &self.state;
        let mut same = state
            .intake
            .step_requests(operation, &scope.kind)
            .map_err(|error| format!("the request was not recorded: {error}"))?
            .into_iter()
            .filter(|status| status.request().scope.same_step(scope))
            .collect::<Vec<_>>();
        let when = |status: &ApprovalStatus| match status {
            ApprovalStatus::Pending(request) => request.requested_at,
            ApprovalStatus::Decided(approval) => approval.at,
        };
        same.sort_by(|left, right| when(right).total_cmp(&when(left)));
        for status in &same {
            let request = status.request();
            let found = match status {
                ApprovalStatus::Pending(request) if request.expires_at > now => {
                    if request.mission.as_str() == mission {
                        hold(state, mission);
                    }
                    let mut found = answer(status);
                    found.message = Some(format!(
                        "this step is already requested ({}); no second request was filed. An \
                         operator decides; do not take the step before it is approved",
                        request.id
                    ));
                    found
                }
                ApprovalStatus::Decided(approval) if approval.is_live(now) => answer(status),
                ApprovalStatus::Decided(approval) if approval.decision == Decision::Rejected => {
                    Answer {
                        request_id: Some(request.id.clone()),
                        status: "rejected",
                        expires_at: None,
                        message: Some(format!(
                            "{} rejected this step{}. The rejection stands until an operator \
                             reopens request {}; do not ask again and do not take the step",
                            approval.signer,
                            approval
                                .note
                                .as_deref()
                                .map(|note| format!(": {note}"))
                                .unwrap_or_default(),
                            request.id
                        )),
                    }
                }
                _ => continue,
            };
            gate_line(
                state,
                operation,
                mission,
                &scope.kind,
                &scope.target,
                &format!("{} by request {}", found.status, request.id),
            );
            return Ok(Some(found));
        }
        Ok(None)
    }
}

/// What a harness gate asks before a command runs: may this Agent take these steps now?
pub(super) struct Checked {
    pub(super) operation: String,
    pub(super) mission: String,
    /// The Run that attempts the steps, when the caller knows it.
    pub(super) run: Option<String>,
    /// Whether the Run's exposure keeps it from class authorisations.
    pub(super) exposed: bool,
    pub(super) agent: String,
    pub(super) steps: Vec<(String, String)>,
}

/// The gate's answer for one step.
#[derive(Debug, Serialize)]
pub(super) struct StepAnswer {
    kind: String,
    target: String,
    /// `approved` (a live approval covers it), `not_required`, `needs_approval` or `refused`.
    pub(super) status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) message: Option<String>,
}

/// The check `theater_run` makes before a command: [`Desk::check`] for `agent` on `mission`.
/// A Mission without an Operation cannot be checked, so its governed steps are held.
pub(super) fn step_check(
    state: &DaemonState,
    mission: &Mission,
    agent: &str,
) -> super::theater_mcp::StepCheck {
    let desk = Desk::new(state.clone());
    let operation = mission.operation_id.clone();
    let mission = mission.mission_id.to_string();
    let agent = agent.to_owned();
    Arc::new(move |steps| {
        let operation = operation
            .clone()
            .ok_or("the Mission has no Operation, so green ink cannot be checked")?;
        let answers = desk.check(Checked {
            operation,
            mission: mission.clone(),
            run: None,
            exposed: true,
            agent: agent.clone(),
            steps: steps
                .iter()
                .map(|step| (step.kind.clone(), step.target.clone()))
                .collect(),
        })?;
        Ok(answers
            .into_iter()
            .map(|answer| (answer.status.to_owned(), answer.message))
            .collect())
    })
}

impl Desk {
    /// Answers, for each step, whether the Agent may take it now. A step no approval covers is
    /// filed as a request on the Agent's behalf, so a denial always leaves a request to decide;
    /// when that cannot be filed the step is refused naming why. Every answer is journaled.
    ///
    /// # Errors
    ///
    /// A message the gate passes on: the Operation, Mission or Agent is unknown here, or the
    /// Registry could not be read.
    pub(super) fn check(&self, checked: Checked) -> Result<Vec<StepAnswer>, String> {
        let (operation, mission) = (checked.operation.clone(), checked.mission.clone());
        let (kind, target) = checked
            .steps
            .first()
            .map_or(("-".to_owned(), "-".to_owned()), Clone::clone);
        self.check_steps(checked).inspect_err(|why| {
            gate_line(
                &self.state,
                &operation,
                &mission,
                &kind,
                &target,
                &format!("error: {why}"),
            );
        })
    }

    fn check_steps(&self, checked: Checked) -> Result<Vec<StepAnswer>, String> {
        let state = &self.state;
        let now = state.clock.now() as f64;
        let operation = state
            .roster
            .operation(&checked.operation)
            .map_err(|_| "the Operation is not configured on this Station".to_owned())?;
        let clearance = super::agents::effective(state, Some(&checked.operation), &checked.agent)
            .map_err(|_| "the Agent is not configured for this Operation".to_owned())?
            .clearance;
        let policy = &operation.green_ink;
        let mut answers = Vec::new();
        for (kind, target) in checked.steps {
            let scope = ActionScope::try_new(&kind, &target)
                .and_then(|scope| {
                    scope
                        .check_size(&state.station.limits().approvals)
                        .map(|()| scope)
                })
                .map_err(|e| e.to_string())?;
            let rule = policy.rule(&scope);
            let (mut status, mut request_id, mut message) =
                match policy.requirement(&clearance, &scope) {
                    _ if matches!(rule, Some((StepRule::Deny, _))) => (
                        "refused",
                        None,
                        Some(format!(
                            "this Operation denies {kind} {target} (green_ink deny {:?})",
                            rule.map_or("", |(_, entry)| entry)
                        )),
                    ),
                    Requirement::NotRequired if rule.is_none() => ("not_required", None, None),
                    Requirement::NotRequired | Requirement::GreenInk => match state
                        .intake
                        .live_approval(&checked.operation, &checked.mission, &scope, now)
                        .map_err(|error| format!("green ink could not be checked: {error}"))?
                    {
                        Some(approval) => {
                            if let Err(error) = state.intake.mark_approval_used(
                                &checked.operation,
                                &approval.request.id,
                                now,
                            ) {
                                eprintln!(
                                    "ai6: green ink approval {:?} was used but not marked: {error}",
                                    approval.request.id
                                );
                            }
                            ("approved", Some(approval.request.id), None)
                        }
                        None => ("needs_approval", None, None),
                    },
                    _ => (
                        "refused",
                        None,
                        Some(format!(
                            "a {clearance} Agent may not {kind} under this Operation's policy"
                        )),
                    ),
                };
            if status == "needs_approval" {
                let run = checked
                    .run
                    .clone()
                    .or_else(|| super::run_hold(state, &checked.mission).map(|hold| hold.id));
                let filed = match run {
                    Some(run) => self.request(Asked {
                        operation: checked.operation.clone(),
                        mission: checked.mission.clone(),
                        run,
                        agent: checked.agent.clone(),
                        kind: kind.clone(),
                        target: target.clone(),
                        reason: format!(
                            "The Agent attempted to {kind} {target} and the gate stopped it \
                             until a person approves. Filed by the gate; the Agent gave no \
                             reason of its own."
                        ),
                        links: Vec::new(),
                        // A fresh key each time: an earlier request for the step that no longer stands
                        // (revoked, spent, lapsed) must not be answered in place of a new one.
                        idempotency_key: format!(
                            "gate:{kind}:{target}:{}",
                            uuid::Uuid::new_v4().simple()
                        ),
                        exposed: checked.exposed,
                    }),
                    None => Err("the Mission has no Run to file it under".to_owned()),
                };
                match filed {
                    Ok(answer) if answer.status == "approved" => {
                        status = "approved";
                        request_id = answer.request_id;
                    }
                    Ok(answer) if answer.status == "pending" => {
                        message = Some(format!(
                            "denied until a person approves; request {} is filed. Wait for the \
                             decision, which arrives as a message",
                            answer.request_id.as_deref().unwrap_or_default()
                        ));
                        request_id = answer.request_id;
                    }
                    Ok(answer) => {
                        status = "refused";
                        message = answer.message;
                        request_id = answer.request_id;
                    }
                    Err(why) => {
                        status = "refused";
                        message =
                            Some(format!("denied, and the request could not be filed: {why}"));
                    }
                }
            }
            gate_line(
                state,
                &checked.operation,
                &checked.mission,
                &kind,
                &target,
                &format!(
                    "{status}{}{}",
                    request_id
                        .as_deref()
                        .map(|id| format!(" (request {id})"))
                        .unwrap_or_default(),
                    message
                        .as_deref()
                        .map(|why| format!(": {why}"))
                        .unwrap_or_default()
                ),
            );
            answers.push(StepAnswer {
                kind,
                target,
                status,
                request_id,
                message,
            });
        }
        Ok(answers)
    }
}

/// The Station's journal line for a gate decision: whose step, and what became of it.
fn gate_line(
    state: &DaemonState,
    operation: &str,
    mission: &str,
    kind: &str,
    target: &str,
    outcome: &str,
) {
    let line = format!(
        "ai6: green ink at {}: Operation {operation:?}, Mission {mission:?}, {kind} {target:?}: {outcome}",
        state.station_id
    );
    #[cfg(test)]
    JOURNAL.lock().unwrap().push(line.clone());
    eprintln!("{line}");
}

/// The gate lines journaled so far, for tests.
#[cfg(test)]
static JOURNAL: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// As [`answer`], except that an approval a person withdrew, or that has lapsed, is not `approved`.
fn answer_at(status: &ApprovalStatus, now: f64) -> Answer {
    let mut found = answer(status);
    if let ApprovalStatus::Decided(approval) = status
        && approval.decision == Decision::Approved
        && !approval.is_live(now)
    {
        found.status = if approval.revoked.is_some() {
            "revoked"
        } else {
            "expired"
        };
        found.message = Some(
            "the approval no longer covers the step; ask again with a new idempotency_key"
                .to_owned(),
        );
    }
    found
}

fn answer(status: &ApprovalStatus) -> Answer {
    let request = status.request();
    match status {
        ApprovalStatus::Pending(_) => Answer {
            request_id: Some(request.id.clone()),
            status: "pending",
            expires_at: Some(request.expires_at),
            message: Some(
                "an operator decides; the decision arrives as a message. Do not take the step \
                 before it is approved"
                    .to_owned(),
            ),
        },
        ApprovalStatus::Decided(approval) => Answer {
            request_id: Some(request.id.clone()),
            status: approval.decision.as_str(),
            expires_at: approval.expires_at,
            message: approval.note.clone(),
        },
    }
}

/// Blocks the Mission for green ink and pauses its Watchdog: waiting for a person is not a
/// stall.
fn hold(state: &DaemonState, mission_id: &str) {
    let blocked = state
        .missions
        .lock()
        .ok()
        .is_some_and(|mut missions| missions.block_for(mission_id, GREEN_INK_REASON).is_ok());
    if let Ok(runs) = state.runs.lock()
        && let Some(operation) = runs.get(mission_id)
    {
        operation.watchdog.pause();
    }
    if blocked {
        sync_run_record(state, mission_id);
        publish_lifecycle(state, mission_id, MissionState::Blocked);
    }
}

/// Whether a Mission waits for green ink.
fn waits(mission: &Mission) -> bool {
    mission.state == MissionState::Blocked
        && mission.blocked_reason.as_deref() == Some(GREEN_INK_REASON)
}

/// Ends a Run whose turn completed while its Mission waits for green ink, keeping the
/// Mission blocked so the decision resumes it. `false` when the Mission does not wait.
pub(super) fn end_turn_waiting(
    state: &DaemonState,
    mission_id: &str,
    run: &Arc<std::sync::Mutex<guatiao::library::Object<dyn AgentSession>>>,
) -> bool {
    let Some(mission) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
    else {
        return false;
    };
    let Some(operation_id) = mission.operation_id.as_deref() else {
        return false;
    };
    let pending = state
        .intake
        .mission_approvals(operation_id, mission_id, state.clock.now() as f64)
        .is_ok_and(|statuses| {
            statuses
                .iter()
                .any(|status| matches!(status, ApprovalStatus::Pending(_)))
        });
    let asked = mission.state == MissionState::Blocked
        && mission.blocked_reason.as_deref() == Some(crate::station::issues::ISSUE_REASON)
        && state
            .intake
            .issue_questions(mission_id)
            .is_ok_and(|questions| {
                questions.iter().any(|question| {
                    question.state
                        == crate::directorates::registry::mission_issues::QuestionState::Open
                })
            });
    if !(asked || waits(&mission) && pending) {
        return false;
    }
    let operation = state.runs.lock().ok().and_then(|mut runs| {
        runs.get(mission_id)
            .is_some_and(|operation| Arc::ptr_eq(&operation.run, run))
            .then(|| runs.remove(mission_id))
            .flatten()
            .inspect(|operation| operation.gate.close())
    });
    if let Some(operation) = operation
        && let Ok(run) = operation.run.lock()
    {
        let _ = run.close();
    }
    sync_run_record(state, mission_id);
    true
}

/// A Mission's live Run and the gate writes to it go through.
type LiveRun = (
    Arc<std::sync::Mutex<guatiao::library::Object<dyn AgentSession>>>,
    super::RunGate,
);

/// Tells Agents the decisions on their requests and lets their Missions go on once nothing
/// they asked for is pending.
pub(super) async fn deliver_decisions(state: &DaemonState) {
    let missions = state
        .missions
        .lock()
        .map(|missions| missions.list())
        .unwrap_or_default()
        .into_iter()
        .filter(|mission| matches!(mission.state, MissionState::Working | MissionState::Blocked))
        .collect::<Vec<_>>();
    let now = state.clock.now() as f64;
    for mission in missions {
        let Some(operation_id) = mission.operation_id.as_deref() else {
            continue;
        };
        let mission_id = mission.mission_id.as_str();
        let Ok(statuses) = state
            .intake
            .mission_approvals(operation_id, mission_id, now)
        else {
            continue;
        };
        let pending = statuses
            .iter()
            .any(|status| matches!(status, ApprovalStatus::Pending(_)));
        let rearmed = statuses
            .iter()
            .filter_map(|status| match status {
                ApprovalStatus::Pending(request)
                    if request
                        .rearmed
                        .as_ref()
                        .is_some_and(|rearming| rearming.told_at.is_none()) =>
                {
                    Some(request.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let untold = statuses
            .into_iter()
            .filter_map(|status| match status {
                ApprovalStatus::Decided(approval) if approval.notified_at.is_none() => {
                    Some(approval)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let live = state.runs.lock().ok().and_then(|runs| {
            runs.get(mission_id)
                .map(|operation| (operation.run.clone(), operation.gate.clone()))
        });
        for request in &rearmed {
            take_up_rearmed(state, &mission, request, live.as_ref(), now).await;
        }
        match live {
            Some((run, gate)) => {
                for approval in &untold {
                    let message = AgentMessage {
                        text: decision_text(approval, now),
                    };
                    let run = run.clone();
                    let gate = gate.clone();
                    let sent = tokio::task::spawn_blocking(move || {
                        gate.lock(&run)
                            .ok()
                            .is_some_and(|run| run.send_message(message).is_ok())
                    })
                    .await
                    .unwrap_or(false);
                    if sent {
                        told(state, approval, now);
                    }
                }
                if !pending && waits(&mission) {
                    release(state, mission_id);
                }
            }
            None if !pending && waits(&mission) && !untold.is_empty() => {
                resume_with(state, mission_id, &untold, now).await;
            }
            None => {}
        }
    }
}

/// When the last sweep for unspent approvals ran, as `f64` bits of Unix seconds.
static UNSPENT_SWEPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Puts in front of a person each approval that lapsed since the last sweep with no step ever
/// taken under it: the work it was for did not happen. One Report per approval, so a sweep seen
/// twice, or by two Stations, files it once.
pub(super) fn report_unspent(state: &DaemonState) {
    use std::sync::atomic::Ordering;
    let now = state.clock.now() as f64;
    let since = f64::from_bits(UNSPENT_SWEPT.swap(now.to_bits(), Ordering::AcqRel));
    report_unspent_since(state, since, now);
}

fn report_unspent_since(state: &DaemonState, since: f64, now: f64) {
    for operation in state.roster.operations() {
        let lapsed = match state.intake.lapsed_approvals(&operation.id, since, now) {
            Ok(lapsed) => lapsed,
            Err(error) => {
                eprintln!(
                    "ai6: unspent approvals of Operation {:?} were not read: {error}",
                    operation.id
                );
                continue;
            }
        };
        for approval in lapsed {
            let request = &approval.request;
            super::observations::observe(
                state,
                super::observations::Observed {
                    operation: &operation.id,
                    filer: ReportFiler::Probe {
                        probe: "green-ink".to_owned(),
                    },
                    kind: "green_ink",
                    severity: Severity::Warning,
                    about: &request.id,
                    title: format!(
                        "Approved {} {} lapsed unspent",
                        request.scope.kind, request.scope.target
                    ),
                    text: format!(
                        "{} approved request {} to {} {} for Mission {}, and no step was taken \
                         under it before it lapsed. The work it was for did not happen; ask \
                         for it again or do it by hand.",
                        approval.signer,
                        request.id,
                        request.scope.kind,
                        request.scope.target,
                        request.mission
                    ),
                    mission: Some(request.mission.as_str()),
                    links: Vec::new(),
                },
            );
            gate_line(
                state,
                &operation.id,
                request.mission.as_str(),
                &request.scope.kind,
                &request.scope.target,
                &format!("approval {} lapsed unspent", request.id),
            );
        }
    }
}

/// Takes up a request a person re-armed: a working Mission waits for it as for a new request, and
/// a live Run is told, having heard the request expired. A Mission no Run of which hears
/// it is told with the decision.
async fn take_up_rearmed(
    state: &DaemonState,
    mission: &Mission,
    request: &ApprovalRequest,
    live: Option<&LiveRun>,
    now: f64,
) {
    if let Some((run, gate)) = live {
        let message = AgentMessage {
            text: rearmed_text(request),
        };
        let (run, gate) = (run.clone(), gate.clone());
        let sent = tokio::task::spawn_blocking(move || {
            gate.lock(&run)
                .ok()
                .is_some_and(|run| run.send_message(message).is_ok())
        })
        .await
        .unwrap_or(false);
        if !sent {
            return;
        }
    }
    let mission_id = mission.mission_id.as_str();
    if mission.state == MissionState::Working {
        hold(state, mission_id);
    }
    if let Err(error) = state
        .intake
        .mark_rearming_told(&request.operation, &request.id, now)
    {
        eprintln!(
            "ai6: green ink request {:?} was re-armed but not marked: {error}",
            request.id
        );
    }
    queue_event(
        state,
        mission_id,
        AgentEvent {
            kind: GREEN_INK_EVENT.to_owned(),
            text: Some(format!(
                "waiting again for green ink to {} {} ({})",
                request.scope.kind, request.scope.target, request.id
            )),
            data: None,
            limit: None,
            tools: Vec::new(),
            report: None,
        },
    );
}

/// What the Agent is told of a request a person re-armed.
fn rearmed_text(request: &ApprovalRequest) -> String {
    format!(
        "Green ink: request {} to {} {} was re-armed by {}: it is waiting for a decision again. \
         Do not take the step before it is approved; the decision arrives as a message.",
        request.id,
        request.scope.kind,
        request.scope.target,
        request
            .rearmed
            .as_ref()
            .map_or("a person", |rearming| rearming.by.as_str()),
    )
}

/// Moves a Mission whose live Run no longer waits back to work.
fn release(state: &DaemonState, mission_id: &str) {
    let working = state.missions.lock().ok().is_some_and(|mut missions| {
        missions
            .transition(mission_id, MissionState::Working)
            .is_ok()
    });
    if let Ok(runs) = state.runs.lock()
        && let Some(operation) = runs.get(mission_id)
    {
        operation.watchdog.resume();
    }
    if working {
        sync_run_record(state, mission_id);
        publish_lifecycle(state, mission_id, MissionState::Working);
    }
}

/// Resumes a Run that ended its turn while it waited, with the decisions as its message.
async fn resume_with(state: &DaemonState, mission_id: &str, untold: &[Approval], now: f64) {
    let waiting = state
        .retry_after
        .lock()
        .ok()
        .and_then(|retries| retries.get(mission_id).copied())
        .is_some_and(|retry_at| retry_at > Instant::now());
    if waiting {
        return;
    }
    let text = untold
        .iter()
        .map(|approval| decision_text(approval, now))
        .collect::<Vec<_>>()
        .join("\n");
    let resumed = resume_mission(
        State(state.clone()),
        Path(mission_id.to_owned()),
        Json(ResumeMission {
            message: Some(text),
            environment: Default::default(),
        }),
    )
    .await;
    match resumed {
        Ok(_) => {
            for approval in untold {
                told(state, approval, now);
            }
            if let Ok(mut retries) = state.retry_after.lock() {
                retries.remove(mission_id);
            }
        }
        Err(error) => {
            eprintln!(
                "ai6: Mission {mission_id:?} was not resumed with its green ink decision: {}",
                error.1.0.error
            );
            if let Ok(mut retries) = state.retry_after.lock() {
                retries.insert(
                    mission_id.to_owned(),
                    Instant::now() + state.station.limits().scheduler.retry_backoff.0,
                );
            }
        }
    }
}

/// Records that the Agent was told, on the Registry record and the timeline.
fn told(state: &DaemonState, approval: &Approval, now: f64) {
    let request = &approval.request;
    if let Err(error) = state
        .intake
        .mark_approval_notified(&request.operation, &request.id, now)
    {
        eprintln!(
            "ai6: green ink request {:?} was told but not marked: {error}",
            request.id
        );
    }
    record_event(state, request, approval.decision.as_str(), &approval.signer);
    if approval.decision == Decision::Expired {
        audit(state, "green_ink.expired", &approval.signer, request, now);
    }
    if approval.decision == Decision::Rejected {
        super::agents::count(
            state,
            &request.agent,
            Some(&request.operation),
            AgentOutcome::GreenInkRejected,
            format!("green-ink#{}", request.id),
            None,
        );
    }
    queue_event(
        state,
        request.mission.as_str(),
        AgentEvent {
            kind: GREEN_INK_EVENT.to_owned(),
            text: Some(format!(
                "green ink {} for {} {} ({})",
                approval.decision.as_str(),
                request.scope.kind,
                request.scope.target,
                request.id
            )),
            data: None,
            limit: None,
            tools: Vec::new(),
            report: None,
        },
    );
}

/// What the Agent is told about a decision.
fn decision_text(approval: &Approval, now: f64) -> String {
    let request = &approval.request;
    let step = format!(
        "Green ink: request {} to {} {}",
        request.id, request.scope.kind, request.scope.target
    );
    match approval.decision {
        Decision::Approved => {
            let minutes = approval
                .expires_at
                .map_or(0.0, |expires| ((expires - now) / 60.0).ceil().max(0.0));
            format!(
                "{step} was approved by {}. Take exactly that step within {minutes:.0} minutes; \
                 the approval does not cover anything else.",
                approval.signer
            )
        }
        Decision::Rejected => format!(
            "{step} was rejected by {}{}. Do not take that step.",
            approval.signer,
            approval
                .note
                .as_deref()
                .map(|note| format!(": {note}"))
                .unwrap_or_default()
        ),
        _ => format!("{step} expired without a decision. Treat it as rejected."),
    }
}

/// Audits a class authorisation's use; the actor is the class authorisation itself.
fn record_class_use(state: &DaemonState, approval: &Approval) {
    let request = &approval.request;
    audit(
        state,
        "class_authorization.used",
        &approval.signer,
        request,
        approval.at,
    );
    record_event(state, request, "approved", &approval.signer);
}

/// Audits `action` on a request: the target names the request and its step, never the
/// Agent's reason.
fn audit(state: &DaemonState, action: &str, actor: &str, request: &ApprovalRequest, at: f64) {
    let record = AuditRecord {
        id: format!("{action}-{}", uuid::Uuid::new_v4().simple()),
        actor: actor.to_owned(),
        action: action.to_owned(),
        operation: request.operation.clone(),
        resource: None,
        target: format!(
            "{} {} {}",
            request.id, request.scope.kind, request.scope.target
        ),
        at,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: green ink audit record {action} was not written: {error}");
    }
}

fn record_event(state: &DaemonState, request: &ApprovalRequest, action: &str, actor: &str) {
    let event = TimelineEvent::new(
        TIMELINE_KIND,
        action,
        request.operation.clone(),
        state.clock.now() as f64,
    )
    .station(state.station_id.clone())
    .mission(request.mission.to_string())
    .run(request.run.to_string())
    .actor(actor)
    .detail("request", request.id.clone())
    .detail("step", request.scope.kind.clone())
    .detail("target", request.scope.target.clone());
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: green ink timeline event was not recorded: {error}");
    }
}

/// Lists, for the operator, the green ink settings each Operation loosens.
pub(super) fn log_weakenings(roster: &Roster) {
    for weakened in weakenings(roster) {
        eprintln!("ai6: {weakened}");
    }
}

/// Every green ink setting looser than its default: each Operation's policy, and each Agent
/// whose roster environment names its own gate program for the Claude hook.
fn weakenings(roster: &Roster) -> Vec<String> {
    let gate = crate::directorates::section_00::gate::GATE_PROGRAM_ENV;
    let mut weakened = Vec::new();
    let mut agents = std::collections::BTreeMap::new();
    for operation in roster.operations() {
        for line in operation.green_ink.weakenings() {
            weakened.push(format!(
                "Operation {:?} green ink is weakened: {line}",
                operation.id
            ));
        }
        for agent in roster
            .agents_for_operation(&operation.id)
            .unwrap_or_default()
        {
            if let Some(program) = agent.env.get(gate) {
                agents.insert(agent.id.clone(), program.clone());
            }
        }
    }
    for (agent, program) in agents {
        weakened.push(format!(
            "Agent {agent:?} green ink is weakened: its {gate} runs {program:?} as the hook's gate \
             instead of the ai6 program hosting the handler"
        ));
    }
    weakened
}

#[cfg(test)]
mod tests {
    use std::path::Path as FsPath;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use super::super::tests::{
        RECORDED_MESSAGES, RECORDED_RESUMES, start_mission_by_id, test_state, wait_for_state,
    };
    use super::*;
    use crate::directorates::registry::approvals::{ClassAuthorization, Ruling};

    /// 007 runs on the recording mock, which reports its Run blocked at once and then stays
    /// quiet; its Watchdog stalls a quiet Run after 5 seconds, long enough for a
    /// loaded Windows runner to make the MCP tool call before it.
    const AGENTS: &str = "[agent.007]\nhandler = \"mock-recorder\"\nclearance = \"field\"\n\
                          [agent.007.limits]\nstall_timeout = 5\n\
                          [agent.desk]\nhandler = \"mock-recorder\"\nclearance = \"desk\"\n";

    /// A Station with one queued Mission of a unique ID for Agent 007 of Operation G.
    fn station(root: &FsPath, name: &str) -> (DaemonState, String) {
        station_with(root, name, AGENTS)
    }

    /// As [`station`], for steps a loaded machine may take longer over than the Watchdog waits.
    fn patient_station(root: &FsPath, name: &str) -> (DaemonState, String) {
        station_with(
            root,
            name,
            &AGENTS.replace("stall_timeout = 5", "stall_timeout = 600"),
        )
    }

    fn station_with(root: &FsPath, name: &str, agents: &str) -> (DaemonState, String) {
        let state = test_state(root, "", agents);
        let mission_id = format!("green-ink-{name}-{}", std::process::id());
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(&mission_id).unwrap(),
                Some("G".to_owned()),
                "release the work".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        (state, mission_id)
    }

    async fn call_tool(
        state: &DaemonState,
        mission_id: &str,
        arguments: serde_json::Value,
    ) -> String {
        use rmcp::ServiceExt;
        use rmcp::model::{CallToolRequestParams, ContentBlock};
        use rmcp::transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        };
        let environment = state.runs.lock().unwrap()[mission_id].environment.clone();
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(environment["AI6_CABLE_MCP_URL"].clone())
                .auth_header(environment["AI6_CABLE_MCP_TOKEN"].clone()),
        );
        let client = ().serve(transport).await.unwrap();
        let response = client
            .call_tool(
                CallToolRequestParams::new("request_approval")
                    .with_arguments(serde_json::from_value(arguments).unwrap()),
            )
            .await
            .unwrap();
        client.cancel().await.unwrap();
        let [ContentBlock::Text(text)] = response.content.as_slice() else {
            panic!("the tool answers one text block");
        };
        text.text.clone()
    }

    fn push_main(key: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "push",
            "target": "origin:refs/heads/main",
            "reason": "the release branch is green",
            "idempotency_key": key,
        })
    }

    fn mission(state: &DaemonState, mission_id: &str) -> Mission {
        state
            .missions
            .lock()
            .unwrap()
            .get(mission_id)
            .cloned()
            .unwrap()
    }

    fn now(state: &DaemonState) -> f64 {
        state.clock.now() as f64
    }

    fn recorded_about(request_id: &str) -> Vec<(String, String)> {
        RECORDED_MESSAGES
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, text)| text.contains(request_id))
            .cloned()
            .collect()
    }

    /// What the mock was told, waiting for it: a message reaches the harness on the ACP
    /// connection's thread, after `send_message` has returned.
    /// Whether a Run is told `words` about `request_id` within the time a message may take.
    async fn is_told(request_id: &str, words: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if recorded_about(request_id)
                .iter()
                .any(|(_, text)| text.contains(words))
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Stops a test's mock Run however the test ends: a Run waiting for green ink has its
    /// Watchdog paused, and its event pump would keep the test's runtime from closing.
    struct StopAtEnd(DaemonState);

    impl Drop for StopAtEnd {
        fn drop(&mut self) {
            self.0.stopping.store(true, Ordering::Release);
        }
    }

    async fn told_about(request_id: &str) -> Vec<(String, String)> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let told = recorded_about(request_id);
            if !told.is_empty() || Instant::now() >= deadline {
                return told;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_run_asks_waits_past_the_watchdog_and_resumes_working_once_approved() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = station(root.path(), "approve");
        start_mission_by_id(&state, &mission_id).await;
        wait_for_state(&state, &mission_id, MissionState::Blocked).await;

        let answer: serde_json::Value =
            serde_json::from_str(&call_tool(&state, &mission_id, push_main("push-1")).await)
                .unwrap();
        assert_eq!(answer["status"], "pending", "{answer}");
        let request_id = answer["request_id"].as_str().unwrap().to_owned();
        let waiting = mission(&state, &mission_id);
        assert_eq!(waiting.state, MissionState::Blocked);
        assert_eq!(waiting.blocked_reason.as_deref(), Some(GREEN_INK_REASON));
        let watchdog = state.runs.lock().unwrap()[&mission_id].watchdog.clone();
        assert!(watchdog.is_paused());

        // More than the stall timeout passes: waiting for a person is not a stall.
        tokio::time::sleep(Duration::from_millis(5_500)).await;
        assert_eq!(mission(&state, &mission_id).state, MissionState::Blocked);
        let pending = state.intake.pending_approvals("G", now(&state)).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].agent, "007");
        assert_eq!(pending[0].run.as_str(), "provider-id-remote");

        // What Universal Exports writes when an operator signs.
        state
            .intake
            .decide_approval(
                "G",
                &request_id,
                &Ruling::approve("operator-m", now(&state), now(&state) + 600.0),
            )
            .unwrap();
        deliver_decisions(&state).await;

        let told = told_about(&request_id).await;
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(told[0].0, "provider-id-remote");
        assert!(
            told[0].1.contains("approved by operator-m"),
            "{}",
            told[0].1
        );
        let working = mission(&state, &mission_id);
        assert_eq!(working.state, MissionState::Working);
        assert_eq!(working.blocked_reason, None);
        assert!(!watchdog.is_paused());
        let Some(ApprovalStatus::Decided(approval)) = state
            .intake
            .approval("G", &request_id, now(&state))
            .unwrap()
        else {
            panic!("the request is decided");
        };
        assert!(approval.notified_at.is_some());
        deliver_decisions(&state).await;
        // Long enough for a second message to arrive if one was sent.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            recorded_about(&request_id).len(),
            1,
            "a decision is told once"
        );
        let actions = state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| {
                event.kind == TIMELINE_KIND && event.mission.as_deref() == Some(&mission_id)
            })
            .map(|event| (event.action, event.actor.unwrap_or_default()))
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            [
                ("requested".to_owned(), "007".to_owned()),
                ("approved".to_owned(), "operator-m".to_owned())
            ]
        );

        let requested = state
            .intake
            .audit_records()
            .unwrap()
            .into_iter()
            .find(|record| {
                record.action == "green_ink.requested" && record.target.starts_with(&request_id)
            })
            .expect("the request is audited");
        assert_eq!(requested.actor, "agent:007");
        assert_eq!(
            requested.target,
            format!("{request_id} push origin:refs/heads/main"),
            "the step, never the reason"
        );

        // Asking again with the same key returns the decision.
        let again: serde_json::Value =
            serde_json::from_str(&call_tool(&state, &mission_id, push_main("push-1")).await)
                .unwrap();
        assert_eq!(again["status"], "approved");
        assert_eq!(again["request_id"], request_id.as_str());
        state.stopping.store(true, Ordering::Release);
    }

    #[tokio::test]
    async fn a_turn_that_ends_while_waiting_is_resumed_with_the_refusal() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = station(root.path(), "reject");
        start_mission_by_id(&state, &mission_id).await;
        wait_for_state(&state, &mission_id, MissionState::Blocked).await;
        let answer: serde_json::Value =
            serde_json::from_str(&call_tool(&state, &mission_id, push_main("push-2")).await)
                .unwrap();
        let request_id = answer["request_id"].as_str().unwrap().to_owned();
        let run = state.runs.lock().unwrap()[&mission_id].run.clone();

        assert!(end_turn_waiting(&state, &mission_id, &run));
        assert!(!state.runs.lock().unwrap().contains_key(&mission_id));
        assert_eq!(mission(&state, &mission_id).state, MissionState::Blocked);
        deliver_decisions(&state).await;
        let resumed_for = |mission_id: &str| {
            RECORDED_RESUMES
                .lock()
                .unwrap()
                .iter()
                .find(|resume| resume.mission_id.as_str() == mission_id)
                .cloned()
        };
        assert!(
            resumed_for(&mission_id).is_none(),
            "nothing resumes before the decision"
        );

        state
            .intake
            .decide_approval(
                "G",
                &request_id,
                &Ruling::reject(
                    "operator-q",
                    now(&state),
                    Some("not before the audit".into()),
                ),
            )
            .unwrap();
        deliver_decisions(&state).await;
        let message = resumed_for(&mission_id)
            .expect("the decision resumes the Run")
            .message
            .unwrap_or_default();
        assert!(
            message.contains(&request_id)
                && message.contains("rejected by operator-q: not before the audit"),
            "{message}"
        );
        state.stopping.store(true, Ordering::Release);
    }

    /// What Claude Code's `PreToolUse` hook gets for a Bash call, answered as `ai6 green-ink
    /// gate` answers it, through the Run's own Cable endpoint.
    async fn hook(
        environment: &std::collections::BTreeMap<String, String>,
        cwd: &FsPath,
        command: &str,
    ) -> Option<serde_json::Value> {
        use crate::directorates::section_00::gate::{GateClient, pre_tool_use};
        let input = serde_json::json!({
            "session_id": "claude-session",
            "transcript_path": "/tmp/transcript.jsonl",
            "cwd": cwd,
            "permission_mode": "default",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": command, "description": "run it", "timeout": 120000 },
            "tool_use_id": "toolu_01"
        })
        .to_string();
        let client = GateClient::from_environment(environment);
        tokio::task::spawn_blocking(move || pre_tool_use(&input, client.as_ref()))
            .await
            .unwrap()
            .map(|output| serde_json::from_str(&output).unwrap())
    }

    #[tokio::test]
    async fn the_gate_holds_a_protected_push_until_green_ink_covers_it() {
        let root = tempfile::tempdir().unwrap();
        // A quiet Run must not stall while a loaded runner makes the gate's calls.
        let agents = "[agent.007]\nhandler = \"mock-recorder\"\nclearance = \"field\"\n\
                      [agent.007.limits]\nstall_timeout = 300\n";
        let state = test_state(root.path(), "", agents);
        let mission_id = format!("green-ink-gate-{}", std::process::id());
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(&mission_id).unwrap(),
                Some("G".to_owned()),
                "push the release".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        start_mission_by_id(&state, &mission_id).await;
        wait_for_state(&state, &mission_id, MissionState::Blocked).await;
        let environment = state.runs.lock().unwrap()[&mission_id].environment.clone();
        // The hook runs in the Agent's checkout: a repository on `main` with a Mission branch.
        let checkout = tempfile::tempdir().unwrap();
        let path = checkout.path().to_string_lossy().into_owned();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "first",
            ],
            &["branch", "ai6/work"],
        ] {
            let mut full = vec!["-C".to_owned(), path.clone()];
            full.extend(args.iter().map(|arg| (*arg).to_owned()));
            assert!(
                crate::directorates::section_00::gate::local_git(&full)
                    .unwrap()
                    .0,
                "git {full:?}"
            );
        }
        let cwd = checkout.path();

        let held = hook(&environment, cwd, "git push origin main")
            .await
            .expect("a protected push is held");
        let output = &held["hookSpecificOutput"];
        assert_eq!(output["hookEventName"], "PreToolUse");
        assert_eq!(output["permissionDecision"], "deny");
        let reason = output["permissionDecisionReason"].as_str().unwrap();
        assert!(
            reason.contains("is filed") && reason.contains("origin:refs/heads/main"),
            "{reason}"
        );
        assert!(
            hook(&environment, cwd, "git push origin ai6/work")
                .await
                .is_none(),
            "a Mission branch needs no green ink"
        );
        assert!(hook(&environment, cwd, "cargo test").await.is_none());
        let filed = state.intake.pending_approvals("G", now(&state)).unwrap();
        assert_eq!(filed.len(), 1, "the gate files the push it denied");
        assert!(reason.contains(&filed[0].id), "{reason}");

        let answer: serde_json::Value = serde_json::from_str(
            &call_tool(
                &state,
                &mission_id,
                serde_json::json!({
                    "kind": "push",
                    "target": "origin:main",
                    "reason": "release 2.0 is tagged",
                    "idempotency_key": "gate-push",
                }),
            )
            .await,
        )
        .unwrap();
        let request_id = answer["request_id"].as_str().unwrap().to_owned();
        assert_eq!(
            request_id, filed[0].id,
            "the Agent's own request is the one filed"
        );
        assert!(
            hook(&environment, cwd, "git push origin main")
                .await
                .is_some(),
            "a pending request does not cover the step"
        );
        state
            .intake
            .decide_approval(
                "G",
                &request_id,
                &Ruling::approve("operator-m", now(&state), now(&state) + 600.0),
            )
            .unwrap();
        assert_eq!(
            hook(&environment, cwd, "git push --force-with-lease origin main").await,
            None,
            "the approval covers the push"
        );
        assert!(
            hook(&environment, cwd, "git push origin refs/tags/v2.0")
                .await
                .is_some(),
            "and nothing else"
        );

        state
            .intake
            .revoke_approval(
                "G",
                &request_id,
                &crate::directorates::registry::approvals::Revocation::new(
                    "operator-m",
                    now(&state),
                    None,
                ),
            )
            .unwrap();
        assert!(
            hook(&environment, cwd, "git push origin main")
                .await
                .is_some(),
            "a revoked approval covers nothing"
        );
        state.stopping.store(true, Ordering::Release);
    }

    /// Runs one turn of a fake external ACP agent that asks leave to run `command`, with the
    /// gate as its decider, and answers the events the Run produced and the option the
    /// agent was answered with.
    async fn external_turn(
        environment: &std::collections::BTreeMap<String, String>,
        cwd: &FsPath,
        command: &'static str,
    ) -> (Vec<AgentEvent>, String) {
        use crate::directorates::section_00::acp::{AcpOptions, AcpRun, GateDecider};
        use coqui_acp_provider::AgentSession as _;
        let outcome = Arc::new(std::sync::Mutex::new(None));
        let channel = crate::station::acp::testing::command_agent(command, outcome.clone());
        let options = AcpOptions {
            cwd: cwd.to_string_lossy().into_owned(),
            prompt: Some("go".to_owned()),
            decider: Arc::new(GateDecider::from_environment(environment, cwd)),
            ..AcpOptions::default()
        };
        tokio::task::spawn_blocking(move || {
            let run = AcpRun::connect(channel, options, None).unwrap();
            let mut events = Vec::new();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while std::time::Instant::now() < deadline {
                if let Some(event) = run.next_event(100).unwrap() {
                    let done = event.kind == "completed";
                    events.push(event);
                    if done {
                        break;
                    }
                }
            }
            let answered = outcome
                .lock()
                .unwrap()
                .clone()
                .expect("the agent was answered");
            let chosen = answered["outcome"]["optionId"].as_str().unwrap().to_owned();
            (events, chosen)
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn an_external_agents_permission_request_is_judged_by_the_same_gate() {
        let root = tempfile::tempdir().unwrap();
        let agents = "[agent.007]\nhandler = \"mock-recorder\"\nclearance = \"field\"\n\
                      [agent.007.limits]\nstall_timeout = 300\n";
        let state = test_state(root.path(), "", agents);
        let mission_id = format!("green-ink-acp-{}", std::process::id());
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(&mission_id).unwrap(),
                Some("G".to_owned()),
                "push the release".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        start_mission_by_id(&state, &mission_id).await;
        wait_for_state(&state, &mission_id, MissionState::Blocked).await;
        let environment = state.runs.lock().unwrap()[&mission_id].environment.clone();
        let checkout = tempfile::tempdir().unwrap();
        let path = checkout.path().to_string_lossy().into_owned();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "first",
            ],
        ] {
            let mut full = vec!["-C".to_owned(), path.clone()];
            full.extend(args.iter().map(|arg| (*arg).to_owned()));
            assert!(
                crate::directorates::section_00::gate::local_git(&full)
                    .unwrap()
                    .0,
                "git {full:?}"
            );
        }
        let cwd = checkout.path();

        // A protected push is refused, and the refusal is in the Run's events.
        let (events, chosen) = external_turn(&environment, cwd, "git push origin main").await;
        assert_eq!(chosen, "no");
        let refusal = events
            .iter()
            .find(|event| event.kind == "permission")
            .and_then(|event| event.text.as_deref())
            .expect("the refusal is an event");
        assert!(
            refusal.contains("refused") && refusal.contains("is filed"),
            "{refusal}"
        );
        // A command that takes no governed step goes ahead.
        let (events, chosen) = external_turn(&environment, cwd, "cargo test").await;
        assert_eq!(chosen, "yes");
        assert!(events.iter().any(|event| {
            event.kind == "permission" && event.text.as_deref().unwrap().contains("granted")
        }));

        // Once green ink covers the push, the same request is allowed.
        let answer: serde_json::Value = serde_json::from_str(
            &call_tool(
                &state,
                &mission_id,
                serde_json::json!({
                    "kind": "push",
                    "target": "origin:main",
                    "reason": "release 2.0 is tagged",
                    "idempotency_key": "acp-push",
                }),
            )
            .await,
        )
        .unwrap();
        let request_id = answer["request_id"].as_str().unwrap().to_owned();
        state
            .intake
            .decide_approval(
                "G",
                &request_id,
                &Ruling::approve("operator-m", now(&state), now(&state) + 600.0),
            )
            .unwrap();
        let (_, chosen) = external_turn(&environment, cwd, "git push origin main").await;
        assert_eq!(chosen, "yes");
        state.stopping.store(true, Ordering::Release);
    }

    #[tokio::test]
    async fn a_request_keeps_the_links_its_agent_gave_and_refuses_a_bad_one() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = patient_station(root.path(), "links");
        let _stop = StopAtEnd(state.clone());
        start_mission_by_id(&state, &mission_id).await;
        wait_for_state(&state, &mission_id, MissionState::Blocked).await;

        let mut bad = push_main("links-bad");
        bad["links"] = serde_json::json!([{ "label": "PR", "url": "http://a.example/1" }]);
        let refused = call_tool(&state, &mission_id, bad).await;
        assert!(refused.contains("https"), "{refused}");
        assert!(
            state
                .intake
                .pending_approvals("G", now(&state))
                .unwrap()
                .is_empty()
        );

        let mut good = push_main("links-good");
        good["links"] = serde_json::json!([
            { "label": "PR 42", "url": "https://github.com/o/n/pull/42" },
        ]);
        let answer: serde_json::Value =
            serde_json::from_str(&call_tool(&state, &mission_id, good).await).unwrap();
        assert_eq!(answer["status"], "pending", "{answer}");
        let pending = state.intake.pending_approvals("G", now(&state)).unwrap();
        assert_eq!(
            pending[0].links,
            [ApprovalLink::new("PR 42", "https://github.com/o/n/pull/42")]
        );
    }

    #[tokio::test]
    async fn a_rearmed_request_holds_the_mission_and_its_decision_reaches_the_run() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = patient_station(root.path(), "rearm");
        let _stop = StopAtEnd(state.clone());
        start_mission_by_id(&state, &mission_id).await;
        wait_for_state(&state, &mission_id, MissionState::Blocked).await;
        let request = ApprovalRequest::try_new(
            "G",
            MissionId::try_new(&mission_id).unwrap(),
            RunId::try_new("provider-id-remote").unwrap(),
            "007",
            ActionScope::try_new("push", "origin:refs/heads/main").unwrap(),
            "the release is green",
            "rearm-1",
            &Default::default(),
        )
        .unwrap()
        .expiring(now(&state) - 120.0, Duration::from_secs(60));
        state.intake.request_approval(&request).unwrap();

        // It lapses and the Agent is told; its Mission goes on working.
        deliver_decisions(&state).await;
        assert!(is_told(&request.id, "expired without a decision").await);
        state
            .missions
            .lock()
            .unwrap()
            .transition(&mission_id, MissionState::Working)
            .unwrap();

        // What Universal Exports writes when a person re-arms it.
        let at = now(&state);
        state
            .intake
            .rearm_approval(
                "G",
                &request.id,
                &crate::directorates::registry::approvals::Rearming::new("operator-m", at, None),
                at + 600.0,
            )
            .unwrap();
        deliver_decisions(&state).await;
        let waiting = mission(&state, &mission_id);
        assert_eq!(waiting.state, MissionState::Blocked);
        assert_eq!(waiting.blocked_reason.as_deref(), Some(GREEN_INK_REASON));
        assert!(
            is_told(&request.id, "re-armed by operator-m").await,
            "the Run hears it waits again"
        );
        assert!(
            state.intake.pending_approvals("G", at).unwrap()[0]
                .rearmed
                .as_ref()
                .unwrap()
                .told_at
                .is_some()
        );

        // Told once, whatever the Agent does next.
        state
            .missions
            .lock()
            .unwrap()
            .transition(&mission_id, MissionState::Working)
            .unwrap();
        deliver_decisions(&state).await;
        assert_eq!(mission(&state, &mission_id).state, MissionState::Working);

        state
            .intake
            .decide_approval(
                "G",
                &request.id,
                &Ruling::approve("operator-m", at + 1.0, at + 60.0),
            )
            .unwrap();
        deliver_decisions(&state).await;
        assert!(is_told(&request.id, "approved by operator-m").await);
    }

    #[tokio::test]
    async fn an_expired_request_is_told_and_audited_without_its_reason() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = station(root.path(), "expire");
        start_mission_by_id(&state, &mission_id).await;
        wait_for_state(&state, &mission_id, MissionState::Blocked).await;
        let request = ApprovalRequest::try_new(
            "G",
            MissionId::try_new(&mission_id).unwrap(),
            RunId::try_new("provider-id-remote").unwrap(),
            "007",
            ActionScope::try_new("push", "origin:refs/heads/main").unwrap(),
            "a reason only the operator reads",
            "expire-1",
            &Default::default(),
        )
        .unwrap()
        .expiring(now(&state) - 120.0, Duration::from_secs(60));
        state.intake.request_approval(&request).unwrap();

        deliver_decisions(&state).await;
        let told = told_about(&request.id).await;
        assert!(
            told.iter()
                .any(|(_, text)| text.contains("expired without a decision")),
            "{told:?}"
        );
        let expired = state
            .intake
            .audit_records()
            .unwrap()
            .into_iter()
            .find(|record| record.action == "green_ink.expired")
            .expect("the expiry is audited");
        assert_eq!(expired.actor, crate::EXPIRY_SIGNER);
        assert_eq!(
            expired.target,
            format!("{} push origin:refs/heads/main", request.id)
        );
        state.stopping.store(true, Ordering::Release);
    }

    fn asked(mission_id: &str, agent: &str, kind: &str, target: &str) -> Asked {
        Asked {
            operation: "G".to_owned(),
            mission: mission_id.to_owned(),
            run: "provider-id-direct".to_owned(),
            agent: agent.to_owned(),
            kind: kind.to_owned(),
            target: target.to_owned(),
            reason: "routine".to_owned(),
            links: Vec::new(),
            idempotency_key: format!("{kind}-{target}"),
            exposed: false,
        }
    }

    #[test]
    fn a_class_authorisation_approves_at_once_unless_the_run_is_exposed() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = station(root.path(), "class");
        let class = ClassAuthorization::new(
            "G",
            vec!["push".to_owned()],
            "origin:refs/heads/release/*",
            "operator-m",
            now(&state),
            now(&state) + 3600.0,
        )
        .for_agents(vec!["007".to_owned()]);
        state.intake.put_class_authorization(&class).unwrap();
        let desk = Desk::new(state.clone());

        let covered = desk
            .request(asked(
                &mission_id,
                "007",
                "push",
                "origin:refs/heads/release/2",
            ))
            .unwrap();
        assert_eq!(covered.status, "approved");
        assert_eq!(
            mission(&state, &mission_id).state,
            MissionState::Queued,
            "an approved request does not hold the Mission"
        );
        assert_eq!(state.intake.class_authorizations("G").unwrap()[0].uses, 1);
        let audit = state.intake.audit_records().unwrap();
        assert!(
            audit
                .iter()
                .any(|record| record.action == "class_authorization.used"
                    && record.actor == format!("class:{}", class.id)),
            "{audit:?}"
        );

        let mut exposed = asked(&mission_id, "007", "push", "origin:refs/heads/release/3");
        exposed.exposed = true;
        assert_eq!(desk.request(exposed).unwrap().status, "pending");
        assert_eq!(state.intake.class_authorizations("G").unwrap()[0].uses, 1);
    }

    #[test]
    fn an_agent_naming_its_own_gate_program_is_a_weakening() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(
            root.path(),
            "",
            "[agent.007]\nhandler = \"mock-recorder\"\n[agent.007.env]\nAI6_GREEN_INK_GATE = \"/opt/gate\"\n\
             [agent.008]\nhandler = \"mock-recorder\"\n",
        );
        let weakened = weakenings(&state.roster);
        assert_eq!(weakened.len(), 1, "{weakened:?}");
        assert!(
            weakened[0].contains("\"007\"") && weakened[0].contains("/opt/gate"),
            "{weakened:?}"
        );
    }

    #[test]
    fn one_step_named_two_ways_is_one_request_and_a_rejection_stands_until_reopened() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = station(root.path(), "dedupe");
        let desk = Desk::new(state.clone());
        let first = desk
            .request(asked(&mission_id, "007", "release", "tag v1.2.0"))
            .unwrap();
        assert_eq!(first.status, "pending");
        let mut drifted = asked(&mission_id, "007", "release", "origin:refs/tags/v1.2.0");
        drifted.idempotency_key = "another-window".to_owned();
        drifted.run = "provider-id-later".to_owned();
        let again = desk.request(drifted).unwrap();
        assert_eq!(again.status, "pending");
        assert_eq!(again.request_id, first.request_id);
        assert_eq!(
            state
                .intake
                .pending_approvals("G", now(&state))
                .unwrap()
                .len(),
            1,
            "no second row"
        );
        let other = desk
            .request(asked(&mission_id, "007", "release", "tag v1.2.1"))
            .unwrap();
        assert_ne!(
            other.request_id, first.request_id,
            "another tag is another step"
        );

        let id = first.request_id.unwrap();
        state
            .intake
            .decide_approval(
                "G",
                &id,
                &Ruling::reject("operator-m", now(&state), Some("no such tag".to_owned())),
            )
            .unwrap();
        let mut refile = asked(&mission_id, "007", "release", "refs/tags/v1.2.0");
        refile.idempotency_key = "after-rejection".to_owned();
        let refused = desk.request(refile).unwrap();
        assert_eq!(refused.status, "rejected");
        assert_eq!(refused.request_id.as_deref(), Some(id.as_str()));
        let said = refused.message.unwrap();
        assert!(
            said.contains("no such tag") && said.contains("reopens"),
            "{said}"
        );
        assert_eq!(
            state
                .intake
                .pending_approvals("G", now(&state))
                .unwrap()
                .len(),
            1,
            "only the other tag waits"
        );

        let at = now(&state) + 1.0;
        state
            .intake
            .rearm_approval(
                "G",
                &id,
                &crate::directorates::registry::approvals::Rearming::new("operator-m", at, None),
                at + 600.0,
            )
            .unwrap();
        let reopened = desk
            .request(asked(&mission_id, "007", "release", "tag v1.2.0"))
            .unwrap();
        assert_eq!(reopened.status, "pending");
        assert_eq!(reopened.request_id.as_deref(), Some(id.as_str()));
    }

    #[test]
    fn the_policy_denies_and_parks_the_steps_it_lists() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(
            root.path(),
            "[operation.G.green_ink]\ndeny = [\"release refs/tags/v0.*\"]\npark = [\"merge PR 7\"]\n\
             [operation.G.green_ink.unattended]\nfield = [\"merge\"]\n",
            AGENTS,
        );
        let mission_id = format!("green-ink-rules-{}", std::process::id());
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(&mission_id).unwrap(),
                Some("G".to_owned()),
                "release the work".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        let desk = Desk::new(state.clone());
        let denied = desk
            .request(asked(&mission_id, "007", "release", "tag v0.3.0"))
            .unwrap_err();
        assert!(
            denied.contains("denies") && denied.contains("v0.*"),
            "{denied}"
        );
        let checked = desk
            .check(Checked {
                operation: "G".to_owned(),
                mission: mission_id.clone(),
                run: Some("provider-id-direct".to_owned()),
                exposed: false,
                agent: "007".to_owned(),
                steps: vec![("release".to_owned(), "refs/tags/v0.3.0".to_owned())],
            })
            .unwrap();
        assert_eq!(checked[0].status, "refused");
        assert!(
            state
                .intake
                .pending_approvals("G", now(&state))
                .unwrap()
                .is_empty(),
            "a denied step files nothing"
        );

        assert_eq!(
            desk.request(asked(&mission_id, "007", "merge", "PR 8"))
                .unwrap()
                .status,
            "not_required",
            "unattended for field"
        );
        assert_eq!(
            desk.request(asked(&mission_id, "007", "merge", "PR 7"))
                .unwrap()
                .status,
            "pending",
            "a parked step waits whatever the policy lets through"
        );
    }

    #[test]
    fn a_gate_check_that_errors_is_journaled_and_names_why() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = station(root.path(), "errors");
        let desk = Desk::new(state);
        let error = desk
            .check(Checked {
                operation: "Nowhere".to_owned(),
                mission: mission_id.clone(),
                run: None,
                exposed: false,
                agent: "007".to_owned(),
                steps: vec![("push".to_owned(), "origin:refs/heads/errors".to_owned())],
            })
            .unwrap_err();
        assert!(error.contains("not configured"), "{error}");
        let lines: Vec<String> = JOURNAL
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.contains("origin:refs/heads/errors"))
            .cloned()
            .collect();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("error: the Operation is not configured"),
            "{lines:?}"
        );
    }

    #[test]
    fn a_denied_step_is_filed_or_refused_naming_why_it_was_not() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = station(root.path(), "denied");
        let desk = Desk::new(state.clone());
        let check = |run: Option<&str>| {
            desk.check(Checked {
                operation: "G".to_owned(),
                mission: mission_id.clone(),
                run: run.map(str::to_owned),
                exposed: false,
                agent: "007".to_owned(),
                steps: vec![("push".to_owned(), "origin:refs/heads/main".to_owned())],
            })
            .unwrap()
            .remove(0)
        };
        // The Mission has no Run on record, so nothing can be filed under one: loud, not silent.
        let unfiled = check(None);
        assert_eq!(unfiled.status, "refused");
        assert!(
            unfiled
                .message
                .as_deref()
                .unwrap()
                .contains("could not be filed"),
            "{unfiled:?}"
        );
        assert!(
            state
                .intake
                .pending_approvals("G", now(&state))
                .unwrap()
                .is_empty()
        );

        let filed = check(Some("provider-id-direct"));
        assert_eq!(filed.status, "needs_approval");
        let id = filed
            .request_id
            .clone()
            .expect("the filed request is named");
        let pending = state.intake.pending_approvals("G", now(&state)).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);
        let again = check(Some("provider-id-later"));
        assert_eq!(
            again.request_id.as_deref(),
            Some(id.as_str()),
            "one request"
        );
        assert_eq!(
            state
                .intake
                .pending_approvals("G", now(&state))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn an_approval_decided_after_its_run_settled_is_spent_by_the_next_window_or_reported() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = station(root.path(), "outlive");
        let desk = Desk::new(state.clone());
        let at = now(&state);
        let mut first = asked(&mission_id, "007", "merge", "PR 11");
        first.run = "provider-id-window-1".to_owned();
        let one = desk.request(first).unwrap().request_id.unwrap();
        let mut second = asked(&mission_id, "007", "merge", "PR 12");
        second.run = "provider-id-window-1".to_owned();
        let two = desk.request(second).unwrap().request_id.unwrap();

        // Window 1 is over. Its asks are decided later: one lives, one lapsed meanwhile.
        state
            .intake
            .decide_approval("G", &one, &Ruling::approve("operator-m", at, at + 3600.0))
            .unwrap();
        state
            .intake
            .decide_approval(
                "G",
                &two,
                &Ruling::approve("operator-m", at - 100.0, at - 10.0),
            )
            .unwrap();

        // Window 2 of the same Mission finds the live approval covering its step.
        let next = |target: &str| {
            desk.check(Checked {
                operation: "G".to_owned(),
                mission: mission_id.clone(),
                run: Some("provider-id-window-2".to_owned()),
                exposed: false,
                agent: "007".to_owned(),
                steps: vec![("merge".to_owned(), target.to_owned())],
            })
            .unwrap()
            .remove(0)
        };
        let spent = next("PR 11");
        assert_eq!(spent.status, "approved");
        assert_eq!(spent.request_id.as_deref(), Some(one.as_str()));

        // The one nobody spent before it lapsed reaches a person; the used one does not.
        report_unspent_since(&state, 0.0, at);
        let reports = state
            .intake
            .reports(
                &crate::directorates::registry::reports::ReportFilter::all().of_kind("green_ink"),
            )
            .unwrap();
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(reports[0].title.contains("PR 12"), "{}", reports[0].title);
        report_unspent_since(&state, 0.0, at);
        assert_eq!(
            state
                .intake
                .reports(
                    &crate::directorates::registry::reports::ReportFilter::all()
                        .of_kind("green_ink")
                )
                .unwrap()
                .len(),
            1,
            "a sweep seen twice files once"
        );
        let used = state.intake.approval("G", &one, at).unwrap().unwrap();
        let ApprovalStatus::Decided(used) = used else {
            panic!("decided")
        };
        assert!(used.used_at.is_some());
    }

    #[test]
    fn the_policy_answers_before_anything_is_stored() {
        let root = tempfile::tempdir().unwrap();
        let (state, mission_id) = station(root.path(), "policy");
        let desk = Desk::new(state.clone());
        let own_branch = desk
            .request(asked(&mission_id, "007", "push", "origin:ai6/work"))
            .unwrap();
        assert_eq!(own_branch.status, "not_required");
        let desk_agent = desk
            .request(asked(&mission_id, "desk", "push", "origin:main"))
            .unwrap_err();
        assert!(
            desk_agent.contains("desk Agent may not ask"),
            "{desk_agent}"
        );
        let unknown = desk
            .request(asked(&mission_id, "007", "deploy", "production"))
            .unwrap_err();
        assert!(unknown.contains("push"), "{unknown}");
        assert!(
            state
                .intake
                .pending_approvals("G", now(&state))
                .unwrap()
                .is_empty()
        );
    }
}
