//! The bootstrap file: what a process needs before it can reach its configuration provider.
//!
//! A Station and the `ai6` command read `ai6.cfg` ([`Bootstrap`]); Universal Exports reads
//! `unex.cfg` with the same loader and sections. A file holds only what starting needs:
//! where the configuration provider and the Registry are, the passwords to reach them,
//! this host's state, identity and secrets directories, its listener, and how it checks the
//! provider libraries it loads. Everything else is a record in the configuration provider.
//!
//! The file is found by [`SearchPath`]: a path given on the command line, else the
//! environment variable naming one, else the user's then the system's configuration
//! directory. The first file found is the only one read; files are never merged, and with
//! none every key takes its default. The working directory is never searched.
//!
//! The format follows the extension ([`format`](mod@format)). Unknown keys are refused. A
//! relative path in the file is relative to the file's directory. Secrets are [`SecretRef`]s,
//! never values the file must hold.

pub mod format;

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer};
use zeroize::Zeroizing;

use crate::bounds::Limit;
use crate::bounds::Seconds;
use crate::directorates::m::config::ConfigLimits;
use crate::directorates::registry::bona_fides::PluginProfile;
use crate::directorates::registry::limits::RegistryLimits;

use self::format::Format;

/// The bootstrap file of a Station and the `ai6` command.
pub const FILE_NAME: &str = "ai6.cfg";
/// The environment variable naming the bootstrap file of a Station and the `ai6` command.
pub const CONFIG_ENV: &str = "AI6_CONFIG";
/// The listener a Station uses when its bootstrap file names none.
pub const DEFAULT_LISTEN: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    std::net::Ipv4Addr::LOCALHOST,
    7081,
));
/// How often a Station checks its configuration provider for changes when the file says
/// nothing.
pub const DEFAULT_POLL: Duration = Duration::from_secs(5);
/// The bootstrap file of Universal Exports (`universal-exports`).
pub const UNEX_FILE_NAME: &str = "unex.cfg";
/// The environment variable naming Universal Exports' bootstrap file.
pub const UNEX_CONFIG_ENV: &str = "UNEX_CONFIG";
/// The fewest bytes a reverse-proxy secret holds unless `unex.cfg` says otherwise.
pub const DEFAULT_PROXY_SECRET_MIN_BYTES: u64 = 32;
/// The loopback listener Universal Exports uses when its bootstrap file names none.
pub const DEFAULT_UNEX_LISTEN: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    std::net::Ipv4Addr::LOCALHOST,
    7082,
));

/// Why a bootstrap file could not be found or read.
#[non_exhaustive]
#[derive(Debug)]
pub struct BootstrapError {
    kind: io::ErrorKind,
    message: String,
}

impl BootstrapError {
    fn new(kind: io::ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn invalid(path: &Path, message: impl fmt::Display) -> Self {
        Self::new(
            io::ErrorKind::InvalidData,
            format!("{}: {message}", path.display()),
        )
    }

    /// `NotFound` for a named file that does not exist or a directory that cannot be
    /// determined, `InvalidData` for a file that is not acceptable, else the I/O error's kind.
    pub fn kind(&self) -> io::ErrorKind {
        self.kind
    }
}

impl fmt::Display for BootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BootstrapError {}

impl From<BootstrapError> for io::Error {
    fn from(error: BootstrapError) -> Self {
        io::Error::new(error.kind, error.message)
    }
}

/// Where a secret's value is read, so a file names a secret without holding it.
///
/// Written as `{ file = "PATH" }`, `{ env = "NAME" }` or a bare string, the value itself. A
/// file is the default and the strongest form; an environment variable is inherited by every
/// process the reader starts, and an inline value sits in a file people copy and commit, so
/// both are reported as weakenings ([`SecretRef::weakening`]).
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq)]
pub enum SecretRef {
    /// A file; one trailing newline is dropped. Relative to the directory the reader resolves
    /// it against.
    File(PathBuf),
    /// An environment variable of the reading process.
    Env(String),
    /// The value itself.
    Inline(Zeroizing<String>),
}

impl fmt::Debug for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(path) => formatter.debug_tuple("File").field(path).finish(),
            Self::Env(name) => formatter.debug_tuple("Env").field(name).finish(),
            Self::Inline(_) => formatter.write_str("Inline(***)"),
        }
    }
}

impl SecretRef {
    /// Why this reference is weaker than a file, if it is.
    pub fn weakening(&self) -> Option<String> {
        match self {
            Self::File(_) => None,
            Self::Env(variable) => Some(format!(
                "read from the environment variable {variable}, which every process started \
                 from this one inherits"
            )),
            Self::Inline(_) => Some(
                "written inline in a configuration file, where it is copied and committed with \
                 the file; name a file instead"
                    .to_owned(),
            ),
        }
    }

