//! A local index of an overlay's knowledge files: text and Markdown cut into passages that keep their path and line range,
//! searched by meaning (vectors from an embedding provider) or by words (BM25, no model at all).
//!
//! Search returns passages ranked by score with their sources, never an answer. An index lives in a directory of its own,
//! under the host's state per overlay version ([`index_dir`]); it saves the settings it was made with, including the exact
//! model, and a write goes to a new generation beside the committed one so that a failed update or rebuild leaves the
//! previous index in place ([`Index::create`], [`Index::update`], [`Index::rebuild`]).
//!
//! Two writers on one directory are not coordinated: each commits a whole generation, and the last to commit is the index.

#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

mod backend;
mod chunk;
mod embed;
#[cfg(feature = "embeddings")]
mod http;
mod lexical;
mod select;
mod store;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

pub use backend::IndexSearch;
pub use embed::Embedder;
#[cfg(feature = "embeddings")]
pub use http::HttpEmbedder;
pub use select::Corpus;

use lexical::Lexical;
use select::{Selector, digest, relative};

/// The most bytes of text one embeddings request carries, a single larger passage excepted.
const REQUEST_BYTES: usize = 16 * 1024;

/// Why an index operation failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A file or folder could not be read or written.
    Io {
        /// Where.
        path: PathBuf,
        /// What the system said.
        message: String,
    },
    /// An index's files are not what this reads.
    Format(String),
    /// The settings are not usable.
    Settings(String),
    /// A path of a file is not relative to the root.
    Path(String),
    /// The embedding provider failed or answered with something unusable.
    Embed(String),
    /// [`Index::create`] met an index.
    Exists(PathBuf),
    /// There is no index in the folder.
    Missing(PathBuf),
    /// The embedding model does not fit the index.
    Model(String),
    /// The search mode needs what the index or the call lacks.
    Mode(String),
}

impl Error {
    pub(crate) fn io(path: &Path, error: &std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Format(message) => write!(f, "the index cannot be read: {message}"),
            Self::Settings(message) => write!(f, "the index settings: {message}"),
            Self::Path(path) => write!(f, "`{path}` is not a path relative to the root"),
            Self::Embed(message) => write!(f, "the embedding provider: {message}"),
            Self::Exists(path) => write!(f, "an index already exists in {}", path.display()),
            Self::Missing(path) => write!(f, "there is no index in {}", path.display()),
            Self::Model(message) | Self::Mode(message) => f.write_str(message),
        }
    }
}

impl core::error::Error for Error {}

/// What an index is made with, saved with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Settings {
    /// The most bytes of a passage, its document prefix included (4 to 65536; bytes, not tokens). 1024 by default.
    pub chunk_bytes: usize,
    /// The most bytes of whole lines two adjacent passages share; below the room a passage has after its prefix. 0 by default.
    pub overlap_bytes: usize,
    /// Globs of the files to index, matched against the path relative to the root; a pattern with no `/` matches at any
    /// depth, `*` stays within a folder and `**` does not. Empty means every file of the corpus.
    pub include: Vec<String>,
    /// Globs of files left out, written like `include`.
    pub exclude: Vec<String>,
    /// Text put before each passage when it is embedded, for models that ask for one.
    pub document_prefix: String,
    /// Text put before the query when it is embedded.
    pub query_prefix: String,
    /// The exact embedding model the vectors come from; `None` for an index of words only. Set from the embedder given to
    /// [`Index::create`] and [`Index::rebuild`], and never chosen here.
    pub model: Option<String>,
    /// The length of the vectors, known once one has been made.
    pub dimensions: Option<usize>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            chunk_bytes: 1024,
            overlap_bytes: 0,
            include: Vec::new(),
            exclude: Vec::new(),
            document_prefix: String::new(),
            query_prefix: String::new(),
            model: None,
            dimensions: None,
        }
    }
}

impl Settings {
    fn validate(&self) -> Result<(), Error> {
        if !(4..=65536).contains(&self.chunk_bytes) {
            return Err(Error::Settings(alloc::format!(
                "chunk_bytes is {}, it must be 4 to 65536",
                self.chunk_bytes
            )));
        }
        let room = self.chunk_bytes.saturating_sub(self.document_prefix.len());
        if room < 4 {
            return Err(Error::Settings(
                "the document prefix leaves a passage less than 4 bytes".into(),
            ));
        }
        if self.overlap_bytes >= room {
            return Err(Error::Settings(alloc::format!(
                "overlap_bytes is {}, it must be below the {room} bytes a passage has after its prefix",
                self.overlap_bytes
            )));
        }
        Selector::new(&self.include, &self.exclude).map(drop)
    }

