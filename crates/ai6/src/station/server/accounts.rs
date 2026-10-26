//! Pool accounts for a Session: which pools its Agent may draw from, the account claimed for
//! it in the Registry, and the one credential that reaches its harness.
//!
//! An environment kind's value is leased like a loadout secret and lives only in the harness
//! environment. A Codex login kind is logged in to a `CODEX_HOME` the Station makes for the
//! Mission under its state directory, kept while the Mission may resume its thread there and
//! removed once the Mission is over. A `config-dir` account hands the harness its directory.
//! Timeline events and messages carry pool and account names, never a credential.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::directorates::m::mission::Mission;
use crate::directorates::m::placement;
use crate::directorates::medical::exposure::{ExposureGate, Restriction};
use crate::directorates::q::{
    Account, AccountKind, AccountLogin, AccountPool, CREDENTIAL_VARIABLES, CommandLogin, Loadout,
    LoadoutCatalog, MAX_SECRET_LEASE_TTL, PoolChoice, SecretBroker, SecretLease,
    SecretLeaseRequest, choose_pools,
};
use crate::directorates::registry::accounts::{
    AccountClaimRequest, AccountProfile, AccountRef, AccountState, QuotaWindow, choose_account,
};
use crate::directorates::registry::store::DEFAULT_LEASE;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::directorates::section_00::agent::QuotaReport;

use super::DaemonState;

/// How long a claim lasts unless the Station renews it at its check-in: a dark Station's
/// accounts free up with its Session leases.
pub(super) const CLAIM_TTL: Duration = DEFAULT_LEASE;
const TIMELINE_KIND: &str = "account";
const CODEX_CONFIG: &str = "# Written by AI6: this home holds one pool account's login.\n\
                            cli_auth_credentials_store = \"file\"\n";

/// The Station's account logins and the account each of its Missions holds.
#[derive(Clone, Default)]
pub(crate) struct AccountDesk {
    login: Option<Arc<dyn AccountLogin>>,
    homes: Option<PathBuf>,
    held: Arc<Mutex<BTreeMap<String, Held>>>,
    waiting: Arc<Mutex<BTreeMap<String, String>>>,
}

impl std::fmt::Debug for AccountDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountDesk")
            .field("homes", &self.homes)
            .finish_non_exhaustive()
    }
}

impl PartialEq for AccountDesk {
    fn eq(&self, other: &Self) -> bool {
        self.homes == other.homes && Arc::ptr_eq(&self.held, &other.held)
    }
}

impl Eq for AccountDesk {}

impl AccountDesk {
    pub(crate) fn with_login(mut self, login: Arc<dyn AccountLogin>) -> Self {
        self.login = Some(login);
        self
    }

    pub(crate) fn with_homes(mut self, dir: PathBuf) -> Self {
        self.homes = Some(dir);
        self
    }

    fn login(&self) -> Arc<dyn AccountLogin> {
        self.login
            .clone()
            .unwrap_or_else(|| Arc::new(CommandLogin::default()))
    }

    fn homes(&self) -> Result<PathBuf, String> {
        match &self.homes {
            Some(dir) => Ok(dir.clone()),
            None => crate::station::safehouse::state_dir()
                .map(|dir| dir.join("accounts"))
                .map_err(|error| format!("no state directory for account homes: {error}")),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Held>> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The account a Mission's Session holds on this Station.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    account: AccountRef,
    kind: AccountKind,
}

/// Where a Session's harness runs and whether it continues a provider Session.
#[derive(Debug, Clone, Copy)]
pub(super) struct Site {
    /// The Session resumes a provider Session rather than starting one.
    pub(super) resuming: bool,
    /// The harness runs on the Station's own host, where a `CODEX_HOME` it makes is visible.
    pub(super) on_station: bool,
}

/// What an Agent may draw from: its handler's pools that admit `operation`, as narrowed by
/// the Operation's `pools` and the Agent's pin.
fn pools_for<'a>(
    state: &DaemonState,
    catalog: &'a LoadoutCatalog,
    operation: Option<&str>,
    agent_id: &str,
) -> PoolChoice<'a> {
    let Ok(agent) = state.roster.agent(operation, agent_id) else {
        return PoolChoice::Unpooled;
    };
    let allowed = operation
        .and_then(|id| state.roster.operation(id).ok())
        .and_then(|operation| operation.pools.clone());
    choose_pools(
        catalog.pools(),
        agent.handler.as_deref(),
        operation,
        allowed.as_deref(),
        agent.pool.as_deref(),
    )
}

