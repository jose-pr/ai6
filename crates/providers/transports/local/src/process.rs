use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{Bytes, Status};
use ai6::{Output, RunSpec};

use crate::{io_error, refuse};

const POLL: Duration = Duration::from_millis(5);
/// How long output readers get to finish after the program has been killed. A descendant
/// that outlives it can hold a pipe open indefinitely.
const READER_GRACE: Duration = Duration::from_secs(1);

pub(crate) fn run(spec: &RunSpec) -> Result<Output, ProviderError> {
    let (program, args) = spec
        .argv
        .split_first()
        .ok_or_else(|| refuse(Status::GUATIAO_ERR_BAD_VALUE, "argv is empty"))?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    if let Some(env) = &spec.env {
        command.envs(env);
    }
    let mut child = command.spawn().map_err(|error| io_error(&error, program))?;

    if let (Some(data), Some(mut stdin)) = (spec.stdin.clone(), child.stdin.take()) {
        // A program that stops reading breaks the pipe; its exit status says what happened.
        thread::spawn(move || {
            let _ = stdin.write_all(&data.0);
        });
    }
    let limit = usize::try_from(spec.max_output).unwrap_or(usize::MAX);
    let stdout = child.stdout.take().map(|pipe| capture(pipe, limit));
    let stderr = child.stderr.take().map(|pipe| capture(pipe, limit));

    let deadline =
        (spec.timeout_ms > 0).then(|| Instant::now() + Duration::from_millis(spec.timeout_ms));
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| io_error(&error, program))?
        {
            break status;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            timed_out = true;
            let _ = child.kill();
            break child.wait().map_err(|error| io_error(&error, program))?;
        }
        thread::sleep(POLL);
    };

    let (stdout, out_truncated) = collect(stdout);
    let (stderr, err_truncated) = collect(stderr);
    Ok(Output {
        code: if timed_out {
            None
        } else {
            status.code().map(i64::from)
        },
        signal: signal_of(&status),
        stdout: Bytes(stdout),
        stderr: Bytes(stderr),
        timed_out,
        truncated: out_truncated || err_truncated,
    })
}

/// Reads a pipe to its end on a thread, keeping at most `limit` bytes and draining the rest
/// so the program never blocks on a full pipe.
fn capture(mut pipe: impl Read + Send + 'static, limit: usize) -> mpsc::Receiver<(Vec<u8>, bool)> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut truncated = false;
        let mut buffer = [0u8; 16 * 1024];
        while let Ok(count) = pipe.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let room = limit.saturating_sub(kept.len());
            kept.extend_from_slice(&buffer[..count.min(room)]);
            truncated |= count > room;
        }
        let _ = sender.send((kept, truncated));
    });
    receiver
}

fn collect(receiver: Option<mpsc::Receiver<(Vec<u8>, bool)>>) -> (Vec<u8>, bool) {
    receiver
        .and_then(|receiver| receiver.recv_timeout(READER_GRACE).ok())
        .unwrap_or_default()
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
