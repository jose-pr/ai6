//! How much the Registry keeps and accepts: [`RegistryLimits`], read from `[registry.limits]`
//! in the bootstrap file, one table per record family. Each default is the value the
//! Registry applies when the file says nothing.
//!
//! Counts and sizes are [`Limit`]s and ages are [`TimeLimit`]s: `null` (or `"none"` in TOML)
//! takes the bound away. A record refused for its size or a family refused for its count
//! fails the write with `InvalidData` (or `InvalidInput` for a request), as without
//! settings.

use guatiao::{FromValue, Schema, ToValue};

use crate::bounds::{Limit, Seconds, TimeLimit};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * 1024;

/// `[registry.limits]`: every bound the Registry applies, one table per record family.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct RegistryLimits {
    /// `[registry.limits.accounts]`: pool accounts.
    #[schema(title = "Pool accounts")]
    #[map(default)]
    pub accounts: AccountLimits,
    /// `[registry.limits.agents]`: Agent records.
    #[schema(title = "Agent records")]
    #[map(default)]
    pub agents: AgentLimits,
    /// `[registry.limits.agent_roles]`: Agent roles of an Operation.
    #[schema(title = "Agent roles of an Operation")]
    #[map(default)]
    pub agent_roles: AgentRoleLimits,
    /// `[registry.limits.approvals]`: green ink requests and class authorizations.
    #[schema(title = "Green ink requests and class authorizations")]
    #[map(default)]
    pub approvals: ApprovalLimits,
    /// `[registry.limits.audit]`: audit records.
    #[schema(title = "Audit records")]
    #[map(default)]
    pub audit: AuditLimits,
    /// `[registry.limits.briefs]`: the brief queue.
    #[schema(title = "The brief queue")]
    #[map(default)]
    pub briefs: BriefLimits,
    /// `[registry.limits.cables]`: Cables.
    #[schema(title = "Cables")]
    #[map(default)]
    pub cables: CableLimits,
    /// `[registry.limits.clearance]`: clearance grants.
    #[schema(title = "Clearance grants")]
    #[map(default)]
    pub clearance: ClearanceLimits,
    /// `[registry.limits.files]`: the file Registry's own waits.
    #[schema(title = "The file Registry's own waits")]
    #[map(default)]
    pub files: FileLimits,
    /// `[registry.limits.held]`: briefs held by vetting.
    #[schema(title = "Briefs held by vetting")]
    #[map(default)]
    pub held: HeldLimits,
    /// `[registry.limits.issues]`: linked issues and the outbox.
    #[schema(title = "Linked issues and the outbox")]
    #[map(default)]
    pub issues: IssueLimits,
    /// `[registry.limits.logbook]`: Station logbooks.
    #[schema(title = "Station logbooks")]
    #[map(default)]
    pub logbook: LogbookLimits,
    /// `[registry.limits.proposals]`: Mission proposals.
    #[schema(title = "Mission proposals")]
    #[map(default)]
    pub proposals: ProposalLimits,
    /// `[registry.limits.quarantine]`: quarantines and their incidents.
    #[schema(title = "Quarantines and their incidents")]
    #[map(default)]
    pub quarantine: QuarantineLimits,
    /// `[registry.limits.sessions]`: Session records.
    #[schema(title = "Session records")]
    #[map(default)]
    pub sessions: SessionLimits,
    /// `[registry.limits.sleepers]`: Sleepers.
    #[schema(title = "Sleepers")]
    #[map(default)]
    pub sleepers: SleeperLimits,
    /// `[registry.limits.sources]`: Source items and reports.
    #[schema(title = "Source items and reports")]
    #[map(default)]
    pub sources: SourceLimits,
    /// `[registry.limits.stations]`: Station records.
    #[schema(title = "Station records")]
    #[map(default)]
    pub stations: StationLimits,
    /// `[registry.limits.timeline]`: the activity timeline.
    #[schema(title = "The activity timeline")]
    #[map(default)]
    pub timeline: TimelineLimits,
    /// `[registry.limits.tokens]`: access tokens.
    #[schema(title = "Access tokens")]
    #[map(default)]
    pub tokens: TokenLimits,
    /// `[registry.limits.usage]`: the usage ledger.
    #[schema(title = "The usage ledger")]
    #[map(default)]
    pub usage: UsageLimits,
}

