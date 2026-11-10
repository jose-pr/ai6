//! Events as ACP `session/update`s, in version 1's shapes or version 2's.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use base64::Engine as _;
use coqui_acp_api::LimitHit;
use serde_json::{Map, Value, json};

use super::{ConfigOption, LIMIT_META_KEY, PLAN_ID};
use crate::event::{
    Command, Content, Diff, Event, FileChange, PlanEntry, Terminal, ToolCall, ToolContent,
    ToolStatus, Usage,
};

/// What a session's updates depend on: the tool calls announced, for version 1, and the turn's last usage.
#[derive(Debug, Default)]
pub(crate) struct Shapes {
    pub(crate) version: u8,
    max_tools: usize,
    /// Tool calls announced, oldest first, with the content version 1 sends whole each time.
    tools: VecDeque<String>,
    content: BTreeMap<String, Vec<Value>>,
    /// The bytes of each tool call's content in `content`, and how much is kept of any.
    content_bytes: BTreeMap<String, usize>,
    max_content_bytes: usize,
    max_content_items: usize,
    /// The tool call that shows each terminal, for version 1, which has no terminals of the agent's.
    terminals: BTreeMap<String, String>,
    /// The last usage of the running turn, which version 2 gives with its `idle`.
    pub(crate) usage: Option<Usage>,
}

impl Shapes {
    pub(crate) fn new(version: u8, max_tools: usize) -> Self {
        Self {
            version,
            max_tools,
            max_content_bytes: usize::MAX,
            max_content_items: usize::MAX,
            ..Self::default()
        }
    }

    /// With bounds on the content version 1 sends whole each time.
    pub(crate) fn bounded(
        version: u8,
        max_tools: usize,
        max_content_bytes: usize,
        max_content_items: usize,
    ) -> Self {
        Self {
            max_content_bytes,
            max_content_items,
            ..Self::new(version, max_tools)
        }
    }

    /// The updates `event` becomes; none for what this version has no update for.
    pub(crate) fn updates(&mut self, event: &Event) -> Vec<Value> {
        let v2 = self.version >= 2;
        match event {
            Event::UserChunk { message, content } => vec![chunk(
                "user_message_chunk",
                message,
                content_block(content),
                v2,
            )],
            Event::UserEnd { message } if v2 => vec![ended("user_message", message)],
            Event::MessageChunk { message, text } => {
                vec![chunk("agent_message_chunk", message, text_block(text), v2)]
            }
            Event::MessageEnd { message } if v2 => vec![ended("agent_message", message)],
            Event::ThoughtChunk { message, text } => {
                vec![chunk("agent_thought_chunk", message, text_block(text), v2)]
            }
            Event::ThoughtEnd { message } if v2 => vec![ended("agent_thought", message)],
            Event::Tool(call) => self.tool(call),
            Event::ToolChunk { id, content } => self.tool_chunk(id, content),
            Event::Terminal(terminal) => self.terminal(terminal),
            Event::Plan(entries) => vec![plan(entries, v2)],
            Event::Usage(usage) if usage.elsewhere => Vec::new(),
            Event::Usage(usage) => {
                self.usage = Some(usage.clone());
                usage_update(usage).into_iter().collect()
            }
            Event::Commands(commands) => vec![available_commands(commands, v2)],
            Event::Options(options) => vec![json!({"sessionUpdate": "config_option_update",
                "configOptions": config_options(options, self.version)})],
            Event::Title(title) => {
                vec![json!({"sessionUpdate": "session_info_update", "title": title})]
            }
            Event::Notice { severity, title } if v2 => {
                vec![
                    json!({"sessionUpdate": "notice", "severity": severity.as_str(), "title": title}),
                ]
            }
            Event::Compaction { id, summary } if v2 => {
                let mut updates = Vec::new();
                if let Some(summary) = summary {
                    updates.push(
                        json!({"sessionUpdate": "compaction_summary_chunk", "compactionId": id,
                        "content": {"type": "text", "text": summary}}),
                    );
                }
                updates.push(json!({"sessionUpdate": "compaction_update", "compactionId": id, "status": "completed"}));
                updates
            }
            Event::Denied(tools) if v2 && !tools.is_empty() => {
                vec![json!({"sessionUpdate": "notice",
                "severity": "warning", "title": alloc::format!("The harness refused {}", tools.join(", "))})]
            }
            _ => Vec::new(),
        }
    }

