//! Placement: which Station, Theater and Agent may run an Operation's Mission, and why
//! the others may not.
//!
//! M decides placement from plain facts: the Operation's policy, what each Station last
//! checked in, and the Agents posted to the Operation. [`place`] returns every eligible
//! (Station, Theater, Agent) choice, best first, and one reason for every rejection, so
//! "why did or didn't it run there" can be answered before anything launches.
//!
//! Order is the least-loaded Station first (the deciding Station wins a tie, then Station
//! ID), the `local` Theater before others, then the Agent with the highest priority.

use std::collections::BTreeMap;
use std::time::Duration;

use globset::Glob;
use serde::{Deserialize, Serialize};

use crate::directorates::m::roster::{Agent, Roster, RosterError};
use crate::directorates::m::theaters::{
    AgentMode, EgressKind, LOCAL_THEATER, TheaterKind, TheaterSummary,
};
use crate::directorates::q::transport::Capabilities;
use crate::directorates::registry::store::StationRecord;

/// What placement needs to know about an Operation and the Agents posted to it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPolicy {
    /// The Operation's codename.
    pub operation: String,
    /// Labels a Station (overlaid by its Theater's labels) must carry.
    #[serde(default)]
    pub station_labels: BTreeMap<String, String>,
    /// Stations allowed to run it; empty allows every Station.
    #[serde(default)]
    pub stations: Vec<String>,
    /// The Theater the Operation is bound to, when it names one.
    #[serde(default)]
    pub theater: Option<String>,
    /// Whether the Operation lives in a local checkout that the Station must have.
    #[serde(default)]
    pub checkout: bool,
    /// The Agents posted to the Operation.
    #[serde(default)]
    pub agents: Vec<AgentFacts>,
}

/// The part of an Agent that placement looks at.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentFacts {
    /// The Agent's codename.
    pub id: String,
    /// The AgentHandler provider the Agent needs, when configured.
    #[serde(default)]
    pub handler: Option<String>,
    /// Higher priority is preferred.
    #[serde(default)]
    pub priority: i64,
    /// Roles the Agent is qualified for.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Tags the Agent carries.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl From<&Agent> for AgentFacts {
    fn from(agent: &Agent) -> Self {
        Self {
            id: agent.id.clone(),
            handler: agent.handler.clone(),
            priority: agent.priority,
            roles: agent.roles.clone(),
            tags: agent.tags.clone(),
        }
    }
}

impl OperationPolicy {
    /// A policy with no restrictions and no Agents, to build on.
    pub fn new(operation: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            station_labels: BTreeMap::new(),
            stations: Vec::new(),
            theater: None,
            checkout: false,
            agents: Vec::new(),
        }
    }

    /// Reads a registered Operation's policy and its posted Agents from the roster.
    pub fn from_roster(roster: &Roster, operation_key: &str) -> Result<Self, RosterError> {
        let operation = roster.operation(operation_key)?;
        let agents = roster
            .agents_for_operation(&operation.id)?
            .iter()
            .map(AgentFacts::from)
            .collect();
        Ok(Self {
            operation: operation.id.clone(),
            station_labels: operation.station_labels.clone(),
            stations: operation.stations.clone(),
            theater: operation.theater.clone(),
            checkout: operation.path.is_some(),
            agents,
        })
    }
}

/// What a Station last checked in, reduced to what placement needs.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StationFacts {
    /// Station identity.
    pub station: String,
    /// Whether the last check-in is recent enough to trust.
    pub healthy: bool,
    /// Seconds since the last check-in.
    pub silent_for: u64,
    /// Station roles; `execution` is needed to run Sessions.
    pub roles: Vec<String>,
    /// Mission capacity it reported.
    pub capacity: usize,
    /// Missions already leased to it.
    pub load: usize,
    /// Its labels.
    pub labels: BTreeMap<String, String>,
    /// Operations whose checkout it has.
    pub checkouts: Vec<String>,
    /// AgentHandler providers it can run; `None` when it did not report them.
    pub handlers: Option<Vec<String>>,
    /// Theaters it reaches.
    pub theaters: Vec<TheaterSummary>,
}

impl StationFacts {
    /// Reads a Registry check-in. `load` is how many Missions are already leased to the
    /// Station; a Station silent for longer than `window` is unhealthy.
    ///
    /// Check-ins written before Stations reported a key fall back to the permissive
    /// reading: execution role, capacity one, own host only, handlers unknown.
    pub fn from_record(record: &StationRecord, now: f64, window: Duration, load: usize) -> Self {
        let info = record.info();
        let strings = |key: &str| {
            info.get(key)
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
        };
        let silent = (now - record.seen()).max(0.0);
        let labels = info
            .get("labels")
            .and_then(serde_json::Value::as_object)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let theaters = info
            .get("theaters")
            .and_then(|value| serde_json::from_value::<Vec<TheaterSummary>>(value.clone()).ok())
            .unwrap_or_else(|| vec![TheaterSummary::local()]);
        Self {
            station: record.station().to_owned(),
            healthy: silent <= window.as_secs_f64(),
            silent_for: silent as u64,
            roles: strings("roles").unwrap_or_else(|| vec!["execution".to_owned()]),
            capacity: info
                .get("capacity")
                .and_then(serde_json::Value::as_u64)
                .and_then(|capacity| usize::try_from(capacity).ok())
                .unwrap_or(1),
            load,
            labels,
            checkouts: strings("operations").unwrap_or_default(),
            handlers: strings("handlers"),
            theaters,
        }
    }
}

