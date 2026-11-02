use std::path::Path as FsPath;
use std::sync::Arc;

use serde_json::json;

use super::super::reload::LiveConfig;
use super::super::tests::{RECORDED_RESUMES, start_mission_by_id, test_state, wait_for_state};
use super::super::{ResumeMission, finalize_mission_resources, resume_mission};
use super::*;
use crate::directorates::m::config::{ConfigProvider, FileProvider};
use crate::directorates::m::mission::MissionState;
use crate::directorates::medical::exposure::{BRIEF_COMPARTMENT, BRIEF_EXPOSURE_KEY};
use crate::directorates::registry::mission_issues::{
    IssueLink, LinkRelation, LinkState, OutboxEntry, OutboxKind,
};
use crate::directorates::registry::products::CUT_MARKER;
use crate::directorates::registry::store::Brief;
use crate::directorates::registry::suite::brief;

const AGENTS: &str = "[agent.007]\nhandler = \"mock-recorder\"\nclearance = \"field\"\n\
                      [agent.007.limits]\nstall_timeout = 5\n\
                      [agent.licensed]\nhandler = \"mock-recorder\"\nclearance = \"00\"\n\
                      [agent.licensed.limits]\nstall_timeout = 5\n";

/// `G`, which is read in, and `B` and `C`, which it reads.
const OPERATIONS: &str = "clearance = \"00\"\n[operation.B]\n[operation.C]\n";

/// A Station reading its configuration from `root`, as `serve` does.
fn station(root: &FsPath) -> DaemonState {
    let mut state = test_state(root, OPERATIONS, AGENTS);
    let source: Arc<dyn ConfigProvider> = Arc::new(FileProvider::new(root));
    let snapshot = source.load(&state.station_id).unwrap();
    state.roster = Arc::new(snapshot.roster.clone());
    state.config = Arc::new(LiveConfig::new(
        source,
        &state.station_id,
        snapshot,
        state.roster.clone(),
        None,
    ));
    state
}

fn grant(root: &FsPath, compartment: &str, actions: &[&str], covers: &str, read_in: &str) {
    let actions: Vec<String> = actions.iter().map(|action| format!("{action:?}")).collect();
    std::fs::write(
        root.join("compartments.toml"),
        format!(
            "[compartment.{compartment}]\nactions = [{}]\ncovers = [{{ {covers} }}]\n\
             read_in = [{{ subject = {read_in:?} }}]\n",
            actions.join(", ")
        ),
    )
    .unwrap();
}

fn read_out(root: &FsPath) {
    std::fs::remove_file(root.join("compartments.toml")).unwrap();
}

fn reader(agent: &str) -> Reader {
    Reader {
        agent: agent.to_owned(),
        operation: "G".to_owned(),
        mission: Some("G-1".to_owned()),
        exposure: ExposureGate::default(),
    }
}

fn filed(state: &DaemonState, id: &str, operation: &str, finished: bool) -> Brief {
    let mut queued = brief(id);
    queued.mission_id = Some(id.to_owned());
    queued.operation = Some(operation.to_owned());
    queued.title = format!("title of {id}");
    queued.body = format!("the brief of {id}");
    queued.role = Some("analyst".to_owned());
    state.intake.submit_brief(&queued).unwrap();
    if finished {
        state
            .intake
            .finish(
                &queued,
                &json!({
                    "state": "done",
                    "mission_id": id,
                    "session_ids": ["provider-secret-session"],
                    "agent": "008",
                    "branch": "ai6/work",
                    "summary": "the product text",
                    "recovered_by": "station-secret",
                }),
                true,
                None,
                None,
            )
            .unwrap();
    }
    queued
}

fn audits(state: &DaemonState) -> Vec<AuditRecord> {
    state
        .intake
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|record| record.id.starts_with("compartment-"))
        .collect()
}

fn desk(state: &DaemonState) -> CompartmentDesk {
    CompartmentDesk::new(state.clone())
}

