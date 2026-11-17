//! The Station's side of a Run's Report tools.
//!
//! Every Agent's Run files Reports against its Mission's Operation, linked to that Mission,
//! and reads them under the same rule as a person: the Operation's read (its own Operation, or
//! another through a compartment granting `mission.read`, audited first), being read in to each
//! of a Report's compartments, and a clearance at or above its classification. The
//! Station's charge also reads its own Station's Reports. Filing creates no Mission, brief or
//! placement.
//!
//! Every call asks the configuration what is granted now, and works from the Run as the
//! Station started it. What it reads is fenced as data; a Report from another Operation marks the
//! Run exposed as that Operation's product.

use std::collections::BTreeMap;
use std::io;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};
use uuid::Uuid;

use super::compartments::Reader;
use super::{DaemonState, agents};
use crate::bounds::Limit;
use crate::directorates::m::compartments::{
    CompartmentAction, CompartmentResource, CompiledCompartments, FieldPrincipal,
};
use crate::directorates::m::knowledge::{
    self, GLOBAL_RESOURCE, KnowledgeCaller, KnowledgeChange, KnowledgeForgotten, KnowledgeOutcome,
    KnowledgeRight, bounded, fenced,
};
use crate::directorates::m::reports::{
    ReportFiling, ReportReader, ReportRules, ReportSettings, ReportSight, report_json, sight,
};
use crate::directorates::m::roster::clearance_rank;
use crate::directorates::medical::exposure::{product_source, report_source};
use crate::directorates::registry::assessments::{
    self, Assessment, Context, Judgement, Verdict, VerdictParts,
};
use crate::directorates::registry::knowledge::ReportQuery;
use crate::directorates::registry::reports::{
    Report, ReportClearance, ReportFiler, ReportFilter, ReportLink, ReportLinkKind, ReportScope,
    ReportState,
};
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::registry::timeline::TimelineEvent;

/// What an audit record's text may hold.
const AUDIT_BYTES: Limit = Limit::at(512);

/// What a call answers when who is asking cannot be settled.
const UNAVAILABLE: &str = "reports are unavailable";

/// A Run asking about Reports, as granted at this call.
struct RunReader<'a> {
    state: &'a DaemonState,
    reader: &'a Reader,
    principal: FieldPrincipal,
    /// The Station whose Reports the Station's charge reads.
    station: Option<&'a str>,
    /// What is granted now; `None` when it cannot be read, which grants nothing.
    compiled: Option<CompiledCompartments>,
    /// Each Operation crossed into in this call, so one is audited once.
    crossed: Mutex<BTreeMap<String, bool>>,
    /// The deployment's lists, as the provider holds them now.
    settings: ReportSettings,
    /// Every clearance, as the Registry holds them now.
    clearances: Vec<ReportClearance>,
    /// The Run, once the harness has reported it.
    run: Option<String>,
    /// M's charge, which decides what is proposed to the global scope.
    m: bool,
}

impl RunReader<'_> {
    fn crosses_into(&self, operation: &str, known: bool) -> bool {
        let mut crossed = self
            .crossed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = format!("{}{operation}", if known { "known:" } else { "" });
        if let Some(seen) = crossed.get(&key) {
            return *seen;
        }
        let action = if known {
            CompartmentAction::MemoryRead
        } else {
            CompartmentAction::MissionRead
        };
        let resource = CompartmentResource::Operation(operation.to_owned());
        let through = self
            .compiled
            .as_ref()
            .and_then(|compiled| compiled.field_allows(&self.principal, action, &resource));
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
        crossed.insert(key, allowed);
        allowed
    }
}

