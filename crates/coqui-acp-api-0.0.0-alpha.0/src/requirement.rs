//! [`Requirement`]: one thing a connection is held to, and the names with a fixed meaning.

use alloc::borrow::ToOwned;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use guatiao::library::ProviderError;
use guatiao::{FromValue, Schema, Status, Tag, ToValue, Value};

use crate::roots::is_plain_absolute;

/// No file is created, changed or removed by the agent or by anything it runs. Commands may still
/// run, where the file system is read-only to them; [`NO_COMMANDS`] takes them away too. No value.
pub const READ_ONLY: &str = "read_only";

/// No command or program is started by the agent, in a sandbox or not: no shell, no script, no
/// terminal. Files may still be written. No value.
pub const NO_COMMANDS: &str = "no_commands";

/// The agent only answers from what the host gives it: it calls no tool of any kind (no file read
/// or write, no command, no MCP tool from any source, no web search or fetch); loads no settings,
/// MCP servers, skills, hooks or instruction files, neither the session directories' nor the
/// user's own global ones, so the model sees only the prompt and what the host sends; and whatever
/// the harness runs to answer runs sandboxed, writing no file and reaching no network but the
/// harness's own model endpoint. Holds everything the other names without a value hold. No value.
pub const ANSWER_ONLY: &str = "answer_only";

/// No configuration is read from a session's directories or their parents: settings, permission
/// rules, hooks, commands and sub-agents declared there are ignored. Instruction files
/// (`AGENTS.md`, `CLAUDE.md`) are context, not configuration, and still load. No value.
pub const NO_PROJECT_SETTINGS: &str = "no_project_settings";

/// No MCP server declared in a session's directories or their parents starts; the servers the
/// client names in ACP and the provider's own configuration still do. No value.
pub const NO_PROJECT_MCP: &str = "no_project_mcp";

/// Files are created, changed or removed only under these directories. The value: a list of
/// absolute paths with no `.` or `..` part; an empty list allows no file to be written.
pub const FILES_UNDER: &str = "files_under";

/// No file under these directories is created, changed or removed, wherever else writing is
/// allowed. The value: a list of absolute paths with no `.` or `..` part.
pub const READ_ONLY_PATHS: &str = "read_only_paths";

/// No file under these directories is read, listed, created, changed or removed. The value: a list
/// of absolute paths with no `.` or `..` part.
pub const DENIED_PATHS: &str = "denied_paths";

/// The agent and what it runs reach only these hosts, besides the harness's own model endpoint.
/// The value: a list of `host`, `host:port` or `*.domain` entries (an IPv6 address in brackets);
/// an empty list reaches nothing else.
pub const NETWORK: &str = "network";

/// The only commands that run are these interpreters, inside the harness's own sandbox; no other
/// command runs. The value: a list of program names (`python3`, `node`).
pub const SANDBOXED_SCRIPTS: &str = "sandboxed_scripts";

/// At most these skills load, and no other; an empty list loads none. The value: a list of skill
/// names.
pub const ONLY_SKILLS: &str = "only_skills";

/// Every name with a fixed meaning, in the order this crate declares them.
pub const NAMES: [&str; 11] = [
    READ_ONLY,
    NO_COMMANDS,
    ANSWER_ONLY,
    NO_PROJECT_SETTINGS,
    NO_PROJECT_MCP,
    FILES_UNDER,
    READ_ONLY_PATHS,
    DENIED_PATHS,
    NETWORK,
    SANDBOXED_SCRIPTS,
    ONLY_SKILLS,
];

/// One thing a host holds a connection to. Each narrows what the harness may do; none widens it,
/// and a harness never reads one as permission.
///
/// [`name`](Self::name) is an open vocabulary: [`NAMES`] are the ones with a fixed meaning, and a
/// harness refuses a name it does not know. A key this type does not declare is refused when read,
/// so a narrowing a newer host adds can never be dropped by an older harness.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct Requirement {
    /// What is required: one of [`NAMES`], or a name a harness may not know.
    pub name: String,
    /// What it is held to, for a name that takes a value; absent for one that takes none.
    pub value: Option<Value>,
}

