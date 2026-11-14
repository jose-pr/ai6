//! Hooks (`spec/v0/hooks.md`): which hooks an overlay has, whether they are on, what each event's
//! hook is told, and what of its output applies. Running a hook is the caller's job: the core
//! reads the hooks, decides, and combines the results of runs the caller made.

use alloc::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::codecs;
use crate::defaults::{DEFAULT_SCRIPTS, INTERPRETERS, OverlayTree, default_on};
use crate::digest::{Digest, TreeEntry, digest_entries};
use crate::limits::Settings;
use crate::names::{is_reserved_variable, is_sensitive_variable, is_variable_name, os_matches};
use crate::read::{Document, read_env_script_output};

/// An event a hook can be bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum HookEvent {
    /// Before an overlay's environment is built.
    PreEnv,
    /// A session starts.
    SessionStart,
    /// A session ends.
    SessionEnd,
    /// A prompt is sent.
    Prompt,
    /// A turn ends.
    TurnEnd,
    /// The working directory changes.
    Cd,
    /// Before the conversation is compacted.
    PreCompact,
    /// Before a tool runs.
    PreTool,
    /// After a tool ran.
    PostTool,
}

impl HookEvent {
    /// Every event, in the order the specification lists them.
    pub const ALL: [Self; 9] = [
        Self::PreEnv,
        Self::SessionStart,
        Self::SessionEnd,
        Self::Prompt,
        Self::TurnEnd,
        Self::Cd,
        Self::PreCompact,
        Self::PreTool,
        Self::PostTool,
    ];

    /// The event's name in a manifest.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreEnv => "pre-env",
            Self::SessionStart => "session-start",
            Self::SessionEnd => "session-end",
            Self::Prompt => "prompt",
            Self::TurnEnd => "turn-end",
            Self::Cd => "cd",
            Self::PreCompact => "pre-compact",
            Self::PreTool => "pre-tool",
            Self::PostTool => "post-tool",
        }
    }

    /// The event a name in a manifest stands for.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.as_str() == name)
    }

    /// Whether `hook.<ext>` is the default of this event.
    const fn by_default(self) -> bool {
        !matches!(self, Self::Prompt | Self::PreTool | Self::PostTool)
    }

    /// The outputs the event takes.
    const fn outputs(self) -> &'static [&'static str] {
        match self {
            Self::PreEnv | Self::Cd => &["env"],
            Self::SessionStart => &["context", "env"],
            Self::SessionEnd | Self::TurnEnd => &[],
            Self::Prompt => &["context", "block", "env"],
            Self::PreCompact => &["context", "block"],
            Self::PreTool => &["context", "deny", "ask", "allow", "input", "env"],
            Self::PostTool => &["context"],
        }
    }

    /// What a refusal is for the event; none when it cannot be refused.
    const fn refusal(self) -> Option<Decision> {
        match self {
            Self::PreEnv | Self::SessionStart => Some(Decision::Refuse),
            Self::Prompt | Self::PreCompact => Some(Decision::Block),
            Self::PreTool => Some(Decision::Deny),
            _ => None,
        }
    }

    /// What `match` selects: the session's reason, or the tool's kind.
    const fn selects(self) -> Option<&'static str> {
        match self {
            Self::SessionStart => Some("reason"),
            Self::PreTool | Self::PostTool => Some("kind"),
            _ => None,
        }
    }

    /// The keys of the payload a hook of this event is told.
    #[must_use]
    pub const fn payload_keys(self) -> &'static [&'static str] {
        match self {
            Self::PreEnv => &["event", "harness", "cwd"],
            Self::SessionStart => &["event", "harness", "cwd", "session", "reason"],
            Self::SessionEnd | Self::TurnEnd => &["event", "harness", "cwd", "session"],
            Self::Prompt => &["event", "harness", "cwd", "session", "prompt"],
            Self::Cd => &["event", "harness", "cwd", "session", "previous"],
            Self::PreCompact => &["event", "harness", "cwd", "session", "trigger"],
            Self::PreTool => &["event", "harness", "cwd", "session", "tool"],
            Self::PostTool => &["event", "harness", "cwd", "session", "tool", "status"],
        }
    }
}

/// What a hook may ask of its event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Decision {
    /// The prompt or compaction does not happen.
    Block,
    /// The tool call does not happen.
    Deny,
    /// The user is asked.
    Ask,
    /// The tool call is approved, as rewritten when the output carries an input.
    Allow,
    /// The session does not start.
    Refuse,
}

