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

use ai6::directorates::q::safehouse::{
    Change, CheckpointSpec, SafeHouseCheckpoint, SafeHouseExports, SafeHouseReport, SafeHouseSpec,
};

const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// A rejected Git operation, as text for the provider error.
pub(crate) type Failure = String;

/// An open safe house: a worktree of `repository`, `repository` itself when not isolated, or
/// a clone (`cloned`), which is its own repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Worktree {
    pub(crate) repository: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) branch: Option<String>,
    /// The commit the work started from; empty for a clone of an empty repository.
    pub(crate) base: String,
    pub(crate) isolated: bool,
    pub(crate) cloned: bool,
    /// A clone's git directory: every command names it and the working tree outright.
    pub(crate) git_dir: Option<PathBuf>,
}

impl Worktree {
    /// Creates, re-attaches or adopts the safe house `spec` describes.
    pub(crate) fn open(spec: &SafeHouseSpec) -> Result<Self, Failure> {
        if let Some(clone) = &spec.clone {
            return Self::clone_into(spec, clone);
        }
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

        let (branch, commit, base) = match &spec.checkpoint {
            Some(checkpoint) => resume_point(&repository, checkpoint)?,
            None => {
                let commit = resolve(&repository, &spec.base)?;
                let branch = branch_name(
                    &spec.branch,
                    &spec.mission_id,
                    &spec.title,
                    &spec.operation_id,
                );
                (branch, commit.clone(), commit)
            }
        };
        git(&repository, &["check-ref-format", "--branch", &branch])?;

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
            base,
            isolated: true,
            cloned: false,
            git_dir: None,
        })
    }

    /// Saves the work so far as one commit on top of `HEAD`, holding the uncommitted changes
    /// too, and pushes it to the safe house's branch on `spec.remote`. The worktree, its
    /// index and `HEAD` are left as they are: the Agent keeps working undisturbed. With
    /// `spec.branch` the work is committed in place and pushed there instead ([`Self::save`]).
    pub(crate) fn checkpoint(&self, spec: &CheckpointSpec) -> Result<SafeHouseCheckpoint, Failure> {
        if let Some(branch) = &spec.branch {
            return self.save(spec, branch);
        }
        let branch = self
            .branch
            .as_ref()
            .ok_or("a safe house in place has no branch of its own to save to")?;
        let head = resolve(&self.path, "HEAD")?;
        let tree = {
            // A private index, so the Agent's own staging area is untouched.
            let index = PrivateIndex::new();
            let env = [("GIT_INDEX_FILE", index.0.to_string_lossy().into_owned())];
            git_with(&self.path, &["read-tree", "HEAD"], &env)?;
            git_with(&self.path, &["add", "--all"], &env)?;
            git_with(&self.path, &["write-tree"], &env)?
                .trim()
                .to_owned()
        };
        let head_tree = git(&self.path, &["rev-parse", "HEAD^{tree}"])?
            .trim()
            .to_owned();
        let commit = if tree == head_tree {
            head
        } else {
            let mut env = Vec::new();
            if let (Some(name), Some(email)) = (&spec.author_name, &spec.author_email) {
                for (key, value) in [
                    ("GIT_AUTHOR_NAME", name),
                    ("GIT_AUTHOR_EMAIL", email),
                    ("GIT_COMMITTER_NAME", name),
                    ("GIT_COMMITTER_EMAIL", email),
                ] {
                    env.push((key, value.clone()));
                }
            }
            git_with(
                &self.path,
                &["commit-tree", &tree, "-p", &head, "-m", &spec.message],
                &env,
            )?
            .trim()
            .to_owned()
        };
        // Each checkpoint replaces the last on the branch; it holds everything the last did.
        git(
            &self.path,
            &[
                "push",
                "--quiet",
                "--force",
                &spec.remote,
                &format!("{commit}:refs/heads/{branch}"),
            ],
        )?;
        Ok(SafeHouseCheckpoint {
            remote: spec.remote.clone(),
            branch: branch.clone(),
            commit,
            base: Some(self.base.clone()),
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
            cloned: false,
            git_dir: None,
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
            cloned: false,
            git_dir: None,
        })
    }

    /// Removes an untouched worktree; retains it when there are commits or changes.
    pub(crate) fn finish(&self) -> Result<SafeHouseReport, Failure> {
        if !self.path.exists() {
            return Ok(self.report(false, Vec::new()));
        }
        if self.cloned {
            return self.finish_clone();
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

    /// What of the work exists only here, judged against the live remotes: each configured
    /// remote is fetched with `--prune` first, so a stale or locally written remote-tracking
    /// ref never makes work read as exported, and a remote that cannot be reached fails the
    /// call. Untracked files count whatever `status.showUntrackedFiles` says; ignored files
    /// are listed for the caller to judge. Submodules and Git LFS objects are not looked
    /// into: their presence is listed as unexported work.
    pub(crate) fn exports(&self) -> Result<SafeHouseExports, Failure> {
        if !self.path.exists() {
            return Ok(SafeHouseExports::default());
        }
        let remotes: Vec<String> = self
            .here(&["remote"])?
            .lines()
            .map(str::trim)
            .filter(|remote| !remote.is_empty())
            .map(str::to_owned)
            .collect();
        for remote in &remotes {
            self.here(&["fetch", "--prune", "--quiet", "--no-tags", "--", remote])?;
        }
        let range = self.since_base();
        let mut args = vec!["log".to_owned(), "--format=%h %s".to_owned(), range];
        if !remotes.is_empty() {
            args.push("--not".to_owned());
            args.extend(remotes.iter().map(|remote| format!("--remotes={remote}/*")));
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut unexported: Vec<String> = self.here(&args)?.lines().map(str::to_owned).collect();
        if self.path.join(".gitmodules").exists() {
            unexported.push("submodules: their own work is not checked".to_owned());
        }
        if fs::read_to_string(self.path.join(".gitattributes"))
            .is_ok_and(|attributes| attributes.contains("filter=lfs"))
        {
            unexported.push("Git LFS objects: whether they were pushed is not checked".to_owned());
        }
        let dirty = !self
            .here(&[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ])?
            .trim()
            .is_empty();
        // Ignored directories are listed whole, not file by file.
        let ignored = self
            .here(&[
                "status",
                "--porcelain",
                "--untracked-files=normal",
                "--ignored=traditional",
            ])?
            .lines()
            .filter_map(|line| line.strip_prefix("!! "))
            .map(|path| path.trim_matches('"').trim_end_matches('/').to_owned())
            .collect();
        Ok(SafeHouseExports {
            unexported,
            dirty,
            ignored,
        })
    }

    /// Removes the worktree and its branch whatever they hold. Working in place is never
    /// removed.
    pub(crate) fn discard(&self) -> Result<(), Failure> {
        if !self.isolated {
            return Err("a safe house in the Operation's own checkout is never removed".to_owned());
        }
        if self.cloned {
            return self.remove_clone();
        }
        if self.path.exists() {
            git(
                &self.repository,
                &[
                    "worktree",
                    "remove",
                    "--force",
                    "--force",
                    self.path.to_string_lossy().as_ref(),
                ],
            )?;
        }
        if let Some(branch) = &self.branch
            && !git(&self.repository, &["branch", "--list", branch])?
                .trim()
                .is_empty()
        {
            git(&self.repository, &["branch", "-D", branch])?;
        }
        Ok(())
    }

    /// Runs `git args` for this safe house: in its directory, or, for a clone, as
    /// [`crate::clone::Repo`] runs it.
    pub(crate) fn here(&self, args: &[&str]) -> Result<String, Failure> {
        match &self.git_dir {
            Some(git_dir) => crate::clone::Repo::new(git_dir, &self.path).run(args, &[]),
            None => git(&self.path, args),
        }
    }

    /// The commits since the base, as a `git log` range: every commit of `HEAD` for a clone
    /// of an empty repository.
    pub(crate) fn since_base(&self) -> String {
        if self.base.is_empty() {
            "HEAD".to_owned()
        } else {
            format!("{}..HEAD", self.base)
        }
    }

    /// One-line summaries of the commits made since the base, newest first.
    pub(crate) fn summaries(&self) -> Result<Vec<String>, Failure> {
        if self.base.is_empty()
            && self
                .here(&["rev-parse", "--verify", "HEAD^{commit}"])
                .is_err()
        {
            return Ok(Vec::new());
        }
        let output = self.here(&["log", "--format=%h %s", &self.since_base()])?;
        Ok(output.lines().map(str::to_owned).collect())
    }

    /// Full commit identity and messages since the base, newest first.
    pub(crate) fn changes(&self) -> Result<Vec<Change>, Failure> {
        if self.base.is_empty()
            && self
                .here(&["rev-parse", "--verify", "HEAD^{commit}"])
                .is_err()
        {
            return Ok(Vec::new());
        }
        let format = "%H%x1f%an <%ae>%x1f%cn <%ce>%x1f%B%x1e";
        let format_arg = format!("--format={format}");
        let range = self.since_base();
        let output = self.here(&["log", &format_arg, &range])?;
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

/// The commit `revision` names in `cwd`.
pub(crate) fn resolve(cwd: &Path, revision: &str) -> Result<String, Failure> {
    let commit = git(
        cwd,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )?
    .trim()
    .to_owned();
    if commit.is_empty() {
        return Err(format!("Git did not resolve {revision:?}"));
    }
    Ok(commit)
}

/// Where a safe house that continues `checkpoint` starts: its branch, the commit to check
/// out, and the base its changes are counted from. The checkpoint's branch is fetched from
/// its remote; its recorded commit is used when present, else the branch's newest one.
fn resume_point(
    repository: &Path,
    checkpoint: &SafeHouseCheckpoint,
) -> Result<(String, String, String), Failure> {
    git(
        repository,
        &[
            "fetch",
            "--quiet",
            &checkpoint.remote,
            &format!("refs/heads/{}", checkpoint.branch),
        ],
    )?;
    let commit =
        resolve(repository, &checkpoint.commit).or_else(|_| resolve(repository, "FETCH_HEAD"))?;
    let base = checkpoint
        .base
        .as_deref()
        .filter(|base| git(repository, &["merge-base", "--is-ancestor", base, &commit]).is_ok())
        .map_or_else(|| commit.clone(), str::to_owned);
    Ok((checkpoint.branch.clone(), commit, base))
}

/// A temporary index file, removed when dropped.
struct PrivateIndex(PathBuf);

impl PrivateIndex {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "ai6-safehouse-{}-{unique}.index",
            std::process::id()
        )))
    }
}

