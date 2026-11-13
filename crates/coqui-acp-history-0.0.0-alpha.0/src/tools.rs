//! The normalised tool table: each built-in tool of a harness named once, whatever it is called
//! natively.
//!
//! A tool call in a history is ACP's: a `kind`, a `rawInput` and a `rawOutput`. The table says
//! which [`Tool`] a native name is and how its input keys are spelled in the normalised form, so a
//! codec writing to another harness can pick that harness's tool and rename the keys. It renames
//! keys only; a change of shape (Codex's `command` as an argument vector, Claude's as a string) is
//! the codec's.

use agent_client_protocol::schema::v1::ToolKind;
use serde_json::{Map, Value};

/// A tool in the normalised vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Tool {
    /// Reads a file or a range of one.
    Read,
    /// Writes a whole file.
    Write,
    /// Replaces text in a file.
    Edit,
    /// Applies a patch in the `apply_patch` format.
    Patch,
    /// Runs a command line.
    Shell,
    /// Searches file contents.
    Grep,
    /// Finds files by name pattern.
    Glob,
    /// Fetches a URL.
    Fetch,
}

impl Tool {
    /// Every tool of the table.
    pub const ALL: [Tool; 8] = [
        Tool::Read,
        Tool::Write,
        Tool::Edit,
        Tool::Patch,
        Tool::Shell,
        Tool::Grep,
        Tool::Glob,
        Tool::Fetch,
    ];

    /// The tool's name in the table and in vectors.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Edit => "edit",
            Self::Patch => "patch",
            Self::Shell => "shell",
            Self::Grep => "grep",
            Self::Glob => "glob",
            Self::Fetch => "fetch",
        }
    }

    /// The ACP `kind` a call of this tool has.
    #[must_use]
    pub fn kind(self) -> ToolKind {
        match self {
            Self::Read => ToolKind::Read,
            Self::Write | Self::Edit | Self::Patch => ToolKind::Edit,
            Self::Shell => ToolKind::Execute,
            Self::Grep | Self::Glob => ToolKind::Search,
            Self::Fetch => ToolKind::Fetch,
        }
    }

    /// The tool a table name (`read`, `shell`) names.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.name() == name)
    }

    /// The tool's native spellings, in the order a codec prefers them.
    #[must_use]
    pub fn natives(self) -> &'static [Native] {
        TABLE
            .iter()
            .find(|row| row.tool == self)
            .map_or(&[], |row| row.natives)
    }

    /// The tool's native spelling on `harness`, the first one when it has several.
    #[must_use]
    pub fn native(self, harness: &str) -> Option<&'static Native> {
        self.natives()
            .iter()
            .find(|native| native.harness == harness)
    }

    /// The tool `harness` calls `name`.
    #[must_use]
    pub fn from_native(harness: &str, name: &str) -> Option<Self> {
        TABLE
            .iter()
            .find(|row| {
                row.natives
                    .iter()
                    .any(|n| n.harness == harness && n.name == name)
            })
            .map(|row| row.tool)
    }
}

/// One harness's spelling of a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Native {
    /// The harness: `claude`, `codex` or `agy`.
    pub harness: &'static str,
    /// The tool's name there.
    pub name: &'static str,
    /// The input keys that are spelled differently: `(normalised, native)`. A key not listed is
    /// spelled the same on both sides.
    pub keys: &'static [(&'static str, &'static str)],
}

struct Row {
    tool: Tool,
    natives: &'static [Native],
}

const fn native(
    harness: &'static str,
    name: &'static str,
    keys: &'static [(&'static str, &'static str)],
) -> Native {
    Native {
        harness,
        name,
        keys,
    }
}

/// The table. Claude's names and keys are those of Claude Code's tool schemas; Codex's are those
/// of its `shell` and `apply_patch` tools. agy's are names only: its input keys are taken to be
/// the normalised ones until recorded fixtures say otherwise.
const TABLE: &[Row] = &[
    Row {
        tool: Tool::Read,
        natives: &[
            native("claude", "Read", &[("path", "file_path")]),
            native("agy", "read_file", &[]),
        ],
    },
    Row {
        tool: Tool::Write,
        natives: &[native("claude", "Write", &[("path", "file_path")])],
    },
    Row {
        tool: Tool::Edit,
        natives: &[
            native(
                "claude",
                "Edit",
                &[
                    ("path", "file_path"),
                    ("old", "old_string"),
                    ("new", "new_string"),
                ],
            ),
            native("agy", "edit_file", &[]),
        ],
    },
    Row {
        tool: Tool::Patch,
        natives: &[native("codex", "apply_patch", &[("patch", "input")])],
    },
    Row {
        tool: Tool::Shell,
        natives: &[
            native("claude", "Bash", &[("timeout_ms", "timeout")]),
            native("codex", "shell", &[("cwd", "workdir")]),
            native("codex", "shell_command", &[("cwd", "workdir")]),
            native("agy", "run_command", &[]),
        ],
    },
    Row {
        tool: Tool::Grep,
        natives: &[native("claude", "Grep", &[])],
    },
    Row {
        tool: Tool::Glob,
        natives: &[native("claude", "Glob", &[])],
    },
    Row {
        tool: Tool::Fetch,
        natives: &[native("claude", "WebFetch", &[])],
    },
];

/// Renames the keys of a native `input` to the normalised ones, for the tool `harness` calls
/// `name`. `None` when the table does not know the tool, the input is not an object, or two of its
/// keys would become the same key (neither value may win).
#[must_use]
pub fn normalise(harness: &str, name: &str, input: &Value) -> Option<(Tool, Value)> {
    let tool = Tool::from_native(harness, name)?;
    let native = tool
        .natives()
        .iter()
        .find(|n| n.harness == harness && n.name == name)?;
    let renamed = rename(input, native.keys, false)?;
    Some((tool, renamed))
}

/// Renames the keys of a normalised `input` to `harness`'s, and gives the native name to call.
/// `None` when `harness` has no such tool, the input is not an object, or two of its keys would
/// become the same key.
#[must_use]
pub fn denormalise(tool: Tool, harness: &str, input: &Value) -> Option<(&'static str, Value)> {
    let native = tool.native(harness)?;
    let renamed = rename(input, native.keys, true)?;
    Some((native.name, renamed))
}

/// Renames the keys of `input` by `keys`, `(normalised, native)` pairs, in the direction given.
/// `None` for an input that is not an object or whose renamed keys collide.
fn rename(input: &Value, keys: &[(&str, &str)], to_native: bool) -> Option<Value> {
    let Value::Object(map) = input else {
        return None;
    };
    let mut out = Map::new();
    for (key, value) in map {
        let renamed = keys
            .iter()
            .find(|(normal, native)| key == if to_native { normal } else { native })
            .map(|(normal, native)| if to_native { *native } else { *normal });
        if out
            .insert(renamed.unwrap_or(key.as_str()).to_owned(), value.clone())
            .is_some()
        {
            return None;
        }
    }
    Some(Value::Object(out))
}