    /// A file reference resolved against `dir` when it is relative; other forms unchanged.
    #[must_use]
    pub fn relative_to(self, dir: &Path) -> Self {
        match self {
            Self::File(path) if path.is_relative() => Self::File(dir.join(path)),
            other => other,
        }
    }

    /// The value. A file's one trailing newline (`\n` or `\r\n`) is dropped.
    ///
    /// # Errors
    ///
    /// `NotFound` for a file or variable that does not exist, `InvalidData` for an empty
    /// value or one that is not UTF-8. The message names the file or variable, never the
    /// value.
    pub fn read(&self) -> io::Result<Zeroizing<String>> {
        let mut value = match self {
            Self::File(path) => Zeroizing::new(std::fs::read_to_string(path).map_err(|error| {
                io::Error::new(error.kind(), format!("{}: {error}", path.display()))
            })?),
            Self::Env(variable) => match std::env::var(variable) {
                Ok(value) => Zeroizing::new(value),
                Err(std::env::VarError::NotPresent) => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("the environment variable {variable} is not set"),
                    ));
                }
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("the environment variable {variable} is not UTF-8"),
                    ));
                }
            },
            Self::Inline(value) => value.clone(),
        };
        if matches!(self, Self::File(_)) && value.ends_with('\n') {
            value.pop();
            if value.ends_with('\r') {
                value.pop();
            }
        }
        if value.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is empty", self.describe()),
            ));
        }
        Ok(value)
    }

    /// Where the value comes from, for messages: the file or variable, never the value.
    pub fn describe(&self) -> String {
        match self {
            Self::File(path) => format!("the secret file {}", path.display()),
            Self::Env(variable) => format!("the environment variable {variable}"),
            Self::Inline(_) => "the inline secret".to_owned(),
        }
    }
}

impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Written {
            file: Option<PathBuf>,
            env: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Form {
            Inline(String),
            Table(Written),
        }
        let refused = |message: &str| serde::de::Error::custom(message);
        match Form::deserialize(deserializer).map_err(|_| {
            refused("a secret is { file = \"PATH\" }, { env = \"NAME\" } or the value itself")
        })? {
            Form::Inline(value) if value.is_empty() => Err(refused("an inline secret is empty")),
            Form::Inline(value) => Ok(Self::Inline(Zeroizing::new(value))),
            Form::Table(Written {
                file: Some(file),
                env: None,
            }) if !file.as_os_str().is_empty() => Ok(Self::File(file)),
            Form::Table(Written {
                file: None,
                env: Some(env),
            }) if is_variable_name(&env) => Ok(Self::Env(env)),
            Form::Table(_) => Err(refused(
                "a secret names one non-empty file or one environment variable",
            )),
        }
    }
}

fn is_variable_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && characters.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// `[provider]`: where the configuration provider is, and how much of a document it reads.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProviderSettings {
    /// A configuration directory, `sqlite:PATH` or a `postgres://` URL. Default: the
    /// directory holding the bootstrap file, else the user's configuration directory.
    pub location: Option<String>,
    /// The database password, when the location needs one and gives none.
    pub password: Option<SecretRef>,
    /// Seconds between checks for changes; 0 turns reloading off. Default 5.
    pub poll: Option<Seconds>,
    /// Bytes of the loadout document ([`ConfigLimits::loadout_bytes`]). Default 1 MiB.
    pub loadout_bytes: Limit,
    /// Characters of one loadout value ([`ConfigLimits::loadout_value_chars`]).
    /// Default 4,096.
    pub loadout_value_chars: Limit,
    /// Bytes of the access policy ([`ConfigLimits::access_policy_bytes`]). Default 1 MiB.
    pub access_policy_bytes: Limit,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        let limits = ConfigLimits::default();
        Self {
            location: None,
            password: None,
            poll: None,
            loadout_bytes: limits.loadout_bytes,
            loadout_value_chars: limits.loadout_value_chars,
            access_policy_bytes: limits.access_policy_bytes,
        }
    }
}

