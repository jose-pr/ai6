//! Moneypenny's Cables, as Universal Exports keeps them in line.
//!
//! The Station serves her Cable tools; Universal Exports decides what they may do. Each job
//! carries the Operations where the person holds `cable.send` or `cable.read` (at most
//! [`ai6::MAX_ASSISTANT_CABLE_GRANTS`]) and whether the conversation is exposed
//! (`Conversation.exposed`, the one record of it). A Station asks again before each send
//! (`GET /v1/station/assistant/exposure`), so a send in the same turn as a read waits too.
//! An exposed send reaches the app as a proposal: the person confirms it (audited
//! `moneypenny.confirmed`, then sent by the Station that holds it) or cancels it. Every Cable
//! sent for the person is audited `moneypenny.cable.sent`, with the person as actor.

use std::collections::BTreeSet;
use std::time::Duration;

use ai6::{
    AssistantCable, AssistantCableDecision, AssistantCableGrant, AssistantEvent, AuditRecord,
    AuthorizationRequest, EVENT_CABLE_PROPOSAL, EVENT_CABLE_SENT, MAX_ASSISTANT_CABLE_GRANTS,
};
use axum::http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

use super::actions::{self, CableHold, Draft, Proposal, Scope};
use super::store::{Bounds, Conversation, Entry, Role};
use super::tools::Call;
use super::{Assistant, Control, audit, control_job, digest, expire_proposals, now};
use crate::{Gateway, authorize_resource_action};

/// What to do once the desk lock is released: audit records to write and notes to give
/// Moneypenny with a message of their own (conversation, person, note).
#[derive(Default)]
pub(super) struct After {
    audits: Vec<AuditRecord>,
    notes: Vec<(String, String, String)>,
}

