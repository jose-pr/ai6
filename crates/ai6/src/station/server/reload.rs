//! Live configuration: the roster and loadouts a Station reads again while it runs.
//!
//! Every poll ([`DEFAULT_CONFIG_POLL`](super::DEFAULT_CONFIG_POLL) unless the Station says
//! otherwise) asks the configuration provider for the revision of the Station's own view. A
//! new revision is loaded, which checks it as the family files are checked, and the families
//! that apply live are applied: requests, the scheduler and new Sessions use them, while a
//! running Session keeps the configuration it started with. A changed family that only a
//! restart applies is reported as needing one, at check-in and in the log. A revision that is
//! not acceptable is refused once, logged and audited, and the last good configuration keeps
//! running. When its loadouts change, the Station declares its pool accounts again.

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::{DaemonState, accounts, publish_station_check_in};
use crate::directorates::m::config::{
    ConfigProvider, ConfigRevision, ConfigSnapshot, ConfigStatus,
};
use crate::directorates::m::roster::{Roster, RosterErrorKind};
use crate::directorates::q::LoadoutCatalog;
use crate::directorates::registry::store::AuditRecord;

/// The audit action of a refused configuration.
const REFUSED_ACTION: &str = "config.refused";
/// The Registry's bound on an audit target.
/// How often the poll task checks whether the Station is stopping.
const STOP_CHECK: Duration = Duration::from_millis(250);

/// The configuration a Station runs and where it reads it again, shared by every copy of the
/// Station's state. The default holds nothing and reads nothing: each copy keeps its own.
#[derive(Default)]
pub(super) struct LiveConfig {
    source: Option<Arc<dyn ConfigProvider>>,
    /// The Station whose view of the provider it reads.
    station: String,
    /// What the Station started with, to tell which restart-only families changed since.
    started: Option<ConfigSnapshot>,
    live: Mutex<Live>,
}

#[derive(Default)]
struct Live {
    roster: Option<Arc<Roster>>,
    /// `Some` when the loadouts come from the provider, which then replaces them.
    catalog: Option<Option<Arc<LoadoutCatalog>>>,
    revision: Option<ConfigRevision>,
    /// The roster revision refused last, and why.
    refused: Option<(ConfigRevision, String)>,
    needs_restart: BTreeSet<&'static str>,
    /// The last failure to read the configuration, logged once.
    unavailable: Option<String>,
}

