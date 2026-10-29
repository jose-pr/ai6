//! Theaters: where a Session's harness runs, and how a Station reaches each one.
//!
//! Each Station reads its own `theaters.toml`:
//!
//! ```toml
//! [theater.build-box]
//! access = { ssh = { host = "build.example.net", user = "ci", port = 22, host_key = "sha256:…" } }
//! labels = { os = "linux", gpu = "yes" }
//! agents = "auto"
//! egress = "direct"
//! harnesses = ["claude"]
//! runner = { command = "/opt/ai6/bin/ai6", plugin_paths = ["/opt/ai6/lib"] }
//! ```
//!
//! A Station's own host is always reachable as the implicit `local` Theater; declaring
//! `[theater.local]` adds labels or a runner to it.
//!
//! `access` names a transport, the way in: `"local"`, `{ ssh = { … } }`, or
//! `{ transport = { provider = "…", config = { … } } }` for any other transport provider.
//! `{ ephemeral = { provider = "…", config = { … } } }` makes the Theater a template: each
//! Mission bound to it gets a fresh Theater from a Theater provider (see
//! [`EphemeralAccess`]).
//! The transport reports what it can do (see
//! [`Capabilities`]). The other keys state
//! facts a transport cannot probe: `agents` says where the harness may run, `egress` how the
//! Theater reaches the provider's API, `harnesses` which harnesses are installed there,
//! `workdir` where a Mission works when its Operation does not name a path on that Theater,
//! and `runner` how to reach the `ai6 runner` that supervises harnesses there (see
//! [`RunnerSettings`]).
//!
//! `requires` says what the Theater must have before Sessions run there and `provision` how
//! to install it (see [`Requirements`] and [`ProvisionSpec`]); the Station probes the
//! Theater and publishes what it found, and `ai6 theater provision` fits it out.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::directorates::q::provision::{ProbeReport, ProvisionSpec, Requirements};
use crate::directorates::q::transport::Capabilities;

/// The Theater every Station reaches without configuration: its own host.
pub const LOCAL_THEATER: &str = "local";

const THEATERS_FILE: &str = "theaters.toml";

/// How a Station reaches a Theater.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TheaterAccess {
    /// The Station's own host.
    Local,
    /// A host reached over SSH.
    Ssh(SshAccess),
    /// A host reached by another transport provider.
    Transport(TransportAccess),
    /// A template: each Mission gets a fresh Theater made from it, destroyed afterwards.
    Ephemeral(EphemeralAccess),
}

fn toml_text(value: &impl Serialize, what: &str) -> Result<String, TheaterError> {
    toml::to_string(value).map_err(|error| TheaterError(format!("{what} settings: {error}")))
}

impl TheaterAccess {
    /// The transport provider that reaches the Theater and its settings as TOML text. An
    /// ephemeral template has none: the Theaters made from it do.
    pub fn transport(&self) -> Result<(String, String), TheaterError> {
        match self {
            Self::Local => Ok(("local".to_owned(), String::new())),
            Self::Ssh(ssh) => Ok(("ssh".to_owned(), toml_text(ssh, "transport")?)),
            Self::Transport(access) => Ok((
                access.provider.clone(),
                toml_text(&access.config, "transport")?,
            )),
            Self::Ephemeral(_) => Err(TheaterError(
                "an ephemeral Theater is made per Mission; reach the Theater made from it"
                    .to_owned(),
            )),
        }
    }
}

/// Theaters an ephemeral template keeps after their Mission failed, unless it says otherwise.
pub const DEFAULT_KEEP_FAILED: usize = 3;
/// Hours a kept Theater stays, unless its template says otherwise.
pub const DEFAULT_KEEP_FAILED_HOURS: u64 = 24;

/// A template for Theaters made fresh for each Mission by a Theater provider:
/// `access = { ephemeral = { provider = "container", config = { … } } }`.
///
/// A Theater made from it is destroyed once its Mission is done or abandoned. One whose
/// Mission failed is kept for inspection, at most `keep_failed` of them and for at most
/// `keep_failed_hours`, the oldest going first.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EphemeralAccess {
    /// The Theater provider ID.
    pub provider: String,
    /// The provider's settings; the provider validates them and refuses unknown keys.
    #[serde(default)]
    pub config: serde_json::Map<String, serde_json::Value>,
    /// How many failed Missions' Theaters to keep; [`DEFAULT_KEEP_FAILED`] when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_failed: Option<usize>,
    /// How long a kept Theater stays, in hours; [`DEFAULT_KEEP_FAILED_HOURS`] when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_failed_hours: Option<u64>,
}

