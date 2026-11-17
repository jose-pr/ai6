//! What an API key can report of itself, read on demand with no session and no model turn: the rate-limit headers a
//! provider puts on its answers, and the key information a LiteLLM-style gateway serves for the key it is given.
//!
//! Each source becomes buckets whose [`Source`] names it ([`SOURCE_HEADERS`], [`SOURCE_GATEWAY`]), so a reader tells a
//! provider's own limit from a gateway's budget. A source the provider lacks gives no bucket and is no error: the report
//! is then [`UsageReport::unsupported`], saying so.
//!
//! The key goes only to the address the account itself names (its model endpoint), the one it is sent to for every turn.
//! No message here carries a request's address, a header or a body.

use core::time::Duration;

use serde_json::Value;

use crate::UsageReport;
use crate::reader::{Exchange, ReadError, get_exchange, rfc3339_seconds};
use crate::usage::{Bucket, Coverage, Refill, Source, Status, Unit};

/// The source of a bucket read from a provider's rate-limit response headers.
pub const SOURCE_HEADERS: &str = "response_headers";
/// The source of a bucket read from a gateway's key information.
pub const SOURCE_GATEWAY: &str = "gateway";
/// The path of a LiteLLM-style gateway's information on the key it is given, under the gateway's root.
pub const GATEWAY_KEY_INFO_PATH: &str = "/key/info";

/// The headers each limit is named by, in the two dialects the providers use: Anthropic's
/// `anthropic-ratelimit-<what>-{limit,remaining,reset}` (reset an RFC 3339 time) and OpenAI's, which gateways and other
/// providers copy, `x-ratelimit-{limit,remaining,reset}-<what>` (reset a duration such as `6m0s`).
const LIMITS: &[(&str, &str, Unit, &str, Option<&str>)] = &[
    (
        "requests",
        "Requests",
        Unit::Requests,
        "anthropic-ratelimit-requests",
        Some("requests"),
    ),
    (
        "tokens",
        "Tokens",
        Unit::Tokens,
        "anthropic-ratelimit-tokens",
        Some("tokens"),
    ),
    (
        "input_tokens",
        "Input tokens",
        Unit::Tokens,
        "anthropic-ratelimit-input-tokens",
        None,
    ),
    (
        "output_tokens",
        "Output tokens",
        Unit::Tokens,
        "anthropic-ratelimit-output-tokens",
        None,
    ),
];

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(each, _)| each.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
}

fn count(text: Option<&str>) -> Option<u64> {
    text?.parse::<u64>().ok()
}

/// A duration as OpenAI writes it (`1s`, `6m0s`, `1h2m3.5s`, `20ms`), in whole seconds rounded up.
#[must_use]
pub fn duration_seconds(text: &str) -> Option<u64> {
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }
    let mut total = 0.0_f64;
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let (number, after) = rest.split_at(digits);
        let number: f64 = number.parse().ok()?;
        let unit_end = after
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(after.len());
        let (unit, next) = after.split_at(unit_end);
        total += number
            * match unit {
                "h" => 3600.0,
                "m" => 60.0,
                "s" => 1.0,
                "ms" => 0.001,
                _ => return None,
            };
        rest = next;
    }
    seconds_up(total)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a finite, non-negative figure no larger than a day's seconds times a year"
)]
fn seconds_up(total: f64) -> Option<u64> {
    (total.is_finite() && (0.0..=3.2e8).contains(&total)).then(|| total.ceil() as u64)
}

/// The limits the response headers name: one bucket for each of requests, tokens, input tokens and output tokens whose
/// limit or remaining count they carry, with the time it refills when they say.
#[must_use]
pub fn rate_buckets(headers: &[(String, String)], now: i64) -> Vec<Bucket> {
    let mut buckets = Vec::new();
    for (id, label, unit, anthropic, openai) in LIMITS {
        let (limit, remaining, reset) = if header(headers, &format!("{anthropic}-limit")).is_some()
            || header(headers, &format!("{anthropic}-remaining")).is_some()
        {
            let reset = header(headers, &format!("{anthropic}-reset")).and_then(rfc3339_seconds);
            (
                count(header(headers, &format!("{anthropic}-limit"))),
                count(header(headers, &format!("{anthropic}-remaining"))),
                reset,
            )
        } else if let Some(what) = openai {
            let reset = header(headers, &format!("x-ratelimit-reset-{what}"))
                .and_then(duration_seconds)
                .and_then(|after| i64::try_from(after).ok())
                .map(|after| now.saturating_add(after));
            (
                count(header(headers, &format!("x-ratelimit-limit-{what}"))),
                count(header(headers, &format!("x-ratelimit-remaining-{what}"))),
                reset,
            )
        } else {
            continue;
        };
        if limit.is_none() && remaining.is_none() {
            continue;
        }
        let mut bucket = Bucket::new(*id);
        bucket.label = Some((*label).to_string());
        bucket.models = Coverage::All;
        bucket.unit = unit.clone();
        bucket.limit = limit;
        bucket.remaining = remaining;
        bucket.used = limit
            .zip(remaining)
            .map(|(limit, remaining)| limit.saturating_sub(remaining));
        bucket.status = if remaining == Some(0) {
            Status::Exhausted
        } else {
            Status::Ok
        };
        bucket.refill = Refill::FixedWindow {
            length: None,
            resets_at: reset,
        };
        bucket.observed_at = now;
        bucket.source = Source::Other(SOURCE_HEADERS.to_string());
        buckets.push(bucket);
    }
    buckets
}

