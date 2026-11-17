//! Live configuration: the roster and loadouts a Station reads again while it runs.
//!
//! Every poll ([`DEFAULT_CONFIG_POLL`](super::DEFAULT_CONFIG_POLL) unless the Station says
//! otherwise) asks the configuration provider for the revision of the Station's own view. A
//! new revision is loaded, which checks it as the family files are checked, and the families
//! that apply live are applied: requests, the scheduler and new Runs use them, while a
//! running Run keeps the configuration it started with. A changed family that only a
//! restart applies is reported as needing one, at check-in and in the log. A revision that is
//! not acceptable is refused once, logged and audited, and the last good configuration keeps
//! running. When its loadouts change, the Station declares its pool accounts again.

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::{DaemonState, StationConfig, accounts, publish_station_check_in};
use crate::directorates::m::compartments::{self, CompiledCompartments};
use crate::directorates::m::config::{
    ConfigProvider, ConfigRecord, ConfigRevision, ConfigSnapshot, ConfigStatus,
};
use crate::directorates::m::placement::Degradation;
use crate::directorates::m::reports::{self, ReportSettings};
use crate::directorates::m::roster::{Roster, RosterError, RosterErrorKind};
use crate::directorates::m::theaters::{Theater, Theaters};
use crate::directorates::medical::watchdog::WatchdogConfig;
use crate::directorates::q::LoadoutCatalog;
use crate::directorates::registry::store::AuditRecord;

/// The audit action of a refused configuration.
const REFUSED_ACTION: &str = "config.refused";
/// The Registry's bound on an audit target.
/// How often the poll task checks whether the Station is stopping.
const STOP_CHECK: Duration = Duration::from_millis(250);

/// The parts of a Station's configuration its `station` record sets.
#[derive(Clone)]
pub(super) struct RuntimeParts {
    pub(super) station: StationConfig,
    pub(super) watchdog: WatchdogConfig,
    pub(super) capacity: usize,
}

