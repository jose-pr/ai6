//! `coqui-roles` as a library a host loads: the Guatiao entry point of the `coqui_roles`
//! provider, whose configuration is a `coqui_roles::RolePolicy`.
//!
//! ```json
//! { "scope_kinds": ["site"],
//!   "roles": [ { "name": "viewer", "actions": ["document.read"] } ],
//!   "rules": [ { "subject": "group:staff", "scope": "site:north", "role": "viewer" } ] }
//! ```
//!
//! Every key is optional; no rule means everything is denied.

coqui_access::guatiao::providers!(coqui_roles::RoleAuthorizer);

#[cfg(test)]
mod tests {
    use coqui_access::guatiao::{FromValue, ToValue};
    use coqui_access::{AuthorizationRequest, Authorizer, Person};
    use coqui_roles::{Role, RoleAuthorizer, RolePolicy, Rule};

    fn config(kinds: &[&str]) -> RolePolicy {
        let mut policy = RolePolicy::new();
        policy.scope_kinds = kinds.iter().map(|kind| (*kind).to_owned()).collect();
        policy.roles = vec![Role::new("editor", ["document.write"])];
        policy.rules = vec![Rule::new("account:admin", "site:north", "editor")];
        policy
    }

    fn may(provider: &RoleAuthorizer, resource: &str) -> bool {
        provider
            .authorize(AuthorizationRequest {
                person: Person {
                    name: "admin".to_owned(),
                    ..Person::default()
                },
                action: "document.write".to_owned(),
                resource: resource.to_owned(),
                ancestors: Vec::new(),
            })
            .unwrap()
            .allowed
    }

    #[test]
    fn the_provider_builds_from_structured_configuration() {
        let provider = RoleAuthorizer::try_from(config(&["site"])).unwrap();
        assert!(may(&provider, "site:north"));
        assert!(!may(&provider, "site:south"));
        assert!(RoleAuthorizer::try_from(config(&["other"])).is_err());
        let nothing = RoleAuthorizer::try_from(RolePolicy::new()).unwrap();
        assert!(!may(&nothing, "site:north"));
    }

    #[test]
    fn the_config_round_trips_through_a_guatiao_value() {
        let value = config(&["site"]).to_value().unwrap();
        let read = RolePolicy::from_value(&value).unwrap();
        assert_eq!(read.roles.len(), 1);
        assert_eq!(read.rules[0].subject, "account:admin");
        assert!(RoleAuthorizer::try_from(read).is_ok());
    }
}
