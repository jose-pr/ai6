//! An overlay's knowledge served through MCP (`spec/v0/knowledge.md`): what is served, the tools
//! `kb_list`, `kb_read` and `kb_search`, the argument checks, the lines and the written-out text, a
//! template rendered per call, and the checks on a search answer.
//!
//! The handlers are transport-agnostic: a [`ToolCall`] goes in and a [`ToolResult`] comes out, so a
//! consumer serves them over its own MCP. Nothing here reads a file; a [`KnowledgeTree`] is built
//! from entries or from a [`FileSystem`](crate::FileSystem) the caller supplies.

mod plan;
mod read;
mod search;
mod serve;
mod serve_text;
mod settings;
mod text;
mod tools;
mod tree;

pub use plan::{KnowledgePlan, knowledge_plan};
pub use read::CallerFacts;
pub use search::{
    Passage, SearchAnswer, SearchBackend, SearchFailure, SearchMode, SearchRequest, search_check,
    search_order,
};
pub use serve::{
    KnowledgeSession, ReadRequest, ToolCall, ToolResult, kb_list, kb_read, kb_tool_result,
    knowledge_call,
};
pub use serve_text::{Renderers, served_texts};
pub use settings::{KnowledgeSettings, MAX_INTEGER};
pub use text::{inert, knowledge_lines};
pub use tools::{ToolName, knowledge_arguments, knowledge_tools};
pub use tree::{
    Engine, FileFormat, KnowledgeEntry, KnowledgeOverlay, KnowledgeSet, KnowledgeTree, ServedFile,
    Skip, SkipReason,
};

use alloc::string::String;
use core::fmt;

/// The class of a refused call, as the specification names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorClass {
    /// The arguments are not as the tool's schema and the rules say.
    InvalidArguments,
    /// A `path` is not a relative path, or holds a character served text writes out.
    InvalidPath,
    /// An `overlay` the session does not have.
    UnknownOverlay,
    /// More than one overlay has the path, and none was named.
    Ambiguous,
    /// No file of the knowledge is at the path.
    NotFound,
    /// The path, or a directory on the way, is a link.
    Link,
    /// The file's bytes are not UTF-8.
    NotText,
    /// The file or its rendered text is past its bound.
    TooLarge,
    /// An `@path` that names a served file and cannot be included.
    IncludeError,
    /// A configured option is not of its declared type.
    InvalidOption,
    /// A Mustache body the subset refuses.
    TemplateError,
    /// `to` before `from`, or `from` past the last line.
    LineRange,
    /// In a search entry: no answer now.
    Unavailable,
    /// In a search entry: not by that mode.
    ModeUnavailable,
}

impl ErrorClass {
    /// The class a name writes.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        [
            Self::InvalidArguments,
            Self::InvalidPath,
            Self::UnknownOverlay,
            Self::Ambiguous,
            Self::NotFound,
            Self::Link,
            Self::NotText,
            Self::TooLarge,
            Self::IncludeError,
            Self::InvalidOption,
            Self::TemplateError,
            Self::LineRange,
            Self::Unavailable,
            Self::ModeUnavailable,
        ]
        .into_iter()
        .find(|class| class.as_str() == name)
    }

    /// The class as the vectors and results write it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid-arguments",
            Self::InvalidPath => "invalid-path",
            Self::UnknownOverlay => "unknown-overlay",
            Self::Ambiguous => "ambiguous",
            Self::NotFound => "not-found",
            Self::Link => "link",
            Self::NotText => "not-text",
            Self::TooLarge => "too-large",
            Self::IncludeError => "include-error",
            Self::InvalidOption => "invalid-option",
            Self::TemplateError => "template-error",
            Self::LineRange => "line-range",
            Self::Unavailable => "unavailable",
            Self::ModeUnavailable => "mode-unavailable",
        }
    }
}

impl fmt::Display for ErrorClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A refused call: its class and a message for a person, never parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct KnowledgeError {
    /// The class.
    pub class: ErrorClass,
    /// What happened, as text for a person.
    pub message: String,
}

impl KnowledgeError {
    /// An error of `class` with `message`.
    #[must_use]
    pub fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }
}

impl fmt::Display for KnowledgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.class, self.message)
    }
}

impl core::error::Error for KnowledgeError {}
