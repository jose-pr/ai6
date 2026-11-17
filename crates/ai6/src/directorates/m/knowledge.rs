//! Who may read, write and forget what AI6 knows, asked at every call.
//!
//! What AI6 knows is a Report of a known kind (a memory, tradecraft, a rule, a contact): the same
//! record as an observation, in the scopes `person:ID` and `global` as well as an Operation's.
//! Every read goes through this module and through [`sight`], whether it is a read of one Report,
//! a search or a starting context, so what a caller cannot see is the same everywhere. A Report
//! the caller may not read answers as an ID that does not exist. A search filters before it ranks
//! or counts: nothing it answers rests on a Report it hid.
//!
//! A scope is written by someone who may write it: a person for their own, a person with
//! `memory.write` for an Operation's or the global one, an Agent at the Operation's clearance.
//! What a caller may only propose is filed `open` and waits for the verdict `accepted`
//! (`registry::assessments`). A Report written by an exposed Run stays marked, and whoever reads
//! it is told: a Run is exposed by it, as by a Cable. A person's own edit clears the mark,
//! audited. A memory is forgotten for good, with its revisions; tradecraft, a rule and a contact
//! are retired, closed and kept.
//!
//! Universal Exports asks as a person, and as Moneypenny for one; a Station asks as a Run. Each
//! says what it holds by implementing [`KnowledgeCaller`], read afresh for the call.

use std::collections::BTreeSet;
use std::io;

use crate::bounds::Limit;
use crate::directorates::m::guides;
use crate::directorates::m::reports::{
    ReportReader, ReportRules, ReportSight, new_report_id, sight,
};
use crate::directorates::medical::exposure::report_source;
use crate::directorates::registry::assessments::{self, Assessment, Context, Verdict};
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::knowledge::{ReportQuery, ReportWriteError, matching};
use crate::directorates::registry::limits::ReportLimits;
use crate::directorates::registry::logbook::Severity;
use crate::directorates::registry::reports::{
    RETIRED, Report, ReportFiler, ReportProposes, ReportResolution, ReportScope, ReportState,
    is_known_kind,
};
use serde_json::{Value, json};

/// The audit action of a change to a Report of a known kind.
pub const AUDIT_WRITE: &str = "memory.write";
/// The audit action of forgetting a memory or retiring another known kind.
pub const AUDIT_FORGET: &str = "memory.forget";
/// The audit action of a read of another person's memory.
pub const AUDIT_READ: &str = "memory.read";
/// The audit action of a change to a Report's compartments.
pub const AUDIT_COMPARTMENTS: &str = "memory.compartments";
/// The audit action of a person's edit that cleared a Report's mark.
pub const AUDIT_MARK_CLEARED: &str = "memory.mark_cleared";
/// The audit action of a proposal kept for a verdict.
pub const AUDIT_PROPOSED: &str = "memory.proposed";
/// The audit resource of a person's memory.
pub const PERSON_RESOURCE: &str = "config:assistant";
/// The audit resource of the global scope, where `memory.write` is granted.
pub const GLOBAL_RESOURCE: &str = "config:memory";

/// What a caller may do to a scope's known Reports.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeRight {
    /// Change them at once.
    Write,
    /// Ask for a verdict.
    Propose,
    /// Nothing.
    Refused,
}

/// Who is asking, and what they hold now. Implementations read what they hold for this call and
/// fail closed: what cannot be read is not held. Reading is [`ReportReader`]'s, with `assesses`
/// and `filed` answered for proposals.
pub trait KnowledgeCaller: ReportReader {
    /// Who a write is recorded as: `person:ID`, `agent:CODENAME` or `moneypenny:ID`.
    fn by(&self) -> String;

    /// How a Report this caller writes names its filer.
    fn filer(&self) -> ReportFiler;

    /// The Run or conversation a write comes from, for the audit record.
    fn run(&self) -> Option<String>;

    /// What the caller may do to the scope's known Reports.
    fn writes_scope(&self, scope: &ReportScope) -> KnowledgeRight;

    /// Whether a person is acting for themselves, so an edit clears a mark: never a Run, nor
    /// Moneypenny for a person.
    fn acts_in_person(&self) -> bool;