    fn room(&self) -> usize {
        self.chunk_bytes - self.document_prefix.len()
    }
}

/// One passage of an indexed file.
#[derive(Clone, Debug)]
pub(crate) struct Chunk {
    pub(crate) path: String,
    pub(crate) heading: String,
    pub(crate) from: usize,
    pub(crate) to: usize,
    pub(crate) text: String,
    pub(crate) vector: Option<Vec<f32>>,
}

/// How a write runs.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    /// [`Index::update`] only: remove the passages of files that are missing or no longer selected; otherwise they stay.
    pub prune: bool,
    /// Report what would change, without writing and without calling the embedder.
    pub dry_run: bool,
    /// The most passages per embeddings request (1 to 128; a request also stops at 16 KiB). 8 by default.
    pub batch_size: usize,
    /// What the index is made from, saved with it: an overlay's tree digest.
    pub source: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            prune: false,
            dry_run: false,
            batch_size: 8,
            source: None,
        }
    }
}

/// What a write did, or with `dry_run` would do. Paths are relative to the root, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Report {
    /// Files new to the index.
    pub added: Vec<String>,
    /// Files whose content changed since they were indexed.
    pub changed: Vec<String>,
    /// Files indexed already with the same content, whose passages were reused.
    pub unchanged: Vec<String>,
    /// Files taken out of the index: missing or no longer selected, pruned or by a rebuild.
    pub removed: Vec<String>,
    /// Files missing or no longer selected that an update left in, for want of `prune`.
    pub retained: Vec<String>,
    /// Files in the corpus that are not UTF-8 text, or too large.
    pub skipped: Vec<String>,
    /// Passages made new (and embedded, for an index with vectors).
    pub passages_new: usize,
    /// Passages reused or retained from the index before.
    pub passages_kept: usize,
    /// Whether nothing was written.
    pub dry_run: bool,
    /// The committed generation after the call, or the one before for a dry run; `None` when there is none.
    pub generation: Option<u64>,
}

/// How a search scores.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mode {
    /// By meaning when the index has vectors and an embedder is given, else by words.
    #[default]
    Auto,
    /// By words (BM25), whatever the index holds.
    Lexical,
    /// By meaning (cosine similarity); needs an index with vectors and an embedder of its model.
    Semantic,
}

/// What a search asks for.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SearchOptions {
    /// The most passages returned, 1 or more; 5 by default.
    pub top: usize,
    /// Passages scoring below this are left out: cosine similarity (-1 to 1) by meaning, the BM25 score by words.
    pub min_score: Option<f32>,
    /// Only passages of this file, or under this folder, relative to the root; not a glob.
    pub path: Option<String>,
    /// How to score.
    pub mode: Mode,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            top: 5,
            min_score: None,
            path: None,
            mode: Mode::Auto,
        }
    }
}

/// A passage found.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Hit {
    /// Its file, relative to the root.
    pub path: String,
    /// The headings above it, joined with ` > `.
    pub heading: String,
    /// Its first line, counting from 1.
    pub from_line: usize,
    /// Its last line, inclusive.
    pub to_line: usize,
    /// How well it matches; see [`SearchOptions::min_score`].
    pub score: f32,
    /// Its text.
    pub text: String,
}

/// A committed index, read into memory.
#[derive(Debug)]
pub struct Index {
    generation: u64,
    settings: Settings,
    source: Option<String>,
    files: BTreeMap<String, String>,
    chunks: Vec<Chunk>,
    lexical: OnceLock<Lexical>,
}

/// The directory holding the index of `overlay` at tree digest `digest` under the host's state `state`. A new version of an
/// overlay is a new directory; [`remove_other_versions`] clears the old ones.
#[must_use]
pub fn index_dir(state: &Path, overlay: &str, digest: &str) -> PathBuf {
    state.join("kb").join(safe(overlay)).join(safe(digest))
}

