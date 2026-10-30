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
