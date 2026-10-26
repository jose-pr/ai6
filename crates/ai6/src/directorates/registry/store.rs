//! The file-backed Registry store: the brief queue, leases, Station check-ins, the
//! Headquarters lease, audit records and access tokens.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::directorates::m::identity::{MissionId, SessionId};
use crate::directorates::m::placement::ResourceLimits;
use crate::directorates::m::roster::AgentRole;
use crate::directorates::registry::cables::{Cable, CableError, CableState};
use crate::directorates::registry::timeline::{TimelineEvent, record_event_locked};

const BRIEF_VERSION: u32 = 1;
pub(super) const MAX_BRIEF_BYTES: u64 = 1_048_576;
pub(super) const MAX_BRIEFS: usize = 1_000;
pub(super) const MAX_STATION_RECORD_BYTES: u64 = 1_048_576;
pub(super) const MAX_STATIONS: usize = 1_000;
pub(super) const MAX_STATION_SETTINGS_BYTES: u64 = 4_096;
/// The largest live Mission capacity accepted for one Station.
pub const MAX_STATION_MISSION_CAPACITY: usize = 1_024;
pub(super) const MAX_CABLE_RECORD_BYTES: u64 = 2_097_152;
pub(super) const MAX_CABLE_FILES: usize = 10_000;
pub(super) const MAX_ACCESS_TOKEN_RECORD_BYTES: u64 = 16_384;
pub(super) const MAX_AUDIT_RECORD_BYTES: u64 = 16_384;
pub(super) const MAX_AUDIT_ENTRIES: usize = 10_000;
pub(super) const MAX_AUDIT_RESULTS: usize = 500;
pub(super) const MAX_AGENT_ROLE_RECORD_BYTES: u64 = 32_768;
pub(super) const MAX_AGENT_ROLES: usize = 256;
pub(super) const MAX_MISSION_PROPOSAL_BYTES: u64 = 1_048_576;
pub(super) const MAX_MISSION_PROPOSALS: usize = 1_000;
pub(super) const MAX_MISSION_PROPOSAL_RESULTS: usize = 500;
const MAX_PROPOSAL_IDEMPOTENCY_KEY_BYTES: usize = 256;
/// Default duration for a file-backed Registry claim.
pub const DEFAULT_LEASE: Duration = Duration::from_secs(60);
/// A Station whose last check-in is older than this has gone dark: three missed check-ins.
pub const GONE_DARK_AFTER: Duration = Duration::from_secs(45);

/// One queued brief with a bounded, versioned file representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Brief {
    /// Human-readable task title.
    pub title: String,
    /// Additional task instructions.
    #[serde(default)]
    pub body: String,
    /// Source-local brief identifier.
    #[serde(default = "default_id")]
    pub id: String,
    /// Intake source name.
    #[serde(default = "default_source")]
    pub source: String,
    /// Operation id, name or alias.
    pub operation: Option<String>,
    /// Explicit Agent identity, when assigned.
    pub agent: Option<String>,
    /// Requested Agent role, when not explicitly assigned.
    pub role: Option<String>,
    /// Requested Agent capabilities.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Stations the Mission is limited to. It can only narrow the Operation's `stations`
    /// list, never widen it; empty adds no limit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stations: Vec<String>,
    /// Higher values sort first.
    #[serde(default)]
    pub priority: i64,
    /// Optional source URL.
    pub url: Option<String>,
    /// Source-specific metadata.
    #[serde(default)]
    pub data: BTreeMap<String, serde_json::Value>,
    /// ISO 8601 creation time used for stable intake ordering.
    #[serde(default)]
    pub created: String,
    /// Brief contract version.
    #[serde(default = "default_version")]
    pub version: u32,
}

impl Brief {
    /// Filesystem-safe key within the Registry.
    pub fn key(&self) -> String {
        format!(
            "{}--{}",
            safe_component(&self.source),
            safe_component(&self.id)
        )
    }

    /// The complete task instruction for an Agent.
    pub fn prompt(&self) -> String {
        let mut parts = Vec::with_capacity(3);
        let title = self.title.trim();
        let body = self.body.trim();
        if !title.is_empty() {
            parts.push(title.to_owned());
        }
        if !body.is_empty() {
            parts.push(body.to_owned());
        }
        if let Some(url) = self
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
        {
            parts.push(format!("Source: {url}"));
        }
        parts.join("\n\n")
    }

    pub(super) fn validate(&self) -> io::Result<()> {
        if self.title.trim().is_empty() {
            return Err(invalid_data("a brief needs a title"));
        }
        if self.version > BRIEF_VERSION {
            return Err(invalid_data(format!(
                "brief version {} is newer than this reader ({BRIEF_VERSION})",
                self.version
            )));
        }
        if self.id.is_empty() || self.source.is_empty() {
            return Err(invalid_data("brief id and source must be non-empty"));
        }
        Ok(())
    }
}

/// An Agent-authored Mission brief waiting for M's authorization.
///
/// Proposals are held outside `new/` and are never visible to the scheduler until an
/// M-controlled path promotes them. The proposal identity is derived from its parent
/// Mission, provider Session and idempotency key.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionProposal {
    /// Stable proposal identity derived from the authenticated Session and retry key.
    pub id: String,
    /// Operation that owns the parent Mission and proposed work.
    pub operation_id: String,
    /// Parent Mission whose coordinator authored this proposal.
    pub parent_mission_id: MissionId,
    /// Authenticated Agent identity supplied by AI6, not by the proposal body.
    pub proposer_agent_id: String,
    /// Provider-issued Session bound to the authenticated MCP endpoint.
    pub proposer_session_id: SessionId,
    /// Proposed child Mission brief.
    pub brief: Brief,
    /// Unix timestamp when the proposal was first stored.
    pub created_at: f64,
}

impl MissionProposal {
    /// Builds a Mission proposal tied to the caller's parent Mission and provider Session.
    ///
    /// `idempotency_key` is scoped to the parent Mission and Session. The proposed brief's
    /// Operation is set from `operation_id`; its source and ID are reserved by AI6.
    ///
    /// # Errors
    ///
    /// Returns `InvalidInput` when the identity, retry key or brief is invalid.
    pub fn try_new(
        operation_id: impl Into<String>,
        parent_mission_id: MissionId,
        proposer_agent_id: impl Into<String>,
        proposer_session_id: SessionId,
        idempotency_key: &str,
        mut brief: Brief,
    ) -> io::Result<Self> {
        let operation_id = operation_id.into();
        let proposer_agent_id = proposer_agent_id.into();
        if idempotency_key.is_empty()
            || idempotency_key.len() > MAX_PROPOSAL_IDEMPOTENCY_KEY_BYTES
            || idempotency_key.chars().any(char::is_control)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "proposal idempotency key is empty, too long, or contains controls",
            ));
        }
        validate_registry_key(&operation_id)?;
        validate_registry_key(&proposer_agent_id)?;
        brief.validate()?;
        if brief.agent.is_some() && brief.role.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a proposed brief cannot select both an Agent and a role",
            ));
        }
        let mut digest = Sha256::new();
        for component in [
            operation_id.as_bytes(),
            parent_mission_id.as_str().as_bytes(),
            proposer_session_id.as_str().as_bytes(),
            idempotency_key.as_bytes(),
        ] {
            digest.update((component.len() as u64).to_be_bytes());
            digest.update(component);
        }
        let id = format!("proposal-{}", hex_digest(&digest.finalize()));
        brief.operation = Some(operation_id.clone());
        brief.source = "ai6-agent-proposal".to_owned();
        brief.id = id.clone();
        Ok(Self {
            id,
            operation_id,
            parent_mission_id,
            proposer_agent_id,
            proposer_session_id,
            brief,
            created_at: 0.0,
        })
    }

    pub(super) fn validate(&self) -> io::Result<()> {
        validate_registry_key(&self.id)?;
        validate_registry_key(&self.operation_id)?;
        validate_registry_key(&self.proposer_agent_id)?;
        self.brief.validate()?;
        if self.brief.operation.as_deref() != Some(self.operation_id.as_str())
            || self.brief.source != "ai6-agent-proposal"
            || self.brief.id != self.id
            || !self.created_at.is_finite()
            || self.created_at < 0.0
        {
            return Err(invalid_data("Mission proposal record is invalid"));
        }
        Ok(())
    }
}

/// Metadata-only audit event stored in the shared file-backed Registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditRecord {
    /// Unique file-safe event identity.
    pub id: String,
    /// Authenticated actor identity supplied by the gateway's Directory.
    pub actor: String,
    /// Bounded action name such as `mission.create`.
    pub action: String,
    /// Operation that authorized the action.
    pub operation: String,
    /// Explicit non-Operation resource for Station-scoped (`station:<id>`) or roster
    /// (`config:roster`) administrative changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// Safe identifier for the affected resource; never a prompt or payload.
    pub target: String,
    /// Unix timestamp in seconds.
    pub at: f64,
}

impl AuditRecord {
    pub(super) fn validate(&self) -> io::Result<()> {
        if self.id.len() > 128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "audit ID exceeds 128 bytes",
            ));
        }
        validate_registry_key(&self.id)?;
        validate_audit_text(&self.actor, 256, "actor")?;
        validate_audit_action(&self.action)?;
        if self.operation.is_empty() {
            if self.resource.as_deref().is_none_or(str::is_empty) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "audit entry needs an Operation or resource scope",
                ));
            }
        } else {
            validate_audit_text(&self.operation, 256, "operation")?;
        }
        if let Some(resource) = &self.resource {
            validate_audit_text(resource, 512, "resource")?;
            if !resource.starts_with("station:") && !resource.starts_with("config:") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "audit resource must use a supported scope",
                ));
            }
        }
        validate_audit_text(&self.target, 512, "target")?;
        if !self.at.is_finite() || self.at <= 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "audit timestamp must be finite and positive",
            ));
        }
        Ok(())
    }
}

/// A station's renewable hold on one claimed brief.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Lease {
    pub(super) key: String,
    pub(super) station: String,
    pub(super) expires: f64,
    pub(super) attempt: u32,
    pub(super) meta: BTreeMap<String, serde_json::Value>,
}

impl PartialEq for Lease {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.station == other.station
            && self.expires == other.expires
            && self.attempt == other.attempt
    }
}

impl Lease {
    /// Returns the filesystem-safe brief key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Returns the station that owns this lease.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// Returns the wall-clock expiry as Unix seconds.
    pub fn expires(&self) -> f64 {
        self.expires
    }

    /// Returns the number of leased attempts for this brief.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Returns the assignment metadata shared with other Registry users.
    pub fn meta(&self) -> &BTreeMap<String, serde_json::Value> {
        &self.meta
    }
}

/// A station's latest shared Registry check-in, with its scheduling attributes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct StationRecord {
    #[serde(flatten)]
    pub(super) info: BTreeMap<String, serde_json::Value>,
    pub(super) station: String,
    pub(super) seen: f64,
}

/// A validated setting that can be changed for one Station while it is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct StationSettings {
    pub(super) station: String,
    pub(super) max_concurrent_missions: usize,
    #[serde(flatten)]
    pub(super) resource_limits: ResourceLimits,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StationSettingsFile {
    station: String,
    max_concurrent_missions: usize,
    /// Absent from settings written before resource limits existed.
    #[serde(default)]
    min_free_memory_mb: Option<u64>,
    #[serde(default)]
    max_cpu_percent: Option<u32>,
}

