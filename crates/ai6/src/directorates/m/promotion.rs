//! Promotion: when M asks a person to raise an Agent's clearance in an Operation.
//!
//! Autonomy is earned per Operation. When an Agent's record there meets a step's thresholds,
//! M files a green ink request of kind [`CLEARANCE_KIND`] for the next clearance; nothing is
//! raised until a person signs it, and the grant it leads to never exceeds the Operation's
//! clearance. An Operation's [`PromotionPolicy`] sets the thresholds (`[operation.ID.promotion]`):
//! by default `desk` to `field` after 10 Missions done with no blown cover or quarantine in
//! 30 days, and `field` to `00` after 25 done, at most 10 % failed or blown and no incident
//! in 60 days. `missions = 0` turns a step off.

use serde::Deserialize;

use crate::directorates::m::roster::CLEARANCES;
use crate::directorates::registry::agent_records::{AgentOutcome, AgentTally};

/// The green ink kind of a promotion request.
pub const CLEARANCE_KIND: &str = "clearance";
/// Missions done before a `desk` Agent is put up for `field`, by default.
pub const DEFAULT_FIELD_MISSIONS: u64 = 10;
/// Days without a blown cover or quarantine before a `desk` Agent is put up for `field`.
pub const DEFAULT_FIELD_QUIET_DAYS: u64 = 30;
/// Missions done before a `field` Agent is put up for `00`, by default.
pub const DEFAULT_00_MISSIONS: u64 = 25;
/// The highest share of failed or blown Missions for `00`, by default.
pub const DEFAULT_00_MAX_FAILURE_RATE: f64 = 0.1;
/// Days without any incident before a `field` Agent is put up for `00`.
pub const DEFAULT_00_QUIET_DAYS: u64 = 60;

/// What a record must show before M asks for one clearance step.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct PromotionStep {
    /// Missions done in the Operation; `0` turns the step off.
    pub missions: u64,
    /// The highest share of failed and blown Missions among those done, failed or blown;
    /// `None` allows any.
    pub max_failure_rate: Option<f64>,
    /// Days that must have passed since the latest of `quiet_on`.
    pub quiet_days: u64,
    /// The outcomes that may not have happened within `quiet_days`.
    pub quiet_on: Vec<AgentOutcome>,
}

impl PromotionStep {
    /// A step after `missions` Missions done, with no failure-rate limit or quiet period.
    pub fn after(missions: u64) -> Self {
        Self {
            missions,
            max_failure_rate: None,
            quiet_days: 0,
            quiet_on: Vec::new(),
        }
    }

    /// Why `tally` (with `everywhere`, the outcomes that apply in every Operation) does not
    /// meet the step at `now`, or `None` when it does.
    pub fn shortfall(
        &self,
        tally: &AgentTally,
        everywhere: &AgentTally,
        now: f64,
    ) -> Option<String> {
        if self.missions == 0 {
            return Some("this step is turned off".to_owned());
        }
        if tally.done < self.missions {
            return Some(format!("{} of {} Missions done", tally.done, self.missions));
        }
        if let Some(limit) = self.max_failure_rate
            && tally.failure_rate().is_some_and(|rate| rate > limit)
        {
            return Some(format!("failure rate above {:.0} %", limit * 100.0));
        }
        let since = now - self.quiet_days as f64 * 86_400.0;
        let latest = [tally, everywhere]
            .iter()
            .filter_map(|tally| tally.latest(&self.quiet_on))
            .reduce(f64::max);
        if latest.is_some_and(|at| at > since) {
            return Some(format!("an incident within {} days", self.quiet_days));
        }
        None
    }

    fn describe(&self) -> String {
        let mut parts = vec![format!("{} Missions done", self.missions)];
        if let Some(limit) = self.max_failure_rate {
            parts.push(format!("at most {:.0} % failed or blown", limit * 100.0));
        }
        if self.quiet_days > 0 && !self.quiet_on.is_empty() {
            let names = self
                .quiet_on
                .iter()
                .map(|outcome| outcome.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("no {names} in {} days", self.quiet_days));
        }
        parts.join(", ")
    }
}

