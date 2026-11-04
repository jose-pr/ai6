//! Live reads of an Operation's trackers. The gateway holds no tracker token: a read waits here
//! until the Headquarters Station takes it (`GET /v1/station/source-reads/next`) and answers it
//! through the Operation's own providers (`POST /v1/station/source-reads/{id}/result`).
//!
//! Reading needs `mission.read` on the Operation. Titles, bodies and comments are the tracker's
//! text, written by anyone who can write there: untrusted.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use ai6::directorates::registry::mission_issues::LinkState;
use ai6::{SourceRead, SourceReadAnswer};
use ai6_diptel::OperationId;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{Notify, oneshot};

use crate::{ApiError, Gateway, authorize_operation_action, bearer, resolve_user};

/// Reads waiting for the Headquarters Station, and the callers waiting for their answers.
#[derive(Default)]
pub(crate) struct SourceReads {
    state: std::sync::Mutex<Book>,
    queued: Notify,
}

#[derive(Default)]
struct Book {
    queue: VecDeque<SourceRead>,
    waiting: HashMap<String, oneshot::Sender<SourceReadAnswer>>,
}

impl SourceReads {
    fn book(&self) -> std::sync::MutexGuard<'_, Book> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queues `read` and waits up to `wait` for its answer; `None` when the queue is full or
    /// nothing answered in time.
    async fn ask(
        &self,
        read: SourceRead,
        most: ai6::Limit,
        wait: Duration,
    ) -> Option<SourceReadAnswer> {
        let id = read.id.clone();
        let (answer, answered) = oneshot::channel();
        {
            let mut book = self.book();
            if !most.allows(book.waiting.len() + 1) {
                return None;
            }
            book.waiting.insert(id.clone(), answer);
            book.queue.push_back(read);
        }
        self.queued.notify_waiters();
        let answered = tokio::time::timeout(wait, answered).await;
        let mut book = self.book();
        book.waiting.remove(&id);
        book.queue.retain(|queued| queued.id != id);
        answered.ok()?.ok()
    }

    /// The oldest read nobody took, waiting up to `wait` for one.
    async fn take(&self, wait: Duration) -> Option<SourceRead> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.queued.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(read) = self.book().queue.pop_front() {
                return Some(read);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return None;
            }
        }
    }

    /// Hands `answer` to whoever waits for read `id`; false when nobody does.
    fn answer(&self, id: &str, answer: SourceReadAnswer) -> bool {
        self.book()
            .waiting
            .remove(id)
            .is_some_and(|waiting| waiting.send(answer).is_ok())
    }
}

