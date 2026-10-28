// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-terminal-pipe`: the `pipe` terminal provider. The program gets plain pipes for its
//! standard input and output, and no terminal, so a harness that checks whether its input is a
//! terminal (Claude Code's `-p`) reads what is written to it, and its output arrives exactly as
//! written. What differs per platform (starting the child alone, owning its process tree,
//! finding the program) is in `unix.rs` and `windows.rs` behind one `os` interface.
//!
//! Output is read on a thread into a channel, so `read` can time out. The output ends when the
//! program has exited and its output pipe is closed, or, when a process it left behind holds the
//! pipe open, once the program has exited and nothing has arrived for 200 ms.

#![doc = include_str!("../README.md")]
#![deny(unsafe_code)]

ai6::guatiao::providers!(Pipe);

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

use std::collections::{BTreeMap, VecDeque};
use std::io::{PipeReader, PipeWriter, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ai6::directorates::q::terminal::{ExitInfo, READ_TIMED_OUT, Spec, Terminal, TerminalSession};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue, Value};

use os::Process;
use process_tree::ProcessTree;

const READ_CHUNK: usize = 65536;
const EXIT_POLL: Duration = Duration::from_millis(10);
/// How often a waiting `read` asks whether the child has exited.
const EXIT_CHECK: Duration = Duration::from_millis(50);
/// How long the output must stay quiet after the child has exited before `read` reports its
/// end while a process the child left behind still holds the output pipe.
const DRAIN_QUIET: Duration = Duration::from_millis(200);
/// The longest `kill` waits for the child and its process tree to have exited.
const KILL_WAIT: Duration = Duration::from_secs(2);
/// How much of the end of standard error [`StderrMode::Tail`] keeps.
pub const STDERR_TAIL_BYTES: usize = 8192;

/// What becomes of the program's standard error.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[map(crate = ::ai6::guatiao, rename_all = "lowercase")]
pub enum StderrMode {
    /// Read through a pipe of its own, and only its last [`STDERR_TAIL_BYTES`] (from the first
    /// whole line in them) added to the output after the program's own output has ended, as
    /// lines of their own. A JSON stream on standard output is never broken up by it, and what
    /// the program said last on standard error, such as why it failed, is still read.
    #[default]
    Tail,
    /// Written into the output stream, in the order the program wrote it.
    Merge,
    /// Discarded.
    Drop,
}

/// `pipe` provider configuration; every field has a default.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[map(crate = ::ai6::guatiao)]
pub struct PipeConfig {
    /// What becomes of standard error: `tail` (the default), `merge` or `drop`.
    #[map(default = StderrMode::Tail)]
    pub stderr: StderrMode,
}

/// Starts programs on plain pipes, with no terminal.
#[derive(Debug, Clone, Copy, Default, Provider)]
#[provider(
    Terminal,
    id = "pipe",
    name = "pipe",
    crate = ::ai6::guatiao,
    config = PipeConfig
)]
pub struct Pipe {
    config: PipeConfig,
}

impl Pipe {
    /// A provider with `config`.
    pub fn new(config: PipeConfig) -> Self {
        Self { config }
    }
}

impl TryFrom<PipeConfig> for Pipe {
    type Error = ProviderError;

    fn try_from(config: PipeConfig) -> Result<Self, Self::Error> {
        Ok(Self::new(config))
    }
}

impl Terminal for Pipe {
    fn spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
        let spec = Spec::from_value(spec).map_err(|e| refuse(Status::GUATIAO_ERR_BAD_VALUE, e))?;
        Ok(PipeSession::start(&spec, self.config.stderr)?.into_object())
    }

    fn tty(&self) -> bool {
        false
    }
}

struct PipeSession {
    pid: i64,
    chunks: Mutex<Receiver<Vec<u8>>>,
    /// The part of a chunk that did not fit the caller's buffer.
    pending: Mutex<Vec<u8>>,
    /// Whether `read` has reported the end of the output.
    eof: AtomicBool,
    /// Whether the output pipe has closed and everything it carried has been taken.
    closed: AtomicBool,
    /// Since when the child has been seen exited with no output arriving.
    quiet_since: Mutex<Option<Instant>>,
    stdin: Mutex<PipeWriter>,
    child: Mutex<Process>,
    tree: ProcessTree,
    exit: Mutex<Option<ExitInfo>>,
    /// With [`StderrMode::Tail`], the end of standard error, until the output has ended.
    errors: Option<Arc<Mutex<ErrorTail>>>,
    /// The last byte `read` handed out, `\n` before any.
    last_byte: AtomicU8,
}

