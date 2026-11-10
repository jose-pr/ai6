//! Re-reading what a limited account has left, so its Missions do not wait out a reset time
//! that was a guess.
//!
//! A provider's reset time or a 429's `retry-after` only says when to look first: a limit can
//! lift early, or move later. Each pass reads every account this Station declared that the
//! Registry holds limited, with no model turn, by the handler that serves it
//! ([`AgentHandler::usage`]), every `[roster.usage] refresh_every` seconds and backing off
//! while the account stays used up:
//!
//! - room in every bucket that covers a parked Mission's model frees the account and resumes the
//!   Missions waiting on it at the next scheduler pass;
//! - a reset the provider moved later re-parks them to the new time;
//! - a handler that learns its usage only during a turn (`needs_turn`), or only with a session
//!   open, is never read: its account waits for the time the limit stated.
//!
//! The settings are the `usage` table of the `roster` record, read on every pass, so a change to
//! it applies at once.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use coqui_acp_api::{Bucket, Coverage, Refill, Status, UsageReport};

use super::{DaemonState, accounts, credentials, models, park_mission};
use crate::directorates::m::limits::parked_until;
use crate::directorates::m::mission::MissionState;
use crate::directorates::registry::accounts::{AccountRef, AccountState};
use crate::directorates::section_00::agent::AgentHandler;

/// What a read of one account came to.
pub(super) enum Read {
    /// The account's handler reports nothing between turns, or cannot be asked now, and why.
    NotPolled(String),
    /// The handler was asked and failed.
    Failed(String),
    /// What the account's buckets are.
    Report(UsageReport),
}

/// A Mission parked on an account, and the model it needs room for.
struct Waiting {
    account: AccountRef,
    model: Option<String>,
}

/// When an account is read next, and how often it was found used up in a row.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Schedule {
    next: i64,
    exhausted_reads: u32,
}

#[derive(Default)]
struct Inner {
    waiting: BTreeMap<String, Waiting>,
    schedules: BTreeMap<String, Schedule>,
}

/// The Station's memory of limited accounts: the Missions parked on each, and when each is read
/// next.
#[derive(Default)]
pub(super) struct UsageWatch {
    inner: Mutex<Inner>,
    /// Set while a pass runs, so the beat never starts a second one beside it.
    running: AtomicBool,
}

