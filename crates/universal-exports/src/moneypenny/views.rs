//! Views Moneypenny shows: a strict, versioned schema the app renders natively.
//!
//! She builds a view with the `show` tool; the gateway refuses one that is not exactly this
//! schema, within these bounds, and tells her why, so the app is never sent one. Nothing in a
//! view runs: there is no HTML, no script and no outside link (a link, in Markdown, on a
//! schedule row or on a graph node, is an app route, `app:/<page>?...`, see [`super::routes`]).
//! A form drafts the input of one of her `propose_*` tools; submitting it records a proposal,
//! which the person still confirms. An `svg` block holds only what [`svg::sanitize`] accepts,
//! in its canonical form.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::checks::{identifier, short_within};
use super::routes::{self, Route};
use super::{actions, svg};

/// The schema version this gateway shows.
pub(crate) const VIEW_VERSION: u32 = 1;

/// The bounds a view is held to; each is at least 1. The app is sent them with a
/// conversation and checks views against the same values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ViewLimits {
    /// Blocks in one view (default 24).
    pub blocks: usize,
    /// Bytes of one view as JSON (default 64 KiB).
    pub view_bytes: usize,
    /// Bytes of a text block's Markdown (default 8 KiB).
    pub text_bytes: usize,
    /// Bytes of a title, label, cell or other short text, and of a `navigate` card's label
    /// (default 512).
    pub short_bytes: usize,
    /// Columns of a table (default 12).
    pub columns: usize,
    /// Rows of a table (default 200).
    pub rows: usize,
    /// Items of a key-value list or a timeline (default 100).
    pub items: usize,
    /// Series of a chart (default 8).
    pub series: usize,
    /// Points of one series (default 200).
    pub points: usize,
    /// Slices of a pie (default 12).
    pub pie_slices: usize,
    /// Bytes of each side of a diff (default 16 KiB).
    pub diff_bytes: usize,
    /// Fields of a form or a card (default 20).
    pub fields: usize,
    /// Rows of a schedule (default 50).
    pub schedule_rows: usize,
    /// Nodes of a graph (default 60).
    pub graph_nodes: usize,
    /// Edges of a graph (default 120).
    pub graph_edges: usize,
    /// Bytes of an SVG drawing (default 32 KiB).
    pub svg_bytes: usize,
    /// Elements of an SVG drawing (default 2,000).
    pub svg_elements: usize,
    /// How deep an SVG drawing nests its elements (default 16).
    pub svg_depth: usize,
}

impl Default for ViewLimits {
    fn default() -> Self {
        Self {
            blocks: 24,
            view_bytes: 64 * 1024,
            text_bytes: 8 * 1024,
            short_bytes: 512,
            columns: 12,
            rows: 200,
            items: 100,
            series: 8,
            points: 200,
            pie_slices: 12,
            diff_bytes: 16 * 1024,
            fields: 20,
            schedule_rows: 50,
            graph_nodes: 60,
            graph_edges: 120,
            svg_bytes: 32 * 1024,
            svg_elements: 2_000,
            svg_depth: 16,
        }
    }
}

impl ViewLimits {
    /// These limits with every zero raised to 1.
    #[must_use]
    pub(crate) fn at_least_one(mut self) -> Self {
        for limit in [
            &mut self.blocks,
            &mut self.view_bytes,
            &mut self.text_bytes,
            &mut self.short_bytes,
            &mut self.columns,
            &mut self.rows,
            &mut self.items,
            &mut self.series,
            &mut self.points,
            &mut self.pie_slices,
            &mut self.diff_bytes,
            &mut self.fields,
            &mut self.schedule_rows,
            &mut self.graph_nodes,
            &mut self.graph_edges,
            &mut self.svg_bytes,
            &mut self.svg_elements,
            &mut self.svg_depth,
        ] {
            *limit = (*limit).max(1);
        }
        self
    }
}

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

