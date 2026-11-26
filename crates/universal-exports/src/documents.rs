//! Documents: a brief, a green ink request, a held brief, a Mission's reviews, a charge's report
//! and a Report as Markdown with the references in it resolved for the person reading.
//!
//! A document is `{markdown, references, trackers}`. `markdown` opens with frontmatter the
//! gateway writes from the record (identifiers, who and where, times, state; never a secret or
//! an environment value) and goes on with the body the record's author wrote. A body that opens
//! with frontmatter of its own keeps it, as text, under the key `author`: the gateway's keys are
//! the only ones a reader can rely on. `references` are spans of the body (character offsets
//! into `markdown`) the gateway resolved for this reader; `trackers` are the hosts of the
//! Operation's Sources, the only hosts the gateway vouches for.
//!
//! What resolves:
//!
//! - An issue key of the record's Operation: `KEY-123` of a Jira Source with that project key,
//!   `owner/repo#123` of a GitHub Source and `group/project#123` of a GitLab one, to the
//!   tracker's address.
//! - An Operation ID, a Mission's UID or handle and a Run's UID, to an `app:` route, only where
//!   the reader holds `mission.read` on the Operation. Another Operation than the record's goes
//!   through the compartments (the crossing is audited before it is allowed); otherwise the
//!   text stays text.
//! - A Mission or Operation reference also carries what it leads to besides its page: the
//!   Mission's linked issues and the board each of the Operation's Sources names.
//!
//! Text inside code (fenced or inline) and inside an address is never a reference. The same
//! routes answer the Markdown alone to `Accept: text/markdown`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::mission_ids::is_uid;
use ai6::directorates::registry::mission_issues::LinkState;
use ai6::{Authorizer, Person, SourcePage};
use ai6_diptel::{MissionId, OperationId};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::compartments::CompartmentAccess;
use crate::tree::{OperationTree, operation_request};
use crate::{
    ApiError, Gateway, authorize_operation_action, bearer, known_operations, named_mission,
    operation_tree, resolve_user,
};

/// A linked issue a Mission reference leads to.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct LinkedIssue {
    /// `TRACKER:ITEM`, as every route under the Mission names it.
    pub(crate) issue: String,
    /// The title when last read. Untrusted text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    /// `open` or `closed` when last read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) state: Option<String>,
    /// The tracker's address for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
}

/// The board one of an Operation's Sources names.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Board {
    /// The Source instance.
    pub(crate) tracker: String,
    /// The provider serving it.
    pub(crate) provider: String,
    /// The `https` page of the board.
    pub(crate) url: String,
}

/// One span of a document's body the gateway resolved for its reader.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Reference {
    /// Where the span starts in `markdown`, in characters.
    pub(crate) start: usize,
    /// Where it ends (exclusive).
    pub(crate) end: usize,
    /// The words of the span.
    pub(crate) text: String,
    /// `issue`, `mission`, `operation` or `run`.
    pub(crate) kind: &'static str,
    /// The tracker's address, for an issue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) url: Option<String>,
    /// The app page, `app:/...`, for a Mission, Operation or Run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) route: Option<String>,
    /// The Operation a Mission, Operation or Run belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) operation: Option<String>,
    /// A Mission's linked issues.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) issues: Vec<LinkedIssue>,
    /// The boards of the Operation's Sources, for a Mission or an Operation.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) boards: Vec<Board>,
}

/// A document as the gateway serves it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Document {
    /// Frontmatter the gateway wrote, then the author's body.
    pub(crate) markdown: String,
    /// The spans of the body that resolved for this reader, in order.
    pub(crate) references: Vec<Reference>,
    /// The hosts of the record's Operation's Sources.
    pub(crate) trackers: Vec<String>,
    /// References past `[text] document_references`, left as text.
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) omitted_references: usize,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// What a route knows of a record: the frontmatter keys in order, the body and the Operation
/// its issue keys are read in.
pub(crate) struct Draft {
    meta: Vec<(String, Value)>,
    body: String,
    operation: Option<String>,
}