/// `[registry]`: where the shared Registry is, and how much it keeps.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RegistrySettings {
    /// A directory, `sqlite:PATH` or a `postgres://` URL. Default: the provider's database
    /// when the provider is SQL, else `registry` in the state directory.
    pub location: Option<String>,
    /// The database password, when the location needs one and gives none.
    pub password: Option<SecretRef>,
    /// Seconds the Headquarters lease lasts between renewals; it also sets the SQL
    /// connections' timeouts. Default 60.
    pub headquarters_lease: Option<Seconds>,
    /// Seconds a SQLite connection waits for another's lock. Default 30.
    pub sqlite_busy_timeout: Option<Seconds>,
    /// `[registry.limits]`: what the Registry keeps and accepts.
    pub limits: RegistryLimits,
}

/// `[identity]`: where this host keeps its certificate identity and pinned peers.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentitySettings {
    /// The directory holding `identity.pem` and `peers/`. Default: the state directory.
    pub dir: Option<PathBuf>,
}

/// `[plugins]`: the provider libraries this host loads, and how it checks them.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSettings {
    /// Directories or library files to scan for providers.
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    /// `strict`, `report` or `off`. Default: `strict` with a pin set, `report` without.
    pub profile: Option<PluginProfile>,
}

/// `[secrets]`: where the Station's own secret backend reads values.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretSettings {
    /// The directory a relative secret file is read from. Default: `secrets` beside the
    /// bootstrap file, else in the user's configuration directory.
    pub dir: Option<PathBuf>,
}

/// `[unex]`: Universal Exports, where a Station publishes and `ai6ctl` connects.
#[non_exhaustive]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnexSettings {
    /// Universal Exports' origin.
    pub url: String,
    /// The Station's credential for publishing. `ai6ctl` never reads it.
    pub token: Option<SecretRef>,
}

/// `ai6.cfg`: what a Station and the `ai6` command need before they reach the configuration
/// provider. Every key has a default, so no file at all is a working configuration.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    /// The Station's ID in the shared Registry. Default: the one kept in the state directory,
    /// made up on first start.
    pub station: Option<String>,
    /// Where the Station keeps its state. Default: the platform's state directory
    /// ([`platform_state_dir`]).
    pub state_dir: Option<PathBuf>,
    /// The Station's private listener. Default [`DEFAULT_LISTEN`].
    pub listen: Option<SocketAddr>,
    /// `[provider]`.
    #[serde(default)]
    pub provider: ProviderSettings,
    /// `[registry]`.
    #[serde(default)]
    pub registry: RegistrySettings,
    /// `[identity]`.
    #[serde(default)]
    pub identity: IdentitySettings,
    /// `[plugins]`.
    #[serde(default)]
    pub plugins: PluginSettings,
    /// `[secrets]`.
    #[serde(default)]
    pub secrets: SecretSettings,
    /// `[unex]`.
    pub unex: Option<UnexSettings>,
}

/// `unex.cfg`: what Universal Exports needs before it reaches the configuration provider.
/// The sign-in providers, login limits, access policy and assistant settings are records in
/// the provider.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnexBootstrap {
    /// The loopback address it listens on, behind the HTTPS reverse proxy. Default
    /// [`DEFAULT_UNEX_LISTEN`].
    pub listen: Option<SocketAddr>,
    /// The public HTTPS origin browsers reach it at. Required to serve.
    pub public_origin: Option<String>,
    /// The credential Stations publish with. Required to serve.
    pub station_token: Option<SecretRef>,
    /// What a trusted reverse proxy sends in `X-AI6-Proxy-Secret`, when it asserts who is
    /// signed in.
    pub proxy_secret: Option<SecretRef>,
    /// The fewest bytes the reverse-proxy secret may hold. Default [`DEFAULT_PROXY_SECRET_MIN_BYTES`];
    /// a smaller value is a weakening.
    pub proxy_secret_min_bytes: Option<u64>,
    /// Where it keeps its state: verified copies of provider libraries, and the Registry when
    /// nothing else names one. Default: the platform's state directory.
    pub state_dir: Option<PathBuf>,
    /// `[provider]`, as in `ai6.cfg`.
    #[serde(default)]
    pub provider: ProviderSettings,
    /// `[registry]`, as in `ai6.cfg`.
    #[serde(default)]
    pub registry: RegistrySettings,
    /// `[plugins]`, as in `ai6.cfg`.
    #[serde(default)]
    pub plugins: PluginSettings,
}

impl UnexBootstrap {
    /// Finds `unex.cfg` ([`SearchPath`] for [`UNEX_FILE_NAME`] and [`UNEX_CONFIG_ENV`]) and
    /// reads it; with none, every key at its default.
    ///
    /// # Errors
    ///
    /// As [`SearchPath::locate`] and [`UnexBootstrap::read`].
    pub fn find(explicit: Option<&Path>) -> Result<Located<Self>, BootstrapError> {
        match SearchPath::new(UNEX_FILE_NAME, UNEX_CONFIG_ENV, explicit).locate()? {
            Some(path) => Ok(Located {
                value: Self::read(&path)?,
                path: Some(path),
            }),
            None => Ok(Located::default()),
        }
    }

