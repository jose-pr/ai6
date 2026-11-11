//! Provider libraries a Station found on its plugin paths and did not load, and a person's
//! approval of one.
//!
//! A Station lists them at check-in with what was read of each as data; the list is the
//! Station's own claim, so the gateway keeps only well-formed entries from a Station it has
//! heard from lately. Approving one writes the Station's own `plugin` record for that file: a
//! SHA-256 pin of the bytes it reported, or a publisher key that records already name. That
//! decides what code the Station loads at its next start, so it needs `plugin.manage` on
//! `config:plugin`, a held answer until it is sent again with `confirm` and with what the preview
//! returned, and an audit line saying who, which file and which digest. The gateway takes no
//! library bytes: the file reaches the host by deployment.

use std::collections::BTreeSet;

use ai6::providers::FoundLibrary;
use ai6::{ConfigRecord, ConfigScope, Person};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::access_admin::token;
use crate::roster::{Refused, audit, changed_response, loosened, settle, store};
use crate::{
    ApiError, Gateway, authorize_resource_action, bearer, resolve_user, resource_action_allowed,
};

/// The resource `plugin.manage` is held on: the `plugin` family.
pub(crate) const RESOURCE: &str = "config:plugin";

/// What `GET /v1/user/stations/{id}/libraries` answers.
#[derive(Debug, Serialize)]
pub(crate) struct Found {
    /// The Station.
    pub host: String,
    /// What it claims to have found and not loaded, if it has been heard from lately.
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
    /// With `confirm`: the `record_revision` the preview returned. A record changed since is a
    /// conflict.
    record_revision: Option<String>,
    /// With `confirm` and `trust: publisher`: the `publisher` the preview returned.
    publisher: Option<String>,
}

/// What an approval that was not stored would do.
#[derive(Debug, Serialize)]
struct Held {
    applied: bool,
    /// What will load that did not, in words.
    changes: Vec<String>,
    /// What the record then gives up that the stored ones do not already give up.
    weakened: Vec<String>,
    /// The revision of the Station's own record now, to send back with `confirm`.
    record_revision: String,
    /// For a publisher trust, the key to send back with `confirm`.
    #[serde(skip_serializing_if = "Option::is_none")]
    publisher: Option<String>,
}

/// Whether `c` is invisible or changes how text reads (a bidirectional override, a zero-width
/// space, a tag character): the Unicode format characters.
fn invisible(c: char) -> bool {
    matches!(c,
        '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{180e}'
        | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}'
        | '\u{e0000}'..='\u{e007f}')
}

/// Whether `text` is plain: no control and no format characters.
fn plain(text: &str) -> bool {
    !text.chars().any(|c| c.is_control() || invisible(c))
}

/// `prefix` and `length` lowercase hexadecimal digits.
fn hex_text(text: &str, prefix: &str, length: usize) -> bool {
    text.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == length && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Whether a reported entry is one the gateway will show and act on: a file name and not a
/// path, within the key bound; a `sha256:` digest; a publisher that is an `ed25519:` key; no
/// control or format characters anywhere.
fn well_formed(library: &FoundLibrary, name_bytes: ai6::Limit) -> bool {
    !library.file.trim().is_empty()
        && name_bytes.allows(library.file.len())
        && !library.file.contains(['/', '\\'])
        && hex_text(&library.digest, "sha256:", 64)
        && library
            .publisher
            .as_deref()
            .is_none_or(|key| hex_text(key, "ed25519:", 64))
        && [&library.file, &library.verdict]
            .into_iter()
            .chain(&library.kinds)
            .chain(&library.declared)
            .all(|text| plain(text))
}

/// The libraries a Station last reported as found and not loaded, those well-formed.
pub(crate) fn published(
    info: &std::collections::BTreeMap<String, serde_json::Value>,
    name_bytes: ai6::Limit,
) -> Vec<FoundLibrary> {
    info.get("found_libraries")
        .and_then(|value| serde_json::from_value::<Vec<FoundLibrary>>(value.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|library| well_formed(library, name_bytes))
        .collect()
}

/// The Station's check-in if it was heard from within `[registry] gone_dark_after`; a Station
/// gone dark has its claims ignored, since the Registry keeps its last check-in indefinitely.
async fn fresh_station(
    gateway: &Gateway,
    host: &str,
) -> Result<Option<ai6::StationRecord>, ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let host = host.to_owned();
    tokio::task::spawn_blocking(move || {
        let fresh = registry.limits().stations.gone_dark_after.0;
        registry
            .stations(fresh)
            .map(|stations| {
                stations
                    .into_iter()
                    .find(|station| station.station() == host)
            })
            .map_err(|_| ApiError::internal())
    })
    .await
    .map_err(|_| ApiError::internal())?
}

/// Requires `plugin.manage` on `config:plugin`; a refusal is audited `plugin.refused` with
/// `target`.
pub(crate) async fn require_manage(
    gateway: &Gateway,
    person: &Person,
    target: &str,
) -> Result<(), ApiError> {
    if resource_action_allowed(
        gateway,
        person.clone(),
        RESOURCE.to_owned(),
        "plugin.manage",
    )
    .await?
    {
        return Ok(());
    }
    audit(
        gateway,
        person,
        RESOURCE,
        "plugin.refused",
        &format!("{target}: needs plugin.manage"),
    )
    .await?;
    Err(ApiError::refused(
        StatusCode::FORBIDDEN,
        format!("the plugin family needs plugin.manage on {RESOURCE}"),
    ))
}

/// `GET /v1/user/stations/{id}/libraries`: needs `station.read` on the Station.
pub(crate) async fn of_host(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(host): Path<String>,
) -> Result<Json<Found>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(&gateway, person, format!("station:{host}"), "station.read").await?;
    let name_bytes = gateway.config.unex().text.roster_key_bytes;
    let libraries = fresh_station(&gateway, &host)
        .await?
        .map(|station| published(station.info(), name_bytes))
        .unwrap_or_default();
    Ok(Json(Found { host, libraries }))
}

