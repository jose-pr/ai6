//! The access policy record: roles, ordered rules, the chief list and the delegation cap.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{AccessPolicyError, OPERATION_ACTIONS};
use crate::bounds::Limit;

/// A role: the actions it grants where a rule binds it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleDef {
    /// The actions the role grants.
    pub actions: Vec<String>,
    /// What the role is for, for the people who edit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl RoleDef {
    /// A role granting `actions`, without a description.
    pub fn new(actions: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            actions: actions.into_iter().map(Into::into).collect(),
            description: None,
        }
    }
}

/// A rule: `subject` holds `role` at `scope`. Rules are tried in order and the first one that
/// matches decides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    /// `account:<name>`, `person:<id>` or `group:<name>`.
    pub subject: String,
    /// `*`, or `<kind>:<id>`.
    pub scope: String,
    /// The role bound.
    pub role: String,
    /// Whether the rule on an Operation reaches the Operations under it (default true).
    #[serde(default = "propagates", skip_serializing_if = "is_true")]
    pub propagate: bool,
    /// Who last wrote the rule, filled in by the gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<String>,
    /// When it was last written, in Unix seconds, filled in by the gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_unix: Option<u64>,
}

impl RuleDef {
    /// A rule binding `role` to `subject` at `scope`, reaching the Operations under it.
    pub fn new(
        subject: impl Into<String>,
        scope: impl Into<String>,
        role: impl Into<String>,
    ) -> Self {
        Self {
            subject: subject.into(),
            scope: scope.into(),
            role: role.into(),
            propagate: true,
            editor: None,
            changed_unix: None,
        }
    }
}

const fn propagates() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_true(value: &bool) -> bool {
    *value
}

/// The `[limits]` table: bounds a caller may change or lift.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsDef {
    /// How many Operations above a resource one request may name: a number, or `"none"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ancestors: Option<serde_json::Value>,
}

/// The global access policy: the record `access`.
///
/// Its file form is `[role.<name>]` tables, ordered `[[rule]]` entries, and the top-level
/// `chief` and `delegable` lists. A stored role or rule may not name `chief`, which is built
/// in.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessPolicy {
    /// The roles by name.
    #[serde(default, rename = "role", skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, RoleDef>,
    /// The rules; their order is their meaning.
    #[serde(default, rename = "rule", skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleDef>,
    /// The Operation actions an Operation's own roles may hold; `None` is the default
    /// ([`default_delegable`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegable: Option<Vec<String>>,
    /// Subjects that are chief, before every stored rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chief: Vec<String>,
    /// Bounds on evaluation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<LimitsDef>,
}

/// The actions an Operation's roles may hold unless the policy says otherwise: every Operation
/// action but those that must not be delegated by default ([`NOT_DELEGABLE`]).
pub fn default_delegable() -> Vec<String> {
    OPERATION_ACTIONS
        .iter()
        .filter(|action| !NOT_DELEGABLE.contains(action))
        .map(|action| (*action).to_owned())
        .collect()
}

/// The Operation actions the default cap withholds.
pub const NOT_DELEGABLE: [&str; 3] = ["access.manage", "approval.class", "compartment.cover"];

impl AccessPolicy {
    /// Reads a policy document.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is over `max_bytes` (`[provider] access_policy_bytes`) or
    /// is not the policy format.
    pub fn from_toml(source: &str, max_bytes: Limit) -> Result<Self, AccessPolicyError> {
        if !max_bytes.allows(source.len()) {
            return Err(AccessPolicyError(format!(
                "access policy exceeds the {max_bytes}-byte limit"
            )));
        }
        toml::from_str(source)
            .map_err(|error| AccessPolicyError(format!("invalid access policy: {error}")))
    }

    /// Reads a policy from the JSON form of its record.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not the policy format.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, AccessPolicyError> {
        Self::deserialize(value)
            .map_err(|error| AccessPolicyError(format!("invalid access policy: {error}")))
    }

    /// The JSON form of the record a configuration provider stores.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// The Operation actions an Operation's roles may hold: `delegable`, else
    /// [`default_delegable`].
    pub fn delegable_set(&self) -> BTreeSet<String> {
        match &self.delegable {
            Some(list) => list.iter().cloned().collect(),
            None => default_delegable().into_iter().collect(),
        }
    }
}

/// Reads and parses an `access.toml` policy file.
///
/// # Errors
///
/// Returns an error when the file cannot be read, is over `max_bytes` or is not the policy
/// format.
pub fn load_policy(
    path: impl AsRef<Path>,
    max_bytes: Limit,
) -> Result<AccessPolicy, AccessPolicyError> {
    let path = path.as_ref();
    let mut source = String::new();
    let most = max_bytes
        .get()
        .map_or(u64::MAX, |bytes| bytes.saturating_add(1));
    std::fs::File::open(path)
        .and_then(|file| file.take(most).read_to_string(&mut source))
        .map_err(|error| {
            AccessPolicyError(format!(
                "{}: could not read access policy: {error}",
                path.display()
            ))
        })?;
    AccessPolicy::from_toml(&source, max_bytes)
        .map_err(|error| AccessPolicyError(format!("{}: {error}", path.display())))
}

/// The policy a first start writes: the roles `controller`, `operator` and `auditor`, no
/// rules, no chief and the default cap. They are ordinary data, editable and deletable, and
/// are never written again once the record exists. A verdict on a Mission's outcome is theirs who
/// check the work, not theirs who run it: `mission.verify` is on `controller` and `auditor`.
pub fn seeded_policy() -> AccessPolicy {
    const READ: [&str; 10] = [
        "mission.read",
        "mission.status",
        "product.read",
        "session.read",
        "agent.read",
        "roster.read",
        "station.read",
        "pool.read",
        "cable.read",
        "compartment.read",
    ];
    const WATCH: [&str; 3] = ["audit.read", "access.read", "session.context"];
    const VERIFY: [&str; 1] = ["mission.verify"];
    const WORK: [&str; 9] = [
        "mission.create",
        "mission.control",
        "session.input",
        "cable.send",
        "approval.decide",
        "issue.link",
        "issue.comment",
        "issue.update",
        "issue.create",
    ];
    const ADMINISTER: [&str; 11] = [
        "memory.write",
        "mission.review",
        "compartment.manage",
        "compartment.cover",
        "roster.manage",
        "agent.configure",
        "station.configure",
        "pool.manage",
        "approval.class",
        "access.manage",
        "account.manage",
    ];
    let role = |description: &str, parts: &[&[&str]]| {
        let mut actions: Vec<String> = parts
            .iter()
            .flat_map(|part| part.iter())
            .map(|action| (*action).to_owned())
            .collect();
        actions.sort();
        RoleDef {
            actions,
            description: Some(description.to_owned()),
        }
    };
    let mut policy = AccessPolicy::default();
    policy.roles.insert(
        "controller".into(),
        role(
            "Administers: everything an operator does, plus roster, Agents, Stations, pools, green-ink classes, access and accounts",
            &[&READ, &WATCH, &VERIFY, &WORK, &ADMINISTER],
        ),
    );
    policy.roles.insert(
        "operator".into(),
        role("A regular user: reads and runs work", &[&READ, &WORK]),
    );
    policy.roles.insert(
        "auditor".into(),
        role(
            "Views everything and gives verdicts on Missions' outcomes; changes nothing else",
            &[&READ, &WATCH, &VERIFY],
        ),
    );
    policy
}
