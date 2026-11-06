//! Held briefs through Universal Exports: the briefs vetting held for an Operation, and an
//! operator's release or refusal.
//!
//! Every route needs `mission.create` on the Operation, as approving a Mission proposal does:
//! releasing a held brief admits work. A release returns the brief to the queue cleared of the
//! kinds it was held for; a refusal files it as failed. Each is audited and recorded on the
//! Operation's timeline with the brief's key, never its text.

use std::sync::Arc;

use ai6::directorates::registry::backend::*;
use ai6::{AuditRecord, HeldBrief, TimelineEvent};
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, Gateway, GatewayError, authorize_operation_action, bearer,
    ensure_operation_registered, resolve_user, unix_timestamp,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RefuseRequest {
    /// Why, for the record; never sent to the Source.
    reason: Option<String>,
}

/// One held brief as the gateway lists it: its Registry key beside the record.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeldBriefEntry {
    /// The key the release and refuse routes take.
    pub key: String,
    /// The Operation, the brief, its vetting and when it was held.
    #[serde(flatten)]
    pub held: HeldBrief,
}

/// What became of a held brief.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeldBriefDecision {
    /// The Operation.
    pub operation_id: String,
    /// The brief's Registry key.
    pub key: String,
    /// `queued` after a release, `refused` after a refusal.
    pub state: &'static str,
}

fn registry_error(error: std::io::Error) -> ApiError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ApiError::not_found(),
        std::io::ErrorKind::AlreadyExists => ApiError::from(GatewayError::Conflict),
        std::io::ErrorKind::InvalidInput => ApiError::bad_request(),
        _ => ApiError::internal(),
    }
}

fn valid_key(key: &str) -> Result<(), ApiError> {
    if key.is_empty()
        || key.len() > 256
        || key
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(ApiError::bad_request());
    }
    Ok(())
}

/// Resolves the caller, checks `mission.create` on the Operation and that it exists.
async fn authorized(
    gateway: &Gateway,
    headers: &HeaderMap,
    operation_id: String,
) -> Result<(ai6::Person, OperationId, Arc<dyn RegistryBackend>), ApiError> {
    let person = resolve_user(gateway, bearer(headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(gateway, person.clone(), &operation, "mission.create").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation).await?;
    Ok((person, operation, registry))
}

/// Audits a decision and adds it to the Operation's timeline. The audit record must be
/// written; a timeline entry that fails is left out.
async fn record(
    registry: Arc<dyn RegistryBackend>,
    person: &ai6::Person,
    operation: &OperationId,
    key: &str,
    action: &'static str,
    at: f64,
) -> Result<(), ApiError> {
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: format!("mission.brief.{action}"),
        operation: operation.to_string(),
        resource: None,
        target: key.to_owned(),
        at,
    };
    let event = TimelineEvent::new("intake", action, operation.to_string(), at)
        .mission(key)
        .actor(person.id.clone());
    tokio::task::spawn_blocking(move || {
        registry.record_audit(&audit)?;
        let _ = registry.record_event(&event);
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())
}

/// `GET /v1/user/operations/{operation_id}/held-briefs`: the briefs vetting holds, oldest
/// first, each with its findings.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<Vec<HeldBriefEntry>>, ApiError> {
    let (_, operation, registry) = authorized(&gateway, &headers, operation_id).await?;
    let operation = operation.to_string();
    let held = tokio::task::spawn_blocking(move || {
        registry.held_briefs(&operation, registry.limits().held.results.or_max())
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(registry_error)?;
    Ok(Json(
        held.into_iter()
            .map(|held| HeldBriefEntry {
                key: held.brief.key(),
                held,
            })
            .collect(),
    ))
}

/// `POST /v1/user/operations/{operation_id}/held-briefs/{key}/release`: returns the brief to
/// the queue, cleared of the kinds it was held for.
pub(crate) async fn release(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, key)): Path<(String, String)>,
) -> Result<Json<HeldBriefDecision>, ApiError> {
    let key = crate::mission_key(&gateway, &key).await?;
    valid_key(&key)?;
    let (person, operation, registry) = authorized(&gateway, &headers, operation_id).await?;
    let now = unix_timestamp()?;
    let store = registry.clone();
    let (operation_key, brief_key, by) = (operation.to_string(), key.clone(), person.id.clone());
    tokio::task::spawn_blocking(move || store.release_held(&operation_key, &brief_key, &by, now))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?;
    record(registry, &person, &operation, &key, "released", now).await?;
    Ok(Json(HeldBriefDecision {
        operation_id: operation.to_string(),
        key,
        state: "queued",
    }))
}