impl Drop for PrivateIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
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
    git_with(cwd, args, &[])
}

pub(crate) fn git_with(
    cwd: &Path,
    args: &[&str],
    env: &[(&str, String)],
) -> Result<String, Failure> {
    git_input(cwd, args, env, None)
}

/// How long a Git command may run, and how large the directories it fills may grow.
pub(crate) struct Limits {
    /// Past it the command is killed.
    pub(crate) timeout: Duration,
    /// The command is killed once these directories hold more than `max_bytes` together.
    pub(crate) watch: Vec<PathBuf>,
    pub(crate) max_bytes: Option<u64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: GIT_TIMEOUT,
            watch: Vec::new(),
            max_bytes: None,
        }
    }
}

/// How often a command's watched directories are measured.
const MEASURE_EVERY: Duration = Duration::from_millis(250);

/// The bytes of the files under `paths`, never following a link.
pub(crate) fn tree_bytes(paths: &[PathBuf]) -> u64 {
    fn walk(path: &Path) -> u64 {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return 0;
        };
        if metadata.is_dir() {
            fs::read_dir(path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| walk(&entry.path()))
                .sum()
        } else {
            metadata.len()
        }
    }
    paths.iter().map(|path| walk(path)).sum()
}

/// [`git_with`], with `input` written to the command's standard input.
pub(crate) fn git_input(
    cwd: &Path,
    args: &[&str],
    env: &[(&str, String)],
    input: Option<&[u8]>,
) -> Result<String, Failure> {
    git_limited(cwd, args, env, input, &Limits::default())
}

