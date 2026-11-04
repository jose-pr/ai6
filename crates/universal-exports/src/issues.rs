//! Linked issues through Universal Exports: what a Mission is linked to and has written, and a
//! person's own links, comments, changes and creations.
//!
//! No route calls a tracker. Each writes Registry records and outbox entries, which the
//! Headquarters Station sends, because only it holds tracker tokens. Reading needs `mission.read`;
//! each write needs its own action (`issue.link`, `issue.comment`, `issue.update`,
//! `issue.create`) on the Operation. A person's writes go out under the Operation's identity with
//! a footer naming them, and need no green ink: the action is the permission.

use std::sync::Arc;

use ai6::directorates::m::issues::{IssuePolicy, is_ready_label};
use ai6::directorates::q::outbound;
use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::mission_issues::{
    IssueLink, IssueProposal, IssueQuestion, IssueReply, LinkRelation, LinkState, OutboxEntry,
    OutboxKind, ProposalState, ProposedIssue, QuestionState, ReplyState,
};
use ai6::{AuditRecord, Person, TimelineEvent};
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, Gateway, GatewayError, authorize_operation_action, bearer,
    ensure_operation_registered, resolve_user, unix_timestamp,
};

/// The Mission a creation names none of its own is held under: a record, never a running Mission.
const NO_MISSION: &str = "operation-created";

/// A linked issue and what the Mission said on it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IssueView {
    /// `TRACKER:ITEM`, as every route under the Mission names it.
    pub issue: String,
    /// The tracker instance.
    pub tracker: String,
    /// The provider-local item.
    pub item: String,
    /// A link to the issue.
    pub url: Option<String>,
    /// `source`, `related`, `child`, `parent` or `mirror`.
    pub relation: LinkRelation,
    /// `proposed`, `active` or `unlinked`.
    pub state: LinkState,
    /// Who linked it.
    pub linked_by: String,
    /// The issue's title when last read. Untrusted text.
    pub title: Option<String>,
    /// `open` or `closed` when last read.
    pub issue_state: Option<String>,
    /// The labels when last read. Untrusted text.
    pub labels: Vec<String>,
    /// The phase AI6 last wrote on it.
    pub phase: Option<String>,
    /// The open question on it, if any.
    pub open_question: Option<QuestionView>,
}

/// A question waiting for an answer.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestionView {
    /// When it was asked, Unix seconds.
    pub asked_at: f64,
    /// When it stops waiting, Unix seconds.
    pub expires_at: f64,
}

/// One write to a tracker, without its text.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EntryView {
    /// The entry.
    pub id: String,
    /// What it writes.
    pub kind: OutboxKind,
    /// `TRACKER:ITEM`; only the tracker for a creation not sent yet.
    pub issue: String,
    /// Who wrote it.
    pub actor: String,
    /// `pending`, `held` (waiting for green ink), `sending`, `posted`, `refused` or `failed`.
    pub state: ai6::directorates::registry::mission_issues::OutboxState,
    /// Where it went, once posted.
    pub url: Option<String>,
    /// Why it was refused or failed.
    pub reason: Option<String>,
    /// What a creation could not carry over.
    pub dropped: Vec<String>,
    /// When it was queued, Unix seconds.
    pub at: f64,
}

/// How many comments came in, and what became of them.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ReplyCounts {
    /// Reached the Agent.
    pub delivered: usize,
    /// Waiting to reach it.
    pub pending: usize,
    /// From someone not permitted, or when the Mission was not running.
    pub ignored: usize,
}

/// `GET .../missions/{mission}/issues`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MissionIssues {
    /// The links, unlinked ones left out.
    pub issues: Vec<IssueView>,
    /// The Mission's writes, oldest first.
    pub entries: Vec<EntryView>,
    /// The replies, as counts.
    pub replies: ReplyCounts,
}

