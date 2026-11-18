//! Reading what a pool account has left, only when something asks.
//!
//! A reading goes to the account's provider with no model turn: through the harness's account
//! provider for a Claude Code or Codex account (`account_usage::read_limits`, which sends the
//! account's own credential to the provider's own endpoint), else through the handler that
//! serves it ([`AgentHandler::usage`]). Nothing reads on a timer; a read starts from:
//!
//! - a person: a request for the account's usage that the gateway left in the Registry
//!   ([`UsageRequest`](crate::UsageRequest)), which a Station holding the account answers on its
//!   next beat ([`tend`]);
//! - an Agent: the tool of its own Run ([`for_run`]);
//! - an event: a Run about to be given an account, or a route about to be chosen
//!   ([`before_claim`]); a Run ended by a limit ([`after_limit`]); a login renewed
//!   ([`after_renewal`]).
//!
//! A reading stands for `[roster.usage] keep` seconds and one reading at a time is made per
//! account, so a burst of asks, whatever starts them, reaches the provider once. What is read
//! goes to the account's quota in the Registry like what a Run reports
//! ([`window_of`](super::accounts::window_of)), which is what the route chooser and the claim
//! read. For an account the Registry holds limited:
//!
//! - room in every bucket that covers a parked Mission's model frees the account and resumes the
//!   Missions waiting on it at the next scheduler pass (not after a limit a Run just hit);
//! - a reset the provider moved later re-parks them to the new time;
//! - a handler that learns its usage only during a turn (`needs_turn`), or only with a run
//!   open, is never read: its account waits for the time the limit stated.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use coqui_acp_api::{Bucket, Coverage, Refill, Status, UsageReport};
use serde_json::{Value, json};

use super::{DaemonState, accounts, credentials, models, park_mission};
use crate::directorates::m::limits::parked_until;
use crate::directorates::m::mission::MissionState;
use crate::directorates::q::{AccountPool, harness_accounts, usage_standing};
use crate::directorates::registry::accounts::{AccountRef, AccountState};
use crate::directorates::section_00::agent::AgentHandler;

/// What starts a reading, for the Station's log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "cables-mcp"), allow(dead_code))]
pub(super) enum Trigger {
    /// A person asked for the account's usage.
    Request,
    /// An Agent asked, in its own Run.
    Agent,
    /// A Run is about to be given an account, or a route is about to be chosen.
    Claim,
    /// A Run ended at the account's limit.
    Limit,
    /// The account's login was renewed.
    Renewal,
}

impl Trigger {
    const fn name(self) -> &'static str {
        match self {
            Self::Request => "a request",
            Self::Agent => "an Agent",
            Self::Claim => "a claim",
            Self::Limit => "a limit",
            Self::Renewal => "a renewal",
        }
    }
}

/// What a read of one account came to.
#[derive(Clone)]
pub(super) enum Read {
    /// The account's handler reports nothing between turns, or cannot be asked now, and why.
    NotPolled(String),
    /// The handler was asked and failed.
    Failed(String),
    /// What the account's buckets are.
    Report(Box<UsageReport>),
}

/// A read, and when it was made.
#[derive(Clone)]
pub(super) struct Reading {
    pub(super) read: Read,
    /// When it was made, as Unix seconds on the Station's clock.
    pub(super) at: i64,
    /// An earlier ask made it and it was kept.
    pub(super) kept: bool,
}

impl Reading {
    /// How the reading went, for a person: no credential is in it.
    fn result(&self, now: i64) -> String {
        match &self.read {
            Read::Report(_) if self.kept => {
                format!("reading from {}s ago", now.saturating_sub(self.at).max(0))
            }
            Read::Report(_) => "read".to_owned(),
            Read::NotPolled(why) | Read::Failed(why) => format!("not read: {why}"),
        }
    }
}

/// How an account is read in a test: a stand-in for the handler.
#[cfg(test)]
type Reader = Arc<dyn Fn(&AccountState) -> Read + Send + Sync>;

