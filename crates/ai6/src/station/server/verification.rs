//! A Session that ends done does not settle its Mission: the Mission is `verifying` until
//! someone decides it, and the Station only provides what a decision is made from.
//!
//! The Station records what the Mission was to deliver, reads those things back on request with
//! read-only probes (`station::probes`), keeps what each found as evidence on the attempt, and
//! carries out the decision a person (later, the Operation's 00) records in the Registry: done,
//! failed, or the next attempt with the reason. It never decides whether the work is enough.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use coqui_acp_provider::AgentEvent;

use super::{DaemonState, agents, head_of_station, publish_lifecycle, session_hold, vetting};
use crate::directorates::m::deliverables::{self, Deliverable, DeliverableKind};
use crate::directorates::m::mission::{Mission, MissionState};
use crate::directorates::m::theaters::{LOCAL_THEATER, TheaterAccess};
use crate::directorates::registry::sessions::{
    Evidence, MissionVerdict, ProbeResult, SessionStatus, SessionUpdate,
};
use crate::directorates::registry::tracker::SourceRead;
use crate::station::probes;
use crate::station::sources::{build_source, build_tracker};

/// The event kind that leaves a Mission to be verified.
pub(super) const VERIFYING_EVENT: &str = "verifying";

/// The sessions a probe is running for, so a pass that finds the request still open does not
/// start a second one.
static PROBING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Whether a Mission's `done` waits to be verified. The Missions that supervise others' work
/// (reviews, panels, Head of Station rounds) and Missions with no Operation, whose outcome is
/// nobody's to answer for, do not.
fn owes_verification(state: &DaemonState, mission: &Mission) -> bool {
    mission.operation_id.is_some()
        && !vetting::is_review_mission(mission)
        && !head_of_station::is_round(state, mission)
}

/// `event`, or for a `completed` whose Mission owes verification, the event that leaves it
/// verifying instead. A Mission already held or verifying is ending by a decision.
pub(super) fn verify_done(
    state: &DaemonState,
    mission_id: &str,
    mut event: AgentEvent,
) -> AgentEvent {
    if event.kind != "completed" {
        return event;
    }
    let owes = state.missions.lock().ok().is_some_and(|missions| {
        missions.get(mission_id).is_some_and(|mission| {
            !matches!(mission.state, MissionState::Held | MissionState::Verifying)
                && owes_verification(state, mission)
        })
    });
    if owes {
        event.kind = VERIFYING_EVENT.to_owned();
    }
    event
}

/// Records on the Mission's latest attempt what it was to deliver: its role's deliverables and its
/// brief's.
pub(super) fn begin(state: &DaemonState, mission_id: &str) {
    let Some(hold) = session_hold(state, mission_id) else {
        return;
    };
    let declared = declared_by(state, mission_id).unwrap_or_else(|error| {
        eprintln!("ai6: Mission {mission_id:?}: its deliverables were not read: {error}");
        Vec::new()
    });
    let update = SessionUpdate::default().deliverables(declared);
    if let Err(error) = state
        .intake
        .update_session(&hold.id, &state.station_id, hold.fence, update)
    {
        eprintln!("ai6: Mission {mission_id:?}: its deliverables were not recorded: {error}");
    }
}

fn declared_by(state: &DaemonState, mission_id: &str) -> Result<Vec<Deliverable>, String> {
    let mission = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
        .ok_or("the Mission is not in this Station's book")?;
    let role = match (mission.role_id.as_deref(), mission.operation_id.as_deref()) {
        (Some(role), operation) => super::configured_agent_role(state, operation, role)
            .map(|role| role.deliverables)
            .unwrap_or_default(),
        (None, _) => Vec::new(),
    };
    let data = state
        .intake
        .filed_brief(mission_id)
        .map_err(|error| error.to_string())?
        .map(|filed| filed.brief.data)
        .unwrap_or_default();
    deliverables::declared(&role, &data).map_err(|error| error.to_string())
}

