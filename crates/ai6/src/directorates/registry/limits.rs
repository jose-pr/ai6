//! How much the Registry keeps and accepts: [`RegistryLimits`], read from `[registry.limits]`
//! in the bootstrap file, one table per record family. Each default is the value the
//! Registry applies when the file says nothing.
//!
//! Counts and sizes are [`Limit`]s and ages are [`TimeLimit`]s: `null` (or `"none"` in TOML)
//! takes the bound away. A record refused for its size or a family refused for its count
//! fails the write with `InvalidData` (or `InvalidInput` for a request), as without
//! settings.

use serde::{Deserialize, Serialize};

use crate::bounds::{Limit, Seconds, TimeLimit};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;

/// `[registry.limits]`: every bound the Registry applies, one table per record family.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RegistryLimits {
    /// `[registry.limits.accounts]`: pool accounts.
    pub accounts: AccountLimits,
    /// `[registry.limits.agents]`: Agent records.
    pub agents: AgentLimits,
    /// `[registry.limits.agent_roles]`: Agent roles of an Operation.
    pub agent_roles: AgentRoleLimits,
    /// `[registry.limits.approvals]`: green ink requests and class authorizations.
    pub approvals: ApprovalLimits,
    /// `[registry.limits.audit]`: audit records.
    pub audit: AuditLimits,
    /// `[registry.limits.briefs]`: the brief queue.
    pub briefs: BriefLimits,
    /// `[registry.limits.cables]`: Cables.
    pub cables: CableLimits,
    /// `[registry.limits.clearance]`: clearance grants.
    pub clearance: ClearanceLimits,
    /// `[registry.limits.files]`: the file Registry's own waits.
    pub files: FileLimits,
    /// `[registry.limits.held]`: briefs held by vetting.
    pub held: HeldLimits,
    /// `[registry.limits.issues]`: linked issues and the outbox.
    pub issues: IssueLimits,
    /// `[registry.limits.logbook]`: Station logbooks.
    pub logbook: LogbookLimits,
    /// `[registry.limits.proposals]`: Mission proposals.
    pub proposals: ProposalLimits,
    /// `[registry.limits.quarantine]`: quarantines and their incidents.
    pub quarantine: QuarantineLimits,
    /// `[registry.limits.sessions]`: Session records.
    pub sessions: SessionLimits,
    /// `[registry.limits.sleepers]`: Sleepers.
    pub sleepers: SleeperLimits,
    /// `[registry.limits.sources]`: Source items and reports.
    pub sources: SourceLimits,
    /// `[registry.limits.stations]`: Station records.
    pub stations: StationLimits,
    /// `[registry.limits.timeline]`: the activity timeline.
    pub timeline: TimelineLimits,
    /// `[registry.limits.tokens]`: access tokens.
    pub tokens: TokenLimits,
    /// `[registry.limits.usage]`: the usage ledger.
    pub usage: UsageLimits,
}

impl RegistryLimits {
    /// One line per limit looser than its default, for the bootstrap file's weakenings.
    pub fn weakenings(&self) -> Vec<String> {
        let (Ok(current), Ok(default)) = (
            serde_json::to_value(self),
            serde_json::to_value(Self::default()),
        ) else {
            return Vec::new();
        };
        crate::bounds::looser("registry.limits", &current, &default, &[])
    }
}

/// `[registry.limits.accounts]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccountLimits {
    /// Bytes of one account's record. Default 256 KiB.
    pub record_bytes: Limit,
    /// Live claims on one account; past it the oldest is dropped. Default 1,024.
    pub claims: Limit,
    /// Quota windows kept per account. Default 8.
    pub windows: Limit,
    /// Days of token counts kept per account. Default 8.
    pub kept_days: u32,
}

impl Default for AccountLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(256 * KIB),
            claims: Limit::at(1024),
            windows: Limit::at(8),
            kept_days: 8,
        }
    }
}

