// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-transport-ssh`: the `ssh` transport provider, the way into a host over SSH. One
//! pure-Rust client (`russh`, `ring` crypto) on every OS: commands over exec channels,
//! terminals on a pty, files over SFTP, and reverse forwards.
//!
//! Provider calls are synchronous. The transport owns a small Tokio runtime and blocks on
//! it, so a caller never needs one. The connection is made on first use and remade when it
//! drops.

#![doc = include_str!("../README.md")]

ai6::guatiao::providers!(Ssh);

mod command;
mod exec;
mod files;
mod forward;
mod session;
mod settings;

use std::sync::Mutex;

use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Status, ToValue, Value};
use ai6::{
    Capabilities, ForwardSpec, ReadSpec, RunSpec, Spec, TerminalSession, Transport,
    TransportConfig, TransportForward, WriteSpec,
};
use tokio::runtime::{Builder, Runtime};

use session::Link;
use settings::Settings;

/// The `ssh` transport: one host, reached with a key file.
#[derive(ai6::guatiao::Provider)]
#[provider(
    Transport,
    id = "ssh",
    name = "ssh",
    config = TransportConfig,
    crate = ::ai6::guatiao
)]
pub struct Ssh {
    link: Link,
    runtime: Option<Runtime>,
    probed: Mutex<Option<Capabilities>>,
}

impl TryFrom<TransportConfig> for Ssh {
    type Error = ProviderError;

    fn try_from(config: TransportConfig) -> Result<Self, Self::Error> {
        let settings = Settings::parse(&config.settings_toml)
            .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ai6-ssh")
            .enable_all()
            .build()
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        Ok(Self {
            link: Link::new(settings),
            runtime: Some(runtime),
            probed: Mutex::new(None),
        })
    }
}

impl Drop for Ssh {
    fn drop(&mut self) {
        // Dropping a runtime on a thread that may be inside another one would panic.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl Ssh {
    fn runtime(&self) -> &Runtime {
        self.runtime
            .as_ref()
            .expect("the runtime lives until the transport is dropped")
    }

    /// What this transport can do before the host has been asked.
    fn declared(&self) -> Capabilities {
        let mut capabilities = Capabilities {
            terminal: true,
            run: true,
            files: true,
            forward: true,
            ..Capabilities::default()
        };
        self.add_host_key_note(&mut capabilities);
        capabilities
    }

    /// A host key trusted on first use is shown, so it can be pinned.
    fn add_host_key_note(&self, capabilities: &mut Capabilities) {
        let settings = self.link.settings();
        if settings.pin().is_none()
            && settings.trust_on_first_use
            && let Some(key) = self.link.host_key.trusted()
        {
            capabilities.notes.push(format!(
                "host key {key} was accepted at first contact; pin it with `host_key`"
            ));
        }
    }

    async fn probe_host(&self) -> Result<Capabilities, ProviderError> {
        let conn = self.link.conn().await?;
        let mut capabilities = self.declared();
        let uname = exec::run(
            &self.link,
            &RunSpec {
                argv: vec!["uname".to_owned(), "-sm".to_owned()],
                timeout_ms: 10_000,
                max_output: 1024,
                ..RunSpec::default()
            },
        )
        .await;
        if let Ok(output) = uname
            && output.code == Some(0)
        {
            let (os, arch) = command::parse_uname(&String::from_utf8_lossy(&output.stdout.0));
            capabilities.os = os;
            capabilities.arch = arch;
        }
        if conn.sftp().await.is_err() {
            capabilities.files = false;
            capabilities
                .notes
                .push("the host offers no SFTP subsystem, so files are unavailable".to_owned());
        }
        Ok(capabilities)
    }
}

impl Transport for Ssh {
    fn capabilities(&self) -> Result<Value, ProviderError> {
        let known = self
            .probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut capabilities = known.unwrap_or_else(|| self.declared());
        capabilities
            .notes
            .retain(|note| !note.starts_with("host key "));
        self.add_host_key_note(&mut capabilities);
        value(&capabilities)
    }

    fn probe(&self) -> Result<Value, ProviderError> {
        let capabilities = self.runtime().block_on(self.probe_host())?;
        *self
            .probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(capabilities.clone());
        value(&capabilities)
    }

    fn open_terminal(&self, spec: &Value) -> Result<Object<dyn TerminalSession>, ProviderError> {
        let spec = spec_of::<Spec>(spec)?;
        let terminal = self
            .runtime()
            .block_on(exec::open_terminal(&self.link, &spec))?;
        Ok(terminal.into_object())
    }

    fn run(&self, spec: &Value) -> Result<Value, ProviderError> {
        let spec = spec_of::<RunSpec>(spec)?;
        value(&self.runtime().block_on(exec::run(&self.link, &spec))?)
    }

    fn read_file(&self, spec: &Value) -> Result<Value, ProviderError> {
        let spec = spec_of::<ReadSpec>(spec)?;
        value(&self.runtime().block_on(files::read(&self.link, &spec))?)
    }

    fn write_file(&self, spec: &Value) -> Result<(), ProviderError> {
        let spec = spec_of::<WriteSpec>(spec)?;
        self.runtime().block_on(files::write(&self.link, &spec))
    }

    fn list_dir(&self, path: &str) -> Result<Value, ProviderError> {
        value(&self.runtime().block_on(files::list(&self.link, path))?)
    }

    fn forward(&self, spec: &Value) -> Result<Object<dyn TransportForward>, ProviderError> {
        let spec = spec_of::<ForwardSpec>(spec)?;
        let forward = self.runtime().block_on(forward::open(&self.link, &spec))?;
        Ok(forward.into_object())
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
