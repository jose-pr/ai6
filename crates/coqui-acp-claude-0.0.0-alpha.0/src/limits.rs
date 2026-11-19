//! Usage, quota and usage limits as Claude Code reports them.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use core::time::Duration;

use coqui_acp_api::usage::{
    Bucket, Coverage, LimitHit, ModelSpend, Refill, Source, Spend, Status, Unit, infer_limit,
};
use coqui_acp_harness::event::Usage;
use serde_json::Value;

/// The words by which Claude Code says a quota stopped it.
const LIMIT_PHRASES: &[&str] = &["usage limit", "limit reached", "hit your limit"];

/// A Unix timestamp that may be in seconds or milliseconds, in seconds.
pub(crate) const fn unix_seconds(value: i64) -> i64 {
    if value > 100_000_000_000 {
        value / 1000
    } else {
        value
    }
}

/// Claude Code's refusal to go on until a quota resets: a `rate_limit_event` whose status is `rejected`, or an error
/// `result` whose text names a limit (`usage limit`, `limit reached`, `hit your limit`) or whose `api_error_status`
/// is 429. A reset time comes from `resetsAt`, or from the number after a trailing `|` of the text; one given only
/// as a local clock time is left unset rather than guessed.
pub(crate) fn limit(value: &Value) -> Option<LimitHit> {
    match value.get("type").and_then(Value::as_str)? {
        "rate_limit_event" => {
            let info = value.get("rate_limit_info")?;
            if info.get("status").and_then(Value::as_str) != Some("rejected") {
                return None;
            }
            let mut limit = LimitHit::new(0);
            limit.resets_at = info
                .get("resetsAt")
                .and_then(Value::as_i64)
                .map(unix_seconds);
            limit.bucket = info
                .get("rateLimitType")
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(limit)
        }
        "result" if value.get("is_error").and_then(Value::as_bool) == Some(true) => {
            let text = value.get("result").and_then(Value::as_str)?;
            let lower = text.to_lowercase();
            let limited = value.get("api_error_status").and_then(Value::as_i64) == Some(429)
                || infer_limit(text, LIMIT_PHRASES).is_some();
            if !limited {
                return None;
            }
            let resets_at = text
                .rsplit_once('|')
                .and_then(|(_, tail)| tail.trim().parse::<i64>().ok())
                .map(unix_seconds);
            let bucket = if lower.contains("5-hour") || lower.contains("five-hour") {
                Some("five_hour".to_string())
            } else if lower.contains("weekly") || lower.contains("7-day") {
                Some("seven_day".to_string())
            } else {
                None
            };
            let mut limit = LimitHit::new(0);
            limit.resets_at = resets_at;
            limit.bucket = bucket;
            limit.source = Source::Inferred;
            limit.text = Some(text.to_string());
            Some(limit)
        }
        _ => None,
    }
}

/// How long a window named `name` lasts: the five-hour window, and each weekly one (`seven_day`, `seven_day_opus`, ...).
/// Any other name is no length rather than a guess.
fn window_length(name: &str) -> Option<Duration> {
    if name == "five_hour" {
        Some(Duration::from_secs(5 * 3600))
    } else if name == "seven_day" || name.starts_with("seven_day_") {
        Some(Duration::from_secs(7 * 24 * 3600))
    } else {
        None
    }
}

/// The models a window named `name` covers: a model-specific weekly window says so by its name, and no other window
/// says anything.
fn window_coverage(name: &str) -> Coverage {
    match name {
        "seven_day_opus" => Coverage::Only {
            models: alloc::vec!["opus".to_string()],
        },
        "seven_day_sonnet" => Coverage::Only {
            models: alloc::vec!["sonnet".to_string()],
        },
        _ => Coverage::Unknown,
    }
}

