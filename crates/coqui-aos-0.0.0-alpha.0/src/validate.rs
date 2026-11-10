//! Validating a manifest's data model: the rules `schemas/overlay.schema.json` states, then
//! those it cannot (an integer refusing a float, cross references, reserved names, files that
//! must exist), each reported with a JSON pointer to the offending value.

use alloc::collections::BTreeSet;

use serde_json::Value as Data;

use super::defaults::{DEFAULT_KEYS, OverlayTree, default_paths};
use super::hooks::HookEvent;
use super::names::{
    is_reserved_variable, is_variable_name, overlay_name_shaped, valid_os_name, valid_overlay_name,
    valid_relative_path, valid_tool_name,
};
use super::read::Document;

/// What is wrong with a manifest, by the class the specification's vectors name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum IssueCode {
    /// `env-name-case-duplicate`.
    EnvNameCaseDuplicate,
    /// `env-path-conflict`.
    EnvPathConflict,
    /// `integer-required`.
    IntegerRequired,
    /// `invalid-source`.
    InvalidSource,
    /// `license-expression`.
    LicenseExpression,
    /// `local-source`.
    LocalSource,
    /// `manifest-file`.
    ManifestFile,
    /// `mcp-command`.
    McpCommand,
    /// `mcp-duplicate-server`.
    McpDuplicateServer,
    /// `mcp-duplicate-skill`.
    McpDuplicateSkill,
    /// `mcp-unknown-server`.
    McpUnknownServer,
    /// `mcp-url`.
    McpUrl,
    /// `missing-file`.
    MissingFile,
    /// `missing-skill`.
    MissingSkill,
    /// `ref-not-allowed`.
    RefNotAllowed,
    /// `reserved-name`.
    ReservedName,
    /// `reserved-variable`.
    ReservedVariable,
    /// `schema`.
    Schema,
    /// `skill-ambiguous`.
    SkillAmbiguous,
    /// `skill-case`.
    SkillCase,
    /// `skill-duplicate`.
    SkillDuplicate,
    /// `skill-name`.
    SkillName,
    /// `source-credentials`.
    SourceCredentials,
    /// `two-manifests`.
    TwoManifests,
}

impl IssueCode {
    /// The class as the specification's vectors write it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EnvNameCaseDuplicate => "env-name-case-duplicate",
            Self::EnvPathConflict => "env-path-conflict",
            Self::IntegerRequired => "integer-required",
            Self::InvalidSource => "invalid-source",
            Self::LicenseExpression => "license-expression",
            Self::LocalSource => "local-source",
            Self::ManifestFile => "manifest-file",
            Self::McpCommand => "mcp-command",
            Self::McpDuplicateServer => "mcp-duplicate-server",
            Self::McpDuplicateSkill => "mcp-duplicate-skill",
            Self::McpUnknownServer => "mcp-unknown-server",
            Self::McpUrl => "mcp-url",
            Self::MissingFile => "missing-file",
            Self::MissingSkill => "missing-skill",
            Self::RefNotAllowed => "ref-not-allowed",
            Self::ReservedName => "reserved-name",
            Self::ReservedVariable => "reserved-variable",
            Self::Schema => "schema",
            Self::SkillAmbiguous => "skill-ambiguous",
            Self::SkillCase => "skill-case",
            Self::SkillDuplicate => "skill-duplicate",
            Self::SkillName => "skill-name",
            Self::SourceCredentials => "source-credentials",
            Self::TwoManifests => "two-manifests",
        }
    }

    /// The class a vector's name stands for.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "env-name-case-duplicate" => Some(Self::EnvNameCaseDuplicate),
            "env-path-conflict" => Some(Self::EnvPathConflict),
            "integer-required" => Some(Self::IntegerRequired),
            "invalid-source" => Some(Self::InvalidSource),
            "license-expression" => Some(Self::LicenseExpression),
            "local-source" => Some(Self::LocalSource),
            "manifest-file" => Some(Self::ManifestFile),
            "mcp-command" => Some(Self::McpCommand),
            "mcp-duplicate-server" => Some(Self::McpDuplicateServer),
            "mcp-duplicate-skill" => Some(Self::McpDuplicateSkill),
            "mcp-unknown-server" => Some(Self::McpUnknownServer),
            "mcp-url" => Some(Self::McpUrl),
            "missing-file" => Some(Self::MissingFile),
            "missing-skill" => Some(Self::MissingSkill),
            "ref-not-allowed" => Some(Self::RefNotAllowed),
            "reserved-name" => Some(Self::ReservedName),
            "reserved-variable" => Some(Self::ReservedVariable),
            "schema" => Some(Self::Schema),
            "skill-ambiguous" => Some(Self::SkillAmbiguous),
            "skill-case" => Some(Self::SkillCase),
            "skill-duplicate" => Some(Self::SkillDuplicate),
            "skill-name" => Some(Self::SkillName),
            "source-credentials" => Some(Self::SourceCredentials),
            "two-manifests" => Some(Self::TwoManifests),
            _ => None,
        }
    }
}

impl core::fmt::Display for IssueCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One problem with a manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct Issue {
    /// Where, as a JSON pointer such as `/env/PATH` or `/setup/0/timeout`.
    pub at: String,
    /// What: `schema`, `integer-required`, `reserved-name`, `reserved-variable`,
    /// `env-path-conflict`, `missing-file`, `missing-skill`, and the like.
    pub code: IssueCode,
}

/// Whether a manifest with these problems is still applied: a problem under `/about`, which
/// is informational only, never stops it.
pub fn applies(issues: &[Issue]) -> bool {
    issues
        .iter()
        .all(|issue| issue.at == "/about" || issue.at.starts_with("/about/"))
}

