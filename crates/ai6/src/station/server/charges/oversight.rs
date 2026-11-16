//! What an Operation's charge reads of its Operation, and nothing else: find Missions, see one
//! with its brief and outcome, its attempts, the briefs held back, its timeline and the green ink
//! waiting.
//!
//! Every tool is read-only and works from the Operation the charge was started for; a Mission of
//! any other Operation answers as one that does not exist, and so does a Mission nobody
//! filed. Text that came from outside the Operation (a brief, an outcome, a held brief's name)
//! is fenced as data and cut at `[oversight] text_bytes`; the reads that cross an Operation are
//! the compartment tools'.

use serde_json::{Value, json};

use super::decide::DecisionDesk;
use crate::bounds::Limit;
use crate::directorates::m::knowledge::fenced;
use crate::directorates::registry::products::mission_status;
use crate::directorates::registry::store::{BriefSearch, FiledBrief};

/// What every Mission the charge may not read answers, whatever the reason.
const UNKNOWN_MISSION: &str = "unknown Mission";

/// What a search of the charge's Operation asks; every field is optional.
#[derive(Debug, Default)]
pub(in crate::station::server) struct Find {
    pub(in crate::station::server) text: Option<String>,
    pub(in crate::station::server) handle: Option<String>,
    pub(in crate::station::server) ticket: Option<String>,
    pub(in crate::station::server) agent: Option<String>,
    pub(in crate::station::server) state: Option<String>,
    pub(in crate::station::server) limit: Option<usize>,
}

