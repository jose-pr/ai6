//! Schema steps: the changes an open makes to a table that already exists, which `CREATE TABLE
//! IF NOT EXISTS` leaves as it is.
//!
//! A step adds one column. The schema files make each table with every column, so a database
//! made by this build has the column already and records the step without running it; one made
//! before the column existed runs the step once and records it in `ai6_schema_steps`. A column
//! someone added by hand is recorded the same way, without running. A step's name is never
//! reused, a new step goes at the end, and an index on its column goes in the schema files,
//! which make indexes after the steps have run.

use super::db::Dialect;

/// One change to a table that exists.
#[derive(Debug, Clone, Copy)]
pub(super) struct Step {
    /// What `ai6_schema_steps` records it as.
    pub(super) name: &'static str,
    /// The table it changes.
    pub(super) table: &'static str,
    /// The column it adds; when the table has it, the step is recorded without running.
    pub(super) column: &'static str,
    /// Its statements on SQLite.
    #[cfg(feature = "sqlite")]
    pub(super) sqlite: &'static str,
    /// Its statements on PostgreSQL.
    #[cfg(feature = "postgres")]
    pub(super) postgres: &'static str,
}

impl Step {
    /// Its statements in `dialect`.
    pub(super) fn statements(&self, dialect: Dialect) -> &'static str {
        match dialect {
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => self.sqlite,
            #[cfg(feature = "postgres")]
            Dialect::Postgres => self.postgres,
        }
    }
}

/// Every step, in the order they run.
pub(super) const STEPS: &[Step] = &[
    // The handle book's handle. A table with rows refuses it: Missions stored before handles
    // were keyed by what is now a handle, and need UIDs no statement can mint.
    Step {
        name: "mission_ids.handle",
        table: "ai6_mission_ids",
        column: "handle",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_mission_ids ADD COLUMN handle TEXT NOT NULL",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_mission_ids ADD COLUMN handle TEXT NOT NULL",
    },
    // When a cancel of a claimed brief was first asked for; existing rows have none asked.
    Step {
        name: "briefs.cancel_requested_at",
        table: "ai6_briefs",
        column: "cancel_requested_at",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_briefs ADD COLUMN cancel_requested_at REAL",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_briefs ADD COLUMN cancel_requested_at DOUBLE PRECISION",
    },
];
