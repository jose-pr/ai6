use crate::{envsubst::EnvsubstOptions, mustache::MustacheOptions};
use alloc::{collections::BTreeMap, string::String};
use core::fmt;

/// Bounds on one render. Going over one is [`Error::Limit`](crate::Error::Limit).
///
/// Start from [`Limits::default`] and assign the fields to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Most bytes of output (default 1,000,000).
    pub max_output: usize,
    /// Deepest nesting of sections and partials (default 100). The template itself is the
    /// first level, so a limit of 3 allows two nested sections.
    pub max_depth: usize,
    /// Most section items and includes expanded in one render (default 100,000).
    pub max_iterations: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_output: 1_000_000,
            max_depth: 100,
            max_iterations: 100_000,
        }
    }
}

/// Supplies the text a template names, so the engines do no I/O.
///
/// A Mustache partial is asked for by its name as written; what the resolver does not return
/// renders as nothing.
pub trait Resolver {
    /// The text called `name`, or `None` when there is none.
    fn read(&self, name: &str) -> Option<String>;
}

impl<F: Fn(&str) -> Option<String>> Resolver for F {
    fn read(&self, name: &str) -> Option<String> {
        self(name)
    }
}

impl Resolver for BTreeMap<String, String> {
    fn read(&self, name: &str) -> Option<String> {
        self.get(name).cloned()
    }
}

/// What a render is given besides the text and the data.
#[derive(Clone, Copy)]
pub struct Options<'a> {
    pub(crate) limits: Limits,
    pub(crate) resolver: Option<&'a dyn Resolver>,
    pub(crate) envsubst: EnvsubstOptions,
    pub(crate) mustache: MustacheOptions,
}

impl<'a> Options<'a> {
    /// Default limits, no resolver, GNU envsubst and HTML-escaping Mustache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            limits: Limits::default(),
            resolver: None,
            envsubst: EnvsubstOptions::default(),
            mustache: MustacheOptions::default(),
        }
    }

    /// The bounds of the render.
    #[must_use]
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Where partials come from. Without one, a partial renders as nothing.
    #[must_use]
    pub fn resolver(mut self, resolver: &'a dyn Resolver) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// The envsubst engine's settings.
    #[must_use]
    pub fn envsubst(mut self, settings: EnvsubstOptions) -> Self {
        self.envsubst = settings;
        self
    }

    /// The Mustache engine's settings.
    #[must_use]
    pub fn mustache(mut self, settings: MustacheOptions) -> Self {
        self.mustache = settings;
        self
    }
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Options<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("limits", &self.limits)
            .field("resolver", &self.resolver.map(|_| ".."))
            .field("envsubst", &self.envsubst)
            .field("mustache", &self.mustache)
            .finish()
    }
}
