//! An overlay's files as the knowledge tools see them, and the set of them that is served.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::path::Path;

use super::settings::KnowledgeSettings;
use super::text::needs_writing_out;
use crate::defaults::{declared, named};
use crate::error::{Class, Error};
use crate::fs::{FileKind, FileSystem, name_bytes};
use crate::limits::Settings;
use crate::names::{EXCLUDED_COMPONENTS, valid_relative_path};
use crate::read::Document;
use crate::template::TemplateFacts;

/// The directories whose files are knowledge.
const DIRECTORIES: [&str; 5] = ["kb", "flows", "references", "rules", "context"];

/// One entry of an overlay's tree.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum KnowledgeEntry {
    /// A regular file and its bytes, at most one byte past `max_file_bytes` when read from a
    /// [`FileSystem`].
    File(Vec<u8>),
    /// A regular file whose bytes were not read because it is no knowledge.
    Unread,
    /// A directory.
    Directory,
    /// A symbolic link, junction or other reparse point.
    Symlink,
    /// A device, pipe, socket or anything else.
    Special,
}

/// A template engine this edition defines, which a server may render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Engine {
    /// `${NAME}` over the facts.
    Envsubst,
    /// The Mustache subset over the facts.
    Mustache,
}

impl Engine {
    /// Every engine this crate renders, in the order `kb_list` names them.
    pub const ALL: [Self; 2] = [Self::Envsubst, Self::Mustache];

    /// The name a first line writes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Envsubst => "envsubst",
            Self::Mustache => "mustache",
        }
    }
}

/// How a served file is read.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileFormat {
    /// `${NAME}` replaced and `@path` included: a markdown file that names no format, `aos` or
    /// `agentic`.
    Expanded,
    /// Byte for byte: a file that is not markdown, or names `markdown`, `md` or `raw`.
    AsWritten,
    /// A template of the engine this name gives, which a server may not render, with the options
    /// its first line gives.
    Engine {
        /// The engine's name.
        name: String,
        /// The `name=value` words of the first line.
        options: BTreeMap<String, String>,
    },
}

/// Why a file of the knowledge is not served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// Its name holds a character served text writes out.
    Name,
    /// It is larger than `max_file_bytes`.
    TooLarge,
    /// Its bytes are not UTF-8.
    NotText,
    /// Its served text cannot be made without a `context`.
    Render,
}

impl SkipReason {
    /// The reason as `kb_list` writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::TooLarge => "too-large",
            Self::NotText => "not-text",
            Self::Render => "render",
        }
    }
}

/// A file of the knowledge that is not served.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Skip {
    /// Its path.
    pub path: String,
    /// Why.
    pub reason: SkipReason,
}

/// A served file.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ServedFile<'a> {
    /// Its stored text.
    pub text: &'a str,
    /// The text after its first line when that line names a format, else all of it.
    pub rest: &'a str,
    /// How it is read.
    pub format: FileFormat,
}

/// An overlay's served files and its skipped ones, both in the order of their paths as UTF-8 bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct KnowledgeSet<'a> {
    /// The served files, by path.
    pub files: BTreeMap<&'a str, ServedFile<'a>>,
    /// The skipped files.
    pub skipped: Vec<Skip>,
}

impl KnowledgeSet<'_> {
    /// The reason `path` is skipped, when it is.
    #[must_use]
    pub fn skip_of(&self, path: &str) -> Option<SkipReason> {
        self.skipped
            .iter()
            .find(|skip| skip.path == path)
            .map(|skip| skip.reason)
    }
}

/// The entries of an overlay's tree, by path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnowledgeTree {
    entries: BTreeMap<String, KnowledgeEntry>,
}

impl KnowledgeTree {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The tree with `entry` at `path`.
    #[must_use]
    pub fn with(mut self, path: impl Into<String>, entry: KnowledgeEntry) -> Self {
        self.entries.insert(path.into(), entry);
        self
    }

    /// The tree with a file of `text` at `path`.
    #[must_use]
    pub fn with_text(self, path: impl Into<String>, text: impl Into<String>) -> Self {
        self.with(path, KnowledgeEntry::File(text.into().into_bytes()))
    }

    /// Puts `entry` at `path`.
    pub fn insert(&mut self, path: impl Into<String>, entry: KnowledgeEntry) {
        self.entries.insert(path.into(), entry);
    }

