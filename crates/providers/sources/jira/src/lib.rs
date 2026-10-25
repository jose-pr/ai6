// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-source-jira`: the `jira` Source provider. It runs a JQL search for the issues that
//! carry the configured labels and comments the outcome of the Mission back on the issue,
//! optionally moving it through a workflow transition. Requests are blocking; the host
//! calls them off its async runtime.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

ai6::guatiao::providers!(Jira);

mod adf;

use std::time::Duration;

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{Provider, Status};
use ai6::{CxReport, Source, SourceBatch, SourceConfig, SourceItem, SourcePoll};
use serde::Deserialize;
use url::Url;

/// The provider ID an Operation names under `sources`.
pub const PROVIDER_ID: &str = "jira";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PAGE_SIZE: u32 = 50;
const MAX_PAGES: usize = 5;
const FIELDS: &str = "summary,description,labels,reporter,updated";

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

/// One Operation's settings, from `[operation.<id>.sources.jira]`.
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
    /// Workflow transition applied when the Mission finishes successfully.
    done_transition: Option<String>,
    /// Workflow transition applied when the Mission fails.
    failed_transition: Option<String>,
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

/// Jira issues as an AI6 Source.
#[derive(Debug, Provider)]
#[provider(
    Source,
    id = "jira",
    name = "Jira issues",
    config = SourceConfig,
    crate = ::ai6::guatiao
)]
pub struct Jira {
    settings: Settings,
    site: Url,
    agent: ureq::Agent,
}

impl Jira {
    /// Builds the provider from the Operation's settings table, serialized as TOML.
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
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        Ok(Self {
            settings,
            site,
            agent,
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

    fn get(
        &self,
        url: &str,
        query: &[(&str, String)],
    ) -> Result<ureq::http::Response<ureq::Body>, ProviderError> {
        let mut request = self
            .agent
            .get(url)
            .header("Accept", "application/json")
            .header("Authorization", &self.authorization()?);
        for (key, value) in query {
            request = request.query(key, value);
        }
        let response = request
            .call()
            .map_err(|e| failure(&format!("request failed: {e}")))?;
        self.check(&response)?;
        Ok(response)
    }

    fn post(&self, url: &str, body: &serde_json::Value) -> Result<(), ProviderError> {
        let response = self
            .agent
            .post(url)
            .header("Accept", "application/json")
            .header("Authorization", &self.authorization()?)
            .send_json(body)
            .map_err(|e| failure(&format!("request failed: {e}")))?;
        self.check(&response)
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
                "Jira rejected the request (check `jql`, `project` and the labels)",
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

    fn transition(&self, key: &str, wanted: &str) -> Result<(), ProviderError> {
        let url = self.api(&format!("/issue/{key}/transitions"));
        let mut response = self.get(&url, &[])?;
        let listing: serde_json::Value = response
            .body_mut()
            .read_json()
            .map_err(|e| failure(&format!("unreadable response: {e}")))?;
        let id = listing
            .get("transitions")
            .and_then(serde_json::Value::as_array)
            .and_then(|transitions| {
                transitions.iter().find_map(|transition| {
                    let id = transition.get("id")?.as_str()?;
                    let name = transition.get("name")?.as_str()?;
                    (id == wanted || name.eq_ignore_ascii_case(wanted)).then(|| id.to_owned())
                })
            })
            .ok_or_else(|| failure(&format!("{key} has no transition named {wanted:?}")))?;
        self.post(&url, &serde_json::json!({ "transition": { "id": id } }))
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
            ("maxResults", PAGE_SIZE.to_string()),
        ];

        let mut items = Vec::new();
        let mut page_token: Option<String> = None;
        let mut start_at = 0_u64;
        for _ in 0..MAX_PAGES {
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

    fn report(&self, report: CxReport) -> Result<(), ProviderError> {
        let key = report.item_id.as_str();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(failure("the item ID is not a Jira issue key"));
        }
        let body = match self.settings.deployment {
            Deployment::Cloud => adf::document(&report.body),
            Deployment::Server => serde_json::Value::String(report.body.clone()),
        };
        self.post(
            &self.api(&format!("/issue/{key}/comment")),
            &serde_json::json!({ "body": body }),
        )?;
        let transition = if report.outcome == "done" {
            &self.settings.done_transition
        } else {
            &self.settings.failed_transition
        };
        if let Some(transition) = transition {
            self.transition(key, transition)?;
        }
        Ok(())
    }
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
