//! Guatiao provider discovery and construction for the daemon.

use core::fmt;
use std::collections::BTreeMap;
use std::env::consts::DLL_EXTENSION;
use std::fs::{File, OpenOptions, TryLockError};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use coqui_access_kit::{CacheOptions, Cached};
use coqui_peer::PeerId;
use guatiao::library::{
    Declared, Instance, Kind, KindMismatch, Order, Registry, SearchPath, bundle_binary, probe,
    scan_path_with,
};
use guatiao::{FromValue as _, Status, ToValue as _, Value};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use rustls_pki_types::PrivatePkcs8KeyDer;
use rustls_pki_types::pem::PemObject as _;
use serde::Serialize;
use sha2::Digest as _;

use crate::directorates::registry::bona_fides::{
    KeepVerified, LibraryPin, PinCheck, PinSet, PluginProfile, PublisherKey, library_pin,
    publisher_message,
};

use crate::directorates::m::access::{Accounts, Authenticator, Authorizer, Directory, KIND_PREFIX};
use crate::directorates::q::ephemeral::TheaterProvider;
use crate::directorates::q::safehouse::SafeHouseProvider;
use crate::directorates::q::secret_kind::SecretProvider;
use crate::directorates::q::terminal::{
    HarnessStdio, Terminal, TerminalOffer, TerminalRefusal, choose_terminal,
};
use crate::directorates::q::transport::Transport;
use crate::directorates::registry::sources::Source;
use crate::directorates::registry::tracker::Tracker;
use crate::directorates::section_00::agent::AgentHandler;

/// A loaded provider visible to the daemon.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderInfo {
    /// The provider ID configured by an Agent or Operation.
    pub id: String,
    /// A human-readable provider name.
    pub name: String,
    /// Why the provider is unavailable, if it reported a local restriction.
    pub unavailable: Option<String>,
}

/// What happened when the daemon scanned configured provider paths.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PluginScanSummary {
    /// The plugin profile the scan ran under: `strict`, `report` or `off`.
    pub profile: String,
    /// Guatiao libraries that loaded, by the path they were found at.
    pub loaded: Vec<String>,
    /// Candidate files skipped by the kind filter or scanner.
    pub skipped: Vec<String>,
    /// Provider libraries that failed to load.
    pub failed: Vec<String>,
    /// Plugin paths or directories the daemon could not inspect.
    pub unreadable: Vec<String>,
    /// Each AI6 provider library found, with its digest and what the pin set said.
    pub checked: Vec<LibraryCheck>,
    /// Libraries the `strict` profile did not load, and why.
    pub refused: Vec<String>,
    /// Every way the scan was looser than `strict` with every library pinned to its digest: the
    /// profile, each library that loaded unpinned or unmatched, SHA-1 pins, a publisher whose
    /// signature alone is enough.
    pub weakened: Vec<String>,
    /// Old verified copies this scan removed.
    pub verified_removed: Vec<String>,
    /// Old verified copies this scan left, with why: another process holds them, or they could
    /// not be removed. The next scan tries again.
    pub verified_held: Vec<String>,
}

/// One AI6 provider library a scan found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibraryCheck {
    /// Where it was found.
    pub path: String,
    /// Its `sha256:HEX` digest.
    pub digest: String,
    /// `accepted: pin sha256:..`, `accepted: publisher ed25519:..`, `unpinned` or
    /// `refused: <why>`.
    pub verdict: String,
    /// The verified copy it was handed to Guatiao as, under `strict`.
    pub loaded_from: Option<String>,
}

/// How [`Plugins::scan_with`] checks the provider libraries it finds.
///
/// Under [`PluginProfile::Strict`] each library is read once, checked against the pin set,
/// and loaded from a copy written into `verified_dir` under its digest, never from where it
/// was found; under `Report` every library loads from where it was found and each one the
/// pin set does not accept is a weakening; under `Off` nothing is checked. A `Strict` scan then
/// removes the copies in `verified_dir` it did not load, as [`KeepVerified`] says: never one a
/// scan elsewhere is about to load or a running process has loaded.
#[derive(Debug, Clone)]
pub struct PluginPolicy {
    profile: PluginProfile,
    pins: PinSet,
    verified_dir: Option<PathBuf>,
    keep_verified: KeepVerified,
}

impl Default for PluginPolicy {
    /// `report` with no pins: every library loads and each is listed as unpinned.
    fn default() -> Self {
        Self {
            profile: PluginProfile::Report,
            pins: PinSet::new(),
            verified_dir: None,
            keep_verified: KeepVerified::default(),
        }
    }
}

impl PluginPolicy {
    /// The policy for `pins` (`None` when the Station has no pin set), copying verified
    /// libraries into `verified_dir`. The profile is the pin set's own, else `strict` with
    /// a pin set and `report` without.
    pub fn new(pins: Option<PinSet>, verified_dir: impl Into<PathBuf>) -> Self {
        let profile = PluginProfile::default_for(pins.is_some());
        Self {
            profile,
            pins: pins.unwrap_or_default(),
            verified_dir: Some(verified_dir.into()),
            keep_verified: KeepVerified::default(),
        }
    }

    /// The policy for the pin set a command was given: `pins` when it names one, which must
    /// exist, else `default_pins`, which may be absent (no pin set: `report`). `profile`
    /// overrides the pin set's own; verified copies go to `verified_dir`.
    ///
    /// # Errors
    ///
    /// Returns an error when `pins` names a file that does not exist, or either file cannot
    /// be read or is not a valid pin set; the message names the file.
    pub fn load(
        pins: Option<&Path>,
        default_pins: Option<&Path>,
        profile: Option<PluginProfile>,
        verified_dir: impl Into<PathBuf>,
    ) -> std::io::Result<Self> {
        let named = |path: &Path, error: std::io::Error| {
            std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        };
        let set = match (pins, default_pins) {
            (Some(path), _) => match PinSet::load(path) {
                Ok(Some(set)) => Some(set),
                Ok(None) => {
                    return Err(named(
                        path,
                        std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "the named pin set does not exist",
                        ),
                    ));
                }
                Err(error) => return Err(named(path, error)),
            },
            (None, Some(path)) => PinSet::load(path).map_err(|error| named(path, error))?,
            (None, None) => None,
        };
        let policy = Self::new(set, verified_dir);
        Ok(match profile {
            Some(profile) => policy.with_profile(profile),
            None => policy,
        })
    }

    /// Uses `profile` whatever the pin set asks for.
    #[must_use]
    pub fn with_profile(mut self, profile: PluginProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Keeps old verified copies as `keep` says.
    #[must_use]
    pub fn with_keep_verified(mut self, keep: KeepVerified) -> Self {
        self.keep_verified = keep;
        self
    }

    /// The profile scans run under.
    pub fn profile(&self) -> PluginProfile {
        self.profile
    }

    /// Which old verified copies a `strict` scan keeps.
    pub fn keep_verified(&self) -> KeepVerified {
        self.keep_verified
    }

    /// The accepted libraries.
    pub fn pins(&self) -> &PinSet {
        &self.pins
    }
}

/// Why a configured provider could not be constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginBuildError {
    /// No loaded provider has this ID.
    NotFound { provider: String },
    /// The provider does not implement the requested AI6 kind.
    WrongKind { provider: String },
    /// The provider reported that it cannot run in this environment.
    Unavailable { provider: String, reason: String },
    /// This provider requires an explicit configuration value.
    ConfigurationRequired { provider: String },
    /// The provider rejected its configuration.
    InvalidConfiguration { provider: String, reason: String },
    /// The provider failed while constructing an instance.
    Refused { provider: String, reason: String },
}

impl fmt::Display for PluginBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { provider } => write!(f, "no provider `{provider}` is loaded"),
            Self::WrongKind { provider } => {
                write!(f, "provider `{provider}` does not implement this AI6 kind")
            }
            Self::Unavailable { provider, reason } => {
                write!(f, "provider `{provider}` is unavailable: {reason}")
            }
            Self::ConfigurationRequired { provider } => {
                write!(f, "provider `{provider}` requires configuration")
            }
            Self::InvalidConfiguration { provider, reason } => {
                write!(
                    f,
                    "provider `{provider}` rejected its configuration: {reason}"
                )
            }
            Self::Refused { provider, reason } => {
                write!(f, "provider `{provider}` refused construction: {reason}")
            }
        }
    }
}

impl core::error::Error for PluginBuildError {}

/// The process-lifetime host for loaded AgentHandler, Terminal, Transport, SafeHouseProvider,
/// Source and secret provider libraries, and the built-in secret providers.
pub struct Plugins {
    registry: ManuallyDrop<Registry>,
    summary: PluginScanSummary,
    /// The configuration each provider is built with when its caller gives none.
    defaults: BTreeMap<String, Value>,
    /// A shared lock on each verified copy the registry mapped, kept as long as the mapping.
    _held_copies: ManuallyDrop<Vec<File>>,
}

impl Plugins {
    /// Scans each directory or library path for AI6 handlers, access providers, terminals,
    /// transports, safe houses or sources under the default policy (`report`, no pins).
    ///
    /// The registry stays mapped for the daemon lifetime because instances and sessions
    /// may call code from the loaded libraries after construction.
    pub fn scan(paths: &[PathBuf]) -> Self {
        Self::scan_with(paths, &PluginPolicy::default())
    }

    /// Scans each directory or library path under `policy`; see [`PluginPolicy`] for what
    /// each profile loads and from where. A directory is not searched recursively.
    pub fn scan_with(paths: &[PathBuf], policy: &PluginPolicy) -> Self {
        let mut registry = Registry::new("ai6", env!("CARGO_PKG_VERSION"));
        register_builtins(&mut registry);
        let mut summary = PluginScanSummary {
            profile: policy.profile.as_str().to_owned(),
            weakened: policy.pins.weakenings(),
            ..PluginScanSummary::default()
        };
        let mut search = SearchPath::new();
        let mut found_at = BTreeMap::new();
        let verified = policy
            .verified_dir
            .as_deref()
            .filter(|_| policy.profile == PluginProfile::Strict);
        let scanning = verified.map(enter_verified);
        if policy.profile == PluginProfile::Off {
            summary
                .weakened
                .push("plugin profile is off: provider libraries load unchecked".to_owned());
            for path in paths {
                search.push(path.clone());
            }
        } else {
            for candidate in candidates(paths, &mut summary) {
                if let Some(load) = check_candidate(&candidate, policy, true, &mut summary) {
                    found_at.insert(load.clone(), candidate);
                    search.push(load);
                }
            }
        }
        let report = scan_path_with(&mut registry, &search, Order::Ascending, declares_ai6_kind);
        let shown = |path: &PathBuf| found_at.get(path).unwrap_or(path).display().to_string();
        summary
            .loaded
            .extend(report.loaded.iter().map(|(path, _)| shown(path)));
        summary.skipped.extend(
            report
                .skipped
                .iter()
                .map(|(path, reason)| format!("{}: {reason:?}", shown(path))),
        );
        summary.failed.extend(
            report
                .failed
                .iter()
                .map(|(path, reason)| format!("{}: {reason}", shown(path))),
        );
        summary.unreadable.extend(
            report
                .unreadable
                .iter()
                .map(|(path, reason)| format!("{}: {reason}", shown(path))),
        );
        let mut held_copies = Vec::new();
        if let (Some(directory), Some(scanning)) = (verified, scanning) {
            // Before the scan's own lock goes, so no clearing elsewhere can come between.
            held_copies.extend(
                report
                    .loaded
                    .iter()
                    .filter(|(path, _)| found_at.contains_key(path))
                    .filter_map(|(path, _)| hold_copy(path)),
            );
            clear_verified(
                directory,
                scanning,
                &found_at,
                policy.keep_verified,
                &mut summary,
            );
        }
        Self {
            registry: ManuallyDrop::new(registry),
            summary,
            defaults: BTreeMap::new(),
            _held_copies: ManuallyDrop::new(held_copies),
        }
    }