impl StationSettings {
    /// Creates settings with a Mission capacity from 1 through 1,024 and the default
    /// [`ResourceLimits`].
    pub fn try_new(station: impl Into<String>, max_concurrent_missions: usize) -> io::Result<Self> {
        let station = station.into();
        validate_station_id(&station)?;
        if !(1..=MAX_STATION_MISSION_CAPACITY).contains(&max_concurrent_missions) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Station Mission capacity must be between 1 and 1024",
            ));
        }
        Ok(Self {
            station,
            max_concurrent_missions,
            resource_limits: ResourceLimits::default(),
        })
    }

    /// Replaces when the Station and its Theaters are too short of resources for new
    /// Missions. `max_cpu_percent` above 100 is refused: no CPU is busier than that.
    pub fn with_resource_limits(mut self, limits: ResourceLimits) -> io::Result<Self> {
        if limits.max_cpu_percent > 100 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Station max_cpu_percent must be between 0 and 100",
            ));
        }
        self.resource_limits = limits;
        Ok(self)
    }

    /// Returns when the Station and its Theaters take no new Missions.
    pub fn resource_limits(&self) -> ResourceLimits {
        self.resource_limits
    }

    /// Returns the Station identity this setting belongs to.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// Returns the Station's maximum number of simultaneous provider Sessions.
    pub fn max_concurrent_missions(&self) -> usize {
        self.max_concurrent_missions
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct StationRecordFile {
    #[serde(flatten)]
    info: BTreeMap<String, serde_json::Value>,
    station: String,
    seen: f64,
}

impl StationRecordFile {
    pub(super) fn into_station_record(self) -> io::Result<StationRecord> {
        validate_station_id(&self.station)?;
        if !self.seen.is_finite() {
            return Err(invalid_data("invalid station check-in time"));
        }
        Ok(StationRecord {
            info: self.info,
            station: self.station,
            seen: self.seen,
        })
    }
}

impl StationRecord {
    /// Returns the stable station identity.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// Returns the wall-clock check-in time as Unix seconds.
    pub fn seen(&self) -> f64 {
        self.seen
    }

    /// Returns the station metadata other than its identity and check-in time.
    pub fn info(&self) -> &BTreeMap<String, serde_json::Value> {
        &self.info
    }
}

/// The current Headquarters lease, whether live or expired.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct HeadquartersLease {
    pub(super) station: String,
    pub(super) expires: f64,
    pub(super) term: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HeadquartersLeaseFile {
    station: String,
    expires: f64,
    #[serde(default)]
    term: u64,
}

impl HeadquartersLeaseFile {
    fn into_lease(self) -> io::Result<HeadquartersLease> {
        validate_station_id(&self.station)?;
        if !self.expires.is_finite() {
            return Err(invalid_data("invalid Headquarters lease expiry"));
        }
        Ok(HeadquartersLease {
            station: self.station,
            expires: self.expires,
            term: self.term,
        })
    }
}

impl HeadquartersLease {
    /// Returns the election term: it grows by one each time a different Station takes the
    /// lease and never repeats, so it orders Headquarters holders. A lease written before
    /// terms existed reads as term 0.
    pub fn term(&self) -> u64 {
        self.term
    }

    /// Returns the station that holds or last held the Headquarters lease.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// Returns the lease expiry as Unix seconds.
    pub fn expires(&self) -> f64 {
        self.expires
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LeaseRecord {
    station: String,
    expires: f64,
    attempt: u32,
    #[serde(default)]
    meta: BTreeMap<String, serde_json::Value>,
}

impl LeaseRecord {
    fn from_lease(lease: &Lease) -> Self {
        Self {
            station: lease.station.clone(),
            expires: lease.expires,
            attempt: lease.attempt,
            meta: lease.meta.clone(),
        }
    }

    fn into_lease(self, key: String) -> io::Result<Lease> {
        if self.station.trim().is_empty() || !self.expires.is_finite() {
            return Err(invalid_data("invalid Registry lease"));
        }
        Ok(Lease {
            key,
            station: self.station,
            expires: self.expires,
            attempt: self.attempt,
            meta: self.meta,
        })
    }
}

/// Result of recovering a Registry lease after its owner stopped renewing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LeaseRecovery {
    /// The brief was returned to `new/` for another attempt.
    Requeued,
    /// The attempt limit was reached and the brief was filed in `failed/`.
    Failed,
}

/// The Registry kept in one directory every Station shares: a
/// [`RegistryBackend`](super::backend::RegistryBackend).
///
/// Records are JSON files written atomically under a directory lock: briefs move through
/// `new/`, `claimed/`, `done/` and `failed/`, with leases in `leases/`. Stations sharing it
/// need synchronized clocks, since leases expire on wall-clock time.
#[derive(Debug, Clone)]
pub struct FileRegistry {
    pub(super) root: PathBuf,
}

impl FileRegistry {
    /// Opens a Registry directory without creating or changing it.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolves `AI6_REGISTRY`, `AI6_HOME/registry`, or the platform state directory.
    ///
    /// # Errors
    ///
    /// `Unsupported` when `AI6_REGISTRY` names a SQL Registry, which
    /// [`backend::from_environment`](super::backend::from_environment) opens.
    pub fn from_environment() -> io::Result<Self> {
        let root = default_registry_dir()?;
        let location = root.to_string_lossy();
        if location.contains("://") || location.starts_with("sqlite:") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "AI6_REGISTRY names a SQL Registry, not a directory",
            ));
        }
        Ok(Self::open(root))
    }

    /// Returns the configured Registry root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn station_settings_path(&self, station: &str) -> PathBuf {
        let digest = hex_digest(&Sha256::digest(station.as_bytes()));
        self.root
            .join("station-settings")
            .join(format!("{digest}.json"))
    }

    fn scan_cables(&self) -> io::Result<Vec<Cable>> {
        let directory = self.root.join("cables");
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
            {
                if paths.len() == MAX_CABLE_FILES {
                    return Err(invalid_data(format!(
                        "Registry has more than {MAX_CABLE_FILES} Cable records"
                    )));
                }
                paths.push(path);
            }
        }
        paths.sort();
        let mut cables = Vec::with_capacity(paths.len());
        for path in paths {
            match read_cable(&path) {
                Ok(Some(cable)) => cables.push(cable),
                Ok(None) => {}
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {}
                Err(error) => return Err(error),
            }
        }
        Ok(cables)
    }
}

/// The brief queue: Mission proposals, queued briefs, their leases and outcomes.
pub trait BriefStore {
    /// Stores an Agent-authored Mission proposal outside the scheduler's queue.
    ///
    /// Repeating an identical proposal is idempotent. Reusing its identity for different
    /// content is rejected, and only an M-controlled authorization path may promote it.
    fn submit_mission_proposal(&self, proposal: &MissionProposal) -> io::Result<()>;

    /// Lists pending Mission proposals for one Operation in creation order.
    fn list_mission_proposals(
        &self,
        operation: &str,
        limit: usize,
    ) -> io::Result<Vec<MissionProposal>>;

    /// Authorizes one pending Mission proposal and promotes its brief to the runnable queue. Repeating an approval after a partial failure or successful promotion is safe.
    fn promote_mission_proposal(&self, operation: &str, proposal_id: &str) -> io::Result<Brief>;

    /// Rejects one pending Mission proposal without exposing it to scheduler intake.
    /// Repeating the rejection is safe; an approved proposal cannot be rejected.
    fn reject_mission_proposal(
        &self,
        operation: &str,
        proposal_id: &str,
    ) -> io::Result<MissionProposal>;

    /// Reads valid queued briefs in priority, creation-time and key order.
    /// Invalid records are skipped; nothing is claimed or moved.
    fn list_new(&self) -> io::Result<Vec<Brief>>;

    /// Submits one validated brief to the shared queue.
    ///
    /// A key already present in any Registry state is never overwritten.
    fn submit_brief(&self, brief: &Brief) -> io::Result<()>;

    /// Atomically claims one queued brief, or the highest-priority queued brief. Concurrent
    /// Stations never claim the same brief.
    fn claim_new(&self, key: Option<&str>) -> io::Result<Option<Brief>>;

    /// Claims the next matching brief and records a renewable lease.
    ///
    /// `key` selects one queued brief or uses Registry order when omitted. `station` identifies
    /// the lease owner; `duration` must be positive. `meta` records assignment details for
    /// other Registry users.
    ///
    /// Returns `None` when no matching brief remains queued.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid lease settings, Registry I/O failures or a corrupted
    /// attempt count.
    fn claim_with_lease(
        &self,
        key: Option<&str>,
        station: &str,
        duration: Duration,
        meta: BTreeMap<String, serde_json::Value>,
    ) -> io::Result<Option<Lease>>;

    /// Reads a brief's lease, if the Registry has one.
    ///
    /// `key` must contain only the filesystem-safe characters used by [`Brief::key`].
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid key, unreadable lease record or Registry I/O failure.
    fn lease(&self, key: &str) -> io::Result<Option<Lease>>;

    /// Reads a currently claimed brief without changing its lease.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid key, unreadable file or invalid brief record.
    fn read_claimed(&self, key: &str) -> io::Result<Option<Brief>>;

    /// Returns leases for currently claimed briefs, ordered by brief key.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry directory or a lease record cannot be read.
    fn leases(&self) -> io::Result<Vec<Lease>>;

    /// Extends a station's lease, returning `false` when it no longer owns a claimed brief.
    ///
    /// `duration` must be positive. Lease expiration uses wall-clock Unix time, so stations
    /// sharing a Registry need synchronized clocks.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid key or lease duration, or Registry I/O failure.
    fn renew(&self, key: &str, station: &str, duration: Duration) -> io::Result<bool>;

    /// Returns expired leases in stable key order.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry directory or a lease record cannot be read.
    fn expired(&self) -> io::Result<Vec<Lease>>;

    /// Recovers an unchanged expired lease, requeueing it or filing a lost outcome.
    ///
    /// `max_attempts` is the maximum number of claims before the brief is failed. `by` is
    /// recorded as the recovering station. A changed, renewed or unexpired lease returns `None`.
    ///
    /// # Errors
    ///
    /// Returns an error when Registry records cannot be read or written.
    fn recover(
        &self,
        lease: &Lease,
        max_attempts: u32,
        by: &str,
    ) -> io::Result<Option<LeaseRecovery>>;

    /// Gives an unchanged expired lease to another Station, which carries on the same claim:
    /// the brief stays claimed and the attempt count is unchanged. `meta` entries are added
    /// to the lease's metadata.
    ///
    /// Returns the new lease, or `None` for a changed, renewed or unexpired lease.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid lease settings or Registry I/O failures.
    fn hand_over(
        &self,
        lease: &Lease,
        station: &str,
        duration: Duration,
        meta: BTreeMap<String, serde_json::Value>,
    ) -> io::Result<Option<Lease>>;

    /// Files an outcome for a brief, refusing to finish a claim held by another station.
    ///
    /// The outcome must be a JSON object. When `station` is provided, a missing or differently
    /// owned lease returns `false`; otherwise the brief and outcome are filed as done or
    /// failed, with an optional transcript, and `true` is returned.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-object outcome or Registry I/O and serialization failures.
    fn finish(
        &self,
        brief: &Brief,
        outcome: &serde_json::Value,
        ok: bool,
        transcript: Option<&[u8]>,
        station: Option<&str>,
    ) -> io::Result<bool>;
}