/// Asks `url` (a cheap authenticated read, such as the provider's model list) with `headers` and reads the rate limits
/// off whatever answer comes, a 429 included, since that is where they matter most.
///
/// # Errors
///
/// [`ReadError::Status`] for any answer but 200 and 429 (a refused key is 401 or 403), or the read's own failure.
pub fn read_rate_limits(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
    now: i64,
) -> Result<Vec<Bucket>, ReadError> {
    let exchange: Exchange = get_exchange(url, headers, timeout)?;
    if !matches!(exchange.status, 200 | 429) {
        return Err(ReadError::Status(exchange.status));
    }
    Ok(rate_buckets(&exchange.headers, now))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to a range u64 holds first"
)]
fn micro_usd(dollars: f64) -> u64 {
    (dollars * 1_000_000.0).round().clamp(0.0, 9.0e18) as u64
}

/// A budget duration as LiteLLM writes it: a number and `s`, `m`, `h`, `d` or `mo` (`30d`, `1mo`).
fn budget_seconds(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    let each = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "mo" => 30 * 86_400,
        _ => return None,
    };
    number.checked_mul(each)
}

/// The buckets a gateway's key information makes: the spend against the budget (in millionths of a dollar, refilled
/// when the budget resets, or never), and the requests and tokens a minute the key may use.
///
/// `document` is the answer's `info` object, or the answer itself when it has none.
#[must_use]
pub fn gateway_buckets(document: &Value, now: i64) -> Vec<Bucket> {
    let info = document
        .get("info")
        .filter(|info| info.is_object())
        .unwrap_or(document);
    let number = |name: &str| info.get(name).and_then(Value::as_f64);
    let mut buckets = Vec::new();
    let gateway = || Source::Other(SOURCE_GATEWAY.to_string());
    let spend = number("spend");
    let max = number("max_budget");
    if spend.is_some() || max.is_some() {
        let mut bucket = Bucket::new("budget");
        bucket.label = Some("Gateway budget".to_string());
        bucket.models = Coverage::All;
        bucket.unit = Unit::MicroUsd;
        bucket.used = spend.map(micro_usd);
        bucket.limit = max.map(micro_usd);
        bucket.remaining = max
            .zip(spend)
            .map(|(max, spend)| micro_usd((max - spend).max(0.0)));
        bucket.status = match (spend, max) {
            (Some(spend), Some(max)) if spend >= max => Status::Exhausted,
            (_, Some(_)) => Status::Ok,
            _ => Status::Unknown,
        };
        let duration = info
            .get("budget_duration")
            .and_then(Value::as_str)
            .and_then(budget_seconds);
        let resets_at = info
            .get("budget_reset_at")
            .and_then(Value::as_str)
            .and_then(|text| {
                rfc3339_seconds(text).or_else(|| rfc3339_seconds(&format!("{text}Z")))
            });
        bucket.refill = if duration.is_some() || resets_at.is_some() {
            Refill::FixedWindow {
                length: duration.map(Duration::from_secs),
                resets_at,
            }
        } else if max.is_some() {
            Refill::Never
        } else {
            Refill::Unknown
        };
        bucket.observed_at = now;
        bucket.source = gateway();
        buckets.push(bucket);
    }
    for (id, label, unit, name) in [
        (
            "rpm",
            "Gateway requests per minute",
            Unit::Requests,
            "rpm_limit",
        ),
        (
            "tpm",
            "Gateway tokens per minute",
            Unit::Tokens,
            "tpm_limit",
        ),
    ] {
        let Some(limit) = info.get(name).and_then(Value::as_u64) else {
            continue;
        };
        let mut bucket = Bucket::new(id);
        bucket.label = Some(label.to_string());
        bucket.models = Coverage::All;
        bucket.unit = unit;
        bucket.limit = Some(limit);
        bucket.status = Status::Unknown;
        bucket.refill = Refill::FixedWindow {
            length: Some(Duration::from_secs(60)),
            resets_at: None,
        };
        bucket.observed_at = now;
        bucket.source = gateway();
        buckets.push(bucket);
    }
    buckets
}