    fn remember(&mut self, id: &str) -> bool {
        if self.tools.iter().any(|known| known == id) {
            return false;
        }
        if self.max_tools == 0 {
            return true;
        }
        if self.tools.len() >= self.max_tools
            && let Some(oldest) = self.tools.pop_front()
        {
            self.content.remove(&oldest);
            self.terminals.retain(|_, tool| *tool != oldest);
        }
        self.tools.push_back(id.to_string());
        true
    }

    fn tool(&mut self, call: &ToolCall) -> Vec<Value> {
        let v2 = self.version >= 2;
        let first = self.remember(&call.id);
        let mut update = Map::new();
        update.insert(
            "sessionUpdate".into(),
            json!(if first && !v2 {
                "tool_call"
            } else {
                "tool_call_update"
            }),
        );
        update.insert("toolCallId".into(), json!(call.id));
        if v2 && let Some(name) = &call.name {
            update.insert("name".into(), json!(name));
        }
        let title = call
            .title
            .clone()
            .or_else(|| (first && !v2).then(|| call.name.clone().unwrap_or_else(|| "Tool".into())));
        if let Some(title) = title {
            update.insert("title".into(), json!(title));
        }
        if let Some(kind) = call.kind {
            update.insert("kind".into(), json!(kind.as_str()));
        }
        if let Some(status) = call.status {
            update.insert("status".into(), json!(status_name(status, v2)));
        }
        if let Some(input) = &call.input {
            update.insert("rawInput".into(), input.clone());
        }
        if let Some(output) = &call.output {
            update.insert("rawOutput".into(), output.clone());
        }
        if let Some(locations) = &call.locations {
            let locations: Vec<Value> = locations
                .iter()
                .map(|location| match location.line {
                    Some(line) => json!({"path": location.path, "line": line}),
                    None => json!({"path": location.path}),
                })
                .collect();
            update.insert("locations".into(), Value::Array(locations));
        }
        if let Some(content) = &call.content {
            let mut items = Vec::new();
            for item in content {
                if let ToolContent::Terminal(terminal) = item {
                    self.terminals.insert(terminal.clone(), call.id.clone());
                }
                if let Some(item) = tool_content(item, v2) {
                    items.push(item);
                }
            }
            if !v2 {
                self.content.insert(call.id.clone(), items.clone());
            }
            update.insert("content".into(), Value::Array(items));
        }
        if matches!(
            call.status,
            Some(ToolStatus::Completed | ToolStatus::Failed | ToolStatus::Cancelled)
        ) {
            self.content.remove(&call.id);
            self.content_bytes.remove(&call.id);
        }
        vec![Value::Object(update)]
    }

    fn tool_chunk(&mut self, id: &str, content: &ToolContent) -> Vec<Value> {
        let v2 = self.version >= 2;
        let Some(item) = tool_content(content, v2) else {
            return Vec::new();
        };
        if v2 {
            return vec![
                json!({"sessionUpdate": "tool_call_content_chunk", "toolCallId": id, "content": item}),
            ];
        }
        let bytes = self.content_bytes.entry(id.to_string()).or_default();
        if *bytes == usize::MAX {
            return Vec::new();
        }
        let content = self.content.entry(id.to_string()).or_default();
        let size = item.to_string().len();
        if bytes.saturating_add(size) > self.max_content_bytes
            || content.len() >= self.max_content_items
        {
            *bytes = usize::MAX;
            content.push(json!({"type": "content",
                "content": {"type": "text", "text": "[the rest of this output was left out]"}}));
        } else {
            *bytes += size;
            content.push(item);
        }
        vec![
            json!({"sessionUpdate": "tool_call_update", "toolCallId": id, "content": content.clone()}),
        ]
    }

    fn terminal(&mut self, terminal: &Terminal) -> Vec<Value> {
        if self.version >= 2 {
            let mut update = Map::new();
            update.insert("sessionUpdate".into(), json!("terminal_update"));
            update.insert("terminalId".into(), json!(terminal.id));
            if let Some(command) = &terminal.command {
                update.insert("command".into(), json!(command));
            }
            if let Some(output) = &terminal.output {
                let data = base64::engine::general_purpose::STANDARD.encode(output.as_bytes());
                update.insert("output".into(), json!({"data": data}));
            }
            if let Some(exit) = terminal.exit {
                update.insert(
                    "exitStatus".into(),
                    exit.map_or_else(|| json!({}), |code| json!({"exitCode": code})),
                );
            }
            return vec![Value::Object(update)];
        }
        // Version 1 shows a terminal's output as text of the tool call that runs it.
        let Some(output) = terminal.output.as_ref().filter(|output| !output.is_empty()) else {
            return Vec::new();
        };
        let Some(tool) = self.terminals.get(&terminal.id).cloned() else {
            return Vec::new();
        };
        self.tool_chunk(&tool, &ToolContent::Text(output.clone()))
    }
}

