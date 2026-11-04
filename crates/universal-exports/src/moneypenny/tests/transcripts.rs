//! Her conversation log: each entry kept with the turn that ran it, no secret in it, and a
//! conversation that outlives the gateway.

use ai6::{AccountRef, AssistantLogEntry, AssistantRun, RouteRef};

use super::*;

/// The log of `conversation` once it holds an entry `ready` accepts, waiting for the writer.
async fn logged(
    served: &Served,
    conversation: &str,
    ready: impl Fn(&[AssistantLogEntry]) -> bool,
) -> Vec<AssistantLogEntry> {
    for _ in 0..600 {
        let rows = served.registry.assistant_log(conversation).unwrap();
        if ready(&rows) {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the log of {conversation} never held what was asked")
}

#[tokio::test]
async fn a_flush_with_no_log_started_returns_at_once() {
    let assistant = super::super::Assistant::new(AssistantConfig::default());
    tokio::time::timeout(Duration::from_secs(5), assistant.flush_log())
        .await
        .expect("nothing to wait for");
}

#[tokio::test]
async fn a_stopping_gateway_keeps_what_her_log_still_queued() {
    let served = served(AssistantConfig::default()).await;
    let (mut app, ready) = open(&served, &served.operator).await;
    let conversation = ready["conversation"].as_str().unwrap().to_owned();
    say(&mut app, "Keep this.", None).await;
    take(&served, "hq").await;
    served.assistant.flush_log().await;
    let kept = served.registry.assistant_log(&conversation).unwrap();
    assert_eq!(kept[0].text, "Keep this.", "kept before the flush returned");
}

/// A second gateway on the same Registry: the first one, restarted.
async fn restarted(served: &Served) -> (String, tokio::task::JoinHandle<()>) {
    let gateway = Gateway::with_access(
        GatewayConfig::new(
            "https://localhost",
            STATION_TOKEN,
            GatewayLimits::default(),
            Duration::from_secs(30),
            Duration::from_secs(30),
        )
        .unwrap(),
        Arc::new(served.registry.clone()),
        AccessProviders::new(
            Arc::new(People),
            Arc::new(People),
            Arc::new(access_from_toml(ROLES, ai6::Limit::UNBOUNDED).unwrap()),
        ),
    )
    .with_assistant(AssistantConfig::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = gateway.router();
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    (base, server)
}

/// Opens the app's socket at `base`, asking for `conversation`.
async fn reopen(base: &str, token: &str, conversation: &str) -> (Socket, Value) {
    let url = format!("{}/v1/user/moneypenny", base.replacen("http", "ws", 1));
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    socket
        .send(WsMessage::Text(
            json!({"type": "hello", "token": token, "conversation": conversation})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let ready = receive(&mut socket).await;
    (socket, ready)
}

#[tokio::test]
async fn a_pooled_turn_is_logged_with_its_pool_and_account_and_no_token_reaches_the_log() {
    let served = served(AssistantConfig::default()).await;
    let (mut app, ready) = open(&served, &served.operator).await;
    let conversation = ready["conversation"].as_str().unwrap().to_owned();
    say(&mut app, "Which Stations are busy?", None).await;
    let job = take(&served, "hq").await;
    assert_eq!(
        report(
            &served,
            "hq",
            &job,
            Some(&job.job_id),
            vec![AssistantEvent::new(EVENT_ACCEPTED, None)]
        )
        .await,
        StatusCode::NO_CONTENT
    );
    // A tool argument that carries her own tool token, and the person's access token.
    call_tool(
        &served,
        &job.mcp_token,
        "board",
        json!({"note": format!("{} and {}", job.mcp_token, served.operator)}),
    )
    .await;
    let route = RouteRef::new("claude", Some("sonnet".to_owned()), Some("team".to_owned()));
    let text = AssistantEvent::new(EVENT_TEXT, Some("Two are busy.".to_owned()));
    let pooled = AssistantReport {
        route: Some(route.clone()),
        run: Some(AssistantRun {
            account: Some(AccountRef::new("team", "alice")),
            model: Some("sonnet".to_owned()),
            tokens: Some(321),
        }),
        station_id: "hq".to_owned(),
        conversation_id: conversation.clone(),
        job_id: None,
        session_id: Some("provider-pooled".to_owned()),
        events: vec![text, AssistantEvent::new(EVENT_COMPLETED, None)],
    };
    let posted = reqwest::Client::new()
        .post(format!("{}/v1/station/assistant/reports", served.base))
        .bearer_auth(STATION_TOKEN)
        .json(&pooled)
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::NO_CONTENT);
    receive_until(&mut app, |frame| {
        frame["type"] == "busy" && frame["busy"] == false
    })
    .await;

    let rows = logged(&served, &conversation, |rows| {
        rows.iter().any(|row| row.kind == "moneypenny")
    })
    .await;
    let kinds: Vec<&str> = rows.iter().map(|row| row.kind.as_str()).collect();
    assert_eq!(
        kinds,
        ["person", "tool_call", "tool_result", "moneypenny"],
        "{rows:#?}"
    );
    assert!(rows.iter().all(|row| row.person == "operator-007"));
    let answer = &rows[3];
    assert_eq!(answer.text, "Two are busy.");
    let turn = answer.turn.as_ref().unwrap();
    assert_eq!(turn.station.as_deref(), Some("hq"));
    assert_eq!(turn.session.as_deref(), Some("provider-pooled"));
    assert_eq!(turn.agent.as_deref(), Some("moneypenny"));
    assert_eq!(
        turn.route.as_ref().and_then(|route| route.pool.as_deref()),
        Some("team")
    );
    assert_eq!(turn.account, Some(AccountRef::new("team", "alice")));
    assert_eq!(
        (turn.model.as_deref(), turn.tokens),
        (Some("sonnet"), Some(321))
    );
    assert!(rows[0].turn.is_none(), "the person's words ran on no turn");
    assert_eq!(rows[1].text, "board");
    assert!(rows[2].data.as_ref().unwrap()["bytes"].as_u64().is_some());
    let stored = serde_json::to_string(&rows).unwrap();
    assert!(!stored.contains(&job.mcp_token), "{stored}");
    assert!(!stored.contains(&served.operator), "{stored}");
    assert!(stored.contains("[redacted] and [redacted]"), "{stored}");
}

#[tokio::test]
async fn a_conversation_comes_back_after_a_restart_and_resumes_its_session() {
    let served = served(AssistantConfig::default()).await;
    let (mut app, ready) = open(&served, &served.operator).await;
    let conversation = ready["conversation"].as_str().unwrap().to_owned();
    say(&mut app, "Remember the build.", None).await;
    let job = take(&served, "hq").await;
    report(
        &served,
        "hq",
        &job,
        Some(&job.job_id),
        vec![AssistantEvent::new(EVENT_ACCEPTED, None)],
    )
    .await;
    report(
        &served,
        "hq",
        &job,
        None,
        vec![
            AssistantEvent::new(EVENT_TEXT, Some("Noted.".to_owned())),
            AssistantEvent::new(EVENT_COMPLETED, None),
        ],
    )
    .await;
    receive_until(&mut app, |frame| {
        frame["type"] == "busy" && frame["busy"] == false
    })
    .await;
    logged(&served, &conversation, |rows| rows.len() >= 2).await;
    drop(app);

    let (base, server) = restarted(&served).await;
    let (mut app, ready) = reopen(&base, &served.operator, &conversation).await;
    assert_eq!(ready["conversation"], conversation.as_str(), "{ready}");
    let texts: Vec<&str> = ready["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["Remember the build.", "Noted."]);

    // Another person asking for it gets a conversation of their own.
    let (_other, theirs) = reopen(&base, &served.viewer, &conversation).await;
    assert_ne!(theirs["conversation"], conversation.as_str());
    assert_eq!(theirs["entries"].as_array().unwrap().len(), 0);

    // Its next message resumes the provider Session on the Station that ran it.
    say(&mut app, "And now?", None).await;
    let client = reqwest::Client::new();
    let next = loop {
        let response = client
            .get(format!(
                "{base}/v1/station/assistant/next?station=hq&wait_ms=1000"
            ))
            .bearer_auth(STATION_TOKEN)
            .send()
            .await
            .unwrap();
        if response.status() == StatusCode::OK {
            break response.json::<AssistantJob>().await.unwrap();
        }
    };
    assert_eq!(next.conversation_id, conversation);
    assert_eq!(next.session_id.as_deref(), Some("provider-assistant-1"));
    server.abort();
}

/// `GET` of `path` with `token`: the status and the body as JSON.
async fn read_as(served: &Served, token: &str, path: &str) -> (StatusCode, Value) {
    let response = reqwest::Client::new()
        .get(format!("{}{path}", served.base))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn a_missions_session_context_needs_session_context_on_its_operation_and_is_audited() {
    let served = served(AssistantConfig::default()).await;
    let content = "{\"type\":\"user\"}\n{\"type\":\"assistant\"}\n";
    served
        .registry
        .file_session_context(&ai6::SessionContext::new(
            "session-g",
            ai6::ContextOwner::Mission {
                operation: "G".to_owned(),
                mission: "mission-g".to_owned(),
            },
            "worker",
            "claude",
            "claude-code-jsonl",
            ai6::ContextSource::Harness,
            100.0,
            content,
        ))
        .unwrap();
    let path = "/v1/user/operations/G/sessions/session-g/context";
    let (status, context) = read_as(&served, &served.operator, path).await;
    assert_eq!(status, StatusCode::OK, "{context}");
    assert_eq!(context["format"], "claude-code-jsonl");
    assert_eq!(context["source"], "harness");
    assert_eq!(context["content"], content);
    assert_eq!(context["bytes"], content.len());
    let read = audits(&served, "session.context.read");
    assert_eq!(read.len(), 1);
    assert_eq!(
        (
            read[0].actor.as_str(),
            read[0].operation.as_str(),
            read[0].target.as_str()
        ),
        ("operator-007", "G", "session-g")
    );
    assert_eq!(read[0].resource.as_deref(), Some("mission:mission-g"));

    // Who may read the Mission may not read its context without the action.
    let (status, _) = read_as(&served, &served.viewer, path).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Asked through another Operation, or for a Session with none, there is nothing.
    let other = "/v1/user/operations/H/sessions/session-g/context";
    assert_eq!(
        read_as(&served, &served.admin, other).await.0,
        StatusCode::NOT_FOUND
    );
    let none = "/v1/user/operations/G/sessions/none/context";
    assert_eq!(
        read_as(&served, &served.operator, none).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(audits(&served, "session.context.read").len(), 1);
}

#[tokio::test]
async fn a_person_reads_their_own_log_and_another_s_only_with_both_actions_on_the_record() {
    let served = served(AssistantConfig::default()).await;
    let (mut app, ready) = open(&served, &served.operator).await;
    let conversation = ready["conversation"].as_str().unwrap().to_owned();
    say(&mut app, "What is the build doing?", None).await;
    let job = take(&served, "hq").await;
    report(
        &served,
        "hq",
        &job,
        Some(&job.job_id),
        vec![AssistantEvent::new(EVENT_ACCEPTED, None)],
    )
    .await;
    logged(&served, &conversation, |rows| !rows.is_empty()).await;
    served
        .registry
        .file_session_context(&ai6::SessionContext::new(
            "provider-assistant-1",
            ai6::ContextOwner::Conversation {
                conversation: conversation.clone(),
            },
            "hq",
            "acp",
            ai6::CONTEXT_EVENTS_FORMAT,
            ai6::ContextSource::Events,
            100.0,
            "{\"kind\":\"text\",\"text\":\"Building.\"}\n",
        ))
        .unwrap();
    let log = format!("/v1/user/assistant/conversations/{conversation}/log");
    let context = format!("/v1/user/assistant/conversations/{conversation}/context");

    // Their own: no action, no audit.
    let (status, own) = read_as(&served, &served.operator, &log).await;
    assert_eq!(status, StatusCode::OK, "{own}");
    assert_eq!(own["person"], "operator-007");
    assert_eq!(own["entries"][0]["text"], "What is the build doing?");
    let (status, contexts) = read_as(&served, &served.operator, &context).await;
    assert_eq!(status, StatusCode::OK, "{contexts}");
    assert_eq!(contexts["contexts"][0]["source"], "events");
    assert_eq!(
        contexts["contexts"][0]["format"],
        ai6::CONTEXT_EVENTS_FORMAT
    );
    let mine = "/v1/user/assistant/conversations";
    let (status, listed) = read_as(&served, &served.operator, mine).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        listed["conversations"][0]["conversation"],
        conversation.as_str()
    );
    assert!(audits(&served, "assistant.log.read").is_empty());

    // Someone without the actions finds nothing, and nothing is audited.
    let operators = "/v1/user/assistant/conversations?person=operator-007";
    for token in [&served.member, &served.viewer] {
        assert_eq!(read_as(&served, token, &log).await.0, StatusCode::NOT_FOUND);
        assert_eq!(
            read_as(&served, token, &context).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            read_as(&served, token, operators).await.0,
            StatusCode::FORBIDDEN
        );
    }
    let (_, theirs) = read_as(&served, &served.member, mine).await;
    assert_eq!(
        theirs["conversations"],
        json!([]),
        "their own list is empty"
    );
    assert!(audits(&served, "assistant.log.read").is_empty());

    // With audit.read and session.context on config:assistant, it is read and audited.
    let (status, read) = read_as(&served, &served.admin, &log).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["entries"], own["entries"]);
    assert_eq!(
        read_as(&served, &served.admin, &context).await.0,
        StatusCode::OK
    );
    let everyone = "/v1/user/assistant/conversations?everyone=true";
    let (status, listed) = read_as(&served, &served.admin, everyone).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["conversations"][0]["person"], "operator-007");
    let read = audits(&served, "assistant.log.read");
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].actor, "admin-007");
    assert_eq!(read[0].resource.as_deref(), Some("config:assistant"));
    assert_eq!(read[0].target, format!("operator-007 {conversation}"));
    assert_eq!(audits(&served, "assistant.context.read").len(), 1);
    assert_eq!(audits(&served, "assistant.log.list").len(), 1);
}
