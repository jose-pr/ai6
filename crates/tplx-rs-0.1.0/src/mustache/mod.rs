//! Mustache as the official specification defines it.

mod standard;

use crate::{error::Error, options::Options};
use alloc::string::String;
use serde_json::Value;

/// A name as written: dotted parts, or no parts for the implicit iterator `.`.
pub(crate) type Name = alloc::vec::Vec<String>;

/// How `{{x}}` writes a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Escape {
    /// `& < > " '` become entities, as the specification says.
    #[default]
    Html,
    /// The value as it is, as `{{{x}}}` always does.
    None,
}

/// The Mustache engine's settings.
///
/// Start from [`MustacheOptions::default`] and assign the fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct MustacheOptions {
    /// How `{{x}}` is written; `{{{x}}}` and `{{&x}}` never escape.
    pub escape: Escape,
}

/// JSON truthiness: missing, `null`, `false` and an empty list are false; `0`, `""` and `{}` are not.
pub(crate) fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::Array(items)) => !items.is_empty(),
        Some(_) => true,
    }
}

/// The value of `name`: its first part in the innermost context that has it as a key,
/// then each later part as a key of what was found.
pub(crate) fn lookup<'v>(stack: &[&'v Value], name: &[String]) -> Option<&'v Value> {
    let Some((first, rest)) = name.split_first() else {
        return stack.last().copied();
    };
    let mut value = stack
        .iter()
        .rev()
        .find_map(|context| context.as_object()?.get(first))?;
    for key in rest {
        value = value.as_object()?.get(key)?;
    }
    Some(value)
}

/// Renders `text` as Mustache the way the official specification defines it.
///
/// Supports variables (`{{x}}` escaped as [`MustacheOptions::escape`] says, `{{{x}}}` and
/// `{{&x}}` not), dotted names, sections and inverted sections, comments, set-delimiter tags,
/// standalone-line handling and partials (`{{> name}}`) from [`Options::resolver`]. A partial
/// the resolver lacks renders as nothing. Lambdas, inheritance (`{{< }}`, `{{$ }}`) and dynamic
/// names are not supported, and the first two sigils are
/// [`SyntaxKind::UnsupportedTag`](crate::SyntaxKind).
///
/// Truthiness is JSON's: missing, `null`, `false` and an empty list are false. A name no
/// context has renders as nothing, so no undefined name is an error.
///
/// # Errors
///
/// [`Error::Syntax`] for a malformed template, [`Error::Limit`] past a limit.
///
/// # Examples
///
/// ```
/// use tplx_rs::{render_mustache, Options};
/// let data = serde_json::json!({"items": ["a", "b"], "x": "<&>"});
/// let out = render_mustache("{{#items}}[{{.}}]{{/items}} {{x}} {{{x}}}", &data, &Options::new())?;
/// assert_eq!(out, "[a][b] &lt;&amp;&gt; <&>");
/// # Ok::<(), tplx_rs::Error>(())
/// ```
pub fn render_mustache(text: &str, data: &Value, options: &Options<'_>) -> Result<String, Error> {
    standard::render(text, data, options)
}
