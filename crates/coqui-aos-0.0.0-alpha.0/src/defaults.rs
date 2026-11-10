//! What conventional files contribute unless `[defaults]` turns them off
//! (`spec/v0/package.md#defaults`), computed from an overlay's list of files.

use alloc::collections::{BTreeMap, BTreeSet};

use serde_json::Value as Data;

use super::error::{Class, Error};
use super::front_matter::split_front_matter;
use super::kinds::Runtime;
use super::limits::{Limit, Settings};
use super::names::{normalize_name, os_matches, valid_relative_path, valid_tool_name};
use super::read::Document;
use super::source::Source;

/// An overlay's files: every path, and the text of those a consumer has read (env files and
/// dependency files), by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlayTree {
    paths: BTreeSet<String>,
    texts: BTreeMap<String, Vec<u8>>,
}

impl OverlayTree {
    /// A tree of these paths, none of them read.
    pub fn from_paths<I: IntoIterator<Item = S>, S: Into<String>>(paths: I) -> Self {
        Self {
            paths: paths.into_iter().map(Into::into).collect(),
            texts: BTreeMap::new(),
        }
    }

    /// A tree of these files and their text.
    pub fn from_texts<I: IntoIterator<Item = (S, T)>, S: Into<String>, T: Into<String>>(
        files: I,
    ) -> Self {
        let texts: BTreeMap<String, Vec<u8>> = files
            .into_iter()
            .map(|(path, text)| (path.into(), text.into().into_bytes()))
            .collect();
        Self {
            paths: texts.keys().cloned().collect(),
            texts,
        }
    }

    /// The tree with the text of `path`, one of its files, as read.
    #[must_use]
    pub fn with_text(mut self, path: &str, text: impl Into<String>) -> Self {
        if self.paths.contains(path) {
            self.texts.insert(path.to_owned(), text.into().into_bytes());
        }
        self
    }

    /// The tree with the bytes of `path`, one of its files, as read.
    #[must_use]
    pub fn with_bytes(mut self, path: &str, bytes: Vec<u8>) -> Self {
        self.paths.insert(path.to_owned());
        self.texts.insert(path.to_owned(), bytes);
        self
    }

    /// Every path, in byte order.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.paths.iter().map(String::as_str)
    }

    /// Whether `path` is a file of the tree.
    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    /// The text of `path`, when it was read.
    pub fn text(&self, path: &str) -> Option<&str> {
        self.bytes(path)
            .and_then(|bytes| core::str::from_utf8(bytes).ok())
    }

    /// The bytes of `path`, when it was read.
    pub fn bytes(&self, path: &str) -> Option<&[u8]> {
        self.texts.get(path).map(Vec::as_slice)
    }

    /// The paths that make a path present: digest members, so no excluded name or `.pyc`.
    pub(crate) fn members(&self) -> impl Iterator<Item = &str> {
        self.paths().filter(|path| valid_relative_path(path))
    }

    fn has_member(&self, path: &str) -> bool {
        self.contains(path) && valid_relative_path(path)
    }
}

/// The keys of `defaults`.
pub const DEFAULT_KEYS: [&str; 10] = [
    "bin",
    "lib",
    "env",
    "env_scripts",
    "setup",
    "deps",
    "context",
    "skills",
    "rules",
    "hooks",
];

/// Default path additions as `(key, variable, directory)`, in the order they apply.
const DEFAULT_PATHS: [(&str, &str, &str); 3] = [
    ("bin", "PATH", "bin"),
    ("lib", "PYTHONPATH", "lib/python"),
    ("lib", "NODE_PATH", "lib/node"),
];

/// Default script extensions in the order a consumer tries them, with their interpreter and
/// the OSes each applies on (`None`: every OS).
pub(crate) const DEFAULT_SCRIPTS: [(&str, &str, Option<&str>); 7] = [
    (".py", "python", None),
    (".js", "node", None),
    (".mjs", "node", None),
    (".cjs", "node", None),
    (".ps1", "powershell", Some("windows")),
    (".bash", "bash", Some("unix")),
    (".sh", "sh", Some("unix")),
];

/// Every interpreter AOS names.
pub const INTERPRETERS: [&str; 5] = ["python", "node", "powershell", "bash", "sh"];

/// Whether the default `key` is on: true unless `defaults` writes it `false`.
pub fn default_on(manifest: &Document, key: &str) -> bool {
    !matches!(
        manifest
            .get("defaults")
            .and_then(|defaults| defaults.get(key)),
        Some(Data::Bool(false))
    )
}