/// `[registry.limits.agents]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentLimits {
    /// Bytes of one Agent record. Default 1 MiB.
    pub record_bytes: Limit,
    /// Operations one Agent record tallies. Default 128.
    pub operations: Limit,
    /// Clearances one Operation of an Agent record tallies. Default 8.
    pub clearances: Limit,
    /// Mission outcomes remembered per Agent, so a repeated report is counted once.
    /// Default 256.
    pub counted: Limit,
    /// Model changes kept per Agent. Default 16.
    pub model_history: Limit,
    /// Bytes of an outcome report's key. Default 512.
    pub key_bytes: Limit,
    /// Bytes of an outcome report's Agent, Operation and model. Default 256.
    pub agent_bytes: Limit,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(MIB),
            operations: Limit::at(128),
            clearances: Limit::at(8),
            counted: Limit::at(256),
            model_history: Limit::at(16),
            key_bytes: Limit::at(512),
            agent_bytes: Limit::at(256),
        }
    }
}

/// `[registry.limits.agent_roles]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentRoleLimits {
    /// Bytes of one Agent role record. Default 32 KiB.
    pub record_bytes: Limit,
    /// Agent roles per Operation. Default 256.
    pub records: Limit,
}

impl Default for AgentRoleLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(32 * KIB),
            records: Limit::at(256),
        }
    }
}

/// `[registry.limits.approvals]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApprovalLimits {
    /// Bytes of one approval record. Default 16 KiB.
    pub record_bytes: Limit,
    /// Pending approval requests. Default 1,000.
    pub pending: Limit,
    /// Class authorizations. Default 1,000.
    pub class_authorizations: Limit,
    /// Requests one Mission may have. Default 64.
    pub mission_requests: Limit,
    /// Bytes of a request's reason. Default 2,048.
    pub reason_bytes: Limit,
    /// Bytes of a decision's note. Default 1,024.
    pub note_bytes: Limit,
    /// Bytes of a signer's name. Default 256.
    pub signer_bytes: Limit,
    /// Bytes of a request's idempotency key. Default 256.
    pub idempotency_key_bytes: Limit,
    /// Step kinds, Agents, roles and proposed roles each class authorization may name.
    /// Default 64.
    pub class_entries: Limit,
    /// Bytes of a class authorization's target pattern. Default 256.
    pub pattern_bytes: Limit,
    /// Bytes of each Agent or role a class authorization names. Default 128.
    pub name_bytes: Limit,
}

impl Default for ApprovalLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(16 * KIB),
            pending: Limit::at(1_000),
            class_authorizations: Limit::at(1_000),
            mission_requests: Limit::at(64),
            reason_bytes: Limit::at(2_048),
            note_bytes: Limit::at(1_024),
            signer_bytes: Limit::at(256),
            idempotency_key_bytes: Limit::at(256),
            class_entries: Limit::at(64),
            pattern_bytes: Limit::at(256),
            name_bytes: Limit::at(128),
        }
    }
}

/// `[registry.limits.audit]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditLimits {
    /// Bytes of one audit record. Default 16 KiB.
    pub record_bytes: Limit,
    /// Audit records kept. Default 10,000.
    pub records: Limit,
    /// Records one listing may ask for. Default 500.
    pub results: Limit,
}

impl Default for AuditLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(16 * KIB),
            records: Limit::at(10_000),
            results: Limit::at(500),
        }
    }
}

/// `[registry.limits.briefs]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BriefLimits {
    /// Bytes of one brief. Default 1 MiB.
    pub record_bytes: Limit,
    /// Queued briefs. Default 1,000.
    pub records: Limit,
    /// Bytes of a brief's provenance. Default 16 KiB.
    pub provenance_bytes: Limit,
}

impl Default for BriefLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(MIB),
            records: Limit::at(1_000),
            provenance_bytes: Limit::at(16 * KIB),
        }
    }
}

/// `[registry.limits.cables]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CableLimits {
    /// Bytes of one Cable record. Default 2 MiB.
    pub record_bytes: Limit,
    /// Cable records the Registry holds. Default 10,000.
    pub records: Limit,
    /// UTF-8 bytes of a Cable's body. Default 16 KiB.
    pub body_bytes: Limit,
    /// Seconds a Cable may live. Default 86,400 (a day).
    pub max_ttl: TimeLimit,
}

impl Default for CableLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(2 * MIB),
            records: Limit::at(10_000),
            body_bytes: Limit::at(16 * KIB),
            max_ttl: TimeLimit::secs(24 * 60 * 60),
        }
    }
}

