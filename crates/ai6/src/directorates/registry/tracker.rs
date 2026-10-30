//! Trackers: the Registry's contract with an issue tracker for reading, commenting,
//! labelling and creating issues.
//!
//! A [`Tracker`] is the second kind a tracker provider serves beside [`Source`]. The
//! provider is built from the same `sources.<name>` settings table, so one table and one
//! token configure both. Every call is blocking and names its issue by the provider-local
//! item ID a [`SourceItem`] carries (`owner/repo#12`, `GUA-12`).
//!
//! [`Source`]: super::sources::Source
//! [`SourceItem`]: super::sources::SourceItem

use guatiao::library::ProviderError;
use guatiao::{FromValue, Schema, ToValue};

/// What one tracker instance can do, discovered from the provider and the tracker.
///
/// Answers about the repository or project are the same for every issue and may be cached
/// by the caller for a while; `transitions` depends on the issue it was asked about.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct TrackerCapabilities {
    /// New comments can be posted.
    pub comment: bool,
    /// A posted comment can be edited in place.
    pub edit_comment: bool,
    /// The labels the repository or project has; empty when `free_labels`.
    pub labels: Vec<String>,
    /// Labels are free text, so any label is accepted and `labels` lists none.
    pub free_labels: bool,
    /// The workflow transitions open on the issue asked about; empty without a workflow.
    pub transitions: Vec<String>,
    /// Assignees can be changed.
    pub assign: bool,
    /// An issue can be closed and reopened.
    pub close: bool,
    /// Issues can be created.
    pub create: bool,
    /// The issue types a created issue may have; empty when the tracker has none.
    pub issue_types: Vec<String>,
    /// The priorities a created issue may carry; empty when the tracker has none.
    pub priorities: Vec<String>,
    /// How a child issue is attached to its parent: `none`, `sub_issues`, `subtasks` or
    /// `parent`.
    pub children: String,
    /// The ways two issues are linked besides naming each other in text: `issue_links`
    /// and `remote_links`; empty when only text can.
    pub links: Vec<String>,
    /// Comments made after a moment can be listed.
    pub replies: bool,
    /// Whether the repository or project is `public`, `private` or `unknown`.
    pub visibility: String,
    /// A comment can carry a hidden marker the provider finds again.
    pub markers: bool,
}

/// One issue as the tracker holds it. Titles and bodies are untrusted text.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct TrackerIssue {
    /// The provider-local identity, as [`SourceItem::id`](super::sources::SourceItem::id).
    pub id: String,
    /// The tracker's own numeric or opaque ID, when linking children needs it.
    pub native_id: String,
    /// The title.
    pub title: String,
    /// The body, as Markdown or text.
    pub body: String,
    /// `open` or `closed`.
    pub state: String,
    /// Labels currently on the issue.
    pub labels: Vec<String>,
    /// Assignee logins or account IDs.
    pub assignees: Vec<String>,
    /// A link to the issue.
    pub url: Option<String>,
    /// When the tracker last changed the issue (ISO 8601).
    pub updated: String,
    /// The parent issue's ID, when the tracker says it has one.
    pub parent: Option<String>,
    /// The author's login or account ID, for the author rule of repliers.
    pub author_id: Option<String>,
}

/// A request for comments made after a moment on a set of issues.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct ReplyPoll {
    /// The issues to read; empty reads nothing.
    pub items: Vec<String>,
    /// The cursor the previous batch returned; `None` reads every comment.
    pub since: Option<String>,
}

/// One comment on an issue.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct Reply {
    /// The tracker's comment ID.
    pub id: String,
    /// The issue it was made on.
    pub item: String,
    /// The author's login or account ID.
    pub author_id: String,
    /// The author's display name.
    pub author: String,
    /// Whether the author may answer for the issue: the issue's author, a repository
    /// owner, member or collaborator, a Jira reporter or assignee, or someone the
    /// settings name.
    pub permitted: bool,
    /// Whether the comment was written by the identity AI6 posts as.
    pub own: bool,
    /// The comment text. Untrusted.
    pub body: String,
    /// When it was made (ISO 8601).
    pub created: String,
}

