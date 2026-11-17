//! `kb_search`: the modes a server declares, what a backend answers, what the handler checks and
//! orders of it, and the checks a conformance run applies to an answer.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

use serde_json::{Map, Value, json};

use super::read::{CallerFacts, choose, path_problem};
use super::serve_text::{Call, Renderers, served_lines};
use super::settings::KnowledgeSettings;
use super::text::inert;
use super::tools::knowledge_arguments;
use super::tree::KnowledgeOverlay;
use super::{ErrorClass, KnowledgeError};

/// How a query is matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum SearchMode {
    /// The words used, with no model.
    Words,
    /// What is meant, with vectors from an embeddings provider.
    Meaning,
    /// What a language model the person chose picks or orders from candidates another mode found.
    Llm,
}

impl SearchMode {
    /// The modes in the order a server declares them.
    pub const ALL: [Self; 3] = [Self::Words, Self::Meaning, Self::Llm];

    /// The mode's name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Words => "words",
            Self::Meaning => "meaning",
            Self::Llm => "llm",
        }
    }

    /// The mode named `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == name)
    }
}

/// One search, for one overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SearchRequest {
    /// The query, as given.
    pub query: String,
    /// The most passages to give.
    pub top: usize,
    /// A file, or a folder, to which the passages are limited.
    pub path: Option<String>,
    /// The mode asked for; `None` is `auto`, which the backend answers by a mode it chooses.
    pub mode: Option<SearchMode>,
}

/// A passage found.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Passage {
    /// The served, plain file it lies in.
    pub path: String,
    /// The Markdown headings above it, outermost first, joined by ` > `.
    pub heading: String,
    /// Its first line, from 1.
    pub from_line: usize,
    /// Its last line, inclusive.
    pub to_line: usize,
    /// How well it matches; comparable only within one answer.
    pub score: f64,
    /// Its text: lines `from_line` to `to_line`, a line longer than a passage possibly cut.
    pub text: String,
}

/// What a backend found for one overlay.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SearchAnswer {
    /// The mode that answered.
    pub mode: SearchMode,
    /// Why this mode answered, when it is not the one asked for.
    pub note: Option<String>,
    /// The passages, in any order.
    pub passages: Vec<Passage>,
}

impl SearchAnswer {
    /// An answer by `mode`.
    #[must_use]
    pub fn new(mode: SearchMode, passages: Vec<Passage>) -> Self {
        Self {
            mode,
            note: None,
            passages,
        }
    }

    /// Says why this mode answered.
    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// An overlay a backend cannot answer now, which does not fail the call.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SearchFailure {
    /// `unavailable` or `mode-unavailable`.
    pub class: ErrorClass,
    /// What happened, as text for a person.
    pub message: String,
}

impl SearchFailure {
    /// An index that is still being made, or failed.
    #[must_use]
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::Unavailable,
            message: message.into(),
        }
    }

    /// A named mode that is not available.
    #[must_use]
    pub fn mode_unavailable(message: impl Into<String>) -> Self {
        Self {
            class: ErrorClass::ModeUnavailable,
            message: message.into(),
        }
    }
}

/// What finds passages. A consumer supplies one to offer `kb_search`.
pub trait SearchBackend {
    /// The modes it declares, at least one.
    fn modes(&self) -> Vec<SearchMode>;

    /// The passages of `overlay` that best answer `request`. The handler drops any passage that
    /// is not of a served, plain file within the path limit, orders them and keeps `top`.
    ///
    /// # Errors
    ///
    /// A [`SearchFailure`] where the overlay cannot answer now.
    fn search(
        &self,
        overlay: &KnowledgeOverlay,
        request: &SearchRequest,
    ) -> Result<SearchAnswer, SearchFailure>;
}

