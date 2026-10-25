//! Q's credentials for a Session: the Agent's loadout secrets, leased per Session from the
//! Station's secret backend and handed to the harness in its environment only.
//!
//! Nothing is written anywhere: a value lives in the Station's memory and the harness's
//! environment. Leases are revoked when the Session's Mission ends or its Session stops on
//! this Station. A `green_ink` secret is leased only while the Mission holds a live green ink
//! approval of a kind it names, once per approval, for no longer than the approval lasts.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::directorates::m::mission::Mission;
use crate::directorates::medical::exposure::{ExposureGate, Restriction};
use crate::directorates::q::{
    LoadoutCatalog, MAX_SECRET_LEASE_TTL, SecretBackend, SecretBroker, SecretLease,
    SecretLeaseRequest, StationSecrets,
};
use crate::directorates::registry::approvals::{Approval, ApprovalStatus};
use crate::directorates::registry::timeline::TimelineEvent;

use super::DaemonState;

/// The timeline kind of a credential event.
const TIMELINE_KIND: &str = "credential";
/// Longest list of secret names a timeline event carries.
const MAX_NAMES_DETAIL: usize = 240;

/// The Station's loadouts and secret backend, and the leases it has issued, by Mission.
#[derive(Clone, Default)]
pub(crate) struct CredentialDesk {
    catalog: Option<Arc<LoadoutCatalog>>,
    backend: Option<Arc<dyn SecretBackend>>,
    issued: Arc<Mutex<BTreeMap<String, Vec<SecretLease>>>>,
    spent: Arc<Mutex<BTreeSet<String>>>,
}

impl std::fmt::Debug for CredentialDesk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialDesk")
            .field("catalog", &self.catalog)
            .finish_non_exhaustive()
    }
}

