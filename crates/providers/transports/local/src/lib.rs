// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-transport-local`: the `local` transport provider, the way into the Station's own
//! host. It runs commands with `std::process` and reads and writes files with `std::fs`.
//!
//! It opens no terminal: a Station opens terminals on its own host with its Terminal
//! provider (`pty`), and reports the Theater's `terminal` capability from that.

#![doc = include_str!("../README.md")]

ai6::guatiao::providers!(Local);

mod files;
mod process;

use ai6::directorates::q::transport::kit::DEFAULT_READER_GRACE_MS;
use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{FromValue, Schema, Status, ToValue, Value};
use ai6::{Capabilities, ReadSpec, RunSpec, Transport, WriteSpec};

/// The Station's own host as a transport.
/// `local` transport configuration; every field has a default.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq)]
#[map(crate = ::ai6::guatiao)]
pub struct LocalConfig {
    /// Milliseconds the output readers get to finish once a command has ended; a descendant
    /// that outlives it can hold a pipe open indefinitely. Default 1000.
    #[map(default = DEFAULT_READER_GRACE_MS)]
    pub reader_grace_ms: u64,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            reader_grace_ms: DEFAULT_READER_GRACE_MS,
        }
    }
}

/// The Station's own host.
#[derive(Debug, Clone, Copy, Default, ai6::guatiao::Provider)]
#[provider(
    Transport,
    id = "local",
    name = "local",
    crate = ::ai6::guatiao,
    config = LocalConfig
)]
pub struct Local {
    config: LocalConfig,
}

impl Local {
    /// A transport with `config`.
    pub fn new(config: LocalConfig) -> Self {
        Self { config }
    }
}

impl TryFrom<LocalConfig> for Local {
    type Error = ProviderError;

    fn try_from(config: LocalConfig) -> Result<Self, Self::Error> {
        Ok(Self::new(config))
    }
}

impl Transport for Local {
    fn capabilities(&self) -> Result<Value, ProviderError> {
        value(&Capabilities {
            terminal: false,
            run: true,
            files: true,
            forward: false,
            os: Some(std::env::consts::OS.to_owned()),
            arch: Some(std::env::consts::ARCH.to_owned()),
            notes: Vec::new(),
        })
    }

    fn run(&self, spec: &Value) -> Result<Value, ProviderError> {
        let spec = spec_of::<RunSpec>(spec)?;
        value(&process::run(
            &spec,
            std::time::Duration::from_millis(self.config.reader_grace_ms),
        )?)
    }

    fn read_file(&self, spec: &Value) -> Result<Value, ProviderError> {
        let spec = spec_of::<ReadSpec>(spec)?;
        value(&files::read(&spec)?)
    }

    fn write_file(&self, spec: &Value) -> Result<(), ProviderError> {
        files::write(&spec_of::<WriteSpec>(spec)?)
    }

    fn list_dir(&self, path: &str) -> Result<Value, ProviderError> {
        value(&files::list(path)?)
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
