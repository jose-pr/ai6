//! The plan limits of a Claude account, read on demand with no session and no model turn.
//!
//! This is the account's side, not the harness's: it takes an [`AccountGrant`] and reads the one document Claude
//! Code's own `/usage` view reads, `GET /api/oauth/usage` on the OAuth API, with the login's access token. It starts
//! no program and knows no session. Only a subscription login (and a long-lived token that carries the profile scope)
//! has plan limits: a Console login answers [`UsageReport::unsupported`]. An API key or gateway token has none, and
//! reports what it can instead: the rate-limit headers of Anthropic's model list and, behind a gateway's own
//! `base_url`, the gateway's key information (see [`coqui_acp_api::key_usage`]).
//!
//! The access token is never renewed here, since renewing rotates the refresh token, which only the file's owner may
//! store; an expired one is refused saying so. The token goes only to the OAuth API address, which the account's
//! `base_url` (the model endpoint) never moves.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::time::Duration;

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::ProviderError;
use coqui_acp_api::key_usage::{KeySources, read_key as read_key_sources};
use coqui_acp_api::reader::{ReadError, get_json, now_seconds, rfc3339_seconds};
use coqui_acp_api::usage::{Bucket, Coverage, Refill, Source, Status as Room, Unit};
use coqui_acp_api::{AccountGrant, Credentials, HARNESS_LOGIN, UsageReport};
use serde_json::Value;

use crate::accounts::{self, API_KEY, AUTH_TOKEN, CONSOLE, FILE_KEY, LOGIN, OAUTH_TOKEN};

use crate::limits::{WEEK, WINDOWS};

/// The OAuth API's address, where Claude Code reads its usage.
pub const DEFAULT_ENDPOINT: &str = "https://api.anthropic.com";
/// The document's path on it.
pub const USAGE_PATH: &str = "/api/oauth/usage";
/// The beta the OAuth API takes a login's token under.
pub const OAUTH_BETA: &str = "oauth-2025-04-20";
/// The scope a login needs for the read: without it the account has no plan limits to show.
pub const PROFILE_SCOPE: &str = "user:profile";

/// How a read is made.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ReadOptions {
    /// The OAuth API's address, in place of [`DEFAULT_ENDPOINT`]: for a test's stand-in or a network's own route. The
    /// token is sent there. It is also where an API key or gateway token whose account names no `base_url` is read.
    pub endpoint: Option<String>,
    /// How long the read may take in all.
    pub timeout: Duration,
    /// The time now in Unix seconds, in place of the clock's.
    pub now: Option<i64>,
}

impl ReadOptions {
    /// The defaults: the real API, ten seconds.
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

/// The windows of the plan the account has used, as the OAuth API reports them: the five-hour session, the weekly
/// window, and a weekly window per model where the plan has one.
///
/// # Errors
///
/// `BAD_VALUE`: an account the provider does not take, or a credential that is not the kind's. `GONE`: the login's
/// access token expired, or the API refused it (401). `INTERNAL`: the API could not be reached or answered something
/// else. An account with no plan limits is no error: it is [`UsageReport::unsupported`], saying why.
pub fn read_limits(
    grant: &AccountGrant,
    options: &ReadOptions,
) -> Result<UsageReport, ProviderError> {
    let grant = accounts::resolved(grant)?;
    accounts::check(&grant.reference())?;
    let now = options.now.unwrap_or_else(now_seconds);
    let credentials = grant.credentials.expose();
    let token = match grant.kind.as_str() {
        LOGIN => {
            let text = credentials
                .get(FILE_KEY)
                .filter(|text| !text.is_empty() && credentials.keys().all(|key| key == FILE_KEY))
                .ok_or_else(|| {
                    error(
                        Status::GUATIAO_ERR_BAD_VALUE,
                        "the login's credentials are missing or not the kind's",
                    )
                })?;
            let oauth = serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|file| file.get("claudeAiOauth").cloned())
                .filter(Value::is_object)
                .ok_or_else(|| {
                    error(
                        Status::GUATIAO_ERR_BAD_VALUE,
                        "the login file holds no claudeAiOauth",
                    )
                })?;
            let token = oauth
                .get("accessToken")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .ok_or_else(|| {
                    error(Status::GUATIAO_ERR_GONE, "the login holds no access token")
                })?;
            if let Some(scopes) = oauth.get("scopes").and_then(Value::as_array)
                && !scopes
                    .iter()
                    .any(|scope| scope.as_str() == Some(PROFILE_SCOPE))
            {
                return Ok(not_offered(
                    "this login lacks the profile scope Claude Code reads plan limits with",
                ));
            }
            if oauth
                .get("expiresAt")
                .and_then(Value::as_i64)
                .is_some_and(|ms| ms / 1000 <= now)
            {
                return Err(error(
                    Status::GUATIAO_ERR_GONE,
                    "the login's access token has expired: renew the login, then read again",
                ));
            }
            token.to_string()
        }
        OAUTH_TOKEN => credentials
            .get("token")
            .filter(|token| !token.is_empty() && credentials.keys().all(|key| key == "token"))
            .ok_or_else(|| {
                error(
                    Status::GUATIAO_ERR_BAD_VALUE,
                    "the credential is missing or not the kind's",
                )
            })?
            .to_string(),
        CONSOLE => {
            return Ok(not_offered(
                "a Console login is billed to an API organization, which has no plan limits",
            ));
        }
        API_KEY | AUTH_TOKEN => return read_key(&grant, credentials, options, now),
        HARNESS_LOGIN => {
            return Ok(not_offered(
                "the login Claude Code keeps for itself is not held by the host, which cannot read its limits",
            ));
        }
        _ => {
            return Ok(not_offered("this kind of account has no plan limits"));
        }
    };
    let base = options
        .endpoint
        .as_deref()
        .unwrap_or(DEFAULT_ENDPOINT)
        .trim_end_matches('/');
    let bearer = format!("Bearer {token}");
    let agent = format!("coqui-acp-claude/{}", env!("CARGO_PKG_VERSION"));
    let document = get_json(
        &format!("{base}{USAGE_PATH}"),
        &[
            ("Authorization", bearer.as_str()),
            ("anthropic-beta", OAUTH_BETA),
            ("User-Agent", agent.as_str()),
        ],
        options.timeout,
    );
    match document {
        Ok(document) => {
            let mut report = report_of(&document, now);
            report.account = Some(grant.id.clone());
            Ok(report)
        }
        // The token lacks what the read needs: the account has no plan limits.
        Err(ReadError::Status(403)) => Ok(not_offered(
            "Claude refused this token the plan limits: it has no profile scope",
        )),
        Err(ReadError::Status(401)) => Err(error(
            Status::GUATIAO_ERR_GONE,
            "Claude refused the login (status 401): renew it, then read again",
        )),
        Err(why) => Err(error(
            Status::GUATIAO_ERR_INTERNAL,
            format!("Claude's usage could not be read: {why}"),
        )),
    }
}

