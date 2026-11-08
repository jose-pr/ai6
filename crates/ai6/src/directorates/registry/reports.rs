//! Reports: what anyone or anything observed, as one record in the Registry.
//!
//! A Report names its scope (an Operation, a Station or M), what it is about (a kind, a
//! severity, a title and a text), who or what filed it, and what it is tied to. It starts `open`
//! and is never deleted by AI6 before `[registry.limits.reports] keep_closed`. Its text is
//! untrusted unless a person filed it. Filing a Report creates no Mission, brief or placement;
//! only a proposal or a person does.
//!
//! It carries markings the filer sets: a classification and a handling mark from the lists in
//! the settings, and compartments. Who may read what is `ai6::directorates::m::reports`.
//!
//! File Registry: `reports/SCOPE/ID.json` and `report-sources/KEY.json`.

use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::limits::ReportLimits;
use super::logbook::{Severity, station_key};
use super::store::{FileRegistry, invalid_data, validate_registry_key, validate_station_id};
use crate::directorates::m::reports::ReportSettings;

/// What a Report is filed against.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ReportScope {
    /// An Operation.
    Operation(String),
    /// A Station.
    Station(String),
    /// M's own scope, for what belongs to no Operation or Station.
    M,
}

impl ReportScope {
    /// The directory a scope's Reports are kept in.
    fn directory(&self) -> String {
        match self {
            Self::Operation(id) => format!("operation-{id}"),
            Self::Station(id) => format!("station-{}", station_key(id)),
            Self::M => "m".to_owned(),
        }
    }

    /// The Operation, when it is one.
    pub fn operation(&self) -> Option<&str> {
        match self {
            Self::Operation(id) => Some(id),
            _ => None,
        }
    }

    fn validate(&self) -> io::Result<()> {
        match self {
            Self::Operation(id) => validate_registry_key(id),
            Self::Station(id) => validate_station_id(id),
            Self::M => Ok(()),
        }
    }
}

impl fmt::Display for ReportScope {
    /// `operation:ID`, `station:ID` or `m`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Operation(id) => write!(f, "operation:{id}"),
            Self::Station(id) => write!(f, "station:{id}"),
            Self::M => f.write_str("m"),
        }
    }
}

impl FromStr for ReportScope {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.split_once(':') {
            Some(("operation", id)) if !id.is_empty() => Ok(Self::Operation(id.to_owned())),
            Some(("station", id)) if !id.is_empty() => Ok(Self::Station(id.to_owned())),
            None if text == "m" => Ok(Self::M),
            _ => Err(format!(
                "{text:?} is not a scope: use operation:ID, station:ID or m"
            )),
        }
    }
}

impl Serialize for ReportScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ReportScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Where a Report stands.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportState {
    /// Filed and not yet assessed.
    Open,
    /// Assessed into something that has not ended.
    Assessed,
    /// What it was assessed into ended, or it needed nothing.
    Closed,
}

impl ReportState {
    /// The name used on the wire: `open`, `assessed` or `closed`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Assessed => "assessed",
            Self::Closed => "closed",
        }
    }
}

impl FromStr for ReportState {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "open" => Ok(Self::Open),
            "assessed" => Ok(Self::Assessed),
            "closed" => Ok(Self::Closed),
            other => Err(format!(
                "{other:?} is not a state: use open, assessed or closed"
            )),
        }
    }
}

impl fmt::Display for ReportState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who or what filed a Report.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReportFiler {
    /// A tracker or feed and one of its items.
    Source {
        /// The Source instance.
        source: String,
        /// The provider-local item.
        item: String,
    },
    /// An Agent, in the Session of a Mission.
    Agent {
        /// The Agent codename.
        agent: String,
        /// The provider Session, or the Mission's ID until the harness has reported one.
        session: String,
        /// The Mission.
        mission: String,
    },
    /// A standing watch's Agent, in the watch's own Session: no Mission.
    Watch {
        /// The Agent codename.
        agent: String,
        /// The watch's scope (`m`, `operation:P`, `station:S`).
        scope: String,
        /// The provider Session, or the scope until the harness has reported one.
        session: String,
    },
    /// A person, by subject.
    Person {
        /// The person's ID.
        person: String,
    },
    /// A probe's check of a deliverable.
    Probe {
        /// The probe's name.
        probe: String,
    },
    /// A review of a Mission.
    Review {
        /// The reviewed Mission.
        mission: String,
    },
    /// The Watchdog.
    Watchdog,
}

impl ReportFiler {
    /// Whether a person wrote the text. Everything else is untrusted.
    pub fn is_person(&self) -> bool {
        matches!(self, Self::Person { .. })
    }

    /// The Session of an Agent's filing, which the rate limit counts.
    pub fn session(&self) -> Option<&str> {
        match self {
            Self::Agent { session, .. } | Self::Watch { session, .. } => Some(session),
            _ => None,
        }
    }

    /// The key of the source whose reliability this filer carries: `source:NAME`,
    /// `agent:CODENAME`, `person:ID`, `probe:NAME`, `review` or `watchdog`.
    pub fn source_key(&self) -> String {
        match self {
            Self::Source { source, .. } => format!("source:{source}"),
            Self::Agent { agent, .. } | Self::Watch { agent, .. } => format!("agent:{agent}"),
            Self::Person { person } => format!("person:{person}"),
            Self::Probe { probe } => format!("probe:{probe}"),
            Self::Review { .. } => "review".to_owned(),
            Self::Watchdog => "watchdog".to_owned(),
        }
    }

    fn texts(&self) -> Vec<&str> {
        match self {
            Self::Source { source, item } => vec![source, item],
            Self::Agent {
                agent,
                session,
                mission,
            } => vec![agent, session, mission],
            Self::Watch {
                agent,
                scope,
                session,
            } => vec![agent, scope, session],
            Self::Person { person } => vec![person],
            Self::Probe { probe } => vec![probe],
            Self::Review { mission } => vec![mission],
            Self::Watchdog => Vec::new(),
        }
    }
}

/// What a link points at.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportLinkKind {
    /// A Mission, by its ID.
    Mission,
    /// Another Report.
    Report,
    /// A decision: a green ink request.
    Decision,
    /// Evidence outside the Registry: a probe's check, a tracker read back.
    Evidence,
}

