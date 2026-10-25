//! Running commands: to completion over an exec channel, or on a terminal.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use ai6::guatiao::Status;
use ai6::guatiao::library::ProviderError;
use ai6::{ExitInfo, Output, READ_TIMED_OUT, RunSpec, Spec, TerminalSession};
use russh::client::Msg;
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Sig};
use tokio::runtime::Handle;

use crate::command::{command_line, signal_number};
use crate::refuse;
use crate::session::Link;

/// How long output and the end of the channel are awaited after the program has exited. A
/// descendant that outlives it can keep the channel open indefinitely.
const EXIT_GRACE: Duration = Duration::from_secs(1);

pub(crate) async fn run(link: &Link, spec: &RunSpec) -> Result<Output, ProviderError> {
    let command = command_line(&spec.argv, spec.cwd.as_deref(), spec.env.as_ref())
        .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
    let (_conn, mut channel) = link.channel().await?;
    let sent = async {
        channel.exec(true, command).await?;
        if let Some(stdin) = &spec.stdin {
            channel.data(&stdin.0[..]).await?;
        }
        channel.eof().await
    };
    sent.await
        .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;

    let limit = usize::try_from(spec.max_output).unwrap_or(usize::MAX);
    let deadline = (spec.timeout_ms > 0)
        .then(|| tokio::time::Instant::now() + Duration::from_millis(spec.timeout_ms));
    let mut output = Output::default();
    let mut exited_at: Option<tokio::time::Instant> = None;
    let mut timed_out = false;
    loop {
        let until = match (deadline, exited_at) {
            (Some(deadline), Some(exited)) => Some(deadline.min(exited + EXIT_GRACE)),
            (Some(deadline), None) => Some(deadline),
            (None, Some(exited)) => Some(exited + EXIT_GRACE),
            (None, None) => None,
        };
        let message = match until {
            Some(until) => match tokio::time::timeout_at(until, channel.wait()).await {
                Ok(message) => message,
                Err(_) => {
                    timed_out = exited_at.is_none();
                    break;
                }
            },
            None => channel.wait().await,
        };
        match message {
            None | Some(ChannelMsg::Close) => break,
            Some(ChannelMsg::Data { data }) => {
                keep(&mut output.stdout.0, &data, limit, &mut output.truncated);
            }
            Some(ChannelMsg::ExtendedData { data, ext: 1 }) => {
                keep(&mut output.stderr.0, &data, limit, &mut output.truncated);
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                output.code = Some(i64::from(exit_status));
                exited_at.get_or_insert_with(tokio::time::Instant::now);
            }
            Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                output.signal = signal_number(sig_name(&signal_name));
                exited_at.get_or_insert_with(tokio::time::Instant::now);
            }
            Some(ChannelMsg::Failure | ChannelMsg::OpenFailure(_)) => {
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "the host refused to run the command",
                ));
            }
            Some(_) => {}
        }
    }
    if timed_out {
        output.timed_out = true;
        output.code = None;
        let _ = channel.signal(Sig::KILL).await;
    }
    let _ = channel.close().await;
    Ok(output)
}

fn sig_name(signal: &Sig) -> &str {
    match signal {
        Sig::ABRT => "ABRT",
        Sig::ALRM => "ALRM",
        Sig::FPE => "FPE",
        Sig::HUP => "HUP",
        Sig::ILL => "ILL",
        Sig::INT => "INT",
        Sig::KILL => "KILL",
        Sig::PIPE => "PIPE",
        Sig::QUIT => "QUIT",
        Sig::SEGV => "SEGV",
        Sig::TERM => "TERM",
        Sig::USR1 => "USR1",
        Sig::Custom(name) => name,
    }
}

/// Appends `data` to `kept` up to `limit` bytes and notes what did not fit.
fn keep(kept: &mut Vec<u8>, data: &[u8], limit: usize, truncated: &mut bool) {
    let room = limit.saturating_sub(kept.len());
    kept.extend_from_slice(&data[..data.len().min(room)]);
    *truncated |= data.len() > room;
}

/// How a terminal's program ended, shared between the reader task and callers.
struct Ending {
    state: Mutex<EndState>,
    changed: Condvar,
}

#[derive(Default)]
struct EndState {
    exited: bool,
    code: Option<i64>,
    signal: Option<i64>,
}

/// A program running on a pty on the host.
pub(crate) struct Terminal {
    runtime: Handle,
    write: Arc<ChannelWriteHalf<Msg>>,
    chunks: Mutex<Receiver<Vec<u8>>>,
    pending: Mutex<Vec<u8>>,
    ending: Arc<Ending>,
    killed: AtomicBool,
}

