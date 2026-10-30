// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-theater-container`: the `ephemeral-container` Theater provider. It makes a fresh
//! Docker or Podman container per Mission from an image, reached afterwards by the `container`
//! transport, and removes it when the Station is done with it.

#![doc = include_str!("../README.md")]

ai6::guatiao::providers!(ContainerTheaters);

mod settings;

use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ai6::directorates::q::transport::kit::{ProcessInput, run_process};
use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{FromValue, Status, ToValue, Value};
use ai6::{
    Capabilities, MadeTheater, MadeTheaters, Output, TheaterProvider, TheaterProviderConfig,
    TheaterRequest,
};

use settings::Settings;

/// The label that names the template a container was made from.
pub const TEMPLATE_LABEL: &str = "ai6.theater";
/// The label that names the Mission a container was made for.
pub const MISSION_LABEL: &str = "ai6.mission";
/// The label that holds when a container was made, as Unix seconds.
pub const CREATED_LABEL: &str = "ai6.created";

/// Runs the engine's command line. The real one starts the executable; a test stands in for
/// the engine with its own.
pub trait Engine: Send + Sync {
    /// Runs `<engine> <args>` to completion.
    fn run(&self, args: &[String]) -> Result<Output, ProviderError>;
}

struct Process {
    executable: String,
    /// How long one engine command may run, how much of its output is kept and how long the
    /// output readers get once it has ended.
    timeout_ms: u64,
    max_output: u64,
    reader_grace: std::time::Duration,
}

impl Engine for Process {
    fn run(&self, args: &[String]) -> Result<Output, ProviderError> {
        run_process(
            Command::new(&self.executable).args(args),
            &ProcessInput {
                stdin: None,
                timeout_ms: self.timeout_ms,
                max_output: self.max_output,
            },
            self.reader_grace,
        )
    }
}

/// Containers made from one image, one per Mission.
#[derive(ai6::guatiao::Provider)]
#[provider(
    TheaterProvider,
    id = "ephemeral-container",
    name = "ephemeral container",
    config = TheaterProviderConfig,
    crate = ::ai6::guatiao
)]
pub struct ContainerTheaters {
    engine: Arc<dyn Engine>,
    settings: Settings,
}

impl TryFrom<TheaterProviderConfig> for ContainerTheaters {
    type Error = ProviderError;

    fn try_from(config: TheaterProviderConfig) -> Result<Self, Self::Error> {
        let settings = Settings::parse(&config.settings_toml)
            .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
        Ok(Self {
            engine: Arc::new(Process {
                executable: settings.engine(),
                timeout_ms: settings.engine_timeout_ms(),
                max_output: settings.engine_output_bytes(),
                reader_grace: settings.reader_grace(),
            }),
            settings,
        })
    }
}

impl ContainerTheaters {
    /// A provider whose engine command line is run by `engine`, with the template settings
    /// `settings_toml`.
    pub fn with_engine(
        engine: Arc<dyn Engine>,
        settings_toml: &str,
    ) -> Result<Self, ProviderError> {
        let settings = Settings::parse(settings_toml)
            .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
        Ok(Self { engine, settings })
    }

    fn engine(&self, args: Vec<String>) -> Result<Output, ProviderError> {
        self.engine.run(&args)
    }

    /// The labels of `container`: template, Mission and creation time. `None` when there is
    /// no such container.
    fn labels(&self, container: &str) -> Result<Option<(String, String, u64)>, ProviderError> {
        let format = format!(
            "{{{{index .Config.Labels \"{TEMPLATE_LABEL}\"}}}}\t{{{{index .Config.Labels \"{MISSION_LABEL}\"}}}}\t{{{{index .Config.Labels \"{CREATED_LABEL}\"}}}}"
        );
        let output = self.engine(vec![
            "inspect".to_owned(),
            "--format".to_owned(),
            format,
            container.to_owned(),
        ])?;
        if output.code != Some(0) {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&output.stdout.0);
        let mut fields = text.trim().splitn(3, '\t');
        Ok(Some((
            fields.next().unwrap_or_default().to_owned(),
            fields.next().unwrap_or_default().to_owned(),
            fields
                .next()
                .and_then(|created| created.trim().parse().ok())
                .unwrap_or_default(),
        )))
    }

