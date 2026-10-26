//! Station logbooks: the reports a Head of Station files on each round of its Station.
//!
//! A round of an `each_station` Sleeper is a Mission of its own, so what one round saw
//! reaches the next through the logbook: the Station keeps the newest
//! [`LOGBOOK_LENGTH`] reports and hands them to the next round as context. The newest report
//! is also published with the Station's check-in, so the operator sees each Station's state
//! in one place.

use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::store::{
    FileRegistry, hex_digest, invalid_data, registry_lock, safe_component, validate_station_id,
    write_atomic,
};

/// The role of the Agent that keeps a Station: a Mission for this role narrowed to one Station
/// is that Station's round, given its logbook and the Station's own tools.
pub const HEAD_OF_STATION_ROLE: &str = "head-of-station";
/// Whether `role` is [`HEAD_OF_STATION_ROLE`] or a role beneath it: only an authorized
/// Sleeper may queue a brief for it.
pub fn is_head_of_station_role(role: &str) -> bool {
    role == HEAD_OF_STATION_ROLE || role.starts_with("head-of-station/")
}

/// The Cable channel a filed report is sent to, when `cables.toml` configures it.
pub const HEAD_OF_STATION_CHANNEL: &str = "heads-of-station";
/// How many reports a Station's logbook keeps; filing another drops the oldest.
pub const LOGBOOK_LENGTH: usize = 20;
/// The longest summary a report may carry, in bytes.
pub const MAX_SUMMARY_BYTES: usize = 4 * 1024;
/// The most findings one report may carry.
pub const MAX_FINDINGS: usize = 32;
/// The longest text one finding may carry, in bytes.
pub const MAX_FINDING_BYTES: usize = 1024;
/// Upper bound of a stored logbook; a larger file is refused as damaged.
pub(super) const MAX_LOGBOOK_BYTES: u64 =
    (LOGBOOK_LENGTH * (MAX_SUMMARY_BYTES + MAX_FINDINGS * (MAX_FINDING_BYTES + 128) + 1024)) as u64;

/// What a finding is about, so the operator's view can group them.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StationFindingKind {
    /// Free space under the Station's state directory.
    Disk,
    /// A Theater the Station reaches: reachability, requirements, provisioning.
    Theater,
    /// A safe house the Station keeps.
    SafeHouse,
    /// A provider library or the plugin scan.
    Provider,
    /// Anything else.
    Other,
}

impl StationFindingKind {
    /// The name used on the wire: `disk`, `theater`, `safe-house`, `provider` or `other`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disk => "disk",
            Self::Theater => "theater",
            Self::SafeHouse => "safe-house",
            Self::Provider => "provider",
            Self::Other => "other",
        }
    }
}

impl FromStr for StationFindingKind {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(match text {
            "disk" => Self::Disk,
            "theater" => Self::Theater,
            "safe-house" => Self::SafeHouse,
            "provider" => Self::Provider,
            "other" => Self::Other,
            other => {
                return Err(format!(
                    "{other:?} is not a finding kind: use disk, theater, safe-house, provider or other"
                ));
            }
        })
    }
}

impl fmt::Display for StationFindingKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How much a finding matters.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Worth knowing; nothing to do.
    Info,
    /// Needs attention before it stops work.
    Warning,
    /// Stops work, or will soon.
    Critical,
}

