//! The authorizer that answers for every tier: the chief and the global policy, then each
//! Operation's own policy. A request is allowed when any tier allows it; within a tier the
//! first rule that matches decides.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use coqui_access::Subject;
use coqui_roles::{RoleLimits, Rule};
use guatiao::library::ProviderError;

use super::policy::AccessPolicy;
use super::{
    ACTIONS, AccessPolicyError, AuthorizationDecision, AuthorizationRequest, Authorizer, Lineage,
    OPERATION_ACTIONS, Person, RoleAuthorizer, operation::Orphan, role_authorizer,
};

/// The built-in role that holds every action.
pub const CHIEF_ROLE: &str = "chief";

/// The suffix of the compiled copy of a role bound at an Operation, which holds only the
/// role's Operation actions.
const OPERATION_COPY: &str = "@operation";

/// An action no request names: the compiled copy of a role with nothing to grant at an
/// Operation keeps its place in the rule order with this one.
const NOTHING: &str = "ai6.none";

/// Which tier answered.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierKind {
    /// The chief list.
    Chief,
    /// The global policy.
    Global,
    /// The policy of this Operation.
    Operation(String),
}

/// Why a request was allowed or not.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation {
    /// The answer.
    pub allowed: bool,
    /// The tier whose rule allowed it; `None` on a denial.
    pub tier: Option<TierKind>,
    /// The index of the rule in its tier (the chief list, the global rules or the Operation's
    /// rules); `None` on a denial.
    pub rule_index: Option<usize>,
    /// The role the rule binds; `None` on a denial.
    pub role: Option<String>,
}

impl Explanation {
    const DENIED: Self = Self {
        allowed: false,
        tier: None,
        rule_index: None,
        role: None,
    };
}

/// One rule of a tier as the tier is built.
pub(super) struct TierRule {
    pub subject: String,
    pub scope: String,
    /// The compiled role name the tier's table holds.
    pub role: String,
    /// The role as its author named it.
    pub shown: String,
    pub propagate: bool,
    pub chief: bool,
    pub index: usize,
}

#[derive(Clone)]
struct WalkRule {
    subject: Subject,
    scope: String,
    role: String,
    shown: String,
    propagate: bool,
    chief: bool,
    index: usize,
}

/// One policy compiled: the `coqui-roles` authorizer that answers, and the rule list `explain`
/// walks the same way.
#[derive(Clone)]
pub(super) struct Tier {
    kind: TierKind,
    authorizer: RoleAuthorizer,
    walk: Vec<WalkRule>,
    roles: BTreeMap<String, BTreeSet<String>>,
}

impl Tier {
    pub(super) fn build(
        kind: TierKind,
        roles: BTreeMap<String, Vec<String>>,
        rules: Vec<TierRule>,
        limits: RoleLimits,
    ) -> Result<Self, AccessPolicyError> {
        let mut builder = role_authorizer().limits(limits);
        let mut table = BTreeMap::new();
        for (name, actions) in roles {
            table.insert(name.clone(), actions.iter().cloned().collect());
            builder = builder.role(name, actions);
        }
        let mut walk = Vec::with_capacity(rules.len());
        for rule in rules {
            let subject: Subject = rule.subject.parse().map_err(|_| {
                AccessPolicyError("rule subject must be account, person or group:<value>".into())
            })?;
            builder = builder.rule(
                Rule::new(rule.subject, rule.scope.clone(), rule.role.clone())
                    .propagate(rule.propagate),
            );
            walk.push(WalkRule {
                subject,
                scope: rule.scope,
                role: rule.role,
                shown: rule.shown,
                propagate: rule.propagate,
                chief: rule.chief,
                index: rule.index,
            });
        }
        let authorizer = builder
            .build()
            .map_err(|error| AccessPolicyError(format!("invalid access policy: {error}")))?;
        Ok(Self {
            kind,
            authorizer,
            walk,
            roles: table,
        })
    }

    fn allows(&self, request: &AuthorizationRequest) -> Result<bool, ProviderError> {
        self.authorizer
            .authorize(request.clone())
            .map(|decision| decision.allowed)
    }

    /// The first rule that matches the request, and whether its role grants the action.
    fn walk(&self, request: &AuthorizationRequest) -> Option<(&WalkRule, bool)> {
        let rule = self.walk.iter().find(|rule| {
            (rule.scope == "*"
                || rule.scope == request.resource
                || (rule.propagate && request.ancestors.contains(&rule.scope)))
                && rule.subject.matches(&request.person)
        })?;
        let grants = self
            .roles
            .get(&rule.role)
            .is_some_and(|actions| actions.contains(&request.action));
        Some((rule, grants))
    }
}