impl BriefStore for FileRegistry {
    fn submit_mission_proposal(&self, proposal: &MissionProposal) -> io::Result<()> {
        proposal.validate()?;
        let _registry_lock = registry_lock(&self.root)?;
        let directory = self
            .root
            .join("proposals")
            .join("pending")
            .join(&proposal.operation_id);
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{}.json", proposal.id));
        let mut stored = proposal.clone();
        if path.exists() {
            let existing = read_mission_proposal(&path)?;
            stored.created_at = existing.created_at;
            if existing == stored {
                return Ok(());
            }
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Mission proposal identity already has different content",
            ));
        }
        let mut count = 0;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            {
                count += 1;
            }
        }
        if count >= MAX_MISSION_PROPOSALS {
            return Err(invalid_data(format!(
                "Operation has reached the {MAX_MISSION_PROPOSALS} pending Mission proposal limit"
            )));
        }
        stored.created_at = unix_time()?;
        let bytes = serde_json::to_vec_pretty(&stored).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_MISSION_PROPOSAL_BYTES {
            return Err(invalid_data("Mission proposal exceeds the size limit"));
        }
        write_atomic(&path, &bytes)?;
        Ok(())
    }

    fn list_mission_proposals(
        &self,
        operation: &str,
        limit: usize,
    ) -> io::Result<Vec<MissionProposal>> {
        validate_registry_key(operation)?;
        if limit == 0 || limit > MAX_MISSION_PROPOSAL_RESULTS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Mission proposal result limit must be between 1 and {MAX_MISSION_PROPOSAL_RESULTS}"
                ),
            ));
        }
        let directory = self.root.join("proposals").join("pending").join(operation);
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
            {
                if paths.len() >= MAX_MISSION_PROPOSALS {
                    return Err(invalid_data(format!(
                        "Operation has more than {MAX_MISSION_PROPOSALS} Mission proposals"
                    )));
                }
                paths.push(path);
            }
        }
        paths.sort();
        let mut proposals = Vec::new();
        for path in paths {
            match read_mission_proposal(&path) {
                Ok(proposal)
                    if proposal.operation_id == operation
                        && path.file_stem().and_then(|stem| stem.to_str())
                            == Some(proposal.id.as_str()) =>
                {
                    proposals.push(proposal)
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {}
                Err(error) => return Err(error),
            }
        }
        proposals.sort_by(|left, right| {
            left.created_at
                .total_cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        proposals.truncate(limit);
        Ok(proposals)
    }

    fn promote_mission_proposal(&self, operation: &str, proposal_id: &str) -> io::Result<Brief> {
        validate_registry_key(operation)?;
        validate_registry_key(proposal_id)?;
        let _registry_lock = registry_lock(&self.root)?;
        let pending = self
            .root
            .join("proposals")
            .join("pending")
            .join(operation)
            .join(format!("{proposal_id}.json"));
        let approved = self
            .root
            .join("proposals")
            .join("approved")
            .join(operation)
            .join(format!("{proposal_id}.json"));
        if !pending.is_file() {
            return match read_mission_proposal(&approved) {
                Ok(proposal) => Ok(proposal.brief),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let rejected = self
                        .root
                        .join("proposals")
                        .join("rejected")
                        .join(operation)
                        .join(format!("{proposal_id}.json"));
                    if rejected.is_file() {
                        Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "Mission proposal has already been rejected",
                        ))
                    } else {
                        Err(error)
                    }
                }
                Err(error) => Err(error),
            };
        }
        let proposal = read_mission_proposal(&pending)?;
        if proposal.operation_id != operation || proposal.id != proposal_id {
            return Err(invalid_data(
                "Mission proposal path does not match its identity",
            ));
        }
        let brief = proposal.brief.clone();
        let key = brief.key();
        for state in ["claimed", "done", "failed"] {
            if self.root.join(state).join(format!("{key}.json")).exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Mission proposal brief already exists in a terminal intake state",
                ));
            }
        }
        let queued = self.root.join("new").join(format!("{key}.json"));
        if queued.is_file() {
            let bytes = fs::read(&queued)?;
            let existing: Brief =
                serde_json::from_slice(&bytes).map_err(|error| invalid_data(error.to_string()))?;
            if existing != brief {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Mission proposal brief conflicts with queued intake",
                ));
            }
        } else {
            let bytes = serde_json::to_vec_pretty(&brief).map_err(io::Error::other)?;
            if bytes.len() as u64 > MAX_BRIEF_BYTES {
                return Err(invalid_data(
                    "Mission proposal brief exceeds the size limit",
                ));
            }
            write_atomic(&queued, &bytes)?;
        }
        let approved_parent = approved
            .parent()
            .ok_or_else(|| invalid_data("Mission proposal path has no parent"))?;
        fs::create_dir_all(approved_parent)?;
        fs::rename(&pending, &approved)?;
        Ok(brief)
    }

    fn reject_mission_proposal(
        &self,
        operation: &str,
        proposal_id: &str,
    ) -> io::Result<MissionProposal> {
        validate_registry_key(operation)?;
        validate_registry_key(proposal_id)?;
        let _registry_lock = registry_lock(&self.root)?;
        let pending = self
            .root
            .join("proposals")
            .join("pending")
            .join(operation)
            .join(format!("{proposal_id}.json"));
        let rejected = self
            .root
            .join("proposals")
            .join("rejected")
            .join(operation)
            .join(format!("{proposal_id}.json"));
        if !pending.is_file() {
            if let Ok(proposal) = read_mission_proposal(&rejected) {
                return Ok(proposal);
            }
            let approved = self
                .root
                .join("proposals")
                .join("approved")
                .join(operation)
                .join(format!("{proposal_id}.json"));
            if approved.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Mission proposal has already been approved",
                ));
            }
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Mission proposal does not exist",
            ));
        }
        let proposal = read_mission_proposal(&pending)?;
        if proposal.operation_id != operation || proposal.id != proposal_id {
            return Err(invalid_data(
                "Mission proposal path does not match its identity",
            ));
        }
        let rejected_parent = rejected
            .parent()
            .ok_or_else(|| invalid_data("Mission proposal path has no parent"))?;
        fs::create_dir_all(rejected_parent)?;
        fs::rename(&pending, &rejected)?;
        Ok(proposal)
    }

    fn list_new(&self) -> io::Result<Vec<Brief>> {
        let directory = self.root.join("new");
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            if paths.len() >= MAX_BRIEFS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Registry has more than {MAX_BRIEFS} queued briefs"),
                ));
            }
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && entry.file_type()?.is_file()
            {
                paths.push(path);
            }
        }
        paths.sort();

        let mut briefs = Vec::new();
        for path in paths {
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.len() > MAX_BRIEF_BYTES {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let Ok(brief) = serde_json::from_slice::<Brief>(&bytes) else {
                continue;
            };
            if brief.validate().is_ok() {
                briefs.push(brief);
            }
        }
        briefs.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.created.cmp(&right.created))
                .then_with(|| left.key().cmp(&right.key()))
        });
        Ok(briefs)
    }

    fn submit_brief(&self, brief: &Brief) -> io::Result<()> {
        brief.validate()?;
        let key = brief.key();
        let _registry_lock = registry_lock(&self.root)?;
        for state in ["new", "claimed", "done", "failed"] {
            if self.root.join(state).join(format!("{key}.json")).exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("brief {key} already exists in {state}"),
                ));
            }
        }
        let bytes = serde_json::to_vec_pretty(brief).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_BRIEF_BYTES {
            return Err(invalid_data("brief exceeds the size limit"));
        }
        let path = self.root.join("new").join(format!("{key}.json"));
        write_atomic(&path, &bytes)?;
        Ok(())
    }

    fn claim_new(&self, key: Option<&str>) -> io::Result<Option<Brief>> {
        let _registry_lock = registry_lock(&self.root)?;
        let destination_dir = self.root.join("claimed");
        fs::create_dir_all(&destination_dir)?;
        for brief in self.list_new()? {
            if key.is_some_and(|key| brief.key() != key) {
                continue;
            }
            let source = self.root.join("new").join(format!("{}.json", brief.key()));
            let destination = destination_dir.join(format!("{}.json", brief.key()));
            if destination.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("brief {} already exists in claimed/", brief.key()),
                ));
            }
            match fs::rename(source, destination) {
                Ok(()) => return Ok(Some(brief)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    fn claim_with_lease(
        &self,
        key: Option<&str>,
        station: &str,
        duration: Duration,
        meta: BTreeMap<String, serde_json::Value>,
    ) -> io::Result<Option<Lease>> {
        if station.trim().is_empty() || duration.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a Registry lease needs a station and positive duration",
            ));
        }
        let _registry_lock = registry_lock(&self.root)?;
        let destination_dir = self.root.join("claimed");
        fs::create_dir_all(&destination_dir)?;
        for brief in self.list_new()? {
            if key.is_some_and(|key| brief.key() != key) {
                continue;
            }
            let key = brief.key();
            let source = self.root.join("new").join(format!("{key}.json"));
            let destination = destination_dir.join(format!("{key}.json"));
            let attempts_path = self.root.join("attempts").join(format!("{key}.json"));
            let previous_attempt = match fs::read_to_string(&attempts_path) {
                Ok(value) => value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| invalid_data("invalid Registry attempt count"))?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
                Err(error) => return Err(error),
            };
            let attempt = previous_attempt
                .checked_add(1)
                .ok_or_else(|| invalid_data("Registry attempt count is exhausted"))?;
            let expires = unix_time()? + duration.as_secs_f64();
            if destination.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("brief {key} already exists in claimed/"),
                ));
            }
            match fs::rename(source, destination) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }

            let lease = Lease {
                key: key.clone(),
                station: station.to_owned(),
                expires,
                attempt,
                meta,
            };
            let record = LeaseRecord::from_lease(&lease);
            let attempts_result = write_atomic(&attempts_path, attempt.to_string().as_bytes());
            let lease_result = serde_json::to_vec(&record)
                .map_err(io::Error::other)
                .and_then(|bytes| {
                    write_atomic(
                        &self.root.join("leases").join(format!("{key}.json")),
                        &bytes,
                    )
                });
            if let Err(error) = attempts_result.and(lease_result) {
                let _ = fs::rename(
                    self.root.join("claimed").join(format!("{key}.json")),
                    self.root.join("new").join(format!("{key}.json")),
                );
                return Err(error);
            }
            return Ok(Some(lease));
        }
        Ok(None)
    }

    fn lease(&self, key: &str) -> io::Result<Option<Lease>> {
        validate_registry_key(key)?;
        let path = self.root.join("leases").join(format!("{key}.json"));
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let record = serde_json::from_slice::<LeaseRecord>(&bytes).map_err(io::Error::other)?;
        record.into_lease(key.to_owned()).map(Some)
    }

    fn read_claimed(&self, key: &str) -> io::Result<Option<Brief>> {
        validate_registry_key(key)?;
        let path = self.root.join("claimed").join(format!("{key}.json"));
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if bytes.len() as u64 > MAX_BRIEF_BYTES {
            return Err(invalid_data("claimed brief exceeds the size limit"));
        }
        let brief = serde_json::from_slice::<Brief>(&bytes).map_err(io::Error::other)?;
        brief.validate()?;
        Ok(Some(brief))
    }

    fn leases(&self) -> io::Result<Vec<Lease>> {
        let directory = self.root.join("claimed");
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut leases = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || entry
                    .path()
                    .extension()
                    .is_none_or(|extension| extension != "json")
            {
                continue;
            }
            if let Some(lease) = self.lease(
                &entry
                    .path()
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy(),
            )? {
                leases.push(lease);
            }
        }
        leases.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(leases)
    }

    fn renew(&self, key: &str, station: &str, duration: Duration) -> io::Result<bool> {
        validate_registry_key(key)?;
        if station.trim().is_empty() || duration.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a Registry lease needs a station and positive duration",
            ));
        }
        let _registry_lock = registry_lock(&self.root)?;
        let Some(mut lease) = self.lease(key)? else {
            return Ok(false);
        };
        if lease.station != station
            || !self
                .root
                .join("claimed")
                .join(format!("{key}.json"))
                .is_file()
        {
            return Ok(false);
        }
        lease.expires = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs_f64()
            + duration.as_secs_f64();
        let bytes =
            serde_json::to_vec(&LeaseRecord::from_lease(&lease)).map_err(io::Error::other)?;
        write_atomic(
            &self.root.join("leases").join(format!("{key}.json")),
            &bytes,
        )?;
        Ok(true)
    }

    fn expired(&self) -> io::Result<Vec<Lease>> {
        let now = unix_time()?;
        Ok(self
            .leases()?
            .into_iter()
            .filter(|lease| lease.expires < now)
            .collect())
    }

    fn recover(
        &self,
        lease: &Lease,
        max_attempts: u32,
        by: &str,
    ) -> io::Result<Option<LeaseRecovery>> {
        validate_registry_key(lease.key())?;
        let _registry_lock = registry_lock(&self.root)?;
        if self.lease(lease.key())?.as_ref() != Some(lease) || lease.expires >= unix_time()? {
            return Ok(None);
        }
        let source = self
            .root
            .join("claimed")
            .join(format!("{}.json", lease.key()));
        if lease.attempt() < max_attempts {
            match fs::rename(
                &source,
                self.root.join("new").join(format!("{}.json", lease.key())),
            ) {
                Ok(()) => {
                    remove_lease(&self.root, lease.key())?;
                    return Ok(Some(LeaseRecovery::Requeued));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        let bytes = match fs::read(&source) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let brief = match serde_json::from_slice::<Brief>(&bytes) {
            Ok(brief) if brief.validate().is_ok() => brief,
            _ => return Ok(None),
        };
        finish_locked(&self.root, &brief, &lost_outcome(lease, by), false, None)?;
        Ok(Some(LeaseRecovery::Failed))
    }

    fn hand_over(
        &self,
        lease: &Lease,
        station: &str,
        duration: Duration,
        meta: BTreeMap<String, serde_json::Value>,
    ) -> io::Result<Option<Lease>> {
        validate_registry_key(lease.key())?;
        if station.trim().is_empty() || duration.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a Registry lease needs a station and positive duration",
            ));
        }
        let _registry_lock = registry_lock(&self.root)?;
        let now = unix_time()?;
        if self.lease(lease.key())?.as_ref() != Some(lease)
            || lease.expires >= now
            || !self
                .root
                .join("claimed")
                .join(format!("{}.json", lease.key()))
                .is_file()
        {
            return Ok(None);
        }
        let mut handed = lease.clone();
        handed.station = station.to_owned();
        handed.expires = now + duration.as_secs_f64();
        handed.meta.extend(meta);
        let bytes =
            serde_json::to_vec(&LeaseRecord::from_lease(&handed)).map_err(io::Error::other)?;
        write_atomic(
            &self
                .root
                .join("leases")
                .join(format!("{}.json", lease.key())),
            &bytes,
        )?;
        Ok(Some(handed))
    }

    fn finish(
        &self,
        brief: &Brief,
        outcome: &serde_json::Value,
        ok: bool,
        transcript: Option<&[u8]>,
        station: Option<&str>,
    ) -> io::Result<bool> {
        if !outcome.is_object() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Registry outcomes must be JSON objects",
            ));
        }
        let _registry_lock = registry_lock(&self.root)?;
        if let Some(station) = station {
            let key = brief.key();
            let lease = self.lease(&key)?;
            let claimed = self
                .root
                .join("claimed")
                .join(format!("{key}.json"))
                .is_file();
            if lease
                .as_ref()
                .is_none_or(|lease| lease.station() != station)
                || !claimed
            {
                return Ok(false);
            }
        }
        finish_locked(&self.root, brief, outcome, ok, transcript)?;
        Ok(true)
    }
}

/// Station check-ins and settings, and the Headquarters lease.
pub trait StationStore {
    /// Records this station's metadata and current check-in time in the shared Registry.
    ///
    /// Reserved `station` and `seen` keys in `info` are replaced with the supplied identity
    /// and current Unix time. Station IDs are bounded to 256 UTF-8 bytes and may not contain
    /// control characters.
    ///
    /// # Errors
    ///
    /// Returns an error when the station ID is invalid, the record exceeds 1 MiB, or the
    /// Registry cannot be locked or written.
    fn check_in(&self, station: &str, info: BTreeMap<String, serde_json::Value>) -> io::Result<()>;

    /// Returns check-ins no older than `within`, in stable station-file order.
    ///
    /// Malformed, oversized and stale records are skipped.
    fn stations(&self, within: Duration) -> io::Result<Vec<StationRecord>>;

    /// Returns validated live settings for one Station, when configured.
    fn station_settings(&self, station: &str) -> io::Result<Option<StationSettings>>;

    /// Persists validated live settings for one Station in the shared Registry.
    fn set_station_settings(&self, station: &str, settings: &StationSettings) -> io::Result<()>;

    /// Holds, renews or takes the shared Headquarters lease for `station`.
    ///
    /// At most one station wins an expired lease because the shared Registry lock covers the
    /// read, expiry move and replacement. `ttl` must be positive. This uses the same
    /// `headquarters/lease.json` record shared by all AI6 Stations.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid station ID or TTL, malformed Registry state, or an
    /// underlying filesystem or locking failure.
    fn elect(&self, station: &str, ttl: Duration) -> io::Result<bool>;

    /// Reads the Headquarters lease, whether live or expired.
    fn headquarters(&self) -> io::Result<Option<HeadquartersLease>>;
}

impl StationStore for FileRegistry {
    fn check_in(&self, station: &str, info: BTreeMap<String, serde_json::Value>) -> io::Result<()> {
        let bytes = check_in_record(station, info, unix_time()?)?;
        let _registry_lock = registry_lock(&self.root)?;
        let path = self
            .root
            .join("stations")
            .join(format!("{}.json", registry_station_component(station)));
        write_atomic(&path, &bytes)
    }

    fn stations(&self, within: Duration) -> io::Result<Vec<StationRecord>> {
        let directory = self.root.join("stations");
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
            {
                if paths.len() == MAX_STATIONS {
                    return Err(invalid_data(format!(
                        "Registry has more than {MAX_STATIONS} station records"
                    )));
                }
                paths.push(path);
            }
        }
        paths.sort();

        let now = unix_time()?;
        let window = within.as_secs_f64();
        let mut stations = Vec::with_capacity(paths.len());
        for path in paths {
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.len() > MAX_STATION_RECORD_BYTES {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let Ok(file) = serde_json::from_slice::<StationRecordFile>(&bytes) else {
                continue;
            };
            let Ok(record) = file.into_station_record() else {
                continue;
            };
            if now - record.seen <= window {
                stations.push(record);
            }
        }
        Ok(stations)
    }

