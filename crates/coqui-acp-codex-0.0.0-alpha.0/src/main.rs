//! `coqui-acp-codex`: Codex as an Agent Client Protocol agent on standard input and output.

#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use coqui_acp_codex::harness::{Settings, serve};
use coqui_acp_codex::{CodexAgent, CodexLaunch};

/// Serves Codex as an Agent Client Protocol agent on standard input and output, in ACP version 1 or 2, whichever the
/// client asks for. Each session runs `codex app-server` with one thread; approvals are asked through ACP.
/// Diagnostics go to standard error.
#[derive(Debug, Parser)]
#[command(
    name = "coqui-acp-codex",
    version,
    after_help = "Codex reads its own variables (CODEX_HOME, OPENAI_API_KEY \
    and others) from the environment it is given."
)]
struct Cli {
    /// The Codex program.
    #[arg(long, env = "COQUI_ACP_CODEX_EXECUTABLE", default_value = "codex")]
    executable: OsString,
    /// The model every session starts with, until the client chooses another.
    #[arg(long)]
    model: Option<String>,
    /// The reasoning effort every session starts with, such as low or high.
    #[arg(long)]
    effort: Option<String>,
    /// The approval policy: untrusted, on-failure, on-request or never.
    #[arg(long)]
    approval_policy: Option<String>,
    /// Offer the Bypass mode: never ask, and no sandbox.
    #[arg(long)]
    allow_bypass: bool,
    /// The sandbox: read-only, workspace-write or danger-full-access.
    #[arg(long)]
    sandbox: Option<String>,
    /// The highest ACP version answered.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=2))]
    max_acp_version: u8,
    /// Sessions at once.
    #[arg(long, default_value_t = 8)]
    max_sessions: usize,
    /// A directory every session's `cwd` and more directories must lie in, repeated for more; none takes any.
    #[arg(long = "working-root", value_name = "DIRECTORY")]
    working_roots: Vec<PathBuf>,
    /// Refuse a session that a client gives MCP servers.
    #[arg(long)]
    no_client_mcp_servers: bool,
    /// Take a network path (`\\\\host\\share`) as a session's `cwd` or more directories.
    #[arg(long)]
    allow_network_paths: bool,
}

fn settings_of(cli: &Cli) -> Settings {
    let mut settings = Settings::new();
    settings.max_version = cli.max_acp_version;
    settings.max_sessions = cli.max_sessions;
    settings.working_roots.clone_from(&cli.working_roots);
    settings.client_mcp_servers = !cli.no_client_mcp_servers;
    settings.network_paths = cli.allow_network_paths;
    settings.on_notice = Some(Arc::new(|text: &str| eprintln!("coqui-acp-codex: {text}")));
    settings
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let settings = settings_of(&cli);
    let mut defaults = CodexLaunch::new(".");
    defaults.executable = cli.executable;
    defaults.model = cli.model;
    defaults.effort = cli.effort;
    defaults.approval_policy = cli.approval_policy;
    defaults.sandbox = cli.sandbox;
    defaults.allow_bypass = cli.allow_bypass;
    let agent = CodexAgent::new().defaults(defaults);
    match serve(
        Arc::new(agent),
        settings,
        std::io::stdin(),
        std::io::stdout(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("coqui-acp-codex: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, settings_of};

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn what_a_host_narrows_on_the_command_line_reaches_the_settings() {
        let settings = settings_of(&Cli::parse_from([
            "coqui-acp-codex",
            "--working-root",
            "/w",
            "--working-root",
            "/x",
            "--no-client-mcp-servers",
        ]));
        assert_eq!(settings.working_roots.len(), 2);
        assert!(!settings.client_mcp_servers);
        assert!(!settings.network_paths);
    }
}
