//! Antigravity's `GetUserStatus` answer, read into the few fields a bucket needs, and the buckets
//! those make.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::time::Duration;

use coqui_acp_api::usage::{Bucket, Coverage, Refill, Status, Unit};
use serde::Deserialize;
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{AcpModel, BUCKET_PREFIX, ReadError};

/// What the permille limit of a bucket is.
const WHOLE: u64 = 1000;

/// The answer, holding the model list and who the account is. Every other field (the account's
/// picture, its terms and limits) is skipped as it is read and never becomes a value.
#[derive(Deserialize)]
struct Answer {
    #[serde(rename = "userStatus")]
    status: Option<UserStatus>,
}

#[derive(Deserialize)]
struct UserStatus {
    #[serde(rename = "cascadeModelConfigData")]
    models: Option<ModelData>,
    email: Option<String>,
    name: Option<String>,
    #[serde(rename = "planStatus")]
    plan: Option<PlanStatus>,
    #[serde(rename = "userTier")]
    tier: Option<Tier>,
}

#[derive(Deserialize)]
struct PlanStatus {
    #[serde(rename = "planInfo")]
    info: Option<PlanInfo>,
}

#[derive(Deserialize)]
struct PlanInfo {
    #[serde(rename = "planName")]
    name: Option<String>,
}

#[derive(Deserialize)]
struct Tier {
    name: Option<String>,
}

/// Who the signed-in account is, as agy's status or its stored login says. Its `Debug` says which fields are known and
/// never their values.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AgyAccount {
    /// The account's email address.
    pub email: Option<String>,
    /// The name on the account.
    pub name: Option<String>,
    /// The plan: its tier's name (`Google AI Pro`), else the plan's own (`Pro`).
    pub plan: Option<String>,
}

impl fmt::Debug for AgyAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let known = |value: &Option<String>| if value.is_some() { "known" } else { "none" };
        f.debug_struct("AgyAccount")
            .field("email", &known(&self.email))
            .field("name", &known(&self.name))
            .field("plan", &known(&self.plan))
            .finish()
    }
}

/// The account `body` says is signed in; none for a body that is not agy's status.
pub(super) fn account(body: &[u8]) -> Option<AgyAccount> {
    let status = serde_json::from_slice::<Answer>(body).ok()?.status?;
    let text = |value: Option<String>| value.filter(|text| !text.is_empty());
    Some(AgyAccount {
        email: text(status.email),
        name: text(status.name),
        plan: text(status.tier.and_then(|tier| tier.name)).or_else(|| {
            text(
                status
                    .plan
                    .and_then(|plan| plan.info)
                    .and_then(|info| info.name),
            )
        }),
    })
}

#[derive(Deserialize)]
struct ModelData {
    #[serde(rename = "clientModelConfigs", default)]
    configs: Vec<Config>,
}

#[derive(Deserialize)]
struct Config {
    label: Option<String>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    #[serde(rename = "quotaInfo")]
    quota: Option<Quota>,
}

/// The two figures stay loose values so that one model with a figure of the wrong type is skipped
/// rather than failing the whole answer.
#[derive(Deserialize)]
struct Quota {
    #[serde(rename = "remainingFraction")]
    fraction: Option<Value>,
    #[serde(rename = "resetTime")]
    reset: Option<Value>,
}

/// One model's allowance as agy reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Model {
    /// agy's id for the model, the ACP model id when it sends one.
    pub(super) id: Option<String>,
    pub(super) label: Option<String>,
    /// What is left, in thousandths: 1000 only for a fraction of exactly 1, 0 only for exactly 0 or
    /// none given, and within 1 to 999 between.
    pub(super) remaining: u64,
    /// When the window ends, in Unix seconds.
    pub(super) resets_at: i64,
}

/// Reads `body` as agy's status.
///
/// A model without a usable allowance, a fraction that is not a number in `0..=1` or a reset that is
/// not an RFC 3339 time, or one that names itself by neither id nor label, is left out. A fraction
/// left out beside a reset is zero: the answer is protobuf JSON, which omits zero values.
pub(super) fn parse(body: &[u8]) -> Result<Vec<Model>, ReadError> {
    let answer: Answer = serde_json::from_slice(body).map_err(|_| ReadError::Malformed)?;
    let data = answer
        .status
        .and_then(|status| status.models)
        .ok_or(ReadError::Malformed)?;
    Ok(data.configs.into_iter().filter_map(model).collect())
}

