//! Pluggable authentication, identity lookup and authorization contracts.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

use guatiao::library::ProviderError;
use guatiao::{FromValue, Schema, ToValue, Value};
use serde::Deserialize;

/// Credentials and request facts presented to an authenticator.
///
/// Secret material and request metadata are omitted from `Debug` output.
#[derive(Clone, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Attempt {
    /// Account name supplied by the user, when the method has one.
    pub name: Option<String>,
    /// Password or bearer key supplied for this attempt.
    pub secret: Option<String>,
    /// Lowercase request header names and their values.
    pub headers: BTreeMap<String, String>,
    /// Remote peer address when available.
    pub peer: Option<String>,
    /// Whether the request arrived over a protected transport.
    pub secure: Option<bool>,
    /// Identity of a client certificate whose key was verified by the transport.
    pub certificate: Option<String>,
}

impl fmt::Debug for Attempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Attempt")
            .field("name", &self.name.as_ref().map(|_| "…"))
            .field("secret", &self.secret.as_ref().map(|_| "…"))
            .field("headers", &self.headers.len())
            .field("peer", &self.peer.as_ref().map(|_| "…"))
            .field("secure", &self.secure)
            .field("certificate", &self.certificate.as_ref().map(|_| "…"))
            .finish()
    }
}

/// An identity lookup key returned after an authenticator accepts credentials.
#[derive(Clone, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Lookup {
    /// Directory attribute to match, such as `id`, `name` or `email`.
    pub by: String,
    /// Value to resolve in a configured Directory.
    pub value: String,
}

impl fmt::Debug for Lookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Lookup")
            .field("by", &self.by)
            .field("value", &"…")
            .finish()
    }
}

/// Result of checking one credential attempt.
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao, tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    /// This authenticator does not handle the presented method or identity.
    Unknown,
    /// This authenticator recognized the identity but rejected the credentials.
    Invalid,
    /// The credentials are valid; resolve this reference through a Directory.
    Valid(Lookup),
}

/// A person record returned only by an authoritative Directory.
#[derive(Clone, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct Person {
    /// Stable directory identity.
    pub id: String,
    /// Canonical account name.
    pub name: String,
    /// Human-readable name, when the directory has one.
    pub display_name: Option<String>,
    /// Email address, when the directory has one.
    pub email: Option<String>,
    /// Directory groups used by authorization policy.
    pub groups: Vec<String>,
    /// Additional directory-owned attributes.
    pub attributes: BTreeMap<String, Value>,
}

impl fmt::Debug for Person {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Person")
            .field("id", &"…")
            .field("name", &"…")
            .field("display_name", &self.display_name.as_ref().map(|_| "…"))
            .field("email", &self.email.as_ref().map(|_| "…"))
            .field("groups", &self.groups.len())
            .field("attributes", &self.attributes.len())
            .finish()
    }
}

/// A resource-scoped authorization request.
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct AuthorizationRequest {
    /// Person returned by the authoritative Directory.
    pub person: Person,
    /// Requested operation, such as `mission.read` or `session.input`.
    pub action: String,
    /// Resource identifier scoped to a Station, Operation, Theater or whole service.
    pub resource: String,
}

/// Authorization result and provider-defined limits for the granted action.
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct AuthorizationDecision {
    /// Whether policy permits the requested action.
    pub allowed: bool,
    /// Limits such as lease duration or maximum concurrent operations.
    pub limits: BTreeMap<String, Value>,
}

/// Checks credentials and returns only a Directory lookup reference.
#[guatiao::kind(name = "lineupmux_authenticator")]
pub trait Authenticator: Send + Sync {
    /// Checks one attempt without returning person details or credential material.
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError>;
}

/// Resolves an authenticated reference into the authoritative person record.
#[guatiao::kind(name = "lineupmux_directory")]
pub trait Directory: Send + Sync {
    /// Returns the person for `lookup`, or `None` when the reference is unknown.
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError>;
}

/// Authorizes one action on one Operation, Theater or global resource.
#[guatiao::kind(name = "lineupmux_authorizer")]
pub trait Authorizer: Send + Sync {
    /// Evaluates the request using the supplied authoritative person record.
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError>;
}

/// First-match role policy configured in `access.toml`.
///
/// The `[[rule]]` entries bind one `account:<name>`, `person:<id>` or
/// `group:<name>` to a role at an exact Station/Operation/Theater scope or `*`.
/// A matching
/// rule is decisive; when its role lacks the requested action, access is denied.
#[derive(Clone, Default)]
pub struct RoleAuthorizer {
    roles: BTreeMap<String, BTreeSet<String>>,
    rules: Vec<RoleRule>,
}