/// The actions of `actions` that exist at an Operation, or the placeholder when none does.
pub(super) fn operation_copy(actions: &[String]) -> Vec<String> {
    let kept: Vec<String> = actions
        .iter()
        .filter(|action| OPERATION_ACTIONS.contains(&action.as_str()))
        .cloned()
        .collect();
    if kept.is_empty() {
        vec![NOTHING.to_owned()]
    } else {
        kept
    }
}

/// Evaluates the global tier (the chief list, then the stored rules) and the tier of each
/// Operation on a request's lineage.
#[derive(Clone)]
pub struct EffectiveAccess {
    global: Tier,
    operations: BTreeMap<String, Tier>,
    chiefs: Vec<Subject>,
    host_chiefs: Vec<String>,
    orphaned: BTreeMap<String, Vec<Orphan>>,
}

impl fmt::Debug for EffectiveAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EffectiveAccess")
            .field("rules", &self.global.walk.len())
            .field("operations", &self.operations.len())
            .finish()
    }
}

/// Reads the policy's `limits` table.
fn limits(policy: &AccessPolicy) -> Result<RoleLimits, AccessPolicyError> {
    let Some(ancestors) = policy
        .limits
        .as_ref()
        .and_then(|limits| limits.ancestors.as_ref())
    else {
        return Ok(RoleLimits::default());
    };
    let limit = match ancestors {
        serde_json::Value::String(text) if text == "none" => coqui_access::Limit::UNBOUNDED,
        serde_json::Value::Number(count) => count
            .as_u64()
            .and_then(|count| u32::try_from(count).ok())
            .map(coqui_access::Limit::at)
            .ok_or_else(|| AccessPolicyError("limits.ancestors is out of range".into()))?,
        _ => {
            return Err(AccessPolicyError(
                "limits.ancestors must be a whole number or \"none\"".into(),
            ));
        }
    };
    Ok(RoleLimits::default().ancestors(limit))
}

/// Checks a stored policy and compiles its global tier. `chief_from_host` are subjects the
/// host names as chief; they hold the chief role whatever is stored.
///
/// # Errors
///
/// Returns an error for a stored role or rule that names `chief`, a role name that holds
/// `@`, a rule naming no role, a `delegable` entry that is not an Operation action, a chief
/// subject that is not `account:`, `person:` or `group:`, or anything `coqui-roles` refuses.
pub fn compile(
    policy: &AccessPolicy,
    chief_from_host: &[String],
) -> Result<EffectiveAccess, AccessPolicyError> {
    compile_with_actions(policy, chief_from_host, ACTIONS)
}

/// [`compile`] with `actions` as the set the chief holds.
pub(super) fn compile_with_actions(
    policy: &AccessPolicy,
    chief_from_host: &[String],
    actions: &[&str],
) -> Result<EffectiveAccess, AccessPolicyError> {
    for name in policy.roles.keys() {
        if name == CHIEF_ROLE {
            return Err(AccessPolicyError("chief is built in".into()));
        }
        if name.contains('@') {
            return Err(AccessPolicyError(format!(
                "role {name:?}: a role name may not hold '@'"
            )));
        }
    }
    if let Some(delegable) = &policy.delegable
        && let Some(entry) = delegable
            .iter()
            .find(|entry| !OPERATION_ACTIONS.contains(&entry.as_str()))
    {
        return Err(AccessPolicyError(format!(
            "delegable {entry:?} is not an Operation action"
        )));
    }
    let mut roles: BTreeMap<String, Vec<String>> = policy
        .roles
        .iter()
        .map(|(name, role)| (name.clone(), role.actions.clone()))
        .collect();
    roles.insert(
        CHIEF_ROLE.to_owned(),
        actions.iter().map(|action| (*action).to_owned()).collect(),
    );
    let mut rules =
        Vec::with_capacity(policy.chief.len() + chief_from_host.len() + policy.rules.len());
    let mut chiefs = Vec::new();
    let mut seen = BTreeSet::new();
    for subject in policy.chief.iter().chain(chief_from_host) {
        let parsed: Subject = subject.parse().map_err(|_| {
            AccessPolicyError(format!(
                "chief subject {subject:?} must be account:<name>, person:<id> or group:<name>"
            ))
        })?;
        if seen.insert(subject.clone()) {
            rules.push(TierRule {
                subject: subject.clone(),
                scope: "*".into(),
                role: CHIEF_ROLE.into(),
                shown: CHIEF_ROLE.into(),
                propagate: true,
                chief: true,
                index: chiefs.len(),
            });
            chiefs.push(parsed);
        }
    }
    for (index, rule) in policy.rules.iter().enumerate() {
        if rule.role == CHIEF_ROLE {
            return Err(AccessPolicyError(
                "chief is built in: no rule binds it".into(),
            ));
        }
        let Some(role) = policy.roles.get(&rule.role) else {
            return Err(AccessPolicyError(format!(
                "rule {index} references unknown role {:?}",
                rule.role
            )));
        };
        let compiled = if rule.scope.starts_with("operation:") {
            let copy = format!("{}{OPERATION_COPY}", rule.role);
            roles
                .entry(copy.clone())
                .or_insert_with(|| operation_copy(&role.actions));
            copy
        } else {
            rule.role.clone()
        };
        rules.push(TierRule {
            subject: rule.subject.clone(),
            scope: rule.scope.clone(),
            role: compiled,
            shown: rule.role.clone(),
            propagate: rule.propagate,
            chief: false,
            index,
        });
    }
    let global = Tier::build(TierKind::Global, roles, rules, limits(policy)?)?;
    Ok(EffectiveAccess {
        global,
        operations: BTreeMap::new(),
        chiefs,
        host_chiefs: chief_from_host.to_vec(),
        orphaned: BTreeMap::new(),
    })
}

