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
    /// `[registry.limits.assistant_log]`: Moneypenny's conversation log.
    #[schema(title = "Moneypenny's conversation log")]
    #[map(default)]
    pub assistant_log: AssistantLogLimits,
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
    /// `[registry.limits.contexts]`: each Session's harness context.
    #[schema(title = "Session contexts")]
    #[map(default)]
    pub contexts: ContextLimits,
    /// `[registry.limits.files]`: the file Registry's own waits.
    #[schema(title = "The file Registry's own waits")]
    #[map(default)]
    pub files: FileLimits,
    /// `[registry.limits.held]`: briefs held by vetting.
    #[schema(title = "Briefs held by vetting")]
    #[map(default)]
    pub held: HeldLimits,
    /// `[registry.limits.holds]`: Agents and routes held out of use.
    #[schema(title = "Holds")]
    #[map(default)]
    pub holds: HoldLimits,
    /// `[registry.limits.issues]`: linked issues and the outbox.
    #[schema(title = "Linked issues and the outbox")]
    #[map(default)]
    pub issues: IssueLimits,
    /// `[registry.limits.logbook]`: Station logbooks.
    #[schema(title = "Station logbooks")]
    #[map(default)]
    pub logbook: LogbookLimits,
    /// `[registry.limits.memories]`: memory entries, per scope.
    #[schema(title = "Memory entries")]
    #[map(default)]
    pub memories: MemoryLimits,
    /// `[registry.limits.proposals]`: Mission proposals.
    #[schema(title = "Mission proposals")]
    #[map(default)]
    pub proposals: ProposalLimits,
    /// `[registry.limits.quarantine]`: quarantines and their incidents.
    #[schema(title = "Quarantines and their incidents")]
    #[map(default)]
    pub quarantine: QuarantineLimits,
    /// `[registry.limits.reports]`: Reports.
    #[schema(title = "Reports")]
    #[map(default)]
    pub reports: ReportLimits,
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
    /// `[registry.limits.watches]`: watch records.
    #[schema(title = "Watch records")]
    #[map(default)]
    pub watches: super::watches::WatchLimits,
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
        let mut lines = crate::bounds::looser("registry.limits", &current, &default, &[]);
        // Its default depends on the window, so there is no number for the walk to compare.
        if self
            .sleepers
            .catch_up
            .is_some_and(|bound| bound.get().is_none())
        {
            lines.push(
                "registry.limits.sleepers.catch_up none is looser than the default, a window's \
                 own period"
                    .to_owned(),
            );
        }
        lines
    }
}

/// `[registry.limits.accounts]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AccountLimits {
    /// Bytes of one account's record.
    #[schema(title = "Largest account record", format = "bytes")]
    #[map(default = AccountLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Live claims on one account; past it the oldest is dropped.
    #[schema(title = "Live claims per account")]
    #[map(default = AccountLimits::default().claims)]
    pub claims: Limit,
    /// Quota windows kept per account.
    #[schema(title = "Quota windows per account")]
    #[map(default = AccountLimits::default().windows)]
    pub windows: Limit,
    /// Days of token counts kept per account.
    #[schema(title = "Days of token counts per account")]
    #[map(default = AccountLimits::default().kept_days)]
    pub kept_days: u32,
    /// Bytes of one sealed value's record, every Station's copy together.
    #[schema(title = "Largest sealed value", format = "bytes")]
    #[map(default = AccountLimits::default().sealed_bytes)]
    pub sealed_bytes: Limit,
}

impl Default for AccountLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(256 * KIB),
            claims: Limit::at(1024),
            windows: Limit::at(8),
            kept_days: 8,
            sealed_bytes: Limit::at(4 * MIB),
        }
    }
}

