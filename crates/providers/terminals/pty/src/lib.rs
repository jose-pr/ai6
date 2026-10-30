// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-terminal-pty`: the `pty` terminal provider, on `portable-pty`, which is
//! `openpty` on Unix and ConPTY on Windows. The session logic is shared; what differs
//! per platform (owning the child's process tree, the terminal's mode, translating input) is
//! in `unix.rs` and `windows.rs` behind one `os` interface.
//!
//! Output is read on a thread into a channel, so `read` can time out: a blocking
//! master read is not reliably interrupted by killing the child. The child's exit
//! is asked of the child itself, never inferred from EOF, because a pseudo-console
//! can stay readable after its program has exited. For the same reason the end of the
//! output is not left to the master: once the child has exited and nothing more has arrived
//! for 200 ms, `read` reports it.
//!
//! On Windows the reading thread also removes cursor position queries (`ESC[6n`) from the
//! output, and another thread answers them, so the reader never waits on the writer. ConPTY
//! sends one before its program's first output and holds that output until it is answered
//! (`portable-pty` creates every pseudo-console with `PSEUDOCONSOLE_INHERIT_CURSOR`), and
//! nothing else is there to answer it. On Unix queries pass through, for whoever reads.

#![doc = include_str!("../README.md")]

ai6::guatiao::providers!(Pty);

#[cfg(unix)]
#[path = "unix.rs"]
mod os;
#[cfg(windows)]
#[path = "windows.rs"]
mod os;
#[cfg(unix)]
#[path = "../../process_tree/unix.rs"]
mod process_tree;
#[cfg(windows)]
#[path = "../../process_tree/windows.rs"]
mod process_tree;

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ai6::directorates::q::terminal::{ExitInfo, READ_TIMED_OUT, Spec, Terminal, TerminalSession};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Schema, Status, ToValue, Value};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use os::ProcessTree;

const EXIT_POLL: Duration = Duration::from_millis(10);
/// How often a waiting `read` asks whether the child has exited.
const EXIT_CHECK: Duration = Duration::from_millis(50);
/// ConPTY takes rows and columns as signed 16-bit numbers.
const MAX_DIMENSION: u16 = i16::MAX as u16;
/// A cursor position query (DSR 6).
#[cfg(any(windows, test))]
const CURSOR_QUERY: &[u8] = b"\x1b[6n";
/// The answer to [`CURSOR_QUERY`]: row 1, column 1, where a fresh terminal's cursor is.
#[cfg(windows)]
const CURSOR_REPORT: &[u8] = b"\x1b[1;1R";

/// `pty` provider configuration; every field has a default.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq)]
#[map(crate = ::ai6::guatiao)]
pub struct PtyConfig {
    /// Bytes of one read of the program's output. At least 1. Default 65536.
    #[map(default = 65536)]
    pub read_chunk_bytes: u64,
    /// Milliseconds the output must stay quiet after the program has exited before `read`
    /// reports its end: ConPTY can deliver the last of a program's output after the program has
    /// exited, and keeps the output open until the pseudo-console is closed. Default 200.
    #[map(default = 200)]
    pub drain_quiet_ms: u64,
    /// Milliseconds `kill` waits for the program and its process tree to have exited. Default
    /// 2000.
    #[map(default = 2_000)]
    pub kill_wait_ms: u64,
    /// How many times the process table is read again for processes started meanwhile when a
    /// tree is adopted (Windows) or killed (Unix). Default 8 on Windows and 32 elsewhere.
    #[map(default = DEFAULT_TREE_SCANS)]
    pub tree_scans: u64,
}

/// The default of [`PtyConfig::tree_scans`].
const DEFAULT_TREE_SCANS: u64 = if cfg!(windows) { 8 } else { 32 };

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            read_chunk_bytes: 65536,
            drain_quiet_ms: 200,
            kill_wait_ms: 2_000,
            tree_scans: DEFAULT_TREE_SCANS,
        }
    }
}

/// Starts programs on a pseudo-terminal.
#[derive(Debug, Clone, Copy, Default, ai6::guatiao::Provider)]
#[provider(
    Terminal,
    id = "pty",
    name = "pty",
    crate = ::ai6::guatiao,
    config = PtyConfig
)]
pub struct Pty {
    config: PtyConfig,
}

impl Pty {
    /// A provider with `config`.
    pub fn new(config: PtyConfig) -> Self {
        Self { config }
    }
}

