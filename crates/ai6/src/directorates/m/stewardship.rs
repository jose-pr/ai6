//! What M proposes from the scorecard, and the one thing it may do by itself.
//!
//! Each round M reads an Operation's scorecard against the Agents posted to it and files
//! **proposals** a person decides: hold an Agent or one of its routes (stop sending it new
//! work), or put one Agent ahead of another that shares a role. Every rule is off until the
//! Operation sets its threshold, and none looks at a row whose sample is small. With
//! `auto_hold` M puts a hold on itself instead of asking, the reversible direction only; a
//! hold that would leave a role with no Agent is refused and proposed instead.
//!
//! Nothing here widens anything: no role, clearance or account is ever granted. An Agent with
//! no roles is used only when named, so it is never proposed. A proposal can only name Agents
//! the Operation's roster already posts, so what the roster keeps out of an Operation stays out.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::directorates::m::placement::AgentFacts;
use crate::directorates::m::roster::RouteRef;
use crate::directorates::m::scorecard::{Scorecard, Standing};
use crate::directorates::registry::holds::RosterHold;

/// The green ink kind of a proposal to hold an Agent or route.
pub const HOLD_KIND: &str = "hold";
/// The green ink kind of a proposal to put an Agent ahead of another.
pub const PREFER_KIND: &str = "prefer";

/// What the Operation lets M do from the scorecard, resolved.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StewardshipPolicy {
    /// Whether M files proposals at all.
    pub propose: bool,
    /// A failure rate above which M proposes holding an Agent or one of its routes; `None`
    /// never proposes a hold.
    pub hold_above: Option<f64>,
    /// A gap in failure rate by which M proposes putting an Agent ahead of another sharing one
    /// of its roles; `None` never proposes it.
    pub reorder_gap: Option<f64>,
    /// Whether M holds by itself what it would propose holding.
    pub auto_hold: bool,
    /// Models M never proposes moving work onto.
    pub never_prefer: Vec<String>,
}

impl Default for StewardshipPolicy {
    fn default() -> Self {
        Self {
            propose: true,
            hold_above: None,
            reorder_gap: None,
            auto_hold: false,
            never_prefer: Vec::new(),
        }
    }
}

/// One step M would take.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// Hold the Agent, or one route of it.
    Hold {
        /// The Agent codename.
        agent: String,
        /// The route; `None` is the whole Agent.
        route: Option<RouteRef>,
    },
    /// Put `agent` ahead of `over` for the roles they share.
    Prefer {
        /// The Agent that has done better.
        agent: String,
        /// The Agent it is put ahead of.
        over: String,
    },
}

/// A step and the evidence for it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// What M would do.
    pub step: Step,
    /// What a green ink request names it by: `AGENT`, `AGENT:HANDLER/MODEL/POOL`, or
    /// `AGENT>OVER`, then `@OPERATION`.
    pub target: String,
    /// Why, with the numbers it rests on.
    pub reason: String,
    /// Judged attempts the proposal rests on, so a refused one is asked again only when the
    /// sample has grown.
    pub judged: u64,
}

impl Proposal {
    /// The green ink kind that carries it.
    pub fn kind(&self) -> &'static str {
        match self.step {
            Step::Hold { .. } => HOLD_KIND,
            Step::Prefer { .. } => PREFER_KIND,
        }
    }
}

/// The name a route goes by in a request's target.
pub fn route_name(route: &RouteRef) -> String {
    format!(
        "{}/{}/{}",
        route.handler,
        route.model.as_deref().unwrap_or("-"),
        route.pool.as_deref().unwrap_or("-")
    )
}

/// The target of a hold on `agent` (or one `route`) in `operation`.
pub fn hold_target(agent: &str, route: Option<&RouteRef>, operation: &str) -> String {
    match route {
        None => format!("{agent}@{operation}"),
        Some(route) => format!("{agent}:{}@{operation}", route_name(route)),
    }
}

fn percent(rate: f64) -> String {
    format!("{:.0} %", rate * 100.0)
}

