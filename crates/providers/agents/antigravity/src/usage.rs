//! What the Antigravity CLI reports of its account's quota, and how it is read with no turn.
//!
//! agy serves the quota on a loopback port of its own language server (see
//! [`coqui_acp_harness::antigravity::AgyQuota`]). A Session's agy may serve it already; when none
//! does, a private `agy acp` with an empty session is started under the account's own home, read
//! and ended.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ai6::{Bucket, UsageReport, UsageSource};
use coqui_acp_harness::antigravity::{AcpModel, AgyQuota, ReadError, ReadLimits};
use serde_json::{Value, json};

/// The agy processes of live Sessions, by the home directory their account lives in.
static LIVE: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());

fn live() -> MutexGuard<'static, Vec<(String, u32)>> {
    LIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The home directory an account's agy keeps its login under: what two environments share when
/// they are the same account on this host.
pub(crate) fn home_of(environment: &BTreeMap<String, String>) -> String {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|name| environment.get(*name))
        .cloned()
        .unwrap_or_default()
}

/// A Session's agy process, listed while it runs.
pub(crate) struct Live {
    home: String,
    pid: u32,
}

impl Live {
    /// Lists `pid` as an agy running under `environment`'s home.
    pub(crate) fn new(environment: &BTreeMap<String, String>, pid: i64) -> Option<Self> {
        let pid = u32::try_from(pid).ok().filter(|pid| *pid != 0)?;
        let home = home_of(environment);
        live().push((home.clone(), pid));
        Some(Self { home, pid })
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let mut listed = live();
        if let Some(at) = listed
            .iter()
            .position(|(home, pid)| *home == self.home && *pid == self.pid)
        {
            listed.remove(at);
        }
    }
}

/// The processes of live Sessions under `home`.
pub(crate) fn live_under(home: &str) -> Vec<u32> {
    live()
        .iter()
        .filter(|(held, _)| held == home)
        .map(|(_, pid)| *pid)
        .collect()
}

/// `roots` and every process below them, from a table of `(pid, parent)`.
pub(crate) fn with_descendants(table: &[(u32, u32)], roots: &[u32]) -> Vec<u32> {
    let mut found: Vec<u32> = roots.to_vec();
    let mut queue: VecDeque<u32> = roots.iter().copied().collect();
    while let Some(parent) = queue.pop_front() {
        for (pid, of) in table {
            if *of == parent && !found.contains(pid) {
                found.push(*pid);
                queue.push_back(*pid);
            }
        }
    }
    found
}

/// The process table as `ps` lists it; empty where there is none.
fn process_table() -> Vec<(u32, u32)> {
    let Ok(output) = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    parse_table(&String::from_utf8_lossy(&output.stdout))
}

pub(crate) fn parse_table(text: &str) -> Vec<(u32, u32)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        })
        .collect()
}

/// The report of `buckets`: reported figures, nothing of a session's spend.
pub(crate) fn report_of(buckets: Vec<Bucket>) -> UsageReport {
    let mut report = UsageReport::new();
    report.sources = vec![UsageSource::Reported];
    report.observed_at = buckets
        .iter()
        .map(|bucket| bucket.observed_at)
        .max()
        .unwrap_or(0);
    report.buckets = buckets;
    report
}

/// The quota read from the agy of a live Session of the account at `home`, when one serves it.
pub(crate) fn through_session(home: &str) -> Option<Vec<Bucket>> {
    let roots = live_under(home);
    if roots.is_empty() {
        return None;
    }
    let owners = with_descendants(&process_table(), &roots);
    AgyQuota::new(ReadLimits::default()).read(&owners, &[]).ok()
}

/// A line of a private agent's output, or the end of it.
fn lines_of(stdout: std::process::ChildStdout) -> Receiver<String> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    receiver
}

fn answer(lines: &Receiver<String>, id: u64, deadline: Instant) -> Result<Value, String> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left) {
            Ok(line) => {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if value.get("id").and_then(Value::as_u64) == Some(id) {
                    return match value.get("error") {
                        Some(error) => Err(format!("agy refused: {error}")),
                        None => Ok(value),
                    };
                }
            }
            Err(RecvTimeoutError::Timeout) => return Err("agy did not answer in time".to_owned()),
            Err(RecvTimeoutError::Disconnected) => {
                return Err("agy ended before it answered".to_owned());
            }
        }
    }
}

