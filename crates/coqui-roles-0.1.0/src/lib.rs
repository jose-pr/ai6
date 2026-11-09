//! A first-match role authorizer, built in code.
//!
//! A policy is named roles that list actions, and ordered rules that bind one
//! `account:<name>`, `person:<id>` or `group:<name>` to a role at one scope. A scope is `*`
//! (everything) or `<kind>:<id>`, where the kinds a deployment uses are configured, for
//! example `site` or `project`; the library has no scope vocabulary of its own. A rule's
//! scope matches a request when it is `*`, when it equals the request's `resource` as an
//! exact string, or when the request names it among its `ancestors` and the rule propagates
//! (the default), so a grant on a parent reaches what sits under it.
//!
//! The first rule whose subject and scope match is decisive: when its role lacks the
//! requested action, access is denied. With no matching rule access is denied. A child
//! narrows its parent's grant with a rule placed before the parent's.
//!
//! ```
//! use coqui_roles::{RoleAuthorizer, Rule};
//!
//! let authorizer = RoleAuthorizer::builder()
//!     .scope_kinds(["site"])
//!     .role("viewer", ["document.read"])
//!     .rule(Rule::new("group:staff", "site:north", "viewer"))
//!     .build()
//!     .unwrap();
//! # let _ = authorizer;
//! ```
//!
//! [`RolePolicy`] is the same thing as plain data with guatiao's value traits, so a caller
//! keeps it in any format; it is also the configuration [`RoleAuthorizer`] takes as the
//! `coqui_roles` provider, which `coqui-roles-plugin` exports.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, Provider, Schema, ToValue};
use coqui_access::{
    AuthorizationDecision, AuthorizationRequest, Authorizer, Limit, Subject, error,
};

/// Bounds on a policy, each a [`Limit`] the caller can change or lift ([`Limit::UNBOUNDED`]).
///
/// They bound the work and memory a policy costs, not who may do what: the rule scan per
/// request, and the tables built once. By default at most 256 roles, 256 actions per role,
/// 10 000 rules, 64 scope kinds, names of 256 bytes, and 64 ancestors in one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct RoleLimits {
    /// Roles a policy defines.
    #[map(default = Limit::at(256))]
    pub roles: Limit,
    /// Actions one role lists.
    #[map(default = Limit::at(256))]
    pub actions_per_role: Limit,
    /// Rules a policy holds.
    #[map(default = Limit::at(10_000))]
    pub rules: Limit,
    /// Scope kinds a policy allows.
    #[map(default = Limit::at(64))]
    pub scope_kinds: Limit,
    /// Bytes in a role, action, subject value, scope kind or scope id.
    #[map(default = Limit::at(256))]
    pub name_bytes: Limit,
    /// Ancestors one request names; a request naming more is an error, never a denial.
    #[map(default = Limit::at(64))]
    pub ancestors: Limit,
}

impl Default for RoleLimits {
    fn default() -> Self {
        Self {
            roles: Limit::at(256),
            actions_per_role: Limit::at(256),
            rules: Limit::at(10_000),
            scope_kinds: Limit::at(64),
            name_bytes: Limit::at(256),
            ancestors: Limit::at(64),
        }
    }
}

impl RoleLimits {
    /// No bound at all.
    pub const UNBOUNDED: Self = Self {
        roles: Limit::UNBOUNDED,
        actions_per_role: Limit::UNBOUNDED,
        rules: Limit::UNBOUNDED,
        scope_kinds: Limit::UNBOUNDED,
        name_bytes: Limit::UNBOUNDED,
        ancestors: Limit::UNBOUNDED,
    };

    /// These limits with `roles`.
    #[must_use]
    pub const fn roles(mut self, roles: Limit) -> Self {
        self.roles = roles;
        self
    }

    /// These limits with `actions_per_role`.
    #[must_use]
    pub const fn actions_per_role(mut self, actions: Limit) -> Self {
        self.actions_per_role = actions;
        self
    }

    /// These limits with `rules`.
    #[must_use]
    pub const fn rules(mut self, rules: Limit) -> Self {
        self.rules = rules;
        self
    }

    /// These limits with `scope_kinds`.
    #[must_use]
    pub const fn scope_kinds(mut self, kinds: Limit) -> Self {
        self.scope_kinds = kinds;
        self
    }

