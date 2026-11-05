//! The checks every `SKILL.md` AI6 delivers passes, whether Moneypenny's catalog or an
//! overlay issues it.

use std::collections::BTreeMap;
use std::path::Path;

use crate::bounds::Limit;

/// The `SKILL.md` front matter keys that grant nothing and run nothing
/// (code.claude.com/docs/en/skills): any other key, such as `allowed-tools`, `hooks`,
/// `shell`, `model` or `context`, or one Claude Code adds later, needs tools allowed.
pub const SKILL_INERT_KEYS: [&str; 12] = [
    "name",
    "description",
    "when_to_use",
    "argument-hint",
    "arguments",
    "disable-model-invocation",
    "user-invocable",
    "disallowed-tools",
    "paths",
    "metadata",
    "license",
    "compatibility",
];

/// Checks the skill in `dir` as `name`: a `SKILL.md` that is a regular file of at most
/// `file_bytes`, UTF-8, opening with `---` front matter that parses as YAML with `name` equal
/// to `name` and a non-empty `description`. Unless `allow_tools`, its front matter has only
/// [`SKILL_INERT_KEYS`] and it has no injected command (`` !`command` ``, or a fenced block
/// opened with `` ```! ``), since those grant tools or run commands in the Session; a refusal
/// names `tools_setting` as what would let it load. Reads the file; never follows a link.
///
/// # Errors
///
/// A message saying what is wrong with it.
pub fn check_skill(
    dir: &Path,
    name: &str,
    allow_tools: bool,
    tools_setting: &str,
    file_bytes: Limit,
) -> Result<(), String> {
    let path = dir.join("SKILL.md");
    let metadata = std::fs::symlink_metadata(dir)
        .and_then(|metadata| {
            if metadata.is_dir() {
                std::fs::symlink_metadata(&path)
            } else {
                Err(std::io::Error::other(
                    "the skill is not a directory (a link is never followed)",
                ))
            }
        })
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.file_type().is_file() || !file_bytes.allows(metadata.len()) {
        return Err(format!(
            "{} is not a regular file of at most {file_bytes} bytes",
            path.display()
        ));
    }
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let (yaml, _) = front_matter(&text)
        .ok_or("SKILL.md does not open with front matter between two `---` lines")?;
    let keys: BTreeMap<String, serde_json::Value> =
        serde_saphyr::from_str(yaml).map_err(|error| {
            format!(
                "SKILL.md front matter is not a YAML mapping: {}",
                error.to_string().lines().next().unwrap_or_default()
            )
        })?;
    if keys.get("name").and_then(serde_json::Value::as_str) != Some(name) {
        return Err(format!("SKILL.md front matter does not have name: {name}"));
    }
    let description = keys
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if description.trim().is_empty() {
        return Err("SKILL.md front matter has no description".to_owned());
    }
    if allow_tools {
        return Ok(());
    }
    if let Some(key) = keys
        .keys()
        .find(|key| !SKILL_INERT_KEYS.contains(&key.as_str()))
    {
        return Err(format!(
            "SKILL.md front matter sets {key:?}, which can grant tools, run commands or change \
             the Session; only {tools_setting} = true lets it load"
        ));
    }
    if injects_commands(&text) {
        return Err(format!(
            "SKILL.md injects a command (!`…` or a ```! block), which runs before the Agent \
             reads it; only {tools_setting} = true lets it load"
        ));
    }
    Ok(())
}

/// What the `SKILL.md` in `dir` says it is for: its front matter's `description`.
pub fn skill_description(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("SKILL.md")).ok()?;
    let (yaml, _) = front_matter(&text)?;
    let keys: BTreeMap<String, serde_json::Value> = serde_saphyr::from_str(yaml).ok()?;
    keys.get("description")?.as_str().map(str::to_owned)
}

/// The YAML between the opening `---` line and the next `---` line, and the text after it.
fn front_matter(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))?;
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Some((&rest[..at], &rest[at + line.len()..]));
        }
        at += line.len();
    }
    None
}

/// Whether `text` holds a command Claude Code runs when the skill loads: `!` then a backtick
/// anywhere (Claude Code runs it only at a line start or after whitespace; any is refused), or
/// a fence of three or more backticks or tildes followed by `!`.
fn injects_commands(text: &str) -> bool {
    text.contains("!`")
        || text.lines().any(|line| {
            let line = line.trim_start();
            ['`', '~'].iter().any(|fence| {
                let after = line.trim_start_matches(*fence);
                line.len() - after.len() >= 3 && after.trim_start().starts_with('!')
            })
        })
}
