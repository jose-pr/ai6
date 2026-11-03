//! Memories as people reach them, on every scope: `/v1/user/memories`.
//!
//! `GET /v1/user/memories[?scope=SCOPE][&q=TEXT]` lists one scope, or searches what the person may
//! see of it or of everything; `POST` adds an entry; `GET`, `PUT` and `DELETE
//! /v1/user/memories/{id}` read, change and forget one, and `GET .../{id}/versions` reads what it
//! held before. `GET /v1/user/memories/proposals[?scope=SCOPE]` lists what Agents proposed and
//! `POST .../proposals/{id}` approves or rejects one.
//!
//! Who may do what is `ai6::directorates::m::memories`, asked at every call. This module says what
//! a person holds: their own memory; an Operation's with `mission.read` there, or through a
//! compartment granting `memory.read` (a crossing, written to the audit log); the global one always;
//! `memory.write` on an Operation or on `config:memory` to change those; and a chief sees every
//! entry whatever its compartments. An entry they may not see answers the same `404` as an ID that
//! does not exist.

use std::sync::{Arc, Mutex, PoisonError};

use ai6::directorates::m::memories::{
    self, AUDIT_READ, GLOBAL_RESOURCE, MemoryCaller, MemoryChange, MemoryOutcome, MemoryRefusal,
    MemoryTarget, MemoryWriteRight, PERSON_RESOURCE, entry_json, header_json, proposal_json,
};
use ai6::directorates::registry::backend::*;
use ai6::{AuthorizationRequest, CompartmentAction, CompartmentResource, MemoryScope, Person};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::compartments::CompartmentAccess;
use crate::tree::{OperationTree, operation_request};
use crate::{ApiError, Gateway, authorize_resource_action, bearer, operation_tree, resolve_user};

/// The action that reads another person's memory, on `config:assistant`.
pub(crate) const MEMORY_ADMIN: &str = "assistant.memory.admin";
/// The action that changes an Operation's memory, or on `config:memory` the global one.
const MEMORY_WRITE: &str = "memory.write";
/// What reads an Operation's memory, for a person.
const OPERATION_READ: &str = "mission.read";

/// A refusal as the app shows it: `{"error": TEXT}` with its status.
#[derive(Debug)]
pub(crate) struct Refused {
    pub(crate) status: StatusCode,
    pub(crate) body: Value,
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<ApiError> for Refused {
    fn from(error: ApiError) -> Self {
        refused(error.status, error.message)
    }
}

impl From<MemoryRefusal> for Refused {
    fn from(refusal: MemoryRefusal) -> Self {
        let status = match &refusal {
            MemoryRefusal::Missing => StatusCode::NOT_FOUND,
            MemoryRefusal::Forbidden(_) => StatusCode::FORBIDDEN,
            MemoryRefusal::Invalid(_) => StatusCode::BAD_REQUEST,
            MemoryRefusal::Full(_) | MemoryRefusal::Changed { .. } => StatusCode::CONFLICT,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        let mut body = json!({"error": refusal.to_string()});
        if let MemoryRefusal::Changed { revision } = refusal {
            body["revision"] = Value::from(revision);
        }
        Self { status, body }
    }
}

pub(crate) fn refused(status: StatusCode, error: impl Into<String>) -> Refused {
    Refused {
        status,
        body: json!({"error": error.into()}),
    }
}

/// A person, or Moneypenny for them, asking of the memory: what they hold is asked as the call
/// needs it, of what is granted for this request.
pub(crate) struct PersonCaller {
    person: Person,
    /// The other person whose memory this request asked to read, allowed and audited.
    reads_person: Option<String>,
    /// The conversation, when she is the one asking.
    conversation: Option<String>,
    registry: Arc<dyn RegistryBackend>,
    access: Arc<CompartmentAccess>,
    tree: OperationTree,
    chief: bool,
    audit_target_bytes: ai6::Limit,
    /// How her conversation was marked before this call.
    marks: Vec<String>,
    /// The marked entries this call showed her.
    seen: Mutex<Vec<String>>,
}

impl PersonCaller {
    /// How many marks a conversation keeps: `[registry.limits.sessions] exposures`.
    pub(crate) fn marks_limit(&self) -> ai6::Limit {
        self.registry.limits().sessions.exposures
    }