/// What the Mission asks for. Everything here only narrows the Operation's policy.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementRequest {
    /// Stations the Mission is limited to; empty adds no limit of its own.
    #[serde(default)]
    pub stations: Vec<String>,
    /// A specific Agent.
    #[serde(default)]
    pub agent: Option<String>,
    /// A role the Agent must be qualified for.
    #[serde(default)]
    pub role: Option<String>,
    /// Tags the Agent must carry.
    #[serde(default)]
    pub tags: Vec<String>,
    /// The Station doing the placing; it wins a tie on load.
    #[serde(default)]
    pub deciding_station: Option<String>,
}

/// Where the agent harness runs relative to the Theater it works on.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionMode {
    /// The harness runs on the Theater itself, in a terminal there.
    #[default]
    Field,
    /// The harness runs on the Station; its shell and file tools act on the Theater through
    /// the transport.
    RemoteHands,
}

impl ExecutionMode {
    /// The name used on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Field => "field",
            Self::RemoteHands => "remote-hands",
        }
    }
}

/// One place a Mission may run.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementChoice {
    /// The Station that would supervise the Session.
    pub station: String,
    /// The Theater the harness would work on.
    pub theater: String,
    /// The Agent that would be inserted.
    pub agent: String,
    /// The Agent's handler provider.
    pub handler: Option<String>,
    /// Whether the harness would run on the Theater or act on it from the Station.
    #[serde(default)]
    pub mode: ExecutionMode,
}

/// One thing that was ruled out, and why. Absent fields mean the reason applies to every
/// Station, Theater or Agent beneath it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementRejection {
    /// The Station ruled out, or `None` for an Agent ruled out everywhere.
    pub station: Option<String>,
    /// The Theater ruled out; `None` when the whole Station or Agent was.
    pub theater: Option<String>,
    /// The Agent ruled out; `None` when the whole Station or Theater was.
    pub agent: Option<String>,
    /// Why, in a sentence an operator can act on.
    pub reason: String,
}

/// The outcome of placement.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    /// Eligible choices, best first.
    pub choices: Vec<PlacementChoice>,
    /// Everything that was ruled out, with reasons.
    pub rejections: Vec<PlacementRejection>,
}

impl Placement {
    /// The choice placement would take.
    pub fn best(&self) -> Option<&PlacementChoice> {
        self.choices.first()
    }
}

/// Decides where a Mission for the Operation may run.
pub fn place(
    policy: &OperationPolicy,
    request: &PlacementRequest,
    stations: &[StationFacts],
) -> Placement {
    let mut placement = Placement::default();
    let candidates = candidate_agents(policy, request, &mut placement);

    let mut stations: Vec<&StationFacts> = stations.iter().collect();
    let deciding = request.deciding_station.as_deref();
    stations.sort_by(|a, b| {
        (a.load, Some(a.station.as_str()) != deciding, &a.station).cmp(&(
            b.load,
            Some(b.station.as_str()) != deciding,
            &b.station,
        ))
    });

    for station in stations {
        if let Some(reason) = station_rejection(policy, request, station) {
            placement.rejections.push(PlacementRejection {
                station: Some(station.station.clone()),
                theater: None,
                agent: None,
                reason,
            });
            continue;
        }
        let mut usable = Vec::new();
        for agent in &candidates {
            let handler = agent.handler.as_deref().unwrap_or_default();
            let missing = station
                .handlers
                .as_ref()
                .is_some_and(|handlers| !handlers.iter().any(|available| available == handler));
            if missing {
                placement.rejections.push(PlacementRejection {
                    station: Some(station.station.clone()),
                    theater: None,
                    agent: Some(agent.id.clone()),
                    reason: format!("handler {handler:?} is not available on this Station"),
                });
            } else {
                usable.push(*agent);
            }
        }
        let mut theaters: Vec<&TheaterSummary> = station.theaters.iter().collect();
        theaters.sort_by_key(|theater| (theater.id != LOCAL_THEATER, &theater.id));
        for theater in theaters {
            let mode = match theater_verdict(policy, station, theater) {
                Ok(mode) => mode,
                Err(reason) => {
                    placement.rejections.push(PlacementRejection {
                        station: Some(station.station.clone()),
                        theater: Some(theater.id.clone()),
                        agent: None,
                        reason,
                    });
                    continue;
                }
            };
            for agent in &usable {
                placement.choices.push(PlacementChoice {
                    station: station.station.clone(),
                    theater: theater.id.clone(),
                    agent: agent.id.clone(),
                    handler: agent.handler.clone(),
                    mode,
                });
            }
        }
    }
    placement
}