    /// Builds provider `<id>` with `configs[<id>]` when its caller gives no configuration of
    /// its own, as the `station` record's `[providers.<id>]` tables say.
    #[must_use]
    pub fn with_provider_configs(mut self, configs: BTreeMap<String, Value>) -> Self {
        self.defaults = configs;
        self
    }

    fn build<K>(&self, provider: &str, config: Option<&Value>) -> Result<Box<K>, PluginBuildError>
    where
        K: ?Sized + Kind,
        Box<K>: From<Instance<K>>,
    {
        build::<K>(
            &self.registry,
            provider,
            config.or_else(|| self.defaults.get(provider)),
        )
    }

    #[cfg(test)]
    pub(crate) fn from_registry(mut registry: Registry) -> Self {
        register_builtins(&mut registry);
        Self {
            registry: ManuallyDrop::new(registry),
            summary: PluginScanSummary::default(),
            defaults: BTreeMap::new(),
            _held_copies: ManuallyDrop::new(Vec::new()),
        }
    }

    /// Returns the AgentHandler providers and their reported availability.
    pub fn handlers(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn AgentHandler>::NAME)
    }

    /// Returns the terminal providers and their reported availability.
    pub fn terminals(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn Terminal>::NAME)
    }

    /// Returns the safe-house providers and their reported availability.
    pub fn safehouses(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn SafeHouseProvider>::NAME)
    }

    /// Returns the transport providers and their reported availability.
    pub fn transports(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn Transport>::NAME)
    }

    /// Returns the Theater providers (ephemeral Theaters) and their reported availability.
    pub fn theater_providers(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn TheaterProvider>::NAME)
    }

    /// Constructs a Theater provider from its ID and optional Guatiao config.
    pub fn theater_provider(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn TheaterProvider>, PluginBuildError> {
        self.build::<dyn TheaterProvider>(provider, config)
    }

    /// Returns the secret providers, the built-in `file`, `env` and `inline` among them, and
    /// their reported availability.
    pub fn secret_providers(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn SecretProvider>::NAME)
    }

    /// Constructs a secret provider from its ID and optional Guatiao config.
    pub fn secret_provider(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn SecretProvider>, PluginBuildError> {
        self.build::<dyn SecretProvider>(provider, config)
    }

    /// Whether a provider of any kind has the ID `id`.
    pub fn has_provider(&self, id: &str) -> bool {
        self.registry.providers_of(id).next().is_some()
    }

    /// The JSON Schema of provider `id`'s configuration, as its library describes it.
    pub fn config_schema(&self, id: &str) -> Option<serde_json::Value> {
        self.registry
            .providers_of(id)
            .find_map(|provider| provider.config_schema())
            .and_then(|schema| serde_json::to_value(guatiao_serde::Serializable::from(schema)).ok())
    }

    /// Returns the Source providers and their reported availability.
    pub fn sources(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn Source>::NAME)
    }

    /// Returns the Tracker providers and their reported availability.
    pub fn trackers(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn Tracker>::NAME)
    }

    /// Returns the configured credential authenticators.
    pub fn authenticators(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn Authenticator>::NAME)
    }

    /// Returns the configured identity directories.
    pub fn directories(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn Directory>::NAME)
    }

    /// Returns the configured authorization policy providers.
    pub fn authorizers(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn Authorizer>::NAME)
    }

    /// Returns the providers that manage their accounts, such as the local, key and peer ones.
    pub fn account_managers(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn Accounts>::NAME)
    }

    /// Constructs an account manager from its provider ID and optional Guatiao config.
    pub fn accounts(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Accounts>, PluginBuildError> {
        self.build::<dyn Accounts>(provider, config)
    }

    /// Constructs an AgentHandler from its provider ID and optional Guatiao config.
    pub fn handler(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn AgentHandler>, PluginBuildError> {
        self.build::<dyn AgentHandler>(provider, config)
    }

    /// Constructs an AgentHandler from its provider ID and an Agent's `handler_settings`;
    /// empty settings build it with its defaults. The settings go beside what the `station`
    /// record's `[providers.<id>]` table says, which they do not replace.
    pub fn handler_with(
        &self,
        provider: &str,
        settings: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Box<dyn AgentHandler>, PluginBuildError> {
        if settings.is_empty() {
            return self.handler(provider, None);
        }
        let invalid = |reason: String| PluginBuildError::InvalidConfiguration {
            provider: provider.to_owned(),
            reason,
        };
        let settings_toml = toml::to_string(settings)
            .map_err(|error| invalid(format!("handler_settings: {error}")))?;
        let config = crate::directorates::section_00::agent::HandlerConfig { settings_toml }
            .to_value()
            .map_err(|error| invalid(error.to_string()))?;
        let read = |value: &Value| {
            BTreeMap::<String, Value>::from_value(value).map_err(|error| invalid(error.to_string()))
        };
        let mut table = match self.defaults.get(provider) {
            Some(defaults) => read(defaults)?,
            None => BTreeMap::new(),
        };
        table.extend(read(&config)?);
        let config = table
            .to_value()
            .map_err(|error| invalid(error.to_string()))?;
        self.handler(provider, Some(&config))
    }

    /// The available terminal providers that build with their defaults, each with whether it
    /// gives a program a terminal ([`Terminal::tty`]).
    pub fn terminal_offers(&self) -> Vec<TerminalOffer> {
        self.terminals()
            .into_iter()
            .filter(|provider| provider.unavailable.is_none())
            .filter_map(|provider| {
                let tty = self.terminal(&provider.id, None).ok()?.tty();
                Some(TerminalOffer {
                    id: provider.id,
                    tty,
                })
            })
            .collect()
    }

    /// The terminal provider for a harness that talks over `stdio`, `pinned` or chosen among
    /// [`Plugins::terminal_offers`]; see [`choose_terminal`]. A pinned provider is built and
    /// asked [`Terminal::tty`] itself, whether or not it is among the offers; one that does not
    /// build is returned for the caller's construction to report.
    ///
    /// # Errors
    ///
    /// [`TerminalRefusal`] when no available provider suits the harness, or the pinned one does
    /// not.
    pub fn choose_terminal(
        &self,
        stdio: HarnessStdio,
        pinned: Option<&str>,
    ) -> Result<String, TerminalRefusal> {
        let mut offers = self.terminal_offers();
        if let Some(pinned) = pinned {
            offers.retain(|offer| offer.id != pinned);
            if let Ok(built) = self.terminal(pinned, None) {
                offers.push(TerminalOffer {
                    id: pinned.to_owned(),
                    tty: built.tty(),
                });
            }
        }
        choose_terminal(stdio, &offers, pinned)
    }

    /// Constructs a Terminal from its provider ID and optional Guatiao config.
    pub fn terminal(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Terminal>, PluginBuildError> {
        self.build::<dyn Terminal>(provider, config)
    }

    /// Constructs a SafeHouseProvider from its provider ID and optional Guatiao config.
    pub fn safehouse(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn SafeHouseProvider>, PluginBuildError> {
        self.build::<dyn SafeHouseProvider>(provider, config)
    }

    /// Constructs a Transport from its provider ID and optional Guatiao config.
    pub fn transport(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Transport>, PluginBuildError> {
        self.build::<dyn Transport>(provider, config)
    }

    /// Builds the transport a Theater's access settings name. Building never contacts the host.
    pub fn theater_transport(
        &self,
        access: &crate::directorates::m::theaters::TheaterAccess,
    ) -> Result<crate::directorates::q::transport::TransportClient, String> {
        use crate::directorates::m::theaters::TheaterAccess;
        use crate::directorates::q::transport::{TransportClient, TransportConfig};
        use guatiao::ToValue;
        let (provider, settings_toml) = access.transport().map_err(|error| error.to_string())?;
        let config = match access {
            TheaterAccess::Local => None,
            _ => Some(
                TransportConfig { settings_toml }
                    .to_value()
                    .map_err(|error| error.to_string())?,
            ),
        };
        self.transport(&provider, config.as_ref())
            .map(TransportClient::new)
            .map_err(|error| error.to_string())
    }

    /// Constructs a Source from its provider ID and Guatiao config.
    pub fn source(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Source>, PluginBuildError> {
        self.build::<dyn Source>(provider, config)
    }

    /// Constructs a Tracker from its provider ID and Guatiao config (a `SourceConfig`: the
    /// same settings table builds the Source).
    pub fn tracker(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Tracker>, PluginBuildError> {
        self.build::<dyn Tracker>(provider, config)
    }

    /// Constructs an Authenticator from its provider ID and optional Guatiao config.
    pub fn authenticator(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Authenticator>, PluginBuildError> {
        self.build::<dyn Authenticator>(provider, config)
    }

    /// Constructs a Directory from its provider ID and optional Guatiao config.
    pub fn directory(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Directory>, PluginBuildError> {
        self.build::<dyn Directory>(provider, config)
    }

    /// Constructs an Authorizer from its provider ID and optional Guatiao config.
    pub fn authorizer(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Authorizer>, PluginBuildError> {
        self.build::<dyn Authorizer>(provider, config)
    }

    /// Constructs a Directory, its lookups kept as `cache` says
    /// ([`CacheOptions::default`]: 5 s found, 1 s not found, 10 000 entries).
    pub fn cached_directory(
        &self,
        provider: &str,
        config: Option<&Value>,
        cache: CacheOptions,
    ) -> Result<Arc<Cached<dyn Directory>>, PluginBuildError> {
        let directory: Arc<dyn Directory> = Arc::from(self.directory(provider, config)?);
        Ok(Arc::new(Cached::new(directory, cache)))
    }

    /// Constructs an Authorizer, its decisions kept as `cache` says
    /// ([`CacheOptions::default`]: 5 s granted, 1 s denied, 10 000 entries).
    pub fn cached_authorizer(
        &self,
        provider: &str,
        config: Option<&Value>,
        cache: CacheOptions,
    ) -> Result<Arc<Cached<dyn Authorizer>>, PluginBuildError> {
        let authorizer: Arc<dyn Authorizer> = Arc::from(self.authorizer(provider, config)?);
        Ok(Arc::new(Cached::new(authorizer, cache)))
    }

    /// Returns the scan results for diagnosis when a provider was expected but is absent.
    pub fn scan_summary(&self) -> &PluginScanSummary {
        &self.summary
    }
}

/// Tells the operator, on standard error, what the scan refused and every way it was weaker
/// than `strict`.
pub fn log_scan(summary: &PluginScanSummary) {
    for refused in &summary.refused {
        eprintln!("ai6: plugin not loaded: {refused}");
    }
    for weakened in &summary.weakened {
        eprintln!("ai6: warning: {weakened}");
    }
    for removed in &summary.verified_removed {
        eprintln!("ai6: removed old verified copy {removed}");
    }
    for held in &summary.verified_held {
        eprintln!("ai6: old verified copy left: {held}");
    }
}

/// Registers the secret providers every program has, before any library is scanned, so a
/// library offering one of their IDs does not take it.
fn register_builtins(registry: &mut Registry) {
    if let Err(error) = registry.register_local(
        "ai6-secrets",
        crate::directorates::q::secret_kind::builtin::library,
    ) {
        eprintln!("ai6: the built-in secret providers did not register: {error}");
    }
    #[cfg(test)]
    registry
        .register_local(
            "ai6-fake-secrets",
            crate::directorates::q::secret_kind::testing::library,
        )
        .expect("the fake secret provider registers");
}

/// The filter a scan applies to what a library declares: one of AI6's own kinds.
fn declares_ai6_kind(declared: &Declared) -> Result<(), String> {
    if declared.kinds().any(|kind| {
        kind.starts_with(KIND_PREFIX)
            || matches!(
                kind,
                "ai6_agent_handler"
                    | "ai6_terminal"
                    | "ai6_safehouse"
                    | "ai6_transport"
                    | "ai6_theater_provider"
                    | "ai6_source"
                    | "ai6_tracker"
                    | "ai6_secret"
            )
    }) {
        Ok(())
    } else {
        Err("it declares no supported AI6 provider kind".to_string())
    }
}

/// The library files on `paths`, as Guatiao's scan finds them: a directory's files with the
/// platform's library extension and its bundles, in name order, and each file entry as given.
fn candidates(paths: &[PathBuf], summary: &mut PluginScanSummary) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in paths {
        let entry = bundle_binary(entry).unwrap_or_else(|| entry.clone());
        if entry.is_dir() {
            match std::fs::read_dir(&entry) {
                Ok(listing) => {
                    let mut files: Vec<PathBuf> = listing
                        .filter_map(Result::ok)
                        .map(|item| item.path())
                        .filter_map(|path| {
                            if path.is_file() {
                                path.extension()
                                    .is_some_and(|extension| {
                                        extension.eq_ignore_ascii_case(DLL_EXTENSION)
                                    })
                                    .then_some(path)
                            } else {
                                bundle_binary(&path)
                            }
                        })
                        .collect();
                    files.sort();
                    found.extend(files);
                }
                Err(error) => summary
                    .unreadable
                    .push(format!("{}: {error}", entry.display())),
            }
        } else if entry.is_file() {
            found.push(entry);
        } else {
            summary
                .unreadable
                .push(format!("{}: not found", entry.display()));
        }
    }
    found.dedup();
    found
}

/// Checks one candidate against the policy and returns the path to load it from, if it
/// loads: the verified copy under `strict`, the file itself otherwise. Files that are not
/// AI6 libraries are passed to Guatiao to report as it does. Without `copy` an accepted library
/// is only recorded, and nothing is returned or written.
fn check_candidate(
    candidate: &Path,
    policy: &PluginPolicy,
    copy: bool,
    summary: &mut PluginScanSummary,
) -> Option<PathBuf> {
    let shown = candidate.display().to_string();
    let not_ai6 = match probe(candidate) {
        Err(_) => Some("not a library this build can examine".to_owned()),
        Ok(probed) if !probed.entry => Some("no Guatiao entry symbol".to_owned()),
        Ok(probed) => declares_ai6_kind(&probed.declared).err(),
    };
    if let Some(reason) = not_ai6 {
        // Under strict, nothing unchecked reaches the loader, even a file that looked inert:
        // it could be replaced between this probe and Guatiao's.
        if policy.profile == PluginProfile::Strict {
            summary.skipped.push(format!("{shown}: {reason}"));
            return None;
        }
        return Some(candidate.to_path_buf());
    }
    let file = candidate
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bytes = match std::fs::read(candidate) {
        Ok(bytes) => bytes,
        Err(error) => {
            summary.failed.push(format!("{shown}: {error}"));
            return None;
        }
    };
    // Hashed once: the pin check reuses this digest instead of hashing the bytes again.
    let sha256: [u8; 32] = sha2::Sha256::digest(&bytes).into();
    let digest = PeerId::from_bytes(sha256);
    let signature = std::fs::read(candidate.with_file_name(format!("{file}.sig"))).ok();
    let check = policy.pins.check_digested(
        &file,
        &bytes,
        &sha256,
        signature.as_deref(),
        &verify_ed25519,
    );
    let verdict = match &check {
        PinCheck::Accepted(held) => format!("accepted: {held}"),
        PinCheck::Unpinned => "unpinned".to_owned(),
        PinCheck::Refused(why) => format!("refused: {why}"),
    };
    summary.checked.push(LibraryCheck {
        path: shown.clone(),
        digest: digest.to_string(),
        verdict,
        loaded_from: None,
    });
    match (policy.profile, check) {
        (PluginProfile::Strict, PinCheck::Accepted(_)) => {
            if !copy {
                return None;
            }
            let Some(directory) = &policy.verified_dir else {
                summary
                    .refused
                    .push(format!("{shown}: no directory for verified copies"));
                return None;
            };
            match copy_verified(directory, &digest, &bytes) {
                Ok(copy) => {
                    if let Some(check) = summary.checked.last_mut() {
                        check.loaded_from = Some(copy.display().to_string());
                    }
                    Some(copy)
                }
                Err(error) => {
                    summary.failed.push(format!(
                        "{shown}: the verified copy was not written: {error}"
                    ));
                    None
                }
            }
        }
        (PluginProfile::Strict, PinCheck::Unpinned) => {
            summary.refused.push(format!(
                "{shown}: {file} is not pinned ({digest}); `ai6 plugins pin` prints its entry"
            ));
            None
        }
        (PluginProfile::Strict, PinCheck::Refused(why)) => {
            summary.refused.push(format!("{shown}: {why}"));
            None
        }
        (_, PinCheck::Unpinned) => {
            summary.weakened.push(format!(
                "library {file} is not pinned ({digest}) and loads because the plugin profile is {}",
                policy.profile.as_str()
            ));
            Some(candidate.to_path_buf())
        }
        (_, PinCheck::Refused(why)) => {
            summary.weakened.push(format!(
                "library {file} is not accepted by its pin ({why}) and loads because the plugin profile is {}",
                policy.profile.as_str()
            ));
            Some(candidate.to_path_buf())
        }
        _ => Some(candidate.to_path_buf()),
    }
}

/// Writes `bytes` into `directory` as `sha256-HEX.EXT`, unless a file of that name already
/// holds them, and returns its path.
fn copy_verified(directory: &Path, digest: &PeerId, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    std::fs::create_dir_all(directory)?;
    let path = directory.join(format!("sha256-{}.{DLL_EXTENSION}", digest.hex()));
    if std::fs::read(&path).is_ok_and(|held| library_pin(&held) == *digest) {
        return Ok(path);
    }
    let mut copy = tempfile::NamedTempFile::new_in(directory)?;
    copy.write_all(bytes)?;
    copy.as_file().sync_all()?;
    copy.persist(&path).map_err(|error| error.error)?;
    Ok(path)
}

/// The lock file of a verified directory. A scan holds it shared from before its first copy until
/// its last load; clearing holds it exclusive, so a copy cannot go between being written and
/// being loaded.
const VERIFIED_LOCK: &str = ".lock";

/// Opens the lock of `directory`, creating both, and waits for any clearing to end. A clearing
/// holds it only while it deletes, and the system releases it when the holder dies.
fn enter_verified(directory: &Path) -> std::io::Result<File> {
    std::fs::create_dir_all(directory)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(VERIFIED_LOCK))?;
    lock.lock_shared()?;
    Ok(lock)
}

/// A shared lock on a copy this process loaded: what another process's clearing asks about, since
/// a mapped file cannot be relied on to refuse its deletion. `None` where no lock can be had.
fn hold_copy(copy: &Path) -> Option<File> {
    let file = File::open(copy).ok()?;
    file.lock_shared().ok()?;
    Some(file)
}

/// Removes the copies in `directory` this scan did not hand to Guatiao, past the newest `keep`.
/// While another scan is under way it removes nothing: the next scan clears them. A failure here
/// never fails the scan.
fn clear_verified(
    directory: &Path,
    scanning: std::io::Result<File>,
    used: &BTreeMap<PathBuf, PathBuf>,
    keep: KeepVerified,
    summary: &mut PluginScanSummary,
) {
    let KeepVerified::Newest(keep) = keep else {
        return;
    };
    let not_cleared = |error: std::io::Error| format!("{}: {error}", directory.display());
    let lock = match scanning {
        Ok(lock) => lock,
        Err(error) => {
            summary.verified_held.push(not_cleared(error));
            return;
        }
    };
    // A shared lock does not become exclusive in place: the copies this scan loaded are held on
    // their own, and any other scan that starts in the gap makes the attempt below fail.
    if let Err(error) = lock.unlock() {
        summary.verified_held.push(not_cleared(error));
        return;
    }
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return,
        Err(TryLockError::Error(error)) => {
            summary.verified_held.push(not_cleared(error));
            return;
        }
    }
    let listing = match std::fs::read_dir(directory) {
        Ok(listing) => listing,
        Err(error) => {
            summary.verified_held.push(not_cleared(error));
            return;
        }
    };
    let used: Vec<&std::ffi::OsStr> = used.keys().filter_map(|copy| copy.file_name()).collect();
    let mut unused: Vec<(std::time::SystemTime, PathBuf)> = listing
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| is_verified_copy(&entry.path()) && !used.contains(&&*entry.file_name()))
        .map(|entry| {
            let written = entry.metadata().and_then(|metadata| metadata.modified());
            (written.unwrap_or(std::time::UNIX_EPOCH), entry.path())
        })
        .collect();
    unused.sort_unstable_by(|a, b| b.cmp(a));
    let keep = usize::try_from(keep).unwrap_or(usize::MAX);
    for (_, copy) in unused.into_iter().skip(keep) {
        let shown = copy.display().to_string();
        match remove_copy(&copy) {
            Ok(()) => summary.verified_removed.push(shown),
            Err(error) => summary.verified_held.push(format!("{shown}: {error}")),
        }
    }
}

/// Whether `path` is named as [`copy_verified`] names a copy.
fn is_verified_copy(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == DLL_EXTENSION)
        && path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("sha256-"))
}

/// Removes a copy unless a process holds it ([`hold_copy`]).
fn remove_copy(copy: &Path) -> std::io::Result<()> {
    let file = File::open(copy)?;
    file.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => std::io::Error::other("another process holds it"),
        TryLockError::Error(error) => error,
    })?;
    std::fs::remove_file(copy)
}

/// Whether `signature` is `key`'s Ed25519 signature of `message`.
fn verify_ed25519(key: &PublisherKey, message: &[u8], signature: &[u8]) -> bool {
    UnparsedPublicKey::new(&ED25519, key.as_bytes())
        .verify(message, signature)
        .is_ok()
}

/// The file name a pin-set entry or a signature uses for the AI6 provider library at `path`.
fn provider_library_name(path: &Path) -> std::io::Result<String> {
    let probed = probe(path)?;
    if !probed.entry || declares_ai6_kind(&probed.declared).is_err() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not an AI6 provider library",
        ));
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))
}

