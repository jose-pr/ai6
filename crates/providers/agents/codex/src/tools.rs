//! The tool check of a restricted thread (answer-only or assistant).
//!
//! A Codex release can offer the model tools that no setting this handler passes switches off,
//! and a new release adds more. So before a restricted app server starts, the handler runs the
//! same Codex with the same settings, environment and working directory: `codex features list`,
//! to switch off every enabled feature not on [`KEPT_FEATURES`], `codex mcp list --json`, to
//! switch off every MCP server of the user's configuration that is not the thread's (a `-c
//! mcp_servers` table adds to the user's servers instead of replacing them), then `codex debug
//! prompt-input`, which renders what the model would be given without asking it anything. The
//! thread starts only when the tools offered are exactly those of its MCP servers (none for an
//! answer-only thread). With `AI6_CODEX_TOOL_CHECK=warn` in the Agent's environment it starts
//! regardless, and what the check found is reported as a weakening.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use ai6::directorates::q::terminal::{
    AgentTerminal, ExitInfo, READ_TIMED_OUT, STREAM_COLS, Spec, strip_controls,
};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Status, ToValue};
use serde_json::Value;

use crate::sandbox::SCRIPT_FEATURES;
use crate::{internal, refuse};

/// The environment variable that chooses what a failed tool check does: `strict` (the
/// default) refuses to start, `warn` starts and reports a weakening.
pub const TOOL_CHECK_ENV: &str = "AI6_CODEX_TOOL_CHECK";

/// Features a restricted thread keeps: how Codex stores its login and reaches its service,
/// none of which offers the model a tool, and `incremental_tools`. Every other enabled feature
/// is switched off.
pub const KEPT_FEATURES: [&str; 5] = [
    "secret_auth_storage",
    "enable_request_compression",
    "system_proxy_fallback",
    "unbounded_connection_retries",
    // Without it `codex debug prompt-input` renders no tool, while the app server still sends
    // the tools of the model's catalog entry (`functions.exec` and `wait` in Codex 0.164.0):
    // the check would see less than the thread is given.
    "incremental_tools",
];

/// How long a feature list stands for the same Codex, settings and environment; the user's
/// own Codex configuration can change under it.
const CHECK_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// How long one of the check's commands may run.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// What a failed check does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ToolCheck {
    Strict,
    Warn,
}

impl ToolCheck {
    /// The choice in `environment`, `strict` when unset; any other value is refused.
    pub(crate) fn from_environment(
        environment: &BTreeMap<String, String>,
    ) -> Result<Self, ProviderError> {
        match environment.get(TOOL_CHECK_ENV).map(|value| value.trim()) {
            None | Some("" | "strict") => Ok(Self::Strict),
            Some("warn") => Ok(Self::Warn),
            Some(other) => Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!("{TOOL_CHECK_ENV} is {other:?}; it takes strict or warn"),
            )),
        }
    }
}

/// What the check decided for a thread that may start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Checked {
    /// `-c` overrides that switch the features off, to add to the app server's.
    pub(crate) overrides: Vec<String>,
    /// What a `warn` check found, to report as a weakening.
    pub(crate) weakening: Option<String>,
}

/// What `codex features list` printed: the enabled features, and every non-blank line that
/// does not read as `name stage state` (the stage possibly two words).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FeatureList {
    pub(crate) enabled: Vec<String>,
    pub(crate) unread: Vec<String>,
}

/// Reads `codex features list` output; `None` when no line reads as a feature.
pub(crate) fn feature_list(output: &[u8]) -> Option<FeatureList> {
    let text = String::from_utf8_lossy(&strip_controls(output)).replace('\r', "");
    let mut read = 0;
    let mut list = FeatureList::default();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let words = line.split_whitespace().collect::<Vec<_>>();
        let named = words.first().is_some_and(|name| {
            name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.".contains(&byte)
            })
        });
        let state = words.last().copied();
        if words.len() < 3 || !named || !matches!(state, Some("true" | "false")) {
            list.unread.push(line.to_owned());
            continue;
        }
        read += 1;
        if state == Some("true") {
            list.enabled.push(words[0].to_owned());
        }
    }
    (read > 0).then_some(list)
}

/// The server names in `codex mcp list --json` output; `None` unless it is one JSON array of
/// objects, each with a string `name`, with nothing after it. The output also holds each
/// server's environment values, so nothing else of it is kept.
pub(crate) fn mcp_server_names(output: &[u8]) -> Option<Vec<String>> {
    // Pretty-printed JSON, as `offered_tools` reads it: every line break can go.
    let mut text = strip_controls(output);
    text.retain(|byte| !matches!(byte, b'\r' | b'\n'));
    let start = (0..text.len()).find(|&at| opens_server_list(&text[at..]))?;
    let servers: Value = serde_json::from_slice(&text[start..]).ok()?;
    servers
        .as_array()?
        .iter()
        .map(|server| Some(server.get("name")?.as_str()?.to_owned()))
        .collect()
}

