//! Green ink through Universal Exports: the requests waiting for a person, the decision on
//! each, and class authorisations (standing approvals of a kind of step).
//!
//! Decisions are written to the shared Registry; the Station holding the Mission reads them
//! and tells the Agent. The signer is always the authenticated person, never request text.
//! Deciding needs `approval.decide` on the Operation; class authorisations need
//! `approval.class`. Neither is implied by `mission.create` or `mission.control`.

use std::sync::Arc;
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{
    Approval, ApprovalRequest, AuditRecord, ClassAuthorization, DEFAULT_APPROVAL_TTL, Ruling,
};
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    ApiError, Gateway, GatewayError, authorize_operation_action, bearer,
    ensure_operation_registered, resolve_user, unix_timestamp,
};

/// The longest an approval or class authorisation may be granted for: one year.
pub(crate) const MAX_GRANT: Duration = Duration::from_secs(365 * 24 * 60 * 60);
const MAX_NOTE_CHARS: usize = 1_024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApproveRequest {
    /// Seconds the approval lasts; one hour when omitted.
    expires_in_secs: Option<u64>,
    /// What to tell the Agent.
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RejectRequest {
    /// Why, for the Agent.
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClassRequest {
    kinds: Vec<String>,
    target_pattern: String,
    #[serde(default)]
    agents: Vec<String>,
    #[serde(default)]
    roles: Vec<String>,
    clearance_min: Option<String>,
    max_uses: Option<u32>,
    expires_in_secs: u64,
}

fn registry_error(error: std::io::Error) -> ApiError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ApiError::not_found(),
        std::io::ErrorKind::AlreadyExists => ApiError::from(GatewayError::Conflict),
        std::io::ErrorKind::InvalidInput => ApiError::bad_request(),
        _ => ApiError::internal(),
    }
}

fn valid_id(id: &str) -> Result<(), ApiError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return Err(ApiError::bad_request());
    }
    Ok(())
}

fn grant(seconds: u64) -> Result<f64, ApiError> {
    if seconds == 0 || seconds > MAX_GRANT.as_secs() {
        return Err(ApiError::bad_request());
    }
    Ok(seconds as f64)
}

fn note(text: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(text) = text.map(|text| text.trim().to_owned()) else {
        return Ok(None);
    };
    if text.is_empty()
        || text.chars().count() > MAX_NOTE_CHARS
        || text
            .chars()
            .any(|character| character.is_control() && character != '\n')
    {
        return Err(ApiError::bad_request());
    }
    Ok(Some(text))
}

/// Resolves the caller, checks `action` on the Operation and that the Operation exists.
async fn authorized(
    gateway: &Gateway,
    headers: &HeaderMap,
    operation_id: String,
    action: &'static str,
) -> Result<(ai6::Person, OperationId, Arc<dyn RegistryBackend>), ApiError> {
    let person = resolve_user(gateway, bearer(headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(gateway, person.clone(), &operation, action).await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation).await?;
    Ok((person, operation, registry))
}

async fn audit(
    registry: Arc<dyn RegistryBackend>,
    actor: String,
    action: &'static str,
    operation: &OperationId,
    target: String,
    at: f64,
) -> Result<(), ApiError> {
    let record = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor,
        action: action.to_owned(),
        operation: operation.to_string(),
        resource: None,
        target,
        at,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(())
}

/// `GET /v1/user/operations/{operation_id}/green-ink`: the requests waiting for a decision,
/// oldest first.
pub(crate) async fn pending(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<Vec<ApprovalRequest>>, ApiError> {
    let (_, operation, registry) =
        authorized(&gateway, &headers, operation_id, "approval.decide").await?;
    let now = unix_timestamp()?;
    let operation = operation.to_string();
    let requests = tokio::task::spawn_blocking(move || registry.pending_approvals(&operation, now))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?;
    Ok(Json(requests))
}

/// `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/approve`.
pub(crate) async fn approve(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, request_id)): Path<(String, String)>,
    Json(request): Json<ApproveRequest>,
) -> Result<Json<Approval>, ApiError> {
    valid_id(&request_id)?;
    let lasts = request
        .expires_in_secs
        .map_or(Ok(DEFAULT_APPROVAL_TTL.as_secs_f64()), grant)?;
    let note = note(request.note)?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "approval.decide").await?;
    let now = unix_timestamp()?;
    let mut ruling = Ruling::approve(person.id.clone(), now, now + lasts);
    if let Some(note) = note {
        ruling = ruling.with_note(note);
    }
    decide(
        registry,
        person,
        operation,
        request_id,
        ruling,
        "green_ink.approved",
    )
    .await
}

/// `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/reject`.
pub(crate) async fn reject(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, request_id)): Path<(String, String)>,
    Json(request): Json<RejectRequest>,
) -> Result<Json<Approval>, ApiError> {
    valid_id(&request_id)?;
    let reason = note(request.reason)?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "approval.decide").await?;
    let now = unix_timestamp()?;
    let ruling = Ruling::reject(person.id.clone(), now, reason);
    decide(
        registry,
        person,
        operation,
        request_id,
        ruling,
        "green_ink.rejected",
    )
    .await
}

