//! Registered Operations and the Agents posted to them.
//!
//! Global configuration lives in `operations.toml` and `agents.toml`. A Operation's
//! `.ai6/agents.toml` is considered only after that Operation is found in the global
//! registry.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::directorates::m::cover::{Cover, CoverError, Covers};
use crate::directorates::medical::watchdog::WatchdogOverrides;
use crate::directorates::registry::sleepers::Sleeper;
use crate::directorates::registry::store::validate_station_id;

const AGENTS_FILE: &str = "agents.toml";
const OPERATIONS_FILE: &str = "operations.toml";
const CLEARANCES: &[&str] = &["desk", "field", "00"];

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
    /// Optional concurrent operation limit.
    pub max_concurrent: Option<usize>,
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
    /// Work sources (trackers, CI) this Operation takes briefs from.
    pub sources: toml::Table,
    /// Standing orders that queue a brief on a schedule, once an operator authorizes them.
    pub sleepers: BTreeMap<String, Sleeper>,
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
    agents: BTreeMap<String, AgentDefinition>,
    roles: BTreeMap<String, AgentRole>,
    #[cfg_attr(
        not(feature = "daemon"),
        allow(dead_code, reason = "used only by the daemon")
    )]
    covers: Covers,
}

/// Configuration could not be read or resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterError(String);