/// Claims an account for `mission`'s Session and puts its credential in `environment`,
/// after removing every other credential variable, so the harness sees exactly one account.
/// Returns the lease to revoke with the Session's, if the credential is a secret.
///
/// An Agent no pool serves gets nothing here. A pool that may not be used, or no account
/// with room, fails the start with the reason. A Session that continues a provider Session
/// keeps its account when that Session cannot move (Codex, `config-dir`).
#[allow(clippy::too_many_arguments)]
pub(super) fn issue(
    state: &DaemonState,
    catalog: &LoadoutCatalog,
    broker: &SecretBroker,
    mission: &Mission,
    agent_id: &str,
    exposure: &ExposureGate,
    site: Site,
    environment: &mut BTreeMap<String, String>,
) -> Result<Option<SecretLease>, String> {
    let operation = mission.operation_id.as_deref();
    let pools = match pools_for(state, catalog, operation, agent_id) {
        PoolChoice::Unpooled => return Ok(None),
        PoolChoice::Refused(reason) => return Err(format!("Agent {agent_id:?}: {reason}")),
        PoolChoice::Pools(pools) => pools,
    };
    for variable in CREDENTIAL_VARIABLES {
        environment.remove(*variable);
    }
    for pool in catalog.pools() {
        for account in pool.accounts() {
            if let Some(variable) = account.variable(pool.handler()) {
                environment.remove(&variable);
            }
        }
    }
    let mission_id = mission.mission_id.as_str();
    if exposure.restricts(Restriction::Secrets) {
        eprintln!(
            "ai6: Mission {mission_id:?} read untrusted input and its Operation restricts \
             secrets, so its Session gets no pool account"
        );
        return Ok(None);
    }
    let now = state.clock.now() as f64;
    let earlier = state
        .intake
        .latest_session(mission_id)
        .ok()
        .flatten()
        .and_then(|record| record.account);
    let (pool, chosen) = claim(state, &pools, mission_id, earlier.as_ref(), site, now)?;
    let account = pool
        .get(&chosen.account.account)
        .ok_or("the claimed account is not configured")?;
    match hand_over(state, broker, pool, account, mission_id, site, environment) {
        Ok(lease) => {
            state.station.credentials.accounts.lock().insert(
                mission_id.to_owned(),
                Held {
                    account: chosen.account.clone(),
                    kind: account.kind(),
                },
            );
            if let Some(operation) = operation {
                record(
                    state,
                    TimelineEvent::new(TIMELINE_KIND, "claimed", operation, now)
                        .mission(mission_id)
                        .actor(agent_id)
                        .detail("account", chosen.account.to_string())
                        .detail("kind", account.kind().as_str()),
                );
            }
            Ok(lease)
        }
        Err(error) => {
            if let Err(release) = state.intake.release_account(&state.station_id, mission_id) {
                eprintln!("ai6: account claim of Mission {mission_id:?} not released: {release}");
            }
            Err(error)
        }
    }
}

/// Claims the first account with room, pool by pool, preferring the account the Mission's
/// last Session used.
fn claim<'a>(
    state: &DaemonState,
    pools: &[&'a AccountPool],
    mission_id: &str,
    earlier: Option<&AccountRef>,
    site: Site,
    now: f64,
) -> Result<(&'a AccountPool, AccountState), String> {
    let request = |pool: &AccountPool| {
        AccountClaimRequest::new(pool, state.station_id.as_str(), mission_id, CLAIM_TTL)
    };
    let failed = |pool: &AccountPool, error: std::io::Error| {
        format!("pool {:?} could not be claimed from: {error}", pool.id())
    };
    if site.resuming
        && let Some(earlier) = earlier
        && let Some(pool) = pools.iter().find(|pool| pool.id() == earlier.pool)
        && pool
            .get(&earlier.account)
            .is_some_and(|account| !account.kind().carries_session())
    {
        let only = request(pool).only(&earlier.account);
        return match state.intake.claim_account(&only, now) {
            Ok(Some(chosen)) => Ok((pool, chosen)),
            Ok(None) => Err(format!(
                "the Mission's provider Session lives with account {earlier}, which has no \
                 room now"
            )),
            Err(error) => Err(failed(pool, error)),
        };
    }
    for pool in pools {
        let mut wanted = request(pool);
        if let Some(earlier) = earlier.filter(|earlier| earlier.pool == pool.id()) {
            wanted = wanted.prefer(&earlier.account);
        }
        match state.intake.claim_account(&wanted, now) {
            Ok(Some(chosen)) => return Ok((pool, chosen)),
            Ok(None) => continue,
            Err(error) => return Err(failed(pool, error)),
        }
    }
    let ids: Vec<String> = pools.iter().map(|pool| pool.id().to_owned()).collect();
    let states = state.intake.accounts().unwrap_or_default();
    Err(placement::no_account_headroom("", &ids, frees_at(pools, &states, now)).reason)
}

/// Puts `account`'s credential where the harness reads it.
fn hand_over(
    state: &DaemonState,
    broker: &SecretBroker,
    pool: &AccountPool,
    account: &Account,
    mission_id: &str,
    site: Site,
    environment: &mut BTreeMap<String, String>,
) -> Result<Option<SecretLease>, String> {
    let name = format!("{}/{}", pool.id(), account.name());
    let variable = account
        .variable(pool.handler())
        .ok_or_else(|| format!("account {name} has no variable for its credential"))?;
    if account.kind() == AccountKind::ConfigDir {
        let dir = account
            .login_dir()
            .ok_or_else(|| format!("account {name} has no directory"))?;
        environment.insert(variable, dir.display().to_string());
        return Ok(None);
    }
    if account.kind().login_flag().is_some() && !site.on_station {
        return Err(format!(
            "account {name} is logged in to a CODEX_HOME on the Station, so its harness must \
             run on the Station's own host"
        ));
    }
    let loadout = Loadout::for_account(pool, account)
        .ok_or_else(|| format!("account {name} reads no secret"))?;
    let secret = loadout.secret_names()[0].clone();
    let request =
        SecretLeaseRequest::with_ttl(state.station_id.clone(), mission_id, MAX_SECRET_LEASE_TTL)
            .map_err(|error| error.to_string())?;
    let lease = broker
        .lease(&loadout, request)
        .map_err(|error| format!("account {name}'s credential could not be leased: {error}"))?;
    let handed = if account.kind().login_flag().is_some() {
        home_for(state, mission_id, pool, account).and_then(|home| {
            let login = state.station.credentials.accounts.login();
            lease
                .with_secret(&secret, |value| login.login(pool, account, &home, value))
                .map_err(|error| error.to_string())?
                .map_err(|error| format!("account {name} was not logged in: {error}"))?;
            environment.insert(variable, home.display().to_string());
            Ok(())
        })
    } else {
        lease
            .with_secret(&secret, |value| {
                environment.insert(variable, value.to_owned());
            })
            .map_err(|error| error.to_string())
    };
    match handed {
        Ok(()) => Ok(Some(lease)),
        Err(error) => {
            let _ = lease.revoke();
            Err(error)
        }
    }
}

