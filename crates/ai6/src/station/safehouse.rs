//! Isolated Git worktrees for Missions at registered Operations.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::MissionId;
use crate::directorates::m::cover::CoverCommit;

/// The state-directory environment variable used by AI6.
pub const STATE_ENV: &str = "AI6_STATE";
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// A rejected safe-house operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SafeHouseError(String);

impl std::fmt::Display for SafeHouseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SafeHouseError {}

/// A Mission worktree cut from a registered Operation's checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SafeHouse {
    /// Operation checkout from which the worktree was created.
    pub repository: PathBuf,
    /// Isolated worktree path.
    pub path: PathBuf,
    /// New branch assigned to the Mission.
    pub branch: String,
    /// Commit the worktree was created from.
    pub base: String,
}

/// Public safe-house details attached to a Mission report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct SafeHouseReport {
    /// Worktree path.
    pub path: String,
    /// Branch assigned to the Mission.
    pub branch: String,
    /// Base commit.
    pub base: String,
    /// Whether the worktree was retained after the Mission.
    pub kept: bool,
    /// Commits made after the base, newest first.
    pub commits: Vec<String>,
}

impl SafeHouse {
    /// Reopens a persisted active worktree after the daemon process restarts.
    pub fn reopen(
        repository: impl AsRef<Path>,
        report: &SafeHouseReport,
    ) -> Result<Self, SafeHouseError> {
        if !report.kept {
            return Err(fail("safe-house report is not active"));
        }
        let repository = repository
            .as_ref()
            .canonicalize()
            .map_err(|error| fail(format!("{}: {error}", repository.as_ref().display())))?;
        let repository = git_path(repository);
        let path = git_path(
            Path::new(&report.path)
                .canonicalize()
                .map_err(|error| fail(format!("{}: {error}", report.path)))?,
        );
        if !path.is_dir() {
            return Err(fail("safe-house worktree is not a directory"));
        }
        let top = PathBuf::from(git(&path, &["rev-parse", "--show-toplevel"])?.trim())
            .canonicalize()
            .map_err(|error| fail(format!("safe-house worktree root: {error}")))?;
        if git_path(top) != path {
            return Err(fail(
                "safe-house report path does not match its Git worktree",
            ));
        }
        let worktrees = git(&repository, &["worktree", "list", "--porcelain"])?;
        let registered = worktrees.lines().any(|line| {
            line.strip_prefix("worktree ")
                .and_then(|value| Path::new(value).canonicalize().ok())
                .is_some_and(|value| git_path(value) == path)
        });
        if !registered {
            return Err(fail(
                "safe-house is not registered with the Operation repository",
            ));
        }
        let branch = git(&path, &["symbolic-ref", "--short", "HEAD"])?;
        if branch.trim() != report.branch {
            return Err(fail("safe-house branch does not match its Mission report"));
        }
        let base = git(
            &path,
            &[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", report.base),
            ],
        )?;
        if base.trim() != report.base {
            return Err(fail(
                "safe-house base commit does not match its Mission report",
            ));
        }
        git(
            &path,
            &["merge-base", "--is-ancestor", &report.base, "HEAD"],
        )?;
        Ok(Self {
            repository,
            path,
            branch: report.branch.clone(),
            base: report.base.clone(),
        })
    }

    /// Creates a new worktree under `root/safe-houses/<operation>/<mission>`.
    pub fn create(
        repository: impl AsRef<Path>,
        root: impl AsRef<Path>,
        operation_id: &str,
        mission_id: &MissionId,
        title: &str,
        branch_template: &str,
        base: &str,
    ) -> Result<Self, SafeHouseError> {
        let repository = repository
            .as_ref()
            .canonicalize()
            .map_err(|error| fail(format!("{}: {error}", repository.as_ref().display())))?;
        let repository = git_path(repository);
        let inside = git(&repository, &["rev-parse", "--is-inside-work-tree"])?;
        if inside.trim() != "true" {
            return Err(fail(format!(
                "{} is not a Git checkout",
                repository.display()
            )));
        }
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|error| fail(format!("{}: {error}", root.display())))?;
        let root = root
            .canonicalize()
            .map_err(|error| fail(format!("{}: {error}", root.display())))?;
        let root = git_path(root);
        if root.starts_with(&repository) {
            return Err(fail(format!(
                "safe-house root {} must be outside Operation checkout {}",
                root.display(),
                repository.display()
            )));
        }