/// The pin-set entry that accepts the AI6 provider library at `path` by its digest: what
/// `ai6 plugins pin PATH` prints.
///
/// # Errors
///
/// Returns an error when the file cannot be read or is not an AI6 provider library.
pub fn pin_entry(path: &Path) -> std::io::Result<LibraryPin> {
    let file = provider_library_name(path)?;
    Ok(LibraryPin::digest(file, library_pin(&std::fs::read(path)?)))
}

/// The pin-set entry that accepts whatever `publisher` signed for the name of the AI6 provider
/// library at `path`: what `ai6 plugins pin --publisher KEY PATH` prints.
///
/// # Errors
///
/// Returns an error when the file cannot be read or is not an AI6 provider library.
pub fn publisher_entry(path: &Path, publisher: PublisherKey) -> std::io::Result<LibraryPin> {
    Ok(LibraryPin::publisher(
        provider_library_name(path)?,
        publisher,
    ))
}

/// What a `strict` scan would say about the libraries on `paths` under `pins`, with nothing loaded
/// and no copy written: `checked`, `refused`, `skipped`, `failed`, `unreadable` and `weakened`.
pub fn check_libraries(paths: &[PathBuf], pins: &PinSet) -> PluginScanSummary {
    let policy = PluginPolicy {
        profile: PluginProfile::Strict,
        pins: pins.clone(),
        verified_dir: None,
        keep_verified: KeepVerified::default(),
    };
    let mut summary = PluginScanSummary {
        profile: policy.profile.as_str().to_owned(),
        weakened: policy.pins.weakenings(),
        ..PluginScanSummary::default()
    };
    for candidate in candidates(paths, &mut summary) {
        check_candidate(&candidate, &policy, false, &mut summary);
    }
    summary
}

