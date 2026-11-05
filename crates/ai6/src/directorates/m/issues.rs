//! M's policy for linked issues: what AI6 writes on a Mission's issues on its own, how much,
//! and the rules the Agent tools and creation follow (`[operation.ID.issues]`).
//!
//! Strict unless configured. Every value looser than `strict` is a weakening the Station
//! prints at start.

use guatiao::{FromValue, Schema, ToValue};
use guatiao_intake::Form;

use crate::bounds::Limit;
use crate::directorates::m::cover::Disclosure;

/// A starting point for the keys left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(rename_all = "snake_case")]
pub enum IssuePreset {
    /// Closing, reopening and assignee changes need green ink at every clearance.
    #[schema(label = "Strict: closing and assigning wait for a person")]
    Strict,
    /// As `strict`, except `00` Agents close and reopen without asking.
    #[schema(label = "Licensed: as strict, but 00 Agents close and reopen without asking")]
    Licensed,
}

/// How much AI6 writes on a Mission's source issue by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(rename_all = "snake_case")]
pub enum IssueUpdates {
    /// Only the report when the Mission ends.
    #[schema(label = "Only the report when the Mission ends")]
    Outcome,
    /// One status comment kept current (started, waiting for input, paused), the report,
    /// and the configured labels or transitions.
    #[schema(label = "One status comment kept current, and the report")]
    Milestones,
    /// As `milestones`, on every linked issue and not only the source.
    #[schema(label = "As that, on every linked issue")]
    All,
}

/// What happens when nobody answers a question on the issue in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(rename_all = "snake_case")]
pub enum OnTimeout {
    /// The Agent is resumed and told nobody answered.
    #[schema(label = "Resume the Agent, told nobody answered")]
    Resume,
    /// The Mission fails.
    #[schema(label = "Fail the Mission")]
    Fail,
    /// The Mission keeps waiting.
    #[schema(label = "Keep waiting")]
    Wait,
}

/// What happens to a post that holds a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(rename_all = "snake_case")]
pub enum OnSecret {
    /// The post is refused.
    #[schema(label = "Refuse the post")]
    Refuse,
    /// The secret is replaced and the rest is posted.
    #[schema(label = "Replace the secret and post the rest")]
    Redact,
}

/// Where links and images in an Agent's text may point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(rename_all = "snake_case")]
pub enum LinkHosts {
    /// Only the tracker's host and the Operation's git remote host.
    #[schema(label = "Only the tracker's and the git remote's hosts")]
    Tracker,
    /// Any host.
    #[schema(label = "Any host")]
    Any,
}

/// Whether an Agent creates issues itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(rename_all = "snake_case")]
pub enum IssueCreate {
    /// A person approves each creation, or a class authorisation approves one from an
    /// unexposed Session with no findings.
    #[schema(label = "A person approves each one")]
    Propose,
    /// The Agent creates the issue at once.
    #[schema(label = "The Agent creates at once")]
    Direct,
    /// Agents cannot ask for issues.
    #[schema(label = "Agents cannot ask")]
    Off,
}

/// An Operation's issue policy, `[operation.ID.issues]`. A key left out takes its default.
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
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
    /// Children one split may ask for.
    #[schema(title = "Children of a split")]
    #[map(default = IssuePolicy::default().max_split_children)]
    pub max_split_children: Limit,
    /// Seconds a title stays a duplicate of an issue AI6 created. Shorter is looser.
    #[schema(title = "A title stays a duplicate for", format = "seconds")]
    #[map(default = IssuePolicy::default().duplicate_window_secs)]
    pub duplicate_window_secs: u64,
    /// Seconds the approval a class authorisation gives an issue creation lasts. Longer is looser.
    #[schema(title = "A creation's approval lasts", format = "seconds")]
    #[map(default = IssuePolicy::default().creation_approval_secs)]
    pub creation_approval_secs: u64,
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
            max_split_children: Limit::at(10),
            duplicate_window_secs: 30 * 24 * 3600,
            creation_approval_secs: 3600,
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
        let mut lines = Vec::new();
        let mut note = |key: &str, value: &str, default: &str| {
            lines.push(format!(
                "issues.{key} {value} is looser than the default {default}"
            ));
        };
        for (key, value, default) in [
            ("max_body_bytes", self.max_body_bytes, strict.max_body_bytes),
            ("posts_per_hour", self.posts_per_hour, strict.posts_per_hour),
            ("post_burst", self.post_burst, strict.post_burst),
            (
                "posts_per_mission",
                self.posts_per_mission,
                strict.posts_per_mission,
            ),
            (
                "operation_posts_per_hour",
                self.operation_posts_per_hour,
                strict.operation_posts_per_hour,
            ),
            (
                "tracker_posts_per_hour",
                self.tracker_posts_per_hour,
                strict.tracker_posts_per_hour,
            ),
            ("max_created", self.max_created, strict.max_created),
            (
                "max_created_per_day",
                self.max_created_per_day,
                strict.max_created_per_day,
            ),
            (
                "max_split_depth",
                self.max_split_depth,
                strict.max_split_depth,
            ),
            (
                "max_split_children",
                self.max_split_children,
                strict.max_split_children,
            ),
        ] {
            let looser = match (value.get(), default.get()) {
                (None, Some(_)) => true,
                (Some(value), Some(default)) => value > default,
                _ => false,
            };
            if looser {
                note(key, &value.to_string(), &default.to_string());
            }
        }
        if self.status_interval_secs < strict.status_interval_secs {
            note(
                "status_interval_secs",
                &self.status_interval_secs.to_string(),
                &strict.status_interval_secs.to_string(),
            );
        }
        if self.duplicate_window_secs < strict.duplicate_window_secs {
            note(
                "duplicate_window_secs",
                &self.duplicate_window_secs.to_string(),
                &strict.duplicate_window_secs.to_string(),
            );
        }
        if self.creation_approval_secs > strict.creation_approval_secs {
            note(
                "creation_approval_secs",
                &self.creation_approval_secs.to_string(),
                &strict.creation_approval_secs.to_string(),
            );
        }
        if self.reply_ttl_secs > strict.reply_ttl_secs {
            note(
                "reply_ttl_secs",
                &self.reply_ttl_secs.to_string(),
                &strict.reply_ttl_secs.to_string(),
            );
        }
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

