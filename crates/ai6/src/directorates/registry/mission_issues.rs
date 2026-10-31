//! Linked issues and the outbox: the Registry's record of which tracker issues a Mission is
//! tied to, and every write AI6 makes to a tracker.
//!
//! A write is an [`OutboxEntry`] first. Any Station and Universal Exports may queue one;
//! only the Headquarters Station sends them, because only it holds tracker tokens. An entry
//! moves `pending` → `sending` (stamped with the sender's election term, its fence) →
//! `posted`, `refused` or `failed`, so a new Headquarters Station can tell what an old one
//! left half done.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::limits::IssueLimits;
use super::store::{FileRegistry, invalid_data, safe_component, validate_registry_key};

/// How a linked issue relates to its Mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkRelation {
    /// The issue the Mission started from.
    Source,
    /// An issue the Mission is about without being its source.
    Related,
    /// An issue created to split the Mission's work.
    Child,
    /// The issue a child was split from.
    Parent,
    /// A copy of the issue in another tracker.
    Mirror,
}

/// Whether a link is in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    /// Suggested and waiting for a person.
    Proposed,
    /// In force: the Mission may write to the issue.
    Active,
    /// Removed; kept as a record.
    Unlinked,
}

/// A tracker issue tied to a Mission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueLink {
    /// The Operation the Mission belongs to.
    pub operation: String,
    /// The Mission.
    pub mission_id: String,
    /// The tracker instance (the key under `sources`).
    pub tracker: String,
    /// The provider-local item ID.
    pub item: String,
    /// A link to the issue.
    #[serde(default)]
    pub url: Option<String>,
    /// How the issue relates to the Mission.
    pub relation: LinkRelation,
    /// Whether the link is in force.
    pub state: LinkState,
    /// Who linked it: `m`, `agent:<codename>` or `person:<subject>`.
    pub linked_by: String,
    /// When it was linked, Unix seconds.
    pub linked_at: f64,
    /// The comment AI6 keeps current on the issue.
    #[serde(default)]
    pub status_comment: Option<String>,
    /// The phase last written: `working`, `waiting`, `paused`, `done` or `failed`.
    #[serde(default)]
    pub phase: Option<String>,
    /// The issue's title as it was known when it was linked. Untrusted text.
    #[serde(default)]
    pub title: Option<String>,
}

impl IssueLink {
    /// `<tracker>:<item>`, the name a link is shown and addressed by.
    pub fn name(&self) -> String {
        format!("{}:{}", self.tracker, self.item)
    }

    fn same(&self, tracker: &str, item: &str) -> bool {
        self.tracker == tracker && self.item == item
    }
}

/// Where a question an Agent asked on an issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionState {
    /// Waiting for an answer.
    Open,
    /// A permitted reply came.
    Answered,
    /// Nobody answered in time.
    Expired,
    /// The Mission stopped waiting for another reason.
    Withdrawn,
}

/// A question an Agent asked on a linked issue, which blocks its Mission until answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueQuestion {
    /// The question's ID.
    pub id: String,
    /// The Mission that asked.
    pub mission_id: String,
    /// The tracker instance.
    pub tracker: String,
    /// The issue.
    pub item: String,
    /// The outbox entry that posts it.
    pub entry: String,
    /// When it was asked, Unix seconds.
    pub asked_at: f64,
    /// When it stops waiting, Unix seconds.
    pub expires_at: f64,
    /// Where it is.
    pub state: QuestionState,
}

/// Where a comment from a linked issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyState {
    /// Stored and waiting for the Station that holds the Mission to deliver it.
    Pending,
    /// The Agent was given it.
    Delivered,
    /// Not delivered: the Mission is not running.
    Ignored,
}

/// A comment from a permitted author on a linked issue. Untrusted text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueReply {
    /// The tracker's comment ID.
    pub id: String,
    /// The Mission whose link it came in on.
    pub mission_id: String,
    /// The tracker instance.
    pub tracker: String,
    /// The issue.
    pub item: String,
    /// The author's login or account ID.
    pub author_id: String,
    /// The author's display name.
    pub author: String,
    /// The comment text, cut to the limit.
    pub body: String,
    /// When it was made.
    pub created: String,
    /// Where it is.
    pub state: ReplyState,
}

/// Where a proposal to create issues is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    /// Waiting for a person (or a class authorisation).
    Pending,
    /// Approved; its creations are queued.
    Approved,
    /// Refused.
    Rejected,
    /// Every issue it asked for was created.
    Created,
}

/// One issue an Agent proposes to create. The text is the Agent's, already checked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposedIssue {
    /// The tracker instance to create it in.
    pub tracker: String,
    /// The title.
    pub title: String,
    /// The body as Markdown, with the cover footer.
    #[serde(default)]
    pub body: String,
    /// The labels asked for, intake labels already removed.
    #[serde(default)]
    pub labels: Vec<String>,
    /// The issue type, when the Agent asked for one.
    #[serde(default)]
    pub issue_type: Option<String>,
    /// The priority in the target's words, when the Agent asked for one.
    #[serde(default)]
    pub priority: Option<String>,
    /// The linked issue it is a child of, as `TRACKER:ITEM`.
    #[serde(default)]
    pub parent: Option<String>,
    /// The issue it copies, as `TRACKER:ITEM`.
    #[serde(default)]
    pub mirror_of: Option<String>,
    /// How many levels below an issue a person made it sits (a child of such an issue is 1).
    #[serde(default)]
    pub depth: u64,
}

/// An Agent's proposal to create issues, which a person approves unless the Operation lets
/// Agents create directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssueProposal {
    /// The proposal's ID; empty on a new one, filled when it is stored.
    #[serde(default)]
    pub id: String,
    /// The Operation the Mission belongs to.
    pub operation: String,
    /// The Mission that proposes.
    pub mission_id: String,
    /// Who proposes: `agent:<codename>`.
    pub proposer: String,
    /// The issues to create.
    pub specs: Vec<ProposedIssue>,
    /// The exposure sources of the proposing Session; an exposed one never auto-approves.
    #[serde(default)]
    pub exposure: Vec<String>,
    /// What the vetting found: `duplicate`, or anything that blocks a class authorisation.
    #[serde(default)]
    pub findings: Vec<String>,
    /// The fields the target tracker cannot keep, for a mirror.
    #[serde(default)]
    pub dropped: Vec<String>,
    /// Where it is.
    pub state: ProposalState,
    /// Who decided it: a person's subject, or `class:<ID>`.
    #[serde(default)]
    pub decided_by: Option<String>,
    /// Whether the approver asked for the intake labels to be added.
    #[serde(default)]
    pub mark_ready: bool,
    /// The items that were created, as `TRACKER:ITEM`.
    #[serde(default)]
    pub created: Vec<String>,
    /// The caller's retry key; proposing again with it returns the first proposal.
    #[serde(default)]
    pub retry_key: Option<String>,
    /// Whether the Agent was told how it came out.
    #[serde(default)]
    pub told: bool,
    /// When it was proposed, Unix seconds.
    pub at: f64,
}

/// What an outbox entry writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxKind {
    /// The Station's status comment, labels and transitions.
    Status,
    /// The Mission's outcome, as a new comment.
    Report,
    /// A comment an Agent or a person wrote.
    Comment,
    /// A question an Agent asked on the issue.
    Question,
    /// Labels, assignees, state or a transition (the payload is a `TrackerUpdate`).
    Update,
    /// A new issue (the payload is an `IssueSpec`).
    Create,
    /// A link between issues (the payload is a `TrackerLink`).
    Link,
}

/// Where an outbox entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxState {
    /// Waiting for the Headquarters Station.
    Pending,
    /// Held for a person.
    Held,
    /// The Headquarters Station is calling the tracker.
    Sending,
    /// Written.
    Posted,
    /// Not written, by policy or because its link was removed.
    Refused,
    /// Not written after the attempts allowed.
    Failed,
}

/// What a settled entry made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxResult {
    /// The comment's ID.
    #[serde(default)]
    pub comment_id: Option<String>,
    /// A link to the comment or created issue.
    #[serde(default)]
    pub url: Option<String>,
    /// The created issue's item ID.
    #[serde(default)]
    pub created_item: Option<String>,
    /// The fields a creation could not carry over, such as a mirror's assignees.
    #[serde(default)]
    pub dropped: Vec<String>,
}

