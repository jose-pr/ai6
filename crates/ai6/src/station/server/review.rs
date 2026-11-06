//! Reviews on the Mission path: where a Mission would be `done`, it waits for its panel.
//!
//! The Station closes the author's Session and blocks the Mission for `review-output`, keeping its
//! safe house. Each scheduler round [`advance`] reads the record the Registry holds, fills the
//! panel, asks whoever has not been asked, and acts on what the votes decide. Nothing is remembered
//! between rounds, so a restart carries on from the record; a seat has one vote a round, so a
//! reviewer started again is not counted twice.
//!
//! A reviewer's turn is an answer-only Session of its own: a brief AI6 queues under a `review`
//! provenance, placed like any other, started as a Chief of Staff review is. Its final message is
//! read for a vote and nothing else.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, State};

use super::{
    AgentEvent, AgentSession, ApiErrorResponse, DaemonState, FinishOutcome, MissionState,
    ResumeMission, TransitionMission, audit_cover, publish_finished_event, publish_lifecycle,
    queue_event, reopen_left_safe_house, resume_mission, stop_session_keeping_mission,
    sync_session_record, transition_mission, vetting,
};
use crate::directorates::m::mission::Mission;
use crate::directorates::m::review::{
    AppliedPolicy, BriefAuthority, BriefNaming, OUTPUT_REASON, PLAN_REASON, PanelCandidate,
    PanelTarget, REDO_REASON, ReviewGate, ReviewRefusal, Reviewer, Step, Vote, VoteAnswer,
    applicable, choice_in_data, choose_panel, decide, is_panel_reference, is_review_reason,
    models_of, next_step, no_agreement, out_of_budget, panel_brief, panel_target, read_vote,
    redo_message,
};
use crate::directorates::registry::bona_fides::Origin;
use crate::directorates::registry::reviews::{
    AskedReviewer, MissionReview, RecordedVote, ReviewMaterial, ReviewStatus, update_mission_review,
};
use crate::directorates::registry::store::{AuditRecord, Brief, Lease};
use crate::directorates::registry::timeline::TimelineEvent;

/// The timeline kind of everything a review records.
pub(super) const REVIEW_EVENT: &str = "review";

/// What a review brief's owner keeps of a brief's naming of a policy: the policy per gate and who
/// the naming speaks for. A Mission keeps it, so a later change to who may weaken does not reach
/// what was asked, and nothing but the brief's provenance decides who it speaks for.
pub(super) fn naming_of(state: &DaemonState, brief: &Brief) -> Option<BriefNaming> {
    let choice = choice_in_data(&brief.data)?;
    Some(BriefNaming::new(choice, authority_of(state, brief)))
}

/// Who a brief speaks for: a person when Universal Exports recorded one and its text did not come
/// from outside; a `00` Agent for what it proposed; anything else counts as outside text.
fn authority_of(state: &DaemonState, brief: &Brief) -> BriefAuthority {
    let Some(provenance) = brief.provenance.as_ref() else {
        return BriefAuthority::Outside;
    };
    match provenance.origin {
        Origin::Operator if !crate::directorates::m::vetting::is_untrusted(brief) => {
            BriefAuthority::Person
        }
        Origin::Proposal => {
            let zero_zero = provenance.principal.as_deref().filter(|agent| {
                super::agents::effective(state, brief.operation.as_deref(), agent)
                    .is_ok_and(|agent| agent.clearance == "00")
            });
            zero_zero.map_or(BriefAuthority::Outside, |agent| {
                BriefAuthority::ZeroZero(agent.to_owned())
            })
        }
        _ => BriefAuthority::Outside,
    }
}

/// Which policy applies to `mission` at `gate` now. The role's choice is the global roster's: a
/// checkout's own files have no say in what reviews it.
pub(super) fn applied_policy(
    state: &DaemonState,
    mission: &Mission,
    gate: ReviewGate,
) -> Result<AppliedPolicy, ReviewRefusal> {
    let role = mission
        .role_id
        .as_deref()
        .and_then(|role| state.roster.role(role).ok());
    let operation = mission
        .operation_id
        .as_deref()
        .and_then(|operation| state.roster.operation(operation).ok());
    let mut lookup =
        crate::directorates::m::review::PolicyLookup::new(gate, state.roster.reviews());
    lookup.brief = mission.review_naming.as_ref();
    lookup.role = role.map(|role| &role.review);
    lookup.operation = operation.map(|operation| &operation.review);
    lookup.author = mission.agent_id.as_deref();
    applicable(&lookup)
}

/// Whether a Mission is a reviewer's turn: AI6's own brief, by the provenance AI6 recorded.
pub(super) fn is_panel_mission(mission: &Mission) -> bool {
    mission.provenance.as_ref().is_some_and(|provenance| {
        provenance.origin == Origin::Review
            && provenance
                .reference
                .as_deref()
                .is_some_and(is_panel_reference)
    })
}

/// Stops the author at the output gate when a review applies: the Session is closed, the safe house
/// kept and the Mission blocked, and `true` is returned. `false` lets the Mission finish.
pub(super) fn holds_output(
    state: &DaemonState,
    mission_id: &str,
    session: &std::sync::Arc<std::sync::Mutex<crate::guatiao::library::Object<dyn AgentSession>>>,
    final_message: Option<&str>,
) -> bool {
    let Some(mission) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
    else {
        return false;
    };
    if vetting::is_review_mission(&mission) || mission.operation_id.is_none() {
        return false;
    }
    let gate = ReviewGate::Output;
    let applied = match applied_policy(state, &mission, gate) {
        Ok(applied) => applied,
        // Not knowing which review applies is no reason to skip one.
        Err(refusal) => {
            stop_author(state, mission_id, session, gate, &refusal.to_string());
            note_refusal(state, &mission, gate, &refusal.to_string());
            return true;
        }
    };
    let Some(name) = applied.name else {
        return false;
    };
    let material = material(state, &mission, final_message);
    open_review(state, &mission, gate, &name, material);
    stop_author(
        state,
        mission_id,
        session,
        gate,
        &format!("waiting for review policy {name:?}"),
    );
    true
}

