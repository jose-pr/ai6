// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A Mission's Git worktree: creating, re-attaching, listing its commits, removing it.
//! Every Git call runs the `git` command with a deadline and without terminal prompts.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use ai6::directorates::q::safehouse::{Change, SafeHouseReport, SafeHouseSpec};

const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// A rejected Git operation, as text for the provider error.
pub(crate) type Failure = String;

/// An open safe house: a worktree of `repository`, or `repository` itself when not isolated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Worktree {
    repository: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) branch: Option<String>,
    pub(crate) base: String,
    pub(crate) isolated: bool,
}

impl Worktree {
    /// Creates, re-attaches or adopts the safe house `spec` describes.
    pub(crate) fn open(spec: &SafeHouseSpec) -> Result<Self, Failure> {
        match (&spec.reopen, spec.isolate) {
            (None, true) => Self::create(spec),
            (Some(report), true) => Self::reopen(spec, report),
            (reopen, false) => Self::in_place(spec, reopen.as_ref()),
        }
    }

    fn create(spec: &SafeHouseSpec) -> Result<Self, Failure> {
        let repository = checkout(&spec.source)?;
        let root = Path::new(&spec.state_root);
        fs::create_dir_all(root).map_err(|error| fail(root, error))?;
        let root = git_path(root.canonicalize().map_err(|error| fail(root, error))?);
        if root.starts_with(&repository) {
            return Err(format!(
                "safe-house root {} must be outside Operation checkout {}",
                root.display(),
                repository.display()
            ));
        }

        let branch = branch_name(
            &spec.branch,
            &spec.mission_id,
            &spec.title,
            &spec.operation_id,
        );
        git(&repository, &["check-ref-format", "--branch", &branch])?;
        let commit = git(
            &repository,
            &[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", spec.base),
            ],
        )?
        .trim()
        .to_owned();
        if commit.is_empty() {
            return Err(format!("Git did not resolve base {:?}", spec.base));
        }

        let path = root
            .join("safe-houses")
            .join(path_segment(&spec.operation_id))
            .join(path_segment(&spec.mission_id));
        if path.exists() {
            return Err(format!("safe-house path {} already exists", path.display()));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| fail(parent, error))?;
        }
        git(
            &repository,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                path.to_string_lossy().as_ref(),
                &commit,
            ],
        )?;
        Ok(Self {
            repository,
            path,
            branch: Some(branch),
            base: commit,
            isolated: true,
        })
    }

    fn reopen(spec: &SafeHouseSpec, report: &SafeHouseReport) -> Result<Self, Failure> {
        if !report.kept {
            return Err("safe-house report is not active".to_owned());
        }
        let (Some(branch), Some(base)) = (&report.branch, &report.base) else {
            return Err("safe-house report has no branch or base".to_owned());
        };
        let repository = checkout(&spec.source)?;
        let path = git_path(
            Path::new(&report.path)
                .canonicalize()
                .map_err(|error| fail(Path::new(&report.path), error))?,
        );
        if !path.is_dir() {
            return Err("safe-house worktree is not a directory".to_owned());
        }
        let top = PathBuf::from(git(&path, &["rev-parse", "--show-toplevel"])?.trim())
            .canonicalize()
            .map_err(|error| format!("safe-house worktree root: {error}"))?;
        if git_path(top) != path {
            return Err("safe-house report path does not match its Git worktree".to_owned());
        }
        let worktrees = git(&repository, &["worktree", "list", "--porcelain"])?;
        let registered = worktrees.lines().any(|line| {
            line.strip_prefix("worktree ")
                .and_then(|value| Path::new(value).canonicalize().ok())
                .is_some_and(|value| git_path(value) == path)
        });
        if !registered {
            return Err("safe-house is not registered with the Operation repository".to_owned());
        }
        let current = git(&path, &["symbolic-ref", "--short", "HEAD"])?;
        if current.trim() != branch {
            return Err("safe-house branch does not match its Mission report".to_owned());
        }
        verify_base(&path, base)?;
        Ok(Self {
            repository,
            path,
            branch: Some(branch.clone()),
            base: base.clone(),
            isolated: true,
        })
    }

    /// Works in the source checkout itself. The base is the commit checked out now, or the
    /// one a re-attached report recorded.
    fn in_place(spec: &SafeHouseSpec, reopen: Option<&SafeHouseReport>) -> Result<Self, Failure> {
        let path = checkout(&spec.source)?;
        let base = match reopen.and_then(|report| report.base.as_deref()) {
            Some(base) => {
                verify_base(&path, base)?;
                base.to_owned()
            }
            None => git(&path, &["rev-parse", "--verify", "HEAD^{commit}"])?
                .trim()
                .to_owned(),
        };
        Ok(Self {
            repository: path.clone(),
            path,
            branch: None,
            base,
            isolated: false,
        })
    }

    /// Removes an untouched worktree; retains it when there are commits or changes.
    pub(crate) fn finish(&self) -> Result<SafeHouseReport, Failure> {
        if !self.path.exists() {
            return Ok(self.report(false, Vec::new()));
        }
        let commits = self.summaries()?;
        if !self.isolated {
            return Ok(self.report(true, commits));
        }
        let dirty = !git(&self.path, &["status", "--porcelain"])?
            .trim()
            .is_empty();
        if !commits.is_empty() || dirty {
            return Ok(self.report(true, commits));
        }
        git(
            &self.repository,
            &[
                "worktree",
                "remove",
                "--force",
                self.path.to_string_lossy().as_ref(),
            ],
        )?;
        if let Some(branch) = &self.branch {
            let _ = git(&self.repository, &["branch", "-D", branch]);
        }
        Ok(self.report(false, Vec::new()))
    }

    /// One-line summaries of the commits made since the base, newest first.
    fn summaries(&self) -> Result<Vec<String>, Failure> {
        let output = git(
            &self.path,
            &["log", "--format=%h %s", &format!("{}..HEAD", self.base)],
        )?;
        Ok(output.lines().map(str::to_owned).collect())
    }

    /// Full commit identity and messages since the base, newest first.
    pub(crate) fn changes(&self) -> Result<Vec<Change>, Failure> {
        let format = "%H%x1f%an <%ae>%x1f%cn <%ce>%x1f%B%x1e";
        let format_arg = format!("--format={format}");
        let range = format!("{}..HEAD", self.base);
        let output = git(&self.path, &["log", &format_arg, &range])?;
        Ok(output
            .split('\u{1e}')
            .filter_map(|record| {
                let fields = record
                    .trim_matches('\n')
                    .split('\u{1f}')
                    .collect::<Vec<_>>();
                let [id, author, committer, message] = fields.as_slice() else {
                    return None;
                };
                Some(Change {
                    id: (*id).to_owned(),
                    author: (*author).to_owned(),
                    committer: (*committer).to_owned(),
                    message: (*message).to_owned(),
                })
            })
            .collect())
    }

    fn report(&self, kept: bool, changes: Vec<String>) -> SafeHouseReport {
        SafeHouseReport {
            path: self.path.to_string_lossy().into_owned(),
            branch: self.branch.clone(),
            base: Some(self.base.clone()),
            kept,
            changes,
        }
    }
}