/// `[registry.limits.agents]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AgentLimits {
    /// Bytes of one Agent record.
    #[schema(title = "Largest Agent record", format = "bytes")]
    #[map(default = AgentLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Operations one Agent record tallies.
    #[schema(title = "Operations per Agent record")]
    #[map(default = AgentLimits::default().operations)]
    pub operations: Limit,
    /// Clearances one Operation of an Agent record tallies.
    #[schema(title = "Clearances per Operation of an Agent record")]
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
    #[schema(title = "Longest outcome report key", format = "bytes")]
    #[map(default = AgentLimits::default().key_bytes)]
    pub key_bytes: Limit,
    /// Bytes of an outcome report's Agent, Operation and model.
    #[schema(
        title = "Longest Agent, Operation or model in a report",
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
    #[schema(title = "Largest Agent role record", format = "bytes")]
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
    #[schema(title = "Largest approval record", format = "bytes")]
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
    #[schema(title = "Requests per Mission")]
    #[map(default = ApprovalLimits::default().mission_requests)]
    pub mission_requests: Limit,
    /// Bytes of a request's reason.
    #[schema(title = "Longest request reason", format = "bytes")]
    #[map(default = ApprovalLimits::default().reason_bytes)]
    pub reason_bytes: Limit,
    /// Bytes of a decision's note.
    #[schema(title = "Longest decision note", format = "bytes")]
    #[map(default = ApprovalLimits::default().note_bytes)]
    pub note_bytes: Limit,
    /// Bytes of a signer's name.
    #[schema(title = "Longest signer name", format = "bytes")]
    #[map(default = ApprovalLimits::default().signer_bytes)]
    pub signer_bytes: Limit,
    /// Bytes of a request's idempotency key.
    #[schema(title = "Longest idempotency key", format = "bytes")]
    #[map(default = ApprovalLimits::default().idempotency_key_bytes)]
    pub idempotency_key_bytes: Limit,
    /// Step kinds, Agents, roles and proposed roles each class authorization may name.
    #[schema(title = "Entries per class authorization")]
    #[map(default = ApprovalLimits::default().class_entries)]
    pub class_entries: Limit,
    /// Bytes of a class authorization's target pattern.
    #[schema(title = "Longest target pattern", format = "bytes")]
    #[map(default = ApprovalLimits::default().pattern_bytes)]
    pub pattern_bytes: Limit,
    /// Bytes of each Agent or role a class authorization names.
    #[schema(title = "Longest name in a class authorization", format = "bytes")]
    #[map(default = ApprovalLimits::default().name_bytes)]
    pub name_bytes: Limit,
    /// Expired requests one list shows, the newest first.
    #[schema(title = "Expired requests listed")]
    #[map(default = ApprovalLimits::default().expired_listed)]
    pub expired_listed: Limit,
    /// Links one request may carry.
    #[schema(title = "Links per request")]
    #[map(default = ApprovalLimits::default().links)]
    pub links: Limit,
    /// Bytes of a link's label.
    #[schema(title = "Longest link label", format = "bytes")]
    #[map(default = ApprovalLimits::default().link_label_bytes)]
    pub link_label_bytes: Limit,
    /// Bytes of a link's address.
    #[schema(title = "Longest link address", format = "bytes")]
    #[map(default = ApprovalLimits::default().link_url_bytes)]
    pub link_url_bytes: Limit,
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
            expired_listed: Limit::at(200),
            links: Limit::at(8),
            link_label_bytes: Limit::at(128),
            link_url_bytes: Limit::at(1_024),
        }
    }
}

/// `[registry.limits.assistant_log]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AssistantLogLimits {
    /// Bytes of one entry of the log; a longer one is kept without its data and with its text
    /// cut, and says how large it was.
    #[schema(title = "Largest log entry", format = "bytes")]
    #[map(default = AssistantLogLimits::default().entry_bytes)]
    pub entry_bytes: Limit,
}

impl Default for AssistantLogLimits {
    fn default() -> Self {
        Self {
            entry_bytes: Limit::at(MIB),
        }
    }
}

/// `[registry.limits.contexts]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ContextLimits {
    /// Bytes of one Session's context kept; a longer one keeps its newest part and says how
    /// large it was.
    #[schema(title = "Largest Session context", format = "bytes")]
    #[map(default = ContextLimits::default().context_bytes)]
    pub context_bytes: Limit,
    /// Session contexts kept; the oldest filed go first.
    #[schema(title = "Session contexts kept")]
    #[map(default = ContextLimits::default().records)]
    pub records: Limit,
}

impl Default for ContextLimits {
    fn default() -> Self {
        Self {
            context_bytes: Limit::at(8 * MIB),
            records: Limit::at(2_000),
        }
    }
}