    /// The entry at `path`.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&KnowledgeEntry> {
        self.entries.get(path)
    }

    /// Every entry, in the order of the paths as UTF-8 bytes.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &KnowledgeEntry)> {
        self.entries
            .iter()
            .map(|(path, entry)| (path.as_str(), entry))
    }

    /// Reads the directory `root` on `files` without following a link: every entry's kind, and the
    /// bytes (at most `max_file_bytes` and one more) of each regular file `wanted` says is knowledge.
    /// An excluded name is left out, as the tree digest leaves it out, and so is a name that is not
    /// UTF-8; the core's tree limits bound the walk.
    ///
    /// Opening a path by name races a change of the tree; a [`FileSystem`] that must not leave
    /// the root through a link opens each path without following one.
    ///
    /// # Errors
    ///
    /// `io` when an entry cannot be read, and `too-large` naming the tree limit passed.
    pub fn read(
        files: &dyn FileSystem,
        root: &Path,
        reading: &Settings,
        settings: &KnowledgeSettings,
        wanted: impl Fn(&str) -> bool,
    ) -> Result<Self, Error> {
        let mut tree = Self::new();
        let mut count = 0usize;
        let mut pending = alloc::vec![(root.to_path_buf(), String::new(), 1usize)];
        while let Some((dir, prefix, depth)) = pending.pop() {
            if !reading.tree_depth.allows(depth) {
                return Err(
                    Error::new(Class::TooLarge, "directories nested too deep").over("tree_depth")
                );
            }
            let names = files
                .list(&dir)
                .map_err(|_| Error::new(Class::Io, "the filesystem"))?;
            for name in names {
                let Some(text) = name.to_str() else { continue };
                let bytes = name_bytes(&name);
                if bytes.is_empty()
                    || text == "."
                    || text == ".."
                    || EXCLUDED_COMPONENTS.contains(&text.to_ascii_lowercase().as_str())
                {
                    continue;
                }
                count += 1;
                if !reading.tree_files.allows(count) {
                    return Err(
                        Error::new(Class::TooLarge, "too many files and directories")
                            .over("tree_files"),
                    );
                }
                let path = if prefix.is_empty() {
                    text.to_owned()
                } else {
                    alloc::format!("{prefix}/{text}")
                };
                let full = dir.join(&name);
                let metadata = files
                    .metadata(&full)
                    .map_err(|_| Error::new(Class::Io, path.as_str()))?;
                match metadata.kind {
                    FileKind::Directory => {
                        tree.insert(path.clone(), KnowledgeEntry::Directory);
                        pending.push((full, path, depth + 1));
                    }
                    FileKind::Symlink => tree.insert(path, KnowledgeEntry::Symlink),
                    FileKind::Other => tree.insert(path, KnowledgeEntry::Special),
                    FileKind::File if wanted(&path) => {
                        let most = settings
                            .max_file_bytes
                            .get()
                            .map_or(u64::MAX, |most| most.saturating_add(1));
                        let data = files
                            .read(&full, most)
                            .map_err(|_| Error::new(Class::Io, path.as_str()))?;
                        tree.insert(path, KnowledgeEntry::File(data));
                    }
                    FileKind::File => tree.insert(path, KnowledgeEntry::Unread),
                }
            }
        }
        Ok(tree)
    }
}

/// An active overlay as the knowledge tools see it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct KnowledgeOverlay {
    /// The identity the consumer holds it under.
    pub identity: String,
    /// Its manifest.
    pub manifest: Document,
    /// Its tree.
    pub tree: KnowledgeTree,
    /// Whether the consumer left its `rules` and `context` files out of the context because they
    /// are served.
    pub served: bool,
    /// What its templates know of it.
    pub facts: TemplateFacts,
    /// The limits reading its front matter follows.
    pub reading: Settings,
}

impl KnowledgeOverlay {
    /// An overlay called `identity` with `manifest` and `tree`, not served in place of context,
    /// with no facts.
    #[must_use]
    pub fn new(identity: impl Into<String>, manifest: Document, tree: KnowledgeTree) -> Self {
        Self {
            identity: identity.into(),
            manifest,
            tree,
            served: false,
            facts: TemplateFacts::new(),
            reading: Settings::default(),
        }
    }

    /// Says whether the consumer left its `rules` and `context` files out of the context.
    #[must_use]
    pub fn served(mut self, served: bool) -> Self {
        self.served = served;
        self
    }

    /// Sets what its templates know of it.
    #[must_use]
    pub fn facts(mut self, facts: TemplateFacts) -> Self {
        self.facts = facts;
        self
    }