impl Decision {
    const fn rank(decision: Option<Self>) -> u8 {
        match decision {
            None => 0,
            Some(Self::Allow) => 1,
            Some(Self::Ask) => 2,
            Some(_) => 3,
        }
    }
}

/// What a failure of a hook does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OnFailure {
    /// The failure is reported and the event goes on.
    Report,
    /// The failure refuses the event, where the event can be refused.
    Refuse,
}

/// A hook an overlay has.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Hook {
    /// The event it is bound to.
    pub event: HookEvent,
    /// The script; none for a `hook.<ext>` none of which can run here.
    pub path: Option<String>,
    /// The interpreter that follows from the script's extension.
    pub interpreter: Option<String>,
    /// Whether the interpreter is one the consumer has.
    pub runs: bool,
    /// The reasons or tool kinds it runs for; none for every one.
    pub select: Option<Vec<String>>,
    /// The variables its `env` output may set. A `pre-env` hook's output is an env script's, whose
    /// names are bound by what an env script may set, not by this list.
    pub sets: Vec<String>,
    /// Seconds before it is stopped.
    pub timeout: u64,
    /// What its failure does.
    pub on_failure: OnFailure,
    /// The keys of the payload it is told.
    pub payload: Vec<&'static str>,
}

fn extension_interpreter(path: &str) -> Option<&'static str> {
    let extension = path.rfind('.').map(|at| &path[at..])?;
    match extension {
        ".py" => Some("python"),
        ".sh" => Some("sh"),
        ".bash" => Some("bash"),
        ".ps1" => Some("powershell"),
        ".js" | ".mjs" | ".cjs" => Some("node"),
        _ => None,
    }
}

/// The `hook.<ext>` chosen by runtime, and every one present.
fn default_hook(
    tree: &OverlayTree,
    os: &str,
    interpreters: &[String],
) -> (Option<(String, &'static str)>, Vec<String>) {
    let present: Vec<String> = DEFAULT_SCRIPTS
        .iter()
        .map(|(extension, _, _)| format!("hook{extension}"))
        .filter(|path| tree.members().any(|member| member == path))
        .collect();
    for (extension, interpreter, on) in DEFAULT_SCRIPTS {
        let path = format!("hook{extension}");
        let names = on.map(|name| vec![name.to_owned()]);
        if present.contains(&path)
            && os_matches(names.as_deref(), os)
            && interpreters.iter().any(|known| known == interpreter)
        {
            return (Some((path, interpreter)), present);
        }
    }
    (None, present)
}

/// Every hook the overlay has, in event order, and the `hook.<ext>` files reported as
/// `script-not-run` because an entry needs one and none can run here.
///
/// An entry whose script cannot run here keeps its path (none for a `hook.<ext>` none of which
/// can) so that firing it fails, and an `on_failure = "refuse"` guard fails closed.
#[must_use]
pub fn hooks_of(
    manifest: &Document,
    tree: &OverlayTree,
    os: &str,
    interpreters: &[String],
) -> (Vec<Hook>, Vec<String>) {
    let declared = manifest.get("hooks").and_then(Value::as_object);
    let (chosen, present) = default_hook(tree, os, interpreters);
    let mut entries = Vec::new();
    let mut needs_default = false;
    for event in HookEvent::ALL {
        let name = event.as_str();
        let table: Map<String, Value> = match declared.and_then(|declared| declared.get(name)) {
            Some(Value::Bool(false)) => continue,
            Some(Value::Object(table)) => table.clone(),
            Some(run) => Map::from_iter([("run".to_owned(), run.clone())]),
            None if event.by_default() && default_on(manifest, "hooks") && !present.is_empty() => {
                Map::new()
            }
            None => continue,
        };
        let (path, interpreter) = if let Some(run) = table.get("run").and_then(Value::as_str) {
            (Some(run.to_owned()), extension_interpreter(run))
        } else if let Some((path, interpreter)) = &chosen {
            (Some(path.clone()), Some(*interpreter))
        } else {
            needs_default = true;
            if declared.is_none_or(|declared| !declared.contains_key(name)) {
                continue;
            }
            (None, None)
        };
        let strings = |key: &str| -> Option<Vec<String>> {
            table.get(key).and_then(Value::as_array).map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
        };
        entries.push(Hook {
            event,
            path,
            runs: interpreter
                .is_some_and(|interpreter| interpreters.iter().any(|known| known == interpreter)),
            interpreter: interpreter.map(str::to_owned),
            select: strings("match"),
            sets: strings("sets").unwrap_or_default(),
            timeout: table.get("timeout").and_then(Value::as_u64).unwrap_or(30),
            on_failure: if table.get("on_failure").and_then(Value::as_str) == Some("refuse") {
                OnFailure::Refuse
            } else {
                OnFailure::Report
            },
            payload: event.payload_keys().to_vec(),
        });
    }
    (entries, if needs_default { present } else { Vec::new() })
}

/// Whether the overlay declares a hook on any OS with any interpreter: [`hooks_of`] finds an entry
/// or a `hook.<ext>` that cannot run, for some OS, were every interpreter AOS names present.
/// Entries an overlay switches off (`false`, or `defaults.hooks = false`) are not declared.
///
/// For a consumer that runs no hooks and refuses an overlay that asks for any, whatever it runs on.
#[must_use]
pub fn hooks_declared(manifest: &Document, tree: &OverlayTree) -> bool {
    let interpreters: Vec<String> = INTERPRETERS.iter().map(|name| (*name).to_owned()).collect();
    ["windows", "linux"].into_iter().any(|os| {
        let (hooks, not_run) = hooks_of(manifest, tree, os, &interpreters);
        !hooks.is_empty() || !not_run.is_empty()
    })
}

/// Whose hooks they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookScope {
    /// An overlay's: on once the user allows them at the overlay's digest.
    #[default]
    Overlay,
    /// The user's own: on.
    User,
    /// A project's: on once the user allows them at the digest of the directory that holds them.
    Project,
}

