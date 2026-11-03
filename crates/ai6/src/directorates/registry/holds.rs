//! Holds: an Agent, or one route of an Agent, taken out of use in an Operation.
//!
//! A hold only stops new work: the Agent is passed over by selection, or the route is not
//! chosen, until a person lifts it. A Session already running is left to finish. M files one
//! when a person signs its proposal, or by itself where the Operation lets it (`auto_hold`), and
//! says why. It never widens anything. Every Station reads the same records:
//! `holds/OPERATION/AGENT--ROUTE.json`.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::limits::HoldLimits;
use super::store::{FileRegistry, hex_digest, invalid_data, safe_component, validate_registry_key};
use crate::directorates::m::roster::RouteRef;

/// The `by` of a hold M put on by itself.
pub const AUTOMATIC: &str = "m";

/// An Agent, or one of its routes, out of use in one Operation.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RosterHold {
    /// The Operation it applies to.
    pub operation: String,
    /// The Agent codename.
    pub agent: String,
    /// The route held; `None` holds the whole Agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<RouteRef>,
    /// Why, with the numbers it rests on.
    pub reason: String,
    /// Who put it on: a person, `m` when M did by itself, or the person who signed M's proposal.
    pub by: String,
    /// When, as Unix seconds.
    pub at: f64,
    /// The green ink request M's proposal was filed under, when a person signed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
}

impl RosterHold {
    /// A hold on `agent` (or one `route` of it) in `operation`.
    pub fn new(
        operation: impl Into<String>,
        agent: impl Into<String>,
        route: Option<RouteRef>,
        reason: impl Into<String>,
        by: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            operation: operation.into(),
            agent: agent.into(),
            route,
            reason: reason.into(),
            by: by.into(),
            at,
            approval: None,
        }
    }

    /// Names the green ink request the hold was signed through.
    #[must_use]
    pub fn signed_through(mut self, approval: impl Into<String>) -> Self {
        self.approval = Some(approval.into());
        self
    }

    /// Whether it holds the whole Agent rather than one route.
    pub fn holds_agent(&self) -> bool {
        self.route.is_none()
    }

    /// Whether it keeps `route` of its Agent out of use: the whole Agent, or that route.
    pub fn covers(&self, route: &RouteRef) -> bool {
        self.route.as_ref().is_none_or(|held| held == route)
    }

    /// The key of the route held, the same for the same route: `-` for the whole Agent.
    pub fn route_key(route: Option<&RouteRef>) -> String {
        match route {
            None => "-".to_owned(),
            Some(route) => {
                let mut digest = Sha256::new();
                for part in [
                    route.handler.as_str(),
                    route.model.as_deref().unwrap_or_default(),
                    route.pool.as_deref().unwrap_or_default(),
                ] {
                    digest.update((part.len() as u64).to_be_bytes());
                    digest.update(part.as_bytes());
                }
                hex_digest(&digest.finalize())[..16].to_owned()
            }
        }
    }

    /// Checks the hold's bounds.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an Agent or Operation that is not a Registry key, an empty or
    /// oversized reason, holder or route name (past `limits.text_bytes`), or a time that is not
    /// finite and positive.
    pub fn validate(&self, limits: &HoldLimits) -> io::Result<()> {
        validate_registry_key(&self.agent)?;
        validate_registry_key(&self.operation)?;
        let bounded = |text: &str| {
            !text.trim().is_empty()
                && limits.text_bytes.allows(text.len())
                && !text.contains(char::is_control)
        };
        if !bounded(&self.reason)
            || !bounded(&self.by)
            || self.approval.as_deref().is_some_and(|id| !bounded(id))
            || self.route.as_ref().is_some_and(|route| {
                !bounded(&route.handler)
                    || route.model.as_deref().is_some_and(|model| !bounded(model))
                    || route.pool.as_deref().is_some_and(|pool| !bounded(pool))
            })
            || !self.at.is_finite()
            || self.at <= 0.0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a hold needs a reason, a holder and a time, each in bounds",
            ));
        }
        Ok(())
    }
}

/// Holds: the Agents and routes taken out of use per Operation.
pub trait HoldStore {
    /// Stores a hold, replacing the one on the same Agent or route.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a hold out of bounds ([`RosterHold::validate`]); `InvalidData` past
    /// `[registry.limits.holds] holds`; I/O errors otherwise.
    fn put_hold(&self, hold: &RosterHold) -> io::Result<()>;

    /// Every hold in `operation`, by Agent then route. Unreadable holds are skipped.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an Operation that is not a Registry key; I/O errors otherwise.
    fn holds(&self, operation: &str) -> io::Result<Vec<RosterHold>>;

    /// Lifts the hold on `agent` (or one `route` of it) in `operation` and returns it, or
    /// `None` when there was none.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a key that is not a Registry key; I/O errors otherwise.
    fn lift_hold(
        &self,
        operation: &str,
        agent: &str,
        route: Option<&RouteRef>,
    ) -> io::Result<Option<RosterHold>>;
}

impl FileRegistry {
    fn holds_root(&self) -> PathBuf {
        self.root.join("holds")
    }

    fn hold_path(&self, operation: &str, agent: &str, route: Option<&RouteRef>) -> PathBuf {
        self.holds_root().join(operation).join(format!(
            "{}--{}.json",
            safe_component(agent),
            RosterHold::route_key(route)
        ))
    }
}