/// The Mission's `CODEX_HOME` for `account`, made private to the Station's user, with a
/// configuration that keeps the login in the home rather than the OS keyring, where every
/// account would share one entry.
fn home_for(
    state: &DaemonState,
    mission_id: &str,
    pool: &AccountPool,
    account: &Account,
) -> Result<PathBuf, String> {
    let root = state.station.credentials.accounts.homes()?;
    let mission = root.join(path_component(mission_id));
    let home = mission.join(format!("{}--{}", pool.id(), account.name()));
    let made = (|| {
        fs::create_dir_all(&home)?;
        for dir in [&root, &mission, &home] {
            private(dir)?;
        }
        fs::write(home.join("config.toml"), CODEX_CONFIG)
    })();
    made.map_err(|error| format!("CODEX_HOME {} could not be made: {error}", home.display()))?;
    Ok(home)
}

#[cfg(unix)]
fn private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn private(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// A Mission ID as one path component.
fn path_component(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Frees the account `mission_id`'s Session holds on this Station, once it stops here.
pub(super) fn release(state: &DaemonState, mission_id: &str) {
    if state
        .station
        .credentials
        .accounts
        .lock()
        .remove(mission_id)
        .is_some()
        && let Err(error) = state.intake.release_account(&state.station_id, mission_id)
    {
        eprintln!("ai6: account claim of Mission {mission_id:?} not released: {error}");
    }
}

/// The account `mission_id`'s Session holds on this Station.
pub(super) fn held(state: &DaemonState, mission_id: &str) -> Option<AccountRef> {
    state
        .station
        .credentials
        .accounts
        .lock()
        .get(mission_id)
        .map(|held| held.account.clone())
}

/// At each check-in: renews this Station's claims for the Missions it still runs (dropping
/// any other), and removes the Codex homes of Missions that are over.
pub(super) fn tend(state: &DaemonState) {
    let Some(catalog) = state.station.credentials.catalog() else {
        return;
    };
    if catalog.pools().next().is_none() {
        return;
    }
    let desk = &state.station.credentials.accounts;
    let missions: Vec<String> = desk.lock().keys().cloned().collect();
    let now = state.clock.now() as f64;
    if let Err(error) =
        state
            .intake
            .renew_account_claims(&state.station_id, &missions, CLAIM_TTL, now)
    {
        eprintln!("ai6: account claims not renewed: {error}");
    }
    let Ok(root) = desk.homes() else {
        return;
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    let live: BTreeSet<String> = state
        .missions
        .lock()
        .map(|book| {
            book.list()
                .into_iter()
                .filter(|mission| !super::is_terminal_state(mission.state))
                .map(|mission| path_component(mission.mission_id.as_str()))
                .collect()
        })
        .unwrap_or_default();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !live.contains(&name)
            && !missions
                .iter()
                .any(|mission| path_component(mission) == name)
            && let Err(error) = fs::remove_dir_all(entry.path())
        {
            eprintln!("ai6: account home {name:?} not removed: {error}");
        }
    }
}

/// Records each configured account in the Registry, with the Operations that may draw from
/// its pool, so the board can show it before any Session claims it.
pub(super) fn declare(state: &DaemonState) {
    let Some(catalog) = state.station.credentials.catalog() else {
        return;
    };
    for pool in catalog.pools() {
        let operations: Vec<String> = state
            .roster
            .operations()
            .filter(|operation| {
                pool.admits(Some(&operation.id))
                    && operation
                        .pools
                        .as_ref()
                        .is_none_or(|allowed| allowed.iter().any(|id| id == pool.id()))
            })
            .map(|operation| operation.id.clone())
            .collect();
        for account in pool.accounts() {
            let Some(profile) = AccountProfile::of(pool, account.name(), operations.clone()) else {
                continue;
            };
            if let Err(error) = state
                .intake
                .declare_account(&AccountRef::new(pool.id(), account.name()), &profile)
            {
                eprintln!(
                    "ai6: account {}/{} not declared: {error}",
                    pool.id(),
                    account.name()
                );
            }
        }
        for account in pool.accounts() {
            if let Some(weakening) = account.weakening(pool.id()) {
                eprintln!("ai6: {weakening}");
            }
        }
    }
}

/// Adds `tokens` to the day's count of the account `mission_id`'s Session holds.
pub(super) fn record_usage(state: &DaemonState, mission_id: &str, tokens: u64) {
    if tokens == 0 {
        return;
    }
    if let Some(account) = held(state, mission_id)
        && let Err(error) = state
            .intake
            .record_account_usage(&account, tokens, state.clock.now())
    {
        eprintln!("ai6: usage of account {account} not recorded: {error}");
    }
}

/// Records the quota windows a provider reported on the account `mission_id`'s Session holds.
pub(super) fn observe_quota(state: &DaemonState, mission_id: &str, reports: &[QuotaReport]) {
    if reports.is_empty() {
        return;
    }
    let Some(account) = held(state, mission_id) else {
        return;
    };
    let now = state.clock.now() as f64;
    let windows: Vec<QuotaWindow> = reports
        .iter()
        .map(|report| {
            let mut window = QuotaWindow::new(&report.window, now);
            if let Some(used) = report.used_permille {
                window = window.used_permille(used);
            }
            if let Some(at) = report.resets_at {
                window = window.resets_at(at as f64);
            }
            if let Some(status) = &report.status {
                window = window.status(status);
            }
            window
        })
        .collect();
    if let Err(error) = state.intake.report_quota(&account, &windows) {
        eprintln!("ai6: quota of account {account} not recorded: {error}");
    }
}

/// What happens to a Session whose account hit a provider limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Rotation {
    /// The Session holds no pool account.
    Unpooled,
    /// Another account has room and the provider Session can move to it: resume it there.
    Resume,
    /// Another account has room but the provider Session cannot move: start a new attempt.
    Retry,
    /// No account has room: wait for the limit to reset.
    Wait,
}

/// Marks the account `mission_id`'s Session holds limited until `resets_at` (or
/// `fallback_secs` from now), frees it, and says whether another account of the Mission's
/// pools has room.
pub(super) fn on_limit(
    state: &DaemonState,
    mission: &Mission,
    resets_at: Option<i64>,
    fallback_secs: u64,
) -> Rotation {
    let mission_id = mission.mission_id.as_str();
    let Some(held) = state.station.credentials.accounts.lock().remove(mission_id) else {
        return Rotation::Unpooled;
    };
    let now = state.clock.now();
    let until = resets_at
        .filter(|reset| *reset > now)
        .unwrap_or_else(|| now.saturating_add(i64::try_from(fallback_secs).unwrap_or(i64::MAX)));
    if let Err(error) = state.intake.limit_account(&held.account, until as f64) {
        eprintln!("ai6: account {} not marked limited: {error}", held.account);
    }
    if let Err(error) = state.intake.release_account(&state.station_id, mission_id) {
        eprintln!("ai6: account claim of Mission {mission_id:?} not released: {error}");
    }
    let room = state.station.credentials.catalog().is_some_and(|catalog| {
        let PoolChoice::Pools(pools) = pools_for(
            state,
            catalog,
            mission.operation_id.as_deref(),
            mission.agent_id.as_deref().unwrap_or_default(),
        ) else {
            return false;
        };
        let states = state.intake.accounts().unwrap_or_default();
        has_room(&pools, &states, now as f64)
    });
    let rotation = match (room, held.kind.carries_session()) {
        (false, _) => Rotation::Wait,
        (true, true) => Rotation::Resume,
        (true, false) => Rotation::Retry,
    };
    if let Some(operation) = mission.operation_id.as_deref() {
        let next = match rotation {
            Rotation::Resume => "resume on another account",
            Rotation::Retry => "new attempt on another account",
            _ => "wait for the reset",
        };
        record(
            state,
            TimelineEvent::new(TIMELINE_KIND, "limited", operation, now as f64)
                .mission(mission_id)
                .detail("account", held.account.to_string())
                .detail("until", until.to_string())
                .detail("next", next),
        );
    }
    rotation
}

/// Whether any of `pools` has an account with room at `now`.
fn has_room(pools: &[&AccountPool], states: &[AccountState], now: f64) -> bool {
    pools.iter().any(|pool| {
        let request = AccountClaimRequest::new(pool, "", "", CLAIM_TTL);
        choose_account(
            pool.balancing(),
            request.candidates(),
            &pool_states(pool, states),
            None,
            now,
        )
        .is_some()
    })
}

/// `pool`'s accounts' states, by account name.
fn pool_states(pool: &AccountPool, states: &[AccountState]) -> BTreeMap<String, AccountState> {
    states
        .iter()
        .filter(|state| state.account.pool == pool.id())
        .map(|state| (state.account.account.clone(), state.clone()))
        .collect()
}

/// When the first of `pools`' accounts comes back from its limit, if every account is
/// limited; `None` when one is merely at its `max_concurrent`.
fn frees_at(pools: &[&AccountPool], states: &[AccountState], now: f64) -> Option<f64> {
    let mut earliest: Option<f64> = None;
    for pool in pools {
        let states = pool_states(pool, states);
        for account in pool.accounts() {
            let until = states
                .get(account.name())
                .and_then(|state| state.limited_until)
                .filter(|until| *until > now)?;
            earliest = Some(earliest.map_or(until, |known| known.min(until)));
        }
    }
    earliest
}

/// Why a brief for `agent_id` in `operation_id` waits in the queue: every account of the
/// pools it would draw from is limited or busy. `None` when it may start.
pub(super) fn intake_hold(
    state: &DaemonState,
    operation_id: &str,
    agent_id: &str,
) -> Option<String> {
    let catalog = state.station.credentials.catalog()?;
    let PoolChoice::Pools(pools) = pools_for(state, catalog, Some(operation_id), agent_id) else {
        return None;
    };
    let states = state.intake.accounts().ok()?;
    let now = state.clock.now() as f64;
    if has_room(&pools, &states, now) {
        return None;
    }
    let ids: Vec<String> = pools.iter().map(|pool| pool.id().to_owned()).collect();
    Some(placement::no_account_headroom(agent_id, &ids, frees_at(&pools, &states, now)).reason)
}

/// Records once, on the Operation's timeline, why the brief `key` waits; a brief that may
/// start again is forgotten.
pub(super) fn note_waiting(
    state: &DaemonState,
    key: &str,
    operation_id: &str,
    reason: Option<&str>,
) {
    let mut waiting = state
        .station
        .credentials
        .accounts
        .waiting
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(reason) = reason else {
        waiting.remove(key);
        return;
    };
    if waiting.get(key).map(String::as_str) == Some(reason) {
        return;
    }
    waiting.insert(key.to_owned(), reason.to_owned());
    drop(waiting);
    eprintln!("ai6: brief {key:?} waits: {reason}");
    record(
        state,
        TimelineEvent::new(
            TIMELINE_KIND,
            "waiting",
            operation_id,
            state.clock.now() as f64,
        )
        .detail("brief", key)
        .detail("reason", reason),
    );
}

fn record(state: &DaemonState, event: TimelineEvent) {
    let event = event.station(state.station_id.clone());
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: account event was not recorded on the timeline: {error}");
    }
}

