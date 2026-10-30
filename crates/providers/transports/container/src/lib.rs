// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-transport-container`: the `container` transport provider, the way into a running
//! Docker or Podman container. It drives the engine's command line: `exec` for commands, and
//! file operations built from shell commands run by `exec`.
//!
//! A container has no use here for a terminal, and a Theater in one runs the harness on the
//! Station in remote-hands mode unless a runner is installed inside it.

#![doc = include_str!("../README.md")]

ai6::guatiao::providers!(Container);

mod settings;

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai6::directorates::q::transport::kit::{self, ProcessInput, Shell, run_process};
use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{FromValue, Status, ToValue, Value};
use ai6::{Capabilities, Output, ReadSpec, RunSpec, Transport, TransportConfig, WriteSpec};

use settings::Settings;

/// Runs the engine's command line. The real one starts the executable; a test stands in for
/// the engine with its own.
pub trait Runner: Send + Sync {
    /// Runs `<engine> <args>` feeding it `input.stdin`.
    fn engine(&self, args: &[String], input: &ProcessInput<'_>) -> Result<Output, ProviderError>;
}

/// Starts the engine's executable.
struct Process {
    executable: String,
    reader_grace: Duration,
}

impl Runner for Process {
    fn engine(&self, args: &[String], input: &ProcessInput<'_>) -> Result<Output, ProviderError> {
        run_process(
            Command::new(&self.executable).args(args),
            input,
            self.reader_grace,
        )
    }
}

/// A running container reached through its engine.
#[derive(ai6::guatiao::Provider)]
#[provider(
    Transport,
    id = "container",
    name = "container",
    config = TransportConfig,
    crate = ::ai6::guatiao
)]
pub struct Container {
    runner: Arc<dyn Runner>,
    container: String,
    user: Option<String>,
    probed: Mutex<Option<Capabilities>>,
}

impl TryFrom<TransportConfig> for Container {
    type Error = ProviderError;

    fn try_from(config: TransportConfig) -> Result<Self, Self::Error> {
        let settings = Settings::parse(&config.settings_toml)
            .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
        Ok(Self::with_runner(
            Arc::new(Process {
                executable: settings.engine(),
                reader_grace: settings.reader_grace(),
            }),
            settings.container,
            settings.user,
        ))
    }
}

impl Container {
    /// A transport to `container` whose engine command line is run by `runner`.
    pub fn with_runner(
        runner: Arc<dyn Runner>,
        container: impl Into<String>,
        user: Option<String>,
    ) -> Self {
        Self {
            runner,
            container: container.into(),
            user,
            probed: Mutex::new(None),
        }
    }

    fn engine(&self, args: Vec<String>, input: &ProcessInput<'_>) -> Result<Output, ProviderError> {
        self.runner.engine(&args, input)
    }

    /// `exec [-i] [-u user] [-w cwd] [-e K=V]... <container> <argv>...`.
    fn exec(
        &self,
        argv: &[String],
        cwd: Option<&str>,
        env: Option<&std::collections::BTreeMap<String, String>>,
        input: &ProcessInput<'_>,
    ) -> Result<Output, ProviderError> {
        if argv.is_empty() {
            return Err(refuse(Status::GUATIAO_ERR_BAD_VALUE, "argv is empty"));
        }
        let mut args = vec!["exec".to_owned()];
        if input.stdin.is_some() {
            args.push("-i".to_owned());
        }
        if let Some(user) = &self.user {
            args.extend(["-u".to_owned(), user.clone()]);
        }
        if let Some(cwd) = cwd {
            args.extend(["-w".to_owned(), cwd.to_owned()]);
        }
        for (name, value) in env.into_iter().flatten() {
            if !kit::is_variable_name(name) {
                return Err(refuse(
                    Status::GUATIAO_ERR_BAD_VALUE,
                    format!("{name:?} is not an environment variable name"),
                ));
            }
            args.extend(["-e".to_owned(), format!("{name}={value}")]);
        }
        args.push(self.container.clone());
        args.extend(argv.iter().cloned());
        self.engine(args, input)
    }

