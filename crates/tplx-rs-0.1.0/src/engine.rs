use crate::{
    envsubst::render_envsubst, error::Error, mustache::render_mustache, options::Options,
    render_raw,
};
use alloc::string::String;
use core::{fmt, str::FromStr};
use serde_json::Value;

/// A template engine.
///
/// Names are lowercase and compared exactly, so a file that names its engine is never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Engine {
    /// The text as it is.
    Raw,
    /// `$NAME` and `${NAME}`, in the dialect of [`Options::envsubst`].
    Envsubst,
    /// Mustache as the official specification defines it.
    Mustache,
}

impl Engine {
    /// Every engine.
    pub const ALL: &'static [Engine] = &[Engine::Raw, Engine::Envsubst, Engine::Mustache];

    /// The engine's lowercase name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Engine::Raw => "raw",
            Engine::Envsubst => "envsubst",
            Engine::Mustache => "mustache",
        }
    }

    /// The file suffixes the engine claims, without the dot.
    #[must_use]
    pub fn suffixes(self) -> &'static [&'static str] {
        match self {
            Engine::Raw => &[],
            Engine::Envsubst => &["envsubst", "template"],
            Engine::Mustache => &["mustache", "mst"],
        }
    }

    /// The engine called `name` exactly (lowercase).
    #[must_use]
    pub fn by_name(name: &str) -> Option<Engine> {
        Self::ALL.iter().copied().find(|e| e.name() == name)
    }

    /// The engine that claims a file suffix, with or without the leading dot, any case.
    #[must_use]
    pub fn by_suffix(suffix: &str) -> Option<Engine> {
        let suffix = suffix.strip_prefix('.').unwrap_or(suffix);
        Self::ALL
            .iter()
            .copied()
            .find(|e| e.suffixes().iter().any(|s| s.eq_ignore_ascii_case(suffix)))
    }

    /// Renders `text` with this engine.
    ///
    /// # Errors
    ///
    /// What the engine's own function returns.
    pub fn render(self, text: &str, data: &Value, options: &Options<'_>) -> Result<String, Error> {
        match self {
            Engine::Raw => render_raw(text, options),
            Engine::Envsubst => render_envsubst(text, data, options),
            Engine::Mustache => render_mustache(text, data, options),
        }
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Engine {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self, Error> {
        Engine::by_name(name).ok_or(Error::UnknownEngine)
    }
}

/// Renders `text` with the engine called `engine`.
///
/// # Errors
///
/// [`Error::UnknownEngine`] when no engine has the name; otherwise what the engine returns.
///
/// # Examples
///
/// ```
/// use tplx_rs::{render, Options};
/// let data = serde_json::json!({"who": "world"});
/// assert_eq!(render("mustache", "hi {{who}}", &data, &Options::new())?, "hi world");
/// # Ok::<(), tplx_rs::Error>(())
/// ```
pub fn render(
    engine: &str,
    text: &str,
    data: &Value,
    options: &Options<'_>,
) -> Result<String, Error> {
    engine.parse::<Engine>()?.render(text, data, options)
}