    /// How what is asking has been exposed, which a write it makes carries.
    fn exposures(&self) -> Vec<String>;

    /// Tells the caller it was shown what an exposed Run wrote; `source` says which.
    fn saw_marked(&self, source: &str);

    /// Bytes of one exposure source (`[registry.limits.runs] exposure_source_bytes`).
    fn mark_bytes(&self) -> Limit;

    /// The time now, as Unix seconds.
    fn now(&self) -> f64;

    /// Records `action` on `target` in `scope`, before it happens.
    ///
    /// # Errors
    ///
    /// Why it could not be recorded; nothing then happens.
    fn audit(&self, action: &str, scope: &ReportScope, target: &str) -> Result<(), String>;
}

/// Why a call did nothing, in the words the caller sees. [`Missing`](Self::Missing) is one answer
/// for a Report that is not there and one the caller may not read.
#[non_exhaustive]
#[derive(Debug)]
pub enum KnowledgeRefusal {
    /// There is no such Report, as far as the caller may know.
    Missing,
    /// The caller may not do this.
    Forbidden(String),
    /// What was asked is out of bounds.
    Invalid(String),
    /// The scope holds as much as it may.
    Full(String),
    /// The Report has moved on from the revision the write was made from.
    Changed {
        /// The revision it is at.
        revision: u64,
    },
    /// The store or the audit log failed.
    Failed(String),
}

impl std::fmt::Display for KnowledgeRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => formatter.write_str("There is no such report."),
            Self::Forbidden(reason)
            | Self::Invalid(reason)
            | Self::Full(reason)
            | Self::Failed(reason) => formatter.write_str(reason),
            Self::Changed { revision } => write!(
                formatter,
                "The report changed since it was read, so nothing was saved: it is at revision \
                 {revision}. Read it again and make the change from what it holds now."
            ),
        }
    }
}

impl std::error::Error for KnowledgeRefusal {}

impl From<io::Error> for KnowledgeRefusal {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::InvalidInput => Self::Invalid(error.to_string()),
            io::ErrorKind::QuotaExceeded => Self::Full(error.to_string()),
            io::ErrorKind::AlreadyExists => Self::Changed { revision: 1 },
            _ => Self::Failed(format!("The reports could not be reached: {error}")),
        }
    }
}

impl From<ReportWriteError> for KnowledgeRefusal {
    fn from(error: ReportWriteError) -> Self {
        match error {
            ReportWriteError::Missing => Self::Missing,
            ReportWriteError::Changed { revision } => Self::Changed { revision },
            ReportWriteError::Failed(error) => error.into(),
        }
    }
}

/// Tells `caller` of each of `reports` an exposed Run wrote.
fn note_marks<'a>(caller: &dyn KnowledgeCaller, reports: impl Iterator<Item = &'a Report>) {
    for report in reports.filter(|report| !report.exposures.is_empty()) {
        caller.saw_marked(&report_source(&report.id, caller.mark_bytes()));
    }
}

/// Whether `caller` reads the whole of `report`: the one question every read asks.
fn readable(caller: &dyn KnowledgeCaller, rules: &ReportRules<'_>, report: &Report) -> bool {
    sight(caller, report, rules) == ReportSight::Content
}

/// The Report `id`, when the caller reads it, without telling them of a mark.
fn find(
    store: &dyn RegistryBackend,
    caller: &dyn KnowledgeCaller,
    rules: &ReportRules<'_>,
    id: &str,
) -> Result<Report, KnowledgeRefusal> {
    let report = guides::find(id)
        .cloned()
        .or_else(|| store.report(id).ok().flatten())
        .ok_or(KnowledgeRefusal::Missing)?;
    if !caller.reads_known_scope(&report.scope) || !readable(caller, rules, &report) {
        return Err(KnowledgeRefusal::Missing);
    }
    Ok(report)
}

/// The Report `id` of a known kind, when the caller reads it; a mark on it exposes the caller.
///
/// # Errors
///
/// [`KnowledgeRefusal::Missing`] for one that is not there, is not a known kind, or that the
/// caller may not read.
pub fn read_report(
    store: &dyn RegistryBackend,
    caller: &dyn KnowledgeCaller,
    rules: &ReportRules<'_>,
    id: &str,
) -> Result<Report, KnowledgeRefusal> {
    let report = find(store, caller, rules, id)?;
    note_marks(caller, std::iter::once(&report));
    Ok(report)
}