    fn station_settings(&self, station: &str) -> io::Result<Option<StationSettings>> {
        validate_station_id(station)?;
        let path = self.station_settings_path(station);
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if bytes.len() as u64 > MAX_STATION_SETTINGS_BYTES {
            return Err(invalid_data("Station settings exceed the size limit"));
        }
        let record: StationSettingsFile = serde_json::from_slice(&bytes)
            .map_err(|_| invalid_data("Station settings are invalid"))?;
        if record.station != station {
            return Err(invalid_data("Station settings identity does not match"));
        }
        let defaults = ResourceLimits::default();
        let limits = ResourceLimits::new(
            record
                .min_free_memory_mb
                .unwrap_or(defaults.min_free_memory_mb),
            record.max_cpu_percent.unwrap_or(defaults.max_cpu_percent),
        );
        StationSettings::try_new(record.station, record.max_concurrent_missions)
            .and_then(|settings| settings.with_resource_limits(limits))
            .map(Some)
            .map_err(|_| invalid_data("Station settings are invalid"))
    }

    fn set_station_settings(&self, station: &str, settings: &StationSettings) -> io::Result<()> {
        check_settings(station, settings)?;
        let record = StationSettingsFile {
            station: settings.station.clone(),
            max_concurrent_missions: settings.max_concurrent_missions,
            min_free_memory_mb: Some(settings.resource_limits.min_free_memory_mb),
            max_cpu_percent: Some(settings.resource_limits.max_cpu_percent),
        };
        let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_STATION_SETTINGS_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Station settings exceed the size limit",
            ));
        }
        let _registry_lock = registry_lock(&self.root)?;
        let path = self.station_settings_path(station);
        write_atomic(&path, &bytes)
    }

    fn elect(&self, station: &str, ttl: Duration) -> io::Result<bool> {
        check_election(station, ttl)?;
        let _registry_lock = registry_lock(&self.root)?;
        let directory = self.root.join("headquarters");
        fs::create_dir_all(&directory)?;
        let path = directory.join("lease.json");
        let current = read_headquarters(&path)?;
        let Some(elected) = elected(current.as_ref(), station, ttl, unix_time()?) else {
            return Ok(false);
        };
        if current
            .as_ref()
            .is_some_and(|lease| lease.station != station)
        {
            let lapsed = directory.join(format!(
                "lease.lapsed.{}",
                registry_station_component(station)
            ));
            match fs::rename(&path, lapsed) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        write_headquarters(&path, &elected)?;
        Ok(true)
    }

    fn headquarters(&self) -> io::Result<Option<HeadquartersLease>> {
        let _registry_lock = registry_lock(&self.root)?;
        read_headquarters(&self.root.join("headquarters").join("lease.json"))
    }
}

/// Universal Exports user access tokens, stored by digest.
pub trait AccessTokenStore {
    /// Stores revocable access-token metadata under the shared Registry.
    ///
    /// Only the SHA-256 digest is retained; the bearer secret is returned to its issuer
    /// once and is never written to the Registry. Token IDs and digests are unique.
    fn store_access_token(&self, record: &crate::AccessTokenRecord) -> io::Result<()>;

    /// Returns an active token record by its lowercase SHA-256 digest.
    fn find_access_token(&self, digest: &str) -> io::Result<Option<crate::AccessTokenRecord>>;

    /// Revokes a token by its public UUID, returning false when unknown or already revoked.
    fn revoke_access_token(&self, token_id: &str) -> io::Result<bool>;
}

impl AccessTokenStore for FileRegistry {
    fn store_access_token(&self, record: &crate::AccessTokenRecord) -> io::Result<()> {
        let record = validate_access_token_record(record)?;
        let token_path = self
            .root
            .join("access-tokens")
            .join(format!("{}.json", record.token_id()));
        let digest_path = self
            .root
            .join("access-token-digests")
            .join(format!("{}.txt", record.digest()));
        let _registry_lock = registry_lock(&self.root)?;
        if token_path.exists() || digest_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "access token ID or digest already exists",
            ));
        }
        let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_ACCESS_TOKEN_RECORD_BYTES {
            return Err(invalid_data("access token record exceeds the size limit"));
        }
        write_atomic(&token_path, &bytes)?;
        write_atomic(&digest_path, record.token_id().as_bytes())
    }

    fn find_access_token(&self, digest: &str) -> io::Result<Option<crate::AccessTokenRecord>> {
        validate_token_digest(digest)?;
        let _registry_lock = registry_lock(&self.root)?;
        let digest_path = self
            .root
            .join("access-token-digests")
            .join(format!("{digest}.txt"));
        let token_id = match fs::read_to_string(digest_path) {
            Ok(token_id) => token_id.trim().to_owned(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let token_path = self
            .root
            .join("access-tokens")
            .join(format!("{}.json", registry_station_component(&token_id)));
        let bytes = match fs::read(&token_path) {
            Ok(bytes) if bytes.len() as u64 <= MAX_ACCESS_TOKEN_RECORD_BYTES => bytes,
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let Ok(record) = serde_json::from_slice::<crate::AccessTokenRecord>(&bytes) else {
            return Ok(None);
        };
        let Ok(record) = validate_access_token_record(&record) else {
            return Ok(None);
        };
        if record.token_id() != token_id || record.digest() != digest {
            return Ok(None);
        }
        Ok(record.active_at(unix_time()?).then_some(record))
    }

    fn revoke_access_token(&self, token_id: &str) -> io::Result<bool> {
        let parsed = uuid::Uuid::parse_str(token_id)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid token ID"))?;
        if parsed.to_string() != token_id {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "token ID must be a canonical lowercase UUID",
            ));
        }
        let token_path = self
            .root
            .join("access-tokens")
            .join(format!("{token_id}.json"));
        let _registry_lock = registry_lock(&self.root)?;
        let bytes = match fs::read(&token_path) {
            Ok(bytes) if bytes.len() as u64 <= MAX_ACCESS_TOKEN_RECORD_BYTES => bytes,
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        let Ok(record) = serde_json::from_slice::<crate::AccessTokenRecord>(&bytes) else {
            return Ok(false);
        };
        let Ok(record) = validate_access_token_record(&record) else {
            return Ok(false);
        };
        if record.token_id() != token_id || record.revoked_at().is_some() {
            return Ok(false);
        }
        let updated = crate::AccessTokenRecord::try_new(
            record.token_id(),
            record.digest(),
            record.person_id(),
            record.issued_at(),
            record.expires_at(),
            Some(unix_time()?),
        )
        .map_err(|_| invalid_data("invalid access token record"))?;
        let updated = serde_json::to_vec(&updated).map_err(io::Error::other)?;
        write_atomic(&token_path, &updated)?;
        Ok(true)
    }
}

/// Cables between Agents, their delivery state and send quotas.
pub trait CableStore {
    /// Stores a Cable and returns the original record on an identical idempotent retry.
    ///
    /// The authenticated sender and idempotency key form the unique request key. Reuse
    /// with different immutable content returns `AlreadyExists`.
    fn submit_cable(&self, cable: &Cable) -> io::Result<Cable>;

    /// Finds an accepted Cable by its authenticated sender and idempotency key.
    fn find_cable(&self, sender_agent: &str, idempotency_key: &str) -> io::Result<Option<Cable>>;

    /// Reads a Cable by its relay-generated identifier.
    fn get_cable(&self, cable_id: &str) -> io::Result<Option<Cable>>;

    /// Returns unexpired queued or delivered Cables for an Agent Session.
    ///
    /// `limit` must be between 1 and 1,000. A recipient with no provider Session is
    /// visible to the Agent's current Session.
    fn cable_inbox(
        &self,
        agent: &str,
        session_id: Option<&str>,
        limit: usize,
        now: Option<f64>,
    ) -> io::Result<Vec<Cable>>;

    /// Returns unexpired Cables with at least one held recipient.
    fn held_cables(&self, limit: usize, now: Option<f64>) -> io::Result<Vec<Cable>>;

    /// Atomically changes one recipient's state and returns the updated Cable.
    fn update_cable_delivery(
        &self,
        cable_id: &str,
        agent: &str,
        session_id: Option<&str>,
        state: CableState,
        now: Option<f64>,
    ) -> io::Result<Option<Cable>>;

    /// Atomically consumes one token from every shared Cable quota bucket.
    ///
    /// Keys are SHA-256 hashed before being written to `cable-quotas.json`, matching I
    /// Registry while keeping caller-provided keys out of the file.
    fn take_cable_quotas(&self, buckets: &[(&str, f64, u32)], now: f64) -> io::Result<bool>;
}

impl CableStore for FileRegistry {
    fn submit_cable(&self, cable: &Cable) -> io::Result<Cable> {
        let _registry_lock = registry_lock(&self.root)?;
        for existing in self.scan_cables()? {
            if existing.sender_agent() == cable.sender_agent()
                && existing.idempotency_key() == cable.idempotency_key()
            {
                if existing.same_request(cable) {
                    return Ok(existing);
                }
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Cable idempotency key was reused for different content",
                ));
            }
        }

        let path = self
            .root
            .join("cables")
            .join(format!("{}.json", cable.id()));
        if path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("Cable {} already exists", cable.id()),
            ));
        }
        let bytes = serde_json::to_vec(cable).map_err(io::Error::other)?;
        write_atomic(&path, &bytes)?;
        // The Cable is sent either way; its timeline entry is a record, not a condition.
        let _ = record_event_locked(&self.root, &cable_sent_event(cable));
        Ok(cable.clone())
    }

    fn find_cable(&self, sender_agent: &str, idempotency_key: &str) -> io::Result<Option<Cable>> {
        for cable in self.scan_cables()? {
            if cable.sender_agent() == sender_agent && cable.idempotency_key() == idempotency_key {
                return Ok(Some(cable));
            }
        }
        Ok(None)
    }

    fn get_cable(&self, cable_id: &str) -> io::Result<Option<Cable>> {
        validate_cable_id(cable_id)?;
        read_cable(&self.root.join("cables").join(format!("{cable_id}.json")))
    }

    fn cable_inbox(
        &self,
        agent: &str,
        session_id: Option<&str>,
        limit: usize,
        now: Option<f64>,
    ) -> io::Result<Vec<Cable>> {
        if agent.trim().is_empty() || limit == 0 || limit > 1_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "agent must be non-empty and limit must be between 1 and 1000",
            ));
        }
        let now = now.map_or_else(unix_time, Ok)?;
        if !now.is_finite() {
            return Err(invalid_data("Cable inbox time must be finite"));
        }
        let mut cables = self
            .scan_cables()?
            .into_iter()
            .filter(|cable| {
                cable.expires_at() > now
                    && cable.recipients().iter().any(|recipient| {
                        recipient.agent() == agent
                            && recipient
                                .session_id()
                                .is_none_or(|target| Some(target) == session_id)
                            && matches!(
                                recipient.state(),
                                CableState::Queued | CableState::Delivered
                            )
                    })
            })
            .collect::<Vec<_>>();
        cables.sort_by(|left, right| {
            left.created_at()
                .total_cmp(&right.created_at())
                .then_with(|| left.id().cmp(right.id()))
        });
        cables.truncate(limit);
        Ok(cables)
    }

    fn held_cables(&self, limit: usize, now: Option<f64>) -> io::Result<Vec<Cable>> {
        if limit == 0 || limit > 1_000 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "held Cable limit must be between 1 and 1000",
            ));
        }
        let now = now.map_or_else(unix_time, Ok)?;
        if !now.is_finite() {
            return Err(invalid_data("held Cable time must be finite"));
        }
        let mut cables = self
            .scan_cables()?
            .into_iter()
            .filter(|cable| {
                cable.expires_at() > now
                    && cable
                        .recipients()
                        .iter()
                        .any(|recipient| recipient.state() == CableState::Held)
            })
            .collect::<Vec<_>>();
        cables.sort_by(|left, right| {
            left.created_at()
                .total_cmp(&right.created_at())
                .then_with(|| left.id().cmp(right.id()))
        });
        cables.truncate(limit);
        Ok(cables)
    }

    fn update_cable_delivery(
        &self,
        cable_id: &str,
        agent: &str,
        session_id: Option<&str>,
        state: CableState,
        now: Option<f64>,
    ) -> io::Result<Option<Cable>> {
        validate_cable_id(cable_id)?;
        let _registry_lock = registry_lock(&self.root)?;
        let Some(cable) = self.get_cable(cable_id)? else {
            return Ok(None);
        };
        let now = now.map_or_else(unix_time, Ok)?;
        if !now.is_finite() {
            return Err(invalid_data("Cable delivery time must be finite"));
        }
        let next_state = if cable.expires_at() <= now && state != CableState::Expired {
            CableState::Expired
        } else {
            state
        };
        let Some(updated) = cable
            .with_recipient_state(agent, session_id, next_state)
            .map_err(cable_error)?
        else {
            return Ok(None);
        };
        let path = self
            .root
            .join("cables")
            .join(format!("{}.json", updated.id()));
        let bytes = serde_json::to_vec(&updated).map_err(io::Error::other)?;
        write_atomic(&path, &bytes)?;
        Ok(Some(updated))
    }

    fn take_cable_quotas(&self, buckets: &[(&str, f64, u32)], now: f64) -> io::Result<bool> {
        check_quota_buckets(buckets, now)?;
        let _registry_lock = registry_lock(&self.root)?;
        let path = self.root.join("cable-quotas.json");
        let mut saved = fs::read(&path)
            .ok()
            .and_then(|bytes| {
                serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes).ok()
            })
            .unwrap_or_default();
        let balances = quota_balances(buckets, now, |key| {
            Ok(saved.get(key).and_then(|value| {
                let values = value.as_array()?;
                if values.len() != 2 {
                    return None;
                }
                Some((values[0].as_f64()?, values[1].as_f64()?))
            }))
        })?;
        let allowed = quota_allows(&balances);
        for balance in balances {
            match balance.remaining(allowed) {
                Some(tokens) => saved.insert(balance.key, serde_json::json!([tokens, now])),
                None => saved.remove(&balance.key),
            };
        }
        let bytes = serde_json::to_vec(&saved).map_err(io::Error::other)?;
        write_atomic(&path, &bytes)?;
        Ok(allowed)
    }
}

/// Metadata-only audit records.
pub trait AuditStore {
    /// Stores one metadata-only audit event without retaining request content.
    fn record_audit(&self, entry: &AuditRecord) -> io::Result<()>;