    /// Reads `unex.cfg` at `path`, resolving its relative paths against its directory.
    ///
    /// # Errors
    ///
    /// As [`read_file`].
    pub fn read(path: &Path) -> Result<Self, BootstrapError> {
        let mut value: Self = read_file(path)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        value.state_dir = value.state_dir.map(|state| beside(dir, &state));
        value.station_token = value.station_token.map(|token| token.relative_to(dir));
        value.proxy_secret = value.proxy_secret.map(|secret| secret.relative_to(dir));
        value.provider.resolve(dir);
        value.registry.resolve(dir);
        value.plugins.resolve(dir);
        Ok(value)
    }
}

impl Located<UnexBootstrap> {
    /// The state directory: the file's `state_dir`, else [`platform_state_dir`].
    ///
    /// # Errors
    ///
    /// `NotFound` when neither is set and the platform's cannot be determined.
    pub fn state_dir(&self) -> Result<PathBuf, BootstrapError> {
        match &self.value.state_dir {
            Some(dir) => Ok(dir.clone()),
            None => platform_state_dir(),
        }
    }

    /// The configuration provider's location, chosen as for `ai6.cfg`
    /// ([`Located::provider_location`]).
    ///
    /// # Errors
    ///
    /// `NotFound` when none can be determined.
    pub fn provider_location(&self) -> Result<String, BootstrapError> {
        provider_location(&self.value.provider, self.dir())
    }

    /// The Registry's location, chosen as for `ai6.cfg` ([`Located::registry_location`]).
    ///
    /// # Errors
    ///
    /// As [`state_dir`](Self::state_dir) and [`provider_location`](Self::provider_location).
    pub fn registry_location(&self) -> Result<String, BootstrapError> {
        registry_location(&self.value.registry, self.provider_location()?, || {
            self.state_dir()
        })
    }

    /// The Registry's password, chosen as for `ai6.cfg` ([`Located::registry_password`]).
    pub fn registry_password(&self) -> Option<&SecretRef> {
        registry_password(&self.value.registry, &self.value.provider)
    }

    /// Its listener: `listen`, else [`DEFAULT_UNEX_LISTEN`].
    pub fn listen(&self) -> SocketAddr {
        self.value.listen.unwrap_or(DEFAULT_UNEX_LISTEN)
    }

    /// The fewest bytes the reverse-proxy secret may hold: `proxy_secret_min_bytes`, else
    /// [`DEFAULT_PROXY_SECRET_MIN_BYTES`].
    pub fn proxy_secret_min_bytes(&self) -> u64 {
        self.value
            .proxy_secret_min_bytes
            .unwrap_or(DEFAULT_PROXY_SECRET_MIN_BYTES)
    }

    /// Each secret of the file that is read from the environment or written inline, and each
    /// `[registry]` or `[provider]` limit looser than its default, worded for a log line.
    pub fn weakenings(&self) -> Vec<String> {
        let mut weakened = Vec::new();
        for (what, secret) in [
            ("Station token", self.value.station_token.as_ref()),
            ("reverse-proxy secret", self.value.proxy_secret.as_ref()),
            ("provider password", self.value.provider.password.as_ref()),
            ("Registry password", self.value.registry.password.as_ref()),
        ] {
            if let Some(weakening) = secret.and_then(SecretRef::weakening) {
                weakened.push(format!("the {what} is {weakening}"));
            }
        }
        if self.proxy_secret_min_bytes() < DEFAULT_PROXY_SECRET_MIN_BYTES {
            weakened.push(format!(
                "proxy_secret_min_bytes {} is looser than the default {DEFAULT_PROXY_SECRET_MIN_BYTES}",
                self.proxy_secret_min_bytes()
            ));
        }
        weakened.extend(limit_weakenings(&self.value.registry, &self.value.provider));
        weakened
    }
}

fn provider_location(
    provider: &ProviderSettings,
    dir: Option<&Path>,
) -> Result<String, BootstrapError> {
    if let Some(location) = &provider.location {
        return Ok(location.clone());
    }
    match dir {
        Some(dir) => Ok(dir.display().to_string()),
        None => platform_config_dir().map(|dir| dir.display().to_string()),
    }
}

