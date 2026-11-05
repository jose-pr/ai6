//! Green ink through Universal Exports: the requests waiting for a person, the decision on
//! each, and class authorisations (standing approvals of a kind of step).
//!
//! Decisions are written to the shared Registry; the Station holding the Mission reads them
//! and tells the Agent. The signer is always the authenticated person, never request text.
//! Deciding needs `approval.decide` on the Operation; class authorisations need
//! `approval.class`, and approving a promotion (kind `clearance`) also `agent.configure`.
//! Neither is implied by `mission.create` or `mission.control`. Each change
//! is audited with the step it concerns, never the Agent's reason.

use std::sync::Arc;
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{
    Approval, ApprovalRequest, ApprovalStatus, AuditRecord, CLEARANCE_KIND, ClassAuthorization,
    DEFAULT_APPROVAL_TTL, Decision, Rearming, Revocation, Ruling, TimelineEvent,
};
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    ApiError, Gateway, GatewayError, authorize_operation_action, bearer,
    ensure_operation_registered, resolve_user, unix_timestamp,
};

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
pub(crate) struct RevokeRequest {
    /// Why, for the record.
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RearmRequest {
    /// Seconds it waits for a decision again; as long as it first waited when omitted.
    expires_in_secs: Option<u64>,
    /// Why, for the record.
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpiredQuery {
    /// The earliest expiry to list, as Unix seconds; `[text] green_ink_expired_window` ago when
    /// omitted.
    since: Option<f64>,
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
    /// For kind `proposal`: roles a proposed Mission may ask for besides its parent's own.
    #[serde(default)]
    proposed_roles: Vec<String>,
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

pub(crate) fn grant(seconds: u64, most: ai6::TimeLimit) -> Result<f64, ApiError> {
    if seconds == 0 || !most.allows(Duration::from_secs(seconds)) {
        return Err(ApiError::bad_request());
    }
    Ok(seconds as f64)
}

fn note(text: Option<String>, chars: ai6::Limit) -> Result<Option<String>, ApiError> {
    let Some(text) = text.map(|text| text.trim().to_owned()) else {
        return Ok(None);
    };
    if text.is_empty()
        || !chars.allows(text.chars().count())
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

/// An audit target naming a request and its step: `ID KIND TARGET`.
pub(crate) fn step_target(request: &ApprovalRequest) -> String {
    format!(
        "{} {} {}",
        request.id, request.scope.kind, request.scope.target
    )
}

/// An audit target naming a class authorisation and what it covers: `ID KINDS PATTERN`.
fn class_target(record: &ClassAuthorization) -> String {
    format!(
        "{} {} {}",
        record.id,
        record.kinds.join(","),
        record.target_pattern
    )
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

/// `GET /v1/user/operations/{operation_id}/green-ink/sources`: where the Operation's Sources are
/// read on the web, so the page can link what a reason names. Empty on a gateway with no roster.
pub(crate) async fn sources(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<Vec<ai6::SourcePage>>, ApiError> {
    let (_, operation, _) = authorized(&gateway, &headers, operation_id, "approval.decide").await?;
    let Some(roster) = gateway.roster.clone() else {
        return Ok(Json(Vec::new()));
    };
    let wanted = operation.to_string();
    let pages = tokio::task::spawn_blocking(move || {
        let snapshot = roster.load("").ok()?;
        Some(ai6::source_pages(
            &snapshot.roster.operation(&wanted).ok()?.sources,
        ))
    })
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(Json(pages.unwrap_or_default()))
}

/// `GET /v1/user/operations/{operation_id}/green-ink/expired?since=`: the requests that expired
/// undecided, the newest first. Re-arming one takes it out of the list.
pub(crate) async fn expired(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    Query(query): Query<ExpiredQuery>,
) -> Result<Json<Vec<Approval>>, ApiError> {
    if query.since.is_some_and(|since| !since.is_finite()) {
        return Err(ApiError::bad_request());
    }
    let (_, operation, registry) =
        authorized(&gateway, &headers, operation_id, "approval.decide").await?;
    let now = unix_timestamp()?;
    let window = gateway.config.unex().text.green_ink_expired_window;
    let since = query.since.unwrap_or_else(|| {
        window
            .get()
            .map_or(0.0, |window| now - window.as_secs_f64())
    });
    let operation = operation.to_string();
    let requests =
        tokio::task::spawn_blocking(move || registry.expired_approvals(&operation, since, now))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(registry_error)?;
    Ok(Json(requests))
}

/// `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/rearm`: re-opens a request
/// that expired undecided, under `approval.decide`. Its Mission's Station takes it up and the
/// person deciding it signs as for any other. Audited as `green_ink.rearmed`, on the timeline.
pub(crate) async fn rearm(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, request_id)): Path<(String, String)>,
    Json(request): Json<RearmRequest>,
) -> Result<Json<ApprovalRequest>, ApiError> {
    valid_id(&request_id)?;
    let lasts = request
        .expires_in_secs
        .map(|seconds| grant(seconds, gateway.config.unex().text.green_ink_max_grant))
        .transpose()?;
    let note = note(
        request.note,
        gateway.config.unex().text.green_ink_note_chars,
    )?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "approval.decide").await?;
    let now = unix_timestamp()?;
    let (store, key, id) = (registry.clone(), operation.to_string(), request_id.clone());
    let status = tokio::task::spawn_blocking(move || store.approval(&key, &id, now))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?;
    let lapsed = match status {
        Some(ApprovalStatus::Decided(approval)) if approval.decision == Decision::Expired => {
            approval
        }
        Some(_) => return Err(ApiError::from(GatewayError::Conflict)),
        None => return Err(ApiError::not_found()),
    };
    let first_wait = lapsed.request.expires_at - lapsed.request.requested_at;
    let target = step_target(&lapsed.request);
    // Audited first: a re-arming that cannot be recorded is not made.
    audit(
        registry.clone(),
        person.id.clone(),
        "green_ink.rearmed",
        &operation,
        target,
        now,
    )
    .await?;
    let rearming = Rearming::new(person.id.clone(), now, note);
    let (store, key) = (registry.clone(), operation.to_string());
    let reopened = tokio::task::spawn_blocking(move || {
        store.rearm_approval(
            &key,
            &request_id,
            &rearming,
            now + lasts.unwrap_or(first_wait),
        )
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(registry_error)?;
    let event = TimelineEvent::new("green_ink", "rearmed", operation.to_string(), now)
        .mission(reopened.mission.to_string())
        .session(reopened.session.to_string())
        .actor(person.id)
        .detail("request", reopened.id.clone())
        .detail("step", reopened.scope.kind.clone())
        .detail("target", reopened.scope.target.clone());
    // A timeline entry that fails is left out; the audit record and the request stand.
    let _ = tokio::task::spawn_blocking(move || registry.record_event(&event)).await;
    Ok(Json(reopened))
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
        .map_or(Ok(DEFAULT_APPROVAL_TTL.as_secs_f64()), |seconds| {
            grant(seconds, gateway.config.unex().text.green_ink_max_grant)
        })?;
    let note = note(
        request.note,
        gateway.config.unex().text.green_ink_note_chars,
    )?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "approval.decide").await?;
    let now = unix_timestamp()?;
    // Approving a promotion raises an Agent's clearance: it also takes `agent.configure`.
    let store = registry.clone();
    let (operation_key, id) = (operation.to_string(), request_id.clone());
    let status = tokio::task::spawn_blocking(move || store.approval(&operation_key, &id, now))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?;
    if status.is_some_and(|status| {
        matches!(
            status.request().scope.kind.as_str(),
            CLEARANCE_KIND | ai6::HOLD_KIND
        )
    }) {
        authorize_operation_action(&gateway, person.clone(), &operation, "agent.configure").await?;
    }
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
    let reason = note(
        request.reason,
        gateway.config.unex().text.green_ink_note_chars,
    )?;
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
    let target = step_target(&approval.request);
    audit(registry, person.id, action, &operation, target, at).await?;
    Ok(Json(approval))
}

/// `POST /v1/user/operations/{operation_id}/green-ink/{request_id}/revoke` with an optional
/// `{"reason"}`: withdraws an approval before it expires. The Station revokes a credential leased
/// against it.
pub(crate) async fn revoke(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, request_id)): Path<(String, String)>,
    request: Option<Json<RevokeRequest>>,
) -> Result<Json<Approval>, ApiError> {
    valid_id(&request_id)?;
    let reason = note(
        request.and_then(|Json(request)| request.reason),
        gateway.config.unex().text.green_ink_note_chars,
    )?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "approval.decide").await?;
    let now = unix_timestamp()?;
    let store = registry.clone();
    let operation_key = operation.to_string();
    let id = request_id.clone();
    let status = tokio::task::spawn_blocking(move || store.approval(&operation_key, &id, now))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?;
    let approval = match status {
        Some(ApprovalStatus::Decided(approval)) if approval.decision == Decision::Approved => {
            approval
        }
        Some(ApprovalStatus::Decided(_)) => return Err(ApiError::from(GatewayError::Conflict)),
        _ => return Err(ApiError::not_found()),
    };
    if approval.revoked.is_some() {
        return Ok(Json(approval));
    }
    // Audited first: a revocation that cannot be recorded is not made.
    audit(
        registry.clone(),
        person.id.clone(),
        "green_ink.revoked",
        &operation,
        step_target(&approval.request),
        now,
    )
    .await?;
    let operation_key = operation.to_string();
    let store = registry.clone();
    let revocation = Revocation::new(person.id.clone(), now, reason);
    let approval = tokio::task::spawn_blocking(move || {
        store.revoke_approval(&operation_key, &request_id, &revocation)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(registry_error)?;
    if approval.request.scope.kind == CLEARANCE_KIND {
        take_back_raise(registry, person.id, &operation, &approval, now).await?;
    } else if approval.request.scope.kind == ai6::HOLD_KIND {
        lift_signed_hold(registry, person.id, &operation, &approval, now).await?;
    }
    Ok(Json(approval))
}

/// Lifts the hold a revoked proposal approval put on, if it is still that hold; audited as
/// `agent.hold.lifted`.
async fn lift_signed_hold(
    registry: Arc<dyn RegistryBackend>,
    actor: String,
    operation: &OperationId,
    approval: &Approval,
    at: f64,
) -> Result<(), ApiError> {
    let (store, operation_key) = (registry.clone(), operation.to_string());
    let holds = tokio::task::spawn_blocking(move || store.holds(&operation_key))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?;
    for hold in holds
        .into_iter()
        .filter(|hold| hold.approval.as_deref() == Some(approval.request.id.as_str()))
    {
        audit(
            registry.clone(),
            actor.clone(),
            "agent.hold.lifted",
            operation,
            ai6::hold_target(&hold.agent, hold.route.as_ref(), operation.as_str()),
            at,
        )
        .await?;
        let store = registry.clone();
        tokio::task::spawn_blocking(move || {
            store
                .lift_hold(&hold.operation, &hold.agent, hold.route.as_ref())
                .map(|_| ())
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?;
    }
    Ok(())
}

/// Removes the clearance grant a revoked promotion approval raised, if it is still the
/// Agent's grant; audited as `agent.clearance.revoked`.
async fn take_back_raise(
    registry: Arc<dyn RegistryBackend>,
    actor: String,
    operation: &OperationId,
    approval: &Approval,
    at: f64,
) -> Result<(), ApiError> {
    let (store, operation_key, agent) = (
        registry.clone(),
        operation.to_string(),
        approval.request.agent.clone(),
    );
    let current =
        tokio::task::spawn_blocking(move || store.clearance_grant(&operation_key, &agent))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(registry_error)?;
    let Some(grant) =
        current.filter(|grant| grant.approval.as_deref() == Some(approval.request.id.as_str()))
    else {
        return Ok(());
    };
    audit(
        registry.clone(),
        actor,
        "agent.clearance.revoked",
        operation,
        format!("{} {}", grant.agent, grant.clearance),
        at,
    )
    .await?;
    tokio::task::spawn_blocking(move || {
        // Removed only while it is still that grant; a newer one stays.
        registry.remove_clearance_grant_if(&grant).map(|_| ())
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(registry_error)
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
    let lasts = grant(
        request.expires_in_secs,
        gateway.config.unex().text.green_ink_max_grant,
    )?;
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
    .for_roles(request.roles)
    .for_proposed_roles(request.proposed_roles);
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
        class_target(&record),
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
        class_target(&record),
        now,
    )
    .await?;
    Ok(Json(record))
}

#[cfg(test)]
mod tests {
    use ai6::directorates::registry::backend::*;
    use ai6::{ActionScope, ApprovalStatus, MissionId, SessionId};

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
            &Default::default(),
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
    async fn an_expired_request_lists_apart_and_is_rearmed_with_its_first_wait() {
        let served = served_with(ROLES).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let lapsed = ai6::ApprovalRequest::try_new(
            "G",
            MissionId::try_new("mission-g").unwrap(),
            SessionId::try_new("session-g").unwrap(),
            "007",
            ActionScope::try_new("push", "origin:refs/heads/main").unwrap(),
            "the release is green",
            "lapsed",
            &Default::default(),
        )
        .unwrap()
        .expiring(now - 120.0, std::time::Duration::from_secs(60));
        served.registry.request_approval(&lapsed).unwrap();

        let pending: serde_json::Value = call(
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
        assert_eq!(pending, serde_json::json!([]));
        let expired: serde_json::Value = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "G/green-ink/expired",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(expired[0]["request"]["id"], lapsed.id.as_str(), "{expired}");
        assert_eq!(expired[0]["decision"], "expired");
        let none: serde_json::Value = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            &format!("G/green-ink/expired?since={}", now + 60.0),
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(none, serde_json::json!([]));

        let path = format!("G/green-ink/{}/rearm", lapsed.id);
        let denied = call(
            &served,
            &served.viewer,
            reqwest::Method::POST,
            &path,
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(denied.status(), 403);

        let rearmed = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &path,
            Some(serde_json::json!({ "note": "back from leave" })),
        )
        .await;
        assert_eq!(rearmed.status(), 200);
        let rearmed: serde_json::Value = rearmed.json().await.unwrap();
        assert_eq!(rearmed["rearmed"]["by"], "operator-007");
        assert_eq!(rearmed["rearmed"]["note"], "back from leave");
        let waits =
            rearmed["expires_at"].as_f64().unwrap() - rearmed["rearmed"]["at"].as_f64().unwrap();
        assert!((waits - 60.0).abs() < 1.0, "{waits}");

        let pending: serde_json::Value = call(
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
        assert_eq!(pending[0]["id"], lapsed.id.as_str());
        let again = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &path,
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(again.status(), 409, "still waiting");

        let audit = served.registry.audit_records().unwrap();
        let record = audit
            .iter()
            .find(|record| record.action == "green_ink.rearmed")
            .unwrap();
        assert_eq!(record.actor, "operator-007");
        assert!(!record.target.contains("leave"), "the note stays out");
        assert!(
            served
                .registry
                .timeline_events("G")
                .unwrap()
                .iter()
                .any(|event| event.kind == "green_ink" && event.action == "rearmed")
        );

        let approved = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/green-ink/{}/approve", lapsed.id),
            Some(serde_json::json!({ "expires_in_secs": 60 })),
        )
        .await;
        assert_eq!(approved.status(), 200, "decidable again");
    }

    async fn approved(served: &Served, id: &str) {
        let path = format!("G/green-ink/{id}/approve");
        let signed = call(
            served,
            &served.operator,
            reqwest::Method::POST,
            &path,
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(signed.status(), 200);
    }

    #[tokio::test]
    async fn a_revoke_keeps_its_reason_and_needs_no_body() {
        let served = served_with(ROLES).await;
        let with_reason = asked(&served, "revoke-reason", 3600);
        approved(&served, &with_reason.id).await;
        let revoked = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/green-ink/{}/revoke", with_reason.id),
            Some(serde_json::json!({ "reason": "wrong branch" })),
        )
        .await;
        assert_eq!(revoked.status(), 200);
        let revoked: serde_json::Value = revoked.json().await.unwrap();
        assert_eq!(revoked["revoked"]["by"], "operator-007");
        assert_eq!(revoked["revoked"]["note"], "wrong branch");

        let bare = asked(&served, "revoke-bare", 3600);
        approved(&served, &bare.id).await;
        let revoked = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/green-ink/{}/revoke", bare.id),
            None,
        )
        .await;
        assert_eq!(revoked.status(), 200);
        let revoked: serde_json::Value = revoked.json().await.unwrap();
        assert!(revoked["revoked"].get("note").is_none(), "{revoked}");
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
        let approved_audit = audit
            .iter()
            .find(|record| record.action == "green_ink.approved")
            .unwrap();
        assert_eq!(approved_audit.actor, "operator-007");
        assert_eq!(
            approved_audit.target,
            format!("{} push origin:refs/heads/main", request.id),
            "the step, never the reason"
        );
        assert!(!approved_audit.target.contains("release is green"));

        let revoked = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/green-ink/{}/revoke", request.id),
            None,
        )
        .await;
        assert_eq!(revoked.status(), 200);
        let revoked: serde_json::Value = revoked.json().await.unwrap();
        assert_eq!(revoked["revoked"]["by"], "operator-007");
        assert!(
            served
                .registry
                .live_approval(
                    "G",
                    "mission-g",
                    &request.scope,
                    approved["at"].as_f64().unwrap()
                )
                .unwrap()
                .is_none(),
            "a revoked approval covers nothing"
        );
        assert!(
            served
                .registry
                .audit_records()
                .unwrap()
                .iter()
                .any(|record| record.action == "green_ink.revoked"
                    && record.actor == "operator-007"
                    && record.target.starts_with(&request.id))
        );
        let other = asked(&served, "push-4", 3600);
        let undecided = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/green-ink/{}/revoke", other.id),
            None,
        )
        .await;
        assert_eq!(
            undecided.status(),
            404,
            "only a decided request can be revoked"
        );
    }

    #[tokio::test]
    async fn a_revocation_that_cannot_be_audited_is_not_made() {
        let served = served_with(ROLES).await;
        let request = asked(&served, "push-5", 3600);
        let approved = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/green-ink/{}/approve", request.id),
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(approved.status(), 200);
        // The audit directory becomes a file, so no audit record can be written.
        let audit = served.registry.root().join("audit");
        std::fs::remove_dir_all(&audit).unwrap();
        std::fs::write(&audit, b"not a directory").unwrap();
        let revoked = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            &format!("G/green-ink/{}/revoke", request.id),
            None,
        )
        .await;
        assert_eq!(revoked.status(), 500);
        let Some(ApprovalStatus::Decided(approval)) = served
            .registry
            .approval("G", &request.id, request.requested_at)
            .unwrap()
        else {
            panic!("the request is decided");
        };
        assert!(
            approval.revoked.is_none(),
            "nothing was revoked without its record"
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
                reqwest::Method::POST,
                format!("G/green-ink/{}/revoke", request.id),
                None,
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

        let proposals = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            "G/class-authorizations",
            Some(serde_json::json!({
                "kinds": ["proposal"],
                "target_pattern": "Update the *",
                "roles": ["implement"],
                "proposed_roles": ["review"],
                "expires_in_secs": 3600
            })),
        )
        .await;
        assert_eq!(proposals.status(), 201);
        let proposals: serde_json::Value = proposals.json().await.unwrap();
        assert_eq!(proposals["proposed_roles"], serde_json::json!(["review"]));

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
                audit.iter().any(|record| record.action == action
                    && record.target == format!("{id} push origin:refs/heads/ai6/*")),
                "{action}"
            );
        }
    }
}