/// When M puts an Agent up for a higher clearance in an Operation.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct PromotionPolicy {
    /// `desk` to `field`.
    pub field: PromotionStep,
    /// `field` to `00`.
    pub double_o: PromotionStep,
}

impl Default for PromotionPolicy {
    fn default() -> Self {
        Self {
            field: PromotionStep {
                missions: DEFAULT_FIELD_MISSIONS,
                max_failure_rate: None,
                quiet_days: DEFAULT_FIELD_QUIET_DAYS,
                quiet_on: vec![AgentOutcome::Blown, AgentOutcome::Quarantined],
            },
            double_o: PromotionStep {
                missions: DEFAULT_00_MISSIONS,
                max_failure_rate: Some(DEFAULT_00_MAX_FAILURE_RATE),
                quiet_days: DEFAULT_00_QUIET_DAYS,
                quiet_on: AgentOutcome::ALL
                    .into_iter()
                    .filter(|outcome| outcome.is_incident())
                    .collect(),
            },
        }
    }
}

/// A clearance step an Agent's record has earned.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    /// The clearance it works at now.
    pub from: String,
    /// The clearance M asks for.
    pub to: String,
    /// How many times over the record meets the step's Mission count; a refused request is
    /// asked again only when this grows.
    pub round: u64,
    /// What the record shows, for the signer.
    pub reason: String,
}

impl PromotionPolicy {
    /// The step from `current` that `tally` has earned at `now`, if it stays within
    /// `ceiling` (the Operation's clearance). `everywhere` holds the outcomes that apply in
    /// every Operation, such as a quarantine of every Operation.
    pub fn due(
        &self,
        current: &str,
        ceiling: &str,
        tally: &AgentTally,
        everywhere: &AgentTally,
        now: f64,
    ) -> Option<Promotion> {
        let (step, to) = match current {
            "desk" => (&self.field, "field"),
            "field" => (&self.double_o, "00"),
            _ => return None,
        };
        let rank = |clearance| CLEARANCES.iter().position(|known| *known == clearance);
        if rank(to)? > rank(ceiling)? || step.shortfall(tally, everywhere, now).is_some() {
            return None;
        }
        Some(Promotion {
            from: current.to_owned(),
            to: to.to_owned(),
            round: tally.done / step.missions,
            reason: format!(
                "{} Missions done; meets {current} to {to}: {}",
                tally.done,
                step.describe()
            ),
        })
    }
}

/// One step as written in configuration; unset keys keep the default.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StepDefinition {
    missions: Option<u64>,
    max_failure_rate: Option<f64>,
    quiet_days: Option<u64>,
    quiet_on: Option<Vec<String>>,
}

impl StepDefinition {
    fn apply(&self, step: &mut PromotionStep, name: &str) -> Result<(), String> {
        if let Some(missions) = self.missions {
            step.missions = missions;
        }
        if let Some(rate) = self.max_failure_rate {
            if !(0.0..=1.0).contains(&rate) {
                return Err(format!(
                    "promotion {name} max_failure_rate must be between 0 and 1"
                ));
            }
            step.max_failure_rate = Some(rate);
        }
        if let Some(days) = self.quiet_days {
            step.quiet_days = days;
        }
        if let Some(names) = &self.quiet_on {
            step.quiet_on = names
                .iter()
                .map(|name| {
                    AgentOutcome::from_name(name)
                        .ok_or_else(|| format!("promotion {name:?} is not an Agent outcome"))
                })
                .collect::<Result<_, _>>()?;
        }
        Ok(())
    }
}

/// The promotion settings as written in configuration (`[operation.ID.promotion]`, with a
/// `field` and a `"00"` table).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PromotionDefinition {
    field: StepDefinition,
    #[serde(rename = "00")]
    double_o: StepDefinition,
}