/// What a write came to.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IssueWrite {
    /// The issue it concerns, when it names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issue: Option<String>,
    /// The outbox entry, when it queued one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// The proposal record, when it creates issues.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<String>,
    /// `queued`, `linked`, `unlinked` or a proposal's state.
    pub state: String,
    /// Whether the comment answered the open question on the issue.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub answered: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinkRequest {
    /// `TRACKER:ITEM`.
    issue: String,
    /// `related` (the default), `child`, `parent` or `mirror`.
    relation: Option<LinkRelation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommentRequest {
    body: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateRequest {
    #[serde(default)]
    add_labels: Vec<String>,
    #[serde(default)]
    remove_labels: Vec<String>,
    transition: Option<String>,
    #[serde(default)]
    close: bool,
    #[serde(default)]
    reopen: bool,
    #[serde(default)]
    add_assignees: Vec<String>,
    #[serde(default)]
    remove_assignees: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateRequest {
    tracker: String,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    labels: Vec<String>,
    issue_type: Option<String>,
    priority: Option<String>,
    /// The linked issue it is a child of, as `TRACKER:ITEM`; needs `mission`.
    parent: Option<String>,
    /// The Mission to link it to.
    mission: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApproveRequest {
    /// Add the intake labels to what it creates; needs `mission.create` too.
    #[serde(default)]
    mark_ready: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RejectRequest {
    /// Why, for the record.
    reason: Option<String>,
}

fn registry_failure(error: std::io::Error) -> ApiError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ApiError::not_found(),
        std::io::ErrorKind::AlreadyExists => ApiError::from(GatewayError::Conflict),
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
            ApiError::bad_request()
        }
        _ => ApiError::internal(),
    }
}

/// Resolves the caller, checks `action` on the Operation and that the Operation exists.
async fn authorized(
    gateway: &Gateway,
    headers: &HeaderMap,
    operation_id: String,
    action: &'static str,
) -> Result<(Person, OperationId, Arc<dyn RegistryBackend>), ApiError> {
    let person = resolve_user(gateway, bearer(headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(gateway, person.clone(), &operation, action).await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation).await?;
    Ok((person, operation, registry))
}

/// Runs Registry work off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_failure)
}

/// The Operation's issue policy, or the strict default when this gateway has no roster.
async fn policy_of(gateway: &Gateway, operation: &OperationId) -> IssuePolicy {
    let Some(source) = gateway.roster.clone() else {
        return IssuePolicy::default();
    };
    let wanted = operation.to_string();
    tokio::task::spawn_blocking(move || {
        let snapshot = source.load("").ok()?;
        Some(snapshot.roster.operation(&wanted).ok()?.issues.clone())
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// Whether `label` would start work on the Operation's trackers.
async fn starts_work(gateway: &Gateway, operation: &OperationId, labels: Vec<String>) -> bool {
    let wanted = operation.to_string();
    let source = gateway.roster.clone();
    tokio::task::spawn_blocking(move || {
        let snapshot = source.and_then(|source| source.load("").ok());
        let held = snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.roster.operation(&wanted).ok());
        labels.iter().any(|label| is_ready_label(held, label))
    })
    .await
    .unwrap_or(true)
}

/// A `TRACKER:ITEM` a route may take, split: both parts short, with no control characters.
fn split_reference(reference: &str) -> Result<(&str, &str), ApiError> {
    let (tracker, item) = reference
        .split_once(':')
        .ok_or_else(ApiError::bad_request)?;
    let fine =
        |part: &str| !part.is_empty() && part.len() <= 512 && !part.chars().any(char::is_control);
    if fine(tracker) && fine(item) {
        Ok((tracker, item))
    } else {
        Err(ApiError::bad_request())
    }
}

/// The Mission's link named `reference` in this Operation, active or proposed; anything else
/// answers `404` before anything is audited.
async fn held_link(
    registry: &Arc<dyn RegistryBackend>,
    operation: &OperationId,
    mission: &str,
    reference: &str,
) -> Result<IssueLink, ApiError> {
    let (tracker, item) = split_reference(reference)?;
    let (tracker, item) = (tracker.to_owned(), item.to_owned());
    let (store, wanted, operation) = (registry.clone(), mission.to_owned(), operation.to_string());
    let links = blocking(move || store.issue_links(&wanted)).await?;
    links
        .into_iter()
        .find(|link| {
            link.tracker == tracker
                && link.item == item
                && link.operation == operation
                && link.state != LinkState::Unlinked
        })
        .ok_or_else(ApiError::not_found)
}

/// Whether the Operation knows this Mission: a link, a Session record or its brief names it, and
/// nothing names another Operation. Refused before anything is audited or told.
async fn known_mission(
    gateway: &Gateway,
    registry: &Arc<dyn RegistryBackend>,
    operation: &OperationId,
    mission: &str,
) -> Result<(), ApiError> {
    let (store, wanted, operation_key) =
        (registry.clone(), mission.to_owned(), operation.to_string());
    let on_board = {
        let inner = gateway.inner.lock().await;
        inner.sessions.values().any(|session| {
            session.mission_id.as_str() == mission && session.operation_id == *operation
        }) || inner
            .missions
            .keys()
            .any(|(held, id)| held == operation && id.as_str() == mission)
    };
    let known = blocking(move || {
        let linked = store
            .issue_links(&wanted)?
            .iter()
            .any(|link| link.operation == operation_key);
        let recorded = store.sessions()?.into_iter().any(|record| {
            record.mission_id == wanted
                && record.operation.as_deref() == Some(operation_key.as_str())
        });
        Ok(linked || recorded)
    })
    .await?;
    if known || on_board {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

async fn record(
    registry: &Arc<dyn RegistryBackend>,
    person: &Person,
    operation: &OperationId,
    mission: Option<&str>,
    action: String,
    target: String,
    at: f64,
) -> Result<(), ApiError> {
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: format!("person:{}", person.id),
        action: action.clone(),
        operation: operation.to_string(),
        resource: None,
        target: target.clone(),
        at,
    };
    let mut event = TimelineEvent::new("issue", action, operation.to_string(), at)
        .actor(format!("person:{}", person.id))
        .detail("issue", target);
    if let Some(mission) = mission {
        event = event.mission(mission.to_owned());
    }
    let store = registry.clone();
    blocking(move || {
        store.record_audit(&audit)?;
        let _ = store.record_event(&event);
        Ok(())
    })
    .await
}

/// `GET /v1/user/operations/{operation_id}/missions/{mission_id}/issues`.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
) -> Result<Json<MissionIssues>, ApiError> {
    let mission_id = crate::mission_key(&gateway, &mission_id).await?;
    let (_, operation, registry) =
        authorized(&gateway, &headers, operation_id, "mission.read").await?;
    known_mission(&gateway, &registry, &operation, &mission_id).await?;
    let store = registry.clone();
    let operation_key = operation.to_string();
    let view = blocking(move || {
        let questions = store.issue_questions(&mission_id)?;
        let replies = store.issue_replies(&mission_id)?;
        let entries = store.outbox_entries(Some(&mission_id))?;
        let links = store.issue_links(&mission_id)?;
        let mut counts = ReplyCounts::default();
        for reply in &replies {
            match reply.state {
                ReplyState::Delivered => counts.delivered += 1,
                ReplyState::Pending => counts.pending += 1,
                ReplyState::Ignored => counts.ignored += 1,
            }
        }
        Ok(MissionIssues {
            issues: links
                .into_iter()
                .filter(|link| link.operation == operation_key && link.state != LinkState::Unlinked)
                .map(|link| view_of(link, &questions))
                .collect(),
            entries: entries
                .into_iter()
                .filter(|entry| entry.operation == operation_key)
                .map(entry_view)
                .collect(),
            replies: counts,
        })
    })
    .await?;
    Ok(Json(view))
}

fn view_of(link: IssueLink, questions: &[IssueQuestion]) -> IssueView {
    let open_question = questions
        .iter()
        .find(|question| {
            question.state == QuestionState::Open
                && question.tracker == link.tracker
                && question.item == link.item
        })
        .map(|question| QuestionView {
            asked_at: question.asked_at,
            expires_at: question.expires_at,
        });
    let issue = link.name();
    let (title, issue_state, labels) = match link.snapshot {
        Some(snapshot) => (Some(snapshot.title), snapshot.state, snapshot.labels),
        None => (None, None, Vec::new()),
    };
    IssueView {
        issue,
        tracker: link.tracker,
        item: link.item,
        url: link.url,
        relation: link.relation,
        state: link.state,
        linked_by: link.linked_by,
        title,
        issue_state,
        labels,
        phase: link.phase,
        open_question,
    }
}

fn entry_view(entry: OutboxEntry) -> EntryView {
    let result = entry.result.unwrap_or_default();
    EntryView {
        issue: if entry.item.is_empty() {
            entry.tracker
        } else {
            format!("{}:{}", entry.tracker, entry.item)
        },
        id: entry.id,
        kind: entry.kind,
        actor: entry.actor,
        state: entry.state,
        url: result.url,
        reason: entry.error,
        dropped: result.dropped,
        at: entry.at,
    }
}

/// `POST .../missions/{mission_id}/issues`: links an issue. The Headquarters Station reads it,
/// and a foreign or missing item comes back unlinked with the reason.
pub(crate) async fn link(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
    Json(request): Json<LinkRequest>,
) -> Result<Json<IssueWrite>, ApiError> {
    let mission_id = crate::mission_key(&gateway, &mission_id).await?;
    let (tracker, item) = split_reference(&request.issue)?;
    let relation = request.relation.unwrap_or(LinkRelation::Related);
    if relation == LinkRelation::Source {
        return Err(ApiError::bad_request());
    }
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "issue.link").await?;
    known_mission(&gateway, &registry, &operation, &mission_id).await?;
    let at = unix_timestamp()?;
    let link = IssueLink {
        operation: operation.to_string(),
        mission_id: mission_id.clone(),
        tracker: tracker.to_owned(),
        item: item.to_owned(),
        url: None,
        relation,
        state: LinkState::Active,
        linked_by: format!("person:{}", person.id),
        linked_at: at,
        status_comment: None,
        phase: None,
        snapshot: None,
    };
    let entry = resolve_and_link(&registry, link, &person, at).await?;
    record(
        &registry,
        &person,
        &operation,
        Some(&mission_id),
        "issue.link".to_owned(),
        request.issue.clone(),
        at,
    )
    .await?;
    Ok(Json(IssueWrite {
        issue: Some(request.issue),
        entry: Some(entry),
        proposal: None,
        state: "linked".to_owned(),
        answered: false,
    }))
}

/// Writes the link active with the entry that has the Headquarters Station read the issue.
async fn resolve_and_link(
    registry: &Arc<dyn RegistryBackend>,
    link: IssueLink,
    person: &Person,
    at: f64,
) -> Result<String, ApiError> {
    let mut entry = OutboxEntry::new(
        &link.operation,
        &link.mission_id,
        &link.tracker,
        &link.item,
        OutboxKind::Link,
        format!("person:{}", person.id),
        at,
    );
    entry.payload = serde_json::json!({ "resolve": true });
    let store = registry.clone();
    blocking(move || {
        store.link_issue(&link)?;
        Ok(store.queue_outbox(&entry)?.id)
    })
    .await
}

/// `DELETE .../missions/{mission_id}/issues/{reference}`: unlinks an issue; what still waits to
/// write to it is refused.
pub(crate) async fn unlink(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id, reference)): Path<(String, String, String)>,
) -> Result<Json<IssueWrite>, ApiError> {
    let mission_id = crate::mission_key(&gateway, &mission_id).await?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "issue.link").await?;
    let held = held_link(&registry, &operation, &mission_id, &reference).await?;
    let at = unix_timestamp()?;
    let store = registry.clone();
    let wanted = mission_id.clone();
    blocking(move || store.unlink_issue(&wanted, &held.tracker, &held.item)).await?;
    record(
        &registry,
        &person,
        &operation,
        Some(&mission_id),
        "issue.unlink".to_owned(),
        reference.clone(),
        at,
    )
    .await?;
    Ok(Json(IssueWrite {
        issue: Some(reference),
        entry: None,
        proposal: None,
        state: "unlinked".to_owned(),
        answered: false,
    }))
}