impl RegistryLimits {
    /// One line per limit looser than its default, for the bootstrap file's weakenings.
    pub fn weakenings(&self) -> Vec<String> {
        let (Some(current), Some(default)) = (
            crate::directorates::m::records::to_json(self),
            crate::directorates::m::records::to_json(&Self::default()),
        ) else {
            return Vec::new();
        };
        crate::bounds::looser("registry.limits", &current, &default, &[])
    }
}

/// `[registry.limits.accounts]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AccountLimits {
    /// Bytes of one account's record.
    #[schema(title = "Bytes of one account's record", format = "bytes")]
    #[map(default = AccountLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Live claims on one account; past it the oldest is dropped.
    #[schema(title = "Live claims on one account")]
    #[map(default = AccountLimits::default().claims)]
    pub claims: Limit,
    /// Quota windows kept per account.
    #[schema(title = "Quota windows kept per account")]
    #[map(default = AccountLimits::default().windows)]
    pub windows: Limit,
    /// Days of token counts kept per account.
    #[schema(title = "Days of token counts kept per account")]
    #[map(default = AccountLimits::default().kept_days)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AgentLimits {
    /// Bytes of one Agent record.
    #[schema(title = "Bytes of one Agent record", format = "bytes")]
    #[map(default = AgentLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Operations one Agent record tallies.
    #[schema(title = "Operations one Agent record tallies")]
    #[map(default = AgentLimits::default().operations)]
    pub operations: Limit,
    /// Clearances one Operation of an Agent record tallies.
    #[schema(title = "Clearances one Operation of an Agent record tallies")]
    #[map(default = AgentLimits::default().clearances)]
    pub clearances: Limit,
    /// Mission outcomes remembered per Agent, so a repeated report is counted once.
    #[schema(title = "Mission outcomes remembered per Agent")]
    #[map(default = AgentLimits::default().counted)]
    pub counted: Limit,
    /// Model changes kept per Agent.
    #[schema(title = "Model changes kept per Agent")]
    #[map(default = AgentLimits::default().model_history)]
    pub model_history: Limit,
    /// Bytes of an outcome report's key.
    #[schema(title = "Bytes of an outcome report's key", format = "bytes")]
    #[map(default = AgentLimits::default().key_bytes)]
    pub key_bytes: Limit,
    /// Bytes of an outcome report's Agent, Operation and model.
    #[schema(
        title = "Bytes of an outcome report's Agent, Operation and model",
        format = "bytes"
    )]
    #[map(default = AgentLimits::default().agent_bytes)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AgentRoleLimits {
    /// Bytes of one Agent role record.
    #[schema(title = "Bytes of one Agent role record", format = "bytes")]
    #[map(default = AgentRoleLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Agent roles per Operation.
    #[schema(title = "Agent roles per Operation")]
    #[map(default = AgentRoleLimits::default().records)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ApprovalLimits {
    /// Bytes of one approval record.
    #[schema(title = "Bytes of one approval record", format = "bytes")]
    #[map(default = ApprovalLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Pending approval requests.
    #[schema(title = "Pending approval requests")]
    #[map(default = ApprovalLimits::default().pending)]
    pub pending: Limit,
    /// Class authorizations.
    #[schema(title = "Class authorizations")]
    #[map(default = ApprovalLimits::default().class_authorizations)]
    pub class_authorizations: Limit,
    /// Requests one Mission may have.
    #[schema(title = "Requests one Mission may have")]
    #[map(default = ApprovalLimits::default().mission_requests)]
    pub mission_requests: Limit,
    /// Bytes of a request's reason.
    #[schema(title = "Bytes of a request's reason", format = "bytes")]
    #[map(default = ApprovalLimits::default().reason_bytes)]
    pub reason_bytes: Limit,
    /// Bytes of a decision's note.
    #[schema(title = "Bytes of a decision's note", format = "bytes")]
    #[map(default = ApprovalLimits::default().note_bytes)]
    pub note_bytes: Limit,
    /// Bytes of a signer's name.
    #[schema(title = "Bytes of a signer's name", format = "bytes")]
    #[map(default = ApprovalLimits::default().signer_bytes)]
    pub signer_bytes: Limit,
    /// Bytes of a request's idempotency key.
    #[schema(title = "Bytes of a request's idempotency key", format = "bytes")]
    #[map(default = ApprovalLimits::default().idempotency_key_bytes)]
    pub idempotency_key_bytes: Limit,
    /// Step kinds, Agents, roles and proposed roles each class authorization may name.
    #[schema(title = "Class entries")]
    #[map(default = ApprovalLimits::default().class_entries)]
    pub class_entries: Limit,
    /// Bytes of a class authorization's target pattern.
    #[schema(
        title = "Bytes of a class authorization's target pattern",
        format = "bytes"
    )]
    #[map(default = ApprovalLimits::default().pattern_bytes)]
    pub pattern_bytes: Limit,
    /// Bytes of each Agent or role a class authorization names.
    #[schema(
        title = "Bytes of each Agent or role a class authorization names",
        format = "bytes"
    )]
    #[map(default = ApprovalLimits::default().name_bytes)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AuditLimits {
    /// Bytes of one audit record.
    #[schema(title = "Bytes of one audit record", format = "bytes")]
    #[map(default = AuditLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Audit records kept.
    #[schema(title = "Audit records kept")]
    #[map(default = AuditLimits::default().records)]
    pub records: Limit,
    /// Records one listing may ask for.
    #[schema(title = "Records one listing may ask for")]
    #[map(default = AuditLimits::default().results)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct BriefLimits {
    /// Bytes of one brief.
    #[schema(title = "Bytes of one brief", format = "bytes")]
    #[map(default = BriefLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Queued briefs.
    #[schema(title = "Queued briefs")]
    #[map(default = BriefLimits::default().records)]
    pub records: Limit,
    /// Bytes of a brief's provenance.
    #[schema(title = "Bytes of a brief's provenance", format = "bytes")]
    #[map(default = BriefLimits::default().provenance_bytes)]
    pub provenance_bytes: Limit,
    /// Bytes of a Mission ID: a longer one keeps its start and adds a short hash. It is a file
    /// name, so it stays under 240 bytes whatever this says. Default 120, which leaves a run's
    /// attempt room within `[registry.limits.sessions] id_bytes`.
    #[schema(title = "Mission id bytes", format = "bytes")]
    #[map(default = BriefLimits::default().mission_id_bytes)]
    pub mission_id_bytes: Limit,
}

