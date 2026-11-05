//! Q Branch execution resources: validated loadouts, secret leases and terminals.

mod mcp;
mod pools;
mod sandbox;
mod secrets;

pub mod ephemeral;
#[cfg(feature = "daemon")]
pub mod logins;
pub mod outbound;
pub mod overlays;
pub mod provision;
pub mod safehouse;
#[cfg(feature = "daemon")]
pub mod sealing;
pub mod secret_kind;
pub mod terminal;
pub mod transport;

pub use mcp::claude_mcp_config;
pub use pools::{
    Account, AccountKind, AccountLogin, AccountPool, CREDENTIAL_VARIABLES, CommandLogin,
    PoolChoice, PoolStrategy, RouteStanding, choose_pools, choose_route,
};
pub use sandbox::{SandboxError, SandboxRequest};
pub use secret_kind::{SecretFields, SecretKeeper, SecretProvider, SecretSource};
pub use secrets::{
    SecretBroker, SecretError, SecretLease, SecretLeaseRequest, SecretProviderLookup, SecretValue,
};

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::bounds::Limit;
use crate::directorates::m::config::ConfigLimits;
use guatiao::{FromValue, Schema, ToValue};
use serde::Deserialize;

/// Stage at which a loadout operation failed.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QErrorKind {
    /// The configured path could not be read.
    Io,
    /// The document exceeded its size limit.
    TooLarge,
    /// The file did not contain valid UTF-8.
    InvalidUtf8,
    /// The document is not valid TOML.
    Parse,
    /// Parsed TOML violates the Q Branch schema or references.
    Schema,
}

/// A loadout read, parse or validation failure.
#[derive(Debug)]
pub struct QError {
    kind: QErrorKind,
    path: Option<PathBuf>,
    line: Option<usize>,
    column: Option<usize>,
    offset: Option<usize>,
    reason: Option<String>,
    source: Option<io::Error>,
}

impl QError {
    fn new(kind: QErrorKind, path: Option<PathBuf>) -> Self {
        Self {
            kind,
            path,
            line: None,
            column: None,
            offset: None,
            reason: None,
            source: None,
        }
    }

    fn because(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// What is wrong, when the check can say without echoing the input: a pool or account
    /// name and the setting at fault.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    fn at(mut self, source: &str, offset: usize) -> Self {
        let mut offset = offset.min(source.len());
        while !source.is_char_boundary(offset) {
            offset -= 1;
        }
        let prefix = &source[..offset];
        self.line = Some(prefix.bytes().filter(|byte| *byte == b'\n').count() + 1);
        self.column = Some(
            prefix
                .rsplit('\n')
                .next()
                .map_or(1, |line| line.chars().count() + 1),
        );
        self.offset = Some(offset);
        self
    }

    fn with_source(mut self, source: io::Error) -> Self {
        self.source = Some(source);
        self
    }

    /// Returns the stage that failed.
    pub const fn kind(&self) -> QErrorKind {
        self.kind
    }

    /// Returns the configured file path, when loading from disk.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Returns the one-based line, when the parser supplied a source position.
    pub const fn line(&self) -> Option<usize> {
        self.line
    }

    /// Returns the one-based column, when the parser supplied a source position.
    pub const fn column(&self) -> Option<usize> {
        self.column
    }

    /// Returns the zero-based byte offset, when the input encoding supplied one.
    pub const fn offset(&self) -> Option<usize> {
        self.offset
    }
}

impl std::fmt::Display for QError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            QErrorKind::Io => write!(f, "could not read Q Branch loadout")?,
            QErrorKind::TooLarge => write!(f, "Q Branch loadout exceeds its size limit")?,
            QErrorKind::InvalidUtf8 => write!(f, "Q Branch loadout is not valid UTF-8")?,
            QErrorKind::Parse => write!(f, "invalid Q Branch TOML")?,
            QErrorKind::Schema => write!(f, "invalid Q Branch loadout schema")?,
        }
        if let Some(path) = &self.path {
            write!(f, " at {}", path.display())?;
        }
        if let (Some(line), Some(column)) = (self.line, self.column) {
            write!(f, ":{line}:{column}")?;
        }
        if let Some(reason) = &self.reason {
            write!(f, ": {reason}")?;
        }
        Ok(())
    }
}

impl std::error::Error for QError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// Values a Session is given in its environment that are no secret: `env` on a record that
/// says how a Session runs. Anyone who can read that record reads them, and exposure does not
/// take them away; [`LoadoutCatalog::check_session_env`] keeps a credential's variables out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, ToValue, FromValue, Schema)]
#[serde(transparent)]
#[map(crate = ::guatiao, transparent)]
pub struct SessionEnv(BTreeMap<String, String>);

impl From<BTreeMap<String, String>> for SessionEnv {
    fn from(values: BTreeMap<String, String>) -> Self {
        Self(values)
    }
}

impl SessionEnv {
    /// The values by variable name.
    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    /// Why an `env` on a roster record cannot be set: a name that is no variable, AI6's own or a
    /// provider credential's ([`CREDENTIAL_VARIABLES`]). The catalog's own variables are checked
    /// when a Station has one ([`LoadoutCatalog::check_session_env`]).
    pub(crate) fn check_on_roster(&self) -> Result<(), String> {
        let taken: Vec<String> = CREDENTIAL_VARIABLES
            .iter()
            .map(|variable| (*variable).to_owned())
            .collect();
        self.check(&taken, Limit::UNBOUNDED)
    }

    /// Why a name or value cannot be set, ignoring case in `taken` as Windows does.
    fn check(&self, taken: &[String], chars: Limit) -> Result<(), String> {
        for (name, value) in &self.0 {
            if !transport::kit::is_variable_name(name) {
                return Err(format!("env {name:?} is not a variable name"));
            }
            if value.chars().any(char::is_control) || !chars.allows(value.chars().count()) {
                return Err(format!(
                    "the value of env {name} has a control character or is over {chars} characters \
                     (`[provider] loadout_value_chars`)"
                ));
            }
            let upper = name.to_ascii_uppercase();
            if upper.starts_with("AI6_") {
                return Err(format!("env {name} is one of AI6's own variables"));
            }
            if taken
                .iter()
                .any(|variable| variable.eq_ignore_ascii_case(name))
            {
                return Err(format!(
                    "env {name} is a credential's variable: a value here is shown to anyone who \
                     reads the record, so a credential belongs in a [secret.<name>]"
                ));
            }
        }
        Ok(())
    }
}

/// One Agent's named sandbox, MCP servers, overlays and secret references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loadout {
    agent: String,
    sandbox: Option<String>,
    mcp: Vec<String>,
    overlays: Vec<String>,
    secrets: Vec<String>,
    env: SessionEnv,
}

impl Loadout {
    /// Returns the configured Agent codename.
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// Returns the optional sandbox profile name.
    pub fn sandbox_name(&self) -> Option<&str> {
        self.sandbox.as_deref()
    }

