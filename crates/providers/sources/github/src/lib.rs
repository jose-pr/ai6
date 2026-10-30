// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-source-github`: the `github` provider. As a Source it lists the open issues (and
//! pull requests) of one repository that carry the configured labels; as a Tracker it reads,
//! comments on, labels, closes and creates issues of that repository. Requests are blocking;
//! the host calls them off its async runtime.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

ai6::guatiao::providers!(Github);

use std::collections::BTreeMap;
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
pub const PROVIDER_ID: &str = "github";

/// One instance's settings, from `[operation.<id>.sources.<name>]`.
///
/// The Station reads `provider`, `intake` and `interval_secs` itself and does not pass them
/// on. Keys the Station applies (status labels, agent labels, the type and priority
/// settings Jira uses) are accepted here so one table serves every tracker; the provider
/// ignores them.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    /// `owner/name`; a bare string under `sources` is this value.
    #[serde(alias = "target")]
    repo: String,
    /// An issue needs every one of these labels to become a Mission.
    #[serde(default = "default_labels")]
    labels: Vec<String>,
    /// Only issues assigned to this login, when set.
    assignee: Option<String>,
    /// The environment variable holding the token; the token itself is never configured.
    #[serde(default = "default_token_env")]
    token_env: String,
    /// The REST API root; `https`, or `http` on a loopback address.
    #[serde(default = "default_api_url")]
    api_url: String,
    /// Seconds one request may take. Default 30; 0 is refused.
    #[serde(default = "default_request_timeout_secs")]
    request_timeout_secs: u64,
    /// Issues asked for per page. Default 100; 0 is refused.
    #[serde(default = "default_page_size")]
    page_size: u32,
    /// Pages one poll reads at most. Default 5; 0 is refused.
    #[serde(default = "default_max_pages")]
    max_pages: usize,
    /// GitHub logins besides the issue's author and the associations below whose comments
    /// may answer a question.
    #[serde(default)]
    reply_authors: Vec<String>,
    /// `author_association` values whose comments may answer a question.
    #[serde(default = "default_reply_associations")]
    reply_associations: Vec<String>,
    /// How long, in seconds, the repository's labels and settings are remembered. Default
    /// 600.
    #[serde(default = "default_capability_ttl_secs")]
    capability_ttl_secs: u64,
    #[serde(default, rename = "status_labels")]
    _status_labels: IgnoredAny,
    #[serde(default, rename = "agent_labels")]
    _agent_labels: IgnoredAny,
    #[serde(default, rename = "issue_type")]
    _issue_type: IgnoredAny,
    #[serde(default, rename = "subtask_type")]
    _subtask_type: IgnoredAny,
    #[serde(default, rename = "priority_map")]
    _priority_map: IgnoredAny,
    #[serde(default, rename = "status_transitions")]
    _status_transitions: IgnoredAny,
}

fn default_request_timeout_secs() -> u64 {
    30
}

fn default_page_size() -> u32 {
    100
}

fn default_max_pages() -> usize {
    5
}

fn default_capability_ttl_secs() -> u64 {
    600
}

fn default_labels() -> Vec<String> {
    vec!["ai6:ready".to_owned()]
}

fn default_reply_associations() -> Vec<String> {
    ["OWNER", "MEMBER", "COLLABORATOR"]
        .map(str::to_owned)
        .to_vec()
}

fn default_token_env() -> String {
    "GITHUB_TOKEN".to_owned()
}

fn default_api_url() -> String {
    "https://api.github.com".to_owned()
}

/// GitHub issues as an AI6 Source and Tracker.
#[derive(Debug, Provider)]
#[provider(
    Source,
    Tracker,
    id = "github",
    name = "GitHub issues",
    config = SourceConfig,
    crate = ::ai6::guatiao
)]
pub struct Github {
    settings: Settings,
    api: Url,
    agent: ureq::Agent,
    /// The repository-level capabilities and when they were read.
    capabilities: Mutex<Option<(Instant, TrackerCapabilities)>>,
    /// The login the token belongs to; `Some(None)` once GitHub declined to say.
    identity: Mutex<Option<Option<String>>>,
}

