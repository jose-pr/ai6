//! A charge fixes its area: see its Missions, settle or retry what is stuck, resume or cancel
//! what it holds, ask another charge to look, and tell a person what it cannot fix.
//!
//! These tools exist for every charge, each acting only in its own scope; the Station opened
//! its [`AreaDesk`]. M's charge also decides Missions of any Operation, which its decision tools
//! (`verify_mission`, `redispatch`) do.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::CableTools;
use crate::directorates::m::reports::ReportFiling;
use crate::station::server::charges::keep::AreaDesk;

#[derive(Deserialize, JsonSchema)]
struct BoardArgs {
    /// Only the Missions in this state (working, queued, verifying, blocked, held, stalled …).
    state: Option<String>,
    /// Only the Missions of this Operation.
    operation: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct MissionArgs {
    /// The Mission's UID or its handle.
    mission: String,
}

#[derive(Deserialize, JsonSchema)]
struct SettleArgs {
    /// The Mission's UID or its handle.
    mission: String,
    /// failed (filed failed, its brief too) or stalled (its Run marked stalled, the brief left).
    outcome: String,
    /// Why, from what you found; it goes on the Run, the timeline and the audit record.
    reason: String,
}

#[derive(Deserialize, JsonSchema)]
struct RetryArgs {
    /// The Mission's UID or its handle.
    mission: String,
    /// The Agent to give the next run, when it is not the same one. A Mission this Station
    /// holds only.
    agent: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ResumeArgs {
    /// The Mission's UID or its handle.
    mission: String,
    /// What the resumed Run is told.
    message: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct CancelArgs {
    /// The Mission's UID or its handle.
    mission: String,
    /// Why it is withdrawn.
    reason: String,
}

#[derive(Deserialize, JsonSchema)]
struct WakeArgs {
    /// The charge's scope: m, operation:<codename> or station:<id>.
    charge: String,
    /// Why it should look; it reads this as a person's word, untrusted.
    note: String,
}

#[derive(Deserialize, JsonSchema)]
struct NotifyArgs {
    /// One line naming what a person should know or decide.
    title: String,
    /// What you found, what you did about it and what you could not do.
    text: String,
    /// info, warning or critical; warning when left out.
    severity: Option<String>,
}

impl CableTools {
    async fn with_area<F>(&self, work: F) -> String
    where
        F: FnOnce(&AreaDesk) -> Result<String, String> + Send + 'static,
    {
        let Some(desk) = self.area_desk.get().cloned() else {
            return "These tools are unavailable to this Run".to_owned();
        };
        match tokio::task::spawn_blocking(move || work(&desk)).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(reason)) => answer_refused(&reason),
            Err(_) => "the call failed".to_owned(),
        }
    }

