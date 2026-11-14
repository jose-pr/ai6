//! Reports as people reach them: `/v1/reports`.
//!
//! `POST /v1/reports` files one; `GET /v1/reports?scope=SCOPE` (or `?mission=ID`) lists them in a
//! charge's order; `GET /v1/reports/{id}` shows one; `PUT /v1/reports/{id}/markings` changes its
//! markings; `GET /v1/missions/{id}/reports` lists the Reports tied to a Mission.
//!
//! `PUT /v1/reports/{id}/assessment` assesses an open Report as a mission, a duplicate, related,
//! no action, awaiting, later or a decision (`mission.control` on its Operation);
//! `POST /v1/reports/{id}/reopen` brings an assessed or closed one back; `GET
//! /v1/reports/sources/{key}` shows a source's reliability with the Reports behind it.
//!
//! `GET /v1/reports/clearances` lists who is cleared for which classification (`access.read`
//! on `config:reports`); `PUT` and `DELETE /v1/reports/clearances/{subject}` grant and revoke one
//! (`access.manage` there), each written to the audit log before it is made.
//!
//! Who sees what is `ai6::directorates::m::reports`, asked at every call: the scope's read
//! (`mission.read` on an Operation, from the access policy or a compartment, the crossing
//! audited; `station.read` on `station:ID`; `station.read` on `config:reports` for M's scope),
//! being read in to each of a Report's compartments, and a clearance at or above its
//! classification. Filing needs `mission.create` on the Operation, or `station.configure` on the
//! Station or on `config:reports`. A Report of a scope the person may not read answers `404`; one
//! they may read but not see the content of answers `403` with its ID and markings.

use std::sync::Arc;

use ai6::DEFAULT_REQUEST_TTL;
use ai6::directorates::m::reports::{
    self, ReportFiling, ReportMarkings, ReportReader, ReportRules, ReportSettings, ReportSight,
    remark, report_json, sight,
};
use ai6::directorates::registry::assessments::{
    self, Assessment, Context, Judgement, Verdict, VerdictParts,
};
use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::reports::{
    ReportClearance, ReportFiler, ReportFilter, ReportScope, ReportState,
};
use ai6::{Authorizer, Person};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::compartments::CompartmentAccess;
use crate::memories::{Refused, refused};
use crate::tree::{OperationTree, resource_request};
use crate::{
    ApiError, Gateway, authorize_resource_action, bearer, operation_tree, resolve_user,
    unix_timestamp,
};

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
    /// The deployment's lists, read for this request.
    settings: ReportSettings,
    /// Every clearance, read for this request.
    clearances: Vec<ReportClearance>,
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

    fn rules(&self) -> ReportRules<'_> {
        ReportRules::new(&self.settings, &self.clearances)
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

/// The deployment's lists and every clearance, as they are now; nothing is kept between
/// requests, so a change holds from the next one.
async fn rules_now(gateway: &Gateway) -> Result<(ReportSettings, Vec<ReportClearance>), Refused> {
    let source = gateway.roster.clone();
    let registry = registry_of(gateway)?;
    blocking(move || {
        let unreadable = |_| {
            refused(
                StatusCode::SERVICE_UNAVAILABLE,
                "The report settings or clearances cannot be read now.",
            )
        };
        let settings = match &source {
            Some(source) => reports::settings(source.as_ref()).map_err(|_| unreadable(()))?,
            None => ReportSettings::default(),
        };
        let clearances = registry.report_clearances().map_err(|_| unreadable(()))?;
        Ok((settings, clearances))
    })
    .await
}

async fn reader_of(gateway: &Gateway, headers: &HeaderMap) -> Result<PersonReader, Refused> {
    let person = resolve_user(gateway, bearer(headers)?).await?;
    let (settings, clearances) = rules_now(gateway).await?;
    Ok(PersonReader {
        person,
        access: gateway.authorizer()?,
        tree: operation_tree(gateway).await?,
        settings,
        clearances,
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
    let seen = sight(reader, report, &reader.rules());
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
        let mut filter = ReportFilter::all().ordered_by(reader.settings.priorities.clone());
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
        Ok(Json(json!({
            "reports": shown,
            "priorities": reader.settings.priorities,
            "classifications": reader.settings.classifications,
            "handlings": reader.settings.handlings,
        })))
    })
    .await
}