/// Where a person reads a tracker instance of an Operation, for linking to what it holds.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SourcePage {
    /// `github`, `gitlab` or `jira`.
    pub provider: String,
    /// The repository's or project's page, or a Jira site's root.
    pub web: String,
    /// A Jira project's key; a Jira instance reading by query alone has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// The web pages of an Operation's `sources`, from the settings its providers read: `github`
/// (`repo`, `api_url`), `gitlab` (`project`, `api_url`) and `jira` (`site`, `project`). An instance
/// of another provider, or one not served over `https`, has none.
pub fn source_pages(sources: &toml::Table) -> Vec<SourcePage> {
    sources
        .iter()
        .filter_map(|(name, source)| {
            let table = source.as_table();
            let get = |key: &str| table.and_then(|table| table.get(key)?.as_str());
            let provider = get("provider").unwrap_or(name);
            let target = source.as_str();
            let (web, project) = match provider {
                "github" => {
                    let repo = get("repo").or(get("target")).or(target)?;
                    let api = get("api_url").unwrap_or("https://api.github.com");
                    let root = match api.trim_end_matches('/') {
                        "https://api.github.com" => "https://github.com",
                        other => other.strip_suffix("/api/v3")?,
                    };
                    (format!("{root}/{repo}"), None)
                }
                "gitlab" => {
                    let project = get("project").or(get("target")).or(target)?;
                    let api = get("api_url").unwrap_or("https://gitlab.com/api/v4");
                    let root = api.trim_end_matches('/').strip_suffix("/api/v4")?;
                    (format!("{root}/{project}"), None)
                }
                "jira" => {
                    let site = get("site")?.trim_end_matches('/').to_owned();
                    (site, get("project").or(get("target")).map(str::to_owned))
                }
                _ => return None,
            };
            web.starts_with("https://").then(|| SourcePage {
                provider: provider.to_owned(),
                web,
                project,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(text: &str) -> Result<IssuePolicy, String> {
        let policy: IssuePolicy = crate::directorates::m::records::from_toml(text)?;
        policy.validate()?;
        Ok(policy)
    }

    #[test]
    fn each_provider_names_its_page() {
        let sources: toml::Table = toml::from_str(
            r#"
            github = "o/n"
            jira = "GUA"

            [board]
            provider = "github"
            repo = "o/board"

            [own]
            provider = "github"
            repo = "o/own"
            api_url = "https://ghe.example/api/v3/"

            [lab]
            provider = "gitlab"
            project = "g/s/n"
            api_url = "https://gitlab.example/api/v4"

            [gitlab]
            project = "g/n"

            [tracker]
            provider = "jira"
            site = "https://example.atlassian.net/"
            project = "OPS"

            [query]
            provider = "jira"
            site = "https://other.atlassian.net"
            jql = "labels = ready"

            [plain]
            provider = "github"
            repo = "o/local"
            api_url = "http://127.0.0.1:1/api/v3"

            [odd]
            provider = "github"
            repo = "o/odd"
            api_url = "https://proxy.example/github"

            [custom]
            provider = "tickets"
            site = "https://tickets.example"
            "#,
        )
        .unwrap();
        let pages: Vec<_> = source_pages(&sources)
            .into_iter()
            .map(|page| (page.provider, page.web, page.project))
            .collect();
        let page = |provider: &str, web: &str, project: Option<&str>| {
            (
                provider.to_owned(),
                web.to_owned(),
                project.map(str::to_owned),
            )
        };
        assert_eq!(
            pages,
            [
                page("github", "https://github.com/o/board", None),
                page("github", "https://github.com/o/n", None),
                page("gitlab", "https://gitlab.com/g/n", None),
                page("gitlab", "https://gitlab.example/g/s/n", None),
                page("github", "https://ghe.example/o/own", None),
                page("jira", "https://other.atlassian.net", None),
                page("jira", "https://example.atlassian.net", Some("OPS")),
            ],
            "a bare Jira key has no site, and http, unknown API roots and other providers have \
             no page"
        );
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
    fn a_table_reads_its_values_and_defaults_the_rest() {
        let policy =
            resolve("create = \"off\"\nmax_created = \"none\"\nreply_ttl_secs = 60").unwrap();
        assert_eq!(policy.create, IssueCreate::Off);
        assert_eq!(policy.max_created, Limit::UNBOUNDED);
        assert_eq!(policy.reply_ttl_secs, 60);
        assert_eq!(policy.posts_per_hour, Limit::at(6), "a default");
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