impl LiveConfig {
    /// The configuration Station `station` starts with: `started`, its view of `source`,
    /// running `roster`, and the loadouts `catalog` when they came from the provider (`None`
    /// when they were set in code, which no reload replaces).
    pub(super) fn new(
        source: Arc<dyn ConfigProvider>,
        station: &str,
        started: ConfigSnapshot,
        roster: Arc<Roster>,
        catalog: Option<Option<Arc<LoadoutCatalog>>>,
    ) -> Self {
        let revision = started.revision;
        Self {
            source: Some(source),
            station: station.to_owned(),
            started: Some(started),
            live: Mutex::new(Live {
                roster: Some(roster),
                catalog,
                revision: Some(revision),
                ..Live::default()
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The roster the Station runs now, when it reads one.
    pub(super) fn roster(&self) -> Option<Arc<Roster>> {
        self.lock().roster.clone()
    }

    /// What the Station reports at check-in.
    pub(super) fn status(&self) -> ConfigStatus {
        let live = self.lock();
        ConfigStatus {
            revision: live.revision.map(|revision| revision.get().to_string()),
            refused: live
                .refused
                .iter()
                .map(|(_, reason)| reason.clone())
                .collect(),
            needs_restart: live
                .needs_restart
                .iter()
                .map(|&name| name.to_owned())
                .collect(),
        }
    }
}

impl DaemonState {
    /// This state with the configuration the Station runs now.
    pub(super) fn current(&self) -> Self {
        let mut state = self.clone();
        let live = self.config.lock();
        if let Some(roster) = &live.roster {
            state.roster = roster.clone();
        }
        if let Some(catalog) = &live.catalog {
            state.station.credentials.set_catalog(catalog.clone());
        }
        state
    }
}

/// The router's state: each request gets the configuration the Station runs when it arrives.
#[derive(Clone)]
pub(super) struct Served(pub(super) DaemonState);

impl axum::extract::FromRef<Served> for DaemonState {
    fn from_ref(served: &Served) -> Self {
        served.0.current()
    }
}

/// Polls the configuration every `state.station.config_poll` until the Station stops; `None`
/// when polling is off (a zero interval).
pub(super) fn spawn(state: &DaemonState) -> Option<tokio::task::JoinHandle<()>> {
    let every = state.station.config_poll;
    if every.is_zero() {
        return None;
    }
    let state = state.clone();
    Some(tokio::spawn(async move {
        let mut next = tokio::time::Instant::now() + every;
        while !state.stopping.load(Ordering::Acquire) {
            if tokio::time::Instant::now() >= next {
                let polling = state.clone();
                // The source reads files or a database, so it is asked on the blocking pool.
                let _ = tokio::task::spawn_blocking(move || poll(&polling)).await;
                next = tokio::time::Instant::now() + every;
            }
            tokio::time::sleep(STOP_CHECK.min(every)).await;
        }
    }))
}

/// Declares the Station's pool accounts from the configuration it runs now.
pub(super) fn declare(state: &DaemonState) {
    accounts::declare(&state.current());
}

/// Reads the configuration again and applies what changed; whether anything did. Blocks on
/// the source and the Registry.
pub(super) fn poll(state: &DaemonState) -> bool {
    let (roster, loadouts) = poll_provider(state);
    if !(roster || loadouts) {
        return false;
    }
    let current = state.current();
    #[cfg(feature = "cables-mcp")]
    if roster {
        super::green_ink::log_weakenings(&current.roster);
    }
    if roster {
        super::vetting::log_weakenings(&current.roster);
        for weakened in current.roster.settings().weakenings() {
            eprintln!("ai6: {weakened}");
        }
        #[cfg(feature = "publisher")]
        super::assistant::log_weakenings(&current.roster);
        super::agents::log_retirements(&current.roster, current.clock.now());
    }
    if loadouts {
        for weakening in current.station.credentials.weakenings() {
            eprintln!("ai6: {weakening}");
        }
    }
    accounts::declare(&current);
    publish_station_check_in(&current);
    true
}

/// Whether a new revision was applied, and whether the loadouts changed with it.
fn poll_provider(state: &DaemonState) -> (bool, bool) {
    let Some(source) = &state.config.source else {
        return (false, false);
    };
    let station = state.config.station.as_str();
    let revision = match source.revision(station) {
        Ok(revision) => revision,
        Err(error) => {
            unavailable(state, &error.to_string());
            return (false, false);
        }
    };
    {
        let mut live = state.config.lock();
        live.unavailable = None;
        if live.revision == Some(revision) {
            if live.refused.take().is_some() {
                eprintln!(
                    "ai6: the configuration source is back at the revision this Station runs"
                );
            }
            return (false, false);
        }
        if live
            .refused
            .as_ref()
            .is_some_and(|(refused, _)| *refused == revision)
        {
            return (false, false);
        }
    }
    match source.load(station) {
        Ok(snapshot) => {
            restart_families(state, &snapshot);
            let loadouts = (snapshot.settings.loadouts != LoadoutCatalog::default())
                .then(|| Arc::new(snapshot.settings.loadouts));
            let mut live = state.config.lock();
            let loadouts_changed =
                live.catalog.is_some() && live.catalog.as_ref() != Some(&loadouts);
            if live.catalog.is_some() {
                live.catalog = Some(loadouts);
            }
            live.roster = Some(Arc::new(snapshot.roster));
            live.revision = Some(snapshot.revision);
            live.refused = None;
            eprintln!(
                "ai6: configuration revision {} applied",
                snapshot.revision.get()
            );
            (true, loadouts_changed)
        }
        Err(error) if error.kind() == RosterErrorKind::Unavailable => {
            unavailable(state, &error.to_string());
            (false, false)
        }
        Err(error) => {
            let reason = format!("configuration revision {}: {error}", revision.get());
            state.config.lock().refused = Some((revision, reason.clone()));
            refuse(state, &reason);
            (false, false)
        }
    }
}

/// Notes each family only a restart applies whose records differ from what the Station
/// started with.
fn restart_families(state: &DaemonState, snapshot: &ConfigSnapshot) {
    let Some(started) = &state.config.started else {
        return;
    };
    let changed = started.changed_families(snapshot);
    let mut live = state.config.lock();
    for record in crate::directorates::m::config::ConfigRecord::ALL {
        if record.applies_live() {
            continue;
        }
        if !changed.contains(&record) {
            live.needs_restart.remove(record.name());
        } else if live.needs_restart.insert(record.name()) {
            eprintln!("ai6: {record} changed; restart the Station to apply it");
        }
    }
}

/// Logs a failure to read the configuration, once until it changes; nothing is audited, and
/// the Station keeps what it runs.
fn unavailable(state: &DaemonState, reason: &str) {
    let mut live = state.config.lock();
    if live.unavailable.as_deref() != Some(reason) {
        eprintln!("ai6: configuration unavailable, keeping the one this Station runs: {reason}");
        live.unavailable = Some(reason.to_owned());
    }
}

/// Logs and audits a configuration the Station refused to apply.
fn refuse(state: &DaemonState, reason: &str) {
    eprintln!("ai6: configuration refused, keeping the one this Station runs: {reason}");
    let record = AuditRecord {
        id: format!("config-refused-{}", uuid::Uuid::new_v4().simple()),
        actor: format!("station:{}", state.station_id),
        action: REFUSED_ACTION.to_owned(),
        operation: String::new(),
        resource: Some(format!("station:{}", state.station_id)),
        target: bounded(reason, state.station.limits().audit.target_bytes),
        at: state.clock.now() as f64,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: configuration refusal audit record was not written: {error}");
    }
}

/// `text` on one line, cut to `bytes` at a character boundary and marked with `…` when cut.
fn bounded(text: &str, bytes: crate::bounds::Limit) -> String {
    let mut line = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if !bytes.allows(line.len()) {
        let mut end = bytes.or_max().saturating_sub('…'.len_utf8());
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('…');
    }
    line
}

#[cfg(test)]
mod tests {
    use axum::extract::FromRef;

    use std::fs;
    use std::path::Path;

    use super::super::tests::test_state;
    use super::*;
    use crate::directorates::m::config::FileProvider;

    const AGENTS: &str = "[agent.007]\nhandler = \"mock-agent\"\npriority = 1\n";

    const POOLS: &str = "[pool.team]\nhandler = \"mock-pooled\"\n\
        [pool.team.account.alpha]\nkind = \"claude-oauth-token\"\nsecret = \"env-alpha\"\n\
        [pool.team.account.beta]\nkind = \"anthropic-api-key\"\nsecret = \"env-beta\"\n";

    /// Makes `state` run what `source` holds, taking its loadouts from it when `loadouts`
    /// says so, as `serve` does.
    fn live(
        state: &mut DaemonState,
        _root: &Path,
        source: Arc<dyn ConfigProvider>,
        loadouts: bool,
    ) {
        let snapshot = source.load(&state.station_id).unwrap();
        let catalog = loadouts.then(|| {
            (snapshot.settings.loadouts != LoadoutCatalog::default())
                .then(|| Arc::new(snapshot.settings.loadouts.clone()))
        });
        state.roster = Arc::new(snapshot.roster.clone());
        state
            .station
            .credentials
            .set_catalog(catalog.clone().flatten());
        state.config = Arc::new(LiveConfig::new(
            source,
            &state.station_id,
            snapshot,
            state.roster.clone(),
            catalog,
        ));
    }

    fn priority(state: &DaemonState) -> i64 {
        state.roster.agent(Some("G"), "007").unwrap().priority
    }

    fn refusals(state: &DaemonState) -> Vec<AuditRecord> {
        state
            .intake
            .list_audit(100)
            .unwrap()
            .into_iter()
            .filter(|record| record.action == REFUSED_ACTION)
            .collect()
    }

    #[test]
    fn a_changed_roster_reaches_requests_and_a_refused_one_leaves_the_last_good_running() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", AGENTS);
        live(
            &mut state,
            root.path(),
            Arc::new(FileProvider::new(root.path())),
            false,
        );
        assert!(!poll(&state), "nothing changed");

        let agents = root.path().join("agents.toml");
        fs::write(&agents, AGENTS.replace("priority = 1", "priority = 7")).unwrap();
        assert!(poll(&state));
        assert_eq!(priority(&state.current()), 7);
        assert_eq!(
            priority(&DaemonState::from_ref(&Served(state.clone()))),
            7,
            "a request gets the configuration the Station runs when it arrives"
        );
        assert_eq!(priority(&state), 1, "a copy taken earlier keeps its own");

        fs::write(&agents, format!("{AGENTS}clearance = \"root\"\n")).unwrap();
        assert!(!poll(&state));
        assert!(!poll(&state), "a refused revision is checked once");
        assert_eq!(priority(&state.current()), 7, "the last good roster runs");
        let refused = refusals(&state);
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0].resource.as_deref(), Some("station:test"));
        assert!(refused[0].target.contains("clearance"), "{refused:?}");
        let status = state.config.status();
        assert_eq!(status.refused.len(), 1, "{status:?}");

        fs::write(&agents, AGENTS.replace("priority = 1", "priority = 8")).unwrap();
        assert!(poll(&state));
        assert_eq!(priority(&state.current()), 8);
        assert!(state.config.status().refused.is_empty());
    }

    fn profiles_of(state: &DaemonState) -> Vec<String> {
        state
            .intake
            .accounts()
            .unwrap()
            .into_iter()
            .filter(|account| account.profiles.contains_key("test"))
            .map(|account| account.account.to_string())
            .collect()
    }

    #[test]
    fn changed_loadouts_declare_pool_accounts_again_and_bad_ones_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", AGENTS);
        let loadouts = root.path().join("loadouts.toml");
        fs::write(&loadouts, POOLS).unwrap();
        live(
            &mut state,
            root.path(),
            Arc::new(FileProvider::new(root.path())),
            true,
        );
        declare(&state);
        assert_eq!(profiles_of(&state), ["team/alpha", "team/beta"]);

        let without_beta = POOLS.split("[pool.team.account.beta]").next().unwrap();
        fs::write(&loadouts, without_beta).unwrap();
        assert!(poll(&state));
        assert_eq!(profiles_of(&state), ["team/alpha"], "beta is withdrawn");
        let current = state.current();
        let catalog = current.station.credentials.catalog().unwrap();
        assert!(catalog.pool("team").unwrap().get("beta").is_none());

        fs::write(&loadouts, "[pool.team]\nhandler = 3\n").unwrap();
        assert!(!poll(&state));
        assert_eq!(refusals(&state).len(), 1);
        assert_eq!(
            profiles_of(&state),
            ["team/alpha"],
            "the last good loadouts run"
        );
        assert!(
            state
                .current()
                .station
                .credentials
                .catalog()
                .and_then(|catalog| catalog.pool("team"))
                .is_some()
        );

        fs::remove_file(&loadouts).unwrap();
        assert!(poll(&state));
        assert!(state.current().station.credentials.catalog().is_none());
        assert!(
            profiles_of(&state).is_empty(),
            "no loadouts, no pool accounts"
        );
    }

