//! What a charge does about what it finds: the tools of its scope for fixing its area.
//!
//! A charge owns its area and may act in all of it: M's charge in all of AI6, a Station's charge
//! in its Station, an Operation's charge in its Operation. Every act here is audited before it is
//! taken, with the charge as actor, and a refused audit refuses the act; each also checks that the
//! charge still holds its lease. A Mission outside the charge's scope answers as one that does not
//! exist. A Station's charge reads state, never an Operation's text (that crosses an Operation),
//! and decides no Operation's outcome: a Mission waiting to be verified is its 00's.

use serde_json::{Value, json};

use super::super::{DaemonState, unix_now};
use super::area::{self, MissionView};
use crate::directorates::m::charges::ChargeScope;
use crate::directorates::m::memories::fenced;
use crate::directorates::m::reports::ReportFiling;
use crate::directorates::registry::reports::{ReportFiler, ReportScope};
use crate::directorates::registry::runs::{RunResult, RunStatus};
use crate::directorates::registry::store::{AuditRecord, LeaseRecovery};
use crate::directorates::registry::timeline::TimelineEvent;

/// The tools a charge fixes its area with.
#[derive(Clone)]
pub(in crate::station::server) struct AreaDesk {
    state: DaemonState,
    scope: ChargeScope,
    agent: String,
}

/// The reason a refused Mission answers with, whether it exists or not.
fn not_yours(scope: &ChargeScope) -> String {
    match scope {
        ChargeScope::M => "no such Mission".to_owned(),
        ChargeScope::Operation(_) => "that is not a Mission of your Operation".to_owned(),
        ChargeScope::Station(_) => "that is not a Mission of your Station".to_owned(),
    }
}

impl AreaDesk {
    pub(in crate::station::server) fn new(
        state: DaemonState,
        scope: ChargeScope,
        agent: impl Into<String>,
    ) -> Self {
        Self {
            state,
            scope,
            agent: agent.into(),
        }
    }

    /// Whether the charge is a Station's.
    pub(in crate::station::server) fn is_station(&self) -> bool {
        self.scope.station().is_some()
    }

    /// The charge as the actor of an audit record and a timeline event.
    pub(in crate::station::server) fn actor(&self) -> String {
        format!("charge:{}", self.scope)
    }

    /// Whether this Station still runs the charge under the lease it holds.
    fn admit(&self) -> Result<(), String> {
        let scope = self.scope.to_string();
        let live = self
            .state
            .charges
            .get(&scope)
            .filter(|live| !live.closing.load(std::sync::atomic::Ordering::Acquire))
            .ok_or("this Station no longer runs the charge")?;
        let record = self
            .state
            .intake
            .charge(&scope)
            .map_err(|error| error.to_string())?
            .ok_or("the charge has no record")?;
        if record.station != self.state.station_id
            || record.fence != live.fence.load(std::sync::atomic::Ordering::Acquire)
            || record.lease_expires < unix_now()
        {
            return Err("the charge no longer holds its lease".to_owned());
        }
        Ok(())
    }

    /// Writes the audit record of an act the charge is about to take; no record, no act.
    fn audit(&self, action: &str, operation: Option<&str>, target: &str) -> Result<(), String> {
        let record = AuditRecord {
            id: format!("{action}-{}", uuid::Uuid::new_v4().simple()),
            actor: self.actor(),
            action: action.to_owned(),
            operation: operation.unwrap_or_default().to_owned(),
            resource: Some(format!("station:{}", self.state.station_id)),
            target: target.chars().take(500).collect(),
            at: unix_now(),
        };
        self.state.intake.record_audit(&record).map_err(|error| {
            format!("the act was not taken, since its audit record could not be written: {error}")
        })
    }

    fn allows(&self, view: &MissionView) -> Result<(), String> {
        let ours = match &self.scope {
            ChargeScope::M => true,
            ChargeScope::Operation(own) => view.operation.as_deref() == Some(own.as_str()),
            ChargeScope::Station(own) => view.station.as_deref() == Some(own.as_str()),
        };
        ours.then_some(()).ok_or_else(|| not_yours(&self.scope))
    }

