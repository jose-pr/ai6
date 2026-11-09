//! What one event means on the wire. No I/O, no clock, no thread.
//!
//! A provider's text is untrusted when it reaches the client, so it is only ever data here: a stop reason,
//! an error code and a tool's kind and identifier never come from it. Sizes are what a part takes as JSON,
//! escapes included, since that is what a line transport counts.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Value, json};

use crate::kind::{
    AgentEvent, BLOCKED_EVENT, CANCELLED_EVENT, COMPLETED_EVENT, DENIED_EVENT, FAILED_EVENT,
    LIMIT_EVENT, TEXT_EVENT, TOOL_EVENT, TURN_CANCELLED,
};

/// The most bytes one byte of [`AgentEvent::data`] takes as JSON.
const JSON_BYTE: usize = 4;

/// What a line holds besides its event, text or titles: the JSON-RPC frame, the keys, and a session
/// identifier of at most 4 KiB.
pub(super) const ENVELOPE: usize = 5 * 1024;

/// The text an event that does not fit a line is replaced by.
const TOO_LARGE: &str = "the event was larger than this agent passes on";

/// How a turn ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Ending {
    /// The `session/prompt` answer's stop reason.
    Stop(&'static str),
    /// The `session/prompt` fails with this text.
    Failure(String),
}

/// The sizes the settings allow.
#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub(super) update: usize,
    pub(super) label: usize,
}

/// The ending an event means, if any. The stop reasons are ACP's own.
pub(super) fn ending(event: &AgentEvent, max_label_bytes: usize) -> Option<Ending> {
    match event.kind.as_str() {
        COMPLETED_EVENT => Some(Ending::Stop("end_turn")),
        LIMIT_EVENT => Some(Ending::Stop("refusal")),
        CANCELLED_EVENT => Some(Ending::Stop("cancelled")),
        BLOCKED_EVENT if event.text.as_deref() == Some(TURN_CANCELLED) => {
            Some(Ending::Stop("cancelled"))
        }
        FAILED_EVENT => Some(Ending::Failure(label(
            event.text.as_deref().unwrap_or("the agent failed"),
            max_label_bytes,
        ))),
        _ => None,
    }
}

/// What `c` takes inside a JSON string.
const fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// The longest start of `text` that takes at most `limit` bytes as JSON.
fn cut_json(text: &str, limit: usize) -> &str {
    let mut taken = 0;
    for (at, c) in text.char_indices() {
        taken += json_len(c);
        if taken > limit {
            return &text[..at];
        }
    }
    text
}

/// A format character (Unicode category Cf): bidirectional overrides and isolates, zero-width characters
/// and the like, which change how text around them is shown.
const fn is_format(c: char) -> bool {
    matches!(c as u32,
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E
        | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF
        | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// A provider's words as a label: a failure. No control character but a line break or a tab, no format
/// character, and at most `limit` bytes as JSON.
pub(super) fn label(text: &str, limit: usize) -> String {
    let clean: String = text
        .chars()
        .filter(|&c| (!c.is_control() || matches!(c, '\n' | '\t')) && !is_format(c))
        .collect();
    cut_json(&clean, limit).to_string()
}

/// A tool's name or title, or an identity: a label on one line.
pub(super) fn title(text: &str, limit: usize) -> String {
    label(&text.replace(['\n', '\t'], " "), limit)
}

fn update(session: &str, update: &Value) -> Value {
    json!({"sessionId": session, "update": update})
}

/// A `tool_call` for a tool the provider names. Its kind is `other`, since a name says nothing the client
/// could act on, and its identifier is ours.
fn tool_call(session: &str, id: &str, title: &str, name: &str, status: &str) -> Value {
    update(
        session,
        &json!({
            "sessionUpdate": "tool_call",
            "toolCallId": id,
            "title": title,
            "name": name,
            "kind": "other",
            "status": status,
        }),
    )
}

/// The update ending a tool call the turn started.
pub(super) fn tool_finished(session: &str, id: &str, status: &str) -> Value {
    update(
        session,
        &json!({"sessionUpdate": "tool_call_update", "toolCallId": id, "status": status}),
    )
}

/// `text` as `agent_message_chunk` notifications, each text at most `limit` bytes as JSON.
fn message_chunks(session: &str, text: &str, limit: usize) -> Vec<Value> {
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let mut part = cut_json(rest, limit);
        if part.is_empty() {
            // A limit below one character still has to make progress.
            let first = rest.chars().next().map_or(rest.len(), char::len_utf8);
            part = &rest[..first];
        }
        rest = &rest[part.len()..];
        chunks.push(update(
            session,
            &json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": part},
            }),
        ));
    }
    chunks
}

