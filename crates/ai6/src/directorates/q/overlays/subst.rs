//! Substitution in env values (`spec/v0/environment.md#substitution`): braced `${NAME}` and
//! the shell-style operators AOS lists, `$$` for a `$`. Nothing runs and no file is read.

use std::fmt;

/// Why a value could not be substituted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstitutionError {
    /// `undefined-variable`, `secret-placeholder` or `invalid-pattern`.
    pub class: &'static str,
    /// The placeholder's variable, or the pattern; never a value.
    pub name: String,
}

impl fmt::Display for SubstitutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.class, self.name)
    }
}

impl std::error::Error for SubstitutionError {}

/// What a placeholder reads: a variable's value, `None` when unset, or a refusal for a
/// variable that holds a secret.
pub type Lookup<'a> = dyn Fn(&str) -> Result<Option<String>, SubstitutionError> + 'a;

/// `text` with every placeholder replaced, reading variables through `lookup`.
///
/// # Errors
///
/// A [`SubstitutionError`] for a name that is unset where its value is needed, a secret read
/// by a placeholder, or an invalid pattern.
pub fn substitute(text: &str, lookup: &Lookup<'_>) -> Result<String, SubstitutionError> {
    let chars: Vec<char> = text.chars().collect();
    substitute_chars(&chars, lookup)
}

fn substitute_chars(text: &[char], lookup: &Lookup<'_>) -> Result<String, SubstitutionError> {
    let mut out = String::new();
    let mut index = 0;
    while index < text.len() {
        let c = text[index];
        if c != '$' {
            out.push(c);
            index += 1;
            continue;
        }
        match text.get(index + 1) {
            Some('$') => {
                out.push('$');
                index += 2;
                continue;
            }
            Some('{') => {
                if let Some(close) = closing_brace(text, index + 2)
                    && let Some(found) = expression(&text[index + 2..close], lookup)?
                {
                    out.push_str(&found);
                    index = close + 1;
                    continue;
                }
            }
            _ => {}
        }
        out.push('$');
        index += 1;
    }
    Ok(out)
}

/// The `}` that ends an expression starting at `index`: the first that closes no `${` opened
/// inside it.
fn closing_brace(text: &[char], mut index: usize) -> Option<usize> {
    let mut depth = 0usize;
    while index < text.len() {
        if text[index] == '$' && text.get(index + 1) == Some(&'{') {
            depth += 1;
            index += 2;
            continue;
        }
        if text[index] == '}' {
            if depth == 0 {
                return Some(index);
            }
            depth -= 1;
        }
        index += 1;
    }
    None
}

fn name_length(text: &[char]) -> usize {
    match text.first() {
        Some(c) if c.is_ascii_alphabetic() || *c == '_' => text
            .iter()
            .position(|c| !(c.is_ascii_alphanumeric() || *c == '_'))
            .unwrap_or(text.len()),
        _ => 0,
    }
}

fn required(name: &str, lookup: &Lookup<'_>) -> Result<Vec<char>, SubstitutionError> {
    lookup(name)?
        .map(|value| value.chars().collect())
        .ok_or_else(|| SubstitutionError {
            class: "undefined-variable",
            name: name.to_owned(),
        })
}

