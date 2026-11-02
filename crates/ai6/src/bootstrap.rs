//! The bootstrap file: what a process needs before it can reach its configuration provider.
//!
//! One file, `ai6.cfg` ([`Bootstrap`]), serves every program on a host or for a user: a Station
//! and the `ai6` command read `[station]`, Universal Exports its part of `[unex]`, `ai6ctl` and a
//! Station the rest of `[unex]`, and they share `[provider]` and `[registry]`. Outer Office reads
//! `[unex]` and `[oo]`, which no Rust program reads. A file holds only what starting needs: where
//! the configuration provider and the Registry are, the passwords to reach them, this host's
//! state, identity and secrets directories, its listeners, and how it checks the provider
//! libraries it loads. Everything else is a record in the configuration provider.
//!
//! The file is found by [`SearchPath`]: a path given on the command line, else the environment
//! variable naming one, else the user's then the system's configuration directory. The first file
//! found is the only one read; files are never merged, and with none every key takes its default.
//! The working directory is never searched.
//!
//! The format follows the extension ([`format`](mod@format)). Unknown keys are refused, by every
//! program, and each program says when a key it needs is missing. A relative path in the file is
//! relative to the file's directory. Secrets are [`SecretRef`]s, never values the file must hold.

pub mod format;
pub mod public;

use std::ffi::OsString;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use guatiao::schema::KindBuilder;
use guatiao::{Alloc, FromValue, MapError, Schema, ToValue, Value, ValueError};
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use zeroize::Zeroizing;

use crate::bounds::Limit;
use crate::bounds::Seconds;
use crate::directorates::m::config::ConfigLimits;
use crate::directorates::registry::bona_fides::{KeepVerified, PluginProfile};
use crate::directorates::registry::limits::RegistryLimits;

use self::format::Format;

/// The bootstrap file.
pub const FILE_NAME: &str = "ai6.cfg";
/// The environment variable naming the bootstrap file.
pub const CONFIG_ENV: &str = "AI6_CONFIG";
/// The listener a Station uses when its bootstrap file names none.
pub const DEFAULT_LISTEN: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
    std::net::Ipv4Addr::LOCALHOST,
    7081,
));
/// How often a Station checks its configuration provider for changes when the file says
/// nothing.
pub const DEFAULT_POLL: Duration = Duration::from_secs(5);
/// The fewest bytes a reverse-proxy secret holds unless the file says otherwise.
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

/// How a file names a secret kept in a file.
#[derive(ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
struct SecretInFile {
    /// Its path, relative to the bootstrap file.
    #[schema(title = "File")]
    file: PathBuf,
}

/// How a file names a secret kept in an environment variable.
#[derive(ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
struct SecretInVariable {
    /// The variable's name.
    #[schema(title = "Environment variable")]
    env: String,
}

impl ToValue for SecretRef {
    /// A file or a variable as a file names it. An inline secret is refused: nothing written
    /// from a [`Bootstrap`] holds a value.
    fn to_value_in(&self, alloc: Alloc) -> Result<Value, ValueError> {
        match self {
            Self::File(file) => SecretInFile { file: file.clone() }.to_value_in(alloc),
            Self::Env(env) => SecretInVariable { env: env.clone() }.to_value_in(alloc),
            Self::Inline(_) => Err(ValueError::Unrepresentable),
        }
    }
}

impl FromValue for SecretRef {
    fn from_value(value: &Value) -> Result<Self, MapError> {
        const WRITTEN: &str = "a secret: one non-empty { file = \"PATH\" }, one \
                               { env = \"NAME\" } or the value itself";
        if let Ok(text) = <&str>::try_from(value) {
            return if text.is_empty() {
                Err(MapError::bad_value(WRITTEN))
            } else {
                Ok(Self::Inline(Zeroizing::new(text.to_owned())))
            };
        }
        if let Ok(SecretInFile { file }) = SecretInFile::from_value(value)
            && !file.as_os_str().is_empty()
        {
            return Ok(Self::File(file));
        }
        if let Ok(SecretInVariable { env }) = SecretInVariable::from_value(value)
            && is_variable_name(&env)
        {
            return Ok(Self::Env(env));
        }
        Err(MapError::bad_value(WRITTEN))
    }
}

