//! Codex's sub-agent items (`collabAgentToolCall`, `subAgentActivity`) as child agents.
//!
//! The app server reports a child by its thread id: the spawning call names it and a later call (`wait`, `closeAgent`)
//! carries its last known status. Codex does not send the child's own messages in the parent's thread, so a child here
//! has a task, a state and a result, and no progress.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use coqui_acp_harness::event::{Child, ChildState, Event, ToolCall, ToolKind, ToolStatus};
use serde_json::Value;

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

fn state(status: &str) -> Option<ChildState> {
    match status {
        "pendingInit" => Some(ChildState::Starting),
        "running" => Some(ChildState::Working),
        "completed" => Some(ChildState::Completed),
        "errored" => Some(ChildState::Failed),
        "interrupted" | "shutdown" => Some(ChildState::Cancelled),
        _ => None,
    }
}

/// A `collabAgentToolCall` item: the call, and a report for each child it names or carries the status of.
pub(crate) fn collab(
    item: &Value,
    id: &str,
    done: bool,
    max_title: usize,
    max_text: usize,
) -> Vec<Event> {
    let tool = text(item, "/tool").unwrap_or("agent");
    let mut call = ToolCall::new(id);
    call.name = Some(tool.to_string());
    call.kind = Some(ToolKind::Think);
    call.title = Some(
        match tool {
            "spawnAgent" => "Start a subagent",
            "wait" => "Wait for subagents",
            "closeAgent" => "Close a subagent",
            "interruptAgent" => "Interrupt a subagent",
            "listAgents" => "List subagents",
            _ => "Message a subagent",
        }
        .to_string(),
    );
    call.input = Some(serde_json::json!({
        "tool": tool,
        "prompt": item.get("prompt"),
        "model": item.get("model"),
    }));
    call.status = Some(match text(item, "/status") {
        Some("completed") => ToolStatus::Completed,
        Some("failed" | "interrupted") => ToolStatus::Failed,
        _ if done => ToolStatus::Completed,
        _ => ToolStatus::InProgress,
    });
    let mut events = vec![Event::Tool(call)];
    let receivers: Vec<&str> = item
        .get("receiverThreadIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let states = item.get("agentsStates").and_then(Value::as_object);
    let mut named: Vec<&str> = receivers.clone();
    for key in states.into_iter().flat_map(|states| states.keys()) {
        if !named.contains(&key.as_str()) {
            named.push(key);
        }
    }
    if named.is_empty() && tool == "spawnAgent" {
        named.push(id);
    }
    for thread in named {
        let mut child = Child::new(thread);
        child.parent_tool_call = Some(id.to_string());
        let reported = states.and_then(|states| states.get(thread));
        child.state = reported
            .and_then(|reported| text(reported, "/status"))
            .and_then(state);
        if tool == "spawnAgent" {
            child.task = text(item, "/prompt").map(|task| cut(task, max_text));
            child.model = text(item, "/model").map(str::to_string);
            child.title = text(item, "/prompt").map(|task| first_line(task, max_title));
            // The call coming back says Codex took the request, not that the child has run.
            if child.state.is_none() && !done {
                child.state = Some(ChildState::Starting);
            }
        }
        if child.state == Some(ChildState::Completed)
            && let Some(said) = reported.and_then(|reported| text(reported, "/message"))
        {
            child.result = Some(cut(said, max_text));
        }
        if child.state == Some(ChildState::Failed)
            && let Some(said) = reported.and_then(|reported| text(reported, "/message"))
        {
            child.result = Some(cut(said, max_text));
        }
        events.push(Event::Child(child));
    }
    events
}

/// A `subAgentActivity` item: a child started, answered, was interrupted or finished.
pub(crate) fn activity(item: &Value, max_title: usize) -> Vec<Event> {
    let Some(thread) = text(item, "/agentThreadId") else {
        return Vec::new();
    };
    let mut child = Child::new(thread);
    child.title = text(item, "/agentPath").map(|path| first_line(path, max_title));
    child.model = text(item, "/model").map(str::to_string);
    child.state = match text(item, "/kind") {
        Some("started") => Some(ChildState::Working),
        Some("completed") => Some(ChildState::Completed),
        Some("interrupted") => Some(ChildState::Cancelled),
        _ => None,
    };
    vec![Event::Child(child)]
}

fn first_line(text: &str, max: usize) -> String {
    let first = text.lines().next().unwrap_or_default();
    cut(first, max)
}

fn cut(text: &str, max: usize) -> String {
    coqui_acp_harness::lines::cut(text, max).to_string()
}