fn evidence(standing: &Standing, window_days: u64) -> String {
    let faults: u64 = standing
        .attempts
        .iter()
        .filter(|(judgment, _)| judgment.is_fault())
        .map(|(_, list)| list.len() as u64)
        .sum();
    let missions: BTreeSet<&str> = standing
        .attempts
        .iter()
        .filter(|(judgment, _)| judgment.is_fault())
        .flat_map(|(_, list)| list.iter().map(|attempt| attempt.mission_id.as_str()))
        .collect();
    let window = if window_days == 0 {
        "on record".to_owned()
    } else {
        format!("in the last {window_days} days")
    };
    format!(
        "{faults} of {} judged attempts failed, blew their cover or stalled {window}, costing \
         ${:.2}. Missions: {}.",
        standing.judged,
        standing.usage.cost_micro_usd as f64 / 1_000_000.0,
        missions.into_iter().collect::<Vec<_>>().join(", ")
    )
}

/// The models of an Agent: its own and each route's.
fn models(agent: &AgentFacts) -> impl Iterator<Item = &str> {
    agent.model.as_deref().into_iter().chain(
        agent
            .routes
            .iter()
            .filter_map(|route| route.model.as_deref()),
    )
}

/// What M proposes for `operation` from `card`, given the Agents posted to it and the holds it
/// already has. An Agent with no roles is skipped by every rule; a row with a small sample is
/// never judged; a held Agent or route is not proposed again.
pub fn propose(
    card: &Scorecard,
    agents: &[AgentFacts],
    held: &[RosterHold],
    policy: &StewardshipPolicy,
) -> Vec<Proposal> {
    let mut proposals = Vec::new();
    if !policy.propose {
        return proposals;
    }
    let operation = card.operation.as_str();
    let named = |agent: &AgentFacts| !agent.roles.is_empty();
    let held_agent = |agent: &str| {
        held.iter()
            .any(|hold| hold.agent == agent && hold.holds_agent())
    };
    let held_route = |agent: &str, route: &RouteRef| {
        held.iter()
            .any(|hold| hold.agent == agent && hold.route.as_ref() == Some(route))
    };
    if let Some(limit) = policy.hold_above {
        for row in &card.agents {
            let Some(facts) = agents.iter().find(|facts| facts.id == row.agent) else {
                continue;
            };
            if !named(facts) {
                continue;
            }
            let over = |standing: &Standing| {
                !standing.small_sample && standing.failure_rate.is_some_and(|rate| rate > limit)
            };
            if over(&row.standing) {
                if !held_agent(&row.agent) {
                    proposals.push(Proposal {
                        step: Step::Hold {
                            agent: row.agent.clone(),
                            route: None,
                        },
                        target: hold_target(&row.agent, None, operation),
                        reason: format!(
                            "Hold {}: {} failure rate {}, above the {} this Operation sets. {}",
                            row.agent,
                            "its",
                            percent(row.standing.failure_rate.unwrap_or_default()),
                            percent(limit),
                            evidence(&row.standing, card.window_days)
                        ),
                        judged: row.standing.judged,
                    });
                }
                continue;
            }
            if facts.routes.len() < 2 || held_agent(&row.agent) {
                continue;
            }
            for route in &row.routes {
                if over(&route.standing) && !held_route(&row.agent, &route.route) {
                    proposals.push(Proposal {
                        step: Step::Hold {
                            agent: row.agent.clone(),
                            route: Some(route.route.clone()),
                        },
                        target: hold_target(&row.agent, Some(&route.route), operation),
                        reason: format!(
                            "Hold {}'s route {}: failure rate {}, above the {} this Operation \
                             sets. {}",
                            row.agent,
                            route.route,
                            percent(route.standing.failure_rate.unwrap_or_default()),
                            percent(limit),
                            evidence(&route.standing, card.window_days)
                        ),
                        judged: route.standing.judged,
                    });
                }
            }
        }
    }
    if let Some(gap) = policy.reorder_gap {
        let rated = |id: &str| {
            card.agents
                .iter()
                .find(|row| row.agent == id)
                .filter(|row| !row.standing.small_sample)
                .and_then(|row| row.standing.failure_rate.map(|rate| (rate, row)))
        };
        for better in agents.iter().filter(|agent| named(agent)) {
            let Some((better_rate, better_row)) = rated(&better.id) else {
                continue;
            };
            if held_agent(&better.id)
                || models(better)
                    .any(|model| policy.never_prefer.iter().any(|never| never == model))
            {
                continue;
            }
            for worse in agents
                .iter()
                .filter(|agent| named(agent) && agent.id != better.id)
            {
                let shares = better.roles.iter().any(|role| worse.roles.contains(role));
                let Some((worse_rate, _)) = rated(&worse.id) else {
                    continue;
                };
                if shares && worse_rate - better_rate >= gap && better.priority <= worse.priority {
                    proposals.push(Proposal {
                        step: Step::Prefer {
                            agent: better.id.clone(),
                            over: worse.id.clone(),
                        },
                        target: format!("{}>{}@{operation}", better.id, worse.id),
                        reason: format!(
                            "Put {} ahead of {} (priority {} against {}): failure rate {} \
                             against {}, a gap of at least the {} this Operation sets. {}",
                            better.id,
                            worse.id,
                            better.priority,
                            worse.priority,
                            percent(better_rate),
                            percent(worse_rate),
                            percent(gap),
                            evidence(&better_row.standing, card.window_days)
                        ),
                        judged: better_row.standing.judged,
                    });
                }
            }
        }
    }
    proposals
}