/// `[registry.limits.clearance]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClearanceLimits {
    /// Bytes of one clearance grant. Default 16 KiB.
    pub record_bytes: Limit,
    /// Clearance grants. Default 10,000.
    pub grants: Limit,
    /// Bytes of a grant's signer, approval, model and reason. Default 512.
    pub text_bytes: Limit,
}

impl Default for ClearanceLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(16 * KIB),
            grants: Limit::at(10_000),
            text_bytes: Limit::at(512),
        }
    }
}

/// `[registry.limits.files]`: how long the file Registry waits. The SQL Registry ignores it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileLimits {
    /// Seconds a record write retries replacing a file another process holds open.
    /// Default 2.
    pub rename_retry: TimeLimit,
    /// Seconds a write waits for the Registry's directory lock. Default 30.
    pub lock_wait: TimeLimit,
}

impl Default for FileLimits {
    fn default() -> Self {
        Self {
            rename_retry: TimeLimit::secs(2),
            lock_wait: TimeLimit::secs(30),
        }
    }
}

/// `[registry.limits.held]`. A held record may be 64 KiB, room for its verdict, larger than a
/// brief's `[registry.limits.briefs] record_bytes`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HeldLimits {
    /// Briefs one Operation may hold. Default 1,000.
    pub per_operation: Limit,
    /// Briefs held across the Registry. Default 10,000.
    pub records: Limit,
    /// Held briefs one listing may ask for. Default 500.
    pub results: Limit,
    /// Bytes of a stored clearance, the finding kinds a brief was released from. Default 4,096.
    pub clearance_bytes: Limit,
    /// Bytes of the reason a held brief is refused for. Default 1,024.
    pub reason_bytes: Limit,
    /// Bytes of who released or refused a held brief. Default 256.
    pub actor_bytes: Limit,
}

impl Default for HeldLimits {
    fn default() -> Self {
        Self {
            per_operation: Limit::at(1_000),
            records: Limit::at(10_000),
            results: Limit::at(500),
            clearance_bytes: Limit::at(4_096),
            reason_bytes: Limit::at(1_024),
            actor_bytes: Limit::at(256),
        }
    }
}

/// What a held record adds to its brief: the verdict and its findings.
const HELD_OVERHEAD_BYTES: u64 = 64 * KIB;

impl RegistryLimits {
    /// Bytes of one held record: a brief's bound plus room for its verdict.
    pub fn held_record_bytes(&self) -> Limit {
        match self.briefs.record_bytes.get() {
            Some(bytes) => Limit::at(bytes.saturating_add(HELD_OVERHEAD_BYTES)),
            None => Limit::UNBOUNDED,
        }
    }
}

/// `[registry.limits.logbook]`. A whole logbook's size is bounded by these four.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogbookLimits {
    /// Reports kept per logbook, and handed to the next round. Default 20.
    pub entries: Limit,
    /// Bytes of a report's summary. Default 4 KiB.
    pub summary_bytes: Limit,
    /// Findings per report. Default 32.
    pub findings: Limit,
    /// Bytes of one finding. Default 1,024.
    pub finding_bytes: Limit,
}

impl Default for LogbookLimits {
    fn default() -> Self {
        Self {
            entries: Limit::at(20),
            summary_bytes: Limit::at(4 * KIB),
            findings: Limit::at(32),
            finding_bytes: Limit::at(1024),
        }
    }
}

/// Room per finding and per report for the fields besides the bounded text.
const FINDING_OVERHEAD_BYTES: u64 = 128;
const REPORT_OVERHEAD_BYTES: u64 = 1024;

impl LogbookLimits {
    /// Bytes of a whole logbook: every report at its bounds.
    pub fn record_bytes(&self) -> Limit {
        let (Some(entries), Some(summary), Some(findings), Some(finding)) = (
            self.entries.get(),
            self.summary_bytes.get(),
            self.findings.get(),
            self.finding_bytes.get(),
        ) else {
            return Limit::UNBOUNDED;
        };
        let report = finding
            .checked_add(FINDING_OVERHEAD_BYTES)
            .and_then(|each| each.checked_mul(findings))
            .and_then(|all| all.checked_add(summary))
            .and_then(|all| all.checked_add(REPORT_OVERHEAD_BYTES))
            .and_then(|report| report.checked_mul(entries));
        report.map_or(Limit::UNBOUNDED, Limit::at)
    }
}

