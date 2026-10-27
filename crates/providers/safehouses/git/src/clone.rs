// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A safe house that is a clone of a repository on one of its branches, saved by committing
//! in place and pushing to a branch the clone shares with everyone else who uses it.
//!
//! The working directory may be written by someone the Station does not trust, so every Git
//! command of a clone names its git directory and working tree outright (no `.git` in the
//! working tree is ever looked for) and runs with no hooks, no filesystem monitor and no
//! commit signing: settings a repository's configuration could otherwise turn into programs
//! the Station runs. The Station user's global and system configuration still apply: they
//! hold its credential helpers, and nothing in a working tree writes them.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use sha2::{Digest, Sha256};

use ai6::RepoKind;
use ai6::directorates::q::safehouse::{
    CheckpointFile, CheckpointSpec, SafeHouseCheckpoint, SafeHouseClone, SafeHouseReport,
    SafeHouseSpec,
};

use crate::worktree::{Failure, Worktree, fail, git, git_input, git_path, git_with};

/// The settings every command of a clone runs with. `core.hooksPath` names a directory that
/// is never made, so no hook runs; `core.symlinks=false` checks a link out as a plain file
/// holding its target, so nothing in a working tree leads outside it. Every filter driver
/// (`lfs` and each the global or system configuration names) is blanked and not required,
/// so no `.gitattributes` in a repository makes a checkout or a save run one.
fn hardened(git_dir: &Path) -> Vec<String> {
    let mut settings = vec![
        "-c".to_owned(),
        format!(
            "core.hooksPath={}",
            git_dir.join("ai6-no-hooks").to_string_lossy()
        ),
        "-c".to_owned(),
        "core.fsmonitor=false".to_owned(),
        "-c".to_owned(),
        "commit.gpgsign=false".to_owned(),
        "-c".to_owned(),
        "core.symlinks=false".to_owned(),
        // A checkout holds each file's bytes as committed, so what was read from it can be
        // compared with what is committed.
        "-c".to_owned(),
        "core.autocrlf=false".to_owned(),
    ];
    for driver in filter_drivers() {
        for key in ["clean", "smudge", "process"] {
            settings.extend(["-c".to_owned(), format!("filter.{driver}.{key}=")]);
        }
        settings.extend(["-c".to_owned(), format!("filter.{driver}.required=false")]);
    }
    settings
}

/// The environment every command of a clone runs with: Git LFS, should it run anyway,
/// fetches nothing.
const HARDENED_ENV: (&str, &str) = ("GIT_LFS_SKIP_SMUDGE", "1");

/// `lfs` and the filter drivers the Station user's global and system configuration define,
/// read once.
fn filter_drivers() -> &'static [String] {
    static DRIVERS: OnceLock<Vec<String>> = OnceLock::new();
    DRIVERS.get_or_init(|| {
        let mut drivers = BTreeSet::from(["lfs".to_owned()]);
        for scope in ["--global", "--system"] {
            let listed = git(
                &std::env::temp_dir(),
                &[
                    "config",
                    scope,
                    "--includes",
                    "--name-only",
                    "--get-regexp",
                    r"^filter\.",
                ],
            )
            .unwrap_or_default();
            for key in listed.lines() {
                // `filter.<driver>.<key>`, where the driver's name may hold dots.
                if let Some(driver) = key
                    .strip_prefix("filter.")
                    .and_then(|rest| rest.rsplit_once('.'))
                    .map(|(driver, _)| driver)
                    .filter(|driver| !driver.is_empty() && !driver.contains(['=', '\n']))
                {
                    drivers.insert(driver.to_owned());
                }
            }
        }
        drivers.into_iter().collect()
    })
}

/// Pathspecs leaving out of a save every path with a name starting with `.`
/// (`.gitattributes`, `.gitmodules`, `.lfsconfig`, `notes/.x`): files that change how Git or
/// its tools treat the repository are never committed unseen.
const HIDDEN_FILES: &str = ":(exclude,glob)**/.*";
const HIDDEN_DIRECTORIES: &str = ":(exclude,glob)**/.*/**";

/// Sets the index to `HEAD` with exactly `files` changed: each text is written as a blob of
/// its own, never read from the working tree, so what someone writes there meanwhile is not
/// taken with it.
fn stage_exactly(repo: &Repo<'_>, files: &[CheckpointFile]) -> Result<(), Failure> {
    for file in files {
        if !exact_path(&file.path) {
            return Err(format!("{:?} is not a file a save may write", file.path));
        }
    }
    if repo.resolve("HEAD").is_ok() {
        repo.run(&["read-tree", "HEAD"], &[])?;
    } else {
        repo.run(&["read-tree", "--empty"], &[])?;
    }
    for file in files {
        match &file.text {
            Some(text) => {
                let blob = repo
                    .input(
                        &["hash-object", "-w", "--no-filters", "--stdin"],
                        text.as_bytes(),
                    )?
                    .trim()
                    .to_owned();
                repo.run(
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        &format!("100644,{blob},{}", file.path),
                    ],
                    &[],
                )?;
            }
            None => {
                repo.run(&["update-index", "--force-remove", "--", &file.path], &[])?;
            }
        }
    }
    Ok(())
}

/// Stages an entry in `record` for each file the index adds or changes against `HEAD` (every
/// staged file when `HEAD` is unborn), outside `record` itself: the entry holds the path and a
/// newline and is named by its own object ID. Answers whether it staged any.
fn record_changes(repo: &Repo<'_>, record: &str) -> Result<bool, Failure> {
    if !exact_path(record) {
        return Err(format!(
            "{record:?} is not a directory a save may record in"
        ));
    }
    let listed = if repo.resolve("HEAD").is_ok() {
        repo.run(
            &[
                "diff-index",
                "--cached",
                "--name-only",
                "--no-renames",
                "--diff-filter=d",
                "-z",
                "HEAD",
            ],
            &[],
        )?
    } else {
        repo.run(&["ls-files", "--cached", "-z"], &[])?
    };
    let inside = format!("{record}/");
    let mut any = false;
    for path in listed.split('\0') {
        if path.is_empty() || path.starts_with(&inside) {
            continue;
        }
        any = true;
        let entry = repo
            .input(
                &["hash-object", "-w", "--no-filters", "--stdin"],
                format!("{path}\n").as_bytes(),
            )?
            .trim()
            .to_owned();
        repo.run(
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{entry},{record}/{entry}"),
            ],
            &[],
        )?;
    }
    Ok(any)
}