impl Requirement {
    /// The requirement `name`, with no value.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: None,
        }
    }

    /// The requirement `name`, held to `value`.
    #[must_use]
    pub fn with_value(name: impl Into<String>, value: impl Into<Value>) -> Self {
        Self {
            name: name.into(),
            value: Some(value.into()),
        }
    }

    /// What it narrows, when its name is one of [`NAMES`] and its value is what that name takes.
    ///
    /// # Errors
    ///
    /// [`RequirementError::Unknown`] for another name, [`RequirementError::BadValue`] for a value
    /// the name does not take, a missing one included.
    pub fn narrowing(&self) -> Result<Narrowing, RequirementError> {
        Narrowing::read(self)
    }
}

impl From<Narrowing> for Requirement {
    fn from(narrowing: Narrowing) -> Self {
        let list = |items: Vec<String>| {
            let mut list = guatiao::List::new();
            for item in items {
                list.push(item.as_str());
            }
            Value::from(list)
        };
        let (name, value) = match narrowing {
            Narrowing::ReadOnly => (READ_ONLY, None),
            Narrowing::NoCommands => (NO_COMMANDS, None),
            Narrowing::AnswerOnly => (ANSWER_ONLY, None),
            Narrowing::NoProjectSettings => (NO_PROJECT_SETTINGS, None),
            Narrowing::NoProjectMcp => (NO_PROJECT_MCP, None),
            Narrowing::FilesUnder(paths) => (FILES_UNDER, Some(list(paths))),
            Narrowing::ReadOnlyPaths(paths) => (READ_ONLY_PATHS, Some(list(paths))),
            Narrowing::DeniedPaths(paths) => (DENIED_PATHS, Some(list(paths))),
            Narrowing::Network(hosts) => (NETWORK, Some(list(hosts))),
            Narrowing::SandboxedScripts(programs) => (SANDBOXED_SCRIPTS, Some(list(programs))),
            Narrowing::OnlySkills(skills) => (ONLY_SKILLS, Some(list(skills))),
        };
        Self {
            name: name.to_owned(),
            value,
        }
    }
}

/// A [`Requirement`] whose name has a fixed meaning, with its value read. Each variant is the name
/// of the same spelling's constant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Narrowing {
    /// [`READ_ONLY`].
    ReadOnly,
    /// [`NO_COMMANDS`].
    NoCommands,
    /// [`ANSWER_ONLY`].
    AnswerOnly,
    /// [`NO_PROJECT_SETTINGS`].
    NoProjectSettings,
    /// [`NO_PROJECT_MCP`].
    NoProjectMcp,
    /// [`FILES_UNDER`], with its directories.
    FilesUnder(Vec<String>),
    /// [`READ_ONLY_PATHS`], with its directories.
    ReadOnlyPaths(Vec<String>),
    /// [`DENIED_PATHS`], with its directories.
    DeniedPaths(Vec<String>),
    /// [`NETWORK`], with the hosts it allows.
    Network(Vec<String>),
    /// [`SANDBOXED_SCRIPTS`], with its interpreters.
    SandboxedScripts(Vec<String>),
    /// [`ONLY_SKILLS`], with its skills.
    OnlySkills(Vec<String>),
}

