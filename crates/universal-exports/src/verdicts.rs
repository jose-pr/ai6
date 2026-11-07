//! A person's verdict on a Mission's outcome: the latest attempt that ended done or held is marked
//! confirmed, disputed or failed verification, under `mission.verify`.
//!
//! The scorecard reads it from the Session record; a Station settles a held Mission by it. A
//! verdict is audited with its word and never its note, and goes on the Operation's timeline.
//!
//! A Mission whose Session ended done is `verifying`, and nothing but a decision settles it: the
//! same action asks the Station to probe what it was to deliver (`probe`) and decides it (`verify`:
//! done, redispatch as the next attempt, or failed), each with its reason.

use ai6::{
    AuditRecord, MissionVerdict, OutcomeVerdict, SessionError, SessionRecord, TimelineEvent,
};
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memories::{self, Refused};
use crate::{
    ApiError, Gateway, authorize_operation_action, bearer, ensure_operation_registered,
    named_mission, resolve_user, unix_timestamp,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerdictRequest {
    /// `confirmed`, `disputed` or `failed-verification`.
    verdict: String,
    /// Why, for the record.
    note: Option<String>,
}

/// A verdict as recorded, with the attempt it is on.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GivenVerdict {
    /// The Operation.
    pub operation_id: String,
    /// The Mission's UID.
    pub mission_id: String,
    /// The attempt it is on.
    pub attempt: u32,
    /// AI6's Session ID of that attempt.
    pub session_id: String,
    /// What was found, by whom and when.
    #[serde(flatten)]
    pub verdict: MissionVerdict,
}

fn refused_by(error: SessionError) -> Refused {
    match error {
        SessionError::NotFound => ApiError::not_found().into(),
        SessionError::Unsettled(_) => memories::refused(StatusCode::CONFLICT, error.to_string()),
        SessionError::Io(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
            memories::refused(StatusCode::BAD_REQUEST, error.to_string())
        }
        _ => ApiError::internal().into(),
    }
}