pub(crate) async fn open_terminal(link: &Link, spec: &Spec) -> Result<Terminal, ProviderError> {
    let command = command_line(&spec.argv, spec.cwd.as_deref(), spec.env.as_ref())
        .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
    let term = spec
        .env
        .as_ref()
        .and_then(|env| env.get("TERM"))
        .map_or("xterm-256color", String::as_str);
    let (_conn, channel) = link.channel().await?;
    channel
        .request_pty(
            true,
            term,
            u32::from(spec.cols),
            u32::from(spec.rows),
            0,
            0,
            &[],
        )
        .await
        .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
    channel
        .exec(true, command)
        .await
        .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
    let (mut read, write) = channel.split();

    // The host answers the pty request and the command in order; output may follow at once.
    let (sender, receiver) = channel_pair();
    let mut replies = 0;
    while replies < 2 {
        match read.wait().await {
            Some(ChannelMsg::Success) => replies += 1,
            Some(ChannelMsg::Data { data }) => {
                let _ = sender.send(data.to_vec());
            }
            Some(ChannelMsg::Failure | ChannelMsg::OpenFailure(_)) | None => {
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "the host refused the terminal or the command",
                ));
            }
            Some(_) => {}
        }
    }

    let ending = Arc::new(Ending {
        state: Mutex::new(EndState::default()),
        changed: Condvar::new(),
    });
    tokio::spawn(pump(read, sender, ending.clone()));
    Ok(Terminal {
        runtime: Handle::current(),
        write: Arc::new(write),
        chunks: Mutex::new(receiver),
        pending: Mutex::new(Vec::new()),
        ending,
        killed: AtomicBool::new(false),
    })
}

fn channel_pair() -> (std::sync::mpsc::Sender<Vec<u8>>, Receiver<Vec<u8>>) {
    channel()
}

/// Moves a terminal's output to its reader and records how the program ended.
async fn pump(
    mut read: ChannelReadHalf,
    output: std::sync::mpsc::Sender<Vec<u8>>,
    ending: Arc<Ending>,
) {
    while let Some(message) = read.wait().await {
        match message {
            ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                if output.send(data.to_vec()).is_err() {
                    break;
                }
            }
            ChannelMsg::ExitStatus { exit_status } => {
                finish(&ending, Some(i64::from(exit_status)), None);
            }
            ChannelMsg::ExitSignal { signal_name, .. } => {
                finish(&ending, None, signal_number(sig_name(&signal_name)));
            }
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    // Dropping the sender tells `read` that nothing more can arrive.
    drop(output);
    finish(&ending, None, None);
}

fn finish(ending: &Ending, code: Option<i64>, signal: Option<i64>) {
    let mut state = ending
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !state.exited {
        state.exited = true;
        state.code = code;
        state.signal = signal;
    }
    ending.changed.notify_all();
}

impl TerminalSession for Terminal {
    fn pid(&self) -> i64 {
        // The host does not tell an SSH client the process ID.
        -1
    }

    fn read(&self, dst: &mut [u8], timeout_ms: i64) -> Result<i64, ProviderError> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.is_empty() {
            let chunks = self
                .chunks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let wait = Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(0));
            match chunks.recv_timeout(wait) {
                Ok(chunk) => *pending = chunk,
                Err(RecvTimeoutError::Timeout) => return Ok(READ_TIMED_OUT),
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        let count = pending.len().min(dst.len());
        dst[..count].copy_from_slice(&pending[..count]);
        pending.drain(..count);
        Ok(i64::try_from(count).unwrap_or(i64::MAX))
    }

    fn write(&self, data: &[u8]) -> Result<(), ProviderError> {
        self.runtime
            .block_on(self.write.data_bytes(data.to_vec()))
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))
    }

    fn resize(&self, rows: i64, cols: i64) -> Result<(), ProviderError> {
        let (rows, cols) = (u32::try_from(rows), u32::try_from(cols));
        let (Ok(rows), Ok(cols)) = (rows, cols) else {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "rows and cols must be positive",
            ));
        };
        self.runtime
            .block_on(self.write.window_change(cols, rows, 0, 0))
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))
    }

    fn kill(&self) -> Result<(), ProviderError> {
        if self.killed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.runtime.block_on(async {
            // Not every server honours a signal request; closing the channel hangs up the
            // terminal either way.
            let _ = self.write.signal(Sig::KILL).await;
            let _ = self.write.close().await;
        });
        // The host may never answer a closed channel, so the program is over from here.
        finish(&self.ending, None, signal_number("KILL"));
        Ok(())
    }

    fn exit_status(&self, timeout_ms: i64) -> Result<ai6::guatiao::Value, ProviderError> {
        let wait = Duration::from_millis(u64::try_from(timeout_ms).unwrap_or(0));
        let deadline = Instant::now() + wait;
        let mut state = self
            .ending
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !state.exited {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self
                .ending
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        let info = ExitInfo {
            running: !state.exited,
            code: state.code,
            signal: state.signal,
        };
        ai6::guatiao::ToValue::to_value(&info)
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if !self.killed.swap(true, Ordering::AcqRel) {
            let write = self.write.clone();
            self.runtime.spawn(async move {
                let _ = write.signal(Sig::KILL).await;
                let _ = write.close().await;
            });
        }
    }
}
