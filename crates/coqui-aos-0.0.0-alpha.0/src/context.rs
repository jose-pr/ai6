//! A project's context files: `AGENTS.md` and its like in each directory from the project root to
//! the working directory, and in each directory's `.agents` one, in the order a later file
//! overrides an earlier one.

use std::path::{Path, PathBuf};

use crate::fs::{FileKind, FileSystem};

const DEFAULT_DIRECTORIES: [&str; 1] = [".agents"];
const DEFAULT_PATTERNS: [&str; 2] = ["AGENTS.md", "AGENTS.*.md"];

/// Which context files a session works on: its project root, its working directory, the two
/// variables that name the places and the files, the paths its harness loads for itself and the
/// levels it has already received.
///
/// Built with [`ProjectContext::new`] and the setters.
#[derive(Clone, Debug)]
pub struct ProjectContext {
    root: Option<PathBuf>,
    cwd: PathBuf,
    directories: Option<String>,
    patterns: Option<String>,
    separator: char,
    native: Vec<String>,
    delivered: Vec<PathBuf>,
}

impl ProjectContext {
    /// A query for the project at `root` (none when the session has no project) and the working
    /// directory `cwd`, with the default places and file names and this platform's separator.
    #[must_use]
    pub fn new(root: Option<PathBuf>, cwd: PathBuf) -> Self {
        Self {
            root,
            cwd,
            directories: None,
            patterns: None,
            separator: if cfg!(windows) { ';' } else { ':' },
            native: Vec::new(),
            delivered: Vec::new(),
        }
    }

    /// The value of `AGENTS_DIR`; `None` for an unset variable.
    #[must_use]
    pub fn agents_dir(mut self, value: Option<String>) -> Self {
        self.directories = value;
        self
    }

    /// The value of `AGENTS_CONTEXT`; `None` for an unset variable.
    #[must_use]
    pub fn agents_context(mut self, value: Option<String>) -> Self {
        self.patterns = value;
        self
    }

    /// The separator of the two variables' lists, `;` or `:`. The default is the platform's.
    #[must_use]
    pub fn separator(mut self, separator: char) -> Self {
        self.separator = separator;
        self
    }

    /// The paths, relative to a level's directory and written with `/`, that the harness loads
    /// itself (`AGENTS.md`): they are left out of the files to feed.
    #[must_use]
    pub fn native<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.native = paths.into_iter().map(Into::into).collect();
        self
    }

    /// The levels a session has already received, so that a change of directory feeds only the new
    /// ones.
    #[must_use]
    pub fn delivered<I, P>(mut self, levels: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.delivered = levels.into_iter().map(Into::into).collect();
        self
    }
}

/// The context files of one query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ContextFiles {
    /// The files to feed, in order: a later file overrides an earlier one.
    pub files: Vec<PathBuf>,
    /// The files of the walk the harness loads itself, in the same order.
    pub skipped: Vec<PathBuf>,
    /// The entries of `AGENTS_DIR` and `AGENTS_CONTEXT` that are not valid, as written.
    pub ignored: Vec<String>,
    /// The levels this answer covers: the walk without the levels already delivered.
    pub levels: Vec<PathBuf>,
}

/// The project's context files for a working directory, read through `fs`.
///
/// The levels are the project root and each directory below it down to the working directory; a
/// working directory outside the root, or no root, is one level. A level's files come as the
/// specification's *Project context files* section orders them: the files that are not local,
/// place by place (the directory, then each `AGENTS_DIR` entry), then the local files. A link to a
/// file is a context file and a link to a directory is not.
#[must_use]
pub fn context_files<F: FileSystem>(fs: &F, query: &ProjectContext) -> ContextFiles {
    let (directories, bad_directories) = entries(
        query.directories.as_deref(),
        query.separator,
        &DEFAULT_DIRECTORIES,
        true,
    );
    let (patterns, bad_patterns) = entries(
        query.patterns.as_deref(),
        query.separator,
        &DEFAULT_PATTERNS,
        false,
    );
    let mut ignored = bad_directories;
    ignored.extend(bad_patterns);

    let levels: Vec<PathBuf> = walk(query, &directories)
        .into_iter()
        .filter(|level| !query.delivered.iter().any(|done| done == level))
        .collect();
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for level in &levels {
        for local in [false, true] {
            for directory in core::iter::once(None).chain(directories.iter().map(Some)) {
                let place = directory.map_or_else(|| level.clone(), |name| level.join(name));
                for name in names_in(fs, &place, &patterns, local) {
                    let path = place.join(&name);
                    if !is_context_file(fs, &path) {
                        continue;
                    }
                    let relative = directory
                        .map_or_else(|| name.clone(), |directory| format!("{directory}/{name}"));
                    if query.native.contains(&relative) {
                        skipped.push(path);
                    } else {
                        files.push(path);
                    }
                }
            }
        }
    }
    ContextFiles {
        files,
        skipped,
        ignored,
        levels,
    }
}

