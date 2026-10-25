//! Durable Mission lifecycle owned by the AI6 daemon.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::directorates::m::cover::{CoverAudit, CoverReport};
use crate::directorates::q::safehouse::SafeHouseReport;
use crate::{MissionId, SessionId};

/// Lifecycle state for a Mission.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionState {
    /// Accepted by AI6 and waiting for assignment.
    Queued,
    /// An agent operation is active.
    Working,
    /// Waiting on an external condition.
    Blocked,
    /// No progress has been observed within the configured limit.
    Stalled,
    /// The work completed successfully.
    Done,
    /// The work ended unsuccessfully.
    Failed,
    /// The work completed but violated its Operation's cover policy.
    Blown,
    /// The work was withdrawn or handed off.
    Abandoned,
}

/// A Mission as stored and returned by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mission {
    /// AI6-owned identifier for the unit of work.
    pub mission_id: MissionId,
    /// Registered Operation that owns the work, when specified.
    pub operation_id: Option<String>,
    /// Task instruction for the selected Agent.
    pub prompt: String,
    /// Configured Agent identity, when assigned.
    pub agent_id: Option<String>,
    /// Canonical configured Agent role requested for this Mission, when specified.
    #[serde(default)]
    pub role_id: Option<String>,
    /// Current lifecycle state.
    pub state: MissionState,
    /// Provider-issued operation identities belonging to this Mission.
    pub session_ids: Vec<SessionId>,
    /// Isolated Operation worktree and final disposition, when one was created.
    pub safe_house: Option<SafeHouseReport>,
    /// Cover policy result, when the Operation's commits were audited.
    pub cover: Option<CoverReport>,
    /// Monotonically increasing state revision.
    pub revision: u64,
}

