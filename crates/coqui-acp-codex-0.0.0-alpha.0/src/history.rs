//! Codex's rollout file as a standard history, and back.
//!
//! [`export`] reads a rollout (`sessions/<year>/<month>/<day>/rollout-<time>-<id>.jsonl`) into a
//! [`Document`]; [`import`] writes a rollout Codex resumes from a [`Document`]. What Codex adds to a
//! thread by itself (its base instructions, the developer messages, the environment context, the
//! tool list, token counts, turn settings) is configuration, not conversation: it is not carried,
//! and Codex adds its own again on the next turn.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use coqui_acp_harness::event::context_block;
use coqui_acp_history::conversation::{
    Boundary, Call, Conversation, Item, Recorder, Stamp, call_note, clean_id, derived_uuid,
    is_uuid, shell_text, valid_time,
};
use coqui_acp_history::{
    ATTACHMENT_BY_DIGEST, ATTACHMENT_META_KEY, Document, Header, LossReport, Origin,
    SUMMARY_MISSING, SeedPoint, Stop, StopReason, THINKING, TOOL_AS_NOTE, Tool,
};
use serde_json::{Map, Value, json};

use crate::{Error, TESTED_VERSIONS};

/// The harness name in a history's header and in the `_meta` this codec keeps.
pub const HARNESS: &str = "codex";

/// A loss the tool table has no name for: content the target cannot take at all.
pub const CONTENT_DROPPED: &str = "content_dropped";

/// What Codex puts before the summary of a compaction, in the message that carries it.
const SUMMARY_PREFIX: &str = "Another language model started to solve this problem and produced a summary of its thinking process. You also have access to the state of the tools that were used by that language model. Use this to build on the work that has already been done and avoid duplicating work. Here is the summary produced by the other language model, use the information in this summary to assist with your own analysis:\n";

/// The text a cancelled tool call's output carries.
const CANCELLED_OUTPUT: &str = "The tool call did not finish before the history ended.";

/// What the codecs read and write, in bytes: a rollout larger than the bound is refused.
pub const DEFAULT_MAX_BYTES: u64 = 256 << 20;

fn refused(why: impl Into<String>) -> Error {
    Error::Refused(why.into())
}

/// Codex's home directory for an environment: `CODEX_HOME`, else `.codex` in the home directory
/// (`USERPROFILE` on Windows, `HOME` elsewhere). `env` is what the launch sets, over this process's
/// environment unless `clear_env`.
#[must_use]
pub fn codex_home(env: &BTreeMap<OsString, OsString>, clear_env: bool) -> Option<PathBuf> {
    let get = |name: &str| {
        env.iter()
            .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
            .or_else(|| {
                if clear_env {
                    None
                } else {
                    std::env::var_os(name)
                }
            })
            .filter(|value| !value.is_empty())
    };
    if let Some(dir) = get("CODEX_HOME") {
        return Some(PathBuf::from(dir));
    }
    let home = if cfg!(windows) {
        get("USERPROFILE").or_else(|| get("HOME"))
    } else {
        get("HOME")
    };
    home.map(|home| PathBuf::from(home).join(".codex"))
}

// ---------------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------------

/// The rollout in `text` as a history named `id` (the host's UUID).
///
/// Lines that are not JSON are skipped. A turn left open (a recording cut short) stays open.
///
/// # Errors
///
/// [`Error::Refused`] for an `id` that is not a UUID or a history the format refuses.
pub fn export(text: &str, id: &str) -> Result<Document, Error> {
    if !is_uuid(id) {
        return Err(refused("the session id is not a UUID"));
    }
    let mut exporter = Exporter::default();
    for line in text
        .lines()
        .filter(|line| line.trim_start().starts_with('{'))
    {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            exporter.record(&value);
        }
    }
    let mut header = Header::new(id, Origin::new(HARNESS));
    header.origin.version = exporter.version.clone();
    header.origin.session_id = exporter.session.clone();
    header.created_at = exporter.created.clone().filter(|at| valid_time(at));
    header.cwd = exporter.cwd.clone();
    header.model = exporter.model.clone();
    header.effort = exporter.effort.clone();
    header = tidied(header);
    let mut document = Document::new(header);
    document.lines = core::mem::take(&mut exporter.rec).finish();
    document
        .seal()
        .map_err(|error| refused(format!("the rollout is not a history: {error}")))?;
    Ok(document)
}

/// `header` with every optional field the format refuses dropped: they come from a file this code
/// does not control.
fn tidied(mut header: Header) -> Header {
    let ok = |header: &Header| header.to_line().is_ok();
    for clear in [
        (|h: &mut Header| h.origin.session_id = None) as fn(&mut Header),
        |h| h.origin.version = None,
        |h| h.cwd = None,
        |h| h.model = None,
        |h| h.effort = None,
        |h| h.created_at = None,
    ] {
        if ok(&header) {
            break;
        }
        clear(&mut header);
    }
    header
}

fn string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

