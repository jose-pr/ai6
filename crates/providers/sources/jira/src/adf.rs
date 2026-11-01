//! Atlassian Document Format: Jira Cloud's rich text, reduced to plain text and back.

use serde_json::{Value, json};

/// Flattens an ADF document to plain text: one line per block, line breaks kept, links and
/// mentions as their visible text.
pub(crate) fn text(document: &Value) -> String {
    let mut out = String::new();
    walk(document, &mut out);
    out.trim_end().to_owned()
}

fn walk(node: &Value, out: &mut String) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "text" => out.push_str(node.get("text").and_then(Value::as_str).unwrap_or_default()),
        "hardBreak" => out.push('\n'),
        "mention" => out.push_str(
            node.pointer("/attrs/text")
                .and_then(Value::as_str)
                .unwrap_or("@someone"),
        ),
        "inlineCard" | "blockCard" | "embedCard" => out.push_str(
            node.pointer("/attrs/url")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        "emoji" => out.push_str(
            node.pointer("/attrs/text")
                .or_else(|| node.pointer("/attrs/shortName"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        "status" => out.push_str(
            node.pointer("/attrs/text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        "date" => {
            if let Some(day) = node
                .pointer("/attrs/timestamp")
                .and_then(Value::as_str)
                .and_then(utc_day)
            {
                out.push_str(&day);
            }
        }
        _ => {
            if kind == "listItem" {
                out.push_str("- ");
            }
            for child in node
                .get("content")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                walk(child, out);
            }
            let block = matches!(
                kind,
                "paragraph" | "heading" | "codeBlock" | "listItem" | "blockquote" | "rule"
            );
            // A paragraph always ends its line, so an empty one is a blank line; a container
            // such as a list item only ends one its last child has not already ended.
            if block && (kind == "paragraph" || !out.ends_with('\n')) {
                out.push('\n');
            }
        }
    }
}

/// The UTC calendar day of a Unix time in milliseconds, which is how ADF stores a date.
///
/// The arithmetic is the `civil_from_days` algorithm of <https://howardhinnant.github.io/date_algorithms.html>.
fn utc_day(millis: &str) -> Option<String> {
    let shifted = millis.parse::<i64>().ok()?.div_euclid(86_400_000) + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// Wraps plain text as an ADF document with one paragraph per line.
pub(crate) fn document(body: &str) -> Value {
    let paragraphs: Vec<Value> = body
        .lines()
        .map(|line| {
            if line.is_empty() {
                json!({ "type": "paragraph" })
            } else {
                json!({ "type": "paragraph", "content": [{ "type": "text", "text": line }] })
            }
        })
        .collect();
    json!({ "type": "doc", "version": 1, "content": paragraphs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_blocks_flatten_to_lines() {
        let doc = json!({
            "type": "doc", "version": 1,
            "content": [
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "It fails " },
                    { "type": "mention", "attrs": { "text": "@sam" } },
                    { "type": "hardBreak" },
                    { "type": "text", "text": "often." }
                ]},
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [
                        { "type": "paragraph", "content": [{ "type": "text", "text": "one" }] }
                    ]}
                ]}
            ]
        });
        assert_eq!(text(&doc), "It fails @sam\noften.\n- one");
    }

    #[test]
    fn inline_atoms_keep_their_text() {
        let doc = json!({
            "type": "doc", "version": 1,
            "content": [{ "type": "paragraph", "content": [
                { "type": "emoji", "attrs": { "shortName": ":smile:", "text": "😄" } },
                { "type": "text", "text": " " },
                { "type": "emoji", "attrs": { "shortName": ":custom:" } },
                { "type": "text", "text": " " },
                { "type": "status", "attrs": { "text": "IN PROGRESS", "color": "blue" } },
                { "type": "text", "text": " " },
                { "type": "date", "attrs": { "timestamp": "1793491200000" } }
            ]}]
        });
        assert_eq!(text(&doc), "😄 :custom: IN PROGRESS 2026-11-01");
    }

    #[test]
    fn dates_are_utc_days() {
        assert_eq!(utc_day("0").as_deref(), Some("1970-01-01"));
        assert_eq!(utc_day("1793491200000").as_deref(), Some("2026-11-01"));
        assert_eq!(utc_day("1709164800000").as_deref(), Some("2024-02-29"));
        assert_eq!(utc_day("-86400000").as_deref(), Some("1969-12-31"));
        assert_eq!(utc_day("soon"), None);
    }

    #[test]
    fn a_comment_document_keeps_one_paragraph_per_line() {
        let doc = document("a\n\nb");
        assert_eq!(doc["content"].as_array().unwrap().len(), 3);
        assert_eq!(text(&doc), "a\n\nb");
    }
}
