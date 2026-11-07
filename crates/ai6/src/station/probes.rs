//! Read-only probes of a Mission's deliverables: what a verifier looks at instead of an Agent's
//! account of its own work.
//!
//! A probe reports what it read and never whether that is enough. Each one is bounded by a
//! `[probes]` timeout, and what it cannot read is `failed`, never `missing`: a remote that did
//! not answer says nothing about the branch.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::directorates::registry::approvals::{ApprovalStatus, Decision};
use crate::directorates::registry::sessions::ProbeResult;

/// What a probe found, and the line that says what it read.
pub(crate) type Found = (ProbeResult, String);

/// Runs `work` on its own thread and gives up on it after `timeout`. The thread is left to
/// finish: what it holds is a request, not a lock.
pub(crate) fn within<T: Send + 'static>(
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(work());
    });
    receive.recv_timeout(timeout).ok()
}

/// A ref or branch name safe to hand to `git` as an argument: it names a branch and cannot be
/// taken for an option or a revision expression.
fn is_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.contains("..")
        && !name
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
}

/// The outcome of one `git` run: its exit status code, what it wrote, and why it failed to run.
struct GitRun {
    code: Option<i32>,
    out: String,
    err: String,
}

/// Reads `stream` to its end on a thread of its own, so a full pipe never stops the child.
fn drain<R: std::io::Read + Send + 'static>(stream: Option<R>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stream {
            let _ = stream.read_to_string(&mut text);
        }
        text
    })
}

/// Runs `git -C dir args`, killing it at `timeout`. It never prompts for a credential.
fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<GitRun, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git could not run: {error}"))?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git took longer than {}s", timeout.as_secs_f64()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(format!("git could not be waited for: {error}")),
        }
    };
    Ok(GitRun {
        code: status.code(),
        out: out.join().unwrap_or_default(),
        err: err.join().unwrap_or_default(),
    })
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default().trim()
}

/// The commit `branch` points to on `origin` of the repository at `repo`: `Ok(None)` when the
/// remote has no such branch.
fn remote_tip(repo: &Path, branch: &str, timeout: Duration) -> Result<Option<String>, String> {
    if !is_ref_name(branch) {
        return Err(format!("{branch:?} is not a branch name"));
    }
    let reference = format!("refs/heads/{branch}");
    let run = git(
        repo,
        &["ls-remote", "--exit-code", "--heads", "origin", &reference],
        timeout,
    )?;
    match run.code {
        Some(0) => Ok(run
            .out
            .split_whitespace()
            .next()
            .map(str::to_owned)
            .or(Some(String::new()))),
        // `--exit-code` answers 2 for a remote that answered and has no such ref.
        Some(2) => Ok(None),
        _ => Err(format!("git ls-remote: {}", first_line(&run.err))),
    }
}

/// A branch on the remote.
pub(crate) fn branch(repo: &Path, branch: &str, timeout: Duration) -> Found {
    match remote_tip(repo, branch, timeout) {
        Ok(Some(tip)) => (
            ProbeResult::Found,
            format!("branch {branch} is on the remote at {}", short(&tip)),
        ),
        Ok(None) => (
            ProbeResult::Missing,
            format!("branch {branch} is not on the remote"),
        ),
        Err(error) => (ProbeResult::Failed, error),
    }
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// A commit on a branch of the remote: `commit` itself, or when none is named the one the
/// working directory `work` is at. The branch's objects are fetched to read its history.
pub(crate) fn commit_on(
    repo: &Path,
    work: Option<&Path>,
    (branch, commit): (&str, Option<&str>),
    timeout: Duration,
) -> Found {
    let tip = match remote_tip(repo, branch, timeout) {
        Ok(Some(tip)) => tip,
        Ok(None) => {
            return (
                ProbeResult::Missing,
                format!("branch {branch} is not on the remote"),
            );
        }
        Err(error) => return (ProbeResult::Failed, error),
    };
    let commit = match (commit, work) {
        (Some(commit), _) => commit.to_owned(),
        (None, Some(work)) => match git(work, &["rev-parse", "HEAD"], timeout) {
            Ok(run) if run.code == Some(0) => first_line(&run.out).to_owned(),
            Ok(run) => {
                return (
                    ProbeResult::Failed,
                    format!("git rev-parse: {}", first_line(&run.err)),
                );
            }
            Err(error) => return (ProbeResult::Failed, error),
        },
        (None, None) => {
            return (
                ProbeResult::Unprobed,
                "the Mission's commit is not readable from this Station: name it as BRANCH@COMMIT"
                    .to_owned(),
            );
        }
    };
    if !is_ref_name(&commit) || commit.len() < 7 {
        return (ProbeResult::Failed, format!("{commit:?} is not a commit"));
    }
    if tip.starts_with(&commit) || (!tip.is_empty() && commit.starts_with(&tip)) {
        return (
            ProbeResult::Found,
            format!("{} is the tip of {branch}", short(&tip)),
        );
    }
    let reference = format!("refs/heads/{branch}");
    match git(
        repo,
        &["fetch", "--quiet", "--no-tags", "origin", &reference],
        timeout,
    ) {
        Ok(run) if run.code == Some(0) => {}
        Ok(run) => {
            return (
                ProbeResult::Failed,
                format!("git fetch: {}", first_line(&run.err)),
            );
        }
        Err(error) => return (ProbeResult::Failed, error),
    }
    match git(
        repo,
        &["merge-base", "--is-ancestor", &commit, "FETCH_HEAD"],
        timeout,
    ) {
        Ok(run) if run.code == Some(0) => (
            ProbeResult::Found,
            format!("{} is in the history of {branch}", short(&commit)),
        ),
        Ok(run) if run.code == Some(1) => (
            ProbeResult::Missing,
            format!("{} is not in the history of {branch}", short(&commit)),
        ),
        Ok(run) => (
            ProbeResult::Failed,
            format!("git merge-base: {}", first_line(&run.err)),
        ),
        Err(error) => (ProbeResult::Failed, error),
    }
}

/// A file inside `work`, named relative to it. A path that leaves the directory, by `..` or a
/// link, is not read.
pub(crate) fn file(work: &Path, relative: &str) -> Found {
    let path = Path::new(relative);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return (
            ProbeResult::Failed,
            format!("{relative:?} is not a path inside the working directory"),
        );
    }
    let joined: PathBuf = work.join(path);
    match (joined.canonicalize(), work.canonicalize()) {
        (Ok(found), Ok(root)) if found.starts_with(&root) => match found.metadata() {
            Ok(meta) if meta.is_file() => (
                ProbeResult::Found,
                format!("{relative} is a file of {} bytes", meta.len()),
            ),
            Ok(_) => (ProbeResult::Found, format!("{relative} is a directory")),
            Err(error) => (ProbeResult::Failed, format!("{relative}: {error}")),
        },
        (Ok(_), Ok(_)) => (
            ProbeResult::Failed,
            format!("{relative:?} leads out of the working directory"),
        ),
        (Err(error), _) if error.kind() == std::io::ErrorKind::NotFound => (
            ProbeResult::Missing,
            format!("{relative} is not in the working directory"),
        ),
        (Err(error), _) | (_, Err(error)) => (ProbeResult::Failed, format!("{relative}: {error}")),
    }
}

