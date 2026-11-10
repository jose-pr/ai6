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

mod checks;
mod host;
mod transport;

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

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

    /// Runs every check against `harness`.
    ///
    /// # Errors
    ///
    /// Every rule it broke.
    pub fn run(&self, harness: &dyn AcpHarness) -> Result<(), Failures> {
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
