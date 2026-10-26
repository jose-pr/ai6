//! Placement: which Station, Theater and Agent may run an Operation's Mission, and why
//! the others may not.
//!
//! M decides placement from plain facts: the Operation's policy, what each Station last
//! checked in, and the Agents posted to the Operation. [`place`] returns every eligible
//! (Station, Theater, Agent) choice, best first, and one reason for every rejection, so
//! "why did or didn't it run there" can be answered before anything launches.
//!
//! A Station or Theater short of memory or CPU ([`ResourceLimits`]) is ruled out like any
//! other. Each remaining (Station, Theater) gets a load score from 0 (idle) to 4, the sum of
//! four shares that each run 0 to 1: Missions held against capacity, CPU busy, memory in
//! use, and the Station's Sessions on that Theater against capacity. A figure that was not
//! reported adds nothing. [`PlacementStrategy::Spread`] puts the lowest score first,
//! [`PlacementStrategy::Pack`] the highest. Equal scores keep the order without scores: the
//! least-loaded Station first (the deciding Station wins a tie, then Station ID), the `local`
//! Theater before others, then the Agent with the highest priority.

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
    /// Whether its Missions go where there is most room or fill busy places first.
    #[serde(default)]
    pub placement: PlacementStrategy,
}

/// How placement orders the places that pass (Operation key `placement`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlacementStrategy {
    /// The least-loaded place first, to share work out.
    #[default]
    Spread,
    /// The busiest place that still passes first, to keep others free.
    Pack,
}

impl PlacementStrategy {
    /// The name used in configuration and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spread => "spread",
            Self::Pack => "pack",
        }
    }
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
            placement: PlacementStrategy::Spread,
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
            placement: operation.placement,
        })
    }
}

/// CPU and memory of a Station's host or a Theater, as last sampled. A value the host did
/// not report is `None`, never a guess.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Resources {
    /// CPUs available: the host's count, or a fraction under a container's CPU quota.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<f64>,
    /// How busy the CPUs were since the previous sample, 0 to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f64>,
    /// The one-minute load average; Windows has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_1m: Option<f64>,
    /// Memory in MiB: the host's, or a container's memory limit when lower.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_total_mb: Option<u64>,
    /// Memory in MiB that new work can still use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_available_mb: Option<u64>,
    /// When it was sampled, as Unix seconds.
    #[serde(default)]
    pub sampled_at: u64,
}

// Every value is finite: JSON cannot carry NaN or infinity, and the samplers drop them.
impl Eq for Resources {}

impl Resources {
    /// How busy the CPUs are, 0 to 1: the CPU percent, else the load average per CPU, else
    /// `None`.
    pub fn cpu_busy(&self) -> Option<f64> {
        self.cpu_percent
            .map(|percent| percent / 100.0)
            .or_else(|| {
                let load = self.load_1m?;
                let cpus = self.cpus.filter(|cpus| *cpus > 0.0)?;
                Some(load / cpus)
            })
            .map(|busy| busy.clamp(0.0, 1.0))
    }

    /// The share of memory in use, 0 to 1, when both totals are known.
    pub fn memory_used(&self) -> Option<f64> {
        let total = self.memory_total_mb.filter(|total| *total > 0)?;
        let available = self.memory_available_mb?.min(total);
        Some((total - available) as f64 / total as f64)
    }
}

/// The default floor of free memory, in MiB, below which a Station or Theater takes no new
/// Missions.
pub const DEFAULT_MIN_FREE_MEMORY_MB: u64 = 512;
/// The default CPU percent above which a Station or Theater takes no new Missions.
pub const DEFAULT_MAX_CPU_PERCENT: u32 = 95;

/// When a Station, and each Theater it reaches, is too short of resources to take new
/// Missions. A figure that was not reported never rejects: a Theater's CPU percent, which
/// the probe does not measure, never does.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Rejects when less memory than this, in MiB, is available; 0 never rejects.
    #[serde(default = "default_min_free_memory_mb")]
    pub min_free_memory_mb: u64,
    /// Rejects when the CPU is busier than this percent; 100 never rejects.
    #[serde(default = "default_max_cpu_percent")]
    pub max_cpu_percent: u32,
}