/// Mission records held in memory and persisted as one atomic snapshot.
#[derive(Debug, Default)]
pub struct MissionStore {
    missions: BTreeMap<String, Mission>,
    operations: BTreeMap<String, LaunchManifest>,
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LaunchManifest {
    pub(crate) handler_provider: String,
    pub(crate) terminal_provider: String,
    pub(crate) working_directory: String,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) cover: Option<CoverAudit>,
    /// The Station that supervises the Session; absent in records written before placement.
    #[serde(default)]
    pub(crate) station_id: Option<String>,
    /// The Theater the harness runs in; absent in records written before placement.
    #[serde(default)]
    pub(crate) theater_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct MissionSnapshot {
    version: u32,
    missions: BTreeMap<String, Mission>,
    operations: BTreeMap<String, LaunchManifest>,
}

impl MissionStore {
    /// Opens a durable Mission book, creating an empty store when needed.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let (missions, operations) = if path.exists() {
            let bytes = fs::read(&path)?;
            let snapshot: MissionSnapshot = serde_json::from_slice(&bytes).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Mission store: {error}"),
                )
            })?;
            if snapshot.version != 3
                || snapshot.missions.iter().any(|(key, mission)| {
                    key != mission.mission_id.as_str()
                        || mission.prompt.trim().is_empty()
                        || mission.revision == 0
                })
                || snapshot.operations.iter().any(|(key, operation)| {
                    !snapshot.missions.contains_key(key)
                        || operation.handler_provider.trim().is_empty()
                        || operation.terminal_provider.trim().is_empty()
                        || !Path::new(&operation.working_directory).is_absolute()
                })
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Mission store has an unsupported version or invalid record",
                ));
            }
            (snapshot.missions, snapshot.operations)
        } else {
            (BTreeMap::new(), BTreeMap::new())
        };
        let book = Self {
            missions,
            operations,
            path: Some(path),
        };
        if !book.path.as_deref().is_some_and(Path::exists) {
            book.persist(&book.missions, &book.operations)?;
        }
        Ok(book)
    }

    /// Creates a queued Mission. The caller supplies the durable identity from the Registry.
    pub fn create(
        &mut self,
        mission_id: MissionId,
        operation_id: Option<String>,
        prompt: String,
        agent_id: Option<String>,
        role_id: Option<String>,
    ) -> Result<Mission, MissionError> {
        if prompt.trim().is_empty() {
            return Err(MissionError::EmptyPrompt);
        }
        let key = mission_id.as_str().to_owned();
        if self.missions.contains_key(&key) {
            return Err(MissionError::AlreadyExists);
        }
        let mission = Mission {
            mission_id,
            operation_id,
            prompt,
            agent_id,
            role_id,
            state: MissionState::Queued,
            session_ids: Vec::new(),
            safe_house: None,
            cover: None,
            revision: 1,
        };
        let mut next = self.missions.clone();
        next.insert(key, mission.clone());
        self.commit(next)?;
        Ok(mission)
    }

    /// Looks up a Mission by its AI6-owned ID.
    pub fn get(&self, mission_id: &str) -> Option<&Mission> {
        self.missions.get(mission_id)
    }

    /// Lists Missions in stable identity order.
    pub fn list(&self) -> Vec<Mission> {
        self.missions.values().cloned().collect()
    }

    #[cfg(feature = "daemon")]
    pub(crate) fn operation(&self, mission_id: &str) -> Option<LaunchManifest> {
        self.operations.get(mission_id).cloned()
    }

    #[cfg(feature = "daemon")]
    pub(crate) fn set_operation(
        &mut self,
        mission_id: &str,
        operation: LaunchManifest,
    ) -> Result<(), MissionError> {
        if !self.missions.contains_key(mission_id) {
            return Err(MissionError::NotFound);
        }
        if operation.handler_provider.trim().is_empty()
            || operation.terminal_provider.trim().is_empty()
            || !Path::new(&operation.working_directory).is_absolute()
        {
            return Err(MissionError::InvalidOperation);
        }
        let mut operations = self.operations.clone();
        operations.insert(mission_id.to_owned(), operation);
        self.commit_with(self.missions.clone(), operations)
    }

    /// Marks persisted active Missions blocked so an operator can reattach their provider Sessions.
    #[cfg(feature = "daemon")]
    pub(crate) fn recover_working(&mut self) -> Result<usize, MissionError> {
        let mut missions = self.missions.clone();
        let mut recovered = 0;
        for mission in missions.values_mut() {
            if mission.state == MissionState::Working {
                mission.state = MissionState::Blocked;
                mission.revision += 1;
                recovered += 1;
            }
        }
        if recovered > 0 {
            self.commit(missions)?;
        }
        Ok(recovered)
    }

    /// Changes lifecycle state, rejecting invalid transitions and treating repeats as no-ops.
    pub fn transition(
        &mut self,
        mission_id: &str,
        state: MissionState,
    ) -> Result<Mission, MissionError> {
        let mut next = self.missions.clone();
        let mission = next.get_mut(mission_id).ok_or(MissionError::NotFound)?;
        if mission.state == state {
            return Ok(mission.clone());
        }
        if !valid_transition(mission.state, state) {
            return Err(MissionError::InvalidTransition {
                from: mission.state,
                to: state,
            });
        }
        mission.state = state;
        mission.revision += 1;
        let updated = mission.clone();
        self.commit(next)?;
        Ok(updated)
    }

    /// Associates a provider-issued Session ID with the Mission that operation serves.
    pub fn attach_session(
        &mut self,
        mission_id: &str,
        session_id: SessionId,
    ) -> Result<Mission, MissionError> {
        let mut next = self.missions.clone();
        let mission = next.get_mut(mission_id).ok_or(MissionError::NotFound)?;
        if matches!(
            mission.state,
            MissionState::Done
                | MissionState::Failed
                | MissionState::Blown
                | MissionState::Abandoned
        ) {
            return Err(MissionError::TerminalMission);
        }
        if mission.session_ids.contains(&session_id) {
            return Err(MissionError::SessionAlreadyAttached);
        }
        mission.session_ids.push(session_id);
        mission.revision += 1;
        let updated = mission.clone();
        self.commit(next)?;
        Ok(updated)
    }

    /// Records the worktree created for this Mission or its final cleanup report.
    pub fn set_safe_house(
        &mut self,
        mission_id: &str,
        report: SafeHouseReport,
    ) -> Result<Mission, MissionError> {
        let mut next = self.missions.clone();
        let mission = next.get_mut(mission_id).ok_or(MissionError::NotFound)?;
        mission.safe_house = Some(report);
        mission.revision += 1;
        let updated = mission.clone();
        self.commit(next)?;
        Ok(updated)
    }

    /// Records the result of auditing commits against the Operation's cover policy.
    pub fn set_cover_report(
        &mut self,
        mission_id: &str,
        report: CoverReport,
    ) -> Result<Mission, MissionError> {
        let mut next = self.missions.clone();
        let mission = next.get_mut(mission_id).ok_or(MissionError::NotFound)?;
        mission.cover = Some(report);
        mission.revision += 1;
        let updated = mission.clone();
        self.commit(next)?;
        Ok(updated)
    }

    fn commit(&mut self, next: BTreeMap<String, Mission>) -> Result<(), MissionError> {
        self.commit_with(next, self.operations.clone())
    }

    fn commit_with(
        &mut self,
        missions: BTreeMap<String, Mission>,
        operations: BTreeMap<String, LaunchManifest>,
    ) -> Result<(), MissionError> {
        self.persist(&missions, &operations)
            .map_err(|_| MissionError::Persistence)?;
        self.missions = missions;
        self.operations = operations;
        Ok(())
    }

    fn persist(
        &self,
        missions: &BTreeMap<String, Mission>,
        operations: &BTreeMap<String, LaunchManifest>,
    ) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec(&MissionSnapshot {
            version: 3,
            missions: missions.clone(),
            operations: operations.clone(),
        })
        .map_err(io::Error::other)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    }
}