/// The array `field` holds, or nothing.
pub(crate) fn declared<'a>(manifest: &'a Document, field: &str) -> &'a [Data] {
    manifest
        .get(field)
        .and_then(Data::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The file or directory an entry names: a string entry, or a mapping's `path`.
pub(crate) fn named(entry: &Data) -> Option<&str> {
    match entry {
        Data::String(path) => Some(path),
        Data::Object(map) => map.get("path").and_then(Data::as_str),
        _ => None,
    }
}

/// Default entries first, then declared ones; a declared entry naming the same file or
/// directory as a default one replaces it.
fn merge(defaults: Vec<Data>, declared: &[Data]) -> Vec<Data> {
    let names: BTreeSet<&str> = declared.iter().filter_map(named).collect();
    defaults
        .into_iter()
        .filter(|entry| named(entry).is_none_or(|name| !names.contains(name)))
        .chain(declared.iter().cloned())
        .collect()
}

/// An `os` list of an entry, when it has one.
pub(crate) fn os_list(entry: &Data) -> Option<Vec<String>> {
    entry.get("os").and_then(Data::as_array).map(|names| {
        names
            .iter()
            .filter_map(Data::as_str)
            .map(str::to_owned)
            .collect()
    })
}

/// Whether an entry applies on `os`.
pub(crate) fn entry_on(entry: &Data, os: &str) -> bool {
    os_matches(os_list(entry).as_deref(), os)
}

/// The first variant that applies on `os`.
pub(crate) fn first_variant<'a>(variants: &'a [Data], os: &str) -> Option<&'a Data> {
    variants.iter().find(|variant| entry_on(variant, os))
}

/// The default path additions that are on, present and not replaced by a declared entry, as
/// `{variable, path}` mappings.
pub fn default_paths(manifest: &Document, tree: &OverlayTree) -> Vec<Data> {
    let names: BTreeSet<&str> = declared(manifest, "paths")
        .iter()
        .filter_map(named)
        .collect();
    DEFAULT_PATHS
        .iter()
        .filter(|(key, _, directory)| {
            default_on(manifest, key)
                && !names.contains(directory)
                && tree.members().any(|path| {
                    path.strip_prefix(directory)
                        .is_some_and(|rest| rest.starts_with('/'))
                })
        })
        .map(
            |(_, variable, directory)| serde_json::json!({"variable": variable, "path": directory}),
        )
        .collect()
}

/// The `bin` and `lib` defaults, then `paths`.
pub fn paths_of(manifest: &Document, tree: &OverlayTree) -> Vec<Data> {
    let mut paths = default_paths(manifest, tree);
    paths.extend(declared(manifest, "paths").iter().cloned());
    paths
}

/// Every `rules/<name>.md` directly in `rules/` by path bytes, then `rules`.
pub fn rules_of(manifest: &Document, tree: &OverlayTree) -> Vec<String> {
    let mut found = Vec::new();
    if default_on(manifest, "rules") {
        found = tree
            .members()
            .filter(|path| {
                path.strip_prefix("rules/")
                    .is_some_and(|name| !name.contains('/') && name.ends_with(".md"))
            })
            .map(|path| Data::String(path.to_owned()))
            .collect();
    }
    strings(merge(found, declared(manifest, "rules")))
}