/// Every publisher key the `plugin` records name, global and the Station's own, with the files
/// each is named for.
fn named_publishers(
    source: &dyn ai6::ConfigProvider,
    host: &str,
) -> Result<std::collections::BTreeMap<String, BTreeSet<String>>, ai6::RosterError> {
    let mut named: std::collections::BTreeMap<String, BTreeSet<String>> = Default::default();
    for scope in [ConfigScope::Global, ConfigScope::Station(host.to_owned())] {
        for (file, definition) in source.definitions(ConfigRecord::Plugin, &scope)?.records {
            if let Some(key) = definition
                .get("publisher")
                .and_then(serde_json::Value::as_str)
            {
                named.entry(key.to_owned()).or_default().insert(file);
            }
        }
    }
    Ok(named)
}

/// What the records at hand say, read once.
struct Records {
    /// The Station's own record for the file, if it has one.
    own: Option<serde_json::Value>,
    /// The global record for the file, which the Station's own would replace.
    global: Option<serde_json::Value>,
    publishers: std::collections::BTreeMap<String, BTreeSet<String>>,
}

/// `POST /v1/user/stations/{id}/libraries/approve`: needs `plugin.manage` on `config:plugin`
/// and `station.read` on the Station. The library has to be one a Station heard from lately
/// reported, by file and digest. Writes the Station's own `plugin` record for the file, which
/// the Station reads at its next start.
///
/// 200 with `applied: false`, what would change, and the `record_revision` and `publisher` to
/// send back, until sent with `confirm`; then 200 with the new revision and what the record gives
/// up. 422 with the reason when the Station reports no such library, when a publisher trust is
/// asked for with a key no record names or that is not the one previewed, or when the
/// configuration it would leave is not acceptable; 409 when the record changed since the
/// preview; 400 when `confirm` lacks what the preview returned.
pub(crate) async fn approve(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(host): Path<String>,
    Json(request): Json<Approve>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let target = format!("station {host}: {} {}", request.file, request.digest);
    require_manage(&gateway, &person, &target).await?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        format!("station:{host}"),
        "station.read",
    )
    .await?;
    if !ai6::ConfigScope::is_plain(&host) {
        return Err(ApiError::bad_request());
    }
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
    let Some(station) = fresh_station(&gateway, &host).await? else {
        return Err(ApiError::not_found());
    };
    let name_bytes = gateway.config.unex().text.roster_key_bytes;
    let Some(library) = published(station.info(), name_bytes)
        .into_iter()
        .find(|library| library.file == request.file && library.digest == request.digest)
    else {
        return refuse(format!(
            "{host} reports no library {} with digest {}",
            request.file, request.digest
        ))
        .await;
    };
    if request.confirm
        && (request.record_revision.is_none()
            || (request.trust == Trust::Publisher && request.publisher.is_none()))
    {
        return Err(ApiError::invalid(
            "confirm carries the record_revision (and for a publisher, the publisher) the \
             preview returned"
                .to_owned(),
        ));
    }
    let source = gateway.roster.clone().ok_or_else(ApiError::unavailable)?;
    let records = {
        let source = source.clone();
        let (file, host) = (library.file.clone(), host.clone());
        tokio::task::spawn_blocking(move || {
            let find =
                |scope: &ConfigScope| -> Result<Option<serde_json::Value>, ai6::RosterError> {
                    Ok(source
                        .definitions(ConfigRecord::Plugin, scope)?
                        .records
                        .into_iter()
                        .find(|(key, _)| *key == file)
                        .map(|(_, definition)| definition))
                };
            Ok::<_, ai6::RosterError>(Records {
                own: find(&ConfigScope::Station(host.clone()))?,
                global: find(&ConfigScope::Global)?,
                publishers: named_publishers(source.as_ref(), &host)?,
            })
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::unavailable())?
    };
    let key = library.file.clone();
    let trust = request.trust;
    let admitted = match trust {
        Trust::Pin => library.digest.clone(),
        Trust::Publisher => {
            let Some(reported) = library.publisher.clone() else {
                return refuse(
                    "no publisher key this Station's records name signed it; approve the \
                     digest instead"
                        .to_owned(),
                )
                .await;
            };
            if !records.publishers.contains_key(&reported) {
                return refuse(format!(
                    "{reported} is named by no plugin record, so the gateway cannot vouch \
                     for it; approve the digest instead"
                ))
                .await;
            }
            if request
                .publisher
                .as_deref()
                .is_some_and(|sent| sent != reported)
            {
                return refuse(format!(
                    "the publisher changed since the preview: it is now {reported}"
                ))
                .await;
            }
            reported
        }
    };
    let own = records.own.clone();
    let both = own
        .as_ref()
        .and_then(|own| own.get("require"))
        .and_then(serde_json::Value::as_str)
        == Some("both");
    if trust == Trust::Publisher && both {
        return refuse(
            "the record requires both a pin and a publisher; approve the digest instead".to_owned(),
        )
        .await;
    }
    // Only what was approved is written, so a record the Station does not have yet does not
    // become a frozen copy of the global one.
    let mut definition = own.clone().unwrap_or_else(|| serde_json::json!({}));
    let object = definition.as_object_mut().ok_or_else(ApiError::internal)?;
    let field = match trust {
        Trust::Pin => "pin",
        Trust::Publisher => "publisher",
    };
    let replaced = object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    object.insert(
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
    if own.is_none()
        && let Some(global) = &records.global
    {
        changes.push(format!(
            "{host} has no record of its own for {key}: this one replaces the global record \
             ({global}) on {host}, which it then stops using"
        ));
    }
    if trust == Trust::Publisher {
        let elsewhere: Vec<&str> = records.publishers[&admitted]
            .iter()
            .filter(|file| **file != key)
            .map(String::as_str)
            .collect();
        if !elsewhere.is_empty() {
            changes.push(format!(
                "{admitted} is already trusted for {}, now extended to {key}",
                elsewhere.join(", ")
            ));
        }
    }
    let weakened = {
        let source = source.clone();
        let (key, definition, host) = (key.clone(), definition.clone(), host.clone());
        tokio::task::spawn_blocking(move || {
            loosened(
                source.as_ref(),
                ConfigRecord::Plugin,
                &ConfigScope::Station(host),
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
            record_revision: token(own.as_ref()),
            publisher: (trust == Trust::Publisher).then(|| admitted.clone()),
        })
        .into_response());
    }
    let expected = request.record_revision.unwrap_or_default();
    let outcome = {
        let (key, definition, host) = (key.clone(), definition.clone(), host.clone());
        tokio::task::spawn_blocking(move || {
            store(
                source.as_ref(),
                ConfigRecord::Plugin,
                &ConfigScope::Station(host),
                &key,
                Some(&expected),
                &definition,
            )
        })
        .await
        .map_err(|_| ApiError::internal())?
    };
    match outcome {
        Ok(Ok(revision)) => {
            settle(
                &gateway,
                &person,
                RESOURCE,
                format!("{target} ({field} {admitted})"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn library(file: &str) -> FoundLibrary {
        FoundLibrary {
            file: file.to_owned(),
            digest: format!("sha256:{}", "ab".repeat(32)),
            kinds: vec!["ai6_agent_handler".to_owned()],
            declared: Vec::new(),
            verdict: "unpinned".to_owned(),
            signed: false,
            publisher: None,
        }
    }

    #[test]
    fn a_reported_entry_is_kept_only_when_it_is_well_formed() {
        let bytes = ai6::Limit::at(255);
        assert!(well_formed(&library("ai6_x.dll"), bytes));
        for file in [
            "",
            "a/b.dll",
            "a\\b.dll",
            "ai6\u{202e}lld.exe",
            "ai6\u{200b}.dll",
            "a\n.dll",
        ] {
            assert!(!well_formed(&library(file), bytes), "{file:?}");
        }
        assert!(!well_formed(&library("ai6_x.dll"), ai6::Limit::at(3)));
        for digest in [
            "sha256:AB",
            "sha1:abababababababababababababababababababab",
            &format!("sha256:{}", "AB".repeat(32)),
            &format!("sha256:{}", "ab".repeat(33)),
        ] {
            let mut entry = library("ai6_x.dll");
            entry.digest = digest.to_owned();
            assert!(!well_formed(&entry, bytes), "{digest}");
        }
        let mut entry = library("ai6_x.dll");
        entry.kinds = vec!["ai6\u{202e}".to_owned()];
        assert!(!well_formed(&entry, bytes));
        let mut entry = library("ai6_x.dll");
        entry.declared = vec!["k=\u{feff}v".to_owned()];
        assert!(!well_formed(&entry, bytes));
        let mut entry = library("ai6_x.dll");
        entry.publisher = Some("ed25519:zz".to_owned());
        assert!(!well_formed(&entry, bytes));
    }
}
