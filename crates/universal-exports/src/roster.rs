//! Roster administration: the Operations, Agents, Agent roles, cover policies and Moneypenny's
//! settings every Station reads, edited through a roster source that takes changes (a SQL
//! roster).
//!
//! Every Station polls the same source, so an accepted change reaches each without a restart.
//! The source checks the whole roster before it keeps a change: a refused edit changes
//! nothing, and is audited as an accepted one is. The routes need `roster.read` or
//! `roster.manage` on the resource `config:roster`.

use ai6::{AssistantSettings, AuditRecord, ConfigRecord, Person, RosterErrorKind};
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
/// One roster record as its source holds it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterRecord {
    /// The Operation key, Agent codename, role ID, cover name, or `assistant`.
    pub key: String,
    /// A JSON object with the keys of the record's table in the configuration files.
    pub definition: serde_json::Value,
}

/// The records of one kind.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterRecords {
    /// `operations`, `agents`, `roles`, `covers` or `assistant` (at most one record, keyed
    /// `settings`).
    pub kind: String,
    /// The roster revision they were read at, in decimal; Stations report the revision they
    /// run at check-in.
    pub revision: String,
    /// Whether the source takes edits.
    pub writable: bool,
    /// The records, by key.
    pub records: Vec<RosterRecord>,
}

/// One Operation as its Stations resolve it: its definition with its ancestors' keys layered
/// in, and which Operation wrote each key.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectiveOperation {
    /// Operation codename.
    pub operation_id: String,
    /// The Operations it is subordinate to, nearest first.
    pub lineage: Vec<String>,
    /// Whether it takes policy from its ancestors.
    pub inherit: bool,
    /// The keys of its `[operation.<id>]` table with its ancestors' layered in, before
    /// presets and defaults; a key no Operation wrote has its default and is absent.
    pub definition: serde_json::Map<String, serde_json::Value>,
    /// Each written key, dotted (`green_ink.kinds`), and the Operation that wrote it.
    pub origins: std::collections::BTreeMap<String, String>,
    /// The roster revision it was read at, in decimal.
    pub revision: String,
}

/// An accepted edit.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RosterChange {
    /// The roster revision the edit moved to, in decimal.
    pub revision: String,
    /// What the stored `assistant` record loosens or redirects against the settings in force
    /// before it ([`AssistantSettings::changes_from`]); omitted when there is nothing.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub weakened: Vec<String>,
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
        "assistant" => "assistant",
        "roster" => "roster",
        _ => return Err(ApiError::not_found()),
    };
    ConfigRecord::from_name(name).ok_or_else(ApiError::not_found)
}

fn checked_key(key: &str, bytes: ai6::Limit) -> Result<(), ApiError> {
    if key.trim().is_empty() || !bytes.allows(key.len()) || key.chars().any(char::is_control) {
        Err(ApiError::bad_request())
    } else {
        Ok(())
    }
}

fn source(gateway: &Gateway) -> Result<std::sync::Arc<dyn ai6::ConfigProvider>, ApiError> {
    gateway.roster.clone().ok_or_else(ApiError::unavailable)
}

