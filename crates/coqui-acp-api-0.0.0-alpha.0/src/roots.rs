//! How a harness compares a path with the working roots, and what a session id looks like.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use std::path::{Path, PathBuf};

/// Which platform's rules a path is read by: where the harness's agent runs, which a spawner that
/// starts it elsewhere says ([`SpawnerInfo::os`](crate::SpawnerInfo::os)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    /// `C:\dir`, `C:/dir` and `\\server\share\dir`; `\` and `/` alike; names compared without
    /// regard to case, as Windows' file systems compare them by default.
    Windows,
    /// `/dir`; names compared exactly.
    Posix,
}

impl PathStyle {
    /// The rules of the platform this was built for.
    #[must_use]
    pub const fn local() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Posix
        }
    }

    /// The rules of the platform `os` names, spelled as Rust's `std::env::consts::OS` spells it.
    #[must_use]
    pub fn of_os(os: &str) -> Self {
        if os == "windows" {
            Self::Windows
        } else {
            Self::Posix
        }
    }

    const fn is_separator(self, c: char) -> bool {
        match self {
            Self::Windows => c == '\\' || c == '/',
            Self::Posix => c == '/',
        }
    }

    /// The path's parts, the first being where it is rooted; `None` for a path that is not
    /// absolute by these rules, or that has a `.` or `..` part.
    fn parts(self, path: &str) -> Option<Vec<String>> {
        let (root, rest) = match self {
            Self::Posix => (String::new(), path.strip_prefix('/')?),
            Self::Windows => {
                let mut chars = path.chars();
                let (first, second) = (chars.next()?, chars.next()?);
                if self.is_separator(first) && self.is_separator(second) {
                    // A share: `\\server\share` is its root.
                    let rest = &path[2..];
                    let mut split = rest.splitn(3, |c| self.is_separator(c));
                    let (server, share) = (split.next()?, split.next()?);
                    if server.is_empty() || share.is_empty() || server == "?" || server == "." {
                        return None;
                    }
                    let root = alloc::format!("//{server}/{share}").to_lowercase();
                    (root, split.next().unwrap_or(""))
                } else if first.is_ascii_alphabetic()
                    && second == ':'
                    && chars.next().is_some_and(|c| self.is_separator(c))
                {
                    (
                        alloc::format!("{}:", first.to_ascii_lowercase()),
                        &path[3..],
                    )
                } else {
                    return None;
                }
            }
        };
        let mut parts = Vec::from([root]);
        for part in rest.split(|c| self.is_separator(c)) {
            match part {
                "" => {}
                "." | ".." => return None,
                part if self == Self::Windows => parts.push(part.to_lowercase()),
                part => parts.push(part.into()),
            }
        }
        Some(parts)
    }
}

/// Whether `path` is absolute by either platform's rules and has no `.` or `..` part: what a path a
/// requirement names must be.
#[must_use]
pub fn is_plain_absolute(path: &str) -> bool {
    PathStyle::Posix.parts(path).is_some() || PathStyle::Windows.parts(path).is_some()
}

/// Whether `text` is a session id as the kind carries one: a UUID in its canonical spelling,
/// lowercase hexadecimal in groups of 8, 4, 4, 4 and 12 joined by `-`.
#[must_use]
pub fn is_session_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(at, byte)| match at {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(byte),
        })
}

/// Why a path is not within the working roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathRefusal {
    /// It is not absolute by the platform's rules, or has a `.` or `..` part.
    NotPlainAbsolute,
    /// It lies in none of the roots, as written.
    Outside,
    /// It lies in a root as written, but a link on the way leads out of every root.
    LinkLeadsOutside,
}

impl fmt::Display for PathRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotPlainAbsolute => "the path is not absolute, or has a `.` or `..` part",
            Self::Outside => "the path is outside the working roots",
            Self::LinkLeadsOutside => "a link in the path leads outside the working roots",
        })
    }
}

impl core::error::Error for PathRefusal {}

/// The directories a request's sessions must work in, and how a path is compared with them.
///
/// A path is within the roots when it is absolute, has no `.` or `..` part (refused, never
/// normalised away), and its parts begin with every part of one root (so `/work-beside` is not
/// in `/work`), compared by the [`PathStyle`]'s rules. Following links, the path and the roots
/// are resolved through the links that exist, and the resolved path must lie in a resolved root
/// too. No roots admit every path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingRoots {
    roots: Vec<String>,
    style: PathStyle,
    follow_links: bool,
}

impl WorkingRoots {
    /// The roots `roots`, read by this platform's rules, links followed.
    #[must_use]
    pub fn new(roots: Vec<String>) -> Self {
        Self {
            roots,
            style: PathStyle::local(),
            follow_links: true,
        }
    }

    /// Reads paths by `style`'s rules: a harness whose agent runs on another platform names it.
    #[must_use]
    pub const fn style(mut self, style: PathStyle) -> Self {
        self.style = style;
        self
    }

    /// Whether links are followed on this machine's file system; off for paths that are not
    /// this machine's.
    #[must_use]
    pub const fn follow_links(mut self, follow: bool) -> Self {
        self.follow_links = follow;
        self
    }

