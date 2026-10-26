//! The Agents of an Operation and their records: what each has done there, at which
//! clearance, and which models occupied its codename.
//!
//! Records come from the Registry, where Stations count each Mission's outcome and each
//! incident. The Agents posted to the Operation come from the Headquarters Station's
//! check-in, so an Agent with no record yet is listed too. Reading needs `agent.read` on the
//! Operation.

use std::collections::BTreeMap;
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{AgentRecord, AgentTally, ModelTenure, NO_OPERATION};
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::{
    ApiError, Gateway, authorize_operation_action, bearer, ensure_operation_registered,
    resolve_user,
};

/// One Agent of an Operation and its record there.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentView {
    /// The Agent codename.
    pub agent: String,
    /// Whether the Headquarters Station's roster posts it to the Operation.
    pub posted: bool,
    /// Its outcomes in this Operation.
    pub operation: AgentTally,
    /// Its outcomes outside any Operation, and quarantines in every Operation.
    pub every_operation: AgentTally,
    /// Its outcomes by the clearance it worked at, in every Operation.
    pub by_clearance: BTreeMap<String, AgentTally>,
    /// When its latest Mission ended, as Unix seconds.
    pub last_mission_at: Option<f64>,
    /// The models that occupied the codename, oldest first.
    pub model_history: Vec<ModelTenure>,
    /// Its retirement, from the Headquarters Station's check-in, once it is near or due.
    pub retirement: Option<RetirementNotice>,
}

/// An Agent whose `retire_on` date is near or has come.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetirementNotice {
    /// The UTC date, `YYYY-MM-DD`.
    pub retire_on: String,
    /// `desk` or `retired`.
    pub on_retire: String,
    /// `retiring` (within the warning window), `desk_duty` or `retired`.
    pub status: String,
}

impl AgentView {
    fn new(agent: String, posted: bool, record: Option<&AgentRecord>, operation: &str) -> Self {
        Self {
            agent,
            posted,
            operation: record
                .map(|record| record.in_operation(operation))
                .unwrap_or_default(),
            every_operation: record
                .map(|record| record.in_operation(NO_OPERATION))
                .unwrap_or_default(),
            by_clearance: record
                .map(|record| record.by_clearance.clone())
                .unwrap_or_default(),
            last_mission_at: record.and_then(|record| record.last_mission_at),
            model_history: record
                .map(|record| record.model_history.clone())
                .unwrap_or_default(),
            retirement: None,
        }
    }
}

/// What the Headquarters Station's check-in says of the roster.
#[derive(Default)]
struct Headquarters(BTreeMap<String, serde_json::Value>);

impl Headquarters {
    fn read(registry: &dyn RegistryBackend) -> Result<Self, ApiError> {
        let Some(headquarters) = registry.headquarters().map_err(|_| ApiError::internal())? else {
            return Ok(Self::default());
        };
        let stations = registry
            .stations(Duration::MAX)
            .map_err(|_| ApiError::internal())?;
        Ok(stations
            .into_iter()
            .find(|station| station.station() == headquarters.station())
            .map(|station| Self(station.info().clone()))
            .unwrap_or_default())
    }