/// What a consumer knows when it plans an overlay's hooks on one harness.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct HookFacts {
    /// The OS AOS names it.
    pub os: String,
    /// The interpreters the consumer can run; none is every one AOS names.
    pub interpreters: Option<Vec<String>>,
    /// The identity the overlay is installed under.
    pub identity: String,
    /// Whose hooks they are.
    pub scope: HookScope,
    /// The overlay's tree digest, when known.
    pub digest: Option<String>,
    /// For a project, every file of the directory that holds its configuration, by path and bytes:
    /// its tree digest replaces `digest`.
    pub tree: Option<Vec<(String, Vec<u8>)>>,
    /// The one digest at which the user allowed these hooks.
    pub allowed: Option<String>,
    /// The events the consumer can fire on this harness; none is every one.
    pub fires: Option<Vec<HookEvent>>,
    /// The events it fires for only some occurrences.
    pub partial: Vec<HookEvent>,
    /// The consumer's bound on one run, in seconds; none is the setting's.
    pub timeout: Option<u64>,
}

impl HookFacts {
    /// Facts about a consumer on `os`, the overlay `identity`, that can fire every event.
    #[must_use]
    pub fn new(os: impl Into<String>, identity: impl Into<String>) -> Self {
        Self {
            os: os.into(),
            identity: identity.into(),
            ..Self::default()
        }
    }
}

/// What a consumer would do with an overlay's hooks on one harness.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct HookPlan {
    /// The hooks, in event order, each with its timeout held to the bound.
    pub hooks: Vec<Hook>,
    /// Whether they are on.
    pub on: bool,
    /// Whether the overlay activates: a guard that is off would fail open, so it waits.
    pub activates: bool,
    /// The interpreters a probe needs, sorted.
    pub needs: Vec<String>,
    /// `(id, subject)` reports, sorted: `hook-not-allowed`, `hook-unsupported`, `hook-partial`.
    pub reports: Vec<(String, String)>,
    /// `(id, subject)` non-default settings, sorted: `script-not-run`, `hook`.
    pub non_defaults: Vec<(String, String)>,
}

/// Whether two digests in text are the same digest; one that does not parse is no digest.
pub(crate) fn same_digest(a: &str, b: &str) -> bool {
    Digest::parse(a)
        .ok()
        .zip(Digest::parse(b).ok())
        .is_some_and(|(a, b)| a == b)
}

