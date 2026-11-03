//! Asking a harness a question that needs no model turn.
//!
//! A handler that can list the models an account may use starts its harness, writes a request,
//! reads the answer and stops it. [`ask`] does that for a harness that speaks JSON lines on its
//! standard input and output: the child gets exactly the environment it is given, the wait is
//! the caller's, and the child is killed when the question is over, answered or not.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A harness started for one question.
pub struct Harness {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    deadline: Instant,
}

impl Harness {
    /// Writes `value` as one JSON line.
    ///
    /// # Errors
    ///
    /// Why the harness did not take it.
    pub fn send(&mut self, value: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{value}")
            .and_then(|()| self.stdin.flush())
            .map_err(|error| format!("the harness did not take the request: {error}"))
    }

    /// Reads lines until one parses as JSON and `wanted` accepts it, and returns it.
    ///
    /// # Errors
    ///
    /// That the wait ran out, or the harness ended first.
    pub fn next_where(&mut self, wanted: impl Fn(&Value) -> bool) -> Result<Value, String> {
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    if let Ok(value) = serde_json::from_str::<Value>(line.trim())
                        && wanted(&value)
                    {
                        return Ok(value);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err("the harness did not answer in time".to_owned());
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("the harness ended without answering".to_owned());
                }
            }
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts `program` with `arguments` and exactly `environment`, within `wait` for all of
/// `question`, and returns what it returns.
///
/// # Errors
///
/// That the harness could not be started, or whatever `question` returns.
pub fn ask<T>(
    program: &str,
    arguments: &[&str],
    environment: &BTreeMap<String, String>,
    wait: Duration,
    question: impl FnOnce(&mut Harness) -> Result<T, String>,
) -> Result<T, String> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .env_clear()
        .envs(environment)
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|error| format!("{program} could not be started: {error}"))?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return Err(format!("{program} has no standard input or output"));
    };
    let (sender, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let mut harness = Harness {
        child,
        stdin,
        lines,
        deadline: Instant::now() + wait,
    };
    question(&mut harness)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_is_not_there_is_an_error_not_a_guess() {
        let result: Result<(), String> = ask(
            "ai6-no-such-harness",
            &[],
            &BTreeMap::new(),
            Duration::from_secs(5),
            |_| Ok(()),
        );
        assert!(result.unwrap_err().contains("could not be started"));
    }

    /// A harness for the tests below: the test binary itself, run for this one test.
    #[test]
    fn fake_harness() {
        let Ok(mode) = std::env::var("AI6_FAKE_LISTING_HARNESS") else {
            return;
        };
        let mut request = String::new();
        std::io::stdin().read_line(&mut request).unwrap();
        let request: Value = serde_json::from_str(&request).unwrap();
        let mut out = std::io::stdout().lock();
        // Noise before the answer, as a real harness writes banners and other messages.
        writeln!(out, "not json at all").unwrap();
        writeln!(out, "{}", serde_json::json!({"id": 99, "other": true})).unwrap();
        match mode.as_str() {
            "answer" => writeln!(
                out,
                "{}",
                serde_json::json!({
                    "id": request["id"],
                    "seen": std::env::var("AI6_LISTING_GIVEN").ok(),
                    "leaked": std::env::var("AI6_LISTING_PARENT").ok(),
                })
            )
            .unwrap(),
            _ => std::thread::sleep(Duration::from_secs(120)),
        }
        out.flush().unwrap();
    }

    fn fake(mode: &str, wait: Duration) -> Result<Value, String> {
        // SAFETY of the test: only this process sets it, and the child must not see it.
        let program = std::env::current_exe().unwrap();
        let environment = BTreeMap::from([
            ("AI6_FAKE_LISTING_HARNESS".to_owned(), mode.to_owned()),
            ("AI6_LISTING_GIVEN".to_owned(), "given".to_owned()),
            (
                "SYSTEMROOT".to_owned(),
                std::env::var("SYSTEMROOT").unwrap_or_default(),
            ),
        ]);
        ask(
            program.to_str().unwrap(),
            &[
                "--exact",
                "directorates::section_00::listing::tests::fake_harness",
                "--nocapture",
            ],
            &environment,
            wait,
            |harness| {
                harness.send(&serde_json::json!({"id": 7, "method": "list"}))?;
                harness.next_where(|line| line["id"] == 7)
            },
        )
    }

    #[test]
    fn the_answer_is_found_among_noise_and_the_child_sees_only_what_it_was_given() {
        let answer = fake("answer", Duration::from_secs(60)).unwrap();
        assert_eq!(answer["seen"], "given");
        assert_eq!(
            answer["leaked"],
            Value::Null,
            "nothing of the parent's environment"
        );
    }

    #[test]
    fn a_harness_that_never_answers_is_unknown_within_the_wait() {
        let started = Instant::now();
        let error = fake("silent", Duration::from_millis(800)).unwrap_err();
        assert!(error.contains("did not answer in time"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "and it is killed, not waited for"
        );
    }
}
