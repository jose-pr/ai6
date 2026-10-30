//! `ai6-source-jira`: the `jira` provider. As a Source it runs a JQL search for the issues
//! that carry the configured labels; as a Tracker it reads, comments on, labels, transitions
//! and creates issues of one project. Requests are blocking; the host calls them off its
//! async runtime.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

ai6::guatiao::providers!(Jira);

mod adf;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{Provider, Status};
use ai6::{
    IssueSpec, PostedComment, Reply, ReplyBatch, ReplyPoll, Source, SourceBatch, SourceConfig,
    SourceItem, SourcePoll, Tracker, TrackerCapabilities, TrackerIssue, TrackerLink, TrackerPost,
    TrackerUpdate,
};
use serde::Deserialize;
use serde::de::IgnoredAny;
use url::Url;

/// The provider ID an Operation names under `sources`.
pub const PROVIDER_ID: &str = "jira";

const FIELDS: &str = "summary,description,labels,reporter,updated";
const ISSUE_FIELDS: &str = "summary,description,labels,reporter,assignee,updated,status,parent";

/// Which Jira API the site speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Deployment {
    /// Jira Cloud: REST v3, ADF text, token-paged `search/jql`.
    Cloud,
    /// Jira Server or Data Center: REST v2, plain text, offset-paged `search`.
    Server,
}

/// How requests authenticate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Auth {
    /// `email:token` as HTTP Basic (Jira Cloud API tokens).
    Basic,
    /// A personal access token as a Bearer credential (Data Center).
    Bearer,
}

/// One instance's settings, from `[operation.<id>.sources.<name>]`.
///
/// The Station reads `provider`, `intake` and `interval_secs` itself and does not pass them
/// on. Keys the Station applies (status labels and transitions, agent labels, the priority
/// map, GitHub's reply associations) are accepted here so one table serves every tracker;
/// the provider ignores them.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    /// The site root, for example `https://example.atlassian.net`.
    site: String,
    /// The project key; a bare string under `sources` is this value.
    #[serde(alias = "target")]
    project: Option<String>,
    /// An extra JQL condition, combined with the project and label filters by AND.
    jql: Option<String>,
    /// An issue needs every one of these labels to become a Mission.
    #[serde(default = "default_labels")]
    labels: Vec<String>,
    #[serde(default = "default_deployment")]
    deployment: Deployment,
    #[serde(default = "default_auth")]
    auth: Auth,
    /// The environment variable holding the account email (Basic authentication only).
    email_env: Option<String>,
    /// The environment variable holding the API token.
    #[serde(default = "default_token_env")]
    token_env: String,
    /// Seconds one request may take. Default 30; 0 is refused.
    #[serde(default = "default_request_timeout_secs")]
    request_timeout_secs: u64,
    /// Issues asked for per page. Default 50; 0 is refused.
    #[serde(default = "default_page_size")]
    page_size: u32,
    /// Pages one poll reads at most. Default 5; 0 is refused.
    #[serde(default = "default_max_pages")]
    max_pages: usize,
    /// Account IDs (Cloud) or user names (Server), besides the issue's reporter and
    /// assignee, whose comments may answer a question.
    #[serde(default)]
    reply_authors: Vec<String>,
    /// The issue type a created issue has. Default `Task`.
    #[serde(default = "default_issue_type")]
    issue_type: String,
    /// The issue type of a child; default the project's first sub-task type.
    subtask_type: Option<String>,
    /// How long, in seconds, the project's answers are remembered. Default 600.
    #[serde(default = "default_capability_ttl_secs")]
    capability_ttl_secs: u64,
    #[serde(default, rename = "status_labels")]
    _status_labels: IgnoredAny,
    #[serde(default, rename = "status_transitions")]
    _status_transitions: IgnoredAny,
    #[serde(default, rename = "agent_labels")]
    _agent_labels: IgnoredAny,
    #[serde(default, rename = "agent_transitions")]
    _agent_transitions: IgnoredAny,
    #[serde(default, rename = "priority_map")]
    _priority_map: IgnoredAny,
    #[serde(default, rename = "reply_associations")]
    _reply_associations: IgnoredAny,
}

fn default_request_timeout_secs() -> u64 {
    30
}

fn default_page_size() -> u32 {
    50
}

