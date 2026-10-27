use std::fs;
use std::path::Path;

use ai6::directorates::q::safehouse::{
    ChangeLog, SafeHouseInfo, SafeHouseProvider, SafeHouseReport, SafeHouseSpec,
};
use ai6::guatiao::{FromValue, ToValue};
use ai6_safehouse_copy::Copier;

fn spec(source: &Path, state: &Path) -> SafeHouseSpec {
    SafeHouseSpec {
        operation_id: "G".to_owned(),
        mission_id: "mission-1".to_owned(),
        title: "Tidy the notes".to_owned(),
        source: source.to_string_lossy().into_owned(),
        base: "HEAD".to_owned(),
        branch: "{id}".to_owned(),
        isolate: true,
        state_root: state.to_string_lossy().into_owned(),
        reopen: None,
        checkpoint: None,
        clone: None,
    }
}

fn describe(session: &ai6::guatiao::library::Object<dyn ai6::SafeHouseSession>) -> SafeHouseInfo {
    SafeHouseInfo::from_value(&session.info().unwrap()).unwrap()
}

#[test]
fn a_mission_works_in_a_copy_and_reports_no_history() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("notes");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("a.txt"), "a\n").unwrap();
    fs::write(source.join("nested").join("b.txt"), "b\n").unwrap();
    let state = root.path().join("state");

    let spec = spec(&source, &state);
    let session = Copier.open(&spec.to_value().unwrap()).unwrap();
    let info = describe(&session);
    assert!(info.isolated && !info.supports_history);
    assert!(info.branch.is_none() && info.base.is_none());
    let copy = Path::new(&info.path);
    assert!(!copy.starts_with(&source));
    // Canonicalize both sides: Windows temp paths may be 8.3 short names or verbatim.
    assert!(
        fs::canonicalize(copy)
            .unwrap()
            .starts_with(fs::canonicalize(&state).unwrap())
    );
    assert_eq!(fs::read_to_string(copy.join("a.txt")).unwrap(), "a\n");
    assert_eq!(
        fs::read_to_string(copy.join("nested").join("b.txt")).unwrap(),
        "b\n"
    );

    fs::write(copy.join("a.txt"), "changed\n").unwrap();
    assert_eq!(fs::read_to_string(source.join("a.txt")).unwrap(), "a\n");
    assert!(
        ChangeLog::from_value(&session.changes().unwrap())
            .unwrap()
            .changes
            .is_empty()
    );

    let report = SafeHouseReport::from_value(&session.finish().unwrap()).unwrap();
    assert!(report.kept && report.changes.is_empty());
    let reopened = Copier
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
        fs::read_to_string(Path::new(&info.path).join("a.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(describe(&reopened).path, info.path);
}

#[test]
fn working_in_place_never_copies_and_a_copy_is_not_made_twice() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("notes");
    fs::create_dir_all(&source).unwrap();
    let state = root.path().join("state");

    let in_place = SafeHouseSpec {
        isolate: false,
        ..spec(&source, &state)
    };
    let session = Copier.open(&in_place.to_value().unwrap()).unwrap();
    assert!(!describe(&session).isolated);
    assert!(!state.exists());

    let isolated = spec(&source, &state);
    assert!(Copier.open(&isolated.to_value().unwrap()).is_ok());
    assert!(Copier.open(&isolated.to_value().unwrap()).is_err());
}

#[test]
fn a_state_root_inside_the_source_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("notes");
    fs::create_dir_all(&source).unwrap();
    let inside = spec(&source, &source.join("state"));
    assert!(Copier.open(&inside.to_value().unwrap()).is_err());
}

#[test]
fn discard_removes_a_copy_and_never_the_source() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("notes");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("a.txt"), "a\n").unwrap();
    let state = root.path().join("state");

    let session = Copier
        .open(&spec(&source, &state).to_value().unwrap())
        .unwrap();
    let copy = describe(&session).path;
    assert!(
        session.exports().is_err(),
        "a copy cannot tell what is exported"
    );
    session.discard().unwrap();
    assert!(!Path::new(&copy).exists());
    session.discard().unwrap();

    let in_place = SafeHouseSpec {
        isolate: false,
        ..spec(&source, &state)
    };
    let session = Copier.open(&in_place.to_value().unwrap()).unwrap();
    assert!(session.discard().is_err());
    assert!(source.join("a.txt").exists());
}

#[test]
fn a_clone_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("notes");
    fs::create_dir_all(&source).unwrap();
    let cloned = SafeHouseSpec {
        clone: Some(ai6::directorates::q::safehouse::SafeHouseClone {
            path: root.path().join("clone").to_string_lossy().into_owned(),
            create_source: false,
            git_dir: None,
        }),
        ..spec(&source, &root.path().join("state"))
    };
    let refused = Copier.open(&cloned.to_value().unwrap()).err().unwrap();
    assert_eq!(
        refused.status,
        ai6::guatiao::Status::GUATIAO_ERR_UNSUPPORTED
    );
    assert!(!root.path().join("clone").exists());
}