/// `POST .../missions/{mission_id}/issues/{reference}/accept`: accepts a link an Agent proposed.
pub(crate) async fn accept(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id, reference)): Path<(String, String, String)>,
) -> Result<Json<IssueWrite>, ApiError> {
    let mission_id = crate::mission_key(&gateway, &mission_id).await?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "issue.link").await?;
    let mut held = held_link(&registry, &operation, &mission_id, &reference).await?;
    if held.state != LinkState::Proposed {
        return Err(ApiError::from(GatewayError::Conflict));
    }
    let at = unix_timestamp()?;
    held.state = LinkState::Active;
    let entry = resolve_and_link(&registry, held, &person, at).await?;
    record(
        &registry,
        &person,
        &operation,
        Some(&mission_id),
        "issue.link.accepted".to_owned(),
        reference.clone(),
        at,
    )
    .await?;
    Ok(Json(IssueWrite {
        issue: Some(reference),
        entry: Some(entry),
        proposal: None,
        state: "linked".to_owned(),
        answered: false,
    }))
}

/// Takes a token from the Mission's and the Operation's buckets, the ones an Agent's posts take.
async fn take_quota(
    registry: &Arc<dyn RegistryBackend>,
    policy: &IssuePolicy,
    operation: &OperationId,
    mission: &str,
    at: f64,
) -> Result<(), ApiError> {
    let mut buckets: Vec<(String, f64, u32)> = Vec::new();
    if let Some(per_hour) = policy.posts_per_hour.get() {
        let burst = policy.post_burst.get().unwrap_or(per_hour).max(1);
        buckets.push((
            format!("issue:mission:{mission}"),
            per_hour as f64 / 3600.0,
            u32::try_from(burst).unwrap_or(u32::MAX),
        ));
    }
    if let Some(per_hour) = policy.operation_posts_per_hour.get() {
        buckets.push((
            format!("issue:operation:{operation}"),
            per_hour as f64 / 3600.0,
            u32::try_from(per_hour).unwrap_or(u32::MAX).max(1),
        ));
    }
    let store = registry.clone();
    let taken = blocking(move || {
        let borrowed: Vec<(&str, f64, u32)> = buckets
            .iter()
            .map(|(key, rate, capacity)| (key.as_str(), *rate, *capacity))
            .collect();
        store.take_quotas(&borrowed, at)
    })
    .await?;
    if taken {
        Ok(())
    } else {
        Err(ApiError::too_many_attempts(std::time::Duration::from_secs(
            60,
        )))
    }
}

