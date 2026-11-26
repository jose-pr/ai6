//! A Report read back from its Markdown document: the front matter is its fields, the body its
//! text.
//!
//! The gateway writes the document (`GET /v1/reports/{id}/document`); this reads one again, from
//! a file a person wrote or one exported earlier. It reads the form, not the meaning: which keys
//! may be set, which only repeat what the gateway owns, and what a change is allowed to touch is
//! the gateway's to decide.

use coqui_aos::{Case, Settings, read_front_matter};
use serde_json::{Map, Value};

/// A Report document: its front matter as fields and its body as text.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportDocument {
    /// The front matter's keys, without `author`.
    pub fields: Map<String, Value>,
    /// The text: the body, led by the author's own front matter when the export kept it under
    /// `author`.
    pub text: String,
}

/// Reads `markdown` as a Report document.
///
/// A body that opened with front matter of its own was exported as the front matter key `author`
/// (the author's words, as text); it is put back in front of the body, so the text is what was
/// filed.
///
/// # Errors
///
/// Why it is not a Report document: no front matter, front matter that does not read as a
/// mapping, a body cut short (`truncated: true`), or an `author` that is not text.
pub fn read(markdown: &str) -> Result<ReportDocument, String> {
    let read = read_front_matter(markdown.as_bytes(), &Settings::default());
    if read.case != Case::Mapping {
        return Err(match read.error {
            Some(error) => format!("the front matter does not read: {error}"),
            None => "a Report document opens with front matter between --- lines".to_owned(),
        });
    }
    let mut fields = read.data.unwrap_or_default();
    let offset = read
        .body_offset
        .filter(|offset| markdown.is_char_boundary(*offset))
        .unwrap_or(markdown.len());
    let body = &markdown[offset..];
    // One blank line separates the front matter from the body.
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
        .unwrap_or(body);
    if fields.remove("truncated") == Some(Value::Bool(true)) {
        return Err("this document was cut short; its text is not all there".to_owned());
    }
    let text = match fields.remove("author") {
        None => body.to_owned(),
        Some(Value::String(author)) => {
            format!("---\n{}\n---\n{body}", author.trim_end_matches('\n'))
        }
        Some(_) => return Err("author is the body's own front matter, as text".to_owned()),
    };
    Ok(ReportDocument { fields, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_matter_is_the_fields_and_the_body_the_text() {
        let document = read(
            "---\nkind: \"tradecraft\"\ntopics: [\"a\",\"b\"]\nrevision: 3\n---\n\nFirst.\n\nSecond.",
        )
        .unwrap();
        assert_eq!(document.fields["kind"], "tradecraft");
        assert_eq!(document.fields["revision"], 3);
        assert_eq!(document.fields["topics"][1], "b");
        assert_eq!(document.text, "First.\n\nSecond.");
    }

    #[test]
    fn the_authors_own_front_matter_goes_back_in_front_of_the_body() {
        let document =
            read("---\nkind: \"memory\"\nauthor: |2\n  tags: x\n\n  more: y\n---\n\nBody").unwrap();
        assert!(!document.fields.contains_key("author"));
        assert_eq!(document.text, "---\ntags: x\n\nmore: y\n---\nBody");
    }

    #[test]
    fn what_is_not_a_whole_report_document_is_refused() {
        assert!(read("no front matter").is_err());
        assert!(read("---\nkind: [\n---\n\nx").is_err());
        assert!(read("---\ntruncated: true\n---\n\nx").is_err());
        assert!(read("---\nauthor: 3\n---\n\nx").is_err());
    }
}
