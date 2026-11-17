//! Mustache as the official specification defines it (variables, sections, inverted sections,
//! comments, delimiters and partials). Lambdas and inheritance are not supported.

use super::{lookup, truthy, Escape, Name};
use crate::{
    error::{Error, Limit, SyntaxKind},
    options::Options,
    sink::{position, Sink},
};
use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use serde_json::Value;

#[derive(Debug)]
enum Node {
    Text(String),
    Var {
        name: Name,
        raw: bool,
    },
    Section {
        name: Name,
        inverted: bool,
        body: Vec<Node>,
    },
    Partial {
        name: String,
        indent: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Var,
    Raw,
    Open,
    Inverted,
    Close,
    Comment,
    Partial,
    Delims,
}

impl Kind {
    /// Whether a tag of this kind alone on its line takes the line with it.
    fn standalone(self) -> bool {
        !matches!(self, Kind::Var | Kind::Raw)
    }
}

enum Tok {
    Text(usize, usize),
    Tag {
        kind: Kind,
        content: String,
        start: usize,
        end: usize,
        indent: String,
    },
}

fn syntax(src: &str, offset: usize, kind: SyntaxKind) -> Error {
    let (line, column) = position(src, offset);
    Error::Syntax { line, column, kind }
}

fn name_of(content: &str) -> Name {
    if content == "." {
        Vec::new()
    } else {
        content.split('.').map(String::from).collect()
    }
}

fn tokenize(src: &str) -> Result<Vec<Tok>, Error> {
    let (mut open, mut close) = (String::from("{{"), String::from("}}"));
    let mut toks = Vec::new();
    let mut pos = 0;
    loop {
        let Some(rel) = src[pos..].find(open.as_str()) else {
            if pos < src.len() {
                toks.push(Tok::Text(pos, src.len()));
            }
            return Ok(toks);
        };
        let start = pos + rel;
        if start > pos {
            toks.push(Tok::Text(pos, start));
        }
        let body = start + open.len();
        let unclosed = || syntax(src, start, SyntaxKind::UnclosedTag);
        let find = |from: usize, pat: &str| src[from..].find(pat).map(|i| i + from);
        let sigil = src[body..].chars().next();
        let (kind, content, end) = match sigil {
            Some('{') => {
                let pat = format!("}}{close}");
                let at = find(body + 1, &pat).ok_or_else(unclosed)?;
                (Kind::Raw, src[body + 1..at].trim(), at + pat.len())
            }
            Some('=') => {
                let at = find(body + 1, &close).ok_or_else(unclosed)?;
                let tag_end = at + close.len();
                let inner = src[body + 1..at]
                    .strip_suffix('=')
                    .ok_or_else(|| syntax(src, start, SyntaxKind::InvalidDelimiters))?;
                let parts: Vec<&str> = inner.split_whitespace().collect();
                match parts.as_slice() {
                    [a, b] if !a.contains('=') && !b.contains('=') => {
                        open = (*a).into();
                        close = (*b).into();
                    }
                    _ => return Err(syntax(src, start, SyntaxKind::InvalidDelimiters)),
                }
                (Kind::Delims, "", tag_end)
            }
            Some('<' | '$') => return Err(syntax(src, start, SyntaxKind::UnsupportedTag)),
            Some(c @ ('#' | '^' | '/' | '>' | '&' | '!')) => {
                let kind = match c {
                    '#' => Kind::Open,
                    '^' => Kind::Inverted,
                    '/' => Kind::Close,
                    '>' => Kind::Partial,
                    '&' => Kind::Raw,
                    _ => Kind::Comment,
                };
                let from = body + c.len_utf8();
                let at = find(from, &close).ok_or_else(unclosed)?;
                (kind, src[from..at].trim(), at + close.len())
            }
            _ => {
                let at = find(body, &close).ok_or_else(unclosed)?;
                (Kind::Var, src[body..at].trim(), at + close.len())
            }
        };
        if content.is_empty() && !matches!(kind, Kind::Comment | Kind::Delims) {
            return Err(syntax(src, start, SyntaxKind::InvalidName));
        }
        toks.push(Tok::Tag {
            kind,
            content: content.into(),
            start,
            end,
            indent: String::new(),
        });
        pos = end;
    }
}

/// Marks standalone tags: each takes its line's leading blanks and line end out of the
/// neighbouring text, and a partial keeps the blanks as its indentation.
fn standalone(src: &str, toks: &mut [Tok]) {
    for i in 0..toks.len() {
        let (kind, start, end) = match &toks[i] {
            Tok::Tag {
                kind, start, end, ..
            } if kind.standalone() => (*kind, *start, *end),
            _ => continue,
        };
        let line_start = src[..start].rfind('\n').map_or(0, |n| n + 1);
        let prefix = &src[line_start..start];
        if !prefix.bytes().all(|c| c == b' ' || c == b'\t') {
            continue;
        }
        let rest = &src[end..];
        let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let after = &rest[blanks..];
        let strip_after = if after.is_empty() {
            blanks
        } else if after.starts_with('\n') {
            blanks + 1
        } else if after.starts_with("\r\n") {
            blanks + 2
        } else {
            continue;
        };
        if i > 0 {
            if let Tok::Text(s, e) = &mut toks[i - 1] {
                *e = (*e).saturating_sub(prefix.len()).max(*s);
            }
        }
        if let Some(Tok::Text(s, e)) = toks.get_mut(i + 1) {
            *s = (*s + strip_after).min(*e);
        }
        if kind == Kind::Partial {
            if let Tok::Tag { indent, .. } = &mut toks[i] {
                *indent = prefix.into();
            }
        }
    }
}

fn parse(src: &str, max_depth: usize, base_depth: usize) -> Result<Vec<Node>, Error> {
    let mut toks = tokenize(src)?;
    standalone(src, &mut toks);
    let mut stack: Vec<(Name, bool, usize, Vec<Node>)> = vec![(Vec::new(), false, 0, Vec::new())];
    for tok in toks {
        let top = |s: &mut Vec<(Name, bool, usize, Vec<Node>)>| -> usize { s.len() - 1 };
        match tok {
            Tok::Text(s, e) => {
                if s < e {
                    let at = top(&mut stack);
                    stack[at].3.push(Node::Text(src[s..e].into()));
                }
            }
            Tok::Tag {
                kind,
                content,
                start,
                indent,
                ..
            } => {
                let at = top(&mut stack);
                match kind {
                    Kind::Comment | Kind::Delims => {}
                    Kind::Var | Kind::Raw => stack[at].3.push(Node::Var {
                        name: name_of(&content),
                        raw: kind == Kind::Raw,
                    }),
                    Kind::Partial => stack[at].3.push(Node::Partial {
                        name: content,
                        indent,
                    }),
                    Kind::Open | Kind::Inverted => {
                        stack.push((name_of(&content), kind == Kind::Inverted, start, Vec::new()));
                        if base_depth + stack.len() > max_depth {
                            return Err(Error::Limit(Limit::Depth));
                        }
                    }
                    Kind::Close => {
                        if stack.len() == 1 {
                            return Err(syntax(src, start, SyntaxKind::StrayClose));
                        }
                        if stack[at].0 != name_of(&content) {
                            return Err(syntax(src, start, SyntaxKind::MismatchedClose));
                        }
                        let (name, inverted, _, body) = stack.pop().expect("checked above");
                        let parent = stack.len() - 1;
                        stack[parent].3.push(Node::Section {
                            name,
                            inverted,
                            body,
                        });
                    }
                }
            }
        }
    }
    if stack.len() != 1 {
        let start = stack.last().map_or(0, |s| s.2);
        return Err(syntax(src, start, SyntaxKind::UnclosedSection));
    }
    Ok(stack.pop().expect("the root").3)
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A partial's source with `indent` put before each of its lines.
fn indented(source: &str, indent: &str) -> String {
    if indent.is_empty() {
        return source.into();
    }
    let mut out = String::new();
    for line in source.split_inclusive('\n') {
        if line != "\n" && line != "\r\n" {
            out.push_str(indent);
        }
        out.push_str(line);
    }
    out
}

struct Renderer<'a, 'o> {
    options: &'a Options<'o>,
    sink: Sink,
    iterations: usize,
}

impl Renderer<'_, '_> {
    fn walk(&mut self, nodes: &[Node], stack: &mut Vec<&Value>, depth: usize) -> Result<(), Error> {
        for node in nodes {
            match node {
                Node::Text(t) => self.sink.push(t)?,
                Node::Var { name, raw } => {
                    if let Some(text) = lookup(stack, name).and_then(text_of) {
                        if *raw || self.options.mustache.escape == Escape::None {
                            self.sink.push(&text)?;
                        } else {
                            self.sink.push(&escape(&text))?;
                        }
                    }
                }
                Node::Section {
                    name,
                    inverted,
                    body,
                } => {
                    let value = lookup(stack, name);
                    if *inverted {
                        if !truthy(value) {
                            self.walk(body, stack, depth)?;
                        }
                    } else if truthy(value) {
                        let value = value.expect("truthy implies present");
                        let items = core::slice::from_ref(value);
                        let items = match value {
                            Value::Array(list) => list.as_slice(),
                            _ => items,
                        };
                        for item in items {
                            self.iterations += 1;
                            if self.iterations > self.options.limits.max_iterations {
                                return Err(Error::Limit(Limit::Iterations));
                            }
                            stack.push(item);
                            let done = self.walk(body, stack, depth);
                            stack.pop();
                            done?;
                        }
                    }
                }
                Node::Partial { name, indent } => {
                    let max = self.options.limits.max_depth;
                    if depth + 1 >= max {
                        return Err(Error::Limit(Limit::Depth));
                    }
                    let Some(source) = self.options.resolver.and_then(|r| r.read(name)) else {
                        continue;
                    };
                    let tree = parse(&indented(&source, indent), max, depth + 1)?;
                    self.walk(&tree, stack, depth + 1)?;
                }
            }
        }
        Ok(())
    }
}

pub(super) fn render(text: &str, data: &Value, options: &Options<'_>) -> Result<String, Error> {
    let tree = parse(text, options.limits.max_depth, 0)?;
    let mut renderer = Renderer {
        options,
        sink: Sink::new(options.limits.max_output),
        iterations: 0,
    };
    let mut stack = vec![data];
    renderer.walk(&tree, &mut stack, 0)?;
    Ok(renderer.sink.finish())
}
