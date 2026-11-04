//! Compartments for people: what the gateway adds to the access policy, and the routes that
//! manage them.
//!
//! A person holds an action where the access policy allows it or a compartment read into them
//! grants it; compartments only add. Only the actions a compartment can grant are asked of it,
//! so a person's compartment can never answer for anything else. Every question reads what is
//! granted now from the configuration provider; one request asks once.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use ai6::directorates::m::compartments::current;
use ai6::directorates::registry::backend::*;
use ai6::{
    AuthorizationDecision, AuthorizationRequest, Authorizer, Compartment, CompartmentAction,
    CompartmentResource, CompartmentSettings, CompiledCompartments, ConfigProvider, ConfigRecord,
    ConfigScope, CoverTarget, Person, ReadInSubject, RosterError, RosterErrorKind,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use guatiao::library::ProviderError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::access_admin::token;
use crate::roster::bounded;
use crate::tree::{OperationTree, operation_request};
use crate::{
    ApiError, Gateway, authorize_resource_action, bearer, operation_tree, resolve_user,
    unix_timestamp,
};

/// When each person's read through a compartment was last written, by what it read.
pub(crate) type AuditMemory = Arc<Mutex<BTreeMap<String, f64>>>;

/// The compartment action a gateway action means; `None` for one no compartment can grant.
fn granted_by(action: &str) -> Option<CompartmentAction> {
    match action {
        "mission.status" => Some(CompartmentAction::MissionStatus),
        "mission.read" => Some(CompartmentAction::MissionRead),
        "product.read" => Some(CompartmentAction::ProductRead),
        "cable.send" => Some(CompartmentAction::CableSend),
        "cable.read" => Some(CompartmentAction::CableRead),
        _ => None,
    }
}

/// Whether a read of `action` is written to the audit log. Her Cables are logged by the relay.
fn is_a_read(action: CompartmentAction) -> bool {
    matches!(
        action,
        CompartmentAction::MissionStatus
            | CompartmentAction::MissionRead
            | CompartmentAction::ProductRead
            | CompartmentAction::MemoryRead
    )
}

/// The access policy with compartments added, for one request.
pub(crate) struct CompartmentAccess {
    inner: Arc<dyn Authorizer>,
    source: Option<Arc<dyn ConfigProvider>>,
    registry: Option<Arc<dyn RegistryBackend>>,
    memory: AuditMemory,
    /// Seconds between records of one read; `None` asks and writes nothing, for what only
    /// shows a person what they could read.
    audit_every: Option<f64>,
    read: OnceLock<Option<CompiledCompartments>>,
}

impl CompartmentAccess {
    pub(crate) fn new(
        inner: Arc<dyn Authorizer>,
        source: Option<Arc<dyn ConfigProvider>>,
        registry: Option<Arc<dyn RegistryBackend>>,
        memory: AuditMemory,
        audit_every: Option<f64>,
    ) -> Self {
        Self {
            inner,
            source,
            registry,
            memory,
            audit_every,
            read: OnceLock::new(),
        }
    }

    /// What is granted now, read once for this request; `None` when it cannot be read, which
    /// grants nothing. Without a provider there are no compartments.
    pub(crate) fn compiled(&self) -> Option<&CompiledCompartments> {
        self.read
            .get_or_init(|| match &self.source {
                Some(source) => current(source.as_ref()).ok(),
                None => Some(CompiledCompartments::default()),
            })
            .as_ref()
    }

    /// What the access policy alone answers, without what compartments add.
    pub(crate) fn policy_allows(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        self.inner.authorize(request)
    }

    /// The compartment that gives `person` `action` on `resource`, the read written to the log.
    /// A failed write refuses.
    pub(crate) fn through(
        &self,
        person: &Person,
        action: CompartmentAction,
        resource: &CompartmentResource,
    ) -> Option<String> {
        let compartment = self
            .compiled()?
            .person_allows(person, action, resource)?
            .to_owned();
        if is_a_read(action) {
            self.write(person, &compartment, action, resource).ok()?;
        }
        Some(compartment)
    }

    fn write(
        &self,
        person: &Person,
        compartment: &str,
        action: CompartmentAction,
        resource: &CompartmentResource,
    ) -> Result<(), ()> {
        let Some(every) = self.audit_every else {
            return Ok(());
        };
        let Some(registry) = &self.registry else {
            return Err(());
        };
        let at = unix_timestamp().map_err(|_| ())?;
        let key = format!(
            "{}\u{1f}{compartment}\u{1f}{action}\u{1f}{resource}",
            person.id
        );
        {
            let mut seen = self.memory.lock().unwrap_or_else(PoisonError::into_inner);
            if seen.get(&key).is_some_and(|last| at - last < every) {
                return Ok(());
            }
            seen.retain(|_, last| at - *last < every);
        }
        let operation = match resource {
            CompartmentResource::Operation(id) => id.clone(),
            CompartmentResource::Mission { operation, .. } => operation.clone(),
            _ => String::new(),
        };
        registry
            .record_audit(&ai6::AuditRecord {
                id: format!("compartment-{}", Uuid::new_v4().simple()),
                actor: person.id.clone(),
                action: "compartment.read".to_owned(),
                operation,
                resource: Some(resource.to_string()),
                target: format!("{compartment} {action}"),
                at,
            })
            .map_err(|_| ())?;
        self.memory
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, at);
        Ok(())
    }

    /// Whether a compartment gives `person` `action` on one Mission of `operation`.
    pub(crate) fn allows_mission(
        &self,
        person: &Person,
        action: CompartmentAction,
        mission: &str,
        operation: &str,
    ) -> bool {
        self.through(
            person,
            action,
            &CompartmentResource::Mission {
                id: mission.to_owned(),
                operation: operation.to_owned(),
            },
        )
        .is_some()
    }
}