/// The buckets a `rate_limit_event` reports, whatever its status: each of `unifiedWindows`, or else the one it
/// names, or `unknown` when it names none. `utilization` is thousandths only when it is a fraction between 0 and 1,
/// since its scale is not documented. The status is Claude's own: `allowed_warning` is the only warning.
pub(crate) fn buckets(value: &Value) -> Vec<Bucket> {
    if value.get("type").and_then(Value::as_str) != Some("rate_limit_event") {
        return Vec::new();
    }
    let Some(info) = value.get("rate_limit_info") else {
        return Vec::new();
    };
    let window = |name: &str, fields: &Value| {
        let mut bucket = Bucket::new(name);
        bucket.unit = Unit::Permille;
        if let Some(thousandths) = fields
            .get("utilization")
            .and_then(Value::as_f64)
            .filter(|fraction| (0.0..=1.0).contains(fraction))
            .map(|fraction| permille(fraction * 1000.0))
        {
            bucket.used = Some(u64::from(thousandths));
            bucket.limit = Some(1000);
        }
        bucket.models = window_coverage(name);
        bucket.refill = Refill::FixedWindow {
            length: window_length(name),
            resets_at: fields
                .get("resetsAt")
                .and_then(Value::as_i64)
                .map(unix_seconds),
        };
        bucket.status = match fields.get("status").and_then(Value::as_str) {
            Some("allowed") => Status::Ok,
            Some("allowed_warning") => Status::Warning,
            Some("rejected") => Status::Exhausted,
            _ => Status::Unknown,
        };
        bucket
    };
    if let Some(windows) = info
        .get("unifiedWindows")
        .and_then(Value::as_object)
        .filter(|windows| !windows.is_empty())
    {
        return windows
            .iter()
            .map(|(name, fields)| window(name, fields))
            .collect();
    }
    vec![window(
        info.get("rateLimitType")
            .and_then(Value::as_str)
            .unwrap_or("unknown"),
        info,
    )]
}

/// The limit an `error` line says, by the same words as a result's; a reset time follows a trailing `|`.
pub(crate) fn limit_of_text(text: &str) -> Option<LimitHit> {
    limit(&serde_json::json!({"type": "result", "is_error": true, "result": text}))
}

/// `limit`, with what `hint`, the rejected event before it, knows. A hit that event backs is the harness's own word, so
/// it is `reported`, and the event's bucket and reset win over what the turn's text says; the text fills in what the
/// event lacks.
pub(crate) fn with_hint(mut limit: LimitHit, hint: Option<LimitHit>) -> LimitHit {
    if let Some(hint) = hint {
        limit.resets_at = hint.resets_at.or(limit.resets_at);
        limit.bucket = hint.bucket.or(limit.bucket);
        limit.source = hint.source;
    }
    limit
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=1000 first"
)]
fn permille(value: f64) -> u16 {
    value.round().clamp(0.0, 1000.0) as u16
}

/// Millionths of a dollar in `usd`, rounded to the nearest; `None` for a figure that is not a cost.
fn micro_usd(usd: f64) -> Option<u64> {
    (usd.is_finite() && usd >= 0.0).then(|| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked finite and not negative; a saturating cast is the bound"
        )]
        let micro = (usd * 1_000_000.0).round() as u64;
        micro
    })
}