#[derive(Default)]
struct Exporter {
    rec: Recorder,
    /// Each announced call's name and input, for its output.
    calls: BTreeMap<String, String>,
    open_calls: BTreeSet<String>,
    error: Option<String>,
    session: Option<String>,
    version: Option<String>,
    created: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

impl Exporter {
    fn record(&mut self, value: &Value) {
        let stamp = Stamp::at(string(value, "timestamp").filter(|at| valid_time(at)));
        let payload = value.get("payload").unwrap_or(&Value::Null);
        match value.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                self.session = self.session.take().or_else(|| string(payload, "id"));
                self.version = self
                    .version
                    .take()
                    .or_else(|| string(payload, "cli_version").filter(|text| text.len() <= 128));
                self.created = self.created.take().or_else(|| string(payload, "timestamp"));
                self.cwd = self.cwd.take().or_else(|| string(payload, "cwd"));
            }
            Some("turn_context") => {
                if let Some(model) = string(payload, "model") {
                    self.model = Some(model);
                }
                if let Some(effort) = string(payload, "effort") {
                    self.effort = Some(effort);
                }
            }
            Some("event_msg") => self.event(payload, stamp),
            Some("response_item") => self.item(payload, stamp),
            Some("compacted") => {
                let message = string(payload, "message").unwrap_or_default();
                let summary = message
                    .strip_prefix(SUMMARY_PREFIX)
                    .unwrap_or(&message)
                    .trim_end_matches('\n')
                    .to_owned();
                let mut fields = Map::new();
                fields.insert("message".into(), json!(message));
                self.rec
                    .compaction(Some(summary), None, None, stamp.native(HARNESS, fields));
            }
            _ => {}
        }
    }

    fn event(&mut self, payload: &Value, stamp: Stamp) {
        match payload.get("type").and_then(Value::as_str) {
            Some("task_complete") => self.end_turn(false, stamp),
            Some("turn_aborted") => self.end_turn(true, stamp),
            Some("error") => {
                self.error =
                    string(payload, "message").or_else(|| Some("the turn failed".to_owned()));
            }
            _ => {}
        }
    }

    fn end_turn(&mut self, aborted: bool, stamp: Stamp) {
        if !self.rec.is_open() {
            self.error = None;
            return;
        }
        let (stop, error) = if aborted {
            self.rec.cancel(Stamp::default());
            (Stop::Acp(StopReason::Cancelled), None)
        } else if let Some(error) = self.error.take() {
            (Stop::Error, Some(error))
        } else {
            (Stop::Acp(StopReason::EndTurn), None)
        };
        self.error = None;
        self.rec.end_turn(stop, error, Stamp::at(stamp.at));
        self.open_calls.clear();
    }

    fn item(&mut self, payload: &Value, stamp: Stamp) {
        match payload.get("type").and_then(Value::as_str) {
            Some("message") => self.message(payload, stamp),
            Some("reasoning") => self.reasoning(payload, stamp),
            Some("function_call") => {
                let name = string(payload, "name").unwrap_or_default();
                let arguments = payload
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let input = serde_json::from_str::<Value>(arguments)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| json!({"arguments": arguments}));
                let mut fields = Map::new();
                fields.insert("item".into(), json!("function_call"));
                fields.insert("arguments".into(), json!(arguments));
                self.call(payload, &name, input, fields, stamp);
            }
            Some("custom_tool_call") => {
                let name = string(payload, "name").unwrap_or_default();
                let input = json!({"input": payload.get("input").cloned().unwrap_or(Value::Null)});
                let mut fields = Map::new();
                fields.insert("item".into(), json!("custom_tool_call"));
                self.call(payload, &name, input, fields, stamp);
            }
            Some("local_shell_call") => {
                let input = payload.get("action").cloned().unwrap_or_else(|| json!({}));
                let mut fields = Map::new();
                fields.insert("item".into(), json!("local_shell_call"));
                self.call(payload, "local_shell", input, fields, stamp);
            }
            Some("web_search_call") => {
                let input = payload.get("action").cloned().unwrap_or_else(|| json!({}));
                let mut fields = Map::new();
                fields.insert("item".into(), json!("web_search_call"));
                if let Some(id) = payload.get("id").and_then(Value::as_str) {
                    fields.insert("itemId".into(), json!(id));
                }
                let status = string(payload, "status").unwrap_or_else(|| "completed".to_owned());
                let id = string(payload, "id")
                    .unwrap_or_else(|| format!("web_search_{}", self.calls.len()));
                let mut call = Call::new(&id);
                call.name = Some("web_search".to_owned());
                call.title = Some("Search the web".to_owned());
                call.kind = Some("fetch".to_owned());
                call.status = Some("pending".to_owned());
                call.raw_input = Some(input);
                self.rec
                    .update(call.announce(), stamp.clone().native(HARNESS, fields));
                let mut done = Call::new(&id);
                done.status = Some(
                    if status == "failed" {
                        "failed"
                    } else {
                        "completed"
                    }
                    .to_owned(),
                );
                self.rec.update(done.finish(), stamp);
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                let Some(id) = payload.get("call_id").and_then(Value::as_str) else {
                    return;
                };
                self.open_calls.remove(id);
                let output = payload.get("output").cloned();
                let failed = output.as_ref().is_some_and(failed_output);
                let mut call = Call::new(id);
                call.status = Some(if failed { "failed" } else { "completed" }.to_owned());
                call.raw_output = output;
                let text = call.output_text();
                if !text.is_empty() {
                    call.content =
                        vec![json!({"type": "content", "content": {"type": "text", "text": text}})];
                }
                self.rec.update(call.finish(), stamp);
            }
            _ => {}
        }
    }

    fn message(&mut self, payload: &Value, stamp: Stamp) {
        let content = payload
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match payload.get("role").and_then(Value::as_str) {
            Some("user") => {
                if is_context(payload, &content) {
                    return;
                }
                let blocks: Vec<Value> = content.iter().filter_map(prompt_block).collect();
                let text = blocks
                    .first()
                    .and_then(|block| block.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                // What the harness echoed of a command or an image is no prompt of the person's.
                let echo = ["<local-command-", "<command-", "[Image: source"]
                    .iter()
                    .any(|lead| text.starts_with(lead));
                if echo {
                    for block in blocks {
                        self.rec.note(block, stamp.clone());
                    }
                } else if !blocks.is_empty() {
                    self.rec.prompt(blocks, stamp);
                }
            }
            Some("assistant") => {
                let text: String = content
                    .iter()
                    .filter(|block| {
                        block.get("type").and_then(Value::as_str) == Some("output_text")
                    })
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect();
                self.rec.message(&text, stamp);
            }
            _ => {}
        }
    }

    fn reasoning(&mut self, payload: &Value, stamp: Stamp) {
        let summary = payload.get("summary").cloned().unwrap_or(Value::Null);
        let text: Vec<&str> = summary
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect();
        let mut fields = Map::new();
        fields.insert("summary".into(), summary.clone());
        if let Some(id) = string(payload, "id") {
            fields.insert("id".into(), json!(id));
        }
        if let Some(content) = payload.get("content").filter(|content| !content.is_null()) {
            fields.insert("content".into(), content.clone());
        }
        if let Some(sealed) = string(payload, "encrypted_content") {
            fields.insert("encrypted".into(), json!(sealed));
        }
        self.rec
            .thought(&text.join("\n\n"), stamp.native(HARNESS, fields));
    }

    fn call(
        &mut self,
        payload: &Value,
        name: &str,
        input: Value,
        mut fields: Map<String, Value>,
        stamp: Stamp,
    ) {
        let Some(id) = payload.get("call_id").and_then(Value::as_str) else {
            return;
        };
        if let Some(item) = string(payload, "id") {
            fields.insert("itemId".into(), json!(item));
        }
        if let Some(namespace) = string(payload, "namespace") {
            fields.insert("namespace".into(), json!(namespace));
        }
        let (kind, title, locations) = describe(name, &input);
        let mut call = Call::new(id);
        call.name = Some(name.to_owned());
        call.title = Some(title);
        call.kind = Some(kind.to_owned());
        call.status = Some("pending".to_owned());
        call.raw_input = Some(input);
        call.locations = locations
            .into_iter()
            .map(|path| json!({"path": path}))
            .collect();
        self.calls.insert(id.to_owned(), name.to_owned());
        self.open_calls.insert(id.to_owned());
        self.rec
            .update(call.announce(), stamp.native(HARNESS, fields));
    }
}

