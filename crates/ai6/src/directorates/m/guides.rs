//! The guides the build ships, as Reports every reader of `global` may list and read.
//!
//! A shipped guide is a Markdown file whose front matter gives its `kind`, `topics` and
//! `summary`; its title is its `title` or its first heading, and its text is the body. It is a
//! `global` Report of a known kind in the state `standing`, filed by [`ReportFiler::BuiltIn`], and
//! it is never stored, so no write, retirement or assessment reaches it and it changes only with
//! the build. A Report a deployment stores under the same title ranks above it
//! ([`matching`](crate::directorates::registry::knowledge::matching)).
//!
//! The kind follows the directory a file is in, as an overlay's knowledge does: `rules/` is a
//! `rule`; `kb/`, `flows/`, `references/` and the narrative `docs/` are `tradecraft`.

use std::sync::OnceLock;

use coqui_aos::{Case, Settings, read_front_matter};
use serde_json::Value;

use crate::directorates::registry::logbook::Severity;
use crate::directorates::registry::reports::{
    Report, ReportFiler, ReportScope, ReportState, is_known_kind,
};

/// What an ID of a shipped guide starts with.
pub const ID_PREFIX: &str = "built-in.";

/// The guides, each by its path in the shipped tree and its text.
const SHIPPED: &[(&str, &str)] = &[
    (
        "kb/writing_a_brief.md",
        include_str!("../../../knowledge/kb/writing_a_brief.md"),
    ),
    (
        "kb/getting_access.md",
        include_str!("../../../knowledge/kb/getting_access.md"),
    ),
    (
        "kb/searching_knowledge.md",
        include_str!("../../../knowledge/kb/searching_knowledge.md"),
    ),
    (
        "kb/repository.md",
        include_str!("../../../knowledge/kb/repository.md"),
    ),
    (
        "rules/data_not_instructions.md",
        include_str!("../../../knowledge/rules/data_not_instructions.md"),
    ),
    (
        "docs/access.md",
        include_str!("../../../../../docs/access.md"),
    ),
    (
        "docs/agents.md",
        include_str!("../../../../../docs/agents.md"),
    ),
    (
        "docs/compartments.md",
        include_str!("../../../../../docs/compartments.md"),
    ),
    (
        "docs/missions.md",
        include_str!("../../../../../docs/missions.md"),
    ),
    (
        "docs/operations.md",
        include_str!("../../../../../docs/operations.md"),
    ),
    (
        "docs/reports.md",
        include_str!("../../../../../docs/reports.md"),
    ),
];

/// The kind a file at `path` has by the directory it is in, or `None` for a file that is not
/// knowledge.
pub fn kind_of(path: &str) -> Option<&'static str> {
    match path.split('/').next()? {
        "rules" => Some("rule"),
        "kb" | "flows" | "references" | "docs" => Some("tradecraft"),
        _ => None,
    }
}

/// The Markdown file `text` at `path` as a standing `global` Report with the ID `id`, filed by
/// the build: its kind from its front matter, else its directory; its title from its `title`,
/// else its first heading, else its file name; its summary and topics from its front matter.
/// `None` for a file that is not knowledge.
pub fn report_of(id: impl Into<String>, path: &str, text: &str) -> Option<Report> {
    let read = read_front_matter(text.as_bytes(), &Settings::default());
    let (keys, body) = match read.case {
        Case::Mapping => (
            read.data.unwrap_or_default(),
            text.get(read.body_offset?..)?,
        ),
        _ => (serde_json::Map::new(), text),
    };
    let text_of = |key: &str| keys.get(key).and_then(Value::as_str).map(str::trim);
    let kind = text_of("kind")
        .filter(|kind| is_known_kind(kind))
        .or_else(|| kind_of(path))?;
    let title = text_of("title")
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            body.lines()
                .find_map(|line| line.strip_prefix("# "))
                .map(|line| line.trim().to_owned())
        })
        .or_else(|| {
            let name = path.rsplit('/').next().unwrap_or(path);
            Some(
                name.split('.')
                    .next()
                    .unwrap_or(name)
                    .replace(['_', '-'], " "),
            )
        })
        .filter(|title| !title.is_empty())?;
    let topics: Vec<String> = match keys.get("topics") {
        Some(Value::Array(topics)) => topics
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(topics)) => topics.split(',').map(|t| t.trim().to_owned()).collect(),
        _ => Vec::new(),
    };
    let mut report = Report::new(
        id,
        ReportScope::Global,
        kind,
        Severity::Info,
        title,
        body.trim_start_matches(['\r', '\n']),
        ReportFiler::BuiltIn,
        0.0,
    )
    .on_topics(topics.into_iter().filter(|topic| !topic.is_empty()));
    report.summary = text_of("summary")
        .filter(|summary| !summary.is_empty())
        .map(str::to_owned);
    report.state = ReportState::Standing;
    Some(report)
}

