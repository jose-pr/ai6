//! The roster kept in a SQL database, so every Station of an HA deployment reads the same
//! Operations, Agents, roles and covers.

use std::fmt;
use std::io;

use super::{ConfigRevision, ConfigSnapshot, ConfigSource};
use crate::directorates::m::cover::{Covers, cover_from};
use crate::directorates::m::roster::{RosterError, error, roster_from_definitions};
use crate::directorates::registry::sql::SqlOptions;
use crate::directorates::registry::sql::db::{Clock, Database, Location, Tx};

/// The kinds of roster record a [`SqlConfigSource`] keeps, one table each.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigRecord {
    /// An Operation: the keys of an `[operation.<id>]` table (`ai6_config_operations`).
    Operation,
    /// An Agent: the keys of an `[agent.<codename>]` table (`ai6_config_agents`).
    Agent,
    /// An Agent role: the keys of a `[role.<id>]` table (`ai6_config_roles`).
    Role,
    /// A cover policy: the keys of a `[cover.<name>]` table (`ai6_config_covers`).
    Cover,
}

impl ConfigRecord {
    const ALL: [Self; 4] = [Self::Operation, Self::Agent, Self::Role, Self::Cover];

    fn select(self) -> &'static str {
        match self {
            Self::Operation => "SELECT id, definition FROM ai6_config_operations ORDER BY id",
            Self::Agent => "SELECT id, definition FROM ai6_config_agents ORDER BY id",
            Self::Role => "SELECT id, definition FROM ai6_config_roles ORDER BY id",
            Self::Cover => "SELECT name, definition FROM ai6_config_covers ORDER BY name",
        }
    }

    fn upsert(self) -> &'static str {
        match self {
            Self::Operation => {
                "INSERT INTO ai6_config_operations (id, definition, revision) VALUES ($1, $2, $3) \
                 ON CONFLICT (id) DO UPDATE SET definition = excluded.definition, revision = excluded.revision"
            }
            Self::Agent => {
                "INSERT INTO ai6_config_agents (id, definition, revision) VALUES ($1, $2, $3) \
                 ON CONFLICT (id) DO UPDATE SET definition = excluded.definition, revision = excluded.revision"
            }
            Self::Role => {
                "INSERT INTO ai6_config_roles (id, definition, revision) VALUES ($1, $2, $3) \
                 ON CONFLICT (id) DO UPDATE SET definition = excluded.definition, revision = excluded.revision"
            }
            Self::Cover => {
                "INSERT INTO ai6_config_covers (name, definition, revision) VALUES ($1, $2, $3) \
                 ON CONFLICT (name) DO UPDATE SET definition = excluded.definition, revision = excluded.revision"
            }
        }
    }

    fn delete(self) -> &'static str {
        match self {
            Self::Operation => "DELETE FROM ai6_config_operations WHERE id = $1",
            Self::Agent => "DELETE FROM ai6_config_agents WHERE id = $1",
            Self::Role => "DELETE FROM ai6_config_roles WHERE id = $1",
            Self::Cover => "DELETE FROM ai6_config_covers WHERE name = $1",
        }
    }
}

/// Every row of the roster tables, as read in one transaction.
struct Rows {
    revision: i64,
    tables: [Vec<(String, String)>; 4],
}

fn rows(tx: &mut Tx<'_>) -> io::Result<Rows> {
    // The revision is read first: a write landing between the statements shows up as a
    // change on the next poll rather than being missed.
    let revision = tx
        .query_opt("SELECT revision FROM ai6_clock WHERE name = 'config'", &[])?
        .map_or(Ok(0), |row| row.int(0))?;
    let mut tables: [Vec<(String, String)>; 4] = Default::default();
    for (record, table) in ConfigRecord::ALL.into_iter().zip(tables.iter_mut()) {
        for row in tx.query(record.select(), &[])? {
            table.push((row.text(0)?, row.text(1)?));
        }
    }
    Ok(Rows { revision, tables })
}

const SOURCE: &str = "SQL configuration";

