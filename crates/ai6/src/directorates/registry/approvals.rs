//! Green ink records: an Agent's request to take one irreversible step, the decision on it,
//! and class authorisations (standing approvals of a kind of step).
//!
//! Every Station and Universal Exports read the same records, under the Registry lock:
//! `approvals/pending/OPERATION/ID.json` until a decision, then
//! `approvals/decided/OPERATION/ID.json`; `approvals/missions/OPERATION/MISSION.json` lists a
//! Mission's request IDs; `class-authorizations/OPERATION/ID.json` holds standing approvals.
//! Records are trusted like every other Registry record: the signer is whoever the writer
//! authenticated, and nothing is signed cryptographically.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use globset::Glob;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::store::{
    FileRegistry, hex_digest, invalid_data, registry_lock, validate_registry_key, write_atomic,
};
use crate::directorates::m::approvals::{ActionScope, clearance_at_least, valid_kind};
use crate::directorates::m::identity::{MissionId, SessionId};

/// The signer of a request that expired without a decision.
pub const EXPIRY_SIGNER: &str = "expiry";

pub(super) const MAX_RECORD_BYTES: u64 = 16 * 1024;
pub(super) const MAX_PENDING: usize = 1_000;
pub(super) const MAX_CLASS_AUTHORIZATIONS: usize = 1_000;
pub(super) const MAX_MISSION_REQUESTS: usize = 64;
const MAX_REASON_BYTES: usize = 2_048;
const MAX_NOTE_BYTES: usize = 1_024;
const MAX_SIGNER_BYTES: usize = 256;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;
const MAX_CLASS_ENTRIES: usize = 64;

/// An Agent's request to take one step that needs green ink.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequest {
    /// `green-ink-` and the SHA-256 of the Operation, Session, scope and retry key.
    pub id: String,
    /// The Operation the Mission belongs to.
    pub operation: String,
    /// The Mission whose Agent asked.
    pub mission: MissionId,
    /// The provider Session that asked, bound to its authenticated tool endpoint.
    pub session: SessionId,
    /// The Agent codename, supplied by AI6 rather than the request text.
    pub agent: String,
    /// The step.
    pub scope: ActionScope,
    /// Why the Agent asks, in its own words; shown to the signer, never audited.
    pub reason: String,
    /// When it asked, as Unix seconds.
    pub requested_at: f64,
    /// When the request expires as refused, as Unix seconds.
    pub expires_at: f64,
}

impl ApprovalRequest {
    /// A request with an identity derived from the Session, the scope and a retry key, so
    /// asking again with the same key finds the same request. Its times are set by
    /// [`ApprovalRequest::expiring`].
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an Operation or Agent that is not a Registry key, an empty or
    /// oversized reason or retry key, or a scope out of bounds.
    pub fn try_new(
        operation: impl Into<String>,
        mission: MissionId,
        session: SessionId,
        agent: impl Into<String>,
        scope: ActionScope,
        reason: impl Into<String>,
        idempotency_key: &str,
    ) -> io::Result<Self> {
        let operation = operation.into();
        if idempotency_key.trim().is_empty()
            || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES
            || idempotency_key.contains(char::is_control)
        {
            return Err(invalid_input(
                "the idempotency key must be 1 to 256 bytes without control characters",
            ));
        }
        let mut digest = Sha256::new();
        for component in [
            operation.as_bytes(),
            session.as_str().as_bytes(),
            scope.kind.as_bytes(),
            scope.target.as_bytes(),
            idempotency_key.as_bytes(),
        ] {
            digest.update((component.len() as u64).to_be_bytes());
            digest.update(component);
        }
        let request = Self {
            id: format!("green-ink-{}", hex_digest(&digest.finalize())),
            operation,
            mission,
            session,
            agent: agent.into(),
            scope,
            reason: reason.into(),
            requested_at: 0.0,
            expires_at: 0.0,
        };
        request.validate_fields()?;
        Ok(request)
    }

    /// Sets when the request was made and how long it waits for a decision.
    #[must_use]
    pub fn expiring(mut self, requested_at: f64, ttl: Duration) -> Self {
        self.requested_at = requested_at;
        self.expires_at = requested_at + ttl.as_secs_f64();
        self
    }

    fn validate_fields(&self) -> io::Result<()> {
        validate_registry_key(&self.id)?;
        validate_registry_key(&self.operation)?;
        validate_registry_key(&self.agent)?;
        self.scope.validate()?;
        if !valid_text(&self.reason, MAX_REASON_BYTES, true) {
            return Err(invalid_input(
                "the reason must be 1 to 2048 bytes without control characters other than newlines",
            ));
        }
        Ok(())
    }

    pub(super) fn validate(&self) -> io::Result<()> {
        self.validate_fields()?;
        if !(self.requested_at.is_finite()
            && self.requested_at > 0.0
            && self.expires_at.is_finite()
            && self.expires_at > self.requested_at)
        {
            return Err(invalid_input(
                "a request needs a positive time and an expiry after it",
            ));
        }
        Ok(())
    }
}

/// What became of a request.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// A person or a class authorisation approved the step.
    Approved,
    /// A person refused it.
    Rejected,
    /// No one decided before the request expired; it counts as refused.
    Expired,
}

impl Decision {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }
}

/// Who withdrew an approval or class authorisation, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revocation {
    /// The authenticated person.
    pub by: String,
    /// When, as Unix seconds.
    pub at: f64,
}