/// The gateway's root for an account whose model endpoint is `base`: the address without a trailing `/v1`.
fn gateway_root(base: &str) -> &str {
    let base = base.trim_end_matches('/');
    base.strip_suffix("/v1").unwrap_or(base)
}

/// Reads the key information a LiteLLM-style gateway at `base` serves for `token`, the key it was given, and makes
/// buckets of it.
///
/// # Errors
///
/// [`ReadError::Status`] when the address does not serve it (a provider's own API answers 404) or refuses the key, and
/// [`ReadError::Malformed`] for an answer that is not a document.
pub fn read_gateway(
    base: &str,
    token: &str,
    timeout: Duration,
    now: i64,
) -> Result<Vec<Bucket>, ReadError> {
    let url = format!("{}{GATEWAY_KEY_INFO_PATH}", gateway_root(base));
    let bearer = format!("Bearer {token}");
    let exchange = get_exchange(&url, &[("Authorization", bearer.as_str())], timeout)?;
    if exchange.status != 200 {
        return Err(ReadError::Status(exchange.status));
    }
    let document = exchange.document.ok_or(ReadError::Malformed)?;
    Ok(gateway_buckets(&document, now))
}

/// Where a key's figures are read from.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct KeySources<'a> {
    /// The cheap authenticated read that carries the rate-limit headers (a model list).
    pub rate_url: &'a str,
    /// The headers that authenticate it with the key.
    pub rate_headers: &'a [(&'a str, &'a str)],
    /// A gateway's model endpoint and the key to give its key-information route; `None` for the provider's own address,
    /// which serves no such route.
    pub gateway: Option<(&'a str, &'a str)>,
    /// How long each read may take in all.
    pub timeout: Duration,
    /// The time now in Unix seconds.
    pub now: i64,
}

impl<'a> KeySources<'a> {
    /// Sources for the rate-limit read at `rate_url` sent `rate_headers`, ten seconds each, at `now`.
    #[must_use]
    pub const fn new(rate_url: &'a str, rate_headers: &'a [(&'a str, &'a str)], now: i64) -> Self {
        Self {
            rate_url,
            rate_headers,
            gateway: None,
            timeout: Duration::from_secs(10),
            now,
        }
    }

    /// Also asks the gateway at `base` for the key's information, given `token`.
    #[must_use]
    pub const fn gateway(mut self, base: &'a str, token: &'a str) -> Self {
        self.gateway = Some((base, token));
        self
    }

    /// Allows `timeout` to each read.
    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// The report of what the key says of itself: every bucket each source gave, its source on each. A source the key's
/// provider lacks is left out; none at all is [`UsageReport::unsupported`] saying so.
///
/// # Errors
///
/// The key was refused (`401` or `403` from the provider, and from the gateway if asked), or no source could be reached
/// at all. One source failing while another answers is no error.
pub fn read_key(sources: &KeySources<'_>) -> Result<UsageReport, ReadError> {
    let headers = read_rate_limits(
        sources.rate_url,
        sources.rate_headers,
        sources.timeout,
        sources.now,
    );
    let gateway = sources
        .gateway
        .map(|(base, token)| read_gateway(base, token, sources.timeout, sources.now));
    let mut buckets = Vec::new();
    let mut failure = None;
    for read in [Some(headers), gateway].into_iter().flatten() {
        match read {
            Ok(found) => buckets.extend(found),
            Err(error) => failure = failure.or(Some(error)),
        }
    }
    if buckets.is_empty() {
        return match failure {
            Some(error @ (ReadError::Status(401 | 403) | ReadError::Unreachable)) => Err(error),
            _ => Ok(UsageReport::unsupported(
                "the provider returned no rate-limit headers for this key and no gateway budget was found",
            )),
        };
    }
    let mut report = UsageReport::new();
    let mut seen: Vec<Source> = Vec::new();
    for bucket in &buckets {
        if !seen.contains(&bucket.source) {
            seen.push(bucket.source.clone());
        }
    }
    report.sources = seen;
    report.observed_at = sources.now;
    report.buckets = buckets;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn durations_are_read_as_openai_writes_them() {
        assert_eq!(duration_seconds("1s"), Some(1));
        assert_eq!(duration_seconds("6m0s"), Some(360));
        assert_eq!(duration_seconds("1h2m3.5s"), Some(3724));
        assert_eq!(duration_seconds("20ms"), Some(1));
        assert_eq!(duration_seconds("0s"), Some(0));
        assert_eq!(duration_seconds(""), None);
        assert_eq!(duration_seconds("soon"), None);
        assert_eq!(duration_seconds("5"), None);
    }

    #[test]
    fn anthropic_headers_make_a_bucket_each() {
        let now = 1_794_800_000;
        let buckets = rate_buckets(
            &headers(&[
                ("anthropic-ratelimit-requests-limit", "50"),
                ("anthropic-ratelimit-requests-remaining", "49"),
                ("anthropic-ratelimit-requests-reset", "2026-11-16T20:00:00Z"),
                ("anthropic-ratelimit-tokens-limit", "40000"),
                ("anthropic-ratelimit-tokens-remaining", "0"),
                ("anthropic-ratelimit-input-tokens-limit", "30000"),
                ("anthropic-ratelimit-input-tokens-remaining", "29000"),
            ]),
            now,
        );
        let ids: Vec<&str> = buckets.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["requests", "tokens", "input_tokens"]);
        assert_eq!(buckets[0].used, Some(1));
        assert_eq!(buckets[0].unit, Unit::Requests);
        assert_eq!(
            buckets[0].refill,
            Refill::FixedWindow {
                length: None,
                resets_at: Some(1_794_859_200)
            }
        );
        assert_eq!(buckets[1].status, Status::Exhausted);
        assert_eq!(buckets[2].status, Status::Ok);
        assert_eq!(buckets[2].source, Source::Other(SOURCE_HEADERS.to_string()));
    }

