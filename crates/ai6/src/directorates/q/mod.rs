//! Q Branch execution resources: validated loadouts, secret leases and terminals.

mod mcp;
mod pools;
mod sandbox;
mod secrets;

pub mod ephemeral;
pub mod provision;
pub mod safehouse;
pub mod terminal;
pub mod transport;

pub use mcp::claude_mcp_config;
pub use pools::{
    Account, AccountKind, AccountLogin, AccountPool, CREDENTIAL_VARIABLES, CommandLogin,
    PoolChoice, PoolStrategy, choose_pools,
};
pub use sandbox::{SandboxError, SandboxRequest};
pub use secrets::{
    DEFAULT_SECRET_LEASE_TTL, MAX_SECRET_LEASE_TTL, SecretBackend, SecretBackendFailure,
    SecretBroker, SecretError, SecretLease, SecretLeaseRequest, SecretRequest, SecretValue,
    StationSecrets,
};

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

/// Maximum UTF-8 size of a Q Branch loadout document.
pub const MAX_LOADOUT_BYTES: usize = 1024 * 1024;
const MAX_VALUE_CHARS: usize = 4096;

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
            QErrorKind::TooLarge => {
                write!(f, "Q Branch loadout exceeds {MAX_LOADOUT_BYTES} bytes")?
            }
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

/// One Agent's named sandbox, MCP servers and secret references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loadout {
    agent: String,
    sandbox: Option<String>,
    mcp: Vec<String>,
    secrets: Vec<String>,
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

    /// Returns secret reference names, never their values.
    pub fn secret_names(&self) -> &[String] {
        &self.secrets
    }

    /// The loadout that declares only a pool account's secret, so a broker leases it and
    /// nothing else; `None` for an account that reads no secret.
    pub fn for_account(pool: &AccountPool, account: &Account) -> Option<Self> {
        Some(Self {
            agent: format!("pool:{}/{}", pool.id(), account.name()),
            sandbox: None,
            mcp: Vec::new(),
            secrets: vec![account.secret_name()?.to_owned()],
        })
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

/// Where the Station's own backend ([`StationSecrets`]) reads a secret's value.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    /// An environment variable of the Station's process. Weaker than a file: every process
    /// the Station starts on its own host, a harness among them, inherits it.
    Env(String),
    /// A file on the Station, relative to its configuration directory unless absolute; one
    /// trailing newline is dropped.
    File(PathBuf),
}

impl SecretSource {
    /// Why this source is weaker than the default, if it is.
    pub fn weakening(&self) -> Option<String> {
        match self {
            Self::Env(variable) => Some(format!(
                "read from the Station's environment variable {variable}, which harnesses on the \
                 Station's own host inherit"
            )),
            Self::File(_) => None,
        }
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
    /// case with `-` and `.` as `_`, read on the Station from the file `secrets/<name>` in its
    /// configuration directory.
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
            source: SecretSource::File(PathBuf::from("secrets").join(&name)),
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

    /// Where [`StationSecrets`] reads it.
    pub fn source(&self) -> &SecretSource {
        &self.source
    }
}

/// Validated loadouts, the named policies they reference, and the account pools.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadoutCatalog {
    loadouts: BTreeMap<String, Loadout>,
    sandboxes: BTreeMap<String, SandboxProfile>,
    servers: BTreeMap<String, McpServer>,
    secrets: BTreeMap<String, SecretDefinition>,
    pools: BTreeMap<String, AccountPool>,
}

impl LoadoutCatalog {
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
        Ok(self)
    }

    /// The account pools in ID order.
    pub fn pools(&self) -> impl Iterator<Item = &AccountPool> {
        self.pools.values()
    }

    /// The pool `id`.
    pub fn pool(&self, id: &str) -> Option<&AccountPool> {
        self.pools.get(id)
    }

    /// Reads and validates a UTF-8 TOML loadout file, bounded to one MiB.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, QError> {
        let path = path.as_ref();
        let mut file = File::open(path).map_err(|error| {
            QError::new(QErrorKind::Io, Some(path.to_path_buf())).with_source(error)
        })?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take((MAX_LOADOUT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                QError::new(QErrorKind::Io, Some(path.to_path_buf())).with_source(error)
            })?;
        if bytes.len() > MAX_LOADOUT_BYTES {
            return Err(QError::new(QErrorKind::TooLarge, Some(path.to_path_buf())));
        }
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            QError::new(QErrorKind::InvalidUtf8, Some(path.to_path_buf())).at(
                std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default(),
                error.valid_up_to(),
            )
        })?;
        parse_document(text, Some(path.to_path_buf()))
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

    /// How the secret `name` reaches a Session: its `[secret.<name>]` table, or every default.
    pub fn secret(&self, name: &str) -> SecretDefinition {
        self.secrets
            .get(name)
            .cloned()
            .unwrap_or_else(|| SecretDefinition::new(name))
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

    /// Parses TOML without reading files, expanding environment or executing commands.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        parse_document(text, None)
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
    secret: BTreeMap<String, RawSecret>,
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
}

