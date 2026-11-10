//! The spawner a host hands every `start`: where and how a harness's programs run.

use alloc::string::String;
use alloc::vec::Vec;

use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, Secret, ToValue};

use crate::environment::Environment;

/// [`SpawnedProcess::read`] answers this when nothing arrived within the timeout.
pub const READ_TIMED_OUT: i64 = -1;

/// [`SpawnedProcess::read`] answers this while the link to where the process runs is down. The
/// process may still run, and its output waits for the link: read again.
pub const READ_LINK_DOWN: i64 = -2;

/// Starts programs where the host chooses: on this machine, on another over SSH, in a container or
/// a virtual machine. The host implements it and hands one to every
/// [`AcpHarness::start`](crate::AcpHarness::start); the provider owns it from then on, starts its
/// programs through it, or never calls it, and destroys it by the time it calls the sink's
/// `closed`.
///
/// Every slot takes `&self`, so a provider may call it from several threads at once.
#[guatiao::kind(object, name = "process_spawner")]
pub trait ProcessSpawner: Send + Sync {
    /// Where its programs run and what it can give them; the same on every call.
    fn describe(&self) -> Result<SpawnerInfo, ProviderError>;

    /// Starts `spec` and hands over the process. Refuses `GUATIAO_ERR_UNSUPPORTED` a spec asking
    /// what [`describe`](Self::describe) says it cannot give, `GUATIAO_ERR_NOT_FOUND` a program
    /// that is not there, `GUATIAO_ERR_BAD_VALUE` a spec it cannot read.
    fn spawn(&self, spec: SpawnSpec) -> Result<Object<dyn SpawnedProcess>, ProviderError>;
}

/// One program a spawner started, and everything it starts in turn.
///
/// Every slot takes `&self`, so one thread may write or kill while another waits in `read`.
/// Destroying it kills the whole tree, as [`kill`](Self::kill) does.
#[guatiao::kind(object, name = "spawned_process")]
pub trait SpawnedProcess: Send + Sync {
    /// Writes all of `bytes` to its standard input (its terminal's, with a terminal).
    /// `GUATIAO_ERR_GONE` once the input is closed or the process has ended.
    fn write(&self, bytes: &[u8]) -> Result<(), ProviderError>;

    /// Closes its standard input: the program reads the end. Closing it twice is no error.
    fn close_input(&self) -> Result<(), ProviderError>;

    /// Copies standard output (a terminal's whole output, with one) into `dst`: the byte count;
    /// `0` once no more can ever come; [`READ_TIMED_OUT`] after `timeout_ms` with nothing (`0`
    /// polls); [`READ_LINK_DOWN`] while the link is down. Never skips or repeats a byte, a link
    /// that dropped and came back included.
    fn read(&self, dst: &mut [u8], timeout_ms: u64) -> Result<i64, ProviderError>;

    /// The same for standard error, when the spec kept it [`ErrorOutput::Separate`]; `0` at once
    /// otherwise.
    fn read_error(&self, dst: &mut [u8], timeout_ms: u64) -> Result<i64, ProviderError>;

    /// Resizes its terminal. `GUATIAO_ERR_UNSUPPORTED` for a process started without one.
    fn resize(&self, size: TerminalSize) -> Result<(), ProviderError>;

    /// Waits up to `timeout_ms` (`0` polls) for it to end, and answers how it stands.
    fn wait(&self, timeout_ms: u64) -> Result<ProcessExit, ProviderError>;

    /// Ends it and every process it started: its process group or job, and those that left it
    /// when [`SpawnerInfo::kills_escaped`] says so. Killing an ended process is no error.
    fn kill(&self) -> Result<(), ProviderError>;

    /// Whether the link to where it runs is up, and how often it has dropped.
    fn link(&self) -> Result<LinkState, ProviderError>;
}

/// Where a spawner's programs run and what it can give them.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct SpawnerInfo {
    /// The operating system they run on, spelled as Rust's `std::env::consts::OS` spells it
    /// (`windows`, `linux`, `macos`, `android`): which rules their paths follow.
    pub os: String,
    /// Their processor, spelled as `std::env::consts::ARCH` spells it.
    pub arch: String,
    /// Whether they run on the host's own machine, sharing its files and paths.
    pub local: bool,
    /// Whether a spec may ask for a terminal.
    #[map(default)]
    pub terminal: bool,
    /// Whether a spec may keep standard error apart from standard output.
    #[map(default)]
    pub separate_error: bool,
    /// Whether a process keeps running while the link to it is down, its output kept for when it
    /// comes back.
    #[map(default)]
    pub survives_link_loss: bool,
    /// Whether [`SpawnedProcess::kill`] also reaches the processes that left the program's process
    /// group or session; when not, those escape it.
    #[map(default)]
    pub kills_escaped: bool,
    /// Whether a process's tree ends when the host dies without killing it (its group or job, at
    /// least); when not, it outlives the host.
    #[map(default)]
    pub ends_with_host: bool,
}

