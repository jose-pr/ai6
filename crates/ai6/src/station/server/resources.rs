//! What an Operation's 00 asks of a Station, what the Station's charge answers, and what M's charge
//! flags.
//!
//! The Station is responsible for running and for providing: the 00 Agent in charge of an
//! Operation asks the Station for an Agent, a Theater, an account or capacity
//! (`request_resources`), and the Station's charge provides it or answers why not: busy, not
//! fitted, or outside the Operation's allowance. It never judges the Operation's work.
//!
//! A request is a control on the Station's charge, and its answer a control on the Operation's
//! charge: both are Registry records that wake the charge they are for, so a Station and an
//! Operation's charge on different Stations still reach each other. [`check`] is Station code,
//! not the charge's word: what the Station can provide is read from placement, the Operation's
//! allowance and the account pools, and an answer that promises more than that is turned into
//! a refusal with the reason.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{DaemonState, StationRole};
use crate::directorates::m::charges::ChargeScope;
use crate::directorates::m::placement::PlacementRequest;
use crate::directorates::registry::charges::ChargeControl;
#[cfg(feature = "cables-mcp")]
use crate::directorates::registry::charges::ChargeError;

/// What a request may ask for.
pub(super) const KINDS: [&str; 4] = ["agent", "theater", "account", "capacity"];

/// Who a request's control is from, followed by the Operation: only a request made through
/// `request_resources` is read as one.
const REQUEST_BY: &str = "resources:operation:";
/// Who an answer's control is from, followed by the Station.
const ANSWER_BY: &str = "resources:station:";

/// An Operation's request of a Station.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    /// The request's ID, which the answer repeats.
    pub(super) id: String,
    /// `agent`, `theater`, `account` or `capacity`.
    pub(super) kind: String,
    /// The Agent, Theater or pool asked for, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    /// Why it is wanted, in the asker's words. Untrusted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) detail: Option<String>,
    /// The Operation that asks.
    pub(super) operation: String,
    /// The Agent in charge of the Operation that asks.
    pub(super) agent: String,
    /// The Station asked.
    pub(super) station: String,
}

/// A Station's answer to a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Answer {
    /// The request it answers.
    pub(super) request: String,
    /// The Station that answers.
    pub(super) station: String,
    /// What was asked: `agent`, `theater`, `account` or `capacity`.
    pub(super) kind: String,
    /// `provided` or `refused`.
    pub(super) answer: String,
    /// What the Station provides, when it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) provided: Option<Value>,
    /// Why not, when it does not. For a refusal the charge wrote, its words; otherwise the
    /// Station's own finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<String>,
}

/// The note of a control that carries `request` or an answer.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Note {
    ResourceRequest(Request),
    ResourceAnswer(Answer),
}

/// What the Station can provide an Operation for a request of `kind` (naming `name`), or why it
/// cannot: not an execution Station, outside the Operation's allowance, busy, or not fitted.
/// Blocks on the Registry.
pub(super) fn check(
    state: &DaemonState,
    operation: &str,
    kind: &str,
    name: Option<&str>,
) -> Result<Value, String> {
    let roster = &state.roster;
    let found = roster
        .operation(operation)
        .map_err(|_| format!("Operation {operation:?} is not on the roster"))?;
    if !state.station.roles.contains(&StationRole::Execution) {
        return Err(
            "not fitted: this Station has no execution role, so it runs nothing".to_owned(),
        );
    }
    if !(found.stations.is_empty() || found.stations.contains(&state.station_id)) {
        return Err(
            "outside the Operation's allowance: its stations list does not name this Station"
                .to_owned(),
        );
    }
    if let Some((key, value)) = found
        .station_labels
        .iter()
        .find(|(key, value)| state.station.labels.get(*key) != Some(*value))
    {
        return Err(format!(
            "outside the Operation's allowance: it requires this Station to carry the label \
             {key}={value}"
        ));
    }
    match kind {
        "account" => {
            let agent = name.ok_or("an account request names the Agent that needs it")?;
            super::accounts::answer_for(state, operation, agent)
        }
        "agent" | "theater" | "capacity" => place(state, operation, kind, name),
        other => Err(format!("{other:?} is not a request kind: use {KINDS:?}")),
    }
}