fn status_name(status: ToolStatus, v2: bool) -> &'static str {
    match status {
        ToolStatus::Cancelled if !v2 => "failed",
        other => other.as_str(),
    }
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

pub(crate) fn content_block(content: &Content) -> Value {
    match content {
        Content::Text(text) => text_block(text),
        Content::Image { mime_type, data } => {
            json!({"type": "image", "mimeType": mime_type, "data": data})
        }
        Content::ResourceLink { uri, name } => {
            json!({"type": "resource_link", "uri": uri, "name": name})
        }
        Content::Resource {
            uri,
            mime_type,
            text,
            blob,
        } => {
            let mut resource = Map::new();
            resource.insert("uri".into(), json!(uri));
            if let Some(mime_type) = mime_type {
                resource.insert("mimeType".into(), json!(mime_type));
            }
            if let Some(text) = text {
                resource.insert("text".into(), json!(text));
            }
            if let Some(blob) = blob {
                resource.insert("blob".into(), json!(blob));
            }
            json!({"type": "resource", "resource": resource})
        }
    }
}

fn chunk(kind: &str, message: &str, content: Value, v2: bool) -> Value {
    if v2 {
        json!({"sessionUpdate": kind, "messageId": message, "content": content})
    } else {
        json!({"sessionUpdate": kind, "content": content})
    }
}

/// A message's end carries no `content`: version 2 reads an empty array as clearing the message.
fn ended(kind: &str, message: &str) -> Value {
    json!({"sessionUpdate": kind, "messageId": message})
}

fn tool_content(content: &ToolContent, v2: bool) -> Option<Value> {
    Some(match content {
        ToolContent::Text(text) => json!({"type": "content", "content": text_block(text)}),
        ToolContent::Diff(diff) if v2 => diff_v2(diff),
        ToolContent::Diff(diff) => json!({"type": "diff", "path": diff.path,
            "oldText": diff.old_text, "newText": diff.new_text.clone().unwrap_or_default()}),
        ToolContent::Terminal(id) if v2 => json!({"type": "terminal", "terminalId": id}),
        ToolContent::Terminal(_) => return None,
    })
}

fn diff_v2(diff: &Diff) -> Value {
    let operation = match diff.change {
        FileChange::Add => "add",
        FileChange::Modify => "modify",
        FileChange::Delete => "delete",
    };
    let mut value = json!({"type": "diff", "changes": [{"operation": operation, "path": diff.path, "fileType": "text"}]});
    if let Some(patch) = &diff.patch {
        value["patch"] = json!({"format": "git_patch", "text": patch});
    }
    value
}

fn plan(entries: &[PlanEntry], v2: bool) -> Value {
    let entries: Vec<Value> = entries
        .iter()
        .map(|entry| json!({"content": entry.content, "status": entry.status.as_str(), "priority": "medium"}))
        .collect();
    if v2 {
        json!({"sessionUpdate": "plan_update", "plan": {"type": "items", "planId": PLAN_ID, "entries": entries}})
    } else {
        json!({"sessionUpdate": "plan", "entries": entries})
    }
}

fn usage_update(usage: &Usage) -> Option<Value> {
    let (Some(used), Some(size)) = (usage.context_used, usage.context_size) else {
        return None;
    };
    let mut update = json!({"sessionUpdate": "usage_update", "used": used, "size": size});
    if let Some(cost) = usage.cost_usd {
        update["cost"] = json!({"amount": cost, "currency": "USD"});
    }
    Some(update)
}

