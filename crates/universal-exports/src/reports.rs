//! Reports as people reach them: `/v1/reports`.
//!
//! `POST /v1/reports` files one; `GET /v1/reports?scope=SCOPE` (or `?mission=ID`) lists them in a
//! watch's order; `GET /v1/reports/{id}` shows one; `PUT /v1/reports/{id}/markings` changes its
//! markings; `GET /v1/missions/{id}/reports` lists the Reports tied to a Mission.
//!
//! Who sees what is `ai6::directorates::m::reports`, asked at every call: the scope's read
//! (`mission.read` on an Operation, from the access policy or a compartment, the crossing
//! audited; `station.read` on `station:ID`; `station.read` on `config:reports` for M's scope),
//! being read in to each of a Report's compartments, and a clearance at or above its
//! classification. Filing needs `mission.create` on the Operation, or `station.configure` on the
//! Station or on `config:reports`. A Report of a scope the person may not read answers `404`; one
//! they may read but not see the content of answers `403` with its ID and markings.

use std::sync::Arc;

use ai6::directorates::m::reports::{
    ReportFiling, ReportMarkings, ReportReader, ReportSight, remark, report_json, sight,
};
use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::reports::{ReportFiler, ReportFilter, ReportScope, ReportState};
use ai6::{Authorizer, Person};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::compartments::CompartmentAccess;
use crate::memories::{Refused, refused};
use crate::tree::{OperationTree, resource_request};
use crate::{ApiError, Gateway, bearer, operation_tree, resolve_user, unix_timestamp};

/// The resource M's own Reports are read and filed at.
pub(crate) const M_RESOURCE: &str = "config:reports";

/// Where a scope's Reports are authorized, what reads them and what files them.
fn actions_of(scope: &ReportScope) -> (String, &'static str, &'static str) {
    match scope {
        ReportScope::Operation(id) => (format!("operation:{id}"), "mission.read", "mission.create"),
        ReportScope::Station(id) => (format!("station:{id}"), "station.read", "station.configure"),
        _ => (M_RESOURCE.to_owned(), "station.read", "station.configure"),
    }
}

/// A person asking about Reports, as granted for this request.
struct PersonReader {
    person: Person,
    access: Arc<CompartmentAccess>,
    tree: OperationTree,
}

impl PersonReader {
    fn holds(&self, action: &str, resource: String) -> bool {
        self.access
            .authorize(resource_request(
                self.person.clone(),
                action,
                resource,
                &self.tree,
            ))
            .is_ok_and(|decision| decision.allowed)
    }

    fn files_in(&self, scope: &ReportScope) -> bool {
        let (resource, _, file) = actions_of(scope);
        self.holds(file, resource)
    }
}

impl ReportReader for PersonReader {
    fn reads_scope(&self, scope: &ReportScope) -> bool {
        let (resource, read, _) = actions_of(scope);
        self.holds(read, resource)
    }

    fn read_in(&self, compartment: &str) -> bool {
        self.access
            .compiled()
            .is_some_and(|compiled| compiled.person_read_in(compartment, &self.person))
    }

    fn subjects(&self) -> Vec<String> {
        std::iter::once(format!("person:{}", self.person.id))
            .chain(
                self.person
                    .groups
                    .iter()
                    .map(|group| format!("group:{group}")),
            )
            .collect()
    }
}

async fn reader_of(gateway: &Gateway, headers: &HeaderMap) -> Result<PersonReader, Refused> {
    let person = resolve_user(gateway, bearer(headers)?).await?;
    Ok(PersonReader {
        person,
        access: gateway.authorizer()?,
        tree: operation_tree(gateway).await?,
    })
}

fn registry_of(gateway: &Gateway) -> Result<Arc<dyn RegistryBackend>, Refused> {
    gateway.registry.clone().ok_or_else(|| {
        refused(
            StatusCode::SERVICE_UNAVAILABLE,
            "This gateway keeps no Registry, so it holds no reports.",
        )
    })
}

/// Runs `work` on the blocking pool, where the access policy and the Registry are asked.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Refused> + Send + 'static,
) -> Result<T, Refused> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| Refused::from(ApiError::internal()))?
}