#[derive(Clone)]
struct RoleRule {
    subject: RoleSubject,
    scope: String,
    role: String,
}

#[derive(Clone)]
enum RoleSubject {
    Account(String),
    Person(String),
    Group(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleDocument {
    #[serde(default, rename = "role")]
    roles: BTreeMap<String, RoleDefinition>,
    #[serde(default, rename = "rule")]
    rules: Vec<RoleRuleDefinition>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleDefinition {
    actions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleRuleDefinition {
    subject: String,
    scope: String,
    role: String,
}

/// Invalid or unreadable built-in role policy configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RolePolicyError(String);

impl fmt::Display for RolePolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl core::error::Error for RolePolicyError {}

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
    /// Parses a built-in first-match role policy from TOML.
    pub fn from_toml(source: &str) -> Result<Self, RolePolicyError> {
        if source.len() > MAX_ROLE_POLICY_BYTES {
            return Err(RolePolicyError(
                "access policy exceeds the 1 MiB limit".into(),
            ));
        }
        let document: RoleDocument = toml::from_str(source)
            .map_err(|error| RolePolicyError(format!("invalid access policy: {error}")))?;
        if document.roles.len() > MAX_ROLES || document.rules.len() > MAX_ROLE_RULES {
            return Err(RolePolicyError(
                "access policy has too many roles or rules".into(),
            ));
        }
        let mut roles = BTreeMap::new();
        for (name, definition) in document.roles {
            if !policy_name(&name) || definition.actions.is_empty() {
                return Err(RolePolicyError(format!("invalid role {name:?}")));
            }
            if definition.actions.len() > MAX_ROLE_ACTIONS {
                return Err(RolePolicyError(format!(
                    "role {name:?} has too many actions"
                )));
            }
            let mut actions = BTreeSet::new();
            for action in definition.actions {
                if !policy_name(&action) || !actions.insert(action.clone()) {
                    return Err(RolePolicyError(format!(
                        "invalid or duplicate action {action:?} in role {name:?}"
                    )));
                }
            }
            roles.insert(name, actions);
        }

        let mut rules = Vec::with_capacity(document.rules.len());
        for rule in document.rules {
            let (kind, value) = rule
                .subject
                .split_once(':')
                .ok_or_else(|| RolePolicyError("rule subject needs a kind and value".into()))?;
            if !policy_name(value) {
                return Err(RolePolicyError("rule subject value is invalid".into()));
            }
            let subject = match kind {
                "account" => RoleSubject::Account(value.to_owned()),
                "person" => RoleSubject::Person(value.to_owned()),
                "group" => RoleSubject::Group(value.to_owned()),
                _ => {
                    return Err(RolePolicyError(
                        "rule subject must be account, person or group".into(),
                    ));
                }
            };
            if rule.scope != "*" {
                let (scope_kind, scope_value) = rule.scope.split_once(':').ok_or_else(|| {
                    RolePolicyError(
                        "rule scope must be *, station:<id>, operation:<id> or theater:<id>".into(),
                    )
                })?;
                if !matches!(scope_kind, "station" | "operation" | "theater")
                    || !policy_name(scope_value)
                {
                    return Err(RolePolicyError(
                        "rule scope must be *, station:<id>, operation:<id> or theater:<id>".into(),
                    ));
                }
            }
            if !roles.contains_key(&rule.role) {
                return Err(RolePolicyError(format!(
                    "rule references unknown role {:?}",
                    rule.role
                )));
            }
            rules.push(RoleRule {
                subject,
                scope: rule.scope,
                role: rule.role,
            });
        }
        Ok(Self { roles, rules })
    }

    /// Reads and validates an `access.toml` policy file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RolePolicyError> {
        let path = path.as_ref();
        let mut source = String::new();
        std::fs::File::open(path)
            .and_then(|file| {
                file.take((MAX_ROLE_POLICY_BYTES + 1) as u64)
                    .read_to_string(&mut source)
            })
            .map_err(|error| {
                RolePolicyError(format!(
                    "{}: could not read access policy: {error}",
                    path.display()
                ))
            })?;
        if source.len() > MAX_ROLE_POLICY_BYTES {
            return Err(RolePolicyError(format!(
                "{}: access policy exceeds the 1 MiB limit",
                path.display()
            )));
        }
        Self::from_toml(&source)
            .map_err(|error| RolePolicyError(format!("{}: {error}", path.display())))
    }

    fn decision(&self, request: &AuthorizationRequest) -> AuthorizationDecision {
        let allowed = self
            .rules
            .iter()
            .find(|rule| {
                (rule.scope == "*" || rule.scope == request.resource)
                    && subject_matches(&rule.subject, &request.person)
            })
            .is_some_and(|rule| {
                self.roles
                    .get(&rule.role)
                    .is_some_and(|actions| actions.contains(&request.action))
            });
        AuthorizationDecision {
            allowed,
            limits: BTreeMap::new(),
        }
    }
}

impl Authorizer for RoleAuthorizer {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        Ok(self.decision(&request))
    }
}

/// Guatiao configuration value containing an `access.toml` policy document.
#[derive(guatiao::Schema, guatiao::FromValue, guatiao::ToValue)]
#[non_exhaustive]
pub struct RoleAuthorizerConfig {
    /// Complete role and rule TOML source.
    pub policy_toml: String,
}

/// Guatiao-loadable provider for the built-in role policy evaluator.
#[derive(guatiao::Provider)]
#[provider(
    Authorizer,
    config = RoleAuthorizerConfig,
    id = "ai6_roles",
    name = "AI6 Role Authorizer"
)]
pub struct RoleAuthorizerProvider(RoleAuthorizer);

