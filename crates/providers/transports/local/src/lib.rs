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

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{FromValue, Status, ToValue, Value};
use ai6::{Capabilities, ReadSpec, RunSpec, Transport, WriteSpec};

/// The Station's own host as a transport.
#[derive(Default, ai6::guatiao::Provider)]
#[provider(
    Transport,
    id = "local",
    name = "local",
    crate = ::ai6::guatiao
)]
pub struct Local;

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
        value(&process::run(&spec)?)
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

/// A failed filesystem or process call, with the status a caller can act on.
fn io_error(error: &std::io::Error, path: &str) -> ProviderError {
    let status = match error.kind() {
        std::io::ErrorKind::NotFound => Status::GUATIAO_ERR_NOT_FOUND,
        _ => Status::GUATIAO_ERR_INTERNAL,
    };
    refuse(status, format!("{path}: {error}"))
}

fn refuse(status: Status, error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(status, &error.to_string())
}
