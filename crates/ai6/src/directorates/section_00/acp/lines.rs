//! Newline-delimited transports over blocking input and output.

use agent_client_protocol::Lines;
use futures::SinkExt;
use futures::channel::mpsc;
use guatiao::library::Object;
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::directorates::q::terminal::{READ_TIMED_OUT, TerminalSession};

/// The transport type [`lines_over_io`] returns.
pub type IoLines = Lines<
    futures::sink::SinkMapErr<mpsc::UnboundedSender<String>, fn(mpsc::SendError) -> io::Error>,
    mpsc::UnboundedReceiver<io::Result<String>>,
>;

/// The last lines a peer wrote that were not protocol.
pub type Diagnostics = Arc<Mutex<VecDeque<String>>>;

/// An ACP transport over a blocking reader and writer, such as a child process's output and
/// input: one JSON-RPC message per line. Each side runs on a thread of its own, and both end
/// when the peer closes or the connection is dropped.
///
/// Lines that do not start a JSON value are not protocol: the last `diagnostic_lines` of them
/// are kept in the returned buffer, for the message a caller shows when the peer dies.
pub fn lines_over_io(
    reader: impl Read + Send + 'static,
    mut writer: impl Write + Send + 'static,
    diagnostic_lines: usize,
) -> (IoLines, Diagnostics) {
    let (incoming_tx, incoming_rx) = mpsc::unbounded::<io::Result<String>>();
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded::<String>();
    let diagnostics = Arc::new(Mutex::new(VecDeque::new()));
    let kept = diagnostics.clone();
    std::thread::Builder::new()
        .name("ai6-acp-read".to_owned())
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&line);
                        let text = text.trim();
                        if text.starts_with('{') || text.starts_with('[') {
                            if incoming_tx.unbounded_send(Ok(text.to_owned())).is_err() {
                                break;
                            }
                        } else if !text.is_empty()
                            && let Ok(mut kept) = kept.lock()
                        {
                            kept.push_back(text.to_owned());
                            while kept.len() > diagnostic_lines {
                                kept.pop_front();
                            }
                        }
                    }
                    Err(error) => {
                        let _ = incoming_tx.unbounded_send(Err(error));
                        break;
                    }
                }
            }
        })
        .ok();
    std::thread::Builder::new()
        .name("ai6-acp-write".to_owned())
        .spawn(move || {
            while let Some(line) =
                futures::executor::block_on(futures::StreamExt::next(&mut outgoing_rx))
            {
                if writer
                    .write_all(line.as_bytes())
                    .and_then(|()| writer.write_all(b"\n"))
                    .and_then(|()| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        })
        .ok();
    let outgoing = outgoing_tx.sink_map_err((|error| io::Error::other(error)) as fn(_) -> _);
    (Lines::new(outgoing, incoming_rx), diagnostics)
}

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