/// A publisher's Ed25519 key: what signs provider libraries for a `publisher` entry. It belongs
/// on the host that builds them; a Station needs only [`PublisherSigner::public_key`].
#[derive(Debug)]
pub struct PublisherSigner(Ed25519KeyPair);

impl PublisherSigner {
    /// Makes a key and writes it to `path` as PKCS#8 DER: `0600` on Unix; on Windows the file
    /// takes its directory's access list. The key never leaves that file.
    ///
    /// # Errors
    ///
    /// `AlreadyExists` when `path` exists, which is left as it was; otherwise the filesystem's
    /// error, or the system's when its random source fails.
    pub fn generate(path: &Path) -> std::io::Result<Self> {
        let document = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .map_err(|_| std::io::Error::other("the system's random source failed"))?;
        coqui_peer::file::write_new(path, document.as_ref(), coqui_peer::file::Access::OwnerOnly)?;
        Self::from_der(document.as_ref())
    }

    /// Reads a PKCS#8 Ed25519 key, DER or PEM, from `path`: what [`PublisherSigner::generate`] or
    /// `openssl genpkey -algorithm ed25519` wrote.
    ///
    /// # Errors
    ///
    /// The filesystem's error, or `InvalidData` when it holds no such key. No message holds any
    /// of the file's text.
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let bytes = zeroize::Zeroizing::new(std::fs::read(path)?);
        if !bytes.starts_with(b"-----") {
            return Self::from_der(&bytes);
        }
        let der = PrivatePkcs8KeyDer::from_pem_slice(&bytes).map_err(|_| not_a_key())?;
        Self::from_der(der.secret_pkcs8_der())
    }

    fn from_der(der: &[u8]) -> std::io::Result<Self> {
        Ed25519KeyPair::from_pkcs8_maybe_unchecked(der)
            .map(Self)
            .map_err(|_| not_a_key())
    }

    /// The key a `publisher` entry names.
    ///
    /// # Panics
    ///
    /// Never: an Ed25519 public key is 32 bytes.
    pub fn public_key(&self) -> PublisherKey {
        let bytes: [u8; 32] = self
            .0
            .public_key()
            .as_ref()
            .try_into()
            .expect("an Ed25519 public key is 32 bytes");
        PublisherKey::from_bytes(bytes)
    }

    /// Signs the AI6 provider library at `path`, over [`publisher_message`] for its file name and
    /// SHA-256, and writes the signature to `FILE.sig` beside it, replacing any there. Returns
    /// that path.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or is not an AI6 provider library, or the
    /// signature cannot be written.
    pub fn sign(&self, path: &Path) -> std::io::Result<PathBuf> {
        let file = provider_library_name(path)?;
        let sha256: [u8; 32] = sha2::Sha256::digest(std::fs::read(path)?).into();
        let signature = self.0.sign(&publisher_message(&file, &sha256));
        let written = path.with_file_name(format!("{file}.sig"));
        coqui_peer::file::replace(
            &written,
            signature.as_ref(),
            coqui_peer::file::Access::Default,
        )?;
        Ok(written)
    }
}

fn not_a_key() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "not a PKCS#8 Ed25519 private key",
    )
}

fn providers(registry: &Registry, kind: &str) -> Vec<ProviderInfo> {
    registry
        .providers(kind)
        .map(|provider| ProviderInfo {
            id: provider.id().to_string(),
            name: provider.display_name().to_string(),
            unavailable: provider.available().err().map(str::to_string),
        })
        .collect()
}

/// Provider configurations as Guatiao values, from the JSON the `station` record holds.
///
/// # Errors
///
/// Names the provider whose configuration holds a number Guatiao cannot represent.
pub fn provider_configs(
    configs: &BTreeMap<String, serde_json::Value>,
) -> Result<BTreeMap<String, Value>, String> {
    configs
        .iter()
        .map(|(id, config)| {
            json_value(config)
                .map(|value| (id.clone(), value))
                .map_err(|reason| format!("providers.{id}: {reason}"))
        })
        .collect()
}

fn json_value(value: &serde_json::Value) -> Result<Value, String> {
    use guatiao::ToValue;
    use std::collections::VecDeque;
    let encoded = match value {
        serde_json::Value::Null => ().to_value(),
        serde_json::Value::Bool(value) => value.to_value(),
        serde_json::Value::Number(value) => guatiao::value::Number::new(&value.to_string())
            .map_err(|_| format!("{value} is not a number a provider can read"))?
            .to_value(),
        serde_json::Value::String(value) => value.to_value(),
        serde_json::Value::Array(values) => values
            .iter()
            .map(json_value)
            .collect::<Result<VecDeque<_>, _>>()?
            .to_value(),
        serde_json::Value::Object(values) => values
            .iter()
            .map(|(key, value)| json_value(value).map(|value| (key.clone(), value)))
            .collect::<Result<BTreeMap<_, _>, _>>()?
            .to_value(),
    };
    encoded.map_err(|error| error.to_string())
}

