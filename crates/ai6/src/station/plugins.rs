//! Guatiao provider discovery and construction for the daemon.

use core::fmt;
use std::mem::ManuallyDrop;
use std::path::PathBuf;

use guatiao::library::{Instance, Kind, KindMismatch, Order, Registry, SearchPath, scan_path_with};
use guatiao::{Status, Value};
use serde::Serialize;

use crate::directorates::m::access::{Accounts, Authenticator, Authorizer, Directory, KIND_PREFIX};
use crate::directorates::q::safehouse::SafeHouseProvider;
use crate::directorates::q::terminal::Terminal;
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

/// The process-lifetime host for loaded AgentHandler, Terminal, SafeHouseProvider and Source libraries.
pub struct Plugins {
    registry: ManuallyDrop<Registry>,
    summary: PluginScanSummary,
}

impl Plugins {
    /// Scans each directory or library path for AI6 handlers, access providers, terminals, safe houses or sources.
    ///
    /// The registry stays mapped for the daemon lifetime because instances and sessions
    /// may call code from the loaded libraries after construction.
    pub fn scan(paths: &[PathBuf]) -> Self {
        let mut registry = Registry::new("ai6", env!("CARGO_PKG_VERSION"));
        let mut search = SearchPath::new();
        for path in paths {
            search.push(path.clone());
        }
        let report = scan_path_with(&mut registry, &search, Order::Ascending, |declared| {
            if declared.kinds().any(|kind| {
                kind.starts_with(KIND_PREFIX)
                    || matches!(
                        kind,
                        "ai6_agent_handler" | "ai6_terminal" | "ai6_safehouse" | "ai6_source"
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