impl Draft {
    /// A document of `kind`, about a record of `operation` when it has one.
    pub(crate) fn new(kind: &str, operation: Option<&str>) -> Self {
        let mut draft = Self::bare(operation);
        draft.meta.push(("kind".to_owned(), Value::from(kind)));
        draft
    }

    /// A document with no keys yet, for a record whose own fields (a Report's `kind`) are all
    /// its frontmatter.
    pub(crate) fn bare(operation: Option<&str>) -> Self {
        Self {
            meta: Vec::new(),
            body: String::new(),
            operation: operation.map(str::to_owned),
        }
    }

    /// Adds the keys of `map` named in `order` first, in that order, then the others in the
    /// map's own, except `except`.
    #[must_use]
    pub(crate) fn with_ordered(
        mut self,
        map: &Map<String, Value>,
        order: &[&str],
        except: &[&str],
    ) -> Self {
        for key in order {
            if let Some(value) = map.get(*key).filter(|_| !except.contains(key)) {
                self = self.with(key, value.clone());
            }
        }
        for (key, value) in map {
            if !order.contains(&key.as_str()) && !except.contains(&key.as_str()) {
                self = self.with(key, value.clone());
            }
        }
        self
    }

    /// Adds a key, unless the value says nothing (null, empty text, empty list or map).
    #[must_use]
    pub(crate) fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        let value = value.into();
        let empty = match &value {
            Value::Null => true,
            Value::String(text) => text.is_empty(),
            Value::Array(items) => items.is_empty(),
            Value::Object(map) => map.is_empty(),
            _ => false,
        };
        if !empty {
            self.meta.push((key.to_owned(), value));
        }
        self
    }

    /// Adds every key of `map` that says something, in the map's order.
    #[must_use]
    pub(crate) fn with_all(mut self, map: &Map<String, Value>, except: &[&str]) -> Self {
        for (key, value) in map {
            if !except.contains(&key.as_str()) {
                self = self.with(key, value.clone());
            }
        }
        self
    }

    /// The text the author wrote.
    #[must_use]
    pub(crate) fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }
}

/// Whether the request prefers the Markdown alone: `text/markdown` named in `Accept` before
/// JSON (or a wildcard) is.
pub(crate) fn wants_markdown(headers: &HeaderMap) -> bool {
    let Some(accept) = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    for range in accept.split(',') {
        let media = range.split(';').next().unwrap_or("").trim();
        if media.eq_ignore_ascii_case("text/markdown") {
            return true;
        }
        if media.eq_ignore_ascii_case("application/json") || media.contains('*') {
            return false;
        }
    }
    false
}

/// `document` as the request asked for it: the JSON form, or the Markdown alone.
pub(crate) fn respond(headers: &HeaderMap, document: Document) -> Response {
    let vary = (header::VARY, HeaderValue::from_static("accept"));
    if wants_markdown(headers) {
        (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/markdown; charset=utf-8"),
                ),
                vary,
            ],
            document.markdown,
        )
            .into_response()
    } else {
        ([vary], Json(document)).into_response()
    }
}

// ---- frontmatter -------------------------------------------------------------------------

/// `text` as a YAML double-quoted scalar. JSON's escapes are YAML's; the three line breaks YAML
/// 1.1 adds are escaped too, so a value never spans a line.
fn quoted(text: &str) -> String {
    escape_breaks(&serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned()))
}

fn escape_breaks(text: &str) -> String {
    text.replace('\u{85}', "\\u0085")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

fn key_text(key: &str) -> String {
    let plain = key
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if plain { key.to_owned() } else { quoted(key) }
}

/// A value on one line: a scalar, or JSON (a YAML flow collection) for an empty or nested one.
fn inline(value: &Value) -> String {
    match value {
        Value::String(text) => quoted(text),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.to_string(),
        other => escape_breaks(&other.to_string()),
    }
}

fn emit_map<'a>(
    out: &mut String,
    entries: impl IntoIterator<Item = (&'a String, &'a Value)>,
    indent: usize,
) {
    let pad = " ".repeat(indent);
    for (key, value) in entries {
        let key = key_text(key);
        match value {
            Value::Object(map) if !map.is_empty() => {
                out.push_str(&format!("{pad}{key}:\n"));
                emit_map(out, map, indent + 2);
            }
            Value::Array(items) if !items.is_empty() => {
                out.push_str(&format!("{pad}{key}:\n"));
                for item in items {
                    emit_item(out, item, indent + 2);
                }
            }
            scalar => out.push_str(&format!("{pad}{key}: {}\n", inline(scalar))),
        }
    }
}

