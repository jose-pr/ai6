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
use crate::directorates::registry::mission_issues::{IssueLink, LinkRelation, LinkState};
use crate::directorates::registry::sleepers::{Fire, Sleeper};
use crate::directorates::registry::sources::{
    FinishedSourceBrief, Ingest, Source, SourceConfig, SourcePoll, reportable,
};
use crate::directorates::registry::tracker::Tracker;
use crate::station::issues::{
    DrainState, TrackerBuilder, drain_outbox, poll_replies, queue_report,
};

use crate::station::limits::SourceLimits;
use crate::station::plugins::Plugins;
use guatiao::ToValue;

/// Constructs the provider for a provider ID and an Operation's settings.
type SourceBuilder<'a> = dyn Fn(&str, &toml::Table) -> Result<Box<dyn Source>, String> + 'a;

/// The words a Mission's progress is written in on an instance: a label or a workflow
/// transition per milestone. Unset milestones write nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct StatusWords {
    /// While an Agent works.
    pub working: Option<String>,
    /// While the Mission waits for an answer on the issue.
    pub waiting: Option<String>,
    /// While the Mission is paused.
    pub paused: Option<String>,
    /// When the Mission ended successfully.
    pub done: Option<String>,
    /// When the Mission ended without completing.
    pub failed: Option<String>,
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
            let words = |key: &str| -> Result<StatusWords, String> {
                settings.get(key).map_or_else(
                    || Ok(StatusWords::default()),
                    |value| {
                        value.clone().try_into().map_err(|error| {
                            format!("Operation {}: sources.{name}.{key}: {error}", operation.id)
                        })
                    },
                )
            };
            let names = |key: &str| -> Result<Vec<String>, String> {
                settings.get(key).map_or_else(
                    || Ok(Vec::new()),
                    |value| {
                        value.clone().try_into().map_err(|error| {
                            format!("Operation {}: sources.{name}.{key}: {error}", operation.id)
                        })
                    },
                )
            };
            let agent_labels = names("agent_labels")?;
            let agent_transitions = names("agent_transitions")?;
            let status_labels = words("status_labels")?;
            let status_transitions = words("status_transitions")?;
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
                status_labels,
                status_transitions,
                agent_labels,
                agent_transitions,
            })
        })
        .collect()
}

/// Builds the provider for one Operation's settings through the plugin host.
fn build_source(
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
fn build_tracker(
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
/// are stored, so a crash repeats work that is already idempotent.
pub(crate) fn poll_source(
    registry: &dyn RegistryBackend,
    station: &str,
    operation: &str,
    provider: &str,
    source: &dyn Source,
) -> Result<PollSummary, String> {
    let cursor = registry
        .source_cursor(operation, provider)
        .map_err(|error| format!("cursor unreadable: {error}"))?;
    let batch = source
        .poll(SourcePoll { cursor })
        .map_err(|error| format!("poll failed: {error}"))?;
    let mut summary = PollSummary::default();
    for item in &batch.items {
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
            Ok(Ingest::InFlight | Ingest::Finished) => summary.skipped += 1,
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

/// The outcome of a Mission, worded for the item that started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CxReport {
    /// The item ID the Mission came from.
    pub item_id: String,
    /// The AI6 Mission ID.
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
    let brief = crate::directorates::registry::sources::source_brief(
        operation,
        provider,
        item,
        &crate::directorates::registry::limits::SourceLimits::default(),
    );
    let Ok(mission) = crate::station::server::mission_id_for_brief(&brief) else {
        return;
    };
    let mission = mission.to_string();
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
            link.title = Some(item.title.clone());
            if let Err(error) = registry.link_issue(&link) {
                eprintln!(
                    "ai6: source {provider}: item {:?} not linked: {error}",
                    item.id
                );
            }
        }
        Err(error) => eprintln!("ai6: source {provider}: links unreadable: {error}"),
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
    let mission_id = outcome_text("mission_id").unwrap_or_default();
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
            lines.push(format!("Mission: `{mission_id}`"));
        }
        _ => lines.push(format!("AI6 Mission `{mission_id}` ended: {outcome}.")),
    }
    if let Some(branch) = &branch {
        lines.push(format!("Branch: `{branch}`"));
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
        mission_id,
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
        title: None,
    }
}

