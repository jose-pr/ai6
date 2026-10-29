//! `provision_theater`: keeping the Theaters a Station reaches as their operator declared them.
//!
//! A check (`apply = false`) only looks. Applying runs [`provision`] with the Theater's own
//! `requires` and `provision` settings: the steps the operator declared, idempotent, and
//! nothing else, which is why a round may run them unattended. What the declaration does
//! not cover is reported, never acted on. A Theater made for one Mission and an ephemeral
//! template are refused: they are fitted out when they are made. Applying is refused while a
//! Mission of this Station works on the Theater. What a failed command printed stays out of
//! the answer: it may carry anything the Theater holds, and `ai6 theater provision` shows it
//! to an operator.

use serde::Serialize;

use crate::directorates::m::mission::Mission;
use crate::directorates::m::theaters::TheaterAccess;
use crate::directorates::q::provision::{self, ProvisionStep, StepStatus};
use crate::directorates::q::transport::describe;
use crate::directorates::registry::store::AuditRecord;

use super::super::DaemonState;

/// What a check or a run found and did.
#[derive(Debug, Clone, Serialize)]
pub(super) struct Provisioned {
    theater: String,
    /// Whether steps were applied, or only checked.
    applied: bool,
    steps: Vec<ProvisionStep>,
    /// Whether every step is as declared now.
    fit: bool,
}

/// Checks the Theater `id` against its declaration, or with `apply` fits it out.
///
/// # Errors
///
/// A Theater this Station does not reach, one made for a Mission, an ephemeral template, or
/// one without a transport.
pub(super) fn provision_theater(
    state: &DaemonState,
    round: &Mission,
    id: &str,
    apply: bool,
) -> Result<Provisioned, String> {
    let theater = state
        .transports
        .theater(id)
        .ok_or_else(|| format!("this Station does not reach a Theater {id:?}"))?;
    if let TheaterAccess::Ephemeral(_) = theater.access {
        return Err(format!(
            "Theater {id:?} is an ephemeral template: each Theater made from it is fitted out when it is made"
        ));
    }
    if state.transports.made().iter().any(|(made, _)| made == id) {
        return Err(format!(
            "Theater {id:?} was made for one Mission and fitted out then; it is not provisioned again"
        ));
    }
    if apply {
        let busy = working_on(state, id);
        if !busy.is_empty() {
            return Err(format!(
                "Missions work on Theater {id:?} ({}): it is fitted out only while none does",
                busy.join(", ")
            ));
        }
    }
    let client = state.transports.client(id)?;
    let mut steps =
        provision::provision(&client, &theater, !apply, &state.station.limits().provision);
    for step in &mut steps {
        if step.status == StepStatus::Failed {
            step.detail = format!(
                "the step failed; what the Theater printed is withheld here (run `ai6 theater \
                 provision {id}` to see it)"
            );
        }
    }
    if apply {
        // What placement and the next station_report see, without waiting for the next probe.
        if let Ok(capabilities) = client.probe().map_err(|error| describe(&error)) {
            let report = provision::probe(
                &client,
                &capabilities,
                theater.requires.as_ref(),
                theater.workdir.as_deref(),
                &state.station.limits().provision,
            );
            state.transports.set_report(id, report);
        }
        let changed: Vec<&str> = steps
            .iter()
            .filter(|step| step.status == StepStatus::Changed)
            .map(|step| step.name.as_str())
            .collect();
        if !changed.is_empty() {
            audit(state, round, &format!("{id} {}", changed.join(" ")));
        }
    }
    Ok(Provisioned {
        theater: id.to_owned(),
        applied: apply,
        fit: steps
            .iter()
            .all(|step| matches!(step.status, StepStatus::Ok | StepStatus::Changed)),
        steps,
    })
}

/// This Station's Missions that are not over and run on the Theater `id`.
fn working_on(state: &DaemonState, id: &str) -> Vec<String> {
    let Ok(missions) = state.missions.lock() else {
        return vec!["the Mission book is unavailable".to_owned()];
    };
    missions
        .list()
        .into_iter()
        .filter(|mission| !super::super::is_terminal_state(mission.state))
        .filter(|mission| {
            missions
                .operation(mission.mission_id.as_str())
                .and_then(|manifest| manifest.theater_id)
                .as_deref()
                == Some(id)
        })
        .map(|mission| mission.mission_id.to_string())
        .collect()
}

