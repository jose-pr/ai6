//! A deadline for a test that waits on threads: one that would hang ends the test binary with a message instead.

use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

/// Ends the deadline when dropped.
pub struct Deadline(#[allow(dead_code)] Sender<()>);

/// Exits the process with a message when the returned guard outlives `seconds`.
pub fn within(seconds: u64) -> Deadline {
    let (stop, stopped) = channel::<()>();
    let name = thread::current().name().unwrap_or("a test").to_owned();
    thread::spawn(move || {
        if stopped.recv_timeout(Duration::from_secs(seconds)) == Err(RecvTimeoutError::Timeout) {
            eprintln!("DEADLINE: {name} did not finish within {seconds} seconds");
            std::process::exit(101);
        }
    });
    Deadline(stop)
}
