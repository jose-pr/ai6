//! Assessing a Report, closing it with what resolves it, and bringing it back.
//!
//! The Operation's charge, or a person where there is none, assesses an open Report as one of
//! `mission`, `duplicate`, `related`, `no_action`, `awaiting`, `later` or `decision`, and states
//! its judgement in the words of the probability yardstick with a confidence. Every assessment is
//! audited before it is recorded. A Report assessed into something that has not ended is
//! `assessed`; it closes when that ends (its Mission settles, its green ink request is signed),
//! keeping its resolution, and `awaiting` and `later` open it again at their time or on request.
//! Closing counts toward the Report's source: the deployment's rule in
//! [`ReportSettings`] says which outcomes count and how a tally becomes a reliability.
//!
//! Nothing here starts work. A `mission` assessment of a Source item is queued by the Station
//! that polls the Source; any other names the Mission already proposed or amended for it.

use std::fmt;
use std::io;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::approvals::{ApprovalRequest, ApprovalStatus, Decision};
use super::backend::RegistryBackend;
use super::reports::{
    Reliability, Report, ReportAssessment, ReportFilter, ReportLink, ReportLinkKind,
    ReportResolution, ReportScope, ReportSource, ReportState,
};
use super::store::AuditRecord;
use super::timeline::TimelineEvent;
use crate::directorates::m::approvals::ActionScope;
use crate::directorates::m::identity::{MissionId, RunId};
use crate::directorates::m::reports::ReportSettings;

/// The audit action of an assessment.
pub const ASSESS_ACTION: &str = "report.assess";
/// The audit action of a Report that closes.
pub const CLOSE_ACTION: &str = "report.close";
/// The audit action of a Report that opens again.
pub const REOPEN_ACTION: &str = "report.reopen";
/// The audit action of a Report that crosses to a place its handling mark must allow.
pub const CROSS_ACTION: &str = "report.cross";

/// The kind of green ink request an assessment `decision` files.
pub const DECISION_KIND: &str = "decision";

/// The Run name a green ink request filed for a Report carries.
const DECISION_RUN: &str = "report";

/// The outcomes a closed Report can carry, which a deployment's rating rule counts.
pub const OUTCOMES: [&str; 7] = [
    "mission_done",
    "mission_failed",
    "decision_approved",
    "decision_rejected",
    "duplicate",
    "related",
    "no_action",
];

/// The probability yardstick's words, least to most likely.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Likelihood {
    /// Under 5%.
    RemoteChance,
    /// 10 to 20%.
    HighlyUnlikely,
    /// 25 to 35%.
    Unlikely,
    /// 40 to 50%.
    RealisticPossibility,
    /// 55 to 75%.
    Likely,
    /// 80 to 90%.
    HighlyLikely,
    /// Over 95%.
    AlmostCertain,
}

impl Likelihood {
    /// The words as the yardstick writes them: `remote chance`, `highly unlikely`, `unlikely`,
    /// `realistic possibility`, `likely`, `highly likely` or `almost certain`.
    pub fn words(self) -> &'static str {
        match self {
            Self::RemoteChance => "remote chance",
            Self::HighlyUnlikely => "highly unlikely",
            Self::Unlikely => "unlikely",
            Self::RealisticPossibility => "realistic possibility",
            Self::Likely => "likely",
            Self::HighlyLikely => "highly likely",
            Self::AlmostCertain => "almost certain",
        }
    }

    /// The yardstick's percentage band, shown beside the words and never in place of them.
    pub fn band(self) -> &'static str {
        match self {
            Self::RemoteChance => "under 5%",
            Self::HighlyUnlikely => "10-20%",
            Self::Unlikely => "25-35%",
            Self::RealisticPossibility => "40-50%",
            Self::Likely => "55-75%",
            Self::HighlyLikely => "80-90%",
            Self::AlmostCertain => "over 95%",
        }
    }
}

impl FromStr for Likelihood {
    type Err = String;

    /// The yardstick's words, spelled with spaces, hyphens or underscores, in any case.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let key = text.trim().to_ascii_lowercase().replace(['_', '-'], " ");
        [
            Self::RemoteChance,
            Self::HighlyUnlikely,
            Self::Unlikely,
            Self::RealisticPossibility,
            Self::Likely,
            Self::HighlyLikely,
            Self::AlmostCertain,
        ]
        .into_iter()
        .find(|each| each.words() == key)
        .ok_or_else(|| {
            format!(
                "{text:?} is not a probability word: use remote chance, highly unlikely, unlikely, \
                 realistic possibility, likely, highly likely or almost certain"
            )
        })
    }
}

impl fmt::Display for Likelihood {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.words())
    }
}

/// How sure the assessor is of its own judgement.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// High confidence.
    High,
    /// Moderate confidence.
    Moderate,
    /// Low confidence.
    Low,
}

impl Confidence {
    /// The wire word: `high`, `moderate` or `low`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Moderate => "moderate",
            Self::Low => "low",
        }
    }
}