/// `text` within `limit` bytes, with a mark where it was cut.
fn cut(text: &str, limit: Limit) -> Value {
    let bytes = limit.or_max();
    if text.len() <= bytes {
        return json!(text);
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    json!({ "text": &text[..end], "cut": true, "bytes": text.len() })
}

impl DecisionDesk {
    /// The `[oversight]` table as this Station runs it.
    fn read_limits(&self) -> crate::station::limits::OversightLimits {
        self.state.station.limits().oversight.clone()
    }

    /// The brief of the Mission `name` names (a UID or any handle), when it is the Operation's.
    fn filed(&self, name: &str) -> Result<FiledBrief, String> {
        let intake = &self.state.intake;
        let key = match intake.resolve_mission(name) {
            Ok(Some(found)) => found.mission_id,
            _ => name.to_owned(),
        };
        intake
            .filed_brief(&key)
            .ok()
            .flatten()
            .filter(|filed| filed.brief.operation.as_deref() == Some(self.operation.as_str()))
            .ok_or_else(|| UNKNOWN_MISSION.to_owned())
    }

    fn row(&self, filed: &FiledBrief) -> Value {
        let mut row = mission_status(self.state.intake.as_ref(), filed);
        row["handle"] = json!(filed.brief.shown_handle());
        row["title_untrusted"] = json!(filed.brief.display_name(256));
        row
    }

    fn page(&self, asked: Option<usize>) -> Result<usize, String> {
        let limits = self.read_limits();
        let page = asked.unwrap_or_else(|| usize::try_from(limits.page).unwrap_or(usize::MAX));
        if page == 0 || !limits.max_page.allows(page) {
            return Err(format!("a page is 1 to {} items", limits.max_page));
        }
        Ok(page)
    }

    /// The Operation's Missions that match, newest first, with their status and no brief.
    pub(in crate::station::server) fn search(&self, find: Find) -> Result<String, String> {
        let limits = self.read_limits();
        let page = self.page(find.limit)?;
        let given = |text: Option<String>| text.filter(|text| !text.trim().is_empty());
        let search = BriefSearch {
            operation: Some(self.operation.clone()),
            text: given(find.text),
            handle: given(find.handle),
            ticket: given(find.ticket),
            agent: given(find.agent),
            state: given(find.state),
            scan: limits.scan,
        };
        if search
            .text
            .as_ref()
            .is_some_and(|text| !limits.text_bytes.allows(text.len()))
        {
            return Err(format!(
                "search text is at most {} bytes",
                limits.text_bytes
            ));
        }
        let found = self
            .state
            .intake
            .search_briefs(&search, page.saturating_add(1))
            .map_err(|error| error.to_string())?;
        let more = found.len() > page;
        let rows: Vec<Value> = found
            .iter()
            .take(page)
            .map(|filed| self.row(filed))
            .collect();
        Ok(fenced(
            "search_missions",
            &json!({ "missions": rows, "more": more }),
        ))
    }

    /// One Mission: its status, brief and outcome.
    pub(in crate::station::server) fn mission(&self, name: &str) -> Result<String, String> {
        let limits = self.read_limits();
        let filed = self.filed(name)?;
        let mut answer = self.row(&filed);
        answer["brief_untrusted"] = cut(&filed.brief.body, limits.text_bytes);
        answer["url"] = json!(filed.brief.url);
        answer["outcome_untrusted"] = match &filed.outcome {
            Some(outcome) => cut(&outcome.to_string(), limits.text_bytes),
            None => Value::Null,
        };
        Ok(fenced("mission", &answer))
    }

    /// A Mission's attempts: each run's state, Agent and times, oldest first.
    pub(in crate::station::server) fn attempts(&self, name: &str) -> Result<String, String> {
        let filed = self.filed(name)?;
        let key = filed.brief.key();
        let mut runs: Vec<_> = self
            .state
            .intake
            .runs()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|run| run.mission_id == key)
            .collect();
        runs.sort_by_key(|run| run.number);
        let rows: Vec<Value> = runs
            .iter()
            .map(|run| {
                json!({
                    "number": run.number,
                    "handle": run.handle,
                    "status": serde_json::to_value(run.status).unwrap_or_default(),
                    "state": run.result.as_ref().map(|result| result.state.as_str()),
                    "agent": run.launch.as_ref().and_then(|launch| launch.agent.as_deref()),
                    "station": run.station,
                    "opened_at": run.opened_at,
                    "closed_at": run.closed_at,
                    "lost": run.lost,
                    "decided": run.verdict.is_some(),
                })
            })
            .collect();
        Ok(fenced(
            "mission_attempts",
            &json!({ "mission": key, "attempts": rows }),
        ))
    }

    /// The briefs vetting is holding back for a person, newest first.
    pub(in crate::station::server) fn held(&self, limit: Option<usize>) -> Result<String, String> {
        let page = self.page(limit)?;
        let held = self
            .state
            .intake
            .held_briefs(&self.operation, page.saturating_add(1))
            .map_err(|error| error.to_string())?;
        let more = held.len() > page;
        let rows: Vec<Value> = held
            .iter()
            .take(page)
            .map(|held| {
                json!({
                    "mission": held.brief.key(),
                    "handle": held.brief.shown_handle(),
                    "title_untrusted": held.brief.display_name(256),
                    "source": held.brief.source,
                    "held_at": held.held_at,
                    "kinds": held.vetting.kinds().iter().map(|kind| format!("{kind:?}")).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(fenced(
            "held_briefs",
            &json!({ "held": rows, "more": more }),
        ))
    }

    /// What happened to a Mission, or to the Operation, newest first.
    pub(in crate::station::server) fn timeline(
        &self,
        mission: Option<&str>,
        limit: Option<usize>,
    ) -> Result<String, String> {
        let page = self.page(limit)?;
        let key = match mission {
            Some(name) => Some(self.filed(name)?.brief.key()),
            None => None,
        };
        let mut events = self
            .state
            .intake
            .timeline_events(&self.operation)
            .map_err(|error| error.to_string())?;
        events.retain(|event| {
            key.as_ref()
                .is_none_or(|key| event.mission.as_ref() == Some(key))
        });
        events.sort_by(|a, b| b.at.total_cmp(&a.at));
        let more = events.len() > page;
        events.truncate(page);
        Ok(fenced(
            "timeline",
            &json!({ "events": events, "more": more }),
        ))
    }

    fn pending(
        &self,
    ) -> Result<Vec<crate::directorates::registry::approvals::ApprovalRequest>, String> {
        self.state
            .intake
            .pending_approvals(&self.operation, self.state.clock.now() as f64)
            .map_err(|error| error.to_string())
    }

    /// The green ink waiting for a decision in the Operation, or every request a Mission's
    /// Agents made with its decision.
    pub(in crate::station::server) fn approvals(
        &self,
        mission: Option<&str>,
    ) -> Result<String, String> {
        let now = self.state.clock.now() as f64;
        let answer = match mission {
            Some(name) => {
                let key = self.filed(name)?.brief.key();
                let statuses = self
                    .state
                    .intake
                    .mission_approvals(&self.operation, &key, now)
                    .map_err(|error| error.to_string())?;
                json!({ "mission": key, "approvals": statuses.iter().map(status_json).collect::<Vec<_>>() })
            }
            None => json!({
                "pending": self.pending()?.iter().map(request_json).collect::<Vec<_>>(),
            }),
        };
        Ok(fenced("approvals", &answer))
    }
}

fn request_json(request: &crate::directorates::registry::approvals::ApprovalRequest) -> Value {
    json!({
        "id": request.id,
        "mission": request.mission.as_str(),
        "agent": request.agent,
        "kind": request.scope.kind,
        "target": request.scope.target,
        "requested_at": request.requested_at,
        "expires_at": request.expires_at,
    })
}

fn status_json(status: &crate::directorates::registry::approvals::ApprovalStatus) -> Value {
    use crate::directorates::registry::approvals::ApprovalStatus;
    match status {
        ApprovalStatus::Pending(request) => {
            let mut row = request_json(request);
            row["decision"] = Value::Null;
            row
        }
        ApprovalStatus::Decided(approval) => {
            let mut row = request_json(&approval.request);
            row["decision"] = json!(approval.decision.as_str());
            row["signer"] = json!(approval.signer);
            row["at"] = json!(approval.at);
            row
        }
    }
}

#[cfg(all(test, feature = "cables-mcp"))]
#[path = "oversight_tests.rs"]
mod tests;
