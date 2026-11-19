//! Starting a harness, and ending it with everything it started.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, TrySendError};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::Instant;

use serde_json::Value;

/// What to start: a program, its arguments, its working directory and its environment.
///
/// `Debug` names the environment's variables and never shows their values, which may be credentials.
#[derive(Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Launch {
    /// The program. A name with no directory is found on this process's `PATH` (and in the folders [`set_program_dirs`] names),
    /// not on a `PATH` this launch's `env` sets, so a folder the environment puts first never decides which program starts.
    pub program: OsString,
    /// Its arguments, each passed as one argument with no shell between.
    pub args: Vec<OsString>,
    /// Its working directory; the caller's when `None`.
    pub cwd: Option<PathBuf>,
    /// Variables set over the inherited environment, or over an empty one with `clear_env`.
    pub env: BTreeMap<OsString, OsString>,
    /// Variables removed from the inherited environment.
    pub remove_env: Vec<OsString>,
    /// Start from an empty environment rather than this process's.
    pub clear_env: bool,
}

impl Launch {
    /// A launch of `program` with no arguments, in the caller's directory and environment.
    #[must_use]
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    /// Adds one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Adds arguments.
    #[must_use]
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets the working directory.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Sets one variable.
    #[must_use]
    pub fn env(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.insert(name.into(), value.into());
        self
    }

    /// Starts from an empty environment.
    #[must_use]
    pub const fn clear_env(mut self, clear: bool) -> Self {
        self.clear_env = clear;
        self
    }

    /// The command line as one text, for messages: the program and each argument, quoted when it holds a space.
    #[must_use]
    pub fn display(&self) -> String {
        let mut text = quoted(&self.program);
        for arg in &self.args {
            text.push(' ');
            text.push_str(&quoted(arg));
        }
        text
    }
}

fn quoted(word: &OsStr) -> String {
    let word = word.to_string_lossy();
    if word.is_empty() || word.contains(' ') {
        alloc::format!("\"{word}\"")
    } else {
        word.into_owned()
    }
}

impl fmt::Debug for Launch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Launch")
            .field("program", &self.program)
            .field("args", &self.args.len())
            .field("cwd", &self.cwd)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("remove_env", &self.remove_env)
            .field("clear_env", &self.clear_env)
            .finish()
    }
}

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Exit {
    /// Its exit code, when it exited by itself.
    pub code: Option<i32>,
    /// The signal that ended it, on Unix.
    pub signal: Option<i32>,
}

impl Exit {
    /// How a process ended: by exiting with `code`, or by `signal`.
    #[must_use]
    pub const fn new(code: Option<i32>, signal: Option<i32>) -> Self {
        Self { code, signal }
    }
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.code, self.signal) {
            (Some(code), _) => write!(f, "exited with code {code}"),
            (None, Some(signal)) => write!(f, "was ended by signal {signal}"),
            (None, None) => f.write_str("exited"),
        }
    }
}

/// A started process, as far as the harness needs it after its pipes are taken.
pub trait Process: Send + Sync {
    /// Ends the process and everything it started. Ending one that has ended is no error.
    ///
    /// # Errors
    ///
    /// The operating system refused.
    fn kill(&self) -> io::Result<()>;

    /// How it ended, or `None` while it runs.
    ///
    /// # Errors
    ///
    /// The operating system could not tell.
    fn try_wait(&self) -> io::Result<Option<Exit>>;
}

/// How `process` ended, for a message, waiting up to `wait` for it to say: `exited with code 3`, `was ended by signal 9`,
/// or `closed its output` when it does not.
#[must_use]
pub fn exit_text(process: &dyn Process, wait: Duration) -> String {
    let until = Instant::now().checked_add(wait);
    loop {
        match process.try_wait() {
            Ok(Some(exit)) => return exit.to_string(),
            Ok(None) if until.is_none_or(|until| Instant::now() < until) => {
                thread::sleep(Duration::from_millis(20));
            }
            _ => return "closed its output".into(),
        }
    }
}

/// Writes one JSON value as one line to a harness's standard input, without waiting for it.
pub type LineWriter = Arc<dyn Fn(&Value) -> io::Result<()> + Send + Sync>;

