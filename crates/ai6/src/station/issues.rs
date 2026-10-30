//! Linked issues on the Station: the status entries a Mission's state changes queue, and the
//! outbox the Headquarters Station drains into the trackers.
//!
//! Every write to a tracker is an outbox entry first (see
//! [`mission_issues`](crate::directorates::registry::mission_issues)). The Headquarters
//! Station alone holds tracker tokens and calls the Tracker; each call is stamped with the
//! election term, so a Station that takes over can tell what the last one left half sent.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::directorates::m::cover::Disclosure;
use crate::directorates::m::issues::{IssuePolicy, IssueUpdates, compose_status};
use crate::directorates::m::mission::MissionState;
use crate::directorates::m::roster::Roster;
use crate::directorates::registry::approvals::{ApprovalStatus, Decision};
use crate::directorates::registry::backend::*;
use crate::directorates::registry::mission_issues::{
    IssueLink, IssueReply, LinkRelation, LinkState, OutboxEntry, OutboxKind, OutboxResult,
    OutboxSettle, OutboxState, ReplyState,
};
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::directorates::registry::tracker::{
    IssueSpec, Tracker, TrackerLink, TrackerPost, TrackerUpdate,
};
use crate::station::limits::SourceLimits;
use crate::station::sources::{SourceSetup, StatusWords, source_setups};
use guatiao::{Status, library::ProviderError};

/// `blocked_reason` of a Mission waiting for an answer on its issue.
pub(crate) const ISSUE_REASON: &str = "issue";

/// Timeline kind of everything the issue machinery records.
const TIMELINE_KIND: &str = "issue";

/// Constructs the Tracker for a provider ID and an instance's settings.
pub(crate) type TrackerBuilder<'a> =
    dyn Fn(&str, &toml::Table) -> Result<Box<dyn Tracker>, String> + 'a;

/// The phase a Mission state shows on its issue, if it shows one.
pub(crate) fn phase_of(state: MissionState, blocked_reason: Option<&str>) -> Option<&'static str> {
    match state {
        MissionState::Working => Some("working"),
        MissionState::Blocked if blocked_reason == Some(ISSUE_REASON) => Some("waiting"),
        MissionState::Blocked
        | MissionState::Stalled
        | MissionState::Parked
        | MissionState::Quarantined => Some("paused"),
        MissionState::Done => Some("done"),
        MissionState::Failed | MissionState::Blown | MissionState::Abandoned => Some("failed"),
        MissionState::Queued => None,
    }
}

/// Queues the status entries a Mission entering `phase` owes its linked issues.
///
/// `updates = outcome` writes none; `milestones` writes `working`, `waiting` and `paused` on
/// the source link (the report covers its end); `all` writes every phase on every link.
/// Entries of one issue coalesce, so a burst of changes sends the last phase once.
pub(crate) fn queue_status(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    mission_id: &str,
    phase: &str,
    now: f64,
) -> usize {
    let Ok(links) = registry.issue_links(mission_id) else {
        return 0;
    };
    let ended = matches!(phase, "done" | "failed");
    let mut queued = 0;
    for link in links.iter().filter(|link| link.state == LinkState::Active) {
        let Ok(operation) = roster.operation(&link.operation) else {
            continue;
        };
        let wanted = match operation.issues.updates {
            IssueUpdates::Outcome => false,
            IssueUpdates::Milestones => link.relation == LinkRelation::Source && !ended,
            IssueUpdates::All => !(ended && link.relation == LinkRelation::Source),
        };
        if !wanted {
            continue;
        }
        let disclosure = roster
            .cover(&operation.cover)
            .map_or(Disclosure::Hidden, |cover| cover.disclosure());
        let mut entry = OutboxEntry::new(
            &link.operation,
            mission_id,
            &link.tracker,
            &link.item,
            OutboxKind::Status,
            "m",
            now,
        );
        entry.body = compose_status(disclosure, phase);
        entry.payload = serde_json::json!({ "phase": phase });
        entry.coalesce = Some(format!("status:{mission_id}:{}", link.name()));
        match registry.queue_outbox(&entry) {
            Ok(stored) => {
                queued += 1;
                audit(registry, "queued", &stored, now);
            }
            Err(error) => eprintln!(
                "ai6: status of {} on {} not queued: {error}",
                mission_id,
                link.name()
            ),
        }
    }
    queued
}