/// A decision on one request.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    /// The request decided.
    pub request: ApprovalRequest,
    /// The outcome.
    pub decision: Decision,
    /// The authenticated person who signed, `class:ID` for a class authorisation, or
    /// [`EXPIRY_SIGNER`].
    pub signer: String,
    /// When it was decided, as Unix seconds.
    pub at: f64,
    /// When an approval stops covering the step, as Unix seconds; unset otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<f64>,
    /// What the signer told the Agent, such as why a request was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Set when an approval was withdrawn before it expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked: Option<Revocation>,
    /// When the Station told the Agent, as Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_at: Option<f64>,
    /// When a green ink credential was leased against it, as Unix seconds; an approval unlocks
    /// one lease, on any Station, ever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent_at: Option<f64>,
}

impl Approval {
    /// Whether the approval covers its step at `now`: approved, not revoked, not expired.
    pub fn is_live(&self, now: f64) -> bool {
        self.decision == Decision::Approved
            && self.revoked.is_none()
            && self.expires_at.is_some_and(|expires| expires > now)
    }
}

/// A person's decision, as handed to [`ApprovalStore::decide_approval`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct Ruling {
    /// Approved or rejected.
    pub decision: Decision,
    /// The authenticated person.
    pub signer: String,
    /// When, as Unix seconds.
    pub at: f64,
    /// When an approval stops covering the step.
    pub expires_at: Option<f64>,
    /// What to tell the Agent.
    pub note: Option<String>,
}

impl Ruling {
    /// Approves until `expires_at`.
    pub fn approve(signer: impl Into<String>, at: f64, expires_at: f64) -> Self {
        Self {
            decision: Decision::Approved,
            signer: signer.into(),
            at,
            expires_at: Some(expires_at),
            note: None,
        }
    }

    /// Refuses, optionally saying why.
    pub fn reject(signer: impl Into<String>, at: f64, note: Option<String>) -> Self {
        Self {
            decision: Decision::Rejected,
            signer: signer.into(),
            at,
            expires_at: None,
            note,
        }
    }

    /// Adds a note for the Agent.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// A request waiting for a decision, or its decision.
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalStatus {
    /// No decision yet.
    Pending(ApprovalRequest),
    /// Decided, or expired.
    Decided(Approval),
}

impl ApprovalStatus {
    /// The request either way.
    pub fn request(&self) -> &ApprovalRequest {
        match self {
            Self::Pending(request) => request,
            Self::Decided(approval) => &approval.request,
        }
    }

    /// `pending`, or the decision's spelling.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Pending(_) => "pending",
            Self::Decided(approval) => approval.decision.as_str(),
        }
    }
}

/// A standing approval of a kind of step: matching requests are approved without asking, until
/// it expires, runs out of uses or is revoked.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassAuthorization {
    /// `class-` and a random identity.
    pub id: String,
    /// The Operation it applies to.
    pub operation: String,
    /// The step kinds it covers.
    pub kinds: Vec<String>,
    /// A glob the request's target must match, such as `origin:refs/heads/ai6/*`.
    pub target_pattern: String,
    /// Agent codenames it covers; `*` is every Agent.
    #[serde(default)]
    pub agents: Vec<String>,
    /// Roles it covers: a Mission for one of them is covered whatever its Agent.
    #[serde(default)]
    pub roles: Vec<String>,
    /// The lowest Agent clearance it covers.
    pub clearance_min: String,
    /// How many requests it may approve; unlimited when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
    /// How many it has approved.
    #[serde(default)]
    pub uses: u32,
    /// When it stops applying, as Unix seconds.
    pub expires_at: f64,
    /// The authenticated person who granted it.
    pub signer: String,
    /// When it was granted, as Unix seconds.
    pub created_at: f64,
    /// Set when it was withdrawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked: Option<Revocation>,
}

impl ClassAuthorization {
    /// A class authorisation of `kinds` on targets matching `target_pattern`, covering no
    /// Agent until [`ClassAuthorization::for_agents`] or [`ClassAuthorization::for_roles`]
    /// names some, for clearance `field` and above.
    pub fn new(
        operation: impl Into<String>,
        kinds: Vec<String>,
        target_pattern: impl Into<String>,
        signer: impl Into<String>,
        created_at: f64,
        expires_at: f64,
    ) -> Self {
        Self {
            id: format!("class-{}", uuid::Uuid::new_v4().simple()),
            operation: operation.into(),
            kinds,
            target_pattern: target_pattern.into(),
            agents: Vec::new(),
            roles: Vec::new(),
            clearance_min: "field".to_owned(),
            max_uses: None,
            uses: 0,
            expires_at,
            signer: signer.into(),
            created_at,
            revoked: None,
        }
    }

    /// The Agents it covers; `*` covers every Agent.
    #[must_use]
    pub fn for_agents(mut self, agents: Vec<String>) -> Self {
        self.agents = agents;
        self
    }

    /// The roles it covers.
    #[must_use]
    pub fn for_roles(mut self, roles: Vec<String>) -> Self {
        self.roles = roles;
        self
    }

    /// The lowest clearance it covers.
    #[must_use]
    pub fn with_clearance_min(mut self, clearance: impl Into<String>) -> Self {
        self.clearance_min = clearance.into();
        self
    }

    /// How many requests it may approve.
    #[must_use]
    pub fn with_max_uses(mut self, uses: u32) -> Self {
        self.max_uses = Some(uses);
        self
    }

    /// Whether it still approves: not revoked, not expired, uses left.
    pub fn is_live(&self, now: f64) -> bool {
        self.revoked.is_none()
            && self.expires_at > now
            && self.max_uses.is_none_or(|max| self.uses < max)
    }

    /// Whether it covers a request from `agent`, on a Mission for `role`, at `clearance`.
    pub fn covers(
        &self,
        agent: &str,
        role: Option<&str>,
        clearance: &str,
        scope: &ActionScope,
    ) -> bool {
        let named = |listed: &String| listed == "*" || listed == agent;
        let who = self.agents.iter().any(named)
            || role.is_some_and(|role| self.roles.iter().any(|listed| listed == role));
        who && clearance_at_least(clearance, &self.clearance_min)
            && self.kinds.contains(&scope.kind)
            && Glob::new(&self.target_pattern)
                .is_ok_and(|glob| glob.compile_matcher().is_match(&scope.target))
    }