/// A page or file at `url`, asked for with GET and its body left unread.
pub(crate) fn artifact(url: &str, timeout: Duration) -> Found {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return (
            ProbeResult::Failed,
            format!("{url:?} is not an http or https address"),
        );
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .into();
    match agent.get(url).call() {
        Ok(response) => {
            let status = response.status().as_u16();
            match status {
                200..=299 => (ProbeResult::Found, format!("{url} answered {status}")),
                404 | 410 => (ProbeResult::Missing, format!("{url} answered {status}")),
                _ => (ProbeResult::Failed, format!("{url} answered {status}")),
            }
        }
        Err(error) => (ProbeResult::Failed, format!("{url}: {error}")),
    }
}

/// A green ink request decided for the Mission in `statuses`: approved, and not withdrawn. With
/// `action`, one for that kind of step.
pub(crate) fn approval(statuses: &[ApprovalStatus], action: Option<&str>) -> Found {
    let wanted = |status: &&ApprovalStatus| {
        action.is_none_or(|action| status.request().scope.kind == action)
    };
    let mut asked = 0;
    for status in statuses.iter().filter(wanted) {
        asked += 1;
        if let ApprovalStatus::Decided(decided) = status
            && decided.decision == Decision::Approved
            && decided.revoked.is_none()
        {
            return (
                ProbeResult::Found,
                format!(
                    "approved by {} for {} {}",
                    decided.signer, decided.request.scope.kind, decided.request.scope.target
                ),
            );
        }
    }
    (
        ProbeResult::Missing,
        match (asked, action) {
            (0, Some(action)) => format!("no green ink request for {action} was made"),
            (0, None) => "no green ink request was made".to_owned(),
            (_, _) => "no request was approved".to_owned(),
        },
    )
}