/// What a consumer would do with an overlay's hooks, or a scope's own, on one harness.
///
/// # Errors
///
/// The digest's classes when `facts.tree` is not a tree that has a digest.
pub fn hook_plan(
    manifest: &Document,
    files: &OverlayTree,
    facts: &HookFacts,
    settings: &Settings,
) -> Result<HookPlan, crate::Error> {
    let interpreters = facts
        .interpreters
        .clone()
        .unwrap_or_else(|| INTERPRETERS.iter().map(|name| (*name).to_owned()).collect());
    let (mut hooks, not_run) = hooks_of(manifest, files, &facts.os, &interpreters);
    let bound = facts
        .timeout
        .unwrap_or_else(|| settings.hook_timeout.get().unwrap_or(u64::MAX));
    for hook in &mut hooks {
        hook.timeout = hook.timeout.min(bound);
    }
    let mut digest = facts.digest.clone();
    if let Some(tree) = &facts.tree {
        // Every file of the directory, not only those the hooks name: a module beside a script is
        // loaded by it.
        let entries: Vec<(Vec<&[u8]>, TreeEntry)> = tree
            .iter()
            .map(|(path, bytes)| {
                (
                    path.split('/').map(str::as_bytes).collect(),
                    TreeEntry::File(bytes.clone()),
                )
            })
            .collect();
        crate::digest::check_entries(&entries, settings)?;
        digest = Some(
            digest_entries(entries.iter().map(|(path, entry)| (path.clone(), entry)))?
                .digest
                .to_string(),
        );
    }
    let on = facts.scope == HookScope::User
        || digest
            .as_deref()
            .zip(facts.allowed.as_deref())
            .is_some_and(|(digest, allowed)| same_digest(digest, allowed));
    let fires: Vec<HookEvent> = facts
        .fires
        .clone()
        .unwrap_or_else(|| HookEvent::ALL.to_vec());
    let mut non_defaults: Vec<(String, String)> = not_run
        .into_iter()
        .map(|path| ("script-not-run".to_owned(), path))
        .collect();
    let mut reports = Vec::new();
    if !hooks.is_empty() && !on {
        reports.push(("hook-not-allowed".to_owned(), facts.identity.clone()));
    } else if on {
        for hook in &hooks {
            if !fires.contains(&hook.event) {
                reports.push((
                    "hook-unsupported".to_owned(),
                    hook.event.as_str().to_owned(),
                ));
                continue;
            }
            non_defaults.push(("hook".to_owned(), hook.event.as_str().to_owned()));
            if facts.partial.contains(&hook.event) {
                reports.push(("hook-partial".to_owned(), hook.event.as_str().to_owned()));
            }
        }
    }
    let guarded = hooks
        .iter()
        .any(|hook| hook.on_failure == OnFailure::Refuse);
    let mut needs: Vec<String> = hooks
        .iter()
        .filter_map(|hook| hook.interpreter.clone())
        .collect();
    needs.sort();
    needs.dedup();
    reports.sort();
    non_defaults.sort();
    Ok(HookPlan {
        on: on && !hooks.is_empty(),
        activates: on || !guarded,
        hooks,
        needs,
        reports,
        non_defaults,
    })
}

/// Whether a hook whose `match` is `select` runs for this reason or tool kind.
///
/// A tool the consumer could not place (`other`) runs every `pre-tool` and `post-tool` hook, so a
/// tool of an unknown kind never slips past a guard.
#[must_use]
pub fn hook_matches(event: HookEvent, select: Option<&[String]>, value: &str) -> bool {
    let Some(select) = select else {
        return true;
    };
    if event.selects() == Some("kind") && value == "other" {
        return true;
    }
    select.iter().any(|item| item == value)
}

/// The payload a hook of `event` is told: the event's own keys and nothing else of what the
/// consumer knows. A key the consumer does not know is absent, never null.
#[must_use]
pub fn hook_payload(event: HookEvent, known: &Map<String, Value>) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("event".to_owned(), Value::String(event.as_str().to_owned()));
    for key in &event.payload_keys()[1..] {
        let Some(value) = known.get(*key) else {
            continue;
        };
        let value = match (*key, value) {
            ("tool", Value::Object(tool)) => Value::Object(
                ["kind", "name", "input"]
                    .iter()
                    .filter_map(|field| tool.get(*field).map(|v| ((*field).to_owned(), v.clone())))
                    .collect(),
            ),
            _ => value.clone(),
        };
        payload.insert((*key).to_owned(), value);
    }
    payload
}

/// Why a run of a hook failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HookFailure {
    /// The hook did not start.
    NotRun,
    /// The payload was past its bound.
    PayloadTooLarge,
    /// The run took past its timeout.
    Timeout,
    /// The output was past its bound.
    TooLarge,
    /// The exit status was not 0.
    Exit,
    /// The output is not what the event reads.
    Output,
    /// The event's own time had passed before the hook started.
    EventTimeout,
}

impl HookFailure {}

/// What a successful run asks for.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct HookOutput {
    /// What it asks the event to do.
    pub decision: Option<Decision>,
    /// Why, in its own words.
    pub reason: Option<String>,
    /// Text for the agent.
    pub context: Option<String>,
    /// With an allow, the whole object that replaces the call's input.
    pub input: Option<Map<String, Value>>,
    /// Variables to set, or `None` to unset, in the order given.
    pub env: Option<Vec<(String, Option<String>)>>,
}