/// `POST .../missions/{mission_id}/issues/{reference}/comments`: posts as the Operation, with a
/// footer naming the person. On an issue with an open question it is the answer, as an
/// operator's text: the Mission resumes unexposed.
pub(crate) async fn comment(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id, reference)): Path<(String, String, String)>,
    Json(request): Json<CommentRequest>,
) -> Result<Json<IssueWrite>, ApiError> {
    let mission_id = crate::mission_key(&gateway, &mission_id).await?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "issue.comment").await?;
    let held = held_link(&registry, &operation, &mission_id, &reference).await?;
    if held.state != LinkState::Active {
        return Err(ApiError::from(GatewayError::Conflict));
    }
    let policy = policy_of(&gateway, &operation).await;
    let body = outbound::check_person(&request.body, policy.max_body_bytes)
        .map_err(|_| ApiError::bad_request())?;
    let at = unix_timestamp()?;
    take_quota(&registry, &policy, &operation, &mission_id, at).await?;
    let name = person.name.clone();
    let mut entry = OutboxEntry::new(
        &held.operation,
        &held.mission_id,
        &held.tracker,
        &held.item,
        OutboxKind::Comment,
        format!("person:{}", person.id),
        at,
    );
    entry.body = format!("{body}\n\n---\nPosted through AI6 by {name}.");
    let store = registry.clone();
    let (answer_text, answer_by) = (body, name);
    let (queued, answered) = blocking(move || {
        let queued = store.queue_outbox(&entry)?;
        let open = store
            .issue_questions(&entry.mission_id)?
            .into_iter()
            .any(|question| {
                question.state == QuestionState::Open
                    && question.tracker == entry.tracker
                    && question.item == entry.item
            });
        if open {
            let reply = IssueReply {
                id: format!("person-{}", queued.id),
                mission_id: entry.mission_id.clone(),
                tracker: entry.tracker.clone(),
                item: entry.item.clone(),
                author_id: format!("person:{}", entry.actor),
                author: answer_by,
                body: answer_text,
                created: String::new(),
                state: ReplyState::Pending,
                operator: true,
            };
            store.store_issue_reply(&reply)?;
        }
        Ok((queued.id, open))
    })
    .await?;
    record(
        &registry,
        &person,
        &operation,
        Some(&mission_id),
        "issue.comment.queued".to_owned(),
        format!("{reference} {queued}"),
        at,
    )
    .await?;
    Ok(Json(IssueWrite {
        issue: Some(reference),
        entry: Some(queued),
        proposal: None,
        state: "queued".to_owned(),
        answered,
    }))
}

