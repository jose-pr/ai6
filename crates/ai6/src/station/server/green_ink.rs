//! The Station's side of green ink: an Agent's request reaches the Registry, its Mission
//! waits with the Watchdog paused, and the decision goes back to the Agent.
//!
//! Decisions are made elsewhere (Universal Exports writes them to the Registry), so the
//! scheduler looks for them: a decision reaches a live Session as a message, or resumes a
//! Session that ended its turn while it waited. A request nobody decides expires as refused.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;
use tokio::time::Instant;

use super::{
    DaemonState, ResumeMission, SCHEDULER_RETRY_DELAY, publish_lifecycle, queue_event,
    resume_mission, sync_session_record,
};
use crate::directorates::m::approvals::{ActionScope, GREEN_INK_REASON, Requirement};
use crate::directorates::m::mission::{Mission, MissionState};
use crate::directorates::m::roster::Roster;
use crate::directorates::registry::agent_records::AgentOutcome;
use crate::directorates::registry::approvals::{
    Approval, ApprovalRequest, ApprovalStatus, Decision,
};
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::directorates::section_00::agent::{AgentEvent, AgentMessage, AgentSession};
use crate::{MissionId, SessionId};

/// The event kind that reports a request waiting for green ink, and its decision.
pub(super) const GREEN_INK_EVENT: &str = "green_ink";

/// The timeline and event kind for green ink.
const TIMELINE_KIND: &str = "green_ink";

/// What an Agent asks for through `request_approval`.
pub(super) struct Asked {
    pub(super) operation: String,
    pub(super) mission: String,
    pub(super) session: String,
    pub(super) agent: String,
    pub(super) kind: String,
    pub(super) target: String,
    pub(super) reason: String,
    pub(super) idempotency_key: String,
    /// Whether the Session's exposure keeps it from class authorisations.
    pub(super) exposed: bool,
}

/// What `request_approval` answers.
#[derive(Debug, Serialize)]
pub(super) struct Answer {
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    /// `approved`, `pending`, `rejected`, `expired` or `not_required`.
    pub(super) status: &'static str,
    /// When an approval stops covering the step, or when a pending request expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// A Session's way to ask its Station for green ink.
#[derive(Clone)]
pub(super) struct Desk {
    state: DaemonState,
}

impl Desk {
    pub(super) fn new(state: DaemonState) -> Self {
        Self { state }
    }

    /// Records a request and answers at once: approved by a class authorisation, not
    /// required by the Operation's policy, or pending while the Mission waits for a person.
    ///
    /// # Errors
    ///
    /// A message the Agent may read: the step is out of bounds, the Agent may not ask, or the
    /// Registry could not be written.
    pub(super) fn request(&self, asked: Asked) -> Result<Answer, String> {
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
        let clearance = state
            .roster
            .agent(Some(&asked.operation), &asked.agent)
            .map_err(|_| "the Agent is not configured for this Operation".to_owned())?
            .clearance;
        let scope = ActionScope::try_new(&asked.kind, &asked.target).map_err(|e| e.to_string())?;
        let policy = &operation.green_ink;
        match policy.requirement(&clearance, &scope) {
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
            SessionId::try_new(&asked.session).map_err(|e| e.to_string())?,
            &asked.agent,
            scope,
            asked.reason,
            &asked.idempotency_key,
        )
        .map_err(|e| e.to_string())?
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
            return Ok(answer(&status));
        }
        if !asked.exposed
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
                usage: None,
                tools: Vec::new(),
                quota: Vec::new(),
            },
        );
        hold(state, &asked.mission);
        Ok(answer(&status))
    }
}

