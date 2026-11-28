//! Secret values and the scope keys that encrypt them, written through the gateway.
//!
//! The gateway holds no Station's sealing key, so it cannot encrypt a value. A person's request
//! (`put`, `rotate`, `rotate-key`, `grant`, `revoke`, `remove`) is sealed to the Stations that
//! hold the scope's key and left in the Registry; one of them does it with its own key and
//! answers. A request returns at once with its ID, and the answer is read from it. A value is in
//! the request once and is never returned: what the routes show is names, scopes, revisions, key
//! IDs and Stations.
//!
//! Every route needs `config.manage` (the listing, `config.read`) on `config:secrets`.

use ai6::directorates::q::sealing::{
    self, Recipient, RequestStatus, Scope, SecretOutcome, SecretRequest,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::{ApiError, Gateway, bearer, resolve_user, resource_action_allowed, unix_timestamp};

/// The authorization resource of secrets.
const RESOURCE: &str = "config:secrets";

async fn require(
    gateway: &Gateway,
    headers: &HeaderMap,
    action: &'static str,
) -> Result<ai6::Person, ApiError> {
    let person = resolve_user(gateway, bearer(headers)?).await?;
    if resource_action_allowed(gateway, person.clone(), RESOURCE.to_owned(), action).await? {
        Ok(person)
    } else {
        Err(ApiError::refused(
            StatusCode::FORBIDDEN,
            format!("secrets need {action} on {RESOURCE}"),
        ))
    }
}

#[derive(Serialize)]
struct ValueRow {
    name: String,
    scope: String,
    revision: u64,
    key_id: String,
}

#[derive(Serialize)]
struct ScopeRow {
    scope: String,
    revision: u64,
    key_id: String,
    stations: Vec<String>,
    rotating: bool,
}

#[derive(Serialize)]
pub(crate) struct Listing {
    values: Vec<ValueRow>,
    scopes: Vec<ScopeRow>,
}

/// `GET /v1/user/secrets`: needs `config.read` on `config:secrets`. The values and scope keys the
/// Registry holds, with no value in it.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<Listing>, ApiError> {
    require(&gateway, &headers, "config.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let (values, scopes) = tokio::task::spawn_blocking(move || {
        Ok::<_, sealing::VaultError>((
            sealing::list_values(registry.as_ref())?,
            sealing::list_scopes(registry.as_ref())?,
        ))
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::unavailable())?;
    Ok(Json(Listing {
        values: values
            .into_iter()
            .map(|value| ValueRow {
                name: value.name,
                scope: value.scope.to_string(),
                revision: value.revision,
                key_id: value.key_id,
            })
            .collect(),
        scopes: scopes
            .into_iter()
            .map(|scope| ScopeRow {
                scope: scope.scope.to_string(),
                revision: scope.revision,
                key_id: scope.key_id,
                stations: scope.stations.into_iter().collect(),
                rotating: scope.rotating,
            })
            .collect(),
    }))
}

#[derive(Serialize)]
struct Accepted {
    id: String,
}

/// Who is asked, and what the audit says, for `request`.
fn plan(
    registry: &dyn ai6::RegistryBackend,
    request: &SecretRequest,
) -> Result<(Vec<Recipient>, String), String> {
    let scopes = sealing::list_scopes(registry).map_err(|error| error.to_string())?;
    let values = sealing::list_values(registry).map_err(|error| error.to_string())?;
    let holders = |scope: &str| -> Result<Vec<Recipient>, String> {
        let scope = Scope::parse(scope).map_err(|error| error.to_string())?;
        scopes
            .iter()
            .find(|held| held.scope == scope)
            .map(|held| {
                held.stations
                    .iter()
                    .cloned()
                    .map(Recipient::named)
                    .collect()
            })
            .ok_or_else(|| format!("scope {scope} has no key yet"))
    };
    match request {
        SecretRequest::Put {
            name,
            scope,
            stations,
            pins,
            ..
        } => {
            let scope = scope
                .clone()
                .or_else(|| {
                    values
                        .iter()
                        .find(|held| &held.name == name)
                        .map(|held| held.scope.to_string())
                })
                .unwrap_or_else(|| "global".to_owned());
            Scope::parse(&scope).map_err(|error| error.to_string())?;
            let target = format!("put {scope} {name}");
            match holders(&scope) {
                Ok(recipients) => Ok((recipients, target)),
                Err(_) if stations.is_empty() => Err(format!(
                    "scope {scope} has no key yet: name the Stations its key is wrapped for"
                )),
                Err(_) => Ok((
                    stations
                        .iter()
                        .map(|station| match pins.get(station) {
                            Some(pin) => Recipient::named(station.clone()).pinned(pin),
                            None => Recipient::named(station.clone()),
                        })
                        .collect(),
                    target,
                )),
            }
        }
        SecretRequest::Rotate { name, .. } => {
            let held = values
                .iter()
                .find(|held| &held.name == name)
                .ok_or_else(|| format!("there is no value {name:?} to rotate"))?;
            Ok((
                holders(held.scope.as_str())?,
                format!("rotate {} {name}", held.scope),
            ))
        }
        SecretRequest::RotateKey { scope } => Ok((holders(scope)?, format!("rotate-key {scope}"))),
        SecretRequest::Grant { scope, station, .. } => {
            Ok((holders(scope)?, format!("grant {scope} {station}")))
        }
        SecretRequest::Revoke { scope, station } => {
            Ok((holders(scope)?, format!("revoke {scope} {station}")))
        }
        SecretRequest::Remove { name, scope } => {
            Ok((holders(scope)?, format!("remove {scope} {name}")))
        }
    }
}

