//! Running a sign-in the way the kind says it ends: the person cancelling, the time running out, a refusal and a failure.
//!
//! A sign-in's work runs on a thread of its own and [`supervise`] watches it, so the answer comes when the person
//! cancels or the time is up even while the work waits on the person or on the command it started. The work starts its
//! commands through [`Job::start`] or [`Job::run`], so the supervisor can end their whole trees.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::{LoginOutcome, LoginPrompter, ProcessSpawner};

use crate::cli::{Cli, Done, plain};
use crate::process::{Launch, Spawn};
use crate::provider::track;

/// How long the supervisor waits, after ending the commands, for the work to let go of what it was handed.
const LET_GO: Duration = Duration::from_millis(1000);

#[derive(Debug, Default)]
struct JobState {
    abandoned: bool,
    started: Vec<Arc<Cli>>,
}

/// What the work of a sign-in can reach to learn it has ended, and where it registers what it starts.
#[derive(Debug, Default)]
pub struct Job {
    state: Mutex<JobState>,
}

impl Job {
    /// Starts `launch` through `spawn` and registers it, so the supervisor ends it with the sign-in.
    ///
    /// # Errors
    ///
    /// The sign-in has ended (`GONE`), or the program could not be started (`INTERNAL`, the reason naming nothing of the
    /// launch).
    pub fn start(&self, spawn: &dyn Spawn, launch: &Launch) -> Result<Arc<Cli>, ProviderError> {
        self.check()?;
        let cli = Cli::start(spawn, launch).map_err(|error| {
            let why = if error.kind() == std::io::ErrorKind::NotFound {
                "the program could not be found"
            } else {
                "the program could not be started"
            };
            ProviderError::new(Status::GUATIAO_ERR_INTERNAL, why)
        })?;
        // The check and the registration are one step: a program started as the supervisor answers is ended here.
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.abandoned {
            drop(state);
            cli.kill();
            return Err(gone("the sign-in has ended"));
        }
        state.started.push(Arc::clone(&cli));
        Ok(cli)
    }

    /// Runs `launch` to its end as [`cli::run`](crate::cli::run) does, registered with the sign-in so cancelling or
    /// running out of time ends it.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start).
    pub fn run(
        &self,
        spawn: &dyn Spawn,
        launch: &Launch,
        input: Option<&[u8]>,
        limit: Duration,
        max_bytes: usize,
    ) -> Result<Done, ProviderError> {
        let cli = self.start(spawn, launch)?;
        Ok(cli.collect(input, limit, max_bytes))
    }

    /// Whether the sign-in is over: the work asks and starts nothing more once it is.
    ///
    /// # Errors
    ///
    /// `GONE` when the supervisor has answered.
    pub fn check(&self) -> Result<(), ProviderError> {
        if self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .abandoned
        {
            Err(gone("the sign-in has ended"))
        } else {
            Ok(())
        }
    }

    fn end(&self) {
        let started = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.abandoned = true;
            state.started.clone()
        };
        for cli in started {
            cli.kill();
        }
    }
}

/// `GUATIAO_ERR_GONE` with `why`.
#[must_use]
pub fn gone(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_GONE, why)
}

/// A prompter's answer, with its refusal read as the person cancelling: `GONE` `login cancelled`.
///
/// # Errors
///
/// The prompter's refusal.
pub fn asked<T>(result: Result<T, ProviderError>) -> Result<T, ProviderError> {
    result.map_err(|error| {
        if error.status == Status::GUATIAO_ERR_GONE {
            gone("login cancelled")
        } else {
            error
        }
    })
}

/// How a sign-in's work reaches what it was handed.
pub type Work = Box<
    dyn FnOnce(
            &Arc<Object<dyn ProcessSpawner>>,
            &Object<dyn LoginPrompter>,
            &Job,
        ) -> Result<LoginOutcome, ProviderError>
        + Send,
>;

/// Runs `work` on a thread of its own and answers its outcome, or `GONE` `login cancelled` when the person cancels
/// and `GONE` `login timed out` when `timeout_ms` passes (each within a moment of it, whatever the work is waiting on),
/// after ending every command the work started. `spawner` and `prompter` are let go by the work before its own
/// answer; when the supervisor answers first, it gives the work a moment (one second) to let go of them, so a caller
/// told of the end usually finds them gone, and a work blocked in the host's prompter lets go when that call returns.
///
/// # Errors
///
/// What the work answers, or the two ends above.
pub fn supervise(
    spawner: Arc<Object<dyn ProcessSpawner>>,
    prompter: Object<dyn LoginPrompter>,
    timeout_ms: u64,
    work: Work,
) -> Result<LoginOutcome, ProviderError> {
    let prompter = Arc::new(prompter);
    let job = Arc::new(Job::default());
    let (sender, receiver) = mpsc::channel();
    let thread = {
        let prompter = Arc::clone(&prompter);
        let job = Arc::clone(&job);
        std::thread::Builder::new()
            .name("coqui-acp-login".into())
            .spawn(move || {
                let result = work(&spawner, &prompter, &job);
                // What the work was handed is let go before the answer, so a caller told of the end finds it so.
                drop(spawner);
                drop(prompter);
                let _ = sender.send(result);
            })
            .map_err(|_| {
                ProviderError::new(Status::GUATIAO_ERR_INTERNAL, "no thread to sign in on")
            })?
    };
    track(thread);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => {
                job.end();
                return result;
            }
            Err(RecvTimeoutError::Disconnected) => {
                job.end();
                return Err(ProviderError::new(
                    Status::GUATIAO_ERR_INTERNAL,
                    "the sign-in ended without an answer",
                ));
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        let ended = if prompter.cancelled().unwrap_or(true) {
            Some("login cancelled")
        } else if Instant::now() >= deadline {
            Some("login timed out")
        } else {
            None
        };
        if let Some(why) = ended {
            job.end();
            // The commands are ended, so the work is on its way out: give it a moment to let go of the spawner and
            // the prompter before answering. Its answer, if it makes one, is not ours.
            let _ = receiver.recv_timeout(LET_GO);
            drop(prompter);
            return Err(gone(why));
        }
    }
}

/// Whether `word` looks like an address: it has a scheme, or a host-and-path (`example.com/...`) or starts `www.`.
fn address_like(word: &str) -> bool {
    word.contains("://")
        || word.to_ascii_lowercase().starts_with("www.")
        || word
            .split_once('/')
            .is_some_and(|(host, _)| host.contains('.') && !host.starts_with('.'))
}

/// Whether `line` echoes `secret`, in whole or in part: all of it, or its first or last four characters or more.
fn echoes(line: &str, secret: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    if line.contains(secret) {
        return true;
    }
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() < 8 {
        return false;
    }
    let first: String = chars[..4].iter().collect();
    let last: String = chars[chars.len() - 4..].iter().collect();
    line.contains(&first) || line.contains(&last)
}

/// The reason a command refused, from what it printed: its last line, with addresses removed (with or without a
/// scheme) and nothing the person typed. `typed` is every value the person gave (a key, a code, an address); a line
/// that echoes any of them in whole or in part (their first or last four characters) is replaced.
#[must_use]
pub fn reason(output: &str, typed: &[&str]) -> String {
    let shown = plain(output);
    let line = shown
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default();
    let words: Vec<&str> = line
        .split_whitespace()
        .filter(|word| !address_like(word))
        .collect();
    let mut text = words.join(" ");
    if text.is_empty() || typed.iter().any(|secret| echoes(&text, secret)) {
        text = "it was not accepted".to_string();
    }
    if text.len() > 200 {
        let mut end = 200;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}
