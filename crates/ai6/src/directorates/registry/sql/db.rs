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

use std::collections::BTreeSet;
use std::io;
#[cfg(feature = "sqlite")]
use std::sync::{Mutex, MutexGuard, PoisonError};
#[cfg(feature = "sqlite")]
use std::time::Duration;

use super::SqlOptions;
#[cfg(feature = "postgres")]
use super::postgres::{self, block_on};
use super::steps::{STEPS, Step};

/// The schema version this build writes; a newer database is refused.
const SCHEMA_VERSION: i64 = 1;

#[cfg(feature = "sqlite")]
const SQLITE_SCHEMA: &str = include_str!("schema/sqlite.sql");
#[cfg(feature = "postgres")]
const POSTGRES_SCHEMA: &str = include_str!("schema/postgres.sql");

/// The clock rows the schema seeds, one per [`Clock`].
const CLOCK_ROWS: i64 = 2;

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

/// One statement of a schema file; its text is safe to run again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Statement {
    /// The table or index it makes; `None` for the one that fills the clock.
    pub(super) creates: Option<String>,
    /// For a table, the columns it is made with, in lower case; `None` for anything else.
    pub(super) columns: Option<Vec<String>>,
    text: String,
}

/// The statements of a schema file, in order. Whole-line comments are dropped first, since one
/// may hold a semicolon.
///
/// # Errors
///
/// `InvalidData` for anything but `CREATE TABLE IF NOT EXISTS`, `CREATE VIRTUAL TABLE IF NOT
/// EXISTS`, `CREATE INDEX IF NOT EXISTS` and the clock's `INSERT`, which could not be run again at
/// every open.
pub(super) fn statements(schema: &str) -> io::Result<Vec<Statement>> {
    let code: String = schema
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .map(|line| format!("{line}\n"))
        .collect();
    code.split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .map(|text| {
            let made = [
                ("CREATE TABLE IF NOT EXISTS ", true),
                ("CREATE VIRTUAL TABLE IF NOT EXISTS ", true),
                ("CREATE INDEX IF NOT EXISTS ", false),
            ]
            .into_iter()
            .find_map(|(head, table)| Some((text.strip_prefix(head)?, table)))
            .and_then(|(rest, table)| Some((rest.split_whitespace().next()?, table)));
            let (creates, columns) = match made {
                Some((name, true)) => (Some(name.to_owned()), Some(made_columns(text)?)),
                Some((name, false)) => (Some(name.to_owned()), None),
                None if text.starts_with("INSERT INTO ai6_clock ") => (None, None),
                None => return Err(invalid(format!("not a repeatable statement: {text}"))),
            };
            Ok(Statement {
                creates,
                columns,
                text: text.to_owned(),
            })
        })
        .collect()
}

/// The columns a `CREATE TABLE` names, in lower case: each element of its column list but table
/// constraints and, of a virtual table, `key=value` options.
fn made_columns(text: &str) -> io::Result<Vec<String>> {
    let unreadable = || invalid(format!("no column list in: {text}"));
    let open = text.find('(').ok_or_else(unreadable)?;
    let mut elements = Vec::new();
    let (mut depth, mut quoted, mut from, mut closed) = (0usize, false, open + 1, false);
    for (at, character) in text.char_indices().filter(|(at, _)| *at > open) {
        match character {
            '\'' => quoted = !quoted,
            _ if quoted => {}
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            ')' => {
                elements.push(&text[from..at]);
                closed = true;
                break;
            }
            ',' if depth == 0 => {
                elements.push(&text[from..at]);
                from = at + 1;
            }
            _ => {}
        }
    }
    if !closed {
        return Err(unreadable());
    }
    let mut columns = Vec::new();
    for element in elements {
        let first = element.split_whitespace().next().ok_or_else(unreadable)?;
        let constraint = [
            "PRIMARY",
            "UNIQUE",
            "FOREIGN",
            "CHECK",
            "CONSTRAINT",
            "EXCLUDE",
        ]
        .iter()
        .any(|word| first.eq_ignore_ascii_case(word));
        if !constraint && !first.contains('=') {
            columns.push(first.trim_matches('"').to_ascii_lowercase());
        }
    }
    Ok(columns)
}

