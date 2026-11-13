//! A Source's item as a Report, and the decision on what it becomes.
//!
//! Each item is one Report of kind [`ITEM_KIND`] in its Operation's scope, whose ID is fixed by
//! the Operation, the Source instance and the item. Polling the item again changes that Report
//! and never files a second; an item that has not changed leaves it alone. The Operation's
//! charge, or a person where there is none, decides the item: a `mission` is queued by the
//! Station that polls the Source, so the Registry keeps one path from an item to a brief.
//!
//! Every decision is audited first, then recorded on the Report and the Operation's timeline.

use std::io;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::assessments::{self, Assessment, Context, Verdict, assess};
use super::backend::RegistryBackend;
use super::logbook::Severity;
use super::reports::{
    Report, ReportFiler, ReportFilter, ReportLink, ReportLinkKind, ReportScope, ReportState,
};
use super::sources::SourceItem;
use super::store::{AuditRecord, hex_digest, one_line_within};
use super::timeline::TimelineEvent;
use crate::directorates::m::reports::ReportSettings;

/// The kind of a Report made from a Source item.
pub const ITEM_KIND: &str = "source-item";

/// The ID of the Report for `item` of the Source `instance` in `operation`.
pub fn report_id(operation: &str, instance: &str, item: &str) -> String {
    let mut digest = Sha256::new();
    for part in [operation, instance, item] {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    format!("src-{}", &hex_digest(&digest.finalize())[..32])
}

/// What polling an item did to its Report.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum Offered {
    /// The item is new: its Report was filed.
    Filed(Report),
    /// The item changed since its Report was written: the Report now holds it. `reopened` says
    /// a closed or deferred Report is open again for the charge.
    Changed {
        /// The Report as changed.
        report: Report,
        /// Whether it was closed, or deferred, and is open again.
        reopened: bool,
    },
    /// The Report already holds the item exactly.
    Unchanged(Report),
}

impl Offered {
    /// The Report.
    pub fn report(&self) -> &Report {
        match self {
            Self::Filed(report) | Self::Unchanged(report) => report,
            Self::Changed { report, .. } => report,
        }
    }
}

/// Cuts `text` to at most `bytes` bytes on a character boundary.
fn cut(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// The item as a Report keeps it: its words cut to the Report's text bound.
fn kept(store: &dyn RegistryBackend, item: &SourceItem) -> SourceItem {
    let bytes = store.limits().reports.text_bytes.or_max();
    SourceItem {
        title: cut(&item.title, bytes),
        body: cut(&item.body, bytes),
        ..item.clone()
    }
}

fn words(store: &dyn RegistryBackend, item: &SourceItem) -> (String, String) {
    let limits = &store.limits().reports;
    let title = one_line_within(&item.title, limits.name_bytes.or_max());
    let title = if title.is_empty() {
        one_line_within(&item.id, limits.name_bytes.or_max())
    } else {
        title
    };
    let text = if item.body.trim().is_empty() {
        item.title.as_str()
    } else {
        item.body.as_str()
    };
    let text = cut(text, limits.text_bytes.or_max());
    let text = if text.trim().is_empty() {
        title.clone()
    } else {
        text
    };
    (title, text)
}

/// Files the Report for `item`, or brings the one that is there up to date.
///
/// # Errors
///
/// An error from the Registry, or `InvalidInput` for an item whose names are out of the
/// Report's bounds.
pub fn offer(
    store: &dyn RegistryBackend,
    operation: &str,
    instance: &str,
    item: &SourceItem,
    now: f64,
) -> io::Result<Offered> {
    let id = report_id(operation, instance, &item.id);
    let item = kept(store, item);
    let (title, text) = words(store, &item);
    let Some(stored) = store.report(&id)? else {
        let report = Report::new(
            id,
            ReportScope::Operation(operation.to_owned()),
            ITEM_KIND,
            Severity::Info,
            title,
            text,
            ReportFiler::Source {
                source: instance.to_owned(),
                item: item.id.clone(),
            },
            now,
        )
        .linked(ReportLink::new(
            ReportLinkKind::Evidence,
            format!("source:{instance}:{}", item.id),
        ))
        .from_item(item);
        store.file_report(&report)?;
        return Ok(Offered::Filed(report));
    };
    if stored.item.as_ref() == Some(&item) {
        return Ok(Offered::Unchanged(stored));
    }
    let deferred = stored
        .assessment
        .as_ref()
        .is_some_and(|assessment| matches!(assessment.verdict.as_str(), "later" | "awaiting"));
    let reopened =
        stored.state == ReportState::Closed || (stored.state == ReportState::Assessed && deferred);
    let report = store.change_report(&id, &mut |report| {
        report.title = title.clone();
        report.text = text.clone();
        report.item = Some(item.clone());
        report.updated = now.max(report.created);
        if reopened {
            report.state = ReportState::Open;
            report.resolution = None;
            report.deadline = None;
        }
        Ok(())
    })?;
    if reopened {
        let by = format!("source:{instance}");
        store.record_audit(&AuditRecord {
            id: uuid::Uuid::new_v4().to_string(),
            actor: by.clone(),
            action: "source.item.reopen".to_owned(),
            operation: operation.to_owned(),
            resource: Some(format!("operation:{operation}")),
            target: format!("{id} changed"),
            at: now,
        })?;
        let _ =
            store.record_event(&TimelineEvent::new("report", "reopened", operation, now).actor(by));
    }
    Ok(Offered::Changed { report, reopened })
}

/// What a charge or a person decided an item becomes.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ItemVerdict {
    /// Work: the item is queued as a brief, vetted as any brief is.
    Mission,
    /// The same as a Mission or another item, named.
    Duplicate {
        /// The Mission, Report or item it repeats.
        of: String,
    },
    /// Nothing to do, for the reason given.
    NoAction,
    /// Look again at the time, in Unix seconds.
    Later {
        /// When it comes back to the deciding charge or person.
        until: f64,
    },
}