/// The `usage_update` for an event reporting a cost. ACP has no field for tokens alone.
fn usage(session: &str, event: &AgentEvent) -> Option<Value> {
    let usage = event
        .usage
        .as_ref()
        .filter(|usage| usage.cost_micro_usd > 0)?;
    let used = usage
        .input
        .saturating_add(usage.output)
        .saturating_add(usage.cache_read)
        .saturating_add(usage.cache_write);
    #[allow(
        clippy::cast_precision_loss,
        reason = "a cost is a display figure, and a dollar amount fits a double's exact range"
    )]
    let amount = usage.cost_micro_usd as f64 / 1_000_000.0;
    Some(update(
        session,
        &json!({
            "sessionUpdate": "usage_update",
            "used": used,
            "size": 0,
            "cost": {"amount": amount, "currency": "USD"},
        }),
    ))
}

/// The tools a `denied` event names: its text, split as its documentation says.
fn denied_tools(event: &AgentEvent) -> Vec<&str> {
    if event.kind != DENIED_EVENT {
        return Vec::new();
    }
    event
        .text
        .as_deref()
        .unwrap_or_default()
        .split(", ")
        .filter(|name| !name.is_empty())
        .collect()
}

/// The titles to announce for an event's tools, as `(title, name)`: a `tool` event's text titles its one
/// tool.
fn tools(event: &AgentEvent, limit: usize) -> Vec<(String, String)> {
    let shared = (event.kind == TOOL_EVENT && event.tools.len() == 1)
        .then_some(event.text.as_deref())
        .flatten();
    event
        .tools
        .iter()
        .map(|name| {
            let name = title(name, limit);
            let shown = shared.map_or_else(|| name.clone(), |text| title(text, limit));
            (shown, name)
        })
        .collect()
}

/// Whether an event has parts no ACP update carries, which then go as [`EVENT_NOTIFICATION`].
///
/// [`EVENT_NOTIFICATION`]: super::EVENT_NOTIFICATION
fn needs_extension(event: &AgentEvent) -> bool {
    if event.limit.is_some() || event.usage.is_some() || !event.quota.is_empty() {
        return true;
    }
    match event.kind.as_str() {
        TEXT_EVENT | TOOL_EVENT | COMPLETED_EVENT | FAILED_EVENT | CANCELLED_EVENT
        | DENIED_EVENT => false,
        BLOCKED_EVENT => event.text.as_deref() != Some(TURN_CANCELLED),
        _ => true,
    }
}

/// `event` for [`EVENT_NOTIFICATION`], each part bounded: names as titles, text cut, and bytes that do not
/// fit left out.
///
/// [`EVENT_NOTIFICATION`]: super::EVENT_NOTIFICATION
fn bounded(event: &AgentEvent, limits: Limits) -> AgentEvent {
    let mut event = event.clone();
    event.kind = title(&event.kind, limits.label);
    if let Some(text) = &mut event.text {
        text.truncate(cut_json(text, limits.update).len());
    }
    if event
        .data
        .as_ref()
        .is_some_and(|data| data.len() > limits.update / JSON_BYTE)
    {
        event.data = None;
    }
    for tool in &mut event.tools {
        *tool = title(tool, limits.label);
    }
    for quota in &mut event.quota {
        quota.window = title(&quota.window, limits.label);
        if let Some(status) = &mut quota.status {
            *status = title(status, limits.label);
        }
    }
    if let Some(limit) = &mut event.limit
        && let Some(window) = &mut limit.window
    {
        *window = title(window, limits.label);
    }
    event
}

/// What a notification takes as one JSON-RPC line: `{"jsonrpc":"2.0","method":..,"params":..}`.
pub(super) fn cost(method: &str, params: &Value) -> usize {
    let params = serde_json::to_string(params).map_or(0, |text| text.len());
    39 + method.len() + params
}

