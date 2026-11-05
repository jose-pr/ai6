// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where secrets come from, as a Guatiao kind: a [`SecretProvider`] reads a secret by its key
//! and the further fields it declares, and may also lease, revoke and store.
//!
//! A provider is built from its instance's settings (`config`). [`SecretProvider::read`] answers
//! a [`SecretRead`]; [`SecretProvider::fields`] declares the schema of what one secret names
//! besides `key`. [`SecretProvider::lease`], [`SecretProvider::revoke`] and
//! [`SecretProvider::store`] are optional: their default answers `GUATIAO_ERR_UNSUPPORTED`. Slot
//! order is the ABI: a new slot is appended with a default body.
//!
//! `file`, `env` and `inline` are built in ([`builtin`]): a bootstrap file reads them before any
//! library loads, and a Station registers them before it scans for others.
//!
//! Provider calls block; call them from `spawn_blocking`, never on an async worker.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use guatiao::library::ProviderError;
use guatiao::{Alloc, FromValue, Map, Schema, Secret, Status, ToValue, Value};
use zeroize::{Zeroize, Zeroizing};

/// The keys a secret's own fields may not use: the reference's (`provider`, `key`, `secret`) and
/// AI6's (`env`, `green_ink`, `kinds`, `targets`).
pub const RESERVED_FIELDS: [&str; 7] = [
    "provider",
    "key",
    "secret",
    "env",
    "green_ink",
    "kinds",
    "targets",
];

/// The providers every program has without loading anything.
pub const BUILT_IN: [&str; 3] = ["file", "env", "inline"];

/// The provider of values sealed in the Registry to named Stations. A Station makes it itself, so
/// it needs a Registry and a sealing key: nothing a bootstrap file reads can be kept there.
pub const REGISTRY_PROVIDER: &str = "registry";

/// The fields of a secret reference or a provider instance beyond the keys AI6 reads, by name,
/// as they were written. Never printed: [`Debug`](fmt::Debug) shows the names alone, and every
/// text in it is overwritten with zeros when it is dropped, since a field such as `inline`'s
/// `value` may hold the secret itself.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretFields(serde_json::Map<String, serde_json::Value>);

impl SecretFields {
    /// The fields of `map`.
    pub fn new(map: serde_json::Map<String, serde_json::Value>) -> Self {
        Self(map)
    }

    /// The field `name`.
    pub fn get(&self, name: &str) -> Option<&serde_json::Value> {
        self.0.get(name)
    }

    /// The names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// Whether there is none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The fields as JSON, for a caller that hands them on.
    pub fn as_json(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.0
    }

    /// The fields as a Guatiao map built in `alloc`.
    ///
    /// # Errors
    ///
    /// A field Guatiao cannot hold, such as a number past its range.
    pub fn to_map_in(&self, alloc: Alloc) -> Result<Map, ProviderError> {
        use serde::de::DeserializeSeed as _;
        let value = guatiao_serde::ValueSeed::new(alloc)
            .deserialize(serde_json::Value::Object(self.0.clone()))
            .map_err(|_| bad_value("a field of the secret cannot be handed to its provider"))?;
        Map::try_from(value).map_err(|_| bad_value("the fields of a secret are not a map"))
    }
}

impl fmt::Debug for SecretFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

impl Drop for SecretFields {
    fn drop(&mut self) {
        fn wipe(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::String(text) => text.zeroize(),
                serde_json::Value::Array(values) => values.iter_mut().for_each(wipe),
                serde_json::Value::Object(map) => map.values_mut().for_each(wipe),
                _ => {}
            }
        }
        self.0.values_mut().for_each(wipe);
    }
}

/// Where one secret is: the provider (an instance's name, or a loaded provider's ID), its key
/// there, and the further fields that provider declares.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretSource {
    /// An instance (`[secret_provider.<name>]`) or a loaded provider with its default settings.
    pub provider: String,
    /// The secret at the provider: a path for `file`, a variable for `env`.
    pub key: String,
    /// What else the provider asks of one secret.
    pub fields: SecretFields,
}