    /// Returns the newest audit events, bounded to 500 metadata-only entries.
    fn list_audit(&self, limit: usize) -> io::Result<Vec<AuditRecord>>;

    /// Returns every stored audit event, in no particular order; at most 10,000 exist.
    fn audit_records(&self) -> io::Result<Vec<AuditRecord>>;
}

impl AuditStore for FileRegistry {
    fn record_audit(&self, entry: &AuditRecord) -> io::Result<()> {
        entry.validate()?;
        let _registry_lock = registry_lock(&self.root)?;
        let directory = self.root.join("audit");
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{}.json", entry.id));
        if path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "audit entry ID already exists",
            ));
        }
        let bytes = serde_json::to_vec_pretty(entry).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_AUDIT_RECORD_BYTES {
            return Err(invalid_data("audit entry exceeds the size limit"));
        }
        write_atomic(&path, &bytes)?;
        Ok(())
    }

    fn list_audit(&self, limit: usize) -> io::Result<Vec<AuditRecord>> {
        if limit == 0 || limit > MAX_AUDIT_RESULTS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("audit result limit must be between 1 and {MAX_AUDIT_RESULTS}"),
            ));
        }
        let mut records = self.audit_records()?;
        records.sort_by(|left, right| {
            right
                .at
                .total_cmp(&left.at)
                .then_with(|| right.id.cmp(&left.id))
        });
        records.truncate(limit);
        Ok(records)
    }

    fn audit_records(&self) -> io::Result<Vec<AuditRecord>> {
        let directory = self.root.join("audit");
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            {
                if paths.len() >= MAX_AUDIT_ENTRIES {
                    return Err(invalid_data(format!(
                        "Registry has more than {MAX_AUDIT_ENTRIES} audit entries"
                    )));
                }
                paths.push(entry.path());
            }
        }
        let mut records = Vec::new();
        for path in paths {
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.len() > MAX_AUDIT_RECORD_BYTES {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let Ok(entry) = serde_json::from_slice::<AuditRecord>(&bytes) else {
                continue;
            };
            if entry.validate().is_ok() {
                records.push(entry);
            }
        }
        Ok(records)
    }
}

/// The Agent-role overlay an operator edits per Operation.
pub trait AgentRoleStore {
    /// Stores an Operation-scoped Agent role override in the shared Registry.
    fn put_agent_role(&self, operation: &str, role: &AgentRole) -> io::Result<()>;

    /// Loads role overrides for one Operation in stable role-ID order.
    fn list_agent_roles(&self, operation: &str) -> io::Result<Vec<AgentRole>>;

    /// Finds an Operation role override by canonical or qualified role identifier.
    fn agent_role(&self, operation: &str, key: &str) -> io::Result<Option<AgentRole>>;

    /// Removes an Operation-scoped role override, returning whether one existed.
    fn remove_agent_role(&self, operation: &str, role_id: &str) -> io::Result<bool>;
}

impl AgentRoleStore for FileRegistry {
    fn put_agent_role(&self, operation: &str, role: &AgentRole) -> io::Result<()> {
        validate_registry_key(operation)?;
        validate_registry_key(&role.id)?;
        role.validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        let bytes = serde_json::to_vec_pretty(role).map_err(io::Error::other)?;
        if bytes.len() as u64 > MAX_AGENT_ROLE_RECORD_BYTES {
            return Err(invalid_data(
                "Agent role configuration exceeds the size limit",
            ));
        }
        let _registry_lock = registry_lock(&self.root)?;
        let directory = self.root.join("agent-roles").join(operation);
        fs::create_dir_all(&directory)?;
        let current = fs::read_dir(&directory)?;
        let mut count = 0;
        for entry in current {
            if entry?.file_type()?.is_file() {
                count += 1;
            }
        }
        let path = directory.join(format!("{}.json", role.id));
        if !path.exists() && count >= MAX_AGENT_ROLES {
            return Err(invalid_data(format!(
                "Operation has reached the {MAX_AGENT_ROLES} Agent role limit"
            )));
        }
        write_atomic(&path, &bytes)
    }

    fn list_agent_roles(&self, operation: &str) -> io::Result<Vec<AgentRole>> {
        validate_registry_key(operation)?;
        let directory = self.root.join("agent-roles").join(operation);
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            {
                if paths.len() >= MAX_AGENT_ROLES {
                    return Err(invalid_data(format!(
                        "Operation has more than {MAX_AGENT_ROLES} Agent roles"
                    )));
                }
                paths.push(entry.path());
            }
        }
        let mut roles = Vec::new();
        for path in paths {
            let metadata = fs::metadata(&path)?;
            if metadata.len() > MAX_AGENT_ROLE_RECORD_BYTES {
                continue;
            }
            let bytes = fs::read(&path)?;
            let Ok(role) = serde_json::from_slice::<AgentRole>(&bytes) else {
                continue;
            };
            if role.validate().is_ok()
                && path.file_stem().and_then(|stem| stem.to_str()) == Some(role.id.as_str())
            {
                roles.push(role);
            }
        }
        roles.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(roles)
    }

    fn agent_role(&self, operation: &str, key: &str) -> io::Result<Option<AgentRole>> {
        let roles = self.list_agent_roles(operation)?;
        Ok(roles
            .into_iter()
            .filter(|role| key == role.id || key.starts_with(&format!("{}/", role.id)))
            .max_by_key(|role| role.id.len()))
    }

    fn remove_agent_role(&self, operation: &str, role_id: &str) -> io::Result<bool> {
        validate_registry_key(operation)?;
        validate_registry_key(role_id)?;
        let _registry_lock = registry_lock(&self.root)?;
        let path = self
            .root
            .join("agent-roles")
            .join(operation)
            .join(format!("{role_id}.json"));
        match fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

pub(super) fn validate_registry_key(key: &str) -> io::Result<()> {
    if key.is_empty()
        || !key.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
        })
        || key == "."
        || key == ".."
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Registry key must contain only ASCII letters, digits, '.', '_' or '-'",
        ));
    }
    Ok(())
}

fn validate_audit_text(value: &str, max_bytes: usize, field: &str) -> io::Result<()> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("audit {field} must be non-empty, bounded and contain no controls"),
        ));
    }
    Ok(())
}

fn validate_audit_action(action: &str) -> io::Result<()> {
    if action.is_empty()
        || action.len() > 128
        || !action.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "audit action must be a bounded lowercase identifier",
        ));
    }
    Ok(())
}

/// The outcome filed for a brief whose last allowed attempt was lost with its Station.
pub(super) fn lost_outcome(lease: &Lease, by: &str) -> serde_json::Value {
    serde_json::json!({
        "state": "lost",
        "reason": format!("station {} went dark; {} attempts spent", lease.station(), lease.attempt()),
        "recovered_by": by,
    })
}

/// The stored form of a check-in: `info` without its reserved keys, stamped with the
/// Station and `seen` (Unix seconds), as pretty JSON within the size limit.
pub(super) fn check_in_record(
    station: &str,
    mut info: BTreeMap<String, serde_json::Value>,
    seen: f64,
) -> io::Result<Vec<u8>> {
    validate_station_id(station)?;
    info.remove("station");
    info.remove("seen");
    let record = StationRecord {
        info,
        station: station.to_owned(),
        seen,
    };
    let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_STATION_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "station record exceeds the size limit",
        ));
    }
    Ok(bytes)
}

/// Checks settings before they are stored for `station`.
pub(super) fn check_settings(station: &str, settings: &StationSettings) -> io::Result<()> {
    validate_station_id(station)?;
    StationSettings::try_new(settings.station.clone(), settings.max_concurrent_missions)?
        .with_resource_limits(settings.resource_limits)?;
    if station != settings.station {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Station settings identity does not match",
        ));
    }
    Ok(())
}

/// Checks a request for the Headquarters lease.
pub(super) fn check_election(station: &str, ttl: Duration) -> io::Result<()> {
    validate_station_id(station)?;
    if ttl.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Headquarters lease duration must be positive",
        ));
    }
    Ok(())
}

/// The Headquarters lease after `station` asks for it at `now`: renewed with the same term
/// when it holds the lease, taken with the next term when the holder's lapsed, and `None`
/// while another Station's lease is live.
pub(super) fn elected(
    current: Option<&HeadquartersLease>,
    station: &str,
    ttl: Duration,
    now: f64,
) -> Option<HeadquartersLease> {
    let expires = now + ttl.as_secs_f64();
    match current {
        Some(held) if held.station == station => Some(HeadquartersLease {
            station: station.to_owned(),
            expires,
            term: held.term,
        }),
        Some(held) if held.expires >= now => None,
        _ => Some(HeadquartersLease {
            station: station.to_owned(),
            expires,
            term: current.map_or(0, |lease| lease.term) + 1,
        }),
    }
}

/// Checks Cable quota buckets: a finite time and unique keys with positive rates and bursts.
pub(super) fn check_quota_buckets(buckets: &[(&str, f64, u32)], now: f64) -> io::Result<()> {
    if !now.is_finite() || buckets.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cable quota buckets need a finite time and at least one limit",
        ));
    }
    let mut keys = BTreeSet::new();
    for (key, rate, capacity) in buckets {
        if key.is_empty()
            || !keys.insert(*key)
            || !rate.is_finite()
            || *rate <= 0.0
            || *capacity == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cable quota needs unique keys, positive rates and positive bursts",
            ));
        }
    }
    Ok(())
}

/// One Cable quota bucket's tokens before a send, keyed by the SHA-256 of its key.
pub(super) struct QuotaBalance {
    pub(super) key: String,
    available: f64,
    capacity: f64,
}

impl QuotaBalance {
    /// The tokens to store after the send, or `None` when the bucket is full again and need
    /// not be stored.
    pub(super) fn remaining(&self, allowed: bool) -> Option<f64> {
        let remaining = if allowed {
            self.available - 1.0
        } else {
            self.available
        };
        (remaining < self.capacity).then_some(remaining)
    }
}

/// Each bucket's tokens at `now`, refilled since `stored` (tokens, time) last saw it.
pub(super) fn quota_balances(
    buckets: &[(&str, f64, u32)],
    now: f64,
    mut stored: impl FnMut(&str) -> io::Result<Option<(f64, f64)>>,
) -> io::Result<Vec<QuotaBalance>> {
    let mut balances = Vec::with_capacity(buckets.len());
    for (key, rate, capacity) in buckets {
        let key = hex_digest(&Sha256::digest(key.as_bytes()));
        let capacity = f64::from(*capacity);
        let (tokens, updated) = stored(&key)?
            .filter(|(tokens, updated)| tokens.is_finite() && updated.is_finite())
            .unwrap_or((capacity, now));
        let available = capacity.min(tokens + (now - updated).max(0.0) * rate);
        balances.push(QuotaBalance {
            key,
            available,
            capacity,
        });
    }
    Ok(balances)
}

/// Whether every bucket has a token for the send.
pub(super) fn quota_allows(balances: &[QuotaBalance]) -> bool {
    balances.iter().all(|balance| balance.available >= 1.0)
}

/// The timeline entry for a sent Cable: its metadata only, never its body or the target
/// name the sending Agent chose.
pub(super) fn cable_sent_event(cable: &Cable) -> TimelineEvent {
    let mut event = TimelineEvent::new(
        "cable",
        "sent",
        cable.sender_operation(),
        cable.created_at(),
    )
    .station(cable.sender_station())
    .actor(cable.sender_agent())
    .detail("cable", cable.id())
    .detail(
        "scope",
        serde_json::to_value(cable.target().scope())
            .ok()
            .and_then(|scope| scope.as_str().map(str::to_owned))
            .unwrap_or_default(),
    )
    .detail("recipients", cable.recipients().len().to_string());
    if let Some(mission) = cable.mission_id() {
        event = event.mission(mission);
    }
    if let Some(session) = cable.session_id() {
        event = event.session(session);
    }
    event
}

pub(super) fn validate_cable_id(id: &str) -> io::Result<()> {
    if id.is_empty()
        || id.chars().count() > 64
        || !id
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Cable id must contain at most 64 letters, digits, '-' or '_'",
        ));
    }
    Ok(())
}

pub(super) fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn read_cable(path: &Path) -> io::Result<Option<Cable>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if bytes.len() as u64 > MAX_CABLE_RECORD_BYTES {
        return Err(invalid_data("Cable record exceeds the size limit"));
    }
    let cable = serde_json::from_slice(&bytes).map_err(|error| invalid_data(error.to_string()))?;
    Ok(Some(cable))
}

fn read_mission_proposal(path: &Path) -> io::Result<MissionProposal> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_MISSION_PROPOSAL_BYTES {
        return Err(invalid_data("Mission proposal exceeds the size limit"));
    }
    let bytes = fs::read(path)?;
    let proposal: MissionProposal =
        serde_json::from_slice(&bytes).map_err(|error| invalid_data(error.to_string()))?;
    proposal
        .validate()
        .map_err(|error| invalid_data(error.to_string()))?;
    Ok(proposal)
}

pub(super) fn cable_error(error: CableError) -> io::Error {
    invalid_data(error.to_string())
}

pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn finish_locked(
    root: &Path,
    brief: &Brief,
    outcome: &serde_json::Value,
    ok: bool,
    transcript: Option<&[u8]>,
) -> io::Result<()> {
    let state = if ok { "done" } else { "failed" };
    let key = brief.key();
    let target = root.join(state).join(format!("{key}.json"));
    let outcome_path = root.join(state).join(format!("{key}.outcome.json"));
    write_atomic(
        &outcome_path,
        &serde_json::to_vec_pretty(outcome).map_err(io::Error::other)?,
    )?;
    if let Some(transcript) = transcript {
        write_atomic(&root.join(state).join(format!("{key}.log")), transcript)?;
    }
    let source = root.join("claimed").join(format!("{key}.json"));
    if source.is_file() {
        fs::rename(source, &target)?;
    } else {
        write_atomic(
            &target,
            &serde_json::to_vec_pretty(brief).map_err(io::Error::other)?,
        )?;
    }
    remove_lease(root, &key)
}

