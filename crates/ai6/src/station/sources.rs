//! Source polling, CX reports and sleepers, run by the Station that holds the Headquarters lease.
//!
//! Each Operation lists its Sources under `sources.<provider>` in `operations.toml`. The
//! desk polls each one on an interval, queues what it finds as briefs, and posts the
//! outcome of every finished Mission back to the item it came from. Cursors live in the
//! Registry, so a Station that takes over the Headquarters lease resumes where the last
//! one stopped. Authorized sleepers queue their briefs from the same pass.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::directorates::m::cover::Disclosure;
use crate::directorates::m::roster::{Operation, Roster};
use crate::directorates::registry::backend::*;
use crate::directorates::registry::sleepers::Fire;
use crate::directorates::registry::sources::{
    CxReport, FinishedSourceBrief, Ingest, Source, SourceConfig, SourcePoll, reportable,
};
use crate::station::plugins::Plugins;
use guatiao::ToValue;

/// Poll interval when an Operation's source table sets none.
pub(crate) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);
/// Shortest accepted `interval_secs`; keeps a typo from hammering a tracker.
pub(crate) const MIN_POLL_INTERVAL: Duration = Duration::from_secs(10);
/// How often finished Missions are checked for reports that still need posting.
const REPORT_INTERVAL: Duration = Duration::from_secs(15);
/// How often sleepers are checked against the clock.
const SLEEPER_INTERVAL: Duration = Duration::from_secs(20);

/// Constructs the provider for a provider ID and an Operation's settings.
type SourceBuilder<'a> = dyn Fn(&str, &toml::Table) -> Result<Box<dyn Source>, String> + 'a;

/// Providers already built during one report pass, by (Operation, provider).
type SourceCache = BTreeMap<(String, String), Result<Box<dyn Source>, String>>;

/// What one poll of one source changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PollSummary {
    pub queued: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub skipped: usize,
}

/// A source provider as configured for one Operation.
pub(crate) struct SourceSetup {
    pub provider: String,
    pub settings: toml::Table,
    pub interval: Duration,
}

