//! `ai6-source-jira`: the `jira` provider. As a Source it runs a JQL search for the issues
//! that carry the configured labels; as a Tracker it reads, comments on, labels, transitions
//! and creates issues of one project. Requests are blocking; the host calls them off its
//! async runtime.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

ai6::guatiao::providers!(Jira);

mod adf;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use ai6::{
    IssueSpec, PostedComment, Reply, ReplyBatch, ReplyPoll, Source, SourceBatch, SourceConfig,
    SourceItem, SourcePoll, Tracker, TrackerCapabilities, TrackerIssue, TrackerLink, TrackerPost,
    TrackerUpdate, settings_from_table, settled_cursor,
};
use url::Url;

/// The provider ID an Operation names under `sources`.
pub const PROVIDER_ID: &str = "jira";

const FIELDS: &str = "summary,description,labels,reporter,updated";
const ISSUE_FIELDS: &str =
    "summary,description,labels,reporter,assignee,updated,status,parent,priority";
/// The longest summary Jira takes.
const SUMMARY_BYTES: usize = 255;

/// Which Jira API the site speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::ai6::guatiao, rename_all = "lowercase")]
enum Deployment {
    /// Jira Cloud: REST v3, ADF text, token-paged `search/jql`.
    Cloud,
    /// Jira Server or Data Center: REST v2, plain text, offset-paged `search`.
    Server,
}

/// How requests authenticate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::ai6::guatiao, rename_all = "lowercase")]
enum Auth {
    /// `email:token` as HTTP Basic (Jira Cloud API tokens).
    Basic,
    /// A personal access token as a Bearer credential (Data Center).
    Bearer,
}

/// One instance's settings, from `[operation.<id>.sources.<name>]`.
///
/// The Station reads its own keys (`provider`, `intake`, `interval_secs`, the status maps and
/// what Agents may touch) and passes the rest here.
#[derive(Debug, Clone, ToValue, FromValue, Schema)]
#[map(crate = ::ai6::guatiao, deny_unknown_fields)]
struct Settings {
    /// The site root, for example `https://example.atlassian.net`.
    #[schema(title = "Site")]
    site: String,
    /// A PEM file of certificate authorities to trust besides the public roots, by absolute path.
    /// A set one is reported as a weakening: more authorities can vouch for this service.
    #[schema(title = "Certificate authorities")]
    ca_file: Option<PathBuf>,
    /// The project key; a bare string under `sources` is this value.
    #[schema(title = "Project")]
    project: Option<String>,
    /// An extra JQL condition, combined with the project and label filters by AND.
    #[schema(title = "Extra JQL")]
    jql: Option<String>,
    /// An issue needs every one of these labels to become a Mission.
    #[schema(title = "Intake labels")]
    #[map(default = vec!["ai6-ready".to_owned()])]
    labels: Vec<String>,
    /// Which API the site speaks.
    #[schema(title = "Deployment")]
    #[map(default = Deployment::Cloud)]
    deployment: Deployment,
    /// How requests authenticate.
    #[schema(title = "Authentication")]
    #[map(default = Auth::Basic)]
    auth: Auth,
    /// The environment variable holding the account email (Basic authentication only).
    #[schema(title = "Email variable")]
    email_env: Option<String>,
    /// The environment variable holding the API token.
    #[schema(title = "Token variable")]
    #[map(default = "JIRA_API_TOKEN".to_owned())]
    token_env: String,
    /// Seconds one request may take. 0 is refused.
    #[schema(title = "A request may take", format = "seconds")]
    #[map(default = 30)]
    request_timeout_secs: u64,
    /// Issues asked for per page. 0 is refused.
    #[schema(title = "Issues per page")]
    #[map(default = 50)]
    page_size: u32,
    /// Pages one poll reads at most. 0 is refused.
    #[schema(title = "Pages per poll")]
    #[map(default = 5)]
    max_pages: u64,
    /// Seconds a poll searches behind the cursor, for an issue the search showed after a newer
    /// one. 0 searches from the cursor's minute.
    #[schema(title = "Search behind the cursor by", format = "seconds")]
    #[map(default = 60)]
    settling_secs: u64,
    /// Account IDs (Cloud) or user names (Server), besides the issue's reporter and
    /// assignee, whose comments may answer a question.
    #[schema(title = "People who may answer")]
    #[map(default)]
    reply_authors: Vec<String>,
    /// The issue type a created issue has.
    #[schema(title = "Type of a new issue")]
    #[map(default = "Task".to_owned())]
    issue_type: String,
    /// The issue type of a child; default the project's first sub-task type.
    #[schema(title = "Type of a child")]
    subtask_type: Option<String>,
    /// How long, in seconds, the project's answers are remembered.
    #[schema(title = "Remember the project for", format = "seconds")]
    #[map(default = 600)]
    capability_ttl_secs: u64,
    /// The issue link type that ties a child to a parent it cannot sit under.
    #[schema(title = "Link type")]
    #[map(default = "Relates".to_owned())]
    link_type: String,
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
}

