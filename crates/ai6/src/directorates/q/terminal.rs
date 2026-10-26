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

/// The widest terminal every provider accepts: ConPTY takes its size as signed 16-bit
/// numbers. A handler whose harness writes one JSON value per line asks for it, so that few
/// lines wrap; [`JsonLines`] rejoins the ones that still do.
pub const STREAM_COLS: u16 = 32767;

/// The longest JSON line [`JsonLines`] holds while it waits for the rest; past it, what was
/// held is handed over as text.
pub const MAX_JSON_LINE: usize = 64 * 1024 * 1024;

/// `bytes` without terminal control sequences: CSI (`ESC [ ... final`), OSC (`ESC ] ... BEL`
/// or `ESC ] ... ESC \`) and two-byte escapes. Other bytes, line breaks included, are kept.
pub fn strip_controls(bytes: &[u8]) -> Vec<u8> {
    let mut visible = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match control_len(&bytes[index..]) {
            Some((len, _)) => index += len,
            None => {
                visible.push(bytes[index]);
                index += 1;
            }
        }
    }
    visible
}

/// The length of the control sequence `bytes` starts with, and whether it moves the cursor to
/// another row (CUP, HVP, CNL, CPL, VPA, IND, NEL, RI); `None` when it starts with none. An
/// unfinished sequence runs to the end.
fn control_len(bytes: &[u8]) -> Option<(usize, bool)> {
    if bytes.first() != Some(&0x1b) {
        return None;
    }
    match bytes.get(1).copied() {
        None => Some((1, false)),
        Some(b'[') => {
            let end = bytes[2..]
                .iter()
                .position(|byte| (0x40..=0x7e).contains(byte))
                .map_or(bytes.len(), |at| at + 3);
            let moves = matches!(bytes.get(end - 1), Some(b'H' | b'f' | b'E' | b'F' | b'd'));
            Some((end, moves))
        }
        Some(b']') => {
            let mut index = 2;
            while index < bytes.len() {
                if bytes[index] == 0x07 {
                    return Some((index + 1, false));
                }
                if bytes[index] == 0x1b && bytes.get(index + 1) == Some(&b'\\') {
                    return Some((index + 2, false));
                }
                index += 1;
            }
            Some((bytes.len(), false))
        }
        Some(byte) => Some((2, matches!(byte, b'D' | b'E' | b'M'))),
    }
}

/// One line of a harness's output, as [`JsonLines`] splits it.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamLine {
    /// A JSON object or array, with its text (terminal controls removed).
    Json {
        value: serde_json::Value,
        text: Vec<u8>,
    },
    /// Anything else, trimmed and with terminal controls removed; never empty.
    Text(String),
}

/// Splits a terminal's output into the JSON values a harness wrote one per line.
///
/// A pseudo-terminal is not a pipe: ConPTY on Windows renders the console it hosts, so its
/// output carries mode and title sequences before and between lines, may move the cursor to
/// the next row instead of writing a line break, and breaks a line longer than the terminal
/// is wide (`\r\n`, then a cursor move). Lines are therefore cut at line breaks and at cursor
/// moves to another row, stripped of controls, and a line that is the start of a JSON value
/// but not the whole of it is joined with the lines that follow until it parses, up to
/// [`MAX_JSON_LINE`] bytes. JSON text cannot hold a raw line break or `ESC`, so neither join
/// nor strip changes a value the harness wrote.
#[derive(Debug, Default)]
pub struct JsonLines {
    /// Output not yet ended by a line break.
    buffer: Vec<u8>,
    /// The start of a JSON value whose rest has not arrived.
    partial: Vec<u8>,
    ready: std::collections::VecDeque<StreamLine>,
}