impl std::fmt::Display for RosterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RosterError {}

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
                let operation = definition.resolve(id, &operation_file)?;
                for key in operation.name.iter().chain(std::iter::once(&operation.id)) {
                    if roster.operations.values().any(|other| {
                        other.id == *key || other.name.as_deref() == Some(key.as_str())
                    }) {
                        return Err(error(format!(
                            "{}: Operation key {key:?} is already registered",
                            operation_file.display()
                        )));
                    }
                }
                roster.operations.insert(operation.id.clone(), operation);
            }
        }
        let agents_file = root.join(AGENTS_FILE);
        if agents_file.exists() {
            let document: AgentsDocument = parse_file(&agents_file)?;
            roster.agents = document.agent;
            for (id, definition) in document.role {
                roster
                    .roles
                    .insert(id.clone(), definition.resolve(id, &agents_file)?);
            }
        }
        Ok(roster)
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
            && let Some(path) = self.operation_agent_file(operation)
            && path.exists()
        {
            let local = parse_operation_agents(&path)?;
            definitions.extend(local.agent);
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

    /// Selects the highest-priority Agent satisfying a role and every requested tag.
    ///
    /// With `available_handlers`, an Agent whose handler is not in the list is skipped, so
    /// the next qualified Agent (its backup) is chosen instead.
    pub fn select_agent(
        &self,
        operation_key: Option<&str>,
        role: Option<&str>,
        tags: &[String],
        available_handlers: Option<&[String]>,
    ) -> Result<Agent, RosterError> {
        let mut keys: BTreeSet<String> = self.agents.keys().cloned().collect();
        if let Some(operation) = operation_key.map(|key| self.operation(key)).transpose()?
            && let Some(path) = self.operation_agent_file(operation)
            && path.exists()
        {
            let local = parse_operation_agents(&path)?;
            keys.extend(local.agent.into_keys());
        }

        let mut candidates = BTreeMap::new();
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
            let available = available_handlers.is_none_or(|handlers| {
                agent
                    .handler
                    .as_ref()
                    .is_some_and(|handler| handlers.contains(handler))
            });
            if role_matches && tags_match && available {
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
                error(format!(
                    "no available Agent matches role {wanted:?} and tags {}",
                    tags.join(", ")
                ))
            })
    }

    /// Lists the Agents posted to a registered Operation after applying its roster policy.
    pub fn agents_for_operation(&self, operation_key: &str) -> Result<Vec<Agent>, RosterError> {
        let operation = self.operation(operation_key)?;
        let mut keys: BTreeSet<String> = self.agents.keys().cloned().collect();
        if let Some(path) = self.operation_agent_file(operation)
            && path.exists()
        {
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

    fn operation_agent_file(&self, operation: &Operation) -> Option<PathBuf> {
        operation
            .config
            .clone()
            .or_else(|| {
                if operation.theater.is_none() {
                    operation.path.as_ref().map(|path| path.join(".ai6"))
                } else {
                    None
                }
            })
            .map(|directory| directory.join(AGENTS_FILE))
    }
}

/// Resolves the standard per-user AI6 configuration directory.
pub fn config_dir() -> Result<PathBuf, RosterError> {
    if let Some(path) = std::env::var_os("AI6_CONFIG_DIR") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("ai6"));
    }
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("APPDATA") {
        return Ok(PathBuf::from(path).join("ai6"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| error("could not determine the user's configuration directory"))?;
    Ok(PathBuf::from(home).join(".config").join("ai6"))
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
    // Keys of the earlier workspace settings, kept only to refuse them by name.
    repos: Option<toml::Value>,
    safe_houses: Option<toml::Value>,
    branch: Option<toml::Value>,
    base: Option<toml::Value>,
    station_labels: BTreeMap<String, String>,
    stations: Vec<String>,
    cover: String,
    cover_identity: Option<CoverIdentity>,
    on_blown: String,
    sources: toml::Table,
    sleepers: BTreeMap<String, Sleeper>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct WorkspaceDefinition {
    provider: Option<String>,
    source: Option<String>,
    base: String,
    branch: String,
    isolate: bool,
}

impl Default for WorkspaceDefinition {
    fn default() -> Self {
        Self {
            provider: None,
            source: None,
            base: String::new(),
            branch: String::new(),
            isolate: true,
        }
    }
}

impl OperationDefinition {
    fn refuse_replaced_keys(&self, id: &str, source: &Path) -> Result<(), RosterError> {
        for (key, replacement) in [
            ("repos", "workspace = { source = \"..\" }"),
            ("safe_houses", "workspace = { isolate = true | false }"),
            ("branch", "workspace = { branch = \"..\" }"),
            ("base", "workspace = { base = \"..\" }"),
        ] {
            let present = match key {
                "repos" => self.repos.is_some(),
                "safe_houses" => self.safe_houses.is_some(),
                "branch" => self.branch.is_some(),
                _ => self.base.is_some(),
            };
            if present {
                return Err(error(format!(
                    "{}: Operation {id:?}: `{key}` is no longer an Operation key; use {replacement}",
                    source.display()
                )));
            }
        }
        Ok(())
    }
}

impl OperationDefinition {
    fn resolve(self, id: String, source: &Path) -> Result<Operation, RosterError> {
        self.refuse_replaced_keys(&id, source)?;
        let path = self.path.map(|path| expand_path(&path));
        let config = self.config.map(|path| expand_path(&path));
        let operation = Operation {
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
                    "HEAD".to_owned()
                } else {
                    self.workspace.base
                },
                branch: if self.workspace.branch.is_empty() {
                    "{id}-{slug}".to_owned()
                } else {
                    self.workspace.branch
                },
                isolate: self.workspace.isolate,
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
            sources: self.sources,
            sleepers: self.sleepers,
        };
        for (name, sleeper) in &operation.sleepers {
            let valid_name = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
            if !valid_name {
                return Err(error(format!(
                    "{}: Operation {:?}: sleeper name {name:?} may use only letters, digits, '-' and '_'",
                    source.display(),
                    operation.id
                )));
            }
            sleeper.validate().map_err(|e| {
                error(format!(
                    "{}: Operation {:?}: sleeper {name:?}: {e}",
                    source.display(),
                    operation.id
                ))
            })?;
        }
        if let Some(station) = operation
            .stations
            .iter()
            .find(|station| validate_station_id(station).is_err())
        {
            return Err(error(format!(
                "{}: Operation {:?}: stations entry {station:?} is not a valid Station ID",
                source.display(),
                operation.id
            )));
        }
        validate_clearance(&operation.clearance).map_err(|e| {
            error(format!(
                "{}: Operation {:?}: {e}",
                source.display(),
                operation.id
            ))
        })?;
        if operation
            .workspace
            .provider
            .as_deref()
            .is_some_and(|provider| provider.trim().is_empty())
        {
            return Err(error(format!(
                "{}: Operation {:?}: workspace.provider must not be empty",
                source.display(),
                operation.id
            )));
        }
        if !matches!(operation.on_blown.as_str(), "flag" | "fail") {
            return Err(error(format!(
                "{}: Operation {:?}: on_blown must be flag or fail",
                source.display(),
                operation.id
            )));
        }
        if let Some(identity) = &operation.cover_identity
            && (identity.name.trim().is_empty() || identity.email.trim().is_empty())
        {
            return Err(error(format!(
                "{}: Operation {:?}: cover_identity needs name and email",
                source.display(),
                operation.id
            )));
        }
        Ok(operation)
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
    max_concurrent: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AgentRoleDefinition {
    purpose: String,
    policy: String,
}

impl AgentRoleDefinition {
    fn resolve(self, id: String, source: &Path) -> Result<AgentRole, RosterError> {
        let role = AgentRole {
            id,
            purpose: self.purpose,
            policy: self.policy,
        };
        role.validate()
            .map_err(|validation| error(format!("{}: {validation}", source.display())))?;
        Ok(role)
    }
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
    definitions: &BTreeMap<String, AgentDefinition>,
    seen: &mut BTreeSet<String>,
) -> Result<Agent, RosterError> {
    let id = if definitions.contains_key(key) {
        key.to_owned()
    } else {
        let mut matches = definitions
            .iter()
            .filter(|(_, value)| value.aliases.iter().any(|alias| alias == key))
            .map(|(id, _)| id.clone());
        let id = matches
            .next()
            .ok_or_else(|| error(format!("no configured Agent or alias {key:?}")))?;
        if matches.next().is_some() {
            return Err(error(format!("Agent alias {key:?} is ambiguous")));
        }
        id
    };
    if id.trim().is_empty() || id.trim() != id || id.chars().any(char::is_control) {
        return Err(error(
            "Agent codename must be non-empty and contain no control characters",
        ));
    }
    if !seen.insert(id.clone()) {
        return Err(error(format!("Agent inheritance cycle at {id:?}")));
    }
    let definition = &definitions[&id];
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
        max_concurrent: definition
            .max_concurrent
            .or_else(|| base.as_ref().and_then(|agent| agent.max_concurrent)),
    };
    validate_clearance(&agent.clearance)?;
    seen.remove(&agent.id);
    Ok(agent)
}

fn parse_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, RosterError> {
    let text = fs::read_to_string(path).map_err(|e| error(format!("{}: {e}", path.display())))?;
    toml::from_str(&text).map_err(|e| error(format!("{}: {e}", path.display())))
}

fn parse_operation_agents(path: &Path) -> Result<AgentsDocument, RosterError> {
    let document: AgentsDocument = parse_file(path)?;
    if !document.role.is_empty() {
        return Err(error(format!(
            "{}: Agent roles must be configured in the global agents.toml",
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

fn clearance_rank(clearance: &str) -> Result<usize, RosterError> {
    CLEARANCES
        .iter()
        .position(|known| *known == clearance)
        .ok_or_else(|| error(format!("unknown clearance {clearance:?}")))
}

fn validate_clearance(clearance: &str) -> Result<(), RosterError> {
    clearance_rank(clearance).map(|_| ())
}

fn error(message: impl Into<String>) -> RosterError {
    RosterError(message.into())
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
            .select_agent(None, Some("case-officer"), &[], None)
            .unwrap();
        assert_eq!(first.id, "007");
        let backup = roster
            .select_agent(None, Some("case-officer"), &[], Some(&["codex".to_owned()]))
            .unwrap();
        assert_eq!(
            (backup.id.as_str(), backup.model.as_deref()),
            ("008", Some("model-b"))
        );
        assert!(
            roster
                .select_agent(
                    None,
                    Some("case-officer"),
                    &[],
                    Some(&["unavailable".to_owned()])
                )
                .unwrap_err()
                .to_string()
                .contains("no available Agent")
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
                "reports-officer",
                "senior-analyst",
                "targeter",
                "technical-officer",
            ]
        );
        for role in roles {
            assert!(
                roster
                    .select_agent(Some("G"), Some(role), &[], None)
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
    fn replaced_workspace_keys_are_refused_by_name() {
        for (key, line) in [
            ("repos", "repos = [\"https://example.invalid/repo\"]"),
            ("safe_houses", "safe_houses = false"),
            ("branch", "branch = \"{id}\""),
            ("base", "base = \"main\""),
        ] {
            let directory = TestDirectory::new();
            directory.write("operations.toml", &format!("[operation.G]\n{line}\n"));
            let message = Roster::load(&directory.0).unwrap_err().to_string();
            assert!(
                message.contains(&format!("`{key}` is no longer an Operation key"))
                    && message.contains("workspace"),
                "{message}"
            );
        }
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
