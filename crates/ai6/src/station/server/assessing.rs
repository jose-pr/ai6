//! Closing the Reports a Mission or a signature resolves, and opening again those whose time has
//! come, as the Station's beat and a Mission's settling find them.
//!
//! A Report assessed into something that has not ended waits as `assessed`. Every Station may run
//! this: closing and reopening are decided where the Registry holds the Report, so a Report two
//! Stations meet at once is changed once. The Operation's charge is told by Cable of each Report
//! that comes back, by its ID and why, never its words.

use super::DaemonState;
use crate::directorates::m::charges::ChargeScope;
use crate::directorates::registry::assessments::{self, Swept};
use crate::directorates::registry::cables::CablePrincipal;
use crate::directorates::registry::reports::Report;

/// One pass over every assessed Report: those whose Mission settled or whose green ink request
/// was signed are closed, and those whose deadline or time came are opened again. The settings
/// are read at every pass; when they cannot be read nothing is judged from memory.
pub(super) fn settle(state: &DaemonState) {
    let Ok(settings) = state.config.report_settings() else {
        return;
    };
    let now = state.clock.now() as f64;
    match assessments::sweep(state.intake.as_ref(), now, &settings) {
        Ok(swept) => told(state, &swept),
        Err(error) => eprintln!("ai6: assessed reports not brought up to date: {error}"),
    }
}

/// Closes the Reports assessed `mission` that name the Mission `mission_id`, which has just
/// settled.
pub(super) fn mission_settled(state: &DaemonState, mission_id: &str) {
    let Ok(settings) = state.config.report_settings() else {
        return;
    };
    let now = state.clock.now() as f64;
    if let Err(error) =
        assessments::mission_settled(state.intake.as_ref(), mission_id, now, &settings)
    {
        eprintln!("ai6: Mission {mission_id:?}: the reports it resolves were not closed: {error}");
    }
}

fn told(state: &DaemonState, swept: &Swept) {
    for report in &swept.reopened {
        tell(state, report);
    }
}

/// Tells the charge of the Report's Operation that `report` is open again.
fn tell(state: &DaemonState, report: &Report) {
    let Some(operation) = report.scope.operation() else {
        return;
    };
    let Some(agent) = state
        .roster
        .charges()
        .get(&ChargeScope::Operation(operation.to_owned()).to_string())
        .map(|charge| charge.settings.agent.clone())
    else {
        return;
    };
    let Ok(from) = CablePrincipal::try_new(
        "report:clock".to_owned(),
        operation.to_owned(),
        None,
        state.station_id.clone(),
        None,
        None,
    ) else {
        return;
    };
    let body = serde_json::json!({
        "report": {
            "change": "reopened",
            "report": report.id,
            "kind": report.kind,
            "was": report
                .assessment
                .as_ref()
                .map(|assessment| assessment.verdict.as_str()),
        },
        "next": "read_report shows it; assess_report decides it again",
    });
    let key = format!("report-{}-reopened-{}", report.id, report.updated as u64);
    if let Err(error) = state
        .cables
        .notice(&from, operation, &agent, body.to_string(), &key)
    {
        eprintln!(
            "ai6: the charge of {operation} was not told that report {} is open: {error:?}",
            report.id
        );
    }
}