fn emit_item(out: &mut String, item: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    match item {
        Value::Object(map) if !map.is_empty() => {
            let mut nested = String::new();
            emit_map(&mut nested, map, indent + 2);
            // The first key rides on the dash.
            out.push_str(&format!("{pad}- {}", &nested[indent + 2..]));
        }
        other => out.push_str(&format!("{pad}- {}\n", inline(other))),
    }
}

/// The author's own frontmatter, when the body opens with a block between `---` lines, and the
/// body after it. The block is text: no parser reads it here.
fn split_author(body: &str) -> (Option<String>, &str) {
    let mut lines = body.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return (None, body);
    };
    if first.trim_end() != "---" {
        return (None, body);
    }
    let mut taken = first.len();
    let mut block = String::new();
    for line in lines {
        taken += line.len();
        if matches!(line.trim_end(), "---" | "...") {
            let block = block
                .chars()
                .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
                .collect::<String>();
            return (Some(block), &body[taken..]);
        }
        block.push_str(&line.replace('\r', ""));
    }
    (None, body)
}

/// The frontmatter block: the gateway's keys, then the author's text under `author`.
fn frontmatter(meta: &[(String, Value)], author: Option<&str>, truncated: bool) -> String {
    let mut out = String::from("---\n");
    emit_map(&mut out, meta.iter().map(|(key, value)| (key, value)), 0);
    if truncated {
        out.push_str("truncated: true\n");
    }
    if let Some(author) = author.filter(|author| !author.trim().is_empty()) {
        out.push_str("author: |2\n");
        for line in author.trim_end_matches('\n').split('\n') {
            if line.is_empty() {
                out.push('\n');
            } else {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out.push_str("---\n");
    out
}

// ---- spans -------------------------------------------------------------------------------

/// What a span of the body looks like before anything is looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Shape {
    /// `owner/repo#123`.
    Slash,
    /// `KEY-123`.
    Key,
    /// Anything else written like an identifier.
    Word,
}

/// A span that may be a reference: byte offsets into the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) shape: Shape,
}

fn fence_marker(line: &str) -> Option<(char, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = line.trim_start_matches(' ');
    let marker = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let count = rest.chars().take_while(|c| *c == marker).count();
    (count >= 3).then_some((marker, count))
}

/// The byte ranges of `body` no reference is read in: fenced code, inline code, and addresses
/// (`scheme://...` and `app:/...`).
fn masked(body: &str) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<(char, usize, usize)> = None;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        match open {
            None => {
                if let Some((marker, count)) = fence_marker(line) {
                    open = Some((marker, count, offset));
                }
            }
            Some((marker, count, from)) => {
                if let Some((closing, length)) = fence_marker(line)
                    && closing == marker
                    && length >= count
                    && line
                        .trim_start_matches(' ')
                        .trim_end()
                        .chars()
                        .all(|c| c == marker)
                {
                    ranges.push((from, offset + line.len()));
                    open = None;
                }
            }
        }
        offset += line.len();
    }
    if let Some((_, _, from)) = open {
        ranges.push((from, body.len()));
    }
    let fenced = ranges.clone();
    let inside = |at: usize| fenced.iter().any(|(from, to)| (*from..*to).contains(&at));
    // Inline code: a run of backticks closes at the next run of the same length.
    let bytes = body.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'`' && !inside(at) {
            let start = at;
            while at < bytes.len() && bytes[at] == b'`' {
                at += 1;
            }
            let length = at - start;
            let mut search = at;
            let mut closed = None;
            while search < bytes.len() {
                if bytes[search] == b'`' {
                    let from = search;
                    while search < bytes.len() && bytes[search] == b'`' {
                        search += 1;
                    }
                    if search - from == length && !inside(from) {
                        closed = Some(search);
                        break;
                    }
                } else {
                    search += 1;
                }
            }
            if let Some(end) = closed {
                ranges.push((start, end));
                at = end;
            }
        } else {
            at += 1;
        }
    }
    // Addresses.
    for marker in ["://", "app:/"] {
        for (found, _) in body.match_indices(marker) {
            let mut start = found;
            while start > 0
                && (body.as_bytes()[start - 1].is_ascii_alphanumeric()
                    || matches!(body.as_bytes()[start - 1], b'+' | b'-' | b'.'))
            {
                start -= 1;
            }
            let end = body[found..]
                .find(|c: char| {
                    c.is_whitespace() || matches!(c, ')' | '>' | ']' | '"' | '\'' | '<')
                })
                .map_or(body.len(), |length| found + length);
            ranges.push((start, end));
        }
    }
    ranges
}

