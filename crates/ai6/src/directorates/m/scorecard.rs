//! The scorecard: how each Agent, route and model has done in an Operation over a window.
//!
//! It is computed from the shared Session records and the usage ledger every time it is asked
//! for; nothing is kept besides them. Each attempt gets one [`Judgment`] from [`judge`], and
//! a judgment names whose doing the outcome was: only `done`, `failed`, `blown` and `stalled` are
//! the Agent's own and are judged, and a person's verdict that a `done` does not hold up turns it
//! into a failure. A provider limit, a lost Station, a hand-off to another Agent, a person's stop,
//! an approval someone has not given yet and a spending limit are shown but not held against it.
//! An attempt that never started (a failed checkout, a brief with no Agent) has no Session record
//! and so does not appear.
//!
//! Every number lists the attempts behind it, so a person can open the Missions and check.

use std::collections::BTreeMap;

use guatiao::{FromValue, Schema, ToValue};
use guatiao_intake::Form;
use serde::Serialize;

use crate::directorates::m::roster::RouteRef;
use crate::directorates::m::stewardship::StewardshipPolicy;
use crate::directorates::medical::usage::UsageTotals;
use crate::directorates::registry::sessions::{
    OutcomeVerdict, SessionRecord, SessionResult, SessionStatus, SessionStore,
};
use crate::directorates::registry::usage::UsageStore;

/// Days of attempts a scorecard covers, unless the Operation sets another.
pub const DEFAULT_WINDOW_DAYS: u64 = 30;
/// Judged attempts below which a row says its sample is small, unless the Operation sets another.
pub const DEFAULT_SMALL_SAMPLE: u64 = 5;

const DAY_SECS: f64 = 86_400.0;

/// What one attempt came to, and so whose doing it was.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Judgment {
    /// The Mission finished: for the Agent.
    Done,
    /// The Mission ended unsuccessfully: the Agent's.
    Failed,
    /// Its commits broke the Operation's cover: the Agent's.
    Blown,
    /// The Watchdog stopped it for making no progress or looping: the Agent's.
    Stalled,
    /// A person disputed its outcome or found it failed verification: the Agent's.
    Disputed,
    /// Waiting on a person's approval, an issue, an answer or a verdict: not a failure.
    WaitingOnPerson,
    /// A spending limit stopped it: a cost, not a failure.
    BudgetStop,
    /// A provider limit holds it until the quota resets: not its doing.
    Parked,
    /// Its Station or runner was lost: the infrastructure's doing.
    Lost,
    /// Another Agent or account took the Mission over after a limit.
    HandedOff,
    /// A person cancelled, interrupted, quarantined or lowered it.
    Withdrawn,
    /// Still working.
    Running,
    /// Closed with no outcome written, such as a start that failed after the record was made.
    Unrecorded,
}

impl Judgment {
    /// Every judgment, in the order a scorecard lists them.
    pub const ALL: [Self; 13] = [
        Self::Done,
        Self::Failed,
        Self::Blown,
        Self::Stalled,
        Self::Disputed,
        Self::WaitingOnPerson,
        Self::BudgetStop,
        Self::Parked,
        Self::Lost,
        Self::HandedOff,
        Self::Withdrawn,
        Self::Running,
        Self::Unrecorded,
    ];

    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Blown => "blown",
            Self::Stalled => "stalled",
            Self::Disputed => "disputed",
            Self::WaitingOnPerson => "waiting_on_person",
            Self::BudgetStop => "budget_stop",
            Self::Parked => "parked",
            Self::Lost => "lost",
            Self::HandedOff => "handed_off",
            Self::Withdrawn => "withdrawn",
            Self::Running => "running",
            Self::Unrecorded => "unrecorded",
        }
    }

    /// Whether the outcome is plausibly the Agent's own, so it is judged.
    pub fn is_judged(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Blown | Self::Stalled | Self::Disputed
        )
    }

    /// Whether it counts against the Agent.
    pub fn is_fault(self) -> bool {
        matches!(
            self,
            Self::Failed | Self::Blown | Self::Stalled | Self::Disputed
        )
    }
}

