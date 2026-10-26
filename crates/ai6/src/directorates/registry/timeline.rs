//! The activity timeline: durable, metadata-only events correlated by Station, Operation,
//! Mission and Session.
//!
//! It records what the audit log does not: lifecycle transitions, the outcomes of operator
//! controls, Cables sent and Sleeper authorizations. An event never carries a prompt, a
//! message, a Cable body, terminal bytes or a secret; only identities, names and states.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::store::{FileRegistry, invalid_data, registry_lock, write_atomic};

/// Events kept; writing past this drops the oldest.
pub const MAX_TIMELINE_EVENTS: usize = 10_000;
const MAX_EVENT_BYTES: u64 = 8_192;
const MAX_ID_BYTES: usize = 128;
const MAX_NAME_BYTES: usize = 64;
const MAX_TEXT_BYTES: usize = 256;
const MAX_DETAIL_ENTRIES: usize = 8;

/// One durable activity event.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineEvent {
    /// Unique file-safe identity.
    pub id: String,
    /// When it happened, as Unix seconds.
    pub at: f64,
    /// What sort of event: `lifecycle`, `control`, `cable` or `sleeper`.
    pub kind: String,
    /// What happened, as a bounded lowercase name: a Mission state for `lifecycle`,
    /// `<action>.<outcome>` for `control`, `sent` for `cable`, `authorized` or `revoked`
    /// for `sleeper`.
    pub action: String,
    /// The Operation whose timeline shows the event.
    pub operation: String,
    /// The Station involved, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    /// The Mission involved, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<String>,
    /// The provider Session involved, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Who acted: a person ID or an Agent codename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Further safe identifiers and names, at most 8.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub detail: BTreeMap<String, String>,
}

impl TimelineEvent {
    /// An event with a fresh ID and nothing correlated yet.
    ///
    /// IDs are time-ordered (UUIDv7, monotonic within a process), so events recorded in the
    /// same second still sort in the order they happened.
    pub fn new(
        kind: impl Into<String>,
        action: impl Into<String>,
        operation: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            at,
            kind: kind.into(),
            action: action.into(),
            operation: operation.into(),
            station: None,
            mission: None,
            session: None,
            actor: None,
            detail: BTreeMap::new(),
        }
    }

    /// Names the Station involved.
    #[must_use]
    pub fn station(mut self, station: impl Into<String>) -> Self {
        self.station = Some(station.into());
        self
    }

    /// Names the Mission involved.
    #[must_use]
    pub fn mission(mut self, mission: impl Into<String>) -> Self {
        self.mission = Some(mission.into());
        self
    }

    /// Names the provider Session involved.
    #[must_use]
    pub fn session(mut self, session: impl Into<String>) -> Self {
        self.session = Some(session.into());
        self
    }

    /// Names who acted.
    #[must_use]
    pub fn actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(actor.into());
        self
    }

    /// Adds one safe detail.
    #[must_use]
    pub fn detail(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.detail.insert(key.into(), value.into());
        self
    }

    fn validate(&self) -> io::Result<()> {
        let invalid = |what: &str| {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("timeline event {what} is invalid"),
            ))
        };
        let name = |value: &str| {
            !value.is_empty()
                && value.len() <= MAX_NAME_BYTES
                && value.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
                })
        };
        let text = |value: &str| {
            !value.trim().is_empty()
                && value.len() <= MAX_TEXT_BYTES
                && !value.contains(char::is_control)
        };
        if self.id.is_empty()
            || self.id.len() > MAX_ID_BYTES
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return invalid("ID");
        }
        if !self.at.is_finite() || self.at <= 0.0 {
            return invalid("time");
        }
        if !name(&self.kind) || !name(&self.action) {
            return invalid("kind or action");
        }
        let optional = [&self.station, &self.mission, &self.session, &self.actor];
        if !text(&self.operation) || optional.into_iter().flatten().any(|value| !text(value)) {
            return invalid("identity");
        }
        if self.detail.len() > MAX_DETAIL_ENTRIES
            || self
                .detail
                .iter()
                .any(|(key, value)| !name(key) || !text(value))
        {
            return invalid("detail");
        }
        Ok(())
    }

    /// The file name: microseconds first, so names sort by time.
    fn file_name(&self) -> String {
        let micros = (self.at * 1_000_000.0) as u64;
        format!("{micros:020}-{}.json", self.id)
    }
}

/// The activity timeline of each Operation.
pub trait TimelineStore {
    /// Records one activity event, dropping the oldest past [`MAX_TIMELINE_EVENTS`].
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an event that breaks the bounds above; I/O errors otherwise.
    fn record_event(&self, event: &TimelineEvent) -> io::Result<()>;

