//! Views Moneypenny shows: a strict, versioned schema the app renders natively.
//!
//! She builds a view with the `show` tool; the gateway refuses one that is not exactly this
//! schema, within these bounds, and tells her why, so the app is never sent one. Nothing in a
//! view runs: there is no HTML, no script and no outside link (a Markdown link must point into
//! the app, `app:/...`). A form drafts the input of one of her `propose_*` tools; submitting it
//! records a proposal, which the person still confirms.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::actions;

/// The schema version this gateway shows.
pub(crate) const VIEW_VERSION: u32 = 1;
const MAX_BLOCKS: usize = 24;
const MAX_TEXT: usize = 8 * 1024;
const MAX_SHORT: usize = 512;
const MAX_COLUMNS: usize = 12;
const MAX_ROWS: usize = 200;
const MAX_ITEMS: usize = 100;
const MAX_SERIES: usize = 8;
const MAX_POINTS: usize = 200;
const MAX_DIFF: usize = 16 * 1024;
const MAX_FIELDS: usize = 20;
const MAX_VIEW_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct View {
    pub(crate) version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    pub(crate) blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pair {
    pub(crate) key: String,
    pub(crate) value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Series {
    pub(crate) name: String,
    pub(crate) points: Vec<Point>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Point {
    pub(crate) x: Value,
    pub(crate) y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Moment {
    /// Unix seconds.
    pub(crate) at: f64,
    pub(crate) label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) label: String,
    /// `text`, `number`, `boolean`, `choice` or `json`.
    #[serde(rename = "type")]
    pub(crate) kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) choices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Block {
    /// Markdown: paragraphs, `#` headings, `-` lists, `**bold**`, `*italic*`, `` `code` ``
    /// and links into the app.
    Text {
        markdown: String,
    },
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
    },
    KeyValues {
        items: Vec<Pair>,
    },
    Chart {
        /// `bar` or `line`.
        chart: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        x_label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        y_label: Option<String>,
        series: Vec<Series>,
    },
    /// A Mission, Station or Theater, linking to its page in the app.
    Card {
        /// `mission`, `station` or `theater`.
        card: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fields: Vec<Pair>,
    },
    Timeline {
        items: Vec<Moment>,
    },
    Diff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        before: String,
        after: String,
    },
    /// A draft of a `propose_*` tool's input: `fixed` as given, `fields` for the person.
    Form {
        tool: String,
        title: String,
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        fixed: Map<String, Value>,
        fields: Vec<Field>,
    },
}

fn short(what: &str, text: &str) -> Result<(), String> {
    if text.len() > MAX_SHORT || text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(format!(
            "{what} is longer than {MAX_SHORT} bytes or has control characters"
        ));
    }
    Ok(())
}

fn scalar(what: &str, value: &Value) -> Result<(), String> {
    match value {
        Value::Null | Value::Bool(_) => Ok(()),
        Value::Number(number) if number.as_f64().is_some_and(f64::is_finite) => Ok(()),
        Value::String(text) => short(what, text),
        _ => Err(format!(
            "{what} must be text, a number, true, false or null"
        )),
    }
}

fn identifier(what: &str, text: &str) -> Result<(), String> {
    if ai6::directorates::section_00::assistant::valid_identity(text) {
        Ok(())
    } else {
        Err(format!("{what} is not an identifier"))
    }
}

/// Every Markdown link points into the app (`app:/...`).
fn links_stay_in_the_app(markdown: &str) -> Result<(), String> {
    let mut rest = markdown;
    while let Some(at) = rest.find("](") {
        let target = &rest[at + 2..];
        if !target.starts_with("app:/") {
            return Err(
                "a link must point into the app (app:/...); outside links are not shown".to_owned(),
            );
        }
        rest = target;
    }
    Ok(())
}