/// Refuses `files` when one with a `base` holds anything else on `branch` of `remote` (fetched
/// now): the change was made from another text, and would replace what was written since.
fn check_bases(
    repo: &Repo<'_>,
    remote: &str,
    branch: &str,
    files: &[CheckpointFile],
) -> Result<(), Failure> {
    if files.iter().all(|file| file.base.is_none()) {
        return Ok(());
    }
    let head = format!("refs/heads/{branch}");
    let tip = match repo.run(&["fetch", "--quiet", "--no-tags", remote, &head], &[]) {
        Ok(_) => Some(repo.resolve("FETCH_HEAD")?),
        // A branch that does not exist yet holds no file.
        Err(error) => match repo.run(&["ls-remote", "--heads", remote, &head], &[]) {
            Ok(listed) if listed.trim().is_empty() => None,
            _ => return Err(error),
        },
    };
    for file in files {
        let Some(base) = &file.base else {
            continue;
        };
        let at = tip.as_ref().map(|tip| format!("{tip}:{}", file.path));
        let found = match at
            .as_deref()
            .map(|at| repo.run(&["cat-file", "-t", at], &[]))
        {
            Some(Ok(kind)) if kind.trim() == "blob" => {
                let text = repo.run(
                    &["cat-file", "blob", at.as_deref().unwrap_or_default()],
                    &[],
                )?;
                Sha256::digest(text.as_bytes())
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect()
            }
            Some(Ok(kind)) => kind.trim().to_owned(),
            Some(Err(_)) | None => String::new(),
        };
        if found != *base {
            return Err(format!(
                "{} changed on {branch} since this change was made from it, so nothing was \
                 committed: make the change again from what it holds now",
                file.path
            ));
        }
    }
    Ok(())
}

/// Makes the working tree hold `HEAD`'s version of each of `files`: checked out when `HEAD`
/// has it, removed when it does not. Paths are literal, never patterns.
fn sync_exactly(repo: &Repo<'_>, files: &[CheckpointFile]) -> Result<(), Failure> {
    let (present, removed): (Vec<String>, Vec<String>) = {
        let (present, removed): (Vec<_>, Vec<_>) =
            files.iter().partition(|file| file.text.is_some());
        let literal = |files: Vec<&CheckpointFile>| {
            files
                .into_iter()
                .map(|file| format!(":(literal){}", file.path))
                .collect()
        };
        (literal(present), literal(removed))
    };
    if !present.is_empty() {
        let mut args = vec!["checkout", "--quiet", "HEAD", "--"];
        args.extend(present.iter().map(String::as_str));
        repo.run(&args, &[])?;
    }
    if !removed.is_empty() {
        let mut args = vec!["clean", "--quiet", "-f", "-x", "--"];
        args.extend(removed.iter().map(String::as_str));
        repo.run(&args, &[])?;
    }
    Ok(())
}

/// Whether `name` is a branch name by Git's rules (`git check-ref-format --branch`), checked
/// here rather than by a Git command, which would read the configuration of whatever
/// repository holds its working directory: no name part empty, starting with `.` or ending
/// in `.lock`; no `..`, `@{`, control character, space or any of `~^:?*[\`; not `@`, not
/// starting with `-` and not ending with `.`.
fn branch_name(name: &str) -> bool {
    !name.is_empty()
        && name != "@"
        && !name.starts_with('-')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// Whether `path` may be given to an exact save: 1 to 8 `/`-separated names, none empty,
/// `.`, `..` or `.git` in any case, none holding `\` or `:`. Hidden names are allowed: the
/// caller chose the file, unlike the working tree's.
fn exact_path(path: &str) -> bool {
    let names: Vec<&str> = path.split('/').collect();
    names.len() <= 8
        && names.iter().all(|name| {
            !matches!(*name, "" | "." | "..")
                && !name.eq_ignore_ascii_case(".git")
                && !name.contains(['\\', ':'])
                && !name.chars().any(char::is_control)
        })
}

/// Whether the index holds anything `HEAD` does not.
fn staged(repo: &Repo<'_>) -> Result<bool, Failure> {
    let tree = repo.run(&["write-tree"], &[])?.trim().to_owned();
    match repo.run(&["rev-parse", "--verify", "HEAD^{tree}"], &[]) {
        Ok(head) => Ok(head.trim() != tree),
        Err(_) => Ok(!repo.run(&["ls-files"], &[])?.trim().is_empty()),
    }
}

/// A clone's git directory and working tree, and how a command runs against them.
pub(crate) struct Repo<'a> {
    git_dir: &'a Path,
    work_tree: &'a Path,
}

impl<'a> Repo<'a> {
    pub(crate) fn new(git_dir: &'a Path, work_tree: &'a Path) -> Self {
        Self { git_dir, work_tree }
    }

    /// Runs `git args` with `env`, hardened, against this git directory and working tree.
    pub(crate) fn run(&self, args: &[&str], env: &[(&str, String)]) -> Result<String, Failure> {
        let mut all = vec![
            format!("--git-dir={}", self.git_dir.to_string_lossy()),
            format!("--work-tree={}", self.work_tree.to_string_lossy()),
        ];
        all.extend(hardened(self.git_dir));
        all.extend(args.iter().map(|arg| (*arg).to_owned()));
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        let mut env = env.to_vec();
        env.push((HARDENED_ENV.0, HARDENED_ENV.1.to_owned()));
        git_with(self.work_tree, &all, &env)
    }