    /// The Agents posted to `operation`.
    fn posted(&self, operation: &str) -> Vec<String> {
        self.0
            .get("operation_policies")
            .and_then(|policies| policies.get(operation))
            .and_then(|policy| policy.get("agents"))
            .and_then(serde_json::Value::as_array)
            .map(|agents| {
                agents
                    .iter()
                    .filter_map(|agent| agent.get("id").and_then(serde_json::Value::as_str))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The Agent's retirement notice, if the check-in lists one.
    fn retirement(&self, agent: &str) -> Option<RetirementNotice> {
        self.0
            .get("retirements")
            .and_then(serde_json::Value::as_array)?
            .iter()
            .find(|notice| notice.get("agent").and_then(serde_json::Value::as_str) == Some(agent))
            .and_then(|notice| serde_json::from_value(notice.clone()).ok())
    }
}

/// `GET /v1/user/operations/{operation_id}/agents`: every Agent posted to the Operation or
/// with a record there, by codename.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<Vec<AgentView>>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person, &operation_id, "agent.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation_id).await?;
    let operation = operation_id.to_string();
    let views = tokio::task::spawn_blocking(move || {
        let headquarters = Headquarters::read(registry.as_ref())?;
        let posted = headquarters.posted(&operation);
        let records = registry.agent_records().map_err(|_| ApiError::internal())?;
        let mut views = BTreeMap::new();
        for agent in posted {
            let record = records.iter().find(|record| record.agent == agent);
            views.insert(
                agent.clone(),
                AgentView::new(agent, true, record, &operation),
            );
        }
        for record in &records {
            if !views.contains_key(&record.agent) && record.by_operation.contains_key(&operation) {
                views.insert(
                    record.agent.clone(),
                    AgentView::new(record.agent.clone(), false, Some(record), &operation),
                );
            }
        }
        for view in views.values_mut() {
            view.retirement = headquarters.retirement(&view.agent);
        }
        Ok::<_, ApiError>(views.into_values().collect::<Vec<_>>())
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(views))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ai6::directorates::registry::backend::*;
    use ai6::{AgentOutcome, AgentOutcomeReport};
    use axum::http::StatusCode;

    use crate::controls::tests::served_with;

    /// Operator 007 may read G's Agents; viewer 007 may read its Missions only.
    const READERS: &str = r#"
        [role.agent-reader]
        actions = ["agent.read", "mission.read"]

        [role.viewer]
        actions = ["mission.read"]

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "agent-reader"

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "viewer"
    "#;

    async fn agents(base: &str, token: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(format!("{base}/v1/user/operations/G/agents"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_record_is_returned_only_to_agent_read() {
        let served = served_with(READERS).await;
        served
            .registry
            .check_in(
                "hq",
                BTreeMap::from([
                    (
                        "operation_policies".to_owned(),
                        serde_json::json!({"G": {"operation": "G", "agents": [{"id": "007"}, {"id": "008"}]}}),
                    ),
                    (
                        "retirements".to_owned(),
                        serde_json::json!([{"agent": "007", "retire_on": "2027-01-31", "on_retire": "desk", "status": "retiring"}]),
                    ),
                ]),
            )
            .unwrap();
        for (key, outcome) in [
            ("m-1#3", AgentOutcome::Done),
            ("m-2#3", AgentOutcome::Done),
            ("m-3#2", AgentOutcome::Stalled),
        ] {
            served
                .registry
                .record_agent_outcome(
                    &AgentOutcomeReport::new("007", Some("G".into()), outcome, key, 10.0)
                        .at_clearance("field"),
                )
                .unwrap();
        }
        served
            .registry
            .record_agent_outcome(&AgentOutcomeReport::new(
                "009",
                Some("L".into()),
                AgentOutcome::Failed,
                "m-l#2",
                11.0,
            ))
            .unwrap();

        let response = agents(&served.base, &served.viewer).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = agents(&served.base, &served.operator).await;
        assert_eq!(response.status(), StatusCode::OK);
        let views: serde_json::Value = response.json().await.unwrap();
        let ids: Vec<_> = views
            .as_array()
            .unwrap()
            .iter()
            .map(|view| view["agent"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            ["007", "008"],
            "another Operation's Agent is not listed"
        );
        assert_eq!(views[0]["operation"]["done"], 2);
        assert_eq!(views[0]["operation"]["stalled"], 1);
        assert_eq!(views[0]["by_clearance"]["field"]["done"], 2);
        assert_eq!(views[1]["operation"]["done"], 0);
        assert_eq!(views[0]["retirement"]["status"], "retiring");
        assert_eq!(views[1]["retirement"], serde_json::Value::Null);
    }
}