fn scope_of(text: &str) -> Result<ReportScope, Refused> {
    text.parse()
        .map_err(|reason: String| refused(StatusCode::BAD_REQUEST, reason))
}

/// `report` as `reader` may see it, with its source's reliability beside the content.
fn shown(
    registry: &dyn RegistryBackend,
    reader: &PersonReader,
    report: &ai6::directorates::registry::reports::Report,
) -> Value {
    let seen = sight(reader, report, &registry.limits().reports);
    let source = (seen == ReportSight::Content)
        .then(|| registry.report_source(&report.filer.source_key()).ok())
        .flatten();
    report_json(report, seen, source.as_ref())
}

/// `?scope=SCOPE&mission=ID&state=STATE&kind=KIND&limit=N`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Listing {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    mission: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// The Operation of the Mission `id`, as the Registry filed it.
fn operation_of_mission(registry: &dyn RegistryBackend, id: &str) -> Option<String> {
    let uid = registry
        .resolve_mission(id)
        .ok()
        .flatten()
        .map_or_else(|| id.to_owned(), |handle| handle.mission_id);
    registry
        .filed_brief(&uid)
        .ok()
        .flatten()
        .and_then(|filed| filed.brief.operation)
}

async fn list_reports(
    gateway: &Gateway,
    headers: &HeaderMap,
    listing: Listing,
) -> Result<Json<Value>, Refused> {
    let reader = reader_of(gateway, headers).await?;
    let registry = registry_of(gateway)?;
    blocking(move || {
        let mut filter = ReportFilter::all();
        match (&listing.scope, &listing.mission) {
            (None, None) => {
                return Err(refused(
                    StatusCode::BAD_REQUEST,
                    "Name a scope (operation:ID, station:ID or m) or a Mission.",
                ));
            }
            (Some(scope), _) => {
                let scope = scope_of(scope)?;
                if !reader.reads_scope(&scope) {
                    return Err(refused(
                        StatusCode::FORBIDDEN,
                        format!("You may not read the reports of {scope}."),
                    ));
                }
                filter = filter.in_scope(scope);
            }
            (None, Some(_)) => {}
        }
        if let Some(mission) = &listing.mission {
            // A Mission's link shows a Report to whoever reads the Mission, as far as they may.
            let readable = operation_of_mission(registry.as_ref(), mission)
                .is_some_and(|operation| reader.reads_scope(&ReportScope::Operation(operation)));
            if !readable {
                return Err(refused(StatusCode::NOT_FOUND, "No such Mission."));
            }
            filter = filter.about_mission(mission.clone());
        }
        if let Some(state) = &listing.state {
            let state: ReportState = state
                .parse()
                .map_err(|reason: String| refused(StatusCode::BAD_REQUEST, reason))?;
            filter = filter.in_state(state);
        }
        if let Some(kind) = listing.kind {
            filter = filter.of_kind(kind);
        }
        if let Some(limit) = listing.limit {
            filter = filter.at_most(limit);
        }
        let reports = registry
            .reports(&filter)
            .map_err(|_| Refused::from(ApiError::internal()))?;
        let shown: Vec<Value> = reports
            .iter()
            .map(|report| shown(registry.as_ref(), &reader, report))
            .collect();
        Ok(Json(json!({ "reports": shown })))
    })
    .await
}

/// `GET /v1/reports?scope=SCOPE|mission=ID[&state][&kind][&limit]`: `{"reports": [...]}`, most
/// urgent first, each as far as the person may see it.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(listing): Query<Listing>,
) -> Result<Json<Value>, Refused> {
    list_reports(&gateway, &headers, listing).await
}