/// Text a hook may give the agent or the user: no control but tab, line feed and carriage return,
/// and no bidirectional override or isolate.
fn hook_text(text: &str) -> bool {
    !text.chars().any(|c| {
        matches!(c, '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
            || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    })
}

/// A successful run's standard output: what it asks for.
///
/// `pre-env` reads as an env script's output. Every other event reads one JSON object, whose keys
/// are those the event takes; a `decision` that is not one of the event's own is an `output`
/// failure, so is an `input` that does not come with an allow, and `env` may name only the
/// variables `sets` lists.
///
/// # Errors
///
/// [`HookFailure::Output`].
pub fn read_hook_output(
    event: HookEvent,
    data: &[u8],
    sets: &[String],
    settings: &Settings,
) -> Result<HookOutput, HookFailure> {
    let outputs = event.outputs();
    if event == HookEvent::PreEnv {
        return read_env_script_output(data, settings)
            .map(|env| HookOutput {
                env: Some(env),
                ..HookOutput::default()
            })
            .map_err(|_| HookFailure::Output);
    }
    let text = core::str::from_utf8(data).map_err(|_| HookFailure::Output)?;
    let whole = text.trim_matches([' ', '\t', '\r', '\n']);
    if whole.is_empty() {
        return Ok(HookOutput::default());
    }
    let Value::Object(value) = codecs::json(whole.as_bytes(), settings.manifest_depth, true)
        .map_err(|_| HookFailure::Output)?
    else {
        return Err(HookFailure::Output);
    };
    let mut out = HookOutput::default();
    if let Some(decision) = value.get("decision") {
        out.decision = match decision.as_str() {
            Some("block") if outputs.contains(&"block") => Some(Decision::Block),
            Some("deny") if outputs.contains(&"deny") => Some(Decision::Deny),
            Some("ask") if outputs.contains(&"ask") => Some(Decision::Ask),
            Some("allow") if outputs.contains(&"allow") => Some(Decision::Allow),
            _ => return Err(HookFailure::Output),
        };
    }
    if value.keys().any(|key| {
        !matches!(
            key.as_str(),
            "context" | "decision" | "reason" | "input" | "env"
        )
    }) {
        return Err(HookFailure::Output);
    }
    if let Some(reason) = value.get("reason") {
        let Some(reason) = reason.as_str() else {
            return Err(HookFailure::Output);
        };
        if out.decision.is_none()
            || !hook_text(reason)
            || !settings.hook_reason_chars.allows(reason.chars().count())
        {
            return Err(HookFailure::Output);
        }
        out.reason = Some(reason.to_owned());
    }
    if let Some(input) = value.get("input") {
        match input {
            Value::Object(input)
                if outputs.contains(&"input") && out.decision == Some(Decision::Allow) =>
            {
                out.input = Some(input.clone());
            }
            _ => return Err(HookFailure::Output),
        }
    }
    if let Some(context) = value.get("context") {
        match context.as_str() {
            Some(context) if outputs.contains(&"context") && hook_text(context) => {
                out.context = Some(context.to_owned());
            }
            _ => return Err(HookFailure::Output),
        }
    }
    if let Some(env) = value.get("env") {
        let Some(env) = env.as_object().filter(|_| outputs.contains(&"env")) else {
            return Err(HookFailure::Output);
        };
        let mut folded = BTreeSet::new();
        let mut set = Vec::new();
        for (name, item) in env {
            if !is_variable_name(name)
                || is_reserved_variable(name)
                || !sets.contains(name)
                || !folded.insert(name.to_ascii_uppercase())
            {
                return Err(HookFailure::Output);
            }
            match item {
                Value::Null => set.push((name.clone(), None)),
                Value::String(text) if !text.contains('\0') => {
                    set.push((name.clone(), Some(text.clone())));
                }
                _ => return Err(HookFailure::Output),
            }
        }
        out.env = Some(set);
    }
    Ok(out)
}

/// One run of a hook, as the consumer saw it.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct HookRun {
    /// Who the hook belongs to: an overlay's identity, or a scope.
    pub source: String,
    /// What its failure does.
    pub on_failure: Option<OnFailure>,
    /// Seconds it took.
    pub seconds: u64,
    /// Whether it did not start.
    pub not_run: bool,
    /// Bytes of the payload it was sent.
    pub payload_size: u64,
    /// Whether it ran past its timeout.
    pub timed_out: bool,
    /// What it wrote to standard output.
    pub stdout: Vec<u8>,
    /// Its exit status.
    pub exit: i32,
    /// The variables its entry declares.
    pub sets: Vec<String>,
    /// Its entry's `match`, or `None` for every call: on a call of kind `other` only a hook with
    /// none, or one naming `other`, may allow.
    pub select: Option<Vec<String>>,
}