/// Closes the author's Session and blocks the Mission for `gate`, telling whoever watches why.
fn stop_author(
    state: &DaemonState,
    mission_id: &str,
    session: &std::sync::Arc<std::sync::Mutex<crate::guatiao::library::Object<dyn AgentSession>>>,
    gate: ReviewGate,
    text: &str,
) {
    #[cfg(feature = "publisher")]
    let telemetry = state
        .sessions
        .lock()
        .ok()
        .and_then(|sessions| sessions.get(mission_id)?.telemetry.clone());
    stop_session_keeping_mission(state, mission_id, session);
    let blocked = state
        .missions
        .lock()
        .is_ok_and(|mut missions| missions.block_for(mission_id, gate.reason()).is_ok());
    if blocked {
        sync_session_record(state, mission_id);
        publish_lifecycle(state, mission_id, MissionState::Blocked);
        #[cfg(feature = "publisher")]
        if let Some(telemetry) = &telemetry {
            telemetry.publish_lifecycle_for("blocked", Some(gate.reason()));
        }
    }
    queue_event(state, mission_id, blocked_event(text));
}

fn blocked_event(text: &str) -> AgentEvent {
    AgentEvent {
        kind: "blocked".to_owned(),
        text: Some(text.to_owned()),
        data: None,
        limit: None,
        usage: None,
        tools: Vec::new(),
        quota: Vec::new(),
    }
}

/// What the panel will judge: the Mission's brief, what the author submitted and, where the Station
/// can read one, the change in its safe house. Each part is cut to its `[review]` bound.
fn material(state: &DaemonState, mission: &Mission, submitted: Option<&str>) -> ReviewMaterial {
    let limits = &state.station.limits().review;
    let mut material = ReviewMaterial {
        goals: cut(&mission.prompt, limits.goals_bytes.or_max()),
        submitted: cut(
            submitted.unwrap_or_default(),
            limits.submitted_bytes.or_max(),
        ),
        ..ReviewMaterial::default()
    };
    if let Some(report) = &mission.safe_house {
        material.branch.clone_from(&report.branch);
        material.base.clone_from(&report.base);
    }
    let house = state.sessions.lock().ok().and_then(|sessions| {
        sessions
            .get(mission.mission_id.as_str())?
            .safe_house
            .clone()
    });
    let Some(house) = house else {
        material.diff_note =
            Some("this work has no safe house the Station can read a change from".to_owned());
        return material;
    };
    material.branch = house.branch().map(str::to_owned).or(material.branch);
    material.base = house.base().map(str::to_owned).or(material.base);
    match house.changes() {
        Ok(changes) => {
            material.commits = changes
                .iter()
                .take(limits.commits.or_max())
                .map(|change| {
                    let subject = change.message.lines().next().unwrap_or_default();
                    format!(
                        "{} {subject}",
                        change.id.chars().take(12).collect::<String>()
                    )
                })
                .collect();
        }
        Err(error) => material.diff_note = Some(format!("its commits could not be read: {error}")),
    }
    match house.diff(limits.diff_bytes) {
        Ok(diff) => {
            material.diff = diff.text;
            if diff.cut {
                material.diff_note = Some(format!(
                    "the change is cut at {} bytes: [review] diff_bytes",
                    limits.diff_bytes
                ));
            }
        }
        Err(error) => {
            material.diff_note = Some(format!("the change could not be read: {error}"));
        }
    }
    material
}