impl ReportReader for RunReader<'_> {
    fn reads_scope(&self, scope: &ReportScope) -> bool {
        match scope {
            ReportScope::Operation(operation) if *operation == self.reader.operation => true,
            ReportScope::Operation(operation) => self.crosses_into(operation, false),
            ReportScope::Station(station) => self.station == Some(station.as_str()),
            ReportScope::Global => true,
            ReportScope::M | ReportScope::Person(_) => false,
        }
    }

    fn reads_known_scope(&self, scope: &ReportScope) -> bool {
        match scope {
            ReportScope::Operation(operation) if *operation != self.reader.operation => {
                self.crosses_into(operation, true)
            }
            scope => self.reads_scope(scope),
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

    /// A charge, which has no Mission, decides what is proposed in its own Operation; M's
    /// decides what is proposed to the global scope.
    fn assesses(&self, scope: &ReportScope) -> bool {
        self.reader.mission.is_none()
            && if self.m {
                *scope == ReportScope::Global
            } else {
                matches!(scope, ReportScope::Operation(operation) if *operation == self.reader.operation)
            }
    }

    fn filed(&self, filer: &ReportFiler) -> bool {
        *filer == self.filer()
    }
}

impl KnowledgeCaller for RunReader<'_> {
    fn by(&self) -> String {
        format!("agent:{}", self.reader.agent)
    }

    fn filer(&self) -> ReportFiler {
        let agent = self.reader.agent.clone();
        match &self.reader.mission {
            Some(mission) => ReportFiler::Agent {
                agent,
                run: self.run.clone().unwrap_or_else(|| mission.clone()),
                mission: mission.clone(),
            },
            None => {
                let scope = format!("operation:{}", self.reader.operation);
                ReportFiler::Charge {
                    agent,
                    run: self.run.clone().unwrap_or_else(|| scope.clone()),
                    scope,
                }
            }
        }
    }

    fn run(&self) -> Option<String> {
        self.run.clone().or_else(|| self.reader.mission.clone())
    }

    fn writes_scope(&self, scope: &ReportScope) -> KnowledgeRight {
        match scope {
            ReportScope::Global => KnowledgeRight::Propose,
            ReportScope::Operation(operation) if *operation == self.reader.operation => {
                let allowed = self
                    .state
                    .roster
                    .operation(operation)
                    .ok()
                    .and_then(|operation| operation.memory_write.clone())
                    .is_some_and(|floor| {
                        let held = clearance_rank(&self.principal.clearance).unwrap_or(0);
                        clearance_rank(&floor).is_ok_and(|floor| held >= floor)
                    });
                if allowed {
                    KnowledgeRight::Write
                } else {
                    KnowledgeRight::Propose
                }
            }
            _ => KnowledgeRight::Refused,
        }
    }

    fn acts_in_person(&self) -> bool {
        false
    }

    fn exposures(&self) -> Vec<String> {
        self.reader
            .exposure
            .exposures()
            .into_iter()
            .map(|exposure| exposure.source)
            .collect()
    }

    fn saw_marked(&self, source: &str) {
        self.reader.exposure.mark(source, self.now());
    }

    fn mark_bytes(&self) -> Limit {
        self.state.intake.limits().runs.exposure_source_bytes
    }

    fn now(&self) -> f64 {
        self.state.clock.now() as f64
    }

    fn audit(&self, action: &str, scope: &ReportScope, target: &str) -> Result<(), String> {
        let (operation, resource) = match scope {
            ReportScope::Operation(operation) => (operation.clone(), None),
            _ => (
                self.reader.operation.clone(),
                Some(GLOBAL_RESOURCE.to_owned()),
            ),
        };
        self.state
            .intake
            .record_audit(&AuditRecord {
                id: format!("report-{}", Uuid::new_v4().simple()),
                actor: format!("agent:{}", self.reader.agent),
                action: action.to_owned(),
                operation,
                resource,
                target: bounded(target, AUDIT_BYTES),
                at: self.now(),
            })
            .map_err(|_| "it could not be audited, so it was not done".to_owned())
    }
}

/// How a refusal reads to the Run.
fn refused(reason: impl Into<String>) -> Value {
    json!({ "status": "refused", "reason": reason.into() })
}

/// What a store error says to the Run.
fn store_refusal(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::QuotaExceeded | io::ErrorKind::InvalidInput => error.to_string(),
        io::ErrorKind::InvalidData => "the Registry holds as many reports as it may".to_owned(),
        _ => "the report could not be stored".to_owned(),
    }
}

/// What a charge says when it assesses a Report.
#[derive(Debug, Clone, Default)]
pub(super) struct AssessCall {
    pub report: String,
    pub assessment: String,
    pub reason: String,
    pub of: Option<String>,
    pub mission: Option<String>,
    pub question: Option<String>,
    pub via: Option<String>,
    pub ask: Option<String>,
    pub condition: Option<String>,
    pub wait_seconds: Option<u64>,
    pub credibility: Option<u8>,
    pub priority: Option<String>,
    pub likelihood: Option<String>,
    pub confidence: Option<String>,
}

