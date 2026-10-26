-- AI6 Registry and roster, PostgreSQL schema version 1.
--
-- Records are JSON text in the same shape the file Registry writes, with the columns a
-- lookup needs beside them. Every row carries the revision of the write that last touched
-- it: the value of its clock row in ai6_clock, which each write transaction advances first.

CREATE TABLE ai6_clock (
    name TEXT PRIMARY KEY,
    revision BIGINT NOT NULL
);
INSERT INTO ai6_clock (name, revision) VALUES ('registry', 0), ('config', 0);

-- The brief queue: state is new, held, claimed, done or failed. held is the record of a
-- brief vetting held (its Operation, the vetting and when); clearance lists the finding kinds
-- the brief no longer stops for.
CREATE TABLE ai6_briefs (
    key TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    brief TEXT NOT NULL,
    attempts BIGINT NOT NULL DEFAULT 0,
    outcome TEXT,
    transcript BYTEA,
    report_attempts BIGINT NOT NULL DEFAULT 0,
    reported BIGINT NOT NULL DEFAULT 0,
    provenance TEXT,
    finished_provenance TEXT,
    held TEXT,
    clearance TEXT,
    revision BIGINT NOT NULL
);
CREATE INDEX ai6_briefs_state ON ai6_briefs (state);

CREATE TABLE ai6_brief_leases (
    key TEXT PRIMARY KEY,
    station TEXT NOT NULL,
    expires DOUBLE PRECISION NOT NULL,
    attempt BIGINT NOT NULL,
    meta TEXT NOT NULL,
    revision BIGINT NOT NULL
);

-- state is pending, approved or rejected.
CREATE TABLE ai6_mission_proposals (
    operation TEXT NOT NULL,
    id TEXT NOT NULL,
    state TEXT NOT NULL,
    proposal TEXT NOT NULL,
    created_at DOUBLE PRECISION NOT NULL,
    revision BIGINT NOT NULL,
    PRIMARY KEY (operation, id)
);

CREATE TABLE ai6_stations (
    station TEXT PRIMARY KEY,
    record TEXT NOT NULL,
    revision BIGINT NOT NULL
);

CREATE TABLE ai6_station_settings (
    station TEXT PRIMARY KEY,
    max_concurrent_missions BIGINT NOT NULL,
    -- NULL reads as the default limit.
    min_free_memory_mb BIGINT,
    max_cpu_percent BIGINT,
    revision BIGINT NOT NULL
);

-- One row: the Headquarters lease.
CREATE TABLE ai6_headquarters (
    id BIGINT PRIMARY KEY CHECK (id = 1),
    station TEXT NOT NULL,
    expires DOUBLE PRECISION NOT NULL,
    term BIGINT NOT NULL,
    revision BIGINT NOT NULL
);

-- Access-token metadata by SHA-256 digest; never a bearer secret.
CREATE TABLE ai6_access_tokens (
    token_id TEXT PRIMARY KEY,
    digest TEXT NOT NULL UNIQUE,
    record TEXT NOT NULL,
    revision BIGINT NOT NULL
);

CREATE TABLE ai6_cables (
    id TEXT PRIMARY KEY,
    sender_agent TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    created_at DOUBLE PRECISION NOT NULL,
    expires_at DOUBLE PRECISION NOT NULL,
    cable TEXT NOT NULL,
    revision BIGINT NOT NULL,
    UNIQUE (sender_agent, idempotency_key)
);
CREATE INDEX ai6_cables_expiry ON ai6_cables (expires_at);

-- Token buckets keyed by the SHA-256 of the quota key.
CREATE TABLE ai6_cable_quotas (
    key TEXT PRIMARY KEY,
    tokens DOUBLE PRECISION NOT NULL,
    updated DOUBLE PRECISION NOT NULL,
    revision BIGINT NOT NULL
);

CREATE TABLE ai6_audit (
    id TEXT PRIMARY KEY,
    at DOUBLE PRECISION NOT NULL,
    record TEXT NOT NULL,
    revision BIGINT NOT NULL
);