/// Whether a function's output says it failed: an object with `success: false`, or the script
/// runner's own words.
fn failed_output(output: &Value) -> bool {
    if output.get("success").and_then(Value::as_bool) == Some(false) {
        return true;
    }
    let first = match output {
        Value::String(text) => Some(text.as_str()),
        Value::Array(items) => items
            .first()
            .and_then(|item| item.get("text"))
            .and_then(Value::as_str),
        _ => None,
    };
    first.is_some_and(|text| text.starts_with("Script failed"))
}

/// The kind, title and paths of a call by its name and input.
fn describe(name: &str, input: &Value) -> (&'static str, String, Vec<String>) {
    let text = |key: &str| input.get(key).and_then(Value::as_str);
    if let Some(tool) = Tool::from_native(HARNESS, name) {
        let kind = match tool {
            Tool::Shell => "execute",
            Tool::Patch | Tool::Write | Tool::Edit => "edit",
            Tool::Read => "read",
            Tool::Grep | Tool::Glob => "search",
            Tool::Fetch => "fetch",
            _ => "other",
        };
        if kind == "edit" {
            let paths = patch_paths(text("input").unwrap_or_default());
            let title = match paths.as_slice() {
                [one] => format!("Edit {one}"),
                [] => "Edit files".to_owned(),
                many => format!("Edit {} files", many.len()),
            };
            return (kind, title, paths);
        }
        let command = input.get("command").map(|command| match command {
            Value::String(line) => line.clone(),
            Value::Array(words) => {
                shell_text(&words.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            }
            _ => String::new(),
        });
        return (kind, cut(command.as_deref().unwrap_or(name)), Vec::new());
    }
    match name {
        "local_shell" | "exec_command" | "container.exec" => {
            let command = match input.get("command").or_else(|| input.get("cmd")) {
                Some(Value::Array(words)) => {
                    shell_text(&words.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                }
                Some(Value::String(line)) => line.clone(),
                _ => name.to_owned(),
            };
            ("execute", cut(&command), Vec::new())
        }
        "exec" => ("execute", "Run code".to_owned(), Vec::new()),
        "update_plan" => ("think", "Update plan".to_owned(), Vec::new()),
        "apply_patch" => {
            let paths = patch_paths(text("input").unwrap_or_default());
            ("edit", "Edit files".to_owned(), paths)
        }
        _ if name.contains("__") => {
            let (server, tool) = name.split_once("__").unwrap_or((name, ""));
            let server = server
                .strip_prefix("mcp")
                .unwrap_or(server)
                .trim_matches('_');
            ("other", format!("{server}: {tool}"), Vec::new())
        }
        _ => ("other", name.to_owned(), Vec::new()),
    }
}

fn cut(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default();
    let cut = coqui_acp_harness::lines::cut(first, 256);
    if cut.len() < text.len() {
        format!("{cut}…")
    } else {
        cut.to_string()
    }
}

/// The paths an `apply_patch` text names.
fn patch_paths(patch: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in patch.lines() {
        for prefix in [
            "*** Add File: ",
            "*** Update File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ] {
            if let Some(path) = line.strip_prefix(prefix) {
                paths.push(path.trim().to_owned());
            }
        }
    }
    paths
}

/// Whether a user message is something Codex put there: its context, instructions and shell echoes.
fn is_context(payload: &Value, content: &[Value]) -> bool {
    let kinds = payload
        .pointer("/internal_chat_message_metadata_passthrough/content_item_kinds")
        .and_then(Value::as_array);
    if let Some(kinds) = kinds.filter(|kinds| !kinds.is_empty()) {
        // Codex says what each part of a message is: the person's own parts are `user.text`, `user.image` and so on.
        return !kinds
            .iter()
            .filter_map(Value::as_str)
            .any(|kind| kind.starts_with("user."));
    }
    let text = content
        .iter()
        .find_map(|block| block.get("text").and_then(Value::as_str))
        .unwrap_or_default();
    const TAGS: [&str; 8] = [
        "<environment_context>",
        "<user_instructions>",
        "# AGENTS.md instructions",
        "<permissions instructions>",
        "<collaboration_mode>",
        "<skills_instructions>",
        "<turn_aborted>",
        "<user_shell_command>",
    ];
    TAGS.iter().any(|tag| text.starts_with(tag))
}

/// An ACP content block of a Codex content item, when it has one.
fn prompt_block(block: &Value) -> Option<Value> {
    match block.get("type").and_then(Value::as_str)? {
        "input_text" | "output_text" => {
            Some(json!({"type": "text", "text": block.get("text")?.as_str()?}))
        }
        "input_image" => {
            let url = block.get("image_url")?.as_str()?;
            match url
                .strip_prefix("data:")
                .and_then(|rest| rest.split_once(','))
            {
                Some((head, data)) => Some(json!({"type": "image",
                    "mimeType": head.strip_suffix(";base64").unwrap_or(head), "data": data})),
                None => Some(json!({"type": "resource_link", "uri": url, "name": "image"})),
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------------------------

/// How an import is done.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImportOptions {
    /// Seed from the latest compaction summary the history has (the default). Off, the seed is every
    /// line of the history and no summary is written.
    pub use_summary: bool,
    /// The Codex version the rollout names; the newest tested one when none.
    pub version: Option<String>,
    /// The model provider the thread names (`openai` by default).
    pub model_provider: Option<String>,
    /// When the thread began, as RFC 3339; the history's `createdAt` when none.
    pub started: Option<String>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            use_summary: true,
            version: None,
            model_provider: None,
            started: None,
        }
    }
}

impl ImportOptions {
    /// The same options seeding from the latest summary or, with `false`, from the first line.
    #[must_use]
    pub const fn use_summary(mut self, yes: bool) -> Self {
        self.use_summary = yes;
        self
    }

    /// The same options naming `version` as the Codex that wrote the rollout.
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// The same options naming `provider` as the thread's model provider.
    #[must_use]
    pub fn model_provider(mut self, provider: impl Into<String>) -> Self {
        self.model_provider = Some(provider.into());
        self
    }

    /// The same options starting the thread at `started`, an RFC 3339 time.
    #[must_use]
    pub fn started(mut self, started: impl Into<String>) -> Self {
        self.started = Some(started.into());
        self
    }
}

/// A rollout written from a history.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Imported {
    /// The rollout: JSON Lines, every line ended by a line feed.
    pub text: String,
    /// What could not be carried over.
    pub report: LossReport,
    /// Where in the history the seed starts.
    pub seeded_from: Option<SeedPoint>,
    /// How many tool calls the history left open and the rollout ends as cancelled.
    pub closed_calls: u64,
    /// Whether the history left a turn open and the rollout ends it as aborted.
    pub closed_turn: bool,
    /// The file the rollout belongs in, under `sessions`: `<year>/<month>/<day>/rollout-<time>-<id>.jsonl`.
    pub path: String,
}

/// The rollout of `doc` for thread `id`, working in `cwd`.
///
/// The conversation is the seed ([`Conversation::of`]): a compaction summary goes only to the slot
/// Codex keeps for one, tool calls are written as Codex's own when the history's origin is Codex or
/// when the tool table names an equivalent (and as a plain note otherwise), and reasoning is
/// written only for a history from Codex, with the encrypted content it was written with. Nothing
/// is run, no permission answer is written as a grant, and no snapshot is made.
///
/// # Errors
///
/// [`Error::Refused`] for an `id` that is not a UUID or a start time that is not RFC 3339.
pub fn import(
    doc: &Document,
    cwd: &str,
    id: &str,
    options: &ImportOptions,
) -> Result<Imported, Error> {
    if !is_uuid(id) {
        return Err(refused("the session id is not a UUID"));
    }
    let started = options
        .started
        .clone()
        .or_else(|| doc.header.created_at.clone())
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_owned());
    if !valid_time(&started) || civil(&started).is_none() {
        return Err(refused("the start time is not an RFC 3339 time"));
    }
    let conversation = Conversation::of(doc, options.use_summary);
    let mut rollout = Rollout::new(doc, cwd, id, options, &started);
    rollout.write(&conversation);
    let mut report = rollout.report;
    report.add(
        SUMMARY_MISSING,
        u64::from(conversation.summary_missing),
        Some("the latest compaction has no summary, so the seed starts earlier"),
    );
    let (year, month, day, hour, minute, second) = civil(&started).unwrap_or_default();
    Ok(Imported {
        text: rollout.out,
        report,
        seeded_from: conversation.seeded_from.clone(),
        closed_calls: conversation.closed_calls,
        closed_turn: conversation.closed_turn,
        path: format!(
            "{year:04}/{month:02}/{day:02}/rollout-{year:04}-{month:02}-{day:02}T{hour:02}-{minute:02}-{second:02}-{id}.jsonl"
        ),
    })
}

/// The year, month, day, hour, minute and second of an RFC 3339 time, as written.
fn civil(time: &str) -> Option<(u32, u32, u32, u32, u32, u32)> {
    let bytes = time.as_bytes();
    let number = |from: usize, to: usize| -> Option<u32> {
        let digits = bytes.get(from..to)?;
        digits.iter().all(u8::is_ascii_digit).then(|| {
            digits
                .iter()
                .fold(0, |sum, digit| sum * 10 + u32::from(digit - b'0'))
        })
    };
    if bytes.get(4) != Some(&b'-') || bytes.get(10) != Some(&b'T') {
        return None;
    }
    Some((
        number(0, 4)?,
        number(5, 7)?,
        number(8, 10)?,
        number(11, 13)?,
        number(14, 16)?,
        number(17, 19)?,
    ))
}

/// Seconds since 1970 of an RFC 3339 time given in UTC or with an offset, ignoring the fraction.
fn epoch(time: &str) -> i64 {
    let Some((year, month, day, hour, minute, second)) = civil(time) else {
        return 0;
    };
    let (year, month, day) = (i64::from(year), i64::from(month), i64::from(day));
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second)
}