impl RawPool {
    fn into_pool(self, id: String) -> Result<AccountPool, String> {
        let mut pool = AccountPool::new(&id, self.handler).operations(self.operations);
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
            pool = pool.account(account);
        }
        pool.validate()?;
        Ok(pool)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSecret {
    env: Option<String>,
    #[serde(default)]
    green_ink: bool,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    targets: Vec<String>,
    from: Option<RawSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum RawSource {
    Env(String),
    File(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLoadout {
    sandbox: Option<String>,
    #[serde(default)]
    mcp: Vec<String>,
    #[serde(default)]
    secrets: Vec<String>,
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

fn parse_document(text: &str, path: Option<PathBuf>) -> Result<LoadoutCatalog, QError> {
    if text.len() > MAX_LOADOUT_BYTES {
        return Err(QError::new(QErrorKind::TooLarge, path));
    }
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

    if document.loadout.is_empty() && document.pool.is_empty() {
        return Err(QError::new(QErrorKind::Schema, path)
            .because("declare at least one [loadout.<agent>] or [pool.<id>]"));
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
        validate_names(&raw.mcp, QErrorKind::Schema, path.clone())?;
        validate_names(&raw.secrets, QErrorKind::Schema, path.clone())?;
        for secret in &raw.secrets {
            validate_name(secret, QErrorKind::Schema, path.clone())?;
        }
        if let Some(sandbox) = &raw.sandbox {
            validate_name(sandbox, QErrorKind::Schema, path.clone())?;
        }
        loadouts.insert(
            name.clone(),
            Loadout {
                agent: name,
                sandbox: raw.sandbox,
                mcp: raw.mcp,
                secrets: raw.secrets,
            },
        );
    }

    let mut sandboxes = BTreeMap::new();
    for (name, raw) in document.sandbox {
        validate_name(&name, QErrorKind::Schema, path.clone())?;
        validate_text(&raw.backend, QErrorKind::Schema, path.clone())?;
        validate_names(&raw.read_write, QErrorKind::Schema, path.clone())?;
        validate_names(&raw.read_only, QErrorKind::Schema, path.clone())?;
        validate_names(&raw.network, QErrorKind::Schema, path.clone())?;
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
            validate_text(value, QErrorKind::Schema, path.clone())?;
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
        definition.source = match raw.from {
            Some(RawSource::Env(variable))
                if crate::directorates::q::transport::kit::is_variable_name(&variable) =>
            {
                SecretSource::Env(variable)
            }
            Some(RawSource::File(file)) if !file.trim().is_empty() => {
                SecretSource::File(PathBuf::from(file))
            }
            Some(_) => return Err(QError::new(QErrorKind::Schema, path)),
            None => definition.source,
        };
        validate_names(&raw.kinds, QErrorKind::Schema, path.clone())?;
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

    Ok(LoadoutCatalog {
        loadouts,
        sandboxes,
        servers,
        secrets,
        pools,
    })
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
) -> Result<(), QError> {
    let mut unique = std::collections::BTreeSet::new();
    for value in values {
        validate_text(value, kind, path.clone())?;
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

fn validate_text(value: &str, kind: QErrorKind, path: Option<PathBuf>) -> Result<(), QError> {
    if !value.is_empty()
        && value.chars().count() <= MAX_VALUE_CHARS
        && !value.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(QError::new(kind, path))
    }
}

#[cfg(test)]
mod tests {
    use super::{LoadoutCatalog, MAX_LOADOUT_BYTES, QErrorKind};
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
        let input = format!("#{}", " ".repeat(MAX_LOADOUT_BYTES));
        assert_eq!(
            LoadoutCatalog::from_str(&input).unwrap_err().kind(),
            QErrorKind::TooLarge
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
from = { file = "/etc/ai6/secrets/release" }
"#,
        )
        .unwrap();
        let forge = catalog.secret("forge-token");
        assert_eq!(forge.env(), "FORGE_TOKEN");
        assert_eq!(
            forge.source(),
            &super::SecretSource::File(std::path::PathBuf::from("secrets").join("forge-token"))
        );
        assert!(forge.source().weakening().is_none());
        assert!(
            super::SecretSource::Env("X".to_owned())
                .weakening()
                .unwrap()
                .contains("inherit")
        );
        assert!(!forge.green_ink() && forge.kinds().is_empty());
        let release = catalog.secret("release");
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
            "[loadout.007]\n[secret.x]\nfrom = { env = \"\" }\n",
            "[loadout.007]\n[secret.x]\nfrom = { vault = \"x\" }\n",
            "[loadout.007]\n[secret.x]\nvalue = \"inline\"\n",
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
