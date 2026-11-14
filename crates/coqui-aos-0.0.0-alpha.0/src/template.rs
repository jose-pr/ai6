//! Templating (`spec/v0/harness.md#templating`): `${NAME}` in the context a consumer assembles
//! and in the body of a bundled skill it serves. Only the names listed, the overlay's non-sensitive
//! options among them, are replaced; every other text is itself. Nothing runs, no file is read, and
//! the installed files are never changed.

use alloc::collections::BTreeMap;
use alloc::string::String;

use serde_json::Value;

use crate::error::{Class, Error};
use crate::front_matter::{SplitCase, context_file_text, split_front_matter};
use crate::limits::Settings;
use crate::names::is_variable_name;
use crate::read::Document;

/// What a consumer knows about the overlay a text is read for. A name with no value is left as
/// written in the text.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TemplateFacts {
    /// `AOS_OVERLAY_ROOT`: the absolute root the consumer exposes, as it holds it.
    pub root: Option<String>,
    /// `AOS_OVERLAY_STATE`: the overlay's state directory, when it has one.
    pub state: Option<String>,
    /// `AOS_OVERLAY_NAME`: the overlay's normalized identity.
    pub name: Option<String>,
    /// `AOS_OVERLAY_SCOPE`: the label the consumer gives the scope the overlay is active in.
    pub scope: Option<String>,
    /// `AOS_PROJECT_ROOT`: the absolute root of the project the session works in.
    pub project: Option<String>,
    /// The values the user configured for the overlay's options, by name. A name the overlay does
    /// not declare is ignored.
    pub options: Document,
}

impl TemplateFacts {
    /// Facts with no value for any name.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `AOS_OVERLAY_ROOT`.
    #[must_use]
    pub fn root(mut self, root: impl Into<String>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// Sets `AOS_OVERLAY_STATE`.
    #[must_use]
    pub fn state(mut self, state: impl Into<String>) -> Self {
        self.state = Some(state.into());
        self
    }

    /// Sets `AOS_OVERLAY_NAME`.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets `AOS_OVERLAY_SCOPE`.
    #[must_use]
    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        self.scope = Some(scope.into());
        self
    }

    /// Sets `AOS_PROJECT_ROOT`.
    #[must_use]
    pub fn project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into());
        self
    }

    /// Sets the configured option values.
    #[must_use]
    pub fn options(mut self, options: Document) -> Self {
        self.options = options;
        self
    }
}

/// The names that are replaced and their text, from [`template_values`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Template {
    values: BTreeMap<String, String>,
}

impl Template {
    /// The text a name is replaced by, when it is replaced.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
}

/// Whether `value` is of the option type `kind` (`string`, `integer` or `boolean`); a boolean is
/// no integer and a float is none.
pub(crate) fn value_is(kind: &str, value: &Value) -> bool {
    match kind {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        _ => false,
    }
}

fn option_text(kind: &str, name: &str, value: &Value) -> Result<String, Error> {
    if !value_is(kind, value) {
        return Err(Error::new(Class::InvalidOption, name));
    }
    Ok(match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

/// The names `manifest` and `facts` give a value, to hand to [`template`], [`context_text`] and
/// [`skill_text`]. A sensitive option never has one. An option's value is the configured one, else
/// its default, else none.
///
/// # Errors
///
/// `invalid-option`, the option's name its subject, when a configured value or a default is not of
/// the option's declared type; it is never converted.
pub fn template_values(manifest: &Document, facts: &TemplateFacts) -> Result<Template, Error> {
    let mut values = BTreeMap::new();
    for (name, value) in [
        ("AOS_OVERLAY_ROOT", &facts.root),
        ("AOS_OVERLAY_STATE", &facts.state),
        ("AOS_OVERLAY_NAME", &facts.name),
        ("AOS_OVERLAY_SCOPE", &facts.scope),
        ("AOS_PROJECT_ROOT", &facts.project),
    ] {
        if let Some(value) = value {
            values.insert(name.to_owned(), value.clone());
        }
    }
    if let Some(options) = manifest.get("options").and_then(Value::as_object) {
        for (name, declared) in options {
            let Some(declared) = declared.as_object() else {
                continue;
            };
            if declared.get("sensitive") == Some(&Value::Bool(true)) || !is_variable_name(name) {
                continue;
            }
            let kind = declared.get("type").and_then(Value::as_str).unwrap_or("");
            if let Some(value) = facts.options.get(name).or_else(|| declared.get("default")) {
                values.insert(name.clone(), option_text(kind, name, value)?);
            }
        }
    }
    Ok(Template { values })
}

/// `text` with each `${NAME}` whose name `values` holds replaced, read from left to right: `$${` is
/// `${`, and every other text is itself, an unknown name, one with an operator, `$NAME`, `$(...)`
/// and a lone `$$` included. A value is inserted as it is and never read again.
#[must_use]
pub fn template(text: &str, values: &Template) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if let Some(after) = rest.strip_prefix("$${") {
            out.push_str("${");
            rest = after;
            continue;
        }
        if let Some(after) = rest.strip_prefix("${")
            && let Some(end) = after.find('}')
            && let Some(value) = values.get(&after[..end])
        {
            out.push_str(value);
            rest = &after[end + 1..];
            continue;
        }
        out.push('$');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

fn utf8(data: &[u8]) -> Result<&str, Error> {
    core::str::from_utf8(data).map_err(|_| Error::new(Class::InvalidUtf8, ""))
}

/// A context file as the agent receives it: a closed front matter block dropped, the rest
/// templated. The file is as it is installed; nothing is written.
///
/// # Errors
///
/// `too-large` when the front matter head bound of `settings` ended before the split did, and
/// `invalid-utf8` for text that is not UTF-8.
pub fn context_text(data: &[u8], values: &Template, settings: &Settings) -> Result<String, Error> {
    Ok(template(utf8(context_file_text(data, settings)?)?, values))
}

/// A bundled skill's `SKILL.md` as served: the front matter exactly as written, the body
/// templated.
///
/// # Errors
///
/// As [`context_text`].
pub fn skill_text(data: &[u8], values: &Template, settings: &Settings) -> Result<String, Error> {
    let split = split_front_matter(data, settings);
    if split.case == SplitCase::TooLarge {
        return Err(Error::new(Class::TooLarge, "front matter")
            .over("manifest_bytes")
            .on_line(1));
    }
    let offset = split.body_offset.unwrap_or(0).min(data.len());
    let mut out = String::from(utf8(&data[..offset])?);
    out.push_str(&template(utf8(&data[offset..])?, values));
    Ok(out)
}