/// One write to a tracker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboxEntry {
    /// The entry's ID; empty on a new entry, filled when it is queued.
    #[serde(default)]
    pub id: String,
    /// The Operation whose tracker instance is written.
    pub operation: String,
    /// The Mission the write belongs to.
    pub mission_id: String,
    /// The tracker instance.
    pub tracker: String,
    /// The issue; empty for a `create` that names no parent.
    #[serde(default)]
    pub item: String,
    /// What it writes.
    pub kind: OutboxKind,
    /// Who wrote it: `m`, `agent:<codename>` or `person:<subject>`.
    pub actor: String,
    /// The comment text, when it writes one.
    #[serde(default)]
    pub body: String,
    /// The typed request for an `update`, `create` or `link`, and the phase of a `status`.
    #[serde(default)]
    pub payload: serde_json::Value,
    /// A key under which a newer pending entry replaces an older one.
    #[serde(default)]
    pub coalesce: Option<String>,
    /// Where it is.
    pub state: OutboxState,
    /// Sends tried so far.
    #[serde(default)]
    pub attempts: u32,
    /// What it made, once posted.
    #[serde(default)]
    pub result: Option<OutboxResult>,
    /// Why it was refused or failed.
    #[serde(default)]
    pub error: Option<String>,
    /// When it was queued, Unix seconds.
    pub at: f64,
    /// Not sent before this time, Unix seconds.
    #[serde(default)]
    pub not_before: f64,
    /// When the current send began, Unix seconds.
    #[serde(default)]
    pub sent_at: Option<f64>,
    /// The election term of the Station that is sending it.
    #[serde(default)]
    pub fence: Option<u64>,
}

impl OutboxEntry {
    /// A pending entry with nothing set but who, where and what.
    pub fn new(
        operation: impl Into<String>,
        mission_id: impl Into<String>,
        tracker: impl Into<String>,
        item: impl Into<String>,
        kind: OutboxKind,
        actor: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            id: String::new(),
            operation: operation.into(),
            mission_id: mission_id.into(),
            tracker: tracker.into(),
            item: item.into(),
            kind,
            actor: actor.into(),
            body: String::new(),
            payload: serde_json::Value::Null,
            coalesce: None,
            state: OutboxState::Pending,
            attempts: 0,
            result: None,
            error: None,
            at,
            not_before: at,
            sent_at: None,
            fence: None,
        }
    }

    /// Whether the entry has reached an end state.
    pub fn settled(&self) -> bool {
        matches!(
            self.state,
            OutboxState::Posted | OutboxState::Refused | OutboxState::Failed
        )
    }
}

/// How a send ended.
#[derive(Debug, Clone, PartialEq)]
pub enum OutboxSettle {
    /// The tracker took it.
    Posted(OutboxResult),
    /// Policy or the link forbids it; never retried.
    Refused(String),
    /// It could not be sent; tried again after `not_before` while attempts remain.
    Retry {
        /// Why.
        error: String,
        /// The earliest next attempt, Unix seconds.
        not_before: f64,
        /// Attempts after which it fails for good.
        max_attempts: u32,
    },
}

/// Linked issues and the outbox.
pub trait MissionIssueStore {
    /// Links an issue to a Mission, or updates the link already there (an unlinked one
    /// becomes active again). The Mission holds at most `[registry.limits.issues]
    /// links_per_mission` links that are not unlinked.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid link, the limit, or Registry I/O failures.
    fn link_issue(&self, link: &IssueLink) -> io::Result<IssueLink>;

    /// Returns the Mission's links, unlinked ones included.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn issue_links(&self, mission_id: &str) -> io::Result<Vec<IssueLink>>;

    /// Returns every active link of every Mission.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn active_issue_links(&self) -> io::Result<Vec<IssueLink>>;

    /// Marks a link unlinked and refuses the entries still waiting to write to it.
    /// Returns whether a link was in force.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn unlink_issue(&self, mission_id: &str, tracker: &str, item: &str) -> io::Result<bool>;

    /// Records the comment and phase AI6 last wrote on a link.
    ///
    /// # Errors
    ///
    /// Returns an error when the link does not exist or the Registry cannot be written.
    fn set_issue_status(
        &self,
        mission_id: &str,
        tracker: &str,
        item: &str,
        comment: Option<&str>,
        phase: Option<&str>,
    ) -> io::Result<()>;

    /// Queues a write. An entry that names an item needs an active link on the Mission in
    /// the entry's Operation and tracker; one that names none (a `create`) does not. When
    /// `coalesce` matches a pending entry, that entry takes the new content and keeps its
    /// ID. Returns the stored entry.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry is invalid or its link is missing or not in force.
    fn queue_outbox(&self, entry: &OutboxEntry) -> io::Result<OutboxEntry>;

    /// Returns one entry.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn outbox_entry(&self, id: &str) -> io::Result<Option<OutboxEntry>>;

    /// Returns the entries of one Mission, or all of them, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn outbox_entries(&self, mission_id: Option<&str>) -> io::Result<Vec<OutboxEntry>>;

    /// Returns the pending entries due at `now`, oldest first, at most `limit`.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn due_outbox(&self, now: f64, limit: usize) -> io::Result<Vec<OutboxEntry>>;

    /// Moves a pending entry to `sending` under `term`. Returns it, or `None` when it is no
    /// longer pending.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn begin_outbox_send(&self, id: &str, term: u64, now: f64) -> io::Result<Option<OutboxEntry>>;

    /// Returns the entries left `sending`, whichever term sent them.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn sending_outbox(&self) -> io::Result<Vec<OutboxEntry>>;

    /// Re-stamps a `sending` entry with `term`, so the new Headquarters Station may settle
    /// what the old one left. Returns `None` when it is no longer sending.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn adopt_outbox_send(&self, id: &str, term: u64) -> io::Result<Option<OutboxEntry>>;

    /// Records a question an Agent asked. An item holds one open question at a time.
    ///
    /// # Errors
    ///
    /// Returns `AlreadyExists` while another question on the item is open.
    fn ask_issue_question(&self, question: &IssueQuestion) -> io::Result<IssueQuestion>;

    /// Returns the Mission's questions, newest last.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn issue_questions(&self, mission_id: &str) -> io::Result<Vec<IssueQuestion>>;

    /// Returns every open question of every Mission.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn open_issue_questions(&self) -> io::Result<Vec<IssueQuestion>>;

    /// Ends a question. Returns whether it was open.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn settle_issue_question(
        &self,
        mission_id: &str,
        id: &str,
        state: QuestionState,
    ) -> io::Result<bool>;

    /// Stores a comment from a permitted author. Returns `false` for one already stored (a
    /// comment is told once however often it is polled).
    ///
    /// # Errors
    ///
    /// Returns an error for an oversized reply, the Mission's limit, or I/O failures.
    fn store_issue_reply(&self, reply: &IssueReply) -> io::Result<bool>;

    /// Returns the Mission's stored replies, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn issue_replies(&self, mission_id: &str) -> io::Result<Vec<IssueReply>>;

    /// Returns every reply waiting to be delivered.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn pending_issue_replies(&self) -> io::Result<Vec<IssueReply>>;

    /// Records what became of a stored reply. Returns whether it was found.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn mark_issue_reply(
        &self,
        mission_id: &str,
        tracker: &str,
        item: &str,
        id: &str,
        state: ReplyState,
    ) -> io::Result<bool>;

    /// Where the last poll of an instance's replies stopped.
    ///
    /// # Errors
    ///
    /// Returns an error when the cursor exists but cannot be read.
    fn issue_cursor(&self, operation: &str, tracker: &str) -> io::Result<Option<String>>;

    /// Records where a poll of an instance's replies stopped.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn set_issue_cursor(&self, operation: &str, tracker: &str, cursor: &str) -> io::Result<()>;

    /// Settles a `held` entry once the green ink it waited for is decided: approved, it
    /// becomes pending; otherwise it is refused with `reason`. Returns `None` when it is no
    /// longer held.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn resolve_held_outbox(
        &self,
        id: &str,
        approved: bool,
        reason: &str,
    ) -> io::Result<Option<OutboxEntry>>;

    /// Stores a proposal to create issues and returns it with its ID.
    ///
    /// # Errors
    ///
    /// Returns an error for a proposal with no issues or an invalid one, or I/O failures.
    fn propose_issues(&self, proposal: &IssueProposal) -> io::Result<IssueProposal>;

    /// Returns one proposal.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn issue_proposal(&self, id: &str) -> io::Result<Option<IssueProposal>>;

    /// Returns the proposals of one Mission, or all of them, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn issue_proposals(&self, mission_id: Option<&str>) -> io::Result<Vec<IssueProposal>>;

    /// Decides a pending proposal. Returns it, or `None` when it is no longer pending.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn decide_issue_proposal(
        &self,
        id: &str,
        approved: bool,
        by: &str,
        mark_ready: bool,
    ) -> io::Result<Option<IssueProposal>>;

    /// Records the items an approved proposal made; with every one made it is `created`.
    ///
    /// # Errors
    ///
    /// Returns an error when the proposal is not approved or the Registry cannot be written.
    fn record_created_issues(&self, id: &str, items: &[String]) -> io::Result<IssueProposal>;

    /// Records that the Agent was told how its proposal came out. Returns whether it was not
    /// told before.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn mark_proposal_told(&self, id: &str) -> io::Result<bool>;

    /// Ends a send. Only the term that holds the entry may settle it.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry is not `sending` under `term` or the Registry cannot
    /// be written.
    fn settle_outbox(&self, id: &str, term: u64, settle: OutboxSettle) -> io::Result<OutboxEntry>;
}

