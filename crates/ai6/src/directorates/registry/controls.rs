//! Controls: the operator's cancel, retry, resume and interrupt requests, queued for the
//! Station that acts on them.
//!
//! The gateway keeps the queue in memory for speed and writes every change here, so a gateway
//! that restarts takes up the queue where it stood: `controls/CONTROL_ID.json`.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::store::{FileRegistry, invalid_data, validate_registry_key};

/// One control and what became of it, as the gateway keeps it. Times are Unix seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRecord {
    /// Identity of the control.
    pub control_id: String,
    /// The Operation the requester was authorized for.
    pub operation: String,
    /// The Mission acted on.
    pub mission: String,
    /// The Run the Station was told to expect, when known.
    #[serde(default)]
    pub run: Option<String>,
    /// The Run the requester named, which a repeated request must name again.
    #[serde(default)]
    pub asked_run: Option<String>,
    /// `cancel`, `retry`, `resume`, `interrupt`, `drop` or `reload`.
    pub action: String,
    /// The Agent a retry is given to.
    #[serde(default)]
    pub agent: Option<String>,
    /// The message a resume carries.
    #[serde(default)]
    pub message: Option<String>,
    /// The Station asked to act.
    pub station: String,
    /// `queued`, `delivered`, `applied`, `rejected` or `expired`.
    pub status: String,
    /// The Mission's state the Station reported afterwards.
    #[serde(default)]
    pub state: Option<String>,
    /// Why the Station rejected it.
    #[serde(default)]
    pub reason: Option<String>,
    /// The person who asked.
    pub requested_by: String,
    /// When it was asked for.
    pub requested_at: f64,
    /// When it was applied, rejected or expired.
    #[serde(default)]
    pub finished_at: Option<f64>,
    /// The requester's idempotency key.
    pub idempotency_key: String,
    /// The requester as the gateway identifies them, the scope of the idempotency key.
    pub actor: String,
}

impl ControlRecord {
    /// Whether no Station has finished it: queued or delivered.
    pub fn is_open(&self) -> bool {
        is_open_status(&self.status)
    }

    /// Checks the record is storable.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a control id that is not a Registry key, or a time that is not
    /// finite.
    pub fn validate(&self) -> io::Result<()> {
        validate_registry_key(&self.control_id)?;
        if !self.requested_at.is_finite() || self.finished_at.is_some_and(|at| !at.is_finite()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a control needs finite times",
            ));
        }
        Ok(())
    }
}

pub(super) fn is_open_status(status: &str) -> bool {
    matches!(status, "queued" | "delivered")
}

/// Controls: the operator's requests of Stations, kept across a gateway restart.
pub trait ControlStore {
    /// Stores a control, replacing the one with the same `control_id`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a record that is not storable ([`ControlRecord::validate`]); I/O
    /// errors otherwise.
    fn put_control(&self, control: &ControlRecord) -> io::Result<()>;

    /// The controls `station` has not finished (`queued` or `delivered`), oldest first.
    /// Unreadable records are skipped.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn open_controls(&self, station: &str) -> io::Result<Vec<ControlRecord>>;

    /// Every control no Station has finished, oldest first.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn all_open_controls(&self) -> io::Result<Vec<ControlRecord>>;

    /// One control by id.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an id that is not a Registry key; I/O errors otherwise.
    fn control(&self, id: &str) -> io::Result<Option<ControlRecord>>;

    /// Forgets the controls finished before `finished_before` (Unix seconds) and returns how
    /// many.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn forget_controls(&self, finished_before: f64) -> io::Result<usize>;
}

pub(super) fn control_bytes(control: &ControlRecord) -> io::Result<Vec<u8>> {
    serde_json::to_vec(control).map_err(io::Error::other)
}

pub(super) fn parse_control(bytes: &[u8]) -> io::Result<ControlRecord> {
    let control: ControlRecord = serde_json::from_slice(bytes)
        .map_err(|error| invalid_data(format!("invalid control: {error}")))?;
    control
        .validate()
        .map_err(|error| invalid_data(error.to_string()))?;
    Ok(control)
}

pub(super) fn oldest_first(controls: &mut [ControlRecord]) {
    controls.sort_by(|left, right| {
        left.requested_at
            .total_cmp(&right.requested_at)
            .then_with(|| left.control_id.cmp(&right.control_id))
    });
}

impl FileRegistry {
    fn controls_root(&self) -> PathBuf {
        self.root.join("controls")
    }

