//! Roster administration: the Operations, Agents, Agent roles and cover policies every
//! Station reads, edited through a roster source that takes changes (a SQL roster).
//!
//! Every Station polls the same source, so an accepted change reaches each without a restart.
//! The source checks the whole roster before it keeps a change: a refused edit changes
//! nothing, and is audited as an accepted one is. The routes need `roster.read` or
//! `roster.manage` on the resource `config:roster`.

use ai6::{AuditRecord, ConfigRecord, Person, RosterErrorKind};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    ApiError, Gateway, GatewayError, authorize_resource_action, bearer, resolve_user,
    unix_timestamp,
};

/// The authorization resource of the roster.
pub(crate) const ROSTER_RESOURCE: &str = "config:roster";
/// The Registry's bound on an audit target.
const MAX_TARGET_BYTES: usize = 512;
/// The longest record key the routes accept.
const MAX_KEY_BYTES: usize = 128;

/// One roster record as its source holds it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterRecord {
    /// The Operation key, Agent codename, role ID or cover name.
    pub key: String,
    /// A JSON object with the keys of the record's table in the configuration files.
    pub definition: serde_json::Value,
}

/// The records of one kind.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterRecords {
    /// `operations`, `agents`, `roles` or `covers`.
    pub kind: String,
    /// The roster revision they were read at, in decimal; Stations report the revision they
    /// run at check-in.
    pub revision: String,
    /// Whether the source takes edits.
    pub writable: bool,
    /// The records, by key.
    pub records: Vec<RosterRecord>,
}

/// An accepted edit.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterChange {
    /// The roster revision the edit moved to, in decimal.
    pub revision: String,
}

/// Why the source refused an edit (HTTP 422).
#[derive(Serialize)]
struct Refused {
    refused: String,
}

fn kind_of(kind: &str) -> Result<ConfigRecord, ApiError> {
    let name = match kind {
        "operations" => "operation",
        "agents" => "agent",
        "roles" => "role",
        "covers" => "cover",
        _ => return Err(ApiError::not_found()),
    };
    ConfigRecord::from_name(name).ok_or_else(ApiError::not_found)
}

fn checked_key(key: &str) -> Result<(), ApiError> {
    if key.trim().is_empty() || key.len() > MAX_KEY_BYTES || key.chars().any(char::is_control) {
        Err(ApiError::bad_request())
    } else {
        Ok(())
    }
}

fn source(gateway: &Gateway) -> Result<std::sync::Arc<dyn ai6::ConfigSource>, ApiError> {
    gateway.roster.clone().ok_or_else(ApiError::unavailable)
}

/// `text` on one line, cut to the audit target's bound at a character boundary.
fn bounded(text: &str) -> String {
    let mut line = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if line.len() > MAX_TARGET_BYTES {
        let mut end = MAX_TARGET_BYTES - '…'.len_utf8();
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    line
}

async fn audit(
    gateway: &Gateway,
    person: &Person,
    action: &str,
    target: &str,
) -> Result<(), ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let record = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: action.to_owned(),
        operation: String::new(),
        resource: Some(ROSTER_RESOURCE.to_owned()),
        target: bounded(target),
        at: unix_timestamp()?,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())
}

/// `GET /v1/user/roster/{kind}`: needs `roster.read`.
pub(crate) async fn records(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(kind): Path<String>,
) -> Result<Json<RosterRecords>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let record = kind_of(&kind)?;
    authorize_resource_action(&gateway, person, ROSTER_RESOURCE.to_owned(), "roster.read").await?;
    let source = source(&gateway)?;
    let definitions = tokio::task::spawn_blocking(move || source.definitions(record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|error| match error.kind() {
            RosterErrorKind::ReadOnly => ApiError::from(GatewayError::Conflict),
            _ => ApiError::unavailable(),
        })?;
    Ok(Json(RosterRecords {
        kind,
        revision: definitions.revision.get().to_string(),
        writable: definitions.writable,
        records: definitions
            .records
            .into_iter()
            .map(|(key, definition)| RosterRecord { key, definition })
            .collect(),
    }))
}