impl TryFrom<RoleAuthorizerConfig> for RoleAuthorizerProvider {
    type Error = ProviderError;

    fn try_from(config: RoleAuthorizerConfig) -> Result<Self, Self::Error> {
        RoleAuthorizer::from_toml(&config.policy_toml)
            .map(Self)
            .map_err(|error| {
                ProviderError::new(guatiao::Status::GUATIAO_ERR_BAD_VALUE, &error.to_string())
            })
    }
}

impl Authorizer for RoleAuthorizerProvider {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        self.0.authorize(request)
    }
}

fn subject_matches(subject: &RoleSubject, person: &Person) -> bool {
    match subject {
        RoleSubject::Account(name) => name == &person.name,
        RoleSubject::Person(id) => id == &person.id,
        RoleSubject::Group(group) => person.groups.iter().any(|member| member == group),
    }
}

fn policy_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(char::is_control)
        && !value.chars().any(char::is_whitespace)
}

const MAX_ROLE_POLICY_BYTES: usize = 1024 * 1024;
const MAX_ROLES: usize = 256;
const MAX_ROLE_ACTIONS: usize = 256;
const MAX_ROLE_RULES: usize = 10_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_kind_floor_hashes_are_pinned() {
        assert_eq!(AuthenticatorVtable::FLOOR_HASH, 4_205_343_784);
        assert_eq!(DirectoryVtable::FLOOR_HASH, 1_937_390_361);
        assert_eq!(AuthorizerVtable::FLOOR_HASH, 2_693_612_885);
    }

    #[test]
    fn valid_verdict_round_trips_its_lookup_reference() {
        let verdict = Verdict::Valid(Lookup {
            by: "email".to_owned(),
            value: "agent@example.invalid".to_owned(),
        });
        let value = verdict.to_value().unwrap();
        let decoded = Verdict::from_value(&value).unwrap();
        assert_eq!(decoded, verdict);
    }

    #[test]
    fn attempt_debug_redacts_credentials_and_request_values() {
        let attempt = Attempt {
            name: Some("private-user".to_owned()),
            secret: Some("private-token".to_owned()),
            headers: BTreeMap::from([("authorization".to_owned(), "private-header".to_owned())]),
            peer: Some("192.0.2.10".to_owned()),
            secure: Some(true),
            certificate: Some("private-certificate".to_owned()),
        };

        let debug = format!("{attempt:?}");
        for secret in [
            "private-user",
            "private-token",
            "private-header",
            "192.0.2.10",
            "private-certificate",
        ] {
            assert!(!debug.contains(secret));
        }
        assert!(!debug.contains("authorization"));
        assert!(debug.contains("headers: 1"));
    }

    #[test]
    fn lookup_and_person_debug_redact_identity_values() {
        let verdict = Verdict::Valid(Lookup {
            by: "email".to_owned(),
            value: "private@example.invalid".to_owned(),
        });
        let person = Person {
            id: "private-id".to_owned(),
            name: "private-name".to_owned(),
            display_name: Some("private-display".to_owned()),
            email: Some("private-email@example.invalid".to_owned()),
            groups: vec!["private-group".to_owned()],
            attributes: BTreeMap::from([("private-key".to_owned(), Value::default())]),
        };
        let debug = format!("{verdict:?} {person:?}");
        for private_value in [
            "private@example.invalid",
            "private-id",
            "private-name",
            "private-display",
            "private-email@example.invalid",
            "private-group",
            "private-key",
        ] {
            assert!(!debug.contains(private_value));
        }
        assert!(debug.contains("by: \"email\""));
    }

    #[test]
    fn role_policy_scopes_roles_and_defaults_to_deny() {
        let policy = RoleAuthorizer::from_toml(
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
        let viewer = Person {
            id: "person-viewer".to_owned(),
            name: "viewer".to_owned(),
            display_name: None,
            email: None,
            groups: vec!["field".to_owned()],
            attributes: BTreeMap::new(),
        };
        let operator = Person {
            id: "person-ops".to_owned(),
            name: "operator".to_owned(),
            display_name: None,
            email: None,
            groups: Vec::new(),
            attributes: BTreeMap::new(),
        };
        let request = |person, action: &str, resource: &str| AuthorizationRequest {
            person,
            action: action.to_owned(),
            resource: resource.to_owned(),
        };

        assert!(
            policy
                .authorize(request(viewer.clone(), "mission.read", "operation:G"))
                .unwrap()
                .allowed
        );
        assert!(
            !policy
                .authorize(request(viewer.clone(), "session.input", "operation:G"))
                .unwrap()
                .allowed
        );
        assert!(
            !policy
                .authorize(request(viewer, "mission.read", "operation:L"))
                .unwrap()
                .allowed
        );
        assert!(
            policy
                .authorize(request(operator.clone(), "session.input", "operation:G"))
                .unwrap()
                .allowed
        );
        assert!(
            !policy
                .authorize(request(operator, "session.input", "operation:L"))
                .unwrap()
                .allowed
        );
        assert!(
            !policy
                .authorize(request(
                    Person {
                        id: "unlisted".to_owned(),
                        name: "unlisted".to_owned(),
                        display_name: None,
                        email: None,
                        groups: Vec::new(),
                        attributes: BTreeMap::new(),
                    },
                    "mission.read",
                    "operation:G"
                ))
                .unwrap()
                .allowed
        );
    }

    #[test]
    fn role_policy_rejects_unknown_roles_and_unrecognized_scopes() {
        assert!(
            RoleAuthorizer::from_toml(
                "[[rule]]\nsubject='account:007'\nscope='operation:G'\nrole='missing'"
            )
            .is_err()
        );
        assert!(RoleAuthorizer::from_toml(
            "[role.viewer]\nactions=['mission.read']\n[[rule]]\nsubject='account:007'\nscope='universe:all'\nrole='viewer'"
        )
        .is_err());
        assert!(RoleAuthorizer::from_toml(&" ".repeat(MAX_ROLE_POLICY_BYTES + 1)).is_err());
    }

    #[test]
    fn role_policy_can_limit_station_administration_to_one_station() {
        let policy = RoleAuthorizer::from_toml(
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
        let person = Person {
            id: "operator-id".to_owned(),
            name: "operator".to_owned(),
            display_name: None,
            email: None,
            groups: Vec::new(),
            attributes: BTreeMap::new(),
        };
        let request = |resource: &str, action: &str| AuthorizationRequest {
            person: person.clone(),
            action: action.to_owned(),
            resource: resource.to_owned(),
        };
        assert!(
            policy
                .authorize(request("station:station-a", "station.configure"))
                .unwrap()
                .allowed
        );
        assert!(
            !policy
                .authorize(request("station:station-b", "station.configure"))
                .unwrap()
                .allowed
        );
        assert!(
            !policy
                .authorize(request("operation:station-a", "station.configure"))
                .unwrap()
                .allowed
        );
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
                        id: "person-007".to_owned(),
                        name: "007".to_owned(),
                        display_name: Some("James Bond".to_owned()),
                        email: Some("007@example.invalid".to_owned()),
                        groups: vec!["field".to_owned()],
                        attributes: BTreeMap::new(),
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
                    limits: BTreeMap::new(),
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
        assert_eq!(authenticators[0].id, directories[0].id);
        assert_eq!(directories[0].id, authorizers[0].id);

        let authenticator = plugins.authenticator(&authenticators[0].id, None).unwrap();
        let directory = plugins.directory(&directories[0].id, None).unwrap();
        let authorizer = plugins.authorizer(&authorizers[0].id, None).unwrap();
        let Verdict::Valid(lookup) = authenticator
            .authenticate(Attempt {
                name: Some("007".to_owned()),
                secret: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                peer: None,
                secure: None,
                certificate: None,
            })
            .unwrap()
        else {
            panic!("the mock Authenticator should accept the test credentials");
        };
        let person = directory.lookup(lookup).unwrap().unwrap();
        assert!(
            authorizer
                .authorize(AuthorizationRequest {
                    person: person.clone(),
                    action: "mission.read".to_owned(),
                    resource: "operation:G".to_owned(),
                })
                .unwrap()
                .allowed
        );
        assert!(
            !authorizer
                .authorize(AuthorizationRequest {
                    person,
                    action: "session.input".to_owned(),
                    resource: "operation:G".to_owned(),
                })
                .unwrap()
                .allowed
        );
    }
}