#[test]
fn a_status_read_lists_exactly_the_status_fields_and_does_not_expose() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "B-1", "B", true);
    filed(&state, "C-1", "C", true);
    grant(
        root.path(),
        "KESTREL",
        &["mission.status"],
        "operation = \"B\"",
        "operation:G",
    );
    let reader = reader("007");
    let answer = desk(&state)
        .missions_in(&reader, Some("B"), None, 50)
        .unwrap();
    let missions = answer["missions"].as_array().unwrap();
    assert_eq!(missions.len(), 1);
    let mut keys: Vec<&str> = missions[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "agent",
            "attempts",
            "created",
            "mission",
            "operation",
            "role",
            "state"
        ]
    );
    assert_eq!(missions[0]["mission"], "B-1");
    assert_eq!(missions[0]["state"], "done");
    assert_eq!(missions[0]["agent"], "008");
    let text = answer.to_string();
    assert!(!text.contains("the brief of"), "{text}");
    assert!(!text.contains("provider-secret-session"), "{text}");
    assert!(!text.contains("station-secret"), "{text}");
    assert!(!reader.exposure.is_exposed());
    let by_name = desk(&state)
        .missions_in(&reader, None, Some("KESTREL"), 50)
        .unwrap();
    assert_eq!(by_name["missions"].as_array().unwrap().len(), 1);
    assert_eq!(by_name["missions"][0]["mission"], "B-1");
    // Another Operation lists nothing, as one that does not exist does.
    for other in ["C", "nowhere"] {
        assert_eq!(
            desk(&state)
                .missions_in(&reader, Some(other), None, 50)
                .unwrap(),
            json!({"missions": []}),
            "{other}"
        );
    }
    assert_eq!(
        desk(&state)
            .missions_in(&reader, None, Some("NOT-HELD"), 50)
            .unwrap(),
        json!({"missions": []})
    );
}

#[test]
fn a_product_read_returns_the_product_and_marks_the_session() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "B-1", "B", true);
    state
        .intake
        .link_issue(&IssueLink {
            operation: "B".to_owned(),
            mission_id: "B-1".to_owned(),
            tracker: "gh".to_owned(),
            item: "o/r#1".to_owned(),
            url: None,
            relation: LinkRelation::Source,
            state: LinkState::Active,
            linked_by: "m".to_owned(),
            linked_at: 1.0,
            status_comment: None,
            phase: None,
            snapshot: None,
        })
        .unwrap();
    let mut report = OutboxEntry::new("B", "B-1", "gh", "o/r#1", OutboxKind::Report, "m", 1.0);
    report.body = "the report text".to_owned();
    state.intake.queue_outbox(&report).unwrap();
    grant(
        root.path(),
        "KESTREL",
        &["product.read"],
        "operation = \"B\"",
        "agent:007",
    );
    let reader = reader("007");
    let answer = desk(&state).mission_products(&reader, "B-1").unwrap();
    assert_eq!(answer["outcome_untrusted"]["summary"], "the product text");
    assert_eq!(answer["cx_report_untrusted"], "the report text");
    assert_eq!(answer["refs"]["branch"], "ai6/work");
    assert_eq!(answer.get("brief_untrusted"), None);
    let text = answer.to_string();
    assert!(!text.contains("provider-secret-session"), "{text}");
    assert!(!text.contains("station-secret"), "{text}");
    let sources: Vec<String> = reader
        .exposure
        .exposures()
        .into_iter()
        .map(|exposure| exposure.source)
        .collect();
    assert_eq!(sources, ["product:B"]);
    assert!(
        desk(&state).mission_brief(&reader, "B-1").is_err(),
        "product.read gives no brief"
    );
}

#[test]
fn a_brief_read_gives_the_text_and_marks_the_session() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "B-1", "B", false);
    grant(
        root.path(),
        "KESTREL",
        &["mission.read"],
        "mission = \"B-1\"",
        "agent:007",
    );
    let reader = reader("007");
    let answer = desk(&state).mission_brief(&reader, "B-1").unwrap();
    assert_eq!(answer["brief_untrusted"], "the brief of B-1");
    assert_eq!(answer["state"], "new");
    assert!(reader.exposure.is_exposed());
}