impl HoldStore for FileRegistry {
    fn put_hold(&self, hold: &RosterHold) -> io::Result<()> {
        let limits = &self.limits.holds;
        hold.validate(limits)?;
        let bytes = hold_bytes(hold, limits)?;
        let _lock = self.lock()?;
        let path = self.hold_path(&hold.operation, &hold.agent, hold.route.as_ref());
        if !path.exists() && !limits.holds.allows(count_files(&self.holds_root())? + 1) {
            return Err(invalid_data("too many holds"));
        }
        self.write_atomic(&path, &bytes)
    }

    fn holds(&self, operation: &str) -> io::Result<Vec<RosterHold>> {
        validate_registry_key(operation)?;
        let entries = match fs::read_dir(self.holds_root().join(operation)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut holds = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if let Ok(bytes) = fs::read(&path)
                && let Ok(hold) = parse_hold(&bytes, &self.limits.holds)
                && hold.operation == operation
                && path.file_stem().and_then(|stem| stem.to_str())
                    == Some(
                        format!(
                            "{}--{}",
                            safe_component(&hold.agent),
                            RosterHold::route_key(hold.route.as_ref())
                        )
                        .as_str(),
                    )
            {
                holds.push(hold);
            }
        }
        holds.sort_by(|left, right| {
            left.agent.cmp(&right.agent).then_with(|| {
                RosterHold::route_key(left.route.as_ref())
                    .cmp(&RosterHold::route_key(right.route.as_ref()))
            })
        });
        Ok(holds)
    }

    fn lift_hold(
        &self,
        operation: &str,
        agent: &str,
        route: Option<&RouteRef>,
    ) -> io::Result<Option<RosterHold>> {
        validate_registry_key(operation)?;
        validate_registry_key(agent)?;
        let _lock = self.lock()?;
        let path = self.hold_path(operation, agent, route);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let hold = parse_hold(&bytes, &self.limits.holds)?;
        fs::remove_file(&path)?;
        Ok(Some(hold))
    }
}

fn count_files(root: &std::path::Path) -> io::Result<usize> {
    let operations = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut count = 0;
    for operation in operations {
        if let Ok(entries) = fs::read_dir(operation?.path()) {
            count += entries.count();
        }
    }
    Ok(count)
}

/// The stored form of a hold, refused past the size limit.
pub(super) fn hold_bytes(hold: &RosterHold, limits: &HoldLimits) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(hold).map_err(io::Error::other)?;
    if !limits.record_bytes.allows(bytes.len()) {
        return Err(invalid_data("the hold exceeds the size limit"));
    }
    Ok(bytes)
}

/// A stored hold, checked against the size limit and its bounds.
pub(super) fn parse_hold(bytes: &[u8], limits: &HoldLimits) -> io::Result<RosterHold> {
    if !limits.record_bytes.allows(bytes.len()) {
        return Err(invalid_data("the hold exceeds the size limit"));
    }
    let hold: RosterHold = serde_json::from_slice(bytes)
        .map_err(|error| invalid_data(format!("invalid hold: {error}")))?;
    hold.validate(limits)
        .map_err(|error| invalid_data(error.to_string()))?;
    Ok(hold)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(a_hold_is_kept_per_agent_and_route_and_lifted);

    fn a_hold_is_kept_per_agent_and_route_and_lifted(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.holds("G").unwrap().is_empty());
        let route = RouteRef::new("claude", Some("opus".to_owned()), Some("team".to_owned()));
        let other = RouteRef::new("claude", Some("opus".to_owned()), None);
        registry
            .put_hold(&RosterHold::new("G", "007", None, "failing", "m", 10.0))
            .unwrap();
        registry
            .put_hold(&RosterHold::new(
                "G",
                "007",
                Some(route.clone()),
                "slow",
                "alice",
                11.0,
            ))
            .unwrap();
        registry
            .put_hold(&RosterHold::new("L", "007", None, "elsewhere", "m", 12.0))
            .unwrap();
        // The same Agent or route again replaces its hold.
        registry
            .put_hold(&RosterHold::new(
                "G",
                "007",
                None,
                "still failing",
                "m",
                13.0,
            ))
            .unwrap();

        let held = registry.holds("G").unwrap();
        assert_eq!(held.len(), 2, "one per Agent and route, none of L's");
        let whole = held.iter().find(|hold| hold.holds_agent()).unwrap();
        assert_eq!(
            (whole.reason.as_str(), whole.by.as_str()),
            ("still failing", "m")
        );
        assert!(whole.covers(&route) && whole.covers(&other));
        let one = held.iter().find(|hold| !hold.holds_agent()).unwrap();
        assert!(one.covers(&route) && !one.covers(&other));
        assert_eq!(registry.holds("L").unwrap().len(), 1);

        assert_eq!(registry.lift_hold("G", "008", None).unwrap(), None);
        assert_eq!(
            registry
                .lift_hold("G", "007", Some(&route))
                .unwrap()
                .unwrap()
                .by,
            "alice"
        );
        assert_eq!(registry.holds("G").unwrap().len(), 1, "the Agent's stays");
        assert!(registry.lift_hold("G", "007", None).unwrap().is_some());
        assert!(registry.holds("G").unwrap().is_empty());

        for bad in [
            RosterHold::new("G", "007", None, "", "m", 1.0),
            RosterHold::new("G", "007", None, "why", "", 1.0),
            RosterHold::new("G", "007", None, "why", "m", f64::NAN),
            RosterHold::new("G/../x", "007", None, "why", "m", 1.0),
        ] {
            assert!(registry.put_hold(&bad).is_err(), "{bad:?}");
        }
    }
}