    fn made(
        &self,
        name: &str,
        template: String,
        mission: String,
        created_at: u64,
    ) -> Result<MadeTheater, ProviderError> {
        let mut transport = toml::Table::new();
        transport.insert("container".to_owned(), container_name(name).into());
        transport.insert("engine".to_owned(), self.settings.engine().into());
        if let Some(user) = &self.settings.user {
            transport.insert("user".to_owned(), user.clone().into());
        }
        Ok(MadeTheater {
            name: name.to_owned(),
            template,
            mission,
            created_at,
            transport: "container".to_owned(),
            settings_toml: toml::to_string(&transport)
                .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error.to_string()))?,
        })
    }
}

/// The container a Theater named `name` is.
pub fn container_name(name: &str) -> String {
    format!("ai6-{name}")
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

impl TheaterProvider for ContainerTheaters {
    fn capabilities(&self) -> Result<Value, ProviderError> {
        Capabilities {
            run: true,
            files: true,
            os: Some("linux".to_owned()),
            ..Capabilities::default()
        }
        .to_value()
        .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error.to_string()))
    }

    fn create(&self, request: &Value) -> Result<Value, ProviderError> {
        let request = TheaterRequest::from_value(request)
            .map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error.to_string()))?;
        if !valid_name(&request.name) {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!("{:?} cannot name a container", request.name),
            ));
        }
        let container = container_name(&request.name);
        if let Some((template, mission, created_at)) = self.labels(&container)? {
            // Made earlier for the same name: start it again if it stopped.
            let started = self.engine(vec!["start".to_owned(), container.clone()])?;
            if started.code != Some(0) {
                return Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    format!(
                        "container {container:?} did not start: {}",
                        failure(&started)
                    ),
                ));
            }
            return self
                .made(&request.name, template, mission, created_at)?
                .to_value()
                .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error.to_string()));
        }
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let mut args = vec![
            "run".to_owned(),
            "--detach".to_owned(),
            "--name".to_owned(),
            container.clone(),
            "--label".to_owned(),
            format!("{TEMPLATE_LABEL}={}", request.template),
            "--label".to_owned(),
            format!("{MISSION_LABEL}={}", request.mission),
            "--label".to_owned(),
            format!("{CREATED_LABEL}={created_at}"),
        ];
        args.extend(self.settings.run_args.iter().cloned());
        args.push(self.settings.image.clone());
        args.extend(self.settings.command());
        let output = self.engine(args)?;
        if output.code != Some(0) {
            return Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!("container {container:?} was not made: {}", failure(&output)),
            ));
        }
        self.made(&request.name, request.template, request.mission, created_at)?
            .to_value()
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error.to_string()))
    }

    fn destroy(&self, name: &str) -> Result<(), ProviderError> {
        let container = container_name(name);
        if self.labels(&container)?.is_none() {
            return Ok(());
        }
        let output = self.engine(vec![
            "rm".to_owned(),
            "--force".to_owned(),
            container.clone(),
        ])?;
        if output.code == Some(0) || self.labels(&container)?.is_none() {
            Ok(())
        } else {
            Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!(
                    "container {container:?} was not removed: {}",
                    failure(&output)
                ),
            ))
        }
    }

    fn list(&self, template: &str) -> Result<Value, ProviderError> {
        let output = self.engine(vec![
            "ps".to_owned(),
            "--all".to_owned(),
            "--filter".to_owned(),
            format!("label={TEMPLATE_LABEL}={template}"),
            "--format".to_owned(),
            "{{.Names}}".to_owned(),
        ])?;
        if output.code != Some(0) {
            return Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!("the containers could not be listed: {}", failure(&output)),
            ));
        }
        let mut theaters = Vec::new();
        for container in String::from_utf8_lossy(&output.stdout.0).lines() {
            let container = container.trim();
            let Some(name) = container.strip_prefix("ai6-") else {
                continue;
            };
            if let Some((made_from, mission, created_at)) = self.labels(container)?
                && made_from == template
            {
                theaters.push(self.made(name, made_from, mission, created_at)?);
            }
        }
        theaters.sort_by_key(|made| made.created_at);
        MadeTheaters { theaters }
            .to_value()
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error.to_string()))
    }
}

fn failure(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr.0);
    let status = output.code.map_or_else(
        || "no exit status".to_owned(),
        |code| format!("exit {code}"),
    );
    match stderr.trim().lines().last() {
        Some(line) => format!("{status}: {line}"),
        None => status,
    }
}