/// `POST /v1/user/operations/{operation_id}/missions/{mission_id}/verdict` with
/// `{"verdict", "note"?}`: `mission.verify`. A later verdict replaces it. A refusal is
/// `{"error": TEXT}`: 409 when the latest attempt has not ended done or held.
pub(crate) async fn give(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
    Json(request): Json<VerdictRequest>,
) -> Result<Json<GivenVerdict>, Refused> {
    let word = OutcomeVerdict::from_name(&request.verdict)
        .filter(|word| *word != OutcomeVerdict::Redispatch)
        .ok_or_else(|| {
            memories::refused(
                StatusCode::BAD_REQUEST,
                "a verdict is confirmed, disputed or failed-verification".to_owned(),
            )
        })?;
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation, "mission.verify").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation).await?;
    let uid = named_mission(&gateway, &mission_id).await?.mission_id;
    let at = unix_timestamp()?;
    let verdict = MissionVerdict::new(word, person.id.clone(), at, request.note);
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: "mission.verdict".to_owned(),
        operation: operation.to_string(),
        resource: Some(format!("mission:{uid}")),
        target: format!("{uid} {}", word.as_str()),
        at,
    };
    let (key, recorded) = (operation.to_string(), verdict.clone());
    let store = registry.clone();
    let record = tokio::task::spawn_blocking(move || {
        // Audited first: a verdict that cannot be audited is not given.
        check_settled(&store.latest_session(&uid)?, &key)?;
        store.record_audit(&audit)?;
        store.record_verdict(&key, &uid, &recorded)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(refused_by)?;
    let event = TimelineEvent::new("verdict", word.as_str(), operation.to_string(), at)
        .mission(record.mission_id.clone())
        .session(record.session_id.clone())
        .actor(person.id);
    // A timeline entry that fails is left out; the audit record and the verdict stand.
    let _ = tokio::task::spawn_blocking(move || registry.record_event(&event)).await;
    Ok(Json(GivenVerdict {
        operation_id: operation.to_string(),
        mission_id: record.mission_id,
        attempt: record.attempt,
        session_id: record.session_id,
        verdict,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifyRequest {
    /// `done`, `redispatch` or `failed`.
    decision: String,
    /// Why, for the record and for the next attempt when it is redispatched.
    reason: String,
    /// For a redispatch, what the next attempt is told beyond its brief and the reason.
    amendment: Option<String>,
}

/// A decision on a verifying Mission as recorded, with the attempt it is on.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GivenDecision {
    /// The Operation.
    pub operation_id: String,
    /// The Mission's UID.
    pub mission_id: String,
    /// The attempt it is on.
    pub attempt: u32,
    /// AI6's Session ID of that attempt.
    pub session_id: String,
    /// `done`, `redispatch` or `failed`.
    pub decision: String,
    /// Why, by whom and when.
    #[serde(flatten)]
    pub verdict: MissionVerdict,
}

/// `POST /v1/user/operations/{operation_id}/missions/{mission_id}/verify` with
/// `{"decision", "reason", "amendment"?}`: `mission.verify`. It decides a Mission whose latest
/// attempt is verifying; its Station then settles it done or failed, or queues the next attempt.
/// 409 when the latest attempt is not verifying.
pub(crate) async fn verify(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
    Json(request): Json<VerifyRequest>,
) -> Result<Json<GivenDecision>, Refused> {
    let word = match request.decision.as_str() {
        "done" => OutcomeVerdict::Confirmed,
        "failed" => OutcomeVerdict::FailedVerification,
        "redispatch" => OutcomeVerdict::Redispatch,
        _ => {
            return Err(memories::refused(
                StatusCode::BAD_REQUEST,
                "a decision is done, redispatch or failed".to_owned(),
            ));
        }
    };
    if request.reason.trim().is_empty()
        || (request.amendment.is_some() && word != OutcomeVerdict::Redispatch)
    {
        return Err(memories::refused(
            StatusCode::BAD_REQUEST,
            "a decision needs its reason, and only a redispatch takes an amendment".to_owned(),
        ));
    }
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation, "mission.verify").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation).await?;
    let uid = named_mission(&gateway, &mission_id).await?.mission_id;
    let at = unix_timestamp()?;
    let verdict = MissionVerdict::new(word, person.id.clone(), at, Some(request.reason))
        .with_amendment(request.amendment);
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: "mission.verify".to_owned(),
        operation: operation.to_string(),
        resource: Some(format!("mission:{uid}")),
        target: format!("{uid} {}", request.decision),
        at,
    };
    let (key, recorded) = (operation.to_string(), verdict.clone());
    let store = registry.clone();
    let record = tokio::task::spawn_blocking(move || {
        check_verifying(&store.latest_session(&uid)?, &key)?;
        // Audited first: a decision that cannot be audited is not given.
        store.record_audit(&audit)?;
        store.record_verdict(&key, &uid, &recorded)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(refused_by)?;
    let event = TimelineEvent::new(
        "verify",
        request.decision.clone(),
        operation.to_string(),
        at,
    )
    .mission(record.mission_id.clone())
    .session(record.session_id.clone())
    .actor(person.id);
    // A timeline entry that fails is left out; the audit record and the decision stand.
    let _ = tokio::task::spawn_blocking(move || registry.record_event(&event)).await;
    Ok(Json(GivenDecision {
        operation_id: operation.to_string(),
        mission_id: record.mission_id,
        attempt: record.attempt,
        session_id: record.session_id,
        decision: request.decision,
        verdict,
    }))
}

/// A request for probes as recorded.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeRequested {
    /// The Operation.
    pub operation_id: String,
    /// The Mission's UID.
    pub mission_id: String,
    /// The attempt whose deliverables will be probed.
    pub attempt: u32,
    /// AI6's Session ID of that attempt.
    pub session_id: String,
    /// When it was asked for, as Unix seconds.
    pub requested_at: f64,
}

/// `POST /v1/user/operations/{operation_id}/missions/{mission_id}/probe`: `mission.verify`. Asks
/// the Station holding a verifying Mission to read back what it was to deliver; the findings
/// appear as the latest attempt's `evidence` in the Mission's detail, and `probe_requested`
/// clears when they have. 409 when the latest attempt is not verifying.
pub(crate) async fn probe(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
) -> Result<Json<ProbeRequested>, Refused> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation, "mission.verify").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation).await?;
    let uid = named_mission(&gateway, &mission_id).await?.mission_id;
    let at = unix_timestamp()?;
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: "mission.probe".to_owned(),
        operation: operation.to_string(),
        resource: Some(format!("mission:{uid}")),
        target: uid.clone(),
        at,
    };
    let key = operation.to_string();
    let record = tokio::task::spawn_blocking(move || {
        check_verifying(&registry.latest_session(&uid)?, &key)?;
        registry.record_audit(&audit)?;
        registry.request_probe(&key, &uid, at)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(refused_by)?;
    Ok(Json(ProbeRequested {
        operation_id: operation.to_string(),
        mission_id: record.mission_id,
        attempt: record.attempt,
        session_id: record.session_id,
        requested_at: at,
    }))
}

/// Why `latest`, the latest attempt at a Mission, is not one `operation` may decide or probe: it
/// is not the Operation's, or not verifying.
fn check_verifying(latest: &Option<SessionRecord>, operation: &str) -> Result<(), SessionError> {
    match latest {
        Some(record) if record.operation.as_deref() == Some(operation) => {
            let state = record
                .result
                .as_ref()
                .map_or("unrecorded", |result| result.state.as_str());
            if state == "verifying" && !record.status.is_closed() {
                Ok(())
            } else {
                Err(SessionError::Unsettled(state.to_owned()))
            }
        }
        _ => Err(SessionError::NotFound),
    }
}

/// Why `latest`, the latest attempt at a Mission, cannot take a verdict in `operation`.
fn check_settled(latest: &Option<SessionRecord>, operation: &str) -> Result<(), SessionError> {
    match latest {
        Some(record) if record.operation.as_deref() == Some(operation) => record.takes_verdict(),
        _ => Err(SessionError::NotFound),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ai6::directorates::m::scorecard::{self, Judgment, ScorecardPolicy};
    use ai6::directorates::registry::backend::*;
    use ai6::{NewSession, SessionLaunch, SessionResult, SessionUpdate};

    use crate::controls::tests::{Served, served_with};

    const ROLES: &str = r#"
        [role.viewer]
        actions = ["mission.read", "mission.control", "mission.create", "approval.decide"]

        [role.checker]
        actions = ["mission.read", "mission.verify", "audit.read"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "viewer"

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "checker"
    "#;

    fn ended(served: &Served, mission: &str, state: &str) {
        let opened = served
            .registry
            .open_session(
                NewSession::new(format!("{mission}-1"), mission, "worker")
                    .with_operation("G")
                    .with_launch(
                        SessionLaunch::new("claude", "pipe", "/w")
                            .with_agent(Some("007".to_owned())),
                    ),
                Duration::from_secs(60),
            )
            .unwrap();
        served
            .registry
            .update_session(
                &opened.session_id,
                "worker",
                opened.fence,
                SessionUpdate::default().result(SessionResult::new(state)),
            )
            .unwrap();
    }

    async fn verdict(
        served: &Served,
        token: &str,
        mission: &str,
        body: serde_json::Value,
    ) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!(
                "{}/v1/user/operations/G/missions/{mission}/verdict",
                served.base
            ))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_verdict_changes_the_scorecard_and_is_audited() {
        let served = served_with(ROLES).await;
        ended(&served, "m-done", "done");
        let card = |served: &Served| {
            scorecard::from_registry(
                &served.registry,
                "G",
                &ScorecardPolicy::default(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs_f64()
                    + 1.0,
            )
            .unwrap()
        };
        assert_eq!(card(&served).agents[0].standing.failure_rate, Some(0.0));

        let given = verdict(
            &served,
            &served.operator,
            "m-done",
            serde_json::json!({
                "verdict": "failed-verification",
                "note": "the tag was never pushed",
            }),
        )
        .await;
        assert_eq!(given.status(), 200);
        let given: serde_json::Value = given.json().await.unwrap();
        assert_eq!(given["mission_id"], "m-done");
        assert_eq!(given["attempt"], 1);
        assert_eq!(given["verdict"], "failed-verification");
        assert_eq!(given["by"], "operator-007", "the signer is the caller");

        let standing = card(&served).agents[0].standing.clone();
        assert_eq!(standing.count(Judgment::Disputed), 1);
        assert_eq!(standing.failure_rate, Some(1.0));

        let audit = served.registry.audit_records().unwrap();
        let record = audit
            .iter()
            .find(|record| record.action == "mission.verdict")
            .unwrap();
        assert_eq!(record.actor, "operator-007");
        assert_eq!(record.target, "m-done failed-verification");
        assert!(
            !record.target.contains("tag"),
            "the note stays out of the audit"
        );
        let timeline = served.registry.timeline_events("G").unwrap();
        assert!(
            timeline
                .iter()
                .any(|event| event.kind == "verdict" && event.action == "failed-verification"),
            "{timeline:?}"
        );

        // A person changes their mind, and the Agent gets its credit back.
        let confirmed = verdict(
            &served,
            &served.operator,
            "m-done",
            serde_json::json!({ "verdict": "confirmed" }),
        )
        .await;
        assert_eq!(confirmed.status(), 200);
        assert_eq!(card(&served).agents[0].standing.failure_rate, Some(0.0));
    }

    #[tokio::test]
    async fn only_mission_verify_gives_a_verdict_and_only_on_an_outcome() {
        let served = served_with(ROLES).await;
        ended(&served, "m-done", "done");
        ended(&served, "m-working", "working");
        let body = serde_json::json!({ "verdict": "disputed" });

        let denied = verdict(&served, &served.viewer, "m-done", body.clone()).await;
        assert_eq!(denied.status(), 403);
        assert_eq!(
            served
                .registry
                .session("m-done-1")
                .unwrap()
                .unwrap()
                .verdict,
            None
        );

        let working = verdict(&served, &served.operator, "m-working", body.clone()).await;
        assert_eq!(working.status(), 409);
        let refused: serde_json::Value = working.json().await.unwrap();
        assert!(
            refused["error"].as_str().unwrap().contains("working"),
            "{refused}"
        );

        let unknown = verdict(&served, &served.operator, "m-none", body.clone()).await;
        assert_eq!(unknown.status(), 404);

        let word = verdict(
            &served,
            &served.operator,
            "m-done",
            serde_json::json!({ "verdict": "wrong" }),
        )
        .await;
        assert_eq!(word.status(), 400);
        let unknown_field = verdict(
            &served,
            &served.operator,
            "m-done",
            serde_json::json!({ "verdict": "disputed", "reason": "x" }),
        )
        .await;
        assert_eq!(unknown_field.status(), 422);
        let blank = verdict(
            &served,
            &served.operator,
            "m-done",
            serde_json::json!({ "verdict": "disputed", "note": "  " }),
        )
        .await;
        assert_eq!(blank.status(), 400);
    }

    async fn act(
        served: &Served,
        token: &str,
        mission: &str,
        action: &str,
        body: Option<serde_json::Value>,
    ) -> reqwest::Response {
        let request = reqwest::Client::new()
            .post(format!(
                "{}/v1/user/operations/G/missions/{mission}/{action}",
                served.base
            ))
            .bearer_auth(token);
        match body {
            Some(body) => request.json(&body).send().await.unwrap(),
            None => request.send().await.unwrap(),
        }
    }

    #[tokio::test]
    async fn a_decision_on_a_verifying_mission_is_audited_and_recorded_with_its_reason() {
        let served = served_with(ROLES).await;
        ended(&served, "m-verifying", "verifying");
        ended(&served, "m-done", "done");

        let denied = act(
            &served,
            &served.viewer,
            "m-verifying",
            "verify",
            Some(serde_json::json!({ "decision": "done", "reason": "ok" })),
        )
        .await;
        assert_eq!(denied.status(), 403);

        // Only an attempt being verified is decided here.
        let not_verifying = act(
            &served,
            &served.operator,
            "m-done",
            "verify",
            Some(serde_json::json!({ "decision": "done", "reason": "ok" })),
        )
        .await;
        assert_eq!(not_verifying.status(), 409);

        for (body, status) in [
            (
                serde_json::json!({ "decision": "maybe", "reason": "x" }),
                400,
            ),
            (
                serde_json::json!({ "decision": "done", "reason": "  " }),
                400,
            ),
            (
                serde_json::json!({ "decision": "failed", "reason": "x", "amendment": "y" }),
                400,
            ),
            (serde_json::json!({ "decision": "done" }), 422),
        ] {
            let refused = act(
                &served,
                &served.operator,
                "m-verifying",
                "verify",
                Some(body),
            )
            .await;
            assert_eq!(refused.status(), status);
        }
        assert_eq!(
            served
                .registry
                .session("m-verifying-1")
                .unwrap()
                .unwrap()
                .verdict,
            None,
            "a refused decision leaves nothing"
        );

        let given = act(
            &served,
            &served.operator,
            "m-verifying",
            "verify",
            Some(serde_json::json!({
                "decision": "redispatch",
                "reason": "no branch on the remote",
                "amendment": "Push the branch.",
            })),
        )
        .await;
        assert_eq!(given.status(), 200);
        let given: serde_json::Value = given.json().await.unwrap();
        assert_eq!(given["decision"], "redispatch");
        assert_eq!(given["verdict"], "redispatch");
        assert_eq!(given["note"], "no branch on the remote");
        assert_eq!(given["amendment"], "Push the branch.");
        assert_eq!(given["by"], "operator-007");

        let verdict = served
            .registry
            .session("m-verifying-1")
            .unwrap()
            .unwrap()
            .verdict
            .unwrap();
        assert_eq!(verdict.verdict, ai6::OutcomeVerdict::Redispatch);
        assert_eq!(verdict.amendment.as_deref(), Some("Push the branch."));
        let audit = served.registry.audit_records().unwrap();
        let record = audit
            .iter()
            .find(|record| record.action == "mission.verify")
            .unwrap();
        assert_eq!(record.target, "m-verifying redispatch");
        assert!(
            !record.target.contains("branch"),
            "the reason stays out of the audit"
        );
        let timeline = served.registry.timeline_events("G").unwrap();
        assert!(
            timeline
                .iter()
                .any(|event| event.kind == "verify" && event.action == "redispatch"),
            "{timeline:?}"
        );

        // The verdict route does not redispatch: that needs the reason and the next attempt.
        let word = verdict_route_redispatch(&served).await;
        assert_eq!(word, 400);
    }

    async fn verdict_route_redispatch(served: &Served) -> u16 {
        verdict(
            served,
            &served.operator,
            "m-verifying",
            serde_json::json!({ "verdict": "redispatch" }),
        )
        .await
        .status()
        .as_u16()
    }

    #[tokio::test]
    async fn a_probe_is_asked_for_by_a_person_with_mission_verify_on_a_verifying_attempt() {
        let served = served_with(ROLES).await;
        ended(&served, "m-verifying", "verifying");
        ended(&served, "m-done", "done");

        let denied = act(&served, &served.viewer, "m-verifying", "probe", None).await;
        assert_eq!(denied.status(), 403);
        let settled = act(&served, &served.operator, "m-done", "probe", None).await;
        assert_eq!(settled.status(), 409);
        assert_eq!(
            served
                .registry
                .session("m-verifying-1")
                .unwrap()
                .unwrap()
                .probe_requested,
            None
        );

        let asked = act(&served, &served.operator, "m-verifying", "probe", None).await;
        assert_eq!(asked.status(), 200);
        let on_board = served.registry.session("record-g").unwrap().unwrap();
        served
            .registry
            .update_session(
                "record-g",
                "worker",
                on_board.fence,
                SessionUpdate::default().result(SessionResult::new("verifying")),
            )
            .unwrap();
        let board = act(&served, &served.operator, "mission-g", "probe", None).await;
        assert_eq!(board.status(), 200);
        let asked: serde_json::Value = asked.json().await.unwrap();
        assert_eq!(asked["attempt"], 1);
        let requested = served
            .registry
            .session("m-verifying-1")
            .unwrap()
            .unwrap()
            .probe_requested;
        assert_eq!(requested, asked["requested_at"].as_f64());
        assert!(
            served
                .registry
                .audit_records()
                .unwrap()
                .iter()
                .any(|record| record.action == "mission.probe" && record.target == "m-verifying")
        );

        // The Station's findings appear in the detail of a Mission on the board.
        let opened = served.registry.session("record-g").unwrap().unwrap();
        served
            .registry
            .update_session(
                "record-g",
                "worker",
                opened.fence,
                SessionUpdate::default()
                    .deliverables(vec![
                        ai6::directorates::m::deliverables::Deliverable::parse(
                            "branch:never-pushed",
                        )
                        .unwrap(),
                    ])
                    .evidence(vec![ai6::Evidence::new(
                        ai6::directorates::m::deliverables::Deliverable::parse(
                            "branch:never-pushed",
                        )
                        .unwrap(),
                        ai6::ProbeResult::Missing,
                        "branch never-pushed is not on the remote",
                        2.0,
                    )])
                    .probed(),
            )
            .unwrap();
        let detail: serde_json::Value = reqwest::Client::new()
            .get(format!(
                "{}/v1/operations/G/missions/mission-g",
                served.base
            ))
            .bearer_auth(&served.operator)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let attempt = &detail["attempts"][0];
        assert_eq!(
            attempt["deliverables"][0], "branch:never-pushed",
            "{detail}"
        );
        assert_eq!(attempt["evidence"][0]["result"], "missing");
        assert!(
            attempt["evidence"][0]["detail"]
                .as_str()
                .unwrap()
                .contains("never-pushed")
        );
        assert!(attempt.get("probe_requested").is_none(), "answered");
    }

    #[tokio::test]
    async fn the_board_shows_a_verdict_and_the_detail_the_final_report() {
        let served = served_with(ROLES).await;
        let opened = served.registry.session("record-g").unwrap().unwrap();
        served
            .registry
            .update_session(
                "record-g",
                "worker",
                opened.fence,
                SessionUpdate::default().result(SessionResult::new("done")),
            )
            .unwrap();
        served
            .registry
            .file_final_report(&ai6::FinalReport::new(
                "record-g",
                "mission-g",
                "All merged.",
                1.0,
                ai6::Limit::UNBOUNDED,
            ))
            .unwrap();
        verdict(
            &served,
            &served.operator,
            "mission-g",
            serde_json::json!({ "verdict": "disputed", "note": "not on main" }),
        )
        .await;

        let client = reqwest::Client::new();
        let detail: serde_json::Value = client
            .get(format!(
                "{}/v1/operations/G/missions/mission-g",
                served.base
            ))
            .bearer_auth(&served.operator)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let attempt = &detail["attempts"][0];
        assert_eq!(attempt["verdict"]["verdict"], "disputed", "{detail}");
        assert_eq!(attempt["verdict"]["note"], "not on main");
        assert_eq!(attempt["final_report"], "All merged.");

        let board: serde_json::Value = client
            .get(format!("{}/v1/missions", served.base))
            .bearer_auth(&served.operator)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let listed = board
            .as_array()
            .unwrap()
            .iter()
            .find(|mission| mission["mission_id"] == "mission-g")
            .map(|mission| &mission["attempts"][0])
            .unwrap();
        assert_eq!(listed["verdict"]["verdict"], "disputed", "{board}");
        assert!(
            listed.get("final_report").is_none(),
            "only the detail has it"
        );
    }
}
