//! Knowledge: Reports of the known kinds, revised in place and searched as one body.
//!
//! A write to a known Report names the revision it read and is refused once the Report has moved
//! on ([`ReportWriteError::Changed`]). The revision it replaces is kept up to
//! `[registry.limits.reports] versions`, and a memory forgotten is removed with them.
//!
//! Who may read or write what is not decided here: `directorates::m::knowledge` does, and every
//! read goes through it. What a store offers a search is
//! [`KnowledgeStore::report_candidates`], every Report of the scopes holding one of the words,
//! whoever may read it; a database's text index only narrows that set. [`matching`] ranks what is
//! left once the caller has filtered it, so a search answers the same on every backend and no
//! order, count or page rests on a Report the asker may not read.
//!
//! The file Registry keeps each prior revision at `report-versions/<id>/<revision>.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::limits::ReportLimits;
use super::memories::words;
use super::reports::{Report, ReportScope, ReportState};
use super::store::{FileRegistry, validate_registry_key};
use crate::bounds::Limit;

pub use super::memories::search_terms;

/// What a revisioned write or a forget did not do.
#[non_exhaustive]
#[derive(Debug)]
pub enum ReportWriteError {
    /// There is no such Report, or none in this scope.
    Missing,
    /// It has moved on from the revision the write was made from.
    Changed {
        /// The revision it is at.
        revision: u64,
    },
    /// The write was refused or the store failed: `InvalidInput` for a Report out of bounds,
    /// `QuotaExceeded` for a scope that holds as much as it may.
    Failed(io::Error),
}

impl fmt::Display for ReportWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("no such report"),
            Self::Changed { revision } => {
                write!(formatter, "the report has moved on to revision {revision}")
            }
            Self::Failed(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ReportWriteError {}

impl From<ReportWriteError> for io::Error {
    fn from(error: ReportWriteError) -> Self {
        match error {
            ReportWriteError::Missing => Self::new(io::ErrorKind::NotFound, "no such report"),
            ReportWriteError::Changed { revision } => Self::new(
                io::ErrorKind::InvalidInput,
                format!("the report has moved on to revision {revision}"),
            ),
            ReportWriteError::Failed(error) => error,
        }
    }
}

impl From<io::Error> for ReportWriteError {
    fn from(error: io::Error) -> Self {
        Self::Failed(error)
    }
}

/// What a search asks for; every part is optional and each one set must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportQuery {
    /// The words of the search: a Report holding any of them in its title, summary, topics or
    /// text matches, and holding more of them ranks higher. None lists every Report.
    pub terms: Vec<String>,
    /// Only this kind.
    pub kind: Option<String>,
    /// Only Reports on each of these topics, whatever their case.
    pub topics: Vec<String>,
    /// Only Reports whose title holds each of these words.
    pub title: Vec<String>,
    /// Only this scope.
    pub scope: Option<ReportScope>,
    /// Only these states. Left out, a known kind is current only when it is `standing`, and an
    /// observation is any state.
    pub states: Option<Vec<ReportState>>,
    /// Bytes of the text the words were taken from, which a limit bounds.
    pub asked_bytes: usize,
}

impl ReportQuery {
    /// A search for the words of `text`.
    pub fn words(text: &str) -> Self {
        Self {
            terms: search_terms(text),
            asked_bytes: text.len(),
            ..Self::default()
        }
    }

    /// Only `kind`.
    #[must_use]
    pub fn of_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Only Reports on `topic`, in addition to any already asked.
    #[must_use]
    pub fn on_topic(mut self, topic: impl Into<String>) -> Self {
        self.topics.push(topic.into());
        self
    }

    /// Only Reports whose title holds every word of `text`.
    #[must_use]
    pub fn titled(mut self, text: &str) -> Self {
        self.title = search_terms(text);
        self
    }