impl Github {
    /// Builds the provider from the instance's settings table, serialized as TOML.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown key, a malformed repository name or an insecure
    /// API address.
    pub fn from_settings(settings_toml: &str) -> Result<Self, String> {
        let settings: Settings = toml::from_str(settings_toml).map_err(|e| e.to_string())?;
        let valid_part = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        match settings.repo.split_once('/') {
            Some((owner, name)) if valid_part(owner) && valid_part(name) => {}
            _ => return Err("repo must be `owner/name`".to_owned()),
        }
        if settings.token_env.trim().is_empty() {
            return Err("token_env must name an environment variable".to_owned());
        }
        let api = Url::parse(settings.api_url.trim_end_matches('/'))
            .map_err(|e| format!("api_url: {e}"))?;
        let loopback = matches!(api.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if api.scheme() != "https" && !(api.scheme() == "http" && loopback) {
            return Err("api_url must be https (http only on a loopback address)".to_owned());
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
            api,
            agent,
            capabilities: Mutex::new(None),
            identity: Mutex::new(None),
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.api.as_str().trim_end_matches('/'))
    }

    fn token(&self) -> Option<String> {
        std::env::var(&self.settings.token_env)
            .ok()
            .filter(|token| !token.trim().is_empty())
    }

    /// One request. Reads go without a token when none is set (public repositories); any
    /// other method needs one.
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
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "ai6-source-github");
        match self.token() {
            Some(token) => request = request.header("Authorization", format!("Bearer {token}")),
            None if method != "GET" => {
                return Err(failure(&format!(
                    "no token: set the environment variable {}",
                    self.settings.token_env
                )));
            }
            None => {}
        }
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
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
        };
        match status {
            200..=299 => Ok(()),
            429 => Err(failure(&format!(
                "GitHub rate limit reached; retry after {}s",
                header("retry-after").unwrap_or("60")
            ))),
            403 if header("x-ratelimit-remaining") == Some("0") => Err(failure(&format!(
                "GitHub rate limit exhausted; the quota resets at epoch second {}",
                header("x-ratelimit-reset").unwrap_or("unknown")
            ))),
            401 | 403 => Err(failure(&format!(
                "GitHub refused the token in {}",
                self.settings.token_env
            ))),
            404 => Err(failure(
                "GitHub does not show this repository or issue to the token",
            )),
            410 => Err(failure("GitHub has issues disabled for this repository")),
            422 => Err(failure("GitHub rejected the request as invalid")),
            other => Err(failure(&format!("GitHub answered status {other}"))),
        }
    }

    fn issues_url(&self) -> String {
        self.endpoint(&format!("/repos/{}/issues", self.settings.repo))
    }

    /// The issue number of `item`, which must name this instance's repository.
    fn number(&self, item: &str) -> Result<u64, ProviderError> {
        let (repo, number) = item
            .rsplit_once('#')
            .ok_or_else(|| bad_value(&format!("{item:?} is not an issue ID like owner/name#1")))?;
        let number = number
            .parse::<u64>()
            .map_err(|_| bad_value(&format!("{item:?} has no issue number")))?;
        if !repo.eq_ignore_ascii_case(&self.settings.repo) {
            return Err(bad_value(&format!(
                "foreign item {item:?}: this instance serves {}",
                self.settings.repo
            )));
        }
        Ok(number)
    }

    fn issue_url(&self, number: u64) -> String {
        format!("{}/{number}", self.issues_url())
    }

    /// The login the token belongs to, asked once; `None` when GitHub will not say.
    fn own_login(&self) -> Option<String> {
        let mut identity = self.identity.lock().ok()?;
        if identity.is_none() {
            let login = self
                .get_json(&self.endpoint("/user"), &[])
                .ok()
                .and_then(|user| user.get("login")?.as_str().map(str::to_owned));
            *identity = Some(login);
        }
        identity.clone().flatten()
    }

    fn repo_capabilities(&self) -> Result<TrackerCapabilities, ProviderError> {
        let ttl = Duration::from_secs(self.settings.capability_ttl_secs);
        if let Ok(cache) = self.capabilities.lock()
            && let Some((at, capabilities)) = cache.as_ref()
            && at.elapsed() < ttl
        {
            return Ok(capabilities.clone());
        }
        let repo = self.get_json(
            &self.endpoint(&format!("/repos/{}", self.settings.repo)),
            &[],
        )?;
        let visibility = match repo.get("private").and_then(serde_json::Value::as_bool) {
            Some(true) => "private",
            Some(false) => "public",
            None => "unknown",
        };
        let mut labels = Vec::new();
        let mut url = self.endpoint(&format!("/repos/{}/labels", self.settings.repo));
        let mut first = true;
        for _ in 0..self.settings.max_pages {
            let mut response = self.get(
                &url,
                &if first {
                    vec![("per_page", self.settings.page_size.to_string())]
                } else {
                    Vec::new()
                },
            )?;
            first = false;
            let next = next_link(response.headers().get("link").and_then(|v| v.to_str().ok()))
                .filter(|next| same_origin(next, &self.api));
            let page: Vec<serde_json::Value> = response
                .body_mut()
                .read_json()
                .map_err(|e| failure(&format!("unreadable response: {e}")))?;
            labels.extend(
                page.iter()
                    .filter_map(|label| label.get("name")?.as_str().map(str::to_owned)),
            );
            match next {
                Some(next) => url = next,
                None => break,
            }
        }
        let capabilities = TrackerCapabilities {
            comment: true,
            edit_comment: true,
            labels,
            free_labels: false,
            transitions: Vec::new(),
            assign: true,
            close: true,
            create: repo
                .get("has_issues")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            issue_types: Vec::new(),
            priorities: Vec::new(),
            children: "sub_issues".to_owned(),
            links: Vec::new(),
            replies: true,
            visibility: visibility.to_owned(),
            markers: true,
        };
        if let Ok(mut cache) = self.capabilities.lock() {
            *cache = Some((Instant::now(), capabilities.clone()));
        }
        Ok(capabilities)
    }

    /// The issue behind `number`, for the author rule of repliers.
    fn author_of(&self, number: u64) -> Option<String> {
        self.get_json(&self.issue_url(number), &[])
            .ok()?
            .pointer("/user/login")?
            .as_str()
            .map(str::to_owned)
    }

    fn unsupported(what: &str, can: &str) -> ProviderError {
        ProviderError::new(
            Status::GUATIAO_ERR_UNSUPPORTED,
            &format!("GitHub cannot {what}; it can {can}"),
        )
    }
}