/// The prior revisions of the Report `id`, newest first, when the caller reads the Report.
///
/// # Errors
///
/// As [`read_report`].
pub fn prior_revisions(
    store: &dyn RegistryBackend,
    caller: &dyn KnowledgeCaller,
    rules: &ReportRules<'_>,
    id: &str,
) -> Result<Vec<Report>, KnowledgeRefusal> {
    read_report(store, caller, rules, id)?;
    let versions = store.report_versions(id)?;
    note_marks(caller, versions.iter());
    Ok(versions)
}

/// What a search found.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct KnowledgeSearch {
    /// The Reports it answers, best first, at most `search_results`.
    pub hits: Vec<Report>,
    /// How many the caller may read matched, whether or not all are in `hits`.
    pub total: usize,
}

/// Searches every scope the caller reads (or the one `query` names) for what `query` asks:
/// what the caller may read, then ranked, then counted, then cut to `limits.search_results`. With
/// no words it is every Report that matches the rest, newest change first.
///
/// # Errors
///
/// A query past `limits.query_bytes`, or a store that cannot be read.
pub fn search_reports(
    store: &dyn RegistryBackend,
    caller: &dyn KnowledgeCaller,
    rules: &ReportRules<'_>,
    query: &ReportQuery,
    limits: &ReportLimits,
) -> Result<KnowledgeSearch, KnowledgeRefusal> {
    search_reports_among(store, caller, rules, query, limits, &[])
}

/// [`search_reports`] over `extra` as well: Reports that are not stored, read as the guides the
/// build ships are, by whoever reads `global`.
///
/// # Errors
///
/// As [`search_reports`].
pub fn search_reports_among(
    store: &dyn RegistryBackend,
    caller: &dyn KnowledgeCaller,
    rules: &ReportRules<'_>,
    query: &ReportQuery,
    limits: &ReportLimits,
    extra: &[Report],
) -> Result<KnowledgeSearch, KnowledgeRefusal> {
    if !limits.query_bytes.allows(query.asked_bytes) {
        return Err(KnowledgeRefusal::Invalid(format!(
            "A search asks for at most {} bytes.",
            limits.query_bytes
        )));
    }
    let readable_scopes: Vec<ReportScope> = store
        .report_scopes()?
        .into_iter()
        .filter(|scope| query.scope.as_ref().is_none_or(|only| only == scope))
        .filter(|scope| match query.kind.as_deref() {
            Some(kind) if is_known_kind(kind) => caller.reads_known_scope(scope),
            Some(_) => caller.reads_scope(scope),
            None => caller.reads_scope(scope) || caller.reads_known_scope(scope),
        })
        .collect();
    let mut candidates = store.report_candidates(&readable_scopes, query)?;
    if caller.reads_known_scope(&ReportScope::Global) {
        candidates.extend(guides::built_in().iter().chain(extra).cloned());
    }
    let seen: Vec<Report> = candidates
        .into_iter()
        .filter(|report| readable(caller, rules, report))
        .collect();
    let ranked = matching(seen, query);
    let total = ranked.len();
    let hits: Vec<Report> = ranked
        .into_iter()
        .take(limits.search_results.or_max())
        .collect();
    note_marks(caller, hits.iter());
    Ok(KnowledgeSearch { hits, total })
}

/// What a write changes.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnowledgeTarget {
    /// A new Report of `kind` in `scope`.
    New {
        /// Where it goes.
        scope: ReportScope,
        /// What it is: one of the known kinds.
        kind: String,
    },
    /// The Report `id`, made from `revision`.
    Existing {
        /// Its ID.
        id: String,
        /// The revision the write was made from.
        revision: u64,
    },
}

