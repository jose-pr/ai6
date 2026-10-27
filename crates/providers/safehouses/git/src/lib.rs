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
    ChangeLog, CheckpointSpec, SafeHouseInfo, SafeHouseProvider, SafeHouseSession, SafeHouseSpec,
};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Status, ToValue, Value};

use worktree::Worktree;

#[derive(Default, ai6::guatiao::Provider)]
#[provider(
    SafeHouseProvider,
    id = "git",
    name = "git",
    crate = ::ai6::guatiao
)]
pub struct Git;

impl SafeHouseProvider for Git {
    fn open(&self, spec: &Value) -> Result<Object<dyn SafeHouseSession>, ProviderError> {
        let spec = SafeHouseSpec::from_value(spec)
            .map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error))?;
        let worktree =
            Worktree::open(&spec).map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        Ok(GitSafeHouse(worktree).into_object())
    }
}

struct GitSafeHouse(Worktree);

impl SafeHouseSession for GitSafeHouse {
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
