//! Registered Operations and the Agents posted to them.
//!
//! A [`Roster`] comes from `operations.toml` and `agents.toml` ([`Roster::load`]) or from
//! typed records built in code ([`Roster::from_parts`], [`Roster::builder`]). Either way, an
//! Operation's `.ai6/agents.toml` is considered only after that Operation is found in the
//! roster.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::directorates::m::approvals::{GreenInkDefinition, GreenInkPolicy};
use crate::directorates::m::assistant::AssistantSettings;
use crate::directorates::m::cover::{Cover, CoverError, Covers};
use crate::directorates::m::harness::HarnessPolicy;
use crate::directorates::m::limits::{DEFAULT_LIMIT_FALLBACK_SECS, OnLimit};
use crate::directorates::m::placement::PlacementStrategy;
use crate::directorates::m::promotion::{PromotionDefinition, PromotionPolicy};
use crate::directorates::m::recovery::DEFAULT_LOST_AFTER_SECS;
use crate::directorates::m::vetting::{VettingPolicy, policy_from_toml};
use crate::directorates::medical::exposure::{ExposureDefinition, ExposurePolicy};
use crate::directorates::medical::quarantine::{QuarantineDefinition, QuarantinePolicy};
use crate::directorates::medical::usage::{Budget, BudgetDefinition, utc_day};
use crate::directorates::medical::watchdog::WatchdogOverrides;
use crate::directorates::registry::clearance::ClearanceGrant;
use crate::directorates::registry::sleepers::Sleeper;
use crate::directorates::registry::store::validate_station_id;

const AGENTS_FILE: &str = "agents.toml";
/// An Operation's own agents file, without its extension.
const AGENTS_STEM: &str = "agents";
const OPERATIONS_FILE: &str = "operations.toml";
/// The clearances, lowest first.
pub(crate) const CLEARANCES: &[&str] = &["desk", "field", "00"];

/// A resolved Agent configuration.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// The configured codename used by Missions.
    pub id: String,
    /// Human-readable purpose.
    pub description: String,
    /// Other names that resolve to this Agent.
    pub aliases: Vec<String>,
    /// Guatiao AgentHandler provider ID.
    pub handler: Option<String>,
    /// Preferred model, when configured.
    pub model: Option<String>,
    /// Preferred reasoning effort, when configured.
    pub effort: Option<String>,
    /// Mission roles this Agent can perform.
    pub roles: Vec<String>,
    /// Labels used when selecting an Agent for a Mission.
    pub tags: Vec<String>,
    /// Highest authority this Agent may exercise.
    pub clearance: String,
    /// Priority used when choosing among suitable Agents.
    pub priority: i64,
    /// Context established before the Mission prompt.
    pub context: String,
    /// Environment additions resolved from the roster hierarchy.
    pub env: BTreeMap<String, String>,
    /// Watchdog limits layered over the daemon settings.
    pub limits: WatchdogOverrides,
    /// Spending limits for this Agent's Sessions, layered over its Operation's.
    pub budget: Budget,
    /// Optional concurrent operation limit.
    pub max_concurrent: Option<usize>,
    /// The one account pool this Agent's Sessions draw from, when it is pinned to one.
    pub pool: Option<String>,
    /// The UTC date (`YYYY-MM-DD`) its model reaches end of life; from that day the Agent
    /// goes to desk duty or retires, as `on_retire` says.
    pub retire_on: Option<String>,
    /// What retirement does to it.
    pub on_retire: OnRetire,
    /// Days before `retire_on` that the Station warns of it; `0` does not warn.
    pub retire_warning_days: u32,
}

/// Days before an Agent's `retire_on` date that the Station warns of it, by default.
pub const DEFAULT_RETIRE_WARNING_DAYS: u32 = 14;

/// What happens to an Agent on its `retire_on` date (`on_retire`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnRetire {
    /// Its clearance is capped at `desk`; it is still chosen for work.
    #[default]
    Desk,
    /// It is never chosen again; its running Missions finish and queued ones go to the
    /// next qualified Agent.
    Retired,
}

impl OnRetire {
    /// The name used in configuration and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Desk => "desk",
            Self::Retired => "retired",
        }
    }
}

/// Where an Agent stands against its `retire_on` date.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retirement {
    /// No date, or the date is further off than the warning window.
    Serving,
    /// The date is within the warning window.
    Retiring,
    /// The date has come and the Agent works at `desk`.
    DeskDuty,
    /// The date has come and the Agent is never chosen.
    Retired,
}

impl Retirement {
    /// The name used on the wire: `serving`, `retiring`, `desk_duty` or `retired`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Serving => "serving",
            Self::Retiring => "retiring",
            Self::DeskDuty => "desk_duty",
            Self::Retired => "retired",
        }
    }
}

impl Agent {
    /// Where the Agent stands against its `retire_on` date at `now` (Unix seconds, UTC
    /// days): retired from the start of that day, retiring from `retire_warning_days`
    /// before it.
    pub fn retirement(&self, now: i64) -> Retirement {
        let Some(date) = &self.retire_on else {
            return Retirement::Serving;
        };
        if utc_day(now).as_str() >= date.as_str() {
            return match self.on_retire {
                OnRetire::Desk => Retirement::DeskDuty,
                OnRetire::Retired => Retirement::Retired,
            };
        }
        let warned = now.saturating_add(i64::from(self.retire_warning_days) * 86_400);
        if self.retire_warning_days > 0 && utc_day(warned).as_str() >= date.as_str() {
            Retirement::Retiring
        } else {
            Retirement::Serving
        }
    }

    /// Whether the Agent is never chosen at `now`.
    pub fn is_retired(&self, now: i64) -> bool {
        self.retirement(now) == Retirement::Retired
    }

    /// The clearance it works at in `operation` at `now`: its configured clearance, or the
    /// grant's, never above the Operation's, and `desk` once retired.
    ///
    /// A live grant that names its green ink request (a raise) replaces the configured
    /// clearance while the Agent keeps the model and configured clearance it was signed for
    /// ([`ClearanceGrant::applies_to`]); any other live grant only lowers it. A grant for
    /// another Agent or Operation, or a raise for another occupant, is ignored.
    pub fn effective_clearance(
        &self,
        operation: &Operation,
        grant: Option<&ClearanceGrant>,
        now: i64,
    ) -> String {
        let rank = |clearance: &str| clearance_rank(clearance).unwrap_or(0);
        let mut effective = rank(&self.clearance);
        if let Some(grant) = grant.filter(|grant| {
            grant.agent == self.id
                && grant.operation == operation.id
                && grant.is_live(now as f64)
                && grant.applies_to(self.model.as_deref(), &self.clearance)
        }) {
            effective = if grant.raises() {
                rank(&grant.clearance)
            } else {
                effective.min(rank(&grant.clearance))
            };
        }
        effective = effective.min(rank(&operation.clearance));
        if matches!(
            self.retirement(now),
            Retirement::DeskDuty | Retirement::Retired
        ) {
            effective = 0;
        }
        CLEARANCES[effective].to_owned()
    }
}

impl Agent {
    /// An Agent with every setting at the default a configuration file gives it: no
    /// handler or roles, clearance `field`, priority 0 and no limits beyond the daemon's.
    ///
    /// Set its public fields, then add it to a [`Roster`]. An Agent built in code inherits
    /// nothing: it is complete as given.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: String::new(),
            aliases: Vec::new(),
            handler: None,
            model: None,
            effort: None,
            roles: Vec::new(),
            tags: Vec::new(),
            clearance: "field".to_owned(),
            priority: 0,
            context: String::new(),
            env: BTreeMap::new(),
            limits: WatchdogOverrides::default(),
            budget: Budget::default(),
            max_concurrent: None,
            pool: None,
            retire_on: None,
            on_retire: OnRetire::Desk,
            retire_warning_days: DEFAULT_RETIRE_WARNING_DAYS,
        }
    }
}

/// A job an Agent can be qualified for: its purpose and the policy it works under.
///
/// A role carries no model settings. The Agent (codename) holds its harness, model and
/// effort; a role's backups are the other Agents qualified for it, chosen by priority.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRole {
    /// Stable role identifier, such as `case-officer`.
    pub id: String,
    /// Human-readable description of the work suited to this role.
    pub purpose: String,
    /// Instructions for performing this role. These do not grant permissions.
    pub policy: String,
}

impl AgentRole {
    /// A role with this identifier, purpose and policy; check it with
    /// [`AgentRole::validate`].
    pub fn new(
        id: impl Into<String>,
        purpose: impl Into<String>,
        policy: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            purpose: purpose.into(),
            policy: policy.into(),
        }
    }

    /// Validates a role configuration before it is persisted or used to start a Mission.
    pub fn validate(&self) -> Result<(), RosterError> {
        if !valid_role_id(&self.id) {
            return Err(error(
                "Agent role ID must use letters, digits, '.', '_' or '-'",
            ));
        }
        if !valid_role_text(&self.purpose, 4096) || !valid_role_text(&self.policy, 16_384) {
            return Err(error(
                "Agent role purpose or policy is empty or exceeds its limit",
            ));
        }
        Ok(())
    }
}