fn remove_lease(root: &Path, key: &str) -> io::Result<()> {
    match fs::remove_file(root.join("leases").join(format!("{key}.json"))) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn unix_time() -> io::Result<f64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs_f64())
}

pub(crate) fn validate_station_id(station: &str) -> io::Result<()> {
    if station.is_empty() || station.len() > 256 || station.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "station ID must be non-empty, at most 256 UTF-8 bytes, and contain no control characters",
        ));
    }
    Ok(())
}

pub(super) fn validate_access_token_record(
    record: &crate::AccessTokenRecord,
) -> io::Result<crate::AccessTokenRecord> {
    crate::AccessTokenRecord::try_new(
        record.token_id(),
        record.digest(),
        record.person_id(),
        record.issued_at(),
        record.expires_at(),
        record.revoked_at(),
    )
    .map_err(|_| invalid_data("invalid access token record"))
}

pub(super) fn validate_token_digest(digest: &str) -> io::Result<()> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "token digest must be lowercase SHA-256 hex",
        ));
    }
    Ok(())
}

fn registry_station_component(station: &str) -> String {
    station
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '_' | '.' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn read_headquarters(path: &Path) -> io::Result<Option<HeadquartersLease>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if bytes.len() as u64 > MAX_STATION_RECORD_BYTES {
        return Err(invalid_data("Headquarters lease exceeds the size limit"));
    }
    let record: HeadquartersLeaseFile = serde_json::from_slice(&bytes)
        .map_err(|error| invalid_data(format!("invalid Headquarters lease: {error}")))?;
    Ok(Some(record.into_lease()?))
}

fn write_headquarters(path: &Path, lease: &HeadquartersLease) -> io::Result<()> {
    let bytes = serde_json::to_vec(lease).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_STATION_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Headquarters lease exceeds the size limit",
        ));
    }
    write_atomic(path, &bytes)
}

pub(super) fn registry_lock(root: &Path) -> io::Result<File> {
    fs::create_dir_all(root)?;
    let path = root.join(".ai6-registry.lock");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)?;
    #[cfg(windows)]
    let file = {
        let mut file = file;
        // Another handle may have written the byte and locked it between the length check
        // and this write; Windows then refuses the write (lock violation), which is fine.
        if file.metadata()?.len() == 0
            && let Err(error) = file.write_all(b"\0").and_then(|()| file.flush())
            && !lock_is_contended(&error)
        {
            return Err(error);
        }
        file
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error) if lock_is_contended(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) if Instant::now() >= deadline => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out locking the Registry: {error}"),
                ));
            }
            Err(error) => return Err(error),
        }
    }
}

fn lock_is_contended(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::PermissionDenied
    ) {
        return true;
    }
    #[cfg(windows)]
    return matches!(error.raw_os_error(), Some(32 | 33));
    #[cfg(not(windows))]
    false
}

/// Returns the default Registry location.
pub fn default_registry_dir() -> io::Result<PathBuf> {
    if let Some(path) = nonempty_env("AI6_REGISTRY") {
        return Ok(path.into());
    }
    if let Some(path) = nonempty_env("AI6_HOME") {
        return Ok(PathBuf::from(path).join("registry"));
    }
    #[cfg(windows)]
    if let Some(path) = nonempty_env("LOCALAPPDATA") {
        return Ok(PathBuf::from(path).join("AI6").join("registry"));
    }
    let state = nonempty_env("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/state"))
        })
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no AI6 Registry state directory")
        })?;
    Ok(state.join("ai6").join("registry"))
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub(super) fn safe_component(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut replacing = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-') {
            result.push(character);
            replacing = false;
        } else if !replacing {
            result.push('_');
            replacing = true;
        }
    }
    result
}

fn default_id() -> String {
    "unknown".to_owned()
}

fn default_source() -> String {
    "manual".to_owned()
}

fn default_version() -> u32 {
    BRIEF_VERSION
}

