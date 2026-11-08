//! Who may read a Report, what of it they see, and who may change its markings.
//!
//! Reading a Report's content takes three things at once: the scope's read (an Operation's
//! `mission.read`, its own or through a compartment), being read in to each of its compartments,
//! and a clearance at or above its classification. It applies to a person and to an Agent's
//! Session alike, and nobody is exempt. Short of that, a reader cleared for the tearline's marking
//! reads the tearline in its place, and anyone else, or anything linking to it, is shown its ID
//! and markings: a link is not access.
//!
//! The filer sets the markings. Changing them afterwards needs someone who reads the content both
//! before and after the change, so lowering them needs someone read in.
//!
//! The lists a Report is marked and prioritised from are the `roster` record's `reports` table
//! ([`ReportSettings`], read with [`settings`]); who is cleared for which classification is a
//! [`ReportClearance`] record in the Registry, one per person, group or Agent. The caller reads
//! both, and answers [`ReportReader`] from what is granted at that moment (the access policy,
//! `compartments::current`), at every call, and keeps nothing between calls: a change holds from
//! the next one.

use guatiao::{FromValue, Schema, ToValue};
use guatiao_intake::Form;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::directorates::m::config::{ConfigProvider, ConfigRecord, ConfigScope};
use crate::directorates::m::records;
use crate::directorates::m::roster::{RosterError, RosterErrorKind, RosterSettings};
use crate::directorates::registry::limits::ReportLimits;
use crate::directorates::registry::logbook::Severity;
use crate::directorates::registry::reports::{
    Report, ReportClearance, ReportFiler, ReportLink, ReportScope, ReportSource, Tearline,
};

/// The lists Reports are marked and prioritised from: the `reports` table of the `roster`
/// record, a deployment policy kept by the configuration provider.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct ReportSettings {
    /// A Report carries one of these or none, which is the lowest; reading it needs a clearance
    /// at or above it.
    #[schema(title = "Classifications, lowest first")]
    #[map(default = ReportSettings::default().classifications)]
    pub classifications: Vec<String>,
    /// How far a Report may go.
    #[schema(title = "Handling marks, narrowest first")]
    #[map(default = ReportSettings::default().handlings)]
    pub handlings: Vec<String>,
    /// A Report's proposed or assessed urgency; one without comes last.
    #[schema(title = "Priorities, most urgent first")]
    #[map(default = ReportSettings::default().priorities)]
    pub priorities: Vec<String>,
}

impl Default for ReportSettings {
    fn default() -> Self {
        let list = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        Self {
            classifications: list(&["unclassified", "restricted", "confidential"]),
            handlings: list(&["operation", "compartments", "peers"]),
            priorities: list(&["critical", "high", "normal", "low"]),
        }
    }
}

impl ReportSettings {
    /// Where `classification` stands in `classifications`, lowest `0`; unset is the lowest, and
    /// one not in the list is `None`.
    pub fn rank(&self, classification: Option<&str>) -> Option<usize> {
        match classification {
            None => Some(0),
            Some(name) => self.classifications.iter().position(|each| each == name),
        }
    }

    /// Checks that `report`'s classification, handling mark and priority are in the lists, and
    /// that its tearline is classified no higher than it.
    ///
    /// # Errors
    ///
    /// What is out of the lists, for the filer.
    pub fn check(&self, report: &Report) -> Result<(), String> {
        let listed = |value: &Option<String>, list: &[String]| {
            value.as_ref().is_none_or(|value| list.contains(value))
        };
        if !listed(&report.classification, &self.classifications)
            || !listed(&report.handling, &self.handlings)
            || !listed(&report.priority, &self.priorities)
        {
            return Err(
                "a report's classification, handling and priority are from the deployment's lists"
                    .to_owned(),
            );
        }
        if let Some(tearline) = &report.tearline {
            let lower = match (
                self.rank(tearline.classification.as_deref()),
                self.rank(report.classification.as_deref()),
            ) {
                (Some(tearline), Some(report)) => tearline <= report,
                _ => false,
            };
            if !lower {
                return Err("a tearline is classified no higher than its report".to_owned());
            }
        }
        Ok(())
    }
}