/// `GET /v1/reports?scope=SCOPE|mission=ID[&state][&kind][&limit]`: `{"reports": [...],
/// "priorities": [...], "classifications": [...], "handlings": [...]}`, the Reports most urgent
/// first, each as far as the person may see it, and the deployment's priorities (most urgent
/// first), classifications (lowest first) and handling marks (narrowest first), which an
/// assessment and the markings choose from.
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

/// One Report waiting for an assessment, as far as the person may see it.
#[derive(Debug, Clone)]
pub(crate) struct OpenReport {
    pub(crate) id: String,
    /// `operation:ID`, `station:ID` or `m`.
    pub(crate) scope: String,
    /// Only for a Report the person may read the content of.
    pub(crate) title: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) severity: Option<String>,
    pub(crate) priority: Option<String>,
    /// How much of it the person sees: `content`, `tearline` or `markings`.
    pub(crate) sight: String,
    /// When it was filed, as Unix seconds.
    pub(crate) created: Option<f64>,
}

/// The unassessed Reports of some scopes, and which scopes were read.
#[derive(Debug, Default)]
pub(crate) struct OpenReports {
    pub(crate) reports: Vec<OpenReport>,
    pub(crate) checked: Vec<String>,
    /// Scopes the person may not read Reports of; theirs to see or not, not a gap.
    pub(crate) not_permitted: Vec<String>,
    /// Scopes that could not be read now: a gap.
    pub(crate) unavailable: Vec<String>,
}

/// The Reports in `open` state of each of `scopes` (`operation:ID`, `station:ID`, `m`), read
/// through the same listing the Reports page uses, so each scope's read, its compartments and
/// the audit of a crossing are decided there. At most `per_scope` of each are read.
pub(crate) async fn open_reports(
    gateway: &Gateway,
    headers: &HeaderMap,
    scopes: &[String],
    per_scope: usize,
) -> OpenReports {
    let mut found = OpenReports::default();
    for scope in scopes {
        let listing = Listing {
            scope: Some(scope.clone()),
            mission: None,
            state: Some("open".to_owned()),
            kind: None,
            limit: Some(per_scope),
        };
        match list_reports(gateway, headers, listing).await {
            Ok(Json(body)) => {
                found.checked.push(scope.clone());
                for report in body["reports"].as_array().into_iter().flatten() {
                    let text = |key: &str| report[key].as_str().map(str::to_owned);
                    found.reports.push(OpenReport {
                        id: text("id").unwrap_or_default(),
                        scope: scope.clone(),
                        title: text("title"),
                        kind: text("kind"),
                        severity: text("severity"),
                        priority: text("priority"),
                        sight: text("sight").unwrap_or_default(),
                        created: report["created"].as_f64(),
                    });
                }
            }
            Err(error) if error.status == StatusCode::FORBIDDEN => {
                found.not_permitted.push(scope.clone());
            }
            Err(_) => found.unavailable.push(scope.clone()),
        }
    }
    found
}

/// `GET /v1/user/reports/unassessed[?operation_id=ID]`: the Reports still waiting for an
/// assessment in every scope the person reads (their Operations, their Stations, and M's), each
/// with the Reports page link that opens it, and which scopes were and were not read.
pub(crate) async fn unassessed(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<UnassessedQuery>,
) -> Result<Json<Value>, ApiError> {
    resolve_user(&gateway, bearer(&headers)?).await?;
    let wanted = query
        .operation_id
        .map(|operation| {
            ai6_diptel::OperationId::new(operation)
                .map(|operation| operation.to_string())
                .map_err(|_| ApiError::bad_request())
        })
        .transpose()?;
    let now = unix_timestamp()?;
    let limits = gateway.config.unex().timeline.clone();
    let board = crate::stations(State(gateway.clone()), headers.clone())
        .await
        .map(|Json(board)| board)
        .ok();
    let scopes = match &board {
        Some(board) => crate::attention::report_scopes(
            &crate::attention::operations_of(board, wanted.as_deref()),
            board.stations.iter().filter(|station| {
                wanted
                    .as_ref()
                    .is_none_or(|operation| station.operations.contains(operation))
            }),
            wanted.is_some(),
        ),
        None => crate::attention::report_scopes(wanted.iter(), [], wanted.is_some()),
    };
    let read = open_reports(
        &gateway,
        &headers,
        &scopes,
        limits.attention_items.or_max().saturating_add(1),
    )
    .await;
    let found = read.reports.len();
    let keep = if limits.attention_items.allows(found) {
        found
    } else {
        limits.attention_items.or_max()
    };
    let reports: Vec<Value> = read
        .reports
        .iter()
        .take(keep)
        .map(|report| {
            let route = json!({"page": "reports", "report_id": report.id});
            let (route, link) = crate::attention::page_link(&route);
            json!({
                "id": report.id,
                "scope": report.scope,
                "title": report.title.as_deref().map(crate::attention::plain),
                "kind": report.kind,
                "severity": report.severity,
                "priority": report.priority,
                "sight": report.sight,
                "created": report.created,
                "route": route,
                "link": link,
            })
        })
        .collect();
    Ok(Json(json!({
        "checked_at": now,
        "scope": wanted.as_ref().map_or_else(
            || "All accessible Operations and Stations".to_owned(),
            |operation| format!("Operation {operation}"),
        ),
        "reports": reports,
        "omitted": found - keep,
        "coverage": {
            "checked": read.checked,
            "not_permitted": read.not_permitted,
            "unavailable": read.unavailable,
        },
        "complete": read.unavailable.is_empty() && board.is_some(),
    })))
}