/// Reads an Operation's `sources` table into per-provider settings.
///
/// A table value holds the provider's settings; a bare string is shorthand for
/// `{ target = "<string>" }`. `interval_secs` belongs to the desk and is not passed on.
pub(crate) fn source_setups(operation: &Operation) -> Vec<Result<SourceSetup, String>> {
    operation
        .sources
        .iter()
        .map(|(provider, value)| {
            let mut settings = match value {
                toml::Value::Table(table) => table.clone(),
                toml::Value::String(target) => {
                    let mut table = toml::Table::new();
                    table.insert("target".to_owned(), toml::Value::String(target.clone()));
                    table
                }
                _ => {
                    return Err(format!(
                        "Operation {}: sources.{provider} must be a table or a string",
                        operation.id
                    ));
                }
            };
            let interval = match settings.remove("interval_secs") {
                None => DEFAULT_POLL_INTERVAL,
                Some(toml::Value::Integer(seconds)) if seconds > 0 => {
                    Duration::from_secs(seconds as u64).max(MIN_POLL_INTERVAL)
                }
                Some(_) => {
                    return Err(format!(
                        "Operation {}: sources.{provider}.interval_secs must be a positive integer",
                        operation.id
                    ));
                }
            };
            Ok(SourceSetup {
                provider: provider.clone(),
                settings,
                interval,
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
            Ok(Ingest::Queued) => summary.queued += 1,
            Ok(Ingest::Updated) => summary.updated += 1,
            Ok(Ingest::Unchanged) => summary.unchanged += 1,
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

/// Posts the outcome of each finished source Mission back to its item.
///
/// `build` constructs the provider for an Operation's settings. A report that fails is
/// retried on later passes, up to the Registry's attempt limit.
pub(crate) fn report_finished(
    registry: &dyn RegistryBackend,
    roster: &Roster,
    build: &SourceBuilder<'_>,
) -> usize {
    let Ok(finished) = registry.unreported_source_briefs() else {
        return 0;
    };
    let mut sources = SourceCache::new();
    let mut reported = 0;
    for entry in finished {
        let result = report_one(roster, build, &mut sources, &entry);
        match result {
            Ok(()) => {
                reported += 1;
                let _ = registry.mark_source_reported(&entry, true);
            }
            Err(error) => {
                eprintln!(
                    "ai6: CX report for {} not posted: {error}",
                    entry.brief.key()
                );
                let _ = registry.mark_source_reported(&entry, false);
            }
        }
    }
    reported
}

fn report_one(
    roster: &Roster,
    build: &SourceBuilder<'_>,
    sources: &mut SourceCache,
    entry: &FinishedSourceBrief,
) -> Result<(), String> {
    let provider = entry
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
    let setup = source_setups(operation)
        .into_iter()
        .filter_map(Result::ok)
        .find(|setup| setup.provider == provider)
        .ok_or_else(|| {
            format!(
                "Operation {} no longer configures source {provider}",
                operation.id
            )
        })?;
    if !reportable(&entry.brief, provider) {
        return Err(format!(
            "the brief has no provenance from source {provider}, so nothing is posted for it"
        ));
    }
    let source = sources
        .entry((operation.id.clone(), provider.to_owned()))
        .or_insert_with(|| build(provider, &setup.settings));
    let source = source.as_ref().map_err(Clone::clone)?;
    source
        .report(compose_report(cover.disclosure(), entry))
        .map_err(|error| error.to_string())
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
            let Some(window) = sleeper.due_window(now, approval.at) else {
                continue;
            };
            match registry.fire_sleeper(&operation.id, name, sleeper, window, station) {
                Ok(Fire::Queued) => {
                    queued += 1;
                    eprintln!(
                        "ai6: sleeper {name} of Operation {} queued a brief",
                        operation.id
                    );
                }
                Ok(Fire::AlreadyFired) => {}
                Err(error) => {
                    eprintln!("ai6: sleeper {name} of Operation {}: {error}", operation.id);
                }
            }
        }
    }
    queued
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
    next_sleepers: Mutex<Instant>,
    warned: Mutex<BTreeSet<String>>,
    /// This Station: the provenance of the briefs it queues names it.
    station: String,
}

impl SourceDesk {
    pub(crate) fn new(station: impl Into<String>) -> Self {
        Self {
            station: station.into(),
            busy: AtomicBool::new(false),
            due: Mutex::new(BTreeMap::new()),
            next_report: Mutex::new(Instant::now()),
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
        self.run_with(roster, registry, &build);
    }

    pub(crate) fn run_with(
        &self,
        roster: &Roster,
        registry: &dyn RegistryBackend,
        build: &SourceBuilder<'_>,
    ) {
        let now = Instant::now();
        for operation in roster.operations() {
            for setup in source_setups(operation) {
                let setup = match setup {
                    Ok(setup) => setup,
                    Err(error) => {
                        eprintln!("ai6: {error}");
                        continue;
                    }
                };
                let key = (operation.id.clone(), setup.provider.clone());
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
                        &setup.provider,
                        source.as_ref(),
                    )
                });
                match result {
                    Ok(summary) if summary.queued + summary.updated > 0 => eprintln!(
                        "ai6: source {} for Operation {}: {} queued, {} updated",
                        setup.provider, operation.id, summary.queued, summary.updated
                    ),
                    Ok(_) => {}
                    Err(error) => eprintln!(
                        "ai6: source {} for Operation {}: {error}",
                        setup.provider, operation.id
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
                    *next = now + REPORT_INTERVAL;
                }
                due
            })
            .unwrap_or(false);
        if report_due {
            report_finished(registry, roster, build);
        }
        let sleepers_due = self
            .next_sleepers
            .lock()
            .map(|mut next| {
                let due = *next <= now;
                if due {
                    *next = now + SLEEPER_INTERVAL;
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
    use crate::directorates::registry::sources::{SourceBatch, SourceItem};
    use crate::directorates::registry::store::FileRegistry;

    static POSTED: Mutex<Vec<CxReport>> = Mutex::new(Vec::new());

    /// Serves the ids listed in its settings; remembers every report it is given.
    #[derive(Provider)]
    #[provider(Source, id = "fake", name = "fake", config = SourceConfig)]
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

        fn report(&self, report: CxReport) -> Result<(), ProviderError> {
            POSTED.lock().unwrap().push(report);
            Ok(())
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
        let desk = SourceDesk::new("station-a");

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
        let setups = source_setups(roster.operation("G").unwrap());
        let setup = setups.into_iter().next().unwrap().unwrap();
        assert_eq!(setup.settings["target"].as_str(), Some("GUA"));
        assert_eq!(setup.interval, DEFAULT_POLL_INTERVAL);
    }

    #[test]
    fn interval_secs_is_clamped_and_validated() {
        let (_dir, roster) = roster_with("covert", "{ fake = { interval_secs = 1 } }");
        let setup = source_setups(roster.operation("G").unwrap())
            .remove(0)
            .unwrap();
        assert_eq!(setup.interval, MIN_POLL_INTERVAL);
        assert!(!setup.settings.contains_key("interval_secs"));
        let (_dir, roster) = roster_with("covert", "{ fake = { interval_secs = \"soon\" } }");
        assert!(
            source_setups(roster.operation("G").unwrap())
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
        let desk = SourceDesk::new("station-a");
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
        desk.run(&plugins, &roster, &registry);
        desk.due.lock().unwrap().clear();
        *desk.next_report.lock().unwrap() = Instant::now();
        desk.run(&plugins, &roster, &registry);

        let posted = POSTED.lock().unwrap();
        let mine: Vec<_> = posted.iter().filter(|r| r.item_id == "report-me").collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].outcome, "done");
        assert_eq!(mine[0].branch.as_deref(), Some("agent/fix"));
        assert!(mine[0].body.contains("007"));
        assert!(mine[0].body.contains("sess-1"));
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

        let window = sleeper.due_window(NOW, 0).unwrap();
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
            brief: crate::source_brief("G", "github", &item),
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
}