impl Default for BriefLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(MIB),
            records: Limit::at(1_000),
            provenance_bytes: Limit::at(16 * KIB),
            mission_id_bytes: Limit::at(120),
        }
    }
}

/// `[registry.limits.cables]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct CableLimits {
    /// Bytes of one Cable record.
    #[schema(title = "Bytes of one Cable record", format = "bytes")]
    #[map(default = CableLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Cable records the Registry holds.
    #[schema(title = "Cable records the Registry holds")]
    #[map(default = CableLimits::default().records)]
    pub records: Limit,
    /// UTF-8 bytes of a Cable's body.
    #[schema(title = "UTF-8 bytes of a Cable's body", format = "bytes")]
    #[map(default = CableLimits::default().body_bytes)]
    pub body_bytes: Limit,
    /// Seconds a Cable may live.
    #[schema(title = "Seconds a Cable may live")]
    #[map(default = CableLimits::default().max_ttl)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ClearanceLimits {
    /// Bytes of one clearance grant.
    #[schema(title = "Bytes of one clearance grant", format = "bytes")]
    #[map(default = ClearanceLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Clearance grants.
    #[schema(title = "Clearance grants")]
    #[map(default = ClearanceLimits::default().grants)]
    pub grants: Limit,
    /// Bytes of a grant's signer, approval, model and reason.
    #[schema(
        title = "Bytes of a grant's signer, approval, model and reason",
        format = "bytes"
    )]
    #[map(default = ClearanceLimits::default().text_bytes)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct FileLimits {
    /// Seconds a record write retries replacing a file another process holds open.
    #[schema(title = "Rename retry")]
    #[map(default = FileLimits::default().rename_retry)]
    pub rename_retry: TimeLimit,
    /// Seconds a write waits for the Registry's directory lock.
    #[schema(title = "Seconds a write waits for the Registry's directory lock")]
    #[map(default = FileLimits::default().lock_wait)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct HeldLimits {
    /// Briefs one Operation may hold.
    #[schema(title = "Briefs one Operation may hold")]
    #[map(default = HeldLimits::default().per_operation)]
    pub per_operation: Limit,
    /// Briefs held across the Registry.
    #[schema(title = "Briefs held across the Registry")]
    #[map(default = HeldLimits::default().records)]
    pub records: Limit,
    /// Held briefs one listing may ask for.
    #[schema(title = "Held briefs one listing may ask for")]
    #[map(default = HeldLimits::default().results)]
    pub results: Limit,
    /// Bytes of a stored clearance, the finding kinds a brief was released from.
    #[schema(title = "Clearance bytes", format = "bytes")]
    #[map(default = HeldLimits::default().clearance_bytes)]
    pub clearance_bytes: Limit,
    /// Bytes of the reason a held brief is refused for.
    #[schema(
        title = "Bytes of the reason a held brief is refused for",
        format = "bytes"
    )]
    #[map(default = HeldLimits::default().reason_bytes)]
    pub reason_bytes: Limit,
    /// Bytes of who released or refused a held brief.
    #[schema(
        title = "Bytes of who released or refused a held brief",
        format = "bytes"
    )]
    #[map(default = HeldLimits::default().actor_bytes)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct LogbookLimits {
    /// Reports kept per logbook, and handed to the next round.
    #[schema(title = "Reports kept per logbook, and handed to the next round")]
    #[map(default = LogbookLimits::default().entries)]
    pub entries: Limit,
    /// Bytes of a report's summary.
    #[schema(title = "Bytes of a report's summary", format = "bytes")]
    #[map(default = LogbookLimits::default().summary_bytes)]
    pub summary_bytes: Limit,
    /// Findings per report.
    #[schema(title = "Findings per report")]
    #[map(default = LogbookLimits::default().findings)]
    pub findings: Limit,
    /// Bytes of one finding.
    #[schema(title = "Bytes of one finding", format = "bytes")]
    #[map(default = LogbookLimits::default().finding_bytes)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ProposalLimits {
    /// Bytes of one Mission proposal.
    #[schema(title = "Bytes of one Mission proposal", format = "bytes")]
    #[map(default = ProposalLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Pending Mission proposals per Operation.
    #[schema(title = "Pending Mission proposals per Operation")]
    #[map(default = ProposalLimits::default().records)]
    pub records: Limit,
    /// Proposals one listing may ask for.
    #[schema(title = "Proposals one listing may ask for")]
    #[map(default = ProposalLimits::default().results)]
    pub results: Limit,
    /// Bytes of a proposal's idempotency key.
    #[schema(title = "Bytes of a proposal's idempotency key", format = "bytes")]
    #[map(default = ProposalLimits::default().idempotency_key_bytes)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct QuarantineLimits {
    /// Bytes of one quarantine record.
    #[schema(title = "Bytes of one quarantine record", format = "bytes")]
    #[map(default = QuarantineLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Quarantine records.
    #[schema(title = "Quarantine records")]
    #[map(default = QuarantineLimits::default().records)]
    pub records: Limit,
    /// Incidents kept per Agent.
    #[schema(title = "Incidents kept per Agent")]
    #[map(default = QuarantineLimits::default().incidents)]
    pub incidents: Limit,
    /// Seconds an incident is kept.
    #[schema(title = "Seconds an incident is kept")]
    #[map(default = QuarantineLimits::default().keep_incidents)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct SessionLimits {
    /// Bytes of one Session record.
    #[schema(title = "Bytes of one Session record", format = "bytes")]
    #[map(default = SessionLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Session records.
    #[schema(title = "Session records")]
    #[map(default = SessionLimits::default().records)]
    pub records: Limit,
    /// Bytes of a Session ID.
    #[schema(title = "Bytes of a Session ID. Default 128", format = "bytes")]
    #[map(default = SessionLimits::default().id_bytes)]
    pub id_bytes: Limit,
    /// Exposures recorded per Session.
    #[schema(title = "Exposures recorded per Session")]
    #[map(default = SessionLimits::default().exposures)]
    pub exposures: Limit,
    /// Bytes of an exposure's source.
    #[schema(title = "Bytes of an exposure's source", format = "bytes")]
    #[map(default = SessionLimits::default().exposure_source_bytes)]
    pub exposure_source_bytes: Limit,
    /// Recent event lines kept per Session.
    #[schema(title = "Recent event lines kept per Session")]
    #[map(default = SessionLimits::default().recent_events)]
    pub recent_events: Limit,
    /// Characters kept of each recent event line and of a lost Session's reason.
    #[schema(title = "Recent event chars")]
    #[map(default = SessionLimits::default().recent_event_chars)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct SleeperLimits {
    /// Seconds a scheduled window may be late and still fire; an older one is skipped.
    #[schema(title = "Seconds a scheduled window may be late and still fire")]
    #[map(default = SleeperLimits::default().window_age)]
    pub window_age: TimeLimit,
    /// Bytes of a Sleeper's prompt.
    #[schema(title = "Bytes of a Sleeper's prompt", format = "bytes")]
    #[map(default = SleeperLimits::default().prompt_bytes)]
    pub prompt_bytes: Limit,
    /// Days a schedule is searched back for its latest window.
    #[schema(title = "Days a schedule is searched back for its latest window")]
    #[map(default = SleeperLimits::default().lookback_days)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct IssueLimits {
    /// Linked issues one Mission holds at once.
    #[schema(title = "Linked issues one Mission holds at once")]
    #[map(default = IssueLimits::default().links_per_mission)]
    pub links_per_mission: Limit,
    /// Outbox entries kept, settled ones dropped oldest first.
    #[schema(title = "Outbox entries kept, settled ones dropped oldest first")]
    #[map(default = IssueLimits::default().outbox_records)]
    pub outbox_records: Limit,
    /// Bytes of one outbox entry's text and payload.
    #[schema(
        title = "Bytes of one outbox entry's text and payload",
        format = "bytes"
    )]
    #[map(default = IssueLimits::default().entry_bytes)]
    pub entry_bytes: Limit,
    /// Replies from a linked issue one Mission stores; delivered ones make room first.
    #[schema(title = "Replies from a linked issue one Mission stores")]
    #[map(default = IssueLimits::default().replies_per_mission)]
    pub replies_per_mission: Limit,
    /// Bytes of one stored reply, cut past it.
    #[schema(title = "Bytes of one stored reply, cut past it", format = "bytes")]
    #[map(default = IssueLimits::default().reply_bytes)]
    pub reply_bytes: Limit,
}