    /// Two Stations sharing one SQL roster and Registry: an Agent edited through the source
    /// reaches both without a restart; a revision written past the source's checks is refused
    /// by each, audited by each, and the roster they ran keeps running.
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    fn an_edit_in_a_shared_sql_roster_reaches_every_station(location: &str) {
        use crate::directorates::m::config::{ConfigRecord, ConfigScope, SqlProvider};
        use crate::directorates::registry::sql::db::{Clock, Database, Location};
        use crate::directorates::registry::sql::{SqlOptions, SqlRegistry};
        use serde_json::json;

        let admin = SqlProvider::open(location).unwrap();
        let global = ConfigScope::Global;
        admin
            .put(
                ConfigRecord::Operation,
                &global,
                "G",
                &json!({"roster": ["007"]}),
            )
            .unwrap();
        admin
            .put(
                ConfigRecord::Agent,
                &global,
                "007",
                &json!({"handler": "mock-agent", "priority": 1}),
            )
            .unwrap();
        let registry: Arc<dyn crate::directorates::registry::backend::RegistryBackend> =
            Arc::new(SqlRegistry::open(location).unwrap());
        let roots = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let stations = ["station-a", "station-b"].map(|name| {
            let root = &roots[usize::from(name == "station-b")];
            let mut state = test_state(root.path(), "", "");
            state.station_id = name.to_owned();
            state.intake = registry.clone();
            live(
                &mut state,
                root.path(),
                Arc::new(SqlProvider::open(location).unwrap()),
                false,
            );
            state
        });
        for station in &stations {
            assert_eq!(priority(&station.current()), 1);
        }

        let edited = admin
            .put(
                ConfigRecord::Agent,
                &global,
                "007",
                &json!({"handler": "mock-agent", "priority": 9}),
            )
            .unwrap();
        for station in &stations {
            assert!(poll(station), "{} takes up the edit", station.station_id);
            assert_eq!(priority(&station.current()), 9);
            assert_eq!(
                station.config.status().revision,
                Some(edited.get().to_string())
            );
        }

        // A writer outside AI6 that skips the checks: the row lands, the revision moves.
        let db =
            Database::open(Location::parse(location).unwrap(), &SqlOptions::default()).unwrap();
        db.write(Clock::Config, |tx, revision| {
            tx.execute(
                "INSERT INTO ai6_config_agent (key, definition, revision) VALUES ($1, $2, $3)",
                &[
                    "009".into(),
                    r#"{"clearance":"root"}"#.into(),
                    revision.into(),
                ],
            )
            .map(|_| ())
        })
        .unwrap();
        for station in &stations {
            assert!(!poll(station));
            assert!(!poll(station));
            assert_eq!(priority(&station.current()), 9, "the last good roster runs");
            assert_eq!(station.config.status().refused.len(), 1);
        }
        let refused = refusals(&stations[0]);
        let mut by = refused
            .iter()
            .map(|record| record.actor.as_str())
            .collect::<Vec<_>>();
        by.sort_unstable();
        assert_eq!(
            by,
            ["station:station-a", "station:station-b"],
            "{refused:?}"
        );

        admin.remove(ConfigRecord::Agent, &global, "009").unwrap();
        for station in &stations {
            assert!(poll(station));
            assert!(station.config.status().refused.is_empty());
        }
    }