        let branch = branch_name(branch_template, mission_id.as_str(), title, operation_id);
        git(&repository, &["check-ref-format", "--branch", &branch])?;
        let commit = git(
            &repository,
            &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
        )?
        .trim()
        .to_owned();
        if commit.is_empty() {
            return Err(fail(format!("Git did not resolve base {base:?}")));
        }

        let safe_station = path_segment(operation_id);
        let safe_mission = path_segment(mission_id.as_str());
        let path = root
            .join("safe-houses")
            .join(safe_station)
            .join(safe_mission);
        if path.exists() {
            return Err(fail(format!(
                "safe-house path {} already exists",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| fail(format!("{}: {error}", parent.display())))?;
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
            branch,
            base: commit,
        })
    }

    /// Removes an untouched worktree; retains it when there are commits or changes.
    pub fn finish(&self) -> Result<SafeHouseReport, SafeHouseError> {
        if !self.path.exists() {
            return Ok(self.report(false, Vec::new()));
        }
        let commits = self.commits()?;
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
        let _ = git(&self.repository, &["branch", "-D", &self.branch]);
        Ok(self.report(false, Vec::new()))
    }

    /// Commits made on this Mission branch since its base.
    pub fn commits(&self) -> Result<Vec<String>, SafeHouseError> {
        let output = git(
            &self.path,
            &["log", "--format=%h %s", &format!("{}..HEAD", self.base)],
        )?;
        Ok(output.lines().map(str::to_owned).collect())
    }

    /// Returns commit identity and full messages since `base` for cover auditing.
    #[cfg_attr(
        not(feature = "daemon"),
        allow(dead_code, reason = "used only by the daemon")
    )]
    pub(crate) fn cover_commits(&self) -> Result<Vec<CoverCommit>, SafeHouseError> {
        commits_since(&self.path, &self.base)
    }

    /// Describes a worktree that is still active.
    pub fn active_report(&self) -> SafeHouseReport {
        self.report(true, Vec::new())
    }

    fn report(&self, kept: bool, commits: Vec<String>) -> SafeHouseReport {
        SafeHouseReport {
            path: self.path.to_string_lossy().into_owned(),
            branch: self.branch.clone(),
            base: self.base.clone(),
            kept,
            commits,
        }
    }
}

/// Resolves the checked-out commit, returning `None` when `path` is not a Git checkout.
#[cfg_attr(
    not(feature = "daemon"),
    allow(dead_code, reason = "used only by the daemon")
)]
pub(crate) fn head(path: &Path) -> Option<String> {
    git(path, &["rev-parse", "--verify", "HEAD^{commit}"])
        .ok()
        .map(|commit| commit.trim().to_owned())
        .filter(|commit| !commit.is_empty())
}

/// Reads full commit metadata since a Mission's starting commit.
#[cfg_attr(
    not(feature = "daemon"),
    allow(dead_code, reason = "used only by the daemon")
)]
pub(crate) fn commits_since(path: &Path, base: &str) -> Result<Vec<CoverCommit>, SafeHouseError> {
    let format = "%H%x1f%an <%ae>%x1f%cn <%ce>%x1f%B%x1e";
    let format_arg = format!("--format={format}");
    let range = format!("{base}..HEAD");
    let output = git(path, &["log", &format_arg, &range])?;
    Ok(output
        .split('\u{1e}')
        .filter_map(|record| {
            let fields = record
                .trim_matches('\n')
                .split('\u{1f}')
                .collect::<Vec<_>>();
            let [sha, author, committer, message] = fields.as_slice() else {
                return None;
            };
            Some(CoverCommit {
                sha: (*sha).to_owned(),
                author: (*author).to_owned(),
                committer: (*committer).to_owned(),
                message: (*message).to_owned(),
            })
        })
        .collect())
}

/// Resolves AI6's state directory for retained worktrees and Mission reports.
pub fn state_dir() -> Result<PathBuf, SafeHouseError> {
    if let Some(path) = std::env::var_os(STATE_ENV) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("AI6_HOME") {
        return Ok(PathBuf::from(path).join("state"));
    }
    #[cfg(windows)]
    if let Some(path) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(path).join("AI6"));
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".local").join("state"))
        })
        .ok_or_else(|| fail("could not determine AI6 state directory"))?;
    Ok(base.join("AI6"))
}

