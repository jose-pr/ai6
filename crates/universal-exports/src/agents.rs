//! The Agents of an Operation and their records: what each has done there, at which
//! clearance, and which models occupied its codename.
//!
//! Records come from the Registry, where Stations count each Mission's outcome and each
//! incident. The Agents posted to the Operation come from the Headquarters Station's
//! check-in, so an Agent with no record yet is listed too, with the clearance it works at
//! there. Reading needs `agent.read` on the Operation.
//!
//! A person with `agent.configure` may lower an Agent's clearance in the Operation or remove
//! its grant, returning it to its configured clearance; raising one is never done here: it
//! takes a green ink approval of M's promotion request. Each change is audited first.

use std::collections::BTreeMap;
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{
    AgentRecord, AgentTally, ApprovalRequest, AuditRecord, CLEARANCE_KIND, ClearanceGrant,
    ModelTenure, NO_OPERATION,
};
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

const CLEARANCES: [&str; 3] = ["desk", "field", "00"];

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
    /// Its outcomes in this Operation by the clearance it worked at.
    pub by_clearance: BTreeMap<String, AgentTally>,
    /// When its latest Mission in this Operation ended, as Unix seconds.
    pub last_mission_at: Option<f64>,
    /// The models that occupied the codename in this Operation, oldest first, with what each
    /// did here.
    pub model_history: Vec<ModelTenure>,
    /// Its retirement, from the Headquarters Station's check-in, once it is near or due.
    pub retirement: Option<RetirementNotice>,
    /// The clearance it works at in this Operation, from the Headquarters Station's check-in.
    pub clearance: Option<String>,
    /// Its configured clearance, from the same check-in.
    pub configured_clearance: Option<String>,
    /// Its clearance grant in this Operation, raised by green ink or lowered by a person.
    pub grant: Option<ClearanceGrant>,
    /// M's promotion request for it waiting for green ink, if any.
    pub promotion: Option<ApprovalRequest>,
    /// Its grant could not be read: Stations work it at `desk`, and so does this view.
    pub grant_unreadable: bool,
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
        let here = record
            .map(|record| record.in_operation(operation))
            .unwrap_or_default();
        Self {
            agent,
            posted,
            operation: here.tally,
            every_operation: record
                .map(|record| record.in_operation(NO_OPERATION).tally)
                .unwrap_or_default(),
            by_clearance: here.by_clearance,
            last_mission_at: here.last_mission_at,
            model_history: here.model_history,
            retirement: None,
            clearance: None,
            configured_clearance: None,
            grant: None,
            promotion: None,
            grant_unreadable: false,
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

    /// The clearance the Agent works at in `operation` and its configured one.
    fn clearance(&self, operation: &str, agent: &str) -> (Option<String>, Option<String>) {
        let entry = self
            .0
            .get("agent_clearances")
            .and_then(|operations| operations.get(operation))
            .and_then(|agents| agents.get(agent));
        let field = |name: &str| {
            entry
                .and_then(|entry| entry.get(name))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        (field("clearance"), field("configured"))
    }

    /// The codename `name` stands for in `operation`: a posted Agent's codename, or the one
    /// whose aliases include it alone.
    fn codename(&self, operation: &str, name: &str) -> Option<String> {
        let agents = self
            .0
            .get("agent_clearances")
            .and_then(|operations| operations.get(operation))
            .and_then(serde_json::Value::as_object)?;
        if agents.contains_key(name) {
            return Some(name.to_owned());
        }
        let mut matches = agents.iter().filter(|(_, entry)| {
            entry
                .get("aliases")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|aliases| aliases.iter().any(|alias| alias.as_str() == Some(name)))
        });
        let (codename, _) = matches.next()?;
        matches.next().is_none().then(|| codename.clone())
    }

    /// Whether the check-in says the Agent's grant in `operation` could not be read.
    fn unreadable(&self, operation: &str, agent: &str) -> bool {
        self.0
            .get("agent_clearances")
            .and_then(|operations| operations.get(operation))
            .and_then(|agents| agents.get(agent))
            .and_then(|entry| entry.get("unreadable"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
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
        let promotions = registry
            .pending_approvals(&operation, unix_timestamp()?)
            .map_err(|_| ApiError::internal())?
            .into_iter()
            .filter(is_promotion)
            .collect::<Vec<_>>();
        for view in views.values_mut() {
            view.retirement = headquarters.retirement(&view.agent);
            (view.clearance, view.configured_clearance) =
                headquarters.clearance(&operation, &view.agent);
            match registry.clearance_grant(&operation, &view.agent) {
                Ok(grant) => view.grant = grant,
                // A codename that is not a Registry key has no grant.
                Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {}
                Err(_) => {
                    view.grant_unreadable = true;
                    view.clearance = Some("desk".to_owned());
                }
            }
            if headquarters.unreadable(&operation, &view.agent) {
                view.grant_unreadable = true;
            }
            view.promotion = promotions
                .iter()
                .find(|request| request.agent == view.agent)
                .cloned();
        }
        Ok::<_, ApiError>(views.into_values().collect::<Vec<_>>())
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(views))
}

/// Whether a waiting request is M's promotion request: kind `clearance`, filed under Session
/// `m` and Mission `promotion-AGENT` for that Agent. A `clearance` request an Agent filed itself
/// is not one, and is never shown as one.
fn is_promotion(request: &ApprovalRequest) -> bool {
    request.scope.kind == CLEARANCE_KIND
        && request.session.as_str() == "m"
        && request.mission.as_str() == format!("promotion-{}", request.agent)
        && request
            .scope
            .target
            .strip_prefix(&request.agent)
            .is_some_and(|rest| rest.starts_with(':'))
}

/// The body of `PUT .../agents/{agent}/clearance`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LowerClearance {
    /// `desk`, `field` or `00`: the most the Agent may work at.
    clearance: String,
    /// Why, 1 to 512 characters.
    reason: String,
    /// Seconds it lasts; until removed when omitted.
    expires_in_secs: Option<u64>,
}

fn valid_agent(agent: &str) -> Result<(), ApiError> {
    if agent.is_empty()
        || agent.len() > 128
        || !agent
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(ApiError::bad_request());
    }
    Ok(())
}