impl HookRun {
    /// A run of the hook of `source` that exited 0 with `stdout`.
    #[must_use]
    pub fn new(source: impl Into<String>, stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            source: source.into(),
            stdout: stdout.into(),
            ..Self::default()
        }
    }
}

/// One run, checked in a fixed order: started, payload, time, size, exit status, then the output
/// itself.
///
/// # Errors
///
/// The first [`HookFailure`] in that order.
pub fn hook_result(
    event: HookEvent,
    run: &HookRun,
    settings: &Settings,
) -> Result<HookOutput, HookFailure> {
    if run.not_run {
        return Err(HookFailure::NotRun);
    }
    if !settings.hook_payload_bytes.allows(run.payload_size) {
        return Err(HookFailure::PayloadTooLarge);
    }
    if run.timed_out {
        return Err(HookFailure::Timeout);
    }
    if !settings.hook_output_bytes.allows(run.stdout.len()) {
        return Err(HookFailure::TooLarge);
    }
    if run.exit != 0 {
        return Err(HookFailure::Exit);
    }
    read_hook_output(event, &run.stdout, &run.sets, settings)
}

/// A report of what a hook did: its id, then what it is about.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct HookReport {
    /// `hook-failed`, `hook-ask-denied` or `hook-not-applied`.
    pub id: &'static str,
    /// The hook's source, the event, and for some the cause or what was not applied.
    pub about: Vec<String>,
}

/// What the hooks of one event amount to.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FiredEvent {
    /// The sources of the hooks that were started, those that failed to start excepted.
    pub ran: Vec<String>,
    /// The tightest decision any asked for: a deny, then an ask, then an allow.
    pub decision: Option<Decision>,
    /// The source of that decision.
    pub by: Option<String>,
    /// Its hook's own words; none for a refusal by failure.
    pub reason: Option<String>,
    /// `(source, text)` for the agent.
    pub context: Vec<(String, String)>,
    /// The variables to set, or `None` to unset.
    pub env: Vec<(String, Option<String>)>,
    /// What went wrong or could not be applied.
    pub reports: Vec<HookReport>,
    /// `(id, subject)`: `env-sensitive`.
    pub non_defaults: Vec<(String, String)>,
    /// With a call's input given: `(source, input)` for what each hook that ran was given.
    pub seen: Option<Vec<(String, Value)>>,
    /// With a call's input given: the input the call runs with. Set only for a combined allow;
    /// `None` is the input it was given.
    pub input: Option<Value>,
}

/// Every hook of one event, in the order given, combined to the tightest result.
///
/// `applies` are the outputs the consumer can apply on this harness (`context`, `env`, `block`,
/// `deny`, `ask`, `allow`, `input`, `refuse`); none is all the event takes, and refuse. A hook that would start after
/// the event's own bound has passed fails as `event-timeout`. `windows` makes a variable's name
/// compare without case, keeping the spelling it was first given.
#[must_use]
pub fn fire_event(
    event: HookEvent,
    runs: &[HookRun],
    applies: Option<&[&str]>,
    settings: &Settings,
    windows: bool,
) -> FiredEvent {
    fire_event_after(event, runs, applies, settings, windows, 0)
}

/// [`fire_event`] for a session whose context already holds `context_used` bytes.
///
/// A part of a hook's context is checked in run order, in the bytes of its UTF-8 text, together
/// with what the session holds: one that would take the total past `settings.context_bytes` is not
/// added, whole and never cut, and is reported as `hook-not-applied`; a later part that fits still
/// is added, and a part that reaches the bound exactly fits.
#[must_use]
pub fn fire_event_after(
    event: HookEvent,
    runs: &[HookRun],
    applies: Option<&[&str]>,
    settings: &Settings,
    windows: bool,
    context_used: usize,
) -> FiredEvent {
    fire_event_for_call(
        event,
        runs,
        applies,
        settings,
        windows,
        context_used,
        None,
        None,
    )
}

