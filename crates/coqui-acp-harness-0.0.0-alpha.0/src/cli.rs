//! A harness's own command line run to completion or conversed with, through a [`Spawn`]: what a provider signs in
//! with, renews with, and asks `status` of.
//!
//! [`Cli`] hands over what a program writes as it arrives, so a sign-in can see the page it prints and answer the
//! question it asks; [`run`] runs one that is not conversed with, bounded in time and in output.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::net::IpAddr;
use core::time::Duration;
use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::process::{Exit, Launch, Process, Spawn};

/// How many pieces of output wait to be read before the program that writes them waits to write.
const PIECES: usize = 256;

/// What a program wrote, in the order it arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Piece {
    /// It came on standard error.
    pub error: bool,
    /// The bytes.
    pub bytes: Vec<u8>,
}

/// What [`Cli::next`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Something was written.
    Piece(Piece),
    /// Nothing within the wait.
    Idle,
    /// Both streams have ended.
    Ended,
}

/// A running program, its output handed over as it arrives.
pub struct Cli {
    stdin: Mutex<Option<Box<dyn Write + Send>>>,
    pieces: Mutex<Receiver<Piece>>,
    process: Box<dyn Process>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl core::fmt::Debug for Cli {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Cli").finish_non_exhaustive()
    }
}

fn pump(
    mut from: Box<dyn Read + Send>,
    error: bool,
    to: SyncSender<Piece>,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("coqui-acp-cli-output".into())
        .spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match from.read(&mut chunk) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        let piece = Piece {
                            error,
                            bytes: chunk[..n].to_vec(),
                        };
                        // A reader that is busy (a sign-in waiting on the person) holds the program back once
                        // `PIECES` are waiting, rather than the provider holding all it writes.
                        if to.send(piece).is_err() {
                            return;
                        }
                    }
                }
            }
        })
}

impl Cli {
    /// Starts `launch` through `spawn`.
    ///
    /// # Errors
    ///
    /// It could not be started.
    pub fn start(spawn: &dyn Spawn, launch: &Launch) -> io::Result<Arc<Self>> {
        let spawned = spawn.spawn(launch)?;
        let (sender, receiver) = mpsc::sync_channel(PIECES);
        let mut threads = vec![pump(spawned.stdout, false, sender.clone())?];
        if let Some(stderr) = spawned.stderr {
            threads.push(pump(stderr, true, sender.clone())?);
        }
        drop(sender);
        Ok(Arc::new(Self {
            stdin: Mutex::new(Some(spawned.stdin)),
            pieces: Mutex::new(receiver),
            process: spawned.process,
            threads: Mutex::new(threads),
        }))
    }

    /// The next thing it wrote, waiting at most `wait`.
    pub fn next(&self, wait: Duration) -> Next {
        let pieces = self.pieces.lock().unwrap_or_else(PoisonError::into_inner);
        match pieces.recv_timeout(wait) {
            Ok(piece) => Next::Piece(piece),
            Err(RecvTimeoutError::Timeout) => Next::Idle,
            Err(RecvTimeoutError::Disconnected) => Next::Ended,
        }
    }

    /// Writes `bytes` to its standard input.
    ///
    /// # Errors
    ///
    /// Its input is closed or it no longer reads.
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let mut stdin = self.stdin.lock().unwrap_or_else(PoisonError::into_inner);
        let pipe = stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "its input is closed"))?;
        pipe.write_all(bytes)?;
        pipe.flush()
    }

    /// Closes its standard input.
    pub fn close_input(&self) {
        drop(
            self.stdin
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
    }

    /// How it ended, when it has.
    pub fn exit(&self) -> Option<Exit> {
        self.process.try_wait().ok().flatten()
    }

    /// How it ended, waiting at most `wait` for it to.
    pub fn wait(&self, wait: Duration) -> Option<Exit> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(exit) = self.exit() {
                return Some(exit);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Ends it and everything it started.
    pub fn kill(&self) {
        self.close_input();
        let _ = self.process.kill();
    }

    /// Runs it to its end: `input` on its standard input (then closed) for at most `limit`, keeping at most
    /// `max_bytes` of each output stream. A program still running at `limit` is ended.
    pub fn collect(&self, input: Option<&[u8]>, limit: Duration, max_bytes: usize) -> Done {
        if let Some(input) = input {
            let _ = self.write(input);
        }
        self.close_input();
        let deadline = Instant::now() + limit;
        let mut done = Done {
            exit: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
            timed_out: false,
        };
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.kill();
                done.timed_out = true;
                return done;
            }
            match self.next(left.min(Duration::from_millis(200))) {
                Next::Piece(piece) => {
                    let kept = if piece.error {
                        &mut done.stderr
                    } else {
                        &mut done.stdout
                    };
                    let room = max_bytes.saturating_sub(kept.len());
                    kept.extend_from_slice(&piece.bytes[..piece.bytes.len().min(room)]);
                }
                Next::Idle => {}
                Next::Ended => break,
            }
        }
        done.exit = self.wait(
            deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(50)),
        );
        if done.exit.is_none() {
            self.kill();
            done.timed_out = true;
        }
        done
    }
}