impl EphemeralAccess {
    /// A template for `provider` with no settings and the default bounds.
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            config: serde_json::Map::new(),
            keep_failed: None,
            keep_failed_hours: None,
        }
    }

    /// Sets one provider setting.
    #[must_use]
    pub fn with_setting(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.config.insert(key.into(), value.into());
        self
    }

    /// Keeps at most `count` failed Missions' Theaters, each for at most `hours`.
    #[must_use]
    pub fn keeping_failed(mut self, count: usize, hours: u64) -> Self {
        self.keep_failed = Some(count);
        self.keep_failed_hours = Some(hours);
        self
    }

    /// The provider's settings as TOML text (a `TheaterProviderConfig`).
    pub fn settings_toml(&self) -> Result<String, TheaterError> {
        toml_text(&self.config, "Theater provider")
    }
}

/// A transport provider other than `local` and `ssh`, with its settings.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportAccess {
    /// The transport provider ID.
    pub provider: String,
    /// The provider's settings; the provider validates them and refuses unknown keys.
    #[serde(default)]
    pub config: serde_json::Map<String, serde_json::Value>,
}

impl TransportAccess {
    /// Settings for `provider` with no configuration.
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            config: serde_json::Map::new(),
        }
    }

    /// Sets one provider setting.
    #[must_use]
    pub fn with_setting(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.config.insert(key.into(), value.into());
        self
    }
}

/// SSH connection settings for a Theater.
///
/// The private key stays in a file on the Station: only its path is configured, never the
/// key or a passphrase.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshAccess {
    /// Host name or address.
    pub host: String,
    /// Remote user; the Station's own user name when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Remote port; 22 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Path of the private key file; `~/.ssh/id_ed25519`, `id_rsa` or `id_ecdsa` when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// The host's key as `sha256:HEX`, the SHA-256 of its public key blob. A connection
    /// whose key differs is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_key: Option<String>,
    /// Accept the host's key at first contact when none is pinned. Weaker than pinning, so
    /// it is reported; the key seen is published so it can be pinned afterwards.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trust_on_first_use: bool,
    /// Seconds to wait for the connection and login; 15 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_secs: Option<u64>,
}

impl SshAccess {
    /// Settings for `host` with every other option at its default.
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            user: None,
            port: None,
            identity: None,
            host_key: None,
            trust_on_first_use: false,
            connect_timeout_secs: None,
        }
    }

    /// Sets the remote user.
    #[must_use]
    pub fn with_user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Sets the remote port.
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Sets the private key file.
    #[must_use]
    pub fn with_identity(mut self, identity: impl Into<String>) -> Self {
        self.identity = Some(identity.into());
        self
    }

    /// Pins the host key (`sha256:HEX`).
    #[must_use]
    pub fn with_host_key(mut self, host_key: impl Into<String>) -> Self {
        self.host_key = Some(host_key.into());
        self
    }

    /// Accepts the host's key at first contact when none is pinned.
    #[must_use]
    pub fn trusting_on_first_use(mut self) -> Self {
        self.trust_on_first_use = true;
        self
    }

    /// Sets how long to wait for the connection and login.
    #[must_use]
    pub fn with_connect_timeout_secs(mut self, secs: u64) -> Self {
        self.connect_timeout_secs = Some(secs);
        self
    }
}

/// Where a Theater lets the agent harness run.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentMode {
    /// Field mode when the Theater can host the harness, remote-hands mode otherwise.
    #[default]
    Auto,
    /// The harness runs on the Theater; the Theater is refused when it cannot host it.
    Local,
    /// The harness runs on the Station and acts on the Theater through its transport.
    RemoteHands,
}

impl AgentMode {
    /// The name used in configuration and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Local => "local",
            Self::RemoteHands => "remote-hands",
        }
    }
}

/// How a Theater reaches the provider's API.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Egress {
    /// Straight to the internet.
    #[default]
    Direct,
    /// Through an HTTP proxy at this URL.
    Proxy(String),
    /// Through a CONNECT proxy the Station provides over the transport's `forward`.
    Station,
    /// Not at all: no internet, so the harness cannot run there.
    #[serde(rename = "none")]
    Isolated,
}

impl Egress {
    /// The URL-free kind, safe to publish.
    pub fn kind(&self) -> EgressKind {
        match self {
            Self::Direct => EgressKind::Direct,
            Self::Proxy(_) => EgressKind::Proxy,
            Self::Station => EgressKind::Station,
            Self::Isolated => EgressKind::Isolated,
        }
    }
}

