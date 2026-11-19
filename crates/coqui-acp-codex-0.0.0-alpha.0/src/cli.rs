//! Codex's agent as library code: its arguments as a `clap::Args` type and a [`run`] that serves ACP on standard input
//! and output. A program that people run is the host's; `examples/serve_codex.rs` is one for this crate's tests.

#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use crate::harness::{Settings, serve};
use crate::{CodexAgent, CodexLaunch};

/// Serves Codex as an Agent Client Protocol agent on standard input and output, in ACP version 1 or 2, whichever the
/// client asks for. Each session runs `codex app-server` with one thread; approvals are asked through ACP.
/// Diagnostics go to standard error.
///
/// Codex reads its own variables (`CODEX_HOME`, `OPENAI_API_KEY` and others) from the environment it is given.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The Codex program.
    #[arg(long, env = "COQUI_ACP_CODEX_EXECUTABLE", default_value = "codex")]
    pub executable: OsString,
    /// The model every session starts with, until the client chooses another.
    #[arg(long)]
    pub model: Option<String>,
    /// The reasoning effort every session starts with, such as low or high.
    #[arg(long)]
    pub effort: Option<String>,
    /// The approval policy: untrusted, on-failure, on-request or never.
    #[arg(long)]
    pub approval_policy: Option<String>,
    /// Offer the Bypass mode: never ask, and no sandbox.
    #[arg(long)]
    pub allow_bypass: bool,
    /// The sandbox: read-only, workspace-write or danger-full-access.
    #[arg(long)]
    pub sandbox: Option<String>,
    /// The highest ACP version answered.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=2))]
    pub max_acp_version: u8,
    /// Sessions at once.
    #[arg(long, default_value_t = 8)]
    pub max_sessions: usize,
    /// A directory every session's `cwd` and more directories must lie in, repeated for more; none takes any.
    #[arg(long = "working-root", value_name = "DIRECTORY")]
    pub working_roots: Vec<PathBuf>,
    /// Refuse a session that a client gives MCP servers.
    #[arg(long)]
    pub no_client_mcp_servers: bool,
    /// Take a network path (`\\\\host\\share`) as a session's `cwd` or more directories.
    #[arg(long)]
    pub allow_network_paths: bool,
}

impl Args {
    /// The settings the harness serves with.
    #[must_use]
    pub fn settings(&self) -> Settings {
        let mut settings = Settings::new();
        settings.max_version = self.max_acp_version;
        settings.max_sessions = self.max_sessions;
        settings.working_roots.clone_from(&self.working_roots);
        settings.client_mcp_servers = !self.no_client_mcp_servers;
        settings.network_paths = self.allow_network_paths;
        settings.on_notice = Some(Arc::new(|_, text: &str| {
            eprintln!("coqui-acp-codex: {text}");
        }));
        settings
    }

    /// The agent, with these arguments as what each session starts with.
    #[must_use]
    pub fn agent(&self) -> CodexAgent {
        let mut defaults = CodexLaunch::new(".");
        defaults.executable.clone_from(&self.executable);
        defaults.model.clone_from(&self.model);
        defaults.effort.clone_from(&self.effort);
        defaults.approval_policy.clone_from(&self.approval_policy);
        defaults.sandbox.clone_from(&self.sandbox);
        defaults.allow_bypass = self.allow_bypass;
        CodexAgent::new().defaults(defaults)
    }
}

/// Serves the agent on standard input and output until the client closes it.
///
/// # Errors
///
/// The connection failed; the message says how.
pub fn run(args: &Args) -> Result<u8, String> {
    serve(
        Arc::new(args.agent()),
        args.settings(),
        std::io::stdin(),
        std::io::stdout(),
    )
    .map(|()| 0)
    .map_err(|error| error.to_string())
}

/// What the agent's history commands do: move a session in and out of the standard history format
/// (`docs/session-history.md`).
#[cfg(feature = "history")]
#[derive(Debug, clap::Subcommand)]
pub enum History {
    /// Writes the session's history to standard output, as it is.
    Export {
        /// Codex's home directory; by default `CODEX_HOME`, else `.codex` in the home directory.
        #[arg(long)]
        config: Option<PathBuf>,
        /// The thread's UUID.
        id: String,
    },
    /// Reads a history from standard input and writes it as a session Codex resumes. Prints what could not
    /// be carried over on standard error; never replaces a session.
    Import {
        /// Codex's home directory; by default `CODEX_HOME`, else `.codex` in the home directory.
        #[arg(long)]
        config: Option<PathBuf>,
        /// The directory the thread works in.
        cwd: PathBuf,
        /// The thread's UUID.
        id: String,
        /// Seed from the first line of the history, not from its latest compaction summary.
        #[arg(long)]
        no_summary: bool,
        /// The model provider the thread names, which Codex resumes only under the same one (default `openai`).
        #[arg(long)]
        model_provider: Option<String>,
    },
}

/// Runs one history command on standard input and output.
///
/// # Errors
///
/// The home directory is unknown, or the session or the history could not be read or written.
#[cfg(feature = "history")]
pub fn run_history(action: &History) -> Result<u8, String> {
    use std::io::{Read, Write};

    use crate::history::{DEFAULT_MAX_BYTES, ImportOptions, export_file, import_file};

    let place = |given: &Option<PathBuf>| {
        given
            .clone()
            .or_else(|| crate::history::codex_home(&std::collections::BTreeMap::new(), false))
            .ok_or_else(|| "the home directory is unknown: pass --config".to_owned())
    };
    match action {
        History::Export { config, id } => {
            let config = place(config)?;
            let doc =
                export_file(&config, id, DEFAULT_MAX_BYTES).map_err(|error| error.to_string())?;
            let bytes = doc.to_vec().map_err(|error| error.to_string())?;
            std::io::stdout()
                .write_all(&bytes)
                .map_err(|error| error.to_string())?;
            Ok(0)
        }
        History::Import {
            config,
            cwd,
            id,
            no_summary,
            model_provider,
        } => {
            let config = place(config)?;
            let mut text = Vec::new();
            std::io::stdin()
                .read_to_end(&mut text)
                .map_err(|error| error.to_string())?;
            let doc = coqui_acp_history::Document::from_slice(&text)
                .map_err(|error| error.to_string())?;
            let options = ImportOptions::default().use_summary(!no_summary);
            let options = match model_provider {
                Some(provider) => options.model_provider(provider.clone()),
                None => options,
            };
            let imported =
                import_file(&config, cwd, id, &doc, &options).map_err(|error| error.to_string())?;
            let lost: Vec<String> = imported
                .report
                .losses()
                .iter()
                .map(|loss| format!("{}:{}", loss.kind, loss.count))
                .collect();
            eprintln!("imported; lost: [{}]", lost.join(", "));
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::Args;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn what_a_host_narrows_on_the_command_line_reaches_the_settings() {
        let settings = Cli::parse_from([
            "codex",
            "--working-root",
            "/w",
            "--working-root",
            "/x",
            "--no-client-mcp-servers",
        ])
        .args
        .settings();
        assert_eq!(settings.working_roots.len(), 2);
        assert!(!settings.client_mcp_servers);
        assert!(!settings.network_paths);
    }
}
