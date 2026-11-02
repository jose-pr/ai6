//! What another Operation may read of a Mission, as the Registry holds it: its status, with no
//! text, and its product: the filed outcome, the CX report and the refs.

use serde_json::{Value, json};

use super::backend::RegistryBackend;
use super::mission_issues::OutboxKind;
use super::sessions::{SessionRecord, SessionStatus};
use super::store::{FiledBrief, Lease};
use crate::bounds::Limit;

/// Marks a text cut at a compartment's `max_product_bytes`.
pub const CUT_MARKER: &str = "… [cut]";

/// The keys of a filed outcome another Operation may read. An outcome also holds Session IDs and
/// where the work ran, which are not part of a product.
const OUTCOME_KEYS: [&str; 4] = ["state", "reason", "branch", "summary"];

/// What any holder of `mission.status` sees of `filed`: its ID, Operation, state, role, Agent
/// codename, attempts, creation time and why it is blocked. Never a brief, a Session ID, a
/// Station, a Theater, a safe house, a launch manifest, usage or cost.
pub fn mission_status(registry: &dyn RegistryBackend, filed: &FiledBrief) -> Value {
    let key = filed.brief.key();
    let lease = (filed.state == "claimed")
        .then(|| registry.lease(&key).ok().flatten())
        .flatten();
    let held = |name: &str| {
        lease
            .as_ref()
            .and_then(|lease| lease.meta().get(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let agent = held("agent")
        .or_else(|| {
            filed
                .outcome
                .as_ref()
                .and_then(|outcome| outcome.get("agent"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| filed.brief.agent.clone());
    let attempts = lease.as_ref().map_or(filed.attempts, Lease::attempt);
    let mut answer = json!({
        "mission": key,
        "operation": filed.brief.operation,
        "state": filed.state,
        "role": held("role").or_else(|| filed.brief.role.clone()),
        "agent": agent,
        "attempts": attempts,
        "created": filed.brief.created,
    });
    if let Some(reason) = blocked_reason(registry, filed) {
        answer["blocked"] = json!(reason);
    }
    answer
}

/// The latest Session of Mission `key`.
fn latest_session(registry: &dyn RegistryBackend, key: &str) -> Option<SessionRecord> {
    registry
        .sessions()
        .ok()?
        .into_iter()
        .filter(|session| session.mission_id == key)
        .max_by_key(|session| session.attempt)
}

/// Why a Mission is not moving, as a word: held by vetting, or its Session parked or lost.
fn blocked_reason(registry: &dyn RegistryBackend, filed: &FiledBrief) -> Option<&'static str> {
    if filed.state == "held" {
        return Some("held");
    }
    if filed.state != "claimed" {
        return None;
    }
    match latest_session(registry, &filed.brief.key())?.status {
        SessionStatus::Parked => Some("parked"),
        SessionStatus::Lost => Some("lost"),
        _ => None,
    }
}

/// `filed`'s status and what it produced, each text within `limit` bytes: `outcome_untrusted`,
/// `cx_report_untrusted` and `refs` (its branch, and the branch and commit of its last
/// checkpoint). A field the Registry does not hold is left out.
pub fn mission_product(registry: &dyn RegistryBackend, filed: &FiledBrief, limit: Limit) -> Value {
    let key = filed.brief.key();
    let mut answer = mission_status(registry, filed);
    if let Some(outcome) = filed.outcome.as_ref().and_then(Value::as_object) {
        let kept: serde_json::Map<String, Value> = outcome
            .iter()
            .filter(|(key, _)| OUTCOME_KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), cut_value(value, limit)))
            .collect();
        answer["outcome_untrusted"] = Value::Object(kept);
    }
    let report = registry
        .outbox_entries(Some(&key))
        .unwrap_or_default()
        .into_iter()
        .rfind(|entry| entry.kind == OutboxKind::Report);
    if let Some(report) = report {
        answer["cx_report_untrusted"] = json!(cut(&report.body, limit));
    }
    let mut refs = serde_json::Map::new();
    if let Some(branch) = filed
        .outcome
        .as_ref()
        .and_then(|outcome| outcome.get("branch"))
        .and_then(Value::as_str)
    {
        refs.insert("branch".to_owned(), json!(branch));
    }
    if let Some(checkpoint) = latest_session(registry, &key).and_then(|session| session.checkpoint)
    {
        refs.insert("branch".to_owned(), json!(checkpoint.branch));
        refs.insert("commit".to_owned(), json!(checkpoint.commit));
    }
    if !refs.is_empty() {
        answer["refs"] = Value::Object(refs);
    }
    answer
}

/// `text` within `limit` bytes, marked when cut.
fn cut(text: &str, limit: Limit) -> String {
    if limit.allows(text.len()) {
        return text.to_owned();
    }
    let mut end = limit.or_max();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{CUT_MARKER}", &text[..end])
}

/// `value` with each string in it within `limit` bytes.
fn cut_value(value: &Value, limit: Limit) -> Value {
    match value {
        Value::String(text) => Value::String(cut(text, limit)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|item| cut_value(item, limit)).collect())
        }
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, item)| (key.clone(), cut_value(item, limit)))
                .collect(),
        ),
        other => other.clone(),
    }
}