#[test]
fn a_refused_mission_answers_as_one_that_does_not_exist_and_is_not_marked() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "B-1", "B", true);
    filed(&state, "G-9", "G", true);
    let reader = reader("007");
    let before = desk(&state).mission_products(&reader, "B-1").unwrap_err();
    assert_eq!(before, UNKNOWN_MISSION);
    assert_eq!(
        desk(&state)
            .mission_products(&reader, "no-such-mission")
            .unwrap_err(),
        before
    );
    assert_eq!(
        desk(&state)
            .mission_products(&reader, "../escape")
            .unwrap_err(),
        before
    );
    grant(
        root.path(),
        "KESTREL",
        &["mission.status"],
        "operation = \"B\"",
        "operation:G",
    );
    assert_eq!(
        desk(&state).mission_products(&reader, "B-1").unwrap_err(),
        before,
        "status alone gives no product"
    );
    grant(
        root.path(),
        "KESTREL",
        &["product.read"],
        "operation = \"G\"",
        "operation:G",
    );
    assert_eq!(
        desk(&state).mission_products(&reader, "G-9").unwrap_err(),
        before,
        "its own Operation is never across"
    );
    assert!(!reader.exposure.is_exposed());
    let refused = audits(&state);
    assert!(refused.len() >= 4);
    assert!(
        refused
            .iter()
            .all(|record| record.target.ends_with("refused"))
    );
}

#[test]
fn a_read_out_holds_from_the_next_call() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "B-1", "B", true);
    grant(
        root.path(),
        "KESTREL",
        &["product.read"],
        "operation = \"B\"",
        "operation:G",
    );
    let reader = reader("007");
    assert!(desk(&state).holds_any(&reader));
    assert!(desk(&state).mission_products(&reader, "B-1").is_ok());
    read_out(root.path());
    assert_eq!(
        desk(&state).mission_products(&reader, "B-1").unwrap_err(),
        UNKNOWN_MISSION
    );
    assert!(!desk(&state).holds_any(&reader));
    assert_eq!(
        desk(&state).compartments(&reader).unwrap(),
        json!({"compartments": []})
    );
}

#[test]
fn what_cannot_be_read_grants_nothing() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "B-1", "B", true);
    let reader = reader("007");
    // Another record of the family is not acceptable: it grants nothing, this one still does.
    std::fs::write(
        root.path().join("compartments.toml"),
        "[compartment.GOOD]\nactions = [\"mission.status\"]\ncovers = [{ operation = \"B\" }]\n\
         read_in = [{ subject = \"operation:G\" }]\n\
         [compartment.BAD]\nactions = [\"roster.manage\", \"product.read\"]\n\
         covers = [{ operation = \"B\" }]\nread_in = [{ subject = \"operation:G\" }]\n",
    )
    .unwrap();
    assert_eq!(
        desk(&state).mission_products(&reader, "B-1").unwrap_err(),
        UNKNOWN_MISSION
    );
    // A family file that cannot be read refuses, whatever was granted a moment ago.
    grant(
        root.path(),
        "KESTREL",
        &["product.read"],
        "operation = \"B\"",
        "operation:G",
    );
    assert!(desk(&state).mission_products(&reader, "B-1").is_ok());
    std::fs::write(root.path().join("compartments.toml"), "not = [a document").unwrap();
    assert_eq!(
        desk(&state).mission_products(&reader, "B-1").unwrap_err(),
        UNAVAILABLE
    );
    assert!(!desk(&state).holds_any(&reader));
}

