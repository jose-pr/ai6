//! Operation policies: a record per Operation holding roles and rules that only add to the
//! global policy, within the global `delegable` cap and inside the Operation's own subtree.

use std::collections::{BTreeMap, BTreeSet};

use coqui_access::Subject;
use serde::{Deserialize, Serialize};

use super::effective::{EffectiveAccess, Tier, TierKind, TierRule};
use super::guard::{Editor, Grant, Refusal, reach};
use super::policy::{AccessPolicy, RoleDef, RuleDef};
use super::{AccessPolicyError, Lineage, OPERATION_ACTIONS};

/// The role names an Operation's record may not define: the built-in ones.
pub const RESERVED_ROLES: [&str; 4] = ["chief", "controller", "operator", "auditor"];

/// The record `operation_access` of one Operation: its own roles and the rules binding roles
/// (its own, an ancestor's or a global one) at that Operation or one under it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationAccess {
    /// The roles by name; each may hold only Operation actions within the global cap.
    #[serde(default, rename = "role", skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, RoleDef>,
    /// The rules; their order is their meaning.
    #[serde(default, rename = "rule", skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RuleDef>,
}

impl OperationAccess {
    /// Reads the JSON form of a record.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not the record format.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, AccessPolicyError> {
        Self::deserialize(value)
            .map_err(|error| AccessPolicyError(format!("invalid Operation access: {error}")))
    }

    /// The JSON form of the record a configuration provider stores.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// A rule or role of an Operation's record that is not applied, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    /// `rule <index>`, `role <name>` or `record`.
    pub item: String,
    /// Why it is skipped.
    pub reason: String,
}

/// Everything a record is checked against: the global policy, every Operation's record and
/// the tree.
#[derive(Debug, Clone, Copy)]
pub struct AccessSet<'a> {
    /// The global policy.
    pub global: &'a AccessPolicy,
    /// The records by Operation ID.
    pub operations: &'a BTreeMap<String, OperationAccess>,
    /// The tree of Operations.
    pub lineage: &'a Lineage,
}

/// Why a role of an Operation's record cannot be held, when it cannot.
fn role_problem(name: &str, role: &RoleDef, delegable: &BTreeSet<String>) -> Option<String> {
    if RESERVED_ROLES.contains(&name) {
        return Some(format!("{name} is a built-in role"));
    }
    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("a role name has no whitespace or control characters".into());
    }
    if role.actions.is_empty() {
        return Some("a role holds at least one action".into());
    }
    let mut seen = BTreeSet::new();
    for action in &role.actions {
        if !seen.insert(action) {
            return Some(format!("{action} is listed twice"));
        }
        if !OPERATION_ACTIONS.contains(&action.as_str()) {
            return Some(format!("{action} is not an Operation action"));
        }
        if !delegable.contains(action) {
            return Some(format!("{action} is not delegable"));
        }
    }
    None
}

/// The Operation actions the role `name` grants for a rule in the record of `operation`:
/// the nearest record that defines it (its own, then each ancestor's), else the global policy.
fn resolve_role(
    operation: &str,
    name: &str,
    set: &AccessSet<'_>,
    delegable: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    let chain = std::iter::once(operation)
        .chain(set.lineage.ancestors(operation).iter().map(String::as_str));
    for owner in chain {
        if let Some(role) = set
            .operations
            .get(owner)
            .and_then(|record| record.roles.get(name))
        {
            return match role_problem(name, role, delegable) {
                None => Ok(role.actions.clone()),
                Some(problem) => Err(format!("role {name} of {owner} is not applied: {problem}")),
            };
        }
    }
    let Some(role) = set.global.roles.get(name) else {
        return Err(format!("no role {name} is defined for this Operation"));
    };
    let held: Vec<String> = role
        .actions
        .iter()
        .filter(|action| OPERATION_ACTIONS.contains(&action.as_str()))
        .cloned()
        .collect();
    if held.is_empty() {
        return Err(format!("role {name} has no Operation actions"));
    }
    if let Some(action) = held.iter().find(|action| !delegable.contains(*action)) {
        return Err(format!(
            "role {name} holds {action}, which is not delegable"
        ));
    }
    Ok(held)
}

