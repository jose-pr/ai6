//! The roster kept in a SQL database, so every Station of an HA deployment reads the same
//! Operations, Agents, roles, covers and Moneypenny's settings.

use std::fmt;
use std::io;

use super::{ConfigDefinitions, ConfigRecord, ConfigRevision, ConfigSnapshot, ConfigSource};
use crate::directorates::m::cover::{Covers, cover_from};
use crate::directorates::m::roster::{
    RosterError, RosterErrorKind, error, error_of, roster_from_definitions,
};
use crate::directorates::registry::sql::SqlOptions;
use crate::directorates::registry::sql::db::{Clock, Database, Location, Tx};

/// The tables of each kind: `ai6_config_operations`, `ai6_config_agents`, `ai6_config_roles`,
/// `ai6_config_covers` and `ai6_config_assistant`.
fn select(record: ConfigRecord) -> &'static str {
    match record {
        ConfigRecord::Operation => "SELECT id, definition FROM ai6_config_operations ORDER BY id",
        ConfigRecord::Agent => "SELECT id, definition FROM ai6_config_agents ORDER BY id",
        ConfigRecord::Role => "SELECT id, definition FROM ai6_config_roles ORDER BY id",
        ConfigRecord::Cover => "SELECT name, definition FROM ai6_config_covers ORDER BY name",
        ConfigRecord::Assistant => {
            "SELECT name, definition FROM ai6_config_assistant ORDER BY name"
        }
    }
}

fn upsert(record: ConfigRecord) -> &'static str {
    match record {
        ConfigRecord::Operation => {
            "INSERT INTO ai6_config_operations (id, definition, revision) VALUES ($1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET definition = excluded.definition, revision = excluded.revision"
        }
        ConfigRecord::Agent => {
            "INSERT INTO ai6_config_agents (id, definition, revision) VALUES ($1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET definition = excluded.definition, revision = excluded.revision"
        }
        ConfigRecord::Role => {
            "INSERT INTO ai6_config_roles (id, definition, revision) VALUES ($1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET definition = excluded.definition, revision = excluded.revision"
        }
        ConfigRecord::Cover => {
            "INSERT INTO ai6_config_covers (name, definition, revision) VALUES ($1, $2, $3) \
             ON CONFLICT (name) DO UPDATE SET definition = excluded.definition, revision = excluded.revision"
        }
        ConfigRecord::Assistant => {
            "INSERT INTO ai6_config_assistant (name, definition, revision) VALUES ($1, $2, $3) \
             ON CONFLICT (name) DO UPDATE SET definition = excluded.definition, revision = excluded.revision"
        }
    }
}

fn delete(record: ConfigRecord) -> &'static str {
    match record {
        ConfigRecord::Operation => "DELETE FROM ai6_config_operations WHERE id = $1",
        ConfigRecord::Agent => "DELETE FROM ai6_config_agents WHERE id = $1",
        ConfigRecord::Role => "DELETE FROM ai6_config_roles WHERE id = $1",
        ConfigRecord::Cover => "DELETE FROM ai6_config_covers WHERE name = $1",
        ConfigRecord::Assistant => "DELETE FROM ai6_config_assistant WHERE name = $1",
    }
}

/// The `config` row of `ai6_clock`, which every change advances.
fn clock(tx: &mut Tx<'_>) -> io::Result<i64> {
    tx.query_opt("SELECT revision FROM ai6_clock WHERE name = 'config'", &[])?
        .map_or(Ok(0), |row| row.int(0))
}

/// Every row of one roster table.
fn table(tx: &mut Tx<'_>, record: ConfigRecord) -> io::Result<Vec<(String, String)>> {
    tx.query(select(record), &[])?
        .into_iter()
        .map(|row| Ok((row.text(0)?, row.text(1)?)))
        .collect()
}

/// Every row of the roster tables, as read in one transaction.
struct Rows {
    revision: i64,
    tables: [Vec<(String, String)>; ConfigRecord::ALL.len()],
}

