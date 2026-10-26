//! Clearance grants: an Agent's clearance in one Operation, set apart from its configured one.
//!
//! A grant that names the green ink request it came from raises the Agent to its clearance
//! (never past the Operation's); one without lowers it, and only lowers it: the Agent works at
//! the lower of its configured clearance and the grant's. One grant per Agent and Operation;
//! removing it returns the Agent to its configured clearance. Every Station reads the same
//! records: `clearance-grants/OPERATION/AGENT.json`.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::store::{
    FileRegistry, invalid_data, registry_lock, safe_component, validate_registry_key, write_atomic,
};
use crate::directorates::m::roster::CLEARANCES;

pub(super) const MAX_RECORD_BYTES: u64 = 16 * 1024;
pub(super) const MAX_GRANTS: usize = 10_000;
const MAX_TEXT_BYTES: usize = 512;

/// An Agent's clearance in one Operation, raised by green ink or lowered by a person.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClearanceGrant {
    /// The Agent codename.
    pub agent: String,
    /// The Operation it applies to.
    pub operation: String,
    /// `desk`, `field` or `00`.
    pub clearance: String,
    /// Who set it: the person who signed the green ink or lowered the clearance, or
    /// `station:ID` for a Station.
    pub signer: String,
    /// When, as Unix seconds.
    pub at: f64,
    /// When it lapses, as Unix seconds; `None` keeps it until it is removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<f64>,
    /// The green ink request that raised it; `None` for a grant that only lowers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
    /// Why, in the signer's words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ClearanceGrant {
    /// A grant raising `agent` to `clearance` in `operation`, signed through the green ink
    /// request `approval`.
    pub fn raise(
        agent: impl Into<String>,
        operation: impl Into<String>,
        clearance: impl Into<String>,
        approval: impl Into<String>,
        signer: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            agent: agent.into(),
            operation: operation.into(),
            clearance: clearance.into(),
            signer: signer.into(),
            at,
            expires_at: None,
            approval: Some(approval.into()),
            reason: None,
        }
    }

    /// A grant that holds `agent` at `clearance` or below in `operation`.
    pub fn lower(
        agent: impl Into<String>,
        operation: impl Into<String>,
        clearance: impl Into<String>,
        signer: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            agent: agent.into(),
            operation: operation.into(),
            clearance: clearance.into(),
            signer: signer.into(),
            at,
            expires_at: None,
            approval: None,
            reason: None,
        }
    }

    /// Sets when it lapses.
    #[must_use]
    pub fn expiring(mut self, expires_at: f64) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Sets why.
    #[must_use]
    pub fn because(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Whether it may raise the Agent above its configured clearance: only a grant signed
    /// through green ink does.
    pub fn raises(&self) -> bool {
        self.approval.is_some()
    }

    /// Whether it applies at `now`.
    pub fn is_live(&self, now: f64) -> bool {
        self.expires_at.is_none_or(|expires| expires > now)
    }

    /// Checks the grant's bounds.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an Agent or Operation that is not a Registry key, an unknown
    /// clearance, an empty or oversized signer, approval or reason, or a time that is not
    /// finite and positive or an expiry before it.
    pub fn validate(&self) -> io::Result<()> {
        validate_registry_key(&self.agent)?;
        validate_registry_key(&self.operation)?;
        let bounded = |text: &str| {
            !text.trim().is_empty()
                && text.len() <= MAX_TEXT_BYTES
                && !text.contains(char::is_control)
        };
        if !CLEARANCES.contains(&self.clearance.as_str())
            || !bounded(&self.signer)
            || self.approval.as_deref().is_some_and(|id| !bounded(id))
            || self
                .reason
                .as_deref()
                .is_some_and(|reason| !bounded(reason))
            || !self.at.is_finite()
            || self.at <= 0.0
            || self
                .expires_at
                .is_some_and(|expires| !expires.is_finite() || expires <= self.at)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a clearance grant needs a known clearance, a signer and a time, each in bounds",
            ));
        }
        Ok(())
    }
}

/// Clearance grants: each Agent's clearance per Operation when it is not the configured one.
pub trait ClearanceStore {
    /// Stores a grant, replacing the Agent's grant in that Operation.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a grant out of bounds ([`ClearanceGrant::validate`]); `InvalidData`
    /// past 10,000 grants; I/O errors otherwise.
    fn put_clearance_grant(&self, grant: &ClearanceGrant) -> io::Result<()>;

    /// The Agent's grant in `operation`, live or lapsed.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a key that is not a Registry key; `InvalidData` for an unreadable
    /// grant; I/O errors otherwise.
    fn clearance_grant(&self, operation: &str, agent: &str) -> io::Result<Option<ClearanceGrant>>;

    /// Every grant in `operation`, by Agent. Unreadable grants are skipped.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an Operation that is not a Registry key; I/O errors otherwise.
    fn clearance_grants(&self, operation: &str) -> io::Result<Vec<ClearanceGrant>>;

    /// Removes the Agent's grant in `operation` and returns it, or `None` when it had none.
    ///
    /// # Errors
    ///
    /// As [`ClearanceStore::clearance_grant`].
    fn remove_clearance_grant(
        &self,
        operation: &str,
        agent: &str,
    ) -> io::Result<Option<ClearanceGrant>>;
}

