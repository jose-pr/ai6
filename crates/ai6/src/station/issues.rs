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
    IssueLink, IssueProposal, IssueReply, IssueSnapshot, LinkRelation, LinkState, OutboxEntry,
    OutboxKind, OutboxResult, OutboxSettle, OutboxState, ProposalState, ProposedIssue, ReplyState,
};
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::directorates::registry::tracker::{
    IssueSpec, Tracker, TrackerLink, TrackerPost, TrackerUpdate,
};
use crate::station::limits::SourceLimits;
use crate::station::sources::{Reopen, SourceSetup, StatusWords, source_setups};
use guatiao::{Status, library::ProviderError};

/// `blocked_reason` of a Mission waiting for an answer on its issue.
pub(crate) const ISSUE_REASON: &str = "issue";

/// The Mission an issue a person created for no Mission is held under, in the gateway's words.
const NO_MISSION: &str = "operation-created";

/// Timeline kind of everything the issue machinery records.
const TIMELINE_KIND: &str = "issue";

/// Constructs the Tracker for a provider ID and an instance's settings.
pub(crate) type TrackerBuilder<'a> =
    dyn Fn(&str, &toml::Table) -> Result<Box<dyn Tracker>, String> + 'a;

/// The phase a Mission state shows on its issue, if it shows one.
pub(crate) fn phase_of(state: MissionState, blocked_reason: Option<&str>) -> Option<&'static str> {
    match state {
        // Not finished until someone has checked it: the issue still reads as being worked on.
        MissionState::Working | MissionState::Verifying => Some("working"),
        MissionState::Blocked if blocked_reason == Some(ISSUE_REASON) => Some("waiting"),
        MissionState::Blocked
        | MissionState::Stalled
        | MissionState::Parked
        | MissionState::Quarantined
        | MissionState::Held => Some("paused"),
        MissionState::Done => Some("done"),
        MissionState::Failed | MissionState::Blown | MissionState::Abandoned => Some("failed"),
        MissionState::Queued => None,
    }
}

/// Queues the status entries a Mission entering `phase` owes its linked issues.
///
/// `updates = outcome` writes none; `milestones` writes `working`, `waiting` and `paused` on
/// the source link (the report covers its end); `all` writes every phase on every link.
/// Entries of one issue coalesce, so a burst of changes sends the last phase once. `branch` is the
/// Mission's branch, which the comment names under any cover, as the report does.
pub(crate) fn queue_status(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    mission_id: &str,
    phase: &str,
    branch: Option<&str>,
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
        if let Some(branch) = branch {
            entry.body.push_str(&format!("\nBranch: `{branch}`"));
        }
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
            let issues = operation
                .issues
                .weakenings()
                .into_iter()
                .map(|line| format!("Operation {:?} issues is weakened: {line}", operation.id));
            let trusted = operation.sources.iter().filter_map(|(name, source)| {
                let file = source.get("ca_file")?;
                Some(format!(
                    "Operation {:?} source {name:?} trusts ca_file {file} besides the public roots",
                    operation.id
                ))
            });
            let reopened = setups_of(operation)
                .unwrap_or_default()
                .into_iter()
                .filter(|setup| setup.reopen == Reopen::Comment)
                .map(|setup| {
                    format!(
                        "Operation {:?} source {:?} reopens a settled item on a comment, so \
                         anyone the tracker lets answer for it can start its work again",
                        operation.id, setup.name
                    )
                });
            issues.chain(trusted).chain(reopened).collect::<Vec<_>>()
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
/// Comments AI6 wrote, by its mark and not their author, are skipped. Any other is recorded as
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
            // A settled item's comments are read only where a comment reopens it.
            let reopens = setup.reopen == Reopen::Comment;
            let watched: Vec<&IssueLink> = links
                .iter()
                .filter(|link| {
                    link.operation == operation.id
                        && link.tracker == setup.name
                        && matches!(
                            link.relation,
                            LinkRelation::Source | LinkRelation::Related | LinkRelation::Child
                        )
                        && (!settled(link) || (reopens && link.relation == LinkRelation::Source))
                })
                .collect();
            let mut reopening: BTreeMap<String, Vec<(crate::Reply, String)>> = BTreeMap::new();
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
                for link in watched.iter().filter(|link| {
                    link.item == reply.item
                        && unix_of(&reply.created).is_none_or(|made| made >= link.linked_at)
                }) {
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
                        operator: false,
                    };
                    if !reply.permitted {
                        record.state = ReplyState::Ignored;
                        record.body = String::new();
                    }
                    match registry.store_issue_reply(&record) {
                        Ok(true) if reply.permitted && settled(link) => {
                            stored += 1;
                            reopening
                                .entry(reply.item.clone())
                                .or_default()
                                .push((reply.clone(), link.mission_id.clone()));
                        }
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
            for (item, replies) in reopening {
                reopen_on_comments(
                    registry,
                    tracker.as_ref(),
                    station,
                    (&operation.id, &setup.name),
                    &item,
                    &replies,
                );
            }
            if let Some(next) = batch.cursor
                && cursor.as_deref() != Some(next.as_str())
            {
                let _ = registry.set_issue_cursor(&operation.id, &setup.name, &next);
            }
            for link in watched.iter().filter(|link| link.snapshot.is_none()) {
                refresh_snapshot(registry, tracker.as_ref(), link);
            }
        }
    }
    stored
}