/// The deployment's [`ReportSettings`] as `provider` holds them now: the `roster` record's
/// `reports` table, or the defaults when there is none or it cannot be parsed.
///
/// # Errors
///
/// When the provider cannot be read; the caller refuses rather than answer from memory.
pub fn settings(provider: &dyn ConfigProvider) -> Result<ReportSettings, RosterError> {
    match provider.definitions(ConfigRecord::Roster, &ConfigScope::Global) {
        Ok(definitions) => Ok(definitions
            .records
            .first()
            .and_then(|(_, definition)| records::from_json::<RosterSettings>(definition).ok())
            .unwrap_or_default()
            .reports),
        // A provider built in code lists nothing: its snapshot is what it holds.
        Err(reason) if reason.kind() == RosterErrorKind::ReadOnly => {
            Ok(provider.load("")?.roster.settings().reports.clone())
        }
        Err(reason) => Err(reason),
    }
}

/// What a read is judged by: the deployment's lists and the clearances, both as they are now.
#[derive(Debug, Clone, Copy)]
pub struct ReportRules<'a> {
    /// The lists.
    pub settings: &'a ReportSettings,
    /// Every clearance.
    pub clearances: &'a [ReportClearance],
}

impl<'a> ReportRules<'a> {
    /// Rules from `settings` and `clearances`.
    pub fn new(settings: &'a ReportSettings, clearances: &'a [ReportClearance]) -> Self {
        Self {
            settings,
            clearances,
        }
    }

    /// The highest classification rank a clearance grants any of `subjects`; `0` for none, and
    /// one naming a classification not in the list grants nothing above it.
    pub fn clearance(&self, subjects: &[String]) -> usize {
        self.clearances
            .iter()
            .filter(|clearance| subjects.contains(&clearance.subject))
            .filter_map(|clearance| self.settings.rank(Some(&clearance.classification)))
            .max()
            .unwrap_or(0)
    }
}

/// Who is asking about a Report, as granted now.
pub trait ReportReader {
    /// Whether the reader holds the scope's read. A read through a compartment is a crossing the
    /// reader writes to the audit log before answering `true`.
    fn reads_scope(&self, scope: &ReportScope) -> bool;

    /// Whether the reader is read in to `compartment`. One that is unknown or cannot be read
    /// answers `false`.
    fn read_in(&self, compartment: &str) -> bool;

    /// The subjects the reader's [`ReportClearance`]s are granted to: `person:ID` and
    /// `group:NAME` for a person, `agent:CODENAME` for an Agent.
    fn subjects(&self) -> Vec<String>;
}

/// How much of a Report a reader sees.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportSight {
    /// Everything.
    Content,
    /// The tearline in place of the title and text, and no links or filer.
    Tearline,
    /// Its ID, scope and markings, and that it exists.
    Markings,
}

/// Whether `reader` is cleared for `classification` and read in to each of `compartments`.
fn cleared(
    reader: &dyn ReportReader,
    rules: &ReportRules<'_>,
    classification: Option<&str>,
    compartments: &[String],
) -> bool {
    let held = rules.clearance(&reader.subjects());
    rules
        .settings
        .rank(classification)
        .is_some_and(|needed| held >= needed)
        && compartments
            .iter()
            .all(|compartment| reader.read_in(compartment))
}

/// What `reader` sees of `report`. Without the scope's read it is [`ReportSight::Markings`], as
/// for anything that only links to it.
pub fn sight(reader: &dyn ReportReader, report: &Report, rules: &ReportRules<'_>) -> ReportSight {
    if !reader.reads_scope(&report.scope) {
        return ReportSight::Markings;
    }
    if cleared(
        reader,
        rules,
        report.classification.as_deref(),
        &report.compartments,
    ) {
        return ReportSight::Content;
    }
    match &report.tearline {
        Some(tearline)
            if cleared(
                reader,
                rules,
                tearline.classification.as_deref(),
                &tearline.compartments,
            ) =>
        {
            ReportSight::Tearline
        }
        _ => ReportSight::Markings,
    }
}