    /// Only `scope`.
    #[must_use]
    pub fn in_scope(mut self, scope: ReportScope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Only `state`.
    #[must_use]
    pub fn in_state(mut self, state: ReportState) -> Self {
        self.states = Some(vec![state]);
        self
    }

    fn admits(&self, report: &Report) -> bool {
        let state = match &self.states {
            Some(states) => states.contains(&report.state),
            None => !report.is_known() || report.state == ReportState::Standing,
        };
        let titled: BTreeSet<String> = words(&report.title).collect();
        let topics: BTreeSet<String> = report
            .topics
            .iter()
            .map(|topic| topic.to_lowercase())
            .collect();
        state
            && self
                .scope
                .as_ref()
                .is_none_or(|scope| *scope == report.scope)
            && self.kind.as_deref().is_none_or(|kind| kind == report.kind)
            && self
                .topics
                .iter()
                .all(|topic| topics.contains(&topic.to_lowercase()))
            && self.title.iter().all(|word| titled.contains(word))
    }
}

/// The distinct words of `report` a database's text index holds, within `limit` bytes: its title,
/// summary, topics and text.
#[cfg_attr(not(any(feature = "sqlite", feature = "postgres")), allow(dead_code))]
pub(super) fn index_words(report: &Report, limit: Limit) -> String {
    let distinct: BTreeSet<String> = words(&report.title)
        .chain(words(report.summary.as_deref().unwrap_or_default()))
        .chain(report.topics.iter().flat_map(|topic| words(topic)))
        .chain(words(&report.text))
        .collect();
    let mut index = String::new();
    for word in distinct {
        if !limit.allows(index.len() + word.len() + 1) {
            break;
        }
        index.push_str(&word);
        index.push(' ');
    }
    index
}

/// `reports` that match `query`, best first; all of them, newest change first, when it has no
/// words. A Report ranks by how many of the words it holds, then how many of those are in its
/// title, summary or topics, then its newest change, then its ID. One holding none is left out.
/// Run on what the asker may read, this is every backend's search.
pub fn matching(reports: Vec<Report>, query: &ReportQuery) -> Vec<Report> {
    let mut ranked: Vec<(usize, usize, Report)> = reports
        .into_iter()
        .filter(|report| query.admits(report))
        .filter_map(|report| {
            let head: BTreeSet<String> = words(&report.title)
                .chain(words(report.summary.as_deref().unwrap_or_default()))
                .chain(report.topics.iter().flat_map(|topic| words(topic)))
                .collect();
            let body: BTreeSet<String> = words(&report.text).collect();
            let in_head = query
                .terms
                .iter()
                .filter(|term| head.contains(*term))
                .count();
            let anywhere = query
                .terms
                .iter()
                .filter(|term| head.contains(*term) || body.contains(*term))
                .count();
            (query.terms.is_empty() || anywhere > 0).then_some((anywhere, in_head, report))
        })
        .collect();
    ranked.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then(right.1.cmp(&left.1))
            .then(right.2.updated.total_cmp(&left.2.updated))
            .then_with(|| left.2.id.cmp(&right.2.id))
    });
    ranked.into_iter().map(|(_, _, report)| report).collect()
}

/// What a full scope says, telling nothing of what it holds.
pub(super) const SCOPE_FULL: &str =
    "this scope is full, so nothing was kept: forget or retire something first";

/// What a full queue of proposals says, telling nothing of what they ask.
pub(super) const PROPOSALS_FULL: &str =
    "too many proposals wait for a verdict here, so this one was not kept";

/// What a scope holds of the known kinds, for [`check_room`].
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Held {
    /// Standing Reports.
    pub(super) standing: u64,
    /// Their bytes ([`Report::size`]).
    pub(super) bytes: u64,
    /// Proposals waiting.
    pub(super) waiting: u64,
}

impl Held {
    /// What `reports` hold: only known kinds count.
    pub(super) fn of<'a>(reports: impl Iterator<Item = &'a Report>) -> Self {
        let mut held = Self::default();
        for report in reports.filter(|report| report.is_known()) {
            match report.state {
                ReportState::Standing => {
                    held.standing += 1;
                    held.bytes += report.size();
                }
                ReportState::Open => held.waiting += 1,
                _ => {}
            }
        }
        held
    }
}