impl Severity {
    /// The name used on the wire: `info`, `warning` or `critical`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

impl FromStr for Severity {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(match text {
            "info" => Self::Info,
            "warning" => Self::Warning,
            "critical" => Self::Critical,
            other => {
                return Err(format!(
                    "{other:?} is not a severity: use info, warning or critical"
                ));
            }
        })
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One thing a round found.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationFinding {
    /// What it is about.
    pub kind: StationFindingKind,
    /// How much it matters.
    pub severity: Severity,
    /// What was found, in a sentence an operator can act on.
    pub text: String,
}

impl StationFinding {
    /// A finding of `kind` and `severity`.
    pub fn new(kind: StationFindingKind, severity: Severity, text: impl Into<String>) -> Self {
        Self {
            kind,
            severity,
            text: text.into(),
        }
    }
}

/// A report a Head of Station filed on its Station.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationReport {
    /// The Station it reports on.
    pub station: String,
    /// When it was filed, as Unix seconds.
    pub at: i64,
    /// What the round saw and did, in a few lines.
    pub summary: String,
    /// What needs the operator's eye.
    #[serde(default)]
    pub findings: Vec<StationFinding>,
    /// The Mission of the round that filed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<String>,
    /// The Agent that filed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

impl StationReport {
    /// A report on `station` filed at `at` (Unix seconds) with no findings.
    pub fn new(station: impl Into<String>, summary: impl Into<String>, at: i64) -> Self {
        Self {
            station: station.into(),
            at,
            summary: summary.into(),
            findings: Vec::new(),
            mission: None,
            agent: None,
        }
    }

    /// Adds findings.
    #[must_use]
    pub fn with_findings(mut self, findings: impl IntoIterator<Item = StationFinding>) -> Self {
        self.findings.extend(findings);
        self
    }

    /// Records the round's Mission and Agent.
    #[must_use]
    pub fn filed_by(mut self, mission: impl Into<String>, agent: impl Into<String>) -> Self {
        self.mission = Some(mission.into());
        self.agent = Some(agent.into());
        self
    }

    /// The most severe of its findings, if it has any.
    pub fn worst(&self) -> Option<Severity> {
        self.findings.iter().map(|finding| finding.severity).max()
    }