/// A rejected Mission operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionError {
    /// A Mission with this ID already exists.
    AlreadyExists,
    /// A Mission needs a non-empty prompt.
    EmptyPrompt,
    /// The requested Mission does not exist.
    NotFound,
    /// The requested state change is not allowed.
    InvalidTransition {
        from: MissionState,
        to: MissionState,
    },
    /// A terminal Mission cannot accept new Sessions.
    TerminalMission,
    /// The provider Session ID is already attached to this Mission.
    SessionAlreadyAttached,
    /// The durable Mission snapshot could not be written.
    Persistence,
    /// Persisted AgentHandler execution settings are incomplete.
    InvalidOperation,
}

fn valid_transition(from: MissionState, to: MissionState) -> bool {
    use MissionState::{Abandoned, Blocked, Blown, Done, Failed, Queued, Stalled, Working};
    matches!(
        (from, to),
        (Queued, Working | Failed | Abandoned)
            | (Working, Blocked | Stalled | Done | Failed | Abandoned)
            | (Blocked, Stalled | Working | Failed | Abandoned)
            | (Working, Blown)
            | (Blocked, Blown)
            | (Stalled, Working | Failed | Blown | Abandoned)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> MissionStore {
        MissionStore::default()
    }

    #[test]
    fn a_mission_groups_provider_sessions_and_preserves_identity() {
        let mut book = book();
        let mission = book
            .create(
                MissionId::try_new("mission-1").unwrap(),
                Some("operation-a".to_owned()),
                "inspect the service".to_owned(),
                Some("007".to_owned()),
                None,
            )
            .unwrap();
        assert_eq!(mission.revision, 1);
        assert_eq!(mission.state, MissionState::Queued);

        book.transition("mission-1", MissionState::Working).unwrap();
        let session = SessionId::try_new("provider-session-abc").unwrap();
        book.attach_session("mission-1", session.clone()).unwrap();
        let mission = book.transition("mission-1", MissionState::Blocked).unwrap();
        let resumed = book.transition("mission-1", MissionState::Working).unwrap();

        assert_eq!(mission.revision, 4);
        assert_eq!(resumed.revision, 5);
        assert_eq!(resumed.session_ids, [session]);
    }

    #[test]
    fn terminal_missions_reject_new_sessions_and_state_changes() {
        let mut book = book();
        book.create(
            MissionId::try_new("mission-1").unwrap(),
            None,
            "inspect the service".to_owned(),
            None,
            None,
        )
        .unwrap();
        book.transition("mission-1", MissionState::Failed).unwrap();

        assert_eq!(
            book.transition("mission-1", MissionState::Working),
            Err(MissionError::InvalidTransition {
                from: MissionState::Failed,
                to: MissionState::Working,
            })
        );
        assert_eq!(
            book.attach_session("mission-1", SessionId::try_new("session-1").unwrap()),
            Err(MissionError::TerminalMission)
        );
    }

    #[test]
    fn a_blocked_mission_can_be_marked_stalled_by_the_watchdog() {
        let mut book = book();
        book.create(
            MissionId::try_new("mission-blocked-watchdog").unwrap(),
            None,
            "waiting for input".to_owned(),
            Some("007".to_owned()),
            None,
        )
        .unwrap();
        book.transition("mission-blocked-watchdog", MissionState::Working)
            .unwrap();
        book.transition("mission-blocked-watchdog", MissionState::Blocked)
            .unwrap();

        let stalled = book
            .transition("mission-blocked-watchdog", MissionState::Stalled)
            .unwrap();

        assert_eq!(stalled.state, MissionState::Stalled);
    }

    #[test]
    fn repeating_the_current_state_does_not_change_the_revision() {
        let mut book = book();
        let created = book
            .create(
                MissionId::try_new("mission-idempotent").unwrap(),
                None,
                "inspect the service".to_owned(),
                None,
                None,
            )
            .unwrap();
        let repeated = book
            .transition("mission-idempotent", MissionState::Queued)
            .unwrap();

        assert_eq!(repeated, created);
    }

    #[test]
    fn a_mission_can_finish_with_a_blown_cover_report() {
        let mut book = book();
        book.create(
            MissionId::try_new("mission-cover").unwrap(),
            Some("operation-g".to_owned()),
            "update the service".to_owned(),
            Some("007".to_owned()),
            None,
        )
        .unwrap();
        book.transition("mission-cover", MissionState::Working)
            .unwrap();
        let report = CoverReport {
            policy: "covert".to_owned(),
            commits: 1,
            checkable: true,
            blown: true,
            violations: vec![crate::directorates::m::cover::CoverViolation {
                commit: "abcdef123456".to_owned(),
                rule: "trailer".to_owned(),
                detail: "Co-Authored-By: Agent <agent@example.invalid>".to_owned(),
            }],
        };
        book.set_cover_report("mission-cover", report.clone())
            .unwrap();
        let blown = book
            .transition("mission-cover", MissionState::Blown)
            .unwrap();

        assert_eq!(blown.state, MissionState::Blown);
        assert_eq!(blown.cover, Some(report));
    }

    #[test]
    fn durable_book_reopens_mission_state_and_provider_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missions.json");
        {
            let mut book = MissionStore::open(&path).unwrap();
            book.create(
                MissionId::try_new("mission-1").unwrap(),
                Some("operation-a".to_owned()),
                "inspect the service".to_owned(),
                Some("007".to_owned()),
                Some("case-officer".to_owned()),
            )
            .unwrap();
            book.transition("mission-1", MissionState::Working).unwrap();
            book.attach_session(
                "mission-1",
                SessionId::try_new("provider-session-9").unwrap(),
            )
            .unwrap();
            book.set_operation(
                "mission-1",
                LaunchManifest {
                    handler_provider: "claude".to_owned(),
                    terminal_provider: "pty".to_owned(),
                    working_directory: directory.path().to_string_lossy().into_owned(),
                    model: Some("sonnet".to_owned()),
                    effort: Some("high".to_owned()),
                    cover: None,
                    station_id: Some("hq".to_owned()),
                    theater_id: Some("local".to_owned()),
                },
            )
            .unwrap();
        }

        let mut reopened = MissionStore::open(path).unwrap();
        let manifest = reopened.operation("mission-1").unwrap();
        assert_eq!(manifest.station_id.as_deref(), Some("hq"));
        assert_eq!(manifest.theater_id.as_deref(), Some("local"));
        let mission = reopened.get("mission-1").unwrap();
        assert_eq!(mission.state, MissionState::Working);
        assert_eq!(mission.role_id.as_deref(), Some("case-officer"));
        assert_eq!(mission.session_ids[0].as_str(), "provider-session-9");
        assert_eq!(mission.revision, 3);
        assert_eq!(
            reopened.operation("mission-1").unwrap().handler_provider,
            "claude"
        );
        assert_eq!(reopened.recover_working().unwrap(), 1);
        assert_eq!(
            reopened.get("mission-1").unwrap().state,
            MissionState::Blocked
        );
        assert_eq!(reopened.get("mission-1").unwrap().revision, 4);
        assert_eq!(reopened.recover_working().unwrap(), 0);
    }

    #[test]
    fn launch_records_written_before_placement_still_load() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missions.json");
        {
            let mut book = MissionStore::open(&path).unwrap();
            book.create(
                MissionId::try_new("old-1").unwrap(),
                None,
                "inspect".to_owned(),
                None,
                None,
            )
            .unwrap();
            book.set_operation(
                "old-1",
                LaunchManifest {
                    handler_provider: "claude".to_owned(),
                    terminal_provider: "pty".to_owned(),
                    working_directory: directory.path().to_string_lossy().into_owned(),
                    model: None,
                    effort: None,
                    cover: None,
                    station_id: Some("hq".to_owned()),
                    theater_id: Some("local".to_owned()),
                },
            )
            .unwrap();
        }
        let mut snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let manifest = snapshot["operations"]["old-1"].as_object_mut().unwrap();
        manifest.remove("station_id").unwrap();
        manifest.remove("theater_id").unwrap();
        fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();

        let reopened = MissionStore::open(&path).unwrap();
        let manifest = reopened.operation("old-1").unwrap();
        assert_eq!(manifest.station_id, None);
        assert_eq!(manifest.theater_id, None);
    }
}