/// The tables and indexes of the schema statements create in, as `CREATE ... IF NOT EXISTS`
/// looks for them; reading them needs no right beyond connecting.
pub(super) fn schema_objects(tx: &mut Tx<'_>) -> io::Result<BTreeSet<String>> {
    let sql = match &tx.inner {
        #[cfg(feature = "sqlite")]
        TxInner::Sqlite(_) => "SELECT name FROM sqlite_master WHERE type IN ('table', 'index')",
        #[cfg(feature = "postgres")]
        TxInner::Postgres(_) => {
            "SELECT c.relname::text FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p', 'i', 'I')"
        }
    };
    tx.query(sql, &[])?
        .into_iter()
        .map(|row| row.text(0))
        .collect()
}

impl Database {
    /// Opens the database at `location` and brings its schema up to date.
    pub(crate) fn open(location: Location<'_>, options: &SqlOptions) -> io::Result<Self> {
        Self::open_with_steps(location, options, STEPS)
    }

    /// [`open`](Self::open), bringing tables up to date with `steps` in place of [`STEPS`].
    pub(super) fn open_with_steps(
        location: Location<'_>,
        options: &SqlOptions,
        steps: &[Step],
    ) -> io::Result<Self> {
        let database = match location {
            Location::Sqlite(path) => Self::open_sqlite(path, options.busy_timeout())?,
            Location::Postgres(url) => Self::open_postgres(url, options)?,
        };
        database.migrate(steps)?;
        Ok(database)
    }

    #[cfg(feature = "sqlite")]
    fn open_sqlite(path: &str, busy_timeout: Duration) -> io::Result<Self> {
        if path.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a sqlite: location needs a file path",
            ));
        }
        let connection = rusqlite::Connection::open(path).map_err(sqlite_error)?;
        connection
            .busy_timeout(busy_timeout)
            .map_err(sqlite_error)?;
        // WAL lets readers in other processes carry on while one Station writes. Changing to it
        // answers busy at once while another connection does the same, without the busy handler.
        let waiting = std::time::Instant::now();
        loop {
            match connection.execute_batch("PRAGMA journal_mode = WAL;") {
                Ok(()) => break,
                Err(rusqlite::Error::SqliteFailure(failure, _))
                    if failure.code == rusqlite::ErrorCode::DatabaseBusy
                        && waiting.elapsed() < busy_timeout =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(failure) => return Err(sqlite_error(failure)),
            }
        }
        Ok(Self {
            kind: Kind::Sqlite(Mutex::new(connection)),
        })
    }

    #[cfg(not(feature = "sqlite"))]
    fn open_sqlite(_path: &str, _busy_timeout: Duration) -> io::Result<Self> {
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

    /// Makes the tables the database lacks, runs the steps it has not recorded, makes the
    /// indexes it lacks, and refuses a table without a column this build needs.
    fn migrate(&self, steps: &[Step]) -> io::Result<()> {
        let dialect = self.dialect();
        let schema = match dialect {
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => SQLITE_SCHEMA,
            #[cfg(feature = "postgres")]
            Dialect::Postgres => POSTGRES_SCHEMA,
        };
        let statements = statements(schema)?;
        // Tables come before the steps and indexes after them, since an index may name a column
        // a step adds.
        let recorded = self.transaction(Mode::Migrate, |tx, _| {
            migration_lock(tx)?;
            let present = schema_objects(tx)?;
            // A newer ai6's database is refused before anything is made in it.
            let current = if present.contains("ai6_schema") {
                schema_version(tx)?
            } else {
                0
            };
            check_schema(current)?;
            make_missing(tx, &statements, &present, |s| s.columns.is_some())?;
            let rows = tx
                .query_opt("SELECT count(*) FROM ai6_clock", &[])?
                .ok_or_else(|| invalid("the clock could not be counted"))?
                .int(0)?;
            if rows < CLOCK_ROWS {
                for statement in statements.iter().filter(|s| s.creates.is_none()) {
                    tx.batch(&statement.text)?;
                }
            }
            if current < 1 {
                tx.execute(
                    "INSERT INTO ai6_schema (version) VALUES ($1)",
                    &[Value::Int(SCHEMA_VERSION)],
                )?;
            }
            recorded_steps(tx)
        })?;
        for step in steps.iter().filter(|step| !recorded.contains(step.name)) {
            self.transaction(Mode::Migrate, |tx, _| apply_step(tx, dialect, step))?;
        }
        self.transaction(Mode::Migrate, |tx, _| {
            migration_lock(tx)?;
            let present = schema_objects(tx)?;
            make_missing(tx, &statements, &present, |s| s.columns.is_none())?;
            check_columns(tx, &statements, dialect)
        })
    }
}

