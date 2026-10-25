use std::fs;
use std::path::Path;
use std::process::Command;

use ai6::directorates::q::safehouse::{
    ChangeLog, SafeHouseInfo, SafeHouseProvider, SafeHouseReport, SafeHouseSpec,
};
use ai6::guatiao::{FromValue, ToValue};
use ai6_safehouse_git::Git;

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
    }
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
    let session = Git.open(&spec.to_value().unwrap()).unwrap();
    let info = SafeHouseInfo::from_value(&session.info().unwrap()).unwrap();
    assert!(info.isolated && info.supports_history);
    assert_eq!(
        info.branch.as_deref(),
        Some("G/mission-1-inspect-the-service")
    );
    let house = Path::new(&info.path);
    assert!(house.join("seed.txt").exists());
    assert!(
        ChangeLog::from_value(&session.changes().unwrap())
            .unwrap()
            .changes
            .is_empty()
    );

    fs::write(house.join("work.txt"), "work\n").unwrap();
    git(house, &["add", "work.txt"]);
    git(house, &["commit", "--quiet", "-m", "add work"]);
    let log = ChangeLog::from_value(&session.changes().unwrap()).unwrap();
    assert_eq!(log.changes.len(), 1);
    assert_eq!(log.changes[0].message.trim(), "add work");

    let report = SafeHouseReport::from_value(&session.finish().unwrap()).unwrap();
    assert!(report.kept);
    let reopened = Git
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
    let error = Git
        .open(&"not a spec".to_value().unwrap())
        .err()
        .expect("a string is not a spec");
    assert_eq!(error.status, ai6::guatiao::Status::GUATIAO_ERR_BAD_VALUE);
}