/// The end of a program's standard error, as its own pipe delivers it.
#[derive(Default)]
struct ErrorTail {
    bytes: VecDeque<u8>,
    /// Whether bytes were dropped from the front.
    cut: bool,
    /// Whether the pipe has closed.
    closed: bool,
    /// Whether the tail has been added to the output.
    taken: bool,
}

impl ErrorTail {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        let over = self.bytes.len().saturating_sub(STDERR_TAIL_BYTES);
        if over > 0 {
            self.bytes.drain(..over);
            self.cut = true;
        }
    }

    /// The tail as whole lines, once: from the first line that was not cut, ending with a line
    /// feed, and starting with one when the output it follows (whose last byte is `last`) ended
    /// inside a line.
    fn take(&mut self, last: u8) -> Vec<u8> {
        if self.taken {
            return Vec::new();
        }
        self.taken = true;
        let mut bytes: Vec<u8> = std::mem::take(&mut self.bytes).into();
        if self.cut
            && let Some(at) = bytes.iter().position(|&byte| byte == b'\n')
            && at + 1 < bytes.len()
        {
            bytes.drain(..=at);
        }
        if bytes.is_empty() {
            return bytes;
        }
        if bytes.last() != Some(&b'\n') {
            bytes.push(b'\n');
        }
        if last != b'\n' {
            bytes.insert(0, b'\n');
        }
        bytes
    }
}

/// A program to start, and the child's ends of its pipes, which `os::spawn` closes in this
/// process once the child has them: until they close, the output never ends.
pub(crate) struct Launch<'a> {
    pub(crate) program: &'a str,
    pub(crate) args: &'a [String],
    /// The whole environment, or `None` for this process's.
    pub(crate) env: Option<&'a BTreeMap<String, String>>,
    pub(crate) cwd: Option<&'a str>,
    pub(crate) stdin: PipeReader,
    pub(crate) stdout: PipeWriter,
    pub(crate) stderr: ErrorOutput,
}

/// Where the child's standard error goes.
pub(crate) enum ErrorOutput {
    /// Into standard output's pipe, which keeps the order the program wrote them in.
    Stdout,
    /// To the null device.
    Null,
    /// Into a pipe of its own.
    Pipe(PipeWriter),
}

