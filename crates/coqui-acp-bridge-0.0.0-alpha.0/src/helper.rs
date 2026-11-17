//! The program that runs this library's helper subcommands: the one an agent's guard waits in, and the one `mcp-offer/1` and
//! `mcp-config` hand to an agent as an MCP server.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

use std::path::PathBuf;

/// A program that runs `agent-guard PID`, `mcp-shim` and `mcp-shim --peer PEER` as its subcommands, after `args`.
///
/// A host that embeds this library names itself: the library starts no program of its own, since a library cannot know what
/// its host is called, or what comes before a subcommand on its command line.
#[derive(Clone, Debug)]
pub struct Helper {
    /// The executable.
    pub program: PathBuf,
    /// What goes before the subcommand on its command line.
    pub args: Vec<String>,
}

impl Helper {
    /// `program`, with `args` before each subcommand.
    #[must_use]
    pub const fn new(program: PathBuf, args: Vec<String>) -> Self {
        Self { program, args }
    }

    /// This program's own executable, with nothing before the subcommand.
    ///
    /// # Errors
    ///
    /// The operating system cannot say where this program is.
    pub fn current() -> Result<Self, String> {
        std::env::current_exe()
            .map(|program| Self::new(program, Vec::new()))
            .map_err(|error| format!("this program cannot find itself: {error}"))
    }

    /// The arguments of one helper subcommand: `args`, then `rest`.
    #[must_use]
    pub fn command(&self, rest: &[&str]) -> Vec<String> {
        self.args
            .iter()
            .cloned()
            .chain(rest.iter().map(|part| (*part).to_owned()))
            .collect()
    }
}
