//! What a personal API key may do: its person's grants, narrowed to the `ACTION@SCOPE` pairs
//! named when it was made.
//!
//! The narrowing travels on the [`Person`] a key signs in as, in two attributes, so that every
//! authorizer in the path sees it. It only removes: a request is allowed when the person's own
//! grants allow it and one scope covers it, so a grant the person loses the key loses too.

use core::fmt;
use core::str::FromStr;

use coqui_access::{
    AuthorizationDecision, AuthorizationRequest, Authorizer, Person,
};
use guatiao::library::ProviderError;
use guatiao::{FromValue, ToValue};

use super::{ACTIONS, SCOPE_KINDS};

/// The attribute holding a key's scopes, each as `ACTION@SCOPE` text.
pub const KEY_SCOPES_ATTRIBUTE: &str = "ai6.key.scopes";

/// The attribute holding the ID of the key a request was made with.
pub const KEY_ID_ATTRIBUTE: &str = "ai6.key.id";

/// One thing a key may do: an action, on a resource (or on everything under it).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyScope {
    /// An action from [`ACTIONS`].
    pub action: String,
    /// A resource such as `config:plugin` or `operation:G`, which covers the Operations under
    /// it, or `*` for every resource.
    pub scope: String,
}

/// Why `ACTION@SCOPE` text is not a key scope.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyScopeError(String);

impl fmt::Display for KeyScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::error::Error for KeyScopeError {}

impl KeyScope {
    /// Whether `request` is something this scope covers.
    pub fn covers(&self, request: &AuthorizationRequest) -> bool {
        self.action == request.action
            && (self.scope == "*"
                || self.scope == request.resource
                || request.ancestors.iter().any(|above| *above == self.scope))
    }
}

impl FromStr for KeyScope {
    type Err = KeyScopeError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (action, scope) = text
            .split_once('@')
            .ok_or_else(|| KeyScopeError("a scope is written ACTION@SCOPE".to_owned()))?;
        if !ACTIONS.contains(&action) {
            return Err(KeyScopeError(format!("{action:?} is not an action")));
        }
        let known = scope == "*"
            || SCOPE_KINDS.iter().any(|kind| {
                scope
                    .strip_prefix(kind)
                    .and_then(|rest| rest.strip_prefix(':'))
                    .is_some_and(|name| !name.is_empty() && !name.chars().any(char::is_control))
            });
        if !known {
            return Err(KeyScopeError(format!(
                "{scope:?} is not a scope: write kind:name (one of {}) or *",
                SCOPE_KINDS.join(", ")
            )));
        }
        Ok(Self {
            action: action.to_owned(),
            scope: scope.to_owned(),
        })
    }
}

impl fmt::Display for KeyScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.action, self.scope)
    }
}

/// Makes `person` act as the holder of key `key_id`, with only `scopes`.
pub fn narrow(person: &mut Person, key_id: &str, scopes: &[KeyScope]) {
    let texts: Vec<String> = scopes.iter().map(ToString::to_string).collect();
    if let (Ok(id), Ok(scopes)) = (key_id.to_owned().to_value(), texts.to_value()) {
        person.attributes.insert(KEY_ID_ATTRIBUTE.to_owned(), id);
        person
            .attributes
            .insert(KEY_SCOPES_ATTRIBUTE.to_owned(), scopes);
    }
}

/// Removes any narrowing from `person`, so an attribute a directory happened to carry under
/// these names grants and limits nothing.
pub fn clear(person: &mut Person) {
    person.attributes.remove(KEY_ID_ATTRIBUTE);
    person.attributes.remove(KEY_SCOPES_ATTRIBUTE);
}

/// The ID of the key `person` signed in with, when it was a personal key.
pub fn key_id(person: &Person) -> Option<String> {
    person
        .attributes
        .get(KEY_ID_ATTRIBUTE)
        .and_then(|value| String::from_value(value).ok())
}

/// Whether `person` signed in with a personal key.
pub fn is_narrowed(person: &Person) -> bool {
    person.attributes.contains_key(KEY_SCOPES_ATTRIBUTE)
}

/// Whether the narrowing on `request`'s person, if any, lets the request through. A scope
/// attribute that cannot be read allows nothing.
pub fn permits(request: &AuthorizationRequest) -> bool {
    let Some(value) = request.person.attributes.get(KEY_SCOPES_ATTRIBUTE) else {
        return true;
    };
    let Ok(texts) = Vec::<String>::from_value(value) else {
        return false;
    };
    texts
        .iter()
        .filter_map(|text| text.parse::<KeyScope>().ok())
        .any(|scope| scope.covers(request))
}

/// An authorizer that lets a request through only when `inner` allows it and the key it was
/// made with, if any, covers it.
pub struct KeyNarrowed<A: ?Sized> {
    inner: A,
}

impl<A> KeyNarrowed<A> {
    /// Narrows `inner`.
    pub fn new(inner: A) -> Self {
        Self { inner }
    }
}

impl<A: Authorizer> Authorizer for KeyNarrowed<A> {
    fn authorize(
        &self,
        request: AuthorizationRequest,
    ) -> Result<AuthorizationDecision, ProviderError> {
        if !permits(&request) {
            return Ok(AuthorizationDecision::default());
        }
        self.inner.authorize(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(action: &str, resource: &str, ancestors: &[&str]) -> AuthorizationRequest {
        AuthorizationRequest {
            person: Person::default(),
            action: action.to_owned(),
            resource: resource.to_owned(),
            ancestors: ancestors.iter().map(|text| (*text).to_owned()).collect(),
        }
    }

    #[test]
    fn a_scope_is_action_at_scope_and_nothing_else() {
        let scope: KeyScope = "config.manage@config:plugin".parse().unwrap();
        assert_eq!(scope.to_string(), "config.manage@config:plugin");
        for bad in ["config.manage", "nothing@config:plugin", "config.manage@plugin", "config.manage@config:"] {
            assert!(bad.parse::<KeyScope>().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_scope_covers_its_resource_and_what_sits_under_it() {
        let scope: KeyScope = "mission.read@operation:G".parse().unwrap();
        assert!(scope.covers(&request("mission.read", "operation:G", &[])));
        assert!(scope.covers(&request("mission.read", "operation:H", &["operation:G"])));
        assert!(!scope.covers(&request("mission.read", "operation:H", &[])));
        assert!(!scope.covers(&request("mission.control", "operation:G", &[])));
        let everywhere: KeyScope = "mission.read@*".parse().unwrap();
        assert!(everywhere.covers(&request("mission.read", "operation:H", &[])));
    }

    #[test]
    fn a_narrowed_person_is_allowed_only_what_a_scope_covers() {
        let mut person = Person::default();
        assert!(permits(&AuthorizationRequest {
            person: person.clone(),
            ..request("roster.manage", "config:roster", &[])
        }));
        narrow(&mut person, "k1", &["config.manage@config:plugin".parse().unwrap()]);
        assert!(is_narrowed(&person));
        assert_eq!(key_id(&person).as_deref(), Some("k1"));
        let ask = |action: &str, resource: &str| AuthorizationRequest {
            person: person.clone(),
            ..request(action, resource, &[])
        };
        assert!(permits(&ask("config.manage", "config:plugin")));
        assert!(!permits(&ask("config.manage", "config:roster")));
        assert!(!permits(&ask("roster.manage", "config:plugin")));
        clear(&mut person);
        assert!(!is_narrowed(&person));
    }
}