/// Queues the brief of every authorized sleeper whose latest window has not fired.
///
/// Returns how many briefs were queued. A sleeper without an approval of its current
/// definition is skipped and named once in `warned`.
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
                Ok(Some(approval)) if approval.fingerprint == sleeper.fingerprint() => approval,
                Ok(_) => {
                    if warned.insert(format!("{}/{name}", operation.id)) {
                        eprintln!(
                            "ai6: sleeper {name} of Operation {} waits for `ai6 sleeper authorize {} {name}`",
                            operation.id, operation.id
                        );
                    }
                    continue;
                }
                Err(error) => {
                    eprintln!("ai6: sleeper {name} of Operation {}: {error}", operation.id);
                    continue;
                }
            };
            let Some(window) = sleeper.due_window(now, approval.at, &registry.limits().sleepers)
            else {
                continue;
            };
            let rounds = if sleeper.each_station {
                round_stations(registry, roster, &operation.id, sleeper, now)
                    .into_iter()
                    .map(Some)
                    .collect()
            } else {
                vec![None]
            };
            for round in rounds {
                match registry.fire_sleeper(
                    &operation.id,
                    name,
                    sleeper,
                    (window, round.as_deref()),
                    station,
                ) {
                    Ok(Fire::Queued) => {
                        queued += 1;
                        match &round {
                            Some(round) => eprintln!(
                                "ai6: sleeper {name} of Operation {} queued the round of Station {round}",
                                operation.id
                            ),
                            None => eprintln!(
                                "ai6: sleeper {name} of Operation {} queued a brief",
                                operation.id
                            ),
                        }
                    }
                    Ok(Fire::AlreadyFired) => {}
                    Err(error) => {
                        eprintln!("ai6: sleeper {name} of Operation {}: {error}", operation.id);
                    }
                }
            }
        }
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
            for setup in source_setups(operation, &self.limits) {
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
                        &operation.id,
                        &setup.name,
                        source.as_ref(),
                    )
                });
                match result {
                    Ok(summary) if summary.queued + summary.updated > 0 => eprintln!(
                        "ai6: source {} for Operation {}: {} queued, {} updated",
                        setup.name, operation.id, summary.queued, summary.updated
                    ),
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
        let running = registry.claim_new(Some("source_fake--1")).unwrap().unwrap();
        desk.due.lock().unwrap().clear();
        desk.run(&plugins, &roster, &registry);
        let queued = registry.list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].key(), "source_fake--2");
        assert!(registry.read_claimed(&running.key()).unwrap().is_some());
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
                "mission_id": "source_fake--report-me",
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
        assert_eq!(queued[0].key(), "source_board--1");
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
        let links = registry.issue_links("source_fake--lk-1").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].relation, LinkRelation::Source);
        assert_eq!(
            (links[0].tracker.as_str(), links[0].item.as_str()),
            ("fake", "lk-1")
        );
        assert_eq!(links[0].linked_by, "m");
        // A person's unlink holds: the next poll does not link it again.
        registry
            .unlink_issue("source_fake--lk-1", "fake", "lk-1")
            .unwrap();
        pass(&desk, &plugins, &roster, &registry);
        assert!(registry.active_issue_links().unwrap().is_empty());
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
        let mission = "source_fake--st-1";

        for phase in ["working", "waiting", "working"] {
            assert_eq!(
                crate::station::issues::queue_status(&registry, &roster, mission, phase, 1.0),
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
        let mission = "source_fake--sp-1";
        crate::station::issues::queue_status(&registry, &roster, mission, "working", 1.0);
        pass(&desk, &plugins, &roster, &registry);
        crate::station::issues::queue_status(&registry, &roster, mission, "paused", 2.0);
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
            crate::station::issues::queue_status(&registry, &roster, "m-oc", "working", 1.0),
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
            crate::station::issues::queue_status(&registry, &roster, "m-al", "working", 1.0),
            2
        );
        assert_eq!(
            crate::station::issues::queue_status(&registry, &roster, "m-al", "done", 2.0),
            1,
            "the report covers the source's end; the child gets a status"
        );
        // Milestones (the default) writes the source only, and not its end.
        let (_dir, roster) = roster_issues("{ fake = { ids = [\"ms-1\"] } }", "");
        registry
            .link_issue(&source_link("G", "m-ms", "fake", "ms-1", None, 1.0))
            .unwrap();
        assert_eq!(
            crate::station::issues::queue_status(&registry, &roster, "m-ms", "waiting", 1.0),
            1
        );
        assert_eq!(
            crate::station::issues::queue_status(&registry, &roster, "m-ms", "done", 2.0),
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
            serde_json::json!({ "state": "done", "mission_id": "source_fake--fo-1" }),
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
        let mission = "source_fake--fo-2";
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
            "source_fake--hq-1",
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
            created: "2026-10-05T10:00:00Z".to_owned(),
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
        let mission = "source_fake--rp-1";
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