/// `POST .../missions/{mission_id}/issues/{reference}/update`: changes labels, workflow state or
/// assignees. Adding a label that starts work also needs `mission.create`.
pub(crate) async fn update(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id, reference)): Path<(String, String, String)>,
    Json(request): Json<UpdateRequest>,
) -> Result<Json<IssueWrite>, ApiError> {
    let mission_id = crate::mission_key(&gateway, &mission_id).await?;
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "issue.update").await?;
    let held = held_link(&registry, &operation, &mission_id, &reference).await?;
    if held.state != LinkState::Active {
        return Err(ApiError::from(GatewayError::Conflict));
    }
    if request.close && request.reopen {
        return Err(ApiError::bad_request());
    }
    let empty = request.add_labels.is_empty()
        && request.remove_labels.is_empty()
        && request.transition.is_none()
        && !request.close
        && !request.reopen
        && request.add_assignees.is_empty()
        && request.remove_assignees.is_empty();
    if empty {
        return Err(ApiError::bad_request());
    }
    if starts_work(&gateway, &operation, request.add_labels.clone()).await {
        authorize_operation_action(&gateway, person.clone(), &operation, "mission.create").await?;
    }
    let at = unix_timestamp()?;
    let mut entry = OutboxEntry::new(
        &held.operation,
        &held.mission_id,
        &held.tracker,
        &held.item,
        OutboxKind::Update,
        format!("person:{}", person.id),
        at,
    );
    entry.payload = serde_json::json!({
        "add_labels": request.add_labels,
        "remove_labels": request.remove_labels,
        "transition": request.transition,
        "add_assignees": request.add_assignees,
        "remove_assignees": request.remove_assignees,
        "state": if request.close {
            Some("closed")
        } else if request.reopen {
            Some("open")
        } else {
            None
        },
    });
    let store = registry.clone();
    let queued = blocking(move || Ok(store.queue_outbox(&entry)?.id)).await?;
    record(
        &registry,
        &person,
        &operation,
        Some(&mission_id),
        "issue.update.queued".to_owned(),
        format!("{reference} {queued}"),
        at,
    )
    .await?;
    Ok(Json(IssueWrite {
        issue: Some(reference),
        entry: Some(queued),
        proposal: None,
        state: "queued".to_owned(),
        answered: false,
    }))
}

