//! Where a Station's configuration comes from: its configuration provider.
//!
//! A [`ConfigProvider`] holds records of each family ([`ConfigRecord`]) at a scope
//! ([`ConfigScope`]): global, or one Station's. A Station's view of a family is its own
//! records over the global ones, key by key; a singleton family's one record replaces the
//! global one whole. The provider hands out that view checked and built
//! ([`ConfigSnapshot`]: the roster and the [`Settings`]) at a [`ConfigRevision`], and says
//! cheaply whether it has changed since.
//!
//! [`FileProvider`] reads a directory of family files (`operations.toml`, `agents.yaml`,
//! `stations/<id>/theaters.json`, ...), read-only; [`MemoryProvider`] holds a roster built in
//! code; with the `sqlite` or `postgres` feature, `SqlProvider` keeps the records in a
//! database every Station shares and takes changes. Any other store builds its roster with
//! [`Roster::from_parts`].
//!
//! A provider that takes changes ([`ConfigProvider::put`], [`ConfigProvider::remove`]) checks
//! every view a change touches before keeping it; a Station polls its provider and applies a
//! new revision of the families that apply live without a restart.

mod file;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod sql;

pub use file::FileProvider;
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub use sql::SqlProvider;

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest, Sha256};

use crate::directorates::m::cover::{Covers, cover_from};
use crate::directorates::m::roster::{
    Roster, RosterError, RosterErrorKind, error, error_of, roster_from_definitions,
};
use crate::directorates::m::theaters::Theaters;

/// The families of record a configuration provider holds.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigRecord {
    /// An Operation: the keys of an `[operation.<id>]` table.
    Operation,
    /// An Agent: the keys of an `[agent.<codename>]` table.
    Agent,
    /// An Agent role: the keys of a `[role.<id>]` table.
    Role,
    /// Moneypenny's settings ([`AssistantSettings`](crate::AssistantSettings)): the keys of
    /// the `[assistant]` table; a singleton.
    Assistant,
    /// A cover policy: the keys of a `[cover.<name>]` table.
    Cover,
    /// A Theater: the keys of a `[theater.<id>]` table; global or one Station's.
    Theater,
}

impl ConfigRecord {
    /// Every family, in the order a snapshot is built.
    pub const ALL: [Self; 6] = [
        Self::Operation,
        Self::Agent,
        Self::Role,
        Self::Assistant,
        Self::Cover,
        Self::Theater,
    ];

    /// The key of a singleton family's one record.
    pub const SINGLETON_KEY: &'static str = "settings";

    /// The family's name, which is also its table in its family file: `operation`, `agent`,
    /// `role`, `assistant`, `cover` or `theater`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Operation => "operation",
            Self::Agent => "agent",
            Self::Role => "role",
            Self::Assistant => "assistant",
            Self::Cover => "cover",
            Self::Theater => "theater",
        }
    }

    /// The family whose [`name`](Self::name) is `name`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|record| record.name() == name)
    }

    /// The family file that holds it, without its extension: `operations`, `agents`,
    /// `covers` or `theaters`.
    pub const fn file(self) -> &'static str {
        match self {
            Self::Operation => "operations",
            Self::Agent | Self::Role | Self::Assistant => "agents",
            Self::Cover => "covers",
            Self::Theater => "theaters",
        }
    }

    /// Whether the family has one record, keyed [`SINGLETON_KEY`](Self::SINGLETON_KEY).
    pub const fn is_singleton(self) -> bool {
        matches!(self, Self::Assistant)
    }

    /// Whether a Station may have records of its own in this family.
    pub const fn station_scoped(self) -> bool {
        matches!(self, Self::Theater)
    }

    /// Whether a running Station applies a change without a restart.
    pub const fn applies_live(self) -> bool {
        !matches!(self, Self::Theater)
    }
}

impl fmt::Display for ConfigRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Where a record applies: to every Station, or to one.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigScope {
    /// Every Station, unless it has a record of its own with the same key.
    #[default]
    Global,
    /// One Station, by ID.
    Station(String),
}

impl ConfigScope {
    /// The text form a provider stores: `""` for [`Global`](Self::Global), `station:<id>`.
    pub fn as_text(&self) -> String {
        match self {
            Self::Global => String::new(),
            Self::Station(id) => format!("station:{id}"),
        }
    }

    /// The scope of a text form: `""` or `station:<id>`; `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        if text.is_empty() {
            return Some(Self::Global);
        }
        text.strip_prefix("station:")
            .filter(|id| !id.is_empty())
            .map(|id| Self::Station(id.to_owned()))
    }

    /// The scope a Station's own records have; [`Global`](Self::Global) for `""`.
    pub fn of_station(station: &str) -> Self {
        if station.is_empty() {
            Self::Global
        } else {
            Self::Station(station.to_owned())
        }
    }
}