/// A record looked over: what is not applied and why, and the rules that are with the
/// Operation actions their role grants.
struct Examined {
    orphans: Vec<Orphan>,
    applied: Vec<(usize, Vec<String>)>,
}

/// Looks a record over, role by role and rule by rule.
fn examine(operation: &str, record: &OperationAccess, set: &AccessSet<'_>) -> Examined {
    let delegable = set.global.delegable_set();
    let mut examined = Examined {
        orphans: Vec::new(),
        applied: Vec::new(),
    };
    for (name, role) in &record.roles {
        if let Some(problem) = role_problem(name, role, &delegable) {
            examined.orphans.push(Orphan {
                item: format!("role {name}"),
                reason: problem,
            });
        }
    }
    for (index, rule) in record.rules.iter().enumerate() {
        let outcome = rule_outcome(operation, rule, set, &delegable);
        match outcome {
            Ok(actions) => examined.applied.push((index, actions)),
            Err(reason) => examined.orphans.push(Orphan {
                item: format!("rule {index}"),
                reason,
            }),
        }
    }
    examined
}

fn rule_outcome(
    operation: &str,
    rule: &RuleDef,
    set: &AccessSet<'_>,
    delegable: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    if rule.subject.parse::<Subject>().is_err() {
        return Err("a subject is account:<name>, person:<id> or group:<name>".into());
    }
    let inside = rule.scope.strip_prefix("operation:").is_some_and(|scope| {
        set.lineage.contains(scope) && set.lineage.in_subtree(scope, operation)
    });
    if !inside {
        return Err(format!(
            "scope {} is not {operation} or an Operation under it",
            rule.scope
        ));
    }
    resolve_role(operation, &rule.role, set, delegable)
}

/// Checks the record of `operation` against `set` (which holds the record, as it would be
/// stored, under `operation`).
///
/// # Errors
///
/// The first thing wrong: a role named `chief`, `controller`, `operator` or `auditor`, or
/// holding no action, an action outside the Operation actions within the global `delegable`,
/// a rule whose scope is not `operation:<operation>` or an Operation under it (never `*`,
/// `station:` or `config:`), or a rule naming a role that resolves nowhere (its own roles,
/// then each ancestor's, then the global policy's Operation actions, all within `delegable`).
pub fn validate_operation_access(
    operation: &str,
    record: &OperationAccess,
    set: &AccessSet<'_>,
) -> Result<(), Refusal> {
    if !set.lineage.contains(operation) {
        return Err(Refusal::new(format!("no Operation {operation} is known")));
    }
    match examine(operation, record, set).orphans.into_iter().next() {
        Some(orphan) => Err(Refusal::new(format!("{}: {}", orphan.item, orphan.reason))),
        None => Ok(()),
    }
}

/// The grants the records of `root` and its subtree make.
fn subtree_grants(root: &str, set: &AccessSet<'_>) -> BTreeSet<Grant> {
    let mut grants = BTreeSet::new();
    for (operation, record) in set.operations {
        if !set.lineage.in_subtree(operation, root) {
            continue;
        }
        for (index, actions) in examine(operation, record, set).applied {
            let rule = &record.rules[index];
            for action in actions {
                grants.insert((
                    rule.subject.clone(),
                    rule.scope.clone(),
                    rule.propagate,
                    action,
                ));
            }
        }
    }
    grants
}