/// `POST /v1/user/operations/{operation_id}/issues`: creates an issue, optionally linked to a
/// Mission. The record is an approved proposal, so the Headquarters Station sends it as it does
/// an Agent's.
pub(crate) async fn create(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    Json(request): Json<CreateRequest>,
) -> Result<(StatusCode, Json<IssueWrite>), ApiError> {
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "issue.create").await?;
    if starts_work(&gateway, &operation, request.labels.clone()).await {
        authorize_operation_action(&gateway, person.clone(), &operation, "mission.create").await?;
    }
    if let Some(mission) = &request.mission {
        known_mission(&gateway, &registry, &operation, mission).await?;
    }
    if let Some(parent) = &request.parent {
        let Some(mission) = &request.mission else {
            return Err(ApiError::bad_request());
        };
        held_link(&registry, &operation, mission, parent).await?;
    }
    if request.title.trim().is_empty() || request.tracker.trim().is_empty() {
        return Err(ApiError::bad_request());
    }
    let policy = policy_of(&gateway, &operation).await;
    let title = outbound::check_person(&request.title, policy.max_body_bytes)
        .map_err(|_| ApiError::bad_request())?;
    let body = if request.body.trim().is_empty() {
        String::new()
    } else {
        outbound::check_person(&request.body, policy.max_body_bytes)
            .map_err(|_| ApiError::bad_request())?
    };
    let at = unix_timestamp()?;
    let by = format!("person:{}", person.id);
    let proposal = IssueProposal {
        id: String::new(),
        operation: operation.to_string(),
        mission_id: request
            .mission
            .clone()
            .unwrap_or_else(|| NO_MISSION.to_owned()),
        proposer: by.clone(),
        specs: vec![ProposedIssue {
            tracker: request.tracker.clone(),
            title,
            body: format!("{body}\n\n---\nCreated through AI6 by {}.", person.name)
                .trim_start()
                .to_owned(),
            labels: request.labels,
            issue_type: request.issue_type,
            priority: request.priority,
            parent: request.parent,
            mirror_of: None,
            depth: 0,
            mission: None,
        }],
        exposure: Vec::new(),
        findings: Vec::new(),
        dropped: Vec::new(),
        state: ProposalState::Pending,
        decided_by: None,
        mark_ready: false,
        created: Vec::new(),
        failed: Vec::new(),
        retry_key: None,
        told: false,
        at,
    };
    let store = registry.clone();
    let signer = person.id.clone();
    let stored = blocking(move || {
        let stored = store.propose_issues(&proposal)?;
        store.decide_issue_proposal(&stored.id, true, &signer, false)?;
        Ok(stored.id)
    })
    .await?;
    record(
        &registry,
        &person,
        &operation,
        request.mission.as_deref(),
        "issue.create.queued".to_owned(),
        format!("{} {stored}", request.tracker),
        at,
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(IssueWrite {
            issue: None,
            entry: None,
            proposal: Some(stored),
            state: "approved".to_owned(),
            answered: false,
        }),
    ))
}

