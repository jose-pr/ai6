//! Per-Session authenticated MCP tools that act on a Theater through its transport.
//!
//! In remote-hands mode the harness runs on the Station and its own shell and file tools
//! are turned off, so everything it does to the Theater goes through these tools and the
//! Theater's transport. Paths are the Theater's; a relative path is resolved against the
//! Mission's working directory there. Every `theater_run` command passes the green ink gate
//! first, asking the Theater's repository through the same transport: the Agent cannot route
//! around it, because this is the only way it runs anything.

use std::sync::Arc;
use std::time::Duration;

use axum::middleware;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::{StreamableHttpServerConfig, StreamableHttpService},
};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::cables_mcp::authorize;
use crate::directorates::m::approvals::{ActionScope, classify};
use crate::directorates::q::transport::{
    Capabilities, ReadSpec, RunSpec, TransportClient, WriteSpec, describe,
};
use crate::directorates::section_00::gate::{
    Answers, GateVerdict, GitOutput, GitRepository, decide,
};
use guatiao::Bytes;

/// Command output kept per stream.
const MAX_OUTPUT: u64 = 256 * 1024;
/// File bytes returned per read unless the caller asks for less.
const MAX_READ: u64 = 256 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 3600;
/// How long one git question about the Theater's repository may take.
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Asks the Station whether a Mission's Agent may take steps now (`check_approval`).
pub(super) type StepCheck = Arc<dyn Fn(&[ActionScope]) -> Result<Answers, String> + Send + Sync>;

/// A local MCP listener bound to one Mission's Theater.
pub(super) struct Endpoint {
    token: String,
    url: String,
    server: JoinHandle<()>,
}

impl Endpoint {
    /// Starts the listener. `root` is the Mission's working directory on the Theater.
    pub(super) async fn start(
        client: Arc<TransportClient>,
        capabilities: Capabilities,
        root: Option<String>,
        check: StepCheck,
    ) -> std::io::Result<Self> {
        let token = Uuid::new_v4().to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let service = StreamableHttpService::<TheaterTools, LocalSessionManager>::new(
            move || {
                Ok(TheaterTools::new(
                    client.clone(),
                    capabilities.clone(),
                    root.clone(),
                    check.clone(),
                ))
            },
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );
        let router = axum::Router::new()
            .route_service("/mcp", service)
            .layer(middleware::from_fn_with_state(token.clone(), authorize));
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self {
            token,
            url: format!("http://{address}/mcp"),
            server,
        })
    }

    pub(super) fn add_environment(
        &self,
        environment: &mut std::collections::BTreeMap<String, String>,
    ) {
        environment.insert("AI6_THEATER_MCP_URL".to_owned(), self.url.clone());
        environment.insert("AI6_THEATER_MCP_TOKEN".to_owned(), self.token.clone());
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// What the harness is told about its tools and where it is working.
pub(super) fn instructions(root: Option<&str>, capabilities: &Capabilities) -> String {
    let place = match root {
        Some(root) => format!("Relative paths are resolved against {root} on the Theater."),
        None => "Relative paths are resolved against the Theater's login directory.".to_owned(),
    };
    let files = if capabilities.files {
        "theater_read, theater_write and theater_list read, write and list files there. "
    } else {
        "This Theater's transport has no file access: use theater_run for files. "
    };
    format!(
        "Your work is on a remote Theater, not on this machine. Your own shell and file tools are \
         disabled; act on the Theater with the AI6 Theater tools. theater_run runs a command \
         there. {files}{place} Everything under your working directory on this machine is \
         scratch space the Theater never sees."
    )
}

#[derive(Clone)]
struct TheaterTools {
    client: Arc<TransportClient>,
    capabilities: Capabilities,
    root: Option<String>,
    check: StepCheck,
    tool_router: ToolRouter<Self>,
}

impl TheaterTools {
    fn new(
        client: Arc<TransportClient>,
        capabilities: Capabilities,
        root: Option<String>,
        check: StepCheck,
    ) -> Self {
        Self {
            client,
            capabilities,
            root,
            check,
            tool_router: Self::tool_router(),
        }
    }

    /// The Theater path a tool argument names.
    fn resolve(&self, path: &str) -> String {
        resolve(self.root.as_deref(), path)
    }

    /// Runs a blocking transport call off the async workers.
    async fn blocking<T: Send + 'static>(
        &self,
        call: impl FnOnce(&TransportClient) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let client = self.client.clone();
        tokio::task::spawn_blocking(move || call(&client))
            .await
            .map_err(|error| format!("the transport call failed: {error}"))?
    }
}