    /// Checks the bounds above.
    ///
    /// # Errors
    ///
    /// `InvalidInput` naming what is out of bounds: the Station ID, an empty or long summary,
    /// too many findings, or an empty or long finding.
    pub fn validate(&self) -> io::Result<()> {
        validate_station_id(&self.station)?;
        let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
        if self.summary.trim().is_empty() || self.summary.len() > MAX_SUMMARY_BYTES {
            return Err(bad(format!(
                "a report's summary must be 1 to {MAX_SUMMARY_BYTES} bytes"
            )));
        }
        if self.findings.len() > MAX_FINDINGS {
            return Err(bad(format!(
                "a report carries at most {MAX_FINDINGS} findings"
            )));
        }
        if self
            .findings
            .iter()
            .any(|finding| finding.text.trim().is_empty() || finding.text.len() > MAX_FINDING_BYTES)
        {
            return Err(bad(format!(
                "each finding's text must be 1 to {MAX_FINDING_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

/// A Station ID as a file or key component. IDs that differ only in characters a key cannot
/// hold get a digest of the whole ID, so two Stations never share one.
pub(super) fn station_key(station: &str) -> String {
    let safe = safe_component(station);
    if safe == station {
        return safe;
    }
    let digest = hex_digest(&Sha256::digest(station.as_bytes()));
    format!("{safe}-{}", &digest[..12])
}

/// Keeps the newest [`LOGBOOK_LENGTH`] reports of a logbook, oldest first.
pub(super) fn append_report(logbook: &mut Vec<StationReport>, report: &StationReport) {
    logbook.push(report.clone());
    let excess = logbook.len().saturating_sub(LOGBOOK_LENGTH);
    logbook.drain(..excess);
}

/// Each Station's logbook of Head of Station reports.
pub trait LogbookStore {
    /// Files `report` in its Station's logbook, dropping the oldest past
    /// [`LOGBOOK_LENGTH`].
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a report out of bounds ([`StationReport::validate`]); an error when
    /// the Registry cannot be read or written.
    fn file_station_report(&self, report: &StationReport) -> io::Result<()>;

    /// The Station's logbook, oldest first; empty when it has none.
    ///
    /// # Errors
    ///
    /// Returns an error when the logbook cannot be read.
    fn logbook(&self, station: &str) -> io::Result<Vec<StationReport>>;
}

impl FileRegistry {
    fn logbook_path(&self, station: &str) -> PathBuf {
        self.root
            .join("logbook")
            .join(format!("{}.json", station_key(station)))
    }
}

impl LogbookStore for FileRegistry {
    fn file_station_report(&self, report: &StationReport) -> io::Result<()> {
        report.validate()?;
        let _lock = registry_lock(&self.root)?;
        let path = self.logbook_path(&report.station);
        let mut logbook = read_logbook(&path)?;
        append_report(&mut logbook, report);
        let bytes = serde_json::to_vec(&logbook).map_err(io::Error::other)?;
        write_atomic(&path, &bytes)
    }

    fn logbook(&self, station: &str) -> io::Result<Vec<StationReport>> {
        validate_station_id(station)?;
        read_logbook(&self.logbook_path(station))
    }
}

fn read_logbook(path: &std::path::Path) -> io::Result<Vec<StationReport>> {
    match fs::read(path) {
        Ok(bytes) if bytes.len() as u64 <= MAX_LOGBOOK_BYTES => serde_json::from_slice(&bytes)
            .map_err(|error| invalid_data(format!("invalid Station logbook: {error}"))),
        Ok(_) => Err(invalid_data("Station logbook exceeds the size limit")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_logbook_keeps_the_newest_reports_per_station,
        a_report_out_of_bounds_is_refused,
    );

    fn report(station: &str, at: i64) -> StationReport {
        StationReport::new(station, format!("round at {at}"), at)
            .with_findings([StationFinding::new(
                StationFindingKind::SafeHouse,
                Severity::Warning,
                "three safe houses hold 40 GB of build caches",
            )])
            .filed_by(format!("round-{at}"), "M")
    }

    fn a_logbook_keeps_the_newest_reports_per_station(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.logbook("station-a").unwrap().is_empty());
        for at in 0..25 {
            registry
                .file_station_report(&report("station-a", at))
                .unwrap();
        }
        registry
            .file_station_report(&report("station b", 99))
            .unwrap();
        let logbook = store.handle().logbook("station-a").unwrap();
        assert_eq!(logbook.len(), LOGBOOK_LENGTH);
        assert_eq!(logbook.first().unwrap().at, 5, "the oldest are dropped");
        assert_eq!(logbook.last().unwrap(), &report("station-a", 24));
        assert_eq!(logbook.last().unwrap().worst(), Some(Severity::Warning));
        assert_eq!(
            registry.logbook("station b").unwrap(),
            [report("station b", 99)]
        );
        assert!(registry.logbook("station_b").unwrap().is_empty());
    }

    fn a_report_out_of_bounds_is_refused(store: &dyn Store) {
        let registry = store.handle();
        let empty = StationReport::new("station-a", " ", 1);
        let crowded = StationReport::new("station-a", "s", 1).with_findings(
            (0..=MAX_FINDINGS)
                .map(|_| StationFinding::new(StationFindingKind::Other, Severity::Info, "x")),
        );
        let long = StationReport::new("station-a", "s", 1).with_findings([StationFinding::new(
            StationFindingKind::Disk,
            Severity::Critical,
            "x".repeat(MAX_FINDING_BYTES + 1),
        )]);
        for bad in [empty, crowded, long] {
            let error = registry.file_station_report(&bad).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
        assert!(registry.logbook("station-a").unwrap().is_empty());
    }

    #[test]
    fn kinds_and_severities_read_as_written() {
        for kind in [
            StationFindingKind::Disk,
            StationFindingKind::Theater,
            StationFindingKind::SafeHouse,
            StationFindingKind::Provider,
            StationFindingKind::Other,
        ] {
            assert_eq!(kind.as_str().parse::<StationFindingKind>(), Ok(kind));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(kind.as_str())
            );
        }
        for severity in [Severity::Info, Severity::Warning, Severity::Critical] {
            assert_eq!(severity.as_str().parse::<Severity>(), Ok(severity));
        }
        assert!("safe_house".parse::<StationFindingKind>().is_err());
        assert!(Severity::Critical > Severity::Warning);
    }
}
