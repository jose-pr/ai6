//! What Claude Code is started with, as its arguments and environment.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::ffi::OsString;
use std::path::PathBuf;

use crate::access::TARGET;
use coqui_acp_harness::{Access, Launch, McpServer, McpTransport};
use serde_json::{Map, Value, json};

/// Which session Claude Code opens.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionStart {
    /// A new session with this id, a UUID.
    New(String),
    /// A new session whose id Claude Code chooses and says with its first line of a turn.
    Chosen,
    /// An earlier session by its id, found only from the directory that recorded it.
    Resume(String),
}

/// Everything Claude Code is started with. Every value is passed as one `--flag=value` argument, so a value that
/// starts with `-` is never read as another option; the MCP servers go as one JSON value.
///
/// `Debug` names the environment's variables and never shows their values.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClaudeLaunch {
    /// The program, `claude` by default, found on `PATH` when it names no directory.
    pub executable: OsString,
    /// The working directory.
    pub cwd: PathBuf,
    /// Which session.
    pub session: SessionStart,
    /// The model, an alias such as `sonnet` or a full name.
    pub model: Option<String>,
    /// The effort: `low`, `medium`, `high`, `xhigh` or `max` in Claude Code 2.1.318.
    pub effort: Option<String>,
    /// Text appended to Claude Code's system prompt. However long, it never goes on the command line (a long argument
    /// stops a process starting): [`ClaudeSession::start`](crate::ClaudeSession::start) writes it to a file only this
    /// user can read and names that file as `append_system_prompt_file`.
    pub append_system_prompt: Option<String>,
    /// The file `--append-system-prompt-file` names, holding the text; `launch` passes the prompt only through it. Set
    /// it when the process does not run on this machine, as a [`ClaudeSession`](crate::ClaudeSession) writes its own
    /// file here otherwise; it then writes none, whatever `append_system_prompt` holds.
    pub append_system_prompt_file: Option<PathBuf>,
    /// MCP servers. Their configuration holds their environment and headers, which may be credentials, so it never
    /// goes on the command line: [`ClaudeSession::start`](crate::ClaudeSession::start) writes it to a file only this
    /// user can read and names that file as `mcp_config_file`.
    pub mcp_servers: Vec<McpServer>,
    /// The file `--mcp-config` names, holding [`mcp_config`] of `mcp_servers`; `launch` passes MCP servers only
    /// through it. Set it when the process does not run on this machine, as a [`ClaudeSession`](crate::ClaudeSession)
    /// writes its own file here otherwise; it then writes none, whatever `mcp_servers` holds.
    pub mcp_config_file: Option<PathBuf>,
    /// Only the servers above, none from the person's or the project's configuration (`--strict-mcp-config`).
    pub strict_mcp_config: bool,
    /// More directories Claude Code may work in (`--add-dir`).
    pub add_dirs: Vec<PathBuf>,
    /// Plugins to load from a directory, whose skills are named `<plugin>:<skill>` (`--plugin-dir`).
    pub plugin_dirs: Vec<PathBuf>,
    /// Ask for permission on standard output, as control requests (`--permission-prompts=host
    /// --permission-prompt-tool=stdio`). Off, a call nothing has granted is refused where it is made, and no question
    /// comes. `dontAsk` decides before these either way. Default on.
    pub permission_prompts: bool,
    /// The permission mode: `default`, `acceptEdits`, `plan`, `dontAsk`, `bypassPermissions`, `auto` or `manual`.
    pub permission_mode: Option<String>,
    /// The harness-neutral access a session starts with: [`ClaudeLaunch::effective_permission_mode`] turns its approvals
    /// into a permission mode (when `permission_mode` is not named), and its sandbox and network into Claude Code's sandbox
    /// settings, given with `--settings`.
    pub access: Option<Access>,
    /// Whether the session may be switched to `bypassPermissions` (`--allow-dangerously-skip-permissions`). Claude Code
    /// takes that mode mid-session only when it was started allowing it; off, the bypass mode is not offered.
    pub allow_bypass: bool,
    /// The modes a client may choose, when the host names them: a widening of the default, which is the launch's own
    /// mode and every stricter one. Bypass is offered only with `allow_bypass`, whatever this names.
    pub modes: Option<Vec<coqui_acp_harness::Mode>>,
    /// The setting sources Claude Code loads, of `user`, `project` and `local`; `Some` of an empty list loads none.
    /// `None` leaves Claude Code's own default, every source.
    pub setting_sources: Option<Vec<String>>,
    /// A spending ceiling, in millionths of a US dollar (`--max-budget-usd`).
    pub max_budget_micro_usd: Option<u64>,
    /// Stream text and thinking as they arrive (`--include-partial-messages`). Default on.
    pub partial_messages: bool,
    /// Variables set for Claude Code over the inherited environment.
    pub env: BTreeMap<OsString, OsString>,
    /// Variables removed from the inherited environment before `env` is set, so a credential the caller's own environment
    /// holds does not reach Claude Code.
    pub remove_env: Vec<OsString>,
    /// Start Claude Code from an empty environment and `env` alone.
    pub clear_env: bool,
    /// More arguments, after all the others, each as given: the caller's own, unchecked.
    pub extra_args: Vec<OsString>,
}