/// Removes the indexes of `overlay` that are for any version but `digest`, and returns how many.
///
/// # Errors
///
/// [`Error::Io`] when a folder cannot be read.
pub fn remove_other_versions(state: &Path, overlay: &str, digest: &str) -> Result<usize, Error> {
    let parent = state.join("kb").join(safe(overlay));
    let keep = safe(digest);
    let entries = match std::fs::read_dir(&parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(Error::io(&parent, &error)),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(|error| Error::io(&parent, &error))?;
        if entry.file_name().to_string_lossy() != keep
            && std::fs::remove_dir_all(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    Ok(removed)
}

/// The folder inside an overlay version's folder that holds the index made with `model`: one for words, one for each model,
/// so changing the model starts a new index and never mixes vectors.
#[must_use]
pub fn variant_dir(version: &Path, model: Option<&str>) -> PathBuf {
    version.join(model.map_or_else(
        || "words".to_owned(),
        |model| alloc::format!("m-{}", safe(model)),
    ))
}

/// The folder of the newest other version of `overlay` that holds an index of `variant` (a [`variant_dir`] name), to seed the
/// index of the version with `digest` from.
#[must_use]
pub fn previous_version(
    state: &Path,
    overlay: &str,
    digest: &str,
    variant: &str,
) -> Option<PathBuf> {
    let parent = state.join("kb").join(safe(overlay));
    let keep = safe(digest);
    let mut found: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&parent).ok()?.flatten() {
        if entry.file_name().to_string_lossy() == keep {
            continue;
        }
        let candidate = entry.path().join(variant);
        let Ok(modified) =
            std::fs::metadata(candidate.join("CURRENT")).and_then(|meta| meta.modified())
        else {
            continue;
        };
        if found.as_ref().is_none_or(|(newest, _)| modified > *newest) {
            found = Some((modified, candidate));
        }
    }
    found.map(|(_, path)| path)
}

/// `name` as one path component: letters, digits, `.`, `-` and `_` stay, anything else becomes `%` and its hex.
fn safe(name: &str) -> String {
    let mut out = String::new();
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            out.push(char::from(byte));
        } else {
            out.push_str(&alloc::format!("%{byte:02x}"));
        }
    }
    if out.is_empty() {
        out.push('%');
    }
    out
}

impl Index {
    /// Reads the committed index in `dir`.
    ///
    /// # Errors
    ///
    /// [`Error::Missing`] when there is none, [`Error::Format`] and [`Error::Io`] when it cannot be read.
    pub fn open(dir: &Path) -> Result<Self, Error> {
        let loaded = store::read(dir)?.ok_or_else(|| Error::Missing(dir.to_path_buf()))?;
        Ok(Self {
            generation: loaded.generation,
            settings: loaded.meta.settings,
            source: loaded.meta.source,
            files: loaded.meta.files,
            chunks: loaded.chunks,
            lexical: OnceLock::new(),
        })
    }

    /// Whether `dir` holds a committed index.
    #[must_use]
    pub fn exists(dir: &Path) -> bool {
        dir.join("CURRENT").is_file()
    }

    /// The settings it was made with.
    #[must_use]
    pub const fn settings(&self) -> &Settings {
        &self.settings
    }

    /// What it was made from, as [`Options::source`] named it.
    #[must_use]
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// The committed generation, which grows with each write.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The indexed files, in order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    /// How many passages it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Whether it holds no passage.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Makes a new index in `dir` from the files of `corpus` that `settings` selects. `embedder` makes it an index with
    /// vectors, of the model it names.
    ///
    /// # Errors
    ///
    /// [`Error::Exists`] when `dir` holds an index; [`Error::Settings`]; [`Error::Embed`]; [`Error::Io`]. A failure leaves
    /// `dir` as it was.
    pub fn create(
        dir: &Path,
        settings: Settings,
        corpus: &Corpus,
        embedder: Option<&dyn Embedder>,
        options: &Options,
    ) -> Result<Report, Error> {
        if Self::exists(dir) {
            return Err(Error::Exists(dir.to_path_buf()));
        }
        run(dir, Op::Create, None, settings, corpus, embedder, options)
    }

    /// Adds the files that are new, refreshes the ones whose content changed and reuses the passages of the rest, by the
    /// SHA-256 of each file. A file missing or no longer selected stays unless `options.prune`. The settings are the saved
    /// ones; `embedder`, for an index with vectors, must be of the saved model.
    ///
    /// # Errors
    ///
    /// [`Error::Missing`] when `dir` holds no index; [`Error::Model`] when the embedder is not the saved model or an index
    /// with vectors has files to embed and none is given; [`Error::Embed`]; [`Error::Io`]. A failure leaves the committed
    /// index as it was.
    pub fn update(
        dir: &Path,
        corpus: &Corpus,
        embedder: Option<&dyn Embedder>,
        options: &Options,
    ) -> Result<Report, Error> {
        let prior = Self::open(dir)?;
        let settings = prior.settings.clone();
        run(
            dir,
            Op::Update,
            Some(&prior),
            settings,
            corpus,
            embedder,
            options,
        )
    }