/// [`fire_event_after`] for a `pre-tool` call whose input, `tool_input`, a hook's allow may
/// rewrite.
///
/// [`FiredEvent::seen`] says what each hook was given: a hook's `input` replaces the call's for
/// the hooks after it. [`FiredEvent::input`] is what the call runs with, set only by a combined
/// allow, since an ask or a deny beats a rewrite. An allow whose `input` the consumer cannot apply
/// (`input` is not in `applies`) is not applied at all and is reported `hook-not-applied`; a
/// rewrite that an ask beat is reported the same way.
///
/// `tool_kind` is the call's kind: on `other`, a run whose [`HookRun::select`] is given and does
/// not name `other` still runs and its deny and ask count, but its allow and input do not, and
/// are reported `hook-not-applied` `allow`.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn fire_event_for_call(
    event: HookEvent,
    runs: &[HookRun],
    applies: Option<&[&str]>,
    settings: &Settings,
    windows: bool,
    context_used: usize,
    tool_input: Option<&Value>,
    tool_kind: Option<&str>,
) -> FiredEvent {
    let mut context_used = context_used;
    let mut current = tool_input.cloned();
    let mut rewriter: Option<String> = None;
    let can = |output: &str| {
        applies.map_or_else(
            || output == "refuse" || event.outputs().contains(&output),
            |list| list.contains(&output),
        )
    };
    let mut spent = 0u64;
    let mut fired = FiredEvent {
        seen: tool_input.map(|_| Vec::new()),
        ..FiredEvent::default()
    };
    // Where each variable sits in `fired.env`, by the name as it compares.
    let mut env_at: BTreeMap<String, usize> = BTreeMap::new();
    for run in runs {
        let source = run.source.clone();
        let failed = |cause: HookFailure| HookReport {
            id: "hook-failed",
            about: vec![
                source.clone(),
                event.as_str().to_owned(),
                cause.as_str().to_owned(),
            ],
        };
        let attempt = if settings
            .hook_event_timeout
            .get()
            .is_some_and(|bound| spent >= bound)
        {
            Err(HookFailure::EventTimeout)
        } else {
            fired.ran.push(source.clone());
            if let (Some(seen), Some(current)) = (&mut fired.seen, &current) {
                seen.push((source.clone(), current.clone()));
            }
            spent = spent.saturating_add(run.seconds);
            hook_result(event, run, settings)
        };
        let mut out = match attempt {
            Ok(out) => out,
            Err(cause) => {
                fired.reports.push(failed(cause));
                match (run.on_failure, event.refusal()) {
                    (Some(OnFailure::Refuse), Some(refusal)) => HookOutput {
                        decision: Some(refusal),
                        ..HookOutput::default()
                    },
                    _ => continue,
                }
            }
        };
        let mut decision = out.decision;
        if decision == Some(Decision::Ask) && !can("ask") {
            fired.reports.push(HookReport {
                id: "hook-ask-denied",
                about: vec![source.clone(), event.as_str().to_owned()],
            });
            decision = Some(Decision::Deny);
        }
        if let Some(asked) = decision
            && !can(asked.as_str())
        {
            fired.reports.push(HookReport {
                id: "hook-not-applied",
                about: vec![
                    source.clone(),
                    event.as_str().to_owned(),
                    asked.as_str().to_owned(),
                ],
            });
            decision = None;
        }
        if decision == Some(Decision::Allow)
            && tool_kind == Some("other")
            && run
                .select
                .as_ref()
                .is_some_and(|select| !select.iter().any(|item| item == "other"))
        {
            fired.reports.push(HookReport {
                id: "hook-not-applied",
                about: vec![
                    source.clone(),
                    event.as_str().to_owned(),
                    "allow".to_owned(),
                ],
            });
            decision = None;
            out.input = None;
        }
        if decision == Some(Decision::Allow)
            && let Some(input) = out.input
        {
            if can("input") {
                if current.is_some() {
                    current = Some(Value::Object(input));
                }
                rewriter = Some(source.clone());
            } else {
                fired.reports.push(HookReport {
                    id: "hook-not-applied",
                    about: vec![
                        source.clone(),
                        event.as_str().to_owned(),
                        "input".to_owned(),
                    ],
                });
                decision = None;
            }
        }
        if Decision::rank(decision) > Decision::rank(fired.decision) {
            fired.decision = decision;
            fired.by = Some(source.clone());
            fired.reason.clone_from(&out.reason);
        }
        if Decision::rank(decision) == 3 {
            // The prompt, compaction, call or session does not happen: nothing else applies.
            fired.context.clear();
            fired.env.clear();
            env_at.clear();
            break;
        }
        if let Some(context) = out.context.filter(|context| !context.is_empty()) {
            let total = context_used.saturating_add(context.len());
            if can("context") && settings.context_bytes.allows(total) {
                context_used = total;
                fired.context.push((source.clone(), context));
            } else {
                fired.reports.push(HookReport {
                    id: "hook-not-applied",
                    about: vec![
                        source.clone(),
                        event.as_str().to_owned(),
                        "context".to_owned(),
                    ],
                });
            }
        }
        if let Some(env) = out.env.filter(|env| !env.is_empty()) {
            if !can("env") {
                fired.reports.push(HookReport {
                    id: "hook-not-applied",
                    about: vec![source.clone(), event.as_str().to_owned(), "env".to_owned()],
                });
                continue;
            }
            for (name, value) in env {
                // On Windows a name set under another case keeps the spelling it has.
                let compared = if windows {
                    name.to_ascii_uppercase()
                } else {
                    name.clone()
                };
                match env_at.get(&compared) {
                    Some(at) => fired.env[*at].1 = value,
                    None => {
                        env_at.insert(compared, fired.env.len());
                        fired.env.push((name, value));
                    }
                }
            }
        }
    }
    let mut sensitive: Vec<(String, String)> = fired
        .env
        .iter()
        .filter(|(name, _)| is_sensitive_variable(name))
        .map(|(name, _)| ("env-sensitive".to_owned(), name.clone()))
        .collect();
    sensitive.sort();
    fired.non_defaults = sensitive;
    if let Some(rewriter) = rewriter
        && fired.seen.is_some()
    {
        match fired.decision {
            Some(Decision::Allow) => fired.input = current,
            Some(Decision::Ask) => fired.reports.push(HookReport {
                id: "hook-not-applied",
                about: vec![rewriter, event.as_str().to_owned(), "input".to_owned()],
            }),
            _ => {}
        }
    }
    fired
}

