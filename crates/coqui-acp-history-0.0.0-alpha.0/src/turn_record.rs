//! The record of one served turn: who served it (harness, agent, account, model, effort, role, profile), what it cost and how it
//! ended, with an open set of tags any host adds to. A host's ledger keeps one per turn, so usage can be summed by model, role or
//! effort later. The crate defines the shape; storing and querying records is the host's.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use core::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The tag namespace for values an ACP agent returned in `_meta`.
pub const ACP_TAGS: &str = "acp";

/// Tokens a turn used, as the agent or provider reported them. A figure not reported is `None`, never zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Tokens {
    /// Tokens read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    /// Tokens written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
    /// Input tokens served from the prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached: Option<u64>,
}

impl Tokens {
    /// Tokens with nothing reported.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            input: None,
            output: None,
            cached: None,
        }
    }
}

/// What a turn cost when the agent or provider said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Cost {
    /// Millionths of the currency's unit.
    pub micros: u64,
    /// The currency, as reported (`USD`).
    pub currency: String,
}

impl Cost {
    /// `micros` millionths of `currency`.
    #[must_use]
    pub fn new(micros: u64, currency: impl Into<String>) -> Self {
        Self {
            micros,
            currency: currency.into(),
        }
    }
}

/// How the turn ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TurnOutcome {
    /// ACP's stop reason (`end_turn`, `cancelled`, ...), or `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<String>,
    /// A limit ended or followed the turn.
    #[serde(default, skip_serializing_if = "is_false")]
    pub limit: bool,
    /// The session moved to another member because of it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub moved_on: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref, reason = "serde's signature")]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// A tag key that is not `namespace:name`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TagError;

impl fmt::Display for TagError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("a tag is `namespace:name`, both non-empty and free of control characters")
    }
}

impl core::error::Error for TagError {}

/// One served turn (or session start). Every field a host does not know stays `None`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TurnRecord {
    /// When it was recorded, in Unix seconds.
    pub at: i64,
    /// The session's id as the host knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The turn's number in the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    /// The kind of program that ran it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// The agent entry that ran it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The account it ran on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The reasoning effort.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The role the session was started for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The pool the member was chosen from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    /// The member of the pool that served it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// The profile it ran with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// How the member was chosen (`start`, `kept`, `in_turn`, `exhausted`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The member the session moved from, when it moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<String>,
    /// Whether the member's usage was reported when it was chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_known: Option<bool>,
    /// Tokens used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
    /// Cost, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    /// How long the turn ran, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// How it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<TurnOutcome>,
    /// Open-ended tags, keyed `namespace:name` by whoever set them: `acp:` for `_meta` the agent returned, `canoa:`, `ai6:`, any
    /// host's own. A reader passes over a namespace it does not know.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, Value>,
}

impl TurnRecord {
    /// A record made at `at` (Unix seconds) with nothing else known.
    #[must_use]
    pub fn new(at: i64) -> Self {
        Self {
            at,
            ..Self::default()
        }
    }

    /// Sets the tag `key`, which is `namespace:name`.
    ///
    /// # Errors
    ///
    /// `key` has no namespace or name, or holds a control character.
    pub fn tag(&mut self, key: &str, value: impl Into<Value>) -> Result<(), TagError> {
        let valid = key
            .split_once(':')
            .is_some_and(|(space, name)| !space.is_empty() && !name.is_empty())
            && !key.chars().any(char::is_control);
        if !valid {
            return Err(TagError);
        }
        self.tags.insert(key.to_string(), value.into());
        Ok(())
    }

    /// The tags of one namespace, with the namespace and colon removed.
    pub fn tags_of<'a>(&'a self, namespace: &'a str) -> impl Iterator<Item = (&'a str, &'a Value)> {
        self.tags.iter().filter_map(move |(key, value)| {
            key.split_once(':')
                .filter(|(space, _)| *space == namespace)
                .map(|(_, name)| (name, value))
        })
    }

    /// Adds each entry of an agent's `_meta` object as the tag `acp:<key>`; a key that cannot be one is passed over.
    pub fn tag_acp_meta(&mut self, meta: &Value) {
        if let Some(object) = meta.as_object() {
            for (key, value) in object {
                let _ = self.tag(&alloc::format!("{ACP_TAGS}:{key}"), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_with_tags_round_trips_and_leaves_out_what_is_unknown() {
        let mut record = TurnRecord::new(1_800_000_000);
        record.role = Some("planner".to_owned());
        record.tokens = Some(Tokens {
            input: Some(10),
            ..Tokens::new()
        });
        record.tag("ai6:run", "r1").expect("a tag");
        record.tag_acp_meta(&serde_json::json!({"cost_hint": 3, "": 1}));
        let text = serde_json::to_string(&record).expect("writes");
        assert!(!text.contains("model"), "{text}");
        assert!(
            text.contains("acp:cost_hint") && !text.contains("\"acp:\""),
            "{text}"
        );
        assert_eq!(
            serde_json::from_str::<TurnRecord>(&text).expect("reads"),
            record
        );
        assert_eq!(record.tags_of("ai6").count(), 1);
    }

    #[test]
    fn a_tag_needs_a_namespace_and_a_name() {
        let mut record = TurnRecord::new(0);
        for key in ["plain", ":x", "x:", "a:b\n"] {
            assert!(record.tag(key, 1).is_err(), "{key:?}");
        }
        assert!(record.tags.is_empty());
    }
}