/// A [`LineWriter`] over `stdin`: a thread of its own writes the lines in order, so a harness that has stopped reading
/// holds up nobody. Past `max_queued` lines waiting, or once a write has failed, a line is refused with an error.
///
/// # Errors
///
/// When the thread could not start.
pub fn line_writer(
    mut stdin: Box<dyn Write + Send>,
    max_queued: usize,
    name: &str,
) -> io::Result<LineWriter> {
    let (sender, lines) = mpsc::sync_channel::<Vec<u8>>(max_queued);
    let broken: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let breaking = Arc::clone(&broken);
    thread::Builder::new().name(name.into()).spawn(move || {
        for line in lines {
            if let Err(error) = stdin.write_all(&line).and_then(|()| stdin.flush()) {
                let _ = breaking.set(error.to_string());
                return;
            }
        }
    })?;
    Ok(Arc::new(move |value: &Value| {
        if let Some(why) = broken.get() {
            return Err(io::Error::other(why.clone()));
        }
        let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
        line.push(b'\n');
        sender.try_send(line).map_err(|error| match error {
            TrySendError::Full(_) => {
                io::Error::other(alloc::format!("{max_queued} lines are already waiting"))
            }
            TrySendError::Disconnected(_) => io::Error::other("its input has closed"),
        })
    }))
}

/// A flag for [`watch_exit`]: whoever sets it first ends the session.
#[must_use]
pub fn ending_once() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Watches `process` on a thread of its own until `done` is set, asking every `poll` whether it has exited. An exit with
/// `done` still unset is a process whose output something else holds open: what it wrote last gets `drain` to be read,
/// what it left behind is ended and gets `drain` too, and then `end` runs, if `done` still was not set. A process that
/// cannot say whether it has exited is not watched.
///
/// # Errors
///
/// When the thread could not start.
pub fn watch_exit(
    process: Arc<dyn Process>,
    poll: Duration,
    drain: Duration,
    done: Arc<AtomicBool>,
    end: impl FnOnce() + Send + 'static,
) -> io::Result<()> {
    let wait = move |done: &AtomicBool, wait: Duration| {
        let until = Instant::now().checked_add(wait);
        while !done.load(Ordering::SeqCst) && until.is_none_or(|until| Instant::now() < until) {
            thread::sleep(Duration::from_millis(10));
        }
    };
    thread::Builder::new()
        .name("coqui-acp-harness-exit".into())
        .spawn(move || {
            loop {
                if done.load(Ordering::SeqCst) {
                    return;
                }
                match process.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => thread::sleep(poll.max(Duration::from_millis(1))),
                    Err(_) => return,
                }
            }
            wait(&done, drain);
            if done.load(Ordering::SeqCst) {
                return;
            }
            let _ = process.kill();
            wait(&done, drain);
            if !done.swap(true, Ordering::SeqCst) {
                end();
            }
        })?;
    Ok(())
}

/// A started process and its pipes. Standard error is `None` when the spawner keeps it.
#[non_exhaustive]
pub struct Spawned {
    /// Its standard input.
    pub stdin: Box<dyn Write + Send>,
    /// Its standard output.
    pub stdout: Box<dyn Read + Send>,
    /// Its standard error.
    pub stderr: Option<Box<dyn Read + Send>>,
    /// The process itself.
    pub process: Box<dyn Process>,
}

impl Spawned {
    /// A spawned process from its parts.
    #[must_use]
    pub fn new(
        stdin: Box<dyn Write + Send>,
        stdout: Box<dyn Read + Send>,
        stderr: Option<Box<dyn Read + Send>>,
        process: Box<dyn Process>,
    ) -> Self {
        Self {
            stdin,
            stdout,
            stderr,
            process,
        }
    }
}

impl fmt::Debug for Spawned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Spawned").finish_non_exhaustive()
    }
}

/// Starts processes. Implement it to run a harness in a container, over SSH or under a terminal; [`PlainSpawn`] runs
/// it here.
pub trait Spawn: Send + Sync {
    /// Starts `launch` with its standard input and output piped.
    ///
    /// # Errors
    ///
    /// The process could not be started.
    fn spawn(&self, launch: &Launch) -> io::Result<Spawned>;
}

