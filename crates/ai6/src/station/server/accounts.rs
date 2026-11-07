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
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::directorates::m::mission::Mission;
use crate::directorates::m::placement;
use crate::directorates::m::roster::{Agent, AgentRoute};
use crate::directorates::q::{
    Account, AccountKind, AccountLogin, AccountPool, CREDENTIAL_VARIABLES, CommandLogin, Loadout,
    LoadoutCatalog, PoolChoice, SecretBroker, SecretLease, SecretLeaseRequest, choose_pools,
};
use crate::directorates::registry::accounts::{
    AccountClaimRequest, AccountProfile, AccountRef, AccountState, QuotaWindow, choose_account,
};
use crate::directorates::registry::sessions::SessionRecord;
use crate::directorates::registry::timeline::TimelineEvent;
use coqui_acp_provider::QuotaReport;

use super::DaemonState;

/// What holds an account: a Mission, or an assistant conversation under the Session name it
/// starts its harness with, which belongs to no Operation.
#[derive(Debug, Clone, Copy)]
pub(super) struct Holder<'a> {
    /// The Mission ID the claim is recorded under.
    pub(super) id: &'a str,
    /// Its Operation, when it has one.
    pub(super) operation: Option<&'a str>,
}

impl<'a> From<&'a Mission> for Holder<'a> {
    fn from(mission: &'a Mission) -> Self {
        Self {
            id: mission.mission_id.as_str(),
            operation: mission.operation_id.as_deref(),
        }
    }
}

/// A claim on an account for `state`'s Session of `mission_id`, held for `[accounts]
/// claim_ttl` unless the Station renews it at its check-in, so a dark Station's accounts free
/// up with its Session leases.
fn claim_request(state: &DaemonState, pool: &AccountPool, mission_id: &str) -> AccountClaimRequest {
    let limits = &state.station.limits().accounts;
    AccountClaimRequest::new(
        pool,
        state.station_id.as_str(),
        mission_id,
        limits.claim_ttl.0,
    )
    .unknown_use(limits.unknown_use_permille)
}
const TIMELINE_KIND: &str = "account";
const CODEX_CONFIG: &str = "# Written by AI6: this home holds one pool account's login.\n\
                            cli_auth_credentials_store = \"file\"\n";
/// Marks a Mission's directory of Codex homes as made by this Station, so cleanup removes
/// nothing else that sits in the homes directory.
const HOME_MARKER: &str = ".ai6-account-homes";

/// The Station's account logins and the account each of its Missions holds.
#[derive(Clone, Default)]
pub(crate) struct AccountDesk {
    login: Option<Arc<dyn AccountLogin>>,
    homes: Option<PathBuf>,
    held: Arc<Mutex<BTreeMap<String, Held>>>,
    /// Missions whose account is being claimed and handed over: their claims are renewed
    /// like held ones.
    pending: Arc<Mutex<BTreeSet<String>>>,
    waiting: Arc<Mutex<BTreeMap<String, String>>>,
    /// Logins kept sealed in the Registry.
    pub(super) logins: super::logins::LoginDesk,
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AccountDesk {
    pub(crate) fn with_login(mut self, login: Arc<dyn AccountLogin>) -> Self {
        self.login = Some(login);
        self
    }

    pub(crate) fn with_homes(mut self, dir: PathBuf) -> Self {
        self.homes = Some(dir);
        self
    }

    /// Makes homes under `dir` unless a directory was already set.
    pub(crate) fn with_default_homes(mut self, dir: PathBuf) -> Self {
        self.homes.get_or_insert(dir);
        self
    }

    /// The login set in code, else the pool's login command, killed after `timeout`.
    fn login(&self, timeout: Duration) -> Arc<dyn AccountLogin> {
        self.login
            .clone()
            .unwrap_or_else(|| Arc::new(CommandLogin::default().with_timeout(timeout)))
    }

    pub(super) fn homes(&self) -> Result<PathBuf, String> {
        match &self.homes {
            Some(dir) => Ok(dir.clone()),
            None => crate::bootstrap::platform_state_dir()
                .map(|dir| dir.join("accounts"))
                .map_err(|error| format!("no state directory for account homes: {error}")),
        }
    }

    /// The Mission's home for `account`, private to the Station's user and marked as this
    /// Station's, so its cleanup removes it with the Mission and nothing else.
    pub(super) fn private_home(
        &self,
        mission_id: &str,
        pool: &AccountPool,
        account: &Account,
    ) -> Result<PathBuf, String> {
        let root = self.homes()?;
        let mission = root.join(path_component(mission_id));
        let home = mission.join(format!("{}--{}", pool.id(), account.name()));
        (|| {
            fs::create_dir_all(&home)?;
            fs::write(mission.join(HOME_MARKER), b"")?;
            for dir in [&root, &mission, &home] {
                private(dir)?;
            }
            Ok::<(), std::io::Error>(())
        })()
        .map_err(|error| format!("{}: {error}", home.display()))?;
        Ok(home)
    }
}

/// The longest a provider limit keeps an account out, `[accounts] limit_horizon`: a reset
/// reported years ahead is not trusted.
fn horizon_secs(state: &DaemonState) -> i64 {
    i64::try_from(state.station.limits().accounts.limit_horizon.0.as_secs()).unwrap_or(i64::MAX)
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

/// Where a route's Session gets its account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Source {
    /// No pool serves it.
    Unpooled,
    /// These pools, by ID, in the order to try them.
    Pools(Vec<String>),
    /// Pools serve it but none may be used here, for this reason.
    Refused(String),
}

/// Where `agent`, on the route it works on, draws its account for `operation`. A pool named where
/// the Station has no loadouts is refused, since nothing is there to read its accounts from.
pub(super) fn source_of(state: &DaemonState, operation: Option<&str>, agent: &Agent) -> Source {
    let Some(catalog) = state.station.credentials.catalog() else {
        return match &agent.pool {
            Some(pool) => Source::Refused(format!("pool {pool:?} is not configured")),
            None => Source::Unpooled,
        };
    };
    match pools_for(state, catalog, operation, agent) {
        PoolChoice::Unpooled => Source::Unpooled,
        PoolChoice::Refused(reason) => Source::Refused(reason),
        PoolChoice::Pools(pools) => {
            Source::Pools(pools.iter().map(|pool| pool.id().to_owned()).collect())
        }
    }
}

/// The best account `pools` have room on at `now`, each pool by its own strategy, the least used
/// across them; `None` when none has room.
pub(super) fn best_account(
    pools: &[&AccountPool],
    states: &[AccountState],
    now: f64,
    unknown_use: u16,
    station: &str,
) -> Option<AccountState> {
    pools
        .iter()
        .filter_map(|pool| {
            let request = AccountClaimRequest::new(pool, "", "", Duration::ZERO);
            let by_name = pool_states(pool, states);
            let chosen = choose_account(
                pool.balancing(),
                request.candidates(),
                &by_name,
                None,
                now,
                unknown_use,
                station,
            )?;
            Some(
                by_name.get(&chosen.account).cloned().unwrap_or_else(|| {
                    AccountState::new(AccountRef::new(pool.id(), &chosen.account))
                }),
            )
        })
        .min_by_key(|state| state.highest_use(now, unknown_use))
}

/// Why `pools` have no account with room, as a person reads it.
pub(super) fn no_room(pools: &[&AccountPool], states: &[AccountState], now: f64) -> String {
    let ids: Vec<String> = pools.iter().map(|pool| pool.id().to_owned()).collect();
    placement::no_account_headroom("", &ids, frees_at(pools, states, now)).reason
}

/// Whether a provider Session can move to an account of `route`'s pools.
pub(super) fn route_carries_session(
    state: &DaemonState,
    operation: Option<&str>,
    agent: &Agent,
    route: &AgentRoute,
) -> bool {
    let Some(catalog) = state.station.credentials.catalog() else {
        return false;
    };
    let PoolChoice::Pools(pools) = pools_for(state, catalog, operation, &agent.on_route(route))
    else {
        return false;
    };
    let states = state.intake.accounts().unwrap_or_default();
    let now = state.clock.now() as f64;
    let unknown_use = state.station.limits().accounts.unknown_use_permille;
    has_room(
        &pools,
        &states,
        (now, unknown_use),
        &state.station_id,
        |account| account.kind().carries_session(),
    )
}

/// Whether the account `earlier` names is of a kind a provider Session can move away from.
pub(super) fn carries_session(state: &DaemonState, earlier: Option<&AccountRef>) -> bool {
    let Some(earlier) = earlier else {
        return false;
    };
    state
        .station
        .credentials
        .catalog()
        .and_then(|catalog| catalog.pool(&earlier.pool))
        .and_then(|pool| pool.get(&earlier.account))
        .is_some_and(|account| account.kind().carries_session())
}

/// What `agent`, on the route it works on, may draw from: its handler's pools that admit
/// `operation`, as narrowed by the Operation's `pools` and the Agent's or route's pin.
fn pools_for<'a>(
    state: &DaemonState,
    catalog: &'a LoadoutCatalog,
    operation: Option<&str>,
    agent: &Agent,
) -> PoolChoice<'a> {
    let allowed = operation
        .and_then(|id| state.roster.operation(id).ok())
        .and_then(|operation| operation.pools.clone());
    let ancestors = lineage(state, operation);
    choose_pools(
        catalog.pools(),
        agent.handler.as_deref(),
        operation,
        &ancestors,
        allowed.as_deref(),
        agent.pool.as_deref(),
    )
}

/// The variables that carry a provider credential, for an Agent that draws from a pool:
/// [`CREDENTIAL_VARIABLES`] and every account's own. `None` for an Agent no pool serves.
pub(super) fn credential_variables(
    state: &DaemonState,
    catalog: &LoadoutCatalog,
    operation: Option<&str>,
    agent: &Agent,
) -> Option<BTreeSet<String>> {
    if let PoolChoice::Unpooled = pools_for(state, catalog, operation, agent) {
        return None;
    }
    let mut variables: BTreeSet<String> = CREDENTIAL_VARIABLES
        .iter()
        .map(|variable| (*variable).to_owned())
        .collect();
    for pool in catalog.pools() {
        for account in pool.accounts() {
            variables.extend(account.variable(pool.handler()));
        }
    }
    Some(variables)
}