    /// These limits with `name_bytes`.
    #[must_use]
    pub const fn name_bytes(mut self, bytes: Limit) -> Self {
        self.name_bytes = bytes;
        self
    }

    /// These limits with `ancestors`.
    #[must_use]
    pub const fn ancestors(mut self, ancestors: Limit) -> Self {
        self.ancestors = ancestors;
        self
    }
}

/// A named set of actions.
#[derive(Debug, Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct Role {
    /// The name rules refer to.
    pub name: String,
    /// The actions it allows, such as `document.read`; none twice.
    pub actions: Vec<String>,
}

impl Role {
    /// A role allowing `actions`.
    pub fn new(
        name: impl Into<String>,
        actions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            actions: actions.into_iter().map(Into::into).collect(),
        }
    }
}

/// Binds a subject to a role at a scope.
#[derive(Debug, Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct Rule {
    /// `account:<name>`, `person:<id>` or `group:<name>`.
    pub subject: String,
    /// `*`, or `<kind>:<id>` with the kind among the policy's scope kinds.
    pub scope: String,
    /// The name of a role the policy defines.
    pub role: String,
    /// Whether it also matches a request that names `scope` among its `ancestors`; true
    /// unless configured. `*` matches every request either way.
    #[map(default = true)]
    pub propagate: bool,
}

impl Rule {
    /// A rule: `subject` holds `role` at `scope` and at whatever sits under it.
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
        }
    }

    /// This rule, reaching what sits under its scope or (`false`) its scope alone.
    #[must_use]
    pub fn propagate(mut self, propagate: bool) -> Self {
        self.propagate = propagate;
        self
    }
}

/// A role policy as plain data: what [`RoleAuthorizer::new`] checks and compiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct RolePolicy {
    /// The kinds a rule scope may use (`site` allows `site:north`); empty allows any kind.
    /// `*` is always allowed.
    #[map(default)]
    pub scope_kinds: Vec<String>,
    /// The roles.
    #[map(default)]
    pub roles: Vec<Role>,
    /// The rules, in the order they are tried.
    #[map(default)]
    pub rules: Vec<Rule>,
    /// Bounds on the policy's size; [`RoleLimits::default`] when absent.
    #[map(default)]
    pub limits: RoleLimits,
}

impl RolePolicy {
    /// An empty policy, which denies everything.
    pub fn new() -> Self {
        Self::default()
    }
}

/// Builds a [`RoleAuthorizer`]; rules are tried in the order they are added.
#[derive(Debug, Clone, Default)]
pub struct RoleAuthorizerBuilder {
    policy: RolePolicy,
}

impl RoleAuthorizerBuilder {
    /// Allows these scope kinds in rules; without any, every kind is allowed.
    #[must_use]
    pub fn scope_kinds(mut self, kinds: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.policy.scope_kinds = kinds.into_iter().map(Into::into).collect();
        self
    }

    /// Defines a role.
    #[must_use]
    pub fn role(
        mut self,
        name: impl Into<String>,
        actions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.policy.roles.push(Role::new(name, actions));
        self
    }

    /// Adds a rule after the ones already added.
    #[must_use]
    pub fn rule(mut self, rule: Rule) -> Self {
        self.policy.rules.push(rule);
        self
    }

    /// Bounds the policy's size. Default [`RoleLimits::default`].
    #[must_use]
    pub fn limits(mut self, limits: RoleLimits) -> Self {
        self.policy.limits = limits;
        self
    }

    /// Checks the policy and compiles it.
    ///
    /// # Errors
    ///
    /// As [`RoleAuthorizer::new`].
    pub fn build(self) -> Result<RoleAuthorizer, RolePolicyError> {
        RoleAuthorizer::new(self.policy)
    }
}

/// First-match role authorizer, and the `coqui_roles` provider, configured with a
/// [`RolePolicy`].
#[derive(Clone, Provider)]
#[provider(
    Authorizer,
    id = "coqui_roles",
    name = "Role authorizer",
    config = RolePolicy,
    crate = ::coqui_access::guatiao
)]
pub struct RoleAuthorizer {
    roles: BTreeMap<String, BTreeSet<String>>,
    rules: Vec<CompiledRule>,
    ancestors: Limit,
}

/// The empty policy, which denies everything, with the default limits.
impl Default for RoleAuthorizer {
    fn default() -> Self {
        Self {
            roles: BTreeMap::new(),
            rules: Vec::new(),
            ancestors: RoleLimits::default().ancestors,
        }
    }
}

