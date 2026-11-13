// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-safehouse-git`: the `git` safe-house provider. Each Mission gets a worktree of the
//! Operation's checkout on a new branch; the work is read back as commits. A spec with
//! `clone_to` gets a clone of a repository instead, saved to a branch it shares. The Git logic
//! is plain Rust in `worktree.rs` and `clone.rs`; this file is the thin layer that implements
//! the kind.

#![doc = include_str!("../README.md")]

ai6::guatiao::providers!(Git);

mod clone;
mod worktree;

use ai6::directorates::q::safehouse::{
    ChangeLog, CheckpointSpec, SafeHouseInfo, SafeHouseProvider, SafeHouseRun, SafeHouseSpec,
};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Schema, Status, ToValue, Value};

use worktree::{Runner, Worktree};

/// `git` provider configuration; every field has a default. The fields in the `operation`
/// section are an Operation's to set (`workspace.settings`); the Station's placement reads them
/// to build each Mission's [`SafeHouseSpec`] and its checkpoints. The others are the Station's.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::ai6::guatiao)]
pub struct GitConfig {
    /// The revision each Mission starts from.
    #[schema(title = "Start from revision", section = "operation")]
    #[map(default = "HEAD".to_owned())]
    pub base: String,
    /// The name each Mission's branch gets; {id}, {slug} and {operation} are filled in.
    #[schema(title = "Branch name", section = "operation")]
    #[map(default = "{id}-{slug}".to_owned())]
    pub branch: String,
    /// Give each Mission its own copy. Off, Missions work in the source itself.
    #[schema(title = "A copy for each Mission", section = "operation")]
    #[map(default = true)]
    pub isolate: bool,
    /// The git remote, by name or address, each Mission's work is saved to, so a later run
    /// on another Station carries on from it. Empty pushes nothing.
    #[schema(title = "Save work to remote", section = "operation")]
    pub checkpoint_remote: Option<String>,
    /// Seconds between saves of a running Mission's work.
    #[schema(title = "Save every", format = "seconds", section = "operation")]
    #[map(default = 300)]
    pub checkpoint_secs: u64,
    /// Milliseconds one Git command may run before it is killed. Default 120000.
    #[map(default = 120_000)]
    pub git_timeout_ms: u64,
    /// Milliseconds between measurements of the directories a size-bounded command (a clone)
    /// fills. Default 250.
    #[map(default = 250)]
    pub measure_every_ms: u64,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            base: "HEAD".to_owned(),
            branch: "{id}-{slug}".to_owned(),
            isolate: true,
            checkpoint_remote: None,
            checkpoint_secs: 300,
            git_timeout_ms: 120_000,
            measure_every_ms: 250,
        }
    }
}

/// Makes a Mission's workspace with Git: a worktree, or a clone.
#[derive(Debug, Clone, Default, ai6::guatiao::Provider)]
#[provider(
    SafeHouseProvider,
    id = "git",
    name = "git",
    crate = ::ai6::guatiao,
    config = GitConfig
)]
pub struct Git {
    config: GitConfig,
}

impl Git {
    /// A provider with `config`.
    pub fn new(config: GitConfig) -> Self {
        Self { config }
    }
}

impl TryFrom<GitConfig> for Git {
    type Error = ProviderError;

    fn try_from(config: GitConfig) -> Result<Self, Self::Error> {
        Ok(Self::new(config))
    }
}

impl SafeHouseProvider for Git {
    fn open(&self, spec: &Value) -> Result<Object<dyn SafeHouseRun>, ProviderError> {
        let spec = SafeHouseSpec::from_value(spec)
            .map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error))?;
        let worktree = Worktree::open(
            &spec,
            Runner {
                timeout: std::time::Duration::from_millis(self.config.git_timeout_ms),
                measure_every: std::time::Duration::from_millis(self.config.measure_every_ms),
            },
        )
        .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        Ok(GitSafeHouse(worktree).into_object())
    }
}

struct GitSafeHouse(Worktree);

impl SafeHouseRun for GitSafeHouse {
    fn info(&self) -> Result<Value, ProviderError> {
        value(&SafeHouseInfo {
            path: self.0.path.to_string_lossy().into_owned(),
            branch: self.0.branch.clone(),
            base: Some(self.0.base.clone()),
            supports_history: true,
            isolated: self.0.isolated,
        })
    }

    fn changes(&self) -> Result<Value, ProviderError> {
        let changes = self
            .0
            .changes()
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        value(&ChangeLog { changes })
    }

    fn finish(&self) -> Result<Value, ProviderError> {
        let report = self
            .0
            .finish()
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        value(&report)
    }

    fn checkpoint(&self, spec: &Value) -> Result<Value, ProviderError> {
        let spec = CheckpointSpec::from_value(spec)
            .map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error))?;
        let saved = self
            .0
            .checkpoint(&spec)
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        value(&saved)
    }

    fn diff(&self, bytes: u64) -> Result<Value, ProviderError> {
        let diff = self
            .0
            .diff(usize::try_from(bytes).unwrap_or(usize::MAX))
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        value(&diff)
    }

    fn exports(&self) -> Result<Value, ProviderError> {
        let exports = self
            .0
            .exports()
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        value(&exports)
    }

    fn discard(&self) -> Result<(), ProviderError> {
        self.0
            .discard()
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))
    }
}

fn value(model: &impl ToValue) -> Result<Value, ProviderError> {
    model
        .to_value()
        .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))
}

fn refuse(status: Status, error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(status, &error.to_string())
}
