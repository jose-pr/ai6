//! The Station's side of a Session's Report tools.
//!
//! Every Agent's Session files Reports against its Mission's Operation, linked to that Mission,
//! and reads them under the same rule as a person: the Operation's read (its own Operation, or
//! another through a compartment granting `mission.read`, audited first), being read in to each
//! of a Report's compartments, and a clearance at or above its classification. A Head of
//! Station round also reads its own Station's Reports. Filing creates no Mission, brief or
//! placement.
//!
//! Every call asks the configuration what is granted now, and works from the Session as the
//! Station started it. What it reads is fenced as data; a Report from another Operation marks the
//! Session exposed as that Operation's product.

use std::collections::BTreeMap;
use std::io;
use std::sync::Mutex;

use serde_json::{Value, json};
use uuid::Uuid;

use super::compartments::Reader;
use super::{DaemonState, agents};
use crate::bounds::Limit;
use crate::directorates::m::compartments::{
    CompartmentAction, CompartmentResource, CompiledCompartments, FieldPrincipal,
};
use crate::directorates::m::memories::{bounded, fenced};
use crate::directorates::m::reports::{
    ReportFiling, ReportReader, ReportRules, ReportSettings, ReportSight, report_json, sight,
};
use crate::directorates::medical::exposure::product_source;
use crate::directorates::registry::reports::{
    Report, ReportClearance, ReportFiler, ReportFilter, ReportLink, ReportScope, ReportState,
};
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::registry::timeline::TimelineEvent;

/// What an audit record's text may hold.
const AUDIT_BYTES: Limit = Limit::at(512);

/// What a call answers when who is asking cannot be settled.
const UNAVAILABLE: &str = "reports are unavailable";

/// A Session asking about Reports, as granted at this call.
struct SessionReader<'a> {
    state: &'a DaemonState,
    reader: &'a Reader,
    principal: FieldPrincipal,
    /// The Station whose Reports a Head of Station round reads.
    station: Option<&'a str>,
    /// What is granted now; `None` when it cannot be read, which grants nothing.
    compiled: Option<CompiledCompartments>,
    /// Each Operation crossed into in this call, so one is audited once.
    crossed: Mutex<BTreeMap<String, bool>>,
    /// The deployment's lists, as the provider holds them now.
    settings: ReportSettings,
    /// Every clearance, as the Registry holds them now.
    clearances: Vec<ReportClearance>,
}

impl SessionReader<'_> {
    fn crosses_into(&self, operation: &str) -> bool {
        let mut crossed = self
            .crossed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(known) = crossed.get(operation) {
            return *known;
        }
        let resource = CompartmentResource::Operation(operation.to_owned());
        let through = self.compiled.as_ref().and_then(|compiled| {
            compiled.field_allows(&self.principal, CompartmentAction::MissionRead, &resource)
        });
        let allowed = through.is_some_and(|compartment| {
            self.state
                .intake
                .record_audit(&AuditRecord {
                    id: format!("report-{}", Uuid::new_v4().simple()),
                    actor: format!("agent:{}", self.reader.agent),
                    action: "compartment.read".to_owned(),
                    operation: self.reader.operation.clone(),
                    resource: Some(format!("operation:{operation}")),
                    target: bounded(&format!("{compartment} reports -"), AUDIT_BYTES),
                    at: self.state.clock.now() as f64,
                })
                .is_ok()
        });
        crossed.insert(operation.to_owned(), allowed);
        allowed
    }
}

impl ReportReader for SessionReader<'_> {
    fn reads_scope(&self, scope: &ReportScope) -> bool {
        match scope {
            ReportScope::Operation(operation) if *operation == self.reader.operation => true,
            ReportScope::Operation(operation) => self.crosses_into(operation),
            ReportScope::Station(station) => self.station == Some(station.as_str()),
            ReportScope::M => false,
        }
    }

    fn read_in(&self, compartment: &str) -> bool {
        self.compiled
            .as_ref()
            .is_some_and(|compiled| compiled.field_read_in(compartment, &self.principal))
    }

    fn subjects(&self) -> Vec<String> {
        vec![format!("agent:{}", self.reader.agent)]
    }
}