fn is_token_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '#')
}

fn segments_ok(path: &str) -> bool {
    path.split('/').all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    })
}

fn classify(text: &str) -> Option<Shape> {
    if !text.is_ascii() {
        return None;
    }
    if let Some((path, number)) = text.split_once('#') {
        let numbered = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit());
        return (numbered && path.contains('/') && segments_ok(path)).then_some(Shape::Slash);
    }
    if text.contains('/') {
        return None;
    }
    if let Some((project, number)) = text.rsplit_once('-')
        && project.len() >= 2
        && project.starts_with(|c: char| c.is_ascii_uppercase())
        && project
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
    {
        return Some(Shape::Key);
    }
    Some(Shape::Word)
}

/// The spans of `body` that may be references, outside code and addresses.
pub(crate) fn candidates(body: &str) -> Vec<Candidate> {
    let masks = masked(body);
    let blocked = |from: usize, to: usize| masks.iter().any(|(a, b)| from < *b && to > *a);
    let mut found = Vec::new();
    let mut run: Option<usize> = None;
    let mut consider = |from: usize, to: usize| {
        let mut start = from;
        let mut end = to;
        while start < end && matches!(body.as_bytes()[start], b'.' | b'-' | b'/' | b'#') {
            start += 1;
        }
        while end > start && matches!(body.as_bytes()[end - 1], b'.' | b'-' | b'/') {
            end -= 1;
        }
        if start < end
            && !blocked(start, end)
            && let Some(shape) = classify(&body[start..end])
        {
            found.push(Candidate { start, end, shape });
        }
    };
    for (at, c) in body.char_indices() {
        match (is_token_char(c), run) {
            (true, None) => run = Some(at),
            (false, Some(from)) => {
                consider(from, at);
                run = None;
            }
            _ => {}
        }
    }
    if let Some(from) = run {
        consider(from, body.len());
    }
    found
}

// ---- resolving ---------------------------------------------------------------------------

/// What one distinct piece of text resolved to.
#[derive(Debug, Clone)]
struct Target {
    kind: &'static str,
    url: Option<String>,
    route: Option<String>,
    operation: Option<String>,
    issues: Vec<LinkedIssue>,
}

/// Looks references up for one reader, one document.
struct Resolver {
    person: Person,
    access: Arc<CompartmentAccess>,
    tree: OperationTree,
    registry: Option<Arc<dyn RegistryBackend>>,
    operations: BTreeSet<String>,
    pages: Vec<SourcePage>,
    reads: HashMap<String, bool>,
}

impl Resolver {
    /// Whether the reader holds `mission.read` on `operation`: the access policy, or a
    /// compartment, the crossing audited before it is allowed.
    fn reads(&mut self, operation: &str) -> bool {
        if let Some(known) = self.reads.get(operation) {
            return *known;
        }
        let request = operation_request(self.person.clone(), "mission.read", operation, &self.tree);
        let allowed = self
            .access
            .authorize(request)
            .is_ok_and(|decision| decision.allowed);
        self.reads.insert(operation.to_owned(), allowed);
        allowed
    }

    fn route(page: &str, fields: Value) -> Option<String> {
        let mut route = json!({ "page": page });
        for (key, value) in fields.as_object()? {
            route[key] = value.clone();
        }
        crate::attention::page_link(&route).1
    }