fn within(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn compare(left: &Passage, right: &Passage) -> Ordering {
    right
        .score
        .partial_cmp(&left.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| left.path.as_bytes().cmp(right.path.as_bytes()))
        .then_with(|| left.from_line.cmp(&right.from_line))
        .then_with(|| left.to_line.cmp(&right.to_line))
}

/// The lines of the served text of each file of `overlay` that can be served with no context.
fn served_of(
    overlay: &KnowledgeOverlay,
    settings: &KnowledgeSettings,
    caller: &CallerFacts,
    engines: &Renderers,
) -> BTreeMap<String, Vec<String>> {
    let set = overlay.set(settings);
    let empty = Map::new();
    served_lines(&Call {
        overlay,
        set: &set,
        context: &empty,
        caller,
        settings,
        engines,
    })
}

/// The answer of a `kb_search` whose arguments were checked.
pub(crate) fn search(
    overlays: &[KnowledgeOverlay],
    arguments: &Map<String, Value>,
    backend: &dyn SearchBackend,
    settings: &KnowledgeSettings,
    caller: &CallerFacts,
    engines: &Renderers,
) -> Result<Value, KnowledgeError> {
    let prefix = arguments.get("path").and_then(Value::as_str);
    if let Some(prefix) = prefix
        && path_problem(prefix)
    {
        return Err(KnowledgeError::new(ErrorClass::InvalidPath, inert(prefix)));
    }
    let declared = backend.modes();
    let mode = arguments
        .get("mode")
        .and_then(Value::as_str)
        .and_then(SearchMode::parse);
    let request = SearchRequest {
        query: arguments["query"].as_str().unwrap_or_default().to_owned(),
        top: arguments
            .get("top")
            .and_then(Value::as_u64)
            .map_or(5, |top| usize::try_from(top).unwrap_or(usize::MAX)),
        path: prefix.map(str::to_owned),
        mode,
    };
    let wanted = arguments.get("overlay").and_then(Value::as_str);
    let mut results = Vec::new();
    for overlay in choose(overlays, wanted)? {
        let answer = backend.search(overlay, &request);
        let served = served_of(overlay, settings, caller, engines);
        results.push(entry(overlay, answer, &request, &declared, &served));
    }
    Ok(json!({"results": results}))
}

fn entry(
    overlay: &KnowledgeOverlay,
    answer: Result<SearchAnswer, SearchFailure>,
    request: &SearchRequest,
    declared: &[SearchMode],
    served: &BTreeMap<String, Vec<String>>,
) -> Value {
    let failed = |class: ErrorClass, message: &str| {
        json!({
            "overlay": overlay.identity,
            "error": {"class": class.as_str(), "message": inert(message)},
        })
    };
    let answer = match answer {
        Ok(answer) => answer,
        Err(failure) => return failed(failure.class, &failure.message),
    };
    if !declared.contains(&answer.mode) || request.mode.is_some_and(|asked| asked != answer.mode) {
        return failed(
            ErrorClass::ModeUnavailable,
            "the backend answered by a mode that was not asked for or declared",
        );
    }
    let mut passages: Vec<Passage> = answer
        .passages
        .into_iter()
        .filter(|passage| {
            let Some(total) = served.get(passage.path.as_str()).map(Vec::len) else {
                return false;
            };
            request
                .path
                .as_deref()
                .is_none_or(|prefix| within(&passage.path, prefix))
                && 1 <= passage.from_line
                && passage.from_line <= passage.to_line
                && passage.to_line <= total
                && passage.score.is_finite()
                && !passage.text.is_empty()
        })
        .collect();
    passages.sort_by(compare);
    passages.truncate(request.top);
    let passages: Vec<Value> = passages
        .iter()
        .map(|passage| {
            json!({
                "path": passage.path,
                "heading": inert(&passage.heading),
                "from_line": passage.from_line,
                "to_line": passage.to_line,
                "score": passage.score,
                "text": inert(&passage.text),
            })
        })
        .collect();
    let mut entry = json!({
        "overlay": overlay.identity,
        "mode": answer.mode.as_str(),
        "passages": passages,
    });
    if let Some(note) = answer.note {
        entry["note"] = json!(inert(&note));
    }
    entry
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn order_key(left: &Value, right: &Value) -> Ordering {
    let score = |value: &Value| number(value, "score").unwrap_or(0.0);
    let text = |value: &Value| {
        value["path"]
            .as_str()
            .unwrap_or_default()
            .as_bytes()
            .to_vec()
    };
    let line = |value: &Value, key: &str| value[key].as_i64().unwrap_or(0);
    score(right)
        .partial_cmp(&score(left))
        .unwrap_or(Ordering::Equal)
        .then_with(|| text(left).cmp(&text(right)))
        .then_with(|| line(left, "from_line").cmp(&line(right, "from_line")))
        .then_with(|| line(left, "to_line").cmp(&line(right, "to_line")))
}

/// Passages (JSON objects with `path`, `from_line`, `to_line` and `score`) by score, highest first,
/// then path as UTF-8 bytes, then first line, then last line; passages equal in all four keep the
/// order given.
#[must_use]
pub fn search_order(passages: &[Value]) -> Vec<Value> {
    let mut ordered = passages.to_vec();
    ordered.sort_by(order_key);
    ordered
}

fn is_plain_number(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Number(_)))
}

fn keys_are(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key)))
}

/// The rules a `kb_search` answer breaks, as sorted ids; empty when it conforms. The ranking itself
/// is not checked, only the shape and the order the scores imply.
///
/// # Errors
///
/// The refusal of the arguments, or `unknown-overlay`.
pub fn search_check(
    overlays: &[KnowledgeOverlay],
    arguments: &Value,
    response: &Value,
    modes: &[SearchMode],
    settings: &KnowledgeSettings,
    caller: &CallerFacts,
    engines: &Renderers,
) -> Result<Vec<String>, KnowledgeError> {
    let args = knowledge_arguments("kb_search", arguments, modes, settings)?;
    let mut broken: alloc::collections::BTreeSet<&'static str> =
        alloc::collections::BTreeSet::new();
    if !keys_are(response, &["results"]) {
        return Ok(alloc::vec!["shape".to_string()]);
    }
    let chosen = choose(overlays, args.get("overlay").and_then(Value::as_str))?;
    let Some(results) = response["results"].as_array() else {
        return Ok(alloc::vec!["shape".to_string()]);
    };
    let named: Vec<Option<&str>> = results
        .iter()
        .map(|entry| entry.get("overlay").and_then(Value::as_str))
        .collect();
    let identities: Vec<Option<&str>> = chosen
        .iter()
        .map(|overlay| Some(overlay.identity.as_str()))
        .collect();
    if named != identities {
        broken.insert("overlays");
    }
    let asked = args["mode"].as_str().unwrap_or("auto");
    let top = args["top"].as_u64().unwrap_or(5);
    let prefix = args.get("path").and_then(Value::as_str);
    for (entry, overlay) in results.iter().zip(&chosen) {
        check_entry(
            entry,
            overlay,
            asked,
            top,
            prefix,
            modes,
            settings,
            caller,
            engines,
            &mut broken,
        );
    }
    Ok(broken.into_iter().map(str::to_owned).collect())
}