/// Whether the tree's `OVERLAY.md`, whose text was read, has a body: anything but white space
/// after its front matter.
fn has_body(tree: &OverlayTree) -> bool {
    tree.bytes("OVERLAY.md").is_some_and(|data| {
        let settings = Settings {
            manifest_bytes: Limit::UNBOUNDED,
            ..Settings::default()
        };
        let split = split_front_matter(data, &settings);
        split
            .body(data)
            .iter()
            .any(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    })
}

/// Whether the body of `OVERLAY.md` is the overlay's context: it has one, and `defaults` leaves
/// `context` on or `context` names `OVERLAY.md`.
fn body_is_context(manifest: &Document, tree: &OverlayTree) -> bool {
    has_body(tree)
        && (default_on(manifest, "context")
            || declared(manifest, "context")
                .iter()
                .any(|entry| entry.as_str() == Some("OVERLAY.md")))
}

/// The body of `OVERLAY.md`, else the root `CONTEXT.md`, then `context`; while a body is
/// context, `CONTEXT.md` is not, wherever it is named.
pub fn context_of(manifest: &Document, tree: &OverlayTree) -> Vec<String> {
    let mut found = Vec::new();
    if default_on(manifest, "context") {
        if has_body(tree) {
            found.push(Data::String("OVERLAY.md".to_owned()));
        } else if tree.has_member("CONTEXT.md") {
            found.push(Data::String("CONTEXT.md".to_owned()));
        }
    }
    let merged = strings(merge(found, declared(manifest, "context")));
    if body_is_context(manifest, tree) {
        return merged
            .into_iter()
            .filter(|entry| entry != "CONTEXT.md")
            .collect();
    }
    merged
}

/// The file a consumer reports as ignored because a body is the context instead.
pub fn context_ignored(manifest: &Document, tree: &OverlayTree) -> Vec<String> {
    if body_is_context(manifest, tree) && tree.has_member("CONTEXT.md") {
        vec!["CONTEXT.md".to_owned()]
    } else {
        Vec::new()
    }
}

/// One harness's `context`, without `CONTEXT.md` while a body is context.
pub fn harness_context_of(manifest: &Document, harness: &str, tree: &OverlayTree) -> Vec<String> {
    let listed: Vec<String> = manifest
        .get("harness")
        .and_then(|hints| hints.get(harness))
        .and_then(|hints| hints.get("context"))
        .and_then(Data::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if body_is_context(manifest, tree) {
        listed
            .into_iter()
            .filter(|entry| entry != "CONTEXT.md")
            .collect()
    } else {
        listed
    }
}

fn strings(entries: Vec<Data>) -> Vec<String> {
    entries
        .into_iter()
        .filter_map(|entry| entry.as_str().map(str::to_owned))
        .collect()
}

/// Where a skill an overlay offers comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillOrigin {
    /// A `skills/<name>/SKILL.md` in the overlay.
    Bundled,
    /// A table naming a source to fetch it from.
    Source,
    /// A bare name for the consumer's catalogs to resolve.
    Catalog,
}

/// A skill an overlay offers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkillEntry {
    /// The skill's name as written.
    pub name: String,
    /// Where it comes from.
    pub origin: SkillOrigin,
    /// The source as shown, without user information or a query; for `Source`.
    pub source: Option<String>,
    /// The source in full, as it is fetched; for `Source`.
    pub key: Option<String>,
    /// The tree digest it must have, when the table pins one.
    pub pin: Option<String>,
    /// The source to fetch it from, with the table's `ref`; for `Source`.
    pub from: Option<Source>,
}

/// The names with a `skills/<name>/SKILL.md` in the overlay.
pub fn bundled_skills(tree: &OverlayTree) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for path in tree.members() {
        let parts: Vec<&str> = path.split('/').collect();
        if let ["skills", name, "SKILL.md"] = parts.as_slice()
            && valid_tool_name(name)
        {
            found.insert((*name).to_owned());
        }
    }
    found
}

/// Every skill the overlay offers, by name bytes: bundled, from a source, or a name for the
/// consumer's catalog to resolve.
pub fn skill_entries(manifest: &Document, tree: &OverlayTree) -> Vec<SkillEntry> {
    let bundled = bundled_skills(tree);
    let mut found: BTreeMap<String, SkillEntry> = BTreeMap::new();
    let plain = |name: &str, origin| SkillEntry {
        name: name.to_owned(),
        origin,
        source: None,
        key: None,
        pin: None,
        from: None,
    };
    if default_on(manifest, "skills") {
        for name in &bundled {
            found.insert(name.clone(), plain(name, SkillOrigin::Bundled));
        }
    }
    for entry in declared(manifest, "skills") {
        match entry {
            Data::Object(table) => {
                let (Some(name), Some(source)) = (
                    table.get("name").and_then(Data::as_str),
                    table.get("source").and_then(Data::as_str),
                ) else {
                    continue;
                };
                let Ok(parsed) = Source::parse(source) else {
                    continue;
                };
                let reference = table.get("ref").and_then(Data::as_str);
                let mut from = parsed.clone();
                if let Some(reference) = reference {
                    from.reference = Some(reference.to_owned());
                }
                found.insert(
                    name.to_owned(),
                    SkillEntry {
                        name: name.to_owned(),
                        origin: SkillOrigin::Source,
                        source: Some(parsed.shown_with(reference.or(parsed.reference.as_deref()))),
                        key: Some(parsed.key_with(reference)),
                        pin: table.get("pin").and_then(Data::as_str).map(str::to_owned),
                        from: Some(from),
                    },
                );
            }
            Data::String(name) => {
                let origin = if bundled.contains(name) {
                    SkillOrigin::Bundled
                } else {
                    SkillOrigin::Catalog
                };
                found.insert(name.clone(), plain(name, origin));
            }
            _ => {}
        }
    }
    found.into_values().collect()
}

