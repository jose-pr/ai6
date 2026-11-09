//! Operator verbs that end what a Station will not: stopping a Sleeper from minting briefs,
//! and settling a claimed Mission whose Station no longer answers.
//!
//! Both need `mission.control` on the Operation and are audited.

use ai6::AuditRecord;
use ai6::directorates::registry::backend::*;
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, Gateway, authorize_operation_action, bearer, resolve_user, unix_timestamp};

/// What a Sleeper is now.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SleeperState {
    /// The Operation.
    pub operation_id: String,
    /// The Sleeper's name.
    pub name: String,
    /// `disabled` or `enabled`.
    pub state: &'static str,
}

fn valid_name(name: &str) -> Result<(), ApiError> {
    if name.trim().is_empty()
        || name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        return Err(ApiError::bad_request());
    }
    Ok(())
}

async fn audit(
    registry: std::sync::Arc<dyn RegistryBackend>,
    record: AuditRecord,
) -> Result<(), ApiError> {
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())
}

async fn set_disabled(
    gateway: Gateway,
    headers: HeaderMap,
    (operation_id, name): (String, String),
    disabled: bool,
) -> Result<Json<SleeperState>, ApiError> {
    valid_name(&name)?;
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation, "mission.control").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let at = unix_timestamp()?;
    let (store, operation_key, sleeper, by) = (
        registry.clone(),
        operation.to_string(),
        name.clone(),
        person.id.clone(),
    );
    let found = tokio::task::spawn_blocking(move || {
        store.set_sleeper_disabled(&operation_key, &sleeper, disabled, &by, at as i64)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    if !found {
        return Err(ApiError::not_found());
    }
    let state = if disabled { "disabled" } else { "enabled" };
    audit(
        registry,
        AuditRecord {
            id: Uuid::new_v4().to_string(),
            actor: person.id,
            action: format!("sleeper.{}", if disabled { "disable" } else { "enable" }),
            operation: operation.to_string(),
            resource: None,
            target: name.clone(),
            at,
        },
    )
    .await?;
    Ok(Json(SleeperState {
        operation_id: operation.to_string(),
        name,
        state,
    }))
}

/// `POST /v1/user/operations/{operation_id}/sleepers/{name}/disable`: stops the Sleeper minting
/// briefs; its definition and authorization stay.
pub(crate) async fn disable_sleeper(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(path): Path<(String, String)>,
) -> Result<Json<SleeperState>, ApiError> {
    set_disabled(gateway, headers, path, true).await
}

/// `POST /v1/user/operations/{operation_id}/sleepers/{name}/enable`: lets it mint briefs again.
pub(crate) async fn enable_sleeper(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(path): Path<(String, String)>,
) -> Result<Json<SleeperState>, ApiError> {
    set_disabled(gateway, headers, path, false).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettleRequest {
    /// Why the Mission is settled, filed as the brief's failure reason.
    reason: String,
}

/// A claimed Mission an operator settled.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettledMission {
    /// The Operation.
    pub operation_id: String,
    /// The Mission.
    pub mission_id: String,
    /// The Station that held the claim.
    pub station_id: String,
    /// Always `failed`: the brief as filed.
    pub state: &'static str,
}

/// `POST /v1/user/operations/{operation_id}/missions/{mission_id}/settle`: files the brief of a
/// claimed Mission as failed with the operator's reason, when the Station holding the claim has
/// gone dark. A Station that still answers is asked with `mission cancel`.
pub(crate) async fn settle(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
    Json(request): Json<SettleRequest>,
) -> Result<Json<SettledMission>, ApiError> {
    let key = crate::mission_key(&gateway, &mission_id).await?;
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation, "mission.control").await?;
    let limit = gateway.config.unex().controls.reason_chars;
    if request.reason.trim().is_empty()
        || request.reason.chars().any(char::is_control)
        || !limit.allows(request.reason.chars().count())
    {
        return Err(ApiError::bad_request());
    }
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let at = unix_timestamp()?;
    let (store, wanted, by, reason) = (
        registry.clone(),
        operation.to_string(),
        person.id.clone(),
        request.reason,
    );
    let settled = tokio::task::spawn_blocking(move || {
        settle_claim(store.as_ref(), &wanted, &key, &by, &reason)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())??;
    audit(
        registry,
        AuditRecord {
            id: Uuid::new_v4().to_string(),
            actor: person.id,
            action: "mission.settle".to_owned(),
            operation: operation.to_string(),
            resource: None,
            target: settled.mission_id.clone(),
            at,
        },
    )
    .await?;
    Ok(Json(settled))
}

fn refused(message: &str) -> ApiError {
    ApiError::refused(StatusCode::CONFLICT, message.to_owned())
}

fn settle_claim(
    registry: &dyn RegistryBackend,
    operation: &str,
    key: &str,
    by: &str,
    reason: &str,
) -> std::io::Result<Result<SettledMission, ApiError>> {
    let Some(filed) = registry.filed_brief(key)? else {
        return Ok(Err(ApiError::not_found()));
    };
    if filed.brief.operation.as_deref() != Some(operation) {
        return Ok(Err(ApiError::not_found()));
    }
    if filed.state != "claimed" {
        return Ok(Err(refused("only a claimed Mission can be settled")));
    }
    let Some(lease) = registry.lease(key)? else {
        return Ok(Err(refused("the Mission has no claim")));
    };
    let live = registry
        .stations(registry.limits().stations.gone_dark_after.0)?
        .into_iter()
        .any(|record| record.station() == lease.station());
    if live {
        return Ok(Err(refused(
            "its Station still answers: use `mission cancel`",
        )));
    }
    let Some(brief) = registry.read_claimed(key)? else {
        return Ok(Err(refused("the Mission has no claim")));
    };
    let session = registry
        .latest_session(key)?
        .and_then(|record| record.provider_session_id);
    let outcome = serde_json::json!({
        "state": "failed",
        "session_id": session,
        "mission_id": key,
        "reason": reason,
        "station": lease.station(),
        "settled_by": by,
    });
    registry.finish(&brief, &outcome, false, None, Some(lease.station()))?;
    Ok(Ok(SettledMission {
        operation_id: operation.to_owned(),
        mission_id: key.to_owned(),
        station_id: lease.station().to_owned(),
        state: "failed",
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use ai6::Sleeper;
    use ai6::directorates::registry::backend::*;

    use crate::controls::tests::{Served, served};

    fn sleeper() -> Sleeper {
        Sleeper {
            schedule: "0 3 * * *".to_owned(),
            title: "Update dependencies".to_owned(),
            prompt: "Update the lockfile.".to_owned(),
            agent: None,
            role: None,
            priority: 0,
            stations: Vec::new(),
            each_station: false,
            standing: false,
            resume: false,
        }
    }

    async fn post(
        served: &Served,
        token: &str,
        path: &str,
        body: serde_json::Value,
    ) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}/v1/user/operations/{path}", served.base))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    fn claimed_by(served: &Served, id: &str, station: &str) -> String {
        let brief: ai6::Brief = serde_json::from_value(serde_json::json!({
            "title": "Crash on save",
            "body": "words",
            "id": id,
            "source": "source:github",
            "operation": "G",
        }))
        .unwrap();
        served.registry.submit_brief(&brief).unwrap();
        let key = brief.key();
        served
            .registry
            .claim_with_lease(
                Some(&key),
                station,
                Duration::from_secs(60),
                BTreeMap::new(),
            )
            .unwrap()
            .unwrap();
        key
    }

    #[tokio::test]
    async fn a_claim_no_station_answers_for_is_settled_and_a_live_one_is_not() {
        let served = served().await;
        let dark = claimed_by(&served, "org/repo#1", "gone-station");
        let live = claimed_by(&served, "org/repo#2", "worker");

        let refused = post(
            &served,
            &served.operator,
            &format!("G/missions/{live}/settle"),
            serde_json::json!({"reason": "poisoned"}),
        )
        .await;
        assert_eq!(refused.status(), 409, "its Station still answers");
        let viewer = post(
            &served,
            &served.viewer,
            &format!("G/missions/{dark}/settle"),
            serde_json::json!({"reason": "poisoned"}),
        )
        .await;
        assert_eq!(viewer.status(), 403);

        let settled = post(
            &served,
            &served.operator,
            &format!("G/missions/{dark}/settle"),
            serde_json::json!({"reason": "poisoned entry"}),
        )
        .await;
        assert_eq!(settled.status(), 200);
        let filed = served.registry.filed_brief(&dark).unwrap().unwrap();
        assert_eq!(filed.state, "failed");
        assert_eq!(filed.outcome.unwrap()["reason"], "poisoned entry");
        let audited = served.registry.audit_records().unwrap();
        assert!(
            audited
                .iter()
                .any(|record| record.action == "mission.settle")
        );
        let again = post(
            &served,
            &served.operator,
            &format!("G/missions/{dark}/settle"),
            serde_json::json!({"reason": "again"}),
        )
        .await;
        assert_eq!(again.status(), 409, "it is no longer claimed");
    }

    #[tokio::test]
    async fn an_operator_disables_and_enables_a_sleeper_with_an_audit() {
        let served = served().await;
        served
            .registry
            .authorize_sleeper("G", "deps", &sleeper(), "operator", 1)
            .unwrap();
        let unknown = post(
            &served,
            &served.operator,
            "G/sleepers/none/disable",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(unknown.status(), 404);
        let viewer = post(
            &served,
            &served.viewer,
            "G/sleepers/deps/disable",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(viewer.status(), 403);

        let disabled = post(
            &served,
            &served.operator,
            "G/sleepers/deps/disable",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(disabled.status(), 200);
        let stored = served.registry.sleeper_authorization("G", "deps").unwrap();
        assert!(stored.unwrap().disabled);
        let enabled = post(
            &served,
            &served.operator,
            "G/sleepers/deps/enable",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(enabled.status(), 200);
        let stored = served.registry.sleeper_authorization("G", "deps").unwrap();
        assert!(!stored.unwrap().disabled);
        let audited = served.registry.audit_records().unwrap();
        for action in ["sleeper.disable", "sleeper.enable"] {
            assert!(audited.iter().any(|record| record.action == action));
        }
    }
}
