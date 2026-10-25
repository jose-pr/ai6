// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Helpers for transport providers that reach a host through a command: shell quoting, running
//! a local process with a timeout and an output limit, and file operations built from POSIX
//! shell commands for hosts that offer nothing but a way to run them.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use guatiao::library::ProviderError;
use guatiao::{Bytes, Status};

use super::{DirEntry, DirListing, FileData, Output, ReadSpec, WriteSpec};

/// The exit status a shell script uses to say that a file or directory does not exist.
pub const EXIT_NOT_FOUND: i64 = 44;

const POLL: Duration = Duration::from_millis(5);
/// How long output readers get to finish after the program has ended. A descendant that
/// outlives it can hold a pipe open indefinitely.
const READER_GRACE: Duration = Duration::from_secs(1);

/// Quotes `word` so a POSIX shell reads it back as exactly one word.
pub fn quote(word: &str) -> String {
    let mut quoted = String::with_capacity(word.len() + 2);
    quoted.push('\'');
    for ch in word.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

/// `cd <cwd> && exec env <K=V>... <argv>...`, leaving out what is not set, for a host that
/// takes one command string and reads it with a POSIX shell.
pub fn command_line(
    argv: &[String],
    cwd: Option<&str>,
    env: Option<&BTreeMap<String, String>>,
) -> Result<String, String> {
    if argv.is_empty() {
        return Err("argv is empty".to_owned());
    }
    if argv.iter().any(|word| word.contains('\0')) {
        return Err("argv holds a NUL byte".to_owned());
    }
    let mut line = String::new();
    if let Some(cwd) = cwd {
        line.push_str("cd ");
        line.push_str(&quote(cwd));
        line.push_str(" && ");
    }
    line.push_str("exec ");
    if let Some(env) = env.filter(|env| !env.is_empty()) {
        line.push_str("env");
        for (name, value) in env {
            if !is_variable_name(name) {
                return Err(format!("{name:?} is not an environment variable name"));
            }
            line.push(' ');
            line.push_str(&quote(&format!("{name}={value}")));
        }
        line.push(' ');
    }
    let words: Vec<String> = argv.iter().map(|word| quote(word)).collect();
    line.push_str(&words.join(" "));
    Ok(line)
}

/// Whether `name` can name an environment variable.
pub fn is_variable_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// What to feed a process and how long and how much to let it produce.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessInput<'a> {
    /// Bytes written to standard input, which is then closed; none closes it at once.
    pub stdin: Option<&'a [u8]>,
    /// Milliseconds before the process is killed; `0` waits for it.
    pub timeout_ms: u64,
    /// Bytes of each output stream kept; the rest is drained and dropped.
    pub max_output: u64,
}

