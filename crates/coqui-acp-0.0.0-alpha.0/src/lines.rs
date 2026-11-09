//! Newline-delimited transports over blocking input and output, such as a child process's pipes.

use alloc::borrow::ToOwned;
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::Mutex;

use agent_client_protocol::Lines;
use futures::SinkExt;
use futures::channel::mpsc;

/// The largest line accepted unless the caller says otherwise: 16 MiB, since a whole session's
/// history can arrive as one message.
pub const DEFAULT_MAX_LINE: u64 = 16 * 1024 * 1024;

/// The transport [`over_io`] returns.
pub type IoLines = Lines<
    futures::sink::SinkMapErr<mpsc::UnboundedSender<String>, fn(mpsc::SendError) -> io::Error>,
    mpsc::UnboundedReceiver<io::Result<String>>,
>;

/// The last lines a peer wrote that were not protocol.
pub type Diagnostics = Arc<Mutex<VecDeque<String>>>;

/// A transport over a blocking reader and writer, one JSON-RPC message per line. Each side runs on a
/// thread of its own, and both end when the peer closes or the connection is dropped.
///
/// A line that does not start a JSON value is not protocol: the last `diagnostic_lines` of them are kept
/// in the returned buffer, for the message a caller shows when the peer dies. A line over `max_line`
/// bytes ends the transport with an error.
///
/// # Errors
///
/// [`std::io::Error`] when a thread could not start.
pub fn over_io(
    reader: impl Read + Send + 'static,
    mut writer: impl Write + Send + 'static,
    diagnostic_lines: usize,
    max_line: u64,
) -> io::Result<(IoLines, Diagnostics)> {
    let (incoming_tx, incoming_rx) = mpsc::unbounded::<io::Result<String>>();
    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded::<String>();
    let diagnostics: Diagnostics = Arc::new(Mutex::new(VecDeque::new()));
    let kept = diagnostics.clone();
    std::thread::Builder::new()
        .name("coqui-acp-read".to_owned())
        .spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut line = alloc::vec::Vec::new();
            loop {
                line.clear();
                // One more than the limit, so an overlong line is seen rather than split.
                match (&mut reader)
                    .take(max_line.saturating_add(1))
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) => break,
                    Ok(count) if u64::try_from(count).unwrap_or(u64::MAX) > max_line => {
                        let _ = incoming_tx
                            .unbounded_send(Err(io::Error::other("a line is over the size limit")));
                        break;
                    }
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&line);
                        let text = text.trim();
                        if text.starts_with('{') || text.starts_with('[') {
                            if incoming_tx.unbounded_send(Ok(text.to_string())).is_err() {
                                break;
                            }
                        } else if !text.is_empty()
                            && let Ok(mut kept) = kept.lock()
                        {
                            kept.push_back(text.to_string());
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
        })?;
    std::thread::Builder::new()
        .name("coqui-acp-write".to_owned())
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
        })?;
    let outgoing = outgoing_tx.sink_map_err((|error| io::Error::other(error)) as fn(_) -> _);
    Ok((Lines::new(outgoing, incoming_rx), diagnostics))
}