/// `POST /v1/user/secrets/requests`: needs `config.manage` on `config:secrets`. The body is a
/// request with its `verb`. 202 with its `id`; 422 with the reason when no Station can be asked,
/// naming that.
pub(crate) async fn request(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(request): Json<SecretRequest>,
) -> Result<Response, ApiError> {
    let person = require(&gateway, &headers, "config.manage").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let now = unix_timestamp()?;
    let sent = id.clone();
    let requester = gateway.secret_requester.clone();
    let planned = tokio::task::spawn_blocking(move || {
        let (recipients, target) = plan(registry.as_ref(), &request)?;
        sealing::ask(
            registry.as_ref(),
            &sent,
            &request,
            &recipients,
            requester
                .as_ref()
                .map(|(signer, lifetime)| (signer, *lifetime)),
            now,
        )
        .map_err(|error| format!("no Station could be asked: {error}"))?;
        Ok::<_, String>(target)
    })
    .await
    .map_err(|_| ApiError::internal())?;
    let target = match planned {
        Ok(target) => target,
        Err(reason) => {
            return Err(ApiError::refused(StatusCode::UNPROCESSABLE_ENTITY, reason));
        }
    };
    crate::roster::audit(&gateway, &person, RESOURCE, "secret.request", &target).await?;
    Ok((StatusCode::ACCEPTED, Json(Accepted { id })).into_response())
}