/// Starts a plain process whose [`Process::kill`] ends the whole tree it started. Standard error is piped.
///
/// On Windows the process and everything it starts are in a job object of its own, which ends them all when it closes:
/// at `kill`, and when the host itself ends without one. A process the harness starts in the instant before it joins
/// its job is outside it.
///
/// On Unix the process leads a process group, which `kill` sends `SIGKILL`; on Linux and Android `kill` first stops
/// and then ends every process descended from it too, those that left the group or session included. A watcher in
/// the group, a `/bin/sh` reading a pipe only the host holds, ends the group when the host ends without a kill; a
/// process that left the group escapes that. Dropping the process kills it as `kill` does.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlainSpawn;

/// What starts every process this crate starts itself ([`PlainSpawn`], and `LocalSpawner` with what it
/// serves) in place of this process's own children, once [`set_starter`] has named one: for a host that cannot wait on its own
/// children, as one inside an application whose process handling collects every child that ends cannot. It is told where the
/// standard error goes, and starts the process as [`PlainSpawn`] describes, in a group of its own where the system has one,
/// and returns a [`Process`] whose `kill` ends that group and collects the process, and whose `try_wait` says how it ended
/// without collecting it.
pub trait Starter: Send + Sync {
    /// Starts `launch` with its standard input and output piped, and its standard error as `errors` says.
    ///
    /// # Errors
    ///
    /// The process could not be started.
    fn start(&self, launch: &Launch, errors: ErrorStream) -> io::Result<Spawned>;
}

static STARTER: Mutex<Option<Arc<dyn Starter>>> = Mutex::new(None);

/// Has every process this crate starts from now on started by `starter`.
pub fn set_starter(starter: Arc<dyn Starter>) {
    *STARTER.lock().unwrap_or_else(PoisonError::into_inner) = Some(starter);
}

type ProgramDirs = fn() -> Vec<PathBuf>;

static PROGRAM_DIRS: Mutex<Option<ProgramDirs>> = Mutex::new(None);

/// Names a function whose folders are searched, after this process's own `PATH`, for the program of a launch that names no
/// directory and whose environment sets `PATH`: a host that keeps programs of its own (a runtime it installed) for its
/// children. It is asked at each start.
pub fn set_program_dirs(dirs: fn() -> Vec<PathBuf>) {
    *PROGRAM_DIRS.lock().unwrap_or_else(PoisonError::into_inner) = Some(dirs);
}

/// `program` where this process finds it when `env` sets `PATH`, else as given. The system looks a bare name up on the child's
/// own `PATH` when the environment it is started with sets one, which would let a folder put first by a profile decide which
/// program an agent is; a name with a directory, and one this process cannot find, are left to the system.
fn found_on_own_path(program: &OsStr, env: &BTreeMap<OsString, OsString>) -> OsString {
    if !env.keys().any(|name| name.eq_ignore_ascii_case("PATH"))
        || program.to_string_lossy().contains(['/', '\\'])
    {
        return program.to_owned();
    }
    let extra = PROGRAM_DIRS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .map_or_else(Vec::new, |dirs| dirs());
    let own = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var("PATHEXT").unwrap_or_default();
    find_in(&own, &extra, &pathext, &program.to_string_lossy())
        .map_or_else(|| program.to_owned(), PathBuf::into_os_string)
}

/// The first runnable file called `name` in an absolute folder of `path`, then of `extra`. On Windows a name takes each of
/// `pathext`'s extensions (the usual four when it is empty) unless it has one.
fn find_in(path: &OsStr, extra: &[PathBuf], pathext: &str, name: &str) -> Option<PathBuf> {
    let suffixes: Vec<String> = if cfg!(windows) {
        let listed: Vec<String> = pathext
            .split(';')
            .filter(|ext| ext.starts_with('.') && ext.len() > 1)
            .map(str::to_ascii_lowercase)
            .collect();
        let listed = if listed.is_empty() {
            [".com", ".exe", ".bat", ".cmd"].map(String::from).to_vec()
        } else {
            listed
        };
        if listed
            .iter()
            .any(|ext| name.to_ascii_lowercase().ends_with(ext))
        {
            vec![String::new()]
        } else {
            listed
        }
    } else {
        vec![String::new()]
    };
    std::env::split_paths(path)
        .chain(extra.iter().cloned())
        .filter(|folder| folder.is_absolute())
        .flat_map(|folder| {
            suffixes
                .iter()
                .map(move |suffix| folder.join(format!("{name}{suffix}")))
        })
        .find(|candidate| runnable(candidate))
}