#[derive(Clone)]
struct CompiledRule {
    subject: Subject,
    scope: String,
    role: String,
    propagate: bool,
}

/// An invalid role policy.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RolePolicyError(String);

impl fmt::Display for RolePolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl core::error::Error for RolePolicyError {}

/// A policy refused while building the provider is a bad configuration value.
impl From<RolePolicyError> for ProviderError {
    fn from(error: RolePolicyError) -> Self {
        error::bad_value(&error.0)
    }
}

impl TryFrom<RolePolicy> for RoleAuthorizer {
    type Error = RolePolicyError;

    fn try_from(policy: RolePolicy) -> Result<Self, RolePolicyError> {
        Self::new(policy)
    }
}

impl fmt::Debug for RoleAuthorizer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoleAuthorizer")
            .field("roles", &self.roles.len())
            .field("rules", &self.rules.len())
            .finish()
    }
}

impl RoleAuthorizer {
    /// Starts a policy in code; add roles and rules, then [`build`](RoleAuthorizerBuilder::build).
    pub fn builder() -> RoleAuthorizerBuilder {
        RoleAuthorizerBuilder::default()
    }

    /// Checks `policy` and compiles it.
    ///
    /// # Errors
    ///
    /// Returns an error for unknown roles, duplicate role names or actions, malformed
    /// subjects or scopes, a scope kind that is not allowed, or a policy past its
    /// [`RoleLimits`].
    pub fn new(policy: RolePolicy) -> Result<Self, RolePolicyError> {
        let fail = |message: &str| RolePolicyError(message.to_owned());
        let limits = policy.limits;
        let name = |value: &str| name(value, limits.name_bytes);
        let scope_kinds = &policy.scope_kinds;
        if !limits.scope_kinds.allows(scope_kinds.len())
            || !scope_kinds.iter().all(|kind| name(kind))
        {
            return Err(fail("scope kinds are invalid"));
        }
        if !limits.roles.allows(policy.roles.len()) || !limits.rules.allows(policy.rules.len()) {
            return Err(fail("policy has too many roles or rules"));
        }
        let mut roles = BTreeMap::new();
        for role in &policy.roles {
            if !name(&role.name) || role.actions.is_empty() {
                return Err(RolePolicyError(format!("invalid role {:?}", role.name)));
            }
            if !limits.actions_per_role.allows(role.actions.len()) {
                return Err(RolePolicyError(format!(
                    "role {:?} has too many actions",
                    role.name
                )));
            }
            let mut actions = BTreeSet::new();
            for action in &role.actions {
                if !name(action) || !actions.insert(action.clone()) {
                    return Err(RolePolicyError(format!(
                        "invalid or duplicate action {action:?} in role {:?}",
                        role.name
                    )));
                }
            }
            if roles.insert(role.name.clone(), actions).is_some() {
                return Err(RolePolicyError(format!(
                    "role {:?} is defined twice",
                    role.name
                )));
            }
        }

        let mut rules = Vec::with_capacity(policy.rules.len());
        for rule in policy.rules {
            let subject: Subject = rule
                .subject
                .parse()
                .map_err(|_| fail("rule subject must be account, person or group:<value>"))?;
            if !name(subject.value()) {
                return Err(fail("rule subject value is invalid"));
            }
            if rule.scope != "*" {
                let allowed = rule.scope.split_once(':').is_some_and(|(kind, id)| {
                    name(id)
                        && name(kind)
                        && (scope_kinds.is_empty() || scope_kinds.iter().any(|k| k == kind))
                });
                if !allowed {
                    return Err(RolePolicyError(format!(
                        "rule scope must be * or <kind>:<id> with kind in {scope_kinds:?}"
                    )));
                }
            }
            if !roles.contains_key(&rule.role) {
                return Err(RolePolicyError(format!(
                    "rule references unknown role {:?}",
                    rule.role
                )));
            }
            rules.push(CompiledRule {
                subject,
                scope: rule.scope,
                role: rule.role,
                propagate: rule.propagate,
            });
        }
        Ok(Self {
            roles,
            rules,
            ancestors: limits.ancestors,
        })
    }