/// Whether the Mission's working directory is on this Station's own disk.
fn works_here(state: &DaemonState, mission_id: &str) -> Option<PathBuf> {
    let manifest = state.missions.lock().ok()?.operation(mission_id)?;
    let theater = manifest
        .theater_id
        .clone()
        .unwrap_or_else(|| LOCAL_THEATER.to_owned());
    let local = state
        .transports
        .theater(&theater)
        .is_none_or(|known| known.access == TheaterAccess::Local);
    let dir = PathBuf::from(manifest.working_directory);
    (local && dir.is_dir()).then_some(dir)
}

/// Probes `deliverables` of the Mission, each within its `[probes]` timeout, and says what each
/// found. A kind it cannot look at from here is `unprobed`, with the reason.
pub(super) fn probe_all(
    state: &DaemonState,
    mission: &Mission,
    deliverables: &[Deliverable],
) -> Vec<Evidence> {
    let mission_id = mission.mission_id.as_str();
    let limits = state.station.limits().probes.clone();
    let work = works_here(state, mission_id);
    // The directory `git` reads a remote from: the Mission's own, else its Operation's checkout.
    let repo = work.clone().or_else(|| {
        let operation = state
            .roster
            .operation(mission.operation_id.as_deref()?)
            .ok()?;
        operation.path.clone().filter(|path| path.is_dir())
    });
    let own_branch = mission
        .safe_house
        .as_ref()
        .and_then(|house| house.branch.clone());
    deliverables
        .iter()
        .map(|deliverable| {
            let (result, detail) = probe_one(
                state,
                mission,
                deliverable,
                (repo.as_deref(), work.as_deref(), own_branch.as_deref()),
                &limits,
            );
            Evidence::new(
                deliverable.clone(),
                result,
                detail,
                state.clock.now() as f64,
            )
        })
        .collect()
}

fn probe_one(
    state: &DaemonState,
    mission: &Mission,
    deliverable: &Deliverable,
    (repo, work, own_branch): (Option<&Path>, Option<&Path>, Option<&str>),
    limits: &crate::station::limits::ProbeLimits,
) -> (ProbeResult, String) {
    let unprobed = |why: &str| (ProbeResult::Unprobed, why.to_owned());
    let target = deliverable.target.as_deref();
    let git_wait = limits.git_timeout.0;
    match deliverable.kind {
        DeliverableKind::Branch => {
            let Some(repo) = repo else {
                return unprobed("the work is not on this Station's disk");
            };
            match target.or(own_branch) {
                Some(name) => probes::branch(repo, name, git_wait),
                None => unprobed("no branch is named and the Mission has none"),
            }
        }
        DeliverableKind::CommitOn => {
            let Some(repo) = repo else {
                return unprobed("the work is not on this Station's disk");
            };
            let (branch, commit) = target
                .and_then(|target| target.split_once('@'))
                .map_or((target.unwrap_or_default(), None), |(branch, commit)| {
                    (branch, Some(commit))
                });
            probes::commit_on(repo, work, (branch, commit), git_wait)
        }
        DeliverableKind::File => {
            let Some(work) = work else {
                return unprobed("the work is not on this Station's disk");
            };
            let (work, relative) = (work.to_path_buf(), target.unwrap_or_default().to_owned());
            probes::within(limits.file_timeout.0, move || {
                probes::file(&work, &relative)
            })
            .unwrap_or_else(|| timed_out(limits.file_timeout.0))
        }
        DeliverableKind::Artifact => {
            probes::artifact(target.unwrap_or_default(), limits.http_timeout.0)
        }
        DeliverableKind::Approval => {
            let Some(operation) = mission.operation_id.as_deref() else {
                return unprobed("the Mission has no Operation");
            };
            let now = state.clock.now() as f64;
            match state
                .intake
                .mission_approvals(operation, mission.mission_id.as_str(), now)
            {
                Ok(statuses) => probes::approval(&statuses, target),
                Err(error) => (
                    ProbeResult::Failed,
                    format!("approvals could not be read: {error}"),
                ),
            }
        }
        DeliverableKind::Comment => comment(state, mission, target.unwrap_or_default(), limits),
    }
}

