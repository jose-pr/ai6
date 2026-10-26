//! The Registry in a SQL database: [`SqlRegistry`], a
//! [`RegistryBackend`](super::backend::RegistryBackend) kept in SQLite (one Station, tests)
//! or PostgreSQL (every Station of an HA deployment, with no shared filesystem).
//!
//! Each record family has its own table, holding the record as JSON text in the shape the
//! file Registry writes, beside the columns a lookup needs. Every write transaction first
//! advances the Registry's clock row, a row lock that serializes writers across Stations as
//! the file Registry's directory lock does; the brief leases, the Headquarters election and
//! Session fencing therefore keep the file Registry's guarantees, terms and fences. Each
//! row carries the revision of the write that last changed it. Leases expire on each
//! Station's wall clock, so Stations sharing a database need synchronized clocks.
//!
//! The schema is created on first use and versioned; a database written by a newer `ai6`
//! is refused. Secrets are never stored: access tokens are kept by digest, and credentials
//! stay in Q's secret backends.

mod approvals;
mod briefs;
pub(crate) mod db;
mod records;
mod sessions;
mod stations;

use std::io;

use serde::Serialize;
use serde::de::DeserializeOwned;

use db::{Clock, Database, Location, Tx};

/// The Registry kept in a SQLite or PostgreSQL database.
///
/// Open one per Station with [`SqlRegistry::open`]; every handle on the same database sees
/// the same records.
#[derive(Debug)]
pub struct SqlRegistry {
    db: Database,
}

impl SqlRegistry {
    /// Opens the Registry at `location`, creating or upgrading its tables.
    ///
    /// `location` is `sqlite:PATH` (or `sqlite://PATH`) for a SQLite file, or a PostgreSQL
    /// connection URL (`postgres://` or `postgresql://`), whose parameters (such as
    /// `options=-c search_path=…` for a schema) pass to the server. PostgreSQL connections
    /// are made without TLS.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a location that is neither, `Unsupported` when this build lacks
    /// that database's cargo feature (`sqlite`, `postgres`) or the schema is newer than this
    /// build, and the database's own error when it cannot be reached.
    pub fn open(location: &str) -> io::Result<Self> {
        let location = Location::parse(location).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "a SQL Registry location starts with sqlite: or postgres://",
            )
        })?;
        Ok(Self {
            db: Database::open(location)?,
        })
    }

    fn read<T: Send, E: From<io::Error> + Send>(
        &self,
        work: impl FnOnce(&mut Tx<'_>) -> Result<T, E> + Send,
    ) -> Result<T, E> {
        self.db.read(work)
    }

    fn write<T: Send, E: From<io::Error> + Send>(
        &self,
        work: impl FnOnce(&mut Tx<'_>, i64) -> Result<T, E> + Send,
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
