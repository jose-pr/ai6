//! Where a Station's [`Roster`] comes from.
//!
//! A [`ConfigSource`] hands out the roster at a [`ConfigRevision`] and says cheaply whether
//! it has changed since. [`TomlConfigDir`] reads `operations.toml`, `agents.toml` and
//! `covers.toml` from a configuration directory, the default; [`MemoryConfigSource`] holds a
//! roster built in code; with the `sqlite` or `postgres` feature, `SqlConfigSource` keeps it in
//! a database every Station shares. Any other store builds its roster with
//! [`Roster::from_parts`].
//!
//! Theaters are not part of it: each Station reads its own.

#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod sql;

#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use sql::{ConfigRecord, SqlConfigSource};

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest, Sha256};

use crate::directorates::m::roster::{Roster, RosterError, error};

/// The files a [`TomlConfigDir`] builds its roster from.
const ROSTER_FILES: [&str; 3] = ["operations.toml", "agents.toml", "covers.toml"];

/// Which state of a [`ConfigSource`] a [`ConfigSnapshot`] is: opaque, compared only for
/// equality, and chosen by the source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigRevision(u64);

impl ConfigRevision {
    /// A revision with this number.
    pub const fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A roster at one [`ConfigRevision`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ConfigSnapshot {
    /// Which state of the source this is.
    pub revision: ConfigRevision,
    /// The checked roster.
    pub roster: Roster,
}

impl ConfigSnapshot {
    /// A snapshot of `roster` at `revision`.
    pub fn new(revision: ConfigRevision, roster: Roster) -> Self {
        Self { revision, roster }
    }
}

/// Where the Station's Operations, Agents, roles and covers come from.
///
/// A source hands out only rosters that passed the same checks as the configuration files
/// ([`Roster::load`], [`Roster::from_parts`]): an invalid configuration is an error, never a
/// roster. Calls are synchronous; the daemon makes them off its async workers.
pub trait ConfigSource: Send + Sync {
    /// The roster, with the revision it is at.
    ///
    /// # Errors
    ///
    /// Returns an error when the configuration cannot be read or is not acceptable.
    fn load(&self) -> Result<ConfigSnapshot, RosterError>;

    /// The roster again when the source is no longer at revision `since`, else `None`.
    ///
    /// The default loads and compares revisions; a source that can tell cheaply overrides
    /// it.
    ///
    /// # Errors
    ///
    /// As [`load`](ConfigSource::load).
    fn changes_since(&self, since: ConfigRevision) -> Result<Option<ConfigSnapshot>, RosterError> {
        let snapshot = self.load()?;
        Ok((snapshot.revision != since).then_some(snapshot))
    }
}

/// Opens the roster at `location`: `sqlite:PATH` (or `sqlite://PATH`) or a `postgres://`
/// URL for a `SqlConfigSource`, anything else a configuration directory ([`TomlConfigDir`]).
///
/// # Errors
///
/// `Unsupported` for a SQL location when this build lacks its cargo feature (`sqlite`,
/// `postgres`); the database's error when it cannot be opened. A directory is not read
/// until the roster is loaded.
pub fn open(location: &str) -> io::Result<Arc<dyn ConfigSource>> {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    return open_with(
        location,
        &crate::directorates::registry::sql::SqlOptions::default(),
    );
    #[cfg(not(any(feature = "sqlite", feature = "postgres")))]
    if is_sql(location) {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this build of ai6 has no SQL configuration support (cargo features `sqlite`, `postgres`)",
        ))
    } else {
        Ok(Arc::new(TomlConfigDir::new(location)))
    }
}

/// As [`open`], connecting a SQL roster with `options` (password, timeouts).
///
/// # Errors
///
/// As [`open`].
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub fn open_with(
    location: &str,
    options: &crate::directorates::registry::sql::SqlOptions,
) -> io::Result<Arc<dyn ConfigSource>> {
    if is_sql(location) {
        Ok(Arc::new(SqlConfigSource::open_with(location, options)?))
    } else {
        Ok(Arc::new(TomlConfigDir::new(location)))
    }
}

fn is_sql(location: &str) -> bool {
    location.starts_with("sqlite:")
        || location.starts_with("postgres://")
        || location.starts_with("postgresql://")
}

/// The configuration directory: `operations.toml`, `agents.toml` and `covers.toml`, read
/// with [`Roster::load`].
///
/// Its revision is a digest of those three files' contents, so asking for changes reads
/// them but parses nothing until one differs.
#[derive(Debug, Clone)]
pub struct TomlConfigDir {
    root: PathBuf,
}