/// How a refusal reads to the Session.
fn refused(reason: impl Into<String>) -> Value {
    json!({ "status": "refused", "reason": reason.into() })
}

/// What a store error says to the Session.
fn store_refusal(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::QuotaExceeded | io::ErrorKind::InvalidInput => error.to_string(),
        io::ErrorKind::InvalidData => "the Registry holds as many reports as it may".to_owned(),
        _ => "the report could not be stored".to_owned(),
    }
}

/// A Session's way to its Reports.
#[derive(Clone)]
pub(super) struct ReportDesk {
    state: DaemonState,
}

impl ReportDesk {
    pub(super) fn new(state: DaemonState) -> Self {
        Self { state }
    }

    /// Bytes of a Report's title and of each name on it.
    pub(super) fn name_bytes(&self) -> usize {
        self.state.intake.limits().reports.name_bytes.or_max()
    }

    /// Who `reader` is now and what is granted now, for `work`. An Agent off the roster holds
    /// nothing, and neither does anyone when the grants cannot be read.
    fn with_reader(
        &self,
        reader: &Reader,
        station: Option<&str>,
        work: impl FnOnce(&DaemonState, &SessionReader<'_>) -> Value,
    ) -> Result<Value, String> {
        let state = self.state.current();
        let clearance = agents::effective(&state, Some(&reader.operation), &reader.agent)
            .map_err(|_| UNAVAILABLE.to_owned())?
            .clearance;
        let settings = state
            .config
            .report_settings()
            .map_err(|_| UNAVAILABLE.to_owned())?;
        let clearances = state
            .intake
            .report_clearances()
            .map_err(|_| UNAVAILABLE.to_owned())?;
        let caller = SessionReader {
            state: &state,
            reader,
            principal: FieldPrincipal::new(
                reader.agent.clone(),
                reader.operation.clone(),
                reader.mission.clone(),
                clearance,
            ),
            station,
            compiled: state.config.compartments().ok(),
            crossed: Mutex::default(),
            settings,
            clearances,
        };
        Ok(work(&state, &caller))
    }

    /// Files `filing` against the Session's Operation, linked to its Mission. `session` is the
    /// provider Session once the harness has reported it.
    pub(super) fn file(
        &self,
        reader: &Reader,
        session: Option<String>,
        filing: ReportFiling,
    ) -> Result<Value, String> {
        let mission = reader
            .mission
            .clone()
            .ok_or_else(|| "file_report needs the Session's Mission".to_owned())?;
        let state = &self.state;
        let filer = ReportFiler::Agent {
            agent: reader.agent.clone(),
            session: session.unwrap_or_else(|| mission.clone()),
            mission: mission.clone(),
        };
        let report = filing.into_report(
            ReportScope::Operation(reader.operation.clone()),
            filer,
            [ReportLink::mission(mission.clone())],
            state.clock.now() as f64,
            &state.intake.limits().reports,
            &state
                .config
                .report_settings()
                .map_err(|_| UNAVAILABLE.to_owned())?,
        )?;
        state
            .intake
            .file_report(&report)
            .map_err(|error| format!("the report was not filed: {}", store_refusal(&error)))?;
        let event = TimelineEvent::new(
            "report",
            "filed",
            reader.operation.clone(),
            state.clock.now() as f64,
        )
        .station(state.station_id.clone())
        .mission(mission)
        .actor(reader.agent.clone())
        .detail("report", report.id.clone());
        if let Err(error) = state.intake.record_event(&event) {
            eprintln!("ai6: a filed report's event was not recorded: {error}");
        }
        Ok(json!({ "filed": true, "id": report.id, "scope": report.scope.to_string() }))
    }

    /// The Reports of `scope` (the Session's Operation when left out) or tied to `mission`, in
    /// a watch's order, each as far as the Session may see it, fenced.
    pub(super) fn list(
        &self,
        reader: &Reader,
        station: Option<&str>,
        scope: Option<&str>,
        mission: Option<String>,
        state_filter: Option<&str>,
    ) -> Result<String, String> {
        let mut filter = ReportFilter::all();
        if let Some(wanted) = state_filter {
            filter = filter.in_state(wanted.parse::<ReportState>()?);
        }
        let scope = match (scope, &mission) {
            (Some(scope), _) => Some(scope.parse::<ReportScope>()?),
            (None, Some(_)) => None,
            (None, None) => Some(ReportScope::Operation(reader.operation.clone())),
        };
        let answer = self.with_reader(reader, station, |state, caller| {
            if let Some(scope) = &scope {
                if !caller.reads_scope(scope) {
                    return refused(format!("{scope} is not yours to read"));
                }
                filter = filter.clone().in_scope(scope.clone());
            }
            if let Some(mission) = mission {
                // A Mission the Session may not read answers as one with no Reports.
                let own = caller.reader.mission.as_deref() == Some(mission.as_str());
                let here = state.missions.lock().ok().and_then(|missions| {
                    missions
                        .get(&mission)
                        .and_then(|found| found.operation_id.clone())
                });
                let readable = own
                    || here
                        .or_else(|| {
                            state
                                .intake
                                .filed_brief(&mission)
                                .ok()
                                .flatten()
                                .and_then(|filed| filed.brief.operation)
                        })
                        .is_some_and(|operation| {
                            caller.reads_scope(&ReportScope::Operation(operation))
                        });
                if !readable {
                    return json!({ "reports": [] });
                }
                filter = filter.clone().about_mission(mission);
            }
            let filter = filter
                .clone()
                .ordered_by(caller.settings.priorities.clone());
            let reports = match state.intake.reports(&filter) {
                Ok(reports) => reports,
                Err(_) => return refused("the reports could not be read"),
            };
            let shown: Vec<Value> = reports
                .iter()
                .map(|report| self.shown(state, caller, report))
                .collect();
            json!({ "reports": shown })
        })?;
        Ok(match answer.get("status") {
            Some(_) => answer.to_string(),
            None => fenced("reports", &answer),
        })
    }

    /// One Report as far as the Session may see it, fenced; refused, with its markings, to a
    /// Session that may see only those.
    pub(super) fn read(
        &self,
        reader: &Reader,
        station: Option<&str>,
        id: &str,
    ) -> Result<String, String> {
        let answer = self.with_reader(reader, station, |state, caller| {
            let report = match state.intake.report(id) {
                Ok(Some(report)) => report,
                Ok(None) | Err(_) => return refused("no such report"),
            };
            // A Report of a scope the Session may not read answers as one that does not exist.
            if !caller.reads_scope(&report.scope) {
                return refused("no such report");
            }
            let shown = self.shown(state, caller, &report);
            if shown["sight"] == "markings" {
                return json!({
                    "status": "refused",
                    "reason": "you are not read in to this report, or not cleared for it",
                    "report": shown,
                });
            }
            shown
        })?;
        Ok(match answer.get("status") {
            Some(_) => answer.to_string(),
            None => fenced("read_report", &answer),
        })
    }

    /// `report` as `caller` may see it; content from another Operation marks the Session.
    fn shown(&self, state: &DaemonState, caller: &SessionReader<'_>, report: &Report) -> Value {
        let rules = ReportRules::new(&caller.settings, &caller.clearances);
        let seen = sight(caller, report, &rules);
        if seen != ReportSight::Markings
            && let Some(operation) = report.scope.operation()
            && operation != caller.reader.operation
        {
            let mark = product_source(
                operation,
                state.intake.limits().sessions.exposure_source_bytes,
            );
            caller.reader.exposure.mark(&mark, state.clock.now() as f64);
        }
        let source = if seen == ReportSight::Content {
            state.intake.report_source(&report.filer.source_key()).ok()
        } else {
            None
        };
        report_json(report, seen, source.as_ref())
    }
}

#[cfg(test)]
mod tests;
