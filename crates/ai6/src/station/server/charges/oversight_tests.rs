//! The read tools of an Operation's charge: only its own Operation, read-only, bounded.

use std::path::Path;

use serde_json::{Value, json};

use super::super::decide::DecisionDesk;
use super::Find;
use crate::directorates::m::vetting::{Finding, FindingKind, Vetting};
use crate::directorates::registry::approvals::ApprovalRequest;
use crate::directorates::registry::store::Brief;
use crate::directorates::registry::suite::brief;
use crate::directorates::registry::timeline::TimelineEvent;
use crate::station::server::DaemonState;
use crate::station::server::tests::test_state;

const AGENTS: &str = "[agent.007]\nhandler = \"mock-recorder\"\nclearance = \"00\"\n";

/// The charge's Operation `G`, and `B`, which it does not answer for.
fn station(root: &Path) -> DaemonState {
    test_state(root, "clearance = \"00\"\n[operation.B]\n", AGENTS)
}

fn desk(state: &DaemonState) -> DecisionDesk {
    DecisionDesk::new(
        state.clone(),
        "G".to_owned(),
        "007".to_owned(),
        "charge--operation-G".to_owned(),
    )
}

/// A Mission of `operation` named `id`, finished when `summary` is given.
fn filed(
    state: &DaemonState,
    id: &str,
    operation: &str,
    agent: &str,
    summary: Option<&str>,
) -> Brief {
    let mut queued = brief(id);
    queued.mission_id = Some(id.to_owned());
    queued.operation = Some(operation.to_owned());
    queued.agent = Some(agent.to_owned());
    queued.title = format!("title of {id}");
    queued.body = format!("the brief of {id}");
    state.intake.submit_brief(&queued).unwrap();
    if let Some(summary) = summary {
        let claimed = state
            .intake
            .claim_new(Some(&queued.key()))
            .unwrap()
            .unwrap();
        state
            .intake
            .finish(
                &claimed,
                &json!({"state": "done", "summary": summary}),
                true,
                None,
                None,
            )
            .unwrap();
    }
    queued
}

/// The JSON inside a tool's `<ai6-data>` fence.
fn data(answer: &str) -> Value {
    let (_, rest) = answer.split_once('\n').expect("a fence");
    let body = rest
        .rsplit_once("\n</ai6-data>")
        .expect("a closing fence")
        .0;
    serde_json::from_str(body).unwrap()
}

fn mission_ids(answer: &str) -> Vec<String> {
    let mut found: Vec<String> = data(answer)["missions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["mission"].as_str().unwrap().to_owned())
        .collect();
    found.sort();
    found
}

fn find(change: impl FnOnce(&mut Find)) -> Find {
    let mut find = Find::default();
    change(&mut find);
    find
}

#[test]
fn a_search_finds_the_operations_missions_and_never_another_operations() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "G-1", "G", "008", Some("pinned the toolchain"));
    filed(&state, "G-2", "G", "009", None);
    filed(&state, "B-1", "B", "008", Some("pinned the toolchain"));
    let desk = desk(&state);

    let by = |change: &dyn Fn(&mut Find)| {
        let mut asked = Find::default();
        change(&mut asked);
        mission_ids(&desk.search(asked).unwrap())
    };
    assert_eq!(by(&|_| {}), ["G-1", "G-2"]);
    assert_eq!(
        by(&|asked| asked.text = Some("TOOLCHAIN".to_owned())),
        ["G-1"],
        "the outcome is searched, without case"
    );
    assert_eq!(by(&|asked| asked.state = Some("new".to_owned())), ["G-2"]);
    assert_eq!(by(&|asked| asked.agent = Some("009".to_owned())), ["G-2"]);
    assert_eq!(by(&|asked| asked.handle = Some("G-1".to_owned())), ["G-1"]);
    assert!(by(&|asked| asked.handle = Some("B-1".to_owned())).is_empty());
    assert!(by(&|asked| asked.agent = Some("nobody".to_owned())).is_empty());

    // A page is bounded, and says when more matched.
    let page = data(&desk.search(find(|asked| asked.limit = Some(1))).unwrap());
    assert_eq!(page["missions"].as_array().unwrap().len(), 1);
    assert_eq!(page["more"], true);
    assert!(desk.search(find(|asked| asked.limit = Some(0))).is_err());
}