/// Queues the report of a finished source Mission as a new comment on its source issue.
pub(crate) fn queue_report(
    registry: &dyn RegistryBackend,
    link: &IssueLink,
    body: String,
    outcome: &str,
    now: f64,
) -> Result<(), String> {
    let mut entry = OutboxEntry::new(
        &link.operation,
        &link.mission_id,
        &link.tracker,
        &link.item,
        OutboxKind::Report,
        "m",
        now,
    );
    entry.body = body;
    entry.payload = serde_json::json!({ "outcome": outcome });
    entry.coalesce = Some(format!("report:{}", link.mission_id));
    let stored = registry
        .queue_outbox(&entry)
        .map_err(|error| error.to_string())?;
    audit(registry, "queued", &stored, now);
    Ok(())
}

/// Every issue setting of every Operation looser than `strict`.
pub(crate) fn weakenings(roster: &Roster) -> Vec<String> {
    roster
        .operations()
        .flat_map(|operation| {
            operation
                .issues
                .weakenings()
                .into_iter()
                .map(|line| format!("Operation {:?} issues is weakened: {line}", operation.id))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Prints [`weakenings`] on standard error at start.
pub(crate) fn log_weakenings(roster: &Roster) {
    for line in weakenings(roster) {
        eprintln!("ai6: {line}");
    }
}

/// Records, metadata only, what happened to an entry: an audit record and a timeline event.
pub(crate) fn audit(registry: &dyn RegistryBackend, what: &str, entry: &OutboxEntry, now: f64) {
    let kind = serde_json::to_value(entry.kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    let target = format!("{}:{} {}", entry.tracker, entry.item, entry.id);
    let record = AuditRecord {
        id: format!("issue-{what}-{}", uuid::Uuid::new_v4().simple()),
        actor: entry.actor.clone(),
        action: format!("issue.{kind}.{what}"),
        operation: entry.operation.clone(),
        resource: None,
        target,
        at: now,
    };
    if let Err(error) = registry.record_audit(&record) {
        eprintln!("ai6: issue audit record was not written: {error}");
    }
    let mut event = TimelineEvent::new(
        TIMELINE_KIND,
        format!("{kind}.{what}"),
        &entry.operation,
        now,
    )
    .mission(entry.mission_id.clone())
    .actor(entry.actor.clone())
    .detail("tracker", entry.tracker.clone())
    .detail("item", entry.item.clone())
    .detail("entry", entry.id.clone());
    if let Some(url) = entry.result.as_ref().and_then(|result| result.url.clone()) {
        event = event.detail("url", url);
    }
    if let Some(error) = &entry.error {
        event = event.detail("reason", error.clone());
    }
    let _ = registry.record_event(&event);
}

fn timeline_note(
    registry: &dyn RegistryBackend,
    entry: &OutboxEntry,
    action: &str,
    detail: &str,
    now: f64,
) {
    let event = TimelineEvent::new(TIMELINE_KIND, action, &entry.operation, now)
        .mission(entry.mission_id.clone())
        .detail("tracker", entry.tracker.clone())
        .detail("item", entry.item.clone())
        .detail("detail", detail.to_owned());
    let _ = registry.record_event(&event);
}

type TrackerCache = BTreeMap<(String, String), Result<Box<dyn Tracker>, String>>;

/// What one drain pass needs to remember between entries.
pub(crate) struct DrainState {
    /// When a status was last sent for an issue, by `<mission>:<tracker>:<item>`.
    pub(crate) last_status: Mutex<BTreeMap<String, f64>>,
}

impl DrainState {
    pub(crate) fn new() -> Self {
        Self {
            last_status: Mutex::new(BTreeMap::new()),
        }
    }
}

/// Reads the new comments on the issues unfinished Missions are linked to, and stores those
/// from people allowed to answer for the issue. Only the Headquarters Station does; returns how
/// many replies it stored.
///
/// Comments by AI6's own identity are skipped. A comment from anyone else is recorded as
/// ignored (the author's name on the timeline, never the text), so a poll that sees it again
/// does nothing.
pub(crate) fn poll_replies(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    build: &TrackerBuilder<'_>,
    station: &str,
    now: f64,
) -> usize {
    let Ok(Some(lease)) = registry.headquarters() else {
        return 0;
    };
    if lease.station() != station {
        return 0;
    }
    let Ok(links) = registry.active_issue_links() else {
        return 0;
    };
    let mut stored = 0;
    for operation in roster.operations() {
        let Ok(setups) = setups_of(operation) else {
            continue;
        };
        for setup in setups {
            let watched: Vec<&IssueLink> = links
                .iter()
                .filter(|link| {
                    link.operation == operation.id
                        && link.tracker == setup.name
                        && matches!(
                            link.relation,
                            LinkRelation::Source | LinkRelation::Related | LinkRelation::Child
                        )
                        && !matches!(link.phase.as_deref(), Some("done" | "failed"))
                })
                .collect();
            if watched.is_empty() {
                continue;
            }
            let mut items: Vec<String> = watched.iter().map(|link| link.item.clone()).collect();
            items.sort();
            items.dedup();
            let cursor = registry
                .issue_cursor(&operation.id, &setup.name)
                .unwrap_or_default();
            let tracker = match build(&setup.provider, &setup.settings) {
                Ok(tracker) => tracker,
                Err(error) => {
                    eprintln!(
                        "ai6: tracker {} for Operation {}: {error}",
                        setup.name, operation.id
                    );
                    continue;
                }
            };
            let batch = match tracker.replies(crate::ReplyPoll {
                items,
                since: cursor.clone(),
            }) {
                Ok(batch) => batch,
                Err(error) => {
                    eprintln!(
                        "ai6: replies of {} for Operation {}: {error}",
                        setup.name, operation.id
                    );
                    continue;
                }
            };
            for reply in batch.replies.into_iter().filter(|reply| !reply.own) {
                for link in watched.iter().filter(|link| link.item == reply.item) {
                    let mut record = IssueReply {
                        id: reply.id.clone(),
                        mission_id: link.mission_id.clone(),
                        tracker: setup.name.clone(),
                        item: reply.item.clone(),
                        author_id: reply.author_id.clone(),
                        author: reply.author.clone(),
                        body: reply.body.clone(),
                        created: reply.created.clone(),
                        state: ReplyState::Pending,
                    };
                    if !reply.permitted {
                        record.state = ReplyState::Ignored;
                        record.body = String::new();
                    }
                    match registry.store_issue_reply(&record) {
                        Ok(true) if reply.permitted => stored += 1,
                        Ok(true) => {
                            let event = TimelineEvent::new(
                                TIMELINE_KIND,
                                "reply_ignored",
                                &operation.id,
                                now,
                            )
                            .mission(link.mission_id.clone())
                            .detail("tracker", setup.name.clone())
                            .detail("item", reply.item.clone())
                            .detail("author", reply.author.clone());
                            let _ = registry.record_event(&event);
                        }
                        Ok(false) => {}
                        Err(error) => eprintln!(
                            "ai6: reply on {}:{} not stored: {error}",
                            setup.name, reply.item
                        ),
                    }
                }
            }
            if let Some(next) = batch.cursor
                && cursor.as_deref() != Some(next.as_str())
            {
                let _ = registry.set_issue_cursor(&operation.id, &setup.name, &next);
            }
        }
    }
    stored
}

fn setups_of(
    operation: &crate::directorates::m::roster::Operation,
) -> Result<Vec<SourceSetup>, ()> {
    Ok(source_setups(operation, &SourceLimits::default())
        .into_iter()
        .filter_map(Result::ok)
        .collect())
}

/// Sends the entries that are due. Only the Headquarters Station of the current term does;
/// returns how many were settled.
pub(crate) fn drain_outbox(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    build: &TrackerBuilder<'_>,
    station: &str,
    limits: &SourceLimits,
    memory: &DrainState,
    now: f64,
) -> usize {
    let Ok(Some(lease)) = registry.headquarters() else {
        return 0;
    };
    if lease.station() != station {
        return 0;
    }
    let term = lease.term();
    release_held(registry, now);
    let mut trackers = TrackerCache::new();
    let mut settled = recover_sending(registry, roster, build, &mut trackers, term, now);
    let Ok(due) = registry.due_outbox(now, limits.outbox_batch.or_max()) else {
        return settled;
    };
    for entry in due {
        let Ok(operation) = roster.operation(&entry.operation) else {
            continue;
        };
        let policy = &operation.issues;
        if entry.kind == OutboxKind::Status && !status_due(memory, &entry, policy, now) {
            continue;
        }
        if !take_tracker_quota(registry, &entry, policy, now) {
            continue;
        }
        let Ok(Some(entry)) = registry.begin_outbox_send(&entry.id, term, now) else {
            continue;
        };
        let outcome = send(registry, roster, build, &mut trackers, &entry, now);
        let settle = match outcome {
            Ok(result) => OutboxSettle::Posted(result),
            Err(failure) if failure.final_error => OutboxSettle::Refused(failure.message),
            Err(failure) => OutboxSettle::Retry {
                error: failure.message,
                not_before: now
                    + limits.retry_backoff.0.as_secs_f64()
                        * f64::from(1u32 << entry.attempts.min(10)),
                max_attempts: limits
                    .send_attempts
                    .get()
                    .map_or(u32::MAX, |most| most.min(u64::from(u32::MAX)) as u32),
            },
        };
        match registry.settle_outbox(&entry.id, term, settle) {
            Ok(done) => {
                settled += 1;
                if done.kind == OutboxKind::Status
                    && done.state == OutboxState::Posted
                    && let Ok(mut last) = memory.last_status.lock()
                {
                    last.insert(
                        format!("{}:{}:{}", done.mission_id, done.tracker, done.item),
                        now,
                    );
                }
                if done.settled() {
                    let what = match done.state {
                        OutboxState::Posted => "posted",
                        OutboxState::Refused => "refused",
                        _ => "failed",
                    };
                    audit(registry, what, &done, now);
                }
            }
            Err(error) => eprintln!("ai6: outbox entry {} not settled: {error}", entry.id),
        }
    }
    settled
}

fn status_due(memory: &DrainState, entry: &OutboxEntry, policy: &IssuePolicy, now: f64) -> bool {
    let key = format!("{}:{}:{}", entry.mission_id, entry.tracker, entry.item);
    memory
        .last_status
        .lock()
        .ok()
        .and_then(|last| last.get(&key).copied())
        .is_none_or(|at| now - at >= policy.status_interval_secs as f64)
}

/// Takes a token from the tracker instance's bucket; a refusal only delays the entry.
fn take_tracker_quota(
    registry: &dyn RegistryBackend,
    entry: &OutboxEntry,
    policy: &IssuePolicy,
    now: f64,
) -> bool {
    let Some(per_hour) = policy.tracker_posts_per_hour.get() else {
        return true;
    };
    let key = format!("issue:tracker:{}:{}", entry.operation, entry.tracker);
    let capacity = u32::try_from(per_hour).unwrap_or(u32::MAX).max(1);
    registry
        .take_quotas(&[(&key, per_hour as f64 / 3600.0, capacity)], now)
        .unwrap_or(false)
}

/// A send that did not succeed: `final_error` ones are never retried.
struct SendFailure {
    message: String,
    final_error: bool,
}

fn fail(error: ProviderError) -> SendFailure {
    let final_error = matches!(
        error.status,
        Status::GUATIAO_ERR_UNSUPPORTED | Status::GUATIAO_ERR_BAD_VALUE
    );
    SendFailure {
        message: error.to_string(),
        final_error,
    }
}

fn plain(message: impl Into<String>) -> SendFailure {
    SendFailure {
        message: message.into(),
        final_error: true,
    }
}

fn setup_of(
    roster: &Roster,
    limits: &SourceLimits,
    operation: &str,
    tracker: &str,
) -> Result<SourceSetup, String> {
    let operation = roster.operation(operation).map_err(|e| e.to_string())?;
    source_setups(operation, limits)
        .into_iter()
        .filter_map(Result::ok)
        .find(|setup| setup.name == tracker)
        .ok_or_else(|| format!("Operation {} configures no tracker {tracker}", operation.id))
}

/// Performs one entry's write.
fn send(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    build: &TrackerBuilder<'_>,
    trackers: &mut TrackerCache,
    entry: &OutboxEntry,
    now: f64,
) -> Result<OutboxResult, SendFailure> {
    let setup = setup_of(
        roster,
        &SourceLimits::default(),
        &entry.operation,
        &entry.tracker,
    )
    .map_err(plain)?;
    let tracker = trackers
        .entry((entry.operation.clone(), entry.tracker.clone()))
        .or_insert_with(|| build(&setup.provider, &setup.settings));
    let tracker = tracker
        .as_ref()
        .map_err(|error| SendFailure {
            message: error.clone(),
            final_error: false,
        })?
        .as_ref();
    let links = registry.issue_links(&entry.mission_id).unwrap_or_default();
    let link = links
        .iter()
        .find(|link| link.tracker == entry.tracker && link.item == entry.item);
    let comment = |body: &str, marker: Option<String>| {
        tracker
            .post(TrackerPost {
                item: entry.item.clone(),
                body: body.to_owned(),
                edit: None,
                marker,
            })
            .map_err(fail)
    };
    let result = |posted: crate::PostedComment| OutboxResult {
        comment_id: Some(posted.id),
        url: posted.url,
        created_item: None,
    };
    match entry.kind {
        OutboxKind::Comment | OutboxKind::Question => {
            let posted = comment(&entry.body, Some(entry.id.clone()))?;
            Ok(result(posted))
        }
        OutboxKind::Report => {
            let phase = entry.payload["outcome"]
                .as_str()
                .map_or(
                    "failed",
                    |outcome| if outcome == "done" { "done" } else { "failed" },
                );
            let posted = comment(&entry.body, Some(entry.id.clone()))?;
            apply_phase(registry, tracker, &setup, entry, phase, now)?;
            let _ = registry.set_issue_status(
                &entry.mission_id,
                &entry.tracker,
                &entry.item,
                None,
                Some(phase),
            );
            Ok(result(posted))
        }
        OutboxKind::Status => {
            let phase = entry.payload["phase"]
                .as_str()
                .unwrap_or("working")
                .to_owned();
            let link = link.ok_or_else(|| plain("the issue is no longer linked"))?;
            let mut made = OutboxResult::default();
            let seen = link.phase.as_deref() == Some(phase.as_str());
            if !(seen && link.status_comment.is_some()) {
                let capabilities = tracker.capabilities(None).map_err(fail)?;
                let posted = match (&link.status_comment, capabilities.edit_comment) {
                    (Some(id), true) => Some(
                        tracker
                            .post(TrackerPost {
                                item: entry.item.clone(),
                                body: entry.body.clone(),
                                edit: Some(id.clone()),
                                marker: Some("status".to_owned()),
                            })
                            .map_err(fail)?,
                    ),
                    (None, _) => Some(comment(&entry.body, Some("status".to_owned()))?),
                    // Editing is unsupported: a new comment only when a person must answer.
                    (Some(_), false) if phase == "waiting" => {
                        Some(comment(&entry.body, Some("status".to_owned()))?)
                    }
                    (Some(_), false) => None,
                };
                if let Some(posted) = posted {
                    made = result(posted);
                    let _ = registry.set_issue_status(
                        &entry.mission_id,
                        &entry.tracker,
                        &entry.item,
                        made.comment_id.as_deref(),
                        None,
                    );
                }
                apply_phase(registry, tracker, &setup, entry, &phase, now)?;
            }
            let _ = registry.set_issue_status(
                &entry.mission_id,
                &entry.tracker,
                &entry.item,
                None,
                Some(&phase),
            );
            Ok(made)
        }
        OutboxKind::Update => {
            tracker.update(update_of(entry)).map_err(fail)?;
            Ok(OutboxResult::default())
        }
        OutboxKind::Create => {
            let issue = tracker.create(spec_of(entry)).map_err(fail)?;
            Ok(OutboxResult {
                comment_id: None,
                url: issue.url,
                created_item: Some(issue.id),
            })
        }
        OutboxKind::Link => {
            tracker.link(link_of(entry)).map_err(fail)?;
            Ok(OutboxResult::default())
        }
    }
}

fn strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn text(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// The `TrackerUpdate` an `update` entry's payload describes.
pub(crate) fn update_of(entry: &OutboxEntry) -> TrackerUpdate {
    let payload = &entry.payload;
    TrackerUpdate {
        item: entry.item.clone(),
        add_labels: strings(&payload["add_labels"]),
        remove_labels: strings(&payload["remove_labels"]),
        transition: text(&payload["transition"]),
        add_assignees: strings(&payload["add_assignees"]),
        remove_assignees: strings(&payload["remove_assignees"]),
        state: text(&payload["state"]),
    }
}

/// The payload of an `update` entry for `update`.
pub(crate) fn update_payload(update: &TrackerUpdate) -> serde_json::Value {
    serde_json::json!({
        "add_labels": update.add_labels,
        "remove_labels": update.remove_labels,
        "transition": update.transition,
        "add_assignees": update.add_assignees,
        "remove_assignees": update.remove_assignees,
        "state": update.state,
    })
}

/// Settles the held entries whose green ink is decided: every approval given and the entry is
/// pending, any refused or expired and it is refused.
pub(crate) fn release_held(registry: &dyn RegistryBackend, now: f64) {
    let Ok(entries) = registry.outbox_entries(None) else {
        return;
    };
    for entry in entries
        .into_iter()
        .filter(|entry| entry.state == OutboxState::Held)
    {
        let Some(ids) = entry.payload["approvals"].as_array() else {
            continue;
        };
        let mut waiting = false;
        let mut refusal = None;
        for id in ids.iter().filter_map(|id| id.as_str()) {
            match registry.approval(&entry.operation, id, now) {
                Ok(Some(ApprovalStatus::Decided(approval))) => match approval.decision {
                    Decision::Approved => {}
                    Decision::Rejected => refusal = Some("green ink was refused"),
                    Decision::Expired => refusal = Some("green ink expired"),
                },
                _ => waiting = true,
            }
        }
        let settled = if let Some(reason) = refusal {
            registry.resolve_held_outbox(&entry.id, false, reason)
        } else if !waiting {
            registry.resolve_held_outbox(&entry.id, true, "")
        } else {
            continue;
        };
        if let Ok(Some(done)) = settled
            && done.state == OutboxState::Refused
        {
            audit(registry, "refused", &done, now);
        }
    }
}

/// The `IssueSpec` a `create` entry's payload describes.
pub(crate) fn spec_of(entry: &OutboxEntry) -> IssueSpec {
    let payload = &entry.payload;
    IssueSpec {
        title: text(&payload["title"]).unwrap_or_default(),
        body: entry.body.clone(),
        labels: strings(&payload["labels"]),
        issue_type: text(&payload["issue_type"]),
        priority: text(&payload["priority"]),
        parent: text(&payload["parent"]),
    }
}

/// The `TrackerLink` a `link` entry's payload describes.
pub(crate) fn link_of(entry: &OutboxEntry) -> TrackerLink {
    let payload = &entry.payload;
    TrackerLink {
        from: entry.item.clone(),
        to: text(&payload["to"]),
        relation: text(&payload["relation"]).unwrap_or_else(|| "related".to_owned()),
        url: text(&payload["url"]),
    }
}

/// Swaps the status labels and makes the status transition for `phase` on the entry's issue.
///
/// A transition the issue does not offer is a timeline event, not a retry.
fn apply_phase(
    registry: &dyn RegistryBackend,
    tracker: &dyn Tracker,
    setup: &SourceSetup,
    entry: &OutboxEntry,
    phase: &str,
    now: f64,
) -> Result<(), SendFailure> {
    let wanted = setup.status_labels.for_phase(phase).map(str::to_owned);
    let remove_labels: Vec<String> = setup
        .status_labels
        .all()
        .into_iter()
        .filter(|label| Some(*label) != wanted.as_deref())
        .map(str::to_owned)
        .collect();
    let mut update = TrackerUpdate {
        item: entry.item.clone(),
        add_labels: wanted.into_iter().collect(),
        remove_labels: if setup.status_labels == StatusWords::default() {
            Vec::new()
        } else {
            remove_labels
        },
        ..TrackerUpdate::default()
    };
    if let Some(name) = setup.status_transitions.for_phase(phase) {
        let offered = tracker
            .capabilities(Some(entry.item.clone()))
            .map_err(fail)?
            .transitions;
        if offered.iter().any(|offer| offer.eq_ignore_ascii_case(name)) {
            update.transition = Some(name.to_owned());
        } else {
            timeline_note(registry, entry, "transition_unavailable", name, now);
        }
    }
    if update.add_labels.is_empty()
        && update.remove_labels.is_empty()
        && update.transition.is_none()
    {
        return Ok(());
    }
    tracker.update(update).map_err(fail)
}

/// Settles what a previous term left `sending`: an entry whose comment is already on the
/// issue is recorded as posted, the rest go back to pending. A `create` is refused instead,
/// since sending it again could make a second issue.
fn recover_sending(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    build: &TrackerBuilder<'_>,
    trackers: &mut TrackerCache,
    term: u64,
    now: f64,
) -> usize {
    let Ok(left) = registry.sending_outbox() else {
        return 0;
    };
    let mut settled = 0;
    for entry in left.into_iter().filter(|entry| entry.fence != Some(term)) {
        let Ok(Some(entry)) = registry.adopt_outbox_send(&entry.id, term) else {
            continue;
        };
        let settle = recovered(roster, build, trackers, &entry);
        if let Ok(done) = registry.settle_outbox(&entry.id, term, settle) {
            settled += 1;
            timeline_note(
                registry,
                &done,
                "recovered",
                "a new Headquarters term settled it",
                now,
            );
        }
    }
    settled
}

fn recovered(
    roster: &Roster,
    build: &TrackerBuilder<'_>,
    trackers: &mut TrackerCache,
    entry: &OutboxEntry,
) -> OutboxSettle {
    if entry.kind == OutboxKind::Create {
        return OutboxSettle::Refused(
            "interrupted by a Headquarters change; check the tracker before creating it again"
                .to_owned(),
        );
    }
    let again = || OutboxSettle::Retry {
        error: "interrupted by a Headquarters change".to_owned(),
        not_before: 0.0,
        max_attempts: u32::MAX,
    };
    let posts_comment = matches!(
        entry.kind,
        OutboxKind::Comment | OutboxKind::Question | OutboxKind::Report | OutboxKind::Status
    );
    if !posts_comment {
        return again();
    }
    let Ok(setup) = setup_of(
        roster,
        &SourceLimits::default(),
        &entry.operation,
        &entry.tracker,
    ) else {
        return again();
    };
    let tracker = trackers
        .entry((entry.operation.clone(), entry.tracker.clone()))
        .or_insert_with(|| build(&setup.provider, &setup.settings));
    let Ok(tracker) = tracker.as_ref() else {
        return again();
    };
    let Ok(batch) = tracker.replies(crate::ReplyPoll {
        items: vec![entry.item.clone()],
        since: None,
    }) else {
        return again();
    };
    let marker = format!("ai6:{}", entry.id);
    let status_marker = "ai6:status";
    let found = batch.replies.iter().rev().find(|reply| {
        reply.own
            && (reply.body.contains(&marker)
                || (entry.kind == OutboxKind::Status && reply.body.contains(status_marker))
                || reply.body.trim() == entry.body.trim())
    });
    match found {
        Some(reply) => OutboxSettle::Posted(OutboxResult {
            comment_id: Some(reply.id.clone()),
            url: None,
            created_item: None,
        }),
        None => again(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mission_states_map_to_the_phases_an_issue_shows() {
        assert_eq!(phase_of(MissionState::Working, None), Some("working"));
        assert_eq!(
            phase_of(MissionState::Blocked, Some(ISSUE_REASON)),
            Some("waiting")
        );
        for paused in [
            phase_of(MissionState::Blocked, Some("green-ink")),
            phase_of(MissionState::Blocked, None),
            phase_of(MissionState::Stalled, None),
            phase_of(MissionState::Parked, None),
            phase_of(MissionState::Quarantined, None),
        ] {
            assert_eq!(paused, Some("paused"));
        }
        assert_eq!(phase_of(MissionState::Done, None), Some("done"));
        for failed in [
            MissionState::Failed,
            MissionState::Blown,
            MissionState::Abandoned,
        ] {
            assert_eq!(phase_of(failed, None), Some("failed"));
        }
        assert_eq!(phase_of(MissionState::Queued, None), None);
    }
}
