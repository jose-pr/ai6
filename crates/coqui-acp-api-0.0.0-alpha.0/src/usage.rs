//! How much of each usage allowance an account has used, how it refills and when: one model for a
//! harness's or provider's answer, the ACP extension that carries it, and the `acp_harness` kind's
//! `usage` method.
//!
//! The model is plain data and the same in every language: counts and money are integers, times are
//! Unix seconds, lengths are Guatiao's `Duration` (`[seconds, nanoseconds]`). No float appears. It
//! crosses the kind as a Guatiao value and the ACP extension as JSON; both spell it the same, and
//! `vectors/usage.json` holds every reader to that spelling.
//!
//! Warning thresholds are the host's: a provider reports [`Status::Warning`] only when the harness
//! said so, and never invents a level of its own.

use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;

use guatiao::{FromValue, Schema, Secret, ToValue};
use serde::{Deserialize, Serialize};

use crate::account::AccountGrant;
use crate::environment::Environment;

/// The key of `agentCapabilities._meta` (`capabilities._meta` in ACP version 2) in which an agent
/// that reports usage says so: `{"version": 1, "sources": ["reported", "inferred"], "spend": true}`,
/// with `"needs_turn": true` added by an agent that learns its usage only during a turn, and
/// `"needs_session": true` by one that learns it once a session is open, with no turn.
/// Absent means the agent reports none, and a client does not call [`USAGE_METHOD`].
pub const USAGE_CAPABILITY: &str = "dev.boriken.coqui.acp/usage";

/// The request a client sends to read an agent's [`UsageReport`]: `{"sessionId"?: uuid}`. It needs
/// no session and no turn, and is cheap, so a host may poll it as often as it likes. An agent that
/// does not know it answers `-32601`.
pub const USAGE_METHOD: &str = "_coqui_acp/usage";

/// The notification an agent sends, only to a client that asked for it, when its report changed:
/// `{"sessionId"?: uuid, "report": UsageReport}`, the whole report every time.
pub const USAGE_NOTIFICATION: &str = "_coqui_acp/usage_update";

/// The `_meta` key of a turn's answer that says a quota refused the turn. Its value is a
/// [`LimitHit`].
pub const LIMIT_META_KEY: &str = "dev.boriken.coqui.acp/limit";

/// The version of the extension. It is independent of ACP's, and is 1 until the first release.
pub const USAGE_VERSION: u32 = 1;

/// Writes the names of an open enum: a unit enum with one `Other(String)` that keeps a name no
/// variant has. Text equal to a known name always reads as that variant.
macro_rules! open_names {
    ($ty:ident { $($variant:ident => $name:literal),+ $(,)? }) => {
        impl $ty {
            /// Its name on the wire.
            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $name,)+
                    Self::Other(text) => text,
                }
            }
        }

        impl From<String> for $ty {
            fn from(text: String) -> Self {
                match text.as_str() {
                    $($name => Self::$variant,)+
                    _ => Self::Other(text),
                }
            }
        }

        impl From<$ty> for String {
            fn from(value: $ty) -> String {
                match value {
                    $($ty::$variant => $name.into(),)+
                    $ty::Other(text) => text,
                }
            }
        }
    };
}

/// A tagged enum read leniently where it sits in a struct: a `kind` no variant has reads as the
/// type's `Other`, never as a failure, so a newer writer's value does not lose the whole report.
macro_rules! lenient {
    ($module:ident, $ty:ident, [$($known:literal),+ $(,)?]) => {
        mod $module {
            use guatiao::schema::KindBuilder;
            use guatiao::{Alloc, FromValue, Map, MapError, Schema, ToValue, Value, ValueError};

            use super::$ty;

            pub(super) fn to_value_in(value: &$ty, alloc: Alloc) -> Result<Value, ValueError> {
                value.to_value_in(alloc)
            }

            pub(super) fn from_value(value: &Value) -> Result<$ty, MapError> {
                let named = <&Map>::try_from(value)
                    .ok()
                    .and_then(|map| map.get("kind"))
                    .and_then(|kind| <&str>::try_from(kind).ok());
                match named {
                    Some(kind) if ![$($known),+].contains(&kind) => Ok($ty::Other),
                    _ => $ty::from_value(value),
                }
            }

            pub(super) fn kind(alloc: Alloc) -> KindBuilder {
                <$ty as Schema>::kind(alloc)
            }
        }
    };
}