impl SecretSource {
    /// The secret `key` of `provider`, with no further fields.
    pub fn new(provider: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            key: key.into(),
            fields: SecretFields::default(),
        }
    }
}

impl fmt::Debug for SecretSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretSource")
            .field("provider", &self.provider)
            .field("key", &self.key)
            .field("fields", &self.fields)
            .finish()
    }
}

/// What one secret names at its provider. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Default)]
#[map(crate = ::guatiao)]
pub struct SecretAsk {
    /// The secret at the provider.
    #[map(default)]
    pub key: String,
    /// The further fields the provider declared, by name.
    #[map(default)]
    pub fields: Map,
}

/// A secret's value as a provider read it. Crosses as a value.
#[derive(ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct SecretRead {
    /// The value.
    pub value: Secret<String>,
    /// The provider's revision of it, when it keeps one: what a later [`SecretStore::expect`]
    /// names.
    pub revision: Option<String>,
}

/// A request for a lease of one secret. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Default)]
#[map(crate = ::guatiao)]
pub struct SecretLeaseAsk {
    /// The secret at the provider.
    #[map(default)]
    pub key: String,
    /// The further fields the provider declared, by name.
    #[map(default)]
    pub fields: Map,
    /// Seconds the lease may last.
    pub ttl_secs: f64,
    /// The Station asking.
    pub station: String,
    /// The Mission, or other holder, the lease is for.
    pub holder: String,
}

/// A lease a provider issued. Crosses as a value.
#[derive(ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct SecretLeased {
    /// What [`SecretProvider::revoke`] takes.
    pub lease: String,
    /// The value.
    pub value: Secret<String>,
}

/// A value to keep. Crosses as a value.
#[derive(ToValue, FromValue, Schema)]
#[map(crate = ::guatiao)]
pub struct SecretStore {
    /// The secret at the provider.
    #[map(default)]
    pub key: String,
    /// The further fields the provider declared, by name.
    #[map(default)]
    pub fields: Map,
    /// The value.
    pub value: Secret<String>,
    /// The revision this value replaces, as a read answered it: the provider refuses the store
    /// with `GUATIAO_ERR_BUSY` when it holds another, so an older value never replaces a newer
    /// one. `None` stores without a check.
    pub expect: Option<String>,
}

/// What a store left. Crosses as a value.
#[derive(ToValue, FromValue, Schema, Debug, Default)]
#[map(crate = ::guatiao)]
pub struct SecretStored {
    /// The revision the provider now holds, when it keeps one.
    pub revision: Option<String>,
}

/// Reads secrets by key, and may lease, revoke and store them.
#[guatiao::kind(name = "ai6_secret")]
pub trait SecretProvider: Send + Sync {
    /// Reads `ask` (a [`SecretAsk`] as a value) and answers a [`SecretRead`]. An error's text
    /// never holds a value: AI6 withholds it from people anyway.
    #[sensitive]
    fn read(&self, ask: &Value) -> Result<Value, ProviderError>;

    /// The JSON Schema, as a value, of the fields one secret names besides `key`. A field may
    /// not take a name in [`RESERVED_FIELDS`]. The default declares none.
    fn fields(&self) -> Result<Value, ProviderError> {
        no_fields()
    }

    /// Leases `ask` (a [`SecretLeaseAsk`] as a value) and answers a [`SecretLeased`]. A
    /// provider that does not lease leaves the default, and is read instead.
    #[sensitive]
    fn lease(&self, ask: &Value) -> Result<Value, ProviderError> {
        let _ = ask;
        Err(not_offered("lease"))
    }

    /// Revokes the lease `lease`; one already gone is not an error.
    fn revoke(&self, lease: &str) -> Result<(), ProviderError> {
        let _ = lease;
        Err(not_offered("revoke"))
    }

    /// Keeps `request` (a [`SecretStore`] as a value) and answers a [`SecretStored`].
    #[sensitive]
    fn store(&self, request: &Value) -> Result<Value, ProviderError> {
        let _ = request;
        Err(not_offered("store"))
    }
}

