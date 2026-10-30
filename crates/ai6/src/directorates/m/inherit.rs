//! Policy down the tree of Operations: an Operation's definition layered over its ancestors'.
//!
//! The definitions of an Operation's lineage are layered as key/value tables, root first and
//! the Operation itself last, before they are read as one Operation: tables merge key by key
//! at any depth, while scalars and arrays replace. The nearest Operation wins, as an Agent's
//! keys win over the Agent it `extends`. Presets and defaults then apply once to the layered
//! table, so a key an ancestor wrote counts as written.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// Keys an Operation never takes from an ancestor, dotted for a key inside a table: they name
/// a checkout or create work (`sources`, `sleepers`), or place the Operation in the tree.
pub const NOT_INHERITED: [&str; 10] = [
    "name",
    "description",
    "path",
    "theater",
    "config",
    "workspace.source",
    "sources",
    "sleepers",
    "parent",
    "inherit",
];

/// Tables that replace as a whole, as a scalar does: an identity is one value, never half of
/// one Operation's and half of another's.
const WHOLE_TABLES: [&str; 1] = ["cover_identity"];

/// One Operation's definition layered over its ancestors', with the writer of each key.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Layered {
    /// The keys of one `[operation.<id>]` table.
    pub(crate) definition: Map<String, Value>,
    /// Each key written, dotted (`green_ink.kinds`), and the codename of the Operation that
    /// wrote it.
    pub(crate) origins: BTreeMap<String, String>,
}

/// Layers `chain`, each Operation's codename and definition, root first and the Operation
/// being resolved last. The keys of [`NOT_INHERITED`] are taken from the last one only.
pub(crate) fn layer(chain: &[(&str, &Map<String, Value>)]) -> Layered {
    let mut layered = Layered::default();
    let Some(last) = chain.len().checked_sub(1) else {
        return layered;
    };
    for (index, (writer, definition)) in chain.iter().enumerate() {
        let mut definition = (*definition).clone();
        if index != last {
            for key in NOT_INHERITED {
                remove_dotted(&mut definition, key);
            }
        }
        merge(
            &mut layered.definition,
            definition,
            "",
            writer,
            &mut layered.origins,
        );
    }
    layered
}

/// Removes the dotted `key` from `table`, if it is there.
fn remove_dotted(table: &mut Map<String, Value>, key: &str) {
    match key.split_once('.') {
        None => {
            table.remove(key);
        }
        Some((head, rest)) => {
            if let Some(Value::Object(inner)) = table.get_mut(head) {
                remove_dotted(inner, rest);
            }
        }
    }
}

/// Merges `source`, written by `writer`, into `target`: a table into a table key by key, any
/// other value replacing what was there. `prefix` is the dotted path of `target`.
fn merge(
    target: &mut Map<String, Value>,
    source: Map<String, Value>,
    prefix: &str,
    writer: &str,
    origins: &mut BTreeMap<String, String>,
) {
    for (key, value) in source {
        let path = format!("{prefix}{key}");
        match value {
            Value::Object(inner) if !WHOLE_TABLES.contains(&path.as_str()) => {
                let slot = target
                    .entry(key)
                    .or_insert_with(|| Value::Object(Map::new()));
                if !slot.is_object() {
                    forget(origins, &path);
                    *slot = Value::Object(Map::new());
                }
                if let Value::Object(slot) = slot {
                    merge(slot, inner, &format!("{path}."), writer, origins);
                }
            }
            value => {
                forget(origins, &path);
                origins.insert(path, writer.to_owned());
                target.insert(key, value);
            }
        }
    }
}

/// Forgets the writer of `path` and of every key under it.
fn forget(origins: &mut BTreeMap<String, String>, path: &str) {
    let below = format!("{path}.");
    origins.retain(|key, _| key != path && !key.starts_with(&below));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => unreachable!("a test table"),
        }
    }

    #[test]
    fn tables_merge_scalars_and_arrays_replace_and_each_key_names_its_writer() {
        let root = table(json!({
            "description": "the deception plan",
            "path": "/srv/bodyguard",
            "station_labels": {"site": "london"},
            "stations": ["hq-1", "hq-2"],
            "green_ink": {"preset": "licensed", "unattended": {"field": ["push"]}},
            "workspace": {"source": "/srv/plans", "base": "main"},
            "sources": {"jira": "LCS"},
            "cover_identity": {"name": "LCS", "email": "lcs@example.invalid"},
        }));
        let child = table(json!({
            "parent": "BODYGUARD",
            "station_labels": {"gpu": "yes"},
            "stations": ["lab"],
            "green_ink": {"kinds": ["push"], "unattended": {"desk": ["read"]}},
            "cover_identity": {"name": "Fortitude"},
        }));
        let layered = layer(&[("BODYGUARD", &root), ("FORTITUDE", &child)]);
        assert_eq!(
            Value::Object(layered.definition),
            json!({
                "parent": "BODYGUARD",
                "station_labels": {"site": "london", "gpu": "yes"},
                "stations": ["lab"],
                "green_ink": {
                    "preset": "licensed",
                    "kinds": ["push"],
                    "unattended": {"field": ["push"], "desk": ["read"]},
                },
                "workspace": {"base": "main"},
                "cover_identity": {"name": "Fortitude"},
            })
        );
        let origin = |key: &str| layered.origins.get(key).map(String::as_str);
        assert_eq!(origin("green_ink.preset"), Some("BODYGUARD"));
        assert_eq!(origin("green_ink.kinds"), Some("FORTITUDE"));
        assert_eq!(origin("station_labels.site"), Some("BODYGUARD"));
        assert_eq!(origin("stations"), Some("FORTITUDE"));
        assert_eq!(origin("cover_identity"), Some("FORTITUDE"));
        assert_eq!(origin("parent"), Some("FORTITUDE"));
        assert_eq!(origin("path"), None, "never inherited");
        assert_eq!(origin("workspace.source"), None);
        assert_eq!(origin("workspace.base"), Some("BODYGUARD"));
    }

    #[test]
    fn a_table_replacing_a_scalar_forgets_its_writer() {
        let root = table(json!({"vetting": false}));
        let child = table(json!({"vetting": {"enabled": true}}));
        let layered = layer(&[("P", &root), ("G", &child)]);
        assert_eq!(layered.definition["vetting"], json!({"enabled": true}));
        assert_eq!(layered.origins.get("vetting"), None);
        assert_eq!(
            layered.origins.get("vetting.enabled").map(String::as_str),
            Some("G")
        );
        assert_eq!(layer(&[]), Layered::default());
    }
}