impl Authorizer for CompartmentAccess {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        let decision = self.inner.authorize(request.clone())?;
        if decision.allowed {
            return Ok(decision);
        }
        let granted = granted_by(&request.action).zip(request.resource.strip_prefix("operation:"));
        let Some((action, operation)) = granted else {
            return Ok(decision);
        };
        let resource = CompartmentResource::Operation(operation.to_owned());
        Ok(AuthorizationDecision {
            allowed: self.through(&request.person, action, &resource).is_some(),
            ..AuthorizationDecision::default()
        })
    }
}

/// One entry of a record as a person sees it.
#[derive(Debug, Serialize)]
struct EntryView {
    entry: String,
    propagate: bool,
}

/// A compartment as a person sees it: a cover or a read-in of what they may not see is neither
/// named nor counted.
#[derive(Debug, Serialize)]
pub(crate) struct CompartmentView {
    id: String,
    description: String,
    actions: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_clearance: Option<String>,
    limits: Value,
    weakenings: Vec<String>,
    covers: Vec<EntryView>,
    read_in: Vec<EntryView>,
    revision: String,
    writable: bool,
}

/// The entry body: `propagate` defaults to on.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntryBody {
    #[serde(default)]
    propagate: Option<bool>,
}

fn source(gateway: &Gateway) -> Result<Arc<dyn ConfigProvider>, ApiError> {
    gateway.roster.clone().ok_or_else(ApiError::unavailable)
}

fn scope(id: &str) -> String {
    format!("compartment:{id}")
}

/// What a person's request learns of a refused or failed write: the provider's own reason.
fn refused(error: &RosterError) -> Response {
    let status = match error.kind() {
        RosterErrorKind::Invalid => StatusCode::BAD_REQUEST,
        RosterErrorKind::ReadOnly | RosterErrorKind::Conflict => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        Json(serde_json::json!({"error": error.to_string()})),
    )
        .into_response()
}

fn bad_request(reason: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": reason})),
    )
        .into_response()
}

/// The Operation a Mission belongs to, from the Registry.
fn operation_of(registry: Option<&Arc<dyn RegistryBackend>>, mission: &str) -> Option<String> {
    registry?.filed_brief(mission).ok()??.brief.operation
}

