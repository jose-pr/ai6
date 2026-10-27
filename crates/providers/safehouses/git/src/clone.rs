// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A safe house that is a clone of a repository on one of its branches, saved by committing
//! in place and pushing to a branch the clone shares with everyone else who uses it.

use std::fs;
use std::path::{Path, PathBuf};

use ai6::RepoKind;
use ai6::directorates::q::safehouse::{
    CheckpointSpec, SafeHouseCheckpoint, SafeHouseReport, SafeHouseSpec,
};

use crate::worktree::{Failure, Worktree, fail, git, git_path, git_with, resolve};

/// A setting the commits of a save run with: no signing, which could wait for a passphrase.
const QUIET_COMMIT: [&str; 2] = ["-c", "commit.gpgsign=false"];

impl Worktree {
    /// Clones `spec.source` into `target` on branch `spec.base`, after creating the source as
    /// an empty bare repository when `create_source` says so and it is a local one that does
    /// not exist. A repository with no branch at all gives an unborn `base`; one without
    /// `base` but with other branches is refused.
    pub(crate) fn clone_into(
        spec: &SafeHouseSpec,
        target: &Path,
        create_source: bool,
    ) -> Result<Self, Failure> {
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
        git(&parent, &["check-ref-format", "--branch", &spec.base])
            .map_err(|_| format!("{:?} is not a branch name", spec.base))?;
        if create_source
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
        git(&parent, &["clone", "--quiet", "--", &spec.source, &name])?;
        let path = parent.join(&name);
        let tracking = format!("refs/remotes/origin/{}", spec.base);
        if resolve(&path, &tracking).is_ok() {
            git(
                &path,
                &[
                    "checkout",
                    "--quiet",
                    "-B",
                    &spec.base,
                    "--track",
                    &format!("origin/{}", spec.base),
                ],
            )?;
        } else if git(&path, &["for-each-ref", "--count=1", "refs/remotes"])?
            .trim()
            .is_empty()
        {
            git(
                &path,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{}", spec.base)],
            )?;
        } else {
            let _ = remove_clone(&path);
            return Err(format!("{} has no branch {:?}", spec.source, spec.base));
        }
        let base = resolve(&path, "HEAD").unwrap_or_default();
        Ok(Self {
            repository: path.clone(),
            path,
            branch: Some(spec.base.clone()),
            base,
            isolated: true,
            cloned: true,
        })
    }

    /// Commits everything in the working directory as `spec`'s author, then pushes `HEAD` to
    /// `branch` of `spec.remote` without force. A refused push is tried once more after the
    /// work is replayed on the remote branch; if that fails too, `HEAD` goes to
    /// `spec.fallback` (forced, since only this work uses it) or the refusal is the error.
    pub(crate) fn save(
        &self,
        spec: &CheckpointSpec,
        branch: &str,
    ) -> Result<SafeHouseCheckpoint, Failure> {
        for name in std::iter::once(branch).chain(spec.fallback.as_deref()) {
            git(&self.path, &["check-ref-format", "--branch", name])
                .map_err(|_| format!("{name:?} is not a branch name"))?;
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
        let quiet = |args: &[&str]| -> Vec<String> {
            QUIET_COMMIT
                .iter()
                .chain(args)
                .map(|arg| (*arg).to_owned())
                .collect()
        };
        let run = |args: &[&str]| {
            let args = quiet(args);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            git_with(&self.path, &args, &env)
        };
        let status = git(
            &self.path,
            &["status", "--porcelain", "--untracked-files=all"],
        )?;
        if !status.trim().is_empty() {
            run(&["add", "--all"])?;
            run(&["commit", "--quiet", "--no-verify", "-m", &spec.message])?;
        }
        let saved = |landed: &str| -> Result<SafeHouseCheckpoint, Failure> {
            Ok(SafeHouseCheckpoint {
                remote: spec.remote.clone(),
                branch: landed.to_owned(),
                commit: resolve(&self.path, "HEAD").unwrap_or_default(),
                base: Some(self.base.clone()),
            })
        };
        let Ok(head) = resolve(&self.path, "HEAD") else {
            // An empty repository with nothing written: there is nothing to save.
            return saved(branch);
        };
        let tracking = format!("refs/remotes/{}/{branch}", spec.remote);
        if resolve(&self.path, &tracking).is_ok_and(|pushed| pushed == head) {
            return saved(branch);
        }
        let target = format!("HEAD:refs/heads/{branch}");
        let Err(refused) = git(&self.path, &["push", "--quiet", &spec.remote, &target]) else {
            return saved(branch);
        };
        let replayed = git(
            &self.path,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                &spec.remote,
                &format!("refs/heads/{branch}"),
            ],
        )
        .and_then(|_| run(&["rebase", "--quiet", "FETCH_HEAD"]));
        if replayed.is_err() {
            let _ = git(&self.path, &["rebase", "--abort"]);
        } else if git(&self.path, &["push", "--quiet", &spec.remote, &target]).is_ok() {
            return saved(branch);
        }
        let Some(fallback) = &spec.fallback else {
            return Err(refused);
        };
        git(
            &self.path,
            &[
                "push",
                "--quiet",
                "--force",
                &spec.remote,
                &format!("HEAD:refs/heads/{fallback}"),
            ],
        )?;
        saved(fallback)
    }

    /// Removes a clone whose work is all on its remote; keeps one holding anything else.
    pub(crate) fn finish_clone(&self) -> Result<SafeHouseReport, Failure> {
        let dirty = !git(
            &self.path,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .trim()
        .is_empty();
        let unpushed = match resolve(&self.path, "HEAD") {
            Err(_) => Vec::new(),
            Ok(_) => git(
                &self.path,
                &[
                    "log",
                    "--format=%h %s",
                    "HEAD",
                    "--not",
                    "--remotes=origin/*",
                ],
            )?
            .lines()
            .map(str::to_owned)
            .collect(),
        };
        let kept = dirty || !unpushed.is_empty();
        if !kept {
            remove_clone(&self.path)?;
        }
        Ok(SafeHouseReport {
            path: self.path.to_string_lossy().into_owned(),
            branch: self.branch.clone(),
            base: Some(self.base.clone()),
            kept,
            changes: unpushed,
        })
    }
}

/// Removes a clone's directory. Git writes its object files read-only, which Windows will not
/// delete, so their attributes are cleared first.
pub(crate) fn remove_clone(path: &Path) -> Result<(), Failure> {
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

/// Makes `path` an empty bare repository whose `HEAD` names `branch`.
fn create_bare(path: &Path, branch: &str) -> Result<(), Failure> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| fail(parent, error))?;
    git(
        parent,
        &["init", "--quiet", "--bare", "--", &path.to_string_lossy()],
    )?;
    git(
        path,
        &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
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