/// The models `session/new` listed.
pub(crate) fn models_of(answer: &Value) -> Vec<AcpModel> {
    answer
        .pointer("/result/models/availableModels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("modelId")?.as_str()?;
            let name = model.get("name").and_then(Value::as_str).unwrap_or(id);
            Some(AcpModel::new(id, name))
        })
        .collect()
}

/// Reads the quota through a private `agy acp` with an empty session, started with `environment`
/// and ended afterwards. `wait` bounds the whole read: agy's ports appear a few seconds after the
/// session starts.
pub(crate) fn through_private_agent(
    executable: &str,
    environment: &BTreeMap<String, String>,
    wait: Duration,
) -> Result<Vec<Bucket>, String> {
    let deadline = Instant::now() + wait;
    let directory: PathBuf = std::env::temp_dir();
    let mut child = Command::new(executable)
        .arg("acp")
        .env_clear()
        .envs(environment)
        .current_dir(&directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{executable} could not be started: {error}"))?;
    let result = read_started(&mut child, &directory, deadline);
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn read_started(
    child: &mut std::process::Child,
    directory: &std::path::Path,
    deadline: Instant,
) -> Result<Vec<Bucket>, String> {
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err("agy has no standard input or output".to_owned());
    };
    let lines = lines_of(stdout);
    let mut send = |value: Value| {
        let mut line = value.to_string();
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("agy could not be written to: {error}"))
    };
    send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": 1, "clientCapabilities": {}}}))?;
    answer(&lines, 1, deadline)?;
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
        "params": {"cwd": directory.display().to_string(), "mcpServers": []}}))?;
    let models = models_of(&answer(&lines, 2, deadline)?);
    let mut quota = AgyQuota::new(ReadLimits::default());
    let mut last = ReadError::NoPort;
    while Instant::now() < deadline {
        let owners = with_descendants(&process_table(), &[child.id()]);
        match quota.read(&owners, &models) {
            Ok(buckets) => return Ok(buckets),
            Err(error) => last = error,
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!("agy's quota was not read: {last}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendants_follow_the_parent_table_through_grandchildren() {
        let table = [(10, 1), (11, 10), (12, 11), (20, 1), (13, 10)];
        let mut family = with_descendants(&table, &[10]);
        family.sort_unstable();
        assert_eq!(family, [10, 11, 12, 13]);
        assert_eq!(with_descendants(&table, &[99]), [99]);
    }

    #[test]
    fn the_process_table_is_read_from_ps_output() {
        assert_eq!(
            parse_table("  10     1\n 11 10\nbad line\n"),
            [(10, 1), (11, 10)]
        );
    }

    #[test]
    fn a_live_session_is_listed_under_its_home_until_it_is_dropped() {
        let environment = BTreeMap::from([("HOME".to_owned(), "/tmp/ai6-usage-home".to_owned())]);
        let other = BTreeMap::from([("HOME".to_owned(), "/tmp/ai6-usage-other".to_owned())]);
        let held = Live::new(&environment, 4242).unwrap();
        assert_eq!(live_under("/tmp/ai6-usage-home"), [4242]);
        assert!(live_under(&home_of(&other)).is_empty());
        assert!(Live::new(&environment, 0).is_none(), "no process, no entry");
        drop(held);
        assert!(live_under("/tmp/ai6-usage-home").is_empty());
    }

    #[test]
    fn the_models_agy_lists_become_the_ones_a_bucket_is_matched_to() {
        let answer = json!({"result": {"models": {"availableModels": [
            {"modelId": "gemini-4-argon", "name": "Gemini 4 Argon"},
            {"modelId": "claude-x"}
        ]}}});
        let models = models_of(&answer);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0], AcpModel::new("gemini-4-argon", "Gemini 4 Argon"));
        assert_eq!(models[1], AcpModel::new("claude-x", "claude-x"));
        assert!(models_of(&json!({})).is_empty());
    }

    #[test]
    fn a_program_that_is_not_there_is_an_error_and_nothing_is_guessed() {
        let error =
            through_private_agent("ai6-no-such-agy", &BTreeMap::new(), Duration::from_secs(1))
                .unwrap_err();
        assert!(error.contains("could not be started"), "{error}");
    }
}
