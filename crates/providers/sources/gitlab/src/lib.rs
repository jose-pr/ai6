// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-source-gitlab`: the `gitlab` provider. As a Source it lists the open issues of one
//! project that carry the configured labels; as a Tracker it reads, comments on, labels,
//! closes, links and creates issues of that project. Requests are blocking; the host calls
//! them off its async runtime.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

ai6::guatiao::providers!(Gitlab);

use std::collections::BTreeMap;
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
pub const PROVIDER_ID: &str = "gitlab";

/// What marks a comment the provider posted, whoever the token turns out to be.
const MARKER_PREFIX: &str = "<!-- ai6:";

/// One instance's settings, from `[operation.<id>.sources.<name>]`.
///
/// The Station reads its own keys (`provider`, `intake`, `interval_secs`, the status maps and
/// what Agents may touch) and passes the rest here.
#[derive(Debug, Clone, ToValue, FromValue, Schema)]
#[map(crate = ::ai6::guatiao, deny_unknown_fields)]
struct Settings {
    /// `group/name`, or `group/subgroup/name`; a bare string under `sources` is this value.
    #[schema(title = "Project")]
    project: String,
    /// An issue needs every one of these labels to become a Mission.
    #[schema(title = "Intake labels")]
    #[map(default = vec!["ai6:ready".to_owned()])]
    labels: Vec<String>,
    /// Only issues assigned to this username, when set.
    #[schema(title = "Assignee")]
    assignee: Option<String>,
    /// The environment variable holding the token; the token itself is never configured.
    #[schema(title = "Token variable")]
    #[map(default = "GITLAB_TOKEN".to_owned())]
    token_env: String,
    /// The REST API root; `https`, or `http` on a loopback address.
    #[schema(title = "API address")]
    #[map(default = "https://gitlab.com/api/v4".to_owned())]
    api_url: String,
    /// A PEM file of certificate authorities to trust besides the public roots, by absolute path.
    /// A set one is reported as a weakening: more authorities can vouch for this service.
    #[schema(title = "Certificate authorities")]
    ca_file: Option<PathBuf>,
    /// Seconds one request may take. 0 is refused.
    #[schema(title = "A request may take", format = "seconds")]
    #[map(default = 30)]
    request_timeout_secs: u64,
    /// Issues, comments or labels asked for per page. 0 is refused.
    #[schema(title = "Items per page")]
    #[map(default = 100)]
    page_size: u32,
    /// Pages one listing reads at most. 0 is refused.
    #[schema(title = "Pages per listing")]
    #[map(default = 5)]
    max_pages: u64,
    /// Seconds a poll searches behind the cursor, for an issue the listing showed after a newer
    /// one, or labelled within a minute of its last change, which GitLab leaves out of its
    /// `updated_at`. 0 searches from the cursor.
    #[schema(title = "Search behind the cursor by", format = "seconds")]
    #[map(default = 120)]
    settling_secs: u64,
    /// GitLab usernames besides the issue's author and the project members below whose comments
    /// may answer a question.
    #[schema(title = "People who may answer")]
    #[map(default)]
    reply_authors: Vec<String>,
    /// The least project access level whose comments may answer a question: 10 Guest,
    /// 15 Planner, 20 Reporter, 30 Developer, 40 Maintainer, 50 Owner.
    #[schema(title = "Least access that may answer")]
    #[map(default = 30)]
    reply_min_access: u64,
    /// How long, in seconds, the project's labels and settings are remembered.
    #[schema(title = "Remember the project for", format = "seconds")]
    #[map(default = 600)]
    capability_ttl_secs: u64,
}

/// GitLab issues as an AI6 Source and Tracker.
#[derive(Debug, Provider)]
#[provider(
    Source,
    Tracker,
    id = "gitlab",
    name = "GitLab issues",
    config = SourceConfig,
    crate = ::ai6::guatiao
)]
pub struct Gitlab {
    settings: Settings,
    api: Url,
    agent: ureq::Agent,
    /// The project-level capabilities and when they were read.
    capabilities: Mutex<Option<(Instant, TrackerCapabilities)>>,
}