    /// The entries this call showed her that were written by an exposed Session.
    pub(crate) fn seen_marks(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether the access policy, without compartments, gives them `action` on `operation`.
    fn holds_on_operation(&self, action: &str, operation: &str) -> bool {
        self.access
            .policy_allows(operation_request(
                self.person.clone(),
                action,
                operation,
                &self.tree,
            ))
            .is_ok_and(|decision| decision.allowed)
    }

    fn holds_memory_write(&self, scope: &MemoryScope) -> bool {
        match scope {
            MemoryScope::Operation(operation) => self.holds_on_operation(MEMORY_WRITE, operation),
            MemoryScope::Global => self
                .access
                .policy_allows(AuthorizationRequest {
                    person: self.person.clone(),
                    action: MEMORY_WRITE.to_owned(),
                    resource: GLOBAL_RESOURCE.to_owned(),
                    ancestors: Vec::new(),
                })
                .is_ok_and(|decision| decision.allowed),
            _ => false,
        }
    }
}

impl MemoryCaller for PersonCaller {
    fn by(&self) -> String {
        match self.conversation {
            Some(_) => format!("moneypenny:{}", self.person.id),
            None => format!("person:{}", self.person.id),
        }
    }

    fn session(&self) -> Option<String> {
        self.conversation.clone()
    }

    fn reads_scope(&self, scope: &MemoryScope) -> bool {
        match scope {
            MemoryScope::Person(id) => {
                *id == self.person.id || self.reads_person.as_deref() == Some(id)
            }
            MemoryScope::Operation(operation) => {
                self.holds_on_operation(OPERATION_READ, operation)
                    || self
                        .access
                        .through(
                            &self.person,
                            CompartmentAction::MemoryRead,
                            &CompartmentResource::Operation(operation.clone()),
                        )
                        .is_some()
            }
            MemoryScope::Global => true,
            _ => false,
        }
    }

    fn writes_scope(&self, scope: &MemoryScope) -> MemoryWriteRight {
        match scope {
            MemoryScope::Person(id) if *id == self.person.id => MemoryWriteRight::Write,
            MemoryScope::Person(_) => MemoryWriteRight::Refused,
            other if !self.holds_memory_write(other) => MemoryWriteRight::Refused,
            // What she keeps for one person alone is hers to keep; what others will read waits.
            _ if self.conversation.is_some() => MemoryWriteRight::Propose,
            _ => MemoryWriteRight::Write,
        }
    }

    fn read_in(&self, compartment: &str) -> bool {
        self.access
            .compiled()
            .is_some_and(|compiled| compiled.person_read_in(compartment, &self.person))
    }

    fn sees_all_compartments(&self) -> bool {
        self.chief
    }

    fn acts_in_person(&self) -> bool {
        self.conversation.is_none()
    }

    fn exposures(&self) -> Vec<String> {
        let mut marks = self.marks.clone();
        marks.extend(self.seen_marks());
        marks
    }

    fn saw_marked(&self, source: &str) {
        if self.conversation.is_some() {
            self.seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(source.to_owned());
        }
    }

    fn mark_bytes(&self) -> ai6::Limit {
        self.registry.limits().sessions.exposure_source_bytes
    }

    fn now(&self) -> f64 {
        crate::unix_timestamp().unwrap_or(0.0)
    }