/// The configuration a Station runs and where it reads it again, shared by every copy of the
/// Station's state. The default holds nothing and reads nothing: each copy keeps its own.
#[derive(Default)]
pub(super) struct LiveConfig {
    source: Option<Arc<dyn ConfigProvider>>,
    /// The Station's configuration without its `station` record, which each revision of the record
    /// is applied over.
    base: Option<RuntimeParts>,
    /// Theaters the Station was given in code, declared with every revision of the provider's.
    extra_theaters: Vec<Theater>,
    /// How much of a refused request the Station's API reads, which a changed `[api]` table updates.
    api_drain: std::sync::OnceLock<Arc<std::sync::atomic::AtomicUsize>>,
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
    /// The `station` record the Station applies now, and the configuration it makes.
    station: crate::directorates::m::config::StationRuntime,
    applied: Option<RuntimeParts>,
    revision: Option<ConfigRevision>,
    /// The roster revision refused last, and why.
    refused: Option<(ConfigRevision, String)>,
    /// The stored records the revision this Station runs left out because they cannot be read.
    unreadable: Vec<crate::directorates::m::config::RefusedRecord>,
    needs_restart: BTreeSet<&'static str>,
    /// The `registry` record the Station's Registry applies now.
    registry: crate::directorates::m::config::RegistryRecord,
    /// The Theaters the Station declares now; `None` when they were set in code.
    theaters: Option<Theaters>,
    /// The weakenings of the Station TLS policy it follows now, once a record has changed them.
    tls_weakened: Option<Vec<String>>,
    /// The `cables` record the Station's relay follows now.
    cables: crate::directorates::registry::cables::CableConfig,
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
        let registry = started.settings.registry.clone();
        let started_station = started.settings.station.clone();
        let started_cables = started.settings.cables.clone();
        let started_unreadable = started.refused.clone();
        Self {
            source: Some(source),
            base: None,
            extra_theaters: Vec::new(),
            api_drain: std::sync::OnceLock::new(),
            station: station.to_owned(),
            started: Some(started),
            live: Mutex::new(Live {
                roster: Some(roster),
                catalog,
                revision: Some(revision),
                unreadable: started_unreadable,
                station: started_station,
                cables: started_cables,
                registry,
                ..Live::default()
            }),
        }
    }

    /// A configuration that runs by `limits`, for tests of what reads them.
    #[cfg(test)]
    pub(super) fn with_station_limits(self, limits: crate::station::limits::StationLimits) -> Self {
        {
            let mut live = self.lock();
            let station = StationConfig {
                limits: Arc::new(limits),
                ..StationConfig::default()
            };
            live.applied = Some(RuntimeParts {
                station,
                watchdog: WatchdogConfig::default(),
                capacity: 1,
            });
        }
        self
    }

    /// Has a changed `[api]` table reach the API\x27s check of a refused request.
    pub(super) fn follow_api_drain(&self, drain: Arc<std::sync::atomic::AtomicUsize>) {
        let _ = self.api_drain.set(drain);
    }

    /// The tables of the `station` record this Station runs by now, each read where it is used
    /// rather than copied when the Station started.
    pub(super) fn limits(&self) -> Arc<crate::station::limits::StationLimits> {
        self.lock()
            .applied
            .as_ref()
            .map(|applied| applied.station.limits.clone())
            .unwrap_or_default()
    }

    /// Makes the Station follow the provider's Theaters, declared with `extra` and starting as
    /// `theaters`.
    pub(super) fn with_theaters(mut self, theaters: Theaters, extra: Vec<Theater>) -> Self {
        self.extra_theaters = extra;
        self.lock().theaters = Some(theaters);
        self
    }

    /// Makes the Station apply each revision of its `station` record over `base`.
    pub(super) fn with_runtime_base(self, base: RuntimeParts) -> Self {
        {
            let mut live = self.lock();
            let mut applied = base.clone();
            let result = super::apply_runtime_parts(&mut applied, &live.station);
            live.applied = result.is_ok().then_some(applied);
        }
        Self {
            base: Some(base),
            ..self
        }
    }

    fn lock(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The roster the Station runs now, when it reads one.
    pub(super) fn roster(&self) -> Option<Arc<Roster>> {
        self.lock().roster.clone()
    }

    /// The compartments as the provider holds them at this moment; none when the Station reads
    /// no provider.
    ///
    /// # Errors
    ///
    /// When the provider cannot be read: the caller refuses, and nothing answers from memory.
    pub(super) fn compartments(&self) -> Result<CompiledCompartments, RosterError> {
        match &self.source {
            Some(source) => compartments::current(source.as_ref()),
            None => Ok(CompiledCompartments::default()),
        }
    }

    /// The lists Reports are marked and prioritised from, read from the provider now; without
    /// one, the running roster's.
    ///
    /// # Errors
    ///
    /// When the provider cannot be read: the caller refuses, and nothing answers from memory.
    pub(super) fn report_settings(&self) -> Result<ReportSettings, RosterError> {
        match &self.source {
            Some(source) => reports::settings(source.as_ref()),
            None => Ok(self
                .roster()
                .map(|roster| roster.settings().reports.clone())
                .unwrap_or_default()),
        }
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
        if let Some(theaters) = &live.theaters {
            state.station.theaters = theaters.clone();
        }
        #[cfg(feature = "station-tls")]
        if let Some(weakened) = &live.tls_weakened {
            state.station.tls_weakened = weakened.clone();
        }
        if let Some(applied) = &live.applied {
            state.station.roles = applied.station.roles.clone();
            state.station.labels = applied.station.labels.clone();
            state.station.terminal = applied.station.terminal.clone();
            state.station.limits = applied.station.limits.clone();
            state.station.hygiene = applied.station.hygiene.clone();
            state.watchdog = applied.watchdog.clone();
            state.max_concurrent_missions = applied.capacity;
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
                // The source reads files or a database, so it is asked on the blocking pool. A
                // login's revision is read at the same beat: a renewal reaches running Runs
                // within one poll.
                let _ = tokio::task::spawn_blocking(move || {
                    poll(&polling);
                    super::logins::tend(&polling);
                })
                .await;
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
        super::agents::log_handler_weakenings(&current.plugins, &current.roster);
        for weakened in current.roster.settings().weakenings() {
            eprintln!("ai6: {weakened}");
        }
        log_compartment_weakenings(&current);
        #[cfg(feature = "publisher")]
        super::assistant::log_weakenings(&current.roster);
        super::agents::log_retirements(&current.roster, current.clock.now());
    }
    if loadouts {
        for weakening in current.station.credentials.weakenings() {
            eprintln!("ai6: {weakening}");
        }
        for refused in current
            .station
            .credentials
            .check_providers(&current.plugins)
        {
            eprintln!("ai6: {refused}");
        }
    }
    accounts::declare(&current);
    publish_station_check_in(&current);
    true
}

/// Logs the records `snapshot` left out because they cannot be read, and those it read in an
/// older shape.
pub(super) fn log_record_notes(snapshot: &ConfigSnapshot) {
    for record in &snapshot.refused {
        eprintln!("ai6: configuration record refused, the rest is applied: {record}");
    }
    for record in &snapshot.migrated {
        eprintln!(
            "ai6: {} {:?} was stored in an older shape and is read as migrated ({}); \
             saving it stores the new shape",
            record.family, record.key, record.migration
        );
    }
}

/// What the configuration this Station runs left out or refused, for its health.
pub(super) fn record_degradations(state: &DaemonState) -> Vec<Degradation> {
    let live = state.config.lock();
    let mut found: Vec<Degradation> = live
        .unreadable
        .iter()
        .map(|record| {
            Degradation::new(
                "config-record",
                format!("{} {}", record.family, record.key),
                format!("refused, it cannot be read: {}", record.reason),
                vec![format!("what depends on this {}", record.family)],
            )
        })
        .collect();
    if let Some((revision, reason)) = &live.refused {
        found.push(Degradation::new(
            "config",
            format!("revision {}", revision.get()),
            format!("refused, the Station keeps the one it runs: {reason}"),
            vec!["changes made since".to_owned()],
        ));
    }
    found
}

/// Logs each compartment limit looser than its default, and a record that grants nothing because
/// it cannot be read.
pub(super) fn log_compartment_weakenings(state: &DaemonState) {
    let Ok(compiled) = state.config.compartments() else {
        return;
    };
    for compartment in compiled.iter() {
        for weakened in compartment.weakenings() {
            eprintln!("ai6: {weakened}");
        }
    }
    for reason in compiled.refused().values() {
        eprintln!("ai6: a compartment grants nothing: {reason}");
    }
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
            let applied = match applied_station(state, &snapshot) {
                Ok(applied) => applied,
                Err(reason) => return refused_revision(state, revision, &reason),
            };
            let theaters = match merged_theaters(state, &snapshot) {
                Ok(theaters) => theaters,
                Err(reason) => return refused_revision(state, revision, &reason),
            };
            restart_families(state, &snapshot);
            let loadouts = (snapshot.settings.loadouts != LoadoutCatalog::default())
                .then(|| Arc::new(snapshot.settings.loadouts.clone()));
            let mut live = state.config.lock();
            let loadouts_changed =
                live.catalog.is_some() && live.catalog.as_ref() != Some(&loadouts);
            if live.catalog.is_some() {
                live.catalog = Some(loadouts);
            }
            apply_registry(state, &mut live, &snapshot.settings.registry);
            apply_cables(state, &mut live, &snapshot.settings.cables);
            apply_theaters(state, &mut live, theaters);
            apply_tls(state, &mut live, &snapshot.settings);
            if let Some(applied) = applied {
                let limits = applied.station.limits.clone();
                state.hub.set_limits(limits.attach.clone());
                #[cfg(feature = "publisher")]
                if let Some(publisher) = &state.publisher {
                    publisher.set_limits(limits.publisher.clone());
                }
                state.transports.set_limits(limits.clone());
                state
                    .transports
                    .set_resources_every(applied.station.resources_every);
                if let Some(drain) = state.config.api_drain.get() {
                    drain.store(
                        limits.api.refused_body_drain_bytes.or_max(),
                        Ordering::Relaxed,
                    );
                }
                live.applied = Some(applied);
                live.station = snapshot.settings.station.clone();
                eprintln!("ai6: station settings applied");
            }
            if live.unreadable != snapshot.refused {
                log_record_notes(&snapshot);
                live.unreadable = snapshot.refused.clone();
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
        Err(error) => refused_revision(state, revision, &error.to_string()),
    }
}

/// Records that `revision` was refused for `reason`, logs and audits it, and reports that nothing
/// changed.
fn refused_revision(state: &DaemonState, revision: ConfigRevision, reason: &str) -> (bool, bool) {
    let reason = format!("configuration revision {}: {reason}", revision.get());
    state.config.lock().refused = Some((revision, reason.clone()));
    refuse(state, &reason);
    (false, false)
}

/// The configuration the revision's `station` record makes when it differs from the one applied, `None`
/// when it is the same or the Station has no base to apply it over.
fn applied_station(
    state: &DaemonState,
    snapshot: &ConfigSnapshot,
) -> Result<Option<RuntimeParts>, String> {
    let Some(base) = &state.config.base else {
        return Ok(None);
    };
    if state.config.lock().station == snapshot.settings.station {
        return Ok(None);
    }
    let mut parts = base.clone();
    super::apply_runtime_parts(&mut parts, &snapshot.settings.station)
        .map_err(|error| error.to_string())?;
    if parts.station.roles.is_empty() {
        return Err("a Station needs at least one role".to_owned());
    }
    Ok(Some(parts))
}

/// Gives the Station's Registry the limits of `record` when they are not the ones it applies,
/// and logs each one looser than its default. A lower limit refuses new writes past it and
/// removes nothing stored.
fn apply_registry(
    state: &DaemonState,
    live: &mut Live,
    record: &crate::directorates::m::config::RegistryRecord,
) {
    if &live.registry == record {
        return;
    }
    record.apply_to(state.intake.as_ref());
    live.registry = record.clone();
    eprintln!("ai6: Registry limits applied");
    for weakened in record.weakenings() {
        eprintln!("ai6: {weakened}");
    }
}

/// The Theaters the snapshot declares and the Station was given in code, when the Station follows
/// its provider's Theaters.
fn merged_theaters(
    state: &DaemonState,
    snapshot: &ConfigSnapshot,
) -> Result<Option<Theaters>, String> {
    if state.config.lock().theaters.is_none() {
        return Ok(None);
    }
    let mut theaters = snapshot.settings.theaters.clone();
    for theater in &state.config.extra_theaters {
        theaters
            .insert(theater.clone())
            .map_err(|error| format!("AI6 Theaters: {error}"))?;
    }
    Ok(Some(theaters))
}

/// Gives the Station the Theaters it declares now and builds the transport of each new or changed
/// one. A Run already on a Theater keeps the transport it started with.
fn apply_theaters(state: &DaemonState, live: &mut Live, theaters: Option<Theaters>) {
    let Some(theaters) = theaters else {
        return;
    };
    if live.theaters.as_ref() == Some(&theaters) {
        return;
    }
    state.transports.reconcile(&theaters, &state.plugins);
    for theater in theaters.reachable() {
        for weakened in theater.weakenings() {
            eprintln!("ai6: Theater {:?} is weakened: {weakened}", theater.id);
        }
    }
    live.theaters = Some(theaters);
    eprintln!("ai6: Theaters applied");
}

/// Makes the Station report, and warn of, the TLS policy it now follows. A connection made after
/// the change is checked against the new policy; none already open is touched.
fn apply_tls(
    state: &DaemonState,
    live: &mut Live,
    settings: &crate::directorates::m::config::Settings,
) {
    #[cfg(feature = "station-tls")]
    {
        let Ok(policy) = settings.tls_policy() else {
            return;
        };
        let weakened: Vec<String> = policy
            .non_defaults_against(&coqui_peer_tls::Policy::default())
            .into_iter()
            .map(|weakening| {
                format!(
                    "{} = {} (default {})",
                    weakening.setting, weakening.value, weakening.default
                )
            })
            .collect();
        let running = live
            .tls_weakened
            .as_ref()
            .unwrap_or(&state.station.tls_weakened);
        if *running == weakened {
            return;
        }
        for weakening in &weakened {
            eprintln!("ai6: warning: the Station TLS policy: {weakening}");
        }
        eprintln!("ai6: Station TLS policy applied");
        live.tls_weakened = Some(weakened);
    }
    #[cfg(not(feature = "station-tls"))]
    let _ = (state, live, settings);
}

/// Makes the Station's Cable relay follow `config` when it is not the one it follows. A relay that
/// refuses it keeps what it had, and the refusal is logged.
fn apply_cables(
    state: &DaemonState,
    live: &mut Live,
    config: &crate::directorates::registry::cables::CableConfig,
) {
    if &live.cables == config {
        return;
    }
    match state.cables.apply_config(config) {
        Ok(()) => {
            live.cables = config.clone();
            eprintln!("ai6: Cable relay settings applied");
            for weakened in config.weakenings() {
                eprintln!("ai6: {weakened}");
            }
        }
        Err(error) => {
            eprintln!("ai6: Cable relay settings refused, keeping the ones it runs: {error}")
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
    for record in ConfigRecord::ALL {
        let pending = if record.applies_live() {
            partly_restart_only(started, snapshot, record)
        } else {
            changed.contains(&record)
        };
        if !pending {
            live.needs_restart.remove(record.name());
        } else if live.needs_restart.insert(record.name()) {
            eprintln!("ai6: {record} changed; restart the Station to apply it");
        }
    }
}

/// Whether a family that applies live holds keys only a restart applies, and one of them changed.
fn partly_restart_only(
    started: &ConfigSnapshot,
    snapshot: &ConfigSnapshot,
    record: ConfigRecord,
) -> bool {
    match record {
        ConfigRecord::Station => started
            .settings
            .station
            .restart_only_differs(&snapshot.settings.station),
        _ => false,
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
        let snapshot = source.load_degraded(&state.station_id).unwrap();
        let catalog = loadouts.then(|| {
            (snapshot.settings.loadouts != LoadoutCatalog::default())
                .then(|| Arc::new(snapshot.settings.loadouts.clone()))
        });
        state.roster = Arc::new(snapshot.roster.clone());
        state
            .station
            .credentials
            .set_catalog(catalog.clone().flatten());
        state.config = Arc::new(
            LiveConfig::new(
                source,
                &state.station_id,
                snapshot,
                state.roster.clone(),
                catalog,
            )
            .with_runtime_base(RuntimeParts {
                station: state.station.clone(),
                watchdog: state.watchdog.clone(),
                capacity: state.max_concurrent_missions,
            })
            .with_theaters(state.station.theaters.clone(), Vec::new()),
        );
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

    #[test]
    fn a_stored_record_a_release_cannot_read_starts_the_station_degraded_naming_it() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", AGENTS);
        let operations = root.path().join("operations.toml");
        let checkout = root.path().join("checkout");
        let good = format!("[operation.G]\npath = {:?}\n", checkout.to_string_lossy());
        fs::write(
            &operations,
            format!("{good}[operation.H.workspace]\nmystery = 1\n"),
        )
        .unwrap();
        let source = Arc::new(FileProvider::new(root.path()));
        let snapshot = source.load_degraded(&state.station_id).unwrap();
        assert_eq!(snapshot.refused.len(), 1);
        live(&mut state, root.path(), source, false);

        let degraded = super::super::health::degradations(&state);
        let named: Vec<_> = degraded
            .iter()
            .filter(|degradation| degradation.kind == "config-record")
            .collect();
        assert_eq!(named.len(), 1, "{degraded:?}");
        assert_eq!(named[0].subject, "operation H");
        assert!(named[0].reason.contains("mystery"), "{}", named[0].reason);
        assert!(state.roster.operation("G").is_ok(), "the rest runs");
        assert!(!poll(&state), "nothing changed, and nothing exits");

        // A later revision with the record still unreadable is refused whole, and named too.
        fs::write(
            &operations,
            format!("{good}[operation.H.workspace]\nmystery = 2\n"),
        )
        .unwrap();
        assert!(!poll(&state));
        assert!(
            super::super::health::degradations(&state)
                .iter()
                .any(|degradation| degradation.kind == "config"),
            "the refused revision is named"
        );

        // Mending the record clears it.
        fs::write(&operations, format!("{good}[operation.H]\n")).unwrap();
        assert!(poll(&state));
        assert!(
            super::super::health::degradations(&state)
                .iter()
                .all(|degradation| !degradation.kind.starts_with("config")),
        );
        assert!(state.current().roster.operation("H").is_ok());
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

    /// The Registry's limits are the `registry` record: a Station applies each revision of it
    /// at its next poll, a lower limit refuses what is written after it and keeps what is
    /// stored, and no record means the defaults.
    fn a_changed_registry_record_changes_a_running_registry(
        registry: Arc<dyn crate::directorates::registry::backend::RegistryBackend>,
        source: Arc<dyn ConfigProvider>,
        root: &Path,
        set: &dyn Fn(Option<serde_json::Value>),
    ) {
        use crate::directorates::registry::limits::RegistryLimits;
        use crate::directorates::registry::runs::NewRun;

        let ttl = Duration::from_secs(60);
        let open = |id: &str| registry.open_run(NewRun::new(id, id, "station-a"), ttl);
        let mut state = test_state(root, "", "");
        state.intake = registry.clone();
        live(&mut state, root, source, false);
        assert_eq!(*registry.limits(), RegistryLimits::default());
        open("run-1").unwrap();
        open("run-2").unwrap();

        set(Some(
            serde_json::json!({ "limits": { "runs": { "records": 2 } } }),
        ));
        assert_eq!(
            registry.limits().runs.records,
            RegistryLimits::default().runs.records,
            "nothing changes before the poll"
        );
        assert!(poll(&state));
        assert_eq!(registry.limits().runs.records, crate::bounds::Limit::at(2));
        let refused = open("run-3").unwrap_err();
        assert!(
            refused.to_string().contains("too many Run records"),
            "{refused}"
        );

        set(Some(
            serde_json::json!({ "limits": { "runs": { "records": 1 } } }),
        ));
        assert!(poll(&state));
        assert_eq!(
            registry.runs().unwrap().len(),
            2,
            "a lower limit keeps what is stored"
        );
        assert!(
            open("run-3").is_err(),
            "and refuses what is written past it"
        );

        set(Some(
            serde_json::json!({ "limits": { "runs": { "records": 10 } } }),
        ));
        assert!(poll(&state));
        open("run-3").unwrap();
        assert_eq!(registry.runs().unwrap().len(), 3);

        set(None);
        assert!(poll(&state));
        assert_eq!(*registry.limits(), RegistryLimits::default());
        assert!(!poll(&state), "nothing changed since");
    }

    #[test]
    fn a_changed_registry_record_changes_a_running_file_registry_at_the_next_poll() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("registry.toml");
        a_changed_registry_record_changes_a_running_registry(
            Arc::new(crate::directorates::registry::store::FileRegistry::open(
                root.path().join("registry"),
            )),
            Arc::new(FileProvider::new(root.path())),
            root.path(),
            &|record| match record {
                Some(record) => {
                    let text = crate::directorates::m::config::json_to_toml(record).unwrap();
                    fs::write(&path, toml::to_string(&text).unwrap()).unwrap();
                }
                None => fs::remove_file(&path).unwrap(),
            },
        );
    }

    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    fn a_changed_registry_record_changes_a_running_sql_registry(location: &str) {
        use crate::directorates::m::config::{ConfigRecord, ConfigScope, SqlProvider};
        use crate::directorates::registry::sql::SqlRegistry;

        let admin = SqlProvider::open(location).unwrap();
        let root = tempfile::tempdir().unwrap();
        a_changed_registry_record_changes_a_running_registry(
            Arc::new(SqlRegistry::open(location).unwrap()),
            Arc::new(SqlProvider::open(location).unwrap()),
            root.path(),
            &|record| {
                let global = ConfigScope::Global;
                match record {
                    Some(record) => {
                        admin
                            .put(
                                ConfigRecord::Registry,
                                &global,
                                ConfigRecord::SINGLETON_KEY,
                                &record,
                            )
                            .unwrap();
                    }
                    None => {
                        admin
                            .remove(ConfigRecord::Registry, &global, ConfigRecord::SINGLETON_KEY)
                            .unwrap();
                    }
                }
            },
        );
    }

    #[test]
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    fn a_changed_registry_record_changes_a_running_sql_registry_at_the_next_poll() {
        #[cfg(feature = "sqlite")]
        {
            let store = crate::directorates::registry::suite::SqliteStore::new();
            a_changed_registry_record_changes_a_running_sql_registry(&store.location());
        }
        #[cfg(feature = "postgres")]
        match crate::directorates::registry::suite::PostgresStore::new() {
            Some(store) => {
                a_changed_registry_record_changes_a_running_sql_registry(&store.location())
            }
            None => eprintln!(
                "skipped the PostgreSQL run: {} is not set",
                crate::directorates::registry::suite::POSTGRES_URL_ENV
            ),
        }
    }

    /// A charge put into the shared database, as the gateway puts one, is the charge a running
    /// Station keeps from its next poll.
    fn a_charge_set_in_a_shared_sql_roster_reaches_a_running_station(location: &str) {
        use crate::directorates::m::config::{ConfigRecord, ConfigScope, SqlProvider};
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
                &json!({"handler": "mock-agent", "clearance": "00"}),
            )
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", "");
        live(
            &mut state,
            root.path(),
            Arc::new(SqlProvider::open(location).unwrap()),
            false,
        );
        let scope = "operation:G";
        assert!(state.current().roster.charges().get(scope).is_none());
        assert!(!poll(&state), "nothing changed");

        let set = admin
            .put(
                ConfigRecord::Charge,
                &global,
                scope,
                &json!({"agent": "007", "max_turns_per_hour": 4}),
            )
            .unwrap();
        assert!(poll(&state), "the next poll takes the charge up");
        let current = state.current();
        let charge = current.roster.charges().get(scope).expect("the charge");
        assert_eq!(charge.settings.agent, "007");
        assert_eq!(charge.settings.max_turns_per_hour, crate::Limit::at(4));
        assert_eq!(state.config.status().revision, Some(set.get().to_string()));
        assert!(state.config.status().needs_restart.is_empty());

        let changed = admin
            .put(
                ConfigRecord::Charge,
                &global,
                scope,
                &json!({"agent": "007", "max_turns_per_hour": 9}),
            )
            .unwrap();
        assert!(poll(&state));
        assert_eq!(
            state.current().roster.charges()[scope]
                .settings
                .max_turns_per_hour,
            crate::Limit::at(9)
        );
        assert_eq!(
            state.config.status().revision,
            Some(changed.get().to_string())
        );

        admin.remove(ConfigRecord::Charge, &global, scope).unwrap();
        assert!(poll(&state));
        assert!(state.current().roster.charges().get(scope).is_none());
    }

    #[test]
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    fn a_charge_set_in_the_shared_database_reaches_a_running_station_without_a_restart() {
        #[cfg(feature = "sqlite")]
        {
            let store = crate::directorates::registry::suite::SqliteStore::new();
            a_charge_set_in_a_shared_sql_roster_reaches_a_running_station(&store.location());
        }
        #[cfg(feature = "postgres")]
        match crate::directorates::registry::suite::PostgresStore::new() {
            Some(store) => {
                a_charge_set_in_a_shared_sql_roster_reaches_a_running_station(&store.location())
            }
            None => eprintln!(
                "skipped the PostgreSQL run: {} is not set",
                crate::directorates::registry::suite::POSTGRES_URL_ENV
            ),
        }
    }

    #[test]
    fn a_changed_station_record_changes_a_running_station_at_the_next_poll() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", AGENTS);
        live(
            &mut state,
            root.path(),
            Arc::new(FileProvider::new(root.path())),
            false,
        );
        let current = state.current();
        assert_eq!(current.max_concurrent_missions, 4);
        let stall = current.watchdog.stall_timeout();
        let station = root.path().join("station.toml");
        fs::write(
            &station,
            "max_concurrent_missions = 2
stall_timeout = 7
roles = [\"execution\"]
\n             [labels]
zone = \"b\"
[scheduler]
check_in = 0.5
",
        )
        .unwrap();
        poll(&state);
        let current = state.current();
        assert_eq!(current.max_concurrent_missions, 2);
        assert_eq!(current.watchdog.stall_timeout(), Duration::from_secs(7));
        assert_eq!(
            current.station.roles,
            [crate::station::server::StationRole::Execution].into()
        );
        assert_eq!(
            current.station.labels.get("zone").map(String::as_str),
            Some("b")
        );
        assert_eq!(
            current.station.limits().scheduler.check_in.0,
            Duration::from_millis(500)
        );
        assert!(state.config.status().needs_restart.is_empty());

        fs::remove_file(&station).unwrap();
        poll(&state);
        let current = state.current();
        assert_eq!(
            current.max_concurrent_missions, 4,
            "unset keys return to the defaults"
        );
        assert_eq!(current.watchdog.stall_timeout(), stall);
        assert!(current.station.labels.is_empty());

        fs::write(
            &station,
            "[publisher]
queue = 3
",
        )
        .unwrap();
        poll(&state);
        assert_eq!(state.config.status().needs_restart, ["station"]);

        fs::write(
            &station,
            "roles = [\"bogus\"]
",
        )
        .unwrap();
        poll(&state);
        assert_eq!(
            state.current().station.roles.len(),
            2,
            "a refused record changes nothing"
        );
        assert_eq!(state.config.status().refused.len(), 1);
    }

    #[test]
    fn tables_read_where_they_are_used_follow_the_station_record_at_the_next_poll() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", AGENTS);
        live(
            &mut state,
            root.path(),
            Arc::new(FileProvider::new(root.path())),
            false,
        );
        let bearer = crate::station::runner::Bearer::new(
            "token".to_owned(),
            &crate::station::limits::ApiLimits::default(),
        );
        state.config.follow_api_drain(bearer.drain_handle());
        let desk =
            crate::station::sources::SourceDesk::new("test", state.config.limits().sources.clone());
        let before = (
            state.hub.limits().history_frames,
            state.transports.pacing().0,
        );

        fs::write(
            root.path().join("station.toml"),
            "resources_every = 5\n[attach]\nhistory_frames = 3\ncall_margin = 5\ncommand_wait = 6\n[publisher]\nrequest_timeout = 9\n[api]\nrefused_body_drain_bytes = 10\n\
             [sources]\nreport_every = 2\n[transports]\nreprobe_after = 7\n",
        )
        .unwrap();
        poll(&state);
        let limits = state.config.limits();
        assert_eq!(limits.attach.history_frames, crate::bounds::Limit::at(3));
        assert_eq!(limits.attach.call_margin.0, Duration::from_secs(5));
        assert_eq!(limits.attach.command_wait.0, Duration::from_secs(6));
        assert_eq!(limits.publisher.request_timeout.0, Duration::from_secs(9));
        assert_ne!(state.hub.limits().history_frames, before.0);
        assert_eq!(
            state.hub.limits().history_frames,
            crate::bounds::Limit::at(3)
        );
        assert_eq!(state.transports.pacing().0, Duration::from_secs(5));
        assert_eq!(
            state.transports.pacing().1.transports,
            limits.transports,
            "the transports probe by the record's tables"
        );
        assert_eq!(
            bearer
                .drain_handle()
                .load(std::sync::atomic::Ordering::Relaxed),
            10
        );
        desk.set_limits(limits.sources.clone());
        assert_eq!(desk.current_limits().report_every.0, Duration::from_secs(2));
        assert_ne!(before.1, Duration::from_secs(5));
        assert_eq!(
            state.config.status().needs_restart,
            Vec::<String>::new(),
            "none of these needs a restart"
        );
    }

    #[test]
    fn a_changed_cables_record_changes_a_running_relay_at_the_next_poll() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", AGENTS);
        live(
            &mut state,
            root.path(),
            Arc::new(FileProvider::new(root.path())),
            false,
        );
        let default_body = state.cables.body_bytes();
        assert_eq!(
            state.cables.cross_operation(),
            crate::directorates::registry::cables::CrossOperation::default()
        );
        let cables = root.path().join("cables.toml");
        fs::write(
            &cables,
            "body_bytes = 100\ncross_operation = \"station_grants\"\n",
        )
        .unwrap();
        poll(&state);
        assert!(!state.cables.body_bytes().allows(101));
        assert!(state.cables.body_bytes().allows(100));
        assert_eq!(
            state.cables.cross_operation(),
            crate::directorates::registry::cables::CrossOperation::StationGrants
        );
        assert!(state.config.status().needs_restart.is_empty());

        fs::remove_file(&cables).unwrap();
        poll(&state);
        assert_eq!(state.cables.body_bytes(), default_body);
    }

    #[test]
    fn a_changed_theater_record_declares_it_to_a_running_station_at_the_next_poll() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", AGENTS);
        live(
            &mut state,
            root.path(),
            Arc::new(FileProvider::new(root.path())),
            false,
        );
        assert!(state.transports.theater("box").is_none());
        let theaters = root.path().join("theaters.toml");
        fs::write(&theaters, "[theater.box]\naccess = \"local\"\n").unwrap();
        poll(&state);
        assert!(state.current().station.theaters.find("box").is_some());
        assert!(state.transports.theater("box").is_some());
        assert!(state.config.status().needs_restart.is_empty());

        fs::write(
            &theaters,
            "[theater.box]\naccess = \"local\"\nlabels = { zone = \"b\" }\n",
        )
        .unwrap();
        poll(&state);
        assert!(
            state
                .transports
                .theater("box")
                .unwrap()
                .labels
                .contains_key("zone"),
            "a changed Theater gets its new declaration"
        );

        fs::remove_file(&theaters).unwrap();
        poll(&state);
        assert!(state.current().station.theaters.find("box").is_none());
        assert!(state.transports.theater("box").is_none());
        assert!(
            state.transports.theater("local").is_some(),
            "the Station's own host stays"
        );
    }

    #[cfg(feature = "station-tls")]
    #[test]
    fn a_changed_tls_record_changes_what_a_running_station_reports_at_the_next_poll() {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", AGENTS);
        live(
            &mut state,
            root.path(),
            Arc::new(FileProvider::new(root.path())),
            false,
        );
        assert!(state.current().station.tls_weakened.is_empty());
        let tls = root.path().join("tls.toml");
        fs::write(&tls, "tls_versions = [\"1.3\"]\n").unwrap();
        poll(&state);
        let weakened = state.current().station.tls_weakened;
        assert_eq!(weakened.len(), 1, "{weakened:?}");
        assert!(weakened[0].starts_with("tls_versions"), "{weakened:?}");
        assert!(state.config.status().needs_restart.is_empty());

        fs::remove_file(&tls).unwrap();
        poll(&state);
        assert!(state.current().station.tls_weakened.is_empty());
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
        let plugins = root.path().join("plugins.toml");
        fs::write(
            &plugins,
            "[[library]]\nfile = \"a.so\"\npublisher = \"ed25519:0000000000000000000000000000000000000000000000000000000000000000\"\n",
        )
        .unwrap();
        poll(&state);
        assert_eq!(state.config.status().needs_restart, ["plugin"]);
        let agents = root.path().join("agents.toml");
        fs::write(&agents, AGENTS.replace("priority = 1", "priority = 6")).unwrap();
        assert!(poll(&state), "an Agent applies live beside it");
        assert_eq!(priority(&state.current()), 6);
        fs::remove_file(&plugins).unwrap();
        poll(&state);
        assert!(state.config.status().needs_restart.is_empty());
    }
}