    fn declared() -> Capabilities {
        Capabilities {
            terminal: false,
            run: true,
            files: true,
            forward: false,
            os: None,
            arch: None,
            notes: Vec::new(),
        }
    }
}

impl Shell for Container {
    fn sh(
        &self,
        script: &str,
        stdin: Option<&[u8]>,
        max_output: u64,
    ) -> Result<Output, ProviderError> {
        self.exec(
            &["sh".to_owned(), "-c".to_owned(), script.to_owned()],
            None,
            None,
            &ProcessInput {
                stdin,
                timeout_ms: 0,
                max_output,
            },
        )
    }
}

impl Transport for Container {
    fn capabilities(&self) -> Result<Value, ProviderError> {
        let known = self
            .probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        value(&known.unwrap_or_else(Self::declared))
    }

    fn probe(&self) -> Result<Value, ProviderError> {
        let short = ProcessInput {
            stdin: None,
            timeout_ms: 20_000,
            max_output: 4096,
        };
        let state = self.engine(
            vec![
                "inspect".to_owned(),
                "-f".to_owned(),
                "{{.State.Running}}".to_owned(),
                self.container.clone(),
            ],
            &short,
        )?;
        let running = String::from_utf8_lossy(&state.stdout.0).trim().to_owned();
        if state.code != Some(0) || running != "true" {
            let why = String::from_utf8_lossy(&state.stderr.0).trim().to_owned();
            return Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!(
                    "container {:?} is not running{}",
                    self.container,
                    if why.is_empty() {
                        String::new()
                    } else {
                        format!(": {why}")
                    }
                ),
            ));
        }
        let mut capabilities = Self::declared();
        let uname = self.exec(&["uname".to_owned(), "-sm".to_owned()], None, None, &short);
        if let Ok(output) = uname
            && output.code == Some(0)
        {
            let text = String::from_utf8_lossy(&output.stdout.0).to_lowercase();
            let mut words = text.split_whitespace();
            capabilities.os = words.next().map(str::to_owned);
            capabilities.arch = words.next().map(|arch| match arch {
                "arm64" => "aarch64".to_owned(),
                "amd64" => "x86_64".to_owned(),
                other => other.to_owned(),
            });
        }
        if !self
            .exec(
                &["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()],
                None,
                None,
                &short,
            )
            .is_ok_and(|output| output.code == Some(0))
        {
            capabilities.files = false;
            capabilities.notes.push(
                "the container has no `sh`, so files are unavailable; commands still run"
                    .to_owned(),
            );
        }
        *self
            .probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(capabilities.clone());
        value(&capabilities)
    }

    fn run(&self, spec: &Value) -> Result<Value, ProviderError> {
        let spec = spec_of::<RunSpec>(spec)?;
        let output = self.exec(
            &spec.argv,
            spec.cwd.as_deref(),
            spec.env.as_ref(),
            &ProcessInput {
                stdin: spec.stdin.as_ref().map(|stdin| stdin.0.as_slice()),
                timeout_ms: spec.timeout_ms,
                max_output: spec.max_output,
            },
        )?;
        value(&output)
    }

    fn read_file(&self, spec: &Value) -> Result<Value, ProviderError> {
        value(&kit::read_file(self, &spec_of::<ReadSpec>(spec)?)?)
    }

    fn write_file(&self, spec: &Value) -> Result<(), ProviderError> {
        kit::write_file(self, &spec_of::<WriteSpec>(spec)?)
    }

    fn list_dir(&self, path: &str) -> Result<Value, ProviderError> {
        value(&kit::list_dir(self, path)?)
    }
}

fn spec_of<T: FromValue>(spec: &Value) -> Result<T, ProviderError> {
    T::from_value(spec).map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error))
}

fn value(model: &impl ToValue) -> Result<Value, ProviderError> {
    model
        .to_value()
        .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))
}

fn refuse(status: Status, error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(status, &error.to_string())
}
