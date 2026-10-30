// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-transport-adb`: the `adb` transport provider, the way into an Android device. It
//! drives the `adb` command line: `adb shell` for commands and file operations built from
//! shell commands, and `adb reverse` for forwards.
//!
//! An Android device cannot run an agent harness, so a Theater on it works in remote-hands
//! mode: set `agents = "remote-hands"` on the Theater.

#![doc = include_str!("../README.md")]

ai6::guatiao::providers!(Adb);

mod settings;

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai6::directorates::q::transport::kit::{self, ProcessInput, Shell, command_line, run_process};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Status, ToValue, Value};
use ai6::{
    Capabilities, ForwardInfo, ForwardSpec, Output, ReadSpec, RunSpec, Transport, TransportConfig,
    TransportForward, WriteSpec,
};

use settings::Settings;

/// Runs the `adb` command line. The real one starts the executable; a test stands in for the
/// device with its own.
pub trait Runner: Send + Sync {
    /// Runs `adb <args>` (the device is already chosen: `-s <serial>` is the runner's to add)
    /// feeding it `input.stdin`.
    fn adb(&self, args: &[String], input: &ProcessInput<'_>) -> Result<Output, ProviderError>;
}

/// Starts the `adb` executable.
struct Process {
    executable: String,
    serial: Option<String>,
    reader_grace: Duration,
}

impl Runner for Process {
    fn adb(&self, args: &[String], input: &ProcessInput<'_>) -> Result<Output, ProviderError> {
        let mut command = Command::new(&self.executable);
        if let Some(serial) = &self.serial {
            command.arg("-s").arg(serial);
        }
        command.args(args);
        run_process(&mut command, input, self.reader_grace)
    }
}

/// An Android device reached with `adb`.
#[derive(ai6::guatiao::Provider)]
#[provider(
    Transport,
    id = "adb",
    name = "adb",
    config = TransportConfig,
    crate = ::ai6::guatiao
)]
pub struct Adb {
    runner: Arc<dyn Runner>,
    probed: Mutex<Option<Capabilities>>,
}

impl TryFrom<TransportConfig> for Adb {
    type Error = ProviderError;

    fn try_from(config: TransportConfig) -> Result<Self, Self::Error> {
        let settings = Settings::parse(&config.settings_toml)
            .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
        Ok(Self::with_runner(Arc::new(Process {
            executable: settings.executable(),
            reader_grace: settings.reader_grace(),
            serial: settings.serial,
        })))
    }
}

impl Adb {
    /// An `adb` transport whose command line is run by `runner`.
    pub fn with_runner(runner: Arc<dyn Runner>) -> Self {
        Self {
            runner,
            probed: Mutex::new(None),
        }
    }

    fn adb(
        &self,
        args: &[&str],
        stdin: Option<&[u8]>,
        timeout_ms: u64,
        max_output: u64,
    ) -> Result<Output, ProviderError> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        self.runner.adb(
            &args,
            &ProcessInput {
                stdin,
                timeout_ms,
                max_output,
            },
        )
    }

    fn declared() -> Capabilities {
        Capabilities {
            terminal: false,
            run: true,
            files: true,
            forward: true,
            os: Some("android".to_owned()),
            arch: None,
            notes: Vec::new(),
        }
    }
}

impl Shell for Adb {
    fn sh(
        &self,
        script: &str,
        stdin: Option<&[u8]>,
        max_output: u64,
    ) -> Result<Output, ProviderError> {
        self.adb(&["shell", script], stdin, 0, max_output)
    }
}

impl Transport for Adb {
    fn capabilities(&self) -> Result<Value, ProviderError> {
        let known = self
            .probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        value(&known.unwrap_or_else(Self::declared))
    }