fn audit(state: &DaemonState, round: &Mission, target: &str) {
    let mut end = target.len().min(500);
    while !target.is_char_boundary(end) {
        end -= 1;
    }
    let record = AuditRecord {
        id: format!(
            "head_of_station.provisioned-{}",
            uuid::Uuid::new_v4().simple()
        ),
        actor: format!("agent:{}", round.agent_id.as_deref().unwrap_or("unknown")),
        action: "head_of_station.provisioned".to_owned(),
        operation: round.operation_id.clone().unwrap_or_default(),
        resource: Some(format!("station:{}", state.station_id)),
        target: target[..end].to_owned(),
        at: state.clock.now() as f64,
    };
    if let Err(error) = state.intake.record_audit(&record) {
        eprintln!("ai6: Head of Station audit record was not written: {error}");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};

    use guatiao::library::ProviderError;
    use guatiao::{Bytes, FromValue, Status, ToValue, Value};

    use super::super::tests::{call, lease_sleeper_round, start, station};
    use crate::directorates::m::theaters::{SshAccess, Theater, Theaters};
    use crate::directorates::q::provision::{ProvisionSpec, Requirements};
    use crate::directorates::q::transport::{Capabilities, Output, RunSpec, Transport};
    use crate::station::transports::Transports;

    /// A host that answers the provisioning probe for the programs it has, installs one
    /// when its declared install command runs, and can lose one again.
    struct ProvisionFake {
        installed: Arc<Mutex<BTreeSet<String>>>,
    }

    fn internal(error: impl std::fmt::Display) -> ProviderError {
        ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string())
    }

    impl Transport for ProvisionFake {
        fn capabilities(&self) -> Result<Value, ProviderError> {
            Capabilities {
                run: true,
                os: Some("linux".to_owned()),
                ..Capabilities::default()
            }
            .to_value()
            .map_err(internal)
        }

        fn run(&self, spec: &Value) -> Result<Value, ProviderError> {
            let spec = RunSpec::from_value(spec).map_err(internal)?;
            let mut installed = self.installed.lock().unwrap();
            let stdout = match spec.argv.as_slice() {
                [sh, c, script, _, _dir, names @ ..]
                    if sh == "sh" && c == "-c" && script.starts_with("ai6_dir=$1") =>
                {
                    let mut lines = vec!["os\tLinux".to_owned()];
                    for name in names {
                        lines.push(if installed.contains(name) {
                            format!("found\t{name}\t{name} 1.2.0")
                        } else {
                            format!("missing\t{name}")
                        });
                    }
                    lines.push("free_kb\t1048576".to_owned());
                    lines.join("\n") + "\n"
                }
                [sh, c, command] if sh == "sh" && c == "-c" && command == "install-buildtool" => {
                    installed.insert("buildtool".to_owned());
                    String::new()
                }
                [sh, c, command] if sh == "sh" && c == "-c" && command == "install-leaky" => {
                    return Output {
                        code: Some(1),
                        stderr: Bytes(b"curl: token=SECRET-1234 rejected".to_vec()),
                        ..Output::default()
                    }
                    .to_value()
                    .map_err(internal);
                }
                // Resource samples and anything else answer nothing in particular.
                _ => String::new(),
            };
            Output {
                code: Some(0),
                stdout: Bytes(stdout.into_bytes()),
                ..Output::default()
            }
            .to_value()
            .map_err(internal)
        }
    }

    fn declared() -> Theater {
        Theater::new(
            "build-box",
            crate::directorates::m::theaters::TheaterAccess::Ssh(SshAccess::new("b.example")),
        )
        .with_requirements(Requirements::default().with_tool("buildtool>=1.2".parse().unwrap()))
        .with_provision(ProvisionSpec::default().with_install("buildtool", "install-buildtool"))
    }

    fn leaky() -> Theater {
        Theater::new(
            "leaky-box",
            crate::directorates::m::theaters::TheaterAccess::Ssh(SshAccess::new("l.example")),
        )
        .with_requirements(Requirements::default().with_tool("leaky".parse().unwrap()))
        .with_provision(ProvisionSpec::default().with_install("leaky", "install-leaky"))
    }

    fn statuses(answer: &serde_json::Value) -> Vec<(String, String)> {
        answer["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| {
                (
                    step["name"].as_str().unwrap().to_owned(),
                    step["status"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn a_drifted_theater_is_fitted_out_again_on_a_round_and_then_checks_ok() {
        let root = tempfile::tempdir().unwrap();
        let mut state = station(root.path());
        let installed = Arc::new(Mutex::new(BTreeSet::new()));
        let mut theaters = Theaters::default();
        theaters.insert(declared()).unwrap();
        theaters.insert(leaky()).unwrap();
        state.transports = Arc::new(
            Transports::build(&theaters, &crate::station::plugins::Plugins::scan(&[]))
                .with_transport(
                    declared(),
                    Box::new(ProvisionFake {
                        installed: installed.clone(),
                    }),
                )
                .with_transport(
                    leaky(),
                    Box::new(ProvisionFake {
                        installed: Arc::default(),
                    }),
                ),
        );
        let round = format!("hos-provision-round-{}", std::process::id());
        lease_sleeper_round(&state, &round);
        start(&state, &round).await;
        let provision = |apply: bool| {
            let (state, round) = (state.clone(), round.clone());
            async move {
                let text = call(
                    &state,
                    &round,
                    "provision_theater",
                    serde_json::json!({ "theater": "build-box", "apply": apply }),
                )
                .await
                .unwrap();
                serde_json::from_str::<serde_json::Value>(&text)
                    .unwrap_or_else(|_| panic!("not JSON: {text}"))
            }
        };
        let step = |status: &str| vec![("buildtool".to_owned(), status.to_owned())];

        // A check changes nothing.
        let checked = provision(false).await;
        assert_eq!(statuses(&checked), step("would-change"), "{checked}");
        assert_eq!(checked["fit"], false);
        assert!(installed.lock().unwrap().is_empty());

        // A round fits it out, then finds it fit.
        let applied = provision(true).await;
        assert_eq!(statuses(&applied), step("changed"), "{applied}");
        assert_eq!(applied["fit"], true);
        assert_eq!(statuses(&provision(true).await), step("ok"));

        // It drifts: the next round puts it back, and the run is audited.
        installed.lock().unwrap().clear();
        assert_eq!(statuses(&provision(true).await), step("changed"));
        assert_eq!(statuses(&provision(false).await), step("ok"));
        let audited = state
            .intake
            .audit_records()
            .unwrap()
            .into_iter()
            .filter(|record| record.action == "head_of_station.provisioned")
            .collect::<Vec<_>>();
        assert_eq!(audited.len(), 2);
        assert_eq!(audited[0].target, "build-box buildtool");
        assert_eq!(audited[0].resource.as_deref(), Some("station:test"));

        // What a failed command printed stays out of the answer.
        let failed = call(
            &state,
            &round,
            "provision_theater",
            serde_json::json!({ "theater": "leaky-box", "apply": true }),
        )
        .await
        .unwrap();
        assert!(failed.contains("\"failed\""), "{failed}");
        assert!(!failed.contains("SECRET-1234"), "{failed}");
        assert!(failed.contains("withheld"), "{failed}");

        // While a Mission of this Station works on the Theater it is only checked.
        let working = format!("hos-provision-working-{}", std::process::id());
        {
            let mut missions = state.missions.lock().unwrap();
            missions
                .create(
                    crate::MissionId::try_new(&working).unwrap(),
                    Some("G".to_owned()),
                    "build".to_owned(),
                    Some("M".to_owned()),
                    None,
                )
                .unwrap();
            missions
                .set_operation(
                    &working,
                    crate::directorates::m::mission::LaunchManifest {
                        handler_provider: "mock-recorder".to_owned(),
                        terminal_provider: "mock-terminal".to_owned(),
                        working_directory: "/work".to_owned(),
                        model: None,
                        effort: None,
                        cover: None,
                        station_id: Some("test".to_owned()),
                        theater_id: Some("build-box".to_owned()),
                        mode: Default::default(),
                        session: None,
                        runner: None,
                    },
                )
                .unwrap();
            missions
                .transition(
                    &working,
                    crate::directorates::m::mission::MissionState::Working,
                )
                .unwrap();
        }
        let busy = call(
            &state,
            &round,
            "provision_theater",
            serde_json::json!({ "theater": "build-box", "apply": true }),
        )
        .await
        .unwrap();
        assert!(busy.contains(&working), "{busy}");
        assert_eq!(
            statuses(&provision(false).await),
            step("ok"),
            "a check still runs"
        );

        // A Theater this Station does not reach is refused.
        let refused = call(
            &state,
            &round,
            "provision_theater",
            serde_json::json!({ "theater": "elsewhere", "apply": true }),
        )
        .await
        .unwrap();
        assert!(refused.contains("does not reach"), "{refused}");
        state.stopping.store(true, Ordering::Release);
    }
}
