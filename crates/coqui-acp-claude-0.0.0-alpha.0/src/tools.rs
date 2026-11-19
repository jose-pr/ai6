//! Claude Code's tools as titles, kinds, locations and content, and their results.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use coqui_acp_harness::event::{
    Diff, FileChange, Location, PlanEntry, PlanStatus, ToolCall, ToolContent, ToolKind, ToolStatus,
};
use serde_json::Value;

/// The tools whose calls run a command in a terminal of Claude Code's own.
pub(crate) const TERMINAL_TOOLS: [&str; 2] = ["Bash", "PowerShell"];

fn text<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input.get(key).and_then(Value::as_str)
}

pub(crate) fn cut(text: &str, max_title: usize) -> String {
    let first = text.lines().next().unwrap_or_default();
    let cut = coqui_acp_harness::lines::cut(first, max_title);
    if cut.len() < text.len() {
        format!("{cut}…")
    } else {
        cut.to_string()
    }
}

/// What a tool call of `name` with `input` is: its kind, title, locations and content. `input` is the model's,
/// untrusted, and only read.
pub(crate) fn describe(id: &str, name: &str, input: &Value, max_title: usize) -> ToolCall {
    let mut call = ToolCall::new(id);
    call.name = Some(name.to_string());
    call.input = Some(input.clone());
    let path = text(input, "file_path").or_else(|| text(input, "notebook_path"));
    let (kind, title) = match name {
        "Read" => (
            ToolKind::Read,
            path.map_or_else(|| "Read".to_string(), |path| format!("Read {path}")),
        ),
        "Edit" | "MultiEdit" | "NotebookEdit" => (
            ToolKind::Edit,
            path.map_or_else(|| "Edit".to_string(), |path| format!("Edit {path}")),
        ),
        "Write" => (
            ToolKind::Edit,
            path.map_or_else(|| "Write".to_string(), |path| format!("Write {path}")),
        ),
        "Bash" | "PowerShell" => (
            ToolKind::Execute,
            text(input, "command").map_or_else(|| name.to_string(), |text| cut(text, max_title)),
        ),
        "Glob" => (
            ToolKind::Search,
            text(input, "pattern").map_or_else(
                || "Find files".to_string(),
                |pattern| format!("Find {}", cut(pattern, max_title)),
            ),
        ),
        "Grep" => (
            ToolKind::Search,
            text(input, "pattern").map_or_else(
                || "Search".to_string(),
                |pattern| format!("Search {}", cut(pattern, max_title)),
            ),
        ),
        "WebFetch" => (
            ToolKind::Fetch,
            text(input, "url").map_or_else(
                || "Fetch".to_string(),
                |url| format!("Fetch {}", cut(url, max_title)),
            ),
        ),
        "WebSearch" => (
            ToolKind::Fetch,
            text(input, "query").map_or_else(
                || "Search the web".to_string(),
                |query| format!("Search the web for {}", cut(query, max_title)),
            ),
        ),
        "Task" | "Agent" => (
            ToolKind::Think,
            text(input, "description")
                .map_or_else(|| "Run a subagent".to_string(), |text| cut(text, max_title)),
        ),
        "EnterPlanMode" => (ToolKind::SwitchMode, "Plan".to_string()),
        "ExitPlanMode" => (ToolKind::SwitchMode, "Ready to code?".to_string()),
        "Skill" => (
            ToolKind::Other,
            text(input, "skill").map_or_else(
                || "Skill".to_string(),
                |skill| format!("Skill {}", cut(skill, max_title)),
            ),
        ),
        other => match other
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"))
        {
            Some((server, tool)) => (ToolKind::Other, format!("{server}: {tool}")),
            None => (ToolKind::Other, other.to_string()),
        },
    };
    call.kind = Some(kind);
    call.title = Some(title);
    call.status = Some(ToolStatus::Pending);
    let line = input
        .get("offset")
        .and_then(Value::as_u64)
        .filter(|line| *line > 0);
    call.locations = Some(match path.or_else(|| text(input, "path")) {
        Some(path) => vec![Location::new(
            path,
            if name == "Read" { line } else { None },
        )],
        None => Vec::new(),
    });
    call.content = Some(match name {
        "Edit" => path
            .map(|path| {
                let mut diff = Diff::new(path, FileChange::Modify);
                diff.old_text = text(input, "old_string").map(str::to_string);
                diff.new_text = text(input, "new_string").map(str::to_string);
                vec![ToolContent::Diff(diff)]
            })
            .unwrap_or_default(),
        "Write" => path
            .map(|path| {
                let mut diff = Diff::new(path, FileChange::Add);
                diff.new_text = text(input, "content").map(str::to_string);
                vec![ToolContent::Diff(diff)]
            })
            .unwrap_or_default(),
        name if TERMINAL_TOOLS.contains(&name) => vec![ToolContent::Terminal(id.to_string())],
        _ => Vec::new(),
    });
    call
}