/// `?operation_id=ID`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnassessedQuery {
    #[serde(default)]
    operation_id: Option<String>,
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
            .into_report(
                scope,
                filer,
                [],
                at,
                &registry.limits().reports,
                &reader.settings,
            )
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
        let mut refusal = None;
        let changed = registry.change_report(&id, &mut |stored| {
            remark(&reader, stored, markings.clone(), &reader.rules()).map_err(|reason| {
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

/// `PUT /v1/reports/{id}/assessment` with `{"assessment", "reason", ...}`: a person assesses an
/// open Report as `mission` (naming `mission`, unless it is a Source item, which the Station that
/// polls the Source queues), `duplicate` or `related` (naming `of`), `no_action`, `awaiting`
/// (`question`, optionally `ask` and `via`, and `wait_seconds` for its deadline), `later`
/// (`wait_seconds`, `condition` or both) or `decision` (`question`, filed as green ink of kind
/// `decision`), as the Operation's charge does. `credibility`, `priority`, `likelihood` and
/// `confidence` are optional. It needs `mission.control` on the Operation (`station.configure`
/// on a Station's or M's scope) and being able to read the Report's content; nothing becomes a
/// Mission by itself. `404` for a Report of a scope the person may not read, `403` without the
/// action or the content, `409` for a Report that is not open, `400` for anything out of bounds.
pub(crate) async fn assess(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<AssessRequest>,
) -> Result<Json<Value>, Refused> {
    let reader = reader_of(&gateway, &headers).await?;
    let registry = registry_of(&gateway)?;
    let at = unix_timestamp()?;
    blocking(move || {
        let report = controlled(registry.as_ref(), &reader, &id)?;
        let parts = VerdictParts {
            of: request.of.as_deref(),
            mission: request.mission.as_deref(),
            question: request.question.as_deref(),
            via: request.via.as_deref(),
            ask: request.ask.as_deref(),
            condition: request.condition.as_deref(),
            wait_seconds: request.wait_seconds,
        };
        let bad = |reason: String| refused(StatusCode::BAD_REQUEST, reason);
        let verdict = Verdict::parse(&request.assessment, &parts, at).map_err(bad)?;
        let judgement =
            Judgement::parse(request.likelihood.as_deref(), request.confidence.as_deref())
                .map_err(bad)?;
        let assessment =
            Assessment::new(&report.id, verdict, &request.reason, &reader.person.id, at)
                .believed(request.credibility)
                .prioritised(request.priority.as_deref())
                .judged(judgement);
        let reads = |scope: &ReportScope| reader.reads_scope(scope);
        let context = Context {
            settings: &reader.settings,
            reads: &reads,
            request_ttl: DEFAULT_REQUEST_TTL,
            agent: "person",
        };
        match assessments::assess(registry.as_ref(), &assessment, &context) {
            Ok(report) => Ok(Json(shown(registry.as_ref(), &reader, &report))),
            Err(error) => Err(store_refusal(&error)),
        }
    })
    .await
}

/// The Report `id` when the person may assess it: they read its scope, hold the action that
/// controls it and read its content.
fn controlled(
    registry: &dyn RegistryBackend,
    reader: &PersonReader,
    id: &str,
) -> Result<ai6::directorates::registry::reports::Report, Refused> {
    let missing = || refused(StatusCode::NOT_FOUND, "No such report.");
    let report = registry.report(id).ok().flatten().ok_or_else(missing)?;
    if !reader.reads_scope(&report.scope) {
        return Err(missing());
    }
    let holds = match &report.scope {
        ReportScope::Operation(operation) => {
            reader.holds("mission.control", format!("operation:{operation}"))
        }
        scope => reader.files_in(scope),
    };
    if !holds || sight(reader, &report, &reader.rules()) != ReportSight::Content {
        return Err(refused(
            StatusCode::FORBIDDEN,
            "Assessing a report needs mission.control on its Operation (station.configure on a \
             Station or M) and reading its content.",
        ));
    }
    Ok(report)
}

/// What a store refusal is to the person.
fn store_refusal(error: &std::io::Error) -> Refused {
    match error.kind() {
        std::io::ErrorKind::NotFound => refused(StatusCode::NOT_FOUND, "No such report."),
        std::io::ErrorKind::InvalidInput => {
            let status = if error.to_string().contains("only an open report") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            refused(status, error.to_string())
        }
        _ => Refused::from(ApiError::internal()),
    }
}

/// What a person assesses a Report as.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssessRequest {
    /// `mission`, `duplicate`, `related`, `no_action`, `awaiting`, `later` or `decision`.
    assessment: String,
    /// Why, kept apart from the Report's own words.
    reason: String,
    /// For duplicate or related: the Report, Mission or evidence.
    #[serde(default)]
    of: Option<String>,
    /// For mission: the Mission proposed or amended for it.
    #[serde(default)]
    mission: Option<String>,
    /// For awaiting or decision: what is asked.
    #[serde(default)]
    question: Option<String>,
    /// For awaiting: who or what is asked.
    #[serde(default)]
    ask: Option<String>,
    /// For awaiting: how far the question goes, a handling mark.
    #[serde(default)]
    via: Option<String>,
    /// For later: what it waits for.
    #[serde(default)]
    condition: Option<String>,
    /// For awaiting (its deadline) or later (its date): seconds from now.
    #[serde(default)]
    wait_seconds: Option<u64>,
    /// `1` to `6`.
    #[serde(default)]
    credibility: Option<u8>,
    /// From the deployment's priorities.
    #[serde(default)]
    priority: Option<String>,
    /// The probability yardstick's word for what the Report says.
    #[serde(default)]
    likelihood: Option<String>,
    /// `high`, `moderate` or `low`.
    #[serde(default)]
    confidence: Option<String>,
}

/// `POST /v1/reports/{id}/reopen` with `{"reason"}`: opens an assessed or closed Report again,
/// audited: the answer to what it awaited arrived, or the condition it waited for is met. The
/// same authority as assessing it. `409` for one already open.
pub(crate) async fn reopen(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<ReopenRequest>,
) -> Result<Json<Value>, Refused> {
    let reader = reader_of(&gateway, &headers).await?;
    let registry = registry_of(&gateway)?;
    let at = unix_timestamp()?;
    blocking(move || {
        controlled(registry.as_ref(), &reader, &id)?;
        match assessments::reopen(
            registry.as_ref(),
            &id,
            &reader.person.id,
            &request.reason,
            at,
        ) {
            Ok(report) => Ok(Json(shown(registry.as_ref(), &reader, &report))),
            Err(error) if error.to_string().contains("open already") => {
                Err(refused(StatusCode::CONFLICT, error.to_string()))
            }
            Err(error) => Err(store_refusal(&error)),
        }
    })
    .await
}

/// Why a Report comes back.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReopenRequest {
    reason: String,
}

/// `GET /v1/reports/sources/{key}`: a source's reliability, how it was earned (the tally and the
/// deployment's rule) and the closed Reports behind it that the person may read, each as far as
/// they may see it.
pub(crate) async fn source(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Json<Value>, Refused> {
    let reader = reader_of(&gateway, &headers).await?;
    let registry = registry_of(&gateway)?;
    blocking(move || {
        let standing = assessments::standing(registry.as_ref(), &key).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                refused(StatusCode::BAD_REQUEST, error.to_string())
            } else {
                Refused::from(ApiError::internal())
            }
        })?;
        let behind: Vec<Value> = standing
            .behind
            .iter()
            .filter(|report| reader.reads_scope(&report.scope))
            .map(|report| shown(registry.as_ref(), &reader, report))
            .collect();
        let rule = &reader.settings;
        Ok(Json(json!({
            "source": standing.source.key,
            "reliability": standing.source.reliability.as_str(),
            "rated": standing.source.rated,
            "counted": standing.source.counted,
            "confirmed": standing.source.confirmed,
            "rule": {
                "min_reports": rule.rating_min_reports,
                "counts": rule.rating_counts,
                "confirms": rule.rating_confirms,
                "bands": rule.rating_bands,
            },
            "reports": behind,
        })))
    })
    .await
}

