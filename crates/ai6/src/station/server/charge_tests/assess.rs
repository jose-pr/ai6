//! A charge assesses its Operation's Reports with its tools over the real endpoint, as a model
//! would: a stand-in reads what it is woken for and calls the tools, and the Registry shows what
//! each assessment did, what closes a Report and what opens it again.

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use super::decides::{Tools, events_of};
use super::*;
use crate::directorates::registry::approvals::{ApprovalStatus, Ruling};
use crate::directorates::registry::logbook::Severity;
use crate::directorates::registry::reports::{
    Report, ReportFiler, ReportLinkKind, ReportScope, ReportState,
};
use crate::station::server::assessing;
use crate::station::sources::SourceDesk;

/// What the stand-in did: each tool it called and what the tool answered.
static DONE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static TOOLS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static WAKES: AtomicUsize = AtomicUsize::new(0);

fn file(state: &DaemonState, tag: &str, id: &str, classification: Option<&str>) {
    let mut report = Report::new(
        id,
        ReportScope::Operation(names(tag).0),
        "finding",
        Severity::Info,
        format!("Title of {id}"),
        "What an Agent saw.",
        ReportFiler::Agent {
            agent: "007".to_owned(),
            run: "run-1".to_owned(),
            mission: "G-1".to_owned(),
        },
        100.0,
    );
    report.classification = classification.map(str::to_owned);
    state.intake.file_report(&report).unwrap();
}

/// Makes the charge act on each wake that carries a `plan` in a Cable: it calls the tools the
/// plan lists, in order, and what they answer is kept in [`DONE`].
fn acting(tag: &str) {
    let key = key(tag);
    TURNS.lock().unwrap().insert(key.clone(), {
        let key = key.clone();
        Arc::new(move |text: &str, environment: &BTreeMap<String, String>| {
            let plans: Vec<Value> = events_of(text)
                .into_iter()
                .filter(|event| event["kind"] == "cable")
                .filter_map(|event| event["detail"]["body"].as_str().map(str::to_owned))
                .filter_map(|body| serde_json::from_str::<Value>(&body).ok())
                .filter_map(|body| body.get("plan").cloned())
                .collect();
            if plans.is_empty() {
                return;
            }
            let (key, environment) = (key.clone(), environment.clone());
            std::thread::spawn(move || {
                let tools = Tools::connect(&key, &environment);
                *TOOLS.lock().unwrap() = tools.names();
                for plan in plans {
                    for step in plan.as_array().unwrap() {
                        let tool = step["tool"].as_str().unwrap();
                        let answer = tools.call(tool, step["arguments"].clone());
                        DONE.lock().unwrap().push((tool.to_owned(), answer));
                    }
                    WAKES.fetch_add(1, Ordering::SeqCst);
                }
                let _ = tools.runtime.block_on(tools.client.cancel());
            })
            .join()
            .unwrap();
        })
    });
}

fn wake(state: &DaemonState, tag: &str, plan: Value, key: &str) {
    let operation = state.roster.operation(&names(tag).0).unwrap();
    let desk = SourceDesk::new("test", Default::default()).with_cables(state.cables.clone());
    let notify = desk.notify_for(&state.roster, operation).unwrap();
    notify("plan", &json!({ "plan": plan }).to_string(), key).unwrap();
}

fn answers(tool: &str) -> Vec<Value> {
    DONE.lock()
        .unwrap()
        .iter()
        .filter(|(called, _)| called == tool)
        .map(|(_, answer)| {
            serde_json::from_str(answer).unwrap_or_else(|_| json!({ "text": answer }))
        })
        .collect()
}

fn report(state: &DaemonState, id: &str) -> Report {
    state.intake.report(id).unwrap().unwrap()
}

