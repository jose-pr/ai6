//! The settings of the configuration builders as one value type, and the field conversions the
//! settings types share.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::Display;
use core::str::FromStr;

use guatiao::schema::KindBuilder;
use guatiao::{Alloc, FromValue, MapError, Schema, ToValue, Value, ValueError};

use crate::policy::Policy;

/// What a configuration builder takes whole through `settings`: the application protocols and
/// the [`Policy`], in the form a file, a form or a peer fills.
///
/// Identities, trust and verifiers are not settings: they hold keys and code, so a builder
/// takes them as arguments. Every key is optional; an absent one is the default (no ALPN,
/// [`Policy::default`]), and an unknown key is refused.
///
/// The type is a guatiao value (`TlsSettings::from_value`, `to_value`, `schema`), and
/// `guatiao_serde::text::{toml, json, yaml}` read and write its text. It is `#[non_exhaustive]`:
/// start from [`Default`] and assign the fields.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
#[map(deny_unknown_fields)]
#[non_exhaustive]
pub struct TlsSettings {
    /// The application protocols offered, most preferred first, as text (`h2`, `coqui/1`), each
    /// 1 to 255 bytes. A protocol that is not text is set with the builder's `alpn`.
    #[map(default, with = alpn_list)]
    pub alpn: Vec<String>,
    /// The policy configurations are built under.
    #[map(default)]
    pub policy: Policy,
}

impl TlsSettings {
    /// The protocols as the bytes a configuration carries.
    pub(crate) fn alpn_bytes(&self) -> Vec<Vec<u8>> {
        self.alpn
            .iter()
            .map(|text| text.as_bytes().to_vec())
            .collect()
    }
}

fn bad(expected: String) -> MapError {
    MapError::BadValue {
        key: String::new(),
        expected,
    }
}

/// The longest application protocol name, in bytes.
const ALPN_MAX: usize = 255;

/// `#[map(with = ..)]` for a list of application protocol names, each 1 to 255 bytes.
pub(crate) mod alpn_list {
    use super::*;

    pub(crate) fn to_value_in(items: &[String], alloc: Alloc) -> Result<Value, ValueError> {
        items.to_value_in(alloc)
    }

    pub(crate) fn from_value(value: &Value) -> Result<Vec<String>, MapError> {
        let items = Vec::<String>::from_value(value)?;
        if items
            .iter()
            .any(|item| item.is_empty() || item.len() > ALPN_MAX)
        {
            return Err(bad("protocol names of 1 to 255 bytes".to_owned()));
        }
        Ok(items)
    }

    pub(crate) fn kind(alloc: Alloc) -> KindBuilder {
        KindBuilder::list_in(
            alloc,
            String::kind(alloc)
                .min_length(1)
                .max_length(ALPN_MAX as u64),
        )
    }
}

/// `#[map(with = ..)]` for a `Vec` of a type with a text form (`Display` out, `FromStr` in).
pub(crate) mod texts {
    use super::*;

    pub(crate) fn to_value_in<T: Display>(items: &[T], alloc: Alloc) -> Result<Value, ValueError> {
        items
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>()
            .to_value_in(alloc)
    }

    pub(crate) fn from_value<T: FromStr>(value: &Value) -> Result<Vec<T>, MapError>
    where
        T::Err: Display,
    {
        Vec::<String>::from_value(value)?
            .iter()
            .map(|text| text.parse().map_err(|error: T::Err| bad(error.to_string())))
            .collect()
    }

    pub(crate) fn kind(alloc: Alloc) -> KindBuilder {
        Vec::<String>::kind(alloc)
    }
}

/// `#[map(with = ..)]` for the pairing ALPN, bytes held as text of 1 to 255 bytes.
pub(crate) mod alpn_text {
    use super::*;

    pub(crate) fn to_value_in(alpn: &[u8], alloc: Alloc) -> Result<Value, ValueError> {
        String::from_utf8(alpn.to_vec())
            .map_err(|_| ValueError::Unrepresentable)?
            .to_value_in(alloc)
    }

    pub(crate) fn from_value(value: &Value) -> Result<Vec<u8>, MapError> {
        let alpn = String::from_value(value)?.into_bytes();
        if alpn.is_empty() || alpn.len() > ALPN_MAX {
            return Err(bad("a protocol name of 1 to 255 bytes".to_owned()));
        }
        Ok(alpn)
    }

    pub(crate) fn kind(alloc: Alloc) -> KindBuilder {
        String::kind(alloc)
            .min_length(1)
            .max_length(ALPN_MAX as u64)
    }
}