/// Where a figure came from.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, rename_all = "snake_case")]
#[serde(from = "String", into = "String")]
#[non_exhaustive]
pub enum Source {
    /// The harness's own structured figure.
    Reported,
    /// Read from an error's words, a 429 or a refusal, by the provider or the host. The bucket's
    /// `evidence` holds the words, for logs; nothing parses it.
    Inferred,
    /// A source a newer writer named.
    #[map(other)]
    Other(String),
}

open_names!(Source { Reported => "reported", Inferred => "inferred" });

/// What a bucket counts.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, rename_all = "snake_case")]
#[serde(from = "String", into = "String")]
#[non_exhaustive]
pub enum Unit {
    /// Tokens.
    Tokens,
    /// Requests.
    Requests,
    /// Millionths of a US dollar.
    MicroUsd,
    /// A percentage only, when the provider gives no absolute number: `limit` is 1000 and `used`
    /// thousandths, so the arithmetic is the same for every unit.
    Permille,
    /// A unit a newer writer named. A reader treats the bucket as unknown, and never fails.
    #[map(other)]
    Other(String),
}

open_names!(Unit { Tokens => "tokens", Requests => "requests", MicroUsd => "micro_usd", Permille => "permille" });

/// What a bucket's account may do now.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, rename_all = "snake_case")]
#[serde(from = "String", into = "String")]
#[non_exhaustive]
pub enum Status {
    /// Room is left.
    Ok,
    /// The harness said it is nearly used. Only when the harness said so: a host applies its own
    /// thresholds to the bucket's fraction.
    Warning,
    /// Nothing is left until the bucket refills.
    Exhausted,
    /// The harness did not say.
    Unknown,
    /// A status a newer writer named.
    #[map(other)]
    Other(String),
}

open_names!(Status { Ok => "ok", Warning => "warning", Exhausted => "exhausted", Unknown => "unknown" });

/// The models a bucket covers.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, tag = "kind", rename_all = "snake_case")]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Coverage {
    /// Every model of the account.
    All,
    /// The harness did not say. A reader counts it as covering every model, so an exhausted bucket
    /// of unknown coverage stops the account for each of them.
    Unknown,
    /// These models only, by the ids or aliases `models()` lists.
    Only {
        /// The ids.
        models: Vec<String>,
    },
    /// A coverage a newer writer named; a reader treats it as unknown.
    #[serde(other)]
    Other,
}

lenient!(
    coverage_codec,
    Coverage,
    ["all", "unknown", "only", "other"]
);

/// How a bucket refills.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao, tag = "kind", rename_all = "snake_case")]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Refill {
    /// All at once when the window ends, such as a five-hour or weekly allowance.
    FixedWindow {
        /// How long the window is, when the harness says.
        #[serde(default, skip_serializing_if = "Option::is_none", with = "pair_option")]
        length: Option<Duration>,
        /// When it ends, in Unix seconds, when the harness says.
        #[serde(skip_serializing_if = "Option::is_none")]
        resets_at: Option<i64>,
    },
    /// Continuously: `amount` units each `per`, up to `max`.
    TokenBucket {
        /// The units added each `per`.
        amount: u64,
        /// The period.
        #[serde(with = "pair")]
        per: Duration,
        /// The most it holds.
        max: u64,
    },
    /// A balance that never refills.
    #[map(rename = "none")]
    #[serde(rename = "none")]
    Never,
    /// The harness did not say.
    Unknown,
    /// A refill a newer writer named; a reader treats it as unknown.
    #[serde(other)]
    Other,
}

lenient!(
    refill_codec,
    Refill,
    ["fixed_window", "token_bucket", "none", "unknown", "other"]
);

