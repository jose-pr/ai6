//! Usage, quota and usage limits as Codex's app server reports them.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use core::time::Duration;

use coqui_acp_api::usage::{Bucket, Coverage, LimitHit, LimitKind, Refill, Spend, Status, Unit};
use coqui_acp_harness::event::{TurnTokens, Usage};
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

    /// The spend control of the limit an update named, when it is used up.
    pub(crate) fn spent(&self, message: &Value) -> Option<LimitHit> {
        if message.get("method").and_then(Value::as_str) != Some("account/rateLimits/updated") {
            return None;
        }
        let id = limit_id(message.pointer("/params/rateLimits")?);
        spent_in(id, self.snapshots.get(id)?)
    }

    /// The same over every limit the last read and the updates since hold.
    pub(crate) fn spent_any(&self) -> Option<LimitHit> {
        self.snapshots
            .iter()
            .filter_map(|(id, snapshot)| spent_in(id, snapshot))
            .max_by_key(|limit| limit.resets_at)
    }
}

/// The spend control of one snapshot, when it is used up: `spendControlReached`, or none of its limit remaining.
fn spent_in(id: &str, snapshot: &Value) -> Option<LimitHit> {
    let cap = snapshot
        .get("individualLimit")
        .filter(|cap| cap.is_object());
    let reached = snapshot.get("spendControlReached").and_then(Value::as_bool) == Some(true)
        || cap
            .and_then(|cap| cap.get("remainingPercent"))
            .and_then(Value::as_f64)
            .is_some_and(|percent| percent <= 0.0);
    reached.then(|| {
        let mut limit = LimitHit::new(0);
        limit.resets_at = cap
            .and_then(|cap| cap.get("resetsAt"))
            .and_then(Value::as_i64);
        limit.bucket = Some(format!("{id}:spend_control"));
        limit.kind = Some(LimitKind::Credit);
        limit.continuable = Some(true);
        limit
    })
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
                // A window of the plan at 100 percent.
                limit.kind = Some(LimitKind::UsageQuota);
                limit.continuable = Some(true);
                limit
            })
        })
        .max_by_key(|limit| limit.resets_at)
}