/// A comment of `bodies`, the latest ones of a tracker item, that carries `text`.
pub(crate) fn comment(item: &str, bodies: &[String], text: &str) -> Found {
    if bodies.iter().any(|body| body.contains(text)) {
        (
            ProbeResult::Found,
            format!("a comment on {item} carries the text"),
        )
    } else {
        (
            ProbeResult::Missing,
            format!(
                "none of the latest {} comments on {item} carries the text",
                bodies.len()
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(20);

    fn run(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    }

    /// A clone with one commit pushed to a bare `origin`, and a branch `feature` pushed too.
    fn repository() -> (tempfile::TempDir, PathBuf, String) {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("origin.git");
        let work = root.path().join("work");
        std::fs::create_dir_all(&bare).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        run(&bare, &["init", "--bare", "--initial-branch=main"]);
        run(&work, &["init", "--initial-branch=main"]);
        run(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        std::fs::write(work.join("a.txt"), "one").unwrap();
        run(&work, &["add", "."]);
        run(&work, &["commit", "-m", "one"]);
        run(&work, &["push", "origin", "main"]);
        run(&work, &["branch", "feature"]);
        run(&work, &["push", "origin", "feature"]);
        std::fs::write(work.join("b.txt"), "two").unwrap();
        run(&work, &["add", "."]);
        run(&work, &["commit", "-m", "two"]);
        let head = git(&work, &["rev-parse", "HEAD"], WAIT).unwrap().out;
        (root, work, head.trim().to_owned())
    }

    #[test]
    fn a_branch_on_the_remote_is_found_and_one_that_is_not_is_missing_by_name() {
        let (_root, work, _) = repository();
        let (found, detail) = branch(&work, "feature", WAIT);
        assert_eq!(found, ProbeResult::Found, "{detail}");
        let (missing, detail) = branch(&work, "never-pushed", WAIT);
        assert_eq!(missing, ProbeResult::Missing);
        assert!(detail.contains("never-pushed"), "{detail}");
        let (bad, _) = branch(&work, "--upload-pack=x", WAIT);
        assert_eq!(bad, ProbeResult::Failed);
    }

    #[test]
    fn an_unreachable_remote_is_failed_not_missing() {
        let root = tempfile::tempdir().unwrap();
        run(root.path(), &["init"]);
        run(
            root.path(),
            &["remote", "add", "origin", "/no/such/place.git"],
        );
        let (result, _) = branch(root.path(), "main", WAIT);
        assert_eq!(result, ProbeResult::Failed);
    }

    #[test]
    fn a_commit_is_on_a_branch_only_when_its_history_has_it() {
        let (_root, work, head) = repository();
        // HEAD ("two") was never pushed: it is on no remote branch.
        let (result, detail) = commit_on(&work, Some(&work), ("main", None), WAIT);
        assert_eq!(result, ProbeResult::Missing, "{detail}");
        run(&work, &["push", "origin", "main"]);
        let (result, detail) = commit_on(&work, Some(&work), ("main", None), WAIT);
        assert_eq!(result, ProbeResult::Found, "{detail}");
        let (result, _) = commit_on(&work, None, ("main", Some(&head)), WAIT);
        assert_eq!(result, ProbeResult::Found);
        let (result, _) = commit_on(&work, None, ("main", None), WAIT);
        assert_eq!(result, ProbeResult::Unprobed);
        let (result, _) = commit_on(&work, None, ("gone", Some(&head)), WAIT);
        assert_eq!(result, ProbeResult::Missing);
    }

    #[test]
    fn a_file_is_read_inside_the_working_directory_only() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("docs")).unwrap();
        std::fs::write(root.path().join("docs/report.md"), "hello").unwrap();
        let (found, detail) = file(root.path(), "docs/report.md");
        assert_eq!(found, ProbeResult::Found, "{detail}");
        assert_eq!(file(root.path(), "docs/none.md").0, ProbeResult::Missing);
        for escape in ["../x", "/etc/passwd", "docs/../../x"] {
            assert_eq!(file(root.path(), escape).0, ProbeResult::Failed, "{escape}");
        }
    }

    #[test]
    fn an_address_that_is_not_http_is_not_asked() {
        let (result, _) = artifact("file:///etc/passwd", WAIT);
        assert_eq!(result, ProbeResult::Failed);
    }

    #[test]
    fn an_artifact_is_found_missing_or_unread_by_what_the_server_answers() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(3) {
                let mut stream = stream.unwrap();
                let mut buffer = [0u8; 1024];
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let status = if request.starts_with("GET /there") {
                    "200 OK"
                } else if request.starts_with("GET /gone") {
                    "404 Not Found"
                } else {
                    "500 Internal Server Error"
                };
                let _ = std::io::Write::write_all(
                    &mut stream,
                    format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                        .as_bytes(),
                );
            }
        });
        let base = format!("http://{address}");
        assert_eq!(
            artifact(&format!("{base}/there"), WAIT).0,
            ProbeResult::Found
        );
        assert_eq!(
            artifact(&format!("{base}/gone"), WAIT).0,
            ProbeResult::Missing
        );
        assert_eq!(
            artifact(&format!("{base}/broken"), WAIT).0,
            ProbeResult::Failed
        );
    }

    #[test]
    fn a_probe_that_runs_past_its_timeout_is_given_up_on() {
        let slow = within(Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(2));
            1
        });
        assert_eq!(slow, None);
        assert_eq!(within(Duration::from_secs(5), || 1), Some(1));
    }

    #[test]
    fn a_comment_is_found_by_its_text() {
        let bodies = vec!["hello".to_owned(), "shipped as v1".to_owned()];
        assert_eq!(comment("t:1", &bodies, "shipped").0, ProbeResult::Found);
        assert_eq!(comment("t:1", &bodies, "reverted").0, ProbeResult::Missing);
    }

    #[test]
    fn no_request_is_no_approval() {
        let (result, detail) = approval(&[], None);
        assert_eq!(result, ProbeResult::Missing);
        assert!(detail.contains("no green ink request"), "{detail}");
    }
}