impl ReportLinkKind {
    /// The name used on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mission => "mission",
            Self::Report => "report",
            Self::Decision => "decision",
            Self::Evidence => "evidence",
        }
    }
}

impl FromStr for ReportLinkKind {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "mission" => Ok(Self::Mission),
            "report" => Ok(Self::Report),
            "decision" => Ok(Self::Decision),
            "evidence" => Ok(Self::Evidence),
            other => Err(format!(
                "{other:?} is not a link: use mission, report, decision or evidence"
            )),
        }
    }
}

/// A tie from a Report to a Mission, another Report, a decision or evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportLink {
    /// What it points at.
    pub kind: ReportLinkKind,
    /// The Mission, Report or request ID, or what names the evidence.
    pub id: String,
}

impl ReportLink {
    /// A link of `kind` to `id`.
    pub fn new(kind: ReportLinkKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }

    /// A link to the Mission `id`.
    pub fn mission(id: impl Into<String>) -> Self {
        Self::new(ReportLinkKind::Mission, id)
    }
}

/// What the scope's watch decided to do with a Report. Kept for the assessment that writes it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportAssessment {
    /// The assessment's name.
    pub verdict: String,
    /// Who assessed it.
    pub by: String,
    /// When, as Unix seconds.
    pub at: f64,
    /// The assessor's comment, kept apart from the text as it was filed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// How a Report ended. Kept for the closing that writes it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportResolution {
    /// Who or what closed it.
    pub by: String,
    /// When, as Unix seconds.
    pub at: f64,
    /// What resolved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A sanitised part of a Report, releasable at a lower marking than the Report's own: those
/// cleared for its marking but not the Report's read it in the Report's place.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tearline {
    /// What may be released.
    pub text: String,
    /// Its classification, no higher than the Report's; unset is the lowest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<String>,
    /// The compartments a reader of it must be read in to, each one of the Report's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compartments: Vec<String>,
}

impl Tearline {
    /// A tearline releasable to everyone who may read the Report's scope.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            classification: None,
            compartments: Vec::new(),
        }
    }

    /// Classifies it.
    #[must_use]
    pub fn classified(mut self, classification: impl Into<String>) -> Self {
        self.classification = Some(classification.into());
        self
    }

    /// Restricts it to readers read in to `compartments`.
    #[must_use]
    pub fn in_compartments(mut self, compartments: Vec<String>) -> Self {
        self.compartments = compartments;
        self
    }
}

/// One observation, with its markings, its ties and its state.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    /// The Report's ID.
    pub id: String,
    /// What it is filed against.
    pub scope: ReportScope,
    /// What it is about: a short label such as `disk`, `finding` or `idea`.
    pub kind: String,
    /// How much it matters.
    pub severity: Severity,
    /// One line naming it; content, like the text.
    pub title: String,
    /// What was observed, in the filer's words.
    pub text: String,
    /// Who or what filed it.
    pub filer: ReportFiler,
    /// The Missions, Reports, decisions and evidence it is tied to.
    #[serde(default)]
    pub links: Vec<ReportLink>,
    /// Who passed the information on to the filer, when the filer is not its origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_source: Option<String>,
    /// When the information is from, as Unix seconds, as against `created`, when it was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    /// How far the information is believed, `1` (confirmed by other sources) to `6` (cannot be
    /// judged); unset until assessed. `1` needs a corroborating Report or evidence link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credibility: Option<u8>,
    /// The classification a reader's clearance must meet, from the `roster` record's
    /// `reports.classifications`; unset is the lowest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<String>,
    /// How far it may go, from `reports.handlings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handling: Option<String>,
    /// The compartments a reader must be read in to, each of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compartments: Vec<String>,
    /// What those cleared for its own marking read in the Report's place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tearline: Option<Tearline>,
    /// How urgent, from `reports.priorities`: the filer's proposal until assessed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// When it needs an answer by, as Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<f64>,
    /// Where it stands.
    pub state: ReportState,
    /// What the watch decided, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<ReportAssessment>,
    /// How it ended, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ReportResolution>,
    /// When it was filed, as Unix seconds.
    pub created: f64,
    /// When it last changed, as Unix seconds.
    pub updated: f64,
}

