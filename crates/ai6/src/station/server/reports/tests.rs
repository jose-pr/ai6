use std::path::Path as FsPath;
use std::sync::Arc;

use serde_json::{Value, json};

use super::super::reload::LiveConfig;
use super::super::tests::test_state;
use super::*;
use crate::bounds::Limit;
use crate::directorates::m::config::{ConfigProvider, FileProvider};
use crate::directorates::medical::exposure::ExposureGate;
use crate::directorates::registry::limits::{RegistryLimits, ReportLimits};
use crate::directorates::registry::store::FileRegistry;

const AGENTS: &str = "[agent.007]\nhandler = \"mock-recorder\"\nclearance = \"field\"\n\
                      [agent.licensed]\nhandler = \"mock-recorder\"\nclearance = \"00\"\n";

/// A Station reading its configuration from `root`, with `limits` on Reports; `agent:licensed`
/// is cleared to `confidential` and `agent:007` to `restricted` in the Registry.
fn station(root: &FsPath, limits: ReportLimits) -> DaemonState {
    let mut state = test_state(root, "clearance = \"00\"\n[operation.B]\n", AGENTS);
    state.intake = Arc::new(FileRegistry::open(root.join("registry")).with_limits(
        RegistryLimits {
            reports: limits,
            ..RegistryLimits::default()
        },
    ));
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
    for (subject, classification) in [
        ("agent:licensed", "confidential"),
        ("agent:007", "restricted"),
    ] {
        state
            .intake
            .put_report_clearance(&ReportClearance::new(
                subject,
                classification,
                "person:chief",
                1.0,
            ))
            .unwrap();
    }
    state
}

/// Writes `compartments.toml`: each `(id, actions, covers, read_in)`.
fn compartments(root: &FsPath, records: &[(&str, &[&str], &str, &[&str])]) {
    let mut text = String::new();
    for (id, actions, covers, read_in) in records {
        let actions: Vec<String> = actions.iter().map(|action| format!("{action:?}")).collect();
        let covers = if covers.is_empty() {
            "[]".to_owned()
        } else {
            format!("[{{ {covers} }}]")
        };
        let read_in: Vec<String> = read_in
            .iter()
            .map(|subject| format!("{{ subject = {subject:?} }}"))
            .collect();
        text.push_str(&format!(
            "[compartment.{id}]\nactions = [{}]\ncovers = {covers}\nread_in = [{}]\n",
            actions.join(", "),
            read_in.join(", ")
        ));
    }
    std::fs::write(root.join("compartments.toml"), text).unwrap();
}

fn reader(agent: &str, operation: &str, mission: &str) -> Reader {
    Reader {
        agent: agent.to_owned(),
        operation: operation.to_owned(),
        mission: Some(mission.to_owned()),
        exposure: ExposureGate::default(),
    }
}

fn filing(value: Value) -> ReportFiling {
    serde_json::from_value(value).unwrap()
}

/// The JSON inside what a read tool answers, checking it is fenced.
fn unfenced(answer: &str, tool: &str) -> Value {
    let opening = format!("<ai6-data tool=\"{tool}\">\n");
    assert!(answer.starts_with(&opening), "fenced as data: {answer}");
    assert!(answer.ends_with("\n</ai6-data>"), "{answer}");
    serde_json::from_str(&answer[opening.len()..answer.len() - "\n</ai6-data>".len()]).unwrap()
}

fn filed(state: &DaemonState, who: &Reader, value: Value) -> String {
    let answer = ReportDesk::new(state.clone())
        .file(who, None, filing(value))
        .unwrap();
    assert_eq!(answer["filed"], true, "{answer}");
    answer["id"].as_str().unwrap().to_owned()
}

fn read(state: &DaemonState, who: &Reader, id: &str) -> Value {
    let answer = ReportDesk::new(state.clone()).read(who, None, id).unwrap();
    if answer.starts_with('<') {
        unfenced(&answer, "read_report")
    } else {
        serde_json::from_str(&answer).unwrap()
    }
}

fn listed(state: &DaemonState, who: &Reader, scope: Option<&str>, mission: Option<&str>) -> Value {
    let answer = ReportDesk::new(state.clone())
        .list(who, None, scope, mission.map(str::to_owned), None)
        .unwrap();
    if answer.starts_with('<') {
        unfenced(&answer, "reports")
    } else {
        serde_json::from_str(&answer).unwrap()
    }
}

fn crossings(state: &DaemonState) -> usize {
    state
        .intake
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|record| record.action == "compartment.read")
        .count()
}