    /// The Mission `name` names (a UID or a handle), as the Registry has it, when it is the
    /// charge's. Blocks.
    fn find(&self, name: &str) -> Result<MissionView, String> {
        let intake = &self.state.intake;
        let id = match intake
            .resolve_mission(name)
            .map_err(|error| error.to_string())?
        {
            Some(found) => found.mission_id,
            None => name.to_owned(),
        };
        let now = unix_now();
        let queue = intake.list_new().map_err(|error| error.to_string())?;
        let waiting = queue.iter().find(|brief| brief.key() == id);
        let view = match intake.latest_run(&id).map_err(|error| error.to_string())? {
            Some(run) => area::run_view(&run, waiting.is_some(), now).unwrap_or_else(|| {
                let mut view = MissionView {
                    mission: run.mission_id.clone(),
                    handle: None,
                    operation: run.operation.clone(),
                    station: Some(run.station.clone()),
                    agent: None,
                    state: run
                        .result
                        .as_ref()
                        .map_or("failed", |result| result.state.as_str())
                        .to_owned(),
                    number: run.number,
                    run: Some(run.id.clone()),
                    since: run.closed_at,
                    unheld: true,
                    decided: run.verdict.is_some(),
                    reason: None,
                };
                view.handle = run.handle.as_deref().map(|handle| {
                    handle
                        .rsplit_once('.')
                        .map_or(handle, |(m, _)| m)
                        .to_owned()
                });
                view
            }),
            None => {
                let brief = waiting.ok_or_else(|| not_yours(&self.scope))?;
                MissionView {
                    mission: id.clone(),
                    handle: brief.handle.clone(),
                    operation: brief.operation.clone(),
                    station: None,
                    agent: brief.agent.clone(),
                    state: "queued".to_owned(),
                    number: 0,
                    run: None,
                    since: None,
                    unheld: true,
                    decided: false,
                    reason: None,
                }
            }
        };
        self.allows(&view)?;
        Ok(view)
    }

    /// Refuses a Mission whose outcome is being decided: that is the Operation's 00's to verify.
    fn not_an_outcome(&self, view: &MissionView) -> Result<(), String> {
        if !matches!(view.state.as_str(), "verifying" | "held") {
            return Ok(());
        }
        Err(if self.is_station() {
            "a Mission waiting for its outcome is its Operation's to decide, not the Station's; \
             notify_person says so"
                .to_owned()
        } else {
            "a Mission waiting for its outcome is decided with verify_mission or redispatch, not \
             as stuck work"
                .to_owned()
        })
    }

    fn timeline(&self, action: &str, view: &MissionView, detail: &str) {
        let Some(operation) = view.operation.as_deref() else {
            return;
        };
        let event = TimelineEvent::new("charge", action, operation, unix_now())
            .mission(view.mission.clone())
            .actor(self.actor())
            .detail("reason", detail.chars().take(500).collect::<String>());
        let _ = self.state.intake.record_event(&event);
    }

    /// The Missions of the area by state, what is stuck, and for M the charges. Blocks.
    pub(in crate::station::server) fn board(
        &self,
        only_state: Option<&str>,
        operation: Option<&str>,
    ) -> Result<String, String> {
        let now = unix_now();
        let views = area::missions(&self.state, &self.scope, now).map_err(|e| e.to_string())?;
        let mut by_state: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
        for view in views
            .iter()
            .filter(|view| only_state.is_none_or(|wanted| view.state == wanted))
            .filter(|view| operation.is_none_or(|wanted| view.operation.as_deref() == Some(wanted)))
        {
            by_state.entry(view.state.clone()).or_default().push(json!({
                "mission": view.mission,
                "handle": view.handle,
                "operation": view.operation,
                "station": view.station,
                "agent": view.agent,
                "run": view.number,
                "since": view.since,
                "held_by_no_run": view.unheld,
                "decided": view.decided,
                "reason": view.reason,
            }));
        }
        let mut board = json!({ "scope": self.scope.to_string(), "missions_by_state": by_state });
        if let Some(charge) = self.state.roster.charges().get(&self.scope.to_string()) {
            let mut first_seen = Default::default();
            let survey = area::survey(&self.state, charge, &mut first_seen, now);
            board["stuck"] = Value::Array(
                survey
                    .stuck
                    .iter()
                    .map(|stuck| area::stuck_detail(stuck, now))
                    .collect(),
            );
            board["open_reports"] = json!(survey.open_reports);
        }
        if self.scope == ChargeScope::M {
            let charges = self.state.intake.charges().map_err(|e| e.to_string())?;
            board["charges"] = Value::Array(
                charges
                    .iter()
                    .map(|record| {
                        json!({
                            "scope": record.scope,
                            "station": record.station,
                            "lease_lapsed": record.lease_expires < now,
                            "last_wake": record.last_wake,
                        })
                    })
                    .collect(),
            );
        }
        Ok(fenced("board", &board))
    }