/// Creates an issue in `tracker` for the Mission `brief`, which `person` asks for with it: an
/// approved proposal whose one issue carries the brief, so the Headquarters Station enters the
/// Mission, numbered by the issue and linked to it, once the issue exists. Returns the proposal.
/// The caller has checked `issue.create` and `mission.create`.
pub(crate) async fn create_for_mission(
    gateway: &Gateway,
    registry: &Arc<dyn RegistryBackend>,
    (person, operation): (&Person, &OperationId),
    (tracker, title, body): (&str, &str, &str),
    brief: ai6::Brief,
) -> Result<String, ApiError> {
    if !operation_trackers(gateway, operation)
        .await
        .is_none_or(|trackers| trackers.iter().any(|known| known == tracker))
    {
        return Err(ApiError::invalid(format!(
            "Operation {operation} has no tracker {tracker:?}"
        )));
    }
    let policy = policy_of(gateway, operation).await;
    let title = outbound::check_person(title, policy.max_body_bytes)
        .map_err(|_| ApiError::bad_request())?;
    let body = if body.trim().is_empty() {
        String::new()
    } else {
        outbound::check_person(body, policy.max_body_bytes).map_err(|_| ApiError::bad_request())?
    };
    let at = unix_timestamp()?;
    let proposal = IssueProposal {
        id: String::new(),
        operation: operation.to_string(),
        mission_id: NO_MISSION.to_owned(),
        proposer: format!("person:{}", person.id),
        specs: vec![ProposedIssue {
            tracker: tracker.to_owned(),
            title,
            body: format!("{body}\n\n---\nCreated through AI6 by {}.", person.name)
                .trim_start()
                .to_owned(),
            mission: Some(brief),
            ..ProposedIssue::default()
        }],
        exposure: Vec::new(),
        findings: Vec::new(),
        dropped: Vec::new(),
        state: ProposalState::Pending,
        decided_by: None,
        mark_ready: false,
        created: Vec::new(),
        failed: Vec::new(),
        retry_key: None,
        told: false,
        at,
    };
    let store = registry.clone();
    let signer = person.id.clone();
    let stored = blocking(move || {
        let stored = store.propose_issues(&proposal)?;
        store.decide_issue_proposal(&stored.id, true, &signer, false)?;
        Ok(stored.id)
    })
    .await?;
    record(
        registry,
        person,
        operation,
        None,
        "issue.create.queued".to_owned(),
        format!("{tracker} {stored}"),
        at,
    )
    .await?;
    Ok(stored)
}

/// The tracker instances of `operation`, or `None` when this gateway has no roster to say.
async fn operation_trackers(gateway: &Gateway, operation: &OperationId) -> Option<Vec<String>> {
    let source = gateway.roster.clone()?;
    let wanted = operation.to_string();
    tokio::task::spawn_blocking(move || {
        let snapshot = source.load("").ok()?;
        let held = snapshot.roster.operation(&wanted).ok()?;
        Some(held.sources.keys().cloned().collect())
    })
    .await
    .ok()
    .flatten()
}

