//! Configuration kept in a SQL database, so every Station of an HA deployment reads the same
//! records: [`SqlProvider`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;

use super::{
    ConfigDefinitions, ConfigProvider, ConfigRecord, ConfigRevision, ConfigScope, ConfigSnapshot,
    Layer, assemble, check_placement, merged,
};
use crate::directorates::m::roster::{RosterError, RosterErrorKind, error, error_of};
use crate::directorates::registry::sql::SqlOptions;
use crate::directorates::registry::sql::db::{Clock, Database, Location, Tx};

const SOURCE: &str = "SQL configuration";

/// The table of a family: `ai6_config_<name>`, from a fixed list of names.
fn table(record: ConfigRecord) -> String {
    format!("ai6_config_{}", record.name())
}

/// The `config` row of `ai6_clock`, which every change advances.
fn clock(tx: &mut Tx<'_>) -> io::Result<i64> {
    tx.query_opt("SELECT revision FROM ai6_clock WHERE name = 'config'", &[])?
        .map_or(Ok(0), |row| row.int(0))
}

/// One stored record: its scope's text form, key and definition text.
type Row = (String, String, String);

/// Every row of one family's table.
fn family(tx: &mut Tx<'_>, record: ConfigRecord) -> io::Result<Vec<Row>> {
    let sql = format!(
        "SELECT scope, key, definition FROM {} ORDER BY scope, key",
        table(record)
    );
    tx.query(&sql, &[])?
        .into_iter()
        .map(|row| Ok((row.text(0)?, row.text(1)?, row.text(2)?)))
        .collect()
}

/// Every record, as read in one transaction.
struct Records {
    revision: i64,
    rows: Vec<(ConfigRecord, Row)>,
}

fn records(tx: &mut Tx<'_>) -> io::Result<Records> {
    // The revision is read first: a write landing between the statements shows up as a
    // change on the next poll rather than being missed.
    let revision = clock(tx)?;
    let mut rows = Vec::new();
    for record in ConfigRecord::ALL {
        rows.extend(family(tx, record)?.into_iter().map(|row| (record, row)));
    }
    Ok(Records { revision, rows })
}

/// The records by scope.
fn layers(records: &Records) -> Result<BTreeMap<ConfigScope, Layer>, RosterError> {
    let mut layers: BTreeMap<ConfigScope, Layer> = BTreeMap::new();
    for (record, (scope, key, text)) in &records.rows {
        let scope = ConfigScope::parse(scope)
            .ok_or_else(|| error(format!("{SOURCE}: {record} {key:?}: scope {scope:?}")))?;
        check_placement(*record, &scope, key, &SOURCE)?;
        let definition = serde_json::from_str::<serde_json::Value>(text)
            .map_err(|reason| error(format!("{SOURCE}: {record} {key:?}: {reason}")))?;
        layers
            .entry(scope)
            .or_default()
            .entry(*record)
            .or_default()
            .insert(key.clone(), definition);
    }
    Ok(layers)
}

fn revision_of(number: i64) -> Result<ConfigRevision, RosterError> {
    u64::try_from(number)
        .map(ConfigRevision::new)
        .map_err(|_| error(format!("{SOURCE}: invalid revision")))
}

/// `station`'s view (`""`: global only), checked and built.
fn snapshot(records: &Records, station: &str) -> Result<ConfigSnapshot, RosterError> {
    let layers = layers(records)?;
    let global = layers
        .get(&ConfigScope::Global)
        .cloned()
        .unwrap_or_default();
    let view = match layers.get(&ConfigScope::of_station(station)) {
        Some(own) if !station.is_empty() => merged(&global, own),
        _ => global,
    };
    assemble(revision_of(records.revision)?, &view, None, &SOURCE)
}

fn failed(reason: io::Error) -> RosterError {
    error_of(RosterErrorKind::Unavailable, format!("{SOURCE}: {reason}"))
}

