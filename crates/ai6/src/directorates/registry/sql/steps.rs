//! Schema steps: the changes an open makes to a table that already exists, which `CREATE TABLE
//! IF NOT EXISTS` leaves as it is.
//!
//! A step adds one column, or retires one a rename left behind. The schema files make each
//! table with every column, so a database made by this build has the
//! column already and records the step without running it; one made before the column existed runs the step once and records it in `ai6_schema_steps`. A column
//! someone added by hand is recorded the same way, without running. A step's name is never
//! reused, a new step goes at the end, and an index on its column goes in the schema files,
//! which make indexes after the steps have run. A retiring step runs while its column is there.

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
    /// Set on a step that retires `column`, a legacy column a rename left behind: it runs only
    /// while the table still has it.
    pub(super) retires: bool,
    /// Set on a step that moves a renamed table's rows: the table they move to. The step refuses
    /// while both tables hold rows, since neither is the one to keep.
    pub(super) into: &'static str,
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
        retires: false,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_mission_ids ADD COLUMN handle TEXT NOT NULL",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_mission_ids ADD COLUMN handle TEXT NOT NULL",
    },
    // The distinct words of a Report that search reads; a row made before has none, and is
    // always a candidate until it is written again.
    Step {
        name: "reports.words",
        table: "ai6_reports",
        column: "words",
        retires: false,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_reports ADD COLUMN words TEXT NOT NULL DEFAULT ''",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_reports ADD COLUMN words TEXT NOT NULL DEFAULT '';                    ALTER TABLE ai6_reports ADD COLUMN search tsvector                    GENERATED ALWAYS AS (to_tsvector('simple', words)) STORED",
    },
    // When a cancel of a claimed brief was first asked for; existing rows have none asked.
    Step {
        name: "briefs.cancel_requested_at",
        table: "ai6_briefs",
        column: "cancel_requested_at",
        retires: false,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_briefs ADD COLUMN cancel_requested_at REAL",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_briefs ADD COLUMN cancel_requested_at DOUBLE PRECISION",
    },
    // The Session-to-Run rename moved four populated columns. Each is added, filled from the
    // column it replaces and then that column is dropped, since a legacy NOT NULL column with
    // no default refuses every insert. A database someone altered by hand has the new column
    // already and only has the old one retired.
    Step {
        name: "briefs.claims",
        table: "ai6_briefs",
        column: "claims",
        retires: false,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_briefs ADD COLUMN claims INTEGER NOT NULL DEFAULT 0; \
                 UPDATE ai6_briefs SET claims = attempts",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_briefs ADD COLUMN claims BIGINT NOT NULL DEFAULT 0; \
                   UPDATE ai6_briefs SET claims = attempts",
    },
    Step {
        name: "briefs.retire_attempts",
        table: "ai6_briefs",
        column: "attempts",
        retires: true,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_briefs DROP COLUMN attempts",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_briefs DROP COLUMN attempts",
    },
    Step {
        name: "brief_leases.number",
        table: "ai6_brief_leases",
        column: "number",
        retires: false,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_brief_leases ADD COLUMN number INTEGER NOT NULL DEFAULT 0; \
                 UPDATE ai6_brief_leases SET number = attempt",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_brief_leases ADD COLUMN number BIGINT NOT NULL DEFAULT 0; \
                   UPDATE ai6_brief_leases SET number = attempt",
    },
    Step {
        name: "brief_leases.retire_attempt",
        table: "ai6_brief_leases",
        column: "attempt",
        retires: true,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_brief_leases DROP COLUMN attempt",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_brief_leases DROP COLUMN attempt",
    },
    Step {
        name: "reports.run_id",
        table: "ai6_reports",
        column: "run_id",
        retires: false,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_reports ADD COLUMN run_id TEXT NOT NULL DEFAULT ''; \
                 UPDATE ai6_reports SET run_id = session_id",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_reports ADD COLUMN run_id TEXT NOT NULL DEFAULT ''; \
                   UPDATE ai6_reports SET run_id = session_id",
    },
    Step {
        name: "reports.retire_session_id",
        table: "ai6_reports",
        column: "session_id",
        retires: true,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_reports DROP COLUMN session_id",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_reports DROP COLUMN session_id",
    },
    // The final report's key was its primary key, which neither database adds or drops in place.
    Step {
        name: "final_reports.run_id",
        table: "ai6_final_reports",
        column: "run_id",
        retires: false,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "ALTER TABLE ai6_final_reports ADD COLUMN run_id TEXT NOT NULL DEFAULT ''; \
                 UPDATE ai6_final_reports SET run_id = session_id",
        #[cfg(feature = "postgres")]
        postgres: "ALTER TABLE ai6_final_reports ADD COLUMN run_id TEXT NOT NULL DEFAULT ''; \
                   UPDATE ai6_final_reports SET run_id = session_id",
    },
    Step {
        name: "final_reports.retire_session_id",
        table: "ai6_final_reports",
        column: "session_id",
        retires: true,
        into: "",
        #[cfg(feature = "sqlite")]
        sqlite: "CREATE TABLE ai6_final_reports_rebuilt (run_id TEXT PRIMARY KEY, \
                 mission_id TEXT NOT NULL, record TEXT NOT NULL, revision INTEGER NOT NULL); \
                 INSERT INTO ai6_final_reports_rebuilt \
                 SELECT COALESCE(NULLIF(run_id, ''), session_id), mission_id, record, revision \
                 FROM ai6_final_reports; \
                 DROP TABLE ai6_final_reports; \
                 ALTER TABLE ai6_final_reports_rebuilt RENAME TO ai6_final_reports",
        #[cfg(feature = "postgres")]
        postgres: "UPDATE ai6_final_reports SET run_id = session_id WHERE run_id = ''; \
                   ALTER TABLE ai6_final_reports DROP COLUMN session_id; \
                   DO $$ BEGIN \
                   IF NOT EXISTS (SELECT 1 FROM pg_index WHERE indisprimary \
                   AND indrelid = 'ai6_final_reports'::regclass) THEN \
                   ALTER TABLE ai6_final_reports ADD PRIMARY KEY (run_id); \
                   END IF; END $$",
    },
    // The same release renamed three tables. The rows move to the new table, with the renamed
    // column mapped, and the old table goes; a database that holds rows in both is refused.
    Step {
        name: "runs.from_sessions",
        table: "ai6_sessions",
        column: "mission_id",
        retires: true,
        into: "ai6_runs",
        #[cfg(feature = "sqlite")]
        sqlite: "INSERT INTO ai6_runs (run_id, mission_id, record, revision) \
                 SELECT session_id, mission_id, record, revision FROM ai6_sessions; \
                 DROP TABLE ai6_sessions",
        #[cfg(feature = "postgres")]
        postgres: "INSERT INTO ai6_runs (run_id, mission_id, record, revision) \
                   SELECT session_id, mission_id, record, revision FROM ai6_sessions; \
                   DROP TABLE ai6_sessions",
    },
    Step {
        name: "run_contexts.from_session_contexts",
        table: "ai6_session_contexts",
        column: "conversation",
        retires: true,
        into: "ai6_run_contexts",
        #[cfg(feature = "sqlite")]
        sqlite: "INSERT INTO ai6_run_contexts \
                 (run, conversation, operation, filed_micros, record, content, revision) \
                 SELECT session, conversation, operation, filed_micros, record, content, revision \
                 FROM ai6_session_contexts; \
                 DROP TABLE ai6_session_contexts",
        #[cfg(feature = "postgres")]
        postgres: "INSERT INTO ai6_run_contexts \
                   (run, conversation, operation, filed_micros, record, content, revision) \
                   SELECT session, conversation, operation, filed_micros, record, content, revision \
                   FROM ai6_session_contexts; \
                   DROP TABLE ai6_session_contexts",
    },
    Step {
        name: "charges.from_watches",
        table: "ai6_watches",
        column: "scope",
        retires: true,
        into: "ai6_charges",
        #[cfg(feature = "sqlite")]
        sqlite: "INSERT INTO ai6_charges (scope, record, revision) \
                 SELECT scope, record, revision FROM ai6_watches; \
                 DROP TABLE ai6_watches",
        #[cfg(feature = "postgres")]
        postgres: "INSERT INTO ai6_charges (scope, record, revision) \
                   SELECT scope, record, revision FROM ai6_watches; \
                   DROP TABLE ai6_watches",
    },
];