    /// Every recorded event of one Operation, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error when the timeline directory cannot be read; unreadable or invalid
    /// event files are skipped.
    fn timeline_events(&self, operation: &str) -> io::Result<Vec<TimelineEvent>>;
}

impl TimelineStore for FileRegistry {
    fn record_event(&self, event: &TimelineEvent) -> io::Result<()> {
        let _lock = registry_lock(self.root())?;
        record_event_locked(self.root(), event)
    }

    fn timeline_events(&self, operation: &str) -> io::Result<Vec<TimelineEvent>> {
        let mut events = Vec::new();
        for path in event_files(self.root())? {
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if metadata.len() > MAX_EVENT_BYTES {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            if let Ok(event) = serde_json::from_slice::<TimelineEvent>(&bytes)
                && event.operation == operation
                && event.validate().is_ok()
            {
                events.push(event);
            }
        }
        Ok(events)
    }
}

/// Records an event while the caller already holds the Registry lock.
pub(super) fn record_event_locked(root: &Path, event: &TimelineEvent) -> io::Result<()> {
    event.validate()?;
    let bytes = serde_json::to_vec(event).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_EVENT_BYTES {
        return Err(invalid_data("timeline event exceeds the size limit"));
    }
    fs::create_dir_all(root.join("timeline"))?;
    write_atomic(&root.join("timeline").join(event.file_name()), &bytes)?;
    let files = event_files(root)?;
    for oldest in files
        .iter()
        .take(files.len().saturating_sub(MAX_TIMELINE_EVENTS))
    {
        let _ = fs::remove_file(oldest);
    }
    Ok(())
}

/// The event files, oldest first.
fn event_files(root: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    let entries = match fs::read_dir(root.join("timeline")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_in_the_same_second_keep_the_order_they_were_recorded_in() {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(directory.path());
        let actions = (0..20)
            .map(|index| format!("step{index}"))
            .collect::<Vec<_>>();
        for action in &actions {
            registry
                .record_event(&TimelineEvent::new("lifecycle", action.clone(), "G", 100.0))
                .unwrap();
        }
        let recorded = registry
            .timeline_events("G")
            .unwrap()
            .into_iter()
            .map(|event| event.action)
            .collect::<Vec<_>>();
        assert_eq!(recorded, actions);
    }

    #[test]
    fn events_are_kept_per_operation_in_time_order_and_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(directory.path());
        let second = TimelineEvent::new("lifecycle", "working", "G", 2.0)
            .station("hq")
            .mission("mission-1")
            .session("provider-1");
        let first = TimelineEvent::new("control", "cancel.applied", "G", 1.0)
            .actor("operator-007")
            .detail("state", "abandoned");
        registry.record_event(&second).unwrap();
        registry.record_event(&first).unwrap();
        registry
            .record_event(&TimelineEvent::new("cable", "sent", "L", 3.0))
            .unwrap();

        assert_eq!(registry.timeline_events("G").unwrap(), [first, second]);
        assert_eq!(registry.timeline_events("L").unwrap().len(), 1);
        assert!(registry.timeline_events("Q").unwrap().is_empty());
    }

    #[test]
    fn events_outside_the_bounds_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(directory.path());
        for event in [
            TimelineEvent::new("Lifecycle", "working", "G", 1.0),
            TimelineEvent::new("lifecycle", "working", "G", 0.0),
            TimelineEvent::new("lifecycle", "working", "", 1.0),
            TimelineEvent::new("lifecycle", "working", "G", 1.0).mission("m\n1"),
            TimelineEvent::new("cable", "sent", "G", 1.0).detail("body", "x".repeat(300)),
        ] {
            assert_eq!(
                registry.record_event(&event).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        let mut crowded = TimelineEvent::new("cable", "sent", "G", 1.0);
        for index in 0..=MAX_DETAIL_ENTRIES {
            crowded = crowded.detail(format!("key{index}"), "value");
        }
        assert!(registry.record_event(&crowded).is_err());
    }

    #[test]
    fn the_oldest_events_go_first_past_the_bound() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("timeline")).unwrap();
        // Planting files directly keeps the test fast; the writer prunes after each write.
        for index in 0..MAX_TIMELINE_EVENTS {
            let event = TimelineEvent::new("lifecycle", "working", "G", 10.0 + index as f64);
            fs::write(
                root.join("timeline").join(event.file_name()),
                serde_json::to_vec(&event).unwrap(),
            )
            .unwrap();
        }
        let registry = FileRegistry::open(root);
        registry
            .record_event(&TimelineEvent::new("lifecycle", "done", "G", 1e9))
            .unwrap();
        let events = registry.timeline_events("G").unwrap();
        assert_eq!(events.len(), MAX_TIMELINE_EVENTS);
        assert_eq!(events[0].at, 11.0);
        assert_eq!(events.last().unwrap().action, "done");
    }
}