/// The readings this Station keeps and the reading it is making of each account.
#[derive(Default)]
pub(super) struct UsageReads {
    kept: Mutex<BTreeMap<String, (i64, Read)>>,
    /// One lock per account, held while it is read.
    flights: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
    /// Set while a beat's answers run, so the beat never starts a second one beside it.
    running: AtomicBool,
    #[cfg(test)]
    reader: Mutex<Option<Reader>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
impl UsageReads {
    /// Answers every read with `reader` rather than the handler.
    pub(super) fn read_with(&self, reader: impl Fn(&AccountState) -> Read + Send + Sync + 'static) {
        *locked(&self.reader) = Some(Arc::new(reader));
    }
}

/// Whether no pass runs; `true` marks one running until [`end`].
pub(super) fn begin(state: &DaemonState) -> bool {
    state
        .usage_reads
        .running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// Marks the pass [`begin`] started as over.
pub(super) fn end(state: &DaemonState) {
    state.usage_reads.running.store(false, Ordering::Release);
}

/// What a beat of the scheduler does for usage: answers the requests that wait for an account
/// this Station holds, and resumes the Missions parked on an account whose limit another Station
/// lifted. It reads an account only for a request that waits; nothing else starts a reading here.
pub(super) fn tend(state: &DaemonState) {
    let Ok(states) = state.intake.accounts() else {
        return;
    };
    let now = state.clock.now();
    // A limit lifted early, by a read on another Station: the Missions still parked on the
    // account for a time to come are due now.
    for account in states
        .iter()
        .filter(|account| account.limited_until.is_none())
    {
        resume_lifted(state, &account.account, now);
    }
    for account in states.iter().filter(|account| {
        account.profiles.contains_key(&state.station_id)
            && account
                .usage_request
                .as_ref()
                .is_some_and(crate::UsageRequest::waits)
    }) {
        let Some(ask) = &account.usage_request else {
            continue;
        };
        let reading = read(state, &account.account, Trigger::Request);
        let result = reading.as_ref().map_or_else(
            || "not read: this Station does not hold the account".to_owned(),
            |reading| reading.result(state.clock.now()),
        );
        if let Err(error) = usage_standing::answer(
            state.intake.as_ref(),
            &account.account,
            ask.at,
            state.clock.now() as f64,
            &result,
        ) {
            eprintln!(
                "ai6: the request for account {}'s usage was not answered: {error}",
                account.account
            );
        }
    }
}

/// Reads `account` now, unless a reading made within `[roster.usage] keep` seconds stands, and
/// records what it found. `None` when this Station did not declare the account, so it cannot
/// read it. One reading at a time is made per account, and an ask that waits for it takes its
/// answer.
pub(super) fn read(state: &DaemonState, account: &AccountRef, trigger: Trigger) -> Option<Reading> {
    let key = account.to_string();
    let keep = i64::try_from(state.roster.settings().usage.keep.0.as_secs()).unwrap_or(i64::MAX);
    let standing = || {
        let now = state.clock.now();
        locked(&state.usage_reads.kept)
            .get(&key)
            .filter(|(at, _)| keep > 0 && now >= *at && now - *at < keep)
            .map(|(at, read)| Reading {
                read: read.clone(),
                at: *at,
                kept: true,
            })
    };
    if let Some(reading) = standing() {
        return Some(reading);
    }
    let flight = locked(&state.usage_reads.flights)
        .entry(key.clone())
        .or_default()
        .clone();
    let _making = locked(&flight);
    // Another ask made the reading while this one waited for the account.
    if let Some(reading) = standing() {
        return Some(reading);
    }
    let held =
        state.intake.accounts().ok()?.into_iter().find(|held| {
            held.account == *account && held.profiles.contains_key(&state.station_id)
        })?;
    #[cfg(test)]
    let stand_in = locked(&state.usage_reads.reader).clone();
    #[cfg(test)]
    let read = match stand_in {
        Some(reader) => reader(&held),
        None => read_account(state, &held),
    };
    #[cfg(not(test))]
    let read = read_account(state, &held);
    let now = state.clock.now();
    locked(&state.usage_reads.kept).insert(key, (now, read.clone()));
    match &read {
        Read::Report(report) => apply(state, &held, report, now, trigger),
        Read::Failed(why) => eprintln!(
            "ai6: usage of account {account} was not read, asked by {}: {why}",
            trigger.name()
        ),
        Read::NotPolled(_) => {}
    }
    Some(Reading {
        read,
        at: now,
        kept: false,
    })
}

/// A Run is about to be given an account of `pools`, or a route that draws on them is about to
/// be chosen: reads each account the choice may fall on, a few at a time, so the choice is made
/// on what the providers say now. An account read within `keep` seconds is not read again.
pub(super) fn before_claim(state: &DaemonState, pools: &[&AccountPool]) {
    const TOGETHER: usize = 4;
    let accounts: Vec<AccountRef> = pools
        .iter()
        .flat_map(|pool| {
            pool.accounts()
                .map(|account| AccountRef::new(pool.id(), account.name()))
        })
        .collect();
    for group in accounts.chunks(TOGETHER) {
        std::thread::scope(|scope| {
            for account in group {
                scope.spawn(|| read(state, account, Trigger::Claim));
            }
        });
    }
}

/// The Run that held `account` ended at its provider's limit: reads it, so what the provider says
/// of its buckets and their reset is on the board before another account or route is chosen.
/// It lifts nothing: a provider that just refused the account is believed over a reading.
pub(super) fn after_limit(state: &DaemonState, account: &AccountRef) {
    read(state, account, Trigger::Limit);
}

/// The login of `account` was renewed: reads its usage, since a renewed login may be a changed
/// plan.
pub(super) fn after_renewal(state: &DaemonState, account: &AccountRef) {
    read(state, account, Trigger::Renewal);
}

/// The Run `mission_id`'s own account, read now for the Run's Agent: its pool and each window
/// with how much is used, when it resets and the models it covers, and how the reading went. No
/// account name and no credential is in it.
///
/// # Errors
///
/// Why there is nothing to say: the Run holds no pool account, or this Station cannot read it.
#[cfg_attr(not(feature = "cables-mcp"), allow(dead_code))]
pub(super) fn for_run(state: &DaemonState, mission_id: &str) -> Result<Value, String> {
    let account = accounts::held(state, mission_id)
        .ok_or("this Run holds no pool account, so there is no quota to read")?;
    let reading = read(state, &account, Trigger::Agent)
        .ok_or("this Station does not hold the account, so it cannot read it")?;
    let now = state.clock.now();
    let held = state
        .intake
        .accounts()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|held| held.account == account)
        .ok_or("the account is not in the Registry")?;
    Ok(json!({
        "pool": account.pool,
        "read": reading.result(now),
        "read_at": reading.at,
        "limited_until": held.limited_until.filter(|until| *until > now as f64),
        "windows": held.windows.iter().map(|window| json!({
            "name": window.name,
            "used_permille": window.used_permille,
            "resets_at": window.resets_at,
            "status": window.status,
            "models": window.models,
            "observed_at": window.observed_at,
        })).collect::<Vec<_>>(),
    }))
}

/// Records what a reading found, and settles a limited account by it.
fn apply(
    state: &DaemonState,
    account: &AccountState,
    report: &UsageReport,
    now: i64,
    trigger: Trigger,
) {
    if !report.buckets.is_empty() {
        let observed = now as f64;
        let windows: Vec<_> = report
            .buckets
            .iter()
            .map(|bucket| accounts::window_of(bucket, observed))
            .collect();
        if let Err(error) = state.intake.report_quota(&account.account, &windows) {
            eprintln!(
                "ai6: quota of account {} not recorded: {error}",
                account.account
            );
        }
    }
    if account.is_limited(now as f64) {
        settle(state, account, report, now, trigger != Trigger::Limit);
    }
}

/// Why `handler` is not asked to read an account between turns, when it is not: it learns its
/// usage only during a turn, or with a run open, or reports none. Asking costs no credential.
pub(super) fn unreadable(handler: &dyn AgentHandler) -> Option<Read> {
    let support = match handler.usage_support() {
        Ok(support) => support,
        Err(error) => return Some(Read::Failed(error.to_string())),
    };
    if !support.reports() {
        return Some(Read::NotPolled("this handler reports no usage".to_owned()));
    }
    if support.needs_turn {
        return Some(Read::NotPolled(
            "this handler learns its usage only during a turn".to_owned(),
        ));
    }
    if support.needs_session {
        return Some(Read::NotPolled(
            "this handler learns its usage only with a run open".to_owned(),
        ));
    }
    None
}

/// Asks `handler` for the usage of the account behind `environment`, unless [`unreadable`].
pub(super) fn read_through(
    handler: &dyn AgentHandler,
    environment: BTreeMap<String, String>,
) -> Read {
    if let Some(skipped) = unreadable(handler) {
        return skipped;
    }
    from_report(
        handler
            .usage(environment)
            .map_err(|error| error.to_string()),
    )
}

/// What a report, or a refusal to give one, comes to: an account with no plan limits is not read.
fn from_report(answer: Result<UsageReport, String>) -> Read {
    match answer {
        Ok(report) if report.supported => Read::Report(Box::new(report)),
        Ok(report) => Read::NotPolled(
            report
                .unsupported_because
                .unwrap_or_else(|| "this account reports no usage".to_owned()),
        ),
        Err(why) => Read::Failed(why),
    }
}

/// Reads `account` through its pool's handler, with its credential leased for the read alone.
fn read_account(state: &DaemonState, account: &AccountState) -> Read {
    let Some(profile) = account.profile() else {
        return Read::NotPolled("no Station declared this account".to_owned());
    };
    let Some((catalog, broker)) = credentials::listing_broker(state) else {
        return Read::NotPolled("this Station has no loadouts to lease its credential".to_owned());
    };
    let Some(pool) = catalog
        .pools()
        .into_iter()
        .find(|pool| pool.id() == account.account.pool)
    else {
        return Read::NotPolled("this Station does not serve the pool".to_owned());
    };
    let Some(member) = pool.get(&account.account.account) else {
        return Read::NotPolled("this Station does not serve the account".to_owned());
    };
    let holder = format!("usage-{}", account.account);
    let handler = match state
        .plugins
        .handler_with(&profile.handler, &serde_json::Map::new())
    {
        Ok(handler) => handler,
        Err(_) => return Read::NotPolled("this handler is not on this Station".to_owned()),
    };
    // Claude Code's and Codex's account providers read the plan limits with the account's own
    // credential, which a handler cannot (Claude Code reports them only in a turn). Another
    // handler, or an account the provider cannot be asked from here, is read by the handler.
    let by_provider = harness_accounts::reads_limits(&profile.handler, member.kind());
    let by_handler = unreadable(handler.as_ref());
    // Only a read that will happen leases the credential.
    if let Some(skipped) = &by_handler
        && !by_provider
    {
        return skipped.clone();
    }
    match models::with_account(state, &broker, pool, member, &holder, |environment| {
        let mut provider = None;
        if by_provider
            && let Some(answer) = harness_accounts::read_limits(
                &profile.handler,
                pool.id(),
                member.name(),
                member.kind(),
                &environment,
                None,
            )
        {
            match from_report(answer) {
                found @ Read::Report(_) => return found,
                other => provider = Some(other),
            }
        }
        match by_handler {
            None => read_through(handler.as_ref(), environment),
            Some(skipped) => provider.unwrap_or(skipped),
        }
    }) {
        Ok(read) => read,
        Err(reason) => Read::Failed(reason),
    }
}

/// What a report says of the room an account has for the models that wait on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Verdict {
    /// Nothing that covers them is used up.
    Room,
    /// Something is, until `resets_at` when the report says when the last of it refills.
    UsedUp { resets_at: Option<i64> },
    /// The report holds no bucket to judge by.
    Unknown,
}

