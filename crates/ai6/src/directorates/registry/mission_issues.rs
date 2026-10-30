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
        }
    }

    fn entry(mission: &str, item: &str, kind: OutboxKind, at: f64) -> OutboxEntry {
        OutboxEntry::new("G", mission, "github", item, kind, "m", at)
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