/// The judgment on one attempt, from its Session record.
pub fn judge(record: &SessionRecord) -> Judgment {
    if record.status == SessionStatus::Lost {
        return Judgment::Lost;
    }
    let verdict = record.verdict.as_ref().map(|verdict| verdict.verdict);
    if verdict.is_some_and(OutcomeVerdict::is_fault) {
        return Judgment::Disputed;
    }
    let closed = record.status.is_closed();
    let Some(result) = &record.result else {
        return if closed {
            Judgment::Unrecorded
        } else {
            Judgment::Running
        };
    };
    let reason = result.reason.as_deref();
    match result.state.as_str() {
        "done" => Judgment::Done,
        "held" if verdict == Some(OutcomeVerdict::Confirmed) => Judgment::Done,
        "held" => Judgment::WaitingOnPerson,
        "verifying" if verdict == Some(OutcomeVerdict::Confirmed) => Judgment::Done,
        "verifying" => Judgment::WaitingOnPerson,
        "failed" => Judgment::Failed,
        "blown" => Judgment::Blown,
        "stalled" => Judgment::Stalled,
        "abandoned" if reason == Some(SessionResult::HANDED_OFF) => Judgment::HandedOff,
        "abandoned" | "quarantined" => Judgment::Withdrawn,
        _ if closed => Judgment::Unrecorded,
        "parked" => Judgment::Parked,
        "blocked" => match reason {
            Some("budget") => Judgment::BudgetStop,
            Some("interrupted" | "clearance-lowered") => Judgment::Withdrawn,
            _ => Judgment::WaitingOnPerson,
        },
        _ => Judgment::Running,
    }
}

/// The scorecard settings of an Operation (`[operation.ID.scorecard]`).
#[derive(Debug, Clone, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct ScorecardDefinition {
    /// Days of attempts a scorecard covers; 0 covers every attempt on record.
    #[schema(title = "Window")]
    #[form(unit = "days")]
    #[map(default = DEFAULT_WINDOW_DAYS)]
    window_days: u64,
    /// Judged attempts below which a row says its sample is too small to rank on.
    #[schema(title = "Small sample below")]
    #[map(default = DEFAULT_SMALL_SAMPLE)]
    small_sample: u64,
    /// Whether M files proposals from the scorecard. Each rule below is off until its
    /// threshold is set.
    #[schema(title = "M proposes")]
    #[map(default = true)]
    propose: bool,
    /// A failure rate, from 0 to 1, above which M proposes holding an Agent or one of its
    /// routes. Leave it empty to never propose a hold.
    #[schema(title = "Propose a hold above")]
    hold_above: Option<f64>,
    /// A gap in failure rate, from 0 to 1, by which M proposes putting an Agent ahead of
    /// another that shares a role. Leave it empty to never propose it.
    #[schema(title = "Propose ahead by a gap of")]
    reorder_gap: Option<f64>,
    /// Whether M puts the holds it would propose on by itself, audited and lifted by a
    /// person. A hold that would leave a role with no Agent is proposed instead.
    #[schema(title = "M holds by itself")]
    #[map(default)]
    auto_hold: bool,
    /// Models M never proposes putting an Agent on or ahead.
    #[schema(title = "Never put ahead")]
    never_prefer: Option<Vec<String>>,
}

impl Default for ScorecardDefinition {
    fn default() -> Self {
        Self {
            window_days: DEFAULT_WINDOW_DAYS,
            small_sample: DEFAULT_SMALL_SAMPLE,
            propose: true,
            hold_above: None,
            reorder_gap: None,
            auto_hold: false,
            never_prefer: None,
        }
    }
}

impl ScorecardDefinition {
    /// The written values over the defaults.
    pub fn resolve(&self) -> ScorecardPolicy {
        ScorecardPolicy {
            window_days: self.window_days,
            small_sample: self.small_sample,
        }
    }