    /// One Mission as a Station's charge reads it: where it stands and each Run's state, never
    /// the Operation's text. Blocks.
    pub(in crate::station::server) fn state_of(&self, name: &str) -> Result<String, String> {
        let view = self.find(name)?;
        let mut runs: Vec<_> = self
            .state
            .intake
            .runs()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|run| run.mission_id == view.mission)
            .collect();
        runs.sort_by_key(|run| run.number);
        let runs: Vec<Value> = runs
            .iter()
            .map(|run| {
                json!({
                    "number": run.number,
                    "run": run.id,
                    "station": run.station,
                    "status": run.status,
                    "state": run.result.as_ref().map(|result| result.state.as_str()),
                    "reason": run.result.as_ref().and_then(|result| result.reason.as_deref()),
                    "lost": run.lost,
                    "decided": run.verdict.as_ref().map(|verdict| verdict.verdict.as_str()),
                })
            })
            .collect();
        Ok(fenced(
            "mission_detail",
            &json!({
                "mission": view.mission,
                "handle": view.handle,
                "operation": view.operation,
                "station": view.station,
                "state": view.state,
                "since": view.since,
                "held_by_no_run": view.unheld,
                "runs": runs,
                "text": "the Operation's brief and report are outside a Station's area",
            }),
        ))
    }

    /// Files the Mission as `failed` or `stalled` for `reason`, when nothing holds it. Blocks.
    pub(in crate::station::server) fn settle(
        &self,
        name: &str,
        outcome: &str,
        reason: &str,
    ) -> Result<String, String> {
        self.admit()?;
        if !matches!(outcome, "failed" | "stalled") {
            return Err("a stuck Mission is settled as failed or stalled".to_owned());
        }
        if reason.trim().is_empty() {
            return Err("settling needs its reason".to_owned());
        }
        let view = self.find(name)?;
        self.not_an_outcome(&view)?;
        if matches!(
            view.state.as_str(),
            "done" | "failed" | "abandoned" | "blown"
        ) {
            return Err(format!("it is already over ({})", view.state));
        }
        if !view.unheld {
            return Err("a Run holds it, so it is not stuck".to_owned());
        }
        if view.number == 0 {
            return Err("it has no Run: cancel_mission withdraws a queued Mission".to_owned());
        }
        let intake = &self.state.intake;
        let run = intake
            .latest_run(&view.mission)
            .map_err(|error| error.to_string())?
            .ok_or("it has no Run")?;
        let now = unix_now();
        if run.status == RunStatus::Active && run.leased_at(now) {
            return Err("its Run's lease is live: a Station still holds it".to_owned());
        }
        self.audit(
            "mission.settle",
            view.operation.as_deref(),
            &format!("{} {outcome}", view.mission),
        )?;
        if run.status == RunStatus::Active || run.status == RunStatus::Parked {
            match intake.lose_run(&run.id, reason) {
                Ok(_) | Err(crate::directorates::registry::runs::RunError::Ended) => {}
                Err(error) => return Err(format!("its Run could not be given up: {error}")),
            }
        }
        let latest = RunResult::new(outcome).with_reason(Some(reason.to_owned()));
        let earlier =
            RunResult::new("abandoned").with_reason(Some(RunResult::SUPERSEDED.to_owned()));
        let settled = intake
            .settle_results(&view.mission, &latest, &earlier)
            .map_err(|error| error.to_string())?;
        let filed = if outcome == "failed" {
            self.file_failed(&view.mission, reason)?
        } else {
            "left as it is"
        };
        self.timeline("settled", &view, reason);
        Ok(json!({
            "settled": outcome,
            "mission": view.mission,
            "runs_settled": settled,
            "brief": filed,
        })
        .to_string())
    }

    /// Files the Mission's brief failed, whatever state the Registry holds it in.
    fn file_failed(&self, mission: &str, reason: &str) -> Result<&'static str, String> {
        let state = &self.state;
        let station = state.station_id.as_str();
        let outcome = json!({
            "state": "failed",
            "reason": reason,
            "settled_by": self.actor(),
        });
        let Some(filed) = state
            .intake
            .filed_brief(mission)
            .map_err(|error| error.to_string())?
        else {
            return Ok("it has none");
        };
        match filed.state.as_str() {
            "failed" => {
                state
                    .intake
                    .refile_lost(mission, &outcome, false)
                    .map_err(|error| error.to_string())?;
                Ok("already filed failed")
            }
            "new" => {
                let meta =
                    std::collections::BTreeMap::from([("mission_id".to_owned(), json!(mission))]);
                let lease = state
                    .intake
                    .claim_with_lease(Some(mission), station, state.mission_lease(), meta)
                    .map_err(|error| error.to_string())?
                    .ok_or("its brief was taken from the queue meanwhile")?;
                super::super::finish_intake_claim(state, &lease, &outcome, false)
                    .ok_or("its brief could not be filed")?;
                Ok("taken from the queue and filed failed")
            }
            "claimed" => {
                let lease = state
                    .intake
                    .lease(mission)
                    .map_err(|error| error.to_string())?
                    .ok_or("its brief has no lease")?;
                if lease.expires() >= unix_now() {
                    return Err(format!(
                        "Station {:?}'s lease on its brief has not lapsed; settle it once it has",
                        lease.station()
                    ));
                }
                match state
                    .intake
                    .recover(&lease, 0, &self.actor())
                    .map_err(|error| error.to_string())?
                {
                    Some(LeaseRecovery::Failed) => {
                        super::super::standing::recovered_failed(state, lease.key());
                        Ok("filed failed")
                    }
                    _ => Err("its brief could not be recovered".to_owned()),
                }
            }
            "done" => Err("its brief is filed done".to_owned()),
            other => Err(format!(
                "its brief is {other}, which settling does not touch"
            )),
        }
    }

    /// Queues the Mission's next run. A Mission this Station holds goes through the operator
    /// control; one whose Station is dark has its brief recovered and queued again, bounded by
    /// the run limit.
    pub(in crate::station::server) async fn retry(
        &self,
        name: &str,
        agent: Option<&str>,
    ) -> Result<String, String> {
        self.admit()?;
        let (desk, mission) = (self.clone(), name.to_owned());
        let view = tokio::task::spawn_blocking(move || desk.find(&mission))
            .await
            .map_err(|_| "the call failed".to_owned())??;
        self.not_an_outcome(&view)?;
        if view.state == "queued" {
            return Err("it is already queued".to_owned());
        }
        if let Some(agent) = agent {
            self.qualified(&view, agent)?;
        }
        if self.holds_here(&view) {
            return self
                .control(&view, ControlKind::Retry { agent }, "mission.retry")
                .await;
        }
        let desk = self.clone();
        let (view, agent) = (view, agent.map(str::to_owned));
        tokio::task::spawn_blocking(move || desk.retry_dark(&view, agent.as_deref()))
            .await
            .map_err(|_| "the call failed".to_owned())?
    }

    /// Whether `agent` may be given the Mission: on the roster for its Operation, and not
    /// quarantined there.
    fn qualified(&self, view: &MissionView, agent: &str) -> Result<(), String> {
        let operation = view.operation.as_deref();
        super::super::agents::effective(&self.state, operation, agent)
            .map_err(|error| error.to_string())?;
        let quarantined = self
            .state
            .intake
            .quarantines()
            .map_err(|error| error.to_string())?
            .iter()
            .any(|held| {
                held.agent == agent
                    && held
                        .operation
                        .as_deref()
                        .is_none_or(|held| Some(held) == operation)
            });
        if quarantined {
            return Err(format!("Agent {agent:?} is quarantined"));
        }
        Ok(())
    }

    /// Whether this Station's book holds the Mission.
    fn holds_here(&self, view: &MissionView) -> bool {
        self.state
            .missions
            .lock()
            .ok()
            .and_then(|missions| missions.get(&view.mission).map(|m| m.operation_id.clone()))
            .is_some_and(|operation| operation == view.operation)
    }

    fn retry_dark(&self, view: &MissionView, agent: Option<&str>) -> Result<String, String> {
        if agent.is_some() {
            return Err(
                "naming an Agent needs the Station that holds the Mission; wake_charge asks it"
                    .to_owned(),
            );
        }
        if !view.unheld {
            return Err("a Run holds it, so it is not stuck".to_owned());
        }
        let intake = &self.state.intake;
        let Some(lease) = intake
            .lease(&view.mission)
            .map_err(|error| error.to_string())?
        else {
            return Err(format!(
                "this Station does not hold it and its brief is not claimed; {}",
                view.station.as_deref().map_or_else(
                    || "wake_charge asks the Station's charge".to_owned(),
                    |station| format!("wake_charge station:{station} asks its charge")
                )
            ));
        };
        if lease.expires() >= unix_now() {
            return Err(format!(
                "Station {:?}'s lease on its brief has not lapsed yet",
                lease.station()
            ));
        }
        let run = intake
            .latest_run(&view.mission)
            .map_err(|error| error.to_string())?;
        self.audit(
            "mission.retry",
            view.operation.as_deref(),
            &format!("{} on the queue", view.mission),
        )?;
        if let Some(run) = run
            && run.status == RunStatus::Active
        {
            let _ = intake.lose_run(&run.id, "its Station stopped renewing it");
        }
        let limit = self.state.station.limits().scheduler.intake_runs;
        match intake
            .recover(&lease, limit, &self.actor())
            .map_err(|error| error.to_string())?
        {
            Some(LeaseRecovery::Requeued) => {
                self.timeline("retried", view, "queued again");
                Ok(
                    json!({ "retried": true, "mission": view.mission, "state": "queued" })
                        .to_string(),
                )
            }
            Some(LeaseRecovery::Failed) => {
                super::super::standing::recovered_failed(&self.state, lease.key());
                Err("its run limit is spent, so it was filed failed".to_owned())
            }
            _ => Err("its brief could not be recovered".to_owned()),
        }
    }

    /// Resumes the Run of a Mission this Station holds, optionally telling it `message`.
    pub(in crate::station::server) async fn resume(
        &self,
        name: &str,
        message: Option<&str>,
    ) -> Result<String, String> {
        self.admit()?;
        let (desk, mission) = (self.clone(), name.to_owned());
        let view = tokio::task::spawn_blocking(move || desk.find(&mission))
            .await
            .map_err(|_| "the call failed".to_owned())??;
        self.not_an_outcome(&view)?;
        if !self.holds_here(&view) {
            return Err(format!(
                "this Station does not hold it{}",
                view.station
                    .as_deref()
                    .map_or_else(String::new, |station| format!(
                        "; wake_charge station:{station} asks the Station that does"
                    ))
            ));
        }
        self.control(&view, ControlKind::Resume { message }, "mission.resume")
            .await
    }

    /// Withdraws the Mission: its Run stopped when this Station holds it, otherwise its brief
    /// asked to be cancelled.
    pub(in crate::station::server) async fn cancel(
        &self,
        name: &str,
        reason: &str,
    ) -> Result<String, String> {
        self.admit()?;
        if reason.trim().is_empty() {
            return Err("cancelling needs its reason".to_owned());
        }
        let (desk, mission) = (self.clone(), name.to_owned());
        let view = tokio::task::spawn_blocking(move || desk.find(&mission))
            .await
            .map_err(|_| "the call failed".to_owned())??;
        self.not_an_outcome(&view)?;
        if self.holds_here(&view) {
            return self
                .control(&view, ControlKind::Cancel, "mission.cancel")
                .await;
        }
        let desk = self.clone();
        let reason = reason.to_owned();
        tokio::task::spawn_blocking(move || {
            desk.audit(
                "mission.cancel",
                view.operation.as_deref(),
                &format!("{} {reason}", view.mission),
            )?;
            let asked = desk
                .state
                .intake
                .request_cancel(&view.mission, unix_now())
                .map_err(|error| error.to_string())?;
            desk.timeline("cancel-asked", &view, &reason);
            Ok(json!({ "cancel_asked": asked, "mission": view.mission }).to_string())
        })
        .await
        .map_err(|_| "the call failed".to_owned())?
    }

    #[cfg(feature = "publisher")]
    async fn control(
        &self,
        view: &MissionView,
        kind: ControlKind<'_>,
        action: &str,
    ) -> Result<String, String> {
        use ai6_diptel::{ControlAction, ControlId, MissionControl, MissionId, OperationId};
        let operation = view
            .operation
            .clone()
            .ok_or("the Mission belongs to no Operation")?;
        fn invalid<E>(_: E) -> String {
            "the control was not valid".to_owned()
        }
        let (verb, control) = {
            let base = MissionControl::new(
                ControlId::new(uuid::Uuid::new_v4().to_string()).map_err(invalid)?,
                OperationId::new(operation.clone()).map_err(invalid)?,
                MissionId::new(view.mission.clone()).map_err(invalid)?,
                match kind {
                    ControlKind::Retry { .. } => ControlAction::Retry,
                    ControlKind::Resume { .. } => ControlAction::Resume,
                    ControlKind::Cancel => ControlAction::Cancel,
                },
            );
            match kind {
                ControlKind::Retry { agent: Some(agent) } => {
                    ("retried", base.with_agent(agent).map_err(invalid)?)
                }
                ControlKind::Retry { agent: None } => ("retried", base),
                ControlKind::Resume {
                    message: Some(message),
                } => ("resumed", base.with_message(message).map_err(invalid)?),
                ControlKind::Resume { message: None } => ("resumed", base),
                ControlKind::Cancel => ("cancelled", base),
            }
        };
        let desk = self.clone();
        let (action, target) = (
            action.to_owned(),
            format!("{} on this Station", view.mission),
        );
        tokio::task::spawn_blocking(move || desk.audit(&action, Some(&operation), &target))
            .await
            .map_err(|_| "the call failed".to_owned())??;
        let outcome = super::super::controls::apply(&self.state, &control).await;
        self.timeline(verb, view, outcome.reason.as_deref().unwrap_or("applied"));
        if outcome.outcome == "applied" {
            Ok(json!({ verb: true, "mission": view.mission, "state": outcome.state }).to_string())
        } else {
            Err(outcome
                .reason
                .unwrap_or_else(|| "the Station refused the control".to_owned()))
        }
    }

    #[cfg(not(feature = "publisher"))]
    async fn control(
        &self,
        _view: &MissionView,
        _kind: ControlKind<'_>,
        _action: &str,
    ) -> Result<String, String> {
        Err("operator controls need a Station built with the publisher".to_owned())
    }

    /// Asks another charge to look: the note is the reason, and the charge reads it as a person's
    /// word, untrusted. M may ask any charge; a Station's charge the Operations' charges for the
    /// Missions it holds; an Operation's charge the Station charges that hold its Missions.
    pub(in crate::station::server) fn wake(
        &self,
        target: &str,
        note: &str,
    ) -> Result<String, String> {
        self.admit()?;
        let target: ChargeScope = target.parse()?;
        let name = target.to_string();
        if !self.state.roster.charges().contains_key(&name) {
            return Err(format!("{name} is not a charge on the roster"));
        }
        let allowed = match &self.scope {
            ChargeScope::M => true,
            own => {
                let views = area::missions(&self.state, own, unix_now())
                    .map_err(|error| error.to_string())?;
                views.iter().any(|view| match (&target, own) {
                    (ChargeScope::Operation(op), ChargeScope::Station(_)) => {
                        view.operation.as_deref() == Some(op.as_str())
                    }
                    (ChargeScope::Station(st), ChargeScope::Operation(_)) => {
                        view.station.as_deref() == Some(st.as_str())
                    }
                    _ => false,
                })
            }
        };
        if !allowed || target == self.scope {
            return Err(format!("you may not wake {name}"));
        }
        self.audit("charge.wake", None, &name)?;
        self.state
            .intake
            .request_charge_wake(&name, &self.actor(), note)
            .map_err(|error| error.to_string())?;
        Ok(json!({ "woken": name }).to_string())
    }

    /// Files a Report in the charge's own scope for a person, who sees it in the attention list:
    /// what was found, and what the charge did or could not do. Blocks.
    pub(in crate::station::server) fn notify(
        &self,
        filing: ReportFiling,
        run: Option<String>,
    ) -> Result<String, String> {
        self.admit()?;
        let state = &self.state;
        let scope = self.scope.to_string();
        let report_scope = match &self.scope {
            ChargeScope::M => ReportScope::M,
            ChargeScope::Operation(operation) => ReportScope::Operation(operation.clone()),
            ChargeScope::Station(station) => ReportScope::Station(station.clone()),
        };
        let report = filing.into_report(
            report_scope,
            ReportFiler::Charge {
                agent: self.agent.clone(),
                run: run.unwrap_or_else(|| scope.clone()),
                scope,
            },
            [],
            state.clock.now() as f64,
            &state.intake.limits().reports,
            &state
                .config
                .report_settings()
                .unwrap_or_else(|_| state.roster.settings().reports.clone()),
        )?;
        self.audit("charge.notified", None, &report.id)?;
        state
            .intake
            .file_report(&report)
            .map_err(|error| format!("the notice was not filed: {error}"))?;
        Ok(
            json!({ "filed": true, "id": report.id, "scope": report.scope.to_string() })
                .to_string(),
        )
    }
}

/// What a control asks of a Mission this Station holds.
enum ControlKind<'a> {
    Retry { agent: Option<&'a str> },
    Resume { message: Option<&'a str> },
    Cancel,
}