fn timed_out(after: Duration) -> (ProbeResult, String) {
    (
        ProbeResult::Failed,
        format!("no answer within {}s", after.as_secs_f64()),
    )
}

/// Reads back the tracker items the Mission is linked to and looks for `text` among their latest
/// comments. Only a Station that holds the tracker's credentials can read it.
fn comment(
    state: &DaemonState,
    mission: &Mission,
    text: &str,
    limits: &crate::station::limits::ProbeLimits,
) -> (ProbeResult, String) {
    let Some(operation) = mission.operation_id.clone() else {
        return (
            ProbeResult::Unprobed,
            "the Mission has no Operation".to_owned(),
        );
    };
    let links = match state.intake.issue_links(mission.mission_id.as_str()) {
        Ok(links) => links,
        Err(error) => {
            return (
                ProbeResult::Failed,
                format!("links could not be read: {error}"),
            );
        }
    };
    if links.is_empty() {
        return (
            ProbeResult::Unprobed,
            "the Mission is linked to no tracker item".to_owned(),
        );
    }
    let wait = limits.tracker_timeout.0;
    let most = u32::try_from(limits.comments.or_max()).unwrap_or(u32::MAX);
    let mut last = (ProbeResult::Missing, String::new());
    for link in links {
        let read = SourceRead {
            id: "probe".to_owned(),
            operation: operation.clone(),
            tracker: Some(link.tracker.clone()),
            item: Some(link.item.clone()),
            query: None,
            comments: most,
            items: 0,
        };
        let worker = state.clone();
        let answered = probes::within(wait, move || {
            let plugins = worker.plugins.clone();
            crate::station::source_reads::answer(
                &read,
                &worker.roster,
                &worker.station.limits().sources,
                &|provider: &str, settings: &toml::Table| {
                    build_source(&plugins, provider, settings)
                },
                &|provider: &str, settings: &toml::Table| {
                    build_tracker(&plugins, provider, settings)
                },
            )
        });
        let name = format!("{}:{}", link.tracker, link.item);
        let found = match answered {
            None => timed_out(wait),
            Some(answer) => match answer.error.filter(|_| answer.item.is_none()) {
                Some(error) => (ProbeResult::Failed, error),
                None => {
                    let bodies: Vec<String> = answer.comments.into_iter().map(|c| c.body).collect();
                    probes::comment(&name, &bodies, text)
                }
            },
        };
        if found.0 == ProbeResult::Found {
            return found;
        }
        last = found;
    }
    last
}

/// Probes every deliverable of the attempt `mission_id` is on and keeps what was found as its
/// evidence, answering the request for it.
pub(super) fn probe_and_record(state: &DaemonState, mission_id: &str) -> Result<usize, String> {
    let mission = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
        .ok_or("the Mission is not in this Station's book")?;
    let hold = session_hold(state, mission_id).ok_or("the Mission has no Session record")?;
    let record = state
        .intake
        .session(&hold.id)
        .map_err(|error| error.to_string())?
        .ok_or("the Session record is gone")?;
    let evidence = probe_all(state, &mission, &record.deliverables);
    let found = evidence.len();
    let update = SessionUpdate::default().evidence(evidence).probed();
    state
        .intake
        .update_session(&hold.id, &state.station_id, hold.fence, update)
        .map_err(|error| error.to_string())?;
    Ok(found)
}

