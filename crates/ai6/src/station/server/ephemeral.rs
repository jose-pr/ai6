//! Ephemeral Theaters: a fresh Theater made for each Mission bound to a template, fitted out
//! by Q, and destroyed once the Mission is done.
//!
//! The Station makes the Theater when the Mission first starts and keeps it while the Mission
//! may still run: a Mission that is blocked, parked or retried comes back to the same Theater.
//! After each check-in it sweeps: a Theater whose Mission is done or abandoned is destroyed;
//! one whose Mission failed, or which no Mission here knows, is kept for inspection within
//! its template's bounds. A Station that restarts adopts the Theaters its templates' providers
//! still list.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::directorates::m::mission::{Mission, MissionState};
use crate::directorates::m::roster::Operation;
use crate::directorates::m::theaters::{
    DEFAULT_KEEP_FAILED, DEFAULT_KEEP_FAILED_HOURS, Theater, TheaterAccess, TransportAccess,
};
use crate::directorates::q::ephemeral::{MadeTheater, TheaterMaker, TheaterRequest};
use crate::directorates::q::provision::{self, StepStatus};
use crate::directorates::q::transport::describe;
use crate::station::transports::Made;

use super::DaemonState;

/// The Theater a Mission works in when its Operation is bound to an ephemeral template: the
/// one made for it, made and fitted out now when there is none. `None` when the Operation
/// is not bound to a template. Blocks on the Theater provider and the new Theater's transport.
pub(super) fn prepare(state: &DaemonState, mission: &Mission) -> Result<Option<String>, String> {
    let Some(operation) = mission
        .operation_id
        .as_deref()
        .and_then(|id| state.roster.operation(id).ok())
    else {
        return Ok(None);
    };
    let Some(template_id) = operation.theater.as_deref() else {
        return Ok(None);
    };
    let Some((template, maker)) = state
        .transports
        .templates()
        .into_iter()
        .find(|(template, _)| template.id == template_id)
    else {
        return Ok(None);
    };
    let maker =
        maker.map_err(|reason| format!("Theater {template_id:?} cannot be made: {reason}"))?;
    let mission_id = mission.mission_id.to_string();
    if let Some((id, _)) = state
        .transports
        .made()
        .into_iter()
        .find(|(_, made)| made.template == template.id && made.mission == mission_id)
    {
        return Ok(Some(id));
    }
    let made = maker
        .create(&TheaterRequest {
            name: theater_name(
                &template.id,
                &mission_id,
                state.station.limits().theaters.ephemeral_name_bytes,
            ),
            template: template.id.clone(),
            mission: mission_id,
        })
        .map_err(|error| {
            format!(
                "Theater {template_id:?} could not make a Theater: {}",
                describe(&error)
            )
        })?;
    let theater = adopt(state, &template, &made, Some(operation))?;
    fit_out(state, &theater)?;
    Ok(Some(theater.id))
}

/// Registers a Theater the provider made, reached by the transport it named. It inherits the
/// template's facts; without a `workdir` of its own it works in the Operation's path.
fn adopt(
    state: &DaemonState,
    template: &Theater,
    made: &MadeTheater,
    operation: Option<&Operation>,
) -> Result<Theater, String> {
    let settings: toml::Table = toml::from_str(&made.settings_toml)
        .map_err(|error| format!("the Theater provider's transport settings: {error}"))?;
    let mut access = TransportAccess::new(made.transport.clone());
    access.config = match serde_json::to_value(settings) {
        Ok(serde_json::Value::Object(config)) => config,
        _ => return Err("the Theater provider's transport settings are not a table".to_owned()),
    };
    let mut theater = template.clone();
    theater.id.clone_from(&made.name);
    theater.access = TheaterAccess::Transport(access);
    if theater.workdir.is_none() {
        theater.workdir = operation
            .and_then(|operation| operation.path.as_ref())
            .map(|path| path.to_string_lossy().replace('\\', "/"));
    }
    let client = state.plugins.theater_transport(&theater.access)?;
    state.transports.add_made(
        theater.clone(),
        client,
        Made {
            template: made.template.clone(),
            mission: made.mission.clone(),
            created_at: made.created_at,
        },
    );
    Ok(theater)
}

/// Fits a new Theater out as its template declares, then records what a probe finds so
/// placement and the launch see it at once.
fn fit_out(state: &DaemonState, theater: &Theater) -> Result<(), String> {
    let client = state.transports.client(&theater.id)?;
    if theater.requires.is_some() || theater.provision.is_some() || theater.runner.is_some() {
        let steps =
            provision::provision(&client, theater, false, &state.station.limits().provision);
        if let Some(step) = steps.iter().find(|step| step.status == StepStatus::Failed) {
            return Err(format!(
                "Theater {:?} could not be fitted out: {}: {}",
                theater.id, step.name, step.detail
            ));
        }
    }
    let capabilities = client.probe().map_err(|error| describe(&error))?;
    let report = provision::probe(
        &client,
        &capabilities,
        theater.requires.as_ref(),
        theater.workdir.as_deref(),
        &state.station.limits().provision,
    );
    state.transports.set_report(&theater.id, report);
    Ok(())
}

