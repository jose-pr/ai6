//! What the Station learns of the models each account and route may use.
//!
//! Each look asks a handler, with the credentials a Session on that account or route would get,
//! to list its models ([`AgentHandler::models`]): no model turn is spent and no pool account is
//! claimed. A handler that cannot list says so, and the answer stays "unknown"; nothing is
//! guessed. The credentials come from the same leases a Session's do and are revoked as soon as
//! the question is over; none is ever part of an answer. The board is rebuilt every
//! `[accounts] models_every` seconds and reported at check-in under `models`; it only informs a
//! person, and makes no route or pool.
//!
//! A look runs on a thread of its own, never on the check-in beat, and asks up to `[accounts]
//! model_probes` handlers at once, waiting `model_probe_timeout` for each; a handler that does
//! not answer in time leaves what the last look found for its account or route.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{DaemonState, accounts, credentials};
use crate::bounds::{Limit, TimeLimit};
use crate::directorates::m::roster::{Agent, AgentRoute, RouteRef};
use crate::directorates::q::{Account, AccountPool};
use crate::directorates::section_00::agent::{AgentHandler, ModelListing};

/// What one account or route may use, as its handler said.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    /// `pool/account`, or `agent/handler/model` for a route that draws from no pool.
    pub source: String,
    /// The pool, for an account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    /// The account, for an account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The Agent, for a route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The route, for a route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<RouteRef>,
    /// The handler asked.
    pub handler: String,
    /// What it said.
    pub listing: ModelListing,
    /// When, as Unix seconds.
    pub at: f64,
}

/// The last look, kept for the next check-in.
#[derive(Default)]
pub(super) struct ModelBoard {
    found: Mutex<Vec<Found>>,
    looked_at: Mutex<Option<f64>>,
    /// Set while a look runs, so the beat never starts a second one beside it.
    looking: AtomicBool,
}

impl ModelBoard {
    /// What the last look found.
    pub(super) fn found(&self) -> Vec<Found> {
        self.found
            .lock()
            .map(|found| found.clone())
            .unwrap_or_default()
    }

    fn put(&self, found: Vec<Found>, at: f64) {
        if let Ok(mut held) = self.found.lock() {
            *held = found;
        }
        if let Ok(mut looked) = self.looked_at.lock() {
            *looked = Some(at);
        }
    }

    /// Whether a look is due at `now`: the first, or one `every` after the last. `every` of 0
    /// never looks on its own.
    fn due(&self, now: f64, every: Duration) -> bool {
        if every.is_zero() {
            return false;
        }
        match self.looked_at.lock().ok().and_then(|looked| *looked) {
            None => true,
            Some(at) => now - at >= every.as_secs_f64(),
        }
    }
}

/// Whether a look is due and none is running; `true` marks one running until [`end`].
pub(super) fn begin(state: &DaemonState) -> bool {
    let now = state.clock.now() as f64;
    let every = state.station.limits().accounts.models_every.0;
    state.models.due(now, every)
        && state
            .models
            .looking
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
}

/// Marks the look [`begin`] started as over.
pub(super) fn end(state: &DaemonState) {
    state.models.looking.store(false, Ordering::Release);
}

/// The variables every harness is started with for a look: where it finds its program and a
/// private home, so the Station user's own login is never read.
fn base(state: &DaemonState) -> BTreeMap<String, String> {
    let home = std::env::temp_dir().join(format!("ai6-models-{}", state.station_id));
    let _ = std::fs::create_dir_all(&home);
    let home = home.display().to_string();
    let mut environment = BTreeMap::new();
    for name in ["PATH", "SYSTEMROOT", "windir", "PATHEXT", "COMSPEC"] {
        if let Ok(value) = std::env::var(name) {
            environment.insert(name.to_owned(), value);
        }
    }
    for name in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
    ] {
        environment.insert(name.to_owned(), home.clone());
    }
    environment.insert("CLAUDE_CONFIG_DIR".to_owned(), format!("{home}/.claude"));
    environment.insert("CODEX_HOME".to_owned(), format!("{home}/.codex"));
    environment
}

