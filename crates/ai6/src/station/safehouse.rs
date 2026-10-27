//! The Station's side of safe houses: opening one through a provider, reading its changes,
//! finishing it, and the state directory that holds them.
//!
//! The Station never runs a version-control command itself; a
//! [`SafeHouseProvider`] does.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use guatiao::library::Object;
use guatiao::{FromValue, ToValue};

use crate::directorates::q::safehouse::{
    Change, ChangeLog, CheckpointSpec, SafeHouseCheckpoint, SafeHouseExports, SafeHouseInfo,
    SafeHouseProvider, SafeHouseReport, SafeHouseSession, SafeHouseSpec,
};

/// The state-directory environment variable used by AI6.
pub const STATE_ENV: &str = "AI6_STATE";

/// A rejected safe-house operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SafeHouseError(String);

impl std::fmt::Display for SafeHouseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SafeHouseError {}

/// A Mission's open safe house: a provider session and what it reported when opened.
#[derive(Clone)]
pub struct SafeHouse {
    session: Arc<Object<dyn SafeHouseSession>>,
    info: SafeHouseInfo,
}

impl std::fmt::Debug for SafeHouse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SafeHouse")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl SafeHouse {
    /// Opens the safe house `spec` describes through `provider`. A `spec.reopen` report
    /// re-attaches to a safe house kept by an earlier run, for example after a restart.
    pub fn open(
        provider: &dyn SafeHouseProvider,
        spec: &SafeHouseSpec,
    ) -> Result<Self, SafeHouseError> {
        let spec = spec.to_value().map_err(fail)?;
        let session = provider.open(&spec).map_err(fail)?;
        let info = SafeHouseInfo::from_value(&session.info().map_err(fail)?).map_err(fail)?;
        Ok(Self {
            session: Arc::new(session),
            info,
        })
    }

    /// The working directory.
    pub fn path(&self) -> &Path {
        Path::new(&self.info.path)
    }

    /// The branch assigned to the Mission, when the provider has branches.
    pub fn branch(&self) -> Option<&str> {
        self.info.branch.as_deref()
    }

    /// The revision the work started from, when the provider has revisions.
    pub fn base(&self) -> Option<&str> {
        self.info.base.as_deref()
    }

    /// Whether [`changes`](Self::changes) can answer; cover is not checkable otherwise.
    pub fn supports_history(&self) -> bool {
        self.info.supports_history
    }

    /// Whether the working directory was made for this Mission, so it is finished and
    /// recorded; a safe house in place is neither.
    pub fn is_isolated(&self) -> bool {
        self.info.isolated
    }

    /// The changes made since the base, newest first; empty without history.
    pub fn changes(&self) -> Result<Vec<Change>, SafeHouseError> {
        let log = self.session.changes().map_err(fail)?;
        Ok(ChangeLog::from_value(&log).map_err(fail)?.changes)
    }

    /// Removes an untouched isolated safe house and keeps one that holds work.
    pub fn finish(&self) -> Result<SafeHouseReport, SafeHouseError> {
        let report = self.session.finish().map_err(fail)?;
        SafeHouseReport::from_value(&report).map_err(fail)
    }

    /// Saves the work so far where another Station can fetch it, leaving the working
    /// directory as it is.
    pub fn checkpoint(&self, spec: &CheckpointSpec) -> Result<SafeHouseCheckpoint, SafeHouseError> {
        let spec = spec.to_value().map_err(fail)?;
        let saved = self.session.checkpoint(&spec).map_err(fail)?;
        SafeHouseCheckpoint::from_value(&saved).map_err(fail)
    }

    /// What of the work exists only here; `None` when the provider cannot tell, which a
    /// caller reads as all of it.
    pub fn exports(&self) -> Result<Option<SafeHouseExports>, SafeHouseError> {
        match self.session.exports() {
            Ok(exports) => SafeHouseExports::from_value(&exports)
                .map(Some)
                .map_err(fail),
            Err(error) if error.status == guatiao::Status::GUATIAO_ERR_UNSUPPORTED => Ok(None),
            Err(error) => Err(fail(error)),
        }
    }

    /// Removes an isolated safe house whatever it holds, through its provider.
    pub fn discard(&self) -> Result<(), SafeHouseError> {
        self.session.discard().map_err(fail)
    }

    /// Describes the safe house while it is still in use.
    pub fn active_report(&self) -> SafeHouseReport {
        SafeHouseReport {
            path: self.info.path.clone(),
            branch: self.info.branch.clone(),
            base: self.info.base.clone(),
            kept: true,
            changes: Vec::new(),
        }
    }
}

