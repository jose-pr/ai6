//! What a Mission is to deliver, declared before the work so what a Session says it did can be
//! checked against something other than its own account.
//!
//! A deliverable is a kind and, for most kinds, a target, written `kind` or `kind:target`
//! (`branch`, `commit_on:main`, `file:docs/report.md`). A role declares the ones all its Missions
//! owe (`deliverables = [...]`) and a brief adds its own. The Station reads them back with the
//! probes in `station::probes`; it never judges whether they are enough.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The key of a brief's `data` that lists its deliverables, an array of strings.
pub const BRIEF_DELIVERABLES_KEY: &str = "deliverables";

/// What kind of thing a deliverable is, and so how it is read back.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliverableKind {
    /// A branch on the remote. The target is its name; none means the Mission's own branch.
    Branch,
    /// A commit of the Mission on a branch of the remote. The target is `BRANCH`, or
    /// `BRANCH@COMMIT` for a commit named outright; without a commit, the Mission's own branch
    /// must be in that branch's history.
    CommitOn,
    /// A comment on a tracker item the Mission is linked to. The target is text the comment
    /// carries.
    Comment,
    /// A page or file served over HTTP. The target is its URL.
    Artifact,
    /// A green ink request decided for the Mission. The target, when given, is the action it
    /// covers.
    Approval,
    /// A file in the Mission's working directory. The target is its path, relative to it.
    File,
}

impl DeliverableKind {
    /// Every kind.
    pub const ALL: [Self; 6] = [
        Self::Branch,
        Self::CommitOn,
        Self::Comment,
        Self::Artifact,
        Self::Approval,
        Self::File,
    ];

    /// The spelling in configuration and on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::CommitOn => "commit_on",
            Self::Comment => "comment",
            Self::Artifact => "artifact",
            Self::Approval => "approval",
            Self::File => "file",
        }
    }

    /// The kind its spelling names.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// Whether a deliverable of this kind must name its target.
    const fn needs_target(self) -> bool {
        matches!(
            self,
            Self::CommitOn | Self::Comment | Self::Artifact | Self::File
        )
    }
}

/// One thing a Mission is to deliver.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Deliverable {
    /// What kind of thing it is.
    pub kind: DeliverableKind,
    /// What names it, for the kinds that take one.
    pub target: Option<String>,
}

/// Why a deliverable could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidDeliverable(String);

impl fmt::Display for InvalidDeliverable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidDeliverable {}

impl Deliverable {
    /// A deliverable of `kind` naming `target`.
    pub fn new(kind: DeliverableKind, target: Option<String>) -> Self {
        Self { kind, target }
    }

    /// Reads `kind` or `kind:target`.
    ///
    /// # Errors
    ///
    /// For an unknown kind, an empty target, a target with control characters, or a kind that
    /// needs a target and has none.
    pub fn parse(text: &str) -> Result<Self, InvalidDeliverable> {
        let (name, target) = match text.split_once(':') {
            Some((name, target)) => (name, Some(target)),
            None => (text, None),
        };
        let kind = DeliverableKind::from_name(name.trim()).ok_or_else(|| {
            InvalidDeliverable(format!(
                "{text:?} is not a deliverable: a kind is one of {}",
                DeliverableKind::ALL.map(DeliverableKind::as_str).join(", ")
            ))
        })?;
        let target = target.map(str::trim);
        if target.is_some_and(|target| target.is_empty() || target.chars().any(char::is_control)) {
            return Err(InvalidDeliverable(format!(
                "{text:?} is not a deliverable: a target is text without control characters"
            )));
        }
        if kind.needs_target() && target.is_none() {
            return Err(InvalidDeliverable(format!(
                "a {} deliverable needs a target, as {}:TARGET",
                kind.as_str(),
                kind.as_str()
            )));
        }
        Ok(Self::new(kind, target.map(str::to_owned)))
    }
}

impl fmt::Display for Deliverable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.target {
            Some(target) => write!(f, "{}:{target}", self.kind.as_str()),
            None => f.write_str(self.kind.as_str()),
        }
    }
}

impl Serialize for Deliverable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Deliverable {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Reads every entry of `texts`.
///
/// # Errors
///
/// The first entry that is not a deliverable.
pub fn parse_all(texts: &[String]) -> Result<Vec<Deliverable>, InvalidDeliverable> {
    texts.iter().map(|text| Deliverable::parse(text)).collect()
}

/// What a Mission owes: its role's deliverables, then its brief's, each once.
///
/// # Errors
///
/// The first one that is not a deliverable, in the brief's `data` or the role.
pub fn declared(
    role: &[String],
    data: &BTreeMap<String, serde_json::Value>,
) -> Result<Vec<Deliverable>, InvalidDeliverable> {
    let mut all = parse_all(role)?;
    if let Some(value) = data.get(BRIEF_DELIVERABLES_KEY) {
        let texts = value
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .map(|item| item.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or_else(|| {
                InvalidDeliverable("a brief's deliverables are a list of text".to_owned())
            })?;
        all.extend(parse_all(&texts)?);
    }
    let mut seen = Vec::new();
    all.retain(|deliverable| {
        let fresh = !seen.contains(deliverable);
        if fresh {
            seen.push(deliverable.clone());
        }
        fresh
    });
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deliverable_is_a_kind_and_a_target() {
        for text in [
            "branch",
            "branch:fix-1",
            "commit_on:main@abc",
            "file:a/b.md",
            "approval",
        ] {
            assert_eq!(Deliverable::parse(text).unwrap().to_string(), text);
        }
        assert_eq!(
            Deliverable::parse("file: x.md ").unwrap().target.as_deref(),
            Some("x.md")
        );
    }

    #[test]
    fn a_malformed_deliverable_is_refused_by_name() {
        for text in [
            "",
            "tag:v1",
            "branch:",
            "file",
            "artifact",
            "comment",
            "file:a\nb",
        ] {
            assert!(Deliverable::parse(text).is_err(), "{text:?}");
        }
        assert!(
            Deliverable::parse("tag:v1")
                .unwrap_err()
                .to_string()
                .contains("tag:v1")
        );
    }

    #[test]
    fn the_role_and_the_brief_both_declare_and_a_repeat_counts_once() {
        let data = BTreeMap::from([(
            BRIEF_DELIVERABLES_KEY.to_owned(),
            serde_json::json!(["branch", "file:out.txt"]),
        )]);
        let all = declared(&["branch".to_owned(), "approval".to_owned()], &data).unwrap();
        assert_eq!(
            all.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["branch", "approval", "file:out.txt"]
        );
        let bad = BTreeMap::from([(BRIEF_DELIVERABLES_KEY.to_owned(), serde_json::json!("x"))]);
        assert!(declared(&[], &bad).is_err());
    }

    #[test]
    fn it_is_text_in_json() {
        let json = serde_json::to_string(&Deliverable::parse("branch:x").unwrap()).unwrap();
        assert_eq!(json, "\"branch:x\"");
        assert!(serde_json::from_str::<Deliverable>("\"nope\"").is_err());
    }
}