fn default_min_free_memory_mb() -> u64 {
    DEFAULT_MIN_FREE_MEMORY_MB
}

fn default_max_cpu_percent() -> u32 {
    DEFAULT_MAX_CPU_PERCENT
}

impl Default for ResourceLimits {
    /// [`DEFAULT_MIN_FREE_MEMORY_MB`] and [`DEFAULT_MAX_CPU_PERCENT`].
    fn default() -> Self {
        Self {
            min_free_memory_mb: DEFAULT_MIN_FREE_MEMORY_MB,
            max_cpu_percent: DEFAULT_MAX_CPU_PERCENT,
        }
    }
}

impl ResourceLimits {
    /// Limits with the given floor of free memory, in MiB, and ceiling of CPU percent.
    pub fn new(min_free_memory_mb: u64, max_cpu_percent: u32) -> Self {
        Self {
            min_free_memory_mb,
            max_cpu_percent,
        }
    }

    /// Why `resources` rule out new work, if they do.
    pub fn rejection(&self, resources: &Resources) -> Option<String> {
        if let Some(available) = resources.memory_available_mb
            && available < self.min_free_memory_mb
        {
            return Some(format!(
                "it has {available} MiB of memory free, below min_free_memory_mb = {}",
                self.min_free_memory_mb
            ));
        }
        if let Some(percent) = resources.cpu_percent
            && percent > f64::from(self.max_cpu_percent)
        {
            return Some(format!(
                "its CPU is {percent:.0}% busy, above max_cpu_percent = {}",
                self.max_cpu_percent
            ));
        }
        None
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
    /// Sessions it reported running, including those no brief lease accounts for.
    #[serde(default)]
    pub running: usize,
    /// Its labels.
    pub labels: BTreeMap<String, String>,
    /// Operations whose checkout it has.
    pub checkouts: Vec<String>,
    /// AgentHandler providers it can run; `None` when it did not report them.
    pub handlers: Option<Vec<String>>,
    /// Theaters it reaches.
    pub theaters: Vec<TheaterSummary>,
    /// Its host's CPU and memory; `None` from a Station that did not report them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Resources>,
    /// When it and its Theaters are too short of resources for new Missions.
    #[serde(default)]
    pub limits: ResourceLimits,
}

impl StationFacts {
    /// How many Missions it holds: its leases or its running Sessions, whichever is more.
    pub fn occupied(&self) -> usize {
        self.load.max(self.running)
    }

    /// Reads a Registry check-in. `load` is how many Missions are already leased to the
    /// Station; a Station silent for longer than `window` is unhealthy.
    ///
    /// Check-ins written before Stations reported a key fall back to the permissive
    /// reading: execution role, capacity one, nothing running, own host only, handlers and
    /// resources unknown, default resource limits.
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
            running: info
                .get("running")
                .and_then(serde_json::Value::as_u64)
                .and_then(|running| usize::try_from(running).ok())
                .unwrap_or_default(),
            labels,
            checkouts: strings("operations").unwrap_or_default(),
            handlers: strings("handlers"),
            theaters,
            resources: info
                .get("resources")
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
            limits: info
                .get("resource_limits")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_default(),
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// How loaded the Station and Theater are, 0 (idle) to 4; see the module docs.
    #[serde(default)]
    pub score: f64,
}

// The score is a finite sum of finite shares.
impl Eq for PlacementChoice {}

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
        (
            a.occupied(),
            Some(a.station.as_str()) != deciding,
            &a.station,
        )
            .cmp(&(
                b.occupied(),
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
            let score = load_score(station, theater);
            for agent in &usable {
                let handler = agent.handler.as_deref().unwrap_or_default();
                if let Err(reason) = harness_verdict(theater, mode, handler) {
                    placement.rejections.push(PlacementRejection {
                        station: Some(station.station.clone()),
                        theater: Some(theater.id.clone()),
                        agent: Some(agent.id.clone()),
                        reason,
                    });
                    continue;
                }
                placement.choices.push(PlacementChoice {
                    station: station.station.clone(),
                    theater: theater.id.clone(),
                    agent: agent.id.clone(),
                    handler: agent.handler.clone(),
                    mode,
                    score,
                });
            }
        }
    }
    // Stable sorts: equal scores keep the order the loops above produced.
    match policy.placement {
        PlacementStrategy::Spread => placement
            .choices
            .sort_by(|a, b| a.score.total_cmp(&b.score)),
        PlacementStrategy::Pack => placement
            .choices
            .sort_by(|a, b| b.score.total_cmp(&a.score)),
    }
    placement
}

