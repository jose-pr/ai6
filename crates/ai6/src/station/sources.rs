//! Source polling, CX reports and sleepers, run by the Station that holds the Headquarters lease.
//!
//! Each Operation lists its tracker instances under `sources.<name>` in `operations.toml`;
//! `provider` names the provider (default: the key) and `intake = false` keeps an instance
//! out of polling. The desk polls each polled instance on an interval, queues what it
//! finds as briefs, and posts the outcome of every finished Mission back to the item it
//! came from through the Tracker kind. Cursors live in the
//! Registry, so a Station that takes over the Headquarters lease resumes where the last
//! one stopped. Authorized sleepers queue their briefs from the same pass.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::directorates::m::cover::Disclosure;
use crate::directorates::m::placement::{self, OperationPolicy, PlacementRequest, StationFacts};
use crate::directorates::m::roster::{Operation, Roster};
use crate::directorates::registry::backend::*;
use crate::directorates::registry::mission_issues::{
    IssueLink, IssueSnapshot, LinkRelation, LinkState,
};
use crate::directorates::registry::sleepers::{Fire, Sleeper, SleeperMark, window_label};
use crate::directorates::registry::sources::{
    FinishedSourceBrief, Ingest, Source, SourceConfig, SourcePoll, reportable,
};
use crate::directorates::registry::timeline::TimelineEvent;
use crate::directorates::registry::tracker::Tracker;
use crate::station::issues::{
    DrainState, TrackerBuilder, drain_outbox, poll_replies, queue_report,
};

use crate::station::limits::SourceLimits;
use crate::station::plugins::Plugins;
use guatiao::{FromValue, Schema, ToValue};

/// Constructs the provider for a provider ID and an Operation's settings.
pub(crate) type SourceBuilder<'a> =
    dyn Fn(&str, &toml::Table) -> Result<Box<dyn Source>, String> + 'a;

/// The words a Mission's progress is written in on an instance: a label or a workflow
/// transition per milestone. Unset milestones write nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub(crate) struct StatusWords {
    /// While an Agent works.
    #[schema(title = "While working")]
    pub working: Option<String>,
    /// While the Mission waits for an answer on the issue.
    #[schema(title = "While waiting for an answer")]
    pub waiting: Option<String>,
    /// While the Mission is paused.
    #[schema(title = "While paused")]
    pub paused: Option<String>,
    /// When the Mission ended successfully.
    #[schema(title = "When done")]
    pub done: Option<String>,
    /// When the Mission ended without completing.
    #[schema(title = "When failed")]
    pub failed: Option<String>,
}

/// What the Station itself reads of `[operation.ID.sources.NAME]`; the rest of the table goes
/// to the provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub(crate) struct InstanceKeys {
    /// The labels a Mission's milestones write.
    #[schema(title = "Status labels")]
    #[map(default)]
    pub status_labels: StatusWords,
    /// The workflow transitions a Mission's milestones make.
    #[schema(title = "Status transitions")]
    #[map(default)]
    pub status_transitions: StatusWords,
    /// The only labels an Agent may add or remove on a linked issue.
    #[schema(title = "Labels Agents may touch")]
    #[map(default)]
    pub agent_labels: Vec<String>,
    /// The workflow transitions an Agent may make without green ink.
    #[schema(title = "Transitions Agents may make")]
    #[map(default)]
    pub agent_transitions: Vec<String>,
    /// The priority a mirrored issue takes here, by the priority it has where it comes from.
    #[schema(title = "Priority map")]
    #[map(default)]
    pub priority_map: BTreeMap<String, String>,
    /// What queues an item whose Mission settled again, as the Mission's next attempt.
    #[schema(title = "Reopen a settled item on")]
    #[map(default)]
    pub reopen: Reopen,
}

impl InstanceKeys {
    const NAMES: [&str; 6] = [
        "status_labels",
        "status_transitions",
        "agent_labels",
        "agent_transitions",
        "priority_map",
        "reopen",
    ];
}

/// What reopens an item whose Mission settled. A reopened item is queued again as the same
/// Mission's next attempt, from the item as it reads then; never while an attempt is in flight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(rename_all = "lowercase")]
pub(crate) enum Reopen {
    /// A label added after the Mission settled: a poll finds the item with a label it did not
    /// have when AI6 last read it. The labels AI6 and its Agents write do not count.
    #[default]
    #[schema(label = "A label added again")]
    Label,
    /// As `label`, and also a comment that AI6 did not write, by anyone the tracker lets answer
    /// for the item. Reported as a weakening: such a person can then start work again.
    #[schema(label = "A label or a comment")]
    Comment,
    /// Nothing: a settled item stays settled.
    #[schema(label = "Never")]
    Never,
}

impl StatusWords {
    /// The word for a phase: `working`, `waiting`, `paused`, `done`; any other is `failed`.
    pub(crate) fn for_phase(&self, phase: &str) -> Option<&str> {
        match phase {
            "working" => self.working.as_deref(),
            "waiting" => self.waiting.as_deref(),
            "paused" => self.paused.as_deref(),
            "done" => self.done.as_deref(),
            _ => self.failed.as_deref(),
        }
    }

    /// Every word set, whichever phase it is for.
    pub(crate) fn all(&self) -> Vec<&str> {
        [
            &self.working,
            &self.waiting,
            &self.paused,
            &self.done,
            &self.failed,
        ]
        .into_iter()
        .filter_map(|word| word.as_deref())
        .collect()
    }
}

/// What one poll of one source changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PollSummary {
    pub queued: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub skipped: usize,
    /// Settled items queued again as their Mission's next attempt.
    pub reopened: usize,
}

/// A tracker instance as configured for one Operation.
pub(crate) struct SourceSetup {
    /// The key under `sources`: what briefs, cursors and linked issues call the instance.
    pub name: String,
    /// The provider that serves it.
    pub provider: String,
    /// The settings the provider is built with.
    pub settings: toml::Table,
    pub interval: Duration,
    /// Whether the instance is polled for work; `false` for one used only to link or create.
    pub intake: bool,
    /// Labels the Station writes for Mission milestones.
    pub status_labels: StatusWords,
    /// Workflow transitions the Station makes for Mission milestones.
    pub status_transitions: StatusWords,
    /// The only labels an Agent may add or remove on a linked issue.
    pub agent_labels: Vec<String>,
    /// The workflow transitions an Agent may make without green ink.
    pub agent_transitions: Vec<String>,
    /// The priority a mirrored issue takes here, by the priority it has where it comes from.
    pub priority_map: BTreeMap<String, String>,
    /// What reopens an item whose Mission settled.
    pub reopen: Reopen,
}

impl SourceSetup {
    /// How a poll of this instance treats an item whose Mission settled.
    pub(crate) fn reopening(&self) -> Reopening {
        let own = self
            .status_labels
            .all()
            .into_iter()
            .chain(self.agent_labels.iter().map(String::as_str))
            .map(str::to_owned)
            .collect();
        Reopening {
            on: self.reopen,
            own,
        }
    }
}

/// How a poll treats an item whose Mission settled ([`Reopen`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Reopening {
    /// What reopens it.
    pub on: Reopen,
    /// Labels AI6 or its Agents write, which reopen nothing.
    pub own: BTreeSet<String>,
}

impl Reopening {
    /// Nothing reopens a settled item.
    #[cfg(test)]
    pub(crate) fn never() -> Self {
        Self {
            on: Reopen::Never,
            own: BTreeSet::new(),
        }
    }
}

/// Reads an Operation's `sources` table into per-instance settings.
///
/// A table value holds the provider's settings; a bare string is shorthand for
/// `{ target = "<string>" }`. `provider`, `intake` and `interval_secs` belong to the desk
/// and are not passed on: `provider` defaults to the key, `intake` to `true`, and the
/// interval to `limits.poll`, raised to `limits.min_poll`, so a typo cannot hammer a tracker.
pub(crate) fn source_setups(
    operation: &Operation,
    limits: &SourceLimits,
) -> Vec<Result<SourceSetup, String>> {
    operation
        .sources
        .iter()
        .map(|(name, value)| {
            let mut settings = match value {
                toml::Value::Table(table) => table.clone(),
                toml::Value::String(target) => {
                    let mut table = toml::Table::new();
                    table.insert("target".to_owned(), toml::Value::String(target.clone()));
                    table
                }
                _ => {
                    return Err(format!(
                        "Operation {}: sources.{name} must be a table or a string",
                        operation.id
                    ));
                }
            };
            let provider = match settings.remove("provider") {
                None => name.clone(),
                Some(toml::Value::String(provider)) if !provider.is_empty() => provider,
                Some(_) => {
                    return Err(format!(
                        "Operation {}: sources.{name}.provider must be a provider ID",
                        operation.id
                    ));
                }
            };
            let intake = match settings.remove("intake") {
                None => true,
                Some(toml::Value::Boolean(intake)) => intake,
                Some(_) => {
                    return Err(format!(
                        "Operation {}: sources.{name}.intake must be true or false",
                        operation.id
                    ));
                }
            };
            let mut own = toml::Table::new();
            for key in InstanceKeys::NAMES {
                if let Some(value) = settings.remove(key) {
                    own.insert(key.to_owned(), value);
                }
            }
            let keys: InstanceKeys = crate::settings_from_table(own)
                .map_err(|error| format!("Operation {}: sources.{name}: {error}", operation.id))?;
            let interval = match settings.remove("interval_secs") {
                None => limits.poll.0,
                Some(toml::Value::Integer(seconds)) if seconds > 0 => {
                    Duration::from_secs(seconds as u64).max(limits.min_poll.0)
                }
                Some(_) => {
                    return Err(format!(
                        "Operation {}: sources.{name}.interval_secs must be a positive integer",
                        operation.id
                    ));
                }
            };
            Ok(SourceSetup {
                name: name.clone(),
                provider,
                settings,
                interval,
                intake,
                status_labels: keys.status_labels,
                status_transitions: keys.status_transitions,
                agent_labels: keys.agent_labels,
                agent_transitions: keys.agent_transitions,
                priority_map: keys.priority_map,
                reopen: keys.reopen,
            })
        })
        .collect()
}

/// Builds the provider for one Operation's settings through the plugin host.
pub(crate) fn build_source(
    plugins: &Plugins,
    provider: &str,
    settings: &toml::Table,
) -> Result<Box<dyn Source>, String> {
    let settings_toml = toml::to_string(settings).map_err(|error| error.to_string())?;
    let config = SourceConfig { settings_toml }
        .to_value()
        .map_err(|error| error.to_string())?;
    plugins
        .source(provider, Some(&config))
        .map_err(|error| error.to_string())
}

/// Builds the Tracker for one instance's settings through the plugin host.
pub(crate) fn build_tracker(
    plugins: &Plugins,
    provider: &str,
    settings: &toml::Table,
) -> Result<Box<dyn Tracker>, String> {
    let settings_toml = toml::to_string(settings).map_err(|error| error.to_string())?;
    let config = SourceConfig { settings_toml }
        .to_value()
        .map_err(|error| error.to_string())?;
    plugins
        .tracker(provider, Some(&config))
        .map_err(|error| error.to_string())
}

/// Polls one source once and queues what it found; the cursor advances after the items
/// are stored, so a crash repeats work that is already idempotent. `shared` says the instance is
/// not its provider's only one in the Operation: its name replaces an item's code tag. An item
/// whose Mission settled is reopened as `reopening` says.
pub(crate) fn poll_source(
    registry: &dyn RegistryBackend,
    station: &str,
    (operation, provider): (&str, &str),
    shared: bool,
    source: &dyn Source,
    reopening: &Reopening,
) -> Result<PollSummary, String> {
    let cursor = registry
        .source_cursor(operation, provider)
        .map_err(|error| format!("cursor unreadable: {error}"))?;
    let batch = source
        .poll(SourcePoll { cursor })
        .map_err(|error| format!("poll failed: {error}"))?;
    let mut summary = PollSummary::default();
    for mut item in batch.items {
        if shared && item.code_tag.is_some() {
            item.code_tag = Some(provider.to_owned());
        }
        let item = &item;
        match registry.ingest_source_item(operation, provider, item, station) {
            Ok(Ingest::Queued) => {
                summary.queued += 1;
                link_source(registry, operation, provider, item);
            }
            Ok(Ingest::Updated) => {
                summary.updated += 1;
                link_source(registry, operation, provider, item);
            }
            Ok(Ingest::Unchanged) => {
                summary.unchanged += 1;
                link_source(registry, operation, provider, item);
            }
            Ok(Ingest::Finished)
                if reopen_on_labels(registry, station, (operation, provider), item, reopening) =>
            {
                summary.reopened += 1;
            }
            Ok(Ingest::InFlight) => {
                summary.skipped += 1;
                // What the item says while its Mission works is what a reopen compares with.
                if let Some(link) = item_link(registry, (operation, provider), item) {
                    let _ = registry.set_issue_snapshot(
                        &link.mission_id,
                        provider,
                        &item.id,
                        item.url.as_deref(),
                        &snapshot_of(item),
                    );
                }
            }
            Ok(Ingest::Finished) => summary.skipped += 1,
            Err(error) => eprintln!(
                "ai6: source {provider}: item {:?} not queued: {error}",
                item.id
            ),
        }
    }
    if let Some(cursor) = batch.cursor {
        registry
            .set_source_cursor(operation, provider, &cursor)
            .map_err(|error| format!("cursor not stored: {error}"))?;
    }
    Ok(summary)
}