/// `POST /v1/user/operations/{operation_id}/held-briefs/{key}/refuse`: files the brief as
/// failed with a `refused` outcome; a Source still gets its report.
pub(crate) async fn refuse(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, key)): Path<(String, String)>,
    Json(request): Json<RefuseRequest>,
) -> Result<Json<HeldBriefDecision>, ApiError> {
    let key = crate::mission_key(&gateway, &key).await?;
    valid_key(&key)?;
    let reason = request.reason.unwrap_or_default().trim().to_owned();
    if !gateway
        .config
        .unex()
        .text
        .held_reason_chars
        .allows(reason.chars().count())
        || reason
            .chars()
            .any(|character| character.is_control() && character != '\n')
    {
        return Err(ApiError::bad_request());
    }
    let (person, operation, registry) = authorized(&gateway, &headers, operation_id).await?;
    let now = unix_timestamp()?;
    let store = registry.clone();
    let (operation_key, brief_key, by) = (operation.to_string(), key.clone(), person.id.clone());
    tokio::task::spawn_blocking(move || {
        store.refuse_held(&operation_key, &brief_key, &by, &reason, now)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(registry_error)?;
    record(registry, &person, &operation, &key, "refused", now).await?;
    Ok(Json(HeldBriefDecision {
        operation_id: operation.to_string(),
        key,
        state: "refused",
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WithdrawRequest {
    /// Why, for the record.
    #[serde(default)]
    note: Option<String>,
}

/// A brief withdrawn before any Station took it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WithdrawnBrief {
    /// The Operation.
    pub operation_id: String,
    /// The Mission's UID, its brief's key.
    pub mission_id: String,
    /// `withdrawn`.
    pub state: &'static str,
    /// The reason it was filed with: who withdrew it, and their note.
    pub reason: String,
}

/// What a withdrawal filed.
pub(crate) struct Withdrawn {
    pub(crate) reason: String,
    pub(crate) at: f64,
}

/// What became of a request to withdraw a brief.
pub(crate) enum Withdrawal {
    /// It was queued or held, and is filed as failed.
    Done(Withdrawn),
    /// The Operation has no such brief queued or held.
    NotQueued,
    /// A Station claimed it, or it finished.
    Decided,
}

/// Withdraws the Operation's brief `key` when it is queued or held, audited as
/// `mission.withdraw` and put on the Operation's timeline. The caller has checked
/// `mission.control`.
pub(crate) async fn withdraw_queued(
    gateway: &Gateway,
    person: &ai6::Person,
    operation: &OperationId,
    key: &str,
    note: Option<String>,
) -> Result<Withdrawal, ApiError> {
    let note = note.unwrap_or_default().trim().to_owned();
    if !gateway
        .config
        .unex()
        .text
        .held_reason_chars
        .allows(note.chars().count())
        || note
            .chars()
            .any(|character| character.is_control() && character != '\n')
    {
        return Err(ApiError::bad_request());
    }
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let at = unix_timestamp()?;
    let (store, operation_key, brief_key, by) = (
        registry.clone(),
        operation.to_string(),
        key.to_owned(),
        person.id.clone(),
    );
    let withdrawn = tokio::task::spawn_blocking(move || {
        store.withdraw_brief(&operation_key, &brief_key, &by, &note, at)
    })
    .await
    .map_err(|_| ApiError::internal())?;
    match withdrawn {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Withdrawal::NotQueued);
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Ok(Withdrawal::Decided);
        }
        Err(error) => return Err(registry_error(error)),
    }
    let filed = registry.clone();
    let brief_key = key.to_owned();
    let reason = tokio::task::spawn_blocking(move || filed.filed_brief(&brief_key))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?
        .and_then(|filed| filed.outcome)
        .and_then(|outcome| outcome["reason"].as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("withdrawn by {}", person.id));
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: "mission.withdraw".to_owned(),
        operation: operation.to_string(),
        resource: None,
        target: key.to_owned(),
        at,
    };
    let event = TimelineEvent::new("intake", "withdrawn", operation.to_string(), at)
        .mission(key)
        .actor(person.id.clone());
    tokio::task::spawn_blocking(move || {
        registry.record_audit(&audit)?;
        let _ = registry.record_event(&event);
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    Ok(Withdrawal::Done(Withdrawn { reason, at }))
}

/// `POST /v1/user/operations/{operation_id}/missions/{mission_id}/withdraw`: files a brief no
/// Station has taken, queued or held, as failed with who withdrew it and their note. Needs
/// `mission.control`, as a cancel does.
pub(crate) async fn withdraw(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
    Json(request): Json<WithdrawRequest>,
) -> Result<Json<WithdrawnBrief>, ApiError> {
    let key = crate::mission_key(&gateway, &mission_id).await?;
    valid_key(&key)?;
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation, "mission.control").await?;
    let withdrawn = match withdraw_queued(&gateway, &person, &operation, &key, request.note).await?
    {
        Withdrawal::Done(withdrawn) => withdrawn,
        Withdrawal::NotQueued => return Err(ApiError::not_found()),
        Withdrawal::Decided => {
            return Err(ApiError::refused(
                axum::http::StatusCode::CONFLICT,
                "a Station took the Mission, or it ended: use `mission cancel`".to_owned(),
            ));
        }
    };
    Ok(Json(WithdrawnBrief {
        operation_id: operation.to_string(),
        mission_id: key,
        state: "withdrawn",
        reason: withdrawn.reason,
    }))
}

#[cfg(test)]
mod tests {
    use ai6::directorates::registry::backend::*;
    use ai6::{Brief, Finding, FindingKind, Vetting};

    use crate::controls::tests::{Served, served_with};

    /// Operator 007 creates Missions in G; viewer 007 only reads them.
    const ROLES: &str = r#"
        [role.viewer]
        actions = ["mission.read"]

        [role.creator]
        actions = ["mission.read", "mission.create"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "viewer"

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "creator"
    "#;

    fn hold(served: &Served, id: &str) -> String {
        let brief: Brief = serde_json::from_value(serde_json::json!({
            "title": "Crash on save",
            "body": "untrusted words",
            "id": id,
            "source": "source:github",
            "operation": "G",
        }))
        .unwrap();
        served.registry.submit_brief(&brief).unwrap();
        let vetting = Vetting::from_findings(vec![
            Finding::new(FindingKind::Duplicate, "the same title as other work")
                .related("mission-g"),
        ]);
        served
            .registry
            .hold_brief("G", &brief.key(), &vetting, 1.0)
            .unwrap()
            .unwrap();
        brief.key()
    }

    async fn call(
        served: &Served,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}/v1/user/operations/{path}", served.base))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await.unwrap()
    }

    #[tokio::test]
    async fn an_operator_lists_releases_and_refuses_held_briefs() {
        let served = served_with(ROLES).await;
        let released = hold(&served, "org/repo#1");
        let refused = hold(&served, "org/repo#2");

        let listed = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "G/held-briefs",
            None,
        )
        .await;
        assert_eq!(listed.status(), 200);
        let listed: serde_json::Value = listed.json().await.unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 2);
        assert_eq!(listed[0]["key"], released.as_str());
        assert_eq!(listed[0]["operation"], "G");
        assert_eq!(listed[0]["vetting"]["verdict"], "hold");
        assert_eq!(listed[0]["vetting"]["findings"][0]["kind"], "duplicate");

        let path = format!("G/held-briefs/{released}/release");
        let response = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &path,
            None,
        )
        .await;
        assert_eq!(response.status(), 200);
        let response: serde_json::Value = response.json().await.unwrap();
        assert_eq!(response["state"], "queued");
        assert_eq!(served.registry.list_new().unwrap()[0].key(), released);
        let again = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &path,
            None,
        )
        .await;
        assert_eq!(again.status(), 200, "releasing again changes nothing");
        assert_eq!(served.registry.list_new().unwrap().len(), 1);
        served
            .registry
            .claim_with_lease(
                Some(&released),
                "hq",
                std::time::Duration::from_secs(60),
                std::collections::BTreeMap::new(),
            )
            .unwrap()
            .unwrap();
        let claimed = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &path,
            None,
        )
        .await;
        assert_eq!(claimed.status(), 409, "a placed brief is past releasing");

        let path = format!("G/held-briefs/{refused}/refuse");
        let body = serde_json::json!({ "reason": "a mirror of mission-g" });
        let response = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &path,
            Some(body),
        )
        .await;
        assert_eq!(response.status(), 200);
        assert!(served.registry.held_briefs("G", 10).unwrap().is_empty());
        let finished = served.registry.unreported_source_briefs().unwrap();
        assert_eq!(finished[0].outcome["state"], "refused");
        assert_eq!(finished[0].outcome["refused_by"], "operator-007");

        let audit = served.registry.audit_records().unwrap();
        for (action, key) in [
            ("mission.brief.released", &released),
            ("mission.brief.refused", &refused),
        ] {
            assert!(
                audit.iter().any(|record| record.action == action
                    && &record.target == key
                    && record.actor == "operator-007"),
                "{action}"
            );
        }
        assert!(!format!("{audit:?}").contains("untrusted words"));
        let actions: Vec<String> = served
            .registry
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .map(|event| event.action)
            .collect();
        // Each request the operator made is recorded, the repeated release too.
        assert_eq!(actions, ["released", "released", "refused"]);
    }

    /// Operator 007 controls Missions in G; viewer 007 only reads them.
    const CONTROL_ROLES: &str = r#"
        [role.viewer]
        actions = ["mission.read"]

        [role.controller]
        actions = ["mission.read", "mission.control"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "viewer"

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "controller"
    "#;

    fn queue(served: &Served, id: &str) -> String {
        let brief: Brief = serde_json::from_value(serde_json::json!({
            "title": "Impossible work",
            "id": id,
            "source": "manual",
            "operation": "G",
            "stations": ["nowhere"],
        }))
        .unwrap();
        served.registry.submit_brief(&brief).unwrap();
        brief.key()
    }

    #[tokio::test]
    async fn a_queued_brief_is_withdrawn_with_its_note_and_audited() {
        let served = served_with(CONTROL_ROLES).await;
        let key = queue(&served, "w-1");
        let path = format!("G/missions/{key}/withdraw");
        let body = serde_json::json!({"note": "it can never place"});
        let viewer = call(
            &served,
            &served.viewer,
            reqwest::Method::POST,
            &path,
            Some(body.clone()),
        )
        .await;
        assert_eq!(viewer.status(), 403, "withdrawing needs mission.control");
        let response = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &path,
            Some(body),
        )
        .await;
        assert_eq!(response.status(), 200);
        let response: serde_json::Value = response.json().await.unwrap();
        assert_eq!(response["state"], "withdrawn");
        assert_eq!(response["mission_id"], key.as_str());
        assert_eq!(
            response["reason"],
            "withdrawn by operator-007: it can never place"
        );
        assert!(served.registry.list_new().unwrap().is_empty());
        let filed = served.registry.filed_brief(&key).unwrap().unwrap();
        assert_eq!(filed.state, "failed");
        assert!(
            served
                .registry
                .audit_records()
                .unwrap()
                .iter()
                .any(|record| {
                    record.action == "mission.withdraw"
                        && record.target == key
                        && record.actor == "operator-007"
                })
        );
        let actions: Vec<String> = served
            .registry
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .map(|event| event.action)
            .collect();
        assert_eq!(actions, ["withdrawn"]);

        let unknown = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            "G/missions/manual--nobody/withdraw",
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(unknown.status(), 404);
        let placed = queue(&served, "w-2");
        served
            .registry
            .claim_with_lease(
                Some(&placed),
                "worker",
                std::time::Duration::from_secs(60),
                std::collections::BTreeMap::new(),
            )
            .unwrap()
            .unwrap();
        let taken = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/missions/{placed}/withdraw"),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(taken.status(), 409);
        assert!(taken.text().await.unwrap().contains("`mission cancel`"));
    }

    /// A Mission whose brief is filed and which has no Session record: what the withdraw's refusal
    /// sends an operator to must work, here through Headquarters.
    #[tokio::test]
    async fn a_cancel_of_a_mission_whose_brief_ended_reaches_a_station() {
        let served = served_with(CONTROL_ROLES).await;
        let key = ended(&served, "e-1");
        let refused = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/missions/{key}/withdraw"),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(refused.status(), 409);
        let asked = crate::controls::tests::ask(
            &served,
            &served.operator,
            "G",
            &key,
            serde_json::json!({"action": "cancel", "idempotency_key": "e-1"}),
        )
        .await;
        assert_eq!(asked.status(), 202);
        let status: serde_json::Value = asked.json().await.unwrap();
        assert_eq!(status["status"], "queued");
        assert_eq!(status["station_id"], "hq");
        let control = crate::controls::tests::take(&served, "hq").await.unwrap();
        assert_eq!(control.action(), ai6_diptel::ControlAction::Cancel);

        // Another Operation's cancel learns nothing of it.
        let elsewhere = crate::controls::tests::ask(
            &served,
            &served.operator,
            "L",
            &key,
            serde_json::json!({"action": "cancel", "idempotency_key": "e-2"}),
        )
        .await;
        assert_ne!(elsewhere.status(), 202);
    }

    /// With no Station to ask, a Mission that already ended is settled by the gateway.
    #[tokio::test]
    async fn a_cancel_of_an_ended_mission_with_no_station_to_ask_is_settled_here() {
        let served = served_with(CONTROL_ROLES).await;
        let key = ended(&served, "e-3");
        served
            .registry
            .elect("hq", std::time::Duration::from_nanos(1))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let asked = crate::controls::tests::ask(
            &served,
            &served.operator,
            "G",
            &key,
            serde_json::json!({"action": "cancel", "idempotency_key": "e-3"}),
        )
        .await;
        assert_eq!(asked.status(), 202);
        let status: serde_json::Value = asked.json().await.unwrap();
        assert_eq!(status["status"], "applied");
        assert_eq!(status["state"], "failed");
    }

    /// Files `id`'s brief as failed, with no Session record and no lease left.
    fn ended(served: &Served, id: &str) -> String {
        let key = queue(served, id);
        let claimed = served.registry.claim_new(Some(&key)).unwrap().unwrap();
        served
            .registry
            .finish(
                &claimed,
                &serde_json::json!({"state": "failed", "reason": "no"}),
                false,
                None,
                None,
            )
            .unwrap();
        key
    }

    #[tokio::test]
    async fn a_cancel_of_a_brief_no_station_took_withdraws_it_and_one_placed_is_recorded() {
        let served = served_with(CONTROL_ROLES).await;
        let key = queue(&served, "c-1");
        let cancel = |idempotency: &str| serde_json::json!({"action": "cancel", "idempotency_key": idempotency});
        let asked =
            crate::controls::tests::ask(&served, &served.operator, "G", &key, cancel("c-1")).await;
        assert_eq!(asked.status(), 202);
        let status: serde_json::Value = asked.json().await.unwrap();
        assert_eq!(status["status"], "applied");
        assert_eq!(status["state"], "failed");
        assert_eq!(status["reason"], "withdrawn by operator-007");
        assert_eq!(
            served.registry.filed_brief(&key).unwrap().unwrap().state,
            "failed"
        );
        assert!(
            crate::controls::tests::take(&served, "hq").await.is_none(),
            "no Station is asked"
        );
        let replay =
            crate::controls::tests::ask(&served, &served.operator, "G", &key, cancel("c-1")).await;
        assert_eq!(
            replay.status(),
            200,
            "the same key returns the same control"
        );

        // A placed brief: the cancel goes to its Station and the Registry keeps when it was asked.
        let placed = queue(&served, "c-2");
        served
            .registry
            .claim_with_lease(
                Some(&placed),
                "worker",
                std::time::Duration::from_secs(60),
                std::collections::BTreeMap::from([
                    ("mission_id".to_owned(), serde_json::json!(placed)),
                    ("operation".to_owned(), serde_json::json!("G")),
                ]),
            )
            .unwrap()
            .unwrap();
        let asked =
            crate::controls::tests::ask(&served, &served.operator, "G", &placed, cancel("c-2"))
                .await;
        assert_eq!(asked.status(), 202);
        let status: serde_json::Value = asked.json().await.unwrap();
        assert_eq!(status["status"], "queued");
        assert_eq!(status["station_id"], "worker");
        let requests = served.registry.cancel_requests().unwrap();
        assert!(requests.contains_key(&placed), "{requests:?}");
    }

    #[tokio::test]
    async fn held_briefs_need_mission_create_on_their_operation() {
        let served = served_with(ROLES).await;
        let key = hold(&served, "org/repo#3");
        for (method, path, body) in [
            (reqwest::Method::GET, "G/held-briefs".to_owned(), None),
            (
                reqwest::Method::POST,
                format!("G/held-briefs/{key}/release"),
                None,
            ),
            (
                reqwest::Method::POST,
                format!("G/held-briefs/{key}/refuse"),
                Some(serde_json::json!({})),
            ),
        ] {
            let denied = call(&served, &served.viewer, method, &path, body).await;
            assert_eq!(denied.status(), 403, "{path}");
        }
        let elsewhere = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("L/held-briefs/{key}/release"),
            None,
        )
        .await;
        assert_eq!(elsewhere.status(), 403, "mission.create is per Operation");
        assert_eq!(served.registry.held_briefs("G", 10).unwrap().len(), 1);
    }
}