/// The roles a hold would leave with no Agent able to take them: each role of the held Agent
/// (or of every Agent a route hold would leave with no route) that no other posted Agent
/// qualified for it can still serve. `unavailable` are the codenames already passed over
/// (quarantined, retired, held).
pub fn roles_left_empty(
    agents: &[AgentFacts],
    holding: &str,
    route: Option<&RouteRef>,
    unavailable: &[String],
    held_routes: &[RosterHold],
) -> Vec<String> {
    let Some(target) = agents.iter().find(|agent| agent.id == holding) else {
        return Vec::new();
    };
    if let Some(route) = route {
        // Another route of the same Agent keeps its roles served.
        let others = target
            .routes
            .iter()
            .filter(|other| *other != route)
            .filter(|other| {
                !held_routes
                    .iter()
                    .any(|hold| hold.agent == holding && hold.route.as_ref() == Some(*other))
            })
            .count();
        if others > 0 {
            return Vec::new();
        }
    }
    let mut empty: Vec<String> = target
        .roles
        .iter()
        .filter(|role| {
            !agents.iter().any(|other| {
                other.id != holding
                    && other.roles.contains(role)
                    && !unavailable.contains(&other.id)
            })
        })
        .cloned()
        .collect();
    empty.sort();
    empty
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::directorates::m::scorecard::{
        AgentCard, AttemptRef, Judgment, RouteCard, Scorecard,
    };

    fn standing(done: usize, failed: usize) -> Standing {
        let mut attempts = BTreeMap::new();
        let refs = |prefix: &str, n: usize| -> Vec<AttemptRef> {
            (0..n)
                .map(|index| AttemptRef {
                    mission_id: format!("{prefix}-{index}"),
                    session_id: format!("{prefix}-{index}.1"),
                    attempt: 1,
                })
                .collect()
        };
        attempts.insert(Judgment::Done, refs("d", done));
        attempts.insert(Judgment::Failed, refs("f", failed));
        let judged = (done + failed) as u64;
        Standing {
            attempts,
            judged,
            failure_rate: (judged > 0).then(|| failed as f64 / judged as f64),
            small_sample: judged < 5,
            ..Standing::default()
        }
    }

    fn facts(id: &str, roles: &[&str], priority: i64, model: &str) -> AgentFacts {
        AgentFacts {
            id: id.to_owned(),
            handler: Some("claude".to_owned()),
            model: Some(model.to_owned()),
            routes: Vec::new(),
            priority,
            roles: roles.iter().map(|role| (*role).to_owned()).collect(),
            tags: Vec::new(),
        }
    }

    fn scorecard_of(agents: Vec<AgentCard>) -> Scorecard {
        Scorecard {
            operation: "G".to_owned(),
            window_days: 30,
            from: None,
            until: 0.0,
            small_sample: 5,
            agents,
            models: Vec::new(),
            unattributed: 0,
        }
    }

    fn row(agent: &str, standing: Standing, routes: Vec<RouteCard>) -> AgentCard {
        AgentCard {
            agent: agent.to_owned(),
            standing,
            routes,
        }
    }

    fn route_row(route: RouteRef, standing: Standing) -> RouteCard {
        RouteCard { route, standing }
    }

    fn policy() -> StewardshipPolicy {
        StewardshipPolicy {
            hold_above: Some(0.5),
            reorder_gap: Some(0.3),
            ..StewardshipPolicy::default()
        }
    }

    #[test]
    fn nothing_is_proposed_until_a_threshold_is_set_or_proposing_is_off() {
        let card = scorecard_of(vec![row("007", standing(1, 9), vec![])]);
        let agents = [facts("007", &["case-officer"], 0, "opus")];
        assert!(propose(&card, &agents, &[], &StewardshipPolicy::default()).is_empty());
        let off = StewardshipPolicy {
            propose: false,
            ..policy()
        };
        assert!(propose(&card, &agents, &[], &off).is_empty());
        assert_eq!(propose(&card, &agents, &[], &policy()).len(), 1);
    }

    #[test]
    fn an_agent_over_the_limit_is_proposed_for_a_hold_with_its_evidence() {
        let card = scorecard_of(vec![row("007", standing(2, 8), vec![])]);
        let agents = [facts("007", &["case-officer"], 0, "opus")];
        let proposals = propose(&card, &agents, &[], &policy());
        let [proposal] = proposals.as_slice() else {
            panic!("{proposals:?}");
        };
        assert_eq!(proposal.kind(), HOLD_KIND);
        assert_eq!(proposal.target, "007@G");
        assert_eq!(proposal.judged, 10);
        assert!(proposal.reason.contains("80 %"), "{}", proposal.reason);
        assert!(proposal.reason.contains("50 %"), "{}", proposal.reason);
        assert!(proposal.reason.contains("8 of 10"), "{}", proposal.reason);
        assert!(proposal.reason.contains("f-0"), "the Missions behind it");
    }

    #[test]
    fn a_small_sample_a_rate_at_the_limit_or_a_held_agent_is_not_proposed() {
        let agents = [facts("007", &["case-officer"], 0, "opus")];
        let small = scorecard_of(vec![row("007", standing(0, 4), vec![])]);
        assert!(propose(&small, &agents, &[], &policy()).is_empty());
        let at_limit = scorecard_of(vec![row("007", standing(5, 5), vec![])]);
        assert!(propose(&at_limit, &agents, &[], &policy()).is_empty());
        let over = scorecard_of(vec![row("007", standing(1, 9), vec![])]);
        let held = [RosterHold::new("G", "007", None, "failing", "m", 1.0)];
        assert!(propose(&over, &agents, &held, &policy()).is_empty());
    }

    #[test]
    fn an_agent_with_no_roles_is_never_proposed() {
        let card = scorecard_of(vec![
            row("fable", standing(0, 9), vec![]),
            row("008", standing(9, 0), vec![]),
        ]);
        let agents = [
            facts("fable", &[], 0, "claude-fable-5-1"),
            facts("008", &["case-officer"], 0, "opus"),
        ];
        assert!(propose(&card, &agents, &[], &policy()).is_empty());
        let named_only_better = scorecard_of(vec![
            row("fable", standing(10, 0), vec![]),
            row("008", standing(2, 8), vec![]),
        ]);
        // It has done better than 008, but it is used only when named: never put ahead.
        let proposals = propose(&named_only_better, &agents, &[], &policy());
        assert!(
            proposals.iter().all(
                |proposal| !matches!(&proposal.step, Step::Prefer { agent, .. } if agent == "fable")
            ),
            "{proposals:?}"
        );
    }

    #[test]
    fn one_route_of_a_routed_agent_is_proposed_alone() {
        let good = RouteRef::new("claude", Some("opus".into()), Some("team".into()));
        let bad = RouteRef::new("claude", Some("gemini-free".into()), None);
        let mut routed = facts("007", &["case-officer"], 0, "opus");
        routed.routes = vec![good.clone(), bad.clone()];
        let card = scorecard_of(vec![row(
            "007",
            standing(10, 6),
            vec![
                route_row(good, standing(9, 1)),
                route_row(bad.clone(), standing(1, 5)),
            ],
        )]);
        let proposals = propose(&card, &[routed.clone()], &[], &policy());
        let [proposal] = proposals.as_slice() else {
            panic!("{proposals:?}");
        };
        assert_eq!(
            proposal.step,
            Step::Hold {
                agent: "007".into(),
                route: Some(bad.clone())
            }
        );
        assert_eq!(proposal.target, "007:claude/gemini-free/-@G");
        let held = [RosterHold::new("G", "007", Some(bad), "bad", "m", 1.0)];
        assert!(propose(&card, &[routed], &held, &policy()).is_empty());
    }

    #[test]
    fn the_better_agent_is_put_ahead_only_across_the_gap_and_never_onto_a_forbidden_model() {
        let rows = || {
            vec![
                row("007", standing(4, 6), vec![]),
                row("008", standing(9, 1), vec![]),
            ]
        };
        let agents = [
            facts("007", &["case-officer"], 5, "sonnet"),
            facts("008", &["case-officer"], 1, "haiku"),
        ];
        let loose = StewardshipPolicy {
            reorder_gap: Some(0.3),
            ..StewardshipPolicy::default()
        };
        let proposals = propose(&scorecard_of(rows()), &agents, &[], &loose);
        let [proposal] = proposals.as_slice() else {
            panic!("{proposals:?}");
        };
        assert_eq!(proposal.kind(), PREFER_KIND);
        assert_eq!(proposal.target, "008>007@G");
        assert!(proposal.reason.contains("10 %") && proposal.reason.contains("60 %"));

        let wide = StewardshipPolicy {
            reorder_gap: Some(0.6),
            ..StewardshipPolicy::default()
        };
        assert!(
            propose(&scorecard_of(rows()), &agents, &[], &wide).is_empty(),
            "gap too small"
        );

        let forbidden = StewardshipPolicy {
            never_prefer: vec!["haiku".into()],
            ..loose.clone()
        };
        assert!(
            propose(&scorecard_of(rows()), &agents, &[], &forbidden).is_empty(),
            "never onto a model the Operation forbids"
        );

        let mut ahead = agents.clone();
        ahead[0].priority = 0;
        ahead[1].priority = 9;
        assert!(
            propose(&scorecard_of(rows()), &ahead, &[], &loose).is_empty(),
            "already ahead"
        );

        let mut apart = agents.clone();
        apart[1].roles = vec!["targeter".into()];
        assert!(
            propose(&scorecard_of(rows()), &apart, &[], &loose).is_empty(),
            "no shared role"
        );
    }

    #[test]
    fn a_hold_that_would_leave_a_role_empty_names_it() {
        let agents = [
            facts("007", &["case-officer", "targeter"], 0, "opus"),
            facts("008", &["case-officer"], 0, "opus"),
        ];
        assert_eq!(
            roles_left_empty(&agents, "007", None, &[], &[]),
            ["targeter"],
            "008 can still take case-officer"
        );
        assert_eq!(
            roles_left_empty(&agents, "007", None, &["008".into()], &[]),
            ["case-officer", "targeter"],
            "008 is already passed over"
        );
        assert!(roles_left_empty(&agents, "008", None, &[], &[]).is_empty());
    }

    #[test]
    fn a_route_hold_leaves_the_agent_served_by_its_other_route() {
        let one = RouteRef::new("claude", None, None);
        let two = RouteRef::new("codex", None, None);
        let mut routed = facts("007", &["case-officer"], 0, "opus");
        routed.routes = vec![one.clone(), two.clone()];
        let agents = [routed];
        assert!(roles_left_empty(&agents, "007", Some(&one), &[], &[]).is_empty());
        let both = [RosterHold::new("G", "007", Some(two), "x", "m", 1.0)];
        assert_eq!(
            roles_left_empty(&agents, "007", Some(&one), &[], &both),
            ["case-officer"],
            "its other route is already held"
        );
    }
}