/// `GET /v1/user/operations/{operation_id}/issue-proposals`: what Agents asked to create and how
/// it came out, newest last. Their text is the Agents' own.
pub(crate) async fn proposals(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<Vec<IssueProposal>>, ApiError> {
    let (_, operation, registry) =
        authorized(&gateway, &headers, operation_id, "mission.read").await?;
    let wanted = operation.to_string();
    let held = blocking(move || registry.issue_proposals(None)).await?;
    Ok(Json(
        held.into_iter()
            .filter(|proposal| proposal.operation == wanted)
            .collect(),
    ))
}

/// The proposal `id` of this Operation; another Operation's answers `404`.
async fn proposal_of(
    registry: &Arc<dyn RegistryBackend>,
    operation: &OperationId,
    id: &str,
) -> Result<IssueProposal, ApiError> {
    let (store, wanted) = (registry.clone(), id.to_owned());
    let held = blocking(move || store.issue_proposal(&wanted)).await?;
    held.filter(|proposal| proposal.operation == operation.as_str())
        .ok_or_else(ApiError::not_found)
}

/// `POST .../issue-proposals/{id}/approve`: the Headquarters Station then creates what it asks
/// for. `mark_ready` also needs `mission.create`.
pub(crate) async fn approve(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, id)): Path<(String, String)>,
    request: Option<Json<ApproveRequest>>,
) -> Result<Json<IssueProposal>, ApiError> {
    let request = request.map(|Json(request)| request).unwrap_or_default();
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "issue.create").await?;
    if request.mark_ready {
        authorize_operation_action(&gateway, person.clone(), &operation, "mission.create").await?;
    }
    proposal_of(&registry, &operation, &id).await?;
    decide(
        &registry,
        &person,
        &operation,
        &id,
        true,
        request.mark_ready,
    )
    .await
}

/// `POST .../issue-proposals/{id}/reject`.
pub(crate) async fn reject(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, id)): Path<(String, String)>,
    request: Option<Json<RejectRequest>>,
) -> Result<Json<IssueProposal>, ApiError> {
    let reason = request
        .map(|Json(request)| request)
        .unwrap_or_default()
        .reason
        .map(|reason| reason.trim().to_owned())
        .filter(|reason| !reason.is_empty());
    if let Some(reason) = &reason
        && (!gateway
            .config
            .unex()
            .text
            .held_reason_chars
            .allows(reason.chars().count())
            || reason
                .chars()
                .any(|character| character.is_control() && character != '\n'))
    {
        return Err(ApiError::bad_request());
    }
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "issue.create").await?;
    proposal_of(&registry, &operation, &id).await?;
    let decided = decide(&registry, &person, &operation, &id, false, false).await?;
    if let Some(reason) = reason {
        let event = TimelineEvent::new(
            "issue",
            "proposal_rejected",
            operation.to_string(),
            unix_timestamp()?,
        )
        .mission(decided.mission_id.clone())
        .actor(format!("person:{}", person.id))
        .detail("proposal", id)
        .detail("reason", reason);
        let store = registry.clone();
        blocking(move || {
            let _ = store.record_event(&event);
            Ok(())
        })
        .await?;
    }
    Ok(decided)
}

async fn decide(
    registry: &Arc<dyn RegistryBackend>,
    person: &Person,
    operation: &OperationId,
    id: &str,
    approved: bool,
    mark_ready: bool,
) -> Result<Json<IssueProposal>, ApiError> {
    let at = unix_timestamp()?;
    let store = registry.clone();
    let (wanted, signer) = (id.to_owned(), person.id.clone());
    let decided =
        blocking(move || store.decide_issue_proposal(&wanted, approved, &signer, mark_ready))
            .await?
            .ok_or_else(|| ApiError::from(GatewayError::Conflict))?;
    record(
        registry,
        person,
        operation,
        Some(&decided.mission_id),
        format!(
            "issue.create.{}",
            if approved { "approved" } else { "rejected" }
        ),
        decided.id.clone(),
        at,
    )
    .await?;
    Ok(Json(decided))
}

#[cfg(test)]
#[path = "issues_tests.rs"]
mod tests;
