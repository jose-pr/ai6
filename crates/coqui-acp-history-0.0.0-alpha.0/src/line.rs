//! The lines after the header.

use agent_client_protocol::schema::v1::StopReason;
use serde_json::{Map, Value, json};

use crate::HistoryError;
use crate::settings::PARSER_DEPTH;
use crate::shape::{Fields, is_timestamp};

/// The `_meta` key under which a codec keeps what only a resume by the same harness needs: the
/// native tool-use ids paired with ACP's, and the exact native input. Opaque to this crate, and
/// never required to read a history on another harness.
pub const NATIVE_META_KEY: &str = "dev.boriken.history/native";

/// The `_meta` key of a content block whose bytes were left out: an object with the content's
/// `sha256` (`sha256:` and 64 hexadecimal digits) and its size in `bytes`.
pub const ATTACHMENT_META_KEY: &str = "dev.boriken.history/attachment";

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stop {
    /// One of ACP's stop reasons.
    Acp(StopReason),
    /// The turn failed; [`Body::TurnEnd::error`] may say how.
    Error,
}

impl Stop {
    /// The reason as the file spells it: ACP's `end_turn` and the rest, or `error`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Acp(StopReason::EndTurn) => "end_turn",
            Self::Acp(StopReason::MaxTokens) => "max_tokens",
            Self::Acp(StopReason::MaxTurnRequests) => "max_turn_requests",
            Self::Acp(StopReason::Refusal) => "refusal",
            Self::Acp(StopReason::Cancelled) => "cancelled",
            // ACP's enum can gain a reason; the format names one only by a version bump.
            Self::Acp(_) => "unknown",
            Self::Error => "error",
        }
    }

    /// The reason a file's `stopReason` spells, or `None` for a name the format does not define.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "end_turn" => Self::Acp(StopReason::EndTurn),
            "max_tokens" => Self::Acp(StopReason::MaxTokens),
            "max_turn_requests" => Self::Acp(StopReason::MaxTurnRequests),
            "refusal" => Self::Acp(StopReason::Refusal),
            "cancelled" => Self::Acp(StopReason::Cancelled),
            "error" => Self::Error,
            _ => return None,
        })
    }
}

/// What set a compaction off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Trigger {
    /// The harness compacted by itself.
    Auto,
    /// The person asked.
    Manual,
}

/// What one line says.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Body {
    /// `session/prompt`: the client's prompt, as ACP content blocks.
    Prompt {
        /// The blocks; each an object with a `type`.
        prompt: Vec<Value>,
    },
    /// `session/update`: a notification from the agent, in ACP's shape.
    Update {
        /// The update: an object with a `sessionUpdate`.
        update: Value,
    },
    /// `session/cancel`: the client cancelled the turn.
    Cancel,
    /// `session/request_permission`: the agent asked.
    PermissionRequest {
        /// The call, an object with a `toolCallId`.
        tool_call: Value,
        /// The options offered, each an object with an `optionId`.
        options: Vec<Value>,
    },
    /// `history/permission_answer`: the client's answer to the request for `tool_call_id`.
    PermissionAnswer {
        /// The call the request named.
        tool_call_id: String,
        /// ACP's outcome: `selected` with an `optionId`, or `cancelled`.
        outcome: Value,
    },
    /// `history/turn_end`: the turn's prompt response.
    TurnEnd {
        /// How it ended.
        stop: Stop,
        /// What failed, when the turn failed.
        error: Option<String>,
    },
    /// `history/compaction`: the harness folded the history so far into a summary.
    Compaction {
        /// The harness's own summary text, when it exposes one.
        summary: Option<String>,
        /// What set it off.
        trigger: Option<Trigger>,
        /// The context size before compaction, in tokens, when the harness says.
        pre_tokens: Option<u64>,
    },
}