// The serde spelling of a length, `[seconds, nanoseconds]`, is Guatiao's.
mod pair {
    use core::time::Duration;

    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S: Serializer>(value: &Duration, to: S) -> Result<S::Ok, S::Error> {
        (value.as_secs(), value.subsec_nanos()).serialize(to)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Duration, D::Error> {
        let (secs, nanos) = <(u64, u32)>::deserialize(from)?;
        if nanos >= 1_000_000_000 {
            return Err(D::Error::custom("nanoseconds below 1000000000"));
        }
        Ok(Duration::new(secs, nanos))
    }
}

mod pair_option {
    use core::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Pair(#[serde(with = "super::pair")] Duration);

    #[allow(
        clippy::ref_option,
        reason = "serde's `with` passes the field by reference"
    )]
    pub(super) fn serialize<S: Serializer>(
        value: &Option<Duration>,
        to: S,
    ) -> Result<S::Ok, S::Error> {
        value.map(Pair).serialize(to)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        from: D,
    ) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<Pair>::deserialize(from)?.map(|pair| pair.0))
    }
}

/// One allowance: what it covers, how much of it is used and how it refills.
///
/// A provider fills only what it was told and never derives one figure from another; the fraction
/// a reader uses is [`UsageReport::fraction_permille`].
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct Bucket {
    /// Stable per harness and account: `five_hour`, `seven_day`, `seven_day_opus`, `codex:primary`.
    pub id: String,
    /// What a person sees, when the harness has a name apart from the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The models it covers.
    #[map(with = coverage_codec)]
    pub models: Coverage,
    /// What it counts.
    pub unit: Unit,
    /// How much is used, when the harness said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<u64>,
    /// How much the bucket holds, when the harness said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// How much is left, when the harness said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<u64>,
    /// How it refills.
    #[map(with = refill_codec)]
    pub refill: Refill,
    /// What the harness says of it.
    pub status: Status,
    /// When these figures were read, in Unix seconds.
    pub observed_at: i64,
    /// Where the figures came from.
    pub source: Source,
    /// The words an inferred bucket was read from, for logs; nothing parses them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

impl Bucket {
    /// A bucket named `id` with nothing known of it: unknown coverage, refill and status, in
    /// permille, reported.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: None,
            models: Coverage::Unknown,
            unit: Unit::Permille,
            used: None,
            limit: None,
            remaining: None,
            refill: Refill::Unknown,
            status: Status::Unknown,
            observed_at: 0,
            source: Source::Reported,
            evidence: None,
        }
    }

    /// How long until a fixed window ends, as of `now` in Unix seconds: zero once its end has
    /// passed (the bucket is then due a new report); `None` for a bucket that is not a fixed
    /// window or does not say when it ends.
    #[must_use]
    pub fn resets_in(&self, now: i64) -> Option<Duration> {
        match &self.refill {
            Refill::FixedWindow {
                resets_at: Some(at),
                ..
            } => Some(Duration::from_secs(
                u64::try_from(at.saturating_sub(now)).unwrap_or(0),
            )),
            _ => None,
        }
    }
}

/// A refused request: the harness said a limit stopped it.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct LimitHit {
    /// The bucket it names, which is `exhausted` in the same report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,
    /// When the limit lifts, in Unix seconds, when the harness said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// How long to wait before trying again, when the refusal said (a 429's `retry-after`). It may
    /// move, which is why a host reads the report again rather than trusting it once.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "pair_option")]
    pub retry_after: Option<Duration>,
    /// Whether the harness said it or the words were read.
    pub source: Source,
    /// The harness's words, for people.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// When it was seen, in Unix seconds.
    pub observed_at: i64,
}

impl LimitHit {
    /// A reported limit seen at `observed_at`, with nothing else known.
    #[must_use]
    pub const fn new(observed_at: i64) -> Self {
        Self {
            bucket: None,
            resets_at: None,
            retry_after: None,
            source: Source::Reported,
            text: None,
            observed_at,
        }
    }
}

/// What one model of a session has used.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct ModelSpend {
    /// The model, as the harness names it.
    pub model: String,
    /// Input tokens not read from a cache.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Input tokens read from a cache.
    pub cache_read: u64,
    /// Input tokens written to a cache.
    pub cache_write: u64,
    /// Millionths of a US dollar, when the harness reported a cost; never `0` for unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_micro_usd: Option<u64>,
}

impl ModelSpend {
    /// Nothing spent yet on `model`.
    #[must_use]
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cost_micro_usd: None,
        }
    }
}

