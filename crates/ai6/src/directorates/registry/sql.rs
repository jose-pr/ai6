//! The Registry in a SQL database: [`SqlRegistry`], a
//! [`RegistryBackend`](super::backend::RegistryBackend) kept in SQLite (one Station, tests)
//! or PostgreSQL (every Station of an HA deployment, with no shared filesystem).
//!
//! Each record family has its own table, holding the record as JSON text in the shape the
//! file Registry writes, beside the columns a lookup needs. Every write transaction first
//! advances the Registry's clock row, a row lock that serializes writers across Stations as
//! the file Registry's directory lock does; the brief leases, the Headquarters election and
//! Session fencing therefore keep the file Registry's guarantees, terms and fences. Each
//! row carries the revision of the write that last changed it. Leases expire on the
//! database's clock, read in the transaction that writes them, so the Stations' own clocks
//! do not decide who holds a lease.
//!
//! The schema, version 1, is created on first use; a database whose schema version is newer
//! is refused, when it is opened and on every write. Secrets are never stored: access
//! tokens are kept by digest, and credentials stay in Q's secret backends.
//!
//! A PostgreSQL location takes, besides the server's own parameters: `sslmode`
//! (`verify-full`, the default: TLS with the server's certificate checked against the
//! platform's roots, or only `sslrootcert=PATH` when given; `require`: TLS without checking
//! the certificate; `disable`: no TLS). A password not in the location comes from
//! [`SqlOptions::with_password`]. [`weakenings`] names what a location gives up; [`redact`]
//! hides its password for messages.

mod accounts;
mod agents;
mod approvals;
mod briefs;
pub(crate) mod db;
mod holds;
mod memories;
mod mission_issues;
#[cfg(feature = "postgres")]
mod postgres;
mod records;
mod reports;
mod reviews;
mod sealed;
mod sessions;
mod stations;
mod steps;
mod transcripts;
mod watches;

use std::fmt;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use db::{Clock, Database, Location, Tx};

use super::backend::Limited;
use super::limits::RegistryLimits;
use crate::bounds::Limit;

/// How a SQL Registry or roster connects to PostgreSQL: a password kept out of the location,
/// and timeouts derived from the Headquarters lease, so a Station that stalls inside a
/// transaction is cut off well before the others need to take over from it.
///
/// From a lease `L`: a statement, a wait for a lock and an idle open transaction are ended
/// after `L/4` (`statement_timeout`, `lock_timeout`, `idle_in_transaction_session_timeout`,
/// set on each session); connecting gives up and TCP keepalives start after `L/6`
/// (`connect_timeout`, `keepalives_idle`), probe every `L/60` (`keepalives_interval`), and
/// unacknowledged data drops the connection after `L/4` (`tcp_user_timeout`, where the
/// platform has it). A location that sets one of them
/// itself (a parameter, or `-c` in `options`) keeps its own. SQLite ignores these, and waits
/// for another connection's lock for its busy timeout instead.
///
/// The options also carry the [`RegistryLimits`] the opened Registry applies, whichever store
/// the location names.
#[derive(Clone)]
pub struct SqlOptions {
    lease: Duration,
    password: Option<Zeroizing<String>>,
    busy_timeout: Duration,
    limits: RegistryLimits,
}

impl fmt::Debug for SqlOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqlOptions")
            .field("lease", &self.lease)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("busy_timeout", &self.busy_timeout)
            .field("limits", &self.limits)
            .finish()
    }
}

impl Default for SqlOptions {
    /// Timeouts for the default Headquarters lease, [`DEFAULT_LEASE`](super::store::DEFAULT_LEASE).
    fn default() -> Self {
        Self::for_lease(super::store::DEFAULT_LEASE)
    }
}

#[cfg_attr(
    not(feature = "postgres"),
    allow(dead_code, reason = "only PostgreSQL connections read these")
)]
impl SqlOptions {
    /// Timeouts derived from a Headquarters lease of `lease`.
    pub fn for_lease(lease: Duration) -> Self {
        Self {
            lease,
            password: None,
            busy_timeout: crate::bootstrap::DEFAULT_SQLITE_BUSY_TIMEOUT,
            limits: RegistryLimits::default(),
        }
    }