/// What the assessor is told comes next.
fn assessed_note(report: &Report) -> &'static str {
    let verdict = report
        .assessment
        .as_ref()
        .map(|assessment| assessment.verdict.as_str());
    let tied = report
        .links
        .iter()
        .any(|link| link.kind == ReportLinkKind::Mission);
    match verdict {
        Some("mission") if report.item.is_some() && !tied => {
            "the Station that polls the Source queues the brief on its next pass; vetting and \
             placement apply as to any brief"
        }
        Some("mission") => "it closes when its Mission settles",
        Some("decision") => "it closes when the green ink request is signed",
        Some("awaiting" | "later") => "it comes back to you at its time, or when you reopen it",
        _ => "recorded on the report and audited",
    }
}

/// A Run's way to its Reports.
#[derive(Clone)]
pub(super) struct ReportDesk {
    state: DaemonState,
    /// M's charge: it decides what is proposed to the global scope, and has no Operation.
    m: bool,
}

impl ReportDesk {
    pub(super) fn new(state: DaemonState) -> Self {
        Self { state, m: false }
    }

    /// The desk of M's charge.
    pub(super) fn for_m(state: DaemonState) -> Self {
        Self { state, m: true }
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
        run: Option<String>,
        work: impl FnOnce(&DaemonState, &RunReader<'_>) -> Value,
    ) -> Result<Value, String> {
        let state = self.state.current();
        let clearance = agents::effective(
            &state,
            (!self.m).then_some(reader.operation.as_str()),
            &reader.agent,
        )
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
        let caller = RunReader {
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
            run,
            m: self.m,
        };
        Ok(work(&state, &caller))
    }

    /// Files `filing` against the Run's Operation, linked to its Mission. `run` is the
    /// Run once the harness has reported it.
    pub(super) fn file(
        &self,
        reader: &Reader,
        run: Option<String>,
        filing: ReportFiling,
    ) -> Result<Value, String> {
        let mission = reader
            .mission
            .clone()
            .ok_or_else(|| "file_report needs the Run's Mission".to_owned())?;
        let state = &self.state;
        let filer = ReportFiler::Agent {
            agent: reader.agent.clone(),
            run: run.unwrap_or_else(|| mission.clone()),
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

    /// The Reports of `scope` (the Run's Operation when left out) or tied to `mission`, in
    /// a charge's order, each as far as the Run may see it, fenced.
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
        let answer = self.with_reader(reader, station, None, |state, caller| {
            if let Some(scope) = &scope {
                if !(caller.reads_scope(scope) || caller.reads_known_scope(scope)) {
                    return refused(format!("{scope} is not yours to read"));
                }
                filter = filter.clone().in_scope(scope.clone());
            }
            if let Some(mission) = mission {
                // A Mission the Run may not read answers as one with no Reports.
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
            // What is known and not readable is not listed at all, as it is not found by a search.
            let rules = ReportRules::new(&caller.settings, &caller.clearances);
            let shown: Vec<Value> = reports
                .iter()
                .filter(|report| {
                    !(report.is_known() && sight(caller, report, &rules) != ReportSight::Content)
                })
                .map(|report| self.shown(state, caller, report))
                .collect();
            json!({ "reports": shown })
        })?;
        Ok(match answer.get("status") {
            Some(_) => answer.to_string(),
            None => fenced("reports", &answer),
        })
    }

    /// One Report as far as the Run may see it, fenced; refused, with its markings, to a
    /// Run that may see only those.
    pub(super) fn read(
        &self,
        reader: &Reader,
        station: Option<&str>,
        id: &str,
    ) -> Result<String, String> {
        let answer = self.with_reader(reader, station, None, |state, caller| {
            let report = match crate::directorates::m::guides::find(id) {
                Some(shipped) => shipped.clone(),
                None => match state.intake.report(id) {
                    Ok(Some(report)) => report,
                    Ok(None) | Err(_) => return refused("no such report"),
                },
            };
            // A Report of a scope the Run may not read answers as one that does not exist.
            let reads = if report.is_known() {
                caller.reads_known_scope(&report.scope)
            } else {
                caller.reads_scope(&report.scope)
            };
            if !reads {
                return refused("no such report");
            }
            let shown = self.shown(state, caller, &report);
            // What is known and not readable is not there.
            if shown["sight"] == "markings" && report.is_known() {
                return refused("no such report");
            }
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

    /// Assesses the Report the call names, as `by` and under the Operation's own settings: the
    /// Report must be the Operation's and its content readable by the Run, as it is to read it.
    /// Answers what the assessment came to, or why it was refused.
    pub(super) fn assess(
        &self,
        reader: &Reader,
        call: &AssessCall,
        by: &str,
        request_ttl: Duration,
    ) -> Result<String, String> {
        let answer = self.with_reader(reader, None, None, |state, caller| {
            let report = match self.readable(state, caller, &call.report) {
                Ok(report) => report,
                Err(refusal) => return refusal,
            };
            let now = state.clock.now() as f64;
            let parts = VerdictParts {
                of: call.of.as_deref(),
                mission: call.mission.as_deref(),
                question: call.question.as_deref(),
                via: call.via.as_deref(),
                ask: call.ask.as_deref(),
                condition: call.condition.as_deref(),
                wait_seconds: call.wait_seconds,
            };
            let verdict = match Verdict::parse(&call.assessment, &parts, now) {
                Ok(verdict) => verdict,
                Err(reason) => return refused(reason),
            };
            let judgement =
                match Judgement::parse(call.likelihood.as_deref(), call.confidence.as_deref()) {
                    Ok(judgement) => judgement,
                    Err(reason) => return refused(reason),
                };
            let assessment = Assessment::new(&report.id, verdict, &call.reason, by, now)
                .believed(call.credibility)
                .prioritised(call.priority.as_deref())
                .judged(judgement);
            let reads = |scope: &ReportScope| caller.reads_scope(scope);
            let context = Context {
                settings: &caller.settings,
                reads: &reads,
                request_ttl,
                agent: &reader.agent,
            };
            match assessments::assess(state.intake.as_ref(), &assessment, &context) {
                Ok(assessed) => json!({
                    "assessed": assessed.assessment.as_ref().map(|each| each.verdict.as_str()),
                    "report": assessed.id,
                    "state": assessed.state.as_str(),
                    "judgement": assessed
                        .assessment
                        .as_ref()
                        .and_then(|each| each.judgement)
                        .map(|each| each.statement()),
                    "note": assessed_note(&assessed),
                }),
                Err(error) => refused(store_refusal(&error)),
            }
        })?;
        Ok(answer.to_string())
    }

    /// Searches what the Run may read for `words`, `topics`, a `title` and a `kind`, in `scope`
    /// (every scope it reads when left out): its ID, kind, title and one-line summary, best first,
    /// fenced. What it may not read is neither listed nor counted.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn search(
        &self,
        reader: &Reader,
        station: Option<&str>,
        scope: Option<&str>,
        words: Option<&str>,
        kind: Option<&str>,
        topics: &[String],
        title: Option<&str>,
        state_filter: Option<&str>,
    ) -> Result<String, String> {
        let mut query = ReportQuery::words(words.unwrap_or_default());
        if let Some(kind) = kind {
            query = query.of_kind(kind);
        }
        for topic in topics {
            query = query.on_topic(topic.clone());
        }
        if let Some(title) = title {
            query = query.titled(title);
        }
        if let Some(scope) = scope {
            query = query.in_scope(scope.parse::<ReportScope>()?);
        }
        if let Some(wanted) = state_filter {
            query = query.in_state(wanted.parse::<ReportState>()?);
        }
        let answer = self.with_reader(reader, station, None, |state, caller| {
            let rules = ReportRules::new(&caller.settings, &caller.clearances);
            let limits = state.intake.limits().reports.clone();
            match knowledge::search_reports(state.intake.as_ref(), caller, &rules, &query, &limits)
            {
                Ok(found) => json!({
                    "reports": found.hits.iter().map(knowledge::brief_json).collect::<Vec<_>>(),
                    "total": found.total,
                }),
                Err(refusal) => refused(refusal.to_string()),
            }
        })?;
        Ok(match answer.get("status") {
            Some(_) => answer.to_string(),
            None => fenced("reports", &answer),
        })
    }

    /// Writes a Report of a known kind as the Run: kept at once where its clearance writes the
    /// scope, otherwise filed `open` for a verdict.
    pub(super) fn write_known(
        &self,
        reader: &Reader,
        run: Option<String>,
        change: KnowledgeChange,
    ) -> Result<Value, String> {
        self.with_reader(reader, None, run, |state, caller| {
            let rules = ReportRules::new(&caller.settings, &caller.clearances);
            match knowledge::write_report(state.intake.as_ref(), caller, &rules, change) {
                Ok(KnowledgeOutcome::Kept(done)) => json!({
                    "filed": true,
                    "id": done.report.id,
                    "scope": done.report.scope.to_string(),
                    "state": done.report.state.as_str(),
                    "revision": done.report.revision,
                }),
                Ok(KnowledgeOutcome::Proposed(proposal)) => json!({
                    "filed": true,
                    "id": proposal.id,
                    "scope": proposal.scope.to_string(),
                    "state": proposal.state.as_str(),
                    "note": "it waits for a verdict from your Operation's charge or a person",
                }),
                Err(refusal) => refused(refusal.to_string()),
            }
        })
    }

    /// Forgets the memory `id`, or retires the tradecraft, rule or contact, as the Run: at once
    /// where its clearance writes the scope, otherwise a proposal that waits for a verdict.
    pub(super) fn forget(
        &self,
        reader: &Reader,
        run: Option<String>,
        id: &str,
        revision: Option<u64>,
    ) -> Result<Value, String> {
        self.with_reader(reader, None, run, |state, caller| {
            let rules = ReportRules::new(&caller.settings, &caller.clearances);
            match knowledge::forget_report(state.intake.as_ref(), caller, &rules, id, revision) {
                Ok(KnowledgeForgotten::Forgotten(gone)) => {
                    json!({ "status": "forgotten", "id": gone.id })
                }
                Ok(KnowledgeForgotten::Retired(kept)) => {
                    json!({ "status": "retired", "id": kept.id, "state": kept.state.as_str() })
                }
                Ok(KnowledgeForgotten::Proposed(proposal)) => json!({
                    "status": "proposed",
                    "id": proposal.id,
                    "note": "it waits for a verdict from your Operation's charge or a person",
                }),
                Err(refusal) => refused(refusal.to_string()),
            }
        })
    }

    /// Opens an assessed or closed Report of the Run's Operation again, for `why`: the answer to
    /// what it awaited, or the condition it waited for.
    pub(super) fn reopen(
        &self,
        reader: &Reader,
        report: &str,
        why: &str,
        by: &str,
    ) -> Result<String, String> {
        let answer = self.with_reader(reader, None, None, |state, caller| {
            let report = match self.readable(state, caller, report) {
                Ok(report) => report,
                Err(refusal) => return refusal,
            };
            match assessments::reopen(
                state.intake.as_ref(),
                &report.id,
                by,
                why,
                state.clock.now() as f64,
            ) {
                Ok(opened) => json!({ "reopened": opened.id, "state": opened.state.as_str() }),
                Err(error) => refused(store_refusal(&error)),
            }
        })?;
        Ok(answer.to_string())
    }

    /// The Report `id`, when it is the Run's Operation's and the Run reads its content.
    fn readable(
        &self,
        state: &DaemonState,
        caller: &RunReader<'_>,
        id: &str,
    ) -> Result<Report, Value> {
        let report = match state.intake.report(id) {
            Ok(Some(report)) => report,
            Ok(None) | Err(_) => return Err(refused("no such report")),
        };
        if self.m {
            // M's charge decides only what is proposed to the global scope.
            if report.scope != ReportScope::Global || !report.is_proposal() {
                return Err(refused("no such report"));
            }
        } else if report.scope != ReportScope::Operation(caller.reader.operation.clone()) {
            return Err(refused("no such report"));
        }
        let rules = ReportRules::new(&caller.settings, &caller.clearances);
        if sight(caller, &report, &rules) != ReportSight::Content {
            return Err(refused(
                "you are not read in to this report, or not cleared for it",
            ));
        }
        Ok(report)
    }

    /// `report` as `caller` may see it; content from another Operation marks the Run.
    fn shown(&self, state: &DaemonState, caller: &RunReader<'_>, report: &Report) -> Value {
        let rules = ReportRules::new(&caller.settings, &caller.clearances);
        let seen = sight(caller, report, &rules);
        if seen != ReportSight::Markings
            && let Some(operation) = report.scope.operation()
            && operation != caller.reader.operation
        {
            let mark = product_source(operation, state.intake.limits().runs.exposure_source_bytes);
            caller.reader.exposure.mark(&mark, state.clock.now() as f64);
        }
        // What an exposed Run wrote exposes whoever reads it.
        if seen == ReportSight::Content && !report.exposures.is_empty() {
            let mark = report_source(&report.id, state.intake.limits().runs.exposure_source_bytes);
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
