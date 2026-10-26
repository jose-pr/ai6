//! Pool accounts as every Station sees them: who is using each, how much of its provider
//! quota is left, until when it is limited, and the tokens it spent per day.
//!
//! Accounts are shared by every Station, so choosing one is a single Registry write: the
//! claim reads the candidates' states, picks by the pool's strategy and records the claim
//! under the same lock (file Registry) or transaction (SQL Registry) that serializes every
//! other write. A claim names its Station and Mission and lapses unless its Station renews
//! it, so a Station that goes dark frees its accounts with its Session leases.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::store::{FileRegistry, invalid_data, registry_lock, write_atomic};
use crate::directorates::medical::usage::utc_day;
use crate::directorates::q::{AccountPool, PoolStrategy};

pub(super) const MAX_ACCOUNT_RECORD_BYTES: u64 = 256 * 1024;
const MAX_CLAIMS: usize = 1024;
const MAX_WINDOWS: usize = 8;
const KEPT_DAYS: usize = 8;
/// The window use an account counts with before its provider has reported any.
pub const UNKNOWN_USE_PERMILLE: u16 = 500;

/// One account of one pool.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountRef {
    /// The pool ID.
    pub pool: String,
    /// The account's name in the pool.
    pub account: String,
}

impl AccountRef {
    /// Account `account` of pool `pool`.
    pub fn new(pool: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            pool: pool.into(),
            account: account.into(),
        }
    }
}

impl fmt::Display for AccountRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.pool, self.account)
    }
}

/// A Session's hold on an account.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountClaim {
    /// The Station running the Session.
    pub station: String,
    /// The Mission the Session works for.
    pub mission: String,
    /// When the claim was made, as Unix seconds.
    pub since: f64,
    /// When the claim lapses unless its Station renews it, as Unix seconds.
    pub expires: f64,
    /// The Mission's Operation, when it has one: the board shows another Operation's claims
    /// only as a count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
}

/// How much of one provider quota window an account has used, as last reported.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaWindow {
    /// The window's name (`five_hour`, `weekly`, ...).
    pub name: String,
    /// Thousandths of the window used, when the provider says.
    pub used_permille: Option<u16>,
    /// When the window resets, as Unix seconds, when the provider says.
    pub resets_at: Option<f64>,
    /// The provider's status for the window (`allowed`, `allowed_warning`, `rejected`), when
    /// it reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// When it was reported, as Unix seconds.
    pub observed_at: f64,
}

impl QuotaWindow {
    /// A report of window `name` observed at `observed_at`, with nothing else known.
    pub fn new(name: impl Into<String>, observed_at: f64) -> Self {
        Self {
            name: name.into(),
            used_permille: None,
            resets_at: None,
            status: None,
            observed_at,
        }
    }

    /// Sets the thousandths used.
    #[must_use]
    pub fn used_permille(mut self, used: u16) -> Self {
        self.used_permille = Some(used.min(1000));
        self
    }

    /// Sets the reset time.
    #[must_use]
    pub fn resets_at(mut self, at: f64) -> Self {
        self.resets_at = Some(at);
        self
    }

    /// Sets the provider's status.
    #[must_use]
    pub fn status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    /// The thousandths used at `now`: none once the window has reset, half when unknown.
    pub fn use_at(&self, now: f64) -> u16 {
        if self.resets_at.is_some_and(|reset| reset <= now) {
            return 0;
        }
        self.used_permille.unwrap_or(UNKNOWN_USE_PERMILLE)
    }
}

/// What a Station's configuration says about an account, for the board: never its secret.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountProfile {
    /// The AgentHandler the pool serves.
    pub handler: String,
    /// How the harness reads the credential (`claude-oauth-token`, ...).
    pub kind: String,
    /// The Operations that may draw from the pool.
    pub operations: Vec<String>,
    /// The account's priority.
    pub priority: i64,
    /// The most Sessions that may use it at once.
    pub max_concurrent: Option<usize>,
    /// Labels shown with the account.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl AccountProfile {
    /// The profile of `account` of `pool`, open to `operations`.
    pub fn of(pool: &AccountPool, account: &str, operations: Vec<String>) -> Option<Self> {
        let account = pool.get(account)?;
        Some(Self {
            handler: pool.handler().to_owned(),
            kind: account.kind().as_str().to_owned(),
            operations,
            priority: account.priority_value(),
            max_concurrent: account.concurrency(),
            labels: account.labels().clone(),
        })
    }
}