/// A handler for pool tests: it records what each start and resume was given, and its
/// provider stops at a usage limit the first time it runs on a credential containing
/// `-limited-`.
#[cfg(test)]
pub(super) mod testing {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    use crate::directorates::m::identity::SessionId;
    use crate::directorates::q::terminal::AgentTerminal;
    use crate::directorates::section_00::agent::{
        AgentEvent, AgentHandler, AgentResume, AgentSession, AgentStart, LIMIT_EVENT, QuotaReport,
        TokenUsage, UsageLimit,
    };
    use crate::guatiao::library::{Object, ProviderError};
    use crate::guatiao::{Provider, Status};

    use super::super::tests::MockSession;

    /// The reset time the pooled mock reports with its limit.
    pub(in crate::station::server) const POOLED_RESET: i64 = 1_790_003_600;

    /// One call: the Mission, `start` or `resume`, the environment and initial context.
    pub(in crate::station::server) type PooledCall =
        (String, &'static str, BTreeMap<String, String>, Vec<String>);

    pub(in crate::station::server) static POOLED_CALLS: Mutex<Vec<PooledCall>> =
        Mutex::new(Vec::new());
    static LIMITED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
    /// Each start is a new provider Session.
    static STARTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    #[derive(Default, Provider)]
    #[provider(AgentHandler, id = "mock-pooled", name = "Mock Pooled", crate = crate::guatiao)]
    pub(in crate::station::server) struct MockPooledHandler;

    /// The credential the harness was given: an environment token, or the login a fake
    /// `codex login` left in its `CODEX_HOME`.
    fn credential(environment: &BTreeMap<String, String>) -> String {
        environment
            .get("CLAUDE_CODE_OAUTH_TOKEN")
            .cloned()
            .or_else(|| {
                environment.get("CODEX_HOME").and_then(|home| {
                    std::fs::read_to_string(std::path::Path::new(home).join("login")).ok()
                })
            })
            .unwrap_or_default()
    }

    fn outcome(environment: &BTreeMap<String, String>) -> AgentEvent {
        let credential = credential(environment);
        let limited =
            credential.contains("-limited-") && LIMITED.lock().unwrap().insert(credential.clone());
        AgentEvent {
            kind: if limited { LIMIT_EVENT } else { "completed" }.to_owned(),
            text: None,
            data: None,
            limit: limited.then_some(UsageLimit {
                resets_at: Some(POOLED_RESET),
                window: Some("five_hour".to_owned()),
            }),
            usage: (!limited).then_some(TokenUsage {
                input: 100,
                output: 20,
                cache_read: 0,
                cache_write: 0,
                cost_micro_usd: 0,
                cumulative: true,
            }),
            tools: Vec::new(),
            quota: vec![QuotaReport {
                window: "five_hour".to_owned(),
                used_permille: Some(if limited { 1000 } else { 300 }),
                resets_at: Some(POOLED_RESET),
                status: None,
            }],
        }
    }

    fn error(message: &str) -> ProviderError {
        ProviderError::new(Status::GUATIAO_ERR_INTERNAL, message)
    }

    impl AgentHandler for MockPooledHandler {
        fn start(
            &self,
            request: AgentStart,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            let event = outcome(&request.environment);
            POOLED_CALLS.lock().unwrap().push((
                request.mission_id.to_string(),
                "start",
                request.environment,
                request.initial_context,
            ));
            let started = STARTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let session = SessionId::try_new(format!("provider-session-pooled-{started}"))
                .map_err(|failure| error(&failure.to_string()))?;
            Ok(MockSession::with_event(session, event).into_object())
        }

        fn resume(
            &self,
            request: AgentResume,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            let event = outcome(&request.environment);
            POOLED_CALLS.lock().unwrap().push((
                request.mission_id.to_string(),
                "resume",
                request.environment,
                Vec::new(),
            ));
            Ok(MockSession::with_event(request.session_id, event).into_object())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::testing::{POOLED_CALLS, POOLED_RESET, PooledCall};
    use super::*;
    use crate::directorates::m::identity::MissionId;
    use crate::directorates::m::mission::MissionState;
    use crate::directorates::q::{SecretBackend, SecretBackendFailure, SecretRequest, SecretValue};
    use crate::directorates::registry::sessions::SessionStatus;
    use crate::directorates::registry::timeline::TimelineEvent;

    use super::super::tests::{limited_state, start_mission_by_id, wait_for_state};
    use super::super::{
        StartMission, publish_station_check_in, schedule_intake_once, schedule_parked_once,
        schedule_queued_once, session_hold, start_mission,
    };

    const START: i64 = POOLED_RESET - 3_600;

    /// Hands out `fake-<name>-0451` for each secret.
    struct FakeSecrets;

    impl SecretBackend for FakeSecrets {
        fn acquire(
            &self,
            _request: &SecretRequest,
            names: &[String],
        ) -> Result<BTreeMap<String, SecretValue>, SecretBackendFailure> {
            Ok(names
                .iter()
                .map(|name| (name.clone(), SecretValue::new(format!("fake-{name}-0451"))))
                .collect())
        }

        fn revoke(&self, _lease_id: &str) -> Result<(), SecretBackendFailure> {
            Ok(())
        }
    }

    /// Stands in for `codex login`: writes the credential to `login` in the home.
    struct FakeLogin(Arc<Mutex<Vec<PathBuf>>>);

    impl AccountLogin for FakeLogin {
        fn login(
            &self,
            _pool: &AccountPool,
            _account: &Account,
            home: &Path,
            credential: &str,
        ) -> Result<(), String> {
            self.0.lock().unwrap().push(home.to_path_buf());
            fs::write(home.join("login"), credential).map_err(|error| error.to_string())
        }
    }

    type Logins = Arc<Mutex<Vec<PathBuf>>>;

    /// A Station whose Agent 007 (handler `mock-pooled`, plus `agent` settings) draws from
    /// the pools of `loadouts`, with Operation `G` set by `operation`, its clock at [`START`].
    fn pooled_state(
        root: &Path,
        operation: &str,
        agent: &str,
        loadouts: &str,
    ) -> (DaemonState, Arc<AtomicI64>, Logins) {
        let (mut state, now) = limited_state(
            root,
            operation,
            &format!("[agent.007]\nhandler = \"mock-pooled\"\n{agent}"),
            START,
        );
        let logins = Arc::new(Mutex::new(Vec::new()));
        state.station = state
            .station
            .clone()
            .with_loadouts(loadouts.parse().unwrap())
            .with_secret_backend(FakeSecrets)
            .with_account_login(FakeLogin(logins.clone()))
            .with_account_homes(root.join("homes"));
        for id in ["mission-1", "mission-2"] {
            abandon(&state, id);
        }
        (state, now, logins)
    }

    fn calls(mission: &str) -> Vec<PooledCall> {
        POOLED_CALLS
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.0 == mission)
            .cloned()
            .collect()
    }

    fn token(mission: &str) -> String {
        calls(mission)
            .last()
            .expect("the handler was called")
            .2
            .get("CLAUDE_CODE_OAUTH_TOKEN")
            .cloned()
            .unwrap_or_default()
    }

    fn account(state: &DaemonState, pool: &str, name: &str) -> AccountState {
        state
            .intake
            .accounts()
            .unwrap()
            .into_iter()
            .find(|state| state.account == AccountRef::new(pool, name))
            .unwrap_or_else(|| panic!("account {pool}/{name} is recorded"))
    }

    fn events(state: &DaemonState, action: &str) -> Vec<TimelineEvent> {
        state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == TIMELINE_KIND && event.action == action)
            .collect()
    }

    /// Waits until no account is claimed: a Session's account is freed just after its
    /// Mission is marked done.
    async fn until_released(state: &DaemonState) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while state
                .intake
                .accounts()
                .unwrap()
                .iter()
                .any(|account| !account.active.is_empty())
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the account claim is released");
    }

