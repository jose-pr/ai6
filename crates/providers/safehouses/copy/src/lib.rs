// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `ai6-safehouse-copy`: the `copy` safe-house provider. Each Mission gets a plain copy of
//! the source directory and the copy is always kept. There is no history, so the provider
//! reports none and a cover check says the work is not checkable.

#![doc = include_str!("../README.md")]

ai6::guatiao::providers!(Copier);

mod tree;

use ai6::directorates::q::safehouse::{
    ChangeLog, SafeHouseInfo, SafeHouseProvider, SafeHouseReport, SafeHouseSession, SafeHouseSpec,
};
use ai6::guatiao::library::{Object, ProviderError};
use ai6::guatiao::{FromValue, Schema, Status, ToValue, Value};

use tree::Tree;

/// `copy` provider configuration; every field has a default.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq)]
#[map(crate = ::ai6::guatiao)]
pub struct CopyConfig {
    /// Characters of an Operation or Mission ID kept in the name of its directory (a hash of
    /// the whole ID follows, so distinct IDs never collide). Default 72.
    #[map(default = 72)]
    pub name_chars: u64,
}

impl Default for CopyConfig {
    fn default() -> Self {
        Self { name_chars: 72 }
    }
}

/// Makes a Mission's workspace as a plain copy of the source directory.
#[derive(Debug, Clone, Copy, Default, ai6::guatiao::Provider)]
#[provider(
    SafeHouseProvider,
    id = "copy",
    name = "copy",
    crate = ::ai6::guatiao,
    config = CopyConfig
)]
pub struct Copier {
    config: CopyConfig,
}

impl Copier {
    /// A provider with `config`.
    pub fn new(config: CopyConfig) -> Self {
        Self { config }
    }
}

impl TryFrom<CopyConfig> for Copier {
    type Error = ProviderError;

    fn try_from(config: CopyConfig) -> Result<Self, Self::Error> {
        Ok(Self::new(config))
    }
}

impl SafeHouseProvider for Copier {
    fn open(&self, spec: &Value) -> Result<Object<dyn SafeHouseSession>, ProviderError> {
        let spec = SafeHouseSpec::from_value(spec)
            .map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error))?;
        if spec.clone.is_some() {
            return Err(refuse(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "the copy provider does not clone repositories",
            ));
        }
        let directory = Tree::open(
            &spec,
            usize::try_from(self.config.name_chars).unwrap_or(usize::MAX),
        )
        .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        Ok(CopySafeHouse(directory).into_object())
    }
}

struct CopySafeHouse(Tree);

impl SafeHouseSession for CopySafeHouse {
    fn info(&self) -> Result<Value, ProviderError> {
        value(&SafeHouseInfo {
            path: self.0.path.to_string_lossy().into_owned(),
            branch: None,
            base: None,
            supports_history: false,
            isolated: self.0.isolated,
        })
    }

    fn changes(&self) -> Result<Value, ProviderError> {
        value(&ChangeLog::default())
    }

    fn finish(&self) -> Result<Value, ProviderError> {
        value(&SafeHouseReport {
            path: self.0.path.to_string_lossy().into_owned(),
            branch: None,
            base: None,
            kept: self.0.path.exists(),
            changes: Vec::new(),
        })
    }

    fn discard(&self) -> Result<(), ProviderError> {
        if !self.0.isolated {
            return Err(refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                "a safe house in the source itself is never removed",
            ));
        }
        match std::fs::remove_dir_all(&self.0.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!("{}: {error}", self.0.path.display()),
            )),
        }
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
