//! Her tools for triage: unassessed Reports, what each charge last did, and why a Station refused
//! a configuration. Each answers as the page does and carries the link that opens the page.

use ai6::directorates::registry::charges::{ChargeLogEntry, ChargeUpdate};
use ai6::directorates::registry::reports::{Report, ReportFiler, ReportScope};

use super::*;

/// The JSON a tool wrapped as data.
fn data(answer: &Value, tool: &str) -> Value {
    assert_eq!(answer["isError"], false, "{answer}");
    let text = answer["content"][0]["text"].as_str().unwrap();
    let open = format!("<ai6-data tool=\"{tool}\">");
    assert!(text.starts_with(&open), "{text}");
    serde_json::from_str(
        text.trim_start_matches(&open)
            .trim_end_matches("</ai6-data>"),
    )
    .unwrap()
}

fn file_report(served: &Served, id: &str, scope: ReportScope, title: &str) {
    served
        .registry
        .file_report(&Report::new(
            id,
            scope,
            "finding",
            ai6::directorates::registry::logbook::Severity::Info,
            title,
            "What was seen.",
            ReportFiler::Person {
                person: "operator-007".to_owned(),
            },
            100.0,
        ))
        .unwrap();
}

fn wake(served: &Served, scope: &str, lease: Duration, turn: &str) {
    let record = served
        .registry
        .claim_charge(scope, "kg", "worker", lease)
        .unwrap();
    let mut update = ChargeUpdate::default();
    update.last_wake = Some(1_000.0);
    update.turn_at = Some(1_000.0);
    update.log = vec![ChargeLogEntry::new("turn", turn, 1_000.0)];
    served
        .registry
        .update_charge(scope, "worker", record.fence, None, update)
        .unwrap();
}

