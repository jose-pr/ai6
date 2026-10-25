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
use ai6::guatiao::{FromValue, Status, ToValue, Value};

use tree::Tree;

#[derive(Default, ai6::guatiao::Provider)]
#[provider(
    SafeHouseProvider,
    id = "copy",
    name = "copy",
    crate = ::ai6::guatiao
)]
pub struct Copier;

impl SafeHouseProvider for Copier {
    fn open(&self, spec: &Value) -> Result<Object<dyn SafeHouseSession>, ProviderError> {
        let spec = SafeHouseSpec::from_value(spec)
            .map_err(|error| refuse(Status::GUATIAO_ERR_BAD_VALUE, error))?;
        let directory =
            Tree::open(&spec).map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
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
}

fn value(model: &impl ToValue) -> Result<Value, ProviderError> {
    model
        .to_value()
        .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))
}

fn refuse(status: Status, error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(status, &error.to_string())
}