/// The primitive reads and writes the shared logic runs on, inside one lock or transaction.
pub(super) trait IssueTables {
    fn links(&mut self, mission: &str) -> io::Result<Vec<IssueLink>>;
    fn all_links(&mut self) -> io::Result<Vec<IssueLink>>;
    fn put_links(&mut self, mission: &str, links: &[IssueLink]) -> io::Result<()>;
    fn outbox(&mut self) -> io::Result<Vec<OutboxEntry>>;
    fn put_entry(&mut self, entry: &OutboxEntry) -> io::Result<()>;
    fn drop_entry(&mut self, id: &str) -> io::Result<()>;
    fn questions(&mut self, mission: &str) -> io::Result<Vec<IssueQuestion>>;
    fn all_questions(&mut self) -> io::Result<Vec<IssueQuestion>>;
    fn put_questions(&mut self, mission: &str, questions: &[IssueQuestion]) -> io::Result<()>;
    fn replies(&mut self, mission: &str) -> io::Result<Vec<IssueReply>>;
    fn all_replies(&mut self) -> io::Result<Vec<IssueReply>>;
    fn put_replies(&mut self, mission: &str, replies: &[IssueReply]) -> io::Result<()>;
    fn proposals(&mut self) -> io::Result<Vec<IssueProposal>>;
    fn put_proposal(&mut self, proposal: &IssueProposal) -> io::Result<()>;
}

pub(super) fn propose_in(
    tables: &mut dyn IssueTables,
    proposal: &IssueProposal,
    limits: &IssueLimits,
) -> io::Result<IssueProposal> {
    validate_registry_key(&proposal.operation)?;
    key_ok(&proposal.mission_id, "Mission ID")?;
    key_ok(&proposal.proposer, "proposer")?;
    if proposal.specs.is_empty()
        || proposal
            .specs
            .iter()
            .any(|spec| spec.title.trim().is_empty())
    {
        return Err(invalid_data("a proposal names issues, each with a title"));
    }
    let bytes = serde_json::to_vec(proposal).map_or(0, |bytes| bytes.len());
    if !limits.entry_bytes.allows(bytes) {
        return Err(invalid_data("the proposal exceeds the size limit"));
    }
    let mut stored = proposal.clone();
    stored.id = format!("ip-{}", uuid::Uuid::new_v4().simple());
    stored.state = ProposalState::Pending;
    stored.decided_by = None;
    stored.created.clear();
    tables.put_proposal(&stored)?;
    Ok(stored)
}

pub(super) fn decide_proposal_in(
    tables: &mut dyn IssueTables,
    id: &str,
    approved: bool,
    by: &str,
    mark_ready: bool,
) -> io::Result<Option<IssueProposal>> {
    let Some(mut proposal) = tables.proposals()?.into_iter().find(|held| held.id == id) else {
        return Ok(None);
    };
    if proposal.state != ProposalState::Pending {
        return Ok(None);
    }
    proposal.state = if approved {
        ProposalState::Approved
    } else {
        ProposalState::Rejected
    };
    proposal.decided_by = Some(by.to_owned());
    proposal.mark_ready = approved && mark_ready;
    tables.put_proposal(&proposal)?;
    Ok(Some(proposal))
}

pub(super) fn mark_told_in(tables: &mut dyn IssueTables, id: &str) -> io::Result<bool> {
    let Some(mut proposal) = tables.proposals()?.into_iter().find(|held| held.id == id) else {
        return Ok(false);
    };
    if proposal.told {
        return Ok(false);
    }
    proposal.told = true;
    tables.put_proposal(&proposal)?;
    Ok(true)
}

pub(super) fn record_created_in(
    tables: &mut dyn IssueTables,
    id: &str,
    items: &[String],
) -> io::Result<IssueProposal> {
    let mut proposal = tables
        .proposals()?
        .into_iter()
        .find(|held| held.id == id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such proposal"))?;
    if !matches!(
        proposal.state,
        ProposalState::Approved | ProposalState::Created
    ) {
        return Err(invalid_data("only an approved proposal creates issues"));
    }
    for item in items {
        if !proposal.created.contains(item) {
            proposal.created.push(item.clone());
        }
    }
    if proposal.created.len() >= proposal.specs.len() {
        proposal.state = ProposalState::Created;
    }
    tables.put_proposal(&proposal)?;
    Ok(proposal)
}

pub(super) fn ask_in(
    tables: &mut dyn IssueTables,
    question: &IssueQuestion,
) -> io::Result<IssueQuestion> {
    key_ok(&question.mission_id, "Mission ID")?;
    key_ok(&question.tracker, "tracker")?;
    key_ok(&question.item, "item")?;
    let mut questions = tables.questions(&question.mission_id)?;
    if questions.iter().any(|held| {
        held.state == QuestionState::Open
            && held.tracker == question.tracker
            && held.item == question.item
    }) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the issue already has an open question",
        ));
    }
    let mut stored = question.clone();
    stored.state = QuestionState::Open;
    if stored.id.is_empty() {
        stored.id = format!("q-{}", uuid::Uuid::new_v4().simple());
    }
    questions.push(stored.clone());
    tables.put_questions(&question.mission_id, &questions)?;
    Ok(stored)
}

pub(super) fn settle_question_in(
    tables: &mut dyn IssueTables,
    mission: &str,
    id: &str,
    state: QuestionState,
) -> io::Result<bool> {
    let mut questions = tables.questions(mission)?;
    let Some(held) = questions
        .iter_mut()
        .find(|held| held.id == id && held.state == QuestionState::Open)
    else {
        return Ok(false);
    };
    held.state = state;
    tables.put_questions(mission, &questions)?;
    Ok(true)
}

pub(super) fn store_reply_in(
    tables: &mut dyn IssueTables,
    reply: &IssueReply,
    limits: &IssueLimits,
) -> io::Result<bool> {
    key_ok(&reply.mission_id, "Mission ID")?;
    key_ok(&reply.id, "comment ID")?;
    let mut replies = tables.replies(&reply.mission_id)?;
    if replies
        .iter()
        .any(|held| held.tracker == reply.tracker && held.item == reply.item && held.id == reply.id)
    {
        return Ok(false);
    }
    let mut stored = reply.clone();
    if let Some(most) = limits.reply_bytes.get() {
        let mut end = (most as usize).min(stored.body.len());
        while !stored.body.is_char_boundary(end) {
            end -= 1;
        }
        stored.body.truncate(end);
    }
    replies.push(stored);
    // The oldest delivered or ignored replies make room; a pending one never does.
    if let Some(most) = limits.replies_per_mission.get() {
        while replies.len() > most as usize {
            match replies
                .iter()
                .position(|held| held.state != ReplyState::Pending)
            {
                Some(index) => {
                    replies.remove(index);
                }
                None => {
                    return Err(invalid_data(
                        "the Mission holds too many undelivered replies",
                    ));
                }
            }
        }
    }
    tables.put_replies(&reply.mission_id, &replies)?;
    Ok(true)
}

pub(super) fn mark_reply_in(
    tables: &mut dyn IssueTables,
    mission: &str,
    tracker: &str,
    item: &str,
    id: &str,
    state: ReplyState,
) -> io::Result<bool> {
    let mut replies = tables.replies(mission)?;
    let Some(held) = replies
        .iter_mut()
        .find(|held| held.tracker == tracker && held.item == item && held.id == id)
    else {
        return Ok(false);
    };
    held.state = state;
    tables.put_replies(mission, &replies)?;
    Ok(true)
}

pub(super) fn resolve_held_in(
    tables: &mut dyn IssueTables,
    id: &str,
    approved: bool,
    reason: &str,
) -> io::Result<Option<OutboxEntry>> {
    let Some(mut entry) = tables.outbox()?.into_iter().find(|entry| entry.id == id) else {
        return Ok(None);
    };
    if entry.state != OutboxState::Held {
        return Ok(None);
    }
    if approved {
        entry.state = OutboxState::Pending;
    } else {
        entry.state = OutboxState::Refused;
        entry.error = Some(reason.to_owned());
    }
    tables.put_entry(&entry)?;
    Ok(Some(entry))
}

fn key_ok(value: &str, what: &str) -> io::Result<()> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(invalid_data(format!(
            "{what} is empty, too long or has control characters"
        )));
    }
    Ok(())
}

fn validate_link(link: &IssueLink) -> io::Result<()> {
    validate_registry_key(&link.operation)?;
    key_ok(&link.mission_id, "Mission ID")?;
    key_ok(&link.tracker, "tracker")?;
    key_ok(&link.item, "item")?;
    key_ok(&link.linked_by, "linked_by")
}