/// Whether `person` may see `operation`'s work, which is what names it to them.
fn sees(
    access: &CompartmentAccess,
    person: &Person,
    operation: &str,
    tree: &OperationTree,
) -> bool {
    ["mission.status", "mission.read"]
        .into_iter()
        .any(|action| {
            access
                .authorize(operation_request(person.clone(), action, operation, tree))
                .is_ok_and(|decision| decision.allowed)
        })
}

fn view_of(
    compartment: &Compartment,
    definition: &Value,
    writable: bool,
    access: &CompartmentAccess,
    person: &Person,
    tree: &OperationTree,
    registry: Option<&Arc<dyn RegistryBackend>>,
) -> CompartmentView {
    let sees_operation = |operation: &str| sees(access, person, operation, tree);
    let sees_mission = |mission: &str| {
        operation_of(registry, mission).is_some_and(|operation| sees_operation(&operation))
    };
    CompartmentView {
        id: compartment.id.clone(),
        description: compartment.description.clone(),
        actions: compartment
            .actions
            .iter()
            .map(|action| action.as_str())
            .collect(),
        min_clearance: compartment.min_clearance.clone(),
        limits: definition.get("limits").cloned().unwrap_or(Value::Null),
        weakenings: compartment.weakenings(),
        covers: compartment
            .covers
            .iter()
            .filter(|cover| match &cover.target {
                CoverTarget::Operation(id) => sees_operation(id),
                CoverTarget::Mission(id) => sees_mission(id),
                _ => false,
            })
            .map(|cover| EntryView {
                entry: cover.target.to_string(),
                propagate: cover.propagate,
            })
            .collect(),
        read_in: compartment
            .read_in
            .iter()
            .filter(|read_in| match &read_in.subject {
                ReadInSubject::Operation(id) => sees_operation(id),
                ReadInSubject::Mission(id) => sees_mission(id),
                _ => true,
            })
            .map(|read_in| EntryView {
                entry: read_in.subject.to_string(),
                propagate: read_in.propagate,
            })
            .collect(),
        revision: token(Some(definition)),
        writable,
    }
}

/// `GET /v1/user/compartments`: the compartments the person may read.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<Vec<CompartmentView>>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let source = source(&gateway)?;
    let access = gateway.probing_authorizer()?;
    let tree = operation_tree(&gateway).await?;
    let registry = gateway.registry.clone();
    let views = tokio::task::spawn_blocking(move || {
        let held = source
            .definitions(ConfigRecord::Compartment, &ConfigScope::Global)
            .map_err(|_| ApiError::unavailable())?;
        let mut views = Vec::new();
        for (id, definition) in held.records {
            let readable = access
                .authorize(AuthorizationRequest {
                    person: person.clone(),
                    action: "compartment.read".to_owned(),
                    resource: scope(&id),
                    ancestors: Vec::new(),
                })
                .is_ok_and(|decision| decision.allowed);
            if !readable {
                continue;
            }
            if let Ok(compartment) = Compartment::from_definition(id, &definition) {
                views.push(view_of(
                    &compartment,
                    &definition,
                    held.writable,
                    &access,
                    &person,
                    &tree,
                    registry.as_ref(),
                ));
            }
        }
        Ok::<_, ApiError>(views)
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(views))
}

