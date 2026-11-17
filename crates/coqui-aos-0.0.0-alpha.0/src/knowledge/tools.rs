//! The tools, their input schemas and the checks of a call's arguments.

use alloc::vec::Vec;

use serde_json::{Map, Value, json};

use super::search::SearchMode;
use super::settings::{KnowledgeSettings, MAX_INTEGER};
use super::{ErrorClass, KnowledgeError};

/// The default of `top`.
pub(crate) const DEFAULT_TOP: i64 = 5;

/// A tool of the knowledge section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolName {
    /// `kb_list`.
    List,
    /// `kb_read`.
    Read,
    /// `kb_search`.
    Search,
}

impl ToolName {
    /// The tool's name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::List => "kb_list",
            Self::Read => "kb_read",
            Self::Search => "kb_search",
        }
    }

    /// The tool named `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        [Self::List, Self::Read, Self::Search]
            .into_iter()
            .find(|tool| tool.as_str() == name)
    }
}

fn declared(modes: &[SearchMode]) -> Vec<SearchMode> {
    SearchMode::ALL
        .into_iter()
        .filter(|mode| modes.contains(mode))
        .collect()
}

/// The tools `tools/list` names, in order, with their input schemas and no description:
/// `kb_list`, `kb_read` and, when `modes` declares one, `kb_search`.
#[must_use]
pub fn knowledge_tools(modes: &[SearchMode], settings: &KnowledgeSettings) -> Vec<Value> {
    let line = json!({"type": "integer", "minimum": 1, "maximum": MAX_INTEGER});
    let mut tools = alloc::vec![
        json!({
            "name": "kb_list",
            "inputSchema": {
                "type": "object",
                "properties": {"overlay": {"type": "string"}},
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "kb_read",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "from": line.clone(),
                    "to": line,
                    "overlay": {"type": "string"},
                    "context": {"type": "object"},
                },
                "required": ["path"],
                "additionalProperties": false,
            },
        }),
    ];
    let modes = declared(modes);
    if !modes.is_empty() {
        let mut query = json!({"type": "string", "minLength": 1});
        if let Some(most) = settings.max_query_characters.get() {
            query["maxLength"] = json!(most);
        }
        let mut top = json!({"type": "integer", "minimum": 1, "default": DEFAULT_TOP});
        if let Some(most) = settings.max_top.get() {
            top["maximum"] = json!(most);
        }
        let mut names = alloc::vec![Value::from("auto")];
        names.extend(modes.iter().map(|mode| Value::from(mode.as_str())));
        tools.push(json!({
            "name": "kb_search",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": query,
                    "top": top,
                    "overlay": {"type": "string"},
                    "mode": {"type": "string", "enum": names, "default": "auto"},
                    "path": {"type": "string"},
                },
                "required": ["query"],
                "additionalProperties": false,
            },
        }));
    }
    tools
}

fn invalid(message: &str) -> KnowledgeError {
    KnowledgeError::new(ErrorClass::InvalidArguments, message)
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn context_ok(value: &Value, depth: u64, budget: &mut u64, settings: &KnowledgeSettings) -> bool {
    if !settings.max_context_depth.allows(depth) {
        return false;
    }
    match value {
        Value::Bool(_) => true,
        Value::Number(_) => is_integer(value),
        Value::String(text) => {
            *budget = budget.saturating_add(text.len() as u64);
            settings.max_context_bytes.allows(*budget)
        }
        Value::Array(items) => items
            .iter()
            .all(|item| context_ok(item, depth + 1, budget, settings)),
        Value::Object(map) => map.iter().all(|(key, item)| {
            if !is_identifier(key) {
                return false;
            }
            *budget = budget.saturating_add(key.len() as u64);
            settings.max_context_bytes.allows(*budget)
                && context_ok(item, depth + 1, budget, settings)
        }),
        Value::Null => false,
    }
}

/// The arguments of a call of `tool`, with the defaults applied. A `null` is the argument absent.
///
/// # Errors
///
/// `invalid-arguments` when `tool` is none of the tools offered, the arguments are not an object
/// with only the keys the tool defines, or a value is not as the schema and the rules say.
pub fn knowledge_arguments(
    tool: &str,
    arguments: &Value,
    modes: &[SearchMode],
    settings: &KnowledgeSettings,
) -> Result<Map<String, Value>, KnowledgeError> {
    let tools = knowledge_tools(modes, settings);
    let schema = tools
        .iter()
        .find(|offered| offered["name"] == tool)
        .map(|offered| &offered["inputSchema"])
        .ok_or_else(|| invalid(&alloc::format!("no tool is named {tool}")))?;
    let Value::Object(given) = arguments else {
        return Err(invalid("the arguments are an object"));
    };
    let properties = schema["properties"]
        .as_object()
        .expect("a schema has properties");
    let mut out = Map::new();
    for (key, value) in given {
        let Some(spec) = properties.get(key) else {
            return Err(invalid(&alloc::format!("no argument is named {key}")));
        };
        if value.is_null() {
            continue;
        }
        match spec["type"].as_str() {
            Some("string") => {
                let Some(text) = value.as_str() else {
                    return Err(invalid(&alloc::format!("{key} is text")));
                };
                if let Some(names) = spec.get("enum").and_then(Value::as_array)
                    && !names.iter().any(|name| name == text)
                {
                    return Err(invalid(&alloc::format!("{key} is not one of its values")));
                }
                if key == "query" {
                    let long = !settings.max_query_characters.allows(text.chars().count());
                    let blank = text
                        .trim_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r'])
                        .is_empty();
                    if long || blank {
                        return Err(invalid("the query is empty or too long"));
                    }
                }
            }
            Some("integer") => {
                if !is_integer(value) {
                    return Err(invalid(&alloc::format!("{key} is a whole number")));
                }
                let number = value.as_i64().unwrap_or(i64::MAX);
                let most = spec
                    .get("maximum")
                    .and_then(Value::as_i64)
                    .unwrap_or(i64::MAX);
                let least = spec
                    .get("minimum")
                    .and_then(Value::as_i64)
                    .unwrap_or(i64::MIN);
                if !(least..=most).contains(&number) {
                    return Err(invalid(&alloc::format!("{key} is out of range")));
                }
            }
            _ => {
                let mut budget = 0;
                if !value.is_object() || !context_ok(value, 1, &mut budget, settings) {
                    return Err(invalid(&alloc::format!("{key} is not a usable context")));
                }
            }
        }
        out.insert(key.clone(), value.clone());
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for key in required {
            let key = key.as_str().unwrap_or_default();
            if !out.contains_key(key) {
                return Err(invalid(&alloc::format!("{key} is required")));
            }
        }
    }
    if tool == "kb_search" {
        out.entry("top").or_insert_with(|| json!(DEFAULT_TOP));
        out.entry("mode").or_insert_with(|| json!("auto"));
    }
    Ok(out)
}

/// Whether `value` is an integer the specification's call and result carry.
fn is_integer(value: &Value) -> bool {
    value
        .as_i64()
        .is_some_and(|number| number.unsigned_abs() <= MAX_INTEGER.unsigned_abs())
        || value
            .as_u64()
            .is_some_and(|number| number <= MAX_INTEGER.unsigned_abs())
}