#[test]
fn a_mission_of_another_operation_answers_as_unknown_everywhere() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "G-1", "G", "008", Some("done well"));
    filed(&state, "B-1", "B", "008", Some("secret result"));
    let desk = desk(&state);

    let own = desk.mission("G-1").unwrap();
    let own = data(&own);
    assert_eq!(own["brief_untrusted"], "the brief of G-1");
    assert!(
        own["outcome_untrusted"]
            .as_str()
            .unwrap()
            .contains("done well")
    );
    assert_eq!(own["state"], "done");

    for result in [
        desk.mission("B-1"),
        desk.mission("nothing"),
        desk.attempts("B-1"),
        desk.timeline(Some("B-1"), None),
        desk.approvals(Some("B-1")),
    ] {
        assert_eq!(result.unwrap_err(), "unknown Mission");
    }
}

#[test]
fn a_long_brief_or_outcome_is_cut_and_says_so() {
    let root = tempfile::tempdir().unwrap();
    let mut state = station(root.path());
    let mut limits = state.station.limits().clone();
    limits.oversight.text_bytes = crate::Limit::at(10);
    state.station = state.station.clone().with_limits(limits);
    filed(
        &state,
        "G-1",
        "G",
        "008",
        Some("a very long summary indeed"),
    );
    let record = data(&desk(&state).mission("G-1").unwrap());
    assert_eq!(record["brief_untrusted"]["cut"], true);
    assert_eq!(record["brief_untrusted"]["text"], "the brief ");
    assert_eq!(record["outcome_untrusted"]["cut"], true);
}

#[test]
fn the_board_the_timeline_the_holds_and_the_approvals_are_the_operations_own() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "G-1", "G", "008", Some("done"));
    filed(&state, "G-2", "G", "008", None);
    filed(&state, "B-1", "B", "008", None);
    let held = filed(&state, "G-3", "G", "008", None);
    state
        .intake
        .hold_brief(
            "G",
            &held.key(),
            &Vetting::from_findings(vec![Finding::new(FindingKind::Duplicate, "twice")]),
            5.0,
        )
        .unwrap();
    let mut other = brief("B-2");
    other.mission_id = Some("B-2".to_owned());
    other.operation = Some("B".to_owned());
    state.intake.submit_brief(&other).unwrap();
    state
        .intake
        .hold_brief("B", "B-2", &Vetting::from_findings(Vec::new()), 6.0)
        .unwrap();
    for (operation, mission, at) in [("G", "G-1", 1.0), ("B", "B-1", 2.0), ("G", "G-2", 3.0)] {
        state
            .intake
            .record_event(&TimelineEvent::new("lifecycle", "done", operation, at).mission(mission))
            .unwrap();
    }
    let now = state.clock.now() as f64;
    for (operation, mission) in [("G", "G-2"), ("B", "B-1")] {
        let request = ApprovalRequest::try_new(
            operation,
            crate::directorates::m::identity::MissionId::try_new(mission).unwrap(),
            crate::directorates::m::identity::RunId::try_new("run-1").unwrap(),
            "008",
            crate::directorates::m::approvals::ActionScope::try_new("merge", "PR 1").unwrap(),
            "ready",
            "key",
            &Default::default(),
        )
        .unwrap()
        .expiring(now, std::time::Duration::from_secs(3_600));
        state.intake.request_approval(&request).unwrap();
    }
    let desk = desk(&state);

    let held = data(&desk.held(None).unwrap());
    assert_eq!(held["held"].as_array().unwrap().len(), 1);
    assert_eq!(held["held"][0]["mission"], "G-3");
    assert_eq!(held["held"][0]["kinds"], json!(["Duplicate"]));

    let all = data(&desk.timeline(None, None).unwrap());
    let missions: Vec<&str> = all["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["mission"].as_str().unwrap())
        .collect();
    assert_eq!(missions, ["G-2", "G-1"], "newest first, and not B's");
    let one = data(&desk.timeline(Some("G-1"), None).unwrap());
    assert_eq!(one["events"].as_array().unwrap().len(), 1);

    let waiting = data(&desk.approvals(None).unwrap());
    assert_eq!(waiting["pending"].as_array().unwrap().len(), 1);
    assert_eq!(waiting["pending"][0]["mission"], "G-2");
    let of_mission = data(&desk.approvals(Some("G-2")).unwrap());
    assert_eq!(of_mission["approvals"][0]["decision"], Value::Null);
}

