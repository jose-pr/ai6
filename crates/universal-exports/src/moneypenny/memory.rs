//! The person's memory, rows in the Registry that need no repository and no Station.
//!
//! `GET /v1/user/assistant/memory[?q=TEXT]` lists or searches what she keeps of the signed-in
//! person, `POST` adds an entry, and `GET`, `PUT` and `DELETE
//! /v1/user/assistant/memory/{id}` read, change and forget one; `GET .../{id}/versions` reads what
//! it held before. An entry that is not there, and another person's, answer the same `404`.
//!
//! `?person=<id>` reads another person's with [`MEMORY_ADMIN`] on `config:assistant`, audited
//! `memory.read`; a change to another person's is refused even then. A change is audited
//! (`memory.write`, `memory.forget`) before it is made.
//!
//! Her tools, `memory_search`, `memory_read`, `memory_write` and `memory_forget`, answer for the
//! same rows, as the person she is talking to; what she changes shows in the conversation.
//!
//! Errors are `{"error": "<what happened>"}`, worded for the person.

use std::sync::Arc;

use ai6::directorates::m::memories::{
    self, AUDIT_READ, GLOBAL_RESOURCE, MemoryCaller, MemoryChange, MemoryRefusal, MemoryTarget,
    MemoryWriteRight, MemoryWritten, PERSON_RESOURCE, entry_json, header_json,
};
use ai6::directorates::registry::backend::*;
use ai6::{MemoryHeader, MemoryScope, Person};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::actions::hidden_characters;
use super::store::{Entry, Role};
use super::tools::{self, Outcome, check, object, text};
use super::{Assistant, Granted, audit, cut, now};
use crate::{ApiError, Gateway, authorize_resource_action, bearer, resolve_user};

/// The action that reads another person's memory, on `config:assistant`.
pub(crate) const MEMORY_ADMIN: &str = "assistant.memory.admin";

pub(crate) const SEARCH: &str = "memory_search";
pub(crate) const READ: &str = "memory_read";
pub(crate) const WRITE: &str = "memory_write";
pub(crate) const FORGET: &str = "memory_forget";

pub(crate) const SEARCH_DESCRIPTION: &str = "Search your memory of the person by words in an \
     entry's name, summary or content, best first. Answers IDs, names and summaries; read an \
     entry with memory_read. No query lists the newest.";
pub(crate) const READ_DESCRIPTION: &str = "Read one entry of your memory, by the ID memory_search \
     gave: its content and the revision a change names. An ID that is not visible answers as one \
     that does not exist.";
pub(crate) const WRITE_DESCRIPTION: &str = "Keep something about the person in your memory, at \
     once and shown to them: a new entry with no id, or a change to the entry id, naming the \
     revision you read, which is refused when it has moved on. summary is one line saying what \
     the entry is for; names and summaries may repeat. Keep who they are, their preferences, \
     standing instructions and ongoing work; never secrets.";
pub(crate) const FORGET_DESCRIPTION: &str = "Forget one entry of your memory, with its prior \
     versions, for good. Say so to the person.";

/// `?person=<id>`: whose memory, when it is not the signed-in person's, and what to search for.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Whose {
    #[serde(default)]
    person: Option<String>,
    #[serde(default)]
    q: Option<String>,
}

/// `?revision=<n>` of a forget: the revision it was read at.
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
    #[serde(default)]
    name: Option<String>,
    summary: String,
    content: String,
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
}