/// The link from the Mission `item` entered as to the item, when there is one.
fn item_link(
    registry: &dyn RegistryBackend,
    (operation, provider): (&str, &str),
    item: &crate::directorates::registry::sources::SourceItem,
) -> Option<IssueLink> {
    let entry = crate::directorates::registry::sources::source_entry(operation, provider, item);
    let mission = registry.entered_mission_id(&entry).ok()??;
    registry
        .issue_links(&mission)
        .ok()?
        .into_iter()
        .find(|link| {
            link.relation == LinkRelation::Source
                && link.tracker == provider
                && link.item == item.id
        })
}

/// What `item` says, as a link keeps it.
fn snapshot_of(item: &crate::directorates::registry::sources::SourceItem) -> IssueSnapshot {
    IssueSnapshot {
        title: item.title.clone(),
        body: item.body.clone(),
        state: Some("open".to_owned()),
        labels: item.labels.clone(),
        priority: None,
    }
}

/// Reopens the settled `item` when a poll finds it with a label it did not have when AI6 last
/// read it, other than one AI6 or its Agents write. Either way its link then keeps the item as it
/// reads now, so the next poll compares with that. Whether it was reopened.
fn reopen_on_labels(
    registry: &dyn RegistryBackend,
    station: &str,
    (operation, provider): (&str, &str),
    item: &crate::directorates::registry::sources::SourceItem,
    reopening: &Reopening,
) -> bool {
    if reopening.on == Reopen::Never {
        return false;
    }
    let Some(link) = item_link(registry, (operation, provider), item) else {
        return false;
    };
    let added: Vec<&str> = match &link.snapshot {
        Some(seen) => item
            .labels
            .iter()
            .filter(|label| !seen.labels.contains(label) && !reopening.own.contains(*label))
            .map(String::as_str)
            .collect(),
        None => Vec::new(),
    };
    let reopened = !added.is_empty()
        && reopen_item(
            registry,
            station,
            (operation, provider),
            item,
            &[],
            &format!("label {} added", added.join(", ")),
        );
    if let Err(error) = registry.set_issue_snapshot(
        &link.mission_id,
        provider,
        &item.id,
        item.url.as_deref(),
        &snapshot_of(item),
    ) {
        eprintln!(
            "ai6: source {provider}: item {:?} not read back into its link: {error}",
            item.id
        );
    }
    reopened
}

/// Queues the settled `item` again as its Mission's next attempt, from the item as it reads now
/// and the `comments` made on it since, and records `why` on the timeline and in the audit log.
/// Whether it was queued: an attempt in flight, or an item that never entered, is left alone.
pub(crate) fn reopen_item(
    registry: &dyn RegistryBackend,
    station: &str,
    (operation, provider): (&str, &str),
    item: &crate::directorates::registry::sources::SourceItem,
    comments: &[crate::Reply],
    why: &str,
) -> bool {
    use crate::directorates::registry::sources::{reopened_brief, source_entry};
    use crate::directorates::registry::store::{AuditRecord, Reentry};
    let brief = reopened_brief(
        (operation, provider),
        item,
        comments,
        &registry.limits().sources,
        station,
    );
    // Only the item's own brief: a person's Mission created with the issue stays theirs.
    let queued =
        match registry.reenter_brief(&brief, &source_entry(operation, provider, item), &|filed| {
            filed.source == brief.source
        }) {
            Ok(Reentry::Queued(queued)) => queued,
            Ok(_) => return false,
            Err(error) => {
                eprintln!(
                    "ai6: source {provider}: item {:?} not reopened: {error}",
                    item.id
                );
                return false;
            }
        };
    let (mission, now) = (queued.key(), unix_seconds() as f64);
    eprintln!(
        "ai6: source {provider}: item {:?} reopened Mission {} ({why})",
        item.id,
        queued.shown_handle()
    );
    let record = AuditRecord {
        id: format!("source-reopened-{}", uuid::Uuid::new_v4().simple()),
        actor: "m".to_owned(),
        action: "mission.reopened".to_owned(),
        operation: operation.to_owned(),
        resource: None,
        target: format!("{mission} {provider}:{}", item.id),
        at: now,
    };
    if let Err(error) = registry.record_audit(&record) {
        eprintln!("ai6: the reopen of Mission {mission} was not audited: {error}");
    }
    let event = TimelineEvent::new("source", "reopened", operation, now)
        .mission(mission)
        .actor("m")
        .detail("tracker", provider.to_owned())
        .detail("item", item.id.clone())
        .detail("reason", why.to_owned());
    let _ = registry.record_event(&event);
    true
}

/// Reads again the newest settled items of each instance whose items reopen, at most
/// `limits.reopen_watch` per instance, so a label taken off one is seen and its return reopens
/// it ([`Reopen::Label`]). Only the Headquarters Station does; returns how many it read.
pub(crate) fn watch_settled(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    build: &TrackerBuilder<'_>,
    station: &str,
    limits: &SourceLimits,
) -> usize {
    if !registry
        .headquarters()
        .ok()
        .flatten()
        .is_some_and(|lease| lease.station() == station)
    {
        return 0;
    }
    let Ok(links) = registry.active_issue_links() else {
        return 0;
    };
    let mut read = 0;
    for operation in roster.operations() {
        for setup in source_setups(operation, limits).into_iter().flatten() {
            if setup.reopen == Reopen::Never {
                continue;
            }
            let mut settled: Vec<&IssueLink> = links
                .iter()
                .filter(|link| {
                    link.operation == operation.id
                        && link.tracker == setup.name
                        && link.relation == LinkRelation::Source
                        && matches!(link.phase.as_deref(), Some("done" | "failed"))
                })
                .collect();
            if settled.is_empty() {
                continue;
            }
            settled.sort_by(|left, right| right.linked_at.total_cmp(&left.linked_at));
            settled.truncate(limits.reopen_watch.or_max());
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
            for link in settled {
                match tracker.issue(link.item.clone()) {
                    Ok(issue) => {
                        let snapshot = IssueSnapshot {
                            title: issue.title,
                            body: issue.body,
                            state: Some(issue.state),
                            labels: issue.labels,
                            priority: issue.priority,
                        };
                        if registry
                            .set_issue_snapshot(
                                &link.mission_id,
                                &link.tracker,
                                &link.item,
                                issue.url.as_deref(),
                                &snapshot,
                            )
                            .is_ok()
                        {
                            read += 1;
                        }
                    }
                    Err(error) => eprintln!(
                        "ai6: settled item {}:{} not read: {error}",
                        link.tracker, link.item
                    ),
                }
            }
        }
    }
    read
}

/// The outcome of a Mission, worded for the item that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CxReport {
    /// The item ID the Mission came from.
    pub item_id: String,
    /// The Mission's UID.
    pub mission_id: String,
    /// `done`, `failed` or another terminal Mission state.
    pub outcome: String,
    /// The comment text, already worded for the Operation's cover policy.
    pub body: String,
    /// The Git branch holding the work, when the Mission kept a safe house.
    pub branch: Option<String>,
}

/// Ties the issue an item came from to the Mission its brief becomes, so the Mission's page
/// can show it before placement. A link already there (even an unlinked one) is left alone.
fn link_source(
    registry: &dyn RegistryBackend,
    operation: &str,
    provider: &str,
    item: &crate::directorates::registry::sources::SourceItem,
) {
    let entry = crate::directorates::registry::sources::source_entry(operation, provider, item);
    let mission = match registry.entered_mission_id(&entry) {
        Ok(Some(mission)) => mission,
        Ok(None) => return,
        Err(error) => {
            eprintln!("ai6: source {provider}: Mission ID unreadable: {error}");
            return;
        }
    };
    match registry.issue_links(&mission) {
        Ok(links)
            if links
                .iter()
                .any(|link| link.tracker == provider && link.item == item.id) => {}
        Ok(_) => {
            let now = unix_seconds() as f64;
            let mut link = source_link(
                operation,
                &mission,
                provider,
                &item.id,
                item.url.clone(),
                now,
            );
            link.snapshot = Some(IssueSnapshot {
                title: item.title.clone(),
                body: item.body.clone(),
                state: Some("open".to_owned()),
                labels: item.labels.clone(),
                priority: None,
            });
            if let Err(error) = registry.link_issue(&link) {
                eprintln!(
                    "ai6: source {provider}: item {:?} not linked: {error}",
                    item.id
                );
            }
            link_parent_of_created(registry, operation, &mission, provider, &item.id, now);
        }
        Err(error) => eprintln!("ai6: source {provider}: links unreadable: {error}"),
    }
}

/// When AI6 created this issue, ties the Mission it became to the issue it was split from,
/// read-only, and names the Mission that split it as who linked it.
fn link_parent_of_created(
    registry: &dyn RegistryBackend,
    operation: &str,
    mission: &str,
    tracker: &str,
    item: &str,
    now: f64,
) {
    let name = format!("{tracker}:{item}");
    let Ok(proposals) = registry.issue_proposals(None) else {
        return;
    };
    let Some(proposal) = proposals
        .iter()
        .find(|proposal| proposal.operation == operation && proposal.created.contains(&name))
    else {
        return;
    };
    let Ok(entries) = registry.outbox_entries(Some(&proposal.mission_id)) else {
        return;
    };
    let parent = entries.iter().find_map(|entry| {
        let made = entry.result.as_ref()?.created_item.as_deref()? == item;
        (made && entry.tracker == tracker)
            .then(|| entry.payload["parent_name"].as_str().map(str::to_owned))
            .flatten()
    });
    let Some((parent_tracker, parent_item)) = parent.as_deref().and_then(|p| p.split_once(':'))
    else {
        return;
    };
    let mut link = source_link(operation, mission, parent_tracker, parent_item, None, now);
    link.relation = LinkRelation::Parent;
    link.linked_by = format!("mission:{}", proposal.mission_id);
    if let Err(error) = registry.link_issue(&link) {
        eprintln!("ai6: parent of {name} not linked: {error}");
    }
}

