//! The attention summary: what needs the signed-in person, read once and said the same way to
//! the Overview page and to Moneypenny.
//!
//! Every list in it is read through the routes the app already uses, with the person's own
//! token, so each read is authorized and audited as that route does it, compartment crossings
//! included. Nothing is kept: the summary is derived on each request.
//!
//! Each item says whether its condition is `current`, `historical` (it happened and has since
//! passed) or `unverified` (it was seen, and the check that would say whether it still holds
//! could not be made). A source that could not be read is named in `coverage`; the summary
//! never says everything is clear while one is missing.

use std::collections::BTreeSet;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::moneypenny::routes::{Limits, Route};
use crate::{
    ApiError, BoardMission, BoardStation, Gateway, MissionsQuery, StationBoard, UserAuditQuery,
    bearer, green_ink, held, resolve_user, unix_timestamp,
};

/// The audit action of a Station refusing a configuration.
const REFUSED_ACTION: &str = "config.refused";

/// `GET /v1/user/attention` filters.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttentionQuery {
    /// Only this Operation, and the Stations that host it.
    operation_id: Option<String>,
}

/// What a condition's state is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Condition {
    /// It holds now, as of the check.
    Current,
    /// It happened and has since passed.
    Historical,
    /// It was seen, and whether it still holds could not be checked.
    Unverified,
}

/// The thing an item is about, with the route that opens it.
#[derive(Debug, Clone, Serialize)]
struct Entity {
    /// `run`, `mission`, `station`, `approval`, `brief` or `proposal`.
    kind: &'static str,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    station_id: Option<String>,
}

/// One thing that needs the person, or did.
#[derive(Debug, Clone, Serialize)]
struct Item {
    /// Stable for the same condition, so a page can keep its place.
    id: String,
    /// `blocked_run`, `green_ink`, `held_brief`, `mission_proposal`, `station_dark` or
    /// `config_refused`.
    kind: &'static str,
    condition: Condition,
    /// One line, with identifiers; untrusted text in it is stripped of control characters.
    title: String,
    /// Why the condition is what it is, when there is more to say.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    entity: Entity,
    /// The app page that deals with it: `{"page": ..., ...}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<Value>,
    /// The same page as `app:/<page>?...`, for a link in text.
    #[serde(skip_serializing_if = "Option::is_none")]
    link: Option<String>,
    /// What was checked for this item: `operation:G` or `station:NAME`.
    scope: String,
    /// When the gateway read its source, as Unix seconds.
    checked_at: f64,
    /// When the Station reported what the item rests on, when it is a Station fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_at: Option<f64>,
    /// When the condition began, as Unix seconds, when the data says.
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<f64>,
}

/// How one source was read.
#[derive(Debug, Clone, Default, Serialize)]
struct Coverage {
    source: &'static str,
    /// What the person calls it.
    label: &'static str,
    /// `checked`, `partial`, `not_permitted` or `unavailable`.
    status: &'static str,
    /// The scopes read: `operation:G`, `station:NAME` or `all`.
    checked: Vec<String>,
    /// Scopes the person may not read this from; not a gap, since it is theirs to see or not.
    not_permitted: Vec<String>,
    /// Scopes that could not be read now: a gap.
    unavailable: Vec<String>,
    checked_at: f64,
}

impl Coverage {
    fn new(source: &'static str, label: &'static str, now: f64) -> Self {
        Self {
            source,
            label,
            checked_at: now,
            ..Self::default()
        }
    }

    fn settle(mut self) -> Self {
        self.status = match (
            self.checked.is_empty(),
            self.not_permitted.is_empty() && self.unavailable.is_empty(),
        ) {
            (_, true) => "checked",
            (false, false) => "partial",
            (true, false) if self.unavailable.is_empty() => "not_permitted",
            (true, false) => "unavailable",
        };
        self
    }
}

/// How a read ended.
enum Read<T> {
    Done(T),
    NotPermitted,
    Unavailable,
}

impl<T> Read<T> {
    fn of(result: Result<T, ApiError>) -> Self {
        match result {
            Ok(value) => Self::Done(value),
            Err(error) if error.status == StatusCode::FORBIDDEN => Self::NotPermitted,
            Err(_) => Self::Unavailable,
        }
    }
}

