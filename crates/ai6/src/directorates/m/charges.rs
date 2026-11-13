//! Charges: one standing Run per scope that supervises its area, woken by the area's events.
//!
//! M's charge (`m`) keeps the whole deployment from the Headquarters Station, an Operation's charge
//! (`operation:P`) its Operation from a Station the Operation allows, and a Station's charge
//! (`station:S`) that Station from itself. A charge is not a Mission: it has no brief, placement,
//! attempt, scorecard credit or board card, and no Sleeper queues it. `[charge.<scope>]` names the
//! Agent that keeps it and paces it ([`ChargeDefinition`]); the Station runs it.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use guatiao::{FromValue, Schema, ToValue};
use guatiao_intake::Form;

use crate::bounds::{Limit, Seconds, TimeLimit};

/// The scope of M's charge.
pub const M_SCOPE: &str = "m";
/// The prefix of an Operation's charge scope.
pub const OPERATION_PREFIX: &str = "operation:";
/// The prefix of a Station's charge scope.
pub const STATION_PREFIX: &str = "station:";

/// The area a charge keeps.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChargeScope {
    /// The whole deployment: M's charge, run by the Headquarters Station.
    M,
    /// One Operation, by codename.
    Operation(String),
    /// One Station, by ID.
    Station(String),
}

impl ChargeScope {
    /// The Operation the scope is, for an Operation's charge.
    pub fn operation(&self) -> Option<&str> {
        match self {
            Self::Operation(operation) => Some(operation),
            _ => None,
        }
    }

    /// The Station the scope is, for a Station's charge.
    pub fn station(&self) -> Option<&str> {
        match self {
            Self::Station(station) => Some(station),
            _ => None,
        }
    }
}

impl fmt::Display for ChargeScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::M => formatter.write_str(M_SCOPE),
            Self::Operation(operation) => write!(formatter, "{OPERATION_PREFIX}{operation}"),
            Self::Station(station) => write!(formatter, "{STATION_PREFIX}{station}"),
        }
    }
}

impl FromStr for ChargeScope {
    type Err = String;

    /// `m`, `operation:<codename>` or `station:<id>`; a name holds no whitespace, control or `:`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let name = |rest: &str, what: &str| {
            let valid = !rest.is_empty()
                && rest.len() <= 128
                && !rest
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || c == ':');
            valid
                .then(|| rest.to_owned())
                .ok_or_else(|| format!("charge scope {text:?} names no valid {what}"))
        };
        if text == M_SCOPE {
            Ok(Self::M)
        } else if let Some(rest) = text.strip_prefix(OPERATION_PREFIX) {
            name(rest, "Operation").map(Self::Operation)
        } else if let Some(rest) = text.strip_prefix(STATION_PREFIX) {
            name(rest, "Station").map(Self::Station)
        } else {
            Err(format!(
                "charge scope {text:?} is not m, operation:<codename> or station:<id>"
            ))
        }
    }
}

/// One charge's settings: the keys of a `[charge.<scope>]` table. Every bound can be lifted with
/// `none`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, ToValue, FromValue, Schema, Form)]
#[map(deny_unknown_fields)]
pub struct ChargeDefinition {
    /// The Agent that keeps the charge, by codename: its handler, model, effort, route and pool
    /// account run the charge's Run.
    #[schema(title = "Agent")]
    pub agent: String,
    /// For M's or a Station's charge, the Operation its Cables are sent and received in, under
    /// that Operation's Cable grants; without one, no Cable wakes it. An Operation's charge is in
    /// its own Operation, which this may only repeat.
    #[schema(title = "Cables in Operation", advanced)]
    pub operation: Option<String>,
    /// Seconds after its last wake that the charge is woken with nothing else to tell it; `none`
    /// wakes it only on events.
    #[schema(title = "Woken at least every")]
    #[map(default = ChargeDefinition::DEFAULTS.wake_every)]
    pub wake_every: TimeLimit,
    /// Turns the charge takes in any hour, each wake one turn; past it, events wait for the hour to
    /// make room.
    #[schema(title = "Turns an hour")]
    #[map(default = ChargeDefinition::DEFAULTS.max_turns_per_hour)]
    pub max_turns_per_hour: Limit,
    /// Events one turn is given; the rest wait for the next turn.
    #[schema(title = "Events one turn is given")]
    #[map(default = ChargeDefinition::DEFAULTS.max_events_per_turn)]
    pub max_events_per_turn: Limit,
    /// Events waiting for a turn; past it the oldest are dropped, and the charge's logbook says how
    /// many.
    #[schema(title = "Events waiting")]
    #[map(default = ChargeDefinition::DEFAULTS.max_waiting_events)]
    pub max_waiting_events: Limit,
    /// Seconds one turn may run before the Station interrupts it.
    #[schema(title = "Longest turn")]
    #[map(default = ChargeDefinition::DEFAULTS.turn_timeout)]
    pub turn_timeout: TimeLimit,
    /// Seconds the Station running the charge holds it without renewing before another Station may
    /// take it over.
    #[schema(title = "Held without renewing for", advanced)]
    #[map(default = ChargeDefinition::DEFAULTS.lease)]
    pub lease: Seconds,
}

