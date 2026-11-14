//! What a person may read of charges: which exist, who keeps each and where it runs, when it last
//! woke, and its logbook.
//!
//! `GET /v1/user/charges` lists every charge the person may see: M's with `config.read` on
//! `config:charge`, an Operation's with `mission.read` there, a Station's with `station.read`
//! there (or `charge.converse` on any of them). It also says what the person may do with each, so
//! a client shows only the controls they hold. `GET /v1/user/charges/{scope}` adds the logbook
//! and needs `charge.converse`, since the logbook is what attaching replays. Both read the
//! Registry and the roster and are not audited, as the roster's reads are not.

use ai6::directorates::m::charges::ChargeScope;
use ai6::directorates::registry::charges::{ChargeLogEntry, ChargeRecord, TURN_WINDOW_SECS};
use ai6::{ConfigRecord, ConfigScope, Person};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::Serialize;
use serde_json::Value;

use crate::{ApiError, Gateway, bearer, resolve_user, resource_action_allowed, unix_timestamp};

/// How a charge's Agent reaches its model, from the roster.
#[derive(Serialize)]
pub(crate) struct RouteView {
    /// The handler of the first route, or of the Agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    handler: Option<String>,
    /// The model the first route, or the Agent, names.
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    /// How many routes the Agent has.
    routes: usize,
}

/// One charge as a person sees it.
#[derive(Serialize)]
pub(crate) struct ChargeView {
    /// `m`, `operation:<codename>` or `station:<id>`.
    scope: String,
    /// The Agent that keeps it: the claim's, else the roster's.
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<RouteView>,
    /// Whether the roster has a record for it.
    configured: bool,
    /// The Station that holds its lease, while the lease is live.
    #[serde(skip_serializing_if = "Option::is_none")]
    station: Option<String>,
    /// Whether a Station holds its lease now: with none, nothing runs it.
    held: bool,
    /// When the holder's lease lapses, as Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_expires: Option<f64>,
    /// When it last woke, as Unix seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_wake: Option<f64>,
    /// What the last turn recorded: what woke it and the start of what it said.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_turn: Option<String>,
    /// How the last turn ended (`completed`, `failed: why`, `limit`, `timed out`...), read from
    /// what it recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_ended: Option<String>,
    /// Turns it began in the last hour.
    turns_last_hour: usize,
    /// Whether the person may attach to it, type and answer.
    can_converse: bool,
    /// Whether the person may interrupt its turn.
    can_interrupt: bool,
}

/// A charge's view with its logbook, oldest first.
#[derive(Serialize)]
pub(crate) struct ChargeDetail {
    #[serde(flatten)]
    charge: ChargeView,
    logbook: Vec<ChargeLogEntry>,
}

/// What a scope's actions are held on: the action that shows it, the resource, and the action
/// that interrupts its turn.
fn rights(scope: &str) -> Option<(&'static str, String, &'static str)> {
    Some(match scope.parse::<ChargeScope>().ok()? {
        ChargeScope::M => (
            "config.read",
            "config:charge".to_owned(),
            "station.configure",
        ),
        ChargeScope::Operation(operation) => (
            "mission.read",
            crate::tree::operation_resource(&operation),
            "mission.control",
        ),
        ChargeScope::Station(station) => (
            "station.read",
            format!("station:{station}"),
            "station.configure",
        ),
        _ => return None,
    })
}

async fn holds(gateway: &Gateway, person: &Person, resource: &str, action: &'static str) -> bool {
    resource_action_allowed(gateway, person.clone(), resource.to_owned(), action)
        .await
        .unwrap_or(false)
}

/// The roster's records of a family, by key; empty when the gateway serves no roster.
async fn roster_of(gateway: &Gateway, record: ConfigRecord) -> Vec<(String, Value)> {
    let Some(source) = gateway.roster.clone() else {
        return Vec::new();
    };
    tokio::task::spawn_blocking(move || source.definitions(record, &ConfigScope::Global))
        .await
        .ok()
        .and_then(Result::ok)
        .map(|definitions| definitions.records.into_iter().collect())
        .unwrap_or_default()
}

