//! The formats this plugin keeps a file table in.

use coqui_access::guatiao::{FromValue, Schema, ToValue};
use coqui_access_kit::file::{Codec, CodecError};
use serde::Serialize;
use serde::de::DeserializeSeed;

/// How a file source is written: `toml` or `json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, rename_all = "lowercase")]
pub enum Format {
    /// A TOML document: the records are an array of tables.
    Toml,
    /// A JSON document, written indented.
    Json,
}

impl Codec for Format {
    fn name(&self) -> &'static str {
        match self {
            Self::Toml => "TOML",
            Self::Json => "JSON",
        }
    }

    fn write<D: Serialize + ?Sized>(&self, document: &D) -> Result<String, CodecError> {
        match self {
            Self::Toml => toml::to_string(document).map_err(|_| CodecError::new()),
            Self::Json => serde_json::to_string_pretty(document).map_err(|_| CodecError::new()),
        }
    }

    fn read<'de, S: DeserializeSeed<'de>>(
        &self,
        text: &'de str,
        seed: S,
    ) -> Result<S::Value, CodecError> {
        match self {
            Self::Toml => {
                let deserializer =
                    toml::Deserializer::parse(text).map_err(|_| CodecError::new())?;
                seed.deserialize(deserializer)
                    .map_err(|_| CodecError::new())
            }
            Self::Json => {
                let mut deserializer = serde_json::Deserializer::from_str(text);
                let read = seed
                    .deserialize(&mut deserializer)
                    .map_err(|_| CodecError::new())?;
                deserializer.end().map_err(|_| CodecError::new())?;
                Ok(read)
            }
        }
    }
}
