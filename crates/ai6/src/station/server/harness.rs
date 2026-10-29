//! The harness policy on the Station: whether a Session's harness may load the configuration
//! its working directory carries.

use std::path::PathBuf;

use super::DaemonState;
use crate::directorates::m::harness::{ProjectDecision, ProjectSettings};
use crate::directorates::m::roster::Roster;
use crate::directorates::registry::timeline::TimelineEvent;

const TIMELINE_KIND: &str = "harness";

/// Decides, for one start or resume, what the harness may load from its working directory.
/// A `pinned` Operation reads the directory's manifest on the blocking pool and records the
/// manifest's digest on the Operation's timeline, as `project-settings-pinned` when it
/// matches and `project-settings-ignored` with the reason when it does not or cannot be
/// read; a harness that does not run on this Station's host (`on_station` false) starts as
/// `ignore`, since the Station cannot read its files.
pub(super) async fn project_settings(
    state: &DaemonState,
    operation: Option<&str>,
    mission_id: &str,
    working_directory: &str,
    on_station: bool,
) -> ProjectDecision {
    let policy = operation
        .and_then(|operation| state.roster.operation(operation).ok())
        .map(|operation| operation.harness.clone())
        .unwrap_or_default();
    if policy.project_settings != ProjectSettings::Pinned {
        return policy.decide(
            std::path::Path::new(working_directory),
            &state.station.limits().harness,
        );
    }
    let (state, operation, mission_id) = (
        state.clone(),
        operation.map(str::to_owned),
        mission_id.to_owned(),
    );
    let root = PathBuf::from(working_directory);
    tokio::task::spawn_blocking(move || {
        let decision = if on_station {
            policy.decide(&root, &state.station.limits().harness)
        } else {
            refused("the working directory is on another host, where the Station cannot read it")
        };
        record(&state, operation.as_deref(), &mission_id, &decision);
        decision
    })
    .await
    .unwrap_or_else(|_| refused("the check of the project configuration failed"))
}

fn refused(reason: &str) -> ProjectDecision {
    ProjectDecision {
        load: false,
        mcp: false,
        manifest: None,
        refused: Some(reason.to_owned()),
    }
}

fn record(
    state: &DaemonState,
    operation: Option<&str>,
    mission_id: &str,
    decision: &ProjectDecision,
) {
    let manifest = decision
        .manifest
        .map_or_else(|| "none".to_owned(), |digest| digest.to_string());
    let action = match &decision.refused {
        Some(reason) => {
            eprintln!(
                "ai6: Mission {mission_id:?} starts with its project settings ignored: {reason}"
            );
            "project-settings-ignored"
        }
        None => "project-settings-pinned",
    };
    let Some(operation) = operation else {
        return;
    };
    let mut event = TimelineEvent::new(TIMELINE_KIND, action, operation, state.clock.now() as f64)
        .station(state.station_id.clone())
        .mission(mission_id)
        .detail("manifest", manifest);
    if let Some(reason) = &decision.refused {
        event = event.detail("reason", reason.clone());
    }
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: harness timeline event was not recorded: {error}");
    }
}

/// Lists, for the operator, each way an Operation's harness policy is looser than the
/// default.
pub(super) fn log_weakenings(roster: &Roster) {
    for operation in roster.operations() {
        for weakened in operation.harness.weakenings() {
            eprintln!("ai6: Operation {} is weakened: {weakened}", operation.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::tests::test_state;
    use super::*;
    use crate::directorates::m::harness::ProjectManifest;

    fn state_with(root: &Path, harness: &str) -> DaemonState {
        test_state(root, harness, "[agent.007]\nhandler = \"mock-agent\"\n")
    }

    #[tokio::test]
    async fn a_changed_manifest_under_pinned_falls_back_to_ignore_with_a_recorded_reason() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        std::fs::create_dir_all(checkout.join(".claude")).unwrap();
        std::fs::write(
            checkout.join(".claude/settings.json"),
            br#"{"permissions":{}}"#,
        )
        .unwrap();
        let pin = ProjectManifest::read(&checkout).unwrap().digest();
        let state = state_with(
            root.path(),
            &format!("[operation.G.harness]\nproject_settings = \"pinned\"\npin = \"{pin}\"\n"),
        );
        let directory = checkout.to_str().unwrap();
        let loaded = project_settings(&state, Some("G"), "m-1", directory, true).await;
        assert!(loaded.load);
        assert!(
            !project_settings(&state, Some("G"), "m-1", directory, false)
                .await
                .load,
            "files on another host cannot be checked"
        );

        std::fs::write(
            checkout.join(".claude/settings.json"),
            br#"{"hooks":{"PreToolUse":[]}}"#,
        )
        .unwrap();
        let refused = project_settings(&state, Some("G"), "m-2", directory, true).await;
        assert!(!refused.load);
        let events = state.intake.timeline_events("G").unwrap();
        let pinned = events
            .iter()
            .find(|event| event.action == "project-settings-pinned")
            .expect("the match is recorded with its manifest");
        assert_eq!(pinned.detail["manifest"], pin.to_string());
        let ignored = events
            .iter()
            .find(|event| {
                event.action == "project-settings-ignored"
                    && event.mission.as_deref() == Some("m-2")
            })
            .unwrap();
        assert!(
            ignored.detail["reason"].contains(&format!("expected {pin}")),
            "{:?}",
            ignored.detail
        );
        assert_eq!(
            ignored.detail["manifest"],
            refused.manifest.unwrap().to_string()
        );
    }

    #[tokio::test]
    async fn project_settings_are_ignored_by_default_and_loaded_only_when_an_operation_says_so() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().to_str().unwrap().to_owned();
        let ignoring = state_with(root.path(), "");
        assert!(
            !project_settings(&ignoring, Some("G"), "m", &directory, true)
                .await
                .load
        );
        assert!(
            !project_settings(&ignoring, None, "m", &directory, true)
                .await
                .load
        );
        assert!(ignoring.intake.timeline_events("G").unwrap().is_empty());

        let loading = state_with(
            root.path(),
            "[operation.G.harness]\nproject_settings = \"load\"\n",
        );
        let decision = project_settings(&loading, Some("G"), "m", &directory, true).await;
        assert!(decision.load && decision.mcp);
    }
}
