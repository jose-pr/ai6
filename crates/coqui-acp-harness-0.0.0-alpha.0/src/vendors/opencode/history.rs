//! OpenCode's `opencode export <id>` output as a standard history.
//!
//! OpenCode keeps a session in a SQLite database of its own. `opencode export` prints one session as one JSON object:
//! `info` (id, `directory`, `title`, `agent`, `model {providerID, id, variant}`, `version`, `time.created`) and `messages`,
//! each `{info, parts}`. A user message has `info.role` `user` and parts of type `text`; an assistant message has
//! `info.role` `assistant`, `info.finish` (`stop`, `tool-calls`, ...) and parts of type `step-start`, `reasoning`, `text`,
//! `tool` (`tool`, `callID`, `state {status, input, output, title, metadata}`) and `step-finish`. Measured on 1.19.6 with a
//! stand-in model, for a text turn and a `bash` call that completed. Not measured: a failed or running tool (`state.status`
//! `error` with an `error` text, by the published types), a compaction (an assistant message whose `info.summary` is true),
//! an aborted turn (`info.error.name` `MessageAbortedError`), attachments (`file` parts), and the input names of the tools
//! other than `bash` (`filePath`, `oldString`, `newString`, `content`, `pattern`, as its documentation names them).

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use coqui_acp_history::conversation::{Call, Recorder, Stamp, derived_uuid, is_uuid, valid_time};
use coqui_acp_history::{Document, Header, Origin, Stop, StopReason};
use serde_json::{Map, Value, json};

use crate::account::rfc3339_ms;

/// The harness name in a history's header and in the `_meta` this codec keeps.
pub const HARNESS: &str = "opencode";

/// The longest session id the provider hands to the program.
const MAX_ID: usize = 96;