fn build<K>(
    registry: &Registry,
    provider: &str,
    config: Option<&Value>,
) -> Result<Box<K>, PluginBuildError>
where
    K: ?Sized + Kind,
    Box<K>: From<Instance<K>>,
{
    let offer = registry
        .offer::<K>(provider)
        .ok_or_else(|| PluginBuildError::NotFound {
            provider: provider.to_owned(),
        })?
        .map_err(|_: KindMismatch| PluginBuildError::WrongKind {
            provider: provider.to_owned(),
        })?;
    if let Err(reason) = offer.available() {
        return Err(PluginBuildError::Unavailable {
            provider: provider.to_owned(),
            reason: reason.to_string(),
        });
    }
    match config {
        Some(config) => offer
            .instantiate(config)
            .map(Box::<K>::from)
            .map_err(|error| {
                let reason = error.to_string();
                if error.status() == Status::GUATIAO_ERR_BAD_VALUE {
                    PluginBuildError::InvalidConfiguration {
                        provider: provider.to_owned(),
                        reason,
                    }
                } else {
                    PluginBuildError::Refused {
                        provider: provider.to_owned(),
                        reason,
                    }
                }
            }),
        None => {
            if let Some(instance) = offer.boxed() {
                return Ok(instance);
            }
            // A provider whose settings all have defaults is built with them.
            let defaults =
                guatiao::ToValue::to_value(&std::collections::BTreeMap::<String, String>::new());
            defaults
                .ok()
                .and_then(|defaults| {
                    let offer = registry.offer::<K>(provider)?.ok()?;
                    offer.instantiate(&defaults).ok()
                })
                .map(Box::<K>::from)
                .ok_or_else(|| PluginBuildError::ConfigurationRequired {
                    provider: provider.to_owned(),
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use guatiao::ToValue;

    use super::*;

    /// A terminal whose `tty` answer comes from its configuration, `true` without one.
    #[derive(guatiao::Provider)]
    #[provider(Terminal, id = "mock-x", name = "Configured mock", config = MockXConfig)]
    struct MockX {
        tty: bool,
    }

    #[derive(guatiao::ToValue, guatiao::FromValue, guatiao::Schema, Debug, Clone)]
    #[map(crate = ::guatiao)]
    struct MockXConfig {
        #[map(default = true)]
        tty: bool,
    }

    impl From<MockXConfig> for MockX {
        fn from(config: MockXConfig) -> Self {
            Self { tty: config.tty }
        }
    }

    impl Terminal for MockX {
        fn spawn(
            &self,
            _spec: &Value,
        ) -> Result<
            guatiao::library::Object<dyn crate::directorates::q::terminal::TerminalSession>,
            guatiao::library::ProviderError,
        > {
            Err(guatiao::library::ProviderError::new(
                Status::GUATIAO_ERR_INTERNAL,
                "starts nothing",
            ))
        }

        fn tty(&self) -> bool {
            self.tty
        }
    }

    mod mock_x {
        use super::*;

        crate::guatiao::local_providers!(id = "ai6-mock-x", version = "0.0.0", providers = [MockX]);
    }

    fn mock_x() -> Plugins {
        let mut registry = Registry::new("ai6-mock-x", env!("CARGO_PKG_VERSION"));
        registry
            .register_local("ai6-mock-x", mock_x::library)
            .expect("the mock registers");
        Plugins::from_registry(registry)
    }

    #[test]
    fn a_station_record_provider_table_configures_a_provider_built_with_none() {
        assert!(mock_x().terminal("mock-x", None).unwrap().tty());
        let record: BTreeMap<String, serde_json::Value> =
            crate::directorates::m::records::from_toml::<
                crate::directorates::m::config::StationRuntime,
            >(
                "[providers.mock-x]
tty = false
",
            )
            .unwrap()
            .providers;
        let configured = mock_x().with_provider_configs(provider_configs(&record).unwrap());
        assert!(!configured.terminal("mock-x", None).unwrap().tty());
        let own = MockXConfig { tty: true }.to_value().unwrap();
        assert!(configured.terminal("mock-x", Some(&own)).unwrap().tty());
    }

    #[test]
    fn construction_reports_a_missing_provider_by_kind() {
        let plugins = Plugins::scan(&[]);
        assert!(matches!(
            plugins.handler("claude", None),
            Err(PluginBuildError::NotFound { provider }) if provider == "claude"
        ));
        assert!(matches!(
            plugins.terminal("pty", None),
            Err(PluginBuildError::NotFound { provider }) if provider == "pty"
        ));
        assert!(matches!(
            plugins.safehouse("worktree", None),
            Err(PluginBuildError::NotFound { provider }) if provider == "worktree"
        ));
        assert!(matches!(
            plugins.transport("ssh", None),
            Err(PluginBuildError::NotFound { provider }) if provider == "ssh"
        ));
        assert!(matches!(
            plugins.source("github", None),
            Err(PluginBuildError::NotFound { provider }) if provider == "github"
        ));
        assert!(matches!(
            plugins.authenticator("password", None),
            Err(PluginBuildError::NotFound { provider }) if provider == "password"
        ));
        assert!(matches!(
            plugins.directory("people", None),
            Err(PluginBuildError::NotFound { provider }) if provider == "people"
        ));
        assert!(matches!(
            plugins.authorizer("roles", None),
            Err(PluginBuildError::NotFound { provider }) if provider == "roles"
        ));
    }

    /// The directory the test binary sits in: Cargo builds the access plugins that the
    /// dev-dependencies name (never used in code) beside it.
    fn plugin_directory() -> PathBuf {
        std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }

    fn text(value: &str) -> Value {
        value.to_owned().to_value().unwrap()
    }

    fn list(values: Vec<Value>) -> Value {
        std::collections::VecDeque::from(values).to_value().unwrap()
    }

    fn object(entries: Vec<(&str, Value)>) -> Value {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<std::collections::BTreeMap<String, Value>>()
            .to_value()
            .unwrap()
    }

    #[test]
    fn the_shared_access_plugins_load_under_their_ids() {
        let plugins = Plugins::scan(&[plugin_directory()]);
        let ids = |providers: Vec<ProviderInfo>| -> Vec<String> {
            providers.into_iter().map(|provider| provider.id).collect()
        };
        let authenticators = ids(plugins.authenticators());
        for id in [
            "coqui_access_password",
            "coqui_access_key",
            "coqui_access_ldap",
            "coqui_access_proxy",
            "coqui_access_mtls",
        ] {
            assert!(
                authenticators.iter().any(|loaded| loaded == id),
                "{id} in {authenticators:?}; {:?}",
                plugins.scan_summary()
            );
        }
        let directories = ids(plugins.directories());
        for id in [
            "coqui_access_password",
            "coqui_access_key",
            "coqui_access_ldap",
            "coqui_access_mtls",
        ] {
            assert!(directories.iter().any(|loaded| loaded == id), "{id}");
        }
        assert!(ids(plugins.authorizers()).contains(&"coqui_roles".to_owned()));
        assert!(ids(plugins.account_managers()).contains(&"coqui_access_password".to_owned()));
        assert!(plugins.scan_summary().failed.is_empty());
    }

    #[test]
    fn the_password_provider_reads_a_file_source_and_logs_a_person_in() {
        use crate::{AccountPut, Attempt, Verdict};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts.toml");
        let plugins = Plugins::scan(&[plugin_directory()]);
        let config = object(vec![
            (
                "source",
                object(vec![(
                    "file",
                    object(vec![
                        ("path", text(path.to_str().unwrap())),
                        ("format", text("toml")),
                    ]),
                )]),
            ),
            (
                "argon2",
                object(vec![
                    ("memory_kib", 64_u32.to_value().unwrap()),
                    ("iterations", 1_u32.to_value().unwrap()),
                ]),
            ),
        ]);
        let accounts = plugins
            .accounts("coqui_access_password", Some(&config))
            .unwrap();
        accounts
            .put(AccountPut {
                name: "007".to_owned(),
                secret: Some("correct horse".to_owned()),
                groups: Some(vec!["field".to_owned()]),
                ..AccountPut::default()
            })
            .unwrap();
        let authenticator = plugins
            .authenticator("coqui_access_password", Some(&config))
            .unwrap();
        let people = plugins
            .directory("coqui_access_password", Some(&config))
            .unwrap();
        let attempt = |secret: &str| Attempt {
            name: Some("007".to_owned()),
            secret: Some(secret.to_owned()),
            ..Attempt::default()
        };
        assert_eq!(
            authenticator.authenticate(attempt("wrong")).unwrap(),
            Verdict::Invalid
        );
        let Verdict::Valid(lookup) = authenticator
            .authenticate(attempt("correct horse"))
            .unwrap()
        else {
            panic!("the stored account should log in");
        };
        let person = people.lookup(lookup.clone()).unwrap().unwrap();
        assert_eq!(person.name, "007");
        assert_eq!(person.groups, ["field"]);

        // Loaded behind a cache, the directory answers a repeated lookup from memory until
        // the account changes.
        let cached = plugins
            .cached_directory(
                "coqui_access_password",
                Some(&config),
                CacheOptions::default(),
            )
            .unwrap();
        assert_eq!(cached.options(), CacheOptions::default());
        assert_eq!(cached.lookup(lookup.clone()).unwrap().unwrap().name, "007");
        accounts.remove("007".to_owned()).unwrap();
        assert!(
            cached.lookup(lookup.clone()).unwrap().is_some(),
            "kept for the cache's ttl"
        );
        cached.invalidate(&lookup);
        assert!(cached.lookup(lookup).unwrap().is_none());
    }

    /// A directory under the build's own output: a library loaded from it stays mapped for
    /// the test process, so Windows cannot remove it when the test ends, and `cargo clean`
    /// then does.
    fn scratch() -> tempfile::TempDir {
        let parent = plugin_directory().join("plugin-scratch");
        std::fs::create_dir_all(&parent).unwrap();
        tempfile::tempdir_in(parent).unwrap()
    }

    /// A directory holding one provider library, the roles plugin, as `roles.EXT`.
    fn one_library() -> (tempfile::TempDir, PathBuf, Vec<u8>) {
        let original = std::fs::read_dir(plugin_directory())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == DLL_EXTENSION)
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().contains("coqui_roles_plugin"))
            })
            .expect("the roles plugin is built beside the test binary");
        let directory = scratch();
        let library = directory.path().join(format!("roles.{DLL_EXTENSION}"));
        let bytes = std::fs::read(original).unwrap();
        std::fs::write(&library, &bytes).unwrap();
        (directory, library, bytes)
    }

    fn file_name() -> String {
        format!("roles.{DLL_EXTENSION}")
    }

    fn has_roles(plugins: &Plugins) -> bool {
        plugins
            .authorizers()
            .iter()
            .any(|provider| provider.id == "coqui_roles")
    }

    /// The library with its last byte changed: still a library a probe reads, other bytes.
    fn tamper(library: &Path) {
        let mut bytes = std::fs::read(library).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x5a;
        std::fs::write(library, bytes).unwrap();
    }

    #[test]
    fn a_pinned_library_loads_from_its_verified_copy() {
        let (directory, library, bytes) = one_library();
        let verified = scratch();
        let pins = PinSet::new().with_library(LibraryPin::digest(file_name(), library_pin(&bytes)));
        let policy = PluginPolicy::new(Some(pins), verified.path());
        assert_eq!(
            policy.profile(),
            PluginProfile::Strict,
            "a pin set defaults to strict"
        );

        let plugins = Plugins::scan_with(&[directory.path().to_path_buf()], &policy);
        let summary = plugins.scan_summary();
        assert!(has_roles(&plugins), "{summary:?}");
        assert!(
            summary.refused.is_empty() && summary.weakened.is_empty(),
            "{summary:?}"
        );
        let check = &summary.checked[0];
        assert_eq!(check.digest, library_pin(&bytes).to_string());
        assert!(
            check.verdict.starts_with("accepted: pin sha256:"),
            "{check:?}"
        );
        let copy = verified.path().join(format!(
            "sha256-{}.{DLL_EXTENSION}",
            library_pin(&bytes).hex()
        ));
        assert_eq!(check.loaded_from.as_deref(), copy.to_str());
        assert_eq!(std::fs::read(&copy).unwrap(), bytes);
        assert_eq!(summary.loaded, [library.display().to_string()]);
    }

    #[test]
    fn a_changed_byte_is_refused_under_strict_with_both_digests() {
        let (directory, library, bytes) = one_library();
        let verified = scratch();
        let pin = library_pin(&bytes);
        tamper(&library);
        let found = library_pin(&std::fs::read(&library).unwrap());
        let pins = PinSet::new().with_library(LibraryPin::digest(file_name(), pin));

        let plugins = Plugins::scan_with(
            &[directory.path().to_path_buf()],
            &PluginPolicy::new(Some(pins), verified.path()),
        );
        let summary = plugins.scan_summary();
        assert!(!has_roles(&plugins));
        assert_eq!(summary.refused.len(), 1, "{summary:?}");
        assert!(
            summary.refused[0].contains(&format!("expected {pin}, found {found}")),
            "{summary:?}"
        );
        assert!(summary.loaded.is_empty());
    }

    #[test]
    fn report_loads_an_unpinned_or_unmatched_library_and_lists_it() {
        let (directory, library, bytes) = one_library();
        let plugins = Plugins::scan(&[directory.path().to_path_buf()]);
        let summary = plugins.scan_summary();
        assert_eq!(summary.profile, "report");
        assert!(has_roles(&plugins));
        let digest = library_pin(&bytes).to_string();
        assert!(
            summary
                .weakened
                .iter()
                .any(|line| line.contains("not pinned") && line.contains(&digest)),
            "{summary:?}"
        );

        // A loaded library stays mapped (and on Windows, locked): change a fresh copy.
        let (changed, changed_library, _) = one_library();
        tamper(&changed_library);
        drop(library);
        let pins = PinSet::new().with_library(LibraryPin::digest(file_name(), library_pin(&bytes)));
        let verified = scratch();
        let policy =
            PluginPolicy::new(Some(pins), verified.path()).with_profile(PluginProfile::Report);
        let plugins = Plugins::scan_with(&[changed.path().to_path_buf()], &policy);
        assert!(has_roles(&plugins), "report still loads it");
        assert!(
            plugins
                .scan_summary()
                .weakened
                .iter()
                .any(|line| line.contains("not accepted by its pin")),
            "{:?}",
            plugins.scan_summary()
        );
        assert!(std::fs::read_dir(verified.path()).unwrap().next().is_none());
    }

    #[test]
    fn strict_refuses_an_unpinned_library_and_off_loads_it_as_a_weakening() {
        let (directory, _library, _bytes) = one_library();
        let verified = scratch();
        let strict = PluginPolicy::new(Some(PinSet::new()), verified.path());
        let plugins = Plugins::scan_with(&[directory.path().to_path_buf()], &strict);
        assert!(!has_roles(&plugins));
        assert!(plugins.scan_summary().refused[0].contains("is not pinned"));

        let off = strict.with_profile(PluginProfile::Off);
        let plugins = Plugins::scan_with(&[directory.path().to_path_buf()], &off);
        assert!(has_roles(&plugins));
        assert!(plugins.scan_summary().weakened[0].contains("off"));
    }

    fn publisher() -> (ring::signature::Ed25519KeyPair, PublisherKey) {
        use ring::signature::KeyPair;
        let random = ring::rand::SystemRandom::new();
        let document = ring::signature::Ed25519KeyPair::generate_pkcs8(&random).unwrap();
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(document.as_ref()).unwrap();
        let key = PublisherKey::from_bytes(pair.public_key().as_ref().try_into().unwrap());
        (pair, key)
    }

    fn sign(pair: &ring::signature::Ed25519KeyPair, library: &Path) {
        use sha2::{Digest, Sha256};
        let digest: [u8; 32] = Sha256::digest(std::fs::read(library).unwrap()).into();
        let file = library.file_name().unwrap().to_str().unwrap();
        let message = crate::directorates::registry::bona_fides::publisher_message(file, &digest);
        let mut signature = library.as_os_str().to_owned();
        signature.push(".sig");
        std::fs::write(signature, pair.sign(&message).as_ref()).unwrap();
    }

    #[test]
    fn a_signed_library_renamed_into_another_providers_slot_is_refused() {
        let (directory, library, bytes) = one_library();
        let (pair, key) = publisher();
        sign(&pair, &library);
        // The same bytes and signature, put where another provider's library goes.
        let other = format!("other.{DLL_EXTENSION}");
        let slot = scratch();
        std::fs::write(slot.path().join(&other), &bytes).unwrap();
        let mut signature = library.as_os_str().to_owned();
        signature.push(".sig");
        std::fs::copy(signature, slot.path().join(format!("{other}.sig"))).unwrap();
        drop(directory);
        let verified = scratch();
        let policy = PluginPolicy::new(
            Some(PinSet::new().with_library(LibraryPin::publisher(other, key))),
            verified.path(),
        );
        let plugins = Plugins::scan_with(&[slot.path().to_path_buf()], &policy);
        assert!(!has_roles(&plugins), "{:?}", plugins.scan_summary());
        assert!(
            plugins.scan_summary().refused[0]
                .contains(&format!("publisher {key}: the signature does not verify")),
            "{:?}",
            plugins.scan_summary()
        );
    }

    #[test]
    fn a_library_its_publisher_signed_loads_and_a_tampered_one_or_another_key_is_refused() {
        let (directory, library, _bytes) = one_library();
        let (pair, key) = publisher();
        sign(&pair, &library);
        let verified = scratch();
        let policy = |key: PublisherKey| {
            PluginPolicy::new(
                Some(PinSet::new().with_library(LibraryPin::publisher(file_name(), key))),
                verified.path(),
            )
        };
        let scan = |key| Plugins::scan_with(&[directory.path().to_path_buf()], &policy(key));

        let plugins = scan(key);
        assert!(has_roles(&plugins), "{:?}", plugins.scan_summary());
        assert_eq!(
            plugins.scan_summary().checked[0].verdict,
            format!("accepted: publisher {key}")
        );

        let (_, stranger) = publisher();
        let plugins = scan(stranger);
        assert!(!has_roles(&plugins));
        assert!(
            plugins.scan_summary().refused[0].contains(&format!(
                "publisher {stranger}: the signature does not verify"
            )),
            "{:?}",
            plugins.scan_summary()
        );

        tamper(&library);
        let plugins = scan(key);
        assert!(!has_roles(&plugins));
        assert!(
            plugins.scan_summary().refused[0]
                .contains(&format!("publisher {key}: the signature does not verify")),
            "{:?}",
            plugins.scan_summary()
        );
    }

    /// Bytes that still load as the roles plugin and hash differently: a rebuild, to a pin set.
    fn rebuilt(bytes: &[u8]) -> Vec<u8> {
        let mut rebuilt = bytes.to_vec();
        rebuilt.extend_from_slice(b"rebuilt");
        rebuilt
    }

    /// A fresh directory holding `bytes` as `roles.EXT`.
    fn holding(bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let directory = scratch();
        let library = directory.path().join(file_name());
        std::fs::write(&library, bytes).unwrap();
        (directory, library)
    }

    fn digest_policy(bytes: &[u8], verified: &Path) -> PluginPolicy {
        PluginPolicy::new(
            Some(PinSet::new().with_library(LibraryPin::digest(file_name(), library_pin(bytes)))),
            verified,
        )
    }

    fn copy_of(bytes: &[u8], verified: &Path) -> PathBuf {
        verified.join(format!(
            "sha256-{}.{DLL_EXTENSION}",
            library_pin(bytes).hex()
        ))
    }

    /// A copy nothing loads, written `hours` ago.
    fn old_copy(verified: &Path, name: &str, hours: u64) -> PathBuf {
        let copy = verified.join(format!("sha256-{name}.{DLL_EXTENSION}"));
        std::fs::write(&copy, name).unwrap();
        let written = std::time::SystemTime::now() - std::time::Duration::from_secs(hours * 3600);
        OpenOptions::new()
            .write(true)
            .open(&copy)
            .unwrap()
            .set_modified(written)
            .unwrap();
        copy
    }

    #[test]
    fn a_strict_scan_keeps_what_it_loaded_and_the_newest_and_removes_the_rest() {
        let (directory, _library, bytes) = one_library();
        let verified = scratch();
        let oldest = old_copy(verified.path(), "oldest", 50);
        let older = old_copy(verified.path(), "older", 40);
        let old = old_copy(verified.path(), "old", 30);
        let newer = old_copy(verified.path(), "newer", 20);
        let newest = old_copy(verified.path(), "newest", 10);
        let other_kind = verified.path().join("sha256-note.txt");
        let not_a_copy = verified.path().join(format!("kept.{DLL_EXTENSION}"));
        std::fs::write(&other_kind, "not a library").unwrap();
        std::fs::write(&not_a_copy, "not named as a copy").unwrap();

        let policy =
            digest_policy(&bytes, verified.path()).with_keep_verified(KeepVerified::Newest(2));
        let plugins = Plugins::scan_with(&[directory.path().to_path_buf()], &policy);
        let summary = plugins.scan_summary();

        assert!(has_roles(&plugins), "{summary:?}");
        assert!(copy_of(&bytes, verified.path()).exists(), "what it loaded");
        assert!(newest.exists() && newer.exists(), "{summary:?}");
        assert!(
            !old.exists() && !older.exists() && !oldest.exists(),
            "{summary:?}"
        );
        assert!(other_kind.exists() && not_a_copy.exists(), "not its files");
        let mut removed = summary.verified_removed.clone();
        removed.sort();
        let mut expected = [&old, &older, &oldest].map(|copy| copy.display().to_string());
        expected.sort();
        assert_eq!(removed, expected);
        assert!(summary.verified_held.is_empty(), "{summary:?}");
    }

    #[test]
    fn keep_zero_removes_every_unused_copy_and_none_removes_nothing() {
        let (directory, _library, bytes) = one_library();
        let verified = scratch();
        let stale = old_copy(verified.path(), "stale", 1);
        let scan = |keep| {
            let policy = digest_policy(&bytes, verified.path()).with_keep_verified(keep);
            Plugins::scan_with(&[directory.path().to_path_buf()], &policy)
        };

        let plugins = scan(KeepVerified::All);
        assert!(has_roles(&plugins));
        assert!(plugins.scan_summary().verified_removed.is_empty());
        assert!(stale.exists(), "none is no limit");

        let plugins = scan(KeepVerified::Newest(0));
        assert!(has_roles(&plugins), "{:?}", plugins.scan_summary());
        assert!(!stale.exists());
        assert!(
            copy_of(&bytes, verified.path()).exists(),
            "what it loaded stays at any count"
        );
    }

    #[test]
    fn only_a_strict_scan_clears_the_verified_directory() {
        let (directory, _library, bytes) = one_library();
        let verified = scratch();
        let stale = old_copy(verified.path(), "stale", 1);
        for profile in [PluginProfile::Report, PluginProfile::Off] {
            let policy = digest_policy(&bytes, verified.path())
                .with_keep_verified(KeepVerified::Newest(0))
                .with_profile(profile);
            let plugins = Plugins::scan_with(&[directory.path().to_path_buf()], &policy);
            assert!(has_roles(&plugins), "{profile:?}");
            assert!(stale.exists(), "{profile:?}");
        }
    }

    #[test]
    fn a_copy_another_process_holds_stays_without_failing_the_scan_and_goes_later() {
        let (directory, _library, bytes) = one_library();
        let verified = scratch();
        let held = old_copy(verified.path(), "held", 1);
        let holder = File::open(&held).unwrap();
        holder.lock_shared().unwrap();
        let scan = || {
            let policy =
                digest_policy(&bytes, verified.path()).with_keep_verified(KeepVerified::Newest(0));
            Plugins::scan_with(&[directory.path().to_path_buf()], &policy)
        };

        let plugins = scan();
        let summary = plugins.scan_summary();
        assert!(has_roles(&plugins), "{summary:?}");
        assert!(summary.failed.is_empty(), "{summary:?}");
        assert!(held.exists());
        assert_eq!(summary.verified_held.len(), 1, "{summary:?}");
        assert!(
            summary.verified_held[0].contains(&held.display().to_string())
                && summary.verified_held[0].contains("another process holds it"),
            "{summary:?}"
        );

        drop(holder);
        let plugins = scan();
        assert!(!held.exists(), "{:?}", plugins.scan_summary());
    }

    /// What holding a library looks like to a clearing on Windows beyond any lock: the system
    /// refuses to open or delete a file another handle shares nothing of.
    #[cfg(windows)]
    #[test]
    fn a_copy_open_without_delete_sharing_stays_without_failing_the_scan() {
        use std::os::windows::fs::OpenOptionsExt;
        let (directory, _library, bytes) = one_library();
        let verified = scratch();
        let held = old_copy(verified.path(), "held", 1);
        let holder = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&held)
            .unwrap();

        let policy =
            digest_policy(&bytes, verified.path()).with_keep_verified(KeepVerified::Newest(0));
        let plugins = Plugins::scan_with(&[directory.path().to_path_buf()], &policy);
        let summary = plugins.scan_summary();
        assert!(has_roles(&plugins), "{summary:?}");
        assert!(summary.failed.is_empty(), "{summary:?}");
        assert!(summary.verified_removed.is_empty(), "{summary:?}");
        assert!(
            summary.verified_held[0].contains(&held.display().to_string()),
            "{summary:?}"
        );
        drop(holder);
        assert!(held.exists());
    }

    #[test]
    fn a_loaded_copy_stays_held_so_a_later_clearing_leaves_it() {
        let (first, _library, bytes) = one_library();
        let verified = scratch();
        let plugins = Plugins::scan_with(
            &[first.path().to_path_buf()],
            &digest_policy(&bytes, verified.path()),
        );
        assert!(has_roles(&plugins), "{:?}", plugins.scan_summary());
        let loaded = copy_of(&bytes, verified.path());
        assert!(matches!(
            File::open(&loaded).unwrap().try_lock(),
            Err(TryLockError::WouldBlock)
        ));

        // A new build scanned by another process, which keeps nothing it did not load.
        let new = rebuilt(&bytes);
        let (second, _second_library) = holding(&new);
        let policy =
            digest_policy(&new, verified.path()).with_keep_verified(KeepVerified::Newest(0));
        let later = Plugins::scan_with(&[second.path().to_path_buf()], &policy);
        let summary = later.scan_summary();
        assert!(has_roles(&later), "{summary:?}");
        assert!(loaded.exists(), "{summary:?}");
        assert!(
            summary.verified_held[0].contains(&loaded.display().to_string()),
            "{summary:?}"
        );
        drop(plugins);
        assert!(
            matches!(
                File::open(&loaded).unwrap().try_lock(),
                Err(TryLockError::WouldBlock)
            ),
            "the mapping outlives the value, and so does the lock"
        );
    }

    #[test]
    fn a_scan_under_way_elsewhere_stops_the_clearing_until_it_ends() {
        let (directory, _library, bytes) = one_library();
        let verified = scratch();
        let stale = old_copy(verified.path(), "stale", 1);
        // Holds the lock as a scan does between its first copy and its last load.
        let elsewhere = enter_verified(verified.path()).unwrap();
        let scan = || {
            let policy =
                digest_policy(&bytes, verified.path()).with_keep_verified(KeepVerified::Newest(0));
            Plugins::scan_with(&[directory.path().to_path_buf()], &policy)
        };

        let plugins = scan();
        assert!(has_roles(&plugins), "{:?}", plugins.scan_summary());
        assert!(stale.exists(), "{:?}", plugins.scan_summary());

        drop(elsewhere);
        let plugins = scan();
        assert!(!stale.exists(), "{:?}", plugins.scan_summary());
    }

    #[test]
    fn a_scan_waits_for_a_clearing_under_way_and_then_loads() {
        let (directory, _library, bytes) = one_library();
        let verified = scratch();
        let clearing = enter_verified(verified.path()).unwrap();
        clearing.unlock().unwrap();
        clearing.try_lock().unwrap();

        let (done, finished) = std::sync::mpsc::channel();
        let policy = digest_policy(&bytes, verified.path());
        let paths = [directory.path().to_path_buf()];
        std::thread::spawn(move || {
            let plugins = Plugins::scan_with(&paths, &policy);
            done.send(has_roles(&plugins)).unwrap();
        });
        // It cannot finish while the lock is held, however fast the machine.
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_err()
        );
        assert!(
            !copy_of(&bytes, verified.path()).exists(),
            "no copy is written during a clearing"
        );

        drop(clearing);
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_secs(300))
                .unwrap()
        );
    }

    #[test]
    fn a_lock_that_cannot_be_taken_stops_the_clearing_and_not_the_scan() {
        let (directory, _library, bytes) = one_library();
        let verified = scratch();
        let stale = old_copy(verified.path(), "stale", 1);
        std::fs::create_dir(verified.path().join(VERIFIED_LOCK)).unwrap();

        let policy =
            digest_policy(&bytes, verified.path()).with_keep_verified(KeepVerified::Newest(0));
        let plugins = Plugins::scan_with(&[directory.path().to_path_buf()], &policy);
        let summary = plugins.scan_summary();
        assert!(has_roles(&plugins), "{summary:?}");
        assert!(stale.exists());
        assert!(
            summary.verified_held[0].contains(&verified.path().display().to_string()),
            "{summary:?}"
        );
    }

    #[test]
    fn a_publisher_entry_refuses_a_library_with_no_signature_under_strict() {
        let (directory, _library, _bytes) = one_library();
        let (_, key) = publisher();
        let verified = scratch();
        let policy = PluginPolicy::new(
            Some(PinSet::new().with_library(LibraryPin::publisher(file_name(), key))),
            verified.path(),
        );
        let plugins = Plugins::scan_with(&[directory.path().to_path_buf()], &policy);
        assert!(!has_roles(&plugins));
        assert!(
            plugins.scan_summary().refused[0]
                .contains(&format!("no signature file {}.sig", file_name())),
            "{:?}",
            plugins.scan_summary()
        );
    }

    #[test]
    fn a_rebuilt_library_loads_under_the_same_publisher_entry_and_both_refuses_it() {
        let (first, library, bytes) = one_library();
        let (pair, key) = publisher();
        sign(&pair, &library);
        let new = rebuilt(&bytes);
        let (second, new_library) = holding(&new);
        sign(&pair, &new_library);
        let verified = scratch();
        let scan = |directory: &Path, entry: LibraryPin| {
            let pins = PinSet::new().with_library(entry);
            Plugins::scan_with(
                &[directory.to_path_buf()],
                &PluginPolicy::new(Some(pins), verified.path()),
            )
        };
        let entry = LibraryPin::publisher(file_name(), key);

        let plugins = scan(first.path(), entry.clone());
        assert!(has_roles(&plugins), "{:?}", plugins.scan_summary());
        let plugins = scan(second.path(), entry);
        assert!(has_roles(&plugins), "{:?}", plugins.scan_summary());
        assert_eq!(
            plugins.scan_summary().checked[0].loaded_from.as_deref(),
            copy_of(&new, verified.path()).to_str()
        );

        // Asked for both, the same key's signature does not stand in for the pinned bytes.
        let both = LibraryPin::digest(file_name(), library_pin(&bytes))
            .with_publisher(key)
            .requiring_both();
        let plugins = scan(first.path(), both.clone());
        assert!(has_roles(&plugins), "{:?}", plugins.scan_summary());
        assert!(
            plugins.scan_summary().checked[0]
                .verdict
                .contains(" and publisher "),
            "{:?}",
            plugins.scan_summary()
        );
        assert!(plugins.scan_summary().weakened.is_empty());
        let plugins = scan(second.path(), both);
        assert!(!has_roles(&plugins));
        assert!(
            plugins.scan_summary().refused[0].contains(&format!(
                "expected {}, found {}",
                library_pin(&bytes),
                library_pin(&new)
            )),
            "{:?}",
            plugins.scan_summary()
        );
    }

    #[test]
    fn check_libraries_says_what_a_strict_scan_would_and_writes_nothing() {
        let (directory, library, bytes) = one_library();
        let pins = PinSet::new().with_library(LibraryPin::digest(file_name(), library_pin(&bytes)));
        let summary = check_libraries(&[directory.path().to_path_buf()], &pins);
        assert_eq!(summary.profile, "strict");
        assert_eq!(summary.checked.len(), 1, "{summary:?}");
        assert!(summary.checked[0].verdict.starts_with("accepted: pin "));
        assert_eq!(summary.checked[0].loaded_from, None);
        assert!(summary.loaded.is_empty() && summary.refused.is_empty());

        let summary = check_libraries(&[library], &PinSet::new());
        assert_eq!(summary.checked[0].verdict, "unpinned");
        assert!(summary.refused[0].contains("is not pinned"), "{summary:?}");
    }

    #[test]
    fn a_generated_key_refuses_to_overwrite_and_reads_back_as_the_same_key() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("publisher.key");
        let made = PublisherSigner::generate(&path).unwrap();
        let written = std::fs::read(&path).unwrap();
        assert_eq!(
            PublisherSigner::read(&path).unwrap().public_key(),
            made.public_key()
        );
        assert_eq!(
            PublisherSigner::generate(&path).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&path).unwrap(), written, "left as it was");
        assert!(!format!("{made:?}").contains(&format!("{written:?}")));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn what_a_signer_signs_is_what_the_verifier_accepts_and_only_for_that_name() {
        let (directory, library, _bytes) = one_library();
        let key_dir = tempfile::tempdir().unwrap();
        let signer = PublisherSigner::generate(&key_dir.path().join("publisher.key")).unwrap();
        let written = signer.sign(&library).unwrap();
        assert_eq!(
            written,
            directory.path().join(format!("{}.sig", file_name()))
        );
        assert_eq!(std::fs::read(&written).unwrap().len(), 64);

        let verified = scratch();
        let pins =
            PinSet::new().with_library(LibraryPin::publisher(file_name(), signer.public_key()));
        let plugins = Plugins::scan_with(
            &[directory.path().to_path_buf()],
            &PluginPolicy::new(Some(pins), verified.path()),
        );
        assert!(has_roles(&plugins), "{:?}", plugins.scan_summary());

        let not_a_library = library.with_file_name("notes.txt");
        std::fs::write(&not_a_library, "text").unwrap();
        assert!(signer.sign(&not_a_library).is_err());
        assert!(!library.with_file_name("notes.txt.sig").exists());
    }

    #[test]
    fn a_named_pin_set_must_exist_and_only_the_default_may_be_missing() {
        let directory = scratch();
        let missing = directory.path().join("plugins.toml");
        let verified = directory.path().join("verified");
        let error = PluginPolicy::load(Some(&missing), None, None, &verified).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(error.to_string().contains("plugins.toml"), "{error}");
        let policy = PluginPolicy::load(None, Some(&missing), None, &verified).unwrap();
        assert_eq!(policy.profile(), PluginProfile::Report);

        std::fs::write(&missing, "").unwrap();
        let policy = PluginPolicy::load(Some(&missing), None, None, &verified).unwrap();
        assert_eq!(policy.profile(), PluginProfile::Strict);
        std::fs::write(&missing, "[[library]]\nfile = \"a.so\"\n").unwrap();
        assert!(PluginPolicy::load(None, Some(&missing), None, &verified).is_err());
    }

    #[test]
    fn pin_entry_prints_the_sha256_pin_of_a_provider_library() {
        let (_directory, library, bytes) = one_library();
        let entry = pin_entry(&library).unwrap();
        assert_eq!(entry.file, file_name());
        assert_eq!(entry.pin, Some(library_pin(&bytes)));
        let text = toml::to_string(&PinSet::new().with_library(entry)).unwrap();
        assert!(text.contains("[[library]]"), "{text}");
        assert!(
            PinSet::from_toml(&text).is_ok(),
            "what it prints reads back: {text}"
        );
        let not_a_library = library.with_file_name("notes.txt");
        std::fs::write(&not_a_library, "text").unwrap();
        assert!(pin_entry(&not_a_library).is_err());
    }

    #[test]
    fn the_roles_plugin_takes_scope_kinds_from_its_config_and_refuses_others() {
        use crate::{AuthorizationRequest, Person};

        let plugins = Plugins::scan(&[plugin_directory()]);
        let policy = |scope: &str| {
            object(vec![
                ("scope_kinds", list(vec![text("operation")])),
                (
                    "roles",
                    list(vec![object(vec![
                        ("name", text("viewer")),
                        ("actions", list(vec![text("mission.read")])),
                    ])]),
                ),
                (
                    "rules",
                    list(vec![object(vec![
                        ("subject", text("account:007")),
                        ("scope", text(scope)),
                        ("role", text("viewer")),
                    ])]),
                ),
            ])
        };
        let authorizer = plugins
            .authorizer("coqui_roles", Some(&policy("operation:G")))
            .unwrap();
        let decision = authorizer
            .authorize(AuthorizationRequest {
                person: Person {
                    id: "person-007".to_owned(),
                    name: "007".to_owned(),
                    ..Person::default()
                },
                action: "mission.read".to_owned(),
                resource: "operation:G".to_owned(),
                ancestors: Vec::new(),
            })
            .unwrap();
        assert!(decision.allowed);
        assert!(matches!(
            plugins.authorizer("coqui_roles", Some(&policy("universe:all"))),
            Err(PluginBuildError::InvalidConfiguration { .. })
        ));
    }

    #[test]
    fn terminal_offers_say_which_providers_give_a_terminal() {
        let plugins = testing::terminal_choice(true);
        let mut offers = plugins.terminal_offers();
        offers.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(
            offers,
            [
                TerminalOffer {
                    id: "pipe".to_owned(),
                    tty: false
                },
                TerminalOffer {
                    id: "pty".to_owned(),
                    tty: true
                },
            ]
        );
    }

    /// The handler declares what its harness needs; the provider is chosen to suit it.
    #[test]
    fn a_handler_declares_its_stdio_and_gets_a_terminal_that_suits_it() {
        let both = testing::terminal_choice(true);
        let json = both.handler("json-agent", None).unwrap().stdio().unwrap();
        let interactive = both.handler("tty-agent", None).unwrap().stdio().unwrap();
        assert_eq!(
            (json, interactive),
            (HarnessStdio::Json, HarnessStdio::Terminal)
        );
        assert_eq!(both.choose_terminal(json, None).unwrap(), "pipe");
        assert_eq!(both.choose_terminal(interactive, None).unwrap(), "pty");
        assert_eq!(
            both.choose_terminal(json, Some("pty")),
            Err(TerminalRefusal::PinnedHasTerminal {
                provider: "pty".to_owned()
            })
        );

        let pty_only = testing::terminal_choice(false);
        assert_eq!(
            pty_only.choose_terminal(json, None),
            Err(TerminalRefusal::NoPipe)
        );
        assert_eq!(pty_only.choose_terminal(interactive, None).unwrap(), "pty");
        assert_eq!(
            pty_only.choose_terminal(json, Some("pty")),
            Err(TerminalRefusal::PinnedHasTerminal {
                provider: "pty".to_owned()
            }),
            "a pinned provider is asked whether it gives a terminal"
        );
        assert_eq!(
            pty_only.choose_terminal(json, Some("absent")).unwrap(),
            "absent",
            "one that does not build is left to the construction to report"
        );
    }
}