impl Body {
    /// The line's `method`.
    #[must_use]
    pub fn method(&self) -> &'static str {
        match self {
            Self::Prompt { .. } => "session/prompt",
            Self::Update { .. } => "session/update",
            Self::Cancel => "session/cancel",
            Self::PermissionRequest { .. } => "session/request_permission",
            Self::PermissionAnswer { .. } => "history/permission_answer",
            Self::TurnEnd { .. } => "history/turn_end",
            Self::Compaction { .. } => "history/compaction",
        }
    }
}

/// One line after the header.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Line {
    /// When it happened, as an RFC 3339 timestamp.
    pub at: Option<String>,
    /// Anything else, as an object, such as [`NATIVE_META_KEY`]; the format reads none of it.
    pub meta: Option<Value>,
    /// What it says.
    pub body: Body,
}

impl Line {
    /// A line saying `body`, with no time and no `_meta`.
    #[must_use]
    pub fn new(body: Body) -> Self {
        Self {
            at: None,
            meta: None,
            body,
        }
    }

    /// A prompt of `blocks`.
    #[must_use]
    pub fn prompt(blocks: Vec<Value>) -> Self {
        Self::new(Body::Prompt { prompt: blocks })
    }

    /// A prompt of one text block.
    #[must_use]
    pub fn text_prompt(text: &str) -> Self {
        Self::prompt(vec![json!({"type": "text", "text": text})])
    }

    /// An update from the agent.
    #[must_use]
    pub fn update(update: Value) -> Self {
        Self::new(Body::Update { update })
    }

    /// The turn's end.
    #[must_use]
    pub fn turn_end(stop: Stop) -> Self {
        Self::new(Body::TurnEnd { stop, error: None })
    }

    /// A compaction boundary holding the harness's `summary`.
    #[must_use]
    pub fn compaction(summary: impl Into<String>) -> Self {
        Self::new(Body::Compaction {
            summary: Some(summary.into()),
            trigger: None,
            pre_tokens: None,
        })
    }

    /// The same line stamped `at`.
    #[must_use]
    pub fn at(mut self, at: impl Into<String>) -> Self {
        self.at = Some(at.into());
        self
    }

    /// The same line with `meta`.
    #[must_use]
    pub fn meta(mut self, meta: Value) -> Self {
        self.meta = Some(meta);
        self
    }

    /// The line as its JSON text, with no line feed. Keys inside ACP payloads come out sorted.
    ///
    /// # Errors
    ///
    /// [`HistoryError::Shape`] when the line is not one the format accepts; the writer's check is
    /// the reader's.
    pub fn to_json(&self) -> Result<String, HistoryError> {
        // Measure before copying anything: a value nested deeper than the parser reads is refused
        // here, so no walk of the line recurses on it.
        if self.payload_depth() > PARSER_DEPTH {
            return Err(HistoryError::too_deep(0, PARSER_DEPTH));
        }
        let mut envelope = Map::new();
        envelope.insert("method".into(), Value::String(self.body.method().into()));
        if let Some(params) = params_of(&self.body) {
            envelope.insert("params".into(), params);
        }
        if let Some(at) = &self.at {
            envelope.insert("at".into(), Value::String(at.clone()));
        }
        if let Some(meta) = &self.meta {
            envelope.insert("_meta".into(), meta.clone());
        }
        crate::canon::to_string(
            &Value::Object(envelope),
            &["method", "params", "at", "_meta"],
        )
        .ok_or(HistoryError::too_deep(0, PARSER_DEPTH))
    }

    /// How deep the line nests as written: the line is one level, `params` a second, and the
    /// payload below them.
    pub(crate) fn payload_depth(&self) -> u64 {
        use crate::strict::depth;
        let deepest = |values: &[Value]| values.iter().map(depth).max().unwrap_or(0);
        let params = match &self.body {
            Body::Prompt { prompt } => 3 + deepest(prompt),
            Body::Update { update } => 2 + depth(update),
            Body::PermissionRequest { tool_call, options } => {
                (2 + depth(tool_call)).max(3 + deepest(options))
            }
            Body::PermissionAnswer { outcome, .. } => 2 + depth(outcome),
            Body::TurnEnd { .. } | Body::Compaction { .. } => 2,
            Body::Cancel => 1,
        };
        params.max(self.meta.as_ref().map_or(1, |meta| 1 + depth(meta)))
    }