fn route_of(agent: &Value) -> RouteView {
    let routes = agent.get("route").and_then(Value::as_array);
    let first = routes.and_then(|routes| routes.first()).unwrap_or(agent);
    let text = |key: &str| {
        first
            .get(key)
            .or_else(|| agent.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    RouteView {
        handler: text("handler"),
        model: text("model"),
        routes: routes.map_or(1, Vec::len),
    }
}

fn last_turn(record: &ChargeRecord) -> Option<String> {
    record
        .logbook
        .iter()
        .rev()
        .find(|entry| entry.kind == "turn")
        .map(|entry| entry.text.clone())
}

/// How a turn ended, from the line the Station logs for it: `woke for X; the turn ended Y. It
/// said: ...`.
fn turn_ended(text: &str) -> Option<String> {
    let (_, rest) = text.split_once("the turn ended ")?;
    let ended = rest
        .split_once(". It said: ")
        .map_or(rest, |(ended, _)| ended);
    let ended = ended.trim();
    (!ended.is_empty()).then(|| ended.to_owned())
}

/// Something wrong with a charge that a person should hear about.
pub(crate) struct Trouble {
    /// `charge_failed` or `charge_lapsed`.
    pub(crate) kind: &'static str,
    /// What the charge did, after its name.
    pub(crate) title: String,
    pub(crate) detail: String,
    /// When it began, as Unix seconds.
    pub(crate) since: Option<f64>,
}

impl ChargeView {
    /// `m`, `operation:<codename>` or `station:<id>`.
    pub(crate) fn scope(&self) -> &str {
        &self.scope
    }

    /// What a person calls it.
    pub(crate) fn label(&self, names: &std::collections::BTreeMap<String, String>) -> String {
        match self.scope.split_once(':') {
            Some(("operation", id)) => {
                format!(
                    "Charge of Operation {}",
                    crate::attention::called(names, id)
                )
            }
            Some(("station", id)) => format!("Charge of Station {id}"),
            _ => "M's charge".to_owned(),
        }
    }

    /// What is wrong with it: its last turn ended failed, or no Station holds its lease any more.
    /// A charge never claimed has no lease to lapse.
    pub(crate) fn trouble(&self) -> Vec<Trouble> {
        let mut found = Vec::new();
        if let Some(ended) = self
            .last_ended
            .as_deref()
            .filter(|e| e.starts_with("failed"))
        {
            found.push(Trouble {
                kind: "charge_failed",
                title: "failed its last turn".to_owned(),
                detail: format!("The last turn ended {}.", plain(ended)),
                since: self.last_wake,
            });
        }
        if !self.held
            && let Some(expired) = self.lease_expires
        {
            found.push(Trouble {
                kind: "charge_lapsed",
                title: "has no Station holding it".to_owned(),
                detail: "Its lease lapsed, so nothing runs it until a Station claims it."
                    .to_owned(),
                since: Some(expired),
            });
        }
        found
    }
}

/// One line, with no control characters.
fn plain(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

struct Sources {
    records: Vec<ChargeRecord>,
    configured: Vec<(String, Value)>,
    agents: Vec<(String, Value)>,
    now: f64,
}

async fn sources(gateway: &Gateway) -> Result<Sources, ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let records = tokio::task::spawn_blocking(move || registry.charges())
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(Sources {
        records,
        configured: roster_of(gateway, ConfigRecord::Charge).await,
        agents: roster_of(gateway, ConfigRecord::Agent).await,
        now: unix_timestamp()?,
    })
}

/// The view of `scope` for `person`, with its record, or `None` when they may not see it.
async fn view(
    gateway: &Gateway,
    person: &Person,
    sources: &Sources,
    scope: &str,
) -> Option<(ChargeView, Option<ChargeRecord>)> {
    let (read, resource, control) = rights(scope)?;
    let can_converse = holds(gateway, person, &resource, "charge.converse").await;
    if !can_converse && !holds(gateway, person, &resource, read).await {
        return None;
    }
    let can_interrupt = can_converse && holds(gateway, person, &resource, control).await;
    let record = sources
        .records
        .iter()
        .find(|record| record.scope == scope)
        .cloned();
    let configured = sources.configured.iter().find(|(key, _)| key == scope);
    let agent = record
        .as_ref()
        .map(|record| record.agent.clone())
        .or_else(|| {
            configured
                .and_then(|(_, definition)| definition.get("agent"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    let route = agent.as_ref().and_then(|agent| {
        sources
            .agents
            .iter()
            .find(|(key, _)| key == agent)
            .map(|(_, definition)| route_of(definition))
    });
    let held = record
        .as_ref()
        .is_some_and(|record| record.lease_expires > sources.now);
    let view = ChargeView {
        scope: scope.to_owned(),
        agent,
        route,
        configured: configured.is_some(),
        station: record
            .as_ref()
            .filter(|_| held)
            .map(|record| record.station.clone()),
        held,
        lease_expires: record.as_ref().map(|record| record.lease_expires),
        last_wake: record.as_ref().and_then(|record| record.last_wake),
        last_ended: record
            .as_ref()
            .and_then(last_turn)
            .as_deref()
            .and_then(turn_ended),
        last_turn: record.as_ref().and_then(last_turn),
        turns_last_hour: record.as_ref().map_or(0, |record| {
            record.turns_since(sources.now - TURN_WINDOW_SECS)
        }),
        can_converse,
        can_interrupt,
    };
    Some((view, record))
}

/// `GET /v1/user/charges`: the charges the person may see, configured or running.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<Vec<ChargeView>>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let sources = sources(&gateway).await?;
    let mut scopes: Vec<String> = sources
        .records
        .iter()
        .map(|record| record.scope.clone())
        .chain(sources.configured.iter().map(|(key, _)| key.clone()))
        .collect();
    scopes.sort();
    scopes.dedup();
    let mut charges = Vec::new();
    for scope in scopes {
        if let Some((view, _)) = view(&gateway, &person, &sources, &scope).await {
            charges.push(view);
        }
    }
    Ok(Json(charges))
}

/// `GET /v1/user/charges/{scope}`: one charge with its logbook; needs `charge.converse` on it.
pub(crate) async fn show(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(scope): Path<String>,
) -> Result<Json<ChargeDetail>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let sources = sources(&gateway).await?;
    let Some((view, record)) = view(&gateway, &person, &sources, &scope).await else {
        return Err(ApiError::not_found());
    };
    if !view.can_converse {
        return Err(ApiError::forbidden());
    }
    Ok(Json(ChargeDetail {
        charge: view,
        logbook: record.map(|record| record.logbook).unwrap_or_default(),
    }))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ai6::directorates::registry::backend::*;
    use ai6::directorates::registry::charges::{ChargeLogEntry, ChargeUpdate};
    use reqwest::StatusCode;
    use serde_json::Value;

    use crate::controls::tests::{Served, served_with};

    /// `operator-007` converses with G's charge and controls its Missions; `viewer-007` reads G
    /// only; `reader-007` reads Station `worker` only.
    const ROLES: &str = r#"
        [role.converser]
        actions = ["charge.converse", "mission.control"]

        [role.viewer]
        actions = ["mission.read"]

        [role.looker]
        actions = ["station.read"]

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "converser"

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "viewer"

        [[rule]]
        subject = "person:viewer-007"
        scope = "station:worker"
        role = "looker"
    "#;

    async fn get(served: &Served, token: &str, path: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(format!("{}/v1/user/charges{path}", served.base))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    async fn served_charges() -> Served {
        let served = served_with(ROLES).await;
        for scope in ["operation:G", "station:worker", "operation:L"] {
            let record = served
                .registry
                .claim_charge(scope, "kg", "worker", Duration::from_secs(600))
                .unwrap();
            let mut update = ChargeUpdate::default();
            update.last_wake = Some(1_000.0);
            update.turn_at = Some(1_000.0);
            update.log = vec![ChargeLogEntry::new("turn", "woke on a timer", 1_000.0)];
            served
                .registry
                .update_charge(scope, "worker", record.fence, None, update)
                .unwrap();
        }
        served
    }

    #[tokio::test]
    async fn a_person_lists_the_charges_they_may_see_with_what_they_may_do() {
        let served = served_charges().await;

        let operator: Vec<Value> = get(&served, &served.operator, "")
            .await
            .json()
            .await
            .unwrap();
        let scopes: Vec<&str> = operator
            .iter()
            .map(|c| c["scope"].as_str().unwrap())
            .collect();
        assert_eq!(scopes, ["operation:G"], "no other scope is theirs");
        let charge = &operator[0];
        assert_eq!(charge["agent"], "kg");
        assert_eq!(charge["station"], "worker");
        assert_eq!(charge["held"], true);
        assert_eq!(charge["last_wake"], 1_000.0);
        assert_eq!(charge["last_turn"], "woke on a timer");
        assert_eq!(charge["can_converse"], true);
        assert_eq!(charge["can_interrupt"], true);

        // Reading shows a charge and offers no control.
        let viewer: Vec<Value> = get(&served, &served.viewer, "").await.json().await.unwrap();
        let scopes: Vec<&str> = viewer
            .iter()
            .map(|c| c["scope"].as_str().unwrap())
            .collect();
        assert_eq!(scopes, ["operation:G", "station:worker"]);
        assert!(viewer.iter().all(|c| c["can_converse"] == false));
        assert!(viewer.iter().all(|c| c["can_interrupt"] == false));

        let stranger = get(&served, "not-a-token", "").await;
        assert_eq!(stranger.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_logbook_needs_charge_converse_and_says_a_lapsed_lease_is_not_held() {
        let served = served_charges().await;
        let shown = get(&served, &served.operator, "/operation:G").await;
        assert_eq!(shown.status(), StatusCode::OK);
        let shown: Value = shown.json().await.unwrap();
        assert_eq!(shown["logbook"][0]["kind"], "turn");
        assert_eq!(shown["logbook"][0]["text"], "woke on a timer");

        assert_eq!(
            get(&served, &served.viewer, "/operation:G").await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(&served, &served.viewer, "/operation:L").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(&served, &served.operator, "/not-a-scope")
                .await
                .status(),
            StatusCode::NOT_FOUND
        );

        served
            .registry
            .claim_charge("operation:G", "kg", "worker", Duration::ZERO)
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let lapsed: Vec<Value> = get(&served, &served.operator, "")
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(lapsed[0]["held"], false);
        assert!(lapsed[0].get("station").is_none());
    }
}
