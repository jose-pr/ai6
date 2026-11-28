use std::fs;
use std::path::Path;
use std::process::Command;

use ai6::directorates::q::safehouse::{
    ChangeLog, CheckpointSpec, SafeHouseCheckpoint, SafeHouseClone, SafeHouseDiff, SafeHouseInfo,
    SafeHouseProvider, SafeHouseReport, SafeHouseRun, SafeHouseSpec,
};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, ToValue};
use ai6_safehouse_git::{Git, GitConfig};

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?} failed");
}

fn spec(source: &Path, state: &Path) -> SafeHouseSpec {
    SafeHouseSpec {
        operation_id: "G".to_owned(),
        mission_id: "mission-1".to_owned(),
        title: "Inspect the service".to_owned(),
        source: source.to_string_lossy().into_owned(),
        base: "HEAD".to_owned(),
        branch: "{operation}/{id}-{slug}".to_owned(),
        isolate: true,
        state_root: state.to_string_lossy().into_owned(),
        reopen: None,
        checkpoint: None,
        clone: None,
    }
}

#[test]
fn a_clone_is_saved_to_its_shared_branch_through_the_kind() {
    let root = tempfile::tempdir().unwrap();
    let bare = root.path().join("people").join("eve.git");
    let target = root.path().join("conversation").join("personal");
    let spec = SafeHouseSpec {
        base: "main".to_owned(),
        clone: Some(SafeHouseClone {
            path: target.to_string_lossy().into_owned(),
            create_source: true,
            git_dir: None,
            depth: None,
            timeout_secs: None,
            max_bytes: None,
        }),
        ..spec(&bare, &root.path().join("state"))
    };
    let run = Git::default().open(&spec.to_value().unwrap()).unwrap();
    let info = SafeHouseInfo::from_value(&run.info().unwrap()).unwrap();
    // The provider reports the canonical path: a Windows short name or macOS /var is resolved.
    assert_eq!(
        fs::canonicalize(&info.path).unwrap(),
        fs::canonicalize(&target).unwrap()
    );
    assert_eq!(info.branch.as_deref(), Some("main"));
    fs::write(target.join("notes.md"), "notes\n").unwrap();
    let saved = SafeHouseCheckpoint::from_value(
        &run.checkpoint(
            &CheckpointSpec {
                remote: "origin".to_owned(),
                message: "moneypenny: notes".to_owned(),
                author_name: Some("Eve".to_owned()),
                author_email: Some("eve@example.invalid".to_owned()),
                branch: Some("main".to_owned()),
                fallback: None,
                files: None,
                exclude: None,
                max_bytes: None,
            }
            .to_value()
            .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(saved.branch, "main");
    assert!(!saved.commit.is_empty());
    let shown = Command::new("git")
        .arg("-C")
        .arg(&bare)
        .args(["show", "main:notes.md"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "notes\n");
    run.discard().unwrap();
    assert!(!target.exists());
}

#[test]
fn a_mission_works_in_a_worktree_and_its_commits_are_reported_through_the_kind() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["config", "user.name", "AI6 Tests"]);
    git(&repo, &["config", "user.email", "ai6@example.invalid"]);
    fs::write(repo.join("seed.txt"), "seed\n").unwrap();
    git(&repo, &["add", "seed.txt"]);
    git(&repo, &["commit", "--quiet", "-m", "seed"]);

    let spec = spec(&repo, &root.path().join("state"));
    let run = Git::default().open(&spec.to_value().unwrap()).unwrap();
    let info = SafeHouseInfo::from_value(&run.info().unwrap()).unwrap();
    assert!(info.isolated && info.supports_history);
    assert_eq!(
        info.branch.as_deref(),
        Some("G/mission-1-inspect-the-service")
    );
    let house = Path::new(&info.path);
    assert!(house.join("seed.txt").exists());
    assert!(
        ChangeLog::from_value(&run.changes().unwrap())
            .unwrap()
            .changes
            .is_empty()
    );

    fs::write(house.join("work.txt"), "work\n").unwrap();
    git(house, &["add", "work.txt"]);
    git(house, &["commit", "--quiet", "-m", "add work"]);
    let log = ChangeLog::from_value(&run.changes().unwrap()).unwrap();
    assert_eq!(log.changes.len(), 1);
    assert_eq!(log.changes[0].message.trim(), "add work");

    let report = SafeHouseReport::from_value(&run.finish().unwrap()).unwrap();
    assert!(report.kept);
    let reopened = Git::default()
        .open(
            &SafeHouseSpec {
                reopen: Some(report),
                ..spec
            }
            .to_value()
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        ChangeLog::from_value(&reopened.changes().unwrap())
            .unwrap()
            .changes
            .len(),
        1
    );
}

#[test]
fn a_malformed_spec_is_refused_as_a_bad_value() {
    let Err(error) = Git::default().open(&"not a spec".to_value().unwrap()) else {
        panic!("a string is not a spec");
    };
    assert_eq!(error.status, ai6::guatiao::Status::GUATIAO_ERR_BAD_VALUE);
}

/// `git_timeout_ms` is the deadline of every Git command: with none to spare, opening a
/// worktree is refused as timed out, and with the default it works.
#[test]
fn the_configured_timeout_stops_a_git_command() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("repo");
    fs::create_dir_all(&source).unwrap();
    git(&source, &["init", "--quiet", "--initial-branch=main"]);
    git(&source, &["config", "user.email", "a@example.invalid"]);
    git(&source, &["config", "user.name", "a"]);
    fs::write(source.join("a.txt"), "a\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "--quiet", "-m", "first"]);
    let spec = spec(&source, &root.path().join("state"));

    let impatient = Git::new(GitConfig {
        git_timeout_ms: 0,
        ..GitConfig::default()
    });
    let Err(error) = impatient.open(&spec.to_value().unwrap()) else {
        panic!("a command with no time must time out");
    };
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(Git::default().open(&spec.to_value().unwrap()).is_ok());
}

#[test]
fn a_mission_change_is_read_through_the_kind_uncommitted_and_new_files_included_and_cut() {
    let source = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    git(source.path(), &["init", "-b", "main"]);
    git(source.path(), &["config", "user.name", "Test"]);
    git(
        source.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    fs::write(source.path().join("a.txt"), "one\n").unwrap();
    git(source.path(), &["add", "."]);
    git(source.path(), &["commit", "-m", "base"]);

    let provider = Git::new(GitConfig::default());
    let run = provider
        .open(&spec(source.path(), state.path()).to_value().unwrap())
        .unwrap();
    let info = SafeHouseInfo::from_value(&run.info().unwrap()).unwrap();
    let house = Path::new(&info.path);
    git(house, &["config", "user.name", "Test"]);
    git(house, &["config", "user.email", "test@example.invalid"]);

    let read = |bytes: u64| SafeHouseDiff::from_value(&run.diff(bytes).unwrap()).unwrap();
    assert_eq!(read(4096).text, "", "nothing changed yet");

    fs::write(house.join("a.txt"), "one\ncommitted\n").unwrap();
    git(house, &["commit", "-am", "work"]);
    fs::write(house.join("a.txt"), "one\ncommitted\nuncommitted\n").unwrap();
    fs::write(house.join("new.txt"), "fresh\n").unwrap();

    let whole = read(4096);
    assert!(!whole.cut);
    assert!(whole.text.contains("+committed"), "{}", whole.text);
    assert!(whole.text.contains("+uncommitted"), "{}", whole.text);
    assert!(whole.text.contains("new.txt"), "{}", whole.text);

    let cut = read(10);
    assert!(cut.cut);
    assert_eq!(cut.text.len(), 10);
}

/// A bare remote with one commit on `main`, and a clone of it opened through the kind.
fn cloned_remote(
    root: &Path,
    provider: &Git,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    Object<dyn SafeHouseRun>,
) {
    let seed = root.join("seed");
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "--quiet", "--initial-branch=main"]);
    git(&seed, &["config", "user.name", "Test"]);
    git(&seed, &["config", "user.email", "test@example.invalid"]);
    fs::write(seed.join("a.txt"), "a\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "--quiet", "-m", "first"]);
    let bare = root.join("remote.git");
    git(
        root,
        &[
            "clone",
            "--quiet",
            "--bare",
            seed.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    let target = root.join("clone");
    let spec = SafeHouseSpec {
        base: "main".to_owned(),
        clone: Some(SafeHouseClone {
            path: target.to_string_lossy().into_owned(),
            create_source: false,
            git_dir: None,
            depth: None,
            timeout_secs: None,
            max_bytes: None,
        }),
        ..spec(&bare, &root.join("state"))
    };
    let run = provider.open(&spec.to_value().unwrap()).unwrap();
    (bare, target, run)
}

/// Rewrites the remote's `main` to a new root, as a history reset does.
fn reset_history(bare: &Path) {
    // Git's well-known empty tree.
    let tree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    let commit = Command::new("git")
        .arg("-C")
        .arg(bare)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit-tree",
            tree,
            "-m",
            "reset",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let commit = String::from_utf8_lossy(&commit.stdout).trim().to_owned();
    git(bare, &["update-ref", "refs/heads/main", &commit]);
}

fn save_main(run: &Object<dyn SafeHouseRun>) -> Result<SafeHouseCheckpoint, ProviderError> {
    run.checkpoint(
        &CheckpointSpec {
            remote: "origin".to_owned(),
            message: "work".to_owned(),
            author_name: Some("Eve".to_owned()),
            author_email: Some("eve@example.invalid".to_owned()),
            branch: Some("main".to_owned()),
            fallback: None,
            files: None,
            exclude: None,
            max_bytes: None,
        }
        .to_value()
        .unwrap(),
    )
    .map(|saved| SafeHouseCheckpoint::from_value(&saved).unwrap())
}

#[test]
fn a_branch_made_before_a_history_reset_is_refused_at_push() {
    let root = tempfile::tempdir().unwrap();
    let (bare, target, run) = cloned_remote(root.path(), &Git::default());
    fs::write(target.join("work.txt"), "work\n").unwrap();
    reset_history(&bare);
    let Err(error) = save_main(&run) else {
        panic!("work from before the reset must not be pushed");
    };
    let text = error.to_string();
    assert!(
        text.contains("shares no history") && text.contains("require_shared_root"),
        "{text}"
    );
    git(&bare, &["rev-parse", "--verify", "refs/heads/main"]);
    assert!(
        !Command::new("git")
            .arg("-C")
            .arg(&bare)
            .args(["cat-file", "-e", "main:work.txt"])
            .status()
            .unwrap()
            .success(),
        "nothing was pushed"
    );

    // An operator who weakens the check gets the push.
    let root = tempfile::tempdir().unwrap();
    let weak = Git::new(GitConfig {
        require_shared_root: false,
        ..GitConfig::default()
    });
    let (bare, target, run) = cloned_remote(root.path(), &weak);
    fs::write(target.join("work.txt"), "work\n").unwrap();
    reset_history(&bare);
    save_main(&run).expect("require_shared_root = false pushes");
}

#[test]
fn a_branch_that_shares_the_current_root_is_pushed() {
    let root = tempfile::tempdir().unwrap();
    let (bare, target, run) = cloned_remote(root.path(), &Git::default());
    fs::write(target.join("work.txt"), "work\n").unwrap();
    save_main(&run).unwrap();
    git(&bare, &["cat-file", "-e", "main:work.txt"]);
}

#[test]
fn a_worktree_based_on_a_branch_the_origin_has_reset_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let (bare, _target, _run) = cloned_remote(root.path(), &Git::default());
    let source = root.path().join("checkout");
    git(
        root.path(),
        &[
            "clone",
            "--quiet",
            bare.to_str().unwrap(),
            source.to_str().unwrap(),
        ],
    );
    reset_history(&bare);
    let spec = SafeHouseSpec {
        base: "main".to_owned(),
        ..spec(&source, &root.path().join("worktree-state"))
    };
    let Err(error) = Git::default().open(&spec.to_value().unwrap()) else {
        panic!("a start from the old root must be refused");
    };
    assert!(error.to_string().contains("shares no history"), "{error}");
    let weak = Git::new(GitConfig {
        require_shared_root: false,
        ..GitConfig::default()
    });
    assert!(weak.open(&spec.to_value().unwrap()).is_ok());
}
