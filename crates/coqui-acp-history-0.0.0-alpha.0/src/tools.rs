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
    /// The harness: `claude`, `codex`, `pi` or `agy`.
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
/// of its `shell` and `apply_patch` tools; Pi's are those of its built-in tools (an `edit` of one
/// replacement and a `bash` timeout in seconds are reshaped by [`normalise`] and [`denormalise`]).
/// agy's are names only: its input keys are taken to be the normalised ones until recorded fixtures
/// say otherwise.
const TABLE: &[Row] = &[
    Row {
        tool: Tool::Read,
        natives: &[
            native("claude", "Read", &[("path", "file_path")]),
            native("pi", "read", &[]),
            native("agy", "read_file", &[]),
        ],
    },
    Row {
        tool: Tool::Write,
        natives: &[
            native("claude", "Write", &[("path", "file_path")]),
            native("pi", "write", &[]),
        ],
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
            native("pi", "edit", &[]),
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
            native("pi", "bash", &[]),
            native("agy", "run_command", &[]),
        ],
    },
    Row {
        tool: Tool::Grep,
        natives: &[
            native("claude", "Grep", &[]),
            native("pi", "grep", &[("-i", "ignoreCase")]),
        ],
    },
    Row {
        tool: Tool::Glob,
        natives: &[native("claude", "Glob", &[]), native("pi", "find", &[])],
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
    Some((tool, pi_shape(harness, tool, renamed, false)?))
}

/// Renames the keys of a normalised `input` to `harness`'s, and gives the native name to call.
/// `None` when `harness` has no such tool, the input is not an object, or two of its keys would
/// become the same key.
#[must_use]
pub fn denormalise(tool: Tool, harness: &str, input: &Value) -> Option<(&'static str, Value)> {
    let native = tool.native(harness)?;
    let renamed = pi_shape(harness, tool, rename(input, native.keys, true)?, true)?;
    Some((native.name, renamed))
}

/// The two changes of shape Pi's tools need, which a rename of keys cannot make: `edit` takes a list of
/// replacements where the normalised form has one `old` and `new`, and `bash` takes its timeout in seconds
/// where the normalised form has `timeout_ms`. `None` for an `edit` of several replacements (or a normalised
/// one that replaces every match), which has no one-call equivalent.
fn pi_shape(harness: &str, tool: Tool, input: Value, to_native: bool) -> Option<Value> {
    if harness != "pi" {
        return Some(input);
    }
    let Value::Object(mut map) = input else {
        return None;
    };
    match (tool, to_native) {
        (Tool::Edit, false) => {
            let edits = map.remove("edits")?;
            let [edit] = edits.as_array()?.as_slice() else {
                return None;
            };
            let (old, new) = (edit.get("oldText")?.clone(), edit.get("newText")?.clone());
            if map.insert("old".to_owned(), old).is_some()
                || map.insert("new".to_owned(), new).is_some()
            {
                return None;
            }
        }
        (Tool::Edit, true) => {
            if map
                .remove("replace_all")
                .is_some_and(|all| all != Value::Bool(false))
            {
                return None;
            }
            let (old, new) = (map.remove("old")?, map.remove("new")?);
            let edit = serde_json::json!({"oldText": old, "newText": new});
            if map
                .insert("edits".to_owned(), Value::Array(vec![edit]))
                .is_some()
            {
                return None;
            }
        }
        (Tool::Shell, false) => {
            if let Some(seconds) = map.remove("timeout") {
                let millis = match seconds.as_u64() {
                    Some(whole) => Value::from(whole.checked_mul(1000)?),
                    None => Value::from(seconds.as_f64()? * 1000.0),
                };
                if map.insert("timeout_ms".to_owned(), millis).is_some() {
                    return None;
                }
            }
        }
        (Tool::Shell, true) => {
            if let Some(millis) = map.remove("timeout_ms") {
                let seconds = match millis.as_u64() {
                    Some(whole) => Value::from(whole.div_ceil(1000)),
                    None => Value::from(millis.as_f64()? / 1000.0),
                };
                if map.insert("timeout".to_owned(), seconds).is_some() {
                    return None;
                }
            }
        }
        _ => {}
    }
    Some(Value::Object(map))
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