/// `fetchAvailableModels`' answer: each model's details under its id. A model's `displayName` is its label.
#[derive(Deserialize)]
struct Available {
    #[serde(default)]
    models: BTreeMap<String, Details>,
}

#[derive(Deserialize)]
struct Details {
    #[serde(rename = "displayName")]
    label: Option<String>,
    #[serde(rename = "quotaInfo")]
    quota: Option<Quota>,
}

/// Reads `body` as `fetchAvailableModels`' answer, with each model's id its key, in the order of the ids; `None` for a body that
/// is not that answer. A model is left out as [`parse`] leaves it out.
pub(super) fn parse_available(body: &[u8]) -> Option<Vec<Model>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    if !value.is_object() {
        return None;
    }
    let answer = Available::deserialize(value).ok()?;
    Some(
        answer
            .models
            .into_iter()
            .filter_map(|(id, details)| {
                model(Config {
                    label: details.label,
                    model_id: Some(id),
                    quota: details.quota,
                })
            })
            .collect(),
    )
}

/// The models `read` lists as an ACP agent's, each with its id and its label for a name (the id when it has none).
pub(super) fn acp_models(read: &[Model]) -> Vec<AcpModel> {
    read.iter()
        .filter_map(|model| {
            let id = model.id.as_deref()?;
            Some(AcpModel::new(id, model.label.as_deref().unwrap_or(id)))
        })
        .collect()
}

fn model(config: Config) -> Option<Model> {
    let quota = config.quota?;
    let fraction = match quota.fraction {
        None => 0.0,
        Some(value) => value.as_f64()?,
    };
    if !(0.0..=1.0).contains(&fraction) {
        return None;
    }
    let resets_at = OffsetDateTime::parse(quota.reset?.as_str()?, &Rfc3339)
        .ok()?
        .unix_timestamp();
    let id = config.model_id.filter(|id| !id.is_empty());
    let label = config.label.filter(|label| !label.is_empty());
    if id.is_none() && label.is_none() {
        return None;
    }
    Some(Model {
        id,
        label,
        remaining: permille(fraction),
        resets_at,
    })
}

/// A fraction in `0..=1` in thousandths. The ends are kept for exactly the ends: a figure that only
/// rounds to one of them is not unused or exhausted.
fn permille(fraction: f64) -> u64 {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the fraction is within 0..=1"
    )]
    let rounded = (fraction * 1000.0).round() as u64;
    if fraction <= 0.0 {
        0
    } else if fraction >= 1.0 {
        WHOLE
    } else {
        rounded.clamp(1, WHOLE - 1)
    }
}

impl Model {
    /// What names the model in a bucket id and in what is remembered of it.
    fn key(&self) -> &str {
        self.id.as_deref().or(self.label.as_deref()).unwrap_or("")
    }
}

/// What a reader remembers between readings: how long the window of each model was when it was
/// last seen unused, so a bucket a model splits off into still has one.
#[derive(Debug, Default)]
pub(super) struct Windows(BTreeMap<String, Duration>);

/// The buckets of `models` as of `now` (Unix seconds): one for each distinct reset time, in the
/// order the first of its models appears. Models agy lists that `acp` does not (by id, then by
/// label) are counted in `unmatched`.
pub(super) fn buckets(
    models: &[Model],
    acp: &[AcpModel],
    now: i64,
    windows: &mut Windows,
    unmatched: &mut u64,
) -> Vec<Bucket> {
    let mut groups: Vec<(i64, Vec<&Model>)> = Vec::new();
    for model in models {
        match groups.iter_mut().find(|(at, _)| *at == model.resets_at) {
            Some((_, members)) => members.push(model),
            None => groups.push((model.resets_at, alloc::vec![model])),
        }
    }
    groups
        .into_iter()
        .map(|(resets_at, members)| bucket(resets_at, &members, acp, now, windows, unmatched))
        .collect()
}