/// The valid entries of a list variable, once each, and the entries that are not valid.
fn entries(
    value: Option<&str>,
    separator: char,
    default: &[&str],
    directory: bool,
) -> (Vec<String>, Vec<String>) {
    let mut kept: Vec<String> = Vec::new();
    let mut ignored = Vec::new();
    for entry in value.unwrap_or_default().split(separator) {
        if entry.is_empty() {
            continue;
        }
        if entry.contains(['/', '\\']) || (directory && (entry == "." || entry == "..")) {
            ignored.push(entry.to_owned());
        } else if !kept.iter().any(|seen| seen == entry) {
            kept.push(entry.to_owned());
        }
    }
    if kept.is_empty() {
        kept = default.iter().map(|entry| (*entry).to_owned()).collect();
    }
    (kept, ignored)
}

/// The levels of the walk, broadest first.
fn walk(query: &ProjectContext, directories: &[String]) -> Vec<PathBuf> {
    let Some(root) = query
        .root
        .as_deref()
        .filter(|root| query.cwd.starts_with(root))
    else {
        return vec![query.cwd.clone()];
    };
    let below = query.cwd.strip_prefix(root).unwrap_or(Path::new(""));
    let mut levels = vec![root.to_path_buf()];
    let mut at = root.to_path_buf();
    for part in below.components() {
        at.push(part);
        let own = part.as_os_str().to_str();
        if !own.is_some_and(|name| directories.iter().any(|entry| entry == name)) {
            levels.push(at.clone());
        }
    }
    levels
}

/// The names in `place` that a pattern matches and whose locality is `local`: pattern by pattern,
/// each in byte order, none twice.
fn names_in<F: FileSystem>(fs: &F, place: &Path, patterns: &[String], local: bool) -> Vec<String> {
    let Ok(listed) = fs.list(place) else {
        return Vec::new();
    };
    let mut names: Vec<String> = listed
        .into_iter()
        .filter_map(|name| name.into_string().ok())
        .filter(|name| is_local(name) == local)
        .collect();
    names.sort();
    let mut found: Vec<String> = Vec::new();
    for pattern in patterns {
        for name in &names {
            if matches(pattern, name) && !found.contains(name) {
                found.push(name.clone());
            }
        }
    }
    found
}

fn is_context_file<F: FileSystem>(fs: &F, path: &Path) -> bool {
    match fs.metadata(path) {
        Ok(metadata) => match metadata.kind {
            FileKind::File => true,
            FileKind::Symlink => fs.list(path).is_err(),
            _ => false,
        },
        Err(_) => false,
    }
}

/// Whether a name is `<stem>.local.<extension>`.
fn is_local(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(rest, extension)| {
        !extension.is_empty()
            && rest
                .strip_suffix(".local")
                .is_some_and(|stem| !stem.is_empty())
    })
}

/// A name pattern: `*` is any run of characters, none included, and the rest is literal.
fn matches(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let mut parts: Vec<&str> = parts.collect();
    let Some(last) = parts.pop() else {
        return rest.is_empty();
    };
    for part in parts {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::{is_local, matches};

    #[test]
    fn a_star_is_a_run_of_characters() {
        assert!(matches("AGENTS.md", "AGENTS.md"));
        assert!(!matches("AGENTS.md", "AGENTS.mdx"));
        assert!(matches("AGENTS.*.md", "AGENTS.a.b.md"));
        assert!(!matches("AGENTS.*.md", "AGENTS.md"));
        assert!(matches("AGENTS*.md", "AGENTS.md"));
        assert!(matches("*", ""));
        assert!(matches("a*b*c", "abc"));
        assert!(matches("a*b*c", "aXbYbc"));
        assert!(!matches("a*b*c", "acb"));
        assert!(!matches("ab*ba", "aba"));
    }

    #[test]
    fn local_is_the_next_to_last_part() {
        assert!(is_local("AGENTS.local.md"));
        assert!(is_local("AGENTS.a.local.md"));
        assert!(!is_local(".local.md"));
        assert!(!is_local("AGENTS.local"));
        assert!(!is_local("AGENTS.local.md.bak"));
        assert!(!is_local("AGENTS.md"));
    }
}