impl JsonLines {
    /// An empty splitter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds output as read from the terminal.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
        let Some(end) = self.buffer.iter().rposition(|byte| *byte == b'\n') else {
            return;
        };
        let rest = self.buffer.split_off(end + 1);
        let complete = std::mem::replace(&mut self.buffer, rest);
        self.cut(&complete);
    }

    /// The next whole line, if one has arrived.
    pub fn next_line(&mut self) -> Option<StreamLine> {
        self.ready.pop_front()
    }

    /// Everything still held, once no more output can arrive: the unended last line, and a
    /// JSON value that never completed, as text.
    pub fn finish(&mut self) -> Option<StreamLine> {
        if !self.buffer.is_empty() {
            let rest = std::mem::take(&mut self.buffer);
            self.cut(&rest);
        }
        if !self.partial.is_empty() {
            let partial = std::mem::take(&mut self.partial);
            self.push_text(&partial);
        }
        self.ready.pop_front()
    }

    /// Cuts `bytes` at line breaks and row moves and takes each piece.
    fn cut(&mut self, bytes: &[u8]) {
        let mut line = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            if let Some((len, moves)) = control_len(&bytes[index..]) {
                if moves {
                    self.take(&std::mem::take(&mut line));
                }
                index += len;
                continue;
            }
            match bytes[index] {
                b'\n' => self.take(&std::mem::take(&mut line)),
                // JSON text holds no raw CR either; ConPTY ends a broken line with CRLF.
                b'\r' => {}
                byte => line.push(byte),
            }
            index += 1;
        }
        self.take(&line);
    }

    /// Takes one piece of a line. A piece that continues a held value is joined untrimmed,
    /// since a break inside a string may fall among its spaces.
    fn take(&mut self, piece: &[u8]) {
        if !self.partial.is_empty() {
            let mut joined = std::mem::take(&mut self.partial);
            joined.extend_from_slice(piece);
            match parse(&joined) {
                Parsed::Value(value) => {
                    joined.truncate(joined.trim_ascii_end().len());
                    self.ready.push_back(StreamLine::Json {
                        value,
                        text: joined,
                    });
                    return;
                }
                // A whole value of its own means the held start was never finished.
                Parsed::Incomplete
                    if joined.len() <= MAX_JSON_LINE && !is_whole_value(piece.trim_ascii()) =>
                {
                    self.partial = joined;
                    return;
                }
                _ => {
                    let held = joined.len() - piece.len();
                    self.push_text(&joined[..held]);
                }
            }
        }
        // The end stays: a value broken at the terminal's width may end in spaces of a string.
        let line = piece.trim_ascii_start();
        if line.is_empty() {
            return;
        }
        if !matches!(line[0], b'{' | b'[') {
            self.push_text(line);
            return;
        }
        match parse(line) {
            Parsed::Value(value) => self.ready.push_back(StreamLine::Json {
                value,
                text: line.trim_ascii_end().to_vec(),
            }),
            Parsed::Incomplete => self.partial = line.to_vec(),
            Parsed::Invalid => self.push_text(line),
        }
    }

    fn push_text(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes).trim().to_owned();
        if !text.is_empty() {
            self.ready.push_back(StreamLine::Text(text));
        }
    }
}

enum Parsed {
    Value(serde_json::Value),
    Incomplete,
    Invalid,
}

fn is_whole_value(line: &[u8]) -> bool {
    matches!(line.first(), Some(b'{' | b'['))
        && serde_json::from_slice::<serde::de::IgnoredAny>(line).is_ok()
}