#[test]
fn an_agents_report_is_stored_linked_to_its_mission_and_read_under_its_operations_read() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path(), ReportLimits::default());
    let filer = reader("007", "G", "mission-1");
    let briefs = state.intake.filed_briefs("G", 100).unwrap().len();
    let missions = state.missions.lock().unwrap().list().len();
    let id = filed(
        &state,
        &filer,
        json!({"title": "A flaky test", "text": "parser_test fails one run in ten", "priority": "high"}),
    );
    let stored = state.intake.report(&id).unwrap().unwrap();
    assert_eq!(stored.scope, ReportScope::Operation("G".to_owned()));
    assert!(stored.about_mission("mission-1"), "linked to its Mission");
    assert!(stored.untrusted(), "an Agent's text is untrusted");
    assert_eq!(
        stored.filer,
        ReportFiler::Agent {
            agent: "007".to_owned(),
            session: "mission-1".to_owned(),
            mission: "mission-1".to_owned(),
        }
    );
    // Filing makes no Mission, brief or placement.
    assert_eq!(state.intake.filed_briefs("G", 100).unwrap().len(), briefs);
    assert_eq!(state.missions.lock().unwrap().list().len(), missions);

    // Another Session of the Operation reads it, and finds it by its Mission.
    let colleague = reader("licensed", "G", "mission-2");
    let shown = read(&state, &colleague, &id);
    assert_eq!(shown["text"], "parser_test fails one run in ten");
    assert_eq!(shown["sight"], "content");
    assert_eq!(shown["reliability"], "F", "a new source");
    let by_mission = listed(&state, &colleague, None, Some("mission-1"));
    assert_eq!(by_mission["reports"][0]["id"], id.as_str());

    // Another Operation's Session does not, unless a compartment gives it the Operation.
    let outsider = reader("007", "B", "B-1");
    assert_eq!(read(&state, &outsider, &id)["reason"], "no such report");
    assert_eq!(
        listed(&state, &outsider, Some("operation:G"), None)["status"],
        "refused"
    );
    assert_eq!(crossings(&state), 0);
    compartments(
        root.path(),
        &[(
            "CROSS",
            &["mission.read"],
            "operation = \"G\"",
            &["agent:007"],
        )],
    );
    let shown = read(&state, &outsider, &id);
    assert_eq!(shown["text"], "parser_test fails one run in ten");
    assert_eq!(crossings(&state), 1, "the crossing is audited");
    assert!(
        outsider
            .exposure
            .exposures()
            .iter()
            .any(|exposure| exposure.source == "product:G"),
        "another Operation's report exposes the reader"
    );
}

#[test]
fn a_marked_report_is_refused_to_an_agent_not_read_in_and_its_mission_shows_only_markings() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path(), ReportLimits::default());
    compartments(
        root.path(),
        &[("KESTREL", &["mission.status"], "", &["agent:007"])],
    );
    let filer = reader("007", "G", "mission-1");
    let id = filed(
        &state,
        &filer,
        json!({
            "title": "Credentials in a log",
            "text": "the token is in build.log",
            "compartments": ["KESTREL"],
        }),
    );
    // The filer is read in; another Session of the Operation is not.
    assert_eq!(read(&state, &filer, &id)["sight"], "content");
    let colleague = reader("licensed", "G", "mission-2");
    let refused = read(&state, &colleague, &id);
    assert_eq!(refused["status"], "refused");
    assert_eq!(refused["report"]["id"], id.as_str());
    assert_eq!(refused["report"]["compartments"], json!(["KESTREL"]));
    assert!(!refused.to_string().contains("token"), "{refused}");
    // The Mission's Reports show it only by ID and markings.
    let by_mission = listed(&state, &colleague, None, Some("mission-1"));
    let entry = &by_mission["reports"][0];
    assert_eq!(entry["id"], id.as_str());
    assert_eq!(entry["sight"], "markings");
    assert!(
        entry.get("title").is_none() && entry.get("text").is_none(),
        "{entry}"
    );
    // Read in, it reads; read out, it is refused again at the next call.
    compartments(
        root.path(),
        &[(
            "KESTREL",
            &["mission.status"],
            "",
            &["agent:007", "agent:licensed"],
        )],
    );
    assert_eq!(read(&state, &colleague, &id)["sight"], "content");
    compartments(
        root.path(),
        &[("KESTREL", &["mission.status"], "", &["agent:007"])],
    );
    assert_eq!(read(&state, &colleague, &id)["status"], "refused");
}

#[test]
fn a_classification_above_the_agents_is_refused_and_its_tearline_shown_in_its_place() {
    let root = tempfile::tempdir().unwrap();
    let state = station(root.path(), ReportLimits::default());
    let filer = reader("licensed", "G", "mission-1");
    let with_tearline = filed(
        &state,
        &filer,
        json!({
            "title": "A supplier is compromised",
            "text": "the vendor's CI signs with a leaked key",
            "classification": "confidential",
            "tearline": {"text": "do not trust new vendor builds", "classification": "restricted"},
        }),
    );
    let without = filed(
        &state,
        &filer,
        json!({"title": "Plain", "text": "secret detail", "classification": "confidential"}),
    );
    assert_eq!(read(&state, &filer, &without)["sight"], "content");
    // 007 is cleared to restricted: the tearline, never the content.
    let lower = reader("007", "G", "mission-2");
    let shown = read(&state, &lower, &with_tearline);
    assert_eq!(shown["sight"], "tearline");
    assert_eq!(shown["text"], "do not trust new vendor builds");
    assert!(!shown.to_string().contains("leaked"), "{shown}");
    let refused = read(&state, &lower, &without);
    assert_eq!(refused["status"], "refused");
    assert!(!refused.to_string().contains("secret detail"));
    // Revoked in the Registry, the next call holds it to the lowest: not even the tearline.
    assert!(state.intake.revoke_report_clearance("agent:007").unwrap());
    assert_eq!(read(&state, &lower, &with_tearline)["status"], "refused");
}

#[test]
fn the_rate_limit_refuses_an_agents_storm() {
    let root = tempfile::tempdir().unwrap();
    let state = station(
        root.path(),
        ReportLimits {
            per_session: Limit::at(2),
            ..ReportLimits::default()
        },
    );
    let filer = reader("007", "G", "mission-1");
    for index in 0..2 {
        filed(
            &state,
            &filer,
            json!({"title": format!("n{index}"), "text": "x"}),
        );
    }
    let refused = ReportDesk::new(state.clone())
        .file(&filer, None, filing(json!({"title": "storm", "text": "x"})))
        .unwrap_err();
    assert!(refused.contains("wait"), "{refused}");
    // Another Session is counted apart.
    ReportDesk::new(state.clone())
        .file(
            &reader("007", "G", "mission-2"),
            None,
            filing(json!({"title": "other", "text": "x"})),
        )
        .unwrap();
}
