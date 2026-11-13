//! An Operation's charge decides its Missions with a stand-in harness: woken for a Mission that
//! entered `verifying` with its final report, brief and deliverables, it uses its AI6 tools over
//! the real endpoint, as a model would, and the Station carries the decision out as it does a
//! person's.

use std::process::Command;

use rmcp::ServiceExt as _;
use rmcp::model::{CallToolRequestParams, ContentBlock};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};

use super::super::tests::{start_mission_by_id, wait_for_state, wait_for_verifying};
use super::*;
use crate::directorates::m::mission::MissionState;
use crate::directorates::m::scorecard::{self, Judgment};
use crate::directorates::registry::decisions::{self, VerifyingDecision};
use crate::directorates::registry::runs::{OutcomeVerdict, ProbeResult};
use crate::directorates::section_00::gate::{CABLE_TOKEN_ENV, CABLE_URL_ENV};

/// The Agents of the Operation's Missions: `ag` does the work and ends done at once, under a role
/// that owes a branch.
const WORKERS: &str = "[role.shipper]\npurpose = \"Ships\"\npolicy = \"Ship it.\"\n\
     deliverables = [\"branch:feature-x\"]\n\
     [agent.ag]\nhandler = \"mock-verdict\"\nmodel = \"clear\"\nroles = [\"shipper\"]\n";

/// What each tool a stand-in charge called answered, `TOOL=>ANSWER`, by the charge's Run name.
static SAID: Mutex<BTreeMap<String, Vec<String>>> = Mutex::new(BTreeMap::new());