impl Narrowing {
    /// Reads `requirement`.
    ///
    /// # Errors
    ///
    /// As [`Requirement::narrowing`].
    pub fn read(requirement: &Requirement) -> Result<Self, RequirementError> {
        let name = requirement.name.as_str();
        let bad = |why: &str| RequirementError::BadValue {
            name: name.to_owned(),
            why: why.to_owned(),
        };
        let none = |narrowing: Self| match &requirement.value {
            None => Ok(narrowing),
            Some(value) if is_nothing(value) => Ok(narrowing),
            Some(_) => Err(bad("this requirement takes no value")),
        };
        let texts = |check: fn(&str) -> Result<(), &'static str>| {
            let value = requirement
                .value
                .as_ref()
                .filter(|value| !is_nothing(value))
                .ok_or_else(|| bad("this requirement takes a list of texts"))?;
            let items = Vec::<String>::from_value(value)
                .map_err(|_| bad("this requirement takes a list of texts"))?;
            for item in &items {
                check(item).map_err(bad)?;
            }
            Ok(items)
        };
        match name {
            READ_ONLY => none(Self::ReadOnly),
            NO_COMMANDS => none(Self::NoCommands),
            ANSWER_ONLY => none(Self::AnswerOnly),
            NO_PROJECT_SETTINGS => none(Self::NoProjectSettings),
            NO_PROJECT_MCP => none(Self::NoProjectMcp),
            FILES_UNDER => texts(path).map(Self::FilesUnder),
            READ_ONLY_PATHS => texts(path).map(Self::ReadOnlyPaths),
            DENIED_PATHS => texts(path).map(Self::DeniedPaths),
            NETWORK => texts(host).map(Self::Network),
            SANDBOXED_SCRIPTS => texts(word).map(Self::SandboxedScripts),
            ONLY_SKILLS => texts(word).map(Self::OnlySkills),
            _ => Err(RequirementError::Unknown {
                name: name.to_owned(),
            }),
        }
    }

    /// Reads every requirement of a request, refusing one whose name comes twice.
    ///
    /// # Errors
    ///
    /// The first requirement that [`read`](Self::read) refuses, or
    /// [`RequirementError::Repeated`].
    pub fn read_all(requirements: &[Requirement]) -> Result<Vec<Self>, RequirementError> {
        let mut seen = BTreeSet::new();
        let mut read = Vec::with_capacity(requirements.len());
        for requirement in requirements {
            if !seen.insert(requirement.name.as_str()) {
                return Err(RequirementError::Repeated {
                    name: requirement.name.clone(),
                });
            }
            read.push(Self::read(requirement)?);
        }
        Ok(read)
    }
}

/// Null, or no value at all: what a name without a value takes.
fn is_nothing(value: &Value) -> bool {
    matches!(value.tag(), Ok(Tag::GUATIAO_NULL | Tag::GUATIAO_ABSENT))
}

fn path(text: &str) -> Result<(), &'static str> {
    if is_plain_absolute(text) {
        Ok(())
    } else {
        Err("each path is absolute, with no `.` or `..` part")
    }
}

fn host(text: &str) -> Result<(), &'static str> {
    let fine = !text.is_empty()
        && !text.contains("://")
        && !text.contains('/')
        && !text.chars().any(|c| c.is_whitespace() || c.is_control());
    if fine {
        Ok(())
    } else {
        Err("each entry is `host`, `host:port` or `*.domain`")
    }
}

fn word(text: &str) -> Result<(), &'static str> {
    if text.is_empty() || text.chars().any(char::is_control) {
        Err("each entry is a name, not empty")
    } else {
        Ok(())
    }
}

/// Why a [`Requirement`] could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequirementError {
    /// The name has no meaning here. A harness refuses it as one it cannot hold.
    Unknown {
        /// The name.
        name: String,
    },
    /// The value is not what the name takes.
    BadValue {
        /// The name.
        name: String,
        /// What it takes.
        why: String,
    },
    /// The name came twice in one request.
    Repeated {
        /// The name.
        name: String,
    },
}

impl RequirementError {
    /// The status a harness refuses the request with: `GUATIAO_ERR_UNSUPPORTED` for an unknown
    /// name, `GUATIAO_ERR_BAD_VALUE` for the rest.
    #[must_use]
    pub const fn status(&self) -> Status {
        match self {
            Self::Unknown { .. } => Status::GUATIAO_ERR_UNSUPPORTED,
            Self::BadValue { .. } | Self::Repeated { .. } => Status::GUATIAO_ERR_BAD_VALUE,
        }
    }
}

impl fmt::Display for RequirementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { name } => {
                write!(f, "requirement `{name}` is not one this harness knows")
            }
            Self::BadValue { name, why } => write!(f, "requirement `{name}`: {why}"),
            Self::Repeated { name } => write!(f, "requirement `{name}` is named twice"),
        }
    }
}

impl core::error::Error for RequirementError {}

impl From<RequirementError> for ProviderError {
    fn from(error: RequirementError) -> Self {
        Self::new(error.status(), &format!("{error}"))
    }
}