    /// Checks the bounds of a record before it is stored.
    ///
    /// # Errors
    ///
    /// `InvalidInput` naming what is wrong: no kinds or an invalid one, a pattern that is not
    /// a glob, neither Agents nor roles, an unknown clearance, a non-positive use limit, or
    /// an expiry not after the grant.
    pub fn validate(&self) -> io::Result<()> {
        validate_registry_key(&self.id)?;
        validate_registry_key(&self.operation)?;
        if self.kinds.is_empty()
            || self.kinds.len() > MAX_CLASS_ENTRIES
            || !self.kinds.iter().all(|kind| valid_kind(kind))
        {
            return Err(invalid_input(
                "a class authorisation names 1 to 64 valid step kinds",
            ));
        }
        if !valid_text(&self.target_pattern, 256, false) || Glob::new(&self.target_pattern).is_err()
        {
            return Err(invalid_input(
                "the target pattern must be a glob of 1 to 256 bytes",
            ));
        }
        let names = self.agents.iter().chain(&self.roles);
        if (self.agents.is_empty() && self.roles.is_empty())
            || self.agents.len() > MAX_CLASS_ENTRIES
            || self.roles.len() > MAX_CLASS_ENTRIES
            || names.clone().any(|name| !valid_text(name, 128, false))
        {
            return Err(invalid_input(
                "a class authorisation covers 1 to 64 Agents or roles",
            ));
        }
        if !clearance_at_least(&self.clearance_min, "desk") {
            return Err(invalid_input("clearance_min must be desk, field or 00"));
        }
        if self.max_uses == Some(0) {
            return Err(invalid_input("max_uses must be at least 1"));
        }
        if !valid_text(&self.signer, MAX_SIGNER_BYTES, false) {
            return Err(invalid_input("the signer must be 1 to 256 bytes"));
        }
        if !(self.created_at.is_finite()
            && self.created_at > 0.0
            && self.expires_at.is_finite()
            && self.expires_at > self.created_at)
        {
            return Err(invalid_input(
                "a class authorisation needs a positive grant time and an expiry after it",
            ));
        }
        Ok(())
    }
}

impl FileRegistry {
    fn approvals_dir(&self, state: &str, operation: &str) -> PathBuf {
        self.root.join("approvals").join(state).join(operation)
    }

    fn mission_index(&self, operation: &str, mission: &str) -> PathBuf {
        self.approvals_dir("missions", operation)
            .join(format!("{mission}.json"))
    }

    fn class_dir(&self, operation: &str) -> PathBuf {
        self.root.join("class-authorizations").join(operation)
    }

