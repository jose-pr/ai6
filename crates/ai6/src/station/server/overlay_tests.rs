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
    assert_eq!(request.overlays.roots, std::slice::from_ref(&overlay_root));
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

/// Overlays on a Theater reached through a transport: a POSIX host whose harness runs under the
/// detached wrapper, as over SSH, and what the Session's own commands see there.
#[cfg(unix)]
mod theater {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::super::tests::shell_station;
    use super::*;
    use crate::directorates::m::theaters::{Theater, TheaterAccess};

    /// A Station whose Operation `G` is on the Theater `box`, Agent `007` on the line-printing
    /// harness with `path` as its PATH, and a loadout issuing `tools` from `dir`.
    fn on_theater(root: &Path, dir: &Path, path: &str, extra: &str) -> DaemonState {
        let work = root.join("work");
        fs::create_dir_all(&work).unwrap();
        let theater = Theater::new(
            "box",
            TheaterAccess::Transport(crate::TransportAccess::new("shell")),
        )
        .with_harness("mock-lines")
        .with_workdir(work.to_string_lossy().into_owned());
        let (mut state, _link, _forwards) = shell_station(root, theater, "mock-lines", None);
        fs::write(
            root.join("agents.toml"),
            format!(
                "[agent.007]\nhandler = \"mock-lines\"\nclearance = \"field\"\n\n\
                 [agent.007.env]\nPATH = {path:?}\n"
            ),
        )
        .unwrap();
        state.roster = Arc::new(Roster::load(root).unwrap());
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
        if !extra.is_empty() {
            let mut limits = (*state.station.limits()).clone();
            limits.overlays = crate::directorates::m::records::from_toml(extra).unwrap();
            state.station.limits = Arc::new(limits);
        }
        state
    }

    fn queue(state: &DaemonState, mission: &str) {
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
    }