/// Whether `text` starts `[`, then `{` or `]`, with only whitespace between.
fn opens_server_list(text: &[u8]) -> bool {
    let mut rest = text.iter().skip_while(|byte| byte.is_ascii_whitespace());
    rest.next() == Some(&b'[')
        && matches!(
            rest.find(|byte| !byte.is_ascii_whitespace()),
            Some(b'{' | b']')
        )
}

/// Codex splits a `-c` key at every dot and reads no quoted part, so only a name that is a
/// bare TOML key can be addressed as `mcp_servers.<name>`.
fn addressable(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

/// `-c` overrides that switch off every listed MCP server that is not one of the thread's,
/// and the listed names that cannot be addressed, which stay on.
pub(crate) fn switch_off_servers(
    listed: &[String],
    servers: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut overrides = Vec::new();
    let mut unaddressable = Vec::new();
    for name in listed.iter().filter(|name| !servers.contains(name)) {
        if addressable(name) {
            overrides.extend(["-c".to_owned(), format!("mcp_servers.{name}.enabled=false")]);
        } else {
            unaddressable.push(format!("{name:?}"));
        }
    }
    (overrides, unaddressable)
}

/// The tools `codex debug prompt-input` output offers the model, named `namespace.tool` inside
/// a namespace; `None` unless the output is one JSON array of input items, holding at least
/// one `message` and otherwise only `additional_tools`, with nothing after it, so an unknown
/// shape is never read as "no tools".
pub(crate) fn offered_tools(output: &[u8]) -> Option<Vec<String>> {
    let items = prompt_items(output)?;
    let mut tools = Vec::new();
    let mut messages = 0;
    for item in &items {
        match item.get("type").and_then(Value::as_str)? {
            "message" => messages += 1,
            "additional_tools" => collect(item.get("tools")?.as_array()?, "", &mut tools)?,
            _ => return None,
        }
    }
    (messages > 0).then_some(tools)
}

/// The one JSON array of input items in `codex debug prompt-input` output.
fn prompt_items(output: &[u8]) -> Option<Vec<Value>> {
    // The output is pretty-printed JSON, and a terminal may break a long line: JSON text holds
    // no raw line break, so every one can go.
    let mut text = strip_controls(output);
    text.retain(|byte| !matches!(byte, b'\r' | b'\n'));
    let start = (0..text.len()).find(|&at| opens_item_list(&text[at..]))?;
    // One value, then only whitespace: `from_slice` refuses anything after it.
    match serde_json::from_slice(&text[start..]).ok()? {
        Value::Array(items) => Some(items),
        _ => None,
    }
}

/// The tools `functions.exec` nests, as its description in `codex debug prompt-input` output
/// declares them, one `declare const tools: { NAME(` per tool under an `exec tool
/// declaration:` heading: `Some` empty when `exec` is not offered, `None` when its declarations
/// cannot be read. Codex leaves deferred tools (an MCP server's, in 0.164.0) out of the
/// description, so those are not seen here.
pub(crate) fn exec_nested(output: &[u8]) -> Option<Vec<String>> {
    let exec = prompt_items(output)?
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("additional_tools"))
        .filter_map(|item| item.get("tools")?.as_array())
        .flatten()
        .filter(|tool| tool.get("name").and_then(Value::as_str) == Some("functions"))
        .filter_map(|namespace| namespace.get("tools")?.as_array())
        .flatten()
        .find(|tool| tool.get("name").and_then(Value::as_str) == Some("exec"))
        .cloned();
    let Some(exec) = exec else {
        return Some(Vec::new());
    };
    let description = exec.get("description")?.as_str()?;
    const DECLARATION: &str = "declare const tools: { ";
    let names: Vec<String> = description
        .match_indices(DECLARATION)
        .map(|(at, _)| {
            description[at + DECLARATION.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$'))
                .collect()
        })
        .collect();
    let headings = description.matches("exec tool declaration:").count();
    (!names.is_empty() && names.len() == headings && names.iter().all(|name| !name.is_empty()))
        .then_some(names)
}

/// Whether `text` starts `[`, `{`, `"type"`, with only whitespace between.
fn opens_item_list(text: &[u8]) -> bool {
    let rest = |text: &[u8], token: &[u8]| -> Option<usize> {
        let skipped = text
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count();
        text[skipped..]
            .starts_with(token)
            .then_some(skipped + token.len())
    };
    let Some(at) = rest(text, b"[") else {
        return false;
    };
    let Some(more) = rest(&text[at..], b"{") else {
        return false;
    };
    rest(&text[at + more..], b"\"type\"").is_some()
}

fn collect(list: &[Value], prefix: &str, tools: &mut Vec<String>) -> Option<()> {
    for tool in list {
        let name = tool.get("name").and_then(Value::as_str)?;
        if tool.get("type").and_then(Value::as_str) == Some("namespace") {
            collect(
                tool.get("tools")?.as_array()?,
                &format!("{prefix}{name}."),
                tools,
            )?;
        } else {
            tools.push(format!("{prefix}{name}"));
        }
    }
    Some(())
}

/// The tools of Codex's code mode: `exec` runs JavaScript that calls the thread's other tools
/// (its MCP servers' and the command tools) and `wait` waits on it. A scripts thread is offered
/// these alone.
pub(crate) const CODE_MODE_TOOLS: [&str; 2] = ["functions.exec", "functions.wait"];

/// The tools nested in `exec` that run a script's commands: `exec_command` starts one and
/// `write_stdin` writes to one it started.
const COMMAND_TOOLS: [&str; 2] = ["exec_command", "write_stdin"];
/// The nested tools that list and read the resources of the configured MCP servers, which the
/// tool check leaves only the thread's.
const MCP_RESOURCE_TOOLS: [&str; 3] = [
    "list_mcp_resources",
    "list_mcp_resource_templates",
    "read_mcp_resource",
];
/// Nested tools that read and reach nothing: the current time. Codex 0.164.0 offers it with
/// every feature but the kept ones off, and no setting removes it.
const INERT_TOOLS: [&str; 1] = ["clock__curr_time"];

/// What a thread may use, for [`judge`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct Allowed<'a> {
    /// The MCP servers the thread may use.
    pub(crate) servers: &'a [String],
    /// Whether it runs scripts: [`CODE_MODE_TOOLS`] are allowed, with [`COMMAND_TOOLS`] nested.
    pub(crate) scripts: bool,
    /// Whether it may change files through Codex's patch tool (`apply_patch`, nested).
    pub(crate) files: bool,
}

