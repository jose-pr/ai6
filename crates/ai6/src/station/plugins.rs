//! Guatiao provider discovery and construction for the daemon.

use core::fmt;
use std::collections::BTreeMap;
use std::env::consts::DLL_EXTENSION;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use coqui_peer::PeerId;
use guatiao::library::{
    Declared, Instance, Kind, KindMismatch, Order, Registry, SearchPath, bundle_binary, probe,
    scan_path_with,
};
use guatiao::{Status, Value};
use serde::Serialize;
use sha2::Digest as _;

use crate::directorates::registry::bona_fides::{
    LibraryPin, PinCheck, PinSet, PluginProfile, PublisherKey, library_pin,
};

use crate::directorates::m::access::{Accounts, Authenticator, Authorizer, Directory, KIND_PREFIX};
use crate::directorates::q::ephemeral::TheaterProvider;
use crate::directorates::q::safehouse::SafeHouseProvider;
use crate::directorates::q::terminal::Terminal;
use crate::directorates::q::transport::Transport;
use crate::directorates::registry::sources::Source;
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
    /// Every way the scan was looser than `strict` with every library pinned: the profile,
    /// each library that loaded unpinned or unmatched, SHA-1 pins.
    pub weakened: Vec<String>,
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
/// pin set does not accept is a weakening; under `Off` nothing is checked.
#[derive(Debug, Clone)]
pub struct PluginPolicy {
    profile: PluginProfile,
    pins: PinSet,
    verified_dir: Option<PathBuf>,
}

impl Default for PluginPolicy {
    /// `report` with no pins: every library loads and each is listed as unpinned.
    fn default() -> Self {
        Self {
            profile: PluginProfile::Report,
            pins: PinSet::new(),
            verified_dir: None,
        }
    }
}