/// Resolves `path` against `root` when it is relative.
fn resolve(root: Option<&str>, path: &str) -> String {
    let absolute = path.starts_with('/')
        || path.starts_with('~')
        || path.starts_with('\\')
        || path.as_bytes().get(1) == Some(&b':');
    match root {
        Some(root) if !absolute => {
            if path.is_empty() || path == "." {
                root.to_owned()
            } else {
                format!("{}/{path}", root.trim_end_matches('/'))
            }
        }
        _ => path.to_owned(),
    }
}

/// Runs `git` with `args` on the Theater.
fn theater_git(client: &TransportClient, args: &[String]) -> Result<GitOutput, String> {
    let mut argv = vec!["git".to_owned()];
    argv.extend(args.iter().cloned());
    let spec = RunSpec {
        argv,
        timeout_ms: GIT_TIMEOUT.as_millis() as u64,
        max_output: MAX_OUTPUT,
        ..RunSpec::default()
    };
    let output = client.run(&spec).map_err(|error| describe(&error))?;
    Ok((
        output.code == Some(0) && !output.timed_out,
        String::from_utf8_lossy(&output.stdout.0).into_owned(),
    ))
}

/// `argv` as one command line the classifier reads like a shell.
fn command_line(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn failure(message: impl std::fmt::Display) -> String {
    serde_json::json!({ "error": message.to_string() }).to_string()
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TheaterTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(instructions(self.root.as_deref(), &self.capabilities))
    }
}

#[derive(Deserialize, JsonSchema)]
struct RunArgs {
    /// The command line to run, read by `sh -c` on the Theater.
    command: Option<String>,
    /// A program and its arguments, run without a shell. Use this or `command`.
    argv: Option<Vec<String>>,
    /// Directory to run in; defaults to the Mission's working directory on the Theater.
    cwd: Option<String>,
    /// Seconds before the command is killed (default 120, at most 3600).
    timeout_secs: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
struct ReadArgs {
    /// Path of the file on the Theater.
    path: String,
    /// First byte to read (default 0).
    offset: Option<u64>,
    /// Most bytes to return (default and maximum 262144).
    limit: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
struct WriteArgs {
    /// Path of the file on the Theater.
    path: String,
    /// The text to write (UTF-8).
    content: String,
    /// Add to the end of the file instead of replacing it.
    #[serde(default)]
    append: bool,
    /// Create missing parent directories.
    #[serde(default)]
    create_dirs: bool,
}

#[derive(Deserialize, JsonSchema)]
struct ListArgs {
    /// Directory on the Theater; defaults to the Mission's working directory.
    path: Option<String>,
}

#[tool_router]
impl TheaterTools {
    /// Run a command on the Theater.
    #[tool(
        description = "Run a command on the remote Theater and return its exit code, stdout and stderr. Give `command` (read by sh -c) or `argv` (no shell)."
    )]
    async fn theater_run(&self, Parameters(args): Parameters<RunArgs>) -> String {
        if !self.capabilities.run {
            return failure("this Theater's transport cannot run commands");
        }
        let (argv, line) = match (args.command, args.argv) {
            (Some(command), None) => (
                vec!["sh".to_owned(), "-c".to_owned(), command.clone()],
                command,
            ),
            (None, Some(argv)) if !argv.is_empty() => {
                let line = command_line(&argv);
                (argv, line)
            }
            _ => return failure("give exactly one of `command` and a non-empty `argv`"),
        };
        let spec = RunSpec {
            argv,
            cwd: Some(self.resolve(args.cwd.as_deref().unwrap_or("."))).filter(|cwd| cwd != "."),
            timeout_ms: Duration::from_secs(
                args.timeout_secs
                    .unwrap_or(DEFAULT_TIMEOUT_SECS)
                    .clamp(1, MAX_TIMEOUT_SECS),
            )
            .as_millis() as u64,
            max_output: MAX_OUTPUT,
            ..RunSpec::default()
        };
        let check = self.check.clone();
        let base = spec.cwd.clone().or_else(|| self.root.clone());
        let verdict = self
            .blocking(move |client| {
                let repository =
                    GitRepository::new(base, |args: &[String]| theater_git(client, args));
                Ok(decide(classify(&line, &repository), |steps| check(steps)))
            })
            .await;
        match verdict {
            Ok(GateVerdict::Allow) => {}
            Ok(GateVerdict::Hold(reason)) => return failure(reason),
            Err(error) => return failure(format!("Green ink: the command is held: {error}")),
        }
        match self
            .blocking(move |client| client.run(&spec).map_err(|error| describe(&error)))
            .await
        {
            Ok(output) => serde_json::json!({
                "exit_code": output.code,
                "signal": output.signal,
                "timed_out": output.timed_out,
                "truncated": output.truncated,
                "stdout": String::from_utf8_lossy(&output.stdout.0),
                "stderr": String::from_utf8_lossy(&output.stderr.0),
            })
            .to_string(),
            Err(error) => failure(error),
        }
    }