#[derive(Serialize)]
pub(crate) struct Answer {
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<SecretOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// `GET /v1/user/secrets/requests/{id}`: needs `config.read` on `config:secrets`. `pending`,
/// `done` with what was done, or `failed` with why; 404 for no such request.
pub(crate) async fn answer(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Answer>, ApiError> {
    require(&gateway, &headers, "config.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let status = tokio::task::spawn_blocking(move || sealing::status(registry.as_ref(), &id))
        .await
        .map_err(|_| ApiError::internal())?;
    match status {
        Ok(RequestStatus::Pending) => Ok(Json(Answer {
            state: "pending",
            outcome: None,
            error: None,
        })),
        Ok(RequestStatus::Done(done)) => Ok(Json(Answer {
            state: "done",
            outcome: Some(done),
            error: None,
        })),
        Ok(RequestStatus::Failed(why)) => Ok(Json(Answer {
            state: "failed",
            outcome: None,
            error: Some(why),
        })),
        Err(sealing::VaultError::NotFound) => Err(ApiError::not_found()),
        Err(_) => Err(ApiError::unavailable()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ai6::directorates::q::sealing::{Scope, SealingKey, Vault};
    use ai6::directorates::registry::backend::*;
    use ai6::directorates::registry::sealed::StationSealingKey;
    use ai6::directorates::registry::store::FileRegistry;
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::controls::tests::{Served, served_with_roster};

    const PLANTED: &str = "planted-secret-0451";
    const ROLES: &str = r#"
        [role.readers]
        actions = ["config.read"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "config:secrets"
        role = "readers"

        [role.managers]
        actions = ["config.read", "config.manage"]

        [[rule]]
        subject = "person:operator-007"
        scope = "config:secrets"
        role = "managers"
    "#;

    fn station(served: &Served, name: &str) -> Vault {
        let registry: Arc<dyn RegistryBackend> =
            Arc::new(FileRegistry::open(served.directory.path()));
        let key = Arc::new(SealingKey::generate());
        registry
            .publish_sealing_key(&StationSealingKey::new(name, key.public_key().to_text()))
            .unwrap();
        Vault::new(registry, name, key).with_trust(ai6::directorates::q::sealing::Trust::unsigned())
    }

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

    async fn ask(served: &Served, body: serde_json::Value) -> String {
        let sent = call(
            served,
            &served.operator,
            reqwest::Method::POST,
            "secrets/requests",
            Some(body),
        )
        .await;
        assert_eq!(
            sent.status(),
            StatusCode::ACCEPTED,
            "{}",
            sent.text().await.unwrap()
        );
        let accepted: serde_json::Value = sent.json().await.unwrap();
        accepted["id"].as_str().unwrap().to_owned()
    }

    async fn answer(served: &Served, id: &str) -> serde_json::Value {
        call(
            served,
            &served.viewer,
            reqwest::Method::GET,
            &format!("secrets/requests/{id}"),
            None,
        )
        .await
        .json()
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_put_through_the_gateway_reaches_two_stations_and_no_response_holds_the_value() {
        let served = served_with_roster(ROLES, None).await;
        let (a, b) = (station(&served, "a"), station(&served, "b"));
        let id = ask(
            &served,
            json!({"verb": "put", "name": "forge", "scope": "operation:alpha",
                   "value": PLANTED, "stations": ["a", "b"]}),
        )
        .await;
        let pending = answer(&served, &id).await;
        assert_eq!(pending["state"], "pending");
        assert_eq!(
            a.serve_requests(1_790_000_000.0).unwrap(),
            std::slice::from_ref(&id)
        );
        let done = answer(&served, &id).await;
        assert_eq!(done["state"], "done", "{done}");
        assert_eq!(done["outcome"]["stations"], json!(["a", "b"]));
        let alpha = Scope::operation("alpha").unwrap();
        for vault in [&a, &b] {
            let read = vault.read_value_in("forge", Some(&alpha)).unwrap();
            assert_eq!(read.value.as_str(), PLANTED);
        }
        let listing: serde_json::Value = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "secrets",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
        assert_eq!(listing["values"][0]["name"], "forge");
        assert_eq!(listing["scopes"][0]["scope"], "operation:alpha");
        // What the gateway answered, listed and audited holds no value.
        let mut seen = format!("{pending} {done} {listing}");
        for record in served.registry.list_audit(100).unwrap() {
            seen.push_str(&format!("{record:?}"));
        }
        assert!(!seen.contains(PLANTED), "{seen}");
        assert!(!served.registry.list_audit(100).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_revoke_through_the_gateway_says_what_the_station_keeps() {
        let served = served_with_roster(ROLES, None).await;
        let (a, b) = (station(&served, "a"), station(&served, "b"));
        let id = ask(
            &served,
            json!({"verb": "put", "name": "forge", "scope": "global",
                   "value": "one", "stations": ["a", "b"]}),
        )
        .await;
        a.serve_requests(1_790_000_000.0).unwrap();
        assert_eq!(answer(&served, &id).await["state"], "done");
        let id = ask(
            &served,
            json!({"verb": "revoke", "scope": "global", "station": "b"}),
        )
        .await;
        a.serve_requests(1_790_000_001.0).unwrap();
        let done = answer(&served, &id).await;
        assert_eq!(done["outcome"]["stations"], json!(["a"]));
        let notes = done["outcome"]["notes"].to_string();
        assert!(notes.contains("keeps every value"), "{notes}");
        assert!(!notes.contains("Registry credentials"), "{notes}");
        let id = ask(
            &served,
            json!({"verb": "put", "name": "forge", "value": "two"}),
        )
        .await;
        a.serve_requests(1_790_000_002.0).unwrap();
        assert_eq!(answer(&served, &id).await["state"], "done");
        assert!(b.read_value("forge").is_err());
        assert_eq!(a.read_value("forge").unwrap().value.as_str(), "two");
    }

    #[tokio::test]
    async fn a_request_is_refused_without_the_action_or_without_a_station_to_ask() {
        let served = served_with_roster(ROLES, None).await;
        let _a = station(&served, "a");
        let body =
            json!({"verb": "put", "name": "forge", "scope": "operation:x", "value": PLANTED});
        let denied = call(
            &served,
            &served.viewer,
            reqwest::Method::POST,
            "secrets/requests",
            Some(body.clone()),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert!(!denied.text().await.unwrap().contains(PLANTED));
        // A scope with no key and no Stations named, and a Station that published no key.
        let refused = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            "secrets/requests",
            Some(body),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let text = refused.text().await.unwrap();
        assert!(text.contains("name the Stations"), "{text}");
        let nobody = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            "secrets/requests",
            Some(
                json!({"verb": "put", "name": "forge", "scope": "global", "value": "x",
                        "stations": ["ghost"]}),
            ),
        )
        .await;
        assert_eq!(nobody.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let text = nobody.text().await.unwrap();
        assert!(text.contains("no Station could be asked"), "{text}");
        assert!(!text.contains("x\""));
        let missing = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "secrets/requests/nothing",
            None,
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
