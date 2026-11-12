//! The Mission board is built from the Registry's Session records, so a gateway that has just
//! started lists the Sessions that ended or wait to be checked, with live frames laid over them.

use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{NewSession, SessionResult, SessionStatus, SessionUpdate};
use ai6_diptel::{LifecycleEvent, SessionState};
use serde_json::Value;

use crate::controls::tests::{STATION_TOKEN, restarted, restarted_with, served};
use crate::{MissionId, OperationId, SessionId};

async fn get(base: &str, token: &str, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("{base}{path}"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
}

/// Records a Session of `mission` in `operation` held by `worker`, in `state`, closed when
/// `status` says so.
fn record(
    registry: &impl RegistryBackend,
    mission: &str,
    operation: &str,
    state: &str,
    status: SessionStatus,
) {
    let opened = registry
        .open_session(
            NewSession::new(format!("record-{mission}"), mission, "worker")
                .with_operation(operation)
                .with_provider_session(format!("session-{mission}")),
            Duration::from_secs(60),
        )
        .unwrap();
    registry
        .update_session(
            &opened.session_id,
            "worker",
            opened.fence,
            SessionUpdate::default()
                .result(SessionResult::new(state))
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
        .flat_map(|entry| entry["sessions"].as_array().unwrap())
        .map(|session| session["state"].as_str().unwrap_or("none"))
        .collect()
}

#[tokio::test]
async fn a_restarted_gateway_lists_ended_verifying_and_live_sessions_from_the_registry() {
    let served = served().await;
    record(
        &served.registry,
        "m-ended",
        "G",
        "done",
        SessionStatus::Ended,
    );
    record(
        &served.registry,
        "m-verifying",
        "G",
        "verifying",
        SessionStatus::Active,
    );
    record(
        &served.registry,
        "m-other",
        "L",
        "done",
        SessionStatus::Ended,
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
        .unwrap()["sessions"][0];
    assert_eq!(ended["station_id"], "worker");
    assert_eq!(ended["session_id"], "session-m-ended");
    // Another Operation's Session is not the viewer's to see, recorded or not.
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
    assert_eq!(detail["sessions"][0]["state"], "done");
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
        SessionStatus::Active,
    );
    record(
        &served.registry,
        "m-ended",
        "G",
        "done",
        SessionStatus::Ended,
    );
    let again = restarted(&served).await;
    for (mission, session) in [
        ("m-verifying", "session-m-verifying"),
        ("m-ended", "session-m-ended"),
    ] {
        let (operation, mission, session) = (
            OperationId::new("G").unwrap(),
            MissionId::new(mission).unwrap(),
            SessionId::new(session).unwrap(),
        );
        again
            .gateway
            .register_session(
                STATION_TOKEN,
                operation.clone(),
                mission.clone(),
                session.clone(),
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
                    session_id: session,
                    state: SessionState::Working,
                    reason: None,
                    sequence: 4,
                },
            )
            .await
            .unwrap();
    }

    let board = again.gateway.board_sessions().await;
    let find = |session: &str| {
        board
            .iter()
            .find(|entry| entry.session_id == session)
            .unwrap()
    };
    let live = find("session-m-verifying");
    assert_eq!(live.state.as_deref(), Some("working"));
    assert_eq!(live.sequence, 4);
    assert_eq!(live.station_id.as_deref(), Some("worker"));
    assert_eq!(find("session-m-ended").state.as_deref(), Some("done"));
    assert_eq!(board.len(), 3, "one entry per Session, however it is known");
}

#[tokio::test]
async fn the_board_keeps_the_most_recently_updated_records_within_its_bound() {
    let served = served().await;
    record(
        &served.registry,
        "m-ended",
        "G",
        "done",
        SessionStatus::Ended,
    );
    let mut limits = ai6::UnexLimits::default();
    limits.gateway.board_sessions = ai6::Limit::at(1);
    let again = restarted_with(&served, limits).await;

    assert_eq!(again.gateway.board_sessions().await.len(), 1);
}

#[tokio::test]
async fn a_recorded_mission_is_not_also_listed_as_pending() {
    let served = served().await;
    record(
        &served.registry,
        "m-ended",
        "G",
        "done",
        SessionStatus::Ended,
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