/// `text` cut to `bytes` on a character boundary.
fn cut(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Puts the work up for review: a new record, or the next iteration of the one there is. The panel
/// is filled in the next round. What cannot be written is logged and retried by [`advance`], which
/// finds a blocked Mission with no record.
fn open_review(
    state: &DaemonState,
    mission: &Mission,
    gate: ReviewGate,
    name: &str,
    material: ReviewMaterial,
) {
    let mission_id = mission.mission_id.as_str();
    let Some(policy) = state.roster.reviews().get(name).cloned() else {
        return;
    };
    let now = state.clock.now() as f64;
    let operation = mission.operation_id.clone().unwrap_or_default();
    let author = mission.agent_id.clone().unwrap_or_default();
    let tries = state.station.limits().review.write_tries.or_max();
    for _ in 0..tries.max(1) {
        let existing = match state.intake.mission_review(mission_id, gate) {
            Ok(existing) => existing,
            Err(error) => {
                eprintln!("ai6: Mission {mission_id:?}: its {gate} review is unreadable: {error}");
                return;
            }
        };
        let review = match existing {
            Some(mut review) => {
                review.iteration += 1;
                review.round = 1;
                review.status = ReviewStatus::Reviewing;
                review.why = None;
                review.panel = Vec::new();
                review.overrule = None;
                name.clone_into(&mut review.policy_name);
                review.policy = policy.clone();
                review.material = material.clone();
                review
            }
            None => MissionReview::open(
                (mission_id, &operation, gate),
                (name, &policy),
                &author,
                material.clone(),
                now,
            ),
        };
        match state.intake.put_mission_review(&review) {
            Ok(Some(written)) => {
                record(
                    state,
                    &written,
                    "opened",
                    &[
                        ("policy", written.policy_name.clone()),
                        ("iteration", written.iteration.to_string()),
                    ],
                );
                return;
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("ai6: Mission {mission_id:?}: its {gate} review was not opened: {error}");
                return;
            }
        }
    }
    eprintln!("ai6: Mission {mission_id:?}: its {gate} review kept changing as it was opened");
}

/// Records on the timeline and in Audit that a brief's naming did not take, or weakened a review.
fn note_choice(state: &DaemonState, mission: &Mission, gate: ReviewGate, applied: &AppliedPolicy) {
    let mission_id = mission.mission_id.as_str();
    let operation = mission.operation_id.clone().unwrap_or_default();
    let now = state.clock.now().max(1) as f64;
    if let Some(ignored) = &applied.ignored {
        eprintln!(
            "ai6: Mission {mission_id:?}: its brief names {gate} review {:?}, ignored: {}",
            ignored.named, ignored.why
        );
        record_event(
            state,
            &operation,
            mission_id,
            "ignored",
            &[
                ("gate", gate.as_str().to_owned()),
                ("named", text(state, &ignored.named)),
                ("why", text(state, &ignored.why)),
            ],
        );
    }
    if let Some(weakens) = &applied.weakens {
        let by = match mission
            .review_naming
            .as_ref()
            .map(|naming| &naming.authority)
        {
            Some(BriefAuthority::ZeroZero(agent)) => format!("agent:{agent}"),
            _ => mission
                .provenance
                .as_ref()
                .and_then(|provenance| provenance.principal.clone())
                .unwrap_or_else(|| "person".to_owned()),
        };
        let chosen = applied.name.as_deref().unwrap_or("none");
        eprintln!(
            "ai6: Mission {mission_id:?}: {by} weakened its {gate} review from {weakens:?} to \
             {chosen:?}"
        );
        audit(
            state,
            &operation,
            "mission.review.weakened",
            &by,
            &format!("{mission_id}:{gate}:{weakens}:{chosen}"),
            now,
        );
        record_event(
            state,
            &operation,
            mission_id,
            "weakened",
            &[
                ("gate", gate.as_str().to_owned()),
                ("from", text(state, weakens)),
                ("to", text(state, chosen)),
                ("by", text(state, &by)),
            ],
        );
    }
}

/// Keeps why no policy could be read where a person sees it. The Mission stays blocked until the
/// configuration is put right and the next round reads it again.
fn note_refusal(state: &DaemonState, mission: &Mission, gate: ReviewGate, why: &str) {
    let mission_id = mission.mission_id.as_str();
    let operation = mission.operation_id.clone().unwrap_or_default();
    eprintln!("ai6: Mission {mission_id:?}: its {gate} review cannot start: {why}");
    record_event(
        state,
        &operation,
        mission_id,
        "waiting",
        &[
            ("gate", gate.as_str().to_owned()),
            ("why", text(state, why)),
        ],
    );
}

/// What a round does for a Mission once it has read the record.
enum After {
    /// The review passed.
    Passed(ReviewGate),
    /// Resume the author with this message.
    SendBack(ReviewGate, String),
    /// End the Mission as failed, for this reason.
    Refused(String),
}

/// The Missions on this Station waiting on a review: the Mission, and the gate its reason names.
/// A redo waiting for a person may be of either gate; the record says which.
fn waiting(state: &DaemonState) -> Vec<(String, Option<ReviewGate>)> {
    state
        .missions
        .lock()
        .map(|missions| missions.list())
        .unwrap_or_default()
        .into_iter()
        .filter(|mission| mission.state == MissionState::Blocked)
        .filter_map(|mission| {
            let reason = mission.blocked_reason.as_deref()?;
            if !is_review_reason(reason) {
                return None;
            }
            let gate = match reason {
                PLAN_REASON => Some(ReviewGate::Plan),
                OUTPUT_REASON => Some(ReviewGate::Output),
                REDO_REASON => None,
                _ => return None,
            };
            Some((mission.mission_id.to_string(), gate))
        })
        .collect()
}

/// One round of every review this Station's Missions wait on, and of its reviewers' turns.
pub(super) async fn advance(state: &DaemonState) {
    let sweeping = state.clone();
    let stale = tokio::task::spawn_blocking(move || sweep_reviewers(&sweeping))
        .await
        .unwrap_or_default();
    for mission_id in stale {
        abandon(state, &mission_id).await;
    }
    for (mission_id, gate) in waiting(state) {
        let (worker, id) = (state.clone(), mission_id.clone());
        let after = tokio::task::spawn_blocking(move || round(&worker, &id, gate))
            .await
            .ok()
            .flatten();
        match after {
            Some(After::Passed(ReviewGate::Output)) => finish_passed(state, &mission_id).await,
            Some(After::Passed(_)) => {
                go_on_after_plan(state, &mission_id).await;
            }
            Some(After::SendBack(gate, message)) => {
                send_back(state, &mission_id, gate, message).await;
            }
            Some(After::Refused(reason)) => refuse(state, &mission_id, &reason).await,
            None => {}
        }
    }
}

/// Reads the record of a blocked Mission and moves it on as far as it can without waiting. Every
/// change is written before it is acted on, so a Station that dies between the two carries on.
fn round(state: &DaemonState, mission_id: &str, gate: Option<ReviewGate>) -> Option<After> {
    let mission = state
        .missions
        .lock()
        .ok()?
        .get(mission_id)
        .cloned()
        .filter(|mission| mission.state == MissionState::Blocked)?;
    let mut review = match gate {
        Some(gate) => match state.intake.mission_review(mission_id, gate) {
            Ok(review) => review,
            Err(error) => {
                eprintln!("ai6: Mission {mission_id:?}: its {gate} review is unreadable: {error}");
                return None;
            }
        },
        None => [ReviewGate::Output, ReviewGate::Plan]
            .into_iter()
            .find_map(|gate| state.intake.mission_review(mission_id, gate).ok().flatten()),
    };
    let Some(mut review) = review.take() else {
        let gate = gate?;
        reopen(state, &mission, gate);
        return None;
    };
    let gate = review.gate;
    match review.status {
        ReviewStatus::Passed => return Some(After::Passed(gate)),
        ReviewStatus::Refused => {
            return Some(After::Refused(
                review
                    .why
                    .clone()
                    .unwrap_or_else(|| "the review refused it".to_owned()),
            ));
        }
        // Redoing while blocked: the Station stopped between the record and the resume.
        ReviewStatus::SendingBack | ReviewStatus::Redoing => {
            return Some(After::SendBack(gate, sent_back_message(state, &review)));
        }
        ReviewStatus::Person => return None,
        ReviewStatus::Reviewing | ReviewStatus::Waiting => {}
    }
    let now = state.clock.now() as f64;
    let mut changed = false;
    if review.panel.is_empty() {
        match fill_panel(state, &mission, &review) {
            Ok(panel) => {
                review.panel = panel;
                review.status = ReviewStatus::Reviewing;
                review.why = None;
                changed = true;
            }
            Err(why) => {
                if review.why.as_deref() != Some(why.as_str())
                    || review.status != ReviewStatus::Waiting
                {
                    review.status = ReviewStatus::Waiting;
                    review.why = Some(why.clone());
                    if put(state, &review).is_some() {
                        eprintln!(
                            "ai6: Mission {mission_id:?}: its {gate} review waits for a panel: \
                             {why}"
                        );
                        record(state, &review, "waiting", &[("why", text(state, &why))]);
                    }
                }
                return None;
            }
        }
    }
    let timeout = review.policy.reviewer_timeout_secs;
    let mut asking = Vec::new();
    let mut out_of_turns = false;
    let tokens = review.tokens;
    let panel = review.panel.clone();
    for reviewer in &panel {
        if review.vote_of(reviewer.seat).is_some() {
            continue;
        }
        match review.asked_of(reviewer.seat).cloned() {
            Some(asked) => {
                if timeout > 0 && asked.at + timeout as f64 <= now {
                    let vote = absent_vote(
                        &review,
                        reviewer,
                        format!("it gave no answer within {timeout} seconds"),
                        now,
                    );
                    review.votes.push(vote);
                    changed = true;
                }
            }
            None if out_of_budget(&review.policy, review.turns(), tokens) => out_of_turns = true,
            None => {
                let target = PanelTarget::new(
                    &review.mission_id,
                    gate,
                    review.iteration,
                    review.round,
                    reviewer.seat,
                );
                review.asked.push(AskedReviewer {
                    iteration: review.iteration,
                    round: review.round,
                    seat: reviewer.seat,
                    review_id: target.review_id(),
                    at: now,
                });
                asking.push(reviewer.clone());
                changed = true;
            }
        }
    }
    let pending = panel.iter().any(|reviewer| {
        review.vote_of(reviewer.seat).is_none() && review.asked_of(reviewer.seat).is_some()
    });
    let ballots = review.ballots();
    let decision = decide(&review.policy, panel.len(), &ballots, review.round);
    let step = if out_of_turns && pending {
        Step::Wait
    } else if out_of_turns {
        no_agreement(&review.policy, gate)
    } else {
        next_step(&review.policy, gate, decision, review.sent_back)
    };
    let mut after = None;
    let mut then = Vec::new();
    match step {
        Step::Pass => {
            review.status = ReviewStatus::Passed;
            after = Some(After::Passed(gate));
            then.push(("passed", String::new()));
            changed = true;
        }
        Step::SendBack => {
            review.status = ReviewStatus::SendingBack;
            review.sent_back += 1;
            after = Some(After::SendBack(gate, sent_back_message(state, &review)));
            then.push(("sent_back", review.sent_back.to_string()));
            changed = true;
        }
        Step::AskAgain => {
            review.round += 1;
            then.push(("next_round", review.round.to_string()));
            changed = true;
        }
        Step::Person(reason) => {
            let why = if out_of_turns {
                "the review ran out of its budget".to_owned()
            } else if reason == REDO_REASON {
                format!("sent back {} times, its limit", review.sent_back)
            } else {
                "the panel did not agree".to_owned()
            };
            review.status = ReviewStatus::Person;
            review.why = Some(why.clone());
            then.push(("person", why));
            changed = true;
            if reason != gate.reason() {
                reblock(state, mission_id, reason);
            }
        }
        Step::Refuse => {
            let why = if out_of_turns {
                "the review ran out of its budget".to_owned()
            } else {
                "the panel did not agree".to_owned()
            };
            review.status = ReviewStatus::Refused;
            review.why = Some(why.clone());
            after = Some(After::Refused(why.clone()));
            then.push(("refused", why));
            changed = true;
        }
        Step::Wait => {}
    }
    if changed {
        let Some(written) = put(state, &review) else {
            // Another Station wrote a vote first: the next round reads it and decides again.
            return None;
        };
        review = written;
    }
    for reviewer in asking {
        ask(state, &review, &reviewer);
    }
    // A brief lost between the record and the queue is entered again.
    for reviewer in &panel {
        if review.vote_of(reviewer.seat).is_none() && review.asked_of(reviewer.seat).is_some() {
            ensure_entered(state, &review, reviewer);
        }
    }
    for (action, detail) in then {
        let details = [("detail", text(state, &detail))];
        record(
            state,
            &review,
            action,
            if detail.is_empty() { &[] } else { &details },
        );
        audit(
            state,
            &review.operation,
            &format!("mission.review.{action}"),
            &format!("station:{}", state.station_id),
            &format!("{mission_id}:{gate}:{}:{}", review.iteration, review.round),
            now.max(1.0),
        );
    }
    after
}

fn absent_vote(
    review: &MissionReview,
    reviewer: &Reviewer,
    reason: String,
    now: f64,
) -> RecordedVote {
    RecordedVote {
        iteration: review.iteration,
        round: review.round,
        seat: reviewer.seat,
        agent: reviewer.agent.clone(),
        model: reviewer.model.clone(),
        weight: reviewer.weight,
        vote: Vote::Absent,
        reasons: vec![reason],
        at: now,
    }
}

/// Moves a blocked Mission to another review reason.
fn reblock(state: &DaemonState, mission_id: &str, reason: &str) {
    let blocked = state
        .missions
        .lock()
        .is_ok_and(|mut missions| missions.block_for(mission_id, reason).is_ok());
    if blocked {
        sync_session_record(state, mission_id);
        publish_lifecycle(state, mission_id, MissionState::Blocked);
    }
}

/// A blocked Mission with no record: its record could not be written when it stopped. Its final
/// message is gone with its Session, so the panel is told; it reads the goals and the change.
fn reopen(state: &DaemonState, mission: &Mission, gate: ReviewGate) {
    let Ok(applied) = applied_policy(state, mission, gate) else {
        return;
    };
    let Some(name) = applied.name else {
        return;
    };
    let mut material = material(state, mission, None);
    material.submitted = "The Agent's message was lost before it could be kept.".to_owned();
    open_review(state, mission, gate, &name, material);
}

/// Fills the panel of `review`, or says why it cannot.
fn fill_panel(
    state: &DaemonState,
    mission: &Mission,
    review: &MissionReview,
) -> Result<Vec<Reviewer>, String> {
    let (candidates, author) = candidates(state, mission, review)?;
    choose_panel(&review.policy, &author, &candidates).map_err(|refusal| refusal.to_string())
}

/// The Agents who could review, each with why it cannot when it cannot, and the author as a panel
/// sees it: every model its routes name, and the one it runs on now. Only the global roster counts:
/// an Agent an Operation's own file changes cannot review ([`vetting::check_reviewer`]).
fn candidates(
    state: &DaemonState,
    mission: &Mission,
    review: &MissionReview,
) -> Result<(Vec<PanelCandidate>, PanelCandidate), String> {
    let operation = state
        .roster
        .operation(&review.operation)
        .map_err(|error| error.to_string())?;
    let quarantined = super::medical::quarantined_agents(state, Some(&review.operation));
    let held = super::stewardship::held_agents(state, Some(&review.operation));
    let handlers = reported_handlers(state);
    let ids = state
        .roster
        .agent_ids()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut candidates = Vec::with_capacity(ids.len());
    for id in ids {
        let Ok(agent) = state.roster.agent(None, &id) else {
            continue;
        };
        let wanted = if agent.routes.is_empty() {
            agent.handler.iter().cloned().collect::<Vec<_>>()
        } else {
            agent
                .routes
                .iter()
                .map(|route| route.handler.clone())
                .collect()
        };
        let unavailable = if !operation.roster.is_empty() && !operation.roster.contains(&id) {
            Some("is not on the Operation's roster".to_owned())
        } else if quarantined.contains(&id) {
            Some("is quarantined".to_owned())
        } else if held.contains(&id) {
            Some("is held".to_owned())
        } else if let Some(retired) = super::agents::retired(state, &agent) {
            Some(retired)
        } else if !wanted.iter().any(|handler| handlers.contains(handler)) {
            Some("no Station reports its handler".to_owned())
        } else {
            state
                .roster
                .agent(Some(&review.operation), &id)
                .map_err(|error| error.to_string())
                .and_then(|effective| vetting::check_reviewer(state, &effective))
                .err()
        };
        candidates.push(PanelCandidate::of(&agent, unavailable));
    }
    let author_agent = state
        .roster
        .agent(Some(&review.operation), &review.author)
        .map_err(|error| error.to_string())?;
    let mut author = PanelCandidate::of(&author_agent, None);
    author.models.extend(models_of(&author_agent));
    if let Some(model) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.operation(mission.mission_id.as_str()))
        .and_then(|manifest| manifest.model)
    {
        author.models.insert(model);
    }
    Ok((candidates, author))
}