impl ChargeDefinition {
    /// The settings a charge has unless its table says otherwise, its Agent unnamed.
    pub const DEFAULTS: Self = Self {
        agent: String::new(),
        operation: None,
        wake_every: TimeLimit::secs(3_600),
        max_turns_per_hour: Limit::at(30),
        max_events_per_turn: Limit::at(50),
        max_waiting_events: Limit::at(500),
        turn_timeout: TimeLimit::secs(1_800),
        lease: Seconds::new(60),
    };

    /// A charge kept by `agent`, every other setting at its default.
    pub fn new(agent: impl Into<String>) -> Self {
        Self {
            agent: agent.into(),
            ..Self::DEFAULTS
        }
    }

    /// Sets how long after its last wake the charge is woken with nothing else to tell it.
    #[must_use]
    pub fn wake_every(mut self, every: TimeLimit) -> Self {
        self.wake_every = every;
        self
    }

    /// Sets the turns it takes in any hour.
    #[must_use]
    pub fn max_turns_per_hour(mut self, turns: Limit) -> Self {
        self.max_turns_per_hour = turns;
        self
    }

    /// Sets how long its Station holds it without renewing.
    #[must_use]
    pub fn lease(mut self, lease: Duration) -> Self {
        self.lease = Seconds(lease);
        self
    }

    /// The Operation its Cables go in, for a charge of `scope`: an Operation's own, else the one
    /// its table names.
    pub fn cable_operation<'a>(&'a self, scope: &'a ChargeScope) -> Option<&'a str> {
        scope.operation().or(self.operation.as_deref())
    }

    /// Checks what no setting can mean for a charge of `scope`: no Agent, or an Operation's charge
    /// naming another Operation for its Cables.
    ///
    /// # Errors
    ///
    /// Names the setting.
    pub fn validate(&self, scope: &ChargeScope) -> Result<(), String> {
        if self.agent.trim().is_empty() {
            return Err("names no agent".to_owned());
        }
        if let (Some(own), Some(named)) = (scope.operation(), self.operation.as_deref())
            && own != named
        {
            return Err(format!(
                "is an Operation's charge, so its Cables are in {own:?}, not {named:?}"
            ));
        }
        Ok(())
    }
}

/// A charge on the roster: its scope and its settings.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Charge {
    /// What it keeps.
    pub scope: ChargeScope,
    /// How it is kept.
    pub settings: ChargeDefinition,
}

impl Charge {
    /// The charge of `scope` with `settings`.
    pub fn new(scope: ChargeScope, settings: ChargeDefinition) -> Self {
        Self { scope, settings }
    }
}

/// The charges of `records`, `[charge.<scope>]` tables as a record holds them, by scope. Each is
/// checked against the roster when the roster is built.
///
/// # Errors
///
/// Names the first scope that is not one, or the first key that is not acceptable.
pub fn charges_from_records(
    records: impl IntoIterator<Item = (String, serde_json::Value)>,
) -> Result<BTreeMap<String, Charge>, String> {
    let mut charges = BTreeMap::new();
    for (key, definition) in records {
        let scope: ChargeScope = key.parse()?;
        let settings: ChargeDefinition = crate::directorates::m::records::from_json(&definition)
            .map_err(|reason| format!("charge {key:?}: {reason}"))?;
        charges.insert(scope.to_string(), Charge::new(scope, settings));
    }
    Ok(charges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_reads_as_it_is_written() {
        for (text, scope) in [
            ("m", ChargeScope::M),
            ("operation:G", ChargeScope::Operation("G".to_owned())),
            ("station:hq-1", ChargeScope::Station("hq-1".to_owned())),
        ] {
            assert_eq!(text.parse::<ChargeScope>(), Ok(scope.clone()));
            assert_eq!(scope.to_string(), text);
        }
        for bad in [
            "",
            "M",
            "operation:",
            "station:a b",
            "operation:a:b",
            "theater:x",
        ] {
            assert!(bad.parse::<ChargeScope>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_operation_charge_keeps_its_cables_in_its_own_operation() {
        let scope = ChargeScope::Operation("G".to_owned());
        let mut settings = ChargeDefinition::new("00");
        assert_eq!(settings.cable_operation(&scope), Some("G"));
        settings.operation = Some("H".to_owned());
        assert!(settings.validate(&scope).is_err());
        let station = ChargeScope::Station("s".to_owned());
        assert!(settings.validate(&station).is_ok());
        assert_eq!(settings.cable_operation(&station), Some("H"));
        assert!(ChargeDefinition::new(" ").validate(&station).is_err());
    }
}