/// A write to one Report of a known kind.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeChange {
    /// What it changes.
    pub target: KnowledgeTarget,
    /// One line naming it.
    pub title: String,
    /// One line saying what it is for, listed beside the title.
    pub summary: Option<String>,
    /// Free words a search and a facet use.
    pub topics: Vec<String>,
    /// Its whole new text.
    pub text: String,
    /// The compartments protecting it; `None` leaves them as they are, and a new Report without.
    pub compartments: Option<Vec<String>>,
}

impl KnowledgeChange {
    /// A change of `target` to this title and text.
    pub fn new(target: KnowledgeTarget, title: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            target,
            title: title.into(),
            summary: None,
            topics: Vec::new(),
            text: text.into(),
            compartments: None,
        }
    }

    /// This change with a one-line summary.
    #[must_use]
    pub fn summarised(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// This change on `topics`.
    #[must_use]
    pub fn on_topics(mut self, topics: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.topics = topics.into_iter().map(Into::into).collect();
        self
    }

    /// This change setting `compartments`, when given.
    #[must_use]
    pub fn with_compartments(mut self, compartments: Option<Vec<String>>) -> Self {
        self.compartments = compartments;
        self
    }
}

/// A write that happened.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct KnowledgeWritten {
    /// The Report as stored.
    pub report: Report,
    /// The text it held before, for a Report that was there.
    pub before: Option<String>,
}

/// What a write came to.
#[derive(Debug, Clone, PartialEq)]
pub enum KnowledgeOutcome {
    /// It is kept, `standing`.
    Kept(KnowledgeWritten),
    /// It waits for a verdict, `open`.
    Proposed(Report),
}

/// What a forget came to.
#[derive(Debug, Clone, PartialEq)]
pub enum KnowledgeForgotten {
    /// A memory is gone, with its revisions.
    Forgotten(Report),
    /// Tradecraft, a rule or a contact is closed as retired, and kept.
    Retired(Report),
    /// It waits for a verdict.
    Proposed(Report),
}

/// Whether the caller reads every compartment in `compartments`: each of them, as for every
/// Report.
fn admits(caller: &dyn KnowledgeCaller, compartments: &[String]) -> bool {
    compartments
        .iter()
        .all(|compartment| caller.read_in(compartment))
}

/// Refuses a change of a Report's compartments from `before` to `after` the caller may not make:
/// adding or removing one it is not read in to, or hiding the Report from itself.
fn check_compartments(
    caller: &dyn KnowledgeCaller,
    before: &[String],
    after: &[String],
) -> Result<(), KnowledgeRefusal> {
    let (before_set, after_set): (BTreeSet<&String>, BTreeSet<&String>) =
        (before.iter().collect(), after.iter().collect());
    if before_set
        .symmetric_difference(&after_set)
        .any(|compartment| !caller.read_in(compartment))
    {
        return Err(KnowledgeRefusal::Forbidden(
            "You add or remove only a compartment you are read in to.".to_owned(),
        ));
    }
    if before_set != after_set && !admits(caller, after) {
        return Err(KnowledgeRefusal::Forbidden(
            "That would hide the report from you.".to_owned(),
        ));
    }
    Ok(())
}

/// What an audit record names: the Report, and the Run or conversation it came from.
fn audit_target(id: &str, caller: &dyn KnowledgeCaller) -> String {
    match caller.run() {
        Some(run) => format!("{id} from {run}"),
        None => id.to_owned(),
    }
}

/// The exposures a write by `caller` carries: its own, and a Report's already held unless a
/// person edits it.
fn marks_of(caller: &dyn KnowledgeCaller, held: &[String]) -> Vec<String> {
    let mut exposures = caller.exposures();
    if !caller.acts_in_person() {
        exposures.extend(held.iter().cloned());
    }
    exposures.sort_unstable();
    exposures.dedup();
    exposures
}

fn is_known(kind: &str) -> Result<(), KnowledgeRefusal> {
    if is_known_kind(kind) {
        Ok(())
    } else {
        Err(KnowledgeRefusal::Invalid(
            "A kind of knowledge is memory, tradecraft, rule or contact.".to_owned(),
        ))
    }
}

