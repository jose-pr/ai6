//! The harness policy on the Station: whether a Session's harness may load the settings its
//! working directory carries.

use std::path::Path;

use super::DaemonState;
use crate::directorates::m::harness::{HarnessPolicy, ProjectSettings};
use crate::directorates::m::roster::Roster;
use crate::directorates::registry::timeline::TimelineEvent;

const TIMELINE_KIND: &str = "harness";

/// Decides, for one start or resume, whether the harness may load its working directory's
/// own settings. A `pinned` Operation whose settings files differ from their pins, or sit
/// on a host the Station cannot read, starts with them ignored, and the reason is recorded
/// on the Operation's timeline. `on_station` says the harness runs on this Station's host.
pub(super) fn project_settings(
    state: &DaemonState,
    operation: Option<&str>,
    mission_id: &str,
    working_directory: &str,
    on_station: bool,
) -> bool {
    let policy = operation
        .and_then(|operation| state.roster.operation(operation).ok())
        .map(|operation| operation.harness.clone())
        .unwrap_or_default();
    let decision = if policy.project_settings == ProjectSettings::Pinned && !on_station {
        Err(
            "the working directory is on another host, where the Station cannot read its \
             settings files"
                .to_owned(),
        )
    } else {
        policy.check(Path::new(working_directory))
    };
    match decision {
        Ok(()) => true,
        Err(reason) => {
            if policy.project_settings == ProjectSettings::Pinned {
                record_ignored(state, operation, mission_id, &reason);
            }
            false
        }
    }
}

fn record_ignored(state: &DaemonState, operation: Option<&str>, mission_id: &str, reason: &str) {
    eprintln!("ai6: Mission {mission_id:?} starts with its project settings ignored: {reason}");
    let Some(operation) = operation else {
        return;
    };
    let event = TimelineEvent::new(
        TIMELINE_KIND,
        "project-settings-ignored",
        operation,
        state.clock.now() as f64,
    )
    .station(state.station_id.clone())
    .mission(mission_id)
    .detail("reason", reason);
    if let Err(error) = state.intake.record_event(&event) {
        eprintln!("ai6: harness timeline event was not recorded: {error}");
    }
}

/// Lists, for the operator, each Operation whose harness policy loads project settings
/// unchecked.
pub(super) fn log_weakenings(roster: &Roster) {
    for operation in roster.operations() {
        if let Some(weakened) = HarnessPolicy::weakening(&operation.harness) {
            eprintln!("ai6: Operation {} is weakened: {weakened}", operation.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_state;
    use super::*;

    fn state_with(root: &Path, harness: &str) -> DaemonState {
        test_state(root, harness, "[agent.007]\nhandler = \"mock-agent\"\n")
    }

    #[test]
    fn a_changed_settings_file_under_pinned_falls_back_to_ignore_with_a_recorded_reason() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("checkout");
        std::fs::create_dir_all(checkout.join(".claude")).unwrap();
        let settings = br#"{"permissions":{}}"#;
        std::fs::write(checkout.join(".claude/settings.json"), settings).unwrap();
        let pin = coqui_peer::PeerId::of(settings);
        let state = state_with(
            root.path(),
            &format!(
                "[operation.G.harness]\nproject_settings = \"pinned\"\n\
                 pins = {{ \".claude/settings.json\" = \"{pin}\" }}\n"
            ),
        );
        let directory = checkout.to_str().unwrap();
        assert!(project_settings(&state, Some("G"), "m-1", directory, true));
        assert!(
            !project_settings(&state, Some("G"), "m-1", directory, false),
            "files on another host cannot be checked"
        );

        std::fs::write(
            checkout.join(".claude/settings.json"),
            br#"{"hooks":{"PreToolUse":[]}}"#,
        )
        .unwrap();
        assert!(!project_settings(&state, Some("G"), "m-2", directory, true));
        let events = state.intake.timeline_events("G").unwrap();
        let ignored: Vec<_> = events
            .iter()
            .filter(|event| event.action == "project-settings-ignored")
            .collect();
        assert_eq!(ignored.len(), 2);
        let reason = &ignored
            .iter()
            .find(|event| event.mission.as_deref() == Some("m-2"))
            .unwrap()
            .detail["reason"];
        assert!(
            reason.contains(&format!(
                ".claude/settings.json: expected {pin}, found sha256:"
            )),
            "{reason}"
        );
    }

    #[test]
    fn project_settings_are_ignored_by_default_and_loaded_only_when_an_operation_says_so() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().to_str().unwrap().to_owned();
        let ignoring = state_with(root.path(), "");
        assert!(!project_settings(
            &ignoring,
            Some("G"),
            "m",
            &directory,
            true
        ));
        assert!(!project_settings(&ignoring, None, "m", &directory, true));
        assert!(ignoring.intake.timeline_events("G").unwrap().is_empty());

        let loading = state_with(
            root.path(),
            "[operation.G.harness]\nproject_settings = \"load\"\n",
        );
        assert!(project_settings(&loading, Some("G"), "m", &directory, true));
    }
}
