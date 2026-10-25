// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Ephemeral Theaters, as a Guatiao kind: a provider that makes a fresh Theater for one
//! Mission from a template and destroys it afterwards (a container, a VM).
//!
//! A [`TheaterProvider`] is built from a template's settings. [`TheaterProvider::create`]
//! makes a Theater and answers how a transport reaches it; [`TheaterProvider::list`] finds
//! the ones it made, so a Station that restarts adopts them again; [`TheaterProvider::destroy`]
//! removes one. Slot order is the ABI: a new slot is appended with a default body.
//!
//! Provider calls block; call them from `spawn_blocking`, never on an async worker.

use guatiao::library::ProviderError;
use guatiao::{FromValue, Schema, Status, ToValue, Value};

use crate::directorates::q::transport::Capabilities;

/// A template's settings, serialized as TOML. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct TheaterProviderConfig {
    /// The template's `config` table as TOML; the provider refuses unknown keys.
    pub settings_toml: String,
}

/// What to make. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct TheaterRequest {
    /// The new Theater's ID: letters, digits, `.`, `_` and `-`, starting with a letter or
    /// digit. The provider names what it makes after it.
    pub name: String,
    /// The template it is made from, so [`TheaterProvider::list`] finds it again.
    pub template: String,
    /// The Mission it is made for.
    pub mission: String,
}

/// A Theater a provider made. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
pub struct MadeTheater {
    /// The Theater's ID, as requested.
    pub name: String,
    /// The template it was made from.
    pub template: String,
    /// The Mission it was made for.
    pub mission: String,
    /// When it was made, as Unix seconds.
    #[map(default)]
    pub created_at: u64,
    /// The transport provider that reaches it.
    pub transport: String,
    /// That transport's settings, as TOML.
    pub settings_toml: String,
}

/// The Theaters a provider made from one template. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq, Default)]
#[map(crate = ::guatiao)]
pub struct MadeTheaters {
    /// Oldest first.
    pub theaters: Vec<MadeTheater>,
}

/// Makes and destroys Theaters for single Missions.
#[guatiao::kind(name = "ai6_theater_provider")]
pub trait TheaterProvider: Send + Sync {
    /// Answers the [`Capabilities`] of the Theaters it makes, without making one.
    fn capabilities(&self) -> Result<Value, ProviderError>;

    /// Makes `request` (a [`TheaterRequest`] as a value) and answers a [`MadeTheater`]. Asked
    /// again for a name it already made, it answers that one.
    fn create(&self, request: &Value) -> Result<Value, ProviderError>;

    /// Destroys the Theater `name`; one already gone is not an error.
    fn destroy(&self, name: &str) -> Result<(), ProviderError>;

    /// Answers [`MadeTheaters`]: every Theater it made from `template` that still exists.
    fn list(&self, template: &str) -> Result<Value, ProviderError>;
}

/// A Theater provider with its models spelled as Rust types.
pub struct TheaterMaker {
    provider: Box<dyn TheaterProvider>,
}

impl TheaterMaker {
    /// Wraps a built provider.
    pub fn new(provider: Box<dyn TheaterProvider>) -> Self {
        Self { provider }
    }

    /// What the Theaters it makes can do.
    pub fn capabilities(&self) -> Result<Capabilities, ProviderError> {
        decode(&self.provider.capabilities()?)
    }

    /// Makes a Theater, or answers the one it already made under that name.
    pub fn create(&self, request: &TheaterRequest) -> Result<MadeTheater, ProviderError> {
        decode(&self.provider.create(&encode(request)?)?)
    }

    /// Destroys a Theater; one already gone is not an error.
    pub fn destroy(&self, name: &str) -> Result<(), ProviderError> {
        self.provider.destroy(name)
    }

    /// The Theaters it made from `template` that still exist, oldest first.
    pub fn list(&self, template: &str) -> Result<Vec<MadeTheater>, ProviderError> {
        decode::<MadeTheaters>(&self.provider.list(template)?).map(|made| made.theaters)
    }
}

fn encode(model: &impl ToValue) -> Result<Value, ProviderError> {
    model
        .to_value()
        .map_err(|error| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &error.to_string()))
}

fn decode<T: FromValue>(value: &Value) -> Result<T, ProviderError> {
    T::from_value(value)
        .map_err(|error| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A changed required signature affects every built provider.
    #[test]
    fn floor_hash_is_pinned() {
        assert_eq!(TheaterProviderVtable::FLOOR_HASH, 102960253);
    }

    #[test]
    fn a_made_theater_round_trips_and_reads_without_its_time() {
        let made = MadeTheater {
            name: "scratch-m1".to_owned(),
            template: "scratch".to_owned(),
            mission: "m1".to_owned(),
            created_at: 7,
            transport: "container".to_owned(),
            settings_toml: "container = \"ai6-scratch-m1\"\n".to_owned(),
        };
        let mut value = made.to_value().unwrap();
        assert_eq!(MadeTheater::from_value(&value).unwrap(), made);
        let map: &mut guatiao::Map = guatiao::TryAsMut::try_as_mut(&mut value).unwrap();
        map.remove("created_at");
        assert_eq!(MadeTheater::from_value(&value).unwrap().created_at, 0);
    }
}