/// The canonical top of the Git work tree at `source`.
fn checkout(source: &str) -> Result<PathBuf, Failure> {
    let source = Path::new(source);
    let repository = git_path(source.canonicalize().map_err(|error| fail(source, error))?);
    let inside = git(&repository, &["rev-parse", "--is-inside-work-tree"])?;
    if inside.trim() != "true" {
        return Err(format!("{} is not a Git checkout", repository.display()));
    }
    Ok(repository)
}

fn verify_base(path: &Path, base: &str) -> Result<(), Failure> {
    let resolved = git(
        path,
        &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?;
    if resolved.trim() != base {
        return Err("safe-house base commit does not match its Mission report".to_owned());
    }
    git(path, &["merge-base", "--is-ancestor", base, "HEAD"])?;
    Ok(())
}

/// Fills `{id}`, `{slug}` and `{operation}` and removes characters Git rejects in refs.
pub(crate) fn branch_name(
    template: &str,
    mission_id: &str,
    title: &str,
    operation: &str,
) -> String {
    let slug = slug(title);
    let expanded = template
        .replace("{id}", mission_id)
        .replace("{slug}", &slug)
        .replace("{operation}", operation);
    let mut safe = String::with_capacity(expanded.len());
    let mut previous_dash = false;
    for character in expanded.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '/' | '-') {
            safe.push(character);
            previous_dash = false;
        } else if !previous_dash {
            safe.push('-');
            previous_dash = true;
        }
    }
    let safe = safe.replace("..", ".");
    let safe = safe.trim_matches(['-', '.', '/']).to_owned();
    if safe.is_empty() {
        "mission".to_owned()
    } else {
        safe
    }
}

