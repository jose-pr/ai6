//! The expansion itself: where a placeholder starts, what its expression means.

use super::{glob::Glob, Feature, Features, Pattern, Undefined};
use crate::{
    error::{Error, Limit},
    options::Options,
    sink::position,
};
use alloc::{collections::BTreeMap, string::String, vec::Vec};

/// An undefined name under `debug` keeps the expression; anything else is an error.
enum Flow {
    Keep,
    Fail(Error),
}

impl From<Error> for Flow {
    fn from(error: Error) -> Self {
        Flow::Fail(error)
    }
}

struct Expander<'a> {
    root: &'a str,
    variables: &'a BTreeMap<String, String>,
    pattern: Pattern,
    features: Features,
    undefined: Undefined,
    max_output: usize,
    max_depth: usize,
    budget: usize,
}

fn is_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

/// The name at the start of `text`, if there is one.
fn name_at(text: &str) -> Option<&str> {
    let b = text.as_bytes();
    if !b.first().is_some_and(|c| is_start(*c)) {
        return None;
    }
    let end = b
        .iter()
        .position(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))
        .unwrap_or(b.len());
    Some(&text[..end])
}

/// Where the `}` that closes a brace opened just before `from` is, if there is one.
fn closing(text: &str, from: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = from;
    while i < b.len() {
        if text[i..].starts_with("${") {
            depth += 1;
            i += 2;
            continue;
        }
        if b[i] == b'}' {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
        }
        i += 1;
    }
    None
}

/// `text` split at its first `separator` that no backslash precedes.
fn split_unescaped(text: &str, separator: char) -> (&str, Option<&str>) {
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c == separator {
            return (&text[..i], Some(&text[i + c.len_utf8()..]));
        }
    }
    (text, None)
}

fn slice(value: &str, offset: i64, length: Option<i64>) -> String {
    let chars: Vec<char> = value.chars().collect();
    let size = i64::try_from(chars.len()).unwrap_or(i64::MAX);
    let mut offset = offset;
    if offset < 0 {
        offset += size;
        if offset < 0 {
            return String::new();
        }
    }
    let end = match length {
        None => size,
        Some(l) if l >= 0 => offset.saturating_add(l),
        Some(l) => size + l,
    };
    if end <= offset || offset >= size {
        return String::new();
    }
    chars[offset as usize..end.min(size) as usize]
        .iter()
        .collect()
}

/// The `offset` and `length` of a `:offset[:length]` expression.
fn parse_slice(rest: &str) -> Option<(i64, Option<i64>)> {
    let after = rest.strip_prefix(':')?;
    let trimmed = after.trim_start();
    let leading = after.len() - trimmed.len();
    let digits = |s: &str| s.bytes().take_while(u8::is_ascii_digit).count();
    let (offset, tail) = if let Some(inner) = trimmed.strip_prefix('(') {
        let inner = inner.trim_start();
        let sign = usize::from(inner.starts_with('-'));
        let n = digits(&inner[sign..]);
        if n == 0 {
            return None;
        }
        let tail = inner[sign + n..].trim_start().strip_prefix(')')?;
        (inner[..sign + n].parse().ok()?, tail)
    } else if digits(trimmed) > 0 {
        let n = digits(trimmed);
        (trimmed[..n].parse().ok()?, &trimmed[n..])
    } else if leading > 0 && trimmed.starts_with('-') && digits(&trimmed[1..]) > 0 {
        let n = 1 + digits(&trimmed[1..]);
        (trimmed[..n].parse().ok()?, &trimmed[n..])
    } else {
        return None;
    };
    let tail = tail.trim_start();
    if tail.is_empty() {
        return Some((offset, None));
    }
    let length = tail.strip_prefix(':')?.trim_start();
    let sign = usize::from(length.starts_with('-'));
    let n = digits(&length[sign..]);
    if n == 0 || !length[sign + n..].trim_start().is_empty() {
        return None;
    }
    Some((offset, Some(length[..sign + n].parse().ok()?)))
}