/// Whether a scope that holds `held` may take `report`, in place of `replaced` (a standing Report
/// of that many bytes) when it replaces one.
///
/// # Errors
///
/// `QuotaExceeded`, in words that tell nothing of what the scope holds.
pub(super) fn check_room(
    limits: &ReportLimits,
    report: &Report,
    held: Held,
    replaced: Option<u64>,
) -> io::Result<()> {
    if !report.is_known() {
        return Ok(());
    }
    let full = |what: &str| io::Error::new(io::ErrorKind::QuotaExceeded, what.to_owned());
    match report.state {
        ReportState::Standing => {
            let after = held
                .bytes
                .saturating_sub(replaced.unwrap_or(0))
                .saturating_add(report.size());
            if (replaced.is_none() && !limits.scope_entries.allows(held.standing.saturating_add(1)))
                || !limits.scope_bytes.allows(after)
            {
                return Err(full(SCOPE_FULL));
            }
        }
        ReportState::Open if !limits.proposals.allows(held.waiting.saturating_add(1)) => {
            return Err(full(PROPOSALS_FULL));
        }
        _ => {}
    }
    Ok(())
}

/// Revisions, forgetting and search over Reports of the known kinds.
pub trait KnowledgeStore {
    /// Replaces the standing Report `next.id`, which must be in `next.scope` at revision `base`,
    /// with `next` at revision `base + 1`: its filing time stays, its kind may not change, what it
    /// held is kept as a prior revision, and a scope that cannot take it refuses. Answers it as
    /// stored.
    ///
    /// # Errors
    ///
    /// [`ReportWriteError`]: not there, moved on, out of bounds or past the scope's limits.
    fn write_report(&self, next: &Report, base: u64) -> Result<Report, ReportWriteError>;

    /// The prior revisions of the Report `id`, newest first; none for an ID that names nothing.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn report_versions(&self, id: &str) -> io::Result<Vec<Report>>;

    /// Removes the memory `id` and its prior revisions, when it is at revision `base` if one is
    /// given. Answers what it held. A Report of any other kind is never removed: it is closed.
    ///
    /// # Errors
    ///
    /// [`ReportWriteError::Missing`], [`ReportWriteError::Changed`], `InvalidInput` for another
    /// kind, or the store failing.
    fn forget_report(&self, id: &str, base: Option<u64>) -> Result<Report, ReportWriteError>;

    /// Every scope that holds a Report.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn report_scopes(&self) -> io::Result<Vec<ReportScope>>;

    /// Every Report of `scopes` that may hold one of `query`'s words, or every one when it has
    /// none: at least what [`matching`] would take, whoever may read it. The caller filters and
    /// then ranks.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn report_candidates(
        &self,
        scopes: &[ReportScope],
        query: &ReportQuery,
    ) -> io::Result<Vec<Report>>;
}

/// `next` as it is stored when it replaces `current` at revision `base`, or why not.
pub(super) fn replacing(
    limits: &ReportLimits,
    next: &Report,
    current: &Report,
    base: u64,
) -> Result<Report, ReportWriteError> {
    if current.scope != next.scope {
        return Err(ReportWriteError::Missing);
    }
    if current.revision != base {
        return Err(ReportWriteError::Changed {
            revision: current.revision,
        });
    }
    let refused = |what: &str| {
        ReportWriteError::Failed(io::Error::new(io::ErrorKind::InvalidInput, what.to_owned()))
    };
    if current.state != ReportState::Standing || next.state != ReportState::Standing {
        return Err(refused("only a standing report is revised"));
    }
    if current.kind != next.kind {
        return Err(refused("a report keeps its kind through its revisions"));
    }
    let mut stored = next.clone();
    stored.created = current.created;
    stored.revision = base + 1;
    stored.validate(limits)?;
    Ok(stored)
}

/// Why a Report is not removed by a forget, or `None` when it is a memory.
pub(super) fn unforgettable(report: &Report) -> Option<ReportWriteError> {
    (report.kind != "memory").then(|| {
        ReportWriteError::Failed(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only a memory is forgotten: a report of any other kind is retired or closed, and kept",
        ))
    })
}

impl FileRegistry {
    pub(super) fn versions_root(&self) -> PathBuf {
        self.root().join("report-versions")
    }

