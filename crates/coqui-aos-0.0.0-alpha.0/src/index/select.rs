//! The files an index is made of: where they come from, and which of them its filters select.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use std::fs;
use std::path::Path;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use sha2::{Digest as _, Sha256};

use super::Error;
use crate::{CallerFacts, KnowledgeOverlay, KnowledgeSettings, Renderers, served_texts};

/// The largest file read from a folder; a bigger one is left out and named in [`Corpus::skipped`].
const MAX_FILE: u64 = 8 * 1024 * 1024;

/// The text files an index is made or updated from, by path relative to their root, with `/` between components.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Corpus {
    files: BTreeMap<String, String>,
    /// The paths of files that are not UTF-8 text (or are too large), which no index holds.
    pub skipped: Vec<String>,
}

impl Corpus {
    /// An empty corpus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the file `path` with its bytes: kept when they are UTF-8 text with no NUL, else named in [`Corpus::skipped`].
    ///
    /// # Errors
    ///
    /// [`Error::Path`] when `path` is empty, absolute, or has a `.`, `..` or empty component.
    pub fn insert(&mut self, path: &str, bytes: Vec<u8>) -> Result<(), Error> {
        let path = relative(path)?;
        match String::from_utf8(bytes) {
            Ok(text) if !text.contains('\0') => {
                self.files.insert(path, text);
            }
            _ => self.skipped.push(path),
        }
        Ok(())
    }

    /// The served text of each file of an overlay's knowledge, as `kb_read` gives it with no `context`:
    /// what a search indexes, so a passage's lines are the lines `kb_read` numbers. A template is read
    /// with the facts of `caller`, so an index of one is as old as those facts.
    #[must_use]
    pub fn from_overlay(
        overlay: &KnowledgeOverlay,
        settings: &KnowledgeSettings,
        caller: &CallerFacts,
        engines: &Renderers,
    ) -> Self {
        let mut corpus = Self::new();
        for (path, text) in served_texts(overlay, settings, caller, engines) {
            if !text.contains('\0') {
                corpus.files.insert(path, text);
            }
        }
        corpus
    }

    /// The files under the folder `root`, without following a link and leaving out `.git`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when a folder or file cannot be read.
    pub fn from_dir(root: &Path) -> Result<Self, Error> {
        let mut corpus = Self::new();
        corpus.walk(root, "")?;
        Ok(corpus)
    }

    fn walk(&mut self, dir: &Path, prefix: &str) -> Result<(), Error> {
        let entries = fs::read_dir(dir).map_err(|error| Error::io(dir, &error))?;
        for entry in entries {
            let entry = entry.map_err(|error| Error::io(dir, &error))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let kind = fs::symlink_metadata(&path).map_err(|error| Error::io(&path, &error))?;
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                alloc::format!("{prefix}/{name}")
            };
            if kind.is_dir() {
                if name != ".git" {
                    self.walk(&path, &relative)?;
                }
            } else if kind.is_file() {
                if kind.len() > MAX_FILE {
                    self.skipped.push(relative);
                } else {
                    let bytes = fs::read(&path).map_err(|error| Error::io(&path, &error))?;
                    self.insert(&relative, bytes)?;
                }
            }
        }
        Ok(())
    }

    /// The text of the file `path`.
    #[must_use]
    pub fn text(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    /// The paths of the files, in order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// How many files it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether it holds no file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub(super) fn files(&self) -> impl Iterator<Item = (&str, &str)> {
        self.files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
    }
}

/// A path relative to a root, in the form the index keeps: components joined with `/`.
pub(super) fn relative(path: &str) -> Result<String, Error> {
    let path = path.replace('\\', "/");
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains(':')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if bad {
        return Err(Error::Path(path));
    }
    Ok(path)
}

/// The SHA-256 of `text`, in lowercase hex.
pub(super) fn digest(text: &str) -> String {
    data_encoding::HEXLOWER.encode(&Sha256::digest(text.as_bytes()))
}

/// The include and exclude filters of an index.
#[derive(Debug)]
pub(super) struct Selector {
    include: Option<GlobSet>,
    exclude: GlobSet,
}

impl Selector {
    /// Compiles the filters; no include filter selects every file.
    pub(super) fn new(include: &[String], exclude: &[String]) -> Result<Self, Error> {
        Ok(Self {
            include: if include.is_empty() {
                None
            } else {
                Some(compile(include)?)
            },
            exclude: compile(exclude)?,
        })
    }

    pub(super) fn selects(&self, path: &str) -> bool {
        self.include
            .as_ref()
            .is_none_or(|include| include.is_match(path))
            && !self.exclude.is_match(path)
    }
}

/// A pattern with no `/` matches at any depth, as in a `.gitignore`; `*` never crosses a `/` and `**` does.
fn compile(patterns: &[String]) -> Result<GlobSet, Error> {
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        let pattern = pattern.replace('\\', "/");
        let full = if pattern.contains('/') {
            pattern
                .trim_start_matches("./")
                .trim_start_matches('/')
                .to_owned()
        } else {
            alloc::format!("**/{pattern}")
        };
        let glob: Glob = GlobBuilder::new(&full)
            .literal_separator(true)
            .build()
            .map_err(|error| Error::Settings(alloc::format!("the pattern `{pattern}`: {error}")))?;
        set.add(glob);
    }
    set.build()
        .map_err(|error| Error::Settings(error.to_string()))
}