    /// Checks a parsed body line and takes its parts out of it, copying only what is small.
    pub(crate) fn from_value(mut value: Value, line: u64) -> Result<Self, HistoryError> {
        let method = Self::check(&value, line)?;
        let Value::Object(root) = &mut value else {
            return Err(HistoryError::UnknownLine { line });
        };
        let at = match root.remove("at") {
            Some(Value::String(at)) => Some(at),
            _ => None,
        };
        let meta = root.remove("_meta");
        let mut params = match root.remove("params") {
            Some(Value::Object(params)) => params,
            _ => Map::new(),
        };
        let body = match method {
            Method::Cancel => Body::Cancel,
            Method::Prompt => Body::Prompt {
                prompt: take_array(&mut params, "prompt"),
            },
            Method::Update => Body::Update {
                update: take(&mut params, "update"),
            },
            Method::PermissionRequest => Body::PermissionRequest {
                tool_call: take(&mut params, "toolCall"),
                options: take_array(&mut params, "options"),
            },
            Method::PermissionAnswer => Body::PermissionAnswer {
                tool_call_id: take_string(&mut params, "toolCallId").unwrap_or_default(),
                outcome: take(&mut params, "outcome"),
            },
            Method::TurnEnd => Body::TurnEnd {
                stop: take_string(&mut params, "stopReason")
                    .and_then(|name| Stop::from_name(&name))
                    .unwrap_or(Stop::Error),
                error: take_string(&mut params, "error"),
            },
            Method::Compaction => Body::Compaction {
                summary: take_string(&mut params, "summary"),
                trigger: match take_string(&mut params, "trigger").as_deref() {
                    Some("auto") => Some(Trigger::Auto),
                    Some("manual") => Some(Trigger::Manual),
                    _ => None,
                },
                pre_tokens: match params.remove("preTokens") {
                    Some(Value::Number(n)) => n.as_u64(),
                    _ => None,
                },
            },
        };
        Ok(Self { at, meta, body })
    }

    /// Checks a parsed body line without taking anything from it.
    fn check(value: &Value, line: u64) -> Result<Method, HistoryError> {
        let root = Fields::root(value, line)?;
        if root.has("format") {
            return Err(HistoryError::Order {
                line,
                rule: crate::OrderRule::HeaderRepeated,
            });
        }
        let Some(Value::String(method)) = root.raw("method") else {
            return Err(HistoryError::UnknownLine { line });
        };
        let method = match method.as_str() {
            "session/prompt" => Method::Prompt,
            "session/update" => Method::Update,
            "session/cancel" => Method::Cancel,
            "session/request_permission" => Method::PermissionRequest,
            "history/permission_answer" => Method::PermissionAnswer,
            "history/turn_end" => Method::TurnEnd,
            "history/compaction" => Method::Compaction,
            _ => return Err(HistoryError::UnknownLine { line }),
        };
        root.only(&["method", "params", "at", "_meta"])?;

        let at = root.string("at", 40, "an RFC 3339 timestamp")?;
        if at.is_some_and(|t| !is_timestamp(t)) {
            return Err(root.shape("at", "an RFC 3339 timestamp"));
        }
        if root.raw("_meta").is_some_and(|meta| !meta.is_object()) {
            return Err(root.shape("_meta", "an object"));
        }

        if method == Method::Cancel {
            if root.has("params") {
                return Err(HistoryError::UnknownField {
                    line,
                    pointer: String::new(),
                });
            }
        } else {
            let params = root.require_object("params")?;
            check_params(method, &params)?;
        }
        Ok(method)
    }
}

