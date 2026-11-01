//! M's policy for linked issues: what AI6 writes on a Mission's issues on its own, how much,
//! and the rules the Agent tools and creation follow (`[operation.ID.issues]`).
//!
//! Strict unless configured. Every value looser than `strict` is a weakening the Station
//! prints at start.

use guatiao::{FromValue, Schema, ToValue};
use serde::{Deserialize, Serialize};

use crate::bounds::Limit;
use crate::directorates::m::cover::Disclosure;

/// A starting point for the keys left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToValue, FromValue, Schema)]
#[serde(rename_all = "snake_case")]
#[map(rename_all = "snake_case")]
pub enum IssuePreset {
    /// Closing, reopening and assignee changes need green ink at every clearance.
    Strict,
    /// As `strict`, except `00` Agents close and reopen without asking.
    Licensed,
}

/// How much AI6 writes on a Mission's source issue by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToValue, FromValue, Schema)]
#[serde(rename_all = "snake_case")]
#[map(rename_all = "snake_case")]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToValue, FromValue, Schema)]
#[serde(rename_all = "snake_case")]
#[map(rename_all = "snake_case")]
pub enum OnTimeout {
    /// The Agent is resumed and told nobody answered.
    Resume,
    /// The Mission fails.
    Fail,
    /// The Mission keeps waiting.
    Wait,
}

/// What happens to a post that holds a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToValue, FromValue, Schema)]
#[serde(rename_all = "snake_case")]
#[map(rename_all = "snake_case")]
pub enum OnSecret {
    /// The post is refused.
    Refuse,
    /// The secret is replaced and the rest is posted.
    Redact,
}

/// Where links and images in an Agent's text may point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToValue, FromValue, Schema)]
#[serde(rename_all = "snake_case")]
#[map(rename_all = "snake_case")]
pub enum LinkHosts {
    /// Only the tracker's host and the Operation's git remote host.
    Tracker,
    /// Any host.
    Any,
}

/// Whether an Agent creates issues itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToValue, FromValue, Schema)]
#[serde(rename_all = "snake_case")]
#[map(rename_all = "snake_case")]
pub enum IssueCreate {
    /// A person approves each creation, or a class authorisation approves one from an
    /// unexposed Session with no findings.
    Propose,
    /// The Agent creates the issue at once.
    Direct,
    /// Agents cannot ask for issues.
    Off,
}