/// `[registry.limits.proposals]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProposalLimits {
    /// Bytes of one Mission proposal. Default 1 MiB.
    pub record_bytes: Limit,
    /// Pending Mission proposals per Operation. Default 1,000.
    pub records: Limit,
    /// Proposals one listing may ask for. Default 500.
    pub results: Limit,
    /// Bytes of a proposal's idempotency key. Default 256.
    pub idempotency_key_bytes: Limit,
}

impl Default for ProposalLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(MIB),
            records: Limit::at(1_000),
            results: Limit::at(500),
            idempotency_key_bytes: Limit::at(256),
        }
    }
}

/// `[registry.limits.quarantine]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuarantineLimits {
    /// Bytes of one quarantine record. Default 16 KiB.
    pub record_bytes: Limit,
    /// Quarantine records. Default 10,000.
    pub records: Limit,
    /// Incidents kept per Agent. Default 256.
    pub incidents: Limit,
    /// Seconds an incident is kept. Default 604,800 (a week).
    pub keep_incidents: TimeLimit,
}

impl Default for QuarantineLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(16 * KIB),
            records: Limit::at(10_000),
            incidents: Limit::at(256),
            keep_incidents: TimeLimit::secs(7 * 24 * 3600),
        }
    }
}

/// How many quarantine records an Agent's incident history may weigh.
const HISTORY_RECORDS: u64 = 16;

impl QuarantineLimits {
    /// Bytes of one Agent's incident history: sixteen records' worth.
    pub fn history_bytes(&self) -> Limit {
        match self.record_bytes.get() {
            Some(bytes) => Limit::at(bytes.saturating_mul(HISTORY_RECORDS)),
            None => Limit::UNBOUNDED,
        }
    }
}

/// `[registry.limits.sessions]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionLimits {
    /// Bytes of one Session record. Default 64 KiB.
    pub record_bytes: Limit,
    /// Session records. Default 100,000.
    pub records: Limit,
    /// Bytes of a Session ID. Default 128.
    pub id_bytes: Limit,
    /// Exposures recorded per Session. Default 32.
    pub exposures: Limit,
    /// Bytes of an exposure's source. Default 128.
    pub exposure_source_bytes: Limit,
    /// Recent event lines kept per Session. Default 20.
    pub recent_events: Limit,
    /// Characters kept of each recent event line and of a lost Session's reason.
    /// Default 300.
    pub recent_event_chars: Limit,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(64 * KIB),
            records: Limit::at(100_000),
            id_bytes: Limit::at(128),
            exposures: Limit::at(32),
            exposure_source_bytes: Limit::at(128),
            recent_events: Limit::at(20),
            recent_event_chars: Limit::at(300),
        }
    }
}

/// `[registry.limits.sleepers]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SleeperLimits {
    /// Seconds a scheduled window may be late and still fire; an older one is skipped.
    /// Default 86,400 (a day).
    pub window_age: TimeLimit,
    /// Bytes of a Sleeper's prompt. Default 16 KiB.
    pub prompt_bytes: Limit,
    /// Days a schedule is searched back for its latest window. Default 366.
    pub lookback_days: u32,
}

impl Default for SleeperLimits {
    fn default() -> Self {
        Self {
            window_age: TimeLimit::secs(24 * 60 * 60),
            prompt_bytes: Limit::at(16 * KIB),
            lookback_days: 366,
        }
    }
}

/// `[registry.limits.issues]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IssueLimits {
    /// Linked issues one Mission holds at once. Default 32.
    pub links_per_mission: Limit,
    /// Outbox entries kept, settled ones dropped oldest first. Default 1,000.
    pub outbox_records: Limit,
    /// Bytes of one outbox entry's text and payload. Default 128 KiB.
    pub entry_bytes: Limit,
}

impl Default for IssueLimits {
    fn default() -> Self {
        Self {
            links_per_mission: Limit::at(32),
            outbox_records: Limit::at(1_000),
            entry_bytes: Limit::at(128 * KIB),
        }
    }
}