/// The handlers the Stations that have checked in lately report; this Station's own when the
/// Registry cannot say.
fn reported_handlers(state: &DaemonState) -> Vec<String> {
    let mut handlers = super::available_handlers(state);
    let window = state.station.limits().review.station_window.0;
    if let Ok(stations) = state.intake.stations(window) {
        for station in stations {
            if let Some(names) = station
                .info()
                .get("handlers")
                .and_then(serde_json::Value::as_array)
            {
                handlers.extend(
                    names
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned),
                );
            }
        }
    }
    handlers
}

/// Writes `review` if it is still at the revision it was read at. `None` when it is not, or the
/// Registry failed, which is logged.
fn put(state: &DaemonState, review: &MissionReview) -> Option<MissionReview> {
    match state.intake.put_mission_review(review) {
        Ok(written) => written,
        Err(error) => {
            eprintln!(
                "ai6: Mission {:?}: its {} review was not written: {error}",
                review.mission_id, review.gate
            );
            None
        }
    }
}

/// Queues one reviewer's turn.
fn ask(state: &DaemonState, review: &MissionReview, reviewer: &Reviewer) {
    ensure_entered(state, review, reviewer);
    record(
        state,
        review,
        "asked",
        &[
            ("agent", text(state, &reviewer.agent)),
            ("model", text(state, &reviewer.model)),
            ("seat", reviewer.seat.to_string()),
            ("round", review.round.to_string()),
        ],
    );
}

