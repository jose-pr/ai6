//! Where an overlay comes from: `[git+]<location>[@<ref>][#<path>]`
//! (`spec/v0/distribution.md`).

use std::fmt;

use super::names::valid_relative_path;

/// What kind of location a source names.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// A directory on the consumer's machine.
    Directory,
    /// A Git repository.
    Git,
    /// Any other URL.
    Url,
}

/// A parsed source string.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Source {
    /// What the location is.
    pub kind: SourceKind,
    /// The directory, repository or URL, as written.
    pub location: String,
    /// A Git location's branch, tag or commit; `None` is the remote's default branch.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// The overlay's root inside the location, a relative path; `None` is its root.
    pub path: Option<String>,
}

/// A source string AOS refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError(&'static str);

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid-source: {}", self.0)
    }
}

impl std::error::Error for SourceError {}

fn is_git_location(location: &str) -> bool {
    location.ends_with(".git")
        || ["git@", "ssh://", "git://"]
            .iter()
            .any(|prefix| location.starts_with(prefix))
}

impl Source {
    /// Parses `text`, surrounding whitespace removed.
    ///
    /// # Errors
    ///
    /// A [`SourceError`] for an empty source, a `#path` that is no relative path, and for a
    /// Git location: a location, ref or host starting with `-`, a remote helper (`::`) or a
    /// scheme other than `https`, `http`, `ssh`, `git` and `file`.
    pub fn parse(text: &str) -> Result<Self, SourceError> {
        let mut raw = text.trim();
        if raw.is_empty() {
            return Err(SourceError("empty"));
        }
        let explicit_git = raw.starts_with("git+");
        if explicit_git {
            raw = &raw["git+".len()..];
        }
        let (body, path) = match raw.split_once('#') {
            Some((body, path)) => {
                let path = path.trim().trim_matches('/');
                (body, (!path.is_empty()).then(|| path.to_owned()))
            }
            None => (raw, None),
        };
        let mut location = body.trim();
        let mut reference = None;
        if let Some((head, tail)) = location.rsplit_once('@')
            && !head.is_empty()
            && !tail.is_empty()
            && !tail.contains(['/', ':', '\\'])
            && !head.ends_with("://")
            && !head.ends_with(':')
            && (explicit_git || is_git_location(head))
        {
            location = head;
            reference = Some(tail.to_owned());
        }
        if path
            .as_deref()
            .is_some_and(|path| !valid_relative_path(path))
        {
            return Err(SourceError("the #path is not a relative path"));
        }
        if explicit_git || is_git_location(location) {
            if location.starts_with('-') || reference.as_deref().is_some_and(|r| r.starts_with('-'))
            {
                return Err(SourceError("a location or ref that reads as an option"));
            }
            if location.contains("::") {
                return Err(SourceError("a Git remote helper"));
            }
            let host = if let Some((scheme, rest)) = location.split_once("://") {
                if !["https", "http", "ssh", "git", "file"]
                    .contains(&scheme.to_ascii_lowercase().as_str())
                {
                    return Err(SourceError("a scheme Git is not given"));
                }
                let authority = rest.split('/').next().unwrap_or_default();
                authority.rsplit('@').next().unwrap_or_default()
            } else if location.contains('@') {
                let before = location.split(':').next().unwrap_or_default();
                before.rsplit('@').next().unwrap_or_default()
            } else {
                ""
            };
            if host.starts_with('-') {
                return Err(SourceError("a host that reads as an option"));
            }
            return Ok(Self {
                kind: SourceKind::Git,
                location: location.to_owned(),
                reference,
                path,
            });
        }
        Ok(Self {
            kind: if location.contains("://") {
                SourceKind::Url
            } else {
                SourceKind::Directory
            },
            location: location.to_owned(),
            reference: None,
            path,
        })
    }

    /// The source as it may be shown or stored: user information and a query removed from a
    /// URL location.
    pub fn shown(&self) -> String {
        let mut location = self.location.clone();
        if let Some((scheme, rest)) = location.split_once("://") {
            let (authority, tail) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            let host = authority.rsplit('@').next().unwrap_or_default();
            let tail = tail.split('?').next().unwrap_or_default();
            location = format!("{scheme}://{host}{tail}");
        }
        let mut shown = location;
        if let Some(reference) = &self.reference {
            shown.push('@');
            shown.push_str(reference);
        }
        if let Some(path) = &self.path {
            shown.push('#');
            shown.push_str(path);
        }
        shown
    }
}