impl OnFailure {
    /// The word the specification and its vectors write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Refuse => "refuse",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "report" => Some(Self::Report),
            "refuse" => Some(Self::Refuse),
            _ => None,
        }
    }
}

impl HookScope {
    /// The word the specification and its vectors write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Overlay => "overlay",
            Self::User => "user",
            Self::Project => "project",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "overlay" => Some(Self::Overlay),
            "user" => Some(Self::User),
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

impl Decision {
    /// The decision's name in hook output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Deny => "deny",
            Self::Ask => "ask",
            Self::Allow => "allow",
            Self::Refuse => "refuse",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "block" => Some(Self::Block),
            "deny" => Some(Self::Deny),
            "ask" => Some(Self::Ask),
            "allow" => Some(Self::Allow),
            "refuse" => Some(Self::Refuse),
            _ => None,
        }
    }
}

impl HookFailure {
    /// The cause as reports name it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRun => "not-run",
            Self::PayloadTooLarge => "payload-too-large",
            Self::Timeout => "timeout",
            Self::TooLarge => "too-large",
            Self::Exit => "exit",
            Self::Output => "output",
            Self::EventTimeout => "event-timeout",
        }
    }

    /// What a word stands for.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        match word {
            "not-run" => Some(Self::NotRun),
            "payload-too-large" => Some(Self::PayloadTooLarge),
            "timeout" => Some(Self::Timeout),
            "too-large" => Some(Self::TooLarge),
            "exit" => Some(Self::Exit),
            "output" => Some(Self::Output),
            "event-timeout" => Some(Self::EventTimeout),
            _ => None,
        }
    }
}

impl core::fmt::Debug for HookOutput {
    /// Shows which outputs there are, never their text or values.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HookOutput")
            .field("decision", &self.decision)
            .field("reason", &self.reason.as_ref().map(String::len))
            .field("context", &self.context.as_ref().map(String::len))
            .field("input", &self.input.as_ref().map(Map::len))
            .field(
                "env",
                &self
                    .env
                    .as_ref()
                    .map(|env| env.iter().map(|(name, _)| name).collect::<Vec<_>>()),
            )
            .finish()
    }
}

impl core::fmt::Debug for HookRun {
    /// Shows the hook and the size of its output, never the output.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HookRun")
            .field("source", &self.source)
            .field("exit", &self.exit)
            .field("timed_out", &self.timed_out)
            .field("stdout_bytes", &self.stdout.len())
            .finish_non_exhaustive()
    }
}

impl core::fmt::Debug for FiredEvent {
    /// Shows who ran and decided and the names of variables set, never text or values.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FiredEvent")
            .field("ran", &self.ran)
            .field("decision", &self.decision)
            .field("by", &self.by)
            .field(
                "context_from",
                &self
                    .context
                    .iter()
                    .map(|(source, _)| source)
                    .collect::<Vec<_>>(),
            )
            .field(
                "env",
                &self.env.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .field("reports", &self.reports)
            .finish_non_exhaustive()
    }
}