impl fmt::Display for ConfigScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Global => formatter.write_str("global"),
            Self::Station(id) => write!(formatter, "station:{id}"),
        }
    }
}

/// The records of one family at one scope as a provider holds them, at one revision.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigDefinitions {
    /// The revision they were read at.
    pub revision: ConfigRevision,
    /// Each record's key and definition: a JSON object with the keys of its table in its
    /// family file, ordered by key.
    pub records: Vec<(String, serde_json::Value)>,
    /// Whether the provider takes [`put`](ConfigProvider::put) and
    /// [`remove`](ConfigProvider::remove).
    pub writable: bool,
}

/// The configuration a Station runs, as it reports it at check-in (key `config`).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConfigStatus {
    /// The [`ConfigRevision`] it runs, in decimal: a digest can pass the 2^53 a JSON number
    /// holds exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Why it refused the newest configuration, while it keeps running the last good one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<String>,
    /// Families changed since it started that only a restart applies, such as `theater`, or
    /// files a Station still reads beside its provider, such as `cables.toml`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs_restart: Vec<String>,
}

fn read_only() -> RosterError {
    error_of(
        RosterErrorKind::ReadOnly,
        "this configuration provider does not take changes",
    )
}

/// Which state of a [`ConfigProvider`] a [`ConfigSnapshot`] is: opaque, compared only for
/// equality, and chosen by the provider.
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

/// What a snapshot holds besides the roster, resolved for one Station.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// The Theaters the Station declares (`theater` records).
    pub theaters: Theaters,
}

/// The configuration of one Station at one [`ConfigRevision`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ConfigSnapshot {
    /// Which state of the provider this is.
    pub revision: ConfigRevision,
    /// The checked roster.
    pub roster: Roster,
    /// Everything else.
    pub settings: Settings,
    /// A digest of each family's records in this view, to tell which families a later
    /// snapshot changed.
    fingerprints: BTreeMap<ConfigRecord, [u8; 32]>,
}

impl ConfigSnapshot {
    /// A snapshot of `roster` and `settings` at `revision`.
    pub fn new(revision: ConfigRevision, roster: Roster, settings: Settings) -> Self {
        Self {
            revision,
            roster,
            settings,
            fingerprints: BTreeMap::new(),
        }
    }

    /// The families whose records differ between this snapshot and `other`. Snapshots built
    /// in code ([`ConfigSnapshot::new`]) carry no records and differ in none.
    pub fn changed_families(&self, other: &Self) -> Vec<ConfigRecord> {
        ConfigRecord::ALL
            .into_iter()
            .filter(|record| self.fingerprints.get(record) != other.fingerprints.get(record))
            .collect()
    }
}

/// Where the Station's Operations, Agents, roles, covers, Moneypenny's settings and its other
/// configuration come from.
///
/// A provider hands out only configuration that passed the same checks as the family files
/// do: an invalid configuration is an error, never a snapshot. Calls are synchronous; the
/// daemon makes them off its async workers.
pub trait ConfigProvider: Send + Sync {
    /// The configuration of `station` (`""`: the global records only), with the revision it
    /// is at.
    ///
    /// # Errors
    ///
    /// Returns an error when the configuration cannot be read or is not acceptable.
    fn load(&self, station: &str) -> Result<ConfigSnapshot, RosterError>;

    /// The configuration of `station` again when the provider is no longer at revision
    /// `since`, else `None`.
    ///
    /// The default loads and compares revisions; a provider that can tell cheaply overrides
    /// it.
    ///
    /// # Errors
    ///
    /// As [`load`](ConfigProvider::load).
    fn changes_since(
        &self,
        station: &str,
        since: ConfigRevision,
    ) -> Result<Option<ConfigSnapshot>, RosterError> {
        let snapshot = self.load(station)?;
        Ok((snapshot.revision != since).then_some(snapshot))
    }

    /// The revision `station`'s configuration is at now, without building it.
    ///
    /// A Station compares it with the revision it runs and the last one it refused, so an
    /// unacceptable revision is checked and reported once. The default loads; a provider
    /// that can tell cheaply overrides it.
    ///
    /// # Errors
    ///
    /// As [`load`](ConfigProvider::load).
    fn revision(&self, station: &str) -> Result<ConfigRevision, RosterError> {
        self.load(station).map(|snapshot| snapshot.revision)
    }

    /// The records of one family at `scope` as the provider holds them, for an administrator
    /// to read and edit.
    ///
    /// # Errors
    ///
    /// [`RosterErrorKind::ReadOnly`] from a provider that cannot list them (the default);
    /// otherwise as [`load`](ConfigProvider::load).
    fn definitions(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
    ) -> Result<ConfigDefinitions, RosterError> {
        let _ = (record, scope);
        Err(read_only())
    }