impl Gitlab {
    /// Builds the provider from the instance's settings table, serialized as TOML.
    ///
    /// # Errors
    ///
    /// Returns a message for an unknown key, a malformed project path, a limit of 0 or an
    /// insecure API address.
    pub fn from_settings(settings_toml: &str) -> Result<Self, String> {
        let mut table: toml::Table = toml::from_str(settings_toml).map_err(|e| e.to_string())?;
        if let Some(target) = table.remove("target") {
            table.insert("project".to_owned(), target);
        }
        let settings: Settings = settings_from_table(table)?;
        let valid_part = |part: &str| {
            !part.is_empty()
                && !part.starts_with('.')
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        let parts: Vec<&str> = settings.project.split('/').collect();
        if parts.len() < 2 || !parts.iter().all(|part| valid_part(part)) {
            return Err("project must be `group/name` or `group/subgroup/name`".to_owned());
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
            api,
            agent,
            capabilities: Mutex::new(None),
        })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.api.as_str().trim_end_matches('/'))
    }

    /// GraphQL beside the REST root: `.../api/v4` answers GraphQL at `.../api/graphql`.
    fn graphql_url(&self) -> String {
        let mut api = self.api.clone();
        if !api.path().ends_with('/') {
            api.set_path(&format!("{}/", api.path()));
        }
        api.join("../graphql")
            .map_or_else(|_| self.endpoint("/graphql"), |url| url.to_string())
    }

    /// The web address of the project, for links GitLab's answers do not carry: the API root
    /// less its `/api/v4`.
    fn web_project(&self) -> Option<String> {
        let api = self.api.as_str().trim_end_matches('/');
        api.strip_suffix("/api/v4")
            .map(|web| format!("{web}/{}", self.settings.project))
    }

    fn token(&self) -> Option<String> {
        std::env::var(&self.settings.token_env)
            .ok()
            .filter(|token| !token.trim().is_empty())
    }

    /// One request, answered with whatever status GitLab gave. Reads go without a token when
    /// none is set (public projects); any other method needs one.
    fn send(
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
            .header("User-Agent", "ai6-source-gitlab");
        match self.token() {
            Some(token) => request = request.header("PRIVATE-TOKEN", token),
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
        self.agent
            .run(request)
            .map_err(|e| failure(&format!("request failed: {e}")))
    }

    /// One request whose status must be a success.
    fn call(
        &self,
        method: &str,
        url: &str,
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<ureq::http::Response<ureq::Body>, ProviderError> {
        let response = self.send(method, url, query, body)?;
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
                "GitLab rate limit reached; retry after {}s",
                header("retry-after").unwrap_or("60")
            ))),
            401 => Err(failure(&format!(
                "GitLab refused the token in {}",
                self.settings.token_env
            ))),
            403 => Err(failure(&format!(
                "GitLab does not let the token in {} do this",
                self.settings.token_env
            ))),
            // A private project answers 404, not 401, to a request with no token.
            404 if self.token().is_none() => Err(failure(&format!(
                "GitLab does not show this project or issue without a token: \
                 set the environment variable {}",
                self.settings.token_env
            ))),
            404 => Err(failure(
                "GitLab does not show this project or issue to the token",
            )),
            400 | 422 => Err(failure("GitLab rejected the request as invalid")),
            other => Err(failure(&format!("GitLab answered status {other}"))),
        }
    }

    fn project_url(&self) -> String {
        self.endpoint(&format!(
            "/projects/{}",
            encode_segment(&self.settings.project)
        ))
    }

    fn issues_url(&self) -> String {
        format!("{}/issues", self.project_url())
    }

    /// The issue's project-local number (`iid`) of `item`, which must name this instance's
    /// project.
    fn iid(&self, item: &str) -> Result<u64, ProviderError> {
        let (project, iid) = item
            .rsplit_once('#')
            .ok_or_else(|| bad_value(&format!("{item:?} is not an issue ID like group/name#1")))?;
        let iid = iid
            .parse::<u64>()
            .map_err(|_| bad_value(&format!("{item:?} has no issue number")))?;
        if !project.eq_ignore_ascii_case(&self.settings.project) {
            return Err(bad_value(&format!(
                "foreign item {item:?}: this instance serves {}",
                self.settings.project
            )));
        }
        Ok(iid)
    }

    fn issue_url(&self, iid: u64) -> String {
        format!("{}/{iid}", self.issues_url())
    }

    fn item_id(&self, iid: u64) -> String {
        format!("{}#{iid}", self.settings.project)
    }

    /// Reads every page of a listing from `url`, up to `max_pages`, following `Link:
    /// rel="next"` on the API origin. Also says whether more pages remained.
    fn pages(
        &self,
        url: &str,
        query: &[(&str, String)],
        mut keep_going: impl FnMut(&[serde_json::Value]) -> bool,
    ) -> Result<(Vec<serde_json::Value>, bool), ProviderError> {
        let mut url = url.to_owned();
        let mut first = true;
        let mut items = Vec::new();
        let mut more = false;
        for _ in 0..self.settings.max_pages {
            let mut response = self.get(&url, if first { query } else { &[] })?;
            first = false;
            let next = next_link(response.headers().get("link").and_then(|v| v.to_str().ok()))
                .filter(|next| same_origin(next, &self.api));
            let exhausted = response
                .headers()
                .get("ratelimit-remaining")
                .and_then(|value| value.to_str().ok())
                == Some("0");
            let page: Vec<serde_json::Value> = response
                .body_mut()
                .read_json()
                .map_err(|e| failure(&format!("unreadable response: {e}")))?;
            let go_on = keep_going(&page);
            items.extend(page);
            more = next.is_some();
            match next {
                Some(next) if go_on && !exhausted => url = next,
                _ => break,
            }
        }
        Ok((items, more))
    }

    fn project_capabilities(&self) -> Result<TrackerCapabilities, ProviderError> {
        let ttl = Duration::from_secs(self.settings.capability_ttl_secs);
        if let Ok(cache) = self.capabilities.lock()
            && let Some((at, capabilities)) = cache.as_ref()
            && at.elapsed() < ttl
        {
            return Ok(capabilities.clone());
        }
        let project = self.get_json(&self.project_url(), &[])?;
        // `internal` is every signed-in user of the instance: neither of the two answers.
        let visibility = match project
            .get("visibility")
            .and_then(serde_json::Value::as_str)
        {
            Some("private") => "private",
            Some("public") => "public",
            _ => "unknown",
        };
        let (labels, _) = self.pages(
            &format!("{}/labels", self.project_url()),
            &[("per_page", self.settings.page_size.to_string())],
            |_| true,
        )?;
        let labels = labels
            .iter()
            .filter_map(|label| label.get("name")?.as_str().map(str::to_owned))
            .collect();
        let issues_enabled = project
            .get("issues_enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true)
            && project
                .get("issues_access_level")
                .and_then(serde_json::Value::as_str)
                != Some("disabled");
        let capabilities = TrackerCapabilities {
            comment: true,
            edit_comment: true,
            labels,
            free_labels: false,
            transitions: Vec::new(),
            assign: true,
            close: true,
            create: issues_enabled,
            issue_types: Vec::new(),
            priorities: Vec::new(),
            children: "subtasks".to_owned(),
            links: vec!["issue_links".to_owned()],
            replies: true,
            visibility: visibility.to_owned(),
            markers: true,
        };
        if let Ok(mut cache) = self.capabilities.lock() {
            *cache = Some((Instant::now(), capabilities.clone()));
        }
        Ok(capabilities)
    }

    /// One GraphQL request; GitLab answers errors inside a 200, so those fail too.
    fn graphql(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let answer = read_json(self.call(
            "POST",
            &self.graphql_url(),
            &[],
            Some(&serde_json::json!({ "query": query, "variables": variables })),
        )?)?;
        if answer
            .get("errors")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|errors| !errors.is_empty())
        {
            return Err(failure("GitLab refused the GraphQL request"));
        }
        Ok(answer)
    }

    /// The parent of a task, which only GraphQL tells: `group/name#iid`.
    fn parent_of(&self, iid: u64) -> Option<String> {
        let answer = self
            .graphql(
                "query($path: ID!, $iid: String!) { project(fullPath: $path) { \
                 workItems(iid: $iid) { nodes { widgets { \
                 ... on WorkItemWidgetHierarchy { parent { iid namespace { fullPath } } } } } } } }",
                serde_json::json!({ "path": self.settings.project, "iid": iid.to_string() }),
            )
            .ok()?;
        let widgets = answer
            .pointer("/data/project/workItems/nodes/0/widgets")?
            .as_array()?;
        widgets.iter().find_map(|widget| {
            let parent = widget.get("parent")?;
            let iid = parent.get("iid")?.as_str()?;
            let path = parent
                .pointer("/namespace/fullPath")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&self.settings.project);
            Some(format!("{path}#{iid}"))
        })
    }

    /// Makes the task `child` (a global issue ID) a child of `parent` (another).
    fn attach(&self, child: u64, parent: u64) -> Result<(), ProviderError> {
        let answer = self.graphql(
            "mutation($id: WorkItemID!, $parent: WorkItemID!) { workItemUpdate(input: \
             {id: $id, hierarchyWidget: {parentId: $parent}}) { errors } }",
            serde_json::json!({
                "id": format!("gid://gitlab/WorkItem/{child}"),
                "parent": format!("gid://gitlab/WorkItem/{parent}"),
            }),
        )?;
        let refused = answer
            .pointer("/data/workItemUpdate/errors")
            .and_then(serde_json::Value::as_array)
            .is_none_or(|errors| !errors.is_empty());
        if refused {
            return Err(failure("GitLab refused to attach the child"));
        }
        Ok(())
    }

    /// The user ID of a project member by username, for assigning.
    fn member_id(&self, username: &str) -> Result<u64, ProviderError> {
        let members = self.get_json(
            &format!("{}/members/all", self.project_url()),
            &[("query", username.to_owned())],
        )?;
        members
            .as_array()
            .into_iter()
            .flatten()
            .find(|member| {
                member
                    .get("username")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|name| name.eq_ignore_ascii_case(username))
            })
            .and_then(|member| member.get("id")?.as_u64())
            .ok_or_else(|| bad_value(&format!("GitLab has no member {username} in this project")))
    }

    /// The access level of a user in the project, 0 when not a member.
    fn access_of(&self, user_id: u64) -> u64 {
        let response = self.send(
            "GET",
            &format!("{}/members/all/{user_id}", self.project_url()),
            &[],
            None,
        );
        match response {
            Ok(response) if response.status().is_success() => read_json(response)
                .ok()
                .and_then(|member| member.get("access_level")?.as_u64())
                .unwrap_or(0),
            _ => 0,
        }
    }

    fn unsupported(what: &str, can: &str) -> ProviderError {
        ProviderError::new(
            Status::GUATIAO_ERR_UNSUPPORTED,
            &format!("GitLab cannot {what}; it can {can}"),
        )
    }
}

