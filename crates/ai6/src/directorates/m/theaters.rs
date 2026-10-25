//! Theaters: where a Session's harness runs, and how a Station reaches each one.
//!
//! Each Station reads its own `theaters.toml`:
//!
//! ```toml
//! [theater.build-box]
//! access = { ssh = { host = "build.example.net", user = "ci", port = 22, identity = "~/.ssh/ci" } }
//! labels = { os = "linux", gpu = "yes" }
//! ```
//!
//! A Station's own host is always reachable as the implicit `local` Theater; declaring
//! `[theater.local]` only adds labels to it.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

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
    /// A host reached over SSH. Recorded and published, but not yet chosen by placement.
    Ssh(SshAccess),
}

/// SSH connection settings for a Theater.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshAccess {
    /// Host name or address.
    pub host: String,
    /// Remote user; the SSH client's default when absent.
    #[serde(default)]
    pub user: Option<String>,
    /// Remote port; the SSH client's default when absent.
    #[serde(default)]
    pub port: Option<u16>,
    /// Path of the private key file; the SSH client's default when absent.
    #[serde(default)]
    pub identity: Option<String>,
}

impl SshAccess {
    /// Settings for `host` with every other option left to the SSH client.
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            user: None,
            port: None,
            identity: None,
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
}

impl Theater {
    /// A Theater with no labels.
    pub fn new(id: impl Into<String>, access: TheaterAccess) -> Self {
        Self {
            id: id.into(),
            access,
            labels: BTreeMap::new(),
        }
    }

    /// Adds or replaces a label.
    #[must_use]
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// The non-secret description a Station publishes at check-in.
    pub fn summary(&self) -> TheaterSummary {
        let (access, host) = match &self.access {
            TheaterAccess::Local => (TheaterKind::Local, None),
            TheaterAccess::Ssh(ssh) => (TheaterKind::Ssh, Some(ssh.host.clone())),
        };
        TheaterSummary {
            id: self.id.clone(),
            access,
            host,
            labels: self.labels.clone(),
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
        if let TheaterAccess::Ssh(ssh) = &self.access {
            // A leading '-' would be read by the SSH client as an option.
            let word = |value: &str| {
                !value.is_empty()
                    && !value.starts_with('-')
                    && !value.chars().any(|c| c.is_whitespace() || c.is_control())
            };
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
        }
        Ok(())
    }
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
}

impl TheaterKind {
    /// The name used in configuration and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Ssh => "ssh",
        }
    }
}

/// What a Station tells the Registry about one Theater it reaches: no user, port or key path.
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
    /// Theater labels.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
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
        }
        let document: Document = toml::from_str(text).map_err(|e| TheaterError(e.to_string()))?;
        let mut theaters = Self::default();
        for (id, definition) in document.theater {
            theaters.insert(Theater {
                id,
                access: definition.access,
                labels: definition.labels,
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

    /// The reachable Theaters as published at check-in.
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
            serde_json::json!({"id": "box", "access": "ssh", "host": "b.example", "labels": {"os": "linux"}})
        );
        assert_eq!(theaters.summaries().len(), 2);
    }

    #[test]
    fn rejects_invalid_theaters() {
        for text in [
            "[theater.x]\n",
            "[theater.x]\naccess = \"remote\"\n",
            "[theater.x]\naccess = { ssh = { host = \"-oProxyCommand=x\" } }\n",
            "[theater.x]\naccess = { ssh = { host = \"h\", port = 0 } }\n",
            "[theater.\"a b\"]\naccess = \"local\"\n",
            "[theater.x]\naccess = \"local\"\nunknown = 1\n",
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