impl PromotionDefinition {
    /// Applies the written values over the defaults.
    ///
    /// # Errors
    ///
    /// Returns a message naming the key for a failure rate outside 0 to 1 or an unknown
    /// outcome in `quiet_on`.
    pub fn resolve(&self) -> Result<PromotionPolicy, String> {
        let mut policy = PromotionPolicy::default();
        self.field.apply(&mut policy.field, "field")?;
        self.double_o.apply(&mut policy.double_o, "00")?;
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: f64 = 86_400.0;

    fn tally(done: u64, failed: u64) -> AgentTally {
        AgentTally {
            done,
            failed,
            ..AgentTally::default()
        }
    }

    #[test]
    fn a_desk_agent_is_put_up_for_field_after_ten_clean_missions() {
        let policy = PromotionPolicy::default();
        let now = 100.0 * DAY;
        let none = AgentTally::default();
        assert_eq!(policy.due("desk", "00", &tally(9, 0), &none, now), None);
        let due = policy.due("desk", "00", &tally(12, 3), &none, now).unwrap();
        assert_eq!(
            (due.from.as_str(), due.to.as_str(), due.round),
            ("desk", "field", 1)
        );
        assert_eq!(
            policy.due("desk", "desk", &tally(12, 0), &none, now),
            None,
            "ceiling"
        );

        let mut blown = tally(12, 0);
        blown.blown = 1;
        blown.last.insert(AgentOutcome::Blown, now - 29.0 * DAY);
        assert_eq!(policy.due("desk", "00", &blown, &none, now), None);
        blown.last.insert(AgentOutcome::Blown, now - 31.0 * DAY);
        assert!(policy.due("desk", "00", &blown, &none, now).is_some());

        let mut quarantined = AgentTally::default();
        quarantined
            .last
            .insert(AgentOutcome::Quarantined, now - DAY);
        assert_eq!(
            policy.due("desk", "00", &tally(12, 0), &quarantined, now),
            None,
            "a quarantine of every Operation counts"
        );
    }

    #[test]
    fn field_to_00_needs_a_low_failure_rate_and_no_incident() {
        let policy = PromotionPolicy::default();
        let now = 200.0 * DAY;
        let none = AgentTally::default();
        assert!(
            policy
                .due("field", "00", &tally(27, 3), &none, now)
                .is_some()
        );
        assert_eq!(policy.due("field", "00", &tally(27, 4), &none, now), None);
        assert_eq!(
            policy.due("field", "field", &tally(50, 0), &none, now),
            None
        );
        let mut stalled = tally(50, 0);
        stalled.last.insert(AgentOutcome::Stalled, now - 59.0 * DAY);
        assert_eq!(policy.due("field", "00", &stalled, &none, now), None);
        assert_eq!(policy.due("00", "00", &tally(99, 0), &none, now), None);
        assert_eq!(
            policy
                .due("field", "00", &tally(50, 0), &none, now)
                .unwrap()
                .round,
            2
        );
    }

    #[test]
    fn steps_are_configurable_and_zero_turns_one_off() {
        let definition: PromotionDefinition = toml::from_str(
            "[field]\nmissions = 0\n[\"00\"]\nmissions = 3\nmax_failure_rate = 0.5\nquiet_on = [\"blown\"]\n",
        )
        .unwrap();
        let policy = definition.resolve().unwrap();
        let none = AgentTally::default();
        assert_eq!(policy.due("desk", "00", &tally(100, 0), &none, 1.0), None);
        assert!(
            policy
                .due("field", "00", &tally(3, 3), &none, 1.0)
                .is_some()
        );
        for bad in [
            "[field]\nmax_failure_rate = 2.0\n",
            "[field]\nquiet_on = [\"late\"]\n",
        ] {
            let definition: PromotionDefinition = toml::from_str(bad).unwrap();
            assert!(definition.resolve().is_err(), "{bad}");
        }
        assert!(toml::from_str::<PromotionDefinition>("[desk]\nmissions = 1\n").is_err());
    }
}