impl SpawnerInfo {
    /// Programs on `os` and `arch`, here or not, with no terminal, standard error merged, a kill
    /// reaching the process group only, and nothing ended with the host.
    #[must_use]
    pub fn new(os: impl Into<String>, arch: impl Into<String>, local: bool) -> Self {
        Self {
            os: os.into(),
            arch: arch.into(),
            local,
            terminal: false,
            separate_error: false,
            survives_link_loss: false,
            kills_escaped: false,
            ends_with_host: false,
        }
    }
}

/// What to start. A key this type does not declare is refused when read, so a spawner never runs
/// a program without a setting a newer provider asked for.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct SpawnSpec {
    /// The program: a path, or a name looked up on the `PATH` where it runs, the spec's own
    /// `PATH` when its environment sets one.
    pub program: String,
    /// Its arguments, one element each, never re-split by a shell.
    #[map(default)]
    pub args: Vec<String>,
    /// Its working directory, by the rules of where it runs; the spawner's choice when absent.
    pub cwd: Option<String>,
    /// Variables set over the spawner's own environment, or over none with `clear_environment`.
    #[map(default)]
    pub environment: Secret<Environment>,
    /// Starts from no variables at all rather than the spawner's own.
    #[map(default)]
    pub clear_environment: bool,
    /// Variables taken out of the spawner's own environment before `environment` is set.
    #[map(default)]
    pub remove_environment: Vec<String>,
    /// A terminal of this size for its input and output, standard error included; plain pipes
    /// when absent.
    pub terminal: Option<TerminalSize>,
    /// Where standard error goes, with pipes.
    #[map(default)]
    pub error_output: ErrorOutput,
}

impl SpawnSpec {
    /// `program` with no arguments, in the spawner's directory and environment, on pipes, standard
    /// error apart.
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    /// Adds one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Its working directory.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// The variables set over the spawner's own.
    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = Secret::new(environment);
        self
    }

    /// Starts from no variables at all.
    #[must_use]
    pub const fn clear_environment(mut self, clear: bool) -> Self {
        self.clear_environment = clear;
        self
    }

    /// A terminal of `size`.
    #[must_use]
    pub const fn terminal(mut self, size: TerminalSize) -> Self {
        self.terminal = Some(size);
        self
    }

    /// Where standard error goes.
    #[must_use]
    pub const fn error_output(mut self, output: ErrorOutput) -> Self {
        self.error_output = output;
        self
    }
}

/// Where a process's standard error goes, on pipes.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[map(crate = ::guatiao, rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorOutput {
    /// Its own stream, read with [`SpawnedProcess::read_error`]: standard output then carries only
    /// what the program wrote there, as a JSON-lines harness needs.
    #[default]
    Separate,
    /// Into standard output.
    Merged,
    /// Nowhere.
    Discarded,
}

/// A terminal's size, in character cells.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct TerminalSize {
    /// Rows.
    pub rows: u16,
    /// Columns.
    pub cols: u16,
}

impl TerminalSize {
    /// `rows` by `cols`.
    #[must_use]
    pub const fn new(rows: u16, cols: u16) -> Self {
        Self { rows, cols }
    }
}

/// How a process stands: running, or how it ended.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct ProcessExit {
    /// Whether it still runs; neither `code` nor `signal` is set while it does.
    pub running: bool,
    /// Its exit code, when it exited by itself.
    pub code: Option<i64>,
    /// The signal that ended it, on Unix.
    pub signal: Option<i64>,
}

impl ProcessExit {
    /// Still running.
    #[must_use]
    pub const fn running() -> Self {
        Self {
            running: true,
            code: None,
            signal: None,
        }
    }

    /// Ended with `code`, or by `signal`.
    #[must_use]
    pub const fn ended(code: Option<i64>, signal: Option<i64>) -> Self {
        Self {
            running: false,
            code,
            signal,
        }
    }
}

/// Whether the link to where a process runs is up.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct LinkState {
    /// Whether it is up now. A local process's link is always up.
    pub up: bool,
    /// How many times it has gone down since the process started: a reader that sees the count
    /// grow knows its output paused.
    pub drops: u64,
}

impl LinkState {
    /// A link that has never dropped.
    #[must_use]
    pub const fn up() -> Self {
        Self { up: true, drops: 0 }
    }
}