/// The plan a `TodoWrite` call writes, whole.
pub(crate) fn todo_plan(input: &Value) -> Option<Vec<PlanEntry>> {
    let todos = input.get("todos")?.as_array()?;
    Some(
        todos
            .iter()
            .filter_map(|todo| {
                Some(PlanEntry::new(
                    text(todo, "content")?,
                    plan_status(text(todo, "status")?)?,
                ))
            })
            .collect(),
    )
}

pub(crate) fn plan_status(status: &str) -> Option<PlanStatus> {
    Some(match status {
        "pending" => PlanStatus::Pending,
        "in_progress" => PlanStatus::InProgress,
        "completed" => PlanStatus::Completed,
        _ => return None,
    })
}

/// The text of a `tool_result`'s content: a string, or the text of its blocks.
pub(crate) fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The diff a finished `Edit` or `Write` reports in its `tool_use_result`, with the patch made from its hunks, and the
/// lines it changed.
pub(crate) fn finished_diff(name: &str, result: &Value) -> Option<(Diff, Vec<Location>)> {
    if !matches!(name, "Edit" | "Write" | "MultiEdit") {
        return None;
    }
    let path = text(result, "filePath")?;
    let created = text(result, "type") == Some("create")
        || (name == "Write" && result.get("originalFile").is_none_or(Value::is_null));
    let change = if created {
        FileChange::Add
    } else {
        FileChange::Modify
    };
    let mut diff = Diff::new(path, change);
    diff.old_text = text(result, "oldString")
        .or_else(|| text(result, "originalFile"))
        .map(str::to_string);
    diff.new_text = text(result, "newString")
        .or_else(|| text(result, "content"))
        .map(str::to_string);
    let hunks = result.get("structuredPatch").and_then(Value::as_array);
    let mut locations = Vec::new();
    if let Some(hunks) = hunks.filter(|hunks| !hunks.is_empty()) {
        diff.patch = Some(git_patch(path, change, hunks));
        for hunk in hunks {
            locations.push(Location::new(
                path,
                hunk.get("newStart").and_then(Value::as_u64),
            ));
        }
    } else {
        locations.push(Location::new(path, None));
    }
    Some((diff, locations))
}

/// A git patch of one file from Claude Code's hunks (`oldStart`, `oldLines`, `newStart`, `newLines`, `lines`). The
/// path keeps its own spelling with forward slashes, and is quoted as git quotes a path that needs it.
pub(crate) fn git_patch(path: &str, change: FileChange, hunks: &[Value]) -> String {
    let path = path.replace('\\', "/");
    let name = quote(&path);
    let mut patch = format!("diff --git {name} {name}\n");
    match change {
        FileChange::Add => patch.push_str(&format!(
            "new file mode 100644\n--- /dev/null\n+++ {name}\n"
        )),
        FileChange::Delete => patch.push_str(&format!(
            "deleted file mode 100644\n--- {name}\n+++ /dev/null\n"
        )),
        _ => patch.push_str(&format!("--- {name}\n+++ {name}\n")),
    }
    let number = |hunk: &Value, key: &str| hunk.get(key).and_then(Value::as_u64).unwrap_or(0);
    for hunk in hunks {
        patch.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            number(hunk, "oldStart"),
            number(hunk, "oldLines"),
            number(hunk, "newStart"),
            number(hunk, "newLines")
        ));
        for line in hunk
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(line) = line.as_str() {
                patch.push_str(line);
                patch.push('\n');
            }
        }
    }
    patch
}