    fn issue_url(&self, text: &str, shape: Shape) -> Option<String> {
        match shape {
            Shape::Key => {
                let project = text.rsplit_once('-')?.0;
                self.pages.iter().find_map(|page| {
                    (page.provider == "jira" && page.project.as_deref() == Some(project))
                        .then(|| format!("{}/browse/{text}", page.web.trim_end_matches('/')))
                })
            }
            Shape::Slash => {
                let (repo, number) = text.split_once('#')?;
                self.pages.iter().find_map(|page| {
                    let tail = page.web.trim_end_matches('/');
                    let same = tail.len() > repo.len()
                        && tail[tail.len() - repo.len()..].eq_ignore_ascii_case(repo)
                        && tail[..tail.len() - repo.len()].ends_with('/');
                    match (page.provider.as_str(), same) {
                        ("github", true) => Some(format!("{tail}/issues/{number}")),
                        ("gitlab", true) => Some(format!("{tail}/-/issues/{number}")),
                        _ => None,
                    }
                })
            }
            Shape::Word => None,
        }
    }

    fn mission_target(&mut self, uid: &str, operation: &str) -> Option<Target> {
        if !self.reads(operation) {
            return None;
        }
        let route = Self::route(
            "missions",
            json!({"operation_id": operation, "mission_id": uid}),
        )?;
        let issues = self
            .registry
            .as_ref()
            .and_then(|registry| registry.issue_links(uid).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|link| link.operation == operation && link.state != LinkState::Unlinked)
            .map(|link| {
                let name = link.name();
                let (title, state) = link.snapshot.map_or((None, None), |snapshot| {
                    (Some(snapshot.title), snapshot.state)
                });
                LinkedIssue {
                    issue: name,
                    title,
                    state,
                    url: link.url,
                }
            })
            .collect();
        Some(Target {
            kind: "mission",
            url: None,
            route: Some(route),
            operation: Some(operation.to_owned()),
            issues,
        })
    }

    /// The Operation a Mission was filed under.
    fn operation_of_mission(&self, uid: &str) -> Option<String> {
        self.registry
            .as_ref()?
            .filed_brief(uid)
            .ok()??
            .brief
            .operation
    }

    fn word(&mut self, text: &str) -> Option<Target> {
        if self.operations.contains(text) {
            if !self.reads(text) {
                return None;
            }
            return Some(Target {
                kind: "operation",
                url: None,
                route: Some(Self::route("operations", json!({"operation_id": text}))?),
                operation: Some(text.to_owned()),
                issues: Vec::new(),
            });
        }
        let id_like = is_uid(text)
            || text
                .chars()
                .any(|c| c.is_ascii_digit() || matches!(c, '-' | '_'));
        if !id_like {
            return None;
        }
        let registry = self.registry.clone()?;
        if let Some(handle) = registry.resolve_mission(text).ok().flatten() {
            let operation = self.operation_of_mission(&handle.mission_id)?;
            return self.mission_target(&handle.mission_id, &operation);
        }
        if is_uid(text) {
            let run = registry.run(text).ok().flatten()?;
            let operation = run
                .operation
                .clone()
                .or_else(|| self.operation_of_mission(&run.mission_id))?;
            if !self.reads(&operation) {
                return None;
            }
            return Some(Target {
                kind: "run",
                url: None,
                route: Some(Self::route(
                    "runs",
                    json!({"operation_id": operation, "run_id": text}),
                )?),
                operation: Some(operation),
                issues: Vec::new(),
            });
        }
        None
    }

    fn resolve(&mut self, text: &str, shape: Shape) -> Option<Target> {
        if matches!(shape, Shape::Key | Shape::Slash)
            && let Some(url) = self.issue_url(text, shape)
        {
            return Some(Target {
                kind: "issue",
                url: Some(url),
                route: None,
                operation: None,
                issues: Vec::new(),
            });
        }
        match shape {
            Shape::Slash => None,
            Shape::Key | Shape::Word => self.word(text),
        }
    }
}

/// The hosts of an Operation's Source pages, once each, in order.
fn hosts(pages: &[SourcePage]) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for page in pages {
        if let Some(host) = url::Url::parse(&page.web)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            && !hosts.contains(&host)
        {
            hosts.push(host);
        }
    }
    hosts
}