    fn class_authorizations_locked(&self, operation: &str) -> io::Result<Vec<ClassAuthorization>> {
        let mut records = Vec::new();
        for path in json_files(&self.class_dir(operation))? {
            if let Ok(Some(record)) = read_json::<ClassAuthorization>(&path)
                && record.operation == operation
            {
                records.push(record);
            }
        }
        records.sort_by(|left, right| {
            left.created_at
                .total_cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(records)
    }

    /// The record of `id`, expiring a pending request at `now`; `now` of zero never expires.
    fn status_locked(
        &self,
        operation: &str,
        id: &str,
        now: f64,
    ) -> io::Result<Option<ApprovalStatus>> {
        let file = format!("{id}.json");
        if let Some(approval) =
            read_json::<Approval>(&self.approvals_dir("decided", operation).join(&file))?
        {
            return Ok(Some(ApprovalStatus::Decided(approval)));
        }
        let Some(request) =
            read_json::<ApprovalRequest>(&self.approvals_dir("pending", operation).join(&file))?
        else {
            return Ok(None);
        };
        if now > 0.0 && request.expires_at <= now {
            return self
                .expire_locked(&request)
                .map(|approval| Some(ApprovalStatus::Decided(approval)));
        }
        Ok(Some(ApprovalStatus::Pending(request)))
    }

    fn expire_locked(&self, request: &ApprovalRequest) -> io::Result<Approval> {
        let approval = expired(request);
        self.settle_locked(&approval)?;
        Ok(approval)
    }

    /// Writes the decision, then removes the pending request.
    fn settle_locked(&self, approval: &Approval) -> io::Result<()> {
        let operation = &approval.request.operation;
        let file = format!("{}.json", approval.request.id);
        write_json(
            &self.approvals_dir("decided", operation).join(&file),
            approval,
        )?;
        match fs::remove_file(self.approvals_dir("pending", operation).join(&file)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn index_locked(&self, request: &ApprovalRequest) -> io::Result<()> {
        let path = self.mission_index(&request.operation, request.mission.as_str());
        let mut ids = read_json::<Vec<String>>(&path)?.unwrap_or_default();
        if ids.contains(&request.id) {
            return Ok(());
        }
        if ids.len() >= MAX_MISSION_REQUESTS {
            return Err(invalid_data(format!(
                "the Mission has made {MAX_MISSION_REQUESTS} green ink requests"
            )));
        }
        ids.push(request.id.clone());
        write_json(&path, &ids)
    }
}

/// Green ink: approval requests, decisions and class authorisations.
pub trait ApprovalStore {
    /// Stores a request for green ink. Asking again with the same identity returns the
    /// record as it stands, pending or decided.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a request out of bounds or without times; `InvalidData` when the
    /// Operation already has 1,000 pending requests or the Mission 64 requests; I/O errors
    /// otherwise.
    fn request_approval(&self, request: &ApprovalRequest) -> io::Result<ApprovalStatus>;

    /// One request and its decision, if any; a pending request past its expiry is recorded
    /// as expired first.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an identity that is not a Registry key; I/O errors otherwise.
    fn approval(&self, operation: &str, id: &str, now: f64) -> io::Result<Option<ApprovalStatus>>;

    /// The Operation's requests waiting for a decision, oldest first. Requests past their
    /// expiry are recorded as expired and left out.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an Operation that is not a Registry key; I/O errors otherwise.
    /// Unreadable request files are skipped.
    fn pending_approvals(&self, operation: &str, now: f64) -> io::Result<Vec<ApprovalRequest>>;

    /// Every request a Mission's Agents made in the Operation, with its decision, in the order
    /// they were made. Requests past their expiry are recorded as expired first.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an identity that is not a Registry key; I/O errors otherwise.
    fn mission_approvals(
        &self,
        operation: &str,
        mission: &str,
        now: f64,
    ) -> io::Result<Vec<ApprovalStatus>>;

    /// The live approval that covers `scope` for a Mission, if one exists
    /// ([`ActionScope::covers`]).
    ///
    /// # Errors
    ///
    /// As [`ApprovalStore::mission_approvals`].
    fn live_approval(
        &self,
        operation: &str,
        mission: &str,
        scope: &ActionScope,
        now: f64,
    ) -> io::Result<Option<Approval>>;

    /// Decides a pending request. Repeating the same decision returns the record.
    ///
    /// # Errors
    ///
    /// `NotFound` for an unknown request; `AlreadyExists` when it was decided otherwise or
    /// expired before `ruling.at`; `InvalidInput` for an approval without an expiry after
    /// `ruling.at`, a ruling that is not approve or reject, or an oversized note or signer.
    fn decide_approval(&self, operation: &str, id: &str, ruling: &Ruling) -> io::Result<Approval>;

    /// Withdraws an approval before it expires. Revoking again returns the record.
    ///
    /// # Errors
    ///
    /// `NotFound` for an unknown or undecided request; `AlreadyExists` when it was not
    /// approved; I/O errors otherwise.
    fn revoke_approval(&self, operation: &str, id: &str, by: &str, at: f64)
    -> io::Result<Approval>;

    /// Records that the Agent was told of a decision.
    ///
    /// # Errors
    ///
    /// `NotFound` for a request without a decision; I/O errors otherwise.
    fn mark_approval_notified(&self, operation: &str, id: &str, at: f64) -> io::Result<()>;

    /// Records that a green ink credential was leased against an approval. `true` the first
    /// time; `false` when it was already spent, or is not a live approval at `at`, so a Station
    /// that loses the race gives its lease back.
    ///
    /// # Errors
    ///
    /// `NotFound` for a request without a decision; I/O errors otherwise.
    fn spend_approval(&self, operation: &str, id: &str, at: f64) -> io::Result<bool>;

    /// Approves a new request with the first live class authorisation that covers it, counting
    /// one use. `None` when none covers it, or when the request is already stored.
    ///
    /// `role` is the Mission's role and `clearance` the Agent's. The approval lasts as long as
    /// the class authorisation and counts as already told to the Agent.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a request out of bounds; I/O errors otherwise.
    fn approve_by_class(
        &self,
        request: &ApprovalRequest,
        role: Option<&str>,
        clearance: &str,
        now: f64,
    ) -> io::Result<Option<Approval>>;

    /// Stores a new class authorisation.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a record out of bounds; `AlreadyExists` when its identity is taken;
    /// `InvalidData` when the Operation already holds 1,000; I/O errors otherwise.
    fn put_class_authorization(&self, record: &ClassAuthorization) -> io::Result<()>;

    /// The Operation's class authorisations, oldest first, revoked and expired ones included.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an Operation that is not a Registry key; I/O errors otherwise.
    fn class_authorizations(&self, operation: &str) -> io::Result<Vec<ClassAuthorization>>;

    /// Withdraws a class authorisation. Revoking again returns the record.
    ///
    /// # Errors
    ///
    /// `NotFound` for an unknown identity; I/O errors otherwise.
    fn revoke_class_authorization(
        &self,
        operation: &str,
        id: &str,
        by: &str,
        at: f64,
    ) -> io::Result<ClassAuthorization>;
}

impl ApprovalStore for FileRegistry {
    fn request_approval(&self, request: &ApprovalRequest) -> io::Result<ApprovalStatus> {
        request.validate()?;
        let _lock = registry_lock(&self.root)?;
        if let Some(status) = self.status_locked(&request.operation, &request.id, 0.0)? {
            return Ok(status);
        }
        let pending = self.approvals_dir("pending", &request.operation);
        if count_json(&pending)? >= MAX_PENDING {
            return Err(invalid_data(format!(
                "the Operation has {MAX_PENDING} requests waiting for green ink"
            )));
        }
        self.index_locked(request)?;
        write_json(&pending.join(format!("{}.json", request.id)), request)?;
        Ok(ApprovalStatus::Pending(request.clone()))
    }

    fn approval(&self, operation: &str, id: &str, now: f64) -> io::Result<Option<ApprovalStatus>> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        let _lock = registry_lock(&self.root)?;
        self.status_locked(operation, id, now)
    }

    fn pending_approvals(&self, operation: &str, now: f64) -> io::Result<Vec<ApprovalRequest>> {
        validate_registry_key(operation)?;
        let _lock = registry_lock(&self.root)?;
        let mut waiting = Vec::new();
        for path in json_files(&self.approvals_dir("pending", operation))? {
            let Ok(Some(request)) = read_json::<ApprovalRequest>(&path) else {
                continue;
            };
            if request.operation != operation {
                continue;
            }
            if request.expires_at <= now {
                self.expire_locked(&request)?;
            } else {
                waiting.push(request);
            }
        }
        waiting.sort_by(|left, right| {
            left.requested_at
                .total_cmp(&right.requested_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(waiting)
    }

    fn mission_approvals(
        &self,
        operation: &str,
        mission: &str,
        now: f64,
    ) -> io::Result<Vec<ApprovalStatus>> {
        validate_registry_key(operation)?;
        validate_registry_key(mission)?;
        let _lock = registry_lock(&self.root)?;
        let ids =
            read_json::<Vec<String>>(&self.mission_index(operation, mission))?.unwrap_or_default();
        let mut statuses = Vec::new();
        for id in ids {
            if validate_registry_key(&id).is_ok()
                && let Some(status) = self.status_locked(operation, &id, now)?
            {
                statuses.push(status);
            }
        }
        Ok(statuses)
    }

    fn live_approval(
        &self,
        operation: &str,
        mission: &str,
        scope: &ActionScope,
        now: f64,
    ) -> io::Result<Option<Approval>> {
        Ok(self
            .mission_approvals(operation, mission, now)?
            .into_iter()
            .find_map(|status| match status {
                ApprovalStatus::Decided(approval)
                    if approval.request.scope.covers(scope) && approval.is_live(now) =>
                {
                    Some(approval)
                }
                _ => None,
            }))
    }

    fn decide_approval(&self, operation: &str, id: &str, ruling: &Ruling) -> io::Result<Approval> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        validate_ruling(ruling)?;
        let _lock = registry_lock(&self.root)?;
        match self.status_locked(operation, id, ruling.at)? {
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no such green ink request",
            )),
            Some(ApprovalStatus::Decided(approval)) if approval.decision == ruling.decision => {
                Ok(approval)
            }
            Some(ApprovalStatus::Decided(approval)) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("the request was already {}", approval.decision.as_str()),
            )),
            Some(ApprovalStatus::Pending(request)) => {
                let approval = ruled(request, ruling);
                self.settle_locked(&approval)?;
                Ok(approval)
            }
        }
    }

    fn revoke_approval(
        &self,
        operation: &str,
        id: &str,
        by: &str,
        at: f64,
    ) -> io::Result<Approval> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        let _lock = registry_lock(&self.root)?;
        let path = self
            .approvals_dir("decided", operation)
            .join(format!("{id}.json"));
        let mut approval = read_json::<Approval>(&path)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no decided green ink request")
        })?;
        if approval.decision != Decision::Approved {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "only an approval can be revoked",
            ));
        }
        if approval.revoked.is_none() {
            approval.revoked = Some(Revocation {
                by: by.to_owned(),
                at,
            });
            write_json(&path, &approval)?;
        }
        Ok(approval)
    }

    fn spend_approval(&self, operation: &str, id: &str, at: f64) -> io::Result<bool> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        let _lock = registry_lock(&self.root)?;
        let path = self
            .approvals_dir("decided", operation)
            .join(format!("{id}.json"));
        let mut approval = read_json::<Approval>(&path)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no decided green ink request")
        })?;
        if approval.spent_at.is_some() || !approval.is_live(at) {
            return Ok(false);
        }
        approval.spent_at = Some(at);
        write_json(&path, &approval)?;
        Ok(true)
    }

    fn mark_approval_notified(&self, operation: &str, id: &str, at: f64) -> io::Result<()> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        let _lock = registry_lock(&self.root)?;
        let path = self
            .approvals_dir("decided", operation)
            .join(format!("{id}.json"));
        let mut approval = read_json::<Approval>(&path)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no decided green ink request")
        })?;
        if approval.notified_at.is_none() {
            approval.notified_at = Some(at);
            write_json(&path, &approval)?;
        }
        Ok(())
    }

    fn approve_by_class(
        &self,
        request: &ApprovalRequest,
        role: Option<&str>,
        clearance: &str,
        now: f64,
    ) -> io::Result<Option<Approval>> {
        request.validate()?;
        let _lock = registry_lock(&self.root)?;
        if self
            .status_locked(&request.operation, &request.id, 0.0)?
            .is_some()
        {
            return Ok(None);
        }
        let Some(mut class) = self
            .class_authorizations_locked(&request.operation)?
            .into_iter()
            .find(|class| {
                class.is_live(now) && class.covers(&request.agent, role, clearance, &request.scope)
            })
        else {
            return Ok(None);
        };
        class.uses += 1;
        write_json(
            &self
                .class_dir(&request.operation)
                .join(format!("{}.json", class.id)),
            &class,
        )?;
        let approval = class_approval(request, &class, now);
        self.index_locked(request)?;
        write_json(
            &self
                .approvals_dir("decided", &request.operation)
                .join(format!("{}.json", request.id)),
            &approval,
        )?;
        Ok(Some(approval))
    }

    fn put_class_authorization(&self, record: &ClassAuthorization) -> io::Result<()> {
        record.validate()?;
        let _lock = registry_lock(&self.root)?;
        let directory = self.class_dir(&record.operation);
        let path = directory.join(format!("{}.json", record.id));
        if path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the class authorisation identity is taken",
            ));
        }
        if count_json(&directory)? >= MAX_CLASS_AUTHORIZATIONS {
            return Err(invalid_data(format!(
                "the Operation holds {MAX_CLASS_AUTHORIZATIONS} class authorisations"
            )));
        }
        write_json(&path, record)
    }

    fn class_authorizations(&self, operation: &str) -> io::Result<Vec<ClassAuthorization>> {
        validate_registry_key(operation)?;
        self.class_authorizations_locked(operation)
    }

    fn revoke_class_authorization(
        &self,
        operation: &str,
        id: &str,
        by: &str,
        at: f64,
    ) -> io::Result<ClassAuthorization> {
        validate_registry_key(operation)?;
        validate_registry_key(id)?;
        let _lock = registry_lock(&self.root)?;
        let path = self.class_dir(operation).join(format!("{id}.json"));
        let mut record = read_json::<ClassAuthorization>(&path)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no such class authorisation")
        })?;
        if record.revoked.is_none() {
            record.revoked = Some(Revocation {
                by: by.to_owned(),
                at,
            });
            write_json(&path, &record)?;
        }
        Ok(record)
    }
}