/// How loaded running on `theater` through `station` is, 0 (idle) to 4: Missions held
/// against capacity, CPU busy, memory in use, and the Station's Sessions on the Theater
/// against capacity, each 0 to 1. A Theater on the Station's own host is judged by the
/// Station's sample. A figure that was not reported adds nothing.
fn load_score(station: &StationFacts, theater: &TheaterSummary) -> f64 {
    let capacity = station.capacity.max(1) as f64;
    let held = (station.occupied() as f64 / capacity).min(1.0);
    let sessions = (theater.sessions as f64 / capacity).min(1.0);
    let resources = if theater.access == TheaterKind::Local {
        station.resources.as_ref().or(theater.resources.as_ref())
    } else {
        theater.resources.as_ref()
    };
    let cpu = resources.and_then(Resources::cpu_busy).unwrap_or_default();
    let memory = resources
        .and_then(Resources::memory_used)
        .unwrap_or_default();
    // Thousandths keep the score readable and equal loads equal.
    ((held + cpu + memory + sessions) * 1000.0).round() / 1000.0
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
    if station.occupied() >= station.capacity {
        return Some(format!(
            "it is at capacity ({} of {} Missions)",
            station.occupied(),
            station.capacity
        ));
    }
    station
        .resources
        .as_ref()
        .and_then(|resources| station.limits.rejection(resources))
}

/// How a Session would run on `theater`, or the reason it may not.
fn theater_verdict(
    policy: &OperationPolicy,
    station: &StationFacts,
    theater: &TheaterSummary,
) -> Result<ExecutionMode, String> {
    if let Some(mission) = &theater.made_for {
        return Err(format!("it was made for Mission {mission:?} alone"));
    }
    if let Some(bound) = &policy.theater
        && bound != &theater.id
    {
        return Err(format!("the Operation runs only in Theater {bound:?}"));
    }
    let mode = execution_mode(theater)?;
    fitness(theater)?;
    // The Station's own host was judged with the Station.
    if theater.access != TheaterKind::Local
        && let Some(reason) = theater
            .resources
            .as_ref()
            .and_then(|resources| station.limits.rejection(resources))
    {
        return Err(reason);
    }
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

/// Why the Theater's declared requirements rule it out, if they do: they have not been
/// checked, or a requirement other than a harness is unmet. A Theater that declares none is
/// not checked, nor is an ephemeral template, whose Theaters are fitted out when they are
/// made; harnesses are judged per Agent by [`harness_verdict`].
pub fn fitness(theater: &TheaterSummary) -> Result<(), String> {
    let Some(requires) = theater
        .requires
        .as_ref()
        .filter(|_| theater.access != TheaterKind::Ephemeral)
    else {
        return Ok(());
    };
    let report = theater.report.as_ref().ok_or_else(|| {
        "its requirements have not been checked yet: the Station's probe has not finished"
            .to_owned()
    })?;
    if let Some(error) = &report.error {
        return Err(format!("its requirements could not be checked: {error}"));
    }
    let unmet: Vec<String> = report
        .unmet
        .iter()
        .filter(|unmet| !requires.harnesses.iter().any(|h| h.name() == unmet.name))
        .map(ToString::to_string)
        .collect();
    if unmet.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "it does not meet its requirements: {}",
            unmet.join("; ")
        ))
    }
}