/// `[registry.limits.audit]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct AuditLimits {
    /// Bytes of one audit record.
    #[schema(title = "Largest audit record", format = "bytes")]
    #[map(default = AuditLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Audit records kept.
    #[schema(title = "Audit records kept")]
    #[map(default = AuditLimits::default().records)]
    pub records: Limit,
    /// Records one listing may ask for.
    #[schema(title = "Most records in one listing")]
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
    #[schema(title = "Largest brief", format = "bytes")]
    #[map(default = BriefLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Queued briefs.
    #[schema(title = "Queued briefs")]
    #[map(default = BriefLimits::default().records)]
    pub records: Limit,
    /// Bytes of a brief's provenance.
    #[schema(title = "Longest brief provenance", format = "bytes")]
    #[map(default = BriefLimits::default().provenance_bytes)]
    pub provenance_bytes: Limit,
    /// Bytes of a Mission's handle: a longer one AI6 gives keeps its start and adds a short hash,
    /// and a rename to a longer one is refused. It names a Git branch, so it stays under 240
    /// bytes whatever this says.
    #[schema(title = "Longest Mission handle", format = "bytes")]
    #[map(default = BriefLimits::default().handle_bytes)]
    pub handle_bytes: Limit,
    /// Seconds after a cancel was asked for before the Headquarters Station settles a claimed
    /// brief its Station has not finished: filed as failed, its lease released.
    #[schema(title = "Settle an unconfirmed cancel after")]
    #[map(default = BriefLimits::default().cancel_settle_after)]
    pub cancel_settle_after: Seconds,
}

impl Default for BriefLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(MIB),
            records: Limit::at(1_000),
            provenance_bytes: Limit::at(16 * KIB),
            handle_bytes: Limit::at(120),
            cancel_settle_after: Seconds::new(300),
        }
    }
}

/// `[registry.limits.cables]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct CableLimits {
    /// Bytes of one Cable record.
    #[schema(title = "Largest Cable record", format = "bytes")]
    #[map(default = CableLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Cable records the Registry holds.
    #[schema(title = "Cable records kept")]
    #[map(default = CableLimits::default().records)]
    pub records: Limit,
    /// UTF-8 bytes of a Cable's body the Registry stores.
    #[schema(title = "Longest Cable body", format = "bytes")]
    #[map(default = CableLimits::default().body_bytes)]
    pub body_bytes: Limit,
    /// Seconds a Cable the Registry stores may live.
    #[schema(title = "Longest a Cable lives")]
    #[map(default = CableLimits::default().max_ttl)]
    pub max_ttl: TimeLimit,
    /// Cables one read of an inbox or of the held Cables takes.
    #[schema(title = "Cables in one read")]
    #[map(default = CableLimits::default().page)]
    pub page: Limit,
}

impl Default for CableLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(2 * MIB),
            records: Limit::at(10_000),
            body_bytes: Limit::at(16 * KIB),
            max_ttl: TimeLimit::secs(24 * 60 * 60),
            page: Limit::at(1_000),
        }
    }
}

/// `[registry.limits.clearance]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ClearanceLimits {
    /// Bytes of one clearance grant.
    #[schema(title = "Largest clearance grant", format = "bytes")]
    #[map(default = ClearanceLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Clearance grants.
    #[schema(title = "Clearance grants")]
    #[map(default = ClearanceLimits::default().grants)]
    pub grants: Limit,
    /// Bytes of a grant's signer, approval, model and reason.
    #[schema(title = "Longest signer, approval, model or reason", format = "bytes")]
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

/// `[registry.limits.holds]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct HoldLimits {
    /// Bytes of one hold.
    #[schema(title = "Largest hold", format = "bytes")]
    #[map(default = HoldLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Holds.
    #[schema(title = "Holds")]
    #[map(default = HoldLimits::default().holds)]
    pub holds: Limit,
    /// Bytes of a hold's reason, holder and route names.
    #[schema(title = "Longest reason, holder or route name", format = "bytes")]
    #[map(default = HoldLimits::default().text_bytes)]
    pub text_bytes: Limit,
}

impl Default for HoldLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(16 * KIB),
            holds: Limit::at(10_000),
            text_bytes: Limit::at(2_048),
        }
    }
}