impl Jira {
    /// Builds the provider from the instance's settings table, serialized as TOML.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown key, a missing project and JQL, a malformed key or
    /// an insecure site address.
    pub fn from_settings(settings_toml: &str) -> Result<Self, String> {
        let mut table: toml::Table = toml::from_str(settings_toml).map_err(|e| e.to_string())?;
        if let Some(target) = table.remove("target") {
            table.insert("project".to_owned(), target);
        }
        let settings: Settings = settings_from_table(table)?;
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
        let mut agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(settings.request_timeout_secs)))
            .max_redirects(0)
            .http_status_as_error(false);
        if let Some(ca_file) = &settings.ca_file {
            agent = agent.tls_config(ai6::tls::trusting_file(ca_file)?);
        }
        let agent = agent.build().into();
        Ok(Self {
            settings,
            site,
            agent,
            discovered: Mutex::new(None),
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
        rejected: &str,
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
        self.check(&response, rejected)?;
        Ok(response)
    }

    fn get(
        &self,
        url: &str,
        query: &[(&str, String)],
        rejected: &str,
    ) -> Result<ureq::http::Response<ureq::Body>, ProviderError> {
        self.call("GET", url, query, None, rejected)
    }

    fn get_json(
        &self,
        url: &str,
        query: &[(&str, String)],
        rejected: &str,
    ) -> Result<serde_json::Value, ProviderError> {
        read_json(self.get(url, query, rejected)?)
    }

    /// `rejected` is the message for a 400, which Jira gives for what was asked, not for who
    /// asked.
    fn check(
        &self,
        response: &ureq::http::Response<ureq::Body>,
        rejected: &str,
    ) -> Result<(), ProviderError> {
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
            // Cloud answers a refused credential on an issue with 404, not 401.
            404 => Err(failure(&format!(
                "Jira does not show this issue or endpoint to the credential in {}",
                self.settings.token_env
            ))),
            410 => Err(failure(
                "Jira has removed this API (check `deployment`: Cloud has no REST v2 search)",
            )),
            400 => Err(failure(rejected)),
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
        let listing = self.get_json(
            &self.api(&format!("/issue/{key}/transitions")),
            &[],
            &format!("Jira rejected listing the transitions of {key}"),
        )?;
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
            &format!("Jira rejected the transition {wanted:?} on {key}"),
        )?;
        Ok(())
    }

    /// Tells an empty search from a search that could see nothing.
    ///
    /// Cloud answers a search by a refused credential, or naming a project the credential
    /// cannot see, with status 200 and no issues, so a revoked token would otherwise look
    /// like no work for as long as it stays revoked. The project lookup answers 404 to both.
    fn confirm_empty_search(&self) -> Result<(), ProviderError> {
        let token_env = &self.settings.token_env;
        let (url, unseen) = match &self.settings.project {
            Some(project) => (
                self.api(&format!("/project/{project}")),
                format!(
                    "Jira shows no project {project} to the credential in {token_env}: \
                     the credential is refused or the key is wrong"
                ),
            ),
            None => (
                self.api("/myself"),
                format!("Jira refused the credential in {token_env}"),
            ),
        };
        let response = self
            .agent
            .get(&url)
            .header("Accept", "application/json")
            .header("Authorization", &self.authorization()?)
            .call()
            .map_err(|e| failure(&format!("request failed: {e}")))?;
        match response.status().as_u16() {
            404 => Err(failure(&unseen)),
            _ => self.check(&response, &unseen),
        }
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
                &format!("Jira rejected asking for the issue types of {project}"),
            );
            if meta.is_err() {
                // A refused credential is an error here, not "this project cannot create".
                self.confirm_empty_search()?;
            }
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
            .get_json(
                &self.api("/priority"),
                &[],
                "Jira rejected listing the priorities",
            )
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
                markers: true,
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