/// Which Operations and Stations a summary covers.
#[derive(Debug, Serialize)]
struct Scope {
    /// `all` (every Operation the person may read) or `operation`.
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    /// The Operations read, as the person may read them.
    operations: Vec<String>,
    /// The Stations read.
    stations: Vec<String>,
    /// In words: "All accessible Operations" or "Operation G".
    label: String,
}

/// The whole answer.
#[derive(Debug, Serialize)]
pub(crate) struct Summary {
    /// When the gateway made the checks, as Unix seconds.
    checked_at: f64,
    scope: Scope,
    /// Current and unverified items first, historical last, each newest first.
    items: Vec<Item>,
    /// Items found past the configured count, left out.
    omitted: usize,
    coverage: Vec<Coverage>,
    /// Whether every source was read: only then is "nothing needs attention" a fact.
    complete: bool,
    /// Whether nothing is current or unverified and the checks were complete.
    clear: bool,
    /// Current items.
    current: usize,
    /// Items seen whose present state could not be checked.
    unverified: usize,
    /// Conditions that have passed.
    historical: usize,
    /// The answer in a sentence, saying what was and was not checked.
    headline: String,
}

/// `GET /v1/user/attention`.
pub(crate) async fn read(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<AttentionQuery>,
) -> Result<Json<Summary>, ApiError> {
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

    let mut coverage = Vec::new();
    let mut found: Vec<Item> = Vec::new();

    // Stations, and with them which Operations the person may read.
    let mut stations_read = Coverage::new("stations", "Stations", now);
    let board = match Read::of(
        crate::stations(State(gateway.clone()), headers.clone())
            .await
            .map(|Json(board)| board),
    ) {
        Read::Done(board) => {
            stations_read.checked.push("all".to_owned());
            board
        }
        Read::NotPermitted => {
            stations_read.not_permitted.push("all".to_owned());
            empty_board()
        }
        Read::Unavailable => {
            stations_read.unavailable.push("all".to_owned());
            empty_board()
        }
    };
    let stations: Vec<&BoardStation> = board
        .stations
        .iter()
        .filter(|station| {
            wanted
                .as_ref()
                .is_none_or(|operation| station.operations.contains(operation))
        })
        .collect();

    // Runs: the Mission list, which also names each Mission.
    let mut runs_read = Coverage::new("runs", "Runs", now);
    let missions: Vec<BoardMission> = match Read::of(
        crate::missions(
            State(gateway.clone()),
            headers.clone(),
            Query(MissionsQuery {
                operation: wanted.clone(),
                ..MissionsQuery::default()
            }),
        )
        .await
        .map(|Json(missions)| missions),
    ) {
        Read::Done(missions) => {
            runs_read.checked.push(scope_of(wanted.as_deref()));
            missions
        }
        Read::NotPermitted => {
            runs_read.not_permitted.push(scope_of(wanted.as_deref()));
            Vec::new()
        }
        Read::Unavailable => {
            runs_read.unavailable.push(scope_of(wanted.as_deref()));
            Vec::new()
        }
    };

    // The Operations to look in: those the person may read, or the one asked for.
    let mut operations: BTreeSet<String> = board
        .operations
        .iter()
        .map(|operation| operation.operation_id.clone())
        .collect();
    operations.extend(missions.iter().map(|mission| mission.operation_id.clone()));
    if let Some(operation) = &wanted {
        operations.retain(|candidate| candidate == operation);
        if board.operations.is_empty() {
            operations.insert(operation.clone());
        }
    }

    // What waits for a decision, per Operation.
    let mut green_read = Coverage::new("green_ink", "Green ink requests", now);
    let mut held_read = Coverage::new("held_briefs", "Held briefs", now);
    let mut proposals_read = Coverage::new("mission_proposals", "Mission proposals", now);
    let mut asked: BTreeSet<(String, String)> = BTreeSet::new();
    for operation in &operations {
        let scope = format!("operation:{operation}");
        let (requests, briefs, proposals) = tokio::join!(
            green_ink::pending(
                State(gateway.clone()),
                headers.clone(),
                Path(operation.clone())
            ),
            held::list(
                State(gateway.clone()),
                headers.clone(),
                Path(operation.clone())
            ),
            crate::list_user_mission_proposals(
                State(gateway.clone()),
                headers.clone(),
                Path(operation.clone())
            ),
        );
        match Read::of(requests) {
            Read::Done(Json(requests)) => {
                green_read.checked.push(scope.clone());
                for request in requests {
                    asked.insert((operation.clone(), request.run.to_string()));
                    let (title, entity) = (
                        format!(
                            "Agent {} waits for green ink to {} {} in Operation {operation}",
                            plain(&request.agent),
                            plain(&request.scope.kind),
                            plain(&request.scope.target)
                        ),
                        Entity {
                            kind: "approval",
                            id: request.id.clone(),
                            operation_id: Some(operation.clone()),
                            station_id: None,
                        },
                    );
                    found.push(item(
                        format!("green_ink:{operation}:{}", request.id),
                        "green_ink",
                        Condition::Current,
                        title,
                        None,
                        entity,
                        json!({"page": "green_ink", "operation_id": operation, "request_id": request.id}),
                        &scope,
                        now,
                        None,
                        Some(request.requested_at),
                    ));
                }
            }
            Read::NotPermitted => green_read.not_permitted.push(scope.clone()),
            Read::Unavailable => green_read.unavailable.push(scope.clone()),
        }
        match Read::of(briefs) {
            Read::Done(Json(briefs)) => {
                held_read.checked.push(scope.clone());
                for entry in briefs {
                    let entity = Entity {
                        kind: "brief",
                        id: entry.key.clone(),
                        operation_id: Some(operation.clone()),
                        station_id: None,
                    };
                    found.push(item(
                        format!("held_brief:{operation}:{}", entry.key),
                        "held_brief",
                        Condition::Current,
                        format!(
                            "Brief \"{}\" is held for review in Operation {operation}",
                            plain(&entry.held.brief.title)
                        ),
                        None,
                        entity,
                        json!({"page": "briefings", "tab": "held", "operation_id": operation, "item_id": entry.key}),
                        &scope,
                        now,
                        None,
                        Some(entry.held.held_at),
                    ));
                }
            }
            Read::NotPermitted => held_read.not_permitted.push(scope.clone()),
            Read::Unavailable => held_read.unavailable.push(scope.clone()),
        }
        match Read::of(proposals) {
            Read::Done(Json(proposals)) => {
                proposals_read.checked.push(scope.clone());
                for proposal in proposals {
                    let entity = Entity {
                        kind: "proposal",
                        id: proposal.id.clone(),
                        operation_id: Some(operation.clone()),
                        station_id: None,
                    };
                    found.push(item(
                        format!("mission_proposal:{operation}:{}", proposal.id),
                        "mission_proposal",
                        Condition::Current,
                        format!(
                            "Mission proposal \"{}\" waits for a decision in Operation {operation}",
                            plain(&proposal.brief.title)
                        ),
                        None,
                        entity,
                        json!({"page": "briefings", "tab": "proposals", "operation_id": operation, "item_id": proposal.id}),
                        &scope,
                        now,
                        None,
                        Some(proposal.created_at),
                    ));
                }
            }
            Read::NotPermitted => proposals_read.not_permitted.push(scope.clone()),
            Read::Unavailable => proposals_read.unavailable.push(scope.clone()),
        }
    }

    // Blocked Runs; one held for green ink is the request waiting for it, not a second item.
    for mission in &missions {
        for run in &mission.runs {
            if run.state.as_deref() != Some("blocked") {
                continue;
            }
            let green = run.reason.as_deref() == Some("green-ink");
            if green && asked.contains(&(mission.operation_id.clone(), run.run_id.clone())) {
                continue;
            }
            let handle = plain(&mission.handle);
            let title = if green {
                format!("Run {} of Mission {handle} waits for green ink", run.run_id)
            } else {
                format!(
                    "Run {} of Mission {handle} is blocked{}",
                    run.run_id,
                    run.reason
                        .as_deref()
                        .map(|reason| format!(": {}", plain(reason)))
                        .unwrap_or_default()
                )
            };
            let route = if green {
                json!({"page": "green_ink", "operation_id": mission.operation_id})
            } else {
                json!({"page": "missions", "operation_id": mission.operation_id,
                       "mission_id": mission.mission_id})
            };
            let entity = Entity {
                kind: "run",
                id: run.run_id.clone(),
                operation_id: Some(mission.operation_id.clone()),
                station_id: run.station_id.clone(),
            };
            found.push(item(
                format!("blocked_run:{}:{}", mission.operation_id, run.run_id),
                "blocked_run",
                Condition::Current,
                title,
                None,
                entity,
                route,
                &format!("operation:{}", mission.operation_id),
                now,
                None,
                run.updated_at,
            ));
        }
    }

    // Stations: dark ones, and the configuration each runs.
    let mut audit_read = Coverage::new("audit", "Configuration refusal history", now);
    for station in &stations {
        let scope = format!("station:{}", station.station_id);
        // This Station's audit log, which the person may or may not read.
        let refusals = match Read::of(
            crate::user_audit(
                State(gateway.clone()),
                headers.clone(),
                Query(UserAuditQuery {
                    operation_id: None,
                    station_id: Some(station.station_id.clone()),
                    limit: Some(limits.max_audit_page.or_max()),
                }),
            )
            .await
            .map(|Json(records)| records),
        ) {
            Read::Done(records) => {
                audit_read.checked.push(scope.clone());
                records
            }
            Read::NotPermitted => {
                audit_read.not_permitted.push(scope.clone());
                Vec::new()
            }
            Read::Unavailable => {
                audit_read.unavailable.push(scope.clone());
                Vec::new()
            }
        };
        let entity = Entity {
            kind: "station",
            id: station.station_id.clone(),
            operation_id: None,
            station_id: Some(station.station_id.clone()),
        };
        let route = json!({"page": "stations", "station_id": station.station_id});
        let observed = (station.seen > 0.0).then_some(station.seen);
        if !station.healthy {
            found.push(item(
                format!("station_dark:{}", station.station_id),
                "station_dark",
                Condition::Current,
                format!("Station {} has gone dark", station.station_id),
                None,
                entity.clone(),
                route.clone(),
                &scope,
                now,
                observed,
                observed,
            ));
        }
        let config = station.config.as_ref();
        let current: Vec<&String> = config
            .map(|config| config.refused.iter().collect())
            .unwrap_or_default();
        let running = config.and_then(|config| config.revision.clone());
        let settings =
            json!({"page": "stations", "station_id": station.station_id, "tab": "settings"});
        let mut seen_now: BTreeSet<String> = BTreeSet::new();
        // A Station that is dark reported these when it last could; whether they still hold is
        // not known.
        let condition = if station.healthy {
            Condition::Current
        } else {
            Condition::Unverified
        };
        for reason in &current {
            let key = refusal_key(reason);
            seen_now.insert(key.clone());
            found.push(item(
                format!("config_refused:{}:{key}", station.station_id),
                "config_refused",
                condition,
                format!(
                    "Station {} refused a configuration{} and keeps running {}",
                    station.station_id,
                    revision_of(reason)
                        .map(|revision| format!(" (revision {revision})"))
                        .unwrap_or_default(),
                    running
                        .as_deref()
                        .map(|revision| format!("revision {revision}"))
                        .unwrap_or_else(|| "the last good one".to_owned())
                ),
                Some(plain(reason)),
                entity.clone(),
                settings.clone(),
                &scope,
                now,
                observed,
                None,
            ));
        }
        // Past refusals, from the audit log, against what the Station reports now.
        let mut shown: BTreeSet<String> = BTreeSet::new();
        for record in refusals.iter().map(|entry| &entry.record).filter(|record| {
            record.action == REFUSED_ACTION
                && record.resource.as_deref() == Some(scope.as_str())
                && limits
                    .attention_history
                    .allows_secs((now - record.at).max(0.0))
        }) {
            let key = refusal_key(&record.target);
            if seen_now.contains(&key) || !shown.insert(key.clone()) {
                continue;
            }
            let (condition, detail) = history_of(station, config.is_some(), running.as_deref());
            found.push(item(
                format!("config_refused:{}:{key}", station.station_id),
                "config_refused",
                condition,
                format!(
                    "Station {} refused a configuration{}",
                    station.station_id,
                    revision_of(&record.target)
                        .map(|revision| format!(" (revision {revision})"))
                        .unwrap_or_default()
                ),
                Some(format!("{} {}", plain(&record.target), detail)),
                entity.clone(),
                settings.clone(),
                &scope,
                now,
                observed,
                Some(record.at),
            ));
        }
    }

    for read in [
        stations_read,
        runs_read,
        green_read,
        held_read,
        proposals_read,
        audit_read,
    ] {
        coverage.push(read.settle());
    }

    found.sort_by(|left, right| {
        rank(left.condition)
            .cmp(&rank(right.condition))
            .then(
                right
                    .since
                    .unwrap_or(0.0)
                    .total_cmp(&left.since.unwrap_or(0.0)),
            )
            .then(left.id.cmp(&right.id))
    });
    let counts = |condition| {
        found
            .iter()
            .filter(|item| item.condition == condition)
            .count()
    };
    let (current, unverified, historical) = (
        counts(Condition::Current),
        counts(Condition::Unverified),
        counts(Condition::Historical),
    );
    let mut omitted = 0;
    if !limits.attention_items.allows(found.len()) {
        let keep = limits.attention_items.or_max();
        omitted = found.len() - keep;
        found.truncate(keep);
    }
    let scope = Scope {
        kind: if wanted.is_some() { "operation" } else { "all" },
        label: wanted.as_ref().map_or_else(
            || "All accessible Operations".to_owned(),
            |operation| format!("Operation {operation}"),
        ),
        operation_id: wanted,
        operations: operations.into_iter().collect(),
        stations: stations
            .iter()
            .map(|station| station.station_id.clone())
            .collect(),
    };
    let gaps: Vec<String> = coverage
        .iter()
        .filter(|read| !read.unavailable.is_empty())
        .map(|read| {
            if read.unavailable.iter().all(|scope| scope == "all") {
                read.label.to_owned()
            } else {
                format!("{} ({})", read.label, read.unavailable.join(", "))
            }
        })
        .collect();
    let complete = gaps.is_empty();
    let clear = complete && current == 0 && unverified == 0;
    let headline = headline(
        &scope.label,
        current,
        unverified,
        historical,
        omitted,
        &gaps,
        &coverage,
    );
    Ok(Json(Summary {
        checked_at: now,
        scope,
        items: found,
        omitted,
        coverage,
        complete,
        clear,
        current,
        unverified,
        historical,
        headline,
    }))
}