fn roster(rows: Rows) -> Result<ConfigSnapshot, RosterError> {
    let [operations, agents, roles, covers] = rows.tables.map(|table| {
        table
            .into_iter()
            .map(|(key, text)| {
                serde_json::from_str::<serde_json::Value>(&text)
                    .map(|definition| (key.clone(), definition))
                    .map_err(|reason| error(format!("{SOURCE}: {key:?}: {reason}")))
            })
            .collect::<Result<Vec<_>, _>>()
    });
    let mut policies = Covers::default();
    for (name, definition) in covers? {
        policies.insert(
            cover_from(name, definition).map_err(|reason| error(format!("{SOURCE}: {reason}")))?,
        );
    }
    let roster = roster_from_definitions(operations?, agents?, roles?, policies, &SOURCE)?;
    let revision =
        u64::try_from(rows.revision).map_err(|_| error(format!("{SOURCE}: invalid revision")))?;
    Ok(ConfigSnapshot::new(ConfigRevision::new(revision), roster))
}

fn failed(reason: io::Error) -> RosterError {
    error(format!("{SOURCE}: {reason}"))
}

/// The roster kept in a SQLite or PostgreSQL database: one table per record kind, each row a
/// record's definition as a JSON object with the keys of its table in `operations.toml`,
/// `agents.toml` or `covers.toml`, built into the roster with [`Roster::from_parts`](crate::Roster::from_parts).
///
/// Its revision is the `config` row of `ai6_clock`, which every [`put`](Self::put) and
/// [`remove`](Self::remove) advances; a writer outside AI6 advances it in the same
/// transaction, or Stations do not see the change. The database can be the Registry's.
pub struct SqlConfigSource {
    db: Database,
}

impl fmt::Debug for SqlConfigSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqlConfigSource")
            .finish_non_exhaustive()
    }
}

impl SqlConfigSource {
    /// Opens the roster at `location` with the default [`SqlOptions`].
    ///
    /// # Errors
    ///
    /// As [`SqlConfigSource::open_with`].
    pub fn open(location: &str) -> io::Result<Self> {
        Self::open_with(location, &SqlOptions::default())
    }

    /// Opens the roster at `location` (`sqlite:PATH` or a `postgres://` URL, read as
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

    /// Stores one record's definition under `key`, replacing what was there, and returns the
    /// revision the roster moved to.
    ///
    /// # Errors
    ///
    /// Refuses, changing nothing, a definition that is not a JSON object or leaves the roster
    /// unacceptable (the checks [`Roster::from_parts`](crate::Roster::from_parts) makes); or when the database fails.
    pub fn put(
        &self,
        record: ConfigRecord,
        key: &str,
        definition: &serde_json::Value,
    ) -> Result<ConfigRevision, RosterError> {
        if key.trim().is_empty() || !definition.is_object() {
            return Err(error(format!(
                "{SOURCE}: a {record:?} needs a key and a JSON object"
            )));
        }
        let text = definition.to_string();
        self.change(|tx, revision| {
            Ok(tx.execute(record.upsert(), &[key.into(), text.into(), revision.into()])? > 0)
        })?
        .ok_or_else(|| error(format!("{SOURCE}: the {record:?} {key:?} was not stored")))
    }

    /// Removes one record; `None` when there was none.
    ///
    /// # Errors
    ///
    /// Refuses, changing nothing, a removal that leaves the roster unacceptable (an Agent
    /// still extended by another, say); or when the database fails.
    pub fn remove(
        &self,
        record: ConfigRecord,
        key: &str,
    ) -> Result<Option<ConfigRevision>, RosterError> {
        self.change(|tx, _| Ok(tx.execute(record.delete(), &[key.into()])? > 0))
    }