#[test]
fn each_allowed_call_leaves_one_record_naming_the_compartment() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "B-1", "B", true);
    grant(
        root.path(),
        "KESTREL",
        &["mission.read", "product.read"],
        "operation = \"B\"",
        "agent:007",
    );
    let reader = reader("007");
    let desk = desk(&state);
    desk.missions_in(&reader, Some("B"), None, 50).unwrap();
    desk.mission_brief(&reader, "B-1").unwrap();
    desk.mission_products(&reader, "B-1").unwrap();
    let mut records = audits(&state);
    records.sort_by(|left, right| left.target.cmp(&right.target));
    let shape: Vec<(&str, &str, &str, Option<&str>, &str)> = records
        .iter()
        .map(|record| {
            (
                record.actor.as_str(),
                record.action.as_str(),
                record.operation.as_str(),
                record.resource.as_deref(),
                record.target.as_str(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            (
                "agent:007",
                "compartment.read",
                "G",
                Some("mission:B-1"),
                "KESTREL mission_brief B-1"
            ),
            (
                "agent:007",
                "compartment.read",
                "G",
                Some("mission:B-1"),
                "KESTREL mission_products B-1"
            ),
            (
                "agent:007",
                "compartment.read",
                "G",
                Some("operation:B"),
                "KESTREL missions_in -"
            ),
        ]
    );
}

/// Makes every audit write fail: the file the log's directory should be stands in its way.
fn break_the_audit_log(root: &FsPath) {
    let audit = root.join("registry").join("audit");
    let _ = std::fs::remove_dir_all(&audit);
    std::fs::create_dir_all(root.join("registry")).unwrap();
    std::fs::write(&audit, "in the way").unwrap();
}

#[test]
fn a_failing_audit_log_refuses_the_read() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "B-1", "B", true);
    grant(
        root.path(),
        "KESTREL",
        &["mission.read", "product.read"],
        "operation = \"B\"",
        "operation:G",
    );
    break_the_audit_log(root.path());
    let reader = reader("007");
    assert_eq!(
        desk(&state).mission_products(&reader, "B-1").unwrap_err(),
        UNAVAILABLE
    );
    assert_eq!(
        desk(&state).mission_brief(&reader, "B-1").unwrap_err(),
        UNAVAILABLE
    );
    assert_eq!(
        desk(&state)
            .missions_in(&reader, Some("B"), None, 50)
            .unwrap_err(),
        UNAVAILABLE
    );
    assert!(!reader.exposure.is_exposed());
}

#[test]
fn a_text_past_the_compartment_s_bound_is_cut_with_a_marker() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    let queued = {
        let mut queued = brief("B-1");
        queued.mission_id = Some("B-1".to_owned());
        queued.operation = Some("B".to_owned());
        queued
    };
    state.intake.submit_brief(&queued).unwrap();
    state
        .intake
        .finish(
            &queued,
            &json!({"state": "done", "summary": "é".repeat(40)}),
            true,
            None,
            None,
        )
        .unwrap();
    std::fs::write(
        root.path().join("compartments.toml"),
        "[compartment.KESTREL]\nactions = [\"product.read\"]\ncovers = [{ operation = \"B\" }]\n\
         read_in = [{ subject = \"operation:G\" }]\nlimits = { max_product_bytes = 11 }\n",
    )
    .unwrap();
    let answer = desk(&state)
        .mission_products(&reader("007"), "B-1")
        .unwrap();
    let summary = answer["outcome_untrusted"]["summary"].as_str().unwrap();
    assert_eq!(summary, format!("{}{CUT_MARKER}", "é".repeat(5)));
}

#[test]
fn a_clearance_floor_is_read_at_call_time() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path());
    filed(&state, "B-1", "B", true);
    std::fs::write(
        root.path().join("compartments.toml"),
        "[compartment.KESTREL]\nactions = [\"product.read\"]\ncovers = [{ operation = \"B\" }]\n\
         read_in = [{ subject = \"operation:G\" }]\nmin_clearance = \"00\"\n",
    )
    .unwrap();
    assert_eq!(
        desk(&state)
            .mission_products(&reader("007"), "B-1")
            .unwrap_err(),
        UNKNOWN_MISSION
    );
    assert!(
        desk(&state)
            .mission_products(&reader("licensed"), "B-1")
            .is_ok()
    );
    assert!(!desk(&state).holds_any(&reader("no-such-agent")));
}

