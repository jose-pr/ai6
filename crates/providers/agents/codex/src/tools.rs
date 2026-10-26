//! The tool check of a restricted thread (answer-only or assistant).
//!
//! A Codex release can offer the model tools that no setting this handler passes switches off,
//! and a new release adds more. So before a restricted app server starts, the handler runs the
//! same Codex with the same settings, environment and working directory twice: `codex features
//! list`, to switch off every enabled feature not on [`KEPT_FEATURES`], then `codex debug
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
/// none of which offers the model a tool. Every other enabled feature is switched off.
pub const KEPT_FEATURES: [&str; 4] = [
    "secret_auth_storage",
    "enable_request_compression",
    "system_proxy_fallback",
    "unbounded_connection_retries",
];

/// How long a passed check stands for the same Codex and settings; the user's own Codex
/// configuration can change under it.
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

/// The enabled features in `codex features list` output (`name stage state` per line, the
/// stage possibly two words); `None` when no line reads that way.
pub(crate) fn enabled_features(output: &[u8]) -> Option<Vec<String>> {
    let text = String::from_utf8_lossy(&strip_controls(output)).replace('\r', "");
    let mut read = 0;
    let mut enabled = Vec::new();
    for line in text.lines() {
        let words = line.split_whitespace().collect::<Vec<_>>();
        let (Some(name), Some(state)) = (words.first(), words.last()) else {
            continue;
        };
        let named = name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.".contains(&byte)
        });
        if words.len() < 3 || !named || !matches!(*state, "true" | "false") {
            continue;
        }
        read += 1;
        if *state == "true" {
            enabled.push((*name).to_owned());
        }
    }
    (read > 0).then_some(enabled)
}

/// The tools `codex debug prompt-input` output offers the model, named `namespace.tool` inside
/// a namespace; `None` when the output is not the list of input items this handler knows
/// (messages and `additional_tools`), so an unknown shape is never read as "no tools".
pub(crate) fn offered_tools(output: &[u8]) -> Option<Vec<String>> {
    // The output is pretty-printed JSON, and a terminal may break a long line: JSON text holds
    // no raw line break, so every one can go.
    let mut text = strip_controls(output);
    text.retain(|byte| !matches!(byte, b'\r' | b'\n'));
    let start = text.iter().position(|byte| *byte == b'[')?;
    let items: Value = serde_json::Deserializer::from_slice(&text[start..])
        .into_iter::<Value>()
        .next()?
        .ok()?;
    let mut tools = Vec::new();
    for item in items.as_array()? {
        match item.get("type").and_then(Value::as_str)? {
            "message" => {}
            "additional_tools" => collect(item.get("tools")?.as_array()?, "", &mut tools)?,
            _ => return None,
        }
    }
    Some(tools)
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

/// Everything a check depends on but the working directory, which only names the project
/// marked untrusted.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CheckKey {
    executable: String,
    version: String,
    overrides: Vec<String>,
    model: Option<String>,
    servers: Vec<String>,
    check: ToolCheck,
}