fn runnable(file: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(file) else {
        return false;
    };
    #[cfg(unix)]
    {
        std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 != 0 && meta.is_file()
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// Runs `start`, again after a short wait while it fails with "text file busy" (Unix `ETXTBSY`): a program just written is
/// refused by `exec` while another thread's `fork` still holds a copy of its write descriptor, which it drops at its own `exec`.
/// Any other failure, and the sixth refusal, are the answer.
pub(crate) fn retry_busy<T>(
    #[cfg_attr(not(unix), allow(unused_mut))] mut start: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    #[cfg(unix)]
    for wait in [50, 100, 200, 300, 350] {
        match start() {
            Err(error) if error.raw_os_error() == Some(26) => {
                thread::sleep(Duration::from_millis(wait));
            }
            other => return other,
        }
    }
    start()
}

/// Where a started process's standard error goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorStream {
    /// A pipe of its own.
    Piped,
    /// The same pipe as standard output.
    IntoOutput,
    /// Nowhere.
    Discarded,
}

impl Spawn for PlainSpawn {
    fn spawn(&self, launch: &Launch) -> io::Result<Spawned> {
        start(launch, ErrorStream::Piped)
    }
}

/// Starts `launch` as [`PlainSpawn`] does, standard error going where `errors` says.
pub(crate) fn start(launch: &Launch, errors: ErrorStream) -> io::Result<Spawned> {
    let starter = STARTER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(starter) = starter {
        return starter.start(launch, errors);
    }
    let found = found_on_own_path(&launch.program, &launch.env);
    // A batch file does not start in a folder on a share: what it names starts there instead.
    let (program, args) = match &launch.cwd {
        Some(cwd) => coqui_acp_api::batch::start_in(&found, &launch.args, cwd)?,
        None => (found, launch.args.clone()),
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut merged = None;
    match errors {
        ErrorStream::Piped => {
            command.stderr(Stdio::piped());
        }
        ErrorStream::Discarded => {
            command.stderr(Stdio::null());
        }
        ErrorStream::IntoOutput => {
            let (reader, writer) = io::pipe()?;
            command.stdout(writer.try_clone()?).stderr(writer);
            merged = Some(reader);
        }
    }
    if launch.clear_env {
        command.env_clear();
    }
    for name in &launch.remove_env {
        command.env_remove(name);
    }
    command.envs(&launch.env);
    if let Some(cwd) = &launch.cwd {
        command.current_dir(cwd);
    }
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = retry_busy(|| command.spawn())?;
    // The pipe's write ends are the child's now: the reader sees the end once it and its tree close theirs.
    drop(command);
    let abandon = |child: &mut Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    #[cfg(windows)]
    let job = match contain(&child) {
        Ok(job) => job,
        Err(error) => {
            abandon(&mut child);
            return Err(error);
        }
    };
    #[cfg(unix)]
    let watcher = match watch_group(&child) {
        Ok(watcher) => watcher,
        Err(error) => {
            abandon(&mut child);
            return Err(error);
        }
    };
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let stdout: Option<Box<dyn Read + Send>> = match merged {
        Some(reader) => Some(Box::new(reader)),
        None => stdout.map(|out| Box::new(out) as Box<dyn Read + Send>),
    };
    let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
        abandon(&mut child);
        return Err(io::Error::other("the process's pipes were not opened"));
    };
    Ok(Spawned {
        stdin: Box::new(stdin),
        stdout,
        stderr: stderr.map(|stderr| Box::new(stderr) as Box<dyn Read + Send>),
        process: Box::new(PlainProcess {
            child: Mutex::new(Running {
                child,
                collected: None,
                #[cfg(unix)]
                watcher: Some(watcher),
            }),
            #[cfg(windows)]
            job: Mutex::new(Some(job)),
        }),
    })
}

struct PlainProcess {
    child: Mutex<Running>,
    /// Holds the process and everything it starts; closing it ends them all.
    #[cfg(windows)]
    job: Mutex<Option<win32job::Job>>,
}

struct Running {
    child: Child,
    /// How it ended, once it has been collected: from then on its id, and its group's, may be another process's, so
    /// nothing is sent to them again.
    collected: Option<Exit>,
    /// The group's watcher and the pipe that keeps it waiting, until the group is ended.
    #[cfg(unix)]
    watcher: Option<(Child, std::process::ChildStdin)>,
}

fn exit_of(status: std::process::ExitStatus) -> Exit {
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal = None;
    Exit {
        code: status.code(),
        signal,
    }
}

impl Process for PlainProcess {
    fn kill(&self) -> io::Result<()> {
        let mut running = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        if running.collected.is_some() {
            return Ok(());
        }
        #[cfg(windows)]
        drop(
            self.job
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        #[cfg(unix)]
        if let Some(group) = i32::try_from(running.child.id())
            .ok()
            .filter(|pid| *pid > 1)
            .and_then(rustix::process::Pid::from_raw)
        {
            use rustix::process::{Signal, kill_process, kill_process_group};
            // The process is not collected before this, so its group id is still its own, and its descendants
            // still name it, or one another, as their parent.
            let escaped = descendants::stopped(group);
            let _ = kill_process_group(group, Signal::KILL);
            for pid in escaped {
                let _ = kill_process(pid, Signal::KILL);
            }
        }
        match running.child.kill() {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {}
            Err(error) => return Err(error),
        }
        let status = running.child.wait()?;
        running.collected = Some(exit_of(status));
        #[cfg(unix)]
        if let Some((mut watcher, pipe)) = running.watcher.take() {
            drop(pipe);
            let _ = watcher.kill();
            let _ = watcher.wait();
        }
        Ok(())
    }

    fn try_wait(&self) -> io::Result<Option<Exit>> {
        let mut running = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(exit) = running.collected {
            return Ok(Some(exit));
        }
        #[cfg(unix)]
        {
            let _ = &mut running;
            peek(running.child.id())
        }
        #[cfg(not(unix))]
        Ok(running.child.try_wait()?.map(exit_of))
    }
}

impl Drop for PlainProcess {
    /// A process nobody kept ends with everything it started, as on Windows, where closing its job does it.
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

/// How the process ended, leaving it uncollected: until it is collected its id, and its group's, stay its own.
#[cfg(unix)]
fn peek(pid: u32) -> io::Result<Option<Exit>> {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::other("not a process id"))?;
    let status = waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    )?;
    Ok(status.map(|status| Exit {
        code: status.exit_status(),
        signal: status.terminating_signal(),
    }))
}

/// Starts a watcher in `child`'s process group: a shell that reads a pipe only this process holds, and ends the group
/// when the pipe ends, which it does when this process ends without a kill. A kill ends the watcher with the group.
#[cfg(unix)]
fn watch_group(child: &Child) -> io::Result<(Child, std::process::ChildStdin)> {
    let group = child.id().to_string();
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            r#"while read -r _; do :; done; kill -s KILL -- "-$1""#,
            "watcher",
        ])
        .arg(&group)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(
        &mut command,
        i32::try_from(child.id()).map_err(io::Error::other)?,
    );
    let mut watcher = command.spawn()?;
    let pipe = watcher
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("the watcher's pipe was not opened"))?;
    Ok((watcher, pipe))
}