/// The buckets of one snapshot: each window as `<limit id>:<five_hour|weekly|primary|secondary>`, and the spend
/// control as `<limit id>:spend_control`. `usedPercent` is thousandths and a window is fixed, with its length when
/// Codex says it. A window at 100 percent is exhausted, and so is every bucket while the backend does not allow
/// ordinary usage. `credits.balance` is left out: its unit is not known.
///
/// What a snapshot covers: the model it names (`normalModelSlug`), else the model its `limitName` names, since an additional
/// limit is one model's own allowance; the default limit, `codex`, names none and is what every model without a limit of its own
/// draws on, so it covers all of them. A snapshot that says none of this covers what it does not say.
fn snapshot_buckets(snapshot: &Value, allowed: Option<bool>) -> Vec<Bucket> {
    let id = limit_id(snapshot);
    let label = snapshot
        .get("limitName")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let slug = snapshot
        .get("normalModelSlug")
        .and_then(Value::as_str)
        .filter(|slug| !slug.is_empty());
    let (models, model_names) = match (slug, &label) {
        (Some(slug), name) => (
            Coverage::Only {
                models: alloc::vec![slug.to_string()],
            },
            alloc::vec![name.clone().unwrap_or_else(|| slug.to_string())],
        ),
        (None, Some(name)) if id != DEFAULT_LIMIT => (
            Coverage::Only {
                models: alloc::vec![name.clone()],
            },
            alloc::vec![name.clone()],
        ),
        (None, _) if id == DEFAULT_LIMIT => (Coverage::All, Vec::new()),
        (None, _) => (Coverage::Unknown, Vec::new()),
    };
    let blocked = allowed == Some(false);
    let mut buckets: Vec<Bucket> = ["primary", "secondary"]
        .iter()
        .filter_map(|position| {
            let window = snapshot.get(position).filter(|window| window.is_object())?;
            let mut bucket = Bucket::new(format!("{id}:{}", window_name(window, position)));
            bucket.label.clone_from(&label);
            bucket.models = models.clone();
            bucket.model_names.clone_from(&model_names);
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
        bucket.model_names = model_names;
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

/// What Codex's `codexErrorInfo` says: the variant's name, the HTTP status a connection variant carries, and the name with
/// its case and underscores removed, which is how it is compared.
struct Info {
    name: String,
    status: Option<u64>,
    norm: String,
}

/// The variant of an error's `codexErrorInfo`: a string, or an object whose one key is the variant.
fn info_of(error: &Value) -> Option<Info> {
    let info = error.get("codexErrorInfo")?;
    let (name, detail) = match info {
        Value::String(name) => (name.clone(), None),
        Value::Object(object) => {
            let (name, detail) = object.iter().next()?;
            (name.clone(), Some(detail))
        }
        _ => return None,
    };
    let status = detail
        .and_then(|detail| detail.get("httpStatusCode"))
        .and_then(Value::as_u64);
    let norm = name.to_lowercase().replace('_', "");
    Some(Info { name, status, norm })
}

/// The kind of limit a variant is and whether the work can go on with another member, or `None` when the variant is no limit
/// but a failure or a refusal of the work. The names are Codex 0.166.0's and 0.172.0's; one a later Codex adds is read by what
/// it says (spend, budget, usage, rate, context window), and any other is a failure.
fn classify(info: &Info) -> Option<(LimitKind, bool)> {
    let kind = match info.norm.as_str() {
        "contextwindowexceeded" => return Some((LimitKind::ContextWindow, false)),
        "usagelimitexceeded" => LimitKind::UsageQuota,
        "ratelimitexceeded" => LimitKind::RateLimit,
        "apikeyspendlimitexceeded"
        | "projectspendlimitexceeded"
        | "organizationspendlimitexceeded" => LimitKind::Credit,
        // A budget the person set on the session is a stop, not an allowance another account makes up.
        "sessionbudgetexceeded" => return Some((LimitKind::Credit, false)),
        "serveroverloaded" | "flexunavailable" | "policycapacityunavailable" | "unauthorized" => {
            LimitKind::Unclassified
        }
        _ if info.status == Some(429) => LimitKind::RateLimit,
        name if name.contains("spendlimit") || name.contains("budget") => LimitKind::Credit,
        name if name.contains("usagelimit") => LimitKind::UsageQuota,
        name if name.contains("ratelimit") => LimitKind::RateLimit,
        name if name.contains("contextwindow") => {
            return Some((LimitKind::ContextWindow, false));
        }
        _ => return None,
    };
    Some((kind, true))
}

/// What the account has said that bears on a limit error.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Hints<'a> {
    /// The window used up, for a usage limit.
    pub window: Option<&'a LimitHit>,
    /// The spend control used up, for a spend limit.
    pub spend: Option<&'a LimitHit>,
    /// The `rateLimitReachedType` a snapshot of the turn named.
    pub reached: Option<&'a str>,
}

/// An error of a turn: the limit it is, when it is one, and the facts of it for the turn's `_meta`: `codex:error_info`
/// (the variant, whatever it is), `codex:http_status`, `codex:additional_details` and `codex:rate_limit_reached_type`.
///
/// A limit is the variants that state an allowance used up (usage, rate, context window, the spend limits and the session
/// budget), the transient ones (overloaded, flex or policy capacity unavailable) and `unauthorized`, which are `other` with the
/// variant named in the tag, and a connection variant with status 429. The reset is the error's `resetsAt`, else the window or
/// spend control the account said was used up. A `usageLimitExceeded` with a `rateLimitReachedType` of depleted credits is a
/// `credit` limit. The words of the error are the limit's text.
pub(crate) fn describe(
    error: &Value,
    hints: &Hints<'_>,
) -> (Option<LimitHit>, BTreeMap<String, Value>) {
    let mut meta = BTreeMap::new();
    let Some(info) = info_of(error) else {
        return (None, meta);
    };
    meta.insert(
        "codex:error_info".to_string(),
        Value::String(info.name.clone()),
    );
    if let Some(status) = info.status {
        meta.insert("codex:http_status".to_string(), Value::from(status));
    }
    if let Some(details) = error
        .get("additionalDetails")
        .and_then(Value::as_str)
        .filter(|details| !details.is_empty())
    {
        meta.insert(
            "codex:additional_details".to_string(),
            Value::String(details.to_string()),
        );
    }
    let Some((mut kind, continuable)) = classify(&info) else {
        return (None, meta);
    };
    if let Some(reached) = hints.reached {
        meta.insert(
            "codex:rate_limit_reached_type".to_string(),
            Value::String(reached.to_string()),
        );
        if kind == LimitKind::UsageQuota && reached.ends_with("credits_depleted") {
            kind = LimitKind::Credit;
        }
    }
    let hint = match kind {
        LimitKind::UsageQuota => hints.window,
        LimitKind::Credit => hints.spend,
        _ => None,
    };
    let mut limit = LimitHit::new(0);
    limit.continuable = Some(continuable);
    limit.resets_at = error
        .get("resetsAt")
        .and_then(Value::as_i64)
        .or_else(|| hint.and_then(|hint| hint.resets_at));
    limit.bucket = hint.and_then(|hint| hint.bucket.clone());
    limit.text = error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string);
    limit.kind = Some(kind);
    (Some(limit), meta)
}

/// The limit a turn's error states, as the provider reads it: `message` is a `error`, `turn/failed` or failed
/// `turn/completed` notification of the app server, or the error object itself (one with a `codexErrorInfo`). `kind` and
/// `continuable` follow the variant: the usage, rate and spend limits and the transient ones are continuable, a context
/// window and a session budget are not; a variant that is no limit, and an error with none, give `None`. The reset is the
/// error's `resetsAt`, else `exhausted`, the window (for a usage limit) or spend control (for a credit one) the account
/// said was used up, which is also where the hit's `bucket` comes from. The words of the error are the hit's `text`.
#[must_use]
pub fn limit_of_message(message: &Value, exhausted: Option<&LimitHit>) -> Option<LimitHit> {
    let error = error_of(message).or_else(|| message.get("codexErrorInfo").map(|_| message))?;
    let hints = Hints {
        window: exhausted,
        spend: exhausted,
        reached: None,
    };
    describe(error, &hints).0
}

/// The `rateLimitReachedType` a snapshot of an `account/rateLimits/updated` names.
pub(crate) fn reached_in_update(message: &Value) -> Option<String> {
    reached_in(message.pointer("/params/rateLimits")?)
}

/// The `rateLimitReachedType` the snapshots of an `account/rateLimits/read` answer name, the first one that names any.
pub(crate) fn reached_in_read(answer: &Value) -> Option<String> {
    answer
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|listed| listed.values())
        .chain(answer.get("rateLimits"))
        .find_map(reached_in)
}