impl FileRegistry {
    fn grants_root(&self) -> PathBuf {
        self.root.join("clearance-grants")
    }

    fn grant_path(&self, operation: &str, agent: &str) -> PathBuf {
        self.grants_root()
            .join(operation)
            .join(format!("{}.json", safe_component(agent)))
    }
}

impl ClearanceStore for FileRegistry {
    fn put_clearance_grant(&self, grant: &ClearanceGrant) -> io::Result<()> {
        grant.validate()?;
        let bytes = grant_bytes(grant)?;
        let _lock = registry_lock(&self.root)?;
        let path = self.grant_path(&grant.operation, &grant.agent);
        if !path.exists() && count_files(&self.grants_root())? >= MAX_GRANTS {
            return Err(invalid_data("too many clearance grants"));
        }
        write_atomic(&path, &bytes)
    }

    fn clearance_grant(&self, operation: &str, agent: &str) -> io::Result<Option<ClearanceGrant>> {
        validate_registry_key(operation)?;
        validate_registry_key(agent)?;
        read_grant(&self.grant_path(operation, agent), operation, agent)
    }

    fn clearance_grants(&self, operation: &str) -> io::Result<Vec<ClearanceGrant>> {
        validate_registry_key(operation)?;
        let entries = match fs::read_dir(self.grants_root().join(operation)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut grants = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if let Ok(bytes) = fs::read(&path)
                && let Ok(grant) = parse_grant(&bytes)
                && grant.operation == operation
                && path.file_stem().and_then(|stem| stem.to_str())
                    == Some(safe_component(&grant.agent).as_str())
            {
                grants.push(grant);
            }
        }
        grants.sort_by(|left, right| left.agent.cmp(&right.agent));
        Ok(grants)
    }

    fn remove_clearance_grant(
        &self,
        operation: &str,
        agent: &str,
    ) -> io::Result<Option<ClearanceGrant>> {
        validate_registry_key(operation)?;
        validate_registry_key(agent)?;
        let _lock = registry_lock(&self.root)?;
        let path = self.grant_path(operation, agent);
        let Some(grant) = read_grant(&path, operation, agent)? else {
            return Ok(None);
        };
        fs::remove_file(&path)?;
        Ok(Some(grant))
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

/// The stored form of a grant, refused past the size limit.
pub(super) fn grant_bytes(grant: &ClearanceGrant) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(grant).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid_data("the clearance grant exceeds the size limit"));
    }
    Ok(bytes)
}

/// A stored grant, checked against the size limit and its bounds.
pub(super) fn parse_grant(bytes: &[u8]) -> io::Result<ClearanceGrant> {
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(invalid_data("the clearance grant exceeds the size limit"));
    }
    let grant: ClearanceGrant = serde_json::from_slice(bytes)
        .map_err(|error| invalid_data(format!("invalid clearance grant: {error}")))?;
    grant
        .validate()
        .map_err(|error| invalid_data(error.to_string()))?;
    Ok(grant)
}

fn read_grant(
    path: &std::path::Path,
    operation: &str,
    agent: &str,
) -> io::Result<Option<ClearanceGrant>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let grant = parse_grant(&bytes)?;
    Ok((grant.operation == operation && grant.agent == agent).then_some(grant))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(a_grant_is_kept_per_agent_and_operation_and_removed);

    fn a_grant_is_kept_per_agent_and_operation_and_removed(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(registry.clearance_grant("G", "007").unwrap(), None);
        let raised = ClearanceGrant::raise("007", "G", "field", "green-ink-1", "m", 10.0);
        registry.put_clearance_grant(&raised).unwrap();
        let lowered = ClearanceGrant::lower("008", "G", "desk", "ops", 11.0)
            .because("review")
            .expiring(100.0);
        registry.put_clearance_grant(&lowered).unwrap();
        registry
            .put_clearance_grant(&ClearanceGrant::lower("007", "L", "desk", "ops", 12.0))
            .unwrap();
        assert_eq!(
            registry.clearance_grant("G", "007").unwrap(),
            Some(raised.clone())
        );
        assert!(raised.raises() && !lowered.raises());
        assert!(lowered.is_live(99.0) && !lowered.is_live(100.0));
        let agents: Vec<_> = registry
            .clearance_grants("G")
            .unwrap()
            .into_iter()
            .map(|grant| grant.agent)
            .collect();
        assert_eq!(agents, ["007", "008"]);

        let replaced = ClearanceGrant::lower("007", "G", "desk", "ops", 13.0);
        registry.put_clearance_grant(&replaced).unwrap();
        assert_eq!(
            registry.remove_clearance_grant("G", "007").unwrap(),
            Some(replaced)
        );
        assert_eq!(registry.remove_clearance_grant("G", "007").unwrap(), None);
        assert_eq!(registry.clearance_grants("L").unwrap().len(), 1);

        for bad in [
            ClearanceGrant::lower("007", "G", "cosmic", "ops", 1.0),
            ClearanceGrant::lower("007", "G", "desk", " ", 1.0),
            ClearanceGrant::lower("0/7", "G", "desk", "ops", 1.0),
            ClearanceGrant::lower("007", "G", "desk", "ops", 5.0).expiring(5.0),
        ] {
            assert!(registry.put_clearance_grant(&bad).is_err(), "{bad:?}");
        }
    }
}
