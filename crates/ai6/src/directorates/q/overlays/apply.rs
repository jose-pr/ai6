//! Applying declarations: an overlay's environment in the order of application, its setup,
//! runtime, needs and weakenings (`spec/v0/environment.md`, `runtime.md`, `security.md`).
//! Pure: nothing runs and no file is read; a consumer passes in what it found.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde_json::Value as Data;

use super::defaults::{
    Dependency, INTERPRETERS, OverlayTree, declared, default_paths, default_script,
    dependencies_of, entry_on, env_files_of, first_variant, named,
};
use super::names::{
    is_path_variable, is_reserved_variable, is_sensitive_variable, normalize_name, root_variable,
    state_variable, valid_relative_path,
};
use super::read::{Document, ReadLimits, parse_dotenv, read_document};
use super::subst::{SubstitutionError, substitute};
use super::validate::validate;

/// A declaration a consumer refuses to apply, naming the variable or file, never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The AOS class: `undefined-variable`, `secret-placeholder`, `invalid-pattern`,
    /// `unresolved-reference`, `invalid-reference`, `missing-file`, `invalid-env-file`,
    /// `env-path-conflict`, `reserved-variable` or `partial-lock`.
    pub class: String,
    /// The variable, file or reference it is about.
    pub subject: String,
}

impl Refusal {
    fn new(class: &str, subject: &str) -> Self {
        Self {
            class: class.to_owned(),
            subject: subject.to_owned(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.class, self.subject)
    }
}

impl std::error::Error for Refusal {}

impl From<(SubstitutionError, &str)> for Refusal {
    fn from((error, variable): (SubstitutionError, &str)) -> Self {
        Self::new(error.class, variable)
    }
}

/// A weakening a consumer reports: its AOS identifier and its subject, never a value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Weakening {
    /// `unpinned`, `setup-network`, `env-script`, `secret-read`, and the others AOS lists.
    pub id: String,
    /// The variable, path, file or reference it is about; empty for `unpinned`.
    pub subject: String,
}

impl Weakening {
    fn new(id: &str, subject: &str) -> Self {
        Self {
            id: id.to_owned(),
            subject: subject.to_owned(),
        }
    }
}

/// An environment being built: names compare without regard to ASCII case on Windows, and a
/// variable whose value came from a secret reference is never read by a placeholder. Its
/// `Debug` shows names only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Environment {
    windows: bool,
    values: Vec<(String, String)>,
    secret: BTreeSet<String>,
    extra: Vec<(String, String)>,
}

impl fmt::Debug for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_set()
            .entries(self.values.iter().map(|(name, _)| name))
            .finish()
    }
}

impl Environment {
    /// An environment starting from `base`; `windows` makes names case-insensitive.
    pub fn new<I: IntoIterator<Item = (String, String)>>(base: I, windows: bool) -> Self {
        let mut environment = Self {
            windows,
            ..Self::default()
        };
        for (name, value) in base {
            environment.set(&name, value, false);
        }
        environment
    }

    fn key(&self, name: &str) -> Option<usize> {
        self.values
            .iter()
            .position(|(key, _)| key == name)
            .or_else(|| {
                self.windows.then(|| {
                    self.values
                        .iter()
                        .position(|(key, _)| key.eq_ignore_ascii_case(name))
                })?
            })
    }

    /// The value of `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.key(name).map(|index| self.values[index].1.as_str())
    }

    /// What a placeholder reads.
    fn lookup(&self, name: &str) -> Result<Option<String>, SubstitutionError> {
        if let Some((_, value)) = self
            .extra
            .iter()
            .find(|(extra, _)| extra == name || (self.windows && extra.eq_ignore_ascii_case(name)))
        {
            return Ok(Some(value.clone()));
        }
        let Some(index) = self.key(name) else {
            return Ok(None);
        };
        if self.secret.contains(&self.values[index].0) {
            return Err(SubstitutionError {
                class: "secret-placeholder",
                name: name.to_owned(),
            });
        }
        Ok(Some(self.values[index].1.clone()))
    }

    /// Sets `name`, keeping the name already there under another case on Windows.
    pub fn set(&mut self, name: &str, value: String, secret: bool) {
        let key = match self.key(name) {
            Some(index) => {
                self.values[index].1 = value;
                self.values[index].0.clone()
            }
            None => {
                self.values.push((name.to_owned(), value));
                name.to_owned()
            }
        };
        if secret {
            self.secret.insert(key);
        } else {
            self.secret.remove(&key);
        }
    }

    /// Removes `name`.
    pub fn unset(&mut self, name: &str) {
        if let Some(index) = self.key(name) {
            let (key, _) = self.values.remove(index);
            self.secret.remove(&key);
        }
    }

    /// The variables in the order they were first set.
    pub fn values(&self) -> &[(String, String)] {
        &self.values
    }

    /// Whether `name` holds a resolved secret, which must never be shown.
    pub fn is_secret(&self, name: &str) -> bool {
        self.key(name)
            .is_some_and(|index| self.secret.contains(&self.values[index].0))
    }

    fn substitute(&self, text: &str, variable: &str) -> Result<String, Refusal> {
        substitute(text, &|name| self.lookup(name))
            .map_err(|error| Refusal::from((error, variable)))
    }
}