/// `GET /v1/user/compartments/{id}`: one, with `compartment.read` on it.
pub(crate) async fn show(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<CompartmentView>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(&gateway, person.clone(), scope(&id), "compartment.read").await?;
    let source = source(&gateway)?;
    let access = gateway.probing_authorizer()?;
    let tree = operation_tree(&gateway).await?;
    let registry = gateway.registry.clone();
    let view = tokio::task::spawn_blocking(move || {
        let held = source
            .definitions(ConfigRecord::Compartment, &ConfigScope::Global)
            .map_err(|_| ApiError::unavailable())?;
        let (_, definition) = held
            .records
            .into_iter()
            .find(|(key, _)| *key == id)
            .ok_or_else(ApiError::not_found)?;
        let compartment =
            Compartment::from_definition(id, &definition).map_err(|_| ApiError::internal())?;
        Ok::<_, ApiError>(view_of(
            &compartment,
            &definition,
            held.writable,
            &access,
            &person,
            &tree,
            registry.as_ref(),
        ))
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(view))
}

/// `GET /v1/user/compartment-form`: what the app draws a compartment's settings with, for
/// anyone signed in; it says nothing about any compartment.
pub(crate) async fn form(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<ai6::RecordForm>, ApiError> {
    resolve_user(&gateway, bearer(&headers)?).await?;
    match ConfigRecord::Compartment.form() {
        Ok(Some(form)) => Ok(Json(form)),
        _ => Err(ApiError::internal()),
    }
}

/// What a change does to the stored compartment; `Err` refuses with that reason.
type Change = Box<dyn FnOnce(Option<Compartment>) -> Result<Compartment, String> + Send>;

/// Reads a compartment, applies `change` and stores it only if nothing changed meanwhile, so an
/// entry the editor cannot see survives, then writes the audit record.
async fn change(
    gateway: &Gateway,
    person: &Person,
    id: String,
    audit_action: &'static str,
    audit_target: String,
    change: Change,
) -> Result<Response, ApiError> {
    let source = source(gateway)?;
    let identity = id.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let global = ConfigScope::Global;
        let stored = source
            .definitions(ConfigRecord::Compartment, &global)?
            .records
            .into_iter()
            .find(|(key, _)| *key == identity)
            .map(|(_, definition)| definition);
        let base = stored
            .as_ref()
            .map(|definition| Compartment::from_definition(identity.clone(), definition))
            .transpose()?;
        let next = match change(base) {
            Ok(next) => next,
            Err(reason) => return Ok(Err(reason)),
        };
        next.validate()?;
        let definition = next.to_definition();
        source.put_if(
            ConfigRecord::Compartment,
            &global,
            &identity,
            stored.as_ref(),
            &definition,
        )?;
        Ok::<_, RosterError>(Ok(next))
    })
    .await
    .map_err(|_| ApiError::internal())?;
    match outcome {
        Ok(Ok(next)) => {
            write_audit(gateway, person, &id, audit_action, &audit_target).await?;
            Ok(Json(serde_json::json!({
                "id": next.id,
                "weakenings": next.weakenings(),
            }))
            .into_response())
        }
        Ok(Err(reason)) => Ok(bad_request(&reason)),
        Err(error) => Ok(refused(&error)),
    }
}

async fn write_audit(
    gateway: &Gateway,
    person: &Person,
    id: &str,
    action: &str,
    target: &str,
) -> Result<(), ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let record = ai6::AuditRecord {
        id: format!("compartment-{}", Uuid::new_v4().simple()),
        actor: person.id.clone(),
        action: action.to_owned(),
        operation: String::new(),
        resource: Some(scope(id)),
        target: bounded(target, gateway.config.unex().text.roster_target_bytes),
        at: unix_timestamp()?,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())
}

/// `PUT /v1/user/compartments/{id}`: its description, actions, floor and limits, with
/// `compartment.manage`; never its covers or read-ins.
pub(crate) async fn put(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(&gateway, person.clone(), scope(&id), "compartment.manage").await?;
    let settings = match CompartmentSettings::from_json(&body) {
        Ok(settings) => settings,
        Err(reason) => return Ok(bad_request(&reason)),
    };
    let key = id.clone();
    change(
        &gateway,
        &person,
        id.clone(),
        "compartment.put",
        id,
        Box::new(move |base| Ok(settings.apply(&key, base))),
    )
    .await
}