impl FromStr for Confidence {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_ascii_lowercase().as_str() {
            "high" => Ok(Self::High),
            "moderate" => Ok(Self::Moderate),
            "low" => Ok(Self::Low),
            _ => Err(format!(
                "{text:?} is not a confidence: use high, moderate or low"
            )),
        }
    }
}

/// An assessor's judgement of what a Report says: a probability word and a confidence.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Judgement {
    /// How likely it is.
    pub likelihood: Likelihood,
    /// How sure the assessor is.
    pub confidence: Confidence,
}

impl Judgement {
    /// A judgement of `likelihood` held with `confidence`.
    pub fn new(likelihood: Likelihood, confidence: Confidence) -> Self {
        Self {
            likelihood,
            confidence,
        }
    }

    /// The judgement a caller words as two optional texts: both or neither.
    ///
    /// # Errors
    ///
    /// Says which is missing or not one of the words.
    pub fn parse(
        likelihood: Option<&str>,
        confidence: Option<&str>,
    ) -> Result<Option<Self>, String> {
        match (likelihood, confidence) {
            (None, None) => Ok(None),
            (Some(likelihood), Some(confidence)) => {
                Ok(Some(Self::new(likelihood.parse()?, confidence.parse()?)))
            }
            _ => Err("a judgement states both how likely it is and the confidence".to_owned()),
        }
    }

    /// The judgement as it is said: `likely (55-75%), moderate confidence`.
    pub fn statement(&self) -> String {
        format!(
            "{} ({}), {} confidence",
            self.likelihood.words(),
            self.likelihood.band(),
            self.confidence.as_str()
        )
    }
}

/// What an assessor decides a Report becomes.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Work. A Source item with no Mission named is queued as a brief by the Station that polls
    /// its Source; any other Report names the Mission proposed or amended for it.
    Mission {
        /// The Mission, by UID or handle.
        mission: Option<String>,
    },
    /// The same as a Report, a Mission or evidence, named.
    Duplicate {
        /// What it repeats.
        of: String,
    },
    /// Tied to another Report, a Mission or evidence, named, and needing nothing more.
    Related {
        /// What it is tied to.
        to: String,
    },
    /// Nothing to do.
    NoAction,
    /// A question is asked, and the Report comes back at its answer or its deadline.
    Awaiting {
        /// What was asked.
        question: String,
        /// Who or what was asked: a Cable channel, a tracker item, a person.
        of: Option<String>,
        /// How far the question goes, a handling mark that the Report's own must allow.
        via: Option<String>,
        /// The deadline, as Unix seconds.
        until: f64,
    },
    /// Not now: it comes back at a time, when a condition is met, or either.
    Later {
        /// When it comes back, as Unix seconds.
        until: Option<f64>,
        /// What it waits for.
        condition: Option<String>,
    },
    /// A decision for the people who may sign it, filed as green ink of kind `decision`.
    Decision {
        /// What is asked of them.
        question: String,
    },
}

impl Verdict {
    /// The verdict `word` names, with what it takes: `mission` the Mission, `duplicate` and
    /// `related` what they name in `of`, `awaiting` and `decision` a `question`, `awaiting` and
    /// `later` a time `wait_seconds` from `now`, and `later` a `condition`.
    ///
    /// # Errors
    ///
    /// Says what is missing, left over or not a verdict.
    pub fn parse(word: &str, parts: &VerdictParts<'_>, now: f64) -> Result<Self, String> {
        let VerdictParts {
            of,
            mission,
            question,
            via,
            ask,
            condition,
            wait_seconds,
        } = *parts;
        let leftover = |allowed: &[&str]| -> Result<(), String> {
            let given = [
                ("of", of.is_some()),
                ("mission", mission.is_some()),
                ("question", question.is_some()),
                ("via", via.is_some()),
                ("ask", ask.is_some()),
                ("condition", condition.is_some()),
                ("wait_seconds", wait_seconds.is_some()),
            ];
            match given
                .iter()
                .find(|(name, set)| *set && !allowed.contains(name))
            {
                Some((name, _)) => Err(format!("{word} does not take {name}")),
                None => Ok(()),
            }
        };
        let ahead = |wait: u64| now + wait as f64;
        match word {
            "mission" => {
                leftover(&["mission"])?;
                Ok(Self::Mission {
                    mission: mission.map(str::to_owned),
                })
            }
            "no_action" => {
                leftover(&[])?;
                Ok(Self::NoAction)
            }
            "duplicate" => {
                leftover(&["of"])?;
                Ok(Self::Duplicate {
                    of: of
                        .ok_or("a duplicate names what it repeats in `of`")?
                        .to_owned(),
                })
            }
            "related" => {
                leftover(&["of"])?;
                Ok(Self::Related {
                    to: of
                        .ok_or("a related report names what it is tied to in `of`")?
                        .to_owned(),
                })
            }
            "awaiting" => {
                leftover(&["question", "ask", "via", "wait_seconds"])?;
                Ok(Self::Awaiting {
                    question: question
                        .ok_or("awaiting takes the question asked")?
                        .to_owned(),
                    of: ask.map(str::to_owned),
                    via: via.map(str::to_owned),
                    until: ahead(wait_seconds.ok_or("awaiting takes wait_seconds, its deadline")?),
                })
            }
            "later" => {
                leftover(&["condition", "wait_seconds"])?;
                if condition.is_none() && wait_seconds.is_none() {
                    return Err("later takes wait_seconds, a condition, or both".to_owned());
                }
                Ok(Self::Later {
                    until: wait_seconds.map(ahead),
                    condition: condition.map(str::to_owned),
                })
            }
            "decision" => {
                leftover(&["question"])?;
                Ok(Self::Decision {
                    question: question
                        .ok_or("a decision takes the question for those who sign it")?
                        .to_owned(),
                })
            }
            _ => Err(
                "an assessment is mission, duplicate, related, no_action, awaiting, later or decision"
                    .to_owned(),
            ),
        }
    }