/// The Operations where `person` may send or read Cables: those the Stations check in with,
/// at most [`MAX_ASSISTANT_CABLE_GRANTS`], in order. Empty when the gateway has no Registry or
/// authorizer.
pub(super) async fn grants(gateway: &Gateway, person: &ai6::Person) -> Vec<AssistantCableGrant> {
    let (Some(registry), Some(access)) = (gateway.registry.clone(), gateway.access.as_ref()) else {
        return Vec::new();
    };
    let (authorizer, person) = (access.authorizer.clone(), person.clone());
    tokio::task::spawn_blocking(move || {
        let operations: BTreeSet<String> = registry
            .stations(Duration::MAX)
            .ok()?
            .iter()
            .filter_map(|station| station.info().get("operations")?.as_array().cloned())
            .flatten()
            .filter_map(|operation| operation.as_str().map(str::to_owned))
            .filter(|operation| ai6::directorates::section_00::assistant::valid_identity(operation))
            .collect();
        let mut grants = Vec::new();
        for operation in operations {
            let allowed = |action: &str| {
                authorizer
                    .authorize(AuthorizationRequest {
                        person: person.clone(),
                        action: action.to_owned(),
                        resource: format!("operation:{operation}"),
                    })
                    .is_ok_and(|decision| decision.allowed)
            };
            let (send, read) = (allowed("cable.send"), allowed("cable.read"));
            if send || read {
                grants.push(AssistantCableGrant {
                    operation,
                    send,
                    read,
                });
            }
            if grants.len() == MAX_ASSISTANT_CABLE_GRANTS {
                break;
            }
        }
        Some(grants)
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// Records one Cable event of a Station's report in the conversation.
pub(super) fn apply(
    conversation: &mut Conversation,
    event: &AssistantEvent,
    station: &str,
    bounds: Bounds,
    after: &mut After,
) {
    match event.kind.as_str() {
        EVENT_CABLE_PROPOSAL => {
            if let Some(cable) = &event.cable {
                propose(conversation, cable, station, bounds);
            }
        }
        EVENT_CABLE_SENT => {
            if let Some(cable) = &event.cable {
                sent(conversation, cable, after);
            }
        }
        _ => {}
    }
}

/// A Cable the Station holds becomes a proposal the person confirms or cancels.
fn propose(conversation: &mut Conversation, cable: &AssistantCable, station: &str, bounds: Bounds) {
    let Some(body) = cable.body.clone() else {
        return;
    };
    let valid = ai6::directorates::section_00::assistant::valid_identity;
    let shown = json!({"operation": cable.operation, "body": body});
    if !valid(&cable.id)
        || !valid(&cable.operation)
        || !valid(&cable.target)
        || !valid(&cable.digest)
        || body.len() > ai6::directorates::registry::cables::MAX_CABLE_BODY_BYTES
        || actions::hidden_characters(&shown, "cable").is_err()
        || actions::hidden_characters(&Value::String(cable.target.clone()), "target").is_err()
    {
        conversation.push(
            Entry {
                role: Role::Error,
                text: "Moneypenny asked to send a Cable that cannot be shown as it is; it was \
                       not sent."
                    .to_owned(),
                at: now(),
                data: None,
            },
            bounds,
        );
        return;
    }
    expire_proposals(conversation);
    while conversation.proposals.len() >= actions::MAX_PROPOSALS {
        let finished = conversation
            .proposals
            .iter()
            .position(|proposal| proposal.status != "pending")
            .unwrap_or(0);
        conversation.proposals.remove(finished);
    }
    let reply = cable.target.starts_with("reply:");
    let summary = format!(
        "{} {} in Operation {}, held because Moneypenny has read text from outside AI6",
        if reply { "Reply" } else { "Send a Cable to" },
        if reply {
            format!("to Cable {}", cable.target.trim_start_matches("reply:"))
        } else {
            cable.target.clone()
        },
        cable.operation
    );
    let proposal = Proposal {
        id: format!("proposal-{}", Uuid::new_v4().simple()),
        tool: if reply {
            "reply_to_cable"
        } else {
            "send_cable"
        },
        draft: Draft {
            call: Call {
                method: Method::from_bytes(b"CABLE").unwrap_or(Method::POST),
                path: cable.target.clone(),
                body: Some(shown),
            },
            scope: Scope::Operation(cable.operation.clone()),
            action: "cable.send",
            summary: summary.clone(),
        },
        status: "pending",
        created_at: now(),
        session: digest(&conversation.bearer),
        result: None,
        work: None,
        cable: Some(CableHold {
            held_id: cable.id.clone(),
            digest: cable.digest.clone(),
            station: station.to_owned(),
        }),
    };
    conversation.push(
        Entry {
            role: Role::Proposal,
            text: summary,
            at: proposal.created_at,
            data: Some(proposal.shown()),
        },
        bounds,
    );
    conversation.proposals.push(proposal);
}

/// A Cable left for the person: audited, and the proposal it answers is done.
fn sent(conversation: &mut Conversation, cable: &AssistantCable, after: &mut After) {
    let Some(cable_id) = cable.cable_id.clone() else {
        return;
    };
    after.audits.push(AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: conversation.person_id.clone(),
        action: "moneypenny.cable.sent".to_owned(),
        operation: cable.operation.clone(),
        resource: None,
        target: super::cut(&format!("{} {cable_id}", conversation.id), 512),
        at: now(),
    });
    let held = conversation.proposals.iter().find(|proposal| {
        proposal.status == "confirming"
            && proposal
                .cable
                .as_ref()
                .is_some_and(|hold| hold.held_id == cable.id)
    });
    if let Some(proposal) = held.map(|proposal| proposal.id.clone()) {
        finish(
            conversation,
            &proposal,
            Ok(format!("Sent as Cable {cable_id}.")),
            after,
        );
    }
}

/// Sets a Cable proposal's outcome and tells Moneypenny: with her next message when she is
/// answering, else with one of its own.
fn finish(
    conversation: &mut Conversation,
    proposal_id: &str,
    outcome: Result<String, String>,
    after: &mut After,
) {
    let Some(proposal) = conversation
        .proposals
        .iter_mut()
        .find(|proposal| proposal.id == proposal_id)
    else {
        return;
    };
    let (status, result) = match outcome {
        Ok(text) => ("done", text),
        Err(text) => ("failed", text),
    };
    proposal.status = status;
    proposal.result = Some(result.clone());
    let shown = proposal.clone();
    conversation.show_proposal(&shown);
    let note = format!(
        "[App: the person confirmed {} ({}). {result}]",
        shown.id, shown.draft.summary
    );
    if conversation.busy {
        conversation.notes.push(note);
    } else {
        after.notes.push((
            conversation.id.clone(),
            conversation.person_id.clone(),
            note,
        ));
    }
}

/// The Station's answer to a confirmed Cable: refused, the proposal failed with its reason;
/// taken, it is done (its `cable_sent` event, sent first, says as what).
pub(super) fn answered(
    conversation: &mut Conversation,
    proposal_id: &str,
    refused: Option<String>,
    after: &mut After,
) {
    let confirming = conversation
        .proposals
        .iter()
        .any(|proposal| proposal.id == proposal_id && proposal.status == "confirming");
    if !confirming {
        return;
    }
    let outcome = match refused {
        Some(reason) => Err(format!("Not sent: {}", super::or_unknown(&reason))),
        None => Ok("Sent.".to_owned()),
    };
    finish(conversation, proposal_id, outcome, after);
}

impl Assistant {
    /// Writes the audit records and gives Moneypenny the notes `after` collected.
    pub(super) async fn follow_up(&self, gateway: &Gateway, after: After) {
        if let Some(registry) = gateway.registry.clone()
            && !after.audits.is_empty()
        {
            let audits = after.audits;
            let written = tokio::task::spawn_blocking(move || {
                audits
                    .iter()
                    .try_for_each(|record| registry.record_audit(record))
            })
            .await;
            if !matches!(written, Ok(Ok(()))) {
                eprintln!("universal-exports: an audit record of Moneypenny's Cables was lost");
            }
        }
        for (conversation, person, note) in after.notes {
            let _ = self
                .queue(gateway, &conversation, &person, &note, None)
                .await;
        }
    }

    /// Runs a confirmed Cable proposal: checks the person still holds `cable.send`, audits
    /// the confirmation, then asks the Station that holds the Cable to send it.
    pub(super) async fn confirm_cable(
        &self,
        gateway: &Gateway,
        conversation_id: &str,
        person: &ai6::Person,
        proposal: &Proposal,
        hold: CableHold,
    ) {
        let resource = proposal.draft.scope.resource();
        let failure =
            if authorize_resource_action(gateway, person.clone(), resource.clone(), "cable.send")
                .await
                .is_err()
            {
                Some(format!(
                    "Refused: the person may not cable.send on {resource}, so nothing was sent."
                ))
            } else {
                let target = format!(
                    "{conversation_id} {} {} {}",
                    proposal.id, proposal.tool, proposal.draft.call.path
                );
                let operation = match &proposal.draft.scope {
                    Scope::Operation(operation) => operation.clone(),
                    Scope::Resource(_) => String::new(),
                };
                // Recorded before it is sent: a confirmation that cannot be audited is not.
                audit(
                    gateway,
                    person,
                    "moneypenny.confirmed",
                    operation,
                    None,
                    &target,
                )
                .await
                .err()
            };
        let mut after = After::default();
        {
            let mut desk = self.desk();
            let agent_id = self.config.agent_id.clone();
            let Some(conversation) = desk.store.owned(conversation_id, &person.id) else {
                return;
            };
            if let Some(reason) = failure {
                finish(conversation, &proposal.id, Err(reason), &mut after);
            } else {
                let control = Control::CableConfirm(proposal.id.clone());
                let mut job = control_job(conversation, &agent_id, &control, None);
                job.cable_confirm = Some(AssistantCableDecision {
                    id: hold.held_id,
                    digest: hold.digest,
                });
                desk.controlling.insert(job.job_id.clone(), control);
                desk.controls.push((hold.station, job));
            }
        }
        self.ready.notify_waiters();
        self.follow_up(gateway, after).await;
    }

    /// Asks the Station holding a cancelled Cable proposal's Cable to drop it.
    pub(super) fn discard_cable(&self, conversation_id: &str, proposal_id: &str, hold: CableHold) {
        {
            let mut desk = self.desk();
            let agent_id = self.config.agent_id.clone();
            let Some(conversation) = desk.store.conversations.get(conversation_id) else {
                return;
            };
            let control = Control::CableDiscard(proposal_id.to_owned());
            let mut job = control_job(conversation, &agent_id, &control, None);
            job.cable_discard = Some(AssistantCableDecision {
                id: hold.held_id,
                digest: hold.digest,
            });
            desk.controlling.insert(job.job_id.clone(), control);
            desk.controls.push((hold.station, job));
        }
        self.ready.notify_waiters();
    }
}