pub(super) fn link_issue_in(
    tables: &mut dyn IssueTables,
    link: &IssueLink,
    limits: &IssueLimits,
) -> io::Result<IssueLink> {
    validate_link(link)?;
    let mut links = tables.links(&link.mission_id)?;
    let stored = match links
        .iter_mut()
        .find(|held| held.same(&link.tracker, &link.item))
    {
        Some(held) => {
            if held.state == LinkState::Unlinked && link.state != LinkState::Unlinked {
                // Coming back into force counts against the limit again.
                let live = links
                    .iter()
                    .filter(|other| other.state != LinkState::Unlinked)
                    .count();
                if !limits.links_per_mission.allows(live + 1) {
                    return Err(invalid_data(format!(
                        "Mission has reached the {} linked issue limit",
                        limits.links_per_mission
                    )));
                }
            }
            let Some(held) = links
                .iter_mut()
                .find(|held| held.same(&link.tracker, &link.item))
            else {
                return Err(invalid_data("link vanished"));
            };
            held.operation = link.operation.clone();
            held.url = link.url.clone().or_else(|| held.url.take());
            held.relation = link.relation;
            held.state = link.state;
            held.linked_by = link.linked_by.clone();
            held.linked_at = link.linked_at;
            held.title = link.title.clone().or_else(|| held.title.take());
            held.clone()
        }
        None => {
            let live = links
                .iter()
                .filter(|other| other.state != LinkState::Unlinked)
                .count();
            if !limits.links_per_mission.allows(live + 1) {
                return Err(invalid_data(format!(
                    "Mission has reached the {} linked issue limit",
                    limits.links_per_mission
                )));
            }
            links.push(link.clone());
            link.clone()
        }
    };
    tables.put_links(&link.mission_id, &links)?;
    Ok(stored)
}

pub(super) fn unlink_in(
    tables: &mut dyn IssueTables,
    mission: &str,
    tracker: &str,
    item: &str,
) -> io::Result<bool> {
    let mut links = tables.links(mission)?;
    let Some(held) = links
        .iter_mut()
        .find(|held| held.same(tracker, item) && held.state != LinkState::Unlinked)
    else {
        return Ok(false);
    };
    held.state = LinkState::Unlinked;
    tables.put_links(mission, &links)?;
    for mut entry in tables.outbox()? {
        if entry.mission_id == mission
            && entry.tracker == tracker
            && entry.item == item
            && matches!(entry.state, OutboxState::Pending | OutboxState::Held)
        {
            entry.state = OutboxState::Refused;
            entry.error = Some("unlinked".to_owned());
            tables.put_entry(&entry)?;
        }
    }
    Ok(true)
}

pub(super) fn set_status_in(
    tables: &mut dyn IssueTables,
    mission: &str,
    tracker: &str,
    item: &str,
    comment: Option<&str>,
    phase: Option<&str>,
) -> io::Result<()> {
    let mut links = tables.links(mission)?;
    let held = links
        .iter_mut()
        .find(|held| held.same(tracker, item))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such linked issue"))?;
    if let Some(comment) = comment {
        held.status_comment = Some(comment.to_owned());
    }
    if let Some(phase) = phase {
        held.phase = Some(phase.to_owned());
    }
    tables.put_links(mission, &links)
}

pub(super) fn queue_in(
    tables: &mut dyn IssueTables,
    entry: &OutboxEntry,
    limits: &IssueLimits,
) -> io::Result<OutboxEntry> {
    validate_registry_key(&entry.operation)?;
    key_ok(&entry.mission_id, "Mission ID")?;
    key_ok(&entry.tracker, "tracker")?;
    key_ok(&entry.actor, "actor")?;
    if entry.state != OutboxState::Pending && entry.state != OutboxState::Held {
        return Err(invalid_data("a new outbox entry is pending or held"));
    }
    if !entry.item.is_empty() {
        let linked = tables.links(&entry.mission_id)?.into_iter().any(|link| {
            link.same(&entry.tracker, &entry.item)
                && link.state == LinkState::Active
                && link.operation == entry.operation
        });
        if !linked {
            return Err(invalid_data(
                "the Mission has no active link to that issue in that Operation",
            ));
        }
    } else if !matches!(entry.kind, OutboxKind::Create) {
        return Err(invalid_data("only a create may name no issue"));
    }
    let text_bytes = entry.body.len() + entry.payload.to_string().len();
    if !limits.entry_bytes.allows(text_bytes) {
        return Err(invalid_data("outbox entry exceeds the size limit"));
    }
    let existing = tables.outbox()?;
    if let Some(key) = &entry.coalesce
        && let Some(mut held) = existing
            .iter()
            .find(|held| held.state == OutboxState::Pending && held.coalesce.as_ref() == Some(key))
            .cloned()
    {
        held.kind = entry.kind;
        held.actor = entry.actor.clone();
        held.body = entry.body.clone();
        held.payload = entry.payload.clone();
        held.at = entry.at;
        held.not_before = entry.not_before;
        tables.put_entry(&held)?;
        return Ok(held);
    }
    let mut stored = entry.clone();
    stored.id = format!("ox-{}", uuid::Uuid::new_v4().simple());
    tables.put_entry(&stored)?;
    // Keep the history bounded: the oldest settled entries go first.
    let mut settled: Vec<&OutboxEntry> = existing.iter().filter(|held| held.settled()).collect();
    settled.sort_by(|a, b| a.at.total_cmp(&b.at));
    let total = existing.len() + 1;
    if let Some(most) = limits.outbox_records.get() {
        let excess = total.saturating_sub(most as usize);
        for old in settled.into_iter().take(excess) {
            tables.drop_entry(&old.id)?;
        }
    }
    Ok(stored)
}

pub(super) fn due_in(
    tables: &mut dyn IssueTables,
    now: f64,
    limit: usize,
) -> io::Result<Vec<OutboxEntry>> {
    let mut due: Vec<OutboxEntry> = tables
        .outbox()?
        .into_iter()
        .filter(|entry| entry.state == OutboxState::Pending && entry.not_before <= now)
        .collect();
    due.sort_by(|a, b| a.at.total_cmp(&b.at));
    due.truncate(limit);
    Ok(due)
}

pub(super) fn begin_in(
    tables: &mut dyn IssueTables,
    id: &str,
    term: u64,
    now: f64,
) -> io::Result<Option<OutboxEntry>> {
    let Some(mut entry) = tables.outbox()?.into_iter().find(|entry| entry.id == id) else {
        return Ok(None);
    };
    if entry.state != OutboxState::Pending {
        return Ok(None);
    }
    entry.state = OutboxState::Sending;
    entry.fence = Some(term);
    entry.sent_at = Some(now);
    tables.put_entry(&entry)?;
    Ok(Some(entry))
}

pub(super) fn adopt_in(
    tables: &mut dyn IssueTables,
    id: &str,
    term: u64,
) -> io::Result<Option<OutboxEntry>> {
    let Some(mut entry) = tables.outbox()?.into_iter().find(|entry| entry.id == id) else {
        return Ok(None);
    };
    if entry.state != OutboxState::Sending {
        return Ok(None);
    }
    entry.fence = Some(term);
    tables.put_entry(&entry)?;
    Ok(Some(entry))
}

pub(super) fn settle_in(
    tables: &mut dyn IssueTables,
    id: &str,
    term: u64,
    settle: OutboxSettle,
) -> io::Result<OutboxEntry> {
    let mut entry = tables
        .outbox()?
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such outbox entry"))?;
    if entry.state != OutboxState::Sending || entry.fence != Some(term) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the entry is not being sent under this term",
        ));
    }
    entry.fence = None;
    match settle {
        OutboxSettle::Posted(result) => {
            entry.state = OutboxState::Posted;
            entry.result = Some(result);
            entry.error = None;
        }
        OutboxSettle::Refused(reason) => {
            entry.state = OutboxState::Refused;
            entry.error = Some(reason);
        }
        OutboxSettle::Retry {
            error,
            not_before,
            max_attempts,
        } => {
            entry.attempts += 1;
            entry.error = Some(error);
            if entry.attempts >= max_attempts {
                entry.state = OutboxState::Failed;
            } else {
                entry.state = OutboxState::Pending;
                entry.not_before = not_before;
            }
        }
    }
    tables.put_entry(&entry)?;
    Ok(entry)
}

/// The file Registry's tables, used while its lock is held.
struct FileTables<'a>(&'a FileRegistry);

impl FileTables<'_> {
    fn links_path(&self, mission: &str) -> PathBuf {
        self.0
            .root()
            .join("mission-issues")
            .join(format!("{}.json", safe_component(mission)))
    }

    fn outbox_dir(&self) -> PathBuf {
        self.0.root().join("issue-outbox")
    }
}