/// `DELETE /v1/user/compartments/{id}`, with `compartment.manage`.
pub(crate) async fn remove(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(&gateway, person.clone(), scope(&id), "compartment.manage").await?;
    let source = source(&gateway)?;
    let key = id.clone();
    let removed = tokio::task::spawn_blocking(move || {
        source.remove(ConfigRecord::Compartment, &ConfigScope::Global, &key)
    })
    .await
    .map_err(|_| ApiError::internal())?;
    match removed {
        Ok(Some(_)) => {
            write_audit(&gateway, &person, &id, "compartment.delete", &id).await?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        Ok(None) => Err(ApiError::not_found()),
        Err(error) => Ok(refused(&error)),
    }
}

/// The Operation that consents to a cover of `target`; `None` for a Mission the Registry does
/// not hold, whose Operation nobody can speak for.
fn consenting_operation(gateway: &Gateway, target: &CoverTarget) -> Option<String> {
    match target {
        CoverTarget::Operation(id) => Some(id.clone()),
        CoverTarget::Mission(id) => operation_of(gateway.registry.as_ref(), id),
        _ => None,
    }
}

/// `PUT /v1/user/compartments/{id}/covers/{resource}`: needs `compartment.manage` on it and
/// `compartment.cover` on the Operation, which is that Operation's consent.
pub(crate) async fn put_cover(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((id, resource)): Path<(String, String)>,
    body: Option<Json<EntryBody>>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let Some(target) = CoverTarget::parse(&resource) else {
        return Ok(bad_request("a cover is operation:ID or mission:ID"));
    };
    let target = by_uid(&gateway, target).await?;
    authorize_resource_action(&gateway, person.clone(), scope(&id), "compartment.manage").await?;
    let tokio_gateway = gateway.clone();
    let lookup = target.clone();
    let operation =
        tokio::task::spawn_blocking(move || consenting_operation(&tokio_gateway, &lookup))
            .await
            .map_err(|_| ApiError::internal())?
            .ok_or_else(ApiError::forbidden)?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        crate::tree::operation_resource(&operation),
        "compartment.cover",
    )
    .await?;
    let propagate = body.and_then(|Json(body)| body.propagate).unwrap_or(true);
    change(
        &gateway,
        &person,
        id.clone(),
        "compartment.cover",
        format!("{id} {resource}"),
        Box::new(move |base| {
            let mut compartment = base.ok_or_else(|| "no such compartment".to_owned())?;
            compartment.covers.retain(|cover| cover.target != target);
            compartment
                .covers
                .push(ai6::CompartmentCover::new(target).with_propagate(propagate));
            Ok(compartment)
        }),
    )
    .await
}

/// A Mission cover as the Registry keeps Missions, by UID, though it was named by a handle.
async fn by_uid(gateway: &Gateway, target: CoverTarget) -> Result<CoverTarget, ApiError> {
    Ok(match target {
        CoverTarget::Mission(id) => CoverTarget::Mission(crate::mission_key(gateway, &id).await?),
        other => other,
    })
}