/// What grants and revokes a clearance, and what lists them: a clearance hands a person or an
/// Agent more than their access, so it takes what handing out access takes.
const CLEARANCE_MANAGE: &str = "access.manage";
const CLEARANCE_READ: &str = "access.read";

/// Writes a clearance change to the audit log; a failed write refuses the change.
async fn audit_clearance(
    gateway: &Gateway,
    person: &Person,
    action: &str,
    target: String,
) -> Result<(), Refused> {
    let registry = registry_of(gateway)?;
    let record = ai6::AuditRecord {
        id: format!("clearance-{}", uuid::Uuid::new_v4().simple()),
        actor: crate::actor_of(person),
        action: action.to_owned(),
        operation: String::new(),
        resource: Some(M_RESOURCE.to_owned()),
        target,
        at: unix_timestamp()?,
    };
    blocking(move || {
        registry.record_audit(&record).map_err(|_| {
            refused(
                StatusCode::SERVICE_UNAVAILABLE,
                "Nothing was changed: it could not be audited.",
            )
        })
    })
    .await
}

/// `GET /v1/reports/clearances`: `{"clearances": [...], "classifications": [...]}`.
pub(crate) async fn clearances(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<Value>, Refused> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(&gateway, person, M_RESOURCE.to_owned(), CLEARANCE_READ).await?;
    let (settings, clearances) = rules_now(&gateway).await?;
    Ok(Json(json!({
        "clearances": clearances,
        "classifications": settings.classifications,
    })))
}