impl TryFrom<PtyConfig> for Pty {
    type Error = ProviderError;

    fn try_from(config: PtyConfig) -> Result<Self, Self::Error> {
        if config.read_chunk_bytes == 0 {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "read_chunk_bytes is 0: a read of no bytes never sees any output",
            ));
        }
        Ok(Self::new(config))
    }
}

impl Terminal for Pty {
    fn spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
        let spec = Spec::from_value(spec).map_err(|e| refuse(Status::GUATIAO_ERR_BAD_VALUE, e))?;
        Ok(PtySession::start(&spec, self.config)?.into_object())
    }
}

struct PtySession {
    pid: i64,
    chunks: Mutex<Receiver<Vec<u8>>>,
    /// The part of a chunk that did not fit the caller's buffer.
    pending: Mutex<Vec<u8>>,
    eof: AtomicBool,
    /// Since when the child has been seen exited with no output arriving.
    quiet_since: Mutex<Option<Instant>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    tree: ProcessTree,
    exit: Mutex<Option<ExitInfo>>,
    /// How long the output must stay quiet after the child has exited, and how long `kill`
    /// waits for it to have gone.
    drain_quiet: Duration,
    kill_wait: Duration,
}

impl PtySession {
    fn start(spec: &Spec, config: PtyConfig) -> Result<Self, ProviderError> {
        let read_chunk = usize::try_from(config.read_chunk_bytes).unwrap_or(usize::MAX);
        let (program, args) = spec
            .argv
            .split_first()
            .ok_or_else(|| refuse(Status::GUATIAO_ERR_BAD_VALUE, "argv is empty"))?;
        let pair = native_pty_system()
            .openpty(size(spec.rows, spec.cols))
            .map_err(internal)?;
        os::set_terminal_mode(&*pair.master).map_err(internal)?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        if let Some(env) = &spec.env {
            cmd.env_clear();
            for (k, v) in env {
                cmd.env(k, v);
            }
        }
        if cmd.get_env("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }
        if let Some(dir) = &spec.cwd {
            cmd.cwd(dir);
        }
        let child = pair.slave.spawn_command(cmd).map_err(internal)?;
        let tree = match os::adopt(
            child.process_id(),
            usize::try_from(config.tree_scans).unwrap_or(usize::MAX),
        ) {
            Ok(tree) => tree,
            Err(error) => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();
                return Err(internal(error));
            }
        };
        // The child owns the slave now; holding it here would keep the pty open past its exit.
        drop(pair.slave);
        let pid = child.process_id().map(i64::from).unwrap_or(-1);

        let mut reader = pair.master.try_clone_reader().map_err(internal)?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer().map_err(internal)?));
        #[cfg(windows)]
        let asks = {
            // A write can block while ConPTY is not reading, holding the writer: the reader
            // only asks, so the program's output keeps draining meanwhile.
            let (asks, asked) = channel::<()>();
            let answers = writer.clone();
            std::thread::Builder::new()
                .name(format!("pty-{pid}-answers"))
                .spawn(move || {
                    for () in asked {
                        answer(&answers);
                    }
                })
                .map_err(internal)?;
            asks
        };
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name(format!("pty-{pid}"))
            .spawn(move || {
                let mut buf = vec![0u8; read_chunk];
                #[cfg(windows)]
                let mut queries = CursorQueries::default();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => {
                            #[cfg(windows)]
                            {
                                let held = queries.finish();
                                if !held.is_empty() {
                                    let _ = tx.send(held);
                                }
                            }
                            break;
                        }
                        Ok(n) => {
                            #[cfg(windows)]
                            let output = {
                                let (output, asked) = queries.filter(&buf[..n]);
                                for _ in 0..asked {
                                    let _ = asks.send(());
                                }
                                output
                            };
                            #[cfg(not(windows))]
                            let output = buf[..n].to_vec();
                            if !output.is_empty() && tx.send(output).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
            .map_err(internal)?;

        Ok(Self {
            pid,
            chunks: Mutex::new(rx),
            pending: Mutex::new(Vec::new()),
            eof: AtomicBool::new(false),
            quiet_since: Mutex::new(None),
            writer,
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            tree,
            exit: Mutex::new(None),
            drain_quiet: Duration::from_millis(config.drain_quiet_ms),
            kill_wait: Duration::from_millis(config.kill_wait_ms),
        })
    }

    fn poll_exit(&self) -> Result<Option<ExitInfo>, ProviderError> {
        let mut exit = lock(&self.exit)?;
        if exit.is_none()
            && let Some(status) = lock(&self.child)?.try_wait().map_err(internal)?
        {
            *exit = Some(exit_info(&status));
        }
        Ok(*exit)
    }
}

impl TerminalSession for PtySession {
    fn pid(&self) -> i64 {
        self.pid
    }

    fn read(&self, dst: &mut [u8], timeout_ms: i64) -> Result<i64, ProviderError> {
        let mut pending = lock(&self.pending)?;
        if pending.is_empty() {
            if self.eof.load(Ordering::Acquire) {
                return Ok(0);
            }
            let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(0) as u64);
            let chunks = lock(&self.chunks)?;
            loop {
                let now = Instant::now();
                let drained_at = match self.poll_exit()? {
                    Some(_) => {
                        Some(*lock(&self.quiet_since)?.get_or_insert(now) + self.drain_quiet)
                    }
                    None => None,
                };
                let until = drained_at.unwrap_or(now + EXIT_CHECK).min(deadline);
                match chunks.recv_timeout(until.saturating_duration_since(now)) {
                    Ok(chunk) => {
                        *lock(&self.quiet_since)? = None;
                        *pending = chunk;
                        break;
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        // The terminal can close before the child is reaped; the end of output
                        // promises its exit status, so wait for it as kill does.
                        let settle = Instant::now() + self.kill_wait;
                        while self.poll_exit()?.is_none() && Instant::now() < settle {
                            std::thread::sleep(EXIT_POLL);
                        }
                        self.eof.store(true, Ordering::Release);
                        return Ok(0);
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        let now = Instant::now();
                        if drained_at.is_some_and(|at| now >= at) {
                            self.eof.store(true, Ordering::Release);
                            return Ok(0);
                        }
                        if now >= deadline {
                            return Ok(READ_TIMED_OUT);
                        }
                    }
                }
            }
        }
        let n = pending.len().min(dst.len());
        dst[..n].copy_from_slice(&pending[..n]);
        pending.drain(..n);
        Ok(n as i64)
    }

    fn write(&self, data: &[u8]) -> Result<(), ProviderError> {
        let mut writer = lock(&self.writer)?;
        let input = os::translate_input(data);
        writer
            .write_all(&input)
            .and_then(|()| writer.flush())
            .map_err(internal)
    }

    fn resize(&self, rows: i64, cols: i64) -> Result<(), ProviderError> {
        let (rows, cols) = (dimension(rows)?, dimension(cols)?);
        lock(&self.master)?
            .resize(size(rows, cols))
            .map_err(internal)
    }

    fn kill(&self) -> Result<(), ProviderError> {
        let leader_alive = lock(&self.exit)?.is_none();
        self.tree.kill(leader_alive).map_err(internal)?;
        // An error here means it already exited, which is what was asked.
        let _ = lock(&self.child)?.kill();
        // Killing is asynchronous; a caller may remove the working directory next.
        let deadline = Instant::now() + self.kill_wait;
        while (self.poll_exit()?.is_none() || self.tree.running()) && Instant::now() < deadline {
            std::thread::sleep(EXIT_POLL);
        }
        Ok(())
    }

    fn exit_status(&self, timeout_ms: i64) -> Result<Value, ProviderError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(0) as u64);
        loop {
            if let Some(exit) = self.poll_exit()? {
                return exit.to_value().map_err(internal);
            }
            if Instant::now() >= deadline {
                return ExitInfo {
                    running: true,
                    ..ExitInfo::default()
                }
                .to_value()
                .map_err(internal);
            }
            std::thread::sleep(EXIT_POLL);
        }
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = TerminalSession::kill(self);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.wait();
        }
    }
}