fn rows(tx: &mut Tx<'_>) -> io::Result<Rows> {
    // The revision is read first: a write landing between the statements shows up as a
    // change on the next poll rather than being missed.
    let revision = clock(tx)?;
    let mut tables: [Vec<(String, String)>; ConfigRecord::ALL.len()] = Default::default();
    for (record, rows) in ConfigRecord::ALL.into_iter().zip(tables.iter_mut()) {
        *rows = table(tx, record)?;
    }
    Ok(Rows { revision, tables })
}

const SOURCE: &str = "SQL configuration";

fn parsed(rows: Vec<(String, String)>) -> Result<Vec<(String, serde_json::Value)>, RosterError> {
    rows.into_iter()
        .map(|(key, text)| {
            serde_json::from_str::<serde_json::Value>(&text)
                .map(|definition| (key.clone(), definition))
                .map_err(|reason| error(format!("{SOURCE}: {key:?}: {reason}")))
        })
        .collect()
}

fn revision_of(number: i64) -> Result<ConfigRevision, RosterError> {
    u64::try_from(number)
        .map(ConfigRevision::new)
        .map_err(|_| error(format!("{SOURCE}: invalid revision")))
}

fn roster(rows: Rows) -> Result<ConfigSnapshot, RosterError> {
    let [operations, agents, roles, covers, assistant] = rows.tables.map(parsed);
    let mut policies = Covers::default();
    for (name, definition) in covers? {
        policies.insert(
            cover_from(name, definition).map_err(|reason| error(format!("{SOURCE}: {reason}")))?,
        );
    }
    let mut settings = None;
    for (key, definition) in assistant? {
        if key != ConfigRecord::ASSISTANT_KEY {
            return Err(error(format!(
                "{SOURCE}: the assistant record is keyed {:?}, not {key:?}",
                ConfigRecord::ASSISTANT_KEY
            )));
        }
        settings = Some(definition);
    }
    let roster =
        roster_from_definitions(operations?, agents?, roles?, policies, settings, &SOURCE)?;
    Ok(ConfigSnapshot::new(revision_of(rows.revision)?, roster))
}

fn failed(reason: io::Error) -> RosterError {
    error_of(RosterErrorKind::Unavailable, format!("{SOURCE}: {reason}"))
}