/// The body of a grant.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Grant {
    classification: String,
}

/// `PUT /v1/reports/clearances/{subject}`: `{"classification"}`, replacing the subject's.
pub(crate) async fn grant(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(subject): Path<String>,
    Json(grant): Json<Grant>,
) -> Result<Json<Value>, Refused> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        M_RESOURCE.to_owned(),
        CLEARANCE_MANAGE,
    )
    .await?;
    let registry = registry_of(&gateway)?;
    let (settings, _) = rules_now(&gateway).await?;
    if !settings.classifications.contains(&grant.classification) {
        return Err(refused(
            StatusCode::BAD_REQUEST,
            format!(
                "{:?} is not one of the classifications: {}",
                grant.classification,
                settings.classifications.join(", ")
            ),
        ));
    }
    let clearance = ReportClearance::new(
        subject.clone(),
        grant.classification.clone(),
        format!("person:{}", person.id),
        unix_timestamp()?,
    );
    clearance
        .validate(&registry.limits().reports)
        .map_err(|error| refused(StatusCode::BAD_REQUEST, error.to_string()))?;
    audit_clearance(
        &gateway,
        &person,
        "report.clearance.grant",
        format!("{subject} {}", grant.classification),
    )
    .await?;
    let stored = clearance.clone();
    blocking(move || {
        registry
            .put_report_clearance(&stored)
            .map_err(|_| Refused::from(ApiError::internal()))
    })
    .await?;
    Ok(Json(json!(clearance)))
}

/// `DELETE /v1/reports/clearances/{subject}`: `{"revoked": bool}`; it holds from the next call.
pub(crate) async fn revoke(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(subject): Path<String>,
) -> Result<Json<Value>, Refused> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        M_RESOURCE.to_owned(),
        CLEARANCE_MANAGE,
    )
    .await?;
    let registry = registry_of(&gateway)?;
    audit_clearance(
        &gateway,
        &person,
        "report.clearance.revoke",
        subject.clone(),
    )
    .await?;
    let revoked = blocking(move || {
        registry
            .revoke_report_clearance(&subject)
            .map_err(|_| Refused::from(ApiError::internal()))
    })
    .await?;
    Ok(Json(json!({ "revoked": revoked })))
}

#[cfg(test)]
#[path = "reports_tests.rs"]
mod tests;