fn refuse(status: Status, message: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(status, &message.to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use ai6::guatiao::Bytes;

    use super::*;

    /// An engine that keeps containers in memory and records every command.
    #[derive(Default)]
    struct Fake {
        containers: Mutex<BTreeMap<String, (String, String, String)>>,
        commands: Mutex<Vec<Vec<String>>>,
    }

    fn answer(code: i64, stdout: &str) -> Output {
        Output {
            code: Some(code),
            stdout: Bytes(stdout.as_bytes().to_vec()),
            ..Output::default()
        }
    }

    impl Engine for Fake {
        fn run(&self, args: &[String]) -> Result<Output, ProviderError> {
            self.commands.lock().unwrap().push(args.to_vec());
            let mut containers = self.containers.lock().unwrap();
            let label = |args: &[String], key: &str| {
                args.iter()
                    .find_map(|arg| arg.strip_prefix(&format!("{key}=")))
                    .unwrap_or_default()
                    .to_owned()
            };
            Ok(match args[0].as_str() {
                "inspect" => match containers.get(&args[3]) {
                    Some((template, mission, created)) => {
                        answer(0, &format!("{template}\t{mission}\t{created}\n"))
                    }
                    None => answer(1, ""),
                },
                "run" => {
                    containers.insert(
                        args[3].clone(),
                        (
                            label(args, TEMPLATE_LABEL),
                            label(args, MISSION_LABEL),
                            label(args, CREATED_LABEL),
                        ),
                    );
                    answer(0, "id\n")
                }
                "rm" => {
                    containers.remove(&args[2]);
                    answer(0, "")
                }
                "ps" => answer(
                    0,
                    &containers
                        .keys()
                        .map(|name| format!("{name}\n"))
                        .collect::<String>(),
                ),
                _ => answer(0, ""),
            })
        }
    }

    fn provider(engine: &Arc<Fake>) -> ContainerTheaters {
        ContainerTheaters::with_engine(
            engine.clone(),
            "image = \"fedora:44\"\nengine = \"podman\"\nrun_args = [\"--network\", \"none\"]\n",
        )
        .unwrap()
    }

    fn create(provider: &ContainerTheaters, name: &str, mission: &str) -> MadeTheater {
        let request = TheaterRequest {
            name: name.to_owned(),
            template: "scratch".to_owned(),
            mission: mission.to_owned(),
        };
        MadeTheater::from_value(&provider.create(&request.to_value().unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn a_theater_is_a_labelled_container_reached_by_the_container_transport() {
        let engine = Arc::new(Fake::default());
        let provider = provider(&engine);
        let made = create(&provider, "scratch-m1", "m1");
        assert_eq!(made.transport, "container");
        assert_eq!(
            made.settings_toml,
            "container = \"ai6-scratch-m1\"\nengine = \"podman\"\n"
        );
        let run = engine.commands.lock().unwrap()[1].clone();
        assert_eq!(
            run[..4],
            ["run", "--detach", "--name", "ai6-scratch-m1"].map(str::to_owned)
        );
        assert!(run.contains(&"ai6.mission=m1".to_owned()));
        assert_eq!(
            run[run.len() - 5..],
            ["--network", "none", "fedora:44", "sleep", "infinity"].map(str::to_owned)
        );

        // Asked again, it answers the same container.
        let again = create(&provider, "scratch-m1", "m1");
        assert_eq!(again.created_at, made.created_at);
        assert_eq!(engine.containers.lock().unwrap().len(), 1);

        let listed = MadeTheaters::from_value(&provider.list("scratch").unwrap())
            .unwrap()
            .theaters;
        assert_eq!(listed, [made]);
        assert!(
            MadeTheaters::from_value(&provider.list("other").unwrap())
                .unwrap()
                .theaters
                .is_empty()
        );

        provider.destroy("scratch-m1").unwrap();
        provider.destroy("scratch-m1").unwrap();
        assert!(engine.containers.lock().unwrap().is_empty());
        let bad = TheaterRequest {
            name: "-rm".to_owned(),
            template: "scratch".to_owned(),
            mission: "m".to_owned(),
        };
        assert!(provider.create(&bad.to_value().unwrap()).is_err());
    }
}