pub(super) fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::cables::{
        CableDraft, CableRecipient, CableScope, CableTarget,
    };
    use crate::directorates::registry::sources::SourceItem;
    use crate::directorates::registry::suite::{Store, backend_tests, brief, lapse};
    use std::fs;

    fn test_cable(id: &str, key: &str, body: &str, session_id: &str) -> Cable {
        Cable::try_new(CableDraft::new(
            "007".into(),
            "operation-a".into(),
            "station-a".into(),
            CableTarget::try_new(CableScope::Agent, Some("008".into())).unwrap(),
            body.into(),
            vec![
                CableRecipient::try_new("008".into(), Some(session_id.into()), CableState::Queued)
                    .unwrap(),
            ],
            key.into(),
            id.into(),
            1_700_000_000.0,
            1_700_000_900.0,
        ))
        .unwrap()
    }

    fn test_agent_role(id: &str, purpose: &str) -> AgentRole {
        AgentRole {
            id: id.to_owned(),
            purpose: purpose.to_owned(),
            policy: "Follow the Operation policy.".to_owned(),
        }
    }

    fn proposal(retry_key: &str, title: &str, role: Option<&str>) -> MissionProposal {
        MissionProposal::try_new(
            "operation-a",
            MissionId::try_new("parent-mission").unwrap(),
            "coordinator",
            SessionId::try_new("provider-session").unwrap(),
            retry_key,
            Brief {
                title: title.to_owned(),
                body: "Bounded child brief".to_owned(),
                id: "child".to_owned(),
                source: "agent".to_owned(),
                operation: Some("ignored-operation".to_owned()),
                agent: None,
                role: role.map(str::to_owned),
                tags: Vec::new(),
                stations: Vec::new(),
                priority: 0,
                url: None,
                data: BTreeMap::new(),
                created: String::new(),
                version: 1,
            },
        )
        .unwrap()
    }

    fn source_item(id: &str) -> SourceItem {
        SourceItem {
            id: id.to_owned(),
            title: "Fix the build".to_owned(),
            body: "It fails.".to_owned(),
            url: None,
            labels: Vec::new(),
            author: None,
            updated: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    fn access_token(now: f64) -> crate::AccessTokenRecord {
        let secret = "ai6_device_secret_0123456789abcdef";
        let digest = Sha256::digest([b"ai6-access-token\0".as_slice(), secret.as_bytes()].concat())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        crate::AccessTokenRecord::try_new(
            "e747dbba-9a28-4463-a1ab-e31ed79d8991",
            digest,
            "person-007",
            now,
            now + 3_600.0,
            None,
        )
        .unwrap()
    }

    backend_tests!(
        operation_agent_role_overrides_are_validated_and_isolated,
        check_ins_replace_reserved_keys_and_list_recent_stations,
        headquarters_election_has_one_winner_and_renews_the_winner,
        the_headquarters_term_grows_on_takeover_and_survives_renewal,
        site_check_in_rejects_invalid_identity_and_oversized_metadata,
        station_settings_are_validated_persisted_and_isolated_by_identity,
        queued_briefs_list_by_priority_creation_time_and_key,
        mission_proposals_are_idempotent_operation_scoped_and_not_runnable,
        mission_proposal_identity_cannot_be_reused_for_different_content,
        proposal_approval_promotes_once_and_rejection_stays_out_of_intake,
        submit_brief_writes_once_to_shared_queue,
        audit_entries_are_metadata_only_bounded_and_sorted_newest_first,
        one_station_claims_a_brief,
        a_leased_claim_is_listed_and_renewed_only_by_its_holder,
        finish_requires_the_current_station_and_files_the_outcome,
        expired_leases_requeue_or_fail_at_the_attempt_limit,
        an_expired_lease_is_handed_over_once_with_its_claim_and_attempt,
        leased_claim_is_exclusive_and_registry_keys_are_checked,
        cable_store_is_idempotent_and_rejects_conflicting_retries,
        cable_inbox_and_delivery_updates_follow_session_and_expiry,
        cable_quotas_are_atomic_and_shared,
        access_tokens_are_shared_by_registries_and_revocation_is_immediate,
    );

    fn operation_agent_role_overrides_are_validated_and_isolated(store: &dyn Store) {
        let registry = store.handle();
        let first = test_agent_role("reviewer", "Review changes");
        registry.put_agent_role("operation-a", &first).unwrap();

        assert_eq!(
            registry.list_agent_roles("operation-a").unwrap(),
            vec![first.clone()]
        );
        assert!(
            registry
                .agent_role("operation-a", "reviewer/security")
                .unwrap()
                .is_some()
        );
        assert!(registry.list_agent_roles("operation-b").unwrap().is_empty());

        let updated = test_agent_role("reviewer", "Review security changes");
        registry.put_agent_role("operation-a", &updated).unwrap();
        assert_eq!(
            registry.list_agent_roles("operation-a").unwrap(),
            vec![updated]
        );

        let invalid = test_agent_role("../escape", "Invalid role");
        assert!(registry.put_agent_role("operation-a", &invalid).is_err());
        assert!(
            registry
                .remove_agent_role("operation-a", "reviewer")
                .unwrap()
        );
        assert!(
            !registry
                .remove_agent_role("operation-a", "reviewer")
                .unwrap()
        );
    }

    fn check_ins_replace_reserved_keys_and_list_recent_stations(store: &dyn Store) {
        let registry = store.handle();
        let mut info = BTreeMap::new();
        info.insert("station_count".to_owned(), serde_json::json!(3));
        info.insert("station".to_owned(), serde_json::json!("forged"));
        info.insert("seen".to_owned(), serde_json::json!(-1));
        let before = unix_time().unwrap();

        registry.check_in("Stätion/one", info).unwrap();

        let stations = store.handle().stations(DEFAULT_LEASE).unwrap();
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].station(), "Stätion/one");
        assert_eq!(stations[0].info()["station_count"], 3);
        assert!(!stations[0].info().contains_key("station"));
        // A SQL Registry stamps the database's clock, to the millisecond.
        assert!((stations[0].seen() - before).abs() < 60.0);
        registry.check_in("Stätion/one", BTreeMap::new()).unwrap();
        assert!(
            registry.stations(DEFAULT_LEASE).unwrap()[0]
                .info()
                .is_empty(),
            "a check-in replaces the last one"
        );
    }

    fn headquarters_election_has_one_winner_and_renews_the_winner(store: &dyn Store) {
        let first = store.handle();
        let second = store.handle();
        let first_thread =
            std::thread::spawn(move || first.elect("station-a", DEFAULT_LEASE).unwrap());
        let second_thread =
            std::thread::spawn(move || second.elect("station-b", DEFAULT_LEASE).unwrap());
        let won = [first_thread.join().unwrap(), second_thread.join().unwrap()];

        assert_eq!(won.iter().filter(|won| **won).count(), 1);
        let registry = store.handle();
        let current = registry.headquarters().unwrap().unwrap();
        let winner = if won[0] { "station-a" } else { "station-b" };
        let loser = if won[0] { "station-b" } else { "station-a" };
        assert_eq!(current.station(), winner);
        assert!(!registry.elect(loser, DEFAULT_LEASE).unwrap());
        assert!(registry.elect(winner, DEFAULT_LEASE).unwrap());
        assert!(registry.headquarters().unwrap().unwrap().expires() >= current.expires());
    }

    fn the_headquarters_term_grows_on_takeover_and_survives_renewal(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.headquarters().unwrap().is_none());
        assert!(registry.elect("station-a", DEFAULT_LEASE).unwrap());
        assert_eq!(registry.headquarters().unwrap().unwrap().term(), 1);
        assert!(registry.elect("station-a", DEFAULT_LEASE).unwrap());
        assert_eq!(registry.headquarters().unwrap().unwrap().term(), 1);

        // The holder's last renewal runs out; another Station takes over in the next term.
        assert!(
            registry
                .elect("station-a", Duration::from_nanos(1))
                .unwrap()
        );
        // Expiry is the backend's call: SQL judges it by the database's clock.
        let standby = store.handle();
        crate::directorates::registry::suite::until(|| {
            standby
                .elect("station-b", DEFAULT_LEASE)
                .unwrap()
                .then_some(())
        });
        let taken = registry.headquarters().unwrap().unwrap();
        assert_eq!((taken.station(), taken.term()), ("station-b", 2));
        assert!(!registry.elect("station-a", DEFAULT_LEASE).unwrap());
    }

    fn site_check_in_rejects_invalid_identity_and_oversized_metadata(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(
            registry
                .check_in("bad\nsite", BTreeMap::new())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );

        let mut info = BTreeMap::new();
        info.insert(
            "payload".to_owned(),
            serde_json::Value::String("x".repeat(MAX_STATION_RECORD_BYTES as usize)),
        );
        assert_eq!(
            registry.check_in("station-a", info).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    fn station_settings_are_validated_persisted_and_isolated_by_identity(store: &dyn Store) {
        let registry = store.handle();
        let first = StationSettings::try_new("station/a", 7).unwrap();
        let second = StationSettings::try_new("station?a", 9).unwrap();

        assert_eq!(registry.station_settings("station/a").unwrap(), None);
        registry.set_station_settings("station/a", &first).unwrap();
        registry.set_station_settings("station?a", &second).unwrap();
        assert_eq!(
            registry.station_settings("station/a").unwrap(),
            Some(first.clone())
        );
        assert_eq!(
            store.handle().station_settings("station?a").unwrap(),
            Some(second)
        );
        assert!(StationSettings::try_new("station/a", 0).is_err());
        assert!(StationSettings::try_new("station/a", MAX_STATION_MISSION_CAPACITY + 1).is_err());
        assert!(registry.set_station_settings("station?a", &first).is_err());

        // Resource limits default and persist.
        assert_eq!(first.resource_limits(), ResourceLimits::default());
        let loose = first
            .clone()
            .with_resource_limits(ResourceLimits::new(0, 100))
            .unwrap();
        registry.set_station_settings("station/a", &loose).unwrap();
        assert_eq!(
            registry
                .station_settings("station/a")
                .unwrap()
                .unwrap()
                .resource_limits(),
            ResourceLimits::new(0, 100)
        );
        assert!(
            first
                .with_resource_limits(ResourceLimits::new(0, 101))
                .is_err()
        );
    }

    #[test]
    fn station_settings_written_before_resource_limits_read_the_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(directory.path());
        fs::create_dir_all(
            registry
                .station_settings_path("station/b")
                .parent()
                .unwrap(),
        )
        .unwrap();
        fs::write(
            registry.station_settings_path("station/b"),
            br#"{"station":"station/b","max_concurrent_missions":3}"#,
        )
        .unwrap();
        let older = registry.station_settings("station/b").unwrap().unwrap();
        assert_eq!(older.max_concurrent_missions(), 3);
        assert_eq!(older.resource_limits(), ResourceLimits::default());
    }

    fn queued_briefs_list_by_priority_creation_time_and_key(store: &dyn Store) {
        let registry = store.handle();
        for brief in [
            r#"{"title":"low","id":"low","priority":2,"created":"2026-01-01"}"#,
            r#"{"title":"high late","id":"late","priority":9,"created":"2026-02-01"}"#,
            r#"{"title":"high early b","id":"early-b","priority":9,"created":"2026-01-01"}"#,
            r#"{"title":"high early a","id":"early-a","priority":9,"created":"2026-01-01"}"#,
        ] {
            registry
                .submit_brief(&serde_json::from_str(brief).unwrap())
                .unwrap();
        }
        assert_eq!(
            store
                .handle()
                .list_new()
                .unwrap()
                .iter()
                .map(|brief| brief.title.as_str())
                .collect::<Vec<_>>(),
            ["high early a", "high early b", "high late", "low"]
        );
    }

    fn mission_proposals_are_idempotent_operation_scoped_and_not_runnable(store: &dyn Store) {
        let registry = store.handle();
        let proposal = proposal("child-1", "Inspect the role catalog", Some("analyst"));

        registry.submit_mission_proposal(&proposal).unwrap();
        // A repeat of the same proposal is accepted and stores nothing new.
        registry.submit_mission_proposal(&proposal).unwrap();

        assert_eq!(registry.list_new().unwrap(), []);
        let proposals = registry.list_mission_proposals("operation-a", 10).unwrap();
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].id, proposal.id);
        assert_eq!(proposals[0].brief.operation.as_deref(), Some("operation-a"));
        assert_eq!(
            registry.list_mission_proposals("operation-b", 10).unwrap(),
            []
        );
    }

    fn mission_proposal_identity_cannot_be_reused_for_different_content(store: &dyn Store) {
        let registry = store.handle();
        registry
            .submit_mission_proposal(&proposal("same-retry-key", "First brief", None))
            .unwrap();

        assert_eq!(
            registry
                .submit_mission_proposal(&proposal("same-retry-key", "Different brief", None))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    fn proposal_approval_promotes_once_and_rejection_stays_out_of_intake(store: &dyn Store) {
        let registry = store.handle();
        let approved = proposal("approve-key", "Approved work", Some("analyst"));
        let rejected = proposal("reject-key", "Rejected work", Some("analyst"));
        registry.submit_mission_proposal(&approved).unwrap();
        registry.submit_mission_proposal(&rejected).unwrap();

        let promoted = registry
            .promote_mission_proposal("operation-a", &approved.id)
            .unwrap();

        assert_eq!(promoted, approved.brief);
        assert_eq!(
            registry
                .promote_mission_proposal("operation-a", &approved.id)
                .unwrap(),
            promoted
        );
        assert_eq!(registry.list_new().unwrap(), [promoted]);
        let pending = registry.list_mission_proposals("operation-a", 10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, rejected.id);

        let declined = registry
            .reject_mission_proposal("operation-a", &rejected.id)
            .unwrap();

        assert_eq!(declined.id, rejected.id);
        assert_eq!(
            registry
                .reject_mission_proposal("operation-a", &rejected.id)
                .unwrap(),
            declined
        );
        assert!(
            registry
                .list_mission_proposals("operation-a", 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(registry.list_new().unwrap().len(), 1);
        assert_eq!(
            registry
                .promote_mission_proposal("operation-a", &rejected.id)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            registry
                .reject_mission_proposal("operation-a", &approved.id)
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            registry
                .promote_mission_proposal("operation-a", "proposal-unknown")
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    fn submit_brief_writes_once_to_shared_queue(store: &dyn Store) {
        let registry = store.handle();
        let brief = Brief {
            title: "Review the control pane".to_owned(),
            body: "Check the Operation context.".to_owned(),
            id: "mission-123".to_owned(),
            source: "universal-exports".to_owned(),
            operation: Some("operation-a".to_owned()),
            agent: None,
            role: Some("implement".to_owned()),
            tags: vec!["ui".to_owned()],
            stations: Vec::new(),
            priority: 0,
            url: None,
            data: BTreeMap::new(),
            created: String::new(),
            version: 1,
        };

        registry.submit_brief(&brief).unwrap();
        let queued = store.handle().list_new().unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0], brief);
        assert_eq!(
            registry.submit_brief(&brief).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    fn audit_entries_are_metadata_only_bounded_and_sorted_newest_first(store: &dyn Store) {
        let registry = store.handle();
        let older = AuditRecord {
            id: "event-1".to_owned(),
            actor: "operator-007".to_owned(),
            action: "mission.create".to_owned(),
            operation: "operation-a".to_owned(),
            resource: None,
            target: "universal-exports--mission-1".to_owned(),
            at: 100.0,
        };
        let newer = AuditRecord {
            id: "event-2".to_owned(),
            at: 101.0,
            ..older.clone()
        };

        registry.record_audit(&older).unwrap();
        registry.record_audit(&newer).unwrap();
        assert_eq!(registry.list_audit(10).unwrap(), [newer, older.clone()]);
        assert_eq!(
            registry.record_audit(&older).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            registry
                .list_audit(MAX_AUDIT_RESULTS + 1)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            registry
                .record_audit(&AuditRecord {
                    target: "prompt\nsecret".to_owned(),
                    id: "event-3".to_owned(),
                    ..older
                })
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(store.handle().audit_records().unwrap().len(), 2);
    }

    fn one_station_claims_a_brief(store: &dyn Store) {
        store.handle().submit_brief(&brief("brief-1")).unwrap();
        let first = store.handle();
        let second = store.handle();
        let first_thread = std::thread::spawn(move || first.claim_new(None).unwrap());
        let second_thread = std::thread::spawn(move || second.claim_new(None).unwrap());
        let claims = [first_thread.join().unwrap(), second_thread.join().unwrap()];

        assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
        let registry = store.handle();
        assert!(registry.read_claimed("manual--brief-1").unwrap().is_some());
        assert!(registry.list_new().unwrap().is_empty());
    }

    fn a_leased_claim_is_listed_and_renewed_only_by_its_holder(store: &dyn Store) {
        let registry = store.handle();
        registry.submit_brief(&brief("brief-1")).unwrap();
        let mut meta = BTreeMap::new();
        meta.insert("operation".to_owned(), serde_json::json!("operation-01"));

        let lease = registry
            .claim_with_lease(None, "operation-01", DEFAULT_LEASE, meta.clone())
            .unwrap()
            .unwrap();
        assert_eq!(lease.key(), "manual--brief-1");
        assert_eq!(lease.station(), "operation-01");
        assert_eq!(lease.attempt(), 1);
        assert_eq!(lease.meta(), &meta);
        assert!(registry.list_new().unwrap().is_empty());
        assert!(registry.read_claimed(lease.key()).unwrap().is_some());

        let other = store.handle();
        let reopened = other.lease(lease.key()).unwrap().unwrap();
        assert_eq!(reopened.key(), lease.key());
        assert_eq!(reopened.station(), lease.station());
        assert_eq!(reopened.attempt(), lease.attempt());
        assert_eq!(reopened.meta(), lease.meta());
        assert!((reopened.expires() - lease.expires()).abs() < 0.000_001);
        let listed = other.leases().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key(), lease.key());
        assert!(other.expired().unwrap().is_empty());

        assert!(
            !registry
                .renew(lease.key(), "operation-02", DEFAULT_LEASE)
                .unwrap()
        );
        assert!(
            registry
                .renew(lease.key(), "operation-01", DEFAULT_LEASE)
                .unwrap()
        );
        assert!(registry.lease(lease.key()).unwrap().unwrap().expires() >= lease.expires());
        assert!(
            !registry
                .renew("manual--absent", "operation-01", DEFAULT_LEASE)
                .unwrap()
        );
    }

    fn finish_requires_the_current_station_and_files_the_outcome(store: &dyn Store) {
        let registry = store.handle();
        registry
            .ingest_source_item("G", "github", &source_item("o/r#1"))
            .unwrap();
        let brief = registry.list_new().unwrap().remove(0);
        registry
            .claim_with_lease(
                Some(&brief.key()),
                "operation-01",
                DEFAULT_LEASE,
                BTreeMap::new(),
            )
            .unwrap()
            .unwrap();

        let outcome = serde_json::json!({"state": "done", "mission_id": "mission-1"});
        assert!(
            !registry
                .finish(
                    &brief,
                    &outcome,
                    true,
                    Some(b"output"),
                    Some("operation-02")
                )
                .unwrap()
        );
        assert!(registry.unreported_source_briefs().unwrap().is_empty());
        assert!(
            registry
                .finish(
                    &brief,
                    &outcome,
                    true,
                    Some(b"output"),
                    Some("operation-01")
                )
                .unwrap()
        );
        assert!(registry.lease(&brief.key()).unwrap().is_none());
        assert!(registry.read_claimed(&brief.key()).unwrap().is_none());
        assert!(registry.list_new().unwrap().is_empty());
        let finished = store.handle().unreported_source_briefs().unwrap();
        assert_eq!(finished.len(), 1);
        assert_eq!((finished[0].ok, &finished[0].outcome), (true, &outcome));
        assert_eq!(
            registry
                .finish(&brief, &serde_json::json!("done"), true, None, None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    fn expired_leases_requeue_or_fail_at_the_attempt_limit(store: &dyn Store) {
        for (id, max_attempts, expected) in [
            ("o/r#requeue", 3, LeaseRecovery::Requeued),
            ("o/r#fail", 1, LeaseRecovery::Failed),
        ] {
            let registry = store.handle();
            registry
                .ingest_source_item("G", "github", &source_item(id))
                .unwrap();
            let lease = registry
                .claim_with_lease(None, "operation-01", DEFAULT_LEASE, BTreeMap::new())
                .unwrap()
                .unwrap();
            let key = lease.key().to_owned();
            assert_eq!(
                registry
                    .recover(&lease, max_attempts, "operation-02")
                    .unwrap(),
                None
            );

            let lapsed = lapse(registry.as_ref(), &key, "operation-01");
            let expired = registry.expired().unwrap();
            assert_eq!(expired.len(), 1);
            assert_eq!(expired[0], lapsed);
            assert_eq!(
                store
                    .handle()
                    .recover(&expired[0], max_attempts, "operation-02")
                    .unwrap(),
                Some(expected)
            );
            assert_eq!(
                registry
                    .recover(&expired[0], max_attempts, "operation-02")
                    .unwrap(),
                None,
                "a lease is recovered once"
            );
            assert!(registry.lease(&key).unwrap().is_none());
            match expected {
                LeaseRecovery::Requeued => {
                    assert_eq!(registry.list_new().unwrap()[0].key(), key);
                    let again = registry
                        .claim_with_lease(None, "operation-02", DEFAULT_LEASE, BTreeMap::new())
                        .unwrap()
                        .unwrap();
                    assert_eq!(again.attempt(), 2);
                    registry
                        .finish(
                            &registry.read_claimed(&key).unwrap().unwrap(),
                            &serde_json::json!({"state": "done"}),
                            true,
                            None,
                            Some("operation-02"),
                        )
                        .unwrap();
                }
                LeaseRecovery::Failed => {
                    assert!(registry.list_new().unwrap().is_empty());
                    let failed = registry
                        .unreported_source_briefs()
                        .unwrap()
                        .into_iter()
                        .find(|finished| finished.brief.key() == key)
                        .unwrap();
                    assert!(!failed.ok);
                    assert_eq!(failed.outcome["state"], "lost");
                    assert_eq!(failed.outcome["recovered_by"], "operation-02");
                }
            }
        }
    }

    fn an_expired_lease_is_handed_over_once_with_its_claim_and_attempt(store: &dyn Store) {
        let registry = store.handle();
        registry.submit_brief(&brief("hand-over")).unwrap();
        let meta = BTreeMap::from([("mission_id".to_owned(), serde_json::json!("m-1"))]);
        let lease = registry
            .claim_with_lease(None, "station-a", DEFAULT_LEASE, meta)
            .unwrap()
            .unwrap();
        let extra = BTreeMap::from([("adopt".to_owned(), serde_json::json!("s-1"))]);

        // A live lease stays with its holder.
        assert_eq!(
            registry
                .hand_over(&lease, "station-b", DEFAULT_LEASE, extra.clone())
                .unwrap(),
            None
        );

        // The holder's last renewal runs out.
        let expired = lapse(registry.as_ref(), lease.key(), "station-a");
        let handed = store
            .handle()
            .hand_over(&expired, "station-b", DEFAULT_LEASE, extra.clone())
            .unwrap()
            .unwrap();
        assert_eq!(handed.station(), "station-b");
        assert_eq!(handed.attempt(), lease.attempt());
        assert_eq!(handed.meta()["mission_id"], "m-1");
        assert_eq!(handed.meta()["adopt"], "s-1");
        let stored = registry.lease(lease.key()).unwrap().unwrap();
        assert_eq!(
            (stored.station(), stored.attempt(), stored.meta()),
            (handed.station(), handed.attempt(), handed.meta())
        );
        assert!(registry.read_claimed(lease.key()).unwrap().is_some());

        // The old view of the lease no longer matches, so it is not handed over twice.
        assert_eq!(
            registry
                .hand_over(&expired, "station-c", DEFAULT_LEASE, extra)
                .unwrap(),
            None
        );
        assert!(
            registry
                .renew(lease.key(), "station-b", DEFAULT_LEASE)
                .unwrap()
        );
        assert!(
            !registry
                .renew(lease.key(), "station-a", DEFAULT_LEASE)
                .unwrap()
        );
    }

    fn leased_claim_is_exclusive_and_registry_keys_are_checked(store: &dyn Store) {
        store.handle().submit_brief(&brief("brief-1")).unwrap();
        let first = store.handle();
        let second = store.handle();
        let first_thread = std::thread::spawn(move || {
            first.claim_with_lease(None, "one", DEFAULT_LEASE, BTreeMap::new())
        });
        let second_thread = std::thread::spawn(move || {
            second.claim_with_lease(None, "two", DEFAULT_LEASE, BTreeMap::new())
        });
        let claims = [
            first_thread.join().unwrap().unwrap(),
            second_thread.join().unwrap().unwrap(),
        ];
        assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
        let registry = store.handle();
        assert!(registry.lease("../outside").is_err());
        assert!(registry.read_claimed("../outside").is_err());
        assert_eq!(
            registry
                .claim_with_lease(None, " ", DEFAULT_LEASE, BTreeMap::new())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    fn cable_store_is_idempotent_and_rejects_conflicting_retries(store: &dyn Store) {
        let registry = store.handle();
        let cable = test_cable("cable-1", "send-1", "Review this", "session-8");

        assert_eq!(registry.submit_cable(&cable).unwrap(), cable);
        let retry = test_cable("cable-retry", "send-1", "Review this", "session-8");
        assert_eq!(store.handle().submit_cable(&retry).unwrap(), cable);
        assert_eq!(
            registry.find_cable("007", "send-1").unwrap(),
            Some(cable.clone())
        );
        assert_eq!(registry.get_cable("cable-1").unwrap(), Some(cable));
        assert_eq!(registry.get_cable("cable-retry").unwrap(), None);

        let conflict = test_cable("cable-2", "send-1", "Different body", "session-8");
        assert_eq!(
            registry.submit_cable(&conflict).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );

        // One timeline entry for the one Cable sent: its metadata, never its body.
        let operation = registry
            .get_cable("cable-1")
            .unwrap()
            .unwrap()
            .sender_operation()
            .to_owned();
        let events = registry.timeline_events(&operation).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            (events[0].kind.as_str(), events[0].action.as_str()),
            ("cable", "sent")
        );
        assert_eq!(events[0].actor.as_deref(), Some("007"));
        assert_eq!(events[0].detail["cable"], "cable-1");
        assert_eq!(events[0].detail["recipients"], "1");
        assert!(
            !serde_json::to_string(&events)
                .unwrap()
                .contains("Review this")
        );
    }

    fn cable_inbox_and_delivery_updates_follow_session_and_expiry(store: &dyn Store) {
        let registry = store.handle();
        let cable = test_cable("cable-1", "send-1", "Review this", "session-8");
        registry.submit_cable(&cable).unwrap();

        assert!(
            registry
                .cable_inbox("008", Some("session-other"), 10, Some(1_700_000_100.0))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            registry
                .cable_inbox("008", Some("session-8"), 10, Some(1_700_000_100.0))
                .unwrap()
                .len(),
            1
        );

        let held = registry
            .update_cable_delivery(
                "cable-1",
                "008",
                Some("session-8"),
                CableState::Held,
                Some(1_700_000_100.0),
            )
            .unwrap()
            .unwrap();
        let listed = store
            .handle()
            .held_cables(10, Some(1_700_000_100.0))
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], held);
        assert!(
            registry
                .held_cables(10, Some(1_700_001_000.0))
                .unwrap()
                .is_empty()
        );

        let delivered = registry
            .update_cable_delivery(
                "cable-1",
                "008",
                Some("session-8"),
                CableState::Delivered,
                Some(1_700_000_100.0),
            )
            .unwrap()
            .unwrap();
        assert_eq!(delivered.recipients()[0].state(), CableState::Delivered);
        assert!(
            registry
                .held_cables(10, Some(1_700_000_100.0))
                .unwrap()
                .is_empty()
        );
        let expired = registry
            .update_cable_delivery(
                "cable-1",
                "008",
                Some("session-8"),
                CableState::Acked,
                Some(1_700_001_000.0),
            )
            .unwrap()
            .unwrap();
        assert_eq!(expired.recipients()[0].state(), CableState::Expired);
        assert!(
            registry
                .cable_inbox("008", Some("session-8"), 10, Some(1_700_000_100.0))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            registry
                .update_cable_delivery("cable-9", "008", None, CableState::Acked, None)
                .unwrap(),
            None
        );
    }

    fn cable_quotas_are_atomic_and_shared(store: &dyn Store) {
        let registry = store.handle();
        let buckets = [("sender:007", 1.0, 2), ("scope:operation:G", 1.0, 1)];

        assert!(registry.take_cable_quotas(&buckets, 100.0).unwrap());
        assert!(!registry.take_cable_quotas(&buckets, 100.0).unwrap());
        assert!(
            registry
                .take_cable_quotas(&[("sender:007", 1.0, 2)], 100.0)
                .unwrap()
        );
        assert!(
            !registry
                .take_cable_quotas(&[("sender:007", 1.0, 2)], 100.0)
                .unwrap()
        );
        assert!(registry.take_cable_quotas(&buckets, 101.0).unwrap());
        assert!(!store.handle().take_cable_quotas(&buckets, 101.0).unwrap());
        assert_eq!(
            registry
                .take_cable_quotas(&[("a", 1.0, 1), ("a", 1.0, 1)], 1.0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    fn access_tokens_are_shared_by_registries_and_revocation_is_immediate(store: &dyn Store) {
        let issuer = store.handle();
        let verifier = store.handle();
        let record = access_token(unix_time().unwrap());
        issuer.store_access_token(&record).unwrap();
        assert_eq!(
            issuer.store_access_token(&record).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );

        let shared = verifier
            .find_access_token(record.digest())
            .unwrap()
            .unwrap();
        assert_eq!(shared.token_id(), record.token_id());
        assert_eq!(shared.digest(), record.digest());
        assert_eq!(shared.person_id(), record.person_id());
        assert!((shared.issued_at() - record.issued_at()).abs() < 1e-6);
        assert!((shared.expires_at() - record.expires_at()).abs() < 1e-6);
        assert_eq!(shared.revoked_at(), record.revoked_at());
        assert!(issuer.revoke_access_token(record.token_id()).unwrap());
        assert!(
            verifier
                .find_access_token(record.digest())
                .unwrap()
                .is_none()
        );
        assert!(!verifier.revoke_access_token(record.token_id()).unwrap());
        assert!(verifier.revoke_access_token("not-a-uuid").is_err());
    }

    // The file Registry's own layout: what other tools find on disk.

    #[test]
    fn station_check_ins_follow_the_registry_record_contract() {
        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        let mut info = BTreeMap::new();
        info.insert("station_count".to_owned(), serde_json::json!(3));
        info.insert("station".to_owned(), serde_json::json!("forged"));

        registry.check_in("Stätion/one", info).unwrap();

        let path = root.path().join("stations/Stätion_one.json");
        let stored: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(stored["station"], "Stätion/one");
        assert_eq!(stored["station_count"], 3);
        assert!(stored["seen"].as_f64().unwrap() > 0.0);
        assert!(stored.get("info").is_none());
    }

    #[test]
    fn site_registry_records_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<StationRecord>();
        assert_send_sync::<HeadquartersLease>();
    }

    #[test]
    fn site_listing_skips_stale_and_malformed_records() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("stations");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("stale.json"),
            r#"{"station":"stale","seen":0}"#,
        )
        .unwrap();
        fs::write(directory.join("malformed.json"), b"not json").unwrap();
        let registry = FileRegistry::open(root.path());
        registry.check_in("fresh", BTreeMap::new()).unwrap();

        let stations = registry.stations(DEFAULT_LEASE).unwrap();

        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].station(), "fresh");
    }

    #[test]
    fn an_expired_headquarters_lease_is_filed_before_the_next_election() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("headquarters");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("lease.json"),
            r#"{"station":"old-station","expires":0}"#,
        )
        .unwrap();
        let registry = FileRegistry::open(root.path());

        assert!(registry.elect("new/station", DEFAULT_LEASE).unwrap());

        let lapsed: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("lease.lapsed.new_station")).unwrap())
                .unwrap();
        assert_eq!(lapsed["station"], "old-station");
        assert_eq!(
            registry.headquarters().unwrap().unwrap().station(),
            "new/station"
        );
    }

    #[test]
    fn a_headquarters_lease_written_before_terms_reads_as_term_zero() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("headquarters");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("lease.json"),
            r#"{"station":"old-station","expires":0}"#,
        )
        .unwrap();
        let registry = FileRegistry::open(root.path());
        assert_eq!(registry.headquarters().unwrap().unwrap().term(), 0);
        assert!(registry.elect("station-a", DEFAULT_LEASE).unwrap());
        assert_eq!(registry.headquarters().unwrap().unwrap().term(), 1);
    }

    #[test]
    fn brief_key_and_prompt_follow_the_registry_contract() {
        let brief: Brief = serde_json::from_str(
            r#"{"title":"  Do it  ","body":"  carefully ","id":"A / 42","source":"help desk","url":"https://example.test"}"#,
        )
        .unwrap();
        assert_eq!(brief.key(), "help_desk--A_42");
        assert_eq!(
            brief.prompt(),
            "Do it\n\ncarefully\n\nSource: https://example.test"
        );
    }

    #[test]
    fn lists_valid_new_briefs_in_registry_order_and_skips_bad_files() {
        let root = tempfile::tempdir().unwrap();
        let queue = root.path().join("new");
        fs::create_dir(&queue).unwrap();
        fs::write(queue.join("invalid.json"), b"not json").unwrap();
        let low = r#"{"title":"low","id":"low","priority":2,"created":"2026-01-01"}"#;
        let high_late = r#"{"title":"high late","id":"late","priority":9,"created":"2026-02-01"}"#;
        let high_early =
            r#"{"title":"high early","id":"early","priority":9,"created":"2026-01-01"}"#;
        fs::write(queue.join("a.json"), low).unwrap();
        fs::write(queue.join("b.json"), high_late).unwrap();
        fs::write(queue.join("c.json"), high_early).unwrap();
        let briefs = FileRegistry::open(root.path()).list_new().unwrap();
        assert_eq!(
            briefs
                .iter()
                .map(|brief| brief.title.as_str())
                .collect::<Vec<_>>(),
            ["high early", "high late", "low"]
        );
    }

    #[test]
    fn leased_claim_follows_the_registry_layout() {
        let root = tempfile::tempdir().unwrap();
        let queue = root.path().join("new");
        fs::create_dir(&queue).unwrap();
        fs::write(
            queue.join("manual--brief-1.json"),
            r#"{"title":"Inspect","id":"brief-1"}"#,
        )
        .unwrap();
        let registry = FileRegistry::open(root.path());
        let mut meta = BTreeMap::new();
        meta.insert("operation".to_owned(), serde_json::json!("operation-01"));

        registry
            .claim_with_lease(None, "operation-01", DEFAULT_LEASE, meta)
            .unwrap()
            .unwrap();
        assert!(!queue.join("manual--brief-1.json").exists());
        assert!(root.path().join("claimed/manual--brief-1.json").is_file());
        assert_eq!(
            fs::read_to_string(root.path().join("attempts/manual--brief-1.json")).unwrap(),
            "1"
        );
        let stored: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("leases/manual--brief-1.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored["station"], "operation-01");
        assert_eq!(stored["attempt"], 1);
        assert_eq!(stored["meta"]["operation"], "operation-01");
        assert!(stored.get("key").is_none());
    }

    #[test]
    fn finish_files_the_outcome_and_transcript_beside_the_brief() {
        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        registry.submit_brief(&brief("brief-1")).unwrap();
        let claimed = registry.claim_new(None).unwrap().unwrap();
        let outcome = serde_json::json!({"state": "done", "mission_id": "mission-1"});
        assert!(
            registry
                .finish(&claimed, &outcome, true, Some(b"output"), None)
                .unwrap()
        );
        let target = root.path().join("done/manual--brief-1.json");
        assert!(target.is_file());
        assert_eq!(fs::read(target.with_extension("log")).unwrap(), b"output");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &fs::read(target.with_file_name("manual--brief-1.outcome.json")).unwrap()
            )
            .unwrap(),
            outcome
        );
    }

    #[test]
    fn rejects_unknown_fields_and_newer_versions() {
        assert!(serde_json::from_str::<Brief>(r#"{"title":"x","typo":1}"#).is_err());
        let brief: Brief = serde_json::from_str(r#"{"title":"x","version":2}"#).unwrap();
        assert_eq!(
            brief.validate().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn cable_quotas_share_the_registry_format() {
        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        let buckets = [("sender:007", 1.0, 2), ("scope:operation:G", 1.0, 1)];
        assert!(registry.take_cable_quotas(&buckets, 100.0).unwrap());

        let saved: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("cable-quotas.json")).unwrap())
                .unwrap();
        assert!(saved.get("sender:007").is_none());
        assert!(
            saved
                .as_object()
                .unwrap()
                .keys()
                .all(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
        );
        assert!(
            saved
                .get("917f1172ca6099e51a0029f95620987bfda15f5a2b08ea43cd6e6e4cacf69e57")
                .is_some()
        );
        assert!(
            saved
                .get("096648f6cda52c6919328146fde0d5e058d725a4d88f32c9e6a5249ccb503e3f")
                .is_some()
        );
    }

    #[test]
    fn access_tokens_are_stored_by_digest() {
        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        let record = access_token(unix_time().unwrap());
        registry.store_access_token(&record).unwrap();
        let stored: serde_json::Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join(format!("access-tokens/{}.json", record.token_id())),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(stored["digest"], record.digest());
        assert_eq!(
            fs::read_to_string(
                root.path()
                    .join(format!("access-token-digests/{}.txt", record.digest()))
            )
            .unwrap()
            .trim(),
            record.token_id()
        );
    }
}