impl Report {
    /// An open Report of `kind` in `scope`, filed at `at` (Unix seconds).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        scope: ReportScope,
        kind: impl Into<String>,
        severity: Severity,
        title: impl Into<String>,
        text: impl Into<String>,
        filer: ReportFiler,
        at: f64,
    ) -> Self {
        Self {
            id: id.into(),
            scope,
            kind: kind.into(),
            severity,
            title: title.into(),
            text: text.into(),
            filer,
            links: Vec::new(),
            sub_source: None,
            from: None,
            credibility: None,
            classification: None,
            handling: None,
            compartments: Vec::new(),
            tearline: None,
            priority: None,
            deadline: None,
            state: ReportState::Open,
            assessment: None,
            resolution: None,
            created: at,
            updated: at,
        }
    }

    /// Adds a link.
    #[must_use]
    pub fn linked(mut self, link: ReportLink) -> Self {
        self.links.push(link);
        self
    }

    /// Names who passed the information on to the filer.
    #[must_use]
    pub fn via(mut self, sub_source: impl Into<String>) -> Self {
        self.sub_source = Some(sub_source.into());
        self
    }

    /// Says when the information is from, as Unix seconds.
    #[must_use]
    pub fn information_from(mut self, at: f64) -> Self {
        self.from = Some(at);
        self
    }

    /// Sets the credibility, `1` to `6`.
    #[must_use]
    pub fn believed(mut self, credibility: u8) -> Self {
        self.credibility = Some(credibility);
        self
    }

    /// Classifies it.
    #[must_use]
    pub fn classified(mut self, classification: impl Into<String>) -> Self {
        self.classification = Some(classification.into());
        self
    }

    /// Marks how far it may go.
    #[must_use]
    pub fn handled(mut self, handling: impl Into<String>) -> Self {
        self.handling = Some(handling.into());
        self
    }

    /// Restricts it to readers read in to every one of `compartments`.
    #[must_use]
    pub fn in_compartments(mut self, compartments: Vec<String>) -> Self {
        self.compartments = compartments;
        self
    }

    /// Gives it a tearline.
    #[must_use]
    pub fn with_tearline(mut self, tearline: Tearline) -> Self {
        self.tearline = Some(tearline);
        self
    }

    /// Proposes a priority.
    #[must_use]
    pub fn prioritised(mut self, priority: impl Into<String>) -> Self {
        self.priority = Some(priority.into());
        self
    }

    /// Sets the deadline, as Unix seconds.
    #[must_use]
    pub fn due(mut self, deadline: f64) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Whether its text is untrusted: everything but a person's.
    pub fn untrusted(&self) -> bool {
        !self.filer.is_person()
    }

    /// Whether it is tied to the Mission `id`.
    pub fn about_mission(&self, id: &str) -> bool {
        self.links
            .iter()
            .any(|link| link.kind == ReportLinkKind::Mission && link.id == id)
    }

    /// Checks the Report against `limits`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` naming what is out of bounds: an ID or scope that is not a key, an empty
    /// or long kind, title or text, too many links, a long ID or name, a tearline in a
    /// compartment the Report is not in, credibility `1` with no corroborating link, or a time
    /// that is not finite. Whether its markings and priority are in the deployment's lists is
    /// `ReportSettings::check`, since those lists are configuration, not Registry bounds.
    pub fn validate(&self, limits: &ReportLimits) -> io::Result<()> {
        validate_registry_key(&self.id)?;
        self.scope.validate()?;
        let bad = |message: &str| io::Error::new(io::ErrorKind::InvalidInput, message.to_owned());
        let named = |text: &str| {
            !text.trim().is_empty()
                && limits.name_bytes.allows(text.len())
                && !text.contains(char::is_control)
        };
        if !named(&self.kind)
            || !self
                .kind
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err(bad(
                "a report's kind is a short label of letters, digits, '-' or '_'",
            ));
        }
        if !named(&self.title) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "a report's title is one line of 1 to {} bytes",
                    limits.name_bytes
                ),
            ));
        }
        let text_in_bounds =
            |text: &str| !text.trim().is_empty() && limits.text_bytes.allows(text.len());
        if !text_in_bounds(&self.text) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("a report's text must be 1 to {} bytes", limits.text_bytes),
            ));
        }
        if !limits.links.allows(self.links.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("a report carries at most {} links", limits.links),
            ));
        }
        if self.links.iter().any(|link| !named(&link.id))
            || self.filer.texts().into_iter().any(|text| !named(text))
        {
            return Err(bad(
                "a report's links and filer carry names, each in bounds",
            ));
        }
        if !limits.links.allows(self.compartments.len())
            || self.compartments.iter().any(|name| !named(name))
            || self.sub_source.as_deref().is_some_and(|name| !named(name))
        {
            return Err(bad(
                "a report's compartments and sub-source are names in bounds",
            ));
        }
        if let Some(tearline) = &self.tearline {
            if !text_in_bounds(&tearline.text) {
                return Err(bad("a tearline's text is in the bounds of a report's text"));
            }
            let within = tearline
                .compartments
                .iter()
                .all(|name| self.compartments.contains(name));
            if !within {
                return Err(bad("a tearline's compartments are its report's"));
            }
        }
        match self.credibility {
            None | Some(2..=6) => {}
            Some(1) => {
                let corroborated = self.links.iter().any(|link| {
                    matches!(link.kind, ReportLinkKind::Report | ReportLinkKind::Evidence)
                });
                if !corroborated {
                    return Err(bad(
                        "credibility 1 (confirmed) needs a link to the corroborating report or evidence",
                    ));
                }
            }
            Some(_) => return Err(bad("credibility is 1 to 6")),
        }
        if self.deadline.is_some_and(|at| !at.is_finite() || at <= 0.0)
            || self.from.is_some_and(|at| !at.is_finite() || at <= 0.0)
        {
            return Err(bad("a report's deadline and information time are finite"));
        }
        if !self.created.is_finite()
            || self.created <= 0.0
            || !self.updated.is_finite()
            || self.updated < self.created
        {
            return Err(bad("a report needs finite times"));
        }
        Ok(())
    }
}

/// Which Reports to list, and the priorities they are ordered by. Every set field must match.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ReportFilter {
    /// Only this scope.
    pub scope: Option<ReportScope>,
    /// Only this state.
    pub state: Option<ReportState>,
    /// Only this kind.
    pub kind: Option<String>,
    /// Only Reports tied to this Mission.
    pub mission: Option<String>,
    /// At most this many; the Registry's `page` bounds it anyway.
    pub limit: Option<usize>,
    /// The deployment's priorities, most urgent first; a Report with another or none comes
    /// after them.
    pub priorities: Vec<String>,
}

impl Default for ReportFilter {
    fn default() -> Self {
        Self {
            scope: None,
            state: None,
            kind: None,
            mission: None,
            limit: None,
            priorities: ReportSettings::default().priorities,
        }
    }
}

impl ReportFilter {
    /// Every Report, up to the Registry's page, ordered by the default priorities.
    pub fn all() -> Self {
        Self::default()
    }

    /// Ordered by `priorities`, most urgent first.
    #[must_use]
    pub fn ordered_by(mut self, priorities: Vec<String>) -> Self {
        self.priorities = priorities;
        self
    }

    /// Only `scope`.
    #[must_use]
    pub fn in_scope(mut self, scope: ReportScope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Only Reports tied to the Mission `id`.
    #[must_use]
    pub fn about_mission(mut self, id: impl Into<String>) -> Self {
        self.mission = Some(id.into());
        self
    }

    /// Only `state`.
    #[must_use]
    pub fn in_state(mut self, state: ReportState) -> Self {
        self.state = Some(state);
        self
    }

    /// Only `kind`.
    #[must_use]
    pub fn of_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// At most `limit`.
    #[must_use]
    pub fn at_most(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    fn matches(&self, report: &Report) -> bool {
        self.scope
            .as_ref()
            .is_none_or(|scope| *scope == report.scope)
            && self.state.is_none_or(|state| state == report.state)
            && self.kind.as_deref().is_none_or(|kind| kind == report.kind)
            && self
                .mission
                .as_deref()
                .is_none_or(|mission| report.about_mission(mission))
    }

    /// How many to return, within `limits.page`.
    pub(super) fn page(&self, limits: &ReportLimits) -> usize {
        let page = limits.page.or_max();
        self.limit.map_or(page, |limit| limit.min(page))
    }
}

/// How far a source has earned belief, `A` (completely reliable) to `F` (cannot be judged).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Reliability {
    /// Completely reliable.
    A,
    /// Usually reliable.
    B,
    /// Fairly reliable.
    C,
    /// Not usually reliable.
    D,
    /// Unreliable.
    E,
    /// Cannot be judged: every new source.
    F,
}

impl Reliability {
    /// The letter.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::E => "E",
            Self::F => "F",
        }
    }
}