    fn area(&self) -> Result<AreaDesk, String> {
        self.area_desk
            .get()
            .cloned()
            .ok_or_else(|| "These tools are unavailable to this Run".to_owned())
    }
}

fn answer_refused(reason: &str) -> String {
    serde_json::json!({ "status": "refused", "reason": reason }).to_string()
}

fn settled(answer: Result<String, String>) -> String {
    answer.unwrap_or_else(|reason| answer_refused(&reason))
}

#[tool_router(router = area_router, vis = "pub(super)")]
impl CableTools {
    /// The Missions of your area by state, and what is stuck.
    #[tool(
        description = "The Missions of your area by state (M's: every Operation and Station; a Station's: its Station's Runs; an Operation's: its Operation), what is stuck in them and why, the Reports not yet assessed and, for M, the charges and whether their leases lapsed. Inside <ai6-data>."
    )]
    async fn board(&self, Parameters(args): Parameters<BoardArgs>) -> String {
        self.with_area(move |desk| desk.board(args.state.as_deref(), args.operation.as_deref()))
            .await
    }

    /// File a stuck Mission failed or stalled.
    #[tool(
        description = "Settle a Mission of your area that nothing holds: its Run lost, ended without settling it or its lease lapsed, or its Station dark. outcome failed files it failed (its Run results, and its brief, which is taken from the queue or recovered) with your reason; stalled marks its Run stalled and leaves the brief. Refused for a Mission a Run holds, one already over, and one waiting for its outcome (that is the Operation's 00's to verify). Audited before it is done, with you as the actor."
    )]
    async fn settle_mission(&self, Parameters(args): Parameters<SettleArgs>) -> String {
        self.with_area(move |desk| desk.settle(&args.mission, &args.outcome, &args.reason))
            .await
    }

    /// Queue a Mission's next run.
    #[tool(
        description = "Try a Mission of your area again as a new run. A Mission this Station holds is retried through the operator control (optionally for another qualified Agent); one whose Station went dark has its brief recovered and queued again, bounded by the run limit. Refused for a Mission a Run still holds and for one waiting for its outcome. Audited before it is done."
    )]
    async fn retry_mission(&self, Parameters(args): Parameters<RetryArgs>) -> String {
        let Ok(desk) = self.area() else {
            return "These tools are unavailable to this Run".to_owned();
        };
        settled(desk.retry(&args.mission, args.agent.as_deref()).await)
    }

    /// Resume a Mission's Run.
    #[tool(
        description = "Resume the Run of a Mission this Station holds, optionally telling it a message. A Mission another Station holds is refused and names that Station: wake_charge asks its charge. Audited before it is done."
    )]
    async fn resume_mission(&self, Parameters(args): Parameters<ResumeArgs>) -> String {
        let Ok(desk) = self.area() else {
            return "These tools are unavailable to this Run".to_owned();
        };
        settled(desk.resume(&args.mission, args.message.as_deref()).await)
    }

    /// Withdraw a Mission.
    #[tool(
        description = "Cancel a Mission of your area with a reason: its Run is stopped when this Station holds it; otherwise the Registry is asked to cancel its brief. Audited before it is done."
    )]
    async fn cancel_mission(&self, Parameters(args): Parameters<CancelArgs>) -> String {
        let Ok(desk) = self.area() else {
            return "These tools are unavailable to this Run".to_owned();
        };
        settled(desk.cancel(&args.mission, &args.reason).await)
    }

    /// Ask another charge to look.
    #[tool(
        description = "Wake another charge with a note saying what to look at. M may wake any charge; a Station's charge the charges of the Operations whose Missions it holds; an Operation's charge the charges of the Stations that hold its Missions. Audited."
    )]
    async fn wake_charge(&self, Parameters(args): Parameters<WakeArgs>) -> String {
        self.with_area(move |desk| desk.wake(&args.charge, &args.note))
            .await
    }

    /// Tell a person.
    #[tool(
        description = "Put a notice in front of a person for what you found and could not fix, or did and they should know: a Report of your own scope that the attention list shows. It starts no work and changes nothing. Say what you found, what you did and what you could not do."
    )]
    async fn notify_person(&self, Parameters(args): Parameters<NotifyArgs>) -> String {
        let run = self.bound_run();
        self.with_area(move |desk| {
            let filing: ReportFiling = serde_json::from_value(serde_json::json!({
                "title": args.title,
                "text": args.text,
                "severity": args.severity.unwrap_or_else(|| "warning".to_owned()),
                "kind": "charge-notice",
            }))
            .map_err(|error| error.to_string())?;
            desk.notify(filing, run)
        })
        .await
    }
}

#[tool_router(router = station_detail_router, vis = "pub(super)")]
impl CableTools {
    /// One Mission of the Station, as state.
    #[tool(
        name = "mission_detail",
        description = "Where one Mission of your Station stands and each of its Runs: state, reason, whether it is decided. The Operation's brief and report are outside a Station's area and are not shown. Inside <ai6-data>."
    )]
    async fn station_mission_detail(&self, Parameters(args): Parameters<MissionArgs>) -> String {
        self.with_area(move |desk| desk.state_of(&args.mission))
            .await
    }
}