    /// Returns MCP server names in the order declared by this loadout.
    pub fn mcp_names(&self) -> &[String] {
        &self.mcp
    }

    /// The overlays it issues, by `[overlay.<name>]` name, in the order declared; a Session
    /// gets them with its Operation's in composition order ([`overlays::issue_overlays`]).
    pub fn overlay_names(&self) -> &[String] {
        &self.overlays
    }

    /// Returns secret reference names, never their values.
    pub fn secret_names(&self) -> &[String] {
        &self.secrets
    }

    /// The values a Session of this Agent is given that are no secret.
    pub fn env(&self) -> &SessionEnv {
        &self.env
    }

    /// The loadout that declares only a pool account's secret, so a broker leases it and
    /// nothing else; `None` for an account that reads no secret.
    pub fn for_account(pool: &AccountPool, account: &Account) -> Option<Self> {
        Some(Self {
            agent: format!("pool:{}/{}", pool.id(), account.name()),
            sandbox: None,
            mcp: Vec::new(),
            overlays: Vec::new(),
            secrets: vec![account.secret_name()?.to_owned()],
            env: SessionEnv::default(),
        })
    }

    /// The loadout that declares only the secrets a route of `agent` names, so a broker leases
    /// them and nothing else.
    pub fn for_route(agent: &str, secrets: &[String]) -> Self {
        Self {
            agent: format!("route:{agent}"),
            sandbox: None,
            mcp: Vec::new(),
            overlays: Vec::new(),
            secrets: secrets.to_vec(),
            env: SessionEnv::default(),
        }
    }
}

/// An inert sandbox policy; it does not resolve paths or run a sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxProfile {
    name: String,
    backend: String,
    read_write: Vec<String>,
    read_only: Vec<String>,
    network: Vec<String>,
}

impl SandboxProfile {
    /// Returns the profile name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the sandbox backend identifier.
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// Returns writable mount declarations in source order.
    pub fn read_write(&self) -> &[String] {
        &self.read_write
    }

    /// Returns read-only mount declarations in source order.
    pub fn read_only(&self) -> &[String] {
        &self.read_only
    }

    /// Returns network declarations in source order.
    pub fn network(&self) -> &[String] {
        &self.network
    }
}

/// An MCP server command vector from trusted configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    name: String,
    command: Vec<String>,
    deny: Vec<String>,
}

impl McpServer {
    /// Returns the server name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the executable and arguments without interpreting them.
    pub fn command(&self) -> &[String] {
        &self.command
    }

    /// Returns tool names the host should deny.
    pub fn deny(&self) -> &[String] {
        &self.deny
    }
}

/// How a loadout secret reaches a Session: `[secret.<name>]`.
///
/// Every key has a default, so a secret a loadout names needs no table of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretDefinition {
    name: String,
    env: String,
    green_ink: bool,
    kinds: Vec<String>,
    targets: Vec<String>,
    source: SecretSource,
}

impl SecretDefinition {
    /// The definition every key at its default: the Session variable is the name in upper
    /// case with `-` and `.` as `_`, read on the Station by the `file` provider from the file
    /// `<name>` in its secrets directory.
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let env: String = name
            .chars()
            .map(|c| match c {
                '-' | '.' => '_',
                other => other.to_ascii_uppercase(),
            })
            .collect();
        Self {
            source: SecretSource::new("file", &name),
            name,
            env,
            green_ink: false,
            kinds: Vec::new(),
            targets: Vec::new(),
        }
    }

    /// The secret's name in loadouts.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The environment variable a Session sees it in.
    pub fn env(&self) -> &str {
        &self.env
    }

    /// Whether it is leased only while the Mission holds a live green ink approval.
    pub fn green_ink(&self) -> bool {
        self.green_ink
    }

    /// The approval kinds that unlock a green ink secret; empty means any.
    pub fn kinds(&self) -> &[String] {
        &self.kinds
    }

    /// Globs an approval's target must match to unlock a green ink secret (as a class
    /// authorisation's `target_pattern` matches); empty means any target, a weakening.
    pub fn targets(&self) -> &[String] {
        &self.targets
    }

    /// Whether an approval of `kind` on `target` unlocks this secret.
    pub fn unlocked_by(&self, kind: &str, target: &str) -> bool {
        (self.kinds.is_empty() || self.kinds.iter().any(|listed| listed == kind))
            && (self.targets.is_empty()
                || self.targets.iter().any(|pattern| {
                    globset::Glob::new(pattern)
                        .is_ok_and(|glob| glob.compile_matcher().is_match(target))
                }))
    }

    /// Why this green ink secret is looser than it could be, if it is: any approval of its
    /// kinds unlocks it whatever its target.
    pub fn green_ink_weakening(&self) -> Option<String> {
        (self.green_ink && self.targets.is_empty()).then(|| {
            format!(
                "green ink secret {:?} is unlocked by an approval of any target; set `targets` \
                 to the refs or releases it may act on",
                self.name
            )
        })
    }

    /// Where it is read: a provider or instance, its key there, and that provider's fields.
    pub fn source(&self) -> &SecretSource {
        &self.source
    }
}

/// A configured secret provider: `[secret_provider.<name>]`, a loaded provider and the settings
/// it is built with, so many secrets share one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretInstance {
    name: String,
    provider: String,
    settings: SecretFields,
}

impl SecretInstance {
    /// The instance's name, which secrets give as their `provider`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The loaded provider it is built from.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Its settings, the provider's `config`; a credential among them is a secret reference.
    pub fn settings(&self) -> &SecretFields {
        &self.settings
    }
}

/// Validated loadouts, the named policies and overlays they reference, and the account pools.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadoutCatalog {
    loadouts: BTreeMap<String, Loadout>,
    sandboxes: BTreeMap<String, SandboxProfile>,
    servers: BTreeMap<String, McpServer>,
    overlays: BTreeMap<String, overlays::OverlayRecord>,
    secrets: BTreeMap<String, SecretDefinition>,
    instances: BTreeMap<String, SecretInstance>,
    pools: BTreeMap<String, AccountPool>,
    #[cfg(test)]
    default_provider: Option<String>,
}

impl LoadoutCatalog {
    /// The `[secret_provider.<name>]` instances by name.
    pub fn secret_providers(&self) -> &BTreeMap<String, SecretInstance> {
        &self.instances
    }

    /// The loaded provider a secret's `provider` comes down to: an instance's provider, or the
    /// name itself.
    pub fn provider_of<'a>(&'a self, provider: &'a str) -> &'a str {
        self.instances
            .get(provider)
            .map_or(provider, |instance| instance.provider.as_str())
    }

    /// Every secret that reads with the `file` provider by default reads with `provider`
    /// instead, and so does a name no table defines: for tests, which hand every secret to one
    /// fake provider.
    #[cfg(test)]
    pub(crate) fn with_default_secret_provider(mut self, provider: &str) -> Self {
        let names: Vec<String> = self.secrets().iter().map(|d| d.name.clone()).collect();
        for name in names {
            let mut definition = self.secret(&name);
            if definition.source.provider == "file" {
                definition.source.provider = provider.to_owned();
            }
            self.secrets.insert(name, definition);
        }
        self.default_provider = Some(provider.to_owned());
        self
    }