async fn decide(
    registry: Arc<dyn RegistryBackend>,
    person: ai6::Person,
    operation: OperationId,
    request_id: String,
    ruling: Ruling,
    action: &'static str,
) -> Result<Json<Approval>, ApiError> {
    let at = ruling.at;
    let store = registry.clone();
    let operation_key = operation.to_string();
    let id = request_id.clone();
    let approval =
        tokio::task::spawn_blocking(move || store.decide_approval(&operation_key, &id, &ruling))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(registry_error)?;
    audit(registry, person.id, action, &operation, request_id, at).await?;
    Ok(Json(approval))
}

/// `GET /v1/user/operations/{operation_id}/class-authorizations`: every class authorisation,
/// oldest first, revoked and expired ones included.
pub(crate) async fn list_classes(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<Vec<ClassAuthorization>>, ApiError> {
    let (_, operation, registry) =
        authorized(&gateway, &headers, operation_id, "approval.class").await?;
    let operation = operation.to_string();
    let records = tokio::task::spawn_blocking(move || registry.class_authorizations(&operation))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?;
    Ok(Json(records))
}

/// `POST /v1/user/operations/{operation_id}/class-authorizations`: grants one, signed by the
/// caller.
pub(crate) async fn create_class(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    Json(request): Json<ClassRequest>,
) -> Result<(StatusCode, Json<ClassAuthorization>), ApiError> {
    let lasts = grant(request.expires_in_secs)?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "approval.class").await?;
    let now = unix_timestamp()?;
    let mut record = ClassAuthorization::new(
        operation.to_string(),
        request.kinds,
        request.target_pattern,
        person.id.clone(),
        now,
        now + lasts,
    )
    .for_agents(request.agents)
    .for_roles(request.roles);
    if let Some(clearance) = request.clearance_min {
        record = record.with_clearance_min(clearance);
    }
    if let Some(uses) = request.max_uses {
        record = record.with_max_uses(uses);
    }
    let store = registry.clone();
    let stored = record.clone();
    tokio::task::spawn_blocking(move || store.put_class_authorization(&stored))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?;
    audit(
        registry,
        person.id,
        "class_authorization.created",
        &operation,
        record.id.clone(),
        now,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(record)))
}