/// Why an Agent whose handler is `handler` cannot run on the Theater in `mode`, if it cannot:
/// in field mode its harness runs there, so a Theater that lists its harnesses must list this
/// one and have it installed. Remote-hands mode runs the harness on the Station.
pub fn harness_verdict(
    theater: &TheaterSummary,
    mode: ExecutionMode,
    handler: &str,
) -> Result<(), String> {
    let Some(requires) = theater
        .requires
        .as_ref()
        .filter(|_| mode == ExecutionMode::Field)
    else {
        return Ok(());
    };
    if !requires.allows_harness(handler) {
        return Err(format!(
            "harness {handler:?} is not one of the Theater's requires.harnesses"
        ));
    }
    let unmet = theater
        .report
        .as_ref()
        .and_then(|report| report.unmet.iter().find(|unmet| unmet.name == handler));
    match unmet {
        Some(unmet) => Err(format!(
            "harness {handler:?} is missing on this Theater: {unmet}"
        )),
        None => Ok(()),
    }
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
            running: 0,
            labels: BTreeMap::from([("station".to_owned(), id.to_owned())]),
            checkouts: vec!["G".to_owned()],
            handlers: Some(vec!["claude".to_owned(), "codex".to_owned()]),
            theaters: vec![TheaterSummary::local()],
            resources: None,
            limits: ResourceLimits::default(),
        }
    }

    #[test]
    fn a_check_in_carries_resources_and_one_without_them_reads_as_unknown() {
        use crate::directorates::registry::backend::StationStore;
        use crate::directorates::registry::store::FileRegistry;
        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        let info = |resources: Option<serde_json::Value>| {
            let mut info = BTreeMap::new();
            if let Some(resources) = resources {
                info.insert("resources".to_owned(), resources);
            }
            info
        };
        registry
            .check_in(
                "new",
                info(Some(serde_json::json!({
                    "cpus": 8.0, "cpu_percent": 12.5, "memory_total_mb": 16384,
                    "memory_available_mb": 9000, "sampled_at": 1_700_000_000
                }))),
            )
            .unwrap();
        registry.check_in("old", info(None)).unwrap();
        registry
            .check_in("odd", info(Some(serde_json::json!("not resources"))))
            .unwrap();
        let facts: BTreeMap<String, StationFacts> = registry
            .stations(Duration::MAX)
            .unwrap()
            .iter()
            .map(|record| {
                let facts = StationFacts::from_record(record, record.seen(), Duration::MAX, 0);
                (facts.station.clone(), facts)
            })
            .collect();
        let resources = facts["new"].resources.as_ref().unwrap();
        assert_eq!(resources.cpus, Some(8.0));
        assert_eq!(resources.load_1m, None, "an absent value stays unknown");
        assert_eq!(resources.memory_used(), Some((16384.0 - 9000.0) / 16384.0));
        assert_eq!(resources.cpu_busy(), Some(0.125));
        assert_eq!(facts["old"].resources, None);
        assert_eq!(facts["odd"].resources, None, "a malformed value is ignored");
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
    fn a_theater_missing_a_harness_is_not_eligible_for_its_agents_and_says_why() {
        use crate::directorates::q::provision::{ProbeReport, Requirements};
        let requires = Requirements::default()
            .with_harness("claude>=2.1".parse().unwrap())
            .with_tool("git".parse().unwrap());
        let mut lab = Theater::new("lab", TheaterAccess::Local)
            .with_requirements(requires.clone())
            .summary();
        let probed = |found: &[(&str, &str)]| {
            let mut report = ProbeReport {
                found: found
                    .iter()
                    .map(|(name, version)| ((*name).to_owned(), (*version).to_owned()))
                    .collect(),
                ..ProbeReport::default()
            };
            report.unmet = requires.check(&report);
            report
        };
        let mut station = station("a", 0);
        station.theaters = vec![lab.clone()];
        let rejected = |placement: &Placement| {
            placement
                .rejections
                .iter()
                .map(|r| (r.theater.clone(), r.agent.clone(), r.reason.clone()))
                .collect::<Vec<_>>()
        };

        // Not probed yet: the whole Theater waits for its report.
        let placement = place(&policy(), &PlacementRequest::default(), &[station.clone()]);
        assert!(placement.choices.is_empty());
        assert_eq!(
            rejected(&placement),
            [(
                Some("lab".to_owned()),
                None,
                "its requirements have not been checked yet: the Station's probe has not finished"
                    .to_owned()
            )]
        );

        // The harness is missing: 007 (claude) is ruled out there with the reason, and 008's
        // codex is not a harness the Theater provides.
        lab.report = Some(probed(&[("git", "git version 2.47.0")]));
        station.theaters = vec![lab.clone()];
        let placement = place(&policy(), &PlacementRequest::default(), &[station.clone()]);
        assert!(placement.choices.is_empty());
        assert_eq!(
            rejected(&placement),
            [
                (
                    Some("lab".to_owned()),
                    Some("007".to_owned()),
                    "harness \"claude\" is missing on this Theater: claude>=2.1: not installed"
                        .to_owned()
                ),
                (
                    Some("lab".to_owned()),
                    Some("008".to_owned()),
                    "harness \"codex\" is not one of the Theater's requires.harnesses".to_owned()
                ),
            ]
        );

        // A missing tool rules out the Theater for everyone.
        lab.report = Some(probed(&[("claude", "2.1.310 (Claude Code)")]));
        station.theaters = vec![lab.clone()];
        let placement = place(&policy(), &PlacementRequest::default(), &[station.clone()]);
        assert_eq!(
            rejected(&placement)[0].2,
            "it does not meet its requirements: git: not installed"
        );

        // Fit: 007 runs there.
        lab.report = Some(probed(&[
            ("claude", "2.1.310"),
            ("git", "git version 2.47.0"),
        ]));
        station.theaters = vec![lab.clone()];
        let placement = place(&policy(), &PlacementRequest::default(), &[station.clone()]);
        assert_eq!(placement.choices.len(), 1);
        assert_eq!(placement.choices[0].agent, "007");

        // In remote-hands mode the harness runs on the Station, so the Theater's does not
        // matter.
        lab.agents = AgentMode::RemoteHands;
        lab.workdir = Some("/srv/work".to_owned());
        lab.capabilities = Some(Capabilities {
            run: true,
            ..Capabilities::default()
        });
        lab.report = Some(probed(&[("git", "git version 2.47.0")]));
        station.theaters = vec![lab];
        let placement = place(&policy(), &PlacementRequest::default(), &[station]);
        assert_eq!(placement.choices.len(), 2);
    }

    #[test]
    fn an_ephemeral_template_is_placed_and_a_theater_made_from_it_is_not() {
        use crate::directorates::m::theaters::EphemeralAccess;
        use crate::directorates::q::provision::Requirements;
        let mut template = Theater::new(
            "scratch",
            TheaterAccess::Ephemeral(EphemeralAccess::new("container")),
        )
        .with_requirements(Requirements::default().with_tool("git".parse().unwrap()))
        .summary();
        template.capabilities = Some(Capabilities {
            run: true,
            files: true,
            ..Capabilities::default()
        });
        let mut made = remote("scratch-m1", template.capabilities.clone());
        made.made_for = Some("m1".to_owned());
        let mut station = station("a", 0);
        station.theaters = vec![template, made];
        let mut bound = policy();
        bound.theater = Some("scratch".to_owned());
        let placement = place(&bound, &PlacementRequest::default(), &[station.clone()]);
        let best = placement.best().unwrap();
        assert_eq!(
            (best.theater.as_str(), best.mode),
            ("scratch", ExecutionMode::RemoteHands),
            "a template's requirements are met when a Theater is made from it"
        );
        let placement = place(&policy(), &PlacementRequest::default(), &[station]);
        assert!(placement.rejections.iter().any(|r| {
            r.theater.as_deref() == Some("scratch-m1")
                && r.reason == "it was made for Mission \"m1\" alone"
        }));
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

    fn sampled(cpu_percent: f64, total_mb: u64, available_mb: u64) -> Resources {
        Resources {
            cpus: Some(8.0),
            cpu_percent: Some(cpu_percent),
            load_1m: None,
            memory_total_mb: Some(total_mb),
            memory_available_mb: Some(available_mb),
            sampled_at: 1_700_000_000,
        }
    }

    fn first_station(placement: &Placement) -> &str {
        placement.best().map(|c| c.station.as_str()).unwrap()
    }

    #[test]
    fn a_busy_station_loses_under_spread_and_wins_under_pack() {
        let mut busy = station("busy", 1);
        busy.resources = Some(sampled(80.0, 16_384, 4_096));
        let mut idle = station("idle", 0);
        idle.resources = Some(sampled(5.0, 16_384, 15_000));
        let stations = [busy, idle];

        let spread = place(&policy(), &PlacementRequest::default(), &stations);
        assert_eq!(first_station(&spread), "idle");
        let scores: Vec<_> = spread.choices.iter().map(|c| c.score).collect();
        assert!(
            scores.windows(2).all(|pair| pair[0] <= pair[1]),
            "{scores:?}"
        );
        // busy: half its capacity held, 80% CPU, 75% memory used.
        let busy_score = spread.choices.last().unwrap().score;
        assert!((2.0..=2.1).contains(&busy_score), "{busy_score}");

        let mut pack = policy();
        pack.placement = PlacementStrategy::Pack;
        let packed = place(&pack, &PlacementRequest::default(), &stations);
        assert_eq!(first_station(&packed), "busy");
        assert_eq!(packed.choices.len(), spread.choices.len());
        assert!(packed.rejections.is_empty());
    }

    #[test]
    fn stations_short_of_memory_or_cpu_are_rejected_with_the_reason() {
        let mut short = station("short", 0);
        short.resources = Some(sampled(10.0, 8_192, 300));
        let mut hot = station("hot", 0);
        hot.resources = Some(sampled(99.0, 8_192, 6_000));
        let mut loose = station("loose", 0);
        loose.resources = Some(sampled(99.0, 8_192, 300));
        loose.limits = ResourceLimits::new(0, 100);
        let placement = place(
            &policy(),
            &PlacementRequest::default(),
            &[short, hot, loose],
        );
        assert_eq!(
            reasons(&placement, "short"),
            ["it has 300 MiB of memory free, below min_free_memory_mb = 512"]
        );
        assert_eq!(
            reasons(&placement, "hot"),
            ["its CPU is 99% busy, above max_cpu_percent = 95"]
        );
        assert!(
            placement.choices.iter().all(|c| c.station == "loose"),
            "a Station's own limits can be loosened"
        );
    }

    #[test]
    fn a_theater_short_of_memory_is_rejected_and_its_sessions_count_against_it() {
        let runs_there = |id: &str| {
            let mut theater = remote(
                id,
                Some(Capabilities {
                    run: true,
                    ..Capabilities::default()
                }),
            );
            theater.runner = true;
            theater.workdir = Some("/srv/work".to_owned());
            theater
        };
        let mut small = runs_there("small");
        small.resources = Some(Resources {
            memory_total_mb: Some(256),
            memory_available_mb: Some(200),
            ..Resources::default()
        });
        // "busy" sorts before "free", so only its Session puts it second.
        let mut busy = runs_there("busy");
        busy.sessions = 1;
        let free = runs_there("free");
        let mut station = station("a", 0);
        station.theaters = vec![small, busy, free];
        let mut policy = policy();
        policy.checkout = false;
        policy.agents.truncate(1);

        let placement = place(&policy, &PlacementRequest::default(), &[station]);
        assert!(placement.rejections.iter().any(|r| {
            r.theater.as_deref() == Some("small")
                && r.reason == "it has 200 MiB of memory free, below min_free_memory_mb = 512"
        }));
        let order: Vec<_> = placement
            .choices
            .iter()
            .map(|c| c.theater.as_str())
            .collect();
        assert_eq!(order, ["free", "busy"]);
    }

    #[test]
    fn stations_that_report_no_resources_are_ordered_as_before_and_running_counts() {
        let mut quiet = station("quiet", 0);
        quiet.running = 2;
        let placement = place(
            &policy(),
            &PlacementRequest::default(),
            &[quiet, station("b", 1), station("c", 0)],
        );
        let order: Vec<_> = placement
            .choices
            .iter()
            .map(|c| (c.station.as_str(), c.score))
            .collect();
        assert_eq!(
            order,
            [("c", 0.0), ("c", 0.0), ("b", 0.5), ("b", 0.5)],
            "lease ratio only"
        );
        assert_eq!(
            reasons(&placement, "quiet"),
            ["it is at capacity (2 of 2 Missions)"],
            "Sessions it reported running fill it even without leases"
        );
    }
}