/// An Operation's issue policy, `[operation.ID.issues]`. A key left out takes its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToValue, FromValue, Schema)]
#[serde(default, deny_unknown_fields)]
#[map(deny_unknown_fields)]
pub struct IssuePolicy {
    /// `strict`, or `licensed`, which lets `00` Agents close and reopen without green ink.
    #[schema(title = "Preset")]
    #[map(default = IssuePolicy::default().preset)]
    pub preset: IssuePreset,
    /// `outcome`: only the report when the Mission ends; `milestones`: one status comment kept
    /// current on the source issue (working, waiting, paused) plus the report; `all`: also on
    /// every other linked issue.
    #[schema(title = "Automatic updates")]
    #[map(default = IssuePolicy::default().updates)]
    pub updates: IssueUpdates,
    /// Bytes of one posted comment.
    #[schema(title = "Size of a post", format = "bytes")]
    #[map(default = IssuePolicy::default().max_body_bytes)]
    pub max_body_bytes: Limit,
    /// Agent and person posts per hour per Mission (the bucket's refill).
    #[schema(title = "Posts an hour, per Mission")]
    #[map(default = IssuePolicy::default().posts_per_hour)]
    pub posts_per_hour: Limit,
    /// Posts a Mission may make in a burst.
    #[schema(title = "Burst of posts")]
    #[map(default = IssuePolicy::default().post_burst)]
    pub post_burst: Limit,
    /// Posts one Mission may make in all.
    #[schema(title = "Posts per Mission")]
    #[map(default = IssuePolicy::default().posts_per_mission)]
    pub posts_per_mission: Limit,
    /// Agent and person posts per hour for the whole Operation.
    #[schema(title = "Posts an hour, per Operation")]
    #[map(default = IssuePolicy::default().operation_posts_per_hour)]
    pub operation_posts_per_hour: Limit,
    /// Writes per hour to one tracker instance, every kind counted.
    #[schema(title = "Writes an hour, per tracker")]
    #[map(default = IssuePolicy::default().tracker_posts_per_hour)]
    pub tracker_posts_per_hour: Limit,
    /// Seconds at least between two status writes to one issue.
    #[schema(title = "Between status writes", format = "seconds")]
    #[map(default = IssuePolicy::default().status_interval_secs)]
    pub status_interval_secs: u64,
    /// Seconds a question waits for an answer.
    #[schema(title = "A question waits", format = "seconds")]
    #[map(default = IssuePolicy::default().reply_ttl_secs)]
    pub reply_ttl_secs: u64,
    /// `resume` tells the Agent nobody answered; `fail` fails the Mission; `wait` keeps waiting.
    #[schema(title = "When nobody answers")]
    #[map(default = IssuePolicy::default().on_timeout)]
    pub on_timeout: OnTimeout,
    /// `refuse` refuses a post holding a secret; `redact` replaces it and posts the rest.
    #[schema(title = "When a post holds a secret")]
    #[map(default = IssuePolicy::default().on_secret)]
    pub on_secret: OnSecret,
    /// `tracker`: links and images in Agent text only to the tracker's host and the Operation's
    /// git remote host; `any`: every host.
    #[schema(title = "Links in Agent text")]
    #[map(default = IssuePolicy::default().links)]
    pub links: LinkHosts,
    /// `propose`: a person approves each creation, or a class authorisation `issue-create` one
    /// from an unexposed Session with no findings; `direct`: the Agent creates at once;
    /// `off`: Agents cannot ask.
    #[schema(title = "Creating issues")]
    #[map(default = IssuePolicy::default().create)]
    pub create: IssueCreate,
    /// Issues one Mission may create, those waiting for a person included.
    #[schema(title = "Issues per Mission")]
    #[map(default = IssuePolicy::default().max_created)]
    pub max_created: Limit,
    /// Issues an Operation's Agents may create per day.
    #[schema(title = "Issues a day, per Operation")]
    #[map(default = IssuePolicy::default().max_created_per_day)]
    pub max_created_per_day: Limit,
    /// How deep a split may go (a child of a child is depth 2).
    #[schema(title = "Depth of a split")]
    #[map(default = IssuePolicy::default().max_split_depth)]
    pub max_split_depth: Limit,
}

impl Default for IssuePolicy {
    fn default() -> Self {
        Self {
            preset: IssuePreset::Strict,
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
            create: IssueCreate::Propose,
            max_created: Limit::at(10),
            max_created_per_day: Limit::at(50),
            max_split_depth: Limit::at(2),
        }
    }
}

impl IssuePolicy {
    /// Whether an Agent of `clearance` closes or reopens an issue without green ink.
    pub fn closes_without_ink(&self, clearance: &str) -> bool {
        self.preset == IssuePreset::Licensed && clearance == "00"
    }

    /// Refuses values that cannot work.
    ///
    /// # Errors
    ///
    /// Returns a message naming the key for a zero body size, burst or reply lifetime.
    pub fn validate(&self) -> Result<(), String> {
        for (key, zero) in [
            ("max_body_bytes", self.max_body_bytes.get() == Some(0)),
            ("post_burst", self.post_burst.get() == Some(0)),
            ("reply_ttl_secs", self.reply_ttl_secs == 0),
        ] {
            if zero {
                return Err(format!("issues {key} must be at least 1"));
            }
        }
        Ok(())
    }

    /// One line per setting looser than the default, for the Station to print and publish.
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
            note("create", "direct", "propose");
        }
        lines
    }
}