/// `text` on one line, cut to the audit target's bound at a character boundary.
fn bounded(text: &str, bytes: ai6::Limit) -> String {
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
    if !bytes.allows(line.len()) {
        let mut end = bytes.or_max().saturating_sub('…'.len_utf8());
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
        target: bounded(target, gateway.config.unex().text.roster_target_bytes),
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
    let definitions =
        tokio::task::spawn_blocking(move || source.definitions(record, &ai6::ConfigScope::Global))
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

/// `GET /v1/user/roster/operation/{id}/effective`: needs `roster.read`.
pub(crate) async fn effective(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<EffectiveOperation>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    checked_key(&id, gateway.config.unex().text.roster_key_bytes)?;
    authorize_resource_action(&gateway, person, ROSTER_RESOURCE.to_owned(), "roster.read").await?;
    let source = source(&gateway)?;
    let snapshot = tokio::task::spawn_blocking(move || source.load(""))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::unavailable())?;
    let operation = snapshot
        .roster
        .operation(&id)
        .map_err(|_| ApiError::not_found())?;
    Ok(Json(EffectiveOperation {
        operation_id: operation.id.clone(),
        lineage: snapshot
            .roster
            .lineage(&operation.id)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        inherit: operation.inherit,
        definition: operation.definition().clone(),
        origins: operation.origins().clone(),
        revision: snapshot.revision.get().to_string(),
    }))
}

/// What an edit came to: accepted at a revision, refused with a reason, or a failure.
/// `weakened` is what an accepted edit loosens.
async fn settle(
    gateway: &Gateway,
    person: &Person,
    target: String,
    outcome: Result<Option<ai6::ConfigRevision>, ai6::RosterError>,
    accepted: &str,
    weakened: Vec<String>,
) -> Result<Response, ApiError> {
    match outcome {
        Ok(Some(revision)) => {
            audit(gateway, person, accepted, &target).await?;
            Ok(Json(RosterChange {
                revision: revision.get().to_string(),
                weakened,
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
/// `roster.manage`. 200 with the new revision, and for the `assistant` record what it loosens
/// or redirects against the settings in force (`weakened`);
/// 422 with the reason when the roster it would leave is not acceptable; 409 when the source
/// takes no edits.
pub(crate) async fn put_record(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((kind, key)): Path<(String, String)>,
    Json(definition): Json<serde_json::Value>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let record = kind_of(&kind)?;
    checked_key(&key, gateway.config.unex().text.roster_key_bytes)?;
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
    let outcome = tokio::task::spawn_blocking(move || {
        // What an `assistant` record changes is told against the settings in force just
        // before it, and only for an accepted edit, whose definition the source parsed the
        // same way.
        let current = (record == ConfigRecord::Assistant)
            .then(|| source.load(""))
            .transpose()?
            .map(|snapshot| snapshot.roster.assistant().clone());
        let revision = source.put(record, &ai6::ConfigScope::Global, &key, &definition)?;
        let weakened = current
            .zip(serde_json::from_value::<AssistantSettings>(definition).ok())
            .map(|(current, settings)| settings.changes_from(&current))
            .unwrap_or_default();
        Ok((revision, weakened))
    })
    .await
    .map_err(|_| ApiError::internal())?;
    let (outcome, weakened) = match outcome {
        Ok((revision, weakened)) => (Ok(Some(revision)), weakened),
        Err(error) => (Err(error), Vec::new()),
    };
    settle(
        &gateway,
        &person,
        target,
        outcome,
        "roster.update",
        weakened,
    )
    .await
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
    checked_key(&key, gateway.config.unex().text.roster_key_bytes)?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        ROSTER_RESOURCE.to_owned(),
        "roster.manage",
    )
    .await?;
    let source = source(&gateway)?;
    let target = format!("{} {key}", record.name());
    let outcome =
        tokio::task::spawn_blocking(move || source.remove(record, &ai6::ConfigScope::Global, &key))
            .await
            .map_err(|_| ApiError::internal())?;
    settle(
        &gateway,
        &person,
        target,
        outcome,
        "roster.remove",
        Vec::new(),
    )
    .await
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
        let source = Arc::new(ai6::FileProvider::new(directory.path()));
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
        use ai6::{ConfigProvider, ConfigRecord, SqlProvider};

        let directory = tempfile::tempdir().unwrap();
        let location = format!("sqlite:{}", directory.path().join("roster.db").display());
        let source = Arc::new(SqlProvider::open(&location).unwrap());
        source
            .put(
                ConfigRecord::Operation,
                &ai6::ConfigScope::Global,
                "G",
                &json!({"roster": ["007"]}),
            )
            .unwrap();
        source
            .put(
                ConfigRecord::Agent,
                &ai6::ConfigScope::Global,
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
        let revision = source.revision("").unwrap();
        assert_eq!(edited["revision"], revision.get().to_string());
        assert_eq!(
            source
                .load("")
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
        assert_eq!(source.revision("").unwrap(), revision, "nothing changed");
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

    #[cfg(feature = "cli")]
    #[tokio::test]
    async fn the_assistant_record_is_edited_like_any_other_and_reports_its_weakenings() {
        use ai6::{ConfigProvider, SqlProvider};

        let directory = tempfile::tempdir().unwrap();
        let location = format!("sqlite:{}", directory.path().join("roster.db").display());
        let source = Arc::new(SqlProvider::open(&location).unwrap());
        let served = served_with_roster(ROLES, Some(source.clone())).await;

        let listed: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "roster/assistant",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(listed["kind"], "assistant");
        assert_eq!(listed["writable"], true);
        assert_eq!(listed["records"], json!([]));

        let viewer_edit = call(
            &served,
            &served.viewer,
            reqwest::Method::PUT,
            "roster/assistant/settings",
            Some(json!({"allow_elevated": true})),
        )
        .await;
        assert_eq!(viewer_edit.status(), StatusCode::FORBIDDEN);

        let strict = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/assistant/settings",
            Some(json!({"personal_repo": "/srv/mp/{person_digest}.git"})),
        )
        .await;
        assert_eq!(strict.status(), StatusCode::OK);
        let strict: serde_json::Value = strict.json().await.unwrap();
        // Looser than nothing in force, but it moves where people's memory goes.
        assert_eq!(
            strict["weakened"].as_array().map(Vec::len),
            Some(1),
            "{strict}"
        );
        assert!(
            strict["weakened"][0]
                .as_str()
                .unwrap()
                .starts_with("assistant.personal_repo changes"),
            "{strict}"
        );

        let loose = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/assistant/settings",
            Some(json!({"allow_elevated": true, "org": {"repo": "/srv/org.git", "write": true}})),
        )
        .await;
        assert_eq!(loose.status(), StatusCode::OK);
        let loose: serde_json::Value = loose.json().await.unwrap();
        let weakened = loose["weakened"].as_array().unwrap();
        assert_eq!(weakened.len(), 4, "{loose}");
        assert!(
            weakened[0]
                .as_str()
                .unwrap()
                .contains("assistant.allow_elevated"),
            "{loose}"
        );
        // The same record again changes nothing in force, so nothing is reported.
        let again = call(
            &served,
            &served.operator,
            reqwest::Method::PUT,
            "roster/assistant/settings",
            Some(json!({"allow_elevated": true, "org": {"repo": "/srv/org.git", "write": true}})),
        )
        .await;
        assert_eq!(again.status(), StatusCode::OK);
        let again: serde_json::Value = again.json().await.unwrap();
        assert!(again.get("weakened").is_none(), "{again}");
        let settings = source.load("").unwrap().roster.assistant().clone();
        assert!(settings.allow_elevated());
        assert_eq!(
            settings.personal_repo(),
            None,
            "the record is replaced whole"
        );
        let revision = source.revision("").unwrap();

        for (key, definition, reason) in [
            ("moneypenny", json!({}), "\"settings\""),
            (
                "settings",
                json!({"personal_repo": "ext::sh"}),
                "assistant.personal_repo",
            ),
        ] {
            let refused = call(
                &served,
                &served.operator,
                reqwest::Method::PUT,
                &format!("roster/assistant/{key}"),
                Some(definition),
            )
            .await;
            assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let refused: serde_json::Value = refused.json().await.unwrap();
            assert!(
                refused["refused"].as_str().unwrap().contains(reason),
                "{refused}"
            );
        }
        assert_eq!(source.revision("").unwrap(), revision, "nothing changed");
        assert_eq!(audits(&served, "roster.refused").len(), 2);
        assert_eq!(
            audits(&served, "roster.update")[0].target,
            "assistant settings"
        );

        let removed = call(
            &served,
            &served.operator,
            reqwest::Method::DELETE,
            "roster/assistant/settings",
            None,
        )
        .await;
        assert_eq!(removed.status(), StatusCode::OK);
        assert_eq!(
            source.load("").unwrap().roster.assistant(),
            &ai6::AssistantSettings::default()
        );
    }
}