/// `[registry.limits.sources]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourceLimits {
    /// Bytes of a Source item's text. Default 64 KiB.
    pub item_text_bytes: Limit,
    /// Times reporting a finished brief to its Source is tried before it is dropped.
    /// Default 5.
    pub report_attempts: Limit,
}

impl Default for SourceLimits {
    fn default() -> Self {
        Self {
            item_text_bytes: Limit::at(64 * KIB),
            report_attempts: Limit::at(5),
        }
    }
}

/// `[registry.limits.stations]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StationLimits {
    /// Bytes of one Station record. Default 1 MiB.
    pub record_bytes: Limit,
    /// Station records. Default 1,000.
    pub records: Limit,
    /// Bytes of a Station's settings record. Default 4,096.
    pub settings_bytes: Limit,
    /// Missions a Station's settings may let it run at once. Default 1,024.
    pub mission_capacity: Limit,
    /// Seconds after its last check-in that a Station counts as dark. Default 45.
    pub gone_dark_after: Seconds,
}

impl Default for StationLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(MIB),
            records: Limit::at(1_000),
            settings_bytes: Limit::at(4_096),
            mission_capacity: Limit::at(1_024),
            gone_dark_after: Seconds::new(45),
        }
    }
}

/// `[registry.limits.timeline]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimelineLimits {
    /// Events kept; the oldest are dropped past it. Default 10,000.
    pub records: Limit,
    /// Bytes of one event. Default 8,192.
    pub record_bytes: Limit,
    /// Bytes of an event's ID. Default 128.
    pub id_bytes: Limit,
    /// Bytes of an event's kind, action and detail keys. Default 64.
    pub name_bytes: Limit,
    /// Bytes of an event's Operation, Station, Mission, Session, actor and detail values.
    /// Default 256.
    pub text_bytes: Limit,
    /// Detail entries per event. Default 8.
    pub detail_entries: Limit,
}

impl Default for TimelineLimits {
    fn default() -> Self {
        Self {
            records: Limit::at(10_000),
            record_bytes: Limit::at(8_192),
            id_bytes: Limit::at(128),
            name_bytes: Limit::at(64),
            text_bytes: Limit::at(256),
            detail_entries: Limit::at(8),
        }
    }
}

/// `[registry.limits.tokens]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TokenLimits {
    /// Bytes of one access token record. Default 16 KiB.
    pub record_bytes: Limit,
}

impl Default for TokenLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(16 * KIB),
        }
    }
}

/// `[registry.limits.usage]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UsageLimits {
    /// Bytes of one Mission's usage ledger. Default 1 MiB.
    pub ledger_bytes: Limit,
    /// Days of usage kept. Default 8.
    pub kept_days: u32,
}

impl Default for UsageLimits {
    fn default() -> Self {
        Self {
            ledger_bytes: Limit::at(MIB),
            kept_days: 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_bounds_follow_their_parts() {
        let limits = RegistryLimits::default();
        assert_eq!(
            limits.logbook.record_bytes(),
            Limit::at(20 * (4 * 1024 + 32 * (1024 + 128) + 1024))
        );
        assert_eq!(limits.held_record_bytes(), Limit::at(MIB + 64 * KIB));
        let mut open = RegistryLimits::default();
        open.logbook.findings = Limit::UNBOUNDED;
        open.briefs.record_bytes = Limit::UNBOUNDED;
        assert_eq!(open.logbook.record_bytes(), Limit::UNBOUNDED);
        assert_eq!(open.held_record_bytes(), Limit::UNBOUNDED);
    }

    #[test]
    fn a_table_reads_partially_and_names_what_it_loosens() {
        let limits: RegistryLimits =
            toml::from_str("[sessions]\nrecords = \"none\"\n[files]\nlock_wait = 5\n").unwrap();
        assert_eq!(limits.sessions.records, Limit::UNBOUNDED);
        assert_eq!(limits.sessions.id_bytes, Limit::at(128));
        assert_eq!(
            limits.weakenings(),
            ["registry.limits.sessions.records none is looser than the default 100000"]
        );
        assert!(toml::from_str::<RegistryLimits>("[sessions]\nrecordz = 1\n").is_err());
    }
}