    /// Adds an account pool, replacing one with the same ID; the way in for pools built in
    /// code. Its accounts' secrets are defined as an unnamed `[secret.<name>]` is.
    ///
    /// # Errors
    ///
    /// A [`QErrorKind::Schema`] error whose [`QError::reason`] names what is wrong.
    pub fn with_pool(mut self, pool: AccountPool) -> Result<Self, QError> {
        pool.validate()
            .and_then(|()| refuse_green_ink(&pool, &self.secrets))
            .map_err(|reason| QError::new(QErrorKind::Schema, None).because(reason))?;
        self.pools.insert(pool.id().to_owned(), pool);
        for loadout in self.loadouts.values() {
            self.check_session_env(&loadout.env, Limit::UNBOUNDED)
                .map_err(|reason| {
                    QError::new(QErrorKind::Schema, None)
                        .because(format!("loadout {:?}: {reason}", loadout.agent))
                })?;
        }
        Ok(self)
    }

    /// Refuses an `env` table a Session could not be given beside this catalog: a name that is a
    /// provider credential's ([`CREDENTIAL_VARIABLES`]), a secret's or a pool account's variable,
    /// or AI6's own. Case is ignored, as Windows ignores it.
    ///
    /// # Errors
    ///
    /// Why, naming the variable and never its value.
    pub fn check_session_env(&self, env: &SessionEnv, chars: Limit) -> Result<(), String> {
        let mut taken: Vec<String> = CREDENTIAL_VARIABLES
            .iter()
            .map(|variable| (*variable).to_owned())
            .collect();
        taken.extend(self.secrets().iter().map(|secret| secret.env().to_owned()));
        for pool in self.pools.values() {
            taken.extend(
                pool.accounts()
                    .filter_map(|account| account.variable(pool.handler())),
            );
        }
        env.check(&taken, chars)
    }

    /// The account pools in ID order.
    pub fn pools(&self) -> impl Iterator<Item = &AccountPool> {
        self.pools.values()
    }

    /// The pool `id`.
    pub fn pool(&self, id: &str) -> Option<&AccountPool> {
        self.pools.get(id)
    }

    /// Reads and validates a UTF-8 TOML loadout file within `limits`.
    pub fn load(path: impl AsRef<Path>, limits: &ConfigLimits) -> Result<Self, QError> {
        let path = path.as_ref();
        let mut file = File::open(path).map_err(|error| {
            QError::new(QErrorKind::Io, Some(path.to_path_buf())).with_source(error)
        })?;
        let mut bytes = Vec::new();
        let most = limits
            .loadout_bytes
            .get()
            .map_or(u64::MAX, |bytes| bytes.saturating_add(1));
        file.by_ref()
            .take(most)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                QError::new(QErrorKind::Io, Some(path.to_path_buf())).with_source(error)
            })?;
        if !limits.loadout_bytes.allows(bytes.len()) {
            return Err(too_large(Some(path.to_path_buf()), limits));
        }
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            QError::new(QErrorKind::InvalidUtf8, Some(path.to_path_buf())).at(
                std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default(),
                error.valid_up_to(),
            )
        })?;
        parse_document(text, Some(path.to_path_buf()), limits)
    }

    /// Parses TOML within `limits`, without reading files, expanding environment or
    /// executing commands.
    ///
    /// # Errors
    ///
    /// A [`QError`] naming why the document is refused.
    pub fn parse(text: &str, limits: &ConfigLimits) -> Result<Self, QError> {
        parse_document(text, None, limits)
    }

    /// Returns the exact Agent loadout or a schema error when none is declared.
    pub fn for_agent(&self, agent: &str) -> Result<&Loadout, QError> {
        self.loadouts
            .get(agent)
            .ok_or_else(|| QError::new(QErrorKind::Schema, None))
    }

    /// Returns the named sandbox profile, or `None` when the loadout has none.
    pub fn sandbox_for(&self, loadout: &Loadout) -> Option<&SandboxProfile> {
        loadout
            .sandbox
            .as_deref()
            .and_then(|name| self.sandboxes.get(name))
    }

    /// Returns an Agent's declared MCP servers in loadout order.
    pub fn servers_for(&self, loadout: &Loadout) -> Result<Vec<&McpServer>, QError> {
        if self.loadouts.get(&loadout.agent) != Some(loadout) {
            return Err(QError::new(QErrorKind::Schema, None));
        }
        loadout
            .mcp
            .iter()
            .map(|name| {
                self.servers
                    .get(name)
                    .ok_or_else(|| QError::new(QErrorKind::Schema, None))
            })
            .collect()
    }

    /// Iterates over Agent loadouts in stable codename order.
    pub fn loadouts(&self) -> impl Iterator<Item = &Loadout> {
        self.loadouts.values()
    }

    /// Iterates over sandbox profiles in stable name order.
    pub fn sandboxes(&self) -> impl Iterator<Item = &SandboxProfile> {
        self.sandboxes.values()
    }

    /// Iterates over MCP server declarations in stable name order.
    pub fn mcp_servers(&self) -> impl Iterator<Item = &McpServer> {
        self.servers.values()
    }

    /// The `[overlay.<name>]` records by identity (the name normalized).
    pub fn overlays(&self) -> &BTreeMap<String, overlays::OverlayRecord> {
        &self.overlays
    }

    /// The overlay `name` names, compared as an identity.
    pub fn overlay(&self, name: &str) -> Option<&overlays::OverlayRecord> {
        self.overlays.get(&overlays::normalize_name(name))
    }

    /// How the secret `name` reaches a Session: its `[secret.<name>]` table, or every default.
    pub fn secret(&self, name: &str) -> SecretDefinition {
        self.secrets.get(name).cloned().unwrap_or_else(|| {
            #[allow(unused_mut)]
            let mut definition = SecretDefinition::new(name);
            #[cfg(test)]
            if let Some(provider) = &self.default_provider {
                definition.source.provider = provider.clone();
            }
            definition
        })
    }

    /// Whether `name` is defined by a `[secret.<name>]` table.
    pub fn knows_secret(&self, name: &str) -> bool {
        self.secrets.contains_key(name)
    }

    /// The definition of every secret any loadout or pool account names, in stable name
    /// order.
    pub fn secrets(&self) -> Vec<SecretDefinition> {
        let names: std::collections::BTreeSet<&str> = self
            .loadouts
            .values()
            .flat_map(|loadout| loadout.secrets.iter().map(String::as_str))
            .chain(self.secrets.keys().map(String::as_str))
            .chain(
                self.pools
                    .values()
                    .flat_map(AccountPool::accounts)
                    .filter_map(Account::secret_name),
            )
            .collect();
        names.into_iter().map(|name| self.secret(name)).collect()
    }
}

