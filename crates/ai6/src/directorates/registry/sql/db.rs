//! One connection to SQLite or PostgreSQL behind one transaction API.
//!
//! Statements are written once, with PostgreSQL's `$1` parameters; SQLite reads them as its
//! own numbered `?1` parameters. Values cross as [`Value`]s and come back as [`Row`]s, so
//! the record code above names no driver type.
//!
//! Every write transaction starts by advancing its clock row in `ai6_clock`. That row lock
//! serializes writers across processes, as the file Registry's directory lock does, and the
//! new number is the revision the transaction stamps on each row it writes. On PostgreSQL,
//! writes run `READ COMMITTED` whatever the server's default, so each statement after the
//! lock sees what the previous holder committed, and reads run `REPEATABLE READ READ ONLY`,
//! so a read of several tables sees one commit.

use std::io;
#[cfg(feature = "sqlite")]
use std::sync::{Mutex, MutexGuard, PoisonError};
#[cfg(feature = "sqlite")]
use std::time::Duration;

use super::SqlOptions;
#[cfg(feature = "postgres")]
use super::postgres::{self, block_on};

/// The schema version this build writes; a newer database is refused.
const SCHEMA_VERSION: i64 = 1;

#[cfg(feature = "sqlite")]
const SQLITE_SCHEMA: &str = include_str!("schema/sqlite.sql");
#[cfg(feature = "postgres")]
const POSTGRES_SCHEMA: &str = include_str!("schema/postgres.sql");

/// How long a SQLite writer waits for another process's write to finish. Generous: a slow
/// disk or a busy CI runner must not turn into a refused write.
#[cfg(feature = "sqlite")]
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(30);

/// With the schema's name, serializes concurrent migrations of one PostgreSQL schema.
#[cfg(feature = "postgres")]
const MIGRATION_LOCK: i64 = 0x0a16_5c4e;

/// Which clock row a write transaction advances.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Clock {
    /// The Registry's records.
    Registry,
    /// The roster's records.
    Config,
}

impl Clock {
    fn name(self) -> &'static str {
        match self {
            Self::Registry => "registry",
            Self::Config => "config",
        }
    }
}