/// Words the report for the Operation's cover policy.
pub(crate) fn compose_report(disclosure: Disclosure, finished: &FinishedSourceBrief) -> CxReport {
    let outcome_text = |key: &str| {
        finished
            .outcome
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let outcome = outcome_text("state")
        .unwrap_or_else(|| if finished.ok { "done" } else { "failed" }.to_owned());
    // A brief that was never placed ended without a Mission.
    let mission_id = outcome_text("mission_id").filter(|id| !id.is_empty());
    // The tracker reads the handle the Mission has when the report is written.
    let shown = mission_id
        .as_ref()
        .map(|id| finished.brief.handle.clone().unwrap_or_else(|| id.clone()));
    let branch = outcome_text("branch");
    let item_id = finished
        .brief
        .data
        .get("source")
        .and_then(|source| source.get("item"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&finished.brief.id)
        .to_owned();

    let mut lines = Vec::new();
    match disclosure {
        Disclosure::Hidden => lines.push(
            if finished.ok {
                "The requested work is finished."
            } else {
                "The requested work could not be completed."
            }
            .to_owned(),
        ),
        Disclosure::Assisted => {
            lines.push(
                if finished.ok {
                    "AI-assisted work on this item is finished."
                } else {
                    "AI-assisted work on this item ended without completing."
                }
                .to_owned(),
            );
            if let Some(shown) = &shown {
                lines.push(format!("Mission: `{shown}`"));
            }
        }
        _ => lines.push(match &shown {
            Some(shown) => format!("AI6 Mission `{shown}` ended: {outcome}."),
            None => "AI6 could not start a Mission for this item.".to_owned(),
        }),
    }
    if let Some(branch) = &branch {
        lines.push(format!("Branch: `{branch}`"));
    }
    let denied_tools: Vec<&str> = finished
        .outcome
        .get("denied_tools")
        .and_then(serde_json::Value::as_array)
        .map(|tools| tools.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    if disclosure != Disclosure::Hidden && !denied_tools.is_empty() {
        lines.push(format!(
            "Refused by its permission rules: {}",
            denied_tools.join(", ")
        ));
    }
    if disclosure == Disclosure::Full {
        if let Some(agent) = outcome_text("agent") {
            lines.push(format!("Agent: {agent}"));
        }
        let session_ids: Vec<&str> = finished
            .outcome
            .get("session_ids")
            .and_then(serde_json::Value::as_array)
            .map(|ids| ids.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default();
        if !session_ids.is_empty() {
            lines.push(format!("Provider Sessions: {}", session_ids.join(", ")));
        }
    }
    CxReport {
        item_id,
        mission_id: mission_id.unwrap_or_default(),
        outcome,
        body: lines.join("\n"),
        branch,
    }
}

/// Queues the report of each finished source Mission as an outbox entry on its source issue.
///
/// A report that cannot be queued is retried on later passes, up to the Registry's attempt
/// limit; the Headquarters Station sends what is queued.
pub(crate) fn report_finished(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    limits: &SourceLimits,
    now: f64,
) -> usize {
    let Ok(finished) = registry.unreported_source_briefs() else {
        return 0;
    };
    let mut queued = 0;
    for entry in finished {
        match queue_one(registry, roster, limits, &entry, now) {
            Ok(()) => {
                queued += 1;
                let _ = registry.mark_source_reported(&entry, true);
            }
            Err(error) => {
                eprintln!(
                    "ai6: CX report for {} not queued: {error}",
                    entry.brief.key()
                );
                let _ = registry.mark_source_reported(&entry, false);
            }
        }
    }
    queued
}

fn queue_one(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    limits: &SourceLimits,
    entry: &FinishedSourceBrief,
    now: f64,
) -> Result<(), String> {
    let instance = entry
        .brief
        .source
        .trim_start_matches(crate::SOURCE_BRIEF_PREFIX);
    let operation_key = entry
        .brief
        .operation
        .as_deref()
        .ok_or("the brief names no Operation")?;
    let operation = roster.operation(operation_key).map_err(|e| e.to_string())?;
    let cover = roster.cover(&operation.cover).map_err(|e| e.to_string())?;
    source_setups(operation, limits)
        .into_iter()
        .filter_map(Result::ok)
        .find(|setup| setup.name == instance)
        .ok_or_else(|| {
            format!(
                "Operation {} no longer configures source {instance}",
                operation.id
            )
        })?;
    if !reportable(&entry.brief, instance) {
        return Err(format!(
            "the brief has no provenance from source {instance}, so nothing is posted for it"
        ));
    }
    let report = compose_report(cover.disclosure(), entry);
    let mission_id = crate::station::server::mission_id_for_brief(&entry.brief)?.to_string();
    let link = match registry
        .issue_links(&mission_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|link| link.tracker == instance && link.item == report.item_id)
    {
        Some(link) if link.state == LinkState::Active => link,
        Some(_) => return Ok(()),
        None => registry
            .link_issue(&source_link(
                operation_key,
                &mission_id,
                instance,
                &report.item_id,
                entry.brief.url.clone(),
                now,
            ))
            .map_err(|e| e.to_string())?,
    };
    queue_report(registry, &link, report.body, &report.outcome, now)
}

/// The link from a Mission to the issue it started from.
pub(crate) fn source_link(
    operation: &str,
    mission_id: &str,
    tracker: &str,
    item: &str,
    url: Option<String>,
    now: f64,
) -> IssueLink {
    IssueLink {
        operation: operation.to_owned(),
        mission_id: mission_id.to_owned(),
        tracker: tracker.to_owned(),
        item: item.to_owned(),
        url,
        relation: LinkRelation::Source,
        state: LinkState::Active,
        linked_by: "m".to_owned(),
        linked_at: now,
        status_comment: None,
        phase: None,
        snapshot: None,
    }
}

/// Why a window passed unfired, as the timeline says it.
const NOT_AUTHORIZED: &str = "not authorized";
const SCHEDULER_DOWN: &str = "the scheduler was not running";
const IN_FLIGHT: &str = "the last attempt was in flight";
const PAST_THE_BOUND: &str = "older than its catch-up bound";
const NO_STATION: &str = "no Station could take it";

/// One standing order as the Headquarters Station reckons with it.
struct Order<'a> {
    registry: &'a dyn RegistryBackend,
    operation: &'a str,
    name: &'a str,
    sleeper: &'a Sleeper,
    now: i64,
}

impl Order<'_> {
    fn tell(&self, action: &str, window: String, why: Option<&str>) {
        let mut event = TimelineEvent::new("sleeper", action, self.operation, self.now as f64)
            .detail("sleeper", self.name)
            .detail("window", window);
        if let Some(why) = why {
            event = event.detail("why", why);
        }
        if let Err(error) = self.registry.record_event(&event) {
            eprintln!(
                "ai6: sleeper {} of Operation {}: timeline event not written: {error}",
                self.name, self.operation
            );
        }
    }

    /// Moves `mark` on to `latest`, telling the timeline of each window left behind unfired: the
    /// one it stood at, for the reason it has, and those no pass reckoned with, for `gap`.
    /// Returns whether any was left behind.
    fn pass_windows(&self, mark: &mut SleeperMark, latest: i64, gap: &str) -> bool {
        let limits = &self.registry.limits().sleepers;
        let mut named = Vec::new();
        let mut older = false;
        let mut window = latest;
        while let Some(before) = self.sleeper.window_before(window, limits) {
            if before <= mark.window || before <= mark.since {
                break;
            }
            if !limits.skipped_events.allows(named.len() + 1) {
                older = true;
                break;
            }
            named.push(before);
            window = before;
        }
        let left = !mark.settled;
        if left {
            self.tell(
                "skipped",
                window_label(mark.window),
                Some(mark.why.as_deref().unwrap_or(SCHEDULER_DOWN)),
            );
        }
        if older {
            self.tell(
                "skipped",
                format!("{} to {}", window_label(mark.window), window_label(window)),
                Some(gap),
            );
        }
        for skipped in named.iter().rev() {
            self.tell("skipped", window_label(*skipped), Some(gap));
        }
        let missed = left || older || !named.is_empty();
        *mark = SleeperMark::new(mark.since, latest);
        missed
    }
}

/// Queues the brief of every authorized sleeper whose latest window has not fired, and puts each
/// window that passed unfired on the timeline.
///
/// Returns how many briefs were queued. A sleeper with no approval of its current definition does
/// not fire and is named once in `warned`; an approval that an edit left behind keeps its windows
/// on the record, and the approval given again may fire the latest of them late.
pub(crate) fn fire_due_sleepers(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    station: &str,
    now: i64,
    warned: &mut BTreeSet<String>,
) -> usize {
    let mut queued = 0;
    for operation in roster.operations() {
        for (name, sleeper) in &operation.sleepers {
            let approval = match registry.sleeper_authorization(&operation.id, name) {
                Ok(approval) => approval,
                Err(error) => {
                    eprintln!("ai6: sleeper {name} of Operation {}: {error}", operation.id);
                    continue;
                }
            };
            let authorized = approval
                .as_ref()
                .is_some_and(|approval| approval.fingerprint == sleeper.fingerprint());
            if !authorized && warned.insert(format!("{}/{name}", operation.id)) {
                eprintln!(
                    "ai6: sleeper {name} of Operation {} waits for `ai6 sleeper authorize {} {name}`",
                    operation.id, operation.id
                );
            }
            let Some(approval) = approval else {
                continue;
            };
            let limits = &registry.limits().sleepers;
            let Some(latest) = sleeper.latest_window(now, limits) else {
                continue;
            };
            let order = Order {
                registry,
                operation: &operation.id,
                name,
                sleeper,
                now,
            };
            let mut mark = approval
                .mark
                .clone()
                .unwrap_or_else(|| SleeperMark::new(approval.at, latest));
            let gap = if authorized {
                SCHEDULER_DOWN
            } else {
                NOT_AUTHORIZED
            };
            let missed = latest > mark.window && order.pass_windows(&mut mark, latest, gap);
            if authorized {
                match sleeper.due_window(now, mark.since, limits) {
                    Some(window) => {
                        let late = missed || mark.why.is_some();
                        let fired =
                            fire_rounds(&order, (window, station), roster, warned, &mut mark);
                        if late && fired > 0 {
                            order.tell("late", window_label(window), None);
                        }
                        queued += fired;
                    }
                    None if !mark.settled => {
                        order.tell("skipped", window_label(latest), Some(PAST_THE_BOUND));
                        mark.settled = true;
                    }
                    None => {}
                }
            } else if !mark.settled {
                mark.why = Some(NOT_AUTHORIZED.to_owned());
            }
            if approval.mark.as_ref() != Some(&mark)
                && let Err(error) = registry.mark_sleeper(&operation.id, name, &mark)
            {
                eprintln!("ai6: sleeper {name} of Operation {}: {error}", operation.id);
            }
        }
    }
    queued
}

/// Offers `window` to the Registry for each of the order's rounds, and notes on `mark` how it went;
/// how many briefs were queued.
fn fire_rounds(
    order: &Order<'_>,
    (window, station): (i64, &str),
    roster: &Roster,
    warned: &mut BTreeSet<String>,
    mark: &mut SleeperMark,
) -> usize {
    let (registry, operation, name, sleeper) =
        (order.registry, order.operation, order.name, order.sleeper);
    let rounds = if sleeper.each_station {
        round_stations(registry, roster, operation, sleeper, order.now)
            .into_iter()
            .map(Some)
            .collect()
    } else {
        vec![None]
    };
    let mut queued = 0;
    let (mut in_flight, mut failed) = (false, false);
    for round in &rounds {
        match registry.fire_sleeper(
            operation,
            name,
            sleeper,
            (window, round.as_deref()),
            station,
        ) {
            Ok(Fire::Queued) => {
                queued += 1;
                match round {
                    Some(round) => eprintln!(
                        "ai6: sleeper {name} of Operation {operation} queued the round of Station {round}",
                    ),
                    None => {
                        eprintln!("ai6: sleeper {name} of Operation {operation} queued a brief")
                    }
                }
            }
            Ok(Fire::InFlight) => {
                in_flight = true;
                // Named once per window; only the latest window is remembered.
                let prefix = format!(
                    "skipped/{operation}/{name}/{}/",
                    round.as_deref().unwrap_or_default()
                );
                let skipped = format!("{prefix}{}", window_label(window));
                warned.retain(|seen| !seen.starts_with(&prefix) || *seen == skipped);
                if warned.insert(skipped) {
                    eprintln!(
                        "ai6: sleeper {name} of Operation {operation} has not fired window {}: \
                         its standing Mission's last attempt is still in flight",
                        window_label(window)
                    );
                }
            }
            Ok(Fire::AlreadyFired) => {}
            Err(error) => {
                failed = true;
                eprintln!("ai6: sleeper {name} of Operation {operation}: {error}");
            }
        }
    }
    if in_flight {
        mark.why = Some(IN_FLIGHT.to_owned());
    } else if rounds.is_empty() {
        mark.why = Some(NO_STATION.to_owned());
    } else if !failed {
        mark.settled = true;
        mark.why = None;
    }
    queued
}

/// The Stations an `each_station` sleeper holds a round on: every Station whose last check-in
/// placement accepts for the Operation, within the sleeper's own `stations`, for the Agent or
/// role it names.
///
/// What only delays a brief does not exclude a Station: its load, its resources and Agent
/// quarantines are left out, so a busy Station still gets its round and runs it when it has
/// room. A Station that has gone dark gets none; it gets the window's round when it checks
/// in again while the window may still fire.
fn round_stations(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    operation: &str,
    sleeper: &Sleeper,
    now: i64,
) -> Vec<String> {
    let Ok(policy) = OperationPolicy::from_roster(roster, operation) else {
        return Vec::new();
    };
    let records = match registry.stations(Duration::MAX) {
        Ok(records) => records,
        Err(error) => {
            eprintln!("ai6: Stations for the rounds of Operation {operation}: {error}");
            return Vec::new();
        }
    };
    records
        .iter()
        .filter(|record| {
            sleeper.stations.is_empty() || sleeper.stations.iter().any(|s| s == record.station())
        })
        .filter_map(|record| {
            let mut facts = StationFacts::from_record(
                record,
                now as f64,
                registry.limits().stations.gone_dark_after.0,
                0,
            );
            facts.running = 0;
            facts.resources = None;
            let request = PlacementRequest {
                stations: vec![record.station().to_owned()],
                agent: sleeper.agent.clone(),
                role: sleeper.role.clone(),
                ..PlacementRequest::default()
            };
            (!placement::place(&policy, &request, &[facts])
                .choices
                .is_empty())
            .then(|| record.station().to_owned())
        })
        .collect()
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// Runs polls, reports and sleepers on a schedule; one pass at a time.
pub(crate) struct SourceDesk {
    busy: AtomicBool,
    due: Mutex<BTreeMap<(String, String), Instant>>,
    next_report: Mutex<Instant>,
    next_drain: Mutex<Instant>,
    next_replies: Mutex<Instant>,
    next_watch: Mutex<Instant>,
    drained: DrainState,
    next_sleepers: Mutex<Instant>,
    warned: Mutex<BTreeSet<String>>,
    /// This Station: the provenance of the briefs it queues names it.
    station: String,
    limits: SourceLimits,
}

impl SourceDesk {
    /// The desk of `station`, polling and reporting as `limits` says.
    pub(crate) fn new(station: impl Into<String>, limits: SourceLimits) -> Self {
        Self {
            station: station.into(),
            limits,
            busy: AtomicBool::new(false),
            due: Mutex::new(BTreeMap::new()),
            next_report: Mutex::new(Instant::now()),
            next_drain: Mutex::new(Instant::now()),
            next_replies: Mutex::new(Instant::now()),
            next_watch: Mutex::new(Instant::now()),
            drained: DrainState::new(),
            next_sleepers: Mutex::new(Instant::now()),
            warned: Mutex::new(BTreeSet::new()),
        }
    }

    /// Claims the single pass slot; `false` means a previous pass is still running.
    pub(crate) fn begin(&self) -> bool {
        !self.busy.swap(true, Ordering::AcqRel)
    }

    pub(crate) fn end(&self) {
        self.busy.store(false, Ordering::Release);
    }

    /// Whether any Operation has a source or a sleeper, so an idle Station spawns nothing.
    pub(crate) fn has_work(roster: &Roster) -> bool {
        roster
            .operations()
            .any(|operation| !operation.sources.is_empty() || !operation.sleepers.is_empty())
    }

    /// Polls every source that is due and posts pending reports. Blocks on the network.
    pub(crate) fn run(&self, plugins: &Plugins, roster: &Roster, registry: &dyn RegistryBackend) {
        let build =
            |provider: &str, settings: &toml::Table| build_source(plugins, provider, settings);
        let build_tracker =
            |provider: &str, settings: &toml::Table| build_tracker(plugins, provider, settings);
        self.run_with(roster, registry, &build, &build_tracker);
    }

    pub(crate) fn run_with(
        &self,
        roster: &Roster,
        registry: &dyn RegistryBackend,
        build: &SourceBuilder<'_>,
        build_tracker: &TrackerBuilder<'_>,
    ) {
        let now = Instant::now();
        for operation in roster.operations() {
            let setups = source_setups(operation, &self.limits);
            let providers: Vec<String> = setups
                .iter()
                .flatten()
                .map(|setup| setup.provider.clone())
                .collect();
            for setup in setups {
                let setup = match setup {
                    Ok(setup) => setup,
                    Err(error) => {
                        eprintln!("ai6: {error}");
                        continue;
                    }
                };
                if !setup.intake {
                    continue;
                }
                let key = (operation.id.clone(), setup.name.clone());
                let due = self
                    .due
                    .lock()
                    .ok()
                    .and_then(|due| due.get(&key).copied())
                    .is_none_or(|at| at <= now);
                if !due {
                    continue;
                }
                if let Ok(mut due) = self.due.lock() {
                    due.insert(key, Instant::now() + setup.interval);
                }
                let result = build(&setup.provider, &setup.settings).and_then(|source| {
                    poll_source(
                        registry,
                        &self.station,
                        (&operation.id, &setup.name),
                        providers.iter().filter(|p| **p == setup.provider).count() > 1,
                        source.as_ref(),
                        &setup.reopening(),
                    )
                });
                match result {
                    Ok(summary) if summary.queued + summary.updated + summary.reopened > 0 => {
                        eprintln!(
                            "ai6: source {} for Operation {}: {} queued, {} updated, {} reopened",
                            setup.name,
                            operation.id,
                            summary.queued,
                            summary.updated,
                            summary.reopened
                        );
                    }
                    Ok(_) => {}
                    Err(error) => eprintln!(
                        "ai6: source {} for Operation {}: {error}",
                        setup.name, operation.id
                    ),
                }
            }
        }
        let report_due = self
            .next_report
            .lock()
            .map(|mut next| {
                let due = *next <= now;
                if due {
                    *next = now + self.limits.report_every.0;
                }
                due
            })
            .unwrap_or(false);
        if report_due {
            report_finished(registry, roster, &self.limits, unix_seconds() as f64);
        }
        let drain_due = self
            .next_drain
            .lock()
            .map(|mut next| {
                let due = *next <= now;
                if due {
                    *next = now + self.limits.outbox_every.0;
                }
                due
            })
            .unwrap_or(false);
        if drain_due {
            drain_outbox(
                registry,
                roster,
                build_tracker,
                &self.station,
                &self.limits,
                &self.drained,
                unix_seconds() as f64,
            );
        }
        let replies_due = self
            .next_replies
            .lock()
            .map(|mut next| {
                let due = *next <= now;
                if due {
                    *next = now + self.limits.reply_every.0;
                }
                due
            })
            .unwrap_or(false);
        if replies_due {
            poll_replies(
                registry,
                roster,
                build_tracker,
                &self.station,
                unix_seconds() as f64,
            );
        }
        let watch_due = self
            .next_watch
            .lock()
            .map(|mut next| {
                let due = *next <= now;
                if due {
                    *next = now + self.limits.reopen_watch_every.0;
                }
                due
            })
            .unwrap_or(false);
        if watch_due {
            watch_settled(registry, roster, build_tracker, &self.station, &self.limits);
        }
        let sleepers_due = self
            .next_sleepers
            .lock()
            .map(|mut next| {
                let due = *next <= now;
                if due {
                    *next = now + self.limits.sleeper_every.0;
                }
                due
            })
            .unwrap_or(false);
        if sleepers_due && let Ok(mut warned) = self.warned.lock() {
            fire_due_sleepers(registry, roster, &self.station, unix_seconds(), &mut warned);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use guatiao::library::{ProviderError, Registry};
    use guatiao::{Provider, Status};

    use super::*;
    use crate::directorates::registry::mission_issues::{
        OutboxEntry, OutboxKind, OutboxState, ReplyState,
    };
    use crate::directorates::registry::sources::{SourceBatch, SourceItem};
    use crate::directorates::registry::store::FileRegistry;
    use crate::{TrackerPost, TrackerUpdate};

    /// The UID of the Mission `handle` names.
    fn uid(registry: &dyn RegistryBackend, handle: &str) -> String {
        registry
            .resolve_mission(handle)
            .unwrap()
            .unwrap()
            .mission_id
    }

    /// Every comment the fake tracker was asked to post, as (item, body).
    static POSTED: Mutex<Vec<(String, String, Option<String>)>> = Mutex::new(Vec::new());
    /// Comments the fake tracker lists as written by AI6's own identity, for the failover check.
    static OWN_REPLIES: Mutex<Vec<crate::Reply>> = Mutex::new(Vec::new());
    static COMMENT_IDS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    /// Every update the fake tracker was asked to make.
    static UPDATED: Mutex<Vec<TrackerUpdate>> = Mutex::new(Vec::new());

    /// Serves the ids listed in its settings; remembers every comment and update it is given.
    #[derive(Provider)]
    #[provider(Source, Tracker, id = "fake", name = "fake", config = SourceConfig)]
    struct Fake {
        ids: Vec<String>,
        body: String,
    }

    impl TryFrom<SourceConfig> for Fake {
        type Error = ProviderError;

        fn try_from(config: SourceConfig) -> Result<Self, Self::Error> {
            let table: toml::Table = toml::from_str(&config.settings_toml)
                .map_err(|e| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &e.to_string()))?;
            let ids = table
                .get("ids")
                .and_then(toml::Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(|id| id.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let body = table
                .get("body")
                .and_then(toml::Value::as_str)
                .unwrap_or("Please look.")
                .to_owned();
            Ok(Self { ids, body })
        }
    }

    impl Source for Fake {
        fn poll(&self, request: SourcePoll) -> Result<SourceBatch, ProviderError> {
            let next = request
                .cursor
                .as_deref()
                .unwrap_or("0")
                .parse::<u32>()
                .unwrap_or(0)
                + 1;
            Ok(SourceBatch {
                items: self
                    .ids
                    .iter()
                    .map(|id| SourceItem {
                        id: id.clone(),
                        title: format!("Item {id}"),
                        body: self.body.clone(),
                        url: None,
                        labels: Vec::new(),
                        author: None,
                        updated: format!("2026-01-0{next}T00:00:00Z"),
                        code: None,
                        code_tag: None,
                    })
                    .collect(),
                cursor: Some(next.to_string()),
            })
        }
    }

    impl Tracker for Fake {
        fn capabilities(
            &self,
            _item: Option<String>,
        ) -> Result<crate::TrackerCapabilities, ProviderError> {
            Ok(crate::TrackerCapabilities {
                comment: true,
                edit_comment: true,
                transitions: vec!["Done".to_owned()],
                ..Default::default()
            })
        }

        fn issue(&self, _item: String) -> Result<crate::TrackerIssue, ProviderError> {
            Err(ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, "fake"))
        }

        fn replies(&self, request: crate::ReplyPoll) -> Result<crate::ReplyBatch, ProviderError> {
            Ok(crate::ReplyBatch {
                replies: OWN_REPLIES
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|reply| request.items.contains(&reply.item))
                    .cloned()
                    .collect(),
                cursor: None,
            })
        }

        fn post(&self, post: TrackerPost) -> Result<crate::PostedComment, ProviderError> {
            POSTED
                .lock()
                .unwrap()
                .push((post.item, post.body, post.edit.clone()));
            let id = post.edit.unwrap_or_else(|| {
                format!(
                    "c{}",
                    COMMENT_IDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                )
            });
            Ok(crate::PostedComment { id, url: None })
        }

        fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError> {
            UPDATED.lock().unwrap().push(update);
            Ok(())
        }

        fn create(&self, _spec: crate::IssueSpec) -> Result<crate::TrackerIssue, ProviderError> {
            Err(ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, "fake"))
        }

        fn link(&self, _link: crate::TrackerLink) -> Result<(), ProviderError> {
            Err(ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, "fake"))
        }
    }

    guatiao::local_providers!(
        id = "ai6-source-test-providers",
        version = "0.0.0",
        providers = [Fake]
    );

    fn plugins() -> Plugins {
        let mut registry = Registry::new("ai6-test", env!("CARGO_PKG_VERSION"));
        registry
            .register_local("ai6-source-test-providers", library)
            .expect("fake source should register");
        Plugins::from_registry(registry)
    }

    fn roster_with(cover: &str, sources: &str) -> (tempfile::TempDir, Roster) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("operations.toml"),
            format!(
                "[operation.G]\nclearance = \"field\"\ncover = \"{cover}\"\nsources = {sources}\n"
            ),
        )
        .unwrap();
        let roster = Roster::load(dir.path()).unwrap();
        (dir, roster)
    }

    fn finish_all(registry: &dyn RegistryBackend, ok: bool, outcome: serde_json::Value) {
        while let Some(brief) = registry.claim_new(None).unwrap() {
            registry.finish(&brief, &outcome, ok, None, None).unwrap();
        }
    }

    #[test]
    fn polling_queues_each_item_once_and_never_duplicates() {
        let plugins = plugins();
        let (_dir, roster) = roster_with(
            "covert",
            "{ fake = { ids = [\"1\", \"2\"], interval_secs = 10 } }",
        );
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let desk = SourceDesk::new("station-a", SourceLimits::default());

        desk.run(&plugins, &roster, &registry);
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 2);
        assert_eq!(
            registry.source_cursor("G", "fake").unwrap().as_deref(),
            Some("1")
        );
        assert_eq!(queued[0].operation.as_deref(), Some("G"));

        // Not yet due: nothing polls, nothing changes.
        desk.run(&plugins, &roster, &registry);
        assert_eq!(
            registry.source_cursor("G", "fake").unwrap().as_deref(),
            Some("1")
        );

        // Due again: the items changed (a later `updated`), so queued briefs update in place.
        desk.due.lock().unwrap().clear();
        desk.run(&plugins, &roster, &registry);
        assert_eq!(registry.list_new().unwrap().len(), 2);
        assert_eq!(
            registry.source_cursor("G", "fake").unwrap().as_deref(),
            Some("2")
        );

        // One item starts running; a further poll leaves it alone and queues no copy.
        let running = registry
            .claim_new(Some(&uid(&registry, "G-fake-1")))
            .unwrap()
            .unwrap();
        desk.due.lock().unwrap().clear();
        desk.run(&plugins, &roster, &registry);
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].shown_handle(), "G-fake-2");
        assert!(registry.read_claimed(&running.key()).unwrap().is_some());
    }

    /// A provider searching behind its cursor returns what it already returned.
    #[test]
    fn an_instance_that_shares_its_provider_names_its_items_itself() {
        struct Coded(&'static str, Option<&'static str>);
        impl Source for Coded {
            fn poll(&self, _request: SourcePoll) -> Result<SourceBatch, ProviderError> {
                Ok(SourceBatch {
                    items: vec![SourceItem {
                        id: format!("item-{}", self.0),
                        title: "Item".to_owned(),
                        body: "Please look.".to_owned(),
                        url: None,
                        labels: Vec::new(),
                        author: None,
                        updated: "2026-01-01T00:00:00Z".to_owned(),
                        code: Some(self.0.to_owned()),
                        code_tag: self.1.map(str::to_owned),
                    }],
                    cursor: None,
                })
            }
        }
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let keys = |instance: &str, shared: bool, source: Coded| {
            poll_source(
                &registry,
                "station-a",
                ("ai6", instance),
                shared,
                &source,
                &Reopening::never(),
            )
            .unwrap();
            registry
                .entered_mission_id(&MissionEntry::source(
                    "ai6",
                    instance,
                    &format!("item-{}", source.0),
                    None,
                    None,
                ))
                .unwrap()
                .map(|uid| registry.resolve_mission(&uid).unwrap().unwrap().handle)
                .unwrap()
        };
        assert_eq!(
            keys("github", false, Coded("63", Some("gh"))),
            "ai6-gh-63",
            "the provider's only instance goes by the provider's short name"
        );
        assert_eq!(
            keys("backend", true, Coded("64", Some("gh"))),
            "ai6-backend-64"
        );
        // A key that names its project needs no instance beside it.
        assert_eq!(keys("tickets", true, Coded("AI6-65", None)), "ai6-AI6-65");
    }

    #[test]
    fn a_repeated_item_makes_no_second_brief_or_mission() {
        struct Repeating;
        impl Source for Repeating {
            fn poll(&self, _request: SourcePoll) -> Result<SourceBatch, ProviderError> {
                Ok(SourceBatch {
                    items: vec![SourceItem {
                        id: "o/r#1".to_owned(),
                        title: "Item".to_owned(),
                        body: "Please look.".to_owned(),
                        url: None,
                        labels: Vec::new(),
                        author: None,
                        updated: "2026-01-01T00:00:00Z".to_owned(),
                        code: None,
                        code_tag: None,
                    }],
                    cursor: Some("2026-01-01T00:00:00Z".to_owned()),
                })
            }
        }
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let poll = || {
            poll_source(
                &registry,
                "station-a",
                ("G", "repeating"),
                false,
                &Repeating,
                &Reopening::never(),
            )
            .unwrap()
        };

        assert_eq!(poll().queued, 1);
        let again = poll();
        assert_eq!((again.queued, again.updated, again.unchanged), (0, 0, 1));
        assert_eq!(registry.list_new().unwrap().len(), 1);

        let running = registry.claim_new(None).unwrap().unwrap();
        let claimed = poll();
        assert_eq!((claimed.queued, claimed.skipped), (0, 1));
        assert!(registry.list_new().unwrap().is_empty());

        registry
            .finish(&running, &serde_json::json!({}), true, None, None)
            .unwrap();
        let finished = poll();
        assert_eq!((finished.queued, finished.skipped), (0, 1));
        assert!(registry.claim_new(None).unwrap().is_none());
    }

    #[test]
    fn a_string_source_is_shorthand_for_a_target() {
        let (_dir, roster) = roster_with("covert", "{ fake = \"GUA\" }");
        let setups = source_setups(roster.operation("G").unwrap(), &SourceLimits::default());
        let setup = setups.into_iter().next().unwrap().unwrap();
        assert_eq!(setup.settings["target"].as_str(), Some("GUA"));
        assert_eq!(setup.interval, SourceLimits::default().poll.0);
    }

    #[test]
    fn interval_secs_is_clamped_and_validated() {
        let (_dir, roster) = roster_with("covert", "{ fake = { interval_secs = 1 } }");
        let limits = SourceLimits::default();
        let setup = source_setups(roster.operation("G").unwrap(), &limits)
            .remove(0)
            .unwrap();
        assert_eq!(setup.interval, limits.min_poll.0);
        assert!(!setup.settings.contains_key("interval_secs"));
        let looser = SourceLimits {
            min_poll: crate::Seconds::new(0),
            ..SourceLimits::default()
        };
        let setup = source_setups(roster.operation("G").unwrap(), &looser)
            .remove(0)
            .unwrap();
        assert_eq!(setup.interval, Duration::from_secs(1));
        let (_dir, roster) = roster_with("covert", "{ fake = { interval_secs = \"soon\" } }");
        assert!(
            source_setups(roster.operation("G").unwrap(), &limits)
                .remove(0)
                .is_err()
        );
    }

    #[test]
    fn a_finished_mission_is_reported_once_with_cover_aware_wording() {
        let plugins = plugins();
        let (_dir, roster) = roster_with("overt", "{ fake = { ids = [\"report-me\"] } }");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        desk.run(&plugins, &roster, &registry);
        finish_all(
            &registry,
            true,
            serde_json::json!({
                "state": "done",
                "mission_id": "G-fake-report-me",
                "session_ids": ["sess-1"],
                "agent": "007",
                "branch": "agent/fix",
            }),
        );
        desk.due.lock().unwrap().clear();
        *desk.next_report.lock().unwrap() = Instant::now();
        *desk.next_drain.lock().unwrap() = Instant::now();
        desk.run(&plugins, &roster, &registry);
        desk.due.lock().unwrap().clear();
        *desk.next_report.lock().unwrap() = Instant::now();
        *desk.next_drain.lock().unwrap() = Instant::now();
        desk.run(&plugins, &roster, &registry);

        let posted = POSTED.lock().unwrap();
        let mine: Vec<_> = posted
            .iter()
            .filter(|(item, _, _)| item == "report-me")
            .collect();
        assert_eq!(mine.len(), 1);
        assert!(mine[0].1.contains("ended: done"));
        assert!(mine[0].1.contains("agent/fix"));
        assert!(mine[0].1.contains("007"));
        assert!(mine[0].1.contains("sess-1"));
    }

    #[test]
    fn a_report_applies_the_status_label_and_transition_of_the_outcome() {
        let plugins = plugins();
        let (_dir, roster) = roster_with(
            "overt",
            "{ fake = { ids = [\"labelled\"], status_labels = { done = \"ai6:done\", failed = \"ai6:failed\" }, status_transitions = { done = \"Done\" } } }",
        );
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        desk.run(&plugins, &roster, &registry);
        finish_all(
            &registry,
            true,
            serde_json::json!({ "state": "done", "mission_id": "m" }),
        );
        desk.due.lock().unwrap().clear();
        *desk.next_report.lock().unwrap() = Instant::now();
        *desk.next_drain.lock().unwrap() = Instant::now();
        desk.run(&plugins, &roster, &registry);
        let updated = UPDATED.lock().unwrap();
        let update = updated
            .iter()
            .find(|update| update.item == "labelled")
            .expect("the outcome's label and transition were written");
        assert_eq!(update.add_labels, ["ai6:done"]);
        assert_eq!(update.transition.as_deref(), Some("Done"));
    }

    #[test]
    fn an_instance_name_may_differ_from_its_provider_and_intake_can_be_off() {
        let plugins = plugins();
        let (_dir, roster) = roster_with(
            "covert",
            "{ board = { provider = \"fake\", ids = [\"1\"] }, mirror = { provider = \"fake\", ids = [\"2\"], intake = false } }",
        );
        let setups: Vec<_> =
            source_setups(roster.operation("G").unwrap(), &SourceLimits::default())
                .into_iter()
                .map(Result::unwrap)
                .collect();
        let board = setups.iter().find(|setup| setup.name == "board").unwrap();
        assert_eq!(board.provider, "fake");
        assert!(board.intake);
        assert!(!board.settings.contains_key("provider"));
        assert!(!board.settings.contains_key("intake"));
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        desk.run(&plugins, &roster, &registry);
        let queued = registry.list_new().unwrap();
        assert_eq!(
            queued.len(),
            1,
            "the intake = false instance is never polled"
        );
        assert_eq!(queued[0].shown_handle(), "G-board-1");
        assert!(registry.source_cursor("G", "mirror").unwrap().is_none());
        let (_dir, roster) = roster_with("covert", "{ board = { provider = 3 } }");
        assert!(
            source_setups(roster.operation("G").unwrap(), &SourceLimits::default())
                .remove(0)
                .is_err()
        );
    }

    /// 2026-10-25T10:00:00Z, a Sunday.
    const NOW: i64 = 1_792_922_400;

    #[test]
    fn an_authorized_sleeper_queues_one_brief_per_window_across_failover() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("operations.toml"),
            "[operation.G]\nclearance = \"field\"\n\
             [operation.G.sleepers.deps]\nschedule = \"0 3 * * *\"\ntitle = \"Update dependencies\"\n\
             prompt = \"Update the lockfile.\"\n",
        )
        .unwrap();
        let roster = Roster::load(dir.path()).unwrap();
        let sleeper = roster.operation("G").unwrap().sleepers["deps"].clone();
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let mut warned = BTreeSet::new();

        // Nothing fires before an operator approves this exact definition.
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", NOW, &mut warned),
            0
        );
        assert_eq!(warned.len(), 1);

        let window = sleeper.due_window(NOW, 0, &Default::default()).unwrap();
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", window - 60)
            .unwrap();
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", NOW, &mut warned),
            1
        );
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", NOW + 600, &mut warned),
            0
        );

        // A Station that takes over the Headquarters lease evaluates the same window.
        let takeover = FileRegistry::open(state.path());
        let mut fresh = BTreeSet::new();
        assert_eq!(
            fire_due_sleepers(&takeover, &roster, "station-a", NOW + 1200, &mut fresh),
            0
        );
        assert_eq!(takeover.list_new().unwrap().len(), 1);

        // Tomorrow's window is a new brief.
        assert_eq!(
            fire_due_sleepers(&takeover, &roster, "station-a", NOW + 86_400, &mut fresh),
            1
        );
        assert_eq!(takeover.list_new().unwrap().len(), 2);
    }

    /// 2026-10-25T03:00:00Z, a window of the nightly Sleeper below.
    const MARK: i64 = 1_792_897_200;
    const DAY: i64 = 86_400;

    /// A registry and a roster whose Operation `G` has the Sleeper `deps` on `schedule`.
    fn order(schedule: &str, extra: &str) -> (tempfile::TempDir, Roster, Sleeper) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("operations.toml"),
            format!(
                "[operation.G]\nclearance = \"field\"\n\
                 [operation.G.sleepers.deps]\nschedule = \"{schedule}\"\ntitle = \"Update\"\n\
                 prompt = \"Update the lockfile.\"\n{extra}"
            ),
        )
        .unwrap();
        let roster = Roster::load(dir.path()).unwrap();
        let sleeper = roster.operation("G").unwrap().sleepers["deps"].clone();
        (dir, roster, sleeper)
    }

    /// The skipped windows on the timeline, as (window, why).
    fn skipped(registry: &FileRegistry) -> Vec<(String, String)> {
        registry
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "sleeper" && event.action == "skipped")
            .map(|event| (event.detail["window"].clone(), event.detail["why"].clone()))
            .collect()
    }

    #[test]
    fn a_missed_mark_fires_once_late_and_every_window_it_skipped_is_recorded() {
        let (_dir, roster, sleeper) = order("0 3 * * *", "");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", MARK - DAY)
            .unwrap();
        let mut warned = BTreeSet::new();
        let fire = |now: i64, warned: &mut BTreeSet<String>| {
            fire_due_sleepers(&registry, &roster, "station-a", now, warned)
        };
        assert_eq!(fire(MARK + 60, &mut warned), 1);
        assert!(skipped(&registry).is_empty());

        // The scheduler is down for three marks and comes back five hours after the last.
        let back = MARK + 3 * DAY + 5 * 3600;
        assert_eq!(fire(back, &mut warned), 1, "one late firing, of the latest");
        assert_eq!(fire(back + 60, &mut warned), 0, "never more than one");
        let late: Vec<String> = registry
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "sleeper" && event.action == "late")
            .map(|event| event.detail["window"].clone())
            .collect();
        assert_eq!(late, [window_label(MARK + 3 * DAY)]);
        assert_eq!(
            skipped(&registry),
            [MARK + DAY, MARK + 2 * DAY].map(|window| (
                window_label(window),
                "the scheduler was not running".to_owned()
            ))
        );
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 2);
        assert!(
            queued
                .iter()
                .any(|brief| brief.created == window_label(MARK + 3 * DAY))
        );
    }

    #[test]
    fn a_weekly_mark_missed_by_days_still_fires_within_its_own_period() {
        let (_dir, roster, sleeper) = order("0 3 * * 0", "");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", MARK - DAY)
            .unwrap();
        let mut warned = BTreeSet::new();
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", MARK + 3 * DAY, &mut warned),
            1
        );
    }

    #[test]
    fn a_mark_past_the_bound_is_recorded_and_does_not_fire() {
        // Weekdays at 09:00: Friday's window is a day after Thursday's, so on Sunday morning,
        // two days old, it is past its own period.
        let (_dir, roster, sleeper) = order("0 9 * * 1-5", "");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", MARK - 7 * DAY)
            .unwrap();
        let mut warned = BTreeSet::new();
        let thursday = MARK - 3 * DAY + 6 * 3600;
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", thursday, &mut warned),
            1
        );
        let sunday = MARK + 7 * 3600;
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", sunday, &mut warned),
            0
        );
        assert_eq!(
            skipped(&registry),
            [(
                window_label(MARK - 2 * DAY + 6 * 3600),
                "older than its catch-up bound".to_owned()
            )]
        );
    }

    #[test]
    fn windows_an_order_was_unauthorized_for_are_recorded_and_the_latest_fires_once_authorized() {
        let (_dir, roster, sleeper) = order("0 3 * * *", "");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", MARK - DAY)
            .unwrap();
        let mut warned = BTreeSet::new();
        let fire = |now: i64, warned: &mut BTreeSet<String>| {
            fire_due_sleepers(&registry, &roster, "station-a", now, warned)
        };
        assert_eq!(fire(MARK + 60, &mut warned), 1);

        // The definition is edited and its approval no longer covers it.
        let edited = Sleeper {
            prompt: "Something else.".to_owned(),
            ..sleeper.clone()
        };
        registry
            .authorize_sleeper("G", "deps", &edited, "operator", MARK + 60)
            .unwrap();
        assert_eq!(fire(MARK + DAY + 60, &mut warned), 0);
        assert_eq!(fire(MARK + 2 * DAY + 60, &mut warned), 0);
        assert_eq!(
            skipped(&registry),
            [(window_label(MARK + DAY), "not authorized".to_owned())]
        );

        // Approved again that morning: the window it missed fires once.
        let approved = MARK + 2 * DAY + 3 * 3600;
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", approved)
            .unwrap();
        assert_eq!(fire(approved + 60, &mut warned), 1);
        assert_eq!(fire(approved + 120, &mut warned), 0);
    }

    #[test]
    fn past_the_setting_a_skipped_stretch_is_one_event() {
        let (_dir, roster, sleeper) = order("0 3 * * *", "");
        let state = tempfile::tempdir().unwrap();
        let mut limits = crate::directorates::registry::limits::RegistryLimits::default();
        limits.sleepers.skipped_events = crate::bounds::Limit::at(2);
        let registry = FileRegistry::open(state.path()).with_limits(limits);
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", MARK - DAY)
            .unwrap();
        let mut warned = BTreeSet::new();
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", MARK + 60, &mut warned),
            1
        );
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", MARK + 6 * DAY, &mut warned),
            1
        );
        let why = "the scheduler was not running".to_owned();
        assert_eq!(
            skipped(&registry),
            [
                (
                    format!("{} to {}", window_label(MARK), window_label(MARK + 4 * DAY)),
                    why.clone()
                ),
                (window_label(MARK + 4 * DAY), why.clone()),
                (window_label(MARK + 5 * DAY), why),
            ]
        );
    }

    #[test]
    fn an_explicit_bound_keeps_a_late_mark_from_firing() {
        let (_dir, roster, sleeper) = order("0 3 * * *", "");
        let state = tempfile::tempdir().unwrap();
        let mut limits = crate::directorates::registry::limits::RegistryLimits::default();
        limits.sleepers.catch_up = Some(crate::bounds::TimeLimit::secs(3600));
        let registry = FileRegistry::open(state.path()).with_limits(limits);
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", MARK - DAY)
            .unwrap();
        let mut warned = BTreeSet::new();
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", MARK + 60, &mut warned),
            1
        );
        assert_eq!(
            fire_due_sleepers(
                &registry,
                &roster,
                "station-a",
                MARK + DAY + 7200,
                &mut warned
            ),
            0,
            "none past the bound"
        );
        assert_eq!(
            skipped(&registry),
            [(
                window_label(MARK + DAY),
                "older than its catch-up bound".to_owned()
            )]
        );
        assert_eq!(
            fire_due_sleepers(
                &registry,
                &roster,
                "station-a",
                MARK + 2 * DAY + 1800,
                &mut warned
            ),
            1,
            "within the bound"
        );
        assert_eq!(skipped(&registry).len(), 1, "told once");
    }

    #[test]
    fn a_first_approval_covers_only_the_windows_after_it() {
        let (_dir, roster, sleeper) = order("0 3 * * *", "");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let approved = MARK + 3600;
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", approved)
            .unwrap();
        let mut warned = BTreeSet::new();
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", approved + 60, &mut warned),
            0
        );
        assert!(skipped(&registry).is_empty());
        assert_eq!(
            fire_due_sleepers(
                &registry,
                &roster,
                "station-a",
                MARK + DAY + 60,
                &mut warned
            ),
            1
        );
        assert!(skipped(&registry).is_empty(), "nothing was missed");
    }

    #[test]
    fn a_revoked_order_owes_no_window_when_it_is_approved_again() {
        let (_dir, roster, sleeper) = order("0 3 * * *", "");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", MARK - DAY)
            .unwrap();
        let mut warned = BTreeSet::new();
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", MARK + 60, &mut warned),
            1
        );
        assert!(registry.revoke_sleeper("G", "deps").unwrap());
        assert_eq!(
            fire_due_sleepers(
                &registry,
                &roster,
                "station-a",
                MARK + DAY + 60,
                &mut warned
            ),
            0
        );
        let approved = MARK + 2 * DAY + 3600;
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", approved)
            .unwrap();
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-a", approved + 60, &mut warned),
            0,
            "the operator stopped it; what passed since is not owed"
        );
        assert!(skipped(&registry).is_empty());
    }

    #[test]
    fn a_window_that_passes_with_an_attempt_in_flight_is_recorded_when_the_next_one_comes() {
        let (_dir, roster, sleeper) = order("0 3 * * *", "standing = true\n");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", MARK - DAY)
            .unwrap();
        let mut warned = BTreeSet::new();
        let fire = |now: i64, warned: &mut BTreeSet<String>| {
            fire_due_sleepers(&registry, &roster, "station-a", now, warned)
        };
        assert_eq!(fire(MARK + 60, &mut warned), 1);
        assert_eq!(fire(MARK + DAY + 60, &mut warned), 0);
        assert!(
            skipped(&registry).is_empty(),
            "it still fires if the attempt settles before the next window"
        );
        assert_eq!(fire(MARK + 2 * DAY + 60, &mut warned), 0);
        assert_eq!(
            skipped(&registry),
            [(
                window_label(MARK + DAY),
                "the last attempt was in flight".to_owned()
            )]
        );
    }

    #[test]
    fn an_each_station_sleeper_rounds_every_eligible_station_once_per_window_across_failover() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("operations.toml"),
            "[operation.H]\nclearance = \"field\"\n\
             [operation.H.sleepers.rounds]\nschedule = \"0 * * * *\"\ntitle = \"Station round\"\n\
             role = \"head-of-station\"\neach_station = true\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("agents.toml"),
            "[agent.M]\nhandler = \"mock\"\nclearance = \"field\"\nroles = [\"head-of-station\"]\n\
             [role.head-of-station]\npurpose = \"Keep the Station fit.\"\npolicy = \"Report.\"\n",
        )
        .unwrap();
        let roster = Roster::load(dir.path()).unwrap();
        let sleeper = roster.operation("H").unwrap().sleepers["rounds"].clone();
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let check_in = |station: &str, roles: &[&str], handlers: &[&str], running: usize| {
            registry
                .check_in(
                    station,
                    BTreeMap::from([
                        ("roles".to_owned(), serde_json::json!(roles)),
                        ("capacity".to_owned(), serde_json::json!(2)),
                        ("running".to_owned(), serde_json::json!(running)),
                        ("handlers".to_owned(), serde_json::json!(handlers)),
                        (
                            "theaters".to_owned(),
                            serde_json::json!([
                                crate::directorates::m::theaters::TheaterSummary::local()
                            ]),
                        ),
                    ]),
                )
                .unwrap();
        };
        // A full Station still gets its round; it runs it when it has room.
        check_in("station-a", &["execution"], &["mock"], 2);
        check_in("station-b", &["control", "execution"], &["mock"], 0);
        check_in("station-c", &["control"], &["mock"], 0);
        check_in("station-d", &["execution"], &["other"], 0);
        // Just after the hour, so every call below evaluates the same window while the
        // check-ins, taken at the real time, are still fresh.
        let real = unix_seconds();
        let now = real - real.rem_euclid(3600) + 1;
        let window = sleeper.due_window(now, 0, &Default::default()).unwrap();
        registry
            .authorize_sleeper("H", "rounds", &sleeper, "operator", window - 60)
            .unwrap();

        let mut warned = BTreeSet::new();
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-b", now, &mut warned),
            2
        );
        assert_eq!(
            fire_due_sleepers(&registry, &roster, "station-b", now + 30, &mut warned),
            0
        );
        // The Station that takes over the Headquarters lease fires no second round.
        let takeover = FileRegistry::open(state.path());
        let mut fresh = BTreeSet::new();
        assert_eq!(
            fire_due_sleepers(&takeover, &roster, "station-a", now + 40, &mut fresh),
            0
        );
        let mut queued = takeover.list_new().unwrap();
        queued.sort_by_key(|brief| brief.key());
        let rounds = queued
            .iter()
            .map(|brief| (brief.stations.clone(), brief.role.clone()))
            .collect::<Vec<_>>();
        let role = Some("head-of-station".to_owned());
        assert_eq!(
            rounds,
            [
                (vec!["station-a".to_owned()], role.clone()),
                (vec!["station-b".to_owned()], role)
            ]
        );
        // A Station that has gone dark is held no round.
        assert!(round_stations(&registry, &roster, "H", &sleeper, real + 120).is_empty());
        // A sleeper's own `stations` narrows the rounds.
        let narrowed = Sleeper {
            stations: vec!["station-b".to_owned()],
            ..sleeper
        };
        assert_eq!(
            round_stations(&registry, &roster, "H", &narrowed, now),
            ["station-b"]
        );
    }

    #[test]
    fn editing_a_sleeper_withdraws_its_approval() {
        let dir = tempfile::tempdir().unwrap();
        let write = |prompt: &str| {
            fs::write(
                dir.path().join("operations.toml"),
                format!(
                    "[operation.G]\n[operation.G.sleepers.deps]\nschedule = \"0 3 * * *\"\n\
                     title = \"t\"\nprompt = \"{prompt}\"\n"
                ),
            )
            .unwrap();
            Roster::load(dir.path()).unwrap()
        };
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let approved = write("one");
        let sleeper = approved.operation("G").unwrap().sleepers["deps"].clone();
        registry
            .authorize_sleeper("G", "deps", &sleeper, "operator", 0)
            .unwrap();
        let edited = write("two");
        let mut warned = BTreeSet::new();
        assert_eq!(
            fire_due_sleepers(&registry, &edited, "station-a", NOW, &mut warned),
            0
        );
        assert_eq!(
            fire_due_sleepers(&registry, &approved, "station-a", NOW, &mut warned),
            1
        );
    }

    #[test]
    fn a_bad_schedule_fails_the_roster_load() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("operations.toml"),
            "[operation.G]\n[operation.G.sleepers.deps]\nschedule = \"every night\"\ntitle = \"t\"\n",
        )
        .unwrap();
        let error = Roster::load(dir.path()).unwrap_err().to_string();
        assert!(
            error.contains("sleeper") && error.contains("schedule"),
            "{error}"
        );
    }

    fn finished(ok: bool) -> FinishedSourceBrief {
        let item = SourceItem {
            id: "o/r#9".to_owned(),
            title: "t".to_owned(),
            body: "b".to_owned(),
            url: None,
            labels: Vec::new(),
            author: None,
            updated: "t".to_owned(),
            code: None,
            code_tag: None,
        };
        FinishedSourceBrief {
            brief: crate::source_brief("G", "github", &item, &Default::default()),
            outcome: serde_json::json!({
                "state": if ok { "done" } else { "failed" },
                "mission_id": "m-9",
                "session_ids": ["sess-9"],
                "agent": "007",
                "branch": "agent/x",
            }),
            ok,
        }
    }

    /// A brief no Mission ran for has no Mission to name.
    #[test]
    fn a_report_of_a_brief_that_never_became_a_mission_names_no_mission() {
        let finished = FinishedSourceBrief {
            brief: crate::directorates::registry::sources::source_brief(
                "G",
                "gh",
                &crate::directorates::registry::sources::SourceItem {
                    id: "o/r#7".to_owned(),
                    title: "t".to_owned(),
                    body: "b".to_owned(),
                    url: None,
                    labels: Vec::new(),
                    author: None,
                    updated: "u".to_owned(),
                    code: None,
                    code_tag: None,
                },
                &Default::default(),
            ),
            outcome: serde_json::json!({
                "state": "failed",
                "error": "no available Agent matches role \"any role\"",
            }),
            ok: false,
        };
        for disclosure in [Disclosure::Hidden, Disclosure::Assisted, Disclosure::Full] {
            let report = compose_report(disclosure, &finished);
            assert!(!report.body.contains("``"), "{}", report.body);
            assert!(!report.body.contains("Mission: "), "{}", report.body);
        }
        let full = compose_report(Disclosure::Full, &finished);
        assert_eq!(full.body, "AI6 could not start a Mission for this item.");
    }

    #[test]
    fn a_report_says_what_the_harness_refused_unless_the_cover_is_hidden() {
        let mut entry = finished(true);
        entry.outcome["denied_tools"] = serde_json::json!(["Write", "Bash"]);
        let line = "Refused by its permission rules: Write, Bash";
        for disclosure in [Disclosure::Assisted, Disclosure::Full] {
            let body = compose_report(disclosure, &entry).body;
            assert!(body.contains(line), "{body}");
        }
        let hidden = compose_report(Disclosure::Hidden, &entry).body;
        assert!(!hidden.contains("Write"), "{hidden}");
        entry.outcome["denied_tools"] = serde_json::json!([]);
        let none = compose_report(Disclosure::Full, &entry).body;
        assert!(!none.contains("Refused"), "{none}");
    }

    #[test]
    fn covert_and_partial_reports_name_neither_agent_nor_session() {
        for disclosure in [Disclosure::Hidden, Disclosure::Assisted] {
            let report = compose_report(disclosure, &finished(true));
            assert!(!report.body.contains("007"), "{}", report.body);
            assert!(!report.body.contains("sess-9"), "{}", report.body);
            assert!(report.body.contains("agent/x"));
            assert_eq!(report.item_id, "o/r#9");
        }
        let hidden = compose_report(Disclosure::Hidden, &finished(true));
        assert!(!hidden.body.contains("AI"));
        assert!(!hidden.body.contains("m-9"));
        let assisted = compose_report(Disclosure::Assisted, &finished(false));
        assert!(assisted.body.contains("AI-assisted"));
        assert!(assisted.body.contains("m-9"));
        let full = compose_report(Disclosure::Full, &finished(true));
        assert!(full.body.contains("sess-9") && full.body.contains("007"));
    }

    fn roster_issues(sources: &str, issues: &str) -> (tempfile::TempDir, Roster) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("operations.toml"),
            format!(
                "[operation.G]\nclearance = \"field\"\ncover = \"overt\"\nsources = {sources}\n\
                 [operation.G.issues]\n{issues}\n"
            ),
        )
        .unwrap();
        let roster = Roster::load(dir.path()).unwrap();
        (dir, roster)
    }

    /// One desk pass with the poll, report and drain timers all due.
    fn pass(desk: &SourceDesk, plugins: &Plugins, roster: &Roster, registry: &FileRegistry) {
        desk.due.lock().unwrap().clear();
        *desk.next_report.lock().unwrap() = Instant::now();
        *desk.next_drain.lock().unwrap() = Instant::now();
        desk.drained.last_status.lock().unwrap().clear();
        desk.run(plugins, roster, registry);
    }

    fn posts_to(item: &str) -> Vec<(String, Option<String>)> {
        POSTED
            .lock()
            .unwrap()
            .iter()
            .filter(|(posted, _, _)| posted == item)
            .map(|(_, body, edit)| (body.clone(), edit.clone()))
            .collect()
    }

    fn updates_to(item: &str) -> Vec<TrackerUpdate> {
        UPDATED
            .lock()
            .unwrap()
            .iter()
            .filter(|update| update.item == item)
            .cloned()
            .collect()
    }

    #[test]
    fn polling_links_the_source_issue_to_the_mission_its_brief_will_get() {
        let plugins = plugins();
        let (_dir, roster) = roster_with("covert", "{ fake = { ids = [\"lk-1\"] } }");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        desk.run(&plugins, &roster, &registry);
        let links = registry
            .issue_links(&uid(&registry, "G-fake-lk-1"))
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].relation, LinkRelation::Source);
        assert_eq!(
            (links[0].tracker.as_str(), links[0].item.as_str()),
            ("fake", "lk-1")
        );
        assert_eq!(links[0].linked_by, "m");
        // A person's unlink holds: the next poll does not link it again.
        registry
            .unlink_issue(&uid(&registry, "G-fake-lk-1"), "fake", "lk-1")
            .unwrap();
        pass(&desk, &plugins, &roster, &registry);
        assert!(registry.active_issue_links().unwrap().is_empty());
    }

    #[test]
    fn a_status_comment_names_the_missions_branch() {
        let (_dir, roster) = roster_issues("{ fake = { ids = [\"br-1\", \"br-2\"] } }", "");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        desk.run(&plugins(), &roster, &registry);
        let status = |mission: &str, branch: Option<&str>| {
            crate::station::issues::queue_status(
                &registry, &roster, mission, "working", branch, 1.0,
            );
            let entries = registry.outbox_entries(Some(mission)).unwrap();
            entries
                .iter()
                .find(|entry| entry.kind == OutboxKind::Status)
                .unwrap()
                .body
                .clone()
        };
        let named = status(&uid(&registry, "G-fake-br-1"), Some("agent/br-1"));
        assert!(
            named.ends_with(
                "
Branch: `agent/br-1`"
            ),
            "{named}"
        );
        let none = status(&uid(&registry, "G-fake-br-2"), None);
        assert!(!none.contains("Branch"), "{none}");
    }

    #[test]
    fn one_status_comment_is_posted_then_edited_through_working_waiting_working() {
        let plugins = plugins();
        let (_dir, roster) = roster_issues(
            "{ fake = { ids = [\"st-1\"], status_labels = { working = \"ai6:working\", waiting = \"ai6:waiting\" } } }",
            "",
        );
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        desk.run(&plugins, &roster, &registry);
        let mission = &uid(&registry, "G-fake-st-1");

        for phase in ["working", "waiting", "working"] {
            assert_eq!(
                crate::station::issues::queue_status(&registry, &roster, mission, phase, None, 1.0),
                1
            );
            pass(&desk, &plugins, &roster, &registry);
        }
        let posts = posts_to("st-1");
        assert_eq!(posts.len(), 3, "{posts:?}");
        assert_eq!(posts[0].1, None, "the first status is a new comment");
        let comment = registry.issue_links(mission).unwrap()[0]
            .status_comment
            .clone()
            .unwrap();
        assert_eq!(
            posts[1].1.as_deref(),
            Some(comment.as_str()),
            "later ones edit it"
        );
        assert_eq!(posts[2].1.as_deref(), Some(comment.as_str()));
        assert!(posts[1].0.contains("waiting"), "{}", posts[1].0);
        let updates = updates_to("st-1");
        assert_eq!(updates.len(), 3);
        assert_eq!(updates[0].add_labels, ["ai6:working"]);
        assert_eq!(updates[0].remove_labels, ["ai6:waiting"]);
        assert_eq!(updates[1].add_labels, ["ai6:waiting"]);
        assert_eq!(updates[1].remove_labels, ["ai6:working"]);
        assert_eq!(updates[2].add_labels, ["ai6:working"]);
        assert_eq!(
            registry.issue_links(mission).unwrap()[0].phase.as_deref(),
            Some("working")
        );
        let entries = registry.outbox_entries(Some(mission)).unwrap();
        assert!(
            entries
                .iter()
                .all(|entry| entry.state == OutboxState::Posted)
        );
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn status_writes_to_one_issue_are_spaced_by_the_policy() {
        let plugins = plugins();
        let (_dir, roster) = roster_issues("{ fake = { ids = [\"sp-1\"] } }", "");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        desk.run(&plugins, &roster, &registry);
        let mission = &uid(&registry, "G-fake-sp-1");
        crate::station::issues::queue_status(&registry, &roster, mission, "working", None, 1.0);
        pass(&desk, &plugins, &roster, &registry);
        crate::station::issues::queue_status(&registry, &roster, mission, "paused", None, 2.0);
        // Not cleared this time: the issue was written a moment ago.
        desk.due.lock().unwrap().clear();
        *desk.next_drain.lock().unwrap() = Instant::now();
        desk.run(&plugins, &roster, &registry);
        assert_eq!(posts_to("sp-1").len(), 1, "the second status waits");
        assert_eq!(
            registry
                .outbox_entries(Some(mission))
                .unwrap()
                .iter()
                .filter(|entry| entry.state == OutboxState::Pending)
                .count(),
            1
        );
    }

    #[test]
    fn updates_outcome_writes_no_status_and_all_writes_every_link() {
        let (_dir, roster) =
            roster_issues("{ fake = { ids = [\"oc-1\"] } }", "updates = \"outcome\"");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry
            .link_issue(&source_link("G", "m-oc", "fake", "oc-1", None, 1.0))
            .unwrap();
        assert_eq!(
            crate::station::issues::queue_status(&registry, &roster, "m-oc", "working", None, 1.0),
            0
        );
        let (_dir, roster) = roster_issues("{ fake = { ids = [\"al-1\"] } }", "updates = \"all\"");
        let mut child = source_link("G", "m-al", "fake", "al-2", None, 1.0);
        child.relation = LinkRelation::Child;
        registry
            .link_issue(&source_link("G", "m-al", "fake", "al-1", None, 1.0))
            .unwrap();
        registry.link_issue(&child).unwrap();
        assert_eq!(
            crate::station::issues::queue_status(&registry, &roster, "m-al", "working", None, 1.0),
            2
        );
        assert_eq!(
            crate::station::issues::queue_status(&registry, &roster, "m-al", "done", None, 2.0),
            1,
            "the report covers the source's end; the child gets a status"
        );
        // Milestones (the default) writes the source only, and not its end.
        let (_dir, roster) = roster_issues("{ fake = { ids = [\"ms-1\"] } }", "");
        registry
            .link_issue(&source_link("G", "m-ms", "fake", "ms-1", None, 1.0))
            .unwrap();
        assert_eq!(
            crate::station::issues::queue_status(&registry, &roster, "m-ms", "waiting", None, 1.0),
            1
        );
        assert_eq!(
            crate::station::issues::queue_status(&registry, &roster, "m-ms", "done", None, 2.0),
            0
        );
    }

    #[test]
    fn the_report_is_a_new_comment_and_is_sent_once_across_a_headquarters_change() {
        let plugins = plugins();
        let (_dir, roster) = roster_with("overt", "{ fake = { ids = [\"fo-1\"] } }");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        desk.run(&plugins, &roster, &registry);
        finish_all(
            &registry,
            true,
            serde_json::json!({ "state": "done", "mission_id": "G-fake-fo-1" }),
        );
        // The old Headquarters Station queued the report, began sending it, and its comment
        // reached the issue; it died before it recorded that.
        *desk.next_report.lock().unwrap() = Instant::now();
        report_finished(&registry, &roster, &SourceLimits::default(), 5.0);
        let entry = registry.outbox_entries(None).unwrap().remove(0);
        assert_eq!(entry.kind, OutboxKind::Report);
        registry
            .begin_outbox_send(&entry.id, 99, 6.0)
            .unwrap()
            .unwrap();
        OWN_REPLIES.lock().unwrap().push(crate::Reply {
            id: "c-old".to_owned(),
            item: "fo-1".to_owned(),
            own: true,
            body: format!("done <!-- ai6:{} -->", entry.id),
            ..crate::Reply::default()
        });
        pass(&desk, &plugins, &roster, &registry);
        let settled = registry.outbox_entry(&entry.id).unwrap().unwrap();
        assert_eq!(settled.state, OutboxState::Posted);
        assert_eq!(
            settled.result.unwrap().comment_id.as_deref(),
            Some("c-old"),
            "recorded, not reposted"
        );
        assert!(posts_to("fo-1").is_empty(), "nothing was posted again");
    }

    #[test]
    fn a_send_the_old_term_left_that_never_arrived_is_sent_again_and_a_create_is_refused() {
        let plugins = plugins();
        let (_dir, roster) = roster_with("overt", "{ fake = { ids = [\"fo-2\"] } }");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        desk.run(&plugins, &roster, &registry);
        let mission = &uid(&registry, "G-fake-fo-2");
        let mut comment =
            OutboxEntry::new("G", mission, "fake", "fo-2", OutboxKind::Comment, "m", 1.0);
        comment.body = "A question.".to_owned();
        let comment = registry.queue_outbox(&comment).unwrap();
        let mut create = OutboxEntry::new("G", mission, "fake", "", OutboxKind::Create, "m", 1.0);
        create.payload = serde_json::json!({ "title": "x" });
        let create = registry.queue_outbox(&create).unwrap();
        for entry in [&comment, &create] {
            registry
                .begin_outbox_send(&entry.id, 99, 2.0)
                .unwrap()
                .unwrap();
        }
        pass(&desk, &plugins, &roster, &registry);
        assert_eq!(
            registry.outbox_entry(&comment.id).unwrap().unwrap().state,
            OutboxState::Posted
        );
        assert_eq!(posts_to("fo-2").len(), 1, "sent again");
        let create = registry.outbox_entry(&create.id).unwrap().unwrap();
        assert_eq!(create.state, OutboxState::Refused);
        assert!(create.error.unwrap().contains("check the tracker"));
    }

    #[test]
    fn only_the_headquarters_station_sends() {
        let plugins = plugins();
        let (_dir, roster) = roster_with("overt", "{ fake = { ids = [\"hq-1\"] } }");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry.elect("station-b", crate::DEFAULT_LEASE).unwrap();
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        desk.run(&plugins, &roster, &registry);
        let mut entry = OutboxEntry::new(
            "G",
            uid(&registry, "G-fake-hq-1"),
            "fake",
            "hq-1",
            OutboxKind::Comment,
            "m",
            1.0,
        );
        entry.body = "x".to_owned();
        registry.queue_outbox(&entry).unwrap();
        pass(&desk, &plugins, &roster, &registry);
        assert!(posts_to("hq-1").is_empty());
        assert_eq!(registry.due_outbox(f64::MAX, 10).unwrap().len(), 1);
    }

    #[test]
    fn a_failing_send_backs_off_and_a_refused_one_is_never_retried() {
        let (_dir, roster) = roster_with("overt", "{ fake = { ids = [\"bo-1\"] } }");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        registry
            .link_issue(&source_link("G", "m-bo", "fake", "bo-1", None, 1.0))
            .unwrap();
        let mut entry =
            OutboxEntry::new("G", "m-bo", "fake", "bo-1", OutboxKind::Comment, "m", 1.0);
        entry.body = "x".to_owned();
        let queued = registry.queue_outbox(&entry).unwrap();
        let memory = crate::station::issues::DrainState::new();
        let limits = SourceLimits::default();
        let now = unix_seconds() as f64;
        let down = |_: &str, _: &toml::Table| -> Result<Box<dyn Tracker>, String> {
            Err("the tracker is unreachable".to_owned())
        };
        drain_outbox(
            &registry,
            &roster,
            &down,
            "station-a",
            &limits,
            &memory,
            now,
        );
        let after = registry.outbox_entry(&queued.id).unwrap().unwrap();
        assert_eq!(after.state, OutboxState::Pending);
        assert_eq!(after.attempts, 1);
        assert!(after.not_before >= now + limits.retry_backoff.0.as_secs_f64());
        assert_eq!(
            drain_outbox(
                &registry,
                &roster,
                &down,
                "station-a",
                &limits,
                &memory,
                now
            ),
            0,
            "not due yet"
        );
        // An unknown tracker instance is refused outright.
        let mut stray =
            OutboxEntry::new("G", "m-bo", "nowhere", "z", OutboxKind::Comment, "m", 1.0);
        stray.body = "x".to_owned();
        registry
            .link_issue(&source_link("G", "m-bo", "nowhere", "z", None, 1.0))
            .unwrap();
        let stray = registry.queue_outbox(&stray).unwrap();
        drain_outbox(
            &registry,
            &roster,
            &down,
            "station-a",
            &limits,
            &memory,
            now,
        );
        let refused = registry.outbox_entry(&stray.id).unwrap().unwrap();
        assert_eq!(refused.state, OutboxState::Refused);
        assert!(refused.error.unwrap().contains("configures no tracker"));
    }

    #[test]
    fn a_looser_issues_setting_is_a_start_up_weakening() {
        let (_dir, roster) = roster_issues(
            "{ fake = { ids = [] } }",
            "posts_per_hour = 600\ncreate = \"direct\"",
        );
        let lines = crate::station::issues::weakenings(&roster);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("Operation \"G\" issues is weakened"));
        let (_dir, strict) = roster_issues("{ fake = { ids = [] } }", "");
        assert!(crate::station::issues::weakenings(&strict).is_empty());
    }

    #[test]
    fn a_source_s_ca_file_is_a_start_up_weakening() {
        let (_dir, roster) = roster_issues(
            "{ fake = { ids = [], ca_file = \"/etc/ai6/enterprise.pem\" } }",
            "",
        );
        let lines = crate::station::issues::weakenings(&roster);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("Operation \"G\" source \"fake\""),
            "{lines:?}"
        );
        assert!(lines[0].contains("/etc/ai6/enterprise.pem"), "{lines:?}");
    }

    #[test]
    fn replies_from_permitted_authors_are_stored_once_others_ignored_and_own_skipped() {
        let plugins = plugins();
        let (_dir, roster) = roster_with("covert", "{ fake = { ids = [\"rp-1\"] } }");
        let state = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(state.path());
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        let desk = SourceDesk::new("station-a", SourceLimits::default());
        desk.run(&plugins, &roster, &registry);
        let reply = |id: &str, permitted: bool, own: bool| crate::Reply {
            id: id.to_owned(),
            item: "rp-1".to_owned(),
            author_id: format!("who-{id}"),
            author: format!("Person {id}"),
            permitted,
            own,
            body: format!("text of {id}"),
            created: "2099-10-05T10:00:00Z".to_owned(),
        };
        OWN_REPLIES.lock().unwrap().extend([
            reply("r1", true, false),
            reply("r2", false, false),
            reply("r3", false, true),
        ]);
        let build =
            |provider: &str, settings: &toml::Table| build_tracker(&plugins, provider, settings);
        assert_eq!(
            poll_replies(&registry, &roster, &build, "station-b", 5.0),
            0,
            "only the Headquarters Station reads replies"
        );
        assert_eq!(
            poll_replies(&registry, &roster, &build, "station-a", 5.0),
            1
        );
        let mission = &uid(&registry, "G-fake-rp-1");
        let stored = registry.issue_replies(mission).unwrap();
        let ids: Vec<&str> = stored.iter().map(|reply| reply.id.as_str()).collect();
        assert_eq!(ids, ["r1", "r2"], "the token's own comment is skipped");
        assert_eq!(
            (stored[0].state, stored[0].body.as_str()),
            (ReplyState::Pending, "text of r1")
        );
        assert_eq!(
            (stored[1].state, stored[1].body.as_str()),
            (ReplyState::Ignored, ""),
            "someone not permitted is recorded without their text"
        );
        let events = registry.timeline_events("G").unwrap();
        let ignored: Vec<_> = events
            .iter()
            .filter(|event| event.action == "reply_ignored")
            .collect();
        assert_eq!(ignored.len(), 1);
        assert!(
            !format!("{:?}", ignored[0]).contains("text of r2"),
            "never the text"
        );
        assert!(format!("{:?}", ignored[0]).contains("Person r2"));
        // Polled again, the same comments change nothing.
        assert_eq!(
            poll_replies(&registry, &roster, &build, "station-a", 6.0),
            0
        );
        assert_eq!(
            registry
                .timeline_events("G")
                .unwrap()
                .iter()
                .filter(|event| event.action == "reply_ignored")
                .count(),
            1
        );
        // A finished Mission's issue is no longer watched.
        registry
            .set_issue_status(mission, "fake", "rp-1", None, Some("done"))
            .unwrap();
        OWN_REPLIES.lock().unwrap().push(reply("r4", true, false));
        assert_eq!(
            poll_replies(&registry, &roster, &build, "station-a", 7.0),
            0
        );
        OWN_REPLIES
            .lock()
            .unwrap()
            .retain(|reply| reply.item != "rp-1");
    }
}