/// An Agent, a Theater or a place, from what placement finds here.
fn place(
    state: &DaemonState,
    operation: &str,
    kind: &str,
    name: Option<&str>,
) -> Result<Value, String> {
    let request = PlacementRequest {
        stations: vec![state.station_id.clone()],
        agent: (kind == "agent").then(|| name.map(str::to_owned)).flatten(),
        role: None,
        tags: Vec::new(),
        deciding_station: Some(state.station_id.clone()),
    };
    let leases = state.intake.leases().unwrap_or_default();
    let (placed, stations) = super::placement_of(state, operation, &request, &leases)?;
    let fits: Vec<_> = placed
        .choices
        .iter()
        .filter(|choice| {
            choice.station == state.station_id
                && super::has_assignment_capacity(state, operation, &choice.agent)
        })
        .collect();
    let Some(first) = fits.first() else {
        return Err(super::unplaced_reason(&placed, &request, &stations));
    };
    match (kind, name) {
        ("theater", Some(theater)) => {
            let choice = fits
                .iter()
                .find(|choice| choice.theater == theater)
                .ok_or_else(|| {
                    let mut theaters: Vec<&str> =
                        fits.iter().map(|choice| choice.theater.as_str()).collect();
                    theaters.dedup();
                    format!(
                        "not fitted: Theater {theater:?} is not among the places that fit \
                         here ({})",
                        theaters.join(", ")
                    )
                })?;
            Ok(json!({
                "station": state.station_id,
                "theater": choice.theater,
                "agent": choice.agent,
            }))
        }
        ("agent", Some(agent)) => Ok(json!({
            "station": state.station_id,
            "agent": agent,
            "theater": first.theater,
        })),
        _ => Ok(json!({
            "station": state.station_id,
            "theater": first.theater,
            "agent": first.agent,
            "capacity": state.mission_capacity(),
        })),
    }
}

/// What a control on the charge of `scope` means, when it is a request or an answer made through
/// the tools: the kind of event it is and what it carries. The Station's own check of a request
/// rides along, so its charge sees the facts it answers from. Blocks on the Registry.
pub(super) fn classify(
    state: &DaemonState,
    scope: &ChargeScope,
    control: &ChargeControl,
) -> Option<(&'static str, Value)> {
    match (scope, serde_json::from_str::<Note>(&control.note).ok()?) {
        (ChargeScope::Station(station), Note::ResourceRequest(request))
            if control.by == format!("{REQUEST_BY}{}", request.operation)
                && request.station == *station =>
        {
            let verdict = match check(
                state,
                &request.operation,
                &request.kind,
                request.name.as_deref(),
            ) {
                Ok(provided) => json!({ "can_provide": provided }),
                Err(why) => json!({ "cannot_provide": why }),
            };
            Some((
                "request",
                json!({ "request": request, "station_check": verdict }),
            ))
        }
        (ChargeScope::Operation(operation), Note::ResourceAnswer(answer))
            if control.by == format!("{ANSWER_BY}{}", answer.station) =>
        {
            let _ = operation;
            Some(("resources", json!({ "answer": answer })))
        }
        _ => None,
    }
}

/// Wakes the charge `target` with `note` from `by`, answering why it was not left.
#[cfg(feature = "cables-mcp")]
fn leave(
    state: &DaemonState,
    target: &ChargeScope,
    by: &str,
    note: &Note,
) -> Result<ChargeControl, String> {
    let text = serde_json::to_string(note).map_err(|error| error.to_string())?;
    state
        .intake
        .request_charge_wake(&target.to_string(), by, &text)
        .map_err(|error| match error {
            ChargeError::NotFound => {
                format!("{target} has never run, so there is nothing to wake: a person decides")
            }
            other => other.to_string(),
        })
}

/// Records what a charge did across scopes, under the Station's name.
#[cfg(feature = "cables-mcp")]
fn audit(state: &DaemonState, action: &str, operation: &str, agent: &str, target: String) {
    use crate::directorates::registry::store::AuditRecord;
    let record = AuditRecord {
        id: format!("{action}-{}", uuid::Uuid::new_v4().simple()),
        actor: format!("agent:{agent}"),
        action: action.to_owned(),
        operation: operation.to_owned(),
        resource: Some(format!("station:{}", state.station_id)),
        target: target.chars().take(500).collect(),
        at: state.clock.now() as f64,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: audit record {action} was not written: {error}");
    }
}

/// A request as the 00 Agent's tool takes it.
#[cfg(feature = "cables-mcp")]
pub(super) struct Asked {
    pub(super) kind: String,
    pub(super) name: Option<String>,
    pub(super) detail: Option<String>,
    pub(super) station: Option<String>,
    pub(super) key: Option<String>,
}

/// An answer as the Station charge's tool takes it.
#[cfg(feature = "cables-mcp")]
pub(super) struct Answering {
    pub(super) request: String,
    pub(super) operation: String,
    pub(super) kind: String,
    pub(super) name: Option<String>,
    /// `provide` or `refuse`.
    pub(super) decision: String,
    pub(super) reason: Option<String>,
}

/// The tools of a charge that asks or flags: an Operation's 00 asks a Station for resources, M's
/// charge raises board flags.
#[cfg(feature = "cables-mcp")]
#[derive(Clone)]
pub(super) struct ResourceDesk {
    state: DaemonState,
    scope: ChargeScope,
    agent: String,
}

#[cfg(feature = "cables-mcp")]
impl ResourceDesk {
    /// The desk of the charge of `scope`, kept by `agent`.
    pub(super) fn new(state: DaemonState, scope: ChargeScope, agent: impl Into<String>) -> Self {
        Self {
            state,
            scope,
            agent: agent.into(),
        }
    }

    /// Whether the charge is M's.
    pub(super) fn is_m(&self) -> bool {
        self.scope == ChargeScope::M
    }