impl EffectiveAccess {
    /// Adds the compiled tiers of Operation records.
    pub(super) fn with_operations(
        mut self,
        operations: BTreeMap<String, Tier>,
        orphaned: BTreeMap<String, Vec<Orphan>>,
    ) -> Self {
        self.operations = operations;
        self.orphaned = orphaned;
        self
    }

    /// The limits the policy asks of every tier.
    pub(super) fn limits_of(policy: &AccessPolicy) -> Result<RoleLimits, AccessPolicyError> {
        limits(policy)
    }

    /// Whether `person` is a chief subject, stored or from the host.
    pub fn is_chief(&self, person: &Person) -> bool {
        self.chiefs.iter().any(|subject| subject.matches(person))
    }

    /// The chief subjects the host named, which no change through the API removes.
    pub fn host_chiefs(&self) -> &[String] {
        &self.host_chiefs
    }

    /// The records of Operations whose rules or roles were skipped, with the reasons.
    pub fn orphaned(&self) -> &BTreeMap<String, Vec<Orphan>> {
        &self.orphaned
    }

    /// Whether the policy holds a record tier for `operation`.
    pub fn has_operation_tier(&self, operation: &str) -> bool {
        self.operations.contains_key(operation)
    }

    /// The Operation tiers a request on `resource` with `ancestors` asks: its own and each
    /// ancestor's.
    fn operation_tiers<'a>(
        &'a self,
        request: &'a AuthorizationRequest,
    ) -> impl Iterator<Item = &'a Tier> {
        let own = request.resource.strip_prefix("operation:");
        own.into_iter()
            .chain(
                request
                    .ancestors
                    .iter()
                    .filter_map(|scope| scope.strip_prefix("operation:")),
            )
            .filter_map(|id| self.operations.get(id))
    }

    /// Whether `person` holds `action` at `scope` through the chief and the access tiers
    /// (compartments do not count). `lineage` names the ancestors of an Operation scope.
    pub fn holds(&self, person: &Person, action: &str, scope: &str, lineage: &Lineage) -> bool {
        if self.is_chief(person) {
            return true;
        }
        let ancestors = scope
            .strip_prefix("operation:")
            .map(|operation| {
                lineage
                    .ancestors(operation)
                    .iter()
                    .map(|ancestor| format!("operation:{ancestor}"))
                    .collect()
            })
            .unwrap_or_default();
        self.authorize(AuthorizationRequest {
            person: person.clone(),
            action: action.to_owned(),
            resource: scope.to_owned(),
            ancestors,
        })
        .is_ok_and(|decision| decision.allowed)
    }

    /// Why a request is allowed or not: the tier, the rule's index in it and the role.
    ///
    /// # Errors
    ///
    /// A request naming more ancestors than the policy's limits allow.
    pub fn explain(&self, request: &AuthorizationRequest) -> Result<Explanation, ProviderError> {
        if !self.authorize(request.clone())?.allowed {
            return Ok(Explanation::DENIED);
        }
        for tier in std::iter::once(&self.global).chain(self.operation_tiers(request)) {
            if let Some((rule, true)) = tier.walk(request) {
                return Ok(Explanation {
                    allowed: true,
                    tier: Some(if rule.chief {
                        TierKind::Chief
                    } else {
                        tier.kind.clone()
                    }),
                    rule_index: Some(rule.index),
                    role: Some(rule.shown.clone()),
                });
            }
        }
        Ok(Explanation::DENIED)
    }
}

impl Authorizer for EffectiveAccess {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        let mut allowed = self.global.allows(&request)?;
        if !allowed {
            for tier in self.operation_tiers(&request) {
                if tier.allows(&request)? {
                    allowed = true;
                    break;
                }
            }
        }
        Ok(AuthorizationDecision {
            allowed,
            ..AuthorizationDecision::default()
        })
    }
}
