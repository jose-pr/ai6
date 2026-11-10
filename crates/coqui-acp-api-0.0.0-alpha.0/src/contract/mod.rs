//! The suite every `acp_harness` provider is held to, from the host's side.
//!
//! [`Contract::run`] drives a provider through the kind and answers every rule it broke. The
//! provider is configured to drive the **contract agent**, which a real harness provides with a
//! stand-in for its program. It answers `initialize`, `session/new` and, when its `describe` says
//! it takes them, `session/load` and `session/resume`, and answers a `session/prompt` by its text:
//!
//! | prompt | the agent |
//! | --- | --- |
//! | `close` | answers as any text, then ends the connection itself, `closed(GUATIAO_OK, "")` |
//! | `wait` | ends the turn only on `session/cancel`, with `stopReason` `cancelled` |
//! | `ask` | asks `session/request_permission` with options `allow` (`allow_once`) and `reject` (`reject_once`); on `allow` sends `echo: allowed`, on a cancelled outcome ends the turn `cancelled`, and on anything else (`reject`, an error, an outcome it does not know) sends `echo: refused` |
//! | `read PATH` | with the client's `fs.readTextFile`, asks `fs/read_text_file` for PATH and sends `echo: CONTENT`, or `echo: refused` on an error; without it, sends `echo: no fs` and asks nothing |
//! | any other text | sends `echo: TEXT` |
//!
//! each as one `agent_message_chunk`, then `stopReason` `end_turn`. It holds the requirements
//! [`Contract::holds`] names and refuses every other, and keeps every session inside the working
//! roots.
//!
//! Its sessions are kept: it imports a history at the host's UUID, an empty one seeding a fresh
//! session, exports what its sessions said in the standard format (a header line with `version`
//! and a `digest`, then the turns) in pages of whole lines, and `session/load` replays a session's
//! updates. A provider that cannot import
//! is run with [`Contract::imports`] off and must refuse with a reason.
//!
//! The `acp-harness-check` program runs the same suite on a library file or a source file, for a
//! provider written in any language Guatiao loads.

mod accounts;
mod checks;
mod host;
mod json;
mod programs;
mod spawner;
mod transport;

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

pub use programs::HOME_VARIABLE;
pub use spawner::spawner_files;

use crate::{AcpHarness, NO_COMMANDS, READ_ONLY, Requirement};

/// What a provider is held to, and how it is driven. From [`Contract::new`].
#[derive(Debug, Clone)]
#[must_use]
pub struct Contract {
    name: String,
    version: u16,
    holds: Vec<Requirement>,
    message_limit: Option<usize>,
    imports: bool,
    logins: bool,
    account: Option<String>,
    program_agent: bool,
}

/// One rule a provider broke.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Failure {
    /// The check, by name.
    pub check: &'static str,
    /// What happened instead.
    pub message: String,
}

/// Every rule a provider broke, from [`Contract::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failures {
    /// The provider, as the contract named it.
    pub provider: String,
    /// The rules, in the order they were checked.
    pub failures: Vec<Failure>,
}

impl fmt::Display for Failures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} broke {} rule(s) of the contract",
            self.provider,
            self.failures.len()
        )?;
        for failure in &self.failures {
            write!(f, "\n  {}: {}", failure.check, failure.message)?;
        }
        Ok(())
    }
}

impl core::error::Error for Failures {}

impl Contract {
    /// The contract for a provider called `name` in the failures, spoken in ACP version 1, holding
    /// [`READ_ONLY`] and [`NO_COMMANDS`].
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: 1,
            holds: alloc::vec![Requirement::new(READ_ONLY), Requirement::new(NO_COMMANDS)],
            message_limit: None,
            imports: true,
            logins: true,
            account: None,
            program_agent: false,
        }
    }

    /// The ACP version `initialize` asks for; the provider's `describe` must list it.
    pub const fn version(mut self, version: u16) -> Self {
        self.version = version;
        self
    }

    /// The requirements the provider holds, in place of the default pair. Each must be one it
    /// accepts alone; every other is expected to be refused.
    pub fn holds(mut self, holds: Vec<Requirement>) -> Self {
        self.holds = holds;
        self
    }

    /// The largest message the provider takes, when it has a limit: one byte more must end the
    /// connection with `GUATIAO_ERR_BAD_VALUE`.
    pub const fn message_limit(mut self, bytes: usize) -> Self {
        self.message_limit = Some(bytes);
        self
    }

    /// Whether the provider imports sessions (the default). One that does not must refuse
    /// `import_session` with `GUATIAO_ERR_UNSUPPORTED` and a reason.
    pub const fn imports(mut self, imports: bool) -> Self {
        self.imports = imports;
        self
    }

    /// Whether the provider offers login methods (the default). One that does not must list none
    /// in its catalog and refuse `login` with `GUATIAO_ERR_UNSUPPORTED` and a reason.
    pub const fn logins(mut self, logins: bool) -> Self {
        self.logins = logins;
        self
    }

    /// Checks the account checks against this kind alone, in place of every kind the catalog
    /// lists. The catalog checks still read all of them.
    pub fn account(mut self, kind: impl Into<String>) -> Self {
        self.account = Some(kind.into());
        self
    }

    /// Whether the agent is the harness's own program, which the provider starts through the spawner and the suite's
    /// spawner then starts for real (the default is the in-process contract agent, and a spawner that starts nothing).
    /// The provider's settings name a stand-in for the program that behaves as the module's documentation says for
    /// `wait`, `ask` and a plain prompt (`echo: TEXT`) and, when `describe` lists usage sources, `quota P` and `limit`.
    ///
    /// What a program agent cannot do is not asked of it: asking the client for a file (`read PATH`), ending the
    /// connection on its own (`close`), and the exact figures `quota` and `limit` script (their ids and token bucket),
    /// of which the checks keep the shape: a report that changes, tells a client that asked, agrees with the kind's
    /// answer, and a limit that refuses the turn and is kept.
    pub const fn program_agent(mut self, program_agent: bool) -> Self {
        self.program_agent = program_agent;
        self
    }

    /// Runs every check against `harness`.
    ///
    /// # Errors
    ///
    /// Every rule it broke.
    pub fn run(&self, harness: &dyn AcpHarness) -> Result<(), Failures> {
        let _running = host::Running::on(self.program_agent);
        let failures: Vec<Failure> = checks::all(self, harness)
            .into_iter()
            .filter_map(|(check, outcome)| outcome.err().map(|message| Failure { check, message }))
            .collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Failures {
                provider: self.name.to_owned(),
                failures,
            })
        }
    }
}