impl TryFrom<SourceConfig> for Github {
    type Error = ProviderError;

    fn try_from(config: SourceConfig) -> Result<Self, Self::Error> {
        Self::from_settings(&config.settings_toml)
            .map_err(|e| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &e))
    }
}

impl Source for Github {
    fn poll(&self, request: SourcePoll) -> Result<SourceBatch, ProviderError> {
        let mut query = vec![
            ("state", "open".to_owned()),
            ("sort", "updated".to_owned()),
            ("direction", "asc".to_owned()),
            ("per_page", self.settings.page_size.to_string()),
        ];
        if !self.settings.labels.is_empty() {
            query.push(("labels", self.settings.labels.join(",")));
        }
        if let Some(assignee) = &self.settings.assignee {
            query.push(("assignee", assignee.clone()));
        }
        if let Some(cursor) = &request.cursor {
            query.push(("since", cursor.clone()));
        }

        let mut items = Vec::new();
        let mut url = self.issues_url();
        let mut first = true;
        for _ in 0..self.settings.max_pages {
            let mut response = self.get(&url, if first { &query } else { &[] })?;
            first = false;
            let next = next_link(response.headers().get("link").and_then(|v| v.to_str().ok()))
                .filter(|next| same_origin(next, &self.api));
            let exhausted = response
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|value| value.to_str().ok())
                == Some("0");
            let page: Vec<serde_json::Value> = response
                .body_mut()
                .read_json()
                .map_err(|e| failure(&format!("unreadable response: {e}")))?;
            items.extend(
                page.iter()
                    .filter_map(|issue| item_of(&self.settings.repo, issue)),
            );
            match next {
                Some(next) if !exhausted => url = next,
                _ => break,
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

impl Tracker for Github {
    fn capabilities(&self, item: Option<String>) -> Result<TrackerCapabilities, ProviderError> {
        let mut capabilities = self.repo_capabilities()?;
        if let Some(item) = item {
            let number = self.number(&item)?;
            let probe = self.call(
                "GET",
                &format!("{}/sub_issues", self.issue_url(number)),
                &[("per_page", "1".to_owned())],
                None,
            );
            if probe.is_err() {
                capabilities.children = "none".to_owned();
            }
        }
        Ok(capabilities)
    }

    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError> {
        let number = self.number(&item)?;
        let issue = self.get_json(&self.issue_url(number), &[])?;
        issue_of(&self.settings.repo, &issue)
            .ok_or_else(|| failure("GitHub answered with an issue this provider cannot read"))
    }

    fn replies(&self, request: ReplyPoll) -> Result<ReplyBatch, ProviderError> {
        let wanted: Vec<u64> = request
            .items
            .iter()
            .map(|item| self.number(item))
            .collect::<Result<_, _>>()?;
        if wanted.is_empty() {
            return Ok(ReplyBatch {
                replies: Vec::new(),
                cursor: request.since,
            });
        }
        let own = self.own_login();
        let mut query = vec![
            ("sort", "updated".to_owned()),
            ("direction", "asc".to_owned()),
            ("per_page", self.settings.page_size.to_string()),
        ];
        if let Some(since) = &request.since {
            query.push(("since", since.clone()));
        }
        let mut url = self.endpoint(&format!("/repos/{}/issues/comments", self.settings.repo));
        let mut first = true;
        let mut replies = Vec::new();
        let mut cursor = request.since.clone();
        let mut issue_authors: BTreeMap<u64, Option<String>> = BTreeMap::new();
        for _ in 0..self.settings.max_pages {
            let mut response = self.get(&url, if first { &query } else { &[] })?;
            first = false;
            let next = next_link(response.headers().get("link").and_then(|v| v.to_str().ok()))
                .filter(|next| same_origin(next, &self.api));
            let page: Vec<serde_json::Value> = response
                .body_mut()
                .read_json()
                .map_err(|e| failure(&format!("unreadable response: {e}")))?;
            for comment in &page {
                let text = |key: &str| comment.get(key).and_then(serde_json::Value::as_str);
                if let Some(updated) = text("updated_at")
                    && cursor.as_deref().is_none_or(|current| updated > current)
                {
                    cursor = Some(updated.to_owned());
                }
                let Some(number) = text("issue_url").and_then(trailing_number) else {
                    continue;
                };
                if !wanted.contains(&number) {
                    continue;
                }
                let login = comment
                    .pointer("/user/login")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let association = text("author_association").unwrap_or_default();
                let permitted = self
                    .settings
                    .reply_associations
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(association))
                    || self
                        .settings
                        .reply_authors
                        .iter()
                        .any(|allowed| allowed.eq_ignore_ascii_case(login))
                    || issue_authors
                        .entry(number)
                        .or_insert_with(|| self.author_of(number))
                        .as_deref()
                        .is_some_and(|author| author.eq_ignore_ascii_case(login));
                replies.push(Reply {
                    id: comment
                        .get("id")
                        .and_then(serde_json::Value::as_u64)
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    item: format!("{}#{number}", self.settings.repo),
                    author_id: login.to_owned(),
                    author: login.to_owned(),
                    permitted,
                    own: own
                        .as_deref()
                        .is_some_and(|own| own.eq_ignore_ascii_case(login)),
                    body: text("body").unwrap_or_default().to_owned(),
                    created: text("created_at").unwrap_or_default().to_owned(),
                });
            }
            match next {
                Some(next) => url = next,
                None => break,
            }
        }
        Ok(ReplyBatch { replies, cursor })
    }

    fn post(&self, post: TrackerPost) -> Result<PostedComment, ProviderError> {
        let number = self.number(&post.item)?;
        let mut body = post.body;
        if let Some(marker) = &post.marker {
            let marker: String = marker
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
                .collect();
            body.push_str(&format!("\n\n<!-- ai6:{marker} -->"));
        }
        let payload = serde_json::json!({ "body": body });
        let comment = match &post.edit {
            Some(id) => {
                if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad_value("a comment ID is a number"));
                }
                read_json(self.call(
                    "PATCH",
                    &self.endpoint(&format!(
                        "/repos/{}/issues/comments/{id}",
                        self.settings.repo
                    )),
                    &[],
                    Some(&payload),
                )?)?
            }
            None => read_json(self.call(
                "POST",
                &format!("{}/comments", self.issue_url(number)),
                &[],
                Some(&payload),
            )?)?,
        };
        Ok(PostedComment {
            id: comment
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .map(|id| id.to_string())
                .ok_or_else(|| failure("GitHub answered without a comment ID"))?,
            url: comment
                .get("html_url")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        })
    }

    fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError> {
        let number = self.number(&update.item)?;
        if update.transition.is_some() {
            return Err(Self::unsupported(
                "make a workflow transition",
                "set the state to open or closed",
            ));
        }
        let base = self.issue_url(number);
        if !update.add_labels.is_empty() {
            self.call(
                "POST",
                &format!("{base}/labels"),
                &[],
                Some(&serde_json::json!({ "labels": update.add_labels })),
            )?;
        }
        for label in &update.remove_labels {
            match self.call(
                "DELETE",
                &format!("{base}/labels/{}", encode_segment(label)),
                &[],
                None,
            ) {
                Ok(_) => {}
                // A label that is already gone is the state asked for.
                Err(error) if error.to_string().contains("does not show") => {}
                Err(error) => return Err(error),
            }
        }
        if !update.add_assignees.is_empty() {
            self.call(
                "POST",
                &format!("{base}/assignees"),
                &[],
                Some(&serde_json::json!({ "assignees": update.add_assignees })),
            )?;
        }
        if !update.remove_assignees.is_empty() {
            self.call(
                "DELETE",
                &format!("{base}/assignees"),
                &[],
                Some(&serde_json::json!({ "assignees": update.remove_assignees })),
            )?;
        }
        match update.state.as_deref() {
            None => {}
            Some("closed") => {
                self.call(
                    "PATCH",
                    &base,
                    &[],
                    Some(&serde_json::json!({ "state": "closed", "state_reason": "completed" })),
                )?;
            }
            Some("open") => {
                self.call(
                    "PATCH",
                    &base,
                    &[],
                    Some(&serde_json::json!({ "state": "open" })),
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
        let capabilities = self.repo_capabilities()?;
        if !capabilities.create {
            return Err(Self::unsupported(
                "create issues here",
                "read and comment on them",
            ));
        }
        let parent = spec
            .parent
            .as_deref()
            .map(|parent| self.number(parent))
            .transpose()?;
        // A label the repository lacks would be created by GitHub: keep the ones it has.
        let labels: Vec<&String> = spec
            .labels
            .iter()
            .filter(|label| capabilities.labels.contains(label))
            .collect();
        let created = read_json(self.call(
            "POST",
            &self.issues_url(),
            &[],
            Some(&serde_json::json!({
                "title": spec.title,
                "body": spec.body,
                "labels": labels,
            })),
        )?)?;
        let mut issue = issue_of(&self.settings.repo, &created)
            .ok_or_else(|| failure("GitHub answered with an issue this provider cannot read"))?;
        if let (Some(parent), Some(id)) = (parent, created.get("id").and_then(|id| id.as_u64())) {
            let attached = self.call(
                "POST",
                &format!("{}/sub_issues", self.issue_url(parent)),
                &[],
                Some(&serde_json::json!({ "sub_issue_id": id })),
            );
            // Sub-issues are not on every plan; the issue exists either way and the caller
            // links it by reference when it has no parent.
            if attached.is_ok() {
                issue.parent = Some(format!("{}#{parent}", self.settings.repo));
            }
        }
        Ok(issue)
    }

    fn link(&self, link: TrackerLink) -> Result<(), ProviderError> {
        self.number(&link.from)?;
        Err(Self::unsupported(
            "link issues",
            "name an issue in a comment, which GitHub turns into a link",
        ))
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

/// The number at the end of an API URL such as `.../issues/12`.
fn trailing_number(url: &str) -> Option<u64> {
    url.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

/// Percent-encodes one path segment.
fn encode_segment(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn issue_of(repo: &str, issue: &serde_json::Value) -> Option<TrackerIssue> {
    let item = item_of(repo, issue)?;
    let login = |value: &serde_json::Value| value.get("login")?.as_str().map(str::to_owned);
    Some(TrackerIssue {
        id: item.id,
        native_id: issue
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .map(|id| id.to_string())
            .unwrap_or_default(),
        title: item.title,
        body: item.body,
        state: issue
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("open")
            .to_owned(),
        labels: item.labels,
        assignees: issue
            .get("assignees")
            .and_then(serde_json::Value::as_array)
            .map(|people| people.iter().filter_map(login).collect())
            .unwrap_or_default(),
        url: item.url,
        updated: item.updated,
        parent: issue
            .get("parent_issue_url")
            .and_then(serde_json::Value::as_str)
            .and_then(trailing_number)
            .map(|number| format!("{repo}#{number}")),
        author_id: item.author,
    })
}

fn bad_value(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

fn item_of(repo: &str, issue: &serde_json::Value) -> Option<SourceItem> {
    let text = |key: &str| issue.get(key).and_then(serde_json::Value::as_str);
    let number = issue.get("number")?.as_u64()?;
    Some(SourceItem {
        id: format!("{repo}#{number}"),
        title: text("title")?.to_owned(),
        body: text("body").unwrap_or_default().to_owned(),
        url: text("html_url").map(str::to_owned),
        labels: issue
            .get("labels")
            .and_then(serde_json::Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| label.get("name")?.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        author: issue
            .get("user")
            .and_then(|user| user.get("login"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        updated: text("updated_at")?.to_owned(),
    })
}

/// The `rel="next"` target of an RFC 8288 `Link` header.
fn next_link(header: Option<&str>) -> Option<String> {
    header?.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        params
            .split(';')
            .any(|param| param.trim().replace(' ', "") == "rel=\"next\"")
            .then(|| {
                target
                    .trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_owned()
            })
    })
}

/// Pagination links must stay on the API origin, so the token is never sent elsewhere.
fn same_origin(candidate: &str, api: &Url) -> bool {
    Url::parse(candidate).is_ok_and(|url| url.origin() == api.origin())
}

fn failure(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_INTERNAL, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_headers_yield_the_next_page() {
        let header = r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=9>; rel="last""#;
        assert_eq!(
            next_link(Some(header)).as_deref(),
            Some("https://api.github.com/x?page=2")
        );
        assert_eq!(next_link(Some(r#"<https://a/x>; rel="last""#)), None);
        assert_eq!(next_link(None), None);
    }

    #[test]
    fn settings_reject_unknown_keys_bad_repos_and_insecure_apis() {
        assert!(Github::from_settings("repo = \"o/r\"").is_ok());
        assert!(Github::from_settings("target = \"o/r\"").is_ok());
        assert!(Github::from_settings("repo = \"o/r\"\nlabel = \"x\"").is_err());
        assert!(Github::from_settings("repo = \"nope\"").is_err());
        assert!(Github::from_settings("repo = \"o/../r\"").is_err());
        assert!(Github::from_settings("repo = \"o/r\"\napi_url = \"http://example.com\"").is_err());
        assert!(Github::from_settings("repo = \"o/r\"\napi_url = \"http://127.0.0.1:9\"").is_ok());
    }

    #[test]
    fn pagination_never_leaves_the_api_origin() {
        let api = Url::parse("https://api.github.com").unwrap();
        assert!(same_origin("https://api.github.com/repos?page=2", &api));
        assert!(!same_origin("https://evil.example/repos?page=2", &api));
    }
}