impl Schema for SecretRef {
    fn kind(alloc: Alloc) -> KindBuilder {
        KindBuilder::union_in(
            alloc,
            vec![
                SecretInFile::kind(alloc),
                SecretInVariable::kind(alloc),
                KindBuilder::string_in(alloc),
            ],
        )
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
#[derive(Debug, Clone, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ProviderSettings {
    /// A configuration directory, `sqlite:PATH` or a `postgres://` URL. Default: the
    /// directory holding the bootstrap file, else the user's configuration directory.
    #[schema(title = "Where the configuration provider is")]
    pub location: Option<String>,
    /// The database password, when the location needs one and gives none. Default: none.
    #[schema(title = "Database password", sensitive)]
    pub password: Option<SecretRef>,
    /// Seconds between checks for changes; 0 turns reloading off. Default: 5.
    #[schema(title = "Between checks for changes")]
    pub poll: Option<Seconds>,
    /// Bytes of the loadout document (`loadout`, `sandbox`, `mcp`, `secret` and `pool` records
    /// together).
    #[schema(title = "Loadout document", format = "bytes")]
    #[map(default = ConfigLimits::default().loadout_bytes)]
    pub loadout_bytes: Limit,
    /// Characters of one text value in the loadout document.
    #[schema(title = "One loadout value, in characters")]
    #[map(default = ConfigLimits::default().loadout_value_chars)]
    pub loadout_value_chars: Limit,
    /// Bytes of the access policy (the `access` record).
    #[schema(title = "Access policy", format = "bytes")]
    #[map(default = ConfigLimits::default().access_policy_bytes)]
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
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct RegistrySettings {
    /// A directory, `sqlite:PATH` or a `postgres://` URL. Default: the provider's database
    /// when the provider is SQL, else `registry` in the reading program's state directory.
    #[schema(title = "Where the Registry is")]
    pub location: Option<String>,
    /// The database password, when the location needs one and gives none. Default: the
    /// provider's, when the Registry is the provider's database.
    #[schema(title = "Database password", sensitive)]
    pub password: Option<SecretRef>,
    /// Seconds the Headquarters lease lasts between renewals; a PostgreSQL statement, lock wait
    /// and idle transaction end after a quarter of it, a connection attempt after a sixth.
    /// Default: 60.
    #[schema(title = "Headquarters lease")]
    pub headquarters_lease: Option<Seconds>,
    /// Seconds a SQLite connection waits for another's lock. Default: 30.
    #[schema(title = "SQLite lock wait")]
    pub sqlite_busy_timeout: Option<Seconds>,
    /// `[registry.limits]`: what the Registry keeps and accepts.
    #[schema(title = "What the Registry keeps and accepts")]
    #[map(default)]
    pub limits: RegistryLimits,
}

/// `[station.identity]`: where this host keeps its certificate identity and pinned peers.
#[non_exhaustive]
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct IdentitySettings {
    /// The directory holding `identity.pem` and `peers/`. Default: the state directory.
    #[schema(title = "Identity directory")]
    pub dir: Option<PathBuf>,
}

/// `[station.plugins]` and `[unex.plugins]`: the provider libraries a program loads, and how it
/// checks them.
#[non_exhaustive]
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct PluginSettings {
    /// Directories or library files to scan for providers.
    #[schema(title = "Where to look for providers")]
    #[map(default)]
    pub paths: Vec<PathBuf>,
    /// How a library is checked. Default: `strict` with a pin set, `report` without.
    #[schema(title = "How libraries are checked")]
    pub profile: Option<PluginProfile>,
    /// Which verified copies a `strict` scan keeps besides the ones it loaded: a count, `0` for
    /// none of them, or `"none"` for no limit. Default: the newest 8.
    #[schema(title = "Verified copies kept")]
    pub keep_verified: Option<KeepVerified>,
}

/// `[station.secrets]`: where the Station's own secret backend reads values.
#[non_exhaustive]
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct SecretSettings {
    /// The directory a relative secret file is read from. Default: `secrets` beside the
    /// bootstrap file, else in the user's configuration directory.
    #[schema(title = "Secrets directory")]
    pub dir: Option<PathBuf>,
}

/// `[station]`: what a Station and the `ai6` command need of this host.
#[non_exhaustive]
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct StationSettings {
    /// The Station's ID in the shared Registry. Default: the one kept in the state directory,
    /// made up on first start.
    #[schema(title = "Station ID")]
    pub id: Option<String>,
    /// Where the Station keeps its state. Default: the platform's state directory.
    #[schema(title = "State directory")]
    pub state_dir: Option<PathBuf>,
    /// The Station's private listener. Default: 127.0.0.1:7081.
    #[schema(title = "Listener")]
    pub listen: Option<SocketAddr>,
    /// `[station.plugins]`.
    #[schema(title = "Provider libraries")]
    #[map(default)]
    pub plugins: PluginSettings,
    /// `[station.identity]`.
    #[schema(title = "Certificate identity")]
    #[map(default)]
    pub identity: IdentitySettings,
    /// `[station.secrets]`.
    #[schema(title = "Secrets")]
    #[map(default)]
    pub secrets: SecretSettings,
}

/// `[unex.access]`: who is chief before any record is read.
#[non_exhaustive]
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AccessSettings {
    /// Subjects (`account:<name>`, `person:<id>`, `group:<name>`) that are always chief, whatever
    /// the access policy says: host access is already the trust root. None of them can be
    /// removed through the API.
    #[schema(title = "Always chief")]
    #[map(default)]
    pub chief: Vec<String>,
}

/// `[unex.pages]`: pages are kept in a directory (so remembered ones outlive a restart) and
/// served under `/pages/` on the API's origin unless they have an origin of their own. Quotas and
/// options are the roster's `[assistant.pages]`.
#[non_exhaustive]
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct PagesSettings {
    /// Where pages are kept. Default: `pages` beside the bootstrap file (the working directory
    /// without one).
    #[schema(title = "Pages directory")]
    pub dir: Option<PathBuf>,
    /// The pages' own origin, such as `https://pages.example.net`, served by a second
    /// listener that holds only page routes. Default: none, so pages share the API's origin by
    /// design, and nothing is logged about it.
    #[schema(title = "The pages' own origin")]
    pub origin: Option<String>,
    /// The loopback address of that second listener. Default: 127.0.0.1:7083.
    #[schema(title = "The pages' own listener")]
    pub listen: Option<SocketAddr>,
}

/// The default address of the pages' own listener.
pub const DEFAULT_PAGES_LISTEN: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 7083);

/// `[unex]`: where Universal Exports is, for its clients, and how it runs, for itself.
#[non_exhaustive]
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct UnexSettings {
    /// Universal Exports' origin, where a Station publishes and `ai6ctl` and Outer Office connect.
    /// Default: none; a Station publishes nothing, and `ai6ctl` tries its loopback default.
    #[schema(title = "Universal Exports' origin", format = "uri")]
    pub url: Option<String>,
    /// A PEM file of certificate authorities its clients trust for Universal Exports besides the
    /// public roots. Default: the public roots alone.
    #[schema(title = "More certificate authorities to trust")]
    pub ca_file: Option<PathBuf>,
    /// The credential of the Station channel: a Station publishes with it, and Universal Exports
    /// accepts it. `ai6ctl` never reads it. Default: none; a Station publishes nothing, and
    /// `universal-exports` refuses to start.
    #[schema(title = "Station channel credential", sensitive)]
    pub station_token: Option<SecretRef>,
    /// The loopback address it listens on, behind the HTTPS reverse proxy. Default: 127.0.0.1:7082.
    #[schema(title = "Gateway listener")]
    pub listen: Option<SocketAddr>,
    /// The public HTTPS origin browsers reach it at. Default: none; `universal-exports` refuses
    /// to start.
    #[schema(title = "Public origin", format = "uri")]
    pub public_origin: Option<String>,
    /// What a trusted reverse proxy sends in `X-AI6-Proxy-Secret`, when it asserts who is
    /// signed in. Default: none, and no reverse-proxy identity is accepted.
    #[schema(title = "Reverse-proxy secret", sensitive)]
    pub proxy_secret: Option<SecretRef>,
    /// The fewest bytes the reverse-proxy secret may hold; a smaller value is a weakening.
    /// Default: 32.
    #[schema(title = "Shortest reverse-proxy secret", format = "bytes")]
    pub proxy_secret_min_bytes: Option<u64>,
    /// Where it keeps its state: verified copies of provider libraries, and the Registry when
    /// nothing else names one. Default: the platform's state directory.
    #[schema(title = "Gateway state directory")]
    pub state_dir: Option<PathBuf>,
    /// `[unex.plugins]`.
    #[schema(title = "Provider libraries")]
    #[map(default)]
    pub plugins: PluginSettings,
    /// `[unex.access]`.
    #[schema(title = "Host access")]
    #[map(default)]
    pub access: AccessSettings,
    /// `[unex.pages]`: where the pages Moneypenny publishes are kept and served from.
    #[schema(title = "Moneypenny's pages")]
    #[map(default)]
    pub pages: PagesSettings,
}

/// `ai6.cfg`: what every program on a host or for a user needs before it reaches the
/// configuration provider. Every key has a default, so no file at all is a working configuration
/// for a program that needs no key.
#[non_exhaustive]
#[derive(Debug, Clone, Default, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct Bootstrap {
    /// `[provider]`.
    #[schema(title = "Configuration provider")]
    #[map(default)]
    pub provider: ProviderSettings,
    /// `[registry]`.
    #[schema(title = "Registry")]
    #[map(default)]
    pub registry: RegistrySettings,
    /// `[unex]`.
    #[schema(title = "Universal Exports")]
    #[map(default)]
    pub unex: UnexSettings,
    /// `[station]`.
    #[schema(title = "Station")]
    #[map(default)]
    pub station: StationSettings,
    /// `[oo]`: Outer Office's settings, which the app reads and declares. No Rust program reads
    /// them; [`public`] copies what a web page needs.
    #[schema(title = "Outer Office")]
    #[map(with = guatiao_serde::adapt::object, default)]
    pub oo: serde_json::Map<String, serde_json::Value>,
}