/// `GET /v1/missions/{id}/reports`: the Reports tied to a Mission the person reads; one they are
/// not read in to shows its ID and markings alone.
pub(crate) async fn of_mission(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Refused> {
    let listing = Listing {
        scope: None,
        mission: Some(id),
        state: None,
        kind: None,
        limit: None,
    };
    list_reports(&gateway, &headers, listing).await
}

/// `GET /v1/reports/{id}`: the Report as far as the person may see it.
pub(crate) async fn show(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Refused> {
    let reader = reader_of(&gateway, &headers).await?;
    let registry = registry_of(&gateway)?;
    blocking(move || {
        let missing = || refused(StatusCode::NOT_FOUND, "No such report.");
        let report = registry.report(&id).ok().flatten().ok_or_else(missing)?;
        if !reader.reads_scope(&report.scope) {
            return Err(missing());
        }
        let shown = shown(registry.as_ref(), &reader, &report);
        if shown["sight"] == "markings" {
            return Err(Refused {
                status: StatusCode::FORBIDDEN,
                body: json!({
                    "error": "You are not read in to this report, or not cleared for it.",
                    "report": shown,
                }),
            });
        }
        Ok(Json(shown))
    })
    .await
}

/// `POST /v1/reports`: `{"scope": SCOPE, "title", "text", ...}`, filed by the person; answers
/// `201` with the Report as they see it. It creates no Mission.
pub(crate) async fn file(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), Refused> {
    let reader = reader_of(&gateway, &headers).await?;
    let registry = registry_of(&gateway)?;
    let scope = match body.as_object_mut().and_then(|body| body.remove("scope")) {
        Some(Value::String(scope)) => scope_of(&scope)?,
        _ => {
            return Err(refused(
                StatusCode::BAD_REQUEST,
                "A report names its scope: operation:ID, station:ID or m.",
            ));
        }
    };
    let filing: ReportFiling = serde_json::from_value(body)
        .map_err(|error| refused(StatusCode::BAD_REQUEST, error.to_string()))?;
    let at = unix_timestamp()?;
    blocking(move || {
        if !reader.files_in(&scope) {
            return Err(refused(
                StatusCode::FORBIDDEN,
                format!("You may not file reports in {scope}."),
            ));
        }
        let filer = ReportFiler::Person {
            person: reader.person.id.clone(),
        };
        let report = filing
            .into_report(scope, filer, [], at, &registry.limits().reports)
            .map_err(|reason| refused(StatusCode::BAD_REQUEST, reason))?;
        registry
            .file_report(&report)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::InvalidInput => {
                    refused(StatusCode::BAD_REQUEST, error.to_string())
                }
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::QuotaExceeded => {
                    refused(StatusCode::CONFLICT, error.to_string())
                }
                _ => Refused::from(ApiError::internal()),
            })?;
        Ok((
            StatusCode::CREATED,
            Json(shown(registry.as_ref(), &reader, &report)),
        ))
    })
    .await
}

/// `PUT /v1/reports/{id}/markings`: `{"classification", "handling", "compartments",
/// "tearline"}`, replacing all four. The person must read the content as it is and as it will
/// be, so lowering them needs someone read in.
pub(crate) async fn mark(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(markings): Json<ReportMarkings>,
) -> Result<Json<Value>, Refused> {
    let reader = reader_of(&gateway, &headers).await?;
    let registry = registry_of(&gateway)?;
    let at = unix_timestamp()?;
    blocking(move || {
        let missing = || refused(StatusCode::NOT_FOUND, "No such report.");
        let report = registry.report(&id).ok().flatten().ok_or_else(missing)?;
        if !reader.reads_scope(&report.scope) {
            return Err(missing());
        }
        let limits = registry.limits().reports.clone();
        let mut refusal = None;
        let changed = registry.change_report(&id, &mut |stored| {
            remark(&reader, stored, markings.clone(), &limits).map_err(|reason| {
                refusal = Some(reason.clone());
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, reason)
            })?;
            stored.updated = stored.updated.max(at);
            Ok(())
        });
        match changed {
            Ok(report) => Ok(Json(shown(registry.as_ref(), &reader, &report))),
            Err(_) if refusal.is_some() => {
                Err(refused(StatusCode::FORBIDDEN, refusal.unwrap_or_default()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
                Err(refused(StatusCode::BAD_REQUEST, error.to_string()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(missing()),
            Err(_) => Err(Refused::from(ApiError::internal())),
        }
    })
    .await
}

#[cfg(test)]
#[path = "reports_tests.rs"]
mod tests;