    /// The verdict's word.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Mission { .. } => "mission",
            Self::Duplicate { .. } => "duplicate",
            Self::Related { .. } => "related",
            Self::NoAction => "no_action",
            Self::Awaiting { .. } => "awaiting",
            Self::Later { .. } => "later",
            Self::Decision { .. } => "decision",
        }
    }
}

/// What a caller says of a verdict, as the tools and routes take it.
#[derive(Debug, Clone, Copy, Default)]
pub struct VerdictParts<'a> {
    /// What a `duplicate` repeats, or a `related` Report is tied to.
    pub of: Option<&'a str>,
    /// The Mission of a `mission`.
    pub mission: Option<&'a str>,
    /// The question of an `awaiting` or a `decision`.
    pub question: Option<&'a str>,
    /// How far an `awaiting` question goes.
    pub via: Option<&'a str>,
    /// Who or what an `awaiting` question is put to.
    pub ask: Option<&'a str>,
    /// What a `later` waits for.
    pub condition: Option<&'a str>,
    /// Seconds from now to an `awaiting` deadline or a `later` date.
    pub wait_seconds: Option<u64>,
}

/// What `by` assesses a Report as.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct Assessment<'a> {
    /// The Report's ID.
    pub report: &'a str,
    /// What it becomes.
    pub verdict: Verdict,
    /// Why; kept as the assessor's comment, apart from the Report's own words.
    pub reason: &'a str,
    /// Who assessed: a person ID, or `charge:operation:<id>`.
    pub by: &'a str,
    /// When, as Unix seconds.
    pub at: f64,
    /// How far the information is believed, `1` to `6`; unset leaves it as it was.
    pub credibility: Option<u8>,
    /// How urgent, from the deployment's priorities; unset leaves it as it was.
    pub priority: Option<&'a str>,
    /// How likely the assessor judges what the Report says.
    pub judgement: Option<Judgement>,
}

impl<'a> Assessment<'a> {
    /// An assessment of `report` as `verdict` by `by`, for `reason`.
    pub fn new(report: &'a str, verdict: Verdict, reason: &'a str, by: &'a str, at: f64) -> Self {
        Self {
            report,
            verdict,
            reason,
            by,
            at,
            credibility: None,
            priority: None,
            judgement: None,
        }
    }

    /// Sets the credibility.
    #[must_use]
    pub fn believed(mut self, credibility: Option<u8>) -> Self {
        self.credibility = credibility;
        self
    }

    /// Sets the priority.
    #[must_use]
    pub fn prioritised(mut self, priority: Option<&'a str>) -> Self {
        self.priority = priority;
        self
    }

    /// States the judgement.
    #[must_use]
    pub fn judged(mut self, judgement: Option<Judgement>) -> Self {
        self.judgement = judgement;
        self
    }
}

/// What an assessment is judged and carried out against.
pub struct Context<'a> {
    /// The deployment's lists, handling marks and rating rule.
    pub settings: &'a ReportSettings,
    /// Whether the assessor reads a scope: a Report in another scope is tied to this one only
    /// when it does, and only as far as the Report's handling mark goes.
    pub reads: &'a dyn Fn(&ReportScope) -> bool,
    /// How long a green ink request of a `decision` waits for its signature.
    pub request_ttl: Duration,
    /// The Agent name a `decision` request is filed under.
    pub agent: &'a str,
}

