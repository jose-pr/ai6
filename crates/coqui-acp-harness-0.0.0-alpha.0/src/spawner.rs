//! The `acp_harness` kind's spawner on this machine, and [`Spawn`] through any host's spawner.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{Secret, Status};
use coqui_acp_api::{
    Environment, ErrorOutput, FileInfo, LinkState, PrivateDir, ProcessExit, ProcessSpawner,
    READ_LINK_DOWN, READ_TIMED_OUT, SecretBytes, SpawnSpec, SpawnedProcess, SpawnerInfo,
    TerminalSize,
};

use crate::process::{ErrorStream, Exit, Launch, Process, Spawn, Spawned, start};

/// Starts programs on this machine, as [`PlainSpawn`](crate::PlainSpawn) does, for a host to hand
/// every `acp_harness` start: the default a provider may ignore.
///
/// A process's tree ends at `kill`, when it is destroyed, and when the host ends without either:
/// [`PlainSpawn`](crate::PlainSpawn) says how on each platform. No terminal: a spec asking for one is
/// refused `GUATIAO_ERR_UNSUPPORTED`.
///
/// It serves the kind's file methods in one directory when given one with [`files`](Self::files),
/// and says `files` false and refuses them `GUATIAO_ERR_UNSUPPORTED` otherwise.
#[derive(Clone, Debug)]
#[must_use]
pub struct LocalSpawner {
    buffer: usize,
    files: Option<PrivateDir>,
}

impl Default for LocalSpawner {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalSpawner {
    /// A spawner holding at most 1 MiB of each output stream unread.
    pub const fn new() -> Self {
        Self {
            buffer: 1 << 20,
            files: None,
        }
    }

    /// Holds at most `bytes` of each output stream unread; past it, the program waits to write.
    pub const fn buffer(mut self, bytes: usize) -> Self {
        self.buffer = bytes;
        self
    }

    /// Serves the file methods in `dir`, which must exist: an account's private directory.
    pub fn files(mut self, dir: impl Into<PathBuf>) -> Self {
        self.files = Some(PrivateDir::new(dir));
        self
    }

    fn root(&self) -> Result<&PrivateDir, ProviderError> {
        self.files.as_ref().ok_or_else(|| {
            refused(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "this spawner has no private files",
            )
        })
    }
}

fn refused(status: Status, message: &str) -> ProviderError {
    ProviderError::new(status, message)
}

/// An `io::Error` as the kind's error: a missing program is `GUATIAO_ERR_NOT_FOUND`.
fn io_refused(error: &io::Error) -> ProviderError {
    let status = match error.kind() {
        io::ErrorKind::NotFound => Status::GUATIAO_ERR_NOT_FOUND,
        io::ErrorKind::BrokenPipe => Status::GUATIAO_ERR_GONE,
        _ => Status::GUATIAO_ERR_INTERNAL,
    };
    refused(status, &error.to_string())
}

impl ProcessSpawner for LocalSpawner {
    fn describe(&self) -> Result<SpawnerInfo, ProviderError> {
        let mut info = SpawnerInfo::new(std::env::consts::OS, std::env::consts::ARCH, true);
        info.separate_error = true;
        info.kills_escaped = cfg!(any(windows, target_os = "linux", target_os = "android"));
        info.ends_with_host = cfg!(any(windows, unix));
        info.files = self.files.is_some();
        Ok(info)
    }

    fn spawn(&self, spec: SpawnSpec) -> Result<Object<dyn SpawnedProcess>, ProviderError> {
        if spec.terminal.is_some() {
            return Err(refused(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "this spawner gives a program no terminal",
            ));
        }
        let errors = match spec.error_output {
            ErrorOutput::Separate => ErrorStream::Piped,
            ErrorOutput::Merged => ErrorStream::IntoOutput,
            ErrorOutput::Discarded => ErrorStream::Discarded,
            _ => {
                return Err(refused(
                    Status::GUATIAO_ERR_UNSUPPORTED,
                    "this spawner does not know where that standard error goes",
                ));
            }
        };
        let mut launch = Launch::new(&spec.program)
            .args(&spec.args)
            .clear_env(spec.clear_environment);
        launch.remove_env = spec.remove_environment.iter().map(OsString::from).collect();
        for (name, value) in spec.environment.expose() {
            launch = launch.env(name, value);
        }
        if let Some(cwd) = &spec.cwd {
            launch = launch.cwd(cwd);
        }
        let spawned = start(&launch, errors).map_err(|e| io_refused(&e))?;
        Ok(LocalProcess::new(spawned, self.buffer)
            .map_err(|e| io_refused(&e))?
            .into_object())
    }

    fn write_file(
        &self,
        path: &str,
        bytes: Secret<SecretBytes>,
        private: bool,
    ) -> Result<(), ProviderError> {
        self.root()?.write(path, bytes.expose().as_slice(), private)
    }

