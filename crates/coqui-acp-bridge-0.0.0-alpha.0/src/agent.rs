//! The agent process: lines out through a bounded queue, lines in through a queue to a thread of their own, since a write to an
//! agent that has stopped reading never returns.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Lines waiting for the network. A full queue stops the reader, so the agent feels back-pressure on its pipe.
const QUEUED_LINES: usize = 256;
/// Bytes waiting for the network, past which the reader stops as it does for a full queue. One line may always wait.
const QUEUED_BYTES: usize = 64 * 1024 * 1024;
/// Lines waiting for the agent. A full queue stops the relay reading, so the peer feels back-pressure.
const LINES_IN: usize = 8;
/// How often a wait looks up.
const POLL: Duration = Duration::from_millis(10);

/// A line the agent wrote, or why its output ended.
#[derive(Debug)]
pub(crate) enum Output {
    Line(Vec<u8>),
    TooLong,
}

/// Why a line did not go to the agent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WriteEnd {
    Ended,
    Stuck,
    Stopped,
}

/// A process an [`Agent`] runs, however it was started. A [`Spawner`] keeps this true: a process that has ended stays
/// uncollected, and its id and its group's stay its own, until [`Process::wait`] or [`Process::try_wait`] collects it.
pub trait Process: Send + fmt::Debug {
    /// Its id, which on Unix is also its process group's.
    fn id(&self) -> u32;

    /// Whether it has ended and how, leaving it uncollected.
    ///
    /// # Errors
    ///
    /// The operating system could not say.
    fn peek(&mut self) -> io::Result<Option<ExitStatus>>;

    /// Ends the process and what it started: its process group on Unix, its job object on Windows. A process already
    /// collected is left alone, since its group's id may be anyone's.
    fn end_group(&mut self);

    /// Whether it has ended and how, collecting it if so.
    ///
    /// # Errors
    ///
    /// The operating system could not say.
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;

    /// Ends the process itself.
    ///
    /// # Errors
    ///
    /// The operating system could not end it.
    fn kill(&mut self) -> io::Result<()>;

    /// Waits for it to end and collects it.
    ///
    /// # Errors
    ///
    /// The operating system could not wait.
    fn wait(&mut self) -> io::Result<ExitStatus>;
}

/// An agent's process and its two standard streams, as a [`Spawner`] starts them.
pub struct Spawned {
    /// The process.
    pub process: Box<dyn Process>,
    /// Its standard input: lines written here reach the agent.
    pub input: Box<dyn Write + Send>,
    /// Its standard output.
    pub output: Box<dyn Read + Send>,
}

impl fmt::Debug for Spawned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Spawned")
            .field("process", &self.process)
            .finish_non_exhaustive()
    }
}

/// What starts an agent's process, for a host whose own children are not its own to wait on: one running inside an
/// application whose process handling collects every child that ends. [`Local`] starts it as this process's own child.
pub trait Spawner: Send + Sync {
    /// Starts `program args...` in a process group of its own, so that [`Process::end_group`] reaches what it starts, with
    /// its standard input and output piped to the streams returned and its standard error this process's.
    ///
    /// # Errors
    ///
    /// The process could not be started.
    fn spawn(&self, program: &OsStr, args: &[OsString]) -> io::Result<Spawned>;
}

/// The spawner [`Agent::spawn`] uses: this process's own children, on Windows each in a job object of its own.
#[derive(Clone, Copy, Debug, Default)]
pub struct Local;

/// A child of this process.
#[derive(Debug)]
struct Own {
    child: Child,
    /// Holds the agent and everything it starts; the system ends them all when the last handle closes, this process
    /// ending included.
    #[cfg(windows)]
    job: Option<win32job::Job>,
}

impl Spawner for Local {
    fn spawn(&self, program: &OsStr, args: &[OsString]) -> io::Result<Spawned> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // Its own group, so `stop` can reach what it starts: an adapter is usually a launcher around the program that does the work.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn()?;
        #[cfg(windows)]
        let job = match contain(&child) {
            Ok(job) => Some(job),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no standard input"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no standard output"))?;
        Ok(Spawned {
            process: Box::new(Own {
                child,
                #[cfg(windows)]
                job,
            }),
            input: Box::new(input),
            output: Box::new(output),
        })
    }
}