fn ask(
    state: &DaemonState,
    handler: &str,
    settings: &serde_json::Map<String, serde_json::Value>,
    environment: BTreeMap<String, String>,
) -> ModelListing {
    let built = state.plugins.handler_with(handler, settings);
    let handler: Box<dyn AgentHandler> = match built {
        Ok(handler) => handler,
        Err(_) => return ModelListing::unknown("this handler is not on this Station"),
    };
    handler
        .models(environment)
        .unwrap_or_else(|error| ModelListing::unknown(format!("the handler failed: {error}")))
}

/// One account or route to ask: its source, and the question, asked on a thread of its own.
type Probe = (String, Box<dyn FnOnce() -> Found + Send>);

/// Looks at every pool account and every route no pool serves, and keeps what it found.
pub(super) fn look(state: &DaemonState) {
    let now = state.clock.now() as f64;
    let mut probes: Vec<Probe> = Vec::new();
    if let Some((catalog, _)) = credentials::listing_broker(state) {
        for pool in catalog.pools() {
            for account in pool.accounts() {
                let (state, pool, account) = (state.clone(), pool.clone(), account.clone());
                probes.push((
                    format!("{}/{}", pool.id(), account.name()),
                    Box::new(move || match credentials::listing_broker(&state) {
                        Some((_, broker)) => of_account(&state, &broker, &pool, &account, now),
                        None => unanswered(&pool, &account, now),
                    }),
                ));
            }
        }
    }
    for id in state.roster.agent_ids() {
        let Ok(agent) = state.roster.agent(None, id) else {
            continue;
        };
        for route in agent.route_list() {
            // A route a pool serves is listed by its pool's accounts.
            let routed = agent.on_route(&route);
            if matches!(
                accounts::source_of(state, None, &routed),
                accounts::Source::Unpooled
            ) {
                let (state, agent) = (state.clone(), agent.clone());
                probes.push((
                    route_source(&agent, &route),
                    Box::new(move || of_route(&state, &agent, &route, now)),
                ));
            }
        }
    }
    let limits = &state.station.limits().accounts;
    let before = state.models.found();
    let found = ask_all(
        probes,
        limits.model_probes,
        limits.model_probe_timeout,
        &before,
        now,
    )
    .unwrap_or(before);
    state.models.put(found, now);
}

/// What each probe found, in the order given, with up to `width` asked at once and each waited
/// for within `timeout`: one that does not answer in time keeps what `before` found for its
/// source, or is unknown. `None` when `width` asks none.
fn ask_all(
    probes: Vec<Probe>,
    width: Limit,
    timeout: TimeLimit,
    before: &[Found],
    now: f64,
) -> Option<Vec<Found>> {
    let count = probes.len();
    let workers = match width.get() {
        Some(0) => return None,
        Some(width) => usize::try_from(width).unwrap_or(usize::MAX).min(count),
        None => count,
    };
    let sources: Vec<String> = probes.iter().map(|(source, _)| source.clone()).collect();
    let queue = Arc::new(Mutex::new(
        probes.into_iter().enumerate().collect::<VecDeque<_>>(),
    ));
    let (answers, answered) = mpsc::channel::<(usize, Option<Found>)>();
    for _ in 0..workers {
        let (queue, answers) = (queue.clone(), answers.clone());
        let worker = move || {
            loop {
                let next = queue.lock().ok().and_then(|mut queue| queue.pop_front());
                let Some((index, (_, probe))) = next else {
                    return;
                };
                let _ = answers.send((index, within(probe, timeout)));
            }
        };
        if let Err(error) = std::thread::Builder::new()
            .name("ai6-models".to_owned())
            .spawn(worker)
        {
            eprintln!("ai6: a model listing worker did not start: {error}");
        }
    }
    drop(answers);
    let mut found: Vec<Option<Found>> = (0..count).map(|_| None).collect();
    for (index, answer) in answered {
        found[index] = answer;
    }
    Some(
        found
            .into_iter()
            .zip(sources)
            .map(|(answer, source)| {
                answer.unwrap_or_else(|| {
                    let mut kept = before
                        .iter()
                        .find(|found| found.source == source)
                        .cloned()
                        .unwrap_or_else(|| Found::unknown(source.clone(), now));
                    if !kept.listing.known {
                        kept.listing = ModelListing::unknown(format!(
                            "the handler did not answer within {} s",
                            timeout.as_secs_f64()
                        ));
                    }
                    kept
                })
            })
            .collect(),
    )
}