    fn read_controls(&self) -> io::Result<Vec<ControlRecord>> {
        let entries = match fs::read_dir(self.controls_root()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut controls = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if let Ok(bytes) = fs::read(&path)
                && let Ok(control) = parse_control(&bytes)
                && path.file_stem().and_then(|stem| stem.to_str())
                    == Some(control.control_id.as_str())
            {
                controls.push(control);
            }
        }
        oldest_first(&mut controls);
        Ok(controls)
    }
}

impl ControlStore for FileRegistry {
    fn put_control(&self, control: &ControlRecord) -> io::Result<()> {
        control.validate()?;
        let bytes = control_bytes(control)?;
        let _lock = self.lock()?;
        self.write_atomic(
            &self
                .controls_root()
                .join(format!("{}.json", control.control_id)),
            &bytes,
        )
    }

    fn open_controls(&self, station: &str) -> io::Result<Vec<ControlRecord>> {
        let mut controls = self.read_controls()?;
        controls.retain(|control| control.is_open() && control.station == station);
        Ok(controls)
    }

    fn all_open_controls(&self) -> io::Result<Vec<ControlRecord>> {
        let mut controls = self.read_controls()?;
        controls.retain(ControlRecord::is_open);
        Ok(controls)
    }

    fn control(&self, id: &str) -> io::Result<Option<ControlRecord>> {
        validate_registry_key(id)?;
        match fs::read(self.controls_root().join(format!("{id}.json"))) {
            Ok(bytes) => parse_control(&bytes).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn forget_controls(&self, finished_before: f64) -> io::Result<usize> {
        let _lock = self.lock()?;
        let mut forgotten = 0;
        for control in self.read_controls()? {
            if control.finished_at.is_some_and(|at| at < finished_before) {
                fs::remove_file(
                    self.controls_root()
                        .join(format!("{}.json", control.control_id)),
                )?;
                forgotten += 1;
            }
        }
        Ok(forgotten)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_control_is_kept_until_finished_and_then_forgotten,
        an_unreadable_control_is_refused_by_the_parser
    );

    fn control(id: &str, station: &str, status: &str, at: f64) -> ControlRecord {
        ControlRecord {
            control_id: id.to_owned(),
            operation: "G".to_owned(),
            mission: "m-1".to_owned(),
            run: Some("run-1".to_owned()),
            asked_run: None,
            action: "cancel".to_owned(),
            agent: None,
            message: None,
            station: station.to_owned(),
            status: status.to_owned(),
            state: None,
            reason: None,
            requested_by: "ada".to_owned(),
            requested_at: at,
            finished_at: (!is_open_status(status)).then_some(at + 1.0),
            idempotency_key: format!("key-{id}"),
            actor: "person:ada".to_owned(),
        }
    }

    fn a_control_is_kept_until_finished_and_then_forgotten(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.all_open_controls().unwrap().is_empty());
        registry
            .put_control(&control("c2", "worker", "queued", 20.0))
            .unwrap();
        registry
            .put_control(&control("c1", "worker", "delivered", 10.0))
            .unwrap();
        registry
            .put_control(&control("c3", "other", "queued", 30.0))
            .unwrap();
        registry
            .put_control(&control("c4", "worker", "applied", 5.0))
            .unwrap();

        let ids = |controls: Vec<ControlRecord>| {
            controls
                .into_iter()
                .map(|control| control.control_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(registry.open_controls("worker").unwrap()), ["c1", "c2"]);
        assert_eq!(
            ids(registry.all_open_controls().unwrap()),
            ["c1", "c2", "c3"]
        );
        // The same id again replaces the record.
        registry
            .put_control(&control("c2", "worker", "applied", 20.0))
            .unwrap();
        assert_eq!(registry.control("c2").unwrap().unwrap().status, "applied");
        assert_eq!(ids(registry.open_controls("worker").unwrap()), ["c1"]);
        assert_eq!(registry.control("nope").unwrap(), None);

        assert_eq!(registry.forget_controls(22.0).unwrap(), 2);
        assert!(registry.control("c4").unwrap().is_none());
        assert!(registry.control("c1").unwrap().is_some(), "still open");
        assert!(
            registry
                .put_control(&control("../x", "w", "queued", 1.0))
                .is_err()
        );
    }

    fn an_unreadable_control_is_refused_by_the_parser(_store: &dyn Store) {
        assert!(parse_control(b"{").is_err());
        assert!(parse_control(br#"{"control_id":"x","unknown":1}"#).is_err());
    }
}