/// One bar of a schedule, from `start` to `end`: both RFC 3339 times or both numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Span {
    pub(crate) label: String,
    pub(crate) start: Value,
    pub(crate) end: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Node {
    pub(crate) id: String,
    pub(crate) label: String,
    /// `operation`, `station`, `theater`, `mission`, `session`, `agent` or `other`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Edge {
    pub(crate) from: String,
    pub(crate) to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
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
        /// `bar`, `line` or `pie`.
        chart: String,
        /// A bar chart's series stacked in one bar per x rather than side by side.
        #[serde(default, skip_serializing_if = "is_false")]
        stacked: bool,
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
    /// Bars over time, one row each (a timeline or Gantt chart).
    Schedule {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        rows: Vec<Span>,
    },
    /// Nodes and the edges between them, laid out by the app.
    Graph {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        nodes: Vec<Node>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        edges: Vec<Edge>,
        /// `layered` (the default) or `tree`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<String>,
    },
    /// A drawing: SVG within [`svg::sanitize`]'s allow-list, held in its canonical form.
    Svg {
        title: String,
        svg: String,
    },
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

impl ViewLimits {
    fn short(&self, what: &str, text: &str) -> Result<(), String> {
        short_within(what, text, self.short_bytes)
    }

    fn scalar(&self, what: &str, value: &Value) -> Result<(), String> {
        match value {
            Value::Null | Value::Bool(_) => Ok(()),
            Value::Number(number) if number.as_f64().is_some_and(f64::is_finite) => Ok(()),
            Value::String(text) => self.short(what, text),
            _ => Err(format!(
                "{what} must be text, a number, true, false or null"
            )),
        }
    }

    fn pairs(&self, items: &[Pair]) -> Result<(), String> {
        for item in items {
            self.short("a key", &item.key)?;
            self.scalar("a value", &item.value)?;
        }
        Ok(())
    }
}

/// A link to a page of the app: an app route in its link form (`app:/<page>?...`).
fn app_link(
    what: &str,
    link: &str,
    limits: &ViewLimits,
    route: routes::Limits,
) -> Result<(), String> {
    if !link.starts_with("app:/") {
        return Err(format!(
            "{what} must point into the app (app:/...); outside links are not shown"
        ));
    }
    limits.short(what, link)?;
    Route::parse_uri(link, route)
        .map(|_| ())
        .map_err(|reason| format!("{what} is not an app route: {reason}"))
}

/// Unix seconds of an RFC 3339 time (`2026-10-29T08:00:00Z`, `2026-10-29T08:00:00.5+07:00`),
/// or `None` when `text` is not one.
pub(crate) fn rfc3339(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    if !text.is_ascii()
        || bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let digits = |from: usize, to: usize| -> Option<i64> {
        let part = &text[from..to];
        if part.bytes().all(|byte| byte.is_ascii_digit()) {
            part.parse().ok()
        } else {
            None
        }
    };
    let (year, month, day) = (digits(0, 4)?, digits(5, 7)?, digits(8, 10)?);
    let (hour, minute, second) = (digits(11, 13)?, digits(14, 16)?, digits(17, 19)?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day < 1 || day > days_in_month || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut at = 19;
    let mut fraction = 0.0;
    if bytes[at] == b'.' {
        let end = text[at + 1..]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(text.len(), |end| at + 1 + end);
        if end == at + 1 {
            return None;
        }
        fraction = format!("0{}", &text[at..end]).parse().ok()?;
        at = end;
    }
    let zone = &text[at..];
    let offset = match zone {
        "Z" | "z" => 0,
        _ if zone.len() == 6 && matches!(bytes[at], b'+' | b'-') && bytes[at + 3] == b':' => {
            let (hours, minutes) = (digits(at + 1, at + 3)?, digits(at + 4, at + 6)?);
            if hours > 23 || minutes > 59 {
                return None;
            }
            let sign = if bytes[at] == b'-' { -1 } else { 1 };
            sign * (hours * 3600 + minutes * 60)
        }
        _ => return None,
    };
    // Days since 1970-01-01 of the civil date (Howard Hinnant's `days_from_civil`).
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    #[allow(clippy::cast_precision_loss)]
    Some(seconds as f64 + fraction)
}

/// A schedule's time, an RFC 3339 string or a finite number: whether it is a string, and its
/// value (Unix seconds for a string).
fn moment(value: &Value) -> Result<(bool, f64), String> {
    match value {
        Value::String(text) => rfc3339(text)
            .map(|at| (true, at))
            .ok_or_else(|| format!("{text:?} is not an RFC 3339 time")),
        Value::Number(number) => number
            .as_f64()
            .filter(|at| at.is_finite())
            .map(|at| (false, at))
            .ok_or_else(|| "a time is a finite number".to_owned()),
        _ => Err("start and end are RFC 3339 times or numbers".to_owned()),
    }
}

/// Every Markdown link is an app route (`app:/<page>?...`).
fn links_stay_in_the_app(markdown: &str, route: routes::Limits) -> Result<(), String> {
    let mut rest = markdown;
    while let Some(at) = rest.find("](") {
        let target = &rest[at + 2..];
        if !target.starts_with("app:/") {
            return Err(
                "a link must point into the app (app:/...); outside links are not shown".to_owned(),
            );
        }
        let end = target.find(')').unwrap_or(target.len());
        Route::parse_uri(&target[..end], route)
            .map_err(|reason| format!("a link is not an app route: {reason}"))?;
        rest = target;
    }
    Ok(())
}

/// The page a card opens: a Mission on Missions, a Station on Stations, a Theater on the
/// Stations page's Theaters tab (a card names no Station, which selecting a Theater needs).
fn card_route(
    card: &str,
    id: &str,
    operation_id: Option<&str>,
    route: routes::Limits,
) -> Result<Route, String> {
    let json = match card {
        "mission" => serde_json::json!({
            "page": "missions", "operation_id": operation_id, "mission_id": id
        }),
        "station" => serde_json::json!({"page": "stations", "station_id": id}),
        _ => serde_json::json!({"page": "stations", "tab": "theaters"}),
    };
    Route::from_json(&json, route)
}

impl View {
    /// Checks every bound and puts every SVG in its canonical form; links and cards are
    /// routes within `route`. The reason names the first bound broken.
    pub(crate) fn validate(
        &mut self,
        limits: &ViewLimits,
        route: routes::Limits,
    ) -> Result<(), String> {
        if self.version != VIEW_VERSION {
            return Err(format!("version must be {VIEW_VERSION}"));
        }
        if let Some(title) = &self.title {
            limits.short("title", title)?;
        }
        if self.blocks.is_empty() || self.blocks.len() > limits.blocks {
            return Err(format!("a view has 1 to {} blocks", limits.blocks));
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
        for (index, block) in self.blocks.iter_mut().enumerate() {
            block
                .validate(limits, route)
                .map_err(|reason| format!("block {index}: {reason}"))?;
        }
        let size = serde_json::to_vec(self).map_or(usize::MAX, |bytes| bytes.len());
        if size > limits.view_bytes {
            return Err(format!("a view is at most {} bytes", limits.view_bytes));
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
    fn validate(&mut self, limits: &ViewLimits, route: routes::Limits) -> Result<(), String> {
        match self {
            Self::Text { markdown } => {
                if markdown.trim().is_empty() || markdown.len() > limits.text_bytes {
                    return Err(format!("text is 1 to {} bytes", limits.text_bytes));
                }
                links_stay_in_the_app(markdown, route)
            }
            Self::Table { columns, rows } => {
                if columns.is_empty() || columns.len() > limits.columns {
                    return Err(format!("a table has 1 to {} columns", limits.columns));
                }
                for column in columns.iter() {
                    limits.short("a column", column)?;
                }
                if rows.len() > limits.rows {
                    return Err(format!("a table has at most {} rows", limits.rows));
                }
                for row in rows {
                    if row.len() != columns.len() {
                        return Err("every row has one cell per column".to_owned());
                    }
                    for cell in row {
                        limits.scalar("a cell", cell)?;
                    }
                }
                Ok(())
            }
            Self::KeyValues { items } => {
                if items.is_empty() || items.len() > limits.items {
                    return Err(format!("a key-value list has 1 to {} items", limits.items));
                }
                limits.pairs(items)
            }
            Self::Chart {
                chart,
                stacked,
                title,
                x_label,
                y_label,
                series,
            } => chart_bounds(chart, *stacked, [title, x_label, y_label], series, limits),
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
                card_route(card, id, operation_id.as_deref(), route)?;
                if let Some(state) = state {
                    limits.short("state", state)?;
                }
                if fields.len() > limits.fields {
                    return Err(format!("a card has at most {} fields", limits.fields));
                }
                limits.pairs(fields)
            }
            Self::Timeline { items } => {
                if items.is_empty() || items.len() > limits.items {
                    return Err(format!("a timeline has 1 to {} items", limits.items));
                }
                for item in items {
                    if !item.at.is_finite() || item.at < 0.0 {
                        return Err("at is Unix seconds".to_owned());
                    }
                    limits.short("a label", &item.label)?;
                    if let Some(detail) = &item.detail {
                        limits.short("a detail", detail)?;
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
                    limits.short("title", title)?;
                }
                if before.len() > limits.diff_bytes || after.len() > limits.diff_bytes {
                    return Err(format!(
                        "each side of a diff is at most {} bytes",
                        limits.diff_bytes
                    ));
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
                limits.short("title", title)?;
                if fields.is_empty() || fields.len() > limits.fields {
                    return Err(format!("a form has 1 to {} fields", limits.fields));
                }
                for value in fixed.values() {
                    if serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
                        > limits.text_bytes
                    {
                        return Err("a fixed value is too long".to_owned());
                    }
                }
                for field in fields {
                    identifier("a field name", &field.name)?;
                    limits.short("a field label", &field.label)?;
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
                        limits.short("a choice", choice)?;
                    }
                }
                Ok(())
            }
            Self::Schedule { title, rows } => {
                schedule_bounds(title.as_deref(), rows, limits, route)
            }
            Self::Graph {
                title,
                nodes,
                edges,
                layout,
            } => graph_bounds(
                title.as_deref(),
                nodes,
                edges,
                layout.as_deref(),
                limits,
                route,
            ),
            Self::Svg { title, svg: text } => {
                limits.short("title", title)?;
                if title.trim().is_empty() {
                    return Err("an SVG has a title".to_owned());
                }
                *text = svg::sanitize(text, limits)?;
                Ok(())
            }
        }
    }
}

fn chart_bounds(
    chart: &str,
    stacked: bool,
    labels: [&Option<String>; 3],
    series: &[Series],
    limits: &ViewLimits,
) -> Result<(), String> {
    if !matches!(chart, "bar" | "line" | "pie") {
        return Err("chart is bar, line or pie".to_owned());
    }
    if stacked && chart != "bar" {
        return Err("only a bar chart is stacked".to_owned());
    }
    for text in labels.into_iter().flatten() {
        limits.short("a chart label", text)?;
    }
    if series.is_empty() || series.len() > limits.series {
        return Err(format!("a chart has 1 to {} series", limits.series));
    }
    for line in series {
        limits.short("a series name", &line.name)?;
        if line.points.is_empty() || line.points.len() > limits.points {
            return Err(format!("a series has 1 to {} points", limits.points));
        }
        let mut seen = HashSet::new();
        for point in &line.points {
            limits.scalar("x", &point.x)?;
            if !point.y.is_finite() {
                return Err("y must be a finite number".to_owned());
            }
            // Bars and slices are placed by their x, so a series names each x once.
            if chart != "line" && !seen.insert(point.x.to_string()) {
                return Err(format!("a series names each x once, not {} twice", point.x));
            }
        }
    }
    if chart == "pie" {
        let [only] = series else {
            return Err("a pie has one series".to_owned());
        };
        if only.points.len() > limits.pie_slices {
            return Err(format!("a pie has at most {} slices", limits.pie_slices));
        }
        if only.points.iter().any(|point| point.y < 0.0) {
            return Err("a pie's values are 0 or more".to_owned());
        }
        if only.points.iter().all(|point| point.y == 0.0) {
            return Err("a pie has a value above 0".to_owned());
        }
    }
    if stacked
        && series
            .iter()
            .flat_map(|line| &line.points)
            .any(|point| point.y < 0.0)
    {
        return Err("a stacked bar's values are 0 or more".to_owned());
    }
    Ok(())
}

fn schedule_bounds(
    title: Option<&str>,
    rows: &[Span],
    limits: &ViewLimits,
    route: routes::Limits,
) -> Result<(), String> {
    if let Some(title) = title {
        limits.short("title", title)?;
    }
    if rows.is_empty() || rows.len() > limits.schedule_rows {
        return Err(format!("a schedule has 1 to {} rows", limits.schedule_rows));
    }
    let mut times = None;
    for row in rows {
        limits.short("a label", &row.label)?;
        if let Some(state) = &row.state {
            limits.short("state", state)?;
        }
        if let Some(link) = &row.link {
            app_link("a link", link, limits, route)?;
        }
        let (start_is_time, start) = moment(&row.start)?;
        let (end_is_time, end) = moment(&row.end)?;
        if start_is_time != end_is_time || *times.get_or_insert(start_is_time) != start_is_time {
            return Err("a schedule's times are all RFC 3339 times or all numbers".to_owned());
        }
        if start >= end {
            return Err(format!("every row starts before it ends ({:?})", row.label));
        }
    }
    Ok(())
}

fn graph_bounds(
    title: Option<&str>,
    nodes: &[Node],
    edges: &[Edge],
    layout: Option<&str>,
    limits: &ViewLimits,
    route: routes::Limits,
) -> Result<(), String> {
    if let Some(title) = title {
        limits.short("title", title)?;
    }
    if !matches!(layout, None | Some("layered" | "tree")) {
        return Err("layout is layered or tree".to_owned());
    }
    if nodes.is_empty() || nodes.len() > limits.graph_nodes {
        return Err(format!("a graph has 1 to {} nodes", limits.graph_nodes));
    }
    if edges.len() > limits.graph_edges {
        return Err(format!("a graph has at most {} edges", limits.graph_edges));
    }
    let mut ids = HashSet::new();
    for node in nodes {
        identifier("a node id", &node.id)?;
        if !ids.insert(node.id.as_str()) {
            return Err(format!("the node id {:?} is used twice", node.id));
        }
        limits.short("a node label", &node.label)?;
        if let Some(kind) = &node.kind
            && !matches!(
                kind.as_str(),
                "operation" | "station" | "theater" | "mission" | "session" | "agent" | "other"
            )
        {
            return Err(
                "a node's kind is operation, station, theater, mission, session, agent or other"
                    .to_owned(),
            );
        }
        if let Some(state) = &node.state {
            limits.short("state", state)?;
        }
        if let Some(link) = &node.link {
            app_link("a link", link, limits, route)?;
        }
    }
    let mut parents: HashMap<&str, &str> = HashMap::new();
    for edge in edges {
        for end in [&edge.from, &edge.to] {
            if !ids.contains(end.as_str()) {
                return Err(format!("an edge names an unknown node {end:?}"));
            }
        }
        if edge.from == edge.to {
            return Err(format!(
                "an edge joins two nodes, not {:?} to itself",
                edge.from
            ));
        }
        if let Some(label) = &edge.label {
            limits.short("an edge label", label)?;
        }
        if layout == Some("tree") && parents.insert(&edge.to, &edge.from).is_some() {
            return Err(format!(
                "in a tree each node has at most one parent; {:?} has two",
                edge.to
            ));
        }
    }
    // With one parent each, a walk up from every node ends at a root unless it loops.
    for start in &ids {
        let mut at = *start;
        for _ in 0..nodes.len() {
            match parents.get(at) {
                Some(parent) => at = parent,
                None => break,
            }
        }
        if parents.contains_key(at) {
            return Err("a tree has no cycle".to_owned());
        }
    }
    // The tree layout places the descendants of the first root only.
    let roots = nodes
        .iter()
        .filter(|node| !parents.contains_key(node.id.as_str()))
        .count();
    if layout == Some("tree") && roots != 1 {
        return Err(format!("a tree has one root, not {roots}"));
    }
    Ok(())
}

/// Parses and checks the `view` input of `show`; its links and cards are routes within
/// `route`.
pub(crate) fn parse(
    input: &Value,
    limits: &ViewLimits,
    route: routes::Limits,
) -> Result<View, String> {
    let object = input
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get("view"))
        .ok_or("show takes exactly {\"view\": {...}}")?;
    let mut view: View =
        serde_json::from_value(object.clone()).map_err(|error| error.to_string())?;
    view.validate(limits, route)?;
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
pub(crate) fn schema(limits: &ViewLimits) -> Value {
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
    let time = json!({"type": ["string", "number"]});
    let span = json!({
        "type": "object", "additionalProperties": false, "required": ["label", "start", "end"],
        "properties": {"label": {"type": "string"}, "start": time, "end": time,
                       "state": {"type": "string"}, "link": {"type": "string"}}
    });
    let node = json!({
        "type": "object", "additionalProperties": false, "required": ["id", "label"],
        "properties": {
            "id": {"type": "string"}, "label": {"type": "string"},
            "kind": {"enum": ["operation", "station", "theater", "mission", "session", "agent",
                              "other"]},
            "state": {"type": "string"}, "link": {"type": "string"}
        }
    });
    let edge = json!({
        "type": "object", "additionalProperties": false, "required": ["from", "to"],
        "properties": {"from": {"type": "string"}, "to": {"type": "string"},
                       "label": {"type": "string"}}
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
                "chart": {"enum": ["bar", "line", "pie"]}, "stacked": {"type": "boolean"},
                "title": {"type": "string"},
                "x_label": {"type": "string"}, "y_label": {"type": "string"},
                "series": {"type": "array", "items": series}
            }),
        ),
        block(
            "schedule",
            &["rows"],
            json!({
                "title": {"type": "string"},
                "rows": {"type": "array", "maxItems": limits.schedule_rows, "items": span}
            }),
        ),
        block(
            "graph",
            &["nodes"],
            json!({
                "title": {"type": "string"},
                "nodes": {"type": "array", "maxItems": limits.graph_nodes, "items": node},
                "edges": {"type": "array", "maxItems": limits.graph_edges, "items": edge},
                "layout": {"enum": ["layered", "tree"]}
            }),
        ),
        block(
            "svg",
            &["title", "svg"],
            json!({
                "title": {"type": "string"},
                "svg": {"type": "string", "maxLength": limits.svg_bytes}
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
            "blocks": {"type": "array", "maxItems": limits.blocks, "items": {"oneOf": blocks}}
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

    fn parse(input: &Value) -> Result<View, String> {
        super::parse(input, &ViewLimits::default(), routes::Limits::default())
    }

    /// One view of every kind, as Moneypenny would show it; the app's widget tests read the
    /// same file.
    pub(crate) fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../../clients/outer-office/test/fixtures/assistant_views.json"
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
    fn an_svg_is_held_in_canonical_form_which_parses_again_unchanged() {
        let messy = "<?xml version=\"1.0\"?>\n<svg viewBox=\"0 0 10 10\">\n  <!-- a note -->\n  \
                     <title>t</title>\n  <rect width=\"5\" height=\"5\"></rect>\n</svg>";
        let view = parse(&json!({"view": {"version": 1, "blocks": [
            {"kind": "svg", "title": "t", "svg": messy}
        ]}}))
        .unwrap();
        let Block::Svg { svg, .. } = &view.blocks[0] else {
            panic!("{view:?}")
        };
        assert_eq!(
            svg,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><title>t</title>\
             <rect width=\"5\" height=\"5\"/></svg>"
        );
        let again = parse(&json!({"view": serde_json::to_value(&view).unwrap()})).unwrap();
        assert_eq!(again, view);
        // The fixture's drawing is already canonical, so the app is sent it as written.
        let fixture = fixture();
        let shown = parse(&json!({"view": fixture["valid"]["svg"]})).unwrap();
        assert_eq!(
            serde_json::to_value(&shown).unwrap(),
            fixture["valid"]["svg"]
        );
    }

    #[test]
    fn rfc3339_times_are_read_as_unix_seconds() {
        assert_eq!(rfc3339("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800.0));
        assert_eq!(rfc3339("2000-03-01t00:00:00.25z"), Some(951_868_800.25));
        assert_eq!(
            rfc3339("2026-10-29T09:10:00+01:00"),
            rfc3339("2026-10-29T08:10:00Z")
        );
        assert_eq!(
            rfc3339("1969-12-31T19:00:00-05:00"),
            Some(0.0),
            "an offset west of UTC"
        );
        for refused in [
            "2026-02-29T00:00:00Z",
            "2026-10-29 08:00:00Z",
            "2026-10-29T08:00:00",
            "2026-10-29T24:00:00Z",
            "2026-10-29T08:00:00.Z",
            "2026-10-29T08:00:00+0100",
            "2026-1०-29T08:00:00Z",
        ] {
            assert_eq!(rfc3339(refused), None, "{refused}");
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