impl TryFrom<SourceConfig> for Gitlab {
    type Error = ProviderError;

    fn try_from(config: SourceConfig) -> Result<Self, Self::Error> {
        Self::from_settings(&config.settings_toml)
            .map_err(|e| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &e))
    }
}

impl Gitlab {
    /// The issues changed since `since`, oldest change first, and whether more remain past the
    /// pages one poll reads.
    fn changed_issues(
        &self,
        since: Option<&str>,
    ) -> Result<(Vec<SourceItem>, bool), ProviderError> {
        let mut query = vec![
            ("state", "opened".to_owned()),
            ("order_by", "updated_at".to_owned()),
            ("sort", "asc".to_owned()),
            ("per_page", self.settings.page_size.to_string()),
        ];
        if !self.settings.labels.is_empty() {
            query.push(("labels", self.settings.labels.join(",")));
        }
        if let Some(assignee) = &self.settings.assignee {
            query.push(("assignee_username", assignee.clone()));
        }
        if let Some(since) = since {
            query.push(("updated_after", since.to_owned()));
        }
        let (issues, more) = self.pages(&self.issues_url(), &query, |_| true)?;
        let items = issues
            .iter()
            .filter_map(|issue| item_of(&self.settings.project, issue))
            .collect();
        Ok((items, more))
    }
}

