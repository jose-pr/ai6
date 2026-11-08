//! Which of an Agent's routes a Session takes: what each route's standing is on this Station, the
//! one chosen by the Agent's `balance`, and where a resume may go.
//!
//! A route counts when its handler runs here, it can hold the Session's clearance, what it names
//! is configured, and its account source has room. The choice only reads: the claim that follows
//! is one Registry write ([`accounts`](super::accounts)), so two Stations never both take the last
//! place, and the one that loses tries again. A route with no pool gets an account of its own
//! that hands over nothing, so a limit on it is seen by every Station.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use super::DaemonState;
use super::accounts;
use crate::bounds::Limit;
use crate::directorates::m::roster::{Agent, AgentRoute, RouteRef};
use crate::directorates::q::{Account, AccountKind, AccountPool, RouteStanding, choose_route};
use crate::directorates::registry::accounts::AccountState;
use crate::directorates::registry::timeline::TimelineEvent;

/// The timeline kind of a route event.
const TIMELINE_KIND: &str = "route";
/// What every route with no pool calls its one account.
const IMPLICIT_ACCOUNT: &str = "login";
/// Starts the pool ID of a route with no pool.
const IMPLICIT_PREFIX: &str = "route-";

/// Where a route would run.
pub(super) struct Place<'a> {
    /// Why a route's handler cannot run here; `None` when it can.
    pub(super) refuses: &'a dyn Fn(&AgentRoute) -> Option<String>,
    /// The clearance its handler must be able to hold; `None` when none is asked of it here (a
    /// runner hosts the handler, or an assistant Session, which is restricted by its mode).
    pub(super) clearance: Option<&'a str>,
    /// The Session is a review, which is issued no credential.
    pub(super) review: bool,
}

/// The route a Session runs on, and what is needed to run on it.
#[derive(Debug, Clone)]
pub(super) struct OnRoute {
    /// The route, complete.
    pub(super) route: AgentRoute,
    /// The account of a route with no pool, claimed like any other.
    pub(super) implicit: Option<AccountPool>,
}

/// What a survey of an Agent's routes found.
pub(super) struct Survey {
    /// The routes with room, in the order they are listed.
    open: Vec<(OnRoute, RouteStanding)>,
    /// Each other route and why it does not count.
    closed: Vec<(RouteRef, String)>,
}