impl TomlConfigDir {
    /// The configuration in `root`. Nothing is read until the source is asked.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The configuration directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn revision(&self) -> Result<ConfigRevision, RosterError> {
        let mut digest = Sha256::new();
        for name in ROSTER_FILES {
            let path = self.root.join(name);
            match fs::read(&path) {
                Ok(bytes) => {
                    digest.update([1]);
                    digest.update((bytes.len() as u64).to_le_bytes());
                    digest.update(&bytes);
                }
                Err(failure) if failure.kind() == io::ErrorKind::NotFound => digest.update([0]),
                Err(failure) => return Err(error(format!("{}: {failure}", path.display()))),
            }
        }
        let digest = digest.finalize();
        let mut number = [0; 8];
        number.copy_from_slice(&digest[..8]);
        Ok(ConfigRevision::new(u64::from_le_bytes(number)))
    }
}

impl ConfigSource for TomlConfigDir {
    fn load(&self) -> Result<ConfigSnapshot, RosterError> {
        // The revision is taken before the files are parsed, so a file changed in between
        // shows up as a change on the next poll rather than being missed.
        let revision = self.revision()?;
        Ok(ConfigSnapshot::new(revision, Roster::load(&self.root)?))
    }

    fn changes_since(&self, since: ConfigRevision) -> Result<Option<ConfigSnapshot>, RosterError> {
        if self.revision()? == since {
            return Ok(None);
        }
        self.load().map(Some)
    }
}

/// A roster kept in memory, built in code. Its revision counts the rosters it has held.
pub struct MemoryConfigSource {
    state: Mutex<(u64, Roster)>,
}

impl fmt::Debug for MemoryConfigSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryConfigSource")
            .finish_non_exhaustive()
    }
}

impl MemoryConfigSource {
    /// A source holding `roster` at revision 0.
    pub fn new(roster: Roster) -> Self {
        Self {
            state: Mutex::new((0, roster)),
        }
    }

    /// Replaces the roster; the source moves to its next revision.
    pub fn replace(&self, roster: Roster) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 += 1;
        state.1 = roster;
    }
}

impl ConfigSource for MemoryConfigSource {
    fn load(&self) -> Result<ConfigSnapshot, RosterError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(ConfigSnapshot::new(
            ConfigRevision::new(state.0),
            state.1.clone(),
        ))
    }

    fn changes_since(&self, since: ConfigRevision) -> Result<Option<ConfigSnapshot>, RosterError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok((state.0 != since.get())
            .then(|| ConfigSnapshot::new(ConfigRevision::new(state.0), state.1.clone())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::m::roster::{Agent, AgentRole, Operation};

    fn roster(agent: &str) -> Roster {
        let mut operation = Operation::new("G");
        operation.roster = vec![agent.to_owned()];
        let mut officer = Agent::new(agent);
        officer.handler = Some("mock-agent".to_owned());
        officer.roles = vec!["case-officer".to_owned()];
        Roster::builder()
            .operation(operation)
            .agent(officer)
            .role(AgentRole::new(
                "case-officer",
                "Runs a Mission end to end.",
                "Report to M.",
            ))
            .build()
            .unwrap()
    }

    #[test]
    fn a_memory_source_reports_each_replacement_once() {
        let source = MemoryConfigSource::new(roster("007"));
        let first = source.load().unwrap();
        assert_eq!(first.roster.agent(Some("G"), "007").unwrap().id, "007");
        assert!(source.changes_since(first.revision).unwrap().is_none());

        source.replace(roster("008"));
        let second = source.changes_since(first.revision).unwrap().unwrap();
        assert_ne!(second.revision, first.revision);
        assert!(second.roster.agent(Some("G"), "007").is_err());
        assert_eq!(second.roster.agent(Some("G"), "008").unwrap().id, "008");
        assert!(source.changes_since(second.revision).unwrap().is_none());
    }

    #[test]
    fn a_config_directory_changes_when_a_roster_file_does() {
        let root = tempfile::tempdir().unwrap();
        let source = TomlConfigDir::new(root.path());
        let empty = source.load().unwrap();
        assert_eq!(empty.roster.operations().count(), 0);
        assert!(source.changes_since(empty.revision).unwrap().is_none());

        fs::write(root.path().join("operations.toml"), "[operation.G]\n").unwrap();
        let changed = source.changes_since(empty.revision).unwrap().unwrap();
        assert_eq!(changed.roster.operation("G").unwrap().id, "G");
        assert!(source.changes_since(changed.revision).unwrap().is_none());

        // A file the roster is not read from is not a change.
        fs::write(root.path().join("theaters.toml"), "").unwrap();
        assert!(source.changes_since(changed.revision).unwrap().is_none());
    }

    #[test]
    fn an_invalid_config_directory_is_an_error_not_a_roster() {
        let root = tempfile::tempdir().unwrap();
        let source = TomlConfigDir::new(root.path());
        let good = source.load().unwrap();
        fs::write(
            root.path().join("operations.toml"),
            "[operation.G]\non_blown = \"shrug\"\n",
        )
        .unwrap();
        let refused = source.changes_since(good.revision).unwrap_err();
        assert!(refused.to_string().contains("on_blown"), "{refused}");
    }
}