/// The processes descended from one, found through `/proc` by their parents.
#[cfg(unix)]
mod descendants {
    use alloc::vec::Vec;

    use rustix::process::Pid;

    /// Every process descended from `root`, each stopped so it starts no more, found again until no new one
    /// appears. Empty where there is no `/proc` to read.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub(super) fn stopped(root: Pid) -> Vec<Pid> {
        use rustix::process::{Signal, kill_process};
        let mut found = alloc::collections::BTreeSet::new();
        for _ in 0..16 {
            let parents = parents();
            let mut grown = false;
            let mut queue = Vec::from([root.as_raw_nonzero().get()]);
            while let Some(parent) = queue.pop() {
                for (child, _) in parents.iter().filter(|(_, p)| **p == parent) {
                    if found.insert(*child) {
                        grown = true;
                        if let Some(pid) = Pid::from_raw(*child) {
                            let _ = kill_process(pid, Signal::STOP);
                        }
                    }
                    queue.push(*child);
                }
            }
            if !grown {
                break;
            }
        }
        found.into_iter().filter_map(Pid::from_raw).collect()
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub(super) fn stopped(_root: Pid) -> Vec<Pid> {
        Vec::new()
    }

    /// Each process's parent, as `/proc/<pid>/stat` says.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn parents() -> alloc::collections::BTreeMap<i32, i32> {
        let mut parents = alloc::collections::BTreeMap::new();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return parents;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<i32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            // `pid (name) state ppid ...`: the name may hold spaces and parentheses, so read from its last `)`.
            let parent = stat
                .rfind(')')
                .and_then(|end| stat[end + 1..].split_whitespace().nth(1))
                .and_then(|ppid| ppid.parse::<i32>().ok());
            if let Some(parent) = parent {
                parents.insert(pid, parent);
            }
        }
        parents
    }
}