/// One account as the Registry holds it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountState {
    /// Which account.
    pub account: AccountRef,
    /// What each Station's configuration says about it, by Station ID: Stations with
    /// different rosters each keep their own.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, AccountProfile>,
    /// The Sessions using it now.
    #[serde(default)]
    pub active: Vec<AccountClaim>,
    /// The quota windows last reported, one per name.
    #[serde(default)]
    pub windows: Vec<QuotaWindow>,
    /// Until when its provider refuses work, as Unix seconds.
    pub limited_until: Option<f64>,
    /// When a Session last claimed it, as Unix seconds.
    pub last_used: Option<f64>,
    /// Tokens its Sessions used per UTC day (`YYYY-MM-DD`), the last few days.
    #[serde(default)]
    pub daily_tokens: BTreeMap<String, u64>,
}

impl AccountState {
    /// A state with nothing recorded.
    pub fn new(account: AccountRef) -> Self {
        Self {
            account,
            profiles: BTreeMap::new(),
            active: Vec::new(),
            windows: Vec::new(),
            limited_until: None,
            last_used: None,
            daily_tokens: BTreeMap::new(),
        }
    }

    /// What the Stations say about it together: the first Station's settings, open to every
    /// Operation any Station names. `None` until one has declared it.
    pub fn profile(&self) -> Option<AccountProfile> {
        let mut merged = self.profiles.values().next()?.clone();
        let mut operations: Vec<String> = self
            .profiles
            .values()
            .flat_map(|profile| profile.operations.iter().cloned())
            .collect();
        operations.sort();
        operations.dedup();
        merged.operations = operations;
        Some(merged)
    }

    /// The claims still live at `now`.
    pub fn live_claims(&self, now: f64) -> impl Iterator<Item = &AccountClaim> {
        self.active.iter().filter(move |claim| claim.expires >= now)
    }

    /// Whether its provider refuses work at `now`.
    pub fn is_limited(&self, now: f64) -> bool {
        self.limited_until.is_some_and(|until| until > now)
    }

    /// The use of its most used window at `now` ([`QuotaWindow::use_at`]); half when none
    /// was reported.
    pub fn highest_use(&self, now: f64) -> u16 {
        self.windows
            .iter()
            .map(|window| window.use_at(now))
            .max()
            .unwrap_or(UNKNOWN_USE_PERMILLE)
    }

    /// Tokens used on the UTC day containing `now`.
    pub fn tokens_on_day(&self, now: i64) -> u64 {
        self.daily_tokens
            .get(&utc_day(now))
            .copied()
            .unwrap_or_default()
    }
}

/// One account a claim may pick, with the settings its pool gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCandidate {
    /// The account's name in the pool.
    pub account: String,
    /// Its priority; higher first.
    pub priority: i64,
    /// The most Sessions that may use it at once.
    pub max_concurrent: Option<usize>,
}

/// A Session's request for one account of a pool.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountClaimRequest {
    pool: String,
    strategy: PoolStrategy,
    candidates: Vec<AccountCandidate>,
    prefer: Option<String>,
    station: String,
    mission: String,
    operation: Option<String>,
    ttl: Duration,
    in_use: bool,
}

impl AccountClaimRequest {
    /// A claim on any account of `pool` for `station`'s Session of `mission`, balanced as
    /// the pool says, held for `ttl` unless renewed.
    pub fn new(
        pool: &AccountPool,
        station: impl Into<String>,
        mission: impl Into<String>,
        ttl: Duration,
    ) -> Self {
        Self {
            pool: pool.id().to_owned(),
            strategy: pool.balancing(),
            candidates: pool
                .accounts()
                .map(|account| AccountCandidate {
                    account: account.name().to_owned(),
                    priority: account.priority_value(),
                    max_concurrent: account.concurrency(),
                })
                .collect(),
            prefer: None,
            station: station.into(),
            mission: mission.into(),
            operation: None,
            ttl,
            in_use: false,
        }
    }

    /// Records the Mission's Operation on the claim.
    #[must_use]
    pub fn operation(mut self, operation: Option<String>) -> Self {
        self.operation = operation;
        self
    }

    /// Records a claim on the first candidate whatever its room: a harness already running
    /// on the account (a re-attached Session) is counted even past `max_concurrent` or a
    /// limit.
    #[must_use]
    pub fn in_use(mut self) -> Self {
        self.in_use = true;
        self
    }

    /// Takes `account` whenever it has room, before the strategy is consulted: a Session
    /// keeps the account its provider history lives with.
    #[must_use]
    pub fn prefer(mut self, account: impl Into<String>) -> Self {
        self.prefer = Some(account.into());
        self
    }

    /// Leaves `account` out.
    #[must_use]
    pub fn excluding(mut self, account: &str) -> Self {
        self.candidates
            .retain(|candidate| candidate.account != account);
        self
    }

