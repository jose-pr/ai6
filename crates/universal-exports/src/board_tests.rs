//! The Mission board is built from the Registry's Run records, so a gateway that has just
//! started lists the Runs that ended or wait to be checked, with live frames laid over them.

use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{NewRun, RunResult, RunStatus, RunUpdate};
use ai6_diptel::{LifecycleEvent, RunState};
use serde_json::Value;

use crate::controls::tests::{STATION_TOKEN, restarted, restarted_with, served};
use crate::{MissionId, OperationId, RunId};

async fn get(base: &str, token: &str, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{base}{path}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
}

/// Records a Run of `mission` in `operation` held by `worker`, in `state`, closed when
/// `status` says so.
fn record(
    registry: &impl RegistryBackend,
    mission: &str,
    operation: &str,
    state: &str,
    status: RunStatus,
) {
    let opened = registry
        .open_run(
            NewRun::new(format!("record-{mission}"), mission, "worker")
                .with_operation(operation)
                .with_provider_id(format!("run-{mission}")),
            Duration::from_secs(60),
        )
        .unwrap();
    registry
        .update_run(
            &opened.id,
            "worker",
            opened.fence,
            RunUpdate::default()
                .result(RunResult::new(state))
                .status(status),
        )
        .unwrap();
}

fn states<'a>(missions: &'a Value, mission: &str) -> Vec<&'a str> {
    missions
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["mission_id"] == mission)
        .flat_map(|entry| entry["runs"].as_array().unwrap())
        .map(|run| run["state"].as_str().unwrap_or("none"))
        .collect()
}

#[tokio::test]
async fn a_restarted_gateway_lists_ended_verifying_and_live_runs_from_the_registry() {
    let served = served().await;
    record(
        &served.registry,
        "m-ended",
        "G",
        "done",
        RunStatus::Ended,
    );
    record(
        &served.registry,
        "m-verifying",
        "G",
        "verifying",
        RunStatus::Active,
    );
    record(
        &served.registry,
        "m-other",
        "L",
        "done",
        RunStatus::Ended,
    );
    let again = restarted(&served).await;

    let listed = get(&again.base, &served.viewer, "/v1/missions").await;
    assert_eq!(listed.status(), 200);
    let missions: Value = listed.json().await.unwrap();

    assert_eq!(states(&missions, "m-ended"), ["done"]);
    assert_eq!(states(&missions, "m-verifying"), ["verifying"]);
    // Recorded as running with no result yet, and no frame received since the restart.
    assert_eq!(states(&missions, "mission-g"), ["none"]);
    let ended = &missions
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["mission_id"] == "m-ended")
        .unwrap()["runs"][0];
    assert_eq!(ended["station_id"], "worker");
    assert_eq!(ended["run_id"], "run-m-ended");
    // Another Operation's Run is not the viewer's to see, recorded or not.
    assert!(states(&missions, "m-other").is_empty());
    assert!(states(&missions, "mission-l").is_empty());

    let board: Value = get(&again.base, &served.viewer, "/v1/board")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(board.as_array().unwrap().len(), 3);

    // The Mission's own route finds it.
    let detail = get(
        &again.base,
        &served.viewer,
        "/v1/operations/G/missions/m-ended",
    )
    .await;
    assert_eq!(detail.status(), 200);
    let detail: Value = detail.json().await.unwrap();
    assert_eq!(detail["runs"][0]["state"], "done");
    let hidden = get(
        &again.base,
        &served.viewer,
        "/v1/operations/L/missions/m-other",
    )
    .await;
    assert_eq!(hidden.status(), 404);
}

#[tokio::test]
async fn live_frames_win_over_a_record_unless_it_has_closed() {
    let served = served().await;
    record(
        &served.registry,
        "m-verifying",
        "G",
        "verifying",
        RunStatus::Active,
    );
    record(
        &served.registry,
        "m-ended",
        "G",
        "done",
        RunStatus::Ended,
    );
    let again = restarted(&served).await;
    for (mission, run) in [
        ("m-verifying", "run-m-verifying"),
        ("m-ended", "run-m-ended"),
    ] {
        let (operation, mission, run) = (
            OperationId::new("G").unwrap(),
            MissionId::new(mission).unwrap(),
            RunId::new(run).unwrap(),
        );
        again
            .gateway
            .register_run(
                STATION_TOKEN,
                operation.clone(),
                mission.clone(),
                run.clone(),
            )
            .await
            .unwrap();
        again
            .gateway
            .publish_lifecycle(
                STATION_TOKEN,
                LifecycleEvent {
                    operation_id: operation,
                    mission_id: mission,
                    run_id: run,
                    state: RunState::Working,
                    reason: None,
                    sequence: 4,
                },
            )
            .await
            .unwrap();
    }

    let board = again.gateway.board_runs().await;
    let find = |run: &str| {
        board
            .iter()
            .find(|entry| entry.run_id == run)
            .unwrap()
    };
    let live = find("run-m-verifying");
    assert_eq!(live.state.as_deref(), Some("working"));
    assert_eq!(live.sequence, 4);
    assert_eq!(live.station_id.as_deref(), Some("worker"));
    assert_eq!(find("run-m-ended").state.as_deref(), Some("done"));
    assert_eq!(board.len(), 3, "one entry per Run, however it is known");
}

#[tokio::test]
async fn the_board_keeps_the_most_recently_updated_records_within_its_bound() {
    let served = served().await;
    record(
        &served.registry,
        "m-ended",
        "G",
        "done",
        RunStatus::Ended,
    );
    let mut limits = ai6::UnexLimits::default();
    limits.gateway.board_runs = ai6::Limit::at(1);
    let again = restarted_with(&served, limits).await;

    assert_eq!(again.gateway.board_runs().await.len(), 1);
}

#[tokio::test]
async fn a_recorded_mission_is_not_also_listed_as_pending() {
    let served = served().await;
    record(
        &served.registry,
        "m-ended",
        "G",
        "done",
        RunStatus::Ended,
    );
    served
        .registry
        .check_in(
            "worker",
            std::collections::BTreeMap::from([
                ("operations".to_owned(), serde_json::json!(["G", "L"])),
                (
                    "queue".to_owned(),
                    serde_json::json!([
                        {"mission_id": "m-ended", "operation_id": "G", "state": "queued"},
                        {"mission_id": "m-new", "operation_id": "G", "state": "queued"},
                    ]),
                ),
            ]),
        )
        .unwrap();
    let again = restarted(&served).await;

    let missions: Value = get(&again.base, &served.viewer, "/v1/missions")
        .await
        .json()
        .await
        .unwrap();
    let listed = |id: &str| {
        missions
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["mission_id"] == id)
            .count()
    };
    assert_eq!(listed("m-ended"), 1);
    assert_eq!(states(&missions, "m-ended"), ["done"]);
    assert_eq!(listed("m-new"), 1);
}
