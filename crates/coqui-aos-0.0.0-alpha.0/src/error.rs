//! The one error type: a refusal by the class the specification's conformance vectors name it.

use core::fmt;

/// What a refusal is, by the class the specification's conformance vectors name it, and this crate's own for what the vectors do not reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Class {
    /// `ambiguous`.
    Ambiguous,
    /// `case-collision`.
    CaseCollision,
    /// `cycle`.
    Cycle,
    /// `datetime-value`.
    DatetimeValue,
    /// `dotenv-syntax`.
    DotenvSyntax,
    /// `duplicate-key`.
    DuplicateKey,
    /// `duplicate-member`.
    DuplicateMember,
    /// `env-path-conflict`.
    EnvPathConflict,
    /// `env-script-output`.
    EnvScriptOutput,
    /// `hook-refused`.
    HookRefused,
    /// `icon-dimensions`.
    IconDimensions,
    /// `icon-format`.
    IconFormat,
    /// `icon-mismatch`.
    IconMismatch,
    /// `icon-too-large`.
    IconTooLarge,
    /// `icon-unreadable`.
    IconUnreadable,
    /// `integer-range`.
    IntegerRange,
    /// `invalid-digest`.
    InvalidDigest,
    /// `invalid-env-file`.
    InvalidEnvFile,
    /// `invalid-label`.
    InvalidLabel,
    /// `invalid-name`.
    InvalidName,
    /// `invalid-option`.
    InvalidOption,
    /// `invalid-pattern`.
    InvalidPattern,
    /// `invalid-reference`.
    InvalidReference,
    /// `invalid-requirements`.
    InvalidRequirements,
    /// `invalid-source`.
    InvalidSource,
    /// `invalid-unicode`.
    InvalidUnicode,
    /// `invalid-utf8`.
    InvalidUtf8,
    /// `io`.
    Io,
    /// `malformed-fence`.
    MalformedFence,
    /// `missing-file`.
    MissingFile,
    /// `no-format-named`.
    NoFormatNamed,
    /// `non-finite-float`.
    NonFiniteFloat,
    /// `non-string-key`.
    NonStringKey,
    /// `non-utf8-name`.
    NonUtf8Name,
    /// `not-a-catalog`.
    NotACatalog,
    /// `not-a-mapping`.
    NotAMapping,
    /// `not-a-skill`.
    NotASkill,
    /// `not-found`.
    NotFound,
    /// `null-value`.
    NullValue,
    /// `parse-error`.
    ParseError,
    /// `partial-lock`.
    PartialLock,
    /// `pin-mismatch`.
    PinMismatch,
    /// `reserved-variable`.
    ReservedVariable,
    /// `secret-placeholder`.
    SecretPlaceholder,
    /// `skill-name-mismatch`.
    SkillNameMismatch,
    /// `skill-unreadable`.
    SkillUnreadable,
    /// `special-file`.
    SpecialFile,
    /// `symlink`.
    Symlink,
    /// `too-complex`.
    TooComplex,
    /// `too-deep`.
    TooDeep,
    /// `too-large`.
    TooLarge,
    /// `two-manifests`.
    TwoManifests,
    /// `undefined-variable`.
    UndefinedVariable,
    /// `unresolved-reference`.
    UnresolvedReference,
    /// `unsupported-format`.
    UnsupportedFormat,
    /// `unterminated-front-matter`.
    UnterminatedFrontMatter,
    /// `yaml-anchor`.
    YamlAnchor,
    /// `yaml-directive`.
    YamlDirective,
    /// `yaml-merge-key`.
    YamlMergeKey,
    /// `yaml-tag`.
    YamlTag,
}