fn registry_location(
    registry: &RegistrySettings,
    provider: String,
    state_dir: impl FnOnce() -> Result<PathBuf, BootstrapError>,
) -> Result<String, BootstrapError> {
    if let Some(location) = &registry.location {
        return Ok(location.clone());
    }
    if is_sql_location(&provider) {
        return Ok(provider);
    }
    Ok(state_dir()?.join("registry").display().to_string())
}

fn registry_password<'a>(
    registry: &'a RegistrySettings,
    provider: &'a ProviderSettings,
) -> Option<&'a SecretRef> {
    match (&registry.password, &registry.location) {
        (Some(password), _) => Some(password),
        (None, None) => provider.password.as_ref(),
        (None, Some(_)) => None,
    }
}

/// A value read from a bootstrap file, with where it came from.
#[derive(Debug, Clone, Default)]
pub struct Located<T> {
    /// The value; its relative paths are already resolved against the file's directory.
    pub value: T,
    /// The file it was read from; `None` when no file was found and every key is a default.
    pub path: Option<PathBuf>,
}

impl<T> Located<T> {
    /// The directory of the file it was read from.
    pub fn dir(&self) -> Option<&Path> {
        self.path.as_deref().and_then(Path::parent)
    }
}

/// Where a bootstrap file is looked for, in order; the first that exists is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPath {
    /// A path given on the command line; it must exist.
    pub explicit: Option<PathBuf>,
    /// The path the environment variable names; it must exist.
    pub variable: Option<(String, PathBuf)>,
    /// The places looked in otherwise, in order; a missing one is skipped.
    pub candidates: Vec<PathBuf>,
}

impl SearchPath {
    /// The search for `name` (such as [`FILE_NAME`]): `explicit`, then the path environment
    /// variable `variable` names, then `name` in the user's configuration directory
    /// ([`platform_config_dir`]), then in the system's ([`system_config_dir`]).
    pub fn new(name: &str, variable: &str, explicit: Option<&Path>) -> Self {
        let named = std::env::var_os(variable)
            .filter(|value| !value.is_empty())
            .map(|value| (variable.to_owned(), PathBuf::from(value)));
        let candidates = [platform_config_dir().ok(), system_config_dir()]
            .into_iter()
            .flatten()
            .map(|dir| dir.join(name))
            .collect();
        Self {
            explicit: explicit.map(Path::to_path_buf),
            variable: named,
            candidates,
        }
    }

    /// The file to read, or `None` when there is none and the defaults apply.
    ///
    /// # Errors
    ///
    /// `NotFound` when the command line or the environment names a file that does not
    /// exist.
    pub fn locate(&self) -> Result<Option<PathBuf>, BootstrapError> {
        let must_exist = |path: &Path, what: &str| {
            if path.is_file() {
                Ok(Some(path.to_path_buf()))
            } else {
                Err(BootstrapError::new(
                    io::ErrorKind::NotFound,
                    format!("{}, named by {what}, does not exist", path.display()),
                ))
            }
        };
        if let Some(path) = &self.explicit {
            return must_exist(path, "--config");
        }
        if let Some((variable, path)) = &self.variable {
            return must_exist(path, variable);
        }
        Ok(self.candidates.iter().find(|path| path.is_file()).cloned())
    }
}