fn audits(state: &DaemonState, action: &str) -> usize {
    state
        .intake
        .audit_records()
        .unwrap()
        .iter()
        .filter(|record| record.action == action)
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_charge_assesses_reports_and_what_resolves_them_closes_them() {
    let root = tempfile::tempdir().unwrap();
    let tag = "a1";
    let mut state = station_with(
        root.path(),
        tag,
        "test",
        "",
        "",
        &Extra {
            operation_cables: true,
            ..Extra::default()
        },
    );
    for id in ["r-first", "r-second", "r-ask", "r-decide"] {
        file(&state, tag, id, None);
    }
    // Above what any Agent is cleared for: the charge may not read it, so it may not assess it.
    file(&state, tag, "r-secret", Some("confidential"));
    acting(tag);
    charges::tend(&state, false, true).await;

    let assess = |report: &str, arguments: Value| {
        let mut arguments = arguments;
        arguments["report"] = json!(report);
        json!({ "tool": "assess_report", "arguments": arguments })
    };
    wake(
        &state,
        tag,
        json!([
            { "tool": "reports", "arguments": {} },
            { "tool": "read_report", "arguments": { "id": "r-first" } },
            assess("r-second", json!({
                "assessment": "duplicate", "of": "r-first", "reason": "the same fault",
                "credibility": 3, "priority": "high",
                "likelihood": "likely", "confidence": "high",
            })),
            assess("r-ask", json!({
                "assessment": "awaiting", "question": "Which branch?", "ask": "cable:ops",
                "wait_seconds": 60, "reason": "cannot tell",
            })),
            assess("r-decide", json!({
                "assessment": "decision", "question": "May we drop the old API?",
                "reason": "it is a person's call",
            })),
            assess("r-secret", json!({ "assessment": "no_action", "reason": "peeking" })),
            assess("r-first", json!({ "assessment": "someday", "reason": "x" })),
            assess("r-first", json!({
                "assessment": "mission", "reason": "work, but which Mission?",
            })),
        ]),
        "plan-1",
    );
    tend_until(&state, "the first plan ran", || {
        WAKES.load(Ordering::SeqCst) >= 1
    })
    .await;

    let tools = TOOLS.lock().unwrap().clone();
    for name in ["reports", "read_report", "assess_report", "reopen_report"] {
        assert!(tools.iter().any(|tool| tool == name), "{name}: {tools:?}");
    }
    let listed = answers("reports").remove(0)["text"].to_string();
    assert!(
        listed.contains("r-first") && listed.contains("r-secret"),
        "{listed}"
    );
    assert!(
        answers("read_report")[0]["text"]
            .as_str()
            .unwrap()
            .contains("What an Agent saw")
    );

    let assessed = answers("assess_report");
    let said = |n: usize| assessed[n].to_string();
    assert_eq!(assessed[0]["assessed"], "duplicate", "{}", said(0));
    assert_eq!(assessed[0]["state"], "closed");
    assert_eq!(assessed[0]["judgement"], "likely (55-75%), high confidence");
    assert_eq!(assessed[1]["state"], "assessed", "{}", said(1));
    assert_eq!(assessed[2]["state"], "assessed", "{}", said(2));
    assert!(
        said(3).contains("not read in") || said(3).contains("no such"),
        "{}",
        said(3)
    );
    assert!(said(4).contains("mission, duplicate"), "{}", said(4));
    assert!(said(5).contains("names the Mission"), "{}", said(5));

    // The duplicate links both ways, and carries what the charge set; the words stay the filer's.
    let second = report(&state, "r-second");
    assert_eq!(second.state, ReportState::Closed);
    assert_eq!(
        (second.credibility, second.priority.as_deref()),
        (Some(3), Some("high"))
    );
    assert_eq!(
        second.assessment.as_ref().unwrap().by,
        format!("charge:operation:{}", names(tag).0)
    );
    assert_eq!(second.text, "What an Agent saw.");
    assert!(
        report(&state, "r-first")
            .links
            .iter()
            .any(|link| link.kind == ReportLinkKind::Report && link.id == "r-second")
    );
    assert_eq!(report(&state, "r-secret").state, ReportState::Open);
    assert_eq!(report(&state, "r-first").state, ReportState::Open);
    assert_eq!(
        audits(&state, "report.assess"),
        3,
        "each given assessment, and no refusal"
    );

    // The decision is green ink of kind decision for the Operation's signers.
    let decide = report(&state, "r-decide");
    let request = decide
        .links
        .iter()
        .find(|link| link.kind == ReportLinkKind::Decision)
        .unwrap()
        .id
        .clone();
    let operation = names(tag).0;
    let now = state.clock.now() as f64;
    let Some(ApprovalStatus::Pending(pending)) =
        state.intake.approval(&operation, &request, now).unwrap()
    else {
        panic!("the decision waits for a signature");
    };
    assert_eq!(pending.scope.kind, "decision");
    assert_eq!(
        pending.agent,
        names(tag).1,
        "filed under the charge's Agent"
    );

    // Nothing has ended yet.
    assessing::settle(&state);
    assert_eq!(report(&state, "r-decide").state, ReportState::Assessed);
    assert_eq!(report(&state, "r-ask").state, ReportState::Assessed);

    // Signed, it closes; past its deadline, the question comes back and the charge is told.
    state
        .intake
        .decide_approval(
            &operation,
            &request,
            &Ruling::approve("person-1", now, now + 3_600.0),
        )
        .unwrap();
    let later = now as i64 + 120;
    state.clock = Clock(Arc::new(move || later));
    assessing::settle(&state);
    let decided = report(&state, "r-decide");
    assert_eq!(decided.state, ReportState::Closed);
    assert_eq!(
        decided.resolution.as_ref().unwrap().outcome.as_deref(),
        Some("decision_approved")
    );
    let asked = report(&state, "r-ask");
    assert_eq!(asked.state, ReportState::Open, "its deadline came");
    assert_eq!(audits(&state, "report.close"), 1);
    assert_eq!(audits(&state, "report.reopen"), 1);
    tend_until(&state, "the charge was told it is open", || {
        calls(&key(tag))
            .iter()
            .any(|call| call.text.contains("reopened"))
    })
    .await;

    // The charge reopens the closed decision itself, with its reason; the report is open again.
    wake(
        &state,
        tag,
        json!([
            { "tool": "reopen_report", "arguments": {
                "report": "r-decide", "reason": "the signature was for something else",
            } },
            { "tool": "reopen_report", "arguments": { "report": "r-decide", "reason": "again" } },
        ]),
        "plan-2",
    );
    tend_until(&state, "the second plan ran", || {
        WAKES.load(Ordering::SeqCst) >= 2
    })
    .await;
    let reopened = answers("reopen_report");
    assert_eq!(reopened[0]["state"], "open", "{}", reopened[0]);
    assert!(
        reopened[1].to_string().contains("open already"),
        "{}",
        reopened[1]
    );
    assert_eq!(report(&state, "r-decide").state, ReportState::Open);
    assert_eq!(audits(&state, "report.reopen"), 2);
    charges::close_all(&state).await;
    state.stopping.store(true, Ordering::Release);
}