    /// The limits the opened Registry applies. Default [`RegistryLimits::default`].
    #[must_use]
    pub fn with_limits(mut self, limits: RegistryLimits) -> Self {
        self.limits = limits;
        self
    }

    /// How long a SQLite connection waits for a lock another connection holds. Default 30 s.
    #[must_use]
    pub fn with_busy_timeout(mut self, timeout: Duration) -> Self {
        self.busy_timeout = timeout;
        self
    }

    /// The limits the opened Registry applies.
    pub fn limits(&self) -> &RegistryLimits {
        &self.limits
    }

    pub(crate) fn busy_timeout(&self) -> Duration {
        self.busy_timeout
    }

    /// The password to connect with when the location gives none.
    #[must_use]
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(Zeroizing::new(password.into()));
        self
    }

    pub(crate) fn password(&self) -> Option<&str> {
        self.password.as_deref().map(String::as_str)
    }

    fn fraction(&self, divisor: u32) -> Duration {
        self.lease / divisor
    }

    pub(crate) fn statement_timeout(&self) -> Duration {
        self.fraction(4)
    }

    pub(crate) fn lock_timeout(&self) -> Duration {
        self.fraction(4)
    }

    pub(crate) fn idle_in_transaction_timeout(&self) -> Duration {
        self.fraction(4)
    }

    pub(crate) fn connect_timeout(&self) -> Duration {
        self.fraction(6)
    }

    pub(crate) fn keepalives_idle(&self) -> Duration {
        self.fraction(6)
    }

    pub(crate) fn keepalives_interval(&self) -> Duration {
        self.fraction(60)
    }

    pub(crate) fn tcp_user_timeout(&self) -> Duration {
        self.fraction(4)
    }
}

/// What a location asks of TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TlsMode {
    /// Encrypted, with the server's certificate verified against the trusted roots and its
    /// name: the default.
    VerifyFull,
    /// Encrypted, with any certificate accepted.
    Require,
    /// Not encrypted.
    Disable,
}

/// One query parameter of a location, decoded, with the pair as written.
pub(crate) struct Parameter<'a> {
    pub(crate) key: String,
    pub(crate) value: String,
    #[cfg_attr(
        not(feature = "postgres"),
        allow(
            dead_code,
            reason = "only a PostgreSQL connection rebuilds its location"
        )
    )]
    pub(crate) written: &'a str,
}

/// A location before its query, and its query parameters.
pub(crate) fn parameters(location: &str) -> io::Result<(&str, Vec<Parameter<'_>>)> {
    let (base, query) = location.split_once('?').unwrap_or((location, ""));
    let mut pairs = Vec::new();
    for written in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = written.split_once('=').unwrap_or((written, ""));
        pairs.push(Parameter {
            key: decode(key)?,
            value: decode(value)?,
            written,
        });
    }
    Ok((base, pairs))
}

fn decode(text: &str) -> io::Result<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let byte = text
                .get(index + 1..index + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or_else(|| invalid_input("a location has a malformed %-escape"))?;
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| invalid_input("a location is not UTF-8"))
}

pub(crate) fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

/// The TLS a PostgreSQL location asks for.
///
/// # Errors
///
/// `InvalidInput` for an `sslmode` other than `verify-full`, `require` or `disable`.
pub(crate) fn tls_mode(location: &str) -> io::Result<TlsMode> {
    let mut mode = TlsMode::VerifyFull;
    for parameter in parameters(location)?.1 {
        if parameter.key == "sslmode" {
            mode = match parameter.value.as_str() {
                "verify-full" => TlsMode::VerifyFull,
                "require" => TlsMode::Require,
                "disable" => TlsMode::Disable,
                other => {
                    return Err(invalid_input(format!(
                        "sslmode {other:?} is not supported: use verify-full (the default), require or disable"
                    )));
                }
            };
        }
    }
    Ok(mode)
}

fn is_postgres(location: &str) -> bool {
    location.starts_with("postgres://") || location.starts_with("postgresql://")
}