impl Process for Own {
    fn id(&self) -> u32 {
        self.child.id()
    }

    #[cfg(unix)]
    fn peek(&mut self) -> io::Result<Option<ExitStatus>> {
        peek(self.child.id())
    }

    #[cfg(windows)]
    fn peek(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn end_group(&mut self) {
        // Before it is collected: until then its id and its group's cannot be anyone else's.
        #[cfg(unix)]
        end_tree(self.child.id());
        #[cfg(windows)]
        drop(self.job.take());
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn kill(&mut self) -> io::Result<()> {
        self.child.kill()
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }
}

/// A running agent. Dropping it ends the process and what it started.
#[derive(Debug)]
pub struct Agent {
    process: Mutex<Box<dyn Process>>,
    input: SyncSender<Vec<u8>>,
    output: Mutex<Receiver<Output>>,
    queued: Arc<AtomicUsize>,
    /// Set once nothing more can come from the agent's output: whatever holds it, the agent or what it started, has closed it.
    output_closed: Arc<AtomicBool>,
    write_limit: Duration,
    stopped: AtomicBool,
    guard: Mutex<Option<Guard>>,
}

/// A helper process waiting to end this agent if this process goes first.
#[derive(Debug)]
struct Guard {
    process: Child,
    told: ChildStdin,
}

/// What a helper started by [`Agent::guarded`] does: waits on `from_bridge`, its standard input, and ends the process group of `agent`
/// unless a byte arrives first. A pipe that ends with no byte means the bridge went, however it went. An id of 0 or 1 names no agent
/// (as a group they mean this group, or every process), so nothing is done for one.
pub fn watch(agent: u32, mut from_bridge: impl Read) {
    if from_bridge.read_exact(&mut [0_u8]).is_err() {
        end_tree(agent);
    }
}

/// Ends the process group `pid` leads. A failure here only means less was ended than asked: `stop` still ends the child itself.
#[cfg(unix)]
fn end_tree(pid: u32) {
    if let Some(group) = group_of(pid) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
}

/// The group `pid` leads, for an id that can name one alone: not 0 or 1, and within what the system counts.
#[cfg(unix)]
fn group_of(pid: u32) -> Option<rustix::process::Pid> {
    i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 1)
        .and_then(rustix::process::Pid::from_raw)
}

/// Ends a process and what it started, by the system's own `taskkill`. The bridge ends its agents by their job objects; this is only for
/// [`watch`], which has a process id.
#[cfg(windows)]
fn end_tree(pid: u32) {
    if pid <= 1 {
        return;
    }
    let system = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
    let taskkill = std::path::Path::new(&system)
        .join("System32")
        .join("taskkill.exe");
    let _ = Command::new(taskkill)
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .output();
}

/// Whether the process has ended and how, leaving it uncollected: until it is collected its id, and its group's, stay its own.
#[cfg(unix)]
fn peek(pid: u32) -> io::Result<Option<ExitStatus>> {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    use std::os::unix::process::ExitStatusExt;
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::other("not a process id"))?;
    let status = waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    )?;
    // A wait status: the exit code in the second byte, or the signal in the low seven bits.
    Ok(status.map(|status| match status.exit_status() {
        Some(code) => ExitStatus::from_raw((code & 0xff) << 8),
        None => ExitStatus::from_raw(status.terminating_signal().unwrap_or(0) & 0x7f),
    }))
}

impl Agent {
    /// Starts `program` as a child of this process, by [`Local`]. Its standard error stays on this process's, where a person
    /// can read it.
    ///
    /// # Errors
    ///
    /// The operating system could not start it, or on Windows could not put it in a job object of its own.
    pub fn spawn(program: &OsString, args: &[OsString], max_line: usize) -> io::Result<Self> {
        Self::spawn_with(&Local, program, args, max_line)
    }