mod over_mcp {
    use std::collections::BTreeMap;

    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, ContentBlock};
    use rmcp::service::{RoleClient, RunningService};
    use rmcp::transport::StreamableHttpClientTransport;
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    use super::super::super::cables_mcp::Endpoint;
    use super::super::super::green_ink;
    use super::*;
    use crate::directorates::m::identity::SessionId;
    use crate::directorates::registry::cables::{
        CableEndpoint, CablePolicy, CablePrincipal, CableRelay,
    };

    async fn connect(url: &str, token: &str) -> RunningService<RoleClient, ()> {
        ().serve(StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(url.to_owned())
                .auth_header(token.to_owned()),
        ))
        .await
        .unwrap()
    }

    async fn call(
        client: &RunningService<RoleClient, ()>,
        tool: &'static str,
        arguments: serde_json::Value,
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

    async fn tools(client: &RunningService<RoleClient, ()>) -> Vec<String> {
        client
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    async fn endpoint(state: &DaemonState, agent: &str, bind: bool) -> Endpoint {
        let identity = CablePrincipal::try_new(
            agent.to_owned(),
            "G".to_owned(),
            None,
            state.station_id.clone(),
            None,
            Some("G-1".to_owned()),
        )
        .unwrap();
        let mut endpoint = Endpoint::start_with_exposure(
            state.cables.clone(),
            identity,
            state.intake.clone(),
            false,
            ExposureGate::default(),
        )
        .await
        .unwrap();
        endpoint.open_green_ink(green_ink::Desk::new(state.clone()));
        endpoint.open_compartment_tools(CompartmentDesk::new(state.clone()));
        if bind {
            endpoint
                .bind_session(Some(&SessionId::try_new("provider-session").unwrap()))
                .unwrap();
        }
        endpoint
    }

    /// A native harness connects and lists its tools before it reports a Session ID, so nothing
    /// here may wait for one.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_tools_are_listed_and_work_before_the_session_id_arrives() {
        let root = tempfile::tempdir().unwrap();
        let state = station(root.path());
        filed(&state, "B-1", "B", true);
        grant(
            root.path(),
            "KESTREL",
            &["product.read"],
            "operation = \"B\"",
            "operation:G",
        );
        let endpoint = endpoint(&state, "007", false).await;
        assert!(!endpoint.is_bound());
        let client = connect(endpoint.url(), endpoint.token()).await;
        assert!(
            tools(&client)
                .await
                .iter()
                .any(|tool| tool == "mission_products")
        );
        let answer = call(&client, "mission_products", json!({"mission": "B-1"})).await;
        assert!(answer.contains("the product text"), "{answer}");
        client.cancel().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_tools_are_listed_only_while_the_session_holds_a_compartment() {
        let root = tempfile::tempdir().unwrap();
        let state = station(root.path());
        filed(&state, "B-1", "B", true);
        let endpoint = endpoint(&state, "007", true).await;
        let client = connect(endpoint.url(), endpoint.token()).await;
        let listed = tools(&client).await;
        for tool in [
            "compartments",
            "missions_in",
            "mission_brief",
            "mission_products",
        ] {
            assert!(
                !listed.iter().any(|name| name == tool),
                "{tool}: {listed:?}"
            );
        }
        assert!(listed.iter().any(|name| name == "send_cable"));
        // Calling one anyway is refused as any Mission it may not read is.
        let refused = call(&client, "mission_products", json!({"mission": "B-1"})).await;
        assert!(refused.contains(UNKNOWN_MISSION), "{refused}");

        grant(
            root.path(),
            "KESTREL",
            &["product.read"],
            "operation = \"B\"",
            "operation:G",
        );
        assert!(
            tools(&client)
                .await
                .iter()
                .any(|name| name == "mission_products")
        );
        let allowed = call(&client, "mission_products", json!({"mission": "B-1"})).await;
        assert!(allowed.contains("the product text"), "{allowed}");

        read_out(root.path());
        assert!(
            !tools(&client)
                .await
                .iter()
                .any(|name| name == "mission_products")
        );
        let refused = call(&client, "mission_products", json!({"mission": "B-1"})).await;
        assert!(refused.contains(UNKNOWN_MISSION), "{refused}");
        client.cancel().await.unwrap();
    }

    fn proposal(key: &str, operation: &str) -> serde_json::Value {
        json!({
            "title": "Look at the release",
            "body": "the proposed brief",
            "idempotency_key": key,
            "operation": operation,
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_licensed_agent_proposes_into_a_covered_operation_as_untrusted_intake() {
        let root = tempfile::tempdir().unwrap();
        let state = station(root.path());
        let grant_it = || {
            grant(
                root.path(),
                "KESTREL",
                &["mission.propose"],
                "operation = \"B\"",
                "operation:G",
            );
        };
        let licensed = endpoint(&state, "licensed", true).await;
        let client = connect(licensed.url(), licensed.token()).await;

        // Without the compartment, refused and not filed.
        let refused = call(&client, "propose_mission", proposal("first", "B")).await;
        assert!(refused.contains("permission is unavailable"), "{refused}");
        assert!(
            state
                .intake
                .list_mission_proposals("B", 10)
                .unwrap()
                .is_empty()
        );

        grant_it();
        let answer = call(&client, "propose_mission", proposal("second", "B")).await;
        assert!(answer.contains("pending_authorization"), "{answer}");
        let filed = state.intake.list_mission_proposals("B", 10).unwrap();
        assert_eq!(filed.len(), 1);
        assert_eq!(filed[0].operation_id, "B");
        assert_eq!(filed[0].proposer_agent_id, "licensed");
        assert_eq!(filed[0].parent_role, None);
        assert_eq!(filed[0].brief.data["from_operation"], "G");
        assert_eq!(filed[0].brief.data["compartment"], "KESTREL");
        assert_eq!(
            filed[0].brief.data[BRIEF_EXPOSURE_KEY],
            json!([BRIEF_COMPARTMENT])
        );
        assert!(
            state.intake.list_new().unwrap().is_empty(),
            "M authorizes it first"
        );
        let records = audits(&state);
        assert!(records.iter().any(|record| record.action == PROPOSE_ACTION
            && record.resource.as_deref() == Some("operation:B")
            && record.target.starts_with("KESTREL propose_mission")));

        // Another Operation the compartment does not cover is refused.
        let refused = call(&client, "propose_mission", proposal("third", "C")).await;
        assert!(refused.contains("permission is unavailable"), "{refused}");
        assert!(
            state
                .intake
                .list_mission_proposals("C", 10)
                .unwrap()
                .is_empty()
        );
        client.cancel().await.unwrap();

        // A field Agent is refused whatever it holds.
        let field = endpoint(&state, "007", true).await;
        let client = connect(field.url(), field.token()).await;
        let refused = call(&client, "propose_mission", proposal("fourth", "B")).await;
        assert!(refused.contains("permission is unavailable"), "{refused}");
        assert_eq!(
            state.intake.list_mission_proposals("B", 10).unwrap().len(),
            1
        );
        client.cancel().await.unwrap();

        // A failing audit log refuses the proposal.
        break_the_audit_log(root.path());
        let client = connect(licensed.url(), licensed.token()).await;
        let refused = call(&client, "propose_mission", proposal("fifth", "B")).await;
        assert!(refused.contains("permission is unavailable"), "{refused}");
        assert_eq!(
            state.intake.list_mission_proposals("B", 10).unwrap().len(),
            1
        );
        client.cancel().await.unwrap();
    }

    /// A relay on `state` as `serve` builds one: the log, and the answers read from the provider.
    fn relay_on(state: &mut DaemonState, endpoints: Vec<CableEndpoint>) {
        let log = state.intake.clone();
        let relay = Arc::new(
            CableRelay::new(
                state.intake.clone(),
                CablePolicy::default(),
                move || endpoints.clone(),
                BTreeMap::new(),
            )
            .with_durable_audit(move |record| {
                super::super::super::write_cable_audit(log.as_ref(), "test", record)
            }),
        );
        state.cables = relay.clone();
        relay.set_compartments(Arc::new(CableAnswers::new(state.clone())));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_cable_to_a_compartment_goes_through_the_tool_and_is_logged() {
        let root = tempfile::tempdir().unwrap();
        let mut state = station(root.path());
        let station_id = state.station_id.clone();
        relay_on(
            &mut state,
            vec![
                CableEndpoint::try_new(
                    "008".to_owned(),
                    "B".to_owned(),
                    None,
                    station_id,
                    Some("session-008".to_owned()),
                    Some("B-1".to_owned()),
                )
                .unwrap(),
            ],
        );
        let endpoint = endpoint(&state, "007", true).await;
        let client = connect(endpoint.url(), endpoint.token()).await;
        let cable = |key: &'static str| json!({"scope": "compartment", "target": "KESTREL", "body": "the release moved", "idempotency_key": key});
        let refused = call(&client, "send_cable", cable("before")).await;
        assert!(refused.contains("not authorized"), "{refused}");

        grant(
            root.path(),
            "KESTREL",
            &["cable.send"],
            "operation = \"B\"",
            "operation:G",
        );
        let peers = call(&client, "discover_agents", json!({})).await;
        assert!(peers.contains("\"compartment\":\"KESTREL\""), "{peers}");
        assert!(!peers.contains("session-008"), "{peers}");
        let sent = call(&client, "send_cable", cable("granted")).await;
        assert!(sent.contains("\"recipient_count\":1"), "{sent}");
        let records = state.intake.audit_records().unwrap();
        let record = records
            .iter()
            .find(|record| record.action == "cable.send")
            .expect("a crossing is in the log");
        assert_eq!(record.actor, "agent:007");
        assert_eq!(record.operation, "G");
        assert_eq!(record.resource.as_deref(), Some("compartment:KESTREL"));
        assert!(
            record.target.starts_with("compartment:KESTREL cable-"),
            "{}",
            record.target
        );

        read_out(root.path());
        let refused = call(&client, "send_cable", cable("after")).await;
        assert!(refused.contains("not authorized"), "{refused}");
        client.cancel().await.unwrap();
    }

    /// Both ways a Station opens a Session's endpoint must give it the tools.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_started_and_a_resumed_session_both_get_the_tools() {
        let root = tempfile::tempdir().unwrap();
        let state = station(root.path());
        filed(&state, "B-1", "B", true);
        grant(
            root.path(),
            "KESTREL",
            &["product.read"],
            "operation = \"B\"",
            "operation:G",
        );
        let mission_id = format!("compartment-read-{}", std::process::id());
        state
            .missions
            .lock()
            .unwrap()
            .create(
                crate::MissionId::try_new(&mission_id).unwrap(),
                Some("G".to_owned()),
                "read across".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        start_mission_by_id(&state, &mission_id).await;
        wait_for_state(&state, &mission_id, MissionState::Blocked).await;
        let environment = state.sessions.lock().unwrap()[&mission_id]
            .environment
            .clone();
        let client = connect(
            &environment["AI6_CABLE_MCP_URL"],
            &environment["AI6_CABLE_MCP_TOKEN"],
        )
        .await;
        assert!(
            tools(&client)
                .await
                .iter()
                .any(|name| name == "mission_products")
        );
        let answer = call(&client, "mission_products", json!({"mission": "B-1"})).await;
        assert!(answer.contains("the product text"), "{answer}");
        client.cancel().await.unwrap();

        let _ = resume_mission(
            axum::extract::State(state.clone()),
            axum::extract::Path(mission_id.clone()),
            axum::Json(ResumeMission {
                message: Some("carry on".to_owned()),
                environment: BTreeMap::new(),
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("resume failed: {:?}", error.1.0.error));
        let resumed = RECORDED_RESUMES
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.mission_id.as_str() == mission_id)
            .cloned()
            .expect("the handler was resumed");
        let client = connect(
            &resumed.environment["AI6_CABLE_MCP_URL"],
            &resumed.environment["AI6_CABLE_MCP_TOKEN"],
        )
        .await;
        assert!(
            tools(&client)
                .await
                .iter()
                .any(|name| name == "mission_products")
        );
        let answer = call(&client, "mission_products", json!({"mission": "B-1"})).await;
        assert!(answer.contains("the product text"), "{answer}");
        // A resumed Session is held to a read out like any other.
        read_out(root.path());
        let refused = call(&client, "mission_products", json!({"mission": "B-1"})).await;
        assert!(refused.contains(UNKNOWN_MISSION), "{refused}");
        client.cancel().await.unwrap();
        finalize_mission_resources(&state, &mission_id, true, false)
            .await
            .unwrap_or_else(|error| panic!("finalize failed: {:?}", error.1.0.error));
    }
}
