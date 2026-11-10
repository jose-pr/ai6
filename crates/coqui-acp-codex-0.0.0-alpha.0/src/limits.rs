//! Usage, quota and usage limits as Codex's app server reports them.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use core::time::Duration;

use coqui_acp_api::usage::{Bucket, Coverage, LimitHit, Refill, Spend, Status, Unit};
use coqui_acp_harness::event::Usage;
use serde_json::Value;

/// The limit id of a snapshot that names none: Codex's own metered allowance.
const DEFAULT_LIMIT: &str = "codex";

/// The name of a quota window by its length, else its position.
fn window_name(window: &Value, position: &str) -> String {
    match window.get("windowDurationMins").and_then(Value::as_i64) {
        Some(300) => "five_hour".to_string(),
        Some(10_080) => "weekly".to_string(),
        _ => position.to_string(),
    }
}

/// `percent` as thousandths, clamped to 0..=1000 before the cast.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=1000 first"
)]
fn permille(percent: f64) -> u16 {
    (percent * 10.0).round().clamp(0.0, 1000.0) as u16
}

/// Puts the values `delta` has over `held`: an object is merged key by key, and a null is a value the update does not
/// have, never one that clears.
fn merge(held: &mut Value, delta: &Value) {
    match (held, delta) {
        (_, Value::Null) => {}
        (Value::Object(held), Value::Object(delta)) => {
            for (key, value) in delta {
                merge(held.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (held, delta) => *held = delta.clone(),
    }
}

/// The limit id of a snapshot.
fn limit_id(snapshot: &Value) -> &str {
    snapshot
        .get("limitId")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_LIMIT)
}

/// What Codex has said of the account's allowances: the last full `account/rateLimits/read`, with each sparse
/// `account/rateLimits/updated` merged into it, since an update holds only the values that changed.
#[derive(Debug, Default)]
pub(crate) struct Account {
    /// The snapshots by limit id.
    snapshots: BTreeMap<String, Value>,
    /// The backend's `ordinaryUsageAllowed` of the last read that carried one. `false` holds every bucket exhausted
    /// until a read says otherwise, since Codex says recovery is not to be inferred from percentages.
    allowed: Option<bool>,
}

impl Account {
    /// Takes a full `account/rateLimits/read` answer in place of what was known, and answers every bucket of it.
    pub(crate) fn read(&mut self, answer: &Value) -> Vec<Bucket> {
        self.snapshots.clear();
        let listed = answer
            .get("rateLimitsByLimitId")
            .and_then(Value::as_object)
            .filter(|listed| !listed.is_empty());
        match listed {
            Some(listed) => {
                for (id, snapshot) in listed.iter().filter(|(_, one)| one.is_object()) {
                    let mut snapshot = snapshot.clone();
                    if snapshot.get("limitId").is_none_or(Value::is_null) {
                        snapshot["limitId"] = Value::String(id.clone());
                    }
                    self.snapshots.insert(id.clone(), snapshot);
                }
            }
            None => {
                if let Some(snapshot) = answer.get("rateLimits").filter(|one| one.is_object()) {
                    self.snapshots
                        .insert(limit_id(snapshot).to_string(), snapshot.clone());
                }
            }
        }
        self.allowed = answer.get("ordinaryUsageAllowed").and_then(Value::as_bool);
        self.buckets()
    }

    /// Every bucket as it now stands.
    pub(crate) fn buckets(&self) -> Vec<Bucket> {
        self.snapshots
            .values()
            .flat_map(|snapshot| snapshot_buckets(snapshot, self.allowed))
            .collect()
    }

    /// Merges an `account/rateLimits/updated` notification into the snapshot of its limit and answers that limit's
    /// buckets as they now stand, or none when the update changed nothing of them; the other limits are untouched.
    pub(crate) fn update(&mut self, message: &Value) -> Vec<Bucket> {
        if message.get("method").and_then(Value::as_str) != Some("account/rateLimits/updated") {
            return Vec::new();
        }
        let Some(delta) = message
            .pointer("/params/rateLimits")
            .filter(|delta| delta.is_object())
        else {
            return Vec::new();
        };
        let held = self
            .snapshots
            .entry(limit_id(delta).to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        let before = snapshot_buckets(held, self.allowed);
        merge(held, delta);
        let after = snapshot_buckets(held, self.allowed);
        if after == before { Vec::new() } else { after }
    }

    /// The window of the limit an update named that is used up, the one that resets last: the usage-limit error that
    /// follows does not always repeat the reset time.
    pub(crate) fn exhausted(&self, message: &Value) -> Option<LimitHit> {
        if message.get("method").and_then(Value::as_str) != Some("account/rateLimits/updated") {
            return None;
        }
        let id = limit_id(message.pointer("/params/rateLimits")?);
        exhausted_in(id, self.snapshots.get(id)?)
    }

    /// The same over every limit the last read and the updates since hold.
    pub(crate) fn exhausted_any(&self) -> Option<LimitHit> {
        self.snapshots
            .iter()
            .filter_map(|(id, snapshot)| exhausted_in(id, snapshot))
            .max_by_key(|limit| limit.resets_at)
    }
}

/// The window of one snapshot that is used up, the one that resets last.
fn exhausted_in(id: &str, snapshot: &Value) -> Option<LimitHit> {
    ["primary", "secondary"]
        .iter()
        .filter_map(|position| {
            let window = snapshot.get(position)?;
            (window.get("usedPercent")?.as_f64()? >= 100.0).then(|| {
                let mut limit = LimitHit::new(0);
                limit.resets_at = window.get("resetsAt").and_then(Value::as_i64);
                limit.bucket = Some(format!("{id}:{}", window_name(window, position)));
                limit
            })
        })
        .max_by_key(|limit| limit.resets_at)
}

/// The buckets of one snapshot: each window as `<limit id>:<five_hour|weekly|primary|secondary>`, and the spend
/// control as `<limit id>:spend_control`. `usedPercent` is thousandths and a window is fixed, with its length when
/// Codex says it. A window at 100 percent is exhausted, and so is every bucket while the backend does not allow
/// ordinary usage. A snapshot that names a model covers only it. `credits.balance` is left out: its unit is not known.
fn snapshot_buckets(snapshot: &Value, allowed: Option<bool>) -> Vec<Bucket> {
    let id = limit_id(snapshot);
    let models = snapshot
        .get("normalModelSlug")
        .and_then(Value::as_str)
        .filter(|slug| !slug.is_empty())
        .map_or(Coverage::Unknown, |slug| Coverage::Only {
            models: alloc::vec![slug.to_string()],
        });
    let label = snapshot
        .get("limitName")
        .and_then(Value::as_str)
        .map(str::to_string);
    let blocked = allowed == Some(false);
    let mut buckets: Vec<Bucket> = ["primary", "secondary"]
        .iter()
        .filter_map(|position| {
            let window = snapshot.get(position).filter(|window| window.is_object())?;
            let mut bucket = Bucket::new(format!("{id}:{}", window_name(window, position)));
            bucket.label.clone_from(&label);
            bucket.models = models.clone();
            bucket.unit = Unit::Permille;
            let used = window.get("usedPercent").and_then(Value::as_f64);
            if let Some(thousandths) = used.map(permille) {
                bucket.used = Some(u64::from(thousandths));
                bucket.limit = Some(1000);
            }
            bucket.status = match used {
                Some(percent) if percent >= 100.0 => Status::Exhausted,
                _ if blocked => Status::Exhausted,
                Some(_) => Status::Ok,
                None => Status::Unknown,
            };
            bucket.refill = Refill::FixedWindow {
                length: window
                    .get("windowDurationMins")
                    .and_then(Value::as_u64)
                    .and_then(|minutes| minutes.checked_mul(60))
                    .map(Duration::from_secs),
                resets_at: window.get("resetsAt").and_then(Value::as_i64),
            };
            Some(bucket)
        })
        .collect();
    if let Some(cap) = snapshot
        .get("individualLimit")
        .filter(|cap| cap.is_object())
    {
        let mut bucket = Bucket::new(format!("{id}:spend_control"));
        bucket.label.clone_from(&label);
        bucket.models = models;
        bucket.unit = Unit::Permille;
        let remaining = cap.get("remainingPercent").and_then(Value::as_f64);
        if let Some(remaining) = remaining {
            bucket.used = Some(1000 - u64::from(permille(remaining)));
            bucket.limit = Some(1000);
        }
        let reached = snapshot.get("spendControlReached").and_then(Value::as_bool) == Some(true)
            || remaining.is_some_and(|percent| percent <= 0.0);
        bucket.status = match remaining {
            _ if reached || blocked => Status::Exhausted,
            Some(_) => Status::Ok,
            None => Status::Unknown,
        };
        bucket.refill = Refill::FixedWindow {
            length: None,
            resets_at: cap.get("resetsAt").and_then(Value::as_i64),
        };
        buckets.push(bucket);
    }
    buckets
}

/// The error object of an `error` or `turn/failed` notification, or of a failed `turn/completed`.
pub(crate) fn error_of(message: &Value) -> Option<&Value> {
    let params = message.get("params")?;
    let error = match message.get("method")?.as_str()? {
        "error" => params.get("error"),
        "turn/failed" => params
            .get("error")
            .or_else(|| params.pointer("/turn/error")),
        "turn/completed"
            if params.pointer("/turn/status").and_then(Value::as_str) == Some("failed") =>
        {
            params.pointer("/turn/error")
        }
        _ => None,
    };
    error.filter(|error| !error.is_null())
}

/// A usage limit in an error: `codexErrorInfo` naming a usage or rate limit, or a connection that failed with HTTP
/// 429. The reset is the error's `resetsAt`, else the exhausted window's `hint`.
pub(crate) fn limit(error: &Value, hint: Option<&LimitHit>) -> Option<LimitHit> {
    let info = error.get("codexErrorInfo")?;
    let named = info
        .as_str()
        .map(|text| text.to_lowercase().replace('_', ""))
        .is_some_and(|text| text.contains("usagelimit") || text.contains("ratelimit"));
    let throttled = info
        .as_object()
        .and_then(|object| object.values().next())
        .and_then(|detail| detail.get("httpStatusCode"))
        .and_then(Value::as_u64)
        == Some(429);
    if !named && !throttled {
        return None;
    }
    let mut limit = LimitHit::new(0);
    limit.resets_at = error
        .get("resetsAt")
        .and_then(Value::as_i64)
        .or_else(|| hint.and_then(|hint| hint.resets_at));
    limit.bucket = hint.and_then(|hint| hint.bucket.clone());
    limit.text = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(limit)
}

/// The tokens one model request used, from `thread/tokenUsage/updated`: its `last`, since the thread's total
/// survives a resume. `inputTokens` include the cached ones, which are split out. Codex reports no cost. The context
/// in use is that request's input and output, of `modelContextWindow`.
pub(crate) fn usage(message: &Value) -> Option<Usage> {
    if message.get("method")?.as_str()? != "thread/tokenUsage/updated" {
        return None;
    }
    let usage = message.pointer("/params/tokenUsage")?;
    let last = usage.get("last")?;
    let count = |key: &str| last.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = count("cachedInputTokens");
    let mut reported = Usage::default();
    reported.input = count("inputTokens").saturating_sub(cached);
    reported.output = count("outputTokens");
    reported.cache_read = cached;
    reported.cache_write = count("cacheWriteInputTokens");
    reported.context_used = Some(count("inputTokens").saturating_add(count("outputTokens")));
    reported.context_size = usage.get("modelContextWindow").and_then(Value::as_u64);
    reported.spend = usage
        .get("total")
        .filter(|total| total.is_object())
        .map(spent);
    Some(reported)
}

/// What the thread has spent, from the `total` of its token usage: running totals for the thread, which a resumed
/// thread carries over from its earlier turns. Codex reports no cost, so none is given. Reasoning tokens are part of
/// the output ones. The session's id is the connection's to fill.
fn spent(total: &Value) -> Spend {
    let count = |key: &str| total.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = count("cachedInputTokens");
    let mut spend = Spend::new("");
    spend.input = count("inputTokens").saturating_sub(cached);
    spend.output = count("outputTokens");
    spend.cache_read = cached;
    spend.cache_write = count("cacheWriteInputTokens");
    spend.reasoning = total.get("reasoningOutputTokens").and_then(Value::as_u64);
    spend
}