impl FromStr for Reliability {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(match text {
            "A" => Self::A,
            "B" => Self::B,
            "C" => Self::C,
            "D" => Self::D,
            "E" => Self::E,
            "F" => Self::F,
            other => return Err(format!("{other:?} is not a reliability: use A to F")),
        })
    }
}

impl fmt::Display for Reliability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A source's record: its key (see [`ReportFiler::source_key`]) and its reliability.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportSource {
    /// `source:NAME`, `agent:CODENAME`, `person:ID`, `probe:NAME`, `review` or `watchdog`.
    pub key: String,
    /// What it has earned; `F` until rated.
    pub reliability: Reliability,
    /// When it was last rated, as Unix seconds; `0` while it is new.
    pub rated: f64,
}

impl ReportSource {
    /// A source nobody has rated: reliability `F`.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            reliability: Reliability::F,
            rated: 0.0,
        }
    }

    /// The source with `reliability`, rated at `at`.
    #[must_use]
    pub fn rated_at(mut self, reliability: Reliability, at: f64) -> Self {
        self.reliability = reliability;
        self.rated = at;
        self
    }

    /// Checks the key's bounds.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an empty or oversized key, or a time that is not finite.
    pub fn validate(&self, limits: &ReportLimits) -> io::Result<()> {
        if self.key.trim().is_empty()
            || !limits.name_bytes.allows(self.key.len())
            || self.key.contains(char::is_control)
            || !self.rated.is_finite()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a source needs a key in bounds",
            ));
        }
        Ok(())
    }
}

/// Orders Reports for a watch: priority, then deadline, credibility, the source's reliability
/// and age (oldest first). Unset priority, deadline and credibility come last, and a source
/// without a record counts as `F`.
pub(super) fn order_reports(
    priorities: &[String],
    sources: &[ReportSource],
    reports: &mut [Report],
) {
    let reliability = |report: &Report| {
        let key = report.filer.source_key();
        sources
            .iter()
            .find(|source| source.key == key)
            .map_or(Reliability::F, |source| source.reliability)
    };
    let rank = |report: &Report| {
        let priority = report
            .priority
            .as_ref()
            .and_then(|wanted| priorities.iter().position(|name| name == wanted))
            .unwrap_or(priorities.len());
        (
            priority,
            report.deadline.unwrap_or(f64::INFINITY),
            report.credibility.unwrap_or(7),
            reliability(report),
            report.created,
        )
    };
    reports.sort_by(|left, right| {
        let (a, b) = (rank(left), rank(right));
        a.0.cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
            .then(a.4.total_cmp(&b.4))
            .then_with(|| left.id.cmp(&right.id))
    });
}

/// Who may read Reports up to which classification: one record per subject, granted by a
/// person and revocable at once.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportClearance {
    /// `person:ID`, `group:NAME` or `agent:CODENAME`.
    pub subject: String,
    /// The highest classification the subject reads, from the `roster` record's
    /// `reports.classifications`.
    pub classification: String,
    /// Who granted it.
    pub by: String,
    /// When, as Unix seconds.
    pub at: f64,
}

impl ReportClearance {
    /// `subject` cleared to `classification`, granted by `by` at `at`.
    pub fn new(
        subject: impl Into<String>,
        classification: impl Into<String>,
        by: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            subject: subject.into(),
            classification: classification.into(),
            by: by.into(),
            at,
        }
    }

    /// Checks the subject's form and the bounds of its names.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a subject that is not `person:`, `group:` or `agent:` and a name, or a
    /// name out of bounds.
    pub fn validate(&self, limits: &ReportLimits) -> io::Result<()> {
        let named = |text: &str| {
            !text.trim().is_empty()
                && limits.name_bytes.allows(text.len())
                && !text.contains(char::is_control)
        };
        let subject = self.subject.split_once(':').is_some_and(|(kind, name)| {
            matches!(kind, "person" | "group" | "agent") && !name.is_empty()
        });
        if !subject
            || !named(&self.subject)
            || !named(&self.classification)
            || !named(&self.by)
            || !self.at.is_finite()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a clearance names person:ID, group:NAME or agent:CODENAME and a classification",
            ));
        }
        Ok(())
    }
}

/// A change to a stored Report, applied where the store holds it.
pub type ReportChange<'a> = &'a mut dyn FnMut(&mut Report) -> io::Result<()>;

/// Reports: what was observed, per scope.
pub trait ReportStore {
    /// Stores a new Report. An Agent's filing is counted against `per_session` and
    /// `per_operation` over `window`; Reports closed longer than `keep_closed` are dropped.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a Report out of bounds ([`Report::validate`]); `AlreadyExists` for an
    /// ID in use; `QuotaExceeded` when the Agent's Session or the scope filed too many in the
    /// window; `InvalidData` past `[registry.limits.reports] reports`; I/O errors otherwise.
    fn file_report(&self, report: &Report) -> io::Result<()>;

    /// The Report `id`, or `None`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an ID that is not a Registry key; I/O errors otherwise.
    fn report(&self, id: &str) -> io::Result<Option<Report>>;

    /// The Reports `filter` matches in a watch's order: priority, deadline, credibility, the
    /// source's reliability, then oldest first. Unreadable ones are skipped.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn reports(&self, filter: &ReportFilter) -> io::Result<Vec<Report>>;