/// Enters the reviewer's brief unless the Registry already holds it, in any state.
fn ensure_entered(state: &DaemonState, review: &MissionReview, reviewer: &Reviewer) {
    let target = PanelTarget::new(
        &review.mission_id,
        review.gate,
        review.iteration,
        review.round,
        reviewer.seat,
    );
    let entry = target.entry(&review.operation);
    match state.intake.entered_mission_id(&entry) {
        Ok(Some(_)) => return,
        Ok(None) => {}
        Err(error) => {
            eprintln!(
                "ai6: review {:?} is unreadable: {error}",
                target.review_id()
            );
            return;
        }
    }
    let brief = panel_brief(
        review,
        reviewer,
        &target,
        &state.station.limits().review,
        (&state.station_id, state.clock.now()),
    );
    match state.intake.enter_brief(&brief, &entry) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            eprintln!(
                "ai6: review {:?} was not queued: {error}",
                target.review_id()
            );
        }
    }
}

/// Reads a reviewer's final message as its vote. Only a genuine turn counts: AI6's own brief, whose
/// target's panel seats this Agent, on a record still in that round.
pub(super) fn reviewer_answered(state: &DaemonState, mission_id: &str, text: Option<&str>) {
    let Some(mission) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
    else {
        return;
    };
    if !is_panel_mission(&mission) {
        return;
    }
    let Some(target) = genuine_target(state, mission_id) else {
        return;
    };
    let limits = &state.station.limits().review;
    let answer = read_vote(
        text.unwrap_or_default(),
        limits.answer_tail_bytes,
        limits.reasons,
        limits.reason_bytes,
    );
    record_vote(state, &target, &mission, answer);
}

/// The target of the reviewer's brief a lease or Mission holds, when the brief is AI6's own.
fn genuine_target(state: &DaemonState, key: &str) -> Option<PanelTarget> {
    let brief = state.intake.read_claimed(key).ok()??;
    panel_target(&brief).filter(|target| brief.id == target.review_id())
}

/// Records a seat's vote on the Registry's record, once.
fn record_vote(state: &DaemonState, target: &PanelTarget, mission: &Mission, answer: VoteAnswer) {
    let agent = mission.agent_id.clone().unwrap_or_default();
    let now = state.clock.now() as f64;
    let tries = state.station.limits().review.write_tries.or_max();
    let answer_vote = answer.vote;
    let reasons = answer.reasons;
    let written = update_mission_review(
        state.intake.as_ref(),
        (&target.mission, target.gate),
        tries,
        |review| {
            let Some(reviewer) = review
                .panel
                .iter()
                .find(|reviewer| reviewer.seat == target.seat && reviewer.agent == agent)
            else {
                return false;
            };
            let vote = RecordedVote {
                iteration: target.iteration,
                round: target.round,
                seat: target.seat,
                agent: reviewer.agent.clone(),
                model: reviewer.model.clone(),
                weight: reviewer.weight,
                vote: answer_vote,
                reasons: reasons.clone(),
                at: now,
            };
            review.record_vote((target.iteration, target.round), vote)
        },
    );
    match written {
        Ok(Some(review)) => {
            let reviewer = review
                .panel
                .iter()
                .find(|reviewer| reviewer.seat == target.seat);
            let (model, weight) = reviewer.map_or((String::new(), 0), |reviewer| {
                (reviewer.model.clone(), reviewer.weight)
            });
            let joined = reasons.join("; ");
            record(
                state,
                &review,
                "vote",
                &[
                    ("agent", self::text(state, &agent)),
                    ("model", self::text(state, &model)),
                    ("weight", weight.to_string()),
                    ("round", target.round.to_string()),
                    ("iteration", target.iteration.to_string()),
                    ("vote", answer_vote.as_str().to_owned()),
                    ("reasons", self::text(state, &joined)),
                ],
            );
            audit(
                state,
                &review.operation,
                "mission.review.vote",
                &format!("agent:{agent}"),
                &format!(
                    "{}:{}:{}:{}:{}:{}",
                    target.mission,
                    target.gate,
                    target.iteration,
                    target.round,
                    target.seat,
                    answer_vote.as_str()
                ),
                now.max(1.0),
            );
        }
        Ok(None) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "ai6: Mission {:?}: a vote on its {} review was not recorded: {error}",
            target.mission, target.gate
        ),
    }
}