    /// Considers `account` alone: a provider Session that cannot move to another account.
    #[must_use]
    pub fn only(mut self, account: &str) -> Self {
        self.candidates
            .retain(|candidate| candidate.account == account);
        self
    }

    /// The pool ID.
    pub fn pool(&self) -> &str {
        &self.pool
    }

    /// The accounts it may pick.
    pub fn candidates(&self) -> &[AccountCandidate] {
        &self.candidates
    }
}

/// Which account a pool's strategy picks among `candidates` at `now`, given their states:
/// [`PoolStrategy`] says how, after `prefer` when it has room. An account has room when it is
/// not limited and has fewer live claims than its `max_concurrent`.
pub fn choose_account<'a>(
    strategy: PoolStrategy,
    candidates: &'a [AccountCandidate],
    states: &BTreeMap<String, AccountState>,
    prefer: Option<&str>,
    now: f64,
) -> Option<&'a AccountCandidate> {
    let active = |candidate: &AccountCandidate| {
        states
            .get(&candidate.account)
            .map_or(0, |state| state.live_claims(now).count())
    };
    let room = |candidate: &&AccountCandidate| {
        let state = states.get(&candidate.account);
        !state.is_some_and(|state| state.is_limited(now))
            && candidate
                .max_concurrent
                .is_none_or(|limit| active(candidate) < limit)
    };
    let open: Vec<&AccountCandidate> = candidates.iter().filter(room).collect();
    if let Some(preferred) = prefer.and_then(|name| open.iter().find(|c| c.account == name)) {
        return Some(preferred);
    }
    let last_used = |candidate: &AccountCandidate| {
        states
            .get(&candidate.account)
            .and_then(|state| state.last_used)
            .unwrap_or(f64::NEG_INFINITY)
    };
    let use_of = |candidate: &AccountCandidate| {
        states
            .get(&candidate.account)
            .map_or(UNKNOWN_USE_PERMILLE, |state| state.highest_use(now))
    };
    open.into_iter().min_by(|left, right| {
        let by_name = left.account.cmp(&right.account);
        let by_last_used = last_used(left).total_cmp(&last_used(right));
        let by_priority = right.priority.cmp(&left.priority);
        match strategy {
            PoolStrategy::RoundRobin => by_last_used.then(by_name),
            PoolStrategy::Priority => by_priority.then(by_name),
            _ => use_of(left)
                .cmp(&use_of(right))
                .then(active(left).cmp(&active(right)))
                .then(by_last_used)
                .then(by_priority)
                .then(by_name),
        }
    })
}

/// Applies a claim to the candidates' `states`: drops lapsed claims and any earlier claim of
/// the same Session, then records the claim on the chosen account. Returns the chosen
/// account and the names of every state that changed.
pub(super) fn claim_in(
    states: &mut BTreeMap<String, AccountState>,
    request: &AccountClaimRequest,
    now: f64,
) -> (Option<String>, Vec<String>) {
    let mut changed = Vec::new();
    for candidate in &request.candidates {
        let state = states.entry(candidate.account.clone()).or_insert_with(|| {
            AccountState::new(AccountRef::new(&request.pool, &candidate.account))
        });
        let before = state.active.len();
        state.active.retain(|claim| {
            claim.expires >= now
                && !(claim.station == request.station && claim.mission == request.mission)
        });
        if state.active.len() != before {
            changed.push(candidate.account.clone());
        }
    }
    let chosen = if request.in_use {
        request.candidates.first()
    } else {
        choose_account(
            request.strategy,
            &request.candidates,
            states,
            request.prefer.as_deref(),
            now,
        )
    }
    .map(|candidate| candidate.account.clone());
    if let Some(name) = &chosen
        && let Some(state) = states.get_mut(name)
    {
        state.active.push(AccountClaim {
            station: request.station.clone(),
            mission: request.mission.clone(),
            since: now,
            expires: now + request.ttl.as_secs_f64(),
            operation: request.operation.clone(),
        });
        if state.active.len() > MAX_CLAIMS {
            state.active.remove(0);
        }
        state.last_used = Some(now);
        if !changed.contains(name) {
            changed.push(name.clone());
        }
    }
    (chosen, changed)
}