/// What an edit came to: accepted at a revision, refused with a reason, or a failure.
async fn settle(
    gateway: &Gateway,
    person: &Person,
    target: String,
    outcome: Result<Option<ai6::ConfigRevision>, ai6::RosterError>,
    accepted: &str,
) -> Result<Response, ApiError> {
    match outcome {
        Ok(Some(revision)) => {
            audit(gateway, person, accepted, &target).await?;
            Ok(Json(RosterChange {
                revision: revision.get().to_string(),
            })
            .into_response())
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(error) => match error.kind() {
            RosterErrorKind::Invalid => {
                let reason = error.to_string();
                audit(
                    gateway,
                    person,
                    "roster.refused",
                    &format!("{target}: {reason}"),
                )
                .await?;
                Ok((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(Refused { refused: reason }),
                )
                    .into_response())
            }
            RosterErrorKind::ReadOnly => Err(ApiError::from(GatewayError::Conflict)),
            _ => Err(ApiError::unavailable()),
        },
    }
}

/// `PUT /v1/user/roster/{kind}/{key}` with the record's definition as a JSON object: needs
/// `roster.manage`. 200 with the new revision; 422 with the reason when the roster it would
/// leave is not acceptable; 409 when the source takes no edits.
pub(crate) async fn put_record(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((kind, key)): Path<(String, String)>,
    Json(definition): Json<serde_json::Value>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let record = kind_of(&kind)?;
    checked_key(&key)?;
    if !definition.is_object() {
        return Err(ApiError::bad_request());
    }
    authorize_resource_action(
        &gateway,
        person.clone(),
        ROSTER_RESOURCE.to_owned(),
        "roster.manage",
    )
    .await?;
    let source = source(&gateway)?;
    let target = format!("{} {key}", record.name());
    let outcome = tokio::task::spawn_blocking(move || source.put(record, &key, &definition))
        .await
        .map_err(|_| ApiError::internal())?
        .map(Some);
    settle(&gateway, &person, target, outcome, "roster.update").await
}

/// `DELETE /v1/user/roster/{kind}/{key}`: needs `roster.manage`. 200 with the new revision;
/// 404 when there is no such record; 422 when the roster left would not be acceptable (an
/// Agent another extends, say).
pub(crate) async fn remove_record(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((kind, key)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let record = kind_of(&kind)?;
    checked_key(&key)?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        ROSTER_RESOURCE.to_owned(),
        "roster.manage",
    )
    .await?;
    let source = source(&gateway)?;
    let target = format!("{} {key}", record.name());
    let outcome = tokio::task::spawn_blocking(move || source.remove(record, &key))
        .await
        .map_err(|_| ApiError::internal())?;
    settle(&gateway, &person, target, outcome, "roster.remove").await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ai6::directorates::registry::backend::*;
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::controls::tests::{Served, served_with_roster};

    const ROLES: &str = r#"
        [role.roster-readers]
        actions = ["roster.read"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "config:roster"
        role = "roster-readers"

        [role.roster-managers]
        actions = ["roster.read", "roster.manage", "audit.read"]

        [[rule]]
        subject = "person:operator-007"
        scope = "config:roster"
        role = "roster-managers"
    "#;

    async fn call(
        served: &Served,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> reqwest::Response {
        let request = reqwest::Client::new()
            .request(method, format!("{}/v1/user/{path}", served.base))
            .bearer_auth(token);
        match body {
            Some(body) => request.json(&body),
            None => request,
        }
        .send()
        .await
        .unwrap()
    }

    fn audits(served: &Served, action: &str) -> Vec<ai6::AuditRecord> {
        served
            .registry
            .list_audit(100)
            .unwrap()
            .into_iter()
            .filter(|record| record.action == action)
            .collect()
    }

    #[tokio::test]
    async fn without_a_roster_source_the_routes_are_unavailable() {
        let served = served_with_roster(ROLES, None).await;
        let response = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "roster/agents",
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_configuration_directory_is_listed_but_takes_no_edits() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("agents.toml"),
            "[agent.007]\nhandler = \"mock-agent\"\n",
        )
        .unwrap();
        let source = Arc::new(ai6::TomlConfigDir::new(directory.path()));
        let served = served_with_roster(ROLES, Some(source)).await;
        let listed: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/agents",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(listed["writable"], false);
        assert_eq!(listed["records"][0]["key"], "007");
        assert_eq!(listed["records"][0]["definition"]["handler"], "mock-agent");
        let refused = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/agents/007",
            Some(json!({"handler": "mock-agent", "priority": 2})),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert!(audits(&served, "roster.update").is_empty());
    }

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn an_edit_writes_through_the_source_and_a_bad_one_is_refused_and_audited() {
        use ai6::{ConfigRecord, ConfigSource, SqlConfigSource};

        let directory = tempfile::tempdir().unwrap();
        let location = format!("sqlite:{}", directory.path().join("roster.db").display());
        let source = Arc::new(SqlConfigSource::open(&location).unwrap());
        source
            .put(ConfigRecord::Operation, "G", &json!({"roster": ["007"]}))
            .unwrap();
        source
            .put(
                ConfigRecord::Agent,
                "007",
                &json!({"handler": "mock-agent", "priority": 1}),
            )
            .unwrap();
        let served = served_with_roster(ROLES, Some(source.clone())).await;

        let profile: serde_json::Value = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "profile",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(
            profile["roster"],
            json!(["roster.read", "roster.manage", "audit.read"])
        );

        let listed: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/agents",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(listed["kind"], "agents");
        assert_eq!(listed["writable"], true);
        assert_eq!(
            listed["records"],
            json!([{"key": "007", "definition": {"handler": "mock-agent", "priority": 1}}])
        );
        let viewer_edit = call(
            &served,
            &served.viewer,
            reqwest::Method::PUT,
            "roster/agents/007",
            Some(json!({"handler": "mock-agent", "priority": 9})),
        )
        .await;
        assert_eq!(viewer_edit.status(), StatusCode::FORBIDDEN);

        let edited = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/agents/007",
            Some(json!({"handler": "mock-agent", "priority": 9})),
        )
        .await;
        assert_eq!(edited.status(), StatusCode::OK);
        let edited: serde_json::Value = edited.json().await.unwrap();
        let revision = source.revision().unwrap();
        assert_eq!(edited["revision"], revision.get().to_string());
        assert_eq!(
            source
                .load()
                .unwrap()
                .roster
                .agent(Some("G"), "007")
                .unwrap()
                .priority,
            9
        );
        let updates = audits(&served, "roster.update");
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].actor, "operator-007");
        assert_eq!(updates[0].resource.as_deref(), Some("config:roster"));
        assert_eq!(updates[0].target, "agent 007");

        let refused = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/agents/009",
            Some(json!({"clearance": "root"})),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let refused: serde_json::Value = refused.json().await.unwrap();
        assert!(
            refused["refused"].as_str().unwrap().contains("clearance"),
            "{refused}"
        );
        assert_eq!(source.revision().unwrap(), revision, "nothing changed");
        let refusals = audits(&served, "roster.refused");
        assert_eq!(refusals.len(), 1);
        assert!(
            refusals[0].target.starts_with("agent 009: "),
            "{refusals:?}"
        );

        let audit: serde_json::Value = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "audit",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        let actions = audit
            .as_array()
            .unwrap()
            .iter()
            .map(|record| record["action"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(actions.contains(&"roster.refused"), "{actions:?}");

        let missing = call(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            "roster/roles/nobody",
            None,
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let unknown = call(
            &served,
            &served.operator,
            reqwest::Method::GET,
            "roster/theaters",
            None,
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }
}