/// The API version the model list is asked under.
const API_VERSION: &str = "2023-06-01";

/// What an API key or gateway token says of itself: Anthropic's rate-limit headers on the model list (a free read), and
/// a LiteLLM-style gateway's budget when the account's `base_url` names one. The key goes only to that `base_url`, where
/// every request of the account goes.
fn read_key(
    grant: &AccountGrant,
    credentials: &Credentials,
    options: &ReadOptions,
    now: i64,
) -> Result<UsageReport, ProviderError> {
    let (name, key) = if grant.kind == API_KEY {
        ("api_key", "x-api-key")
    } else {
        ("auth_token", "Authorization")
    };
    let secret = credentials
        .get(name)
        .filter(|secret| !secret.is_empty() && credentials.keys().all(|each| each == name))
        .ok_or_else(|| {
            error(
                Status::GUATIAO_ERR_BAD_VALUE,
                "the credential is missing or not the kind's",
            )
        })?;
    let custom = crate::accounts::endpoint(&grant.settings)?;
    let base = custom
        .as_deref()
        .or(options.endpoint.as_deref())
        .unwrap_or(DEFAULT_ENDPOINT)
        .trim_end_matches('/');
    let bearer = format!("Bearer {secret}");
    let value = if grant.kind == API_KEY {
        secret
    } else {
        bearer.as_str()
    };
    let headers = [(key, value), ("anthropic-version", API_VERSION)];
    let url = format!("{base}/v1/models?limit=1");
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

/// `percent` as thousandths, clamped to 0..=1000 before the cast.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=1000 first"
)]
fn permille(percent: f64) -> u64 {
    (percent * 10.0).round().clamp(0.0, 1000.0) as u64
}

/// When a window resets: an RFC 3339 text, or Unix seconds (or milliseconds) as a number.
fn resets_at(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::String(text) => rfc3339_seconds(text),
        Value::Number(number) => number.as_i64().map(crate::limits::unix_seconds),
        _ => None,
    }
}

fn window(
    id: &str,
    label: &str,
    length: Option<u64>,
    models: Coverage,
    fields: &Value,
    now: i64,
) -> Bucket {
    let mut bucket = Bucket::new(id);
    bucket.label = Some(label.to_string());
    bucket.model_names = crate::limits::model_names(&models);
    bucket.models = models;
    bucket.unit = Unit::Permille;
    let percent = fields.get("utilization").and_then(Value::as_f64);
    if let Some(percent) = percent {
        bucket.used = Some(permille(percent));
        bucket.limit = Some(1000);
    }
    bucket.status = match percent {
        Some(percent) if percent >= 100.0 => Room::Exhausted,
        Some(_) => Room::Ok,
        None => Room::Unknown,
    };
    bucket.refill = Refill::FixedWindow {
        length: length.map(Duration::from_secs),
        resets_at: resets_at(fields.get("resets_at")),
    };
    bucket.observed_at = now;
    bucket.source = Source::Reported;
    bucket
}