/// `location` with any password it carries replaced by `***`, for messages and logs.
pub fn redact(location: &str) -> String {
    if !is_postgres(location) {
        return location.to_owned();
    }
    let (scheme, rest) = location.split_once("://").unwrap_or(("", location));
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let authority = match authority.rsplit_once('@') {
        Some((userinfo, host)) => match userinfo.split_once(':') {
            Some((user, _)) => format!("{user}:***@{host}"),
            None => authority.to_owned(),
        },
        None => authority.to_owned(),
    };
    let tail = match tail.split_once('?') {
        Some((path, query)) => {
            let query = query
                .split('&')
                .map(|pair| match pair.split_once('=') {
                    Some(("password", _)) => "password=***".to_owned(),
                    _ => pair.to_owned(),
                })
                .collect::<Vec<_>>()
                .join("&");
            format!("{path}?{query}")
        }
        None => tail.to_owned(),
    };
    format!("{scheme}://{authority}{tail}")
}

/// What `location` gives up against the defaults, worded for a log line: a connection that
/// is not encrypted or whose server is not verified, and a password that a process listing
/// shows. Empty for a directory, a SQLite file, or a PostgreSQL location at the defaults.
pub fn weakenings(location: &str) -> Vec<String> {
    if !is_postgres(location) {
        return Vec::new();
    }
    let mut weaker = Vec::new();
    match tls_mode(location) {
        Ok(TlsMode::Disable) => {
            weaker.push("the PostgreSQL connection is not encrypted (sslmode=disable)".to_owned())
        }
        Ok(TlsMode::Require) => weaker.push(
            "the PostgreSQL server's certificate is not verified (sslmode=require)".to_owned(),
        ),
        Ok(TlsMode::VerifyFull) | Err(_) => {}
    }
    if redact(location) != location {
        weaker.push(
            "the PostgreSQL password is in the location, where a process listing shows it; \
             name a password secret in the bootstrap file instead"
                .to_owned(),
        );
    }
    weaker
}

/// Runs `sql` on a session of its own at the PostgreSQL `location`.
#[cfg(all(test, feature = "postgres"))]
pub(crate) fn postgres_admin(location: &str, sql: &str) -> io::Result<()> {
    postgres::admin(location, sql)
}

/// The Registry kept in a SQLite or PostgreSQL database.
///
/// Open one per Station with [`SqlRegistry::open`]; every handle on the same database sees
/// the same records.
#[derive(Debug)]
pub struct SqlRegistry {
    db: Database,
    limits: Arc<RegistryLimits>,
}

impl SqlRegistry {
    /// Opens the Registry at `location` with the default [`SqlOptions`].
    ///
    /// # Errors
    ///
    /// As [`SqlRegistry::open_with`].
    pub fn open(location: &str) -> io::Result<Self> {
        Self::open_with(location, &SqlOptions::default())
    }

    /// Opens the Registry at `location`, creating or upgrading its tables.
    ///
    /// `location` is `sqlite:PATH` (or `sqlite://PATH`) for a SQLite file, or a PostgreSQL
    /// connection URL (`postgres://` or `postgresql://`), whose parameters (such as
    /// `options=-c search_path=…` for a schema) pass to the server, besides those this module
    /// reads (`sslmode`, `sslrootcert`).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a location that is neither or an unsupported `sslmode`,
    /// `Unsupported` when this build lacks that database's cargo feature (`sqlite`,
    /// `postgres`) or the schema is newer than this build, and the database's own error when
    /// it cannot be reached or its certificate is not trusted.
    pub fn open_with(location: &str, options: &SqlOptions) -> io::Result<Self> {
        let location = Location::parse(location).ok_or_else(|| {
            invalid_input("a SQL Registry location starts with sqlite: or postgres://")
        })?;
        Ok(Self {
            db: Database::open(location, options)?,
            limits: Arc::new(options.limits.clone()),
        })
    }

    fn read<T, E: From<io::Error>>(
        &self,
        work: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        self.db.read(work)
    }

    fn write<T, E: From<io::Error>>(
        &self,
        work: impl FnOnce(&mut Tx<'_>, i64) -> Result<T, E>,
    ) -> Result<T, E> {
        self.db.write(Clock::Registry, work)
    }
}

impl Limited for SqlRegistry {
    fn limits(&self) -> &RegistryLimits {
        &self.limits
    }
}

/// A record as stored: compact JSON.
fn to_json(value: &impl Serialize) -> io::Result<String> {
    serde_json::to_string(value).map_err(io::Error::other)
}

/// A record as stored, refused past `limit` bytes as the file Registry refuses it.
fn bounded_json(value: &impl Serialize, limit: Limit, what: &str) -> io::Result<String> {
    let text = to_json(value)?;
    if !limit.allows(text.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{what} exceeds the size limit"),
        ));
    }
    Ok(text)
}