/// `[registry.limits.reports]`: sizes, counts and rates. What a Report may be marked with, and
/// who is cleared for what, are the `roster` record and the Registry's clearances.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct ReportLimits {
    /// Bytes of one Report record.
    #[schema(title = "Largest Report", format = "bytes")]
    #[map(default = ReportLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Reports kept.
    #[schema(title = "Reports kept")]
    #[map(default = ReportLimits::default().reports)]
    pub reports: Limit,
    /// Bytes of a Report's text.
    #[schema(title = "Longest Report text", format = "bytes")]
    #[map(default = ReportLimits::default().text_bytes)]
    pub text_bytes: Limit,
    /// Bytes of a Report's kind and title, and of each name on it: filer, links, compartments.
    #[schema(title = "Longest title or name", format = "bytes")]
    #[map(default = ReportLimits::default().name_bytes)]
    pub name_bytes: Limit,
    /// Links on one Report.
    #[schema(title = "Links on a Report")]
    #[map(default = ReportLimits::default().links)]
    pub links: Limit,
    /// Reports one list returns.
    #[schema(title = "Reports in one list")]
    #[map(default = ReportLimits::default().page)]
    pub page: Limit,
    /// Reports one Agent Session may file in the window.
    #[schema(title = "Reports per Session in the window")]
    #[map(default = ReportLimits::default().per_session)]
    pub per_session: Limit,
    /// Reports Agents together may file against one scope in the window.
    #[schema(title = "Agent Reports per scope in the window")]
    #[map(default = ReportLimits::default().per_operation)]
    pub per_operation: Limit,
    /// Seconds the two counts above look back over.
    #[schema(title = "Rate window")]
    #[map(default = ReportLimits::default().window)]
    pub window: Seconds,
    /// Seconds a closed Report is kept; `null` keeps it for good.
    #[schema(title = "Keep a closed Report for")]
    #[map(default = ReportLimits::default().keep_closed)]
    pub keep_closed: TimeLimit,
}

impl Default for ReportLimits {
    fn default() -> Self {
        Self {
            record_bytes: Limit::at(64 * KIB),
            reports: Limit::at(100_000),
            text_bytes: Limit::at(16 * KIB),
            name_bytes: Limit::at(256),
            links: Limit::at(64),
            page: Limit::at(1_000),
            per_session: Limit::at(20),
            per_operation: Limit::at(200),
            window: Seconds::new(3_600),
            keep_closed: TimeLimit::UNBOUNDED,
        }
    }
}

/// `[registry.limits.memories]`: what one memory scope (a person, an Operation, the global one)
/// may hold.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct MemoryLimits {
    /// Bytes of one entry's content.
    #[schema(title = "Longest entry", format = "bytes")]
    #[map(default = MemoryLimits::default().content_bytes)]
    pub content_bytes: Limit,
    /// Entries in one scope.
    #[schema(title = "Entries in a scope")]
    #[map(default = MemoryLimits::default().entries)]
    pub entries: Limit,
    /// Bytes of every entry's name, summary and content in one scope.
    #[schema(title = "All the entries of a scope", format = "bytes")]
    #[map(default = MemoryLimits::default().scope_bytes)]
    pub scope_bytes: Limit,
    /// Prior versions kept of one entry; forgetting the entry removes them. `0` keeps none.
    #[schema(title = "Prior versions of an entry")]
    #[map(default = MemoryLimits::default().versions)]
    pub versions: Limit,
    /// Bytes of an entry's one-line summary.
    #[schema(title = "Longest summary", format = "bytes")]
    #[map(default = MemoryLimits::default().summary_bytes)]
    pub summary_bytes: Limit,
    /// Bytes of an entry's name, or of who wrote it.
    #[schema(title = "Longest name or author", format = "bytes")]
    #[map(default = MemoryLimits::default().name_bytes)]
    pub name_bytes: Limit,
    /// Compartments on one entry, each at most `name_bytes`.
    #[schema(title = "Compartments on an entry")]
    #[map(default = MemoryLimits::default().compartments)]
    pub compartments: Limit,
    /// Bytes of an entry's distinct words a database's text index holds. A word past it is found
    /// only by a search that matched another word of the entry, and PostgreSQL refuses an index
    /// of 1 MiB or more for one entry.
    #[schema(title = "Words of an entry in the text index", format = "bytes")]
    #[map(default = MemoryLimits::default().index_bytes)]
    pub index_bytes: Limit,
    /// Entries one search answers.
    #[schema(title = "Entries a search answers")]
    #[map(default = MemoryLimits::default().search_results)]
    pub search_results: Limit,
    /// Bytes of what one search asks for.
    #[schema(title = "Longest search", format = "bytes")]
    #[map(default = MemoryLimits::default().query_bytes)]
    pub query_bytes: Limit,
    /// Agents' proposals waiting for a person in one scope.
    #[schema(title = "Proposals waiting in a scope")]
    #[map(default = MemoryLimits::default().proposals)]
    pub proposals: Limit,
}