    /// What M may do from the scorecard.
    ///
    /// # Errors
    ///
    /// Returns a message naming the key for a threshold outside 0 to 1.
    pub fn rules(&self) -> Result<StewardshipPolicy, String> {
        for (name, value) in [
            ("hold_above", self.hold_above),
            ("reorder_gap", self.reorder_gap),
        ] {
            if value.is_some_and(|value| !(0.0..=1.0).contains(&value)) {
                return Err(format!("scorecard {name} must be between 0 and 1"));
            }
        }
        Ok(StewardshipPolicy {
            propose: self.propose,
            hold_above: self.hold_above,
            reorder_gap: self.reorder_gap,
            auto_hold: self.auto_hold,
            never_prefer: self.never_prefer.clone().unwrap_or_default(),
        })
    }
}

/// What an Operation's scorecard covers, resolved.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ScorecardPolicy {
    /// Days of attempts it covers; 0 covers every attempt on record.
    pub window_days: u64,
    /// Judged attempts below which a row says its sample is small.
    pub small_sample: u64,
}

impl Default for ScorecardPolicy {
    fn default() -> Self {
        Self {
            window_days: DEFAULT_WINDOW_DAYS,
            small_sample: DEFAULT_SMALL_SAMPLE,
        }
    }
}

/// One attempt behind a number, enough to open its Mission.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptRef {
    /// The Mission.
    pub mission_id: String,
    /// AI6's Session ID of the attempt.
    pub session_id: String,
    /// Which try at the Mission it was.
    pub attempt: u32,
}

/// What a group of attempts came to.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Standing {
    /// The attempts of each judgment, oldest first. The numbers are their lengths.
    pub attempts: BTreeMap<Judgment, Vec<AttemptRef>>,
    /// Attempts whose outcome is the Agent's own: done, failed, blown, stalled or disputed.
    pub judged: u64,
    /// Failed, blown, stalled and disputed among the judged; `None` before any was judged.
    pub failure_rate: Option<f64>,
    /// Whether `judged` is below the Operation's small-sample line, so no ranking should rest
    /// on it.
    pub small_sample: bool,
    /// Tools its harness refused, summed over the attempts.
    pub refused_tools: u64,
    /// Attempts in which it refused at least one.
    pub attempts_with_refusals: u64,
    /// Input, output and cache tokens its attempts consumed, from the usage ledger.
    pub usage: UsageTotals,
    /// Wall time of the closed attempts, in seconds: parked time included.
    pub wall_secs: f64,
}

impl Standing {
    /// How many attempts had `judgment`.
    pub fn count(&self, judgment: Judgment) -> u64 {
        self.attempts
            .get(&judgment)
            .map_or(0, |list| list.len() as u64)
    }

    fn add(&mut self, record: &SessionRecord, judgment: Judgment, usage: Option<&UsageTotals>) {
        self.attempts.entry(judgment).or_default().push(AttemptRef {
            mission_id: record.mission_id.clone(),
            session_id: record.session_id.clone(),
            attempt: record.attempt,
        });
        let denied = record
            .result
            .as_ref()
            .map_or(0, |result| result.denied_tools);
        self.refused_tools += u64::from(denied);
        self.attempts_with_refusals += u64::from(denied > 0);
        if let Some(usage) = usage {
            self.usage.add(usage);
        }
        if let (Some(opened), Some(closed)) = (record.opened_at, record.closed_at) {
            self.wall_secs += (closed - opened).max(0.0);
        }
    }

    fn finish(&mut self, small_sample: u64) {
        self.judged = Judgment::ALL
            .into_iter()
            .filter(|judgment| judgment.is_judged())
            .map(|judgment| self.count(judgment))
            .sum();
        let faults: u64 = Judgment::ALL
            .into_iter()
            .filter(|judgment| judgment.is_fault())
            .map(|judgment| self.count(judgment))
            .sum();
        self.failure_rate = (self.judged > 0).then(|| faults as f64 / self.judged as f64);
        self.small_sample = self.judged < small_sample;
    }
}