/// Every problem with `manifest`, sorted; empty when it is valid. With `tree`, every file a
/// field names must be one of the overlay's files, and `env` may set no variable a default
/// path addition adds to.
pub fn validate(manifest: &Document, tree: Option<&OverlayTree>) -> Vec<Issue> {
    let mut check = Check::default();
    check.document(manifest);
    semantic(manifest, &mut check);
    if let Some(tree) = tree {
        files(manifest, tree, &mut check);
    }
    let unique: BTreeSet<Issue> = check.issues.into_iter().collect();
    unique.into_iter().collect()
}

#[derive(Default)]
struct Check {
    issues: Vec<Issue>,
}

/// One reference token of a JSON Pointer: `~` is `~0` and `/` is `~1`.
fn token(text: &str) -> String {
    text.replace('~', "~0").replace('/', "~1")
}

fn join(at: &str, key: impl core::fmt::Display) -> String {
    format!("{at}/{}", token(&key.to_string()))
}

fn no_nul(text: &str) -> bool {
    !text.contains('\0')
}

fn is_line(text: &str) -> bool {
    !text.is_empty()
        && !text.chars().any(|c| {
            matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}')
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
}

fn is_web_url(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("https://") else {
        return false;
    };
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(host_end);
    let control = |c: char| matches!(c, '\u{0}'..='\u{20}' | '\u{7f}'..='\u{9f}');
    !host.is_empty()
        && !host.chars().any(|c| control(c) || matches!(c, '@' | '\\'))
        && !tail.chars().any(control)
}

fn is_mcp_url(text: &str) -> bool {
    let lower: String = text
        .chars()
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &text[8..]
    } else if lower.starts_with("http://") {
        &text[7..]
    } else {
        return false;
    };
    let host_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(host_end);
    !host.is_empty()
        && !host
            .chars()
            .any(|c| matches!(c, '#' | '@') || c.is_whitespace())
        && !tail.chars().any(|c| c == '#' || c.is_whitespace())
}

fn is_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() <= 3
        && parts.iter().all(|part| {
            (1..=9).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn is_environment_name(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

pub(crate) fn is_digest(text: &str) -> bool {
    text.strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn is_lowercase_id(text: &str, first_digit: bool, extra: &[u8]) -> bool {
    let bytes = text.as_bytes();
    bytes
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || (first_digit && byte.is_ascii_digit()))
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || *byte == b'-'
                || extra.contains(byte)
        })
}

/// A script a hook runs: a script path with an extension that names an interpreter.
fn is_hook_run(text: &str) -> bool {
    is_script_path(text)
        && [".py", ".sh", ".bash", ".ps1", ".js", ".mjs", ".cjs"]
            .iter()
            .any(|extension| text.ends_with(extension))
}

fn is_script_path(text: &str) -> bool {
    valid_relative_path(text)
        && text.split('/').all(|component| {
            let bytes = component.as_bytes();
            bytes.first().is_some_and(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-')
            }) && bytes.iter().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+' | b'-' | b'.')
            })
        })
}