impl Allowed<'_> {
    /// Whether `exec` may nest `tool`: the command tools, `apply_patch` with files, the
    /// resource tools and `mcp__<server>__<tool>` of the thread's servers, and the inert ones.
    fn nests(&self, tool: &str) -> bool {
        COMMAND_TOOLS.contains(&tool)
            || INERT_TOOLS.contains(&tool)
            || (self.files && tool == "apply_patch")
            || (!self.servers.is_empty() && MCP_RESOURCE_TOOLS.contains(&tool))
            || self.servers.iter().any(|server| {
                // Codex names a nested tool as a JavaScript identifier.
                let server: String = server
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                tool.strip_prefix(&format!("mcp__{server}__"))
                    .is_some_and(|rest| !rest.is_empty())
            })
    }
}

/// Judges the offered tools against what a thread may use: each tool must belong to one of its
/// MCP servers (`mcp__<server>.<tool>`) and each server must offer at least one. With
/// `scripts`, [`CODE_MODE_TOOLS`] are allowed too, a server need not offer a tool directly
/// (code mode nests them in `exec`), and every tool `exec` declares (`nested`, from
/// [`exec_nested`]) must be one it may nest ([`Allowed::nests`]). `Ok(None)` passes;
/// `Ok(Some(found))` is a failure a `warn` check starts with; `Err` refuses.
pub(crate) fn judge(
    offered: Option<&[String]>,
    nested: Option<&[String]>,
    allowed: Allowed<'_>,
    check: ToolCheck,
) -> Result<Option<String>, String> {
    let Allowed {
        servers, scripts, ..
    } = allowed;
    let found = match offered {
        None => {
            "the tools Codex offers could not be read from `codex debug prompt-input`".to_owned()
        }
        Some(offered) => {
            let offers = |tool: &str| {
                (scripts && CODE_MODE_TOOLS.contains(&tool))
                    || servers.iter().any(|server| {
                        tool.strip_prefix(&format!("mcp__{server}."))
                            .is_some_and(|rest| !rest.is_empty())
                    })
            };
            let extra = offered
                .iter()
                .filter(|tool| !offers(tool))
                .cloned()
                .collect::<BTreeSet<_>>();
            let missing = servers
                .iter()
                .filter(|_| !scripts)
                .filter(|server| {
                    !offered
                        .iter()
                        .any(|tool| tool.starts_with(&format!("mcp__{server}.")))
                })
                .cloned()
                .collect::<Vec<_>>();
            let mut found = Vec::new();
            if !extra.is_empty() {
                found.push(format!(
                    "Codex offers tools beyond the thread's MCP servers: {}",
                    extra.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
            if !missing.is_empty() {
                found.push(format!(
                    "Codex offers no tool of MCP server {} directly",
                    missing.join(", ")
                ));
            }
            if scripts && offered.iter().any(|tool| tool == CODE_MODE_TOOLS[0]) {
                match nested {
                    None => found.push(
                        "the tools nested in functions.exec could not be read from its \
                         description"
                            .to_owned(),
                    ),
                    Some(nested) => {
                        let extra = nested
                            .iter()
                            .filter(|tool| !allowed.nests(tool))
                            .cloned()
                            .collect::<BTreeSet<_>>();
                        if !extra.is_empty() {
                            found.push(format!(
                                "Codex nests tools in functions.exec beyond the thread's: {}",
                                extra.into_iter().collect::<Vec<_>>().join(", ")
                            ));
                        }
                    }
                }
            }
            if found.is_empty() {
                return Ok(None);
            }
            found.join("; ")
        }
    };
    match check {
        ToolCheck::Strict => Err(format!(
            "this restricted Codex thread was not started: {found} \
             ({TOOL_CHECK_ENV}=warn starts it anyway, as a weakening)"
        )),
        ToolCheck::Warn => Ok(Some(found)),
    }
}

/// Refuses, naming what was found, when a `strict` check found anything.
fn refuse_if_strict(found: &[String], check: ToolCheck) -> Result<(), ProviderError> {
    if found.is_empty() || check == ToolCheck::Warn {
        return Ok(());
    }
    Err(refuse(
        Status::GUATIAO_ERR_UNSUPPORTED,
        format!(
            "this restricted Codex thread was not started: {} \
             ({TOOL_CHECK_ENV}=warn starts it anyway, as a weakening)",
            found.join("; ")
        ),
    ))
}

/// The environment variables that decide which Codex runs and which configuration it reads.
pub(crate) const CONFIG_ENVIRONMENT: [&str; 7] = [
    "CODEX_HOME",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CONFIG_HOME",
    "PATH",
];

/// What a feature list depends on but the working directory, which only names the project
/// marked untrusted and the paths of the scripts' sandbox profile.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FeaturesKey {
    executable: String,
    version: String,
    overrides: Vec<String>,
    environment: Vec<(String, Option<String>)>,
    check: ToolCheck,
    scripts: bool,
}

/// The features and MCP servers last switched off for a key, as `-c` overrides, and what a
/// `warn` check found reading them.
type SwitchedOff = (Vec<String>, Option<String>);

fn feature_lists() -> &'static Mutex<HashMap<FeaturesKey, (Instant, SwitchedOff)>> {
    static LISTS: OnceLock<Mutex<HashMap<FeaturesKey, (Instant, SwitchedOff)>>> = OnceLock::new();
    LISTS.get_or_init(Mutex::default)
}

/// One restricted thread's check, run through `terminal` with the app server's settings.
pub(crate) struct Probe<'a> {
    pub(crate) terminal: &'a Object<dyn AgentTerminal>,
    pub(crate) executable: &'a str,
    /// The app server's `-c` overrides, as `-c` and value pairs.
    pub(crate) overrides: &'a [String],
    pub(crate) cwd: &'a str,
    pub(crate) environment: &'a BTreeMap<String, String>,
    pub(crate) model: Option<&'a str>,
    /// The MCP servers the thread may use.
    pub(crate) servers: &'a [String],
    /// Whether the thread runs scripts in Codex's sandbox: it keeps
    /// [`SCRIPT_FEATURES`](crate::sandbox::SCRIPT_FEATURES) and may be offered
    /// [`CODE_MODE_TOOLS`].
    pub(crate) scripts: bool,
    /// Whether it may change files through Codex's patch tool.
    pub(crate) files: bool,
}