/// What a consumer knows when it applies one overlay: where it is exposed, the files it has,
/// what is granted it, and its own policy.
#[derive(Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct Facts {
    /// The OS AOS names it (`linux`, `windows`, `macos`, ...).
    pub os: String,
    /// The overlay root as exposed, absolute.
    pub root: String,
    /// The identity the overlay is installed under.
    pub identity: String,
    /// The state directory, when the overlay has one.
    pub state: Option<String>,
    /// Whether a pin covers it; without one `unpinned` is reported.
    pub pinned: bool,
    /// The environment the consumer starts from.
    pub base_env: BTreeMap<String, String>,
    /// The consumer's own environment, which an `env` provider reference reads.
    pub host_env: BTreeMap<String, String>,
    /// What this overlay is granted, by provider (`env`, `file`, ...) or `secret`.
    pub grants: BTreeMap<String, Vec<String>>,
    /// The values of named secrets the consumer defines.
    pub secrets: BTreeMap<String, String>,
    /// The files of the consumer's secrets directory, by key; `None` when it has none.
    pub secret_files: Option<BTreeMap<String, String>>,
    /// The providers the consumer has; `None` is `env` and `file`.
    pub providers: Option<Vec<String>>,
    /// Further providers' values, by provider and key.
    pub provider_values: BTreeMap<String, BTreeMap<String, String>>,
    /// Whether the consumer can deny setup the network; `None` is yes.
    pub enforces_network: Option<bool>,
    /// Whether env scripts run; `None` is yes.
    pub env_scripts_enabled: Option<bool>,
    /// The interpreters the consumer can run; `None` is every one AOS names.
    pub interpreters: Option<Vec<String>>,
    /// Each runtime's installed packages directory, by runtime.
    pub packages: BTreeMap<String, String>,
    /// The overlay's files and the text of those read.
    #[serde(skip)]
    pub tree: OverlayTree,
    /// How much of an `aos` env file is read (`[overlays] manifest_bytes` and
    /// `manifest_depth` on a Station).
    #[serde(skip)]
    pub read_limits: ReadLimits,
}

impl fmt::Debug for Facts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Facts")
            .field("os", &self.os)
            .field("root", &self.root)
            .field("identity", &self.identity)
            .field("pinned", &self.pinned)
            .finish_non_exhaustive()
    }
}

impl Facts {
    fn windows(&self) -> bool {
        self.os == "windows"
    }

    fn interpreters(&self) -> Vec<String> {
        self.interpreters
            .clone()
            .unwrap_or_else(|| INTERPRETERS.iter().map(|name| (*name).to_owned()).collect())
    }

    fn granted(&self, entry: &Document) -> bool {
        let empty = Vec::new();
        if let Some(secret) = entry.get("secret").and_then(Data::as_str) {
            return self
                .grants
                .get("secret")
                .unwrap_or(&empty)
                .iter()
                .any(|name| name == secret);
        }
        let provider = entry
            .get("provider")
            .and_then(Data::as_str)
            .unwrap_or_default();
        let key = entry.get("key").and_then(Data::as_str).unwrap_or_default();
        self.grants
            .get(provider)
            .unwrap_or(&empty)
            .iter()
            .any(|name| name == key)
    }