    /// Starts `mission` with `script` as its harness and answers the lines it printed.
    async fn run(state: &DaemonState, mission: &str, script: &str) -> Result<Vec<String>, String> {
        queue(state, mission);
        start_mission(
            State(state.clone()),
            axum::extract::Path(mission.to_owned()),
            Json(StartMission {
                working_directory: None,
                initial_context: Vec::new(),
                environment: BTreeMap::from([("MOCK_LINES_SCRIPT".to_owned(), script.to_owned())]),
            }),
        )
        .await
        .map(drop)
        .map_err(|error| error.1.0.error.clone())?;
        let mut lines = Vec::new();
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                match pop_event(state, mission) {
                    Some(event) if event.kind == "completed" => break,
                    Some(event) => lines.extend(event.text),
                    None => tokio::time::sleep(Duration::from_millis(5)).await,
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the Session did not complete: {lines:?}"));
        Ok(lines)
    }

    /// A directory holding links to `programs` from the host's PATH, and nothing else: a Theater
    /// with no Python, as far as a Session's PATH can tell.
    fn bare_path(root: &Path, programs: &[&str]) -> PathBuf {
        let dir = root.join("bare-path");
        fs::create_dir_all(&dir).unwrap();
        for program in programs {
            let found = crate::directorates::section_00::programs::find_program(
                program,
                &BTreeMap::from([("PATH".to_owned(), std::env::var("PATH").unwrap())]),
            )
            .unwrap_or_else(|| panic!("{program} is not on this host"));
            std::os::unix::fs::symlink(found, dir.join(program)).unwrap();
        }
        dir
    }

    fn python_here() -> bool {
        Command::new("python3")
            .args(["-m", "venv", "--help"])
            .output()
            .is_ok_and(|output| output.status.success())
    }

    fn host_path() -> String {
        std::env::var("PATH").unwrap()
    }

    /// A deployment's report: helpers staged by hand on each Station. A helper in an overlay's
    /// `bin/` reaches the Mission's PATH on a Theater with nothing staged.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_helper_in_bin_reaches_the_missions_path_on_a_theater_with_nothing_staged() {
        let root = tempfile::tempdir().unwrap();
        let dir = overlay_dir(
            root.path(),
            "source-tools",
            &[
                (
                    "bin/ship-helper",
                    "#!/bin/sh\necho shipped-by-the-overlay\n",
                ),
                ("rules/always.md", "Always-on rule.\n"),
            ],
        );
        let state = on_theater(root.path(), &dir, &host_path(), "");
        let mission = format!("overlay-helper-{}", std::process::id());
        let lines = run(
            &state,
            &mission,
            "ship-helper; echo \"root $TOOLS_OVERLAY_ROOT\"; echo \"where $(command -v ship-helper)\"; \
             { echo planted > \"$TOOLS_OVERLAY_ROOT/planted\"; } 2>/dev/null && echo wrote; sleep 0.2",
        )
        .await
        .unwrap();
        assert_eq!(lines[0], "shipped-by-the-overlay", "{lines:?}");
        let home = root.path().join("home");
        let tree = home
            .join(".ai6/overlays/trees")
            .join(&pin_of(&dir).to_string()["sha256:".len()..]);
        assert_eq!(
            lines[1],
            format!("root {}", fs::canonicalize(&tree).unwrap().display()),
            "{lines:?}"
        );
        assert!(lines[2].ends_with("/bin/ship-helper"), "{lines:?}");
        assert!(!lines.iter().any(|line| line == "wrote"), "{lines:?}");
        assert_eq!(pin_of(&tree), pin_of(&dir));
        assert_eq!(
            fs::metadata(tree.join("bin/ship-helper"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o555
        );

        // A second Session reuses the copy: it is the same inode, not written again.
        use std::os::unix::fs::MetadataExt;
        let inode = fs::metadata(tree.join("bin/ship-helper")).unwrap().ino();
        let second = format!("overlay-helper-again-{}", std::process::id());
        let again = run(&state, &second, "ship-helper; sleep 0.2")
            .await
            .unwrap();
        assert_eq!(again[0], "shipped-by-the-overlay");
        assert_eq!(
            fs::metadata(tree.join("bin/ship-helper")).unwrap().ino(),
            inode
        );
        release_store(&root.path().join("state"));
        crate::station::overlays::release(&home.join(".ai6/overlays")).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_copy_changed_on_the_theater_refuses_the_next_start() {
        let root = tempfile::tempdir().unwrap();
        let dir = overlay_dir(
            root.path(),
            "source-tools",
            &[("bin/ship-helper", "#!/bin/sh\n")],
        );
        let state = on_theater(root.path(), &dir, &host_path(), "");
        let first = format!("overlay-theater-tamper-a-{}", std::process::id());
        run(&state, &first, "sleep 0.1").await.unwrap();
        let home = root.path().join("home");
        let tree = home
            .join(".ai6/overlays/trees")
            .join(&pin_of(&dir).to_string()["sha256:".len()..]);
        open_tree(&tree);
        fs::write(tree.join("bin/ship-helper"), "#!/bin/sh\necho planted\n").unwrap();
        let second = format!("overlay-theater-tamper-b-{}", std::process::id());
        let refused = run(&state, &second, "sleep 0.1").await.err().unwrap();
        assert!(
            refused.contains("has other bytes in bin/ship-helper"),
            "{refused}"
        );
        release_store(&root.path().join("state"));
    }

    /// One Theater, its PATH holding no Python: text runs on it, code waits with the reason.
    #[tokio::test(flavor = "multi_thread")]
    async fn text_runs_on_a_theater_with_no_python_and_code_waits_there_with_the_reason() {
        let root = tempfile::tempdir().unwrap();
        let bare = bare_path(
            root.path(),
            &[
                "sh", "head", "sleep", "cat", "rm", "mkfifo", "mkdir", "chmod", "find", "mv",
            ],
        );
        let text_only = overlay_dir(
            root.path(),
            "source-text",
            &[
                ("rules/always.md", "A rule.\n"),
                ("skills/review/SKILL.md", SKILL),
            ],
        );
        let state = on_theater(root.path(), &text_only, &bare.to_string_lossy(), "");
        let mission = format!("overlay-text-{}", std::process::id());
        let lines = run(&state, &mission, "echo ran; sleep 0.1").await.unwrap();
        assert_eq!(lines, ["ran"]);

        let root_two = tempfile::tempdir().unwrap();
        let bare = bare_path(root_two.path(), &["sh", "head"]);
        let with_code = overlay_dir(
            root_two.path(),
            "source-code",
            &[("lib/python/shiphelp.py", "VALUE = 1\n")],
        );
        let state = on_theater(root_two.path(), &with_code, &bare.to_string_lossy(), "");
        let parked = format!("overlay-code-{}", std::process::id());
        let refused = run(&state, &parked, "echo ran").await.err().unwrap();
        assert!(refused.contains("python3 or python"), "{refused}");
        assert!(refused.contains("lib/python"), "{refused}");
        let mission = state
            .missions
            .lock()
            .unwrap()
            .get(&parked)
            .cloned()
            .unwrap();
        assert_eq!(mission.state, MissionState::Queued);
        assert!(
            mission
                .waiting
                .as_deref()
                .is_some_and(|why| why.contains("python3 or python")),
            "{:?}",
            mission.waiting
        );
        release_store(&root.path().join("state"));
        release_store(&root_two.path().join("state"));
        crate::station::overlays::release(&root.path().join("home/.ai6/overlays")).unwrap();
        crate::station::overlays::release(&root_two.path().join("home/.ai6/overlays")).unwrap();
    }

    /// A stand-in for `dotagents.pyz`: Python runs any file by name, and this one runs a command
    /// module from `AGENTS_CMDS_PATH`, as the real one discovers it.
    const DOTAGENTS: &str = "import os, runpy, sys\n\
         print('home', os.environ['AGENTS_HOME'])\n\
         print('listed', len(os.listdir(os.environ['AGENTS_HOME'])))\n\
         for d in os.environ['AGENTS_CMDS_PATH'].split(os.pathsep):\n\
         \x20   runpy.run_path(os.path.join(d, sys.argv[1] + '.py'), run_name='__main__')\n";

    #[tokio::test(flavor = "multi_thread")]
    async fn python_imports_the_overlays_lib_and_dotagents_runs_its_command() {
        if !python_here() {
            eprintln!("skipped: this host has no python3 with venv");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let dir = overlay_dir(
            root.path(),
            "source-tools",
            &[
                ("lib/python/shiphelp.py", "VALUE = 'from the lib'\n"),
                (
                    "cmds/hello.py",
                    "import shiphelp\nprint('command says', shiphelp.VALUE)\n",
                ),
            ],
        );
        let pyz = root.path().join("dotagents.pyz");
        fs::write(&pyz, DOTAGENTS).unwrap();
        let pin = crate::directorates::q::overlays::Digest::of(DOTAGENTS.as_bytes());
        let state = on_theater(
            root.path(),
            &dir,
            &host_path(),
            &format!(
                "dotagents_pyz = {:?}\ndotagents_pin = {:?}\n",
                pyz.to_string_lossy(),
                pin.to_string()
            ),
        );
        let mission = format!("overlay-python-{}", std::process::id());
        let lines = run(
            &state,
            &mission,
            "\"$AGENTS_PYTHON\" -c 'import shiphelp; print(shiphelp.VALUE)'; dotagents hello; \
             env | grep -c '^DOTAGENTS_'; sleep 0.1",
        )
        .await
        .unwrap();
        assert_eq!(lines[0], "from the lib", "{lines:?}");
        assert!(lines[1].starts_with("home "), "{lines:?}");
        assert_eq!(
            lines[2], "listed 0",
            "its agents directory is empty: {lines:?}"
        );
        assert_eq!(lines[3], "command says from the lib", "{lines:?}");
        assert_eq!(lines[4], "0", "nothing of the Station's: {lines:?}");
        release_store(&root.path().join("state"));
        crate::station::overlays::release(&root.path().join("home/.ai6/overlays")).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn commands_with_no_dotagents_setting_wait_naming_it_and_a_wrong_pin_fails() {
        let root = tempfile::tempdir().unwrap();
        if !python_here() {
            eprintln!("skipped: this host has no python3 with venv");
            return;
        }
        let dir = overlay_dir(
            root.path(),
            "source-tools",
            &[("cmds/hello.py", "print(1)\n")],
        );
        let state = on_theater(root.path(), &dir, &host_path(), "");
        let mission = format!("overlay-no-pyz-{}", std::process::id());
        let refused = run(&state, &mission, "echo ran").await.err().unwrap();
        assert!(
            refused.contains("station.overlays.dotagents_pyz"),
            "{refused}"
        );
        release_store(&root.path().join("state"));
    }

    /// Setup in its phases: `install` with no record, nothing for the recorded digest, `sync` for
    /// another; its state read-only to the Session and kept across digests.
    #[tokio::test(flavor = "multi_thread")]
    async fn setup_runs_install_then_nothing_then_sync_and_its_state_is_read_only_to_a_session() {
        let root = tempfile::tempdir().unwrap();
        let script = "#!/bin/sh\nset -eu\necho \"$1 $AOS_SETUP_PHASE $AOS_OVERLAY_NAME\" >> \"$AOS_OVERLAY_STATE/phases\"\n";
        let manifest = "[[setup]]\ninterpreter = \"sh\"\npath = \"setup.sh\"\n";
        let first = overlay_dir(
            root.path(),
            "source-one",
            &[("overlay.cfg", manifest), ("setup.sh", script)],
        );
        let state = on_theater(root.path(), &first, &host_path(), "");
        let read = "cat \"$TOOLS_OVERLAY_STATE/phases\"; \
                    { echo x >> \"$TOOLS_OVERLAY_STATE/phases\"; } 2>/dev/null && echo wrote; sleep 0.1";
        let a = format!("overlay-setup-a-{}", std::process::id());
        let lines = run(&state, &a, read).await.unwrap();
        assert_eq!(lines, ["install install tools"]);
        let b = format!("overlay-setup-b-{}", std::process::id());
        let lines = run(&state, &b, read).await.unwrap();
        assert_eq!(
            lines,
            ["install install tools"],
            "the recorded digest runs nothing"
        );

        let second = overlay_dir(
            root.path(),
            "source-two",
            &[
                ("overlay.cfg", manifest),
                ("setup.sh", script),
                ("kb/new.md", "new\n"),
            ],
        );
        let mut state = state;
        state.station = state.station.clone().with_loadouts(
            format!(
                "[overlay.tools]\npath = {:?}\npin = {:?}\n\n[loadout.007]\noverlays = [\"tools\"]\n",
                second.to_string_lossy(),
                pin_of(&second).to_string(),
            )
            .parse::<crate::directorates::q::LoadoutCatalog>()
            .unwrap(),
        );
        let c = format!("overlay-setup-c-{}", std::process::id());
        let lines = run(&state, &c, read).await.unwrap();
        assert_eq!(lines, ["install install tools", "sync sync tools"]);
        let weakened: Vec<String> = state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "overlay" && event.action == "weakened")
            .map(|event| event.detail["weakening"].clone())
            .collect();
        assert!(
            weakened.iter().any(|id| id == "network-not-enforced"),
            "{weakened:?}"
        );
        release_store(&root.path().join("state"));
        crate::station::overlays::release(&root.path().join("home/.ai6/overlays")).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_setup_fails_the_start_naming_it_and_its_digest_is_not_recorded() {
        let root = tempfile::tempdir().unwrap();
        let dir = overlay_dir(
            root.path(),
            "source-tools",
            &[
                (
                    "overlay.cfg",
                    "[[setup]]\ninterpreter = \"sh\"\npath = \"setup.sh\"\n",
                ),
                ("setup.sh", "#!/bin/sh\necho it broke >&2\nexit 7\n"),
            ],
        );
        let state = on_theater(root.path(), &dir, &host_path(), "");
        let mission = format!("overlay-setup-fails-{}", std::process::id());
        let refused = run(&state, &mission, "echo ran").await.err().unwrap();
        assert!(refused.contains("exit 7: it broke"), "{refused}");
        assert!(refused.contains("not prepared"), "{refused}");
        assert!(
            !root
                .path()
                .join("state/overlays/records/theater-box/tools.json")
                .exists()
        );
        release_store(&root.path().join("state"));
        crate::station::overlays::release(&root.path().join("home/.ai6/overlays")).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_setup_past_its_timeout_is_stopped_with_what_it_started() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("child-ran-on");
        let script = format!(
            "#!/bin/sh\n(sleep 4; echo late > {:?}) &\nsleep 30\n",
            marker.to_string_lossy()
        );
        let dir = overlay_dir(
            root.path(),
            "source-tools",
            &[
                (
                    "overlay.cfg",
                    "[[setup]]\ninterpreter = \"sh\"\npath = \"setup.sh\"\ntimeout = 1\n",
                ),
                ("setup.sh", script.as_str()),
            ],
        );
        let state = on_theater(root.path(), &dir, &host_path(), "");
        let mission = format!("overlay-setup-slow-{}", std::process::id());
        let started = std::time::Instant::now();
        let refused = run(&state, &mission, "echo ran").await.err().unwrap();
        assert!(refused.contains("stopped at its time limit"), "{refused}");
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "{:?}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(!marker.exists(), "what it started ended with it");
        release_store(&root.path().join("state"));
        crate::station::overlays::release(&root.path().join("home/.ai6/overlays")).unwrap();
    }

    /// One environment per digest, built before the harness starts, and an unlocked file reported.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_requirements_file_gets_a_venv_of_its_digest_and_unlocked_is_reported() {
        if !python_here() {
            eprintln!("skipped: this host has no python3 with venv");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let dir = overlay_dir(
            root.path(),
            "source-tools",
            &[("lib/python/requirements.txt", "# nothing to fetch\n")],
        );
        let state = on_theater(root.path(), &dir, &host_path(), "");
        let mission = format!("overlay-venv-{}", std::process::id());
        let lines = run(
            &state,
            &mission,
            "echo \"$PYTHONPATH\"; echo \"$AGENTS_PYTHON\"; sleep 0.1",
        )
        .await
        .unwrap();
        let hex = pin_of(&dir).to_string()["sha256:".len()..].to_owned();
        assert!(
            lines[0].contains(&format!("envs/{hex}/python")),
            "{lines:?}"
        );
        assert!(lines[0].contains("site-packages"), "{lines:?}");
        assert!(!lines[1].is_empty());
        let reported: Vec<String> = state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "overlay" && event.action == "weakened")
            .map(|event| event.detail["weakening"].clone())
            .collect();
        for id in ["deps-network", "unlocked-dependencies"] {
            assert!(
                reported.iter().any(|found| found == id),
                "{id}: {reported:?}"
            );
        }
        release_store(&root.path().join("state"));
        crate::station::overlays::release(&root.path().join("home/.ai6/overlays")).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_partly_hashed_or_optioned_requirements_file_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let dir = overlay_dir(
            root.path(),
            "source-tools",
            &[(
                "lib/python/requirements.txt",
                "one==1 --hash=sha256:aa\ntwo==2\n",
            )],
        );
        let state = on_theater(root.path(), &dir, &host_path(), "");
        let mission = format!("overlay-partial-{}", std::process::id());
        let refused = run(&state, &mission, "echo ran").await.err().unwrap();
        assert!(refused.contains("partial-lock"), "{refused}");
        release_store(&root.path().join("state"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_env_script_runs_outside_the_session_and_what_it_prints_reaches_it() {
        let root = tempfile::tempdir().unwrap();
        let dir = overlay_dir(
            root.path(),
            "source-tools",
            &[(
                "env.sh",
                "#!/bin/sh\necho '{\"SHIP_FROM_SCRIPT\": \"computed\"}'\n",
            )],
        );
        let state = on_theater(root.path(), &dir, &host_path(), "");
        let mission = format!("overlay-env-script-{}", std::process::id());
        let lines = run(&state, &mission, "echo \"$SHIP_FROM_SCRIPT\"; sleep 0.1")
            .await
            .unwrap();
        assert_eq!(lines, ["computed"]);
        let reported = state
            .intake
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .any(|event| {
                event.kind == "overlay"
                    && event.detail.get("weakening").map(String::as_str) == Some("env-script")
            });
        assert!(reported);
        release_store(&root.path().join("state"));
        crate::station::overlays::release(&root.path().join("home/.ai6/overlays")).unwrap();
    }
}