impl Default for IssueLimits {
    fn default() -> Self {
        Self {
            links_per_mission: Limit::at(32),
            outbox_records: Limit::at(1_000),
            entry_bytes: Limit::at(128 * KIB),
            replies_per_mission: Limit::at(200),
            reply_bytes: Limit::at(64 * KIB),
        }
    }
}

/// `[registry.limits.sources]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct SourceLimits {
    /// Bytes of a Source item's text.
    #[schema(title = "Bytes of a Source item's text", format = "bytes")]
    #[map(default = SourceLimits::default().item_text_bytes)]
    pub item_text_bytes: Limit,
    /// Times reporting a finished brief to its Source is tried before it is dropped.
    #[schema(title = "Report attempts")]
    #[map(default = SourceLimits::default().report_attempts)]
    pub report_attempts: Limit,
    /// Bytes of the name a brief shows beside its Mission ID.
    #[schema(title = "Name bytes", format = "bytes")]
    #[map(default = SourceLimits::default().name_bytes)]
    pub name_bytes: Limit,
}

impl Default for SourceLimits {
    fn default() -> Self {
        Self {
            item_text_bytes: Limit::at(64 * KIB),
            report_attempts: Limit::at(5),
            name_bytes: Limit::at(256),
        }
    }
}

/// `[registry.limits.stations]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct StationLimits {
    /// Bytes of one Station record.
    #[schema(title = "Bytes of one Station record", format = "bytes")]
    #[map(default = StationLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Station records.
    #[schema(title = "Station records")]
    #[map(default = StationLimits::default().records)]
    pub records: Limit,
    /// Bytes of a Station's settings record.
    #[schema(title = "Bytes of a Station's settings record", format = "bytes")]
    #[map(default = StationLimits::default().settings_bytes)]
    pub settings_bytes: Limit,
    /// Missions a Station's settings may let it run at once.
    #[schema(title = "Missions a Station's settings may let it run at once")]
    #[map(default = StationLimits::default().mission_capacity)]
    pub mission_capacity: Limit,
    /// Seconds after its last check-in that a Station counts as dark.
    #[schema(title = "Seconds after its last check-in that a Station counts as dark")]
    #[map(default = StationLimits::default().gone_dark_after)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct TimelineLimits {
    /// Events kept; the oldest are dropped past it.
    #[schema(title = "Events kept")]
    #[map(default = TimelineLimits::default().records)]
    pub records: Limit,
    /// Bytes of one event.
    #[schema(title = "Bytes of one event", format = "bytes")]
    #[map(default = TimelineLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Bytes of an event's ID.
    #[schema(title = "Bytes of an event's ID. Default 128", format = "bytes")]
    #[map(default = TimelineLimits::default().id_bytes)]
    pub id_bytes: Limit,
    /// Bytes of an event's kind, action and detail keys.
    #[schema(
        title = "Bytes of an event's kind, action and detail keys",
        format = "bytes"
    )]
    #[map(default = TimelineLimits::default().name_bytes)]
    pub name_bytes: Limit,
    /// Bytes of an event's Operation, Station, Mission, Session, actor and detail values.
    #[schema(title = "Text bytes", format = "bytes")]
    #[map(default = TimelineLimits::default().text_bytes)]
    pub text_bytes: Limit,
    /// Detail entries per event.
    #[schema(title = "Detail entries per event")]
    #[map(default = TimelineLimits::default().detail_entries)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct TokenLimits {
    /// Bytes of one access token record.
    #[schema(title = "Bytes of one access token record", format = "bytes")]
    #[map(default = TokenLimits::default().record_bytes)]
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
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct UsageLimits {
    /// Bytes of one Mission's usage ledger.
    #[schema(title = "Bytes of one Mission's usage ledger", format = "bytes")]
    #[map(default = UsageLimits::default().ledger_bytes)]
    pub ledger_bytes: Limit,
    /// Days of usage kept.
    #[schema(title = "Days of usage kept")]
    #[map(default = UsageLimits::default().kept_days)]
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
        let limits: RegistryLimits = crate::directorates::m::records::from_toml(
            "[sessions]\nrecords = \"none\"\n[files]\nlock_wait = 5\n",
        )
        .unwrap();
        assert_eq!(limits.sessions.records, Limit::UNBOUNDED);
        assert_eq!(limits.sessions.id_bytes, Limit::at(128));
        assert_eq!(
            limits.weakenings(),
            ["registry.limits.sessions.records none is looser than the default 100000"]
        );
        assert!(
            crate::directorates::m::records::from_toml::<RegistryLimits>(
                "[sessions]\nrecordz = 1\n"
            )
            .is_err()
        );
    }
}