/// What one event sends, and the tool calls it opens.
pub(super) struct Outgoing {
    pub(super) messages: Vec<(&'static str, Value)>,
    pub(super) opened: Vec<String>,
}

/// Every notification `event` becomes. No line is longer than `limits.update + 2 * limits.label` and
/// [`ENVELOPE`]: an event for [`EVENT_NOTIFICATION`] that would be is replaced by its kind and a note.
///
/// [`EVENT_NOTIFICATION`]: super::EVENT_NOTIFICATION
pub(super) fn outgoing(
    session: &str,
    provider: Option<&str>,
    event: &AgentEvent,
    limits: Limits,
) -> Outgoing {
    let mut messages = Vec::new();
    let mut opened = Vec::new();
    if event.kind == TEXT_EVENT
        && let Some(text) = &event.text
    {
        for chunk in message_chunks(session, text, limits.update) {
            messages.push(("session/update", chunk));
        }
    }
    for (shown, name) in tools(event, limits.label) {
        let id = uuid::Uuid::new_v4().to_string();
        messages.push((
            "session/update",
            tool_call(session, &id, &shown, &name, "in_progress"),
        ));
        opened.push(id);
    }
    for name in denied_tools(event) {
        let id = uuid::Uuid::new_v4().to_string();
        let name = title(name, limits.label);
        messages.push((
            "session/update",
            tool_call(session, &id, &name, &name, "failed"),
        ));
    }
    if let Some(usage) = usage(session, event) {
        messages.push(("session/update", usage));
    }
    if needs_extension(event) {
        let line = |event: &AgentEvent| json!({"sessionId": session, "providerSessionId": provider, "event": event});
        let mut params = line(&bounded(event, limits));
        let longest = limits.update + 2 * limits.label + ENVELOPE;
        if cost(super::EVENT_NOTIFICATION, &params) > longest {
            let kind = title(&event.kind, limits.label);
            params = line(&AgentEvent::new(kind, Some(TOO_LARGE.to_string())));
        }
        messages.push((super::EVENT_NOTIFICATION, params));
    }
    Outgoing { messages, opened }
}

/// The prompt's text: text blocks, and each resource link as its address, so nothing the client sent goes
/// unmentioned. `None` for any other block, which this agent did not advertise taking.
pub(super) fn prompt_text(params: &Value) -> Option<String> {
    let blocks = params.get("prompt")?.as_array()?;
    let mut parts = Vec::with_capacity(blocks.len());
    for block in blocks {
        let part = match block.get("type").and_then(Value::as_str)? {
            "text" => block.get("text").and_then(Value::as_str)?,
            "resource_link" => block.get("uri").and_then(Value::as_str)?,
            _ => return None,
        };
        parts.push(part.to_string());
    }
    Some(parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, text: &str) -> AgentEvent {
        AgentEvent::new(kind, Some(text.to_string()))
    }

    #[test]
    fn a_stop_reason_comes_from_the_kind_alone() {
        assert_eq!(
            ending(&event(COMPLETED_EVENT, "max_tokens"), 100),
            Some(Ending::Stop("end_turn"))
        );
        assert_eq!(
            ending(&event(LIMIT_EVENT, "end_turn"), 100),
            Some(Ending::Stop("refusal"))
        );
        assert_eq!(
            ending(&event(BLOCKED_EVENT, TURN_CANCELLED), 100),
            Some(Ending::Stop("cancelled"))
        );
        assert_eq!(ending(&event(BLOCKED_EVENT, "refused"), 100), None);
        assert_eq!(ending(&event("end_turn", ""), 100), None);
    }

    #[test]
    fn a_label_loses_what_a_terminal_would_act_on_and_is_bounded() {
        assert_eq!(label("ok\u{1b}[2J\u{7}\nnext", 100), "ok[2J\nnext");
        assert_eq!(label("ééé", 3), "é");
        assert_eq!(label("a\u{202e}b\u{200b}c\u{feff}", 100), "abc");
        assert_eq!(label("\"\"\"", 4), "\"\"");
        assert_eq!(title("a\nb\tc", 100), "a b c");
    }

    #[test]
    fn text_splits_on_character_boundaries_and_keeps_every_byte() {
        let text = "é".repeat(10);
        let chunks = message_chunks("s", &text, 3);
        assert_eq!(chunks.len(), 10);
        let joined: String = chunks
            .iter()
            .map(|chunk| chunk["update"]["content"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(joined, text);
        assert_eq!(message_chunks("s", "é", 1).len(), 1);
        assert!(message_chunks("s", "", 3).is_empty());
    }

    #[test]
    fn a_cost_is_the_line_as_written() {
        let params = json!({"a": "\"x\""});
        let line = json!({"jsonrpc": "2.0", "method": "m", "params": params}).to_string();
        assert_eq!(cost("m", &params), line.len());
    }

    #[test]
    fn a_prompt_refuses_what_it_cannot_carry() {
        let params = json!({"prompt": [
            {"type": "text", "text": "read"},
            {"type": "resource_link", "uri": "file:///a", "name": "a"},
        ]});
        assert_eq!(prompt_text(&params).unwrap(), "read\n\nfile:///a");
        assert!(prompt_text(&json!({"prompt": [{"type": "image", "data": "x"}]})).is_none());
        assert!(prompt_text(&json!({})).is_none());
    }

    #[test]
    fn a_denied_event_names_tools_that_did_not_run() {
        assert_eq!(
            denied_tools(&event(DENIED_EVENT, "Bash, WebFetch")),
            ["Bash", "WebFetch"]
        );
        assert!(denied_tools(&event(TEXT_EVENT, "Bash, WebFetch")).is_empty());
    }

    #[test]
    fn a_kind_no_update_carries_goes_as_the_extension() {
        assert!(needs_extension(&event("notice", "x")));
        assert!(needs_extension(&event(BLOCKED_EVENT, "refused")));
        assert!(!needs_extension(&event(BLOCKED_EVENT, TURN_CANCELLED)));
        assert!(!needs_extension(&event(TEXT_EVENT, "x")));
    }
}