/// Puts a process just started in a job object of its own that ends everything in it when its last handle closes.
#[cfg(windows)]
fn contain(child: &Child) -> io::Result<win32job::Job> {
    use std::os::windows::io::AsRawHandle;
    let mut limits = win32job::ExtendedLimitInfo::new();
    limits.limit_kill_on_job_close();
    let job = win32job::Job::create_with_limit_info(&limits).map_err(io::Error::other)?;
    job.assign_process(child.as_raw_handle() as isize)
        .map_err(io::Error::other)?;
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn a_start_refused_as_busy_is_tried_again_and_any_other_error_is_not() {
        let mut calls = 0;
        let started = retry_busy(|| {
            calls += 1;
            if calls < 3 {
                Err(io::Error::from_raw_os_error(26))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(started.expect("started on the third try"), 3);
        let mut calls = 0;
        let refused = retry_busy(|| -> io::Result<()> {
            calls += 1;
            Err(io::Error::from_raw_os_error(2))
        });
        assert_eq!((calls, refused.is_err()), (1, true));
        let mut calls = 0;
        let busy = retry_busy(|| -> io::Result<()> {
            calls += 1;
            Err(io::Error::from_raw_os_error(26))
        });
        assert_eq!((calls, busy.is_err()), (6, true));
    }

    #[test]
    fn a_program_is_found_in_an_absolute_folder_of_the_path_then_in_the_extra_ones() {
        let root = std::env::temp_dir().join(format!("acp-find-{}", std::process::id()));
        let (first, second) = (root.join("first"), root.join("second"));
        for folder in [&first, &second] {
            std::fs::create_dir_all(folder).expect("a folder");
        }
        let file = if cfg!(windows) { "tool.exe" } else { "tool" };
        for folder in [&first, &second] {
            let path = folder.join(file);
            std::fs::write(&path, "").expect("a file");
            #[cfg(unix)]
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .expect("executable");
        }
        let path = std::env::join_paths([&first]).expect("a path");
        assert_eq!(
            find_in(&path, core::slice::from_ref(&second), ".EXE", "tool"),
            Some(first.join(file))
        );
        let none = std::env::join_paths([root.join("none")]).expect("a path");
        assert_eq!(
            find_in(&none, core::slice::from_ref(&second), ".EXE", "tool"),
            Some(second.join(file))
        );
        assert_eq!(find_in(&none, &[], ".EXE", "tool"), None);
        // A folder that is not absolute is not searched, and a file that cannot run is not found.
        assert_eq!(find_in(OsStr::new("first"), &[], "", "tool"), None);
        #[cfg(unix)]
        {
            std::fs::set_permissions(
                first.join(file),
                std::os::unix::fs::PermissionsExt::from_mode(0o644),
            )
            .expect("not executable");
            assert_eq!(find_in(&path, &[], "", "tool"), None);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