/// Checks that `editor` may change the record of `operation` from `before` to `after`.
///
/// The editor needs `access.manage` at `operation:<operation>` through the chief and the
/// access tiers. `after` must be valid ([`validate_operation_access`]). An action added to
/// a role must be one the editor holds at the Operation, and a grant added by a rule (here or,
/// through a role of this record, in the records of Operations under it) must be one they hold
/// at the rule's scope. Removals and reorders that add nothing pass.
///
/// # Errors
///
/// The first of those the change breaks.
pub fn check_operation_change(
    operation: &str,
    before: &OperationAccess,
    after: &OperationAccess,
    editor: &Editor,
    effective: &EffectiveAccess,
    set: &AccessSet<'_>,
) -> Result<(), Refusal> {
    let held = |action: &str, scope: &str| {
        editor.chief || effective.holds(&editor.person, action, scope, set.lineage)
    };
    let here = format!("operation:{operation}");
    if !held("access.manage", &here) {
        return Err(Refusal::new(format!(
            "changing access of {operation} needs access.manage there"
        )));
    }
    let mut stored = set.operations.clone();
    stored.insert(operation.to_owned(), before.clone());
    let current = AccessSet {
        operations: &stored,
        ..*set
    };
    let mut changed = set.operations.clone();
    changed.insert(operation.to_owned(), after.clone());
    let next = AccessSet {
        operations: &changed,
        ..*set
    };
    validate_operation_access(operation, after, &next)?;
    for (name, role) in &after.roles {
        let known: BTreeSet<&str> = before
            .roles
            .get(name)
            .map(|role| role.actions.iter().map(String::as_str).collect())
            .unwrap_or_default();
        if let Some(action) = role
            .actions
            .iter()
            .find(|action| !known.contains(action.as_str()) && !held(action, &here))
        {
            return Err(Refusal::new(format!(
                "role {name} would hold {action}, which you do not hold at {here}"
            )));
        }
    }
    let was = subtree_grants(operation, &current);
    for (subject, scope, propagate, action) in subtree_grants(operation, &next).difference(&was) {
        if let Some(at) = reach(scope, *propagate, set.lineage)
            .iter()
            .find(|at| !held(action, at))
        {
            return Err(Refusal::new(format!(
                "{subject} would hold {action} at {at}, which you do not hold there"
            )));
        }
    }
    Ok(())
}

/// [`compile`](super::compile) with the Operation records: each becomes a tier a request on
/// its Operation or one under it asks. What a record holds that is no longer valid (the tree
/// moved, a global role or the cap changed) is skipped and listed in
/// [`EffectiveAccess::orphaned`], the rest of the record staying in force; a record for an
/// Operation the tree does not name is ignored and listed.
///
/// # Errors
///
/// As [`compile`](super::compile), for the global policy.
pub fn compile_with_operations(
    policy: &AccessPolicy,
    chief_from_host: &[String],
    operations: &BTreeMap<String, OperationAccess>,
    lineage: &Lineage,
) -> Result<EffectiveAccess, AccessPolicyError> {
    let access = super::compile(policy, chief_from_host)?;
    let limits = EffectiveAccess::limits_of(policy)?;
    let set = AccessSet {
        global: policy,
        operations,
        lineage,
    };
    let mut tiers = BTreeMap::new();
    let mut orphaned: BTreeMap<String, Vec<Orphan>> = BTreeMap::new();
    for (operation, record) in operations {
        if !lineage.contains(operation) {
            orphaned.entry(operation.clone()).or_default().push(Orphan {
                item: "record".into(),
                reason: format!("no Operation {operation} is known"),
            });
            continue;
        }
        let mut roles = BTreeMap::new();
        let mut rules = Vec::new();
        let examined = examine(operation, record, &set);
        if !examined.orphans.is_empty() {
            orphaned.insert(operation.clone(), examined.orphans);
        }
        for (index, actions) in examined.applied {
            let rule = &record.rules[index];
            roles.insert(rule.role.clone(), actions);
            rules.push(TierRule {
                subject: rule.subject.clone(),
                scope: rule.scope.clone(),
                role: rule.role.clone(),
                shown: rule.role.clone(),
                propagate: rule.propagate,
                chief: false,
                index,
            });
        }
        if rules.is_empty() {
            continue;
        }
        match Tier::build(TierKind::Operation(operation.clone()), roles, rules, limits) {
            Ok(tier) => {
                tiers.insert(operation.clone(), tier);
            }
            Err(error) => orphaned.entry(operation.clone()).or_default().push(Orphan {
                item: "record".into(),
                reason: error.to_string(),
            }),
        }
    }
    Ok(access.with_operations(tiers, orphaned))
}
