//! A person's verdict on a Mission's outcome: the latest attempt that ended done or held is marked
//! confirmed, disputed or failed verification, under `mission.verify`.
//!
//! The scorecard reads it from the Session record; a Station settles a held Mission by it. A
//! verdict is audited with its word and never its note, and goes on the Operation's timeline.

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
    let word = OutcomeVerdict::from_name(&request.verdict).ok_or_else(|| {
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