    /// Runs `git args`, hardened, with `input` on its standard input.
    fn input(&self, args: &[&str], input: &[u8]) -> Result<String, Failure> {
        let mut all = vec![
            format!("--git-dir={}", self.git_dir.to_string_lossy()),
            format!("--work-tree={}", self.work_tree.to_string_lossy()),
        ];
        all.extend(hardened(self.git_dir));
        all.extend(args.iter().map(|arg| (*arg).to_owned()));
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        let env = [(HARDENED_ENV.0, HARDENED_ENV.1.to_owned())];
        git_input(self.work_tree, &all, &env, Some(input))
    }

    /// The commit `revision` names.
    fn resolve(&self, revision: &str) -> Result<String, Failure> {
        let commit = self
            .run(
                &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
                &[],
            )?
            .trim()
            .to_owned();
        if commit.is_empty() {
            return Err(format!("Git did not resolve {revision:?}"));
        }
        Ok(commit)
    }
}

impl Worktree {
    /// Clones `spec.source` into `clone.path` on branch `spec.base`, after creating the source
    /// as an empty bare repository when `clone.create_source` says so and it is a local one
    /// that does not exist. With `clone.git_dir` the git directory goes there and the working
    /// tree keeps no `.git`. A repository with no branch at all gives an unborn `base`; one
    /// without `base` but with other branches is refused.
    pub(crate) fn clone_into(
        spec: &SafeHouseSpec,
        clone: &SafeHouseClone,
    ) -> Result<Self, Failure> {
        let target = Path::new(&clone.path);
        if target.exists() {
            return Err(format!(
                "safe-house path {} already exists",
                target.display()
            ));
        }
        let kind = RepoKind::of(&spec.source)?;
        let parent = target
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", target.display()))?;
        fs::create_dir_all(parent).map_err(|error| fail(parent, error))?;
        let parent = git_path(parent.canonicalize().map_err(|error| fail(parent, error))?);
        if !branch_name(&spec.base) {
            return Err(format!("{:?} is not a branch name", spec.base));
        }
        if clone.create_source
            && kind == RepoKind::Local
            && let Some(local) = local_repository(&spec.source)
            && !local.exists()
        {
            create_bare(&local, &spec.base)?;
        }
        let name = target
            .file_name()
            .ok_or_else(|| format!("{} has no name", target.display()))?
            .to_string_lossy()
            .into_owned();
        let path = parent.join(&name);
        let git_dir = match &clone.git_dir {
            Some(git_dir) => {
                let git_dir = Path::new(git_dir);
                if git_dir.exists() {
                    return Err(format!(
                        "git directory {} already exists",
                        git_dir.display()
                    ));
                }
                let above = git_dir
                    .parent()
                    .ok_or_else(|| format!("{} has no parent directory", git_dir.display()))?;
                fs::create_dir_all(above).map_err(|error| fail(above, error))?;
                git_path(above.canonicalize().map_err(|error| fail(above, error))?).join(
                    git_dir
                        .file_name()
                        .ok_or_else(|| format!("{} has no name", git_dir.display()))?,
                )
            }
            None => path.join(".git"),
        };
        let mut args = hardened(&git_dir);
        args.extend(["clone".to_owned(), "--quiet".to_owned()]);
        if clone.git_dir.is_some() {
            args.push(format!("--separate-git-dir={}", git_dir.to_string_lossy()));
        }
        args.extend(["--".to_owned(), spec.source.clone(), name]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        git_with(
            &parent,
            &args,
            &[(HARDENED_ENV.0, HARDENED_ENV.1.to_owned())],
        )?;
        if clone.git_dir.is_some() {
            // The pointer Git leaves in the working tree is not needed: every command names the
            // git directory itself.
            fs::remove_file(path.join(".git")).map_err(|error| fail(&path, error))?;
        }
        let house = Self {
            repository: path.clone(),
            path,
            branch: Some(spec.base.clone()),
            base: String::new(),
            isolated: true,
            cloned: true,
            git_dir: Some(git_dir),
        };
        let repo = house.repo();
        let tracking = format!("refs/remotes/origin/{}", spec.base);
        if repo.resolve(&tracking).is_ok() {
            repo.run(
                &[
                    "checkout",
                    "--quiet",
                    "-B",
                    &spec.base,
                    "--track",
                    &format!("origin/{}", spec.base),
                ],
                &[],
            )?;
        } else if repo
            .run(&["for-each-ref", "--count=1", "refs/remotes"], &[])?
            .trim()
            .is_empty()
        {
            repo.run(
                &["symbolic-ref", "HEAD", &format!("refs/heads/{}", spec.base)],
                &[],
            )?;
        } else {
            let _ = house.remove_clone();
            return Err(format!("{} has no branch {:?}", spec.source, spec.base));
        }
        let base = repo.resolve("HEAD").unwrap_or_default();
        Ok(Self { base, ..house })
    }

    /// How commands run against this clone.
    fn repo(&self) -> Repo<'_> {
        Repo::new(self.git_dir.as_deref().unwrap_or(&self.path), &self.path)
    }