/// A refusal as the app shows it: `{"error": TEXT}` with its status.
#[derive(Debug)]
pub(crate) struct Refused {
    status: StatusCode,
    body: Value,
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

fn refused(status: StatusCode, error: impl Into<String>) -> Refused {
    Refused {
        status,
        body: json!({"error": error.into()}),
    }
}

/// A person, or Moneypenny for them, asking of the memory. What they hold is settled before the
/// call runs on the blocking pool.
pub(crate) struct PersonCaller {
    person: Person,
    /// The other person whose memory they were allowed to read, by this request's own `?person`.
    reads_person: Option<String>,
    /// The conversation, when she is the one asking.
    conversation: Option<String>,
    registry: Arc<dyn RegistryBackend>,
    audit_target_bytes: ai6::Limit,
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
            _ => false,
        }
    }

    fn writes_scope(&self, scope: &MemoryScope) -> MemoryWriteRight {
        match scope {
            MemoryScope::Person(id) if *id == self.person.id => MemoryWriteRight::Write,
            _ => MemoryWriteRight::Refused,
        }
    }

    fn read_in(&self, _compartment: &str) -> bool {
        false
    }

    fn sees_all_compartments(&self) -> bool {
        false
    }

    fn now(&self) -> f64 {
        now()
    }

    fn audit(&self, action: &str, scope: &MemoryScope, target: &str) -> Result<(), String> {
        let resource = match scope {
            MemoryScope::Global => GLOBAL_RESOURCE,
            _ => PERSON_RESOURCE,
        };
        let target = match scope {
            MemoryScope::Person(id) => format!("{id} {target}"),
            _ => target.to_owned(),
        };
        self.registry
            .record_audit(&ai6::AuditRecord {
                id: Uuid::new_v4().to_string(),
                actor: self.person.id.clone(),
                action: action.to_owned(),
                operation: String::new(),
                resource: Some(resource.to_owned()),
                target: cut(&target, self.audit_target_bytes.or_max()),
                at: now(),
            })
            .map_err(|_| "It could not be audited, so it was not done.".to_owned())
    }
}

/// Runs `work` against the Registry on the blocking pool.
async fn on_registry<T: Send + 'static>(
    caller: PersonCaller,
    work: impl FnOnce(&PersonCaller, &dyn RegistryBackend) -> Result<T, MemoryRefusal> + Send + 'static,
) -> Result<T, Refused> {
    tokio::task::spawn_blocking(move || {
        let registry = caller.registry.clone();
        work(&caller, registry.as_ref())
    })
    .await
    .map_err(|_| Refused::from(ApiError::internal()))?
    .map_err(Refused::from)
}

/// Settles who is asking from a signed-in person: their own memory, or reading another's
/// (`whose`) with [`MEMORY_ADMIN`], which is audited here.
async fn caller_of(
    gateway: &Gateway,
    person: Person,
    whose: Option<String>,
    change: bool,
    conversation: Option<String>,
    what: &str,
) -> Result<PersonCaller, Refused> {
    let registry = gateway.registry.clone().ok_or_else(|| {
        refused(
            StatusCode::SERVICE_UNAVAILABLE,
            "This gateway keeps no Registry, so Moneypenny keeps no memory here.",
        )
    })?;
    let reads_person = whose.filter(|owner| *owner != person.id);
    if let Some(owner) = &reads_person {
        if change {
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
        audit(
            gateway,
            &person,
            AUDIT_READ,
            String::new(),
            Some(PERSON_RESOURCE.to_owned()),
            &format!("{owner} {what}"),
        )
        .await
        .map_err(|reason| {
            refused(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("Nothing was done: {reason}."),
            )
        })?;
    }
    Ok(PersonCaller {
        person,
        reads_person,
        conversation,
        registry,
        audit_target_bytes: super::gateway_limits(gateway).audit_target_bytes,
    })
}

/// The signed-in person of `headers`, on a gateway that serves her.
async fn signed_in(gateway: &Gateway, headers: &HeaderMap) -> Result<Person, Refused> {
    if gateway.assistant.is_none() {
        return Err(refused(
            StatusCode::SERVICE_UNAVAILABLE,
            "Universal Exports does not serve Moneypenny here.",
        ));
    }
    Ok(resolve_user(gateway, bearer(headers)?).await?)
}

/// `GET /v1/user/assistant/memory[?person=ID][&q=TEXT]`: `{"person", "entries", "total"}`, newest
/// change first, or best first with `q`, which answers at most
/// `[registry.limits.memories] search_results`.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(whose): Query<Whose>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let owner = whose.person.clone().unwrap_or_else(|| person.id.clone());
    let caller = caller_of(&gateway, person, whose.person, false, None, "memory").await?;
    let scope = MemoryScope::Person(owner.clone());
    let query = whose.q;
    let (entries, total) = on_registry(caller, move |caller, registry| match query {
        Some(query) => {
            let limits = registry.limits().memories.clone();
            let found = memories::search_entries(registry, caller, &[scope], &query, &limits)?;
            Ok((found.hits, found.total))
        }
        None => {
            let listed = memories::list_entries(registry, caller, &scope)?;
            let total = listed.len();
            Ok((listed, total))
        }
    })
    .await?;
    Ok(Json(json!({
        "person": owner,
        "entries": entries.iter().map(header_json).collect::<Vec<Value>>(),
        "total": total,
    })))
}