struct Rollout<'a> {
    doc: &'a Document,
    session: String,
    cwd: String,
    version: String,
    provider: String,
    out: String,
    report: LossReport,
    time: String,
    turn: Option<(String, i64)>,
    turns: u64,
    last_message: Option<String>,
    count: u64,
    ids: BTreeMap<String, String>,
}

impl<'a> Rollout<'a> {
    fn new(
        doc: &'a Document,
        cwd: &str,
        session: &str,
        options: &ImportOptions,
        started: &str,
    ) -> Self {
        Self {
            doc,
            session: session.to_owned(),
            cwd: cwd.to_owned(),
            version: options
                .version
                .clone()
                .or_else(|| TESTED_VERSIONS.last().map(|version| (*version).to_owned()))
                .unwrap_or_default(),
            provider: options
                .model_provider
                .clone()
                .unwrap_or_else(|| "openai".to_owned()),
            out: String::new(),
            report: LossReport::new(),
            time: started.to_owned(),
            turn: None,
            turns: 0,
            last_message: None,
            count: 0,
            ids: BTreeMap::new(),
        }
    }

    fn is_codex(&self) -> bool {
        self.doc.header.origin.harness == HARNESS
    }

    fn write(&mut self, conversation: &Conversation) {
        let started = self.time.clone();
        self.line(
            "session_meta",
            json!({"id": self.session, "timestamp": started, "cwd": self.cwd,
                "originator": "coqui-acp-history", "cli_version": self.version,
                "source": "vscode", "model_provider": self.provider}),
        );
        if let Some(boundary) = &conversation.summary {
            self.boundary(boundary);
        }
        for item in &conversation.items {
            self.item(conversation, item);
        }
    }

