//! The handlers: a call in, a result out.

use alloc::string::String;
use alloc::vec::Vec;

use serde_json::{Map, Value, json};

use super::read::{CallerFacts, choose, read};
use super::search::{SearchBackend, SearchMode, search};
use super::serve_text::{Call, Renderers, served_text};
use super::settings::KnowledgeSettings;
use super::text::{inert, knowledge_lines};
use super::tools::{ToolName, knowledge_arguments, knowledge_tools};
use super::tree::{FileFormat, KnowledgeOverlay, SkipReason};
use super::{ErrorClass, KnowledgeError};

/// A call of a tool, as the consumer's MCP server received it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ToolCall {
    /// The tool's name.
    pub name: String,
    /// Its arguments.
    pub arguments: Value,
}

impl ToolCall {
    /// A call of `name` with `arguments`.
    #[must_use]
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            name: name.into(),
            arguments,
        }
    }
}

/// What a tool answers: a JSON object, or the error object, to return as the call's
/// `structuredContent` and as the same JSON text in one `text` block.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ToolResult {
    /// The result, or `{"error": {"class", "message"}}`.
    pub structured: Value,
    /// Whether the call failed.
    pub is_error: bool,
}

impl ToolResult {
    /// The result as JSON text, as the `text` block carries it.
    #[must_use]
    pub fn text(&self) -> String {
        self.structured.to_string()
    }

    /// The result as MCP's `tools/call` result: `content`, `structuredContent` and `isError`.
    #[must_use]
    pub fn to_mcp(&self) -> Value {
        json!({
            "content": [{"type": "text", "text": self.text()}],
            "structuredContent": self.structured,
            "isError": self.is_error,
        })
    }
}

/// A handler's outcome as a [`ToolResult`].
#[must_use]
pub fn kb_tool_result(outcome: Result<Value, KnowledgeError>) -> ToolResult {
    match outcome {
        Ok(structured) => ToolResult {
            structured,
            is_error: false,
        },
        Err(error) => ToolResult {
            structured: json!({"error": {
                "class": error.class.as_str(),
                "message": error.message,
            }}),
            is_error: true,
        },
    }
}

fn list(
    overlays: &[KnowledgeOverlay],
    arguments: &Map<String, Value>,
    modes: &[SearchMode],
    settings: &KnowledgeSettings,
    caller: &CallerFacts,
    engines: &Renderers,
) -> Result<Value, KnowledgeError> {
    let mut listed = Vec::new();
    let empty = Map::new();
    for overlay in choose(overlays, arguments.get("overlay").and_then(Value::as_str))? {
        let set = overlay.set(settings);
        let call = Call {
            overlay,
            set: &set,
            context: &empty,
            caller,
            settings,
            engines,
        };
        let mut files = Vec::new();
        let mut skipped: Vec<(String, SkipReason)> = set
            .skipped
            .iter()
            .map(|skip| (skip.path.clone(), skip.reason))
            .collect();
        for (path, file) in &set.files {
            let Ok(text) = served_text(&call, path) else {
                skipped.push(((*path).to_owned(), SkipReason::Render));
                continue;
            };
            let mut entry = json!({
                "path": path,
                "lines": knowledge_lines(&text).len(),
                "bytes": text.len(),
            });
            if let FileFormat::Engine { name, options } = &file.format {
                entry["template"] = json!(name);
                entry["rendered"] = json!(engines.renders(name, options));
            }
            files.push(entry);
        }
        skipped.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
        let skipped: Vec<Value> = skipped
            .iter()
            .map(|(path, reason)| json!({"path": inert(path), "reason": reason.as_str()}))
            .collect();
        listed.push(json!({
            "overlay": overlay.identity,
            "served": overlay.served,
            "files": files,
            "skipped": skipped,
        }));
    }
    let names = engines.names();
    let mut result = json!({"overlays": listed, "engines": names});
    let declared: Vec<&str> = SearchMode::ALL
        .into_iter()
        .filter(|mode| modes.contains(mode))
        .map(SearchMode::as_str)
        .collect();
    if !declared.is_empty() {
        result["search"] = json!({"modes": declared});
    }
    Ok(result)
}

/// What a server answers a call of `kb_list` or `kb_read` over `overlays`, the session's active
/// overlays in composition order; `modes` are the search modes it declares, which only `kb_list`
/// reports; `engines` are the template engines it renders with. `kb_search` is the [`KnowledgeSession`]'s.
///
/// # Errors
///
/// The refusal, as a [`KnowledgeError`].
pub fn knowledge_call(
    tool: &str,
    overlays: &[KnowledgeOverlay],
    arguments: &Value,
    modes: &[SearchMode],
    engines: &Renderers,
    settings: &KnowledgeSettings,
    caller: &CallerFacts,
) -> Result<Value, KnowledgeError> {
    let arguments = knowledge_arguments(tool, arguments, modes, settings)?;
    match ToolName::parse(tool) {
        Some(ToolName::List) => list(overlays, &arguments, modes, settings, caller, engines),
        Some(ToolName::Read) => read(overlays, &arguments, settings, caller, engines),
        _ => Err(KnowledgeError::new(
            ErrorClass::InvalidArguments,
            alloc::format!("no tool is named {tool}"),
        )),
    }
}