/// Runs the probes a person asked for on Missions of this Station that are verifying, each on a
/// thread of its own so a slow remote never holds up the Station's beat.
pub(super) fn answer_probe_requests(state: &DaemonState) {
    let verifying: Vec<String> = state
        .missions
        .lock()
        .map(|missions| {
            missions
                .list()
                .into_iter()
                .filter(|mission| mission.state == MissionState::Verifying)
                .map(|mission| mission.mission_id.to_string())
                .collect()
        })
        .unwrap_or_default();
    for mission_id in verifying {
        let Some(hold) = session_hold(state, &mission_id) else {
            continue;
        };
        let asked = state
            .intake
            .session(&hold.id)
            .ok()
            .flatten()
            .is_some_and(|record| record.probe_requested.is_some());
        if !asked {
            continue;
        }
        let claimed = PROBING.lock().is_ok_and(|mut running| {
            let fresh = !running.contains(&hold.id);
            if fresh {
                running.push(hold.id.clone());
            }
            fresh
        });
        if !claimed {
            continue;
        }
        let worker = state.clone();
        std::thread::spawn(move || {
            if let Err(error) = probe_and_record(&worker, &mission_id) {
                eprintln!("ai6: Mission {mission_id:?}: its deliverables were not probed: {error}");
            }
            if let Ok(mut running) = PROBING.lock() {
                running.retain(|id| *id != hold.id);
            }
        });
    }
}

