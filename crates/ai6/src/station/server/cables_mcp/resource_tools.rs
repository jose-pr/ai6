//! What a charge asks of a Station, and what M's charge flags.
//!
//! `request_resources` exists for an Operation's charge: the 00 Agent in charge asks a Station for
//! an Agent, a Theater, an account or capacity. The Station's charge answers, and the answer
//! wakes the 00. `raise_flag` exists for M's charge: a flag on M's board, which is a Report of
//! M's own scope. Neither gives a charge a power it did not have: the first is a question the
//! Station answers by its own rules, the second a Report any Agent may file.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use super::CableTools;
use crate::directorates::m::reports::ReportFiling;
use crate::station::server::resources::{Asked, ResourceDesk};

#[derive(Deserialize, JsonSchema)]
struct RequestArgs {
    /// agent, theater, account or capacity.
    kind: String,
    /// The Agent (for agent or account), the Theater or nothing, to take whatever fits.
    name: Option<String>,
    /// Why you need it, in a sentence the Station's charge can answer.
    detail: Option<String>,
    /// The Station to ask; the Station running your charge when left out.
    station: Option<String>,
    /// Stable retry key; asking again with it is the same request.
    idempotency_key: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct FlagArgs {
    /// One line naming what you flag.
    title: String,
    /// What you found, from what you read, and what you did or ask be done.
    text: String,
    /// info, warning or critical; info when left out.
    severity: Option<String>,
    /// A short label, `flag` when left out.
    kind: Option<String>,
}

impl CableTools {
    async fn with_resources<F>(&self, work: F) -> String
    where
        F: FnOnce(&ResourceDesk) -> Result<serde_json::Value, String> + Send + 'static,
    {
        let Some(desk) = self.resource_desk.get().cloned() else {
            return "These tools are unavailable to this Run".to_owned();
        };
        match tokio::task::spawn_blocking(move || work(&desk)).await {
            Ok(Ok(answer)) => answer.to_string(),
            Ok(Err(reason)) => {
                serde_json::json!({ "status": "refused", "reason": reason }).to_string()
            }
            Err(_) => "the call failed".to_owned(),
        }
    }
}

#[tool_router(router = ask_router, vis = "pub(super)")]
impl CableTools {
    /// Ask a Station for what your Operation needs to run.
    #[tool(
        description = "Ask a Station for an Agent, a Theater, an account or capacity your Operation needs to run its work. The Station's charge provides it or answers why not (busy, not fitted, outside your Operation's allowance); the answer arrives as a wake of your charge, so end your turn and read it then. The Station never judges your Operation's work, and a refusal says what to change."
    )]
    async fn request_resources(&self, Parameters(args): Parameters<RequestArgs>) -> String {
        self.with_resources(move |desk| {
            desk.ask(Asked {
                kind: args.kind,
                name: args.name,
                detail: args.detail,
                station: args.station,
                key: args.idempotency_key,
            })
        })
        .await
    }
}

#[tool_router(router = flag_router, vis = "pub(super)")]
impl CableTools {
    /// Raise a flag on M's board.
    #[tool(
        description = "Raise a flag on M's board: a Report of M's own scope that the board shows to people, for what you found across the deployment that someone should know or decide about (a Station gone dark, a Source poll that froze, a Cable that cannot be placed). It starts no work and changes nothing: you act only through the controls M already has."
    )]
    async fn raise_flag(&self, Parameters(args): Parameters<FlagArgs>) -> String {
        let run = self.bound_run();
        self.with_resources(move |desk| {
            let filing: ReportFiling = serde_json::from_value(serde_json::json!({
                "title": args.title,
                "text": args.text,
                "severity": args.severity,
                "kind": args.kind.unwrap_or_else(|| "flag".to_owned()),
            }))
            .map_err(|error| error.to_string())?;
            desk.flag(filing, run)
        })
        .await
    }
}