    fn line(&mut self, kind: &str, payload: Value) {
        let line = json!({"timestamp": self.time, "type": kind, "payload": payload});
        self.out.push_str(&line.to_string());
        self.out.push('\n');
        self.count += 1;
    }

    fn event(&mut self, payload: Value) {
        self.line("event_msg", payload);
    }

    fn note_time(&mut self, stamp: &Stamp) {
        if let Some(at) = stamp
            .at
            .as_deref()
            .filter(|at| valid_time(at) && civil(at).is_some())
        {
            at.clone_into(&mut self.time);
        }
    }

    fn boundary(&mut self, boundary: &Boundary) {
        self.note_time(&boundary.stamp);
        let message = boundary
            .stamp
            .native_of(HARNESS)
            .filter(|_| self.is_codex())
            .and_then(|fields| fields.get("message"))
            .and_then(Value::as_str)
            .map_or_else(
                || format!("{SUMMARY_PREFIX}{}", boundary.summary),
                str::to_owned,
            );
        self.line(
            "compacted",
            json!({"message": message, "replacement_history": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": message}]}]}),
        );
    }

    fn ensure_turn(&mut self) {
        if self.turn.is_none() {
            let id = derived_uuid(&self.session, "turn", self.turns);
            self.turns += 1;
            let at = epoch(&self.time);
            self.event(
                json!({"type": "task_started", "turn_id": id, "started_at": at,
                "collaboration_mode_kind": "default"}),
            );
            self.turn = Some((id, at));
        }
    }

    fn end_turn(&mut self, aborted: bool) {
        if let Some((id, started)) = self.turn.take() {
            let at = epoch(&self.time);
            if aborted {
                self.event(json!({"type": "turn_aborted", "turn_id": id, "reason": "interrupted"}));
            } else {
                let last = self.last_message.take();
                self.event(
                    json!({"type": "task_complete", "turn_id": id, "last_agent_message": last,
                    "started_at": started, "completed_at": at}),
                );
            }
        }
        self.last_message = None;
    }

    fn item(&mut self, conversation: &Conversation, item: &Item) {
        match item {
            Item::Prompt { blocks, stamp } => {
                self.note_time(stamp);
                self.end_turn(false);
                let (content, images, text) = self.content(blocks);
                if content.is_empty() {
                    return;
                }
                self.ensure_turn();
                self.line(
                    "response_item",
                    json!({"type": "message", "role": "user", "content": content}),
                );
                self.event(
                    json!({"type": "user_message", "message": text, "images": images,
                    "local_images": [], "text_elements": []}),
                );
            }
            Item::Note { block, stamp } => {
                self.note_time(stamp);
                let (content, _, _) = self.content(core::slice::from_ref(block));
                if !content.is_empty() {
                    self.line(
                        "response_item",
                        json!({"type": "message", "role": "user", "content": content}),
                    );
                }
            }
            Item::Message { text, stamp } => {
                self.note_time(stamp);
                self.assistant(text);
            }
            Item::Thought { text, stamp } => {
                self.note_time(stamp);
                self.thought(text, stamp);
            }
            Item::Call { id } => {
                if let Some(call) = conversation.call(id) {
                    self.call(call);
                }
            }
            Item::Result { id } => {
                if let Some(call) = conversation.call(id) {
                    self.result(call);
                }
            }
            Item::TurnEnd { stop, stamp, .. } => {
                self.note_time(stamp);
                self.end_turn(matches!(stop, Stop::Acp(StopReason::Cancelled)));
            }
            _ => {}
        }
    }

    fn assistant(&mut self, text: &str) {
        self.ensure_turn();
        self.line(
            "response_item",
            json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}),
        );
        self.event(json!({"type": "agent_message", "message": text}));
        self.last_message = Some(text.to_owned());
    }