/// Configuration kept in a SQLite or PostgreSQL database: one table per family,
/// `ai6_config_<name>` ([`ConfigRecord::name`]), each row a record's scope (`""` or
/// `station:<id>`), key and definition, a JSON object with the keys of its table in the
/// family file. A singleton family's one record is keyed
/// [`ConfigRecord::SINGLETON_KEY`].
///
/// Its revision is the `config` row of `ai6_clock`, which every
/// [`put`](ConfigProvider::put) and [`remove`](ConfigProvider::remove) advances; a writer
/// outside AI6 advances it in the same transaction, or Stations do not see the change. A
/// change is checked in that transaction against every view it touches: the global one and
/// each Station's with records of its own for a global change, the Station's for one of its
/// own. The database can be the Registry's.
pub struct SqlProvider {
    db: Database,
}

impl fmt::Debug for SqlProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqlProvider")
            .finish_non_exhaustive()
    }
}

impl SqlProvider {
    /// Opens the configuration at `location` with the default [`SqlOptions`].
    ///
    /// # Errors
    ///
    /// As [`SqlProvider::open_with`].
    pub fn open(location: &str) -> io::Result<Self> {
        Self::open_with(location, &SqlOptions::default())
    }

    /// Opens the configuration at `location` (`sqlite:PATH` or a `postgres://` URL, read as
    /// [`SqlRegistry::open_with`](crate::directorates::registry::sql::SqlRegistry::open_with)
    /// reads it), creating or upgrading its tables.
    ///
    /// # Errors
    ///
    /// As [`SqlRegistry::open_with`](crate::directorates::registry::sql::SqlRegistry::open_with).
    pub fn open_with(location: &str, options: &SqlOptions) -> io::Result<Self> {
        let location = Location::parse(location).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "a SQL configuration location starts with sqlite: or postgres://",
            )
        })?;
        Ok(Self {
            db: Database::open(location, options)?,
        })
    }

    /// Applies `change` and checks every view at `scope` it touches, all in one
    /// transaction, which is rolled back when nothing changed or a view is not acceptable.
    fn change(
        &self,
        scope: &ConfigScope,
        change: impl FnOnce(&mut Tx<'_>, i64) -> io::Result<bool> + Send,
    ) -> Result<Option<ConfigRevision>, RosterError> {
        let scope = scope.clone();
        let outcome = self.db.write(Clock::Config, move |tx, revision| {
            if !change(tx, revision)? {
                return Err(io::Error::new(io::ErrorKind::NotFound, "unchanged"));
            }
            let records = records(tx)?;
            let invalid = |reason: RosterError| {
                io::Error::new(io::ErrorKind::InvalidInput, reason.to_string())
            };
            let stations: BTreeSet<String> = match &scope {
                ConfigScope::Station(id) => [id.clone()].into(),
                ConfigScope::Global => records
                    .rows
                    .iter()
                    .filter_map(|(_, (scope, _, _))| match ConfigScope::parse(scope) {
                        Some(ConfigScope::Station(id)) => Some(id),
                        _ => None,
                    })
                    .chain([String::new()])
                    .collect(),
            };
            let mut revision = None;
            for station in stations {
                revision = Some(snapshot(&records, &station).map_err(invalid)?.revision);
            }
            Ok(revision.unwrap_or_default())
        });
        match outcome {
            Ok(revision) => Ok(Some(revision)),
            Err(reason) if reason.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(reason) if reason.kind() == io::ErrorKind::InvalidInput => {
                Err(error(reason.to_string()))
            }
            Err(reason) => Err(failed(reason)),
        }
    }
}

impl ConfigProvider for SqlProvider {
    fn load(&self, station: &str) -> Result<ConfigSnapshot, RosterError> {
        snapshot(&self.db.read(records).map_err(failed)?, station)
    }

    fn changes_since(
        &self,
        station: &str,
        since: ConfigRevision,
    ) -> Result<Option<ConfigSnapshot>, RosterError> {
        if self.revision(station)? == since {
            return Ok(None);
        }
        self.load(station).map(Some)
    }

    fn revision(&self, _station: &str) -> Result<ConfigRevision, RosterError> {
        revision_of(self.db.read(clock).map_err(failed)?)
    }