impl<'a> Context<'a> {
    /// A context that lets the assessor read only the scope of the Report it assesses.
    pub fn within(settings: &'a ReportSettings, reads: &'a dyn Fn(&ReportScope) -> bool) -> Self {
        Self {
            settings,
            reads,
            request_ttl: crate::directorates::m::approvals::DEFAULT_REQUEST_TTL,
            agent: "report",
        }
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

/// The Operation of `scope` and the audit resource that names it.
fn audit_scope(scope: &ReportScope) -> (String, String) {
    match scope {
        ReportScope::Operation(id) => (id.clone(), format!("operation:{id}")),
        ReportScope::Station(id) => (String::new(), format!("station:{id}")),
        ReportScope::M => (String::new(), "config:reports".to_owned()),
    }
}

fn audit(
    store: &dyn RegistryBackend,
    action: &str,
    scope: &ReportScope,
    by: &str,
    target: String,
    at: f64,
) -> io::Result<()> {
    let (operation, resource) = audit_scope(scope);
    store.record_audit(&AuditRecord {
        id: uuid::Uuid::new_v4().to_string(),
        actor: by.to_owned(),
        action: action.to_owned(),
        operation,
        resource: Some(resource),
        target,
        at,
    })
}

fn timeline(store: &dyn RegistryBackend, scope: &ReportScope, word: &str, by: &str, at: f64) {
    if let ReportScope::Operation(operation) = scope {
        let _ = store.record_event(&TimelineEvent::new("report", word, operation, at).actor(by));
    }
}

/// What a name in `of` is, as a link: a Report, a Mission or, failing both, evidence.
fn link_to(store: &dyn RegistryBackend, name: &str) -> io::Result<(ReportLink, Option<Report>)> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("a link names what it points at"));
    }
    if let Some(other) = store.report(name)? {
        return Ok((ReportLink::new(ReportLinkKind::Report, name), Some(other)));
    }
    if let Some(handle) = store.resolve_mission(name)? {
        return Ok((ReportLink::mission(handle.mission_id), None));
    }
    Ok((ReportLink::new(ReportLinkKind::Evidence, name), None))
}

/// Refuses a tie to a Report of another scope unless the assessor reads that scope and the
/// Report's handling mark lets it go as far as the compartments; audits the crossing it allows.
fn check_crossing(
    store: &dyn RegistryBackend,
    report: &Report,
    other: &Report,
    a: &Assessment<'_>,
    cx: &Context<'_>,
) -> io::Result<()> {
    if other.scope == report.scope {
        return Ok(());
    }
    if !(cx.reads)(&other.scope) {
        return Err(invalid("that report is in a scope you may not read"));
    }
    cx.settings
        .permits(report, "compartments")
        .and_then(|()| cx.settings.permits(other, "compartments"))
        .map_err(invalid)?;
    audit(
        store,
        CROSS_ACTION,
        &report.scope,
        a.by,
        format!("{} to {}", report.id, other.scope),
        a.at,
    )
}

fn resolution(by: &str, at: f64, outcome: &str, note: String) -> ReportResolution {
    let mut resolution = ReportResolution::new(by, at);
    resolution.outcome = Some(outcome.to_owned());
    resolution.note = Some(note);
    resolution
}