mod over_mcp {
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, ContentBlock};
    use rmcp::service::{RoleClient, RunningService};
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    use super::*;
    use crate::directorates::medical::exposure::ExposureGate;
    use crate::directorates::registry::cables::CablePrincipal;
    use crate::station::server::cables_mcp::Endpoint;

    const READS: [&str; 6] = [
        "search_missions",
        "mission_record",
        "mission_attempts",
        "held_briefs",
        "mission_timeline",
        "operation_approvals",
    ];

    async fn connect(endpoint: &Endpoint) -> RunningService<RoleClient, ()> {
        ().serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(endpoint.url().to_owned())
                .auth_header(endpoint.token().to_owned()),
        ))
        .await
        .unwrap()
    }

    async fn endpoint(state: &DaemonState, charge: bool) -> Endpoint {
        let identity = CablePrincipal::try_new(
            "007".to_owned(),
            "G".to_owned(),
            None,
            state.station_id.clone(),
            None,
            None,
        )
        .unwrap();
        let endpoint = Endpoint::start_with_exposure(
            state.cables.clone(),
            identity,
            state.intake.clone(),
            false,
            ExposureGate::default(),
        )
        .await
        .unwrap();
        if charge {
            endpoint.open_decision_tools(desk(state));
        }
        endpoint
    }

    async fn names(client: &RunningService<RoleClient, ()>) -> Vec<String> {
        client
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    async fn call(
        client: &RunningService<RoleClient, ()>,
        tool: &'static str,
        arguments: Value,
    ) -> String {
        let response = client
            .call_tool(
                CallToolRequestParams::new(tool)
                    .with_arguments(serde_json::from_value(arguments).unwrap()),
            )
            .await
            .unwrap();
        let [ContentBlock::Text(text)] = response.content.as_slice() else {
            panic!("the tool answers one text block");
        };
        text.text.clone()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn only_the_operations_charge_has_the_tools_and_they_answer_for_its_operation() {
        let root = tempfile::tempdir().unwrap();
        let state = station(root.path());
        filed(&state, "G-1", "G", "008", Some("pinned the toolchain"));
        filed(&state, "B-1", "B", "008", Some("pinned the toolchain"));

        let plain = endpoint(&state, false).await;
        let client = connect(&plain).await;
        let listed = names(&client).await;
        for tool in READS {
            assert!(
                !listed.iter().any(|name| name == tool),
                "{tool}: {listed:?}"
            );
        }
        client.cancel().await.unwrap();

        let charge = endpoint(&state, true).await;
        let client = connect(&charge).await;
        let listed = names(&client).await;
        for tool in READS {
            assert!(listed.iter().any(|name| name == tool), "{tool}: {listed:?}");
        }
        let found = call(&client, "search_missions", json!({"text": "toolchain"})).await;
        assert_eq!(mission_ids(&found), ["G-1"], "{found}");
        let refused = call(&client, "mission_record", json!({"mission": "B-1"})).await;
        assert!(refused.contains("unknown Mission"), "{refused}");
        let own = call(&client, "mission_record", json!({"mission": "G-1"})).await;
        assert!(own.contains("the brief of G-1"), "{own}");
        client.cancel().await.unwrap();
    }
}