fn not_offered(what: &str) -> ProviderError {
    ProviderError::new(
        Status::GUATIAO_ERR_UNSUPPORTED,
        &format!("this secret provider does not {what}"),
    )
}

fn bad_value(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

/// The schema of no further fields.
fn no_fields() -> Result<Value, ProviderError> {
    let mut schema = Map::new();
    schema.set("type", "object");
    schema.set("properties", Map::new());
    schema.set("additionalProperties", false);
    Ok(schema.into())
}

/// Why a store did not keep its value. `Debug` shows a provider's status, never its words.
#[non_exhaustive]
pub enum SecretStoreError {
    /// The provider holds a revision other than the one the store expected.
    Conflict,
    /// The provider does not store.
    NotOffered,
    /// Anything else, from the provider.
    Failed(ProviderError),
}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict => f.write_str("the secret changed since it was read"),
            Self::NotOffered => f.write_str("this secret provider does not store"),
            Self::Failed(_) => f.write_str("the secret provider did not store the value"),
        }
    }
}

impl fmt::Debug for SecretStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict => f.write_str("Conflict"),
            Self::NotOffered => f.write_str("NotOffered"),
            Self::Failed(error) => f
                .debug_tuple("Failed")
                .field(&format_args!("{:?}", error.status))
                .finish(),
        }
    }
}

impl std::error::Error for SecretStoreError {}

/// A built provider with its models spelled as Rust types, and the further fields it declared.
pub struct SecretKeeper {
    provider: Box<dyn SecretProvider>,
    fields: Vec<String>,
}

impl fmt::Debug for SecretKeeper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretKeeper")
            .field("fields", &self.fields)
            .finish_non_exhaustive()
    }
}

impl SecretKeeper {
    /// Wraps a built provider, reading the fields it declares.
    ///
    /// # Errors
    ///
    /// Why it is refused: its field schema cannot be read, or declares a name in
    /// [`RESERVED_FIELDS`].
    pub fn new(provider: Box<dyn SecretProvider>) -> Result<Self, String> {
        let schema = provider
            .fields()
            .map_err(|_| "its field schema could not be read".to_owned())?;
        let schema = serde_json::to_value(guatiao_serde::Serializable::from(&schema))
            .map_err(|_| "its field schema is not JSON".to_owned())?;
        let fields: Vec<String> = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .map(|properties| properties.keys().cloned().collect())
            .unwrap_or_default();
        if let Some(reserved) = fields
            .iter()
            .find(|field| RESERVED_FIELDS.contains(&field.as_str()))
        {
            return Err(format!(
                "it declares the field `{reserved}`, which a secret keeps for AI6 \
                 ({})",
                RESERVED_FIELDS.join(", ")
            ));
        }
        Ok(Self { provider, fields })
    }

    /// The fields one secret may name besides `key`.
    pub fn fields(&self) -> &[String] {
        &self.fields
    }

    /// Refuses a field this provider did not declare, naming it and never its value.
    ///
    /// # Errors
    ///
    /// Which field.
    pub fn check_fields(&self, fields: &SecretFields) -> Result<(), String> {
        match fields
            .names()
            .find(|name| !self.fields.iter().any(|f| f == name))
        {
            Some(unknown) => Err(format!("its provider declares no field `{unknown}`")),
            None => Ok(()),
        }
    }

    /// Reads `key` with `fields`.
    ///
    /// # Errors
    ///
    /// The provider's.
    pub fn read(&self, key: &str, fields: &SecretFields) -> Result<SecretRead, ProviderError> {
        let ask = SecretAsk {
            key: key.to_owned(),
            fields: fields.to_map_in(Alloc::wiping())?,
        };
        decode(&self.provider.read(&encode(&ask)?)?)
    }