impl Survey {
    /// Each route that does not count and why, for a person to read.
    pub(super) fn reasons(&self) -> String {
        self.closed
            .iter()
            .map(|(route, reason)| format!("{route}: {reason}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The open route `reference` names.
    fn open_named(&self, reference: &RouteRef) -> Option<&OnRoute> {
        self.open
            .iter()
            .map(|(on, _)| on)
            .find(|on| on.route.reference() == *reference)
    }
}

/// The account of a route of `agent` with no pool: its ID comes from what names the route, so
/// every Station that lists the route counts the same row, and a route that changes starts clean.
pub(super) fn implicit_pool(agent: &str, route: &AgentRoute) -> AccountPool {
    let mut digest = Sha256::new();
    for part in [
        agent,
        route.handler.as_str(),
        route.model.as_deref().unwrap_or_default(),
    ] {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    let id: String = digest
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let account = Account::new(IMPLICIT_ACCOUNT, AccountKind::HarnessLogin)
        .label("agent", agent)
        .label("handler", &route.handler);
    let account = match &route.model {
        Some(model) => account.label("model", model),
        None => account,
    };
    AccountPool::new(format!("{IMPLICIT_PREFIX}{id}"), &route.handler).account(account)
}

/// Whether `pool` names a route's own account rather than a configured pool.
pub(super) fn is_implicit(pool: &str) -> bool {
    pool.starts_with(IMPLICIT_PREFIX)
}

/// A route's own account again, from what a Session record names, for a harness that already runs
/// on it.
pub(super) fn implicit_pool_named(pool: &str, handler: &str) -> AccountPool {
    AccountPool::new(pool, handler)
        .account(Account::new(IMPLICIT_ACCOUNT, AccountKind::HarnessLogin))
}

/// The standing of `route`: `Err` says why it does not count.
fn standing_of(
    state: &DaemonState,
    operation: Option<&str>,
    agent: &Agent,
    route: &AgentRoute,
    place: &Place<'_>,
    states: &[AccountState],
) -> Result<(OnRoute, RouteStanding), String> {
    if let Some(reason) = (place.refuses)(route) {
        return Err(reason);
    }
    if let Some(hold) = super::stewardship::hold_on(state, operation, &agent.id, &route.reference())
    {
        return Err(format!("held by {}: {}", hold.by, hold.reason));
    }
    if let Some(clearance) = place.clearance {
        let handler = state
            .plugins
            .handler_with(&route.handler, &route.handler_settings)
            .map_err(|error| format!("its handler could not be built: {error}"))?;
        let refusal = handler
            .clearance_refusal(clearance.to_owned())
            .map_err(|error| format!("its handler did not answer: {error}"))?;
        if let Some(reason) = refusal {
            return Err(format!("it cannot hold {clearance} clearance: {reason}"));
        }
    }
    let catalog = state.station.credentials.catalog();
    if let Some(catalog) = catalog {
        catalog.check_session_env(&route.env, Limit::UNBOUNDED)?;
    }
    for name in &route.secrets {
        match catalog {
            Some(catalog) if catalog.knows_secret(name) && !catalog.secret(name).green_ink() => {}
            Some(catalog) if catalog.knows_secret(name) => {
                return Err(format!(
                    "secret {name:?} is a green ink secret, which only an approval leases"
                ));
            }
            _ => return Err(format!("secret {name:?} is not configured on this Station")),
        }
    }
    let routed = agent.on_route(route);
    let now = state.clock.now() as f64;
    let unknown_use = state.station.limits().accounts.unknown_use_permille;
    let (best, implicit) = match accounts::source_of(state, operation, &routed) {
        accounts::Source::Refused(reason) => return Err(reason),
        accounts::Source::Pools(ids) => {
            let Some(catalog) = catalog else {
                return Err("its pool is not configured on this Station".to_owned());
            };
            let pools: Vec<&AccountPool> = ids.iter().filter_map(|id| catalog.pool(id)).collect();
            let best = accounts::best_account(&pools, states, now, unknown_use, &state.station_id)
                .ok_or_else(|| accounts::no_room(&pools, states, now))?;
            (Some(best), None)
        }
        accounts::Source::Unpooled => {
            let pool = implicit_pool(&agent.id, route);
            let row = states
                .iter()
                .find(|row| row.account.pool == pool.id())
                .cloned();
            if let Some(until) = row
                .as_ref()
                .filter(|row| row.is_limited(now))
                .and_then(|row| row.limited_until)
            {
                return Err(format!("its limit resets at {until:.0} (Unix seconds)"));
            }
            (row, Some(pool))
        }
    };
    if place.review && (!route.secrets.is_empty() || implicit.is_none()) {
        return Err("a review is issued no credential, and this route needs one".to_owned());
    }
    let standing = RouteStanding {
        // A use reported as unknown stays unknown here too: `unknown_use` weighs it.
        used_permille: best
            .as_ref()
            .filter(|row| !row.windows.is_empty())
            .map(|row| row.highest_use(now, unknown_use)),
        active: best.as_ref().map_or(0, |row| row.live_claims(now).count()),
        last_used: best.as_ref().and_then(|row| row.last_used),
        priority: route.priority,
    };
    Ok((
        OnRoute {
            route: route.clone(),
            implicit,
        },
        standing,
    ))
}

/// The standing of each of `agent`'s routes at `place`.
pub(super) fn survey(
    state: &DaemonState,
    operation: Option<&str>,
    agent: &Agent,
    place: &Place<'_>,
) -> Survey {
    let states = state.intake.accounts().unwrap_or_default();
    let mut survey = Survey {
        open: Vec::new(),
        closed: Vec::new(),
    };
    for route in agent.route_list() {
        match standing_of(state, operation, agent, &route, place, &states) {
            Ok(open) => survey.open.push(open),
            Err(reason) => survey.closed.push((route.reference(), reason)),
        }
    }
    survey
}

/// Why no route of `agent` can take a Session, for the Mission's or the conversation's record.
fn none_open(agent: &Agent, survey: &Survey) -> String {
    format!(
        "no route of Agent {:?} has room: {}",
        agent.id,
        survey.reasons()
    )
}

/// The route `balance` picks among `survey`'s, keeping nothing.
fn pick(state: &DaemonState, agent: &Agent, survey: &Survey) -> Option<OnRoute> {
    let standings: Vec<RouteStanding> = survey.open.iter().map(|(_, standing)| *standing).collect();
    let unknown_use = state.station.limits().accounts.unknown_use_permille;
    let chosen = choose_route(agent.balance.strategy(), &standings, unknown_use)?;
    Some(survey.open[chosen].0.clone())
}

/// The route a new Session of the routed `agent` takes at `place`.
///
/// # Errors
///
/// Why no route has room, naming each and why.
pub(super) fn choose(
    state: &DaemonState,
    operation: Option<&str>,
    agent: &Agent,
    place: &Place<'_>,
) -> Result<OnRoute, String> {
    let found = survey(state, operation, agent, place);
    pick(state, agent, &found).ok_or_else(|| none_open(agent, &found))
}

/// The route a conversation of the routed `agent` runs on at `place`: `earlier`, the route it last
/// ran on, while that has room, which is the one way to keep its provider Session; else the one
/// `balance` picks. Whether it stayed.
///
/// # Errors
///
/// Why no route has room, naming each and why.
pub(super) fn for_conversation(
    state: &DaemonState,
    agent: &Agent,
    earlier: Option<&RouteRef>,
    place: &Place<'_>,
) -> Result<(OnRoute, bool), String> {
    let found = survey(state, None, agent, place);
    if let Some(on) = earlier.and_then(|earlier| found.open_named(earlier)) {
        return Ok((on.clone(), true));
    }
    let on = pick(state, agent, &found).ok_or_else(|| none_open(agent, &found))?;
    Ok((on, false))
}

/// Why a brief for the routed `agent` waits: no route has room as far as the deciding Station
/// can tell. Which Station runs it, and so which handlers it has, is decided later.
pub(super) fn hold(state: &DaemonState, operation: Option<&str>, agent: &Agent) -> Option<String> {
    let place = Place {
        refuses: &|_| None,
        clearance: None,
        review: false,
    };
    let found = survey(state, operation, agent, &place);
    found.open.is_empty().then(|| none_open(agent, &found))
}

/// What a limited Session of the routed `agent` may do next: whether any route has room, and
/// whether one of `handler` has an account a provider Session can move to.
pub(super) fn room_after_limit(
    state: &DaemonState,
    operation: Option<&str>,
    agent: &Agent,
    handler: &str,
) -> (bool, bool) {
    let available = super::available_handlers(state);
    let place = Place {
        refuses: &|route| (!available.contains(&route.handler)).then(|| "no handler".to_owned()),
        clearance: None,
        review: false,
    };
    let found = survey(state, operation, agent, &place);
    let carrying = found
        .open
        .iter()
        .filter(|(on, _)| on.route.handler == handler)
        .any(|(on, _)| accounts::route_carries_session(state, operation, agent, &on.route));
    (!found.open.is_empty(), carrying)
}

/// Where a resume of `agent`'s Session on `earlier` goes.
#[derive(Debug, Clone)]
pub(super) enum ResumeOn {
    /// The route it ran on has room, or it holds its place there.
    Stay(OnRoute),
    /// Another route of the same handler, whose account the provider Session can move to.
    Move(OnRoute),
    /// No route it can move to has room, but another has: a new attempt, saying why.
    NewAttempt(String),
    /// No route has room: it waits, saying why.
    Wait(String),
}

/// Applies the resume rule: a Session stays on its route, since its history belongs to the harness
/// and the source; without room it moves only to a route of the same handler whose accounts carry
/// that history; otherwise another route means a new attempt. `carried` says whether the account
/// the Session last held is of a kind that carries it.
pub(super) fn for_resume(
    state: &DaemonState,
    operation: Option<&str>,
    agent: &Agent,
    earlier: &RouteRef,
    carried: bool,
    place: &Place<'_>,
) -> ResumeOn {
    let found = survey(state, operation, agent, place);
    if let Some(on) = found.open_named(earlier) {
        return ResumeOn::Stay(on.clone());
    }
    if carried
        && let Some((on, _)) = found.open.iter().find(|(on, _)| {
            on.route.handler == earlier.handler
                && accounts::route_carries_session(state, operation, agent, &on.route)
        })
    {
        return ResumeOn::Move(on.clone());
    }
    let why = found
        .closed
        .iter()
        .find(|(route, _)| route == earlier)
        .map_or_else(
            || format!("route {earlier} is no longer on Agent {:?}", agent.id),
            |(_, reason)| format!("route {earlier} cannot take it: {reason}"),
        );
    if found.open.is_empty() {
        ResumeOn::Wait(none_open(agent, &found))
    } else {
        ResumeOn::NewAttempt(why)
    }
}

/// Records on the Mission's timeline which route it took, and when a route was passed over why.
pub(super) fn record_chosen(
    state: &DaemonState,
    operation: Option<&str>,
    mission: &str,
    agent: &str,
    action: &str,
    on: &OnRoute,
) {
    let Some(operation) = operation else {
        return;
    };
    let route = on.route.reference();
    let mut event = TimelineEvent::new(TIMELINE_KIND, action, operation, state.clock.now() as f64)
        .station(state.station_id.clone())
        .mission(mission)
        .actor(agent)
        .detail("handler", route.handler.as_str());
    if let Some(model) = &route.model {
        event = event.detail("model", model.as_str());
    }
    if let Some(pool) = &route.pool {
        event = event.detail("pool", pool.as_str());
    }
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: route of Mission {mission:?} was not recorded on the timeline: {error}");
    }
}

/// The routes of a roster with no pool, each with its account and who may draw on it, so the board
/// can show their limits like any account's.
pub(super) fn implicit_accounts(
    state: &DaemonState,
) -> BTreeMap<String, (AccountPool, Vec<String>)> {
    let mut found: BTreeMap<String, (AccountPool, Vec<String>)> = BTreeMap::new();
    let mut note = |agent: &Agent, operation: Option<&str>| {
        for route in agent.route_list() {
            let routed = agent.on_route(&route);
            if matches!(
                accounts::source_of(state, operation, &routed),
                accounts::Source::Unpooled
            ) {
                let pool = implicit_pool(&agent.id, &route);
                let entry = found
                    .entry(pool.id().to_owned())
                    .or_insert_with(|| (pool, Vec::new()));
                entry.1.extend(operation.map(str::to_owned));
            }
        }
    };
    for id in state.roster.agent_ids() {
        if let Ok(agent) = state.roster.agent(None, id)
            && agent.is_routed()
        {
            note(&agent, None);
        }
    }
    for operation in state.roster.operations() {
        for agent in state
            .roster
            .agents_for_operation(&operation.id)
            .unwrap_or_default()
        {
            if agent.is_routed() {
                note(&agent, Some(&operation.id));
            }
        }
    }
    found
}