fn passed() -> &'static Mutex<HashMap<CheckKey, (Instant, Checked)>> {
    static PASSED: OnceLock<Mutex<HashMap<CheckKey, (Instant, Checked)>>> = OnceLock::new();
    PASSED.get_or_init(Mutex::default)
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
    /// Runs the check, or takes the last passed one for the same Codex and settings.
    pub(crate) fn check(&self) -> Result<Checked, ProviderError> {
        let check = ToolCheck::from_environment(self.environment)?;
        let version = String::from_utf8_lossy(&strip_controls(
            &self.capture(&[self.executable.to_owned(), "--version".to_owned()])?,
        ))
        .trim()
        .to_owned();
        let key = CheckKey {
            executable: self.executable.to_owned(),
            version,
            overrides: self
                .overrides
                .iter()
                .filter(|value| !value.starts_with("projects="))
                .cloned()
                .collect(),
            model: self.model.map(str::to_owned),
            servers: self.servers.to_vec(),
            check,
        };
        if let Ok(passed) = passed().lock()
            && let Some((at, checked)) = passed.get(&key)
            && at.elapsed() < CHECK_LIFETIME
        {
            return Ok(checked.clone());
        }
        let checked = self.run(check)?;
        if let Ok(mut passed) = passed().lock() {
            passed.retain(|_, (at, _)| at.elapsed() < CHECK_LIFETIME);
            passed.insert(key, (Instant::now(), checked.clone()));
        }
        Ok(checked)
    }

    fn run(&self, check: ToolCheck) -> Result<Checked, ProviderError> {
        let mut argv = vec![
            self.executable.to_owned(),
            "features".to_owned(),
            "list".to_owned(),
        ];
        argv.extend_from_slice(self.overrides);
        let enabled = enabled_features(&self.capture(&argv)?);
        let mut weakenings = Vec::new();
        let overrides = match enabled {
            Some(enabled) => enabled
                .into_iter()
                .filter(|name| !KEPT_FEATURES.contains(&name.as_str()))
                .flat_map(|name| ["-c".to_owned(), format!("features.{name}=false")])
                .collect::<Vec<_>>(),
            None => {
                let found =
                    "the features Codex enables could not be read from `codex features list`";
                match check {
                    ToolCheck::Strict => {
                        return Err(refuse(
                            Status::GUATIAO_ERR_UNSUPPORTED,
                            format!(
                                "this restricted Codex thread was not started: {found} \
                                 ({TOOL_CHECK_ENV}=warn starts it anyway, as a weakening)"
                            ),
                        ));
                    }
                    ToolCheck::Warn => {
                        weakenings.push(found.to_owned());
                        Vec::new()
                    }
                }
            }
        };
        let mut argv = vec![
            self.executable.to_owned(),
            "debug".to_owned(),
            "prompt-input".to_owned(),
        ];
        argv.extend_from_slice(self.overrides);
        argv.extend_from_slice(&overrides);
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

    /// `codex debug prompt-input` and `codex features list` output of Codex 0.164.0 on
    /// Windows, model descriptions shortened and message text replaced (it holds the
    /// machine's paths): with the settings this handler passed before the tool check, with
    /// those it passes now (answer-only, and an assistant with one MCP server, `moneypenny`),
    /// and with code mode kept and that server's namespace exposed directly.
    const BEFORE: &str = include_str!("../tests/fixtures/codex-0.164.0/prompt-input-main.json");
    const ANSWER_ONLY: &str =
        include_str!("../tests/fixtures/codex-0.164.0/prompt-input-answer-only.json");
    const ASSISTANT: &str =
        include_str!("../tests/fixtures/codex-0.164.0/prompt-input-assistant.json");
    const CODE_MODE: &str =
        include_str!("../tests/fixtures/codex-0.164.0/prompt-input-code-mode.json");
    const FEATURES: &str = include_str!("../tests/fixtures/codex-0.164.0/features-list.txt");

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
    fn an_answer_only_thread_passes_with_no_tool_at_all() {
        let offered = offered_tools(ANSWER_ONLY.as_bytes()).unwrap();
        assert!(offered.is_empty(), "{offered:?}");
        assert_eq!(judge(Some(&offered), &[], ToolCheck::Strict), Ok(None));
    }

    #[test]
    fn an_assistant_whose_server_codex_does_not_offer_is_refused() {
        let offered = offered_tools(ASSISTANT.as_bytes()).unwrap();
        let refused = judge(Some(&offered), &moneypenny(), ToolCheck::Strict).unwrap_err();
        assert!(
            refused.contains("no tool of MCP server moneypenny"),
            "{refused}"
        );
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
        let enabled = enabled_features(FEATURES.as_bytes()).unwrap();
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
        assert_eq!(enabled_features(b"Error: no such command\n"), None);
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