    /// A granted reference's value, or `None` when it cannot be resolved.
    fn secret(&self, entry: &Document) -> Result<Option<String>, Refusal> {
        let provider = entry.get("provider").and_then(Data::as_str);
        let key = entry.get("key").and_then(Data::as_str).unwrap_or_default();
        if provider == Some("file") && !valid_relative_path(key) {
            return Err(Refusal::new("invalid-reference", key));
        }
        if !self.granted(entry) {
            return Ok(None);
        }
        if let Some(name) = entry.get("secret").and_then(Data::as_str) {
            return Ok(self.secrets.get(name).cloned());
        }
        let provider = provider.unwrap_or_default();
        let has = self
            .providers
            .as_ref()
            .map_or(matches!(provider, "env" | "file"), |providers| {
                providers.iter().any(|known| known == provider)
            });
        if !has {
            return Ok(None);
        }
        Ok(match provider {
            "env" => Environment::new(self.host_env.clone(), self.windows())
                .get(key)
                .map(str::to_owned),
            "file" => self
                .secret_files
                .as_ref()
                .and_then(|files| files.get(key))
                .map(|value| {
                    value
                        .strip_suffix("\r\n")
                        .or_else(|| value.strip_suffix('\n'))
                        .unwrap_or(value)
                        .to_owned()
                }),
            other => self
                .provider_values
                .get(other)
                .and_then(|values| values.get(key))
                .cloned(),
        })
    }
}

/// How a report names a reference: `secret:<name>` or `<provider>:<key>`, never a value.
fn reference_name(entry: &Document) -> String {
    match entry.get("secret").and_then(Data::as_str) {
        Some(name) => format!("secret:{name}"),
        None => format!(
            "{}:{}",
            entry
                .get("provider")
                .and_then(Data::as_str)
                .unwrap_or_default(),
            entry.get("key").and_then(Data::as_str).unwrap_or_default()
        ),
    }
}

/// The absolute path of `relative` under `root`, with the platform's separator.
pub(crate) fn resolve_path(root: &str, relative: &str, windows: bool) -> String {
    let separator = if windows { "\\" } else { "/" };
    format!(
        "{}{separator}{}",
        root.trim_end_matches(['/', '\\']),
        relative.replace('/', separator)
    )
}

fn is_reference(entry: &Data) -> bool {
    entry.get("provider").is_some() || entry.get("secret").is_some()
}

fn apply_mapping(
    env: &mut Environment,
    mapping: &Document,
    facts: &Facts,
    root: &str,
    weakenings: &mut Vec<Weakening>,
) -> Result<(), Refusal> {
    let windows = env.windows;
    let before = env.clone();
    for (name, entry) in mapping {
        if is_reserved_variable(name) {
            return Err(Refusal::new("reserved-variable", name));
        }
        let entry = match entry {
            Data::String(text) => {
                let mut map = Document::new();
                map.insert("value".to_owned(), Data::String(text.clone()));
                map
            }
            Data::Object(map) => map.clone(),
            _ => continue,
        };
        if entry.get("if_unset") == Some(&Data::Bool(true)) && before.get(name).is_some() {
            continue;
        }
        let mut secret = false;
        let value = if let Some(text) = entry.get("value").and_then(Data::as_str) {
            // Mapping order carries no meaning, so no entry sees another of the same mapping.
            before.substitute(text, name)?
        } else if let Some(path) = entry.get("path").and_then(Data::as_str) {
            resolve_path(root, path, windows)
        } else {
            let Some(value) = facts.secret(&entry)? else {
                if entry.get("optional") == Some(&Data::Bool(true)) {
                    continue;
                }
                return Err(Refusal::new("unresolved-reference", name));
            };
            secret = true;
            if entry.get("provider").and_then(Data::as_str) == Some("env") {
                let key = entry.get("key").and_then(Data::as_str).unwrap_or_default();
                weakenings.push(Weakening::new("env-passthrough", key));
            } else {
                weakenings.push(Weakening::new("secret-read", &reference_name(&entry)));
            }
            value
        };
        if is_sensitive_variable(name) {
            weakenings.push(Weakening::new("env-sensitive", name));
        }
        env.set(name, value, secret);
    }
    Ok(())
}

