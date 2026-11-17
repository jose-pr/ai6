use thiserror::Error;

/// Why a render failed.
///
/// Messages never quote the template or the data.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Error {
    /// No engine has the requested name.
    #[error("unknown template engine")]
    UnknownEngine,
    /// The template uses syntax the engine does not define, or is malformed.
    #[error("template refused at line {line}, column {column}: {kind}")]
    Syntax {
        /// 1-based line.
        line: usize,
        /// 1-based column, in characters.
        column: usize,
        /// What was refused.
        kind: SyntaxKind,
    },
    /// A name has no value and the envsubst mode is [`Undefined::Strict`](crate::Undefined).
    #[error("a name has no value at line {line}, column {column}")]
    Undefined {
        /// 1-based line.
        line: usize,
        /// 1-based column, in characters.
        column: usize,
    },
    /// A sandbox limit was reached.
    #[error("limit exceeded: {0}")]
    Limit(Limit),
}

/// What the Mustache parser refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum SyntaxKind {
    /// A tag with no closing delimiter.
    #[error("a tag is not closed")]
    UnclosedTag,
    /// An empty name.
    #[error("a name is not usable")]
    InvalidName,
    /// An inheritance block (`{{<`, `{{$`) or another sigil Mustache does not define here.
    #[error("a tag of this kind is not defined")]
    UnsupportedTag,
    /// A delimiter change that does not give two delimiters.
    #[error("the delimiters are not usable")]
    InvalidDelimiters,
    /// A section with no closing tag.
    #[error("a section is not closed")]
    UnclosedSection,
    /// A closing tag with no open section.
    #[error("a closing tag has no section")]
    StrayClose,
    /// A closing tag that names another section.
    #[error("a section is closed by another name")]
    MismatchedClose,
}

/// The sandbox limit that was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Limit {
    /// The output grew past [`Limits::max_output`](crate::Limits).
    #[error("output size")]
    Output,
    /// Sections, partials or includes nested past [`Limits::max_depth`](crate::Limits).
    #[error("nesting depth")]
    Depth,
    /// Section items or includes past [`Limits::max_iterations`](crate::Limits).
    #[error("iterations")]
    Iterations,
}