impl View {
    /// Checks every bound; the reason names the first one broken.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.version != VIEW_VERSION {
            return Err(format!("version must be {VIEW_VERSION}"));
        }
        if let Some(title) = &self.title {
            short("title", title)?;
        }
        if self.blocks.is_empty() || self.blocks.len() > MAX_BLOCKS {
            return Err(format!("a view has 1 to {MAX_BLOCKS} blocks"));
        }
        if self
            .blocks
            .iter()
            .filter(|block| matches!(block, Block::Form { .. }))
            .count()
            > 1
        {
            return Err("a view has at most one form".to_owned());
        }
        for (index, block) in self.blocks.iter().enumerate() {
            block
                .validate()
                .map_err(|reason| format!("block {index}: {reason}"))?;
        }
        let size = serde_json::to_vec(self).map_or(usize::MAX, |bytes| bytes.len());
        if size > MAX_VIEW_BYTES {
            return Err(format!("a view is at most {MAX_VIEW_BYTES} bytes"));
        }
        Ok(())
    }

    /// The view's form, when it has one.
    pub(crate) fn form(&self) -> Option<(&str, &Map<String, Value>, &[Field])> {
        self.blocks.iter().find_map(|block| match block {
            Block::Form {
                tool,
                fixed,
                fields,
                ..
            } => Some((tool.as_str(), fixed, fields.as_slice())),
            _ => None,
        })
    }
}

impl Block {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Text { markdown } => {
                if markdown.trim().is_empty() || markdown.len() > MAX_TEXT {
                    return Err(format!("text is 1 to {MAX_TEXT} bytes"));
                }
                links_stay_in_the_app(markdown)
            }
            Self::Table { columns, rows } => {
                if columns.is_empty() || columns.len() > MAX_COLUMNS {
                    return Err(format!("a table has 1 to {MAX_COLUMNS} columns"));
                }
                for column in columns {
                    short("a column", column)?;
                }
                if rows.len() > MAX_ROWS {
                    return Err(format!("a table has at most {MAX_ROWS} rows"));
                }
                for row in rows {
                    if row.len() != columns.len() {
                        return Err("every row has one cell per column".to_owned());
                    }
                    for cell in row {
                        scalar("a cell", cell)?;
                    }
                }
                Ok(())
            }
            Self::KeyValues { items } => {
                if items.is_empty() || items.len() > MAX_ITEMS {
                    return Err(format!("a key-value list has 1 to {MAX_ITEMS} items"));
                }
                pairs(items)
            }
            Self::Chart {
                chart,
                title,
                x_label,
                y_label,
                series,
            } => {
                if !matches!(chart.as_str(), "bar" | "line") {
                    return Err("chart is bar or line".to_owned());
                }
                for text in [title, x_label, y_label].into_iter().flatten() {
                    short("a chart label", text)?;
                }
                if series.is_empty() || series.len() > MAX_SERIES {
                    return Err(format!("a chart has 1 to {MAX_SERIES} series"));
                }
                for line in series {
                    short("a series name", &line.name)?;
                    if line.points.is_empty() || line.points.len() > MAX_POINTS {
                        return Err(format!("a series has 1 to {MAX_POINTS} points"));
                    }
                    for point in &line.points {
                        scalar("x", &point.x)?;
                        if !point.y.is_finite() {
                            return Err("y must be a finite number".to_owned());
                        }
                    }
                }
                Ok(())
            }
            Self::Card {
                card,
                id,
                operation_id,
                state,
                fields,
            } => {
                if !matches!(card.as_str(), "mission" | "station" | "theater") {
                    return Err("card is mission, station or theater".to_owned());
                }
                identifier("id", id)?;
                if let Some(operation) = operation_id {
                    identifier("operation_id", operation)?;
                }
                if card == "mission" && operation_id.is_none() {
                    return Err("a Mission card names its operation_id".to_owned());
                }
                if let Some(state) = state {
                    short("state", state)?;
                }
                if fields.len() > MAX_FIELDS {
                    return Err(format!("a card has at most {MAX_FIELDS} fields"));
                }
                pairs(fields)
            }
            Self::Timeline { items } => {
                if items.is_empty() || items.len() > MAX_ITEMS {
                    return Err(format!("a timeline has 1 to {MAX_ITEMS} items"));
                }
                for item in items {
                    if !item.at.is_finite() || item.at < 0.0 {
                        return Err("at is Unix seconds".to_owned());
                    }
                    short("a label", &item.label)?;
                    if let Some(detail) = &item.detail {
                        short("a detail", detail)?;
                    }
                }
                Ok(())
            }
            Self::Diff {
                title,
                before,
                after,
            } => {
                if let Some(title) = title {
                    short("title", title)?;
                }
                if before.len() > MAX_DIFF || after.len() > MAX_DIFF {
                    return Err(format!("each side of a diff is at most {MAX_DIFF} bytes"));
                }
                Ok(())
            }
            Self::Form {
                tool,
                title,
                fixed,
                fields,
            } => {
                if !actions::write_tools()
                    .iter()
                    .any(|write| write.name == tool)
                {
                    return Err(format!(
                        "a form drafts one of the propose_* tools, not {tool:?}"
                    ));
                }
                short("title", title)?;
                if fields.is_empty() || fields.len() > MAX_FIELDS {
                    return Err(format!("a form has 1 to {MAX_FIELDS} fields"));
                }
                for value in fixed.values() {
                    if serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len()) > MAX_TEXT
                    {
                        return Err("a fixed value is too long".to_owned());
                    }
                }
                for field in fields {
                    identifier("a field name", &field.name)?;
                    short("a field label", &field.label)?;
                    if fixed.contains_key(&field.name) {
                        return Err(format!("{} is both fixed and a field", field.name));
                    }
                    if !matches!(
                        field.kind.as_str(),
                        "text" | "number" | "boolean" | "choice" | "json"
                    ) {
                        return Err(
                            "a field's type is text, number, boolean, choice or json".to_owned()
                        );
                    }
                    if (field.kind == "choice") == field.choices.is_empty() {
                        return Err("a choice field, and only one, lists choices".to_owned());
                    }
                    for choice in &field.choices {
                        short("a choice", choice)?;
                    }
                }
                Ok(())
            }
        }
    }
}

