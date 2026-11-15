//! What a charge's area holds: its Missions by state, the ones that are stuck, and whether it has
//! any open work.
//!
//! The Registry's Run records and queue say where each Mission stands for every scope: M's charge
//! reads all of them, an Operation's its Operation's, a Station's the Runs of that Station. A
//! Mission is stuck when it stands where nothing is going to move it: `working` with no Run
//! holding it, `verifying` or `queued` for long, `blocked` or `held` for long. Every threshold is
//! a setting of the charge, with no floor, and `none` turns it off.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::super::DaemonState;
use crate::directorates::m::charges::{Charge, ChargeScope};
use crate::directorates::registry::reports::{ReportFilter, ReportScope, ReportState};
use crate::directorates::registry::runs::{RunRecord, RunStatus};
use crate::directorates::registry::store::Brief;

/// The states a Mission is over in.
fn is_over(state: &str) -> bool {
    matches!(state, "done" | "failed" | "abandoned" | "blown")
}

/// A Mission of a charge's area, as its latest Run and the queue say it stands.
#[derive(Debug, Clone)]
pub(in crate::station::server) struct MissionView {
    pub(in crate::station::server) mission: String,
    pub(in crate::station::server) handle: Option<String>,
    pub(in crate::station::server) operation: Option<String>,
    pub(in crate::station::server) station: Option<String>,
    pub(in crate::station::server) agent: Option<String>,
    /// `working`, `blocked`, `stalled`, `parked`, `held`, `verifying`, `queued` and the rest of
    /// the Run states, as the latest Run reports them.
    pub(in crate::station::server) state: String,
    /// The latest Run's number, 0 for a Mission that has none.
    pub(in crate::station::server) number: u32,
    /// The latest Run's ID.
    pub(in crate::station::server) run: Option<String>,
    /// When it came to stand there, as Unix seconds, when the Registry says.
    pub(in crate::station::server) since: Option<f64>,
    /// Whether the latest Run is closed, or its lease lapsed: nothing holds the Mission.
    pub(in crate::station::server) unheld: bool,
    /// Whether a verdict has been recorded on the latest Run.
    pub(in crate::station::server) decided: bool,
    /// Why it stands there, when AI6 knows (a lost Run's cause, a block's code).
    pub(in crate::station::server) reason: Option<String>,
}

/// One reason a Mission, or a charge, is judged stuck.
#[derive(Debug, Clone)]
pub(in crate::station::server) struct Stuck {
    /// `run-lost`, `verifying`, `queued`, `blocked`, `held`, `charge-lapsed` or `charge-failing`.
    pub(in crate::station::server) condition: &'static str,
    pub(in crate::station::server) since: f64,
    pub(in crate::station::server) why: String,
    /// What the charge remembers it by, so it is told once.
    pub(in crate::station::server) key: String,
    pub(in crate::station::server) view: Option<MissionView>,
    /// The scope of a charge that is stuck.
    pub(in crate::station::server) charge: Option<String>,
}

/// What a charge's area holds now.
#[derive(Debug, Default)]
pub(in crate::station::server) struct Survey {
    pub(in crate::station::server) missions: Vec<MissionView>,
    pub(in crate::station::server) stuck: Vec<Stuck>,
    /// Reports of the area not yet assessed.
    pub(in crate::station::server) open_reports: usize,
}

impl Survey {
    /// Whether the area has work that is open: a Mission that is not over, a Report not yet
    /// assessed.
    pub(in crate::station::server) fn has_open_work(&self) -> bool {
        !self.missions.is_empty() || self.open_reports > 0
    }

    /// A short health summary of the area: counts by state, the oldest Mission that is not over.
    pub(in crate::station::server) fn summary(&self) -> Value {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for view in &self.missions {
            *counts.entry(view.state.as_str()).or_default() += 1;
        }
        let oldest = self
            .missions
            .iter()
            .filter(|view| view.since.is_some())
            .min_by(|a, b| a.since.unwrap_or(0.0).total_cmp(&b.since.unwrap_or(0.0)));
        json!({
            "missions_by_state": counts,
            "oldest_open": oldest.map(|view| json!({
                "mission": view.mission,
                "handle": view.handle,
                "state": view.state,
                "since": view.since,
            })),
            "reports_unassessed": self.open_reports,
            "stuck_now": self.stuck.len(),
        })
    }
}