fn apply_paths(
    env: &mut Environment,
    paths: &[Data],
    root: &str,
    os: &str,
    weakenings: &mut Vec<Weakening>,
) {
    let windows = env.windows;
    let separator = if windows { ';' } else { ':' };
    let mut order: Vec<String> = Vec::new();
    let mut grouped: BTreeMap<String, (String, Vec<&Data>)> = BTreeMap::new();
    for entry in paths {
        if !entry_on(entry, os) {
            continue;
        }
        let variable = entry
            .get("variable")
            .and_then(Data::as_str)
            .unwrap_or_default();
        let key = if windows {
            variable.to_ascii_uppercase()
        } else {
            variable.to_owned()
        };
        if !grouped.contains_key(&key) {
            order.push(key.clone());
        }
        grouped
            .entry(key)
            .or_insert_with(|| (variable.to_owned(), Vec::new()))
            .1
            .push(entry);
    }
    for key in order {
        let (variable, entries) = &grouped[&key];
        if is_sensitive_variable(variable) && !is_path_variable(variable) {
            weakenings.push(Weakening::new("env-sensitive", variable));
        }
        let (mut prepend, mut append, mut seen) = (Vec::new(), Vec::new(), BTreeSet::new());
        for entry in entries {
            let resolved = match entry.get("packages").and_then(Data::as_str) {
                Some(packages) => packages.to_owned(),
                None => resolve_path(
                    root,
                    entry.get("path").and_then(Data::as_str).unwrap_or_default(),
                    windows,
                ),
            };
            if !seen.insert(resolved.clone()) {
                continue;
            }
            if entry.get("position").and_then(Data::as_str) == Some("append") {
                append.push(resolved);
            } else {
                prepend.push(resolved);
            }
        }
        let current = env.get(variable).unwrap_or_default().to_owned();
        let kept = current
            .split(separator)
            .filter(|component| !component.is_empty() && !seen.contains(*component))
            .map(str::to_owned);
        let joined: Vec<String> = prepend.into_iter().chain(kept).chain(append).collect();
        let secret = env.is_secret(variable);
        env.set(variable, joined.join(&separator.to_string()), secret);
    }
}

/// An env script a consumer runs, with its settings.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ScriptRun {
    /// The interpreter name.
    pub interpreter: String,
    /// The script, a relative path.
    pub path: String,
    /// Arguments after the script.
    pub args: Vec<String>,
    /// Seconds before it is stopped; 0 for no limit.
    pub timeout: u64,
}

/// The setup variant a consumer runs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SetupRun {
    /// The interpreter name.
    pub interpreter: String,
    /// The script, a relative path.
    pub path: String,
    /// Arguments after the phase.
    pub args: Vec<String>,
    /// Seconds before it is stopped; 0 for no limit.
    pub timeout: u64,
    /// Whether it gets the network.
    pub network: bool,
}