/// An [`Egress`] without its URL, as a Station publishes it.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressKind {
    /// Straight to the internet.
    #[default]
    Direct,
    /// Through an HTTP proxy.
    Proxy,
    /// Through a Station-provided proxy.
    Station,
    /// Not at all.
    #[serde(rename = "none")]
    Isolated,
}

impl EgressKind {
    /// The name used in configuration and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Proxy => "proxy",
            Self::Station => "station",
            Self::Isolated => "none",
        }
    }
}

/// A runner on a Theater: the `ai6` binary in its runner mode supervises the harness there and
/// spools its events, so a Session outlives the Station that started it and a Station that
/// comes back (or another one) re-attaches and replays what it missed.
///
/// `runner = {}` uses every default.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerSettings {
    /// The `ai6` command on the Theater: `ai6` on its `PATH`, or the Station's own binary on
    /// the Station's own host, when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The runner's state directory on the Theater; the runner's own default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_dir: Option<String>,
    /// Directories or library files on the Theater that the runner loads providers from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugin_paths: Vec<String>,
    /// The terminal provider the runner uses; when absent, an available one that suits the
    /// handler (`pipe` for a JSON harness, else `pty`, preferred).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

impl RunnerSettings {
    /// Sets the `ai6` command on the Theater.
    #[must_use]
    pub fn with_command(mut self, command: impl Into<String>) -> Self {
        self.command = Some(command.into());
        self
    }

    /// Sets the runner's state directory.
    #[must_use]
    pub fn with_state_dir(mut self, state_dir: impl Into<String>) -> Self {
        self.state_dir = Some(state_dir.into());
        self
    }

    /// Adds a directory or library file the runner loads providers from.
    #[must_use]
    pub fn with_plugin_path(mut self, path: impl Into<String>) -> Self {
        self.plugin_paths.push(path.into());
        self
    }

    /// Pins the terminal provider the runner uses.
    #[must_use]
    pub fn with_terminal(mut self, terminal: impl Into<String>) -> Self {
        self.terminal = Some(terminal.into());
        self
    }
}

/// A host or sandbox where a Session's harness can run.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theater {
    /// Theater identifier, unique within one Station's configuration.
    pub id: String,
    /// How the Station reaches it.
    pub access: TheaterAccess,
    /// Labels an Operation's `station_labels` may match, overlaid on the Station's own.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    /// Where the harness may run.
    #[serde(default)]
    pub agents: AgentMode,
    /// How the Theater reaches the provider's API.
    #[serde(default)]
    pub egress: Egress,
    /// Harnesses (AgentHandler provider IDs) installed on the Theater.
    #[serde(default)]
    pub harnesses: Vec<String>,
    /// The directory a remote-hands Mission works in when its Operation names no path on
    /// this Theater; the transport's own default when absent.
    #[serde(default)]
    pub workdir: Option<String>,
    /// The runner that supervises harnesses on the Theater; without one, the Station
    /// supervises them itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<RunnerSettings>,
    /// What the Theater must have before Sessions run there; nothing is checked when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Requirements>,
    /// How `ai6 theater provision` installs what the Theater lacks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provision: Option<ProvisionSpec>,
}

impl Theater {
    /// A Theater with no labels and every other setting at its default.
    pub fn new(id: impl Into<String>, access: TheaterAccess) -> Self {
        Self {
            id: id.into(),
            access,
            labels: BTreeMap::new(),
            agents: AgentMode::default(),
            egress: Egress::default(),
            harnesses: Vec::new(),
            workdir: None,
            runner: None,
            requires: None,
            provision: None,
        }
    }

    /// Declares what the Theater must have before Sessions run there.
    #[must_use]
    pub fn with_requirements(mut self, requires: Requirements) -> Self {
        self.requires = Some(requires);
        self
    }

    /// Declares how to install what the Theater lacks.
    #[must_use]
    pub fn with_provision(mut self, provision: ProvisionSpec) -> Self {
        self.provision = Some(provision);
        self
    }

    /// Has a runner supervise the harnesses on the Theater.
    #[must_use]
    pub fn with_runner(mut self, runner: RunnerSettings) -> Self {
        self.runner = Some(runner);
        self
    }

    /// Adds or replaces a label.
    #[must_use]
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// Sets where the harness may run.
    #[must_use]
    pub fn with_agents(mut self, agents: AgentMode) -> Self {
        self.agents = agents;
        self
    }

    /// Sets how the Theater reaches the provider's API.
    #[must_use]
    pub fn with_egress(mut self, egress: Egress) -> Self {
        self.egress = egress;
        self
    }

    /// Declares a harness installed on the Theater.
    #[must_use]
    pub fn with_harness(mut self, harness: impl Into<String>) -> Self {
        self.harnesses.push(harness.into());
        self
    }