/// An Agent's standing, and its standing on each route it ran.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentCard {
    /// The Agent codename.
    pub agent: String,
    /// Everything it ran in the window.
    pub standing: Standing,
    /// The same by the route it ran on: handler, model and pool.
    pub routes: Vec<RouteCard>,
}

/// A route's standing.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RouteCard {
    /// The handler, model and pool it ran on.
    pub route: RouteRef,
    /// What its attempts came to.
    pub standing: Standing,
}

/// A model's standing across every Agent and route that ran it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelCard {
    /// The model; `None` is a handler's default.
    pub model: Option<String>,
    /// What its attempts came to.
    pub standing: Standing,
}

/// The scorecard of one Operation.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scorecard {
    /// The Operation.
    pub operation: String,
    /// The window it covers, in days; 0 is every attempt on record.
    pub window_days: u64,
    /// When the window starts, as Unix seconds; `None` when it is not limited.
    pub from: Option<f64>,
    /// When the scorecard was computed, as Unix seconds.
    pub until: f64,
    /// The judged-attempts line below which a row's sample is small.
    pub small_sample: u64,
    /// Each Agent that ran an attempt, by codename.
    pub agents: Vec<AgentCard>,
    /// Each model that ran one.
    pub models: Vec<ModelCard>,
    /// Attempts in the window whose record names no Agent.
    pub unattributed: u64,
}

type RouteKey = (String, Option<String>, Option<String>);

/// The scorecard of `operation` at `now`, read from `registry`'s Session records and usage
/// ledger.
///
/// # Errors
///
/// Any error reading the Session records or the ledger.
pub fn from_registry<R>(
    registry: &R,
    operation: &str,
    policy: &ScorecardPolicy,
    now: f64,
) -> std::io::Result<Scorecard>
where
    R: SessionStore + UsageStore + ?Sized,
{
    let sessions = registry.sessions()?;
    let missions: std::collections::BTreeSet<&str> = sessions
        .iter()
        .filter(|session| session.operation.as_deref() == Some(operation))
        .map(|session| session.mission_id.as_str())
        .collect();
    let mut usage = BTreeMap::new();
    for mission in missions {
        if let Some(ledger) = registry.mission_usage(Some(operation), mission)? {
            usage.extend(ledger.sessions);
        }
    }
    Ok(Scorecard::build(operation, &sessions, &usage, policy, now))
}