/// Writes [`CURSOR_REPORT`] as the terminal's answer. A failed write means the program is gone.
#[cfg(windows)]
fn answer(writer: &Mutex<Box<dyn Write + Send>>) {
    if let Ok(mut writer) = writer.lock() {
        let _ = writer
            .write_all(CURSOR_REPORT)
            .and_then(|()| writer.flush());
    }
}

/// Removes [`CURSOR_QUERY`] from a stream read in chunks, holding back the end of a chunk that
/// may be the start of one.
#[cfg(any(windows, test))]
#[derive(Default)]
struct CursorQueries {
    held: Vec<u8>,
}

#[cfg(any(windows, test))]
impl CursorQueries {
    /// The chunk with every whole query removed, and how many there were.
    fn filter(&mut self, chunk: &[u8]) -> (Vec<u8>, usize) {
        let mut bytes = std::mem::take(&mut self.held);
        bytes.extend_from_slice(chunk);
        let mut output = Vec::with_capacity(bytes.len());
        let mut asked = 0;
        let mut index = 0;
        while index < bytes.len() {
            let rest = &bytes[index..];
            if rest.starts_with(CURSOR_QUERY) {
                asked += 1;
                index += CURSOR_QUERY.len();
            } else if CURSOR_QUERY.starts_with(rest) {
                self.held = rest.to_vec();
                break;
            } else {
                output.push(bytes[index]);
                index += 1;
            }
        }
        (output, asked)
    }