    /// Starts `program` with `spawner`, for a host whose children are not its own to wait on.
    ///
    /// # Errors
    ///
    /// `spawner` could not start it.
    pub fn spawn_with(
        spawner: &dyn Spawner,
        program: &OsStr,
        args: &[OsString],
        max_line: usize,
    ) -> io::Result<Self> {
        let Spawned {
            process,
            input: mut stdin,
            output: stdout,
        } = spawner.spawn(program, args)?;
        let (input, lines_in) = mpsc::sync_channel::<Vec<u8>>(LINES_IN);
        thread::Builder::new()
            .name("coqui-acp-bridge-agent-input".to_owned())
            .spawn(move || {
                while let Ok(line) = lines_in.recv() {
                    if stdin.write_all(&line).and_then(|()| stdin.flush()).is_err() {
                        return;
                    }
                }
            })?;
        let (lines, output) = mpsc::sync_channel(QUEUED_LINES);
        let queued = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&queued);
        let output_closed = Arc::new(AtomicBool::new(false));
        let closed = Arc::clone(&output_closed);
        thread::Builder::new()
            .name("coqui-acp-bridge-agent".to_owned())
            .spawn(move || {
                read_output(stdout, max_line, &lines, &counted);
                closed.store(true, Ordering::SeqCst);
            })?;
        Ok(Self {
            process: Mutex::new(process),
            input,
            output: Mutex::new(output),
            queued,
            output_closed,
            write_limit: Duration::from_secs(30),
            stopped: AtomicBool::new(false),
            guard: Mutex::new(None),
        })
    }

    /// On Unix, starts `program args... PID`, which has to call [`watch`] with this agent's id and its standard input, so that what is
    /// left of this agent's process group is ended if this process goes, whatever ends it and whether or not the agent reads its input.
    /// It is apart from a terminal's Ctrl-C, which has to leave it standing to do this. It is cleanup at best: a process that leaves the
    /// group escapes it, and anything that can signal it can end it first.
    ///
    /// On Windows it starts nothing: every agent is in a job object, which the system ends with this process.
    ///
    /// # Errors
    ///
    /// The operating system could not start it, and this agent has been ended.
    pub fn guarded(self, program: &OsStr, args: &[&str]) -> io::Result<Self> {
        #[cfg(windows)]
        let _ = (program, args);
        #[cfg(unix)]
        {
            let pid = self.pid();
            let mut command = Command::new(program);
            command
                .args(args)
                .arg(pid.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            std::os::unix::process::CommandExt::process_group(&mut command, 0);
            let mut process = command.spawn()?;
            let told = process
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("no standard input"))?;
            *self.guard.lock().unwrap_or_else(PoisonError::into_inner) =
                Some(Guard { process, told });
        }
        Ok(self)
    }

    /// How long a line may wait on an agent that has stopped reading, after which the link ends as
    /// [`LinkEnd::AgentStuck`](crate::relay::LinkEnd::AgentStuck). Default 30 seconds.
    #[must_use]
    pub const fn with_write_limit(mut self, limit: Duration) -> Self {
        self.write_limit = limit;
        self
    }

    pub(crate) fn write_line(&self, message: &[u8], stop: &AtomicBool) -> Result<(), WriteEnd> {
        let mut line = Vec::with_capacity(message.len().saturating_add(1));
        line.extend_from_slice(message);
        line.push(b'\n');
        let until = Instant::now() + self.write_limit;
        loop {
            match self.input.try_send(line) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => return Err(WriteEnd::Ended),
                Err(TrySendError::Full(back)) => {
                    line = back;
                    if stop.load(Ordering::SeqCst) {
                        return Err(WriteEnd::Stopped);
                    }
                    if Instant::now() >= until {
                        return Err(WriteEnd::Stuck);
                    }
                    thread::sleep(POLL);
                }
            }
        }
    }

    pub(crate) fn next_output(&self, wait: Duration) -> Result<Output, RecvTimeoutError> {
        let output = self.output.lock().unwrap_or_else(PoisonError::into_inner);
        let next = output.recv_timeout(wait)?;
        if let Output::Line(line) = &next {
            self.queued.fetch_sub(line.len(), Ordering::SeqCst);
        }
        Ok(next)
    }

    /// Whether the agent can still say anything: its output is open, whether its own process holds it or one it started does, as when
    /// the program run is a launcher that has exited. Lines it wrote before may still wait in the queue.
    pub(crate) fn output_open(&self) -> bool {
        !self.output_closed.load(Ordering::SeqCst)
    }

    /// The process's id.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.process
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .id()
    }

    /// The id of the helper [`Self::guarded`] started, until it has been dismissed. Always `None` on Windows.
    #[must_use]
    pub fn guard_pid(&self) -> Option<u32> {
        self.guard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|guard| guard.process.id())
    }

    /// Whether the process has ended, and how. It is not collected until [`Self::stop`], so its id stays its own until then.
    ///
    /// # Errors
    ///
    /// The operating system could not say.
    pub fn try_status(&self) -> io::Result<Option<ExitStatus>> {
        let mut process = self.process.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.stopped.load(Ordering::SeqCst) {
            return process.peek();
        }
        process.try_wait()
    }

    /// Ends the process and what it started, and collects its status: by process group on Unix, by job object on Windows.
    ///
    /// # Errors
    ///
    /// The operating system could not end it.
    pub fn stop(&self) -> io::Result<ExitStatus> {
        let mut process = self.process.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.stopped.swap(true, Ordering::SeqCst) {
            // Before collecting it: until then its id and its group's cannot be anyone else's.
            process.end_group();
        }
        // Dismissed before the agent is collected, after which a helper ending that group could reach someone else's.
        if let Some(mut guard) = self
            .guard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = guard.told.write_all(b"x");
            drop(guard.told);
            let _ = guard.process.wait();
        }
        if let Some(status) = process.try_wait()? {
            return Ok(status);
        }
        process.kill()?;
        process.wait()
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Puts a process just started in a job object of its own that ends everything in it when its last handle closes. A process it
/// starts in the moment before this is not in the job.
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

