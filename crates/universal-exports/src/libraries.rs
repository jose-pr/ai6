//! Provider libraries a Station found on its plugin paths and did not load, and a person's
//! approval of one.
//!
//! A Station lists them at check-in with what was read of each as data. Approving one writes the
//! Station's own `plugin` record for that file: a SHA-256 pin of the bytes it reported, or the
//! publisher key that signed it. That decides what code the Station loads at its next start, so
//! it needs `plugin.manage` on `config:plugin`, a held answer until it is sent with `confirm`,
//! and an audit line saying who, which file and which digest. The gateway takes no library
//! bytes: the file reaches the host by deployment.

use ai6::providers::FoundLibrary;
use ai6::{ConfigRecord, ConfigScope};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::access_admin::token;
use crate::roster::{Refused, audit, changed_response, loosened, settle, store, stored_record};
use crate::{
    ApiError, Gateway, authorize_resource_action, bearer, ensure_station_registered, resolve_user,
};

/// The resource `plugin.manage` is held on: the `plugin` family.
const RESOURCE: &str = "config:plugin";

/// What `GET /v1/user/stations/{id}/libraries` answers.
#[derive(Debug, Serialize)]
pub(crate) struct Found {
    /// The Station.
    pub host: String,
    /// What it found and did not load.
    pub libraries: Vec<FoundLibrary>,
}

/// How an approval admits the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Trust {
    /// Exactly these bytes.
    Pin,
    /// Anything the publisher that signed these bytes signs for this file name.
    Publisher,
}

/// A request to approve one library a Station reported.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Approve {
    /// The file name the Station reported.
    file: String,
    /// The digest the Station reported for it; the approval is for these bytes.
    digest: String,
    /// What to trust.
    trust: Trust,
    /// Store it. Without this the answer says what would change and stores nothing.
    #[serde(default)]
    confirm: bool,
}

/// What an approval that was not stored would do.
#[derive(Debug, Serialize)]
struct Held {
    applied: bool,
    /// What will load that did not, in words.
    changes: Vec<String>,
    /// What the record then gives up that the stored ones do not already give up.
    weakened: Vec<String>,
}