/// Whether `id` is shaped like an id OpenCode made (`ses_` and letters and digits): the only text the provider puts after
/// `export`, so that no id can be taken for an option.
#[must_use]
pub fn is_native_id(id: &str) -> bool {
    id.strip_prefix("ses_").is_some_and(|rest| {
        !rest.is_empty()
            && id.len() <= MAX_ID
            && rest.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

/// The id OpenCode's store holds the session the host knows as the UUID `id` under: `ses_` and the UUID's 32 hex
/// digits, so that the same UUID is the same session on every connection and after a restart. `None` for text that is not
/// a UUID.
#[must_use]
pub fn native_id_of(id: &str) -> Option<String> {
    let uuid = uuid::Uuid::try_parse(id).ok()?;
    Some(format!("ses_{}", uuid.simple()))
}

/// The UUID a session of OpenCode's is known by to the host, when its id is one [`native_id_of`] made: `ses_` and 32
/// lower-case hex digits. `None` for every id OpenCode made itself (`ses_` and 26 letters and digits).
#[must_use]
pub fn host_id_of(native: &str) -> Option<String> {
    let rest = native.strip_prefix("ses_")?;
    let uuid = uuid::Uuid::try_parse(rest).ok()?;
    (rest == uuid.simple().to_string()).then(|| uuid.hyphenated().to_string())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn time_of(value: Option<&Value>) -> Option<String> {
    let at = rfc3339_ms(value?.as_i64()?);
    valid_time(&at).then_some(at)
}

/// One line of `text`, cut at the first line break.
fn one_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_owned()
}

/// What a tool call shows: its ACP kind, a title, and where it works.
struct View {
    kind: &'static str,
    title: String,
    locations: Vec<Value>,
    content: Vec<Value>,
}

fn view(name: &str, input: &Value, title: Option<&str>) -> View {
    let path = input
        .get("filePath")
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty());
    let at: Vec<Value> = path.iter().map(|path| json!({"path": path})).collect();
    let pattern = input
        .get("pattern")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let shown = |fallback: String| {
        title
            .filter(|title| !title.is_empty())
            .map_or(fallback, one_line)
    };
    match name {
        "bash" => View {
            kind: "execute",
            title: shown(one_line(
                input
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )),
            locations: Vec::new(),
            content: Vec::new(),
        },
        "read" => View {
            kind: "read",
            title: shown(format!("Read {}", path.unwrap_or_default())),
            locations: at,
            content: Vec::new(),
        },
        "write" | "edit" | "patch" | "multiedit" => View {
            kind: "edit",
            title: shown(format!("Edit {}", path.unwrap_or_default())),
            content: match (
                path,
                input.get("newString").or_else(|| input.get("content")),
            ) {
                (Some(path), Some(new)) => new
                    .as_str()
                    .map(|new| {
                        json!({
                            "type": "diff",
                            "path": path,
                            "oldText": input.get("oldString").and_then(Value::as_str),
                            "newText": new,
                        })
                    })
                    .into_iter()
                    .collect(),
                _ => Vec::new(),
            },
            locations: at,
        },
        "grep" | "glob" | "list" => View {
            kind: "search",
            title: shown(format!("Search {}", one_line(pattern))),
            locations: at,
            content: Vec::new(),
        },
        "webfetch" | "websearch" => View {
            kind: "fetch",
            title: shown(format!(
                "Fetch {}",
                input
                    .get("url")
                    .or_else(|| input.get("query"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )),
            locations: Vec::new(),
            content: Vec::new(),
        },
        other => View {
            kind: "other",
            title: shown(other.to_owned()),
            locations: at,
            content: Vec::new(),
        },
    }
}

#[derive(Default)]
struct Exporter {
    rec: Recorder,
    used: BTreeSet<String>,
    /// How the turn in progress ends: what the last assistant message said.
    stop: Option<(Stop, Option<String>)>,
    model: Option<String>,
}

impl Exporter {
    fn stamp(info: &Value, id: Option<&str>) -> Stamp {
        let at = time_of(info.pointer("/time/created"));
        let mut fields = Map::new();
        if let Some(id) = id {
            fields.insert("messageId".into(), json!(id));
        }
        if fields.is_empty() {
            Stamp::at(at)
        } else {
            Stamp::at(at).native(HARNESS, fields)
        }
    }

    fn close_turn(&mut self) {
        if self.rec.is_open() {
            let (stop, error) = self
                .stop
                .take()
                .unwrap_or((Stop::Acp(StopReason::EndTurn), None));
            self.rec.end_turn(stop, error, Stamp::default());
        }
        self.stop = None;
    }

    fn user(&mut self, message: &Value) {
        let info = &message["info"];
        let mut blocks = Vec::new();
        for part in message["parts"].as_array().into_iter().flatten() {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = text(part, "text") {
                        blocks.push(json!({"type": "text", "text": text}));
                    }
                }
                Some("file") => {
                    let name = text(part, "filename")
                        .or_else(|| text(part, "url"))
                        .unwrap_or_default();
                    blocks.push(json!({"type": "text", "text": format!("[attached file: {}]", one_line(&name))}));
                }
                _ => {}
            }
        }
        if blocks.is_empty() {
            return;
        }
        self.close_turn();
        let id = text(info, "id");
        self.rec.prompt(blocks, Self::stamp(info, id.as_deref()));
    }

    fn assistant(&mut self, message: &Value) {
        let info = &message["info"];
        let id = text(info, "id");
        let stamp = Self::stamp(info, id.as_deref());
        if let (Some(provider), Some(model)) = (text(info, "providerID"), text(info, "modelID")) {
            self.model = Some(format!("{provider}/{model}"));
        }
        let summary = info.get("summary").and_then(Value::as_bool) == Some(true);
        for part in message["parts"].as_array().into_iter().flatten() {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(body) = text(part, "text") {
                        if summary {
                            self.rec.compaction(Some(body), None, None, stamp.clone());
                        } else {
                            self.rec.message(&body, stamp.clone());
                        }
                    }
                }
                Some("reasoning") => {
                    if let Some(body) = text(part, "text") {
                        self.rec.thought(&body, stamp.clone());
                    }
                }
                Some("tool") => self.tool(part, &stamp),
                _ => {}
            }
        }
        if let Some(error) = info.get("error").filter(|error| error.is_object()) {
            if error.get("name").and_then(Value::as_str) == Some("MessageAbortedError") {
                self.rec.cancel(stamp);
                self.stop = Some((Stop::Acp(StopReason::Cancelled), None));
            } else {
                let why = error
                    .pointer("/data/message")
                    .and_then(Value::as_str)
                    .or_else(|| error.get("name").and_then(Value::as_str))
                    .unwrap_or("the model call failed");
                self.stop = Some((Stop::Error, Some(one_line(why))));
            }
        } else {
            self.stop = Some(match info.get("finish").and_then(Value::as_str) {
                Some("length") => (Stop::Acp(StopReason::MaxTokens), None),
                _ => (Stop::Acp(StopReason::EndTurn), None),
            });
        }
    }

    fn fresh_id(&mut self, native: &str) -> String {
        let mut id = native.to_owned();
        let mut again = 1_u64;
        while self.used.contains(&id) {
            again += 1;
            id = format!("{native}~{again}");
        }
        self.used.insert(id.clone());
        id
    }

    fn tool(&mut self, part: &Value, stamp: &Stamp) {
        let Some(name) = text(part, "tool") else {
            return;
        };
        let native = text(part, "callID")
            .or_else(|| text(part, "id"))
            .unwrap_or_else(|| name.clone());
        let id = self.fresh_id(&native);
        let state = part.get("state").cloned().unwrap_or(Value::Null);
        let input = state
            .get("input")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        let shown = view(&name, &input, state.get("title").and_then(Value::as_str));
        let mut call = Call::new(&id);
        call.name = Some(name.clone());
        call.title = Some(shown.title);
        call.kind = Some(shown.kind.to_owned());
        call.status = Some("pending".to_owned());
        call.raw_input = Some(input);
        call.locations = shown.locations;
        call.content = shown.content;
        self.rec.update(call.announce(), stamp.clone());
        let status = match state.get("status").and_then(Value::as_str) {
            Some("completed") => "completed",
            Some("error") => "failed",
            // A call still pending or running was cut short: the history ends it as cancelled.
            _ => return,
        };
        let output = state
            .get("output")
            .and_then(Value::as_str)
            .or_else(|| state.get("error").and_then(Value::as_str))
            .unwrap_or_default();
        let mut done = Call::new(&id);
        done.status = Some(status.to_owned());
        done.raw_output = Some(json!(output));
        done.content = if output.is_empty() || name == "read" {
            Vec::new()
        } else {
            vec![json!({"type": "content", "content": {"type": "text", "text": output}})]
        };
        self.rec.update(done.finish(), stamp.clone());
    }
}