/// The posted Agents the Mission may use, best first. The others are rejected once,
/// everywhere, with the reason.
fn candidate_agents<'a>(
    policy: &'a OperationPolicy,
    request: &PlacementRequest,
    placement: &mut Placement,
) -> Vec<&'a AgentFacts> {
    let mut candidates = Vec::new();
    for agent in &policy.agents {
        match agent_rejection(agent, request) {
            Some(reason) => placement.rejections.push(PlacementRejection {
                station: None,
                theater: None,
                agent: Some(agent.id.clone()),
                reason,
            }),
            None => candidates.push(agent),
        }
    }
    candidates.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.id.cmp(&b.id)));
    candidates
}

fn agent_rejection(agent: &AgentFacts, request: &PlacementRequest) -> Option<String> {
    if request
        .agent
        .as_ref()
        .is_some_and(|wanted| wanted != &agent.id)
    {
        return Some("the Mission asks for a different Agent".to_owned());
    }
    if agent.handler.is_none() {
        return Some("it has no handler configured".to_owned());
    }
    if let Some(role) = request.role.as_deref()
        && !agent.roles.iter().any(|candidate| {
            role == candidate || role.starts_with(&format!("{}/", candidate.trim_end_matches('/')))
        })
    {
        return Some(format!("it is not qualified for role {role:?}"));
    }
    request
        .tags
        .iter()
        .find(|tag| !agent.tags.contains(tag))
        .map(|tag| format!("it does not carry tag {tag:?}"))
}

fn station_rejection(
    policy: &OperationPolicy,
    request: &PlacementRequest,
    station: &StationFacts,
) -> Option<String> {
    let id = &station.station;
    if !policy.stations.is_empty() && !policy.stations.contains(id) {
        return Some(format!(
            "the Operation allows only Stations [{}]",
            policy.stations.join(", ")
        ));
    }
    if !request.stations.is_empty() && !request.stations.contains(id) {
        return Some(format!(
            "the Mission narrows placement to Stations [{}]",
            request.stations.join(", ")
        ));
    }
    if !station.roles.iter().any(|role| role == "execution") {
        return Some("it has no execution role".to_owned());
    }
    if !station.healthy {
        return Some(format!(
            "it has not checked in for {} seconds (gone dark)",
            station.silent_for
        ));
    }
    if station.capacity == 0 {
        return Some("it accepts no Missions (capacity 0)".to_owned());
    }
    if station.load >= station.capacity {
        return Some(format!(
            "it is at capacity ({} of {} Missions)",
            station.load, station.capacity
        ));
    }
    None
}

/// How a Session would run on `theater`, or the reason it may not.
fn theater_verdict(
    policy: &OperationPolicy,
    station: &StationFacts,
    theater: &TheaterSummary,
) -> Result<ExecutionMode, String> {
    if let Some(bound) = &policy.theater
        && bound != &theater.id
    {
        return Err(format!("the Operation runs only in Theater {bound:?}"));
    }
    let mode = execution_mode(theater)?;
    // The Theater's own labels win over the Station's, so a Theater can refine them.
    let mut labels = station.labels.clone();
    labels.extend(theater.labels.clone());
    let unmet: Vec<String> = policy
        .station_labels
        .iter()
        .filter_map(|(key, pattern)| {
            let matched = labels.get(key).is_some_and(|value| {
                Glob::new(pattern).is_ok_and(|glob| glob.compile_matcher().is_match(value))
            });
            (!matched).then(|| match labels.get(key) {
                Some(value) => format!("{key}={pattern} (has {value:?})"),
                None => format!("{key}={pattern} (not set)"),
            })
        })
        .collect();
    if !unmet.is_empty() {
        return Err(format!(
            "labels do not satisfy station_labels: {}",
            unmet.join(", ")
        ));
    }
    if policy.checkout {
        let own_host = theater.access == TheaterKind::Local;
        match mode {
            ExecutionMode::Field if own_host && !station.checkouts.contains(&policy.operation) => {
                return Err(format!(
                    "this Station has no checkout of Operation {:?}",
                    policy.operation
                ));
            }
            // The checkout is on the Theater, which the Station cannot inspect: the
            // Operation must say where it is by binding to the Theater, or the Theater
            // must say where its Missions work.
            _ if (mode == ExecutionMode::RemoteHands || !own_host)
                && policy.theater.as_deref() != Some(theater.id.as_str())
                && theater.workdir.is_none() =>
            {
                return Err(format!(
                    "Operation {:?} has a checkout and this Theater has no `workdir`: bind the Operation to the Theater or set the Theater's `workdir`",
                    policy.operation
                ));
            }
            _ => {}
        }
    }
    Ok(mode)
}