/// Adopts the Theaters each template's provider lists that this Station does not know, then
/// destroys those whose Mission is done or abandoned, and those kept past their template's
/// bounds. Blocks on the Theater providers.
pub(super) fn sweep(state: &DaemonState) {
    let templates = state.transports.templates();
    if templates.is_empty() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    for (template, maker) in templates {
        let Ok(maker) = maker else {
            continue;
        };
        match maker.list(&template.id) {
            Ok(listed) => {
                for made in listed {
                    if state.transports.theater(&made.name).is_none()
                        && let Err(error) = adopt(state, &template, &made, None)
                    {
                        eprintln!(
                            "ai6: Theater {:?} made from {:?} was not adopted: {error}",
                            made.name, template.id
                        );
                    }
                }
            }
            Err(error) => eprintln!(
                "ai6: the Theaters made from {:?} could not be listed: {}",
                template.id,
                describe(&error)
            ),
        }
        let (keep, hours) = match &template.access {
            TheaterAccess::Ephemeral(access) => (
                access.keep_failed.unwrap_or(DEFAULT_KEEP_FAILED),
                access
                    .keep_failed_hours
                    .unwrap_or(DEFAULT_KEEP_FAILED_HOURS),
            ),
            _ => continue,
        };
        let mut kept = Vec::new();
        for (id, made) in state.transports.made() {
            if made.template != template.id || in_use(state, &made.mission) {
                continue;
            }
            match mission_state(state, &made.mission) {
                Some(MissionState::Done | MissionState::Abandoned) => {
                    destroy(state, &maker, &id, "its Mission is over");
                }
                Some(MissionState::Failed | MissionState::Blown) | None => kept.push((id, made)),
                Some(_) => {}
            }
        }
        // Newest first: the oldest go once there are too many.
        kept.sort_by_key(|(_, made)| std::cmp::Reverse(made.created_at));
        for (index, (id, made)) in kept.iter().enumerate() {
            if index >= keep {
                destroy(
                    state,
                    &maker,
                    id,
                    "more failed Missions' Theaters are kept than allowed",
                );
            } else if now.saturating_sub(made.created_at) > hours.saturating_mul(3600) {
                destroy(state, &maker, id, "it was kept for as long as allowed");
            }
        }
    }
}

fn mission_state(state: &DaemonState, mission_id: &str) -> Option<MissionState> {
    state
        .missions
        .lock()
        .ok()?
        .get(mission_id)
        .map(|mission| mission.state)
}

/// Whether a Session of the Mission is starting or running on this Station.
fn in_use(state: &DaemonState, mission_id: &str) -> bool {
    state
        .starting
        .lock()
        .is_ok_and(|starting| starting.contains(mission_id))
        || state
            .sessions
            .lock()
            .is_ok_and(|sessions| sessions.contains_key(mission_id))
}

fn destroy(state: &DaemonState, maker: &TheaterMaker, id: &str, why: &str) {
    match maker.destroy(id) {
        Ok(()) => {
            state.transports.remove(id);
            eprintln!("ai6: destroyed Theater {id:?}: {why}");
        }
        Err(error) => eprintln!(
            "ai6: Theater {id:?} was not destroyed: {}",
            describe(&error)
        ),
    }
}

/// `<template>-<mission>` with anything but letters, digits, `.`, `_` and `-` as `-`, cut to
/// `bytes` with a digest of the whole when it is longer.
fn theater_name(template: &str, mission: &str, bytes: crate::bounds::Limit) -> String {
    use sha2::{Digest, Sha256};
    let name: String = format!("{template}-{mission}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if bytes.allows(name.len()) {
        return name;
    }
    let digest = Sha256::digest(name.as_bytes());
    let suffix: String = digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{}-{suffix}", &name[..bytes.or_max().saturating_sub(9)])
}

/// An in-process Theater provider and transport for tests: a "Theater" is a directory under
/// the provider's `root`, reached by the `mock-dir` transport.
#[cfg(test)]
pub(super) mod testing {
    use std::path::PathBuf;

    use guatiao::library::ProviderError;
    use guatiao::{FromValue, Provider, Status, ToValue, Value};

    use crate::directorates::q::ephemeral::{
        MadeTheater, MadeTheaters, TheaterProvider, TheaterProviderConfig, TheaterRequest,
    };
    use crate::directorates::q::transport::{Capabilities, Transport, TransportConfig};
    use crate::station::transports::testing::DirTransport;