/// Applies a claim to every recorded account: [`claim_in`] on the request's pool, and the
/// Session's claims on any other account dropped, so a Mission holds one account at a time.
/// Returns the chosen account's state and every state that changed.
pub(super) fn claim_among(
    all: Vec<AccountState>,
    request: &AccountClaimRequest,
    now: f64,
) -> (Option<AccountState>, Vec<AccountState>) {
    let mut pool = BTreeMap::new();
    let mut changed = Vec::new();
    for mut state in all {
        if state.account.pool == request.pool
            && request
                .candidates
                .iter()
                .any(|candidate| candidate.account == state.account.account)
        {
            pool.insert(state.account.account.clone(), state);
        } else if release_in(&mut state, &request.station, &request.mission) {
            changed.push(state);
        }
    }
    let (chosen, names) = claim_in(&mut pool, request, now);
    changed.extend(names.iter().filter_map(|name| pool.get(name).cloned()));
    (chosen.and_then(|name| pool.remove(&name)), changed)
}

/// Drops `station`'s claims for `mission`; whether there were any.
pub(super) fn release_in(state: &mut AccountState, station: &str, mission: &str) -> bool {
    let before = state.active.len();
    state
        .active
        .retain(|claim| !(claim.station == station && claim.mission == mission));
    state.active.len() != before
}

/// Extends `station`'s claims for `missions` to `now + ttl`, drops its others made before
/// `now` (a claim made since the caller listed its Missions stays) and every lapsed one;
/// whether anything changed.
pub(super) fn renew_in(
    state: &mut AccountState,
    station: &str,
    missions: &[String],
    ttl: Duration,
    now: f64,
) -> bool {
    let before = state.active.clone();
    state.active.retain(|claim| {
        claim.expires >= now
            && (claim.station != station
                || claim.since >= now
                || missions.iter().any(|m| m == &claim.mission))
    });
    for claim in &mut state.active {
        if claim.station == station {
            claim.expires = now + ttl.as_secs_f64();
        }
    }
    state.active != before
}

/// Merges `reports` into `state`'s windows, replacing each window of the same name.
pub(super) fn report_in(state: &mut AccountState, reports: &[QuotaWindow]) {
    for report in reports {
        state.windows.retain(|window| window.name != report.name);
        state.windows.push(report.clone());
    }
    state
        .windows
        .sort_by(|left, right| left.name.cmp(&right.name));
    while state.windows.len() > MAX_WINDOWS {
        state.windows.remove(0);
    }
}

/// Adds `tokens` to the UTC day of `now`, keeping the last few days.
pub(super) fn add_tokens(state: &mut AccountState, tokens: u64, now: i64) {
    let day = state.daily_tokens.entry(utc_day(now)).or_default();
    *day = day.saturating_add(tokens);
    while state.daily_tokens.len() > KEPT_DAYS {
        state.daily_tokens.pop_first();
    }
}

/// Checks the names of an account before a path or row is made from them.
pub(super) fn validate_ref(account: &AccountRef) -> io::Result<()> {
    let valid = |value: &str| {
        let mut bytes = value.bytes();
        value.len() <= 64
            && bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    };
    if valid(&account.pool) && valid(&account.account) {
        Ok(())
    } else {
        Err(invalid_data(format!("invalid pool account {account}")))
    }
}

/// Pool accounts: their claims, quota and daily tokens.
pub trait AccountStore {
    /// Records what `station`'s configuration says about an account, replacing only that
    /// Station's profile and keeping what Sessions recorded.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name or when the Registry cannot be written.
    fn declare_account(
        &self,
        account: &AccountRef,
        station: &str,
        profile: &AccountProfile,
    ) -> io::Result<()>;

    /// Picks an account of the request's pool and records the claim, in one write; the
    /// Session's claims on any other account are dropped in the same write. `None` when no
    /// account has room: each is limited or at its `max_concurrent`.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read or written.
    fn claim_account(
        &self,
        request: &AccountClaimRequest,
        now: f64,
    ) -> io::Result<Option<AccountState>>;

    /// Drops `station`'s claims for `mission`, once its Session ends, is fenced out or fails;
    /// whether there were any.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn release_account(&self, station: &str, mission: &str) -> io::Result<bool>;

    /// Extends `station`'s claims for the Missions it still runs and drops its others.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn renew_account_claims(
        &self,
        station: &str,
        missions: &[String],
        ttl: Duration,
        now: f64,
    ) -> io::Result<()>;

    /// Records quota windows a provider reported for an account.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name or when the Registry cannot be written.
    fn report_quota(&self, account: &AccountRef, windows: &[QuotaWindow]) -> io::Result<()>;

    /// Marks an account limited until `until` (Unix seconds); a later limit already recorded
    /// is kept.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name or when the Registry cannot be written.
    fn limit_account(&self, account: &AccountRef, until: f64) -> io::Result<()>;

    /// Lifts an account's limit; whether it had one.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name or when the Registry cannot be written.
    fn clear_account_limit(&self, account: &AccountRef) -> io::Result<bool>;

    /// Adds `tokens` to an account's count for the UTC day of `now` (Unix seconds).
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name or when the Registry cannot be written.
    fn record_account_usage(&self, account: &AccountRef, tokens: u64, now: i64) -> io::Result<()>;

    /// Every account recorded, by pool and name.
    ///
    /// # Errors
    ///
    /// Returns an error when a record cannot be read.
    fn accounts(&self) -> io::Result<Vec<AccountState>>;
}