fn pairs(items: &[Pair]) -> Result<(), String> {
    for item in items {
        short("a key", &item.key)?;
        scalar("a value", &item.value)?;
    }
    Ok(())
}

/// Parses and checks the `view` input of `show`.
pub(crate) fn parse(input: &Value) -> Result<View, String> {
    let object = input
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get("view"))
        .ok_or("show takes exactly {\"view\": {...}}")?;
    let view: View = serde_json::from_value(object.clone()).map_err(|error| error.to_string())?;
    view.validate()?;
    Ok(view)
}

/// The input of the form's tool: its fixed values and the person's values for its fields,
/// each converted to the field's type. The tool's own schema checks the result.
pub(crate) fn form_input(
    view: &View,
    values: &Map<String, Value>,
) -> Result<(String, Value), String> {
    let (tool, fixed, fields) = view.form().ok_or("the view has no form")?;
    let mut input = fixed.clone();
    for (name, value) in values {
        let field = fields
            .iter()
            .find(|field| field.name == *name)
            .ok_or_else(|| format!("{name} is not a field of the form"))?;
        let value = match (field.kind.as_str(), value) {
            ("json", Value::String(text)) => {
                serde_json::from_str(text).map_err(|_| format!("{name} is not valid JSON"))?
            }
            ("choice", Value::String(choice)) if !field.choices.contains(choice) => {
                return Err(format!("{name} is not one of its choices"));
            }
            (_, value) => value.clone(),
        };
        input.insert(name.clone(), value);
    }
    Ok((tool.to_owned(), Value::Object(input)))
}

