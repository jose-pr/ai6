//! The Station's terminal, as the process `coqui-acp-codex` starts the app server through.
//!
//! The terminal may be a pseudo-terminal, whose output carries controls, breaks long lines and
//! echoes what is written to it, so the output is read through [`JsonLines`] and the echo of each
//! request is dropped before the driver reads it.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ai6::directorates::q::terminal::{
    AgentTerminal, ExitInfo, JsonLines, READ_TIMED_OUT, STREAM_COLS, Spec, StreamLine, TerminalRun,
};
use ai6::guatiao::library::Object;
use ai6::guatiao::{FromValue, ToValue};
use ai6::station::limits::TerminalOutputLimits;
use coqui_acp_codex::harness::process::{Exit, Launch, Process, Spawn, Spawned};
use serde_json::Value;

/// How long one read of the terminal waits before it looks at whether the Run was ended.
const POLL_MS: i64 = 200;

/// How many written lines are remembered to drop their echo.
const ECHOES: usize = 64;

/// What the reader thread and the Run share about the terminal.
#[derive(Default)]
pub(crate) struct Link {
    /// Whether the line read last was rebuilt from the terminal's rendering rather than read as
    /// written ([`StreamLine::Json::rebuilt`]); an approval acts on the exact text only.
    pub rebuilt: AtomicBool,
    /// Set once the Run ended the program on purpose.
    killed: AtomicBool,
    /// The program's terminal, once started.
    run: OnceLock<Arc<Object<dyn TerminalRun>>>,
}

impl Link {
    /// Ends the program and everything it started.
    pub fn kill(&self) {
        self.killed.store(true, Ordering::Release);
        if let Some(run) = self.run.get() {
            let _ = run.kill();
        }
    }
}

/// Starts the app server on the Station's terminal provider.
pub(crate) struct TerminalSpawn {
    pub terminal: Object<dyn AgentTerminal>,
    pub environment: std::collections::BTreeMap<String, String>,
    pub limits: TerminalOutputLimits,
    pub link: Arc<Link>,
}

impl Spawn for TerminalSpawn {
    fn spawn(&self, launch: &Launch) -> io::Result<Spawned> {
        let mut argv = vec![launch.program.to_string_lossy().into_owned()];
        argv.extend(
            launch
                .args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        );
        let spec = Spec {
            argv,
            cwd: launch
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().into_owned()),
            env: Some(self.environment.clone()),
            rows: 24,
            cols: STREAM_COLS,
        };
        let value = spec.to_value().map_err(io::Error::other)?;
        let run = Arc::new(
            self.terminal
                .spawn(&value)
                .map_err(|error| io::Error::other(error.to_string()))?,
        );
        let _ = self.link.run.set(Arc::clone(&run));
        let echoes = Arc::new(Mutex::new(VecDeque::new()));
        Ok(Spawned::new(
            Box::new(Input {
                run: Arc::clone(&run),
                echoes: Arc::clone(&echoes),
            }),
            Box::new(Output {
                run: Arc::clone(&run),
                lines: JsonLines::new(&self.limits),
                chunk: vec![0; 8192],
                pending: Vec::new(),
                at: 0,
                ended: false,
                link: Arc::clone(&self.link),
                echoes,
            }),
            None,
            Box::new(Program {
                run,
                link: Arc::clone(&self.link),
            }),
        ))
    }
}

type Echoes = Arc<Mutex<VecDeque<Value>>>;

/// The app server's standard input: what is written is also remembered, for the terminal's echo.
struct Input {
    run: Arc<Object<dyn TerminalRun>>,
    echoes: Echoes,
}

impl Write for Input {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if let Ok(value) = serde_json::from_slice::<Value>(data.trim_ascii_end())
            && let Ok(mut echoes) = self.echoes.lock()
        {
            if echoes.len() >= ECHOES {
                echoes.pop_front();
            }
            echoes.push_back(value);
        }
        self.run
            .write(data)
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The app server's output as the lines it wrote: controls removed, a line the terminal broke
/// joined, the echo of what was written dropped.
struct Output {
    run: Arc<Object<dyn TerminalRun>>,
    lines: JsonLines,
    chunk: Vec<u8>,
    pending: Vec<u8>,
    at: usize,
    ended: bool,
    link: Arc<Link>,
    echoes: Echoes,
}

impl Output {
    /// Takes one line as the bytes the driver reads next; `false` for an echo, which is dropped.
    fn take(&mut self, line: StreamLine) -> bool {
        match line {
            StreamLine::Json {
                value,
                text,
                rebuilt,
            } => {
                if let Ok(mut echoes) = self.echoes.lock()
                    && let Some(at) = echoes.iter().position(|echo| *echo == value)
                {
                    echoes.remove(at);
                    return false;
                }
                self.link.rebuilt.store(rebuilt, Ordering::Release);
                self.pending = text;
            }
            StreamLine::Text(text) => self.pending = text.into_bytes(),
        }
        self.pending.push(b'\n');
        self.at = 0;
        true
    }
}

impl Read for Output {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.at < self.pending.len() {
                let count = buffer.len().min(self.pending.len() - self.at);
                buffer[..count].copy_from_slice(&self.pending[self.at..self.at + count]);
                self.at += count;
                return Ok(count);
            }
            if let Some(line) = self.lines.next_line() {
                self.take(line);
                continue;
            }
            if self.ended {
                match self.lines.finish() {
                    Some(line) => {
                        self.take(line);
                        continue;
                    }
                    None => return Ok(0),
                }
            }
            let count = self
                .run
                .read(&mut self.chunk, POLL_MS)
                .map_err(|error| io::Error::other(error.to_string()))?;
            if count == READ_TIMED_OUT {
                // A terminal may keep its output open for good once the program is ended.
                if self.link.killed.load(Ordering::Acquire) {
                    self.ended = true;
                }
                continue;
            }
            match usize::try_from(count) {
                Ok(0) => self.ended = true,
                Ok(count) => self.lines.push(&self.chunk[..count]),
                Err(_) => {
                    return Err(io::Error::other(
                        "the terminal returned an invalid read count",
                    ));
                }
            }
        }
    }
}

/// The program, as far as the driver needs it.
struct Program {
    run: Arc<Object<dyn TerminalRun>>,
    link: Arc<Link>,
}

impl Process for Program {
    fn kill(&self) -> io::Result<()> {
        self.link.killed.store(true, Ordering::Release);
        self.run
            .kill()
            .map_err(|error| io::Error::other(error.to_string()))
    }

    fn try_wait(&self) -> io::Result<Option<Exit>> {
        // A terminal that cannot say makes the driver stop asking.
        let status = self
            .run
            .exit_status(0)
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(ExitInfo::from_value(&status)
            .ok()
            .filter(|exit| !exit.running)
            .map(|exit| {
                Exit::new(
                    exit.code.map(|code| code as i32),
                    exit.signal.map(|signal| signal as i32),
                )
            }))
    }
}
