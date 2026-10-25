//! Quarantine: when an Agent is stopped everywhere, and what counts as a reason.
//!
//! The Registry keeps the quarantine records and the incidents that lead to them (see
//! [`crate::FileRegistry::quarantine_agent`]). An Operation's [`QuarantinePolicy`] says how
//! many incidents within what window quarantine an Agent. Every threshold is a setting and
//! `0` turns the matching trigger off; only an operator releases a quarantine.

use serde::{Deserialize, Serialize};

/// Default window over which incidents are counted: 24 hours.
pub const DEFAULT_WINDOW_SECS: u64 = 24 * 60 * 60;
/// Default number of stalls or loops within the window that quarantine an Agent.
pub const DEFAULT_STALLS: u32 = 3;
/// Default number of budget hard stops within the window that quarantine an Agent.
pub const DEFAULT_BUDGET_STOPS: u32 = 2;

/// What an incident was.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IncidentKind {
    /// The Watchdog found no activity and stopped the Session.
    Stall,
    /// The Watchdog found repeated output and stopped the Session.
    Loop,
    /// The Agent's commits violated the Operation's cover policy.
    Blown,
    /// A budget limit stopped the Session.
    Budget,
}

/// One incident in an Agent's recent history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Incident {
    /// What happened.
    pub kind: IncidentKind,
    /// When, as Unix seconds.
    pub at: f64,
    /// The Operation it happened in, when it had one.
    pub operation: Option<String>,
    /// The Mission it happened in.
    pub mission: String,
}

/// When an Operation quarantines an Agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuarantinePolicy {
    /// How far back incidents count, in seconds.
    pub window_secs: u64,
    /// Stalls and loops within the window that quarantine; `0` never does.
    pub stalls: u32,
    /// Budget hard stops within the window that quarantine; `0` never does.
    pub budget_stops: u32,
    /// Whether one blown cover quarantines.
    pub blown: bool,
}

impl Default for QuarantinePolicy {
    fn default() -> Self {
        Self {
            window_secs: DEFAULT_WINDOW_SECS,
            stalls: DEFAULT_STALLS,
            budget_stops: DEFAULT_BUDGET_STOPS,
            blown: true,
        }
    }
}

impl QuarantinePolicy {
    /// Why the Agent should be quarantined given its incidents, or `None`.
    ///
    /// `incidents` may include old ones; only those within the window before `now` count.
    pub fn reason(&self, incidents: &[Incident], now: f64) -> Option<String> {
        let since = now - self.window_secs as f64;
        let recent = || incidents.iter().filter(|incident| incident.at >= since);
        let count = |kinds: &[IncidentKind]| {
            recent()
                .filter(|incident| kinds.contains(&incident.kind))
                .count() as u32
        };
        let hours = self.window_secs as f64 / 3600.0;
        if self.blown && count(&[IncidentKind::Blown]) > 0 {
            return Some("a blown cover".to_owned());
        }
        let stalls = count(&[IncidentKind::Stall, IncidentKind::Loop]);
        if self.stalls > 0 && stalls >= self.stalls {
            return Some(format!("{stalls} stalls or loops within {hours:.0} hours"));
        }
        let stops = count(&[IncidentKind::Budget]);
        if self.budget_stops > 0 && stops >= self.budget_stops {
            return Some(format!("{stops} budget hard stops within {hours:.0} hours"));
        }
        None
    }
}

/// The quarantine settings as written in configuration (`[operation.ID.quarantine]`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuarantineDefinition {
    window_hours: Option<f64>,
    stalls: Option<u32>,
    budget_stops: Option<u32>,
    blown: Option<bool>,
}

impl QuarantineDefinition {
    /// Applies the written values over the defaults.
    ///
    /// # Errors
    ///
    /// Returns a message naming `window_hours` when it is not a positive number of hours.
    pub fn resolve(&self) -> Result<QuarantinePolicy, String> {
        let mut policy = QuarantinePolicy::default();
        if let Some(hours) = self.window_hours {
            if !hours.is_finite() || hours <= 0.0 {
                return Err("quarantine window_hours must be above zero".to_owned());
            }
            policy.window_secs = (hours * 3600.0).round() as u64;
        }
        policy.stalls = self.stalls.unwrap_or(policy.stalls);
        policy.budget_stops = self.budget_stops.unwrap_or(policy.budget_stops);
        policy.blown = self.blown.unwrap_or(policy.blown);
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incident(kind: IncidentKind, at: f64) -> Incident {
        Incident {
            kind,
            at,
            operation: Some("G".to_owned()),
            mission: "m".to_owned(),
        }
    }

    #[test]
    fn three_stalls_or_loops_in_a_day_quarantine_and_older_ones_do_not_count() {
        let policy = QuarantinePolicy::default();
        let now = 200_000.0;
        let day = DEFAULT_WINDOW_SECS as f64;
        let two = [
            incident(IncidentKind::Stall, now - 10.0),
            incident(IncidentKind::Loop, now - 20.0),
        ];
        assert_eq!(policy.reason(&two, now), None);
        let mut three = two.to_vec();
        three.push(incident(IncidentKind::Stall, now - 30.0));
        assert_eq!(
            policy.reason(&three, now).as_deref(),
            Some("3 stalls or loops within 24 hours")
        );
        three[2].at = now - day - 1.0;
        assert_eq!(policy.reason(&three, now), None);
    }

    #[test]
    fn a_blown_cover_and_repeated_budget_stops_quarantine() {
        let policy = QuarantinePolicy::default();
        let now = 10_000.0;
        assert_eq!(
            policy
                .reason(&[incident(IncidentKind::Blown, now)], now)
                .as_deref(),
            Some("a blown cover")
        );
        let one = [incident(IncidentKind::Budget, now)];
        assert_eq!(policy.reason(&one, now), None);
        let two = [one[0].clone(), incident(IncidentKind::Budget, now - 5.0)];
        assert!(
            policy
                .reason(&two, now)
                .unwrap()
                .contains("budget hard stops")
        );
    }

    #[test]
    fn zero_turns_a_trigger_off_and_the_window_is_a_setting() {
        let off: QuarantineDefinition =
            toml::from_str("stalls = 0\nbudget_stops = 0\nblown = false\nwindow_hours = 1\n")
                .unwrap();
        let policy = off.resolve().unwrap();
        assert_eq!(policy.window_secs, 3600);
        let now = 100_000.0;
        let many = vec![
            incident(IncidentKind::Stall, now),
            incident(IncidentKind::Stall, now),
            incident(IncidentKind::Stall, now),
            incident(IncidentKind::Budget, now),
            incident(IncidentKind::Budget, now),
            incident(IncidentKind::Blown, now),
        ];
        assert_eq!(policy.reason(&many, now), None);
        assert!(toml::from_str::<QuarantineDefinition>("stalls = -1").is_err());
        let bad: QuarantineDefinition = toml::from_str("window_hours = 0").unwrap();
        assert!(bad.resolve().unwrap_err().contains("window_hours"));
    }
}