/// What a session has spent: running totals.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct Spend {
    /// The session, by the id ACP reports.
    pub session_id: String,
    /// Input tokens not read from a cache.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Input tokens read from a cache.
    pub cache_read: u64,
    /// Input tokens written to a cache.
    pub cache_write: u64,
    /// The part of `output` spent reasoning, when the harness separates it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
    /// Millionths of a US dollar, when the harness reported a cost; `None` when it did not or the
    /// figure is partial, never `0` for unknown. A conversion from another unit rounds to the
    /// nearest, halves up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_micro_usd: Option<u64>,
    /// The same by model, when the harness separates them.
    #[map(default)]
    #[serde(default)]
    pub by_model: Vec<ModelSpend>,
}

impl Spend {
    /// Nothing spent yet in `session_id`.
    #[must_use]
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: None,
            cost_micro_usd: None,
            by_model: Vec::new(),
        }
    }
}

/// What an account has used, as of the newest of its buckets.
///
/// `supported: false` is a real answer: this harness reports no usage. `supported` with no buckets
/// is a different one, nothing reported yet.
#[derive(ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct UsageReport {
    /// Whether the harness reports usage at all.
    pub supported: bool,
    /// Why not, when `supported` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsupported_because: Option<String>,
    /// Which kinds of figure it can give at all, so a host knows an agent never reports figures.
    #[map(default)]
    #[serde(default)]
    pub sources: Vec<Source>,
    /// The host's id for the account the connection runs on, echoed and never interpreted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Grows by one with each change on a connection (and per provider instance for the kind's
    /// answer), so a reader that polls keeps the newest and an unchanged account costs nothing.
    pub revision: u64,
    /// The newest bucket's `observed_at`, in Unix seconds; `0` with no bucket.
    pub observed_at: i64,
    /// The allowances, each by its id.
    #[map(default)]
    #[serde(default)]
    pub buckets: Vec<Bucket>,
    /// The last refusal the report keeps, until its reset passes or a later report shows room.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<LimitHit>,
    /// What the session asked about has spent; absent without a session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spend: Option<Spend>,
}

impl UsageReport {
    /// A supported harness's report with nothing reported yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            supported: true,
            unsupported_because: None,
            sources: Vec::new(),
            account: None,
            revision: 0,
            observed_at: 0,
            buckets: Vec::new(),
            limit: None,
            spend: None,
        }
    }

    /// The report of a harness that reports no usage, and why.
    #[must_use]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            supported: false,
            unsupported_because: Some(reason.into()),
            ..Self::new()
        }
    }

    /// How much of `bucket` is used, in thousandths and at most 1000: `used / limit`, else
    /// `(limit - remaining) / limit`, rounded down. `None` when the bucket gives neither pair or
    /// its limit is zero.
    #[must_use]
    pub fn fraction_permille(bucket: &Bucket) -> Option<u16> {
        let limit = bucket.limit.filter(|limit| *limit > 0)?;
        let used = bucket
            .used
            .or_else(|| bucket.remaining.map(|left| limit.saturating_sub(left)))?;
        let permille = (u128::from(used) * 1000 / u128::from(limit)).min(1000);
        u16::try_from(permille).ok()
    }
}

impl Default for UsageReport {
    fn default() -> Self {
        Self::new()
    }
}

/// What a harness can report, known before anything starts, in
/// [`HarnessInfo::usage`](crate::HarnessInfo::usage). Empty `sources` means none: the harness
/// answers [`USAGE_METHOD`] with `-32601`, advertises no capability, and its `usage` slot says
/// unsupported.
#[derive(
    ToValue, FromValue, Schema, Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq,
)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct UsageSupport {
    /// Which kinds of figure it can give.
    #[map(default)]
    #[serde(default)]
    pub sources: Vec<Source>,
    /// Whether it reports a session's [`Spend`].
    #[map(default)]
    #[serde(default)]
    pub spend: bool,
    /// Whether it learns its usage only during a turn. Then a read with no turn answers what the
    /// connection last saw, and a host keeps a timed retry instead of polling for fresh figures.
    /// `false` means [`USAGE_METHOD`] re-reads the account each time, so a host may poll it.
    #[map(default)]
    #[serde(default)]
    pub needs_turn: bool,
    /// Whether it learns its usage only once a session is open, though no turn is needed (an agent
    /// that serves its figures from the process a session starts). With none open, a read answers
    /// what the connection last saw, and nothing is started for it.
    #[map(default)]
    #[serde(default)]
    pub needs_session: bool,
}