impl Source for Gitlab {
    fn poll(&self, request: SourcePoll) -> Result<SourceBatch, ProviderError> {
        let since = request.cursor.as_deref().map(|cursor| {
            settled_cursor(cursor, self.settings.settling_secs).unwrap_or_else(|| cursor.to_owned())
        });
        let (mut items, more) = self.changed_issues(since.as_deref())?;
        // Pages filled by what the margin repeats would keep every poll from what follows them.
        if more
            && since != request.cursor
            && !items
                .iter()
                .any(|item| Some(item.updated.as_str()) > request.cursor.as_deref())
        {
            items = self.changed_issues(request.cursor.as_deref())?.0;
        }
        // What the margin repeats is older than the cursor, which never moves back for it.
        let cursor = items
            .iter()
            .map(|item| item.updated.clone())
            .chain(request.cursor)
            .max();
        Ok(SourceBatch { items, cursor })
    }
}

impl Tracker for Gitlab {
    fn capabilities(&self, item: Option<String>) -> Result<TrackerCapabilities, ProviderError> {
        let mut capabilities = self.project_capabilities()?;
        if let Some(item) = item {
            let iid = self.iid(&item)?;
            // A task takes no children; an issue that cannot be read takes none either.
            let kind = self
                .get_json(&self.issue_url(iid), &[])
                .ok()
                .and_then(|issue| Some(issue.get("issue_type")?.as_str()?.to_owned()));
            if kind.as_deref() != Some("issue") {
                capabilities.children = "none".to_owned();
            }
        }
        Ok(capabilities)
    }