/// Two Stations opening a database that lacks something make it once: SQLite's IMMEDIATE
/// transaction and this lock make the second wait for the first.
fn migration_lock(tx: &mut Tx<'_>) -> io::Result<()> {
    #[cfg(feature = "postgres")]
    if matches!(tx.inner, TxInner::Postgres(_)) {
        tx.execute(
            "SELECT pg_advisory_xact_lock($1, hashtext(current_schema()))",
            &[Value::Int(MIGRATION_LOCK)],
        )?;
    }
    #[cfg(not(feature = "postgres"))]
    let _ = tx;
    Ok(())
}

/// Runs the statements `which` picks that make something `present` lacks. Nothing runs when
/// nothing is missing, since `IF NOT EXISTS` needs the rights of a `CREATE` even then, and a
/// role without them may run a database someone else keeps up to date.
fn make_missing(
    tx: &mut Tx<'_>,
    statements: &[Statement],
    present: &BTreeSet<String>,
    which: impl Fn(&Statement) -> bool,
) -> io::Result<()> {
    // Asked first: PostgreSQL ends the transaction at a refused statement. A refusal names
    // everything missing, tables and indexes, so one start as a role that may makes it all.
    let missing: Vec<&Statement> = statements
        .iter()
        .filter(|s| {
            s.creates
                .as_ref()
                .is_some_and(|name| !present.contains(name))
        })
        .collect();
    if !missing.iter().any(|s| which(s)) {
        return Ok(());
    }
    let at = where_we_are(tx)?;
    for statement in missing.iter().filter(|s| which(s)) {
        tx.batch(&statement.text).map_err(|failure| {
            if failure.kind() == io::ErrorKind::PermissionDenied {
                refused_to_create(&at, &missing, &failure)
            } else {
                failure
            }
        })?;
    }
    Ok(())
}

/// The names of the steps `ai6_schema_steps` records.
fn recorded_steps(tx: &mut Tx<'_>) -> io::Result<BTreeSet<String>> {
    tx.query("SELECT name FROM ai6_schema_steps", &[])?
        .into_iter()
        .map(|row| row.text(0))
        .collect()
}

/// Runs `step` unless another Station has, or the table has its column already, and records it,
/// stamped with the database's clock.
fn apply_step(tx: &mut Tx<'_>, dialect: Dialect, step: &Step) -> io::Result<()> {
    migration_lock(tx)?;
    if recorded_steps(tx)?.contains(step.name) {
        return Ok(());
    }
    let has_column = table_columns(tx)?
        .get(step.table)
        .is_some_and(|columns| columns.contains(step.column));
    if !has_column {
        let (role, schema) = where_we_are(tx)?;
        tx.batch(step.statements(dialect)).map_err(|failure| {
            let what = format!(
                "schema step {} (column {} of table {})",
                step.name, step.column, step.table
            );
            if failure.kind() == io::ErrorKind::PermissionDenied {
                io::Error::new(
                    failure.kind(),
                    format!(
                        "{what} is not done and role {role} may not alter that table in schema \
                         {schema}: start this ai6 once as a role that may ({failure})"
                    ),
                )
            } else {
                io::Error::new(failure.kind(), format!("{what} failed: {failure}"))
            }
        })?;
    }
    let micros = (tx.now()? * 1_000_000.0).round() as i64;
    tx.execute(
        "INSERT INTO ai6_schema_steps (name, applied_micros) VALUES ($1, $2)",
        &[step.name.into(), Value::Int(micros)],
    )?;
    Ok(())
}