fn slug(title: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = true;
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() {
        "mission".to_owned()
    } else {
        slug
    }
}

/// A directory name for `value`: safe characters plus a hash, so distinct IDs never collide.
fn path_segment(value: &str) -> String {
    let mut segment = String::new();
    for character in value.chars() {
        let safe = if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            character
        } else {
            '-'
        };
        if segment.len() + safe.len_utf8() > 72 {
            break;
        }
        segment.push(safe);
    }
    let segment = segment.trim_matches('-');
    let stem = if segment.is_empty() { "id" } else { segment };
    let hash = value.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{stem}-{hash:016x}")
}

pub(crate) fn git(cwd: &Path, args: &[&str]) -> Result<String, Failure> {
    let name = args.first().copied().unwrap_or("command");
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git {name}: {error}"))?;
    let stdout_pipe = child
        .stdout
        .take()
        .ok_or("git stdout pipe was unavailable")?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or("git stderr pipe was unavailable")?;
    let stdout_reader = thread::spawn(move || read_pipe(stdout_pipe));
    let stderr_reader = thread::spawn(move || read_pipe(stderr_pipe));
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!(
                    "git {name} timed out after {} seconds",
                    GIT_TIMEOUT.as_secs()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("git {name}: {error}"));
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| "git stdout reader panicked".to_owned())?
        .map_err(|error| format!("git stdout: {error}"))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "git stderr reader panicked".to_owned())?
        .map_err(|error| format!("git stderr: {error}"))?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
        return Err(format!("git {name} failed in {}: {stderr}", cwd.display()));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    pipe.read_to_end(&mut output)?;
    Ok(output)
}

fn fail(path: &Path, error: std::io::Error) -> Failure {
    format!("{}: {error}", path.display())
}