/// Queues a verifying Mission as its next attempt, told what the person who decided said was
/// wrong. The attempt that was verified is over: it keeps its verdict, and so counts against its
/// Agent.
pub(super) fn redispatch(state: &DaemonState, mission: &Mission, verdict: &MissionVerdict) {
    let mission_id = mission.mission_id.as_str();
    let agent = match agents::retry_agent(state, mission) {
        Ok(agent) => agent,
        Err(reason) => {
            eprintln!("ai6: Mission {mission_id:?} was not redispatched: {reason}");
            return;
        }
    };
    let attempt = session_hold(state, mission_id).map_or(1, |hold| hold.attempt);
    let mut told = format!(
        "Attempt {attempt} of this Mission was not accepted by {}: {}",
        verdict.by,
        verdict.note.as_deref().unwrap_or("no reason was given"),
    );
    if let Some(amendment) = &verdict.amendment {
        told.push_str("\n\n");
        told.push_str(amendment);
    }
    if let Some(hold) = session_hold(state, mission_id) {
        let _ = state.intake.update_session(
            &hold.id,
            &state.station_id,
            hold.fence,
            SessionUpdate::default().status(SessionStatus::Ended),
        );
    }
    let queued = state
        .missions
        .lock()
        .map_err(|_| "the Mission book is unavailable".to_owned())
        .and_then(|mut missions| {
            missions
                .redispatch(mission_id, agent, told)
                .map_err(|error| format!("{error:?}"))
        });
    match queued {
        Ok(queued) => publish_lifecycle(state, mission_id, queued.state),
        Err(error) => eprintln!("ai6: Mission {mission_id:?} was not redispatched: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::super::tests::{
        start_mission_by_id, test_state, wait_for_state, wait_for_verifying,
    };
    use super::*;
    use crate::directorates::m::identity::MissionId;
    use crate::directorates::m::scorecard::{self, Judgment};
    use crate::directorates::registry::sessions::OutcomeVerdict;

    /// Agents that finish at once, under a role that owes a branch that was never pushed and a
    /// file that was never written, and under one that owes nothing.
    const AGENTS: &str = "[role.shipper]\npurpose = \"Ships\"\npolicy = \"Ship it.\"\n\
         deliverables = [\"branch:never-pushed\", \"file:out.txt\"]\n\
         [role.plain]\npurpose = \"Writes\"\npolicy = \"Write it.\"\n\
         [agent.007]\nhandler = \"mock-verdict\"\nmodel = \"clear\"\n\
         roles = [\"shipper\", \"plain\"]\n";

    fn mission_for(state: &DaemonState, name: &str, role: &str) -> String {
        let mission_id = format!("verifying-{name}-{}", std::process::id());
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(&mission_id).unwrap(),
                Some("G".to_owned()),
                "do the work".to_owned(),
                Some("007".to_owned()),
                Some(role.to_owned()),
            )
            .unwrap();
        mission_id
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

    /// The Operation's checkout with a pushed `main` on a bare `origin`.
    fn checkout_with_remote(root: &Path) {
        let (bare, work) = (root.join("origin.git"), root.join("checkout"));
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--bare", "--initial-branch=main"]);
        git(&work, &["init", "--initial-branch=main"]);
        git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        std::fs::write(work.join("a.txt"), "one").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "one"]);
        git(&work, &["push", "origin", "main"]);
    }

    fn latest(state: &DaemonState, mission_id: &str) -> crate::SessionRecord {
        state.intake.latest_session(mission_id).unwrap().unwrap()
    }

    fn stop(state: &DaemonState) {
        state
            .stopping
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[tokio::test]
    async fn a_done_session_leaves_its_mission_verifying_and_nothing_settles_it() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        let owing = mission_for(&state, "owing", "shipper");
        let owing_nothing = mission_for(&state, "nothing", "plain");
        for mission_id in [&owing, &owing_nothing] {
            start_mission_by_id(&state, mission_id).await;
            wait_for_verifying(&state, mission_id).await;
        }

        // Passes of the Station's beat and its scheduler change nothing.
        for _ in 0..3 {
            super::super::verdicts::settle_held(&state);
            answer_probe_requests(&state);
            super::super::schedule_intake_once(&state);
            super::super::schedule_queued_once(&state).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        for mission_id in [&owing, &owing_nothing] {
            let mission = state
                .missions
                .lock()
                .unwrap()
                .get(mission_id)
                .cloned()
                .unwrap();
            assert_eq!(mission.state, MissionState::Verifying, "{mission_id}");
            let record = latest(&state, mission_id);
            assert_eq!(record.result.as_ref().unwrap().state, "verifying");
            assert!(!record.status.is_closed(), "a decision settles it");
            assert_eq!(scorecard::judge(&record), Judgment::WaitingOnPerson);
        }
        let declared = |mission_id: &str| {
            latest(&state, mission_id)
                .deliverables
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(declared(&owing), ["branch:never-pushed", "file:out.txt"]);
        assert!(declared(&owing_nothing).is_empty());
        stop(&state);
    }

    #[tokio::test]
    async fn a_probe_of_a_missing_branch_reports_it_by_name_and_keeps_the_evidence() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        let mission_id = mission_for(&state, "probe", "shipper");
        start_mission_by_id(&state, &mission_id).await;
        wait_for_verifying(&state, &mission_id).await;
        // The checkout is a repository only once the work is done: a Session in one would be
        // given a safe house, which this Station's test providers do not include.
        checkout_with_remote(root.path());
        assert!(latest(&state, &mission_id).evidence.is_empty());

        // A person asks; the Station answers on its next pass, off its beat's thread.
        let now = state.clock.now() as f64;
        state.intake.request_probe("G", &mission_id, now).unwrap();
        assert_eq!(latest(&state, &mission_id).probe_requested, Some(now));
        answer_probe_requests(&state);
        let answered = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let record = latest(&state, &mission_id);
                if record.probe_requested.is_none() && !record.evidence.is_empty() {
                    return record;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the probe was answered");
        let found: Vec<_> = answered
            .evidence
            .iter()
            .map(|found| (found.deliverable.to_string(), found.result))
            .collect();
        assert_eq!(
            found,
            [
                ("branch:never-pushed".to_owned(), ProbeResult::Missing),
                ("file:out.txt".to_owned(), ProbeResult::Missing),
            ]
        );
        assert!(
            answered.evidence[0].detail.contains("never-pushed"),
            "{}",
            answered.evidence[0].detail
        );
        assert_eq!(
            state
                .missions
                .lock()
                .unwrap()
                .get(&mission_id)
                .unwrap()
                .state,
            MissionState::Verifying,
            "a probe decides nothing"
        );

        // Once the work is there, the same probe finds it.
        let checkout = root.path().join("checkout");
        git(&checkout, &["branch", "never-pushed"]);
        git(&checkout, &["push", "origin", "never-pushed"]);
        std::fs::write(checkout.join("out.txt"), "done").unwrap();
        assert_eq!(probe_and_record(&state, &mission_id), Ok(2));
        let results: Vec<_> = latest(&state, &mission_id)
            .evidence
            .iter()
            .map(|found| found.result)
            .collect();
        assert_eq!(
            results,
            [
                ProbeResult::Missing,
                ProbeResult::Missing,
                ProbeResult::Found,
                ProbeResult::Found
            ],
            "the evidence grows, oldest first"
        );
        stop(&state);
    }

    #[tokio::test]
    async fn a_decision_settles_done_redispatches_as_the_next_attempt_or_fails() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        let (done, failed, again) = (
            mission_for(&state, "done", "plain"),
            mission_for(&state, "failed", "plain"),
            mission_for(&state, "again", "plain"),
        );
        for mission_id in [&done, &failed, &again] {
            start_mission_by_id(&state, mission_id).await;
            wait_for_verifying(&state, mission_id).await;
        }
        let now = state.clock.now() as f64;
        let give = |mission_id: &str, verdict: MissionVerdict| {
            state
                .intake
                .record_verdict("G", mission_id, &verdict)
                .unwrap();
        };
        let said =
            |word, note: &str| MissionVerdict::new(word, "person-m", now, Some(note.to_owned()));
        give(&done, said(OutcomeVerdict::Confirmed, "all there"));
        give(
            &failed,
            said(OutcomeVerdict::FailedVerification, "never pushed"),
        );
        give(
            &again,
            said(OutcomeVerdict::Redispatch, "no branch on the remote")
                .with_amendment(Some("Push the branch.".to_owned())),
        );
        super::super::verdicts::settle_held(&state);
        wait_for_state(&state, &done, MissionState::Done).await;
        wait_for_state(&state, &failed, MissionState::Failed).await;

        // The redispatched Mission is queued as the next attempt, told why.
        let queued = state.missions.lock().unwrap().get(&again).cloned().unwrap();
        assert_eq!(queued.state, MissionState::Queued);
        assert_eq!(queued.prompt, "do the work", "the brief is as filed");
        let task = queued.task();
        assert!(
            task.contains(
                "Attempt 1 of this Mission was not accepted by person-m: no branch on the remote"
            ) && task.ends_with("Push the branch."),
            "{task}"
        );
        let first = latest(&state, &again);
        assert_eq!(first.attempt, 1);
        assert!(first.status.is_closed(), "the verified attempt is over");
        assert_eq!(
            scorecard::judge(&first),
            Judgment::Disputed,
            "it counts against its Agent"
        );

        super::super::schedule_queued_once(&state).await;
        wait_for_verifying(&state, &again).await;
        let second = latest(&state, &again);
        assert_eq!(second.attempt, 2);
        assert_eq!(
            second.predecessor.as_deref(),
            Some(first.session_id.as_str())
        );
        assert!(second.verdict.is_none(), "the next attempt starts clean");
        stop(&state);
    }

    #[tokio::test]
    async fn a_mission_with_no_operation_settles_done_as_before() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        let free = format!("no-operation-{}", std::process::id());
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(&free).unwrap(),
                None,
                "do the work".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        let event = |kind: &str| AgentEvent {
            kind: kind.to_owned(),
            text: None,
            data: None,
            limit: None,
            usage: None,
            tools: Vec::new(),
            quota: Vec::new(),
        };
        assert_eq!(
            verify_done(&state, &free, event("completed")).kind,
            "completed"
        );
        let owing = mission_for(&state, "owing", "plain");
        assert_eq!(
            verify_done(&state, &owing, event("completed")).kind,
            VERIFYING_EVENT
        );
        assert_eq!(verify_done(&state, &owing, event("failed")).kind, "failed");
        stop(&state);
    }
}