/// Whether `view` belongs to the area of `scope`.
fn in_scope(scope: &ChargeScope, operation: Option<&str>, station: Option<&str>) -> bool {
    match scope {
        ChargeScope::M => true,
        ChargeScope::Operation(own) => operation == Some(own.as_str()),
        ChargeScope::Station(own) => station == Some(own.as_str()),
    }
}

/// When a queued brief was accepted, as Unix seconds.
fn queued_since(brief: &Brief) -> Option<f64> {
    brief
        .provenance
        .as_ref()
        .map(|provenance| provenance.received_at as f64)
        .or_else(|| {
            time::OffsetDateTime::parse(
                &brief.created,
                &time::format_description::well_known::Rfc3339,
            )
            .ok()
            .map(|at| at.unix_timestamp() as f64)
        })
}

/// The view of a Mission from its latest Run `run`, `now` being Unix seconds.
pub(in crate::station::server) fn run_view(
    run: &RunRecord,
    queued: bool,
    now: f64,
) -> Option<MissionView> {
    let reported = run
        .result
        .as_ref()
        .map_or("working", |result| result.state.as_str());
    let closed = run.status.is_closed();
    if is_over(reported) && closed && !queued {
        return None;
    }
    let lapsed = run.status == RunStatus::Active && !run.leased_at(now);
    // A lost Run reports its Mission queued; with nothing in the queue it is queued nowhere.
    let state = if queued {
        "queued"
    } else if closed && reported == "queued" {
        "working"
    } else {
        reported
    };
    let since = if closed {
        run.closed_at
    } else if lapsed {
        Some(run.lease_expires)
    } else {
        None
    };
    Some(MissionView {
        mission: run.mission_id.clone(),
        handle: run
            .handle
            .as_deref()
            .map(|handle| {
                handle
                    .rsplit_once('.')
                    .map_or(handle, |(mission, _)| mission)
            })
            .map(str::to_owned),
        operation: run.operation.clone(),
        station: Some(run.station.clone()),
        agent: run.launch.as_ref().and_then(|launch| launch.agent.clone()),
        state: state.to_owned(),
        number: run.number,
        run: Some(run.id.clone()),
        since,
        unheld: closed || lapsed,
        decided: run.verdict.is_some(),
        reason: run
            .lost
            .clone()
            .or_else(|| run.result.as_ref().and_then(|result| result.reason.clone())),
    })
}

/// The Missions of `scope` that are not over, from the Run records and the queue. Blocks.
pub(in crate::station::server) fn missions(
    state: &DaemonState,
    scope: &ChargeScope,
    now: f64,
) -> std::io::Result<Vec<MissionView>> {
    let queue = state.intake.list_new()?;
    let queued: BTreeMap<String, &Brief> = queue
        .iter()
        .map(|brief| (brief.key().to_owned(), brief))
        .collect();
    let mut latest: BTreeMap<String, RunRecord> = BTreeMap::new();
    for record in state.intake.runs()? {
        match latest.get(&record.mission_id) {
            Some(held) if held.number >= record.number => {}
            _ => {
                latest.insert(record.mission_id.clone(), record);
            }
        }
    }
    let mut views = Vec::new();
    for (mission, run) in &latest {
        let waiting = queued.contains_key(mission);
        if let Some(mut view) = run_view(run, waiting, now) {
            if waiting {
                view.since = view
                    .since
                    .or_else(|| queued.get(mission).and_then(|brief| queued_since(brief)));
            }
            if in_scope(scope, view.operation.as_deref(), view.station.as_deref()) {
                views.push(view);
            }
        }
    }
    // Queued Missions that have no Run yet belong to no Station; a Station's charge has none.
    if !matches!(scope, ChargeScope::Station(_)) {
        for brief in &queue {
            if latest.contains_key(&brief.key()) {
                continue;
            }
            let operation = brief.operation.clone();
            if in_scope(scope, operation.as_deref(), None) {
                views.push(MissionView {
                    mission: brief.key().to_owned(),
                    handle: brief.handle.clone(),
                    operation,
                    station: None,
                    agent: brief.agent.clone(),
                    state: "queued".to_owned(),
                    number: 0,
                    run: None,
                    since: queued_since(brief),
                    unheld: true,
                    decided: false,
                    reason: None,
                });
            }
        }
    }
    Ok(views)
}