/// Whether `bucket` stops work at `now`: the harness says it is exhausted, or it is used whole;
/// a window that has refilled by `now` stops nothing.
fn stops(bucket: &Bucket, now: i64) -> bool {
    if bucket.resets_in(now).is_some_and(|left| left.is_zero()) {
        return false;
    }
    bucket.status == Status::Exhausted || UsageReport::fraction_permille(bucket) == Some(1000)
}

/// Whether `bucket` covers `model`: a bucket of unknown coverage covers every model, and a
/// Mission that names none is held by every bucket.
fn covers(bucket: &Bucket, model: Option<&str>) -> bool {
    match (&bucket.models, model) {
        (Coverage::Only { models }, Some(model)) => models.iter().any(|named| named == model),
        _ => true,
    }
}

/// Judges `report` at `now` for Missions needing `models`.
pub(super) fn verdict(report: &UsageReport, models: &[Option<String>], now: i64) -> Verdict {
    if report.buckets.is_empty() {
        return Verdict::Unknown;
    }
    let wanted: Vec<Option<&str>> = if models.is_empty() {
        vec![None]
    } else {
        models.iter().map(Option::as_deref).collect()
    };
    let stopping: Vec<&Bucket> = report
        .buckets
        .iter()
        .filter(|bucket| wanted.iter().any(|model| covers(bucket, *model)) && stops(bucket, now))
        .collect();
    if stopping.is_empty() {
        return Verdict::Room;
    }
    let resets_at = stopping
        .iter()
        .map(|bucket| match &bucket.refill {
            Refill::FixedWindow { resets_at, .. } => *resets_at,
            _ => None,
        })
        .try_fold(i64::MIN, |latest, at| at.map(|at| latest.max(at)));
    Verdict::UsedUp {
        resets_at: resets_at.filter(|at| *at != i64::MIN),
    }
}