/// `text` cut to at most `bytes`, at a character boundary.
fn cut(text: &str, bytes: usize) -> &str {
    if text.len() <= bytes {
        return text;
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The document `draft` is, for `person`, before the boards are asked of the Headquarters
/// Station: the Operations whose boards a reference carries come with it.
fn assemble(
    mut resolver: Resolver,
    draft: Draft,
    limits: &ai6::TextLimits,
) -> (Document, BTreeSet<String>) {
    let (author, body) = split_author(&draft.body);
    let mut head = frontmatter(&draft.meta, author.as_deref(), false);
    let mut body = body.to_owned();
    // A blank line separates the frontmatter from the body.
    if !limits.document_bytes.allows(head.len() + 1 + body.len()) {
        head = frontmatter(&draft.meta, author.as_deref(), true);
        let room = limits
            .document_bytes
            .or_max()
            .saturating_sub(head.len() + 1);
        body = cut(&body, room).to_owned();
    }
    let prefix = head.chars().count() + 1;
    let (mut references, mut omitted) = (Vec::new(), 0);
    let mut resolved: HashMap<(String, Shape), Option<Target>> = HashMap::new();
    let mut boards_for = BTreeSet::new();
    for candidate in candidates(&body) {
        let text = &body[candidate.start..candidate.end];
        let target = resolved
            .entry((text.to_owned(), candidate.shape))
            .or_insert_with(|| resolver.resolve(text, candidate.shape))
            .clone();
        let Some(target) = target else { continue };
        if !limits.document_references.allows(references.len() + 1) {
            omitted += 1;
            continue;
        }
        let start = prefix + body[..candidate.start].chars().count();
        if matches!(target.kind, "mission" | "operation")
            && let Some(operation) = &target.operation
        {
            boards_for.insert(operation.clone());
        }
        references.push(Reference {
            start,
            end: start + text.chars().count(),
            text: text.to_owned(),
            kind: target.kind,
            url: target.url,
            route: target.route,
            operation: target.operation,
            issues: target.issues,
            boards: Vec::new(),
        });
    }
    (
        Document {
            markdown: format!("{head}\n{body}"),
            references,
            trackers: hosts(&resolver.pages),
            omitted_references: omitted,
        },
        boards_for,
    )
}

/// The pages of `operation`'s Sources, from the roster.
fn pages_of(gateway: &Gateway, operation: &str) -> Option<Vec<SourcePage>> {
    let roster = gateway.roster.clone()?;
    let snapshot = roster.load("").ok()?;
    Some(ai6::source_pages(
        &snapshot.roster.operation(operation).ok()?.sources,
    ))
}

/// Writes `draft` as a document for `person`, with every reference resolved for them.
pub(crate) async fn compose(
    gateway: &Gateway,
    person: Person,
    draft: Draft,
) -> Result<Document, ApiError> {
    let access = gateway.authorizer()?;
    let tree = operation_tree(gateway).await?;
    let operations: BTreeSet<String> = known_operations(gateway).await.into_iter().collect();
    let registry = gateway.registry.clone();
    let limits = gateway.config.unex().text.clone();
    let worker = gateway.clone();
    let (mut document, boards_for) = tokio::task::spawn_blocking(move || {
        let pages = draft
            .operation
            .as_deref()
            .and_then(|operation| pages_of(&worker, operation))
            .unwrap_or_default();
        let resolver = Resolver {
            person,
            access,
            tree,
            registry,
            operations,
            pages,
            reads: HashMap::new(),
        };
        assemble(resolver, draft, &limits)
    })
    .await
    .map_err(|_| ApiError::internal())?;
    let mut boards: BTreeMap<String, Vec<Board>> = BTreeMap::new();
    for operation in boards_for {
        let found = crate::source_reads::boards(gateway, &operation).await;
        boards.insert(
            operation,
            found
                .into_iter()
                .map(|board| Board {
                    tracker: board.tracker,
                    provider: board.provider,
                    url: board.url,
                })
                .collect(),
        );
    }
    for reference in &mut document.references {
        if let Some(found) = reference
            .operation
            .as_ref()
            .filter(|_| matches!(reference.kind, "mission" | "operation"))
            .and_then(|operation| boards.get(operation))
        {
            reference.boards = found.clone();
        }
    }
    Ok(document)
}

// ---- the brief ---------------------------------------------------------------------------

/// `GET /v1/user/operations/{operation_id}/missions/{mission_id}/brief`: the Mission's brief as
/// a document, its launch context in the frontmatter (the names of its environment variables,
/// never their values). Needs `mission.read`.
pub(crate) async fn brief(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person.clone(), &operation, "mission.read").await?;
    let named = named_mission(&gateway, &mission_id).await?;
    let uid = named.mission_id.clone();
    let key = MissionId::new(uid.clone()).map_err(|_| ApiError::not_found())?;
    let manifest = gateway
        .inner
        .lock()
        .await
        .missions
        .get(&(operation.clone(), key))
        .cloned();
    let filed = match gateway.registry.clone() {
        Some(registry) => {
            let wanted = uid.clone();
            tokio::task::spawn_blocking(move || registry.filed_brief(&wanted))
                .await
                .map_err(|_| ApiError::internal())?
                .map_err(|_| ApiError::internal())?
        }
        None => None,
    };
    if filed
        .as_ref()
        .is_some_and(|filed| filed.brief.operation.as_deref() != Some(operation.as_str()))
    {
        return Err(ApiError::not_found());
    }
    if manifest.is_none() && filed.is_none() {
        return Err(ApiError::not_found());
    }
    let mut draft = Draft::new("brief", Some(operation.as_str()))
        .with("operation", operation.as_str())
        .with("mission", uid)
        .with("handle", named.handle);
    if let Some(filed) = &filed {
        let brief = &filed.brief;
        draft = draft
            .with("title", brief.title.clone())
            .with("state", filed.state.clone())
            .with("source", brief.source.clone())
            .with("created", brief.created.clone())
            .with("url", brief.url.clone())
            .with("stations", brief.stations.clone());
    }
    let name = manifest
        .as_ref()
        .and_then(|manifest| manifest.name.clone())
        .or_else(|| filed.as_ref().and_then(|filed| filed.brief.name.clone()));
    draft = draft.with("name", name);
    if let Some(manifest) = &manifest {
        draft = draft
            .with("agent", manifest.agent_id.clone())
            .with("role", manifest.role_id.clone())
            .with("station", manifest.station_id.clone())
            .with("theater", manifest.theater_id.clone())
            .with("mode", manifest.mode.clone())
            .with("handler", manifest.handler_provider.clone())
            .with("terminal", manifest.terminal_provider.clone())
            .with("working_directory", manifest.working_directory.clone())
            .with("model", manifest.model.clone())
            .with("effort", manifest.effort.clone())
            .with("environment_names", manifest.environment_keys.clone());
        if let Some(Value::Object(mut origin)) = manifest
            .provenance
            .as_ref()
            .and_then(|provenance| serde_json::to_value(provenance).ok())
        {
            origin.remove("digest");
            draft = draft.with("origin", Value::Object(origin));
        }
    } else if let Some(filed) = &filed {
        draft = draft
            .with("agent", filed.brief.agent.clone())
            .with("role", filed.brief.role.clone());
    }
    let body = match (&manifest, &filed) {
        (Some(manifest), _) => manifest.prompt.clone(),
        (None, Some(filed)) => filed.brief.body.clone(),
        (None, None) => String::new(),
    };
    let document = compose(&gateway, person, draft.body(body)).await?;
    Ok(respond(&headers, document))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(body: &str) -> Vec<(&str, Shape)> {
        candidates(body)
            .into_iter()
            .map(|c| (&body[c.start..c.end], c.shape))
            .collect()
    }

    #[test]
    fn the_scanner_finds_keys_slash_forms_and_identifiers_and_trims_punctuation() {
        let shapes = found("See OPS-12, o/r#7. Then ai6-42.");
        assert!(shapes.contains(&("OPS-12", Shape::Key)), "{shapes:?}");
        assert!(shapes.contains(&("o/r#7", Shape::Slash)), "{shapes:?}");
        assert!(shapes.contains(&("ai6-42", Shape::Word)), "{shapes:?}");
        assert!(found("a/b/c#9").contains(&("a/b/c#9", Shape::Slash)));
        assert!(
            !found("path/to/file")
                .iter()
                .any(|(_, s)| *s == Shape::Slash)
        );
        assert!(!found("x#12").iter().any(|(_, s)| *s == Shape::Slash));
        assert!(found("é-12").is_empty(), "non-ASCII is never an identifier");
    }

    #[test]
    fn code_and_addresses_are_never_references() {
        let body = "`OPS-1` and ``a`b OPS-2`` and\n```\nOPS-3\n```\nhttps://example.test/o/r#4 \
                    app:/missions?operation_id=G [x](https://e.test/OPS-5) OPS-6";
        let texts: Vec<&str> = found(body).into_iter().map(|(text, _)| text).collect();
        assert!(texts.contains(&"OPS-6"), "{texts:?}");
        for hidden in ["OPS-1", "OPS-2", "OPS-3", "o/r#4", "OPS-5"] {
            assert!(!texts.contains(&hidden), "{hidden} in {texts:?}");
        }
        assert!(!texts.contains(&"operation_id"), "{texts:?}");
        let open = found("```\nOPS-7 stays code to the end\n");
        assert!(
            open.is_empty(),
            "an unclosed fence runs to the end: {open:?}"
        );
    }

    #[test]
    fn frontmatter_quotes_every_value_and_never_lets_one_become_structure() {
        let meta = vec![
            ("kind".to_owned(), json!("brief")),
            ("name".to_owned(), json!("a: b\n---\nkey: &x [1]")),
            ("count".to_owned(), json!(3)),
            ("names".to_owned(), json!(["A", "B"])),
            (
                "links".to_owned(),
                json!([{"label": "PR", "url": "https://e.test/1"}]),
            ),
            (
                "origin".to_owned(),
                json!({"origin": "operator", "empty": []}),
            ),
            ("odd key".to_owned(), json!("x\u{2028}y")),
        ];
        let text = frontmatter(&meta, None, false);
        assert_eq!(
            text,
            "---\nkind: \"brief\"\nname: \"a: b\\n---\\nkey: &x [1]\"\ncount: 3\nnames:\n  - \"A\"\n  - \"B\"\n\
             links:\n  - label: \"PR\"\n    url: \"https://e.test/1\"\n\
             origin:\n  origin: \"operator\"\n  empty: []\n\"odd key\": \"x\\u2028y\"\n---\n"
        );
    }

    #[test]
    fn an_authors_frontmatter_is_kept_as_text_under_author() {
        let (author, rest) = split_author("---\ntitle: x\nkind: forged\n---\nBody\n");
        assert_eq!(author.as_deref(), Some("title: x\nkind: forged\n"));
        assert_eq!(rest, "Body\n");
        let meta = vec![("kind".to_owned(), json!("brief"))];
        let text = frontmatter(&meta, author.as_deref(), false);
        assert_eq!(
            text,
            "---\nkind: \"brief\"\nauthor: |2\n  title: x\n  kind: forged\n---\n"
        );
        // A rule that closes nothing is a body, not frontmatter.
        assert_eq!(split_author("---\nno end\n"), (None, "---\nno end\n"));
        assert_eq!(split_author("text\n---\na\n---\n").0, None);
        let (clean, _) = split_author("---\na: \u{7}b\r\n---\n");
        assert_eq!(clean.as_deref(), Some("a: b\n"));
    }

    #[test]
    fn markdown_is_wanted_only_when_it_is_named_before_json() {
        let accept = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT, HeaderValue::from_str(value).unwrap());
            wants_markdown(&headers)
        };
        assert!(accept("text/markdown"));
        assert!(accept("Text/Markdown; charset=utf-8, application/json"));
        assert!(!accept("application/json, text/markdown"));
        assert!(!accept("*/*"));
        assert!(!accept("text/html"));
        assert!(!wants_markdown(&HeaderMap::new()));
    }

    #[test]
    fn long_text_is_cut_at_a_character() {
        assert_eq!(cut("héllo", 2), "h");
        assert_eq!(cut("héllo", 3), "hé");
        assert_eq!(cut("abc", 10), "abc");
    }
}