    /// Read a text file on the Theater.
    #[tool(description = "Read part of a UTF-8 text file on the remote Theater.")]
    async fn theater_read(&self, Parameters(args): Parameters<ReadArgs>) -> String {
        if !self.capabilities.files {
            return failure("this Theater's transport has no file access; use theater_run");
        }
        let path = self.resolve(&args.path);
        let spec = ReadSpec {
            path: path.clone(),
            offset: args.offset.unwrap_or(0),
            limit: args.limit.unwrap_or(MAX_READ).min(MAX_READ),
        };
        let offset = spec.offset;
        match self
            .blocking(move |client| client.read_file(&spec).map_err(|error| describe(&error)))
            .await
        {
            Ok(data) => match String::from_utf8(data.data.0) {
                Ok(content) => serde_json::json!({
                    "path": path,
                    "size": data.size,
                    "truncated": offset + (content.len() as u64) < data.size,
                    "content": content,
                })
                .to_string(),
                Err(_) => failure("the file is not UTF-8 text at that offset"),
            },
            Err(error) => failure(error),
        }
    }

    /// Write a text file on the Theater.
    #[tool(
        description = "Write a UTF-8 text file on the remote Theater, replacing it or appending."
    )]
    async fn theater_write(&self, Parameters(args): Parameters<WriteArgs>) -> String {
        if !self.capabilities.files {
            return failure("this Theater's transport has no file access; use theater_run");
        }
        let path = self.resolve(&args.path);
        let written = args.content.len();
        let spec = WriteSpec {
            path: path.clone(),
            data: Bytes(args.content.into_bytes()),
            append: args.append,
            create_dirs: args.create_dirs,
        };
        match self
            .blocking(move |client| client.write_file(&spec).map_err(|error| describe(&error)))
            .await
        {
            Ok(()) => serde_json::json!({ "path": path, "bytes_written": written }).to_string(),
            Err(error) => failure(error),
        }
    }

    /// List a directory on the Theater.
    #[tool(
        description = "List a directory on the remote Theater: name, kind (file, dir, link, other) and size."
    )]
    async fn theater_list(&self, Parameters(args): Parameters<ListArgs>) -> String {
        if !self.capabilities.files {
            return failure("this Theater's transport has no file access; use theater_run");
        }
        let path = self.resolve(args.path.as_deref().unwrap_or("."));
        let listed = path.clone();
        match self
            .blocking(move |client| client.list_dir(&listed).map_err(|error| describe(&error)))
            .await
        {
            Ok(listing) => serde_json::json!({
                "path": path,
                "entries": listing.entries.iter().map(|entry| serde_json::json!({
                    "name": entry.name, "kind": entry.kind, "size": entry.size,
                })).collect::<Vec<_>>(),
            })
            .to_string(),
            Err(error) => failure(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, ContentBlock};
    use rmcp::transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    };

    use super::*;
    use crate::station::transports::testing::DirTransport;

    /// A Station whose policy lets the Agent take every step.
    fn unchecked() -> StepCheck {
        Arc::new(|steps| {
            Ok(steps
                .iter()
                .map(|_| ("not_required".to_owned(), None))
                .collect())
        })
    }

    async fn call(
        client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
        tool: &'static str,
        arguments: serde_json::Value,
    ) -> serde_json::Value {
        let response = client
            .call_tool(
                CallToolRequestParams::new(tool)
                    .with_arguments(serde_json::from_value(arguments).unwrap()),
            )
            .await
            .unwrap();
        let [ContentBlock::Text(text)] = response.content.as_slice() else {
            panic!("a tool answers one text block");
        };
        serde_json::from_str(&text.text).unwrap()
    }

    async fn connect(endpoint: &Endpoint) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
        let denied = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(endpoint.url.clone()),
        );
        assert!(
            ().serve(denied).await.is_err(),
            "the endpoint needs its token"
        );
        ().serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(endpoint.url.clone())
                .auth_header(endpoint.token.clone()),
        ))
        .await
        .unwrap()
    }

    #[test]
    fn relative_paths_resolve_against_the_missions_directory_on_the_theater() {
        assert_eq!(resolve(Some("/srv/work"), "a/b.txt"), "/srv/work/a/b.txt");
        assert_eq!(resolve(Some("/srv/work/"), "a"), "/srv/work/a");
        assert_eq!(resolve(Some("/srv/work"), "."), "/srv/work");
        assert_eq!(resolve(Some("/srv/work"), "/etc/hosts"), "/etc/hosts");
        assert_eq!(resolve(Some("/srv/work"), "~/x"), "~/x");
        assert_eq!(resolve(Some("C:/work"), "D:/x"), "D:/x");
        assert_eq!(resolve(None, "a"), "a");
    }

    #[tokio::test]
    async fn the_tools_act_on_the_theater_and_nothing_lands_on_the_station() {
        let theater = tempfile::tempdir().unwrap();
        let station = tempfile::tempdir().unwrap();
        let transport = DirTransport::new(theater.path());
        let runs = transport.runs();
        let capabilities = transport.capabilities_model();
        let client = Arc::new(TransportClient::new(Box::new(transport)));
        let endpoint = Endpoint::start(client, capabilities, Some("/work".to_owned()), unchecked())
            .await
            .unwrap();
        let tools = connect(&endpoint).await;

        let mut names = tools
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            [
                "theater_list",
                "theater_read",
                "theater_run",
                "theater_write"
            ]
        );

        let written = call(
            &tools,
            "theater_write",
            serde_json::json!({"path": "src/main.rs", "content": "fn main() {}\n", "create_dirs": true}),
        )
        .await;
        assert_eq!(written["path"], "/work/src/main.rs");
        assert!(theater.path().join("work/src/main.rs").is_file());
        assert_eq!(
            std::fs::read_dir(station.path()).unwrap().count(),
            0,
            "the Station's disk is untouched"
        );

        let read = call(
            &tools,
            "theater_read",
            serde_json::json!({"path": "src/main.rs"}),
        )
        .await;
        assert_eq!(read["content"], "fn main() {}\n");
        assert_eq!(read["truncated"], false);
        let part = call(
            &tools,
            "theater_read",
            serde_json::json!({"path": "src/main.rs", "offset": 3, "limit": 4}),
        )
        .await;
        assert_eq!(
            (part["content"].as_str(), part["truncated"].as_bool()),
            (Some("main"), Some(true))
        );

        let listing = call(&tools, "theater_list", serde_json::json!({"path": "src"})).await;
        assert_eq!(listing["entries"][0]["name"], "main.rs");
        assert_eq!(listing["entries"][0]["kind"], "file");

        let output = call(
            &tools,
            "theater_run",
            serde_json::json!({"command": "cargo test", "timeout_secs": 5}),
        )
        .await;
        assert_eq!(output["exit_code"], 0);
        let ran = runs.lock().unwrap().clone();
        assert_eq!(ran.len(), 1);
        assert_eq!(ran[0].argv, ["sh", "-c", "cargo test"]);
        assert_eq!(ran[0].cwd.as_deref(), Some("/work"));
        assert_eq!(ran[0].timeout_ms, 5000);

        let bad = call(&tools, "theater_run", serde_json::json!({})).await;
        assert!(bad["error"].as_str().unwrap().contains("exactly one"));
        let missing = call(
            &tools,
            "theater_read",
            serde_json::json!({"path": "nope.txt"}),
        )
        .await;
        assert!(missing["error"].is_string());
        tools.cancel().await.unwrap();
    }

    #[tokio::test]
    async fn a_transport_without_files_points_the_harness_at_theater_run() {
        let theater = tempfile::tempdir().unwrap();
        let transport = DirTransport::new(theater.path()).without_files();
        let capabilities = transport.capabilities_model();
        assert!(instructions(None, &capabilities).contains("use theater_run for files"));
        let client = Arc::new(TransportClient::new(Box::new(transport)));
        let endpoint = Endpoint::start(client, capabilities, None, unchecked())
            .await
            .unwrap();
        let tools = connect(&endpoint).await;
        let refused = call(&tools, "theater_read", serde_json::json!({"path": "a"})).await;
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("no file access")
        );
        let ran = call(&tools, "theater_run", serde_json::json!({"argv": ["ls"]})).await;
        assert_eq!(ran["exit_code"], 0);
        tools.cancel().await.unwrap();
    }
}
