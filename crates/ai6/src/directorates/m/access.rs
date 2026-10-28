//! Pluggable authentication, identity lookup and authorization contracts.
//!
//! The kinds, models and the first-match role authorizer belong to the shared `coqui-access`
//! and `coqui-roles` crates and are re-exported here, so a provider built for any host loads
//! into AI6. This module adds what is AI6's own: the Station, Operation and Theater scope
//! kinds, the action names the gateway checks, and the `access.toml` policy file.

use core::fmt;
use std::io::Read;
use std::path::Path;

pub use coqui_access::{
    AccountInfo, AccountPut, Accounts, Attempt, Authenticator, AuthorizationDecision,
    AuthorizationRequest, Authorizer, Description, Directory, KIND_PREFIX, Lookup, Person,
    PutOutcome, Verdict, Waiting, takes,
};
pub use coqui_roles::{
    Role, RoleAuthorizer, RoleAuthorizerBuilder, RolePolicy, RolePolicyError, Rule,
};
use serde::Deserialize;

/// The resource kinds a role rule may be scoped to in AI6: `station:<id>`, `operation:<id>`,
/// `theater:<id>`, `config:roster` (the roster every Station reads) and `config:assistant`
/// (Moneypenny's repositories). A rule may also name `*`.
pub const SCOPE_KINDS: [&str; 4] = ["station", "operation", "theater", "config"];

/// The actions the gateway checks, for writing roles. `cable.send` and `cable.read` on
/// `operation:<id>` bound what Moneypenny may do with Cables on the person's behalf.
/// `assistant.memory.admin` on `config:assistant` reads another person's memory, audited; a
/// person's own memory needs no action, and nobody changes another's.
pub const ACTIONS: &[&str] = &[
    "agent.configure",
    "agent.read",
    "approval.class",
    "approval.decide",
    "assistant.memory.admin",
    "audit.read",
    "cable.read",
    "cable.send",
    "mission.control",
    "mission.create",
    "mission.read",
    "pool.manage",
    "pool.read",
    "roster.manage",
    "roster.read",
    "session.input",
    "session.read",
    "station.configure",
    "station.read",
];

/// A role authorizer builder that accepts AI6's scope kinds and no others.
pub fn role_authorizer() -> RoleAuthorizerBuilder {
    RoleAuthorizer::builder().scope_kinds(SCOPE_KINDS)
}

/// Why an `access.toml` policy could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AccessPolicyError(String);