/// The names of [`skill_entries`].
pub fn skills_of(manifest: &Document, tree: &OverlayTree) -> Vec<String> {
    skill_entries(manifest, tree)
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

/// The root file `env` read as dotenv, then `env_files`.
pub fn env_files_of(manifest: &Document, tree: &OverlayTree) -> Vec<Data> {
    let mut found = Vec::new();
    if default_on(manifest, "env") && tree.has_member("env") {
        found.push(serde_json::json!({"path": "env", "format": "dotenv"}));
    }
    merge(found, declared(manifest, "env_files"))
}

/// The default `<stem>.<ext>` a consumer runs for `field` (`env_scripts` or `setup`).
pub(crate) struct DefaultScript {
    /// The chosen entry, its settings as declared or by default.
    pub chosen: Option<Data>,
    /// Whether a declared entry configured it.
    pub configured: bool,
    /// The candidate files present, reported when none is chosen.
    pub present: Vec<String>,
    /// The declared entries that name no candidate.
    pub others: Vec<Data>,
}

pub(crate) fn default_script(
    manifest: &Document,
    field: &str,
    stem: &str,
    tree: &OverlayTree,
    os: &str,
    interpreters: &[String],
) -> DefaultScript {
    let declared = declared(manifest, field);
    if !default_on(manifest, field) {
        return DefaultScript {
            chosen: None,
            configured: false,
            present: Vec::new(),
            others: declared.to_vec(),
        };
    }
    let present: Vec<String> = DEFAULT_SCRIPTS
        .iter()
        .map(|(extension, _, _)| format!("{stem}{extension}"))
        .filter(|path| tree.has_member(path))
        .collect();
    let configured: BTreeMap<&str, &Data> = declared
        .iter()
        .filter(|entry| entry.is_object())
        .filter_map(|entry| {
            named(entry)
                .filter(|path| present.iter().any(|p| p == path))
                .map(|path| (path, entry))
        })
        .collect();
    let others: Vec<Data> = declared
        .iter()
        .filter(|entry| named(entry).is_none_or(|path| !configured.contains_key(path)))
        .cloned()
        .collect();
    for (extension, interpreter, on) in DEFAULT_SCRIPTS {
        let path = format!("{stem}{extension}");
        if !present.contains(&path) {
            continue;
        }
        let entry = configured.get(path.as_str()).map_or_else(
            || {
                let mut entry = serde_json::json!({"interpreter": interpreter, "path": path});
                if let Some(on) = on {
                    entry["os"] = serde_json::json!([on]);
                }
                entry
            },
            |entry| (*entry).clone(),
        );
        let runs = entry
            .get("interpreter")
            .and_then(Data::as_str)
            .is_some_and(|interpreter| interpreters.iter().any(|known| known == interpreter));
        if entry_on(&entry, os) && runs {
            return DefaultScript {
                configured: configured.contains_key(path.as_str()),
                chosen: Some(entry),
                present,
                others,
            };
        }
    }
    DefaultScript {
        chosen: None,
        configured: false,
        present,
        others,
    }
}

/// The environment a runtime variant installs into: `shared`, `isolated` or a name.
pub(crate) fn environment_name(variant: Option<&Data>) -> String {
    normalize_name(
        variant
            .and_then(|variant| variant.get("environment"))
            .and_then(Data::as_str)
            .unwrap_or("shared"),
    )
}

/// A dependency file a consumer installs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Dependency {
    /// `python` or `node`.
    pub runtime: Runtime,
    /// The file, a relative path.
    pub file: String,
    /// The lock that pins it, when there is one: the file itself for a hashed requirements
    /// file.
    pub lock: Option<String>,
    /// `shared`, `isolated` or a name.
    pub environment: String,
}

/// A dependency file that is neither pinned nor unpinned: some requirements carry a hash and
/// some do not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialLock(pub String);

impl core::fmt::Display for PartialLock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "partial-lock: {}", self.0)
    }
}

impl core::error::Error for PartialLock {}

