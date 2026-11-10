//! Reading a harness's output on a thread of its own, until it ends, and saying how it ended once.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use std::io::{self, Read};
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;

use serde_json::Value;

use crate::event::{Event, Severity};
use crate::lines::{self, Line, Tail};
use crate::process::{Process, ending_once, exit_text, watch_exit};

/// A check of every line a harness writes, before anything reads it; `false` ends the harness.
pub type LineHook = Arc<dyn Fn(&Value) -> bool + Send + Sync>;

/// Where a session's events go, until it answers `false`.
pub type EventHook = Arc<dyn Fn(Event) -> bool + Send + Sync>;

/// The answer to one of our requests, by its id as text: its result, or why not.
pub type Answer = (String, Result<Value, String>);

/// The requests waiting for an answer, by id as text.
pub type Waiting = Arc<Mutex<BTreeMap<String, mpsc::Sender<Result<Value, String>>>>>;

/// What one line of a harness's output comes to.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Step {
    /// The events to tell, in order.
    pub events: Vec<Event>,
    /// The answers to our own requests it held.
    pub answers: Vec<Answer>,
    /// Why nothing more of this harness is to be read: it is ended, once `events` are told.
    pub stop: Option<String>,
}

impl Step {
    /// The events and answers of a line, going on.
    #[must_use]
    pub const fn new(events: Vec<Event>, answers: Vec<Answer>) -> Self {
        Self {
            events,
            answers,
            stop: None,
        }
    }