/// What `sight` lets a reader see of `report`, as routes and tools answer it. `source` is the
/// filer's source record, shown with the content.
pub fn report_json(report: &Report, sight: ReportSight, source: Option<&ReportSource>) -> Value {
    let mut shown = json!({
        "id": report.id,
        "scope": report.scope.to_string(),
        "classification": report.classification,
        "handling": report.handling,
        "compartments": report.compartments,
        "sight": match sight {
            ReportSight::Content => "content",
            ReportSight::Tearline => "tearline",
            ReportSight::Markings => "markings",
        },
    });
    if sight == ReportSight::Markings {
        return shown;
    }
    for (key, value) in [
        ("kind", json!(report.kind)),
        ("severity", json!(report.severity.as_str())),
        ("state", json!(report.state.as_str())),
        ("priority", json!(report.priority)),
        ("deadline", json!(report.deadline)),
        ("credibility", json!(report.credibility)),
        ("created", json!(report.created)),
        ("updated", json!(report.updated)),
        ("untrusted", json!(report.untrusted())),
    ] {
        shown[key] = value;
    }
    if sight == ReportSight::Tearline {
        if let Some(tearline) = &report.tearline {
            shown["text"] = json!(tearline.text);
        }
        return shown;
    }
    for (key, value) in [
        ("title", json!(report.title)),
        ("text", json!(report.text)),
        ("filer", json!(report.filer)),
        ("source", json!(report.filer.source_key())),
        (
            "reliability",
            json!(source.map_or("F", |source| source.reliability.as_str())),
        ),
        ("sub_source", json!(report.sub_source)),
        ("from", json!(report.from)),
        ("links", json!(report.links)),
        ("tearline", json!(report.tearline)),
        ("assessment", json!(report.assessment)),
        ("resolution", json!(report.resolution)),
    ] {
        shown[key] = value;
    }
    shown
}

/// The markings a Report carries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportMarkings {
    /// From [`ReportSettings::classifications`]; unset is the lowest.
    #[serde(default)]
    pub classification: Option<String>,
    /// From `handlings`.
    #[serde(default)]
    pub handling: Option<String>,
    /// Each one a reader must be read in to.
    #[serde(default)]
    pub compartments: Vec<String>,
    /// The tearline; unset leaves none.
    #[serde(default)]
    pub tearline: Option<TearlineFiling>,
}

/// Remarks `report` for `reader`, who must read its content both as it is and as it would be.
///
/// # Errors
///
/// Why it is refused, for the caller to show; the Report is left as it was.
pub fn remark(
    reader: &dyn ReportReader,
    report: &mut Report,
    markings: ReportMarkings,
    rules: &ReportRules<'_>,
) -> Result<(), String> {
    if sight(reader, report, rules) != ReportSight::Content {
        return Err(
            "changing a report's markings needs someone read in to it and cleared for it"
                .to_owned(),
        );
    }
    let mut changed = report.clone();
    changed.classification = markings.classification;
    changed.handling = markings.handling;
    changed.compartments = markings.compartments;
    changed.tearline = markings.tearline.map(TearlineFiling::into_tearline);
    if !cleared(
        reader,
        rules,
        changed.classification.as_deref(),
        &changed.compartments,
    ) {
        return Err(
            "you may mark a report only as far as you are cleared and read in yourself".to_owned(),
        );
    }
    rules.settings.check(&changed)?;
    *report = changed;
    Ok(())
}

/// A tearline as a filer writes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TearlineFiling {
    /// What may be released.
    pub text: String,
    /// Its classification, no higher than the Report's.
    #[serde(default)]
    pub classification: Option<String>,
    /// Its compartments, each one of the Report's.
    #[serde(default)]
    pub compartments: Vec<String>,
}

impl TearlineFiling {
    fn into_tearline(self) -> Tearline {
        let mut tearline = Tearline::new(self.text).in_compartments(self.compartments);
        tearline.classification = self.classification;
        tearline
    }
}

