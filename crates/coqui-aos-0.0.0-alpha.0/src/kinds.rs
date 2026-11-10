//! The closed sets of words the specification and its vectors use: a runtime, the end state of an
//! environment, a manifest file, a format, a setup step and the settings a consumer reports.

/// A runtime whose packages an overlay installs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Runtime {
    /// `python`.
    Python,
    /// `node`.
    Node,
}

impl Runtime {
    /// The word the specification and its vectors write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Node => "node",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "python" => Some(Self::Python),
            "node" => Some(Self::Node),
            _ => None,
        }
    }
}

impl core::fmt::Display for Runtime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a removed overlay's dependencies leave of an environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum LeftState {
    /// `rebuilt`.
    Rebuilt,
    /// `deleted`.
    Deleted,
}

impl LeftState {
    /// The word the specification and its vectors write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rebuilt => "rebuilt",
            Self::Deleted => "deleted",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "rebuilt" => Some(Self::Rebuilt),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

impl core::fmt::Display for LeftState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The file an overlay's manifest is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ManifestFile {
    /// `OVERLAY.md`.
    OverlayMd,
    /// `overlay.cfg`.
    OverlayCfg,
}

impl ManifestFile {
    /// The word the specification and its vectors write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OverlayMd => "OVERLAY.md",
            Self::OverlayCfg => "overlay.cfg",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "OVERLAY.md" => Some(Self::OverlayMd),
            "overlay.cfg" => Some(Self::OverlayCfg),
            _ => None,
        }
    }
}

impl core::fmt::Display for ManifestFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A format a document is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Format {
    /// `toml`.
    Toml,
    /// `json`.
    Json,
    /// `yaml`.
    Yaml,
}

impl Format {
    /// The word the specification and its vectors write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Toml => "toml",
            Self::Json => "json",
            Self::Yaml => "yaml",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "toml" => Some(Self::Toml),
            "json" => Some(Self::Json),
            "yaml" => Some(Self::Yaml),
            _ => None,
        }
    }
}

impl core::fmt::Display for Format {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The step of a preparation that fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum FailStep {
    /// `deps`.
    Deps,
    /// `setup`.
    Setup,
}

impl FailStep {
    /// The word the specification and its vectors write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deps => "deps",
            Self::Setup => "setup",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "deps" => Some(Self::Deps),
            "setup" => Some(Self::Setup),
            _ => None,
        }
    }
}

impl core::fmt::Display for FailStep {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A non-default setting a consumer reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NonDefaultId {
    /// `deps-network`.
    DepsNetwork,
    /// `env-passthrough`.
    EnvPassthrough,
    /// `env-script`.
    EnvScript,
    /// `env-sensitive`.
    EnvSensitive,
    /// `network-not-enforced`.
    NetworkNotEnforced,
    /// `no-timeout`.
    NoTimeout,
    /// `script-not-run`.
    ScriptNotRun,
    /// `setup-network`.
    SetupNetwork,
    /// `unpinned`.
    Unpinned,
    /// `secret-read`.
    SecretRead,
    /// `insecure-transport`.
    InsecureTransport,
    /// `private-source`.
    PrivateSource,
    /// `hook`.
    Hook,
    /// `unlocked-dependencies`.
    UnlockedDependencies,
}

impl NonDefaultId {
    /// The word the specification and its vectors write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DepsNetwork => "deps-network",
            Self::EnvPassthrough => "env-passthrough",
            Self::EnvScript => "env-script",
            Self::EnvSensitive => "env-sensitive",
            Self::NetworkNotEnforced => "network-not-enforced",
            Self::NoTimeout => "no-timeout",
            Self::ScriptNotRun => "script-not-run",
            Self::SetupNetwork => "setup-network",
            Self::Unpinned => "unpinned",
            Self::SecretRead => "secret-read",
            Self::InsecureTransport => "insecure-transport",
            Self::PrivateSource => "private-source",
            Self::Hook => "hook",
            Self::UnlockedDependencies => "unlocked-dependencies",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "deps-network" => Some(Self::DepsNetwork),
            "env-passthrough" => Some(Self::EnvPassthrough),
            "env-script" => Some(Self::EnvScript),
            "env-sensitive" => Some(Self::EnvSensitive),
            "network-not-enforced" => Some(Self::NetworkNotEnforced),
            "no-timeout" => Some(Self::NoTimeout),
            "script-not-run" => Some(Self::ScriptNotRun),
            "setup-network" => Some(Self::SetupNetwork),
            "unpinned" => Some(Self::Unpinned),
            "secret-read" => Some(Self::SecretRead),
            "insecure-transport" => Some(Self::InsecureTransport),
            "private-source" => Some(Self::PrivateSource),
            "hook" => Some(Self::Hook),
            "unlocked-dependencies" => Some(Self::UnlockedDependencies),
            _ => None,
        }
    }
}

impl core::fmt::Display for NonDefaultId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One step of preparing an overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Step {
    /// The overlay's state directory is emptied.
    EmptyState,
    /// A runtime's dependencies are installed.
    Deps(Runtime),
    /// The setup script runs.
    Setup,
    /// The record of the preparation is written.
    Record,
    /// The overlay's environments are deleted.
    Delete,
}

impl core::fmt::Display for Step {
    /// The step as the vectors write it: `empty-state`, `deps:<runtime>`, `setup`, `record`,
    /// `delete`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyState => f.write_str("empty-state"),
            Self::Deps(runtime) => write!(f, "deps:{runtime}"),
            Self::Setup => f.write_str("setup"),
            Self::Record => f.write_str("record"),
            Self::Delete => f.write_str("delete"),
        }
    }
}

impl PartialOrd for NonDefaultId {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Reports are listed by the word, as the vectors sort them.
impl Ord for NonDefaultId {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}