impl core::fmt::Debug for ClaudeLaunch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ClaudeLaunch")
            .field("executable", &self.executable)
            .field("cwd", &self.cwd)
            .field("session", &self.session)
            .field("model", &self.model)
            .field("effort", &self.effort)
            .field("append_system_prompt_file", &self.append_system_prompt_file)
            .field("mcp_servers", &self.mcp_servers)
            .field("mcp_config_file", &self.mcp_config_file)
            .field("add_dirs", &self.add_dirs)
            .field("plugin_dirs", &self.plugin_dirs)
            .field("permission_prompts", &self.permission_prompts)
            .field("permission_mode", &self.permission_mode)
            .field("access", &self.access)
            .field("allow_bypass", &self.allow_bypass)
            .field("modes", &self.modes)
            .field("setting_sources", &self.setting_sources)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("remove_env", &self.remove_env)
            .field("clear_env", &self.clear_env)
            .finish_non_exhaustive()
    }
}

impl ClaudeLaunch {
    /// A new session in `cwd` with Claude Code's defaults, its id a new UUID.
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            executable: OsString::from("claude"),
            cwd: cwd.into(),
            session: SessionStart::New(uuid::Uuid::new_v4().to_string()),
            model: None,
            effort: None,
            append_system_prompt: None,
            append_system_prompt_file: None,
            mcp_servers: Vec::new(),
            mcp_config_file: None,
            strict_mcp_config: false,
            add_dirs: Vec::new(),
            plugin_dirs: Vec::new(),
            permission_prompts: true,
            permission_mode: None,
            access: None,
            allow_bypass: false,
            modes: None,
            setting_sources: None,
            max_budget_micro_usd: None,
            partial_messages: true,
            env: BTreeMap::new(),
            remove_env: Vec::new(),
            clear_env: false,
            extra_args: Vec::new(),
        }
    }

    /// The session's id, the new one's or the resumed one's, unless Claude Code is to choose it.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        match &self.session {
            SessionStart::New(id) | SessionStart::Resume(id) => Some(id),
            SessionStart::Chosen => None,
        }
    }

    /// The permission mode the session starts in: the named one, else what the access amounts to: `plan` for a read-only
    /// sandbox, else `default` for Ask, `acceptEdits` for Edits and `bypassPermissions` for Never.
    #[must_use]
    pub fn effective_permission_mode(&self) -> Option<String> {
        if let Some(mode) = &self.permission_mode {
            return Some(mode.clone());
        }
        let mode = self.access?.translate(&TARGET).mode()?;
        Some(mode.to_string())
    }

    /// Claude Code's sandbox settings for the access, as the JSON `--settings` takes; `None` when it asks for none. A
    /// workspace sandbox turns Claude Code's own on, with no retry outside it. With the network off, only the hosts a
    /// settings file allows are reached and none is asked about (`strictAllowlist`, no `allowedDomains`); with no sandbox
    /// that is the sandbox's network side alone (`filesystem.disabled`). A read-only sandbox is the `plan` mode: Claude
    /// Code's sandbox has no switch that forbids every write.
    #[must_use]
    pub fn access_settings(&self) -> Option<Value> {
        let sandbox = self.access?.translate(&TARGET).json()?;
        Some(json!({ "sandbox": sandbox }))
    }

    /// The command line and environment: stream-json both ways on standard input and output, the person's messages
    /// echoed back, and permission prompts asked on standard output as control requests.
    #[must_use]
    pub fn launch(&self) -> Launch {
        let mut args: Vec<OsString> = [
            "-p",
            "--input-format=stream-json",
            "--output-format=stream-json",
            "--verbose",
            "--replay-user-messages",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        if self.permission_prompts {
            args.push("--permission-prompts=host".into());
            args.push("--permission-prompt-tool=stdio".into());
        }
        let permission_mode = self.effective_permission_mode();
        if self.allow_bypass || permission_mode.as_deref() == Some("bypassPermissions") {
            args.push("--allow-dangerously-skip-permissions".into());
        }
        if self.partial_messages {
            args.push("--include-partial-messages".into());
        }
        match &self.session {
            SessionStart::New(id) => args.push(format!("--session-id={id}").into()),
            SessionStart::Resume(id) => args.push(format!("--resume={id}").into()),
            SessionStart::Chosen => {}
        }
        let flag = |args: &mut Vec<OsString>, name: &str, value: &str| {
            args.push(format!("--{name}={value}").into());
        };
        if let Some(model) = &self.model {
            flag(&mut args, "model", model);
        }
        if let Some(effort) = &self.effort {
            flag(&mut args, "effort", effort);
        }
        if let Some(file) = &self.append_system_prompt_file {
            let mut arg = OsString::from("--append-system-prompt-file=");
            arg.push(file);
            args.push(arg);
        }
        if let Some(mode) = &permission_mode {
            flag(&mut args, "permission-mode", mode);
        }
        if let Some(settings) = self.access_settings() {
            flag(&mut args, "settings", &settings.to_string());
        }
        if let Some(sources) = &self.setting_sources {
            flag(&mut args, "setting-sources", &sources.join(","));
        }
        if let Some(micro) = self.max_budget_micro_usd {
            flag(
                &mut args,
                "max-budget-usd",
                &format!("{}.{:06}", micro / 1_000_000, micro % 1_000_000),
            );
        }
        if let Some(file) = &self.mcp_config_file {
            let mut arg = OsString::from("--mcp-config=");
            arg.push(file);
            args.push(arg);
        }
        if self.strict_mcp_config {
            args.push("--strict-mcp-config".into());
        }
        for directory in &self.add_dirs {
            let mut arg = OsString::from("--add-dir=");
            arg.push(directory);
            args.push(arg);
        }
        for directory in &self.plugin_dirs {
            let mut arg = OsString::from("--plugin-dir=");
            arg.push(directory);
            args.push(arg);
        }
        args.extend(self.extra_args.iter().cloned());
        let mut launch = Launch::new(self.executable.clone())
            .args(args)
            .cwd(self.cwd.clone())
            .clear_env(self.clear_env);
        launch.env.clone_from(&self.env);
        launch.remove_env.clone_from(&self.remove_env);
        launch
    }
}