impl FromStr for LoadoutCatalog {
    type Err = QError;

    /// Parses TOML within the default [`ConfigLimits`], as [`LoadoutCatalog::parse`].
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        parse_document(text, None, &ConfigLimits::default())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[serde(default)]
    loadout: BTreeMap<String, RawLoadout>,
    #[serde(default)]
    sandbox: BTreeMap<String, RawSandbox>,
    #[serde(default)]
    mcp: BTreeMap<String, RawMcpServer>,
    #[serde(default)]
    overlay: BTreeMap<String, RawOverlay>,
    #[serde(default)]
    secret: BTreeMap<String, RawSecret>,
    #[serde(default)]
    secret_provider: BTreeMap<String, RawSecretProvider>,
    #[serde(default)]
    pool: BTreeMap<String, RawPool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPool {
    handler: String,
    #[serde(default)]
    operations: Vec<String>,
    strategy: Option<String>,
    login_command: Option<Vec<String>>,
    #[serde(default)]
    named_only: bool,
    #[serde(default)]
    account: BTreeMap<String, RawAccount>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAccount {
    kind: String,
    secret: Option<String>,
    dir: Option<String>,
    env: Option<String>,
    #[serde(default)]
    priority: i64,
    max_concurrent: Option<usize>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    renew_command: Vec<String>,
    #[serde(default)]
    renew_env: BTreeMap<String, String>,
}

impl RawPool {
    fn into_pool(self, id: String) -> Result<AccountPool, String> {
        let mut pool = AccountPool::new(&id, self.handler)
            .operations(self.operations)
            .named_only(self.named_only);
        if let Some(strategy) = self.strategy {
            pool = pool.strategy(PoolStrategy::parse(&strategy).ok_or_else(|| {
                format!(
                    "pool {id:?}: strategy {strategy:?} is not headroom, round_robin or priority"
                )
            })?);
        }
        if let Some(command) = self.login_command {
            pool = pool.login_command(command);
        }
        for (name, raw) in self.account {
            let kind = AccountKind::parse(&raw.kind).ok_or_else(|| {
                format!(
                    "pool {id:?} account {name:?}: kind {:?} is unknown",
                    raw.kind
                )
            })?;
            let mut account = Account::new(&name, kind).priority(raw.priority);
            if let Some(secret) = raw.secret {
                account = account.secret(secret);
            }
            if let Some(dir) = raw.dir {
                account = account.dir(PathBuf::from(dir));
            }
            if let Some(env) = raw.env {
                account = account.env(env);
            }
            if let Some(limit) = raw.max_concurrent {
                account = account.max_concurrent(limit);
            }
            for (key, value) in raw.labels {
                account = account.label(key, value);
            }
            account = account.files(raw.files).renew_command(raw.renew_command);
            for (key, value) in raw.renew_env {
                account = account.renew_env(key, value);
            }
            pool = pool.account(account);
        }
        pool.validate()?;
        Ok(pool)
    }
}

/// `[secret.<name>]`: AI6's keys, then the provider, its key and that provider's fields, flat.
#[derive(Deserialize)]
struct RawSecret {
    env: Option<String>,
    #[serde(default)]
    green_ink: bool,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    targets: Vec<String>,
    provider: Option<String>,
    key: Option<String>,
    #[serde(flatten)]
    fields: serde_json::Map<String, serde_json::Value>,
}

/// `[secret_provider.<name>]`: the provider, then its settings, flat.
#[derive(Deserialize)]
struct RawSecretProvider {
    provider: String,
    #[serde(flatten)]
    settings: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLoadout {
    sandbox: Option<String>,
    #[serde(default)]
    mcp: Vec<String>,
    #[serde(default)]
    overlays: Vec<String>,
    #[serde(default)]
    secrets: Vec<String>,
    #[serde(default)]
    env: SessionEnv,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOverlay {
    source: Option<String>,
    path: Option<String>,
    pin: Option<String>,
    #[serde(default)]
    secrets: Vec<String>,
}

impl RawOverlay {
    fn into_record(self, name: &str, chars: Limit) -> Result<overlays::OverlayRecord, String> {
        let key = format!("overlay {name:?}");
        if !overlays::valid_overlay_name(name) {
            return Err(format!(
                "{key}: a name is an ASCII letter, then letters, digits, '_', '.' or '-', ending \
                 in a letter or digit, and no scope label or device name"
            ));
        }
        let from = match (self.source, self.path) {
            (Some(source), None) => overlays::OverlayFrom::Source(
                overlays::Source::parse(&source)
                    .map_err(|error| format!("{key}.source: {error}"))?,
            ),
            (None, Some(path))
                if !path.trim().is_empty()
                    && !path.chars().any(char::is_control)
                    && chars.allows(path.chars().count()) =>
            {
                overlays::OverlayFrom::Path(PathBuf::from(path))
            }
            (None, Some(_)) => {
                return Err(format!(
                    "{key}.path: empty, too long or holding a control character"
                ));
            }
            _ => return Err(format!("{key}: exactly one of source and path")),
        };
        let mut record = overlays::OverlayRecord::new(name, from);
        if let Some(pin) = self.pin {
            record = record.pinned(
                overlays::Digest::parse(&pin).map_err(|reason| format!("{key}.pin: {reason}"))?,
            );
        }
        for secret in &self.secrets {
            validate_name(secret, QErrorKind::Schema, None)
                .map_err(|_| format!("{key}.secrets: {secret:?} is not a secret name"))?;
        }
        Ok(record.with_secrets(self.secrets))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSandbox {
    #[serde(default = "default_backend")]
    backend: String,
    #[serde(default)]
    read_write: Vec<String>,
    #[serde(default)]
    read_only: Vec<String>,
    #[serde(default)]
    network: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMcpServer {
    command: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
}

fn default_backend() -> String {
    "bubblewrap".to_owned()
}

fn too_large(path: Option<PathBuf>, limits: &ConfigLimits) -> QError {
    QError::new(QErrorKind::TooLarge, path).because(format!(
        "over {} bytes (`[provider] loadout_bytes`)",
        limits.loadout_bytes
    ))
}

fn parse_document(
    text: &str,
    path: Option<PathBuf>,
    limits: &ConfigLimits,
) -> Result<LoadoutCatalog, QError> {
    if !limits.loadout_bytes.allows(text.len()) {
        return Err(too_large(path, limits));
    }
    let chars = limits.loadout_value_chars;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let value = toml::from_str::<toml::Value>(text).map_err(|error| {
        let mut failure = QError::new(QErrorKind::Parse, path.clone());
        if let Some(span) = error.span() {
            failure = failure.at(text, span.start);
        }
        failure
    })?;
    let document = value
        .try_into::<Document>()
        .map_err(|error: toml::de::Error| {
            let mut failure = QError::new(QErrorKind::Schema, path.clone());
            if let Some(span) = error.span() {
                failure = failure.at(text, span.start);
            }
            failure
        })?;

    if document.loadout.is_empty()
        && document.pool.is_empty()
        && document.secret.is_empty()
        && document.secret_provider.is_empty()
        && document.overlay.is_empty()
    {
        return Err(QError::new(QErrorKind::Schema, path).because(
            "declare at least one [loadout.<agent>], [pool.<id>], [secret.<name>], \
             [secret_provider.<name>] or [overlay.<name>]",
        ));
    }
    let mut overlay_records = BTreeMap::new();
    for (name, raw) in document.overlay {
        let record = raw
            .into_record(&name, chars)
            .map_err(|reason| QError::new(QErrorKind::Schema, path.clone()).because(reason))?;
        if let Some(other) = overlay_records.insert(record.identity(), record) {
            return Err(QError::new(QErrorKind::Schema, path).because(format!(
                "overlay {name:?} and overlay {:?} are one overlay: names compare in lower case, \
                 '_' and '.' as '-'",
                other.name()
            )));
        }
    }
    let mut pools = BTreeMap::new();
    for (id, raw) in document.pool {
        let pool = raw
            .into_pool(id.clone())
            .map_err(|reason| QError::new(QErrorKind::Schema, path.clone()).because(reason))?;
        pools.insert(id, pool);
    }
    let mut loadouts = BTreeMap::new();
    for (name, raw) in document.loadout {
        validate_name(&name, QErrorKind::Schema, path.clone())?;
        validate_names(&raw.mcp, QErrorKind::Schema, path.clone(), chars)?;
        validate_names(&raw.secrets, QErrorKind::Schema, path.clone(), chars)?;
        for secret in &raw.secrets {
            validate_name(secret, QErrorKind::Schema, path.clone())?;
        }
        if let Some(sandbox) = &raw.sandbox {
            validate_name(sandbox, QErrorKind::Schema, path.clone())?;
        }
        check_overlay_list(&raw.overlays, Some(&overlay_records)).map_err(|reason| {
            QError::new(QErrorKind::Schema, path.clone())
                .because(format!("loadout {name:?}: {reason}"))
        })?;
        loadouts.insert(
            name.clone(),
            Loadout {
                agent: name,
                sandbox: raw.sandbox,
                mcp: raw.mcp,
                overlays: raw.overlays,
                secrets: raw.secrets,
                env: raw.env,
            },
        );
    }

    let mut sandboxes = BTreeMap::new();
    for (name, raw) in document.sandbox {
        validate_name(&name, QErrorKind::Schema, path.clone())?;
        validate_text(&raw.backend, QErrorKind::Schema, path.clone(), chars)?;
        validate_names(&raw.read_write, QErrorKind::Schema, path.clone(), chars)?;
        validate_names(&raw.read_only, QErrorKind::Schema, path.clone(), chars)?;
        validate_names(&raw.network, QErrorKind::Schema, path.clone(), chars)?;
        sandboxes.insert(
            name.clone(),
            SandboxProfile {
                name,
                backend: raw.backend,
                read_write: raw.read_write,
                read_only: raw.read_only,
                network: raw.network,
            },
        );
    }

    let mut servers = BTreeMap::new();
    for (name, raw) in document.mcp {
        validate_name(&name, QErrorKind::Schema, path.clone())?;
        if raw.command.is_empty() {
            return Err(QError::new(QErrorKind::Schema, path));
        }
        for value in raw.command.iter().chain(&raw.deny) {
            validate_text(value, QErrorKind::Schema, path.clone(), chars)?;
        }
        servers.insert(
            name.clone(),
            McpServer {
                name,
                command: raw.command,
                deny: raw.deny,
            },
        );
    }

    let mut instances = BTreeMap::new();
    for (name, raw) in document.secret_provider {
        let refused = |reason: String| {
            QError::new(QErrorKind::Schema, path.clone())
                .because(format!("secret_provider {name:?}: {reason}"))
        };
        validate_name(&name, QErrorKind::Schema, path.clone())?;
        if secret_kind::BUILT_IN.contains(&name.as_str()) || name == secret_kind::REGISTRY_PROVIDER
        {
            return Err(refused(
                "an instance may not take a built-in provider's name".to_owned(),
            ));
        }
        let settings = SecretFields::new(raw.settings);
        if let Some(reason) = secret_kind::builtin_settings_check(&raw.provider, &settings) {
            return Err(refused(reason));
        }
        instances.insert(
            name.clone(),
            SecretInstance {
                name,
                provider: raw.provider,
                settings,
            },
        );
    }
    for instance in instances.values() {
        if instances.contains_key(&instance.provider) || instance.provider.trim().is_empty() {
            return Err(QError::new(QErrorKind::Schema, path).because(format!(
                "secret_provider {:?}: `provider` names a loaded provider, not another instance",
                instance.name
            )));
        }
    }

    let mut secrets = BTreeMap::new();
    for (name, raw) in document.secret {
        validate_name(&name, QErrorKind::Schema, path.clone())?;
        let mut definition = SecretDefinition::new(&name);
        if let Some(env) = raw.env {
            definition.env = env;
        }
        if !crate::directorates::q::transport::kit::is_variable_name(&definition.env) {
            return Err(QError::new(QErrorKind::Schema, path));
        }
        let refused = |reason: String| {
            QError::new(QErrorKind::Schema, path.clone())
                .because(format!("secret {name:?}: {reason}"))
        };
        if let Some(provider) = raw.provider {
            if provider.trim().is_empty() {
                return Err(refused("`provider` is empty".to_owned()));
            }
            definition.source.provider = provider;
        }
        if let Some(key) = raw.key {
            definition.source.key = key;
        }
        if let Some(reserved) = raw
            .fields
            .keys()
            .find(|field| secret_kind::RESERVED_FIELDS.contains(&field.as_str()))
        {
            return Err(refused(format!(
                "`{reserved}` is not a field of its provider"
            )));
        }
        definition.source.fields = SecretFields::new(raw.fields);
        let provider = instances
            .get(&definition.source.provider)
            .map_or(definition.source.provider.as_str(), |instance| {
                instance.provider.as_str()
            });
        if let Some(reason) = secret_kind::builtin_fields_check(provider, &definition.source.fields)
        {
            return Err(refused(reason));
        }
        let key = &definition.source.key;
        match provider {
            "env" if !crate::directorates::q::transport::kit::is_variable_name(key) => {
                return Err(refused(
                    "`key` of an env secret is an environment variable's name".to_owned(),
                ));
            }
            "file" if key.trim().is_empty() => {
                return Err(refused("`key` of a file secret is its path".to_owned()));
            }
            _ => {}
        }
        validate_names(&raw.kinds, QErrorKind::Schema, path.clone(), chars)?;
        if (!raw.kinds.is_empty() || !raw.targets.is_empty()) && !raw.green_ink {
            return Err(QError::new(QErrorKind::Schema, path));
        }
        if raw
            .targets
            .iter()
            .any(|pattern| pattern.trim().is_empty() || globset::Glob::new(pattern).is_err())
        {
            return Err(QError::new(QErrorKind::Schema, path));
        }
        definition.green_ink = raw.green_ink;
        definition.kinds = raw.kinds;
        definition.targets = raw.targets;
        secrets.insert(name, definition);
    }

    for loadout in loadouts.values() {
        if loadout
            .sandbox
            .as_ref()
            .is_some_and(|name| !sandboxes.contains_key(name))
            || loadout.mcp.iter().any(|name| !servers.contains_key(name))
        {
            return Err(QError::new(QErrorKind::Schema, path));
        }
    }

    for pool in pools.values() {
        refuse_green_ink(pool, &secrets)
            .map_err(|reason| QError::new(QErrorKind::Schema, path.clone()).because(reason))?;
    }

    let catalog = LoadoutCatalog {
        loadouts,
        sandboxes,
        servers,
        overlays: overlay_records,
        secrets,
        instances,
        pools,
        #[cfg(test)]
        default_provider: None,
    };
    for loadout in catalog.loadouts.values() {
        catalog
            .check_session_env(&loadout.env, chars)
            .map_err(|reason| {
                QError::new(QErrorKind::Schema, path.clone())
                    .because(format!("loadout {:?}: {reason}", loadout.agent))
            })?;
    }
    Ok(catalog)
}

/// Why a list of overlays cannot be issued: a name that is no overlay identity, one named
/// twice (as identities compare), or, with `records`, one no `[overlay.<name>]` defines.
pub(crate) fn check_overlay_list(
    names: &[String],
    records: Option<&BTreeMap<String, overlays::OverlayRecord>>,
) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for name in names {
        if !overlays::valid_overlay_name(name) {
            return Err(format!("overlays: {name:?} is no overlay name"));
        }
        let identity = overlays::normalize_name(name);
        if !seen.insert(identity.clone()) {
            return Err(format!("overlays: {name:?} is named twice"));
        }
        if records.is_some_and(|records| !records.contains_key(&identity)) {
            return Err(format!(
                "overlays: {name:?} is not defined by an [overlay.<name>]"
            ));
        }
    }
    Ok(())
}

/// Refuses a pool account whose secret is a green ink one: a pool account is handed to every
/// Session that draws it, while a green ink credential is only for a step a person approved.
fn refuse_green_ink(
    pool: &AccountPool,
    secrets: &BTreeMap<String, SecretDefinition>,
) -> Result<(), String> {
    for account in pool.accounts() {
        if let Some(name) = account.secret_name()
            && secrets.get(name).is_some_and(SecretDefinition::green_ink)
        {
            return Err(format!(
                "pool {:?} account {:?}: secret {name:?} is a green ink secret, which is only for \
                 approved steps; give the account a secret of its own",
                pool.id(),
                account.name()
            ));
        }
    }
    Ok(())
}

fn validate_names(
    values: &[String],
    kind: QErrorKind,
    path: Option<PathBuf>,
    chars: Limit,
) -> Result<(), QError> {
    let mut unique = std::collections::BTreeSet::new();
    for value in values {
        validate_text(value, kind, path.clone(), chars)?;
        if !unique.insert(value) {
            return Err(QError::new(kind, path));
        }
    }
    Ok(())
}

fn validate_name(value: &str, kind: QErrorKind, path: Option<PathBuf>) -> Result<(), QError> {
    let mut bytes = value.bytes();
    let valid = value.len() <= 64
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(QError::new(kind, path))
    }
}

fn validate_text(
    value: &str,
    kind: QErrorKind,
    path: Option<PathBuf>,
    chars: Limit,
) -> Result<(), QError> {
    if !value.is_empty()
        && chars.allows(value.chars().count())
        && !value.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(QError::new(kind, path))
    }
}

#[cfg(test)]
mod tests {
    use super::{Account, AccountKind, AccountPool, BTreeMap, PathBuf, SessionEnv};
    use super::{ConfigLimits, Limit, LoadoutCatalog, QErrorKind};
    use std::str::FromStr;

    const GOOD: &str = r#"
[loadout.007]
sandbox = "worktree-rw"
mcp = ["filesystem", "shell"]
secrets = ["github-pr"]

[sandbox.worktree-rw]
read_write = ["$WORKTREE"]
read_only = ["/usr", "/etc/ssl"]
network = ["api.github.com:443"]

[mcp.filesystem]
command = ["mcp-server-filesystem", "$WORKTREE"]

[mcp.shell]
command = ["mcp-server-shell", "--flag", "same", "--flag", "same"]
deny = ["git push --force"]
"#;

    #[test]
    fn a_loadout_s_env_is_config_a_session_is_given() {
        let catalog: LoadoutCatalog =
            "[loadout.007]\nenv = { PATH = \"/opt/tools/bin\", TOOL_MODE = \"\" }\n"
                .parse()
                .unwrap();
        let env = catalog.for_agent("007").unwrap().env();
        assert_eq!(env.values()["PATH"], "/opt/tools/bin");
        assert_eq!(env.values()["TOOL_MODE"], "", "an empty value is a value");
    }

    #[test]
    fn a_loadout_s_env_may_not_take_a_credential_s_variable() {
        let secret = "[secret.forge]\nenv = \"GH_TOKEN\"\n";
        let pool = "[pool.team]\nhandler = \"claude\"\n[pool.team.account.a]\nkind = \"config-dir\"\ndir = \"/x\"\nenv = \"TEAM_LOGIN\"\n";
        for (document, name) in [
            (
                "env = { ANTHROPIC_API_KEY = \"planted\" }\n".to_owned(),
                "ANTHROPIC_API_KEY",
            ),
            (
                "env = { anthropic_api_key = \"planted\" }\n".to_owned(),
                "anthropic_api_key",
            ),
            (
                "env = { CODEX_HOME = \"planted\" }\n".to_owned(),
                "CODEX_HOME",
            ),
            (
                "secrets = [\"forge\"]\nenv = { GH_TOKEN = \"planted\" }\n".to_owned() + secret,
                "GH_TOKEN",
            ),
            // A secret no loadout names yet is still a secret's variable.
            (
                "env = { GH_TOKEN = \"planted\" }\n".to_owned() + secret,
                "GH_TOKEN",
            ),
            // Named, with its variable left to the default.
            (
                "secrets = [\"forge-token\"]\nenv = { FORGE_TOKEN = \"planted\" }\n".to_owned(),
                "FORGE_TOKEN",
            ),
            (
                "env = { TEAM_LOGIN = \"planted\" }\n".to_owned() + pool,
                "TEAM_LOGIN",
            ),
            (
                "env = { AI6_GREEN_INK_GATE = \"planted\" }\n".to_owned(),
                "AI6_GREEN_INK_GATE",
            ),
            (
                "env = { AI6_ANYTHING = \"planted\" }\n".to_owned(),
                "AI6_ANYTHING",
            ),
            (
                "env = { \"NOT A NAME\" = \"planted\" }\n".to_owned(),
                "NOT A NAME",
            ),
            (
                "env = { BAD_VALUE = \"line\\nbreak planted\" }\n".to_owned(),
                "BAD_VALUE",
            ),
        ] {
            let text = format!("[loadout.007]\n{document}");
            let refused = text.parse::<LoadoutCatalog>().unwrap_err();
            assert_eq!(refused.kind(), QErrorKind::Schema, "{text}");
            let reason = refused.reason().unwrap_or_default().to_owned();
            assert!(reason.contains(name), "{text}: {reason}");
            assert!(reason.contains("007"), "{text}: {reason}");
            assert!(
                !reason.contains("planted"),
                "a value is not repeated: {reason}"
            );
        }
        let limits = ConfigLimits {
            loadout_value_chars: Limit::at(4),
            ..ConfigLimits::default()
        };
        let long = "[loadout.007]\nenv = { PATH = \"/opt/tools\" }\n";
        assert!(LoadoutCatalog::parse(long, &limits).is_err());
    }

    #[test]
    fn a_pool_added_in_code_is_checked_against_a_loadout_s_env() {
        let catalog: LoadoutCatalog = "[loadout.007]\nenv = { TEAM_LOGIN = \"x\" }\n"
            .parse()
            .unwrap();
        let pool = AccountPool::new("team", "claude").account(
            Account::new("a", AccountKind::ConfigDir)
                .dir(PathBuf::from("/x"))
                .env("TEAM_LOGIN".to_owned()),
        );
        let refused = catalog.with_pool(pool).unwrap_err();
        assert!(
            refused.reason().unwrap_or_default().contains("TEAM_LOGIN"),
            "{refused}"
        );
    }

    #[test]
    fn the_env_type_reads_and_writes_as_a_table_for_any_record_that_holds_one() {
        use crate::guatiao::{FromValue, ToValue};
        let env = SessionEnv::from(BTreeMap::from([("PATH".to_owned(), "/bin".to_owned())]));
        assert_eq!(
            SessionEnv::from_value(&env.to_value().unwrap()).unwrap(),
            env
        );
        let by_serde: SessionEnv = toml::from_str::<toml::Table>("PATH = \"/bin\"\n")
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(by_serde, env);
    }

    #[test]
    fn loadout_references_resolve_and_keep_declared_server_order() {
        let catalog = LoadoutCatalog::from_str(GOOD).unwrap();
        let loadout = catalog.for_agent("007").unwrap();

        assert_eq!(loadout.sandbox_name(), Some("worktree-rw"));
        assert_eq!(loadout.mcp_names(), ["filesystem", "shell"]);
        assert_eq!(loadout.secret_names(), ["github-pr"]);
        assert_eq!(
            catalog.sandbox_for(loadout).unwrap().read_write(),
            ["$WORKTREE"]
        );
        let servers = catalog.servers_for(loadout).unwrap();
        assert_eq!(
            servers
                .iter()
                .map(|server| server.name())
                .collect::<Vec<_>>(),
            ["filesystem", "shell"]
        );
        assert_eq!(
            servers[1].command(),
            ["mcp-server-shell", "--flag", "same", "--flag", "same"]
        );
        assert_eq!(servers[1].deny(), ["git push --force"]);
    }

    #[test]
    fn unknown_fields_and_references_fail_schema_validation() {
        for input in [
            "[loadout.007]\nunknown = 'x'",
            "[loadout.007]\nmcp = ['missing']",
            "[loadout.007]\nsandbox = 'missing'",
            "[loadout.007]\nmcp = ['x', 'x']",
            "[loadout.007]\nsecrets = ['bad name']",
            "[unknown]\nvalue = 1",
        ] {
            let error = LoadoutCatalog::from_str(input).unwrap_err();
            assert_eq!(error.kind(), QErrorKind::Schema);
        }
    }

    #[test]
    fn malformed_toml_reports_position_without_echoing_input() {
        let input = "[loadout.007]\nsecret = \"TOP-SECRET\n";
        let error = LoadoutCatalog::from_str(input).unwrap_err();

        assert_eq!(error.kind(), QErrorKind::Parse);
        assert!(error.line().is_some());
        assert!(error.column().is_some());
        assert!(!error.to_string().contains("TOP-SECRET"));
        assert!(!format!("{error:?}").contains("TOP-SECRET"));
    }

    #[test]
    fn duplicate_table_keys_are_parse_errors_and_bom_is_accepted() {
        let duplicate = "[loadout.007]\nsandbox = 'one'\nsandbox = 'two'";
        assert_eq!(
            LoadoutCatalog::from_str(duplicate).unwrap_err().kind(),
            QErrorKind::Parse
        );
        assert!(LoadoutCatalog::from_str(&format!("\u{feff}{GOOD}")).is_ok());
    }

    #[test]
    fn loadout_document_size_is_bounded() {
        let input = format!("{GOOD}#{}", " ".repeat(1024 * 1024));
        assert_eq!(
            LoadoutCatalog::from_str(&input).unwrap_err().kind(),
            QErrorKind::TooLarge
        );
        let roomy = ConfigLimits {
            loadout_bytes: Limit::UNBOUNDED,
            ..ConfigLimits::default()
        };
        assert!(LoadoutCatalog::parse(&input, &roomy).is_ok());
        assert!(LoadoutCatalog::parse(GOOD, &ConfigLimits::default()).is_ok());
        let tight = ConfigLimits {
            loadout_value_chars: Limit::at(3),
            ..ConfigLimits::default()
        };
        assert_eq!(
            LoadoutCatalog::parse(GOOD, &tight).unwrap_err().kind(),
            QErrorKind::Schema
        );
    }

    #[test]
    fn secrets_default_their_variable_and_source_and_green_ink_needs_its_flag() {
        let catalog = LoadoutCatalog::from_str(
            r#"
[loadout.007]
secrets = ["forge-token", "release"]

[secret.release]
env = "GH_TOKEN"
green_ink = true
kinds = ["release", "push"]
provider = "keys"
key = "release"

[secret_provider.keys]
provider = "file"
dir = "/etc/ai6/secrets"
"#,
        )
        .unwrap();
        let forge = catalog.secret("forge-token");
        assert_eq!(forge.env(), "FORGE_TOKEN");
        assert_eq!(
            forge.source(),
            &crate::directorates::q::SecretSource::new("file", "forge-token")
        );
        assert!(!forge.green_ink() && forge.kinds().is_empty());
        let release = catalog.secret("release");
        assert_eq!(
            release.source(),
            &crate::directorates::q::SecretSource::new("keys", "release")
        );
        assert_eq!(catalog.provider_of("keys"), "file");
        assert_eq!(catalog.provider_of("file"), "file");
        assert_eq!(release.env(), "GH_TOKEN");
        assert!(release.green_ink());
        assert_eq!(release.kinds(), ["release", "push"]);
        assert_eq!(
            catalog
                .secrets()
                .iter()
                .map(|definition| definition.name())
                .collect::<Vec<_>>(),
            ["forge-token", "release"]
        );
        for bad in [
            "[loadout.007]\n[secret.x]\nenv = \"not a name\"\n",
            "[loadout.007]\n[secret.x]\nkinds = [\"push\"]\n",
            "[loadout.007]\n[secret.x]\nprovider = \"env\"\nkey = \"\"\n",
            "[loadout.007]\n[secret.x]\nprovider = \"env\"\nkey = \"1x\"\n",
            "[loadout.007]\n[secret.x]\nfrom = { file = \"x\" }\n",
            "[loadout.007]\n[secret.x]\nvalue = \"inline\"\n",
            "[loadout.007]\n[secret.x]\nprovider = \"inline\"\n",
            "[loadout.007]\n[secret.x]\nprovider = \"inline\"\nvalue = \"v\"\nother = 1\n",
            "[loadout.007]\n[secret.x]\nprovider = \"\"\n",
            "[loadout.007]\n[secret.x]\nsecret = \"y\"\n",
            "[secret_provider.file]\nprovider = \"file\"\n",
            "[secret_provider.a]\nprovider = \"b\"\n[secret_provider.b]\nprovider = \"file\"\n",
            "[secret_provider.a]\nprovider = \"file\"\nroot = \"/x\"\n",
            "[secret_provider.a]\nprovider = \"env\"\ndir = \"/x\"\n",
            "[secret_provider.a]\nprovider = \"file\"\n[secret.x]\nprovider = \"a\"\nmount = 1\n",
            "[loadout.007]\n[secret.x]\ntargets = [\"origin:*\"]\n",
            "[loadout.007]\n[secret.x]\ngreen_ink = true\ntargets = [\"refs/[\"]\n",
        ] {
            assert_eq!(
                LoadoutCatalog::from_str(bad).unwrap_err().kind(),
                QErrorKind::Schema,
                "{bad}"
            );
        }
    }

    #[test]
    fn a_green_ink_secret_is_unlocked_only_by_its_kinds_and_targets() {
        let catalog = LoadoutCatalog::from_str(
            r#"
[loadout.007]
secrets = ["release", "anything"]

[secret.release]
green_ink = true
kinds = ["release"]
targets = ["tag v1.*"]

[secret.anything]
green_ink = true
"#,
        )
        .unwrap();
        let release = catalog.secret("release");
        assert_eq!(release.targets(), ["tag v1.*"]);
        assert!(release.unlocked_by("release", "tag v1.4.0"));
        assert!(
            !release.unlocked_by("release", "tag v2.0.0"),
            "another target"
        );
        assert!(!release.unlocked_by("push", "tag v1.4.0"), "another kind");
        assert!(release.green_ink_weakening().is_none());
        let anything = catalog.secret("anything");
        assert!(anything.unlocked_by("push", "origin:refs/heads/main"));
        assert!(
            anything
                .green_ink_weakening()
                .unwrap()
                .contains("any target")
        );
    }

    #[test]
    fn pools_load_with_their_accounts_and_secrets_and_errors_name_the_setting() {
        let catalog = LoadoutCatalog::from_str(
            r#"
[pool.claude-team]
handler = "claude"
operations = ["ai6-*"]
strategy = "round_robin"

[pool.claude-team.account.alice]
kind = "claude-oauth-token"
secret = "alice-token"
priority = 2
max_concurrent = 1
labels = { plan = "max" }

[pool.claude-team.account.bob]
kind = "config-dir"
dir = "/logins/bob"

[pool.codex-team]
handler = "codex"
login_command = ["/opt/codex/bin/codex"]

[pool.codex-team.account.carol]
kind = "openai-api-key"
"#,
        )
        .unwrap();
        let team = catalog.pool("claude-team").unwrap();
        assert_eq!(team.handler(), "claude");
        assert_eq!(team.operation_globs(), ["ai6-*"]);
        assert_eq!(team.balancing(), super::PoolStrategy::RoundRobin);
        let alice = team.get("alice").unwrap();
        assert_eq!(alice.kind(), super::AccountKind::ClaudeOauthToken);
        assert_eq!(alice.secret_name(), Some("alice-token"));
        assert_eq!((alice.priority_value(), alice.concurrency()), (2, Some(1)));
        assert_eq!(alice.labels()["plan"], "max");
        assert_eq!(
            team.get("bob").unwrap().login_dir(),
            Some(std::path::Path::new("/logins/bob"))
        );
        assert_eq!(
            catalog.pool("codex-team").unwrap().login(),
            ["/opt/codex/bin/codex"]
        );
        // Account secrets are served like loadout secrets.
        let names: Vec<_> = catalog
            .secrets()
            .iter()
            .map(|definition| definition.name().to_owned())
            .collect();
        assert_eq!(names, ["alice-token", "carol"]);

        for (bad, expected) in [
            ("[pool.p]\nhandler = \"claude\"\n", "has no accounts"),
            (
                "[pool.p]\nhandler = \"claude\"\n[pool.p.account.a]\nkind = \"password\"\n",
                "kind \"password\" is unknown",
            ),
            (
                "[pool.p]\nhandler = \"claude\"\nstrategy = \"random\"\n[pool.p.account.a]\nkind = \"anthropic-api-key\"\n",
                "strategy \"random\"",
            ),
        ] {
            let error = LoadoutCatalog::from_str(bad).unwrap_err();
            assert_eq!(error.kind(), QErrorKind::Schema);
            assert!(error.to_string().contains(expected), "{error}");
        }
        assert!(
            LoadoutCatalog::from_str("[pool.p]\nhandler = \"claude\"\nvalue = 1\n").is_err(),
            "unknown keys are refused"
        );
        let green = LoadoutCatalog::from_str(
            "[pool.p]\nhandler = \"claude\"\n[pool.p.account.a]\nkind = \"anthropic-api-key\"\n\
             secret = \"release\"\n[secret.release]\ngreen_ink = true\n",
        )
        .unwrap_err();
        assert!(
            green.to_string().contains("is a green ink secret"),
            "{green}"
        );
        let catalog =
            LoadoutCatalog::from_str("[loadout.007]\n[secret.release]\ngreen_ink = true\n")
                .unwrap();
        assert!(
            catalog
                .with_pool(super::AccountPool::new("p", "claude").account(
                    super::Account::new("a", super::AccountKind::AnthropicApiKey).secret("release")
                ))
                .unwrap_err()
                .reason()
                .unwrap()
                .contains("green ink"),
            "built in code too"
        );
        assert!(
            LoadoutCatalog::default()
                .with_pool(super::AccountPool::new("p", "claude"))
                .unwrap_err()
                .reason()
                .unwrap()
                .contains("has no accounts")
        );
    }

    #[test]
    fn loadout_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LoadoutCatalog>();
    }
}