    /// Sets the directory a remote-hands Mission works in.
    #[must_use]
    pub fn with_workdir(mut self, workdir: impl Into<String>) -> Self {
        self.workdir = Some(workdir.into());
        self
    }

    /// The settings looser than their defaults, each in a sentence an operator can act on.
    pub fn weakenings(&self) -> Vec<String> {
        let mut weakened = Vec::new();
        if let TheaterAccess::Ssh(ssh) = &self.access
            && ssh.trust_on_first_use
            && ssh.host_key.is_none()
        {
            weakened.push(
                "trust_on_first_use: the SSH host key is accepted at first contact instead of pinned"
                    .to_owned(),
            );
        }
        if matches!(self.access, TheaterAccess::Local)
            && let Some(runner) = &self.runner
            && (runner.command.is_some() || runner.state_dir.is_some())
        {
            weakened.push(
                "runner.command or runner.state_dir: the configuration names a program or \
                 directory this Station uses on its own host"
                    .to_owned(),
            );
        }
        weakened
    }

    /// The non-secret description a Station publishes at check-in. The transport's
    /// capabilities are not known here; the Station adds them.
    pub fn summary(&self) -> TheaterSummary {
        let (access, host, provider) = match &self.access {
            TheaterAccess::Local => (TheaterKind::Local, None, None),
            TheaterAccess::Ssh(ssh) => (TheaterKind::Ssh, Some(ssh.host.clone()), None),
            TheaterAccess::Transport(transport) => (
                TheaterKind::Transport,
                None,
                Some(transport.provider.clone()),
            ),
            TheaterAccess::Ephemeral(template) => (
                TheaterKind::Ephemeral,
                None,
                Some(template.provider.clone()),
            ),
        };
        TheaterSummary {
            id: self.id.clone(),
            access,
            host,
            provider,
            labels: self.labels.clone(),
            capabilities: None,
            agents: self.agents,
            egress: self.egress.kind(),
            harnesses: self.harnesses.clone(),
            workdir: self.workdir.clone(),
            runner: self.runner.is_some(),
            unavailable: None,
            weakened: self.weakenings(),
            requires: self.requires.clone(),
            report: None,
            made_for: None,
            resources: None,
            sessions: 0,
        }
    }

    fn validate(&self) -> Result<(), TheaterError> {
        let id = &self.id;
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(TheaterError(format!(
                "Theater ID {id:?} must use 1 to 128 letters, digits, '.', '_' or '-'"
            )));
        }
        // A leading '-' would be read by a command-line client as an option.
        let word = |value: &str| {
            !value.is_empty()
                && !value.starts_with('-')
                && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        };
        match &self.access {
            TheaterAccess::Local => {}
            TheaterAccess::Ssh(ssh) => {
                if !word(&ssh.host) {
                    return Err(TheaterError(format!(
                        "Theater {id:?}: ssh host must be a non-empty word not starting with '-'"
                    )));
                }
                if ssh.user.as_deref().is_some_and(|user| !word(user)) {
                    return Err(TheaterError(format!(
                        "Theater {id:?}: ssh user must be a non-empty word not starting with '-'"
                    )));
                }
                if ssh.port == Some(0) {
                    return Err(TheaterError(format!(
                        "Theater {id:?}: ssh port must be 1 through 65535"
                    )));
                }
                if ssh.identity.as_deref().is_some_and(str::is_empty) {
                    return Err(TheaterError(format!(
                        "Theater {id:?}: ssh identity must not be empty"
                    )));
                }
                if ssh.connect_timeout_secs == Some(0) {
                    return Err(TheaterError(format!(
                        "Theater {id:?}: ssh connect_timeout_secs must be at least 1"
                    )));
                }
                if let Some(pin) = &ssh.host_key
                    && !is_sha256_pin(pin)
                {
                    return Err(TheaterError(format!(
                        "Theater {id:?}: ssh host_key must be \"sha256:\" and 64 hex digits"
                    )));
                }
            }
            TheaterAccess::Transport(transport) => {
                if !word(&transport.provider) {
                    return Err(TheaterError(format!(
                        "Theater {id:?}: transport provider must be a non-empty word not starting with '-'"
                    )));
                }
            }
            TheaterAccess::Ephemeral(template) => {
                if !word(&template.provider) {
                    return Err(TheaterError(format!(
                        "Theater {id:?}: ephemeral provider must be a non-empty word not starting with '-'"
                    )));
                }
                if id == LOCAL_THEATER {
                    return Err(TheaterError(
                        "the Theater `local` is the Station's own host and cannot be ephemeral"
                            .to_owned(),
                    ));
                }
            }
        }
        if self.harnesses.iter().any(|harness| !word(harness)) {
            return Err(TheaterError(format!(
                "Theater {id:?}: harnesses must be non-empty words"
            )));
        }
        match &self.egress {
            Egress::Proxy(url) if !word(url) => {
                return Err(TheaterError(format!(
                    "Theater {id:?}: egress proxy must be a URL"
                )));
            }
            _ => {}
        }
        if self.workdir.as_deref().is_some_and(str::is_empty) {
            return Err(TheaterError(format!(
                "Theater {id:?}: workdir must not be empty"
            )));
        }
        if let Some(runner) = &self.runner {
            let text = |value: &str| !value.is_empty() && !value.chars().any(char::is_control);
            if runner
                .command
                .as_deref()
                .is_some_and(|command| !text(command))
            {
                return Err(TheaterError(format!(
                    "Theater {id:?}: runner command must be non-empty text"
                )));
            }
            if runner.state_dir.as_deref().is_some_and(|dir| !text(dir))
                || runner.plugin_paths.iter().any(|path| !text(path))
            {
                return Err(TheaterError(format!(
                    "Theater {id:?}: runner paths must be non-empty text"
                )));
            }
            if runner
                .terminal
                .as_deref()
                .is_some_and(|terminal| !word(terminal))
            {
                return Err(TheaterError(format!(
                    "Theater {id:?}: runner terminal must be a provider ID"
                )));
            }
        }
        if let Some(requires) = &self.requires {
            requires
                .validate()
                .map_err(|error| TheaterError(format!("Theater {id:?}: {error}")))?;
        }
        if let Some(provision) = &self.provision {
            provision
                .validate()
                .map_err(|error| TheaterError(format!("Theater {id:?}: {error}")))?;
        }
        Ok(())
    }
}