/// [`git_input`] within `limits`.
pub(crate) fn git_limited(
    cwd: &Path,
    args: &[&str],
    env: &[(&str, String)],
    input: Option<&[u8]>,
    limits: &Limits,
) -> Result<String, Failure> {
    // The subcommand, past the global options (`-c key=value`, `--git-dir=...`).
    let name = args
        .iter()
        .enumerate()
        .find(|(index, arg)| !arg.starts_with('-') && (*index == 0 || args[index - 1] != "-c"))
        .map_or("command", |(_, arg)| *arg);
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .envs(env.iter().map(|(key, value)| (*key, value.as_str())))
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git {name}: {error}"))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        let input = input.to_vec();
        thread::spawn(move || {
            use std::io::Write;
            let _ = stdin.write_all(&input);
        });
    }
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
    let deadline = Instant::now() + limits.timeout;
    let mut measured = Instant::now();
    let status = loop {
        let over = limits
            .max_bytes
            .filter(|_| measured.elapsed() >= MEASURE_EVERY)
            .and_then(|max| {
                measured = Instant::now();
                let bytes = tree_bytes(&limits.watch);
                (bytes > max).then_some(max)
            });
        let stopped = match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if over.is_none() && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Ok(None) => match over {
                Some(max) => format!("git {name} was stopped: it wrote more than {max} bytes"),
                None => format!(
                    "git {name} timed out after {} seconds",
                    limits.timeout.as_secs()
                ),
            },
            Err(error) => format!("git {name}: {error}"),
        };
        let _ = child.kill();
        let _ = child.wait();
        // Not joined: a process Git started may still hold the pipes open, and its readers end
        // when it does.
        drop((stdout_reader, stderr_reader));
        return Err(stopped);
    };
    // One that finished between two measurements is measured once more.
    if let Some(max) = limits.max_bytes
        && tree_bytes(&limits.watch) > max
    {
        return Err(format!(
            "git {name} was stopped: it wrote more than {max} bytes"
        ));
    }
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