impl FileRegistry {
    fn accounts_dir(&self) -> PathBuf {
        self.root.join("accounts")
    }

    fn account_path(&self, account: &AccountRef) -> PathBuf {
        self.accounts_dir()
            .join(&account.pool)
            .join(format!("{}.json", account.account))
    }

    fn read_account(&self, account: &AccountRef) -> io::Result<AccountState> {
        let bytes = match fs::read(self.account_path(account)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(AccountState::new(account.clone()));
            }
            Err(error) => return Err(error),
        };
        parse_state(&bytes)
    }

    fn write_account(&self, state: &AccountState) -> io::Result<()> {
        write_atomic(&self.account_path(&state.account), &state_bytes(state)?)
    }

    /// Reads, changes and writes one account under the Registry lock.
    fn update_account(
        &self,
        account: &AccountRef,
        change: impl FnOnce(&mut AccountState),
    ) -> io::Result<()> {
        validate_ref(account)?;
        let _lock = registry_lock(&self.root)?;
        let mut state = self.read_account(account)?;
        change(&mut state);
        self.write_account(&state)
    }
}

/// A stored account, refused past the size limit.
pub(super) fn parse_state(bytes: &[u8]) -> io::Result<AccountState> {
    if bytes.len() as u64 > MAX_ACCOUNT_RECORD_BYTES {
        return Err(invalid_data("account record exceeds the size limit"));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| invalid_data(format!("invalid account record: {error}")))
}

/// An account as stored, refused past the size limit.
pub(super) fn state_bytes(state: &AccountState) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(state).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_ACCOUNT_RECORD_BYTES {
        return Err(invalid_data("account record exceeds the size limit"));
    }
    Ok(bytes)
}

impl AccountStore for FileRegistry {
    fn declare_account(
        &self,
        account: &AccountRef,
        station: &str,
        profile: &AccountProfile,
    ) -> io::Result<()> {
        self.update_account(account, |state| {
            state.profiles.insert(station.to_owned(), profile.clone());
        })
    }

    fn claim_account(
        &self,
        request: &AccountClaimRequest,
        now: f64,
    ) -> io::Result<Option<AccountState>> {
        for candidate in &request.candidates {
            validate_ref(&AccountRef::new(&request.pool, &candidate.account))?;
        }
        let _lock = registry_lock(&self.root)?;
        let (chosen, changed) = claim_among(self.accounts()?, request, now);
        for state in &changed {
            self.write_account(state)?;
        }
        Ok(chosen)
    }

    fn release_account(&self, station: &str, mission: &str) -> io::Result<bool> {
        let _lock = registry_lock(&self.root)?;
        let mut released = false;
        for mut state in self.accounts()? {
            if release_in(&mut state, station, mission) {
                self.write_account(&state)?;
                released = true;
            }
        }
        Ok(released)
    }

    fn renew_account_claims(
        &self,
        station: &str,
        missions: &[String],
        ttl: Duration,
        now: f64,
    ) -> io::Result<()> {
        let _lock = registry_lock(&self.root)?;
        for mut state in self.accounts()? {
            if renew_in(&mut state, station, missions, ttl, now) {
                self.write_account(&state)?;
            }
        }
        Ok(())
    }

    fn report_quota(&self, account: &AccountRef, windows: &[QuotaWindow]) -> io::Result<()> {
        self.update_account(account, |state| report_in(state, windows))
    }

    fn limit_account(&self, account: &AccountRef, until: f64) -> io::Result<()> {
        self.update_account(account, |state| {
            state.limited_until = Some(state.limited_until.map_or(until, |own| own.max(until)));
        })
    }

    fn clear_account_limit(&self, account: &AccountRef) -> io::Result<bool> {
        let mut cleared = false;
        self.update_account(account, |state| {
            cleared = state.limited_until.take().is_some();
        })?;
        Ok(cleared)
    }

    fn record_account_usage(&self, account: &AccountRef, tokens: u64, now: i64) -> io::Result<()> {
        self.update_account(account, |state| add_tokens(state, tokens, now))
    }

