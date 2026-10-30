//! M's policy for linked issues: what AI6 writes on a Mission's issues on its own, how much,
//! and the rules the Agent tools and creation follow (`[operation.ID.issues]`).
//!
//! Strict unless configured. Every value looser than `strict` is a weakening the Station
//! prints at start.

use serde::{Deserialize, Serialize};

use crate::bounds::Limit;
use crate::directorates::m::cover::Disclosure;

/// A starting point for the keys left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssuePreset {
    /// Closing, reopening and assignee changes need green ink at every clearance.
    Strict,
    /// As `strict`, except `00` Agents close and reopen without asking.
    Licensed,
}

/// How much AI6 writes on a Mission's source issue by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueUpdates {
    /// Only the report when the Mission ends.
    Outcome,
    /// One status comment kept current (started, waiting for input, paused), the report,
    /// and the configured labels or transitions.
    Milestones,
    /// As `milestones`, on every linked issue and not only the source.
    All,
}

/// What happens when nobody answers a question on the issue in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnTimeout {
    /// The Agent is resumed and told nobody answered.
    Resume,
    /// The Mission fails.
    Fail,
}

/// What happens to a post that holds a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnSecret {
    /// The post is refused.
    Refuse,
    /// The secret is replaced and the rest is posted.
    Redact,
}

/// Where links and images in an Agent's text may point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkHosts {
    /// Only the tracker's host and the Operation's git remote host.
    Tracker,
    /// Any host.
    Any,
}

/// Whether an Agent creates issues itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCreate {
    /// A person approves each creation.
    Proposal,
    /// The Agent creates the issue.
    Direct,
}

/// An Operation's issue policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssuePolicy {
    /// The preset the rest started from.
    pub preset: IssuePreset,
    /// How much AI6 writes by itself.
    pub updates: IssueUpdates,
    /// Bytes of one posted comment.
    pub max_body_bytes: Limit,
    /// Agent and person posts per hour per Mission (the bucket's refill).
    pub posts_per_hour: Limit,
    /// Posts a Mission may make in a burst.
    pub post_burst: Limit,
    /// Posts one Mission may make in all.
    pub posts_per_mission: Limit,
    /// Agent and person posts per hour for the whole Operation.
    pub operation_posts_per_hour: Limit,
    /// Writes per hour to one tracker instance, every kind counted.
    pub tracker_posts_per_hour: Limit,
    /// Seconds at least between two status writes to one issue.
    pub status_interval_secs: u64,
    /// Seconds a question waits for an answer.
    pub reply_ttl_secs: u64,
    /// What a question nobody answers does.
    pub on_timeout: OnTimeout,
    /// What a post that holds a secret does.
    pub on_secret: OnSecret,
    /// Where links and images in Agent text may point.
    pub links: LinkHosts,
    /// Whether an Agent creates issues itself.
    pub create: IssueCreate,
    /// Issues one Mission may create.
    pub max_created: Limit,
    /// Issues an Operation's Agents may create per day.
    pub max_created_per_day: Limit,
    /// How deep a split may go (a child of a child is depth 2).
    pub max_split_depth: Limit,
}

impl Default for IssuePolicy {
    fn default() -> Self {
        Self::preset(IssuePreset::Strict)
    }
}

impl IssuePolicy {
    /// The policy of a named preset.
    pub fn preset(preset: IssuePreset) -> Self {
        Self {
            preset,
            updates: IssueUpdates::Milestones,
            max_body_bytes: Limit::at(8192),
            posts_per_hour: Limit::at(6),
            post_burst: Limit::at(3),
            posts_per_mission: Limit::at(40),
            operation_posts_per_hour: Limit::at(60),
            tracker_posts_per_hour: Limit::at(300),
            status_interval_secs: 60,
            reply_ttl_secs: 3 * 24 * 3600,
            on_timeout: OnTimeout::Resume,
            on_secret: OnSecret::Refuse,
            links: LinkHosts::Tracker,
            create: IssueCreate::Proposal,
            max_created: Limit::at(10),
            max_created_per_day: Limit::at(50),
            max_split_depth: Limit::at(2),
        }
    }

    /// Whether an Agent of `clearance` closes or reopens an issue without green ink.
    pub fn closes_without_ink(&self, clearance: &str) -> bool {
        self.preset == IssuePreset::Licensed && clearance == "00"
    }

    /// One line per setting looser than `strict`, for the Station to print and publish.
    pub fn weakenings(&self) -> Vec<String> {
        let strict = Self::default();
        let mut lines = crate::bounds::looser(
            "issues",
            &serde_json::to_value(self).unwrap_or_default(),
            &serde_json::to_value(&strict).unwrap_or_default(),
            &[],
        );
        let mut note = |key: &str, value: &str, default: &str| {
            lines.push(format!(
                "issues.{key} {value} is looser than the default {default}"
            ));
        };
        if self.preset == IssuePreset::Licensed {
            note("preset", "licensed", "strict");
        }
        if self.on_secret == OnSecret::Redact {
            note("on_secret", "redact", "refuse");
        }
        if self.links == LinkHosts::Any {
            note("links", "any", "tracker");
        }
        if self.create == IssueCreate::Direct {
            note("create", "direct", "proposal");
        }
        lines
    }
}