/// Makes `change` as the caller, audited first: kept `standing` at once, or filed `open` as a
/// proposal when that is all the caller may do. A person's own edit clears a mark, audited;
/// anyone else's keeps it, and adds how they were exposed.
///
/// # Errors
///
/// [`KnowledgeRefusal`]: a Report the caller may not read is [`KnowledgeRefusal::Missing`], a scope
/// they may not write or a compartment they may not set is [`KnowledgeRefusal::Forbidden`].
pub fn write_report(
    store: &dyn RegistryBackend,
    caller: &dyn KnowledgeCaller,
    rules: &ReportRules<'_>,
    change: KnowledgeChange,
) -> Result<KnowledgeOutcome, KnowledgeRefusal> {
    let at = caller.now();
    let limits = store.limits().reports.clone();
    let (scope, kind, current, revision) = match &change.target {
        KnowledgeTarget::New { scope, kind } => {
            is_known(kind)?;
            (scope.clone(), kind.clone(), None, None)
        }
        KnowledgeTarget::Existing { id, revision } => {
            let current = find(store, caller, rules, id)?;
            if !current.is_known() || current.state != ReportState::Standing {
                return Err(KnowledgeRefusal::Missing);
            }
            (
                current.scope.clone(),
                current.kind.clone(),
                Some(current),
                Some(*revision),
            )
        }
    };
    let right = caller.writes_scope(&scope);
    if right == KnowledgeRight::Refused {
        return Err(KnowledgeRefusal::Forbidden(format!(
            "You may not change what is known in {scope}."
        )));
    }
    let before: Vec<String> = current
        .as_ref()
        .map(|report| report.compartments.clone())
        .unwrap_or_default();
    let mut after = change
        .compartments
        .clone()
        .unwrap_or_else(|| before.clone());
    after.sort();
    after.dedup();
    check_compartments(caller, &before, &after)?;
    let held = current
        .as_ref()
        .map(|report| report.exposures.as_slice())
        .unwrap_or_default();
    let exposures = marks_of(caller, held);

    let mut next = Report::new(
        current
            .as_ref()
            .map_or_else(new_report_id, |report| report.id.clone()),
        scope.clone(),
        kind,
        Severity::Info,
        change.title,
        change.text,
        caller.filer(),
        at,
    )
    .on_topics(change.topics)
    .in_compartments(after.clone())
    .exposed_by(exposures);
    next.summary = change.summary;
    next.updated_by = Some(caller.by());

    if right == KnowledgeRight::Propose {
        next.id = new_report_id();
        next.state = ReportState::Open;
        next.proposes = current
            .as_ref()
            .map(|report| ReportProposes::rewrite(report.id.clone(), report.revision))
            .zip(revision)
            .map(|(proposes, revision)| ReportProposes {
                revision,
                ..proposes
            });
        next.validate(&limits)
            .map_err(|error| KnowledgeRefusal::Invalid(error.to_string()))?;
        caller
            .audit(AUDIT_PROPOSED, &scope, &audit_target(&next.id, caller))
            .map_err(KnowledgeRefusal::Failed)?;
        store.file_report(&next)?;
        return Ok(KnowledgeOutcome::Proposed(next));
    }

    next.state = ReportState::Standing;
    next.validate(&limits)
        .map_err(|error| KnowledgeRefusal::Invalid(error.to_string()))?;
    let id = next.id.clone();
    caller
        .audit(AUDIT_WRITE, &scope, &audit_target(&id, caller))
        .map_err(KnowledgeRefusal::Failed)?;
    if before != next.compartments {
        caller
            .audit(
                AUDIT_COMPARTMENTS,
                &scope,
                &format!("{id} [{}] to [{}]", before.join(","), after.join(",")),
            )
            .map_err(KnowledgeRefusal::Failed)?;
    }
    if current
        .as_ref()
        .is_some_and(|report| !report.exposures.is_empty())
        && next.exposures.is_empty()
    {
        caller
            .audit(AUDIT_MARK_CLEARED, &scope, &id)
            .map_err(KnowledgeRefusal::Failed)?;
    }
    let report = match revision {
        None => {
            store.file_report(&next)?;
            next
        }
        Some(revision) => store.write_report(&next, revision)?,
    };
    Ok(KnowledgeOutcome::Kept(KnowledgeWritten {
        report,
        before: current.map(|report| report.text),
    }))
}

