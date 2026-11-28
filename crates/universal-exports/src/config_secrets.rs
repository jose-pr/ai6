//! The credentials in a `unex` record's provider configurations, as the record is shown and
//! written back. Which fields hold one is the record's `secret_fields`; a reference shows as it is,
//! a literal as `<redacted>`, and `<redacted>` sent back keeps the stored literal
//! ([`ai6::directorates::m::provider_secrets`]).

use ai6::directorates::m::provider_secrets::SecretBearing;
use serde_json::{Map, Value};

/// The provider configurations of a `unex` definition: the four `*_config` objects, each
/// authenticator's `config` and the sign-in policy's.
fn configs(definition: &mut Value) -> Vec<(String, &mut Map<String, Value>)> {
    let Some(record) = definition.as_object_mut() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (name, value) in record.iter_mut() {
        match (name.as_str(), value) {
            (
                "directory_config"
                | "authorizer_config"
                | "accounts_config"
                | "page_authenticator_config",
                Value::Object(config),
            ) => found.push((name.clone(), config)),
            ("authenticators", Value::Array(entries)) => {
                for (index, entry) in entries.iter_mut().enumerate() {
                    if let Some(Value::Object(config)) = entry
                        .as_object_mut()
                        .and_then(|entry| entry.get_mut("config"))
                    {
                        found.push((format!("authenticators[{index}]"), config));
                    }
                }
            }
            ("sign_in_policy", Value::Object(policy)) => {
                if let Some(Value::Object(config)) = policy.get_mut("config") {
                    found.push(("sign_in_policy".to_owned(), config));
                }
            }
            _ => {}
        }
    }
    found
}

fn listed(definition: &Value) -> Vec<String> {
    definition
        .get("secret_fields")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `definition` as it is shown: every secret-bearing literal is `<redacted>`.
pub(crate) fn redact_unex(definition: &mut Value) {
    let listed = listed(definition);
    let bearing = SecretBearing {
        schema: None,
        listed: &listed,
    };
    for (_, config) in configs(definition) {
        bearing.redact(config);
    }
}

/// Puts the literals `stored` holds back where `definition` sends `<redacted>`.
pub(crate) fn keep_redacted_unex(definition: &mut Value, stored: &Value) {
    let names = {
        let mut names = listed(definition);
        names.extend(listed(stored));
        names
    };
    let bearing = SecretBearing {
        schema: None,
        listed: &names,
    };
    let mut stored = stored.clone();
    let held: std::collections::BTreeMap<String, Map<String, Value>> = configs(&mut stored)
        .into_iter()
        .map(|(label, config)| (label, config.clone()))
        .collect();
    for (label, config) in configs(definition) {
        if let Some(held) = held.get(&label) {
            bearing.keep_redacted(config, held);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const PLANTED: &str = "hunter2-planted";

    fn record() -> Value {
        json!({
            "directory": "ldap",
            "secret_fields": ["bind_password"],
            "directory_config": {"url": "ldaps://dc", "bind_password": PLANTED},
            "authorizer_config": {"bind_password": {"provider": "file", "key": "ldap-bind"}},
            "authenticators": [{"provider": "ldap", "config": {"bind_password": PLANTED}}],
        })
    }

    #[test]
    fn a_shown_record_holds_no_credential_and_keeps_its_references() {
        let mut shown = record();
        redact_unex(&mut shown);
        assert!(!shown.to_string().contains(PLANTED), "{shown}");
        assert_eq!(shown["directory_config"]["bind_password"], "<redacted>");
        assert_eq!(shown["directory_config"]["url"], "ldaps://dc");
        assert_eq!(
            shown["authorizer_config"]["bind_password"],
            record()["authorizer_config"]["bind_password"]
        );
        assert_eq!(
            shown["authenticators"][0]["config"]["bind_password"],
            "<redacted>"
        );
    }

    #[test]
    fn a_record_shown_and_written_back_keeps_what_it_stored() {
        let mut sent = record();
        redact_unex(&mut sent);
        sent["directory_config"]["url"] = json!("ldaps://other");
        keep_redacted_unex(&mut sent, &record());
        assert_eq!(sent["directory_config"]["bind_password"], PLANTED);
        assert_eq!(sent["directory_config"]["url"], "ldaps://other");
        assert_eq!(
            sent["authenticators"][0]["config"]["bind_password"],
            PLANTED
        );
        // A name no one listed is shown as written: nothing is assumed.
        let mut unlisted = record();
        unlisted.as_object_mut().unwrap().remove("secret_fields");
        redact_unex(&mut unlisted);
        assert_eq!(unlisted["directory_config"]["bind_password"], PLANTED);
    }
}