fn bucket(
    resets_at: i64,
    members: &[&Model],
    acp: &[AcpModel],
    now: i64,
    windows: &mut Windows,
    unmatched: &mut u64,
) -> Bucket {
    let first = members[0];
    let mut bucket = Bucket::new(alloc::format!("{BUCKET_PREFIX}{}", first.key()));

    let mut covered: Vec<String> = Vec::new();
    let mut all_matched = true;
    for member in members {
        match find(member, acp) {
            Some(id) => {
                if !covered.iter().any(|held| held == id) {
                    covered.push(id.into());
                }
            }
            None => {
                all_matched = false;
                *unmatched += 1;
            }
        }
    }
    if all_matched {
        bucket.models = Coverage::Only { models: covered };
    }

    let remaining = members.iter().map(|m| m.remaining).min().unwrap_or(0);
    bucket.limit = Some(WHOLE);
    bucket.remaining = Some(remaining);
    bucket.status = if remaining == 0 {
        Status::Exhausted
    } else {
        Status::Ok
    };
    bucket.unit = Unit::Permille;
    bucket.observed_at = now;

    // An unused allowance has no window yet: agy reports a reset a whole window away that moves with
    // every reading, so it is a length to remember and not a time to keep.
    let length = if remaining == WHOLE {
        let length = window_length(resets_at, now);
        if let Some(length) = length {
            for member in members {
                windows.0.insert(member.key().into(), length);
            }
        }
        length
    } else {
        members
            .iter()
            .find_map(|member| windows.0.get(member.key()).copied())
    };
    bucket.refill = Refill::FixedWindow {
        length,
        resets_at: (remaining != WHOLE).then_some(resets_at),
    };
    bucket.label = length.map(name_of);
    bucket
}

/// The ACP id of the model `agy` describes: by id, else by label against the ACP model's name.
fn find<'a>(model: &Model, acp: &'a [AcpModel]) -> Option<&'a str> {
    let by_id = model
        .id
        .as_deref()
        .and_then(|id| acp.iter().find(|held| held.id == id));
    by_id
        .or_else(|| {
            let label = model.label.as_deref()?;
            acp.iter().find(|held| held.name == label)
        })
        .map(|held| held.id.as_str())
}

/// Time until `resets_at`, to the nearest minute.
fn window_length(resets_at: i64, now: i64) -> Option<Duration> {
    let seconds = u64::try_from(resets_at.checked_sub(now)?).ok()?;
    let minutes = seconds.checked_add(30)? / 60;
    (minutes > 0).then(|| Duration::from_secs(minutes * 60))
}

/// A window as a person reads it: `5 h`, `7 d`.
fn name_of(length: Duration) -> String {
    let minutes = length.as_secs() / 60;
    if minutes.checked_rem(24 * 60) == Some(0) {
        alloc::format!("{} d", minutes / (24 * 60))
    } else if minutes.checked_rem(60) == Some(0) {
        alloc::format!("{} h", minutes / 60)
    } else {
        alloc::format!("{minutes} min")
    }
}

/// `retrieveUserQuotaSummary`' answer: the groups of models, each with its own windows.
#[derive(Deserialize)]
struct Summary {
    #[serde(default)]
    groups: Vec<SummaryGroup>,
}

#[derive(Deserialize)]
struct SummaryGroup {
    #[serde(rename = "displayName")]
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    buckets: Vec<SummaryBucket>,
}

/// The fraction stays a loose value so that one window with a figure of the wrong type is skipped, not the answer.
#[derive(Deserialize)]
struct SummaryBucket {
    #[serde(rename = "bucketId")]
    id: Option<String>,
    #[serde(rename = "displayName")]
    name: Option<String>,
    window: Option<String>,
    #[serde(rename = "remainingFraction")]
    fraction: Option<Value>,
    #[serde(default)]
    disabled: bool,
    #[serde(rename = "resetTime")]
    reset: Option<String>,
}

/// What a group's description says before the models it lists.
const MODELS_LISTED: &str = "Models within this group:";

/// The models a group names. Its description lists them (`Models within this group: A, B`) when it has a list; a group whose
/// description says anything else is one model, the one it is named for.
fn group_models(group: &SummaryGroup) -> Vec<String> {
    let listed = group
        .description
        .as_deref()
        .and_then(|text| text.trim().strip_prefix(MODELS_LISTED));
    match listed {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(String::from)
            .collect(),
        None => group
            .name
            .iter()
            .filter(|name| !name.is_empty())
            .cloned()
            .collect(),
    }
}

