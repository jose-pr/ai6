//! The plan limits of a Codex account, read on demand with no session and no model turn.
//!
//! This is the account's side, not the harness's: it takes an [`AccountGrant`] and reads the document Codex's own
//! `account/rateLimits/read` reads, `GET <chatgpt_base_url>/wham/usage`, with the login's access token and its
//! `ChatGPT-Account-Id`. It starts no program (not `codex app-server` either) and knows no session. An API key has no
//! plan limits, and reports what it can instead: the rate-limit headers of OpenAI's model list and, behind a gateway's own
//! `openai_base_url`, the gateway's key information (see [`coqui_acp_api::key_usage`]).
//!
//! The access token is never renewed here, since renewing rotates the refresh token, which only the file's owner may
//! store; an expired one is refused saying so. The token goes to the account's own `chatgpt_base_url`, where Codex
//! sends it as well.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use core::time::Duration;

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::ProviderError;
use coqui_acp_api::key_usage::{KeySources, read_key as read_key_sources};
use coqui_acp_api::reader::{ReadError, get_json, now_seconds};
use coqui_acp_api::usage::Source;
use coqui_acp_api::{AccountGrant, HARNESS_LOGIN, UsageReport};
use coqui_acp_harness::account::jwt_claims;
use serde_json::{Value, json};

use crate::accounts::{self, ACCESS_TOKEN, API_KEY, CHATGPT, FILE_KEY};
use crate::limits::Account;

/// The address Codex reads `chatgpt_base_url` as when nothing sets it.
pub const DEFAULT_ENDPOINT: &str = "https://chatgpt.com/backend-api";

/// How a read is made.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ReadOptions {
    /// The address to read from, in place of the account's `chatgpt_base_url` and [`DEFAULT_ENDPOINT`]: for a test's
    /// stand-in or a network's own route. The token is sent there. It is also where an API key whose account names no
    /// `openai_base_url` is read.
    pub endpoint: Option<String>,
    /// How long the read may take in all.
    pub timeout: Duration,
    /// The time now in Unix seconds, in place of the clock's.
    pub now: Option<i64>,
}

impl ReadOptions {
    /// The defaults: the account's endpoint, ten seconds.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            endpoint: None,
            timeout: Duration::from_secs(10),
            now: None,
        }
    }

    /// Reads from `endpoint`.
    #[must_use]
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Allows `timeout` in all.
    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Takes `now` for the time.
    #[must_use]
    pub const fn now(mut self, now: i64) -> Self {
        self.now = Some(now);
        self
    }
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self::new()
    }
}

fn error(status: Status, why: impl Into<String>) -> ProviderError {
    ProviderError::new(status, &why.into())
}

/// The address Codex sends model requests to when the account names no `openai_base_url`.
const API: &str = "https://api.openai.com/v1";

/// What an OpenAI API key says of itself: the rate-limit headers on the model list (a read that costs no tokens), and a
/// LiteLLM-style gateway's budget when the account's `openai_base_url` names one. The key goes only to that address,
/// where every request of the account goes.
fn read_key(
    grant: &AccountGrant,
    options: &ReadOptions,
    now: i64,
) -> Result<UsageReport, ProviderError> {
    let credentials = grant.credentials.expose();
    let secret = credentials
        .get("api_key")
        .filter(|secret| !secret.is_empty() && credentials.keys().all(|each| each == "api_key"))
        .ok_or_else(|| {
            error(
                Status::GUATIAO_ERR_BAD_VALUE,
                "the credential is missing or not the kind's",
            )
        })?;
    let custom = accounts::endpoint(&grant.settings, "openai_base_url")?;
    let base = custom
        .as_deref()
        .or(options.endpoint.as_deref())
        .unwrap_or(API)
        .trim_end_matches('/');
    let bearer = format!("Bearer {secret}");
    let headers = [("Authorization", bearer.as_str())];
    let url = format!("{base}/models");
    let mut sources = KeySources::new(&url, &headers, now).timeout(options.timeout);
    if custom.is_some() {
        sources = sources.gateway(base, secret);
    }
    match read_key_sources(&sources) {
        Ok(mut report) => {
            report.account = Some(grant.id.clone());
            Ok(report)
        }
        Err(ReadError::Status(401 | 403)) => Err(error(
            Status::GUATIAO_ERR_INTERNAL,
            "the provider refused the key (status 401 or 403)",
        )),
        Err(why) => Err(error(
            Status::GUATIAO_ERR_INTERNAL,
            format!("the key's rate limits could not be read: {why}"),
        )),
    }
}

fn not_offered(why: &str) -> UsageReport {
    UsageReport::unsupported(why)
}