/// The conditions `view` is stuck in, by `charge`'s thresholds. `first_seen` remembers when a
/// Mission was first seen blocked, which the Registry does not date.
fn judge(
    view: &MissionView,
    charge: &Charge,
    now: f64,
    first_seen: &mut BTreeMap<String, f64>,
    live_stations: usize,
) -> Option<Stuck> {
    let settings = &charge.settings;
    let run = view.run.as_deref().unwrap_or("queue");
    let key = |condition: &str| format!("stuck:{condition}:{}:{}:{run}", view.mission, view.number);
    let quiet = |since: f64| now - since;
    let past = |after: crate::bounds::TimeLimit, since: f64| {
        after
            .get()
            .is_some_and(|after| quiet(since) >= after.as_secs_f64())
    };
    let stuck = |condition: &'static str, since: f64, why: String| Stuck {
        condition,
        since,
        why,
        key: key(condition),
        view: Some(view.clone()),
        charge: None,
    };
    match view.state.as_str() {
        "queued" => {
            let since = view.since?;
            past(settings.stuck_queued_after, since).then(|| {
                stuck(
                    "queued",
                    since,
                    format!(
                        "it has waited in the queue for {}s and {live_stations} Station(s) are \
                         checking in; none has taken it",
                        quiet(since) as u64
                    ),
                )
            })
        }
        "verifying" => {
            let since = match view.since {
                Some(since) => since,
                None => *first_seen.entry(key("verifying")).or_insert(now),
            };
            past(settings.stuck_verifying_after, since).then(|| {
                stuck(
                    "verifying",
                    since,
                    format!(
                        "it has waited {}s for its outcome to be verified{}",
                        quiet(since) as u64,
                        if view.decided {
                            "; a verdict is recorded, its Station has not settled it"
                        } else {
                            ""
                        }
                    ),
                )
            })
        }
        "held" | "blocked" => {
            let condition = if view.state == "held" {
                "held"
            } else {
                "blocked"
            };
            let since = match view.since {
                Some(since) => since,
                None => *first_seen.entry(key(condition)).or_insert(now),
            };
            past(settings.stuck_blocked_after, since).then(|| {
                stuck(
                    condition,
                    since,
                    format!("it has been {condition} for {}s", quiet(since) as u64),
                )
            })
        }
        "parked" | "quarantined" | "stalled" => None,
        _ => {
            // Working, or any state a Run reports as under way: stuck when nothing holds it.
            if !view.unheld {
                return None;
            }
            let since = view.since?;
            past(settings.stuck_run_lost_after, since).then(|| {
                stuck(
                    "run-lost",
                    since,
                    format!(
                        "it stands {} but no Run holds it for {}s",
                        view.state,
                        quiet(since) as u64
                    ),
                )
            })
        }
    }
}