/// Whether `label` would start work: it looks like a ready label, or the Operation's instances
/// take intake from it. `None` when the Operation's settings cannot be read.
pub fn is_ready_label(
    operation: Option<&crate::directorates::m::roster::Operation>,
    label: &str,
) -> bool {
    let lower = label.to_lowercase();
    lower.starts_with("ai6:ready")
        || lower.starts_with("ai6-ready")
        || operation.is_some_and(|operation| {
            operation.sources.values().any(|instance| {
                instance
                    .get("labels")
                    .and_then(toml::Value::as_array)
                    .is_some_and(|labels| {
                        labels
                            .iter()
                            .filter_map(toml::Value::as_str)
                            .any(|named| named.eq_ignore_ascii_case(label))
                    })
            })
        })
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

/// A reference table of a settings struct, from its one declaration: each key's title,
/// description and default.
#[cfg(test)]
pub(crate) fn schema_table<T: Schema + ToValue + Default>() -> String {
    let json = |value: guatiao::Value| {
        serde_json::to_value(guatiao_serde::Serializable::from(&value)).unwrap()
    };
    let schema = json(T::schema().unwrap());
    let defaults = json(T::default().to_value().unwrap());
    let shown = |value: Option<&serde_json::Value>| match value {
        None | Some(serde_json::Value::Null) => "none".to_owned(),
        Some(serde_json::Value::String(text)) => format!("`{text}`"),
        Some(serde_json::Value::Array(list)) if list.is_empty() => "none".to_owned(),
        Some(serde_json::Value::Object(map)) if map.is_empty() => "none".to_owned(),
        Some(serde_json::Value::Object(map)) if map.values().all(serde_json::Value::is_null) => {
            "none".to_owned()
        }
        Some(other) => format!("`{other}`"),
    };
    let mut table = String::from("| Key | Default | Meaning |\n| --- | --- | --- |\n");
    for (key, field) in schema["properties"].as_object().unwrap() {
        let description = field["description"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let title = field["title"].as_str().unwrap_or_default();
        table.push_str(&format!(
            "| `{key}` | {} | **{title}.** {description} |\n",
            shown(defaults.get(key))
        ));
    }
    table
}

/// Checks the table between `<!-- schema:NAME:begin -->` and its end in `docs/operations.md`, or
/// writes it when `AI6_WRITE_SCHEMA_TABLES` is set.
#[cfg(test)]
pub(crate) fn check_schema_table(name: &str, table: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/operations.md");
    let text = std::fs::read_to_string(&path).unwrap();
    let (begin, end) = (
        format!("<!-- schema:{name}:begin -->\n"),
        format!("<!-- schema:{name}:end -->"),
    );
    let start = text.find(&begin).expect("the begin marker") + begin.len();
    let stop = text.find(&end).expect("the end marker");
    if std::env::var_os("AI6_WRITE_SCHEMA_TABLES").is_some() {
        std::fs::write(&path, format!("{}{table}{}", &text[..start], &text[stop..])).unwrap();
        return;
    }
    assert_eq!(
        &text[start..stop],
        table,
        "the {name} table in docs/operations.md is not its struct's: run this test with \
         AI6_WRITE_SCHEMA_TABLES=1"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_in_the_docs_is_the_structs() {
        check_schema_table("issues", &schema_table::<IssuePolicy>());
    }

    #[test]
    fn the_table_comes_from_the_declaration() {
        let table = schema_table::<IssuePolicy>();
        assert!(
            table.contains("| `create` | `propose` | **Creating issues.** `propose`: a person"),
            "{table}"
        );
        assert!(table.contains("| `posts_per_hour` | `6` |"), "{table}");
    }

    fn resolve(text: &str) -> Result<IssuePolicy, String> {
        let policy: IssuePolicy = toml::from_str(text).map_err(|error| error.to_string())?;
        policy.validate()?;
        Ok(policy)
    }

    #[test]
    fn every_setting_a_person_writes_has_a_title_and_a_description() {
        let schema = IssuePolicy::schema().unwrap();
        let schema = serde_json::to_value(guatiao_serde::Serializable::from(&schema)).unwrap();
        let properties = schema["properties"].as_object().unwrap();
        assert!(properties.len() >= 17, "{schema}");
        for (key, field) in properties {
            assert!(field["title"].is_string(), "{key} has no title: {field}");
            assert!(
                field["description"].is_string(),
                "{key} has no description: {field}"
            );
        }
    }

    #[test]
    fn a_value_reads_the_same_through_guatiao_as_through_toml() {
        let table: toml::Table =
            toml::from_str("create = \"off\"\nmax_created = \"none\"\nreply_ttl_secs = 60")
                .unwrap();
        let value = guatiao_serde::from_serde(toml::Value::Table(table.clone()))
            .map_err(|error: toml::de::Error| error)
            .unwrap();
        let through_value = IssuePolicy::from_value(&value).unwrap();
        let through_toml: IssuePolicy = toml::Value::Table(table).try_into().unwrap();
        assert_eq!(through_value, through_toml);
        assert_eq!(through_value.create, IssueCreate::Off);
        assert_eq!(through_value.max_created, Limit::UNBOUNDED);
        assert_eq!(through_value.reply_ttl_secs, 60);
        assert_eq!(through_value.posts_per_hour, Limit::at(6), "a default");
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
            "issues.create direct is looser than the default propose",
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