    /// Stores one record's definition under `key` at `scope`, replacing what was there, and
    /// returns the revision the provider moved to. `definition` is a JSON object with the
    /// keys of the record's table in its family file.
    ///
    /// # Errors
    ///
    /// [`RosterErrorKind::ReadOnly`] from a provider that takes no changes (the default);
    /// [`RosterErrorKind::Invalid`], changing nothing, when a configuration it would leave is
    /// not acceptable or the family has no records at that scope;
    /// [`RosterErrorKind::Unavailable`] when the provider fails.
    fn put(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
        key: &str,
        definition: &serde_json::Value,
    ) -> Result<ConfigRevision, RosterError> {
        let _ = (record, scope, key, definition);
        Err(read_only())
    }

    /// Removes one record; `None` when there was none.
    ///
    /// # Errors
    ///
    /// As [`put`](ConfigProvider::put): a removal that leaves a configuration unacceptable
    /// (an Agent another still extends, say) is refused.
    fn remove(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
        key: &str,
    ) -> Result<Option<ConfigRevision>, RosterError> {
        let _ = (record, scope, key);
        Err(read_only())
    }
}

/// Opens the configuration provider at `location`: `sqlite:PATH` (or `sqlite://PATH`) or a
/// `postgres://` URL for a `SqlProvider`, anything else a [`FileProvider`] directory.
///
/// # Errors
///
/// `Unsupported` for a SQL location when this build lacks its cargo feature (`sqlite`,
/// `postgres`); the database's error when it cannot be opened. A directory is not read
/// until it is loaded.
pub fn open(location: &str) -> io::Result<Arc<dyn ConfigProvider>> {
    #[cfg(any(feature = "sqlite", feature = "postgres"))]
    return open_with(
        location,
        &crate::directorates::registry::sql::SqlOptions::default(),
    );
    #[cfg(not(any(feature = "sqlite", feature = "postgres")))]
    if crate::bootstrap::is_sql_location(location) {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this build of ai6 has no SQL configuration support (cargo features `sqlite`, `postgres`)",
        ))
    } else {
        Ok(Arc::new(FileProvider::new(location)))
    }
}

/// As [`open`], connecting a SQL provider with `options` (password, timeouts).
///
/// # Errors
///
/// As [`open`].
#[cfg(any(feature = "sqlite", feature = "postgres"))]
pub fn open_with(
    location: &str,
    options: &crate::directorates::registry::sql::SqlOptions,
) -> io::Result<Arc<dyn ConfigProvider>> {
    if crate::bootstrap::is_sql_location(location) {
        Ok(Arc::new(SqlProvider::open_with(location, options)?))
    } else {
        Ok(Arc::new(FileProvider::new(location)))
    }
}

/// Each family's records by key, at one scope or in one Station's view.
pub(crate) type Layer = BTreeMap<ConfigRecord, BTreeMap<String, serde_json::Value>>;

/// `station`'s records over `global`'s, key by key.
pub(crate) fn merged(global: &Layer, station: &Layer) -> Layer {
    let mut view = global.clone();
    for (record, records) in station {
        let family = view.entry(*record).or_default();
        for (key, definition) in records {
            family.insert(key.clone(), definition.clone());
        }
    }
    view
}

/// Refuses a record a family does not take at `scope`, or a singleton with another key.
pub(crate) fn check_placement(
    record: ConfigRecord,
    scope: &ConfigScope,
    key: &str,
    source: &dyn fmt::Display,
) -> Result<(), RosterError> {
    if *scope != ConfigScope::Global && !record.station_scoped() {
        return Err(error(format!(
            "{source}: {record} records are global; {scope} cannot have one"
        )));
    }
    if record.is_singleton() && key != ConfigRecord::SINGLETON_KEY {
        return Err(error(format!(
            "{source}: the {record} record is keyed {:?}, not {key:?}",
            ConfigRecord::SINGLETON_KEY
        )));
    }
    Ok(())
}

