// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Terminal contracts used by AI6's supervisor and AgentHandlers, as Guatiao kinds.
//!
//! A [`Terminal`] provider starts a program on a terminal and hands back a
//! [`TerminalSession`] the caller owns. A provider is a guatiao library in any
//! language; the supervisor finds it on a search path and drives it through
//! these tables. Slot order is the ABI: append, never reorder.

pub use guatiao;

use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, ToValue, Value};
use std::sync::Arc;

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
#[guatiao::kind(name = "ai6_terminal")]
pub trait Terminal: Send + Sync {
    /// Starts `spec` (a [`Spec`] as a value) and hands over its session.
    fn spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError>;
}

/// The object handle AgentHandlers use to ask AI6's selected terminal provider to spawn.
#[guatiao::kind(object, name = "ai6_agent_terminal")]
pub trait AgentTerminal: Send + Sync {
    /// Starts a terminal process through AI6's selected Terminal provider.
    fn spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError>;
}

/// Wraps an AI6-discovered terminal provider in the object handle passed to AgentHandlers.
pub struct TerminalBridge {
    provider: Box<dyn Terminal>,
    watchdog: Option<crate::directorates::medical::watchdog::Watchdog>,
    #[cfg(feature = "publisher")]
    telemetry: Option<std::sync::Arc<crate::station::publisher::SessionTelemetry>>,
}

#[cfg(feature = "publisher")]
pub(crate) trait TerminalInputTarget: Send + Sync {
    fn write(&self, data: &[u8]) -> Result<(), &'static str>;
    fn resize(&self, rows: i64, cols: i64) -> Result<(), &'static str>;
}

impl TerminalBridge {
    /// Creates the ABI-safe terminal handle given to an AgentHandler.
    pub fn agent_terminal(provider: Box<dyn Terminal>) -> Object<dyn AgentTerminal> {
        Self {
            provider,
            watchdog: None,
            #[cfg(feature = "publisher")]
            telemetry: None,
        }
        .into_object()
    }

    #[cfg_attr(
        not(feature = "daemon"),
        allow(dead_code, reason = "used only by the daemon")
    )]
    pub(crate) fn agent_terminal_with_watchdog(
        provider: Box<dyn Terminal>,
        watchdog: crate::directorates::medical::watchdog::Watchdog,
    ) -> Object<dyn AgentTerminal> {
        Self {
            provider,
            watchdog: Some(watchdog),
            #[cfg(feature = "publisher")]
            telemetry: None,
        }
        .into_object()
    }

    #[cfg(feature = "publisher")]
    pub(crate) fn agent_terminal_with_watchdog_and_telemetry(
        provider: Box<dyn Terminal>,
        telemetry: std::sync::Arc<crate::station::publisher::SessionTelemetry>,
        watchdog: crate::directorates::medical::watchdog::Watchdog,
    ) -> Object<dyn AgentTerminal> {
        Self {
            provider,
            watchdog: Some(watchdog),
            telemetry: Some(telemetry),
        }
        .into_object()
    }
}

impl AgentTerminal for TerminalBridge {
    fn spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
        let session = Arc::new(self.provider.spawn(spec)?);
        #[cfg(feature = "publisher")]
        if let Some(telemetry) = &self.telemetry {
            telemetry.attach_terminal_target(Arc::new(TerminalInputBridge {
                session: session.clone(),
            }));
            return Ok(TerminalSessionBridge {
                session,
                watchdog: self.watchdog.clone(),
                telemetry: Some(telemetry.clone()),
            }
            .into_object());
        }
        if let Some(watchdog) = &self.watchdog {
            return Ok(TerminalSessionBridge {
                session,
                watchdog: Some(watchdog.clone()),
                #[cfg(feature = "publisher")]
                telemetry: None,
            }
            .into_object());
        }
        Ok(Arc::try_unwrap(session).unwrap_or_else(|_| unreachable!("no shared terminal handle")))
    }
}

#[cfg(feature = "publisher")]
struct TerminalInputBridge {
    session: Arc<Object<dyn TerminalSession>>,
}

#[cfg(feature = "publisher")]
impl TerminalInputTarget for TerminalInputBridge {
    fn write(&self, data: &[u8]) -> Result<(), &'static str> {
        self.session
            .write(data)
            .map_err(|_| "terminal input failed")
    }

    fn resize(&self, rows: i64, cols: i64) -> Result<(), &'static str> {
        self.session
            .resize(rows, cols)
            .map_err(|_| "terminal resize failed")
    }
}

struct TerminalSessionBridge {
    session: Arc<Object<dyn TerminalSession>>,
    watchdog: Option<crate::directorates::medical::watchdog::Watchdog>,
    #[cfg(feature = "publisher")]
    telemetry: Option<std::sync::Arc<crate::station::publisher::SessionTelemetry>>,
}