/// What a harness gate asks before a command runs: may this Agent take these steps now?
pub(super) struct Checked {
    pub(super) operation: String,
    pub(super) mission: String,
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
    /// Answers, for each step, whether the Agent may take it now. Nothing is recorded: the
    /// gate refuses what is not covered and tells the Agent to ask with `request_approval`.
    ///
    /// # Errors
    ///
    /// A message the gate passes on: the Operation, Mission or Agent is unknown here, or the
    /// Registry could not be read.
    pub(super) fn check(&self, checked: Checked) -> Result<Vec<StepAnswer>, String> {
        let state = &self.state;
        let now = state.clock.now() as f64;
        let operation = state
            .roster
            .operation(&checked.operation)
            .map_err(|_| "the Operation is not configured on this Station".to_owned())?;
        let clearance = state
            .roster
            .agent(Some(&checked.operation), &checked.agent)
            .map_err(|_| "the Agent is not configured for this Operation".to_owned())?
            .clearance;
        let policy = &operation.green_ink;
        let mut answers = Vec::new();
        for (kind, target) in checked.steps {
            let scope = ActionScope::try_new(&kind, &target).map_err(|e| e.to_string())?;
            let (status, request_id, message) = match policy.requirement(&clearance, &scope) {
                Requirement::NotRequired => ("not_required", None, None),
                Requirement::GreenInk => match state
                    .intake
                    .live_approval(&checked.operation, &checked.mission, &scope, now)
                    .map_err(|error| format!("green ink could not be checked: {error}"))?
                {
                    Some(approval) => ("approved", Some(approval.request.id), None),
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
    if let Ok(sessions) = state.sessions.lock()
        && let Some(operation) = sessions.get(mission_id)
    {
        operation.watchdog.pause();
    }
    if blocked {
        sync_session_record(state, mission_id);
        publish_lifecycle(state, mission_id, MissionState::Blocked);
    }
}

/// Whether a Mission waits for green ink.
fn waits(mission: &Mission) -> bool {
    mission.state == MissionState::Blocked
        && mission.blocked_reason.as_deref() == Some(GREEN_INK_REASON)
}

/// Ends a Session whose turn completed while its Mission waits for green ink, keeping the
/// Mission blocked so the decision resumes it. `false` when the Mission does not wait.
pub(super) fn end_turn_waiting(
    state: &DaemonState,
    mission_id: &str,
    session: &Arc<std::sync::Mutex<guatiao::library::Object<dyn AgentSession>>>,
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
    if !waits(&mission) || !pending {
        return false;
    }
    let operation = state.sessions.lock().ok().and_then(|mut sessions| {
        sessions
            .get(mission_id)
            .is_some_and(|operation| Arc::ptr_eq(&operation.session, session))
            .then(|| sessions.remove(mission_id))
            .flatten()
            .inspect(|operation| operation.gate.close())
    });
    if let Some(operation) = operation
        && let Ok(session) = operation.session.lock()
    {
        let _ = session.close();
    }
    sync_session_record(state, mission_id);
    true
}

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
        let untold = statuses
            .into_iter()
            .filter_map(|status| match status {
                ApprovalStatus::Decided(approval) if approval.notified_at.is_none() => {
                    Some(approval)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let live = state.sessions.lock().ok().and_then(|sessions| {
            sessions
                .get(mission_id)
                .map(|operation| (operation.session.clone(), operation.gate.clone()))
        });
        match live {
            Some((session, gate)) => {
                for approval in &untold {
                    let message = AgentMessage {
                        text: decision_text(approval, now),
                    };
                    let session = session.clone();
                    let gate = gate.clone();
                    let sent = tokio::task::spawn_blocking(move || {
                        gate.lock(&session)
                            .ok()
                            .is_some_and(|session| session.send_message(message).is_ok())
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

/// Moves a Mission whose live Session no longer waits back to work.
fn release(state: &DaemonState, mission_id: &str) {
    let working = state.missions.lock().ok().is_some_and(|mut missions| {
        missions
            .transition(mission_id, MissionState::Working)
            .is_ok()
    });
    if let Ok(sessions) = state.sessions.lock()
        && let Some(operation) = sessions.get(mission_id)
    {
        operation.watchdog.resume();
    }
    if working {
        sync_session_record(state, mission_id);
        publish_lifecycle(state, mission_id, MissionState::Working);
    }
}

/// Resumes a Session that ended its turn while it waited, with the decisions as its message.
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
                    Instant::now() + SCHEDULER_RETRY_DELAY,
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
            usage: None,
            tools: Vec::new(),
            quota: Vec::new(),
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
    .session(request.session.to_string())
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

    /// 007 runs on the recording mock, which reports its Session blocked at once and then stays
    /// quiet; its Watchdog stalls a quiet Session after 5 seconds, long enough for a
    /// loaded Windows runner to make the MCP tool call before it.
    const AGENTS: &str = "[agent.007]\nhandler = \"mock-recorder\"\nclearance = \"field\"\n\
                          [agent.007.limits]\nstall_timeout = 5\n\
                          [agent.desk]\nhandler = \"mock-recorder\"\nclearance = \"desk\"\n";

    /// A Station with one queued Mission of a unique ID for Agent 007 of Operation G.
    fn station(root: &FsPath, name: &str) -> (DaemonState, String) {
        let state = test_state(root, "", AGENTS);
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
        let environment = state.sessions.lock().unwrap()[mission_id]
            .environment
            .clone();
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

    fn told_about(request_id: &str) -> Vec<(String, String)> {
        RECORDED_MESSAGES
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, text)| text.contains(request_id))
            .cloned()
            .collect()
    }

    #[tokio::test]
    async fn a_session_asks_waits_past_the_watchdog_and_resumes_working_once_approved() {
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
        let watchdog = state.sessions.lock().unwrap()[&mission_id].watchdog.clone();
        assert!(watchdog.is_paused());

        // More than the stall timeout passes: waiting for a person is not a stall.
        tokio::time::sleep(Duration::from_millis(5_500)).await;
        assert_eq!(mission(&state, &mission_id).state, MissionState::Blocked);
        let pending = state.intake.pending_approvals("G", now(&state)).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].agent, "007");
        assert_eq!(pending[0].session.as_str(), "provider-session-remote");

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

        let told = told_about(&request_id);
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(told[0].0, "provider-session-remote");
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
        assert_eq!(told_about(&request_id).len(), 1, "a decision is told once");
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
        let session = state.sessions.lock().unwrap()[&mission_id].session.clone();

        assert!(end_turn_waiting(&state, &mission_id, &session));
        assert!(!state.sessions.lock().unwrap().contains_key(&mission_id));
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
            .expect("the decision resumes the Session")
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
    /// gate` answers it, through the Session's own Cable endpoint.
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
        // A quiet Session must not stall while a loaded runner makes the gate's calls.
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
        let environment = state.sessions.lock().unwrap()[&mission_id]
            .environment
            .clone();
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
            reason.contains("request_approval") && reason.contains("origin:refs/heads/main"),
            "{reason}"
        );
        assert!(
            hook(&environment, cwd, "git push origin ai6/work")
                .await
                .is_none(),
            "a Mission branch needs no green ink"
        );
        assert!(hook(&environment, cwd, "cargo test").await.is_none());
        assert!(
            state
                .intake
                .pending_approvals("G", now(&state))
                .unwrap()
                .is_empty(),
            "the gate asks nobody; the Agent does"
        );

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
            .revoke_approval("G", &request_id, "operator-m", now(&state))
            .unwrap();
        assert!(
            hook(&environment, cwd, "git push origin main")
                .await
                .is_some(),
            "a revoked approval covers nothing"
        );
        state.stopping.store(true, Ordering::Release);
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
            SessionId::try_new("provider-session-remote").unwrap(),
            "007",
            ActionScope::try_new("push", "origin:refs/heads/main").unwrap(),
            "a reason only the operator reads",
            "expire-1",
        )
        .unwrap()
        .expiring(now(&state) - 120.0, Duration::from_secs(60));
        state.intake.request_approval(&request).unwrap();

        deliver_decisions(&state).await;
        let told = told_about(&request.id);
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
            session: "provider-session-direct".to_owned(),
            agent: agent.to_owned(),
            kind: kind.to_owned(),
            target: target.to_owned(),
            reason: "routine".to_owned(),
            idempotency_key: format!("{kind}-{target}"),
            exposed: false,
        }
    }

    #[test]
    fn a_class_authorisation_approves_at_once_unless_the_session_is_exposed() {
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