    fn issue(&self, item: String) -> Result<TrackerIssue, ProviderError> {
        let iid = self.iid(&item)?;
        let issue = self.get_json(&self.issue_url(iid), &[])?;
        let mut read = issue_of(&self.settings.project, &issue)
            .ok_or_else(|| failure("GitLab answered with an issue this provider cannot read"))?;
        if issue.get("issue_type").and_then(serde_json::Value::as_str) == Some("task") {
            read.parent = self.parent_of(iid);
        }
        Ok(read)
    }

    fn replies(&self, request: ReplyPoll) -> Result<ReplyBatch, ProviderError> {
        let wanted: Vec<u64> = request
            .items
            .iter()
            .map(|item| self.iid(item))
            .collect::<Result<_, _>>()?;
        let mut cursor = request.since.clone();
        let mut replies = Vec::new();
        let mut access: BTreeMap<u64, u64> = BTreeMap::new();
        let since = request.since.as_deref();
        for iid in wanted {
            // Newest first, so a page that reaches the cursor is the last one read.
            let (notes, _) = self.pages(
                &format!("{}/notes", self.issue_url(iid)),
                &[
                    ("order_by", "updated_at".to_owned()),
                    ("sort", "desc".to_owned()),
                    ("per_page", self.settings.page_size.to_string()),
                ],
                |page| {
                    page.iter().all(|note| {
                        since.is_none_or(|since| {
                            note.get("updated_at")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|updated| updated >= since)
                        })
                    })
                },
            )?;
            let mut author: Option<Option<String>> = None;
            for note in notes.iter().rev() {
                let text = |key: &str| note.get(key).and_then(serde_json::Value::as_str);
                let Some(updated) = text("updated_at") else {
                    continue;
                };
                if since.is_some_and(|since| updated < since) {
                    continue;
                }
                if cursor.as_deref().is_none_or(|current| updated > current) {
                    cursor = Some(updated.to_owned());
                }
                // Label, assignee and state changes are notes too, written by GitLab.
                if note.get("system").and_then(serde_json::Value::as_bool) == Some(true) {
                    continue;
                }
                let username = note
                    .pointer("/author/username")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let user_id = note
                    .pointer("/author/id")
                    .and_then(serde_json::Value::as_u64);
                let permitted = self
                    .settings
                    .reply_authors
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(username))
                    || author
                        .get_or_insert_with(|| {
                            self.get_json(&self.issue_url(iid), &[])
                                .ok()
                                .and_then(|issue| {
                                    Some(issue.pointer("/author/username")?.as_str()?.to_owned())
                                })
                        })
                        .as_deref()
                        .is_some_and(|author| author.eq_ignore_ascii_case(username))
                    || user_id.is_some_and(|id| {
                        *access.entry(id).or_insert_with(|| self.access_of(id))
                            >= self.settings.reply_min_access
                    });
                replies.push(Reply {
                    id: note
                        .get("id")
                        .and_then(serde_json::Value::as_u64)
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    item: self.item_id(iid),
                    author_id: username.to_owned(),
                    author: username.to_owned(),
                    permitted,
                    // By the marker alone: the token may be a person's, whose replies count.
                    own: text("body").is_some_and(|body| body.contains(MARKER_PREFIX)),
                    body: text("body").unwrap_or_default().to_owned(),
                    created: text("created_at").unwrap_or_default().to_owned(),
                });
            }
        }
        Ok(ReplyBatch { replies, cursor })
    }

    fn post(&self, post: TrackerPost) -> Result<PostedComment, ProviderError> {
        let iid = self.iid(&post.item)?;
        if post.body.trim().is_empty() {
            return Err(bad_value(
                "the comment is empty, and GitLab refuses such a comment",
            ));
        }
        let mut body = post.body;
        let marker: String = post
            .marker
            .as_deref()
            .unwrap_or("comment")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
            .collect();
        body.push_str(&format!("\n\n{MARKER_PREFIX}{marker} -->"));
        let payload = serde_json::json!({ "body": body });
        let notes = format!("{}/notes", self.issue_url(iid));
        let note = match &post.edit {
            Some(id) => {
                if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(bad_value("a comment ID is a number"));
                }
                read_json(self.call("PUT", &format!("{notes}/{id}"), &[], Some(&payload))?)?
            }
            None => read_json(self.call("POST", &notes, &[], Some(&payload))?)?,
        };
        let id = note
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| failure("GitLab answered without a comment ID"))?;
        Ok(PostedComment {
            id: id.to_string(),
            url: self
                .web_project()
                .map(|web| format!("{web}/-/issues/{iid}#note_{id}")),
        })
    }

    fn update(&self, update: TrackerUpdate) -> Result<(), ProviderError> {
        let iid = self.iid(&update.item)?;
        if update.transition.is_some() {
            return Err(Self::unsupported(
                "make a workflow transition",
                "set the state to open or closed",
            ));
        }
        let state_event = match update.state.as_deref() {
            None => None,
            Some("closed") => Some("close"),
            Some("open") => Some("reopen"),
            Some(other) => {
                return Err(bad_value(&format!(
                    "state {other:?} is not `open` or `closed`"
                )));
            }
        };
        let mut changes = serde_json::Map::new();
        // GitLab takes labels as one comma-separated list, and ignores one the issue lacks.
        if !update.add_labels.is_empty() {
            changes.insert("add_labels".to_owned(), update.add_labels.join(",").into());
        }
        if !update.remove_labels.is_empty() {
            changes.insert(
                "remove_labels".to_owned(),
                update.remove_labels.join(",").into(),
            );
        }
        if let Some(event) = state_event {
            changes.insert("state_event".to_owned(), event.into());
        }
        if !update.add_assignees.is_empty() || !update.remove_assignees.is_empty() {
            // GitLab replaces the whole list, so the change starts from the issue's own.
            let issue = self.get_json(&self.issue_url(iid), &[])?;
            let mut assignees: Vec<(u64, String)> = issue
                .get("assignees")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|person| {
                    Some((
                        person.get("id")?.as_u64()?,
                        person.get("username")?.as_str()?.to_owned(),
                    ))
                })
                .collect();
            assignees.retain(|(_, name)| {
                !update
                    .remove_assignees
                    .iter()
                    .any(|gone| gone.eq_ignore_ascii_case(name))
            });
            for name in &update.add_assignees {
                if !assignees
                    .iter()
                    .any(|(_, have)| have.eq_ignore_ascii_case(name))
                {
                    assignees.push((self.member_id(name)?, name.clone()));
                }
            }
            changes.insert(
                "assignee_ids".to_owned(),
                assignees
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>()
                    .into(),
            );
        }
        if changes.is_empty() {
            return Ok(());
        }
        self.call(
            "PUT",
            &self.issue_url(iid),
            &[],
            Some(&serde_json::Value::Object(changes)),
        )?;
        Ok(())
    }

    fn create(&self, spec: IssueSpec) -> Result<TrackerIssue, ProviderError> {
        let capabilities = self.project_capabilities()?;
        if !capabilities.create {
            return Err(Self::unsupported(
                "create issues here",
                "read and comment on them",
            ));
        }
        // Only an issue takes children, and a child is a task under it.
        let parent = match spec.parent.as_deref() {
            Some(parent) => {
                let iid = self.iid(parent)?;
                self.get_json(&self.issue_url(iid), &[])
                    .ok()
                    .filter(|issue| {
                        issue.get("issue_type").and_then(serde_json::Value::as_str) == Some("issue")
                    })
                    .and_then(|issue| Some((iid, issue.get("id")?.as_u64()?)))
            }
            None => None,
        };
        // A label the project lacks would be created by GitLab: keep the ones it has.
        let labels: Vec<&str> = spec
            .labels
            .iter()
            .filter(|label| capabilities.labels.contains(label))
            .map(String::as_str)
            .collect();
        let mut payload = serde_json::json!({
            "title": spec.title,
            "description": spec.body,
            "labels": labels.join(","),
        });
        if parent.is_some() {
            payload["issue_type"] = "task".into();
        }
        let created = read_json(self.call("POST", &self.issues_url(), &[], Some(&payload))?)?;
        let mut issue = issue_of(&self.settings.project, &created)
            .ok_or_else(|| failure("GitLab answered with an issue this provider cannot read"))?;
        if let (Some((parent_iid, parent_id)), Some(id)) = (
            parent,
            created.get("id").and_then(serde_json::Value::as_u64),
        ) {
            // The task exists either way; the caller names its parent in text when unattached.
            if self.attach(id, parent_id).is_ok() {
                issue.parent = Some(self.item_id(parent_iid));
            }
        }
        Ok(issue)
    }

    fn link(&self, link: TrackerLink) -> Result<(), ProviderError> {
        let from = self.iid(&link.from)?;
        let Some(to) = link.to.as_deref() else {
            if link.url.is_some() {
                return Err(Self::unsupported(
                    "link an issue to an address",
                    "link two issues, or name the address in a comment",
                ));
            }
            return Err(bad_value("a link names an issue or an address"));
        };
        let to = self.iid(to)?;
        let response = self.send(
            "POST",
            &format!("{}/links", self.issue_url(from)),
            &[],
            Some(&serde_json::json!({
                "target_project_id": self.settings.project,
                "target_issue_iid": to.to_string(),
                "link_type": "relates_to",
            })),
        )?;
        // Two issues already linked answer 409: the state asked for.
        if response.status().as_u16() == 409 {
            return Ok(());
        }
        self.check(&response)
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

/// Percent-encodes one path segment; a project path's `/` becomes `%2F`, as GitLab asks.
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

fn issue_of(project: &str, issue: &serde_json::Value) -> Option<TrackerIssue> {
    let item = item_of(project, issue)?;
    let username = |value: &serde_json::Value| value.get("username")?.as_str().map(str::to_owned);
    Some(TrackerIssue {
        id: item.id,
        native_id: issue
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .map(|id| id.to_string())
            .unwrap_or_default(),
        title: item.title,
        body: item.body,
        state: match issue.get("state").and_then(serde_json::Value::as_str) {
            Some("closed") => "closed",
            _ => "open",
        }
        .to_owned(),
        labels: item.labels,
        assignees: issue
            .get("assignees")
            .and_then(serde_json::Value::as_array)
            .map(|people| people.iter().filter_map(username).collect())
            .unwrap_or_default(),
        url: item.url,
        updated: item.updated,
        parent: None,
        author_id: item.author,
        priority: None,
        code: item.code,
        code_tag: item.code_tag,
    })
}

fn bad_value(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

fn item_of(project: &str, issue: &serde_json::Value) -> Option<SourceItem> {
    let text = |key: &str| issue.get(key).and_then(serde_json::Value::as_str);
    let iid = issue.get("iid")?.as_u64()?;
    Some(SourceItem {
        id: format!("{project}#{iid}"),
        title: text("title")?.to_owned(),
        body: text("description").unwrap_or_default().to_owned(),
        url: text("web_url").map(str::to_owned),
        labels: issue
            .get("labels")
            .and_then(serde_json::Value::as_array)
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| label.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        author: issue
            .pointer("/author/username")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        updated: text("updated_at")?.to_owned(),
        code: Some(iid.to_string()),
        code_tag: Some("gl".to_owned()),
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
        let header = r#"<https://gitlab.com/api/v4/x?page=2>; rel="next", <https://gitlab.com/api/v4/x?page=1>; rel="first""#;
        assert_eq!(
            next_link(Some(header)).as_deref(),
            Some("https://gitlab.com/api/v4/x?page=2")
        );
        assert_eq!(next_link(Some(r#"<https://a/x>; rel="last""#)), None);
        assert_eq!(next_link(None), None);
    }

    #[test]
    fn settings_reject_unknown_keys_bad_projects_and_insecure_apis() {
        assert!(Gitlab::from_settings("project = \"g/p\"").is_ok());
        assert!(Gitlab::from_settings("project = \"g/sub/p\"").is_ok());
        assert!(Gitlab::from_settings("target = \"g/p\"").is_ok());
        assert!(Gitlab::from_settings("project = \"g/p\"\nlabel = \"x\"").is_err());
        assert!(Gitlab::from_settings("project = \"nope\"").is_err());
        assert!(Gitlab::from_settings("project = \"g/../p\"").is_err());
        assert!(Gitlab::from_settings("project = \"g//p\"").is_err());
        assert!(
            Gitlab::from_settings("project = \"g/p\"\napi_url = \"http://example.com\"").is_err()
        );
        assert!(
            Gitlab::from_settings("project = \"g/p\"\napi_url = \"http://127.0.0.1:9\"").is_ok()
        );
    }

    #[test]
    fn graphql_and_web_addresses_sit_beside_the_api_root() {
        let gitlab = Gitlab::from_settings("project = \"g/sub/p\"").unwrap();
        assert_eq!(gitlab.graphql_url(), "https://gitlab.com/api/graphql");
        assert_eq!(
            gitlab.web_project().as_deref(),
            Some("https://gitlab.com/g/sub/p")
        );
        assert_eq!(
            gitlab.project_url(),
            "https://gitlab.com/api/v4/projects/g%2Fsub%2Fp"
        );
        let elsewhere = Gitlab::from_settings(
            "project = \"g/p\"\napi_url = \"https://git.example.com/gitlab/api/v4/\"",
        )
        .unwrap();
        assert_eq!(
            elsewhere.graphql_url(),
            "https://git.example.com/gitlab/api/graphql"
        );
        assert_eq!(
            elsewhere.web_project().as_deref(),
            Some("https://git.example.com/gitlab/g/p")
        );
    }

    #[test]
    fn pagination_never_leaves_the_api_origin() {
        let api = Url::parse("https://gitlab.com/api/v4").unwrap();
        assert!(same_origin(
            "https://gitlab.com/api/v4/projects?page=2",
            &api
        ));
        assert!(!same_origin("https://evil.example/projects?page=2", &api));
    }
}