/// What a `result` says the session has spent. `modelUsage` and `total_cost_usd` are running totals, one row per model
/// that spent; the `usage` beside them is the turn's own, so it stands in only when no row is given. A cost Claude Code
/// did not give stays unset, never 0. The session's id is the connection's to fill.
fn spend(value: &Value) -> Spend {
    let models = value
        .get("modelUsage")
        .and_then(Value::as_object)
        .filter(|models| !models.is_empty());
    let mut spend = Spend::new("");
    if models.is_none() {
        let counts = value.get("usage");
        let count = |key: &str| {
            counts
                .and_then(|usage| usage.get(key))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        spend.input = count("input_tokens");
        spend.output = count("output_tokens");
        spend.cache_read = count("cache_read_input_tokens");
        spend.cache_write = count("cache_creation_input_tokens");
        spend.reasoning = counts
            .and_then(|usage| usage.pointer("/output_tokens_details/thinking_tokens"))
            .and_then(Value::as_u64);
    }
    // `total_cost_usd` is the session's running total: beside only the turn's own counts it would be a partial pair, and
    // a cost is unset when it is partial.
    if models.is_some() {
        spend.cost_micro_usd = value
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .and_then(micro_usd);
    }
    for (name, used) in models.into_iter().flatten() {
        let tokens = |key: &str| used.get(key).and_then(Value::as_u64).unwrap_or(0);
        let mut row = ModelSpend::new(name.as_str());
        row.input = tokens("inputTokens");
        row.output = tokens("outputTokens");
        row.cache_read = tokens("cacheReadInputTokens");
        row.cache_write = tokens("cacheCreationInputTokens");
        row.cost_micro_usd = used
            .get("costUSD")
            .and_then(Value::as_f64)
            .and_then(micro_usd);
        spend.input = spend.input.saturating_add(row.input);
        spend.output = spend.output.saturating_add(row.output);
        spend.cache_read = spend.cache_read.saturating_add(row.cache_read);
        spend.cache_write = spend.cache_write.saturating_add(row.cache_write);
        if let Some(thinking) = used.get("thinkingTokens").and_then(Value::as_u64) {
            spend.reasoning = Some(spend.reasoning.unwrap_or(0).saturating_add(thinking));
        }
        spend.by_model.push(row);
    }
    spend
}

/// The usage a `result` reports: its `usage` token counts and `total_cost_usd`, running totals for the process, and
/// the model's context size from `modelUsage`, with the same as the session's [`Spend`]. `context_used` is the
/// caller's, from the last model request.
pub(crate) fn usage(value: &Value) -> Option<Usage> {
    if value.get("type").and_then(Value::as_str)? != "result" {
        return None;
    }
    let counts = value.get("usage");
    let cost = value.get("total_cost_usd").and_then(Value::as_f64);
    if counts.is_none() && cost.is_none() {
        return None;
    }
    let count = |key: &str| {
        counts
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let size = value
        .get("modelUsage")
        .and_then(Value::as_object)
        .and_then(|models| {
            models
                .values()
                .filter_map(|model| model.get("contextWindow").and_then(Value::as_u64))
                .max()
        });
    let mut usage = Usage::default();
    usage.input = count("input_tokens");
    usage.output = count("output_tokens");
    usage.cache_read = count("cache_read_input_tokens");
    usage.cache_write = count("cache_creation_input_tokens");
    usage.cumulative = true;
    usage.cost_usd = cost;
    usage.context_size = size;
    usage.spend = Some(spend(value));
    Some(usage)
}

/// The tokens of the model's context one request used: its input, cached or not, and its output.
pub(crate) fn context_used(message_usage: &Value) -> Option<u64> {
    let count = |key: &str| message_usage.get(key).and_then(Value::as_u64);
    let input = count("input_tokens")?;
    Some(
        input
            .saturating_add(count("cache_read_input_tokens").unwrap_or(0))
            .saturating_add(count("cache_creation_input_tokens").unwrap_or(0))
            .saturating_add(count("output_tokens").unwrap_or(0)),
    )
}

/// Whether a `result` says the turn did not do its work: `is_error`, which Claude Code sets with `subtype: success`
/// when no login reached a model, or an `error_*` subtype.
pub(crate) fn is_error(value: &Value) -> bool {
    value.get("is_error").and_then(Value::as_bool) == Some(true)
        || value
            .get("subtype")
            .and_then(Value::as_str)
            .is_some_and(|subtype| subtype.starts_with("error"))
}

/// What a failed `result` says: its `result` text, else its `errors`, one per line.
pub(crate) fn error_text(value: &Value) -> String {
    if let Some(text) = value.get("result").and_then(Value::as_str) {
        return text.to_string();
    }
    let errors: Vec<&str> = value
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if errors.is_empty() {
        value
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or("the turn failed")
            .to_string()
    } else {
        errors.join("\n")
    }
}

/// The tools a `result` lists under `permission_denials`, each named once, in the order first refused. A call's
/// input stays out, since it may hold anything the model wrote.
pub(crate) fn denials(value: &Value) -> Vec<String> {
    let mut tools: Vec<String> = Vec::new();
    for denial in value
        .get("permission_denials")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(tool) = denial.get("tool_name").and_then(Value::as_str)
            && !tools.iter().any(|known| known == tool)
        {
            tools.push(tool.to_string());
        }
    }
    tools
}