    /// Applies `change` to the Report `id` atomically and stores the result, which must keep its
    /// ID, scope, filer and filing time and stay in bounds. Answers the stored Report.
    ///
    /// # Errors
    ///
    /// `NotFound` for no such Report; what `change` returns; `InvalidInput` for a result out of
    /// bounds or one that changed what may not change; I/O errors otherwise.
    fn change_report(&self, id: &str, change: ReportChange<'_>) -> io::Result<Report>;

    /// The record of the source `key`: reliability `F` when nobody has rated it.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a key out of bounds; I/O errors otherwise.
    fn report_source(&self, key: &str) -> io::Result<ReportSource>;

    /// Stores a source's record, replacing the one with its key.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a record out of bounds; I/O errors otherwise.
    fn put_report_source(&self, source: &ReportSource) -> io::Result<()>;

    /// Every clearance, by subject. Unreadable ones are skipped, which clears nobody.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn report_clearances(&self) -> io::Result<Vec<ReportClearance>>;

    /// Stores a clearance, replacing the subject's.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a record out of bounds; I/O errors otherwise.
    fn put_report_clearance(&self, clearance: &ReportClearance) -> io::Result<()>;

    /// Removes the subject's clearance; `false` when it had none.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn revoke_report_clearance(&self, subject: &str) -> io::Result<bool>;
}

/// Refuses `report` when its Agent's Session, or its scope, filed too much in the window.
/// `recent` is the Sessions of the Agent-filed Reports of the scope inside the window.
pub(super) fn check_rate(
    limits: &ReportLimits,
    report: &Report,
    recent: &[String],
) -> io::Result<()> {
    let Some(session) = report.filer.session() else {
        return Ok(());
    };
    let mine = recent.iter().filter(|other| *other == session).count();
    let refuse = |what: String| io::Error::new(io::ErrorKind::QuotaExceeded, what);
    if !limits.per_session.allows(mine + 1) {
        return Err(refuse(format!(
            "this Session filed {} reports in the window: wait",
            limits.per_session
        )));
    }
    if !limits.per_operation.allows(recent.len() + 1) {
        return Err(refuse(format!(
            "agents filed {} reports against {} in the window: wait",
            limits.per_operation, report.scope
        )));
    }
    Ok(())
}

/// Applies `change` to `stored`, refusing a result that moved what may not move.
pub(super) fn apply_change(
    limits: &ReportLimits,
    stored: &Report,
    change: ReportChange<'_>,
) -> io::Result<Report> {
    let mut changed = stored.clone();
    change(&mut changed)?;
    if changed.id != stored.id
        || changed.scope != stored.scope
        || changed.filer != stored.filer
        || changed.created.to_bits() != stored.created.to_bits()
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a report keeps its ID, scope, filer and filing time",
        ));
    }
    changed.validate(limits)?;
    Ok(changed)
}

/// The stored form of a Report, refused past the size limit.
pub(super) fn report_bytes(report: &Report, limits: &ReportLimits) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(report).map_err(io::Error::other)?;
    if !limits.record_bytes.allows(bytes.len()) {
        return Err(invalid_data("the report exceeds the size limit"));
    }
    Ok(bytes)
}

/// A stored Report, checked against the size limit and its bounds.
pub(super) fn parse_report(bytes: &[u8], limits: &ReportLimits) -> io::Result<Report> {
    if !limits.record_bytes.allows(bytes.len()) {
        return Err(invalid_data("the report exceeds the size limit"));
    }
    let report: Report = serde_json::from_slice(bytes)
        .map_err(|error| invalid_data(format!("invalid report: {error}")))?;
    report
        .validate(limits)
        .map_err(|error| invalid_data(error.to_string()))?;
    Ok(report)
}

/// The earliest `created` still inside the window that ends at `at`.
pub(super) fn window_start(limits: &ReportLimits, at: f64) -> f64 {
    at - limits.window.0.as_secs_f64()
}

/// The `updated` before which a closed Report is dropped, or `None` when they are kept.
pub(super) fn closed_before(limits: &ReportLimits, at: f64) -> Option<f64> {
    limits.keep_closed.get().map(|age| at - age.as_secs_f64())
}

impl FileRegistry {
    fn reports_root(&self) -> PathBuf {
        self.root().join("reports")
    }

    fn report_sources_root(&self) -> PathBuf {
        self.root().join("report-sources")
    }

    fn report_clearances_root(&self) -> PathBuf {
        self.root().join("report-clearances")
    }

    fn scope_directories(&self, scope: Option<&ReportScope>) -> io::Result<Vec<PathBuf>> {
        let root = self.reports_root();
        if let Some(scope) = scope {
            return Ok(vec![root.join(scope.directory())]);
        }
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect()
    }