    fn read_file(&self, path: &str, max_bytes: u64) -> Result<Secret<SecretBytes>, ProviderError> {
        self.root()?
            .read(path, max_bytes)
            .map(|bytes| Secret::new(SecretBytes::new(bytes)))
    }

    fn file_info(&self, path: &str) -> Result<Option<FileInfo>, ProviderError> {
        self.root()?.info(path)
    }

    fn remove_file(&self, path: &str) -> Result<(), ProviderError> {
        self.root()?.remove(path)
    }
}

#[derive(Default)]
struct Buffered {
    bytes: VecDeque<u8>,
    ended: bool,
}

/// One output stream, read by a thread of its own into a bounded buffer.
struct Pump {
    shared: Arc<(Mutex<Buffered>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Pump {
    fn start(mut from: Box<dyn Read + Send>, limit: usize) -> io::Result<Self> {
        let shared = Arc::new((Mutex::new(Buffered::default()), Condvar::new()));
        let filling = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("coqui-acp-harness-output".into())
            .spawn(move || {
                let (lock, changed) = &*filling;
                let mut chunk = [0u8; 8192];
                loop {
                    let read = from.read(&mut chunk);
                    let mut buffered = lock.lock().unwrap_or_else(PoisonError::into_inner);
                    match read {
                        Ok(0) | Err(_) => {
                            buffered.ended = true;
                            changed.notify_all();
                            return;
                        }
                        Ok(n) => {
                            while buffered.bytes.len() >= limit.max(1) {
                                buffered = changed
                                    .wait(buffered)
                                    .unwrap_or_else(PoisonError::into_inner);
                            }
                            buffered.bytes.extend(&chunk[..n]);
                            changed.notify_all();
                        }
                    }
                }
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    fn read(&self, dst: &mut [u8], timeout: Duration) -> i64 {
        let (lock, changed) = &*self.shared;
        let deadline = Instant::now().checked_add(timeout);
        let mut buffered = lock.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if !buffered.bytes.is_empty() {
                let n = dst.len().min(buffered.bytes.len());
                for (slot, byte) in dst.iter_mut().zip(buffered.bytes.drain(..n)) {
                    *slot = byte;
                }
                changed.notify_all();
                return i64::try_from(n).unwrap_or(i64::MAX);
            }
            if buffered.ended {
                return 0;
            }
            let now = Instant::now();
            let Some(left) = deadline.map(|d| d.saturating_duration_since(now)) else {
                buffered = changed
                    .wait(buffered)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            };
            if left.is_zero() {
                return READ_TIMED_OUT;
            }
            buffered = changed
                .wait_timeout(buffered, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for Pump {
    /// Joins its thread when it has ended; one still reading a pipe some escaped process holds open ends when that
    /// process does, so dropping never waits for it.
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take_if(|thread| thread.is_finished()) {
            let _ = thread.join();
        }
    }
}

/// A process this machine runs, as the kind's object.
struct LocalProcess {
    stdin: Mutex<Option<Box<dyn Write + Send>>>,
    output: Pump,
    error: Option<Pump>,
    process: Box<dyn Process>,
}

impl LocalProcess {
    fn new(spawned: Spawned, buffer: usize) -> io::Result<Self> {
        let Spawned {
            stdin,
            stdout,
            stderr,
            process,
        } = spawned;
        Ok(Self {
            stdin: Mutex::new(Some(stdin)),
            output: Pump::start(stdout, buffer)?,
            error: stderr.map(|e| Pump::start(e, buffer)).transpose()?,
            process,
        })
    }

    fn stdin(&self) -> MutexGuard<'_, Option<Box<dyn Write + Send>>> {
        self.stdin.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn exit(exit: Option<Exit>) -> ProcessExit {
    match exit {
        None => ProcessExit::running(),
        Some(exit) => ProcessExit::ended(exit.code.map(i64::from), exit.signal.map(i64::from)),
    }
}

impl SpawnedProcess for LocalProcess {
    fn write(&self, bytes: &[u8]) -> Result<(), ProviderError> {
        let mut stdin = self.stdin();
        let Some(pipe) = stdin.as_mut() else {
            return Err(refused(Status::GUATIAO_ERR_GONE, "its input is closed"));
        };
        if let Err(error) = pipe.write_all(bytes).and_then(|()| pipe.flush()) {
            *stdin = None;
            return Err(if error.kind() == io::ErrorKind::BrokenPipe {
                refused(Status::GUATIAO_ERR_GONE, "it no longer reads its input")
            } else {
                io_refused(&error)
            });
        }
        Ok(())
    }

    fn close_input(&self) -> Result<(), ProviderError> {
        drop(self.stdin().take());
        Ok(())
    }

    fn read(&self, dst: &mut [u8], timeout_ms: u64) -> Result<i64, ProviderError> {
        Ok(self.output.read(dst, Duration::from_millis(timeout_ms)))
    }

    fn read_error(&self, dst: &mut [u8], timeout_ms: u64) -> Result<i64, ProviderError> {
        Ok(self
            .error
            .as_ref()
            .map_or(0, |pump| pump.read(dst, Duration::from_millis(timeout_ms))))
    }

    fn resize(&self, _size: TerminalSize) -> Result<(), ProviderError> {
        Err(refused(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "the process has no terminal",
        ))
    }

    fn wait(&self, timeout_ms: u64) -> Result<ProcessExit, ProviderError> {
        let deadline = Instant::now().checked_add(Duration::from_millis(timeout_ms));
        loop {
            let now = self.process.try_wait().map_err(|e| io_refused(&e))?;
            if now.is_some() || deadline.is_some_and(|d| Instant::now() >= d) {
                return Ok(exit(now));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill(&self) -> Result<(), ProviderError> {
        drop(self.stdin().take());
        self.process.kill().map_err(|e| io_refused(&e))
    }

    fn link(&self) -> Result<LinkState, ProviderError> {
        Ok(LinkState::up())
    }
}

/// [`Spawn`] through a host's `process_spawner`: what a harness provider written over
/// [`serve`](crate::serve) starts its program with, so its program runs wherever the host's spawner runs it.
///
/// Standard error is kept apart. Reading waits through a link that is down, as the spawner keeps the output.
pub struct ThroughSpawner {
    spawner: Object<dyn ProcessSpawner>,
}

impl core::fmt::Debug for ThroughSpawner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ThroughSpawner").finish_non_exhaustive()
    }
}

impl ThroughSpawner {
    /// Starts programs through `spawner`.
    pub fn new(spawner: Object<dyn ProcessSpawner>) -> Self {
        Self { spawner }
    }
}

fn text(what: &str, value: &std::ffi::OsStr) -> io::Result<String> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other(alloc::format!("{what} is not UTF-8")))
}

fn as_io(error: &ProviderError) -> io::Error {
    let kind = match error.status {
        Status::GUATIAO_ERR_NOT_FOUND => io::ErrorKind::NotFound,
        Status::GUATIAO_ERR_GONE => io::ErrorKind::BrokenPipe,
        Status::GUATIAO_ERR_UNSUPPORTED => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, error.message().to_owned())
}

impl Spawn for ThroughSpawner {
    fn spawn(&self, launch: &Launch) -> io::Result<Spawned> {
        let mut spec = SpawnSpec::new(text("the program", &launch.program)?)
            .clear_environment(launch.clear_env)
            .error_output(ErrorOutput::Separate);
        for arg in &launch.args {
            spec = spec.arg(text("an argument", arg)?);
        }
        if let Some(cwd) = &launch.cwd {
            spec = spec.cwd(text("the directory", cwd.as_os_str())?);
        }
        let mut environment = Environment::new();
        for (name, value) in &launch.env {
            environment.insert(text("a variable's name", name)?, text("a variable", value)?);
        }
        spec = spec.environment(environment);
        spec.remove_environment = launch
            .remove_env
            .iter()
            .map(|name| text("a variable's name", name))
            .collect::<io::Result<Vec<_>>>()?;
        let process = Arc::new(self.spawner.spawn(spec).map_err(|e| as_io(&e))?);
        Ok(Spawned::new(
            Box::new(Input(Arc::clone(&process))),
            Box::new(Output {
                process: Arc::clone(&process),
                error: false,
            }),
            Some(Box::new(Output {
                process: Arc::clone(&process),
                error: true,
            })),
            Box::new(Remote(process)),
        ))
    }
}

type Shared = Arc<Object<dyn SpawnedProcess>>;

struct Input(Shared);

impl Write for Input {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf).map_err(|e| as_io(&e))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        let _ = self.0.close_input();
    }
}

struct Output {
    process: Shared,
    error: bool,
}

impl Read for Output {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let read = if self.error {
                self.process.read_error(buf, 1000)
            } else {
                self.process.read(buf, 1000)
            }
            .map_err(|e| as_io(&e))?;
            match read {
                READ_TIMED_OUT | READ_LINK_DOWN => {}
                n if n < 0 => return Err(io::Error::other("the spawner answered no count")),
                n => return Ok(usize::try_from(n).unwrap_or(0)),
            }
        }
    }
}

struct Remote(Shared);

impl Process for Remote {
    fn kill(&self) -> io::Result<()> {
        self.0.kill().map_err(|e| as_io(&e))
    }

    fn try_wait(&self) -> io::Result<Option<Exit>> {
        let now = self.0.wait(0).map_err(|e| as_io(&e))?;
        Ok((!now.running).then(|| Exit {
            code: now.code.and_then(|c| i32::try_from(c).ok()),
            signal: now.signal.and_then(|s| i32::try_from(s).ok()),
        }))
    }
}