/// `DELETE /v1/user/operations/{operation_id}/class-authorizations/{id}`: revokes one.
pub(crate) async fn revoke_class(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, id)): Path<(String, String)>,
) -> Result<Json<ClassAuthorization>, ApiError> {
    valid_id(&id)?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "approval.class").await?;
    let now = unix_timestamp()?;
    let store = registry.clone();
    let operation_key = operation.to_string();
    let by = person.id.clone();
    let target = id.clone();
    let record = tokio::task::spawn_blocking(move || {
        store.revoke_class_authorization(&operation_key, &target, &by, now)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(registry_error)?;
    audit(
        registry,
        person.id,
        "class_authorization.revoked",
        &operation,
        id,
        now,
    )
    .await?;
    Ok(Json(record))
}

#[cfg(test)]
mod tests {
    use ai6::directorates::registry::backend::*;
    use ai6::{ActionScope, MissionId, SessionId};

    use crate::controls::tests::{Served, served_with};

    /// Operator 007 decides green ink and grants class authorisations in G; viewer 007 only
    /// reads Missions.
    const ROLES: &str = r#"
        [role.viewer]
        actions = ["mission.read", "mission.control", "mission.create"]

        [role.signer]
        actions = ["mission.read", "approval.decide", "approval.class"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "viewer"

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "signer"
    "#;

    fn asked(served: &Served, key: &str, ttl: u64) -> ai6::ApprovalRequest {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let request = ai6::ApprovalRequest::try_new(
            "G",
            MissionId::try_new("mission-g").unwrap(),
            SessionId::try_new("session-g").unwrap(),
            "007",
            ActionScope::try_new("push", "origin:refs/heads/main").unwrap(),
            "the release is green",
            key,
        )
        .unwrap()
        .expiring(now, std::time::Duration::from_secs(ttl));
        served.registry.request_approval(&request).unwrap();
        request
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
    async fn an_operator_signs_and_the_registry_holds_the_decision() {
        let served = served_with(ROLES).await;
        let request = asked(&served, "push-1", 3600);

        let inbox = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "G/green-ink",
            None,
        )
        .await;
        assert_eq!(inbox.status(), 200);
        let inbox: serde_json::Value = inbox.json().await.unwrap();
        assert_eq!(inbox[0]["id"], request.id.as_str());
        assert_eq!(inbox[0]["scope"]["target"], "origin:refs/heads/main");

        let path = format!("G/green-ink/{}/approve", request.id);
        let approved = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &path,
            Some(serde_json::json!({ "expires_in_secs": 600, "note": "go ahead" })),
        )
        .await;
        assert_eq!(approved.status(), 200);
        let approved: serde_json::Value = approved.json().await.unwrap();
        assert_eq!(approved["decision"], "approved");
        assert_eq!(
            approved["signer"], "operator-007",
            "the signer is the caller"
        );
        let lasts = approved["expires_at"].as_f64().unwrap() - approved["at"].as_f64().unwrap();
        assert!((lasts - 600.0).abs() < 1.0, "{lasts}");
        let live = served
            .registry
            .live_approval(
                "G",
                "mission-g",
                &request.scope,
                approved["at"].as_f64().unwrap(),
            )
            .unwrap();
        assert!(live.is_some());

        let rejected = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/green-ink/{}/reject", request.id),
            Some(serde_json::json!({ "reason": "changed my mind" })),
        )
        .await;
        assert_eq!(rejected.status(), 409, "a decided request stays decided");
        let audit = served.registry.audit_records().unwrap();
        assert!(
            audit
                .iter()
                .any(|record| record.action == "green_ink.approved"
                    && record.actor == "operator-007"
                    && record.target == request.id)
        );
    }

    #[tokio::test]
    async fn deciding_needs_approval_decide_and_stays_in_its_operation() {
        let served = served_with(ROLES).await;
        let request = asked(&served, "push-2", 3600);
        for (method, path, body) in [
            (reqwest::Method::GET, "G/green-ink".to_owned(), None),
            (
                reqwest::Method::POST,
                format!("G/green-ink/{}/approve", request.id),
                Some(serde_json::json!({})),
            ),
            (
                reqwest::Method::POST,
                format!("G/green-ink/{}/reject", request.id),
                Some(serde_json::json!({})),
            ),
            (
                reqwest::Method::GET,
                "G/class-authorizations".to_owned(),
                None,
            ),
        ] {
            let denied = call(&served, &served.viewer, method, &path, body).await;
            assert_eq!(denied.status(), 403, "{path}");
        }
        let elsewhere = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("L/green-ink/{}/approve", request.id),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(elsewhere.status(), 403, "approval.decide is per Operation");
        assert_eq!(
            served
                .registry
                .pending_approvals("G", request.requested_at)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn an_expired_request_leaves_the_inbox_and_cannot_be_approved() {
        let served = served_with(ROLES).await;
        let request = asked(&served, "push-3", 1);
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        let inbox: serde_json::Value = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "G/green-ink",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(inbox, serde_json::json!([]));
        let late = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/green-ink/{}/approve", request.id),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(late.status(), 409);
    }

    #[tokio::test]
    async fn a_class_authorisation_is_granted_listed_and_revoked() {
        let served = served_with(ROLES).await;
        let created = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            "G/class-authorizations",
            Some(serde_json::json!({
                "kinds": ["push"],
                "target_pattern": "origin:refs/heads/ai6/*",
                "agents": ["007"],
                "max_uses": 3,
                "expires_in_secs": 3600
            })),
        )
        .await;
        assert_eq!(created.status(), 201);
        let created: serde_json::Value = created.json().await.unwrap();
        assert_eq!(created["signer"], "operator-007");
        let id = created["id"].as_str().unwrap().to_owned();

        let listed: serde_json::Value = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "G/class-authorizations",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(listed[0]["id"], id.as_str());

        let revoked = call(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            &format!("G/class-authorizations/{id}"),
            None,
        )
        .await;
        assert_eq!(revoked.status(), 200);
        let revoked: serde_json::Value = revoked.json().await.unwrap();
        assert_eq!(revoked["revoked"]["by"], "operator-007");
        let stored = served.registry.class_authorizations("G").unwrap();
        assert!(stored[0].revoked.is_some());

        for bad in [
            serde_json::json!({"kinds": [], "target_pattern": "*", "agents": ["*"], "expires_in_secs": 60}),
            serde_json::json!({"kinds": ["push"], "target_pattern": "*", "expires_in_secs": 60}),
            serde_json::json!({"kinds": ["push"], "target_pattern": "*", "agents": ["*"], "expires_in_secs": 0}),
            serde_json::json!({"kinds": ["push"], "target_pattern": "*", "agents": ["*"], "expires_in_secs": 60, "signer": "m"}),
        ] {
            let refused = call(
                &served,
                &served.operator,
                reqwest::Method::POST,
                "G/class-authorizations",
                Some(bad.clone()),
            )
            .await;
            assert!(refused.status().is_client_error(), "{bad}");
        }
        let denied = call(
            &served,
            &served.viewer,
            reqwest::Method::DELETE,
            &format!("G/class-authorizations/{id}"),
            None,
        )
        .await;
        assert_eq!(denied.status(), 403);
        let audit = served.registry.audit_records().unwrap();
        for action in ["class_authorization.created", "class_authorization.revoked"] {
            assert!(
                audit
                    .iter()
                    .any(|record| record.action == action && record.target == id)
            );
        }
    }
}