/// The record of a request no one decided before it expired: refused, signed by
/// [`EXPIRY_SIGNER`] at its expiry.
pub(super) fn expired(request: &ApprovalRequest) -> Approval {
    Approval {
        request: request.clone(),
        decision: Decision::Expired,
        signer: EXPIRY_SIGNER.to_owned(),
        at: request.expires_at,
        expires_at: None,
        note: None,
        revoked: None,
        notified_at: None,
        spent_at: None,
    }
}

/// The record of a person's ruling on a pending request.
pub(super) fn ruled(request: ApprovalRequest, ruling: &Ruling) -> Approval {
    Approval {
        request,
        decision: ruling.decision,
        signer: ruling.signer.clone(),
        at: ruling.at,
        expires_at: ruling.expires_at,
        note: ruling.note.clone(),
        revoked: None,
        notified_at: None,
        spent_at: None,
    }
}

/// The approval a class authorisation gives a request at `now`: it lasts as long as the
/// class and counts as already told to the Agent.
pub(super) fn class_approval(
    request: &ApprovalRequest,
    class: &ClassAuthorization,
    now: f64,
) -> Approval {
    Approval {
        request: request.clone(),
        decision: Decision::Approved,
        signer: format!("class:{}", class.id),
        at: now,
        expires_at: Some(class.expires_at),
        note: None,
        revoked: None,
        notified_at: Some(now),
        spent_at: None,
    }
}