    #[test]
    fn openai_headers_reset_after_a_duration() {
        let buckets = rate_buckets(
            &headers(&[
                ("x-ratelimit-limit-requests", "500"),
                ("x-ratelimit-remaining-requests", "499"),
                ("x-ratelimit-reset-requests", "120ms"),
                ("x-ratelimit-limit-tokens", "30000"),
                ("x-ratelimit-remaining-tokens", "29900"),
                ("x-ratelimit-reset-tokens", "6m0s"),
            ]),
            1000,
        );
        assert_eq!(buckets.len(), 2);
        assert_eq!(
            buckets[0].refill,
            Refill::FixedWindow {
                length: None,
                resets_at: Some(1001)
            }
        );
        assert_eq!(
            buckets[1].refill,
            Refill::FixedWindow {
                length: None,
                resets_at: Some(1360)
            }
        );
        assert_eq!(buckets[1].unit, Unit::Tokens);
    }

    #[test]
    fn no_headers_and_unreadable_figures_make_no_bucket() {
        assert!(rate_buckets(&headers(&[("content-type", "x")]), 1).is_empty());
        assert!(rate_buckets(&headers(&[("x-ratelimit-limit-requests", "many")]), 1).is_empty());
    }

    #[test]
    fn a_gateway_key_makes_a_budget_and_rate_limits() {
        let buckets = gateway_buckets(
            &json!({"key": "hashed", "info": {
                "spend": 1.5, "max_budget": 10.0, "budget_duration": "30d",
                "budget_reset_at": "2026-12-01T00:00:00Z", "rpm_limit": 60, "tpm_limit": 100_000
            }}),
            5,
        );
        let ids: Vec<&str> = buckets.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["budget", "rpm", "tpm"]);
        let budget = &buckets[0];
        assert_eq!(
            (budget.used, budget.limit, budget.remaining),
            (Some(1_500_000), Some(10_000_000), Some(8_500_000))
        );
        assert_eq!(budget.status, Status::Ok);
        assert_eq!(
            budget.refill,
            Refill::FixedWindow {
                length: Some(Duration::from_secs(30 * 86_400)),
                resets_at: Some(1_796_083_200)
            }
        );
        assert_eq!(buckets[1].limit, Some(60));
        assert_eq!(buckets[2].unit, Unit::Tokens);
    }

    #[test]
    fn a_spent_budget_is_exhausted_and_one_with_no_cap_is_only_a_spend() {
        let spent = gateway_buckets(&json!({"info": {"spend": 10.0, "max_budget": 10.0}}), 5);
        assert_eq!(spent[0].status, Status::Exhausted);
        assert_eq!(spent[0].refill, Refill::Never);
        let open = gateway_buckets(&json!({"info": {"spend": 2.0, "max_budget": null}}), 5);
        assert_eq!(open[0].limit, None);
        assert_eq!(open[0].used, Some(2_000_000));
        assert_eq!(open[0].status, Status::Unknown);
        assert!(gateway_buckets(&json!({"info": {"models": []}}), 5).is_empty());
    }

    #[test]
    fn a_model_endpoint_with_v1_is_the_gateway_root_without_it() {
        assert_eq!(gateway_root("https://gw.example/v1/"), "https://gw.example");
        assert_eq!(gateway_root("https://gw.example"), "https://gw.example");
    }
}
