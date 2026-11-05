//! What the user profile says a person holds: the action catalog, narrowed to them.

use crate::controls::tests::{Served, served_with};

async fn profile(served: &Served, token: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{}/v1/user/profile", served.base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn held(profile: &serde_json::Value, section: &str, key: &str, id: &str) -> Vec<String> {
    profile[section]
        .as_array()
        .unwrap()
        .iter()
        .find(|access| access[key] == id)
        .map(|access| serde_json::from_value(access["actions"].clone()).unwrap())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_person_sees_mission_review_only_when_they_hold_it() {
    let served = served_with(
        r#"
        [role.reviewer]
        actions = ["mission.read", "mission.review"]

        [role.viewer]
        actions = ["mission.read"]

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "reviewer"

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "viewer"
        "#,
    )
    .await;
    let reviewer = profile(&served, &served.operator).await;
    assert_eq!(
        held(&reviewer, "operations", "operation_id", "G"),
        ["mission.read", "mission.review"]
    );
    let viewer = profile(&served, &served.viewer).await;
    assert_eq!(
        held(&viewer, "operations", "operation_id", "G"),
        ["mission.read"]
    );
}

#[tokio::test]
async fn every_action_of_the_catalog_is_reachable_through_the_profile() {
    let list = |actions: &[&str]| {
        actions
            .iter()
            .map(|action| format!("{action:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let served = served_with(&format!(
        r#"
        [role.everything]
        actions = [{}]

        [role.station]
        actions = [{}]

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "everything"

        [[rule]]
        subject = "person:operator-007"
        scope = "station:worker"
        role = "station"
        "#,
        list(ai6::OPERATION_ACTIONS),
        list(ai6::STATION_ACTIONS),
    ))
    .await;
    let seen = profile(&served, &served.operator).await;
    assert_eq!(
        held(&seen, "operations", "operation_id", "G"),
        ai6::OPERATION_ACTIONS
    );
    assert_eq!(
        held(&seen, "stations", "station_id", "worker"),
        ai6::STATION_ACTIONS
    );
}

/// An action added to `ACTIONS` has to say where it means something, or this fails: the profile
/// reports those of an Operation and a Station, and the rest are held on what they act on.
#[test]
fn every_action_is_in_a_catalog_the_profile_reads_or_acts_elsewhere() {
    const ELSEWHERE: [&str; 6] = [
        "account.manage",
        "assistant.memory.admin",
        "compartment.manage",
        "compartment.read",
        "roster.manage",
        "roster.read",
    ];
    for action in ai6::ACTIONS {
        assert!(
            ai6::OPERATION_ACTIONS.contains(action)
                || ai6::STATION_ACTIONS.contains(action)
                || ELSEWHERE.contains(action),
            "{action} is in no catalog the profile reads"
        );
    }
    for action in ai6::STATION_ACTIONS {
        assert!(ai6::ACTIONS.contains(action), "{action}");
    }
}