/// The directories a Station treats as rebuildable caches unless told otherwise.
pub const DEFAULT_CACHE_DIRS: [&str; 3] = ["target", "node_modules", ".dart_tool"];
/// How long a terminal Mission's safe house is left alone before its caches may go.
pub const DEFAULT_CACHE_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// What a Station may tidy in the safe houses it keeps without asking anyone: which
/// directories are rebuildable caches, and how long a terminal Mission's safe house is left
/// alone before they may be removed.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeHouseHygiene {
    caches: Vec<String>,
    cache_age: std::time::Duration,
}

impl Default for SafeHouseHygiene {
    /// [`DEFAULT_CACHE_DIRS`] after [`DEFAULT_CACHE_AGE`].
    fn default() -> Self {
        Self {
            caches: DEFAULT_CACHE_DIRS.map(str::to_owned).to_vec(),
            cache_age: DEFAULT_CACHE_AGE,
        }
    }
}

impl SafeHouseHygiene {
    /// Replaces the cache directory names; an empty list removes no cache.
    ///
    /// # Errors
    ///
    /// A name that is not one plain directory name: empty, `.`, `..`, `.git`, or holding a
    /// path separator or `:`.
    pub fn with_caches(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, SafeHouseError> {
        let names: Vec<String> = names.into_iter().map(Into::into).collect();
        if let Some(name) = names.iter().find(|name| {
            matches!(name.as_str(), "" | "." | ".." | ".git")
                || name.contains(['/', '\\', ':'])
                || name.chars().any(char::is_control)
        }) {
            return Err(fail(format!(
                "{name:?} is not a cache directory name: use one plain directory name such as \"target\""
            )));
        }
        self.caches = names;
        Ok(self)
    }

    /// Sets how long a terminal Mission's safe house is left alone before its caches may go.
    #[must_use]
    pub fn with_cache_age(mut self, age: std::time::Duration) -> Self {
        self.cache_age = age;
        self
    }

    /// The cache directory names.
    pub fn caches(&self) -> &[String] {
        &self.caches
    }

    /// How long a terminal Mission's safe house is left alone first.
    pub fn cache_age(&self) -> std::time::Duration {
        self.cache_age
    }

    /// Whether a directory of this name is a cache.
    pub fn is_cache(&self, name: &str) -> bool {
        self.caches.iter().any(|cache| cache == name)
    }
}

/// Resolves AI6's state directory for retained safe houses and Mission reports.
pub fn state_dir() -> Result<PathBuf, SafeHouseError> {
    if let Some(path) = std::env::var_os(STATE_ENV) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("AI6_HOME") {
        return Ok(PathBuf::from(path).join("state"));
    }
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(path).join("AI6"));
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".local").join("state"))
        })
        .ok_or_else(|| fail("could not determine AI6 state directory"))?;
    Ok(base.join("AI6"))
}