/// Terminal providers `pty` and `pipe`, and a JSON and an interactive handler, for the tests of
/// how a Station and a runner choose a terminal.
#[cfg(test)]
pub(crate) mod testing {
    use guatiao::Provider;
    use guatiao::library::{Object, ProviderError, Registry};

    use super::Plugins;
    use crate::directorates::q::terminal::{
        AgentTerminal, HarnessStdio, Terminal, TerminalSession,
    };
    use crate::directorates::section_00::agent::{AgentHandler, AgentResume, AgentStart};
    use coqui_acp_provider::AgentSession;

    /// What the mock providers answer when asked to start anything: the choice comes before.
    pub(crate) const REFUSED_START: &str = "the mock provider starts nothing";

    fn refused() -> ProviderError {
        ProviderError::new(guatiao::Status::GUATIAO_ERR_INTERNAL, REFUSED_START)
    }

    #[derive(Default, Provider)]
    #[provider(Terminal, id = "pty", name = "Mock pty", crate = crate::guatiao)]
    pub(crate) struct MockPty;

    impl Terminal for MockPty {
        fn spawn(
            &self,
            _spec: &guatiao::Value,
        ) -> Result<Object<dyn TerminalSession>, ProviderError> {
            Err(refused())
        }
    }

    #[derive(Default, Provider)]
    #[provider(Terminal, id = "pipe", name = "Mock pipe", crate = crate::guatiao)]
    pub(crate) struct MockPipe;

