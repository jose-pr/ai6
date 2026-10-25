//! Guatiao provider discovery and construction for the daemon.

use core::fmt;
use std::mem::ManuallyDrop;
use std::path::PathBuf;

use guatiao::library::{Instance, Kind, KindMismatch, Order, Registry, SearchPath, scan_path_with};
use guatiao::{Status, Value};
use serde::Serialize;

use crate::directorates::m::access::{
    Authenticator, Authorizer, Directory, RoleAuthorizerProvider,
};
use crate::directorates::q::safehouse::SafeHouseProvider;
use crate::directorates::q::terminal::Terminal;
use crate::directorates::section_00::agent::AgentHandler;

guatiao::local_providers!(
    id = "ai6-built-in-access",
    version = "0.1.0",
    providers = [RoleAuthorizerProvider]
);

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
    /// Guatiao libraries that loaded.
    pub loaded: Vec<String>,
    /// Candidate files skipped by the kind filter or scanner.
    pub skipped: Vec<String>,
    /// Provider libraries that failed to load.
    pub failed: Vec<String>,
    /// Plugin paths or directories the daemon could not inspect.
    pub unreadable: Vec<String>,
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

/// The process-lifetime host for loaded AgentHandler, Terminal and SafeHouseProvider libraries.
pub struct Plugins {
    registry: ManuallyDrop<Registry>,
    summary: PluginScanSummary,
}

impl Plugins {
    /// Scans each directory or library path for AI6 handlers, access providers, terminals or safe houses.
    ///
    /// The registry stays mapped for the daemon lifetime because instances and sessions
    /// may call code from the loaded libraries after construction.
    pub fn scan(paths: &[PathBuf]) -> Self {
        let mut registry = Registry::new("ai6", env!("CARGO_PKG_VERSION"));
        registry
            .register_local("ai6-built-in-access", library)
            .expect("built-in access provider library must register");
        let mut search = SearchPath::new();
        for path in paths {
            search.push(path.clone());
        }
        let report = scan_path_with(&mut registry, &search, Order::Ascending, |declared| {
            if declared.kinds().any(|kind| {
                matches!(
                    kind,
                    "ai6_agent_handler"
                        | "ai6_terminal"
                        | "ai6_safehouse"
                        | "lineupmux_authenticator"
                        | "lineupmux_directory"
                        | "lineupmux_authorizer"
                )
            }) {
                Ok(())
            } else {
                Err("it declares no supported AI6 provider kind".to_string())
            }
        });
        let summary = PluginScanSummary {
            loaded: report
                .loaded
                .iter()
                .map(|(path, _)| path.display().to_string())
                .collect(),
            skipped: report
                .skipped
                .iter()
                .map(|(path, reason)| format!("{}: {reason:?}", path.display()))
                .collect(),
            failed: report
                .failed
                .iter()
                .map(|(path, reason)| format!("{}: {reason}", path.display()))
                .collect(),
            unreadable: report
                .unreadable
                .iter()
                .map(|(path, reason)| format!("{}: {reason}", path.display()))
                .collect(),
        };
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
        None => offer
            .boxed()
            .ok_or_else(|| PluginBuildError::ConfigurationRequired {
                provider: provider.to_owned(),
            }),
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn built_in_role_provider_is_registered_and_uses_its_config() {
        use crate::{AuthorizationRequest, Person, RoleAuthorizerConfig};
        use guatiao::ToValue;
        use std::collections::BTreeMap;

        let plugins = Plugins::scan(&[]);
        assert!(
            plugins
                .authorizers()
                .iter()
                .any(|provider| provider.id == "ai6_roles")
        );
        let config = RoleAuthorizerConfig {
            policy_toml: "[role.viewer]\nactions=['mission.read']\n[[rule]]\nsubject='account:007'\nscope='operation:G'\nrole='viewer'".to_owned(),
        }
        .to_value()
        .unwrap();
        let authorizer = plugins.authorizer("ai6_roles", Some(&config)).unwrap();
        let decision = authorizer
            .authorize(AuthorizationRequest {
                person: Person {
                    id: "person-007".to_owned(),
                    name: "007".to_owned(),
                    display_name: None,
                    email: None,
                    groups: Vec::new(),
                    attributes: BTreeMap::new(),
                },
                action: "mission.read".to_owned(),
                resource: "operation:G".to_owned(),
            })
            .unwrap();
        assert!(decision.allowed);
    }
}