impl Class {
    /// The class as the specification's vectors write it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ambiguous => "ambiguous",
            Self::CaseCollision => "case-collision",
            Self::Cycle => "cycle",
            Self::DatetimeValue => "datetime-value",
            Self::DotenvSyntax => "dotenv-syntax",
            Self::DuplicateKey => "duplicate-key",
            Self::DuplicateMember => "duplicate-member",
            Self::EnvPathConflict => "env-path-conflict",
            Self::EnvScriptOutput => "env-script-output",
            Self::HookRefused => "hook-refused",
            Self::IconDimensions => "icon-dimensions",
            Self::IconFormat => "icon-format",
            Self::IconMismatch => "icon-mismatch",
            Self::IconTooLarge => "icon-too-large",
            Self::IconUnreadable => "icon-unreadable",
            Self::IntegerRange => "integer-range",
            Self::InvalidDigest => "invalid-digest",
            Self::InvalidEnvFile => "invalid-env-file",
            Self::InvalidLabel => "invalid-label",
            Self::InvalidName => "invalid-name",
            Self::InvalidOption => "invalid-option",
            Self::InvalidPattern => "invalid-pattern",
            Self::InvalidReference => "invalid-reference",
            Self::InvalidRequirements => "invalid-requirements",
            Self::InvalidSource => "invalid-source",
            Self::InvalidUnicode => "invalid-unicode",
            Self::InvalidUtf8 => "invalid-utf8",
            Self::Io => "io",
            Self::MalformedFence => "malformed-fence",
            Self::MissingFile => "missing-file",
            Self::NoFormatNamed => "no-format-named",
            Self::NonFiniteFloat => "non-finite-float",
            Self::NonStringKey => "non-string-key",
            Self::NonUtf8Name => "non-utf8-name",
            Self::NotACatalog => "not-a-catalog",
            Self::NotAMapping => "not-a-mapping",
            Self::NotASkill => "not-a-skill",
            Self::NotFound => "not-found",
            Self::NullValue => "null-value",
            Self::ParseError => "parse-error",
            Self::PartialLock => "partial-lock",
            Self::PinMismatch => "pin-mismatch",
            Self::ReservedVariable => "reserved-variable",
            Self::SecretPlaceholder => "secret-placeholder",
            Self::SkillNameMismatch => "skill-name-mismatch",
            Self::SkillUnreadable => "skill-unreadable",
            Self::SpecialFile => "special-file",
            Self::Symlink => "symlink",
            Self::TooComplex => "too-complex",
            Self::TooDeep => "too-deep",
            Self::TooLarge => "too-large",
            Self::TwoManifests => "two-manifests",
            Self::UndefinedVariable => "undefined-variable",
            Self::UnresolvedReference => "unresolved-reference",
            Self::UnsupportedFormat => "unsupported-format",
            Self::UnterminatedFrontMatter => "unterminated-front-matter",
            Self::YamlAnchor => "yaml-anchor",
            Self::YamlDirective => "yaml-directive",
            Self::YamlMergeKey => "yaml-merge-key",
            Self::YamlTag => "yaml-tag",
        }
    }

    /// The class a vector's name stands for.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "ambiguous" => Some(Self::Ambiguous),
            "case-collision" => Some(Self::CaseCollision),
            "cycle" => Some(Self::Cycle),
            "datetime-value" => Some(Self::DatetimeValue),
            "dotenv-syntax" => Some(Self::DotenvSyntax),
            "duplicate-key" => Some(Self::DuplicateKey),
            "duplicate-member" => Some(Self::DuplicateMember),
            "env-path-conflict" => Some(Self::EnvPathConflict),
            "env-script-output" => Some(Self::EnvScriptOutput),
            "hook-refused" => Some(Self::HookRefused),
            "icon-dimensions" => Some(Self::IconDimensions),
            "icon-format" => Some(Self::IconFormat),
            "icon-mismatch" => Some(Self::IconMismatch),
            "icon-too-large" => Some(Self::IconTooLarge),
            "icon-unreadable" => Some(Self::IconUnreadable),
            "integer-range" => Some(Self::IntegerRange),
            "invalid-digest" => Some(Self::InvalidDigest),
            "invalid-env-file" => Some(Self::InvalidEnvFile),
            "invalid-label" => Some(Self::InvalidLabel),
            "invalid-name" => Some(Self::InvalidName),
            "invalid-option" => Some(Self::InvalidOption),
            "invalid-pattern" => Some(Self::InvalidPattern),
            "invalid-reference" => Some(Self::InvalidReference),
            "invalid-requirements" => Some(Self::InvalidRequirements),
            "invalid-source" => Some(Self::InvalidSource),
            "invalid-unicode" => Some(Self::InvalidUnicode),
            "invalid-utf8" => Some(Self::InvalidUtf8),
            "io" => Some(Self::Io),
            "malformed-fence" => Some(Self::MalformedFence),
            "missing-file" => Some(Self::MissingFile),
            "no-format-named" => Some(Self::NoFormatNamed),
            "non-finite-float" => Some(Self::NonFiniteFloat),
            "non-string-key" => Some(Self::NonStringKey),
            "non-utf8-name" => Some(Self::NonUtf8Name),
            "not-a-catalog" => Some(Self::NotACatalog),
            "not-a-mapping" => Some(Self::NotAMapping),
            "not-a-skill" => Some(Self::NotASkill),
            "not-found" => Some(Self::NotFound),
            "null-value" => Some(Self::NullValue),
            "parse-error" => Some(Self::ParseError),
            "partial-lock" => Some(Self::PartialLock),
            "pin-mismatch" => Some(Self::PinMismatch),
            "reserved-variable" => Some(Self::ReservedVariable),
            "secret-placeholder" => Some(Self::SecretPlaceholder),
            "skill-name-mismatch" => Some(Self::SkillNameMismatch),
            "skill-unreadable" => Some(Self::SkillUnreadable),
            "special-file" => Some(Self::SpecialFile),
            "symlink" => Some(Self::Symlink),
            "too-complex" => Some(Self::TooComplex),
            "too-deep" => Some(Self::TooDeep),
            "too-large" => Some(Self::TooLarge),
            "two-manifests" => Some(Self::TwoManifests),
            "undefined-variable" => Some(Self::UndefinedVariable),
            "unresolved-reference" => Some(Self::UnresolvedReference),
            "unsupported-format" => Some(Self::UnsupportedFormat),
            "unterminated-front-matter" => Some(Self::UnterminatedFrontMatter),
            "yaml-anchor" => Some(Self::YamlAnchor),
            "yaml-directive" => Some(Self::YamlDirective),
            "yaml-merge-key" => Some(Self::YamlMergeKey),
            "yaml-tag" => Some(Self::YamlTag),
            _ => None,
        }
    }
}