/// Reads a bootstrap document of type `T` from `path`, in the format its extension names.
///
/// # Errors
///
/// `NotFound` or another I/O kind when the file cannot be read, `InvalidData` when it is not
/// valid in its format or has a key `T` does not know (the error names the key).
pub fn read_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, BootstrapError> {
    let format = Format::for_path(path)
        .map_err(|error| BootstrapError::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let text = std::fs::read_to_string(path).map_err(|error| {
        BootstrapError::new(error.kind(), format!("{}: {error}", path.display()))
    })?;
    format::from_str(format, &text).map_err(|error| BootstrapError::invalid(path, error))
}

/// A path from a file, resolved against the file's directory when relative.
fn beside(dir: &Path, path: &Path) -> PathBuf {
    if path.is_relative() {
        dir.join(path)
    } else {
        path.to_path_buf()
    }
}

/// A provider or Registry location from a file: a relative directory or `sqlite:` path is
/// resolved against the file's directory; a URL is left as it is.
fn location_beside(dir: &Path, location: &str) -> String {
    if location.starts_with("postgres://") || location.starts_with("postgresql://") {
        return location.to_owned();
    }
    for prefix in ["sqlite://", "sqlite:"] {
        if let Some(path) = location.strip_prefix(prefix) {
            let (path, query) = path
                .split_once('?')
                .map_or((path, None), |(p, q)| (p, Some(q)));
            let resolved = beside(dir, Path::new(path));
            let mut resolved = format!("sqlite:{}", resolved.display());
            if let Some(query) = query {
                resolved.push('?');
                resolved.push_str(query);
            }
            return resolved;
        }
    }
    beside(dir, Path::new(location)).display().to_string()
}

/// Whether `location` names a SQL database rather than a directory.
pub fn is_sql_location(location: &str) -> bool {
    location.starts_with("sqlite:")
        || location.starts_with("postgres://")
        || location.starts_with("postgresql://")
}

impl ProviderSettings {
    fn resolve(&mut self, dir: &Path) {
        if let Some(location) = &self.location {
            self.location = Some(location_beside(dir, location));
        }
        self.password = self.password.take().map(|secret| secret.relative_to(dir));
    }

    /// The poll interval: [`DEFAULT_POLL`] unless the file says otherwise.
    pub fn poll(&self) -> Duration {
        self.poll.map_or(DEFAULT_POLL, |seconds| seconds.0)
    }

    /// The limits the configuration provider reads documents within.
    pub fn limits(&self) -> ConfigLimits {
        ConfigLimits {
            loadout_bytes: self.loadout_bytes,
            loadout_value_chars: self.loadout_value_chars,
            access_policy_bytes: self.access_policy_bytes,
        }
    }
}

impl RegistrySettings {
    fn resolve(&mut self, dir: &Path) {
        if let Some(location) = &self.location {
            self.location = Some(location_beside(dir, location));
        }
        self.password = self.password.take().map(|secret| secret.relative_to(dir));
    }

    /// The Headquarters lease: [`DEFAULT_LEASE`](crate::DEFAULT_LEASE) unless the file says
    /// otherwise.
    pub fn headquarters_lease(&self) -> Duration {
        self.headquarters_lease.map_or(
            crate::directorates::registry::store::DEFAULT_LEASE,
            |seconds| seconds.0,
        )
    }

    /// How long a SQLite connection waits for another's lock: 30 s unless the file says
    /// otherwise.
    pub fn sqlite_busy_timeout(&self) -> Duration {
        self.sqlite_busy_timeout
            .map_or(DEFAULT_SQLITE_BUSY_TIMEOUT, |seconds| seconds.0)
    }
}

/// How long a SQLite connection waits for another's lock when the file says nothing: a slow
/// disk or a busy CI runner must not turn into a refused write.
pub(crate) const DEFAULT_SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// One line per limit of `[registry]` and `[provider]` looser than its default.
fn limit_weakenings(registry: &RegistrySettings, provider: &ProviderSettings) -> Vec<String> {
    let mut weakened = registry.limits.weakenings();
    if registry.sqlite_busy_timeout() > DEFAULT_SQLITE_BUSY_TIMEOUT {
        weakened.push(format!(
            "registry.sqlite_busy_timeout {} is looser than the default {}",
            registry.sqlite_busy_timeout().as_secs_f64(),
            DEFAULT_SQLITE_BUSY_TIMEOUT.as_secs_f64()
        ));
    }
    weakened.extend(provider.limits().weakenings());
    weakened
}

impl PluginSettings {
    fn resolve(&mut self, dir: &Path) {
        for path in &mut self.paths {
            *path = beside(dir, path);
        }
    }
}

impl Bootstrap {
    /// Finds the bootstrap file ([`SearchPath`] for [`FILE_NAME`] and [`CONFIG_ENV`]) and
    /// reads it; with none, every key at its default.
    ///
    /// # Errors
    ///
    /// As [`SearchPath::locate`] and [`Bootstrap::read`].
    pub fn find(explicit: Option<&Path>) -> Result<Located<Self>, BootstrapError> {
        Self::find_in(&SearchPath::new(FILE_NAME, CONFIG_ENV, explicit))
    }

    /// As [`Bootstrap::find`], along a given search path.
    ///
    /// # Errors
    ///
    /// As [`Bootstrap::find`].
    pub fn find_in(search: &SearchPath) -> Result<Located<Self>, BootstrapError> {
        match search.locate()? {
            Some(path) => Ok(Located {
                value: Self::read(&path)?,
                path: Some(path),
            }),
            None => Ok(Located::default()),
        }
    }

    /// Reads the bootstrap file at `path`, resolving its relative paths against its
    /// directory.
    ///
    /// # Errors
    ///
    /// As [`read_file`]; and `InvalidData` when `[unex]` has a `url` that is empty.
    pub fn read(path: &Path) -> Result<Self, BootstrapError> {
        let mut value: Self = read_file(path)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        value.state_dir = value.state_dir.map(|state| beside(dir, &state));
        value.identity.dir = value.identity.dir.map(|identity| beside(dir, &identity));
        value.secrets.dir = value.secrets.dir.map(|secrets| beside(dir, &secrets));
        value.provider.resolve(dir);
        value.registry.resolve(dir);
        value.plugins.resolve(dir);
        if let Some(unex) = &mut value.unex {
            if unex.url.trim().is_empty() {
                return Err(BootstrapError::invalid(path, "[unex] url is empty"));
            }
            unex.token = unex.token.take().map(|token| token.relative_to(dir));
        }
        Ok(value)
    }
}

impl Located<Bootstrap> {
    /// The state directory: the file's `state_dir`, else [`platform_state_dir`].
    ///
    /// # Errors
    ///
    /// `NotFound` when neither is set and the platform's cannot be determined.
    pub fn state_dir(&self) -> Result<PathBuf, BootstrapError> {
        match &self.value.state_dir {
            Some(dir) => Ok(dir.clone()),
            None => platform_state_dir(),
        }
    }

    /// The configuration provider's location: `[provider] location`, else the directory
    /// holding the bootstrap file, else [`platform_config_dir`].
    ///
    /// # Errors
    ///
    /// `NotFound` when none can be determined.
    pub fn provider_location(&self) -> Result<String, BootstrapError> {
        provider_location(&self.value.provider, self.dir())
    }

    /// The Registry's location: `[registry] location`, else the provider's database when the
    /// provider is SQL, else `registry` in the state directory.
    ///
    /// # Errors
    ///
    /// As [`state_dir`](Self::state_dir) and [`provider_location`](Self::provider_location).
    pub fn registry_location(&self) -> Result<String, BootstrapError> {
        registry_location(&self.value.registry, self.provider_location()?, || {
            self.state_dir()
        })
    }

    /// The Registry's password: `[registry] password`, else the provider's when the Registry
    /// is the provider's database.
    pub fn registry_password(&self) -> Option<&SecretRef> {
        registry_password(&self.value.registry, &self.value.provider)
    }

    /// The identity directory: `[identity] dir`, else the state directory.
    ///
    /// # Errors
    ///
    /// As [`state_dir`](Self::state_dir).
    pub fn identity_dir(&self) -> Result<PathBuf, BootstrapError> {
        match &self.value.identity.dir {
            Some(dir) => Ok(dir.clone()),
            None => self.state_dir(),
        }
    }

    /// The secrets directory: `[secrets] dir`, else `secrets` beside the bootstrap file, else
    /// in [`platform_config_dir`].
    ///
    /// # Errors
    ///
    /// `NotFound` when none can be determined.
    pub fn secrets_dir(&self) -> Result<PathBuf, BootstrapError> {
        if let Some(dir) = &self.value.secrets.dir {
            return Ok(dir.clone());
        }
        match self.dir() {
            Some(dir) => Ok(dir.join("secrets")),
            None => platform_config_dir().map(|dir| dir.join("secrets")),
        }
    }

    /// The Station's listener: `listen`, else [`DEFAULT_LISTEN`].
    pub fn listen(&self) -> SocketAddr {
        self.value.listen.unwrap_or(DEFAULT_LISTEN)
    }

    /// What the file gives up against the defaults, worded for a log line: each secret that
    /// is read from the environment or written inline, and each `[registry]` or `[provider]`
    /// limit looser than its default.
    pub fn weakenings(&self) -> Vec<String> {
        let mut weakened = Vec::new();
        let mut secret = |what: &str, secret: Option<&SecretRef>| {
            if let Some(weakening) = secret.and_then(SecretRef::weakening) {
                weakened.push(format!("the {what} is {weakening}"));
            }
        };
        secret("provider password", self.value.provider.password.as_ref());
        secret("Registry password", self.value.registry.password.as_ref());
        secret(
            "Universal Exports token",
            self.value
                .unex
                .as_ref()
                .and_then(|unex| unex.token.as_ref()),
        );
        weakened.extend(limit_weakenings(&self.value.registry, &self.value.provider));
        weakened
    }
}

fn environment(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn home() -> Option<PathBuf> {
    environment("HOME")
        .or_else(|| environment("USERPROFILE"))
        .map(PathBuf::from)
}

/// The user's AI6 configuration directory: `XDG_CONFIG_HOME/ai6`, `%APPDATA%\ai6` on
/// Windows, else `~/.config/ai6`.
///
/// # Errors
///
/// `NotFound` when no home directory can be determined.
pub fn platform_config_dir() -> Result<PathBuf, BootstrapError> {
    if let Some(path) = environment("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("ai6"));
    }
    #[cfg(windows)]
    if let Some(path) = environment("APPDATA") {
        return Ok(PathBuf::from(path).join("ai6"));
    }
    home()
        .map(|home| home.join(".config").join("ai6"))
        .ok_or_else(|| {
            BootstrapError::new(
                io::ErrorKind::NotFound,
                "could not determine the user's configuration directory",
            )
        })
}

/// The system's AI6 configuration directory: `/etc/ai6` on Unix, `%ProgramData%\ai6` on
/// Windows; `None` where there is none.
pub fn system_config_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return environment("ProgramData").map(|path| PathBuf::from(path).join("ai6"));
    #[cfg(not(windows))]
    Some(PathBuf::from("/etc/ai6"))
}

/// The platform's AI6 state directory: `%LOCALAPPDATA%\AI6` on Windows, else
/// `XDG_STATE_HOME/AI6` or `~/.local/state/AI6`.
///
/// # Errors
///
/// `NotFound` when no home directory can be determined.
pub fn platform_state_dir() -> Result<PathBuf, BootstrapError> {
    #[cfg(windows)]
    if let Some(path) = environment("LOCALAPPDATA") {
        return Ok(PathBuf::from(path).join("AI6"));
    }
    environment("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|home| home.join(".local").join("state")))
        .map(|base| base.join("AI6"))
        .ok_or_else(|| {
            BootstrapError::new(
                io::ErrorKind::NotFound,
                "could not determine AI6's state directory",
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_reference_reads_a_file_variable_or_inline_value_and_hides_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("token"), "s3cret\r\n").unwrap();
        let file: SecretRef = toml::from_str::<toml::Table>("s = { file = \"token\" }")
            .unwrap()
            .remove("s")
            .unwrap()
            .try_into()
            .unwrap();
        let file = file.relative_to(dir.path());
        assert_eq!(file.read().unwrap().as_str(), "s3cret");
        assert_eq!(file.weakening(), None);

        let inline: SecretRef = serde_json::from_str("\"hunter2\"").unwrap();
        assert_eq!(format!("{inline:?}"), "Inline(***)");
        assert!(inline.weakening().unwrap().contains("inline"));
        assert_eq!(inline.read().unwrap().as_str(), "hunter2");

        let env: SecretRef =
            serde_json::from_str(r#"{"env": "AI6_TEST_NO_SUCH_VARIABLE"}"#).unwrap();
        assert!(
            env.weakening()
                .unwrap()
                .contains("AI6_TEST_NO_SUCH_VARIABLE")
        );
        assert_eq!(env.read().unwrap_err().kind(), io::ErrorKind::NotFound);

        for refused in [
            r#"{"file": "a", "env": "B"}"#,
            r#"{"env": "1X"}"#,
            r#""""#,
            "{}",
        ] {
            assert!(
                serde_json::from_str::<SecretRef>(refused).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn unex_cfg_reports_its_loosened_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unex.cfg");
        std::fs::write(
            &path,
            "[provider]\naccess_policy_bytes = \"none\"\n[registry.limits.held]\nresults = 100\n",
        )
        .unwrap();
        let located = UnexBootstrap::find(Some(&path)).unwrap();
        assert_eq!(located.value.registry.limits.held.results, Limit::at(100));
        assert_eq!(
            located.weakenings(),
            ["provider.access_policy_bytes none is looser than the default 1048576"]
        );
    }

    #[test]
    fn unex_cfg_reads_the_proxy_secret_floor_and_reports_a_lower_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unex.cfg");
        std::fs::write(&path, "").unwrap();
        let located = UnexBootstrap::find(Some(&path)).unwrap();
        assert_eq!(located.proxy_secret_min_bytes(), 32);
        assert!(located.weakenings().is_empty());
        std::fs::write(
            &path,
            "proxy_secret_min_bytes = 8
",
        )
        .unwrap();
        let located = UnexBootstrap::find(Some(&path)).unwrap();
        assert_eq!(located.proxy_secret_min_bytes(), 8);
        assert_eq!(
            located.weakenings(),
            ["proxy_secret_min_bytes 8 is looser than the default 32"]
        );
    }

    #[test]
    fn relative_paths_and_locations_resolve_beside_the_file() {
        let dir = Path::new("/srv/ai6");
        assert_eq!(
            location_beside(dir, "config"),
            dir.join("config").display().to_string()
        );
        assert_eq!(
            location_beside(dir, "sqlite:ai6.db?mode=rw"),
            format!("sqlite:{}?mode=rw", dir.join("ai6.db").display())
        );
        assert_eq!(
            location_beside(dir, "postgres://db/ai6"),
            "postgres://db/ai6"
        );
    }
}