impl TerminalSession for TerminalSessionBridge {
    fn pid(&self) -> i64 {
        self.session.pid()
    }

    fn read(&self, dst: &mut [u8], timeout_ms: i64) -> Result<i64, ProviderError> {
        let read = self.session.read(dst, timeout_ms)?;
        let Some(output) = usize::try_from(read)
            .ok()
            .filter(|&length| length > 0 && length <= dst.len())
            .map(|length| &dst[..length])
        else {
            return Ok(read);
        };
        if let Some(watchdog) = &self.watchdog {
            watchdog.record_terminal_output(output);
        }
        #[cfg(feature = "publisher")]
        if let Some(telemetry) = &self.telemetry {
            telemetry.publish_terminal(output);
        }
        Ok(read)
    }

    fn write(&self, data: &[u8]) -> Result<(), ProviderError> {
        self.session.write(data)
    }

    fn resize(&self, rows: i64, cols: i64) -> Result<(), ProviderError> {
        self.session.resize(rows, cols)
    }

    fn kill(&self) -> Result<(), ProviderError> {
        self.session.kill()
    }

    fn exit_status(&self, timeout_ms: i64) -> Result<Value, ProviderError> {
        self.session.exit_status(timeout_ms)
    }
}

/// One running program on one terminal. Every slot takes `&self`, so a caller may
/// write, resize or kill from one thread while another blocks in `read`.
#[guatiao::kind(object, name = "ai6_terminal_session")]
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

    // A changed required signature affects every built terminal provider.
    #[test]
    fn floor_hashes_are_pinned() {
        assert_eq!(TerminalVtable::FLOOR_HASH, 3285297011);
        assert_eq!(TerminalSessionVtable::FLOOR_HASH, 3148381037);
    }

    struct MockTerminal;

    impl Terminal for MockTerminal {
        fn spawn(&self, _spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
            Ok(MockTerminalSession.into_object())
        }
    }

    struct MockTerminalSession;

    impl TerminalSession for MockTerminalSession {
        fn pid(&self) -> i64 {
            17
        }

        fn read(&self, _dst: &mut [u8], _timeout_ms: i64) -> Result<i64, ProviderError> {
            Ok(0)
        }

        fn write(&self, _data: &[u8]) -> Result<(), ProviderError> {
            Ok(())
        }

        fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
            Ok(())
        }

        fn kill(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn exit_status(&self, _timeout_ms: i64) -> Result<Value, ProviderError> {
            ExitInfo::default().to_value().map_err(|error| {
                ProviderError::new(guatiao::Status::GUATIAO_ERR_INTERNAL, &error.to_string())
            })
        }
    }

    #[test]
    fn agent_terminal_bridge_owns_the_provider_and_returns_a_terminal_session() {
        let terminal = TerminalBridge::agent_terminal(Box::new(MockTerminal));
        let spec = Spec {
            argv: vec!["mock".to_owned()],
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        }
        .to_value()
        .unwrap();
        assert_eq!(terminal.spawn(&spec).unwrap().pid(), 17);
    }

    #[test]
    fn terminal_bridge_feeds_raw_output_to_the_shared_watchdog() {
        let watchdog = crate::directorates::medical::watchdog::Watchdog::with_limits(
            std::time::Duration::from_secs(1),
            std::time::Duration::ZERO,
            std::time::Duration::from_secs(1),
            1,
        );
        let terminal = TerminalBridge::agent_terminal_with_watchdog(
            Box::new(OutputTerminal),
            watchdog.clone(),
        );
        let spec = Spec {
            argv: vec!["mock".to_owned()],
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        }
        .to_value()
        .unwrap();
        let session = terminal.spawn(&spec).unwrap();
        let mut output = [0; 32];

        assert_eq!(session.read(&mut output, 100).unwrap(), 12);
        assert!(watchdog.is_looping());
    }

    struct OutputTerminal;

    impl Terminal for OutputTerminal {
        fn spawn(&self, _spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
            Ok(OutputTerminalSession.into_object())
        }
    }

    struct OutputTerminalSession;

    impl TerminalSession for OutputTerminalSession {
        fn pid(&self) -> i64 {
            18
        }

        fn read(&self, dst: &mut [u8], _timeout_ms: i64) -> Result<i64, ProviderError> {
            let output = b"retry\nretry\n";
            dst[..output.len()].copy_from_slice(output);
            Ok(output.len() as i64)
        }

        fn write(&self, _data: &[u8]) -> Result<(), ProviderError> {
            Ok(())
        }

        fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
            Ok(())
        }

        fn kill(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn exit_status(&self, _timeout_ms: i64) -> Result<Value, ProviderError> {
            ExitInfo::default().to_value().map_err(|error| {
                ProviderError::new(guatiao::Status::GUATIAO_ERR_INTERNAL, &error.to_string())
            })
        }
    }
}