impl PartialEq for CredentialDesk {
    fn eq(&self, other: &Self) -> bool {
        self.catalog == other.catalog
            && Arc::ptr_eq(&self.issued, &other.issued)
            && match (&self.backend, &other.backend) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Eq for CredentialDesk {}

impl CredentialDesk {
    pub(crate) fn with_catalog(mut self, catalog: LoadoutCatalog) -> Self {
        self.catalog = Some(Arc::new(catalog));
        self
    }

    pub(crate) fn with_backend(mut self, backend: Arc<dyn SecretBackend>) -> Self {
        self.backend = Some(backend);
        self
    }

    pub(crate) fn has_catalog(&self) -> bool {
        self.catalog.is_some()
    }

    /// The names of the secrets leased to `mission_id`'s Session now.
    #[cfg(test)]
    pub(crate) fn issued_for(&self, mission_id: &str) -> Vec<String> {
        self.issued
            .lock()
            .unwrap()
            .get(mission_id)
            .into_iter()
            .flatten()
            .flat_map(SecretLease::names)
            .collect()
    }

    /// The backend leases come from: the one set in code, else the Station's own.
    fn broker(&self, catalog: &LoadoutCatalog) -> SecretBroker {
        let backend: Arc<dyn SecretBackend> = match &self.backend {
            Some(backend) => backend.clone(),
            None => Arc::new(StationSecrets::new(catalog.secrets())),
        };
        SecretBroker::new(Shared(backend))
    }
}

/// A shared backend handed to a broker that wants one of its own.
struct Shared(Arc<dyn SecretBackend>);

impl SecretBackend for Shared {
    fn acquire(
        &self,
        request: &crate::directorates::q::SecretRequest,
        names: &[String],
    ) -> Result<
        BTreeMap<String, crate::directorates::q::SecretValue>,
        crate::directorates::q::SecretBackendFailure,
    > {
        self.0.acquire(request, names)
    }

    fn revoke(&self, lease_id: &str) -> Result<(), crate::directorates::q::SecretBackendFailure> {
        self.0.revoke(lease_id)
    }
}

/// Leases the Agent's loadout secrets for a Session of `mission` and puts each value in
/// `environment` under its variable, replacing what the Mission held before. An Agent
/// without a loadout gets nothing; an exposed Session whose Operation restricts `secrets`
/// gets no new leases.
///
/// Fails when the backend refuses an ordinary secret, so a Session never starts without the
/// credentials its loadout promises. Blocks on the backend and the Registry.
pub(super) fn issue(
    state: &DaemonState,
    mission: &Mission,
    agent: &str,
    exposure: &ExposureGate,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let desk = &state.station.credentials;
    let mission_id = mission.mission_id.to_string();
    release(state, &mission_id);
    let Some(catalog) = &desk.catalog else {
        return Ok(());
    };
    let Ok(loadout) = catalog.for_agent(agent) else {
        return Ok(());
    };
    if loadout.secret_names().is_empty() {
        return Ok(());
    }
    // What an earlier Session of the Mission held goes, so only this issue's leases remain.
    for name in loadout.secret_names() {
        environment.remove(catalog.secret(name).env());
    }
    if exposure.restricts(Restriction::Secrets) {
        eprintln!(
            "ai6: Mission {mission_id:?} read untrusted input and its Operation restricts \
             secrets, so its Session gets no credentials"
        );
        return Ok(());
    }
    let broker = desk.broker(catalog);
    let (ordinary, protected): (Vec<_>, Vec<_>) = loadout
        .secret_names()
        .iter()
        .map(|name| catalog.secret(name))
        .partition(|definition| !definition.green_ink());
    let mut leases = Vec::new();
    let mut leased = Vec::new();

    if !ordinary.is_empty() {
        let names: Vec<String> = ordinary.iter().map(|d| d.name().to_owned()).collect();
        let request = SecretLeaseRequest::with_ttl(
            state.station_id.clone(),
            mission_id.clone(),
            MAX_SECRET_LEASE_TTL,
        )
        .map_err(|error| error.to_string())?;
        let lease = broker
            .lease_names(loadout, &names, request)
            .map_err(|error| {
                format!("Agent {agent:?}'s credentials could not be leased: {error}")
            })?;
        for definition in &ordinary {
            lease
                .with_secret(definition.name(), |value| {
                    environment.insert(definition.env().to_owned(), value.to_owned())
                })
                .map_err(|error| error.to_string())?;
        }
        leased.extend(names);
        leases.push(lease);
    }

    let mut approvals = Vec::new();
    if !protected.is_empty() {
        let approved = live_approvals(state, mission);
        let mut spent = desk
            .spent
            .lock()
            .map_err(|_| "credential lock was poisoned")?;
        for definition in &protected {
            let Some(approval) = approved.iter().find(|approval| {
                !spent.contains(&approval.request.id)
                    && (definition.kinds().is_empty()
                        || definition.kinds().contains(&approval.request.scope.kind))
            }) else {
                continue;
            };
            let now = state.clock.now() as f64;
            let remaining = approval.expires_at.unwrap_or(now) - now;
            let ttl = Duration::from_secs_f64(remaining.max(1.0)).min(MAX_SECRET_LEASE_TTL);
            let names = vec![definition.name().to_owned()];
            let lease =
                SecretLeaseRequest::with_ttl(state.station_id.clone(), mission_id.clone(), ttl)
                    .and_then(|request| broker.lease_names(loadout, &names, request));
            let lease = match lease {
                Ok(lease) => lease,
                Err(error) => {
                    eprintln!(
                        "ai6: green ink credential {:?} for Mission {mission_id:?} was not leased: {error}",
                        definition.name()
                    );
                    continue;
                }
            };
            lease
                .with_secret(definition.name(), |value| {
                    environment.insert(definition.env().to_owned(), value.to_owned())
                })
                .map_err(|error| error.to_string())?;
            spent.insert(approval.request.id.clone());
            approvals.push(approval.request.id.clone());
            leased.push(definition.name().to_owned());
            leases.push(lease);
        }
    }

    if !leases.is_empty() {
        record(state, mission, agent, &leased, &approvals);
        desk.issued
            .lock()
            .map_err(|_| "credential lock was poisoned")?
            .insert(mission_id, leases);
    }
    Ok(())
}

/// Revokes every lease issued for `mission_id`, once its Session stops on this Station or the
/// Mission ends.
pub(super) fn release(state: &DaemonState, mission_id: &str) {
    let leases = state
        .station
        .credentials
        .issued
        .lock()
        .ok()
        .and_then(|mut issued| issued.remove(mission_id));
    for lease in leases.into_iter().flatten() {
        if let Err(error) = lease.revoke() {
            eprintln!("ai6: a credential lease of Mission {mission_id:?} was not revoked: {error}");
        }
    }
}

/// The Mission's approvals that are live now.
fn live_approvals(state: &DaemonState, mission: &Mission) -> Vec<Approval> {
    let Some(operation) = mission.operation_id.as_deref() else {
        return Vec::new();
    };
    let now = state.clock.now() as f64;
    match state
        .intake
        .mission_approvals(operation, mission.mission_id.as_str(), now)
    {
        Ok(statuses) => statuses
            .into_iter()
            .filter_map(|status| match status {
                ApprovalStatus::Decided(approval) if approval.is_live(now) => Some(approval),
                _ => None,
            })
            .collect(),
        Err(error) => {
            eprintln!(
                "ai6: green ink approvals of Mission {:?} unreadable: {error}",
                mission.mission_id.as_str()
            );
            Vec::new()
        }
    }
}

/// Records which credentials a Mission's Session was issued: names, never values.
fn record(
    state: &DaemonState,
    mission: &Mission,
    agent: &str,
    names: &[String],
    approvals: &[String],
) {
    let Some(operation) = mission.operation_id.as_deref() else {
        return;
    };
    let mut listed = String::new();
    for name in names {
        if listed.len() + name.len() + 1 > MAX_NAMES_DETAIL {
            break;
        }
        if !listed.is_empty() {
            listed.push(',');
        }
        listed.push_str(name);
    }
    let mut event =
        TimelineEvent::new(TIMELINE_KIND, "leased", operation, state.clock.now() as f64)
            .station(state.station_id.clone())
            .mission(mission.mission_id.to_string())
            .actor(agent)
            .detail("secrets", listed);
    if let Some(approval) = approvals.first() {
        event = event.detail("approval", approval.clone());
    }
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: credential lease was not recorded on the timeline: {error}");
    }
}