/// Assesses an open Report: audited first, then kept on the Report as the assessment with its
/// reason, the judgement, the credibility and priority, the links the verdict makes (both ways
/// for a tie to a Report), and on the Operation's timeline. `duplicate`, `related` and
/// `no_action` close it; the others leave it `assessed` until what it waits for ends.
///
/// # Errors
///
/// `NotFound` for no such Report; `InvalidInput` for one that is not open, a missing reason, a
/// verdict out of bounds (a time not ahead, a Mission that is not the Operation's, a question
/// that goes further than the Report's handling mark, a priority outside the list), or a tie
/// across scopes the assessor may not make; an audit that cannot be written, or a Registry failure.
pub fn assess(
    store: &dyn RegistryBackend,
    a: &Assessment<'_>,
    cx: &Context<'_>,
) -> io::Result<Report> {
    let stored = store
        .report(a.report)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such report"))?;
    if stored.state != ReportState::Open {
        return Err(invalid(format!(
            "this report is {}: only an open report is assessed",
            stored.state.as_str()
        )));
    }
    if a.reason.trim().is_empty() {
        return Err(invalid("an assessment needs its reason"));
    }
    if let Some(priority) = a.priority
        && !cx.settings.priorities.iter().any(|each| each == priority)
    {
        return Err(invalid(
            "a priority is one of the deployment's, most urgent first",
        ));
    }
    if a.credibility.is_some_and(|value| !(1..=6).contains(&value)) {
        return Err(invalid("credibility is 1 to 6"));
    }
    let operation = stored.scope.operation().map(str::to_owned);
    // What the verdict ties the Report to counts toward `1`, so it is read before the change.
    let corroborated = |links: &[ReportLink]| {
        links
            .iter()
            .any(|link| matches!(link.kind, ReportLinkKind::Report | ReportLinkKind::Evidence))
    };
    let mut tie: Option<ReportLink> = None;
    let mut other: Option<Report> = None;
    match &a.verdict {
        Verdict::Duplicate { of: name } | Verdict::Related { to: name } => {
            let (link, found) = link_to(store, name)?;
            if link.kind == ReportLinkKind::Report && link.id == stored.id {
                return Err(invalid(
                    "a report is not a duplicate of, or tied to, itself",
                ));
            }
            if let Some(found) = &found {
                check_crossing(store, &stored, found, a, cx)?;
            }
            if let (ReportLinkKind::Mission, Some(operation)) = (link.kind, &operation) {
                check_mission_scope(store, &link.id, operation)?;
            }
            tie = Some(link);
            other = found;
        }
        Verdict::Mission { mission } => match mission {
            Some(name) => {
                let handle = store
                    .resolve_mission(name.trim())?
                    .ok_or_else(|| invalid("no Mission goes by that name"))?;
                if let Some(operation) = &operation {
                    check_mission_scope(store, &handle.mission_id, operation)?;
                }
                tie = Some(ReportLink::mission(handle.mission_id));
            }
            None if stored.item.is_some() => {}
            None => {
                return Err(invalid(
                    "a mission assessment names the Mission proposed or amended for the report",
                ));
            }
        },
        Verdict::Awaiting {
            question,
            via,
            until,
            ..
        } => {
            if question.trim().is_empty() {
                return Err(invalid("awaiting takes the question asked"));
            }
            if !until.is_finite() || *until <= a.at {
                return Err(invalid("awaiting needs a deadline ahead"));
            }
            if let Some(via) = via {
                cx.settings.permits(&stored, via).map_err(invalid)?;
            }
        }
        Verdict::Later { until, condition } => {
            if until.is_some_and(|until| !until.is_finite() || until <= a.at) {
                return Err(invalid("later needs a time ahead"));
            }
            if until.is_none()
                && condition
                    .as_deref()
                    .is_none_or(|text| text.trim().is_empty())
            {
                return Err(invalid("later needs a time ahead or a condition"));
            }
        }
        Verdict::Decision { question } => {
            if question.trim().is_empty() {
                return Err(invalid(
                    "a decision takes the question for those who sign it",
                ));
            }
            if operation.is_none() {
                return Err(invalid(
                    "a decision is asked of the people who may sign in an Operation",
                ));
            }
        }
        Verdict::NoAction => {}
    }
    if a.credibility == Some(1)
        && !corroborated(&stored.links)
        && !tie
            .as_ref()
            .is_some_and(|link| corroborated(std::slice::from_ref(link)))
    {
        return Err(invalid(
            "credibility 1 (confirmed) needs a link to the corroborating report or evidence",
        ));
    }
    let word = a.verdict.word();
    // Audited first: an assessment that cannot be audited is not given.
    audit(
        store,
        ASSESS_ACTION,
        &stored.scope,
        a.by,
        format!("{} {word}", stored.id),
        a.at,
    )?;
    if let Verdict::Awaiting { via: Some(via), .. } = &a.verdict {
        audit(
            store,
            CROSS_ACTION,
            &stored.scope,
            a.by,
            format!("{} to {via}", stored.id),
            a.at,
        )?;
    }
    if let Verdict::Decision { question } = &a.verdict {
        let request = decision_request(store, &stored, question, a, cx)?;
        tie = Some(ReportLink::new(ReportLinkKind::Decision, request.id));
    }
    // The other Report names this one back, so each lists the other.
    if let Some(other) = &other {
        let back = ReportLink::new(ReportLinkKind::Report, stored.id.clone());
        store.change_report(&other.id, &mut |report| {
            if !report.links.contains(&back) {
                report.links.push(back.clone());
            }
            Ok(())
        })?;
    }
    let mut outcome: Option<&'static str> = None;
    let report = store.change_report(&stored.id, &mut |report| {
        if report.state != ReportState::Open {
            return Err(invalid("this report was assessed meanwhile"));
        }
        let mut assessment = ReportAssessment::new(word, a.by, a.at);
        assessment.comment = Some(a.reason.to_owned());
        assessment.judgement = a.judgement;
        report.updated = a.at.max(report.created);
        if let Some(credibility) = a.credibility {
            report.credibility = Some(credibility);
        }
        if let Some(priority) = a.priority {
            report.priority = Some(priority.to_owned());
        }
        if let Some(link) = &tie
            && !report.links.contains(link)
        {
            report.links.push(link.clone());
        }
        let mut close = |report: &mut Report, kind: &'static str, note: String| {
            outcome = Some(kind);
            report.state = ReportState::Closed;
            report.resolution = Some(resolution(a.by, a.at, kind, note));
        };
        match &a.verdict {
            Verdict::Mission { .. } | Verdict::Decision { .. } => {
                report.state = ReportState::Assessed;
                if let Verdict::Decision { question } = &a.verdict {
                    assessment.waits_for = Some(question.clone());
                }
            }
            Verdict::Awaiting {
                question,
                of,
                until,
                ..
            } => {
                report.state = ReportState::Assessed;
                report.deadline = Some(*until);
                assessment.until = Some(*until);
                assessment.waits_for = Some(match of {
                    Some(of) => format!("{question} (asked of {of})"),
                    None => question.clone(),
                });
            }
            Verdict::Later { until, condition } => {
                report.state = ReportState::Assessed;
                if let Some(until) = until {
                    report.deadline = Some(*until);
                }
                assessment.until = *until;
                assessment.waits_for = condition.clone();
            }
            Verdict::Duplicate { of } => {
                close(report, "duplicate", format!("duplicate of {}", of.trim()));
            }
            Verdict::Related { to } => {
                close(report, "related", format!("related to {}", to.trim()));
            }
            Verdict::NoAction => close(report, "no_action", a.reason.to_owned()),
        }
        report.assessment = Some(assessment);
        Ok(())
    })?;
    timeline(store, &report.scope, word, a.by, a.at);
    if let Some(outcome) = outcome {
        rate(store, &report, outcome, cx.settings, a.at)?;
    }
    Ok(report)
}