/// Fills `{id}`, `{slug}` and `{operation}` and removes characters Git rejects in refs.
pub fn branch_name(template: &str, mission_id: &str, title: &str, operation: &str) -> String {
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

fn git(cwd: &Path, args: &[&str]) -> Result<String, SafeHouseError> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            fail(format!(
                "git {}: {error}",
                args.first().copied().unwrap_or("")
            ))
        })?;
    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| fail("git stdout pipe was unavailable"))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| fail("git stderr pipe was unavailable"))?;
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
                return Err(fail(format!(
                    "git {} timed out after {} seconds",
                    args.first().copied().unwrap_or("command"),
                    GIT_TIMEOUT.as_secs()
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(fail(format!(
                    "git {}: {error}",
                    args.first().copied().unwrap_or("")
                )));
            }
        }
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| fail("git stdout reader panicked"))?
        .map_err(|error| fail(format!("git stdout: {error}")))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| fail("git stderr reader panicked"))?
        .map_err(|error| fail(format!("git stderr: {error}")))?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr).trim().to_owned();
        return Err(fail(format!(
            "git {} failed in {}: {stderr}",
            args.first().copied().unwrap_or("command"),
            cwd.display()
        )));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

fn read_pipe(mut pipe: impl Read) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    pipe.read_to_end(&mut output)?;
    Ok(output)
}

fn fail(message: impl Into<String>) -> SafeHouseError {
    SafeHouseError(message.into())
}

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
                "ai6-safehouse-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn git(&self, cwd: &Path, args: &[&str]) {
            let status = Command::new("git")
                .arg("-C")
                .arg(cwd)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
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
        let house = SafeHouse::create(
            &repo,
            &temp.0,
            "G",
            &MissionId::try_new("mission-1").unwrap(),
            "Inspect the service",
            "{operation}/{id}-{slug}",
            "HEAD",
        )
        .unwrap();
        assert!(house.path.join("seed.txt").exists());
        let report = house.finish().unwrap();
        assert!(!report.kept);
        assert!(!house.path.exists());
        assert!(
            !git(&repo, &["branch", "--list", &house.branch])
                .unwrap()
                .contains(&house.branch)
        );
    }

    #[test]
    fn modified_worktree_is_kept_and_reported() {
        let temp = Temp::new();
        let repo = temp.repo();
        let house = SafeHouse::create(
            &repo,
            &temp.0,
            "G",
            &MissionId::try_new("mission-2").unwrap(),
            "Inspect the service",
            "{id}-{slug}",
            "HEAD",
        )
        .unwrap();
        fs::write(house.path.join("work.txt"), "work\n").unwrap();
        let report = house.finish().unwrap();
        assert!(report.kept);
        assert!(house.path.exists());
        assert!(report.commits.is_empty());
    }

    #[test]
    fn committed_worktree_is_kept_with_its_commit_summary() {
        let temp = Temp::new();
        let repo = temp.repo();
        let house = SafeHouse::create(
            &repo,
            &temp.0,
            "G",
            &MissionId::try_new("mission-commit").unwrap(),
            "Inspect the service",
            "{id}-{slug}",
            "HEAD",
        )
        .unwrap();
        fs::write(house.path.join("work.txt"), "work\n").unwrap();
        let status = Command::new("git")
            .arg("-C")
            .arg(&house.path)
            .args(["add", "work.txt"])
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .arg("-C")
            .arg(&house.path)
            .args(["commit", "--quiet", "-m", "add work"])
            .status()
            .unwrap();
        assert!(status.success());
        let commits = house.cover_commits().unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].author, "AI6 Tests <ai6@example.invalid>");
        assert_eq!(commits[0].committer, "AI6 Tests <ai6@example.invalid>");
        assert_eq!(commits[0].message.trim(), "add work");
        let report = house.finish().unwrap();
        assert!(report.kept);
        assert_eq!(report.commits.len(), 1);
        assert!(report.commits[0].ends_with(" add work"));
        assert!(house.path.exists());
    }

    #[test]
    fn refuses_to_put_safe_houses_inside_the_checkout_or_reuse_a_path() {
        let temp = Temp::new();
        let repo = temp.repo();
        let id = MissionId::try_new("mission-3").unwrap();
        assert!(
            SafeHouse::create(
                &repo,
                repo.join(".ai6-state"),
                "G",
                &id,
                "Inspect",
                "{id}-{slug}",
                "HEAD",
            )
            .is_err()
        );
        let house =
            SafeHouse::create(&repo, &temp.0, "G", &id, "Inspect", "{id}-{slug}", "HEAD").unwrap();
        assert!(
            SafeHouse::create(&repo, &temp.0, "G", &id, "Inspect", "{id}-{slug}", "HEAD",).is_err()
        );
        house.finish().unwrap();
    }
}