    /// Applies `change` and checks the roster it leaves, all in one transaction, which is
    /// rolled back when nothing changed or the roster is not acceptable.
    fn change(
        &self,
        change: impl FnOnce(&mut Tx<'_>, i64) -> io::Result<bool> + Send,
    ) -> Result<Option<ConfigRevision>, RosterError> {
        let outcome = self.db.write(Clock::Config, |tx, revision| {
            if !change(tx, revision)? {
                return Err(io::Error::new(io::ErrorKind::NotFound, "unchanged"));
            }
            roster(rows(tx)?)
                .map(|snapshot| snapshot.revision)
                .map_err(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason.to_string()))
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

impl ConfigSource for SqlConfigSource {
    fn load(&self) -> Result<ConfigSnapshot, RosterError> {
        roster(self.db.read(rows).map_err(failed)?)
    }

    fn changes_since(&self, since: ConfigRevision) -> Result<Option<ConfigSnapshot>, RosterError> {
        let revision = self
            .db
            .read(|tx| {
                tx.query_opt("SELECT revision FROM ai6_clock WHERE name = 'config'", &[])?
                    .map_or(Ok(0), |row| row.int(0))
            })
            .map_err(failed)?;
        if u64::try_from(revision).ok() == Some(since.get()) {
            return Ok(None);
        }
        self.load().map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn seed(source: &SqlConfigSource) {
        source
            .put(
                ConfigRecord::Operation,
                "G",
                &json!({"roster": ["007", "008"]}),
            )
            .unwrap();
        source
            .put(
                ConfigRecord::Role,
                "case-officer",
                &json!({"purpose": "Runs a Mission end to end.", "policy": "Report to M."}),
            )
            .unwrap();
        source
            .put(
                ConfigRecord::Agent,
                "007",
                &json!({"handler": "mock-agent", "roles": ["case-officer"], "priority": 5}),
            )
            .unwrap();
        source
            .put(
                ConfigRecord::Agent,
                "008",
                &json!({"extends": "007", "priority": 1}),
            )
            .unwrap();
        source
            .put(
                ConfigRecord::Cover,
                "house",
                &json!({"description": "House style.", "trailers": ["Assisted-by"]}),
            )
            .unwrap();
    }

    fn a_roster_is_built_from_typed_rows_and_shared_by_revision(location: &str) {
        let station_a = SqlConfigSource::open(location).unwrap();
        let empty = station_a.load().unwrap();
        assert_eq!(empty.roster.operations().count(), 0);
        assert!(station_a.changes_since(empty.revision).unwrap().is_none());

        seed(&station_a);
        let station_b = SqlConfigSource::open(location).unwrap();
        let seeded = station_b.changes_since(empty.revision).unwrap().unwrap();
        assert_ne!(seeded.revision, empty.revision);
        assert_eq!(seeded.roster.operation("G").unwrap().roster, ["007", "008"]);
        let inherited = seeded.roster.agent(Some("G"), "008").unwrap();
        assert_eq!(
            (inherited.handler.as_deref(), inherited.priority),
            (Some("mock-agent"), 1),
            "008 extends 007"
        );
        assert_eq!(
            seeded
                .roster
                .select_agent(Some("G"), Some("case-officer"), &[], None)
                .unwrap()
                .id,
            "007"
        );
        assert!(seeded.roster.cover("house").is_ok());
        assert!(
            seeded.roster.cover("covert").is_ok(),
            "built-in covers stay"
        );
        assert!(station_a.changes_since(seeded.revision).unwrap().is_none());
        assert_eq!(station_a.load().unwrap().revision, seeded.revision);

        assert!(
            station_b
                .remove(ConfigRecord::Cover, "house")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            station_b.remove(ConfigRecord::Cover, "house").unwrap(),
            None
        );
        let changed = station_a.changes_since(seeded.revision).unwrap().unwrap();
        assert!(changed.roster.cover("house").is_err());
    }

    fn an_unacceptable_change_is_refused_and_changes_nothing(location: &str) {
        let source = SqlConfigSource::open(location).unwrap();
        seed(&source);
        let good = source.load().unwrap();
        for (record, key, definition) in [
            (ConfigRecord::Agent, "009", json!({"clearance": "root"})),
            (ConfigRecord::Operation, "L", json!({"on_blown": "shrug"})),
            (ConfigRecord::Operation, "M", json!({"repos": ["x"]})),
            (
                ConfigRecord::Role,
                "spy",
                json!({"purpose": "", "policy": "p"}),
            ),
            (ConfigRecord::Agent, "010", json!({"extends": "nobody"})),
            (ConfigRecord::Cover, "odd", json!({"unknown": true})),
            (ConfigRecord::Agent, "011", json!("not an object")),
        ] {
            assert!(
                source.put(record, key, &definition).is_err(),
                "{record:?} {key} {definition}"
            );
        }
        let refused = source.remove(ConfigRecord::Agent, "007").unwrap_err();
        assert!(refused.to_string().contains("007"), "{refused}");
        assert!(source.changes_since(good.revision).unwrap().is_none());
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
    fn an_unacceptable_sql_roster_change_is_refused_and_changes_nothing() {
        run(an_unacceptable_change_is_refused_and_changes_nothing);
    }
}
