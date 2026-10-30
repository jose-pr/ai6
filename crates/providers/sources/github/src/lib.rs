// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-source-github`: the `github` Source provider. It lists the open issues (and pull
//! requests) of one repository that carry the configured labels, and comments the outcome
//! of the Mission back on the issue. Requests are blocking; the host calls them off its
//! async runtime.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

ai6::guatiao::providers!(Github);

use std::time::Duration;

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{Provider, Status};
use ai6::{CxReport, Source, SourceBatch, SourceConfig, SourceItem, SourcePoll};
use serde::Deserialize;
use url::Url;

/// The provider ID an Operation names under `sources`.
pub const PROVIDER_ID: &str = "github";

/// One Operation's settings, from `[operation.<id>.sources.github]`.
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
    /// Label added to the issue when its Mission finishes successfully.
    done_label: Option<String>,
    /// Label added to the issue when its Mission fails.
    failed_label: Option<String>,
    /// Seconds one request may take. Default 30; 0 is refused.
    #[serde(default = "default_request_timeout_secs")]
    request_timeout_secs: u64,
    /// Issues asked for per page. Default 100; 0 is refused.
    #[serde(default = "default_page_size")]
    page_size: u32,
    /// Pages one poll reads at most. Default 5; 0 is refused.
    #[serde(default = "default_max_pages")]
    max_pages: usize,
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

fn default_labels() -> Vec<String> {
    vec!["ai6:ready".to_owned()]
}

fn default_token_env() -> String {
    "GITHUB_TOKEN".to_owned()
}

fn default_api_url() -> String {
    "https://api.github.com".to_owned()
}

/// GitHub issues as an AI6 Source.
#[derive(Debug, Provider)]
#[provider(
    Source,
    id = "github",
    name = "GitHub issues",
    config = SourceConfig,
    crate = ::ai6::guatiao
)]
pub struct Github {
    settings: Settings,
    api: Url,
    agent: ureq::Agent,
}

impl Github {
    /// Builds the provider from the Operation's settings table, serialized as TOML.
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

    fn get(
        &self,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<ureq::http::Response<ureq::Body>, ProviderError> {
        let mut request = self
            .agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "ai6-source-github");
        for (key, value) in query {
            request = request.query(key, value);
        }
        if let Some(token) = self.token() {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let response = request
            .call()
            .map_err(|e| failure(&format!("request failed: {e}")))?;
        self.check(&response)?;
        Ok(response)
    }

    fn post(&self, path: &str, body: &serde_json::Value) -> Result<(), ProviderError> {
        let token = self.token().ok_or_else(|| {
            failure(&format!(
                "no token: set the environment variable {}",
                self.settings.token_env
            ))
        })?;
        let response = self
            .agent
            .post(&self.endpoint(path))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "ai6-source-github")
            .header("Authorization", &format!("Bearer {token}"))
            .send_json(body)
            .map_err(|e| failure(&format!("request failed: {e}")))?;
        self.check(&response)
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
            other => Err(failure(&format!("GitHub answered status {other}"))),
        }
    }

    fn issues_url(&self) -> String {
        self.endpoint(&format!("/repos/{}/issues", self.settings.repo))
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

    fn report(&self, report: CxReport) -> Result<(), ProviderError> {
        let number = report
            .item_id
            .rsplit_once('#')
            .and_then(|(_, number)| number.parse::<u64>().ok())
            .ok_or_else(|| failure("the item ID has no issue number"))?;
        let base = format!("/repos/{}/issues/{number}", self.settings.repo);
        self.post(
            &format!("{base}/comments"),
            &serde_json::json!({ "body": report.body }),
        )?;
        let label = if report.outcome == "done" {
            &self.settings.done_label
        } else {
            &self.settings.failed_label
        };
        if let Some(label) = label {
            self.post(
                &format!("{base}/labels"),
                &serde_json::json!({ "labels": [label] }),
            )?;
        }
        Ok(())
    }
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
