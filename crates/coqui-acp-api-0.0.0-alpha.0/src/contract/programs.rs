//! A spawner's process that runs a real program, for a provider that drives its harness's own command line.
//!
//! The suite's other processes do nothing until they are killed. A provider of a harness that starts a
//! program (a stand-in for it, named by the provider's own settings) is held to the same rules when the
//! spawner starts what it is asked: on this machine, standard error apart, with no terminal. This is the
//! smallest spawner that does so; a host's own is [`ProcessSpawner`](crate::ProcessSpawner)'s to provide.
//!
//! A program never runs against the person's home: it gets a home of the suite's own, and the variables that name a
//! command line's configuration directory are not passed on from this process. What the spec's environment sets stays.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use guatiao::Status;
use guatiao::library::ProviderError;

use crate::spawner::{
    ErrorOutput, LinkState, ProcessExit, READ_TIMED_OUT, SpawnSpec, SpawnedProcess, TerminalSize,
};

/// The variable that names the home the suite gives its programs, set by `acp-harness-check --program-agent` for the
/// process it runs the suite in, and by a test that does the same. A stand-in program runs only where it is set.
pub const HOME_VARIABLE: &str = "ACP_HARNESS_CHECK_HOME";

/// The variables that place a command line's home, configuration and state, set to the suite's home.
const HOME_VARIABLES: [&str; 7] = [
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
];

/// The variables a command line reads its configuration directory from, which are not passed on.
const CONFIG_VARIABLES: [&str; 3] = ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "PI_CODING_AGENT_DIR"];

/// What one output stream of the program holds that has not been read.
#[derive(Default)]
struct Pipe {
    state: Mutex<PipeState>,
    arrived: Condvar,
}

#[derive(Default)]
struct PipeState {
    bytes: VecDeque<u8>,
    ended: bool,
}