/// `GET /v1/user/assistant/memory/{id}[?person=ID]`: the entry and its content; `404` for an ID
/// that is not one of this person's.
pub(crate) async fn read(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(whose): Query<Whose>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let caller = caller_of(&gateway, person, whose.person, false, None, &id).await?;
    let entry = on_registry(caller, move |caller, registry| {
        memories::read_entry(registry, caller, &id)
    })
    .await?;
    Ok(Json(entry_json(&entry)))
}

/// `GET /v1/user/assistant/memory/{id}/versions[?person=ID]`: `{"versions"}`, what it held before,
/// newest first; `404` as a read.
pub(crate) async fn versions(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(whose): Query<Whose>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let caller = caller_of(&gateway, person, whose.person, false, None, &id).await?;
    let versions = on_registry(caller, move |caller, registry| {
        memories::prior_versions(registry, caller, &id)
    })
    .await?;
    Ok(Json(json!({
        "versions": versions.iter().map(entry_json).collect::<Vec<Value>>(),
    })))
}

/// `POST /v1/user/assistant/memory` with `{"name"?, "summary", "content"}`: adds an entry to the
/// signed-in person's memory; `201` with it.
pub(crate) async fn add(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(added): Json<Added>,
) -> Result<(StatusCode, Json<Value>), Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let scope = MemoryScope::Person(person.id.clone());
    let caller = caller_of(&gateway, person, None, true, None, "").await?;
    let change = MemoryChange::new(
        MemoryTarget::New(scope),
        added.name,
        added.summary,
        added.content,
    );
    let written = on_registry(caller, move |caller, registry| {
        memories::write_entry(registry, caller, change)
    })
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(header_json(&written.entry.header)),
    ))
}

/// `PUT /v1/user/assistant/memory/{id}` with `{"name"?, "summary", "content", "revision"}`: changes
/// the signed-in person's entry; the entry as stored. `409` with the `revision` it is at when it
/// has moved on from `revision`.
pub(crate) async fn change(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(whose): Query<Whose>,
    Json(changed): Json<Changed>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let caller = caller_of(&gateway, person, whose.person, true, None, &id).await?;
    let change = MemoryChange::new(
        MemoryTarget::Existing {
            id,
            revision: changed.revision,
        },
        changed.name,
        changed.summary,
        changed.content,
    );
    let written = on_registry(caller, move |caller, registry| {
        memories::write_entry(registry, caller, change)
    })
    .await?;
    Ok(Json(header_json(&written.entry.header)))
}

/// `DELETE /v1/user/assistant/memory/{id}[?revision=N]`: forgets the signed-in person's entry and
/// its prior versions; `{"id"}`. `409` when it has moved on from `revision`, when given.
pub(crate) async fn forget(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(at): Query<AtRevision>,
) -> Result<Json<Value>, Refused> {
    let person = signed_in(&gateway, &headers).await?;
    let caller = caller_of(&gateway, person, None, true, None, &id).await?;
    let forgotten = on_registry(caller, move |caller, registry| {
        memories::forget_entry(registry, caller, &id, at.revision)
    })
    .await?;
    Ok(Json(json!({"id": forgotten.header.id})))
}