    /// Commits everything in the working directory but hidden paths ([`HIDDEN_FILES`]) as
    /// `spec`'s author, then pushes `HEAD` to
    /// `branch` of `spec.remote` without force. A refused push is tried once more after the
    /// work is replayed on the remote branch; if that fails too, `HEAD` goes to
    /// `spec.fallback` (forced, since only this work uses it) or the refusal is the error.
    pub(crate) fn save(
        &self,
        spec: &CheckpointSpec,
        branch: &str,
    ) -> Result<SafeHouseCheckpoint, Failure> {
        for name in std::iter::once(branch).chain(spec.fallback.as_deref()) {
            if !branch_name(name) {
                return Err(format!("{name:?} is not a branch name"));
            }
        }
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
        let repo = self.repo();
        let run = |args: &[&str]| repo.run(args, &env);
        if let Some(files) = &spec.files {
            check_bases(&repo, &spec.remote, branch, files)?;
            stage_exactly(&repo, files)?;
            if staged(&repo)? {
                run(&["commit", "--quiet", "--no-verify", "-m", &spec.message])?;
            }
        } else {
            let status = run(&["status", "--porcelain", "--untracked-files=all"])?;
            if !status.trim().is_empty() {
                let mut add = vec![
                    "add".to_owned(),
                    "--all".to_owned(),
                    "--".to_owned(),
                    ".".to_owned(),
                    HIDDEN_FILES.to_owned(),
                    HIDDEN_DIRECTORIES.to_owned(),
                ];
                for path in spec.exclude.iter().flatten() {
                    if !exact_path(path) {
                        return Err(format!("{path:?} is not a path a save may leave out"));
                    }
                    add.push(format!(":(exclude,literal){path}"));
                }
                run(&add.iter().map(String::as_str).collect::<Vec<_>>())?;
                let recorded = match &spec.record {
                    Some(record) => record_changes(&repo, record)?,
                    None => false,
                };
                if staged(&repo)? {
                    run(&["commit", "--quiet", "--no-verify", "-m", &spec.message])?;
                    if let Some(record) = spec.record.as_ref().filter(|_| recorded) {
                        // The entries were staged as blobs: Git writes them here too.
                        run(&[
                            "checkout",
                            "--quiet",
                            "HEAD",
                            "--",
                            &format!(":(literal){record}"),
                        ])?;
                    }
                }
            }
        }
        let saved = |landed: &str| -> Result<SafeHouseCheckpoint, Failure> {
            Ok(SafeHouseCheckpoint {
                remote: spec.remote.clone(),
                branch: landed.to_owned(),
                commit: repo.resolve("HEAD").unwrap_or_default(),
                base: Some(self.base.clone()),
            })
        };
        let Ok(head) = repo.resolve("HEAD") else {
            // An empty repository with nothing written: there is nothing to save.
            return saved(branch);
        };
        let tracking = format!("refs/remotes/{}/{branch}", spec.remote);
        if repo.resolve(&tracking).is_ok_and(|pushed| pushed == head) {
            return saved(branch);
        }
        let target = format!("HEAD:refs/heads/{branch}");
        let Err(refused) = run(&["push", "--quiet", &spec.remote, &target]) else {
            return saved(branch);
        };
        if let Some(files) = &spec.files {
            // An exact save never read its files from the working tree, which a replay needs to
            // match `HEAD` for them: Git writes them there, never through a link.
            sync_exactly(&repo, files)?;
        }
        let replayed = run(&[
            "fetch",
            "--quiet",
            "--no-tags",
            &spec.remote,
            &format!("refs/heads/{branch}"),
        ])
        .and_then(|_| run(&["rebase", "--quiet", "FETCH_HEAD"]));
        if replayed.is_err() {
            let _ = run(&["rebase", "--abort"]);
        } else if run(&["push", "--quiet", &spec.remote, &target]).is_ok() {
            return saved(branch);
        }
        let Some(fallback) = &spec.fallback else {
            return Err(refused);
        };
        run(&[
            "push",
            "--quiet",
            "--force",
            &spec.remote,
            &format!("HEAD:refs/heads/{fallback}"),
        ])?;
        saved(fallback)
    }

    /// Removes a clone whose work is all on its remote; keeps one holding anything else.
    pub(crate) fn finish_clone(&self) -> Result<SafeHouseReport, Failure> {
        let repo = self.repo();
        let dirty = !repo
            .run(&["status", "--porcelain", "--untracked-files=all"], &[])?
            .trim()
            .is_empty();
        let unpushed = match repo.resolve("HEAD") {
            Err(_) => Vec::new(),
            Ok(_) => repo
                .run(
                    &[
                        "log",
                        "--format=%h %s",
                        "HEAD",
                        "--not",
                        "--remotes=origin/*",
                    ],
                    &[],
                )?
                .lines()
                .map(str::to_owned)
                .collect(),
        };
        let kept = dirty || !unpushed.is_empty();
        if !kept {
            self.remove_clone()?;
        }
        Ok(SafeHouseReport {
            path: self.path.to_string_lossy().into_owned(),
            branch: self.branch.clone(),
            base: Some(self.base.clone()),
            kept,
            changes: unpushed,
        })
    }

    /// Removes the clone's working tree and its git directory.
    pub(crate) fn remove_clone(&self) -> Result<(), Failure> {
        remove_tree(&self.path)?;
        match &self.git_dir {
            Some(git_dir) if !git_dir.starts_with(&self.path) => remove_tree(git_dir),
            _ => Ok(()),
        }
    }
}

/// Removes a directory of a clone. Git writes its object files read-only, which Windows will not
/// delete, so their attributes are cleared first.
fn remove_tree(path: &Path) -> Result<(), Failure> {
    if !path.exists() {
        return Ok(());
    }
    if fs::remove_dir_all(path).is_ok() {
        return Ok(());
    }
    make_writable(path);
    fs::remove_dir_all(path).map_err(|error| fail(path, error))
}

fn make_writable(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            make_writable(&entry.path());
        }
    }
    let mut permissions = metadata.permissions();
    if permissions.readonly() && !metadata.file_type().is_symlink() {
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
}

/// The directory of a local repository: a path, or a `file://` URL's path.
fn local_repository(source: &str) -> Option<PathBuf> {
    let Some(rest) = source.strip_prefix("file://") else {
        return Some(PathBuf::from(source));
    };
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    // `file:///C:/x` names `C:/x` on Windows.
    let bytes = path.as_bytes();
    if bytes.len() > 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return Some(PathBuf::from(&path[1..]));
    }
    path.starts_with('/').then(|| PathBuf::from(path))
}