/// The servers as Claude Code's MCP configuration: `{"mcpServers": {name: server}}`.
#[must_use]
pub fn mcp_config(servers: &[McpServer]) -> Value {
    let mut config = Map::new();
    let pairs = |pairs: &[(String, String)]| {
        Value::Object(
            pairs
                .iter()
                .map(|(name, value)| (name.clone(), json!(value)))
                .collect(),
        )
    };
    for server in servers {
        let value = match &server.transport {
            McpTransport::Stdio { command, args, env } => {
                json!({"type": "stdio", "command": command, "args": args, "env": pairs(env)})
            }
            McpTransport::Http { url, headers } => {
                json!({"type": "http", "url": url, "headers": pairs(headers)})
            }
            McpTransport::Sse { url, headers } => {
                json!({"type": "sse", "url": url, "headers": pairs(headers)})
            }
            _ => continue,
        };
        config.insert(server.name.clone(), value);
    }
    json!({ "mcpServers": config })
}

/// Whether `id` can name a Claude Code session: letters, digits and dashes, 1 to 128 of them. A session id reaches a
/// command line and a file name, so nothing else is taken.
#[must_use]
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(launch: &ClaudeLaunch) -> Vec<String> {
        launch
            .launch()
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// A model, effort or context starting with `-` stays the value of its own option.
    #[test]
    fn launch_option_values_are_never_read_as_options() {
        let mut launch = ClaudeLaunch::new("/w");
        launch.append_system_prompt_file = Some(PathBuf::from("--prompt.md"));
        launch.model = Some("--dangerously-skip-permissions".into());
        launch.effort = Some("-p".into());
        launch.add_dirs = vec![PathBuf::from("--x")];
        let args = args(&launch);
        for expected in [
            "--append-system-prompt-file=--prompt.md",
            "--model=--dangerously-skip-permissions",
            "--effort=-p",
            "--add-dir=--x",
        ] {
            assert!(
                args.iter().any(|arg| arg == expected),
                "{expected} in {args:?}"
            );
        }
        assert!(
            !args
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );
    }

    #[test]
    fn bypass_is_allowed_only_when_asked_for() {
        let mut launch = ClaudeLaunch::new("/w");
        assert!(
            !args(&launch)
                .iter()
                .any(|arg| arg == "--allow-dangerously-skip-permissions")
        );
        launch.allow_bypass = true;
        assert!(
            args(&launch)
                .iter()
                .any(|arg| arg == "--allow-dangerously-skip-permissions")
        );
    }

    #[test]
    fn every_part_reaches_its_flag() {
        let mut launch = ClaudeLaunch::new("/w");
        launch.session = SessionStart::Resume("claude-session-42".into());
        launch.setting_sources = Some(Vec::new());
        launch.plugin_dirs = vec![PathBuf::from("/tmp/p/canoa-profile")];
        launch.strict_mcp_config = true;
        launch.permission_mode = Some("acceptEdits".into());
        launch.mcp_servers = vec![McpServer::new(
            "files",
            McpTransport::Stdio {
                command: "/bin/files".into(),
                args: vec!["-v".into()],
                env: vec![("TOKEN".into(), "t".into())],
            },
        )];
        let args = args(&launch);
        for expected in [
            "--resume=claude-session-42",
            "--setting-sources=",
            "--plugin-dir=/tmp/p/canoa-profile",
            "--strict-mcp-config",
            "--permission-mode=acceptEdits",
            "--permission-prompt-tool=stdio",
            "--include-partial-messages",
        ] {
            assert!(
                args.iter().any(|arg| arg == expected),
                "{expected} in {args:?}"
            );
        }
        assert!(
            !args.iter().any(|arg| arg.starts_with("--mcp-config")),
            "servers go only through a file"
        );
        launch.mcp_config_file = Some(PathBuf::from("/tmp/mcp.json"));
        let args = super::tests::args(&launch);
        assert!(args.iter().any(|arg| arg == "--mcp-config=/tmp/mcp.json"));
        assert!(!args.iter().any(|arg| arg.contains("TOKEN")));
        let config = mcp_config(&launch.mcp_servers);
        assert_eq!(config["mcpServers"]["files"]["env"]["TOKEN"], "t");
        assert!(!format!("{launch:?}").contains("\"t\""));
    }

    /// A spending ceiling in millionths of a dollar, as dollars with six decimals.
    #[test]
    fn a_spend_ceiling_is_passed_as_max_budget_usd() {
        let mut launch = ClaudeLaunch::new("/w");
        launch.max_budget_micro_usd = Some(2_500_007);
        assert!(
            args(&launch)
                .iter()
                .any(|arg| arg == "--max-budget-usd=2.500007")
        );
        assert!(
            !args(&ClaudeLaunch::new("/w"))
                .iter()
                .any(|arg| arg.starts_with("--max-budget-usd"))
        );
    }

    #[test]
    fn permission_prompts_and_the_session_id_can_be_left_to_claude_code() {
        let mut launch = ClaudeLaunch::new("/w");
        assert!(
            args(&launch)
                .iter()
                .any(|arg| arg == "--permission-prompt-tool=stdio")
        );
        assert!(
            args(&launch)
                .iter()
                .any(|arg| arg.starts_with("--session-id="))
        );
        launch.permission_prompts = false;
        launch.session = SessionStart::Chosen;
        let args = args(&launch);
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("--permission-prompt")),
            "{args:?}"
        );
        assert!(
            !args.iter().any(|arg| arg.starts_with("--session-id")),
            "{args:?}"
        );
        assert_eq!(launch.session_id(), None);
    }

    #[test]
    fn a_session_id_is_letters_digits_and_dashes() {
        assert!(valid_session_id("6f2c1b0e-0000-4000-8000-00000000c0de"));
        for bad in ["", "../x", "a b", "a/b", "a\\b", "x\0"] {
            assert!(!valid_session_id(bad), "{bad:?}");
        }
    }
}