pub(super) fn validate_ruling(ruling: &Ruling) -> io::Result<()> {
    if !valid_text(&ruling.signer, MAX_SIGNER_BYTES, false) {
        return Err(invalid_input("the signer must be 1 to 256 bytes"));
    }
    if !ruling.at.is_finite() || ruling.at <= 0.0 {
        return Err(invalid_input("a decision needs a positive time"));
    }
    if ruling
        .note
        .as_deref()
        .is_some_and(|note| !valid_text(note, MAX_NOTE_BYTES, true))
    {
        return Err(invalid_input(
            "the note must be 1 to 1024 bytes without control characters other than newlines",
        ));
    }
    match (ruling.decision, ruling.expires_at) {
        (Decision::Approved, Some(expires)) if expires.is_finite() && expires > ruling.at => Ok(()),
        (Decision::Approved, _) => Err(invalid_input("an approval needs an expiry after it")),
        (Decision::Rejected, None) => Ok(()),
        (Decision::Rejected, Some(_)) => Err(invalid_input("a refusal has no expiry")),
        (Decision::Expired, _) => Err(invalid_input("expiry is recorded by the Registry")),
    }
}

fn valid_text(value: &str, max_bytes: usize, multiline: bool) -> bool {
    !value.trim().is_empty()
        && value.len() <= max_bytes
        && !value
            .chars()
            .any(|character| character.is_control() && !(multiline && "\n\t".contains(character)))
}