    /// Leases `key` with `fields` for `ttl_secs`; `None` from a provider that does not lease.
    ///
    /// # Errors
    ///
    /// The provider's.
    pub fn lease(
        &self,
        key: &str,
        fields: &SecretFields,
        ttl_secs: f64,
        station: &str,
        holder: &str,
    ) -> Result<Option<SecretLeased>, ProviderError> {
        let ask = SecretLeaseAsk {
            key: key.to_owned(),
            fields: fields.to_map_in(Alloc::wiping())?,
            ttl_secs,
            station: station.to_owned(),
            holder: holder.to_owned(),
        };
        match self.provider.lease(&encode(&ask)?) {
            Ok(leased) => decode(&leased).map(Some),
            Err(error) if error.status == Status::GUATIAO_ERR_UNSUPPORTED => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Revokes `lease`.
    ///
    /// # Errors
    ///
    /// The provider's.
    pub fn revoke(&self, lease: &str) -> Result<(), ProviderError> {
        self.provider.revoke(lease)
    }

    /// Stores `value` as `key` with `fields`, replacing only the revision `expect` names when it
    /// names one.
    ///
    /// # Errors
    ///
    /// [`SecretStoreError::Conflict`] when the provider holds another revision.
    pub fn store(
        &self,
        key: &str,
        fields: &SecretFields,
        value: &str,
        expect: Option<&str>,
    ) -> Result<SecretStored, SecretStoreError> {
        let request = SecretStore {
            key: key.to_owned(),
            fields: fields
                .to_map_in(Alloc::wiping())
                .map_err(SecretStoreError::Failed)?,
            value: Secret::new(value.to_owned()),
            expect: expect.map(str::to_owned),
        };
        let encoded = request
            .to_value_in(Alloc::wiping())
            .map_err(|error| SecretStoreError::Failed(bad_value(&error.to_string())))?;
        match self.provider.store(&encoded) {
            Ok(stored) => decode(&stored).map_err(SecretStoreError::Failed),
            Err(error) if error.status == Status::GUATIAO_ERR_BUSY => {
                Err(SecretStoreError::Conflict)
            }
            Err(error) if error.status == Status::GUATIAO_ERR_UNSUPPORTED => {
                Err(SecretStoreError::NotOffered)
            }
            Err(error) => Err(SecretStoreError::Failed(error)),
        }
    }
}

fn encode(model: &impl ToValue) -> Result<Value, ProviderError> {
    model
        .to_value_in(Alloc::wiping())
        .map_err(|_| bad_value("a request to the secret provider could not be built"))
}

fn decode<T: FromValue>(value: &Value) -> Result<T, ProviderError> {
    // The reader's message names what it expected, never what it found.
    T::from_value(value).map_err(|error| bad_value(&error.to_string()))
}

/// Why a built-in provider could not read a secret: the kind of failure and a message naming
/// the file or variable, never the value.
pub(crate) type ReadFailure = (io::ErrorKind, String);

/// `file`: the file `key`, relative to `dir` when relative; one trailing newline is dropped.
pub(crate) fn read_file(dir: Option<&Path>, key: &str) -> Result<Zeroizing<String>, ReadFailure> {
    if key.trim().is_empty() {
        return Err((
            io::ErrorKind::InvalidInput,
            "a secret of the file provider names no file (`key`)".to_owned(),
        ));
    }
    let path = match dir {
        Some(dir) if Path::new(key).is_relative() => dir.join(key),
        _ => PathBuf::from(key),
    };
    let mut value = Zeroizing::new(
        std::fs::read_to_string(&path)
            .map_err(|error| (error.kind(), format!("{}: {error}", path.display())))?,
    );
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    if value.is_empty() {
        return Err((
            io::ErrorKind::InvalidData,
            format!("the secret file {} is empty", path.display()),
        ));
    }
    Ok(value)
}

/// `env`: the variable `key` of this process.
pub(crate) fn read_env(key: &str) -> Result<Zeroizing<String>, ReadFailure> {
    if !crate::directorates::q::transport::kit::is_variable_name(key) {
        return Err((
            io::ErrorKind::InvalidInput,
            format!("{key:?} is not an environment variable's name"),
        ));
    }
    match std::env::var(key) {
        Ok(value) if value.is_empty() => Err((
            io::ErrorKind::InvalidData,
            format!("the environment variable {key} is empty"),
        )),
        Ok(value) => Ok(Zeroizing::new(value)),
        Err(std::env::VarError::NotPresent) => Err((
            io::ErrorKind::NotFound,
            format!("the environment variable {key} is not set"),
        )),
        Err(std::env::VarError::NotUnicode(_)) => Err((
            io::ErrorKind::InvalidData,
            format!("the environment variable {key} is not UTF-8"),
        )),
    }
}

/// `inline`: the field `value`, which must be the only one.
pub(crate) fn read_inline(fields: &SecretFields) -> Result<Zeroizing<String>, ReadFailure> {
    if let Some(other) = fields.names().find(|name| *name != "value") {
        return Err((
            io::ErrorKind::InvalidInput,
            format!("the inline provider takes only `value`, not `{other}`"),
        ));
    }
    match fields.get("value").and_then(serde_json::Value::as_str) {
        Some(value) if !value.is_empty() => Ok(Zeroizing::new(value.to_owned())),
        _ => Err((
            io::ErrorKind::InvalidInput,
            "an inline secret holds its value as non-empty text in `value`".to_owned(),
        )),
    }
}

/// Refuses fields `provider` (a built-in) does not take; `None` for another provider, which
/// declares its own.
pub(crate) fn builtin_fields_check(provider: &str, fields: &SecretFields) -> Option<String> {
    match provider {
        "file" | "env" => fields
            .names()
            .next()
            .map(|name| format!("the {provider} provider takes no field `{name}`")),
        "inline" => read_inline(fields).err().map(|(_, message)| message),
        _ => None,
    }
}

/// Refuses settings a built-in provider instance does not take; `None` for another provider.
pub(crate) fn builtin_settings_check(provider: &str, settings: &SecretFields) -> Option<String> {
    let allowed: &[&str] = match provider {
        "file" => &["dir"],
        "env" | "inline" => &[],
        _ => return None,
    };
    if let Some(name) = settings.names().find(|name| !allowed.contains(name)) {
        return Some(format!("the {provider} provider has no setting `{name}`"));
    }
    if provider == "file"
        && settings
            .get("dir")
            .is_some_and(|dir| dir.as_str().is_none_or(|dir| dir.trim().is_empty()))
    {
        return Some("the file provider's `dir` is a non-empty path".to_owned());
    }
    None
}

/// The built-in providers, registered by a Station before it scans for others.
pub mod builtin {
    use std::path::PathBuf;

    use guatiao::library::ProviderError;
    use guatiao::{Alloc, FromValue, Schema, Secret, Status, ToValue, Value};

    use super::{
        SecretAsk, SecretFields, SecretProvider, SecretRead, read_env, read_file, read_inline,
    };

    /// `file`'s settings.
    #[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
    #[map(crate = ::guatiao, deny_unknown_fields)]
    pub struct FileSecretsConfig {
        /// The directory a relative key is read from. Default: the Station's secrets directory
        /// (`[station.secrets] dir`); the working directory where there is none.
        #[schema(title = "Directory")]
        pub dir: Option<PathBuf>,
    }

    /// `env`'s settings: none.
    #[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
    #[map(crate = ::guatiao, deny_unknown_fields)]
    pub struct EnvSecretsConfig {}

    /// `inline`'s settings: none.
    #[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq, Eq)]
    #[map(crate = ::guatiao, deny_unknown_fields)]
    pub struct InlineSecretsConfig {}

    /// What an inline secret names: its value.
    #[derive(ToValue, FromValue, Schema)]
    #[map(crate = ::guatiao, deny_unknown_fields)]
    pub struct InlineFields {
        /// The value itself, kept in the configuration: a weakening.
        #[schema(title = "Value")]
        pub value: Secret<String>,
    }

    /// Reads each secret from a file, `key` its path.
    #[derive(guatiao::Provider, Debug, Clone, Default)]
    #[provider(
        SecretProvider,
        id = "file",
        name = "Secret files",
        crate = ::guatiao,
        config = FileSecretsConfig
    )]
    pub struct FileSecrets {
        dir: Option<PathBuf>,
    }