/// The issue policy as written in configuration (`[operation.ID.issues]`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IssuesDefinition {
    preset: Option<IssuePreset>,
    updates: Option<IssueUpdates>,
    max_body_bytes: Option<Limit>,
    posts_per_hour: Option<Limit>,
    post_burst: Option<Limit>,
    posts_per_mission: Option<Limit>,
    operation_posts_per_hour: Option<Limit>,
    tracker_posts_per_hour: Option<Limit>,
    status_interval_secs: Option<u64>,
    reply_ttl_secs: Option<u64>,
    on_timeout: Option<OnTimeout>,
    on_secret: Option<OnSecret>,
    links: Option<LinkHosts>,
    create: Option<IssueCreate>,
    max_created: Option<Limit>,
    max_created_per_day: Option<Limit>,
    max_split_depth: Option<Limit>,
}

impl IssuesDefinition {
    /// Applies the written values over the preset.
    ///
    /// # Errors
    ///
    /// Returns a message naming the key for a value that cannot work: a zero body size, a
    /// zero burst or a zero reply lifetime.
    pub fn resolve(&self) -> Result<IssuePolicy, String> {
        let mut policy = IssuePolicy::preset(self.preset.unwrap_or(IssuePreset::Strict));
        macro_rules! take {
            ($($field:ident),*) => {
                $(if let Some(value) = self.$field { policy.$field = value; })*
            };
        }
        take!(
            updates,
            max_body_bytes,
            posts_per_hour,
            post_burst,
            posts_per_mission,
            operation_posts_per_hour,
            tracker_posts_per_hour,
            status_interval_secs,
            reply_ttl_secs,
            on_timeout,
            on_secret,
            links,
            create,
            max_created,
            max_created_per_day,
            max_split_depth
        );
        for (key, zero) in [
            ("max_body_bytes", policy.max_body_bytes.get() == Some(0)),
            ("post_burst", policy.post_burst.get() == Some(0)),
            ("reply_ttl_secs", policy.reply_ttl_secs == 0),
        ] {
            if zero {
                return Err(format!("issues {key} must be at least 1"));
            }
        }
        Ok(policy)
    }
}

/// A Mission's progress on its issue, in the words the Operation's cover policy allows.
///
/// `phase` is `working`, `waiting`, `paused`, `done` or `failed`.
pub fn compose_status(disclosure: Disclosure, phase: &str) -> String {
    let (hidden, full) = match phase {
        "working" => (
            "The requested work has started.",
            "AI6 is working on this item.",
        ),
        "waiting" => (
            "The requested work is waiting for an answer on this item.",
            "AI6 is waiting for an answer on this item.",
        ),
        "paused" => (
            "The requested work is paused.",
            "AI6 has paused work on this item.",
        ),
        "done" => (
            "The requested work is finished.",
            "AI6 finished work on this item.",
        ),
        _ => (
            "The requested work could not be completed.",
            "AI6 ended work on this item without completing it.",
        ),
    };
    match disclosure {
        Disclosure::Hidden => hidden.to_owned(),
        Disclosure::Assisted => format!("AI-assisted work: {}", hidden.to_lowercase()),
        _ => full.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(text: &str) -> Result<IssuePolicy, String> {
        toml::from_str::<IssuesDefinition>(text)
            .map_err(|error| error.to_string())?
            .resolve()
    }

    #[test]
    fn the_default_is_strict_and_weakens_nothing() {
        let policy = resolve("").unwrap();
        assert_eq!(policy, IssuePolicy::default());
        assert!(policy.weakenings().is_empty());
        assert_eq!(policy.updates, IssueUpdates::Milestones);
        assert_eq!(policy.posts_per_mission.get(), Some(40));
        assert_eq!(policy.reply_ttl_secs, 259_200);
    }

    #[test]
    fn looser_values_are_each_a_weakening_line() {
        let policy = resolve(
            "preset = \"licensed\"\nposts_per_hour = 60\nmax_created = \"none\"\n\
             on_secret = \"redact\"\nlinks = \"any\"\ncreate = \"direct\"\nupdates = \"all\"",
        )
        .unwrap();
        let lines = policy.weakenings().join("\n");
        for expected in [
            "issues.posts_per_hour 60 is looser than the default 6",
            "issues.max_created none is looser than the default 10",
            "issues.preset licensed is looser than the default strict",
            "issues.on_secret redact is looser than the default refuse",
            "issues.links any is looser than the default tracker",
            "issues.create direct is looser than the default proposal",
        ] {
            assert!(lines.contains(expected), "{expected} in {lines}");
        }
        assert!(policy.closes_without_ink("00"));
        assert!(!policy.closes_without_ink("field"));
        assert!(!IssuePolicy::default().closes_without_ink("00"));
    }

    #[test]
    fn nonsense_and_unknown_keys_are_refused_naming_the_key() {
        for bad in [
            "max_body_bytes = 0",
            "post_burst = 0",
            "reply_ttl_secs = 0",
            "surprise = 1",
            "updates = \"sometimes\"",
        ] {
            let error = resolve(bad).unwrap_err();
            let key = bad.split(' ').next().unwrap();
            assert!(
                error.contains(key) || error.contains("unknown"),
                "{bad}: {error}"
            );
        }
    }

    #[test]
    fn status_words_follow_the_cover_policy() {
        assert!(!compose_status(Disclosure::Hidden, "working").contains("AI"));
        assert!(compose_status(Disclosure::Assisted, "waiting").starts_with("AI-assisted"));
        assert!(compose_status(Disclosure::Full, "failed").contains("without completing"));
        for phase in ["working", "waiting", "paused", "done", "failed"] {
            assert!(
                !compose_status(Disclosure::Hidden, phase).contains("AI6"),
                "{phase}"
            );
        }
    }
}