/// The input of [`SEARCH`].
pub(crate) fn search_schema() -> Value {
    object(
        &[(
            "query",
            text("Words to look for; leave it out for the newest entries."),
        )],
        &[],
    )
}

/// The input of [`READ`].
pub(crate) fn read_schema() -> Value {
    object(&[("id", text("The entry's ID."))], &["id"])
}

/// The input of [`WRITE`].
pub(crate) fn write_schema() -> Value {
    object(
        &[
            (
                "id",
                text("The entry to change; leave it out for a new entry."),
            ),
            (
                "revision",
                json!({"type": "integer", "description": "The revision you read; required with id."}),
            ),
            ("name", text("A short name; it may repeat.")),
            ("summary", text("One line saying what the entry is for.")),
            ("content", text("The entry's whole new text.")),
        ],
        &["summary", "content"],
    )
}

/// The input of [`FORGET`].
pub(crate) fn forget_schema() -> Value {
    object(
        &[
            ("id", text("The entry's ID.")),
            (
                "revision",
                json!({"type": "integer", "description": "The revision you read, if you want it refused once it has moved on."}),
            ),
        ],
        &["id"],
    )
}

impl Assistant {
    /// Runs one of her memory tools as the person of `granted`.
    pub(crate) async fn memory_tool(
        &self,
        gateway: &Gateway,
        granted: &Granted,
        name: &str,
        input: &Value,
    ) -> Outcome {
        let schema = match name {
            SEARCH => search_schema(),
            READ => read_schema(),
            WRITE => write_schema(),
            _ => forget_schema(),
        };
        let invalid = |problem: String| Outcome::error(format!("INVALID_INPUT: {problem}"));
        if let Err(problem) = check(&schema, input, "input") {
            return invalid(problem);
        }
        if matches!(name, WRITE)
            && let Err(problem) = hidden_characters(input, "input")
        {
            return invalid(problem);
        }
        let Ok(person) = resolve_user(gateway, &granted.bearer).await else {
            return Outcome::error("The person's sign-in has ended; nothing was done.");
        };
        let chars = super::gateway_limits(gateway).result_chars;
        let caller = match caller_of(
            gateway,
            person.clone(),
            None,
            false,
            Some(granted.conversation_id.clone()),
            "",
        )
        .await
        {
            Ok(caller) => caller,
            Err(refused) => {
                return Outcome::error(refused.body["error"].as_str().unwrap_or_default());
            }
        };
        let field = |key: &str| input[key].as_str().map(str::to_owned);
        let scope = MemoryScope::Person(person.id.clone());
        let revision = input["revision"].as_u64();
        match name {
            SEARCH => {
                let query = field("query").unwrap_or_default();
                let found = on_registry(caller, move |caller, registry| {
                    let limits = registry.limits().memories.clone();
                    memories::search_entries(registry, caller, &[scope], &query, &limits)
                })
                .await;
                match found {
                    Ok(found) => Outcome {
                        text: tools::wrap(
                            name,
                            &json!({
                                "entries": found.hits.iter().map(header_json).collect::<Vec<Value>>(),
                                "total": found.total,
                            }),
                            chars,
                        ),
                        is_error: false,
                    },
                    Err(refused) => refused_outcome(&refused),
                }
            }
            READ => {
                let id = field("id").unwrap_or_default();
                match on_registry(caller, move |caller, registry| {
                    memories::read_entry(registry, caller, &id)
                })
                .await
                {
                    Ok(entry) => Outcome {
                        text: tools::wrap(name, &entry_json(&entry), chars),
                        is_error: false,
                    },
                    Err(refused) => refused_outcome(&refused),
                }
            }
            WRITE => {
                let target = match (field("id"), revision) {
                    (Some(id), Some(revision)) => MemoryTarget::Existing { id, revision },
                    (Some(_), None) => {
                        return invalid("input.revision is required with id".to_owned());
                    }
                    (None, _) => MemoryTarget::New(scope),
                };
                let change = MemoryChange::new(
                    target,
                    field("name"),
                    field("summary").unwrap_or_default(),
                    field("content").unwrap_or_default(),
                );
                match on_registry(caller, move |caller, registry| {
                    memories::write_entry(registry, caller, change)
                })
                .await
                {
                    Ok(written) => {
                        self.show_memory_change(granted, &written);
                        Outcome {
                            text: format!(
                                "Kept as {} at revision {}; the person sees the change.",
                                written.entry.header.id, written.entry.header.revision
                            ),
                            is_error: false,
                        }
                    }
                    Err(refused) => refused_outcome(&refused),
                }
            }
            _ => {
                let id = field("id").unwrap_or_default();
                match on_registry(caller, move |caller, registry| {
                    memories::forget_entry(registry, caller, &id, revision)
                })
                .await
                {
                    Ok(forgotten) => {
                        self.show_forgotten(granted, &forgotten.header.summary);
                        Outcome {
                            text: format!("Forgot {}.", forgotten.header.id),
                            is_error: false,
                        }
                    }
                    Err(refused) => refused_outcome(&refused),
                }
            }
        }
    }

