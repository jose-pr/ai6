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
    Change, ChangeLog, SafeHouseInfo, SafeHouseProvider, SafeHouseReport, SafeHouseSession,
    SafeHouseSpec,
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
    fn a_provider_without_history_says_so() {
        let house = SafeHouse::open(&Stub { history: false }, &spec(false)).unwrap();
        assert!(!house.supports_history() && !house.is_isolated());
        assert_eq!(house.base(), None);
    }
}