/// A link as a filer writes it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkFiling {
    /// `mission`, `report`, `decision` or `evidence`.
    pub kind: String,
    /// What it points at.
    pub id: String,
}

/// A Report as a person or an Agent files it: what they say and how they mark it. Credibility,
/// the assessment and the resolution are not theirs to set.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportFiling {
    /// One line naming it.
    pub title: String,
    /// What was observed.
    pub text: String,
    /// A short label; `finding` when left out.
    #[serde(default)]
    pub kind: Option<String>,
    /// `info`, `warning` or `critical`; `info` when left out.
    #[serde(default)]
    pub severity: Option<String>,
    /// From [`ReportSettings::priorities`].
    #[serde(default)]
    pub priority: Option<String>,
    /// When it needs an answer by, as Unix seconds.
    #[serde(default)]
    pub deadline: Option<f64>,
    /// When the information is from, as Unix seconds.
    #[serde(default)]
    pub information_from: Option<f64>,
    /// Who passed it on to the filer.
    #[serde(default)]
    pub sub_source: Option<String>,
    /// From [`ReportSettings::classifications`]; unset is the lowest.
    #[serde(default)]
    pub classification: Option<String>,
    /// From `handlings`.
    #[serde(default)]
    pub handling: Option<String>,
    /// Each one a reader must be read in to.
    #[serde(default)]
    pub compartments: Vec<String>,
    /// What those cleared for its own marking read in the Report's place.
    #[serde(default)]
    pub tearline: Option<TearlineFiling>,
    /// What it is tied to.
    #[serde(default)]
    pub links: Vec<LinkFiling>,
}

/// A new Report ID.
pub fn new_report_id() -> String {
    format!("report-{}", Uuid::new_v4().simple())
}

