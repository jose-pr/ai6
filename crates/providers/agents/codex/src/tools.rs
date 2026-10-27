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
    // The output is pretty-printed JSON, and a terminal may break a long line: JSON text holds
    // no raw line break, so every one can go.
    let mut text = strip_controls(output);
    text.retain(|byte| !matches!(byte, b'\r' | b'\n'));
    let start = (0..text.len()).find(|&at| opens_item_list(&text[at..]))?;
    // One value, then only whitespace: `from_slice` refuses anything after it.
    let items: Value = serde_json::from_slice(&text[start..]).ok()?;
    let mut tools = Vec::new();
    let mut messages = 0;
    for item in items.as_array()? {
        match item.get("type").and_then(Value::as_str)? {
            "message" => messages += 1,
            "additional_tools" => collect(item.get("tools")?.as_array()?, "", &mut tools)?,
            _ => return None,
        }
    }
    (messages > 0).then_some(tools)
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

/// Judges the offered tools against the MCP servers a thread may use: each tool must belong
/// to one of them (`mcp__<server>.<tool>`) and each must offer at least one. `Ok(None)` passes;
/// `Ok(Some(found))` is a failure a `warn` check starts with; `Err` refuses.
pub(crate) fn judge(
    offered: Option<&[String]>,
    servers: &[String],
    check: ToolCheck,
) -> Result<Option<String>, String> {
    let found = match offered {
        None => {
            "the tools Codex offers could not be read from `codex debug prompt-input`".to_owned()
        }
        Some(offered) => {
            let allowed = |tool: &str| {
                servers.iter().any(|server| {
                    tool.strip_prefix(&format!("mcp__{server}."))
                        .is_some_and(|rest| !rest.is_empty())
                })
            };
            let extra = offered
                .iter()
                .filter(|tool| !allowed(tool))
                .cloned()
                .collect::<BTreeSet<_>>();
            let missing = servers
                .iter()
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
const CONFIG_ENVIRONMENT: [&str; 7] = [
    "CODEX_HOME",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CONFIG_HOME",
    "PATH",
];

/// What a feature list depends on but the working directory, which only names the project
/// marked untrusted.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FeaturesKey {
    executable: String,
    version: String,
    overrides: Vec<String>,
    environment: Vec<(String, Option<String>)>,
    check: ToolCheck,
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
                .filter(|value| !value.starts_with("projects="))
                .cloned()
                .collect(),
            environment: CONFIG_ENVIRONMENT
                .iter()
                .map(|name| ((*name).to_owned(), self.environment.get(*name).cloned()))
                .collect(),
            check,
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
        let offered = offered_tools(&self.capture(&argv)?);
        if let Some(found) = judge(offered.as_deref(), self.servers, check)
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
        let refused = judge(Some(&offered), &[], ToolCheck::Strict).unwrap_err();
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
        let refused = judge(Some(&offered), &[], ToolCheck::Strict).unwrap_err();
        assert!(
            refused.contains("functions.exec, functions.wait"),
            "{refused}"
        );
        let weakening = judge(Some(&offered), &[], ToolCheck::Warn)
            .unwrap()
            .unwrap();
        assert!(weakening.contains("functions.exec"), "{weakening}");
        assert_eq!(
            judge(Some(&[]), &[], ToolCheck::Strict),
            Ok(None),
            "an answer-only thread passes with no tool at all"
        );
    }

    #[test]
    fn an_assistant_whose_server_codex_does_not_offer_is_refused() {
        let offered = offered_tools(ASSISTANT.as_bytes()).unwrap();
        let refused = judge(Some(&offered), &moneypenny(), ToolCheck::Strict).unwrap_err();
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
        let refused = judge(Some(&offered), &moneypenny(), ToolCheck::Strict).unwrap_err();
        assert!(
            refused.contains("functions.exec, functions.request_user_input"),
            "{refused}"
        );
        assert!(
            !refused.contains("board"),
            "the server's own tool is allowed"
        );
        let weakening = judge(Some(&offered), &moneypenny(), ToolCheck::Warn)
            .unwrap()
            .unwrap();
        assert!(weakening.contains("functions.exec"), "{weakening}");
    }

    #[test]
    fn exactly_the_servers_tools_pass() {
        let offered = vec![
            "mcp__moneypenny.board".to_owned(),
            "mcp__moneypenny.missions".to_owned(),
        ];
        assert_eq!(
            judge(Some(&offered), &moneypenny(), ToolCheck::Strict),
            Ok(None)
        );
        let lookalike = vec!["mcp__moneypenny_evil.board".to_owned()];
        assert!(judge(Some(&lookalike), &moneypenny(), ToolCheck::Strict).is_err());
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
            assert!(judge(None, &[], ToolCheck::Strict).is_err());
            assert!(judge(None, &[], ToolCheck::Warn).unwrap().is_some());
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