impl UsageSupport {
    /// Reports nothing.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            sources: Vec::new(),
            spend: false,
            needs_turn: false,
            needs_session: false,
        }
    }

    /// Reports `sources`, and a session's spend when `spend`; it can be read with no turn.
    #[must_use]
    pub const fn new(sources: Vec<Source>, spend: bool) -> Self {
        Self {
            sources,
            spend,
            needs_turn: false,
            needs_session: false,
        }
    }

    /// Says whether it learns its usage only during a turn.
    #[must_use]
    pub const fn needs_turn(mut self, needs: bool) -> Self {
        self.needs_turn = needs;
        self
    }

    /// Says whether it learns its usage only once a session is open.
    #[must_use]
    pub const fn needs_session(mut self, needs: bool) -> Self {
        self.needs_session = needs;
        self
    }

    /// Whether it reports anything.
    #[must_use]
    pub fn reports(&self) -> bool {
        !self.sources.is_empty()
    }
}

/// What a host asks [`AcpHarness::usage`](crate::AcpHarness::usage): refuses a key it does not
/// declare.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct UsageQuery {
    /// Variables for what the harness runs to ask, over its configuration's. A provider shows none
    /// of them in an answer, a refusal, an error or a log.
    #[map(default)]
    pub environment: Secret<Environment>,
    /// The account to report for; its `id` is echoed into the report and never interpreted. The
    /// provider applies the rest, as `start` does.
    pub account: Option<AccountGrant>,
}

impl UsageQuery {
    /// A query adding nothing to the provider's configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The variables for what the harness runs.
    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = Secret::new(environment);
        self
    }

    /// The account the report is for.
    #[must_use]
    pub fn account(mut self, account: AccountGrant) -> Self {
        self.account = Some(account);
        self
    }
}

/// The words by which [`infer_limit`] reads a failed turn as a quota or rate refusal, when a host
/// sets none of its own. A host's list replaces these; it does not extend them.
///
/// They cover Antigravity's Google errors (`RESOURCE_EXHAUSTED`, "Resource has been exhausted",
/// "exhausted your capacity"), an HTTP 429 ("Error 429"), a LiteLLM gateway's `throttling_error`
/// and the plain words "rate limit", "usage limit" and "quota". Antigravity's 503 "No capacity
/// available" is a shortage of capacity, not a quota, and matches none of them.
pub const DEFAULT_LIMIT_PHRASES: &[&str] = &[
    "RESOURCE_EXHAUSTED",
    "Resource has been exhausted",
    "exhausted your capacity",
    "Error 429",
    "rate limit",
    "usage limit",
    "quota",
    "throttling_error",
];

/// The id of the bucket a host keeps for an agent that reports nothing.
const INFERRED_BUCKET: &str = "inferred";

/// The most of a text [`infer_limit`] reads, in bytes; the rest is ignored.
const MAX_SCAN_BYTES: usize = 64 * 1024;

/// The longest phrase [`infer_limit`] uses, in bytes; a longer one is skipped.
const MAX_PHRASE_BYTES: usize = 256;

/// The most of a phrase kept as evidence, in bytes.
const MAX_EVIDENCE_BYTES: usize = 120;

/// Reads a failed turn's text as a refusal by a limit: the earliest of `phrases` found in it,
/// compared without regard to ASCII case, makes the answer, and no match is `None`. An empty list
/// infers nothing, and an empty phrase is skipped.
///
/// The hit is [`Source::Inferred`] and names the bucket `inferred`. Its `text` is the matched
/// phrase as given, at most 120 bytes, never the turn's text. Only what the text states is read:
/// - `resets_at`: a UTC time written as `reset at: 2026-11-10T12:00:00Z` (also `resets at`,
///   `reset_at`);
/// - `retry_after`: a length written after `retry after`, `retry in`, `try again in`, `reset in`
///   or `reset after` (and their plurals), as `2h30m`, `90 seconds` or `1 hour and 5 minutes`; a
///   bare number is seconds only after `retry-after:`. A length with a fraction, no unit, or units
///   out of order is not read.
///
/// `observed_at` is 0: the caller, which has a clock, stamps it. A relative length is not turned
/// into `resets_at` for the same reason.
///
/// Never panics. Only the first 64 KiB of `text` is read, and a phrase over 256 bytes is skipped.
/// The work is linear in the text for a given list.
#[must_use]
pub fn infer_limit(text: &str, phrases: &[&str]) -> Option<LimitHit> {
    let hay = scanned(text);
    let phrase = earliest(hay, phrases)?;
    let mut hit = LimitHit::new(0);
    hit.bucket = Some(String::from(INFERRED_BUCKET));
    hit.source = Source::Inferred;
    hit.text = Some(String::from(truncated(phrase, MAX_EVIDENCE_BYTES)));
    hit.resets_at = stated_time(hay);
    hit.retry_after = stated_length(hay);
    Some(hit)
}

