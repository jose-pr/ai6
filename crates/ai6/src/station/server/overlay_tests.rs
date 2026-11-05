//! Overlays reaching a Mission's Session through the Station: what its handler is given, what a
//! commit holds, and what makes the Mission wait.

use std::path::Path;
use std::process::Command;

use super::tests::{RECORDED_RESUMES, RECORDED_STARTS, test_state};
use super::*;
use crate::directorates::q::overlays::read_overlay;
use crate::station::limits::OverlayLimits;
use crate::station::overlays::testing::{open_tree, overlay_dir, pin_of, release_store};

const SKILL: &str = "---\nname: review\ndescription: Reviews a change.\n---\n\nHow to review.\n";

/// A Station with Operation `G` on a real Git checkout, Agent `007` on the recording handler at
/// `clearance`, and a loadout issuing the overlay `tools` from `dir`, pinned.
fn overlay_state(root: &Path, dir: &Path, clearance: &str, mission: &str) -> DaemonState {
    let mut state = test_state(
        root,
        "workspace = { provider = \"mock-plain\", isolate = false }\n",
        &format!("[agent.007]\nhandler = \"mock-recorder\"\nclearance = {clearance:?}\n"),
    );
    let checkout = root.join("checkout");
    for arguments in [
        &["init", "-q"][..],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "start",
        ],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(arguments)
                .status()
                .unwrap()
                .success()
        );
    }
    let catalog = format!(
        "[overlay.tools]\npath = {:?}\npin = {:?}\n\n[loadout.007]\noverlays = [\"tools\"]\n",
        dir.to_string_lossy(),
        pin_of(dir).to_string(),
    )
    .parse::<crate::directorates::q::LoadoutCatalog>()
    .unwrap();
    state.station = state
        .station
        .clone()
        .with_loadouts(catalog)
        .with_state_dir(root.join("state"));
    state
        .missions
        .lock()
        .unwrap()
        .create(
            MissionId::try_new(mission).unwrap(),
            Some("G".to_owned()),
            "work".to_owned(),
            Some("007".to_owned()),
            None,
        )
        .unwrap();
    state
}

fn tools(root: &Path) -> std::path::PathBuf {
    overlay_dir(
        root,
        "source-tools",
        &[
            ("bin/ship-helper", "#!/bin/sh\necho shipped\n"),
            ("skills/review/SKILL.md", SKILL),
            ("rules/always.md", "Always-on rule of the overlay.\n"),
        ],
    )
}

async fn start(state: &DaemonState, mission: &str) -> Result<(), String> {
    start_mission(
        State(state.clone()),
        axum::extract::Path(mission.to_owned()),
        Json(StartMission {
            working_directory: None,
            initial_context: Vec::new(),
            environment: BTreeMap::new(),
        }),
    )
    .await
    .map(drop)
    .map_err(|error| error.1.0.error.clone())
}