    impl Terminal for MockPipe {
        fn spawn(
            &self,
            _spec: &guatiao::Value,
        ) -> Result<Object<dyn TerminalSession>, ProviderError> {
            Err(refused())
        }

        fn tty(&self) -> bool {
            false
        }
    }

    #[derive(Default, Provider)]
    #[provider(AgentHandler, id = "json-agent", name = "Mock JSON handler", crate = crate::guatiao)]
    pub(crate) struct MockJsonHandler;

    impl AgentHandler for MockJsonHandler {
        fn start(
            &self,
            _request: AgentStart,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            Err(refused())
        }

        fn resume(
            &self,
            _request: AgentResume,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            Err(refused())
        }

        fn stdio(&self) -> Result<HarnessStdio, ProviderError> {
            Ok(HarnessStdio::Json)
        }
    }

    #[derive(Default, Provider)]
    #[provider(AgentHandler, id = "tty-agent", name = "Mock interactive handler", crate = crate::guatiao)]
    pub(crate) struct MockTtyHandler;

    impl AgentHandler for MockTtyHandler {
        fn start(
            &self,
            _request: AgentStart,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            Err(refused())
        }

        fn resume(
            &self,
            _request: AgentResume,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            Err(refused())
        }
    }

    /// A handler whose Session says `hello`, answers each message with `echo:MESSAGE` and
    /// completes when told `finish`.
    #[derive(Default, Provider)]
    #[provider(AgentHandler, id = "stream-agent", name = "Mock streaming handler", crate = crate::guatiao)]
    pub(crate) struct MockStreamHandler;