impl PluginPolicy {
    /// The policy for `pins` (`None` when the Station has no pin set), copying verified
    /// libraries into `verified_dir`. The profile is the pin set's own, else `strict` with
    /// a pin set and `report` without.
    pub fn new(pins: Option<PinSet>, verified_dir: impl Into<PathBuf>) -> Self {
        let profile = pins
            .as_ref()
            .map_or(PluginProfile::default_for(false), |pins| {
                pins.profile.unwrap_or(PluginProfile::default_for(true))
            });
        Self {
            profile,
            pins: pins.unwrap_or_default(),
            verified_dir: Some(verified_dir.into()),
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

    /// The profile scans run under.
    pub fn profile(&self) -> PluginProfile {
        self.profile
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

/// The process-lifetime host for loaded AgentHandler, Terminal, Transport, SafeHouseProvider and Source libraries.
pub struct Plugins {
    registry: ManuallyDrop<Registry>,
    summary: PluginScanSummary,
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
        let mut summary = PluginScanSummary {
            profile: policy.profile.as_str().to_owned(),
            weakened: policy.pins.weakenings(),
            ..PluginScanSummary::default()
        };
        let mut search = SearchPath::new();
        let mut found_at = BTreeMap::new();
        if policy.profile == PluginProfile::Off {
            summary
                .weakened
                .push("plugin profile is off: provider libraries load unchecked".to_owned());
            for path in paths {
                search.push(path.clone());
            }
        } else {
            for candidate in candidates(paths, &mut summary) {
                if let Some(load) = check_candidate(&candidate, policy, &mut summary) {
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
        Self {
            registry: ManuallyDrop::new(registry),
            summary,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_registry(registry: Registry) -> Self {
        Self {
            registry: ManuallyDrop::new(registry),
            summary: PluginScanSummary::default(),
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
        build::<dyn TheaterProvider>(&self.registry, provider, config)
    }

    /// Returns the Source providers and their reported availability.
    pub fn sources(&self) -> Vec<ProviderInfo> {
        providers(&self.registry, <dyn Source>::NAME)
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
        build::<dyn Accounts>(&self.registry, provider, config)
    }

    /// Constructs an AgentHandler from its provider ID and optional Guatiao config.
    pub fn handler(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn AgentHandler>, PluginBuildError> {
        build::<dyn AgentHandler>(&self.registry, provider, config)
    }

    /// Constructs a Terminal from its provider ID and optional Guatiao config.
    pub fn terminal(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Terminal>, PluginBuildError> {
        build::<dyn Terminal>(&self.registry, provider, config)
    }

    /// Constructs a SafeHouseProvider from its provider ID and optional Guatiao config.
    pub fn safehouse(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn SafeHouseProvider>, PluginBuildError> {
        build::<dyn SafeHouseProvider>(&self.registry, provider, config)
    }

    /// Constructs a Transport from its provider ID and optional Guatiao config.
    pub fn transport(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Transport>, PluginBuildError> {
        build::<dyn Transport>(&self.registry, provider, config)
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
        build::<dyn Source>(&self.registry, provider, config)
    }

    /// Constructs an Authenticator from its provider ID and optional Guatiao config.
    pub fn authenticator(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Authenticator>, PluginBuildError> {
        build::<dyn Authenticator>(&self.registry, provider, config)
    }

    /// Constructs a Directory from its provider ID and optional Guatiao config.
    pub fn directory(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Directory>, PluginBuildError> {
        build::<dyn Directory>(&self.registry, provider, config)
    }

    /// Constructs an Authorizer from its provider ID and optional Guatiao config.
    pub fn authorizer(
        &self,
        provider: &str,
        config: Option<&Value>,
    ) -> Result<Box<dyn Authorizer>, PluginBuildError> {
        build::<dyn Authorizer>(&self.registry, provider, config)
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
/// AI6 libraries are passed to Guatiao to report as it does.
fn check_candidate(
    candidate: &Path,
    policy: &PluginPolicy,
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

/// Whether `signature` is `key`'s Ed25519 signature of `message`.
fn verify_ed25519(key: &PublisherKey, message: &[u8], signature: &[u8]) -> bool {
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key.as_bytes())
        .verify(message, signature)
        .is_ok()
}

/// The pin-set entry that accepts the AI6 provider library at `path` by its digest: what
/// `ai6 plugins pin PATH` prints.
///
/// # Errors
///
/// Returns an error when the file cannot be read or is not an AI6 provider library.
pub fn pin_entry(path: &Path) -> std::io::Result<LibraryPin> {
    let probed = probe(path)?;
    if !probed.entry || declares_ai6_kind(&probed.declared).is_err() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not an AI6 provider library",
        ));
    }
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    Ok(LibraryPin::digest(file, library_pin(&std::fs::read(path)?)))
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
            "coqui_access_local",
            "coqui_access_key",
            "coqui_access_ldap",
            "coqui_access_proxy",
            "coqui_access_peer",
        ] {
            assert!(
                authenticators.iter().any(|loaded| loaded == id),
                "{id} in {authenticators:?}; {:?}",
                plugins.scan_summary()
            );
        }
        let directories = ids(plugins.directories());
        for id in [
            "coqui_access_local",
            "coqui_access_key",
            "coqui_access_ldap",
            "coqui_access_peer",
        ] {
            assert!(directories.iter().any(|loaded| loaded == id), "{id}");
        }
        assert!(ids(plugins.authorizers()).contains(&"coqui_roles".to_owned()));
        assert!(ids(plugins.account_managers()).contains(&"coqui_access_local".to_owned()));
        assert!(plugins.scan_summary().failed.is_empty());
    }

    #[test]
    fn the_local_provider_reads_a_file_source_and_logs_a_person_in() {
        use crate::{AccountPut, Attempt, Verdict};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts.toml");
        let plugins = Plugins::scan(&[plugin_directory()]);
        let config = object(vec![
            (
                "source",
                object(vec![(
                    "file",
                    object(vec![("path", text(path.to_str().unwrap()))]),
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
            .accounts("coqui_access_local", Some(&config))
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
            .authenticator("coqui_access_local", Some(&config))
            .unwrap();
        let people = plugins
            .directory("coqui_access_local", Some(&config))
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
        let person = people.lookup(lookup).unwrap().unwrap();
        assert_eq!(person.name, "007");
        assert_eq!(person.groups, ["field"]);
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
            })
            .unwrap();
        assert!(decision.allowed);
        assert!(matches!(
            plugins.authorizer("coqui_roles", Some(&policy("universe:all"))),
            Err(PluginBuildError::InvalidConfiguration { .. })
        ));
    }
}
