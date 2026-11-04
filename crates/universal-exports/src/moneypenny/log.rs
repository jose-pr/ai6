//! Moneypenny's durable conversation log: every entry of every conversation, kept in the
//! Registry ([`ai6::AssistantLogStore`]) by a writer of its own so a conversation never waits
//! on the database, and dropped by `[assistant.gateway] log_age` and `log_bytes`.
//!
//! Besides what the app shows, the log keeps each tool call she made with its arguments
//! (`tool_call`) and the size and status of its answer (`tool_result`), and each change of a
//! proposal's status (`proposal_status`). The person's access token and her tool token never
//! reach it: they are not part of an entry, and are taken out of any text that carries them.

use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ai6::directorates::registry::backend::*;
use ai6::{AssistantGatewayLimits, AssistantLogEntry};
use serde_json::Value;

use super::store::{Bounds, Conversation, Entry, Role};

/// A tool call she made: `text` names the tool, `data` is `{"arguments": ...}`.
pub(crate) const TOOL_CALL: &str = "tool_call";
/// The answer to a tool call: `text` names the tool, `data` is `{"bytes", "error", "ms"}`.
pub(crate) const TOOL_RESULT: &str = "tool_result";
/// A proposal's new status: `data` is the proposal as the app shows it.
pub(crate) const PROPOSAL_STATUS: &str = "proposal_status";

/// What a secret is replaced with.
const REDACTED: &str = "[redacted]";

/// Where a conversation's entries go to be kept.
#[derive(Clone)]
pub(crate) struct Sink(mpsc::Sender<AssistantLogEntry>);

impl std::fmt::Debug for Sink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sink")
    }
}

impl Sink {
    /// Queues `entry` for the writer; one the writer can no longer take is dropped.
    pub(crate) fn send(&self, entry: AssistantLogEntry) {
        let _ = self.0.send(entry);
    }
}

