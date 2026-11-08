//! Every setting a person writes, read from the one declaration on its struct.
//!
//! A section is named by a dotted path: `provider`, `registry.limits.sessions` and `oo.chat` are
//! sections of `ai6.cfg`, and `record.station.scheduler` or `record.operation.issues` are parts
//! of a record kind. `ai6 config schema [SECTION]` prints one, and [`reference`](mod@reference) writes the tables
//! of the narrative docs and what Outer Office reads from the same schemas.

pub mod reference;

use guatiao::Schema;
use serde_json::Value;

use crate::bootstrap::Bootstrap;
use crate::directorates::m::assistant::AssistantSettings;
use crate::directorates::m::compartments::CompartmentSettings;
use crate::directorates::m::config::{StationRuntime, UnexRecord};
use crate::directorates::m::cover::CoverDefinition;
use crate::directorates::m::review::ReviewPolicy;
use crate::directorates::m::roster::{
    AgentDefinition, AgentRoleDefinition, OperationDefinition, RosterSettings,
};
use crate::directorates::m::watches::WatchDefinition;

/// A declaration's schema as JSON. A derive that cannot build its schema is a defect a test holds
/// every root to, so it reads as an empty schema here.
fn json_of<T: Schema>() -> Value {
    T::schema()
        .ok()
        .and_then(|schema| serde_json::to_value(guatiao_serde::Serializable::from(&schema)).ok())
        .unwrap_or(Value::Null)
}

/// The prefix of each declaration, longest first so a path finds the one it is under. The empty
/// prefix is `ai6.cfg` itself.
fn roots() -> Vec<(&'static str, Value)> {
    let mut roots = vec![
        ("record.station", json_of::<StationRuntime>()),
        ("record.unex", json_of::<UnexRecord>()),
        ("record.assistant", json_of::<AssistantSettings>()),
        ("record.operation", json_of::<OperationDefinition>()),
        ("record.agent", json_of::<AgentDefinition>()),
        ("record.role", json_of::<AgentRoleDefinition>()),
        ("record.cover", json_of::<CoverDefinition>()),
        ("record.roster", json_of::<RosterSettings>()),
        ("record.compartment", json_of::<CompartmentSettings>()),
        ("record.review", json_of::<ReviewPolicy>()),
        ("record.watch", json_of::<WatchDefinition>()),
    ];
    #[cfg(feature = "daemon")]
    roots.push((
        "record.source",
        json_of::<crate::station::sources::InstanceKeys>(),
    ));
    roots.push(("", json_of::<Bootstrap>()));
    roots
}

/// The member `key` of an object's schema, an optional object's among its arms.
fn member<'a>(node: &'a Value, key: &str) -> Option<&'a Value> {
    if let Some(field) = node.get("properties").and_then(|fields| fields.get(key)) {
        return Some(field);
    }
    node.get("anyOf")?
        .as_array()?
        .iter()
        .find_map(|arm| member(arm, key))
}

/// The fields of a table's schema, its own or an optional table's. A choice between shapes, as
/// a secret reference is, is one value and has none.
fn fields(node: &Value) -> Option<&serde_json::Map<String, Value>> {
    if let Some(fields) = node.get("properties").and_then(Value::as_object) {
        return Some(fields);
    }
    let arms = node.get("anyOf")?.as_array()?;
    let mut present = arms.iter().filter(|arm| arm["type"] != "null");
    let only = present.next()?;
    if present.next().is_some() {
        return None;
    }
    fields(only)
}

/// The schema of `section`, a dotted path; `None` when no setting or table has that name.
pub fn schema(section: &str) -> Option<Value> {
    for (prefix, root) in roots() {
        let rest = if prefix.is_empty() {
            Some(section)
        } else if section == prefix {
            return Some(root);
        } else {
            section
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('.'))
        };
        let Some(rest) = rest else { continue };
        if prefix.is_empty() && section.is_empty() {
            return Some(root);
        }
        let mut node = &root;
        for key in rest.split('.') {
            node = member(node, key)?;
        }
        return Some(node.clone());
    }
    None
}

/// Every section's name and title: each table of `ai6.cfg` and of every record kind.
pub fn sections() -> Vec<(String, String)> {
    fn walk(path: &str, node: &Value, found: &mut Vec<(String, String)>) {
        let Some(members) = fields(node) else { return };
        let title = node
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !path.is_empty() {
            found.push((path.to_owned(), title.to_owned()));
        }
        for (key, field) in members {
            let at = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            walk(&at, field, found);
        }
    }
    let mut found = Vec::new();
    for (prefix, root) in roots() {
        walk(prefix, &root, &mut found);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_is_found_by_its_path_and_lists_with_its_title() {
        assert!(schema("provider").unwrap()["properties"]["poll"].is_object());
        assert!(schema("registry.limits.sessions").unwrap()["properties"]["records"].is_object());
        assert!(schema("record.station").unwrap()["properties"]["scheduler"].is_object());
        assert!(schema("record.station.scheduler").unwrap()["properties"]["check_in"].is_object());
        assert!(schema("record.unex.gateway").unwrap()["properties"]["frame_bytes"].is_object());
        assert!(schema("record.operation.issues").unwrap()["properties"]["create"].is_object());
        assert!(schema("record.unex.nothing").is_none());
        assert!(schema("nothing").is_none());
        let sections = sections();
        for (section, _) in &sections {
            assert!(schema(section).is_some(), "{section}");
        }
        let names: Vec<&str> = sections.iter().map(|(name, _)| name.as_str()).collect();
        for name in [
            "provider",
            "registry.limits.cables",
            "unex.pages",
            "station.plugins",
            "record.station.runner",
            "record.assistant.desk",
            "record.unex.text",
        ] {
            assert!(names.contains(&name), "{name}: {names:?}");
        }
        let titled = sections
            .iter()
            .find(|(name, _)| name == "record.station.scheduler")
            .unwrap();
        assert_eq!(titled.1, "The scheduler");
    }
}