impl IssueTables for FileTables<'_> {
    fn links(&mut self, mission: &str) -> io::Result<Vec<IssueLink>> {
        match fs::read(self.links_path(mission)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| invalid_data(e.to_string())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    fn all_links(&mut self) -> io::Result<Vec<IssueLink>> {
        let mut all = Vec::new();
        let entries = match fs::read_dir(self.0.root().join("mission-issues")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(all),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if let Ok(bytes) = fs::read(&path)
                && let Ok(links) = serde_json::from_slice::<Vec<IssueLink>>(&bytes)
            {
                all.extend(links);
            }
        }
        Ok(all)
    }

    fn put_links(&mut self, mission: &str, links: &[IssueLink]) -> io::Result<()> {
        fs::create_dir_all(self.0.root().join("mission-issues"))?;
        let bytes = serde_json::to_vec_pretty(links).map_err(io::Error::other)?;
        self.0.write_atomic(&self.links_path(mission), &bytes)
    }

    fn outbox(&mut self) -> io::Result<Vec<OutboxEntry>> {
        let mut all = Vec::new();
        let entries = match fs::read_dir(self.outbox_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(all),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if let Ok(bytes) = fs::read(&path)
                && let Ok(entry) = serde_json::from_slice::<OutboxEntry>(&bytes)
            {
                all.push(entry);
            }
        }
        all.sort_by(|a, b| a.at.total_cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
        Ok(all)
    }

    fn put_entry(&mut self, entry: &OutboxEntry) -> io::Result<()> {
        fs::create_dir_all(self.outbox_dir())?;
        let bytes = serde_json::to_vec_pretty(entry).map_err(io::Error::other)?;
        self.0.write_atomic(
            &self.outbox_dir().join(format!("{}.json", entry.id)),
            &bytes,
        )
    }

    fn drop_entry(&mut self, id: &str) -> io::Result<()> {
        match fs::remove_file(self.outbox_dir().join(format!("{id}.json"))) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn questions(&mut self, mission: &str) -> io::Result<Vec<IssueQuestion>> {
        read_list(&self.mission_file("issue-questions", mission))
    }

    fn all_questions(&mut self) -> io::Result<Vec<IssueQuestion>> {
        read_all(&self.0.root().join("issue-questions"))
    }

    fn put_questions(&mut self, mission: &str, questions: &[IssueQuestion]) -> io::Result<()> {
        self.write_list("issue-questions", mission, questions)
    }

    fn replies(&mut self, mission: &str) -> io::Result<Vec<IssueReply>> {
        read_list(&self.mission_file("issue-replies", mission))
    }

    fn all_replies(&mut self) -> io::Result<Vec<IssueReply>> {
        read_all(&self.0.root().join("issue-replies"))
    }

    fn put_replies(&mut self, mission: &str, replies: &[IssueReply]) -> io::Result<()> {
        self.write_list("issue-replies", mission, replies)
    }

    fn proposals(&mut self) -> io::Result<Vec<IssueProposal>> {
        let mut all: Vec<IssueProposal> = Vec::new();
        let directory = self.0.root().join("issue-proposals");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(all),
            Err(error) => return Err(error),
        };
        for entry in entries {
            if let Ok(bytes) = fs::read(entry?.path())
                && let Ok(proposal) = serde_json::from_slice::<IssueProposal>(&bytes)
            {
                all.push(proposal);
            }
        }
        all.sort_by(|a, b| a.at.total_cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
        Ok(all)
    }

    fn put_proposal(&mut self, proposal: &IssueProposal) -> io::Result<()> {
        let directory = self.0.root().join("issue-proposals");
        fs::create_dir_all(&directory)?;
        let bytes = serde_json::to_vec_pretty(proposal).map_err(io::Error::other)?;
        self.0
            .write_atomic(&directory.join(format!("{}.json", proposal.id)), &bytes)
    }
}

impl FileTables<'_> {
    fn mission_file(&self, directory: &str, mission: &str) -> PathBuf {
        self.0
            .root()
            .join(directory)
            .join(format!("{}.json", safe_component(mission)))
    }

    fn write_list<T: Serialize>(
        &self,
        directory: &str,
        mission: &str,
        list: &[T],
    ) -> io::Result<()> {
        fs::create_dir_all(self.0.root().join(directory))?;
        let bytes = serde_json::to_vec_pretty(list).map_err(io::Error::other)?;
        self.0
            .write_atomic(&self.mission_file(directory, mission), &bytes)
    }
}

fn read_list<T: for<'de> Deserialize<'de>>(path: &std::path::Path) -> io::Result<Vec<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| invalid_data(e.to_string())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn read_all<T: for<'de> Deserialize<'de>>(directory: &std::path::Path) -> io::Result<Vec<T>> {
    let mut all = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(all),
        Err(error) => return Err(error),
    };
    for entry in entries {
        if let Ok(list) = read_list::<T>(&entry?.path()) {
            all.extend(list);
        }
    }
    Ok(all)
}

impl MissionIssueStore for FileRegistry {
    fn link_issue(&self, link: &IssueLink) -> io::Result<IssueLink> {
        let _lock = self.lock()?;
        link_issue_in(&mut FileTables(self), link, &self.limits.issues)
    }

    fn issue_links(&self, mission_id: &str) -> io::Result<Vec<IssueLink>> {
        FileTables(self).links(mission_id)
    }

    fn active_issue_links(&self) -> io::Result<Vec<IssueLink>> {
        Ok(FileTables(self)
            .all_links()?
            .into_iter()
            .filter(|link| link.state == LinkState::Active)
            .collect())
    }

    fn unlink_issue(&self, mission_id: &str, tracker: &str, item: &str) -> io::Result<bool> {
        let _lock = self.lock()?;
        unlink_in(&mut FileTables(self), mission_id, tracker, item)
    }

    fn set_issue_status(
        &self,
        mission_id: &str,
        tracker: &str,
        item: &str,
        comment: Option<&str>,
        phase: Option<&str>,
    ) -> io::Result<()> {
        let _lock = self.lock()?;
        set_status_in(
            &mut FileTables(self),
            mission_id,
            tracker,
            item,
            comment,
            phase,
        )
    }

    fn queue_outbox(&self, entry: &OutboxEntry) -> io::Result<OutboxEntry> {
        let _lock = self.lock()?;
        queue_in(&mut FileTables(self), entry, &self.limits.issues)
    }

    fn outbox_entry(&self, id: &str) -> io::Result<Option<OutboxEntry>> {
        Ok(FileTables(self)
            .outbox()?
            .into_iter()
            .find(|entry| entry.id == id))
    }

    fn outbox_entries(&self, mission_id: Option<&str>) -> io::Result<Vec<OutboxEntry>> {
        Ok(FileTables(self)
            .outbox()?
            .into_iter()
            .filter(|entry| mission_id.is_none_or(|mission| entry.mission_id == mission))
            .collect())
    }

    fn due_outbox(&self, now: f64, limit: usize) -> io::Result<Vec<OutboxEntry>> {
        due_in(&mut FileTables(self), now, limit)
    }

    fn begin_outbox_send(&self, id: &str, term: u64, now: f64) -> io::Result<Option<OutboxEntry>> {
        let _lock = self.lock()?;
        begin_in(&mut FileTables(self), id, term, now)
    }

    fn sending_outbox(&self) -> io::Result<Vec<OutboxEntry>> {
        Ok(FileTables(self)
            .outbox()?
            .into_iter()
            .filter(|entry| entry.state == OutboxState::Sending)
            .collect())
    }

    fn adopt_outbox_send(&self, id: &str, term: u64) -> io::Result<Option<OutboxEntry>> {
        let _lock = self.lock()?;
        adopt_in(&mut FileTables(self), id, term)
    }

    fn settle_outbox(&self, id: &str, term: u64, settle: OutboxSettle) -> io::Result<OutboxEntry> {
        let _lock = self.lock()?;
        settle_in(&mut FileTables(self), id, term, settle)
    }

    fn propose_issues(&self, proposal: &IssueProposal) -> io::Result<IssueProposal> {
        let _lock = self.lock()?;
        propose_in(&mut FileTables(self), proposal, &self.limits.issues)
    }

    fn issue_proposal(&self, id: &str) -> io::Result<Option<IssueProposal>> {
        Ok(FileTables(self)
            .proposals()?
            .into_iter()
            .find(|proposal| proposal.id == id))
    }

    fn issue_proposals(&self, mission_id: Option<&str>) -> io::Result<Vec<IssueProposal>> {
        Ok(FileTables(self)
            .proposals()?
            .into_iter()
            .filter(|proposal| mission_id.is_none_or(|mission| proposal.mission_id == mission))
            .collect())
    }

    fn decide_issue_proposal(
        &self,
        id: &str,
        approved: bool,
        by: &str,
        mark_ready: bool,
    ) -> io::Result<Option<IssueProposal>> {
        let _lock = self.lock()?;
        decide_proposal_in(&mut FileTables(self), id, approved, by, mark_ready)
    }

    fn record_created_issues(&self, id: &str, items: &[String]) -> io::Result<IssueProposal> {
        let _lock = self.lock()?;
        record_created_in(&mut FileTables(self), id, items)
    }

    fn mark_proposal_told(&self, id: &str) -> io::Result<bool> {
        let _lock = self.lock()?;
        mark_told_in(&mut FileTables(self), id)
    }

    fn ask_issue_question(&self, question: &IssueQuestion) -> io::Result<IssueQuestion> {
        let _lock = self.lock()?;
        ask_in(&mut FileTables(self), question)
    }

    fn issue_questions(&self, mission_id: &str) -> io::Result<Vec<IssueQuestion>> {
        FileTables(self).questions(mission_id)
    }

    fn open_issue_questions(&self) -> io::Result<Vec<IssueQuestion>> {
        Ok(FileTables(self)
            .all_questions()?
            .into_iter()
            .filter(|question| question.state == QuestionState::Open)
            .collect())
    }

    fn settle_issue_question(
        &self,
        mission_id: &str,
        id: &str,
        state: QuestionState,
    ) -> io::Result<bool> {
        let _lock = self.lock()?;
        settle_question_in(&mut FileTables(self), mission_id, id, state)
    }

    fn store_issue_reply(&self, reply: &IssueReply) -> io::Result<bool> {
        let _lock = self.lock()?;
        store_reply_in(&mut FileTables(self), reply, &self.limits.issues)
    }

    fn issue_replies(&self, mission_id: &str) -> io::Result<Vec<IssueReply>> {
        FileTables(self).replies(mission_id)
    }

    fn pending_issue_replies(&self) -> io::Result<Vec<IssueReply>> {
        Ok(FileTables(self)
            .all_replies()?
            .into_iter()
            .filter(|reply| reply.state == ReplyState::Pending)
            .collect())
    }

    fn mark_issue_reply(
        &self,
        mission_id: &str,
        tracker: &str,
        item: &str,
        id: &str,
        state: ReplyState,
    ) -> io::Result<bool> {
        let _lock = self.lock()?;
        mark_reply_in(&mut FileTables(self), mission_id, tracker, item, id, state)
    }

    fn issue_cursor(&self, operation: &str, tracker: &str) -> io::Result<Option<String>> {
        match fs::read_to_string(self.issue_cursor_path(operation, tracker)) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn set_issue_cursor(&self, operation: &str, tracker: &str, cursor: &str) -> io::Result<()> {
        let _lock = self.lock()?;
        fs::create_dir_all(self.root().join("issue-cursors"))?;
        self.write_atomic(
            &self.issue_cursor_path(operation, tracker),
            cursor.as_bytes(),
        )
    }

    fn resolve_held_outbox(
        &self,
        id: &str,
        approved: bool,
        reason: &str,
    ) -> io::Result<Option<OutboxEntry>> {
        let _lock = self.lock()?;
        resolve_held_in(&mut FileTables(self), id, approved, reason)
    }
}

impl FileRegistry {
    fn issue_cursor_path(&self, operation: &str, tracker: &str) -> PathBuf {
        self.root().join("issue-cursors").join(format!(
            "{}--{}.txt",
            safe_component(operation),
            safe_component(tracker)
        ))
    }
}

/// Counts entries by state, for tests and the operator.
pub fn outbox_counts(entries: &[OutboxEntry]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for entry in entries {
        let name = serde_json::to_value(entry.state)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        *counts.entry(name).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_proposal_is_decided_once_and_is_created_when_every_issue_is_made,
        a_question_blocks_one_per_item_and_settles_once,
        replies_are_stored_once_delivered_once_and_bounded_by_the_limit,
        cursors_and_held_entries_follow_their_decisions,
        a_source_link_is_kept_per_mission_and_relinking_reactivates_it,
        links_are_bounded_per_mission,
        an_entry_needs_an_active_link_in_its_operation,
        a_coalesced_entry_replaces_the_pending_one_and_keeps_its_id,
        unlinking_refuses_the_entries_still_waiting,
        only_the_sending_term_may_settle_an_entry,
        a_new_term_adopts_what_the_old_one_left_sending,
        a_retry_waits_and_fails_after_the_attempts_allowed,
    );

    fn link(mission: &str, tracker: &str, item: &str) -> IssueLink {
        IssueLink {
            operation: "G".to_owned(),
            mission_id: mission.to_owned(),
            tracker: tracker.to_owned(),
            item: item.to_owned(),
            url: None,
            relation: LinkRelation::Source,
            state: LinkState::Active,
            linked_by: "m".to_owned(),
            linked_at: 1.0,
            status_comment: None,
            phase: None,
            title: None,
        }
    }

    fn entry(mission: &str, item: &str, kind: OutboxKind, at: f64) -> OutboxEntry {
        OutboxEntry::new("G", mission, "github", item, kind, "m", at)
    }

    fn proposal(mission: &str, titles: &[&str]) -> IssueProposal {
        IssueProposal {
            id: String::new(),
            operation: "G".to_owned(),
            mission_id: mission.to_owned(),
            proposer: "agent:007".to_owned(),
            specs: titles
                .iter()
                .map(|title| ProposedIssue {
                    tracker: "github".to_owned(),
                    title: (*title).to_owned(),
                    ..ProposedIssue::default()
                })
                .collect(),
            exposure: Vec::new(),
            findings: Vec::new(),
            dropped: Vec::new(),
            state: ProposalState::Pending,
            decided_by: None,
            mark_ready: false,
            created: Vec::new(),
            retry_key: None,
            told: false,
            at: 1.0,
        }
    }

    fn a_proposal_is_decided_once_and_is_created_when_every_issue_is_made(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.propose_issues(&proposal("m1", &[])).is_err());
        assert!(registry.propose_issues(&proposal("m1", &[" "])).is_err());
        let first = registry
            .propose_issues(&proposal("m1", &["a", "b"]))
            .unwrap();
        let second = registry.propose_issues(&proposal("m2", &["c"])).unwrap();
        assert!(first.id.starts_with("ip-"));
        assert_eq!(registry.issue_proposals(Some("m1")).unwrap().len(), 1);
        assert_eq!(registry.issue_proposals(None).unwrap().len(), 2);
        assert!(
            registry
                .record_created_issues(&first.id, &["github:o/r#1".to_owned()])
                .is_err(),
            "not approved yet"
        );
        let approved = registry
            .decide_issue_proposal(&first.id, true, "person:sam", true)
            .unwrap()
            .unwrap();
        assert_eq!(approved.state, ProposalState::Approved);
        assert!(approved.mark_ready);
        assert_eq!(approved.decided_by.as_deref(), Some("person:sam"));
        assert!(
            registry
                .decide_issue_proposal(&first.id, false, "person:kim", false)
                .unwrap()
                .is_none(),
            "decided once"
        );
        let rejected = registry
            .decide_issue_proposal(&second.id, false, "person:sam", true)
            .unwrap()
            .unwrap();
        assert_eq!(
            (rejected.state, rejected.mark_ready),
            (ProposalState::Rejected, false)
        );
        let part = registry
            .record_created_issues(&first.id, &["github:o/r#1".to_owned()])
            .unwrap();
        assert_eq!(part.state, ProposalState::Approved);
        let done = registry
            .record_created_issues(
                &first.id,
                &["github:o/r#2".to_owned(), "github:o/r#1".to_owned()],
            )
            .unwrap();
        assert_eq!(
            (done.state, done.created.len()),
            (ProposalState::Created, 2)
        );
    }

    fn question(mission: &str, item: &str) -> IssueQuestion {
        IssueQuestion {
            id: String::new(),
            mission_id: mission.to_owned(),
            tracker: "github".to_owned(),
            item: item.to_owned(),
            entry: "ox-1".to_owned(),
            asked_at: 1.0,
            expires_at: 9.0,
            state: QuestionState::Open,
        }
    }

    fn reply(mission: &str, id: &str, body: &str) -> IssueReply {
        IssueReply {
            id: id.to_owned(),
            mission_id: mission.to_owned(),
            tracker: "github".to_owned(),
            item: "o/r#1".to_owned(),
            author_id: "maint".to_owned(),
            author: "Maint".to_owned(),
            body: body.to_owned(),
            created: "t".to_owned(),
            state: ReplyState::Pending,
        }
    }

    fn a_question_blocks_one_per_item_and_settles_once(store: &dyn Store) {
        let registry = store.handle();
        let first = registry
            .ask_issue_question(&question("m1", "o/r#1"))
            .unwrap();
        assert!(first.id.starts_with("q-"));
        assert_eq!(
            registry
                .ask_issue_question(&question("m1", "o/r#1"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        registry
            .ask_issue_question(&question("m1", "o/r#2"))
            .unwrap();
        registry
            .ask_issue_question(&question("m2", "o/r#1"))
            .unwrap();
        assert_eq!(registry.open_issue_questions().unwrap().len(), 3);
        assert!(
            registry
                .settle_issue_question("m1", &first.id, QuestionState::Answered)
                .unwrap()
        );
        assert!(
            !registry
                .settle_issue_question("m1", &first.id, QuestionState::Expired)
                .unwrap(),
            "settled once"
        );
        let held = registry.issue_questions("m1").unwrap();
        assert_eq!(held[0].state, QuestionState::Answered);
        assert_eq!(registry.open_issue_questions().unwrap().len(), 2);
        // The answered item may be asked about again.
        registry
            .ask_issue_question(&question("m1", "o/r#1"))
            .unwrap();
    }

    fn replies_are_stored_once_delivered_once_and_bounded_by_the_limit(store: &dyn Store) {
        let registry = store.handle();
        assert!(
            registry
                .store_issue_reply(&reply("m1", "c1", "hello"))
                .unwrap()
        );
        assert!(
            !registry
                .store_issue_reply(&reply("m1", "c1", "hello"))
                .unwrap(),
            "a comment polled twice is stored once"
        );
        let long = "é".repeat(registry.limits().issues.reply_bytes.get().unwrap() as usize);
        registry
            .store_issue_reply(&reply("m1", "c2", &long))
            .unwrap();
        let stored = registry.issue_replies("m1").unwrap();
        assert!(
            stored[1].body.len() <= registry.limits().issues.reply_bytes.get().unwrap() as usize
        );
        assert_eq!(registry.pending_issue_replies().unwrap().len(), 2);
        assert!(
            registry
                .mark_issue_reply("m1", "github", "o/r#1", "c1", ReplyState::Delivered)
                .unwrap()
        );
        assert!(
            !registry
                .mark_issue_reply("m1", "github", "o/r#1", "nope", ReplyState::Ignored)
                .unwrap()
        );
        assert_eq!(registry.pending_issue_replies().unwrap().len(), 1);
    }

    fn cursors_and_held_entries_follow_their_decisions(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(registry.issue_cursor("G", "github").unwrap(), None);
        registry
            .set_issue_cursor("G", "github", "2026-10-05T10:00:00Z")
            .unwrap();
        assert_eq!(
            registry.issue_cursor("G", "github").unwrap().as_deref(),
            Some("2026-10-05T10:00:00Z")
        );
        assert_eq!(registry.issue_cursor("G", "jira").unwrap(), None);
        registry.link_issue(&link("m1", "github", "o/r#1")).unwrap();
        let mut held = entry("m1", "o/r#1", OutboxKind::Update, 1.0);
        held.state = OutboxState::Held;
        let held_one = registry.queue_outbox(&held).unwrap();
        let held_two = registry.queue_outbox(&held).unwrap();
        assert!(
            registry.due_outbox(9.0, 10).unwrap().is_empty(),
            "held waits"
        );
        let approved = registry
            .resolve_held_outbox(&held_one.id, true, "")
            .unwrap()
            .unwrap();
        assert_eq!(approved.state, OutboxState::Pending);
        let refused = registry
            .resolve_held_outbox(&held_two.id, false, "rejected")
            .unwrap()
            .unwrap();
        assert_eq!(
            (refused.state, refused.error.as_deref()),
            (OutboxState::Refused, Some("rejected"))
        );
        assert!(
            registry
                .resolve_held_outbox(&held_one.id, true, "")
                .unwrap()
                .is_none()
        );
        assert_eq!(registry.due_outbox(9.0, 10).unwrap().len(), 1);
    }

    fn a_source_link_is_kept_per_mission_and_relinking_reactivates_it(store: &dyn Store) {
        let registry = store.handle();
        registry.link_issue(&link("m1", "github", "o/r#1")).unwrap();
        registry.link_issue(&link("m1", "jira", "GUA-1")).unwrap();
        registry.link_issue(&link("m2", "github", "o/r#2")).unwrap();
        assert_eq!(registry.issue_links("m1").unwrap().len(), 2);
        assert_eq!(registry.active_issue_links().unwrap().len(), 3);
        registry
            .set_issue_status("m1", "github", "o/r#1", Some("77"), Some("working"))
            .unwrap();
        assert!(registry.unlink_issue("m1", "github", "o/r#1").unwrap());
        assert!(!registry.unlink_issue("m1", "github", "o/r#1").unwrap());
        assert_eq!(registry.active_issue_links().unwrap().len(), 2);
        let mut again = link("m1", "github", "o/r#1");
        again.relation = LinkRelation::Related;
        let stored = registry.link_issue(&again).unwrap();
        assert_eq!(stored.state, LinkState::Active);
        let kept = registry
            .issue_links("m1")
            .unwrap()
            .into_iter()
            .find(|held| held.tracker == "github")
            .unwrap();
        assert_eq!(
            kept.status_comment.as_deref(),
            Some("77"),
            "the comment stays"
        );
        assert_eq!(kept.relation, LinkRelation::Related);
        assert!(
            registry
                .set_issue_status("m1", "github", "nope", None, None)
                .is_err()
        );
    }

    fn links_are_bounded_per_mission(store: &dyn Store) {
        let registry = store.handle();
        let most = registry.limits().issues.links_per_mission.get().unwrap() as usize;
        for number in 0..most {
            registry
                .link_issue(&link("m1", "github", &format!("o/r#{number}")))
                .unwrap();
        }
        assert!(
            registry
                .link_issue(&link("m1", "github", "o/r#999"))
                .is_err()
        );
        registry.unlink_issue("m1", "github", "o/r#0").unwrap();
        registry
            .link_issue(&link("m1", "github", "o/r#999"))
            .unwrap();
        assert!(
            registry.link_issue(&link("m1", "github", "o/r#0")).is_err(),
            "relinking counts against the limit again"
        );
    }

    fn an_entry_needs_an_active_link_in_its_operation(store: &dyn Store) {
        let registry = store.handle();
        assert!(
            registry
                .queue_outbox(&entry("m1", "o/r#1", OutboxKind::Comment, 1.0))
                .is_err(),
            "no link"
        );
        registry.link_issue(&link("m1", "github", "o/r#1")).unwrap();
        let mut elsewhere = entry("m1", "o/r#1", OutboxKind::Comment, 1.0);
        elsewhere.operation = "H".to_owned();
        assert!(
            registry.queue_outbox(&elsewhere).is_err(),
            "another Operation"
        );
        let stored = registry
            .queue_outbox(&entry("m1", "o/r#1", OutboxKind::Comment, 1.0))
            .unwrap();
        assert!(stored.id.starts_with("ox-"));
        assert!(
            registry
                .queue_outbox(&entry("m1", "", OutboxKind::Comment, 1.0))
                .is_err(),
            "only a create names no issue"
        );
        registry
            .queue_outbox(&entry("m1", "", OutboxKind::Create, 1.0))
            .unwrap();
        assert_eq!(registry.outbox_entries(Some("m1")).unwrap().len(), 2);
        assert!(registry.outbox_entries(Some("m2")).unwrap().is_empty());
    }

    fn a_coalesced_entry_replaces_the_pending_one_and_keeps_its_id(store: &dyn Store) {
        let registry = store.handle();
        registry.link_issue(&link("m1", "github", "o/r#1")).unwrap();
        let mut first = entry("m1", "o/r#1", OutboxKind::Status, 1.0);
        first.coalesce = Some("status:m1:o/r#1".to_owned());
        first.body = "working".to_owned();
        let one = registry.queue_outbox(&first).unwrap();
        first.body = "waiting".to_owned();
        first.at = 2.0;
        let two = registry.queue_outbox(&first).unwrap();
        assert_eq!(one.id, two.id);
        let all = registry.outbox_entries(None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].body, "waiting");
        // Once it is being sent a new status is a new entry.
        registry
            .begin_outbox_send(&one.id, 1, 3.0)
            .unwrap()
            .unwrap();
        let three = registry.queue_outbox(&first).unwrap();
        assert_ne!(three.id, one.id);
    }

    fn unlinking_refuses_the_entries_still_waiting(store: &dyn Store) {
        let registry = store.handle();
        registry.link_issue(&link("m1", "github", "o/r#1")).unwrap();
        let waiting = registry
            .queue_outbox(&entry("m1", "o/r#1", OutboxKind::Comment, 1.0))
            .unwrap();
        let sending = registry
            .queue_outbox(&entry("m1", "o/r#1", OutboxKind::Comment, 2.0))
            .unwrap();
        registry
            .begin_outbox_send(&sending.id, 1, 3.0)
            .unwrap()
            .unwrap();
        registry.unlink_issue("m1", "github", "o/r#1").unwrap();
        let refused = registry.outbox_entry(&waiting.id).unwrap().unwrap();
        assert_eq!(refused.state, OutboxState::Refused);
        assert_eq!(refused.error.as_deref(), Some("unlinked"));
        assert_eq!(
            registry.outbox_entry(&sending.id).unwrap().unwrap().state,
            OutboxState::Sending,
            "a send in flight is not recalled"
        );
        assert!(
            registry
                .queue_outbox(&entry("m1", "o/r#1", OutboxKind::Comment, 4.0))
                .is_err()
        );
    }

    fn only_the_sending_term_may_settle_an_entry(store: &dyn Store) {
        let registry = store.handle();
        registry.link_issue(&link("m1", "github", "o/r#1")).unwrap();
        let queued = registry
            .queue_outbox(&entry("m1", "o/r#1", OutboxKind::Comment, 1.0))
            .unwrap();
        assert_eq!(
            registry.due_outbox(0.5, 10).unwrap().len(),
            0,
            "not due yet"
        );
        assert_eq!(registry.due_outbox(1.0, 10).unwrap().len(), 1);
        let sending = registry
            .begin_outbox_send(&queued.id, 4, 2.0)
            .unwrap()
            .unwrap();
        assert_eq!(
            (sending.state, sending.fence),
            (OutboxState::Sending, Some(4))
        );
        assert!(
            registry
                .begin_outbox_send(&queued.id, 5, 2.0)
                .unwrap()
                .is_none()
        );
        assert!(registry.due_outbox(9.0, 10).unwrap().is_empty());
        let stale =
            registry.settle_outbox(&queued.id, 3, OutboxSettle::Posted(OutboxResult::default()));
        assert_eq!(stale.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        let posted = registry
            .settle_outbox(
                &queued.id,
                4,
                OutboxSettle::Posted(OutboxResult {
                    comment_id: Some("9".to_owned()),
                    ..OutboxResult::default()
                }),
            )
            .unwrap();
        assert_eq!(posted.state, OutboxState::Posted);
        assert_eq!(posted.result.unwrap().comment_id.as_deref(), Some("9"));
        assert!(
            registry
                .settle_outbox(&queued.id, 4, OutboxSettle::Refused("x".to_owned()))
                .is_err(),
            "a settled entry stays settled"
        );
    }

    fn a_new_term_adopts_what_the_old_one_left_sending(store: &dyn Store) {
        let registry = store.handle();
        registry.link_issue(&link("m1", "github", "o/r#1")).unwrap();
        let queued = registry
            .queue_outbox(&entry("m1", "o/r#1", OutboxKind::Comment, 1.0))
            .unwrap();
        registry.begin_outbox_send(&queued.id, 1, 2.0).unwrap();
        let left = registry.sending_outbox().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].sent_at, Some(2.0));
        let adopted = registry.adopt_outbox_send(&queued.id, 2).unwrap().unwrap();
        assert_eq!(adopted.fence, Some(2));
        assert!(
            registry
                .settle_outbox(&queued.id, 1, OutboxSettle::Refused("x".to_owned()))
                .is_err(),
            "the old term is fenced out"
        );
        registry
            .settle_outbox(
                &queued.id,
                2,
                OutboxSettle::Retry {
                    error: "not found".to_owned(),
                    not_before: 3.0,
                    max_attempts: 5,
                },
            )
            .unwrap();
        assert_eq!(
            registry.due_outbox(3.0, 10).unwrap().len(),
            1,
            "back to pending"
        );
        assert!(registry.adopt_outbox_send(&queued.id, 3).unwrap().is_none());
    }

    fn a_retry_waits_and_fails_after_the_attempts_allowed(store: &dyn Store) {
        let registry = store.handle();
        registry.link_issue(&link("m1", "github", "o/r#1")).unwrap();
        let queued = registry
            .queue_outbox(&entry("m1", "o/r#1", OutboxKind::Comment, 1.0))
            .unwrap();
        for attempt in 1..=3u32 {
            registry
                .begin_outbox_send(&queued.id, 1, 2.0)
                .unwrap()
                .unwrap();
            let settled = registry
                .settle_outbox(
                    &queued.id,
                    1,
                    OutboxSettle::Retry {
                        error: "429".to_owned(),
                        not_before: 10.0 * f64::from(attempt),
                        max_attempts: 3,
                    },
                )
                .unwrap();
            assert_eq!(settled.attempts, attempt);
            if attempt < 3 {
                assert_eq!(settled.state, OutboxState::Pending);
                assert!(registry.due_outbox(5.0, 10).unwrap().is_empty());
            } else {
                assert_eq!(settled.state, OutboxState::Failed);
                assert_eq!(settled.error.as_deref(), Some("429"));
            }
        }
    }

    /// Tables in memory, so a limit smaller than the default can be tried.
    #[derive(Default)]
    struct Mem {
        links: BTreeMap<String, Vec<IssueLink>>,
        outbox: Vec<OutboxEntry>,
        replies: BTreeMap<String, Vec<IssueReply>>,
    }

    impl IssueTables for Mem {
        fn links(&mut self, mission: &str) -> io::Result<Vec<IssueLink>> {
            Ok(self.links.get(mission).cloned().unwrap_or_default())
        }
        fn all_links(&mut self) -> io::Result<Vec<IssueLink>> {
            Ok(self.links.values().flatten().cloned().collect())
        }
        fn put_links(&mut self, mission: &str, links: &[IssueLink]) -> io::Result<()> {
            self.links.insert(mission.to_owned(), links.to_vec());
            Ok(())
        }
        fn outbox(&mut self) -> io::Result<Vec<OutboxEntry>> {
            Ok(self.outbox.clone())
        }
        fn put_entry(&mut self, entry: &OutboxEntry) -> io::Result<()> {
            self.outbox.retain(|held| held.id != entry.id);
            self.outbox.push(entry.clone());
            Ok(())
        }
        fn drop_entry(&mut self, id: &str) -> io::Result<()> {
            self.outbox.retain(|held| held.id != id);
            Ok(())
        }
        fn questions(&mut self, _: &str) -> io::Result<Vec<IssueQuestion>> {
            Ok(Vec::new())
        }
        fn all_questions(&mut self) -> io::Result<Vec<IssueQuestion>> {
            Ok(Vec::new())
        }
        fn put_questions(&mut self, _: &str, _: &[IssueQuestion]) -> io::Result<()> {
            Ok(())
        }
        fn replies(&mut self, mission: &str) -> io::Result<Vec<IssueReply>> {
            Ok(self.replies.get(mission).cloned().unwrap_or_default())
        }
        fn all_replies(&mut self) -> io::Result<Vec<IssueReply>> {
            Ok(self.replies.values().flatten().cloned().collect())
        }
        fn put_replies(&mut self, mission: &str, replies: &[IssueReply]) -> io::Result<()> {
            self.replies.insert(mission.to_owned(), replies.to_vec());
            Ok(())
        }
        fn proposals(&mut self) -> io::Result<Vec<IssueProposal>> {
            Ok(Vec::new())
        }
        fn put_proposal(&mut self, _: &IssueProposal) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn settled_history_is_pruned_to_the_limit_and_never_the_unsettled() {
        let limits = IssueLimits {
            outbox_records: crate::bounds::Limit::at(3),
            ..IssueLimits::default()
        };
        let mut mem = Mem::default();
        link_issue_in(&mut mem, &link("m1", "github", "o/r#1"), &limits).unwrap();
        for number in 0..6 {
            let queued = queue_in(
                &mut mem,
                &entry("m1", "o/r#1", OutboxKind::Comment, f64::from(number)),
                &limits,
            )
            .unwrap();
            begin_in(&mut mem, &queued.id, 1, 1.0).unwrap();
            settle_in(
                &mut mem,
                &queued.id,
                1,
                OutboxSettle::Posted(OutboxResult::default()),
            )
            .unwrap();
        }
        let pending = queue_in(
            &mut mem,
            &entry("m1", "o/r#1", OutboxKind::Comment, 9.0),
            &limits,
        )
        .unwrap();
        assert!(mem.outbox.len() <= 3, "{} entries", mem.outbox.len());
        assert!(mem.outbox.iter().any(|held| held.id == pending.id));
        assert!(
            mem.outbox.iter().any(|held| held.at == 5.0),
            "the newest settled stays"
        );
        assert!(
            !mem.outbox.iter().any(|held| held.at == 0.0),
            "the oldest goes"
        );
    }

    #[test]
    fn an_oversized_entry_is_refused() {
        let limits = IssueLimits {
            entry_bytes: crate::bounds::Limit::at(10),
            ..IssueLimits::default()
        };
        let mut mem = Mem::default();
        link_issue_in(&mut mem, &link("m1", "github", "o/r#1"), &limits).unwrap();
        let mut big = entry("m1", "o/r#1", OutboxKind::Comment, 1.0);
        big.body = "x".repeat(11);
        assert!(queue_in(&mut mem, &big, &limits).is_err());
    }
}
