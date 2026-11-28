// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Credentials inside a provider's configuration (a directory's bind password, an authorizer's
//! token): which fields hold one, what they may hold, and what a read of the record shows.
//!
//! A field is secret-bearing when the provider's own schema marks it `x-sensitive` or the record's
//! `secret_fields` names it; no name is assumed. Such a field holds a **reference**,
//! `{ provider = "file", key = "..." }` or `{ secret = "name" }`, read when the provider is
//! built and used in memory only. A literal there still works and is reported as a weakening.
//! Anything that shows the record shows a reference as it is and a literal as [`REDACTED`], and a
//! write that sends [`REDACTED`] back leaves the stored literal as it was.

use std::collections::BTreeSet;

use serde_json::{Map, Value};
pub use zeroize::Zeroizing;

use crate::bootstrap::SecretRef;

/// What a secret-bearing literal is shown as.
pub const REDACTED: &str = "<redacted>";

/// Which fields of a provider's configuration hold a credential.
#[derive(Debug, Clone, Copy)]
pub struct SecretBearing<'a> {
    /// The provider's configuration schema, when one is known.
    pub schema: Option<&'a Value>,
    /// The record's `secret_fields`.
    pub listed: &'a [String],
}

impl SecretBearing<'_> {
    /// Calls `visit` on each secret-bearing field of `config`, at any depth.
    fn walk<E>(
        &self,
        config: &mut Map<String, Value>,
        schema: Option<&Value>,
        visit: &mut dyn FnMut(&str, &mut Value) -> Result<(), E>,
    ) -> Result<(), E> {
        let properties = schema
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object);
        for (field, value) in config.iter_mut() {
            let property = properties.and_then(|properties| properties.get(field));
            let marked = property
                .and_then(|property| property.get("x-sensitive"))
                .and_then(Value::as_bool)
                == Some(true);
            if marked || self.listed.iter().any(|name| name == field) {
                visit(field, value)?;
            } else if let Some(inner) = value.as_object_mut() {
                self.walk(inner, property, visit)?;
            }
        }
        Ok(())
    }

    /// `config` with each secret-bearing literal replaced by [`REDACTED`]; a reference stays.
    pub fn redact(&self, config: &mut Map<String, Value>) {
        let shown: Result<(), std::convert::Infallible> =
            self.walk(config, self.schema, &mut |_, value| {
                if value.is_string() {
                    *value = Value::String(REDACTED.to_owned());
                }
                Ok(())
            });
        // Nothing in the visitor can fail.
        let _ = shown;
    }

    /// Where a write sends [`REDACTED`] for a secret-bearing field, puts the literal `stored`
    /// holds there back, so a record read and written back loses nothing.
    pub fn keep_redacted(&self, config: &mut Map<String, Value>, stored: &Map<String, Value>) {
        let mut kept: Vec<(String, String)> = Vec::new();
        let mut literals = stored.clone();
        let mut collect = |field: &str, value: &mut Value| -> Result<(), ()> {
            if let Value::String(text) = value {
                kept.push((field.to_owned(), std::mem::take(text)));
            }
            Ok(())
        };
        let _ = self.walk(&mut literals, self.schema, &mut collect);
        let mut restore = |field: &str, value: &mut Value| -> Result<(), ()> {
            if value.as_str() == Some(REDACTED)
                && let Some((_, literal)) = kept.iter().find(|(name, _)| name == field)
            {
                *value = Value::String(literal.clone());
            }
            Ok(())
        };
        let _ = self.walk(config, self.schema, &mut restore);
    }

    /// Replaces each reference in a secret-bearing field with what `read` gives for it; a literal
    /// stays. The values are in memory only: the caller wipes what it builds from them.
    ///
    /// # Errors
    ///
    /// A field holding an object that is no reference, or what `read` refuses; the text names the
    /// field and never a value.
    pub fn resolve(
        &self,
        config: &mut Map<String, Value>,
        read: &dyn Fn(&SecretRef) -> Result<Zeroizing<String>, String>,
    ) -> Result<(), String> {
        self.walk(config, self.schema, &mut |field, value| {
            if !value.is_object() {
                return Ok(());
            }
            let reference: SecretRef = crate::directorates::m::records::from_json(value)
                .map_err(|_| format!("`{field}` is a credential: a secret reference"))?;
            let text = read(&reference).map_err(|why| format!("`{field}`: {why}"))?;
            *value = Value::String(text.as_str().to_owned());
            Ok(())
        })
    }

    /// The credential fields of `config` written as a literal, by name, for a weakening.
    pub fn literals(&self, config: &Map<String, Value>) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut copy = config.clone();
        let _ = self.walk(&mut copy, self.schema, &mut |field,
                                                        value|
         -> Result<(), ()> {
            if value.is_string() {
                found.insert(field.to_owned());
            }
            Ok(())
        });
        found
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const PLANTED: &str = "hunter2-planted";

    fn config() -> Map<String, Value> {
        json!({
            "url": "ldaps://dc",
            "bind_password": PLANTED,
            "inner": { "token": PLANTED, "name": "x" },
            "ref_password": { "provider": "file", "key": "ldap-bind" },
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn listed() -> Vec<String> {
        ["bind_password", "token", "ref_password"]
            .map(str::to_owned)
            .to_vec()
    }

    fn schema() -> Value {
        json!({"properties": {"inner": {"properties": {"name": {"x-sensitive": true}}}}})
    }

    #[test]
    fn a_literal_is_shown_redacted_and_a_reference_as_it_is_at_any_depth() {
        let list = listed();
        let bearing = SecretBearing {
            schema: None,
            listed: &list,
        };
        let mut shown = config();
        bearing.redact(&mut shown);
        assert_eq!(shown["bind_password"], REDACTED);
        assert_eq!(shown["inner"]["token"], REDACTED);
        assert_eq!(shown["inner"]["name"], "x");
        assert_eq!(shown["url"], "ldaps://dc");
        assert_eq!(shown["ref_password"], config()["ref_password"]);
        assert!(!Value::Object(shown).to_string().contains(PLANTED));
    }

    #[test]
    fn the_schema_marks_fields_with_no_list_and_no_name_is_assumed() {
        let marked = schema();
        let bearing = SecretBearing {
            schema: Some(&marked),
            listed: &[],
        };
        let mut shown = config();
        bearing.redact(&mut shown);
        assert_eq!(shown["inner"]["name"], REDACTED);
        // `bind_password` is named by neither: shown as written.
        assert_eq!(shown["bind_password"], PLANTED);
        assert_eq!(
            bearing.literals(&config()).into_iter().collect::<Vec<_>>(),
            ["name"]
        );
    }

    #[test]
    fn a_redacted_value_written_back_keeps_the_stored_literal() {
        let list = listed();
        let bearing = SecretBearing {
            schema: None,
            listed: &list,
        };
        let mut sent = config();
        bearing.redact(&mut sent);
        sent.insert("url".to_owned(), json!("ldaps://other"));
        bearing.keep_redacted(&mut sent, &config());
        assert_eq!(sent["bind_password"], PLANTED);
        assert_eq!(sent["inner"]["token"], PLANTED);
        assert_eq!(sent["url"], "ldaps://other");
        // A new literal replaces the old; a reference replaces a literal.
        let mut changed = config();
        changed.insert("bind_password".to_owned(), json!("new-one"));
        bearing.keep_redacted(&mut changed, &config());
        assert_eq!(changed["bind_password"], "new-one");
    }

    #[test]
    fn a_reference_is_read_into_memory_and_a_bad_one_is_refused_naming_the_field() {
        let list = listed();
        let bearing = SecretBearing {
            schema: None,
            listed: &list,
        };
        let mut built = config();
        built.insert(
            "bind_password".to_owned(),
            json!({ "provider": "file", "key": "ldap-bind" }),
        );
        let read = |reference: &SecretRef| -> Result<Zeroizing<String>, String> {
            match reference {
                SecretRef::Provider(source) if source.key == "ldap-bind" => {
                    Ok(Zeroizing::new("from-the-file".to_owned()))
                }
                _ => Err("not found".to_owned()),
            }
        };
        bearing.resolve(&mut built, &read).unwrap();
        assert_eq!(built["bind_password"], "from-the-file");
        assert_eq!(built["ref_password"], "from-the-file");
        // The literal in `inner.token` stays as written.
        assert_eq!(built["inner"]["token"], PLANTED);

        let mut bad = config();
        bad.insert("bind_password".to_owned(), json!({ "nonsense": 1 }));
        let refused = bearing.resolve(&mut bad, &read).unwrap_err();
        assert!(refused.contains("bind_password"), "{refused}");
        let mut missing = config();
        missing.insert(
            "bind_password".to_owned(),
            json!({ "provider": "file", "key": "other" }),
        );
        let refused = bearing.resolve(&mut missing, &read).unwrap_err();
        assert!(
            refused.contains("bind_password") && !refused.contains(PLANTED),
            "{refused}"
        );
    }

    #[test]
    fn literals_are_named_for_a_weakening_and_never_shown() {
        let list = listed();
        let bearing = SecretBearing {
            schema: None,
            listed: &list,
        };
        let found = bearing.literals(&config());
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            ["bind_password", "token"]
        );
    }
}