/// Starts the writer: it keeps each entry in `registry` in the order it was sent, and every
/// `log_prune_every` drops what `limits` keeps no longer. It stops when every [`Sink`] is gone.
pub(crate) fn start(registry: Arc<dyn RegistryBackend>, limits: &AssistantGatewayLimits) -> Sink {
    let (sender, receiver) = mpsc::channel::<AssistantLogEntry>();
    let (age, bytes, every) = (limits.log_age, limits.log_bytes, limits.log_prune_every.0);
    let spawned = std::thread::Builder::new()
        .name("moneypenny-log".to_owned())
        .spawn(move || {
            let mut pruned: Option<Instant> = None;
            loop {
                if pruned.is_none_or(|at| at.elapsed() >= every) {
                    let before = age.get().map(|age| super::now() - age.as_secs_f64());
                    if let Err(error) = registry.prune_assistant_log(before, bytes) {
                        eprintln!("universal-exports: Moneypenny's log was not trimmed: {error}");
                    }
                    pruned = Some(Instant::now());
                }
                match receiver.recv_timeout(every.max(Duration::from_millis(1))) {
                    Ok(entry) => {
                        if let Err(error) = registry.log_assistant_entry(&entry) {
                            eprintln!(
                                "universal-exports: an entry of Moneypenny's conversation {} was \
                                 not logged: {error}",
                                entry.conversation
                            );
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("universal-exports: Moneypenny's log writer did not start: {error}");
    }
    Sink(sender)
}

/// `text` with each of `secrets` (at least 16 bytes long, so that no short word is taken for
/// one) replaced.
pub(crate) fn redact(text: &str, secrets: &[String]) -> String {
    let mut text = text.to_owned();
    for secret in secrets.iter().filter(|secret| secret.len() >= 16) {
        if text.contains(secret.as_str()) {
            text = text.replace(secret.as_str(), REDACTED);
        }
    }
    text
}

/// `value` with [`redact`] applied to every string in it, keys included.
pub(crate) fn redact_value(value: &Value, secrets: &[String]) -> Value {
    match value {
        Value::String(text) => Value::String(redact(text, secrets)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_value(item, secrets))
                .collect(),
        ),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, item)| (redact(key, secrets), redact_value(item, secrets)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A conversation as its log left it, for a gateway that no longer holds it: what the app
/// shows, within `bounds`, and the Station, provider Session and route of its last turn, so
/// its next message resumes there. A proposal still waiting is shown as expired: the gateway
/// that held it is gone.
pub(crate) fn rebuild(
    rows: &[AssistantLogEntry],
    bearer: &str,
    bounds: Bounds,
    buffer: usize,
) -> Option<Conversation> {
    let first = rows.first()?;
    let mut conversation = Conversation::new(
        first.conversation.clone(),
        first.person.clone(),
        bearer.to_owned(),
        first.at,
        buffer,
    );
    for row in rows {
        if let Some(turn) = &row.turn {
            if turn.station.is_some() {
                conversation.station.clone_from(&turn.station);
            }
            if turn.session.is_some() {
                conversation.session_id.clone_from(&turn.session);
                conversation.route.clone_from(&turn.route);
            }
            if turn.account.is_some() {
                conversation.account.clone_from(&turn.account);
            }
            if turn.model.is_some() {
                conversation.model.clone_from(&turn.model);
            }
        }
        if row.kind == PROPOSAL_STATUS {
            let id = row.data.as_ref().and_then(|data| data.get("id"));
            if let Some(entry) = conversation.entries.iter_mut().rev().find(|entry| {
                entry.role == Role::Proposal
                    && entry.data.as_ref().and_then(|data| data.get("id")) == id
            }) {
                entry.data.clone_from(&row.data);
            }
            continue;
        }
        let Ok(role) = serde_json::from_value::<Role>(Value::String(row.kind.clone())) else {
            continue;
        };
        conversation.push(
            Entry {
                role,
                text: row.text.clone(),
                at: row.at,
                data: row.data.clone(),
            },
            bounds,
        );
    }
    for entry in &mut conversation.entries {
        if entry.role == Role::Proposal
            && let Some(data) = entry.data.as_mut()
            && data.get("status").and_then(Value::as_str) == Some("pending")
        {
            data["status"] = Value::from("expired");
        }
    }
    Some(conversation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_taken_out_wherever_it_appears_and_short_words_are_left() {
        let secrets = vec!["0123456789abcdef0123".to_owned(), "short".to_owned()];
        let value = serde_json::json!({
            "query": "token 0123456789abcdef0123 here",
            "list": ["0123456789abcdef0123", 7],
            "0123456789abcdef0123": "short words stay",
        });
        let redacted = redact_value(&value, &secrets).to_string();
        assert!(!redacted.contains("0123456789abcdef0123"), "{redacted}");
        assert!(redacted.contains("short words stay"));
        assert_eq!(redacted.matches(REDACTED).count(), 3);
    }

    fn row(kind: &str, text: &str, at: f64) -> AssistantLogEntry {
        AssistantLogEntry::new("c-1", "eve", kind, text, at)
    }

    #[test]
    fn a_conversation_is_rebuilt_from_its_log_with_its_last_turn() {
        let mut turn = ai6::AssistantTurn::default();
        turn.station = Some("hq".to_owned());
        turn.session = Some("provider-1".to_owned());
        turn.route = Some(ai6::RouteRef::new("acp", None, Some("team".to_owned())));
        let proposal = serde_json::json!({"id": "proposal-1", "status": "pending"});
        let rows = vec![
            row("person", "Hello", 1.0),
            row(TOOL_CALL, "list_missions", 2.0).with_turn(turn.clone()),
            row("moneypenny", "Hi.", 3.0).with_turn(turn),
            row("proposal", "", 4.0).with_data(proposal),
            row(PROPOSAL_STATUS, "", 5.0)
                .with_data(serde_json::json!({"id": "proposal-1", "status": "confirming"})),
            row("proposal", "", 6.0)
                .with_data(serde_json::json!({"id": "proposal-2", "status": "pending"})),
        ];
        let bounds = Bounds {
            entries: usize::MAX,
            bytes: usize::MAX,
        };
        let conversation = rebuild(&rows, "token", bounds, 8).unwrap();
        let roles: Vec<Role> = conversation
            .entries
            .iter()
            .map(|entry| entry.role)
            .collect();
        assert_eq!(
            roles,
            [
                Role::Person,
                Role::Moneypenny,
                Role::Proposal,
                Role::Proposal
            ]
        );
        assert_eq!(conversation.station.as_deref(), Some("hq"));
        assert_eq!(conversation.session_id.as_deref(), Some("provider-1"));
        assert_eq!(conversation.person_id, "eve");
        let statuses: Vec<&str> = conversation
            .entries
            .iter()
            .skip(2)
            .map(|entry| entry.data.as_ref().unwrap()["status"].as_str().unwrap())
            .collect();
        assert_eq!(statuses, ["confirming", "expired"]);
        assert!(rebuild(&[], "token", bounds, 8).is_none());
    }
}