async fn audit(
    registry: std::sync::Arc<dyn RegistryBackend>,
    actor: String,
    action: &'static str,
    operation: String,
    target: String,
    at: f64,
) -> Result<(), ApiError> {
    let record = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor,
        action: action.to_owned(),
        operation,
        resource: None,
        target,
        at,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())
}

/// The codename `name` stands for in the Operation, from the Headquarters Station's check-in.
async fn resolve(
    registry: &std::sync::Arc<dyn RegistryBackend>,
    operation: &OperationId,
    name: &str,
) -> Result<Option<String>, ApiError> {
    let (registry, operation, name) = (registry.clone(), operation.to_string(), name.to_owned());
    tokio::task::spawn_blocking(move || {
        Ok(Headquarters::read(registry.as_ref())?.codename(&operation, &name))
    })
    .await
    .map_err(|_| ApiError::internal())?
}

/// Withdraws the approval a replaced or removed grant was raised by, while it is live.
fn withdraw_raise(
    registry: &dyn RegistryBackend,
    previous: Option<&ClearanceGrant>,
    by: &str,
    at: f64,
) {
    if let Some(grant) = previous
        && let Some(approval) = grant.approval.as_deref()
    {
        // Best effort: M applies an approval once, so the removed grant stays removed either way.
        let _ = registry.revoke_approval(&grant.operation, approval, by, at);
    }
}