#[derive(Deserialize)]
pub(crate) struct ItemQuery {
    tracker: String,
    item: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct ItemsQuery {
    tracker: Option<String>,
    query: Option<String>,
}

/// The Operation `operation_id` once `headers`' person holds `mission.read` there.
async fn reader(
    gateway: &Gateway,
    headers: &HeaderMap,
    operation_id: String,
) -> Result<OperationId, ApiError> {
    let person = resolve_user(gateway, bearer(headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(gateway, person, &operation, "mission.read").await?;
    Ok(operation)
}

/// Asks the Headquarters Station for `read` and answers the caller with what it found.
async fn relayed(gateway: &Gateway, read: SourceRead) -> Result<SourceReadAnswer, Box<Response>> {
    let limits = crate::moneypenny::gateway_limits(gateway);
    let answer = gateway
        .source_reads
        .ask(read, limits.source_reads, limits.source_wait.0)
        .await;
    let Some(answer) = answer else {
        return Err(Box::new(
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(json!({"error": "the Headquarters Station did not answer the tracker read in time"})),
            )
                .into_response(),
        ));
    };
    if answer.item.is_none()
        && answer.items.is_empty()
        && let Some(error) = &answer.error
    {
        return Err(Box::new(
            (StatusCode::BAD_GATEWAY, Json(json!({"error": error}))).into_response(),
        ));
    }
    Ok(answer)
}

/// A new read of Operation `operation`.
fn read_of(gateway: &Gateway, operation: &OperationId) -> SourceRead {
    let limits = crate::moneypenny::gateway_limits(gateway);
    let bound = |limit: ai6::Limit| u32::try_from(limit.or_max()).unwrap_or(u32::MAX);
    SourceRead {
        id: uuid::Uuid::new_v4().to_string(),
        operation: operation.to_string(),
        tracker: None,
        item: None,
        query: None,
        comments: bound(limits.source_comments),
        items: bound(limits.source_items),
    }
}

/// `GET /v1/user/operations/{operation_id}/source-item?tracker=&item=`: one item of the
/// Operation's trackers as it is now, `{item, comments, missions, error?}`: its title, state,
/// labels and assignees, its latest comments with their authors' roles and dates, and the
/// Missions linked to it. Needs `mission.read`.
pub(crate) async fn item(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    Query(query): Query<ItemQuery>,
) -> Result<Response, ApiError> {
    let operation = reader(&gateway, &headers, operation_id).await?;
    let mut read = read_of(&gateway, &operation);
    read.tracker = Some(query.tracker.clone());
    read.item = Some(query.item.clone());
    let answer = match relayed(&gateway, read).await {
        Ok(answer) => answer,
        Err(response) => return Ok(*response),
    };
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let (operation_key, tracker, wanted) = (operation.to_string(), query.tracker, query.item);
    let missions: Vec<String> = tokio::task::spawn_blocking(move || {
        registry.active_issue_links().map(|links| {
            let mut missions: Vec<String> = links
                .into_iter()
                .filter(|link| {
                    link.operation == operation_key
                        && link.tracker == tracker
                        && link.item == wanted
                        && link.state == LinkState::Active
                })
                .map(|link| link.mission_id)
                .collect();
            missions.sort();
            missions.dedup();
            missions
        })
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    Ok(Json(json!({
        "item": answer.item,
        "comments": answer.comments,
        "missions": missions,
        "error": answer.error,
    }))
    .into_response())
}

/// `GET /v1/user/operations/{operation_id}/source-items?tracker=&query=`: the items the
/// Operation's sources find now, `{items, error?}`, those whose title holds `query` when given.
/// Needs `mission.read`.
pub(crate) async fn items(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
    Query(query): Query<ItemsQuery>,
) -> Result<Response, ApiError> {
    let operation = reader(&gateway, &headers, operation_id).await?;
    let mut read = read_of(&gateway, &operation);
    read.tracker = query.tracker;
    read.query = query.query;
    Ok(match relayed(&gateway, read).await {
        Ok(answer) => Json(json!({"items": answer.items, "error": answer.error})).into_response(),
        Err(response) => *response,
    })
}

#[derive(Deserialize)]
pub(crate) struct StationQuery {
    wait_ms: Option<u64>,
}

/// `GET /v1/station/source-reads/next?station=&wait_ms=`: the oldest read waiting, `204` when
/// none comes within `wait_ms`, held to `[assistant.gateway] max_wait`.
pub(crate) async fn next(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<StationQuery>,
) -> Result<Response, ApiError> {
    gateway
        .authenticate_station(bearer(&headers)?)
        .map_err(|_| ApiError::unauthorized())?;
    let most = crate::moneypenny::gateway_limits(&gateway).max_wait.0;
    let wait = Duration::from_millis(query.wait_ms.unwrap_or_default()).min(most);
    Ok(match gateway.source_reads.take(wait).await {
        Some(read) => Json(read).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

/// `POST /v1/station/source-reads/{id}/result?station=`: the Station's answer to read `id`,
/// held to `[assistant.gateway] source_answer_bytes`; `404` when nobody waits for it any more.
pub(crate) async fn result(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Result<StatusCode, ApiError> {
    gateway
        .authenticate_station(bearer(&headers)?)
        .map_err(|_| ApiError::unauthorized())?;
    let most = crate::moneypenny::gateway_limits(&gateway).source_answer_bytes;
    let bytes = axum::body::to_bytes(body, most.or_max())
        .await
        .map_err(|_| ApiError::payload_too_large())?;
    let answer: SourceReadAnswer =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::bad_request())?;
    if gateway.source_reads.answer(&id, answer) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