    fn thought(&mut self, text: &str, stamp: &Stamp) {
        let native = stamp.native_of(HARNESS).filter(|_| self.is_codex());
        let Some(native) = native.filter(|fields| fields.contains_key("summary")) else {
            self.report.add(
                THINKING,
                1,
                Some("Codex takes reasoning only with the encrypted content it was written with"),
            );
            return;
        };
        self.ensure_turn();
        let mut item = Map::new();
        item.insert("type".into(), json!("reasoning"));
        if let Some(id) = native.get("id").and_then(Value::as_str) {
            item.insert("id".into(), json!(id));
        }
        item.insert("summary".into(), native["summary"].clone());
        item.insert(
            "content".into(),
            native.get("content").cloned().unwrap_or(Value::Null),
        );
        item.insert(
            "encrypted_content".into(),
            native.get("encrypted").cloned().unwrap_or(Value::Null),
        );
        self.line("response_item", Value::Object(item));
        if !text.is_empty() {
            self.event(json!({"type": "agent_reasoning", "text": text}));
        }
    }

    /// How a call is written: Codex's own tool when there is one.
    fn native_call(&self, call: &Call) -> Option<Native> {
        let name = call.name.as_deref()?;
        let input = call.raw_input.as_ref()?;
        if self.is_codex() {
            let fields = call
                .call_meta
                .as_ref()
                .and_then(|meta| coqui_acp_history::conversation::native_of(Some(meta), HARNESS))?;
            let item = fields.get("item").and_then(Value::as_str)?;
            return Some(Native {
                item: item.to_owned(),
                name: name.to_owned(),
                input: input.clone(),
                arguments: fields
                    .get("arguments")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                namespace: fields
                    .get("namespace")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                item_id: fields
                    .get("itemId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
        let (tool, normal) =
            coqui_acp_history::tools::normalise(&self.doc.header.origin.harness, name, input)?;
        if matches!(tool, Tool::Write | Tool::Edit) {
            let text = |key: &str| normal.get(key).and_then(Value::as_str);
            let path = text("path")?;
            let patch = if tool == Tool::Write {
                coqui_acp_history::patch::add(path, text("content")?)
            } else {
                coqui_acp_history::patch::update(path, text("old")?, text("new")?)
            };
            return Some(Native {
                item: "custom_tool_call".to_owned(),
                name: "apply_patch".to_owned(),
                input: json!({"input": patch}),
                arguments: None,
                namespace: None,
                item_id: None,
            });
        }
        if tool != Tool::Shell {
            return None;
        }
        // Codex's `shell_command` takes the command line as text, as Claude Code's Bash does.
        let native = Tool::Shell
            .natives()
            .iter()
            .find(|n| n.harness == HARNESS && n.name == "shell_command")?;
        let mut arguments = Map::new();
        for (key, value) in normal.as_object()? {
            let renamed = native
                .keys
                .iter()
                .find(|(normalised, _)| normalised == key)
                .map_or(key.as_str(), |(_, native)| native);
            if matches!(renamed, "command" | "workdir" | "timeout_ms") {
                arguments.insert(renamed.to_owned(), value.clone());
            }
        }
        if let Some(argv) = arguments.get("command").and_then(Value::as_array) {
            let words: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
            arguments.insert("command".into(), json!(shell_text(&words)));
        }
        arguments.get("command").and_then(Value::as_str)?;
        Some(Native {
            item: "function_call".to_owned(),
            name: "shell_command".to_owned(),
            input: Value::Object(arguments),
            arguments: None,
            namespace: None,
            item_id: None,
        })
    }

    fn call_id(&mut self, id: &str) -> String {
        if let Some(known) = self.ids.get(id) {
            return known.clone();
        }
        let clean = clean_id(id, &self.ids);
        self.ids.insert(id.to_owned(), clean.clone());
        clean
    }

    fn call(&mut self, call: &Call) {
        let stamp = Stamp::new(call.call_at.clone(), call.call_meta.clone());
        self.note_time(&stamp);
        let Some(native) = self.native_call(call) else {
            return;
        };
        self.ensure_turn();
        let id = self.call_id(&call.id);
        let mut item = Map::new();
        item.insert("call_id".into(), json!(id));
        item.insert("name".into(), json!(native.name));
        if let Some(item_id) = &native.item_id {
            item.insert("id".into(), json!(item_id));
        }
        if let Some(namespace) = &native.namespace {
            item.insert("namespace".into(), json!(namespace));
        }
        match native.item.as_str() {
            "custom_tool_call" => {
                item.insert("type".into(), json!("custom_tool_call"));
                item.insert(
                    "input".into(),
                    native.input.get("input").cloned().unwrap_or(Value::Null),
                );
            }
            "local_shell_call" => {
                item.remove("name");
                item.insert("type".into(), json!("local_shell_call"));
                item.insert("action".into(), native.input.clone());
                item.insert("status".into(), json!("completed"));
            }
            "web_search_call" => {
                item.remove("name");
                item.remove("call_id");
                item.insert("type".into(), json!("web_search_call"));
                item.insert("action".into(), native.input.clone());
                item.insert("status".into(), json!("completed"));
            }
            _ => {
                item.insert("type".into(), json!("function_call"));
                // The text the call was written with, when it still says the same.
                let arguments = native
                    .arguments
                    .filter(|text| {
                        serde_json::from_str::<Value>(text).ok().as_ref() == Some(&native.input)
                    })
                    .unwrap_or_else(|| native.input.to_string());
                item.insert("arguments".into(), json!(arguments));
            }
        }
        self.line("response_item", Value::Object(item));
    }

    fn result(&mut self, call: &Call) {
        if !call.announced {
            return;
        }
        let stamp = Stamp::new(
            call.result_at.clone().or_else(|| call.call_at.clone()),
            call.result_meta.clone(),
        );
        self.note_time(&stamp);
        if let Some(native) = self.native_call(call) {
            self.ensure_turn();
            if native.item == "web_search_call" {
                return;
            }
            let id = self.call_id(&call.id);
            let output = self.output_of(call);
            let kind = if native.item == "custom_tool_call" {
                "custom_tool_call_output"
            } else {
                "function_call_output"
            };
            self.line(
                "response_item",
                json!({"type": kind, "call_id": id, "output": output}),
            );
        } else {
            self.report.add(
                TOOL_AS_NOTE,
                1,
                Some("Codex has no equivalent tool, so the call is written as text"),
            );
            let note = call_note(call);
            self.assistant(&note);
        }
    }

    fn output_of(&self, call: &Call) -> Value {
        if call.status.as_deref() == Some("cancelled")
            && call.raw_output.is_none()
            && call.content.is_empty()
        {
            return json!(CANCELLED_OUTPUT);
        }
        if self.is_codex()
            && let Some(output) = call
                .raw_output
                .as_ref()
                .filter(|o| o.is_string() || o.is_array() || o.is_object())
        {
            return output.clone();
        }
        let text = call.output_text();
        match &call.raw_output {
            Some(Value::String(text)) => json!(text),
            Some(other) if text.is_empty() && !other.is_null() => json!(other.to_string()),
            _ => json!(text),
        }
    }

    /// Codex content items of ACP's blocks, the images as data URLs, and the text of the blocks.
    fn content(&mut self, blocks: &[Value]) -> (Vec<Value>, Vec<Value>, String) {
        let mut out = Vec::new();
        let mut images = Vec::new();
        let mut texts = Vec::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        out.push(json!({"type": "input_text", "text": text}));
                        texts.push(text.to_owned());
                    }
                }
                Some("image") => {
                    let data = block
                        .get("data")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let mime = block
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("image/png");
                    if data.is_empty() {
                        let by_digest = block
                            .get("_meta")
                            .and_then(|m| m.get(ATTACHMENT_META_KEY))
                            .is_some();
                        self.report.add(
                            if by_digest {
                                ATTACHMENT_BY_DIGEST
                            } else {
                                CONTENT_DROPPED
                            },
                            1,
                            Some("the history left the bytes out"),
                        );
                        out.push(json!({"type": "input_text", "text": format!("[an attachment of {mime} was left out of the history]")}));
                    } else {
                        let url = format!("data:{mime};base64,{data}");
                        out.push(json!({"type": "input_image", "image_url": url}));
                        images.push(json!(url));
                    }
                }
                Some("resource_link") => {
                    let uri = block.get("uri").and_then(Value::as_str).unwrap_or_default();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|n| !n.is_empty())
                        .unwrap_or(uri);
                    let text = format!("[@{name}]({uri})");
                    out.push(json!({"type": "input_text", "text": text}));
                    texts.push(text);
                }
                Some("resource") => {
                    let uri = block
                        .pointer("/resource/uri")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if let Some(text) = block.pointer("/resource/text").and_then(Value::as_str) {
                        let text = context_block(uri, text);
                        out.push(json!({"type": "input_text", "text": text}));
                        texts.push(text);
                    } else {
                        self.report.add(
                            CONTENT_DROPPED,
                            1,
                            Some("a resource of bytes, which Codex reads only as an image"),
                        );
                    }
                }
                _ => self.report.add(
                    CONTENT_DROPPED,
                    1,
                    Some("a content block of a kind Codex does not take"),
                ),
            }
        }
        (out, images, texts.join("\n"))
    }
}

