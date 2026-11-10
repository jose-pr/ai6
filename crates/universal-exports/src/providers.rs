//! The providers each host has loaded, with the schema and form of each one's configuration.
//!
//! A Station publishes its catalog at check-in; the gateway adds its own. The app draws a
//! provider's settings from what is reported here, so a new kind or provider needs no change in
//! AI6.

use std::collections::BTreeMap;

use ai6::providers::ProviderEntry;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::{
    ApiError, Gateway, authorize_resource_action, bearer, ensure_station_registered, resolve_user,
    resource_action_allowed,
};

/// The name the gateway's own providers are listed under.
pub(crate) const GATEWAY: &str = "gateway";

/// What `GET /v1/user/stations/{id}/providers` answers: one host's providers.
#[derive(Debug, Serialize)]
pub(crate) struct HostProviders {
    /// The Station, or `gateway`.
    pub host: String,
    /// What each loaded provider serves, and the schema and form of its configuration.
    pub providers: Vec<ProviderEntry>,
}

/// One provider across every host that reports it.
#[derive(Debug, Serialize)]
pub(crate) struct ReportedProvider {
    #[serde(flatten)]
    pub entry: ProviderEntry,
    /// The hosts that have it loaded.
    pub hosts: Vec<String>,
}

/// What `GET /v1/user/providers` answers: every provider the hosts report.
#[derive(Debug, Serialize)]
pub(crate) struct ProvidersReport {
    /// Each provider once (by ID); the first host's schema and form stand for the rest unless
    /// the ID is in `differs`.
    pub providers: Vec<ReportedProvider>,
    /// The hosts that reported, the gateway among them.
    pub hosts: Vec<String>,
    /// Providers whose schema or form is not the same on every host that has them.
    pub differs: Vec<String>,
}

/// Which providers a request asks for.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Wanted {
    /// Only providers serving this Guatiao kind.
    kind: Option<String>,
}

/// The catalog a Station last published, empty when it publishes none.
pub(crate) fn published(info: &BTreeMap<String, serde_json::Value>) -> Vec<ProviderEntry> {
    info.get("providers")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// `GET /v1/user/stations/{id}/providers`: needs `station.read` on the Station; the gateway's
/// own, under the id `gateway`, need `config.read` on `config:unex`.
pub(crate) async fn of_host(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(host): Path<String>,
    Query(wanted): Query<Wanted>,
) -> Result<Json<HostProviders>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    if host == GATEWAY {
        authorize_resource_action(&gateway, person, "config:unex".to_owned(), "config.read")
            .await?;
        return Ok(Json(HostProviders {
            host,
            providers: filtered(gateway.own_providers.to_vec(), &wanted),
        }));
    }
    authorize_resource_action(&gateway, person, format!("station:{host}"), "station.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let station = ensure_station_registered(&registry, &host).await?;
    Ok(Json(HostProviders {
        host,
        providers: filtered(published(station.info()), &wanted),
    }))
}

/// `GET /v1/user/providers`: the providers every host the person may read reports, merged.
pub(crate) async fn all(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(wanted): Query<Wanted>,
) -> Result<Json<ProvidersReport>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    Ok(Json(merged(reports(&gateway, person).await?, &wanted)))
}

/// What every host has loaded, by host, whoever asks; for the checks a write makes.
pub(crate) async fn every_host(
    gateway: &Gateway,
) -> Result<Vec<(String, Vec<ProviderEntry>)>, ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let records = tokio::task::spawn_blocking(move || {
        registry
            .stations(std::time::Duration::MAX)
            .map_err(|_| ApiError::internal())
    })
    .await
    .map_err(|_| ApiError::internal())??;
    let mut reports: Vec<(String, Vec<ProviderEntry>)> = records
        .iter()
        .map(|record| (record.station().to_owned(), published(record.info())))
        .collect();
    reports.push((GATEWAY.to_owned(), gateway.own_providers.to_vec()));
    reports.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(reports)
}

/// What each host the person may read has loaded, by host.
pub(crate) async fn reports(
    gateway: &Gateway,
    person: ai6::Person,
) -> Result<Vec<(String, Vec<ProviderEntry>)>, ApiError> {
    let mut allowed = Vec::new();
    for (host, entries) in every_host(gateway).await? {
        let (resource, action) = if host == GATEWAY {
            ("config:unex".to_owned(), "config.read")
        } else {
            (format!("station:{host}"), "station.read")
        };
        if resource_action_allowed(gateway, person.clone(), resource, action).await? {
            allowed.push((host, entries));
        }
    }
    Ok(allowed)
}

fn filtered(mut entries: Vec<ProviderEntry>, wanted: &Wanted) -> Vec<ProviderEntry> {
    if let Some(kind) = &wanted.kind {
        entries.retain(|entry| entry.serves(kind));
    }
    entries
}

/// Each provider once, with the hosts that have it.
pub(crate) fn merged(
    reports: Vec<(String, Vec<ProviderEntry>)>,
    wanted: &Wanted,
) -> ProvidersReport {
    let hosts = reports.iter().map(|(host, _)| host.clone()).collect();
    let mut found: BTreeMap<String, ReportedProvider> = BTreeMap::new();
    let mut differs = Vec::new();
    for (host, entries) in reports {
        for entry in filtered(entries, wanted) {
            if let Some(known) = found.get_mut(&entry.id) {
                if known.entry.digest != entry.digest && !differs.contains(&entry.id) {
                    differs.push(entry.id.clone());
                }
                for kind in &entry.kinds {
                    if !known.entry.kinds.contains(kind) {
                        known.entry.kinds.push(kind.clone());
                    }
                }
                if known.entry.schema.is_none() && entry.schema.is_some() {
                    known.entry.schema = entry.schema;
                    known.entry.form = entry.form;
                    known.entry.schema_omitted = false;
                }
                known.hosts.push(host.clone());
            } else {
                found.insert(
                    entry.id.clone(),
                    ReportedProvider {
                        entry,
                        hosts: vec![host.clone()],
                    },
                );
            }
        }
    }
    ProvidersReport {
        providers: found.into_values().collect(),
        hosts,
        differs,
    }
}