    fn audit(&self, action: &str, scope: &MemoryScope, target: &str) -> Result<(), String> {
        let (operation, resource, target) = match scope {
            MemoryScope::Operation(operation) => (operation.clone(), None, target.to_owned()),
            MemoryScope::Global => (
                String::new(),
                Some(GLOBAL_RESOURCE.to_owned()),
                target.to_owned(),
            ),
            MemoryScope::Person(id) => (
                String::new(),
                Some(PERSON_RESOURCE.to_owned()),
                format!("{id} {target}"),
            ),
            _ => (String::new(), None, target.to_owned()),
        };
        self.registry
            .record_audit(&ai6::AuditRecord {
                id: Uuid::new_v4().to_string(),
                actor: self.person.id.clone(),
                action: action.to_owned(),
                operation,
                resource,
                target: ai6::directorates::m::memories::bounded(&target, self.audit_target_bytes),
                at: self.now(),
            })
            .map_err(|_| "It could not be audited, so it was not done.".to_owned())
    }
}

/// What a request asks of a caller.
#[derive(Default)]
pub(crate) struct Asking {
    /// Another person's memory, by this request's own `?person`, to read.
    pub(crate) whose: Option<String>,
    /// It changes something.
    pub(crate) change: bool,
    /// She asks, in this conversation, with these marks on it.
    pub(crate) her: Option<(String, Vec<String>)>,
    /// What the audit of another person's memory names.
    pub(crate) what: String,
}

/// Settles who is asking from a signed-in person. Reading another person's memory needs
/// [`MEMORY_ADMIN`], and is audited here before anything is read.
pub(crate) async fn caller_of(
    gateway: &Gateway,
    person: Person,
    asking: Asking,
) -> Result<PersonCaller, Refused> {
    let registry = gateway.registry.clone().ok_or_else(|| {
        refused(
            StatusCode::SERVICE_UNAVAILABLE,
            "This gateway keeps no Registry, so nothing is remembered here.",
        )
    })?;
    let reads_person = asking.whose.filter(|owner| *owner != person.id);
    if let Some(owner) = &reads_person {
        if asking.change {
            return Err(refused(
                StatusCode::FORBIDDEN,
                "Only its person changes their memory.",
            ));
        }
        if !ai6::directorates::section_00::assistant::valid_identity(owner) {
            return Err(refused(StatusCode::BAD_REQUEST, "That is not a person."));
        }
        authorize_resource_action(
            gateway,
            person.clone(),
            PERSON_RESOURCE.to_owned(),
            MEMORY_ADMIN,
        )
        .await
        .map_err(|_| {
            refused(
                StatusCode::FORBIDDEN,
                format!(
                    "Reading another person's memory needs {MEMORY_ADMIN} on {PERSON_RESOURCE}."
                ),
            )
        })?;
        let record = ai6::AuditRecord {
            id: Uuid::new_v4().to_string(),
            actor: person.id.clone(),
            action: AUDIT_READ.to_owned(),
            operation: String::new(),
            resource: Some(PERSON_RESOURCE.to_owned()),
            target: format!("{owner} {}", asking.what),
            at: crate::unix_timestamp().unwrap_or(0.0),
        };
        let log = registry.clone();
        tokio::task::spawn_blocking(move || log.record_audit(&record))
            .await
            .map_err(|_| Refused::from(ApiError::internal()))?
            .map_err(|_| {
                refused(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Nothing was done: it could not be audited.",
                )
            })?;
    }
    let chief = gateway
        .access_admin
        .as_ref()
        .is_some_and(|live| live.effective().is_chief(&person));
    let tree = operation_tree(gateway).await?;
    let (conversation, marks) = match asking.her {
        Some((conversation, marks)) => (Some(conversation), marks),
        None => (None, Vec::new()),
    };
    Ok(PersonCaller {
        person,
        reads_person,
        conversation,
        access: gateway.authorizer()?,
        registry,
        tree,
        chief,
        audit_target_bytes: crate::moneypenny::gateway_limits(gateway).audit_target_bytes,
        marks,
        seen: Mutex::default(),
    })
}

/// Runs `work` against the Registry on the blocking pool, and hands the caller back.
pub(crate) async fn on_registry<T: Send + 'static>(
    caller: PersonCaller,
    work: impl FnOnce(&PersonCaller, &dyn RegistryBackend) -> Result<T, MemoryRefusal> + Send + 'static,
) -> Result<(PersonCaller, T), Refused> {
    tokio::task::spawn_blocking(move || {
        let registry = caller.registry.clone();
        let done = work(&caller, registry.as_ref());
        done.map(|done| (caller, done))
    })
    .await
    .map_err(|_| Refused::from(ApiError::internal()))?
    .map_err(Refused::from)
}

/// Every scope a person might read, by what the Stations publish: their own, the global one and
/// each Operation.
pub(crate) async fn every_scope(gateway: &Gateway, person: &Person) -> Vec<MemoryScope> {
    let mut scopes = vec![MemoryScope::Person(person.id.clone()), MemoryScope::Global];
    scopes.extend(
        crate::known_operations(gateway)
            .await
            .into_iter()
            .map(MemoryScope::Operation),
    );
    scopes
}

/// `?scope=SCOPE&q=TEXT`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Listing {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    q: Option<String>,
}

/// `?revision=N`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AtRevision {
    #[serde(default)]
    revision: Option<u64>,
}

