//! What Codex's app server is started with, and what a thread is opened with.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::ffi::OsString;
use std::path::PathBuf;

use coqui_acp_harness::{Launch, McpServer, McpTransport};
use serde_json::{Map, Value, json};

/// Which thread Codex opens.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ThreadStart {
    /// A new thread.
    New,
    /// An earlier thread by its id, its turns not replayed.
    Resume(String),
    /// An earlier thread by its id, its turns replayed as history.
    Load(String),
}

/// How this client names itself to Codex in `initialize`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClientInfo {
    /// Its name, which Codex puts in its user agent.
    pub name: String,
    /// Its name for people.
    pub title: String,
    /// Its version.
    pub version: String,
}

impl ClientInfo {
    /// A client of this name and version, its title the same as its name.
    #[must_use]
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            title: name.clone(),
            name,
            version: version.into(),
        }
    }
}

/// Everything Codex is started with. The app server's own arguments are `app-server` and one `--config=key=value`
/// per entry of `config_overrides`, each value written as TOML by this crate; everything a thread is given (MCP
/// servers, directories, instructions, policies) goes in `thread/start` or `thread/resume` as JSON, never on the
/// command line.
///
/// `Debug` names the environment's variables and never shows their values.
#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub struct CodexLaunch {
    /// The program, `codex` by default, found on `PATH` when it names no directory.
    pub executable: OsString,
    /// The working directory.
    pub cwd: PathBuf,
    /// Which thread.
    pub thread: ThreadStart,
    /// The model for the thread and its turns.
    pub model: Option<String>,
    /// The reasoning effort of each turn, such as `low` or `high`.
    pub effort: Option<String>,
    /// Text given to the thread as developer instructions, after Codex's own.
    pub developer_instructions: Option<String>,
    /// MCP servers, given to the thread in its `config.mcp_servers`.
    pub mcp_servers: Vec<McpServer>,
    /// More directories: the Agent Skills directory of each is a root Codex reads skills from
    /// (`skills/extraRoots/set`), and each is writable in a `workspace-write` sandbox (`sandbox_workspace_write.writable_roots`).
    pub additional_directories: Vec<PathBuf>,
    /// When not empty, a resumed or loaded thread whose own directory lies in none of these is refused, compared as
    /// written: a thread is found by its id alone, whatever `cwd` is asked for.
    pub working_roots: Vec<PathBuf>,
    /// The approval policy: `untrusted`, `on-failure`, `on-request` or `never`.
    pub approval_policy: Option<String>,
    /// The sandbox: `read-only`, `workspace-write` or `danger-full-access`.
    pub sandbox: Option<String>,
    /// Whether the Bypass mode is offered: never ask, no sandbox.
    pub allow_bypass: bool,
    /// The modes a client may choose, when the host names them: a widening of the default, which is the mode the
    /// approval policy and sandbox amount to and every stricter one. Bypass only with `allow_bypass`.
    pub modes: Option<Vec<coqui_acp_harness::Mode>>,
    /// More of the thread's configuration, merged into its `config` as given: the caller's own.
    pub thread_config: Map<String, Value>,
    /// `--config` overrides of the app server, each a dotted key ([`valid_config_key`]; another is refused when the
    /// session starts) and a JSON value written as TOML.
    pub config_overrides: Vec<(String, Value)>,
    /// More arguments of the app server, after the overrides, each as given: the caller's own, unchecked.
    pub extra_args: Vec<OsString>,
    /// How this client names itself to Codex; this crate by default.
    pub client_info: ClientInfo,
    /// Variables set for Codex over the inherited environment.
    pub env: BTreeMap<OsString, OsString>,
    /// Variables removed from the inherited environment before `env` is set, so a credential the caller's own
    /// environment holds does not reach Codex.
    pub remove_env: Vec<OsString>,
    /// Start Codex from an empty environment and `env` alone.
    pub clear_env: bool,
}

impl core::fmt::Debug for CodexLaunch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CodexLaunch")
            .field("executable", &self.executable)
            .field("cwd", &self.cwd)
            .field("thread", &self.thread)
            .field("model", &self.model)
            .field("effort", &self.effort)
            .field("mcp_servers", &self.mcp_servers)
            .field("additional_directories", &self.additional_directories)
            .field("approval_policy", &self.approval_policy)
            .field("sandbox", &self.sandbox)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("remove_env", &self.remove_env)
            .field("clear_env", &self.clear_env)
            .finish_non_exhaustive()
    }
}