    /// Sets the limits reading its front matter follows.
    #[must_use]
    pub fn reading(mut self, reading: Settings) -> Self {
        self.reading = reading;
        self
    }

    /// Whether `path` is of the knowledge by where it lies and what the manifest names.
    #[must_use]
    pub fn selects(&self, path: &str) -> bool {
        selected(path, &self.manifest)
    }

    /// The files of the knowledge: served, and skipped with the reason.
    #[must_use]
    pub fn set(&self, settings: &KnowledgeSettings) -> KnowledgeSet<'_> {
        let mut set = KnowledgeSet::default();
        for (path, entry) in self.tree.entries() {
            let KnowledgeEntry::File(data) = entry else {
                continue;
            };
            if !selected(path, &self.manifest) {
                continue;
            }
            if needs_writing_out(path) {
                set.skipped.push(skip(path, SkipReason::Name));
            } else if !settings.max_file_bytes.allows(data.len()) {
                set.skipped.push(skip(path, SkipReason::TooLarge));
            } else if let Ok(text) = core::str::from_utf8(data) {
                let (format, rest) = detect(path, text);
                set.files.insert(path, ServedFile { text, rest, format });
            } else {
                set.skipped.push(skip(path, SkipReason::NotText));
            }
        }
        set
    }
}

fn skip(path: &str, reason: SkipReason) -> Skip {
    Skip {
        path: path.to_string(),
        reason,
    }
}

fn selected(path: &str, manifest: &Document) -> bool {
    if matches!(path, "OVERLAY.md" | "overlay.cfg") || !valid_relative_path(path) {
        return false;
    }
    let first = path.split('/').next().unwrap_or("");
    (path.contains('/') && DIRECTORIES.contains(&first))
        || path == "CONTEXT.md"
        || ["rules", "context"]
            .iter()
            .flat_map(|field| declared(manifest, field))
            .any(|entry| named(entry) == Some(path))
}

impl KnowledgeOverlay {
    /// Reads the overlay at `root` on `files`: its tree as [`KnowledgeTree::read`] does, the bytes
    /// of the files its manifest makes knowledge only.
    ///
    /// # Errors
    ///
    /// As [`KnowledgeTree::read`].
    pub fn read(
        identity: impl Into<String>,
        manifest: Document,
        files: &dyn FileSystem,
        root: &Path,
        reading: &Settings,
        settings: &KnowledgeSettings,
    ) -> Result<Self, Error> {
        let tree = KnowledgeTree::read(files, root, reading, settings, |path| {
            selected(path, &manifest)
        })?;
        Ok(Self::new(identity, manifest, tree).reading(*reading))
    }
}

/// A `name=value` word: `name` lower-case letters and `_`, `value` with no white space.
fn option_of(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    let mut chars = name.chars();
    let named = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c == '_');
    (named && !value.is_empty() && !value.chars().any(char::is_whitespace)).then_some((name, value))
}

/// The name a first line gives its format and its options, or none: `#!` and the last word that is
/// no option, after any slash or backslash, leading dots dropped and ASCII capitals lower-cased.
fn format_line(text: &str) -> Option<(String, BTreeMap<String, String>)> {
    let line = text.strip_prefix("#!")?;
    let line = line.split('\n').next().unwrap_or("");
    let mut options = BTreeMap::new();
    let mut name = None;
    for word in line
        .split([' ', '\t', '\u{c}', '\r'])
        .filter(|word| !word.is_empty())
    {
        match option_of(word) {
            Some((key, value)) => {
                options.insert(key.to_owned(), value.to_owned());
            }
            None => name = Some(word),
        }
    }
    let word = name?.rsplit(['/', '\\']).next().unwrap_or("");
    let word = word.trim_start_matches('.');
    (!word.is_empty()).then(|| (word.to_ascii_lowercase(), options))
}

/// The one place a file is told from a template: only a markdown file reads its first line.
fn detect<'a>(path: &str, text: &'a str) -> (FileFormat, &'a str) {
    let lower = path.to_ascii_lowercase();
    if !(lower.ends_with(".md") || lower.ends_with(".markdown")) {
        return (FileFormat::AsWritten, text);
    }
    let Some((name, options)) = format_line(text) else {
        return (FileFormat::Expanded, text);
    };
    let rest = text.find('\n').map_or("", |end| &text[end + 1..]);
    let format = match name.as_str() {
        "markdown" | "md" | "raw" => FileFormat::AsWritten,
        "aos" | "agentic" => FileFormat::Expanded,
        _ => FileFormat::Engine { name, options },
    };
    (format, rest)
}