    fn create_mission(state: &DaemonState, id: &str) {
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(id).unwrap(),
                Some("G".to_owned()),
                format!("work for {id}"),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
    }

    fn abandon(state: &DaemonState, id: &str) {
        state
            .missions
            .lock()
            .unwrap()
            .transition(id, MissionState::Abandoned)
            .unwrap();
    }

    const ENV_POOLS: &str = r#"
[pool.team]
handler = "mock-pooled"

[pool.team.account.alpha]
kind = "claude-oauth-token"
secret = "env-alpha"

[pool.team.account.beta]
kind = "anthropic-api-key"
secret = "env-beta"

[pool.elsewhere]
handler = "mock-pooled"
operations = ["other-*"]

[pool.elsewhere.account.gamma]
kind = "anthropic-auth-token"
secret = "env-gamma"
"#;

    #[tokio::test]
    async fn a_session_gets_exactly_one_account_credential_and_frees_it_when_done() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, _) = pooled_state(
            root.path(),
            "",
            "[agent.007.env]\nANTHROPIC_API_KEY = \"stray\"\nCLAUDE_CONFIG_DIR = \"/stray\"\n",
            ENV_POOLS,
        );
        declare(&state);
        create_mission(&state, "pooled-env");
        start_mission_by_id(&state, "pooled-env").await;
        wait_for_state(&state, "pooled-env", MissionState::Done).await;
        let (_, _, environment, _) = calls("pooled-env").remove(0);
        let carried: Vec<_> = CREDENTIAL_VARIABLES
            .iter()
            .filter(|variable| environment.contains_key(**variable))
            .collect();
        assert_eq!(
            carried,
            [&"CLAUDE_CODE_OAUTH_TOKEN"],
            "one account, and no stray credential"
        );
        assert_eq!(
            environment["CLAUDE_CODE_OAUTH_TOKEN"],
            "fake-env-alpha-0451"
        );
        let hold = session_hold(&state, "pooled-env").unwrap();
        let record = state.intake.session(&hold.id).unwrap().unwrap();
        assert_eq!(record.account, Some(AccountRef::new("team", "alpha")));
        until_released(&state).await;
        let alpha = account(&state, "team", "alpha");
        assert_eq!(alpha.last_used, Some(START as f64));
        assert_eq!(
            alpha.windows.len(),
            1,
            "the provider's quota report is kept"
        );
        assert_eq!(alpha.windows[0].name, "five_hour");
        assert_eq!(alpha.windows[0].used_permille, Some(300));
        assert_eq!(alpha.windows[0].resets_at, Some(POOLED_RESET as f64));
        assert_eq!(
            alpha.tokens_on_day(START),
            120,
            "the Session's tokens count for it"
        );
        assert_eq!(
            alpha.profile.unwrap().operations,
            ["G"],
            "declared with the Operations that may draw from it"
        );
        assert!(
            account(&state, "elsewhere", "gamma")
                .profile
                .unwrap()
                .operations
                .is_empty(),
            "pool elsewhere admits no registered Operation"
        );
        let claimed = events(&state, "claimed");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].detail["account"], "team/alpha");
        assert!(
            !serde_json::to_string(&state.intake.timeline_events("G").unwrap())
                .unwrap()
                .contains("0451"),
            "the timeline carries names, never a credential"
        );
    }

    #[tokio::test]
    async fn the_operation_list_and_the_agent_pin_refuse_the_start_by_name() {
        for (operation, agent, expected) in [
            (
                "pools = [\"elsewhere\"]\n",
                "",
                "pool \"team\" is not in the Operation's `pools`",
            ),
            (
                "",
                "pool = \"elsewhere\"\n",
                "pinned pool \"elsewhere\" admits only Operations matching other-*",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let (state, _now, _) = pooled_state(root.path(), operation, agent, ENV_POOLS);
            create_mission(&state, "pooled-refused");
            let Err(refused) = start_mission(
                axum::extract::State(state.clone()),
                axum::extract::Path("pooled-refused".to_owned()),
                axum::Json(StartMission {
                    working_directory: None,
                    initial_context: Vec::new(),
                    environment: BTreeMap::new(),
                }),
            )
            .await
            else {
                panic!("the start is refused");
            };
            assert!(
                refused.1.0.error.contains(expected),
                "{}",
                refused.1.0.error
            );
            assert!(calls("pooled-refused").is_empty(), "no harness was started");
        }
    }

    #[tokio::test]
    async fn a_limit_resumes_the_same_claude_session_on_another_account_until_the_reset() {
        let root = tempfile::tempdir().unwrap();
        let loadouts = r#"
[pool.rotate]
handler = "mock-pooled"
[pool.rotate.account.a]
kind = "claude-oauth-token"
secret = "rotate-limited-a"
[pool.rotate.account.b]
kind = "claude-oauth-token"
secret = "rotate-b"
"#;
        let (state, now, _) = pooled_state(root.path(), "", "", loadouts);
        create_mission(&state, "pooled-rotate");
        start_mission_by_id(&state, "pooled-rotate").await;
        wait_for_state(&state, "pooled-rotate", MissionState::Parked).await;
        let limited = account(&state, "rotate", "a");
        assert_eq!(limited.limited_until, Some(POOLED_RESET as f64));
        assert!(limited.active.is_empty(), "the limited account is freed");
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get("pooled-rotate")
                .unwrap()
                .parked_until,
            Some(START),
            "parked only until the next scheduler pass"
        );
        assert_eq!(
            events(&state, "limited")[0].detail["next"],
            "resume on another account"
        );
        schedule_parked_once(&state).await;
        wait_for_state(&state, "pooled-rotate", MissionState::Done).await;
        let made = calls("pooled-rotate");
        assert_eq!(
            made.iter().map(|call| call.1).collect::<Vec<_>>(),
            ["start", "resume"],
            "the same provider Session resumes"
        );
        assert_eq!(
            made[0].2["CLAUDE_CODE_OAUTH_TOKEN"],
            "fake-rotate-limited-a-0451"
        );
        assert_eq!(made[1].2["CLAUDE_CODE_OAUTH_TOKEN"], "fake-rotate-b-0451");
        let provider_ids = state
            .missions
            .lock()
            .unwrap()
            .get("pooled-rotate")
            .unwrap()
            .session_ids
            .len();
        assert_eq!(provider_ids, 1, "one provider Session");
        let hold = session_hold(&state, "pooled-rotate").unwrap();
        assert_eq!(
            state.intake.session(&hold.id).unwrap().unwrap().account,
            Some(AccountRef::new("rotate", "b"))
        );
        until_released(&state).await;

        // A is skipped until its reset ...
        now.store(START + 10, Ordering::SeqCst);
        create_mission(&state, "pooled-rotate-3");
        start_mission_by_id(&state, "pooled-rotate-3").await;
        wait_for_state(&state, "pooled-rotate-3", MissionState::Done).await;
        assert_eq!(token("pooled-rotate-3"), "fake-rotate-b-0451");
        until_released(&state).await;

        // ... and taken again after it, being the least recently used.
        now.store(POOLED_RESET + 1, Ordering::SeqCst);
        create_mission(&state, "pooled-rotate-4");
        start_mission_by_id(&state, "pooled-rotate-4").await;
        wait_for_state(&state, "pooled-rotate-4", MissionState::Done).await;
        assert_eq!(token("pooled-rotate-4"), "fake-rotate-limited-a-0451");
    }

    #[tokio::test]
    async fn a_codex_limit_retries_the_mission_on_another_account_with_a_hand_off() {
        let root = tempfile::tempdir().unwrap();
        let loadouts = r#"
[pool.codex-team]
handler = "mock-pooled"
[pool.codex-team.account.a]
kind = "openai-api-key"
secret = "codex-limited-a"
[pool.codex-team.account.b]
kind = "codex-access-token"
secret = "codex-b"
"#;
        let (state, _now, logins) = pooled_state(root.path(), "", "", loadouts);
        create_mission(&state, "pooled-codex");
        start_mission_by_id(&state, "pooled-codex").await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while state
                .missions
                .lock()
                .unwrap()
                .get("pooled-codex")
                .unwrap()
                .state
                != MissionState::Queued
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the limited Mission is queued again");
        let first = session_hold(&state, "pooled-codex").unwrap();
        assert_eq!(
            state.intake.session(&first.id).unwrap().unwrap().status,
            SessionStatus::Ended,
            "the limited attempt is closed"
        );
        schedule_queued_once(&state).await;
        wait_for_state(&state, "pooled-codex", MissionState::Done).await;
        let made = calls("pooled-codex");
        assert_eq!(
            made.iter().map(|call| call.1).collect::<Vec<_>>(),
            ["start", "start"],
            "a new attempt, not a resume"
        );
        let homes: Vec<String> = made
            .iter()
            .map(|call| call.2["CODEX_HOME"].clone())
            .collect();
        assert_ne!(homes[0], homes[1], "each account has its own CODEX_HOME");
        assert!(homes[1].ends_with("codex-team--b"), "{homes:?}");
        assert!(
            made[1]
                .3
                .iter()
                .any(|line| line.contains("continues an earlier attempt")),
            "the new attempt is told it continues one"
        );
        assert_eq!(logins.lock().unwrap().len(), 2);
        assert_eq!(
            fs::read_to_string(Path::new(&homes[1]).join("config.toml")).unwrap(),
            CODEX_CONFIG,
            "the login stays in the home, out of the OS keyring"
        );
        let second = session_hold(&state, "pooled-codex").unwrap();
        assert_eq!(second.attempt, 2);
        assert_eq!(
            state.intake.session(&second.id).unwrap().unwrap().account,
            Some(AccountRef::new("codex-team", "b"))
        );
        assert_eq!(
            events(&state, "limited")[0].detail["next"],
            "new attempt on another account"
        );
        until_released(&state).await;
        // The Mission is over: its homes go at the next check-in.
        tend(&state);
        assert!(!Path::new(&homes[0]).exists() && !Path::new(&homes[1]).exists());
    }

    #[test]
    fn a_brief_whose_accounts_are_all_limited_waits_with_the_reason_until_the_reset() {
        let root = tempfile::tempdir().unwrap();
        let (state, now, _) = pooled_state(root.path(), "", "", ENV_POOLS);
        for name in ["alpha", "beta"] {
            state
                .intake
                .limit_account(&AccountRef::new("team", name), POOLED_RESET as f64)
                .unwrap();
        }
        let queue = root.path().join("registry").join("new");
        fs::create_dir_all(&queue).unwrap();
        fs::write(
            queue.join("manual--brief-9.json"),
            r#"{"title":"Pooled work","id":"brief-9","source":"manual","operation":"G","agent":"007"}"#,
        )
        .unwrap();
        publish_station_check_in(&state);
        schedule_intake_once(&state);
        assert!(
            state.intake.lease("manual--brief-9").unwrap().is_none(),
            "it waits"
        );
        assert_eq!(state.intake.list_new().unwrap().len(), 1);
        let waiting = events(&state, "waiting");
        assert_eq!(waiting.len(), 1);
        assert_eq!(
            waiting[0].detail["reason"],
            format!(
                "no account with headroom in pools team; the first limit resets at \
                 {POOLED_RESET} (Unix seconds)"
            )
        );
        schedule_intake_once(&state);
        assert_eq!(
            events(&state, "waiting").len(),
            1,
            "the reason is recorded once"
        );

        now.store(POOLED_RESET, Ordering::SeqCst);
        schedule_intake_once(&state);
        assert_eq!(
            state
                .intake
                .lease("manual--brief-9")
                .unwrap()
                .unwrap()
                .station(),
            "test",
            "placed once a limit has reset"
        );
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get("manual--brief-9")
                .unwrap()
                .state,
            MissionState::Queued
        );
    }
}