/// What the charge's tools answered, once at least `count` calls have: the decision is recorded
/// before the call returns to the harness.
async fn said_after(tag: &str, count: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let said = SAID
            .lock()
            .unwrap()
            .get(&key(tag))
            .cloned()
            .unwrap_or_default();
        if said.len() >= count || Instant::now() > deadline {
            return said;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The charge's AI6 tools, called over the endpoint its environment names.
pub(super) struct Tools {
    pub(super) client: rmcp::service::RunningService<rmcp::RoleClient, ()>,
    pub(super) runtime: tokio::runtime::Runtime,
    key: String,
}

impl Tools {
    pub(super) fn connect(key: &str, environment: &BTreeMap<String, String>) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // The transport spawns its worker when it is made, so it is made inside the runtime.
        let client = runtime
            .block_on(async {
                let transport = StreamableHttpClientTransport::from_config(
                    StreamableHttpClientTransportConfig::with_uri(
                        environment[CABLE_URL_ENV].clone(),
                    )
                    .auth_header(environment[CABLE_TOKEN_ENV].clone()),
                );
                ().serve(transport).await
            })
            .unwrap();
        Self {
            runtime,
            client,
            key: key.to_owned(),
        }
    }

    pub(super) fn names(&self) -> Vec<String> {
        self.runtime
            .block_on(self.client.list_all_tools())
            .unwrap()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    pub(super) fn call(&self, tool: &str, arguments: Value) -> String {
        let answer = self
            .runtime
            .block_on(
                self.client.call_tool(
                    CallToolRequestParams::new(tool.to_owned())
                        .with_arguments(arguments.as_object().cloned().unwrap_or_default()),
                ),
            )
            .map(|result| {
                result
                    .content
                    .iter()
                    .filter_map(|part| match part {
                        ContentBlock::Text(text) => Some(text.text.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|error| format!("ERROR {error}"));
        SAID.lock()
            .unwrap()
            .entry(self.key.clone())
            .or_default()
            .push(format!("{tool}=>{answer}"));
        answer
    }
}

/// The events of a wake's text.
pub(super) fn events_of(text: &str) -> Vec<Value> {
    text.split_once("\n\n")
        .and_then(|(_, events)| serde_json::from_str(events).ok())
        .unwrap_or_default()
}

/// Makes the charge of `tag` act in each turn it takes: for each Mission that entered verifying,
/// `judge` is given its tools and the event's detail.
fn judging(tag: &str, judge: impl Fn(&Tools, &Value) + Send + Sync + 'static) {
    let (key, judge) = (key(tag), Arc::new(judge));
    TURNS.lock().unwrap().insert(key.clone(), {
        let key = key.clone();
        Arc::new(move |text: &str, environment: &BTreeMap<String, String>| {
            let verifying: Vec<Value> = events_of(text)
                .into_iter()
                .filter(|event| event["kind"] == "verifying")
                .map(|event| event["detail"].clone())
                .collect();
            if verifying.is_empty() {
                return;
            }
            let (judge, key, environment) = (judge.clone(), key.clone(), environment.clone());
            std::thread::spawn(move || {
                let tools = Tools::connect(&key, &environment);
                for detail in &verifying {
                    judge(&tools, detail);
                }
                let _ = tools.runtime.block_on(tools.client.cancel());
            })
            .join()
            .unwrap();
        })
    });
}

/// A 00 that probes what the Mission was to deliver: a missing deliverable is redispatched with
/// what to do about it, and otherwise the Mission is done.
fn probes_then_decides(tools: &Tools, detail: &Value) {
    let mission = detail["mission"].as_str().unwrap();
    let probed = tools.call("deliverable_probe", json!({ "mission": mission }));
    if probed.contains("\"missing\"") {
        tools.call(
            "redispatch",
            json!({
                "mission": mission,
                "brief_amendment": "Push the branch feature-x.",
                "reason": "branch feature-x is missing on the remote",
            }),
        );
    } else {
        tools.call(
            "verify_mission",
            json!({ "mission": mission, "decision": "done", "reason": "every deliverable is there" }),
        );
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
}

/// The Operation's checkout with a pushed `main` on a bare `origin`, and `feature-x` pushed too
/// when `with_branch`. Made once the Mission's Run is done: a Run started in a repository
/// would be given a safe house, which this Station's test providers do not include.
fn remote(root: &Path, with_branch: bool) {
    let (bare, work) = (root.join("origin.git"), root.join("checkout"));
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--bare", "--initial-branch=main"]);
    git(&work, &["init", "--initial-branch=main"]);
    git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
    std::fs::write(work.join("a.txt"), "one").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "one"]);
    git(&work, &["push", "origin", "main"]);
    if with_branch {
        git(&work, &["branch", "feature-x"]);
        git(&work, &["push", "origin", "feature-x"]);
    }
}

/// Files a brief for the role `shipper` in the Operation of `tag`, has the Station claim it and
/// start its Mission with `ag`, and waits for the Mission to be verifying. Its UID.
async fn verifying_mission(state: &DaemonState, tag: &str, name: &str) -> String {
    let brief: crate::Brief = serde_json::from_value(json!({
        "title": format!("Ship {name}"),
        "body": "Put the work on branch feature-x.",
        "id": format!("{name}-{}", std::process::id()),
        "source": "manual",
        "operation": names(tag).0,
        "agent": "ag",
        "role": "shipper",
        "url": null,
        "stations": [],
    }))
    .unwrap();
    state.intake.submit_brief(&brief).unwrap();
    schedule_intake_once(state);
    let mission = brief.key();
    start_mission_by_id(state, &mission).await;
    wait_for_verifying(state, &mission).await;
    mission
}

/// Tends the charges and settles what has been decided, as the Station's beat does, until `done`.
async fn tend_and_settle(state: &DaemonState, what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(90);
    while !done() {
        assert!(Instant::now() < deadline, "never happened: {what}");
        charges::tend(state, false, true).await;
        super::super::verdicts::settle_held(state);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn state_of(state: &DaemonState, mission: &str) -> MissionState {
    state
        .missions
        .lock()
        .unwrap()
        .get(mission)
        .map(|mission| mission.state)
        .unwrap()
}

fn attempt(state: &DaemonState, mission: &str, attempt: u32) -> crate::RunRecord {
    state
        .intake
        .runs()
        .unwrap()
        .into_iter()
        .find(|record| record.mission_id == mission && record.number == attempt)
        .unwrap()
}

fn station_for(root: &Path, tag: &str) -> DaemonState {
    let state = station_with(
        root,
        tag,
        "test",
        "",
        "",
        &Extra {
            agents: WORKERS,
            ..Extra::default()
        },
    );
    publish_station_check_in(&state);
    state
}

fn person_decides(state: &DaemonState, tag: &str, mission: &str, verdict: OutcomeVerdict) {
    let operation = names(tag).0;
    decisions::decide(
        state.intake.as_ref(),
        &VerifyingDecision::new(
            &operation,
            mission,
            verdict,
            "checked by hand",
            "person-m",
            unix_now(),
        ),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mission_whose_branch_is_missing_is_redispatched_with_the_reason_and_its_next_run_runs()
 {
    let root = tempfile::tempdir().unwrap();
    let tag = "d1";
    let state = station_for(root.path(), tag);
    let mission = verifying_mission(&state, tag, "missing").await;
    remote(root.path(), false);
    judging(tag, probes_then_decides);

    tend_and_settle(&state, "the Mission is queued again", || {
        state_of(&state, &mission) == MissionState::Queued
    })
    .await;

    // It was woken with the final report, the brief and the deliverables, and probed before it
    // decided.
    let woken = calls(&key(tag));
    let wake = &woken[0].text;
    for expected in [
        "\"verifying\"",
        "All good.",
        "Put the work on branch feature-x.",
        "branch:feature-x",
        "untrusted",
    ] {
        assert!(wake.contains(expected), "{expected}: {wake}");
    }
    let tools = said_after(tag, 2).await;
    assert!(tools[0].starts_with("deliverable_probe=>"), "{tools:?}");
    assert!(tools[0].contains("\"missing\"") && tools[0].contains("feature-x"));
    assert!(tools[1].starts_with("redispatch=>"), "{tools:?}");

    // What the probe found is also a Report of the Operation, tied to the Mission.
    let readbacks = state
        .intake
        .reports(&crate::directorates::registry::reports::ReportFilter::all().of_kind("evidence"))
        .unwrap();
    assert_eq!(readbacks.len(), 1, "{readbacks:?}");
    assert!(
        readbacks[0].about_mission(&mission),
        "{:?}",
        readbacks[0].links
    );
    assert!(
        readbacks[0].text.contains("branch:feature-x: missing"),
        "{}",
        readbacks[0].text
    );
    assert_eq!(
        readbacks[0].severity,
        crate::directorates::registry::logbook::Severity::Warning
    );

    // Its probes are the Mission's evidence, and the decision is the run's verdict, audited,
    // on the timeline, counted against the run's Agent, and told to the next run.
    let first = attempt(&state, &mission, 1);
    assert_eq!(
        first
            .evidence
            .iter()
            .map(|found| (found.deliverable.to_string(), found.result))
            .collect::<Vec<_>>(),
        [("branch:feature-x".to_owned(), ProbeResult::Missing)]
    );
    let verdict = first.verdict.clone().unwrap();
    assert_eq!(verdict.verdict, OutcomeVerdict::Redispatch);
    assert_eq!(verdict.by, format!("charge:operation:{}", names(tag).0));
    assert_eq!(
        verdict.note.as_deref(),
        Some("branch feature-x is missing on the remote")
    );
    assert_eq!(scorecard::judge(&first), Judgment::Disputed);
    let audited: Vec<_> = state
        .intake
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|record| record.target.starts_with(&mission))
        .map(|record| (record.action, record.actor))
        .collect();
    assert!(
        audited.contains(&("mission.verify".to_owned(), verdict.by.clone())),
        "{audited:?}"
    );
    assert!(
        audited.contains(&("mission.probe".to_owned(), verdict.by.clone())),
        "{audited:?}"
    );
    assert!(
        state
            .intake
            .timeline_events(&names(tag).0)
            .unwrap()
            .iter()
            .any(|event| event.kind == "verify"
                && event.action == "redispatch"
                && event.actor.as_deref() == Some(verdict.by.as_str())),
        "on the timeline"
    );
    let queued = state
        .missions
        .lock()
        .unwrap()
        .get(&mission)
        .cloned()
        .unwrap();
    let task = queued.task();
    assert!(
        task.contains("branch feature-x is missing on the remote")
            && task.ends_with("Push the branch feature-x."),
        "{task}"
    );

    // The next run runs. A repository would be given a safe house, which this Station's test
    // providers do not include.
    std::fs::remove_dir_all(root.path().join("checkout").join(".git")).unwrap();
    start_mission_by_id(&state, &mission).await;
    wait_for_verifying(&state, &mission).await;
    let second = attempt(&state, &mission, 2);
    assert_eq!(
        second.predecessor.as_deref(),
        Some(first.id.as_str())
    );
    assert!(second.verdict.is_none());
    charges::close_all(&state).await;
    state.stopping.store(true, Ordering::Release);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_verified_mission_settles_done() {
    let root = tempfile::tempdir().unwrap();
    let tag = "d2";
    let state = station_for(root.path(), tag);
    let mission = verifying_mission(&state, tag, "verified").await;
    remote(root.path(), true);
    judging(tag, probes_then_decides);

    tend_and_settle(&state, "the Mission is done", || {
        state_of(&state, &mission) == MissionState::Done
    })
    .await;
    let tools = said_after(tag, 2).await;
    assert!(
        tools[0].starts_with("deliverable_probe=>") && tools[0].contains("\"found\""),
        "{tools:?}"
    );
    assert!(tools[1].starts_with("verify_mission=>"), "{tools:?}");
    let record = attempt(&state, &mission, 1);
    assert_eq!(record.verdict.unwrap().verdict, OutcomeVerdict::Confirmed);
    assert_eq!(record.evidence.len(), 1);
    assert_eq!(
        scorecard::judge(&attempt(&state, &mission, 1)),
        Judgment::Done
    );
    charges::close_all(&state).await;
    state.stopping.store(true, Ordering::Release);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charges_failed_settles_the_mission_failed() {
    let root = tempfile::tempdir().unwrap();
    let tag = "d3";
    let state = station_for(root.path(), tag);
    let mission = verifying_mission(&state, tag, "failing").await;
    judging(tag, |tools, detail| {
        let mission = detail["mission"].as_str().unwrap();
        // A decision needs its reason.
        let refused = tools.call(
            "verify_mission",
            json!({ "mission": mission, "decision": "failed", "reason": " " }),
        );
        assert!(refused.contains("needs its reason"), "{refused}");
        tools.call(
            "verify_mission",
            json!({ "mission": mission, "decision": "failed", "reason": "the work is not there" }),
        );
    });

    tend_and_settle(&state, "the Mission failed", || {
        state_of(&state, &mission) == MissionState::Failed
    })
    .await;
    let record = attempt(&state, &mission, 1);
    assert_eq!(
        record.verdict.unwrap().verdict,
        OutcomeVerdict::FailedVerification
    );
    assert_eq!(
        scorecard::judge(&attempt(&state, &mission, 1)),
        Judgment::Disputed
    );
    charges::close_all(&state).await;
    state.stopping.store(true, Ordering::Release);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persons_later_decision_overrides_the_charges() {
    let root = tempfile::tempdir().unwrap();
    let tag = "d4";
    let state = station_for(root.path(), tag);
    let mission = verifying_mission(&state, tag, "overridden").await;
    judging(tag, |tools, detail| {
        let mission = detail["mission"].as_str().unwrap();
        tools.call(
            "verify_mission",
            json!({ "mission": mission, "decision": "failed", "reason": "no branch" }),
        );
    });
    // The charge decides; nothing is settled until the Station's beat applies it.
    tend_until(&state, "the charge decided", || {
        attempt(&state, &mission, 1).verdict.is_some()
    })
    .await;
    assert_eq!(state_of(&state, &mission), MissionState::Verifying);
    assert_eq!(
        attempt(&state, &mission, 1).verdict.unwrap().verdict,
        OutcomeVerdict::FailedVerification
    );

    // A person decides otherwise, and theirs stands.
    person_decides(&state, tag, &mission, OutcomeVerdict::Confirmed);
    let decided = attempt(&state, &mission, 1).verdict.unwrap();
    assert_eq!(
        (decided.verdict, decided.by.as_str()),
        (OutcomeVerdict::Confirmed, "person-m")
    );
    super::super::verdicts::settle_held(&state);
    wait_for_state(&state, &mission, MissionState::Done).await;

    // A charge never replaces a person's decision.
    let refused = {
        let desk =
            charges::decide::DecisionDesk::new(state.clone(), names(tag).0, names(tag).1, key(tag));
        desk.decide(&mission, "failed", "again", None)
    };
    assert!(refused.is_err());
    assert_eq!(attempt(&state, &mission, 1).verdict.unwrap().by, "person-m");
    charges::close_all(&state).await;
    state.stopping.store(true, Ordering::Release);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_charge_at_desk_clearance_only_recommends() {
    let root = tempfile::tempdir().unwrap();
    let tag = "d5";
    let state = station_with(
        root.path(),
        tag,
        "test",
        "",
        "",
        &Extra {
            charge_agent: "retire_on = \"2000-01-01\"\n",
            agents: WORKERS,
            ..Extra::default()
        },
    );
    publish_station_check_in(&state);
    let mission = verifying_mission(&state, tag, "advised").await;
    judging(tag, |tools, detail| {
        let mission = detail["mission"].as_str().unwrap();
        tools.call(
            "redispatch",
            json!({ "mission": mission, "reason": "no branch", "brief_amendment": "Push it." }),
        );
    });
    tend_and_settle(&state, "the charge recommended", || {
        attempt(&state, &mission, 1).recommendation.is_some()
    })
    .await;
    let advised = said_after(tag, 1).await;
    assert!(advised[0].contains("\"recommended\""), "{advised:?}");
    for _ in 0..3 {
        super::super::verdicts::settle_held(&state);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let record = attempt(&state, &mission, 1);
    assert!(record.verdict.is_none(), "a recommendation decides nothing");
    assert_eq!(
        record.recommendation.unwrap().verdict,
        OutcomeVerdict::Redispatch
    );
    assert_eq!(state_of(&state, &mission), MissionState::Verifying);
    assert!(
        state
            .intake
            .audit_records()
            .unwrap()
            .iter()
            .any(|record| record.action == "mission.recommend"),
        "audited"
    );

    // A person confirms it by deciding the same way.
    person_decides(&state, tag, &mission, OutcomeVerdict::Confirmed);
    super::super::verdicts::settle_held(&state);
    wait_for_state(&state, &mission, MissionState::Done).await;
    charges::close_all(&state).await;
    state.stopping.store(true, Ordering::Release);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cable_to_the_operation_reaches_its_charge_and_creates_no_mission() {
    let root = tempfile::tempdir().unwrap();
    let tag = "d6";
    let state = station_with(
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
    let operation = names(tag).0;
    let before = state.missions.lock().unwrap().list().len();
    charges::tend(&state, false, true).await;
    let from = CablePrincipal::try_new(
        "007".to_owned(),
        operation.clone(),
        None,
        "test".to_owned(),
        None,
        None,
    )
    .unwrap();
    let sent = state
        .cables
        .send(
            &from,
            CableSendRequest::new(
                CableTarget::try_new(CableScope::Operation, Some(operation.clone())).unwrap(),
                "please look at the release".to_owned(),
                "to-the-operation-1".to_owned(),
            ),
        )
        .unwrap();
    assert_eq!(
        sent.recipients()
            .iter()
            .map(|recipient| recipient.agent())
            .collect::<Vec<_>>(),
        [names(tag).1.as_str()],
        "the Operation's coordinating role is its charge"
    );
    tend_until(&state, "a turn for the Cable", || turns(&state, tag) == 1).await;
    let first = &calls(&key(tag))[0];
    assert!(
        first.text.contains("\"cable\"") && first.text.contains("please look at the release"),
        "{}",
        first.text
    );
    assert_no_mission(&state, tag);
    assert_eq!(state.missions.lock().unwrap().list().len(), before);
    assert!(state.intake.runs().unwrap().is_empty());
    charges::close_all(&state).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_charge_has_the_decision_tools_and_proposes_missions_as_its_own() {
    let root = tempfile::tempdir().unwrap();
    let tag = "d7";
    let state = station(root.path(), tag, "test", "", "");
    static LISTED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static PROPOSED: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let charge_key = key(tag);
    TURNS.lock().unwrap().insert(
        charge_key.clone(),
        Arc::new({
            let charge_key = charge_key.clone();
            move |text: &str, environment: &BTreeMap<String, String>| {
                // The first turn starts the Run; the proposal waits for a bound one.
                if !text.contains("second") {
                    return;
                }
                let (environment, charge_key) = (environment.clone(), charge_key.clone());
                std::thread::spawn(move || {
                    let tools = Tools::connect(&charge_key, &environment);
                    *LISTED.lock().unwrap() = tools.names();
                    let answer = tools.call(
                        "propose_mission",
                        json!({
                            "title": "Cut the release",
                            "body": "Tag it.",
                            "idempotency_key": "cut-1",
                        }),
                    );
                    PROPOSED.lock().unwrap().push(answer);
                    let _ = tools.runtime.block_on(tools.client.cancel());
                })
                .join()
                .unwrap();
            }
        }),
    );
    charges::tend(&state, false, true).await;
    control(&state, tag, "first");
    tend_until(&state, "the first turn", || turns(&state, tag) == 1).await;
    control(&state, tag, "second");
    tend_until(&state, "the second turn", || turns(&state, tag) == 2).await;

    let tools = LISTED.lock().unwrap().clone();
    for tool in [
        "operation_missions",
        "mission_detail",
        "deliverables",
        "deliverable_probe",
        "verify_mission",
        "redispatch",
        "propose_mission",
    ] {
        assert!(tools.iter().any(|name| name == tool), "{tool}: {tools:?}");
    }
    let answer = PROPOSED.lock().unwrap().join(" ");
    assert!(answer.contains("pending_authorization"), "{answer}");
    let proposals = state
        .intake
        .list_mission_proposals(&names(tag).0, 10)
        .unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].parent_mission_id.as_str(), key(tag));
    assert_eq!(proposals[0].proposer_agent_id, names(tag).1);
    charges::close_all(&state).await;
}

#[test]
fn a_charge_agent_without_00_is_refused_at_configuration() {
    let write = |root: &Path, clearance: &str| {
        std::fs::write(
            root.join("operations.toml"),
            "[operation.G]\npath = \"/tmp\"\nclearance = \"00\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("agents.toml"),
            format!("[agent.chief]\nhandler = \"mock-charge\"\nclearance = \"{clearance}\"\n"),
        )
        .unwrap();
        std::fs::write(
            root.join("charges.toml"),
            "[charge.\"operation:G\"]\nagent = \"chief\"\n[charge.\"station:s\"]\nagent = \"chief\"\n",
        )
        .unwrap();
    };
    for clearance in ["field", "desk"] {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), clearance);
        let refused = crate::directorates::m::roster::Roster::load(root.path())
            .err()
            .unwrap()
            .to_string();
        assert!(
            refused.contains("operation:G")
                && refused.contains("chief")
                && refused.contains(clearance)
                && refused.contains("holds 00"),
            "{clearance}: {refused}"
        );
    }
    let root = tempfile::tempdir().unwrap();
    write(root.path(), "00");
    let roster = crate::directorates::m::roster::Roster::load(root.path()).unwrap();
    assert_eq!(roster.charges().len(), 2);
}