pub(crate) fn fail(path: &Path, error: std::io::Error) -> Failure {
    format!("{}: {error}", path.display())
}

/// Drops the verbatim prefix `canonicalize` adds on Windows, which Git does not accept.
pub(crate) fn git_path(path: PathBuf) -> PathBuf {
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
                checkpoint: None,
                clone: None,
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
    fn exports_name_the_commits_no_remote_holds_and_discard_removes_them_all() {
        let temp = Temp::new();
        let repo = temp.repo();
        let remote = temp.0.join("remote.git");
        temp.git(&temp.0, &["init", "--quiet", "--bare", "remote.git"]);
        temp.git(
            &repo,
            &["remote", "add", "origin", remote.to_string_lossy().as_ref()],
        );
        let house = Worktree::open(&temp.spec(&repo, "mission-export", "{id}-{slug}")).unwrap();
        assert_eq!(house.exports().unwrap(), SafeHouseExports::default());

        // A remote-tracking ref written locally, without the remote holding the commit, does not
        // make the work exported: the remote is asked.
        fs::write(house.path.join("forged.txt"), "forged\n").unwrap();
        temp.git(&house.path, &["add", "forged.txt"]);
        temp.git(&house.path, &["commit", "--quiet", "-m", "forged export"]);
        temp.git(
            &repo,
            &[
                "update-ref",
                "refs/remotes/origin/forged",
                &resolve(&house.path, "HEAD").unwrap(),
            ],
        );
        let exports = house.exports().unwrap();
        assert_eq!(exports.unexported.len(), 1, "{exports:?}");
        temp.git(&house.path, &["reset", "--quiet", "--hard", "HEAD~1"]);

        // Untracked files count even when status hides them; ignored ones are listed.
        temp.git(&house.path, &["config", "status.showUntrackedFiles", "no"]);
        fs::write(house.path.join(".gitignore"), ".env\ntarget/\n").unwrap();
        temp.git(&house.path, &["add", ".gitignore"]);
        temp.git(&house.path, &["commit", "--quiet", "-m", "ignore"]);
        temp.git(
            &house.path,
            &["push", "--quiet", "origin", "HEAD:refs/heads/ignore"],
        );
        fs::create_dir_all(house.path.join("target/debug")).unwrap();
        fs::write(house.path.join("target/debug/app"), "x").unwrap();
        fs::write(house.path.join(".env"), "TOKEN=x\n").unwrap();
        let exports = house.exports().unwrap();
        assert!(exports.unexported.is_empty(), "{exports:?}");
        assert!(!exports.dirty);
        assert_eq!(exports.ignored, [".env", "target"]);
        fs::write(house.path.join("loose.txt"), "loose\n").unwrap();
        assert!(house.exports().unwrap().dirty);
        fs::remove_file(house.path.join("loose.txt")).unwrap();
        fs::remove_file(house.path.join(".env")).unwrap();
        fs::remove_dir_all(house.path.join("target")).unwrap();
        fs::write(house.path.join(".gitattributes"), "*.bin filter=lfs\n").unwrap();
        assert!(
            house
                .exports()
                .unwrap()
                .unexported
                .iter()
                .any(|line| line.contains("LFS")),
            "LFS objects are not looked into"
        );
        fs::remove_file(house.path.join(".gitattributes")).unwrap();

        fs::write(house.path.join("work.txt"), "work\n").unwrap();
        temp.git(&house.path, &["add", "work.txt"]);
        temp.git(&house.path, &["commit", "--quiet", "-m", "add work"]);
        let exports = house.exports().unwrap();
        assert_eq!(exports.unexported.len(), 1, "{exports:?}");
        assert!(exports.unexported[0].ends_with(" add work"));
        assert!(!exports.dirty);

        let branch = house.branch.clone().unwrap();
        temp.git(&house.path, &["push", "--quiet", "origin", &branch]);
        assert!(house.exports().unwrap().is_exported(|_| false));
        fs::write(house.path.join("loose.txt"), "loose\n").unwrap();
        assert!(house.exports().unwrap().dirty);

        house.discard().unwrap();
        assert!(!house.path.exists());
        assert!(
            git(&repo, &["branch", "--list", &branch])
                .unwrap()
                .trim()
                .is_empty()
        );
        // Discarding what is already gone is a no-op; work in place is never discarded.
        house.discard().unwrap();
        let mut in_place = temp.spec(&repo, "mission-export-2", "{id}");
        in_place.isolate = false;
        assert!(Worktree::open(&in_place).unwrap().discard().is_err());
        assert!(repo.join("seed.txt").exists());
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

    fn checkpoint_spec(remote: &Path) -> CheckpointSpec {
        CheckpointSpec {
            remote: remote.to_string_lossy().into_owned(),
            message: "Checkpoint of work in progress".to_owned(),
            author_name: Some("Field Station".to_owned()),
            author_email: Some("station@example.invalid".to_owned()),
            branch: None,
            fallback: None,
            files: None,
            record: None,
            exclude: None,
            max_bytes: None,
        }
    }

    #[test]
    fn a_checkpoint_pushes_committed_and_uncommitted_work_and_leaves_the_worktree_alone() {
        let temp = Temp::new();
        let repo = temp.repo();
        let remote = temp.0.join("remote.git");
        temp.git(&temp.0, &["init", "--quiet", "--bare", "remote.git"]);
        let house = Worktree::open(&temp.spec(&repo, "mission-8", "{id}")).unwrap();
        fs::write(house.path.join("done.txt"), "done\n").unwrap();
        temp.git(&house.path, &["add", "done.txt"]);
        temp.git(&house.path, &["commit", "--quiet", "-m", "done part"]);
        fs::write(house.path.join("staged.txt"), "staged\n").unwrap();
        temp.git(&house.path, &["add", "staged.txt"]);
        fs::write(house.path.join("loose.txt"), "loose\n").unwrap();
        let head = resolve(&house.path, "HEAD").unwrap();
        let status = git(&house.path, &["status", "--porcelain"]).unwrap();

        let saved = house.checkpoint(&checkpoint_spec(&remote)).unwrap();
        assert_eq!(saved.branch, "mission-8");
        assert_eq!(saved.base.as_deref(), Some(house.base.as_str()));
        assert_ne!(saved.commit, head);
        // The Agent's worktree, staging and HEAD are as they were.
        assert_eq!(resolve(&house.path, "HEAD").unwrap(), head);
        assert_eq!(
            git(&house.path, &["status", "--porcelain"]).unwrap(),
            status
        );

        // The remote branch holds everything, under the cover identity.
        let pushed = resolve(&remote, "refs/heads/mission-8").unwrap();
        assert_eq!(pushed, saved.commit);
        let files = git(&remote, &["ls-tree", "--name-only", &pushed]).unwrap();
        for file in ["done.txt", "loose.txt", "seed.txt", "staged.txt"] {
            assert!(files.lines().any(|line| line == file), "{file}: {files}");
        }
        let author = git(
            &remote,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>", &pushed],
        )
        .unwrap();
        assert_eq!(
            author.trim(),
            "Field Station <station@example.invalid>|Field Station <station@example.invalid>"
        );

        // Nothing new since the last checkpoint pushes HEAD itself.
        fs::remove_file(house.path.join("loose.txt")).unwrap();
        temp.git(&house.path, &["reset", "--quiet"]);
        fs::remove_file(house.path.join("staged.txt")).unwrap();
        let clean = house.checkpoint(&checkpoint_spec(&remote)).unwrap();
        assert_eq!(clean.commit, head);
        assert_eq!(resolve(&remote, "refs/heads/mission-8").unwrap(), head);
    }

    #[test]
    fn a_safe_house_on_another_checkout_starts_from_a_checkpoint() {
        let temp = Temp::new();
        let repo = temp.repo();
        let remote = temp.0.join("remote.git");
        temp.git(&temp.0, &["init", "--quiet", "--bare", "remote.git"]);
        temp.git(
            &repo,
            &[
                "push",
                "--quiet",
                remote.to_str().unwrap(),
                "HEAD:refs/heads/main",
            ],
        );
        let house = Worktree::open(&temp.spec(&repo, "mission-9", "{id}")).unwrap();
        fs::write(house.path.join("half.txt"), "half done\n").unwrap();
        let saved = house.checkpoint(&checkpoint_spec(&remote)).unwrap();

        // Another Station has its own clone and its own state directory.
        temp.git(
            &temp.0,
            &["clone", "--quiet", remote.to_str().unwrap(), "other"],
        );
        let other = temp.0.join("other");
        // A Windows runner's global autocrlf would rewrite the file on checkout.
        temp.git(&other, &["config", "core.autocrlf", "false"]);
        let mut spec = temp.spec(&other, "mission-9", "{id}");
        spec.state_root = temp.0.join("other-state").to_string_lossy().into_owned();
        spec.checkpoint = Some(saved.clone());
        let resumed = Worktree::open(&spec).unwrap();
        assert_eq!(resumed.branch.as_deref(), Some("mission-9"));
        assert_eq!(resumed.base, house.base);
        assert_eq!(resolve(&resumed.path, "HEAD").unwrap(), saved.commit);
        assert_eq!(
            fs::read_to_string(resumed.path.join("half.txt")).unwrap(),
            "half done\n"
        );
        // The checkpoint commit counts as the Mission's work.
        assert_eq!(resumed.changes().unwrap().len(), 1);
    }

    #[test]
    fn a_safe_house_in_place_has_no_branch_to_checkpoint() {
        let temp = Temp::new();
        let repo = temp.repo();
        let spec = SafeHouseSpec {
            isolate: false,
            ..temp.spec(&repo, "mission-10", "{id}")
        };
        let house = Worktree::open(&spec).unwrap();
        assert!(house.checkpoint(&checkpoint_spec(&temp.0)).is_err());
    }

    #[test]
    fn a_directory_that_is_not_a_checkout_is_refused() {
        let temp = Temp::new();
        let plain = temp.0.join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert!(Worktree::open(&temp.spec(&plain, "mission-7", "{id}")).is_err());
    }
}