impl Drop for Cli {
    fn drop(&mut self) {
        self.kill();
        // The output threads end with the program; one still reading a pipe some escaped process holds open ends when
        // that process does, so dropping never waits for it.
        let threads =
            core::mem::take(&mut *self.threads.lock().unwrap_or_else(PoisonError::into_inner));
        for thread in threads {
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                #[cfg(feature = "kind")]
                crate::provider::track(thread);
            }
        }
    }
}

/// What a program that was run to its end wrote and how it ended.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Done {
    /// How it ended; `None` when it was ended for taking too long.
    pub exit: Option<Exit>,
    /// What it wrote on standard output, at most the bound.
    pub stdout: Vec<u8>,
    /// What it wrote on standard error, at most the bound.
    pub stderr: Vec<u8>,
    /// It took longer than allowed and was ended.
    pub timed_out: bool,
}

impl Done {
    /// It exited by itself with code 0.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.exit.as_ref().is_some_and(|exit| exit.code == Some(0))
    }
}

/// Runs `launch` with `input` on its standard input (then closed) for at most `limit`, keeping at most `max_bytes` of each
/// output stream. A program still running at `limit` is ended.
///
/// Inside a sign-in, start the program with [`Job::run`](crate::login::Job::run) instead, so cancelling ends it.
///
/// # Errors
///
/// It could not be started.
pub fn run(
    spawn: &dyn Spawn,
    launch: &Launch,
    input: Option<&[u8]>,
    limit: Duration,
    max_bytes: usize,
) -> io::Result<Done> {
    Ok(Cli::start(spawn, launch)?.collect(input, limit, max_bytes))
}

/// `text` without the escape sequences a terminal reads: colour codes and operating system commands such as the
/// hyperlinks of OSC 8, which leave only their visible text.
#[must_use]
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The host of an `http(s)` address, lower case, without user, port or brackets.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if let Some(inner) = authority.strip_prefix('[') {
        inner.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.trim_end_matches('.').to_ascii_lowercase())
}

/// Whether `host` is this machine: `localhost`, a loopback or unspecified address.
#[must_use]
pub fn is_loopback(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
}

/// Whether `url` is a page the person can open: not this machine's, and when `hosts` names any, on one of them or
/// under one of them.
fn openable(url: &str, hosts: &[&str]) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    if is_loopback(&host) {
        return false;
    }
    hosts.is_empty()
        || hosts.iter().any(|allowed| {
            let allowed = allowed.to_ascii_lowercase();
            host == allowed || host.ends_with(&format!(".{allowed}"))
        })
}

/// The first `https://` (or `http://`) address in `text` that is a page to open, from an OSC 8 hyperlink when it has
/// one, else from its visible text; `None` while the address may still be arriving (nothing ends it yet).
///
/// An address on this machine (a login server a command starts on `localhost`) is never one. When `hosts` is not
/// empty only an address on one of them, or under one, is: a command's output cannot send the person elsewhere.
#[must_use]
pub fn address_in(text: &str, hosts: &[&str]) -> Option<String> {
    const OSC8: &str = "\u{1b}]8;;";
    let mut from = 0;
    while let Some(found) = text[from..].find(OSC8) {
        let rest = &text[from + found + OSC8.len()..];
        let Some(end) = rest.find(['\u{1b}', '\u{7}']) else {
            break;
        };
        let url = &rest[..end];
        if openable(url, hosts) {
            return Some(url.to_string());
        }
        from += found + OSC8.len() + end;
    }
    let shown = plain(text);
    let mut from = 0;
    while let Some(found) = shown[from..].find("http") {
        let at = from + found;
        let rest = &shown[at..];
        if !(rest.starts_with("https://") || rest.starts_with("http://")) {
            from = at + 4;
            continue;
        }
        let end = rest.find(char::is_whitespace)?;
        let url = rest[..end].trim_end_matches(['.', ',', ')']);
        if openable(url, hosts) {
            return Some(url.to_string());
        }
        from = at + end;
    }
    None
}