CREATE TABLE ai6_agent_roles (
    operation TEXT NOT NULL,
    id TEXT NOT NULL,
    role TEXT NOT NULL,
    revision BIGINT NOT NULL,
    PRIMARY KEY (operation, id)
);

-- Green ink: state is pending or decided; seq orders a Mission's requests.
CREATE TABLE ai6_approvals (
    operation TEXT NOT NULL,
    id TEXT NOT NULL,
    mission TEXT NOT NULL,
    state TEXT NOT NULL,
    record TEXT NOT NULL,
    seq BIGINT NOT NULL,
    revision BIGINT NOT NULL,
    PRIMARY KEY (operation, id)
);
CREATE INDEX ai6_approvals_mission ON ai6_approvals (operation, mission, seq);

CREATE TABLE ai6_class_authorizations (
    operation TEXT NOT NULL,
    id TEXT NOT NULL,
    created_at DOUBLE PRECISION NOT NULL,
    record TEXT NOT NULL,
    revision BIGINT NOT NULL,
    PRIMARY KEY (operation, id)
);

-- scope is the Operation, or '' for every Operation.
CREATE TABLE ai6_quarantines (
    agent TEXT NOT NULL,
    scope TEXT NOT NULL,
    record TEXT NOT NULL,
    revision BIGINT NOT NULL,
    PRIMARY KEY (agent, scope)
);

CREATE TABLE ai6_incidents (
    agent TEXT PRIMARY KEY,
    history TEXT NOT NULL,
    revision BIGINT NOT NULL
);

CREATE TABLE ai6_sessions (
    session_id TEXT PRIMARY KEY,
    mission_id TEXT NOT NULL,
    record TEXT NOT NULL,
    revision BIGINT NOT NULL
);
CREATE INDEX ai6_sessions_mission ON ai6_sessions (mission_id);

CREATE TABLE ai6_sleeper_authorizations (
    operation TEXT NOT NULL,
    name TEXT NOT NULL,
    record TEXT NOT NULL,
    revision BIGINT NOT NULL,
    PRIMARY KEY (operation, name)
);

CREATE TABLE ai6_source_cursors (
    operation TEXT NOT NULL,
    provider TEXT NOT NULL,
    cursor TEXT NOT NULL,
    revision BIGINT NOT NULL,
    PRIMARY KEY (operation, provider)
);

CREATE TABLE ai6_timeline (
    id TEXT PRIMARY KEY,
    operation TEXT NOT NULL,
    at_micros BIGINT NOT NULL,
    event TEXT NOT NULL,
    revision BIGINT NOT NULL
);
CREATE INDEX ai6_timeline_order ON ai6_timeline (at_micros, id);
CREATE INDEX ai6_timeline_operation ON ai6_timeline (operation, at_micros, id);

-- Pool accounts: claims, quota windows and daily tokens, one row per account.
CREATE TABLE ai6_accounts (
    pool TEXT NOT NULL,
    account TEXT NOT NULL,
    record TEXT NOT NULL,
    revision BIGINT NOT NULL,
    PRIMARY KEY (pool, account)
);

-- operation is '' for usage outside any Operation.
CREATE TABLE ai6_usage (
    operation TEXT NOT NULL,
    mission TEXT NOT NULL,
    ledger TEXT NOT NULL,
    revision BIGINT NOT NULL,
    PRIMARY KEY (operation, mission)
);

-- The roster: each definition is a JSON object with the keys of the record's table in
-- operations.toml, agents.toml or covers.toml.
CREATE TABLE ai6_config_operations (
    id TEXT PRIMARY KEY,
    definition TEXT NOT NULL,
    revision BIGINT NOT NULL
);

CREATE TABLE ai6_config_agents (
    id TEXT PRIMARY KEY,
    definition TEXT NOT NULL,
    revision BIGINT NOT NULL
);

CREATE TABLE ai6_config_roles (
    id TEXT PRIMARY KEY,
    definition TEXT NOT NULL,
    revision BIGINT NOT NULL
);

CREATE TABLE ai6_config_covers (
    name TEXT PRIMARY KEY,
    definition TEXT NOT NULL,
    revision BIGINT NOT NULL
);