/// Whether `pin` is `sha256:` followed by 64 hexadecimal digits.
pub fn is_sha256_pin(pin: &str) -> bool {
    pin.strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// The transport of a published Theater.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TheaterKind {
    /// The Station's own host.
    Local,
    /// A host reached over SSH.
    Ssh,
    /// A host reached by another transport provider; see [`TheaterSummary::provider`].
    Transport,
    /// A template whose Theaters a Theater provider makes per Mission; see
    /// [`TheaterSummary::provider`].
    Ephemeral,
}

impl TheaterKind {
    /// The name used in configuration and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Ssh => "ssh",
            Self::Transport => "transport",
            Self::Ephemeral => "ephemeral",
        }
    }
}

/// What a Station tells the Registry about one Theater it reaches: no user, port, key path,
/// proxy URL or transport settings.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TheaterSummary {
    /// Theater identifier.
    pub id: String,
    /// How the Station reaches it.
    pub access: TheaterKind,
    /// SSH host, for an `ssh` Theater.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The transport provider ID, for a `transport` Theater.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Theater labels.
    pub labels: BTreeMap<String, String>,
    /// What the transport can do; absent when the Station could not learn it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Capabilities>,
    /// Where the harness may run.
    pub agents: AgentMode,
    /// How the Theater reaches the provider's API.
    pub egress: EgressKind,
    /// Harnesses installed on the Theater.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<String>,
    /// The directory a remote-hands Mission works in when its Operation names no path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// A runner supervises harnesses on the Theater, so its Sessions outlive the Station.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub runner: bool,
    /// Why the Station cannot reach the Theater now: no transport provider, or the last
    /// contact failed. Absent when it can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// Settings looser than their defaults, and credentials found on the Theater outside
    /// AI6's leases.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weakened: Vec<String>,
    /// What the Theater must have before Sessions run there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Requirements>,
    /// What the Station's last probe of the Theater found; absent until one finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<ProbeReport>,
    /// The Mission an ephemeral Theater was made for; placement never chooses it for another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_for: Option<String>,
    /// The Theater's CPUs, load and memory as last sampled: the Station's own for its host,
    /// else the Station's probe of it. Absent until a sample succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<crate::directorates::m::placement::Resources>,
    /// Sessions the Station is running on the Theater.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub sessions: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

impl TheaterSummary {
    /// The implicit `local` Theater, which carries no labels of its own.
    pub fn local() -> Self {
        Theater::new(LOCAL_THEATER, TheaterAccess::Local).summary()
    }
}

/// The Theaters a Station declares. Its own host is always reachable as `local` as well.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Theaters {
    declared: BTreeMap<String, Theater>,
}

/// `theaters.toml` could not be read or holds an invalid Theater.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TheaterError(String);