/// Builds and checks the configuration of one view: the roster, every Agent resolved, and the
/// settings. A relative path in the assistant settings is resolved against `base`.
pub(crate) fn assemble(
    revision: ConfigRevision,
    view: &Layer,
    base: Option<&Path>,
    source: &dyn fmt::Display,
) -> Result<ConfigSnapshot, RosterError> {
    let family = |record: ConfigRecord| -> Vec<(String, serde_json::Value)> {
        view.get(&record)
            .map(|records| {
                records
                    .iter()
                    .map(|(key, definition)| (key.clone(), definition.clone()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut covers = Covers::default();
    for (name, definition) in family(ConfigRecord::Cover) {
        covers.insert(
            cover_from(name, definition).map_err(|reason| error(format!("{source}: {reason}")))?,
        );
    }
    let assistant = family(ConfigRecord::Assistant)
        .into_iter()
        .next()
        .map(|(_, definition)| definition);
    let roster = roster_from_definitions(
        family(ConfigRecord::Operation),
        family(ConfigRecord::Agent),
        family(ConfigRecord::Role),
        covers,
        assistant,
        base,
        source,
    )?;
    roster
        .check_agents()
        .map_err(|reason| error(format!("{source}: {reason}")))?;
    let theaters = Theaters::from_records(family(ConfigRecord::Theater))
        .map_err(|reason| error(format!("{source}: {reason}")))?;
    let fingerprints = ConfigRecord::ALL
        .into_iter()
        .map(|record| {
            let text = serde_json::to_string(&view.get(&record)).unwrap_or_default();
            (record, Sha256::digest(text.as_bytes()).into())
        })
        .collect();
    Ok(ConfigSnapshot {
        revision,
        roster,
        settings: Settings { theaters },
        fingerprints,
    })
}

/// Configuration kept in memory, built in code. Its revision counts the replacements.
pub struct MemoryProvider {
    state: Mutex<(u64, Roster, Settings)>,
}

impl fmt::Debug for MemoryProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryProvider")
            .finish_non_exhaustive()
    }
}

impl MemoryProvider {
    /// A provider holding `roster` and default settings at revision 0, the same for every
    /// Station.
    pub fn new(roster: Roster) -> Self {
        Self {
            state: Mutex::new((0, roster, Settings::default())),
        }
    }

    /// This provider with `settings` instead of the defaults.
    #[must_use]
    pub fn with_settings(self, settings: Settings) -> Self {
        let mut state = self
            .state
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        state.2 = settings;
        Self {
            state: Mutex::new(state),
        }
    }

    /// Replaces the roster, keeping the settings; the provider moves to its next revision.
    pub fn replace(&self, roster: Roster) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 += 1;
        state.1 = roster;
    }

    /// Replaces the roster and the settings; the provider moves to its next revision.
    pub fn replace_all(&self, roster: Roster, settings: Settings) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 += 1;
        state.1 = roster;
        state.2 = settings;
    }
}

impl ConfigProvider for MemoryProvider {
    fn load(&self, _station: &str) -> Result<ConfigSnapshot, RosterError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(ConfigSnapshot::new(
            ConfigRevision::new(state.0),
            state.1.clone(),
            state.2.clone(),
        ))
    }

    fn revision(&self, _station: &str) -> Result<ConfigRevision, RosterError> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(ConfigRevision::new(state.0))
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
    fn a_memory_provider_reports_each_replacement_once() {
        let provider = MemoryProvider::new(roster("007"));
        let first = provider.load("").unwrap();
        assert_eq!(first.roster.agent(Some("G"), "007").unwrap().id, "007");
        assert!(
            provider
                .changes_since("", first.revision)
                .unwrap()
                .is_none()
        );

        provider.replace(roster("008"));
        let second = provider.changes_since("", first.revision).unwrap().unwrap();
        assert_ne!(second.revision, first.revision);
        assert!(second.roster.agent(Some("G"), "007").is_err());
        assert_eq!(second.roster.agent(Some("G"), "008").unwrap().id, "008");
        assert!(
            provider
                .changes_since("", second.revision)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn scopes_have_a_text_form_and_families_know_where_they_apply() {
        assert_eq!(ConfigScope::Global.as_text(), "");
        assert_eq!(ConfigScope::of_station("a").as_text(), "station:a");
        assert_eq!(
            ConfigScope::parse("station:a"),
            Some(ConfigScope::of_station("a"))
        );
        assert_eq!(ConfigScope::parse(""), Some(ConfigScope::Global));
        assert_eq!(ConfigScope::parse("station:"), None);
        assert_eq!(ConfigScope::parse("hq"), None);
        assert_eq!(ConfigScope::of_station(""), ConfigScope::Global);
        assert!(ConfigRecord::Theater.station_scoped() && !ConfigRecord::Theater.applies_live());
        assert!(!ConfigRecord::Agent.station_scoped() && ConfigRecord::Agent.applies_live());
        assert_eq!(
            ConfigRecord::from_name("assistant"),
            Some(ConfigRecord::Assistant)
        );
        assert!(
            check_placement(
                ConfigRecord::Agent,
                &ConfigScope::of_station("a"),
                "007",
                &"t"
            )
            .is_err()
        );
        assert!(
            check_placement(
                ConfigRecord::Assistant,
                &ConfigScope::Global,
                "assistant",
                &"t"
            )
            .is_err()
        );
    }
}