/// A window's length from its name: `weekly`, `5h`, `2d`.
fn summary_window(name: &str) -> Option<Duration> {
    if name.eq_ignore_ascii_case("weekly") {
        return Some(Duration::from_secs(7 * 24 * 3600));
    }
    let (count, unit) = name.split_at(name.len().checked_sub(1)?);
    let count: u64 = count.parse().ok().filter(|count| *count > 0)?;
    let each = match unit {
        "h" => 3600,
        "d" => 24 * 3600,
        "m" => 60,
        _ => return None,
    };
    count.checked_mul(each).map(Duration::from_secs)
}

/// The ACP ids of the models named `names`: those whose name is the listed one or the listed one with an effort after it
/// (`Claude Opus 5.5` and `Claude Opus 5.5 (High)`). `None` unless every name has one, since a bucket that covers a model
/// it cannot name must not stop the others.
fn covered_ids(names: &[String], acp: &[AcpModel]) -> Option<Vec<String>> {
    if names.is_empty() {
        return None;
    }
    let mut ids: Vec<String> = Vec::new();
    for name in names {
        let before = ids.len();
        for held in acp {
            let effort = held
                .name
                .strip_prefix(name.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(" ("));
            if effort && !ids.contains(&held.id) {
                ids.push(held.id.clone());
            }
        }
        if ids.len() == before {
            return None;
        }
    }
    Some(ids)
}

/// The buckets of `retrieveUserQuotaSummary`' answer `body`, as of `now` (Unix seconds): each window of each group, in the
/// order the answer gives them, with the group's name and models on each. A window that does not apply is `disabled`, and
/// keeps no figure; the reset time of every other is the service's. `None` for a body that is not that answer or holds no
/// window, so the caller reads the models' own quota instead.
pub(super) fn summary_buckets(body: &[u8], acp: &[AcpModel], now: i64) -> Option<Vec<Bucket>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    if !value.is_object() {
        return None;
    }
    let summary = Summary::deserialize(value).ok()?;
    let mut made = Vec::new();
    for group in &summary.groups {
        let names = group_models(group);
        let covered = covered_ids(&names, acp);
        for window in &group.buckets {
            made.extend(summary_bucket(
                group,
                &names,
                covered.as_deref(),
                window,
                now,
            ));
        }
    }
    (!made.is_empty()).then_some(made)
}