/// A name usable in a bucket's id.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

/// The report the usage document makes: each window it names and is not null, then each per-model weekly window of
/// the model-scoped weekly entries of `limits`. No window at all is an account with no plan limits.
fn report_of(document: &Value, now: i64) -> UsageReport {
    let mut buckets: Vec<Bucket> = Vec::new();
    for known in WINDOWS {
        let Some(fields) = document.get(known.id).filter(|fields| fields.is_object()) else {
            continue;
        };
        let models = match known.model {
            Some(model) => Coverage::Only {
                models: alloc::vec![model.to_string()],
            },
            None if known.all_models => Coverage::All,
            None => Coverage::Unknown,
        };
        buckets.push(window(
            known.id,
            known.label,
            Some(known.seconds),
            models,
            fields,
            now,
        ));
    }
    // The plan's per-model weekly limits: `limits` holds one entry per limit, and a `weekly_scoped` one names the model it
    // is for under `scope.model.display_name` (`Fable`). It is a bucket of its own whether or not the plan counts it yet
    // (`is_active`).
    if let Some(limits) = document.get("limits").and_then(Value::as_array) {
        for fields in limits.iter().filter(|fields| fields.is_object()) {
            if fields.get("kind").and_then(Value::as_str) != Some("weekly_scoped") {
                continue;
            }
            let Some(name) = fields
                .pointer("/scope/model/display_name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            let id = format!("seven_day_model_{}", slug(name));
            if buckets.iter().any(|bucket| bucket.id == id) {
                continue;
            }
            let mut bucket = window(
                &id,
                &format!("Weekly, {name}"),
                Some(WEEK),
                Coverage::Only {
                    models: alloc::vec![name.to_lowercase()],
                },
                &Value::Null,
                now,
            );
            if let Some(percent) = fields.get("percent").and_then(Value::as_f64) {
                bucket.used = Some(permille(percent));
                bucket.limit = Some(1000);
                bucket.status = if percent >= 100.0 {
                    Room::Exhausted
                } else {
                    Room::Ok
                };
            }
            bucket.refill = Refill::FixedWindow {
                length: Some(Duration::from_secs(WEEK)),
                resets_at: resets_at(fields.get("resets_at")),
            };
            buckets.push(bucket);
        }
    }
    if buckets.is_empty() {
        return not_offered("the account has no plan limits to report");
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
    use serde_json::json;

    #[test]
    fn the_document_makes_a_window_per_name_and_per_model() {
        let now = 1_794_850_000;
        let report = report_of(
            &json!({
                "five_hour": {"utilization": 12.5, "resets_at": "2026-11-16T20:00:00+00:00"},
                "seven_day": {"utilization": 100.0, "resets_at": "2026-11-20T10:00:00Z"},
                "seven_day_opus": null,
                "seven_day_sonnet": {"utilization": 5.0, "resets_at": 1_795_000_000},
                "limits": [
                    {"kind": "weekly_all", "percent": 100, "resets_at": "2026-11-20T10:00:00Z", "scope": null},
                    {"kind": "weekly_scoped", "percent": 33, "resets_at": "2026-11-20T10:00:00Z", "scope": {"model": {"id": null, "display_name": "Fable 2"}}},
                ],
            }),
            now,
        );
        let ids: Vec<&str> = report.buckets.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "five_hour",
                "seven_day",
                "seven_day_sonnet",
                "seven_day_model_fable_2"
            ]
        );
        let session = &report.buckets[0];
        assert_eq!((session.used, session.limit), (Some(125), Some(1000)));
        assert_eq!(session.status, Room::Ok);
        assert_eq!(
            session.refill,
            Refill::FixedWindow {
                length: Some(Duration::from_secs(18_000)),
                resets_at: Some(1_794_859_200)
            }
        );
        assert_eq!(report.buckets[1].status, Room::Exhausted);
        assert_eq!(
            report.buckets[2].models,
            Coverage::Only {
                models: alloc::vec!["sonnet".to_string()]
            }
        );
        assert_eq!(
            report.buckets[3].models,
            Coverage::Only {
                models: alloc::vec!["fable 2".to_string()]
            }
        );
        assert_eq!(report.observed_at, now);
        // The two plain windows say they cover every model; a model's own window names it.
        assert_eq!(report.buckets[0].models, Coverage::All);
        assert!(report.buckets[0].model_names.is_empty());
        assert_eq!(report.buckets[2].model_names, ["Sonnet"]);
        assert_eq!(report.buckets[3].model_names, ["Fable 2"]);
    }

    #[test]
    fn a_document_with_no_window_is_no_plan_limits() {
        let report = report_of(&json!({"five_hour": null, "seven_day": null}), 1);
        assert!(!report.supported);
        assert!(report.unsupported_because.is_some());
    }
}
