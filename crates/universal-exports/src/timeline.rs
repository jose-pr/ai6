//! The Operation activity timeline: one authorized, redacted, cursor-paginated view of what
//! changed across an Operation.
//!
//! Items come from two durable sources in the shared Registry: the activity events
//! (`ai6::TimelineEvent`: lifecycle transitions, control outcomes, Cables sent, Sleeper
//! authorizations) and the audit records (Mission requests and controls, proposal
//! decisions, Agent role, quarantine and Station settings changes). Live terminal frames
//! are never part of it.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use ai6::{AuditRecord, AuthorizationRequest, TimelineEvent};
use ai6_diptel::{MissionId, OperationId};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::controls::ControlStatus;
use crate::{ApiError, Gateway, authorize_operation_action, bearer, resolve_user, unix_timestamp};

/// Items in a page unless the request asks for fewer or more.
pub const DEFAULT_TIMELINE_PAGE: usize = 50;
/// The most items one page returns.
pub const MAX_TIMELINE_PAGE: usize = 200;
const MAX_TEXT_CHARS: usize = 256;

/// Audit actions whose target is a Mission ID.
const MISSION_AUDITS: [&str; 5] = [
    "mission.create.requested",
    "mission.cancel",
    "mission.retry",
    "mission.resume",
    "session.interrupt",
];

/// One entry of an Operation's activity timeline: metadata only.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineItem {
    /// Stable identity: `event:<id>` for an activity event, `audit:<id>` for an audit record.
    pub id: String,
    /// When it happened, as Unix seconds.
    pub at: f64,
    /// `lifecycle`, `control`, `cable`, `sleeper` or `audit`.
    pub kind: String,
    /// What happened: a Mission state, `<action>.<outcome>`, `sent`, `authorized`,
    /// `revoked`, or the audit action.
    pub action: String,
    /// The Operation whose timeline this is.
    pub operation_id: String,
    /// The Station involved, when known.
    pub station_id: Option<String>,
    /// The Mission involved, when there is one.
    pub mission_id: Option<String>,
    /// The provider Session involved, when there is one.
    pub session_id: Option<String>,
    /// Who acted: a person ID or an Agent codename.
    pub actor: Option<String>,
    /// An audit record's target: a Mission, proposal, role, Agent or setting name.
    pub target: Option<String>,
    /// Further safe identifiers and names.
    pub detail: BTreeMap<String, String>,
}

/// One page of a timeline, newest first.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelinePage {
    /// The Operation.
    pub operation_id: String,
    /// At most the requested number of items, newest first.
    pub items: Vec<TimelineItem>,
    /// Pass back as `cursor` for the next, older page; absent on the last page.
    pub next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TimelineQuery {
    cursor: Option<String>,
    mission: Option<String>,
    limit: Option<usize>,
}

/// Text from a record as the timeline shows it: no control characters, bounded.
fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_TEXT_CHARS)
        .collect()
}

fn clean_option(value: Option<&str>) -> Option<String> {
    value.map(clean).filter(|value| !value.is_empty())
}

fn from_event(event: TimelineEvent) -> TimelineItem {
    TimelineItem {
        id: format!("event:{}", event.id),
        at: event.at,
        kind: clean(&event.kind),
        action: clean(&event.action),
        operation_id: clean(&event.operation),
        station_id: clean_option(event.station.as_deref()),
        mission_id: clean_option(event.mission.as_deref()),
        session_id: clean_option(event.session.as_deref()),
        actor: clean_option(event.actor.as_deref()),
        target: None,
        detail: event
            .detail
            .iter()
            .map(|(key, value)| (clean(key), clean(value)))
            .collect(),
    }
}

fn from_audit(record: &AuditRecord, operation: &str) -> TimelineItem {
    TimelineItem {
        id: format!("audit:{}", record.id),
        at: record.at,
        kind: "audit".to_owned(),
        action: clean(&record.action),
        operation_id: operation.to_owned(),
        station_id: clean_option(
            record
                .resource
                .as_deref()
                .and_then(|resource| resource.strip_prefix("station:")),
        ),
        mission_id: MISSION_AUDITS
            .contains(&record.action.as_str())
            .then(|| clean(&record.target)),
        session_id: None,
        actor: clean_option(Some(&record.actor)),
        target: clean_option(Some(&record.target)),
        detail: BTreeMap::new(),
    }
}