impl ReportFiling {
    /// The Report this filing makes, filed by `filer` in `scope` at `at`, with `links` added.
    ///
    /// # Errors
    ///
    /// Why it is refused: an unknown severity or link kind, or anything [`Report::validate`]
    /// refuses.
    pub fn into_report(
        self,
        scope: ReportScope,
        filer: ReportFiler,
        links: impl IntoIterator<Item = ReportLink>,
        at: f64,
        limits: &ReportLimits,
        settings: &ReportSettings,
    ) -> Result<Report, String> {
        let severity = match self.severity.as_deref() {
            None => Severity::Info,
            Some(severity) => severity.parse()?,
        };
        let mut report = Report::new(
            new_report_id(),
            scope,
            self.kind.unwrap_or_else(|| "finding".to_owned()),
            severity,
            self.title,
            self.text,
            filer,
            at,
        );
        for link in self.links {
            report = report.linked(ReportLink::new(link.kind.parse()?, link.id));
        }
        for link in links {
            if !report.links.contains(&link) {
                report = report.linked(link);
            }
        }
        report.priority = self.priority;
        report.deadline = self.deadline;
        report.from = self.information_from;
        report.sub_source = self.sub_source;
        report.classification = self.classification;
        report.handling = self.handling;
        report.compartments = self.compartments;
        report.tearline = self.tearline.map(TearlineFiling::into_tearline);
        report.validate(limits).map_err(|error| error.to_string())?;
        settings.check(&report)?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::directorates::registry::reports::ReportLinkKind;

    /// A reader with what a test gives it.
    #[derive(Default)]
    struct Given {
        scopes: Vec<ReportScope>,
        compartments: Vec<String>,
        subjects: Vec<String>,
        asked: Cell<usize>,
    }

    impl ReportReader for Given {
        fn reads_scope(&self, scope: &ReportScope) -> bool {
            self.asked.set(self.asked.get() + 1);
            self.scopes.contains(scope)
        }

        fn read_in(&self, compartment: &str) -> bool {
            self.compartments.iter().any(|each| each == compartment)
        }

        fn subjects(&self) -> Vec<String> {
            self.subjects.clone()
        }
    }

    fn clearances() -> Vec<ReportClearance> {
        [
            ("person:cleared", "confidential"),
            ("group:analysts", "restricted"),
            ("agent:007", "unknown-mark"),
        ]
        .into_iter()
        .map(|(subject, classification)| {
            ReportClearance::new(subject, classification, "person:chief", 1.0)
        })
        .collect()
    }

    fn operation() -> ReportScope {
        ReportScope::Operation("G".to_owned())
    }

    fn marked() -> Report {
        Report::new(
            "r",
            operation(),
            "finding",
            Severity::Warning,
            "secret title",
            "secret text",
            ReportFiler::Watchdog,
            1.0,
        )
        .linked(ReportLink::mission("G-1"))
        .classified("confidential")
        .in_compartments(vec!["KESTREL".to_owned()])
        .with_tearline(Tearline::new("released").classified("restricted"))
    }

    fn reader(subjects: &[&str], compartments: &[&str]) -> Given {
        Given {
            scopes: vec![operation()],
            compartments: compartments.iter().map(|each| (*each).to_owned()).collect(),
            subjects: subjects.iter().map(|each| (*each).to_owned()).collect(),
            ..Given::default()
        }
    }

    #[test]
    fn content_needs_the_scope_each_compartment_and_the_clearance() {
        let (settings, clearances) = (ReportSettings::default(), clearances());
        let rules = ReportRules::new(&settings, &clearances);
        let report = marked();
        let full = reader(&["person:cleared"], &["KESTREL"]);
        assert_eq!(sight(&full, &report, &rules), ReportSight::Content);
        // Read in but below its classification: the tearline, which is restricted.
        let analyst = reader(&["person:x", "group:analysts"], &["KESTREL"]);
        assert_eq!(sight(&analyst, &report, &rules), ReportSight::Tearline);
        // Cleared but not read in: still the tearline, which names no compartment.
        let outsider = reader(&["person:cleared"], &[]);
        assert_eq!(sight(&outsider, &report, &rules), ReportSight::Tearline);
        // Below the tearline's marking too, or an unknown classification: markings alone.
        for nobody in [
            reader(&["person:x"], &["KESTREL"]),
            reader(&["agent:007"], &["KESTREL"]),
        ] {
            assert_eq!(sight(&nobody, &report, &rules), ReportSight::Markings);
        }
        // Without the scope's read, even full clearance sees markings, as a link shows.
        let elsewhere = Given {
            scopes: Vec::new(),
            ..reader(&["person:cleared"], &["KESTREL"])
        };
        assert_eq!(sight(&elsewhere, &report, &rules), ReportSight::Markings);
        // No tearline: markings.
        let mut bare = report.clone();
        bare.tearline = None;
        assert_eq!(sight(&analyst, &bare, &rules), ReportSight::Markings);
    }

    #[test]
    fn what_each_sight_shows() {
        let report = marked();
        let markings = report_json(&report, ReportSight::Markings, None);
        assert_eq!(markings["id"], "r");
        assert_eq!(markings["classification"], "confidential");
        assert_eq!(markings["compartments"], json!(["KESTREL"]));
        let leaked = markings.to_string();
        for hidden in ["secret", "G-1", "released", "warning"] {
            assert!(!leaked.contains(hidden), "{hidden} in {leaked}");
        }
        let tearline = report_json(&report, ReportSight::Tearline, None);
        assert_eq!(tearline["text"], "released");
        assert!(tearline.get("title").is_none() && tearline.get("links").is_none());
        assert!(!tearline.to_string().contains("secret"));
        let content = report_json(&report, ReportSight::Content, None);
        assert_eq!(content["title"], "secret title");
        assert_eq!(content["text"], "secret text");
        assert_eq!(content["reliability"], "F");
        assert_eq!(content["untrusted"], true);
        assert_eq!(content["links"][0]["kind"], "mission");
    }

    #[test]
    fn lowering_markings_needs_someone_read_in() {
        let (settings, clearances) = (ReportSettings::default(), clearances());
        let rules = ReportRules::new(&settings, &clearances);
        let lower = ReportMarkings {
            classification: Some("restricted".to_owned()),
            tearline: None,
            ..ReportMarkings::default()
        };
        // Someone who reads only the tearline cannot lower it.
        let mut report = marked();
        let analyst = reader(&["group:analysts"], &["KESTREL"]);
        assert!(remark(&analyst, &mut report, lower.clone(), &rules).is_err());
        assert_eq!(report, marked(), "a refusal changes nothing");
        // Someone read in and cleared can.
        let full = reader(&["person:cleared"], &["KESTREL"]);
        remark(&full, &mut report, lower, &rules).unwrap();
        assert_eq!(report.classification.as_deref(), Some("restricted"));
        assert!(report.compartments.is_empty());
        assert_eq!(sight(&analyst, &report, &rules), ReportSight::Content);
        // Nobody marks it past what they themselves hold.
        let past = ReportMarkings {
            compartments: vec!["OTHER".to_owned()],
            ..ReportMarkings::default()
        };
        assert!(remark(&full, &mut report, past, &rules).is_err());
        // Raising within what they hold is theirs to do.
        let raise = ReportMarkings {
            classification: Some("confidential".to_owned()),
            compartments: vec!["KESTREL".to_owned()],
            ..ReportMarkings::default()
        };
        remark(&full, &mut report, raise, &rules).unwrap();
        assert_eq!(sight(&analyst, &report, &rules), ReportSight::Markings);
    }

    #[test]
    fn a_filing_becomes_a_report_with_its_links() {
        let settings = ReportSettings::default();
        let filing: ReportFiling = serde_json::from_value(json!({
            "title": "disk",
            "text": "full",
            "priority": "high",
            "classification": "restricted",
            "compartments": ["KESTREL"],
            "tearline": {"text": "a disk"},
            "links": [{"kind": "report", "id": "other"}],
        }))
        .unwrap();
        let report = filing
            .into_report(
                operation(),
                ReportFiler::Watchdog,
                [ReportLink::mission("G-1")],
                5.0,
                &ReportLimits::default(),
                &settings,
            )
            .unwrap();
        assert!(report.id.starts_with("report-"));
        assert_eq!(report.kind, "finding");
        assert_eq!(report.severity, Severity::Info);
        assert!(report.credibility.is_none(), "unset until assessed");
        assert_eq!(report.links[0].kind, ReportLinkKind::Report);
        assert!(report.about_mission("G-1"));
        assert_eq!(report.tearline.as_ref().unwrap().text, "a disk");
        for refused in [
            json!({"title": "t", "text": "x", "severity": "dire"}),
            json!({"title": "t", "text": "x", "priority": "someday"}),
            json!({"title": "t", "text": "x", "links": [{"kind": "rumour", "id": "y"}]}),
            json!({"title": "t", "text": "x", "credibility": 1}),
        ] {
            let filed = serde_json::from_value::<ReportFiling>(refused.clone())
                .map_err(|error| error.to_string())
                .and_then(|filing| {
                    filing.into_report(
                        operation(),
                        ReportFiler::Watchdog,
                        [],
                        5.0,
                        &ReportLimits::default(),
                        &settings,
                    )
                });
            assert!(filed.is_err(), "{refused}");
        }
    }

    #[test]
    fn the_lists_are_the_roster_records_and_default_without_one() {
        use crate::directorates::m::config::FileProvider;

        let root = tempfile::tempdir().unwrap();
        let provider = FileProvider::new(root.path());
        assert_eq!(settings(&provider).unwrap(), ReportSettings::default());
        std::fs::write(
            root.path().join("operations.toml"),
            "[roster.reports]\npriorities = [\"now\", \"later\"]\n",
        )
        .unwrap();
        let read = settings(&provider).unwrap();
        assert_eq!(read.priorities, ["now", "later"]);
        assert_eq!(
            read.classifications,
            ReportSettings::default().classifications
        );
        let mut report = marked();
        report.priority = Some("high".to_owned());
        assert!(
            read.check(&report).is_err(),
            "high is not in this deployment's list"
        );
        report.priority = Some("now".to_owned());
        read.check(&report).unwrap();
    }
}
