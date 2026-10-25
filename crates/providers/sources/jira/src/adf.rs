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
        "inlineCard" => out.push_str(
            node.pointer("/attrs/url")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
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
    fn a_comment_document_keeps_one_paragraph_per_line() {
        let doc = document("a\n\nb");
        assert_eq!(doc["content"].as_array().unwrap().len(), 3);
        assert_eq!(text(&doc), "a\n\nb");
    }
}
