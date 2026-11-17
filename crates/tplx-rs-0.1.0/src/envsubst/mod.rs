//! `$NAME` and `${NAME}` as envsubst writes them, in three dialects.

mod expand;
mod glob;

use crate::{error::Error, options::Options};
use alloc::{collections::BTreeMap, string::String, vec::Vec};
use serde_json::Value;

/// Which envsubst the syntax follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Dialect {
    /// GNU gettext: `$NAME` and `${NAME}` and nothing else. `$$` is not an escape, and an
    /// expression it does not know (`${A:-x}`) is left as text.
    #[default]
    Gnu,
    /// github.com/a8m/envsubst: both forms, POSIX defaults (`:-`, `-`), assignment (`:=`, `=`),
    /// alternates (`:+`, `+`) whose words are substituted, and `$$` as an escape.
    A8m,
    /// github.com/drone/envsubst: `${NAME}` only, defaults, slices, replacements, trims, case
    /// changes and length, and `$$` as an escape.
    Drone,
}

/// Which spellings of a placeholder are substituted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Pattern {
    /// `${NAME}` only.
    Braced,
    /// `$NAME` only.
    Unbraced,
    /// Both.
    All,
}

/// A syntax feature of the braced form that a dialect may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Feature {
    /// A doubled `$` is one literal `$`.
    Escape,
    /// `${NAME-word}` (unset) and `${NAME:-word}` (unset or empty).
    Default,
    /// `${NAME=word}` and `${NAME:=word}`, read as `-` and `:-`.
    Assign,
    /// `${NAME+word}` (set) and `${NAME:+word}` (set and not empty).
    Alternate,
    /// A `word` is itself substituted (`${A:-${B}}`); otherwise it is text.
    Nested,
    /// One layer of matching quotes is stripped from a `word`.
    Unquote,
    /// `${NAME:offset}` and `${NAME:offset:length}`, as bash (`${A: -2}`).
    Slice,
    /// `${NAME/pattern/text}`, `//` every match, `/#` at the start, `/%` at the end; the pattern
    /// is a bash glob (`*`, `?`, `[...]`).
    Replace,
    /// `${NAME#pattern}`, `##`, `%`, `%%`: the shortest or longest matching prefix or suffix removed.
    Trim,
    /// `${NAME^}`, `${NAME^^}`, `${NAME,}`, `${NAME,,}`.
    Case,
    /// `${#NAME}`.
    Length,
}

impl Feature {
    const fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// A set of [`Feature`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Features(u16);

impl Features {
    /// No feature.
    #[must_use]
    pub const fn none() -> Self {
        Self(0)
    }

    /// The features in `list`.
    #[must_use]
    pub fn of(list: &[Feature]) -> Self {
        Self(list.iter().fold(0, |bits, f| bits | f.bit()))
    }

    /// This set and `feature`.
    #[must_use]
    pub fn with(self, feature: Feature) -> Self {
        Self(self.0 | feature.bit())
    }

    /// Whether `feature` is in the set.
    #[must_use]
    pub fn contains(self, feature: Feature) -> bool {
        self.0 & feature.bit() != 0
    }
}

/// What an undefined name does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Undefined {
    /// [`Error::Undefined`], as tplx's default.
    #[default]
    Strict,
    /// Renders as nothing, as GNU envsubst does.
    Lenient,
    /// The expression is written back as it was, as nginx's own `$request_uri` should be.
    Debug,
}

/// The envsubst engine's settings.
///
/// Start from [`EnvsubstOptions::default`] (GNU, strict) and assign the fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct EnvsubstOptions {
    /// The dialect, which gives the pattern and the features unless they are set.
    pub dialect: Dialect,
    /// Which spellings are substituted; `None` is the dialect's.
    pub pattern: Option<Pattern>,
    /// Which features are on; `None` is the dialect's.
    pub features: Option<Features>,
    /// What a name with no value does.
    pub undefined: Undefined,
}

impl EnvsubstOptions {
    /// The pattern and features in force.
    #[must_use]
    pub fn syntax(&self) -> (Pattern, Features) {
        let (pattern, features) = match self.dialect {
            Dialect::Gnu => (Pattern::All, Features::none()),
            Dialect::A8m => (
                Pattern::All,
                Features::of(&[
                    Feature::Escape,
                    Feature::Default,
                    Feature::Assign,
                    Feature::Alternate,
                    Feature::Nested,
                ]),
            ),
            Dialect::Drone => (
                Pattern::Braced,
                Features::of(&[
                    Feature::Escape,
                    Feature::Default,
                    Feature::Assign,
                    Feature::Nested,
                    Feature::Slice,
                    Feature::Replace,
                    Feature::Trim,
                    Feature::Case,
                    Feature::Length,
                ]),
            ),
        };
        (
            self.pattern.unwrap_or(pattern),
            self.features.unwrap_or(features),
        )
    }
}

/// The data as the placeholders read it: one level of text keyed by the path joined with `_`
/// and upper-cased, a list item by its index, `null` empty and a boolean `true` or `false`.
fn flatten(data: &Value, max_depth: usize) -> BTreeMap<String, String> {
    fn walk(
        value: &Value,
        path: &mut Vec<String>,
        out: &mut BTreeMap<String, String>,
        left: usize,
    ) {
        match value {
            Value::Object(map) if left > 0 => {
                for (key, item) in map {
                    path.push(key.clone());
                    walk(item, path, out, left - 1);
                    path.pop();
                }
            }
            Value::Array(items) if left > 0 => {
                for (index, item) in items.iter().enumerate() {
                    path.push(alloc::format!("{index}"));
                    walk(item, path, out, left - 1);
                    path.pop();
                }
            }
            Value::Object(_) | Value::Array(_) => {}
            scalar => {
                if !path.is_empty() {
                    let text = match scalar {
                        Value::Null => String::new(),
                        Value::Bool(b) => String::from(if *b { "true" } else { "false" }),
                        Value::Number(n) => alloc::format!("{n}"),
                        Value::String(s) => s.clone(),
                        _ => return,
                    };
                    out.insert(path.join("_").to_uppercase(), text);
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(data, &mut Vec::new(), &mut out, max_depth);
    out
}

/// Renders `text` as envsubst does, in the dialect of [`Options::envsubst`].
///
/// The names come from `data`, flattened (`{"a": {"b": 1}}` is `A_B`) and matched
/// case-insensitively, and an object is expected: any other data has no names. A name with no
/// value follows [`Undefined`] (default: an error). A value is inserted as it is and never read
/// again. In the default GNU dialect only `$NAME` and `${NAME}` are read, `$$` is not an escape,
/// and anything else is left as text.
///
/// # Errors
///
/// [`Error::Undefined`] under [`Undefined::Strict`], the default; [`Error::Limit`] past a limit.
///
/// # Examples
///
/// ```
/// use tplx_rs::{render_envsubst, Options};
/// let data = serde_json::json!({"NAME": "world"});
/// let out = render_envsubst("hello $NAME ${NAME}!", &data, &Options::new())?;
/// assert_eq!(out, "hello world world!");
/// # Ok::<(), tplx_rs::Error>(())
/// ```
pub fn render_envsubst(text: &str, data: &Value, options: &Options<'_>) -> Result<String, Error> {
    let variables = flatten(data, options.limits.max_depth);
    expand::render(text, &variables, options)
}
