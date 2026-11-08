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
//! The caller answers [`ReportReader`] from what is granted at that moment (the access policy,
//! `compartments::current`) and keeps nothing between calls.

use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::directorates::registry::limits::ReportLimits;
use crate::directorates::registry::logbook::Severity;
use crate::directorates::registry::reports::{
    Report, ReportFiler, ReportLink, ReportScope, ReportSource, Tearline,
};

/// Who is asking about a Report, as granted now.
pub trait ReportReader {
    /// Whether the reader holds the scope's read. A read through a compartment is a crossing the
    /// reader writes to the audit log before answering `true`.
    fn reads_scope(&self, scope: &ReportScope) -> bool;

    /// Whether the reader is read in to `compartment`. One that is unknown or cannot be read
    /// answers `false`.
    fn read_in(&self, compartment: &str) -> bool;

    /// What the reader's clearance is looked up by in `[registry.limits.reports] clearances`:
    /// `person:ID` and `group:NAME` for a person, `agent:CODENAME` for an Agent.
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
    limits: &ReportLimits,
    classification: Option<&str>,
    compartments: &[String],
) -> bool {
    let subjects = reader.subjects();
    let held = limits.clearance(subjects.iter().map(String::as_str));
    limits
        .rank(classification)
        .is_some_and(|needed| held >= needed)
        && compartments
            .iter()
            .all(|compartment| reader.read_in(compartment))
}

/// What `reader` sees of `report`. Without the scope's read it is [`ReportSight::Markings`], as
/// for anything that only links to it.
pub fn sight(reader: &dyn ReportReader, report: &Report, limits: &ReportLimits) -> ReportSight {
    if !reader.reads_scope(&report.scope) {
        return ReportSight::Markings;
    }
    if cleared(
        reader,
        limits,
        report.classification.as_deref(),
        &report.compartments,
    ) {
        return ReportSight::Content;
    }
    match &report.tearline {
        Some(tearline)
            if cleared(
                reader,
                limits,
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
    /// From `[registry.limits.reports] classifications`; unset is the lowest.
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
    limits: &ReportLimits,
) -> Result<(), String> {
    if sight(reader, report, limits) != ReportSight::Content {
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
        limits,
        changed.classification.as_deref(),
        &changed.compartments,
    ) {
        return Err(
            "you may mark a report only as far as you are cleared and read in yourself".to_owned(),
        );
    }
    changed
        .validate(limits)
        .map_err(|error| error.to_string())?;
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
    /// From `[registry.limits.reports] priorities`.
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
    /// From `[registry.limits.reports] classifications`; unset is the lowest.
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
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;

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

    fn limits() -> ReportLimits {
        ReportLimits {
            clearances: BTreeMap::from([
                ("person:cleared".to_owned(), "confidential".to_owned()),
                ("group:analysts".to_owned(), "restricted".to_owned()),
                ("agent:007".to_owned(), "unknown-mark".to_owned()),
            ]),
            ..ReportLimits::default()
        }
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
        let limits = limits();
        let report = marked();
        let full = reader(&["person:cleared"], &["KESTREL"]);
        assert_eq!(sight(&full, &report, &limits), ReportSight::Content);
        // Read in but below its classification: the tearline, which is restricted.
        let analyst = reader(&["person:x", "group:analysts"], &["KESTREL"]);
        assert_eq!(sight(&analyst, &report, &limits), ReportSight::Tearline);
        // Cleared but not read in: still the tearline, which names no compartment.
        let outsider = reader(&["person:cleared"], &[]);
        assert_eq!(sight(&outsider, &report, &limits), ReportSight::Tearline);
        // Below the tearline's marking too, or an unknown classification: markings alone.
        for nobody in [
            reader(&["person:x"], &["KESTREL"]),
            reader(&["agent:007"], &["KESTREL"]),
        ] {
            assert_eq!(sight(&nobody, &report, &limits), ReportSight::Markings);
        }
        // Without the scope's read, even full clearance sees markings, as a link shows.
        let elsewhere = Given {
            scopes: Vec::new(),
            ..reader(&["person:cleared"], &["KESTREL"])
        };
        assert_eq!(sight(&elsewhere, &report, &limits), ReportSight::Markings);
        // No tearline: markings.
        let mut bare = report.clone();
        bare.tearline = None;
        assert_eq!(sight(&analyst, &bare, &limits), ReportSight::Markings);
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
        let limits = limits();
        let lower = ReportMarkings {
            classification: Some("restricted".to_owned()),
            tearline: None,
            ..ReportMarkings::default()
        };
        // Someone who reads only the tearline cannot lower it.
        let mut report = marked();
        let analyst = reader(&["group:analysts"], &["KESTREL"]);
        assert!(remark(&analyst, &mut report, lower.clone(), &limits).is_err());
        assert_eq!(report, marked(), "a refusal changes nothing");
        // Someone read in and cleared can.
        let full = reader(&["person:cleared"], &["KESTREL"]);
        remark(&full, &mut report, lower, &limits).unwrap();
        assert_eq!(report.classification.as_deref(), Some("restricted"));
        assert!(report.compartments.is_empty());
        assert_eq!(sight(&analyst, &report, &limits), ReportSight::Content);
        // Nobody marks it past what they themselves hold.
        let past = ReportMarkings {
            compartments: vec!["OTHER".to_owned()],
            ..ReportMarkings::default()
        };
        assert!(remark(&full, &mut report, past, &limits).is_err());
        // Raising within what they hold is theirs to do.
        let raise = ReportMarkings {
            classification: Some("confidential".to_owned()),
            compartments: vec!["KESTREL".to_owned()],
            ..ReportMarkings::default()
        };
        remark(&full, &mut report, raise, &limits).unwrap();
        assert_eq!(sight(&analyst, &report, &limits), ReportSight::Markings);
    }

    #[test]
    fn a_filing_becomes_a_report_with_its_links() {
        let limits = limits();
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
                &limits,
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
                    filing.into_report(operation(), ReportFiler::Watchdog, [], 5.0, &limits)
                });
            assert!(filed.is_err(), "{refused}");
        }
    }
}