impl core::fmt::Display for Class {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The longest subject an [`Error`] keeps, in characters, the escapes counted.
const SUBJECT_LIMIT: usize = 256;

/// `subject` with each control, format, line- and paragraph-separator, surrogate, private-use and
/// unassigned character written as `\xNN`, `\uNNNN` or `\UNNNNNNNN` in lowercase hexadecimal, and
/// the whole cut to 256 characters, then `...`: a name from a stranger's document can neither
/// drive a terminal nor fill a log.
#[must_use]
pub(crate) fn safe_subject(subject: &str) -> String {
    use core::fmt::Write as _;
    use unicode_general_category::{GeneralCategory as Category, get_general_category};

    let mut out = String::new();
    for c in subject.chars().take(SUBJECT_LIMIT + 1) {
        let code = u32::from(c);
        let hidden = matches!(
            get_general_category(c),
            Category::Control
                | Category::Format
                | Category::LineSeparator
                | Category::ParagraphSeparator
                | Category::Surrogate
                | Category::PrivateUse
                | Category::Unassigned
        );
        if !hidden {
            out.push(c);
        } else if code < 0x100 {
            let _ = write!(out, "\\x{code:02x}");
        } else if code < 0x1_0000 {
            let _ = write!(out, "\\u{code:04x}");
        } else {
            let _ = write!(out, "\\U{code:08x}");
        }
    }
    if out.chars().count() <= SUBJECT_LIMIT {
        return out;
    }
    let mut cut: String = out.chars().take(SUBJECT_LIMIT).collect();
    cut.push_str("...");
    cut
}

/// A refusal, by the class the specification's conformance vectors name it (`parse-error`,
/// `undefined-variable`, `symlink`, ...), and what it is about: a variable, a path, a format or
/// an overlay, never a value from a document.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Error {
    /// The class.
    pub class: Class,
    /// The variable, path, format name or overlay it is about; empty when there is none to
    /// name.
    pub subject: String,
    /// The one-based line, when the refusal has a position in a document.
    pub line: Option<usize>,
    /// The one-based column, when it has one.
    pub column: Option<usize>,
    /// For a refusal of a limit (`too-large`, `too-deep`, `too-complex`, `icon-too-large`,
    /// `icon-dimensions`): the key of the [`Settings`](crate::Settings) field that was passed,
    /// such as `tree_files`. Stable, unlike `subject`'s wording.
    pub setting: Option<&'static str>,
}

impl Error {
    /// A refusal of `class` about `subject`, kept with each control, format, separator, surrogate,
    /// private-use and unassigned character written as `\xNN`, `\uNNNN` or `\UNNNNNNNN`, and cut to
    /// 256 characters, then `...`.
    #[must_use]
    pub fn new(class: Class, subject: impl Into<String>) -> Self {
        Self {
            class,
            subject: safe_subject(&subject.into()),
            line: None,
            column: None,
            setting: None,
        }
    }

    /// The same refusal, naming the `Settings` field whose limit was passed.
    #[must_use]
    pub fn over(mut self, setting: &'static str) -> Self {
        self.setting = Some(setting);
        self
    }

    /// The same refusal at a position in the document.
    #[must_use]
    pub fn at(mut self, line: usize, column: usize) -> Self {
        self.line = Some(line);
        self.column = Some(column);
        self
    }

    /// The same refusal on a line.
    #[must_use]
    pub fn on_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.class.as_str())?;
        if !self.subject.is_empty() {
            write!(f, ": {}", self.subject)?;
        }
        if let Some(line) = self.line {
            write!(f, " at line {line}")?;
            if let Some(column) = self.column {
                write!(f, " column {column}")?;
            }
        }
        Ok(())
    }
}

impl core::error::Error for Error {}