#[tokio::test]
async fn her_reports_tool_lists_what_waits_with_the_link_and_what_could_not_be_read() {
    let served = served(AssistantConfig::default()).await;
    file_report(
        &served,
        "report-g",
        ReportScope::Operation("G".to_owned()),
        "Key in the CI log",
    );
    file_report(
        &served,
        "report-w",
        ReportScope::Station("worker".to_owned()),
        "Station only",
    );
    let (_app, job) = accepted(&served, &served.operator, "Any Reports to assess?").await;

    let tools = mcp(&served, &job.mcp_token, "tools/list", json!({}))
        .await
        .json::<Value>()
        .await
        .unwrap();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    for tool in ["reports", "charges", "config_refusals"] {
        assert!(names.contains(&tool), "{names:?}");
    }

    let answer = call_tool(&served, &job.mcp_token, "reports", json!({})).await;
    let listed = data(&answer, "reports");
    let reports = listed["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 1, "{listed}");
    assert_eq!(reports[0]["id"], "report-g");
    assert_eq!(reports[0]["link"], "app:/reports?report_id=report-g");
    assert_eq!(reports[0]["title"], "Key in the CI log");
    // The Station's Reports are not the operator's to read: said, not listed.
    let coverage = &listed["coverage"];
    assert!(
        coverage["checked"]
            .as_array()
            .unwrap()
            .contains(&json!("operation:G")),
        "{listed}"
    );
    assert!(
        coverage["not_permitted"]
            .as_array()
            .unwrap()
            .contains(&json!("station:worker")),
        "{listed}"
    );
    assert!(!listed.to_string().contains("Station only"));

    // The same read as the page's route, so what she says is what the app would show.
    let narrowed = data(
        &call_tool(
            &served,
            &job.mcp_token,
            "reports",
            json!({"operation_id": "G"}),
        )
        .await,
        "reports",
    );
    assert_eq!(narrowed["reports"][0]["id"], "report-g");
    assert_eq!(narrowed["scope"], "Operation G");
    let sneaky = call_tool(
        &served,
        &job.mcp_token,
        "reports",
        json!({"scope": "station:worker"}),
    )
    .await;
    assert_eq!(sneaky["isError"], true);
}

#[tokio::test]
async fn a_viewer_without_the_scope_is_told_so_by_her_reports_tool() {
    let served = served(AssistantConfig::default()).await;
    file_report(
        &served,
        "report-g",
        ReportScope::Operation("G".to_owned()),
        "Visible to G",
    );
    let (_app, job) = accepted(&served, &served.viewer, "Reports?").await;
    let listed = data(
        &call_tool(&served, &job.mcp_token, "reports", json!({})).await,
        "reports",
    );
    assert_eq!(listed["reports"][0]["id"], "report-g");
    assert!(
        listed["coverage"]["not_permitted"]
            .as_array()
            .unwrap()
            .contains(&json!("m")),
        "{listed}"
    );
}

#[tokio::test]
async fn her_charges_tool_says_what_each_last_did_and_offers_its_page() {
    let served = served(AssistantConfig::default()).await;
    wake(
        &served,
        "operation:G",
        Duration::from_secs(600),
        "woke for a timer; the turn ended failed: no quota. It said: nothing",
    );
    wake(
        &served,
        "operation:L",
        Duration::from_secs(600),
        "woke for a timer; the turn ended completed",
    );
    let (_app, job) = accepted(&served, &served.operator, "What did the charges do?").await;
    let charges = data(
        &call_tool(&served, &job.mcp_token, "charges", json!({})).await,
        "charges",
    );
    let charges = charges.as_array().unwrap();
    // Operation L is not the operator's.
    assert_eq!(charges.len(), 1, "{charges:?}");
    let charge = &charges[0];
    assert_eq!(charge["scope"], "operation:G");
    assert_eq!(charge["last_ended"], "failed: no quota");
    assert!(
        charge["last_turn"]
            .as_str()
            .unwrap()
            .contains("the turn ended failed")
    );
    assert_eq!(charge["link"], "app:/charges?operation_id=G");
    assert_eq!(charge["route"]["page"], "charges");
}

#[tokio::test]
async fn her_refusals_tool_answers_with_the_refusal_its_revision_and_the_editor_for_the_station() {
    let served = served(AssistantConfig::default()).await;
    served
        .registry
        .check_in(
            "worker",
            BTreeMap::from([
                ("operations".to_owned(), json!(["G"])),
                ("capacity".to_owned(), json!(4)),
                (
                    "config".to_owned(),
                    json!({"revision": "77",
                           "refused": ["configuration revision 78: the roster does not parse"]}),
                ),
            ]),
        )
        .unwrap();
    let (_app, job) = accepted(
        &served,
        &served.operator,
        "Why did this Station refuse configuration?",
    )
    .await;
    let answer = data(
        &call_tool(
            &served,
            &job.mcp_token,
            "config_refusals",
            json!({"station_id": "worker"}),
        )
        .await,
        "config_refusals",
    );
    let refusals = answer["refusals"].as_array().unwrap();
    assert_eq!(refusals.len(), 1, "{answer}");
    let refusal = &refusals[0];
    assert_eq!(refusal["kind"], "config_refused");
    assert_eq!(refusal["revision"], "78");
    assert_eq!(refusal["scope"], "station:worker");
    assert_eq!(refusal["condition"], "current");
    assert!(
        refusal["detail"]
            .as_str()
            .unwrap()
            .contains("the roster does not parse")
    );
    assert_eq!(
        refusal["editor_link"],
        "app:/roster?kind=station&station_id=worker"
    );
    assert!(
        answer["coverage"]
            .as_array()
            .unwrap()
            .iter()
            .any(|read| read["source"] == "stations")
    );

    // Another Station's refusals are not this one's.
    let other = data(
        &call_tool(
            &served,
            &job.mcp_token,
            "config_refusals",
            json!({"station_id": "hq"}),
        )
        .await,
        "config_refusals",
    );
    assert!(other["refusals"].as_array().unwrap().is_empty(), "{other}");
}