/// An opaque position: the item's time bits and identity.
fn cursor_of(item: &TimelineItem) -> String {
    format!("{:016x}.{}", item.at.to_bits(), item.id)
}

fn parse_cursor(cursor: &str) -> Option<(f64, String)> {
    let (bits, id) = cursor.split_once('.')?;
    if bits.len() != 16 || id.is_empty() || id.len() > 300 {
        return None;
    }
    let at = f64::from_bits(u64::from_str_radix(bits, 16).ok()?);
    at.is_finite().then(|| (at, id.to_owned()))
}

/// Newest first; ties broken by identity so the order never changes between pages.
fn newer_first(left: &TimelineItem, right: &TimelineItem) -> std::cmp::Ordering {
    right
        .at
        .total_cmp(&left.at)
        .then_with(|| right.id.cmp(&left.id))
}

pub(crate) async fn operation_timeline(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    Query(query): Query<TimelineQuery>,
) -> Result<Json<TimelinePage>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    let limit = query.limit.unwrap_or(DEFAULT_TIMELINE_PAGE);
    if limit == 0 || limit > MAX_TIMELINE_PAGE {
        return Err(ApiError::bad_request());
    }
    let mission = query
        .mission
        .map(MissionId::new)
        .transpose()
        .map_err(|_| ApiError::bad_request())?
        .map(|mission| mission.to_string());
    let cursor = query
        .cursor
        .as_deref()
        .map(|cursor| parse_cursor(cursor).ok_or_else(ApiError::bad_request))
        .transpose()?;
    authorize_operation_action(&gateway, person.clone(), &operation, "audit.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let authorizer = gateway
        .access
        .as_ref()
        .ok_or_else(ApiError::unavailable)?
        .authorizer
        .clone();
    let operation = operation.to_string();
    let page = tokio::task::spawn_blocking(move || {
        let mut items = registry
            .timeline_events(&operation)
            .map_err(|_| ApiError::internal())?
            .into_iter()
            .map(from_event)
            .collect::<Vec<_>>();
        let hosts = registry
            .stations(Duration::MAX)
            .map_err(|_| ApiError::internal())?
            .into_iter()
            .filter(|station| {
                station
                    .info()
                    .get("operations")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|operations| {
                        operations
                            .iter()
                            .any(|value| value.as_str() == Some(&operation))
                    })
            })
            .map(|station| format!("station:{}", station.station()))
            .collect::<BTreeSet<_>>();
        let mut station_access = BTreeMap::new();
        for record in registry.audit_records().map_err(|_| ApiError::internal())? {
            let visible = if record.operation == operation {
                true
            } else if let Some(resource) = record.resource.as_ref()
                && record.operation.is_empty()
                && hosts.contains(resource)
            {
                // A Station's own records are shown only to who may read that Station's.
                match station_access.get(resource) {
                    Some(allowed) => *allowed,
                    None => {
                        let allowed = authorizer
                            .authorize(AuthorizationRequest {
                                person: person.clone(),
                                action: "audit.read".to_owned(),
                                resource: resource.clone(),
                                ancestors: Vec::new(),
                            })
                            .map_err(|_| ApiError::internal())?
                            .allowed;
                        station_access.insert(resource.clone(), allowed);
                        allowed
                    }
                }
            } else {
                false
            };
            if visible {
                items.push(from_audit(&record, &operation));
            }
        }
        if let Some(mission) = &mission {
            items.retain(|item| item.mission_id.as_ref() == Some(mission));
        }
        items.sort_by(newer_first);
        if let Some((at, id)) = &cursor {
            items.retain(|item| {
                item.at
                    .total_cmp(at)
                    .then_with(|| item.id.as_str().cmp(id.as_str()))
                    .is_lt()
            });
        }
        let next_cursor = (items.len() > limit).then(|| cursor_of(&items[limit - 1]));
        items.truncate(limit);
        Ok::<_, ApiError>(TimelinePage {
            operation_id: operation,
            items,
            next_cursor,
        })
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(page))
}

