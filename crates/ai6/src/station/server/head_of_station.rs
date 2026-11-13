//! The Station charge: what the Chief of Station (Head of Station in code) may do for its
//! Station, as the Station's own tools.
//!
//! The Station's charge (`station:<id>`, [`crate::directorates::m::charges`]) is one standing Run
//! that keeps this Station fit: its safe houses, caches and Theaters, its Runs' progress, its
//! queue, and what the Operations' 00 Agents ask of it. It is not a Mission, and no Sleeper
//! queues it; the next wake carries what happened. Continuity is its logbook on the charge
//! record.
//!
//! The Station tools (`station_report`, `file_report`, `tidy_safe_house`, `provision_theater`,
//! `surrender_claim`, `answer_request`) act on this Station only, and every call checks again that
//! this Station still runs the charge. What they may change unattended is fixed in Station code,
//! never in the prompt.

#[cfg(feature = "cables-mcp")]
mod claims;
#[cfg(feature = "cables-mcp")]
mod hygiene;
#[cfg(feature = "cables-mcp")]
mod provision;
#[cfg(feature = "cables-mcp")]
mod report;
#[cfg(feature = "cables-mcp")]
mod safe_houses;
#[cfg(feature = "cables-mcp")]
mod survey;

use std::collections::BTreeMap;

#[cfg(feature = "cables-mcp")]
use crate::directorates::registry::logbook::StationFinding;
#[cfg(feature = "cables-mcp")]
use crate::directorates::registry::logbook::StationReport;
#[cfg(feature = "cables-mcp")]
use crate::directorates::registry::timeline::TimelineEvent;

use super::DaemonState;

#[cfg(feature = "cables-mcp")]
pub(super) use report::StationState;

/// The check-in key that carries a Station's latest Head of Station report.
pub(super) const CHECK_IN_KEY: &str = "head_of_station";

/// How long a Run has made no progress, and whether that counts as a stall: it has been quiet for
/// `quiet_after` while working, or its own watchdog judged the Mission stalled.
pub(super) fn quiet_of(
    state: &DaemonState,
    mission: &str,
    mission_state: crate::directorates::m::mission::MissionState,
    quiet_after: Option<std::time::Duration>,
) -> (Option<std::time::Duration>, bool) {
    use crate::directorates::m::mission::MissionState;
    let quiet = state.runs.lock().ok().and_then(|runs| {
        runs.get(mission)
            .and_then(|operation| operation.watchdog.quiet_for())
    });
    let stalled = mission_state == MissionState::Stalled
        || (mission_state == MissionState::Working
            && matches!((quiet, quiet_after), (Some(quiet), Some(after)) if quiet >= after));
    (quiet, stalled)
}

/// Adds this Station's latest Head of Station report to its check-in.
pub(super) fn check_in(state: &DaemonState, info: &mut BTreeMap<String, serde_json::Value>) {
    match state.intake.logbook(&state.station_id) {
        Ok(logbook) => {
            if let Some(latest) = logbook.last() {
                info.insert(CHECK_IN_KEY.to_owned(), serde_json::json!(latest));
            }
        }
        Err(error) => eprintln!(
            "ai6: the logbook of Station {:?} is unreadable: {error}",
            state.station_id
        ),
    }
}

/// The Station charge's access to its Station: what its Cable endpoint's Station tools call.
#[cfg(feature = "cables-mcp")]
#[derive(Clone)]
pub(super) struct Desk {
    state: DaemonState,
    /// The Agent that keeps the charge.
    agent: String,
}

#[cfg(feature = "cables-mcp")]
impl Desk {
    /// The desk of this Station's charge, kept by `agent`.
    pub(super) fn new(state: DaemonState, agent: impl Into<String>) -> Self {
        Self {
            state,
            agent: agent.into(),
        }
    }

    /// Checks that this Station still runs its charge.
    fn admit(&self) -> Result<(), String> {
        if self.state.charges.is_live(&station_scope(&self.state)) {
            Ok(())
        } else {
            Err("this Station no longer runs its charge".to_owned())
        }
    }

    /// The charge's settings as the roster has them now.
    fn settings(&self) -> Option<crate::directorates::m::charges::Charge> {
        self.state
            .roster
            .charges()
            .get(&station_scope(&self.state))
            .cloned()
    }

    /// Who asks, for a call made under the charge's Run `run`.
    fn asker<'a>(&'a self, run: &'a str) -> hygiene::Asker<'a> {
        hygiene::Asker {
            agent: &self.agent,
            run,
        }
    }

