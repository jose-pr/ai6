//! A terminal provider's session as the blocking input and output of an ACP agent.

use guatiao::library::Object;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::directorates::q::terminal::{READ_TIMED_OUT, TerminalSession};

/// A terminal provider's session as blocking [`Read`] and [`Write`]: the standard output and
/// input of an agent the `pipe` terminal started. Reading returns end of file once the
/// program's output can never arrive or [`TerminalIo::stop`] was called.
#[derive(Clone)]
pub struct TerminalIo {
    session: Arc<Object<dyn TerminalSession>>,
    stopped: Arc<AtomicBool>,
    poll_ms: i64,
}

impl TerminalIo {
    /// Reads and writes `session`, checking for a stop request every `poll_ms` milliseconds
    /// of silence.
    pub fn new(session: Arc<Object<dyn TerminalSession>>, poll_ms: i64) -> Self {
        Self {
            session,
            stopped: Arc::new(AtomicBool::new(false)),
            poll_ms,
        }
    }

    /// Makes the reader report end of file at its next poll.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}

impl Read for TerminalIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.stopped.load(Ordering::SeqCst) {
                return Ok(0);
            }
            match self.session.read(buf, self.poll_ms) {
                Ok(READ_TIMED_OUT) => continue,
                Ok(count) => return Ok(usize::try_from(count).unwrap_or(0)),
                Err(error) => return Err(io::Error::other(error.to_string())),
            }
        }
    }
}

impl Write for TerminalIo {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.session
            .write(data)
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