/// Reads the agent's output into `lines` until it ends, holding back while the queue is full.
fn read_output(
    stdout: impl Read,
    max_line: usize,
    lines: &SyncSender<Output>,
    counted: &Arc<AtomicUsize>,
) {
    let limit = u64::try_from(max_line)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = Vec::new();
        match (&mut reader).take(limit).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) if line.len() > max_line && line.last() != Some(&b'\n') => {
                let _ = lines.send(Output::TooLong);
                return;
            }
            Ok(_) => {
                while matches!(line.last(), Some(b'\n' | b'\r')) {
                    line.pop();
                }
                if line.is_empty() {
                    continue;
                }
                // The count has one other holder for as long as anyone can take a line.
                while counted.load(Ordering::SeqCst) > 0
                    && counted.load(Ordering::SeqCst).saturating_add(line.len()) > QUEUED_BYTES
                {
                    if Arc::strong_count(counted) == 1 {
                        return;
                    }
                    thread::sleep(POLL);
                }
                counted.fetch_add(line.len(), Ordering::SeqCst);
                if lines.send(Output::Line(line)).is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn an_id_that_would_name_this_group_or_every_process_names_no_group() {
        assert!(group_of(0).is_none());
        assert!(group_of(1).is_none());
        assert!(group_of(u32::MAX).is_none());
        assert_eq!(
            group_of(2).map(rustix::process::Pid::as_raw_nonzero),
            core::num::NonZeroI32::new(2)
        );
    }
}

#[cfg(test)]
mod seam {
    use super::*;
    use std::io::Cursor;

    type Calls = Arc<Mutex<Vec<&'static str>>>;

    fn status(code: u8) -> ExitStatus {
        #[cfg(unix)]
        {
            std::os::unix::process::ExitStatusExt::from_raw(i32::from(code) << 8)
        }
        #[cfg(windows)]
        {
            std::os::windows::process::ExitStatusExt::from_raw(u32::from(code))
        }
    }

