// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The terminal contract of coagent's supervisor, as guatiao kinds.
//!
//! A [`Terminal`] provider starts a program on a terminal and hands back a
//! [`TerminalSession`] the caller owns. A provider is a guatiao library in any
//! language; the supervisor finds it on a search path and drives it through
//! these tables. Slot order is the ABI: append, never reorder.

pub use guatiao;

use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, ToValue, Value};

/// What to start. Crosses as a value: `{"argv": [..], "cwd": .., "env": {..},
/// "rows": .., "cols": ..}`.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao)]
pub struct Spec {
    /// The program and its arguments, one element each; never re-split by a shell.
    pub argv: Vec<String>,
    /// Working directory; absent inherits the supervisor's.
    pub cwd: Option<String>,
    /// The whole environment; absent inherits the supervisor's.
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[map(default = 24)]
    pub rows: u16,
    #[map(default = 80)]
    pub cols: u16,
}

/// How a program ended, or that it has not. At most one of `code` and `signal` is set,
/// and neither while `running`.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[map(crate = ::guatiao)]
pub struct ExitInfo {
    pub running: bool,
    pub code: Option<i64>,
    pub signal: Option<i64>,
}

/// `read` answers this when nothing arrived within the timeout.
pub const READ_TIMED_OUT: i64 = -1;

/// Starts programs on terminals.
#[guatiao::kind(name = "coagent_terminal")]
pub trait Terminal: Send + Sync {
    /// Starts `spec` (a [`Spec`] as a value) and hands over its session.
    fn spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError>;
}

/// One running program on one terminal. Every slot takes `&self`, so a caller may
/// write, resize or kill from one thread while another blocks in `read`.
#[guatiao::kind(object, name = "coagent_terminal_session")]
pub trait TerminalSession: Send + Sync {
    /// The program's process id.
    fn pid(&self) -> i64;
    /// Copies output into `dst`: the byte count, `0` once no more can ever arrive, or
    /// [`READ_TIMED_OUT`] after `timeout_ms` milliseconds with nothing.
    fn read(&self, dst: &mut [u8], timeout_ms: i64) -> Result<i64, ProviderError>;
    /// Input, as typed at the terminal; all of it, or an error.
    fn write(&self, data: &[u8]) -> Result<(), ProviderError>;
    fn resize(&self, rows: i64, cols: i64) -> Result<(), ProviderError>;
    /// Ends the program and every process it started. Idempotent.
    fn kill(&self) -> Result<(), ProviderError>;
    /// Waits up to `timeout_ms` (0 polls) and answers an [`ExitInfo`] as a value.
    fn exit_status(&self, timeout_ms: i64) -> Result<Value, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // A changed required signature changes these; every built provider and the Python
    // mirror in coagent_m.terminal must change with them.
    #[test]
    fn floor_hashes_are_pinned() {
        assert_eq!(TerminalVtable::FLOOR_HASH, 3285297011);
        assert_eq!(TerminalSessionVtable::FLOOR_HASH, 3148381037);
    }
}