    /// Whether there are no roots, and so no narrowing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Whether `path` lies in one of the roots. A root that is not absolute, or has a `.` or `..`
    /// part, admits nothing.
    ///
    /// # Errors
    ///
    /// Why it does not.
    pub fn admits(&self, path: &str) -> Result<(), PathRefusal> {
        if self.roots.is_empty() {
            return Ok(());
        }
        let parts = self
            .style
            .parts(path)
            .ok_or(PathRefusal::NotPlainAbsolute)?;
        let inside = |parts: &[String], root: &str| {
            self.style
                .parts(root)
                .is_some_and(|root| parts.starts_with(&root))
        };
        if !self.roots.iter().any(|root| inside(&parts, root)) {
            return Err(PathRefusal::Outside);
        }
        if !self.follow_links {
            return Ok(());
        }
        let resolved = |path: &str| unverbatim(&resolved_path(Path::new(path)));
        let path_resolved = self
            .style
            .parts(&resolved(path))
            .ok_or(PathRefusal::LinkLeadsOutside)?;
        if self
            .roots
            .iter()
            .any(|root| inside(&path_resolved, &resolved(root)))
        {
            Ok(())
        } else {
            Err(PathRefusal::LinkLeadsOutside)
        }
    }
}

/// `path` with every link in the part of it that exists resolved, and the rest as written.
fn resolved_path(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(mut found) = std::fs::canonicalize(existing) {
            for part in rest.iter().rev() {
                found.push(part);
            }
            return found;
        }
        let (Some(parent), Some(part)) = (existing.parent(), existing.file_name()) else {
            return path.to_path_buf();
        };
        rest.push(part);
        existing = parent;
    }
}

/// A resolved path as text, without the `\\?\` prefix Windows gives a canonical one.
fn unverbatim(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        alloc::format!(r"\\{share}")
    } else if let Some(local) = text.strip_prefix(r"\\?\") {
        local.into()
    } else {
        text.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(style: PathStyle, roots: &[&str]) -> WorkingRoots {
        WorkingRoots::new(roots.iter().map(|r| String::from(*r)).collect())
            .style(style)
            .follow_links(false)
    }

    #[test]
    fn a_posix_path_is_compared_by_its_parts() {
        let work = roots(PathStyle::Posix, &["/work/"]);
        assert_eq!(work.admits("/work"), Ok(()));
        assert_eq!(work.admits("/work/a//b/"), Ok(()));
        assert_eq!(work.admits("/work-beside"), Err(PathRefusal::Outside));
        assert_eq!(work.admits("/Work"), Err(PathRefusal::Outside));
        assert_eq!(
            work.admits("/work/../etc"),
            Err(PathRefusal::NotPlainAbsolute)
        );
        assert_eq!(work.admits("/work/./a"), Err(PathRefusal::NotPlainAbsolute));
        assert_eq!(work.admits("work/a"), Err(PathRefusal::NotPlainAbsolute));
        assert_eq!(work.admits(r"C:\work"), Err(PathRefusal::NotPlainAbsolute));
    }

    #[test]
    fn a_windows_path_is_compared_without_case_and_either_separator() {
        let work = roots(PathStyle::Windows, &[r"C:\Work"]);
        assert_eq!(work.admits(r"c:\work\a"), Ok(()));
        assert_eq!(work.admits("C:/WORK/a"), Ok(()));
        assert_eq!(work.admits(r"C:\Work-beside"), Err(PathRefusal::Outside));
        assert_eq!(work.admits(r"D:\Work"), Err(PathRefusal::Outside));
        assert_eq!(
            work.admits(r"C:\Work\..\x"),
            Err(PathRefusal::NotPlainAbsolute)
        );
        assert_eq!(work.admits(r"C:Work"), Err(PathRefusal::NotPlainAbsolute));
        assert_eq!(work.admits(r"\Work"), Err(PathRefusal::NotPlainAbsolute));
        let share = roots(PathStyle::Windows, &[r"\\Server\Share\dir"]);
        assert_eq!(share.admits(r"\\server\share\DIR\x"), Ok(()));
        assert_eq!(
            share.admits(r"\\server\other\dir"),
            Err(PathRefusal::Outside)
        );
        assert_eq!(
            share.admits(r"\\?\C:\x"),
            Err(PathRefusal::NotPlainAbsolute)
        );
    }

    #[test]
    fn a_bad_root_admits_nothing_and_no_roots_admit_everything() {
        assert_eq!(
            roots(PathStyle::Posix, &["relative"]).admits("/relative"),
            Err(PathRefusal::Outside)
        );
        assert_eq!(
            roots(PathStyle::Posix, &["/a/../b"]).admits("/b"),
            Err(PathRefusal::Outside)
        );
        assert_eq!(roots(PathStyle::Posix, &[]).admits("anything"), Ok(()));
    }

    #[test]
    fn a_session_id_is_a_canonical_uuid() {
        assert!(is_session_id("0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert!(!is_session_id("0F8FAD5B-D9CB-469F-A165-70867728950E"));
        assert!(!is_session_id("0f8fad5bd9cb469fa16570867728950e"));
        assert!(!is_session_id("{0f8fad5b-d9cb-469f-a165-70867728950e}"));
        assert!(!is_session_id("echo-1"));
    }

    #[test]
    fn a_requirement_path_is_plain_and_absolute_in_either_style() {
        for path in ["/a", r"C:\a", "C:/a", r"\\s\share\a"] {
            assert!(is_plain_absolute(path), "{path}");
        }
        for path in ["a", "/a/../b", r"C:\a\.", "", "C:", r"\\s"] {
            assert!(!is_plain_absolute(path), "{path}");
        }
    }
}