    /// Tells the person what she kept, with its diff.
    fn show_memory_change(&self, granted: &Granted, written: &MemoryWritten) {
        let before = written.before.as_deref().unwrap_or_default();
        let diff = similar::TextDiff::from_lines(before, &written.entry.content)
            .unified_diff()
            .header("before", "after")
            .to_string();
        self.notice(
            granted,
            format!(
                "Moneypenny keeps in her memory: {}",
                written.entry.header.summary
            ),
            Some(json!({"diff": diff})),
        );
    }

    /// Tells the person what she forgot.
    fn show_forgotten(&self, granted: &Granted, summary: &str) {
        self.notice(
            granted,
            format!("Moneypenny forgot from her memory: {summary}"),
            None,
        );
    }

    fn notice(&self, granted: &Granted, text: String, data: Option<Value>) {
        let bounds = self.config.bounds();
        let mut desk = self.desk();
        if let Some(conversation) = desk
            .store
            .owned(&granted.conversation_id, &granted.person_id)
        {
            conversation.push(
                Entry {
                    role: Role::Notice,
                    text,
                    at: now(),
                    data,
                },
                bounds,
            );
        }
    }
}

/// What a tool says of a refused call; an entry it may not see, as a route says of what is not
/// there.
fn refused_outcome(refused: &Refused) -> Outcome {
    if refused.status == StatusCode::NOT_FOUND {
        return Outcome::error(tools::refusal(
            StatusCode::NOT_FOUND,
            &Value::Null,
            ai6::Limit::UNBOUNDED,
        ));
    }
    Outcome::error(refused.body["error"].as_str().unwrap_or_default())
}

/// What she keeps of the signed-in person, newest first, for the context of a new harness. Empty
/// when she keeps nothing of them or it cannot be read now; she can still search.
pub(crate) async fn starting_headers(gateway: &Gateway, person: &Person) -> Vec<MemoryHeader> {
    let Ok(caller) = caller_of(gateway, person.clone(), None, false, None, "").await else {
        return Vec::new();
    };
    let scope = MemoryScope::Person(person.id.clone());
    on_registry(caller, move |caller, registry| {
        memories::list_entries(registry, caller, &scope)
    })
    .await
    .unwrap_or_default()
}

/// `headers` as the context of a new harness, fenced as data, in at most `room` bytes; `None` when
/// none fits.
pub(crate) fn starting_summaries(
    headers: &[MemoryHeader],
    room: ai6::Limit,
    chars: ai6::Limit,
) -> Option<String> {
    let lead = "Your memory of this person, newest first; read an entry with memory_read when you \
                need it.\n";
    let left = match room.get() {
        Some(bytes) => ai6::Limit::at(bytes.checked_sub(lead.len() as u64)?),
        None => ai6::Limit::UNBOUNDED,
    };
    memories::summaries_for_a_model(headers, left, |value| tools::wrap(SEARCH, value, chars))
        .map(|summaries| format!("{lead}{summaries}"))
}