    fn probe(&self) -> Result<Value, ProviderError> {
        let state = self.adb(&["get-state"], None, 15_000, 1024)?;
        let text = String::from_utf8_lossy(&state.stdout.0).trim().to_owned();
        if state.code != Some(0) || text != "device" {
            let why = String::from_utf8_lossy(&state.stderr.0).trim().to_owned();
            return Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!(
                    "the device is not ready (state {:?}{})",
                    if text.is_empty() { "unknown" } else { &text },
                    if why.is_empty() {
                        String::new()
                    } else {
                        format!(": {why}")
                    }
                ),
            ));
        }
        let mut capabilities = Self::declared();
        let abi = self.adb(&["shell", "getprop ro.product.cpu.abi"], None, 15_000, 1024)?;
        capabilities.arch = match String::from_utf8_lossy(&abi.stdout.0).trim() {
            "arm64-v8a" => Some("aarch64".to_owned()),
            "armeabi-v7a" | "armeabi" => Some("arm".to_owned()),
            "x86_64" => Some("x86_64".to_owned()),
            "x86" => Some("x86".to_owned()),
            "" => None,
            other => Some(other.to_owned()),
        };
        *self
            .probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(capabilities.clone());
        value(&capabilities)
    }

    fn run(&self, spec: &Value) -> Result<Value, ProviderError> {
        let spec = spec_of::<RunSpec>(spec)?;
        let line = command_line(&spec.argv, spec.cwd.as_deref(), spec.env.as_ref())
            .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
        let stdin = spec.stdin.as_ref().map(|stdin| stdin.0.as_slice());
        value(&self.adb(&["shell", &line], stdin, spec.timeout_ms, spec.max_output)?)
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

    fn forward(&self, spec: &Value) -> Result<Object<dyn TransportForward>, ProviderError> {
        let spec = spec_of::<ForwardSpec>(spec)?;
        let (host, port) = spec.target.rsplit_once(':').ok_or_else(|| {
            refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "target must be host:port on the Station",
            )
        })?;
        if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "adb reverse reaches only the Station's loopback, so target must be 127.0.0.1:PORT",
            ));
        }
        let target: u16 = port
            .parse()
            .map_err(|_| refuse(Status::GUATIAO_ERR_BAD_VALUE, "target port is not a number"))?;
        let remote = format!("tcp:{}", spec.remote_port);
        let output = self.adb(
            &["reverse", &remote, &format!("tcp:{target}")],
            None,
            15_000,
            1024,
        )?;
        if output.code != Some(0) {
            let why = String::from_utf8_lossy(&output.stderr.0).trim().to_owned();
            return Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!("adb reverse failed: {why}"),
            ));
        }
        // With port 0 adb prints the port the device chose; with a fixed one it prints nothing.
        let chosen = String::from_utf8_lossy(&output.stdout.0)
            .trim()
            .parse::<u16>()
            .unwrap_or(spec.remote_port);
        if chosen == 0 {
            return Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                "adb did not say which port the device listens on",
            ));
        }
        Ok(DeviceForward {
            runner: self.runner.clone(),
            info: ForwardInfo {
                remote_host: "127.0.0.1".to_owned(),
                remote_port: chosen,
            },
            open: std::sync::atomic::AtomicBool::new(true),
        }
        .into_object())
    }
}

/// An open `adb reverse`, removed when closed or dropped.
struct DeviceForward {
    runner: Arc<dyn Runner>,
    info: ForwardInfo,
    open: std::sync::atomic::AtomicBool,
}

impl DeviceForward {
    fn remove(&self) {
        if self.open.swap(false, std::sync::atomic::Ordering::AcqRel) {
            let args = [
                "reverse".to_owned(),
                "--remove".to_owned(),
                format!("tcp:{}", self.info.remote_port),
            ];
            let _ = self.runner.adb(
                &args,
                &ProcessInput {
                    stdin: None,
                    timeout_ms: 15_000,
                    max_output: 1024,
                },
            );
        }
    }
}

impl TransportForward for DeviceForward {
    fn info(&self) -> Result<Value, ProviderError> {
        value(&self.info)
    }

    fn close(&self) -> Result<(), ProviderError> {
        self.remove();
        Ok(())
    }
}

impl Drop for DeviceForward {
    fn drop(&mut self) {
        self.remove();
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