/// Records a control's outcome in its Operation's timeline; the control stands either way.
pub(crate) async fn record_control(gateway: &Gateway, status: &ControlStatus) {
    let Some(registry) = gateway.registry.clone() else {
        return;
    };
    let Ok(at) = unix_timestamp() else {
        return;
    };
    let mut event = TimelineEvent::new(
        "control",
        format!("{}.{}", status.action, status.status),
        status.operation_id.clone(),
        status.finished_at.unwrap_or(at),
    )
    .station(status.station_id.clone())
    .mission(status.mission_id.clone())
    .actor(status.requested_by.clone())
    .detail("control", status.control_id.clone());
    if let Some(session) = &status.session_id {
        event = event.session(session.clone());
    }
    if let Some(agent) = &status.agent {
        event = event.detail("agent", agent.clone());
    }
    if let Some(state) = &status.state {
        event = event.detail("state", state.clone());
    }
    if let Some(reason) = &status.reason {
        event = event.detail("reason", clean(reason));
    }
    let _ = tokio::task::spawn_blocking(move || registry.record_event(&event)).await;
}

/// A Session's lifecycle transition as the gateway saw it.
pub(crate) struct Transition {
    pub(crate) operation: String,
    pub(crate) mission: String,
    pub(crate) session: String,
    pub(crate) state: String,
    pub(crate) station: Option<String>,
    pub(crate) at: f64,
}

/// Records a lifecycle transition; publication stands either way.
pub(crate) async fn record_transition(gateway: &Gateway, transition: Transition) {
    let Some(registry) = gateway.registry.clone() else {
        return;
    };
    let mut event = TimelineEvent::new(
        "lifecycle",
        transition.state,
        transition.operation,
        transition.at,
    )
    .mission(transition.mission)
    .session(transition.session);
    if let Some(station) = transition.station {
        event = event.station(station);
    }
    let _ = tokio::task::spawn_blocking(move || registry.record_event(&event)).await;
}

#[cfg(test)]
mod tests {
    use ai6::directorates::registry::backend::*;
    use ai6::{AuditRecord, TimelineEvent};
    use axum::http::StatusCode;

    use crate::controls::tests::{STATION_TOKEN, Served, ask, report, served, take};