    pub(crate) struct StreamSession {
        id: crate::SessionId,
        events: std::sync::Mutex<std::collections::VecDeque<coqui_acp_provider::AgentEvent>>,
    }

    fn said(kind: &str, text: String) -> coqui_acp_provider::AgentEvent {
        coqui_acp_provider::AgentEvent {
            kind: kind.to_owned(),
            text: Some(text),
            data: None,
            limit: None,
            usage: None,
            tools: Vec::new(),
            quota: Vec::new(),
        }
    }

    impl StreamSession {
        fn new(id: &str, first: String) -> Self {
            Self {
                id: crate::SessionId::try_new(id).unwrap(),
                events: std::sync::Mutex::new(std::collections::VecDeque::from([said(
                    "text", first,
                )])),
            }
        }
    }

    impl AgentHandler for MockStreamHandler {
        fn start(
            &self,
            request: AgentStart,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            Ok(
                StreamSession::new("stream-session-1", format!("hello {}", request.prompt))
                    .into_object(),
            )
        }

        fn resume(
            &self,
            request: AgentResume,
            _terminal: Object<dyn AgentTerminal>,
        ) -> Result<Object<dyn AgentSession>, ProviderError> {
            Ok(StreamSession::new(request.session_id.as_str(), "resumed".to_owned()).into_object())
        }

        fn stdio(&self) -> Result<HarnessStdio, ProviderError> {
            Ok(HarnessStdio::Json)
        }
    }

    impl AgentSession for StreamSession {
        fn session_id(&self) -> Result<Option<String>, ProviderError> {
            Ok(Some(self.id.to_string()))
        }

        fn send_message(
            &self,
            message: coqui_acp_provider::AgentMessage,
        ) -> Result<(), ProviderError> {
            let event = if message.text == "finish" {
                said("completed", "finished".to_owned())
            } else {
                said("text", format!("echo:{}", message.text))
            };
            self.events.lock().unwrap().push_back(event);
            Ok(())
        }

        fn next_event(
            &self,
            timeout_ms: i64,
        ) -> Result<Option<coqui_acp_provider::AgentEvent>, ProviderError> {
            let next = self.events.lock().unwrap().pop_front();
            if next.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(
                    timeout_ms.clamp(0, 10) as u64
                ));
            }
            Ok(next)
        }

        fn interrupt(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn close(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    mod both {
        use super::*;

        crate::guatiao::local_providers!(
            id = "ai6-terminal-choice-both",
            version = "0.0.0",
            providers = [
                MockPty,
                MockPipe,
                MockJsonHandler,
                MockTtyHandler,
                MockStreamHandler
            ]
        );
    }

    mod pty_only {
        use super::*;

        crate::guatiao::local_providers!(
            id = "ai6-terminal-choice-pty",
            version = "0.0.0",
            providers = [MockPty, MockJsonHandler, MockTtyHandler]
        );
    }

    /// Both mock handlers, with `pty` and, when `with_pipe`, `pipe`.
    pub(crate) fn terminal_choice(with_pipe: bool) -> Plugins {
        let mut registry = Registry::new("ai6-terminal-choice", env!("CARGO_PKG_VERSION"));
        let registered = if with_pipe {
            registry
                .register_local("ai6-terminal-choice-both", both::library)
                .map(drop)
        } else {
            registry
                .register_local("ai6-terminal-choice-pty", pty_only::library)
                .map(drop)
        };
        registered.expect("mock providers should register");
        Plugins::from_registry(registry)
    }
}