/// The text for `${body}`, or `None` when it is no expression AOS defines.
fn expression(body: &[char], lookup: &Lookup<'_>) -> Result<Option<String>, SubstitutionError> {
    if body.first() == Some(&'#') {
        let length = name_length(&body[1..]);
        if length > 0 && length == body.len() - 1 {
            let name: String = body[1..].iter().collect();
            return Ok(Some(required(&name, lookup)?.len().to_string()));
        }
    }
    let length = name_length(body);
    if length == 0 {
        return Ok(None);
    }
    let key: String = body[..length].iter().collect();
    let rest = &body[length..];
    if rest.is_empty() {
        return Ok(Some(required(&key, lookup)?.into_iter().collect()));
    }
    for operator in [":-", ":=", ":+", "-", "=", "+"] {
        let operator: Vec<char> = operator.chars().collect();
        if rest.starts_with(&operator) {
            let word = &rest[operator.len()..];
            let value = lookup(&key)?;
            let present = value
                .as_ref()
                .is_some_and(|value| !(operator[0] == ':' && value.is_empty()));
            return Ok(Some(if operator.last() == Some(&'+') {
                if present {
                    substitute_chars(word, lookup)?
                } else {
                    String::new()
                }
            } else {
                match value {
                    Some(value) if present => value,
                    _ => substitute_chars(word, lookup)?,
                }
            }));
        }
    }
    match rest[0] {
        ':' => {
            let Some((offset, length)) = slice_spec(&rest[1..]) else {
                return Ok(None);
            };
            let value = required(&key, lookup)?;
            Ok(Some(slice(&value, offset, length)))
        }
        '/' => {
            let spec = &rest[1..];
            let kind = spec
                .first()
                .copied()
                .filter(|c| matches!(c, '/' | '#' | '%'));
            let (pattern, replacement) = split_pattern(&spec[usize::from(kind.is_some())..]);
            let value = required(&key, lookup)?;
            Ok(Some(replace(&value, kind, pattern, replacement)?))
        }
        '#' | '%' => {
            let double = rest.get(1) == Some(&rest[0]);
            let operator_length = if double { 2 } else { 1 };
            let value = required(&key, lookup)?;
            Ok(Some(trim(
                &value,
                rest[0],
                double,
                &rest[operator_length..],
            )?))
        }
        _ => {
            let operator: String = rest.iter().collect();
            if !matches!(operator.as_str(), "^" | "^^" | "," | ",,") {
                return Ok(None);
            }
            let value: String = required(&key, lookup)?.into_iter().collect();
            Ok(Some(match operator.as_str() {
                "^^" => value.to_uppercase(),
                ",," => value.to_lowercase(),
                single => {
                    let mut chars = value.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => {
                            let mut out: String = if single == "^" {
                                first.to_uppercase().collect()
                            } else {
                                first.to_lowercase().collect()
                            };
                            out.extend(chars);
                            out
                        }
                    }
                }
            }))
        }
    }
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}')
}

/// Optionally signed digits at `text[*at..]`, saturating at the bounds of `i128`.
fn signed(text: &[char], at: &mut usize) -> Option<i128> {
    let negative = text.get(*at) == Some(&'-');
    let start = *at + usize::from(negative);
    let mut end = start;
    while text.get(end).is_some_and(char::is_ascii_digit) {
        end += 1;
    }
    if end == start {
        return None;
    }
    *at = end;
    let mut value: i128 = 0;
    for digit in &text[start..end] {
        value = value
            .saturating_mul(10)
            .saturating_add(i128::from(digit.to_digit(10).unwrap_or(0)));
    }
    Some(if negative { -value } else { value })
}

/// The offset and length of a slice after its `:`, or `None` when the text is no slice.
fn slice_spec(text: &[char]) -> Option<(i128, Option<i128>)> {
    let mut at = 0;
    while text.get(at).copied().is_some_and(is_space) {
        at += 1;
    }
    let spaced = at > 0;
    let offset = match text.get(at) {
        Some('(') => {
            at += 1;
            while text.get(at).copied().is_some_and(is_space) {
                at += 1;
            }
            let offset = signed(text, &mut at)?;
            while text.get(at).copied().is_some_and(is_space) {
                at += 1;
            }
            if text.get(at) != Some(&')') {
                return None;
            }
            at += 1;
            offset
        }
        Some(c) if c.is_ascii_digit() => signed(text, &mut at)?,
        Some('-') if spaced => signed(text, &mut at)?,
        _ => return None,
    };
    let mut length = None;
    if text.get(at) == Some(&':') {
        at += 1;
        while text.get(at).copied().is_some_and(is_space) {
            at += 1;
        }
        length = Some(signed(text, &mut at)?);
    }
    while text.get(at).copied().is_some_and(is_space) {
        at += 1;
    }
    (at == text.len()).then_some((offset, length))
}

fn slice(value: &[char], offset: i128, length: Option<i128>) -> String {
    let size = value.len() as i128;
    let mut offset = offset;
    if offset < 0 {
        offset += size;
        if offset < 0 {
            return String::new();
        }
    }
    let end = match length {
        None => size,
        Some(length) if length >= 0 => offset.saturating_add(length),
        Some(length) => size + length,
    };
    let (start, end) = (offset.min(size), end.min(size));
    if end <= start {
        return String::new();
    }
    value[start as usize..end as usize].iter().collect()
}

/// A replacement's pattern and text, split at the first `/` that follows no `\`.
fn split_pattern(spec: &[char]) -> (&[char], &[char]) {
    let mut index = 0;
    while index < spec.len() {
        if spec[index] == '\\' {
            index += 2;
            continue;
        }
        if spec[index] == '/' {
            return (&spec[..index], &spec[index + 1..]);
        }
        index += 1;
    }
    (spec, &[])
}

enum Token {
    Literal(char),
    Any,
    Star,
    Class(bool, Vec<(char, char)>),
}