fn default_max_pages() -> usize {
    5
}

fn default_capability_ttl_secs() -> u64 {
    600
}

fn default_issue_type() -> String {
    "Task".to_owned()
}

fn default_labels() -> Vec<String> {
    vec!["ai6-ready".to_owned()]
}

fn default_deployment() -> Deployment {
    Deployment::Cloud
}

fn default_auth() -> Auth {
    Auth::Basic
}

fn default_token_env() -> String {
    "JIRA_API_TOKEN".to_owned()
}

/// What the project's create metadata said, kept with the capabilities.
#[derive(Debug, Clone)]
struct Discovered {
    capabilities: TrackerCapabilities,
    /// The first sub-task issue type, when the project has one.
    subtask_type: Option<String>,
}

/// Jira issues as an AI6 Source and Tracker.
#[derive(Debug, Provider)]
#[provider(
    Source,
    Tracker,
    id = "jira",
    name = "Jira issues",
    config = SourceConfig,
    crate = ::ai6::guatiao
)]
pub struct Jira {
    settings: Settings,
    site: Url,
    agent: ureq::Agent,
    /// The project-level answers and when they were read.
    discovered: Mutex<Option<(Instant, Discovered)>>,
    /// The account the credential belongs to; `Some(None)` once Jira declined to say.
    identity: Mutex<Option<Option<String>>>,
}