impl std::fmt::Display for TheaterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TheaterError {}

impl Theaters {
    /// Loads `theaters.toml` from a configuration directory. A missing file declares
    /// nothing; an invalid file fails.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, TheaterError> {
        let path = root.as_ref().join(THEATERS_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(TheaterError(format!("{}: {error}", path.display()))),
        };
        Self::parse(&text).map_err(|error| TheaterError(format!("{}: {error}", path.display())))
    }

    /// Parses the text of a `theaters.toml`.
    pub fn parse(text: &str) -> Result<Self, TheaterError> {
        let document: toml::Table =
            toml::from_str(text).map_err(|e| TheaterError(e.to_string()))?;
        Self::from_document(toml::Value::Table(document))
    }

    /// The Theaters of `theater` records, each the keys of a `[theater.<id>]` table, as a
    /// configuration provider holds them.
    pub(crate) fn from_records(
        records: impl IntoIterator<Item = (String, serde_json::Value)>,
    ) -> Result<Self, TheaterError> {
        let theater: serde_json::Map<String, serde_json::Value> = records.into_iter().collect();
        Self::from_document(serde_json::json!({ "theater": theater }))
    }

    fn from_document<'de, D>(document: D) -> Result<Self, TheaterError>
    where
        D: serde::Deserializer<'de>,
        D::Error: std::fmt::Display,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Document {
            #[serde(default)]
            theater: BTreeMap<String, Definition>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Definition {
            access: TheaterAccess,
            #[serde(default)]
            labels: BTreeMap<String, String>,
            #[serde(default)]
            agents: AgentMode,
            #[serde(default)]
            egress: Egress,
            #[serde(default)]
            harnesses: Vec<String>,
            #[serde(default)]
            workdir: Option<String>,
            #[serde(default)]
            runner: Option<RunnerSettings>,
            #[serde(default)]
            requires: Option<Requirements>,
            #[serde(default)]
            provision: Option<ProvisionSpec>,
        }
        let document = Document::deserialize(document).map_err(|e| TheaterError(e.to_string()))?;
        let mut theaters = Self::default();
        for (id, definition) in document.theater {
            theaters.insert(Theater {
                id,
                access: definition.access,
                labels: definition.labels,
                agents: definition.agents,
                egress: definition.egress,
                harnesses: definition.harnesses,
                workdir: definition.workdir,
                runner: definition.runner,
                requires: definition.requires,
                provision: definition.provision,
            })?;
        }
        Ok(theaters)
    }

    /// Declares a Theater, replacing one with the same ID.
    pub fn insert(&mut self, theater: Theater) -> Result<(), TheaterError> {
        theater.validate()?;
        self.declared.insert(theater.id.clone(), theater);
        Ok(())
    }

    /// Declares every Theater of `other`, which wins on a shared ID.
    pub fn merge(&mut self, other: Theaters) {
        self.declared.extend(other.declared);
    }

    /// Every Theater the Station reaches: those declared, plus the implicit `local` unless
    /// it was declared.
    pub fn reachable(&self) -> Vec<Theater> {
        let mut theaters = self.declared.clone();
        theaters
            .entry(LOCAL_THEATER.to_owned())
            .or_insert_with(|| Theater::new(LOCAL_THEATER, TheaterAccess::Local));
        theaters.into_values().collect()
    }

    /// One reachable Theater by ID.
    pub fn find(&self, id: &str) -> Option<Theater> {
        self.reachable()
            .into_iter()
            .find(|theater| theater.id == id)
    }

    /// The reachable Theaters as published at check-in, without transport capabilities.
    pub fn summaries(&self) -> Vec<TheaterSummary> {
        self.reachable().iter().map(Theater::summary).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_is_implicit_and_declarable() {
        let theaters = Theaters::default();
        assert_eq!(theaters.summaries(), vec![TheaterSummary::local()]);

        let theaters =
            Theaters::parse("[theater.local]\naccess = \"local\"\nlabels = { zone = \"a\" }\n")
                .unwrap();
        let all = theaters.summaries();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].labels["zone"], "a");
    }

    #[test]
    fn parses_ssh_theaters_and_publishes_only_the_host() {
        let theaters = Theaters::parse(
            "[theater.box]\naccess = { ssh = { host = \"b.example\", user = \"ci\", port = 2222, identity = \"~/.ssh/k\" } }\nlabels = { os = \"linux\" }\n",
        )
        .unwrap();
        let remote = theaters.find("box").unwrap();
        let TheaterAccess::Ssh(ssh) = &remote.access else {
            panic!("expected ssh access");
        };
        assert_eq!(ssh.user.as_deref(), Some("ci"));
        assert_eq!(ssh.port, Some(2222));
        assert_eq!(
            serde_json::to_value(remote.summary()).unwrap(),
            serde_json::json!({
                "id": "box", "access": "ssh", "host": "b.example", "labels": {"os": "linux"},
                "agents": "auto", "egress": "direct"
            })
        );
        assert_eq!(theaters.summaries().len(), 2);
    }

    #[test]
    fn declared_facts_default_and_are_published_without_secrets() {
        let theaters = Theaters::parse(
            r#"
[theater.phone]
access = { transport = { provider = "adb", config = { serial = "emulator-5554" } } }
agents = "remote-hands"
workdir = "/sdcard/work"

[theater.lab]
access = { ssh = { host = "lab.example" } }
egress = { proxy = "http://user:secret@proxy.example:3128" }
harnesses = ["claude", "codex"]
"#,
        )
        .unwrap();
        let phone = theaters.find("phone").unwrap();
        assert_eq!(phone.agents, AgentMode::RemoteHands);
        assert_eq!(phone.egress, Egress::Direct);
        assert_eq!(phone.workdir.as_deref(), Some("/sdcard/work"));
        assert_eq!(
            phone.access.transport().unwrap(),
            ("adb".to_owned(), "serial = \"emulator-5554\"\n".to_owned())
        );
        let lab = theaters.find("lab").unwrap();
        assert_eq!(lab.harnesses, ["claude", "codex"]);
        let published = serde_json::to_string(&lab.summary()).unwrap();
        assert!(published.contains(r#""egress":"proxy""#));
        assert!(!published.contains("secret"));
        assert!(
            serde_json::to_string(&phone.summary())
                .unwrap()
                .contains(r#""provider":"adb""#)
        );
    }

    #[test]
    fn a_runner_is_declared_with_defaults_and_published_as_a_fact() {
        let theaters = Theaters::parse(
            r#"
[theater.local]
access = "local"
runner = {}

[theater.box]
access = { ssh = { host = "b.example" } }
runner = { command = "/opt/ai6/bin/ai6", state_dir = "/var/lib/ai6/runner", plugin_paths = ["/opt/ai6/lib"], terminal = "pty" }
"#,
        )
        .unwrap();
        let local = theaters.find("local").unwrap();
        assert_eq!(local.runner, Some(RunnerSettings::default()));
        let remote = theaters.find("box").unwrap();
        let runner = remote.runner.clone().unwrap();
        assert_eq!(runner.command.as_deref(), Some("/opt/ai6/bin/ai6"));
        assert_eq!(runner.plugin_paths, ["/opt/ai6/lib"]);
        let published = serde_json::to_value(remote.summary()).unwrap();
        assert_eq!(published["runner"], true);
        assert!(!published.to_string().contains("/opt/ai6"));
        assert!(
            Theaters::parse("[theater.box]\naccess = \"local\"\nrunner = { tmux = true }\n")
                .is_err(),
            "unknown runner settings are refused"
        );
        assert!(
            Theaters::parse("[theater.box]\naccess = \"local\"\nrunner = { command = \"\" }\n")
                .is_err()
        );
    }

    #[test]
    fn requirements_are_published_and_the_provision_spec_is_not() {
        let theaters = Theaters::parse(
            r#"
[theater.box]
access = { ssh = { host = "b.example" } }

[theater.box.requires]
os = "linux"
harnesses = ["claude>=2.1"]
tools = ["git"]
disk_mb = 512

[theater.box.provision]
runner = "/srv/builds/ai6"
install = { git = "sudo dnf install -y git" }
"#,
        )
        .unwrap();
        let theater = theaters.find("box").unwrap();
        let requires = theater.requires.clone().unwrap();
        assert_eq!(requires.harnesses[0].to_string(), "claude>=2.1");
        assert_eq!(requires.disk_mb, Some(512));
        let provision = theater.provision.clone().unwrap();
        assert_eq!(provision.install["git"], "sudo dnf install -y git");
        let published = serde_json::to_value(theater.summary()).unwrap();
        assert_eq!(
            published["requires"],
            serde_json::json!({"os": "linux", "harnesses": ["claude>=2.1"], "tools": ["git"], "disk_mb": 512})
        );
        assert!(!published.to_string().contains("dnf"));
        for bad in [
            "[theater.x]\naccess = \"local\"\nrequires = { tools = [\"git\", \"git\"] }\n",
            "[theater.x]\naccess = \"local\"\nrequires = { tools = [\"git>=x\"] }\n",
            "[theater.x]\naccess = \"local\"\nrequires = { cpu = 4 }\n",
            "[theater.x]\naccess = \"local\"\nprovision = { install = { git = \"\" } }\n",
        ] {
            assert!(Theaters::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_ephemeral_template_names_its_provider_and_bounds_what_it_keeps() {
        let theaters = Theaters::parse(
            r#"
[theater.scratch]
access = { ephemeral = { provider = "container", config = { image = "fedora:44", engine = "podman" }, keep_failed = 1 } }
agents = "remote-hands"
"#,
        )
        .unwrap();
        let scratch = theaters.find("scratch").unwrap();
        let TheaterAccess::Ephemeral(template) = &scratch.access else {
            panic!("expected an ephemeral template");
        };
        assert_eq!(
            (template.keep_failed, template.keep_failed_hours),
            (Some(1), None)
        );
        assert_eq!(
            template.settings_toml().unwrap(),
            "engine = \"podman\"\nimage = \"fedora:44\"\n"
        );
        assert!(scratch.access.transport().is_err());
        let published = serde_json::to_value(scratch.summary()).unwrap();
        assert_eq!(published["access"], "ephemeral");
        assert_eq!(published["provider"], "container");
        assert!(!published.to_string().contains("fedora"));
        for bad in [
            "[theater.x]\naccess = { ephemeral = { provider = \"\" } }\n",
            "[theater.x]\naccess = { ephemeral = { provider = \"c\", keep = 1 } }\n",
            "[theater.local]\naccess = { ephemeral = { provider = \"c\" } }\n",
        ] {
            assert!(Theaters::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ssh_access_becomes_the_ssh_transports_settings() {
        let access = TheaterAccess::Ssh(
            SshAccess::new("h.example")
                .with_user("ci")
                .with_port(2222)
                .with_host_key(format!("sha256:{}", "ab".repeat(32))),
        );
        let (provider, settings) = access.transport().unwrap();
        assert_eq!(provider, "ssh");
        let parsed: SshAccess = toml::from_str(&settings).unwrap();
        assert_eq!(TheaterAccess::Ssh(parsed), access);
        assert_eq!(
            TheaterAccess::Local.transport().unwrap(),
            ("local".to_owned(), String::new())
        );
    }

    #[test]
    fn trust_on_first_use_is_a_reported_weakening_and_a_pin_removes_it() {
        let loose = Theater::new(
            "a",
            TheaterAccess::Ssh(SshAccess::new("h").trusting_on_first_use()),
        );
        assert_eq!(loose.weakenings().len(), 1);
        assert_eq!(loose.summary().weakened, loose.weakenings());
        let pinned = Theater::new(
            "a",
            TheaterAccess::Ssh(
                SshAccess::new("h")
                    .trusting_on_first_use()
                    .with_host_key(format!("sha256:{}", "0".repeat(64))),
            ),
        );
        assert!(pinned.weakenings().is_empty());
        assert!(
            Theater::new("a", TheaterAccess::Local)
                .weakenings()
                .is_empty()
        );
    }

    #[test]
    fn rejects_invalid_theaters() {
        for text in [
            "[theater.x]\n",
            "[theater.x]\naccess = \"remote\"\n",
            "[theater.x]\naccess = { ssh = { host = \"-oProxyCommand=x\" } }\n",
            "[theater.x]\naccess = { ssh = { host = \"h\", port = 0 } }\n",
            "[theater.x]\naccess = { ssh = { host = \"h\", host_key = \"md5:aa\" } }\n",
            "[theater.x]\naccess = { ssh = { host = \"h\", host_key = \"sha256:abc\" } }\n",
            "[theater.\"a b\"]\naccess = \"local\"\n",
            "[theater.x]\naccess = \"local\"\nunknown = 1\n",
            "[theater.x]\naccess = \"local\"\nagents = \"remote\"\n",
            "[theater.x]\naccess = \"local\"\negress = \"open\"\n",
            "[theater.x]\naccess = \"local\"\negress = { proxy = \"\" }\n",
            "[theater.x]\naccess = \"local\"\nworkdir = \"\"\n",
            "[theater.x]\naccess = { transport = { provider = \"\" } }\n",
            "[theater.x]\naccess = { transport = { provider = \"adb\", other = 1 } }\n",
        ] {
            assert!(Theaters::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn merge_prefers_code_declared_theaters() {
        let mut file = Theaters::parse("[theater.a]\naccess = \"local\"\n").unwrap();
        let mut code = Theaters::default();
        code.insert(Theater::new("a", TheaterAccess::Local).with_label("k", "v"))
            .unwrap();
        file.merge(code);
        assert_eq!(file.find("a").unwrap().labels["k"], "v");
    }
}