/// Ends a reviewer's turn: what it spent counts on the review, and a turn that ended with no vote
/// counts as absent.
pub(super) fn reviewer_ended(state: &DaemonState, lease: &Lease, mission: &Mission) {
    if !is_panel_mission(mission) {
        return;
    }
    let Some(target) = genuine_target(state, lease.key()) else {
        return;
    };
    let spent = state
        .intake
        .mission_usage(mission.operation_id.as_deref(), mission.mission_id.as_str())
        .ok()
        .flatten()
        .map_or(0, |usage| usage.total.input + usage.total.output);
    if spent > 0 {
        let tries = state.station.limits().review.write_tries.or_max();
        if let Err(error) = update_mission_review(
            state.intake.as_ref(),
            (&target.mission, target.gate),
            tries,
            |review| {
                review.tokens = review.tokens.saturating_add(spent);
                true
            },
        ) && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "ai6: Mission {:?}: what a reviewer spent was not counted: {error}",
                target.mission
            );
        }
    }
    let state_name = mission.state.as_str();
    record_vote(
        state,
        &target,
        mission,
        VoteAnswer {
            vote: Vote::Absent,
            reasons: vec![format!("its turn ended {state_name} without an answer")],
        },
    );
}

/// The reviewers' turns on this Station nothing is waiting for any more, or that cannot answer: a
/// turn stalled, or whose seat voted or review moved on. Each is returned to be abandoned. A turn
/// lost in a restart before its harness named a Session is queued again.
fn sweep_reviewers(state: &DaemonState) -> Vec<String> {
    let turns = state
        .missions
        .lock()
        .map(|missions| missions.list())
        .unwrap_or_default()
        .into_iter()
        .filter(|mission| is_panel_mission(mission) && !super::is_terminal_state(mission.state))
        .collect::<Vec<_>>();
    let mut stale = Vec::new();
    for mission in turns {
        let mission_id = mission.mission_id.as_str();
        let Some(target) = genuine_target(state, mission_id) else {
            continue;
        };
        let Ok(review) = state.intake.mission_review(&target.mission, target.gate) else {
            continue;
        };
        let Some(review) = review else {
            continue;
        };
        let moved_on = (review.iteration, review.round) != (target.iteration, target.round)
            || review.status != ReviewStatus::Reviewing;
        let voted = review.vote_of(target.seat).is_some();
        if moved_on || voted {
            if mission.state != MissionState::Working || moved_on {
                stale.push(mission_id.to_owned());
            }
            continue;
        }
        match mission.state {
            MissionState::Stalled | MissionState::Quarantined => {
                let why = format!("its turn is {}", mission.state.as_str());
                record_vote(
                    state,
                    &target,
                    &mission,
                    VoteAnswer {
                        vote: Vote::Absent,
                        reasons: vec![why],
                    },
                );
                stale.push(mission_id.to_owned());
            }
            MissionState::Blocked if mission.session_ids.is_empty() => {
                restart_turn(state, &mission);
            }
            _ => {}
        }
    }
    stale
}

/// Queues again a reviewer's turn whose harness never named a Session, so there is nothing to
/// resume: the seat has no vote yet, so this counts nothing twice.
fn restart_turn(state: &DaemonState, mission: &Mission) {
    let mission_id = mission.mission_id.as_str();
    // A blocked Mission is not queued again directly; one that stalled is.
    let requeued = state.missions.lock().ok().is_some_and(|mut missions| {
        missions
            .transition(mission_id, MissionState::Stalled)
            .and_then(|_| missions.retry_past_brief(mission_id, mission.agent_id.clone()))
            .is_ok()
    });
    if requeued {
        eprintln!("ai6: Mission {mission_id:?}: a reviewer's lost turn is started again");
        if let Ok(mut pending) = state.startup_recovery.lock() {
            pending.remove(mission_id);
        }
    }
}

async fn abandon(state: &DaemonState, mission_id: &str) {
    let result = transition_mission(
        State(state.clone()),
        Path(mission_id.to_owned()),
        Json(TransitionMission {
            state: MissionState::Abandoned,
        }),
    )
    .await;
    if let Err(ApiErrorResponse(_, Json(error))) = result {
        eprintln!(
            "ai6: Mission {mission_id:?}: a reviewer's turn was not ended: {}",
            error.error
        );
    }
}

/// What the author is told when sent back: the panel's reasons, or a person's.
fn sent_back_message(state: &DaemonState, review: &MissionReview) -> String {
    let reasons = match &review.overrule {
        Some(overrule) if overrule.decision == "redo" => vec![overrule.reason.clone()],
        _ => review.redo_reasons(),
    };
    let reasons = reasons
        .into_iter()
        .take(state.station.limits().review.reasons.or_max())
        .collect::<Vec<_>>();
    redo_message(
        review.gate,
        &reasons,
        review.sent_back,
        review.policy.redo_limit,
    )
}

/// Resumes the author's Session with the reasons. Tried again next round when it cannot start; the
/// record says so until it does.
async fn send_back(state: &DaemonState, mission_id: &str, gate: ReviewGate, message: String) {
    let waiting = state
        .retry_after
        .lock()
        .ok()
        .and_then(|retries| retries.get(mission_id).copied())
        .is_some_and(|retry_at| retry_at > tokio::time::Instant::now());
    if waiting {
        return;
    }
    // Recorded first: the author's next output can come before this round would write it.
    let tries = state.station.limits().review.write_tries.or_max();
    let (worker, id) = (state.clone(), mission_id.to_owned());
    let marked = tokio::task::spawn_blocking(move || {
        update_mission_review(worker.intake.as_ref(), (&id, gate), tries, |review| {
            let sending = matches!(
                review.status,
                ReviewStatus::SendingBack | ReviewStatus::Redoing
            );
            review.status = ReviewStatus::Redoing;
            sending
        })
    })
    .await;
    if !matches!(marked, Ok(Ok(_))) {
        return;
    }
    let resumed = resume_mission(
        State(state.clone()),
        Path(mission_id.to_owned()),
        Json(ResumeMission {
            message: Some(message),
            environment: BTreeMap::new(),
        }),
    )
    .await;
    match resumed {
        Ok(_) => {
            if let Ok(mut retries) = state.retry_after.lock() {
                retries.remove(mission_id);
            }
        }
        Err(ApiErrorResponse(_, Json(error))) => {
            eprintln!(
                "ai6: Mission {mission_id:?} was not sent back yet: {}",
                error.error
            );
            if let Ok(mut retries) = state.retry_after.lock() {
                retries.insert(
                    mission_id.to_owned(),
                    tokio::time::Instant::now() + state.station.limits().scheduler.retry_delay.0,
                );
            }
        }
    }
}