/// The probe's answer, or `None` when it takes longer than `timeout`; it then runs on, unread.
fn within(probe: Box<dyn FnOnce() -> Found + Send>, timeout: TimeLimit) -> Option<Found> {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name("ai6-models-probe".to_owned())
        .spawn(move || {
            let _ = sender.send(probe());
        })
        .ok()?;
    match timeout.get() {
        Some(timeout) => receiver.recv_timeout(timeout).ok(),
        None => receiver.recv().ok(),
    }
}

impl Found {
    /// A source no answer came for, with nothing found before.
    fn unknown(source: String, at: f64) -> Self {
        Self {
            source,
            pool: None,
            account: None,
            agent: None,
            route: None,
            handler: String::new(),
            listing: ModelListing::unknown("the handler did not answer"),
            at,
        }
    }
}

/// An account looked at while the Station has no loadouts to lease its credential from.
fn unanswered(pool: &AccountPool, account: &Account, now: f64) -> Found {
    Found {
        source: format!("{}/{}", pool.id(), account.name()),
        pool: Some(pool.id().to_owned()),
        account: Some(account.name().to_owned()),
        agent: None,
        route: None,
        handler: pool.handler().to_owned(),
        listing: ModelListing::unknown("this Station has no loadouts to lease its credential"),
        at: now,
    }
}

fn route_source(agent: &Agent, route: &AgentRoute) -> String {
    format!(
        "{}/{}/{}",
        agent.id,
        route.handler,
        route.model.as_deref().unwrap_or("-")
    )
}

fn of_account(
    state: &DaemonState,
    broker: &crate::directorates::q::SecretBroker,
    pool: &AccountPool,
    account: &Account,
    now: f64,
) -> Found {
    let holder = format!("models-{}-{}", pool.id(), account.name());
    let mut environment = base(state);
    let site = accounts::Site {
        resuming: false,
        on_station: true,
    };
    let listing = match accounts::hand_over(
        state,
        broker,
        pool,
        account,
        &holder,
        site,
        &mut environment,
    ) {
        Ok(lease) => {
            let listing = ask(state, pool.handler(), &serde_json::Map::new(), environment);
            if let Some(lease) = lease
                && let Err(error) = lease.revoke()
            {
                eprintln!("ai6: a credential lease for listing models was not revoked: {error}");
            }
            listing
        }
        Err(reason) => ModelListing::unknown(reason),
    };
    Found {
        source: format!("{}/{}", pool.id(), account.name()),
        pool: Some(pool.id().to_owned()),
        account: Some(account.name().to_owned()),
        agent: None,
        route: None,
        handler: pool.handler().to_owned(),
        listing,
        at: now,
    }
}

fn of_route(state: &DaemonState, agent: &Agent, route: &AgentRoute, now: f64) -> Found {
    let reference = route.reference();
    let holder = format!("models-{}-{}", agent.id, route.handler);
    let routed = agent.on_route(route);
    let mut environment = base(state);
    environment.extend(routed.env.clone());
    let listing =
        match credentials::issue_for_listing(state, &holder, &routed, route, &mut environment) {
            Ok(()) => {
                let listing = ask(state, &route.handler, &route.handler_settings, environment);
                credentials::release(state, &holder);
                listing
            }
            Err(reason) => ModelListing::unknown(reason),
        };
    Found {
        source: route_source(agent, route),
        pool: None,
        account: None,
        agent: Some(agent.id.clone()),
        route: Some(reference),
        handler: route.handler.clone(),
        listing,
        at: now,
    }
}