    impl From<FileSecretsConfig> for FileSecrets {
        fn from(config: FileSecretsConfig) -> Self {
            Self { dir: config.dir }
        }
    }

    /// Reads each secret from the reading process's environment, `key` the variable.
    #[derive(guatiao::Provider, Debug, Clone, Default)]
    #[provider(
        SecretProvider,
        id = "env",
        name = "Environment variables",
        crate = ::guatiao,
        config = EnvSecretsConfig
    )]
    pub struct EnvSecrets;

    impl From<EnvSecretsConfig> for EnvSecrets {
        fn from(_: EnvSecretsConfig) -> Self {
            Self
        }
    }

    /// Reads each secret from its own `value` field.
    #[derive(guatiao::Provider, Debug, Clone, Default)]
    #[provider(
        SecretProvider,
        id = "inline",
        name = "Values written inline",
        crate = ::guatiao,
        config = InlineSecretsConfig
    )]
    pub struct InlineSecrets;

    impl From<InlineSecretsConfig> for InlineSecrets {
        fn from(_: InlineSecretsConfig) -> Self {
            Self
        }
    }

    fn fields_of(ask: &Value) -> Result<(SecretAsk, SecretFields), ProviderError> {
        let ask = SecretAsk::from_value(ask).map_err(|error| {
            ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &error.to_string())
        })?;
        let fields = serde_json::to_value(guatiao_serde::Serializable::from(&Value::from(
            ask.fields.clone_in(Alloc::wiping()).map_err(|_| {
                ProviderError::new(Status::GUATIAO_ERR_ALLOC, "the fields could not be read")
            })?,
        )))
        .ok()
        .and_then(|value| match value {
            serde_json::Value::Object(map) => Some(SecretFields::new(map)),
            _ => None,
        })
        .ok_or_else(|| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, "fields are not a map"))?;
        Ok((ask, fields))
    }

    fn answer(
        read: Result<zeroize::Zeroizing<String>, super::ReadFailure>,
        alloc: Alloc,
    ) -> Result<Value, ProviderError> {
        let value = read.map_err(|(kind, message)| {
            let status = if kind == std::io::ErrorKind::NotFound {
                Status::GUATIAO_ERR_NOT_FOUND
            } else {
                Status::GUATIAO_ERR_BAD_VALUE
            };
            ProviderError::new(status, &message)
        })?;
        SecretRead {
            value: Secret::new(value.as_str().to_owned()),
            revision: None,
        }
        .to_value_in(alloc)
        .map_err(|error| ProviderError::new(Status::GUATIAO_ERR_ALLOC, &error.to_string()))
    }

    impl SecretProvider for FileSecrets {
        fn read(&self, ask: &Value) -> Result<Value, ProviderError> {
            self.read_in(ask, Alloc::wiping())
        }

        fn read_in(&self, ask: &Value, alloc: Alloc) -> Result<Value, ProviderError> {
            let (ask, fields) = fields_of(ask)?;
            if let Some(refused) = super::builtin_fields_check("file", &fields) {
                return Err(ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &refused));
            }
            answer(read_file(self.dir.as_deref(), &ask.key), alloc)
        }
    }

    impl SecretProvider for EnvSecrets {
        fn read(&self, ask: &Value) -> Result<Value, ProviderError> {
            self.read_in(ask, Alloc::wiping())
        }

        fn read_in(&self, ask: &Value, alloc: Alloc) -> Result<Value, ProviderError> {
            let (ask, fields) = fields_of(ask)?;
            if let Some(refused) = super::builtin_fields_check("env", &fields) {
                return Err(ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, &refused));
            }
            answer(read_env(&ask.key), alloc)
        }
    }

    impl SecretProvider for InlineSecrets {
        fn read(&self, ask: &Value) -> Result<Value, ProviderError> {
            self.read_in(ask, Alloc::wiping())
        }

        fn read_in(&self, ask: &Value, alloc: Alloc) -> Result<Value, ProviderError> {
            let (_, fields) = fields_of(ask)?;
            answer(read_inline(&fields), alloc)
        }

        fn fields(&self) -> Result<Value, ProviderError> {
            InlineFields::schema().map_err(|error| {
                ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string())
            })
        }
    }

    guatiao::local_providers!(
        id = "ai6-secrets",
        version = env!("CARGO_PKG_VERSION"),
        providers = [FileSecrets, EnvSecrets, InlineSecrets]
    );
}