fn is_command_name(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn is_header_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+.^_`|~-".contains(&byte))
}

impl Check {
    fn fail(&mut self, code: IssueCode, at: impl Into<String>) {
        self.issues.push(Issue {
            at: at.into(),
            code,
        });
    }

    /// Checks `value` with `ok`, reporting `at` when it fails.
    fn require(&mut self, ok: bool, at: &str) {
        if !ok {
            self.fail(IssueCode::Schema, at);
        }
    }

    fn string(&mut self, value: &Data, at: &str, ok: impl Fn(&str) -> bool) {
        let fine = value.as_str().is_some_and(ok);
        self.require(fine, at);
    }

    fn boolean(&mut self, value: &Data, at: &str) {
        self.require(value.is_boolean(), at);
    }

    /// An integer in `low..=high`; a float of any value is `integer-required`.
    fn integer(&mut self, value: &Data, at: &str, low: i64, high: i64) {
        if value.is_f64() {
            self.fail(IssueCode::IntegerRequired, at);
        } else {
            let fine = value.as_i64().is_some_and(|n| (low..=high).contains(&n));
            self.require(fine, at);
        }
    }

    fn array<'a>(&mut self, value: &'a Data, at: &str) -> &'a [Data] {
        match value.as_array() {
            Some(items) => items,
            None => {
                self.fail(IssueCode::Schema, at);
                &[]
            }
        }
    }

    fn each(&mut self, value: &Data, at: &str, item: impl Fn(&mut Self, &Data, &str)) {
        for (index, entry) in self.array(value, at).iter().enumerate() {
            item(self, entry, &join(at, index));
        }
    }

    fn unique(&mut self, value: &Data, at: &str) {
        if let Some(items) = value.as_array() {
            let mut seen = BTreeSet::new();
            for item in items {
                if !seen.insert(item.to_string()) {
                    self.fail(IssueCode::Schema, at);
                    return;
                }
            }
        }
    }

    /// A mapping whose keys are only `known`; `None` for something that is not one.
    fn object<'a>(&mut self, value: &'a Data, at: &str, known: &[&str]) -> Option<&'a Document> {
        let Some(map) = value.as_object() else {
            self.fail(IssueCode::Schema, at);
            return None;
        };
        for key in map.keys() {
            if !known.contains(&key.as_str()) {
                self.fail(IssueCode::Schema, join(at, key));
            }
        }
        Some(map)
    }

    fn required(&mut self, map: &Document, at: &str, keys: &[&str]) {
        for key in keys {
            if !map.contains_key(*key) {
                self.fail(IssueCode::Schema, at);
            }
        }
    }

    fn rel_path(&mut self, value: &Data, at: &str) {
        self.string(value, at, valid_relative_path);
    }

    fn os_list(&mut self, value: &Data, at: &str) {
        self.each(value, at, |check, name, at| {
            check.string(name, at, valid_os_name)
        });
    }

    fn variable(&mut self, name: &str, at: &str) {
        self.require(is_variable_name(name) && !is_reserved_variable(name), at);
    }

    fn document(&mut self, doc: &Document) {
        for (key, value) in doc {
            let at = format!("/{}", token(key));
            match key.as_str() {
                "name" | "description" => self.string(value, &at, |_| true),
                "requires" => self.each(value, &at, |check, name, at| {
                    check.string(name, at, overlay_name_shaped);
                }),
                "routing" => self.each(value, &at, |check, line, at| {
                    check.string(line, at, |_| true)
                }),
                "rules" | "context" => self.each(value, &at, Self::rel_path),
                "priority" => self.integer(value, &at, i64::MIN, i64::MAX),
                "defaults" => {
                    if let Some(map) = self.object(value, &at, &DEFAULT_KEYS) {
                        for (key, flag) in map {
                            self.boolean(flag, &join(&at, key));
                        }
                    }
                }
                "skills" => {
                    self.each(value, &at, Self::skill_entry);
                    self.unique(value, &at);
                }
                "env" => self.env_map(value, &at),
                "env_files" => self.each(value, &at, Self::env_file),
                "paths" => self.each(value, &at, Self::path_addition),
                "env_scripts" => self.each(value, &at, |check, entry, at| {
                    check.script(entry, at, false)
                }),
                "setup" => self.each(value, &at, |check, entry, at| check.script(entry, at, true)),
                "runtime" => self.runtime(value, &at),
                "scripts" => self.scripts(value, &at),
                "mcp" => self.mcp(value, &at),
                "harness" => self.harness(value, &at),
                "hooks" => self.hooks(value, &at),
                "about" => self.about(value, &at),
                _ => {}
            }
        }
    }

    /// A skill's name, or a table naming its source.
    fn skill_entry(&mut self, entry: &Data, at: &str) {
        match entry {
            Data::String(name) => self.require(valid_tool_name(name), at),
            Data::Object(_) => {
                let Some(map) = self.object(entry, at, &["name", "source", "ref", "pin"]) else {
                    return;
                };
                self.required(map, at, &["name", "source"]);
                for (key, value) in map {
                    let there = join(at, key);
                    match key.as_str() {
                        "name" => self.string(value, &there, valid_tool_name),
                        "source" => self.string(value, &there, |text| {
                            text.chars().any(|c| !matches!(c, ' ' | '\t' | '\r' | '\n'))
                        }),
                        "ref" => self.string(value, &there, is_git_ref),
                        "pin" => self.string(value, &there, is_digest),
                        _ => {}
                    }
                }
            }
            _ => self.fail(IssueCode::Schema, at),
        }
    }

    /// What runs at each event: a script, a table, or false for nothing.
    fn hooks(&mut self, value: &Data, at: &str) {
        let Some(map) = value.as_object() else {
            self.fail(IssueCode::Schema, at);
            return;
        };
        for (name, entry) in map {
            let Some(event) = HookEvent::named(name) else {
                self.fail(IssueCode::Schema, at);
                continue;
            };
            let here = join(at, name);
            match entry {
                Data::Bool(false) => {}
                Data::String(run) => self.require(is_hook_run(run), &here),
                Data::Object(_) => self.hook_table(event, entry, &here),
                _ => self.fail(IssueCode::Schema, here),
            }
        }
    }

    fn hook_table(&mut self, event: HookEvent, entry: &Data, at: &str) {
        let (selects, sets, refuses) = match event {
            HookEvent::PreEnv | HookEvent::PreCompact => (false, false, true),
            HookEvent::SessionStart | HookEvent::PreTool => (true, true, true),
            HookEvent::Prompt => (false, true, true),
            HookEvent::PostTool => (true, false, false),
            _ => (false, event == HookEvent::Cd, false),
        };
        let mut known = vec!["run", "timeout", "on_failure"];
        if selects {
            known.push("match");
        }
        if sets {
            known.push("sets");
        }
        let Some(map) = self.object(entry, at, &known) else {
            return;
        };
        for (key, value) in map {
            let here = join(at, key);
            match key.as_str() {
                "run" => self.string(value, &here, is_hook_run),
                "timeout" => self.integer(value, &here, 1, 3600),
                "on_failure" => self.string(value, &here, |text| {
                    text == "report" || (refuses && text == "refuse")
                }),
                "match" => {
                    let choices: &[&str] = if event == HookEvent::SessionStart {
                        &["new", "resume", "clear", "compact", "fork"]
                    } else {
                        &[
                            "read",
                            "edit",
                            "delete",
                            "move",
                            "search",
                            "execute",
                            "think",
                            "fetch",
                            "switch_mode",
                            "other",
                        ]
                    };
                    self.hook_list(value, &here, |text| choices.contains(&text));
                }
                "sets" => self.hook_list(value, &here, |text| {
                    is_variable_name(text) && !is_reserved_variable(text)
                }),
                _ => {}
            }
        }
    }

    /// A list of one or more distinct strings that pass `ok`.
    fn hook_list(&mut self, value: &Data, at: &str, ok: impl Fn(&str) -> bool) {
        let Some(items) = value.as_array() else {
            self.fail(IssueCode::Schema, at);
            return;
        };
        self.require(!items.is_empty(), at);
        for (index, item) in items.iter().enumerate() {
            self.string(item, &join(at, index), &ok);
        }
        self.unique(value, at);
    }

    fn env_map(&mut self, value: &Data, at: &str) {
        let Some(map) = value.as_object() else {
            self.fail(IssueCode::Schema, at);
            return;
        };
        for (name, entry) in map {
            let here = join(at, name);
            self.variable(name, &here);
            self.require(env_value_ok(entry), &here);
        }
    }

    fn env_file(&mut self, entry: &Data, at: &str) {
        if entry.is_string() {
            self.rel_path(entry, at);
            return;
        }
        if let Some(map) = self.object(entry, at, &["path", "format", "os"]) {
            self.required(map, at, &["path"]);
            for (key, value) in map {
                let here = join(at, key);
                match key.as_str() {
                    "path" => self.rel_path(value, &here),
                    "format" => {
                        self.string(value, &here, |format| matches!(format, "aos" | "dotenv"))
                    }
                    "os" => self.os_list(value, &here),
                    _ => {}
                }
            }
        }
    }

    fn path_addition(&mut self, entry: &Data, at: &str) {
        if let Some(map) = self.object(entry, at, &["variable", "path", "position", "os"]) {
            self.required(map, at, &["variable", "path"]);
            for (key, value) in map {
                let here = join(at, key);
                match key.as_str() {
                    "variable" => {
                        let fine = value.as_str().is_some_and(|name| {
                            is_variable_name(name) && !is_reserved_variable(name)
                        });
                        self.require(fine, &here);
                    }
                    "path" => self.rel_path(value, &here),
                    "position" => {
                        self.string(value, &here, |position| {
                            matches!(position, "prepend" | "append")
                        });
                    }
                    "os" => self.os_list(value, &here),
                    _ => {}
                }
            }
        }
    }

    fn script(&mut self, entry: &Data, at: &str, setup: bool) {
        let known: &[&str] = if setup {
            &["interpreter", "path", "args", "timeout", "network", "os"]
        } else {
            &["interpreter", "path", "args", "timeout", "os"]
        };
        if let Some(map) = self.object(entry, at, known) {
            self.required(map, at, &["interpreter", "path"]);
            for (key, value) in map {
                let here = join(at, key);
                match key.as_str() {
                    "interpreter" => {
                        self.string(value, &here, |name| is_lowercase_id(name, false, b""))
                    }
                    "path" => self.rel_path(value, &here),
                    "args" => {
                        self.each(value, &here, |check, arg, at| check.string(arg, at, no_nul))
                    }
                    "timeout" => self.integer(value, &here, 0, 2_147_483_647),
                    "network" => self.boolean(value, &here),
                    "os" => self.os_list(value, &here),
                    _ => {}
                }
            }
        }
    }

    fn runtime(&mut self, value: &Data, at: &str) {
        let Some(map) = self.object(value, at, &["os", "commands", "python", "node"]) else {
            return;
        };
        for (key, value) in map {
            let here = join(at, key);
            match key.as_str() {
                "os" => self.os_list(value, &here),
                "commands" => self.each(value, &here, |check, command, at| {
                    if command.is_string() {
                        check.string(command, at, is_command_name);
                    } else if let Some(map) = check.object(command, at, &["name", "os"]) {
                        check.required(map, at, &["name"]);
                        if let Some(name) = map.get("name") {
                            check.string(name, &join(at, "name"), is_command_name);
                        }
                        if let Some(os) = map.get("os") {
                            check.os_list(os, &join(at, "os"));
                        }
                    }
                }),
                "python" => self.each(value, &here, |check, variant, at| {
                    check.variant(variant, at, true)
                }),
                "node" => self.each(value, &here, |check, variant, at| {
                    check.variant(variant, at, false)
                }),
                _ => {}
            }
        }
    }

    fn variant(&mut self, value: &Data, at: &str, python: bool) {
        let known: &[&str] = if python {
            &[
                "version",
                "requirements",
                "lock",
                "lock_digest",
                "environment",
                "os",
            ]
        } else {
            &["version", "package", "environment", "os"]
        };
        let Some(map) = self.object(value, at, known) else {
            return;
        };
        if map.contains_key("requirements") && map.contains_key("lock") {
            self.fail(IssueCode::Schema, at);
        }
        if map.contains_key("lock_digest") && !map.contains_key("lock") {
            self.fail(IssueCode::Schema, at);
        }
        for (key, value) in map {
            let here = join(at, key);
            match key.as_str() {
                "version" => self.string(value, &here, is_version),
                "requirements" | "lock" => self.rel_path(value, &here),
                "lock_digest" => self.string(value, &here, is_digest),
                "environment" => self.string(value, &here, is_environment_name),
                "package" => self.string(value, &here, |path| {
                    valid_relative_path(path)
                        && (path == "package.json" || path.ends_with("/package.json"))
                }),
                "os" => self.os_list(value, &here),
                _ => {}
            }
        }
    }

    fn scripts(&mut self, value: &Data, at: &str) {
        let Some(map) = value.as_object() else {
            self.fail(IssueCode::Schema, at);
            return;
        };
        for (name, target) in map {
            let here = join(at, name);
            let named =
                is_command_name_strict(name) && !name.to_ascii_lowercase().ends_with(".exe");
            self.require(named, &here);
            let fine = match target {
                Data::Bool(false) => true,
                Data::String(path) => {
                    is_script_path(path)
                        && [".py", ".sh", ".bash", ".ps1", ".js", ".mjs", ".cjs"]
                            .iter()
                            .any(|extension| path.ends_with(extension))
                }
                Data::Object(entry) => {
                    entry
                        .keys()
                        .all(|key| matches!(key.as_str(), "path" | "interpreter" | "os"))
                        && entry
                            .get("path")
                            .and_then(Data::as_str)
                            .is_some_and(is_script_path)
                        && entry
                            .get("interpreter")
                            .and_then(Data::as_str)
                            .is_some_and(|name| is_lowercase_id(name, false, b""))
                        && entry.get("os").is_none_or(os_list_ok)
                }
                _ => false,
            };
            self.require(fine, &here);
        }
    }

    fn mcp(&mut self, value: &Data, at: &str) {
        let Some(map) = self.object(value, at, &["servers", "skills"]) else {
            return;
        };
        if let Some(servers) = map.get("servers") {
            self.each(servers, &join(at, "servers"), Self::server);
        }
        if let Some(skills) = map.get("skills") {
            self.each(skills, &join(at, "skills"), |check, link, at| {
                if let Some(map) = check.object(link, at, &["name", "server"]) {
                    check.required(map, at, &["name", "server"]);
                    for (key, value) in map {
                        check.string(value, &join(at, key), valid_tool_name);
                    }
                }
            });
        }
    }

    fn server(&mut self, value: &Data, at: &str) {
        let known = [
            "name",
            "transport",
            "timeout",
            "command",
            "args",
            "env",
            "env_passthrough",
            "url",
            "headers_env",
        ];
        let Some(map) = self.object(value, at, &known) else {
            return;
        };
        self.required(map, at, &["name", "transport"]);
        let misshapen = match map.get("transport").and_then(Data::as_str) {
            Some("stdio") => {
                !map.contains_key("command")
                    || map.contains_key("url")
                    || map.contains_key("headers_env")
            }
            Some("streamable-http") => {
                !map.contains_key("url")
                    || ["command", "args", "env", "env_passthrough"]
                        .iter()
                        .any(|key| map.contains_key(*key))
            }
            _ => false,
        };
        if misshapen {
            self.fail(IssueCode::Schema, at);
        }
        for (key, value) in map {
            let here = join(at, key);
            match key.as_str() {
                "name" => self.string(value, &here, valid_tool_name),
                "transport" => {
                    self.string(value, &here, |transport| {
                        matches!(transport, "stdio" | "streamable-http")
                    });
                }
                "timeout" => {
                    let fine = value
                        .as_f64()
                        .is_some_and(|seconds| (0.001..=2_147_483_647.0).contains(&seconds));
                    self.require(fine, &here);
                }
                "command" => self.string(value, &here, |command| {
                    !command.is_empty() && no_nul(command)
                }),
                "args" => self.each(value, &here, |check, arg, at| check.string(arg, at, no_nul)),
                "env" => self.env_map(value, &here),
                "env_passthrough" => {
                    self.each(value, &here, |check, name, at| {
                        check.string(name, at, is_variable_name)
                    });
                    self.unique(value, &here);
                }
                "url" => self.string(value, &here, is_mcp_url),
                "headers_env" => {
                    if let Some(headers) = value.as_object() {
                        for (header, variable) in headers {
                            let fine = is_header_name(header)
                                && variable.as_str().is_some_and(is_variable_name);
                            self.require(fine, &join(&here, header));
                        }
                    } else {
                        self.fail(IssueCode::Schema, here);
                    }
                }
                _ => {}
            }
        }
    }

    fn harness(&mut self, value: &Data, at: &str) {
        let Some(map) = value.as_object() else {
            self.fail(IssueCode::Schema, at);
            return;
        };
        for (id, hints) in map {
            let here = join(at, id);
            self.require(is_lowercase_id(id, true, b""), &here);
            let Some(hints) = hints.as_object() else {
                self.fail(IssueCode::Schema, here);
                continue;
            };
            for (key, value) in hints {
                let there = join(&here, key);
                match key.as_str() {
                    "context" => self.each(value, &there, Self::rel_path),
                    "skills" => self.boolean(value, &there),
                    "model" | "effort" => self.string(value, &there, |text| !text.is_empty()),
                    _ => {}
                }
            }
        }
    }

    fn about(&mut self, value: &Data, at: &str) {
        let known = [
            "summary",
            "icon",
            "homepage",
            "repository",
            "license",
            "authors",
            "tags",
        ];
        let Some(map) = self.object(value, at, &known) else {
            return;
        };
        for (key, value) in map {
            let here = join(at, key);
            match key.as_str() {
                "summary" => self.string(value, &here, is_line),
                "icon" => self.string(value, &here, |path| {
                    let lower = path.to_ascii_lowercase();
                    valid_relative_path(path)
                        && (lower.ends_with(".png") || lower.ends_with(".webp"))
                }),
                "homepage" | "repository" => self.string(value, &here, is_web_url),
                "license" => {
                    self.string(value, &here, |license| {
                        !license.is_empty()
                            && license.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || b".+:() -".contains(&byte)
                            })
                    });
                    if let Some(license) = value.as_str()
                        && !valid_license(license)
                    {
                        self.fail(IssueCode::LicenseExpression, here);
                    }
                }
                "authors" => self.each(value, &here, |check, author, at| {
                    check.string(author, at, is_line)
                }),
                "tags" => {
                    self.each(value, &here, |check, tag, at| {
                        check.string(tag, at, |tag| {
                            tag.chars().count() <= 32
                                && !tag.is_empty()
                                && tag.split('-').all(|run| {
                                    !run.is_empty()
                                        && run.bytes().all(|byte| {
                                            byte.is_ascii_lowercase() || byte.is_ascii_digit()
                                        })
                                })
                        });
                    });
                    self.unique(value, &here);
                }
                _ => {}
            }
        }
    }
}

/// `[A-Za-z0-9][A-Za-z0-9._-]*`, a script command's name.
fn is_command_name_strict(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn os_list_ok(value: &Data) -> bool {
    value.as_array().is_some_and(|names| {
        names
            .iter()
            .all(|name| name.as_str().is_some_and(valid_os_name))
    })
}

/// Whether `entry` is exactly one form of env value.
pub(crate) fn env_value_ok(entry: &Data) -> bool {
    let forms = [
        entry.as_str().is_some_and(no_nul),
        object_form(entry, "value", &["if_unset"], |text| {
            text.as_str().is_some_and(no_nul)
        }),
        object_form(entry, "path", &["if_unset"], |path| {
            path.as_str().is_some_and(valid_relative_path)
        }),
        object_form(entry, "secret", &["if_unset", "optional"], |name| {
            name.as_str().is_some_and(|name| !name.is_empty())
        }),
        provider_form(entry),
    ];
    forms.iter().filter(|form| **form).count() == 1
}

fn object_form(entry: &Data, source: &str, flags: &[&str], ok: impl Fn(&Data) -> bool) -> bool {
    let Some(map) = entry.as_object() else {
        return false;
    };
    map.get(source).is_some_and(ok)
        && map.iter().all(|(key, value)| {
            key == source || (flags.contains(&key.as_str()) && value.is_boolean())
        })
}

fn provider_form(entry: &Data) -> bool {
    let Some(map) = entry.as_object() else {
        return false;
    };
    let (Some(provider), Some(key)) = (
        map.get("provider").and_then(Data::as_str),
        map.get("key").and_then(Data::as_str),
    ) else {
        return false;
    };
    let provider_ok = provider
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_lowercase)
        && provider.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        });
    let key_ok = !key.is_empty()
        && no_nul(key)
        && match provider {
            "file" => valid_relative_path(key),
            "env" => is_variable_name(key),
            _ => true,
        };
    provider_ok
        && key_ok
        && map.iter().all(|(field, value)| match field.as_str() {
            "provider" | "key" => true,
            "if_unset" | "optional" => value.is_boolean(),
            "value" | "path" | "secret" => false,
            _ => value.is_string() || value.is_i64() || value.is_u64() || value.is_boolean(),
        })
}

/// The rules JSON Schema cannot state.
fn semantic(doc: &Document, check: &mut Check) {
    for (index, name) in super::defaults::declared(doc, "requires")
        .iter()
        .enumerate()
    {
        if let Some(name) = name.as_str()
            && overlay_name_shaped(name)
            && !valid_overlay_name(name)
        {
            check.fail(IssueCode::ReservedName, format!("/requires/{index}"));
        }
    }
    let mut env_names = BTreeSet::new();
    if let Some(env) = doc.get("env").and_then(Data::as_object) {
        for name in env.keys() {
            if !env_names.insert(name.to_ascii_uppercase()) {
                check.fail(
                    IssueCode::EnvNameCaseDuplicate,
                    format!("/env/{}", token(name)),
                );
            }
            if is_reserved_variable(name) {
                check.fail(IssueCode::ReservedVariable, format!("/env/{}", token(name)));
            }
        }
    }
    for (index, entry) in super::defaults::declared(doc, "paths").iter().enumerate() {
        if let Some(variable) = entry.get("variable").and_then(Data::as_str) {
            if env_names.contains(&variable.to_ascii_uppercase()) {
                check.fail(
                    IssueCode::EnvPathConflict,
                    format!("/paths/{index}/variable"),
                );
            }
            if is_reserved_variable(variable) {
                check.fail(
                    IssueCode::ReservedVariable,
                    format!("/paths/{index}/variable"),
                );
            }
        }
    }
    skills(doc, check);
    manifest_named(doc, check);
    let Some(mcp) = doc.get("mcp").and_then(Data::as_object) else {
        return;
    };
    let empty = Vec::new();
    let servers = mcp
        .get("servers")
        .and_then(Data::as_array)
        .unwrap_or(&empty);
    let mut names = BTreeSet::new();
    for (index, server) in servers.iter().enumerate() {
        let Some(server) = server.as_object() else {
            continue;
        };
        if !names.insert(server.get("name").map(Data::to_string)) {
            check.fail(
                IssueCode::McpDuplicateServer,
                format!("/mcp/servers/{index}/name"),
            );
        }
        if let Some(command) = server.get("command").and_then(Data::as_str)
            && command.contains(['/', '\\'])
            && !valid_relative_path(command)
        {
            check.fail(
                IssueCode::McpCommand,
                format!("/mcp/servers/{index}/command"),
            );
        }
        if let Some(url) = server.get("url").and_then(Data::as_str) {
            let after = url.split_once("://").map_or(url, |(_, rest)| rest);
            let authority = after.split('/').next().unwrap_or_default();
            if url.contains('#') || authority.contains('@') {
                check.fail(IssueCode::McpUrl, format!("/mcp/servers/{index}/url"));
            }
        }
        if let Some(env) = server.get("env").and_then(Data::as_object) {
            let mut seen = BTreeSet::new();
            for name in env.keys() {
                if is_reserved_variable(name) {
                    check.fail(
                        IssueCode::ReservedVariable,
                        format!("/mcp/servers/{index}/env/{}", token(name)),
                    );
                }
                if !seen.insert(name.to_ascii_uppercase()) {
                    check.fail(
                        IssueCode::EnvNameCaseDuplicate,
                        format!("/mcp/servers/{index}/env/{}", token(name)),
                    );
                }
            }
        }
    }
    let links = mcp.get("skills").and_then(Data::as_array).unwrap_or(&empty);
    let mut linked = BTreeSet::new();
    for (index, link) in links.iter().enumerate() {
        let Some(link) = link.as_object() else {
            continue;
        };
        if !linked.insert(link.get("name").map(Data::to_string)) {
            check.fail(
                IssueCode::McpDuplicateSkill,
                format!("/mcp/skills/{index}/name"),
            );
        }
        if !names.contains(&link.get("server").map(Data::to_string)) {
            check.fail(
                IssueCode::McpUnknownServer,
                format!("/mcp/skills/{index}/server"),
            );
        }
    }
}

/// Every `(pointer, relative path)` of a file a field requires to exist.
pub fn named_files(doc: &Document) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut each = |key: &str, field: Option<&str>| {
        for (index, item) in super::defaults::declared(doc, key).iter().enumerate() {
            match (field, item) {
                (None, Data::String(path)) => out.push((format!("/{key}/{index}"), path.clone())),
                (_, Data::Object(map)) => {
                    let field = field.unwrap_or("path");
                    if let Some(path) = map.get(field).and_then(Data::as_str) {
                        out.push((format!("/{key}/{index}/{field}"), path.to_owned()));
                    }
                }
                _ => {}
            }
        }
    };
    each("rules", None);
    each("context", None);
    each("env_files", None);
    each("env_scripts", Some("path"));
    each("setup", Some("path"));
    if let Some(runtime) = doc.get("runtime").and_then(Data::as_object) {
        for (runtime_name, fields) in [
            ("python", &["requirements", "lock"][..]),
            ("node", &["package"][..]),
        ] {
            for (index, variant) in runtime
                .get(runtime_name)
                .and_then(Data::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .enumerate()
            {
                for field in fields {
                    if let Some(path) = variant.get(*field).and_then(Data::as_str) {
                        out.push((
                            format!("/runtime/{runtime_name}/{index}/{field}"),
                            path.to_owned(),
                        ));
                    }
                }
            }
        }
    }
    if let Some(hooks) = doc.get("hooks").and_then(Data::as_object) {
        for (event, entry) in hooks {
            match entry {
                Data::String(run) => out.push((format!("/hooks/{}", token(event)), run.clone())),
                Data::Object(map) => {
                    if let Some(run) = map.get("run").and_then(Data::as_str) {
                        out.push((format!("/hooks/{}/run", token(event)), run.to_owned()));
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(scripts) = doc.get("scripts").and_then(Data::as_object) {
        for (name, target) in scripts {
            match target {
                Data::String(path) => out.push((format!("/scripts/{}", token(name)), path.clone())),
                Data::Object(map) => {
                    if let Some(path) = map.get("path").and_then(Data::as_str) {
                        out.push((format!("/scripts/{}/path", token(name)), path.to_owned()));
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(harness) = doc.get("harness").and_then(Data::as_object) {
        for (id, hints) in harness {
            for (index, path) in hints
                .get("context")
                .and_then(Data::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .enumerate()
            {
                if let Some(path) = path.as_str() {
                    out.push((
                        format!("/harness/{}/context/{index}", token(id)),
                        path.to_owned(),
                    ));
                }
            }
        }
    }
    if let Some(icon) = doc
        .get("about")
        .and_then(|about| about.get("icon"))
        .and_then(Data::as_str)
    {
        out.push(("/about/icon".to_owned(), icon.to_owned()));
    }
    out
}

fn files(doc: &Document, tree: &OverlayTree, check: &mut Check) {
    for (at, path) in named_files(doc) {
        if !tree.contains(&path) {
            check.fail(IssueCode::MissingFile, at);
        }
    }
    if let Some(env) = doc.get("env").and_then(Data::as_object) {
        let added: BTreeSet<String> = default_paths(doc, tree)
            .iter()
            .filter_map(|entry| entry.get("variable").and_then(Data::as_str))
            .map(str::to_ascii_uppercase)
            .collect();
        for name in env.keys() {
            if added.contains(&name.to_ascii_uppercase()) {
                check.fail(IssueCode::EnvPathConflict, format!("/env/{}", token(name)));
            }
        }
    }
    if let Some(hooks) = doc.get("hooks").and_then(Data::as_object)
        && !super::defaults::DEFAULT_SCRIPTS
            .iter()
            .any(|(extension, _, _)| {
                tree.members()
                    .any(|path| path == format!("hook{extension}"))
            })
    {
        // A table with no run runs hook.<ext>, which has to be there.
        for (event, entry) in hooks {
            if entry
                .as_object()
                .is_some_and(|table| !table.contains_key("run"))
            {
                check.fail(IssueCode::MissingFile, format!("/hooks/{}", token(event)));
            }
        }
    }
    let bundled = super::defaults::bundled_skills(tree);
    let folded: BTreeSet<String> = bundled.iter().map(|name| name.to_lowercase()).collect();
    for (index, entry) in super::defaults::declared(doc, "skills").iter().enumerate() {
        let Some(name) = skill_name(entry) else {
            continue;
        };
        if !folded.contains(&name.to_lowercase()) {
            continue;
        }
        if entry.is_object() {
            check.fail(IssueCode::SkillAmbiguous, skill_at(entry, index));
        } else if !bundled.contains(name) {
            check.fail(IssueCode::SkillCase, skill_at(entry, index));
        }
    }
    if tree.contains("OVERLAY.md") && tree.contains("overlay.cfg") {
        check.fail(IssueCode::TwoManifests, "");
    }
    if let Some(links) = doc
        .get("mcp")
        .and_then(|mcp| mcp.get("skills"))
        .and_then(Data::as_array)
    {
        for (index, link) in links.iter().enumerate() {
            if let Some(name) = link.get("name").and_then(Data::as_str)
                && !tree.contains(&format!("skills/{name}/SKILL.md"))
            {
                check.fail(IssueCode::MissingSkill, format!("/mcp/skills/{index}/name"));
            }
        }
    }
}

/// The name of an entry of `skills`: the string, or a table's `name`.
fn skill_name(entry: &Data) -> Option<&str> {
    match entry {
        Data::Object(table) => table.get("name").and_then(Data::as_str),
        other => other.as_str(),
    }
}

/// The pointer of an entry's name: the entry, or the `name` of a table.
fn skill_at(entry: &Data, index: usize) -> String {
    if entry.is_object() {
        format!("/skills/{index}/name")
    } else {
        format!("/skills/{index}")
    }
}

/// The rules of `skills` that need no files: names, and what a table's source may be.
fn skills(doc: &Document, check: &mut Check) {
    let mut seen = BTreeSet::new();
    for (index, entry) in super::defaults::declared(doc, "skills").iter().enumerate() {
        let Some(name) = skill_name(entry) else {
            continue;
        };
        let at = skill_at(entry, index);
        if valid_tool_name(name) && !valid_relative_path(name) {
            check.fail(IssueCode::SkillName, at.clone());
        }
        if !seen.insert(name.to_lowercase()) {
            check.fail(IssueCode::SkillDuplicate, at);
        }
        if let Some(table) = entry.as_object()
            && let Some(source) = table.get("source").and_then(Data::as_str)
        {
            let there = format!("/skills/{index}");
            let Ok(parsed) = super::source::Source::parse(source) else {
                check.fail(IssueCode::InvalidSource, format!("{there}/source"));
                continue;
            };
            if parsed.is_local() {
                check.fail(IssueCode::LocalSource, format!("{there}/source"));
            } else if parsed.kind == super::source::SourceKind::Url && !parsed.is_web_url() {
                check.fail(IssueCode::InvalidSource, format!("{there}/source"));
            } else if parsed.holds_credentials() {
                check.fail(IssueCode::SourceCredentials, format!("{there}/source"));
            }
            if table.contains_key("ref")
                && (parsed.kind != super::source::SourceKind::Git || parsed.reference.is_some())
            {
                check.fail(IssueCode::RefNotAllowed, format!("{there}/ref"));
            }
        }
    }
}

/// A manifest is never another field's file: only `context` names `OVERLAY.md`, for its body,
/// and nothing names `overlay.cfg`.
fn manifest_named(doc: &Document, check: &mut Check) {
    for (at, path) in named_files(doc) {
        let is_context = at.starts_with("/context/")
            || (at.starts_with("/harness/") && at.contains("/context/"));
        if path == "overlay.cfg" || (path == "OVERLAY.md" && !is_context) {
            check.fail(IssueCode::ManifestFile, at);
        }
    }
}

/// What a validator flags without refusing: each skill name neither bundled nor given a source,
/// which only a consumer's catalogs can resolve, so a typo is seen before it is looked up. The
/// pointers of the names.
pub fn skill_flags(doc: &Document, tree: &OverlayTree) -> Vec<String> {
    let folded: BTreeSet<String> = super::defaults::bundled_skills(tree)
        .iter()
        .map(|name| name.to_lowercase())
        .collect();
    super::defaults::declared(doc, "skills")
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let name = entry.as_str()?;
            (!folded.contains(&name.to_lowercase())).then(|| format!("/skills/{index}"))
        })
        .collect()
}

/// Whether one informational field of `about` is valid; an unknown field is not.
pub(crate) fn about_field_valid(field: &str, value: &Data) -> bool {
    let mut check = Check::default();
    let mut map = Document::new();
    map.insert(field.to_owned(), value.clone());
    check.about(&Data::Object(map), "/about");
    check.issues.is_empty()
}

/// Whether one field of a catalog's own (`name`, `description`, `homepage`) is valid.
pub(crate) fn catalog_field_valid(field: &str, value: &Data) -> bool {
    match (field, value) {
        ("name", Data::String(text)) => is_line(text),
        ("description", Data::String(_)) => true,
        ("homepage", Data::String(text)) => is_web_url(text),
        _ => false,
    }
}

/// A Git ref a skill table names: no `-` first, and none of `/ : \ @ #`, controls or spaces.
pub(crate) fn is_git_ref(text: &str) -> bool {
    let mut chars = text.chars();
    let bad = |c: char| matches!(c, '/' | ':' | '\\' | '@' | '#') || c <= ' ' || c == '\u{7f}';
    chars
        .next()
        .is_some_and(|first| first != '-' && !bad(first))
        && chars.all(|c| !bad(c))
}

/// Whether `expression` is an SPDX license expression: identifiers and `LicenseRef-`
/// references, `+`, `WITH`, `AND` and `OR` in upper case, and parentheses.
pub fn valid_license(expression: &str) -> bool {
    if expression.is_empty()
        || !expression
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".+:() -".contains(&byte))
    {
        return false;
    }
    let mut tokens = Vec::new();
    let mut word = String::new();
    for c in expression.chars() {
        if matches!(c, '(' | ')' | ' ') {
            if !word.is_empty() {
                tokens.push(core::mem::take(&mut word));
            }
            if c != ' ' {
                tokens.push(c.to_string());
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    let mut parser = License {
        tokens: &tokens,
        at: 0,
        depth: 0,
    };
    parser.compound() && parser.at == tokens.len()
}

/// How deep parentheses may nest in a license expression: past it the text is no expression.
const LICENSE_DEPTH: usize = 128;

struct License<'a> {
    tokens: &'a [String],
    at: usize,
    depth: usize,
}

fn id_chars(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn reference(text: &str, kind: &str) -> bool {
    let text = match text
        .strip_prefix("DocumentRef-")
        .and_then(|rest| rest.split_once(':'))
    {
        Some((document, rest)) if id_chars(document) => rest,
        Some(_) => return false,
        None => text,
    };
    text.strip_prefix(kind).is_some_and(id_chars)
}

impl License<'_> {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.at).map(String::as_str)
    }

    fn take(&mut self) -> Option<&str> {
        let token = self.tokens.get(self.at).map(String::as_str);
        self.at += 1;
        token
    }

    fn license_or_group(&mut self) -> bool {
        match self.take() {
            Some("(") => {
                self.depth += 1;
                let closed =
                    self.depth <= LICENSE_DEPTH && self.compound() && self.take() == Some(")");
                self.depth -= 1;
                closed
            }
            None | Some("AND" | "OR" | "WITH" | ")") => false,
            Some(token) => {
                let license = reference(token, "LicenseRef-")
                    || id_chars(token.strip_suffix('+').unwrap_or(token));
                if !license {
                    return false;
                }
                if self.peek() == Some("WITH") {
                    self.take();
                    return self.take().is_some_and(|exception| {
                        id_chars(exception) || reference(exception, "AdditionRef-")
                    });
                }
                true
            }
        }
    }

    fn term(&mut self) -> bool {
        if !self.license_or_group() {
            return false;
        }
        while self.peek() == Some("AND") {
            self.take();
            if !self.license_or_group() {
                return false;
            }
        }
        true
    }

    fn compound(&mut self) -> bool {
        if !self.term() {
            return false;
        }
        while self.peek() == Some("OR") {
            self.take();
            if !self.term() {
                return false;
            }
        }
        true
    }
}
