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

/// Turns Markdown into an ADF document: headings, bullet and numbered lists, fenced code
/// blocks, `[text](url)` links on `http` and `https` addresses, and one paragraph per other
/// line (a blank line is an empty paragraph). Everything else, raw HTML included, stays
/// literal text.
pub(crate) fn document(body: &str) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(rest) = line.trim_start().strip_prefix("```") {
            let language = rest.trim();
            let mut code = Vec::new();
            for next in lines.by_ref() {
                if next.trim_start().starts_with("```") {
                    break;
                }
                code.push(next);
            }
            let mut block = json!({ "type": "codeBlock" });
            if !language.is_empty() {
                block["attrs"] = json!({ "language": language });
            }
            if !code.is_empty() {
                block["content"] = json!([{ "type": "text", "text": code.join("\n") }]);
            }
            blocks.push(block);
        } else if let Some((level, text)) = heading(line) {
            blocks.push(json!({
                "type": "heading",
                "attrs": { "level": level },
                "content": inline(text),
            }));
        } else if let Some((ordered, text)) = list_item(line) {
            let mut items = vec![item(text)];
            while let Some(next) = lines.peek().and_then(|next| list_item(next)) {
                if next.0 != ordered {
                    break;
                }
                items.push(item(next.1));
                lines.next();
            }
            blocks.push(json!({
                "type": if ordered { "orderedList" } else { "bulletList" },
                "content": items,
            }));
        } else if line.is_empty() {
            blocks.push(json!({ "type": "paragraph" }));
        } else {
            blocks.push(json!({ "type": "paragraph", "content": inline(line) }));
        }
    }
    json!({ "type": "doc", "version": 1, "content": blocks })
}

/// A `#` to `######` heading line: its level and text.
fn heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    let text = line[hashes..].strip_prefix(' ')?;
    ((1..=6).contains(&hashes) && !text.trim().is_empty()).then(|| (hashes, text.trim()))
}

/// A `- `, `* ` or `1. ` list line: whether it is numbered, and its text.
fn list_item(line: &str) -> Option<(bool, &str)> {
    let line = line.trim_start();
    if let Some(text) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some((false, text));
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    let text = line[digits..].strip_prefix(". ")?;
    (digits > 0).then_some((true, text))
}

fn item(text: &str) -> Value {
    json!({
        "type": "listItem",
        "content": [{ "type": "paragraph", "content": inline(text) }],
    })
}

/// Text with `[label](url)` links as link marks; any other character is literal.
fn inline(text: &str) -> Vec<Value> {
    let mut nodes = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let parsed = rest[start + 1..]
            .split_once("](")
            .and_then(|(label, tail)| {
                let (address, after) = tail.split_once(')')?;
                let web = address.starts_with("https://") || address.starts_with("http://");
                (web && !label.is_empty() && !label.contains('[') && !address.contains(' '))
                    .then_some((label, address, after))
            });
        match parsed {
            Some((label, address, after)) => {
                plain.push_str(&rest[..start]);
                if !plain.is_empty() {
                    nodes.push(json!({ "type": "text", "text": std::mem::take(&mut plain) }));
                }
                nodes.push(json!({
                    "type": "text",
                    "text": label,
                    "marks": [{ "type": "link", "attrs": { "href": address } }],
                }));
                rest = after;
            }
            None => {
                plain.push_str(&rest[..=start]);
                rest = &rest[start + 1..];
            }
        }
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        nodes.push(json!({ "type": "text", "text": plain }));
    }
    nodes
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

    #[test]
    fn markdown_blocks_become_adf_blocks() {
        let doc = document(
            "## Plan\n- one\n- two\n1. first\nSee [the PR](https://example.invalid/pr/1).\n\
             ```rust\nfn main() {}\n```",
        );
        let kinds: Vec<&str> = doc["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| block["type"].as_str().unwrap())
            .collect();
        assert_eq!(
            kinds,
            [
                "heading",
                "bulletList",
                "orderedList",
                "paragraph",
                "codeBlock"
            ]
        );
        assert_eq!(doc["content"][0]["attrs"]["level"], 2);
        assert_eq!(doc["content"][1]["content"].as_array().unwrap().len(), 2);
        let link = &doc["content"][3]["content"][1];
        assert_eq!(link["text"], "the PR");
        assert_eq!(
            link["marks"][0]["attrs"]["href"],
            "https://example.invalid/pr/1"
        );
        assert_eq!(doc["content"][4]["attrs"]["language"], "rust");
        assert_eq!(doc["content"][4]["content"][0]["text"], "fn main() {}");
    }

    #[test]
    fn html_and_unsafe_links_stay_literal_text() {
        let doc = document("<script>x</script> [a](javascript:alert(1)) [b](https://ok.invalid)");
        let content = doc["content"][0]["content"].as_array().unwrap();
        assert_eq!(
            content[0]["text"],
            "<script>x</script> [a](javascript:alert(1)) "
        );
        assert_eq!(
            content[1]["marks"][0]["attrs"]["href"],
            "https://ok.invalid"
        );
        assert_eq!(doc.to_string().matches("\"link\"").count(), 1);
    }
}