/// A parameter or column value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Bytes(Vec<u8>),
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Self::Text(value.clone())
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Self::Int(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Real(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<&[u8]> for Value {
    fn from(value: &[u8]) -> Self {
        Self::Bytes(value.to_vec())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

/// One result row.
#[derive(Debug, Clone)]
pub(crate) struct Row(pub(super) Vec<Value>);

impl Row {
    fn get(&self, index: usize) -> io::Result<&Value> {
        self.0
            .get(index)
            .ok_or_else(|| io::Error::other(format!("no column {index} in the result")))
    }

    pub(crate) fn text(&self, index: usize) -> io::Result<String> {
        self.opt_text(index)?
            .ok_or_else(|| invalid(format!("column {index} is NULL")))
    }

    pub(crate) fn opt_text(&self, index: usize) -> io::Result<Option<String>> {
        match self.get(index)? {
            Value::Null => Ok(None),
            Value::Text(text) => Ok(Some(text.clone())),
            other => Err(invalid(format!("column {index} is not text: {other:?}"))),
        }
    }

    pub(crate) fn int(&self, index: usize) -> io::Result<i64> {
        match self.get(index)? {
            Value::Int(value) => Ok(*value),
            other => Err(invalid(format!(
                "column {index} is not an integer: {other:?}"
            ))),
        }
    }

    pub(crate) fn real(&self, index: usize) -> io::Result<f64> {
        match self.get(index)? {
            Value::Real(value) => Ok(*value),
            other => Err(invalid(format!(
                "column {index} is not a number: {other:?}"
            ))),
        }
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// A database a SQL Registry or roster is kept in, with its schema in place.
pub(crate) struct Database {
    kind: Kind,
}

enum Kind {
    #[cfg(feature = "sqlite")]
    Sqlite(Mutex<rusqlite::Connection>),
    #[cfg(feature = "postgres")]
    Postgres(Box<postgres::Connection>),
}

impl std::fmt::Debug for Database {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Database")
            .field("dialect", &self.dialect())
            .finish_non_exhaustive()
    }
}

/// The SQL dialect of a [`Database`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    #[cfg(feature = "sqlite")]
    Sqlite,
    #[cfg(feature = "postgres")]
    Postgres,
}

/// Where a database is: the path of a SQLite file or a PostgreSQL connection string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Location<'a> {
    Sqlite(&'a str),
    Postgres(&'a str),
}

impl<'a> Location<'a> {
    /// Reads `sqlite:PATH`, `sqlite://PATH`, `postgres://…` and `postgresql://…`; `None`
    /// for anything else, which callers treat as a directory.
    pub(crate) fn parse(text: &'a str) -> Option<Self> {
        if let Some(path) = text
            .strip_prefix("sqlite://")
            .or_else(|| text.strip_prefix("sqlite:"))
        {
            return Some(Self::Sqlite(path));
        }
        if text.starts_with("postgres://") || text.starts_with("postgresql://") {
            return Some(Self::Postgres(text));
        }
        None
    }
}

impl Database {
    /// Opens the database at `location` and brings its schema up to date.
    pub(crate) fn open(location: Location<'_>, options: &SqlOptions) -> io::Result<Self> {
        let database = match location {
            Location::Sqlite(path) => Self::open_sqlite(path)?,
            Location::Postgres(url) => Self::open_postgres(url, options)?,
        };
        database.migrate()?;
        Ok(database)
    }

    #[cfg(feature = "sqlite")]
    fn open_sqlite(path: &str) -> io::Result<Self> {
        if path.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a sqlite: location needs a file path",
            ));
        }
        let connection = rusqlite::Connection::open(path).map_err(sqlite_error)?;
        connection
            .busy_timeout(SQLITE_BUSY_TIMEOUT)
            .map_err(sqlite_error)?;
        // WAL lets readers in other processes carry on while one Station writes.
        connection
            .execute_batch("PRAGMA journal_mode = WAL;")
            .map_err(sqlite_error)?;
        Ok(Self {
            kind: Kind::Sqlite(Mutex::new(connection)),
        })
    }

    #[cfg(not(feature = "sqlite"))]
    fn open_sqlite(_path: &str) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this build of ai6 has no SQLite support (cargo feature `sqlite`)",
        ))
    }

    #[cfg(feature = "postgres")]
    fn open_postgres(url: &str, options: &SqlOptions) -> io::Result<Self> {
        Ok(Self {
            kind: Kind::Postgres(Box::new(postgres::Connection::open(url, options)?)),
        })
    }

    #[cfg(not(feature = "postgres"))]
    fn open_postgres(_url: &str, _options: &SqlOptions) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this build of ai6 has no PostgreSQL support (cargo feature `postgres`)",
        ))
    }

    pub(crate) fn dialect(&self) -> Dialect {
        match &self.kind {
            #[cfg(feature = "sqlite")]
            Kind::Sqlite(_) => Dialect::Sqlite,
            #[cfg(feature = "postgres")]
            Kind::Postgres { .. } => Dialect::Postgres,
        }
    }

    /// Runs `work` in a transaction that writes nothing.
    pub(crate) fn read<T, E: From<io::Error>>(
        &self,
        work: impl FnOnce(&mut Tx<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        self.transaction(Mode::Read, |tx, _| work(tx))
    }

    /// Runs `work` in a write transaction holding `clock`, passing the revision to stamp on
    /// written rows. An error from `work` rolls everything back.
    pub(crate) fn write<T, E: From<io::Error>>(
        &self,
        clock: Clock,
        work: impl FnOnce(&mut Tx<'_>, i64) -> Result<T, E>,
    ) -> Result<T, E> {
        self.transaction(Mode::Write(clock), work)
    }

    fn transaction<T, E: From<io::Error>>(
        &self,
        mode: Mode,
        work: impl FnOnce(&mut Tx<'_>, i64) -> Result<T, E>,
    ) -> Result<T, E> {
        match &self.kind {
            #[cfg(feature = "sqlite")]
            Kind::Sqlite(connection) => {
                let mut connection = lock(connection);
                // A write starts IMMEDIATE so it holds SQLite's write lock from its first
                // read: a read-then-write never meets another writer half-way.
                let behavior = match mode {
                    Mode::Read => rusqlite::TransactionBehavior::Deferred,
                    Mode::Write(_) | Mode::Migrate => rusqlite::TransactionBehavior::Immediate,
                };
                let transaction = connection
                    .transaction_with_behavior(behavior)
                    .map_err(sqlite_error)?;
                let mut tx = Tx {
                    inner: TxInner::Sqlite(transaction),
                };
                let result = run_in(&mut tx, mode, work);
                finish(tx, result)
            }
            #[cfg(feature = "postgres")]
            Kind::Postgres(connection) => {
                use tokio_postgres::IsolationLevel;
                let mut slot = connection.lock();
                let client = connection.connected(&mut slot)?;
                let builder = match mode {
                    Mode::Read => client
                        .build_transaction()
                        .isolation_level(IsolationLevel::RepeatableRead)
                        .read_only(true),
                    Mode::Write(_) | Mode::Migrate => client
                        .build_transaction()
                        .isolation_level(IsolationLevel::ReadCommitted),
                };
                let transaction = block_on(builder.start()).map_err(postgres::error)?;
                let mut tx = Tx {
                    inner: TxInner::Postgres(transaction),
                };
                let result = run_in(&mut tx, mode, work);
                finish(tx, result)
            }
        }
    }

    fn migrate(&self) -> io::Result<()> {
        let dialect = self.dialect();
        let schema = match dialect {
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => SQLITE_SCHEMA,
            #[cfg(feature = "postgres")]
            Dialect::Postgres => POSTGRES_SCHEMA,
        };
        self.transaction(Mode::Migrate, |tx, _| {
            // Two Stations opening a new database migrate it once: SQLite's IMMEDIATE
            // transaction and this lock make the second wait for the first.
            #[cfg(feature = "postgres")]
            if dialect == Dialect::Postgres {
                tx.execute(
                    "SELECT pg_advisory_xact_lock($1, hashtext(current_schema()))",
                    &[Value::Int(MIGRATION_LOCK)],
                )?;
            }
            tx.batch("CREATE TABLE IF NOT EXISTS ai6_schema (version BIGINT NOT NULL)")?;
            let current = schema_version(tx)?;
            check_schema(current)?;
            if current < 1 {
                tx.batch(schema)?;
                tx.execute(
                    "INSERT INTO ai6_schema (version) VALUES ($1)",
                    &[Value::Int(SCHEMA_VERSION)],
                )?;
            }
            Ok(())
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum Mode {
    Read,
    Write(Clock),
    Migrate,
}

fn run_in<T, E: From<io::Error>>(
    tx: &mut Tx<'_>,
    mode: Mode,
    work: impl FnOnce(&mut Tx<'_>, i64) -> Result<T, E>,
) -> Result<T, E> {
    let revision = match mode {
        Mode::Write(clock) => {
            let revision = tx
                .query_opt(
                    "UPDATE ai6_clock SET revision = revision + 1 WHERE name = $1 RETURNING revision",
                    &[clock.name().into()],
                )?
                .ok_or_else(|| invalid(format!("the {} clock row is missing", clock.name())))?
                .int(0)?;
            // A newer ai6 may have migrated the database since this handle opened it; this
            // build must not write records in a shape it no longer knows.
            check_schema(schema_version(tx)?)?;
            revision
        }
        Mode::Read | Mode::Migrate => 0,
    };
    work(tx, revision)
}

fn schema_version(tx: &mut Tx<'_>) -> io::Result<i64> {
    match tx.query_opt("SELECT max(version) FROM ai6_schema", &[])? {
        Some(row) if row.get(0)? != &Value::Null => row.int(0),
        _ => Ok(0),
    }
}

fn check_schema(version: i64) -> io::Result<()> {
    if version > SCHEMA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "the database schema is version {version}; this ai6 knows versions up to {SCHEMA_VERSION}"
            ),
        ));
    }
    Ok(())
}

fn finish<T, E: From<io::Error>>(tx: Tx<'_>, result: Result<T, E>) -> Result<T, E> {
    match result {
        Ok(value) => {
            tx.commit()?;
            Ok(value)
        }
        // Dropping the transaction rolls it back.
        Err(error) => Err(error),
    }
}

#[cfg(feature = "sqlite")]
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An open transaction.
pub(crate) struct Tx<'a> {
    inner: TxInner<'a>,
}

enum TxInner<'a> {
    #[cfg(feature = "sqlite")]
    Sqlite(rusqlite::Transaction<'a>),
    #[cfg(feature = "postgres")]
    Postgres(tokio_postgres::Transaction<'a>),
}

impl Tx<'_> {
    /// The database's clock, in Unix seconds: one clock for every Station's leases, whatever
    /// their own clocks say.
    pub(crate) fn now(&mut self) -> io::Result<f64> {
        let sql = match &self.inner {
            #[cfg(feature = "sqlite")]
            TxInner::Sqlite(_) => "SELECT unixepoch('subsec')",
            #[cfg(feature = "postgres")]
            TxInner::Postgres(_) => "SELECT extract(epoch FROM clock_timestamp())::float8",
        };
        let now = self
            .query_opt(sql, &[])?
            .ok_or_else(|| invalid("the database returned no time"))?
            .real(0)?;
        if !now.is_finite() {
            return Err(invalid("the database returned an invalid time"));
        }
        Ok(now)
    }

    /// Runs one statement and returns how many rows it changed.
    pub(crate) fn execute(&mut self, sql: &str, params: &[Value]) -> io::Result<u64> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            TxInner::Sqlite(transaction) => {
                let mut statement = transaction
                    .prepare_cached(&sqlite_sql(sql))
                    .map_err(sqlite_error)?;
                statement
                    .execute(rusqlite::params_from_iter(params))
                    .map(|changed| changed as u64)
                    .map_err(sqlite_error)
            }
            #[cfg(feature = "postgres")]
            TxInner::Postgres(transaction) => {
                block_on(transaction.execute(sql, &postgres::params(params)))
                    .map_err(postgres::error)
            }
        }
    }

    /// Runs one query and returns every row.
    pub(crate) fn query(&mut self, sql: &str, params: &[Value]) -> io::Result<Vec<Row>> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            TxInner::Sqlite(transaction) => {
                let mut statement = transaction
                    .prepare_cached(&sqlite_sql(sql))
                    .map_err(sqlite_error)?;
                let columns = statement.column_count();
                let mut rows = statement
                    .query(rusqlite::params_from_iter(params))
                    .map_err(sqlite_error)?;
                let mut result = Vec::new();
                while let Some(row) = rows.next().map_err(sqlite_error)? {
                    let mut values = Vec::with_capacity(columns);
                    for index in 0..columns {
                        values.push(sqlite_value(row.get_ref(index).map_err(sqlite_error)?));
                    }
                    result.push(Row(values));
                }
                Ok(result)
            }
            #[cfg(feature = "postgres")]
            TxInner::Postgres(transaction) => {
                block_on(transaction.query(sql, &postgres::params(params)))
                    .map_err(postgres::error)?
                    .iter()
                    .map(postgres::row)
                    .collect()
            }
        }
    }

    /// Runs one query and returns its first row.
    pub(crate) fn query_opt(&mut self, sql: &str, params: &[Value]) -> io::Result<Option<Row>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    /// Runs statements that take no parameters, such as a schema.
    fn batch(&mut self, sql: &str) -> io::Result<()> {
        match &mut self.inner {
            #[cfg(feature = "sqlite")]
            TxInner::Sqlite(transaction) => transaction.execute_batch(sql).map_err(sqlite_error),
            #[cfg(feature = "postgres")]
            TxInner::Postgres(transaction) => {
                block_on(transaction.batch_execute(sql)).map_err(postgres::error)
            }
        }
    }

    fn commit(self) -> io::Result<()> {
        match self.inner {
            #[cfg(feature = "sqlite")]
            TxInner::Sqlite(transaction) => transaction.commit().map_err(sqlite_error),
            #[cfg(feature = "postgres")]
            TxInner::Postgres(transaction) => {
                block_on(transaction.commit()).map_err(postgres::error)
            }
        }
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_sql(sql: &str) -> String {
    sql.replace('$', "?")
}

#[cfg(feature = "sqlite")]
impl rusqlite::ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, ValueRef};
        Ok(ToSqlOutput::Borrowed(match self {
            Self::Null => ValueRef::Null,
            Self::Int(value) => ValueRef::Integer(*value),
            Self::Real(value) => ValueRef::Real(*value),
            Self::Text(value) => ValueRef::Text(value.as_bytes()),
            Self::Bytes(value) => ValueRef::Blob(value),
        }))
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_value(value: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::Int(value),
        ValueRef::Real(value) => Value::Real(value),
        ValueRef::Text(text) => Value::Text(String::from_utf8_lossy(text).into_owned()),
        ValueRef::Blob(bytes) => Value::Bytes(bytes.to_vec()),
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_error(error: rusqlite::Error) -> io::Error {
    use rusqlite::ErrorCode;
    let kind = match error.sqlite_error_code() {
        Some(ErrorCode::ConstraintViolation) => io::ErrorKind::AlreadyExists,
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, format!("SQLite: {error}"))
}