/// The prefix of `text` that is read, as bytes.
fn scanned(text: &str) -> &[u8] {
    let bytes = text.as_bytes();
    bytes.get(..MAX_SCAN_BYTES).unwrap_or(bytes)
}

/// `text` cut to at most `max` bytes at a character boundary.
fn truncated(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or("")
}

/// The phrase that starts earliest in `hay`; of two at one place, the first listed.
fn earliest<'a>(hay: &[u8], phrases: &[&'a str]) -> Option<&'a str> {
    (0..hay.len()).find_map(|start| {
        let rest = hay.get(start..)?;
        phrases
            .iter()
            .filter(|phrase| !phrase.is_empty() && phrase.len() <= MAX_PHRASE_BYTES)
            .find(|phrase| starts_with_ignore_case(rest, phrase.as_bytes()))
            .copied()
    })
}

fn starts_with_ignore_case(hay: &[u8], needle: &[u8]) -> bool {
    hay.get(..needle.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(needle))
}

/// The index after the first place `needle` occurs in `hay`, without regard to ASCII case.
fn find_after(hay: &[u8], needle: &str) -> Option<usize> {
    (0..hay.len()).find_map(|start| {
        starts_with_ignore_case(hay.get(start..)?, needle.as_bytes()).then(|| start + needle.len())
    })
}

fn skip(hay: &[u8], mut at: usize, skipped: impl Fn(u8) -> bool) -> usize {
    while hay.get(at).is_some_and(|byte| skipped(*byte)) {
        at += 1;
    }
    at
}

/// A run of at most nine digits at `at`, and the index after it.
fn number(hay: &[u8], at: usize) -> Option<(u64, usize)> {
    let end = skip(hay, at, |byte| byte.is_ascii_digit());
    if end == at || end - at > 9 {
        return None;
    }
    let value = hay
        .get(at..end)?
        .iter()
        .fold(0_u64, |sum, digit| sum * 10 + u64::from(digit - b'0'));
    Some((value, end))
}

/// Seconds in a unit, and its rank (larger is a larger unit), from the word after a number.
fn unit(word: &[u8]) -> Option<(u64, u8)> {
    const UNITS: [(&[&str], u64, u8); 4] = [
        (&["d", "day", "days"], 86_400, 3),
        (&["h", "hr", "hrs", "hour", "hours"], 3_600, 2),
        (&["m", "min", "mins", "minute", "minutes"], 60, 1),
        (&["s", "sec", "secs", "second", "seconds"], 1, 0),
    ];
    UNITS.iter().find_map(|(names, seconds, rank)| {
        names
            .iter()
            .any(|name| word.eq_ignore_ascii_case(name.as_bytes()))
            .then_some((*seconds, *rank))
    })
}

/// The length written at `at`: `2h30m15s`, `90 seconds`, `1 hour and 5 minutes`. With
/// `bare_seconds`, a first number with no unit is seconds.
fn length_at(hay: &[u8], at: usize, bare_seconds: bool) -> Option<Duration> {
    let mut at = skip(hay, at, |byte| matches!(byte, b' ' | b'\t' | b':' | b'='));
    let mut total = 0_u64;
    let mut last_rank = u8::MAX;
    let mut first = true;
    while let Some((value, after)) = number(hay, at) {
        if hay.get(after) == Some(&b'.') {
            return None;
        }
        let word_start = skip(hay, after, |byte| byte == b' ');
        let word_end = skip(hay, word_start, |byte| byte.is_ascii_alphabetic());
        let word = hay.get(word_start..word_end)?;
        let (seconds, rank) = match unit(word) {
            Some(found) => found,
            None if first && bare_seconds && word.is_empty() => (1, 0),
            None => return None,
        };
        if rank >= last_rank {
            return None;
        }
        last_rank = rank;
        total += value * seconds;
        first = false;
        at = skip(hay, word_end, |byte| matches!(byte, b' ' | b','));
        if starts_with_ignore_case(hay.get(at..)?, b"and ") {
            at += 4;
        }
    }
    (!first).then(|| Duration::from_secs(total))
}