    /// Makes the index again from the files of `corpus` under `settings`, which replace the saved ones (start from
    /// [`Index::settings`] to change one): every passage is made again, and files missing or no longer selected go. Model
    /// as in [`Index::create`]: `embedder` is the new model, and none makes an index of words.
    ///
    /// # Errors
    ///
    /// As [`Index::create`], with [`Error::Missing`] for `dir` holding no index. A failure leaves the committed index as it
    /// was.
    pub fn rebuild(
        dir: &Path,
        settings: Settings,
        corpus: &Corpus,
        embedder: Option<&dyn Embedder>,
        options: &Options,
    ) -> Result<Report, Error> {
        let prior = Self::open(dir)?;
        run(
            dir,
            Op::Rebuild,
            Some(&prior),
            settings,
            corpus,
            embedder,
            options,
        )
    }

    /// Creates the index when `dir` holds none, else updates it with pruning: what to call when an overlay's version may
    /// have changed. A new index starts from the one in `seed` (a folder of an earlier version) when that was made with the
    /// same settings and model, so only the files that differ from it are made again.
    ///
    /// # Errors
    ///
    /// As [`Index::create`] or [`Index::update`].
    pub fn refresh(
        dir: &Path,
        settings: Settings,
        corpus: &Corpus,
        embedder: Option<&dyn Embedder>,
        options: &Options,
        seed: Option<&Path>,
    ) -> Result<Report, Error> {
        if !Self::exists(dir)
            && let Some(from) = seed
            && let Ok(earlier) = Self::open(from)
        {
            let mut wanted = settings.clone();
            wanted.model = embedder.map(|embedder| embedder.model().to_string());
            wanted.dimensions = earlier.settings.dimensions;
            if wanted == earlier.settings {
                let meta = store::Meta::new(earlier.settings.clone(), None, earlier.files.clone());
                store::commit(dir, &meta, &earlier.chunks)?;
            }
        }
        if Self::exists(dir) {
            let mut options = options.clone();
            options.prune = true;
            Self::update(dir, corpus, embedder, &options)
        } else {
            Self::create(dir, settings, corpus, embedder, options)
        }
    }