    #[test]
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    fn an_edit_in_a_shared_sql_roster_reaches_every_station_without_a_restart() {
        #[cfg(feature = "sqlite")]
        {
            let store = crate::directorates::registry::suite::SqliteStore::new();
            an_edit_in_a_shared_sql_roster_reaches_every_station(&store.location());
        }
        #[cfg(feature = "postgres")]
        match crate::directorates::registry::suite::PostgresStore::new() {
            Some(store) => an_edit_in_a_shared_sql_roster_reaches_every_station(&store.location()),
            None => eprintln!(
                "skipped the PostgreSQL run: {} is not set",
                crate::directorates::registry::suite::POSTGRES_URL_ENV
            ),
        }
    }

    #[test]
    fn restart_only_families_and_files_are_reported_as_needing_a_restart() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", AGENTS);
        live(
            &mut state,
            root.path(),
            Arc::new(FileProvider::new(root.path())),
            false,
        );
        let theaters = root.path().join("theaters.toml");
        fs::write(&theaters, "[theater.box]\naccess = \"local\"\n").unwrap();
        poll(&state);
        assert_eq!(state.config.status().needs_restart, ["theater"]);
        assert!(
            state.current().station.theaters.find("box").is_none(),
            "a restart applies it, not a poll"
        );
        let agents = root.path().join("agents.toml");
        fs::write(&agents, AGENTS.replace("priority = 1", "priority = 6")).unwrap();
        assert!(poll(&state), "an Agent applies live beside it");
        assert_eq!(priority(&state.current()), 6);
        fs::remove_file(&theaters).unwrap();
        poll(&state);
        assert!(state.config.status().needs_restart.is_empty());

        fs::write(root.path().join("cables.toml"), "max_fanout = 3\n").unwrap();
        poll(&state);
        assert_eq!(state.config.status().needs_restart, ["cables"]);
    }
}