    fn accounts(&self) -> io::Result<Vec<AccountState>> {
        let pools = match fs::read_dir(self.accounts_dir()) {
            Ok(pools) => pools,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut states = Vec::new();
        for pool in pools {
            let pool = pool?.path();
            if !pool.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&pool)? {
                let path = entry?.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    states.push(parse_state(&fs::read(&path)?)?);
                }
            }
        }
        states.sort_by(|left, right| left.account.cmp(&right.account));
        Ok(states)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;
    use crate::directorates::q::{Account, AccountKind};
    use crate::directorates::registry::backend::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    const NOW: f64 = 1_790_000_000.0;
    const TTL: Duration = Duration::from_secs(60);

    fn pool(strategy: PoolStrategy, accounts: &[(&str, i64, Option<usize>)]) -> AccountPool {
        let mut pool = AccountPool::new("team", "claude").strategy(strategy);
        for (name, priority, limit) in accounts {
            let mut account =
                Account::new(*name, AccountKind::ClaudeOauthToken).priority(*priority);
            if let Some(limit) = limit {
                account = account.max_concurrent(*limit);
            }
            pool = pool.account(account);
        }
        pool
    }

    fn claim(
        registry: &dyn RegistryBackend,
        pool: &AccountPool,
        station: &str,
        mission: &str,
        now: f64,
    ) -> Option<String> {
        registry
            .claim_account(&AccountClaimRequest::new(pool, station, mission, TTL), now)
            .unwrap()
            .map(|state| state.account.account)
    }

    fn state(registry: &dyn RegistryBackend, name: &str) -> AccountState {
        registry
            .accounts()
            .unwrap()
            .into_iter()
            .find(|state| state.account.account == name)
            .unwrap()
    }

    backend_tests!(
        concurrent_stations_claiming_a_pool_of_one_get_one_winner,
        a_limited_account_is_skipped_until_its_reset,
        headroom_prefers_the_least_used_then_the_least_busy_account,
        round_robin_and_priority_pick_as_they_say,
        a_session_keeps_its_preferred_account_while_it_has_room,
        claims_are_released_on_session_end_and_lapse_with_a_dark_station,
        quota_reports_and_usage_are_kept_per_account,
        a_claim_in_another_pool_frees_the_missions_earlier_account,
        stations_with_different_rosters_keep_their_own_profiles,
    );

    fn a_claim_in_another_pool_frees_the_missions_earlier_account(store: &dyn Store) {
        let registry = store.handle();
        let first = pool(PoolStrategy::Headroom, &[("a", 0, None)]);
        let second = AccountPool::new("other", "claude")
            .account(Account::new("b", AccountKind::ClaudeOauthToken));
        assert!(claim(registry.as_ref(), &first, "s", "m1", NOW).is_some());
        assert!(claim(registry.as_ref(), &second, "s", "m1", NOW).is_some());
        assert!(
            state(registry.as_ref(), "a").active.is_empty(),
            "a Mission holds one account at a time"
        );
        assert_eq!(state(registry.as_ref(), "b").active.len(), 1);
    }

    fn stations_with_different_rosters_keep_their_own_profiles(store: &dyn Store) {
        let registry = store.handle();
        let account = AccountRef::new("team", "a");
        let pool = pool(PoolStrategy::Headroom, &[("a", 0, None)]);
        for (station, operations) in [("s1", vec!["G"]), ("s2", vec!["L"]), ("s1", vec!["G", "H"])]
        {
            let profile = AccountProfile::of(
                &pool,
                "a",
                operations.into_iter().map(str::to_owned).collect(),
            )
            .unwrap();
            registry
                .declare_account(&account, station, &profile)
                .unwrap();
        }
        let a = state(registry.as_ref(), "a");
        assert_eq!(
            a.profiles.len(),
            2,
            "one per Station; a Station replaces its own"
        );
        assert_eq!(a.profile().unwrap().operations, ["G", "H", "L"]);
    }

