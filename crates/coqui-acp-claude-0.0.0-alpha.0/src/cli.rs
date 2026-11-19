//! Claude Code's agent as library code: its arguments as a `clap::Args` type and a [`run`] that serves ACP on standard
//! input and output. A program that people run is the host's; `examples/claude.rs` is one for this crate's tests.

#![allow(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use crate::harness::{Settings, serve};
use crate::{ClaudeAgent, ClaudeLaunch};

/// Serves Claude Code as an Agent Client Protocol agent on standard input and output, in ACP version 1 or 2,
/// whichever the client asks for. Each session runs `claude` in stream-json mode; permission prompts are asked
/// through ACP. Diagnostics go to standard error.
///
/// Claude Code reads its own variables (`CLAUDE_CONFIG_DIR`, `ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN` and others)
/// from the environment it is given.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The Claude Code program.
    #[arg(long, env = "COQUI_ACP_CLAUDE_EXECUTABLE", default_value = "claude")]
    pub executable: OsString,
    /// The model every session starts with, until the client chooses another.
    #[arg(long)]
    pub model: Option<String>,
    /// The effort every session starts with: low, medium, high, xhigh or max.
    #[arg(long)]
    pub effort: Option<String>,
    /// The permission mode: default, acceptEdits, plan, dontAsk, bypassPermissions, auto or manual.
    #[arg(long)]
    pub permission_mode: Option<String>,
    /// Offer the Bypass mode: Claude Code is started allowing a session to switch to `bypassPermissions`.
    #[arg(long)]
    pub allow_bypass: bool,
    /// The setting sources Claude Code loads, comma-separated, of user, project and local; empty loads none.
    #[arg(long)]
    pub setting_sources: Option<String>,
    /// Only the MCP servers a session is given, none from the person's or the project's configuration.
    #[arg(long)]
    pub strict_mcp_config: bool,
    /// The highest ACP version answered.
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=2))]
    pub max_acp_version: u8,
    /// Sessions at once.
    #[arg(long, default_value_t = 8)]
    pub max_sessions: usize,
    /// Do not ask for permission on standard output: what nothing has granted is refused where it is made.
    #[arg(long)]
    pub no_permission_prompts: bool,
    /// Let a client load plugins through `_meta.claudeCode.options.plugins`; their hooks and servers run unasked.
    #[arg(long)]
    pub allow_client_plugins: bool,
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
            eprintln!("coqui-acp-claude: {text}");
        }));
        settings
    }

    /// The agent, with these arguments as what each session starts with.
    #[must_use]
    pub fn agent(&self) -> ClaudeAgent {
        let mut defaults = ClaudeLaunch::new(".");
        defaults.executable.clone_from(&self.executable);
        defaults.model.clone_from(&self.model);
        defaults.effort.clone_from(&self.effort);
        defaults.permission_mode.clone_from(&self.permission_mode);
        defaults.allow_bypass = self.allow_bypass;
        defaults.setting_sources = self.setting_sources.as_ref().map(|sources| {
            sources
                .split(',')
                .filter(|source| !source.is_empty())
                .map(str::to_string)
                .collect()
        });
        defaults.strict_mcp_config = self.strict_mcp_config;
        defaults.permission_prompts = !self.no_permission_prompts;
        ClaudeAgent::new()
            .defaults(defaults)
            .client_plugins(self.allow_client_plugins)
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
        /// Claude Code's configuration directory; by default `CLAUDE_CONFIG_DIR`, else `.claude` in the home directory.
        #[arg(long)]
        config: Option<PathBuf>,
        /// The directory the session worked in.
        cwd: PathBuf,
        /// The session's UUID.
        id: String,
    },
    /// Reads a history from standard input and writes it as a session Claude Code resumes. Prints what could not
    /// be carried over on standard error; never replaces a session.
    Import {
        /// Claude Code's configuration directory; by default `CLAUDE_CONFIG_DIR`, else `.claude` in the home directory.
        #[arg(long)]
        config: Option<PathBuf>,
        /// The directory the session worked in.
        cwd: PathBuf,
        /// The session's UUID.
        id: String,
        /// Seed from the first line of the history, not from its latest compaction summary.
        #[arg(long)]
        no_summary: bool,
    },
}

/// Runs one history command on standard input and output.
///
/// # Errors
///
/// The configuration directory is unknown, or the session or the history could not be read or written.
#[cfg(feature = "history")]
pub fn run_history(action: &History) -> Result<u8, String> {
    use std::io::{Read, Write};

    use crate::history::{DEFAULT_MAX_BYTES, ImportOptions, export_file, import_file};

    let place = |given: &Option<PathBuf>| {
        given
            .clone()
            .or_else(|| crate::config_dir(&std::collections::BTreeMap::new(), false))
            .ok_or_else(|| "the configuration directory is unknown: pass --config".to_owned())
    };
    match action {
        History::Export { config, cwd, id } => {
            let config = place(config)?;
            let doc = export_file(&config, cwd, id, DEFAULT_MAX_BYTES)
                .map_err(|error| error.to_string())?;
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
        } => {
            let config = place(config)?;
            let mut text = Vec::new();
            std::io::stdin()
                .read_to_end(&mut text)
                .map_err(|error| error.to_string())?;
            let doc = coqui_acp_history::Document::from_slice(&text)
                .map_err(|error| error.to_string())?;
            let options = ImportOptions::default().use_summary(!no_summary);
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
            "claude",
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

    #[test]
    fn plugins_from_a_client_and_the_prompt_flags_are_a_hosts_to_set() {
        let args = Cli::parse_from(["claude"]).args;
        assert!(!args.allow_client_plugins);
        assert!(!args.no_permission_prompts);
        let args = Cli::parse_from([
            "claude",
            "--allow-client-plugins",
            "--no-permission-prompts",
        ])
        .args;
        assert!(args.allow_client_plugins);
        assert!(args.no_permission_prompts);
    }
}