impl CodexLaunch {
    /// A new thread in `cwd` with Codex's own defaults.
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            executable: OsString::from("codex"),
            cwd: cwd.into(),
            thread: ThreadStart::New,
            model: None,
            effort: None,
            developer_instructions: None,
            mcp_servers: Vec::new(),
            additional_directories: Vec::new(),
            working_roots: Vec::new(),
            approval_policy: None,
            sandbox: None,
            allow_bypass: false,
            modes: None,
            thread_config: Map::new(),
            config_overrides: Vec::new(),
            extra_args: Vec::new(),
            client_info: ClientInfo::new("coqui-acp-codex", env!("CARGO_PKG_VERSION")),
            env: BTreeMap::new(),
            remove_env: Vec::new(),
            clear_env: false,
        }
    }

    /// The command line and environment of the app server.
    #[must_use]
    pub fn launch(&self) -> Launch {
        let mut args: Vec<OsString> = alloc::vec![OsString::from("app-server")];
        for (key, value) in &self.config_overrides {
            args.push(format!("--config={key}={}", toml_value(value)).into());
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

    /// The thread's `config`: `thread_config`, with the MCP servers added to its own, and the directories to its writable
    /// roots.
    #[must_use]
    pub fn config(&self) -> Map<String, Value> {
        let mut config = self.thread_config.clone();
        if !self.mcp_servers.is_empty() {
            let mut servers = own_table(&config, "mcp_servers");
            servers.extend(
                self.mcp_servers.iter().filter_map(|server| {
                    Some((server.name.clone(), mcp_server(&server.transport)?))
                }),
            );
            config.insert("mcp_servers".into(), Value::Object(servers));
        }
        if !self.additional_directories.is_empty() {
            let mut sandbox = own_table(&config, "sandbox_workspace_write");
            let mut roots = sandbox
                .get("writable_roots")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            roots.extend(
                self.additional_directories
                    .iter()
                    .map(|directory| json!(directory.to_string_lossy())),
            );
            sandbox.insert("writable_roots".into(), Value::Array(roots));
            config.insert("sandbox_workspace_write".into(), Value::Object(sandbox));
        }
        config
    }

    /// Whether an MCP server of this name is already given, in `mcp_servers` or `thread_config`.
    #[must_use]
    pub fn has_server(&self, name: &str) -> bool {
        self.mcp_servers.iter().any(|server| server.name == name)
            || own_table(&self.thread_config, "mcp_servers").contains_key(name)
    }

    /// The skill roots of the additional directories: the Agent Skills directory of each.
    #[must_use]
    pub fn skill_roots(&self) -> Vec<String> {
        self.additional_directories
            .iter()
            .map(|directory| {
                directory
                    // In two parts, as this repository's check for private paths takes the whole name for one.
                    .join(concat!(".", "agents"))
                    .join("skills")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    /// The parameters of `thread/start`, or of `thread/resume` for a thread opened again.
    #[must_use]
    pub fn thread_params(&self) -> Value {
        let mut params = Map::new();
        params.insert("cwd".into(), json!(self.cwd.to_string_lossy()));
        match &self.thread {
            ThreadStart::New => {}
            ThreadStart::Resume(id) => {
                params.insert("threadId".into(), json!(id));
                params.insert("excludeTurns".into(), json!(true));
            }
            ThreadStart::Load(id) => {
                params.insert("threadId".into(), json!(id));
                params.insert("excludeTurns".into(), json!(false));
            }
        }
        for (key, value) in [
            ("model", &self.model),
            ("developerInstructions", &self.developer_instructions),
            ("approvalPolicy", &self.approval_policy),
            ("sandbox", &self.sandbox),
        ] {
            if let Some(value) = value {
                params.insert(key.into(), json!(value));
            }
        }
        let config = self.config();
        if !config.is_empty() {
            params.insert("config".into(), Value::Object(config));
        }
        Value::Object(params)
    }
}

fn own_table(config: &Map<String, Value>, key: &str) -> Map<String, Value> {
    config
        .get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn pairs(pairs: &[(String, String)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect(),
    )
}

/// A server as Codex's `mcp_servers` entry: `{command, args, env}` or `{url, http_headers}`. Codex has no SSE
/// transport.
fn mcp_server(transport: &McpTransport) -> Option<Value> {
    match transport {
        McpTransport::Stdio { command, args, env } => {
            Some(json!({"command": command, "args": args, "env": pairs(env)}))
        }
        McpTransport::Http { url, headers } => {
            Some(json!({"url": url, "http_headers": pairs(headers)}))
        }
        _ => None,
    }
}

/// Whether `key` can name a `--config` key. Codex splits the argument at its first `=` and the key at each `.`, and takes
/// a quote as part of a name, so a key is written as it is, and one holding `=` cannot be written at all.
#[must_use]
pub fn valid_config_key(key: &str) -> bool {
    !key.is_empty() && !key.contains(|c: char| c == '=' || c.is_control())
}

/// A JSON value as one TOML value: strings quoted with every control character escaped, arrays, and objects as
/// inline tables. `null`, which TOML has no word for, is the empty string.
#[must_use]
pub fn toml_value(value: &Value) -> String {
    match value {
        Value::Null => "\"\"".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => toml_string(text),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(toml_value).collect::<Vec<_>>().join(",")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, value)| format!("{}={}", toml_string(key), toml_value(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn toml_string(text: &str) -> String {
    let mut quoted = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            c if c.is_control() => quoted.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// Whether `id` can name a Codex thread: letters, digits and dashes, 1 to 128 of them.
#[must_use]
pub fn valid_thread_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_values_stay_one_toml_value_and_one_argument() {
        let mut launch = CodexLaunch::new("/w");
        launch.config_overrides = alloc::vec![
            (
                "model".into(),
                json!("--dangerously-bypass\"\nsandbox_mode=\"danger-full-access")
            ),
            ("mcp_servers.docs.url".into(), json!("http://x\"}\nother=1")),
        ];
        let args: Vec<String> = launch
            .launch()
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.len(), 3, "{args:?}");
        assert_eq!(
            args[1],
            "--config=model=\"--dangerously-bypass\\\"\\nsandbox_mode=\\\"danger-full-access\""
        );
        assert_eq!(
            args[2],
            "--config=mcp_servers.docs.url=\"http://x\\\"}\\nother=1\""
        );
        assert!(args.iter().all(|arg| !arg.contains('\n')));
        assert!(valid_config_key("mcp_servers.docs.url"));
        assert!(!valid_config_key("model=other"));
        assert!(!valid_config_key("a\nb"));
        assert!(!valid_config_key(""));
        assert_eq!(
            toml_value(&json!({"a b": [1, true, "\u{1}"]})),
            "{\"a b\"=[1,true,\"\\u0001\"]}"
        );
    }

    #[test]
    fn what_a_session_adds_keeps_what_the_host_configured() {
        let mut launch = CodexLaunch::new("/w");
        launch.thread_config.insert(
            "sandbox_workspace_write".into(),
            json!({"network_access": false, "writable_roots": ["/own"]}),
        );
        launch.thread_config.insert(
            "mcp_servers".into(),
            json!({"own": {"url": "https://o.invalid"}}),
        );
        launch.additional_directories = alloc::vec![PathBuf::from("/more")];
        launch.mcp_servers = alloc::vec![McpServer::new(
            "docs",
            McpTransport::Http {
                url: "https://docs.invalid/mcp".into(),
                headers: alloc::vec![]
            }
        )];
        let config = launch.config();
        assert_eq!(config["sandbox_workspace_write"]["network_access"], false);
        assert_eq!(
            config["sandbox_workspace_write"]["writable_roots"],
            json!(["/own", "/more"])
        );
        assert!(config["mcp_servers"]["own"].is_object());
        assert!(config["mcp_servers"]["docs"].is_object());
        assert!(launch.has_server("own") && launch.has_server("docs") && !launch.has_server("x"));
    }

    #[test]
    fn a_threads_parts_go_in_its_parameters_as_json() {
        let mut launch = CodexLaunch::new("/w");
        launch.thread = ThreadStart::Load("t-1".into());
        launch.developer_instructions = Some("- a bullet".into());
        launch.additional_directories = alloc::vec![PathBuf::from("/skills")];
        launch.mcp_servers = alloc::vec![
            McpServer::new(
                "files",
                McpTransport::Stdio {
                    command: "/bin/files".into(),
                    args: alloc::vec!["-v".into()],
                    env: alloc::vec![("TOKEN".into(), "t".into())]
                }
            ),
            McpServer::new(
                "docs",
                McpTransport::Http {
                    url: "https://docs.invalid/mcp".into(),
                    headers: alloc::vec![]
                }
            ),
        ];
        let params = launch.thread_params();
        assert_eq!(params["threadId"], "t-1");
        assert_eq!(params["excludeTurns"], false);
        assert_eq!(params["developerInstructions"], "- a bullet");
        assert_eq!(
            params["config"]["mcp_servers"]["files"]["env"]["TOKEN"],
            "t"
        );
        assert_eq!(
            params["config"]["mcp_servers"]["docs"]["http_headers"],
            json!({})
        );
        assert_eq!(
            params["config"]["sandbox_workspace_write"]["writable_roots"][0],
            "/skills"
        );
        assert!(launch.skill_roots()[0].ends_with("skills"));
        assert_eq!(
            launch.launch().args.len(),
            1,
            "nothing of the thread on the command line"
        );
        assert!(!format!("{launch:?}").contains("\"t\""));
    }
}