/// Claims the account of a route with no pool for `holder`'s Session: it hands over nothing, and
/// the claim fails when the route's limit stands, so a limit one Station saw holds on every one.
pub(super) fn claim_implicit(
    state: &DaemonState,
    pool: &AccountPool,
    holder: Holder<'_>,
) -> Result<(), String> {
    let now = state.clock.now() as f64;
    let desk = &state.station.credentials.accounts;
    lock(&desk.pending).insert(holder.id.to_owned());
    let request =
        claim_request(state, pool, holder.id).operation(holder.operation.map(str::to_owned));
    let claimed = state.intake.claim_account(&request, now);
    lock(&desk.pending).remove(holder.id);
    let chosen = claimed
        .map_err(|error| format!("route's account could not be claimed: {error}"))?
        .ok_or("the route's limit has not reset")?;
    lock(&desk.held).insert(
        holder.id.to_owned(),
        Held {
            account: chosen.account,
            kind: AccountKind::HarnessLogin,
        },
    );
    Ok(())
}

/// Claims an account for `mission`'s Session and puts its credential in `environment`,
/// after removing every other credential variable, so the harness sees exactly one account.
/// Returns the lease to revoke with the Session's, if the credential is a secret.
///
/// An Agent no pool serves gets nothing here. A pool that may not be used, or no account
/// with room, fails the start with the reason. A Session that continues a provider Session
/// keeps its account when that Session cannot move (Codex, `config-dir`), and moves only to
/// an account of a kind that carries it.
#[allow(clippy::too_many_arguments)]
pub(super) fn issue(
    state: &DaemonState,
    catalog: &LoadoutCatalog,
    broker: &SecretBroker,
    holder: Holder<'_>,
    agent: &Agent,
    site: Site,
    environment: &mut BTreeMap<String, String>,
) -> Result<Option<SecretLease>, String> {
    let agent_id = agent.id.as_str();
    let pools = match pools_for(state, catalog, holder.operation, agent) {
        PoolChoice::Unpooled => return Ok(None),
        PoolChoice::Refused(reason) => return Err(format!("Agent {agent_id:?}: {reason}")),
        PoolChoice::Pools(pools) => pools,
    };
    for variable in
        credential_variables(state, catalog, holder.operation, agent).unwrap_or_default()
    {
        environment.remove(&variable);
    }
    let mission_id = holder.id;
    let desk = &state.station.credentials.accounts;
    // A check-in during a slow hand-over (a Codex login) renews this claim too.
    lock(&desk.pending).insert(mission_id.to_owned());
    let issued = claim_and_hand_over(
        state,
        broker,
        catalog,
        &pools,
        holder,
        agent_id,
        site,
        environment,
    );
    lock(&desk.pending).remove(mission_id);
    if issued.is_err()
        && let Err(release) = state.intake.release_account(&state.station_id, mission_id)
    {
        eprintln!("ai6: account claim of Mission {mission_id:?} not released: {release}");
    }
    issued
}