impl Default for MemoryLimits {
    fn default() -> Self {
        Self {
            content_bytes: Limit::at(256 * KIB),
            entries: Limit::at(1_024),
            scope_bytes: Limit::at(8 * MIB),
            versions: Limit::at(8),
            summary_bytes: Limit::at(KIB),
            name_bytes: Limit::at(256),
            compartments: Limit::at(16),
            index_bytes: Limit::at(512 * KIB),
            search_results: Limit::at(50),
            query_bytes: Limit::at(4 * KIB),
            proposals: Limit::at(64),
        }
    }
}

/// `[registry.limits.files]`: how long the file Registry waits. The SQL Registry ignores it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct FileLimits {
    /// Seconds a record write retries replacing a file another process holds open.
    #[schema(title = "Retry replacing a held file for")]
    #[map(default = FileLimits::default().rename_retry)]
    pub rename_retry: TimeLimit,
    /// Seconds a write waits for the Registry's directory lock.
    #[schema(title = "Wait for the directory lock")]
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
    #[schema(title = "Held briefs per Operation")]
    #[map(default = HeldLimits::default().per_operation)]
    pub per_operation: Limit,
    /// Briefs held across the Registry.
    #[schema(title = "Held briefs in all")]
    #[map(default = HeldLimits::default().records)]
    pub records: Limit,
    /// Held briefs one listing may ask for.
    #[schema(title = "Most held briefs in one listing")]
    #[map(default = HeldLimits::default().results)]
    pub results: Limit,
    /// Bytes of a stored clearance, the finding kinds a brief was released from.
    #[schema(title = "Largest stored clearance", format = "bytes")]
    #[map(default = HeldLimits::default().clearance_bytes)]
    pub clearance_bytes: Limit,
    /// Bytes of the reason a held brief is refused for.
    #[schema(title = "Longest refusal reason", format = "bytes")]
    #[map(default = HeldLimits::default().reason_bytes)]
    pub reason_bytes: Limit,
    /// Bytes of who released or refused a held brief.
    #[schema(title = "Longest name of who decided", format = "bytes")]
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
    #[schema(title = "Reports per logbook")]
    #[map(default = LogbookLimits::default().entries)]
    pub entries: Limit,
    /// Bytes of a report's summary.
    #[schema(title = "Longest report summary", format = "bytes")]
    #[map(default = LogbookLimits::default().summary_bytes)]
    pub summary_bytes: Limit,
    /// Findings per report.
    #[schema(title = "Findings per report")]
    #[map(default = LogbookLimits::default().findings)]
    pub findings: Limit,
    /// Bytes of one finding.
    #[schema(title = "Longest finding", format = "bytes")]
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
    #[schema(title = "Largest Mission proposal", format = "bytes")]
    #[map(default = ProposalLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Pending Mission proposals per Operation.
    #[schema(title = "Pending proposals per Operation")]
    #[map(default = ProposalLimits::default().records)]
    pub records: Limit,
    /// Proposals one listing may ask for.
    #[schema(title = "Most proposals in one listing")]
    #[map(default = ProposalLimits::default().results)]
    pub results: Limit,
    /// Bytes of a proposal's idempotency key.
    #[schema(title = "Longest idempotency key", format = "bytes")]
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
    #[schema(title = "Largest quarantine record", format = "bytes")]
    #[map(default = QuarantineLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Quarantine records.
    #[schema(title = "Quarantine records")]
    #[map(default = QuarantineLimits::default().records)]
    pub records: Limit,
    /// Incidents kept per Agent.
    #[schema(title = "Incidents per Agent")]
    #[map(default = QuarantineLimits::default().incidents)]
    pub incidents: Limit,
    /// Seconds an incident is kept.
    #[schema(title = "Keep an incident for")]
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
    #[schema(title = "Largest Session record", format = "bytes")]
    #[map(default = SessionLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Session records.
    #[schema(title = "Session records")]
    #[map(default = SessionLimits::default().records)]
    pub records: Limit,
    /// Bytes of a Session ID.
    #[schema(title = "Longest Session ID", format = "bytes")]
    #[map(default = SessionLimits::default().id_bytes)]
    pub id_bytes: Limit,
    /// Exposures recorded per Session.
    #[schema(title = "Exposures per Session")]
    #[map(default = SessionLimits::default().exposures)]
    pub exposures: Limit,
    /// Bytes of an exposure's source.
    #[schema(title = "Longest exposure source", format = "bytes")]
    #[map(default = SessionLimits::default().exposure_source_bytes)]
    pub exposure_source_bytes: Limit,
    /// Recent event lines kept per Session.
    #[schema(title = "Recent event lines per Session")]
    #[map(default = SessionLimits::default().recent_events)]
    pub recent_events: Limit,
    /// Characters kept of each recent event line and of a lost Session's reason.
    #[schema(title = "Longest recent event line")]
    #[map(default = SessionLimits::default().recent_event_chars)]
    pub recent_event_chars: Limit,
    /// Sources of exposure one Session's brief records; the rest are not kept.
    #[schema(title = "Sources of exposure per brief")]
    #[map(default = SessionLimits::default().exposure_sources)]
    pub exposure_sources: Limit,
    /// Bytes kept of an Agent's final message for its Mission's outcome; a longer one is cut and
    /// says so.
    #[schema(title = "Longest final report", format = "bytes")]
    #[map(default = SessionLimits::default().final_report_bytes)]
    pub final_report_bytes: Limit,
    /// Bytes of the note a person gives with a verdict on a Mission's outcome.
    #[schema(title = "Longest verdict note", format = "bytes")]
    #[map(default = SessionLimits::default().verdict_note_bytes)]
    pub verdict_note_bytes: Limit,
    /// Bytes of what a redispatch adds to the next attempt's task.
    #[schema(title = "Longest redispatch amendment", format = "bytes")]
    #[map(default = SessionLimits::default().amendment_bytes)]
    pub amendment_bytes: Limit,
    /// Deliverables kept on one attempt.
    #[schema(title = "Deliverables per attempt")]
    #[map(default = SessionLimits::default().deliverables)]
    pub deliverables: Limit,
    /// Probe findings kept on one attempt; the oldest go first.
    #[schema(title = "Probe findings per attempt")]
    #[map(default = SessionLimits::default().evidence_entries)]
    pub evidence_entries: Limit,
    /// Characters kept of a probe finding's detail.
    #[schema(title = "Longest probe finding")]
    #[map(default = SessionLimits::default().evidence_detail_chars)]
    pub evidence_detail_chars: Limit,
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
            exposure_sources: Limit::at(32),
            final_report_bytes: Limit::at(64 * KIB),
            verdict_note_bytes: Limit::at(4 * KIB),
            amendment_bytes: Limit::at(8 * KIB),
            deliverables: Limit::at(32),
            evidence_entries: Limit::at(64),
            evidence_detail_chars: Limit::at(300),
        }
    }
}