impl Probe<'_> {
    /// Runs the check. The features and MCP servers to switch off are reused for the same
    /// Codex, settings and configuration environment for [`CHECK_LIFETIME`]; what Codex offers
    /// is rendered on every start, so a change the key cannot see (a configuration file
    /// edited) is caught there, except an MCP server added to the user's configuration, whose
    /// tools `prompt-input` does not render: it is switched off once the lifetime has passed.
    pub(crate) fn check(&self) -> Result<Checked, ProviderError> {
        let check = ToolCheck::from_environment(self.environment)?;
        let version = String::from_utf8_lossy(&strip_controls(
            &self.capture(&[self.executable.to_owned(), "--version".to_owned()])?,
        ))
        .trim()
        .to_owned();
        let key = FeaturesKey {
            executable: self.executable.to_owned(),
            version,
            overrides: self
                .overrides
                .iter()
                .filter(|value| {
                    !value.starts_with("projects=") && !value.starts_with("permissions.")
                })
                .cloned()
                .collect(),
            environment: CONFIG_ENVIRONMENT
                .iter()
                .map(|name| ((*name).to_owned(), self.environment.get(*name).cloned()))
                .collect(),
            check,
            scripts: self.scripts,
        };
        let cached = feature_lists().lock().ok().and_then(|lists| {
            lists
                .get(&key)
                .filter(|(at, _)| at.elapsed() < CHECK_LIFETIME)
                .map(|(_, switched)| switched.clone())
        });
        let (overrides, read) = match cached {
            Some(switched) => switched,
            None => {
                let switched = self.switch_off(check)?;
                if let Ok(mut lists) = feature_lists().lock() {
                    lists.retain(|_, (at, _)| at.elapsed() < CHECK_LIFETIME);
                    lists.insert(key, (Instant::now(), switched.clone()));
                }
                switched
            }
        };
        let mut weakenings = read.into_iter().collect::<Vec<_>>();
        let mut argv = self.command(&["debug", "prompt-input"], &overrides);
        if let Some(model) = self.model {
            let model = serde_json::to_string(model).map_err(internal)?;
            argv.extend(["-c".to_owned(), format!("model={model}")]);
        }
        let output = self.capture(&argv)?;
        let allowed = Allowed {
            servers: self.servers,
            scripts: self.scripts,
            files: self.files,
        };
        let offered = offered_tools(&output);
        let nested = exec_nested(&output);
        if let Some(found) = judge(offered.as_deref(), nested.as_deref(), allowed, check)
            .map_err(|reason| refuse(Status::GUATIAO_ERR_UNSUPPORTED, reason))?
        {
            weakenings.push(found);
        }
        Ok(Checked {
            overrides,
            weakening: (!weakenings.is_empty()).then(|| weakenings.join("; ")),
        })
    }

    /// The `-c` overrides that switch off every enabled feature not kept, from `codex features
    /// list`, and every MCP server of the user's that is not the thread's, from `codex mcp list
    /// --json` with those features off; and what a `warn` check found reading them.
    fn switch_off(&self, check: ToolCheck) -> Result<SwitchedOff, ProviderError> {
        let mut found = Vec::new();
        let list = feature_list(&self.capture(&self.command(&["features", "list"], &[]))?);
        match &list {
            None => found.push(
                "the features Codex enables could not be read from `codex features list`"
                    .to_owned(),
            ),
            Some(list) => found.extend(list.unread.first().map(|line| {
                format!(
                    "`codex features list` printed {} line(s) that are not a feature, such as {line:?}",
                    list.unread.len()
                )
            })),
        }
        refuse_if_strict(&found, check)?;
        let mut overrides = list
            .map(|list| list.enabled)
            .unwrap_or_default()
            .into_iter()
            .filter(|name| !KEPT_FEATURES.contains(&name.as_str()))
            .filter(|name| !(self.scripts && SCRIPT_FEATURES.contains(&name.as_str())))
            .flat_map(|name| ["-c".to_owned(), format!("features.{name}=false")])
            .collect::<Vec<_>>();
        let listed =
            mcp_server_names(&self.capture(&self.command(&["mcp", "list", "--json"], &overrides))?);
        match listed {
            None => found.push(
                "the MCP servers of Codex's configuration could not be read from \
                 `codex mcp list --json`, so none of the user's is switched off"
                    .to_owned(),
            ),
            Some(listed) => {
                let (off, unaddressable) = switch_off_servers(&listed, self.servers);
                overrides.extend(off);
                if !unaddressable.is_empty() {
                    found.push(format!(
                        "MCP server {} of Codex's configuration cannot be switched off by \
                         name and stays on",
                        unaddressable.join(", ")
                    ));
                }
            }
        }
        refuse_if_strict(&found, check)?;
        Ok((overrides, (!found.is_empty()).then(|| found.join("; "))))
    }

    /// `codex <words>` with the app server's settings and `more`.
    fn command(&self, words: &[&str], more: &[String]) -> Vec<String> {
        let mut argv = vec![self.executable.to_owned()];
        argv.extend(words.iter().map(|word| (*word).to_owned()));
        argv.extend_from_slice(self.overrides);
        argv.extend_from_slice(more);
        argv
    }

    /// Everything `argv` writes until it exits.
    fn capture(&self, argv: &[String]) -> Result<Vec<u8>, ProviderError> {
        let spec = Spec {
            argv: argv.to_vec(),
            cwd: Some(self.cwd.to_owned()),
            env: Some(self.environment.clone()),
            rows: 24,
            cols: STREAM_COLS,
        };
        let session = self.terminal.spawn(&spec.to_value().map_err(internal)?)?;
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut output = Vec::new();
        let mut buffer = vec![0_u8; 65536];
        let mut exited = false;
        loop {
            if Instant::now() >= deadline {
                let _ = session.kill();
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    format!("`{}` did not finish", argv[1..].join(" ")),
                ));
            }
            let count = session.read(&mut buffer, 100)?;
            if count > 0 {
                output.extend_from_slice(&buffer[..count as usize]);
                continue;
            }
            // A pseudo-console can stay open after its program exits: once the program has
            // gone, the first quiet read ends the output.
            if count == 0 || (count == READ_TIMED_OUT && exited) {
                break;
            }
            exited = session
                .exit_status(0)
                .ok()
                .and_then(|value| ExitInfo::from_value(&value).ok())
                .is_some_and(|exit| !exit.running);
        }
        let _ = session.kill();
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `codex debug prompt-input`, `codex features list` and `codex mcp list --json` output
    /// of Codex 0.164.0 on Windows, model descriptions shortened, message text and server
    /// commands and environment values replaced (they hold the machine's paths): with the
    /// settings this handler passed before the tool check, with those it passes now
    /// (answer-only, and an assistant with one MCP server, `moneypenny`, model `gpt-6.1-sol`,
    /// a user configuration holding one stdio server, `node_repl`), and with code mode kept
    /// and that server's namespace exposed directly.
    const BEFORE: &str = include_str!("../tests/fixtures/codex-0.164.0/prompt-input-main.json");
    const ANSWER_ONLY: &str =
        include_str!("../tests/fixtures/codex-0.164.0/prompt-input-answer-only.json");
    const ASSISTANT: &str =
        include_str!("../tests/fixtures/codex-0.164.0/prompt-input-assistant.json");
    const CODE_MODE: &str =
        include_str!("../tests/fixtures/codex-0.164.0/prompt-input-code-mode.json");
    const FEATURES: &str = include_str!("../tests/fixtures/codex-0.164.0/features-list.txt");
    const MCP_LIST: &str = include_str!("../tests/fixtures/codex-0.164.0/mcp-list.json");
    const MCP_LIST_ASSISTANT: &str =
        include_str!("../tests/fixtures/codex-0.164.0/mcp-list-assistant.json");

    fn moneypenny() -> Vec<String> {
        vec!["moneypenny".to_owned()]
    }

    /// A thread on `servers`, with scripts and files or without either.
    fn allow(servers: &[String], scripts: bool) -> Allowed<'_> {
        Allowed {
            servers,
            scripts,
            files: scripts,
        }
    }

    #[test]
    fn the_settings_before_the_check_left_code_mode_agents_and_computer_use_on() {
        let offered = offered_tools(BEFORE.as_bytes()).unwrap();
        for tool in [
            "functions.exec",
            "functions.request_user_input",
            "clock.sleep",
            "collaboration.spawn_agent",
            "mcp__cua_repl.js",
        ] {
            assert!(offered.iter().any(|t| t == tool), "{tool}: {offered:?}");
        }
        let refused =
            judge(Some(&offered), None, allow(&[], false), ToolCheck::Strict).unwrap_err();
        assert!(refused.contains("functions.exec") && refused.contains("mcp__cua_repl.js"));
    }

    #[test]
    fn an_answer_only_thread_offered_the_catalogs_exec_and_wait_is_refused() {
        let offered = offered_tools(ANSWER_ONLY.as_bytes()).unwrap();
        assert_eq!(
            offered,
            ["functions.exec", "functions.wait"],
            "request_user_input is switched off; exec and wait come with the model"
        );
        let refused =
            judge(Some(&offered), None, allow(&[], false), ToolCheck::Strict).unwrap_err();
        assert!(
            refused.contains("functions.exec, functions.wait"),
            "{refused}"
        );
        let weakening = judge(Some(&offered), None, allow(&[], false), ToolCheck::Warn)
            .unwrap()
            .unwrap();
        assert!(weakening.contains("functions.exec"), "{weakening}");
        assert_eq!(
            judge(Some(&[]), None, allow(&[], false), ToolCheck::Strict),
            Ok(None),
            "an answer-only thread passes with no tool at all"
        );
    }

    #[test]
    fn an_assistant_whose_server_codex_does_not_offer_is_refused() {
        let offered = offered_tools(ASSISTANT.as_bytes()).unwrap();
        let refused = judge(
            Some(&offered),
            None,
            allow(&moneypenny(), false),
            ToolCheck::Strict,
        )
        .unwrap_err();
        assert!(
            refused.contains("functions.exec, functions.wait")
                && refused.contains("no tool of MCP server moneypenny"),
            "{refused}"
        );
    }

    #[test]
    fn the_users_mcp_servers_but_the_threads_are_switched_off_by_name() {
        assert_eq!(
            mcp_server_names(MCP_LIST.as_bytes()).unwrap(),
            ["node_repl"]
        );
        let listed = mcp_server_names(MCP_LIST_ASSISTANT.as_bytes()).unwrap();
        assert_eq!(
            listed,
            ["moneypenny", "node_repl"],
            "the thread's server is listed beside the user's"
        );
        assert_eq!(
            switch_off_servers(&listed, &moneypenny()),
            (
                vec![
                    "-c".to_owned(),
                    "mcp_servers.node_repl.enabled=false".to_owned()
                ],
                Vec::new()
            )
        );
        assert_eq!(
            switch_off_servers(&listed, &[]).0,
            [
                "-c",
                "mcp_servers.moneypenny.enabled=false",
                "-c",
                "mcp_servers.node_repl.enabled=false"
            ]
        );
        let odd = [
            "we.ird".to_owned(),
            "sp ace".to_owned(),
            "ok-name_1".to_owned(),
        ];
        assert_eq!(
            switch_off_servers(&odd, &[]),
            (
                vec![
                    "-c".to_owned(),
                    "mcp_servers.ok-name_1.enabled=false".to_owned()
                ],
                vec!["\"we.ird\"".to_owned(), "\"sp ace\"".to_owned()]
            ),
            "a name Codex cannot address by a dotted key stays on and is reported"
        );
    }

    #[test]
    fn mcp_list_output_is_read_exactly_or_not_at_all() {
        assert_eq!(mcp_server_names(b"[]\n"), Some(Vec::new()));
        let mut rendered = b"\x1b[?25l\x1b]0;codex\x07\r\n".to_vec();
        rendered.extend_from_slice(MCP_LIST.replace('\n', "\r\n").as_bytes());
        assert_eq!(
            mcp_server_names(&rendered),
            mcp_server_names(MCP_LIST.as_bytes())
        );
        for output in [
            "",
            "No MCP servers configured yet. Try `codex mcp add my-tool -- my-command`.",
            "Error: failed to load bootstrap configuration\n\nCaused by:\n    invalid transport",
            "[{\"enabled\":true}]",
            "[{\"name\":7}]",
            "{\"name\":\"node_repl\"}",
            "[] trailing",
            "[{\"name\":\"a\"}] [{\"name\":\"b\"}]",
        ] {
            assert_eq!(mcp_server_names(output.as_bytes()), None, "{output}");
        }
    }

    #[test]
    fn extra_tools_are_refused_and_named_or_started_as_a_weakening() {
        let offered = offered_tools(CODE_MODE.as_bytes()).unwrap();
        assert!(offered.iter().any(|tool| tool == "mcp__moneypenny.board"));
        let refused = judge(
            Some(&offered),
            None,
            allow(&moneypenny(), false),
            ToolCheck::Strict,
        )
        .unwrap_err();
        assert!(
            refused.contains("functions.exec, functions.request_user_input"),
            "{refused}"
        );
        assert!(
            !refused.contains("board"),
            "the server's own tool is allowed"
        );
        let weakening = judge(
            Some(&offered),
            None,
            allow(&moneypenny(), false),
            ToolCheck::Warn,
        )
        .unwrap()
        .unwrap();
        assert!(weakening.contains("functions.exec"), "{weakening}");
    }

    /// `codex debug prompt-input` of Codex 0.164.0 on Windows for an assistant with scripts: the
    /// kept script features, the sandbox profile with `personal/` and `scratch/` writable and
    /// one MCP server, `moneypenny`; `exec`'s description whole.
    const SCRIPTS: &str = include_str!("../tests/fixtures/codex-0.164.0/prompt-input-scripts.json");

    #[test]
    fn a_scripts_thread_is_offered_code_mode_alone_and_passes_only_with_scripts() {
        let servers = moneypenny();
        let offered = offered_tools(SCRIPTS.as_bytes()).unwrap();
        let nested = exec_nested(SCRIPTS.as_bytes()).unwrap();
        assert_eq!(
            offered, CODE_MODE_TOOLS,
            "the server's tools are nested in exec"
        );
        assert_eq!(
            nested,
            [
                "apply_patch",
                "exec_command",
                "list_mcp_resource_templates",
                "list_mcp_resources",
                "read_mcp_resource",
                "write_stdin",
                "clock__curr_time"
            ],
            "the server's own tools are deferred, out of the description"
        );
        let judged = |offered: &[String], nested: &[String], allowed| {
            judge(Some(offered), Some(nested), allowed, ToolCheck::Strict)
        };
        assert_eq!(judged(&offered, &nested, allow(&servers, true)), Ok(None));
        let refused = judged(&offered, &nested, allow(&servers, false)).unwrap_err();
        assert!(
            refused.contains("functions.exec, functions.wait"),
            "{refused}"
        );
        let more = [
            offered.clone(),
            vec!["functions.request_user_input".to_owned()],
        ]
        .concat();
        let refused = judged(&more, &nested, allow(&servers, true)).unwrap_err();
        assert!(
            refused.contains("functions.request_user_input") && !refused.contains("exec"),
            "only code mode is admitted: {refused}"
        );
    }

    #[test]
    fn exec_nests_only_the_command_tools_the_patch_tool_with_files_and_the_servers() {
        let servers = moneypenny();
        let offered = offered_tools(SCRIPTS.as_bytes()).unwrap();
        let nested = exec_nested(SCRIPTS.as_bytes()).unwrap();
        let no_files = Allowed {
            servers: &servers,
            scripts: true,
            files: false,
        };
        let refused =
            judge(Some(&offered), Some(&nested), no_files, ToolCheck::Strict).unwrap_err();
        assert!(
            refused.contains("functions.exec beyond the thread's: apply_patch"),
            "{refused}"
        );

        // A new nested tool, declared as Codex 0.164.0 declares its own.
        let browser = "\\n\\n### `browser__open`\\nOpens a page.\\n\\nexec tool declaration:\\n\
                       ```ts\\ndeclare const tools: { browser__open(args: {}): Promise<unknown>; \
                       };\\n```";
        let at = SCRIPTS.find("\\n\\n## clock").unwrap();
        let widened = format!("{}{browser}{}", &SCRIPTS[..at], &SCRIPTS[at..]);
        let nested = exec_nested(widened.as_bytes()).unwrap();
        assert!(
            nested.iter().any(|tool| tool == "browser__open"),
            "{nested:?}"
        );
        let refused = judge(
            Some(&offered),
            Some(&nested),
            allow(&servers, true),
            ToolCheck::Strict,
        )
        .unwrap_err();
        assert!(
            refused.contains("beyond the thread's: browser__open"),
            "{refused}"
        );

        let allowed = allow(&servers, true);
        assert!(
            allowed.nests("mcp__moneypenny__board")
                && !allowed.nests("mcp__moneypenny_evil__board")
        );
        assert!(!allow(&[], true).nests("read_mcp_resource"));

        let cut = SCRIPTS.replacen("exec tool declaration:", "", 1);
        assert_eq!(
            exec_nested(cut.as_bytes()),
            None,
            "a declaration without its heading"
        );
        assert!(
            judge(
                Some(&offered),
                None,
                allow(&servers, true),
                ToolCheck::Strict
            )
            .unwrap_err()
            .contains("could not be read")
        );
        assert_eq!(
            exec_nested(ANSWER_ONLY.as_bytes()),
            None,
            "the older fixtures cut exec's description"
        );
        assert_eq!(exec_nested(b"[{\"type\":\"message\"}]"), Some(Vec::new()));
    }

    #[test]
    fn exactly_the_servers_tools_pass() {
        let offered = vec![
            "mcp__moneypenny.board".to_owned(),
            "mcp__moneypenny.missions".to_owned(),
        ];
        assert_eq!(
            judge(
                Some(&offered),
                None,
                allow(&moneypenny(), false),
                ToolCheck::Strict
            ),
            Ok(None)
        );
        let lookalike = vec!["mcp__moneypenny_evil.board".to_owned()];
        assert!(
            judge(
                Some(&lookalike),
                None,
                allow(&moneypenny(), false),
                ToolCheck::Strict
            )
            .is_err()
        );
    }

    #[test]
    fn unreadable_output_is_refused_unless_warned() {
        for output in [
            "",
            "Error: unknown command",
            "[{\"type\":\"hosted_tools\",\"tools\":[]}]",
            "[{\"type\":\"additional_tools\",\"tools\":[{\"type\":\"function\"}]}]",
            // No message: not a prompt.
            "[{\"type\":\"additional_tools\",\"tools\":[]}]",
            // Anything after the list.
            "[{\"type\":\"message\"}] [{\"type\":\"message\"}]",
            "[{\"type\":\"message\"}] trailing",
            // A list that does not open with an item.
            "[1, {\"type\":\"message\"}]",
        ] {
            let offered = offered_tools(output.as_bytes());
            assert_eq!(offered, None, "{output}");
            assert!(judge(None, None, allow(&[], false), ToolCheck::Strict).is_err());
            assert!(
                judge(None, None, allow(&[], false), ToolCheck::Warn)
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn prompt_input_reads_through_a_terminal_that_broke_its_lines() {
        let mut rendered = b"\x1b[?9001h\x1b[?1004h\x1b[m\x1b]0;codex\x07\x1b[?25h\r\n".to_vec();
        let text = CODE_MODE.replace('\n', "\r\n");
        let (head, tail) = text.split_at(text.find("V8 isolate").unwrap());
        rendered.extend_from_slice(head.as_bytes());
        rendered.extend_from_slice(b"\r\n\x1b[23;80H");
        rendered.extend_from_slice(tail.as_bytes());
        assert_eq!(
            offered_tools(&rendered),
            offered_tools(CODE_MODE.as_bytes())
        );
    }

    #[test]
    fn the_feature_list_names_what_is_on() {
        let list = feature_list(FEATURES.as_bytes()).unwrap();
        assert_eq!(list.unread, Vec::<String>::new(), "every line is a feature");
        let enabled = list.enabled;
        for feature in [
            "code_mode_host",
            "computer_use",
            "browser_use",
            "plugins",
            "hooks",
            "skill_search",
            "sleep_tool",
            "tool_suggest",
            "image_generation",
            "incremental_tools",
        ] {
            assert!(enabled.iter().any(|f| f == feature), "{feature}");
        }
        assert!(!enabled.iter().any(|f| f == "memories"), "off by default");
        assert!(
            KEPT_FEATURES.contains(&"incremental_tools"),
            "kept, or prompt-input renders no tool"
        );
        assert_eq!(feature_list(b"Error: no such command\n"), None);
        let noisy = format!("{FEATURES}\nWARNING: plugin cache is stale\n");
        assert_eq!(
            feature_list(noisy.as_bytes()).unwrap().unread,
            ["WARNING: plugin cache is stale"],
            "a line that is not a feature is kept, for the check to judge"
        );
    }

    #[test]
    fn the_check_choice_is_strict_unless_warn() {
        let with = |value: &str| {
            ToolCheck::from_environment(&BTreeMap::from([(
                TOOL_CHECK_ENV.to_owned(),
                value.to_owned(),
            )]))
        };
        assert_eq!(
            ToolCheck::from_environment(&BTreeMap::new()).unwrap(),
            ToolCheck::Strict
        );
        assert_eq!(with("warn").unwrap(), ToolCheck::Warn);
        assert!(with("off").is_err(), "no way to skip the check");
    }
}