/// The libraries a Station last reported as found and not loaded.
pub(crate) fn published(
    info: &std::collections::BTreeMap<String, serde_json::Value>,
) -> Vec<FoundLibrary> {
    info.get("found_libraries")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// `GET /v1/user/stations/{id}/libraries`: needs `station.read` on the Station.
pub(crate) async fn of_host(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(host): Path<String>,
) -> Result<Json<Found>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(&gateway, person, format!("station:{host}"), "station.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let station = ensure_station_registered(&registry, &host).await?;
    Ok(Json(Found {
        host,
        libraries: published(station.info()),
    }))
}

/// `POST /v1/user/stations/{id}/libraries/approve`: needs `plugin.manage` on `config:plugin`
/// and `station.read` on the Station. The library has to be one the Station reported, by file
/// and digest. Writes the Station's own `plugin` record for the file, which the Station reads at
/// its next start.
///
/// 200 with `applied: false` and what would change until sent with `confirm`; then 200 with the
/// new revision and what the record gives up. 422 with the reason when the Station reports no
/// such library, when a publisher is asked for and none is known, or when the configuration it
/// would leave is not acceptable; 409 when the record changed meanwhile.
pub(crate) async fn approve(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(host): Path<String>,
    Json(request): Json<Approve>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        RESOURCE.to_owned(),
        "plugin.manage",
    )
    .await?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        format!("station:{host}"),
        "station.read",
    )
    .await?;
    if !ConfigScope::is_plain(&host) {
        return Err(ApiError::bad_request());
    }
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let station = ensure_station_registered(&registry, &host).await?;
    let source = gateway.roster.clone().ok_or_else(ApiError::unavailable)?;
    let target = format!("station {host}: {} {}", request.file, request.digest);
    let refuse = |reason: String| async {
        audit(
            &gateway,
            &person,
            RESOURCE,
            "plugin.refused",
            &format!("{target}: {reason}"),
        )
        .await?;
        Ok::<_, ApiError>(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(Refused { refused: reason }),
            )
                .into_response(),
        )
    };
    let Some(library) = published(station.info())
        .into_iter()
        .find(|library| library.file == request.file && library.digest == request.digest)
    else {
        return refuse(format!(
            "{host} reports no library {} with digest {}",
            request.file, request.digest
        ))
        .await;
    };
    let key = library.file.clone();
    let scope = ConfigScope::Station(host.clone());
    let trust = request.trust;
    let digest = library.digest.clone();
    let publisher = library.publisher.clone();
    let read = {
        let source = source.clone();
        let (scope, key) = (scope.clone(), key.clone());
        tokio::task::spawn_blocking(move || {
            let own = stored_record(source.as_ref(), ConfigRecord::Plugin, &scope, &key)?;
            let inherited = match &own {
                Some(_) => None,
                None => stored_record(
                    source.as_ref(),
                    ConfigRecord::Plugin,
                    &ConfigScope::Global,
                    &key,
                )?,
            };
            Ok::<_, ai6::RosterError>((own, inherited))
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::unavailable())?
    };
    let (own, inherited) = read;
    let base = own
        .clone()
        .or(inherited)
        .unwrap_or_else(|| serde_json::json!({}));
    let admitted = match trust {
        Trust::Pin => digest.clone(),
        Trust::Publisher => match publisher {
            Some(key) => key,
            None => {
                return refuse(
                    "no publisher key this Station's records name signed it; approve the \
                     digest instead"
                        .to_owned(),
                )
                .await;
            }
        },
    };
    let both = base.get("require").and_then(serde_json::Value::as_str) == Some("both");
    if trust == Trust::Publisher && both {
        return refuse(
            "the record requires both a pin and a publisher; approve the digest instead".to_owned(),
        )
        .await;
    }
    let mut definition = base.clone();
    let record = definition.as_object_mut().ok_or_else(ApiError::internal)?;
    let (field, replaced) = match trust {
        Trust::Pin => ("pin", record.get("pin")),
        Trust::Publisher => ("publisher", record.get("publisher")),
    };
    let replaced = replaced
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    record.insert(
        field.to_owned(),
        serde_json::Value::String(admitted.clone()),
    );
    let mut changes = vec![match trust {
        Trust::Pin => format!("{host} will load {key} when its bytes are {admitted}"),
        Trust::Publisher => {
            format!("{host} will load {key} when it is signed by {admitted}, whatever its bytes")
        }
    }];
    if let Some(replaced) = replaced.filter(|held| *held != admitted) {
        changes.push(format!(
            "replacing {field} {replaced} of the record for {key}"
        ));
    }
    let weakened = {
        let source = source.clone();
        let (scope, key, definition) = (scope.clone(), key.clone(), definition.clone());
        tokio::task::spawn_blocking(move || {
            loosened(
                source.as_ref(),
                ConfigRecord::Plugin,
                &scope,
                &[(key, definition)],
            )
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::unavailable())?
    };
    if !request.confirm {
        return Ok(Json(Held {
            applied: false,
            changes,
            weakened,
        })
        .into_response());
    }
    let expected = token(own.as_ref());
    let outcome = {
        let (scope, key, definition) = (scope.clone(), key.clone(), definition.clone());
        tokio::task::spawn_blocking(move || {
            store(
                source.as_ref(),
                ConfigRecord::Plugin,
                &scope,
                &key,
                Some(&expected),
                &definition,
            )
        })
        .await
        .map_err(|_| ApiError::internal())?
    };
    let how = match trust {
        Trust::Pin => "pin",
        Trust::Publisher => "publisher",
    };
    match outcome {
        Ok(Ok(revision)) => {
            settle(
                &gateway,
                &person,
                RESOURCE,
                format!("{target} ({how} {admitted})"),
                Ok(Some(revision)),
                "plugin.approve",
                weakened,
            )
            .await
        }
        Ok(Err(changed)) => Ok(changed_response(changed)),
        Err(error) => {
            settle(
                &gateway,
                &person,
                RESOURCE,
                target,
                Err(error),
                "plugin.approve",
                Vec::new(),
            )
            .await
        }
    }
}