/// Drops the verbatim prefix `canonicalize` adds on Windows, which Git does not accept.
fn git_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(path) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(path);
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "ai6-safehouse-git-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn git(&self, cwd: &Path, args: &[&str]) {
            git(cwd, args).unwrap();
        }

        fn repo(&self) -> PathBuf {
            let repo = self.0.join("repo");
            fs::create_dir_all(&repo).unwrap();
            self.git(&repo, &["init", "--quiet"]);
            self.git(&repo, &["config", "user.name", "AI6 Tests"]);
            self.git(&repo, &["config", "user.email", "ai6@example.invalid"]);
            fs::write(repo.join("seed.txt"), "seed\n").unwrap();
            self.git(&repo, &["add", "seed.txt"]);
            self.git(&repo, &["commit", "--quiet", "-m", "seed"]);
            repo
        }

        fn spec(&self, repo: &Path, mission: &str, branch: &str) -> SafeHouseSpec {
            SafeHouseSpec {
                operation_id: "G".to_owned(),
                mission_id: mission.to_owned(),
                title: "Inspect the service".to_owned(),
                source: repo.to_string_lossy().into_owned(),
                base: "HEAD".to_owned(),
                branch: branch.to_owned(),
                isolate: true,
                state_root: self.0.to_string_lossy().into_owned(),
                reopen: None,
            }
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn branch_templates_are_safe_git_refs() {
        assert_eq!(
            branch_name("{operation}/{id}-{slug}", "x..y", "Fix the build!", "G"),
            "G/x.y-fix-the-build"
        );
        assert_eq!(branch_name("{slug}", "1", "***", "G"), "mission");
    }

    #[test]
    fn untouched_worktree_and_branch_are_removed() {
        let temp = Temp::new();
        let repo = temp.repo();
        let house =
            Worktree::open(&temp.spec(&repo, "mission-1", "{operation}/{id}-{slug}")).unwrap();
        assert!(house.path.join("seed.txt").exists());
        let report = house.finish().unwrap();
        assert!(!report.kept);
        assert!(!house.path.exists());
        let branch = house.branch.as_deref().unwrap();
        assert!(
            !git(&repo, &["branch", "--list", branch])
                .unwrap()
                .contains(branch)
        );
    }

    #[test]
    fn modified_worktree_is_kept_and_reported() {
        let temp = Temp::new();
        let repo = temp.repo();
        let house = Worktree::open(&temp.spec(&repo, "mission-2", "{id}-{slug}")).unwrap();
        fs::write(house.path.join("work.txt"), "work\n").unwrap();
        let report = house.finish().unwrap();
        assert!(report.kept);
        assert!(house.path.exists());
        assert!(report.changes.is_empty());
    }

    #[test]
    fn committed_worktree_is_kept_with_its_commit_summary() {
        let temp = Temp::new();
        let repo = temp.repo();
        let house = Worktree::open(&temp.spec(&repo, "mission-commit", "{id}-{slug}")).unwrap();
        fs::write(house.path.join("work.txt"), "work\n").unwrap();
        temp.git(&house.path, &["add", "work.txt"]);
        temp.git(&house.path, &["commit", "--quiet", "-m", "add work"]);
        let changes = house.changes().unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].author, "AI6 Tests <ai6@example.invalid>");
        assert_eq!(changes[0].committer, "AI6 Tests <ai6@example.invalid>");
        assert_eq!(changes[0].message.trim(), "add work");
        let report = house.finish().unwrap();
        assert!(report.kept);
        assert_eq!(report.changes.len(), 1);
        assert!(report.changes[0].ends_with(" add work"));
        assert!(house.path.exists());
    }

    #[test]
    fn refuses_to_put_safe_houses_inside_the_checkout_or_reuse_a_path() {
        let temp = Temp::new();
        let repo = temp.repo();
        let mut inside = temp.spec(&repo, "mission-3", "{id}-{slug}");
        inside.state_root = repo.join(".ai6-state").to_string_lossy().into_owned();
        assert!(Worktree::open(&inside).is_err());
        let spec = temp.spec(&repo, "mission-3", "{id}-{slug}");
        let house = Worktree::open(&spec).unwrap();
        assert!(Worktree::open(&spec).is_err());
        house.finish().unwrap();
    }

    #[test]
    fn a_kept_worktree_is_reattached_from_its_report() {
        let temp = Temp::new();
        let repo = temp.repo();
        let spec = temp.spec(&repo, "mission-4", "{operation}/{id}-{slug}");
        let house = Worktree::open(&spec).unwrap();
        assert_eq!(
            house.branch.as_deref(),
            Some("G/mission-4-inspect-the-service")
        );
        fs::write(house.path.join("work.txt"), "work\n").unwrap();
        let report = house.finish().unwrap();
        assert!(report.kept);
        let reopened = Worktree::open(&SafeHouseSpec {
            reopen: Some(report),
            ..spec
        })
        .unwrap();
        assert_eq!(reopened, house);
    }

    #[test]
    fn a_report_for_another_branch_or_base_is_refused() {
        let temp = Temp::new();
        let repo = temp.repo();
        let spec = temp.spec(&repo, "mission-5", "{id}");
        let house = Worktree::open(&spec).unwrap();
        fs::write(house.path.join("work.txt"), "work\n").unwrap();
        let report = house.finish().unwrap();
        for bad in [
            SafeHouseReport {
                branch: Some("other".to_owned()),
                ..report.clone()
            },
            SafeHouseReport {
                base: Some("0".repeat(40)),
                ..report.clone()
            },
            SafeHouseReport {
                kept: false,
                ..report
            },
        ] {
            let attempt = SafeHouseSpec {
                reopen: Some(bad),
                ..spec.clone()
            };
            assert!(Worktree::open(&attempt).is_err());
        }
    }

    #[test]
    fn working_in_place_reports_commits_and_is_never_removed() {
        let temp = Temp::new();
        let repo = temp.repo();
        let spec = SafeHouseSpec {
            isolate: false,
            ..temp.spec(&repo, "mission-6", "{id}")
        };
        let house = Worktree::open(&spec).unwrap();
        assert!(!house.isolated);
        assert_eq!(house.path, git_path(repo.canonicalize().unwrap()));
        assert!(house.changes().unwrap().is_empty());
        fs::write(repo.join("work.txt"), "work\n").unwrap();
        temp.git(&repo, &["add", "work.txt"]);
        temp.git(&repo, &["commit", "--quiet", "-m", "in place"]);
        assert_eq!(house.changes().unwrap().len(), 1);
        let report = house.finish().unwrap();
        assert!(report.kept && repo.exists());
        let again = Worktree::open(&SafeHouseSpec {
            reopen: Some(SafeHouseReport {
                kept: true,
                ..report
            }),
            ..spec
        })
        .unwrap();
        assert_eq!(again.base, house.base);
        assert_eq!(again.changes().unwrap().len(), 1);
    }

    #[test]
    fn a_directory_that_is_not_a_checkout_is_refused() {
        let temp = Temp::new();
        let plain = temp.0.join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert!(Worktree::open(&temp.spec(&plain, "mission-7", "{id}")).is_err());
    }
}