/// A call as Codex writes it.
struct Native {
    item: String,
    name: String,
    input: Value,
    arguments: Option<String>,
    namespace: Option<String>,
    item_id: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------------------------

/// The rollout of thread `id` under `home`: the one file of `sessions` named for it, not through a
/// link.
///
/// # Errors
///
/// [`Error::Refused`] for an id that cannot name a thread, or one with no rollout.
pub fn rollout(home: &Path, id: &str) -> Result<PathBuf, Error> {
    if !is_uuid(id) {
        return Err(refused("that is not a Codex thread id"));
    }
    let suffix = format!("-{id}.jsonl");
    let mut found = None;
    let years = std::fs::read_dir(home.join("sessions")).map_err(|_| not_found())?;
    for year in years.flatten().filter(is_directory) {
        for month in std::fs::read_dir(year.path())
            .into_iter()
            .flatten()
            .flatten()
            .filter(is_directory)
        {
            for day in std::fs::read_dir(month.path())
                .into_iter()
                .flatten()
                .flatten()
                .filter(is_directory)
            {
                for file in std::fs::read_dir(day.path())
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let name = file.file_name().to_string_lossy().into_owned();
                    let regular = file.file_type().is_ok_and(|kind| kind.is_file());
                    if regular && name.starts_with("rollout-") && name.ends_with(&suffix) {
                        found = Some(file.path());
                    }
                }
            }
        }
    }
    found.ok_or_else(not_found)
}