/// Whether a link's Mission settled: its report was written on the issue.
fn settled(link: &IssueLink) -> bool {
    matches!(link.phase.as_deref(), Some("done" | "failed"))
}

/// Reopens the settled item `item` for the comments made on it since, each with the Mission
/// whose link it came in on, reading the item as it is now. The comments travel in the brief, so
/// they are marked delivered; an attempt in flight leaves them for it.
fn reopen_on_comments(
    registry: &dyn RegistryBackend,
    tracker: &dyn Tracker,
    station: &str,
    (operation, instance): (&str, &str),
    item: &str,
    replies: &[(crate::Reply, String)],
) {
    let issue = match tracker.issue(item.to_owned()) {
        Ok(issue) => issue,
        Err(error) => {
            eprintln!("ai6: {instance}:{item} not read to reopen it: {error}");
            return;
        }
    };
    let comments: Vec<crate::Reply> = replies.iter().map(|(reply, _)| reply.clone()).collect();
    // The item as its link names it, whatever spelling the tracker answers with.
    let found = crate::directorates::registry::sources::SourceItem {
        id: item.to_owned(),
        title: issue.title,
        body: issue.body,
        url: issue.url,
        labels: issue.labels,
        author: None,
        updated: issue.updated,
        code: issue.code,
        code_tag: issue.code_tag,
    };
    let why = format!("{} new comment(s)", comments.len());
    if crate::station::sources::reopen_item(
        registry,
        station,
        (operation, instance),
        &found,
        &comments,
        &why,
    ) {
        for (reply, mission) in replies {
            let _ = registry.mark_issue_reply(
                mission,
                instance,
                item,
                &reply.id,
                ReplyState::Delivered,
            );
        }
    }
}

/// Reads a link's issue and records what it says, for the tools that show it.
fn refresh_snapshot(registry: &dyn RegistryBackend, tracker: &dyn Tracker, link: &IssueLink) {
    match tracker.issue(link.item.clone()) {
        Ok(issue) => {
            let snapshot = IssueSnapshot {
                title: issue.title,
                body: issue.body,
                state: Some(issue.state),
                labels: issue.labels,
                priority: issue.priority,
            };
            if let Err(error) = registry.set_issue_snapshot(
                &link.mission_id,
                &link.tracker,
                &link.item,
                issue.url.as_deref(),
                &snapshot,
            ) {
                eprintln!("ai6: issue {} not recorded: {error}", link.name());
            }
        }
        Err(error) => eprintln!("ai6: issue {} not read: {error}", link.name()),
    }
}