/// `path` as git writes it with the default `core.quotePath`: in double quotes, with C escapes and octal bytes, when
/// it holds a quote, a backslash, a control byte or a byte outside ASCII.
fn quote(path: &str) -> String {
    let needs = path
        .bytes()
        .any(|byte| byte == b'"' || byte == b'\\' || !(0x20..0x7f).contains(&byte));
    if !needs {
        return path.to_string();
    }
    let mut quoted = String::from("\"");
    for byte in path.bytes() {
        match byte {
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            b'\t' => quoted.push_str("\\t"),
            b'\n' => quoted.push_str("\\n"),
            b'\r' => quoted.push_str("\\r"),
            0x20..=0x7e => quoted.push(char::from(byte)),
            other => quoted.push_str(&format!("\\{other:03o}")),
        }
    }
    quoted.push('"');
    quoted
}

/// The exit code a failed command's result names (`Exit code N`), when it does.
pub(crate) fn exit_code(text: &str) -> Option<i32> {
    let at = text.find("Exit code ")?;
    let digits: String = text[at + "Exit code ".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_tool_call_says_what_it_does_where_and_what_it_changes() {
        let edit = describe(
            "t",
            "Edit",
            &json!({"file_path": "C:/w/a.txt", "old_string": "two", "new_string": "three"}),
            256,
        );
        assert_eq!(edit.kind, Some(ToolKind::Edit));
        assert_eq!(edit.title.as_deref(), Some("Edit C:/w/a.txt"));
        assert_eq!(
            edit.locations,
            Some(vec![Location::new("C:/w/a.txt", None)])
        );
        let Some(ToolContent::Diff(diff)) =
            edit.content.as_ref().and_then(|content| content.first())
        else {
            panic!("an edit shows its change")
        };
        assert_eq!(
            (diff.old_text.as_deref(), diff.new_text.as_deref()),
            (Some("two"), Some("three"))
        );
        let read = describe("r", "Read", &json!({"file_path": "/a", "offset": 7}), 256);
        assert_eq!(read.locations, Some(vec![Location::new("/a", Some(7))]));
        let bash = describe("b", "Bash", &json!({"command": "echo one\necho two"}), 256);
        assert_eq!(bash.title.as_deref(), Some("echo one…"));
        assert_eq!(bash.content, Some(vec![ToolContent::Terminal("b".into())]));
        let mcp = describe("m", "mcp__echo__echo", &json!({"text": "hi"}), 256);
        assert_eq!(
            (mcp.kind, mcp.title.as_deref()),
            (Some(ToolKind::Other), Some("echo: echo"))
        );
    }

    #[test]
    fn a_patch_is_made_from_claude_codes_hunks_as_git_writes_one() {
        let hunks = [
            json!({"oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2, "lines": [" one", "-two", "+three"]}),
        ];
        assert_eq!(
            git_patch("C:\\w\\notes.txt", FileChange::Modify, &hunks),
            "diff --git C:/w/notes.txt C:/w/notes.txt\n--- C:/w/notes.txt\n+++ C:/w/notes.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+three\n"
        );
        assert!(
            git_patch("/w/né\"w", FileChange::Add, &[])
                .starts_with("diff --git \"/w/n\\303\\251\\\"w\"")
        );
        assert_eq!(exit_code("Error: Exit code 3\nboom"), Some(3));
    }
}
