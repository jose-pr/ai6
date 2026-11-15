//! Searching Missions: `GET /v1/missions/search`.
//!
//! The Registry holds every brief in whatever state it is, so a Mission is found whether or not
//! anything was published for it. A search filters on the Operation, Agent, state, a piece of
//! the handle (a former handle written whole finds it too), a tracker item's key, and words
//! that must each appear in the brief's title, name or body or in its outcome. The answer
//! is newest first and holds only what the person may read: `mission.read` on the Mission's
//! Operation, or a compartment giving it on that Mission (the crossing audited before it
//! is shown). Anything else is left out as if it did not exist.
//!
//! How many Missions a page holds, and how many briefs one search reads, are the `[search]`
//! table of the `unex` record.

use std::collections::HashMap;

use ai6::directorates::registry::store::BriefSearch;
use ai6::{Authorizer as _, CompartmentAction, FiledBrief};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::tree::operation_request;
use crate::{ApiError, Gateway, bearer, operation_tree, resolve_user};

/// What the query string may carry.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchQuery {
    /// Only this Operation's Missions.
    operation: Option<String>,
    /// Only Missions assigned this Agent.
    agent: Option<String>,
    /// `new`, `held`, `claimed`, `done` or `failed`.
    state: Option<String>,
    /// A piece of a handle.
    handle: Option<String>,
    /// A tracker item's key, its number or its code.
    ticket: Option<String>,
    /// Words that must each appear in the brief or the outcome.
    text: Option<String>,
    /// How many Missions to return; the `[search]` page by default.
    limit: Option<usize>,
}

/// One Mission a search found.
#[derive(Debug, Serialize)]
pub(crate) struct Found {
    mission_id: String,
    handle: String,
    operation_id: String,
    /// Where its brief stands in the Registry.
    state: String,
    agent: Option<String>,
    role: Option<String>,
    title: String,
    name: Option<String>,
    url: Option<String>,
    /// The tracker item it came from or follows up, as the Source names it.
    ticket: Option<String>,
    created: String,
    /// How many times a Station claimed it.
    attempts: u32,
}

/// The answer: a page of Missions, and whether more matched.
#[derive(Debug, Serialize)]
pub(crate) struct Page {
    missions: Vec<Found>,
    /// Whether more Missions matched than this page holds.
    more: bool,
}

const STATES: [&str; 5] = ["new", "held", "claimed", "done", "failed"];

/// A filter that is text: not empty, short, one line.
fn filter(value: Option<&String>, longest: usize) -> Result<Option<String>, ApiError> {
    match value.map(|text| text.trim()) {
        None => Ok(None),
        Some(text)
            if text.is_empty() || text.len() > longest || text.chars().any(char::is_control) =>
        {
            Err(ApiError::bad_request())
        }
        Some(text) => Ok(Some(text.to_owned())),
    }
}

fn found_of(filed: &FiledBrief) -> Found {
    let brief = &filed.brief;
    let ticket = brief
        .data
        .get("source")
        .or_else(|| brief.data.get("follow_up"))
        .and_then(|source| source.get("item"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Found {
        mission_id: brief.key(),
        handle: brief.shown_handle(),
        operation_id: brief.operation.clone().unwrap_or_default(),
        state: filed.state.clone(),
        agent: brief.agent.clone(),
        role: brief.role.clone(),
        title: brief.title.clone(),
        name: brief.name.clone(),
        url: brief.url.clone(),
        ticket,
        created: brief.created.clone(),
        attempts: filed.runs,
    }
}

/// `GET /v1/missions/search`.
pub(crate) async fn search(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Page>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let pages = gateway.config.unex().search.clone();
    let limit = query
        .limit
        .unwrap_or_else(|| usize::try_from(pages.page).unwrap_or(usize::MAX));
    if limit == 0 || !pages.max_page.allows(limit) {
        return Err(ApiError::bad_request());
    }
    // Every filter is held to the one bound on text.
    let longest = pages.filter_bytes.or_max();
    let mut request = BriefSearch::default();
    request.operation = filter(query.operation.as_ref(), longest)?;
    request.agent = filter(query.agent.as_ref(), longest)?;
    request.handle = filter(query.handle.as_ref(), longest)?;
    request.ticket = filter(query.ticket.as_ref(), longest)?;
    request.text = filter(query.text.as_ref(), longest)?;
    request.state = filter(query.state.as_ref(), longest)?;
    if request
        .state
        .as_deref()
        .is_some_and(|state| !STATES.contains(&state))
    {
        return Err(ApiError::bad_request());
    }
    request.scan = pages.scan;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let authorizer = gateway.authorizer()?;
    let tree = operation_tree(&gateway).await?;
    let page = tokio::task::spawn_blocking(move || {
        let candidates = registry.search_briefs(&request, usize::MAX)?;
        let mut readable = HashMap::<String, bool>::new();
        let mut missions = Vec::new();
        let mut more = false;
        for filed in candidates {
            let Some(operation) = filed.brief.operation.clone() else {
                continue;
            };
            let allowed = *readable.entry(operation.clone()).or_insert_with(|| {
                authorizer
                    .authorize(operation_request(
                        person.clone(),
                        "mission.read",
                        &operation,
                        &tree,
                    ))
                    .is_ok_and(|decision| decision.allowed)
            });
            // A compartment may cover this Mission alone, not its Operation.
            if !allowed
                && !authorizer.allows_mission(
                    &person,
                    CompartmentAction::MissionRead,
                    &filed.brief.key(),
                    &operation,
                )
            {
                continue;
            }
            if missions.len() == limit {
                more = true;
                break;
            }
            missions.push(found_of(&filed));
        }
        Ok::<_, std::io::Error>(Page { missions, more })
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    Ok(Json(page))
}