/// The length the earliest retry or reset marker states.
fn stated_length(hay: &[u8]) -> Option<Duration> {
    const MARKERS: [(&str, bool); 10] = [
        ("retry-after", true),
        ("retry_after", true),
        ("retry after", false),
        ("retry in", false),
        ("retries in", false),
        ("try again in", false),
        ("resets after", false),
        ("reset after", false),
        ("resets in", false),
        ("reset in", false),
    ];
    MARKERS
        .iter()
        .filter_map(|(marker, bare)| Some((find_after(hay, marker)?, *bare)))
        .min_by_key(|(after, _)| *after)
        .and_then(|(after, bare)| length_at(hay, after, bare))
}

/// The UTC time the earliest reset-at marker states, as Unix seconds.
fn stated_time(hay: &[u8]) -> Option<i64> {
    const MARKERS: [&str; 3] = ["reset at", "resets at", "reset_at"];
    let after = MARKERS
        .iter()
        .filter_map(|marker| find_after(hay, marker))
        .min()?;
    let at = skip(hay, after, |byte| {
        matches!(byte, b' ' | b'\t' | b':' | b'=' | b'"')
    });
    utc_time_at(hay, at)
}

/// `YYYY-MM-DDTHH:MM:SS` (a space for the `T`) with an optional fraction and a `Z` or `+00:00`.
fn utc_time_at(hay: &[u8], at: usize) -> Option<i64> {
    let digits = |from: usize, count: usize| -> Option<i64> {
        let part = hay.get(from..from + count)?;
        part.iter().all(u8::is_ascii_digit).then(|| {
            part.iter()
                .fold(0_i64, |sum, digit| sum * 10 + i64::from(digit - b'0'))
        })
    };
    let at_is = |from: usize, byte: u8| hay.get(from) == Some(&byte);
    let (year, month, day) = (digits(at, 4)?, digits(at + 5, 2)?, digits(at + 8, 2)?);
    let (hour, minute, second) = (
        digits(at + 11, 2)?,
        digits(at + 14, 2)?,
        digits(at + 17, 2)?,
    );
    let shaped = at_is(at + 4, b'-')
        && at_is(at + 7, b'-')
        && at_is(at + 13, b':')
        && at_is(at + 16, b':')
        && (at_is(at + 10, b'T') || at_is(at + 10, b' '));
    let mut end = at + 19;
    if at_is(end, b'.') {
        end = skip(hay, end + 1, |byte| byte.is_ascii_digit());
    }
    let utc = at_is(end, b'Z') || hay.get(end..end + 6).is_some_and(|zone| zone == b"+00:00");
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(shaped && utc) || day < 1 || day > month_days || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    // Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted / 400;
    let year_of_era = shifted - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

impl UsageReport {
    /// What a host reports for an agent that reports nothing, once a failed turn of it was read as
    /// a refusal by a limit (see [`infer_limit`]): supported, `sources` of `inferred` only, and
    /// the one bucket `inferred`, exhausted, of unknown coverage, with no figures. Its refill is a
    /// fixed window ending at `hit.resets_at` when that is set, else unknown. The bucket's
    /// `evidence` and `observed_at` are the hit's, and the hit is the report's `limit`. The host
    /// numbers `revision` and sets `account`.
    #[must_use]
    pub fn inferred(hit: LimitHit) -> Self {
        let mut bucket = Bucket::new(INFERRED_BUCKET);
        bucket.status = Status::Exhausted;
        bucket.source = Source::Inferred;
        bucket.observed_at = hit.observed_at;
        bucket.evidence.clone_from(&hit.text);
        if hit.resets_at.is_some() {
            bucket.refill = Refill::FixedWindow {
                length: None,
                resets_at: hit.resets_at,
            };
        }
        let mut report = Self::new();
        report.sources = alloc::vec![Source::Inferred];
        report.observed_at = hit.observed_at;
        report.buckets = alloc::vec![bucket];
        report.limit = Some(hit);
        report
    }
}