fn is_directory(entry: &std::fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_dir())
}

fn not_found() -> Error {
    refused("no rollout of that thread was found")
}

/// The history of thread `id` from its rollout under `home`.
///
/// # Errors
///
/// [`Error::Refused`] when there is no such rollout, it is larger than `max_bytes`, or it is not a
/// rollout.
pub fn export_file(home: &Path, id: &str, max_bytes: u64) -> Result<Document, Error> {
    export_file_named(home, id, id, max_bytes)
}

/// The history of thread `thread` from its rollout under `home`, named `id` (the host's UUID, which a provider
/// maps to the thread Codex assigned).
///
/// # Errors
///
/// As [`export_file`].
pub fn export_file_named(
    home: &Path,
    thread: &str,
    id: &str,
    max_bytes: u64,
) -> Result<Document, Error> {
    let path = rollout(home, thread)?;
    let file = std::fs::File::open(&path)
        .map_err(|_| refused("the thread's rollout could not be read"))?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| refused("the thread's rollout could not be read"))?;
    if bytes.len() as u64 > max_bytes {
        return Err(refused(format!(
            "the thread's rollout is larger than {max_bytes} bytes"
        )));
    }
    export(&String::from_utf8_lossy(&bytes), id)
}

/// Writes the rollout of `doc` as thread `id` in `cwd` under `home`, where `thread/resume` finds it. A
/// rollout of that thread is never replaced.
///
/// # Errors
///
/// [`Error::Refused`] for an `id` that is not a UUID, a thread that exists or a directory that cannot
/// be made.
pub fn import_file(
    home: &Path,
    cwd: &Path,
    id: &str,
    doc: &Document,
    options: &ImportOptions,
) -> Result<Imported, Error> {
    if rollout(home, id).is_ok() {
        return Err(refused("the id names a session already"));
    }
    let mut options = options.clone();
    if options.started.is_none() && doc.header.created_at.is_none() {
        options.started = Some(now());
    }
    let imported = import(doc, &cwd.to_string_lossy(), id, &options)?;
    let path = home.join("sessions").join(&imported.path);
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)
            .map_err(|_| refused("the thread's directory could not be made"))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| refused("the thread's rollout could not be written"))?;
    if file.write_all(imported.text.as_bytes()).is_err() {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(refused("the thread's rollout could not be written"));
    }
    Ok(imported)
}

/// The time now as RFC 3339 in UTC, to the second.
fn now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(0));
    let days = seconds.div_euclid(86_400) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let rest = seconds.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}
