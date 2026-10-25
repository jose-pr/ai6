// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-terminal-pty`: the `pty` terminal provider, on `portable-pty`, which is
//! `openpty` on Unix and ConPTY on Windows. The session logic is shared; what differs
//! per platform (owning the child's process tree, translating input) is in `unix.rs`
//! and `windows.rs` behind one `os` interface.
//!
//! Output is read on a thread into a channel, so `read` can time out: a blocking
//! master read is not reliably interrupted by killing the child. The child's exit
//! is asked of the child itself, never inferred from EOF, because a pseudo-console
//! can stay readable after its program has exited.

#![doc = include_str!("../README.md")]

ai6::guatiao::providers!(Pty);

#[cfg(unix)]
#[path = "unix.rs"]
mod os;
#[cfg(windows)]
#[path = "windows.rs"]
mod os;

use std::io::{Read, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use ai6::directorates::q::terminal::{ExitInfo, READ_TIMED_OUT, Spec, Terminal, TerminalSession};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Status, ToValue, Value};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use os::ProcessTree;

const READ_CHUNK: usize = 65536;
const EXIT_POLL: Duration = Duration::from_millis(10);

#[derive(Default, ai6::guatiao::Provider)]
#[provider(
    Terminal,
    id = "pty",
    name = "pty",
    crate = ::ai6::guatiao
)]
pub struct Pty;

impl Terminal for Pty {
    fn spawn(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
        let spec = Spec::from_value(spec).map_err(|e| refuse(Status::GUATIAO_ERR_BAD_VALUE, e))?;
        Ok(PtySession::start(&spec)?.into_object())
    }
}

struct PtySession {
    pid: i64,
    chunks: Mutex<Receiver<Vec<u8>>>,
    /// The part of a chunk that did not fit the caller's buffer.
    pending: Mutex<Vec<u8>>,
    eof: AtomicBool,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    tree: ProcessTree,
    exit: Mutex<Option<ExitInfo>>,
}

impl PtySession {
    fn start(spec: &Spec) -> Result<Self, ProviderError> {
        let (program, args) = spec
            .argv
            .split_first()
            .ok_or_else(|| refuse(Status::GUATIAO_ERR_BAD_VALUE, "argv is empty"))?;
        let pair = native_pty_system()
            .openpty(size(spec.rows, spec.cols))
            .map_err(internal)?;
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
        let tree = match ProcessTree::adopt(child.process_id()) {
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
        let writer = pair.master.take_writer().map_err(internal)?;
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name(format!("pty-{pid}"))
            .spawn(move || {
                let mut buf = vec![0u8; READ_CHUNK];
                loop {
                    match reader.read(&mut buf) {
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
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            tree,
            exit: Mutex::new(None),
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
            let timeout = Duration::from_millis(timeout_ms.max(0) as u64);
            match lock(&self.chunks)?.recv_timeout(timeout) {
                Ok(chunk) => *pending = chunk,
                Err(RecvTimeoutError::Timeout) => return Ok(READ_TIMED_OUT),
                Err(RecvTimeoutError::Disconnected) => {
                    self.eof.store(true, Ordering::Release);
                    return Ok(0);
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
        self.tree.kill().map_err(internal)?;
        // An error here means it already exited, which is what was asked.
        let _ = lock(&self.child)?.kill();
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

fn exit_info(status: &portable_pty::ExitStatus) -> ExitInfo {
    match status.signal() {
        Some(name) => ExitInfo {
            running: false,
            code: None,
            signal: Some(signal_number(name)),
        },
        None => ExitInfo {
            running: false,
            code: Some(i64::from(status.exit_code())),
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
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn dimension(n: i64) -> Result<u16, ProviderError> {
    u16::try_from(n).ok().filter(|&n| n > 0).ok_or_else(|| {
        refuse(
            Status::GUATIAO_ERR_BAD_VALUE,
            "rows and cols must be 1 to 65535",
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