/// Forgets or retires the Report `id` as the caller, audited first; `revision`, when given, is
/// the one it was read at. A memory goes with its revisions; tradecraft, a rule and a contact are
/// closed with the resolution `retired` and kept. Where the caller may only propose, it is a
/// proposal.
///
/// # Errors
///
/// As [`write_report`].
pub fn forget_report(
    store: &dyn RegistryBackend,
    caller: &dyn KnowledgeCaller,
    rules: &ReportRules<'_>,
    id: &str,
    revision: Option<u64>,
) -> Result<KnowledgeForgotten, KnowledgeRefusal> {
    let current = find(store, caller, rules, id)?;
    if !current.is_known() || current.state != ReportState::Standing {
        return Err(KnowledgeRefusal::Missing);
    }
    let scope = current.scope.clone();
    let limits = store.limits().reports.clone();
    match caller.writes_scope(&scope) {
        KnowledgeRight::Refused => Err(KnowledgeRefusal::Forbidden(format!(
            "You may not change what is known in {scope}."
        ))),
        KnowledgeRight::Propose => {
            let asked = revision.unwrap_or(current.revision);
            let mut proposal = Report::new(
                new_report_id(),
                scope.clone(),
                current.kind.clone(),
                Severity::Info,
                current.title.clone(),
                String::new(),
                caller.filer(),
                caller.now(),
            )
            .exposed_by(marks_of(caller, &[]))
            .proposing(ReportProposes::forget(current.id.clone(), asked));
            proposal.summary = current.summary.clone();
            proposal.updated_by = Some(caller.by());
            proposal
                .validate(&limits)
                .map_err(|error| KnowledgeRefusal::Invalid(error.to_string()))?;
            caller
                .audit(AUDIT_PROPOSED, &scope, &audit_target(&proposal.id, caller))
                .map_err(KnowledgeRefusal::Failed)?;
            store.file_report(&proposal)?;
            Ok(KnowledgeForgotten::Proposed(proposal))
        }
        KnowledgeRight::Write => {
            caller
                .audit(AUDIT_FORGET, &scope, &audit_target(id, caller))
                .map_err(KnowledgeRefusal::Failed)?;
            if current.kind == "memory" {
                return Ok(KnowledgeForgotten::Forgotten(
                    store.forget_report(id, revision)?,
                ));
            }
            if revision.is_some_and(|revision| revision != current.revision) {
                return Err(KnowledgeRefusal::Changed {
                    revision: current.revision,
                });
            }
            let (by, at) = (caller.by(), caller.now());
            let retired = store.change_report(id, &mut |report| {
                report.state = ReportState::Closed;
                let mut resolution = ReportResolution::new(by.clone(), at);
                resolution.outcome = Some(RETIRED.to_owned());
                resolution.note = Some("retired".to_owned());
                report.resolution = Some(resolution);
                report.updated = at.max(report.created);
                report.updated_by = Some(by.clone());
                Ok(())
            })?;
            Ok(KnowledgeForgotten::Retired(retired))
        }
    }
}

/// `text` on one line within `limit` bytes, for an audit record.
pub fn bounded(text: &str, limit: Limit) -> String {
    let line: String = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let mut end = line.len().min(limit.or_max());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_owned()
}

/// `value` fenced as data for a model: `<` is escaped inside the JSON, so nothing in a Report can
/// close the element.
pub fn fenced(tool: &str, value: &Value) -> String {
    let body = serde_json::to_string_pretty(value)
        .unwrap_or_default()
        .replace('<', "\\u003c");
    format!("<ai6-data tool=\"{tool}\">\n{body}\n</ai6-data>")
}

/// A Report of a known kind as tools and routes show it: its fields and its text. What a reader
/// may see of an observation is [`report_json`](crate::directorates::m::reports::report_json).
pub fn knowledge_json(report: &Report) -> Value {
    json!({
        "id": report.id,
        "scope": report.scope.to_string(),
        "kind": report.kind,
        "state": report.state.as_str(),
        "title": report.title,
        "summary": report.summary,
        "topics": report.topics,
        "text": report.text,
        "revision": report.revision,
        "created": report.created,
        "updated": report.updated,
        "updated_by": report.updated_by,
        "filer": report.filer,
        "exposed": !report.exposures.is_empty(),
        "exposures": report.exposures,
        "compartments": report.compartments,
        "proposes": report.proposes,
        "untrusted": report.untrusted(),
    })
}