impl Expander<'_> {
    fn value(&self, name: &str, whole: &str, at: usize) -> Result<String, Flow> {
        if let Some(value) = self.variables.get(&name.to_uppercase()) {
            return Ok(value.clone());
        }
        match self.undefined {
            Undefined::Debug => Err(Flow::Keep),
            Undefined::Lenient => Ok(String::new()),
            Undefined::Strict => {
                let _ = whole;
                let (line, column) = position(self.root, at);
                Err(Flow::Fail(Error::Undefined { line, column }))
            }
        }
    }

    fn check(&self, out: &str) -> Result<(), Error> {
        if out.len() > self.max_output {
            return Err(Error::Limit(Limit::Output));
        }
        Ok(())
    }

    /// `text` with every placeholder substituted. `anchor` is where errors point: the
    /// expression a nested word belongs to, or `None` at the top, where offsets are the text's.
    fn expand(&self, text: &str, anchor: Option<usize>, depth: usize) -> Result<String, Error> {
        if depth > self.max_depth {
            return Err(Error::Limit(Limit::Depth));
        }
        let braced = matches!(self.pattern, Pattern::Braced | Pattern::All);
        let unbraced = matches!(self.pattern, Pattern::Unbraced | Pattern::All);
        let mut out = String::new();
        let mut pos = 0;
        loop {
            let Some(rel) = text[pos..].find('$') else {
                out.push_str(&text[pos..]);
                self.check(&out)?;
                return Ok(out);
            };
            let start = pos + rel;
            out.push_str(&text[pos..start]);
            self.check(&out)?;
            let after = start + 1;
            let here = anchor.unwrap_or(start);
            if self.features.contains(Feature::Escape) && text[after..].starts_with('$') {
                out.push('$');
                pos = after + 1;
                continue;
            }
            if braced && text[after..].starts_with('{') {
                if let Some(close) = closing(text, after + 1) {
                    let whole = &text[start..=close];
                    if let Some(found) = self.braced(&text[after + 1..close], whole, here, depth)? {
                        out.push_str(&found);
                        pos = close + 1;
                        continue;
                    }
                }
            }
            if unbraced {
                if let Some(name) = name_at(&text[after..]) {
                    let end = after + name.len();
                    match self.value(name, &text[start..end], here) {
                        Ok(v) => out.push_str(&v),
                        Err(Flow::Keep) => out.push_str(&text[start..end]),
                        Err(Flow::Fail(e)) => return Err(e),
                    }
                    pos = end;
                    continue;
                }
            }
            out.push('$');
            pos = after;
        }
    }

    fn word(&self, word: &str, at: usize, depth: usize) -> Result<String, Error> {
        let mut word = if self.features.contains(Feature::Nested) {
            self.expand(word, Some(at), depth + 1)?
        } else {
            String::from(word)
        };
        if self.features.contains(Feature::Unquote) {
            let b = word.as_bytes();
            if b.len() >= 2 && b[0] == b[b.len() - 1] && (b[0] == b'"' || b[0] == b'\'') {
                word = String::from(&word[1..word.len() - 1]);
            }
        }
        Ok(word)
    }

    /// The text for `{body}`, or `None` for an expression this syntax does not know.
    fn braced(
        &self,
        body: &str,
        whole: &str,
        at: usize,
        depth: usize,
    ) -> Result<Option<String>, Error> {
        match self.braced_inner(body, whole, at, depth) {
            Ok(found) => Ok(found),
            Err(Flow::Keep) => Ok(Some(String::from(whole))),
            Err(Flow::Fail(e)) => Err(e),
        }
    }

    fn braced_inner(
        &self,
        body: &str,
        whole: &str,
        at: usize,
        depth: usize,
    ) -> Result<Option<String>, Flow> {
        let has = |f| self.features.contains(f);
        if has(Feature::Length) {
            if let Some(name) = body.strip_prefix('#') {
                if name_at(name) == Some(name) {
                    return Ok(Some(alloc::format!(
                        "{}",
                        self.value(name, whole, at)?.chars().count()
                    )));
                }
            }
        }
        let Some(key) = name_at(body) else {
            return Ok(None);
        };
        let rest = &body[key.len()..];
        if rest.is_empty() {
            return self.value(key, whole, at).map(Some);
        }
        let value = self.variables.get(&key.to_uppercase());
        for (operator, feature) in [
            (":-", Feature::Default),
            (":=", Feature::Assign),
            (":+", Feature::Alternate),
            ("-", Feature::Default),
            ("=", Feature::Assign),
            ("+", Feature::Alternate),
        ] {
            if let Some(word) = rest.strip_prefix(operator) {
                if !has(feature) {
                    return Ok(None);
                }
                let empty_counts = operator.starts_with(':');
                let present = value.is_some_and(|v| !(empty_counts && v.is_empty()));
                return if operator.ends_with('+') {
                    Ok(Some(if present {
                        self.word(word, at, depth)?
                    } else {
                        String::new()
                    }))
                } else if present {
                    Ok(value.cloned())
                } else {
                    Ok(Some(self.word(word, at, depth)?))
                };
            }
        }
        if rest.starts_with(':') {
            let Some((offset, length)) = parse_slice(rest).filter(|_| has(Feature::Slice)) else {
                return Ok(None);
            };
            let value = self.value(key, whole, at)?;
            return Ok(Some(slice(&value, offset, length)));
        }
        if let Some(spec) = rest.strip_prefix('/') {
            if !has(Feature::Replace) {
                return Ok(None);
            }
            let kind = spec.chars().next().filter(|c| matches!(c, '/' | '#' | '%'));
            let spec = &spec[kind.map_or(0, char::len_utf8)..];
            let (pattern, text) = split_unescaped(spec, '/');
            let value = self.value(key, whole, at)?;
            return Ok(Some(self.replace(
                &value,
                kind,
                pattern,
                text.unwrap_or(""),
            )?));
        }
        if rest.starts_with(['#', '%']) {
            if !has(Feature::Trim) {
                return Ok(None);
            }
            let operator_len = if rest.starts_with("##") || rest.starts_with("%%") {
                2
            } else {
                1
            };
            let value = self.value(key, whole, at)?;
            return Ok(Some(self.trim(
                &value,
                &rest[..operator_len],
                &rest[operator_len..],
            )?));
        }
        if matches!(rest, "^" | "^^" | "," | ",,") {
            if !has(Feature::Case) {
                return Ok(None);
            }
            let value = self.value(key, whole, at)?;
            return Ok(Some(match rest {
                "^^" => value.to_uppercase(),
                ",," => value.to_lowercase(),
                _ => {
                    let mut chars = value.chars();
                    match chars.next() {
                        Some(first) => {
                            let head: String = if rest == "^" {
                                first.to_uppercase().collect()
                            } else {
                                first.to_lowercase().collect()
                            };
                            head + chars.as_str()
                        }
                        None => String::new(),
                    }
                }
            }));
        }
        Ok(None)
    }

    fn trim(&self, value: &str, operator: &str, pattern: &str) -> Result<String, Error> {
        let chars: Vec<char> = value.chars().collect();
        let glob = Glob::new(pattern);
        let mut budget = self.budget;
        if operator.starts_with('#') {
            let end = glob.match_at(&chars, 0, operator == "#", false, &mut budget)?;
            return Ok(end.map_or_else(|| String::from(value), |e| chars[e..].iter().collect()));
        }
        let starts: Vec<usize> = if operator == "%" {
            (0..=chars.len()).rev().collect()
        } else {
            (0..=chars.len()).collect()
        };
        for start in starts {
            if glob
                .match_at(&chars, start, false, true, &mut budget)?
                .is_some()
            {
                return Ok(chars[..start].iter().collect());
            }
        }
        Ok(String::from(value))
    }

    fn replace(
        &self,
        value: &str,
        kind: Option<char>,
        pattern: &str,
        text: &str,
    ) -> Result<String, Error> {
        if pattern.is_empty() {
            return Ok(String::from(value));
        }
        let chars: Vec<char> = value.chars().collect();
        let glob = Glob::new(pattern);
        let mut budget = self.budget;
        match kind {
            Some('#') => {
                let end = glob.match_at(&chars, 0, false, false, &mut budget)?;
                return Ok(end.map_or_else(
                    || String::from(value),
                    |e| String::from(text) + &chars[e..].iter().collect::<String>(),
                ));
            }
            Some('%') => {
                for start in 0..=chars.len() {
                    if glob
                        .match_at(&chars, start, false, true, &mut budget)?
                        .is_some()
                    {
                        return Ok(chars[..start].iter().collect::<String>() + text);
                    }
                }
                return Ok(String::from(value));
            }
            _ => {}
        }
        let mut out = String::new();
        let mut position = 0;
        'scan: while position <= chars.len() {
            let mut found = None;
            for start in position..=chars.len() {
                if let Some(end) = glob.match_at(&chars, start, false, false, &mut budget)? {
                    found = Some((start, end));
                    break;
                }
            }
            let Some((start, end)) = found else { break };
            if end == start {
                if start >= chars.len() {
                    break 'scan;
                }
                out.extend(&chars[position..=start]);
                position = start + 1;
                continue;
            }
            out.extend(&chars[position..start]);
            out.push_str(text);
            position = end;
            self.check(&out)?;
            if kind != Some('/') {
                break;
            }
        }
        if position < chars.len() {
            out.extend(&chars[position..]);
        }
        self.check(&out)?;
        Ok(out)
    }
}

pub(super) fn render(
    text: &str,
    variables: &BTreeMap<String, String>,
    options: &Options<'_>,
) -> Result<String, Error> {
    let settings = options.envsubst;
    let (pattern, features) = settings.syntax();
    let expander = Expander {
        root: text,
        variables,
        pattern,
        features,
        undefined: settings.undefined,
        max_output: options.limits.max_output,
        max_depth: options.limits.max_depth,
        budget: options.limits.max_iterations.saturating_mul(10),
    };
    expander.expand(text, None, 0)
}