    fn concurrent_stations_claiming_a_pool_of_one_get_one_winner(store: &dyn Store) {
        let pool = Arc::new(pool(PoolStrategy::Headroom, &[("only", 0, Some(1))]));
        for round in 0..10 {
            let stations = 6;
            let barrier = Arc::new(Barrier::new(stations));
            let winners: Vec<_> = (0..stations)
                .map(|index| {
                    let (registry, pool, barrier) = (store.handle(), pool.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        claim(
                            registry.as_ref(),
                            &pool,
                            &format!("station-{index}"),
                            &format!("mission-{round}-{index}"),
                            NOW + f64::from(round),
                        )
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|handle| handle.join().unwrap())
                .collect();
            assert_eq!(winners, ["only"], "round {round}: one winner");
            let registry = store.handle();
            assert_eq!(state(registry.as_ref(), "only").live_claims(NOW).count(), 1);
            let holder = state(registry.as_ref(), "only").active[0].clone();
            assert!(
                registry
                    .release_account(&holder.station, &holder.mission)
                    .unwrap()
            );
        }
    }

    fn a_limited_account_is_skipped_until_its_reset(store: &dyn Store) {
        let registry = store.handle();
        let pool = pool(PoolStrategy::Headroom, &[("a", 0, None), ("b", 0, None)]);
        registry
            .limit_account(&AccountRef::new("team", "a"), NOW + 3_600.0)
            .unwrap();
        registry
            .limit_account(&AccountRef::new("team", "a"), NOW + 60.0)
            .unwrap();
        assert_eq!(
            state(registry.as_ref(), "a").limited_until,
            Some(NOW + 3_600.0)
        );
        for index in 0..3 {
            assert_eq!(
                claim(registry.as_ref(), &pool, "s", &format!("m{index}"), NOW).as_deref(),
                Some("b")
            );
        }
        let request = AccountClaimRequest::new(&pool, "s", "m9", TTL).excluding("b");
        assert!(registry.claim_account(&request, NOW).unwrap().is_none());
        assert_eq!(
            registry
                .claim_account(&request, NOW + 3_600.0)
                .unwrap()
                .map(|state| state.account.account)
                .as_deref(),
            Some("a"),
            "used again after its reset"
        );
    }

    fn headroom_prefers_the_least_used_then_the_least_busy_account(store: &dyn Store) {
        let registry = store.handle();
        let pool = pool(
            PoolStrategy::Headroom,
            &[("a", 0, None), ("b", 0, None), ("c", 0, None)],
        );
        let report = |name: &str, used: u16| {
            registry
                .report_quota(
                    &AccountRef::new("team", name),
                    &[QuotaWindow::new("five_hour", NOW).used_permille(used)],
                )
                .unwrap();
        };
        report("a", 800);
        report("b", 200);
        // c has reported nothing, so it counts as half used.
        assert_eq!(
            claim(registry.as_ref(), &pool, "s", "m1", NOW).as_deref(),
            Some("b")
        );
        report("b", 600);
        assert_eq!(
            claim(registry.as_ref(), &pool, "s", "m2", NOW).as_deref(),
            Some("c")
        );
        report("c", 600);
        // b and c are equally used; b's Session has ended and c's has not.
        assert!(registry.release_account("s", "m1").unwrap());
        assert_eq!(
            claim(registry.as_ref(), &pool, "s", "m3", NOW).as_deref(),
            Some("b")
        );
        // A window that has reset counts as unused.
        registry
            .report_quota(
                &AccountRef::new("team", "a"),
                &[QuotaWindow::new("five_hour", NOW)
                    .used_permille(900)
                    .resets_at(NOW + 10.0)],
            )
            .unwrap();
        assert_eq!(
            claim(registry.as_ref(), &pool, "s", "m4", NOW + 11.0).as_deref(),
            Some("a")
        );
    }

    fn round_robin_and_priority_pick_as_they_say(store: &dyn Store) {
        let registry = store.handle();
        let accounts = [("a", 1, None), ("b", 5, Some(1)), ("c", 3, None)];
        let rotating = pool(PoolStrategy::RoundRobin, &accounts);
        let picks: Vec<_> = (0..4)
            .map(|index| {
                claim(
                    registry.as_ref(),
                    &rotating,
                    "s",
                    &format!("r{index}"),
                    NOW + f64::from(index),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(
            picks,
            ["a", "b", "c", "a"],
            "b is full after its one Session"
        );

        let ranked = AccountPool::new("ranked", "claude")
            .strategy(PoolStrategy::Priority)
            .account(Account::new("a", AccountKind::ClaudeOauthToken).priority(1))
            .account(
                Account::new("b", AccountKind::ClaudeOauthToken)
                    .priority(5)
                    .max_concurrent(1),
            )
            .account(Account::new("c", AccountKind::ClaudeOauthToken).priority(3));
        assert_eq!(
            claim(registry.as_ref(), &ranked, "s", "p1", NOW).as_deref(),
            Some("b")
        );
        assert_eq!(
            claim(registry.as_ref(), &ranked, "s", "p2", NOW).as_deref(),
            Some("c")
        );
    }

    fn a_session_keeps_its_preferred_account_while_it_has_room(store: &dyn Store) {
        let registry = store.handle();
        let pool = pool(PoolStrategy::Headroom, &[("a", 0, None), ("b", 0, Some(1))]);
        registry
            .report_quota(
                &AccountRef::new("team", "b"),
                &[QuotaWindow::new("five_hour", NOW).used_permille(900)],
            )
            .unwrap();
        let request = AccountClaimRequest::new(&pool, "s", "m1", TTL).prefer("b");
        let chosen = registry.claim_account(&request, NOW).unwrap().unwrap();
        assert_eq!(chosen.account.account, "b");
        // Claiming again for the same Session replaces its claim rather than adding one.
        let again = registry
            .claim_account(&request, NOW + 1.0)
            .unwrap()
            .unwrap();
        assert_eq!(again.account.account, "b");
        assert_eq!(again.live_claims(NOW).count(), 1);
        // Full for another Session, so the preference gives way.
        let other = AccountClaimRequest::new(&pool, "s", "m2", TTL).prefer("b");
        assert_eq!(
            registry
                .claim_account(&other, NOW)
                .unwrap()
                .unwrap()
                .account
                .account,
            "a"
        );
    }

    fn claims_are_released_on_session_end_and_lapse_with_a_dark_station(store: &dyn Store) {
        let registry = store.handle();
        let pool = pool(PoolStrategy::Headroom, &[("only", 0, Some(1))]);
        assert!(claim(registry.as_ref(), &pool, "dark", "m1", NOW).is_some());
        assert!(claim(registry.as_ref(), &pool, "live", "m2", NOW).is_none());
        // The live Station renews its own (it has none here); the dark one renews nothing.
        registry
            .renew_account_claims("live", &["m2".to_owned()], TTL, NOW + 30.0)
            .unwrap();
        assert!(
            claim(registry.as_ref(), &pool, "live", "m2", NOW + 30.0).is_none(),
            "still held"
        );
        assert_eq!(
            claim(registry.as_ref(), &pool, "live", "m2", NOW + 61.0).as_deref(),
            Some("only"),
            "a dark Station's claim lapses with its lease"
        );
        // Renewal keeps the claims of Missions the Station still runs and drops the rest.
        registry
            .renew_account_claims("live", &["m2".to_owned()], TTL, NOW + 100.0)
            .unwrap();
        assert!(claim(registry.as_ref(), &pool, "x", "m3", NOW + 150.0).is_none());
        registry
            .renew_account_claims("live", &[], TTL, NOW + 150.0)
            .unwrap();
        assert!(
            state(registry.as_ref(), "only").active.is_empty(),
            "fenced out"
        );
        assert!(claim(registry.as_ref(), &pool, "x", "m3", NOW + 150.0).is_some());
        assert!(registry.release_account("x", "m3").unwrap());
        assert!(!registry.release_account("x", "m3").unwrap());
        assert!(state(registry.as_ref(), "only").active.is_empty());
    }

    fn quota_reports_and_usage_are_kept_per_account(store: &dyn Store) {
        let registry = store.handle();
        let account = AccountRef::new("team", "a");
        let profile = AccountProfile {
            handler: "claude".to_owned(),
            kind: "claude-oauth-token".to_owned(),
            operations: vec!["G".to_owned()],
            priority: 1,
            max_concurrent: Some(2),
            labels: BTreeMap::new(),
        };
        registry
            .report_quota(
                &account,
                &[
                    QuotaWindow::new("five_hour", NOW).used_permille(100),
                    QuotaWindow::new("weekly", NOW).used_permille(300),
                ],
            )
            .unwrap();
        registry
            .report_quota(
                &account,
                &[QuotaWindow::new("five_hour", NOW + 5.0)
                    .used_permille(400)
                    .status("allowed_warning")],
            )
            .unwrap();
        registry
            .declare_account(&account, "station-a", &profile)
            .unwrap();
        let day = NOW as i64;
        registry.record_account_usage(&account, 1_000, day).unwrap();
        registry.record_account_usage(&account, 500, day).unwrap();
        registry
            .record_account_usage(&AccountRef::new("team", "b"), 7, day)
            .unwrap();
        let a = state(registry.as_ref(), "a");
        assert_eq!(
            a.profile().as_ref(),
            Some(&profile),
            "declaring keeps the rest"
        );
        assert_eq!(a.windows.len(), 2);
        assert_eq!(a.highest_use(NOW), 400);
        assert_eq!(a.windows[0].status.as_deref(), Some("allowed_warning"));
        assert_eq!(a.tokens_on_day(day), 1_500);
        assert_eq!(a.tokens_on_day(day + 86_400), 0);
        assert_eq!(state(registry.as_ref(), "b").tokens_on_day(day), 7);
        assert!(
            registry
                .limit_account(&AccountRef::new("bad name", "a"), NOW)
                .is_err()
        );
    }
}