    fn decision(
        &self,
        request: &AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        if !self.ancestors.allows(request.ancestors.len()) {
            return Err(error::bad_value(
                "the request names more ancestors than the policy's limits allow",
            ));
        }
        let allowed = self
            .rules
            .iter()
            .find(|rule| {
                (rule.scope == "*"
                    || rule.scope == request.resource
                    || (rule.propagate && request.ancestors.contains(&rule.scope)))
                    && rule.subject.matches(&request.person)
            })
            .is_some_and(|rule| {
                self.roles
                    .get(&rule.role)
                    .is_some_and(|actions| actions.contains(&request.action))
            });
        Ok(AuthorizationDecision {
            allowed,
            limits: BTreeMap::new(),
        })
    }
}

impl Authorizer for RoleAuthorizer {
    /// # Errors
    ///
    /// A request naming more `ancestors` than [`RoleLimits::ancestors`] allows.
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        self.decision(&request)
    }
}

fn name(value: &str, most: Limit) -> bool {
    !value.is_empty()
        && most.allows(value.len())
        && !value.chars().any(char::is_control)
        && !value.chars().any(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use coqui_access::Person;

    use super::*;

    fn person(id: &str, name: &str, groups: &[&str]) -> Person {
        Person {
            id: id.to_owned(),
            name: name.to_owned(),
            groups: groups.iter().map(|group| (*group).to_owned()).collect(),
            ..Person::default()
        }
    }

    fn allowed(policy: &RoleAuthorizer, person: &Person, action: &str, resource: &str) -> bool {
        under(policy, person, action, resource, &[]).unwrap()
    }

    fn under(
        policy: &RoleAuthorizer,
        person: &Person,
        action: &str,
        resource: &str,
        ancestors: &[&str],
    ) -> Result<bool, ProviderError> {
        policy
            .authorize(AuthorizationRequest {
                person: person.clone(),
                action: action.to_owned(),
                resource: resource.to_owned(),
                ancestors: ancestors.iter().map(|scope| (*scope).to_owned()).collect(),
            })
            .map(|decision| decision.allowed)
    }

    #[test]
    fn a_grant_on_an_ancestor_reaches_what_sits_under_it_unless_it_does_not_propagate() {
        let policy = RoleAuthorizer::builder()
            .scope_kinds(["site"])
            .role("viewer", ["document.read"])
            .role("none", ["x.nothing"])
            .rule(Rule::new("group:staff", "site:secret", "none"))
            .rule(Rule::new("group:staff", "site:europe", "viewer"))
            .rule(Rule::new("group:guests", "site:europe", "viewer").propagate(false))
            .build()
            .unwrap();
        let staff = person("s", "staff", &["staff"]);
        let guest = person("g", "guest", &["guests"]);
        let lineage = ["site:europe", "site:world"];
        assert!(under(&policy, &staff, "document.read", "site:north", &lineage).unwrap());
        assert!(
            !under(&policy, &staff, "document.read", "site:north", &[]).unwrap(),
            "without its ancestry a request reaches only direct rules"
        );
        assert!(!under(&policy, &guest, "document.read", "site:north", &lineage).unwrap());
        assert!(
            under(
                &policy,
                &guest,
                "document.read",
                "site:europe",
                &["site:world"]
            )
            .unwrap()
        );
        assert!(
            !under(
                &policy,
                &staff,
                "document.read",
                "site:north",
                &["site:secret", "site:europe"]
            )
            .unwrap(),
            "an earlier rule on a nearer ancestor narrows the later grant"
        );

        let deep: Vec<String> = (0..65).map(|level| format!("site:l{level}")).collect();
        let deep: Vec<&str> = deep.iter().map(String::as_str).collect();
        assert!(under(&policy, &staff, "document.read", "site:north", &deep[..64]).is_ok());
        assert!(
            under(&policy, &staff, "document.read", "site:north", &deep).is_err(),
            "65 ancestors is an error by default, never a denial"
        );
        let lifted = RoleAuthorizer::builder()
            .role("viewer", ["document.read"])
            .rule(Rule::new("group:staff", "site:l64", "viewer"))
            .limits(RoleLimits::default().ancestors(Limit::UNBOUNDED))
            .build()
            .unwrap();
        assert!(under(&lifted, &staff, "document.read", "site:north", &deep).unwrap());
    }

    fn standard() -> RoleAuthorizerBuilder {
        RoleAuthorizer::builder()
            .scope_kinds(["site"])
            .role("viewer", ["document.read", "audit.read"])
            .role("editor", ["document.read", "document.write"])
            .rule(Rule::new("group:staff", "site:north", "viewer"))
            .rule(Rule::new("person:p-editor", "site:north", "editor"))
            .rule(Rule::new("account:admin", "*", "editor"))
    }

    #[test]
    fn scopes_roles_and_default_deny() {
        let policy = standard().build().unwrap();
        let debug = format!("{policy:?}");
        assert!(!debug.contains("p-editor") && !debug.contains("staff"));
        let viewer = person("p-viewer", "viewer", &["staff"]);
        let editor = person("p-editor", "editor", &[]);
        assert!(allowed(&policy, &viewer, "document.read", "site:north"));
        assert!(!allowed(&policy, &viewer, "document.write", "site:north"));
        assert!(!allowed(&policy, &viewer, "document.read", "site:south"));
        assert!(allowed(&policy, &editor, "document.write", "site:north"));
        assert!(!allowed(&policy, &editor, "document.write", "site:south"));
        assert!(allowed(
            &policy,
            &person("p", "admin", &[]),
            "document.write",
            "site:any"
        ));
        assert!(!allowed(
            &policy,
            &person("p", "nobody", &[]),
            "document.read",
            "site:north"
        ));
        assert!(!allowed(
            &RoleAuthorizer::builder().build().unwrap(),
            &editor,
            "document.read",
            "*"
        ));
    }

    #[test]
    fn a_policy_value_builds_the_same_authorizer() {
        let mut policy = RolePolicy::new();
        policy.scope_kinds = vec!["site".to_owned()];
        policy.roles = vec![Role::new("viewer", ["document.read"])];
        policy.rules = vec![Rule::new("group:staff", "site:north", "viewer")];
        let policy = RoleAuthorizer::new(policy).unwrap();
        assert!(allowed(
            &policy,
            &person("i", "n", &["staff"]),
            "document.read",
            "site:north"
        ));
    }

    #[test]
    fn the_first_matching_rule_is_decisive() {
        let policy = RoleAuthorizer::builder()
            .role("none", ["x.nothing"])
            .role("all", ["x.read"])
            .rule(Rule::new("group:g", "*", "none"))
            .rule(Rule::new("group:g", "*", "all"))
            .build()
            .unwrap();
        assert!(!allowed(
            &policy,
            &person("i", "n", &["g"]),
            "x.read",
            "anything"
        ));
    }

    #[test]
    fn scope_kinds_are_configurable_and_everything_else_is_refused() {
        // Another vocabulary works with no change to the library.
        let policy = RoleAuthorizer::builder()
            .scope_kinds(["project"])
            .role("r", ["a.b"])
            .rule(Rule::new("account:x", "project:p1", "r"))
            .build()
            .unwrap();
        assert!(allowed(
            &policy,
            &person("i", "x", &[]),
            "a.b",
            "project:p1"
        ));
        let with_scope = |scope: &str, kinds: &[&str]| {
            RoleAuthorizer::builder()
                .scope_kinds(kinds.iter().copied())
                .role("r", ["a.b"])
                .rule(Rule::new("account:x", scope, "r"))
                .build()
        };
        assert!(with_scope("project:p1", &["site"]).is_err());
        assert!(with_scope("site", &["site"]).is_err());
        assert!(with_scope("project:p1", &[]).is_ok());
        assert!(
            RoleAuthorizer::builder()
                .rule(Rule::new("account:x", "*", "missing"))
                .build()
                .is_err()
        );
        assert!(
            RoleAuthorizer::builder()
                .role("r", ["a", "a"])
                .build()
                .is_err()
        );
        assert!(
            RoleAuthorizer::builder()
                .role("r", ["a"])
                .role("r", ["b"])
                .build()
                .is_err()
        );
        assert!(
            RoleAuthorizer::builder()
                .role("r", ["a"])
                .rule(Rule::new("nobody", "*", "r"))
                .build()
                .is_err()
        );
        assert!(
            RoleAuthorizer::builder()
                .role("r", ["a"])
                .rule(Rule::new("robot:x", "*", "r"))
                .build()
                .is_err()
        );
    }

    #[test]
    fn the_bounds_hold() {
        const ROLES: usize = 256;
        const ROLE_ACTIONS: usize = 256;
        const RULES: usize = 10_000;
        const SCOPE_KINDS: usize = 64;
        let many_rules = (0..=RULES).fold(RoleAuthorizer::builder().role("r", ["a"]), |b, n| {
            b.rule(Rule::new(format!("account:u{n}"), "*", "r"))
        });
        assert!(many_rules.build().is_err());
        let many_roles = (0..=ROLES).fold(RoleAuthorizer::builder(), |b, n| {
            b.role(format!("r{n}"), ["a"])
        });
        assert!(many_roles.build().is_err());
        let actions = (0..=ROLE_ACTIONS).map(|n| format!("a{n}"));
        assert!(
            RoleAuthorizer::builder()
                .role("r", actions)
                .build()
                .is_err()
        );
        let kinds = (0..=SCOPE_KINDS).map(|n| format!("k{n}"));
        assert!(
            RoleAuthorizer::builder()
                .scope_kinds(kinds)
                .build()
                .is_err()
        );
        // At the bounds is fine.
        let actions = (0..ROLE_ACTIONS).map(|n| format!("a{n}"));
        assert!(RoleAuthorizer::builder().role("r", actions).build().is_ok());
    }

    #[test]
    fn the_bounds_are_options_and_none_lifts_one() {
        let rules = |limits: RoleLimits| {
            (0..10_001)
                .fold(RoleAuthorizer::builder().role("r", ["a"]), |b, n| {
                    b.rule(Rule::new(format!("account:u{n}"), "*", "r"))
                })
                .limits(limits)
                .build()
        };
        assert!(rules(RoleLimits::default()).is_err());
        assert!(rules(RoleLimits::default().rules(Limit::UNBOUNDED)).is_ok());
        let long = "x".repeat(300);
        let named = |limits: RoleLimits| {
            RoleAuthorizer::builder()
                .role(long.clone(), ["a"])
                .limits(limits)
                .build()
        };
        assert!(named(RoleLimits::default()).is_err());
        assert!(named(RoleLimits::default().name_bytes(Limit::at(300))).is_ok());
        let tight = RoleAuthorizer::builder()
            .role("r", ["a", "b"])
            .limits(RoleLimits::UNBOUNDED.actions_per_role(Limit::at(1)))
            .build();
        assert!(tight.is_err());
    }

    #[test]
    fn the_limits_read_from_configuration() {
        use coqui_access::guatiao::Map;
        let mut limits = Map::new();
        limits.set("rules", Limit::UNBOUNDED.to_value().unwrap());
        limits.set("roles", 2u32);
        let mut policy = Map::new();
        policy.set("limits", limits);
        let read = RolePolicy::from_value(&policy.into()).unwrap().limits;
        assert_eq!(read.rules, Limit::UNBOUNDED);
        assert_eq!(read.roles, Limit::at(2));
        assert_eq!(read.scope_kinds, Limit::at(64));
        assert_eq!(
            RolePolicy::from_value(&Map::new().into()).unwrap().limits,
            RoleLimits::default()
        );
    }

    #[test]
    fn a_policy_is_plain_data_that_any_format_carries() {
        let mut policy = RolePolicy::new();
        policy.scope_kinds = vec!["site".to_owned()];
        policy.roles = vec![Role::new("viewer", ["document.read"])];
        policy.rules = vec![Rule::new("group:staff", "site:north", "viewer")];
        let value = policy.to_value().unwrap();
        assert_eq!(RolePolicy::from_value(&value).unwrap(), policy);
        let authorizer = RoleAuthorizer::try_from(policy).unwrap();
        assert!(allowed(
            &authorizer,
            &person("i", "n", &["staff"]),
            "document.read",
            "site:north"
        ));
        let mut surprise = coqui_access::guatiao::Map::new();
        surprise.set("surprise", true);
        assert!(RolePolicy::from_value(&surprise.into()).is_err());
        let empty = coqui_access::guatiao::Map::new();
        assert_eq!(
            RolePolicy::from_value(&empty.into()).unwrap(),
            RolePolicy::new()
        );
    }

    #[test]
    fn a_refused_policy_is_a_bad_value_for_the_host() {
        let refused = RoleAuthorizer::builder()
            .rule(Rule::new("account:x", "*", "missing"))
            .build()
            .unwrap_err();
        let error = ProviderError::from(refused);
        assert_eq!(
            error.status,
            coqui_access::guatiao::Status::GUATIAO_ERR_BAD_VALUE
        );
    }
}