fn strings_of(entry: &Data, key: &str) -> Vec<String> {
    entry
        .get(key)
        .and_then(Data::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Data::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn text_of(entry: &Data, key: &str) -> String {
    entry
        .get(key)
        .and_then(Data::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Steps 2.1 to 2.4 of the order of application for one overlay; returns the env scripts
/// that run.
fn apply_overlay(
    env: &mut Environment,
    manifest: &Document,
    facts: &Facts,
    weakenings: &mut Vec<Weakening>,
) -> Result<Vec<ScriptRun>, Refusal> {
    env.extra = vec![
        ("AOS_OVERLAY_ROOT".to_owned(), facts.root.clone()),
        (
            "AOS_OVERLAY_NAME".to_owned(),
            normalize_name(&facts.identity),
        ),
    ];
    if let Some(state) = &facts.state {
        env.extra
            .push(("AOS_OVERLAY_STATE".to_owned(), state.clone()));
    }
    let result = apply_overlay_inner(env, manifest, facts, weakenings);
    env.extra.clear();
    result
}

fn apply_overlay_inner(
    env: &mut Environment,
    manifest: &Document,
    facts: &Facts,
    weakenings: &mut Vec<Weakening>,
) -> Result<Vec<ScriptRun>, Refusal> {
    let (os, root, tree) = (facts.os.as_str(), facts.root.as_str(), &facts.tree);
    let mut paths = default_paths(manifest, tree);
    let dependencies = dependencies_of(manifest, tree, os)
        .map_err(|lock| Refusal::new("partial-lock", &lock.0))?;
    for dependency in &dependencies {
        if let Some(packages) = facts.packages.get(&dependency.runtime) {
            let variable = if dependency.runtime == "python" {
                "PYTHONPATH"
            } else {
                "NODE_PATH"
            };
            paths.push(serde_json::json!({"variable": variable, "packages": packages}));
        }
    }
    paths.extend(declared(manifest, "paths").iter().cloned());
    let listed: BTreeSet<String> = paths
        .iter()
        .filter_map(|entry| entry.get("variable").and_then(Data::as_str))
        .map(str::to_ascii_uppercase)
        .collect();
    let empty = Document::new();
    let own_env = manifest
        .get("env")
        .and_then(Data::as_object)
        .unwrap_or(&empty);
    if let Some(name) = own_env
        .keys()
        .find(|name| listed.contains(&name.to_ascii_uppercase()))
    {
        return Err(Refusal::new("env-path-conflict", name));
    }
    for item in env_files_of(manifest, tree) {
        if !entry_on(&item, os) {
            continue;
        }
        let path = named(&item).unwrap_or_default().to_owned();
        let dotenv = item.get("format").and_then(Data::as_str) == Some("dotenv");
        let Some(text) = tree.text(&path) else {
            return Err(Refusal::new("missing-file", &path));
        };
        let invalid = || Refusal::new("invalid-env-file", &path);
        if dotenv {
            let lines = parse_dotenv(text.as_bytes()).map_err(|_| invalid())?;
            if let Some(line) = lines
                .iter()
                .find(|line| listed.contains(&line.name.to_ascii_uppercase()))
            {
                return Err(Refusal::new("env-path-conflict", &line.name));
            }
            for line in lines {
                if is_reserved_variable(&line.name) {
                    return Err(Refusal::new("reserved-variable", &line.name));
                }
                let value = if line.literal {
                    line.text
                } else {
                    env.substitute(&line.text, &line.name)?
                };
                if is_sensitive_variable(&line.name) {
                    weakenings.push(Weakening::new("env-sensitive", &line.name));
                }
                env.set(&line.name, value, false);
            }
        } else {
            let mapping =
                read_document(text.as_bytes(), &facts.read_limits).map_err(|_| invalid())?;
            let mut wrapped = Document::new();
            wrapped.insert("env".to_owned(), Data::Object(mapping.clone()));
            if !validate(&wrapped, None).is_empty() {
                return Err(invalid());
            }
            if let Some(name) = mapping
                .keys()
                .find(|name| listed.contains(&name.to_ascii_uppercase()))
            {
                return Err(Refusal::new("env-path-conflict", name));
            }
            apply_mapping(env, &mapping, facts, root, weakenings)?;
        }
    }
    apply_mapping(env, own_env, facts, root, weakenings)?;
    apply_paths(env, &paths, root, os, weakenings);
    let mut scripts = Vec::new();
    if facts.env_scripts_enabled != Some(false) {
        let choice = default_script(
            manifest,
            "env_scripts",
            "env",
            tree,
            os,
            &facts.interpreters(),
        );
        if choice.chosen.is_none() {
            weakenings.extend(
                choice
                    .present
                    .iter()
                    .map(|path| Weakening::new("script-not-run", path)),
            );
        }
        for script in choice.chosen.iter().chain(&choice.others) {
            if !entry_on(script, os) {
                continue;
            }
            let run = ScriptRun {
                interpreter: text_of(script, "interpreter"),
                path: text_of(script, "path"),
                args: strings_of(script, "args"),
                timeout: script.get("timeout").and_then(Data::as_u64).unwrap_or(60),
            };
            weakenings.push(Weakening::new("env-script", &run.path));
            if run.timeout == 0 {
                weakenings.push(Weakening::new("no-timeout", &run.path));
            }
            scripts.push(run);
        }
    }
    Ok(scripts)
}

/// Applies a `dotenv` file to `env` line by line, each line substituted against the
/// environment the earlier ones left, and returns what it set, in order.
///
/// # Errors
///
/// A [`ReadError`](super::ReadError) for a file that does not read, or whose substitution
/// fails (`undefined-variable`, `secret-placeholder`, `invalid-pattern`, naming the variable).
pub fn read_dotenv(
    data: &[u8],
    env: &mut Environment,
) -> Result<Vec<(String, String)>, super::ReadError> {
    let mut set = Vec::new();
    for line in parse_dotenv(data)? {
        let value = if line.literal {
            line.text
        } else {
            env.substitute(&line.text, &line.name)
                .map_err(|refusal| super::ReadError::new(&refusal.class, refusal.subject))?
        };
        env.set(&line.name, value.clone(), false);
        set.push((line.name, value));
    }
    Ok(set)
}

/// What a target must have for an overlay on its OS.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Needs {
    /// Interpreters, sorted.
    pub interpreters: Vec<String>,
    /// Commands found on the base environment's `PATH`, sorted.
    pub commands: Vec<String>,
}

/// What a consumer applies for one overlay on one OS.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    /// The references that were not granted, as `secret:<name>` or `<provider>:<key>`.
    pub ungranted: Vec<String>,
    /// Whether `runtime.os` admits the OS.
    pub supported: bool,
    /// The environment of the overlay's own processes.
    pub env: Environment,
    /// The env scripts that run, in order.
    pub env_scripts: Vec<ScriptRun>,
    /// The setup variant that runs, if any.
    pub setup: Option<SetupRun>,
    /// The selected Python variant, its `os` left out.
    pub python: Option<Document>,
    /// The selected Node.js variant, its `os` left out.
    pub node: Option<Document>,
    /// The dependency files installed.
    pub dependencies: Vec<Dependency>,
    /// What a target needs.
    pub needs: Needs,
    /// What is reported, sorted, each once.
    pub weakenings: Vec<Weakening>,
}

/// What a consumer applies for one overlay: its environment, setup, runtime, needs and
/// weakenings, from its manifest and the consumer's facts.
///
/// # Errors
///
/// A [`Refusal`] when the overlay's environment cannot be applied.
pub fn effective(manifest: &Document, facts: &Facts) -> Result<Effective, Refusal> {
    let os = facts.os.as_str();
    let mut weakenings = Vec::new();
    if !facts.pinned {
        weakenings.push(Weakening::new("unpinned", ""));
    }
    let runtime = manifest.get("runtime").cloned().unwrap_or(Data::Null);
    let supported = runtime.get("os").is_none() || entry_on(&runtime, os);
    let mut env = Environment::new(facts.base_env.clone(), facts.windows());
    env.set(&root_variable(&facts.identity), facts.root.clone(), false);
    if let Some(state) = &facts.state {
        env.set(&state_variable(&facts.identity), state.clone(), false);
    }
    let scripts = apply_overlay(&mut env, manifest, facts, &mut weakenings)?;
    let choice = default_script(
        manifest,
        "setup",
        "setup",
        &facts.tree,
        os,
        &facts.interpreters(),
    );
    // Declared variants come first; the default setup is the last one.
    let mut network = false;
    let mut setup = first_variant(&choice.others, os).cloned();
    if setup.is_none() {
        setup = choice.chosen.clone();
        // A default setup that declares nothing is taken to need the network.
        network = !choice.configured;
        if setup.is_none() {
            weakenings.extend(
                choice
                    .present
                    .iter()
                    .map(|path| Weakening::new("script-not-run", path)),
            );
        }
    }
    let setup = setup.map(|variant| SetupRun {
        interpreter: text_of(&variant, "interpreter"),
        path: text_of(&variant, "path"),
        args: strings_of(&variant, "args"),
        timeout: variant.get("timeout").and_then(Data::as_u64).unwrap_or(300),
        network: variant
            .get("network")
            .and_then(Data::as_bool)
            .unwrap_or(network),
    });
    if let Some(setup) = &setup {
        if setup.network {
            weakenings.push(Weakening::new("setup-network", &setup.path));
        } else if facts.enforces_network == Some(false) {
            weakenings.push(Weakening::new("network-not-enforced", &setup.path));
        }
        if setup.timeout == 0 {
            weakenings.push(Weakening::new("no-timeout", &setup.path));
        }
    }
    let variant = |name: &str| {
        runtime
            .get(name)
            .and_then(Data::as_array)
            .and_then(|variants| first_variant(variants, os))
            .and_then(Data::as_object)
            .map(|variant| {
                let mut variant = variant.clone();
                variant.remove("os");
                variant
            })
    };
    let python = variant("python");
    let dependencies = dependencies_of(manifest, &facts.tree, os)
        .map_err(|lock| Refusal::new("partial-lock", &lock.0))?;
    for dependency in &dependencies {
        // Installing fetches packages: it needs the network, locked or not.
        weakenings.push(Weakening::new("deps-network", &dependency.file));
        if dependency.lock.is_none() {
            weakenings.push(Weakening::new("unlocked-dependencies", &dependency.file));
        }
    }
    let node = variant("node");
    let mut interpreters: BTreeSet<String> = scripts
        .iter()
        .map(|script| script.interpreter.clone())
        .collect();
    if let Some(setup) = &setup {
        interpreters.insert(setup.interpreter.clone());
    }
    if let Some(scripts) = manifest.get("scripts").and_then(Data::as_object) {
        for target in scripts.values() {
            match target {
                Data::String(path) => {
                    let extension = path.rfind('.').map_or("", |at| &path[at..]);
                    let interpreter = match extension {
                        ".py" => "python",
                        ".sh" => "sh",
                        ".bash" => "bash",
                        ".ps1" => "powershell",
                        _ => "node",
                    };
                    interpreters.insert(interpreter.to_owned());
                }
                Data::Object(_) if entry_on(target, os) => {
                    interpreters.insert(text_of(target, "interpreter"));
                }
                _ => {}
            }
        }
    }
    if python.is_some() {
        interpreters.insert("python".to_owned());
    }
    if node.is_some() {
        interpreters.insert("node".to_owned());
    }
    let mut commands = BTreeSet::new();
    for command in runtime
        .get("commands")
        .and_then(Data::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        match command {
            Data::String(name) => {
                commands.insert(name.clone());
            }
            _ if entry_on(command, os) => {
                commands.insert(text_of(command, "name"));
            }
            _ => {}
        }
    }
    for dependency in &dependencies {
        interpreters.insert(dependency.runtime.clone());
        if dependency.runtime == "node" {
            commands.insert("npm".to_owned());
        }
    }
    let mut ungranted = BTreeSet::new();
    let host_grants = facts.grants.get("env").cloned().unwrap_or_default();
    let servers = manifest
        .get("mcp")
        .and_then(|mcp| mcp.get("servers"))
        .and_then(Data::as_array)
        .map_or(&[][..], Vec::as_slice);
    for server in servers {
        if let Some(server_env) = server.get("env").and_then(Data::as_object) {
            for value in server_env.values() {
                let Some(reference) = value.as_object().filter(|_| is_reference(value)) else {
                    continue;
                };
                if !facts.granted(reference) {
                    ungranted.insert(reference_name(reference));
                } else if reference.get("provider").and_then(Data::as_str) == Some("env") {
                    weakenings.push(Weakening::new("env-passthrough", &text_of(value, "key")));
                } else {
                    weakenings.push(Weakening::new("secret-read", &reference_name(reference)));
                }
            }
        }
        let headers = server
            .get("headers_env")
            .and_then(Data::as_object)
            .map(|headers| {
                headers
                    .values()
                    .filter_map(Data::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_else(Vec::new);
        for name in strings_of(server, "env_passthrough")
            .into_iter()
            .chain(headers)
        {
            if host_grants.contains(&name) {
                weakenings.push(Weakening::new("env-passthrough", &name));
            } else {
                ungranted.insert(format!("env:{name}"));
            }
        }
        let url = text_of(server, "url");
        if url.len() >= 7 && url[..7].eq_ignore_ascii_case("http://") {
            let authority = url[7..].split('/').next().unwrap_or_default();
            let authority = authority.split('?').next().unwrap_or_default();
            let host = authority
                .rsplit_once(':')
                .map_or(authority, |(host, _)| host)
                .to_ascii_lowercase();
            if host != "localhost" && host != "[::1]" && !host.starts_with("127.") {
                weakenings.push(Weakening::new(
                    "insecure-transport",
                    &text_of(server, "name"),
                ));
            }
        }
    }
    for entry in own_env(manifest).values() {
        if let Some(reference) = entry.as_object().filter(|_| is_reference(entry))
            && reference.get("optional") == Some(&Data::Bool(true))
            && !facts.granted(reference)
        {
            ungranted.insert(reference_name(reference));
        }
    }
    let weakenings: BTreeSet<Weakening> = weakenings.into_iter().collect();
    Ok(Effective {
        ungranted: ungranted.into_iter().collect(),
        supported,
        env,
        env_scripts: scripts,
        setup,
        python,
        node,
        dependencies,
        needs: Needs {
            interpreters: interpreters.into_iter().collect(),
            commands: commands.into_iter().collect(),
        },
        weakenings: weakenings.into_iter().collect(),
    })
}

fn own_env(manifest: &Document) -> Document {
    manifest
        .get("env")
        .and_then(Data::as_object)
        .cloned()
        .unwrap_or_default()
}

/// What a consumer is doing to an overlay when it decides whether setup runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    /// Adding it.
    Add,
    /// A sync: a fetch of its source, a change of ref or pin, or setup run again on request.
    Sync,
    /// Activating a digest outside a sync.
    Activate,
    /// Removing it.
    Remove,
}

/// A setup phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// The first preparation of an identity on an OS.
    Install,
    /// Every later preparation.
    Sync,
    /// Removal.
    Remove,
}

impl Phase {
    /// `install`, `sync` or `remove`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Sync => "sync",
            Self::Remove => "remove",
        }
    }
}

/// The last preparation that succeeded for an overlay identity on one OS (per Theater, for a
/// Station): its digest and phase.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SetupRecord {
    /// The digest prepared, `sha256:<hex>`.
    pub digest: String,
    /// The phase that prepared it.
    pub phase: Phase,
}