/// Refuses a Mission that is not `operation`'s.
fn check_mission_scope(
    store: &dyn RegistryBackend,
    mission: &str,
    operation: &str,
) -> io::Result<()> {
    let mine = store
        .filed_brief(mission)?
        .is_some_and(|filed| filed.brief.operation.as_deref() == Some(operation));
    if mine {
        Ok(())
    } else {
        Err(invalid("that Mission is not in the report's Operation"))
    }
}

/// Files the green ink request a `decision` assessment asks: kind `decision`, its target the
/// Report, its reason the question. Asking again for the same Report in the same second finds
/// the request that exists.
fn decision_request(
    store: &dyn RegistryBackend,
    report: &Report,
    question: &str,
    a: &Assessment<'_>,
    cx: &Context<'_>,
) -> io::Result<ApprovalRequest> {
    let operation = report
        .scope
        .operation()
        .ok_or_else(|| invalid("a decision is asked in an Operation"))?;
    let limits = store.limits().approvals.clone();
    let text = question.trim();
    let mut cut = text.len().min(limits.reason_bytes.or_max());
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let to_identity = |_| invalid("a report's ID names the request's Mission");
    let request = ApprovalRequest::try_new(
        operation,
        MissionId::try_new(report.id.clone()).map_err(to_identity)?,
        RunId::try_new(DECISION_RUN).map_err(to_identity)?,
        cx.agent,
        ActionScope::try_new(DECISION_KIND, format!("report:{}", report.id))?,
        &text[..cut],
        &format!("{}#{}", report.id, a.at as u64),
        &limits,
    )?
    .expiring(a.at, cx.request_ttl);
    match store.request_approval(&request)? {
        ApprovalStatus::Pending(stored) => Ok(stored),
        ApprovalStatus::Decided(approval) => Ok(approval.request),
    }
}

/// Counts the closing of `report` toward its source, by the deployment's rule, and re-rates the
/// source when its tally reaches the number of Reports the rule asks for. An outcome the rule
/// does not count leaves the source as it was.
///
/// # Errors
///
/// An error from the Registry.
pub fn rate(
    store: &dyn RegistryBackend,
    report: &Report,
    outcome: &str,
    settings: &ReportSettings,
    at: f64,
) -> io::Result<()> {
    if !settings.rating_counts.iter().any(|each| each == outcome) {
        return Ok(());
    }
    let mut source = store.report_source(&report.filer.source_key())?;
    source.counted = source.counted.saturating_add(1);
    if settings.rating_confirms.iter().any(|each| each == outcome) {
        source.confirmed = source.confirmed.saturating_add(1);
    }
    if source.counted >= settings.rating_min_reports.max(1) {
        source.reliability =
            reliability_of(source.confirmed, source.counted, &settings.rating_bands);
        source.rated = at;
    }
    store.put_report_source(&source)
}

/// The reliability `confirmed` of `counted` earns: the first letter among `A` to `D` whose band
/// (percent) it reaches, else `E`.
pub fn reliability_of(confirmed: u32, counted: u32, bands: &[u8]) -> Reliability {
    let percent = if counted == 0 {
        0.0
    } else {
        f64::from(confirmed) * 100.0 / f64::from(counted)
    };
    let letters = [
        Reliability::A,
        Reliability::B,
        Reliability::C,
        Reliability::D,
    ];
    letters
        .into_iter()
        .zip(bands)
        .find(|(_, band)| percent >= f64::from(**band))
        .map_or(Reliability::E, |(letter, _)| letter)
}

/// A source's record and the closed Reports its rating rests on.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct SourceStanding {
    /// The record: its reliability and tally.
    pub source: ReportSource,
    /// Its closed Reports, with the outcome of each in its resolution, most urgent first.
    pub behind: Vec<Report>,
}

/// The source `key` with the closed Reports behind its rating. Reports dropped after
/// `keep_closed` are gone from the list but stay in the tally.
///
/// # Errors
///
/// An error from the Registry.
pub fn standing(store: &dyn RegistryBackend, key: &str) -> io::Result<SourceStanding> {
    Ok(SourceStanding {
        source: store.report_source(key)?,
        behind: store.reports(
            &ReportFilter::all()
                .from_source(key)
                .in_state(ReportState::Closed),
        )?,
    })
}