impl PipeSession {
    fn start(spec: &Spec, stderr: StderrMode) -> Result<Self, ProviderError> {
        let (program, args) = spec
            .argv
            .split_first()
            .ok_or_else(|| refuse(Status::GUATIAO_ERR_BAD_VALUE, "argv is empty"))?;
        let (stdin, input) = std::io::pipe().map_err(internal)?;
        let (mut output, writer) = std::io::pipe().map_err(internal)?;
        let (error_output, error_reader) = match stderr {
            StderrMode::Tail => {
                let (reader, writer) = std::io::pipe().map_err(internal)?;
                (ErrorOutput::Pipe(writer), Some(reader))
            }
            StderrMode::Merge => (ErrorOutput::Stdout, None),
            StderrMode::Drop => (ErrorOutput::Null, None),
        };
        let launch = Launch {
            program,
            args,
            env: spec.env.as_ref(),
            cwd: spec.cwd.as_deref(),
            stdin,
            stdout: writer,
            stderr: error_output,
        };
        let (child, tree) = os::spawn(launch)
            .map_err(|error| internal(format!("cannot start {program:?}: {error}")))?;
        let pid = i64::from(child.id());
        let stdin = input;

        let errors = match error_reader {
            Some(mut reader) => {
                let errors = Arc::new(Mutex::new(ErrorTail::default()));
                let tail = errors.clone();
                std::thread::Builder::new()
                    .name(format!("pipe-{pid}-stderr"))
                    .spawn(move || {
                        let mut buf = vec![0u8; READ_CHUNK];
                        loop {
                            let read = reader.read(&mut buf);
                            let Ok(mut tail) = tail.lock() else { break };
                            match read {
                                Ok(0) | Err(_) => {
                                    tail.closed = true;
                                    break;
                                }
                                Ok(n) => tail.push(&buf[..n]),
                            }
                        }
                    })
                    .map_err(internal)?;
                Some(errors)
            }
            None => None,
        };

        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name(format!("pipe-{pid}"))
            .spawn(move || {
                let mut buf = vec![0u8; READ_CHUNK];
                loop {
                    match output.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
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
            closed: AtomicBool::new(false),
            quiet_since: Mutex::new(None),
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            tree,
            exit: Mutex::new(None),
            errors,
            last_byte: AtomicU8::new(b'\n'),
        })
    }

    /// The tail of standard error, once the output has ended, if it holds anything and has not
    /// been taken: waits up to [`DRAIN_QUIET`] for its pipe to close, which a process the
    /// program left running may hold open.
    fn error_tail(&self) -> Result<Option<Vec<u8>>, ProviderError> {
        let Some(errors) = &self.errors else {
            return Ok(None);
        };
        let deadline = Instant::now() + DRAIN_QUIET;
        loop {
            let mut tail = lock(errors)?;
            if tail.taken {
                return Ok(None);
            }
            if tail.closed || Instant::now() >= deadline {
                let bytes = tail.take(self.last_byte.load(Ordering::Acquire));
                return Ok((!bytes.is_empty()).then_some(bytes));
            }
            drop(tail);
            std::thread::sleep(EXIT_POLL);
        }
    }

    fn poll_exit(&self) -> Result<Option<ExitInfo>, ProviderError> {
        let mut exit = lock(&self.exit)?;
        if exit.is_none()
            && let Some(status) = lock(&self.child)?.try_wait().map_err(internal)?
        {
            *exit = Some(status);
        }
        Ok(*exit)
    }

    /// Reports the end of the output; every later `read` reports it again.
    fn end(&self) -> Result<i64, ProviderError> {
        self.eof.store(true, Ordering::Release);
        Ok(0)
    }
}

impl TerminalSession for PipeSession {
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
                let exited = self.poll_exit()?.is_some();
                if self.closed.load(Ordering::Acquire) {
                    // The pipe has closed; the output ends with the program.
                    if exited {
                        if let Some(tail) = self.error_tail()? {
                            *pending = tail;
                            break;
                        }
                        return self.end();
                    }
                    if now >= deadline {
                        return Ok(READ_TIMED_OUT);
                    }
                    std::thread::sleep(EXIT_POLL.min(deadline - now));
                    continue;
                }
                let drained_at = match exited {
                    true => Some(*lock(&self.quiet_since)?.get_or_insert(now) + DRAIN_QUIET),
                    false => None,
                };
                let until = drained_at.unwrap_or(now + EXIT_CHECK).min(deadline);
                match chunks.recv_timeout(until.saturating_duration_since(now)) {
                    Ok(chunk) => {
                        *lock(&self.quiet_since)? = None;
                        *pending = chunk;
                        break;
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        self.closed.store(true, Ordering::Release);
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        let now = Instant::now();
                        if drained_at.is_some_and(|at| now >= at) {
                            if let Some(tail) = self.error_tail()? {
                                *pending = tail;
                                break;
                            }
                            return self.end();
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
        if let Some(&last) = dst[..n].last() {
            self.last_byte.store(last, Ordering::Release);
        }
        Ok(n as i64)
    }

    fn write(&self, data: &[u8]) -> Result<(), ProviderError> {
        let mut stdin = lock(&self.stdin)?;
        stdin
            .write_all(data)
            .and_then(|()| stdin.flush())
            .map_err(internal)
    }

    /// Nothing to resize: there is no terminal.
    fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
        Ok(())
    }

    fn kill(&self) -> Result<(), ProviderError> {
        let leader_alive = lock(&self.exit)?.is_none();
        self.tree.kill(leader_alive).map_err(internal)?;
        // An error here means it already exited, which is what was asked.
        let _ = lock(&self.child)?.kill();
        // Killing is asynchronous; a caller may remove the working directory next.
        let deadline = Instant::now() + KILL_WAIT;
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

impl Drop for PipeSession {
    fn drop(&mut self) {
        let _ = TerminalSession::kill(self);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.wait();
        }
    }
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