/// The arguments of a `kb_read`.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ReadRequest {
    /// The path to read, relative to an overlay's root.
    pub path: String,
    /// The first line, from 1; the first line when none.
    pub from: Option<u64>,
    /// The last line; the end when none.
    pub to: Option<u64>,
    /// The overlay asked; every active overlay when none.
    pub overlay: Option<String>,
    /// What the call's `context` gives a template.
    pub context: Option<Map<String, Value>>,
}

impl ReadRequest {
    /// A request for the whole file at `path`.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            ..Self::default()
        }
    }

    /// Reads from line `from`.
    #[must_use]
    pub fn from(mut self, from: u64) -> Self {
        self.from = Some(from);
        self
    }

    /// Reads to line `to`.
    #[must_use]
    pub fn to(mut self, to: u64) -> Self {
        self.to = Some(to);
        self
    }

    /// Asks only `overlay`.
    #[must_use]
    pub fn overlay(mut self, overlay: impl Into<String>) -> Self {
        self.overlay = Some(overlay.into());
        self
    }

    /// Gives a template `context`.
    #[must_use]
    pub fn context(mut self, context: Map<String, Value>) -> Self {
        self.context = Some(context);
        self
    }

    fn arguments(&self) -> Value {
        let mut map = Map::new();
        map.insert("path".into(), json!(self.path));
        if let Some(from) = self.from {
            map.insert("from".into(), json!(from));
        }
        if let Some(to) = self.to {
            map.insert("to".into(), json!(to));
        }
        if let Some(overlay) = &self.overlay {
            map.insert("overlay".into(), json!(overlay));
        }
        if let Some(context) = &self.context {
            map.insert("context".into(), Value::Object(context.clone()));
        }
        Value::Object(map)
    }
}

/// `kb_read`: the lines of one file of `overlays`, a template rendered for this call from the
/// facts of `caller` and the request's context, so a consumer never renders one itself.
#[must_use]
pub fn kb_read(
    overlays: &[KnowledgeOverlay],
    request: &ReadRequest,
    caller: &CallerFacts,
    settings: &KnowledgeSettings,
) -> ToolResult {
    kb_tool_result(knowledge_call(
        "kb_read",
        overlays,
        &request.arguments(),
        &[],
        &Renderers::all(),
        settings,
        caller,
    ))
}

/// `kb_list`: each overlay's served files and what is skipped, of `overlay` only when it is given;
/// `modes` are the search modes the server declares.
#[must_use]
pub fn kb_list(
    overlays: &[KnowledgeOverlay],
    overlay: Option<&str>,
    modes: &[SearchMode],
    settings: &KnowledgeSettings,
) -> ToolResult {
    let arguments = overlay.map_or_else(|| json!({}), |name| json!({"overlay": name}));
    kb_tool_result(knowledge_call(
        "kb_list",
        overlays,
        &arguments,
        modes,
        &Renderers::all(),
        settings,
        &CallerFacts::new(),
    ))
}

/// One session's knowledge tools: its active overlays, the facts of its caller and, when it offers
/// `kb_search`, the backend that finds passages.
#[derive(Clone)]
#[non_exhaustive]
pub struct KnowledgeSession<'a> {
    overlays: &'a [KnowledgeOverlay],
    settings: &'a KnowledgeSettings,
    caller: &'a CallerFacts,
    backend: Option<&'a dyn SearchBackend>,
    engines: Renderers,
}

impl core::fmt::Debug for KnowledgeSession<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("KnowledgeSession")
            .field("overlays", &self.overlays.len())
            .field("search", &self.backend.is_some())
            .finish_non_exhaustive()
    }
}

impl<'a> KnowledgeSession<'a> {
    /// A session over `overlays`, in composition order.
    #[must_use]
    pub fn new(
        overlays: &'a [KnowledgeOverlay],
        settings: &'a KnowledgeSettings,
        caller: &'a CallerFacts,
    ) -> Self {
        Self {
            overlays,
            settings,
            caller,
            backend: None,
            engines: Renderers::all(),
        }
    }

    /// Offers `kb_search` through `backend`, which declares at least one mode.
    #[must_use]
    pub fn search(mut self, backend: &'a dyn SearchBackend) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Renders only these engines; the rest are served with their context, unrendered.
    #[must_use]
    pub fn engines(mut self, engines: Renderers) -> Self {
        self.engines = engines;
        self
    }

    fn modes(&self) -> Vec<SearchMode> {
        self.backend.map(SearchBackend::modes).unwrap_or_default()
    }

    /// The tools to list: `kb_list`, `kb_read` and, when a backend declares a mode, `kb_search`.
    #[must_use]
    pub fn tools(&self) -> Vec<Value> {
        knowledge_tools(&self.modes(), self.settings)
    }

    /// Answers a call.
    #[must_use]
    pub fn call(&self, call: &ToolCall) -> ToolResult {
        let modes = self.modes();
        kb_tool_result(self.outcome(call, &modes))
    }

    fn outcome(&self, call: &ToolCall, modes: &[SearchMode]) -> Result<Value, KnowledgeError> {
        if let (Some(ToolName::Search), Some(backend)) = (ToolName::parse(&call.name), self.backend)
            && !modes.is_empty()
        {
            let arguments = knowledge_arguments(&call.name, &call.arguments, modes, self.settings)?;
            return search(
                self.overlays,
                &arguments,
                backend,
                self.settings,
                self.caller,
                &self.engines,
            );
        }
        knowledge_call(
            &call.name,
            self.overlays,
            &call.arguments,
            modes,
            &self.engines,
            self.settings,
            self.caller,
        )
    }
}