    fn read_sources(&self) -> io::Result<Vec<ReportSource>> {
        let entries = match fs::read_dir(self.report_sources_root()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut sources = Vec::new();
        for entry in entries {
            if let Ok(bytes) = fs::read(entry?.path())
                && let Ok(source) = serde_json::from_slice::<ReportSource>(&bytes)
                && source.validate(&self.limits.reports).is_ok()
            {
                sources.push(source);
            }
        }
        Ok(sources)
    }

    fn read_reports(&self, scope: Option<&ReportScope>) -> io::Result<Vec<(PathBuf, Report)>> {
        let mut found = Vec::new();
        for directory in self.scope_directories(scope)? {
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for entry in entries {
                let path = entry?.path();
                // A record is kept where its own scope and ID put it, and nowhere else.
                if let Ok(bytes) = fs::read(&path)
                    && let Ok(report) = parse_report(&bytes, &self.limits.reports)
                    && directory.file_name().and_then(|name| name.to_str())
                        == Some(report.scope.directory().as_str())
                    && path.file_stem().and_then(|stem| stem.to_str()) == Some(&report.id)
                {
                    found.push((path, report));
                }
            }
        }
        Ok(found)
    }
}

impl ReportStore for FileRegistry {
    fn file_report(&self, report: &Report) -> io::Result<()> {
        let limits = &self.limits.reports;
        report.validate(limits)?;
        let bytes = report_bytes(report, limits)?;
        let _lock = self.lock()?;
        let mut all = self.read_reports(None)?;
        if all.iter().any(|(_, other)| other.id == report.id) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a report with this ID exists",
            ));
        }
        let since = window_start(limits, report.created);
        let recent: Vec<String> = all
            .iter()
            .filter(|(_, other)| other.scope == report.scope && other.created >= since)
            .filter_map(|(_, other)| other.filer.session().map(str::to_owned))
            .collect();
        check_rate(limits, report, &recent)?;
        if let Some(before) = closed_before(limits, report.created) {
            all.retain(|(path, other)| {
                let stale = other.state == ReportState::Closed && other.updated < before;
                !(stale && fs::remove_file(path).is_ok())
            });
        }
        if !limits.reports.allows(all.len() + 1) {
            return Err(invalid_data("too many reports"));
        }
        let directory = self.reports_root().join(report.scope.directory());
        fs::create_dir_all(&directory)?;
        self.write_atomic(&directory.join(format!("{}.json", report.id)), &bytes)
    }

    fn report(&self, id: &str) -> io::Result<Option<Report>> {
        validate_registry_key(id)?;
        Ok(self
            .read_reports(None)?
            .into_iter()
            .map(|(_, report)| report)
            .find(|report| report.id == id))
    }

    fn reports(&self, filter: &ReportFilter) -> io::Result<Vec<Report>> {
        let mut found: Vec<Report> = self
            .read_reports(filter.scope.as_ref())?
            .into_iter()
            .map(|(_, report)| report)
            .filter(|report| filter.matches(report))
            .collect();
        let sources = self.read_sources()?;
        order_reports(&filter.priorities, &sources, &mut found);
        found.truncate(filter.page(&self.limits.reports));
        Ok(found)
    }

    fn change_report(&self, id: &str, change: ReportChange<'_>) -> io::Result<Report> {
        validate_registry_key(id)?;
        let limits = &self.limits.reports;
        let _lock = self.lock()?;
        let (path, stored) = self
            .read_reports(None)?
            .into_iter()
            .find(|(_, report)| report.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such report"))?;
        let changed = apply_change(limits, &stored, change)?;
        self.write_atomic(&path, &report_bytes(&changed, limits)?)?;
        Ok(changed)
    }

    fn report_source(&self, key: &str) -> io::Result<ReportSource> {
        ReportSource::new(key).validate(&self.limits.reports)?;
        Ok(self
            .read_sources()?
            .into_iter()
            .find(|source| source.key == key)
            .unwrap_or_else(|| ReportSource::new(key)))
    }

    fn put_report_source(&self, source: &ReportSource) -> io::Result<()> {
        source.validate(&self.limits.reports)?;
        let bytes = serde_json::to_vec(source).map_err(io::Error::other)?;
        let _lock = self.lock()?;
        let directory = self.report_sources_root();
        fs::create_dir_all(&directory)?;
        self.write_atomic(
            &directory.join(format!("{}.json", station_key(&source.key))),
            &bytes,
        )
    }

    fn report_clearances(&self) -> io::Result<Vec<ReportClearance>> {
        let entries = match fs::read_dir(self.report_clearances_root()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut clearances = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if let Ok(bytes) = fs::read(&path)
                && let Ok(clearance) = serde_json::from_slice::<ReportClearance>(&bytes)
                && clearance.validate(&self.limits.reports).is_ok()
                && path.file_stem().and_then(|stem| stem.to_str())
                    == Some(station_key(&clearance.subject).as_str())
            {
                clearances.push(clearance);
            }
        }
        clearances.sort_by(|left, right| left.subject.cmp(&right.subject));
        Ok(clearances)
    }

    fn put_report_clearance(&self, clearance: &ReportClearance) -> io::Result<()> {
        clearance.validate(&self.limits.reports)?;
        let bytes = serde_json::to_vec(clearance).map_err(io::Error::other)?;
        let _lock = self.lock()?;
        let directory = self.report_clearances_root();
        fs::create_dir_all(&directory)?;
        self.write_atomic(
            &directory.join(format!("{}.json", station_key(&clearance.subject))),
            &bytes,
        )
    }

    fn revoke_report_clearance(&self, subject: &str) -> io::Result<bool> {
        let _lock = self.lock()?;
        let path = self
            .report_clearances_root()
            .join(format!("{}.json", station_key(subject)));
        match fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounds::{Limit, TimeLimit};
    use crate::directorates::registry::limits::RegistryLimits;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_report_is_kept_per_scope_and_listed_by_what_it_is_tied_to,
        the_listing_takes_priority_deadline_credibility_reliability_then_age,
        credibility_one_needs_a_corroborating_link,
        a_new_source_is_f_until_rated,
        a_change_keeps_what_was_filed_and_stays_in_bounds,
        an_agents_storm_is_refused_per_session_and_per_scope,
        closed_reports_go_after_their_age_and_the_count_is_bounded,
        a_clearance_is_kept_per_subject_and_revoked_at_once
    );

    fn agent(session: &str, mission: &str) -> ReportFiler {
        ReportFiler::Agent {
            agent: "007".to_owned(),
            session: session.to_owned(),
            mission: mission.to_owned(),
        }
    }

    fn report(id: &str, scope: ReportScope, filer: ReportFiler, at: f64) -> Report {
        Report::new(
            id,
            scope,
            "finding",
            Severity::Warning,
            "title",
            "text",
            filer,
            at,
        )
    }

    fn operation(id: &str) -> ReportScope {
        ReportScope::Operation(id.to_owned())
    }

    fn ids(reports: Vec<Report>) -> Vec<String> {
        reports.into_iter().map(|found| found.id).collect()
    }

    fn a_report_is_kept_per_scope_and_listed_by_what_it_is_tied_to(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.reports(&ReportFilter::all()).unwrap().is_empty());
        let agents = report("r1", operation("G"), agent("s1", "G-1"), 10.0)
            .linked(ReportLink::mission("G-1"))
            .classified("restricted")
            .in_compartments(vec!["KESTREL".to_owned()])
            .with_tearline(Tearline::new("sanitised"));
        let person = report(
            "r2",
            operation("G"),
            ReportFiler::Person {
                person: "alice".to_owned(),
            },
            11.0,
        );
        let station = report(
            "r3",
            ReportScope::Station("alpha:1".to_owned()),
            agent("s2", "S-1"),
            12.0,
        );
        for each in [&agents, &person, &station] {
            registry.file_report(each).unwrap();
        }
        assert_eq!(registry.report("r1").unwrap().unwrap(), agents);
        assert!(registry.report("nope").unwrap().is_none());
        assert!(agents.untrusted() && !person.untrusted());
        let listed = |filter: ReportFilter| ids(registry.reports(&filter).unwrap());
        assert_eq!(
            listed(ReportFilter::all()),
            ["r1", "r2", "r3"],
            "oldest first"
        );
        assert_eq!(
            listed(ReportFilter::all().in_scope(operation("G"))),
            ["r1", "r2"]
        );
        assert_eq!(
            listed(ReportFilter::all().in_scope(ReportScope::Station("alpha:1".to_owned()))),
            ["r3"]
        );
        assert_eq!(listed(ReportFilter::all().about_mission("G-1")), ["r1"]);
        assert!(listed(ReportFilter::all().in_state(ReportState::Closed)).is_empty());
        assert_eq!(
            listed(ReportFilter::all().of_kind("finding").at_most(1)),
            ["r1"]
        );
        assert_eq!(
            registry.file_report(&agents).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        let watchdog = |id: &str, scope: ReportScope, kind: &str, text: &str, at: f64| {
            Report::new(
                id,
                scope,
                kind,
                Severity::Info,
                "t",
                text,
                ReportFiler::Watchdog,
                at,
            )
        };
        for bad in [
            watchdog("x/y", operation("G"), "finding", "t", 1.0),
            watchdog("b1", operation("G"), "", "t", 1.0),
            watchdog("b2", operation("G"), "not a label", "t", 1.0),
            watchdog("b3", operation("G"), "finding", "  ", 1.0),
            watchdog("b4", operation("G/.."), "finding", "t", 1.0),
            watchdog("b5", operation("G"), "finding", "t", f64::NAN),
            // A tearline in a compartment the Report is not in.
            watchdog("b9", operation("G"), "finding", "t", 1.0)
                .with_tearline(Tearline::new("x").in_compartments(vec!["K".to_owned()])),
        ] {
            assert_eq!(
                registry.file_report(&bad).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{bad:?}"
            );
        }
    }

    fn the_listing_takes_priority_deadline_credibility_reliability_then_age(store: &dyn Store) {
        let registry = store.handle();
        let source = |name: &str| ReportFiler::Source {
            source: name.to_owned(),
            item: "1".to_owned(),
        };
        let corroborated =
            |report: Report| report.linked(ReportLink::new(ReportLinkKind::Report, "z"));
        registry
            .put_report_source(&ReportSource::new("source:trusted").rated_at(Reliability::A, 5.0))
            .unwrap();
        let filed = [
            // No priority: last.
            report("none", operation("G"), source("trusted"), 1.0),
            // Low comes after normal, whatever else it has.
            report("low", operation("G"), source("trusted"), 2.0)
                .prioritised("low")
                .due(1.0),
            // Among normal: a deadline first, the sooner first.
            report("normal-late", operation("G"), source("new"), 3.0)
                .prioritised("normal")
                .due(900.0),
            report("normal-soon", operation("G"), source("new"), 4.0)
                .prioritised("normal")
                .due(800.0),
            // No deadline: credibility, then the source's reliability, then age.
            corroborated(report("normal-c1", operation("G"), source("new"), 9.0))
                .prioritised("normal")
                .believed(1),
            report("normal-c3-new", operation("G"), source("new"), 5.0)
                .prioritised("normal")
                .believed(3),
            report("normal-c3-trusted", operation("G"), source("trusted"), 8.0)
                .prioritised("normal")
                .believed(3),
            report("normal-older", operation("G"), source("new"), 6.0).prioritised("normal"),
            report("normal-newer", operation("G"), source("new"), 7.0).prioritised("normal"),
            report("critical", operation("G"), source("new"), 10.0).prioritised("critical"),
        ];
        for each in &filed {
            registry.file_report(each).unwrap();
        }
        assert_eq!(
            ids(registry.reports(&ReportFilter::all()).unwrap()),
            [
                "critical",
                "normal-soon",
                "normal-late",
                "normal-c1",
                "normal-c3-trusted",
                "normal-c3-new",
                "normal-older",
                "normal-newer",
                "low",
                "none",
            ]
        );
        assert_eq!(
            ids(registry.reports(&ReportFilter::all().at_most(2)).unwrap()),
            ["critical", "normal-soon"],
            "the page is cut after ordering"
        );
        // The deployment's own list orders them: here low first; normal is then unlisted, so
        // it falls back on the deadline.
        let low_first = ReportFilter::all()
            .ordered_by(vec!["low".to_owned(), "critical".to_owned()])
            .at_most(3);
        assert_eq!(
            ids(registry.reports(&low_first).unwrap()),
            ["low", "critical", "normal-soon"]
        );
    }

    fn credibility_one_needs_a_corroborating_link(store: &dyn Store) {
        let registry = store.handle();
        let bare = report("bare", operation("G"), ReportFiler::Watchdog, 1.0)
            .linked(ReportLink::mission("G-1"))
            .believed(1);
        let refused = registry.file_report(&bare).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
        assert!(refused.to_string().contains("corroborating"), "{refused}");
        for (id, kind) in [
            ("by-report", ReportLinkKind::Report),
            ("by-evidence", ReportLinkKind::Evidence),
        ] {
            registry
                .file_report(
                    &report(id, operation("G"), ReportFiler::Watchdog, 1.0)
                        .linked(ReportLink::new(kind, "corroboration"))
                        .believed(1),
                )
                .unwrap();
        }
        for credibility in [0, 7] {
            assert_eq!(
                registry
                    .file_report(
                        &report("out", operation("G"), ReportFiler::Watchdog, 1.0)
                            .believed(credibility)
                    )
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        // A change cannot make it confirmed without the link either.
        let refused = registry
            .change_report("by-report", &mut |report| {
                report.links.clear();
                Ok(())
            })
            .unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
    }

    fn a_new_source_is_f_until_rated(store: &dyn Store) {
        let registry = store.handle();
        let fresh = registry.report_source("agent:007").unwrap();
        assert_eq!(fresh.reliability, Reliability::F);
        assert_eq!(fresh, ReportSource::new("agent:007"));
        registry
            .put_report_source(&ReportSource::new("agent:007").rated_at(Reliability::B, 9.0))
            .unwrap();
        assert_eq!(
            registry.report_source("agent:007").unwrap().reliability,
            Reliability::B
        );
        assert_eq!(
            registry.report_source("agent:008").unwrap().reliability,
            Reliability::F
        );
        assert_eq!(
            registry.report_source("").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    fn a_change_keeps_what_was_filed_and_stays_in_bounds(store: &dyn Store) {
        let registry = store.handle();
        let filed = report("r", operation("G"), agent("s1", "G-1"), 10.0)
            .linked(ReportLink::mission("G-1"))
            .classified("restricted");
        registry.file_report(&filed).unwrap();
        let changed = registry
            .change_report("r", &mut |report| {
                report.classification = Some("confidential".to_owned());
                report.links.push(ReportLink::mission("G-2"));
                report.updated = 20.0;
                Ok(())
            })
            .unwrap();
        assert_eq!(changed.classification.as_deref(), Some("confidential"));
        assert_eq!(registry.report("r").unwrap().unwrap(), changed);
        assert_eq!(
            ids(registry
                .reports(&ReportFilter::all().about_mission("G-2"))
                .unwrap()),
            ["r"],
            "a new link is found"
        );
        for refused in [
            registry.change_report("r", &mut |report| {
                report.scope = operation("L");
                Ok(())
            }),
            registry.change_report("r", &mut |report| {
                report.filer = ReportFiler::Watchdog;
                Ok(())
            }),
            registry.change_report("r", &mut |report| {
                report.title = String::new();
                Ok(())
            }),
        ] {
            assert_eq!(refused.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        }
        assert_eq!(
            registry
                .change_report("r", &mut |_| Err(io::Error::other("no")))
                .unwrap_err()
                .to_string(),
            "no"
        );
        assert_eq!(
            registry
                .change_report("missing", &mut |_| Ok(()))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            registry.report("r").unwrap().unwrap(),
            changed,
            "refusals change nothing"
        );
    }

    fn an_agents_storm_is_refused_per_session_and_per_scope(store: &dyn Store) {
        let registry = store.handle_with(RegistryLimits {
            reports: ReportLimits {
                per_session: Limit::at(3),
                per_operation: Limit::at(5),
                ..ReportLimits::default()
            },
            ..RegistryLimits::default()
        });
        let limits = registry.limits().reports.clone();
        assert!(
            ReportLimits::default().per_session.get().is_some(),
            "a default bounds a storm"
        );
        for index in 0..3 {
            registry
                .file_report(&report(
                    &format!("a{index}"),
                    operation("G"),
                    agent("s1", "G-1"),
                    100.0 + f64::from(index),
                ))
                .unwrap();
        }
        let storm = report("storm", operation("G"), agent("s1", "G-1"), 103.0);
        assert_eq!(
            registry.file_report(&storm).unwrap_err().kind(),
            io::ErrorKind::QuotaExceeded
        );
        // Another Session, another scope and a person are counted apart.
        for (id, scope, filer) in [
            ("other", operation("G"), agent("s2", "G-1")),
            ("other-2", operation("G"), agent("s3", "G-1")),
            ("elsewhere", operation("L"), agent("s1", "L-1")),
            (
                "person",
                operation("G"),
                ReportFiler::Person {
                    person: "alice".to_owned(),
                },
            ),
        ] {
            registry
                .file_report(&report(id, scope, filer, 200.0))
                .unwrap();
        }
        // The Operation's five are used: a fresh Session is refused too.
        assert_eq!(
            registry
                .file_report(&report("sixth", operation("G"), agent("s4", "G-1"), 201.0))
                .unwrap_err()
                .kind(),
            io::ErrorKind::QuotaExceeded
        );
        // Past the window, the Session files again.
        let later = 200.0 + limits.window.0.as_secs_f64() + 1.0;
        registry
            .file_report(&report("later", operation("G"), agent("s1", "G-1"), later))
            .unwrap();
    }

    fn closed_reports_go_after_their_age_and_the_count_is_bounded(store: &dyn Store) {
        let registry = store.handle_with(RegistryLimits {
            reports: ReportLimits {
                keep_closed: TimeLimit::secs(100),
                reports: Limit::at(3),
                ..ReportLimits::default()
            },
            ..RegistryLimits::default()
        });
        let mut old = report("old", operation("G"), ReportFiler::Watchdog, 10.0);
        old.state = ReportState::Closed;
        let open = report("open", operation("G"), ReportFiler::Watchdog, 10.0);
        registry.file_report(&old).unwrap();
        registry.file_report(&open).unwrap();
        registry
            .file_report(&report("new", operation("G"), ReportFiler::Watchdog, 500.0))
            .unwrap();
        assert!(
            registry.report("old").unwrap().is_none(),
            "closed and past its age"
        );
        assert!(
            registry.report("open").unwrap().is_some(),
            "an open one stays"
        );
        registry
            .file_report(&report(
                "third",
                operation("G"),
                ReportFiler::Watchdog,
                501.0,
            ))
            .unwrap();
        assert_eq!(
            registry
                .file_report(&report(
                    "fourth",
                    operation("G"),
                    ReportFiler::Watchdog,
                    502.0
                ))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    fn a_clearance_is_kept_per_subject_and_revoked_at_once(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.report_clearances().unwrap().is_empty());
        let ana = ReportClearance::new("person:ana", "confidential", "person:chief", 5.0);
        let security = ReportClearance::new("group:security", "restricted", "person:chief", 6.0);
        registry.put_report_clearance(&ana).unwrap();
        registry.put_report_clearance(&security).unwrap();
        // Another handle on the same storage sees them at once.
        let other = store.handle();
        assert_eq!(other.report_clearances().unwrap(), [security.clone(), ana]);
        let lowered = ReportClearance::new("person:ana", "restricted", "person:chief", 7.0);
        registry.put_report_clearance(&lowered).unwrap();
        assert_eq!(
            other.report_clearances().unwrap(),
            [security.clone(), lowered],
            "one per subject"
        );
        assert!(registry.revoke_report_clearance("person:ana").unwrap());
        assert!(!registry.revoke_report_clearance("person:ana").unwrap());
        assert_eq!(other.report_clearances().unwrap(), [security]);
        for bad in [
            ReportClearance::new("ana", "restricted", "person:chief", 1.0),
            ReportClearance::new("operation:G", "restricted", "person:chief", 1.0),
            ReportClearance::new("person:", "restricted", "person:chief", 1.0),
            ReportClearance::new("person:ana", "", "person:chief", 1.0),
        ] {
            assert_eq!(
                registry.put_report_clearance(&bad).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{bad:?}"
            );
        }
    }
}