/// The `show` tool's input schema, for Moneypenny.
pub(crate) fn schema() -> Value {
    use serde_json::json;

    let scalar = json!({"type": ["string", "number", "boolean", "null"]});
    let pair = json!({
        "type": "object", "additionalProperties": false, "required": ["key", "value"],
        "properties": {"key": {"type": "string"}, "value": scalar}
    });
    let block = |kind: &str, required: &[&str], properties: Value| {
        let mut properties = properties;
        properties["kind"] = json!({"const": kind});
        let mut required: Vec<&str> = required.to_vec();
        required.insert(0, "kind");
        json!({
            "type": "object", "additionalProperties": false,
            "required": required, "properties": properties
        })
    };
    let point = json!({
        "type": "object", "additionalProperties": false, "required": ["x", "y"],
        "properties": {"x": scalar, "y": {"type": "number"}}
    });
    let series = json!({
        "type": "object", "additionalProperties": false, "required": ["name", "points"],
        "properties": {"name": {"type": "string"}, "points": {"type": "array", "items": point}}
    });
    let moment = json!({
        "type": "object", "additionalProperties": false, "required": ["at", "label"],
        "properties": {"at": {"type": "number"}, "label": {"type": "string"},
                       "detail": {"type": "string"}}
    });
    let field = json!({
        "type": "object", "additionalProperties": false, "required": ["name", "label", "type"],
        "properties": {
            "name": {"type": "string"}, "label": {"type": "string"},
            "type": {"enum": ["text", "number", "boolean", "choice", "json"]},
            "value": {}, "choices": {"type": "array", "items": {"type": "string"}}
        }
    });
    let blocks = vec![
        block(
            "text",
            &["markdown"],
            json!({"markdown": {"type": "string"}}),
        ),
        block(
            "table",
            &["columns", "rows"],
            json!({
                "columns": {"type": "array", "items": {"type": "string"}},
                "rows": {"type": "array", "items": {"type": "array", "items": scalar}}
            }),
        ),
        block(
            "key_values",
            &["items"],
            json!({"items": {"type": "array", "items": pair}}),
        ),
        block(
            "chart",
            &["chart", "series"],
            json!({
                "chart": {"enum": ["bar", "line"]}, "title": {"type": "string"},
                "x_label": {"type": "string"}, "y_label": {"type": "string"},
                "series": {"type": "array", "items": series}
            }),
        ),
        block(
            "card",
            &["card", "id"],
            json!({
                "card": {"enum": ["mission", "station", "theater"]},
                "id": {"type": "string"}, "operation_id": {"type": "string"},
                "state": {"type": "string"}, "fields": {"type": "array", "items": pair}
            }),
        ),
        block(
            "timeline",
            &["items"],
            json!({"items": {"type": "array", "items": moment}}),
        ),
        block(
            "diff",
            &["before", "after"],
            json!({
                "title": {"type": "string"}, "before": {"type": "string"},
                "after": {"type": "string"}
            }),
        ),
        block(
            "form",
            &["tool", "title", "fields"],
            json!({
                "tool": {"type": "string"}, "title": {"type": "string"},
                "fixed": {"type": "object"},
                "fields": {"type": "array", "items": field}
            }),
        ),
    ];
    let view = json!({
        "type": "object", "additionalProperties": false, "required": ["version", "blocks"],
        "properties": {
            "version": {"const": VIEW_VERSION},
            "title": {"type": "string"},
            "blocks": {"type": "array", "maxItems": MAX_BLOCKS, "items": {"oneOf": blocks}}
        }
    });
    json!({
        "type": "object", "additionalProperties": false, "required": ["view"],
        "properties": {"view": view}
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// One view of every kind, as Moneypenny would show it; the app's widget tests read the
    /// same file.
    pub(crate) fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../../clients/moneypenny/test/fixtures/assistant_views.json"
        ))
        .unwrap()
    }

    #[test]
    fn every_kind_of_the_fixture_is_accepted() {
        let fixture = fixture();
        for (name, view) in fixture["valid"].as_object().unwrap() {
            let parsed = parse(&json!({ "view": view }));
            assert!(parsed.is_ok(), "{name}: {parsed:?}");
        }
    }

    #[test]
    fn every_invalid_view_of_the_fixture_is_refused_with_its_reason() {
        let fixture = fixture();
        for case in fixture["invalid"].as_array().unwrap() {
            let refused = parse(&json!({ "view": case["view"] })).unwrap_err();
            let reason = case["reason"].as_str().unwrap();
            assert!(
                refused.contains(reason),
                "{refused:?} should name {reason:?}"
            );
        }
    }

    #[test]
    fn a_form_fills_its_tool_from_fixed_values_and_typed_fields() {
        let view = parse(&json!({"view": {"version": 1, "blocks": [{
            "kind": "form", "tool": "propose_roster_edit", "title": "Edit Agent 008",
            "fixed": {"kind": "agents", "key": "008"},
            "fields": [{"name": "definition", "label": "Settings", "type": "json"}]
        }]}}))
        .unwrap();
        let values = json!({"definition": "{\"priority\": 3}"});
        let (tool, input) = form_input(&view, values.as_object().unwrap()).unwrap();
        assert_eq!(tool, "propose_roster_edit");
        assert_eq!(
            input,
            json!({"kind": "agents", "key": "008", "definition": {"priority": 3}})
        );
        let sneaky = json!({"key": "007"});
        assert!(form_input(&view, sneaky.as_object().unwrap()).is_err());
    }
}