fn invalid_input(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn json_files(directory: &Path) -> io::Result<Vec<PathBuf>> {
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
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn count_json(directory: &Path) -> io::Result<usize> {
    json_files(directory).map(|paths| paths.len())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Option<T>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid_data("green ink record exceeds the size limit"));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| invalid_data(format!("invalid green ink record: {error}")))
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid_data("green ink record exceeds the size limit"));
    }
    write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        an_approval_is_spent_once_and_only_while_live,
        stations_spending_one_approval_at_once_have_one_winner,
        a_request_is_approved_and_then_found_by_its_scope,
        expired_and_revoked_approvals_do_not_match,
        an_undecided_request_expires_as_refused,
        a_class_authorisation_matches_by_kind_glob_and_agent_and_stops_at_max_uses,
        a_class_authorisation_covers_roles_and_stops_when_revoked_or_expired,
        requests_are_bounded,
    );

    const NOW: f64 = 1_800_000_000.0;
    const HOUR: Duration = Duration::from_secs(3600);

    fn request(key: &str, kind: &str, target: &str) -> ApprovalRequest {
        ApprovalRequest::try_new(
            "G",
            MissionId::try_new("mission-1").unwrap(),
            SessionId::try_new("provider-session-1").unwrap(),
            "007",
            ActionScope::try_new(kind, target).unwrap(),
            "the release is ready",
            key,
        )
        .unwrap()
        .expiring(NOW, HOUR)
    }

    fn an_approval_is_spent_once_and_only_while_live(store: &dyn Store) {
        let registry = store.handle();
        let asked = request("release-1", "release", "tag v1");
        registry.request_approval(&asked).unwrap();
        assert!(
            registry.spend_approval("G", &asked.id, NOW).is_err(),
            "an undecided request cannot be spent"
        );
        registry
            .decide_approval("G", &asked.id, &Ruling::approve("m", NOW, NOW + 60.0))
            .unwrap();
        assert!(
            !registry.spend_approval("G", &asked.id, NOW + 61.0).unwrap(),
            "expired"
        );
        assert!(registry.spend_approval("G", &asked.id, NOW + 1.0).unwrap());
        assert!(
            !registry.spend_approval("G", &asked.id, NOW + 2.0).unwrap(),
            "the second Station to spend it loses"
        );
        // Another handle on the same Registry, as after a restart, sees the use.
        let reopened = store.handle();
        let Some(ApprovalStatus::Decided(approval)) =
            reopened.approval("G", &asked.id, NOW + 3.0).unwrap()
        else {
            panic!("the request is decided");
        };
        assert_eq!(approval.spent_at, Some(NOW + 1.0));
        assert!(!reopened.spend_approval("G", &asked.id, NOW + 3.0).unwrap());
    }

    fn stations_spending_one_approval_at_once_have_one_winner(store: &dyn Store) {
        let registry = store.handle();
        let asked = request("release-2", "release", "tag v2");
        registry.request_approval(&asked).unwrap();
        registry
            .decide_approval("G", &asked.id, &Ruling::approve("m", NOW, NOW + 60.0))
            .unwrap();
        let spenders = (0..4)
            .map(|_| {
                let station = store.handle();
                let id = asked.id.clone();
                std::thread::spawn(move || station.spend_approval("G", &id, NOW + 1.0).unwrap())
            })
            .collect::<Vec<_>>();
        let won = spenders
            .into_iter()
            .map(|spender| spender.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(won, 1);
    }

    fn a_request_is_approved_and_then_found_by_its_scope(store: &dyn Store) {
        let registry = store.handle();
        let asked = request("push-1", "push", "origin:refs/heads/main");
        let status = registry.request_approval(&asked).unwrap();
        assert_eq!(status, ApprovalStatus::Pending(asked.clone()));
        assert_eq!(
            registry.request_approval(&asked).unwrap().name(),
            "pending",
            "asking again finds the same request"
        );
        assert_eq!(
            registry.pending_approvals("G", NOW).unwrap(),
            std::slice::from_ref(&asked)
        );
        let scope = asked.scope.clone();
        assert!(
            registry
                .live_approval("G", "mission-1", &scope, NOW)
                .unwrap()
                .is_none()
        );

        let approved = registry
            .decide_approval(
                "G",
                &asked.id,
                &Ruling::approve("operator-m", NOW + 1.0, NOW + 600.0),
            )
            .unwrap();
        assert_eq!(approved.signer, "operator-m");
        assert!(
            registry
                .pending_approvals("G", NOW + 2.0)
                .unwrap()
                .is_empty()
        );
        let live = registry
            .live_approval("G", "mission-1", &scope, NOW + 2.0)
            .unwrap()
            .expect("the approval covers its scope");
        assert_eq!(live.request.id, asked.id);
        let other = ActionScope::try_new("push", "origin:refs/heads/develop").unwrap();
        assert!(
            registry
                .live_approval("G", "mission-1", &other, NOW + 2.0)
                .unwrap()
                .is_none()
        );
        assert!(
            registry
                .live_approval("L", "mission-1", &scope, NOW + 2.0)
                .unwrap()
                .is_none()
        );

        // The same decision again is harmless; the other one is refused.
        registry
            .decide_approval(
                "G",
                &asked.id,
                &Ruling::approve("operator-m", NOW + 3.0, NOW + 600.0),
            )
            .unwrap();
        let refused = registry
            .decide_approval(
                "G",
                &asked.id,
                &Ruling::reject("operator-q", NOW + 3.0, None),
            )
            .unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            registry.request_approval(&asked).unwrap().name(),
            "approved",
            "asking again after the decision returns it"
        );
    }

    fn expired_and_revoked_approvals_do_not_match(store: &dyn Store) {
        let registry = store.handle();
        let asked = request("push-2", "push", "main");
        registry.request_approval(&asked).unwrap();
        registry
            .decide_approval(
                "G",
                &asked.id,
                &Ruling::approve("operator-m", NOW + 1.0, NOW + 60.0),
            )
            .unwrap();
        let scope = asked.scope.clone();
        assert!(
            registry
                .live_approval("G", "mission-1", &scope, NOW + 59.0)
                .unwrap()
                .is_some()
        );
        assert!(
            registry
                .live_approval("G", "mission-1", &scope, NOW + 60.0)
                .unwrap()
                .is_none(),
            "an approval past its expiry"
        );

        let again = request("push-3", "push", "main");
        registry.request_approval(&again).unwrap();
        registry
            .decide_approval(
                "G",
                &again.id,
                &Ruling::approve("operator-m", NOW + 1.0, NOW + 600.0),
            )
            .unwrap();
        let after_first = NOW + 61.0;
        assert_eq!(
            registry
                .live_approval("G", "mission-1", &scope, after_first)
                .unwrap()
                .map(|approval| approval.request.id),
            Some(again.id.clone())
        );
        let revoked = registry
            .revoke_approval("G", &again.id, "operator-q", after_first)
            .unwrap();
        assert_eq!(revoked.revoked.as_ref().unwrap().by, "operator-q");
        assert!(
            registry
                .live_approval("G", "mission-1", &scope, after_first + 1.0)
                .unwrap()
                .is_none()
        );
    }

    fn an_undecided_request_expires_as_refused(store: &dyn Store) {
        let registry = store.handle();
        let asked = request("delete-1", "delete", "origin:refs/heads/old");
        registry.request_approval(&asked).unwrap();
        let later = NOW + HOUR.as_secs_f64();
        assert!(registry.pending_approvals("G", later).unwrap().is_empty());
        let Some(ApprovalStatus::Decided(expired)) =
            registry.approval("G", &asked.id, later).unwrap()
        else {
            panic!("the request is decided by its expiry");
        };
        assert_eq!(
            (expired.decision, expired.signer.as_str()),
            (Decision::Expired, EXPIRY_SIGNER)
        );
        let late = registry
            .decide_approval(
                "G",
                &asked.id,
                &Ruling::approve("operator-m", later + 1.0, later + 60.0),
            )
            .unwrap_err();
        assert_eq!(late.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            registry
                .mission_approvals("G", "mission-1", later)
                .unwrap()
                .len(),
            1
        );
        registry
            .mark_approval_notified("G", &asked.id, later)
            .unwrap();
        let Some(ApprovalStatus::Decided(told)) = registry.approval("G", &asked.id, later).unwrap()
        else {
            panic!("still decided");
        };
        assert_eq!(told.notified_at, Some(later));
    }

    fn a_class_authorisation_matches_by_kind_glob_and_agent_and_stops_at_max_uses(
        store: &dyn Store,
    ) {
        let registry = store.handle();
        let class = ClassAuthorization::new(
            "G",
            vec!["push".to_owned()],
            "origin:refs/heads/ai6/*",
            "operator-m",
            NOW,
            NOW + 3600.0,
        )
        .for_agents(vec!["007".to_owned()])
        .with_max_uses(2);
        registry.put_class_authorization(&class).unwrap();

        let wrong_kind = request("merge-1", "merge", "origin:refs/heads/ai6/one");
        assert!(
            registry
                .approve_by_class(&wrong_kind, None, "field", NOW)
                .unwrap()
                .is_none()
        );
        let wrong_target = request("push-main", "push", "origin:refs/heads/main");
        assert!(
            registry
                .approve_by_class(&wrong_target, None, "field", NOW)
                .unwrap()
                .is_none()
        );
        let mut other_agent = request("push-008", "push", "origin:refs/heads/ai6/one");
        other_agent.agent = "008".to_owned();
        assert!(
            registry
                .approve_by_class(&other_agent, None, "field", NOW)
                .unwrap()
                .is_none()
        );
        let first = request("push-a", "push", "origin:refs/heads/ai6/one");
        assert!(
            registry
                .approve_by_class(&first, None, "desk", NOW)
                .unwrap()
                .is_none(),
            "below the class authorisation's clearance"
        );

        let approved = registry
            .approve_by_class(&first, None, "field", NOW)
            .unwrap()
            .expect("covered by the class authorisation");
        assert_eq!(approved.signer, format!("class:{}", class.id));
        assert!(approved.is_live(NOW + 1.0));
        assert!(
            registry
                .approve_by_class(&first, None, "field", NOW)
                .unwrap()
                .is_none(),
            "a request already stored is not counted twice"
        );
        let second = request("push-b", "push", "origin:refs/heads/ai6/two/deep");
        assert!(
            registry
                .approve_by_class(&second, None, "00", NOW)
                .unwrap()
                .is_some()
        );
        let third = request("push-c", "push", "origin:refs/heads/ai6/three");
        assert!(
            registry
                .approve_by_class(&third, None, "field", NOW)
                .unwrap()
                .is_none(),
            "max_uses is spent"
        );
        assert_eq!(registry.class_authorizations("G").unwrap()[0].uses, 2);
    }

    fn a_class_authorisation_covers_roles_and_stops_when_revoked_or_expired(store: &dyn Store) {
        let registry = store.handle();
        let class = ClassAuthorization::new(
            "G",
            vec!["merge".to_owned()],
            "PR *",
            "operator-m",
            NOW,
            NOW + 100.0,
        )
        .for_roles(vec!["reviewer".to_owned()]);
        registry.put_class_authorization(&class).unwrap();
        let asked = request("merge-1", "merge", "PR 42");
        assert!(
            registry
                .approve_by_class(&asked, None, "field", NOW)
                .unwrap()
                .is_none()
        );
        assert!(
            registry
                .approve_by_class(&asked, Some("reviewer"), "field", NOW + 100.0)
                .unwrap()
                .is_none(),
            "expired"
        );
        registry
            .revoke_class_authorization("G", &class.id, "operator-q", NOW + 1.0)
            .unwrap();
        assert!(
            registry
                .approve_by_class(&asked, Some("reviewer"), "field", NOW + 2.0)
                .unwrap()
                .is_none(),
            "revoked"
        );

        for bad in [
            ClassAuthorization::new("G", vec![], "*", "m", NOW, NOW + 1.0)
                .for_agents(vec!["*".to_owned()]),
            ClassAuthorization::new("G", vec!["push".to_owned()], "*", "m", NOW, NOW + 1.0),
            ClassAuthorization::new("G", vec!["push".to_owned()], "[", "m", NOW, NOW + 1.0)
                .for_agents(vec!["*".to_owned()]),
            ClassAuthorization::new("G", vec!["push".to_owned()], "*", "m", NOW, NOW)
                .for_agents(vec!["*".to_owned()]),
            ClassAuthorization::new("G", vec!["push".to_owned()], "*", "m", NOW, NOW + 1.0)
                .for_agents(vec!["*".to_owned()])
                .with_clearance_min("root"),
        ] {
            assert_eq!(
                registry.put_class_authorization(&bad).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    fn requests_are_bounded(store: &dyn Store) {
        let mission = MissionId::try_new("mission-1").unwrap();
        let session = SessionId::try_new("provider-session-1").unwrap();
        let scope = ActionScope::try_new("push", "main").unwrap();
        let attempt = |agent: &str, reason: &str, key: &str| {
            ApprovalRequest::try_new(
                "G",
                mission.clone(),
                session.clone(),
                agent,
                scope.clone(),
                reason,
                key,
            )
        };
        assert!(attempt("007", "line one\nline two", "key").is_ok());
        assert!(attempt("007", " ", "key").is_err());
        assert!(attempt("007", "reason", "").is_err());
        assert!(attempt("../007", "reason", "key").is_err());
        assert!(attempt("007", &"x".repeat(MAX_REASON_BYTES + 1), "key").is_err());
        let registry = store.handle();
        let untimed = attempt("007", "reason", "key").unwrap();
        assert_eq!(
            registry.request_approval(&untimed).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