/// `limit` as a SQL row count: `i64::MAX` when there is none.
fn row_limit(limit: Limit) -> i64 {
    limit
        .get()
        .and_then(|bound| i64::try_from(bound).ok())
        .unwrap_or(i64::MAX)
}

/// Reads a stored record; a malformed one is `InvalidData`.
fn from_json<T: DeserializeOwned>(text: &str, what: &str) -> io::Result<T> {
    serde_json::from_str(text).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {what}: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::backend::*;
    use crate::directorates::registry::suite;

    #[test]
    fn a_password_never_reaches_a_message_and_weakenings_are_named() {
        assert_eq!(
            redact("postgres://ai6:s3cret@db.internal:5432/ai6?sslmode=require&password=two"),
            "postgres://ai6:***@db.internal:5432/ai6?sslmode=require&password=***"
        );
        assert_eq!(redact("postgres://ai6@db/ai6"), "postgres://ai6@db/ai6");
        assert_eq!(
            redact("sqlite:C:/state/registry.db"),
            "sqlite:C:/state/registry.db"
        );
        assert!(weakenings("postgres://ai6@db/ai6").is_empty());
        assert!(
            weakenings("postgres://ai6@db/ai6?sslmode=verify-full&sslrootcert=ca.pem").is_empty()
        );
        assert!(weakenings("sqlite:registry.db").is_empty());
        let named = weakenings("postgres://ai6:pw@db/ai6?sslmode=disable");
        assert_eq!(named.len(), 2, "{named:?}");
        assert!(named[0].contains("not encrypted"));
        assert!(named[1].contains("password"));
        assert!(weakenings("postgres://ai6@db/ai6?sslmode=require")[0].contains("not verified"));
        assert_eq!(
            tls_mode("postgres://ai6@db/ai6?sslmode=prefer")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!format!("{:?}", SqlOptions::default().with_password("s3cret")).contains("s3cret"));
    }

    #[test]
    fn timeouts_follow_the_headquarters_lease() {
        let options = SqlOptions::for_lease(Duration::from_secs(60));
        assert_eq!(options.lock_timeout(), Duration::from_secs(15));
        assert_eq!(
            options.idle_in_transaction_timeout(),
            Duration::from_secs(15)
        );
        assert_eq!(options.connect_timeout(), Duration::from_secs(10));
        assert_eq!(
            SqlOptions::for_lease(Duration::from_secs(2)).lock_timeout(),
            Duration::from_millis(500),
            "a short lease gives short timeouts"
        );
    }

    /// Runs `case` on SQLite and, when a server is named, on PostgreSQL, with the location.
    fn on_each_database(case: fn(&str)) {
        #[cfg(feature = "sqlite")]
        case(&suite::SqliteStore::new().location());
        #[cfg(feature = "postgres")]
        match suite::PostgresStore::new() {
            Some(store) => case(&store.location()),
            None => eprintln!("skipped the PostgreSQL run: {}", suite::POSTGRES_URL_ENV),
        }
    }

    #[test]
    fn a_station_older_than_the_schema_stops_writing() {
        on_each_database(|location| {
            let station = SqlRegistry::open(location).unwrap();
            station.check_in("station-a", Default::default()).unwrap();
            // A newer ai6 migrates the database while this Station keeps running.
            station
                .db
                .write(Clock::Registry, |tx, _| {
                    tx.execute(
                        "INSERT INTO ai6_schema (version) VALUES ($1)",
                        &[Value::Int(2)],
                    )
                })
                .unwrap();
            let refused = station
                .check_in("station-a", Default::default())
                .unwrap_err();
            assert_eq!(refused.kind(), io::ErrorKind::Unsupported, "{refused}");
            assert!(
                station.stations(Duration::from_secs(60)).is_ok(),
                "reading goes on"
            );
            assert_eq!(
                SqlRegistry::open(location).unwrap_err().kind(),
                io::ErrorKind::Unsupported
            );
        });
    }

    fn table_answers(registry: &SqlRegistry, table: &str) -> bool {
        registry
            .db
            .read(|tx| tx.query_opt(&format!("SELECT count(*) FROM {table}"), &[]))
            .is_ok()
    }

    /// Takes away one table and one index, as a database a newer build first opens lacks them.
    fn lose_a_table_and_an_index(registry: &SqlRegistry) {
        registry
            .db
            .write(Clock::Registry, |tx, _| {
                tx.execute("DROP TABLE ai6_issue_proposals", &[])?;
                tx.execute("DROP INDEX ai6_timeline_order", &[])
            })
            .unwrap();
    }

    fn clock_revision(registry: &SqlRegistry) -> i64 {
        registry
            .db
            .read(|tx| {
                tx.query_opt(
                    "SELECT revision FROM ai6_clock WHERE name = $1",
                    &["registry".into()],
                )?
                .ok_or_else(|| io::Error::other("no clock row"))?
                .int(0)
            })
            .unwrap()
    }

    #[test]
    fn what_a_database_lacks_is_made_when_it_is_opened_and_nothing_else_changes() {
        on_each_database(|location| {
            let station = SqlRegistry::open(location).unwrap();
            station.check_in("station-a", Default::default()).unwrap();
            lose_a_table_and_an_index(&station);
            assert!(!table_answers(&station, "ai6_issue_proposals"));
            let revision = clock_revision(&station);

            let again = SqlRegistry::open(location).unwrap();
            assert!(table_answers(&again, "ai6_issue_proposals"));
            let objects = again.db.read(db::schema_objects).unwrap();
            assert!(objects.contains("ai6_timeline_order"), "{objects:?}");
            assert_eq!(clock_revision(&again), revision, "the clock keeps counting");
            assert_eq!(again.stations(Duration::from_secs(60)).unwrap().len(), 1);
            let (rows, version) = again
                .db
                .read(|tx| {
                    let row = tx
                        .query_opt("SELECT count(*), max(version) FROM ai6_schema", &[])?
                        .ok_or_else(|| io::Error::other("no row"))?;
                    Ok::<_, io::Error>((row.int(0)?, row.int(1)?))
                })
                .unwrap();
            assert_eq!((rows, version), (1, 1), "still one row of version 1");
        });
    }

    #[test]
    fn stations_opening_together_each_find_the_schema_whole() {
        on_each_database(|location| {
            let open_together = || {
                std::thread::scope(|scope| {
                    let opening: Vec<_> = (0..6)
                        .map(|_| scope.spawn(|| SqlRegistry::open(location).map(|_| ())))
                        .collect();
                    for one in opening {
                        one.join().unwrap().unwrap();
                    }
                });
            };
            open_together();
            let station = SqlRegistry::open(location).unwrap();
            lose_a_table_and_an_index(&station);
            back_before_handles(&station);
            open_together();
            assert!(table_answers(&station, "ai6_issue_proposals"));
            assert_eq!(step_names(&station), every_step());
        });
    }

    /// Puts the handle book back as the schema made it before handles, with no step recorded.
    fn back_before_handles(registry: &SqlRegistry) {
        registry
            .db
            .write(Clock::Registry, |tx, _| {
                tx.execute("DROP TABLE ai6_mission_ids", &[])?;
                tx.execute(
                    "CREATE TABLE ai6_mission_ids (folded TEXT PRIMARY KEY, \
                     mission_id TEXT NOT NULL, entry TEXT UNIQUE, revision BIGINT NOT NULL)",
                    &[],
                )?;
                tx.execute("DELETE FROM ai6_schema_steps", &[])
            })
            .unwrap();
    }

    fn alter(registry: &SqlRegistry, sql: &str) {
        registry
            .db
            .write(Clock::Registry, |tx, _| tx.execute(sql, &[]))
            .unwrap();
    }

    /// The recorded steps, by name, with when each was recorded.
    fn step_records(registry: &SqlRegistry) -> Vec<(String, i64)> {
        registry
            .db
            .read(|tx| {
                tx.query(
                    "SELECT name, applied_micros FROM ai6_schema_steps ORDER BY name",
                    &[],
                )?
                .into_iter()
                .map(|row| Ok((row.text(0)?, row.int(1)?)))
                .collect::<io::Result<Vec<_>>>()
            })
            .unwrap()
    }

    fn step_names(registry: &SqlRegistry) -> Vec<String> {
        step_records(registry)
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// Every step's name, in the order `step_records` lists them.
    fn every_step() -> Vec<String> {
        let mut names: Vec<String> = steps::STEPS
            .iter()
            .map(|step| step.name.to_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_brief_table_from_before_cancel_requests_gains_the_column_and_keeps_its_rows() {
        on_each_database(|location| {
            let old = SqlRegistry::open(location).unwrap();
            let brief: crate::Brief = serde_json::from_value(serde_json::json!({
                "title": "Kept", "id": "kept", "source": "manual", "operation": "G",
                "agent": null, "role": null, "url": null
            }))
            .unwrap();
            old.submit_brief(&brief).unwrap();
            alter(
                &old,
                "ALTER TABLE ai6_briefs DROP COLUMN cancel_requested_at",
            );
            alter(
                &old,
                "DELETE FROM ai6_schema_steps WHERE name = 'briefs.cancel_requested_at'",
            );

            let again = SqlRegistry::open(location).unwrap();
            assert_eq!(step_names(&again), every_step());
            assert_eq!(again.list_new().unwrap()[0].key(), brief.key());
            again
                .claim_with_lease(
                    Some(&brief.key()),
                    "station-a",
                    Duration::from_secs(60),
                    Default::default(),
                )
                .unwrap()
                .unwrap();
            assert!(again.request_cancel(&brief.key(), 10.0).unwrap());
            assert_eq!(
                again.cancel_requests().unwrap(),
                std::collections::BTreeMap::from([(brief.key(), 10.0)])
            );
        });
    }

    #[test]
    fn a_database_from_before_handles_gains_the_column_once() {
        on_each_database(|location| {
            let old = SqlRegistry::open(location).unwrap();
            back_before_handles(&old);
            alter(&old, "DROP TABLE ai6_schema_steps");

            let first = SqlRegistry::open(location).unwrap();
            let recorded = step_records(&first);
            assert_eq!(step_names(&first), every_step());
            let mission = first
                .allot_mission_id(&MissionEntry::person(Some("ops")))
                .unwrap();
            assert_eq!(
                first.resolve_mission(&mission.handle).unwrap(),
                Some(mission)
            );

            let second = SqlRegistry::open(location).unwrap();
            assert_eq!(step_records(&second), recorded, "recorded once");
        });
    }

    #[test]
    fn missions_kept_before_handles_stop_the_step_by_name() {
        on_each_database(|location| {
            let old = SqlRegistry::open(location).unwrap();
            back_before_handles(&old);
            old.db
                .write(Clock::Registry, |tx, revision| {
                    tx.execute(
                        "INSERT INTO ai6_mission_ids (folded, mission_id, entry, revision) \
                         VALUES ('ops-1', 'ops-1', NULL, $1)",
                        &[Value::Int(revision)],
                    )
                })
                .unwrap();
            let refused = SqlRegistry::open(location).unwrap_err();
            assert!(
                refused.to_string().contains("mission_ids.handle"),
                "{refused}"
            );
            assert!(step_records(&old).is_empty(), "nothing is recorded");
        });
    }

    #[test]
    fn a_column_added_by_hand_records_its_step_without_running_it() {
        on_each_database(|location| {
            let old = SqlRegistry::open(location).unwrap();
            back_before_handles(&old);
            alter(
                &old,
                "ALTER TABLE ai6_mission_ids ADD COLUMN handle TEXT NOT NULL",
            );
            let again = SqlRegistry::open(location)
                .expect("the step's own ALTER would fail on a column that exists");
            assert_eq!(step_names(&again), every_step());
        });
    }

    #[test]
    fn a_column_dropped_by_hand_is_refused_by_name_and_an_extra_one_is_not() {
        on_each_database(|location| {
            let station = SqlRegistry::open(location).unwrap();
            alter(
                &station,
                "ALTER TABLE ai6_stations ADD COLUMN added_later TEXT",
            );
            let again = SqlRegistry::open(location).expect("an extra column opens");
            again.check_in("station-a", Default::default()).unwrap();

            alter(&station, "ALTER TABLE ai6_mission_ids DROP COLUMN handle");
            let refused = SqlRegistry::open(location).unwrap_err();
            assert_eq!(refused.kind(), io::ErrorKind::InvalidData, "{refused}");
            assert!(
                refused
                    .to_string()
                    .contains("table ai6_mission_ids lacks column handle"),
                "{refused}"
            );
        });
    }

    #[test]
    fn a_step_runs_once_however_often_the_database_is_opened() {
        // Counts its runs on the roster's clock row.
        const COUNTED: steps::Step = steps::Step {
            name: "test.counted",
            table: "ai6_stations",
            column: "counted",
            #[cfg(feature = "sqlite")]
            sqlite: "ALTER TABLE ai6_stations ADD COLUMN counted TEXT; \
                     UPDATE ai6_clock SET revision = revision + 1 WHERE name = 'config'",
            #[cfg(feature = "postgres")]
            postgres: "ALTER TABLE ai6_stations ADD COLUMN counted TEXT; \
                       UPDATE ai6_clock SET revision = revision + 1 WHERE name = 'config'",
        };
        on_each_database(|location| {
            let open = || {
                Database::open_with_steps(
                    Location::parse(location).unwrap(),
                    &SqlOptions::default(),
                    &[COUNTED],
                )
                .unwrap()
            };
            let runs = |db: &Database| {
                db.read(|tx| {
                    tx.query_opt("SELECT revision FROM ai6_clock WHERE name = 'config'", &[])?
                        .ok_or_else(|| io::Error::other("no clock row"))?
                        .int(0)
                })
                .unwrap()
            };
            let first = open();
            assert_eq!(runs(&first), 1);
            // Without its column, an unrecorded step would run again.
            first
                .write(Clock::Registry, |tx, _| {
                    tx.execute("ALTER TABLE ai6_stations DROP COLUMN counted", &[])
                })
                .unwrap();
            let second = open();
            assert_eq!(runs(&second), 1, "the step ran once");
        });
    }

    /// The server role a deployment runs under when someone else owns the schema: it may read
    /// and write every table and create nothing. Dropped with the value.
    #[cfg(feature = "postgres")]
    struct UnprivilegedRole<'a> {
        store: &'a suite::PostgresStore,
        name: String,
    }

    #[cfg(feature = "postgres")]
    impl<'a> UnprivilegedRole<'a> {
        /// `None` when the server's user may not make roles.
        fn new(store: &'a suite::PostgresStore) -> Option<Self> {
            let name = format!("ai6_role_{}", uuid::Uuid::new_v4().simple());
            match store.admin(&format!("CREATE ROLE {name} LOGIN PASSWORD 'unprivileged'")) {
                Ok(()) => Some(Self { store, name }),
                Err(error) => {
                    eprintln!("skipped: the server's user cannot create a role: {error}");
                    None
                }
            }
        }

        fn grant(&self) {
            let (schema, name) = (self.store.schema(), &self.name);
            self.store
                .admin(&format!(
                    "GRANT USAGE ON SCHEMA {schema} TO {name}; \
                     GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA {schema} TO {name}"
                ))
                .unwrap();
        }

        fn location(&self) -> String {
            self.store.location_as(&self.name, "unprivileged")
        }
    }

    #[cfg(feature = "postgres")]
    impl Drop for UnprivilegedRole<'_> {
        fn drop(&mut self) {
            let name = &self.name;
            let _ = self
                .store
                .admin(&format!("DROP OWNED BY {name} CASCADE; DROP ROLE {name}"));
        }
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn a_role_that_cannot_create_starts_on_a_whole_database_and_is_told_what_is_missing() {
        let Some(store) = suite::PostgresStore::new() else {
            eprintln!("skipped a PostgreSQL test: {}", suite::POSTGRES_URL_ENV);
            return;
        };
        let owner = SqlRegistry::open(&store.location()).unwrap();
        let Some(role) = UnprivilegedRole::new(&store) else {
            return;
        };
        role.grant();

        let station = SqlRegistry::open(&role.location())
            .expect("nothing is missing, so no DDL right is needed");
        station.check_in("station-a", Default::default()).unwrap();
        crate::directorates::m::config::SqlProvider::open(&role.location())
            .expect("the roster on the same database too");

        lose_a_table_and_an_index(&owner);
        let refused = SqlRegistry::open(&role.location()).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied, "{refused}");
        let said = refused.to_string();
        assert!(said.contains(&role.name), "names the role: {said}");
        assert!(
            said.contains("ai6_issue_proposals"),
            "names the table: {said}"
        );
        assert!(
            said.contains("ai6_timeline_order"),
            "names the index: {said}"
        );
        assert!(said.contains(store.schema()), "names the schema: {said}");

        // The owner makes them; the role then only needs its grants on what is new.
        let again = SqlRegistry::open(&store.location()).unwrap();
        assert!(table_answers(&again, "ai6_issue_proposals"));
        role.grant();
        SqlRegistry::open(&role.location()).unwrap();

        // A step alters a table, which only its owner may: the role is told which step.
        back_before_handles(&owner);
        role.grant();
        let refused = SqlRegistry::open(&role.location()).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied, "{refused}");
        let said = refused.to_string();
        assert!(
            said.contains("mission_ids.handle"),
            "names the step: {said}"
        );
        assert!(said.contains(&role.name), "names the role: {said}");
        SqlRegistry::open(&store.location()).unwrap();
        SqlRegistry::open(&role.location()).unwrap();
    }

    #[test]
    fn every_step_adds_a_column_both_schemas_make() {
        let names: std::collections::BTreeSet<_> = steps::STEPS.iter().map(|s| s.name).collect();
        assert_eq!(names.len(), steps::STEPS.len(), "each name once");
        for schema in [
            include_str!("sql/schema/postgres.sql"),
            include_str!("sql/schema/sqlite.sql"),
        ] {
            let statements = db::statements(schema).unwrap();
            for step in steps::STEPS {
                let made = statements
                    .iter()
                    .find(|s| s.creates.as_deref() == Some(step.table))
                    .and_then(|s| s.columns.as_ref());
                assert!(
                    made.is_some_and(|columns| columns.iter().any(|c| c == step.column)),
                    "{} names a column its table is made with",
                    step.name
                );
            }
        }
    }

    #[test]
    fn the_two_schemas_name_the_same_tables_and_indexes() {
        let names = |schema: &str| -> std::collections::BTreeSet<String> {
            db::statements(schema)
                .unwrap()
                .iter()
                .filter_map(|statement| statement.creates.clone())
                .collect()
        };
        let postgres = names(include_str!("sql/schema/postgres.sql"));
        assert_eq!(postgres, names(include_str!("sql/schema/sqlite.sql")));
        assert!(postgres.contains("ai6_schema"), "{postgres:?}");
        assert!(postgres.contains("ai6_timeline_order"), "{postgres:?}");
        for schema in [
            include_str!("sql/schema/postgres.sql"),
            include_str!("sql/schema/sqlite.sql"),
        ] {
            let statements = db::statements(schema).unwrap();
            let seeds = statements.iter().filter(|s| s.creates.is_none()).count();
            assert_eq!(seeds, 1, "one statement fills the clock");
            assert_eq!(statements[0].creates.as_deref(), Some("ai6_schema"));
            let columns = |table: &str| {
                statements
                    .iter()
                    .find(|s| s.creates.as_deref() == Some(table))
                    .and_then(|s| s.columns.clone())
                    .unwrap()
            };
            assert_eq!(
                columns("ai6_mission_proposals"),
                [
                    "operation",
                    "id",
                    "state",
                    "proposal",
                    "created_at",
                    "revision"
                ],
                "table constraints are not columns"
            );
            assert!(
                statements
                    .iter()
                    .filter(|s| s.creates.as_deref() == Some("ai6_timeline_order"))
                    .all(|s| s.columns.is_none()),
                "an index has no columns of its own"
            );
        }
        assert_eq!(
            db::statements(include_str!("sql/schema/sqlite.sql"))
                .unwrap()
                .iter()
                .find(|s| s.creates.as_deref() == Some("ai6_memories_search"))
                .and_then(|s| s.columns.clone())
                .unwrap(),
            ["id", "words"]
        );
    }

    use db::Value;
}