/// The roster kept in a SQLite or PostgreSQL database: one table per record kind, each row a
/// record's definition as a JSON object with the keys of its table in `operations.toml`,
/// `agents.toml` or `covers.toml`, built into the roster with [`Roster::from_parts`](crate::Roster::from_parts).
/// Moneypenny's settings are one row keyed `assistant` holding the keys of `[assistant]`;
/// without it the roster has the default settings.
///
/// Its revision is the `config` row of `ai6_clock`, which every
/// [`put`](ConfigSource::put) and [`remove`](ConfigSource::remove) advances; a writer outside
/// AI6 advances it in the same transaction, or Stations do not see the change. The database
/// can be the Registry's.
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
        if self.revision()? == since {
            return Ok(None);
        }
        self.load().map(Some)
    }

    fn revision(&self) -> Result<ConfigRevision, RosterError> {
        revision_of(self.db.read(clock).map_err(failed)?)
    }

    fn definitions(&self, record: ConfigRecord) -> Result<ConfigDefinitions, RosterError> {
        let (revision, rows) = self
            .db
            .read(|tx| Ok((clock(tx)?, table(tx, record)?)))
            .map_err(failed)?;
        Ok(ConfigDefinitions {
            revision: revision_of(revision)?,
            records: parsed(rows)?,
            writable: true,
        })
    }

    fn put(
        &self,
        record: ConfigRecord,
        key: &str,
        definition: &serde_json::Value,
    ) -> Result<ConfigRevision, RosterError> {
        if key.trim().is_empty() || !definition.is_object() {
            return Err(error(format!(
                "{SOURCE}: a {} needs a key and a JSON object",
                record.name()
            )));
        }
        let text = definition.to_string();
        self.change(|tx, revision| {
            Ok(tx.execute(upsert(record), &[key.into(), text.into(), revision.into()])? > 0)
        })?
        .ok_or_else(|| {
            error_of(
                RosterErrorKind::Unavailable,
                format!("{SOURCE}: the {} {key:?} was not stored", record.name()),
            )
        })
    }

    fn remove(
        &self,
        record: ConfigRecord,
        key: &str,
    ) -> Result<Option<ConfigRevision>, RosterError> {
        self.change(|tx, _| Ok(tx.execute(delete(record), &[key.into()])? > 0))
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
        assert_eq!(station_a.revision().unwrap(), seeded.revision);
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
                .select_agent(
                    &crate::directorates::m::roster::AgentQuery::new()
                        .operation(Some("G"))
                        .role(Some("case-officer"))
                )
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

        let agents = station_a.definitions(ConfigRecord::Agent).unwrap();
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
            let refused = source.put(record, key, &definition).unwrap_err();
            assert_eq!(
                refused.kind(),
                RosterErrorKind::Invalid,
                "{record:?} {key} {definition}: {refused}"
            );
        }
        let refused = source.remove(ConfigRecord::Agent, "007").unwrap_err();
        assert!(refused.to_string().contains("007"), "{refused}");
        assert_eq!(refused.kind(), RosterErrorKind::Invalid);
        assert!(source.changes_since(good.revision).unwrap().is_none());
    }

    fn an_assistant_row_gives_the_roster_its_settings(location: &str) {
        let source = SqlConfigSource::open(location).unwrap();
        seed(&source);
        let seeded = source.load().unwrap();
        assert_eq!(
            seeded.roster.assistant(),
            &crate::AssistantSettings::default()
        );
        assert!(
            source
                .definitions(ConfigRecord::Assistant)
                .unwrap()
                .records
                .is_empty()
        );

        let settings = json!({
            "personal_repo": "/srv/mp/{person_digest}.git",
            "allow_elevated": true,
            "shared": {"ops": {"repo": "/srv/ops.git", "write": true}},
        });
        source
            .put(ConfigRecord::Assistant, "assistant", &settings)
            .unwrap();
        let other_station = SqlConfigSource::open(location).unwrap();
        let set = other_station
            .changes_since(seeded.revision)
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
                .definitions(ConfigRecord::Assistant)
                .unwrap()
                .records,
            [("assistant".to_owned(), settings)]
        );

        for (key, definition, names) in [
            ("moneypenny", json!({}), "\"assistant\""),
            (
                "assistant",
                json!({"personal_repo": "ext::sh"}),
                "assistant.personal_repo",
            ),
            ("assistant", json!({"allow_elevated": "yes"}), "assistant"),
            ("assistant", json!({"unknown": true}), "unknown"),
            (
                "assistant",
                json!({"shared": {"org": {"repo": "/srv/org.git"}}}),
                "assistant.shared",
            ),
        ] {
            let refused = source
                .put(ConfigRecord::Assistant, key, &definition)
                .unwrap_err();
            assert_eq!(refused.kind(), RosterErrorKind::Invalid, "{refused}");
            assert!(refused.to_string().contains(names), "{refused}");
        }
        assert!(source.changes_since(set.revision).unwrap().is_none());

        assert!(
            source
                .remove(ConfigRecord::Assistant, "assistant")
                .unwrap()
                .is_some()
        );
        let removed = source.changes_since(set.revision).unwrap().unwrap();
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
    fn an_unacceptable_sql_roster_change_is_refused_and_changes_nothing() {
        run(an_unacceptable_change_is_refused_and_changes_nothing);
    }

    #[test]
    fn an_assistant_row_gives_a_sql_roster_its_settings() {
        run(an_assistant_row_gives_the_roster_its_settings);
    }
}