impl Scorecard {
    /// The scorecard of `operation` at `now` from its Sessions (other Operations' are
    /// ignored); `usage` has each Session's totals by AI6 Session ID.
    pub fn build(
        operation: &str,
        records: &[SessionRecord],
        usage: &BTreeMap<String, UsageTotals>,
        policy: &ScorecardPolicy,
        now: f64,
    ) -> Self {
        let from = (policy.window_days > 0).then_some(now - policy.window_days as f64 * DAY_SECS);
        let mut agents: BTreeMap<String, (Standing, BTreeMap<RouteKey, Standing>)> =
            BTreeMap::new();
        let mut models: BTreeMap<Option<String>, Standing> = BTreeMap::new();
        let mut unattributed = 0;
        let mut ordered: Vec<&SessionRecord> = records
            .iter()
            .filter(|record| record.operation.as_deref() == Some(operation))
            .filter(|record| match from {
                None => true,
                Some(from) => record
                    .closed_at
                    .or(record.opened_at)
                    .is_some_and(|at| at >= from),
            })
            .collect();
        ordered.sort_by(|left, right| {
            left.opened_at
                .unwrap_or_default()
                .total_cmp(&right.opened_at.unwrap_or_default())
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        for record in ordered {
            let judgment = judge(record);
            let totals = usage.get(&record.session_id);
            let launch = record.launch.as_ref();
            models
                .entry(launch.and_then(|launch| launch.model.clone()))
                .or_default()
                .add(record, judgment, totals);
            let Some(agent) = launch.and_then(|launch| launch.agent.clone()) else {
                unattributed += 1;
                continue;
            };
            let entry = agents.entry(agent).or_default();
            entry.0.add(record, judgment, totals);
            if let Some(launch) = launch {
                entry
                    .1
                    .entry((
                        launch.handler.clone(),
                        launch.model.clone(),
                        launch.pool.clone(),
                    ))
                    .or_default()
                    .add(record, judgment, totals);
            }
        }
        let line = policy.small_sample;
        Self {
            operation: operation.to_owned(),
            window_days: policy.window_days,
            from,
            until: now,
            small_sample: line,
            agents: agents
                .into_iter()
                .map(|(agent, (mut standing, routes))| {
                    standing.finish(line);
                    AgentCard {
                        agent,
                        standing,
                        routes: routes
                            .into_iter()
                            .map(|((handler, model, pool), mut standing)| {
                                standing.finish(line);
                                RouteCard {
                                    route: RouteRef::new(handler, model, pool),
                                    standing,
                                }
                            })
                            .collect(),
                    }
                })
                .collect(),
            models: models
                .into_iter()
                .map(|(model, mut standing)| {
                    standing.finish(line);
                    ModelCard { model, standing }
                })
                .collect(),
            unattributed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::sessions::SessionLaunch;

    fn attempt(
        id: &str,
        agent: &str,
        model: &str,
        state: &str,
        reason: Option<&str>,
    ) -> SessionRecord {
        SessionRecord {
            session_id: format!("{id}.1"),
            handle: None,
            mission_id: id.to_owned(),
            operation: Some("G".to_owned()),
            provider_session_id: None,
            station: "s".to_owned(),
            theater: None,
            attempt: 1,
            predecessor: None,
            status: match state {
                "done" | "failed" | "blown" | "abandoned" => SessionStatus::Ended,
                "parked" => SessionStatus::Parked,
                _ => SessionStatus::Active,
            },
            parked_until: None,
            lease_expires: 0.0,
            fence: 1,
            exposure: Vec::new(),
            launch: Some(
                SessionLaunch::new("claude", "pipe", "/w")
                    .with_model(Some(model.to_owned()), None)
                    .with_agent(Some(agent.to_owned())),
            ),
            recent: Vec::new(),
            checkpoint: None,
            checkpointed_at: None,
            lost: None,
            account: None,
            opened_at: Some(1_000.0),
            closed_at: matches!(state, "done" | "failed" | "blown" | "abandoned")
                .then_some(1_100.0),
            result: Some(SessionResult::new(state).with_reason(reason.map(str::to_owned))),
            window: None,
            verdict: None,
            deliverables: Vec::new(),
            evidence: Vec::new(),
            probe_requested: None,
        }
    }

    #[test]
    fn each_outcome_is_judged_by_whose_doing_it_was() {
        let cases = [
            ("done", None, Judgment::Done),
            ("failed", None, Judgment::Failed),
            ("blown", None, Judgment::Blown),
            ("stalled", None, Judgment::Stalled),
            ("abandoned", None, Judgment::Withdrawn),
            ("abandoned", Some("handed-off"), Judgment::HandedOff),
            ("quarantined", None, Judgment::Withdrawn),
            ("parked", None, Judgment::Parked),
            ("blocked", Some("green-ink"), Judgment::WaitingOnPerson),
            ("blocked", Some("issue"), Judgment::WaitingOnPerson),
            ("blocked", None, Judgment::WaitingOnPerson),
            ("blocked", Some("budget"), Judgment::BudgetStop),
            ("blocked", Some("interrupted"), Judgment::Withdrawn),
            ("blocked", Some("clearance-lowered"), Judgment::Withdrawn),
            ("working", None, Judgment::Running),
        ];
        for (state, reason, expected) in cases {
            let record = attempt("m", "007", "x", state, reason);
            assert_eq!(judge(&record), expected, "{state} {reason:?}");
        }
    }

    #[test]
    fn a_lost_session_and_one_closed_without_an_outcome_are_not_the_agents() {
        let mut lost = attempt("m", "007", "x", "working", None);
        lost.status = SessionStatus::Lost;
        assert_eq!(judge(&lost), Judgment::Lost);
        let mut bare = attempt("m", "007", "x", "working", None);
        bare.result = None;
        assert_eq!(judge(&bare), Judgment::Running);
        bare.status = SessionStatus::Ended;
        assert_eq!(judge(&bare), Judgment::Unrecorded);
        let mut ended_working = attempt("m", "007", "x", "working", None);
        ended_working.status = SessionStatus::Ended;
        assert_eq!(judge(&ended_working), Judgment::Unrecorded);
    }

    #[test]
    fn only_the_agents_own_outcomes_are_judged_and_faults_are_a_subset() {
        for judgment in Judgment::ALL {
            let judged = matches!(
                judgment,
                Judgment::Done
                    | Judgment::Failed
                    | Judgment::Blown
                    | Judgment::Stalled
                    | Judgment::Disputed
            );
            assert_eq!(judgment.is_judged(), judged, "{judgment:?}");
            assert!(!judgment.is_fault() || judgment.is_judged());
        }
    }

    #[test]
    fn a_verdict_changes_what_an_attempt_counts_as() {
        use crate::directorates::registry::sessions::MissionVerdict;

        let judged = |state: &str, verdict: Option<OutcomeVerdict>| {
            let mut record = attempt("m", "007", "x", state, None);
            record.verdict = verdict.map(|verdict| MissionVerdict::new(verdict, "p", 1.0, None));
            judge(&record)
        };
        assert_eq!(judged("done", None), Judgment::Done);
        assert_eq!(
            judged("done", Some(OutcomeVerdict::Confirmed)),
            Judgment::Done
        );
        assert_eq!(
            judged("done", Some(OutcomeVerdict::Disputed)),
            Judgment::Disputed
        );
        assert_eq!(
            judged("done", Some(OutcomeVerdict::FailedVerification)),
            Judgment::Disputed
        );
        assert_eq!(judged("held", None), Judgment::WaitingOnPerson);
        assert_eq!(
            judged("held", Some(OutcomeVerdict::Confirmed)),
            Judgment::Done
        );
        assert_eq!(
            judged("held", Some(OutcomeVerdict::Disputed)),
            Judgment::Disputed
        );
        assert_eq!(
            judged("failed", Some(OutcomeVerdict::Disputed)),
            Judgment::Disputed,
            "a held Mission a verdict failed counts once"
        );

        let records = [attempt("m1", "007", "x", "done", None), {
            let mut record = attempt("m2", "007", "x", "done", None);
            record.verdict = Some(MissionVerdict::new(
                OutcomeVerdict::Disputed,
                "p",
                1.0,
                None,
            ));
            record
        }];
        let card = Scorecard::build(
            "G",
            &records,
            &BTreeMap::new(),
            &ScorecardPolicy::default(),
            2_000.0,
        );
        let standing = &card.agents[0].standing;
        assert_eq!(standing.judged, 2);
        assert_eq!(standing.count(Judgment::Done), 1);
        assert_eq!(standing.failure_rate, Some(0.5));
    }

    #[test]
    fn a_fixture_of_missions_adds_up_by_hand() {
        let mut records = vec![
            attempt("m1", "007", "opus", "done", None),
            attempt("m2", "007", "opus", "done", None),
            attempt("m3", "007", "opus", "failed", None),
            attempt("m4", "007", "sonnet", "stalled", None),
            attempt("m5", "007", "sonnet", "blocked", Some("green-ink")),
            attempt("m6", "007", "sonnet", "abandoned", None),
            attempt("m7", "008", "haiku", "done", None),
            attempt("m8", "008", "haiku", "blown", None),
        ];
        records[5].result.as_mut().unwrap().denied_tools = 2;
        records[1].result.as_mut().unwrap().denied_tools = 1;
        let mut elsewhere = attempt("m9", "007", "opus", "failed", None);
        elsewhere.operation = Some("L".to_owned());
        records.push(elsewhere);
        let mut stray = attempt("m10", "x", "opus", "done", None);
        stray.launch.as_mut().unwrap().agent = None;
        records.push(stray);
        let usage = BTreeMap::from([
            (
                "m1.1".to_owned(),
                UsageTotals {
                    input: 100,
                    output: 10,
                    cost_micro_usd: 500,
                    ..UsageTotals::default()
                },
            ),
            (
                "m3.1".to_owned(),
                UsageTotals {
                    input: 50,
                    output: 5,
                    cost_micro_usd: 250,
                    ..UsageTotals::default()
                },
            ),
        ]);
        let card = Scorecard::build("G", &records, &usage, &ScorecardPolicy::default(), 2_000.0);
        assert_eq!(card.unattributed, 1);
        let seven = &card.agents[0];
        assert_eq!(seven.agent, "007");
        let standing = &seven.standing;
        assert_eq!(
            (
                standing.count(Judgment::Done),
                standing.count(Judgment::Failed),
                standing.count(Judgment::Stalled),
                standing.count(Judgment::WaitingOnPerson),
                standing.count(Judgment::Withdrawn),
            ),
            (2, 1, 1, 1, 1)
        );
        assert_eq!(standing.judged, 4);
        assert_eq!(standing.failure_rate, Some(0.5));
        assert!(standing.small_sample, "4 judged is under 5");
        assert_eq!(
            (standing.refused_tools, standing.attempts_with_refusals),
            (3, 2)
        );
        assert_eq!(standing.usage.cost_micro_usd, 750);
        assert_eq!(standing.usage.tokens(), 165);
        assert_eq!(standing.wall_secs, 400.0);
        assert_eq!(
            standing.attempts[&Judgment::Failed],
            vec![AttemptRef {
                mission_id: "m3".to_owned(),
                session_id: "m3.1".to_owned(),
                attempt: 1
            }]
        );
        let routes: Vec<_> = seven
            .routes
            .iter()
            .map(|card| (card.route.model.clone().unwrap(), card.standing.judged))
            .collect();
        assert_eq!(routes, [("opus".to_owned(), 3), ("sonnet".to_owned(), 1)]);
        let eight = &card.agents[1];
        assert_eq!(eight.standing.failure_rate, Some(0.5));
        let by_model: Vec<_> = card
            .models
            .iter()
            .map(|card| (card.model.clone().unwrap(), card.standing.judged))
            .collect();
        assert_eq!(
            by_model,
            [
                ("haiku".to_owned(), 2),
                ("opus".to_owned(), 4),
                ("sonnet".to_owned(), 1)
            ],
            "the stray attempt still counts toward its model"
        );
    }

    #[test]
    fn the_window_keeps_recent_attempts_and_zero_keeps_all() {
        let mut old = attempt("old", "007", "x", "done", None);
        old.opened_at = Some(10.0);
        old.closed_at = Some(20.0);
        let recent = attempt("new", "007", "x", "done", None);
        let records = [old, recent];
        let policy = ScorecardPolicy {
            window_days: 1,
            small_sample: 1,
        };
        let now = 20.0 + 2.0 * DAY_SECS;
        let mut recent_only = records.clone();
        recent_only[1].closed_at = Some(now - 100.0);
        let card = Scorecard::build("G", &recent_only, &BTreeMap::new(), &policy, now);
        assert_eq!(card.agents[0].standing.judged, 1);
        assert!(!card.agents[0].standing.small_sample);
        let all = ScorecardPolicy {
            window_days: 0,
            small_sample: 1,
        };
        let card = Scorecard::build("G", &records, &BTreeMap::new(), &all, now);
        assert_eq!((card.from, card.agents[0].standing.judged), (None, 2));
    }

    #[test]
    fn settings_default_and_resolve() {
        let policy: ScorecardDefinition =
            crate::directorates::m::records::from_toml("window_days = 7\n").unwrap();
        assert_eq!(
            policy.resolve(),
            ScorecardPolicy {
                window_days: 7,
                small_sample: DEFAULT_SMALL_SAMPLE
            }
        );
        assert_eq!(
            ScorecardDefinition::default().resolve(),
            ScorecardPolicy::default()
        );
        assert!(
            crate::directorates::m::records::from_toml::<ScorecardDefinition>("days = 7\n")
                .is_err()
        );
    }
}