    fn definitions(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
    ) -> Result<ConfigDefinitions, RosterError> {
        let (revision, rows) = self
            .db
            .read(|tx| Ok((clock(tx)?, family(tx, record)?)))
            .map_err(failed)?;
        let wanted = scope.as_text();
        let records = rows
            .into_iter()
            .filter(|(scope, _, _)| *scope == wanted)
            .map(|(_, key, text)| {
                serde_json::from_str::<serde_json::Value>(&text)
                    .map(|definition| (key.clone(), definition))
                    .map_err(|reason| error(format!("{SOURCE}: {record} {key:?}: {reason}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ConfigDefinitions {
            revision: revision_of(revision)?,
            records,
            writable: true,
        })
    }

    fn put(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
        key: &str,
        definition: &serde_json::Value,
    ) -> Result<ConfigRevision, RosterError> {
        if key.trim().is_empty() || !definition.is_object() {
            return Err(error(format!(
                "{SOURCE}: a {record} needs a key and a JSON object"
            )));
        }
        check_placement(record, scope, key, &SOURCE)?;
        let text = definition.to_string();
        let sql = format!(
            "INSERT INTO {} (scope, key, definition, revision) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (scope, key) DO UPDATE SET definition = excluded.definition, \
             revision = excluded.revision",
            table(record)
        );
        let scope_text = scope.as_text();
        self.change(scope, move |tx, revision| {
            Ok(tx.execute(
                &sql,
                &[scope_text.into(), key.into(), text.into(), revision.into()],
            )? > 0)
        })?
        .ok_or_else(|| {
            error_of(
                RosterErrorKind::Unavailable,
                format!("{SOURCE}: the {record} {key:?} was not stored"),
            )
        })
    }

    fn remove(
        &self,
        record: ConfigRecord,
        scope: &ConfigScope,
        key: &str,
    ) -> Result<Option<ConfigRevision>, RosterError> {
        let sql = format!(
            "DELETE FROM {} WHERE scope = $1 AND key = $2",
            table(record)
        );
        let scope_text = scope.as_text();
        self.change(scope, move |tx, _| {
            Ok(tx.execute(&sql, &[scope_text.into(), key.into()])? > 0)
        })
    }
    /// Stores every record in one transaction, checked once at the end.
    fn put_all(
        &self,
        scope: &ConfigScope,
        records: &[(ConfigRecord, String, serde_json::Value)],
    ) -> Result<Option<ConfigRevision>, RosterError> {
        for (record, key, definition) in records {
            if key.trim().is_empty() || !definition.is_object() {
                return Err(error(format!(
                    "{SOURCE}: a {record} needs a key and a JSON object"
                )));
            }
            check_placement(*record, scope, key, &SOURCE)?;
        }
        let scope_text = scope.as_text();
        self.change(scope, move |tx, revision| {
            let mut changed = false;
            for (record, key, definition) in records {
                let sql = format!(
                    "INSERT INTO {} (scope, key, definition, revision) VALUES ($1, $2, $3, $4) \
                     ON CONFLICT (scope, key) DO UPDATE SET definition = excluded.definition, \
                     revision = excluded.revision",
                    table(*record)
                );
                changed |= tx.execute(
                    &sql,
                    &[
                        scope_text.clone().into(),
                        key.as_str().into(),
                        definition.to_string().into(),
                        revision.into(),
                    ],
                )? > 0;
            }
            Ok(changed)
        })
    }

    fn scopes(&self) -> Result<Vec<ConfigScope>, RosterError> {
        let records = self.db.read(records).map_err(failed)?;
        let mut scopes: BTreeSet<ConfigScope> = [ConfigScope::Global].into();
        for (_, (scope, _, _)) in &records.rows {
            if let Some(scope) = ConfigScope::parse(scope) {
                scopes.insert(scope);
            }
        }
        Ok(scopes.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const GLOBAL: ConfigScope = ConfigScope::Global;

    fn seed(provider: &SqlProvider) {
        for (record, key, definition) in [
            (
                ConfigRecord::Operation,
                "G",
                json!({"roster": ["007", "008"]}),
            ),
            (
                ConfigRecord::Role,
                "case-officer",
                json!({"purpose": "Runs a Mission end to end.", "policy": "Report to M."}),
            ),
            (
                ConfigRecord::Agent,
                "007",
                json!({"handler": "mock-agent", "roles": ["case-officer"], "priority": 5}),
            ),
            (
                ConfigRecord::Agent,
                "008",
                json!({"extends": "007", "priority": 1}),
            ),
            (
                ConfigRecord::Cover,
                "house",
                json!({"description": "House style.", "trailers": ["Assisted-by"]}),
            ),
        ] {
            provider.put(record, &GLOBAL, key, &definition).unwrap();
        }
    }

    fn a_roster_is_built_from_typed_rows_and_shared_by_revision(location: &str) {
        let station_a = SqlProvider::open(location).unwrap();
        let empty = station_a.load("").unwrap();
        assert_eq!(empty.roster.operations().count(), 0);
        assert!(
            station_a
                .changes_since("", empty.revision)
                .unwrap()
                .is_none()
        );

        seed(&station_a);
        let station_b = SqlProvider::open(location).unwrap();
        let seeded = station_b
            .changes_since("b", empty.revision)
            .unwrap()
            .unwrap();
        assert_ne!(seeded.revision, empty.revision);
        assert_eq!(station_a.revision("a").unwrap(), seeded.revision);
        assert_eq!(seeded.roster.operation("G").unwrap().roster, ["007", "008"]);
        let inherited = seeded.roster.agent(Some("G"), "008").unwrap();
        assert_eq!(
            (inherited.handler.as_deref(), inherited.priority),
            (Some("mock-agent"), 1),
            "008 extends 007"
        );
        assert!(seeded.roster.cover("house").is_ok());
        assert!(
            seeded.roster.cover("covert").is_ok(),
            "built-in covers stay"
        );
        assert!(
            station_a
                .changes_since("a", seeded.revision)
                .unwrap()
                .is_none()
        );

        let agents = station_a.definitions(ConfigRecord::Agent, &GLOBAL).unwrap();
        assert_eq!(agents.revision, seeded.revision);
        assert_eq!(
            agents.records,
            [
                (
                    "007".to_owned(),
                    json!({"handler": "mock-agent", "roles": ["case-officer"], "priority": 5})
                ),
                ("008".to_owned(), json!({"extends": "007", "priority": 1})),
            ]
        );

        assert!(
            station_b
                .remove(ConfigRecord::Cover, &GLOBAL, "house")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            station_b
                .remove(ConfigRecord::Cover, &GLOBAL, "house")
                .unwrap(),
            None
        );
        let changed = station_a
            .changes_since("a", seeded.revision)
            .unwrap()
            .unwrap();
        assert!(changed.roster.cover("house").is_err());
    }

    fn a_tree_of_operations_loads_the_same_through_sql_files_and_code(location: &str) {
        let provider = SqlProvider::open(location).unwrap();
        let checkout = json!({"path": "/srv/fortitude-south"});
        assert!(
            provider
                .put(
                    ConfigRecord::Operation,
                    &GLOBAL,
                    "SOUTH",
                    &json!({"parent": "FORTITUDE"})
                )
                .is_err(),
            "its parent is not stored yet"
        );
        for (key, definition) in [
            (
                "BODYGUARD",
                json!({"station_labels": {"site": "london"}, "green_ink": {"preset": "licensed"}}),
            ),
            ("FORTITUDE", json!({"parent": "BODYGUARD"})),
            ("SOUTH", {
                let mut south = checkout.clone();
                south["parent"] = json!("FORTITUDE");
                south
            }),
        ] {
            provider
                .put(ConfigRecord::Operation, &GLOBAL, key, &definition)
                .unwrap();
        }
        let from_sql = provider.load("").unwrap().roster;
        assert_eq!(from_sql.lineage("SOUTH"), ["FORTITUDE", "BODYGUARD"]);
        let south = from_sql.operation("SOUTH").unwrap();
        assert_eq!(south.station_labels["site"], "london");
        assert_eq!(
            south.origins().get("green_ink.preset").map(String::as_str),
            Some("BODYGUARD")
        );

        let refused = provider
            .remove(ConfigRecord::Operation, &GLOBAL, "FORTITUDE")
            .unwrap_err();
        assert!(
            refused.to_string().contains("not a registered Operation"),
            "{refused}"
        );
        let shallow = json!({"max_operation_depth": 2});
        let key = ConfigRecord::SINGLETON_KEY;
        assert!(
            provider
                .put(ConfigRecord::Roster, &GLOBAL, key, &shallow)
                .is_err()
        );
        provider
            .put(
                ConfigRecord::Roster,
                &GLOBAL,
                key,
                &json!({"max_operation_depth": "none"}),
            )
            .unwrap();
        let unbounded = provider.load("").unwrap().roster;
        assert_eq!(unbounded.settings().max_operation_depth, None);

        let dir = tempfile::tempdir().unwrap();
        crate::directorates::m::config::export(&provider, dir.path()).unwrap();
        let from_files = super::super::FileProvider::new(dir.path())
            .load("")
            .unwrap()
            .roster;
        let from_code = crate::Roster::builder()
            .operation(crate::Operation::new("BODYGUARD"))
            .operation(crate::Operation::new("FORTITUDE").with_parent("BODYGUARD"))
            .operation({
                let mut south = crate::Operation::new("SOUTH").with_parent("FORTITUDE");
                south.path = Some("/srv/fortitude-south".into());
                south
            })
            .max_operation_depth(None)
            .build()
            .unwrap();
        for id in ["BODYGUARD", "FORTITUDE", "SOUTH"] {
            assert_eq!(from_files.operation(id), unbounded.operation(id), "{id}");
        }
        // Records built in code take nothing from their parent: only the tree is the same.
        for roster in [&from_files, &from_code] {
            assert_eq!(roster.settings(), unbounded.settings());
            assert_eq!(roster.subtree("BODYGUARD"), unbounded.subtree("BODYGUARD"));
            assert_eq!(roster.lineage("SOUTH"), unbounded.lineage("SOUTH"));
        }
    }

    fn an_unacceptable_change_is_refused_and_changes_nothing(location: &str) {
        let provider = SqlProvider::open(location).unwrap();
        seed(&provider);
        let good = provider.load("").unwrap();
        for (record, scope, key, definition) in [
            (
                ConfigRecord::Agent,
                GLOBAL,
                "009",
                json!({"clearance": "root"}),
            ),
            (
                ConfigRecord::Operation,
                GLOBAL,
                "L",
                json!({"on_blown": "shrug"}),
            ),
            (
                ConfigRecord::Operation,
                GLOBAL,
                "M",
                json!({"repos": ["x"]}),
            ),
            (
                ConfigRecord::Role,
                GLOBAL,
                "spy",
                json!({"purpose": "", "policy": "p"}),
            ),
            (
                ConfigRecord::Agent,
                GLOBAL,
                "010",
                json!({"extends": "nobody"}),
            ),
            (ConfigRecord::Cover, GLOBAL, "odd", json!({"unknown": true})),
            (ConfigRecord::Agent, GLOBAL, "011", json!("not an object")),
            (
                ConfigRecord::Agent,
                ConfigScope::of_station("a"),
                "012",
                json!({"handler": "mock-agent"}),
            ),
            (
                ConfigRecord::Theater,
                ConfigScope::of_station("a"),
                "box",
                json!({"access": {"carrier-pigeon": {}}}),
            ),
        ] {
            let refused = provider.put(record, &scope, key, &definition).unwrap_err();
            assert_eq!(
                refused.kind(),
                RosterErrorKind::Invalid,
                "{record:?} {key} {definition}: {refused}"
            );
        }
        let refused = provider
            .remove(ConfigRecord::Agent, &GLOBAL, "007")
            .unwrap_err();
        assert!(refused.to_string().contains("007"), "{refused}");
        assert_eq!(refused.kind(), RosterErrorKind::Invalid);
        assert!(provider.changes_since("", good.revision).unwrap().is_none());
    }

    fn a_station_scoped_record_changes_only_that_station(location: &str) {
        let provider = SqlProvider::open(location).unwrap();
        let theater = |host: &str| json!({"access": {"ssh": {"host": host}}});
        provider
            .put(
                ConfigRecord::Theater,
                &GLOBAL,
                "box",
                &theater("global.example"),
            )
            .unwrap();
        provider
            .put(
                ConfigRecord::Theater,
                &ConfigScope::of_station("a"),
                "box",
                &theater("a.example"),
            )
            .unwrap();
        let host = |station: &str| {
            let snapshot = provider.load(station).unwrap();
            format!(
                "{:?}",
                snapshot.settings.theaters.find("box").unwrap().access
            )
        };
        assert!(host("a").contains("a.example"));
        assert!(host("b").contains("global.example"));
        assert_eq!(
            provider
                .definitions(ConfigRecord::Theater, &ConfigScope::of_station("a"))
                .unwrap()
                .records
                .len(),
            1
        );
        // A global change is checked against Station a's view too.
        let refused = provider
            .put(
                ConfigRecord::Operation,
                &GLOBAL,
                "G",
                &json!({"theater": 3}),
            )
            .unwrap_err();
        assert_eq!(refused.kind(), RosterErrorKind::Invalid);
    }

    fn an_assistant_row_gives_the_roster_its_settings(location: &str) {
        let provider = SqlProvider::open(location).unwrap();
        seed(&provider);
        let seeded = provider.load("").unwrap();
        assert_eq!(
            seeded.roster.assistant(),
            &crate::AssistantSettings::default()
        );
        assert!(
            provider
                .definitions(ConfigRecord::Assistant, &GLOBAL)
                .unwrap()
                .records
                .is_empty()
        );

        let settings = json!({
            "personal_repo": "/srv/mp/{person_digest}.git",
            "allow_elevated": true,
            "shared": {"ops": {"repo": "/srv/ops.git", "write": true}},
        });
        let key = ConfigRecord::SINGLETON_KEY;
        provider
            .put(ConfigRecord::Assistant, &GLOBAL, key, &settings)
            .unwrap();
        let other_station = SqlProvider::open(location).unwrap();
        let set = other_station
            .changes_since("", seeded.revision)
            .unwrap()
            .unwrap();
        let assistant = set.roster.assistant();
        assert!(assistant.allow_elevated());
        assert_eq!(
            assistant.personal_repo(),
            Some("/srv/mp/{person_digest}.git")
        );
        assert!(assistant.shared()["ops"].write());
        assert_eq!(assistant.weakenings().len(), 2, "{assistant:?}");
        assert_eq!(
            other_station
                .definitions(ConfigRecord::Assistant, &GLOBAL)
                .unwrap()
                .records,
            [(key.to_owned(), settings)]
        );

        for (key, definition, names) in [
            ("moneypenny", json!({}), "\"settings\""),
            (
                key,
                json!({"personal_repo": "ext::sh"}),
                "assistant.personal_repo",
            ),
            (key, json!({"allow_elevated": "yes"}), "assistant"),
            (key, json!({"unknown": true}), "unknown"),
            (
                key,
                json!({"shared": {"org": {"repo": "/srv/org.git"}}}),
                "assistant.shared",
            ),
        ] {
            let refused = provider
                .put(ConfigRecord::Assistant, &GLOBAL, key, &definition)
                .unwrap_err();
            assert_eq!(refused.kind(), RosterErrorKind::Invalid, "{refused}");
            assert!(refused.to_string().contains(names), "{refused}");
        }
        assert!(provider.changes_since("", set.revision).unwrap().is_none());

        assert!(
            provider
                .remove(ConfigRecord::Assistant, &GLOBAL, key)
                .unwrap()
                .is_some()
        );
        let removed = provider.changes_since("", set.revision).unwrap().unwrap();
        assert_eq!(
            removed.roster.assistant(),
            &crate::AssistantSettings::default()
        );
    }

    fn run(case: fn(&str)) {
        #[cfg(feature = "sqlite")]
        {
            let store = crate::directorates::registry::suite::SqliteStore::new();
            case(&store.location());
        }
        #[cfg(feature = "postgres")]
        match crate::directorates::registry::suite::PostgresStore::new() {
            Some(store) => case(&store.location()),
            None => eprintln!(
                "skipped the PostgreSQL run: {} is not set",
                crate::directorates::registry::suite::POSTGRES_URL_ENV
            ),
        }
    }

    #[test]
    fn a_sql_roster_is_built_from_typed_rows_and_shared_by_revision() {
        run(a_roster_is_built_from_typed_rows_and_shared_by_revision);
    }

    #[test]
    fn a_tree_of_operations_loads_the_same_through_every_provider() {
        run(a_tree_of_operations_loads_the_same_through_sql_files_and_code);
    }

    #[test]
    fn an_unacceptable_sql_change_is_refused_and_changes_nothing() {
        run(an_unacceptable_change_is_refused_and_changes_nothing);
    }

    #[test]
    fn a_sql_station_scoped_record_changes_only_that_station() {
        run(a_station_scoped_record_changes_only_that_station);
    }

    #[test]
    fn an_assistant_row_gives_a_sql_roster_its_settings() {
        run(an_assistant_row_gives_the_roster_its_settings);
    }
}