fn fail(error: impl std::fmt::Display) -> SafeHouseError {
    SafeHouseError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guatiao::Value;
    use guatiao::library::ProviderError;

    struct Stub {
        history: bool,
    }

    impl SafeHouseProvider for Stub {
        fn open(&self, spec: &Value) -> Result<Object<dyn SafeHouseSession>, ProviderError> {
            let spec = SafeHouseSpec::from_value(spec).unwrap();
            Ok(StubSession {
                info: SafeHouseInfo {
                    path: spec.source,
                    branch: None,
                    base: self.history.then(|| "base".to_owned()),
                    supports_history: self.history,
                    isolated: spec.isolate,
                },
            }
            .into_object())
        }
    }

    struct StubSession {
        info: SafeHouseInfo,
    }

    impl SafeHouseSession for StubSession {
        fn info(&self) -> Result<Value, ProviderError> {
            Ok(self.info.to_value().unwrap())
        }

        fn changes(&self) -> Result<Value, ProviderError> {
            Ok(ChangeLog {
                changes: vec![Change {
                    id: "1".to_owned(),
                    author: "a".to_owned(),
                    committer: "a".to_owned(),
                    message: "m".to_owned(),
                }],
            }
            .to_value()
            .unwrap())
        }

        fn finish(&self) -> Result<Value, ProviderError> {
            Ok(SafeHouseReport {
                path: self.info.path.clone(),
                branch: None,
                base: None,
                kept: false,
                changes: Vec::new(),
            }
            .to_value()
            .unwrap())
        }
    }

    fn spec(isolate: bool) -> SafeHouseSpec {
        SafeHouseSpec {
            operation_id: "G".to_owned(),
            mission_id: "mission-1".to_owned(),
            title: "t".to_owned(),
            source: "/work".to_owned(),
            base: "HEAD".to_owned(),
            branch: "{id}".to_owned(),
            isolate,
            state_root: "/state".to_owned(),
            reopen: None,
            checkpoint: None,
            clone: None,
        }
    }

    #[test]
    fn the_host_reads_a_provider_session_through_the_kind() {
        let house = SafeHouse::open(&Stub { history: true }, &spec(true)).unwrap();
        assert_eq!(house.path(), Path::new("/work"));
        assert_eq!(house.base(), Some("base"));
        assert!(house.is_isolated() && house.supports_history());
        assert_eq!(house.changes().unwrap().len(), 1);
        assert!(!house.finish().unwrap().kept);
        let active = house.active_report();
        assert!(active.kept && active.path == "/work");
    }

    #[test]
    fn a_provider_that_cannot_tell_what_is_exported_says_so_and_discards_nothing() {
        let house = SafeHouse::open(&Stub { history: true }, &spec(true)).unwrap();
        assert_eq!(house.exports().unwrap(), None);
        assert!(house.discard().is_err());
        let exported = SafeHouseExports::default();
        assert!(exported.is_exported(|_| false));
        let dirty = SafeHouseExports {
            dirty: true,
            ..SafeHouseExports::default()
        };
        assert!(!dirty.is_exported(|_| true));
        let ignored = SafeHouseExports {
            ignored: vec![".env".to_owned()],
            ..SafeHouseExports::default()
        };
        assert!(!ignored.is_exported(|path| path == "target"));
        assert!(ignored.is_exported(|path| path == ".env"));
    }

    #[test]
    fn hygiene_takes_plain_directory_names_only() {
        let hygiene = SafeHouseHygiene::default();
        assert!(hygiene.is_cache("target") && hygiene.is_cache(".dart_tool"));
        assert!(!hygiene.is_cache("src"));
        assert_eq!(hygiene.cache_age(), DEFAULT_CACHE_AGE);
        for bad in ["", ".", "..", ".git", "a/b", "a\\b", "C:"] {
            assert!(
                SafeHouseHygiene::default().with_caches([bad]).is_err(),
                "{bad:?}"
            );
        }
        let custom = SafeHouseHygiene::default()
            .with_caches(["build", ".venv"])
            .unwrap()
            .with_cache_age(std::time::Duration::from_secs(60));
        assert_eq!(custom.caches(), ["build", ".venv"]);
        assert!(!custom.is_cache("target"));
    }

    #[test]
    fn a_provider_without_history_says_so() {
        let house = SafeHouse::open(&Stub { history: false }, &spec(false)).unwrap();
        assert!(!house.supports_history() && !house.is_isolated());
        assert_eq!(house.base(), None);
    }
}