#[cfg(test)]
pub(crate) mod testing {
    //! A secret provider for tests: `fake-<key>-0451` for every key, leased, and its
    //! revocations recorded by holder.

    use std::sync::Mutex;

    use guatiao::library::ProviderError;
    use guatiao::{Alloc, FromValue, Schema, Secret, Status, ToValue, Value};

    use super::{SecretLeaseAsk, SecretLeased, SecretProvider, SecretRead};

    /// The leases the fake provider revoked, with the holder each was for.
    pub(crate) static REVOKED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    /// The leases the fake provider issued, with their holders.
    pub(crate) static ISSUED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    /// How many of `holder`'s leases the fake provider revoked.
    pub(crate) fn revoked_for(holder: &str) -> usize {
        REVOKED
            .lock()
            .unwrap()
            .iter()
            .filter(|(of, _)| of == holder)
            .count()
    }

    #[derive(ToValue, FromValue, Schema, Debug, Clone, Default)]
    #[map(crate = ::guatiao)]
    pub(crate) struct FakeSecretsConfig {}

    /// `fake-<key>-0451` for every key.
    #[derive(guatiao::Provider, Debug, Clone, Default)]
    #[provider(
        SecretProvider,
        id = "fake-secrets",
        name = "Fake secrets",
        crate = ::guatiao,
        config = FakeSecretsConfig
    )]
    pub(crate) struct FakeSecrets;

    impl From<FakeSecretsConfig> for FakeSecrets {
        fn from(_: FakeSecretsConfig) -> Self {
            Self
        }
    }

    fn error(message: &str) -> ProviderError {
        ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
    }

    impl SecretProvider for FakeSecrets {
        fn read(&self, ask: &Value) -> Result<Value, ProviderError> {
            let ask = super::SecretAsk::from_value(ask).map_err(|e| error(&e.to_string()))?;
            SecretRead {
                value: Secret::new(format!("fake-{}-0451", ask.key)),
                revision: None,
            }
            .to_value_in(Alloc::wiping())
            .map_err(|e| error(&e.to_string()))
        }

        fn lease(&self, ask: &Value) -> Result<Value, ProviderError> {
            let ask = SecretLeaseAsk::from_value(ask).map_err(|e| error(&e.to_string()))?;
            let lease = uuid::Uuid::new_v4().simple().to_string();
            ISSUED
                .lock()
                .unwrap()
                .push((ask.holder.clone(), lease.clone()));
            SecretLeased {
                lease,
                value: Secret::new(format!("fake-{}-0451", ask.key)),
            }
            .to_value_in(Alloc::wiping())
            .map_err(|e| error(&e.to_string()))
        }

        fn revoke(&self, lease: &str) -> Result<(), ProviderError> {
            let holder = ISSUED
                .lock()
                .unwrap()
                .iter()
                .find(|(_, issued)| issued == lease)
                .map(|(holder, _)| holder.clone())
                .unwrap_or_default();
            REVOKED.lock().unwrap().push((holder, lease.to_owned()));
            Ok(())
        }
    }

    guatiao::local_providers!(
        id = "ai6-fake-secrets",
        version = env!("CARGO_PKG_VERSION"),
        providers = [FakeSecrets]
    );
}