/// One globally registered project and its Operation policy.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    /// Operation codename.
    pub id: String,
    /// Project name, also accepted as a lookup key.
    pub name: Option<String>,
    /// Human-readable project purpose.
    pub description: String,
    /// Local checkout path. A remote Operation has no local path.
    pub path: Option<PathBuf>,
    /// Remote Theater name, when the Operation is elsewhere.
    pub theater: Option<String>,
    /// Alternate local location for this Operation's trusted `.ai6` config.
    pub config: Option<PathBuf>,
    /// Allowed Agent codenames or aliases; empty allows every Agent.
    pub roster: Vec<String>,
    /// Highest Agent clearance allowed at this Operation.
    pub clearance: String,
    /// Optional concurrent operation limit.
    pub max_concurrent: Option<usize>,
    /// Where Missions work: the safe-house provider and its settings.
    pub workspace: Workspace,
    /// Station labels required before a Station may run this Operation's Missions.
    pub station_labels: BTreeMap<String, String>,
    /// Stations allowed to run this Operation's Missions; empty allows every Station. A
    /// Mission may narrow this list but never widen it.
    pub stations: Vec<String>,
    /// Cover policy used when work is committed.
    pub cover: String,
    /// Optional Git author the Operation's Agents commit under (their cover).
    pub cover_identity: Option<CoverIdentity>,
    /// Response when a commit violates the cover policy.
    pub on_blown: String,
    /// Response when a provider usage limit stops a Session.
    pub on_limit: OnLimit,
    /// Seconds to park when a limit reports no reset time.
    pub limit_fallback_secs: u64,
    /// Response when a Session is lost: with its Station when no other Station can adopt it,
    /// or when its runner or wrapped harness stays out of reach past `lost_after_secs`.
    pub on_lost: OnLost,
    /// Whether Missions go to the least-loaded place (`spread`, the default) or fill the
    /// busiest place that still has room (`pack`).
    pub placement: PlacementStrategy,
    /// Seconds a Session's runner, or its wrapped harness, may stay out of reach before its
    /// Station probes it once more and, if it is still gone, marks the Session lost.
    pub lost_after_secs: u64,
    /// Spending limits for this Operation's Missions; none is set unless configured.
    pub budget: Budget,
    /// What a Session may do once it has read untrusted input; strict unless configured.
    pub exposure: ExposurePolicy,
    /// When an Agent working here is quarantined.
    pub quarantine: QuarantinePolicy,
    /// Which irreversible steps need a person's approval; strict unless configured.
    pub green_ink: GreenInkPolicy,
    /// How briefs are vetted before they are placed; every check on unless configured.
    pub vetting: VettingPolicy,
    /// Work sources (trackers, CI) this Operation takes briefs from.
    pub sources: toml::Table,
    /// Standing orders that queue a brief on a schedule, once an operator authorizes them.
    pub sleepers: BTreeMap<String, Sleeper>,
    /// The account pools its Missions may draw from; `None` (the default) is every pool
    /// whose own `operations` admit the Operation.
    pub pools: Option<Vec<String>>,
    /// Whether a harness may load the settings its working directory carries; `ignore`
    /// unless configured.
    pub harness: HarnessPolicy,
    /// When M asks a person to raise an Agent's clearance here.
    pub promotion: PromotionPolicy,
}

/// How an Operation's Missions get their working directory (`workspace = { .. }`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The safe-house provider ID. `None` picks `git` when the Operation's path is a Git
    /// checkout, and otherwise works in the Operation's directory with no safe house.
    pub provider: Option<String>,
    /// Where the work comes from; defaults to the Operation's `path`.
    pub source: Option<PathBuf>,
    /// Revision a safe house starts from.
    pub base: String,
    /// Branch name template; `{id}`, `{slug}` and `{operation}` are filled in.
    pub branch: String,
    /// Whether each Mission gets its own copy. `false` works in the source in place.
    pub isolate: bool,
    /// The Git remote, by name or URL, a Mission's safe house is checkpointed to, so a
    /// later attempt on another Station starts from its work. `None` (the default) pushes
    /// nothing.
    pub checkpoint_remote: Option<String>,
    /// Seconds between checkpoints while a Session runs.
    pub checkpoint_secs: u64,
}

/// Seconds between safe-house checkpoints when an Operation does not say.
pub const DEFAULT_CHECKPOINT_SECS: u64 = 300;

impl Default for Workspace {
    /// Isolated copies from `HEAD` on branch `{id}-{slug}`, checkpointed nowhere, every
    /// [`DEFAULT_CHECKPOINT_SECS`].
    fn default() -> Self {
        Self {
            provider: None,
            source: None,
            base: "HEAD".to_owned(),
            branch: "{id}-{slug}".to_owned(),
            isolate: true,
            checkpoint_remote: None,
            checkpoint_secs: DEFAULT_CHECKPOINT_SECS,
        }
    }
}

/// What happens to a Mission whose Session is lost with its Station (`on_lost`).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnLost {
    /// Try again as a new attempt on another eligible Station, with a hand-off brief, until
    /// the Registry's attempt limit.
    #[default]
    Retry,
    /// File the Mission as failed.
    Fail,
}

impl Operation {
    /// The provider that makes this Operation's safe houses, or `None` when Missions work
    /// in the Operation's directory without one.
    pub fn safe_house_provider(&self) -> Option<String> {
        if let Some(provider) = &self.workspace.provider {
            return Some(provider.clone());
        }
        let source = self.workspace.source.as_ref().or(self.path.as_ref())?;
        source.join(".git").exists().then(|| "git".to_owned())
    }
}

/// Git author identity configured by an Operation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverIdentity {
    /// Commit author name.
    pub name: String,
    /// Commit author email.
    pub email: String,
}

/// Loaded global Operation registry and Agent roster.
#[derive(Debug, Clone, Default)]
pub struct Roster {
    operations: BTreeMap<String, Operation>,
    agents: BTreeMap<String, AgentEntry>,
    roles: BTreeMap<String, AgentRole>,
    #[cfg_attr(
        not(feature = "daemon"),
        allow(dead_code, reason = "used only by the daemon")
    )]
    covers: Covers,
    assistant: AssistantSettings,
}

/// Configuration could not be read or resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterError {
    message: String,
    kind: RosterErrorKind,
}

/// What kind of failure a [`RosterError`] is.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosterErrorKind {
    /// The configuration is not acceptable: the change or file is refused as it stands.
    Invalid,
    /// The configuration could not be read or written; trying again may succeed.
    Unavailable,
    /// The source does not take changes ([`ConfigProvider::put`](crate::ConfigProvider::put)).
    ReadOnly,
}

impl RosterError {
    /// What kind of failure this is.
    pub fn kind(&self) -> RosterErrorKind {
        self.kind
    }
}

impl std::fmt::Display for RosterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RosterError {}

/// One Agent of a [`Roster`]: as a configuration file defines it, where unset settings come
/// from the Agent it extends, or complete as built in code.
#[derive(Debug, Clone)]
enum AgentEntry {
    Defined(AgentDefinition),
    Built(Agent),
}

impl AgentEntry {
    fn aliases(&self) -> &[String] {
        match self {
            Self::Defined(definition) => &definition.aliases,
            Self::Built(agent) => &agent.aliases,
        }
    }
}

/// Collects the typed records of a [`Roster`]; [`RosterBuilder::build`] checks them.
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct RosterBuilder {
    operations: Vec<Operation>,
    agents: Vec<Agent>,
    roles: Vec<AgentRole>,
    covers: Covers,
    assistant: AssistantSettings,
}

impl RosterBuilder {
    /// Registers an Operation.
    pub fn operation(mut self, operation: Operation) -> Self {
        self.operations.push(operation);
        self
    }

    /// Adds an Agent.
    pub fn agent(mut self, agent: Agent) -> Self {
        self.agents.push(agent);
        self
    }

    /// Adds an Agent role.
    pub fn role(mut self, role: AgentRole) -> Self {
        self.roles.push(role);
        self
    }

    /// Adds a cover policy, replacing a built-in or earlier one with the same name.
    pub fn cover(mut self, cover: Cover) -> Self {
        self.covers.insert(cover);
        self
    }

    /// Sets Moneypenny's `[assistant]` settings.
    pub fn assistant(mut self, settings: AssistantSettings) -> Self {
        self.assistant = settings;
        self
    }

    /// The roster of these records, checked as [`Roster::from_parts`] checks them, with the
    /// assistant settings checked by [`AssistantSettings::validate`].
    ///
    /// # Errors
    ///
    /// As [`Roster::from_parts`], or assistant settings that are not acceptable.
    pub fn build(self) -> Result<Roster, RosterError> {
        self.assistant.validate().map_err(error)?;
        let mut roster = Roster::from_parts(self.operations, self.agents, self.roles, self.covers)?;
        roster.assistant = self.assistant;
        Ok(roster)
    }
}

/// What [`Roster::select_agent`] looks for. The default asks for any Agent of the global
/// roster, whatever its handler or retirement date.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default)]
#[must_use]
pub struct AgentQuery<'a> {
    /// The Operation the Agent must be posted to; `None` is the global roster.
    pub operation: Option<&'a str>,
    /// The role it must be qualified for, or a qualified role name such as `analyst/deep`.
    pub role: Option<&'a str>,
    /// Tags it must carry, all of them.
    pub tags: &'a [String],
    /// The handlers that can run now; `None` takes any handler.
    pub available_handlers: Option<&'a [String]>,
    /// Agents to pass over, such as quarantined ones.
    pub excluded: &'a [String],
    /// Unix seconds to judge retirement at; `None` ignores retirement.
    pub now: Option<i64>,
}

impl<'a> AgentQuery<'a> {
    /// Any Agent of the global roster.
    pub fn new() -> Self {
        Self::default()
    }

    /// Posted to `operation`.
    pub fn operation(mut self, operation: Option<&'a str>) -> Self {
        self.operation = operation;
        self
    }

    /// Qualified for `role`.
    pub fn role(mut self, role: Option<&'a str>) -> Self {
        self.role = role;
        self
    }

    /// Carrying every one of `tags`.
    pub fn tags(mut self, tags: &'a [String]) -> Self {
        self.tags = tags;
        self
    }

    /// With a handler among `handlers`, when given.
    pub fn handlers(mut self, handlers: Option<&'a [String]>) -> Self {
        self.available_handlers = handlers;
        self
    }

    /// Not one of `agents`.
    pub fn excluding(mut self, agents: &'a [String]) -> Self {
        self.excluded = agents;
        self
    }

    /// Not retired at `now` (Unix seconds).
    pub fn at(mut self, now: i64) -> Self {
        self.now = Some(now);
        self
    }
}