/// `DELETE .../covers/{resource}`: needs `compartment.manage` on it or `compartment.cover` on the
/// Operation, which may always withdraw.
pub(crate) async fn remove_cover(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((id, resource)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let Some(target) = CoverTarget::parse(&resource) else {
        return Ok(bad_request("a cover is operation:ID or mission:ID"));
    };
    let target = by_uid(&gateway, target).await?;
    if authorize_resource_action(&gateway, person.clone(), scope(&id), "compartment.manage")
        .await
        .is_err()
    {
        let tokio_gateway = gateway.clone();
        let lookup = target.clone();
        let operation =
            tokio::task::spawn_blocking(move || consenting_operation(&tokio_gateway, &lookup))
                .await
                .map_err(|_| ApiError::internal())?
                .ok_or_else(ApiError::forbidden)?;
        authorize_resource_action(
            &gateway,
            person.clone(),
            crate::tree::operation_resource(&operation),
            "compartment.cover",
        )
        .await?;
    }
    change(
        &gateway,
        &person,
        id.clone(),
        "compartment.uncover",
        format!("{id} {resource}"),
        Box::new(move |base| {
            let mut compartment = base.ok_or_else(|| "no such compartment".to_owned())?;
            compartment.covers.retain(|cover| cover.target != target);
            Ok(compartment)
        }),
    )
    .await
}

/// `PUT .../read-in/{subject}`: needs `compartment.manage`; a person, group or account also
/// `access.manage` on it, since that hands a person access.
pub(crate) async fn put_read_in(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((id, subject)): Path<(String, String)>,
    body: Option<Json<EntryBody>>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let Some(parsed) = ReadInSubject::parse(&subject) else {
        return Ok(bad_request(
            "a read-in is person:, group:, account:, operation:, mission: or agent:",
        ));
    };
    authorize_resource_action(&gateway, person.clone(), scope(&id), "compartment.manage").await?;
    if parsed.is_people() {
        authorize_resource_action(&gateway, person.clone(), scope(&id), "access.manage").await?;
    }
    let propagate = body.and_then(|Json(body)| body.propagate).unwrap_or(true);
    change(
        &gateway,
        &person,
        id.clone(),
        "compartment.read_in",
        format!("{id} {subject}"),
        Box::new(move |base| {
            let mut compartment = base.ok_or_else(|| "no such compartment".to_owned())?;
            compartment
                .read_in
                .retain(|read_in| read_in.subject != parsed);
            compartment
                .read_in
                .push(ai6::ReadIn::new(parsed).with_propagate(propagate));
            Ok(compartment)
        }),
    )
    .await
}

/// `DELETE .../read-in/{subject}`: reads a subject out, with `compartment.manage`.
pub(crate) async fn remove_read_in(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((id, subject)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let Some(parsed) = ReadInSubject::parse(&subject) else {
        return Ok(bad_request(
            "a read-in is person:, group:, account:, operation:, mission: or agent:",
        ));
    };
    authorize_resource_action(&gateway, person.clone(), scope(&id), "compartment.manage").await?;
    change(
        &gateway,
        &person,
        id.clone(),
        "compartment.read_out",
        format!("{id} {subject}"),
        Box::new(move |base| {
            let mut compartment = base.ok_or_else(|| "no such compartment".to_owned())?;
            compartment
                .read_in
                .retain(|read_in| read_in.subject != parsed);
            Ok(compartment)
        }),
    )
    .await
}

/// `GET /v1/missions/{id}/products`: what a Mission produced, for a person who may read it or
/// holds `product.read` through a compartment. Anything else answers as a Mission that does not
/// exist.
pub(crate) async fn mission_products(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(mission): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mission = crate::mission_key(&gateway, &mission).await?;
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let policy = gateway.access.clone().ok_or_else(ApiError::unavailable)?;
    let access = gateway.authorizer()?;
    let tree = operation_tree(&gateway).await?;
    let product = tokio::task::spawn_blocking(move || {
        let filed = registry.filed_brief(&mission).unwrap_or(None)?;
        let operation = filed.brief.operation.clone()?;
        let by_policy = ["product.read", "mission.read"].into_iter().any(|action| {
            policy
                .authorizer()
                .authorize(operation_request(person.clone(), action, &operation, &tree))
                .is_ok_and(|decision| decision.allowed)
        });
        let limit = if by_policy {
            ai6::Limit::UNBOUNDED
        } else {
            let resource = CompartmentResource::Mission {
                id: mission.clone(),
                operation: operation.clone(),
            };
            let through = access.through(&person, CompartmentAction::ProductRead, &resource)?;
            access
                .compiled()?
                .get(&through)
                .map(|compartment| compartment.limits.max_product_bytes)?
        };
        Some(ai6::directorates::registry::products::mission_product(
            registry.as_ref(),
            &filed,
            limit,
        ))
    })
    .await
    .map_err(|_| ApiError::internal())?;
    product.map(Json).ok_or_else(ApiError::not_found)
}

/// Whether `session` of `operation` is in compartment `id`, for the list filter.
pub(crate) fn in_compartment(
    compiled: &CompiledCompartments,
    id: &str,
    operation: &str,
    mission: &str,
) -> bool {
    let Some(compartment) = compiled.get(id) else {
        return false;
    };
    let lineage = compiled.lineage();
    compartment.covers.iter().any(|cover| match &cover.target {
        CoverTarget::Mission(covered) => covered == mission,
        CoverTarget::Operation(covered) if cover.propagate => {
            lineage.in_subtree(operation, covered)
        }
        CoverTarget::Operation(covered) => covered == operation,
        _ => false,
    })
}

#[cfg(all(test, feature = "cli"))]
#[path = "compartments_tests.rs"]
mod tests;