/// Reopening settled items, on every Registry backend.
#[cfg(test)]
mod reopen_tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use guatiao::library::ProviderError;

    use super::*;
    use crate::directorates::registry::mission_issues::ReplyState;
    use crate::directorates::registry::sources::{SourceBatch, SourceItem};
    use crate::directorates::registry::suite::{Store, backend_tests};
    use crate::{
        IssueSpec, PostedComment, Reply, ReplyBatch, ReplyPoll, TrackerCapabilities, TrackerIssue,
        TrackerLink, TrackerPost, TrackerUpdate,
    };

    backend_tests!(
        a_finished_item_with_its_label_added_again_is_its_missions_next_attempt,
        an_item_that_never_reopens_stays_finished,
        a_comment_reopens_a_settled_item_and_one_ai6_wrote_does_not,
    );

    /// One item, as the tracker shows it now, its comments, and polls that find it changed.
    #[derive(Clone, Default)]
    struct Issue {
        labels: Arc<Mutex<Vec<String>>>,
        replies: Arc<Mutex<Vec<Reply>>>,
        polls: Arc<Mutex<u32>>,
    }

    impl Issue {
        fn label(&self, labels: &[&str]) {
            *self.labels.lock().unwrap() = labels.iter().map(|label| (*label).to_owned()).collect();
        }

        fn item(&self) -> SourceItem {
            SourceItem {
                id: "o/r#7".to_owned(),
                title: "The build fails".to_owned(),
                body: "It broke.".to_owned(),
                url: Some("https://tracker.test/o/r/7".to_owned()),
                labels: self.labels.lock().unwrap().clone(),
                author: Some("reporter".to_owned()),
                updated: format!("2026-11-05T00:00:{:02}Z", *self.polls.lock().unwrap()),
                code: Some("7".to_owned()),
                code_tag: Some("gh".to_owned()),
            }
        }
    }

    impl Source for Issue {
        fn poll(&self, _: SourcePoll) -> Result<SourceBatch, ProviderError> {
            *self.polls.lock().unwrap() += 1;
            Ok(SourceBatch {
                items: vec![self.item()],
                cursor: None,
            })
        }
    }

    impl Tracker for Issue {
        fn capabilities(&self, _: Option<String>) -> Result<TrackerCapabilities, ProviderError> {
            Ok(TrackerCapabilities::default())
        }

        fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError> {
            let found = self.item();
            Ok(TrackerIssue {
                id: item,
                title: found.title,
                body: found.body,
                state: "open".to_owned(),
                labels: found.labels,
                url: found.url,
                updated: found.updated,
                ..TrackerIssue::default()
            })
        }

        fn replies(&self, request: ReplyPoll) -> Result<ReplyBatch, ProviderError> {
            Ok(ReplyBatch {
                replies: self
                    .replies
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|reply| request.items.contains(&reply.item))
                    .cloned()
                    .collect(),
                cursor: None,
            })
        }

        fn post(&self, _: TrackerPost) -> Result<PostedComment, ProviderError> {
            Ok(PostedComment {
                id: "c1".to_owned(),
                url: None,
            })
        }

        fn update(&self, _: TrackerUpdate) -> Result<(), ProviderError> {
            Ok(())
        }

        fn create(&self, _: IssueSpec) -> Result<TrackerIssue, ProviderError> {
            Ok(TrackerIssue::default())
        }

        fn link(&self, _: TrackerLink) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    /// Operation `G` with one instance, `gh`, reopening on `reopen`, and its setup.
    fn roster(reopen: &str) -> (tempfile::TempDir, Roster, SourceSetup) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("operations.toml"),
            format!(
                "[operation.G]\nsources = {{ gh = {{ provider = \"fake\", reopen = \"{reopen}\", \
                 status_labels = {{ done = \"ai6:done\" }} }} }}\n"
            ),
        )
        .unwrap();
        let roster = Roster::load(dir.path()).unwrap();
        let setup = source_setups(roster.operation("G").unwrap(), &SourceLimits::default())
            .remove(0)
            .unwrap();
        (dir, roster, setup)
    }

    fn poll(registry: &dyn RegistryBackend, issue: &Issue, setup: &SourceSetup) -> PollSummary {
        poll_source(
            registry,
            "station-a",
            ("G", "gh"),
            false,
            issue,
            &setup.reopening(),
        )
        .unwrap()
    }

    /// Claims the item's brief, finishes it and writes its report's phase, as a run does.
    fn settle(registry: &dyn RegistryBackend, attempt: u32) -> String {
        let lease = registry
            .claim_with_lease(None, "station-a", crate::DEFAULT_LEASE, BTreeMap::new())
            .unwrap()
            .expect("the item's brief is queued");
        assert_eq!(lease.attempt(), attempt);
        let brief = registry.read_claimed(lease.key()).unwrap().unwrap();
        assert_eq!(brief.handle.as_deref(), Some("G-gh-7"));
        registry
            .finish(
                &brief,
                &serde_json::json!({"state": "done"}),
                true,
                None,
                None,
            )
            .unwrap();
        registry
            .set_issue_status(lease.key(), "gh", "o/r#7", None, Some("done"))
            .unwrap();
        lease.key().to_owned()
    }

    fn reopened(registry: &dyn RegistryBackend) -> Vec<String> {
        registry
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "source" && event.action == "reopened")
            .map(|event| event.detail["reason"].clone())
            .collect()
    }

    fn a_finished_item_with_its_label_added_again_is_its_missions_next_attempt(store: &dyn Store) {
        let handle = store.handle();
        let registry: &dyn RegistryBackend = handle.as_ref();
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        let (_dir, roster, setup) = roster("label");
        let issue = Issue::default();
        issue.label(&["ai6"]);
        assert_eq!(poll(registry, &issue, &setup).queued, 1);
        let key = settle(registry, 1);

        // Changed, its labels as they were, or with only AI6's own: still settled.
        issue.label(&["ai6", "ai6:done"]);
        let summary = poll(registry, &issue, &setup);
        assert_eq!((summary.reopened, summary.skipped), (0, 1));

        // The label is taken off, and a later round reads the item without it.
        issue.label(&["ai6:done"]);
        let build = |_: &str, _: &toml::Table| -> Result<Box<dyn Tracker>, String> {
            Ok(Box::new(issue.clone()))
        };
        assert_eq!(
            watch_settled(
                registry,
                &roster,
                &build,
                "station-b",
                &SourceLimits::default()
            ),
            0,
            "only the Headquarters Station reads settled items"
        );
        assert_eq!(
            watch_settled(
                registry,
                &roster,
                &build,
                "station-a",
                &SourceLimits::default()
            ),
            1
        );

        // Put back, it reopens the same Mission as its next attempt, once.
        issue.label(&["ai6", "ai6:done"]);
        assert_eq!(poll(registry, &issue, &setup).reopened, 1);
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(
            (queued[0].key(), queued[0].shown_handle()),
            (key.clone(), "G-gh-7".to_owned())
        );
        assert_eq!(poll(registry, &issue, &setup).reopened, 0);
        assert_eq!(reopened(registry), ["label ai6 added"]);
        assert!(
            registry
                .audit_records()
                .unwrap()
                .iter()
                .any(
                    |record| record.action == "mission.reopened" && record.target.starts_with(&key)
                )
        );
        assert_eq!(settle(registry, 1), key, "the same Mission's next attempt");
    }

    fn an_item_that_never_reopens_stays_finished(store: &dyn Store) {
        let handle = store.handle();
        let registry: &dyn RegistryBackend = handle.as_ref();
        let (_dir, _roster, setup) = roster("never");
        let issue = Issue::default();
        issue.label(&["ai6"]);
        poll(registry, &issue, &setup);
        let key = settle(registry, 1);
        registry
            .set_issue_snapshot(&key, "gh", "o/r#7", None, &Default::default())
            .unwrap();
        issue.label(&["ai6", "bug"]);
        let summary = poll(registry, &issue, &setup);
        assert_eq!((summary.reopened, summary.skipped), (0, 1));
        assert!(registry.list_new().unwrap().is_empty());
        assert_eq!(registry.filed_brief(&key).unwrap().unwrap().state, "done");
        assert!(reopened(registry).is_empty());
    }

    fn a_comment_reopens_a_settled_item_and_one_ai6_wrote_does_not(store: &dyn Store) {
        let handle = store.handle();
        let registry: &dyn RegistryBackend = handle.as_ref();
        registry.elect("station-a", crate::DEFAULT_LEASE).unwrap();
        let (_dir, roster, setup) = roster("comment");
        let issue = Issue::default();
        issue.label(&["ai6"]);
        poll(registry, &issue, &setup);
        let key = settle(registry, 1);
        let build = |_: &str, _: &toml::Table| -> Result<Box<dyn Tracker>, String> {
            Ok(Box::new(issue.clone()))
        };
        let comment = |id: &str, author: &str, own: bool| Reply {
            id: id.to_owned(),
            item: "o/r#7".to_owned(),
            author_id: author.to_owned(),
            author: author.to_owned(),
            permitted: true,
            own,
            body: format!("{id}: it broke again"),
            created: "2099-11-05T10:00:00Z".to_owned(),
        };
        issue
            .replies
            .lock()
            .unwrap()
            .push(comment("c1", "ai6-bot", true));
        poll_replies(registry, &roster, &build, "station-a", 5.0);
        assert!(registry.list_new().unwrap().is_empty(), "AI6's own comment");

        issue
            .replies
            .lock()
            .unwrap()
            .push(comment("c2", "reporter", false));
        assert_eq!(poll_replies(registry, &roster, &build, "station-a", 6.0), 1);
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].key(), key);
        let begin = queued[0].body.find("BEGIN gh ITEM").unwrap();
        let end = queued[0].body.find("END gh ITEM").unwrap();
        assert!(
            queued[0].body[begin..end].contains("Comment by reporter"),
            "the comment is quoted as the item is: {}",
            queued[0].body
        );
        assert!(queued[0].body[begin..end].contains("c2: it broke again"));
        assert!(!queued[0].body.contains("c1: it broke again"));
        let stored = registry.issue_replies(&key).unwrap();
        assert_eq!(
            stored
                .iter()
                .find(|reply| reply.id == "c2")
                .map(|reply| reply.state),
            Some(ReplyState::Delivered)
        );
        assert_eq!(reopened(registry), ["1 new comment(s)"]);
        assert_eq!(settle(registry, 1), key);
        assert!(
            crate::station::issues::weakenings(&roster)
                .iter()
                .any(|line| line.contains("reopens a settled item on a comment")),
            "a comment that reopens is a weakening"
        );
    }
}