impl Pipe {
    fn lock(&self) -> MutexGuard<'_, PipeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reads `source` to its end into the pipe, on a thread of its own.
    fn fill(self: &Arc<Self>, mut source: impl Read + Send + 'static) {
        let pipe = Arc::clone(self);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match source.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        pipe.lock().bytes.extend(&chunk[..n]);
                        pipe.arrived.notify_all();
                    }
                }
            }
            pipe.lock().ended = true;
            pipe.arrived.notify_all();
        });
    }

    fn read(&self, dst: &mut [u8], timeout: Duration) -> i64 {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if !state.bytes.is_empty() {
                let n = dst.len().min(state.bytes.len());
                for (slot, byte) in dst.iter_mut().zip(state.bytes.drain(..n)) {
                    *slot = byte;
                }
                return i64::try_from(n).unwrap_or(i64::MAX);
            }
            if state.ended {
                return 0;
            }
            let now = Instant::now();
            if now >= deadline {
                return READ_TIMED_OUT;
            }
            state = self
                .arrived
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// The process, until it has been collected: after that its id, and its group's, may be another process's.
struct Inner {
    child: Child,
    collected: Option<std::process::ExitStatus>,
}

struct Shared {
    inner: Mutex<Inner>,
    /// The program has exited or been killed.
    ended: Arc<AtomicBool>,
    /// The program is let go of: its watcher stops.
    dropped: AtomicBool,
    /// Holds the process and everything it starts; closing it ends them all.
    #[cfg(windows)]
    job: Mutex<Option<win32job::Job>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How the program ended, once it has, collecting it.
    fn exited(&self) -> Option<std::process::ExitStatus> {
        let mut inner = self.lock();
        if inner.collected.is_none() {
            inner.collected = inner.child.try_wait().ok().flatten();
        }
        if inner.collected.is_some() {
            self.ended.store(true, Ordering::SeqCst);
        }
        inner.collected
    }

    /// Ends the program and everything it started.
    fn end_tree(&self) {
        #[cfg(windows)]
        drop(
            self.job
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        let mut inner = self.lock();
        #[cfg(unix)]
        if inner.collected.is_none()
            && let Some(group) = i32::try_from(inner.child.id())
                .ok()
                .filter(|pid| *pid > 1)
                .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
        if inner.collected.is_none() {
            let _ = inner.child.kill();
            inner.collected = inner.child.wait().ok();
        }
        self.ended.store(true, Ordering::SeqCst);
    }
}

/// A program the suite started: its standard input and output, and whether it has ended.
pub(super) struct Program {
    shared: Arc<Shared>,
    input: Mutex<Option<ChildStdin>>,
    out: Arc<Pipe>,
    err: Arc<Pipe>,
}

fn refuse(status: Status, why: &str) -> ProviderError {
    ProviderError::new(status, why)
}

impl Program {
    /// Starts what `spec` names, or answers why it could not be: a missing program is `NOT_FOUND`, a terminal or a
    /// merged standard error `UNSUPPORTED`. `home` is the home it runs in. `ended` is set when the program exits or is
    /// killed.
    pub(super) fn start(
        spec: &SpawnSpec,
        ended: Arc<AtomicBool>,
        home: &Path,
    ) -> Result<Self, ProviderError> {
        if spec.terminal.is_some() {
            return Err(refuse(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "this spawner gives no terminal",
            ));
        }
        if spec.error_output == ErrorOutput::Merged {
            return Err(refuse(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "this spawner keeps standard error apart",
            ));
        }
        let args: alloc::vec::Vec<std::ffi::OsString> =
            spec.args.iter().map(std::ffi::OsString::from).collect();
        let (program, args) = match &spec.cwd {
            Some(cwd) => crate::batch::start_in(spec.program.as_ref(), &args, Path::new(cwd))
                .map_err(|error| refuse(Status::GUATIAO_ERR_UNSUPPORTED, &error.to_string()))?,
            None => (std::ffi::OsString::from(&spec.program), args),
        };
        let mut command = Command::new(program);
        command.args(args);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        if spec.clear_environment {
            command.env_clear();
        }
        for name in CONFIG_VARIABLES {
            command.env_remove(name);
        }
        for name in HOME_VARIABLES {
            command.env(name, home);
        }
        command.env(HOME_VARIABLE, home);
        for name in &spec.remove_environment {
            command.env_remove(name);
        }
        for (name, value) in spec.environment.expose().iter() {
            command.env(name, value);
        }
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(
            if spec.error_output == ErrorOutput::Discarded {
                Stdio::null()
            } else {
                Stdio::piped()
            },
        );
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().map_err(|error| {
            let status = if error.kind() == std::io::ErrorKind::NotFound {
                Status::GUATIAO_ERR_NOT_FOUND
            } else {
                Status::GUATIAO_ERR_INTERNAL
            };
            refuse(status, &error.kind().to_string())
        })?;
        #[cfg(windows)]
        let job = {
            use std::os::windows::io::AsRawHandle;
            let mut limits = win32job::ExtendedLimitInfo::new();
            limits.limit_kill_on_job_close();
            win32job::Job::create_with_limit_info(&limits)
                .and_then(|job| {
                    job.assign_process(child.as_raw_handle() as isize)
                        .map(|()| job)
                })
                .ok()
        };
        let out = Arc::new(Pipe::default());
        let err = Arc::new(Pipe::default());
        if let Some(stdout) = child.stdout.take() {
            out.fill(stdout);
        } else {
            out.lock().ended = true;
        }
        if let Some(stderr) = child.stderr.take() {
            err.fill(stderr);
        } else {
            err.lock().ended = true;
        }
        let input = child.stdin.take();
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                child,
                collected: None,
            }),
            ended,
            dropped: AtomicBool::new(false),
            #[cfg(windows)]
            job: Mutex::new(job),
        });
        // A program has ended when it has exited, whether or not anyone asks: the suite looks at that flag.
        let watching = Arc::clone(&shared);
        std::thread::spawn(move || {
            while !watching.dropped.load(Ordering::SeqCst) && watching.exited().is_none() {
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        Ok(Self {
            shared,
            input: Mutex::new(input),
            out,
            err,
        })
    }
}

impl SpawnedProcess for Program {
    fn write(&self, bytes: &[u8]) -> Result<(), ProviderError> {
        let mut input = self.input.lock().unwrap_or_else(PoisonError::into_inner);
        let stdin = input
            .as_mut()
            .ok_or_else(|| refuse(Status::GUATIAO_ERR_GONE, "input is closed"))?;
        stdin
            .write_all(bytes)
            .and_then(|()| stdin.flush())
            .map_err(|_| refuse(Status::GUATIAO_ERR_GONE, "the program has ended"))
    }

    fn close_input(&self) -> Result<(), ProviderError> {
        drop(
            self.input
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        Ok(())
    }

    fn read(&self, dst: &mut [u8], timeout_ms: u64) -> Result<i64, ProviderError> {
        Ok(self.out.read(dst, Duration::from_millis(timeout_ms)))
    }

    fn read_error(&self, dst: &mut [u8], timeout_ms: u64) -> Result<i64, ProviderError> {
        Ok(self.err.read(dst, Duration::from_millis(timeout_ms)))
    }

    fn resize(&self, _size: TerminalSize) -> Result<(), ProviderError> {
        Err(refuse(Status::GUATIAO_ERR_UNSUPPORTED, "no terminal"))
    }

    fn wait(&self, timeout_ms: u64) -> Result<ProcessExit, ProviderError> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if let Some(status) = self.shared.exited() {
                return Ok(ProcessExit::ended(
                    status.code().map(i64::from),
                    signal_of(&status),
                ));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(ProcessExit::running());
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(10)));
        }
    }

    fn kill(&self) -> Result<(), ProviderError> {
        self.shared.end_tree();
        Ok(())
    }

    fn link(&self) -> Result<LinkState, ProviderError> {
        Ok(LinkState::up())
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        self.shared.dropped.store(true, Ordering::SeqCst);
        self.shared.end_tree();
    }
}

#[cfg(unix)]
fn signal_of(status: &std::process::ExitStatus) -> Option<i64> {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map(i64::from)
}

#[cfg(not(unix))]
fn signal_of(_status: &std::process::ExitStatus) -> Option<i64> {
    None
}