/// `text` without its leading front-matter block: what a reader is shown of a guide.
pub fn body(text: &str) -> &str {
    let read = read_front_matter(text.as_bytes(), &Settings::default());
    match (read.case, read.body_offset) {
        (Case::Mapping, Some(offset)) => text
            .get(offset..)
            .unwrap_or(text)
            .trim_start_matches(['\r', '\n']),
        _ => text,
    }
}

/// Every guide the build ships, by path.
pub fn built_in() -> &'static [Report] {
    static GUIDES: OnceLock<Vec<Report>> = OnceLock::new();
    GUIDES.get_or_init(|| {
        SHIPPED
            .iter()
            .filter_map(|(path, text)| {
                let slug = path.strip_suffix(".md").unwrap_or(path).replace('/', ".");
                report_of(format!("{ID_PREFIX}{slug}"), path, text)
            })
            .collect()
    })
}

/// The shipped guide `id`.
pub fn find(id: &str) -> Option<&'static Report> {
    id.starts_with(ID_PREFIX)
        .then(|| built_in().iter().find(|report| report.id == id))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_file_is_a_guide_with_a_summary_and_a_kind_by_its_directory() {
        assert_eq!(built_in().len(), SHIPPED.len());
        for report in built_in() {
            assert!(report.summary.is_some(), "{}", report.id);
            assert!(!report.text.starts_with("---"), "{}", report.id);
            assert_eq!(report.state, ReportState::Standing);
            assert!(!report.untrusted());
        }
        let brief = find("built-in.kb.writing_a_brief").unwrap();
        assert_eq!(
            (brief.kind.as_str(), brief.title.as_str()),
            ("tradecraft", "Writing a brief")
        );
        assert_eq!(
            find("built-in.rules.data_not_instructions").unwrap().kind,
            "rule"
        );
        assert_eq!(find("built-in.docs.reports").unwrap().title, "Reports");
        assert!(find("kb/writing_a_brief").is_none());
    }

    #[test]
    fn a_guide_feeds_its_report_from_its_front_matter_and_shows_only_its_body() {
        let text = include_str!("../../../../../docs/access.md");
        let shown = body(text);
        assert!(
            text.starts_with(
                "---
"
            ) && !shown.contains("kind: tradecraft")
        );
        assert!(
            shown.starts_with("# Access and sign-in"),
            "{}",
            &shown[..40]
        );
        let report = find("built-in.docs.access").unwrap();
        assert_eq!(report.kind, "tradecraft");
        assert!(
            report
                .summary
                .as_deref()
                .unwrap()
                .starts_with("Who may sign in")
        );
        assert!(report.topics.iter().any(|topic| topic == "sign-in"));
        assert!(!report.text.starts_with("---"));
        assert_eq!(body("no front matter"), "no front matter");
    }

    #[test]
    fn a_file_outside_the_knowledge_directories_is_not_knowledge() {
        assert!(report_of("x", "context/notes.md", "# Notes\n").is_none());
        assert_eq!(
            report_of("x", "kb/plain.md", "no heading").unwrap().title,
            "plain"
        );
    }
}