#[allow(clippy::too_many_arguments)]
fn claim_and_hand_over(
    state: &DaemonState,
    broker: &SecretBroker,
    catalog: &LoadoutCatalog,
    pools: &[&AccountPool],
    holder: Holder<'_>,
    agent_id: &str,
    site: Site,
    environment: &mut BTreeMap<String, String>,
) -> Result<Option<SecretLease>, String> {
    let mission_id = holder.id;
    let now = state.clock.now() as f64;
    let earlier = state
        .intake
        .latest_session(mission_id)
        .ok()
        .flatten()
        .and_then(|record| record.account);
    let (pool, chosen) = claim(state, catalog, pools, holder, earlier.as_ref(), site, now)?;
    let account = pool
        .get(&chosen.account.account)
        .ok_or("the claimed account is not configured")?;
    let lease = hand_over(state, broker, pool, account, mission_id, site, environment)?;
    lock(&state.station.credentials.accounts.held).insert(
        mission_id.to_owned(),
        Held {
            account: chosen.account.clone(),
            kind: account.kind(),
        },
    );
    if let Some(operation) = holder.operation {
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

/// The kind of the account `earlier` names, when it is still configured, whichever route it
/// was claimed for.
fn kind_of(catalog: &LoadoutCatalog, earlier: &AccountRef) -> Option<AccountKind> {
    catalog
        .pool(&earlier.pool)
        .and_then(|pool| pool.get(&earlier.account))
        .map(Account::kind)
}

/// Claims the first account with room, pool by pool, preferring the account the Mission's
/// last Session used. A resumed provider Session keeps an account of a kind that cannot carry
/// it, and otherwise moves only to accounts of a kind that can.
fn claim<'a>(
    state: &DaemonState,
    catalog: &LoadoutCatalog,
    pools: &[&'a AccountPool],
    holder: Holder<'_>,
    earlier: Option<&AccountRef>,
    site: Site,
    now: f64,
) -> Result<(&'a AccountPool, AccountState), String> {
    let request = |pool: &AccountPool| {
        claim_request(state, pool, holder.id).operation(holder.operation.map(str::to_owned))
    };
    let failed = |pool: &AccountPool, error: std::io::Error| {
        format!("pool {:?} could not be claimed from: {error}", pool.id())
    };
    let earlier_kind = earlier.and_then(|earlier| kind_of(catalog, earlier));
    if site.resuming
        && let (Some(earlier), Some(kind)) = (earlier, earlier_kind)
        && !kind.carries_session()
        && let Some(pool) = pools.iter().find(|pool| pool.id() == earlier.pool)
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
    let carried = site.resuming && earlier_kind.is_some_and(AccountKind::carries_session);
    for pool in pools {
        let mut wanted = request(pool);
        if carried {
            for account in pool.accounts() {
                if !account.kind().carries_session() {
                    wanted = wanted.excluding(account.name());
                }
            }
        }
        if wanted.candidates().is_empty() {
            continue;
        }
        if let Some(earlier) = earlier.filter(|earlier| earlier.pool == pool.id()) {
            wanted = wanted.prefer(&earlier.account);
        }
        match state.intake.claim_account(&wanted, now) {
            Ok(Some(chosen)) => return Ok((pool, chosen)),
            Ok(None) => continue,
            Err(error) => return Err(failed(pool, error)),
        }
    }
    let states = state.intake.accounts().unwrap_or_default();
    Err(no_room(pools, &states, now))
}

/// Puts `account`'s credential where the harness reads it.
pub(super) fn hand_over(
    state: &DaemonState,
    broker: &SecretBroker,
    pool: &AccountPool,
    account: &Account,
    mission_id: &str,
    site: Site,
    environment: &mut BTreeMap<String, String>,
) -> Result<Option<SecretLease>, String> {
    if account.kind() == AccountKind::HarnessLogin {
        if account.keeps_login_files() {
            super::logins::hand_over(state, pool, account, mission_id, site, environment)?;
        }
        return Ok(None);
    }
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
    let request = SecretLeaseRequest::with_ttl(
        state.station_id.clone(),
        mission_id,
        state.station.limits().secrets.lease_ttl.0,
    )
    .map_err(|error| error.to_string())?;
    let lease = broker
        .lease(&loadout, request)
        .map_err(|error| format!("account {name}'s credential could not be leased: {error}"))?;
    let handed = if account.kind().login_flag().is_some() {
        home_for(state, mission_id, pool, account).and_then(|home| {
            let login = state
                .station
                .credentials
                .accounts
                .login(state.station.limits().accounts.login_timeout.0);
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
    let home = state
        .station
        .credentials
        .accounts
        .private_home(mission_id, pool, account)
        .map_err(|error| format!("CODEX_HOME could not be made: {error}"))?;
    fs::write(home.join("config.toml"), CODEX_CONFIG)
        .map_err(|error| format!("CODEX_HOME {} could not be made: {error}", home.display()))?;
    Ok(home)
}

#[cfg(unix)]
fn private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// Windows keeps the inherited access list of the state directory; [`declare`] logs that as a
/// weakening.
#[cfg(not(unix))]
fn private(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// A Mission ID as one path component.
pub(super) fn path_component(id: &str) -> String {
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
    if lock(&state.station.credentials.accounts.held)
        .remove(mission_id)
        .is_some()
        && let Err(error) = state.intake.release_account(&state.station_id, mission_id)
    {
        eprintln!("ai6: account claim of Mission {mission_id:?} not released: {error}");
    }
}

/// The account `mission_id`'s Session holds on this Station.
pub(super) fn held(state: &DaemonState, mission_id: &str) -> Option<AccountRef> {
    lock(&state.station.credentials.accounts.held)
        .get(mission_id)
        .map(|held| held.account.clone())
}

/// Takes the account a re-attached Session (after a restart or an adoption) already runs on,
/// as its Session record names it: the harness keeps the credential it started with, so the
/// claim is recorded even past the account's `max_concurrent` or a limit, and said so on the
/// timeline.
pub(super) fn reattach(state: &DaemonState, mission: &Mission, session: &SessionRecord) {
    let Some(account) = &session.account else {
        return;
    };
    let mission_id = mission.mission_id.as_str();
    let configured = state.station.credentials.catalog().and_then(|catalog| {
        let pool = catalog.pool(&account.pool)?;
        Some((pool.clone(), pool.get(&account.account)?.kind()))
    });
    let own_account = || {
        let handler = session.launch.as_ref()?.handler.as_str();
        super::routes::is_implicit(&account.pool).then(|| {
            (
                super::routes::implicit_pool_named(&account.pool, handler),
                AccountKind::HarnessLogin,
            )
        })
    };
    let Some((pool, kind)) = configured.or_else(own_account) else {
        eprintln!(
            "ai6: Mission {mission_id:?} runs on account {account}, which this Station does not \
             configure; its use is not counted"
        );
        return;
    };
    let request = claim_request(state, &pool, mission_id)
        .operation(mission.operation_id.clone())
        .only(&account.account)
        .in_use();
    let now = state.clock.now() as f64;
    match state.intake.claim_account(&request, now) {
        Ok(Some(_)) => {}
        Ok(None) => {
            eprintln!("ai6: account {account} of Mission {mission_id:?} could not be claimed");
            return;
        }
        Err(error) => {
            eprintln!("ai6: account {account} of Mission {mission_id:?} not claimed: {error}");
            return;
        }
    }
    lock(&state.station.credentials.accounts.held).insert(
        mission_id.to_owned(),
        Held {
            account: account.clone(),
            kind,
        },
    );
    if let Some(operation) = mission.operation_id.as_deref() {
        record(
            state,
            TimelineEvent::new(TIMELINE_KIND, "reclaimed", operation, now)
                .mission(mission_id)
                .detail("account", account.to_string())
                .detail(
                    "note",
                    "a re-attached Session keeps its account, even past max_concurrent or a limit",
                ),
        );
    }
}

/// At each check-in: renews this Station's claims for the Missions it still runs (dropping
/// any other made before this pass), and removes the private homes of Missions that are over,
/// after taking back what a harness renewed in one.
pub(super) fn tend(state: &DaemonState) {
    let desk = &state.station.credentials.accounts;
    let has_pools = !lock(&desk.held).is_empty()
        || state
            .station
            .credentials
            .catalog()
            .is_some_and(|catalog| catalog.pools().next().is_some());
    // Pending first: an issue moves its Mission to held before it leaves pending.
    let mut missions: BTreeSet<String> = lock(&desk.pending).clone();
    missions.extend(lock(&desk.held).keys().cloned());
    let missions: Vec<String> = missions.into_iter().collect();
    let now = state.clock.now() as f64;
    if has_pools
        && let Err(error) = state.intake.renew_account_claims(
            &state.station_id,
            &missions,
            state.station.limits().accounts.claim_ttl.0,
            now,
        )
    {
        eprintln!("ai6: account claims not renewed: {error}");
    }
    super::logins::tend(state);
    sweep_homes(state, &missions);
}

/// Removes the Codex homes of Missions that are over, also once their pool is gone. Only the
/// directories this Station made are touched, and nothing while the Mission book is unreadable.
fn sweep_homes(state: &DaemonState, claimed: &[String]) {
    let Ok(root) = state.station.credentials.accounts.homes() else {
        return;
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    let Ok(book) = state.missions.lock() else {
        return;
    };
    let live: BTreeSet<String> = book
        .list()
        .into_iter()
        .filter(|mission| !super::is_terminal_state(mission.state))
        .map(|mission| path_component(mission.mission_id.as_str()))
        .collect();
    drop(book);
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if live.contains(&name)
            || claimed
                .iter()
                .any(|mission| path_component(mission) == name)
            || !entry.path().join(HOME_MARKER).is_file()
        {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(entry.path()) {
            eprintln!("ai6: account home {name:?} not removed: {error}");
        }
    }
}

/// The Operations `operation` is subordinate to, nearest first.
fn lineage(state: &DaemonState, operation: Option<&str>) -> Vec<String> {
    operation
        .map(|operation| {
            state
                .roster
                .lineage(operation)
                .into_iter()
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Records this Station's view of each configured account in the Registry, with the
/// Operations of its roster that may draw from the pool, so the board can show it before any
/// Session claims it, and withdraws its profile of every account its configuration no longer
/// declares. Run when the Station starts and whenever its roster or loadouts change.
pub(super) fn declare(state: &DaemonState) {
    let mut declared = std::collections::BTreeSet::new();
    let pools = state.station.credentials.catalog().into_iter();
    for pool in pools.flat_map(|catalog| catalog.pools()) {
        let operations: Vec<String> = state
            .roster
            .operations()
            .filter(|operation| {
                pool.admits(Some(&operation.id), &lineage(state, Some(&operation.id)))
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
            let reference = AccountRef::new(pool.id(), account.name());
            let declaring = state
                .intake
                .declare_account(&reference, &state.station_id, &profile);
            declared.insert(reference);
            if let Err(error) = declaring {
                eprintln!(
                    "ai6: account {}/{} not declared: {error}",
                    pool.id(),
                    account.name()
                );
            }
            if let Some(weakening) = account.weakening(pool.id()) {
                eprintln!("ai6: {weakening}");
            }
            if cfg!(windows) && account.kind().login_flag().is_some() {
                eprintln!(
                    "ai6: account {}/{}: on Windows its CODEX_HOME keeps the access list it \
                     inherits from the Station's state directory",
                    pool.id(),
                    account.name()
                );
            }
            if cfg!(windows) && account.keeps_login_files() {
                eprintln!(
                    "ai6: account {}/{}: on Windows a Session's home keeps the access list it \
                     inherits from the Station's state directory",
                    pool.id(),
                    account.name()
                );
            }
        }
    }
    for (pool, operations) in super::routes::implicit_accounts(state).into_values() {
        for account in pool.accounts() {
            let Some(profile) = AccountProfile::of(&pool, account.name(), operations.clone())
            else {
                continue;
            };
            let reference = AccountRef::new(pool.id(), account.name());
            if let Err(error) =
                state
                    .intake
                    .declare_account(&reference, &state.station_id, &profile)
            {
                eprintln!("ai6: route account {reference} not declared: {error}");
            }
            declared.insert(reference);
        }
    }
    let stale = match state.intake.accounts() {
        Ok(accounts) => accounts
            .into_iter()
            .filter(|account| {
                account.profiles.contains_key(&state.station_id)
                    && !declared.contains(&account.account)
            })
            .map(|account| account.account)
            .collect::<Vec<_>>(),
        Err(error) => {
            eprintln!("ai6: accounts this Station no longer declares were not withdrawn: {error}");
            Vec::new()
        }
    };
    for account in stale {
        if let Err(error) = state.intake.withdraw_account(&account, &state.station_id) {
            eprintln!("ai6: account {account} not withdrawn: {error}");
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
    /// Another account of a kind that carries the provider Session has room: resume it there.
    Resume,
    /// Another account has room but the provider Session cannot move to it: start a new
    /// attempt.
    Retry,
    /// No account has room: wait for the limit to reset.
    Wait,
}

/// Marks the account `holder`'s Session holds limited until `resets_at` (or `fallback_secs` from
/// now, and never past the Station's limit horizon) and frees it. The account and its kind, and
/// until when; `None` when it holds none.
pub(super) fn mark_limited(
    state: &DaemonState,
    holder: Holder<'_>,
    resets_at: Option<i64>,
    fallback_secs: u64,
) -> Option<(AccountRef, bool, i64)> {
    let mission_id = holder.id;
    let desk = &state.station.credentials.accounts;
    let held = lock(&desk.held).remove(mission_id)?;
    let now = state.clock.now();
    let until = resets_at
        .filter(|reset| *reset > now)
        .unwrap_or_else(|| now.saturating_add(i64::try_from(fallback_secs).unwrap_or(i64::MAX)))
        .min(now.saturating_add(horizon_secs(state)));
    if let Err(error) = state.intake.limit_account(&held.account, until as f64) {
        eprintln!("ai6: account {} not marked limited: {error}", held.account);
    }
    if let Err(error) = state.intake.release_account(&state.station_id, mission_id) {
        eprintln!("ai6: account claim of Mission {mission_id:?} not released: {error}");
    }
    Some((held.account, held.kind.carries_session(), until))
}

/// Marks the account `mission_id`'s Session holds limited ([`mark_limited`]) and says whether
/// another account of the Mission's pools, or another of its Agent's routes, has room.
pub(super) fn on_limit(
    state: &DaemonState,
    mission: &Mission,
    resets_at: Option<i64>,
    fallback_secs: u64,
) -> Rotation {
    let mission_id = mission.mission_id.as_str();
    let Some((account, carried, until)) =
        mark_limited(state, mission.into(), resets_at, fallback_secs)
    else {
        return Rotation::Unpooled;
    };
    let now = state.clock.now();
    let operation = mission.operation_id.as_deref();
    let agent = state
        .roster
        .agent(operation, mission.agent_id.as_deref().unwrap_or_default())
        .ok();
    let (any_room, carrying_room) = match agent.as_ref().filter(|agent| agent.is_routed()) {
        Some(agent) => {
            let handler = state
                .missions
                .lock()
                .ok()
                .and_then(|missions| missions.operation(mission_id))
                .map(|manifest| manifest.handler_provider)
                .unwrap_or_default();
            super::routes::room_after_limit(state, operation, agent, &handler)
        }
        None => state
            .station
            .credentials
            .catalog()
            .zip(agent.as_ref())
            .map_or((false, false), |(catalog, agent)| {
                let PoolChoice::Pools(pools) = pools_for(state, catalog, operation, agent) else {
                    return (false, false);
                };
                let states = state.intake.accounts().unwrap_or_default();
                let now = now as f64;
                let unknown_use = state.station.limits().accounts.unknown_use_permille;
                let station = state.station_id.as_str();
                (
                    has_room(&pools, &states, (now, unknown_use), station, |_| true),
                    has_room(&pools, &states, (now, unknown_use), station, |account| {
                        account.kind().carries_session()
                    }),
                )
            }),
    };
    let rotation = match (carried, carrying_room, any_room) {
        (true, true, _) => Rotation::Resume,
        (_, _, true) => Rotation::Retry,
        _ => Rotation::Wait,
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
                .detail("account", account.to_string())
                .detail("until", until.to_string())
                .detail("next", next),
        );
    }
    rotation
}

/// Whether any of `pools`' accounts that `keep` admits has room at `now`, an account with no
/// reported use counting as `unknown_use` thousandths used.
fn has_room(
    pools: &[&AccountPool],
    states: &[AccountState],
    (now, unknown_use): (f64, u16),
    station: &str,
    keep: impl Fn(&Account) -> bool,
) -> bool {
    pools.iter().any(|pool| {
        // Only its candidates are read: it claims nothing.
        let mut request = AccountClaimRequest::new(pool, "", "", Duration::ZERO);
        for account in pool.accounts() {
            if !keep(account) {
                request = request.excluding(account.name());
            }
        }
        choose_account(
            pool.balancing(),
            request.candidates(),
            &pool_states(pool, states),
            None,
            now,
            unknown_use,
            station,
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
    let agent = state.roster.agent(Some(operation_id), agent_id).ok()?;
    if agent.is_routed() {
        return super::routes::hold(state, Some(operation_id), &agent);
    }
    let catalog = state.station.credentials.catalog()?;
    let PoolChoice::Pools(pools) = pools_for(state, catalog, Some(operation_id), &agent) else {
        return None;
    };
    let states = state.intake.accounts().ok()?;
    let now = state.clock.now() as f64;
    let unknown_use = state.station.limits().accounts.unknown_use_permille;
    if has_room(
        &pools,
        &states,
        (now, unknown_use),
        &state.station_id,
        |_| true,
    ) {
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
    match reason {
        Some(reason) => note(state, key, operation_id, reason, reason),
        None => {
            lock(&state.station.credentials.accounts.waiting).remove(key);
        }
    }
}

/// Records, as [`note_waiting`] does, why placement found nothing for the brief `key`. Reasons
/// that differ only in their figures (seconds silent, Missions held) are one reason, so a
/// brief is noted again only when what rules each place out changes.
pub(super) fn note_unplaced(state: &DaemonState, key: &str, operation_id: &str, reason: &str) {
    let figures_aside: String = reason
        .chars()
        .map(|character| {
            if character.is_ascii_digit() {
                '#'
            } else {
                character
            }
        })
        .collect();
    note(state, key, operation_id, reason, &figures_aside);
}

/// Logs and records `reason` once for `key`, until `seen` (what tells one reason from another)
/// changes. The timeline keeps as much of it as `[registry.limits.timeline] text_bytes` allows.
fn note(state: &DaemonState, key: &str, operation_id: &str, reason: &str, seen: &str) {
    let mut waiting = lock(&state.station.credentials.accounts.waiting);
    if waiting.get(key).map(String::as_str) == Some(seen) {
        return;
    }
    waiting.insert(key.to_owned(), seen.to_owned());
    drop(waiting);
    eprintln!("ai6: brief {key:?} waits: {reason}");
    let kept = cut(reason, state.intake.limits().timeline.text_bytes.or_max());
    record(
        state,
        TimelineEvent::new(
            TIMELINE_KIND,
            "waiting",
            operation_id,
            state.clock.now() as f64,
        )
        .detail("brief", key)
        .detail("reason", kept),
    );
}

/// `text` within `bytes`, cut at a character and marked with an ellipsis when it was longer.
fn cut(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let room = bytes.saturating_sub('…'.len_utf8());
    let end = (0..=room)
        .rev()
        .find(|end| text.is_char_boundary(*end))
        .unwrap_or_default();
    format!("{}…", &text[..end])
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
    use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart};
    use crate::guatiao::library::{Object, ProviderError};
    use crate::guatiao::{Provider, Status};
    use coqui_acp_provider::{
        AgentEvent, AgentSession, LIMIT_EVENT, QuotaReport, TokenUsage, UsageLimit,
    };

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

    /// The environment of each look at the models, in order.
    pub(in crate::station::server) static LISTED: Mutex<Vec<BTreeMap<String, String>>> =
        Mutex::new(Vec::new());

    impl AgentHandler for MockPooledHandler {
        /// Lists one model, and keeps the environment it was asked with.
        fn models(
            &self,
            environment: BTreeMap<String, String>,
        ) -> Result<crate::directorates::section_00::agent::ModelListing, ProviderError> {
            let had_credential = environment.contains_key("CLAUDE_CODE_OAUTH_TOKEN")
                || environment.contains_key("ANTHROPIC_API_KEY")
                || environment.contains_key("ANTHROPIC_AUTH_TOKEN");
            LISTED.lock().unwrap().push(environment);
            Ok(crate::directorates::section_00::agent::ModelListing::of(
                vec![crate::directorates::section_00::agent::ModelInfo {
                    id: "mock-model".to_owned(),
                    name: None,
                    description: Some(
                        if had_credential {
                            "listed with a credential"
                        } else {
                            "listed with none"
                        }
                        .to_owned(),
                    ),
                }],
            ))
        }

        /// Stands for a handler that cannot hold a `desk` Session, as `acp` cannot.
        fn clearance_refusal(&self, clearance: String) -> Result<Option<String>, ProviderError> {
            Ok((clearance == "desk").then(|| "it cannot keep commands from one".to_owned()))
        }

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

    use super::testing::{LISTED, POOLED_CALLS, POOLED_RESET, PooledCall};
    use super::*;
    use crate::directorates::m::identity::MissionId;
    use crate::directorates::m::mission::MissionState;
    use crate::directorates::registry::sessions::SessionStatus;
    use crate::directorates::registry::timeline::TimelineEvent;

    use super::super::routes;
    use super::super::tests::{limited_state, start_mission_by_id, wait_for_done, wait_for_state};
    use super::super::{
        StartMission, publish_station_check_in, schedule_intake_once, schedule_parked_once,
        schedule_queued_once, session_hold, start_mission,
    };

    const START: i64 = POOLED_RESET - 3_600;

    /// The homes a fake `codex login` logged in to, and what it does while it logs in.
    #[derive(Default)]
    struct LoginLog {
        homes: Vec<PathBuf>,
        during: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    type Logins = Arc<Mutex<LoginLog>>;

    /// Stands in for `codex login`: writes the credential to `login` in the home.
    struct FakeLogin(Logins);

    impl AccountLogin for FakeLogin {
        fn login(
            &self,
            _pool: &AccountPool,
            _account: &Account,
            home: &Path,
            credential: &str,
        ) -> Result<(), String> {
            let during = {
                let mut log = self.0.lock().unwrap();
                log.homes.push(home.to_path_buf());
                log.during.clone()
            };
            if let Some(during) = during {
                during();
            }
            fs::write(home.join("login"), credential).map_err(|error| error.to_string())
        }
    }

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
        let logins = Logins::default();
        state.station = state
            .station
            .clone()
            .with_loadouts(
                loadouts
                    .parse::<crate::directorates::q::LoadoutCatalog>()
                    .unwrap()
                    .with_default_secret_provider("fake-secrets"),
            )
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

    /// Two routes of Agent 007 to one model under two names, each with a pool of its own.
    const ROUTE_POOLS: &str = r#"
[pool.one]
handler = "mock-pooled"
named_only = true
[pool.one.account.a]
kind = "claude-oauth-token"
secret = "one-a"

[pool.two]
handler = "mock-pooled"
named_only = true
[pool.two.account.b]
kind = "claude-oauth-token"
secret = "two-b"
"#;

    fn two_routes(balance: &str) -> String {
        format!(
            r#"[agent.007]
balance = "{balance}"
[[agent.007.route]]
handler = "mock-pooled"
model = "name-one"
pool = "one"
priority = 1
[[agent.007.route]]
handler = "mock-pooled"
model = "name-two"
pool = "two"
priority = 2
"#
        )
    }

    /// A Station with the roster's agents file `agents` and `loadouts`, Operation `G` set by
    /// `operation`, its clock at [`START`].
    fn routed_state(
        root: &Path,
        operation: &str,
        agents: &str,
        loadouts: &str,
    ) -> (DaemonState, Arc<AtomicI64>) {
        let (mut state, now) = limited_state(root, operation, agents, START);
        state.station = state
            .station
            .clone()
            .with_loadouts(
                loadouts
                    .parse::<crate::directorates::q::LoadoutCatalog>()
                    .unwrap()
                    .with_default_secret_provider("fake-secrets"),
            )
            .with_account_homes(root.join("homes"));
        for id in ["mission-1", "mission-2"] {
            abandon(&state, id);
        }
        (state, now)
    }

    /// Starts `id` and waits for it to finish.
    async fn run(state: &DaemonState, id: &str) {
        create_mission(state, id);
        start_mission_by_id(state, id).await;
        wait_for_done(state, id).await;
        until_released(state).await;
    }

    fn launch_of(
        state: &DaemonState,
        id: &str,
    ) -> crate::directorates::registry::sessions::SessionLaunch {
        let hold = session_hold(state, id).unwrap();
        state
            .intake
            .session(&hold.id)
            .unwrap()
            .unwrap()
            .launch
            .unwrap()
    }

    fn report_use(state: &DaemonState, pool: &str, name: &str, used: u16) {
        state
            .intake
            .report_quota(
                &AccountRef::new(pool, name),
                &[QuotaWindow::new("five_hour", START as f64).used_permille(used)],
            )
            .unwrap();
    }

    #[test]
    fn the_models_each_account_may_use_are_listed_with_its_credential_and_it_is_never_in_the_answer()
     {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, _logins) = pooled_state(root.path(), "", "", ENV_POOLS);
        LISTED.lock().unwrap().clear();
        super::super::models::look(&state);

        let found = state.models.found();
        let sources: Vec<_> = found.iter().map(|found| found.source.as_str()).collect();
        assert_eq!(sources, ["elsewhere/gamma", "team/alpha", "team/beta"]);
        for entry in &found {
            assert!(entry.listing.known, "{entry:?}");
            assert_eq!(entry.listing.models[0].id, "mock-model");
            assert_eq!(
                entry.listing.models[0].description.as_deref(),
                Some("listed with a credential"),
                "the handler was given the account's credential: {entry:?}"
            );
        }
        let given = LISTED.lock().unwrap().clone();
        let credentials: Vec<String> = given
            .iter()
            .filter_map(|environment| {
                environment
                    .get("CLAUDE_CODE_OAUTH_TOKEN")
                    .or_else(|| environment.get("ANTHROPIC_API_KEY"))
                    .or_else(|| environment.get("ANTHROPIC_AUTH_TOKEN"))
                    .cloned()
            })
            .collect();
        // Other tests share the handler's record; these three accounts' credentials are among it.
        for expected in [
            "fake-env-alpha-0451",
            "fake-env-beta-0451",
            "fake-env-gamma-0451",
        ] {
            assert!(
                credentials.iter().any(|given| given == expected),
                "the handler was given {expected}"
            );
        }
        let reported = serde_json::to_string(&found).unwrap();
        let checked_in = serde_json::to_string(&serde_json::json!(state.models.found())).unwrap();
        for credential in &credentials {
            assert!(
                credential.starts_with("fake-"),
                "a planted credential: {credential}"
            );
            assert!(!reported.contains(credential.as_str()), "{reported}");
            assert!(!checked_in.contains(credential.as_str()));
        }
        assert!(
            given
                .iter()
                .all(|environment| !environment.contains_key("ANTHROPIC_BASE_URL")),
            "nothing but the account's own and the Station's base"
        );
    }

    #[test]
    fn a_route_no_pool_serves_is_listed_with_its_own_env_and_secret_and_none_leaks() {
        let root = tempfile::tempdir().unwrap();
        let agents = r#"[agent.007]
env = { SHARED = "agent" }
[[agent.007.route]]
handler = "mock-pooled"
model = "anthropic/name"
secrets = ["gateway"]
env = { GATEWAY_URL = "https://llm.example", SHARED = "route" }
"#;
        let loadouts = format!("{ROUTE_POOLS}[secret.gateway]\nenv = \"ANTHROPIC_AUTH_TOKEN\"\n");
        let (state, _now) = routed_state(root.path(), "", agents, &loadouts);
        LISTED.lock().unwrap().clear();
        super::super::models::look(&state);

        let found = state.models.found();
        let route = found
            .iter()
            .find(|found| found.agent.as_deref() == Some("007"))
            .expect("its route is looked at");
        assert!(route.listing.known, "{route:?}");
        assert_eq!(
            route.route.as_ref().unwrap().model.as_deref(),
            Some("anthropic/name")
        );
        let given = LISTED.lock().unwrap().clone();
        let asked = given
            .iter()
            .find(|environment| environment.contains_key("GATEWAY_URL"))
            .expect("the route's env reached the handler");
        assert_eq!(asked["SHARED"], "route", "the route's over the Agent's");
        assert_eq!(asked["ANTHROPIC_AUTH_TOKEN"], "fake-gateway-0451");
        assert!(
            !serde_json::to_string(&found)
                .unwrap()
                .contains("fake-gateway")
        );
        assert!(
            found.iter().any(|found| found.source == "one/a"),
            "the pool accounts are listed too"
        );
        assert!(
            state
                .station
                .credentials
                .issued_for("models-007-mock-pooled")
                .is_empty(),
            "the lease taken for a look is revoked"
        );
    }

    #[test]
    fn a_handler_that_cannot_list_or_is_not_here_says_unknown() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, _logins) = pooled_state(
            root.path(),
            "",
            "",
            r#"
[pool.absent]
handler = "not-on-this-station"
[pool.absent.account.a]
kind = "claude-oauth-token"
secret = "env-alpha"
"#,
        );
        super::super::models::look(&state);
        let found = state.models.found();
        let absent = found
            .iter()
            .find(|found| found.source == "absent/a")
            .expect("the account is looked at");
        assert!(!absent.listing.known && absent.listing.models.is_empty());
        assert!(
            absent
                .listing
                .unknown_because
                .as_deref()
                .is_some_and(|reason| reason.contains("not on this Station"))
        );
    }

    #[tokio::test]
    async fn headroom_sends_work_to_the_route_whose_best_account_has_most_left() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now) = routed_state(root.path(), "", &two_routes("headroom"), ROUTE_POOLS);
        report_use(&state, "one", "a", 800);
        report_use(&state, "two", "b", 100);
        run(&state, "route-headroom-1").await;
        assert_eq!(token("route-headroom-1"), "fake-two-b-0451");
        let launch = launch_of(&state, "route-headroom-1");
        assert_eq!(
            (
                launch.handler.as_str(),
                launch.model.as_deref(),
                launch.pool.as_deref()
            ),
            ("mock-pooled", Some("name-two"), Some("two")),
            "the Session record says which route ran"
        );
        // The mock reports 300 on the account it ran on, so `two` stays ahead of 800.
        report_use(&state, "two", "b", 900);
        run(&state, "route-headroom-2").await;
        assert_eq!(token("route-headroom-2"), "fake-one-a-0451");
        let chosen: Vec<_> = state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "route" && event.action == "chosen")
            .collect();
        assert_eq!(chosen.len(), 2, "the timeline says which route each took");
        assert_eq!(chosen[0].detail["model"], "name-two");
        assert_eq!(chosen[0].detail["pool"], "two");
        assert_eq!(chosen[1].detail["model"], "name-one");
    }

    #[tokio::test]
    async fn round_robin_takes_each_route_in_turn() {
        let root = tempfile::tempdir().unwrap();
        let (state, now) = routed_state(root.path(), "", &two_routes("round_robin"), ROUTE_POOLS);
        let mut seen = Vec::new();
        for (turn, id) in ["route-rr-1", "route-rr-2", "route-rr-3"]
            .iter()
            .enumerate()
        {
            now.store(START + 1 + turn as i64, Ordering::SeqCst);
            run(&state, id).await;
            seen.push(token(id));
        }
        assert_eq!(
            seen,
            ["fake-one-a-0451", "fake-two-b-0451", "fake-one-a-0451"]
        );
    }

    #[tokio::test]
    async fn priority_keeps_to_the_first_with_room_and_a_limited_route_is_skipped_until_its_reset()
    {
        let root = tempfile::tempdir().unwrap();
        let (state, now) = routed_state(root.path(), "", &two_routes("priority"), ROUTE_POOLS);
        run(&state, "route-priority-1").await;
        assert_eq!(token("route-priority-1"), "fake-two-b-0451", "priority 2");
        state
            .intake
            .limit_account(&AccountRef::new("two", "b"), (START + 100) as f64)
            .unwrap();
        run(&state, "route-priority-2").await;
        assert_eq!(token("route-priority-2"), "fake-one-a-0451");
        now.store(START + 101, Ordering::SeqCst);
        run(&state, "route-priority-3").await;
        assert_eq!(
            token("route-priority-3"),
            "fake-two-b-0451",
            "back after it"
        );
    }

    #[tokio::test]
    async fn a_held_route_takes_no_new_work_until_a_person_lifts_the_hold() {
        use crate::directorates::m::roster::RouteRef;
        use crate::directorates::registry::holds::RosterHold;

        let root = tempfile::tempdir().unwrap();
        let (state, _now) = routed_state(root.path(), "", &two_routes("priority"), ROUTE_POOLS);
        let two = RouteRef::new(
            "mock-pooled",
            Some("name-two".to_owned()),
            Some("two".to_owned()),
        );
        state
            .intake
            .put_hold(&RosterHold::new(
                "G",
                "007",
                Some(two.clone()),
                "failing",
                "alice",
                1.0,
            ))
            .unwrap();
        run(&state, "route-held-1").await;
        assert_eq!(
            token("route-held-1"),
            "fake-one-a-0451",
            "priority prefers two, which is held"
        );
        assert_eq!(
            launch_of(&state, "route-held-1").model.as_deref(),
            Some("name-one")
        );

        state.intake.lift_hold("G", "007", Some(&two)).unwrap();
        run(&state, "route-held-2").await;
        assert_eq!(token("route-held-2"), "fake-two-b-0451", "back once lifted");
    }

    #[tokio::test]
    async fn a_handler_missing_on_this_station_skips_its_routes_and_with_none_left_the_mission_waits()
     {
        let root = tempfile::tempdir().unwrap();
        let agents = r#"[agent.007]
balance = "priority"
[[agent.007.route]]
handler = "not-on-this-station"
model = "elsewhere"
priority = 9
[[agent.007.route]]
handler = "mock-pooled"
model = "here"
pool = "one"
"#;
        let (state, _now) = routed_state(root.path(), "", agents, ROUTE_POOLS);
        run(&state, "route-missing-1").await;
        assert_eq!(
            launch_of(&state, "route-missing-1").model.as_deref(),
            Some("here")
        );

        state
            .intake
            .limit_account(&AccountRef::new("one", "a"), (START + 100) as f64)
            .unwrap();
        create_mission(&state, "route-missing-2");
        let Err(refused) = start_mission(
            axum::extract::State(state.clone()),
            axum::extract::Path("route-missing-2".to_owned()),
            axum::Json(StartMission {
                working_directory: None,
                initial_context: Vec::new(),
                environment: BTreeMap::new(),
            }),
        )
        .await
        else {
            panic!("no route has room");
        };
        let reason = &refused.1.0.error;
        assert!(
            reason.contains("not-on-this-station elsewhere")
                && reason.contains("is not available on this Station"),
            "{reason}"
        );
        assert!(
            reason.contains("mock-pooled here (pool one)") && reason.contains("no account"),
            "{reason}"
        );
        let waiting = state
            .missions
            .lock()
            .unwrap()
            .get("route-missing-2")
            .unwrap()
            .waiting
            .clone();
        assert_eq!(
            waiting.as_ref(),
            Some(reason),
            "the Mission says why it waits"
        );
        assert!(
            calls("route-missing-2").is_empty(),
            "no harness was started"
        );
        assert_eq!(
            intake_hold(&state, "G", "007"),
            None,
            "another Station may have the first route's handler, so a brief need not wait"
        );
        let agent = state.roster.agent(Some("G"), "007").unwrap();
        let elsewhere = routes::implicit_pool("007", &agent.route_list()[0]);
        state
            .intake
            .limit_account(
                &AccountRef::new(elsewhere.id(), "login"),
                (START + 100) as f64,
            )
            .unwrap();
        let waits = intake_hold(&state, "G", "007").expect("no route has room anywhere");
        assert!(
            waits.contains("elsewhere") && waits.contains("here"),
            "{waits}"
        );
    }

    #[tokio::test]
    async fn a_route_that_cannot_hold_desk_is_not_chosen_and_the_reason_says_so() {
        let root = tempfile::tempdir().unwrap();
        let agents = r#"[agent.007]
clearance = "desk"
balance = "priority"
[[agent.007.route]]
handler = "mock-pooled"
priority = 5
[[agent.007.route]]
handler = "mock-agent"
"#;
        let (state, _now) =
            routed_state(root.path(), "clearance = \"desk\"\n", agents, ROUTE_POOLS);
        create_mission(&state, "route-desk-1");
        start_mission_by_id(&state, "route-desk-1").await;
        assert_eq!(launch_of(&state, "route-desk-1").handler, "mock-agent");
        assert!(
            calls("route-desk-1").is_empty(),
            "the other handler never ran"
        );
        state.stopping.store(true, Ordering::Release);

        let root = tempfile::tempdir().unwrap();
        let only =
            "[agent.007]\nclearance = \"desk\"\n[[agent.007.route]]\nhandler = \"mock-pooled\"\n";
        let (state, _now) = routed_state(root.path(), "clearance = \"desk\"\n", only, ROUTE_POOLS);
        create_mission(&state, "route-desk-2");
        let Err(refused) = start_mission(
            axum::extract::State(state.clone()),
            axum::extract::Path("route-desk-2".to_owned()),
            axum::Json(StartMission {
                working_directory: None,
                initial_context: Vec::new(),
                environment: BTreeMap::new(),
            }),
        )
        .await
        else {
            panic!("it cannot hold desk");
        };
        assert!(
            refused.1.0.error.contains("cannot hold desk clearance"),
            "{}",
            refused.1.0.error
        );
    }

    #[tokio::test]
    async fn a_route_brings_its_own_env_and_secret_even_to_an_exposed_session_and_its_pool_serves_by_name()
     {
        let root = tempfile::tempdir().unwrap();
        let agents = r#"[agent.007]
balance = "priority"
env = { SHARED = "agent" }
[[agent.007.route]]
handler = "mock-pooled"
model = "anthropic/name"
secrets = ["gateway"]
env = { GATEWAY_URL = "https://llm.example", SHARED = "route" }
[agent.009]
handler = "mock-pooled"
"#;
        let loadouts = format!(
            "{ROUTE_POOLS}[secret.gateway]\nenv = \"ANTHROPIC_AUTH_TOKEN\"\n[secret.other]\nenv = \"OTHER\"\n"
        );
        let (state, _now) = routed_state(root.path(), "", agents, &loadouts);
        create_mission(&state, "route-env-1");
        state
            .missions
            .lock()
            .unwrap()
            .expose("route-env-1", &["brief:source".to_owned()])
            .unwrap();
        start_mission_by_id(&state, "route-env-1").await;
        wait_for_done(&state, "route-env-1").await;
        let (_, _, environment, _) = calls("route-env-1").remove(0);
        assert_eq!(environment["GATEWAY_URL"], "https://llm.example");
        assert_eq!(
            environment["SHARED"], "route",
            "the route's over the Agent's"
        );
        assert_eq!(
            environment["ANTHROPIC_AUTH_TOKEN"], "fake-gateway-0451",
            "what reaches its model is issued whatever the Session read"
        );
        assert!(!environment.contains_key("OTHER"));
        assert!(
            !serde_json::to_string(&state.intake.timeline_events("G").unwrap())
                .unwrap()
                .contains("0451"),
            "the timeline carries names, never a credential"
        );
        until_released(&state).await;

        // Agent 009 names no pool, and these serve only who names them.
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new("route-plain-1").unwrap(),
                Some("G".to_owned()),
                "work".to_owned(),
                Some("009".to_owned()),
                None,
            )
            .unwrap();
        start_mission_by_id(&state, "route-plain-1").await;
        wait_for_done(&state, "route-plain-1").await;
        let (_, _, environment, _) = calls("route-plain-1").remove(0);
        assert!(!environment.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
        assert_eq!(
            state
                .intake
                .session(&session_hold(&state, "route-plain-1").unwrap().id)
                .unwrap()
                .unwrap()
                .account,
            None
        );
    }

    #[tokio::test]
    async fn a_route_naming_what_this_station_lacks_is_passed_over_with_the_reason() {
        let root = tempfile::tempdir().unwrap();
        let agents = r#"[agent.007]
balance = "priority"
[[agent.007.route]]
handler = "mock-pooled"
secrets = ["nowhere"]
[[agent.007.route]]
handler = "mock-pooled"
model = "green"
secrets = ["signed"]
[[agent.007.route]]
handler = "mock-pooled"
model = "unknown"
pool = "nowhere"
"#;
        let loadouts = format!(
            "{ROUTE_POOLS}[secret.signed]\nenv = \"X\"\nprovider = \"env\"\nkey = \"X\"\ngreen_ink = true\nkinds = [\"push\"]\n"
        );
        let (state, _now) = routed_state(root.path(), "", agents, &loadouts);
        let agent = state.roster.agent(Some("G"), "007").unwrap();
        let place = routes::Place {
            refuses: &|_| None,
            clearance: None,
            review: false,
        };
        let reason = routes::choose(&state, Some("G"), &agent, &place).unwrap_err();
        assert!(
            reason.contains("secret \"nowhere\" is not configured"),
            "{reason}"
        );
        assert!(reason.contains("green ink"), "{reason}");
        assert!(
            reason.contains("pinned pool \"nowhere\" is not configured"),
            "{reason}"
        );
    }

    #[tokio::test]
    async fn a_limit_moves_a_resumable_session_to_a_route_of_the_same_handler_and_says_so() {
        let root = tempfile::tempdir().unwrap();
        let loadouts = r#"
[pool.one]
handler = "mock-pooled"
[pool.one.account.a]
kind = "claude-oauth-token"
secret = "route-limited-a"
[pool.two]
handler = "mock-pooled"
[pool.two.account.b]
kind = "claude-oauth-token"
secret = "two-b"
"#;
        let (state, _now) = routed_state(root.path(), "", &two_routes("headroom"), loadouts);
        report_use(&state, "two", "b", 900);
        create_mission(&state, "route-move");
        start_mission_by_id(&state, "route-move").await;
        wait_for_state(&state, "route-move", MissionState::Parked).await;
        assert_eq!(
            events(&state, "limited")[0].detail["next"],
            "resume on another account"
        );
        assert!(account(&state, "one", "a").limited_until.is_some());
        schedule_parked_once(&state).await;
        wait_for_done(&state, "route-move").await;
        let made = calls("route-move");
        assert_eq!(
            made.iter().map(|call| call.1).collect::<Vec<_>>(),
            ["start", "resume"],
            "the same provider Session, since this kind carries it"
        );
        assert_eq!(made[1].2["CLAUDE_CODE_OAUTH_TOKEN"], "fake-two-b-0451");
        let launch = launch_of(&state, "route-move");
        assert_eq!(launch.model.as_deref(), Some("name-two"));
        assert_eq!(launch.pool.as_deref(), Some("two"));
        assert_eq!(
            state
                .intake
                .timeline_events("G")
                .unwrap()
                .iter()
                .filter(|event| event.kind == "route" && event.action == "moved")
                .count(),
            1,
            "the timeline says it moved"
        );
    }

    #[tokio::test]
    async fn a_resume_stays_on_its_route_when_only_that_route_has_room() {
        let root = tempfile::tempdir().unwrap();
        let loadouts = r#"
[pool.one]
handler = "mock-pooled"
[pool.one.account.a]
kind = "claude-oauth-token"
secret = "keep-limited-a"
"#;
        let agents = "[agent.007]\n[[agent.007.route]]\nhandler = \"mock-pooled\"\nmodel = \"name-one\"\npool = \"one\"\n";
        let (state, now) = routed_state(root.path(), "", agents, loadouts);
        create_mission(&state, "route-keep");
        start_mission_by_id(&state, "route-keep").await;
        wait_for_state(&state, "route-keep", MissionState::Parked).await;
        schedule_parked_once(&state).await;
        assert_eq!(calls("route-keep").len(), 1, "it waits for the reset");
        now.store(POOLED_RESET + 3_600, Ordering::SeqCst);
        schedule_parked_once(&state).await;
        wait_for_done(&state, "route-keep").await;
        let made = calls("route-keep");
        assert_eq!(
            made.iter().map(|call| call.1).collect::<Vec<_>>(),
            ["start", "resume"]
        );
        assert_eq!(
            made[1].2["CLAUDE_CODE_OAUTH_TOKEN"],
            "fake-keep-limited-a-0451"
        );
        assert_eq!(
            launch_of(&state, "route-keep").model.as_deref(),
            Some("name-one")
        );
    }

    #[tokio::test]
    async fn a_limit_on_a_route_history_cannot_leave_starts_a_new_attempt_on_another() {
        let root = tempfile::tempdir().unwrap();
        let agents = r#"[agent.007]
balance = "priority"
[[agent.007.route]]
handler = "mock-pooled"
model = "first"
secrets = ["fresh-limited-a"]
priority = 2
[[agent.007.route]]
handler = "mock-pooled"
model = "second"
secrets = ["second"]
"#;
        let loadouts = "[secret.fresh-limited-a]\nenv = \"CLAUDE_CODE_OAUTH_TOKEN\"\n\
                        [secret.second]\nenv = \"CLAUDE_CODE_OAUTH_TOKEN\"\n";
        let (state, _now) = routed_state(root.path(), "", agents, loadouts);
        declare(&state);
        create_mission(&state, "route-fresh");
        start_mission_by_id(&state, "route-fresh").await;
        tokio::time::timeout(Duration::from_secs(30), async {
            while state
                .missions
                .lock()
                .unwrap()
                .get("route-fresh")
                .unwrap()
                .state
                != MissionState::Queued
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("a new attempt is queued");
        let first_route = state
            .intake
            .accounts()
            .unwrap()
            .into_iter()
            .find(|row| row.is_limited(START as f64))
            .expect("the route's own account is marked");
        assert!(routes::is_implicit(&first_route.account.pool));
        assert_eq!(
            first_route.profile().unwrap().labels["model"],
            "first",
            "the board can say which route it is"
        );
        // The attempt it replaces closes just after the Mission is queued.
        schedule_queued_once(&state).await;
        wait_for_done(&state, "route-fresh").await;
        let made = calls("route-fresh");
        assert_eq!(
            made.iter().map(|call| call.1).collect::<Vec<_>>(),
            ["start", "start"],
            "history does not leave the first source"
        );
        assert_eq!(made[1].2["CLAUDE_CODE_OAUTH_TOKEN"], "fake-second-0451");
        assert!(
            made[1]
                .3
                .iter()
                .any(|line| line.contains("continues an earlier attempt")),
            "the Agent is told: {:?}",
            made[1].3
        );
        assert_eq!(
            launch_of(&state, "route-fresh").model.as_deref(),
            Some("second")
        );
    }

    #[tokio::test]
    async fn the_resume_rule_stays_moves_starts_afresh_or_waits() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now) = routed_state(root.path(), "", &two_routes("priority"), ROUTE_POOLS);
        let agent = state.roster.agent(Some("G"), "007").unwrap();
        let place = routes::Place {
            refuses: &|_| None,
            clearance: None,
            review: false,
        };
        let one = crate::directorates::m::roster::RouteRef::new(
            "mock-pooled",
            Some("name-one".to_owned()),
            Some("one".to_owned()),
        );
        let resume = |carried, earlier: &crate::directorates::m::roster::RouteRef| {
            routes::for_resume(&state, Some("G"), &agent, earlier, carried, &place)
        };
        assert!(
            matches!(resume(false, &one), routes::ResumeOn::Stay(on) if on.route.reference() == one)
        );
        state
            .intake
            .limit_account(&AccountRef::new("one", "a"), (START + 100) as f64)
            .unwrap();
        assert!(
            matches!(resume(true, &one), routes::ResumeOn::Move(on) if on.route.model.as_deref() == Some("name-two")),
            "same handler, an account kind that carries the history"
        );
        assert!(
            matches!(resume(false, &one), routes::ResumeOn::NewAttempt(why) if why.contains("name-one")),
            "the last account cannot carry it"
        );
        let gone = crate::directorates::m::roster::RouteRef::new(
            "mock-pooled",
            Some("gone".to_owned()),
            None,
        );
        assert!(
            matches!(resume(false, &gone), routes::ResumeOn::NewAttempt(why) if why.contains("no longer")),
        );
        state
            .intake
            .limit_account(&AccountRef::new("two", "b"), (START + 100) as f64)
            .unwrap();
        assert!(
            matches!(resume(true, &one), routes::ResumeOn::Wait(why) if why.contains("name-two")),
            "no route has room"
        );
    }

    #[tokio::test]
    async fn a_reattached_session_keeps_the_account_of_its_route_with_no_pool() {
        let root = tempfile::tempdir().unwrap();
        let agents = "[agent.007]\n[[agent.007.route]]\nhandler = \"mock-pooled\"\nmodel = \"m\"\n";
        let (state, _now) = routed_state(root.path(), "", agents, ROUTE_POOLS);
        run(&state, "route-again").await;
        let hold = session_hold(&state, "route-again").unwrap();
        let record = state.intake.session(&hold.id).unwrap().unwrap();
        let own = record.account.clone().expect("its own account is recorded");
        assert!(routes::is_implicit(&own.pool));
        let mission = state
            .missions
            .lock()
            .unwrap()
            .get("route-again")
            .cloned()
            .unwrap();
        reattach(&state, &mission, &record);
        assert_eq!(held(&state, "route-again"), Some(own));
    }

    /// Strict exposure takes an exposed Session's secrets, not the credential its harness reaches
    /// its model with: without that one no Source Mission could run at all.
    #[tokio::test]
    async fn an_exposed_session_gets_its_harness_credential_and_no_other_secret() {
        for (mission, loadouts, wanted) in [
            (
                "exposed-pooled",
                format!(
                    "{ENV_POOLS}\n[loadout.007]\nsecrets = [\"forge\"]\n[secret.forge]\nenv = \"GH_TOKEN\"\n"
                ),
                "fake-env-alpha-0451",
            ),
            (
                "exposed-loadout",
                "[loadout.007]\nsecrets = [\"model\", \"forge\"]\n\
                 [secret.model]\nenv = \"CLAUDE_CODE_OAUTH_TOKEN\"\n\
                 [secret.forge]\nenv = \"GH_TOKEN\"\n"
                    .to_owned(),
                "fake-model-0451",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let (state, _now, _) = pooled_state(root.path(), "", "", &loadouts);
            declare(&state);
            create_mission(&state, mission);
            state
                .missions
                .lock()
                .unwrap()
                .expose(mission, &["brief:source".to_owned()])
                .unwrap();
            start_mission_by_id(&state, mission).await;
            wait_for_done(&state, mission).await;
            let (_, _, environment, _) = calls(mission).remove(0);
            assert_eq!(
                environment
                    .get("CLAUDE_CODE_OAUTH_TOKEN")
                    .map(String::as_str),
                Some(wanted),
                "{mission}"
            );
            assert!(!environment.contains_key("GH_TOKEN"), "{mission}");
            until_released(&state).await;
        }
    }

    #[tokio::test]
    async fn a_session_gets_exactly_one_account_credential_and_frees_it_when_done() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, _) = pooled_state(
            root.path(),
            "",
            "[agent.007.env]\nANTHROPIC_API_KEY = \"stray\"\nCLAUDE_CONFIG_DIR = \"/stray\"\n",
            &format!(
                "{ENV_POOLS}\n[loadout.007]\nsecrets = [\"leftover\", \"forge\"]\n\
                 [secret.leftover]\nenv = \"ANTHROPIC_API_KEY\"\n\
                 [secret.forge]\nenv = \"GH_TOKEN\"\n"
            ),
        );
        declare(&state);
        create_mission(&state, "pooled-env");
        start_mission_by_id(&state, "pooled-env").await;
        wait_for_done(&state, "pooled-env").await;
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
        assert_eq!(
            environment["GH_TOKEN"], "fake-forge-0451",
            "a loadout secret outside the credential variables is still issued"
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
            alpha.profile().unwrap().operations,
            ["G"],
            "declared with the Operations that may draw from it"
        );
        assert!(
            account(&state, "elsewhere", "gamma")
                .profile()
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
        wait_for_done(&state, "pooled-rotate").await;
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
        wait_for_done(&state, "pooled-rotate-3").await;
        assert_eq!(token("pooled-rotate-3"), "fake-rotate-b-0451");
        until_released(&state).await;

        // ... and taken again after it, being the least recently used.
        now.store(POOLED_RESET + 1, Ordering::SeqCst);
        create_mission(&state, "pooled-rotate-4");
        start_mission_by_id(&state, "pooled-rotate-4").await;
        wait_for_done(&state, "pooled-rotate-4").await;
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
        // The Mission is queued again before the attempt it replaces is closed.
        tokio::time::timeout(Duration::from_secs(5), async {
            while state.intake.session(&first.id).unwrap().unwrap().status != SessionStatus::Ended {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the limited attempt is closed");
        schedule_queued_once(&state).await;
        wait_for_done(&state, "pooled-codex").await;
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
        assert_eq!(logins.lock().unwrap().homes.len(), 2);
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

    const LOGIN_POOLS: &str = "[pool.free]\nhandler = \"mock-pooled\"\n\
                               [pool.free.account.shared]\nkind = \"harness-login\"\n\
                               files = [\".fake/auth.json\"]\n";
    const LOGIN_FILE: &str = ".fake/auth.json";
    const LOGIN_TEXT: &str = r#"{"refresh_token":"planted-0451","expires_at":1790100000}"#;

    /// A Station with a sealing key, and a second Station's vault; the login is sealed to
    /// `stations`.
    fn login_state(
        root: &Path,
        stations: &[&str],
    ) -> (DaemonState, Arc<crate::directorates::q::sealing::Vault>) {
        use crate::directorates::q::logins::add_login;
        use crate::directorates::q::sealing::{Recipient, SealingKey, Vault};
        use crate::directorates::registry::sealed::StationSealingKey;

        let (mut state, _, _) = pooled_state(root, "", "", LOGIN_POOLS);
        let vault = |name: &str| {
            let key = Arc::new(SealingKey::generate());
            state
                .intake
                .publish_sealing_key(&StationSealingKey::new(name, key.public_key().to_text()))
                .unwrap();
            Arc::new(Vault::new(state.intake.clone(), name, key))
        };
        let (own, other) = (vault(state.station_id.as_str()), vault("other"));
        let dir = root.join("logged-in");
        fs::create_dir_all(dir.join(".fake")).unwrap();
        fs::write(dir.join(LOGIN_FILE), LOGIN_TEXT).unwrap();
        let named: Vec<Recipient> = stations
            .iter()
            .map(|name| {
                Recipient::named(match *name {
                    "own" => state.station_id.as_str(),
                    other => other,
                })
            })
            .collect();
        add_login(
            &other,
            &AccountRef::new("free", "shared"),
            &dir,
            &[LOGIN_FILE.to_owned()],
            &named,
            START as f64,
        )
        .unwrap();
        state.station.credentials.accounts.logins =
            super::super::logins::LoginDesk::default().with_vault(own);
        (state, other)
    }

    fn opened_login(vault: &crate::directorates::q::sealing::Vault) -> String {
        use crate::directorates::q::logins::sealed_name;
        vault
            .open(&sealed_name(&AccountRef::new("free", "shared")))
            .unwrap()
            .value
            .as_str()
            .to_owned()
    }

    #[tokio::test]
    async fn a_session_on_a_sealed_login_has_its_own_home_and_what_it_renews_comes_back() {
        use base64::Engine as _;
        let root = tempfile::tempdir().unwrap();
        let (state, other) = login_state(root.path(), &["own", "other"]);
        run(&state, "login-1").await;
        let made = calls("login-1");
        let home = made[0].2["HOME"].clone();
        assert!(home.ends_with("free--shared"), "{home}");
        assert!(Path::new(&home).starts_with(root.path()), "{home}");
        if cfg!(windows) {
            assert_eq!(made[0].2["USERPROFILE"], home);
        }
        let file = Path::new(&home).join(LOGIN_FILE);
        assert_eq!(fs::read_to_string(&file).unwrap(), LOGIN_TEXT);
        assert_eq!(
            state
                .intake
                .session(&session_hold(&state, "login-1").unwrap().id)
                .unwrap()
                .unwrap()
                .account,
            Some(AccountRef::new("free", "shared"))
        );

        // The harness renews: the next poll seals it for the other Station too.
        let renewed = r#"{"refresh_token":"renewed-0451","expires_at":1790200000}"#;
        fs::write(&file, renewed).unwrap();
        super::super::logins::tend(&state);
        let text = base64::engine::general_purpose::STANDARD;
        assert!(opened_login(&other).contains(&text.encode(renewed)));
        let login = account(&state, "free", "shared").login.unwrap();
        assert_eq!(login.revision, 2);
        assert_eq!(login.expires_at, Some(1_790_200_000.0));

        // The Mission is over: its home goes at the next check-in, and nothing stays.
        tend(&state);
        assert!(!Path::new(&home).exists());
        assert!(!Path::new(&home).parent().unwrap().exists());
    }

    #[tokio::test]
    async fn a_station_the_login_does_not_name_does_not_take_the_account() {
        let root = tempfile::tempdir().unwrap();
        let (state, other) = login_state(root.path(), &["other"]);
        let before = opened_login(&other);
        create_mission(&state, "login-2");
        let reason = refused_start(&state, "login-2").await;
        assert!(reason.contains("free"), "{reason}");
        assert!(calls("login-2").is_empty(), "its harness never started");
        assert_eq!(
            account(&state, "free", "shared").active.len(),
            0,
            "no claim was left behind"
        );
        assert_eq!(opened_login(&other), before);
    }

    #[tokio::test]
    async fn a_person_is_needed_and_the_account_is_passed_over() {
        let root = tempfile::tempdir().unwrap();
        let (state, _) = login_state(root.path(), &["own"]);
        state
            .intake
            .change_login(&AccountRef::new("free", "shared"), &mut |login| {
                login.as_mut().unwrap().needs_person = Some("a person must log in".to_owned());
            })
            .unwrap();
        create_mission(&state, "login-3");
        let reason = refused_start(&state, "login-3").await;
        assert!(reason.contains("free"), "{reason}");
        assert!(calls("login-3").is_empty(), "its harness never started");
    }

    /// Why a Mission that should not start did not.
    async fn refused_start(state: &DaemonState, id: &str) -> String {
        let Err(refused) = start_mission(
            axum::extract::State(state.clone()),
            axum::extract::Path(id.to_owned()),
            axum::Json(StartMission {
                working_directory: None,
                initial_context: Vec::new(),
                environment: BTreeMap::new(),
            }),
        )
        .await
        else {
            panic!("{id} started");
        };
        refused.1.0.error.clone()
    }

    #[test]
    fn a_failed_session_names_the_renewal_under_it() {
        let root = tempfile::tempdir().unwrap();
        let (state, other) = login_state(root.path(), &["own", "other"]);
        let pool = state
            .station
            .credentials
            .catalog()
            .unwrap()
            .pool("free")
            .unwrap()
            .clone();
        let site = Site {
            resuming: false,
            on_station: true,
        };
        let mut environment = BTreeMap::new();
        super::super::logins::hand_over(
            &state,
            &pool,
            pool.get("shared").unwrap(),
            "login-4",
            site,
            &mut environment,
        )
        .unwrap();
        assert_eq!(super::super::logins::stale_note(&state, "login-4"), None);
        let name = crate::directorates::q::logins::sealed_name(&AccountRef::new("free", "shared"));
        other
            .replace(&name, &opened_login(&other), 1, START as f64)
            .unwrap();
        super::super::logins::tend(&state);
        let note = super::super::logins::stale_note(&state, "login-4").unwrap();
        assert!(
            note.contains("free/shared") && note.contains("revision 1 to 2"),
            "{note}"
        );
        assert!(!note.contains("planted-0451"), "{note}");
        // A resume starts on the current login, and the note is spent.
        super::super::logins::hand_over(
            &state,
            &pool,
            pool.get("shared").unwrap(),
            "login-4",
            site,
            &mut environment,
        )
        .unwrap();
        assert_eq!(super::super::logins::stale_note(&state, "login-4"), None);
        // Off the Station's own host there is nowhere to write it.
        let refused = super::super::logins::hand_over(
            &state,
            &pool,
            pool.get("shared").unwrap(),
            "login-4",
            Site {
                resuming: false,
                on_station: false,
            },
            &mut BTreeMap::new(),
        )
        .unwrap_err();
        assert!(refused.contains("Station's own host"), "{refused}");
    }

    #[test]
    fn a_station_that_restarts_picks_up_its_homes_and_what_they_renewed() {
        let root = tempfile::tempdir().unwrap();
        let (state, other) = login_state(root.path(), &["own", "other"]);
        let pool = state
            .station
            .credentials
            .catalog()
            .unwrap()
            .pool("free")
            .unwrap()
            .clone();
        let mut environment = BTreeMap::new();
        let site = Site {
            resuming: false,
            on_station: true,
        };
        super::super::logins::hand_over(
            &state,
            &pool,
            pool.get("shared").unwrap(),
            "login-5",
            site,
            &mut environment,
        )
        .unwrap();
        let home = Path::new(&environment["HOME"]).to_path_buf();
        let scratch = state
            .station
            .credentials
            .accounts
            .homes()
            .unwrap()
            .join("renewing-left");
        fs::create_dir_all(&scratch).unwrap();
        fs::write(
            home.join(LOGIN_FILE),
            r#"{"refresh_token":"while-down","expires_at":1790300000}"#,
        )
        .unwrap();
        // The same state directory under a Station that remembers nothing.
        let mut restarted = state.clone();
        restarted.station.credentials.accounts.logins = super::super::logins::LoginDesk::default()
            .with_vault(
                state
                    .station
                    .credentials
                    .accounts
                    .logins
                    .vault()
                    .unwrap()
                    .clone(),
            );
        super::super::logins::recover(&restarted);
        let name = crate::directorates::q::logins::sealed_name(&AccountRef::new("free", "shared"));
        assert_eq!(
            other.open(&name).unwrap().revision,
            2,
            "what it renewed went back"
        );
        assert!(!scratch.exists(), "a throwaway home a renewal left is gone");
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

    const CODEX_ONE: &str = r#"
[pool.solo]
handler = "mock-pooled"
[pool.solo.account.only]
kind = "openai-api-key"
secret = "race-only"
"#;

    #[tokio::test]
    async fn a_check_in_during_a_slow_hand_over_keeps_the_new_claim() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, logins) = pooled_state(root.path(), "", "", CODEX_ONE);
        let (watcher, kept) = (state.clone(), Arc::new(Mutex::new(None)));
        let seen = kept.clone();
        logins.lock().unwrap().during = Some(Arc::new(move || {
            tend(&watcher);
            let claimed = watcher.intake.accounts().unwrap().iter().any(|account| {
                account
                    .live_claims(START as f64)
                    .any(|claim| claim.mission == "pooled-race")
            });
            *seen.lock().unwrap() = Some(claimed);
        }));
        create_mission(&state, "pooled-race");
        start_mission_by_id(&state, "pooled-race").await;
        wait_for_done(&state, "pooled-race").await;
        assert_eq!(
            *kept.lock().unwrap(),
            Some(true),
            "the check-in renewed the claim being handed over"
        );
    }

    #[test]
    fn a_reattached_session_takes_its_recorded_account_even_past_its_limit() {
        let root = tempfile::tempdir().unwrap();
        let loadouts = "[pool.solo]\nhandler = \"mock-pooled\"\n[pool.solo.account.only]\n\
                        kind = \"claude-oauth-token\"\nmax_concurrent = 1\n";
        let (state, _now, _) = pooled_state(root.path(), "", "", loadouts);
        let pool: crate::directorates::q::LoadoutCatalog = loadouts.parse().unwrap();
        let pool = pool.pool("solo").unwrap().clone();
        state
            .intake
            .claim_account(
                &AccountClaimRequest::new(
                    &pool,
                    "elsewhere",
                    "other",
                    crate::directorates::registry::store::DEFAULT_LEASE,
                ),
                START as f64,
            )
            .unwrap()
            .unwrap();
        create_mission(&state, "pooled-reattach");
        let mission = state
            .missions
            .lock()
            .unwrap()
            .get("pooled-reattach")
            .unwrap()
            .clone();
        let record = state
            .intake
            .open_session(
                crate::directorates::registry::sessions::NewSession::new(
                    "record-reattach",
                    "pooled-reattach",
                    "test",
                )
                .with_account(AccountRef::new("solo", "only")),
                crate::directorates::registry::store::DEFAULT_LEASE,
            )
            .unwrap();
        reattach(&state, &mission, &record);
        assert_eq!(
            held(&state, "pooled-reattach"),
            Some(AccountRef::new("solo", "only"))
        );
        tend(&state);
        let only = account(&state, "solo", "only");
        let missions: Vec<_> = only
            .live_claims(START as f64)
            .map(|claim| claim.mission.as_str())
            .collect();
        assert_eq!(
            missions,
            ["other", "pooled-reattach"],
            "counted past max_concurrent, and kept by the check-in"
        );
        let reclaimed = events(&state, "reclaimed");
        assert_eq!(reclaimed.len(), 1);
        assert!(reclaimed[0].detail["note"].contains("past max_concurrent"));
    }

    #[tokio::test]
    async fn a_claude_session_is_not_resumed_on_a_config_dir_account_but_tried_again() {
        let root = tempfile::tempdir().unwrap();
        let loadouts = r#"
[pool.mixed]
handler = "mock-pooled"
[pool.mixed.account.a]
kind = "claude-oauth-token"
secret = "mixed-limited-a"
[pool.mixed.account.b]
kind = "config-dir"
dir = "/logins/b"
env = "MOCK_CONFIG_DIR"
"#;
        let (state, _now, _) = pooled_state(root.path(), "", "", loadouts);
        create_mission(&state, "pooled-mixed");
        start_mission_by_id(&state, "pooled-mixed").await;
        wait_for_state(&state, "pooled-mixed", MissionState::Queued).await;
        assert_eq!(
            events(&state, "limited")[0].detail["next"],
            "new attempt on another account",
            "b's directory holds none of a's transcript"
        );
    }

    #[tokio::test]
    async fn a_limit_keeps_an_account_out_no_longer_than_the_horizon() {
        let root = tempfile::tempdir().unwrap();
        let loadouts = "[pool.short]\nhandler = \"mock-pooled\"\n[pool.short.account.a]\n\
                        kind = \"claude-oauth-token\"\nsecret = \"short-limited-a\"\n";
        let (mut state, _now, _) = pooled_state(root.path(), "", "", loadouts);
        let mut limits = crate::station::limits::StationLimits::default();
        limits.accounts.limit_horizon = crate::Seconds::new(60);
        state.station = state.station.clone().with_limits(limits);
        create_mission(&state, "pooled-short");
        start_mission_by_id(&state, "pooled-short").await;
        wait_for_state(&state, "pooled-short", MissionState::Parked).await;
        assert_eq!(
            account(&state, "short", "a").limited_until,
            Some((START + 60) as f64),
            "the reported reset is an hour away; the horizon is a minute"
        );
    }

    #[tokio::test]
    async fn a_mission_outside_any_operation_rotates_by_default() {
        let root = tempfile::tempdir().unwrap();
        let loadouts = r#"
[pool.free]
handler = "mock-pooled"
[pool.free.account.a]
kind = "claude-oauth-token"
secret = "free-limited-a"
[pool.free.account.b]
kind = "claude-oauth-token"
secret = "free-b"
"#;
        let (state, _now, _) = pooled_state(root.path(), "", "", loadouts);
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new("pooled-free").unwrap(),
                None,
                "work outside any Operation".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        let Ok(_) = start_mission(
            axum::extract::State(state.clone()),
            axum::extract::Path("pooled-free".to_owned()),
            axum::Json(StartMission {
                working_directory: Some(root.path().join("checkout").display().to_string()),
                initial_context: Vec::new(),
                environment: BTreeMap::new(),
            }),
        )
        .await
        else {
            panic!("the Mission starts");
        };
        wait_for_state(&state, "pooled-free", MissionState::Parked).await;
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get("pooled-free")
                .unwrap()
                .parked_until,
            Some(START),
            "rotated: parked only until the next pass, not until the reset"
        );
    }

    #[test]
    fn homes_go_only_when_made_here_and_the_mission_book_is_readable() {
        let root = tempfile::tempdir().unwrap();
        let (mut state, _now) = limited_state(
            root.path(),
            "",
            "[agent.007]\nhandler = \"mock-pooled\"\n",
            START,
        );
        let homes = root.path().join("homes");
        // No pool is configured any more; homes made for one still go.
        state.station = state
            .station
            .clone()
            .with_loadouts("[loadout.007]\n".parse().unwrap())
            .with_account_homes(homes.clone());
        for name in ["gone", "gone-later"] {
            fs::create_dir_all(homes.join(name).join("old--a")).unwrap();
            fs::write(homes.join(name).join(HOME_MARKER), b"").unwrap();
        }
        fs::create_dir_all(homes.join("someone-elses")).unwrap();
        let book = state.missions.clone();
        let _ = std::thread::spawn(move || {
            let _held = book.lock().unwrap();
            panic!("poison the Mission book");
        })
        .join();
        tend(&state);
        assert!(
            homes.join("gone").exists(),
            "an unreadable Mission book removes nothing"
        );
        state.missions = Arc::new(Mutex::new(Default::default()));
        tend(&state);
        assert!(!homes.join("gone").exists() && !homes.join("gone-later").exists());
        assert!(
            homes.join("someone-elses").exists(),
            "a directory the Station did not make stays"
        );
    }
}