/// The body of a `POST`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Added {
    scope: String,
    #[serde(default)]
    name: Option<String>,
    summary: String,
    content: String,
    #[serde(default)]
    compartments: Option<Vec<String>>,
}

/// The body of a `PUT`: the new text, and the revision it was made from.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Changed {
    #[serde(default)]
    name: Option<String>,
    summary: String,
    content: String,
    revision: u64,
    #[serde(default)]
    compartments: Option<Vec<String>>,
}

/// The body of a decision on a proposal.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Decision {
    approve: bool,
}

fn scope_of(text: &str) -> Result<MemoryScope, Refused> {
    MemoryScope::parse(text).ok_or_else(|| {
        refused(
            StatusCode::BAD_REQUEST,
            format!("{text:?} is not a memory scope: person:ID, operation:ID or global."),
        )
    })
}

/// What a write answers: the entry, or the proposal it became.
fn outcome_json(outcome: &MemoryOutcome) -> (StatusCode, Value) {
    match outcome {
        MemoryOutcome::Kept(written) => (StatusCode::OK, header_json(&written.entry.header)),
        MemoryOutcome::Proposed(proposal) => (StatusCode::ACCEPTED, proposal_json(proposal)),
    }
}

async fn signed_in(gateway: &Gateway, headers: &HeaderMap) -> Result<Person, Refused> {
    Ok(resolve_user(gateway, bearer(headers)?).await?)
}

/// `GET /v1/user/memories[?scope=SCOPE][&q=TEXT]`: `{"entries", "total"}`, with no content. One
/// scope lists newest change first; with `q`, or across every scope, it is a search, best first,
/// at most `[registry.limits.memories] search_results`.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(listing): Query<Listing>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let caller = caller_of(&gateway, person.clone(), Asking::default()).await?;
    let (entries, total) = match (listing.scope, listing.q) {
        (Some(scope), None) => {
            let scope = scope_of(&scope)?;
            let (_, listed) = on_registry(caller, move |caller, registry| {
                memories::list_entries(registry, caller, &scope)
            })
            .await?;
            let total = listed.len();
            (listed, total)
        }
        (scope, query) => {
            let scopes = match scope {
                Some(scope) => vec![scope_of(&scope)?],
                None => every_scope(&gateway, &person).await,
            };
            let query = query.unwrap_or_default();
            let (_, found) = on_registry(caller, move |caller, registry| {
                let limits = registry.limits().memories.clone();
                memories::search_entries(registry, caller, &scopes, &query, &limits)
            })
            .await?;
            (found.hits, found.total)
        }
    };
    Ok(Json(json!({
        "entries": entries.iter().map(header_json).collect::<Vec<Value>>(),
        "total": total,
    })))
}

/// `POST /v1/user/memories` with `{"scope", "name"?, "summary", "content", "compartments"?}`: adds
/// an entry; `201` with its header. Needs the scope's write, and to be read into a compartment it
/// names.
pub(crate) async fn add(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(added): Json<Added>,
) -> Result<(StatusCode, Json<Value>), Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let scope = scope_of(&added.scope)?;
    let caller = caller_of(
        &gateway,
        person,
        Asking {
            change: true,
            ..Asking::default()
        },
    )
    .await?;
    let change = MemoryChange::new(
        MemoryTarget::New(scope),
        added.name,
        added.summary,
        added.content,
    )
    .with_compartments(added.compartments);
    let (_, outcome) = on_registry(caller, move |caller, registry| {
        memories::write_entry(registry, caller, change)
    })
    .await?;
    let (status, body) = outcome_json(&outcome);
    let status = if status == StatusCode::OK {
        StatusCode::CREATED
    } else {
        status
    };
    Ok((status, Json(body)))
}