/// What the Station's own host can do when it has not said: everything but forwarding.
fn assumed_own_host() -> Capabilities {
    Capabilities {
        terminal: true,
        run: true,
        files: true,
        ..Capabilities::default()
    }
}

/// How a Session would run on `theater`, or the capability that is missing.
///
/// Field mode runs the harness on the Theater and needs a route to the provider's API. A
/// runner there supervises it with its own providers; on another host the Station reaches
/// the runner with `run`. Without a runner, the Station's own host needs `terminal` and
/// `files`, and another host needs, for the detached wrapper the Station supervises over the
/// transport, `run`, `files`, a POSIX host and a declared harness. Remote-hands mode runs the
/// harness on the Station and needs only the transport's `run`. `agents` can force either.
pub fn execution_mode(theater: &TheaterSummary) -> Result<ExecutionMode, String> {
    if let Some(reason) = &theater.unavailable {
        return Err(format!("the Station cannot reach it: {reason}"));
    }
    let own_host = theater.access == TheaterKind::Local;
    let capabilities = match (&theater.capabilities, own_host) {
        (Some(capabilities), _) => capabilities.clone(),
        (None, true) => assumed_own_host(),
        (None, false) => {
            return Err(
                "the Station did not report what this Theater's transport can do".to_owned(),
            );
        }
    };

    let mut field_missing = Vec::new();
    if theater.runner {
        if !own_host && !capabilities.run {
            field_missing.push("the `run` capability to reach its runner".to_owned());
        }
    } else if own_host {
        if !capabilities.terminal {
            field_missing.push("the `terminal` capability".to_owned());
        }
        if !capabilities.files {
            field_missing.push("the `files` capability".to_owned());
        }
    } else {
        let mut wrapper = Vec::new();
        if theater.harnesses.is_empty() {
            wrapper.push("a declared harness (`harnesses`)");
        }
        if !capabilities.run {
            wrapper.push("the `run` capability");
        }
        if !capabilities.files {
            wrapper.push("the `files` capability");
        }
        match capabilities.os.as_deref() {
            Some("windows") => wrapper.push("a POSIX host"),
            None => wrapper.push("a known POSIX host (the transport has not reported its os)"),
            Some(_) => {}
        }
        if !wrapper.is_empty() {
            field_missing.push(format!(
                "a runner on the Theater (`runner`), or for the detached wrapper {}",
                wrapper.join(", ")
            ));
        }
    }
    match theater.egress {
        EgressKind::Isolated => {
            field_missing.push("a route to the provider's API (egress is `none`)".to_owned());
        }
        EgressKind::Station if !capabilities.forward => field_missing
            .push("the `forward` capability that egress = \"station\" relies on".to_owned()),
        _ => {}
    }
    let hands_missing = (!capabilities.run).then_some("the `run` capability");

    let reasons = |field: &[String], hands: Option<&str>| {
        format!(
            "field mode needs {}; remote-hands mode needs {}",
            field.join(", "),
            hands.unwrap_or("nothing more"),
        )
    };
    match theater.agents {
        AgentMode::Local => match field_missing.as_slice() {
            [] => Ok(ExecutionMode::Field),
            missing => Err(format!("agents = \"local\" needs {}", missing.join(", "))),
        },
        AgentMode::RemoteHands => match hands_missing {
            None => Ok(ExecutionMode::RemoteHands),
            Some(missing) => Err(format!("agents = \"remote-hands\" needs {missing}")),
        },
        AgentMode::Auto => {
            if field_missing.is_empty() {
                Ok(ExecutionMode::Field)
            } else if hands_missing.is_none() {
                Ok(ExecutionMode::RemoteHands)
            } else {
                Err(reasons(&field_missing, hands_missing))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::m::theaters::{SshAccess, Theater, TheaterAccess};

    fn agent(id: &str, handler: &str, priority: i64) -> AgentFacts {
        AgentFacts {
            id: id.to_owned(),
            handler: Some(handler.to_owned()),
            priority,
            roles: vec!["case-officer".to_owned()],
            tags: Vec::new(),
        }
    }

    fn station(id: &str, load: usize) -> StationFacts {
        StationFacts {
            station: id.to_owned(),
            healthy: true,
            silent_for: 1,
            roles: vec!["execution".to_owned()],
            capacity: 2,
            load,
            labels: BTreeMap::from([("station".to_owned(), id.to_owned())]),
            checkouts: vec!["G".to_owned()],
            handlers: Some(vec!["claude".to_owned(), "codex".to_owned()]),
            theaters: vec![TheaterSummary::local()],
        }
    }

    fn policy() -> OperationPolicy {
        let mut policy = OperationPolicy::new("G");
        policy.checkout = true;
        policy.agents = vec![agent("007", "claude", 10), agent("008", "codex", 5)];
        policy
    }

    fn reasons(placement: &Placement, station: &str) -> Vec<String> {
        placement
            .rejections
            .iter()
            .filter(|r| r.station.as_deref() == Some(station))
            .map(|r| r.reason.clone())
            .collect()
    }

    #[test]
    fn orders_least_loaded_then_deciding_station_then_agent_priority() {
        let stations = [station("a", 1), station("b", 0), station("c", 0)];
        let request = PlacementRequest {
            deciding_station: Some("c".to_owned()),
            ..PlacementRequest::default()
        };
        let placement = place(&policy(), &request, &stations);
        let order: Vec<_> = placement
            .choices
            .iter()
            .map(|c| (c.station.as_str(), c.agent.as_str()))
            .collect();
        assert_eq!(
            order,
            [
                ("c", "007"),
                ("c", "008"),
                ("b", "007"),
                ("b", "008"),
                ("a", "007"),
                ("a", "008"),
            ]
        );
        assert_eq!(placement.best().unwrap().theater, "local");
        assert!(placement.rejections.is_empty());
    }

    #[test]
    fn every_station_level_rejection_carries_its_reason() {
        let mut dark = station("dark", 0);
        dark.healthy = false;
        dark.silent_for = 90;
        let mut control = station("control", 0);
        control.roles = vec!["control".to_owned()];
        let mut full = station("full", 2);
        full.capacity = 2;
        let mut none = station("none", 0);
        none.capacity = 0;
        let mut no_checkout = station("bare", 0);
        no_checkout.checkouts.clear();
        let mut gpu = station("cpu", 0);
        gpu.labels.insert("gpu".to_owned(), "no".to_owned());

        let mut policy = policy();
        policy
            .station_labels
            .insert("gpu".to_owned(), "yes".to_owned());
        let mut ok = station("ok", 0);
        ok.labels.insert("gpu".to_owned(), "yes".to_owned());
        let stations = [dark, control, full, none, no_checkout, gpu, ok];
        let placement = place(&policy, &PlacementRequest::default(), &stations);

        assert_eq!(
            placement
                .choices
                .iter()
                .map(|c| c.station.as_str())
                .collect::<Vec<_>>(),
            ["ok", "ok"]
        );
        assert_eq!(
            reasons(&placement, "dark"),
            ["it has not checked in for 90 seconds (gone dark)"]
        );
        assert_eq!(reasons(&placement, "control"), ["it has no execution role"]);
        assert_eq!(
            reasons(&placement, "full"),
            ["it is at capacity (2 of 2 Missions)"]
        );
        assert_eq!(
            reasons(&placement, "none"),
            ["it accepts no Missions (capacity 0)"]
        );
        assert_eq!(
            reasons(&placement, "bare"),
            ["labels do not satisfy station_labels: gpu=yes (not set)"]
        );
        assert_eq!(
            reasons(&placement, "cpu"),
            ["labels do not satisfy station_labels: gpu=yes (has \"no\")"]
        );
    }

    #[test]
    fn a_missing_checkout_is_a_reason() {
        let mut bare = station("bare", 0);
        bare.checkouts.clear();
        let placement = place(&policy(), &PlacementRequest::default(), &[bare]);
        assert!(placement.choices.is_empty());
        assert_eq!(
            reasons(&placement, "bare"),
            ["this Station has no checkout of Operation \"G\""]
        );
    }

    #[test]
    fn a_mission_can_narrow_the_stations_list_but_never_widen_it() {
        let stations = [station("a", 0), station("b", 0), station("c", 0)];
        let mut policy = policy();
        policy.stations = vec!["a".to_owned(), "b".to_owned()];

        let open = place(&policy, &PlacementRequest::default(), &stations);
        let chosen: Vec<_> = open.choices.iter().map(|c| c.station.as_str()).collect();
        assert!(chosen.contains(&"a") && chosen.contains(&"b") && !chosen.contains(&"c"));
        assert_eq!(
            reasons(&open, "c"),
            ["the Operation allows only Stations [a, b]"]
        );

        let narrowed = place(
            &policy,
            &PlacementRequest {
                stations: vec!["b".to_owned(), "c".to_owned()],
                ..PlacementRequest::default()
            },
            &stations,
        );
        let chosen: Vec<_> = narrowed
            .choices
            .iter()
            .map(|c| c.station.as_str())
            .collect();
        assert_eq!(chosen, ["b", "b"], "c is outside the Operation's list");
        assert_eq!(
            reasons(&narrowed, "a"),
            ["the Mission narrows placement to Stations [b, c]"]
        );
        assert_eq!(
            reasons(&narrowed, "c"),
            ["the Operation allows only Stations [a, b]"],
            "the Operation's list is reported first, so a Mission cannot widen it"
        );
    }

    #[test]
    fn a_backup_agent_stands_in_where_the_first_handler_is_missing() {
        let mut codex_only = station("a", 0);
        codex_only.handlers = Some(vec!["codex".to_owned()]);
        let placement = place(&policy(), &PlacementRequest::default(), &[codex_only]);
        assert_eq!(placement.choices.len(), 1);
        assert_eq!(placement.choices[0].agent, "008");
        assert_eq!(
            placement.rejections,
            [PlacementRejection {
                station: Some("a".to_owned()),
                theater: None,
                agent: Some("007".to_owned()),
                reason: "handler \"claude\" is not available on this Station".to_owned(),
            }]
        );
        // A Station that never reported handlers is not excluded for them.
        let mut unknown = station("b", 0);
        unknown.handlers = None;
        let placement = place(&policy(), &PlacementRequest::default(), &[unknown]);
        assert_eq!(placement.choices.len(), 2);
    }

    #[test]
    fn requests_filter_agents_and_explain_why() {
        let mut policy = policy();
        policy.agents.push(AgentFacts {
            handler: None,
            ..agent("009", "x", 1)
        });
        let placement = place(
            &policy,
            &PlacementRequest {
                agent: Some("008".to_owned()),
                ..PlacementRequest::default()
            },
            &[station("a", 0)],
        );
        assert_eq!(placement.choices.len(), 1);
        assert_eq!(placement.choices[0].agent, "008");
        let for_agent = |id: &str| {
            placement
                .rejections
                .iter()
                .find(|r| r.agent.as_deref() == Some(id) && r.station.is_none())
                .map(|r| r.reason.as_str())
        };
        assert_eq!(
            for_agent("007"),
            Some("the Mission asks for a different Agent")
        );
        assert_eq!(
            for_agent("009"),
            Some("the Mission asks for a different Agent")
        );

        let by_role = place(
            &policy,
            &PlacementRequest {
                role: Some("reviewer".to_owned()),
                tags: vec!["fast".to_owned()],
                ..PlacementRequest::default()
            },
            &[station("a", 0)],
        );
        assert!(by_role.choices.is_empty());
        assert!(by_role.rejections.iter().any(|r| {
            r.agent.as_deref() == Some("009") && r.reason == "it has no handler configured"
        }));
        assert!(by_role.rejections.iter().any(|r| {
            r.agent.as_deref() == Some("007")
                && r.reason == "it is not qualified for role \"reviewer\""
        }));
    }

    fn remote(id: &str, capabilities: Option<Capabilities>) -> TheaterSummary {
        let mut summary =
            Theater::new(id, TheaterAccess::Ssh(SshAccess::new("b.example"))).summary();
        summary.capabilities = capabilities;
        summary
    }

    fn runs() -> Capabilities {
        Capabilities {
            terminal: true,
            run: true,
            files: true,
            ..Capabilities::default()
        }
    }

    fn mode_of(theater: &TheaterSummary) -> Result<ExecutionMode, String> {
        execution_mode(theater)
    }

    #[test]
    fn theater_binding_labels_and_capabilities_decide_the_theater() {
        let mut ssh = remote("box", None);
        ssh.labels.insert("gpu".to_owned(), "yes".to_owned());
        let mut lab = Theater::new("lab", TheaterAccess::Local).summary();
        lab.labels.insert("gpu".to_owned(), "yes".to_owned());
        let mut multi = station("a", 0);
        multi.theaters.push(ssh);
        multi.theaters.push(lab);

        // Unbound: local first; a Theater whose transport reported nothing is rejected.
        let placement = place(&policy(), &PlacementRequest::default(), &[multi.clone()]);
        assert_eq!(placement.choices[0].theater, "local");
        assert_eq!(placement.choices[0].mode, ExecutionMode::Field);
        let reason = placement
            .rejections
            .iter()
            .find(|r| r.theater.as_deref() == Some("box"))
            .unwrap();
        assert_eq!(
            reason.reason,
            "the Station did not report what this Theater's transport can do"
        );

        // Bound to the ssh Theater: nothing is eligible, and each Theater says why.
        let mut bound = policy();
        bound.theater = Some("box".to_owned());
        let placement = place(&bound, &PlacementRequest::default(), &[multi.clone()]);
        assert!(placement.choices.is_empty());
        assert!(placement.rejections.iter().any(|r| {
            r.theater.as_deref() == Some("local")
                && r.reason == "the Operation runs only in Theater \"box\""
        }));

        // A Theater's labels satisfy station_labels the Station itself does not.
        let mut gpu = policy();
        gpu.station_labels
            .insert("gpu".to_owned(), "yes".to_owned());
        let placement = place(&gpu, &PlacementRequest::default(), &[multi]);
        let chosen: Vec<_> = placement
            .choices
            .iter()
            .map(|c| c.theater.as_str())
            .collect();
        assert_eq!(chosen, ["lab", "lab"]);
    }

    #[test]
    fn auto_prefers_field_mode_on_the_stations_own_host_and_remote_hands_elsewhere() {
        assert_eq!(
            mode_of(&TheaterSummary::local()),
            Ok(ExecutionMode::Field),
            "an own host that reported nothing is assumed able"
        );
        assert_eq!(
            mode_of(&remote("box", Some(runs()))),
            Ok(ExecutionMode::RemoteHands),
            "a remote host with neither a runner nor a declared harness gets remote hands"
        );
        let run_only = Capabilities {
            run: true,
            ..Capabilities::default()
        };
        assert_eq!(
            mode_of(&remote("phone", Some(run_only))),
            Ok(ExecutionMode::RemoteHands)
        );
    }

    #[test]
    fn a_theater_that_cannot_run_commands_cannot_host_remote_hands() {
        let files_only = Capabilities {
            files: true,
            ..Capabilities::default()
        };
        let reason = mode_of(&remote("box", Some(files_only))).unwrap_err();
        assert!(reason.contains("remote-hands mode needs the `run` capability"));
        assert!(reason.contains("a runner on the Theater"));
    }

    #[test]
    fn declared_facts_force_a_mode_and_a_rejection_names_the_missing_capability() {
        let with = |mut summary: TheaterSummary, agents, egress| {
            summary.agents = agents;
            summary.egress = egress;
            summary
        };
        // The own host can be told to use remote hands, for a harness that must not touch it.
        assert_eq!(
            mode_of(&with(
                TheaterSummary::local(),
                AgentMode::RemoteHands,
                EgressKind::Direct
            )),
            Ok(ExecutionMode::RemoteHands)
        );
        // No route to the provider's API: field mode is out, remote hands stays.
        assert_eq!(
            mode_of(&with(
                TheaterSummary::local(),
                AgentMode::Auto,
                EgressKind::Isolated
            )),
            Ok(ExecutionMode::RemoteHands)
        );
        // `agents = "local"` is never silently downgraded.
        let reason = mode_of(&with(
            TheaterSummary::local(),
            AgentMode::Local,
            EgressKind::Isolated,
        ))
        .unwrap_err();
        assert_eq!(
            reason,
            "agents = \"local\" needs a route to the provider's API (egress is `none`)"
        );
        let reason = mode_of(&with(
            remote("box", Some(runs())),
            AgentMode::Local,
            EgressKind::Direct,
        ))
        .unwrap_err();
        assert!(reason.starts_with("agents = \"local\" needs a runner on the Theater"));
        // Egress through the Station relies on the transport's forward.
        let reason = mode_of(&with(
            TheaterSummary::local(),
            AgentMode::Local,
            EgressKind::Station,
        ))
        .unwrap_err();
        assert!(reason.contains("`forward` capability"));
        let mut forwards = TheaterSummary::local();
        forwards.egress = EgressKind::Station;
        forwards.capabilities = Some(Capabilities {
            forward: true,
            ..runs()
        });
        assert_eq!(mode_of(&forwards), Ok(ExecutionMode::Field));
        // `agents = "remote-hands"` needs `run`.
        let mut hands = remote("box", Some(Capabilities::default()));
        hands.agents = AgentMode::RemoteHands;
        assert_eq!(
            mode_of(&hands).unwrap_err(),
            "agents = \"remote-hands\" needs the `run` capability"
        );
    }

    #[test]
    fn remote_hands_choices_carry_their_mode_and_need_to_know_where_to_work() {
        let mut box_theater = remote("box", Some(runs()));
        box_theater.labels.insert("zone".to_owned(), "b".to_owned());
        let mut station = station("a", 0);
        station.theaters.push(box_theater);

        // The Operation has a checkout on the Station; the Theater is not bound and has no
        // `workdir`, so the Mission would not know where to work.
        let placement = place(&policy(), &PlacementRequest::default(), &[station.clone()]);
        assert!(placement.choices.iter().all(|c| c.theater == "local"));
        assert!(placement.rejections.iter().any(|r| {
            r.theater.as_deref() == Some("box") && r.reason.contains("has no `workdir`")
        }));

        // Bound to the Theater, the Operation's path is the working directory there.
        let mut bound = policy();
        bound.theater = Some("box".to_owned());
        let placement = place(&bound, &PlacementRequest::default(), &[station.clone()]);
        let best = placement.best().unwrap();
        assert_eq!(
            (best.theater.as_str(), best.mode),
            ("box", ExecutionMode::RemoteHands)
        );

        // A `workdir` on the Theater is enough for an unbound Operation.
        station.theaters.last_mut().unwrap().workdir = Some("/srv/work".to_owned());
        let placement = place(&policy(), &PlacementRequest::default(), &[station.clone()]);
        assert!(placement.choices.iter().any(|c| c.theater == "box"));

        // The Station's own checkout is irrelevant in remote-hands mode.
        station.checkouts.clear();
        let placement = place(&bound, &PlacementRequest::default(), &[station]);
        assert_eq!(placement.best().unwrap().theater, "box");
    }

    #[test]
    fn a_remote_host_hosts_the_harness_with_a_runner_or_the_detached_wrapper() {
        let local_only = |mut summary: TheaterSummary| {
            summary.agents = AgentMode::Local;
            summary
        };
        // A runner on the Station's own host brings its own terminal provider.
        let mut own_runner = TheaterSummary::local();
        own_runner.runner = true;
        own_runner.capabilities = Some(Capabilities::default());
        assert_eq!(mode_of(&own_runner), Ok(ExecutionMode::Field));

        // With a runner, `run` (to reach it) is all field mode needs from the transport.
        let mut with_runner = remote(
            "box",
            Some(Capabilities {
                run: true,
                ..Capabilities::default()
            }),
        );
        with_runner.runner = true;
        assert_eq!(mode_of(&with_runner), Ok(ExecutionMode::Field));
        let mut unreachable_runner = remote(
            "box",
            Some(Capabilities {
                files: true,
                ..Capabilities::default()
            }),
        );
        unreachable_runner.runner = true;
        assert_eq!(
            mode_of(&local_only(unreachable_runner)).unwrap_err(),
            "agents = \"local\" needs the `run` capability to reach its runner"
        );

        // Without one, a POSIX host with `run`, `files` and a declared harness gets the
        // detached wrapper.
        let posix = Capabilities {
            os: Some("linux".to_owned()),
            ..runs()
        };
        let mut wrapper = remote("box", Some(posix.clone()));
        wrapper.harnesses = vec!["claude".to_owned()];
        assert_eq!(mode_of(&wrapper), Ok(ExecutionMode::Field));

        let mut windows = wrapper.clone();
        windows.capabilities = Some(Capabilities {
            os: Some("windows".to_owned()),
            ..posix.clone()
        });
        assert_eq!(mode_of(&windows), Ok(ExecutionMode::RemoteHands));
        let reason = mode_of(&local_only(windows)).unwrap_err();
        assert!(
            reason.contains("for the detached wrapper a POSIX host"),
            "{reason}"
        );

        let mut unknown = wrapper.clone();
        unknown.capabilities = Some(runs());
        let reason = mode_of(&local_only(unknown)).unwrap_err();
        assert!(reason.contains("has not reported its os"), "{reason}");

        let mut undeclared = wrapper;
        undeclared.harnesses.clear();
        let reason = mode_of(&local_only(undeclared)).unwrap_err();
        assert_eq!(
            reason,
            "agents = \"local\" needs a runner on the Theater (`runner`), or for the detached \
             wrapper a declared harness (`harnesses`)"
        );
    }

    #[test]
    fn field_mode_on_a_remote_host_works_where_the_theater_says() {
        let mut runner_box = remote(
            "box",
            Some(Capabilities {
                run: true,
                ..Capabilities::default()
            }),
        );
        runner_box.runner = true;
        let mut station = station("a", 0);
        station.theaters.push(runner_box);

        // The Station's checkout is not on the Theater: an unbound Operation needs `workdir`.
        let placement = place(&policy(), &PlacementRequest::default(), &[station.clone()]);
        assert!(placement.choices.iter().all(|c| c.theater == "local"));
        assert!(placement.rejections.iter().any(|r| {
            r.theater.as_deref() == Some("box") && r.reason.contains("has no `workdir`")
        }));

        // Bound to the Theater, it runs there in field mode with no checkout on the Station.
        let mut bound = policy();
        bound.theater = Some("box".to_owned());
        station.checkouts.clear();
        let placement = place(&bound, &PlacementRequest::default(), &[station]);
        let best = placement.best().unwrap();
        assert_eq!(
            (best.theater.as_str(), best.mode),
            ("box", ExecutionMode::Field)
        );
    }

    #[test]
    fn an_older_choice_without_a_mode_reads_as_field() {
        let choice: PlacementChoice = serde_json::from_value(serde_json::json!({
            "station": "a", "theater": "local", "agent": "007", "handler": "claude"
        }))
        .unwrap();
        assert_eq!(choice.mode, ExecutionMode::Field);
    }

    #[test]
    fn label_patterns_are_globs() {
        let mut policy = policy();
        policy
            .station_labels
            .insert("station".to_owned(), "build-*".to_owned());
        let placement = place(
            &policy,
            &PlacementRequest::default(),
            &[station("build-1", 0), station("dev-1", 0)],
        );
        assert!(placement.choices.iter().all(|c| c.station == "build-1"));
        assert_eq!(
            reasons(&placement, "dev-1"),
            ["labels do not satisfy station_labels: station=build-* (has \"dev-1\")"]
        );
    }
}
