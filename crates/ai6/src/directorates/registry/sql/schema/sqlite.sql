-- AI6 Registry and roster, SQLite schema version 1.
--
-- Records are JSON text in the same shape the file Registry writes, with the columns a
-- lookup needs beside them. Every row carries the revision of the write that last touched
-- it: the value of its clock row in ai6_clock, which each write transaction advances first.
--
-- Opening a database runs the statements for what it lacks, so each one can be run again: a new
-- table or index goes here and needs no script, and the version stays. A new column goes in its
-- table here and as a step in ../steps.rs, which adds it to a table that exists. What moves data
-- needs a script.

CREATE TABLE IF NOT EXISTS ai6_schema (version INTEGER NOT NULL);

-- Each step of ../steps.rs this database has, and when it was recorded, in microseconds of the
-- database's clock.
CREATE TABLE IF NOT EXISTS ai6_schema_steps (
    name TEXT PRIMARY KEY,
    applied_micros INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai6_clock (
    name TEXT PRIMARY KEY,
    revision INTEGER NOT NULL
);
INSERT INTO ai6_clock (name, revision) VALUES ('registry', 0), ('config', 0)
    ON CONFLICT (name) DO NOTHING;

-- The brief queue: state is new, held, claimed, done or failed. held is the record of a
-- brief vetting held (its Operation, the vetting and when); clearance lists the finding kinds
-- the brief no longer stops for; cancel_requested_at is when a cancel of the claimed brief was
-- first asked for, as Unix seconds.
CREATE TABLE IF NOT EXISTS ai6_briefs (
    key TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    brief TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    outcome TEXT,
    transcript BLOB,
    report_attempts INTEGER NOT NULL DEFAULT 0,
    reported INTEGER NOT NULL DEFAULT 0,
    provenance TEXT,
    finished_provenance TEXT,
    held TEXT,
    clearance TEXT,
    cancel_requested_at REAL,
    revision INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ai6_briefs_state ON ai6_briefs (state);

CREATE TABLE IF NOT EXISTS ai6_brief_leases (
    key TEXT PRIMARY KEY,
    station TEXT NOT NULL,
    expires REAL NOT NULL,
    attempt INTEGER NOT NULL,
    meta TEXT NOT NULL,
    revision INTEGER NOT NULL
);

-- The last number given per Operation (its name folded to lower case); it only grows.
CREATE TABLE IF NOT EXISTS ai6_mission_counters (
    name TEXT PRIMARY KEY,
    last INTEGER NOT NULL,
    revision INTEGER NOT NULL
);

-- Every Mission handle given, current or former, by its lower-case form, since two that
-- differ only by case are one branch elsewhere; handle is as given, mission_id the Mission's
-- UID. entry names what can enter again and must name the same Mission: it sits on the row of
-- the Mission's current handle.
CREATE TABLE IF NOT EXISTS ai6_mission_ids (
    folded TEXT PRIMARY KEY,
    handle TEXT NOT NULL,
    mission_id TEXT NOT NULL,
    entry TEXT UNIQUE,
    revision INTEGER NOT NULL
);

-- state is pending, approved or rejected.
CREATE TABLE IF NOT EXISTS ai6_mission_proposals (
    operation TEXT NOT NULL,
    id TEXT NOT NULL,
    state TEXT NOT NULL,
    proposal TEXT NOT NULL,
    created_at REAL NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (operation, id)
);

CREATE TABLE IF NOT EXISTS ai6_stations (
    station TEXT PRIMARY KEY,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai6_station_settings (
    station TEXT PRIMARY KEY,
    max_concurrent_missions INTEGER NOT NULL,
    min_free_memory_mb INTEGER NOT NULL,
    max_cpu_percent INTEGER NOT NULL,
    revision INTEGER NOT NULL
);

-- One row: the Headquarters lease.
CREATE TABLE IF NOT EXISTS ai6_headquarters (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    station TEXT NOT NULL,
    expires REAL NOT NULL,
    term INTEGER NOT NULL,
    revision INTEGER NOT NULL
);

-- Access-token metadata by SHA-256 digest; never a bearer secret.
CREATE TABLE IF NOT EXISTS ai6_access_tokens (
    token_id TEXT PRIMARY KEY,
    digest TEXT NOT NULL UNIQUE,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai6_cables (
    id TEXT PRIMARY KEY,
    sender_agent TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    created_at REAL NOT NULL,
    expires_at REAL NOT NULL,
    cable TEXT NOT NULL,
    revision INTEGER NOT NULL,
    UNIQUE (sender_agent, idempotency_key)
);
CREATE INDEX IF NOT EXISTS ai6_cables_expiry ON ai6_cables (expires_at);

-- Token buckets keyed by the SHA-256 of the quota key.
CREATE TABLE IF NOT EXISTS ai6_cable_quotas (
    key TEXT PRIMARY KEY,
    tokens REAL NOT NULL,
    updated REAL NOT NULL,
    revision INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai6_audit (
    id TEXT PRIMARY KEY,
    at REAL NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai6_agent_roles (
    operation TEXT NOT NULL,
    id TEXT NOT NULL,
    role TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (operation, id)
);

-- Green ink: state is pending or decided; seq orders a Mission's requests.
CREATE TABLE IF NOT EXISTS ai6_approvals (
    operation TEXT NOT NULL,
    id TEXT NOT NULL,
    mission TEXT NOT NULL,
    state TEXT NOT NULL,
    record TEXT NOT NULL,
    seq INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (operation, id)
);
CREATE INDEX IF NOT EXISTS ai6_approvals_mission ON ai6_approvals (operation, mission, seq);

CREATE TABLE IF NOT EXISTS ai6_class_authorizations (
    operation TEXT NOT NULL,
    id TEXT NOT NULL,
    created_at REAL NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (operation, id)
);

-- scope is the Operation, or '' for every Operation.
CREATE TABLE IF NOT EXISTS ai6_quarantines (
    agent TEXT NOT NULL,
    scope TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (agent, scope)
);

CREATE TABLE IF NOT EXISTS ai6_incidents (
    agent TEXT PRIMARY KEY,
    history TEXT NOT NULL,
    revision INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai6_agent_records (
    agent TEXT PRIMARY KEY,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai6_clearance_grants (
    operation TEXT NOT NULL,
    agent TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (operation, agent)
);

CREATE TABLE IF NOT EXISTS ai6_roster_holds (
    operation TEXT NOT NULL,
    agent TEXT NOT NULL,
    route TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (operation, agent, route)
);

CREATE TABLE IF NOT EXISTS ai6_sessions (
    session_id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ai6_sessions_mission ON ai6_sessions (mission_id);

-- An Agent's last message on a Session, apart from its record so a board read stays small.
CREATE TABLE IF NOT EXISTS ai6_final_reports (
    session_id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai6_sleeper_authorizations (
    operation TEXT NOT NULL,
    name TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (operation, name)
);

-- Each Station's logbook: the newest Head of Station reports, oldest first.
CREATE TABLE IF NOT EXISTS ai6_logbooks (
    station TEXT PRIMARY KEY,
    reports TEXT NOT NULL,
    revision INTEGER NOT NULL
);

-- Each watch: its standing Session, lease and logbook as JSON, keyed by its scope.
CREATE TABLE IF NOT EXISTS ai6_watches (
    scope TEXT PRIMARY KEY,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai6_source_cursors (
    operation TEXT NOT NULL,
    provider TEXT NOT NULL,
    cursor TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (operation, provider)
);

CREATE TABLE IF NOT EXISTS ai6_timeline (
    id TEXT PRIMARY KEY,
    operation TEXT NOT NULL,
    at_micros INTEGER NOT NULL,
    event TEXT NOT NULL,
    revision INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ai6_timeline_order ON ai6_timeline (at_micros, id);
CREATE INDEX IF NOT EXISTS ai6_timeline_operation ON ai6_timeline (operation, at_micros, id);

-- Pool accounts: claims, quota windows and daily tokens, one row per account.
CREATE TABLE IF NOT EXISTS ai6_accounts (
    pool TEXT NOT NULL,
    account TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (pool, account)
);

-- Values sealed to named Stations: ciphertext and who may open it, never a key or a value.
CREATE TABLE IF NOT EXISTS ai6_sealed_values (
    name TEXT PRIMARY KEY,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

-- Each Station's public sealing key, signed by its identity.
CREATE TABLE IF NOT EXISTS ai6_sealing_keys (
    station TEXT PRIMARY KEY,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

-- operation is '' for usage outside any Operation.
CREATE TABLE IF NOT EXISTS ai6_usage (
    operation TEXT NOT NULL,
    mission TEXT NOT NULL,
    ledger TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (operation, mission)
);

-- The configuration provider: one table per record family. scope is '' for a global
-- record or station:<id> for one Station's; definition is a JSON object with the keys of the
-- record's table in its family file. A singleton family's one record is keyed 'settings'.

CREATE TABLE IF NOT EXISTS ai6_config_operation (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_agent (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_role (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_assistant (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_cover (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_theater (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_loadout (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_sandbox (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_mcp (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_overlay (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_secret (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_secret_provider (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_pool (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_cables (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_plugin (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_station (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_tls (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_access (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_operation_access (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_unex (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_roster (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_compartment (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_review (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

CREATE TABLE IF NOT EXISTS ai6_config_watch (
    scope TEXT NOT NULL DEFAULT '',
    key TEXT NOT NULL,
    definition TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (scope, key)
);

-- A Mission's reviews, one JSON record per gate, written over the revision it read.
CREATE TABLE IF NOT EXISTS ai6_mission_reviews (
    mission_id TEXT NOT NULL,
    gate TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (mission_id, gate)
);

-- Reports: one JSON record each, with what a list filters and orders by beside it. session_id is
-- the Agent Session that filed it and source_key the source whose reliability orders it; priority
-- is empty, and deadline and credibility null, while unset.
CREATE TABLE IF NOT EXISTS ai6_reports (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    state TEXT NOT NULL,
    kind TEXT NOT NULL,
    session_id TEXT NOT NULL,
    source_key TEXT NOT NULL,
    priority TEXT NOT NULL,
    deadline REAL,
    credibility INTEGER,
    created REAL NOT NULL,
    updated REAL NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ai6_reports_scope ON ai6_reports (scope, created);

-- The Missions, Reports, decisions and evidence a Report is tied to, to find a Mission's Reports.
CREATE TABLE IF NOT EXISTS ai6_report_links (
    report_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    target TEXT NOT NULL,
    PRIMARY KEY (report_id, kind, target)
);
CREATE INDEX IF NOT EXISTS ai6_report_links_target ON ai6_report_links (kind, target);

-- Each rated source of Reports; one with no row is reliability F.
CREATE TABLE IF NOT EXISTS ai6_report_sources (
    source_key TEXT PRIMARY KEY,
    reliability TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

-- Who reads Reports up to which classification: one JSON record per person, group or Agent.
CREATE TABLE IF NOT EXISTS ai6_report_clearances (
    subject TEXT PRIMARY KEY,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

-- The issues a Mission is linked to, as one JSON list per Mission.
CREATE TABLE IF NOT EXISTS ai6_mission_issues (
    mission_id TEXT PRIMARY KEY,
    links TEXT NOT NULL,
    revision INTEGER NOT NULL
);

-- Every write to a tracker: state is pending, held, sending, posted, refused or failed.
CREATE TABLE IF NOT EXISTS ai6_issue_outbox (
    id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ai6_issue_outbox_state ON ai6_issue_outbox (state);

-- The questions Agents asked on linked issues, one JSON list per Mission.
CREATE TABLE IF NOT EXISTS ai6_issue_questions (
    mission_id TEXT PRIMARY KEY,
    questions TEXT NOT NULL,
    revision INTEGER NOT NULL
);

-- Comments from permitted authors of linked issues, one JSON list per Mission.
CREATE TABLE IF NOT EXISTS ai6_issue_replies (
    mission_id TEXT PRIMARY KEY,
    replies TEXT NOT NULL,
    revision INTEGER NOT NULL
);

-- Where the last poll of an instance's replies stopped.
CREATE TABLE IF NOT EXISTS ai6_issue_cursors (
    operation TEXT NOT NULL,
    tracker TEXT NOT NULL,
    cursor TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (operation, tracker)
);

-- Agents' proposals to create issues: state is pending, approved, rejected or created.
CREATE TABLE IF NOT EXISTS ai6_issue_proposals (
    id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL,
    state TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);

-- Memories: rows of text with a scope (person:ID, operation:ID or global). entry_revision counts an
-- entry's writes, what a write names; revision is the clock's, as on every row. source and
-- compartments are JSON. Search reads the words of an entry from ai6_memories_search, kept in the
-- same transaction, and filters and ranks what it finds outside the database.
CREATE TABLE IF NOT EXISTS ai6_memories (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    name TEXT,
    summary TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL,
    updated_by TEXT NOT NULL,
    source TEXT NOT NULL,
    compartments TEXT NOT NULL,
    entry_revision INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    revision INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ai6_memories_scope ON ai6_memories (scope);
CREATE VIRTUAL TABLE IF NOT EXISTS ai6_memories_search USING fts5 (id UNINDEXED, words);

-- What each entry held before its later writes, gone when the entry is forgotten.
CREATE TABLE IF NOT EXISTS ai6_memory_versions (
    id TEXT NOT NULL,
    entry_revision INTEGER NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL,
    PRIMARY KEY (id, entry_revision)
);

-- What Agents asked to change in the memory, each until a person decides it.
CREATE TABLE IF NOT EXISTS ai6_memory_proposals (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL,
    record TEXT NOT NULL,
    revision INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ai6_memory_proposals_scope ON ai6_memory_proposals (scope);

-- Moneypenny's conversation log: one row per entry, the entry as JSON; bytes is its size.
CREATE TABLE IF NOT EXISTS ai6_assistant_log (
    id TEXT PRIMARY KEY,
    conversation TEXT NOT NULL,
    person TEXT NOT NULL,
    at_micros INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    entry TEXT NOT NULL,
    revision INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ai6_assistant_log_conversation ON ai6_assistant_log (conversation, at_micros, id);
CREATE INDEX IF NOT EXISTS ai6_assistant_log_person ON ai6_assistant_log (person);
CREATE INDEX IF NOT EXISTS ai6_assistant_log_order ON ai6_assistant_log (at_micros, id);

-- Each provider Session's harness context: the record as JSON, its content as text. conversation is
-- '' for a Mission's Session, operation '' for one of Moneypenny's.
CREATE TABLE IF NOT EXISTS ai6_session_contexts (
    session TEXT PRIMARY KEY,
    conversation TEXT NOT NULL,
    operation TEXT NOT NULL,
    filed_micros INTEGER NOT NULL,
    record TEXT NOT NULL,
    content TEXT NOT NULL,
    revision INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ai6_session_contexts_conversation ON ai6_session_contexts (conversation);
CREATE INDEX IF NOT EXISTS ai6_session_contexts_order ON ai6_session_contexts (filed_micros, session);