fn reached_in(snapshot: &Value) -> Option<String> {
    snapshot
        .get("rateLimitReachedType")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The tokens one model request used, from `thread/tokenUsage/updated`: its `last`, since the thread's total
/// survives a resume. `inputTokens` include the cached ones, which are split out. Codex reports no cost. The context
/// in use is that request's input and output, of `modelContextWindow`.
///
/// With `counted`, the same report holds what the running turn has used in all its requests (`Usage::turn`): the thread's
/// `total` less what it was when the turn began.
pub(crate) fn usage(message: &Value, counted: Option<&mut Counted>) -> Option<Usage> {
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
    if let (Some(counted), Some(total)) = (
        counted,
        usage.get("total").filter(|total| total.is_object()),
    ) {
        reported.turn = Some(counted.take(Counts::of(total), Counts::of(last)));
    }
    Some(reported)
}

/// The five counts of a `TokenUsageBreakdown`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Counts {
    input: u64,
    cached: u64,
    cache_write: u64,
    output: u64,
    reasoning: u64,
}

impl Counts {
    fn of(breakdown: &Value) -> Self {
        let count = |key: &str| breakdown.get(key).and_then(Value::as_u64).unwrap_or(0);
        Self {
            input: count("inputTokens"),
            cached: count("cachedInputTokens"),
            cache_write: count("cacheWriteInputTokens"),
            output: count("outputTokens"),
            reasoning: count("reasoningOutputTokens"),
        }
    }

    fn minus(self, base: Self) -> Self {
        Self {
            input: self.input.saturating_sub(base.input),
            cached: self.cached.saturating_sub(base.cached),
            cache_write: self.cache_write.saturating_sub(base.cache_write),
            output: self.output.saturating_sub(base.output),
            reasoning: self.reasoning.saturating_sub(base.reasoning),
        }
    }
}

/// What a turn has used: the thread's total as it stood when the turn began, which is the total the last report left, or,
/// for a thread whose first report is this turn's, that report's total less its own last request.
#[derive(Debug, Default)]
pub(crate) struct Counted {
    latest: Option<Counts>,
    base: Option<Counts>,
}

impl Counted {
    /// A new turn begins: what is used from here on is its own.
    pub(crate) fn begin(&mut self) {
        self.base = None;
    }

    fn take(&mut self, total: Counts, last: Counts) -> TurnTokens {
        let latest = self.latest;
        let base = *self
            .base
            .get_or_insert_with(|| latest.unwrap_or_else(|| total.minus(last)));
        self.latest = Some(total);
        let used = total.minus(base);
        let mut tokens = TurnTokens::default();
        tokens.input = used.input.saturating_sub(used.cached);
        tokens.output = used.output;
        tokens.cache_read = used.cached;
        tokens.cache_write = used.cache_write;
        tokens.reasoning = Some(used.reasoning);
        tokens
    }
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