impl fmt::Display for AccessPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl core::error::Error for AccessPolicyError {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyDocument {
    #[serde(default, rename = "role")]
    roles: std::collections::BTreeMap<String, RoleDefinition>,
    #[serde(default, rename = "rule")]
    rules: Vec<RuleDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleDefinition {
    actions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleDefinition {
    subject: String,
    scope: String,
    role: String,
}

const MAX_POLICY_BYTES: usize = 1024 * 1024;

/// Reads an `access.toml` policy: `[role.<name>]` tables with `actions`, then `[[rule]]`
/// entries binding a `subject` to a `role` at a `scope`, tried in order.
///
/// # Errors
///
/// Returns an error when the text is over 1 MiB, is not the policy format, or names a role,
/// subject or scope the authorizer refuses.
pub fn role_authorizer_from_toml(source: &str) -> Result<RoleAuthorizer, AccessPolicyError> {
    if source.len() > MAX_POLICY_BYTES {
        return Err(AccessPolicyError(
            "access policy exceeds the 1 MiB limit".into(),
        ));
    }
    let document: PolicyDocument = toml::from_str(source)
        .map_err(|error| AccessPolicyError(format!("invalid access policy: {error}")))?;
    let mut builder = role_authorizer();
    for (name, definition) in document.roles {
        builder = builder.role(name, definition.actions);
    }
    for rule in document.rules {
        builder = builder.rule(Rule::new(rule.subject, rule.scope, rule.role));
    }
    builder
        .build()
        .map_err(|error| AccessPolicyError(format!("invalid access policy: {error}")))
}

/// Reads and validates an `access.toml` policy file.
///
/// # Errors
///
/// Returns an error when the file cannot be read or [`role_authorizer_from_toml`] refuses it.
pub fn load_role_authorizer(path: impl AsRef<Path>) -> Result<RoleAuthorizer, AccessPolicyError> {
    let path = path.as_ref();
    let mut source = String::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take((MAX_POLICY_BYTES + 1) as u64)
                .read_to_string(&mut source)
        })
        .map_err(|error| {
            AccessPolicyError(format!(
                "{}: could not read access policy: {error}",
                path.display()
            ))
        })?;
    role_authorizer_from_toml(&source)
        .map_err(|error| AccessPolicyError(format!("{}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: &str, name: &str, groups: &[&str]) -> Person {
        Person {
            id: id.to_owned(),
            name: name.to_owned(),
            groups: groups.iter().map(|group| (*group).to_owned()).collect(),
            ..Person::default()
        }
    }

    fn request(person: Person, action: &str, resource: &str) -> AuthorizationRequest {
        AuthorizationRequest {
            person,
            action: action.to_owned(),
            resource: resource.to_owned(),
        }
    }

    #[test]
    fn role_policy_scopes_roles_and_defaults_to_deny() {
        let policy = role_authorizer_from_toml(
            r#"
                [role.viewer]
                actions = ["mission.read", "transcript.read"]

                [role.operator]
                actions = ["mission.read", "session.input", "session.kill"]

                [[rule]]
                subject = "group:field"
                scope = "operation:G"
                role = "viewer"

                [[rule]]
                subject = "person:person-ops"
                scope = "operation:G"
                role = "operator"
            "#,
        )
        .unwrap();
        let debug = format!("{policy:?}");
        assert!(!debug.contains("person-ops"));
        assert!(!debug.contains("field"));
        let viewer = person("person-viewer", "viewer", &["field"]);
        let operator = person("person-ops", "operator", &[]);
        let allowed = |person: &Person, action: &str, resource: &str| {
            policy
                .authorize(request(person.clone(), action, resource))
                .unwrap()
                .allowed
        };
        assert!(allowed(&viewer, "mission.read", "operation:G"));
        assert!(!allowed(&viewer, "session.input", "operation:G"));
        assert!(!allowed(&viewer, "mission.read", "operation:L"));
        assert!(allowed(&operator, "session.input", "operation:G"));
        assert!(!allowed(&operator, "session.input", "operation:L"));
        assert!(!allowed(
            &person("unlisted", "unlisted", &[]),
            "mission.read",
            "operation:G"
        ));
    }

    #[test]
    fn role_policy_rejects_unknown_roles_and_unrecognized_scopes() {
        assert!(
            role_authorizer_from_toml(
                "[[rule]]\nsubject='account:007'\nscope='operation:G'\nrole='missing'"
            )
            .is_err()
        );
        assert!(role_authorizer_from_toml(
            "[role.viewer]\nactions=['mission.read']\n[[rule]]\nsubject='account:007'\nscope='universe:all'\nrole='viewer'"
        )
        .is_err());
        assert!(role_authorizer_from_toml("[role.viewer]\nunknown = 1").is_err());
        assert!(role_authorizer_from_toml(&" ".repeat(MAX_POLICY_BYTES + 1)).is_err());
    }

    #[test]
    fn role_policy_can_limit_station_administration_to_one_station() {
        let policy = role_authorizer_from_toml(
            r#"
                [role.station-admin]
                actions = ["station.read", "station.configure", "audit.read"]

                [[rule]]
                subject = "account:operator"
                scope = "station:station-a"
                role = "station-admin"
            "#,
        )
        .unwrap();
        let operator = person("operator-id", "operator", &[]);
        let allowed = |resource: &str| {
            policy
                .authorize(request(operator.clone(), "station.configure", resource))
                .unwrap()
                .allowed
        };
        assert!(allowed("station:station-a"));
        assert!(!allowed("station:station-b"));
        assert!(!allowed("operation:station-a"));
    }

    #[test]
    fn every_scope_kind_is_accepted_by_the_ai6_authorizer() {
        for kind in SCOPE_KINDS {
            let policy = format!(
                "[role.r]\nactions=['mission.read']\n[[rule]]\nsubject='account:a'\nscope='{kind}:x'\nrole='r'"
            );
            assert!(role_authorizer_from_toml(&policy).is_ok(), "{kind}");
        }
    }

    #[cfg(feature = "daemon")]
    #[test]
    fn the_shared_kind_names_are_the_ones_ai6_hosts() {
        use guatiao::library::Kind;

        assert_eq!(<dyn Authenticator>::NAME, "coqui_authenticator");
        assert_eq!(<dyn Directory>::NAME, "coqui_directory");
        assert_eq!(<dyn Authorizer>::NAME, "coqui_authorizer");
        assert_eq!(<dyn Accounts>::NAME, "coqui_accounts");
    }

    #[cfg(feature = "daemon")]
    #[test]
    fn guatiao_mock_loads_and_serves_all_access_kinds() {
        use guatiao::Provider;
        use guatiao::library::ProviderError;

        #[derive(Default, Provider)]
        #[provider(Authenticator, Directory, Authorizer)]
        struct MockAccess;

        impl Authenticator for MockAccess {
            fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
                if attempt.name.as_deref() == Some("007")
                    && attempt.secret.as_deref() == Some("secret")
                {
                    Ok(Verdict::Valid(Lookup {
                        by: "name".to_owned(),
                        value: "007".to_owned(),
                    }))
                } else {
                    Ok(Verdict::Invalid)
                }
            }
        }

        impl Directory for MockAccess {
            fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
                Ok(
                    (lookup.by == "name" && lookup.value == "007").then(|| Person {
                        display_name: Some("James Bond".to_owned()),
                        email: Some("007@example.invalid".to_owned()),
                        ..person("person-007", "007", &["field"])
                    }),
                )
            }
        }

        impl Authorizer for MockAccess {
            fn authorize(
                &self,
                request: AuthorizationRequest,
            ) -> Result<AuthorizationDecision, ProviderError> {
                let allowed = request.action == "mission.read"
                    && request.resource == "operation:G"
                    && request.person.groups.iter().any(|group| group == "field");
                Ok(AuthorizationDecision {
                    allowed,
                    ..AuthorizationDecision::default()
                })
            }
        }

        crate::guatiao::local_providers!(
            id = "ai6-access-test-providers",
            version = "0.0.0",
            providers = [MockAccess]
        );

        let mut registry = guatiao::library::Registry::new("ai6-access-test", "0.0.0");
        registry
            .register_local("ai6-access-test-providers", library)
            .unwrap();
        let plugins = crate::station::plugins::Plugins::from_registry(registry);

        let authenticators = plugins.authenticators();
        let directories = plugins.directories();
        let authorizers = plugins.authorizers();
        assert_eq!(authenticators.len(), 1);
        assert_eq!(directories.len(), 1);
        assert_eq!(authorizers.len(), 1);

        let authenticator = plugins.authenticator(&authenticators[0].id, None).unwrap();
        let directory = plugins.directory(&directories[0].id, None).unwrap();
        let authorizer = plugins.authorizer(&authorizers[0].id, None).unwrap();
        let Verdict::Valid(lookup) = authenticator
            .authenticate(Attempt {
                name: Some("007".to_owned()),
                secret: Some("secret".to_owned()),
                ..Attempt::default()
            })
            .unwrap()
        else {
            panic!("the mock Authenticator should accept the test credentials");
        };
        let person = directory.lookup(lookup).unwrap().unwrap();
        assert!(
            authorizer
                .authorize(request(person.clone(), "mission.read", "operation:G"))
                .unwrap()
                .allowed
        );
        assert!(
            !authorizer
                .authorize(request(person, "session.input", "operation:G"))
                .unwrap()
                .allowed
        );
    }
}