impl Token {
    fn matches(&self, c: char) -> bool {
        match self {
            Self::Literal(literal) => *literal == c,
            Self::Any => true,
            Self::Star => false,
            Self::Class(negate, ranges) => {
                ranges.iter().any(|(low, high)| (*low..=*high).contains(&c)) != *negate
            }
        }
    }
}

fn glob(pattern: &[char]) -> Result<Vec<Token>, SubstitutionError> {
    let invalid = || SubstitutionError {
        class: "invalid-pattern",
        name: pattern.iter().collect(),
    };
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < pattern.len() {
        let c = pattern[index];
        if c == '\\' && index + 1 < pattern.len() {
            tokens.push(Token::Literal(pattern[index + 1]));
            index += 2;
            continue;
        }
        let close = if c == '[' {
            pattern
                .get(index + 2..)
                .and_then(|after| after.iter().position(|c| *c == ']'))
                .map(|at| index + 2 + at)
        } else {
            None
        };
        match (c, close) {
            ('*', _) => tokens.push(Token::Star),
            ('?', _) => tokens.push(Token::Any),
            ('[', Some(close)) => {
                let body = &pattern[index + 1..close];
                let negate = matches!(body.first(), Some('!' | '^'));
                let items = if negate { &body[1..] } else { body };
                if items.is_empty() {
                    return Err(invalid());
                }
                let mut ranges = Vec::new();
                let mut at = 0;
                while at < items.len() {
                    if at + 2 < items.len() && items[at + 1] == '-' {
                        let (low, high) = (items[at], items[at + 2]);
                        if high < low {
                            return Err(invalid());
                        }
                        ranges.push((low, high));
                        at += 3;
                    } else {
                        ranges.push((items[at], items[at]));
                        at += 1;
                    }
                }
                tokens.push(Token::Class(negate, ranges));
                index = close + 1;
                continue;
            }
            _ => tokens.push(Token::Literal(c)),
        }
        index += 1;
    }
    Ok(tokens)
}

/// Whether `tokens` match all of `text`.
fn whole(tokens: &[Token], text: &[char]) -> bool {
    let (mut t, mut p) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < tokens.len() && matches!(tokens[p], Token::Star) {
            star = Some((p, t));
            p += 1;
        } else if p < tokens.len() && tokens[p].matches(text[t]) {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    tokens[p..].iter().all(|token| matches!(token, Token::Star))
}

/// Every end at which the pattern matches from `start`, shortest first.
fn ends(tokens: &[Token], value: &[char], start: usize) -> Vec<usize> {
    (start..=value.len())
        .filter(|end| whole(tokens, &value[start..*end]))
        .collect()
}

fn replace(
    value: &[char],
    kind: Option<char>,
    pattern: &[char],
    replacement: &[char],
) -> Result<String, SubstitutionError> {
    let collect = |chars: &[char]| chars.iter().collect::<String>();
    if pattern.is_empty() {
        return Ok(collect(value));
    }
    let tokens = glob(pattern)?;
    let replacement = collect(replacement);
    match kind {
        Some('#') => Ok(match ends(&tokens, value, 0).last() {
            Some(end) => replacement + &collect(&value[*end..]),
            None => collect(value),
        }),
        Some('%') => Ok(
            match (0..=value.len()).find(|start| whole(&tokens, &value[*start..])) {
                Some(start) => collect(&value[..start]) + &replacement,
                None => collect(value),
            },
        ),
        _ => {
            let mut out = String::new();
            let mut position = 0;
            loop {
                let found = (position..value.len()).find_map(|start| {
                    ends(&tokens, value, start)
                        .last()
                        .filter(|end| **end > start)
                        .map(|end| (start, *end))
                });
                let Some((start, end)) = found else { break };
                out.push_str(&collect(&value[position..start]));
                out.push_str(&replacement);
                position = end;
                if kind != Some('/') {
                    break;
                }
            }
            out.push_str(&collect(&value[position..]));
            Ok(out)
        }
    }
}

fn trim(
    value: &[char],
    side: char,
    longest: bool,
    pattern: &[char],
) -> Result<String, SubstitutionError> {
    let tokens = glob(pattern)?;
    let collect = |chars: &[char]| chars.iter().collect::<String>();
    if side == '#' {
        let found = ends(&tokens, value, 0);
        let end = if longest { found.last() } else { found.first() };
        return Ok(end.map_or_else(|| collect(value), |end| collect(&value[*end..])));
    }
    let starts: Vec<usize> = (0..=value.len())
        .filter(|start| whole(&tokens, &value[*start..]))
        .collect();
    let start = if longest {
        starts.first()
    } else {
        starts.last()
    };
    Ok(start.map_or_else(|| collect(value), |start| collect(&value[..*start])))
}