impl Jira {
    /// The issues changed since `since`, oldest change first, and whether more remain past the
    /// pages one poll reads.
    fn changed_issues(
        &self,
        since: Option<&str>,
    ) -> Result<(Vec<SourceItem>, bool), ProviderError> {
        let jql = self.jql(since);
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
        let mut remaining = false;
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
            let mut response = self.get(
                &url,
                &page_query,
                "Jira rejected the search (check `jql`, `project` and the labels)",
            )?;
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
            remaining = more;
            if !more {
                break;
            }
        }
        Ok((items, remaining))
    }
}

impl Source for Jira {
    fn poll(&self, request: SourcePoll) -> Result<SourceBatch, ProviderError> {
        let since = request.cursor.as_deref().map(|cursor| {
            settled_cursor(cursor, self.settings.settling_secs).unwrap_or_else(|| cursor.to_owned())
        });
        let (mut items, more) = self.changed_issues(since.as_deref())?;
        // Pages filled by what the margin repeats would keep every poll from what follows them.
        if more
            && since != request.cursor
            && !items.iter().any(|item| {
                request
                    .cursor
                    .as_deref()
                    .is_some_and(|cursor| is_after(&item.updated, cursor))
            })
        {
            items = self.changed_issues(request.cursor.as_deref())?.0;
        }
        if items.is_empty() && self.settings.deployment == Deployment::Cloud {
            self.confirm_empty_search()?;
        }
        // What the margin repeats is older than the cursor, which never moves back for it.
        let mut cursor = request.cursor;
        for item in &items {
            if cursor
                .as_deref()
                .is_none_or(|current| is_after(&item.updated, current))
            {
                cursor = Some(item.updated.clone());
            }
        }
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
            &format!("Jira rejected reading {key}"),
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
        for key in keys {
            let mut who: Option<Vec<String>> = None;
            let mut start_at = 0_u64;
            for _ in 0..self.settings.max_pages {
                let page = self.get_json(
                    &self.api(&format!("/issue/{key}/comment")),
                    &[
                        ("orderBy", "created".to_owned()),
                        ("expand", "properties".to_owned()),
                        ("startAt", start_at.to_string()),
                        ("maxResults", self.settings.page_size.to_string()),
                    ],
                    &format!("Jira rejected listing the comments on {key}"),
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
                        .is_some_and(|since| !is_after(&updated, since))
                    {
                        continue;
                    }
                    if cursor.as_deref().is_none_or(|c| is_after(&updated, c)) {
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
                        own: is_ai6_comment(comment),
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
        if post.body.trim().is_empty() {
            return Err(bad_value(
                "the comment is empty, and Jira refuses such a comment",
            ));
        }
        let rejected = format!("Jira rejected the comment on {key}");
        let comment = match &post.edit {
            Some(id) => {
                if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad_value("a comment ID is a number"));
                }
                // The property stays through an edit that does not name it.
                read_json(self.call(
                    "PUT",
                    &self.api(&format!("/issue/{key}/comment/{id}")),
                    &[],
                    Some(&serde_json::json!({ "body": self.text(&post.body) })),
                    &rejected,
                )?)?
            }
            None => read_json(self.call(
                "POST",
                &self.api(&format!("/issue/{key}/comment")),
                &[],
                Some(&serde_json::json!({
                    "body": self.text(&post.body),
                    "properties": [{
                        "key": COMMENT_PROPERTY,
                        "value": { "marker": post.marker.as_deref().unwrap_or("comment") },
                    }],
                })),
                &rejected,
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
                &format!("Jira rejected the labels of {key}"),
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
                &format!("Jira rejected the assignee of {key}"),
            )?;
        } else if !update.remove_assignees.is_empty() {
            self.call(
                "PUT",
                &self.api(&format!("/issue/{key}/assignee")),
                &[],
                Some(&serde_json::json!({ (assignee_field): null })),
                &format!("Jira rejected clearing the assignee of {key}"),
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
                    &format!("Jira rejected the transition to {state} on {key}"),
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
        let standard = spec
            .issue_type
            .clone()
            .unwrap_or_else(|| self.settings.issue_type.clone());
        let child_type = self
            .settings
            .subtask_type
            .clone()
            .or(discovered.subtask_type.clone());
        // What may sit under what: on Cloud a standard type under an epic and a sub-task type
        // under a standard issue; on Server a sub-task type under anything. Anything else is
        // created beside its parent and linked to it.
        let (kind, attach) = match (parent, self.settings.deployment) {
            (None, _) => (standard, false),
            (Some(_), Deployment::Server) => (
                child_type.ok_or_else(|| {
                    unsupported(
                        "create a child without a sub-task type",
                        "create issues without a parent",
                    )
                })?,
                true,
            ),
            (Some(parent), Deployment::Cloud) => match self.hierarchy_level(parent)? {
                level if level > 0 => (standard, true),
                0 => match child_type {
                    Some(child) => (child, true),
                    None => (standard, false),
                },
                _ => (standard, false),
            },
        };
        let mut fields = serde_json::json!({
            "project": { "key": project },
            "summary": summary_of(&spec.title),
            "issuetype": { "name": kind },
            "labels": spec.labels.iter().map(|label| jira_label(label)).collect::<Vec<_>>(),
        });
        if !spec.body.trim().is_empty() {
            fields["description"] = self.text(&spec.body);
        }
        let priorities = &discovered.capabilities.priorities;
        if let Some(priority) = spec
            .priority
            .as_ref()
            .filter(|priority| priorities.is_empty() || priorities.contains(priority))
        {
            fields["priority"] = serde_json::json!({ "name": priority });
        }
        if let Some(parent) = parent.filter(|_| attach) {
            fields["parent"] = serde_json::json!({ "key": parent });
        }
        let created = read_json(self.call(
            "POST",
            &self.api("/issue"),
            &[],
            Some(&serde_json::json!({ "fields": fields })),
            "Jira rejected the new issue (check its type, labels and priority)",
        )?)?;
        let key = created
            .get("key")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| failure("Jira answered without an issue key"))?;
        if let Some(parent) = parent.filter(|_| !attach) {
            self.link_issues(key, parent, "the child")?;
        }
        self.issue(key.to_owned())
    }

    fn link(&self, link: TrackerLink) -> Result<(), ProviderError> {
        let from = self.key(&link.from)?;
        match (&link.to, &link.url) {
            (Some(to), _) => {
                self.link_issues(from, self.key(to)?, "the link")?;
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
                    &format!("Jira rejected the remote link on {from}"),
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

/// The comment entity property AI6 sets on every comment it writes. A person's comment cannot carry
/// it by accident, whoever's account it is on: Jira's UI sets no property, and only a comment read
/// with `expand=properties` shows one. Tested on Jira Cloud's REST API v3.
const COMMENT_PROPERTY: &str = "dev.coqui.ai6";

/// Whether AI6 wrote `comment`, read with `expand=properties`.
fn is_ai6_comment(comment: &serde_json::Value) -> bool {
    comment
        .get("properties")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|properties| {
            properties.iter().any(|property| {
                property.get("key").and_then(serde_json::Value::as_str) == Some(COMMENT_PROPERTY)
            })
        })
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

/// A title as a summary: one line of at most [`SUMMARY_BYTES`], cut on a character boundary.
fn summary_of(title: &str) -> String {
    let line = title.lines().next().unwrap_or_default();
    let mut end = line.len().min(SUMMARY_BYTES);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_owned()
}

/// Whether timestamp `a` is later than `b`. Each carries the account's offset, which a change of
/// clock moves, so the text alone does not order them; text that does not parse compares as text.
fn is_after(a: &str, b: &str) -> bool {
    match (instant(a), instant(b)) {
        (Some(a), Some(b)) => a > b,
        _ => a > b,
    }
}

/// `2026-11-01T22:57:12.421+0700` as milliseconds since the Unix epoch.
fn instant(text: &str) -> Option<i64> {
    let at = time::OffsetDateTime::parse(
        text,
        &time::format_description::well_known::Iso8601::DEFAULT,
    )
    .ok()?;
    Some(at.unix_timestamp() * 1_000 + i64::from(at.millisecond()))
}

/// Jira labels cannot hold whitespace.
fn jira_label(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join("-")
}

impl Jira {
    /// Links two issues of this project with the instance's `link_type`.
    fn link_issues(&self, from: &str, to: &str, what: &str) -> Result<(), ProviderError> {
        self.call(
            "POST",
            &self.api("/issueLink"),
            &[],
            Some(&serde_json::json!({
                "type": { "name": self.settings.link_type },
                "inwardIssue": { "key": from },
                "outwardIssue": { "key": to },
            })),
            &format!(
                "Jira rejected {what} from {from} to {to} (check `link_type`: {})",
                self.settings.link_type
            ),
        )?;
        Ok(())
    }

    /// An issue's level: above 0 an epic, 0 a standard issue, below 0 a sub-task.
    fn hierarchy_level(&self, key: &str) -> Result<i64, ProviderError> {
        let issue = self.get_json(
            &self.api(&format!("/issue/{key}")),
            &[("fields", "issuetype".to_owned())],
            &format!("Jira rejected reading {key}"),
        )?;
        let kind = issue.pointer("/fields/issuetype");
        Ok(kind
            .and_then(|kind| kind.get("hierarchyLevel"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_else(|| {
                let subtask = kind.and_then(|kind| kind.get("subtask"));
                if subtask.and_then(serde_json::Value::as_bool) == Some(true) {
                    -1
                } else {
                    0
                }
            }))
    }

    /// The reporter's and assignee's IDs: the people who may answer for an issue.
    fn reporter_and_assignee(&self, key: &str) -> Vec<String> {
        self.get_json(
            &self.api(&format!("/issue/{key}")),
            &[("fields", "reporter,assignee".to_owned())],
            &format!("Jira rejected reading {key}"),
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
            priority: fields
                .pointer("/priority/name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            code: item.code,
            code_tag: item.code_tag,
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
            code: Some(key.to_owned()),
            code_tag: None,
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
    fn timestamps_order_by_the_instant_they_name_and_not_their_text() {
        let east = "2026-11-01T22:57:12.421+0700";
        let utc = "2026-11-01T15:57:12.421+0000";
        assert_eq!(instant(east), instant(utc));
        assert_eq!(instant("1970-01-01T00:00:01.5Z"), Some(1_500));
        assert_eq!(
            instant("2024-02-29T23:59:59.999-0800"),
            Some(1_709_279_999_999),
            "a leap day west of UTC"
        );
        assert_eq!(instant("2100-03-01T00:00:00Z"), Some(4_107_542_400_000));
        assert_eq!(
            instant("2026-02-29T00:00:00Z"),
            None,
            "2026 is not a leap year"
        );
        assert!(
            is_after("2026-11-01T15:57:13.000+0000", east),
            "later, though its text sorts first"
        );
        assert!(!is_after(east, utc));
        assert!(
            is_after("b", "a"),
            "text that does not parse compares as text"
        );
    }

    #[test]
    fn a_summary_is_one_line_cut_on_a_character_boundary() {
        assert_eq!(summary_of("one\ntwo"), "one");
        let long = "ñ".repeat(200);
        let cut = summary_of(&long);
        assert!(cut.len() <= SUMMARY_BYTES && cut.chars().all(|c| c == 'ñ'));
        assert_eq!(cut.len(), 254);
    }

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