/// The URL of the usage document under `base`, as Codex builds it: a ChatGPT backend address (one with `/backend-api`
/// in it) has `/wham/usage`, any other address `/api/codex/usage`. A trailing `/codex`, the model endpoint's, is dropped.
fn usage_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let base = base.strip_suffix("/codex").unwrap_or(base);
    if base.contains("/backend-api") {
        format!("{base}/wham/usage")
    } else {
        format!("{base}/api/codex/usage")
    }
}

/// The windows of the plan the account has used, as Codex reports them: the five-hour and the weekly window, and a
/// pair per additional metered limit (a model's own allowance).
///
/// # Errors
///
/// `BAD_VALUE`: an account the provider does not take, or a credential that is not the kind's. `GONE`: the access token
/// expired, or ChatGPT refused it (401 or 403). `INTERNAL`: the backend could not be reached or answered something
/// else. An account with no plan limits is no error: it is [`UsageReport::unsupported`], saying why.
pub fn read_limits(
    grant: &AccountGrant,
    options: &ReadOptions,
) -> Result<UsageReport, ProviderError> {
    let grant = accounts::resolved(grant)?;
    accounts::check(&grant.reference())?;
    let now = options.now.unwrap_or_else(now_seconds);
    let credentials = grant.credentials.expose();
    let bad = |why: &str| error(Status::GUATIAO_ERR_BAD_VALUE, why);
    let (token, account_id) = match grant.kind.as_str() {
        CHATGPT => {
            let text = credentials
                .get(FILE_KEY)
                .filter(|text| !text.is_empty() && credentials.keys().all(|key| key == FILE_KEY))
                .ok_or_else(|| bad("the login's credentials are missing or not the kind's"))?;
            let tokens = serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|file| file.get("tokens").cloned())
                .filter(Value::is_object)
                .ok_or_else(|| bad("the login file holds no tokens"))?;
            let token = tokens
                .get("access_token")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .ok_or_else(|| {
                    error(Status::GUATIAO_ERR_GONE, "the login holds no access token")
                })?;
            let id = tokens
                .get("account_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .or_else(|| claimed_account(token));
            (token.to_string(), id)
        }
        ACCESS_TOKEN => {
            let token = credentials
                .get("access_token")
                .filter(|token| {
                    !token.is_empty() && credentials.keys().all(|key| key == "access_token")
                })
                .ok_or_else(|| bad("the credential is missing or not the kind's"))?;
            (token.to_string(), claimed_account(token))
        }
        API_KEY => return read_key(&grant, options, now),
        HARNESS_LOGIN => {
            return Ok(not_offered(
                "the login Codex keeps for itself is not held by the host, which cannot read its limits",
            ));
        }
        _ => return Ok(not_offered("this kind of account has no plan limits")),
    };
    if jwt_claims(&token)
        .and_then(|claims| claims.get("exp").and_then(Value::as_i64))
        .is_some_and(|exp| exp <= now)
    {
        return Err(error(
            Status::GUATIAO_ERR_GONE,
            "the login's access token has expired: renew the login, then read again",
        ));
    }
    let base = match &options.endpoint {
        Some(endpoint) => Some(endpoint.clone()),
        None => accounts::endpoint(&grant.settings, "chatgpt_base_url")?,
    };
    let url = usage_url(base.as_deref().unwrap_or(DEFAULT_ENDPOINT));
    let bearer = format!("Bearer {token}");
    let agent = format!("coqui-acp-codex/{}", env!("CARGO_PKG_VERSION"));
    let mut headers = alloc::vec![
        ("Authorization", bearer.as_str()),
        ("User-Agent", agent.as_str()),
    ];
    if let Some(id) = &account_id {
        headers.push(("ChatGPT-Account-Id", id.as_str()));
    }
    match get_json(&url, &headers, options.timeout) {
        Ok(document) => {
            let mut report = report_of(&document, now);
            report.account = Some(grant.id.clone());
            Ok(report)
        }
        Err(ReadError::Status(401 | 403)) => Err(error(
            Status::GUATIAO_ERR_GONE,
            "ChatGPT refused the login: renew it, then read again",
        )),
        Err(why) => Err(error(
            Status::GUATIAO_ERR_INTERNAL,
            format!("Codex's usage could not be read: {why}"),
        )),
    }
}