/// `GET /v1/user/memories/{id}`: the entry and its content; `404` for one the person may not see,
/// the same as for an ID that does not exist.
pub(crate) async fn read(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let caller = caller_of(&gateway, person, Asking::default()).await?;
    let (_, entry) = on_registry(caller, move |caller, registry| {
        memories::read_entry(registry, caller, &id)
    })
    .await?;
    Ok(Json(entry_json(&entry)))
}

/// `GET /v1/user/memories/{id}/versions`: `{"versions"}`, what it held before, newest first.
pub(crate) async fn versions(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let caller = caller_of(&gateway, person, Asking::default()).await?;
    let (_, versions) = on_registry(caller, move |caller, registry| {
        memories::prior_versions(registry, caller, &id)
    })
    .await?;
    Ok(Json(json!({
        "versions": versions.iter().map(entry_json).collect::<Vec<Value>>(),
    })))
}

/// `PUT /v1/user/memories/{id}` with `{"name"?, "summary", "content", "revision",
/// "compartments"?}`: changes it, and clears any mark on it, audited; the entry as stored. `409`
/// with the `revision` it is at when it has moved on.
pub(crate) async fn change(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(changed): Json<Changed>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let caller = caller_of(
        &gateway,
        person,
        Asking {
            change: true,
            ..Asking::default()
        },
    )
    .await?;
    let change = MemoryChange::new(
        MemoryTarget::Existing {
            id,
            revision: changed.revision,
        },
        changed.name,
        changed.summary,
        changed.content,
    )
    .with_compartments(changed.compartments);
    let (_, outcome) = on_registry(caller, move |caller, registry| {
        memories::write_entry(registry, caller, change)
    })
    .await?;
    Ok(Json(outcome_json(&outcome).1))
}

/// `DELETE /v1/user/memories/{id}[?revision=N]`: forgets it with its prior versions; `{"id"}`.
pub(crate) async fn forget(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(at): Query<AtRevision>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let caller = caller_of(
        &gateway,
        person,
        Asking {
            change: true,
            ..Asking::default()
        },
    )
    .await?;
    let (_, forgotten) = on_registry(caller, move |caller, registry| {
        memories::forget_entry(registry, caller, &id, at.revision)
    })
    .await?;
    Ok(Json(match forgotten {
        memories::MemoryForgotten::Forgotten(entry) => json!({"id": entry.header.id}),
        memories::MemoryForgotten::Proposed(proposal) => proposal_json(&proposal),
    }))
}

/// `GET /v1/user/memories/proposals[?scope=SCOPE]`: `{"proposals"}`, what waits for a person who
/// may change the scope and may see the entry, oldest first.
pub(crate) async fn proposals(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(listing): Query<Listing>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let scopes = match listing.scope {
        Some(scope) => vec![scope_of(&scope)?],
        None => every_scope(&gateway, &person).await,
    };
    let caller = caller_of(&gateway, person, Asking::default()).await?;
    let (_, waiting) = on_registry(caller, move |caller, registry| {
        let mut waiting = Vec::new();
        for scope in &scopes {
            waiting.extend(memories::list_proposals(registry, caller, scope)?);
        }
        Ok(waiting)
    })
    .await?;
    Ok(Json(json!({
        "proposals": waiting.iter().map(proposal_json).collect::<Vec<Value>>(),
    })))
}

/// `POST /v1/user/memories/proposals/{id}` with `{"approve"}`: carries it out or drops it, audited.
/// `{"approved"}`, and the entry's header when one was written; `409` when the entry has moved on.
pub(crate) async fn decide(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(decision): Json<Decision>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let caller = caller_of(
        &gateway,
        person,
        Asking {
            change: true,
            ..Asking::default()
        },
    )
    .await?;
    let approve = decision.approve;
    let (_, written) = on_registry(caller, move |caller, registry| {
        memories::decide_proposal(registry, caller, &id, approve)
    })
    .await?;
    let mut answer = json!({"approved": approve});
    if let Some(entry) = written {
        answer["entry"] = header_json(&entry.header);
    }
    Ok(Json(answer))
}

#[cfg(test)]
#[path = "memories_tests.rs"]
mod tests;