impl ItemVerdict {
    /// The verdict `word` names: `mission` and `no_action` take nothing, `duplicate` takes what
    /// it repeats in `of`, and `later` takes `wait_seconds` from `now` (Unix seconds).
    ///
    /// # Errors
    ///
    /// Says what is missing, left over or not a verdict.
    pub fn parse(
        word: &str,
        of: Option<&str>,
        wait_seconds: Option<u64>,
        now: f64,
    ) -> Result<Self, String> {
        match (word, of, wait_seconds) {
            ("mission", None, None) => Ok(Self::Mission),
            ("no_action", None, None) => Ok(Self::NoAction),
            ("duplicate", Some(of), None) => Ok(Self::Duplicate { of: of.to_owned() }),
            ("later", None, Some(wait)) => Ok(Self::Later {
                until: now + wait as f64,
            }),
            ("duplicate", None, _) => Err("a duplicate names what it repeats in `of`".to_owned()),
            ("later", _, None) => Err("later takes wait_seconds".to_owned()),
            ("mission" | "no_action" | "duplicate" | "later", ..) => {
                Err("only duplicate takes `of` and only later takes wait_seconds".to_owned())
            }
            _ => Err("an assessment is mission, duplicate, no_action or later".to_owned()),
        }
    }

    /// The word of the verdict: `mission`, `duplicate`, `no_action` or `later`.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Mission => "mission",
            Self::Duplicate { .. } => "duplicate",
            Self::NoAction => "no_action",
            Self::Later { .. } => "later",
        }
    }
}

/// What `by` decided of the item whose Report is `report`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct ItemDecision<'a> {
    /// The Operation whose item it is.
    pub operation: &'a str,
    /// The Report's ID.
    pub report: &'a str,
    /// What the item becomes.
    pub verdict: ItemVerdict,
    /// Why; kept as the assessor's comment, apart from the item's words.
    pub reason: &'a str,
    /// Who decided: a person ID, or `charge:operation:<codename>`.
    pub by: &'a str,
    /// When, as Unix seconds.
    pub at: f64,
}

impl<'a> ItemDecision<'a> {
    /// A decision of `by` on `report`.
    pub fn new(
        operation: &'a str,
        report: &'a str,
        verdict: ItemVerdict,
        reason: &'a str,
        by: &'a str,
        at: f64,
    ) -> Self {
        Self {
            operation,
            report,
            verdict,
            reason,
            by,
            at,
        }
    }
}