fn empty_board() -> StationBoard {
    StationBoard {
        stations: Vec::new(),
        headquarters_station: None,
        operations: Vec::new(),
    }
}

fn scope_of(operation: Option<&str>) -> String {
    operation.map_or_else(
        || "all".to_owned(),
        |operation| format!("operation:{operation}"),
    )
}

fn rank(condition: Condition) -> u8 {
    match condition {
        Condition::Current => 0,
        Condition::Unverified => 1,
        Condition::Historical => 2,
    }
}

#[allow(clippy::too_many_arguments)]
fn item(
    id: String,
    kind: &'static str,
    condition: Condition,
    title: String,
    detail: Option<String>,
    entity: Entity,
    route: Value,
    scope: &str,
    checked_at: f64,
    observed_at: Option<f64>,
    since: Option<f64>,
) -> Item {
    // A value a route does not take (an identifier from outside AI6) opens the page alone.
    let checked = Route::from_json(&route, Limits::default()).or_else(|_| {
        let page = route.get("page").cloned().unwrap_or(Value::Null);
        Route::from_json(&json!({"page": page}), Limits::default())
    });
    let (route, link) = match checked {
        Ok(route) => (Some(route.to_json()), Some(route.to_uri())),
        Err(_) => (None, None),
    };
    Item {
        id,
        kind,
        condition,
        title,
        detail,
        entity,
        route,
        link,
        scope: scope.to_owned(),
        checked_at,
        observed_at,
        since,
    }
}