/// Each table's columns as the database's catalog has them, in lower case; reading them needs
/// no right on the tables.
fn table_columns(
    tx: &mut Tx<'_>,
) -> io::Result<std::collections::BTreeMap<String, BTreeSet<String>>> {
    let sql = match &tx.inner {
        #[cfg(feature = "sqlite")]
        TxInner::Sqlite(_) => {
            // Hidden 1 is a virtual table's own hidden column, which no statement names.
            "SELECT m.name, c.name FROM sqlite_master m, pragma_table_xinfo(m.name) c \
             WHERE m.type = 'table' AND c.hidden <> 1"
        }
        #[cfg(feature = "postgres")]
        TxInner::Postgres(_) => {
            "SELECT c.relname::text, a.attname::text FROM pg_catalog.pg_attribute a \
             JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p') \
             AND a.attnum > 0 AND NOT a.attisdropped"
        }
    };
    let mut tables = std::collections::BTreeMap::<String, BTreeSet<String>>::new();
    for row in tx.query(sql, &[])? {
        tables
            .entry(row.text(0)?)
            .or_default()
            .insert(row.text(1)?.to_ascii_lowercase());
    }
    Ok(tables)
}

/// Refuses a database whose tables lack a column the schema makes them with. A column this
/// build does not know is left alone, as an earlier build opening a later one's database finds.
fn check_columns(tx: &mut Tx<'_>, statements: &[Statement], dialect: Dialect) -> io::Result<()> {
    let tables = table_columns(tx)?;
    let mut lacking = Vec::new();
    for statement in statements {
        let (Some(table), Some(columns)) = (&statement.creates, &statement.columns) else {
            continue;
        };
        let have = tables.get(table);
        for column in columns {
            if !have.is_some_and(|have| have.contains(column)) {
                lacking.push(format!("table {table} lacks column {column}"));
            }
        }
    }
    if lacking.is_empty() {
        return Ok(());
    }
    let file = match dialect {
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => "sqlite.sql",
        #[cfg(feature = "postgres")]
        Dialect::Postgres => "postgres.sql",
    };
    Err(invalid(format!(
        "this database does not have what this ai6 needs, and no schema step left to run adds it: \
         {}; put each column back as crates/ai6/src/directorates/registry/sql/schema/{file} \
         makes it",
        lacking.join(", ")
    )))
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

/// The role and schema a `CREATE` would run as and in; on SQLite, the file's own.
fn where_we_are(tx: &mut Tx<'_>) -> io::Result<(String, String)> {
    match &tx.inner {
        #[cfg(feature = "sqlite")]
        TxInner::Sqlite(_) => Ok(("this process".to_owned(), "main".to_owned())),
        #[cfg(feature = "postgres")]
        TxInner::Postgres(_) => {
            let row = tx
                .query_opt("SELECT current_user::text, current_schema()::text", &[])?
                .ok_or_else(|| invalid("the database did not say who is connected"))?;
            Ok((row.text(0)?, row.text(1)?))
        }
    }
}

fn refused_to_create(
    (role, schema): &(String, String),
    missing: &[&Statement],
    failure: &io::Error,
) -> io::Error {
    let names: Vec<&str> = missing
        .iter()
        .filter_map(|statement| statement.creates.as_deref())
        .collect();
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "this database lacks {} and role {role} may not make them in schema {schema}: start \
             this ai6 once as a role that may, or create them from the schema file, and grant \
             them to {role} ({failure})",
            names.join(", ")
        ),
    )
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