    /// A process that ends once killed, saying what it was asked.
    #[derive(Debug)]
    struct Fake {
        calls: Calls,
        killed: bool,
    }

    impl Process for Fake {
        fn id(&self) -> u32 {
            4242
        }

        fn peek(&mut self) -> io::Result<Option<ExitStatus>> {
            self.calls.lock().unwrap().push("peek");
            Ok(self.killed.then(|| status(9)))
        }

        fn end_group(&mut self) {
            self.calls.lock().unwrap().push("end_group");
            self.killed = true;
        }

        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.calls.lock().unwrap().push("try_wait");
            Ok(self.killed.then(|| status(9)))
        }

        fn kill(&mut self) -> io::Result<()> {
            self.calls.lock().unwrap().push("kill");
            self.killed = true;
            Ok(())
        }

        fn wait(&mut self) -> io::Result<ExitStatus> {
            self.calls.lock().unwrap().push("wait");
            Ok(status(9))
        }
    }

    #[derive(Clone, Default)]
    struct Written(Arc<Mutex<Vec<u8>>>);

    impl Write for Written {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Scripted {
        calls: Calls,
        written: Written,
        said: &'static [u8],
        started: Mutex<Vec<(OsString, Vec<OsString>)>>,
    }

    impl Spawner for Scripted {
        fn spawn(&self, program: &OsStr, args: &[OsString]) -> io::Result<Spawned> {
            self.started
                .lock()
                .unwrap()
                .push((program.to_owned(), args.to_vec()));
            Ok(Spawned {
                process: Box::new(Fake {
                    calls: Arc::clone(&self.calls),
                    killed: false,
                }),
                input: Box::new(self.written.clone()),
                output: Box::new(Cursor::new(self.said.to_vec())),
            })
        }
    }

    fn scripted() -> Scripted {
        Scripted {
            calls: Calls::default(),
            written: Written::default(),
            said: b"hello\n",
            started: Mutex::default(),
        }
    }

    #[test]
    fn an_agent_runs_on_the_process_and_streams_its_spawner_gave() {
        let spawner = scripted();
        let args = [OsString::from("--acp")];
        let agent = Agent::spawn_with(&spawner, OsStr::new("adapter"), &args, 1024).unwrap();
        assert_eq!(
            spawner.started.lock().unwrap().as_slice(),
            [(OsString::from("adapter"), args.to_vec())]
        );
        assert_eq!(agent.pid(), 4242);
        match agent.next_output(Duration::from_secs(5)).unwrap() {
            Output::Line(line) => assert_eq!(line, b"hello"),
            Output::TooLong => panic!("a short line"),
        }
        agent
            .write_line(b"{}", &AtomicBool::new(false))
            .expect("the line is queued");
        let until = Instant::now() + Duration::from_secs(5);
        while spawner.written.0.lock().unwrap().is_empty() && Instant::now() < until {
            thread::sleep(POLL);
        }
        assert_eq!(spawner.written.0.lock().unwrap().as_slice(), b"{}\n");
    }

    #[test]
    fn stopping_ends_the_group_once_before_the_process_is_collected() {
        let spawner = scripted();
        let agent = Agent::spawn_with(&spawner, OsStr::new("adapter"), &[], 1024).unwrap();
        // Looked at before a stop, an agent is not collected.
        assert!(agent.try_status().unwrap().is_none());
        assert_eq!(*spawner.calls.lock().unwrap(), ["peek"]);
        let ended = agent.stop().unwrap();
        assert_eq!(ended, status(9));
        assert_eq!(
            *spawner.calls.lock().unwrap(),
            ["peek", "end_group", "try_wait"],
            "the group goes first, and the process is collected after"
        );
        // A second stop, as dropping does, signals nothing more.
        drop(agent);
        let calls = spawner.calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|call| **call == "end_group").count(), 1);
        assert_eq!(calls.iter().filter(|call| **call == "kill").count(), 0);
    }
}