    fn admit(&self) -> Result<(), String> {
        if self.state.charges.is_live(&self.scope.to_string()) {
            Ok(())
        } else {
            Err("this Station no longer runs the charge".to_owned())
        }
    }

    /// Asks a Station for what the Operation needs. The answer comes as a wake of this charge.
    /// Blocks.
    pub(super) fn ask(&self, asked: Asked) -> Result<Value, String> {
        self.admit()?;
        let operation = self
            .scope
            .operation()
            .ok_or("only an Operation's 00 Agent in charge asks a Station for resources")?;
        if !KINDS.contains(&asked.kind.as_str()) {
            return Err(format!(
                "{:?} is not a request kind: use {KINDS:?}",
                asked.kind
            ));
        }
        let longest = self.state.intake.limits().charges.entry_bytes.or_max();
        let plain = |text: &str| {
            !text.is_empty() && text.len() <= longest && !text.chars().any(char::is_control)
        };
        if asked.name.as_deref().is_some_and(|name| !plain(name)) {
            return Err(format!(
                "a name is 1 to {longest} bytes without control characters"
            ));
        }
        let station = asked
            .station
            .unwrap_or_else(|| self.state.station_id.clone());
        let target: ChargeScope = format!("station:{station}").parse()?;
        let id = asked
            .key
            .filter(|key| plain(key))
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        let by = format!("{REQUEST_BY}{operation}");
        // Asking again with the same key is the same request.
        if let Ok(Some(record)) = self.state.intake.charge(&target.to_string())
            && record.controls.iter().any(|control| {
                control.by == by
                    && matches!(
                        serde_json::from_str::<Note>(&control.note),
                        Ok(Note::ResourceRequest(request)) if request.id == id
                    )
            })
        {
            return Ok(
                json!({ "id": id, "station": station, "status": "asked", "repeated": true }),
            );
        }
        let request = Request {
            id: id.clone(),
            kind: asked.kind,
            name: asked.name,
            detail: asked.detail,
            operation: operation.to_owned(),
            agent: self.agent.clone(),
            station: station.clone(),
        };
        leave(
            &self.state,
            &target,
            &by,
            &Note::ResourceRequest(request.clone()),
        )?;
        audit(
            &self.state,
            "resources.requested",
            operation,
            &self.agent,
            format!("{id} {} of Station {station}", request.kind),
        );
        Ok(json!({
            "id": id,
            "station": station,
            "status": "asked",
            "answer": "the Station's charge answers; it arrives as a wake of your charge",
        }))
    }

    /// Raises a flag on M's board: a Report of M's own scope, filed by this charge. Blocks.
    pub(super) fn flag(
        &self,
        filing: crate::directorates::m::reports::ReportFiling,
        run: Option<String>,
    ) -> Result<Value, String> {
        use crate::directorates::registry::reports::{ReportFiler, ReportScope};
        self.admit()?;
        if !self.is_m() {
            return Err("only M's charge raises flags on M's board".to_owned());
        }
        let state = &self.state;
        let scope = self.scope.to_string();
        let report = filing.into_report(
            ReportScope::M,
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
        state
            .intake
            .file_report(&report)
            .map_err(|error| format!("the flag was not filed: {error}"))?;
        audit(state, "flag.raised", "", &self.agent, report.id.clone());
        Ok(json!({ "filed": true, "id": report.id, "scope": report.scope.to_string() }))
    }
}

/// Answers a request on behalf of the Station's charge, kept by `agent`: the Station checks what
/// it can provide, so a provision it cannot make becomes a refusal with the reason. Blocks.
#[cfg(feature = "cables-mcp")]
pub(super) fn answer(
    state: &DaemonState,
    agent: &str,
    answering: Answering,
) -> Result<Value, String> {
    let target = ChargeScope::Operation(answering.operation.clone());
    let verdict = check(
        state,
        &answering.operation,
        &answering.kind,
        answering.name.as_deref(),
    );
    let said = answering
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(str::to_owned);
    let (outcome, provided, reason) = match (answering.decision.as_str(), verdict) {
        ("provide", Ok(provided)) => ("provided", Some(provided), None),
        ("provide", Err(why)) => ("refused", None, Some(why)),
        ("refuse", Ok(_)) => (
            "refused",
            None,
            Some(said.ok_or("a refusal gives its reason")?),
        ),
        ("refuse", Err(why)) => ("refused", None, Some(said.unwrap_or(why))),
        (other, _) => {
            return Err(format!(
                "{other:?} is not a decision: use provide or refuse"
            ));
        }
    };
    let reply = Answer {
        request: answering.request.clone(),
        station: state.station_id.clone(),
        kind: answering.kind.clone(),
        answer: outcome.to_owned(),
        provided,
        reason,
    };
    leave(
        state,
        &target,
        &format!("{ANSWER_BY}{}", state.station_id),
        &Note::ResourceAnswer(reply.clone()),
    )?;
    audit(
        state,
        "resources.answered",
        &answering.operation,
        agent,
        format!("{} {outcome}", answering.request),
    );
    serde_json::to_value(&reply).map_err(|error| error.to_string())
}