/// The ChatGPT account an access token names in its claims.
fn claimed_account(token: &str) -> Option<String> {
    jwt_claims(token)?
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// One window of the document in the shape of Codex's own answer, which the harness's reader already turns into
/// buckets: its length in minutes, and when it ends.
fn window(fields: &Value, now: i64) -> Option<Value> {
    let fields = fields.as_object()?;
    let ends = fields.get("reset_at").and_then(Value::as_i64).or_else(|| {
        fields
            .get("reset_after_seconds")
            .and_then(Value::as_i64)
            .map(|after| now.saturating_add(after))
    });
    Some(json!({
        "usedPercent": fields.get("used_percent").and_then(Value::as_f64),
        "windowDurationMins": fields
            .get("limit_window_seconds")
            .and_then(Value::as_u64)
            .map(|seconds| seconds / 60),
        "resetsAt": ends,
    }))
}

/// One limit of the document as a snapshot of Codex's answer.
fn snapshot(id: &str, name: Option<&str>, plan: Option<&str>, limit: &Value, now: i64) -> Value {
    json!({
        "limitId": id,
        "limitName": name,
        "planType": plan,
        "primary": limit.get("primary_window").and_then(|w| window(w, now)),
        "secondary": limit.get("secondary_window").and_then(|w| window(w, now)),
    })
}

/// The report the usage document makes. No limit in it is an account with no plan limits.
fn report_of(document: &Value, now: i64) -> UsageReport {
    let plan = document.get("plan_type").and_then(Value::as_str);
    let mut snapshots = BTreeMap::new();
    let main = document.get("rate_limit").filter(|limit| limit.is_object());
    if let Some(main) = main {
        snapshots.insert(
            "codex".to_string(),
            snapshot("codex", None, plan, main, now),
        );
    }
    if let Some(extra) = document
        .get("additional_rate_limits")
        .and_then(Value::as_array)
    {
        for entry in extra {
            let Some(limit) = entry.get("rate_limit").filter(|limit| limit.is_object()) else {
                continue;
            };
            let Some(id) = entry
                .get("metered_feature")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            let name = entry.get("limit_name").and_then(Value::as_str);
            snapshots.insert(id.to_string(), snapshot(id, name, plan, limit, now));
        }
    }
    let answer = json!({
        "rateLimitsByLimitId": snapshots,
        "ordinaryUsageAllowed": main
            .and_then(|main| main.get("allowed"))
            .and_then(Value::as_bool)
            .filter(|allowed| !allowed),
    });
    let mut buckets = Account::default().read(&answer);
    if buckets.is_empty() {
        return not_offered("the account has no plan limits to report");
    }
    for bucket in &mut buckets {
        bucket.observed_at = now;
    }
    let mut report = UsageReport::new();
    report.sources = alloc::vec![Source::Reported];
    report.observed_at = now;
    report.buckets = buckets;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_follows_codexs_own_rule() {
        assert_eq!(
            usage_url("https://chatgpt.com/backend-api/"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            usage_url("https://chatgpt.com/backend-api/codex"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            usage_url("http://127.0.0.1:1234"),
            "http://127.0.0.1:1234/api/codex/usage"
        );
    }

    #[test]
    fn the_document_makes_buckets_for_each_window_and_each_extra_limit() {
        let report = report_of(
            &json!({
                "plan_type": "plus",
                "rate_limit": {"allowed": true, "limit_reached": false,
                    "primary_window": {"used_percent": 17, "limit_window_seconds": 18000, "reset_after_seconds": 3600, "reset_at": 1_794_850_000},
                    "secondary_window": {"used_percent": 42.5, "limit_window_seconds": 604_800, "reset_after_seconds": 90_000}},
                "additional_rate_limits": [{"limit_name": "Spark", "metered_feature": "spark",
                    "rate_limit": {"allowed": true, "primary_window": {"used_percent": 3, "limit_window_seconds": 18000, "reset_at": 1_794_846_000}, "secondary_window": null}}],
            }),
            1_794_800_000,
        );
        let ids: Vec<&str> = report.buckets.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["codex:five_hour", "codex:weekly", "spark:five_hour"]);
        assert_eq!(report.buckets[0].used, Some(170));
        assert_eq!(report.buckets[1].used, Some(425));
        assert_eq!(
            report.buckets[1].refill,
            coqui_acp_api::usage::Refill::FixedWindow {
                length: Some(Duration::from_secs(604_800)),
                resets_at: Some(1_794_890_000)
            }
        );
        assert_eq!(report.buckets[2].label.as_deref(), Some("Spark"));
        assert!(
            report
                .buckets
                .iter()
                .all(|b| b.observed_at == 1_794_800_000)
        );
    }

    #[test]
    fn a_disallowed_account_is_exhausted_and_one_with_no_limit_is_not_offered() {
        let report = report_of(
            &json!({"rate_limit": {"allowed": false, "primary_window": {"used_percent": 10, "limit_window_seconds": 18000, "reset_at": 5}}}),
            1,
        );
        assert_eq!(
            report.buckets[0].status,
            coqui_acp_api::usage::Status::Exhausted
        );
        let none = report_of(&json!({"plan_type": "free"}), 1);
        assert!(!none.supported);
    }
}