/// The phase a change runs, or `None` when nothing runs.
pub fn setup_phase(change: Change, recorded: Option<&str>, digest: Option<&str>) -> Option<Phase> {
    match (change, recorded) {
        (Change::Remove, Some(_)) => Some(Phase::Remove),
        (Change::Remove, None) => None,
        (_, None) => Some(Phase::Install),
        (Change::Activate, Some(recorded)) if Some(recorded) == digest => None,
        _ => Some(Phase::Sync),
    }
}

/// What a preparation does: its phase, the steps in order, the record afterwards and whether
/// the digest activates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preparation {
    /// The phase, or `None` when nothing runs.
    pub phase: Option<Phase>,
    /// Setup's arguments: the phase, then the variant's own.
    pub arguments: Option<Vec<String>>,
    /// `empty-state`, `deps:<runtime>`, `setup`, `record` and `delete`, in order.
    pub steps: Vec<String>,
    /// The record after the steps.
    pub record: Option<SetupRecord>,
    /// Whether the digest may be activated.
    pub activates: bool,
}

/// The preparation of `digest` for `change`, given the `record` kept for the overlay and the
/// step that `fails` (`deps` or `setup`), if one does.
pub fn preparation(
    change: Change,
    record: Option<&SetupRecord>,
    digest: Option<&str>,
    setup: Option<&SetupRun>,
    dependencies: &[Dependency],
    fails: Option<&str>,
) -> Preparation {
    let recorded = record.map(|record| record.digest.as_str());
    let phase = if setup.is_some() || !dependencies.is_empty() {
        setup_phase(change, recorded, digest)
    } else {
        None
    };
    let mut steps = Vec::new();
    if phase == Some(Phase::Install) {
        steps.push("empty-state".to_owned());
    }
    if matches!(phase, Some(Phase::Install | Phase::Sync)) {
        steps.extend(
            dependencies
                .iter()
                .map(|dependency| format!("deps:{}", dependency.runtime)),
        );
    }
    if phase.is_some() && setup.is_some() {
        steps.push("setup".to_owned());
    }
    let mut failed = false;
    if let Some(fails) = fails
        && let Some(index) = steps
            .iter()
            .position(|step| step.split(':').next() == Some(fails))
    {
        steps.truncate(index + 1);
        failed = true;
    }
    let (after, activates) = if change == Change::Remove {
        steps.push("delete".to_owned());
        (None, false)
    } else if phase.is_none() {
        (record.cloned(), true)
    } else if failed {
        // The record still names the last digest prepared; only that one stays usable.
        (record.cloned(), recorded.is_some() && recorded == digest)
    } else {
        steps.push("record".to_owned());
        (
            Some(SetupRecord {
                digest: digest.unwrap_or_default().to_owned(),
                phase: phase.unwrap_or(Phase::Install),
            }),
            true,
        )
    };
    Preparation {
        arguments: phase.and_then(|phase| {
            setup.map(|setup| {
                std::iter::once(phase.name().to_owned())
                    .chain(setup.args.iter().cloned())
                    .collect()
            })
        }),
        phase,
        steps,
        record: after,
        activates,
    }
}

/// The environment of every active overlay, in composition order, from `base` and each
/// overlay's root: what an agent Session gets.
///
/// # Errors
///
/// The first [`Refusal`] of any overlay.
pub fn composed_environment(
    active: &[(&Document, Facts)],
    base: Environment,
    weakenings: &mut Vec<Weakening>,
) -> Result<Environment, Refusal> {
    let mut env = base;
    for (_, facts) in active {
        env.set(&root_variable(&facts.identity), facts.root.clone(), false);
        if let Some(state) = &facts.state {
            env.set(&state_variable(&facts.identity), state.clone(), false);
        }
    }
    for (manifest, facts) in active {
        apply_overlay(&mut env, manifest, facts, weakenings)?;
    }
    Ok(env)
}
