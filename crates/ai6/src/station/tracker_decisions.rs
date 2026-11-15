//! Decisions written as comments on a linked item.
//!
//! Off unless a Source lists `decisions = { approvers = [...] }`. A comment on an item a Mission is
//! linked to that is exactly `/approve`, or `/reject` and a reason, decides the green ink
//! the Mission's Agent is waiting for when its author is listed, by the tracker's account ID
//! (`Reply::author_id`). Such a comment is never delivered to the Agent, whoever wrote it.
//!
//! The decision is audited before it is made, with the comment's ID, and a comment from anyone
//! not listed decides nothing and is audited as refused. A request that asks to raise an Agent's
//! clearance or to hold one needs `agent.configure`, which a comment cannot show: those are
//! left for a person. With more than one request waiting for the Mission, a bare comment cannot
//! say which, so it decides none of them.

use std::time::Duration;

use guatiao::{FromValue, Schema, ToValue};

use crate::directorates::m::approvals::DEFAULT_APPROVAL_TTL;
use crate::directorates::m::promotion::CLEARANCE_KIND;
use crate::directorates::m::stewardship::HOLD_KIND;
use crate::directorates::registry::approvals::Ruling;
use crate::directorates::registry::backend::*;
use crate::directorates::registry::mission_issues::IssueLink;
use crate::directorates::registry::store::AuditRecord;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::directorates::registry::tracker::Reply;

/// Timeline kind of everything recorded here.
const TIMELINE_KIND: &str = "issue";

/// `[operation.ID.sources.NAME] decisions`: who may decide from the tracker.
#[derive(Debug, Clone, Default, PartialEq, Eq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub(crate) struct Decisions {
    /// The tracker's account IDs whose `/approve` and `/reject` comments decide green ink. Empty
    /// turns tracker decisions off, and such comments are ordinary comments.
    #[schema(title = "Approvers")]
    #[map(default)]
    pub approvers: Vec<String>,
    /// Seconds an approval given by comment covers its step; the same default as an approval
    /// given in the app when unset.
    #[schema(title = "An approval lasts", format = "seconds")]
    pub approval_secs: Option<u64>,
}

impl Decisions {
    /// Whether this instance reads decisions from comments at all.
    pub(crate) fn enabled(&self) -> bool {
        !self.approvers.is_empty()
    }
}

/// What a comment asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Approve,
    Reject(String),
}

/// The command a comment is, if it is exactly one: `/approve`, or `/reject` and a reason.
pub(crate) fn command_of(body: &str) -> Option<Command> {
    let body = body.trim();
    if body == "/approve" {
        return Some(Command::Approve);
    }
    let reason = body.strip_prefix("/reject")?;
    // `/rejected` and `/reject-all` are not it.
    if !reason.starts_with(char::is_whitespace) || reason.trim().is_empty() {
        return None;
    }
    Some(Command::Reject(reason.trim().to_owned()))
}

/// Why a command decided nothing.
fn refusal(reason: &str) -> String {
    format!("refused: {reason}")
}

fn clip(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Settles `command`, made in `reply` on the item `link` names, at `now`: decides the one
/// request waiting for the Mission when the author is listed, and records what happened either
/// way. The audit record comes first; if it cannot be written, nothing is decided.
pub(crate) fn settle(
    registry: &dyn RegistryBackend,
    (instance, decisions): (&str, &Decisions),
    link: &IssueLink,
    reply: &Reply,
    command: &Command,
    now: f64,
) {
    let actor = format!("tracker:{instance}:{}", reply.author_id);
    let item = format!("{instance}:{} comment {}", reply.item, reply.id);
    let record = |action: &str, target: String| AuditRecord {
        id: format!("tracker-decision-{}", uuid::Uuid::new_v4().simple()),
        actor: actor.clone(),
        action: action.to_owned(),
        operation: link.operation.clone(),
        resource: Some(format!("mission:{}", link.mission_id)),
        target,
        at: now,
    };
    let note = |action: &str, detail: String| {
        let event = TimelineEvent::new(TIMELINE_KIND, action, &link.operation, now)
            .mission(link.mission_id.clone())
            .detail("tracker", instance.to_owned())
            .detail("item", reply.item.clone())
            .detail("comment", reply.id.clone())
            .detail("author", reply.author_id.clone())
            .detail("detail", detail);
        let _ = registry.record_event(&event);
    };
    let refuse = |why: &str| {
        let why = refusal(why);
        if registry
            .record_audit(&record("green_ink.refused", format!("{item} {why}")))
            .is_err()
        {
            eprintln!(
                "ai6: a tracker decision on {item} was refused and its audit record not written"
            );
        }
        note("decision_refused", why);
    };
    if !decisions.approvers.contains(&reply.author_id) {
        refuse("the author is not a listed approver");
        return;
    }
    let waiting: Vec<_> = match registry.pending_approvals(&link.operation, now) {
        Ok(requests) => requests
            .into_iter()
            .filter(|request| request.mission.as_str() == link.mission_id)
            .collect(),
        Err(error) => {
            eprintln!(
                "ai6: green ink of {} not read for a tracker decision: {error}",
                link.operation
            );
            return;
        }
    };
    let [request] = waiting.as_slice() else {
        refuse(if waiting.is_empty() {
            "no request is waiting for this Mission"
        } else {
            "more than one request is waiting, so a comment cannot say which"
        });
        return;
    };
    if matches!(request.scope.kind.as_str(), CLEARANCE_KIND | HOLD_KIND) {
        refuse("that request needs a person with agent.configure");
        return;
    }
    let (action, ruling) = match command {
        Command::Approve => {
            let lasts = decisions
                .approval_secs
                .map_or(DEFAULT_APPROVAL_TTL, Duration::from_secs);
            (
                "green_ink.approved",
                Ruling::approve(actor.clone(), now, now + lasts.as_secs_f64()),
            )
        }
        Command::Reject(reason) => {
            let bytes = registry.limits().approvals.note_bytes.or_max();
            (
                "green_ink.rejected",
                Ruling::reject(actor.clone(), now, Some(clip(reason, bytes))),
            )
        }
    };
    let target = format!(
        "{} {} {} {item}",
        request.id, request.scope.kind, request.scope.target
    );
    if registry.record_audit(&record(action, target)).is_err() {
        eprintln!(
            "ai6: a tracker decision on {item} was not made: its audit record was not written"
        );
        return;
    }
    match registry.decide_approval(&link.operation, &request.id, &ruling) {
        Ok(_) => note(
            "decision",
            format!("{} {}", ruling.decision.as_str(), request.id),
        ),
        Err(error) => {
            eprintln!("ai6: tracker decision on {item} not applied: {error}");
            note("decision_refused", refusal(&error.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_exact_command_is_one() {
        assert_eq!(command_of("/approve"), Some(Command::Approve));
        assert_eq!(command_of("  /approve\n"), Some(Command::Approve));
        assert_eq!(
            command_of("/reject needs a rebase"),
            Some(Command::Reject("needs a rebase".to_owned()))
        );
        for other in [
            "/approve please",
            "approve",
            "/approved",
            "/reject",
            "/reject   ",
            "/rejected nope",
            "LGTM /approve",
            "/Approve",
            "",
        ] {
            assert_eq!(command_of(other), None, "{other:?}");
        }
    }
}
