#![doc = include_str!("../README.md")]
#![no_std]
#![warn(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]
#![cfg_attr(docsrs, feature(doc_cfg))]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod engine;
mod envsubst;
mod error;
mod mustache;
mod options;
mod sink;

pub use engine::{render, Engine};
pub use envsubst::{
    render_envsubst, Dialect, EnvsubstOptions, Feature, Features, Pattern, Undefined,
};
pub use error::{Error, Limit, SyntaxKind};
pub use mustache::{render_mustache, Escape, MustacheOptions};
pub use options::{Limits, Options, Resolver};
pub use serde_json::Value;

/// Returns `text` unchanged: the engine of a file that is not a template.
///
/// The text is still held to [`Limits::max_output`].
///
/// # Errors
///
/// [`Error::Limit`] when `text` is longer than the output limit.
pub fn render_raw(text: &str, options: &Options<'_>) -> Result<alloc::string::String, Error> {
    let mut sink = sink::Sink::new(options.limits.max_output);
    sink.push(text)?;
    Ok(sink.finish())
}
