//! Reading what was said and done in a Session: Moneypenny's conversation log and each
//! Session's harness context, as the Registry keeps them.
//!
//! - `GET /v1/user/operations/{operation_id}/sessions/{session_id}/context`: a Mission's
//!   Session's context, with `session.context` on the Operation; audited
//!   `session.context.read`.
//! - `GET /v1/user/assistant/conversations[?person=ID|?everyone=true]`: the person's own
//!   conversations in the log, or another's (or everyone's) with `audit.read` and
//!   `session.context` on `config:assistant`; audited `assistant.log.list`.
//! - `GET /v1/user/assistant/conversations/{id}/log`: a conversation's entries, each with the
//!   turn that ran it.
//! - `GET /v1/user/assistant/conversations/{id}/context`: the contexts of its harnesses'
//!   Sessions.
//!
//! A person reads their own conversations freely. Another person's needs both actions on
//! `config:assistant`, and the read is audited (`assistant.log.read`,
//! `assistant.context.read`) before anything is read. A conversation that is not there, and
//! another person's to someone without the actions, answer the same `404`.

use ai6::{AuditRecord, Person, SessionContext};
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    ApiError, Gateway, authorize_operation_action, authorize_resource_action, bearer, resolve_user,
    unix_timestamp,
};

/// Where the actions that read another person's conversations are held.
const ASSISTANT_RESOURCE: &str = "config:assistant";
/// Reads what an agent was given and did in a Session.
pub(crate) const SESSION_CONTEXT: &str = "session.context";
/// Reads the audit trail; with [`SESSION_CONTEXT`], another person's conversations.
const AUDIT_READ: &str = "audit.read";

/// A context as the app reads it: its record, and its content as kept.
fn context_json(context: &SessionContext) -> Value {
    let mut shown = serde_json::to_value(context).unwrap_or_else(|_| json!({}));
    shown["bytes"] = Value::from(context.content.len());
    shown["content"] = Value::from(context.content.as_str());
    shown
}

/// Records `action` by `person` before the read it audits; nothing is read when it cannot be.
async fn audit(
    gateway: &Gateway,
    person: &Person,
    action: &str,
    operation: Option<&str>,
    resource: String,
    target: String,
) -> Result<(), ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let most = crate::moneypenny::gateway_limits(gateway)
        .audit_target_bytes
        .or_max();
    let mut target = target;
    if target.len() > most {
        let mut end = most;
        while !target.is_char_boundary(end) {
            end -= 1;
        }
        target.truncate(end);
    }
    let record = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: action.to_owned(),
        operation: operation.unwrap_or_default().to_owned(),
        resource: Some(resource),
        target,
        at: unix_timestamp().unwrap_or(0.0),
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::unavailable())
}

/// Whether `person` may read another person's conversations: `audit.read` and
/// `session.context` on `config:assistant`.
async fn reads_others(gateway: &Gateway, person: &Person) -> Result<(), ApiError> {
    for action in [AUDIT_READ, SESSION_CONTEXT] {
        authorize_resource_action(
            gateway,
            person.clone(),
            ASSISTANT_RESOURCE.to_owned(),
            action,
        )
        .await?;
    }
    Ok(())
}

/// `GET /v1/user/operations/{operation_id}/sessions/{session_id}/context`.
pub(crate) async fn mission_context(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, session_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation, SESSION_CONTEXT).await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let wanted = session_id.clone();
    let context = tokio::task::spawn_blocking(move || registry.session_context(&wanted))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .filter(|context| context.operation() == Some(operation.as_str()))
        .ok_or_else(ApiError::not_found)?;
    let mission = match &context.owner {
        ai6::ContextOwner::Mission { mission, .. } => mission.clone(),
        _ => return Err(ApiError::not_found()),
    };
    audit(
        &gateway,
        &person,
        "session.context.read",
        Some(operation.as_str()),
        format!("mission:{mission}"),
        session_id,
    )
    .await?;
    Ok(Json(context_json(&context)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConversationsQuery {
    /// Another person's conversations.
    person: Option<String>,
    /// Everyone's conversations.
    #[serde(default)]
    everyone: bool,
}

/// `GET /v1/user/assistant/conversations`.
pub(crate) async fn conversations(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<ConversationsQuery>,
) -> Result<Json<Value>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let whose = match (query.everyone, query.person) {
        (true, Some(_)) => return Err(ApiError::bad_request()),
        (true, None) => None,
        (false, Some(other)) if other != person.id => {
            if !ai6::directorates::section_00::assistant::valid_identity(&other) {
                return Err(ApiError::bad_request());
            }
            Some(other)
        }
        (false, _) => Some(person.id.clone()),
    };
    if whose.as_deref() != Some(person.id.as_str()) {
        reads_others(&gateway, &person).await?;
        audit(
            &gateway,
            &person,
            "assistant.log.list",
            None,
            ASSISTANT_RESOURCE.to_owned(),
            whose.clone().unwrap_or_else(|| "everyone".to_owned()),
        )
        .await?;
    }
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let listed =
        tokio::task::spawn_blocking(move || registry.assistant_conversations(whose.as_deref()))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|_| ApiError::internal())?;
    Ok(Json(json!({ "conversations": listed })))
}

/// The person whose conversation `id` is, by its log; `None` when the log has none of it.
async fn owner_of(gateway: &Gateway, id: &str) -> Result<Option<String>, ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let wanted = id.to_owned();
    let conversations = tokio::task::spawn_blocking(move || registry.assistant_log(&wanted))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(conversations.first().map(|entry| entry.person.clone()))
}

/// Settles whether `person` may read conversation `id`, auditing `action` when it is another
/// person's; answers whose it is.
async fn may_read(
    gateway: &Gateway,
    person: &Person,
    id: &str,
    action: &str,
) -> Result<String, ApiError> {
    let owner = owner_of(gateway, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if owner != person.id {
        reads_others(gateway, person)
            .await
            .map_err(|_| ApiError::not_found())?;
        audit(
            gateway,
            person,
            action,
            None,
            ASSISTANT_RESOURCE.to_owned(),
            format!("{owner} {id}"),
        )
        .await?;
    }
    Ok(owner)
}

/// `GET /v1/user/assistant/conversations/{id}/log`.
pub(crate) async fn conversation_log(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let owner = may_read(&gateway, &person, &id, "assistant.log.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let wanted = id.clone();
    let entries = tokio::task::spawn_blocking(move || registry.assistant_log(&wanted))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(Json(json!({
        "conversation": id,
        "person": owner,
        "entries": entries,
    })))
}

/// `GET /v1/user/assistant/conversations/{id}/context`.
pub(crate) async fn conversation_context(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let owner = may_read(&gateway, &person, &id, "assistant.context.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let wanted = id.clone();
    let contexts = tokio::task::spawn_blocking(move || registry.conversation_contexts(&wanted))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(Json(json!({
        "conversation": id,
        "person": owner,
        "contexts": contexts.iter().map(context_json).collect::<Vec<_>>(),
    })))
}