    /// Keeps `previous` as a prior revision and drops the oldest past `[..] versions`.
    fn keep_report_version(&self, previous: &Report) -> io::Result<()> {
        let keep = self.current_limits().reports.versions;
        let dir = self.versions_root().join(&previous.id);
        if keep.allows(1u64) {
            let bytes = serde_json::to_vec(previous).map_err(io::Error::other)?;
            self.write_atomic(&dir.join(format!("{}.json", previous.revision)), &bytes)?;
        }
        let mut kept: BTreeMap<u64, PathBuf> = BTreeMap::new();
        for path in json_files(&dir)? {
            if let Some(revision) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse().ok())
            {
                kept.insert(revision, path);
            }
        }
        while !keep.allows(kept.len()) {
            let Some((_, oldest)) = kept.pop_first() else {
                break;
            };
            fs::remove_file(oldest)?;
        }
        Ok(())
    }

    /// The stored Report `id`, with the path it is kept at.
    fn stored_report(&self, id: &str) -> io::Result<Option<(PathBuf, Report)>> {
        if validate_registry_key(id).is_err() {
            return Ok(None);
        }
        Ok(self
            .read_reports(None)?
            .into_iter()
            .find(|(_, report)| report.id == id))
    }
}

/// The `.json` files directly in `dir`; none when it is not there.
fn json_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            files.push(path);
        }
    }
    Ok(files)
}

impl KnowledgeStore for FileRegistry {
    fn write_report(&self, next: &Report, base: u64) -> Result<Report, ReportWriteError> {
        let limits = self.current_limits().reports.clone();
        next.validate(&limits)?;
        let _lock = self.lock()?;
        let (path, current) = self
            .stored_report(&next.id)?
            .ok_or(ReportWriteError::Missing)?;
        let stored = replacing(&limits, next, &current, base)?;
        let in_scope = self.read_reports(Some(&stored.scope))?;
        let held = Held::of(in_scope.iter().map(|(_, report)| report));
        check_room(&limits, &stored, held, Some(current.size()))?;
        let bytes = super::reports::report_bytes(&stored, &limits)?;
        self.keep_report_version(&current)?;
        self.write_atomic(&path, &bytes)?;
        Ok(stored)
    }

    fn report_versions(&self, id: &str) -> io::Result<Vec<Report>> {
        if validate_registry_key(id).is_err() {
            return Ok(Vec::new());
        }
        let mut versions = Vec::new();
        for path in json_files(&self.versions_root().join(id))? {
            if let Ok(bytes) = fs::read(&path)
                && let Ok(version) = serde_json::from_slice::<Report>(&bytes)
                && version.id == id
            {
                versions.push(version);
            }
        }
        versions.sort_by_key(|version| std::cmp::Reverse(version.revision));
        Ok(versions)
    }

    fn forget_report(&self, id: &str, base: Option<u64>) -> Result<Report, ReportWriteError> {
        let _lock = self.lock()?;
        let (path, report) = self.stored_report(id)?.ok_or(ReportWriteError::Missing)?;
        if let Some(refusal) = unforgettable(&report) {
            return Err(refusal);
        }
        if base.is_some_and(|base| base != report.revision) {
            return Err(ReportWriteError::Changed {
                revision: report.revision,
            });
        }
        match fs::remove_dir_all(self.versions_root().join(id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        fs::remove_file(path)?;
        Ok(report)
    }

    fn report_scopes(&self) -> io::Result<Vec<ReportScope>> {
        let mut scopes: Vec<ReportScope> = Vec::new();
        for (_, report) in self.read_reports(None)? {
            if !scopes.contains(&report.scope) {
                scopes.push(report.scope);
            }
        }
        Ok(scopes)
    }

    fn report_candidates(
        &self,
        scopes: &[ReportScope],
        _query: &ReportQuery,
    ) -> io::Result<Vec<Report>> {
        // No index here: the scan is `matching` itself.
        let mut found = Vec::new();
        for scope in scopes {
            found.extend(
                self.read_reports(Some(scope))?
                    .into_iter()
                    .map(|(_, report)| report),
            );
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests;