/// Text from a record or an Agent on one line, with nothing that renders as nothing.
fn plain(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// The revision a refusal names: the Station words it `configuration revision N: why`.
fn revision_of(reason: &str) -> Option<&str> {
    let rest = reason.strip_prefix("configuration revision ")?;
    let (revision, _) = rest.split_once(':')?;
    (!revision.is_empty() && revision.chars().all(|c| c.is_ascii_digit())).then_some(revision)
}

/// What identifies a refusal across the Station's report and its audit record (which cuts and
/// flattens the text): the revision when it names one, else the text.
fn refusal_key(reason: &str) -> String {
    revision_of(reason).map_or_else(|| plain(reason), str::to_owned)
}

/// Whether a refusal in the audit log still holds, from what the Station reports now.
fn history_of(
    station: &BoardStation,
    reports_config: bool,
    running: Option<&str>,
) -> (Condition, String) {
    if !station.healthy {
        return (
            Condition::Unverified,
            "(the Station is dark, so whether it still refuses could not be checked)".to_owned(),
        );
    }
    if !reports_config {
        return (
            Condition::Unverified,
            "(the Station does not report its configuration, so whether it still refuses could \
             not be checked)"
                .to_owned(),
        );
    }
    (
        Condition::Historical,
        running.map_or_else(
            || "(the Station now reports no refusal)".to_owned(),
            |revision| format!("(the Station now runs revision {revision} and reports no refusal)"),
        ),
    )
}

fn headline(
    scope: &str,
    current: usize,
    unverified: usize,
    historical: usize,
    omitted: usize,
    gaps: &[String],
    coverage: &[Coverage],
) -> String {
    let mut parts = Vec::new();
    if current == 0 && unverified == 0 && gaps.is_empty() {
        parts.push(format!("Nothing needs attention now in {scope}."));
    } else if current == 0 && unverified == 0 {
        parts.push(format!(
            "Nothing found that needs attention in {scope}, from what could be read."
        ));
    } else {
        let mut said = Vec::new();
        if current > 0 {
            said.push(format!(
                "{current} {} attention now",
                if current == 1 {
                    "item needs"
                } else {
                    "items need"
                }
            ));
        }
        if unverified > 0 {
            said.push(format!("{unverified} could not be confirmed as current"));
        }
        parts.push(format!("In {scope}: {}.", said.join("; ")));
    }
    if historical > 0 {
        parts.push(format!(
            "{historical} past {} listed as historical, not current.",
            if historical == 1 {
                "condition is"
            } else {
                "conditions are"
            }
        ));
    }
    if omitted > 0 {
        parts.push(format!("{omitted} more were found than the summary lists."));
    }
    if !gaps.is_empty() {
        parts.push(format!("Could not be checked: {}.", gaps.join(", ")));
    }
    let hidden: Vec<&str> = coverage
        .iter()
        .filter(|read| !read.not_permitted.is_empty())
        .map(|read| read.label)
        .collect();
    if !hidden.is_empty() {
        parts.push(format!(
            "Not visible to you, so not included: {}.",
            hidden.join(", ")
        ));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gap(label: &'static str) -> Coverage {
        Coverage {
            unavailable: vec!["all".to_owned()],
            ..Coverage::new("stations", label, 0.0)
        }
        .settle()
    }

    #[test]
    fn the_headline_never_says_clear_while_a_source_is_missing() {
        let said = headline(
            "All",
            0,
            0,
            0,
            0,
            &["Stations".to_owned()],
            &[gap("Stations")],
        );
        assert!(!said.contains("Nothing needs attention now"), "{said}");
        assert!(said.contains("Could not be checked: Stations."), "{said}");
        let clear = headline("All", 0, 0, 0, 0, &[], &[]);
        assert_eq!(clear, "Nothing needs attention now in All.");
    }

    #[test]
    fn a_source_is_unavailable_only_when_nothing_of_it_was_read() {
        assert_eq!(gap("Stations").status, "unavailable");
        let mut some = gap("Stations");
        some.checked.push("operation:G".to_owned());
        assert_eq!(some.settle().status, "partial");
        let mut hidden = Coverage::new("audit", "Audit", 0.0);
        hidden.not_permitted.push("all".to_owned());
        assert_eq!(hidden.settle().status, "not_permitted");
    }

    #[test]
    fn a_refusal_is_known_by_its_revision_in_the_report_and_in_the_audit_text() {
        let reported = "configuration revision 78: the roster does not parse";
        let audited = "configuration revision 78: the roster does\nnot parse";
        assert_eq!(refusal_key(reported), refusal_key(audited));
        assert_eq!(revision_of(reported), Some("78"));
        assert_eq!(revision_of("configuration revision x: no"), None);
        assert_eq!(refusal_key("it broke"), "it broke");
    }
}