/// Finishes a Mission whose output passed: its cover is audited on what is there now, its safe house
/// finished, and only then is it `done`, which is what moves its issues and reports.
async fn finish_passed(state: &DaemonState, mission_id: &str) {
    let Some((mission, manifest)) = state.missions.lock().ok().and_then(|missions| {
        Some((
            missions.get(mission_id)?.clone(),
            missions.operation(mission_id),
        ))
    }) else {
        return;
    };
    let house = match reopen_left_safe_house(state, &mission, manifest.as_ref()).await {
        Ok(house) => house,
        Err(ApiErrorResponse(_, Json(error))) => {
            eprintln!(
                "ai6: Mission {mission_id:?}: its safe house could not be opened after its \
                 review: {}",
                error.error
            );
            None
        }
    };
    let (worker, id) = (state.clone(), mission_id.to_owned());
    let audited = tokio::task::spawn_blocking(move || {
        let mut failure = None;
        let mut cover_blown = false;
        let mut fail_on_blow = false;
        let model = manifest
            .as_ref()
            .and_then(|manifest| manifest.model.clone());
        if let Some(audit) = manifest
            .as_ref()
            .and_then(|manifest| manifest.cover.clone())
        {
            fail_on_blow = audit.fail_on_blow;
            match audit_cover(&worker, &id, &audit, house.as_ref(), model.as_deref()) {
                Ok(blown) => cover_blown = blown,
                Err(error) => failure = Some(format!("cover audit failed: {error}")),
            }
        }
        if let Some(house) = house.filter(crate::station::safehouse::SafeHouse::is_isolated) {
            match house.finish() {
                Ok(report) => {
                    if let Ok(mut missions) = worker.missions.lock()
                        && let Err(error) = missions.set_safe_house(&id, report)
                    {
                        failure.get_or_insert_with(|| {
                            format!("safe-house report could not be persisted: {error:?}")
                        });
                    }
                }
                Err(error) => {
                    failure
                        .get_or_insert_with(|| format!("safe-house finalization failed: {error}"));
                }
            }
        }
        FinishOutcome {
            cover_blown,
            fail_on_blow,
            failure,
            #[cfg(feature = "publisher")]
            telemetry: published(&worker, &id),
        }
    })
    .await;
    let Ok(finished) = audited else {
        return;
    };
    let event = AgentEvent {
        kind: "completed".to_owned(),
        text: Some("its review passed".to_owned()),
        data: None,
        limit: None,
        usage: None,
        tools: Vec::new(),
        quota: Vec::new(),
    };
    publish_finished_event(state, mission_id, event, Some(finished));
}

/// The telemetry of a Mission with no live Session, so its end reaches Universal Exports.
#[cfg(feature = "publisher")]
pub(super) fn published(
    state: &DaemonState,
    mission_id: &str,
) -> Option<std::sync::Arc<crate::station::publisher::SessionTelemetry>> {
    let (mission, manifest) = state.missions.lock().ok().and_then(|missions| {
        Some((
            missions.get(mission_id)?.clone(),
            missions.operation(mission_id)?,
        ))
    })?;
    let operation = mission.operation_id.clone()?;
    let telemetry = state
        .publisher
        .as_ref()?
        .session(operation, mission.mission_id.clone());
    telemetry.set_mission_publication(crate::station::publisher::MissionPublication {
        name: mission.name.clone(),
        prompt: mission.prompt.clone(),
        agent_id: mission.agent_id.clone(),
        role_id: mission.role_id.clone(),
        handler_provider: manifest.handler_provider.clone(),
        terminal_provider: manifest.terminal_provider.clone(),
        working_directory: manifest.working_directory.clone(),
        model: manifest.model.clone(),
        effort: manifest.effort.clone(),
        environment_keys: Vec::new(),
        station_id: Some(state.station_id.clone()),
        theater_id: manifest.theater_id.clone(),
        mode: manifest.mode,
        provenance: mission.provenance.clone(),
    });
    if let Some(session) = mission.session_ids.last() {
        telemetry.bind(session.clone());
    }
    Some(telemetry)
}

/// Ends a Mission a review refused.
async fn refuse(state: &DaemonState, mission_id: &str, reason: &str) {
    eprintln!("ai6: Mission {mission_id:?} failed: its review refused it: {reason}");
    let result = transition_mission(
        State(state.clone()),
        Path(mission_id.to_owned()),
        Json(TransitionMission {
            state: MissionState::Failed,
        }),
    )
    .await;
    if let Err(ApiErrorResponse(_, Json(error))) = result {
        eprintln!("ai6: Mission {mission_id:?} was not ended: {}", error.error);
        return;
    }
    #[cfg(feature = "publisher")]
    if let Some(telemetry) = published(state, mission_id) {
        telemetry.publish_lifecycle_for("failed", Some("review-refused"));
    }
    queue_event(
        state,
        mission_id,
        AgentEvent {
            kind: "failed".to_owned(),
            text: Some(format!("its review refused it: {reason}")),
            data: None,
            limit: None,
            usage: None,
            tools: Vec::new(),
            quota: Vec::new(),
        },
    );
}

/// Told to an Agent whose plan is reviewed before it works, with the tools it has meanwhile.
pub(super) const PLAN_CONTEXT: &str = "This Mission's plan is reviewed before any work starts. \
    For now you can read and search, and change nothing. Work out how you would do the task, then \
    call the AI6 tool submit_plan with the whole plan, the steps and how you will test them, and \
    end your turn. If the tool is not there, make the plan your final message. A review may send \
    it back with reasons; once it passes you are resumed with the tools to carry it out.";

/// Told to an Agent when its plan passed.
const PLAN_PASSED: &str = "Your plan passed its review. You now have this Mission's tools: carry \
    the plan out.";

/// Whether `mission` is held to reading until its plan passes: a plan review applies and has not
/// passed. When the Registry or the policy cannot say, it is held.
pub(super) fn in_plan_stage(state: &DaemonState, mission: &Mission) -> bool {
    if vetting::is_review_mission(mission) || mission.operation_id.is_none() {
        return false;
    }
    match applied_policy(state, mission, ReviewGate::Plan) {
        Ok(applied) if applied.name.is_none() => false,
        _ => !matches!(
            state
                .intake
                .mission_review(mission.mission_id.as_str(), ReviewGate::Plan),
            Ok(Some(review)) if review.status == ReviewStatus::Passed
        ),
    }
}