fn recorded_start(mission: &str) -> AgentStart {
    RECORDED_STARTS
        .lock()
        .unwrap()
        .iter()
        .find(|request| request.mission_id.as_str() == mission)
        .cloned()
        .expect("the handler was started")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mission_gets_its_overlays_outside_the_checkout_and_a_commit_holds_none_of_them() {
    let root = tempfile::tempdir().unwrap();
    let mission = format!("overlay-commit-{}", std::process::id());
    let dir = tools(root.path());
    let state = overlay_state(root.path(), &dir, "field", &mission);
    start(&state, &mission).await.unwrap();
    let request = recorded_start(&mission);

    let overlay_root = request.environment["TOOLS_OVERLAY_ROOT"].clone();
    assert_eq!(request.overlays.roots, [overlay_root.clone()]);
    let bin = Path::new(&overlay_root).join("bin");
    assert!(
        request.environment["PATH"].contains(&*bin.to_string_lossy()),
        "{:?}",
        request.environment.get("PATH")
    );
    assert!(bin.join("ship-helper").is_file());
    assert_eq!(request.overlays.skills[0].name, "review");
    assert!(request.overlays.plugin_dir.is_some());
    assert!(
        request
            .initial_context
            .iter()
            .any(|part| part.contains("Always-on rule of the overlay.")),
        "{:?}",
        request.initial_context
    );

    let work = Path::new(&request.working_directory);
    assert!(!Path::new(&overlay_root).starts_with(work));
    let git = |arguments: &[&str]| {
        let output = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .arg("-C")
            .arg(work)
            .args(arguments)
            .output()
            .unwrap();
        assert!(output.status.success(), "{arguments:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    fs::write(work.join("work.txt"), "the Session's work\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "the Session's commit"]);
    let committed = git(&["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(
        committed.trim(),
        "work.txt",
        "the commit holds only the work"
    );

    assert_eq!(
        read_overlay(Path::new(&overlay_root), &OverlayLimits::default())
            .unwrap()
            .digest
            .digest,
        pin_of(&dir)
    );
    state.stopping.store(true, Ordering::Release);
    release_store(&root.path().join("state"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_desk_session_gets_the_text_and_the_root_and_nothing_to_run() {
    let root = tempfile::tempdir().unwrap();
    let mission = format!("overlay-desk-{}", std::process::id());
    let dir = tools(root.path());
    let state = overlay_state(root.path(), &dir, "desk", &mission);
    start(&state, &mission).await.unwrap();
    let request = recorded_start(&mission);
    assert!(request.environment.contains_key("TOOLS_OVERLAY_ROOT"));
    assert!(
        !request.environment.contains_key("PATH"),
        "no bin/ on a PATH"
    );
    assert!(
        request
            .initial_context
            .iter()
            .any(|part| part.contains("Always-on rule of the overlay."))
    );
    state.stopping.store(true, Ordering::Release);
    release_store(&root.path().join("state"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tampered_store_keeps_the_mission_waiting_with_the_reason_and_no_harness_starts() {
    let root = tempfile::tempdir().unwrap();
    let dir = tools(root.path());
    let first = format!("overlay-tamper-a-{}", std::process::id());
    let state = overlay_state(root.path(), &dir, "field", &first);
    start(&state, &first).await.unwrap();
    let stored = recorded_start(&first).environment["TOOLS_OVERLAY_ROOT"].clone();
    state.stopping.store(true, Ordering::Release);

    open_tree(Path::new(&stored));
    fs::write(Path::new(&stored).join("bin/ship-helper"), "planted\n").unwrap();

    let second = format!("overlay-tamper-b-{}", std::process::id());
    state
        .missions
        .lock()
        .unwrap()
        .create(
            MissionId::try_new(&second).unwrap(),
            Some("G".to_owned()),
            "work".to_owned(),
            Some("007".to_owned()),
            None,
        )
        .unwrap();
    let refused = start(&state, &second).await.err().unwrap();
    assert!(
        refused.contains("changed after it was fetched"),
        "{refused}"
    );
    let mission = state
        .missions
        .lock()
        .unwrap()
        .get(&second)
        .cloned()
        .unwrap();
    assert_eq!(mission.state, MissionState::Queued);
    assert!(
        mission
            .waiting
            .as_deref()
            .is_some_and(|reason| reason.contains("changed after it was fetched")),
        "{:?}",
        mission.waiting
    );
    assert!(
        RECORDED_STARTS
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.mission_id.as_str() != second),
        "no harness was started"
    );
    release_store(&root.path().join("state"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resume_gets_the_same_digests_or_the_change_is_on_the_timeline() {
    let root = tempfile::tempdir().unwrap();
    let mission = format!("overlay-resume-{}", std::process::id());
    let dir = tools(root.path());
    let state = overlay_state(root.path(), &dir, "field", &mission);
    start(&state, &mission).await.unwrap();
    tests::wait_for_state(&state, &mission, MissionState::Blocked).await;

    let resume = || {
        resume_mission(
            State(state.clone()),
            axum::extract::Path(mission.clone()),
            Json(ResumeMission {
                message: Some("carry on".to_owned()),
                environment: BTreeMap::new(),
            }),
        )
    };
    let changes = || {
        state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "overlay" && event.action == "changed")
            .count()
    };
    let _ = resume()
        .await
        .unwrap_or_else(|error| panic!("resume failed: {:?}", error.1.0.error));
    tests::wait_for_state(&state, &mission, MissionState::Blocked).await;
    let resumed = RECORDED_RESUMES
        .lock()
        .unwrap()
        .iter()
        .rfind(|request| request.mission_id.as_str() == mission)
        .cloned()
        .unwrap();
    assert_eq!(
        resumed.environment["TOOLS_OVERLAY_ROOT"],
        recorded_start(&mission).environment["TOOLS_OVERLAY_ROOT"]
    );
    assert_eq!(resumed.overlays.skills[0].name, "review");
    assert!(
        resumed
            .initial_context
            .iter()
            .any(|part| part.contains("Always-on rule of the overlay."))
    );
    assert_eq!(changes(), 0, "the same digests say nothing");

    // The operator pins other bytes: the Session gets them, and the timeline says so.
    let other = overlay_dir(root.path(), "source-other", &[("kb/new.md", "new\n")]);
    let mut station = state.station.clone();
    station = station.with_loadouts(
        format!(
            "[overlay.tools]\npath = {:?}\npin = {:?}\n\n[loadout.007]\noverlays = [\"tools\"]\n",
            other.to_string_lossy(),
            pin_of(&other).to_string(),
        )
        .parse::<crate::directorates::q::LoadoutCatalog>()
        .unwrap(),
    );
    let mut changed_state = state.clone();
    changed_state.station = station;
    let _ = resume_mission(
        State(changed_state.clone()),
        axum::extract::Path(mission.clone()),
        Json(ResumeMission {
            message: Some("carry on".to_owned()),
            environment: BTreeMap::new(),
        }),
    )
    .await
    .unwrap_or_else(|error| panic!("resume failed: {:?}", error.1.0.error));
    tests::wait_for_state(&state, &mission, MissionState::Blocked).await;
    assert_eq!(changes(), 1, "the timeline names the change");
    let event = state
        .intake
        .timeline_events("G")
        .unwrap()
        .into_iter()
        .find(|event| event.action == "changed" && event.kind == "overlay")
        .unwrap();
    assert!(event.detail["change"].contains(&pin_of(&other).to_string()));
    state.stopping.store(true, Ordering::Release);
    release_store(&root.path().join("state"));
}
