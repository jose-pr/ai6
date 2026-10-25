// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-transport-qemu`: the `qemu` transport provider, the way into a QEMU guest through its
//! guest agent (`qemu-ga`). Commands run with `guest-exec`; file operations are built from
//! shell commands run the same way, so the guest needs a POSIX `sh`.
//!
//! A guest runs the harness on the Station in remote-hands mode unless a runner is installed
//! inside it.

#![doc = include_str!("../README.md")]

ai6::guatiao::providers!(Qemu);

mod agent;
mod settings;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use ai6::directorates::q::transport::kit::{self, Shell, command_line};
use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{Bytes, FromValue, Status, ToValue, Value};
use ai6::{Capabilities, Output, ReadSpec, RunSpec, Transport, TransportConfig, WriteSpec};
use data_encoding::BASE64;
use serde_json::json;

use agent::{Agent, Endpoint};
use settings::Settings;

const POLL_START: Duration = Duration::from_millis(10);
const POLL_MAX: Duration = Duration::from_millis(250);

/// A QEMU guest reached through its guest agent.
#[derive(ai6::guatiao::Provider)]
#[provider(
    Transport,
    id = "qemu",
    name = "qemu",
    config = TransportConfig,
    crate = ::ai6::guatiao
)]
pub struct Qemu {
    agent: Agent,
    probed: Mutex<Option<Capabilities>>,
}

impl TryFrom<TransportConfig> for Qemu {
    type Error = ProviderError;

    fn try_from(config: TransportConfig) -> Result<Self, Self::Error> {
        let settings = Settings::parse(&config.settings_toml)
            .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
        let endpoint = match (&settings.address, &settings.socket) {
            (Some(address), None) => Endpoint::Tcp(address.clone()),
            (None, Some(socket)) => Endpoint::Unix(socket.clone()),
            _ => unreachable!("settings hold exactly one endpoint"),
        };
        Ok(Self {
            agent: Agent::new(endpoint, settings.reply_timeout()),
            probed: Mutex::new(None),
        })
    }
}

impl Qemu {
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

    /// Runs `path` with `args` in the guest and waits for it.
    fn exec(
        &self,
        path: &str,
        args: &[String],
        env: &[String],
        stdin: Option<&[u8]>,
        timeout_ms: u64,
        max_output: u64,
    ) -> Result<Output, ProviderError> {
        let mut request = json!({ "path": path, "arg": args, "capture-output": true });
        if !env.is_empty() {
            request["env"] = json!(env);
        }
        if let Some(stdin) = stdin {
            request["input-data"] = json!(BASE64.encode(stdin));
        }
        let started = self.agent.call("guest-exec", request)?;
        let pid = started
            .get("pid")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    "guest-exec did not return a pid",
                )
            })?;

        let deadline = (timeout_ms > 0).then(|| Instant::now() + Duration::from_millis(timeout_ms));
        let mut pause = POLL_START;
        let status = loop {
            let status = self
                .agent
                .call("guest-exec-status", json!({ "pid": pid }))?;
            if status.get("exited").and_then(serde_json::Value::as_bool) == Some(true) {
                break Some(status);
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                break None;
            }
            std::thread::sleep(pause);
            pause = (pause * 2).min(POLL_MAX);
        };

        let Some(status) = status else {
            // The guest agent cannot end a process it started.
            return Ok(Output {
                timed_out: true,
                ..Output::default()
            });
        };
        let limit = usize::try_from(max_output).unwrap_or(usize::MAX);
        let mut truncated = ["out-truncated", "err-truncated"]
            .iter()
            .any(|key| status.get(*key).and_then(serde_json::Value::as_bool) == Some(true));
        let mut stream = |key: &str| -> Vec<u8> {
            let mut data = status
                .get(key)
                .and_then(serde_json::Value::as_str)
                .and_then(|text| BASE64.decode(text.as_bytes()).ok())
                .unwrap_or_default();
            if data.len() > limit {
                data.truncate(limit);
                truncated = true;
            }
            data
        };
        let (stdout, stderr) = (stream("out-data"), stream("err-data"));
        Ok(Output {
            code: status.get("exitcode").and_then(serde_json::Value::as_i64),
            signal: status.get("signal").and_then(serde_json::Value::as_i64),
            stdout: Bytes(stdout),
            stderr: Bytes(stderr),
            timed_out: false,
            truncated,
        })
    }
}

impl Shell for Qemu {
    fn sh(
        &self,
        script: &str,
        stdin: Option<&[u8]>,
        max_output: u64,
    ) -> Result<Output, ProviderError> {
        self.exec(
            "/bin/sh",
            &["-c".to_owned(), script.to_owned()],
            &[],
            stdin,
            0,
            max_output,
        )
    }
}

impl Transport for Qemu {
    fn capabilities(&self) -> Result<Value, ProviderError> {
        let known = self
            .probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        value(&known.unwrap_or_else(Self::declared))
    }

    fn probe(&self) -> Result<Value, ProviderError> {
        self.agent.call("guest-ping", json!({}))?;
        let mut capabilities = Self::declared();
        if let Ok(info) = self.agent.call("guest-get-osinfo", json!({})) {
            let id = info
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            capabilities.os = match id {
                "" => None,
                "mswindows" => Some("windows".to_owned()),
                "freebsd" | "openbsd" | "netbsd" => Some(id.to_owned()),
                "macos" | "darwin" => Some("macos".to_owned()),
                _ => Some("linux".to_owned()),
            };
            capabilities.arch = info.get("machine").and_then(serde_json::Value::as_str).map(
                |machine| match machine {
                    "arm64" => "aarch64".to_owned(),
                    "amd64" => "x86_64".to_owned(),
                    other => other.to_owned(),
                },
            );
        }
        if capabilities.os.as_deref() == Some("windows") {
            capabilities.files = false;
            capabilities.notes.push(
                "files need a POSIX shell in the guest, which a Windows guest does not have"
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
        if spec.argv.is_empty() {
            return Err(refuse(Status::GUATIAO_ERR_BAD_VALUE, "argv is empty"));
        }
        let stdin = spec.stdin.as_ref().map(|stdin| stdin.0.as_slice());
        let output = if spec.cwd.is_some() {
            // `guest-exec` has no working directory, so a POSIX shell sets it.
            let line = command_line(&spec.argv, spec.cwd.as_deref(), spec.env.as_ref())
                .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
            self.exec(
                "/bin/sh",
                &["-c".to_owned(), line],
                &[],
                stdin,
                spec.timeout_ms,
                spec.max_output,
            )?
        } else {
            let env: Vec<String> = spec
                .env
                .iter()
                .flatten()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            self.exec(
                &spec.argv[0],
                &spec.argv[1..],
                &env,
                stdin,
                spec.timeout_ms,
                spec.max_output,
            )?
        };
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

pub(crate) fn refuse(status: Status, error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(status, &error.to_string())
}
