//! What a person may change in the global policy: only what they hold themselves, and only a
//! chief changes who is chief.

use std::collections::BTreeSet;
use std::fmt;

use super::effective::{EffectiveAccess, compile};
use super::policy::AccessPolicy;
use super::{CONFIG_ACCESS, Lineage, OPERATION_ACTIONS, Person};

/// A change the guard refused, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The reason, for the person who asked.
    pub reason: String,
}

impl Refusal {
    pub(super) fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl core::error::Error for Refusal {}

/// Who is changing a policy.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Editor {
    /// The person.
    pub person: Person,
    /// Whether the person is a chief subject, stored or from the host.
    pub chief: bool,
}

impl Editor {
    /// `person`, a chief when `effective` says so.
    pub fn new(person: Person, effective: &EffectiveAccess) -> Self {
        let chief = effective.is_chief(&person);
        Self { person, chief }
    }
}

/// What a rule grants: `(subject, scope, propagates, action)`.
pub(super) type Grant = (String, String, bool, String);

/// Where a grant at `scope` takes effect: the scope itself and, when it propagates, every
/// Operation under it. An editor must hold the action at each.
pub(super) fn reach(scope: &str, propagate: bool, lineage: &Lineage) -> Vec<String> {
    let mut at = vec![scope.to_owned()];
    if propagate && let Some(operation) = scope.strip_prefix("operation:") {
        at.extend(
            lineage
                .subtree(operation)
                .into_iter()
                .filter(|id| *id != operation)
                .map(|id| format!("operation:{id}")),
        );
    }
    at
}

/// The grants a policy.s rules make: a role bound at an Operation grants only its Operation
/// actions there.
pub(super) fn grants(policy: &AccessPolicy) -> BTreeSet<Grant> {
    let mut grants = BTreeSet::new();
    for rule in &policy.rules {
        let Some(role) = policy.roles.get(&rule.role) else {
            continue;
        };
        let operation = rule.scope.starts_with("operation:");
        for action in &role.actions {
            if !operation || OPERATION_ACTIONS.contains(&action.as_str()) {
                grants.insert((
                    rule.subject.clone(),
                    rule.scope.clone(),
                    rule.propagate,
                    action.clone(),
                ));
            }
        }
    }
    grants
}

/// Checks that `editor` may change the global policy from `before` to `after`.
///
/// The editor needs `access.manage` at `config:access`. Only a chief changes the chief list,
/// and the result keeps a chief (stored or from the host). An action added to a role, or a
/// rule added or changed, must be one the editor holds at the rule's scope (at `*` for a role
/// no rule binds), through the chief and the access tiers: compartments do not count. A
/// `delegable` entry added must be an Operation action the editor holds at `*`. Removals and
/// reorders that add nothing pass.
///
/// # Errors
///
/// The first of those the change breaks, or the error of an `after` that does not compile.
pub fn check_change(
    before: &AccessPolicy,
    after: &AccessPolicy,
    editor: &Editor,
    effective: &EffectiveAccess,
    lineage: &Lineage,
) -> Result<(), Refusal> {
    let held = |action: &str, scope: &str| {
        editor.chief || effective.holds(&editor.person, action, scope, lineage)
    };
    if !held("access.manage", CONFIG_ACCESS) {
        return Err(Refusal::new(
            "changing access needs access.manage on the global policy",
        ));
    }
    let changed =
        before.chief.iter().collect::<BTreeSet<_>>() != after.chief.iter().collect::<BTreeSet<_>>();
    if changed && !editor.chief {
        return Err(Refusal::new("only a chief changes the chief list"));
    }
    compile(after, effective.host_chiefs()).map_err(|error| Refusal::new(error.to_string()))?;
    if after.chief.is_empty() && effective.host_chiefs().is_empty() {
        return Err(Refusal::new(
            "the change would leave no chief: nobody could change access again",
        ));
    }
    let before_grants = grants(before);
    for (subject, scope, propagate, action) in grants(after).difference(&before_grants) {
        if let Some(at) = reach(scope, *propagate, lineage)
            .iter()
            .find(|at| !held(action, at))
        {
            return Err(Refusal::new(format!(
                "{subject} would hold {action} at {at}, which you do not hold there"
            )));
        }
    }
    let bound: BTreeSet<&str> = after.rules.iter().map(|rule| rule.role.as_str()).collect();
    for (name, role) in &after.roles {
        if bound.contains(name.as_str()) {
            continue;
        }
        let known: BTreeSet<&str> = before
            .roles
            .get(name)
            .map(|role| role.actions.iter().map(String::as_str).collect())
            .unwrap_or_default();
        if let Some(action) = role
            .actions
            .iter()
            .find(|action| !known.contains(action.as_str()) && !held(action, "*"))
        {
            return Err(Refusal::new(format!(
                "role {name} would hold {action}, which you do not hold at *"
            )));
        }
    }
    let was = before.delegable_set();
    for action in after.delegable_set().difference(&was) {
        if !OPERATION_ACTIONS.contains(&action.as_str()) {
            return Err(Refusal::new(format!(
                "{action} is not an Operation action, so it cannot be delegable"
            )));
        }
        if !held(action, "*") {
            return Err(Refusal::new(format!(
                "making {action} delegable needs you to hold it at *"
            )));
        }
    }
    Ok(())
}