/// Opens an assessed or closed Report again, audited first, with `why`; its assessment stays as
/// the last one given and its resolution is dropped. This is how an answer to an `awaiting`
/// question, or a met `later` condition, brings a Report back.
///
/// # Errors
///
/// `NotFound` for no such Report; `InvalidInput` for one already open or no reason; an audit that
/// cannot be written, or a Registry failure.
pub fn reopen(
    store: &dyn RegistryBackend,
    id: &str,
    by: &str,
    why: &str,
    at: f64,
) -> io::Result<Report> {
    let stored = store
        .report(id)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such report"))?;
    if stored.state == ReportState::Open {
        return Err(invalid("this report is open already"));
    }
    if why.trim().is_empty() {
        return Err(invalid("reopening a report needs its reason"));
    }
    audit(
        store,
        REOPEN_ACTION,
        &stored.scope,
        by,
        format!("{id} {}", one_line(why)),
        at,
    )?;
    let report = store.change_report(id, &mut |report| {
        if report.state == ReportState::Open {
            return Err(invalid("this report was opened meanwhile"));
        }
        report.state = ReportState::Open;
        report.resolution = None;
        report.updated = at.max(report.created);
        Ok(())
    })?;
    timeline(store, &report.scope, "reopened", by, at);
    Ok(report)
}

fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut end = flat.trim().len().min(200);
    while !flat.trim().is_char_boundary(end) {
        end -= 1;
    }
    flat.trim()[..end].to_owned()
}

/// Opens again the assessed Reports `filter` matches whose `awaiting` deadline or `later` time
/// has come, audited, and returns them.
///
/// # Errors
///
/// An error from the Registry.
pub fn reopen_due(
    store: &dyn RegistryBackend,
    filter: ReportFilter,
    now: f64,
) -> io::Result<Vec<Report>> {
    let mut opened = Vec::new();
    for report in store.reports(&filter.in_state(ReportState::Assessed))? {
        if let Some(why) = due(&report, now) {
            opened.extend(reopen_if_assessed(store, &report, why, now)?);
        }
    }
    Ok(opened)
}

/// Why an assessed Report is due to open again at `now`, when it is.
fn due(report: &Report, now: f64) -> Option<&'static str> {
    let assessment = report.assessment.as_ref()?;
    if !assessment.until.is_some_and(|until| until <= now) {
        return None;
    }
    match assessment.verdict.as_str() {
        "awaiting" => Some("deadline reached"),
        "later" => Some("time reached"),
        _ => None,
    }
}

/// What a pass found: Reports closed because what they were assessed into ended, and Reports
/// opened again.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Swept {
    /// The Reports it closed.
    pub closed: Vec<Report>,
    /// The Reports it opened again, at their deadline or time, or because nobody signed.
    pub reopened: Vec<Report>,
}

/// Brings every assessed Report up to date: closes those whose Mission has settled or whose
/// green ink request is signed, and opens again those whose `awaiting` deadline or `later` time
/// has come, or whose request expired unsigned. Each is audited. Safe to run from every Station
/// as often as it likes: a Report that is not assessed any more is left alone.
///
/// # Errors
///
/// An error from the Registry.
pub fn sweep(
    store: &dyn RegistryBackend,
    now: f64,
    settings: &ReportSettings,
) -> io::Result<Swept> {
    sweep_matching(store, ReportFilter::all(), now, settings)
}

/// [`sweep`], over the assessed Reports `filter` matches.
///
/// # Errors
///
/// An error from the Registry.
pub fn sweep_matching(
    store: &dyn RegistryBackend,
    filter: ReportFilter,
    now: f64,
    settings: &ReportSettings,
) -> io::Result<Swept> {
    let mut swept = Swept::default();
    for report in store.reports(&filter.in_state(ReportState::Assessed))? {
        let Some(assessment) = &report.assessment else {
            continue;
        };
        match assessment.verdict.as_str() {
            "mission" => {
                if let Some((outcome, note)) = mission_ended(store, &report)? {
                    swept.closed.extend(close(
                        store,
                        &report,
                        "mission",
                        (outcome, note),
                        now,
                        settings,
                    )?);
                }
            }
            "decision" => match decision_state(store, &report, now)? {
                Some(Signed::Closed(outcome, note)) => {
                    swept.closed.extend(close(
                        store,
                        &report,
                        "decision",
                        (outcome, note),
                        now,
                        settings,
                    )?);
                }
                Some(Signed::Unsigned) => {
                    swept.reopened.extend(reopen_if_assessed(
                        store,
                        &report,
                        "request expired unsigned",
                        now,
                    )?);
                }
                None => {}
            },
            _ => {
                if let Some(why) = due(&report, now) {
                    swept
                        .reopened
                        .extend(reopen_if_assessed(store, &report, why, now)?);
                }
            }
        }
    }
    Ok(swept)
}