    /// The passages that best match `query`, best first, ties by path and line.
    ///
    /// # Errors
    ///
    /// [`Error::Settings`] for a `top` of 0; [`Error::Path`]; [`Error::Mode`] when `Semantic` is asked of
    /// an index with no vectors or with no embedder; [`Error::Model`] when the embedder is not the saved model;
    /// [`Error::Embed`].
    pub fn search(
        &self,
        query: &str,
        options: &SearchOptions,
        embedder: Option<&dyn Embedder>,
    ) -> Result<Vec<Hit>, Error> {
        if options.top < 1 {
            return Err(Error::Settings("top is 0, it must be 1 or more".into()));
        }
        let within = options.path.as_deref().map(relative).transpose()?;
        let semantic = match options.mode {
            Mode::Lexical => false,
            Mode::Auto => self.settings.model.is_some() && embedder.is_some(),
            Mode::Semantic => {
                if self.settings.model.is_none() {
                    return Err(Error::Mode("the index has no vectors".into()));
                }
                if embedder.is_none() {
                    return Err(Error::Mode("a search by meaning needs an embedder".into()));
                }
                true
            }
        };
        let candidates: Vec<usize> = (0..self.chunks.len())
            .filter(|&index| {
                within.as_deref().is_none_or(|prefix| {
                    let path = &self.chunks[index].path;
                    path == prefix
                        || path
                            .strip_prefix(prefix)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
            })
            .collect();
        let mut scored: Vec<(usize, f32)> = if semantic {
            self.by_meaning(query, &candidates, embedder)?
        } else {
            self.by_words(query, &candidates)
        };
        if let Some(floor) = options.min_score {
            scored.retain(|(_, score)| *score >= floor);
        }
        scored.sort_by(|(a, left), (b, right)| {
            right
                .total_cmp(left)
                .then_with(|| self.chunks[*a].path.cmp(&self.chunks[*b].path))
                .then_with(|| self.chunks[*a].from.cmp(&self.chunks[*b].from))
                .then_with(|| self.chunks[*a].to.cmp(&self.chunks[*b].to))
        });
        scored.truncate(options.top);
        Ok(scored
            .into_iter()
            .map(|(index, score)| {
                let chunk = &self.chunks[index];
                Hit {
                    path: chunk.path.clone(),
                    heading: chunk.heading.clone(),
                    from_line: chunk.from,
                    to_line: chunk.to,
                    score,
                    text: chunk.text.clone(),
                }
            })
            .collect())
    }

    fn by_words(&self, query: &str, candidates: &[usize]) -> Vec<(usize, f32)> {
        let lexical = self.lexical.get_or_init(|| {
            Lexical::new(self.chunks.iter().map(|chunk| {
                [
                    chunk.path.as_str(),
                    chunk.heading.as_str(),
                    chunk.text.as_str(),
                ]
            }))
        });
        let mut words: Vec<String> = lexical::words(query).collect();
        words.sort();
        words.dedup();
        candidates
            .iter()
            .map(|&index| (index, lexical.score(index, &words)))
            .filter(|(_, score)| *score > 0.0)
            .collect()
    }

    fn by_meaning(
        &self,
        query: &str,
        candidates: &[usize],
        embedder: Option<&dyn Embedder>,
    ) -> Result<Vec<(usize, f32)>, Error> {
        let (Some(embedder), Some(saved)) = (embedder, self.settings.model.as_deref()) else {
            return Err(Error::Mode("a search by meaning needs an embedder".into()));
        };
        if embedder.model() != saved {
            return Err(Error::Model(alloc::format!(
                "the index holds vectors of {saved}, not of {}",
                embedder.model()
            )));
        }
        let input = alloc::format!("{}{query}", self.settings.query_prefix);
        let mut vectors = embedder.embed(&[input])?;
        let mut vector = vectors
            .pop()
            .ok_or_else(|| Error::Embed("no vector for the query".into()))?;
        if Some(vector.len()) != self.settings.dimensions {
            return Err(Error::Embed(alloc::format!(
                "the query's vector has {} values, the index's {:?}",
                vector.len(),
                self.settings.dimensions
            )));
        }
        normalize(&mut vector)?;
        Ok(candidates
            .iter()
            .filter_map(|&index| {
                let stored = self.chunks[index].vector.as_deref()?;
                Some((index, stored.iter().zip(&vector).map(|(a, b)| a * b).sum()))
            })
            .collect())
    }
}

fn normalize(vector: &mut [f32]) -> Result<(), Error> {
    if vector.iter().any(|value| !value.is_finite()) {
        return Err(Error::Embed(
            "a vector holds a value that is not finite".into(),
        ));
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm == 0.0 {
        return Err(Error::Embed("a vector is all zeros".into()));
    }
    for value in vector {
        *value /= norm;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Create,
    Update,
    Rebuild,
}

/// The files a write keeps from the index before, and what it makes new.
fn run(
    dir: &Path,
    op: Op,
    prior: Option<&Index>,
    mut settings: Settings,
    corpus: &Corpus,
    embedder: Option<&dyn Embedder>,
    options: &Options,
) -> Result<Report, Error> {
    if !(1..=128).contains(&options.batch_size) {
        return Err(Error::Settings(alloc::format!(
            "batch_size is {}, it must be 1 to 128",
            options.batch_size
        )));
    }
    if op != Op::Update {
        // The embedder decides what vectors the index holds; a new model means new dimensions.
        let model = embedder.map(|embedder| embedder.model().to_string());
        if model != settings.model || model.is_none() {
            settings.dimensions = None;
        }
        settings.model = model;
    }
    settings.validate()?;
    let selector = Selector::new(&settings.include, &settings.exclude)?;
    let mut report = Report {
        dry_run: options.dry_run,
        generation: prior.map(|prior| prior.generation),
        skipped: corpus.skipped.clone(),
        ..Report::default()
    };
    report.skipped.sort();

    let reusing = if op == Op::Update { prior } else { None };
    let mut kept: BTreeMap<String, Vec<Chunk>> = BTreeMap::new();
    let mut fresh: BTreeMap<String, Vec<Chunk>> = BTreeMap::new();
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    let mut selected = BTreeSet::new();
    for (path, text) in corpus.files() {
        if !selector.selects(path) {
            continue;
        }
        selected.insert(path.to_string());
        let sum = digest(text);
        let before = prior.and_then(|prior| prior.files.get(path));
        if reusing.is_some() && before == Some(&sum) {
            report.unchanged.push(path.to_string());
            kept.insert(path.to_string(), Vec::new());
        } else {
            if before.is_some() && op == Op::Update {
                report.changed.push(path.to_string());
            } else {
                report.added.push(path.to_string());
            }
            let pieces = chunk::chunk(
                text,
                chunk::is_markdown(path),
                settings.room(),
                settings.overlap_bytes,
            );
            fresh.insert(
                path.to_string(),
                pieces
                    .into_iter()
                    .map(|piece| Chunk {
                        path: path.to_string(),
                        heading: piece.heading,
                        from: piece.from,
                        to: piece.to,
                        text: piece.text,
                        vector: None,
                    })
                    .collect(),
            );
        }
        files.insert(path.to_string(), sum);
    }
    // Passages of unchanged files, and of stale ones an update retains, come from the index before.
    if let Some(prior) = prior {
        let retain_stale = op == Op::Update && !options.prune;
        for chunk in &prior.chunks {
            if let Some(slot) = kept.get_mut(&chunk.path) {
                slot.push(chunk.clone());
            }
        }
        for (path, sum) in &prior.files {
            if selected.contains(path) {
                continue;
            }
            if retain_stale {
                report.retained.push(path.clone());
                files.insert(path.clone(), sum.clone());
                kept.insert(
                    path.clone(),
                    prior
                        .chunks
                        .iter()
                        .filter(|chunk| &chunk.path == path)
                        .cloned()
                        .collect(),
                );
            } else {
                report.removed.push(path.clone());
            }
        }
    }
    report.passages_kept = kept.values().map(Vec::len).sum();
    report.passages_new = fresh.values().map(Vec::len).sum();
    if options.dry_run {
        return Ok(report);
    }

    if settings.model.is_some() && report.passages_new > 0 {
        let embedder = embedder.ok_or_else(|| {
            Error::Model(
                "this index holds vectors, and files need embedding: an embedder is needed".into(),
            )
        })?;
        if settings.model.as_deref() != Some(embedder.model()) {
            return Err(Error::Model(alloc::format!(
                "the index holds vectors of {}, not of {}; rebuild it to change the model",
                settings.model.as_deref().unwrap_or_default(),
                embedder.model()
            )));
        }
        let mut targets: Vec<&mut Chunk> = fresh.values_mut().flatten().collect();
        embed_all(&mut targets, embedder, &mut settings, options.batch_size)?;
    } else if settings.model.is_some()
        && embedder.is_some_and(|e| Some(e.model()) != settings.model.as_deref())
    {
        return Err(Error::Model(
            "the embedder is not of the index's model".into(),
        ));
    }

    let mut chunks: Vec<Chunk> = Vec::new();
    let mut paths: BTreeSet<String> = kept.keys().cloned().collect();
    paths.extend(fresh.keys().cloned());
    for path in paths {
        if let Some(list) = kept.remove(&path) {
            chunks.extend(list);
        }
        if let Some(list) = fresh.remove(&path) {
            chunks.extend(list);
        }
    }
    let meta = store::Meta::new(settings, options.source.clone(), files);
    let generation = store::commit(dir, &meta, &chunks)?;
    report.generation = Some(generation);
    Ok(report)
}

/// Embeds the passages in requests of at most `batch` passages and 16 KiB, and stores their unit vectors.
fn embed_all(
    targets: &mut [&mut Chunk],
    embedder: &dyn Embedder,
    settings: &mut Settings,
    batch: usize,
) -> Result<(), Error> {
    let inputs: Vec<String> = targets
        .iter()
        .map(|chunk| alloc::format!("{}{}", settings.document_prefix, chunk.text))
        .collect();
    let mut start = 0;
    while start < inputs.len() {
        let mut end = start;
        let mut bytes = 0;
        while end < inputs.len()
            && end - start < batch
            && (end == start || bytes + inputs[end].len() <= REQUEST_BYTES)
        {
            bytes += inputs[end].len();
            end += 1;
        }
        let vectors = embedder.embed(&inputs[start..end])?;
        if vectors.len() != end - start {
            return Err(Error::Embed(alloc::format!(
                "{} vectors for {} inputs",
                vectors.len(),
                end - start
            )));
        }
        for (target, mut vector) in targets[start..end].iter_mut().zip(vectors) {
            match settings.dimensions {
                Some(size) if size != vector.len() => {
                    return Err(Error::Embed(alloc::format!(
                        "a vector has {} values after others of {size}",
                        vector.len()
                    )));
                }
                Some(_) => {}
                None => settings.dimensions = Some(vector.len()),
            }
            normalize(&mut vector)?;
            target.vector = Some(vector);
        }
        start = end;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
