//! Q Branch execution resources: validated loadouts, secret leases and terminals.

mod mcp;
mod sandbox;
mod secrets;

pub mod terminal;

pub use mcp::claude_mcp_config;
pub use sandbox::{SandboxError, SandboxRequest};
pub use secrets::{
    DEFAULT_SECRET_LEASE_TTL, MAX_SECRET_LEASE_TTL, SecretBackend, SecretBackendFailure,
    SecretBroker, SecretError, SecretLease, SecretLeaseRequest, SecretRequest, SecretValue,
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
            source: None,
        }
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

/// Validated loadouts and the named policies they reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadoutCatalog {
    loadouts: BTreeMap<String, Loadout>,
    sandboxes: BTreeMap<String, SandboxProfile>,
    servers: BTreeMap<String, McpServer>,
}

impl LoadoutCatalog {
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

    if document.loadout.is_empty() {
        return Err(QError::new(QErrorKind::Schema, path));
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

    Ok(LoadoutCatalog {
        loadouts,
        sandboxes,
        servers,
    })
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
    fn loadout_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LoadoutCatalog>();
    }
}