/// Whether a requirements file is a lock: every requirement in it carries `--hash`.
pub(crate) fn requirements_locked(text: &str, path: &str) -> Result<bool, PartialLock> {
    let joined = text.replace("\\\r\n", " ").replace("\\\n", " ");
    let hashed: Vec<bool> = joined
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            line.match_indices("--hash").any(|(at, _)| {
                let before = at == 0 || line[..at].ends_with(char::is_whitespace);
                let after = line[at + "--hash".len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c == '=' || c.is_whitespace());
                before && after
            })
        })
        .collect();
    if !hashed.is_empty() && hashed.iter().all(|hashed| *hashed) {
        return Ok(true);
    }
    if hashed.iter().any(|hashed| *hashed) {
        return Err(PartialLock(path.to_owned()));
    }
    Ok(false)
}

/// Why a requirements file is refused before pip reads it: it holds only requirement lines and
/// comments (`spec/v0/runtime.md#runtime`), since pip would otherwise follow `-r` and `-c`, change
/// the index or install a direct URL, and in a `locked` file every requirement is pinned with `==`.
///
/// # Errors
///
/// An [`Error`] of class `invalid-requirements` on the first line that is refused, with the reason
/// as its subject and the line's number; the line's text, which may hold a credential, is never
/// quoted.
pub fn check_requirements(text: &str, locked: bool) -> Result<(), Error> {
    let joined = text.replace("\\\r\n", " ").replace("\\\n", " ");
    for (number, line) in joined.lines().enumerate() {
        let refused = |why: &str| Error::new(Class::InvalidRequirements, why).on_line(number + 1);
        let line = line.split(" #").next().unwrap_or_default().trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (requirement, options) = line.split_once(" --").unwrap_or((line, ""));
        if requirement.starts_with('-') {
            return Err(refused("an option, and only requirements are read"));
        }
        if requirement.contains("://") || requirement.contains(" @ ") {
            return Err(refused("a direct URL"));
        }
        if !options.is_empty()
            && options
                .split(" --")
                .any(|option| !option.trim_start_matches('-').starts_with("hash"))
        {
            return Err(refused("an option other than --hash"));
        }
        if locked && !requirement.contains("==") {
            return Err(refused("a requirement not pinned with =="));
        }
    }
    Ok(())
}

/// The dependency files a consumer installs on `os`, one per runtime. A selected variant that
/// names a file replaces the default one; one that names none still chooses its environment.
///
/// # Errors
///
/// A [`PartialLock`] naming a requirements file only some of whose requirements are hashed.
pub fn dependencies_of(
    manifest: &Document,
    tree: &OverlayTree,
    os: &str,
) -> Result<Vec<Dependency>, PartialLock> {
    let runtime = manifest.get("runtime");
    let variants = |name: &str| {
        runtime
            .and_then(|runtime| runtime.get(name))
            .and_then(Data::as_array)
            .map_or(&[][..], Vec::as_slice)
    };
    let mut found = Vec::new();
    for (name, default, beside) in [
        ("python", "lib/python/requirements.txt", None),
        (
            "node",
            "lib/node/package.json",
            Some("lib/node/package-lock.json"),
        ),
    ] {
        let variant = first_variant(variants(name), os);
        let field = |key: &str| {
            variant
                .and_then(|variant| variant.get(key))
                .and_then(Data::as_str)
        };
        let (file, lock) = if name == "python"
            && (field("lock").is_some() || field("requirements").is_some())
        {
            let file = field("lock")
                .or_else(|| field("requirements"))
                .unwrap_or_default();
            (file.to_owned(), field("lock").map(str::to_owned))
        } else if name == "node" && field("package").is_some() {
            let file = field("package").unwrap_or_default();
            let lock = file
                .strip_suffix("package.json")
                .map(|stem| format!("{stem}package-lock.json"))
                .filter(|candidate| tree.has_member(candidate));
            (file.to_owned(), lock)
        } else if default_on(manifest, "deps") && tree.has_member(default) {
            let lock = match beside {
                None => match tree.text(default) {
                    Some(text) if requirements_locked(text, default)? => Some(default.to_owned()),
                    _ => None,
                },
                Some(beside) => tree.has_member(beside).then(|| beside.to_owned()),
            };
            (default.to_owned(), lock)
        } else {
            continue;
        };
        found.push(Dependency {
            runtime: Runtime::named(name).unwrap_or(Runtime::Python),
            file,
            lock,
            environment: environment_name(variant),
        });
    }
    Ok(found)
}
