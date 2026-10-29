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
#[cfg(feature = "postgres")]
mod postgres;
mod records;
mod sessions;
mod stations;

use std::fmt;
use std::io;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use db::{Clock, Database, Location, Tx};

/// How a SQL Registry or roster connects to PostgreSQL: a password kept out of the location,
/// and timeouts derived from the Headquarters lease, so a Station that stalls inside a
/// transaction is cut off well before the others need to take over from it.
///
/// From a lease `L`: a statement, a wait for a lock and an idle open transaction are ended
/// after `L/4` (`statement_timeout`, `lock_timeout`, `idle_in_transaction_session_timeout`,
/// set on each session); connecting gives up and TCP keepalives start after `L/6`
/// (`connect_timeout`, `keepalives_idle`), and unacknowledged data drops the connection after
/// `L/4` (`tcp_user_timeout`, where the platform has it). Each is at least a second. A
/// location that sets one of them itself (a parameter, or `-c` in `options`) keeps its own.
/// SQLite ignores these.
#[derive(Clone)]
pub struct SqlOptions {
    lease: Duration,
    password: Option<Zeroizing<String>>,
}

impl fmt::Debug for SqlOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqlOptions")
            .field("lease", &self.lease)
            .field("password", &self.password.as_ref().map(|_| "***"))
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
        }
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
        (self.lease / divisor).max(Duration::from_secs(1))
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

/// A record as stored: compact JSON.
fn to_json(value: &impl Serialize) -> io::Result<String> {
    serde_json::to_string(value).map_err(io::Error::other)
}

/// A record as stored, refused past `limit` bytes as the file Registry refuses it.
fn bounded_json(value: &impl Serialize, limit: u64, what: &str) -> io::Result<String> {
    let text = to_json(value)?;
    if text.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{what} exceeds the size limit"),
        ));
    }
    Ok(text)
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
            Duration::from_secs(1),
            "never under a second"
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

    use db::Value;
}