/// A Report of a known kind in a listing: never its text.
pub fn brief_json(report: &Report) -> Value {
    json!({
        "id": report.id,
        "scope": report.scope.to_string(),
        "kind": report.kind,
        "state": report.state.as_str(),
        "title": report.title,
        "summary": report.summary,
        "topics": report.topics,
        "revision": report.revision,
        "updated": report.updated,
        "exposed": !report.exposures.is_empty(),
    })
}

/// What a model is given of `reports`, fenced as data, within `bytes`: the first that fit, and how
/// many did not. `None` when there is nothing to say.
pub fn summaries_for_a_model(
    reports: &[Report],
    bytes: Limit,
    fence: impl Fn(&Value) -> String,
) -> Option<String> {
    if reports.is_empty() || bytes == Limit::at(0) {
        return None;
    }
    let mut told: Vec<Value> = Vec::new();
    let mut said = None;
    for report in reports {
        told.push(json!({
            "id": report.id,
            "scope": report.scope.to_string(),
            "kind": report.kind,
            "title": report.title,
            "summary": report.summary,
            "revision": report.revision,
            "exposed": !report.exposures.is_empty(),
        }));
        let more = reports.len() - told.len();
        let text = fence(&json!({"reports": told, "not_shown": more}));
        if !bytes.allows(text.len()) {
            told.pop();
            break;
        }
        said = Some(text);
    }
    said
}

/// The standing Reports of `scope` the caller reads, newest change first; a mark on one exposes the
/// caller.
///
/// # Errors
///
/// When the store cannot be read.
pub fn list_reports(
    store: &dyn RegistryBackend,
    caller: &dyn KnowledgeCaller,
    rules: &ReportRules<'_>,
    scope: &ReportScope,
) -> Result<Vec<Report>, KnowledgeRefusal> {
    if !caller.reads_known_scope(scope) {
        return Ok(Vec::new());
    }
    let query = ReportQuery::default().in_scope(scope.clone());
    let seen: Vec<Report> = store
        .report_candidates(std::slice::from_ref(scope), &query)?
        .into_iter()
        .filter(|report| readable(caller, rules, report))
        .collect();
    let listed = matching(seen, &query);
    note_marks(caller, listed.iter());
    Ok(listed)
}

/// Decides the proposal `id` as someone who assesses its scope: `accept` makes it standing, or
/// applies it as its target's next revision, forgets or retires its target, and closes it; not
/// accepted, it is closed `no_action`. Audited first, as every assessment is. A proposal the
/// caller may not read answers as one that does not exist.
///
/// # Errors
///
/// [`KnowledgeRefusal`]: [`KnowledgeRefusal::Missing`] for a proposal not there or not readable,
/// [`KnowledgeRefusal::Forbidden`] for a caller who does not assess its scope, and what the
/// assessment refuses (a target that has moved on).
pub fn decide_proposal(
    store: &dyn RegistryBackend,
    caller: &dyn KnowledgeCaller,
    rules: &ReportRules<'_>,
    id: &str,
    accept: bool,
    reason: &str,
) -> Result<Report, KnowledgeRefusal> {
    let proposal = find(store, caller, rules, id)?;
    if !proposal.is_proposal() {
        return Err(KnowledgeRefusal::Missing);
    }
    if !caller.assesses(&proposal.scope) {
        return Err(KnowledgeRefusal::Forbidden(format!(
            "You may not decide what is proposed in {}.",
            proposal.scope
        )));
    }
    let verdict = if accept {
        Verdict::Accepted
    } else {
        Verdict::NoAction
    };
    let by = caller.by();
    let assessment = Assessment::new(id, verdict, reason, &by, caller.now());
    let reads = |scope: &ReportScope| caller.reads_known_scope(scope);
    let context = Context::within(rules.settings, &reads);
    Ok(assessments::assess(store, &assessment, &context)?)
}

#[cfg(test)]
mod tests;