    /// Everything the charge may read about its Station. Blocks; call from `spawn_blocking`.
    pub(super) fn report(&self) -> Result<StationState, String> {
        self.admit()?;
        let quiet_after = self
            .settings()
            .and_then(|charge| charge.settings.stall_after.get());
        Ok(report::station_state(&self.state, quiet_after))
    }

    /// The Registry's limits on Reports.
    pub(super) fn report_limits(&self) -> crate::directorates::registry::limits::ReportLimits {
        self.state.intake.limits().reports.clone()
    }

    /// Files a report in the Station's logbook: the next check-in publishes it. `filing` is the
    /// same report as a Report of this Station; why it was not stored is the second answer, and
    /// leaves the logbook as filed. `run` is the charge's Run once known. Blocks; call from
    /// `spawn_blocking`.
    pub(super) fn file(
        &self,
        summary: String,
        findings: Vec<StationFinding>,
        filing: crate::directorates::m::reports::ReportFiling,
        run: Option<String>,
    ) -> Result<(StationReport, Result<String, String>), String> {
        use crate::directorates::registry::reports::{ReportFiler, ReportScope};

        self.admit()?;
        let state = &self.state;
        let scope = station_scope(state);
        let mut report = StationReport::new(state.station_id.clone(), summary, state.clock.now())
            .with_findings(findings);
        report.agent = Some(self.agent.clone());
        let operation = self
            .settings()
            .and_then(|charge| charge.settings.operation)
            .unwrap_or_default();
        state
            .intake
            .file_station_report(&report)
            .map_err(|error| format!("the report was not filed: {error}"))?;
        let mut event = TimelineEvent::new(
            "head_of_station",
            "reported",
            operation,
            state.clock.now() as f64,
        )
        .station(state.station_id.clone())
        .detail("findings", report.findings.len().to_string())
        .actor(self.agent.clone());
        if let Some(worst) = report.worst() {
            event = event.detail("worst", worst.as_str());
        }
        if let Err(error) = state.intake.record_event(&event) {
            eprintln!("ai6: Head of Station report event was not recorded: {error}");
        }
        // A charge has no Mission and files as itself.
        let filer = ReportFiler::Charge {
            agent: self.agent.clone(),
            run: run.unwrap_or_else(|| scope.clone()),
            scope,
        };
        let stored = filing
            .into_report(
                ReportScope::Station(state.station_id.clone()),
                filer,
                Vec::new(),
                state.clock.now() as f64,
                &state.intake.limits().reports,
                &state
                    .config
                    .report_settings()
                    .unwrap_or_else(|_| state.roster.settings().reports.clone()),
            )
            .and_then(|stored| {
                state
                    .intake
                    .file_report(&stored)
                    .map(|()| stored.id)
                    .map_err(|error| format!("the Station report was not stored: {error}"))
            });
        Ok((report, stored))
    }

    /// This Station's ID.
    pub(super) fn station(&self) -> &str {
        &self.state.station_id
    }

    /// Applies `action` (`caches` or `remove`) to the safe house of `target`, within the
    /// limits [`hygiene`] fixes. `run` is the charge's Run, which a green ink request names.
    /// Blocks; call from `spawn_blocking`.
    pub(super) fn tidy(
        &self,
        run: &str,
        target: &str,
        action: &str,
        reason: Option<String>,
    ) -> Result<serde_json::Value, String> {
        self.admit()?;
        let action = action.parse()?;
        let tidied = hygiene::tidy(&self.state, &self.asker(run), target, action, reason)?;
        serde_json::to_value(tidied).map_err(|error| error.to_string())
    }

    /// Checks a Theater against its declaration, or with `apply` runs the steps it declares.
    /// Blocks on the Theater's transport; call from `spawn_blocking`.
    pub(super) fn provision(
        &self,
        theater: &str,
        apply: bool,
    ) -> Result<serde_json::Value, String> {
        self.admit()?;
        let provisioned =
            provision::provision_theater(&self.state, &self.asker(""), theater, apply)?;
        serde_json::to_value(provisioned).map_err(|error| error.to_string())
    }

    /// Answers an Operation's request on behalf of the charge, as the Station can provide it.
    /// Blocks; call from `spawn_blocking`.
    pub(super) fn answer(
        &self,
        answering: super::resources::Answering,
    ) -> Result<serde_json::Value, String> {
        self.admit()?;
        super::resources::answer(&self.state, &self.agent, answering)
    }

    /// Gives up the claim this Station holds on a stalled Mission's brief, so the Registry
    /// queues it again or files it under the Operation's `on_lost`. Blocks; call from
    /// `spawn_blocking`.
    pub(super) fn surrender(
        &self,
        mission: &str,
        reason: Option<String>,
    ) -> Result<serde_json::Value, String> {
        self.admit()?;
        let quiet_after = self
            .settings()
            .and_then(|charge| charge.settings.stall_after.get());
        let surrendered =
            claims::surrender(&self.state, &self.asker(""), mission, reason, quiet_after)?;
        serde_json::to_value(surrendered).map_err(|error| error.to_string())
    }
}