/// `[registry.limits.sleepers]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct SleeperLimits {
    /// Seconds a missed window may be late and still fire, once; an older one is skipped and
    /// recorded. `none` keeps no limit. Default: the window's own period, the time since the window
    /// before it.
    #[schema(title = "A missed window still fires within")]
    pub catch_up: Option<TimeLimit>,
    /// Bytes of a Sleeper's prompt.
    #[schema(title = "Longest Sleeper prompt", format = "bytes")]
    #[map(default = SleeperLimits::default().prompt_bytes)]
    pub prompt_bytes: Limit,
    /// Days a schedule is searched back for its latest window.
    #[schema(title = "Days searched back for a window")]
    #[map(default = SleeperLimits::default().lookback_days)]
    pub lookback_days: u32,
    /// Skipped windows the timeline names one by one when a later window comes; any older ones
    /// are one event.
    #[schema(title = "Skipped windows named one by one")]
    #[map(default = SleeperLimits::default().skipped_events)]
    pub skipped_events: Limit,
}

impl Default for SleeperLimits {
    fn default() -> Self {
        Self {
            catch_up: None,
            prompt_bytes: Limit::at(16 * KIB),
            lookback_days: 366,
            skipped_events: Limit::at(64),
        }
    }
}

/// `[registry.limits.issues]`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct IssueLimits {
    /// Linked issues one Mission holds at once; unlinked ones do not count.
    #[schema(title = "Linked issues per Mission")]
    #[map(default = IssueLimits::default().links_per_mission)]
    pub links_per_mission: Limit,
    /// Outbox entries kept; settled ones are dropped oldest first.
    #[schema(title = "Outbox entries kept")]
    #[map(default = IssueLimits::default().outbox_records)]
    pub outbox_records: Limit,
    /// Bytes of one outbox entry's text and payload.
    #[schema(title = "Largest outbox entry", format = "bytes")]
    #[map(default = IssueLimits::default().entry_bytes)]
    pub entry_bytes: Limit,
    /// Replies from a linked issue one Mission stores; delivered ones make room first.
    #[schema(title = "Stored replies per Mission")]
    #[map(default = IssueLimits::default().replies_per_mission)]
    pub replies_per_mission: Limit,
    /// Bytes of one stored reply, cut past it.
    #[schema(title = "Longest stored reply", format = "bytes")]
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
    #[schema(title = "Longest Source item text", format = "bytes")]
    #[map(default = SourceLimits::default().item_text_bytes)]
    pub item_text_bytes: Limit,
    /// Times reporting a finished brief to its Source is tried before it is dropped.
    #[schema(title = "Tries to report a finished brief")]
    #[map(default = SourceLimits::default().report_attempts)]
    pub report_attempts: Limit,
    /// Bytes of the name a brief shows beside its Mission ID.
    #[schema(title = "Longest name beside a Mission ID", format = "bytes")]
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
    #[schema(title = "Largest Station record", format = "bytes")]
    #[map(default = StationLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Station records.
    #[schema(title = "Station records")]
    #[map(default = StationLimits::default().records)]
    pub records: Limit,
    /// Bytes of a Station's settings record.
    #[schema(title = "Largest Station settings record", format = "bytes")]
    #[map(default = StationLimits::default().settings_bytes)]
    pub settings_bytes: Limit,
    /// Missions a Station's settings may let it run at once.
    #[schema(title = "Most Missions a Station may run at once")]
    #[map(default = StationLimits::default().mission_capacity)]
    pub mission_capacity: Limit,
    /// Seconds after its last check-in that a Station counts as dark.
    #[schema(title = "A Station goes dark after")]
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
    #[schema(title = "Largest event", format = "bytes")]
    #[map(default = TimelineLimits::default().record_bytes)]
    pub record_bytes: Limit,
    /// Bytes of an event's ID.
    #[schema(title = "Longest event ID", format = "bytes")]
    #[map(default = TimelineLimits::default().id_bytes)]
    pub id_bytes: Limit,
    /// Bytes of an event's kind, action and detail keys.
    #[schema(title = "Longest kind, action or detail key", format = "bytes")]
    #[map(default = TimelineLimits::default().name_bytes)]
    pub name_bytes: Limit,
    /// Bytes of an event's Operation, Station, Mission, Session, actor and detail values.
    #[schema(title = "Longest event text", format = "bytes")]
    #[map(default = TimelineLimits::default().text_bytes)]
    pub text_bytes: Limit,
    /// Detail entries per event.
    #[schema(title = "Details per event")]
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
    #[schema(title = "Largest access token record", format = "bytes")]
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
    #[schema(title = "Largest usage ledger", format = "bytes")]
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
    fn catch_up_reads_a_bound_or_none_and_none_is_named() {
        let unset: RegistryLimits = crate::directorates::m::records::from_toml("").unwrap();
        assert_eq!(unset.sleepers.catch_up, None);
        assert!(unset.weakenings().is_empty());
        let bound: RegistryLimits =
            crate::directorates::m::records::from_toml("[sleepers]\ncatch_up = 3600\n").unwrap();
        assert_eq!(bound.sleepers.catch_up, Some(TimeLimit::secs(3600)));
        assert!(bound.weakenings().is_empty(), "{:?}", bound.weakenings());
        let none: RegistryLimits =
            crate::directorates::m::records::from_toml("[sleepers]\ncatch_up = \"none\"\n")
                .unwrap();
        assert_eq!(none.sleepers.catch_up, Some(TimeLimit::UNBOUNDED));
        assert_eq!(
            none.weakenings(),
            [
                "registry.limits.sleepers.catch_up none is looser than the default, a window's own \
              period"
            ]
        );
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
