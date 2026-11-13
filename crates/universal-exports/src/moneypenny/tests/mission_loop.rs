//! Drafting a Mission in the conversation: the proposal, its one confirmation, and the receipt
//! that reopens the Mission it queued.

use super::*;

/// The operator's conversation holding a pending `propose_mission`; answers the socket, the job
/// and the proposal's ID.
async fn drafted(served: &Served) -> (Socket, AssistantJob, String) {
    let (mut app, job) = accepted(
        served,
        &served.operator,
        "Queue a Mission to fix the build.",
    )
    .await;
    let proposed = call_tool(
        served,
        &job.mcp_token,
        "propose_mission",
        json!({"operation_id": "G", "title": "Fix the build", "prompt": "Make CI green."}),
    )
    .await;
    assert_eq!(proposed["isError"], false, "{proposed}");
    let frames = finish(served, &mut app, &job).await;
    let proposal = proposal_of(&frames);
    assert_eq!(proposal["status"], "pending", "{proposal}");
    assert!(proposal.get("receipt").is_none(), "{proposal}");
    let id = proposal["id"].as_str().unwrap().to_owned();
    (app, job, id)
}

async fn confirm(app: &mut Socket, id: &str) {
    app.send(WsMessage::Text(
        json!({"type": "confirm", "proposal": id})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
}

/// Reads until the proposal settles or the gateway refuses the message.
async fn settled(app: &mut Socket) -> Vec<Value> {
    receive_until(app, |frame| {
        frame["type"] == "error"
            || (frame["type"] == "proposal"
                && ["done", "failed"].contains(&frame["proposal"]["status"].as_str().unwrap()))
    })
    .await
}

#[tokio::test]
async fn a_confirmed_mission_is_one_audited_outcome_with_a_receipt_that_reopens_it() {
    let served = served(AssistantConfig::default()).await;
    let (mut app, _job, id) = drafted(&served).await;

    // Two clicks send two confirms: the first runs, the second finds it taken.
    confirm(&mut app, &id).await;
    confirm(&mut app, &id).await;
    let frames = receive_until(&mut app, |frame| frame["type"] == "error").await;
    assert_eq!(frames.last().unwrap()["code"], "conflict", "{frames:?}");
    let frames = if frames
        .iter()
        .any(|frame| frame["type"] == "proposal" && frame["proposal"]["status"] == "done")
    {
        frames
    } else {
        settled(&mut app).await
    };
    let done = proposal_of(&frames);
    assert_eq!(done["status"], "done", "{done}");
    let mission = done["receipt"]["mission_id"].as_str().unwrap();
    assert_eq!(done["receipt"]["operation_id"], "G");
    assert_eq!(
        done["opens"],
        format!("app:/missions?operation_id=G&mission_id={mission}")
    );
    assert_eq!(audits(&served, "moneypenny.confirmed").len(), 1);

    // The Mission exists, and the person's own read of it finds it.
    let found = reqwest::Client::new()
        .get(format!(
            "{}/v1/operations/G/missions/{mission}",
            served.base
        ))
        .bearer_auth(&served.operator)
        .send()
        .await
        .unwrap();
    assert_eq!(found.status(), StatusCode::OK);

    // A reconnect shows the settled proposal with its receipt, and confirming it again is
    // still refused.
    let url = format!(
        "{}/v1/user/moneypenny",
        served.base.replacen("http", "ws", 1)
    );
    let (mut again, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    again
        .send(WsMessage::Text(
            json!({"type": "hello", "token": served.operator, "conversation": _job.conversation_id})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let ready = receive(&mut again).await;
    let shown = ready["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["role"] == "proposal")
        .unwrap();
    assert_eq!(shown["data"]["status"], "done", "{ready}");
    assert_eq!(shown["data"]["receipt"]["mission_id"], mission);
    confirm(&mut again, &id).await;
    let frames = receive_until(&mut again, |frame| frame["type"] == "error").await;
    assert_eq!(frames.last().unwrap()["code"], "conflict");
    assert_eq!(audits(&served, "moneypenny.confirmed").len(), 1);
}

#[tokio::test]
async fn a_stale_mission_proposal_is_refused_and_queues_nothing() {
    let served = served(AssistantConfig::default()).await;
    let (mut app, job, id) = drafted(&served).await;
    {
        let mut desk = served.assistant.desk();
        let held = desk
            .store
            .conversations
            .get_mut(&job.conversation_id)
            .unwrap();
        for proposal in &mut held.proposals {
            proposal.created_at -= 24.0 * 3600.0;
        }
    }
    confirm(&mut app, &id).await;
    // The refusal and the card's new state travel on different paths: take both.
    let mut frames = receive_until(&mut app, |frame| frame["type"] == "error").await;
    assert_eq!(frames.last().unwrap()["code"], "conflict", "{frames:?}");
    if frames.iter().all(|frame| frame["type"] != "proposal") {
        frames.extend(receive_until(&mut app, |frame| frame["type"] == "proposal").await);
    }
    let expired = proposal_of(&frames);
    assert_eq!(expired["status"], "expired", "{expired}");
    assert!(audits(&served, "moneypenny.confirmed").is_empty());
}

#[tokio::test]
async fn a_mission_proposal_the_person_may_no_longer_make_fails_without_a_receipt() {
    let served = served(AssistantConfig::default()).await;
    let (mut app, _job, id) = drafted(&served).await;
    served
        .denied
        .lock()
        .unwrap()
        .push("mission.create".to_owned());
    confirm(&mut app, &id).await;
    let frames = settled(&mut app).await;
    let failed = proposal_of(&frames);
    assert_eq!(failed["status"], "failed", "{failed}");
    assert!(failed.get("receipt").is_none(), "{failed}");
    assert_eq!(failed["opens"], "app:/missions?operation_id=G&state=queued");

    // And a person without the permission is never offered the proposal.
    let (_viewer, job) = accepted(&served, &served.viewer, "Queue a Mission.").await;
    let refused = call_tool(
        &served,
        &job.mcp_token,
        "propose_mission",
        json!({"operation_id": "G", "title": "t", "prompt": "p"}),
    )
    .await;
    assert_eq!(refused["isError"], true, "{refused}");
}