    /// What was held back, once the stream has ended.
    fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.held)
    }
}

fn exit_info(status: &portable_pty::ExitStatus) -> ExitInfo {
    match status.signal() {
        Some(name) => ExitInfo {
            running: false,
            code: None,
            signal: Some(signal_number(name)),
        },
        None => ExitInfo {
            running: false,
            // `portable-pty` keeps Windows' unsigned exit code; it is reported as the signed
            // 32-bit value the standard library and every transport give (`0xFFFFFFFF` is -1).
            code: Some(i64::from(status.exit_code().cast_signed())),
            signal: None,
        },
    }
}

/// `portable-pty` reports a signal by name; the contract carries its number.
fn signal_number(name: &str) -> i64 {
    match name.trim_start_matches("SIG").to_ascii_uppercase().as_str() {
        "HUP" | "HANGUP" => 1,
        "INT" | "INTERRUPT" => 2,
        "QUIT" => 3,
        "ABRT" | "ABORTED" => 6,
        "KILL" | "KILLED" => 9,
        "SEGV" | "SEGMENTATION FAULT" => 11,
        "PIPE" | "BROKEN PIPE" => 13,
        "TERM" | "TERMINATED" => 15,
        _ => -1,
    }
}

fn size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows: rows.min(MAX_DIMENSION),
        cols: cols.min(MAX_DIMENSION),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn dimension(n: i64) -> Result<u16, ProviderError> {
    u16::try_from(n)
        .ok()
        .filter(|&n| (1..=MAX_DIMENSION).contains(&n))
        .ok_or_else(|| {
            refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "rows and cols must be 1 to 32767",
            )
        })
}

fn lock<T>(m: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, ProviderError> {
    m.lock()
        .map_err(|_| refuse(Status::GUATIAO_ERR_INTERNAL, "a session lock was poisoned"))
}

fn refuse(status: Status, message: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(status, &message.to_string())
}

fn internal(e: impl std::fmt::Display) -> ProviderError {
    refuse(Status::GUATIAO_ERR_INTERNAL, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filtered(chunks: &[&[u8]]) -> (Vec<u8>, usize) {
        let mut queries = CursorQueries::default();
        let mut output = Vec::new();
        let mut asked = 0;
        for chunk in chunks {
            let (out, n) = queries.filter(chunk);
            output.extend(out);
            asked += n;
        }
        output.extend(queries.finish());
        (output, asked)
    }

    #[test]
    fn an_exit_code_with_the_high_bit_set_is_negative() {
        for (raw, code) in [
            (0, 0),
            (3, 3),
            (0xFFFF_FFFF, -1),
            (0xC000_013A, -1_073_741_510),
        ] {
            let exit = exit_info(&portable_pty::ExitStatus::with_exit_code(raw));
            assert_eq!(exit.code, Some(code), "{raw:#010X}");
        }
    }

    #[test]
    fn a_cursor_query_is_counted_and_removed_even_across_chunks() {
        assert_eq!(
            filtered(&[b"\x1b[6n\x1b[?9001hready\r\n"]),
            (b"\x1b[?9001hready\r\n".to_vec(), 1)
        );
        assert_eq!(
            filtered(&[b"a\x1b", b"[", b"6", b"nb\x1b[6n"]),
            (b"ab".to_vec(), 2)
        );
    }

    #[test]
    fn other_sequences_and_a_held_tail_pass_through() {
        assert_eq!(
            filtered(&[b"\x1b[6m\x1b[66n", b"\x1b[6"]),
            (b"\x1b[6m\x1b[66n\x1b[6".to_vec(), 0)
        );
    }
}