/// `header` with every optional field the format refuses dropped: they come from output this code does not control.
fn tidied(mut header: Header) -> Header {
    let ok = |header: &Header| header.to_line().is_ok();
    for clear in [
        (|h: &mut Header| h.origin.session_id = None) as fn(&mut Header),
        |h| h.origin.version = None,
        |h| h.cwd = None,
        |h| h.model = None,
        |h| h.effort = None,
        |h| h.mode = None,
        |h| h.created_at = None,
    ] {
        if ok(&header) {
            break;
        }
        clear(&mut header);
    }
    header
}

/// The session `export` printed as a history whose id is `id` (a UUID, or the host's own id that is turned into one).
///
/// # Errors
///
/// The text is not what `opencode export` prints, or the history it makes is one the format refuses.
pub fn export(text_in: &str, id: &str) -> Result<Document, String> {
    let root: Value = serde_json::from_str(text_in)
        .map_err(|_| "the program did not print a session as JSON".to_owned())?;
    let info = root
        .get("info")
        .filter(|info| info.is_object())
        .ok_or_else(|| "the output has no session information".to_owned())?;
    let messages = root
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "the output has no messages".to_owned())?;
    let mut exporter = Exporter::default();
    for message in messages {
        match message.pointer("/info/role").and_then(Value::as_str) {
            Some("user") => exporter.user(message),
            Some("assistant") => exporter.assistant(message),
            _ => {}
        }
    }
    exporter.close_turn();
    let model_of_session = || {
        let model = info.get("model")?;
        Some(format!(
            "{}/{}",
            model.get("providerID")?.as_str()?,
            model.get("id")?.as_str()?
        ))
    };
    let host_id = if is_uuid(id) {
        id.to_owned()
    } else {
        derived_uuid(id, HARNESS, 0)
    };
    let mut header = Header::new(host_id, Origin::new(HARNESS));
    header.origin.version = text(info, "version");
    header.origin.session_id = text(info, "id");
    header.created_at = time_of(info.pointer("/time/created"));
    header.cwd = text(info, "directory");
    header.mode = text(info, "agent");
    header.model = exporter.model.clone().or_else(model_of_session);
    header.effort = info
        .pointer("/model/variant")
        .and_then(Value::as_str)
        .filter(|variant| *variant != "default")
        .map(str::to_owned);
    let mut document = Document::new(tidied(header));
    document.lines = exporter.rec.finish();
    document
        .seal()
        .map_err(|error| format!("the session is not a history: {error}"))?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ids_opencode_makes_reach_its_command_line() {
        for good in ["ses_e11822fdeffeQonF7BHkuhxiss", "ses_1"] {
            assert!(is_native_id(good), "{good}");
        }
        for bad in [
            "", "ses_", "--help", "-s", "ses_a b", "ses_../x", "ses_a;b", "SES_a",
        ] {
            assert!(!is_native_id(bad), "{bad:?}");
        }
        assert!(!is_native_id(&format!("ses_{}", "a".repeat(MAX_ID))));
    }

    #[test]
    fn a_title_longer_than_a_line_is_cut_to_its_first() {
        assert_eq!(one_line("first\nsecond"), "first");
    }

    fn session(messages: Value) -> String {
        json!({"info": {"id": "ses_x", "directory": "/w", "version": "1.19.6",
                        "model": {"id": "m", "providerID": "p", "variant": "high"}},
               "messages": messages})
        .to_string()
    }

    fn kinds(document: &Document) -> Vec<String> {
        use coqui_acp_history::Body;
        document
            .lines
            .iter()
            .map(|line| match &line.body {
                Body::Prompt { .. } => "prompt".to_owned(),
                Body::Update { update } => update["sessionUpdate"]
                    .as_str()
                    .map_or_else(String::new, |kind| {
                        format!("{kind}:{}", update["status"].as_str().unwrap_or_default())
                    }),
                Body::Cancel => "cancel".to_owned(),
                Body::TurnEnd { stop, .. } => format!("end:{}", stop.name()),
                Body::Compaction { .. } => "compaction".to_owned(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    // The shapes below are the published types', not measured ones (see the module's note).
    #[test]
    fn a_failed_call_a_cut_short_call_an_abort_and_a_summary_are_told_as_such() {
        let text = session(json!([
            {"info": {"role": "user", "id": "m1"}, "parts": [
                {"type": "text", "text": "go"},
                {"type": "file", "filename": "a.png", "mime": "image/png", "url": "file:///a.png"}]},
            {"info": {"role": "assistant", "id": "m2", "finish": "tool-calls"}, "parts": [
                {"type": "tool", "tool": "read", "callID": "c", "state": {"status": "error", "input": {"filePath": "/x"}, "error": "no such file"}},
                {"type": "tool", "tool": "bash", "callID": "c", "state": {"status": "running", "input": {"command": "sleep 9"}}}]},
            {"info": {"role": "assistant", "id": "m3", "error": {"name": "MessageAbortedError"}}, "parts": []},
            {"info": {"role": "user", "id": "m4"}, "parts": [{"type": "text", "text": "again"}]},
            {"info": {"role": "assistant", "id": "m5", "summary": true}, "parts": [{"type": "text", "text": "so far: a file"}]},
        ]));
        let document = export(&text, "ses_x").expect("a history");
        document.check().expect("it keeps the rules");
        assert_eq!(
            kinds(&document),
            [
                "prompt",
                "tool_call:pending",
                "tool_call_update:failed",
                "tool_call:pending",
                "cancel",
                "end:cancelled",
                "prompt",
                "compaction",
                "end:end_turn"
            ]
        );
        assert_eq!(document.header.effort.as_deref(), Some("high"));
        assert_eq!(document.header.model.as_deref(), Some("p/m"));
        // The id is not a UUID, so the history's own is made from it, the same each time.
        assert!(is_uuid(&document.header.id));
        assert_eq!(
            document.header.id,
            export(&text, "ses_x").expect("again").header.id
        );
        let second = document
            .lines
            .iter()
            .filter_map(|line| match &line.body {
                coqui_acp_history::Body::Update { update }
                    if update["sessionUpdate"] == "tool_call" =>
                {
                    update["toolCallId"].as_str().map(str::to_owned)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(second, ["c", "c~2"]);
    }

    #[test]
    fn a_uuid_and_the_id_it_has_in_opencodes_store_are_each_others_and_nothing_else_is() {
        let uuid = "7d8c1f4e-2b9a-4c63-8f10-5a6b7c8d9e0f";
        let native = native_id_of(uuid).expect("an id");
        assert_eq!(native, "ses_7d8c1f4e2b9a4c638f105a6b7c8d9e0f");
        assert!(is_native_id(&native));
        assert_eq!(host_id_of(&native).as_deref(), Some(uuid));
        // Whatever spelling the host sends, the same session.
        assert_eq!(native_id_of(&uuid.to_ascii_uppercase()), Some(native));
        // An id OpenCode made itself, or anything else, is no UUID's.
        for other in [
            "ses_e11822fdeffeQonF7BHkuhxiss",
            "ses_7D8C1F4E2B9A4C638F105A6B7C8D9E0F",
            "ses_7d8c1f4e2b9a4c638f105a6b7c8d9e0",
            "7d8c1f4e2b9a4c638f105a6b7c8d9e0f",
            "ses_",
        ] {
            assert_eq!(host_id_of(other), None, "{other}");
        }
        for not_uuid in ["ses_x", "", "--help"] {
            assert_eq!(native_id_of(not_uuid), None, "{not_uuid}");
        }
    }

    #[test]
    fn what_is_not_a_session_is_refused() {
        assert!(export("not json", "x").is_err());
        assert!(export("{}", "x").is_err());
        assert!(export("{\"info\": {}}", "x").is_err());
    }
}