fn available_commands(commands: &[Command], v2: bool) -> Value {
    let commands: Vec<Value> = commands
        .iter()
        .map(|command| {
            let mut value = json!({"name": command.name, "description": command.description});
            if v2 {
                value["type"] = json!("command");
            }
            if let Some(hint) = &command.hint {
                value["input"] = if v2 {
                    json!({"type": "text", "hint": hint})
                } else {
                    json!({"hint": hint})
                };
            }
            value
        })
        .collect();
    json!({"sessionUpdate": "available_commands_update", "availableCommands": commands})
}

/// A turn's token counts as version 2 gives them with `idle`.
pub(crate) fn turn_usage(usage: &Usage) -> Value {
    json!({
        "inputTokens": usage.input,
        "outputTokens": usage.output,
        "cachedReadTokens": usage.cache_read,
        "cachedWriteTokens": usage.cache_write,
        "totalTokens": usage.input.saturating_add(usage.output).saturating_add(usage.cache_read).saturating_add(usage.cache_write),
    })
}

/// The `_meta` that says a quota refused a turn.
pub(crate) fn limit_meta(limit: &LimitHit) -> Value {
    json!({ LIMIT_META_KEY: serde_json::to_value(limit).unwrap_or(Value::Null) })
}

/// Options as ACP selects: `id` in version 1, `configId` in version 2.
pub(crate) fn config_options(options: &[ConfigOption], version: u8) -> Value {
    Value::Array(
        options
            .iter()
            .map(|option| {
                let mut value = Map::new();
                value.insert(
                    (if version >= 2 { "configId" } else { "id" }).into(),
                    json!(option.id),
                );
                value.insert("name".into(), json!(option.name));
                if let Some(description) = &option.description {
                    value.insert("description".into(), json!(description));
                }
                if let Some(category) = &option.category {
                    value.insert("category".into(), json!(category));
                }
                value.insert("type".into(), json!("select"));
                value.insert("currentValue".into(), json!(option.current));
                let values: Vec<Value> = option
                    .values
                    .iter()
                    .map(|choice| {
                        let mut entry = json!({"value": choice.id, "name": choice.name});
                        if let Some(description) = &choice.description {
                            entry["description"] = json!(description);
                        }
                        entry
                    })
                    .collect();
                value.insert("options".into(), Value::Array(values));
                Value::Object(value)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn usage_of_another_thread_changes_neither_the_context_nor_the_turns_usage() {
        let mut shapes = super::Shapes::new(2, 8);
        let own = crate::event::Usage {
            input: 1,
            context_used: Some(5),
            context_size: Some(10),
            ..Default::default()
        };
        assert_eq!(
            shapes
                .updates(&crate::event::Event::Usage(own.clone()))
                .len(),
            1
        );
        let other = crate::event::Usage {
            input: 100,
            elsewhere: true,
            context_used: Some(99),
            context_size: Some(100),
            ..Default::default()
        };
        assert!(
            shapes
                .updates(&crate::event::Event::Usage(other))
                .is_empty()
        );
        assert_eq!(shapes.usage, Some(own));
    }

    #[test]
    fn counts_past_any_size_add_up_to_the_largest() {
        let usage = crate::event::Usage {
            input: u64::MAX,
            output: 1,
            ..Default::default()
        };
        assert_eq!(super::turn_usage(&usage)["totalTokens"], u64::MAX);
    }

    use super::*;
    use crate::event::{Location, ToolKind};

    fn edit() -> ToolCall {
        let mut call = ToolCall::new("t1");
        call.name = Some("Edit".into());
        call.title = Some("Edit notes.txt".into());
        call.kind = Some(ToolKind::Edit);
        call.status = Some(ToolStatus::Pending);
        call.locations = Some(vec![Location::new("/w/notes.txt", Some(2))]);
        let mut diff = Diff::new("/w/notes.txt", FileChange::Modify);
        diff.old_text = Some("two".into());
        diff.new_text = Some("three".into());
        diff.patch = Some("diff --git a/w/notes.txt b/w/notes.txt\n".into());
        call.content = Some(vec![ToolContent::Diff(diff)]);
        call
    }

    #[test]
    fn version_1_announces_a_tool_call_and_version_2_upserts_it() {
        let mut v1 = Shapes::new(1, 16);
        let first = v1.updates(&Event::Tool(edit()));
        assert_eq!(first[0]["sessionUpdate"], "tool_call");
        assert_eq!(
            first[0]["content"][0],
            json!({"type": "diff", "path": "/w/notes.txt", "oldText": "two", "newText": "three"})
        );
        assert!(
            first[0].get("name").is_none(),
            "version 1 has no name field"
        );
        let mut done = ToolCall::new("t1");
        done.status = Some(ToolStatus::Cancelled);
        let second = v1.updates(&Event::Tool(done.clone()));
        assert_eq!(second[0]["sessionUpdate"], "tool_call_update");
        assert_eq!(second[0]["status"], "failed", "version 1 has no cancelled");

        let mut v2 = Shapes::new(2, 16);
        let first = v2.updates(&Event::Tool(edit()));
        assert_eq!(first[0]["sessionUpdate"], "tool_call_update");
        assert_eq!(first[0]["name"], "Edit");
        assert_eq!(
            first[0]["content"][0]["changes"][0],
            json!({"operation": "modify", "path": "/w/notes.txt", "fileType": "text"})
        );
        assert_eq!(first[0]["content"][0]["patch"]["format"], "git_patch");
        assert_eq!(v2.updates(&Event::Tool(done))[0]["status"], "cancelled");
    }

    #[test]
    fn a_terminal_is_its_own_update_in_version_2_and_the_tools_text_in_version_1() {
        let mut call = ToolCall::new("b1");
        call.title = Some("echo hi".into());
        call.content = Some(vec![ToolContent::Terminal("b1".into())]);
        let mut output = Terminal::new("b1");
        output.output = Some("hi\n".into());
        output.exit = Some(Some(0));

        let mut v2 = Shapes::new(2, 16);
        v2.updates(&Event::Tool(call.clone()));
        let update = v2.updates(&Event::Terminal(output.clone()));
        assert_eq!(update[0]["sessionUpdate"], "terminal_update");
        assert_eq!(update[0]["output"]["data"], "aGkK");
        assert_eq!(update[0]["exitStatus"]["exitCode"], 0);

        let mut v1 = Shapes::new(1, 16);
        let announced = v1.updates(&Event::Tool(call));
        assert_eq!(
            announced[0]["content"],
            json!([]),
            "version 1 has no agent terminal"
        );
        let update = v1.updates(&Event::Terminal(output));
        assert_eq!(update[0]["content"][0]["content"]["text"], "hi\n");
    }

    #[test]
    fn version_2_chunks_carry_their_message_and_version_1_kinds_stay_version_1() {
        let mut v1 = Shapes::new(1, 16);
        let mut v2 = Shapes::new(2, 16);
        let chunk = Event::MessageChunk {
            message: "m1".into(),
            text: "hi".into(),
        };
        assert!(v1.updates(&chunk)[0].get("messageId").is_none());
        assert_eq!(v2.updates(&chunk)[0]["messageId"], "m1");
        let end = Event::MessageEnd {
            message: "m1".into(),
        };
        assert!(v1.updates(&end).is_empty());
        assert_eq!(
            v2.updates(&end)[0],
            json!({"sessionUpdate": "agent_message", "messageId": "m1"})
        );
        let notice = Event::Notice {
            severity: crate::event::Severity::Warning,
            title: "untested".into(),
        };
        assert!(v1.updates(&notice).is_empty());
        assert_eq!(v2.updates(&notice)[0]["sessionUpdate"], "notice");
        let entries = vec![PlanEntry::new("Read", crate::event::PlanStatus::Completed)];
        assert_eq!(
            v1.updates(&Event::Plan(entries.clone()))[0]["sessionUpdate"],
            "plan"
        );
        assert_eq!(
            v2.updates(&Event::Plan(entries))[0]["plan"]["planId"],
            PLAN_ID
        );
    }

    #[test]
    fn the_oldest_tool_past_the_limit_is_forgotten_and_announced_again() {
        let mut v1 = Shapes::new(1, 1);
        assert_eq!(
            v1.updates(&Event::Tool(ToolCall::new("a")))[0]["sessionUpdate"],
            "tool_call"
        );
        assert_eq!(
            v1.updates(&Event::Tool(ToolCall::new("b")))[0]["sessionUpdate"],
            "tool_call"
        );
        assert_eq!(
            v1.updates(&Event::Tool(ToolCall::new("a")))[0]["sessionUpdate"],
            "tool_call"
        );
        let update = v1.updates(&Event::Tool(ToolCall::new("a")));
        assert_eq!(update[0]["sessionUpdate"], "tool_call_update");
        assert!(
            update[0].get("title").is_none(),
            "an update has no title of its own"
        );
    }
}