impl UsageWatch {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Notes that `mission_id` waits on the limited `account`, needing room for `model` (any
    /// model when none is named).
    pub(super) fn hold(&self, account: &AccountRef, mission_id: &str, model: Option<String>) {
        self.lock().waiting.insert(
            mission_id.to_owned(),
            Waiting {
                account: account.clone(),
                model,
            },
        );
    }

    /// The models of the Missions waiting on `account`.
    fn needs_of(&self, account: &AccountRef) -> Vec<(String, Option<String>)> {
        self.lock()
            .waiting
            .iter()
            .filter(|(_, waiting)| waiting.account == *account)
            .map(|(id, waiting)| (id.clone(), waiting.model.clone()))
            .collect()
    }

    fn forget_waiting(&self, mission_ids: &[String]) {
        let mut inner = self.lock();
        for id in mission_ids {
            inner.waiting.remove(id);
        }
    }
}

/// Whether a pass is due and none runs; `true` marks one running until [`end`].
pub(super) fn begin(state: &DaemonState) -> bool {
    state.roster.settings().usage.refresh_every.0 > std::time::Duration::ZERO
        && state
            .usage_watch
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
}

/// Marks the pass [`begin`] started as over.
pub(super) fn end(state: &DaemonState) {
    state.usage_watch.running.store(false, Ordering::Release);
}

/// One pass at this Station's clock, reading each due account through its handler.
pub(super) fn run(state: &DaemonState) {
    pass(state, state.clock.now(), &|account| {
        read_account(state, account)
    });
}

/// Asks `handler` for the usage of the account behind `environment`, when it can be asked with no
/// turn: a handler that learns its usage only during a turn, or with a session open, or reports
/// none, is not asked.
pub(super) fn read_through(
    handler: &dyn AgentHandler,
    environment: BTreeMap<String, String>,
) -> Read {
    let support = match handler.usage_support() {
        Ok(support) => support,
        Err(error) => return Read::Failed(error.to_string()),
    };
    if !support.reports() {
        return Read::NotPolled("this handler reports no usage".to_owned());
    }
    if support.needs_turn {
        return Read::NotPolled("this handler learns its usage only during a turn".to_owned());
    }
    if support.needs_session {
        return Read::NotPolled(
            "this handler learns its usage only with a session open".to_owned(),
        );
    }
    match handler.usage(environment) {
        Ok(report) if report.supported => Read::Report(report),
        Ok(report) => Read::NotPolled(
            report
                .unsupported_because
                .unwrap_or_else(|| "this handler reports no usage".to_owned()),
        ),
        Err(error) => Read::Failed(error.to_string()),
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
    match models::with_account(state, &broker, pool, member, &holder, |environment| {
        read_through(handler.as_ref(), environment)
    }) {
        Ok(read) => read,
        Err(reason) => Read::Failed(reason),
    }
}

/// The seconds to wait after the account was found used up `exhausted` reads in a row.
pub(super) fn wait_after(exhausted: u32, every: u64, factor: u32, longest: u64) -> u64 {
    if factor <= 1 {
        return every;
    }
    let grown = (0..exhausted).fold(every, |wait, _| wait.saturating_mul(u64::from(factor)));
    grown.min(longest)
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

/// One pass over the limited accounts at `now`, reading each one that is due with `read`.
pub(super) fn pass(state: &DaemonState, now: i64, read: &dyn Fn(&AccountState) -> Read) {
    let usage = &state.roster.settings().usage;
    let every = usage.refresh_every.0.as_secs();
    let factor = usage.backoff_factor;
    let longest = usage.backoff_max.0.as_secs();
    let grace =
        i64::try_from(state.station.limits().accounts.reset_grace.0.as_secs()).unwrap_or(i64::MAX);
    let Ok(states) = state.intake.accounts() else {
        return;
    };
    let limited: Vec<&AccountState> = states
        .iter()
        .filter(|account| {
            account.profiles.contains_key(&state.station_id) && account.is_limited(now as f64)
        })
        .collect();
    // An account no longer limited, or a setting of 0, leaves nothing to schedule.
    state.usage_watch.lock().schedules.retain(|key, _| {
        every > 0
            && limited
                .iter()
                .any(|account| account.account.to_string() == *key)
    });
    if every == 0 {
        return;
    }
    for account in limited {
        let key = account.account.to_string();
        let until = account.limited_until.map_or(now, |until| until as i64);
        let first = now.saturating_add(i64::try_from(every).unwrap_or(i64::MAX));
        let due = {
            let mut inner = state.usage_watch.lock();
            let schedule = inner.schedules.entry(key.clone()).or_insert(Schedule {
                next: until.min(first),
                exhausted_reads: 0,
            });
            schedule.next <= now
        };
        if !due {
            continue;
        }
        let waiting = state.usage_watch.needs_of(&account.account);
        let report = match read(account) {
            Read::NotPolled(why) => {
                // Its timed retry is the stated time; looking again before then answers the same.
                if let Some(schedule) = state.usage_watch.lock().schedules.get_mut(&key)
                    && schedule.next != until
                {
                    schedule.next = until;
                    eprintln!(
                        "ai6: account {} is not re-read before its limit ends: {why}",
                        account.account
                    );
                }
                continue;
            }
            Read::Failed(reason) => {
                eprintln!(
                    "ai6: usage of account {} was not read: {reason}",
                    account.account
                );
                back_off(state, &key, now, (every, factor, longest));
                continue;
            }
            Read::Report(report) => report,
        };
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
        let models: Vec<Option<String>> = waiting.iter().map(|(_, model)| model.clone()).collect();
        match verdict(&report, &models, now) {
            Verdict::Room => {
                free(state, account, &waiting, now);
                state.usage_watch.lock().schedules.remove(&key);
            }
            Verdict::UsedUp { resets_at } => {
                if let Some(reset) = resets_at.filter(|reset| (*reset as f64) > until as f64) {
                    move_later(state, account, &waiting, reset, grace, now);
                }
                back_off(state, &key, now, (every, factor, longest));
            }
            Verdict::Unknown => back_off(state, &key, now, (every, factor, longest)),
        }
    }
}

fn back_off(state: &DaemonState, key: &str, now: i64, (every, factor, longest): (u64, u32, u64)) {
    if let Some(schedule) = state.usage_watch.lock().schedules.get_mut(key) {
        schedule.exhausted_reads = schedule.exhausted_reads.saturating_add(1);
        let wait = wait_after(schedule.exhausted_reads, every, factor, longest);
        schedule.next = now.saturating_add(i64::try_from(wait).unwrap_or(i64::MAX));
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
    let mut resumed = Vec::new();
    for (mission_id, _) in waiting {
        if is_parked(state, mission_id) && park_mission(state, mission_id, now) {
            eprintln!(
                "ai6: account {} has room again; Mission {mission_id:?} resumes",
                account.account
            );
            resumed.push(mission_id.clone());
        }
    }
    state.usage_watch.forget_waiting(&resumed);
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
    use crate::directorates::registry::accounts::AccountProfile;
    use coqui_acp_api::UsageSupport;
    use guatiao::library::ProviderError;
    use std::cell::Cell;
    use std::sync::Arc;
    use std::sync::atomic::AtomicI64;

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
        let profile = AccountProfile {
            handler: "mock-limit".to_owned(),
            kind: "claude-oauth-token".to_owned(),
            operations: vec!["G".to_owned()],
            priority: 0,
            max_concurrent: None,
            labels: BTreeMap::new(),
        };
        state
            .intake
            .declare_account(&account, &state.station_id, &profile)
            .unwrap();
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
            .park("waits", STATED_RESET)
            .unwrap();
        state
            .usage_watch
            .hold(&account, "waits", Some("model-a".to_owned()));
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

    #[test]
    fn room_in_the_bucket_of_the_missions_model_frees_the_account_before_its_stated_reset() {
        let root = tempfile::tempdir().unwrap();
        let (state, now, account) = limited_account(root.path(), "");
        let reads = Cell::new(0);
        let read = |_: &AccountState| {
            reads.set(reads.get() + 1);
            Read::Report(reported(vec![
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
            ]))
        };
        // Nothing is read before the first interval has gone by.
        pass(&state, START, &read);
        assert_eq!(reads.get(), 0);
        now.store(START + 301, Ordering::SeqCst);
        pass(&state, START + 301, &read);
        assert_eq!(reads.get(), 1);
        let freed = account_state(&state, &account);
        assert_eq!(freed.limited_until, None, "the account is free");
        assert_eq!(freed.windows.len(), 2, "what was read is on the board");
        assert_eq!(
            parked_until_of(&state),
            Some(START + 301),
            "the Mission is due at the next scheduler pass, hours before the stated reset"
        );
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

    #[test]
    fn a_reset_the_provider_moved_later_re_parks_the_mission_and_keeps_the_account_out() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let moved = STATED_RESET + 7_200;
        let read = |_: &AccountState| {
            Read::Report(reported(vec![bucket("mine", 1000, moved, Coverage::All)]))
        };
        pass(&state, START, &read);
        pass(&state, START + 300, &read);
        assert_eq!(
            account_state(&state, &account).limited_until,
            Some(moved as f64)
        );
        let grace = state.station.limits().accounts.reset_grace.0.as_secs() as i64;
        assert_eq!(parked_until_of(&state), Some(moved + grace));
    }

    #[test]
    fn a_used_up_account_is_read_again_after_a_longer_wait_up_to_the_ceiling() {
        assert_eq!(wait_after(1, 300, 2, 3_600), 600);
        assert_eq!(wait_after(2, 300, 2, 3_600), 1_200);
        assert_eq!(
            wait_after(4, 300, 2, 3_600),
            3_600,
            "never past the ceiling"
        );
        assert_eq!(
            wait_after(5, 300, 1, 3_600),
            300,
            "a factor of 1 keeps the interval"
        );
        assert_eq!(wait_after(5, 300, 0, 3_600), 300);
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let reads = Cell::new(0);
        let read = |_: &AccountState| {
            reads.set(reads.get() + 1);
            Read::Report(reported(vec![bucket(
                "mine",
                1000,
                STATED_RESET,
                Coverage::All,
            )]))
        };
        pass(&state, START, &read);
        pass(&state, START + 300, &read);
        assert_eq!(reads.get(), 1);
        pass(&state, START + 300 + 599, &read);
        assert_eq!(reads.get(), 1, "the second wait is twice the first");
        pass(&state, START + 300 + 600, &read);
        assert_eq!(reads.get(), 2);
        assert_eq!(
            account_state(&state, &account).limited_until,
            Some(STATED_RESET as f64),
            "an unmoved reset changes nothing"
        );
    }

    #[test]
    fn an_account_whose_handler_cannot_be_read_between_turns_is_never_polled() {
        let root = tempfile::tempdir().unwrap();
        let (state, _now, account) = limited_account(root.path(), "");
        let asked = Cell::new(0);
        let read = |_: &AccountState| {
            asked.set(asked.get() + 1);
            Read::NotPolled("it learns its usage only during a turn".to_owned())
        };
        pass(&state, START, &read);
        pass(&state, START + 300, &read);
        pass(&state, START + 301, &read);
        pass(&state, STATED_RESET - 1, &read);
        assert_eq!(asked.get(), 1, "asked once; the stated time is its retry");
        assert_eq!(
            account_state(&state, &account).limited_until,
            Some(STATED_RESET as f64)
        );
        assert_eq!(parked_until_of(&state), Some(STATED_RESET));
    }

    #[derive(Default)]
    struct Counting {
        support: UsageSupport,
        reads: std::sync::atomic::AtomicUsize,
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
    fn a_handler_that_needs_a_turn_or_a_session_or_reports_nothing_is_not_asked_to_read() {
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

    #[test]
    fn the_interval_is_read_from_the_settings_on_every_pass_and_zero_reads_nothing() {
        let root = tempfile::tempdir().unwrap();
        let (mut state, _now, account) = limited_account(root.path(), "");
        let reads = Cell::new(0);
        let read = |_: &AccountState| {
            reads.set(reads.get() + 1);
            Read::Report(reported(vec![bucket(
                "mine",
                1000,
                STATED_RESET,
                Coverage::All,
            )]))
        };
        // The interval is 300 s: the pass that sees the limit reads nothing.
        pass(&state, START, &read);
        assert_eq!(reads.get(), 0);
        // The record is changed to 60 s while the Station runs, as a reload of the roster does.
        retune(
            &mut state,
            root.path(),
            "\n[roster.usage]\nrefresh_every = 60\n",
        );
        // A schedule made under the old interval is kept until it falls due; a fresh account
        // takes the new one at once.
        state.usage_watch.lock().schedules.clear();
        pass(&state, START, &read);
        pass(&state, START + 100, &read);
        assert_eq!(reads.get(), 1);
        // Zero reads nothing and forgets the schedules.
        retune(
            &mut state,
            root.path(),
            "\n[roster.usage]\nrefresh_every = 0\n",
        );
        pass(&state, START + 100_000, &read);
        assert_eq!(reads.get(), 1);
        assert!(state.usage_watch.lock().schedules.is_empty());
        assert!(account_state(&state, &account).limited_until.is_some());
        assert!(!begin(&state), "a pass is not begun when reads are off");
    }
}