impl Roster {
    /// Loads global `operations.toml` and `agents.toml` from a configuration directory.
    /// Missing files mean an empty registry; invalid files fail closed.
    pub fn load(root: impl Into<PathBuf>) -> Result<Self, RosterError> {
        let root = root.into();
        let mut roster = Self {
            covers: Covers::from_config_dir(&root)
                .map_err(|cover_error| error(cover_error.to_string()))?,
            ..Self::default()
        };
        let operation_file = root.join(OPERATIONS_FILE);
        if operation_file.exists() {
            let document: OperationsDocument = parse_file(&operation_file)?;
            for (id, definition) in document.operation {
                let operation = definition.resolve(id, &operation_file.display())?;
                roster
                    .register(operation)
                    .map_err(|reason| error(format!("{}: {reason}", operation_file.display())))?;
            }
        }
        let agents_file = root.join(AGENTS_FILE);
        if agents_file.exists() {
            let document: AgentsDocument = parse_file(&agents_file)?;
            for (id, definition) in &document.agent {
                definition.budget.resolve().map_err(|message| {
                    error(format!(
                        "{}: Agent {id:?}: {message}",
                        agents_file.display()
                    ))
                })?;
            }
            roster.agents = document
                .agent
                .into_iter()
                .map(|(id, definition)| (id, AgentEntry::Defined(definition)))
                .collect();
            for (id, definition) in document.role {
                roster
                    .roles
                    .insert(id.clone(), definition.resolve(id, &agents_file.display())?);
            }
            if let Some(mut assistant) = document.assistant {
                assistant.resolve_paths(&root);
                assistant
                    .validate()
                    .map_err(|reason| error(format!("{}: {reason}", agents_file.display())))?;
                roster.assistant = assistant;
            }
        }
        Ok(roster)
    }

    /// Moneypenny's settings, the `[assistant]` table of the global `agents.toml`; the default
    /// when it has none.
    pub fn assistant(&self) -> &AssistantSettings {
        &self.assistant
    }

    /// Builds a roster from typed records, with no configuration files: the way in for
    /// configuration kept anywhere else.
    ///
    /// Each Operation and role is checked as a configuration file is; each Agent's codename
    /// and clearance are checked here rather than when it is first resolved. `covers` holds
    /// the cover policies, built-in ones included ([`Covers::default`]).
    ///
    /// # Errors
    ///
    /// Returns an error naming the first record that is not acceptable, or an Operation key,
    /// Agent codename or role ID given twice.
    pub fn from_parts(
        operations: impl IntoIterator<Item = Operation>,
        agents: impl IntoIterator<Item = Agent>,
        roles: impl IntoIterator<Item = AgentRole>,
        covers: Covers,
    ) -> Result<Self, RosterError> {
        let mut roster = Self {
            covers,
            ..Self::default()
        };
        for operation in operations {
            operation.validate()?;
            roster.register(operation)?;
        }
        for agent in agents {
            validate_codename(&agent.id)?;
            validate_clearance(&agent.clearance)
                .map_err(|reason| error(format!("Agent {:?}: {reason}", agent.id)))?;
            validate_retire_on(&agent)?;
            let id = agent.id.clone();
            if roster.agents.contains_key(&id) {
                return Err(error(format!("Agent {id:?} is given twice")));
            }
            roster.agents.insert(id, AgentEntry::Built(agent));
        }
        for role in roles {
            role.validate()?;
            if roster.roles.contains_key(&role.id) {
                return Err(error(format!("Agent role {:?} is given twice", role.id)));
            }
            roster.roles.insert(role.id.clone(), role);
        }
        Ok(roster)
    }

    /// Starts a roster from typed records; see [`Roster::from_parts`].
    pub fn builder() -> RosterBuilder {
        RosterBuilder::default()
    }

    /// Adds an Operation whose codename and project name are both still unused.
    fn register(&mut self, operation: Operation) -> Result<(), RosterError> {
        for key in operation.name.iter().chain(std::iter::once(&operation.id)) {
            if self
                .operations
                .values()
                .any(|other| other.id == *key || other.name.as_deref() == Some(key.as_str()))
            {
                return Err(error(format!(
                    "Operation key {key:?} is already registered"
                )));
            }
        }
        self.operations.insert(operation.id.clone(), operation);
        Ok(())
    }