/// Stops the author at the plan gate, as [`holds_output`] does at the output gate: its plan is what
/// it submitted with `submit_plan`, else its final message.
pub(super) fn holds_plan(
    state: &DaemonState,
    mission_id: &str,
    session: &std::sync::Arc<std::sync::Mutex<crate::guatiao::library::Object<dyn AgentSession>>>,
    final_message: Option<&str>,
) -> bool {
    let Some(mission) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
    else {
        return false;
    };
    if !in_plan_stage(state, &mission) {
        return false;
    }
    let gate = ReviewGate::Plan;
    let name = match applied_policy(state, &mission, gate) {
        Ok(applied) => applied.name,
        Err(refusal) => {
            stop_author(state, mission_id, session, gate, &refusal.to_string());
            note_refusal(state, &mission, gate, &refusal.to_string());
            return true;
        }
    };
    let Some(name) = name else {
        return false;
    };
    let limits = &state.station.limits().review;
    let plan = mission
        .plan
        .clone()
        .or_else(|| final_message.map(str::to_owned))
        .unwrap_or_default();
    let material = ReviewMaterial {
        goals: cut(&mission.prompt, limits.goals_bytes.or_max()),
        submitted: cut(&plan, limits.submitted_bytes.or_max()),
        ..ReviewMaterial::default()
    };
    open_review(state, &mission, gate, &name, material);
    if let Ok(mut missions) = state.missions.lock() {
        let _ = missions.set_plan(mission_id, None);
    }
    stop_author(
        state,
        mission_id,
        session,
        gate,
        &format!("its plan waits for review policy {name:?}"),
    );
    true
}

/// Why a Mission held to reading cannot start on `handler`, when it cannot.
pub(super) fn plan_stage_refusal(
    handler: &dyn crate::directorates::section_00::agent::AgentHandler,
    remote_hands: bool,
) -> Option<String> {
    let refused = match handler.read_only_refusal() {
        Ok(refused) => refused,
        Err(error) => Some(format!("its handler could not say whether it can: {error}")),
    };
    refused
        .or_else(|| {
            remote_hands.then(|| {
                "in remote-hands mode the Theater's tools can change it, whatever the handler offers"
                    .to_owned()
            })
        })
        .map(|why| format!("its plan is reviewed before it may change anything, and {why}"))
}

/// The `submit_plan` tool of a Mission held to reading: it keeps the plan for its review.
#[derive(Clone)]
#[cfg_attr(not(feature = "cables-mcp"), allow(dead_code))]
pub(super) struct PlanDesk {
    state: DaemonState,
    mission_id: String,
}

#[cfg_attr(not(feature = "cables-mcp"), allow(dead_code))]
impl PlanDesk {
    pub(super) fn new(state: DaemonState, mission_id: String) -> Self {
        Self { state, mission_id }
    }

    /// Keeps `plan`, cut to `[review] submitted_bytes`, and says what happens next.
    pub(super) fn submit(&self, plan: &str) -> String {
        let bytes = self.state.station.limits().review.submitted_bytes.or_max();
        let kept = self.state.missions.lock().ok().is_some_and(|mut missions| {
            missions
                .set_plan(&self.mission_id, Some(cut(plan, bytes)))
                .is_ok()
        });
        if kept {
            "Your plan is kept for its review. End your turn now: you are resumed with the outcome."
                .to_owned()
        } else {
            "Your plan could not be kept; give it as your final message instead.".to_owned()
        }
    }
}

/// Goes on after a plan passed: the author is resumed at its Mission's clearance with the
/// Mission's tools. Tried again next round when it cannot start.
async fn go_on_after_plan(state: &DaemonState, mission_id: &str) {
    let waiting = state
        .retry_after
        .lock()
        .ok()
        .and_then(|retries| retries.get(mission_id).copied())
        .is_some_and(|retry_at| retry_at > tokio::time::Instant::now());
    if waiting {
        return;
    }
    let resumed = resume_mission(
        State(state.clone()),
        Path(mission_id.to_owned()),
        Json(ResumeMission {
            message: Some(PLAN_PASSED.to_owned()),
            environment: BTreeMap::new(),
        }),
    )
    .await;
    if let Err(ApiErrorResponse(_, Json(error))) = resumed {
        eprintln!(
            "ai6: Mission {mission_id:?} did not go on after its plan passed: {}",
            error.error
        );
        if let Ok(mut retries) = state.retry_after.lock() {
            retries.insert(
                mission_id.to_owned(),
                tokio::time::Instant::now() + state.station.limits().scheduler.retry_delay.0,
            );
        }
    }
}

/// Records, once when a Mission is created, a brief's naming that did not take or that weakened a
/// review, for each gate.
pub(super) fn note_naming(state: &DaemonState, mission_id: &str) {
    let Some(mission) = state
        .missions
        .lock()
        .ok()
        .and_then(|missions| missions.get(mission_id).cloned())
    else {
        return;
    };
    if mission.review_naming.is_none() {
        return;
    }
    for gate in [ReviewGate::Plan, ReviewGate::Output] {
        if let Ok(applied) = applied_policy(state, &mission, gate) {
            note_choice(state, &mission, gate, &applied);
        }
    }
}

/// Writes one timeline event of a review.
fn record(state: &DaemonState, review: &MissionReview, action: &str, details: &[(&str, String)]) {
    let mut all = vec![
        ("gate", review.gate.as_str().to_owned()),
        ("iteration", review.iteration.to_string()),
    ];
    all.extend(details.iter().cloned());
    record_event(state, &review.operation, &review.mission_id, action, &all);
}

fn record_event(
    state: &DaemonState,
    operation: &str,
    mission_id: &str,
    action: &str,
    details: &[(&str, String)],
) {
    let mut event = TimelineEvent::new(
        REVIEW_EVENT,
        action,
        operation.to_owned(),
        state.clock.now().max(1) as f64,
    )
    .station(state.station_id.clone())
    .mission(mission_id);
    for (key, value) in details {
        event = event.detail(*key, value.clone());
    }
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: review timeline event was not recorded: {error}");
    }
}

fn audit(state: &DaemonState, operation: &str, action: &str, actor: &str, target: &str, at: f64) {
    let record = AuditRecord {
        id: format!("{action}-{}", uuid::Uuid::new_v4().simple()),
        actor: actor.to_owned(),
        action: action.to_owned(),
        operation: operation.to_owned(),
        resource: None,
        target: target.to_owned(),
        at,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: review audit record was not written: {error}");
    }
}

/// Text safe for a timeline detail: one line, within `[review] reason_bytes`, never empty.
fn text(state: &DaemonState, text: &str) -> String {
    let line =
        crate::directorates::m::review::one_line(text, state.station.limits().review.reason_bytes);
    if line.is_empty() {
        "unknown".to_owned()
    } else {
        line
    }
}

#[cfg(test)]
pub(super) mod testing;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod plan_tests;