    async fn timeline(served: &Served, token: &str, query: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(format!(
                "{}/v1/user/operations/G/timeline{query}",
                served.base
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    async fn page(served: &Served, token: &str, query: &str) -> serde_json::Value {
        let response = timeline(served, token, query).await;
        assert_eq!(response.status(), StatusCode::OK);
        response.json().await.unwrap()
    }

    async fn publish(served: &Served, path: &str, body: Vec<u8>, json: bool) {
        let mut request = reqwest::Client::new()
            .post(format!(
                "{}/v1/station/sessions/session-g/{path}",
                served.base
            ))
            .bearer_auth(STATION_TOKEN)
            .body(body);
        if json {
            request = request.header("content-type", "application/json");
        }
        assert_eq!(request.send().await.unwrap().status(), StatusCode::ACCEPTED);
    }

    fn lifecycle(state: &str) -> Vec<u8> {
        serde_json::json!({"operation_id": "G", "mission_id": "mission-g", "state": state})
            .to_string()
            .into_bytes()
    }

    fn ids(page: &serde_json::Value) -> Vec<String> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn the_timeline_correlates_lifecycle_controls_and_audit_without_content() {
        let served = served().await;
        publish(&served, "lifecycle/1", lifecycle("working"), true).await;
        publish(&served, "lifecycle/2", lifecycle("working"), true).await;
        publish(&served, "terminal/3", b"TERMINAL-SECRET".to_vec(), false).await;
        publish(&served, "lifecycle/4", lifecycle("blocked"), true).await;
        let asked = ask(
            &served,
            &served.operator,
            "G",
            "mission-g",
            serde_json::json!({
                "action": "resume",
                "message": "carry on, MESSAGE-SECRET",
                "idempotency_key": "resume-1"
            }),
        )
        .await;
        assert_eq!(asked.status(), StatusCode::ACCEPTED);
        let control = take(&served, "worker").await.unwrap();
        assert_eq!(
            report(
                &served,
                control.control_id().as_str(),
                serde_json::json!({
                    "station": "worker", "outcome": "applied", "state": "working", "reason": null
                }),
            )
            .await,
            StatusCode::NO_CONTENT
        );
        served
            .registry
            .record_event(&TimelineEvent::new("sleeper", "authorized", "L", 5.0))
            .unwrap();
        served
            .registry
            .record_audit(&AuditRecord {
                id: "settings-1".to_owned(),
                actor: "operator-007".to_owned(),
                action: "station.settings.update".to_owned(),
                operation: String::new(),
                resource: Some("station:worker".to_owned()),
                target: "max_concurrent_missions".to_owned(),
                at: 6.0,
            })
            .unwrap();

        let viewed = page(&served, &served.viewer, "").await;
        let text = viewed.to_string();
        assert!(!text.contains("SECRET"), "{text}");
        let items = viewed["items"].as_array().unwrap();
        let actions = items
            .iter()
            .map(|item| {
                (
                    item["kind"].as_str().unwrap(),
                    item["action"].as_str().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        // Newest first; a repeated state is not a second transition; another Operation's
        // events and Station records the viewer may not read stay out.
        assert_eq!(
            actions,
            [
                ("control", "resume.applied"),
                ("audit", "mission.resume"),
                ("lifecycle", "blocked"),
                ("lifecycle", "working"),
            ]
        );
        assert!(
            items
                .iter()
                .all(|item| item["mission_id"] == "mission-g" && item["operation_id"] == "G")
        );
        assert_eq!(items[0]["station_id"], "worker");
        assert_eq!(items[0]["actor"], "operator-007");
        assert_eq!(items[0]["detail"]["state"], "working");
        assert_eq!(items[2]["session_id"], "session-g");
        assert_eq!(viewed["next_cursor"], serde_json::Value::Null);

        // The operator may read the Station's records too.
        let operated = page(&served, &served.operator, "").await;
        assert!(operated["items"].as_array().unwrap().iter().any(|item| {
            item["action"] == "station.settings.update" && item["station_id"] == "worker"
        }));

        let filtered = page(&served, &served.viewer, "?mission=mission-g").await;
        assert_eq!(ids(&filtered), ids(&viewed));
        let other = page(&served, &served.viewer, "?mission=mission-x").await;
        assert!(ids(&other).is_empty());
    }

    #[tokio::test]
    async fn pages_follow_the_cursor_without_gaps_or_repeats() {
        let served = served().await;
        for (index, at) in [10.0, 20.0, 20.0, 30.0, 40.0, 50.0, 60.0]
            .into_iter()
            .enumerate()
        {
            let mut event = TimelineEvent::new("lifecycle", "working", "G", at)
                .mission("mission-g")
                .session("session-g");
            event.id = format!("event-{index}");
            served.registry.record_event(&event).unwrap();
        }
        let everything = ids(&page(&served, &served.viewer, "?limit=200").await);
        assert_eq!(everything.len(), 7);

        let mut seen = Vec::new();
        let mut query = "?limit=3".to_owned();
        loop {
            let current = page(&served, &served.viewer, &query).await;
            seen.extend(ids(&current));
            if seen.len() == 3 {
                // A newer event arriving between pages does not shift the older ones.
                served
                    .registry
                    .record_event(&TimelineEvent::new("lifecycle", "done", "G", 99.0))
                    .unwrap();
            }
            match current["next_cursor"].as_str() {
                Some(cursor) => query = format!("?limit=3&cursor={cursor}"),
                None => break,
            }
        }
        assert_eq!(seen, everything);

        for invalid in ["?limit=0", "?limit=201", "?cursor=nonsense", "?unknown=1"] {
            assert_eq!(
                timeline(&served, &served.viewer, invalid).await.status(),
                StatusCode::BAD_REQUEST,
                "{invalid}"
            );
        }
    }

    #[tokio::test]
    async fn another_operations_timeline_is_refused() {
        let served = served().await;
        let refused = reqwest::Client::new()
            .get(format!("{}/v1/user/operations/L/timeline", served.base))
            .bearer_auth(&served.viewer)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let anonymous = reqwest::Client::new()
            .get(format!("{}/v1/user/operations/G/timeline", served.base))
            .send()
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    }
}