impl Located<Bootstrap> {
    /// Where the gateway keeps pages: `[unex.pages] dir`, else `pages` beside the bootstrap file,
    /// else `pages` in the working directory.
    pub fn pages_dir(&self) -> PathBuf {
        match &self.value.unex.pages.dir {
            Some(dir) => dir.clone(),
            None => self
                .dir()
                .map_or_else(|| PathBuf::from("pages"), |dir| dir.join("pages")),
        }
    }

    /// The pages' own listener: `[unex.pages] listen`, else [`DEFAULT_PAGES_LISTEN`].
    pub fn pages_listen(&self) -> SocketAddr {
        self.value.unex.pages.listen.unwrap_or(DEFAULT_PAGES_LISTEN)
    }

    /// The gateway's state directory: `[unex] state_dir`, else [`platform_state_dir`].
    ///
    /// # Errors
    ///
    /// `NotFound` when neither is set and the platform's cannot be determined.
    pub fn unex_state_dir(&self) -> Result<PathBuf, BootstrapError> {
        match &self.value.unex.state_dir {
            Some(dir) => Ok(dir.clone()),
            None => platform_state_dir(),
        }
    }

    /// The gateway's Registry, chosen as [`registry_location`](Self::registry_location) is, in
    /// its own state directory.
    ///
    /// # Errors
    ///
    /// As [`unex_state_dir`](Self::unex_state_dir) and
    /// [`provider_location`](Self::provider_location).
    pub fn unex_registry_location(&self) -> Result<String, BootstrapError> {
        registry_location(&self.value.registry, self.provider_location()?, || {
            self.unex_state_dir()
        })
    }