#[allow(clippy::too_many_arguments)]
fn check_entry(
    entry: &Value,
    overlay: &KnowledgeOverlay,
    asked: &str,
    top: u64,
    prefix: Option<&str>,
    modes: &[SearchMode],
    settings: &KnowledgeSettings,
    caller: &CallerFacts,
    engines: &Renderers,
    broken: &mut alloc::collections::BTreeSet<&'static str>,
) {
    let Some(map) = entry.as_object() else {
        broken.insert("shape");
        return;
    };
    if map.contains_key("error") {
        let error = &map["error"];
        let class = error.get("class").and_then(Value::as_str);
        if !keys_are(entry, &["overlay", "error"])
            || !keys_are(error, &["class", "message"])
            || !matches!(class, Some("unavailable" | "mode-unavailable"))
        {
            broken.insert("shape");
        } else if class == Some("mode-unavailable") && asked == "auto" {
            broken.insert("mode");
        }
        return;
    }
    let allowed = ["overlay", "mode", "passages", "note"];
    if !(["overlay", "mode", "passages"]
        .iter()
        .all(|key| map.contains_key(*key))
        && map.keys().all(|key| allowed.contains(&key.as_str())))
    {
        broken.insert("shape");
        return;
    }
    let mode = map["mode"].as_str().and_then(SearchMode::parse);
    if mode.is_none_or(|mode| !modes.contains(&mode)) || (asked != "auto" && map["mode"] != asked) {
        broken.insert("mode");
    }
    if let Some(note) = map.get("note")
        && note.as_str().is_none_or(|note| note != inert(note))
    {
        broken.insert("inert");
    }
    let Some(passages) = map["passages"].as_array() else {
        broken.insert("shape");
        return;
    };
    if passages.len() as u64 > top {
        broken.insert("top");
    }
    let served = served_of(overlay, settings, caller, engines);
    for passage in passages {
        check_passage(passage, &served, prefix, broken);
    }
    let keyed = passages.iter().all(|passage| {
        is_plain_number(passage.get("score"))
            && passage.get("path").is_some_and(Value::is_string)
            && passage
                .get("from_line")
                .is_some_and(|line| line.is_i64() || line.is_u64())
            && passage
                .get("to_line")
                .is_some_and(|line| line.is_i64() || line.is_u64())
    });
    if keyed && search_order(passages) != *passages {
        broken.insert("order");
    }
}

fn check_passage(
    passage: &Value,
    served: &BTreeMap<String, Vec<String>>,
    prefix: Option<&str>,
    broken: &mut alloc::collections::BTreeSet<&'static str>,
) {
    if !keys_are(
        passage,
        &["path", "heading", "from_line", "to_line", "score", "text"],
    ) {
        broken.insert("shape");
        return;
    }
    let finite = passage["score"].as_f64().is_some_and(f64::is_finite);
    if !passage["score"].is_number() || !finite {
        broken.insert("shape");
        return;
    }
    let Some(path) = passage["path"].as_str() else {
        broken.insert("not-knowledge");
        return;
    };
    if prefix.is_some_and(|prefix| !within(path, prefix)) {
        broken.insert("path-limit");
    }
    let Some(lines) = served.get(path) else {
        broken.insert("not-knowledge");
        return;
    };
    let (first, last) = (passage["from_line"].as_i64(), passage["to_line"].as_i64());
    let (Some(first), Some(last)) = (first, last) else {
        broken.insert("range");
        return;
    };
    if !(1 <= first && first <= last && last as usize <= lines.len()) {
        broken.insert("range");
        return;
    }
    let span = inert(&lines[first as usize - 1..last as usize].join("\n"));
    match passage["text"].as_str() {
        Some(text) if !text.is_empty() && span.contains(text) => {}
        _ => {
            broken.insert("text");
        }
    }
    for field in ["heading", "text"] {
        if passage[field]
            .as_str()
            .is_none_or(|text| text != inert(text))
        {
            broken.insert("inert");
        }
    }
}

impl Passage {
    /// A passage of `path` under `heading`, lines `from_line` to `to_line`, scored `score`.
    #[must_use]
    pub fn new(
        path: impl Into<String>,
        heading: impl Into<String>,
        from_line: usize,
        to_line: usize,
        score: f64,
        text: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            heading: heading.into(),
            from_line,
            to_line,
            score,
            text: text.into(),
        }
    }
}