    #[cfg_attr(
        not(feature = "daemon"),
        allow(dead_code, reason = "used only by the daemon")
    )]
    pub(crate) fn cover(&self, name: &str) -> Result<&Cover, CoverError> {
        self.covers.get(name)
    }

    /// Finds a configured role by its canonical identifier or a qualified role name.
    pub fn role(&self, key: &str) -> Result<&AgentRole, RosterError> {
        self.roles
            .get(key)
            .or_else(|| {
                self.roles
                    .iter()
                    .filter(|(id, _)| key.starts_with(&format!("{id}/")))
                    .max_by_key(|(id, _)| id.len())
                    .map(|(_, role)| role)
            })
            .ok_or_else(|| error(format!("no configured Agent role {key:?}")))
    }

    /// The codenames of the global roster's Agents, in order.
    pub fn agent_ids(&self) -> impl Iterator<Item = &str> {
        self.agents.keys().map(String::as_str)
    }

    /// Lists configured Agent roles in stable identifier order.
    pub fn roles(&self) -> impl Iterator<Item = &AgentRole> {
        self.roles.values()
    }

    /// Finds an Operation by codename or project name.
    pub fn operation(&self, key: &str) -> Result<&Operation, RosterError> {
        self.operations
            .get(key)
            .or_else(|| {
                self.operations
                    .values()
                    .find(|operation| operation.name.as_deref() == Some(key))
            })
            .ok_or_else(|| error(format!("no registered Operation {key:?}")))
    }

    /// Resolves a global Agent and, for a registered Operation, its trusted local override.
    pub fn agent(
        &self,
        operation_key: Option<&str>,
        agent_key: &str,
    ) -> Result<Agent, RosterError> {
        let operation = operation_key.map(|key| self.operation(key)).transpose()?;
        let mut definitions = self.agents.clone();
        if let Some(operation) = operation
            && let Some(path) = self.operation_agent_file(operation)?
        {
            let local = parse_operation_agents(&path)?;
            definitions.extend(
                local
                    .agent
                    .into_iter()
                    .map(|(id, definition)| (id, AgentEntry::Defined(definition))),
            );
        }
        let agent = resolve_agent(agent_key, &definitions, &mut BTreeSet::new())?;
        if let Some(operation) = operation {
            if !operation.roster.is_empty()
                && !operation.roster.iter().any(|member| {
                    member == &agent.id || agent.aliases.iter().any(|alias| alias == member)
                })
            {
                return Err(error(format!(
                    "Agent {:?} is not posted to Operation {:?}",
                    agent.id, operation.id
                )));
            }
            if clearance_rank(&agent.clearance)? > clearance_rank(&operation.clearance)? {
                return Err(error(format!(
                    "Agent {:?} clearance {:?} exceeds Operation {:?} clearance {:?}",
                    agent.id, agent.clearance, operation.id, operation.clearance
                )));
            }
        }
        Ok(agent)
    }

    /// Selects the highest-priority Agent posted to the query's Operation that is qualified
    /// for its role and carries every tag.
    ///
    /// An Agent whose handler is not among `available_handlers`, that is `excluded` (a
    /// quarantined one) or that is retired at `now` is skipped, so the next qualified Agent
    /// (its backup) is chosen instead. When only retired Agents qualify, the error names
    /// them and their dates.
    ///
    /// # Errors
    ///
    /// No Agent qualifies, or the Operation is not registered or its checkout's
    /// `.ai6/agents.toml` cannot be read.
    pub fn select_agent(&self, query: &AgentQuery<'_>) -> Result<Agent, RosterError> {
        let AgentQuery {
            operation: operation_key,
            role,
            tags,
            available_handlers,
            excluded,
            now,
        } = *query;
        let mut keys: BTreeSet<String> = self.agents.keys().cloned().collect();
        if let Some(operation) = operation_key.map(|key| self.operation(key)).transpose()?
            && let Some(path) = self.operation_agent_file(operation)?
        {
            let local = parse_operation_agents(&path)?;
            keys.extend(local.agent.into_keys());
        }

        let mut candidates = BTreeMap::new();
        let mut retired = Vec::new();
        for key in keys {
            let Ok(agent) = self.agent(operation_key, &key) else {
                continue;
            };
            let role_matches = role.is_none_or(|requested| {
                agent.roles.iter().any(|candidate| {
                    requested == candidate
                        || requested.starts_with(&format!("{}/", candidate.trim_end_matches('/')))
                })
            });
            let tags_match = tags.iter().all(|tag| agent.tags.contains(tag));
            if !role_matches || !tags_match || excluded.contains(&agent.id) {
                continue;
            }
            if now.is_some_and(|now| agent.is_retired(now)) {
                retired.push(format!(
                    "{} retired on {}",
                    agent.id,
                    agent.retire_on.as_deref().unwrap_or_default()
                ));
                continue;
            }
            let available = available_handlers.is_none_or(|handlers| {
                agent
                    .handler
                    .as_ref()
                    .is_some_and(|handler| handlers.contains(handler))
            });
            if available {
                candidates.insert(agent.id.clone(), agent);
            }
        }
        candidates
            .into_values()
            .max_by(|left, right| {
                left.priority
                    .cmp(&right.priority)
                    .then_with(|| right.id.cmp(&left.id))
            })
            .ok_or_else(|| {
                let wanted = role.unwrap_or("any role");
                if retired.is_empty() {
                    error(format!(
                        "no available Agent matches role {wanted:?} and tags {}",
                        tags.join(", ")
                    ))
                } else {
                    error(format!(
                        "no qualified Agent for role {wanted:?}; {}",
                        retired.join(", ")
                    ))
                }
            })
    }

    /// Resolves every global Agent, `extends` and clearance included, as a SQL roster does
    /// when it is read: an Agent that cannot be resolved is an error here rather than missing
    /// when it is first wanted. An Operation's trusted `.ai6/agents.toml` is not read.
    ///
    /// # Errors
    ///
    /// Returns an error naming the first Agent that cannot be resolved.
    pub fn check_agents(&self) -> Result<(), RosterError> {
        for key in self.agents.keys() {
            resolve_agent(key, &self.agents, &mut BTreeSet::new())?;
        }
        Ok(())
    }

    /// Lists the Agents posted to a registered Operation after applying its roster policy.
    pub fn agents_for_operation(&self, operation_key: &str) -> Result<Vec<Agent>, RosterError> {
        let operation = self.operation(operation_key)?;
        let mut keys: BTreeSet<String> = self.agents.keys().cloned().collect();
        if let Some(path) = self.operation_agent_file(operation)? {
            let local = parse_operation_agents(&path)?;
            keys.extend(local.agent.into_keys());
        }

        let mut agents = BTreeMap::new();
        for key in keys {
            if let Ok(agent) = self.agent(Some(operation_key), &key) {
                agents.insert(agent.id.clone(), agent);
            }
        }
        Ok(agents.into_values().collect())
    }

    /// Lists registered Operations in stable codename order.
    pub fn operations(&self) -> impl Iterator<Item = &Operation> {
        self.operations.values()
    }

    /// The Operation's own agents file in its trusted `.ai6` directory: `agents.cfg`,
    /// `.toml`, `.json`, `.yaml` or `.yml`; `None` when there is none.
    fn operation_agent_file(&self, operation: &Operation) -> Result<Option<PathBuf>, RosterError> {
        let Some(directory) = operation.config.clone().or_else(|| {
            if operation.theater.is_none() {
                operation.path.as_ref().map(|path| path.join(".ai6"))
            } else {
                None
            }
        }) else {
            return Ok(None);
        };
        let found: Vec<PathBuf> = crate::bootstrap::format::EXTENSIONS
            .iter()
            .map(|extension| directory.join(format!("{AGENTS_STEM}.{extension}")))
            .filter(|path| path.is_file())
            .collect();
        match found.as_slice() {
            [] => Ok(None),
            [path] => Ok(Some(path.clone())),
            _ => Err(error(format!(
                "{} are the same file; keep one of them",
                found
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" and ")
            ))),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationsDocument {
    #[serde(default)]
    operation: BTreeMap<String, OperationDefinition>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentsDocument {
    #[serde(default)]
    agent: BTreeMap<String, AgentDefinition>,
    #[serde(default)]
    role: BTreeMap<String, AgentRoleDefinition>,
    #[serde(default)]
    assistant: Option<AssistantSettings>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OperationDefinition {
    name: Option<String>,
    description: String,
    path: Option<String>,
    theater: Option<String>,
    config: Option<String>,
    roster: Vec<String>,
    clearance: String,
    max_concurrent: Option<usize>,
    workspace: WorkspaceDefinition,
    station_labels: BTreeMap<String, String>,
    stations: Vec<String>,
    cover: String,
    cover_identity: Option<CoverIdentity>,
    on_blown: String,
    on_limit: OnLimit,
    limit_fallback_secs: Option<u64>,
    on_lost: OnLost,
    placement: PlacementStrategy,
    lost_after_secs: Option<u64>,
    budget: BudgetDefinition,
    exposure: ExposureDefinition,
    quarantine: QuarantineDefinition,
    green_ink: GreenInkDefinition,
    vetting: Option<toml::Value>,
    sources: toml::Table,
    sleepers: BTreeMap<String, Sleeper>,
    pools: Option<Vec<String>>,
    harness: HarnessPolicy,
    promotion: PromotionDefinition,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WorkspaceDefinition {
    provider: Option<String>,
    source: Option<String>,
    base: String,
    branch: String,
    isolate: bool,
    checkpoint_remote: Option<String>,
    checkpoint_secs: Option<u64>,
}

impl Default for WorkspaceDefinition {
    fn default() -> Self {
        Self {
            provider: None,
            source: None,
            base: String::new(),
            branch: String::new(),
            isolate: true,
            checkpoint_remote: None,
            checkpoint_secs: None,
        }
    }
}

impl OperationDefinition {
    fn resolve(self, id: String, source: &dyn fmt::Display) -> Result<Operation, RosterError> {
        let in_file = |reason: RosterError| error(format!("{source}: {reason}"));
        let operation = self.into_operation(id).map_err(in_file)?;
        operation.validate().map_err(in_file)?;
        Ok(operation)
    }

    /// The Operation these settings describe, with every unset key at its default.
    fn into_operation(self, id: String) -> Result<Operation, RosterError> {
        let path = self.path.map(|path| expand_path(&path));
        let config = self.config.map(|path| expand_path(&path));
        let budget = self
            .budget
            .resolve()
            .map_err(|message| error(format!("Operation {id:?}: {message}")))?;
        let exposure = self
            .exposure
            .resolve()
            .map_err(|message| error(format!("Operation {id:?}: {message}")))?;
        let quarantine = self
            .quarantine
            .resolve()
            .map_err(|message| error(format!("Operation {id:?}: {message}")))?;
        self.harness
            .validate()
            .map_err(|message| error(format!("Operation {id:?}: {message}")))?;
        let green_ink = self
            .green_ink
            .resolve()
            .map_err(|message| error(format!("Operation {id:?}: {message}")))?;
        let vetting = policy_from_toml(self.vetting.as_ref())
            .map_err(|message| error(format!("Operation {id:?}: {message}")))?;
        let promotion = self
            .promotion
            .resolve()
            .map_err(|message| error(format!("Operation {id:?}: {message}")))?;
        let workspace = Workspace::default();
        Ok(Operation {
            id,
            name: self.name,
            description: self.description,
            path,
            theater: self.theater,
            config,
            roster: self.roster,
            clearance: if self.clearance.is_empty() {
                "field".to_owned()
            } else {
                self.clearance
            },
            max_concurrent: self.max_concurrent,
            workspace: Workspace {
                provider: self.workspace.provider,
                source: self.workspace.source.map(|path| expand_path(&path)),
                base: if self.workspace.base.is_empty() {
                    workspace.base
                } else {
                    self.workspace.base
                },
                branch: if self.workspace.branch.is_empty() {
                    workspace.branch
                } else {
                    self.workspace.branch
                },
                isolate: self.workspace.isolate,
                checkpoint_remote: self.workspace.checkpoint_remote,
                checkpoint_secs: self
                    .workspace
                    .checkpoint_secs
                    .unwrap_or(workspace.checkpoint_secs),
            },
            station_labels: self.station_labels,
            stations: self.stations,
            cover: if self.cover.is_empty() {
                "covert".to_owned()
            } else {
                self.cover
            },
            cover_identity: self.cover_identity,
            on_blown: if self.on_blown.is_empty() {
                "flag".to_owned()
            } else {
                self.on_blown
            },
            on_limit: self.on_limit,
            limit_fallback_secs: self
                .limit_fallback_secs
                .unwrap_or(DEFAULT_LIMIT_FALLBACK_SECS),
            on_lost: self.on_lost,
            placement: self.placement,
            lost_after_secs: self.lost_after_secs.unwrap_or(DEFAULT_LOST_AFTER_SECS),
            budget,
            exposure,
            quarantine,
            green_ink,
            vetting,
            sources: self.sources,
            sleepers: self.sleepers,
            pools: self.pools,
            harness: self.harness,
            promotion,
        })
    }
}

impl Operation {
    /// An Operation with every setting at the default a configuration file gives it: no
    /// path, clearance `field`, the `covert` cover, strict exposure and green ink.
    ///
    /// Set its public fields, then check it with [`Operation::validate`] or build a
    /// [`Roster`] from it.
    pub fn new(id: impl Into<String>) -> Self {
        OperationDefinition::default()
            .into_operation(id.into())
            .expect("the default Operation settings resolve")
    }

    /// Checks the settings a configuration file is checked for: Sleeper names and
    /// schedules, Station IDs, clearance, workspace, `on_blown`, the checkpoint, limit and
    /// `lost_after_secs` intervals and the cover identity.
    ///
    /// # Errors
    ///
    /// Names the Operation and the first setting that is not acceptable.
    pub fn validate(&self) -> Result<(), RosterError> {
        let id = &self.id;
        for (name, sleeper) in &self.sleepers {
            let valid_name = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
            if !valid_name {
                return Err(error(format!(
                    "Operation {id:?}: sleeper name {name:?} may use only letters, digits, '-' and '_'"
                )));
            }
            sleeper
                .validate()
                .map_err(|e| error(format!("Operation {id:?}: sleeper {name:?}: {e}")))?;
        }
        if let Some(station) = self
            .stations
            .iter()
            .find(|station| validate_station_id(station).is_err())
        {
            return Err(error(format!(
                "Operation {id:?}: stations entry {station:?} is not a valid Station ID"
            )));
        }
        validate_clearance(&self.clearance).map_err(|e| error(format!("Operation {id:?}: {e}")))?;
        if self
            .workspace
            .provider
            .as_deref()
            .is_some_and(|provider| provider.trim().is_empty())
        {
            return Err(error(format!(
                "Operation {id:?}: workspace.provider must not be empty"
            )));
        }
        if !matches!(self.on_blown.as_str(), "flag" | "fail") {
            return Err(error(format!(
                "Operation {id:?}: on_blown must be flag or fail"
            )));
        }
        if self.workspace.checkpoint_secs == 0
            || self
                .workspace
                .checkpoint_remote
                .as_deref()
                .is_some_and(|remote| remote.trim().is_empty())
        {
            return Err(error(format!(
                "Operation {id:?}: workspace.checkpoint_secs must be greater than zero and \
                 workspace.checkpoint_remote must not be empty"
            )));
        }
        if self.limit_fallback_secs == 0 {
            return Err(error(format!(
                "Operation {id:?}: limit_fallback_secs must be greater than zero"
            )));
        }
        if self.lost_after_secs == 0 {
            return Err(error(format!(
                "Operation {id:?}: lost_after_secs must be greater than zero"
            )));
        }
        if let Some(identity) = &self.cover_identity
            && (identity.name.trim().is_empty() || identity.email.trim().is_empty())
        {
            return Err(error(format!(
                "Operation {id:?}: cover_identity needs name and email"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AgentDefinition {
    aliases: Vec<String>,
    description: String,
    extends: Option<String>,
    handler: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    roles: Vec<String>,
    tags: Vec<String>,
    clearance: Option<String>,
    priority: Option<i64>,
    context: Option<String>,
    env: BTreeMap<String, String>,
    limits: WatchdogOverrides,
    budget: BudgetDefinition,
    max_concurrent: Option<usize>,
    pool: Option<String>,
    retire_on: Option<String>,
    on_retire: Option<OnRetire>,
    retire_warning_days: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AgentRoleDefinition {
    purpose: String,
    policy: String,
}

impl AgentRoleDefinition {
    fn resolve(self, id: String, source: &dyn fmt::Display) -> Result<AgentRole, RosterError> {
        let role = AgentRole {
            id,
            purpose: self.purpose,
            policy: self.policy,
        };
        role.validate()
            .map_err(|validation| error(format!("{source}: {validation}")))?;
        Ok(role)
    }
}

/// The roster of records kept somewhere other than configuration files: each record's
/// definition as the keys of its table in `operations.toml` or `agents.toml`, in any serde
/// format, keyed by its ID, built into typed records and checked by [`Roster::from_parts`].
/// `assistant` holds the keys of `[assistant]`, checked by [`AssistantSettings::validate`];
/// `None` is the default settings. `source` names where they came from in error messages.
///
/// Agents are resolved here, `extends` included, so the roster holds them complete. A
/// relative skill path in the assistant settings is resolved against `base` when given.
pub(crate) fn roster_from_definitions<'de, D>(
    operations: impl IntoIterator<Item = (String, D)>,
    agents: impl IntoIterator<Item = (String, D)>,
    roles: impl IntoIterator<Item = (String, D)>,
    covers: Covers,
    assistant: Option<D>,
    base: Option<&Path>,
    source: &dyn fmt::Display,
) -> Result<Roster, RosterError>
where
    D: serde::Deserializer<'de>,
    D::Error: fmt::Display,
{
    let invalid = |what: &str, id: &str, reason: &dyn fmt::Display| {
        error(format!("{source}: {what} {id:?}: {reason}"))
    };
    let mut parsed = Vec::new();
    for (id, definition) in operations {
        let definition = OperationDefinition::deserialize(definition)
            .map_err(|reason| invalid("Operation", &id, &reason))?;
        parsed.push(definition.resolve(id, source)?);
    }
    let mut definitions = BTreeMap::new();
    for (id, definition) in agents {
        let definition = AgentDefinition::deserialize(definition)
            .map_err(|reason| invalid("Agent", &id, &reason))?;
        definition
            .budget
            .resolve()
            .map_err(|message| invalid("Agent", &id, &message))?;
        definitions.insert(id, AgentEntry::Defined(definition));
    }
    let resolved = definitions
        .keys()
        .map(|id| {
            resolve_agent(id, &definitions, &mut BTreeSet::new())
                .map_err(|reason| error(format!("{source}: {reason}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut typed_roles = Vec::new();
    for (id, definition) in roles {
        let definition = AgentRoleDefinition::deserialize(definition)
            .map_err(|reason| invalid("Agent role", &id, &reason))?;
        typed_roles.push(definition.resolve(id, source)?);
    }
    let assistant = match assistant {
        Some(definition) => {
            let mut settings = AssistantSettings::deserialize(definition)
                .map_err(|reason| error(format!("{source}: assistant: {reason}")))?;
            if let Some(base) = base {
                settings.resolve_paths(base);
            }
            settings
                .validate()
                .map_err(|reason| error(format!("{source}: {reason}")))?;
            settings
        }
        None => AssistantSettings::default(),
    };
    let mut roster = Roster::from_parts(parsed, resolved, typed_roles, covers)?;
    roster.assistant = assistant;
    Ok(roster)
}

fn valid_role_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_role_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= limit
        && value.trim() == value
        && value
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\t'))
}

fn resolve_agent(
    key: &str,
    definitions: &BTreeMap<String, AgentEntry>,
    seen: &mut BTreeSet<String>,
) -> Result<Agent, RosterError> {
    let id = if definitions.contains_key(key) {
        key.to_owned()
    } else {
        let mut matches = definitions
            .iter()
            .filter(|(_, value)| value.aliases().iter().any(|alias| alias == key))
            .map(|(id, _)| id.clone());
        let id = matches
            .next()
            .ok_or_else(|| error(format!("no configured Agent or alias {key:?}")))?;
        if matches.next().is_some() {
            return Err(error(format!("Agent alias {key:?} is ambiguous")));
        }
        id
    };
    validate_codename(&id)?;
    if !seen.insert(id.clone()) {
        return Err(error(format!("Agent inheritance cycle at {id:?}")));
    }
    let definition = match &definitions[&id] {
        AgentEntry::Built(agent) => {
            seen.remove(&id);
            return Ok(agent.clone());
        }
        AgentEntry::Defined(definition) => definition,
    };
    let base = definition
        .extends
        .as_deref()
        .map(|parent| resolve_agent(parent, definitions, seen))
        .transpose()?;
    let mut env = base
        .as_ref()
        .map(|agent| agent.env.clone())
        .unwrap_or_default();
    env.extend(definition.env.clone());
    let own_budget = definition
        .budget
        .resolve()
        .map_err(|message| error(format!("Agent {id:?}: {message}")))?;
    let agent = Agent {
        id,
        description: if definition.description.is_empty() {
            base.as_ref()
                .map(|agent| agent.description.clone())
                .unwrap_or_default()
        } else {
            definition.description.clone()
        },
        aliases: definition.aliases.clone(),
        handler: definition
            .handler
            .clone()
            .or_else(|| base.as_ref().and_then(|agent| agent.handler.clone())),
        model: definition
            .model
            .clone()
            .or_else(|| base.as_ref().and_then(|agent| agent.model.clone())),
        effort: definition
            .effort
            .clone()
            .or_else(|| base.as_ref().and_then(|agent| agent.effort.clone())),
        roles: if definition.roles.is_empty() {
            base.as_ref()
                .map(|agent| agent.roles.clone())
                .unwrap_or_default()
        } else {
            definition.roles.clone()
        },
        tags: if definition.tags.is_empty() {
            base.as_ref()
                .map(|agent| agent.tags.clone())
                .unwrap_or_default()
        } else {
            definition.tags.clone()
        },
        clearance: definition
            .clearance
            .clone()
            .or_else(|| base.as_ref().map(|agent| agent.clearance.clone()))
            .unwrap_or_else(|| "field".to_owned()),
        priority: definition
            .priority
            .or_else(|| base.as_ref().map(|agent| agent.priority))
            .unwrap_or_default(),
        context: definition
            .context
            .clone()
            .or_else(|| base.as_ref().map(|agent| agent.context.clone()))
            .unwrap_or_default(),
        env,
        limits: base
            .as_ref()
            .map(|agent| agent.limits.merge(&definition.limits))
            .unwrap_or_else(|| definition.limits.clone()),
        budget: base
            .as_ref()
            .map_or(own_budget, |agent| agent.budget.merge(&own_budget)),
        max_concurrent: definition
            .max_concurrent
            .or_else(|| base.as_ref().and_then(|agent| agent.max_concurrent)),
        pool: definition
            .pool
            .clone()
            .or_else(|| base.as_ref().and_then(|agent| agent.pool.clone())),
        retire_on: definition
            .retire_on
            .clone()
            .or_else(|| base.as_ref().and_then(|agent| agent.retire_on.clone())),
        on_retire: definition
            .on_retire
            .or_else(|| base.as_ref().map(|agent| agent.on_retire))
            .unwrap_or_default(),
        retire_warning_days: definition
            .retire_warning_days
            .or_else(|| base.as_ref().map(|agent| agent.retire_warning_days))
            .unwrap_or(DEFAULT_RETIRE_WARNING_DAYS),
    };
    validate_clearance(&agent.clearance)?;
    validate_retire_on(&agent)?;
    seen.remove(&agent.id);
    Ok(agent)
}

fn parse_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, RosterError> {
    let text = fs::read_to_string(path).map_err(|e| {
        let kind = if e.kind() == std::io::ErrorKind::InvalidData {
            RosterErrorKind::Invalid
        } else {
            RosterErrorKind::Unavailable
        };
        error_of(kind, format!("{}: {e}", path.display()))
    })?;
    let format =
        crate::bootstrap::format::Format::for_path(path).map_err(|e| error(e.to_string()))?;
    crate::bootstrap::format::from_str(format, &text)
        .map_err(|e| error(format!("{}: {e}", path.display())))
}

fn parse_operation_agents(path: &Path) -> Result<AgentsDocument, RosterError> {
    let document: AgentsDocument = parse_file(path)?;
    if !document.role.is_empty() {
        return Err(error(format!(
            "{}: Agent roles must be configured in the global agents.toml",
            path.display()
        )));
    }
    if document.assistant.is_some() {
        return Err(error(format!(
            "{}: [assistant] must be configured in the global agents.toml",
            path.display()
        )));
    }
    Ok(document)
}

fn expand_path(path: &str) -> PathBuf {
    if path == "~" {
        return std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

fn validate_codename(id: &str) -> Result<(), RosterError> {
    if id.trim().is_empty() || id.trim() != id || id.chars().any(char::is_control) {
        return Err(error(
            "Agent codename must be non-empty and contain no control characters",
        ));
    }
    Ok(())
}

fn clearance_rank(clearance: &str) -> Result<usize, RosterError> {
    CLEARANCES
        .iter()
        .position(|known| *known == clearance)
        .ok_or_else(|| error(format!("unknown clearance {clearance:?}")))
}

fn validate_clearance(clearance: &str) -> Result<(), RosterError> {
    clearance_rank(clearance).map(|_| ())
}

/// Checks that `retire_on`, when set, is a calendar date written `YYYY-MM-DD`.
fn validate_retire_on(agent: &Agent) -> Result<(), RosterError> {
    let Some(date) = &agent.retire_on else {
        return Ok(());
    };
    let parts: Vec<&str> = date.split('-').collect();
    let number = |text: &str, digits: usize| {
        (text.len() == digits && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
    };
    let valid = match parts.as_slice() {
        [year, month, day] => match (number(year, 4), number(month, 2), number(day, 2)) {
            (Some(year), Some(month @ 1..=12), Some(day)) => {
                let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
                let days = match month {
                    2 if leap => 29,
                    2 => 28,
                    4 | 6 | 9 | 11 => 30,
                    _ => 31,
                };
                (1..=days).contains(&day)
            }
            _ => false,
        },
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(error(format!(
            "Agent {:?}: retire_on {date:?} is not a date written YYYY-MM-DD",
            agent.id
        )))
    }
}

pub(crate) fn error(message: impl Into<String>) -> RosterError {
    RosterError {
        message: message.into(),
        kind: RosterErrorKind::Invalid,
    }
}

/// A [`RosterError`] of another kind than [`RosterErrorKind::Invalid`].
pub(crate) fn error_of(kind: RosterErrorKind, message: impl Into<String>) -> RosterError {
    RosterError {
        message: message.into(),
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ai6-roster-{}-{}",
                std::process::id(),
                TEST_DIRECTORY_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, path: impl AsRef<Path>, text: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    static TEST_DIRECTORY_ID: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    #[test]
    fn an_operation_may_list_the_stations_allowed_to_run_it() {
        let directory = TestDirectory::new();
        directory.write(
            "operations.toml",
            "[operation.G]\nstations = [\"hq-1\", \"lab-gpu\"]\n[operation.H]\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        assert_eq!(roster.operation("G").unwrap().stations, ["hq-1", "lab-gpu"]);
        assert!(roster.operation("H").unwrap().stations.is_empty());

        directory.write("operations.toml", "[operation.G]\nstations = [\"\"]\n");
        assert!(Roster::load(&directory.0).is_err());
    }

    #[test]
    fn an_operation_may_list_its_pools_and_an_agent_may_pin_one_it_passes_on() {
        let directory = TestDirectory::new();
        directory.write(
            "operations.toml",
            "[operation.G]\npools = [\"claude-team\"]\n[operation.H]\n",
        );
        directory.write(
            "agents.toml",
            "[agent.007]\npool = \"claude-team\"\n[agent.008]\nextends = \"007\"\n[agent.009]\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        assert_eq!(
            roster.operation("G").unwrap().pools.as_deref(),
            Some(&["claude-team".to_owned()][..])
        );
        assert_eq!(roster.operation("H").unwrap().pools, None, "every pool");
        assert_eq!(
            roster.agent(None, "008").unwrap().pool.as_deref(),
            Some("claude-team")
        );
        assert_eq!(roster.agent(None, "009").unwrap().pool, None);
    }

    #[test]
    fn records_built_in_code_default_as_configuration_files_do() {
        let directory = TestDirectory::new();
        directory.write("operations.toml", "[operation.G]\n");
        directory.write("agents.toml", "[agent.007]\n");
        let from_files = Roster::load(&directory.0).unwrap();
        let built = Roster::builder()
            .operation(Operation::new("G"))
            .agent(Agent::new("007"))
            .build()
            .unwrap();
        assert_eq!(built.operation("G"), from_files.operation("G"));
        assert_eq!(built.agent(None, "007"), from_files.agent(None, "007"));
        assert_eq!(built.cover("covert").unwrap().name(), "covert");
    }

    #[test]
    fn records_built_in_code_are_checked_like_configuration_files() {
        let mut blown = Operation::new("G");
        blown.on_blown = "shrug".to_owned();
        let refused = Roster::builder().operation(blown).build().unwrap_err();
        assert_eq!(
            refused.to_string(),
            "Operation \"G\": on_blown must be flag or fail"
        );

        let mut named = Operation::new("G");
        named.name = Some("H".to_owned());
        assert!(
            Roster::builder()
                .operation(named)
                .operation(Operation::new("H"))
                .build()
                .is_err(),
            "a project name may not reuse another Operation's codename"
        );

        let mut cleared = Agent::new("007");
        cleared.clearance = "cosmic".to_owned();
        assert!(Roster::builder().agent(cleared).build().is_err());
        assert!(
            Roster::builder()
                .agent(Agent::new("007"))
                .agent(Agent::new("007"))
                .build()
                .is_err()
        );
        assert!(
            Roster::builder()
                .role(AgentRole::new("case officer", "Runs Missions.", "Report."))
                .build()
                .is_err()
        );
    }

    #[test]
    fn an_operation_checkout_may_extend_an_agent_built_in_code() {
        let directory = TestDirectory::new();
        directory.write(
            "project/.ai6/agents.toml",
            "[agent.008]\nextends = \"007\"\nmodel = \"local-model\"\n",
        );
        let mut operation = Operation::new("G");
        operation.path = Some(directory.0.join("project"));
        let mut agent = Agent::new("007");
        agent.handler = Some("claude".to_owned());
        agent.roles = vec!["plan".to_owned()];
        let roster = Roster::builder()
            .operation(operation)
            .agent(agent)
            .cover(Cover::new("plain", "Says nothing."))
            .build()
            .unwrap();

        let local = roster.agent(Some("G"), "008").unwrap();
        assert_eq!(local.handler.as_deref(), Some("claude"));
        assert_eq!(local.model.as_deref(), Some("local-model"));
        assert_eq!(local.roles, ["plan"]);
        assert!(roster.agent(None, "008").is_err());
        assert_eq!(roster.cover("plain").unwrap().name(), "plain");
    }

    #[test]
    fn resolves_agent_alias_inheritance_and_registered_operation_override() {
        let directory = TestDirectory::new();
        let checkout = directory.0.join("project");
        directory.write(
            "operations.toml",
            &format!(
                "[operation.G]\nname = \"project\"\npath = {:?}\nworkspace = {{ provider = \"git\", source = \"/srv/project\", base = \"main\", branch = \"ai6/{{id}}\", isolate = false }}\nroster = [\"JamesBond\"]\nclearance = \"00\"
station_labels = {{ station = \"dev\" }}
cover_identity = {{ name = \"Station\", email = \"station@example.invalid\" }}
sources = {{ jira = \"GUA\" }}\n",
                checkout.to_string_lossy()
            ),
        );
        directory.write(
            "agents.toml",
            "[agent.007]\naliases = [\"JamesBond\"]\nhandler = \"claude\"\nmodel = \"opus\"\nroles = [\"plan\"]\nclearance = \"00\"\nenv = { A = \"global\", B = \"base\" }\n[agent.q]\nextends = \"007\"\neffort = \"high\"\nenv = { B = \"child\" }\n",
        );
        directory.write(
            "project/.ai6/agents.toml",
            "[agent.007]\naliases = [\"JamesBond\"]\nhandler = \"codex\"\nmodel = \"local-model\"\nroles = [\"review\"]\nclearance = \"field\"\n",
        );

        let roster = Roster::load(&directory.0).unwrap();
        let operation = roster.operation("G").unwrap();
        assert_eq!(operation.workspace.provider.as_deref(), Some("git"));
        assert_eq!(
            operation.workspace.source.as_deref(),
            Some(Path::new("/srv/project"))
        );
        assert_eq!(operation.workspace.base, "main");
        assert_eq!(operation.workspace.branch, "ai6/{id}");
        assert!(!operation.workspace.isolate);
        assert_eq!(operation.cover_identity.as_ref().unwrap().name, "Station");
        assert_eq!(operation.sources["jira"].as_str(), Some("GUA"));
        let agent = roster.agent(Some("project"), "JamesBond").unwrap();
        assert_eq!(agent.id, "007");
        assert_eq!(agent.handler.as_deref(), Some("codex"));
        assert_eq!(agent.model.as_deref(), Some("local-model"));
        assert_eq!(agent.roles, ["review"]);
        assert_eq!(
            roster
                .agents_for_operation("G")
                .unwrap()
                .iter()
                .map(|agent| agent.id.as_str())
                .collect::<Vec<_>>(),
            ["007"]
        );

        let inherited = roster.agent(None, "q").unwrap();
        assert_eq!(inherited.handler.as_deref(), Some("claude"));
        assert_eq!(inherited.effort.as_deref(), Some("high"));
        assert_eq!(inherited.env.get("A").map(String::as_str), Some("global"));
        assert_eq!(inherited.env.get("B").map(String::as_str), Some("child"));
    }

    #[test]
    fn agent_watchdog_limits_merge_through_inheritance() {
        let directory = TestDirectory::new();
        directory.write(
            "agents.toml",
            "[agent.base]\n[agent.base.limits]\nstall_timeout = 45.5\nloop_min_lines = 7\nnormalize_digits = true\n[agent.child]\nextends = \"base\"\n[agent.child.limits]\nblocked_after = 1.25\nnormalize_digits = false\n",
        );

        let roster = Roster::load(&directory.0).unwrap();
        let agent = roster.agent(None, "child").unwrap();
        let defaults = crate::directorates::medical::watchdog::WatchdogConfig::default();
        let config = agent.limits.apply_to(&defaults);

        assert_eq!(
            config.stall_timeout(),
            std::time::Duration::from_millis(45_500)
        );
        assert_eq!(config.loop_min_lines(), 7);
        assert_eq!(
            config.blocked_after(),
            std::time::Duration::from_millis(1_250)
        );
        assert!(!config.normalize_digits());
    }

    #[test]
    fn role_backups_are_the_next_qualified_agent_with_an_available_handler() {
        let directory = TestDirectory::new();
        directory.write(
            "agents.toml",
            "[role.case-officer]\npurpose = \"Coordinate a bounded assignment\"\npolicy = \"Follow the brief and request authorization for restricted actions\"\n[agent.007]\nhandler = \"claude\"\nmodel = \"model-a\"\nroles = [\"case-officer\"]\npriority = 10\n[agent.008]\nhandler = \"codex\"\nmodel = \"model-b\"\nroles = [\"case-officer\"]\npriority = 5\n",
        );

        let roster = Roster::load(&directory.0).unwrap();
        let role = roster.role("case-officer/implementation").unwrap();
        assert_eq!(role.id, "case-officer");
        let first = roster
            .select_agent(&AgentQuery::new().role(Some("case-officer")))
            .unwrap();
        assert_eq!(first.id, "007");
        let backup = roster
            .select_agent(
                &AgentQuery::new()
                    .role(Some("case-officer"))
                    .handlers(Some(&["codex".to_owned()])),
            )
            .unwrap();
        assert_eq!(
            (backup.id.as_str(), backup.model.as_deref()),
            ("008", Some("model-b"))
        );
        assert!(
            roster
                .select_agent(
                    &AgentQuery::new()
                        .role(Some("case-officer"))
                        .handlers(Some(&["unavailable".to_owned()]))
                )
                .unwrap_err()
                .to_string()
                .contains("no available Agent")
        );
    }

    /// 2027-01-31T00:00:00Z.
    const RETIRE_DAY: i64 = 1_801_353_600;
    const DAY: i64 = 86_400;

    #[test]
    fn an_agent_retires_on_its_date_to_desk_duty_or_out_of_selection() {
        assert_eq!(utc_day(RETIRE_DAY), "2027-01-31");
        let directory = TestDirectory::new();
        directory.write("operations.toml", "[operation.G]\nclearance = \"00\"\n");
        directory.write(
            "agents.toml",
            "[agent.007]\nhandler = \"claude\"\nroles = [\"case-officer\"]\nclearance = \"00\"\npriority = 10\nretire_on = \"2027-01-31\"\n[agent.008]\nextends = \"007\"\non_retire = \"retired\"\nretire_warning_days = 3\n[agent.009]\nhandler = \"codex\"\nroles = [\"analyst\"]\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        let operation = roster.operation("G").unwrap();
        let desk = roster.agent(Some("G"), "007").unwrap();
        let retired = roster.agent(Some("G"), "008").unwrap();
        assert_eq!(
            (desk.on_retire, desk.retire_warning_days),
            (OnRetire::Desk, DEFAULT_RETIRE_WARNING_DAYS)
        );
        assert_eq!(
            retired.retire_on.as_deref(),
            Some("2027-01-31"),
            "inherited"
        );

        let long_before = RETIRE_DAY - 20 * DAY;
        assert_eq!(desk.retirement(long_before), Retirement::Serving);
        assert_eq!(desk.effective_clearance(operation, None, long_before), "00");
        let warned = RETIRE_DAY - 10 * DAY;
        assert_eq!(desk.retirement(warned), Retirement::Retiring);
        assert_eq!(
            retired.retirement(warned),
            Retirement::Serving,
            "3-day window"
        );
        assert_eq!(retired.retirement(RETIRE_DAY - DAY), Retirement::Retiring);
        assert_eq!(desk.effective_clearance(operation, None, warned), "00");

        let after = RETIRE_DAY + 60;
        assert_eq!(desk.retirement(after), Retirement::DeskDuty);
        assert_eq!(desk.effective_clearance(operation, None, after), "desk");
        assert_eq!(retired.retirement(after), Retirement::Retired);

        let query = AgentQuery::new()
            .operation(Some("G"))
            .role(Some("case-officer"));
        assert_eq!(roster.select_agent(&query.at(warned)).unwrap().id, "007");
        let only_desk = query.excluding(std::slice::from_ref(&desk.id));
        assert_eq!(
            roster.select_agent(&only_desk.at(warned)).unwrap().id,
            "008"
        );
        assert_eq!(
            roster
                .select_agent(&only_desk.at(after))
                .unwrap_err()
                .to_string(),
            "no qualified Agent for role \"case-officer\"; 008 retired on 2027-01-31"
        );
        assert_eq!(
            roster.select_agent(&only_desk).unwrap().id,
            "008",
            "without a time, retirement is not judged"
        );
        assert_eq!(roster.select_agent(&query.at(after)).unwrap().id, "007");

        for bad in ["2027-02-30", "2027-1-31", "31-01-2027", "2027-13-01", ""] {
            directory.write(
                "agents.toml",
                &format!("[agent.007]\nretire_on = {bad:?}\n"),
            );
            let roster = Roster::load(&directory.0).unwrap();
            assert!(roster.agent(None, "007").is_err(), "{bad}");
        }
        directory.write("agents.toml", "[agent.007]\nretire_on = \"2028-02-29\"\n");
        assert!(
            Roster::load(&directory.0)
                .unwrap()
                .agent(None, "007")
                .is_ok()
        );
        directory.write("agents.toml", "[agent.007]\non_retire = \"fired\"\n");
        assert!(Roster::load(&directory.0).is_err());
        let mut built = Agent::new("007");
        built.retire_on = Some("tomorrow".to_owned());
        assert!(Roster::builder().agent(built).build().is_err());
    }

    #[test]
    fn a_grant_raises_within_the_operations_ceiling_and_a_lower_one_only_lowers() {
        let mut operation = Operation::new("G");
        operation.clearance = "field".to_owned();
        let mut agent = Agent::new("007");
        agent.clearance = "desk".to_owned();
        let raise = |clearance: &str| {
            ClearanceGrant::raise("007", "G", clearance, "gi-1", "m", 10.0)
                .for_occupant(None, "desk")
        };
        assert_eq!(agent.effective_clearance(&operation, None, 20), "desk");
        assert_eq!(
            agent.effective_clearance(&operation, Some(&raise("field")), 20),
            "field"
        );
        assert_eq!(
            agent.effective_clearance(&operation, Some(&raise("00")), 20),
            "field",
            "never above the Operation's clearance"
        );
        let elsewhere = ClearanceGrant::raise("007", "L", "field", "gi-2", "m", 10.0)
            .for_occupant(None, "desk");
        assert_eq!(
            agent.effective_clearance(&operation, Some(&elsewhere), 20),
            "desk"
        );
        let lapsed = raise("field").expiring(15.0);
        assert_eq!(
            agent.effective_clearance(&operation, Some(&lapsed), 20),
            "desk"
        );
        // A new occupant, or a lower configured clearance, does not inherit the raise.
        agent.model = Some("next-model".to_owned());
        assert_eq!(
            agent.effective_clearance(&operation, Some(&raise("field")), 20),
            "desk"
        );
        agent.model = None;
        let mut higher = operation.clone();
        higher.clearance = "00".to_owned();
        let from_field =
            ClearanceGrant::raise("007", "G", "00", "gi-3", "m", 10.0).for_occupant(None, "field");
        agent.clearance = "field".to_owned();
        assert_eq!(
            agent.effective_clearance(&higher, Some(&from_field), 20),
            "00"
        );
        agent.clearance = "desk".to_owned();
        assert_eq!(
            agent.effective_clearance(&higher, Some(&from_field), 20),
            "desk",
            "lowering the configured clearance takes the raise away"
        );

        agent.clearance = "field".to_owned();
        let lower = |clearance: &str| ClearanceGrant::lower("007", "G", clearance, "ops", 10.0);
        assert_eq!(
            agent.effective_clearance(&operation, Some(&lower("desk")), 20),
            "desk"
        );
        assert_eq!(
            agent.effective_clearance(&operation, Some(&lower("00")), 20),
            "field"
        );
    }

    #[test]
    fn example_roster_loads_roles_and_their_agents() {
        let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config");
        let roster = Roster::load(config).unwrap();
        let roles = roster
            .roles()
            .map(|role| role.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            roles,
            [
                "analyst",
                "case-officer",
                "chief-of-staff",
                "head-of-station",
                "reports-officer",
                "senior-analyst",
                "targeter",
                "technical-officer",
            ]
        );
        for role in roles {
            assert!(
                roster
                    .select_agent(&AgentQuery::new().operation(Some("G")).role(Some(role)))
                    .is_ok(),
                "no example Agent is qualified for {role}"
            );
        }
    }

    #[test]
    fn rejects_roles_without_a_purpose_and_policy() {
        let directory = TestDirectory::new();
        directory.write(
            "agents.toml",
            "[role.case-officer]\npurpose = \"\"\npolicy = \"\"\n",
        );

        assert!(Roster::load(&directory.0).is_err());
    }

    #[test]
    fn roster_rejects_invalid_agent_watchdog_values() {
        let directory = TestDirectory::new();
        directory.write("agents.toml", "[agent.007.limits]\nstall_timeout = -1.0\n");

        assert!(Roster::load(&directory.0).is_err());
    }

    #[test]
    fn an_operation_rotates_accounts_on_a_provider_limit_unless_it_asks_otherwise() {
        let directory = TestDirectory::new();
        directory.write(
            "operations.toml",
            "[operation.A]\n[operation.B]\non_limit = \"backup\"\nlimit_fallback_secs = 90\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        let rotate = roster.operation("A").unwrap();
        assert_eq!(rotate.on_limit, OnLimit::Rotate);
        assert_eq!(rotate.limit_fallback_secs, DEFAULT_LIMIT_FALLBACK_SECS);
        let backup = roster.operation("B").unwrap();
        assert_eq!(backup.on_limit, OnLimit::Backup);
        assert_eq!(backup.limit_fallback_secs, 90);

        for invalid in ["on_limit = \"retry\"", "limit_fallback_secs = 0"] {
            directory.write("operations.toml", &format!("[operation.C]\n{invalid}\n"));
            assert!(Roster::load(&directory.0).is_err(), "{invalid}");
        }
    }

    #[test]
    fn a_lost_session_is_retried_and_checkpoints_push_nothing_unless_configured() {
        let directory = TestDirectory::new();
        directory.write(
            "operations.toml",
            "[operation.A]\n[operation.B]\non_lost = \"fail\"\nplacement = \"pack\"\nlost_after_secs = 10\nworkspace = { checkpoint_remote = \"origin\", checkpoint_secs = 60 }\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        let defaults = roster.operation("A").unwrap();
        assert_eq!(defaults.on_lost, OnLost::Retry);
        assert_eq!(defaults.placement, PlacementStrategy::Spread);
        assert_eq!(
            roster.operation("B").unwrap().placement,
            PlacementStrategy::Pack
        );
        assert_eq!(defaults.lost_after_secs, DEFAULT_LOST_AFTER_SECS);
        assert_eq!(defaults.workspace.checkpoint_remote, None);
        assert_eq!(defaults.workspace.checkpoint_secs, DEFAULT_CHECKPOINT_SECS);
        let set = roster.operation("B").unwrap();
        assert_eq!(set.on_lost, OnLost::Fail);
        assert_eq!(set.lost_after_secs, 10);
        assert_eq!(set.workspace.checkpoint_remote.as_deref(), Some("origin"));
        assert_eq!(set.workspace.checkpoint_secs, 60);

        for invalid in [
            "on_lost = \"ignore\"",
            "placement = \"random\"",
            "lost_after_secs = 0",
            "workspace = { checkpoint_secs = 0 }",
            "workspace = { checkpoint_remote = \" \" }",
        ] {
            directory.write("operations.toml", &format!("[operation.C]\n{invalid}\n"));
            assert!(Roster::load(&directory.0).is_err(), "{invalid}");
        }
    }

    #[test]
    fn budgets_are_unset_by_default_and_layer_from_preset_to_agent() {
        let directory = TestDirectory::new();
        directory.write(
            "operations.toml",
            "[operation.A]\n[operation.B.budget]\npreset = \"strict\"\nmission_cost_usd = 3\n",
        );
        directory.write(
            "agents.toml",
            "[agent.base.budget]\nsession_tokens = 1000\n[agent.child]\nextends = \"base\"\n[agent.child.budget]\nwarn_at = 0.5\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        assert!(!roster.operation("A").unwrap().budget.is_limited());
        let strict = roster.operation("B").unwrap().budget;
        assert_eq!(strict.mission_cost_micro_usd, Some(3_000_000));
        assert!(strict.session_tokens.is_some());
        let child = roster.agent(None, "child").unwrap().budget;
        assert_eq!(child.session_tokens, Some(1000));
        assert_eq!(child.warn_at_permille, Some(500));
    }

    #[test]
    fn a_negative_or_non_numeric_budget_is_refused_naming_its_key() {
        let directory = TestDirectory::new();
        for (file, text, key) in [
            (
                "operations.toml",
                "[operation.A.budget]\nmission_tokens = -5\n",
                "mission_tokens",
            ),
            (
                "operations.toml",
                "[operation.A.budget]\nmission_cost_usd = \"lots\"\n",
                "mission_cost_usd",
            ),
            (
                "agents.toml",
                "[agent.007.budget]\nsession_tokens = 1.5\n",
                "session_tokens",
            ),
            (
                "agents.toml",
                "[agent.007.budget]\nwarn_at = 2\n",
                "warn_at",
            ),
        ] {
            directory.write(file, text);
            let message = Roster::load(&directory.0).unwrap_err().to_string();
            assert!(message.contains(key), "{text}: {message}");
            directory.write(file, "");
        }
    }

    #[test]
    fn rejects_unregistered_operation_and_operation_policy_violations() {
        let directory = TestDirectory::new();
        directory.write(
            "operations.toml",
            "[operation.G]\nname = \"project\"\nroster = [\"007\"]\nclearance = \"field\"\n",
        );
        directory.write(
            "agents.toml",
            "[agent.007]\nhandler = \"claude\"\nclearance = \"00\"\n[agent.006]\nhandler = \"claude\"\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        assert!(roster.agent(Some("not-registered"), "007").is_err());
        assert!(roster.agent(Some("G"), "007").is_err());
        assert!(roster.agent(Some("G"), "006").is_err());
    }

    #[test]
    fn workspace_defaults_to_git_only_for_a_git_checkout() {
        let directory = TestDirectory::new();
        let checkout = directory.0.join("checkout");
        let plain = directory.0.join("plain");
        fs::create_dir_all(checkout.join(".git")).unwrap();
        fs::create_dir_all(&plain).unwrap();
        directory.write(
            "operations.toml",
            &format!(
                "[operation.A]\npath = {:?}\n[operation.B]\npath = {:?}\n[operation.C]\npath = {:?}\nworkspace = {{ provider = \"copy\" }}\n[operation.D]\npath = {:?}\nworkspace = {{ isolat = true }}\n",
                checkout.to_string_lossy(),
                plain.to_string_lossy(),
                plain.to_string_lossy(),
                plain.to_string_lossy()
            ),
        );
        assert!(Roster::load(&directory.0).is_err(), "misspelled key");
        directory.write(
            "operations.toml",
            &format!(
                "[operation.A]\npath = {:?}\n[operation.B]\npath = {:?}\n[operation.C]\npath = {:?}\nworkspace = {{ provider = \"copy\" }}\n",
                checkout.to_string_lossy(),
                plain.to_string_lossy(),
                plain.to_string_lossy()
            ),
        );
        let roster = Roster::load(&directory.0).unwrap();
        let a = roster.operation("A").unwrap();
        assert_eq!(a.safe_house_provider().as_deref(), Some("git"));
        assert!(a.workspace.isolate);
        assert_eq!(
            (a.workspace.base.as_str(), a.workspace.branch.as_str()),
            ("HEAD", "{id}-{slug}")
        );
        assert_eq!(roster.operation("B").unwrap().safe_house_provider(), None);
        assert_eq!(
            roster
                .operation("C")
                .unwrap()
                .safe_house_provider()
                .as_deref(),
            Some("copy")
        );
    }

    #[test]
    fn green_ink_is_strict_by_default_and_an_operation_may_choose_licensed() {
        use crate::directorates::m::approvals::{ActionScope, Requirement};
        let directory = TestDirectory::new();
        directory.write(
            "operations.toml",
            "[operation.A]\n[operation.B.green_ink]\npreset = \"licensed\"\nrequest_ttl_secs = 60\n\
             [operation.C.green_ink]\nunattended = { field = [\"nuke\"] }\n",
        );
        let push = ActionScope::try_new("push", "origin:main").unwrap();
        let roster = Roster::load(&directory.0);
        let message = roster.as_ref().unwrap_err().to_string();
        assert!(
            message.contains("Operation \"C\"") && message.contains("nuke"),
            "{message}"
        );

        directory.write(
            "operations.toml",
            "[operation.A]\n[operation.B.green_ink]\npreset = \"licensed\"\nrequest_ttl_secs = 60\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        let strict = &roster.operation("A").unwrap().green_ink;
        assert_eq!(strict.requirement("00", &push), Requirement::GreenInk);
        assert!(strict.weakenings().is_empty());
        let licensed = &roster.operation("B").unwrap().green_ink;
        assert_eq!(licensed.requirement("00", &push), Requirement::NotRequired);
        assert_eq!(licensed.requirement("field", &push), Requirement::GreenInk);
        assert_eq!(licensed.request_ttl(), std::time::Duration::from_secs(60));
    }

    #[test]
    fn the_assistant_table_lives_in_the_global_agents_file_only() {
        let directory = TestDirectory::new();
        assert_eq!(
            Roster::load(&directory.0).unwrap().assistant(),
            &AssistantSettings::default()
        );
        directory.write(
            "agents.toml",
            "[agent.moneypenny]\nhandler = \"claude\"\n[assistant]\npersonal_repo = \"/srv/mp/{person}.git\"\n[assistant.shared.ops]\nrepo = \"/srv/ops.git\"\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        assert_eq!(
            roster
                .assistant()
                .personal_repo_for("007")
                .unwrap()
                .as_deref(),
            Some("/srv/mp/007.git")
        );
        assert_eq!(roster.assistant().shared()["ops"].repo(), "/srv/ops.git");

        directory.write("agents.toml", "[assistant]\npersonal_repo = \"ext::sh\"\n");
        let refused = Roster::load(&directory.0).unwrap_err().to_string();
        assert!(refused.contains("assistant.personal_repo"), "{refused}");

        let checkout = directory.0.join("project");
        directory.write(
            "operations.toml",
            &format!("[operation.G]\npath = {:?}\n", checkout.to_string_lossy()),
        );
        directory.write("agents.toml", "[agent.007]\nhandler = \"claude\"\n");
        directory.write(
            "project/.ai6/agents.toml",
            "[assistant]\nallow_elevated = true\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        let refused = roster.agent(Some("G"), "007").unwrap_err().to_string();
        assert!(refused.contains("[assistant]"), "{refused}");

        let built = Roster::builder()
            .assistant(AssistantSettings::new().with_allow_elevated(true))
            .build()
            .unwrap();
        assert!(built.assistant().allow_elevated());
        assert!(
            Roster::builder()
                .assistant(AssistantSettings::new().with_personal_repo("fd::3"))
                .build()
                .is_err()
        );
    }

    #[test]
    fn a_skill_path_in_agents_toml_is_relative_to_its_directory_and_its_skill_md_is_read() {
        let directory = TestDirectory::new();
        directory.write(
            "skills/charts/SKILL.md",
            "---\nname: charts\ndescription: Draws charts.\nallowed-tools: Bash\n---\nDraw.\n",
        );
        let pin = crate::ProjectManifest::read_tree(&directory.0.join("skills/charts"))
            .unwrap()
            .digest();
        let agents = |extra: &str| {
            format!(
                "[assistant]\n{extra}[assistant.skills.charts]\npath = \"skills/charts\"\n\
                 pin = \"{pin}\"\n"
            )
        };
        directory.write("agents.toml", &agents(""));
        let refused = Roster::load(&directory.0).unwrap_err().to_string();
        assert!(
            refused.contains("assistant.skills.charts") && refused.contains("allowed-tools"),
            "{refused}"
        );
        directory.write("agents.toml", &agents("skills_allow_tools = true\n"));
        let roster = Roster::load(&directory.0).unwrap();
        let path = roster.assistant().skills()["charts"].path();
        assert_eq!(
            Path::new(path),
            directory.0.join("skills").join("charts").as_path()
        );
    }

    #[test]
    fn rejects_bad_clearance_and_inheritance_cycles() {
        let directory = TestDirectory::new();
        directory.write(
            "operations.toml",
            "[operation.G]\nclearance = \"unlimited\"\n",
        );
        assert!(Roster::load(&directory.0).is_err());

        directory.write("operations.toml", "");
        directory.write(
            "agents.toml",
            "[agent.a]\nextends = \"b\"\n[agent.b]\nextends = \"a\"\n",
        );
        let roster = Roster::load(&directory.0).unwrap();
        assert!(
            roster
                .agent(None, "a")
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
    }
}