/// The item Report `id` of `operation`, when it is one.
///
/// # Errors
///
/// `NotFound` for no such Report in the Operation, or one that is not made from a Source item.
pub fn item_report(store: &dyn RegistryBackend, operation: &str, id: &str) -> io::Result<Report> {
    store
        .report(id)?
        .filter(|report| {
            report.kind == ITEM_KIND
                && report.scope == ReportScope::Operation(operation.to_owned())
                && matches!(report.filer, ReportFiler::Source { .. })
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such Source item"))
}

/// Decides an open item: audited, kept on its Report as the assessment, and on the timeline. A
/// `mission` leaves the Report assessed, and the Station that polls the Source queues the brief
/// on its next pass; `duplicate` and `no_action` close it, and `later` keeps it until its time.
/// This is [`assess`] for an item, which any other Report takes as well.
///
/// # Errors
///
/// `NotFound` for no such item, `InvalidInput` for one already decided or for a decision without
/// its reason, a duplicate that names nothing or a time that is not ahead; an audit that cannot
/// be written, or a Registry failure.
pub fn decide_item(
    store: &dyn RegistryBackend,
    decision: &ItemDecision<'_>,
    settings: &ReportSettings,
) -> io::Result<Report> {
    item_report(store, decision.operation, decision.report)?;
    let verdict = match &decision.verdict {
        ItemVerdict::Mission => Verdict::Mission { mission: None },
        ItemVerdict::Duplicate { of } => Verdict::Duplicate { of: of.clone() },
        ItemVerdict::NoAction => Verdict::NoAction,
        ItemVerdict::Later { until } => Verdict::Later {
            until: Some(*until),
            condition: None,
        },
    };
    let own = ReportScope::Operation(decision.operation.to_owned());
    let reads = |scope: &ReportScope| *scope == own;
    assess(
        store,
        &Assessment::new(
            decision.report,
            verdict,
            decision.reason,
            decision.by,
            decision.at,
        ),
        &Context::within(settings, &reads),
    )
}

/// Opens again the items deferred with `later` (or waiting with `awaiting`) whose time has come,
/// audited, and returns them.
///
/// # Errors
///
/// An error from the Registry.
pub fn reopen_due(
    store: &dyn RegistryBackend,
    operation: &str,
    now: f64,
) -> io::Result<Vec<Report>> {
    assessments::reopen_due(
        store,
        ReportFilter::all()
            .in_scope(ReportScope::Operation(operation.to_owned()))
            .of_kind(ITEM_KIND),
        now,
    )
}

/// The items decided `mission` whose brief has not been queued: assessed, and tied to no
/// Mission yet.
///
/// # Errors
///
/// An error from the Registry.
pub fn awaiting_queue(store: &dyn RegistryBackend, operation: &str) -> io::Result<Vec<Report>> {
    Ok(store
        .reports(
            &ReportFilter::all()
                .in_scope(ReportScope::Operation(operation.to_owned()))
                .of_kind(ITEM_KIND)
                .in_state(ReportState::Assessed),
        )?
        .into_iter()
        .filter(|report| {
            report
                .assessment
                .as_ref()
                .is_some_and(|assessment| assessment.verdict == "mission")
                && !report
                    .links
                    .iter()
                    .any(|link| link.kind == ReportLinkKind::Mission)
        })
        .collect())
}

/// Whether the Report's item was decided `mission`: from then on polling it takes the path an
/// item has always taken, and the Mission is its own.
pub fn is_mission(report: &Report) -> bool {
    report
        .assessment
        .as_ref()
        .is_some_and(|assessment| assessment.verdict == "mission")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        an_item_is_one_report_however_often_it_changes,
        a_decision_is_audited_and_kept_on_the_report,
        a_deferred_item_comes_back_at_its_time,
    );

    fn item(id: &str, body: &str) -> SourceItem {
        SourceItem {
            id: id.to_owned(),
            title: "Fix the build".to_owned(),
            body: body.to_owned(),
            url: Some("https://example.invalid/1".to_owned()),
            labels: vec!["bug".to_owned()],
            author: Some("reporter".to_owned()),
            updated: "2026-01-01T00:00:00Z".to_owned(),
            code: None,
            code_tag: None,
        }
    }

    fn audits(store: &dyn RegistryBackend, action: &str) -> usize {
        store
            .audit_records()
            .unwrap()
            .iter()
            .filter(|record| record.action == action)
            .count()
    }

    fn an_item_is_one_report_however_often_it_changes(store: &dyn Store) {
        let registry = store.handle();
        let first = item("o/r#1", "It fails.");
        let filed = offer(registry.as_ref(), "G", "github", &first, 100.0).unwrap();
        let Offered::Filed(report) = &filed else {
            panic!("a new item is filed: {filed:?}");
        };
        assert_eq!(report.kind, ITEM_KIND);
        assert_eq!(
            report.filer,
            ReportFiler::Source {
                source: "github".to_owned(),
                item: "o/r#1".to_owned()
            }
        );
        assert!(report.untrusted(), "an item's words are never a person's");
        assert_eq!(report.state, ReportState::Open);
        assert_eq!(report.item.as_ref(), Some(&first));
        assert_eq!(
            report.filer.source_key(),
            "source:github",
            "the Source is the Report's source"
        );
        let id = report.id.clone();
        assert!(matches!(
            offer(registry.as_ref(), "G", "github", &first, 110.0).unwrap(),
            Offered::Unchanged(_)
        ));
        let second = item("o/r#1", "It fails on macOS.");
        let Offered::Changed { report, reopened } =
            offer(registry.as_ref(), "G", "github", &second, 120.0).unwrap()
        else {
            panic!("a changed item changes its Report");
        };
        assert!(!reopened);
        assert_eq!(report.text, "It fails on macOS.");
        let third = item("o/r#1", "It fails on macOS and Windows.");
        offer(registry.as_ref(), "G", "github", &third, 130.0).unwrap();
        let all = registry
            .reports(&ReportFilter::all().of_kind(ITEM_KIND))
            .unwrap();
        assert_eq!(all.len(), 1, "one Report per item");
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].item.as_ref(), Some(&third));
        assert_eq!(all[0].created, 100.0);
        assert_eq!(all[0].updated, 130.0);
        // Another instance's item of the same id is another Report.
        offer(registry.as_ref(), "G", "gitlab", &first, 140.0).unwrap();
        assert_eq!(
            registry
                .reports(&ReportFilter::all().of_kind(ITEM_KIND))
                .unwrap()
                .len(),
            2
        );
    }

    fn a_decision_is_audited_and_kept_on_the_report(store: &dyn Store) {
        let registry = store.handle();
        let store = registry.as_ref();
        let id = |n: u32| {
            offer(store, "G", "github", &item(&format!("o/r#{n}"), "x"), 100.0)
                .unwrap()
                .report()
                .id
                .clone()
        };
        let (a, b, c) = (id(1), id(2), id(3));
        let decide = |report: &str, verdict: ItemVerdict, reason: &str| {
            decide_item(
                store,
                &ItemDecision::new("G", report, verdict, reason, "charge:operation:G", 200.0),
                &ReportSettings::default(),
            )
        };
        let closed = decide(
            &a,
            ItemVerdict::Duplicate {
                of: "G-github-o_r_9".to_owned(),
            },
            "same as the open one",
        )
        .unwrap();
        assert_eq!(closed.state, ReportState::Closed);
        assert_eq!(closed.assessment.as_ref().unwrap().verdict, "duplicate");
        assert_eq!(
            closed.resolution.as_ref().unwrap().note.as_deref(),
            Some("duplicate of G-github-o_r_9")
        );
        assert!(
            closed.links.iter().any(|link| link.id == "G-github-o_r_9"),
            "the duplicate names what it repeats"
        );
        let none = decide(&b, ItemVerdict::NoAction, "a question, not work").unwrap();
        assert_eq!(none.state, ReportState::Closed);
        assert_eq!(
            none.assessment.as_ref().unwrap().comment.as_deref(),
            Some("a question, not work"),
            "the comment is apart from the item's words"
        );
        assert_eq!(audits(store, "report.assess"), 2);
        // Decided once: a second decision on a closed item is refused.
        let again = decide(&a, ItemVerdict::NoAction, "again").unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::InvalidInput);
        assert!(
            decide(&c, ItemVerdict::NoAction, " ").is_err(),
            "needs a reason"
        );
        assert!(
            decide(&c, ItemVerdict::Later { until: 100.0 }, "soon").is_err(),
            "later needs a time ahead"
        );
        assert!(
            decide(&c, ItemVerdict::Duplicate { of: " ".to_owned() }, "x").is_err(),
            "a duplicate names something"
        );
        // Another Operation's item is not found.
        let other = decide_item(
            store,
            &ItemDecision::new("H", &c, ItemVerdict::NoAction, "x", "p", 200.0),
            &ReportSettings::default(),
        )
        .unwrap_err();
        assert_eq!(other.kind(), io::ErrorKind::NotFound);
        let mission = decide(&c, ItemVerdict::Mission, "a real fault").unwrap();
        assert_eq!(mission.state, ReportState::Assessed);
        assert_eq!(awaiting_queue(store, "G").unwrap().len(), 1);
        // A changed item that was closed is open again, audited.
        let Offered::Changed { reopened, .. } = offer(
            store,
            "G",
            "github",
            &item("o/r#2", "It is work after all."),
            300.0,
        )
        .unwrap() else {
            panic!("changed");
        };
        assert!(reopened);
        assert_eq!(audits(store, "source.item.reopen"), 1);
        assert_eq!(store.report(&b).unwrap().unwrap().state, ReportState::Open);
    }

    fn a_deferred_item_comes_back_at_its_time(store: &dyn Store) {
        let registry = store.handle();
        let store = registry.as_ref();
        let id = offer(store, "G", "github", &item("o/r#1", "x"), 100.0)
            .unwrap()
            .report()
            .id
            .clone();
        let later = decide_item(
            store,
            &ItemDecision::new(
                "G",
                &id,
                ItemVerdict::Later { until: 500.0 },
                "after the release",
                "charge:operation:G",
                200.0,
            ),
            &ReportSettings::default(),
        )
        .unwrap();
        assert_eq!(later.state, ReportState::Assessed);
        assert_eq!(later.deadline, Some(500.0));
        assert!(reopen_due(store, "G", 499.0).unwrap().is_empty());
        let opened = reopen_due(store, "G", 500.0).unwrap();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].state, ReportState::Open);
        assert!(reopen_due(store, "G", 600.0).unwrap().is_empty());
        assert_eq!(audits(store, "report.reopen"), 1);
    }
}
