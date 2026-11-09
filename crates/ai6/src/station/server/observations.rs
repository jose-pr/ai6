//! What the Station's own checks observe, filed as Reports beside the record each already keeps:
//! deliverables read back, a brief held by vetting or by the Chief of Staff, a scorecard proposal.
//!
//! Each is tied to what it is about, and filing again for the same thing updates the one Report,
//! so a probe run twice or a hold seen on every pass is not a flood. Filing never stops the check:
//! a Report that cannot be stored is logged and the check goes on.

use sha2::{Digest, Sha256};

use super::DaemonState;
use crate::directorates::registry::logbook::Severity;
use crate::directorates::registry::reports::{
    Report, ReportFiler, ReportLink, ReportLinkKind, ReportScope,
};

/// One thing observed.
pub(super) struct Observed<'a> {
    /// The Operation it is filed against.
    pub operation: &'a str,
    /// Who observed it.
    pub filer: ReportFiler,
    /// The Report's kind.
    pub kind: &'a str,
    pub severity: Severity,
    /// What it is about, apart from the Operation: the same value finds the same Report.
    pub about: &'a str,
    pub title: String,
    pub text: String,
    /// The Mission it is about.
    pub mission: Option<&'a str>,
    /// What else it is tied to: a decision, evidence.
    pub links: Vec<ReportLink>,
}

/// Cuts `text` to at most `bytes` bytes on a character boundary, as one line.
fn line(text: &str, bytes: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    let mut end = flat.len().min(bytes);
    while !flat.is_char_boundary(end) {
        end -= 1;
    }
    flat[..end].to_owned()
}

fn cut(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// The ID of the Report about `about` in `operation` of `kind`, filed by `filer`.
fn id_of(operation: &str, filer: &ReportFiler, kind: &str, about: &str) -> String {
    let mut digest = Sha256::new();
    for part in [operation, &filer.source_key(), kind, about] {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    let hex: String = digest
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("obs-{hex}")
}

/// Files the Report, or brings the one for the same thing up to date.
pub(super) fn observe(state: &DaemonState, observed: Observed<'_>) {
    let limits = state.intake.limits().reports.clone();
    let now = state.clock.now().max(1) as f64;
    let id = id_of(
        observed.operation,
        &observed.filer,
        observed.kind,
        observed.about,
    );
    let mut title = line(&observed.title, limits.name_bytes.or_max());
    if title.is_empty() {
        title = observed.kind.to_owned();
    }
    let mut text = cut(&observed.text, limits.text_bytes.or_max());
    if text.trim().is_empty() {
        text = title.clone();
    }
    let result = match state.intake.report(&id) {
        Ok(Some(_)) => state
            .intake
            .change_report(&id, &mut |report| {
                report.title = title.clone();
                report.text = text.clone();
                report.severity = observed.severity;
                report.updated = now.max(report.created);
                Ok(())
            })
            .map(|_| ()),
        Ok(None) => {
            let mut report = Report::new(
                id.clone(),
                ReportScope::Operation(observed.operation.to_owned()),
                observed.kind,
                observed.severity,
                title.clone(),
                text.clone(),
                observed.filer.clone(),
                now,
            );
            if let Some(mission) = observed.mission {
                report = report.linked(ReportLink::mission(mission));
            }
            for link in &observed.links {
                report = report.linked(link.clone());
            }
            state.intake.file_report(&report)
        }
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!(
            "ai6: a {} report about {:?} was not kept: {error}",
            observed.kind, observed.about
        );
    }
}

/// A link to a decision (a green ink request).
pub(super) fn decision(id: &str) -> ReportLink {
    ReportLink::new(ReportLinkKind::Decision, id)
}