    fn refuse(error: impl std::fmt::Display) -> ProviderError {
        ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string())
    }

    fn root_of(settings_toml: &str) -> Result<PathBuf, ProviderError> {
        let table: toml::Table = toml::from_str(settings_toml).map_err(refuse)?;
        table
            .get("root")
            .and_then(toml::Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| refuse("root is required"))
    }

    #[derive(Provider)]
    #[provider(
        TheaterProvider,
        id = "mock-maker",
        name = "Mock Theater maker",
        config = TheaterProviderConfig,
        crate = crate::guatiao
    )]
    pub(crate) struct MockMaker {
        root: PathBuf,
    }

    impl TryFrom<TheaterProviderConfig> for MockMaker {
        type Error = ProviderError;

        fn try_from(config: TheaterProviderConfig) -> Result<Self, Self::Error> {
            Ok(Self {
                root: root_of(&config.settings_toml)?,
            })
        }
    }

    impl TheaterProvider for MockMaker {
        fn capabilities(&self) -> Result<Value, ProviderError> {
            Capabilities {
                run: true,
                files: true,
                os: Some("linux".to_owned()),
                ..Capabilities::default()
            }
            .to_value()
            .map_err(refuse)
        }

        fn create(&self, request: &Value) -> Result<Value, ProviderError> {
            let request = TheaterRequest::from_value(request).map_err(refuse)?;
            let dir = self.root.join(&request.name);
            // Later Theaters are newer even within one second.
            let made_before = std::fs::read_dir(&self.root).map_or(0, Iterator::count) as u64;
            let created_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs())
                + made_before;
            std::fs::create_dir_all(dir.join("work")).map_err(refuse)?;
            let made = MadeTheater {
                name: request.name,
                template: request.template,
                mission: request.mission,
                created_at,
                transport: "mock-dir".to_owned(),
                settings_toml: toml::to_string(&toml::Table::from_iter([(
                    "root".to_owned(),
                    toml::Value::String(dir.to_string_lossy().into_owned()),
                )]))
                .map_err(refuse)?,
            };
            std::fs::write(
                dir.with_extension("made"),
                format!("{}\n{}\n{}\n", made.template, made.mission, made.created_at),
            )
            .map_err(refuse)?;
            made.to_value().map_err(refuse)
        }

        fn destroy(&self, name: &str) -> Result<(), ProviderError> {
            let dir = self.root.join(name);
            let _ = std::fs::remove_file(dir.with_extension("made"));
            match std::fs::remove_dir_all(dir) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(refuse(error)),
                _ => Ok(()),
            }
        }

        fn list(&self, template: &str) -> Result<Value, ProviderError> {
            let mut theaters = Vec::new();
            for entry in std::fs::read_dir(&self.root).map_err(refuse)?.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|ext| ext != "made") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).map_err(refuse)?;
                let mut lines = text.lines();
                let (Some(made_from), Some(mission), Some(created_at)) =
                    (lines.next(), lines.next(), lines.next())
                else {
                    continue;
                };
                if made_from != template {
                    continue;
                }
                let dir = path.with_extension("");
                theaters.push(MadeTheater {
                    name: dir.file_name().unwrap().to_string_lossy().into_owned(),
                    template: template.to_owned(),
                    mission: mission.to_owned(),
                    created_at: created_at.parse().map_err(refuse)?,
                    transport: "mock-dir".to_owned(),
                    settings_toml: toml::to_string(&toml::Table::from_iter([(
                        "root".to_owned(),
                        toml::Value::String(dir.to_string_lossy().into_owned()),
                    )]))
                    .map_err(refuse)?,
                });
            }
            MadeTheaters { theaters }.to_value().map_err(refuse)
        }
    }

    /// The `mock-dir` transport: a directory standing in for a host.
    #[derive(Provider)]
    #[provider(
        Transport,
        id = "mock-dir",
        name = "Mock directory transport",
        config = TransportConfig,
        crate = crate::guatiao
    )]
    pub(crate) struct MockDir(DirTransport);

    impl TryFrom<TransportConfig> for MockDir {
        type Error = ProviderError;

        fn try_from(config: TransportConfig) -> Result<Self, Self::Error> {
            Ok(Self(DirTransport::new(&root_of(&config.settings_toml)?)))
        }
    }

    impl Transport for MockDir {
        fn capabilities(&self) -> Result<Value, ProviderError> {
            self.0.capabilities()
        }

        fn run(&self, spec: &Value) -> Result<Value, ProviderError> {
            self.0.run(spec)
        }

        fn read_file(&self, spec: &Value) -> Result<Value, ProviderError> {
            self.0.read_file(spec)
        }

        fn write_file(&self, spec: &Value) -> Result<(), ProviderError> {
            self.0.write_file(spec)
        }

        fn list_dir(&self, path: &str) -> Result<Value, ProviderError> {
            self.0.list_dir(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::theater_name;
    use crate::bounds::Limit;

    #[test]
    fn a_made_theater_is_named_after_its_template_and_mission_within_bounds() {
        let bytes = Limit::at(100);
        assert_eq!(theater_name("scratch", "m-1", bytes), "scratch-m-1");
        assert_eq!(theater_name("scratch", "a b/c:d", bytes), "scratch-a-b-c-d");
        let long = theater_name("scratch", &"x".repeat(300), bytes);
        assert_eq!(long.len(), 100);
        assert_ne!(long, theater_name("scratch", &"x".repeat(301), bytes));
        assert_eq!(
            theater_name("scratch", &"x".repeat(300), Limit::UNBOUNDED).len(),
            308
        );
    }
}