/// Runs `command` to completion. A process that cannot be started is an error (`NOT_FOUND`
/// when its program does not exist); one that fails is a normal [`Output`].
pub fn run_process(
    command: &mut Command,
    input: &ProcessInput<'_>,
) -> Result<Output, ProviderError> {
    command
        .stdin(if input.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .spawn()
        .map_err(|error| io_error(&error, &program))?;

    if let (Some(data), Some(mut stdin)) = (input.stdin, child.stdin.take()) {
        let data = data.to_vec();
        // A program that stops reading breaks the pipe; its exit status says what happened.
        thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }
    let limit = usize::try_from(input.max_output).unwrap_or(usize::MAX);
    let stdout = child.stdout.take().map(|pipe| capture(pipe, limit));
    let stderr = child.stderr.take().map(|pipe| capture(pipe, limit));

    let deadline =
        (input.timeout_ms > 0).then(|| Instant::now() + Duration::from_millis(input.timeout_ms));
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| io_error(&error, &program))?
        {
            break status;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            timed_out = true;
            let _ = child.kill();
            break child.wait().map_err(|error| io_error(&error, &program))?;
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

/// A failed filesystem or process call, with the status a caller can act on.
pub fn io_error(error: &std::io::Error, what: &str) -> ProviderError {
    let status = match error.kind() {
        std::io::ErrorKind::NotFound => Status::GUATIAO_ERR_NOT_FOUND,
        _ => Status::GUATIAO_ERR_INTERNAL,
    };
    ProviderError::new(status, &format!("{what}: {error}"))
}

/// A host that can run a POSIX shell script. File operations are built on it.
pub trait Shell {
    /// Runs `script` with `sh -c` on the host, feeding it `stdin`, and keeps at most
    /// `max_output` bytes of each output stream.
    fn sh(
        &self,
        script: &str,
        stdin: Option<&[u8]>,
        max_output: u64,
    ) -> Result<Output, ProviderError>;
}

fn failed(output: &Output, path: &str) -> ProviderError {
    let text = String::from_utf8_lossy(&output.stderr.0);
    let text = text.trim();
    let status = if output.code == Some(EXIT_NOT_FOUND) {
        Status::GUATIAO_ERR_NOT_FOUND
    } else {
        Status::GUATIAO_ERR_INTERNAL
    };
    let why = if text.is_empty() {
        format!("exit {:?}", output.code)
    } else {
        text.to_owned()
    };
    ProviderError::new(status, &format!("{path}: {why}"))
}

/// Reads part of a file with `wc`, `tail` and `head`. Binary-safe if the host's output is.
pub fn read_file(shell: &dyn Shell, spec: &ReadSpec) -> Result<FileData, ProviderError> {
    let script = format!(
        "f={path}; [ -f \"$f\" ] || {{ echo \"no such file\" >&2; exit {EXIT_NOT_FOUND}; }}; \
         wc -c < \"$f\"; tail -c +{from} \"$f\" | head -c {limit}",
        path = quote(&spec.path),
        from = spec.offset.saturating_add(1),
        limit = spec.limit,
    );
    let room = spec.limit.saturating_add(64);
    let output = shell.sh(&script, None, room)?;
    if output.code != Some(0) || output.truncated {
        return Err(failed(&output, &spec.path));
    }
    let bytes = output.stdout.0;
    let newline = bytes
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or_else(|| {
            ProviderError::new(
                Status::GUATIAO_ERR_INTERNAL,
                &format!("{}: the host did not report the file's size", spec.path),
            )
        })?;
    let size = String::from_utf8_lossy(&bytes[..newline])
        .trim()
        .parse::<u64>()
        .map_err(|_| {
            ProviderError::new(
                Status::GUATIAO_ERR_INTERNAL,
                &format!("{}: the host reported an unreadable size", spec.path),
            )
        })?;
    Ok(FileData {
        data: Bytes(bytes[newline + 1..].to_vec()),
        size,
    })
}

/// Writes (or appends to) a file with `cat`.
pub fn write_file(shell: &dyn Shell, spec: &WriteSpec) -> Result<(), ProviderError> {
    let parents = if spec.create_dirs {
        "mkdir -p \"$(dirname \"$f\")\" || exit 1; "
    } else {
        ""
    };
    let redirect = if spec.append { ">>" } else { ">" };
    let script = format!(
        "f={path}; {parents}cat {redirect} \"$f\"",
        path = quote(&spec.path)
    );
    let output = shell.sh(&script, Some(&spec.data.0), 64 * 1024)?;
    if output.code == Some(0) {
        Ok(())
    } else {
        Err(failed(&output, &spec.path))
    }
}

/// Lists a directory; entries are sorted by name. A name holding a tab or a newline is not
/// listed correctly.
pub fn list_dir(shell: &dyn Shell, path: &str) -> Result<DirListing, ProviderError> {
    let script = format!(
        "d={path}; [ -d \"$d\" ] || {{ echo \"no such directory\" >&2; exit {EXIT_NOT_FOUND}; }}; \
         for f in \"$d\"/* \"$d\"/.[!.]* \"$d\"/..?*; do \
           [ -e \"$f\" ] || [ -L \"$f\" ] || continue; \
           if [ -L \"$f\" ]; then k=link; s=0; \
           elif [ -d \"$f\" ]; then k=dir; s=0; \
           elif [ -f \"$f\" ]; then k=file; s=$(wc -c < \"$f\"); \
           else k=other; s=0; fi; \
           printf '%s\\t%s\\t%s\\n' \"$k\" \"$s\" \"${{f##*/}}\"; \
         done",
        path = quote(path),
    );
    let output = shell.sh(&script, None, 4 * 1024 * 1024)?;
    if output.code != Some(0) {
        return Err(failed(&output, path));
    }
    let mut entries = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout.0).lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(kind), Some(size), Some(name)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        entries.push(DirEntry {
            name: name.to_owned(),
            kind: kind.to_owned(),
            size: size.trim().parse().unwrap_or(0),
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(DirListing { entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_survive_the_shell_unchanged() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote("a b; rm -rf *"), "'a b; rm -rf *'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn the_command_line_carries_cwd_and_environment() {
        let argv = ["echo".to_owned(), "hi there".to_owned()];
        assert_eq!(
            command_line(&argv, None, None).unwrap(),
            "exec 'echo' 'hi there'"
        );
        let env = BTreeMap::from([("A".to_owned(), "x y".to_owned())]);
        assert_eq!(
            command_line(&argv, Some("/srv/it's"), Some(&env)).unwrap(),
            "cd '/srv/it'\\''s' && exec env 'A=x y' 'echo' 'hi there'"
        );
    }

    #[test]
    fn invalid_specs_are_refused_before_anything_is_sent() {
        assert!(command_line(&[], None, None).is_err());
        assert!(command_line(&["a\0".to_owned()], None, None).is_err());
        for name in ["", "A=B", "1A", "A-B", "A B"] {
            let env = BTreeMap::from([(name.to_owned(), "v".to_owned())]);
            assert!(command_line(&["x".to_owned()], None, Some(&env)).is_err());
        }
    }

    #[test]
    fn a_missing_program_is_not_found_and_a_failing_one_is_an_output() {
        let missing = run_process(
            &mut Command::new("ai6-no-such-program"),
            &ProcessInput::default(),
        );
        assert!(missing.is_err_and(|e| e.status == Status::GUATIAO_ERR_NOT_FOUND));
    }
}