/// Settles a limited `account` by what `report` says of its room: frees it and resumes the
/// Missions parked on it when nothing that covers their models is used up and `may_free`, and
/// moves their wait when the provider now states a later reset.
fn settle(
    state: &DaemonState,
    account: &AccountState,
    report: &UsageReport,
    now: i64,
    may_free: bool,
) {
    let grace =
        i64::try_from(state.station.limits().accounts.reset_grace.0.as_secs()).unwrap_or(i64::MAX);
    let until = account.limited_until.map_or(now, |until| until as i64);
    let waiting = parked_on(state, &account.account);
    let models: Vec<Option<String>> = waiting.iter().map(|(_, model)| model.clone()).collect();
    match verdict(report, &models, now) {
        Verdict::Room if may_free => free(state, account, &waiting, now),
        Verdict::UsedUp { resets_at } => {
            if let Some(reset) = resets_at.filter(|reset| (*reset as f64) > until as f64) {
                move_later(state, account, &waiting, reset, grace, now);
            }
        }
        Verdict::Room | Verdict::Unknown => {}
    }
}

/// The Missions of this Station's book parked on `account` for a time to come, with the model
/// each needs room for. A Mission that has left `Parked` names nothing.
fn parked_on(state: &DaemonState, account: &AccountRef) -> Vec<(String, Option<String>)> {
    let key = account.to_string();
    let now = state.clock.now();
    state
        .missions
        .lock()
        .map(|missions| {
            missions
                .list()
                .into_iter()
                .filter(|mission| {
                    mission.state == MissionState::Parked
                        && mission.parked_until.is_some_and(|until| until > now)
                })
                .filter_map(|mission| {
                    let on = mission.parked_on?;
                    (on.account == key).then(|| (mission.mission_id.to_string(), on.model))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Resumes, at the next scheduler pass, the Missions parked on `account` whose limit is gone.
fn resume_lifted(state: &DaemonState, account: &AccountRef, now: i64) {
    for (mission_id, _) in parked_on(state, account) {
        if park_mission(state, &mission_id, now) {
            eprintln!(
                "ai6: account {account} is no longer limited; Mission {mission_id:?} resumes"
            );
        }
    }
}

/// Lifts the account's limit and resumes the Missions parked on it at the next scheduler pass.
fn free(
    state: &DaemonState,
    account: &AccountState,
    waiting: &[(String, Option<String>)],
    now: i64,
) {
    if let Err(error) = state.intake.clear_account_limit(&account.account) {
        eprintln!(
            "ai6: the limit of account {} was not lifted: {error}",
            account.account
        );
        return;
    }
    for (mission_id, _) in waiting {
        if is_parked(state, mission_id) && park_mission(state, mission_id, now) {
            eprintln!(
                "ai6: account {} has room again; Mission {mission_id:?} resumes",
                account.account
            );
        }
    }
}

/// Keeps the account out, and its parked Missions parked, until the later reset the provider now
/// states.
fn move_later(
    state: &DaemonState,
    account: &AccountState,
    waiting: &[(String, Option<String>)],
    reset: i64,
    grace: i64,
    now: i64,
) {
    if let Err(error) = state.intake.limit_account(&account.account, reset as f64) {
        eprintln!("ai6: account {} not kept limited: {error}", account.account);
    }
    let until = parked_until(now, Some(reset), None, 0, grace);
    for (mission_id, _) in waiting {
        let current = state.missions.lock().ok().and_then(|missions| {
            missions
                .get(mission_id)
                .and_then(|mission| mission.parked_until)
        });
        if is_parked(state, mission_id) && current.is_some_and(|current| current < until) {
            park_mission(state, mission_id, until);
        }
    }
}

fn is_parked(state: &DaemonState, mission_id: &str) -> bool {
    state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).map(|mission| mission.state))
        == Some(MissionState::Parked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::m::identity::MissionId;
    use crate::directorates::m::mission::ParkedOn;
    use crate::directorates::q::{Account, AccountKind};
    use crate::directorates::registry::accounts::AccountProfile;
    use coqui_acp_api::UsageSupport;
    use guatiao::library::ProviderError;
    use std::sync::atomic::{AtomicI64, AtomicUsize};
    use std::time::Duration;

    use super::super::tests::limited_state;
    use crate::directorates::q::terminal::AgentTerminal;
    use crate::directorates::section_00::agent::{AgentResume, AgentStart};
    use guatiao::library::Object;

    const START: i64 = 1_000_000;
    const STATED_RESET: i64 = START + 18_000;

    fn bucket(id: &str, used: u64, resets_at: i64, models: Coverage) -> Bucket {
        let mut bucket = Bucket::new(id);
        bucket.limit = Some(1000);
        bucket.used = Some(used);
        bucket.models = models;
        bucket.refill = Refill::FixedWindow {
            length: None,
            resets_at: Some(resets_at),
        };
        bucket
    }

    fn reported(buckets: Vec<Bucket>) -> UsageReport {
        let mut report = UsageReport::new();
        report.buckets = buckets;
        report
    }

    fn boxed(buckets: Vec<Bucket>) -> Box<UsageReport> {
        Box::new(reported(buckets))
    }

    fn profile() -> AccountProfile {
        AccountProfile {
            handler: "mock-limit".to_owned(),
            kind: "claude-oauth-token".to_owned(),
            operations: vec!["G".to_owned()],
            priority: 0,
            max_concurrent: None,
            labels: BTreeMap::new(),
            sealed_login: false,
        }
    }

    fn declared_by(state: &DaemonState, account: &AccountRef) {
        state
            .intake
            .declare_account(account, &state.station_id, &profile())
            .unwrap();
    }

    /// A state at `START` with one account of a pool, declared by this Station and limited until
    /// `STATED_RESET`, and a Mission parked on it until `STATED_RESET`.
    fn limited_account(
        root: &std::path::Path,
        usage: &str,
    ) -> (DaemonState, Arc<AtomicI64>, AccountRef) {
        let (state, now) = limited_state(
            root,
            usage,
            "[agent.007]\nhandler = \"mock-limit\"\n",
            START,
        );
        let account = AccountRef::new("pool", "one");
        declared_by(&state, &account);
        state
            .intake
            .limit_account(&account, STATED_RESET as f64)
            .unwrap();
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new("waits").unwrap(),
                Some("G".to_owned()),
                "work".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        state
            .missions
            .lock()
            .unwrap()
            .transition("waits", MissionState::Working)
            .unwrap();
        state
            .missions
            .lock()
            .unwrap()
            .park_on(
                "waits",
                STATED_RESET,
                Some(ParkedOn::new("pool/one", Some("model-a".to_owned()))),
            )
            .unwrap();
        (state, now, account)
    }

    /// The roster file with `usage` appended to the Operation's table, read again.
    fn retune(state: &mut DaemonState, root: &std::path::Path, usage: &str) {
        let checkout = root.join("checkout");
        std::fs::write(
            root.join("operations.toml"),
            format!(
                "[operation.G]\npath = {:?}\n{usage}",
                checkout.to_string_lossy()
            ),
        )
        .unwrap();
        state.roster = Arc::new(crate::directorates::m::roster::Roster::load(root).unwrap());
    }

    fn account_state(state: &DaemonState, account: &AccountRef) -> AccountState {
        state
            .intake
            .accounts()
            .unwrap()
            .into_iter()
            .find(|held| held.account == *account)
            .unwrap()
    }

    fn parked_until_of(state: &DaemonState) -> Option<i64> {
        state
            .missions
            .lock()
            .unwrap()
            .get("waits")
            .unwrap()
            .parked_until
    }

    /// Makes every read of `state` answer `buckets`, and counts them.
    fn counting(state: &DaemonState, buckets: Vec<Bucket>) -> Arc<AtomicUsize> {
        let reads = Arc::new(AtomicUsize::new(0));
        let seen = reads.clone();
        state.usage_reads.read_with(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Read::Report(boxed(buckets.clone()))
        });
        reads
    }

    fn room_for_model_a() -> Vec<Bucket> {
        vec![bucket(
            "mine",
            100,
            STATED_RESET,
            Coverage::Only {
                models: vec!["model-a".to_owned()],
            },
        )]
    }

    fn reads_of(counted: &Arc<AtomicUsize>) -> usize {
        counted.load(Ordering::SeqCst)
    }

    #[test]
    fn a_person_s_request_is_answered_by_one_reading_that_reaches_the_board() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let reads = counting(&state, room_for_model_a());
        usage_standing::request(state.intake.as_ref(), &account, "person", START as f64).unwrap();
        assert_eq!(reads_of(&reads), 0, "asking reads nothing by itself");
        tend(&state);
        assert_eq!(reads_of(&reads), 1);
        let held = account_state(&state, &account);
        let ask = held.usage_request.as_ref().unwrap();
        assert!(!ask.waits());
        assert_eq!(ask.result.as_deref(), Some("read"));
        assert_eq!(held.windows.len(), 1, "what was read is on the board");
        assert_eq!(held.limited_until, None, "room frees the limited account");
        assert_eq!(
            parked_until_of(&state),
            Some(START),
            "and its Mission is due"
        );
        // The same person asks again at once: the reading stands, nothing more is read.
        usage_standing::request(
            state.intake.as_ref(),
            &account,
            "person",
            START as f64 + 0.5,
        )
        .unwrap();
        tend(&state);
        assert_eq!(reads_of(&reads), 1);
        let ask = account_state(&state, &account).usage_request.unwrap();
        assert!(!ask.waits());
        assert_eq!(ask.result.as_deref(), Some("reading from 0s ago"));
    }

    #[test]
    fn a_request_made_twice_before_a_station_answers_is_one_request() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let first =
            usage_standing::request(state.intake.as_ref(), &account, "one", START as f64).unwrap();
        let second =
            usage_standing::request(state.intake.as_ref(), &account, "two", START as f64 + 9.0)
                .unwrap();
        assert_eq!(first, second, "the waiting request is kept as it is");
    }

    #[test]
    fn a_request_waits_for_a_station_that_holds_the_account() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let reads = counting(&state, room_for_model_a());
        let other = DaemonState {
            station_id: "other".to_owned(),
            missions: Arc::new(Mutex::new(Default::default())),
            usage_reads: Default::default(),
            ..state.clone()
        };
        let others = counting(&other, room_for_model_a());
        usage_standing::request(state.intake.as_ref(), &account, "person", START as f64).unwrap();
        tend(&other);
        assert_eq!(reads_of(&others), 0, "it never declared the account");
        assert!(
            account_state(&state, &account)
                .usage_request
                .unwrap()
                .waits()
        );
        tend(&state);
        assert_eq!(reads_of(&reads), 1);
    }

    #[test]
    fn an_account_that_cannot_be_read_is_answered_with_why() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        usage_standing::request(state.intake.as_ref(), &account, "person", START as f64).unwrap();
        // No stand-in: the real path, which finds this test Station has no loadouts.
        tend(&state);
        let ask = account_state(&state, &account).usage_request.unwrap();
        assert!(!ask.waits());
        let result = ask.result.unwrap();
        assert!(result.starts_with("not read: "), "{result}");
    }

    #[test]
    fn asks_in_a_burst_reach_the_provider_once_whatever_started_them() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let reads = Arc::new(AtomicUsize::new(0));
        let seen = reads.clone();
        state.usage_reads.read_with(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(150));
            Read::Report(boxed(room_for_model_a()))
        });
        let triggers = [
            Trigger::Request,
            Trigger::Agent,
            Trigger::Claim,
            Trigger::Limit,
            Trigger::Renewal,
            Trigger::Agent,
        ];
        let answers: Vec<Option<Reading>> = std::thread::scope(|scope| {
            let asking: Vec<_> = triggers
                .iter()
                .map(|trigger| scope.spawn(|| read(&state, &account, *trigger)))
                .collect();
            asking
                .into_iter()
                .map(|each| each.join().unwrap())
                .collect()
        });
        assert_eq!(reads_of(&reads), 1, "six asks at once, one request");
        assert!(answers.iter().all(|answer| answer.is_some()));
        assert_eq!(
            answers
                .iter()
                .flatten()
                .filter(|reading| !reading.kept)
                .count(),
            1,
            "one made the reading and the rest took it"
        );
    }

    #[test]
    fn a_reading_stands_for_keep_seconds_and_a_keep_of_zero_keeps_none() {
        let root = tempfile::tempdir().unwrap();
        let (mut state, now, account) = limited_account(root.path(), "");
        let reads = counting(&state, room_for_model_a());
        read(&state, &account, Trigger::Agent).unwrap();
        now.store(START + 59, Ordering::SeqCst);
        read(&state, &account, Trigger::Agent).unwrap();
        assert_eq!(reads_of(&reads), 1, "59 s into the 60 s a reading stands");
        now.store(START + 60, Ordering::SeqCst);
        read(&state, &account, Trigger::Agent).unwrap();
        assert_eq!(reads_of(&reads), 2);
        // The setting is read at every ask.
        retune(&mut state, root.path(), "\n[roster.usage]\nkeep = 0\n");
        state.usage_reads.read_with({
            let seen = reads.clone();
            move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
                Read::Report(boxed(room_for_model_a()))
            }
        });
        read(&state, &account, Trigger::Agent).unwrap();
        read(&state, &account, Trigger::Agent).unwrap();
        assert_eq!(reads_of(&reads), 4, "a keep of 0 reads at every ask");
    }

    #[test]
    fn nothing_reads_on_a_timer() {
        let root = tempfile::tempdir().unwrap();
        let (state, now, account) = limited_account(root.path(), "");
        let reads = counting(&state, room_for_model_a());
        // Days of beats of the scheduler with the clock running: the account is limited and a
        // Mission waits on it, which a timer would have read.
        for hour in 1..=72 {
            now.store(START + hour * 3_600, Ordering::SeqCst);
            tend(&state);
        }
        assert_eq!(reads_of(&reads), 0, "no ask, no read");
        assert!(account_state(&state, &account).limited_until.is_some());
        assert_eq!(parked_until_of(&state), Some(STATED_RESET));
        assert!(account_state(&state, &account).windows.is_empty());
    }

    #[test]
    fn an_agent_reads_its_own_account_once_and_is_told_its_windows_without_a_name_or_credential() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let reads = counting(&state, room_for_model_a());
        assert!(
            for_run(&state, "run-1")
                .unwrap_err()
                .contains("no pool account"),
            "a Run that holds none is told so"
        );
        super::super::accounts::testing::hold(&state, "run-1", account.clone());
        let first = for_run(&state, "run-1").unwrap();
        let second = for_run(&state, "run-1").unwrap();
        assert_eq!(reads_of(&reads), 1, "the second ask is the same reading");
        assert_eq!(first["pool"], "pool");
        assert_eq!(first["read"], "read");
        assert_eq!(second["read"], "reading from 0s ago");
        assert_eq!(first["windows"].as_array().unwrap().len(), 1);
        assert_eq!(first["windows"][0]["name"], "mine");
        let text = first.to_string();
        assert!(
            !text.contains("pool/one") && first.get("account").is_none(),
            "{text}"
        );
    }

    #[test]
    fn a_claim_reads_each_account_of_its_pools_and_the_chooser_sees_the_readings() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now) = limited_state(
            root.path(),
            "",
            "[agent.007]\nhandler = \"mock-limit\"\n",
            START,
        );
        let pool = AccountPool::new("pool", "mock-limit")
            .account(Account::new("one", AccountKind::ClaudeOauthToken))
            .account(Account::new("two", AccountKind::ClaudeOauthToken));
        for name in ["one", "two"] {
            declared_by(&state, &AccountRef::new("pool", name));
        }
        let reads = Arc::new(AtomicUsize::new(0));
        let seen = reads.clone();
        state.usage_reads.read_with(move |held| {
            seen.fetch_add(1, Ordering::SeqCst);
            let used = if held.account.account == "one" {
                900
            } else {
                100
            };
            Read::Report(boxed(vec![bucket(
                "five_hour",
                used,
                STATED_RESET,
                Coverage::All,
            )]))
        });
        let station = state.station_id.clone();
        let choose = || {
            let states = state.intake.accounts().unwrap();
            super::super::accounts::best_account(&[&pool], &states, START as f64, 500, &station)
                .unwrap()
                .account
                .account
        };
        let before = choose();
        before_claim(&state, &[&pool]);
        assert_eq!(reads_of(&reads), 2, "each account once");
        assert_eq!(
            choose(),
            "two",
            "the account with room, as the providers say now"
        );
        assert_eq!(
            account_state(&state, &AccountRef::new("pool", "one")).highest_use(START as f64, 500),
            900
        );
        before_claim(&state, &[&pool]);
        assert_eq!(reads_of(&reads), 2, "a second claim at once reads nothing");
        let _ = before;
    }

    #[test]
    fn a_run_ended_by_a_limit_reads_the_account_and_never_lifts_the_limit_it_just_hit() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let reads = counting(&state, room_for_model_a());
        after_limit(&state, &account);
        assert_eq!(reads_of(&reads), 1);
        let held = account_state(&state, &account);
        assert_eq!(held.windows.len(), 1, "the reading is recorded");
        assert_eq!(
            held.limited_until,
            Some(STATED_RESET as f64),
            "a provider that just refused the account is believed over a reading"
        );
        assert_eq!(parked_until_of(&state), Some(STATED_RESET));
    }

    #[test]
    fn a_limit_that_moved_later_is_kept_when_a_run_ends_at_it() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let moved = STATED_RESET + 7_200;
        counting(&state, vec![bucket("mine", 1000, moved, Coverage::All)]);
        after_limit(&state, &account);
        assert_eq!(
            account_state(&state, &account).limited_until,
            Some(moved as f64)
        );
        let grace = state.station.limits().accounts.reset_grace.0.as_secs() as i64;
        assert_eq!(parked_until_of(&state), Some(moved + grace));
    }

    #[test]
    fn the_account_a_run_hit_a_limit_on_is_read_by_the_limit_handler() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let reads = counting(&state, room_for_model_a());
        super::super::accounts::testing::hold(&state, "waits", account.clone());
        let mission = state.missions.lock().unwrap().get("waits").unwrap().clone();
        let _ = super::super::accounts::on_limit(&state, &mission, None, 60);
        assert_eq!(
            reads_of(&reads),
            1,
            "a Run ending at a limit reads its account once"
        );
    }

    #[test]
    fn a_renewed_login_reads_the_account_and_no_other_notice_does() {
        use crate::directorates::q::logins::{LoginNotice, NoticeKind};
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let reads = counting(&state, room_for_model_a());
        let notice = |kind| LoginNotice {
            account: account.clone(),
            kind,
            text: "a login".to_owned(),
        };
        super::super::logins::tell(&state, &notice(NoticeKind::Unchanged));
        super::super::logins::tell(&state, &notice(NoticeKind::Failed));
        assert_eq!(reads_of(&reads), 0);
        super::super::logins::tell(&state, &notice(NoticeKind::Renewed));
        assert_eq!(reads_of(&reads), 1);
    }

    #[test]
    fn a_reading_of_room_in_the_models_bucket_frees_the_account_for_an_ask_and_not_for_a_limit() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        counting(
            &state,
            vec![
                bucket(
                    "other",
                    1000,
                    STATED_RESET,
                    Coverage::Only {
                        models: vec!["model-b".to_owned()],
                    },
                ),
                bucket(
                    "mine",
                    400,
                    STATED_RESET,
                    Coverage::Only {
                        models: vec!["model-a".to_owned()],
                    },
                ),
            ],
        );
        read(&state, &account, Trigger::Agent).unwrap();
        let freed = account_state(&state, &account);
        assert_eq!(
            freed.limited_until, None,
            "a used-up bucket of another model holds nothing"
        );
        assert_eq!(freed.windows.len(), 2);
        assert_eq!(parked_until_of(&state), Some(START));
    }

    #[test]
    fn a_limit_another_station_lifted_resumes_this_stations_mission_at_its_next_beat() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let reads = counting(&state, room_for_model_a());
        state.intake.clear_account_limit(&account).unwrap();
        tend(&state);
        assert_eq!(reads_of(&reads), 0, "resuming reads nothing");
        assert_eq!(parked_until_of(&state), Some(START));
    }

    #[test]
    fn a_limit_that_ran_out_by_itself_does_not_resume_a_mission_before_its_grace() {
        let root = tempfile::tempdir().unwrap();
        let (state, now, account) = limited_account(root.path(), "");
        now.store(STATED_RESET + 1, Ordering::SeqCst);
        tend(&state);
        assert!(account_state(&state, &account).limited_until.is_some());
        assert_eq!(parked_until_of(&state), Some(STATED_RESET));
    }

    #[test]
    fn a_mission_that_left_parked_no_longer_counts_toward_the_next_limit() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        assert_eq!(parked_on(&state, &account).len(), 1);
        state
            .missions
            .lock()
            .unwrap()
            .transition("waits", MissionState::Abandoned)
            .unwrap();
        let left = state.missions.lock().unwrap().get("waits").unwrap().clone();
        assert_eq!(
            left.parked_on, None,
            "it names nothing once it leaves Parked"
        );
        assert!(parked_on(&state, &account).is_empty());
        // Its model does not hold the account's next limit: a bucket of another model that is
        // used up is no reason to keep waiting, and nothing waits at all.
        counting(&state, room_for_model_a());
        read(&state, &account, Trigger::Agent).unwrap();
        assert_eq!(account_state(&state, &account).limited_until, None);
    }

    #[test]
    fn a_bucket_of_another_model_that_is_used_up_does_not_hold_the_mission() {
        let report = reported(vec![
            bucket(
                "other",
                1000,
                STATED_RESET,
                Coverage::Only {
                    models: vec!["model-b".to_owned()],
                },
            ),
            bucket(
                "mine",
                400,
                STATED_RESET,
                Coverage::Only {
                    models: vec!["model-a".to_owned()],
                },
            ),
        ]);
        assert_eq!(
            verdict(&report, &[Some("model-a".to_owned())], START),
            Verdict::Room
        );
        assert_eq!(
            verdict(&report, &[Some("model-b".to_owned())], START),
            Verdict::UsedUp {
                resets_at: Some(STATED_RESET)
            }
        );
        assert_eq!(verdict(&UsageReport::new(), &[], START), Verdict::Unknown);
        let mut unknown_coverage = bucket("all", 1000, STATED_RESET, Coverage::Unknown);
        unknown_coverage.status = Status::Exhausted;
        assert_eq!(
            verdict(
                &reported(vec![unknown_coverage]),
                &[Some("model-a".to_owned())],
                START
            ),
            Verdict::UsedUp {
                resets_at: Some(STATED_RESET)
            },
            "an exhausted bucket of unknown coverage stops every model"
        );
        let refilled = bucket("old", 1000, START - 1, Coverage::All);
        assert_eq!(
            verdict(&reported(vec![refilled]), &[], START),
            Verdict::Room
        );
    }

    #[derive(Default)]
    struct Counting {
        support: UsageSupport,
        reads: AtomicUsize,
    }

    impl AgentHandler for Counting {
        fn start(
            &self,
            _request: AgentStart,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn crate::AgentSession>, ProviderError> {
            Err(ProviderError::new(
                guatiao::Status::GUATIAO_ERR_UNSUPPORTED,
                "no",
            ))
        }

        fn resume(
            &self,
            _request: AgentResume,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn crate::AgentSession>, ProviderError> {
            Err(ProviderError::new(
                guatiao::Status::GUATIAO_ERR_UNSUPPORTED,
                "no",
            ))
        }

        fn usage_support(&self) -> Result<UsageSupport, ProviderError> {
            Ok(self.support.clone())
        }

        fn usage(&self, _: BTreeMap<String, String>) -> Result<UsageReport, ProviderError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(reported(vec![bucket(
                "b",
                100,
                STATED_RESET,
                Coverage::All,
            )]))
        }
    }

    #[test]
    fn a_handler_that_needs_a_turn_or_a_run_or_reports_nothing_is_not_asked_to_read() {
        use coqui_acp_api::Source;
        let reporting = UsageSupport::new(vec![Source::Reported], true);
        for support in [
            reporting.clone().needs_turn(true),
            reporting.clone().needs_session(true),
            UsageSupport::none(),
        ] {
            let handler = Counting {
                support,
                ..Counting::default()
            };
            assert!(matches!(
                read_through(&handler, BTreeMap::new()),
                Read::NotPolled(_)
            ));
            assert_eq!(handler.reads.load(Ordering::SeqCst), 0);
        }
        let handler = Counting {
            support: reporting,
            ..Counting::default()
        };
        assert!(matches!(
            read_through(&handler, BTreeMap::new()),
            Read::Report(_)
        ));
        assert_eq!(handler.reads.load(Ordering::SeqCst), 1);
    }
}