impl Jira {
    /// Builds the provider from the instance's settings table, serialized as TOML.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown key, a missing project and JQL, a malformed key or
    /// an insecure site address.
    pub fn from_settings(settings_toml: &str) -> Result<Self, String> {
        let settings: Settings = toml::from_str(settings_toml).map_err(|e| e.to_string())?;
        if settings.project.is_none() && settings.jql.is_none() {
            return Err("set `project` or `jql`".to_owned());
        }
        if let Some(project) = &settings.project
            && (project.is_empty()
                || !project
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            return Err("project must be a Jira project key such as GUA".to_owned());
        }
        if settings.token_env.trim().is_empty() {
            return Err("token_env must name an environment variable".to_owned());
        }
        if settings.auth == Auth::Basic && settings.email_env.is_none() {
            return Err("auth = \"basic\" needs email_env (or use auth = \"bearer\")".to_owned());
        }
        let site =
            Url::parse(settings.site.trim_end_matches('/')).map_err(|e| format!("site: {e}"))?;
        let loopback = matches!(site.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if site.scheme() != "https" && !(site.scheme() == "http" && loopback) {
            return Err("site must be https (http only on a loopback address)".to_owned());
        }
        for (name, zero) in [
            ("request_timeout_secs", settings.request_timeout_secs == 0),
            ("page_size", settings.page_size == 0),
            ("max_pages", settings.max_pages == 0),
        ] {
            if zero {
                return Err(format!("{name} must be at least 1"));
            }
        }
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(settings.request_timeout_secs)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        Ok(Self {
            settings,
            site,
            agent,
            discovered: Mutex::new(None),
            identity: Mutex::new(None),
        })
    }

    fn api(&self, path: &str) -> String {
        let version = match self.settings.deployment {
            Deployment::Cloud => 3,
            Deployment::Server => 2,
        };
        format!(
            "{}/rest/api/{version}{path}",
            self.site.as_str().trim_end_matches('/')
        )
    }

    fn authorization(&self) -> Result<String, ProviderError> {
        let env = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    failure(&format!(
                        "no credential: set the environment variable {name}"
                    ))
                })
        };
        let token = env(&self.settings.token_env)?;
        Ok(match (self.settings.auth, &self.settings.email_env) {
            (Auth::Basic, Some(email_env)) => {
                let pair = format!("{}:{token}", env(email_env)?);
                format!("Basic {}", data_encoding::BASE64.encode(pair.as_bytes()))
            }
            _ => format!("Bearer {token}"),
        })
    }

    fn call(
        &self,
        method: &str,
        url: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<ureq::http::Response<ureq::Body>, ProviderError> {
        let mut url = Url::parse(url).map_err(|e| failure(&format!("bad address: {e}")))?;
        for (key, value) in query {
            url.query_pairs_mut().append_pair(key, value);
        }
        let mut request = ureq::http::Request::builder()
            .method(method)
            .uri(url.as_str())
            .header("Accept", "application/json")
            .header("Authorization", self.authorization()?);
        let bytes = match body {
            Some(body) => {
                request = request.header("Content-Type", "application/json");
                serde_json::to_vec(body).map_err(|e| failure(&e.to_string()))?
            }
            None => Vec::new(),
        };
        let request = request
            .body(bytes)
            .map_err(|e| failure(&format!("bad request: {e}")))?;
        let response = self
            .agent
            .run(request)
            .map_err(|e| failure(&format!("request failed: {e}")))?;
        self.check(&response)?;
        Ok(response)
    }

    fn get(
        &self,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<ureq::http::Response<ureq::Body>, ProviderError> {
        self.call("GET", url, query, None)
    }

    fn get_json(
        &self,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<serde_json::Value, ProviderError> {
        read_json(self.get(url, query)?)
    }

    fn check(&self, response: &ureq::http::Response<ureq::Body>) -> Result<(), ProviderError> {
        let status = response.status().as_u16();
        match status {
            200..=299 => Ok(()),
            429 => Err(failure(&format!(
                "Jira rate limit reached; retry after {}s",
                response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("60")
            ))),
            401 | 403 => Err(failure(&format!(
                "Jira refused the credential in {}",
                self.settings.token_env
            ))),
            404 => Err(failure(
                "Jira does not show this issue or endpoint to the credential",
            )),
            400 => Err(failure(
                "Jira rejected the request (check `jql`, `project`, the labels and the fields)",
            )),
            other => Err(failure(&format!("Jira answered status {other}"))),
        }
    }

    /// The JQL for one poll: the configured filters, then everything changed since the cursor.
    fn jql(&self, cursor: Option<&str>) -> String {
        let mut clauses = Vec::new();
        if let Some(project) = &self.settings.project {
            clauses.push(format!("project = \"{project}\""));
        }
        for label in &self.settings.labels {
            clauses.push(format!("labels = \"{}\"", label.replace(['"', '\\'], "")));
        }
        if let Some(jql) = &self.settings.jql {
            clauses.push(format!("({jql})"));
        }
        clauses.push("statusCategory != Done".to_owned());
        if let Some(since) = cursor.and_then(jql_minute) {
            clauses.push(format!("updated >= \"{since}\""));
        }
        format!("{} ORDER BY updated ASC", clauses.join(" AND "))
    }

    /// The issue key of `item`, which must be well formed and in this instance's project.
    fn key<'a>(&self, item: &'a str) -> Result<&'a str, ProviderError> {
        let well_formed = item.split_once('-').is_some_and(|(project, number)| {
            !project.is_empty()
                && project
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !number.is_empty()
                && number.bytes().all(|b| b.is_ascii_digit())
        });
        if !well_formed {
            return Err(bad_value(&format!("{item:?} is not a Jira issue key")));
        }
        if let Some(project) = &self.settings.project
            && item.split_once('-').map(|(prefix, _)| prefix) != Some(project.as_str())
        {
            return Err(bad_value(&format!(
                "foreign item {item:?}: this instance serves project {project}"
            )));
        }
        Ok(item)
    }

    fn text(&self, body: &str) -> serde_json::Value {
        match self.settings.deployment {
            Deployment::Cloud => adf::document(body),
            Deployment::Server => serde_json::Value::String(body.to_owned()),
        }
    }

    /// The transitions open on `key`: ID, name and the category of the status it leads to.
    fn transitions(&self, key: &str) -> Result<Vec<(String, String, String)>, ProviderError> {
        let listing = self.get_json(&self.api(&format!("/issue/{key}/transitions")), &[])?;
        Ok(listing
            .get("transitions")
            .and_then(serde_json::Value::as_array)
            .map(|transitions| {
                transitions
                    .iter()
                    .filter_map(|transition| {
                        Some((
                            transition.get("id")?.as_str()?.to_owned(),
                            transition.get("name")?.as_str()?.to_owned(),
                            transition
                                .pointer("/to/statusCategory/key")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn transition(&self, key: &str, wanted: &str) -> Result<(), ProviderError> {
        let transitions = self.transitions(key)?;
        let id = transitions
            .iter()
            .find_map(|(id, name, _)| {
                (id == wanted || name.eq_ignore_ascii_case(wanted)).then_some(id)
            })
            .ok_or_else(|| {
                unsupported(
                    &format!("make the transition {wanted:?} on {key}"),
                    &format!(
                        "make {}",
                        transitions
                            .iter()
                            .map(|(_, name, _)| name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?;
        self.call(
            "POST",
            &self.api(&format!("/issue/{key}/transitions")),
            &[],
            Some(&serde_json::json!({ "transition": { "id": id } })),
        )?;
        Ok(())
    }

    /// The account the credential belongs to, asked once.
    fn own_account(&self) -> Option<String> {
        let mut identity = self.identity.lock().ok()?;
        if identity.is_none() {
            let account = self
                .get_json(&self.api("/myself"), &[])
                .ok()
                .and_then(|me| person_id(&me));
            *identity = Some(account);
        }
        identity.clone().flatten()
    }

    fn discover(&self) -> Result<Discovered, ProviderError> {
        let ttl = Duration::from_secs(self.settings.capability_ttl_secs);
        if let Ok(cache) = self.discovered.lock()
            && let Some((at, discovered)) = cache.as_ref()
            && at.elapsed() < ttl
        {
            return Ok(discovered.clone());
        }
        let mut issue_types = Vec::new();
        let mut subtask_type = None;
        let mut create = false;
        if let Some(project) = &self.settings.project {
            let meta = self.get_json(
                &self.api(&format!("/issue/createmeta/{project}/issuetypes")),
                &[],
            );
            if let Ok(meta) = meta {
                let types = meta
                    .get("issueTypes")
                    .or_else(|| meta.get("values"))
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                create = !types.is_empty();
                for kind in types {
                    let Some(name) = kind.get("name").and_then(serde_json::Value::as_str) else {
                        continue;
                    };
                    if kind.get("subtask").and_then(serde_json::Value::as_bool) == Some(true) {
                        subtask_type.get_or_insert_with(|| name.to_owned());
                    } else {
                        issue_types.push(name.to_owned());
                    }
                }
            }
        }
        let priorities = self
            .get_json(&self.api("/priority"), &[])
            .ok()
            .and_then(|list| {
                Some(
                    list.as_array()?
                        .iter()
                        .filter_map(|p| p.get("name")?.as_str().map(str::to_owned))
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();
        let children = match (self.settings.deployment, &subtask_type) {
            (Deployment::Cloud, _) => "parent",
            (Deployment::Server, Some(_)) => "subtasks",
            (Deployment::Server, None) => "none",
        };
        let discovered = Discovered {
            capabilities: TrackerCapabilities {
                comment: true,
                edit_comment: true,
                labels: Vec::new(),
                free_labels: true,
                transitions: Vec::new(),
                assign: true,
                close: true,
                create,
                issue_types,
                priorities,
                children: children.to_owned(),
                links: vec!["issue_links".to_owned(), "remote_links".to_owned()],
                replies: true,
                visibility: "unknown".to_owned(),
                markers: false,
            },
            subtask_type,
        };
        if let Ok(mut cache) = self.discovered.lock() {
            *cache = Some((Instant::now(), discovered.clone()));
        }
        Ok(discovered)
    }
}

impl TryFrom<SourceConfig> for Jira {
    type Error = ProviderError;

    fn try_from(config: SourceConfig) -> Result<Self, Self::Error> {
        Self::from_settings(&config.settings_toml)
            .map_err(|e| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &e))
    }
}

impl Source for Jira {
    fn poll(&self, request: SourcePoll) -> Result<SourceBatch, ProviderError> {
        let jql = self.jql(request.cursor.as_deref());
        let url = match self.settings.deployment {
            Deployment::Cloud => self.api("/search/jql"),
            Deployment::Server => self.api("/search"),
        };
        let query: Vec<(&str, String)> = vec![
            ("jql", jql),
            ("fields", FIELDS.to_owned()),
            ("maxResults", self.settings.page_size.to_string()),
        ];

        let mut items = Vec::new();
        let mut page_token: Option<String> = None;
        let mut start_at = 0_u64;
        for _ in 0..self.settings.max_pages {
            let mut page_query = query.clone();
            match self.settings.deployment {
                Deployment::Cloud => {
                    if let Some(token) = &page_token {
                        page_query.push(("nextPageToken", token.clone()));
                    }
                }
                Deployment::Server => page_query.push(("startAt", start_at.to_string())),
            }
            let mut response = self.get(&url, &page_query)?;
            let body: serde_json::Value = response
                .body_mut()
                .read_json()
                .map_err(|e| failure(&format!("unreadable response: {e}")))?;
            let issues = body
                .get("issues")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            items.extend(issues.iter().filter_map(|issue| self.item_of(issue)));
            let more = match self.settings.deployment {
                Deployment::Cloud => {
                    page_token = body
                        .get("nextPageToken")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    page_token.is_some()
                }
                Deployment::Server => {
                    start_at += issues.len() as u64;
                    let total = body.get("total").and_then(serde_json::Value::as_u64);
                    !issues.is_empty() && total.is_some_and(|total| start_at < total)
                }
            };
            if !more {
                break;
            }
        }
        let cursor = items
            .iter()
            .map(|item| item.updated.as_str())
            .max()
            .map(str::to_owned)
            .or(request.cursor);
        Ok(SourceBatch { items, cursor })
    }
}

impl Tracker for Jira {
    fn capabilities(&self, item: Option<String>) -> Result<TrackerCapabilities, ProviderError> {
        let mut capabilities = self.discover()?.capabilities;
        if let Some(item) = item {
            let key = self.key(&item)?;
            capabilities.transitions = self
                .transitions(key)?
                .into_iter()
                .map(|(_, name, _)| name)
                .collect();
        }
        Ok(capabilities)
    }

    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError> {
        let key = self.key(&item)?;
        let issue = self.get_json(
            &self.api(&format!("/issue/{key}")),
            &[("fields", ISSUE_FIELDS.to_owned())],
        )?;
        self.issue_of(&issue)
            .ok_or_else(|| failure("Jira answered with an issue this provider cannot read"))
    }

    fn replies(&self, request: ReplyPoll) -> Result<ReplyBatch, ProviderError> {
        let keys: Vec<&str> = request
            .items
            .iter()
            .map(|item| self.key(item))
            .collect::<Result<_, _>>()?;
        let mut cursor = request.since.clone();
        let mut replies = Vec::new();
        if keys.is_empty() {
            return Ok(ReplyBatch { replies, cursor });
        }
        let own = self.own_account();
        for key in keys {
            let mut who: Option<Vec<String>> = None;
            let mut start_at = 0_u64;
            for _ in 0..self.settings.max_pages {
                let page = self.get_json(
                    &self.api(&format!("/issue/{key}/comment")),
                    &[
                        ("orderBy", "created".to_owned()),
                        ("startAt", start_at.to_string()),
                        ("maxResults", self.settings.page_size.to_string()),
                    ],
                )?;
                let comments = page
                    .get("comments")
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                for comment in comments {
                    let text = |value: &serde_json::Value, key: &str| {
                        value
                            .get(key)
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    };
                    let updated = text(comment, "updated")
                        .or_else(|| text(comment, "created"))
                        .unwrap_or_default();
                    if request
                        .since
                        .as_deref()
                        .is_some_and(|since| updated.as_str() <= since)
                    {
                        continue;
                    }
                    if cursor.as_deref().is_none_or(|c| updated.as_str() > c) {
                        cursor = Some(updated.clone());
                    }
                    let author = comment.get("author").cloned().unwrap_or_default();
                    let author_id = person_id(&author).unwrap_or_default();
                    let permitted = self.settings.reply_authors.contains(&author_id) || {
                        if who.is_none() {
                            who = Some(self.reporter_and_assignee(key));
                        }
                        who.as_deref()
                            .is_some_and(|people| people.contains(&author_id))
                    };
                    let body = match comment.get("body") {
                        Some(serde_json::Value::String(text)) => text.clone(),
                        Some(document @ serde_json::Value::Object(_)) => adf::text(document),
                        _ => String::new(),
                    };
                    replies.push(Reply {
                        id: comment
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        item: key.to_owned(),
                        permitted: permitted && !author_id.is_empty(),
                        own: own.as_deref() == Some(author_id.as_str()) && !author_id.is_empty(),
                        author_id,
                        author: text(&author, "displayName").unwrap_or_default(),
                        body,
                        created: text(comment, "created").unwrap_or_default(),
                    });
                }
                start_at += comments.len() as u64;
                let total = page.get("total").and_then(serde_json::Value::as_u64);
                if comments.is_empty() || total.is_none_or(|total| start_at >= total) {
                    break;
                }
            }
        }
        replies.sort_by(|a, b| a.created.cmp(&b.created));
        Ok(ReplyBatch { replies, cursor })
    }

    fn post(&self, post: TrackerPost) -> Result<PostedComment, ProviderError> {
        let key = self.key(&post.item)?;
        let payload = serde_json::json!({ "body": self.text(&post.body) });
        let comment = match &post.edit {
            Some(id) => {
                if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad_value("a comment ID is a number"));
                }
                read_json(self.call(
                    "PUT",
                    &self.api(&format!("/issue/{key}/comment/{id}")),
                    &[],
                    Some(&payload),
                )?)?
            }
            None => read_json(self.call(
                "POST",
                &self.api(&format!("/issue/{key}/comment")),
                &[],
                Some(&payload),
            )?)?,
        };
        let id = comment
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| failure("Jira answered without a comment ID"))?
            .to_owned();
        Ok(PostedComment {
            url: Some(format!(
                "{}/browse/{key}?focusedCommentId={id}",
                self.site.as_str().trim_end_matches('/')
            )),
            id,
        })
    }

    fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError> {
        let key = self.key(&update.item)?;
        if !update.add_labels.is_empty() || !update.remove_labels.is_empty() {
            let edits: Vec<serde_json::Value> = update
                .add_labels
                .iter()
                .map(|label| serde_json::json!({ "add": jira_label(label) }))
                .chain(
                    update
                        .remove_labels
                        .iter()
                        .map(|label| serde_json::json!({ "remove": jira_label(label) })),
                )
                .collect();
            self.call(
                "PUT",
                &self.api(&format!("/issue/{key}")),
                &[],
                Some(&serde_json::json!({ "update": { "labels": edits } })),
            )?;
        }
        if update.add_assignees.len() > 1 {
            return Err(bad_value("a Jira issue has one assignee"));
        }
        let assignee_field = match self.settings.deployment {
            Deployment::Cloud => "accountId",
            Deployment::Server => "name",
        };
        if let Some(person) = update.add_assignees.first() {
            self.call(
                "PUT",
                &self.api(&format!("/issue/{key}/assignee")),
                &[],
                Some(&serde_json::json!({ (assignee_field): person })),
            )?;
        } else if !update.remove_assignees.is_empty() {
            self.call(
                "PUT",
                &self.api(&format!("/issue/{key}/assignee")),
                &[],
                Some(&serde_json::json!({ (assignee_field): null })),
            )?;
        }
        if let Some(transition) = &update.transition {
            self.transition(key, transition)?;
        }
        match update.state.as_deref() {
            None => {}
            Some(state @ ("closed" | "open")) => {
                let wanted: &[&str] = if state == "closed" {
                    &["done"]
                } else {
                    &["new", "indeterminate"]
                };
                let transitions = self.transitions(key)?;
                let id = transitions
                    .iter()
                    .find_map(|(id, _, category)| wanted.contains(&category.as_str()).then_some(id))
                    .ok_or_else(|| {
                        unsupported(
                            &format!("set {key} to {state}"),
                            &format!(
                                "make {}",
                                transitions
                                    .iter()
                                    .map(|(_, name, _)| name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        )
                    })?;
                self.call(
                    "POST",
                    &self.api(&format!("/issue/{key}/transitions")),
                    &[],
                    Some(&serde_json::json!({ "transition": { "id": id } })),
                )?;
            }
            Some(other) => {
                return Err(bad_value(&format!(
                    "state {other:?} is not `open` or `closed`"
                )));
            }
        }
        Ok(())
    }

    fn create(&self, spec: IssueSpec) -> Result<TrackerIssue, ProviderError> {
        let Some(project) = &self.settings.project else {
            return Err(unsupported(
                "create issues without a project",
                "read and comment on them; set `project` to create",
            ));
        };
        let discovered = self.discover()?;
        if !discovered.capabilities.create {
            return Err(unsupported(
                "create issues in this project",
                "read and comment on them",
            ));
        }
        let parent = spec
            .parent
            .as_deref()
            .map(|parent| self.key(parent))
            .transpose()?;
        let child_type = self
            .settings
            .subtask_type
            .clone()
            .or(discovered.subtask_type.clone());
        let kind = match (parent, self.settings.deployment) {
            (Some(_), Deployment::Server) => child_type.ok_or_else(|| {
                unsupported(
                    "create a child without a sub-task type",
                    "create issues without a parent",
                )
            })?,
            (Some(_), Deployment::Cloud) => child_type
                .or(spec.issue_type.clone())
                .unwrap_or_else(|| self.settings.issue_type.clone()),
            (None, _) => spec
                .issue_type
                .clone()
                .unwrap_or_else(|| self.settings.issue_type.clone()),
        };
        let mut fields = serde_json::json!({
            "project": { "key": project },
            "summary": spec.title.lines().next().unwrap_or_default(),
            "description": self.text(&spec.body),
            "issuetype": { "name": kind },
            "labels": spec.labels.iter().map(|label| jira_label(label)).collect::<Vec<_>>(),
        });
        let priorities = &discovered.capabilities.priorities;
        if let Some(priority) = spec
            .priority
            .as_ref()
            .filter(|priority| priorities.is_empty() || priorities.contains(priority))
        {
            fields["priority"] = serde_json::json!({ "name": priority });
        }
        if let Some(parent) = parent {
            fields["parent"] = serde_json::json!({ "key": parent });
        }
        let created = read_json(self.call(
            "POST",
            &self.api("/issue"),
            &[],
            Some(&serde_json::json!({ "fields": fields })),
        )?)?;
        let key = created
            .get("key")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| failure("Jira answered without an issue key"))?;
        self.issue(key.to_owned())
    }

    fn link(&self, link: TrackerLink) -> Result<(), ProviderError> {
        let from = self.key(&link.from)?;
        match (&link.to, &link.url) {
            (Some(to), _) => {
                let to = self.key(to)?;
                self.call(
                    "POST",
                    &self.api("/issueLink"),
                    &[],
                    Some(&serde_json::json!({
                        "type": { "name": "Relates" },
                        "inwardIssue": { "key": from },
                        "outwardIssue": { "key": to },
                    })),
                )?;
            }
            (None, Some(url)) => {
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err(bad_value("a remote link is an http or https address"));
                }
                self.call(
                    "POST",
                    &self.api(&format!("/issue/{from}/remotelink")),
                    &[],
                    Some(&serde_json::json!({
                        "relationship": link.relation,
                        "object": { "url": url, "title": url },
                    })),
                )?;
            }
            (None, None) => return Err(bad_value("a link names an issue or an address")),
        }
        Ok(())
    }
}

fn read_json(
    mut response: ureq::http::Response<ureq::Body>,
) -> Result<serde_json::Value, ProviderError> {
    response
        .body_mut()
        .read_json()
        .map_err(|e| failure(&format!("unreadable response: {e}")))
}

/// A person's stable ID: the Cloud account ID, else the Server user key or name.
fn person_id(person: &serde_json::Value) -> Option<String> {
    ["accountId", "key", "name"].iter().find_map(|key| {
        person
            .get(key)?
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    })
}

/// Jira labels cannot hold whitespace.
fn jira_label(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join("-")
}

impl Jira {
    /// The reporter's and assignee's IDs: the people who may answer for an issue.
    fn reporter_and_assignee(&self, key: &str) -> Vec<String> {
        self.get_json(
            &self.api(&format!("/issue/{key}")),
            &[("fields", "reporter,assignee".to_owned())],
        )
        .ok()
        .map(|issue| {
            ["reporter", "assignee"]
                .iter()
                .filter_map(|field| person_id(issue.pointer(&format!("/fields/{field}"))?))
                .collect()
        })
        .unwrap_or_default()
    }

    fn issue_of(&self, issue: &serde_json::Value) -> Option<TrackerIssue> {
        let item = self.item_of(issue)?;
        let fields = issue.get("fields")?;
        let done = fields
            .pointer("/status/statusCategory/key")
            .and_then(serde_json::Value::as_str)
            == Some("done");
        Some(TrackerIssue {
            id: item.id,
            native_id: issue
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            title: item.title,
            body: item.body,
            state: if done { "closed" } else { "open" }.to_owned(),
            labels: item.labels,
            assignees: fields
                .get("assignee")
                .and_then(person_id)
                .into_iter()
                .collect(),
            url: item.url,
            updated: item.updated,
            parent: fields
                .pointer("/parent/key")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            author_id: fields.get("reporter").and_then(person_id),
        })
    }
}

fn unsupported(what: &str, can: &str) -> ProviderError {
    ProviderError::new(
        Status::GUATIAO_ERR_UNSUPPORTED,
        &format!("Jira cannot {what}; it can {can}"),
    )
}

fn bad_value(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

impl Jira {
    fn item_of(&self, issue: &serde_json::Value) -> Option<SourceItem> {
        let key = issue.get("key")?.as_str()?;
        let fields = issue.get("fields")?;
        let description = match fields.get("description") {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(document @ serde_json::Value::Object(_)) => adf::text(document),
            _ => String::new(),
        };
        Some(SourceItem {
            id: key.to_owned(),
            title: fields.get("summary")?.as_str()?.to_owned(),
            body: description,
            url: Some(format!(
                "{}/browse/{key}",
                self.site.as_str().trim_end_matches('/')
            )),
            labels: fields
                .get("labels")
                .and_then(serde_json::Value::as_array)
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(|label| label.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            author: fields
                .get("reporter")
                .and_then(|reporter| reporter.get("displayName"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            updated: fields.get("updated")?.as_str()?.to_owned(),
        })
    }
}

/// Turns Jira's `2026-10-02T09:30:00.000+0000` into the `2026-10-02 09:30` JQL accepts.
///
/// JQL compares at minute precision in the caller's time zone, which is the zone Jira
/// already used for the timestamp, so the date and time are kept as written.
fn jql_minute(updated: &str) -> Option<String> {
    let minute = updated.get(..16)?;
    let (date, time) = minute.split_once('T')?;
    let valid = date.len() == 10
        && time.len() == 5
        && date
            .bytes()
            .chain(time.bytes())
            .all(|b| b.is_ascii_digit() || b == b'-' || b == b':');
    valid.then(|| format!("{date} {time}"))
}

fn failure(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_become_minute_precision_jql_times() {
        assert_eq!(
            jql_minute("2026-10-02T09:30:45.123+0000").as_deref(),
            Some("2026-10-02 09:30")
        );
        assert_eq!(jql_minute("garbage"), None);
        assert_eq!(jql_minute("2026-10-02"), None);
    }

    #[test]
    fn settings_are_checked_before_any_request() {
        let ok = "site = \"https://x.atlassian.net\"\nproject = \"GUA\"\nemail_env = \"E\"";
        assert!(Jira::from_settings(ok).is_ok());
        assert!(Jira::from_settings(&ok.replace("project", "target")).is_ok());
        assert!(Jira::from_settings(&format!("{ok}\nsurprise = 1")).is_err());
        assert!(
            Jira::from_settings("site = \"https://x.atlassian.net\"\nemail_env = \"E\"").is_err()
        );
        assert!(Jira::from_settings(&ok.replace("GUA", "G\\\" OR 1=1")).is_err());
        assert!(Jira::from_settings("site = \"https://x\"\nproject = \"G\"").is_err());
        assert!(
            Jira::from_settings("site = \"https://x\"\nproject = \"G\"\nauth = \"bearer\"").is_ok()
        );
        assert!(
            Jira::from_settings(
                "site = \"http://example.com\"\nproject = \"G\"\nauth = \"bearer\""
            )
            .is_err()
        );
    }

    #[test]
    fn the_search_is_filtered_by_project_labels_and_cursor() {
        let jira = Jira::from_settings(
            "site = \"https://x\"\nproject = \"GUA\"\nauth = \"bearer\"\njql = \"component = Core\"",
        )
        .unwrap();
        assert_eq!(
            jira.jql(Some("2026-10-02T09:30:45.123+0000")),
            "project = \"GUA\" AND labels = \"ai6-ready\" AND (component = Core) AND \
             statusCategory != Done AND updated >= \"2026-10-02 09:30\" ORDER BY updated ASC"
        );
    }
}