/// `PUT /v1/user/operations/{operation_id}/agents/{agent}/clearance`: holds the Agent at the
/// given clearance or below in the Operation, replacing any grant it had there.
pub(crate) async fn lower(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, agent)): Path<(String, String)>,
    Json(request): Json<LowerClearance>,
) -> Result<Json<ClearanceGrant>, ApiError> {
    valid_agent(&agent)?;
    let reason = request.reason.trim().to_owned();
    if !CLEARANCES.contains(&request.clearance.as_str())
        || reason.is_empty()
        || !gateway
            .config
            .unex()
            .text
            .agent_reason_chars
            .allows(reason.chars().count())
        || reason.chars().any(char::is_control)
        || request.expires_in_secs.is_some_and(|secs| {
            secs == 0
                || !gateway
                    .config
                    .unex()
                    .text
                    .green_ink_max_grant
                    .allows(std::time::Duration::from_secs(secs))
        })
    {
        return Err(ApiError::bad_request());
    }
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation_id, "agent.configure").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation_id).await?;
    // A grant is kept under the codename Stations read; an unknown name is refused.
    let agent = resolve(&registry, &operation_id, &agent)
        .await?
        .ok_or_else(|| ApiError::from(GatewayError::NotFound))?;
    let now = unix_timestamp()?;
    let mut grant = ClearanceGrant::lower(
        agent.clone(),
        operation_id.to_string(),
        request.clearance.clone(),
        person.id.clone(),
        now,
    )
    .because(reason);
    if let Some(secs) = request.expires_in_secs {
        grant = grant.expiring(now + secs as f64);
    }
    // Audited first: a change that cannot be recorded is not made.
    audit(
        registry.clone(),
        person.id.clone(),
        "agent.clearance.lowered",
        operation_id.to_string(),
        format!("{agent} {}", request.clearance),
        now,
    )
    .await?;
    let stored = grant.clone();
    tokio::task::spawn_blocking(move || {
        let previous = registry.clearance_grant(&stored.operation, &stored.agent)?;
        registry.put_clearance_grant(&stored)?;
        withdraw_raise(registry.as_ref(), previous.as_ref(), &person.id, now);
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    Ok(Json(grant))
}

/// `DELETE /v1/user/operations/{operation_id}/agents/{agent}/clearance`: removes the Agent's
/// grant there, so it works at its configured clearance; a raise is withdrawn for good.
pub(crate) async fn restore(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, agent)): Path<(String, String)>,
) -> Result<Json<ClearanceGrant>, ApiError> {
    valid_agent(&agent)?;
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation_id = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation_id, "agent.configure").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation_id).await?;
    // An Agent no longer posted keeps its codename: its grant can still be removed by it.
    let agent = resolve(&registry, &operation_id, &agent)
        .await?
        .unwrap_or(agent);
    let now = unix_timestamp()?;
    let store = registry.clone();
    let (operation, name) = (operation_id.to_string(), agent.clone());
    let current = tokio::task::spawn_blocking(move || store.clearance_grant(&operation, &name))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .ok_or_else(|| ApiError::from(GatewayError::NotFound))?;
    audit(
        registry.clone(),
        person.id.clone(),
        "agent.clearance.revoked",
        operation_id.to_string(),
        format!("{agent} {}", current.clearance),
        now,
    )
    .await?;
    let (operation, name) = (operation_id.to_string(), agent);
    let removed = tokio::task::spawn_blocking(move || {
        let removed = registry.remove_clearance_grant(&operation, &name)?;
        withdraw_raise(registry.as_ref(), removed.as_ref(), &person.id, now);
        Ok::<_, std::io::Error>(removed)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?
    .ok_or_else(|| ApiError::from(GatewayError::NotFound))?;
    Ok(Json(removed))
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
        served
            .registry
            .record_agent_outcome(
                &AgentOutcomeReport::new(
                    "007",
                    Some("L".into()),
                    AgentOutcome::Done,
                    "m-l2#1#done",
                    99.0,
                )
                .at_clearance("00")
                .with_model(Some("haiku".into())),
            )
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
        assert_eq!(
            views[0]["by_clearance"]["00"],
            serde_json::Value::Null,
            "nothing of L"
        );
        assert_eq!(views[0]["last_mission_at"], 10.0);
        assert_eq!(views[0]["model_history"].as_array().unwrap().len(), 1);
        assert_eq!(views[1]["operation"]["done"], 0);
        assert_eq!(views[0]["retirement"]["status"], "retiring");
        assert_eq!(views[1]["retirement"], serde_json::Value::Null);
    }

    /// Operator 007 may configure G's Agents and sign green ink; viewer 007 may only sign.
    const CONFIGURERS: &str = r#"
        [role.configurer]
        actions = ["agent.read", "agent.configure", "approval.decide", "mission.read"]

        [role.signer]
        actions = ["approval.decide", "mission.read"]

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "configurer"

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "signer"
    "#;

    async fn call(
        method: reqwest::Method,
        url: String,
        token: &str,
        body: Option<serde_json::Value>,
    ) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .request(method, url)
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await.unwrap()
    }

    #[tokio::test]
    async fn a_promotion_needs_agent_configure_to_sign_and_a_person_may_only_lower() {
        let served = served_with(CONFIGURERS).await;
        served
            .registry
            .check_in(
                "hq",
                BTreeMap::from([
                    (
                        "operation_policies".to_owned(),
                        serde_json::json!({"G": {"operation": "G", "agents": [{"id": "007"}]}}),
                    ),
                    (
                        "agent_clearances".to_owned(),
                        serde_json::json!({"G": {"007": {
                            "clearance": "desk", "configured": "desk", "aliases": ["JamesBond"]
                        }}}),
                    ),
                ]),
            )
            .unwrap();
        let now = crate::unix_timestamp().ok().unwrap();
        let request = ai6::ApprovalRequest::try_new(
            "G",
            ai6::MissionId::try_new("promotion-007").unwrap(),
            ai6::SessionId::try_new("m").unwrap(),
            "007",
            ai6::ActionScope::try_new("clearance", "007:field@G").unwrap(),
            "10 Missions done",
            "007:field@G#1",
            &Default::default(),
        )
        .unwrap()
        .expiring(now, std::time::Duration::from_secs(3600));
        served.registry.request_approval(&request).unwrap();
        // 007 files a clearance request of its own; it is not M's promotion.
        let own = ai6::ApprovalRequest::try_new(
            "G",
            ai6::MissionId::try_new("mission-g").unwrap(),
            ai6::SessionId::try_new("session-g").unwrap(),
            "007",
            ai6::ActionScope::try_new("clearance", "007:00@G").unwrap(),
            "let me",
            "self",
            &Default::default(),
        )
        .unwrap()
        .expiring(now - 1.0, std::time::Duration::from_secs(3600));
        served.registry.request_approval(&own).unwrap();

        let views: serde_json::Value = agents(&served.base, &served.operator)
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(views[0]["clearance"], "desk");
        assert_eq!(views[0]["promotion"]["id"], request.id.as_str());

        let approve = format!(
            "{}/v1/user/operations/G/green-ink/{}/approve",
            served.base, request.id
        );
        let refused = call(
            reqwest::Method::POST,
            approve.clone(),
            &served.viewer,
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(
            refused.status(),
            StatusCode::FORBIDDEN,
            "approval.decide alone"
        );
        let signed = call(
            reqwest::Method::POST,
            approve,
            &served.operator,
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(signed.status(), StatusCode::OK);

        // M raises 007 with it; revoking the approval takes the raise back, audited.
        let raise = ai6::ClearanceGrant::raise(
            "007",
            "G",
            "field",
            request.id.clone(),
            "operator-007",
            now,
        )
        .for_occupant(None, "desk");
        served.registry.put_clearance_grant(&raise).unwrap();
        let revoked = call(
            reqwest::Method::POST,
            format!(
                "{}/v1/user/operations/G/green-ink/{}/revoke",
                served.base, request.id
            ),
            &served.operator,
            None,
        )
        .await;
        assert_eq!(revoked.status(), StatusCode::OK);
        assert!(
            served
                .registry
                .clearance_grant("G", "007")
                .unwrap()
                .is_none()
        );

        // An unreadable grant shows as desk, as Stations work it.
        let grants = served.directory.path().join("clearance-grants").join("G");
        std::fs::create_dir_all(&grants).unwrap();
        std::fs::write(grants.join("007.json"), b"{ not a grant").unwrap();
        let views: serde_json::Value = agents(&served.base, &served.operator)
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(views[0]["grant_unreadable"], true);
        assert_eq!(views[0]["clearance"], "desk");
        std::fs::remove_file(grants.join("007.json")).unwrap();

        let clearance = format!("{}/v1/user/operations/G/agents/007/clearance", served.base);
        let lower = serde_json::json!({"clearance": "desk", "reason": "rewriting history"});
        let refused = call(
            reqwest::Method::PUT,
            clearance.clone(),
            &served.viewer,
            Some(lower.clone()),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let bad = call(
            reqwest::Method::PUT,
            clearance.clone(),
            &served.operator,
            Some(serde_json::json!({"clearance": "cosmic", "reason": "x"})),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        let unknown = call(
            reqwest::Method::PUT,
            format!(
                "{}/v1/user/operations/G/agents/nobody/clearance",
                served.base
            ),
            &served.operator,
            Some(lower.clone()),
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        let lowered = call(
            reqwest::Method::PUT,
            format!(
                "{}/v1/user/operations/G/agents/JamesBond/clearance",
                served.base
            ),
            &served.operator,
            Some(lower),
        )
        .await;
        assert_eq!(lowered.status(), StatusCode::OK, "by alias");
        let grant = served
            .registry
            .clearance_grant("G", "007")
            .unwrap()
            .unwrap();
        assert!(!grant.raises(), "a person's grant only lowers");
        assert_eq!(grant.signer, "operator-007");

        let removed = call(
            reqwest::Method::DELETE,
            clearance.clone(),
            &served.operator,
            None,
        )
        .await;
        assert_eq!(removed.status(), StatusCode::OK);
        assert!(
            served
                .registry
                .clearance_grant("G", "007")
                .unwrap()
                .is_none()
        );
        let again = call(reqwest::Method::DELETE, clearance, &served.operator, None).await;
        assert_eq!(again.status(), StatusCode::NOT_FOUND);
        let actions: Vec<_> = served
            .registry
            .list_audit(20)
            .unwrap()
            .into_iter()
            .map(|record| record.action)
            .collect();
        for action in [
            "agent.clearance.lowered",
            "agent.clearance.revoked",
            "green_ink.approved",
        ] {
            assert!(
                actions.contains(&action.to_owned()),
                "{action}: {actions:?}"
            );
        }
    }
}