/// The comments a poll found and where to resume.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct ReplyBatch {
    /// Comments after the cursor, oldest first.
    pub replies: Vec<Reply>,
    /// The cursor for the next poll; `None` keeps the one sent.
    pub cursor: Option<String>,
}

/// A comment to post or edit.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct TrackerPost {
    /// The issue.
    pub item: String,
    /// The comment text, as Markdown.
    pub body: String,
    /// The comment to replace, when this edits one.
    pub edit: Option<String>,
    /// A short name the provider hides in the comment where the tracker allows it.
    pub marker: Option<String>,
}

/// The comment a post made or edited.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct PostedComment {
    /// The tracker's comment ID, kept to edit it later.
    pub id: String,
    /// A link to the comment.
    pub url: Option<String>,
}

/// Changes to an issue's labels, workflow state and assignees.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct TrackerUpdate {
    /// The issue.
    pub item: String,
    /// Labels to add.
    pub add_labels: Vec<String>,
    /// Labels to remove.
    pub remove_labels: Vec<String>,
    /// A workflow transition, by ID or name (Jira).
    pub transition: Option<String>,
    /// Assignees to add.
    pub add_assignees: Vec<String>,
    /// Assignees to remove.
    pub remove_assignees: Vec<String>,
    /// `open` or `closed` (GitHub); another value is refused.
    pub state: Option<String>,
}

/// An issue to create.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct IssueSpec {
    /// The title.
    pub title: String,
    /// The body as Markdown; the provider words it for the tracker.
    pub body: String,
    /// Labels to set; the provider keeps those the tracker has.
    pub labels: Vec<String>,
    /// The issue type; empty takes the instance's `issue_type`.
    pub issue_type: Option<String>,
    /// The priority, already in the target tracker's words.
    pub priority: Option<String>,
    /// The parent issue, when the new one is a child.
    pub parent: Option<String>,
}

/// A link between two issues.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct TrackerLink {
    /// The issue in this tracker the link is made on.
    pub from: String,
    /// The other issue, by ID in this tracker, when it lives here.
    pub to: Option<String>,
    /// The link's meaning: `related`, `child`, `parent` or `mirror`.
    pub relation: String,
    /// The other issue's URL, when it lives in another tracker (a remote link).
    pub url: Option<String>,
}

/// An issue tracker that is read, commented on and written to on demand.
///
/// A call the instance cannot make fails with `GUATIAO_ERR_UNSUPPORTED` and a message
/// naming what it can do; an item outside the instance's repository or project fails with
/// `GUATIAO_ERR_BAD_VALUE` and a message that starts `foreign item`.
#[guatiao::kind(name = "ai6_tracker")]
pub trait Tracker: Send + Sync {
    /// Returns what the instance can do; `item` adds the answers that depend on one issue.
    fn capabilities(&self, item: Option<String>) -> Result<TrackerCapabilities, ProviderError>;

    /// Reads one issue.
    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError>;

    /// Lists the comments made after `request.since` on the issues it names.
    fn replies(&self, request: ReplyPoll) -> Result<ReplyBatch, ProviderError>;

    /// Posts a comment, or edits the one `post.edit` names.
    fn post(&self, post: TrackerPost) -> Result<PostedComment, ProviderError>;

    /// Changes labels, workflow state or assignees.
    fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError>;

    /// Creates an issue and returns it, with its parent when one was attached.
    fn create(&self, spec: IssueSpec) -> Result<TrackerIssue, ProviderError>;

    /// Links two issues, or an issue and an address elsewhere.
    fn link(&self, link: TrackerLink) -> Result<(), ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // A changed required signature affects every built tracker provider.
    #[test]
    fn floor_hashes_are_pinned() {
        assert_eq!(TrackerVtable::FLOOR_HASH, 1899884072);
    }
}