/// Makes `path` an empty bare repository whose `HEAD` names `branch`, private to the Station
/// user: on Unix the directory is made `0700` and Git keeps everything it writes there `0600`
/// (`core.sharedRepository=0600`), whatever the umask.
fn create_bare(path: &Path, branch: &str) -> Result<(), Failure> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| fail(parent, error))?;
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path).map_err(|error| fail(path, error))?;
    let git_dir = format!("--git-dir={}", path.to_string_lossy());
    git(
        parent,
        &[&git_dir, "init", "--quiet", "--bare", "--shared=0600"],
    )?;
    git(
        parent,
        &[
            &git_dir,
            "symbolic-ref",
            "HEAD",
            &format!("refs/heads/{branch}"),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai6::directorates::q::safehouse::SafeHouseClone;
    use std::process::Command;

    fn run(cwd: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn spec(source: &str, target: &Path, create: bool) -> SafeHouseSpec {
        SafeHouseSpec {
            operation_id: "assistant".to_owned(),
            mission_id: "conversation".to_owned(),
            title: "personal".to_owned(),
            source: source.to_owned(),
            base: "main".to_owned(),
            branch: "main".to_owned(),
            isolate: true,
            state_root: String::new(),
            reopen: None,
            checkpoint: None,
            clone: Some(SafeHouseClone {
                path: target.to_string_lossy().into_owned(),
                create_source: create,
                git_dir: None,
            }),
        }
    }

    fn save_spec(message: &str) -> CheckpointSpec {
        CheckpointSpec {
            remote: "origin".to_owned(),
            message: message.to_owned(),
            author_name: Some("Eve Moneypenny".to_owned()),
            author_email: Some("eve@example.invalid".to_owned()),
            branch: Some("main".to_owned()),
            fallback: Some("moneypenny/conversation".to_owned()),
            files: None,
            record: None,
            exclude: None,
        }
    }

    #[test]
    fn a_missing_local_repository_is_created_bare_and_the_first_save_makes_main() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("people").join("eve.git");
        let house = Worktree::open(&spec(
            &bare.to_string_lossy(),
            &root.path().join("one").join("personal"),
            true,
        ))
        .unwrap();
        assert!(bare.join("HEAD").exists(), "made bare");
        assert!(
            house.base.is_empty(),
            "an empty repository's main is unborn"
        );
        assert!(house.cloned && house.isolated);
        let nothing = house
            .save(&save_spec("moneypenny: nothing"), "main")
            .unwrap();
        assert_eq!(
            (nothing.branch.as_str(), nothing.commit.as_str()),
            ("main", "")
        );

        fs::create_dir_all(house.path.join("memory")).unwrap();
        fs::write(house.path.join("memory").join("MEMORY.md"), "# Memory\n").unwrap();
        let saved = house
            .save(
                &save_spec("moneypenny: start\n\nVia: Moneypenny (conversation c-1)"),
                "main",
            )
            .unwrap();
        assert_eq!(saved.branch, "main");
        let log = run(&bare, &["log", "--format=%an <%ae>|%B", "main"]);
        assert!(
            log.starts_with("Eve Moneypenny <eve@example.invalid>|moneypenny: start"),
            "{log}"
        );
        assert!(log.contains("Via: Moneypenny (conversation c-1)"), "{log}");
        assert_eq!(
            run(&bare, &["ls-tree", "-r", "--name-only", "main"]).trim(),
            "memory/MEMORY.md"
        );
        assert_eq!(
            house
                .save(&save_spec("moneypenny: again"), "main")
                .unwrap()
                .commit,
            saved.commit,
            "nothing new: nothing pushed"
        );

        // The next clone starts from what was saved, and an existing source is never re-made.
        let next = Worktree::open(&spec(
            &bare.to_string_lossy(),
            &root.path().join("two").join("personal"),
            true,
        ))
        .unwrap();
        assert_eq!(next.base, saved.commit);
        assert!(next.path.join("memory").join("MEMORY.md").exists());
        next.discard().unwrap();
        assert!(!next.path.exists());
        house.discard().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_created_repository_is_private_to_the_station_user() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("people").join("eve.git");
        let house = Worktree::open(&spec(
            &bare.to_string_lossy(),
            &root.path().join("personal"),
            true,
        ))
        .unwrap();
        fs::write(house.path.join("notes.md"), "notes\n").unwrap();
        house.save(&save_spec("moneypenny: notes"), "main").unwrap();
        // Everything in it, the pushed objects too, is the Station user's alone.
        let mut open = Vec::new();
        let mut pending = vec![bare.clone()];
        while let Some(path) = pending.pop() {
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.permissions().mode() & 0o077 != 0 {
                open.push(format!(
                    "{} {:o}",
                    path.display(),
                    metadata.permissions().mode()
                ));
            }
            if metadata.is_dir() {
                pending.extend(fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            }
        }
        assert!(open.is_empty(), "{open:#?}");
        house.discard().unwrap();
    }

    #[test]
    fn a_moved_branch_is_replayed_on_and_a_conflict_goes_to_the_fallback() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("eve.git");
        let source = bare.to_string_lossy().into_owned();
        let a = Worktree::open(&spec(&source, &root.path().join("a"), true)).unwrap();
        fs::write(a.path.join("shared.md"), "one\n").unwrap();
        a.save(&save_spec("moneypenny: one"), "main").unwrap();
        let b = Worktree::open(&spec(&source, &root.path().join("b"), false)).unwrap();

        // Another conversation moved main: a different file replays cleanly.
        fs::write(a.path.join("a.md"), "a\n").unwrap();
        a.save(&save_spec("moneypenny: a"), "main").unwrap();
        fs::write(b.path.join("b.md"), "b\n").unwrap();
        let replayed = b.save(&save_spec("moneypenny: b"), "main").unwrap();
        assert_eq!(replayed.branch, "main");
        let files = run(&bare, &["ls-tree", "-r", "--name-only", "main"]);
        assert!(files.contains("a.md") && files.contains("b.md"), "{files}");

        // The same lines changed both ways: the work goes to the fallback branch.
        fs::write(a.path.join("shared.md"), "from a\n").unwrap();
        a.save(&save_spec("moneypenny: a2"), "main").unwrap();
        fs::write(b.path.join("shared.md"), "from b\n").unwrap();
        let aside = b.save(&save_spec("moneypenny: b2"), "main").unwrap();
        assert_eq!(aside.branch, "moneypenny/conversation");
        assert_eq!(
            run(&bare, &["show", "main:shared.md"]).trim(),
            "from a",
            "main keeps the other conversation's work"
        );
        assert_eq!(
            run(&bare, &["show", "moneypenny/conversation:shared.md"]).trim(),
            "from b"
        );
        let mut strict = save_spec("moneypenny: b3");
        strict.fallback = None;
        fs::write(b.path.join("shared.md"), "from b again\n").unwrap();
        assert!(b.save(&strict, "main").is_err(), "no fallback: the refusal");
        a.discard().unwrap();
        b.discard().unwrap();
    }

    #[test]
    fn a_remote_must_exist_and_have_the_branch() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("missing.git");
        assert!(
            Worktree::open(&spec(
                &missing.to_string_lossy(),
                &root.path().join("x"),
                false
            ))
            .is_err(),
            "not created without create_source"
        );
        assert!(!missing.exists());
        let other = root.path().join("other");
        fs::create_dir_all(&other).unwrap();
        run(&other, &["init", "--quiet", "--initial-branch=trunk"]);
        fs::write(other.join("f"), "f").unwrap();
        run(&other, &["add", "f"]);
        run(&other, &["commit", "--quiet", "-m", "f"]);
        let refused = Worktree::open(&spec(
            &other.to_string_lossy(),
            &root.path().join("y"),
            false,
        ))
        .unwrap_err();
        assert!(refused.contains("no branch \"main\""), "{refused}");
        assert!(
            !root.path().join("y").exists(),
            "the failed clone is removed"
        );
        assert!(
            Worktree::open(&spec("ext::sh -c x", &root.path().join("z"), true)).is_err(),
            "a remote helper is refused before Git sees it"
        );
    }

    /// Writes an executable hook script that records it ran in `marker`.
    fn hook(path: &Path, marker: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            format!(
                "#!/bin/sh\necho ran > '{}'\nexit 0\n",
                marker.to_string_lossy().replace('\\', "/")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn a_separate_git_dir_leaves_no_dot_git_and_planted_programs_never_run() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("eve.git");
        let mut spec = spec(
            &bare.to_string_lossy(),
            &root.path().join("area").join("personal"),
            true,
        );
        let git_dir = root.path().join("repos").join("personal");
        if let Some(clone) = spec.clone.as_mut() {
            clone.git_dir = Some(git_dir.to_string_lossy().into_owned());
        }
        let house = Worktree::open(&spec).unwrap();
        assert!(
            !house.path.join(".git").exists(),
            "nothing of Git is left to write"
        );
        assert!(git_dir.join("HEAD").exists());

        // A repository whose configuration names hooks and a filesystem monitor.
        let hooks = root.path().join("planted");
        let marker = root.path().join("hook-ran");
        hook(&hooks.join("post-commit"), &marker);
        hook(&hooks.join("pre-commit"), &marker);
        let monitor = root.path().join("monitor-ran");
        hook(&hooks.join("monitor"), &monitor);
        let config = |key: &str, value: &str| {
            run(
                root.path(),
                &[
                    &format!("--git-dir={}", git_dir.to_string_lossy()),
                    "config",
                    key,
                    value,
                ],
            );
        };
        config(
            "core.hooksPath",
            &hooks.to_string_lossy().replace('\\', "/"),
        );
        config(
            "core.fsmonitor",
            &hooks.join("monitor").to_string_lossy().replace('\\', "/"),
        );
        // Plain Git runs the planted hook: the plant works.
        run(
            root.path(),
            &[
                &format!("--git-dir={}", git_dir.to_string_lossy()),
                &format!("--work-tree={}", house.path.to_string_lossy()),
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "control",
            ],
        );
        assert!(marker.exists(), "the planted hook runs under plain Git");
        fs::remove_file(&marker).unwrap();
        let _ = fs::remove_file(&monitor);

        fs::write(house.path.join("notes.md"), "notes\n").unwrap();
        let saved = house.save(&save_spec("moneypenny: notes"), "main").unwrap();
        assert_eq!(saved.branch, "main");
        assert!(!marker.exists(), "no hook ran during the save");
        assert!(
            !monitor.exists(),
            "no filesystem monitor ran during the save"
        );
        assert_eq!(run(&bare, &["show", "main:notes.md"]), "notes\n");
        house.discard().unwrap();
        assert!(!house.path.exists() && !git_dir.exists());
    }

    #[test]
    fn no_filter_runs_whatever_the_attributes_say() {
        let root = tempfile::tempdir().unwrap();
        let seed = root.path().join("seed");
        fs::create_dir_all(&seed).unwrap();
        run(&seed, &["init", "--quiet", "--initial-branch=main"]);
        fs::write(seed.join(".gitattributes"), "*.md filter=lfs\n").unwrap();
        run(&seed, &["add", ".gitattributes"]);
        run(&seed, &["commit", "--quiet", "-m", "attributes"]);
        run(
            root.path(),
            &["clone", "--quiet", "--bare", "seed", "eve.git"],
        );
        let bare = root.path().join("eve.git");
        let mut spec = spec(
            &bare.to_string_lossy(),
            &root.path().join("area").join("personal"),
            false,
        );
        let git_dir = root.path().join("repos").join("personal");
        if let Some(clone) = spec.clone.as_mut() {
            clone.git_dir = Some(git_dir.to_string_lossy().into_owned());
        }
        let house = Worktree::open(&spec).unwrap();

        // The clone's configuration names a filter program for `lfs`.
        let marker = root.path().join("filter-ran");
        let filter = root.path().join("planted").join("filter");
        fs::create_dir_all(filter.parent().unwrap()).unwrap();
        fs::write(
            &filter,
            format!(
                "#!/bin/sh\necho ran > '{}'\ncat\n",
                marker.to_string_lossy().replace('\\', "/")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&filter, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let command = format!("'{}'", filter.to_string_lossy().replace('\\', "/"));
        // `process`, which Git prefers, is what `git lfs install` sets; the script starts and
        // fails its handshake, which is enough to show it ran.
        for (key, value) in [
            ("filter.lfs.process", command.as_str()),
            ("filter.lfs.clean", command.as_str()),
            ("filter.lfs.smudge", command.as_str()),
            ("filter.lfs.required", "true"),
        ] {
            run(
                root.path(),
                &[
                    &format!("--git-dir={}", git_dir.to_string_lossy()),
                    "config",
                    key,
                    value,
                ],
            );
        }
        // Plain Git runs the planted filter: the plant works.
        fs::write(house.path.join("control.md"), "control\n").unwrap();
        let _ = Command::new("git")
            .arg("-C")
            .arg(&house.path)
            .arg(format!("--git-dir={}", git_dir.to_string_lossy()))
            .arg(format!("--work-tree={}", house.path.to_string_lossy()))
            .args(["hash-object", "control.md"])
            .output()
            .unwrap();
        assert!(marker.exists(), "the planted filter runs under plain Git");
        fs::remove_file(&marker).unwrap();
        fs::remove_file(house.path.join("control.md")).unwrap();

        fs::write(house.path.join("notes.md"), "notes\n").unwrap();
        house.save(&save_spec("moneypenny: notes"), "main").unwrap();
        assert!(!marker.exists(), "no filter ran during the save");
        assert_eq!(run(&bare, &["show", "main:notes.md"]), "notes\n");
        house.discard().unwrap();
    }

    #[test]
    fn hidden_paths_are_never_committed_by_a_save() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("eve.git");
        let house = Worktree::open(&spec(
            &bare.to_string_lossy(),
            &root.path().join("personal"),
            true,
        ))
        .unwrap();
        fs::write(house.path.join(".gitattributes"), "* filter=lfs\n").unwrap();
        fs::write(house.path.join(".lfsconfig"), "[lfs]\n").unwrap();
        fs::create_dir_all(house.path.join("notes").join(".cache")).unwrap();
        fs::write(house.path.join("notes").join(".x"), "x").unwrap();
        fs::write(house.path.join("notes").join(".cache").join("y"), "y").unwrap();
        let nothing = house
            .save(&save_spec("moneypenny: hidden only"), "main")
            .unwrap();
        assert_eq!(nothing.commit, "", "only hidden paths: nothing to save");
        fs::write(house.path.join("notes").join("a.md"), "a\n").unwrap();
        house.save(&save_spec("moneypenny: notes"), "main").unwrap();
        assert_eq!(
            run(&bare, &["ls-tree", "-r", "--name-only", "main"]).trim(),
            "notes/a.md"
        );
        house.discard().unwrap();
    }

    #[test]
    fn an_exact_save_takes_nothing_from_the_working_tree_and_replays_on_a_moved_branch() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("eve.git");
        let source = bare.to_string_lossy().into_owned();
        let a = Worktree::open(&spec(&source, &root.path().join("a"), true)).unwrap();
        fs::write(a.path.join("x.md"), "x\n").unwrap();
        a.save(&save_spec("moneypenny: x"), "main").unwrap();
        let b = Worktree::open(&spec(&source, &root.path().join("b"), false)).unwrap();
        fs::write(a.path.join("y.md"), "y\n").unwrap();
        a.save(&save_spec("moneypenny: y"), "main").unwrap();

        // Nothing of the change is in b's working tree, and something else is.
        fs::write(b.path.join("junk.md"), "junk\n").unwrap();
        let mut exact = save_spec("moneypenny: exact");
        exact.files = Some(vec![
            CheckpointFile {
                path: "memory/m.md".to_owned(),
                text: Some("m\n".to_owned()),
                base: None,
            },
            CheckpointFile {
                path: "x.md".to_owned(),
                text: None,
                base: None,
            },
        ]);
        let saved = b.save(&exact, "main").unwrap();
        assert_eq!(saved.branch, "main", "replayed on the moved main");
        let files = run(&bare, &["ls-tree", "-r", "--name-only", "main"]);
        assert_eq!(files.trim(), "memory/m.md\ny.md", "{files}");
        a.discard().unwrap();
        b.discard().unwrap();
    }

    #[test]
    fn a_save_leaves_out_what_it_is_told_to() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("eve.git");
        let house = Worktree::open(&spec(
            &bare.to_string_lossy(),
            &root.path().join("personal"),
            true,
        ))
        .unwrap();
        fs::create_dir_all(house.path.join("memory").join("deep")).unwrap();
        fs::write(house.path.join("memory").join("deep").join("m.md"), "m\n").unwrap();
        fs::write(house.path.join("memory").join("MEMORY.md"), "#\n").unwrap();
        fs::write(house.path.join("memoryless.md"), "kept\n").unwrap();
        let mut leaving = save_spec("moneypenny: work");
        leaving.exclude = Some(vec!["memory".to_owned()]);
        house.save(&leaving, "main").unwrap();
        assert_eq!(
            run(&bare, &["ls-tree", "-r", "--name-only", "main"]).trim(),
            "memoryless.md"
        );
        leaving.exclude = Some(vec!["../x".to_owned()]);
        fs::write(house.path.join("more.md"), "more\n").unwrap();
        assert!(house.save(&leaving, "main").is_err());
        house.discard().unwrap();
    }

    #[test]
    fn a_recording_save_names_every_file_it_adds_or_changes_in_the_same_commit() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("eve.git");
        let house = Worktree::open(&spec(
            &bare.to_string_lossy(),
            &root.path().join("personal"),
            true,
        ))
        .unwrap();
        fs::write(house.path.join("kept.md"), "kept\n").unwrap();
        fs::write(house.path.join("gone.md"), "gone\n").unwrap();
        house.save(&save_spec("moneypenny: first"), "main").unwrap();

        let mut recording = save_spec("moneypenny: second");
        recording.record = Some(".moneypenny/outside-text".to_owned());
        fs::create_dir_all(house.path.join("notes").join("deep")).unwrap();
        fs::write(house.path.join("notes").join("deep").join("a.md"), "a\n").unwrap();
        fs::write(house.path.join("kept.md"), "changed\n").unwrap();
        fs::remove_file(house.path.join("gone.md")).unwrap();
        fs::write(house.path.join(".hidden"), "never committed\n").unwrap();
        house.save(&recording, "main").unwrap();

        let entries = run(
            &bare,
            &[
                "show",
                "--name-only",
                "--format=",
                "main",
                "--",
                ".moneypenny/outside-text",
            ],
        );
        let mut named: Vec<String> = entries
            .lines()
            .map(|entry| run(&bare, &["show", &format!("main:{entry}")]))
            .collect();
        named.sort();
        assert_eq!(named, ["kept.md\n", "notes/deep/a.md\n"], "{entries}");
        assert!(
            house
                .path
                .join(".moneypenny")
                .join("outside-text")
                .read_dir()
                .unwrap()
                .count()
                >= 2,
            "the entries are in the working tree too"
        );
        house.discard().unwrap();
    }

    #[test]
    fn an_exact_save_is_refused_when_a_file_changed_since_its_base() {
        let hex = |text: &str| -> String {
            Sha256::digest(text.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        };
        let file = |path: &str, text: &str, base: Option<String>| CheckpointFile {
            path: path.to_owned(),
            text: Some(text.to_owned()),
            base,
        };
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("eve.git");
        let source = bare.to_string_lossy().into_owned();
        let a = Worktree::open(&spec(&source, &root.path().join("a"), true)).unwrap();
        let mut exact = save_spec("moneypenny: exact");
        // No branch yet: a file that must not exist does not.
        exact.files = Some(vec![file("m.md", "one\n", Some(String::new()))]);
        a.save(&exact, "main").unwrap();
        let b = Worktree::open(&spec(&source, &root.path().join("b"), false)).unwrap();

        exact.files = Some(vec![file("m.md", "two\n", Some(hex("one\n")))]);
        a.save(&exact, "main").unwrap();
        assert_eq!(run(&bare, &["show", "main:m.md"]), "two\n");

        // b's change was made from "one\n", which is gone: refused, whatever b's own HEAD says.
        exact.files = Some(vec![file("m.md", "three\n", Some(hex("one\n")))]);
        let refused = b.save(&exact, "main").unwrap_err();
        assert!(refused.contains("m.md changed on main"), "{refused}");
        exact.files = Some(vec![
            file("n.md", "new\n", Some(String::new())),
            file("m.md", "x\n", Some(String::new())),
        ]);
        assert!(b.save(&exact, "main").is_err(), "m.md exists now");
        assert_eq!(run(&bare, &["show", "main:m.md"]), "two\n");
        a.discard().unwrap();
        b.discard().unwrap();
    }

    #[test]
    fn a_committed_link_is_checked_out_as_a_plain_file() {
        let root = tempfile::tempdir().unwrap();
        let bare = root.path().join("eve.git");
        let seed = root.path().join("seed");
        fs::create_dir_all(&seed).unwrap();
        run(&seed, &["init", "--quiet", "--initial-branch=main"]);
        // A link to a directory outside any checkout, committed as Git keeps links.
        let blob = Command::new("git")
            .arg("-C")
            .arg(&seed)
            .args(["hash-object", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(b"../../outside")?;
                child.wait_with_output()
            })
            .unwrap();
        let blob = String::from_utf8_lossy(&blob.stdout).trim().to_owned();
        run(
            &seed,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("120000,{blob},memory"),
            ],
        );
        run(&seed, &["commit", "--quiet", "-m", "link"]);
        run(
            root.path(),
            &["clone", "--quiet", "--bare", "seed", "eve.git"],
        );
        #[cfg(unix)]
        {
            // Plain Git here makes it a link: the plant works.
            run(root.path(), &["clone", "--quiet", "eve.git", "control"]);
            assert!(
                fs::symlink_metadata(root.path().join("control").join("memory"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
        let house = Worktree::open(&spec(
            &bare.to_string_lossy(),
            &root.path().join("area").join("personal"),
            false,
        ))
        .unwrap();
        let memory = house.path.join("memory");
        let metadata = fs::symlink_metadata(&memory).unwrap();
        assert!(metadata.is_file(), "{metadata:?}");
        assert_eq!(fs::read_to_string(&memory).unwrap(), "../../outside");
        house.discard().unwrap();
    }

    #[test]
    fn branch_names_are_checked_as_git_checks_them_without_running_it() {
        let names = [
            "main",
            "moneypenny/0123abcd",
            "a.b/c-d_e",
            "",
            "@",
            "-x",
            "x.",
            "a..b",
            "a@{1}",
            "a b",
            "a~1",
            "a^",
            "a:b",
            "a?",
            "a*",
            "a[b",
            "a\\b",
            "a/.b",
            "a.lock",
            "a/b.lock/c",
            "a//b",
            "/a",
            "a/",
            "a\u{7}",
        ];
        let outside = tempfile::tempdir().unwrap();
        for name in names {
            let git = Command::new("git")
                .arg("-C")
                .arg(outside.path())
                .args(["check-ref-format", "--branch", name])
                .output()
                .unwrap();
            // Never one Git refuses; `@` alone, which Git reads as `HEAD`, is refused too.
            assert_eq!(
                branch_name(name),
                git.status.success() && name != "@",
                "{name:?}"
            );
        }
    }

    #[test]
    fn file_urls_name_local_paths() {
        assert_eq!(
            local_repository("/srv/a.git"),
            Some(PathBuf::from("/srv/a.git"))
        );
        assert_eq!(
            local_repository("file:///srv/a.git"),
            Some(PathBuf::from("/srv/a.git"))
        );
        assert_eq!(
            local_repository("file:///C:/srv/a.git"),
            Some(PathBuf::from("C:/srv/a.git"))
        );
        assert_eq!(local_repository("file://host/a.git"), None);
    }
}