/// A timestamp's Unix seconds, whichever offset it carries: `2026-11-01T15:56:39Z` or
/// `2026-11-01T22:57:12.421+0700`. `None` for one that does not parse.
pub(crate) fn unix_of(text: &str) -> Option<f64> {
    let at = time::OffsetDateTime::parse(
        text,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .ok()?;
    Some(at.unix_timestamp() as f64 + f64::from(at.nanosecond()) / 1e9)
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
    advance_proposals(registry, roster, build, &mut trackers, now);
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
                // A link a person made that cannot be read does not stay.
                if done.kind == OutboxKind::Link
                    && done.payload["resolve"] == true
                    && done.settled()
                    && done.state != OutboxState::Posted
                {
                    let _ = registry.unlink_issue(&done.mission_id, &done.tracker, &done.item);
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
        ..OutboxResult::default()
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
        OutboxKind::Create => create_issue(tracker, entry),
        OutboxKind::Link if entry.payload["resolve"] == true => {
            let link = link.ok_or_else(|| plain("the issue is no longer linked"))?;
            let issue = tracker.issue(entry.item.clone()).map_err(fail)?;
            let url = issue.url.clone();
            let snapshot = IssueSnapshot {
                title: issue.title,
                body: issue.body,
                state: Some(issue.state),
                labels: issue.labels,
                priority: issue.priority,
            };
            registry
                .set_issue_snapshot(
                    &link.mission_id,
                    &link.tracker,
                    &link.item,
                    url.as_deref(),
                    &snapshot,
                )
                .map_err(|error| SendFailure {
                    message: error.to_string(),
                    final_error: false,
                })?;
            Ok(OutboxResult {
                url,
                ..OutboxResult::default()
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
#[cfg_attr(not(feature = "cables-mcp"), allow(dead_code))]
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

/// Creates an entry's issue. A child the tracker cannot attach, or whose parent lives in another
/// tracker, names its parent in its text instead.
fn create_issue(tracker: &dyn Tracker, entry: &OutboxEntry) -> Result<OutboxResult, SendFailure> {
    let mut spec = spec_of(entry);
    if let Some(parent) = spec.parent.clone() {
        let children = tracker.capabilities(Some(parent)).map_err(fail)?.children;
        if children == "none" {
            spec.parent = None;
        }
    }
    if spec.parent.is_none()
        && let Some(reference) = text(&entry.payload["parent_reference"])
    {
        spec.body = format!("Part of {reference}\n\n{}", spec.body);
    }
    let asked: Vec<String> = spec.labels.clone();
    let wanted_parent = spec.parent.clone();
    let issue = tracker.create(spec).map_err(fail)?;
    let mut dropped = strings(&entry.payload["dropped"]);
    dropped.extend(
        asked
            .iter()
            .filter(|label| !issue.labels.iter().any(|kept| kept == *label))
            .filter(|label| {
                !issue
                    .labels
                    .iter()
                    .any(|kept| kept == &label.replace(' ', "-"))
            })
            .map(|label| format!("label {label}")),
    );
    Ok(OutboxResult {
        comment_id: None,
        url: issue.url,
        created_item: Some(issue.id),
        created_code: issue.code.map(|code| (code, issue.code_tag)),
        dropped,
        attached_to: issue.parent.filter(|_| wanted_parent.is_some()),
    })
}

/// The labels an instance takes intake from.
pub(crate) fn intake_labels(setup: &SourceSetup) -> Vec<String> {
    match setup.settings.get("labels").and_then(toml::Value::as_array) {
        Some(labels) => labels
            .iter()
            .filter_map(toml::Value::as_str)
            .map(str::to_owned)
            .collect(),
        None => match setup.provider.as_str() {
            "github" => vec!["ai6:ready".to_owned()],
            "jira" => vec!["ai6-ready".to_owned()],
            _ => Vec::new(),
        },
    }
}

/// Whether `label` is one any of the Operation's instances takes intake from, or looks like one,
/// so that nothing AI6 creates or changes can start work by itself.
pub(crate) fn is_intake_label(setups: &[SourceSetup], label: &str) -> bool {
    let lower = label.to_lowercase();
    lower.starts_with("ai6:ready")
        || lower.starts_with("ai6-ready")
        || setups.iter().any(|setup| {
            intake_labels(setup)
                .iter()
                .any(|intake| intake.eq_ignore_ascii_case(label))
        })
}

/// The Operation's tracker instances.
pub(crate) fn instances(operation: &crate::directorates::m::roster::Operation) -> Vec<SourceSetup> {
    source_setups(operation, &SourceLimits::default())
        .into_iter()
        .filter_map(Result::ok)
        .collect()
}

/// `TRACKER:ITEM` split in two.
fn reference(name: &str) -> Option<(&str, &str)> {
    name.split_once(':')
}

/// The payload of the `create` entry of one proposed issue.
fn create_payload(
    proposal: &IssueProposal,
    index: usize,
    spec: &ProposedIssue,
) -> serde_json::Value {
    let parent = spec.parent.as_deref().and_then(reference);
    // A parent in another tracker cannot be attached to; the child only names it.
    let attachable = parent.filter(|(tracker, _)| *tracker == spec.tracker);
    serde_json::json!({
        "proposal": proposal.id,
        "index": index,
        "title": spec.title,
        "labels": spec.labels,
        "issue_type": spec.issue_type,
        "priority": spec.priority,
        "parent": attachable.map(|(_, item)| item),
        "parent_reference": parent.map(|(_, item)| item),
        "parent_name": spec.parent,
        "mirror_of": spec.mirror_of,
        "dropped": proposal.dropped,
    })
}

/// Moves the approved proposals on: queues the creation of what each asks for, and settles what
/// was created, in one idempotent pass, so a Station that takes over finishes what another left.
fn advance_proposals(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    build: &TrackerBuilder<'_>,
    trackers: &mut TrackerCache,
    now: f64,
) {
    let Ok(proposals) = registry.issue_proposals(None) else {
        return;
    };
    for proposal in proposals
        .into_iter()
        .filter(|proposal| proposal.state == ProposalState::Approved)
    {
        let entries: Vec<OutboxEntry> = registry
            .outbox_entries(Some(&proposal.mission_id))
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| entry.payload["proposal"] == proposal.id.as_str())
            .collect();
        for (index, spec) in proposal.specs.iter().enumerate() {
            if entries
                .iter()
                .any(|entry| entry.payload["index"] == serde_json::json!(index))
            {
                continue;
            }
            let mut entry = OutboxEntry::new(
                &proposal.operation,
                &proposal.mission_id,
                &spec.tracker,
                "",
                OutboxKind::Create,
                &proposal.proposer,
                now,
            );
            entry.body = spec.body.clone();
            entry.payload = create_payload(&proposal, index, spec);
            match registry.queue_outbox(&entry) {
                Ok(stored) => audit(registry, "queued", &stored, now),
                Err(error) => {
                    eprintln!("ai6: creation of {} not queued: {error}", proposal.id);
                }
            }
        }
        let mut made = Vec::new();
        let mut failures = Vec::new();
        for entry in entries.iter().filter(|entry| entry.settled()) {
            let index = entry.payload["index"].as_u64().unwrap_or(0) as usize;
            let spec = proposal.specs.get(index);
            match (entry.state, entry.result.as_ref()) {
                (OutboxState::Posted, Some(result)) => {
                    if let Some(item) = &result.created_item {
                        let name = format!("{}:{item}", entry.tracker);
                        if !proposal.created.contains(&name) {
                            after_created(
                                registry, roster, build, trackers, &proposal, spec, entry, item,
                                now,
                            );
                        }
                        made.push(name);
                    }
                }
                _ => failures.push(format!(
                    "{}: {}",
                    spec.map_or("an issue", |spec| spec.title.as_str()),
                    entry.error.as_deref().unwrap_or("it could not be created")
                )),
            }
        }
        if made.is_empty() && failures.is_empty() {
            continue;
        }
        let was_open = proposal.state == ProposalState::Approved;
        match registry.record_created_issues(&proposal.id, &made, &failures) {
            Ok(done) if was_open && done.state == ProposalState::Created => {
                name_children_on_parents(registry, &done, &entries, now);
            }
            Ok(_) => {}
            Err(error) => eprintln!("ai6: proposal {} not recorded: {error}", proposal.id),
        }
    }
}

/// Ties a created issue to its Mission, leads the original of a mirror to its copy, and adds the
/// intake labels when the person who approved it asked for them.
#[allow(clippy::too_many_arguments)]
fn after_created(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    build: &TrackerBuilder<'_>,
    trackers: &mut TrackerCache,
    proposal: &IssueProposal,
    spec: Option<&ProposedIssue>,
    entry: &OutboxEntry,
    item: &str,
    now: f64,
) {
    if let Some(brief) = spec.and_then(|spec| spec.mission.as_ref()) {
        enter_created_mission(registry, proposal, entry, item, brief, now);
        return;
    }
    if proposal.mission_id == NO_MISSION {
        return;
    }
    let url = entry.result.as_ref().and_then(|result| result.url.clone());
    let mirror_of = spec.and_then(|spec| spec.mirror_of.clone());
    let link = IssueLink {
        operation: proposal.operation.clone(),
        mission_id: proposal.mission_id.clone(),
        tracker: entry.tracker.clone(),
        item: item.to_owned(),
        url: url.clone(),
        relation: if mirror_of.is_some() {
            LinkRelation::Mirror
        } else {
            LinkRelation::Child
        },
        state: LinkState::Active,
        linked_by: proposal.proposer.clone(),
        linked_at: now,
        status_comment: None,
        phase: None,
        snapshot: spec.map(|spec| IssueSnapshot {
            title: spec.title.clone(),
            body: spec.body.clone(),
            state: Some("open".to_owned()),
            labels: spec.labels.clone(),
            priority: spec.priority.clone(),
        }),
    };
    if let Err(error) = registry.link_issue(&link) {
        eprintln!("ai6: created issue {} not linked: {error}", link.name());
    }
    if let Some(original) = mirror_of.as_deref().and_then(reference) {
        back_link(
            registry,
            roster,
            build,
            trackers,
            proposal,
            original,
            url.as_deref(),
            &link,
            now,
        );
    }
    if proposal.mark_ready {
        mark_ready(registry, roster, proposal, entry, item, now);
    }
}

/// Enters the Mission a person created with the issue `item`: numbered by the issue as its Source
/// would number it, and linked to it as its source. A Mission already entered for the issue is
/// left as it is.
fn enter_created_mission(
    registry: &dyn RegistryBackend,
    proposal: &IssueProposal,
    entry: &OutboxEntry,
    item: &str,
    brief: &crate::Brief,
    now: f64,
) {
    let result = entry.result.as_ref();
    let (code, tag) = result
        .and_then(|result| result.created_code.clone())
        .map_or((None, None), |(code, tag)| (Some(code), tag));
    let at = crate::directorates::registry::mission_ids::MissionEntry::source(
        &proposal.operation,
        &entry.tracker,
        item,
        code.as_deref(),
        tag.as_deref(),
    );
    let entered = match registry.enter_brief(brief, &at) {
        Ok(entered) => entered,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return,
        Err(error) => {
            eprintln!(
                "ai6: the Mission created with {}:{item} did not enter: {error}",
                entry.tracker
            );
            return;
        }
    };
    let link = IssueLink {
        operation: proposal.operation.clone(),
        mission_id: entered.key(),
        tracker: entry.tracker.clone(),
        item: item.to_owned(),
        url: result.and_then(|result| result.url.clone()),
        relation: LinkRelation::Source,
        state: LinkState::Active,
        linked_by: proposal.proposer.clone(),
        linked_at: now,
        status_comment: None,
        phase: None,
        snapshot: None,
    };
    if let Err(error) = registry.link_issue(&link) {
        eprintln!("ai6: created issue {} not linked: {error}", link.name());
    }
    let record = AuditRecord {
        id: format!("mission-entered-{}", uuid::Uuid::new_v4().simple()),
        actor: proposal.proposer.clone(),
        action: "mission.create.entered".to_owned(),
        operation: proposal.operation.clone(),
        resource: None,
        target: format!("{} {}:{item}", entered.key(), entry.tracker),
        at: now,
    };
    if let Err(error) = registry.record_audit(&record) {
        eprintln!("ai6: issue audit record was not written: {error}");
    }
}

/// Leads the original of a mirror to its copy: a comment, and a remote link where the tracker
/// has them.
#[allow(clippy::too_many_arguments)]
fn back_link(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    build: &TrackerBuilder<'_>,
    trackers: &mut TrackerCache,
    proposal: &IssueProposal,
    (tracker, item): (&str, &str),
    url: Option<&str>,
    copy: &IssueLink,
    now: f64,
) {
    let target = url.map_or_else(|| copy.name(), str::to_owned);
    let mut comment = OutboxEntry::new(
        &proposal.operation,
        &proposal.mission_id,
        tracker,
        item,
        OutboxKind::Comment,
        "m",
        now,
    );
    comment.body = format!("Tracked in {target}");
    comment.payload = serde_json::json!({ "back_link_of": copy.name() });
    match registry.queue_outbox(&comment) {
        Ok(stored) => audit(registry, "queued", &stored, now),
        Err(error) => eprintln!("ai6: back-link on {tracker}:{item} not queued: {error}"),
    }
    let Some(url) = url else {
        return;
    };
    let Ok(setup) = setup_of(
        roster,
        &SourceLimits::default(),
        &proposal.operation,
        tracker,
    ) else {
        return;
    };
    let remote_links = trackers
        .entry((proposal.operation.clone(), tracker.to_owned()))
        .or_insert_with(|| build(&setup.provider, &setup.settings))
        .as_ref()
        .ok()
        .and_then(|tracker| tracker.capabilities(None).ok())
        .is_some_and(|capabilities| capabilities.links.iter().any(|kind| kind == "remote_links"));
    if !remote_links {
        return;
    }
    let mut link = OutboxEntry::new(
        &proposal.operation,
        &proposal.mission_id,
        tracker,
        item,
        OutboxKind::Link,
        "m",
        now,
    );
    link.payload = serde_json::json!({ "relation": "mirror", "url": url });
    match registry.queue_outbox(&link) {
        Ok(stored) => audit(registry, "queued", &stored, now),
        Err(error) => eprintln!("ai6: remote link on {tracker}:{item} not queued: {error}"),
    }
}

/// Adds the intake labels to an issue created under an approval that asked for them. The person
/// who approved it is the actor, and intake, vetting and review still decide what runs.
fn mark_ready(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    proposal: &IssueProposal,
    entry: &OutboxEntry,
    item: &str,
    now: f64,
) {
    let Ok(setup) = setup_of(
        roster,
        &SourceLimits::default(),
        &proposal.operation,
        &entry.tracker,
    ) else {
        return;
    };
    let actor = proposal
        .decided_by
        .as_deref()
        .map_or_else(|| "m".to_owned(), |by| format!("person:{by}"));
    let mut update = OutboxEntry::new(
        &proposal.operation,
        &proposal.mission_id,
        &entry.tracker,
        item,
        OutboxKind::Update,
        actor.clone(),
        now,
    );
    update.payload = update_payload(&TrackerUpdate {
        item: item.to_owned(),
        add_labels: intake_labels(&setup),
        ..TrackerUpdate::default()
    });
    match registry.queue_outbox(&update) {
        Ok(stored) => {
            let record = AuditRecord {
                id: format!("issue-ready-{}", uuid::Uuid::new_v4().simple()),
                actor,
                action: "issue.ready".to_owned(),
                operation: proposal.operation.clone(),
                resource: None,
                target: format!("{}:{item} {}", entry.tracker, stored.id),
                at: now,
            };
            if let Err(error) = registry.record_audit(&record) {
                eprintln!("ai6: issue audit record was not written: {error}");
            }
        }
        Err(error) => eprintln!("ai6: ready labels on {item} not queued: {error}"),
    }
}

/// A comment on each parent listing the children of a finished proposal the tracker could not
/// attach, so the split can be followed from the parent.
fn name_children_on_parents(
    registry: &dyn RegistryBackend,
    proposal: &IssueProposal,
    entries: &[OutboxEntry],
    now: f64,
) {
    let mut by_parent: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in entries {
        let Some(result) = &entry.result else {
            continue;
        };
        let (Some(parent), Some(item), None) = (
            text(&entry.payload["parent_name"]),
            result.created_item.as_ref(),
            result.attached_to.as_ref(),
        ) else {
            continue;
        };
        by_parent
            .entry(parent)
            .or_default()
            .push(result.url.clone().unwrap_or_else(|| item.clone()));
    }
    for (parent, children) in by_parent {
        let Some((tracker, item)) = reference(&parent) else {
            continue;
        };
        let mut comment = OutboxEntry::new(
            &proposal.operation,
            &proposal.mission_id,
            tracker,
            item,
            OutboxKind::Comment,
            "m",
            now,
        );
        comment.body = format!("Split into:\n\n- {}", children.join("\n- "));
        comment.payload = serde_json::json!({ "children_of": proposal.id });
        match registry.queue_outbox(&comment) {
            Ok(stored) => audit(registry, "queued", &stored, now),
            Err(error) => eprintln!("ai6: children of {parent} not named: {error}"),
        }
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
            ..OutboxResult::default()
        }),
        None => again(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_of_both_trackers_read_as_one_clock() {
        assert_eq!(unix_of("1970-01-01T00:00:01Z"), Some(1.0));
        assert_eq!(
            unix_of("2026-11-01T22:57:12.421+0700"),
            unix_of("2026-11-01T15:57:12.421Z")
        );
        assert_eq!(unix_of("not a time"), None);
        assert_eq!(unix_of("2024-02-29T23:59:59-08:00"), Some(1_709_279_999.0));
        assert_eq!(unix_of("2100-03-01T00:00:00Z"), Some(4_107_542_400.0));
        assert_eq!(unix_of("1900-03-01T00:00:00+0000"), Some(-2_203_891_200.0));
        assert_eq!(unix_of("2026-02-29T00:00:00Z"), None);
    }

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

#[cfg(test)]
#[path = "issues_tests.rs"]
mod creating_tests;

#[cfg(test)]
#[path = "issues_live_tests.rs"]
mod live_drain;