fn summary_bucket(
    group: &SummaryGroup,
    names: &[String],
    covered: Option<&[String]>,
    window: &SummaryBucket,
    now: i64,
) -> Option<Bucket> {
    let id = window.id.as_deref().filter(|id| !id.is_empty())?;
    let mut bucket = Bucket::new(alloc::format!("{BUCKET_PREFIX}{id}"));
    bucket.label.clone_from(&window.name);
    bucket.group = group.name.clone().filter(|name| !name.is_empty());
    bucket.model_names = names.to_vec();
    if let Some(ids) = covered {
        bucket.models = Coverage::Only {
            models: ids.to_vec(),
        };
    }
    bucket.unit = Unit::Permille;
    bucket.observed_at = now;
    bucket.refill = Refill::FixedWindow {
        length: window.window.as_deref().and_then(summary_window),
        resets_at: window
            .reset
            .as_deref()
            .and_then(|text| OffsetDateTime::parse(text, &Rfc3339).ok())
            .map(OffsetDateTime::unix_timestamp),
    };
    if window.disabled {
        bucket.status = Status::Disabled;
        return Some(bucket);
    }
    // A fraction left out is zero: the answer is protobuf JSON, which omits zero values.
    let fraction = match &window.fraction {
        None => 0.0,
        Some(value) => value
            .as_f64()
            .filter(|fraction| (0.0..=1.0).contains(fraction))?,
    };
    let remaining = permille(fraction);
    bucket.limit = Some(WHOLE);
    bucket.remaining = Some(remaining);
    bucket.status = if remaining == 0 {
        Status::Exhausted
    } else {
        Status::Ok
    };
    Some(bucket)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_at(id: &str, permille: u64, resets_at: i64) -> Model {
        Model {
            id: Some(id.into()),
            label: None,
            remaining: permille,
            resets_at,
        }
    }

    #[test]
    fn the_available_models_are_read_by_their_ids() {
        let body = br#"{"models":{"b":{"displayName":"B","quotaInfo":{"resetTime":"2030-01-01T00:00:00Z"}},
            "a":{"displayName":"A","quotaInfo":{"remainingFraction":0.25,"resetTime":"2030-01-01T00:00:00Z"}},
            "c":{"displayName":"C"}}}"#;
        let models = parse_available(body).unwrap();
        let shown: Vec<_> = models
            .iter()
            .map(|m| (m.id.as_deref(), m.label.as_deref(), m.remaining))
            .collect();
        assert_eq!(
            shown,
            [(Some("a"), Some("A"), 250), (Some("b"), Some("B"), 0)]
        );
        assert_eq!(parse_available(b"{}").unwrap(), []);
        assert_eq!(parse_available(b"[]"), None);
    }

    #[test]
    fn an_accounts_debug_shows_no_value() {
        let who = AgyAccount {
            email: Some("someone@example.test".into()),
            name: None,
            plan: Some("Pro".into()),
        };
        assert_eq!(
            alloc::format!("{who:?}"),
            r#"AgyAccount { email: "known", name: "none", plan: "known" }"#
        );
    }

    #[test]
    fn a_window_is_named_as_a_person_reads_it() {
        assert_eq!(name_of(Duration::from_secs(18_000)), "5 h");
        assert_eq!(name_of(Duration::from_secs(604_800)), "7 d");
        assert_eq!(name_of(Duration::from_secs(90 * 60)), "90 min");
    }

    #[test]
    fn a_length_is_rounded_to_the_minute() {
        assert_eq!(
            window_length(18_000 - 1, 0),
            Some(Duration::from_secs(18_000))
        );
        assert_eq!(window_length(10, 0), None);
        assert_eq!(window_length(0, 10), None);
    }

    #[test]
    fn the_least_left_in_a_group_is_the_buckets() {
        let models = [model_at("a", 700, 9_000), model_at("b", 400, 9_000)];
        let mut windows = Windows::default();
        let made = buckets(&models, &[], 0, &mut windows, &mut 0);
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].remaining, Some(400));
        assert_eq!(made[0].id, "agy:a");
    }

    const SUMMARY: &str = include_str!("../../tests/fixtures/agy-1.5.15/retrieveusersummary.json");

    #[test]
    fn a_summary_makes_a_bucket_of_each_window_of_each_group() {
        let now = 1_893_000_000;
        let made = summary_buckets(SUMMARY.as_bytes(), &[], now).expect("buckets");
        let shown: Vec<_> = made
            .iter()
            .map(|bucket| {
                (
                    bucket.group.as_deref().unwrap_or(""),
                    bucket.label.as_deref().unwrap_or(""),
                    bucket.remaining,
                    bucket.status.clone(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (
                    "Gemini Models",
                    "Weekly Limit Remaining",
                    Some(420),
                    Status::Ok
                ),
                (
                    "Gemini Models",
                    "Five Hour Limit Remaining",
                    Some(1000),
                    Status::Ok
                ),
                (
                    "Claude and GPT models",
                    "Weekly Limit Remaining",
                    Some(0),
                    Status::Exhausted
                ),
                (
                    "Claude and GPT models",
                    "Five Hour Limit Remaining",
                    None,
                    Status::Disabled
                ),
                (
                    "Gemini 4 Argon",
                    "Gemini 4 Argon Weekly Limit",
                    Some(70),
                    Status::Ok
                ),
            ]
        );
        let ids: Vec<_> = made.iter().map(|bucket| bucket.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "agy:gemini-weekly",
                "agy:gemini-5h",
                "agy:3p-weekly",
                "agy:3p-5h",
                "agy:gemini-argon-wtu-weekly"
            ]
        );
        assert!(made.iter().all(|bucket| bucket.observed_at == now));
    }

    #[test]
    fn a_windows_length_and_reset_are_the_services() {
        let made = summary_buckets(SUMMARY.as_bytes(), &[], 0).expect("buckets");
        assert_eq!(
            made[0].refill,
            Refill::FixedWindow {
                length: Some(Duration::from_secs(604_800)),
                resets_at: Some(1_894_104_000),
            }
        );
        assert_eq!(
            made[1].refill,
            Refill::FixedWindow {
                length: Some(Duration::from_secs(18_000)),
                resets_at: Some(1_893_517_200),
            }
        );
        // A window that does not apply says nothing is left or used.
        assert_eq!((made[3].remaining, made[3].limit), (None, None));
        assert_eq!(made[3].status, Status::Disabled);
    }

    #[test]
    fn a_group_lists_its_models_or_is_the_one_it_is_named_for() {
        let made = summary_buckets(SUMMARY.as_bytes(), &[], 0).expect("buckets");
        assert_eq!(
            made[0].model_names,
            ["Gemini 4 Neon", "Gemini 4 Hydrogen", "Gemini 3.8 Flash"]
        );
        assert_eq!(made[0].model_names, made[1].model_names);
        assert_eq!(
            made[2].model_names,
            ["Claude Opus 5.5", "Claude Sonnet 5.5", "GPT-OSS 120B"]
        );
        assert_eq!(made[4].model_names, ["Gemini 4 Argon"]);
        // With no models of the agent's to match, a host may not stop any of them for these.
        assert!(made.iter().all(|bucket| bucket.models == Coverage::Unknown));
    }

    #[test]
    fn a_group_covers_the_agents_models_it_names_with_any_effort() {
        let acp = [
            AcpModel::new("claude-opus-5-5-high", "Claude Opus 5.5 (High)"),
            AcpModel::new("claude-opus-5-5-low", "Claude Opus 5.5 (Low)"),
            AcpModel::new("claude-sonnet-5-5", "Claude Sonnet 5.5"),
            AcpModel::new("gpt-oss-120b", "GPT-OSS 120B"),
            AcpModel::new("other", "Another"),
        ];
        let made = summary_buckets(SUMMARY.as_bytes(), &acp, 0).expect("buckets");
        assert_eq!(
            made[2].models,
            Coverage::Only {
                models: alloc::vec![
                    "claude-opus-5-5-high".into(),
                    "claude-opus-5-5-low".into(),
                    "claude-sonnet-5-5".into(),
                    "gpt-oss-120b".into()
                ]
            }
        );
        // A group with a model the agent does not list is of unknown coverage: it stops none of the others.
        assert_eq!(made[0].models, Coverage::Unknown);
    }

    #[test]
    fn a_summary_with_no_window_or_that_is_not_one_is_not_read() {
        assert!(summary_buckets(br#"{"groups": []}"#, &[], 0).is_none());
        assert!(
            summary_buckets(
                br#"{"groups": [{"displayName": "G", "buckets": []}]}"#,
                &[],
                0
            )
            .is_none()
        );
        assert!(summary_buckets(b"[]", &[], 0).is_none());
        assert!(summary_buckets(b"not json", &[], 0).is_none());
        // A figure that is not a fraction leaves that window out, not the answer.
        let one_bad = br#"{"groups": [{"displayName": "G", "buckets": [
            {"bucketId": "a", "remainingFraction": "x"}, {"bucketId": "b", "remainingFraction": 0.5}]}]}"#;
        let made = summary_buckets(one_bad, &[], 0).expect("one window");
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].id, "agy:b");
    }

    #[test]
    fn a_windows_name_is_its_length() {
        assert_eq!(summary_window("weekly"), Some(Duration::from_secs(604_800)));
        assert_eq!(summary_window("5h"), Some(Duration::from_secs(18_000)));
        assert_eq!(summary_window("2d"), Some(Duration::from_secs(172_800)));
        assert_eq!(summary_window("30m"), Some(Duration::from_secs(1_800)));
        assert_eq!(summary_window("0h"), None);
        assert_eq!(summary_window("soon"), None);
        assert_eq!(summary_window(""), None);
    }

    #[test]
    fn the_recorded_answer_says_who_is_signed_in_and_their_plan() {
        let body = include_str!("../../tests/fixtures/agy-1.5.3/getuserstatus.json");
        let who = account(body.as_bytes()).expect("an account");
        assert!(
            who.email.as_deref().is_some_and(|email| !email.is_empty()),
            "{who:?}"
        );
        assert!(
            who.plan.as_deref().is_some_and(|plan| !plan.is_empty()),
            "{who:?}"
        );
        assert_eq!(account(b"{}"), None);
        let plan_only = br#"{"userStatus":{"planStatus":{"planInfo":{"planName":"Pro"}}}}"#;
        assert_eq!(
            account(plan_only),
            Some(AgyAccount {
                email: None,
                name: None,
                plan: Some("Pro".to_owned())
            })
        );
    }

    #[test]
    fn the_recorded_answer_makes_two_unused_buckets_of_five_hours_and_seven_days() {
        let body = include_str!("../../tests/fixtures/agy-1.5.3/getuserstatus.json");
        let models = parse(body.as_bytes()).unwrap();
        assert_eq!(models.len(), 13);
        let now = models[0].resets_at - 604_800;
        let mut windows = Windows::default();
        let made = buckets(&models, &[], now, &mut windows, &mut 0);
        let shown: Vec<_> = made
            .iter()
            .map(|b| (b.id.as_str(), b.label.as_deref(), b.remaining))
            .collect();
        assert_eq!(
            shown,
            [
                ("agy:gemini-4-argon-medium", Some("7 d"), Some(1000)),
                ("agy:gemini-3.8-flash-high", Some("5 h"), Some(1000)),
            ]
        );
        assert!(made.iter().all(|b| b.models == Coverage::Unknown));
    }

    /// An answer of models `(id, quotaInfo)`; a `None` fraction leaves the key out as protobuf JSON
    /// leaves out a zero.
    fn body(models: &[(&str, Option<f64>, i64)]) -> Vec<u8> {
        let configs: Vec<_> = models
            .iter()
            .map(|(id, fraction, reset)| {
                let mut quota = serde_json::json!({"resetTime": rfc3339(*reset)});
                if let Some(fraction) = fraction {
                    quota["remainingFraction"] = serde_json::json!(fraction);
                }
                serde_json::json!({"label": id, "modelId": id, "quotaInfo": quota})
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({"userStatus": {"cascadeModelConfigData":
            {"clientModelConfigs": configs}}}))
        .unwrap()
    }

    fn rfc3339(unix: i64) -> String {
        OffsetDateTime::from_unix_timestamp(unix)
            .unwrap()
            .format(&Rfc3339)
            .unwrap()
    }

    fn read(models: &[(&str, Option<f64>, i64)], now: i64, windows: &mut Windows) -> Vec<Bucket> {
        let parsed = parse(&body(models)).unwrap();
        buckets(&parsed, &[], now, windows, &mut 0)
    }

    const WEEK: i64 = 604_800;

    #[test]
    fn a_model_that_omits_its_fraction_is_exhausted() {
        let made = read(&[("a", None, 5_000)], 0, &mut Windows::default());
        assert_eq!(made[0].remaining, Some(0));
        assert_eq!(made[0].status, Status::Exhausted);
    }

    #[test]
    fn only_a_fraction_of_exactly_one_is_unused_and_only_zero_is_exhausted() {
        let mut windows = Windows::default();
        let near_empty = read(&[("a", Some(0.0004), 5_000)], 0, &mut windows);
        assert_eq!(near_empty[0].remaining, Some(1));
        assert_eq!(near_empty[0].status, Status::Ok);
        let near_full = read(&[("a", Some(0.9996), 5_000)], 0, &mut windows);
        assert_eq!(near_full[0].remaining, Some(999));
        assert!(matches!(
            near_full[0].refill,
            Refill::FixedWindow {
                resets_at: Some(5_000),
                ..
            }
        ));
        let full = read(&[("a", Some(1.0), 5_000)], 0, &mut windows);
        assert_eq!(full[0].remaining, Some(1000));
        let empty = read(&[("a", Some(0.0), 5_000)], 0, &mut windows);
        assert_eq!(empty[0].status, Status::Exhausted);
    }

    #[test]
    fn a_model_that_splits_off_a_group_keeps_the_length_the_group_had() {
        let mut windows = Windows::default();
        let unused = [
            ("a", Some(1.0), WEEK),
            ("b", Some(1.0), WEEK),
            ("c", Some(1.0), WEEK),
        ];
        read(&unused, 0, &mut windows);
        let split = read(
            &[
                ("a", Some(1.0), WEEK + 60),
                ("b", Some(1.0), WEEK + 60),
                ("c", Some(0.5), 3_000),
            ],
            60,
            &mut windows,
        );
        let c = split.iter().find(|b| b.id == "agy:c").unwrap();
        assert_eq!(c.label.as_deref(), Some("7 d"));
        assert!(matches!(
            c.refill,
            Refill::FixedWindow {
                length: Some(length),
                resets_at: Some(3_000),
            } if length.as_secs() == 604_800
        ));
    }
}