/// The kinds of body line, by `method`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Prompt,
    Update,
    Cancel,
    PermissionRequest,
    PermissionAnswer,
    TurnEnd,
    Compaction,
}

fn take(map: &mut Map<String, Value>, key: &str) -> Value {
    map.remove(key).unwrap_or(Value::Null)
}

fn take_array(map: &mut Map<String, Value>, key: &str) -> Vec<Value> {
    match map.remove(key) {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    }
}

fn take_string(map: &mut Map<String, Value>, key: &str) -> Option<String> {
    match map.remove(key) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}

fn check_params(method: Method, params: &Fields<'_>) -> Result<(), HistoryError> {
    match method {
        Method::Prompt => {
            params.only(&["prompt"])?;
            params.tagged_array("prompt", "type", true)?;
        }
        Method::Update => {
            params.only(&["update"])?;
            let update = params.require_object("update")?;
            update.require_string("sessionUpdate", 64, "a non-empty string")?;
        }
        Method::PermissionRequest => {
            params.only(&["toolCall", "options"])?;
            let call = params.require_object("toolCall")?;
            call.require_string("toolCallId", 512, "a non-empty string")?;
            params.tagged_array("options", "optionId", true)?;
        }
        Method::PermissionAnswer => {
            params.only(&["toolCallId", "outcome"])?;
            params.require_string("toolCallId", 512, "a non-empty string")?;
            let outcome = params.require_object("outcome")?;
            match outcome.require_string("outcome", 16, "selected or cancelled")? {
                "selected" => {
                    outcome.require_string("optionId", 256, "a non-empty string")?;
                }
                "cancelled" => {}
                _ => return Err(outcome.shape("outcome", "selected or cancelled")),
            }
        }
        Method::TurnEnd => {
            params.only(&["stopReason", "error"])?;
            let name = params.require_string("stopReason", 32, "a stop reason")?;
            if Stop::from_name(name).is_none() {
                return Err(params.shape("stopReason", "a stop reason"));
            }
            params.string("error", 65_536, "a non-empty string")?;
        }
        Method::Compaction => {
            params.only(&["summary", "trigger", "preTokens"])?;
            if !matches!(
                params.string("trigger", 16, "auto or manual")?,
                None | Some("auto" | "manual")
            ) {
                return Err(params.shape("trigger", "auto or manual"));
            }
            params.string("summary", usize::MAX, "a non-empty string")?;
            params.unsigned("preTokens")?;
        }
        Method::Cancel => {}
    }
    Ok(())
}

/// The `params` a body is written with; `None` for a line that has none.
fn params_of(body: &Body) -> Option<Value> {
    Some(match body {
        Body::Prompt { prompt } => json!({ "prompt": prompt }),
        Body::Update { update } => json!({ "update": update }),
        Body::Cancel => return None,
        Body::PermissionRequest { tool_call, options } => {
            json!({ "toolCall": tool_call, "options": options })
        }
        Body::PermissionAnswer {
            tool_call_id,
            outcome,
        } => {
            json!({ "toolCallId": tool_call_id, "outcome": outcome })
        }
        Body::TurnEnd { stop, error } => {
            let mut map = Map::new();
            map.insert("stopReason".into(), Value::String(stop.name().into()));
            if let Some(error) = error {
                map.insert("error".into(), Value::String(error.clone()));
            }
            Value::Object(map)
        }
        Body::Compaction {
            summary,
            trigger,
            pre_tokens,
        } => {
            let mut map = Map::new();
            if let Some(summary) = summary {
                map.insert("summary".into(), Value::String(summary.clone()));
            }
            if let Some(trigger) = trigger {
                let name = match trigger {
                    Trigger::Auto => "auto",
                    Trigger::Manual => "manual",
                };
                map.insert("trigger".into(), Value::String(name.into()));
            }
            if let Some(tokens) = pre_tokens {
                map.insert("preTokens".into(), Value::from(*tokens));
            }
            Value::Object(map)
        }
    })
}