/// The other charges that are stuck, for M's charge: a lease that lapsed, or a last turn that
/// failed to run, for longer than `stuck_charge_after`.
fn charges_stuck(state: &DaemonState, charge: &Charge, now: f64) -> Vec<Stuck> {
    let Some(after) = charge.settings.stuck_charge_after.get() else {
        return Vec::new();
    };
    let own = charge.scope.to_string();
    let Ok(records) = state.intake.charges() else {
        return Vec::new();
    };
    let wanted: Vec<String> = state.roster.charges().keys().cloned().collect();
    records
        .into_iter()
        .filter(|record| record.scope != own && wanted.contains(&record.scope))
        .filter_map(|record| {
            let after = after.as_secs_f64();
            if record.lease_expires < now && now - record.lease_expires >= after {
                return Some(Stuck {
                    condition: "charge-lapsed",
                    since: record.lease_expires,
                    why: format!(
                        "its lease lapsed {}s ago: no Station is running the charge",
                        (now - record.lease_expires) as u64
                    ),
                    key: format!(
                        "stuck:charge-lapsed:{}:{}",
                        record.scope, record.lease_expires
                    ),
                    view: None,
                    charge: Some(record.scope.clone()),
                });
            }
            let failed = record
                .logbook
                .iter()
                .rev()
                .find(|entry| entry.kind == "turn" || entry.kind == "turn-refused")
                .filter(|entry| entry.kind == "turn-refused")?;
            let since = failed.at;
            (now - since >= after).then(|| Stuck {
                condition: "charge-failing",
                since,
                why: "its last turn did not run".to_owned(),
                key: format!("stuck:charge-failing:{}:{since}", record.scope),
                view: None,
                charge: Some(record.scope.clone()),
            })
        })
        .collect()
}

/// Reads the area of `charge`: its Missions, what is stuck in them and its open Reports. Blocks.
pub(in crate::station::server) fn survey(
    state: &DaemonState,
    charge: &Charge,
    first_seen: &mut BTreeMap<String, f64>,
    now: f64,
) -> Survey {
    let missions = match missions(state, &charge.scope, now) {
        Ok(missions) => missions,
        Err(error) => {
            eprintln!("ai6: charge {}: Missions unreadable: {error}", charge.scope);
            return Survey::default();
        }
    };
    let live_stations = state
        .intake
        .stations(state.intake.limits().stations.gone_dark_after.0)
        .map_or(0, |stations| stations.len());
    // What was blocked and is no more starts its clock again.
    let blocked: Vec<String> = missions
        .iter()
        .filter(|view| matches!(view.state.as_str(), "blocked" | "held" | "verifying"))
        .map(|view| {
            format!(
                "stuck:{}:{}:{}:{}",
                view.state,
                view.mission,
                view.number,
                view.run.as_deref().unwrap_or("queue")
            )
        })
        .collect();
    first_seen.retain(|key, _| blocked.contains(key));
    let mut stuck: Vec<Stuck> = missions
        .iter()
        .filter_map(|view| judge(view, charge, now, first_seen, live_stations))
        .collect();
    if charge.scope == ChargeScope::M {
        stuck.extend(charges_stuck(state, charge, now));
    }
    let scope = match &charge.scope {
        ChargeScope::M => None,
        ChargeScope::Operation(operation) => Some(ReportScope::Operation(operation.clone())),
        ChargeScope::Station(station) => Some(ReportScope::Station(station.clone())),
    };
    let mut filter = ReportFilter::all().in_state(ReportState::Open);
    if let Some(scope) = scope {
        filter = filter.in_scope(scope);
    }
    let open_reports = state
        .intake
        .reports(&filter)
        .map_or(0, |reports| reports.len());
    Survey {
        missions,
        stuck,
        open_reports,
    }
}

/// A stuck Mission or charge as the event's detail.
pub(in crate::station::server) fn stuck_detail(stuck: &Stuck, now: f64) -> Value {
    let mut detail = json!({
        "condition": stuck.condition,
        "since": stuck.since,
        "stuck_for_secs": (now - stuck.since).max(0.0) as u64,
        "why": stuck.why,
    });
    if let Some(view) = &stuck.view {
        detail["mission"] = json!(view.mission);
        detail["handle"] = json!(view.handle);
        detail["operation"] = json!(view.operation);
        detail["station"] = json!(view.station);
        detail["agent"] = json!(view.agent);
        detail["state"] = json!(view.state);
        detail["run"] = json!(view.run);
    }
    if let Some(charge) = &stuck.charge {
        detail["charge"] = json!(charge);
    }
    detail
}