/// Closes the Reports assessed `mission` that name the Mission `mission_id` once it has settled:
/// the same as [`sweep`] but for one Mission, for the Station that has just filed its outcome.
///
/// # Errors
///
/// An error from the Registry.
pub fn mission_settled(
    store: &dyn RegistryBackend,
    mission_id: &str,
    now: f64,
    settings: &ReportSettings,
) -> io::Result<Vec<Report>> {
    Ok(sweep_matching(
        store,
        ReportFilter::all().about_mission(mission_id),
        now,
        settings,
    )?
    .closed)
}

/// How the Missions of a Report assessed `mission` stand: `None` until every one has settled,
/// then the outcome of the last one linked and what it says.
fn mission_ended(
    store: &dyn RegistryBackend,
    report: &Report,
) -> io::Result<Option<(&'static str, String)>> {
    let mut last = None;
    for link in report
        .links
        .iter()
        .filter(|link| link.kind == ReportLinkKind::Mission)
    {
        let uid = store
            .resolve_mission(&link.id)?
            .map_or_else(|| link.id.clone(), |handle| handle.mission_id);
        let Some(filed) = store.filed_brief(&uid)? else {
            return Ok(None);
        };
        // A brief filed lost, because its Station went dark, may still be settled by it.
        let lost = filed
            .outcome
            .as_ref()
            .is_some_and(|outcome| outcome["state"] == "lost");
        match (filed.state.as_str(), lost) {
            ("done", _) => last = Some(("mission_done", format!("Mission {uid} settled done"))),
            ("failed", false) => {
                last = Some(("mission_failed", format!("Mission {uid} settled failed")));
            }
            _ => return Ok(None),
        }
    }
    Ok(last)
}

enum Signed {
    /// Approved or rejected: the outcome and what it says.
    Closed(&'static str, String),
    /// Nobody signed before the request expired.
    Unsigned,
}

/// Where the green ink request of a Report assessed `decision` stands.
fn decision_state(
    store: &dyn RegistryBackend,
    report: &Report,
    now: f64,
) -> io::Result<Option<Signed>> {
    let (Some(operation), Some(link)) = (
        report.scope.operation(),
        report
            .links
            .iter()
            .rev()
            .find(|link| link.kind == ReportLinkKind::Decision),
    ) else {
        return Ok(None);
    };
    Ok(match store.approval(operation, &link.id, now)? {
        Some(ApprovalStatus::Decided(approval)) => Some(match approval.decision {
            Decision::Approved => Signed::Closed(
                "decision_approved",
                format!("approved by {}", approval.signer),
            ),
            Decision::Rejected => Signed::Closed(
                "decision_rejected",
                format!("rejected by {}", approval.signer),
            ),
            _ => Signed::Unsigned,
        }),
        _ => None,
    })
}

/// Closes `report` if it is still assessed `verdict`, audited, and counts it toward its source.
fn close(
    store: &dyn RegistryBackend,
    report: &Report,
    verdict: &str,
    (outcome, note): (&'static str, String),
    at: f64,
    settings: &ReportSettings,
) -> io::Result<Option<Report>> {
    const BY: &str = "station:resolution";
    audit(
        store,
        CLOSE_ACTION,
        &report.scope,
        BY,
        format!("{} {outcome}", report.id),
        at,
    )?;
    let mut applied = false;
    let closed = store.change_report(&report.id, &mut |report| {
        let still = report.state == ReportState::Assessed
            && report
                .assessment
                .as_ref()
                .is_some_and(|assessment| assessment.verdict == verdict);
        if still {
            applied = true;
            report.state = ReportState::Closed;
            report.resolution = Some(resolution(BY, at, outcome, note.clone()));
            report.updated = at.max(report.created);
        }
        Ok(())
    })?;
    if !applied {
        return Ok(None);
    }
    timeline(store, &closed.scope, "closed", BY, at);
    rate(store, &closed, outcome, settings, at)?;
    Ok(Some(closed))
}

/// Opens `report` again if it is still assessed.
fn reopen_if_assessed(
    store: &dyn RegistryBackend,
    report: &Report,
    why: &str,
    at: f64,
) -> io::Result<Option<Report>> {
    const BY: &str = "station:clock";
    audit(
        store,
        REOPEN_ACTION,
        &report.scope,
        BY,
        format!("{} {why}", report.id),
        at,
    )?;
    let mut applied = false;
    let opened = store.change_report(&report.id, &mut |report| {
        if report.state == ReportState::Assessed {
            applied = true;
            report.state = ReportState::Open;
            report.deadline = None;
            report.updated = at.max(report.created);
        }
        Ok(())
    })?;
    if !applied {
        return Ok(None);
    }
    timeline(store, &opened.scope, "reopened", BY, at);
    Ok(Some(opened))
}

#[cfg(test)]
mod tests;