/// The scope of this Station's charge.
#[cfg(feature = "cables-mcp")]
fn station_scope(state: &DaemonState) -> String {
    crate::directorates::m::charges::ChargeScope::Station(state.station_id.clone()).to_string()
}

#[cfg(all(test, feature = "cables-mcp"))]
pub(super) mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use std::collections::BTreeMap;

    use super::super::tests::{start_mission_by_id, test_state, wait_for_state};
    use super::*;
    use crate::MissionId;
    use crate::directorates::m::mission::MissionState;
    use crate::directorates::m::roster::Roster;
    use crate::directorates::q::safehouse::SafeHouseReport;
    use crate::directorates::registry::store::{Brief, DEFAULT_LEASE};

    pub(in super::super) const AGENTS: &str = "[agent.M]\nhandler = \"mock-recorder\"\nclearance = \"field\"\n\
         roles = [\"analyst\"]\n\
         [agent.M.limits]\nstall_timeout = 300\n\
         [role.analyst]\npurpose = \"Analyse.\"\npolicy = \"Report.\"\n";

    /// A Station `test` with Operation `G`, whose Missions get mock safe houses, and Operation
    /// `H`; its charge (`station:test`) is kept by Agent `M`.
    pub(in super::super) fn station(root: &Path) -> DaemonState {
        let mut state = test_state(root, "", AGENTS);
        let checkout = root.join("checkout");
        std::fs::write(
            root.join("operations.toml"),
            format!(
                "[operation.G]\npath = {checkout:?}\nworkspace = {{ provider = \"mock-safehouse\" }}\n\
                 [operation.H]\npath = {checkout:?}\n",
                checkout = checkout.to_string_lossy()
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("charges.toml"),
            "[charge.\"station:test\"]\nagent = \"M\"\nwake_every = \"none\"\n",
        )
        .unwrap();
        state.roster = Arc::new(Roster::load(root).unwrap());
        state
    }

    /// [`station`], with its charge claimed and running here.
    pub(in super::super) async fn live_station(root: &Path) -> DaemonState {
        let state = station(root);
        super::super::charges::tend(&state, false, true).await;
        assert!(
            state.charges.is_live("station:test"),
            "the charge runs here"
        );
        state
    }

    /// Leases a brief for `mission_id` of Operation `operation` to this Station, as Headquarters
    /// does when it places a brief here, and records the Mission, kept by Agent `M`.
    pub(in super::super) fn lease_mission(state: &DaemonState, operation: &str, mission_id: &str) {
        let brief: Brief = serde_json::from_value(serde_json::json!({
            "title": "work", "id": mission_id, "source": "test",
            "operation": operation, "stations": [state.station_id],
        }))
        .unwrap();
        state.intake.submit_brief(&brief).unwrap();
        let meta = BTreeMap::from([
            ("mission_id".to_owned(), serde_json::json!(mission_id)),
            ("operation".to_owned(), serde_json::json!(operation)),
            ("agent".to_owned(), serde_json::json!("M")),
        ]);
        state
            .intake
            .claim_with_lease(Some(&brief.key()), &state.station_id, DEFAULT_LEASE, meta)
            .unwrap()
            .expect("the brief is leased to this Station");
        state
            .missions
            .lock()
            .unwrap()
            .create(
                MissionId::try_new(mission_id).unwrap(),
                Some(operation.to_owned()),
                "work".to_owned(),
                Some("M".to_owned()),
                None,
            )
            .unwrap();
    }

    /// Starts a leased Mission and waits until its mock Run reports itself blocked.
    pub(in super::super) async fn start(state: &DaemonState, mission_id: &str) {
        start_mission_by_id(state, mission_id).await;
        wait_for_state(state, mission_id, MissionState::Blocked).await;
    }

    /// The desk of the Station's charge.
    pub(in super::super) fn desk(state: &DaemonState) -> Desk {
        Desk::new(state.clone(), "M")
    }

    /// Records a Mission of Operation `G` that ends `ended` (or still works, for
    /// `Working`) with a kept safe house under `root`, holding `unexported` changes and a
    /// `target/` cache. Returns the safe house's path.
    pub(in super::super) fn kept_safe_house(
        state: &DaemonState,
        root: &Path,
        mission_id: &str,
        ended: MissionState,
        unexported: &[&str],
    ) -> PathBuf {
        let path = root.join("safe-houses").join(mission_id);
        std::fs::create_dir_all(path.join("target/debug")).unwrap();
        std::fs::write(path.join("target/debug/app"), [0_u8; 64]).unwrap();
        std::fs::write(path.join("main.rs"), "fn main() {}\n").unwrap();
        if !unexported.is_empty() {
            std::fs::write(path.join(".mock-unexported"), unexported.join("\n")).unwrap();
        }
        let mut missions = state.missions.lock().unwrap();
        missions
            .create(
                MissionId::try_new(mission_id).unwrap(),
                Some("G".to_owned()),
                format!("work for {mission_id}"),
                Some("M".to_owned()),
                None,
            )
            .unwrap();
        missions
            .transition(mission_id, MissionState::Working)
            .unwrap();
        missions
            .set_safe_house(
                mission_id,
                SafeHouseReport {
                    path: path.to_string_lossy().into_owned(),
                    branch: Some(format!("ai6/{mission_id}")),
                    base: Some("base0".to_owned()),
                    kept: true,
                    changes: unexported.iter().map(|line| (*line).to_owned()).collect(),
                },
            )
            .unwrap();
        missions.transition(mission_id, ended).unwrap();
        path
    }

    #[tokio::test]
    async fn the_charge_reads_its_station_and_files_a_report_the_check_in_publishes() {
        let root = tempfile::tempdir().unwrap();
        let state = live_station(root.path()).await;
        let desk = desk(&state);
        let pid = std::process::id();
        let kept = format!("hos-kept-{pid}");
        kept_safe_house(
            &state,
            root.path(),
            &kept,
            MissionState::Done,
            &["abc1234 add the parser"],
        );
        let working = format!("hos-working-{pid}");
        kept_safe_house(&state, root.path(), &working, MissionState::Working, &[]);

        let report = serde_json::to_value(desk.report().unwrap()).unwrap();
        assert_eq!(report["station"], "test");
        assert_eq!(report["capacity"], 4);
        let live = report["missions"].as_array().unwrap();
        assert!(live.iter().any(|m| m["mission"] == working.as_str()));
        assert!(live.iter().all(|m| m["mission"] != kept.as_str()));
        let house = &report["safe_houses"][0];
        assert_eq!(house["mission"], kept.as_str());
        assert_eq!(house["state"], "done");
        assert_eq!(house["live"], false);
        assert_eq!(house["exported"], false);
        assert_eq!(house["unexported"][0], "abc1234 add the parser");
        assert_eq!(house["caches"][0]["path"], "target");
        assert_eq!(house["caches"][0]["size_bytes"], 64);
        assert_eq!(report["limits"]["cache_dirs"][0], "target");
        assert_eq!(report["queue"]["unclaimed"], 0);
        assert!(
            !report.to_string().contains("AI6_CABLE_MCP_TOKEN"),
            "no secrets"
        );

        let findings = vec![StationFinding::new(
            crate::directorates::registry::logbook::StationFindingKind::SafeHouse,
            crate::directorates::registry::logbook::Severity::Warning,
            format!("{kept} holds one unexported commit"),
        )];
        let filing = serde_json::from_value(serde_json::json!({
            "title": "One safe house holds unexported work.",
            "text": "details",
            "kind": "station",
            "severity": "warning",
        }))
        .unwrap();
        let (logged, stored) = desk
            .file(
                "One safe house holds unexported work.".to_owned(),
                findings,
                filing,
                Some("run-1".to_owned()),
            )
            .unwrap();
        assert_eq!(logged.agent.as_deref(), Some("M"));
        // The report is also a Report of this Station, filed by the charge: no Mission.
        let stored = state.intake.report(&stored.unwrap()).unwrap().unwrap();
        assert_eq!(
            stored.scope,
            crate::directorates::registry::reports::ReportScope::Station("test".to_owned())
        );
        assert_eq!(stored.title, "One safe house holds unexported work.");
        assert!(stored.links.is_empty());

        super::super::publish_station_check_in(&state);
        let record = state
            .intake
            .stations(std::time::Duration::MAX)
            .unwrap()
            .into_iter()
            .find(|record| record.station() == "test")
            .unwrap();
        let published = &record.info()[CHECK_IN_KEY];
        assert_eq!(
            published["summary"],
            "One safe house holds unexported work."
        );
        assert_eq!(published["findings"][0]["kind"], "safe-house");
        assert_eq!(published["agent"], "M");
        assert_eq!(state.intake.logbook("test").unwrap().len(), 1);

        // Once another Station runs the charge, the tools refuse.
        super::super::charges::close_all(&state).await;
        assert!(desk.report().unwrap_err().contains("no longer runs"));
    }
}