fn parse(bytes: &[u8]) -> Parsed {
    match serde_json::from_slice(bytes) {
        Ok(value) => Parsed::Value(value),
        Err(error) if error.is_eof() => Parsed::Incomplete,
        Err(_) => Parsed::Invalid,
    }
}

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

    fn lines_of(chunks: &[&[u8]]) -> Vec<StreamLine> {
        let mut lines = JsonLines::new();
        let mut out = Vec::new();
        for chunk in chunks {
            lines.push(chunk);
            out.extend(std::iter::from_fn(|| lines.next_line()));
        }
        out.extend(std::iter::from_fn(|| lines.finish()));
        out
    }

    fn json(text: &str) -> StreamLine {
        StreamLine::Json {
            value: serde_json::from_str(text).unwrap(),
            text: text.as_bytes().to_vec(),
        }
    }

    // ConPTY's start-up sequences (Windows 11), as they precede a harness's first line.
    const CONPTY_START: &[u8] =
        b"\x1b[?9001h\x1b[?1004h\x1b[m\x1b]0;C:\\tools\\codex.exe\x07\x1b[?25h\x1b[?25l\r\n";

    #[test]
    fn json_lines_ignore_the_controls_conpty_writes_before_and_between_lines() {
        let mut stream = CONPTY_START.to_vec();
        stream.extend_from_slice(b"{\"id\":1}\r\n\x1b[?25h{\"id\":2}\x1b[m\r\n");
        assert_eq!(
            lines_of(&[&stream]),
            [json("{\"id\":1}"), json("{\"id\":2}")]
        );
    }

    #[test]
    fn json_lines_cut_at_a_cursor_move_to_another_row() {
        assert_eq!(
            lines_of(&[b"{\"a\":1}\x1b[8;1H{\"b\":2}\x1b[11;1H\r\n"]),
            [json("{\"a\":1}"), json("{\"b\":2}")]
        );
    }

    /// ConPTY breaks a line longer than the terminal is wide as CRLF, a move back to the last
    /// column of the row above and that column's character (Windows 11, 32767 columns).
    #[test]
    fn json_lines_rejoin_a_line_the_terminal_broke() {
        let text = format!("{{\"text\":\"{}  {}\"}}", "x".repeat(40), "y".repeat(40));
        let (head, tail) = text.split_at(30);
        let (last, tail) = tail.split_at(1);
        let broken = format!("{head}\r\n\x1b[23;32767H{last}{tail}\r\n");
        assert_eq!(lines_of(&[broken.as_bytes()]), [json(&text)]);

        let at_spaces = text.find("  ").unwrap() + 1;
        let (head, tail) = text.split_at(at_spaces);
        let broken = format!("{head}\r\n{tail}\r\n");
        assert_eq!(
            lines_of(&[broken.as_bytes()]),
            [json(&text)],
            "spaces at the break are kept"
        );
    }

    #[test]
    fn json_lines_arrive_whole_whatever_the_chunks() {
        let mut stream = CONPTY_START.to_vec();
        stream.extend_from_slice(b"{\"type\":\"system\",\"session_id\":\"s-1\"}\r\n");
        stream.extend_from_slice(
            "{\"type\":\"result\",\"result\":\"d\u{e9}j\u{e0}\"}\r\n".as_bytes(),
        );
        let chunks = stream.chunks(1).collect::<Vec<_>>();
        assert_eq!(
            lines_of(&chunks),
            [
                json("{\"type\":\"system\",\"session_id\":\"s-1\"}"),
                json("{\"type\":\"result\",\"result\":\"d\u{e9}j\u{e0}\"}"),
            ]
        );
    }

    #[test]
    fn json_lines_hand_over_other_output_as_text() {
        assert_eq!(
            lines_of(&[b"\x1b[31mwarning: slow\x1b[0m\r\n42\r\n{not json}\r\n{\"ok\":true}\r\n"]),
            [
                StreamLine::Text("warning: slow".to_owned()),
                StreamLine::Text("42".to_owned()),
                StreamLine::Text("{not json}".to_owned()),
                json("{\"ok\":true}"),
            ]
        );
    }

    #[test]
    fn an_unfinished_value_gives_way_to_a_whole_one_and_ends_as_text() {
        assert_eq!(
            lines_of(&[b"{\"cut\":\"off\r\n{\"next\":1}\r\n{\"tail\":"]),
            [
                StreamLine::Text("{\"cut\":\"off".to_owned()),
                json("{\"next\":1}"),
                StreamLine::Text("{\"tail\":".to_owned()),
            ]
        );
    }

    #[test]
    fn strip_controls_keeps_text_and_line_breaks() {
        assert_eq!(
            strip_controls(b"\x1b[31mred\x1b[0m\r\n\x1b]0;title\x07ok\x1b]2;t\x1b\\!\x1b="),
            b"red\r\nok!"
        );
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