    /// The same, ending the harness for this reason once they are told.
    #[must_use]
    pub const fn stopping(events: Vec<Event>, answers: Vec<Answer>, why: String) -> Self {
        Self {
            events,
            answers,
            stop: Some(why),
        }
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The reader of one harness's output.
#[derive(Clone)]
#[non_exhaustive]
pub struct Stream {
    /// The harness's name in a failure, such as `Claude Code`.
    pub name: &'static str,
    /// Its process.
    pub process: Arc<dyn Process>,
    /// The longest line read.
    pub max_line: u64,
    /// How often the process is asked whether it has exited while its output stays open.
    pub exit_poll: Duration,
    /// How long, after an exit, what it wrote last is read for, and then how long the end of its output is waited for
    /// once what it left behind is ended; and how long the end of its output waits for its exit status.
    pub exit_wait: Duration,
    /// The last lines that were not protocol, which a failure quotes.
    pub tail: Arc<Mutex<Tail>>,
    /// Set once the session is closed on purpose: its end is not told.
    pub closed: Arc<AtomicBool>,
    /// Where events go.
    pub sink: EventHook,
    /// The requests waiting for an answer.
    pub waiting: Waiting,
    /// How it ended, once it has: a request made after that fails at once.
    pub ended: Arc<OnceLock<String>>,
    /// The host's check of each line, before anything reads it.
    pub on_line: Option<LineHook>,
}

impl core::fmt::Debug for Stream {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Stream")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Stream {
    /// A reader with the name, process, tail, flags and sink given, and the bounds to set.
    #[must_use]
    pub fn new(
        name: &'static str,
        process: Arc<dyn Process>,
        tail: Arc<Mutex<Tail>>,
        closed: Arc<AtomicBool>,
        sink: EventHook,
    ) -> Self {
        Self {
            name,
            process,
            max_line: 64 * 1024 * 1024,
            exit_poll: Duration::from_millis(100),
            exit_wait: Duration::from_secs(1),
            tail,
            closed,
            sink,
            waiting: Arc::new(Mutex::new(BTreeMap::new())),
            ended: Arc::new(OnceLock::new()),
            on_line: None,
        }
    }

    /// Reads `stdout` on a thread of its own, giving each JSON line to `line`, until the output ends or the process
    /// has exited with its output still open, as when something it started holds the pipe. Then every request still
    /// waiting fails, `last` gives what is left to tell, and [`Event::Exited`] follows, once, unless the session was
    /// closed or the sink stopped listening.
    ///
    /// # Errors
    ///
    /// When a thread could not start.
    pub fn read(
        self,
        stdout: Box<dyn Read + Send>,
        thread_name: &str,
        mut line: impl FnMut(&Value) -> Step + Send + 'static,
        last: impl Fn() -> Vec<Event> + Send + Sync + 'static,
    ) -> io::Result<()> {
        let open = Arc::new(AtomicBool::new(true));
        let stopped: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let failed_read: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let done = ending_once();
        // Lines started, and whether one is being told: a slow client holds the reader back, and that is not an end.
        let progress = Arc::new(AtomicU64::new(0));
        let telling = Arc::new(AtomicBool::new(false));
        let ending = Arc::new({
            let stream = self.clone();
            let open = Arc::clone(&open);
            let stopped = Arc::clone(&stopped);
            let failed_read = Arc::clone(&failed_read);
            move |left_open: bool| {
                let reason = locked(&stopped)
                    .take()
                    .unwrap_or_else(|| stream.reason(locked(&failed_read).take(), left_open));
                let _ = stream.ended.set(reason.clone());
                for (_, waiter) in core::mem::take(&mut *locked(&stream.waiting)) {
                    let _ = waiter.send(Err(reason.clone()));
                }
                if stream.closed.load(Ordering::SeqCst) || !open.load(Ordering::SeqCst) {
                    return;
                }
                for event in last() {
                    if !(stream.sink)(event) {
                        return;
                    }
                }
                (stream.sink)(Event::Exited(reason));
            }
        });
        {
            let ending = Arc::clone(&ending);
            let progress = Arc::clone(&progress);
            let telling = Arc::clone(&telling);
            let quiet = self.exit_wait.max(Duration::from_millis(1));
            watch_exit(
                Arc::clone(&self.process),
                self.exit_poll,
                self.exit_wait,
                Arc::clone(&done),
                move || {
                    loop {
                        let before = progress.load(Ordering::SeqCst);
                        thread::sleep(quiet);
                        if !telling.load(Ordering::SeqCst)
                            && progress.load(Ordering::SeqCst) == before
                        {
                            break;
                        }
                    }
                    ending(true);
                },
            )?;
        }
        let stream = self;
        thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || {
                let read = lines::read_lines(stdout, stream.max_line, |read| {
                    progress.fetch_add(1, Ordering::SeqCst);
                    telling.store(true, Ordering::SeqCst);
                    let going = {
                        match read {
                            Line::Json(value) => {
                                if let Some(check) = &stream.on_line
                                    && !check(&value)
                                {
                                    *locked(&stopped) = Some(format!(
                                        "{} wrote a line the host refused",
                                        stream.name
                                    ));
                                    let _ = stream.process.kill();
                                    telling.store(false, Ordering::SeqCst);
                                    return false;
                                }
                                let step = line(&value);
                                for (id, answer) in step.answers {
                                    let waiter = locked(&stream.waiting).remove(&id);
                                    match (waiter, answer) {
                                        (Some(waiter), answer) => {
                                            let _ = waiter.send(answer);
                                        }
                                        // Nobody waits for an interrupt's answer: a refusal is still told.
                                        (None, Err(why)) => {
                                            let told = (stream.sink)(Event::Notice {
                                                severity: Severity::Warning,
                                                title: why,
                                            });
                                            if !told {
                                                open.store(false, Ordering::SeqCst);
                                            }
                                        }
                                        (None, Ok(_)) => {}
                                    }
                                }
                                for event in step.events {
                                    if open.load(Ordering::SeqCst) && !(stream.sink)(event) {
                                        open.store(false, Ordering::SeqCst);
                                    }
                                }
                                if let Some(why) = step.stop {
                                    *locked(&stopped) = Some(why);
                                    let _ = stream.process.kill();
                                    telling.store(false, Ordering::SeqCst);
                                    return false;
                                }
                            }
                            Line::Text(text) => {
                                locked(&stream.tail).push(&text);
                                if open.load(Ordering::SeqCst)
                                    && !(stream.sink)(Event::Output(text))
                                {
                                    open.store(false, Ordering::SeqCst);
                                }
                            }
                            Line::TooLong(bytes) => {
                                locked(&stream.tail).push(&format!(
                                    "a line of {bytes} bytes or more, past the limit"
                                ));
                                let _ = stream.process.kill();
                            }
                        }
                        open.load(Ordering::SeqCst)
                    };
                    telling.store(false, Ordering::SeqCst);
                    going
                });
                if let Err(error) = read {
                    *locked(&failed_read) = Some(error.to_string());
                }
                if !done.swap(true, Ordering::SeqCst) {
                    ending(false);
                }
            })?;
        Ok(())
    }

    fn reason(&self, failed_read: Option<String>, left_open: bool) -> String {
        let how = exit_text(self.process.as_ref(), self.exit_wait);
        let mut reason = format!("{} {how} before completing the session", self.name);
        if left_open {
            reason.push_str(" (something it started kept its output open)");
        }
        if let Some(error) = failed_read {
            reason.push_str(&format!(" (its output could not be read: {error})"));
        }
        let tail = locked(&self.tail);
        if !tail.is_empty() {
            reason.push_str("; its last output:\n");
            reason.push_str(&tail.text());
        }
        reason
    }
}