    /// The gateway's listener: `[unex] listen`, else [`DEFAULT_UNEX_LISTEN`].
    pub fn unex_listen(&self) -> SocketAddr {
        self.value.unex.listen.unwrap_or(DEFAULT_UNEX_LISTEN)
    }

    /// The fewest bytes the reverse-proxy secret may hold: `[unex] proxy_secret_min_bytes`, else
    /// [`DEFAULT_PROXY_SECRET_MIN_BYTES`].
    pub fn proxy_secret_min_bytes(&self) -> u64 {
        self.value
            .unex
            .proxy_secret_min_bytes
            .unwrap_or(DEFAULT_PROXY_SECRET_MIN_BYTES)
    }

    /// What the file gives up against the defaults for the gateway, worded for a log line: each
    /// of its secrets read from the environment or written inline, and each `[registry]` or
    /// `[provider]` limit looser than its default.
    pub fn unex_weakenings(&self) -> Vec<String> {
        let mut weakened = Vec::new();
        for (what, secret) in [
            ("Station token", self.value.unex.station_token.as_ref()),
            (
                "reverse-proxy secret",
                self.value.unex.proxy_secret.as_ref(),
            ),
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
pub fn read_file<T: FromValue>(path: &Path) -> Result<T, BootstrapError> {
    Format::for_path(path)
        .map_err(|error| BootstrapError::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let text = std::fs::read_to_string(path).map_err(|error| {
        BootstrapError::new(error.kind(), format!("{}: {error}", path.display()))
    })?;
    format::read(path, &text).map_err(|error| BootstrapError::invalid(path, error))
}

/// Reads a PEM file of certificate authorities (`[unex] ca_file`, `ai6ctl --ca-file`): the DER of
/// each certificate in it, in file order. Anything else in the file is ignored.
///
/// # Errors
///
/// The I/O error's kind when the file cannot be read; `InvalidData` when its PEM is broken, it
/// holds no certificate, or a certificate cannot be parsed. Each message names `path`.
pub fn read_ca_file(path: &Path) -> Result<Vec<Vec<u8>>, BootstrapError> {
    let pem = std::fs::read(path).map_err(|error| {
        BootstrapError::new(error.kind(), format!("{}: {error}", path.display()))
    })?;
    let mut certificates = Vec::new();
    for certificate in CertificateDer::pem_slice_iter(&pem) {
        let certificate = certificate
            .map_err(|error| BootstrapError::invalid(path, format!("broken PEM: {error}")))?;
        // A TLS library would skip a certificate it cannot use as a root, which only shows
        // later as a refused connection.
        webpki::anchor_from_trusted_cert(&certificate).map_err(|error| {
            BootstrapError::invalid(path, format!("a certificate cannot be read: {error}"))
        })?;
        certificates.push(certificate.to_vec());
    }
    if certificates.is_empty() {
        return Err(BootstrapError::invalid(path, "holds no certificate"));
    }
    Ok(certificates)
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

    /// [`PluginSettings::keep_verified`], or its default.
    pub fn keep_verified(&self) -> KeepVerified {
        self.keep_verified.unwrap_or_default()
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
    /// As [`read_file`]; and `InvalidData` when `[unex] url` is empty.
    pub fn read(path: &Path) -> Result<Self, BootstrapError> {
        let mut value: Self = read_file(path)?;
        let dir = path.parent().unwrap_or(Path::new("."));
        value.provider.resolve(dir);
        value.registry.resolve(dir);
        let station = &mut value.station;
        station.state_dir = station.state_dir.take().map(|state| beside(dir, &state));
        station.identity.dir = station.identity.dir.take().map(|id| beside(dir, &id));
        station.secrets.dir = station.secrets.dir.take().map(|kept| beside(dir, &kept));
        station.plugins.resolve(dir);
        let unex = &mut value.unex;
        if unex.url.as_deref().is_some_and(|url| url.trim().is_empty()) {
            return Err(BootstrapError::invalid(path, "[unex] url is empty"));
        }
        unex.ca_file = unex.ca_file.take().map(|file| beside(dir, &file));
        unex.station_token = unex
            .station_token
            .take()
            .map(|token| token.relative_to(dir));
        unex.proxy_secret = unex
            .proxy_secret
            .take()
            .map(|secret| secret.relative_to(dir));
        unex.state_dir = unex.state_dir.take().map(|state| beside(dir, &state));
        unex.pages.dir = unex.pages.dir.take().map(|pages| beside(dir, &pages));
        unex.plugins.resolve(dir);
        Ok(value)
    }
}

impl Located<Bootstrap> {
    /// The Station's state directory: `[station] state_dir`, else [`platform_state_dir`].
    ///
    /// # Errors
    ///
    /// `NotFound` when neither is set and the platform's cannot be determined.
    pub fn state_dir(&self) -> Result<PathBuf, BootstrapError> {
        match &self.value.station.state_dir {
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

    /// The identity directory: `[station.identity] dir`, else the state directory.
    ///
    /// # Errors
    ///
    /// As [`state_dir`](Self::state_dir).
    pub fn identity_dir(&self) -> Result<PathBuf, BootstrapError> {
        match &self.value.station.identity.dir {
            Some(dir) => Ok(dir.clone()),
            None => self.state_dir(),
        }
    }

    /// The secrets directory: `[station.secrets] dir`, else `secrets` beside the bootstrap file,
    /// else in [`platform_config_dir`].
    ///
    /// # Errors
    ///
    /// `NotFound` when none can be determined.
    pub fn secrets_dir(&self) -> Result<PathBuf, BootstrapError> {
        if let Some(dir) = &self.value.station.secrets.dir {
            return Ok(dir.clone());
        }
        match self.dir() {
            Some(dir) => Ok(dir.join("secrets")),
            None => platform_config_dir().map(|dir| dir.join("secrets")),
        }
    }

    /// The Station's listener: `[station] listen`, else [`DEFAULT_LISTEN`].
    pub fn listen(&self) -> SocketAddr {
        self.value.station.listen.unwrap_or(DEFAULT_LISTEN)
    }

    /// What the file gives up against the defaults for a Station and the `ai6` command, worded for
    /// a log line: each secret that is read from the environment or written inline, and each
    /// `[registry]` or `[provider]` limit looser than its default.
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
            self.value.unex.station_token.as_ref(),
        );
        weakened.extend(limit_weakenings(&self.value.registry, &self.value.provider));
        if self.value.unex.ca_file.is_some() {
            weakened.push(
                "[unex] ca_file trusts its certificates for Universal Exports besides the public \
                 roots"
                    .to_owned(),
            );
        }
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
    use crate::directorates::m::assistant::AssistantSettings;
    use crate::directorates::m::config::{StationRuntime, UnexRecord};
    use crate::directorates::m::roster::RosterSettings;

    fn from_toml<T: FromValue>(text: &str) -> Result<T, String> {
        format::read(Path::new("a.toml"), text).map_err(|error| error.to_string())
    }

    #[test]
    fn a_secret_reference_reads_a_file_variable_or_inline_value_and_hides_it() {
        #[derive(FromValue)]
        struct Holder {
            s: SecretRef,
        }
        let secret = |text: &str| from_toml::<Holder>(text).map(|holder| holder.s);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("token"), "s3cret\r\n").unwrap();
        let file = secret("s = { file = \"token\" }")
            .unwrap()
            .relative_to(dir.path());
        assert_eq!(file.read().unwrap().as_str(), "s3cret");
        assert_eq!(file.weakening(), None);

        let inline = secret("s = \"hunter2\"").unwrap();
        assert_eq!(format!("{inline:?}"), "Inline(***)");
        assert!(inline.weakening().unwrap().contains("inline"));
        assert_eq!(inline.read().unwrap().as_str(), "hunter2");
        assert!(
            inline.to_value().is_err(),
            "an inline secret is never written out"
        );

        let env = secret("s = { env = \"AI6_TEST_NO_SUCH_VARIABLE\" }").unwrap();
        assert!(
            env.weakening()
                .unwrap()
                .contains("AI6_TEST_NO_SUCH_VARIABLE")
        );
        assert_eq!(env.read().unwrap_err().kind(), io::ErrorKind::NotFound);

        for refused in [
            "s = { file = \"hunter2\", env = \"B\" }",
            "s = { env = \"1hunter2\" }",
            "s = \"\"",
            "s = {}",
            "s = 12345",
        ] {
            let refusal = secret(refused).map(|_| ()).unwrap_err();
            assert!(refusal.contains("'s'"), "{refused}: {refusal}");
            assert!(!refusal.contains("hunter2"), "{refused}: {refusal}");
        }
    }

    fn located(text: &str) -> Located<Bootstrap> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, text).unwrap();
        Bootstrap::find(Some(&path)).unwrap()
    }

    #[test]
    fn the_gateway_reports_its_loosened_limits() {
        let located = located(
            "[provider]\naccess_policy_bytes = \"none\"\n[registry.limits.held]\nresults = 100\n",
        );
        assert_eq!(located.value.registry.limits.held.results, Limit::at(100));
        assert_eq!(
            located.unex_weakenings(),
            ["provider.access_policy_bytes none is looser than the default 1048576"]
        );
    }

    #[test]
    fn the_gateway_reads_the_proxy_secret_floor_and_reports_a_lower_one() {
        let default = located("");
        assert_eq!(default.proxy_secret_min_bytes(), 32);
        assert!(default.unex_weakenings().is_empty());
        let lowered = located("[unex]\nproxy_secret_min_bytes = 8\n");
        assert_eq!(lowered.proxy_secret_min_bytes(), 8);
        assert_eq!(
            lowered.unex_weakenings(),
            ["proxy_secret_min_bytes 8 is looser than the default 32"]
        );
    }

    /// Every place a person writes a value, as `(path, its schema, whether it must be written)`.
    fn writable<'a>(
        path: &str,
        node: &'a serde_json::Value,
        found: &mut Vec<(String, &'a serde_json::Value, bool)>,
    ) {
        let required = |key: &str| {
            node.get("required")
                .and_then(|keys| keys.as_array())
                .is_some_and(|keys| keys.iter().any(|name| name == key))
        };
        if let Some(properties) = node.get("properties").and_then(|p| p.as_object()) {
            for (key, field) in properties {
                let at = format!("{path}.{key}");
                found.push((at.clone(), field, required(key)));
                writable(&at, field, found);
            }
        }
        if let Some(arms) = node.get("anyOf").and_then(|arms| arms.as_array()) {
            for arm in arms {
                writable(path, arm, found);
            }
        }
    }

    fn schema_of<T: Schema>() -> serde_json::Value {
        let schema = T::schema().unwrap();
        serde_json::to_value(guatiao_serde::Serializable::from(&schema)).unwrap()
    }

    /// The fields of `T` a person writes with no title, and those they may leave out with neither
    /// a schema default nor a description saying what the default is.
    fn undeclared<T: Schema>() -> Vec<String> {
        let schema = schema_of::<T>();
        let mut found = Vec::new();
        writable("", &schema, &mut found);
        found
            .into_iter()
            .filter(|(_, field, required)| {
                let says = |key: &str, words: &str| {
                    field
                        .get(key)
                        .and_then(|text| text.as_str())
                        .is_some_and(|text| text.contains(words))
                };
                !says("title", "")
                    || !(*required
                        || field.get("default").is_some()
                        || says("description", "Default:"))
            })
            .map(|(path, _, _)| path)
            .collect()
    }

    #[test]
    fn every_section_a_person_writes_declares_its_titles_and_defaults() {
        let schema = schema_of::<Bootstrap>();
        let mut found = Vec::new();
        writable("", &schema, &mut found);
        // [registry.limits] alone holds about a hundred; a walk that stopped early finds nothing.
        assert!(found.len() > 150, "the walk reached {}", found.len());
        assert_eq!(undeclared::<Bootstrap>(), Vec::<String>::new());
        assert_eq!(undeclared::<StationRuntime>(), Vec::<String>::new());
        assert_eq!(undeclared::<UnexRecord>(), Vec::<String>::new());
        assert_eq!(undeclared::<AssistantSettings>(), Vec::<String>::new());
        assert_eq!(undeclared::<RosterSettings>(), Vec::<String>::new());
        let mut station = Vec::new();
        let station_schema = schema_of::<StationRuntime>();
        writable("", &station_schema, &mut station);
        // Twenty tables of a few keys each; a walk that stopped at the record's own keys is short.
        assert!(station.len() > 130, "the walk reached {}", station.len());

        // The check itself, on one field of each kind it refuses.
        #[derive(ToValue, FromValue, Schema)]
        struct Undeclared {
            /// Nothing says what this one defaults to.
            #[schema(title = "Titled")]
            no_default: Option<String>,
            /// Default: it has one, and no title.
            no_title: Option<String>,
            /// Default: it has both.
            #[schema(title = "Both")]
            both: Option<String>,
            /// It must be written, so it needs no default.
            #[schema(title = "Written")]
            written: String,
        }
        assert_eq!(
            undeclared::<Undeclared>(),
            [".no_default".to_owned(), ".no_title".to_owned()]
        );
    }

    #[test]
    fn no_setting_is_required_by_the_file_itself() {
        let schema = schema_of::<Bootstrap>();
        fn required(path: &str, node: &serde_json::Value, found: &mut Vec<String>) {
            if node.get("required").is_some_and(|keys| {
                keys.as_array().is_some_and(|keys| !keys.is_empty()) && !path.contains("oo")
            }) {
                found.push(path.to_owned());
            }
            if let Some(properties) = node.get("properties").and_then(|p| p.as_object()) {
                for (key, field) in properties {
                    required(&format!("{path}.{key}"), field, found);
                }
            }
        }
        let mut found = Vec::new();
        required("", &schema, &mut found);
        assert_eq!(found, Vec::<String>::new());
        Bootstrap::from_value(&format::parse(Path::new("a.toml"), "").unwrap()).unwrap();
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
