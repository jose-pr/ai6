//! Values sealed to named Stations: the Station's sealing key, the [`Vault`] that seals and opens
//! what the Registry holds, and the `registry` secret provider over it.
//!
//! A value is sealed separately to each Station, in a sealed box (X25519, XSalsa20-Poly1305) only
//! that Station's key opens. What is sealed names who it is sealed to, with their public keys, so
//! a later write goes to those Stations and no others whatever the Registry's records say: writing
//! a Station into a record gives it nothing. A Station's key is its own, kept beside its identity:
//! the identity's key signs TLS handshakes, may come from a CA and may be replaced, and a signing
//! key is not used to decrypt.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use crypto_box::aead::OsRng;
use guatiao::library::ProviderError;
use guatiao::{Alloc, FromValue, Schema, Secret, Status, ToValue, Value};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

pub use super::secret_kind::REGISTRY_PROVIDER;
use super::secret_kind::{SecretAsk, SecretProvider, SecretRead, SecretStore, SecretStored};
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::sealed::{SealedCopy, SealedExpect, SealedWriteError};

/// The file beside a Station's identity that holds its sealing key.
pub const SEALING_KEY_FILE: &str = "sealing.key";

fn base64_text(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn from_base64(text: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

/// A Station's private sealing key. `Debug` shows only the public half.
pub struct SealingKey(crypto_box::SecretKey);

impl SealingKey {
    /// A new key.
    pub fn generate() -> Self {
        Self(crypto_box::SecretKey::generate(&mut OsRng))
    }

    /// The key in `path`, creating it readable only by this user on first use.
    ///
    /// # Errors
    ///
    /// When the file cannot be read or made, or holds no key; the message names the path, never
    /// the key.
    pub fn open_or_create(path: &Path) -> io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let text = Zeroizing::new(text);
                let bytes = Zeroizing::new(from_base64(text.trim()).ok_or_else(|| bad_key(path))?);
                let bytes: [u8; 32] = bytes.as_slice().try_into().map_err(|_| bad_key(path))?;
                Ok(Self(crypto_box::SecretKey::from_bytes(bytes)))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let key = Self::generate();
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let text = Zeroizing::new(base64_text(&key.0.to_bytes()) + "\n");
                match write_new_private(path, text.as_bytes()) {
                    Ok(()) => Ok(key),
                    // Another process made it first: its key is the one.
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        Self::open_or_create(path)
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// The public half.
    pub fn public_key(&self) -> SealingPublicKey {
        SealingPublicKey(self.0.public_key())
    }

    fn open(&self, sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
        self.0.unseal(sealed).ok().map(Zeroizing::new)
    }
}

impl fmt::Debug for SealingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SealingKey")
            .field(&self.public_key())
            .finish()
    }
}

fn bad_key(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{} holds no sealing key", path.display()),
    )
}

#[cfg(unix)]
fn write_new_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Windows keeps the access list the file inherits from the Station's state directory.
#[cfg(not(unix))]
fn write_new_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// A Station's public sealing key.
#[derive(Clone, PartialEq, Eq)]
pub struct SealingPublicKey(crypto_box::PublicKey);

impl SealingPublicKey {
    /// The key as the Registry records it: base64.
    pub fn to_text(&self) -> String {
        base64_text(self.0.as_bytes())
    }

    /// The key `text` spells, or `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes: [u8; 32] = from_base64(text)?.try_into().ok()?;
        Some(Self(crypto_box::PublicKey::from_bytes(bytes)))
    }

    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, VaultError> {
        self.0
            .seal(&mut OsRng, plain)
            .map_err(|_| VaultError::Sealing)
    }
}

impl fmt::Debug for SealingPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SealingPublicKey")
            .field(&self.to_text())
            .finish()
    }
}

/// Why a [`Vault`] could not do what it was asked. Never holds a value.
#[non_exhaustive]
#[derive(Debug)]
pub enum VaultError {
    /// There is no such value.
    NotFound,
    /// The value is not sealed to this Station.
    NotNamed,
    /// This Station's copy does not open with its key, or is not what a [`Vault`] seals.
    Unreadable,
    /// A Station has published no usable sealing key.
    NoKey(String),
    /// A Station's published key is not signed by the identity named for it.
    Unverified(String),
    /// Nothing could be sealed.
    Sealing,
    /// The value is at another revision than the one asked about.
    Conflict {
        /// What it is at; `None` when it does not exist.
        held: Option<u64>,
    },
    /// The Registry failed.
    Registry(io::Error),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("no such sealed value"),
            Self::NotNamed => f.write_str("the value is not sealed to this Station"),
            Self::Unreadable => f.write_str("this Station's sealed copy cannot be opened"),
            Self::NoKey(station) => write!(f, "Station {station:?} has no usable sealing key"),
            Self::Unverified(station) => write!(
                f,
                "Station {station:?}'s sealing key is not signed by the identity named for it"
            ),
            Self::Sealing => f.write_str("the value could not be sealed"),
            Self::Conflict { held: Some(held) } => {
                write!(f, "the sealed value is at revision {held} now")
            }
            Self::Conflict { held: None } => f.write_str("the sealed value does not exist"),
            Self::Registry(error) => write!(f, "the Registry failed: {error}"),
        }
    }
}

impl std::error::Error for VaultError {}

impl From<io::Error> for VaultError {
    fn from(error: io::Error) -> Self {
        Self::Registry(error)
    }
}

impl From<SealedWriteError> for VaultError {
    fn from(error: SealedWriteError) -> Self {
        match error {
            SealedWriteError::Conflict { held } => Self::Conflict { held },
            SealedWriteError::Io(error) => Self::Registry(error),
        }
    }
}

/// What is sealed to each Station.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    /// Who it is sealed to: Station ID and public key. A later write seals to exactly these.
    recipients: BTreeMap<String, String>,
    value: String,
}

impl Drop for Envelope {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

/// A value this Station opened. `Debug` shows who holds it and its revision.
pub struct Opened {
    /// The value.
    pub value: Zeroizing<String>,
    /// The revision it was read at.
    pub revision: u64,
    /// The Stations it is sealed to.
    pub stations: BTreeSet<String>,
    recipients: BTreeMap<String, String>,
}

impl fmt::Debug for Opened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opened")
            .field("revision", &self.revision)
            .field("stations", &self.stations)
            .finish_non_exhaustive()
    }
}

/// A Station to seal to, and optionally the identity its key must be signed by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    station: String,
    fingerprint: Option<String>,
}

impl Recipient {
    /// The Station `station`, on the strength of the key it published.
    pub fn named(station: impl Into<String>) -> Self {
        Self {
            station: station.into(),
            fingerprint: None,
        }
    }

    /// Also requires its key to be signed by the identity with this fingerprint.
    #[must_use]
    pub fn pinned(mut self, fingerprint: impl Into<String>) -> Self {
        self.fingerprint = Some(fingerprint.into());
        self
    }

    /// The Station.
    pub fn station(&self) -> &str {
        &self.station
    }
}

/// One Station's access to what is sealed in the Registry.
pub struct Vault {
    registry: Arc<dyn RegistryBackend>,
    station: String,
    key: Arc<SealingKey>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vault")
            .field("station", &self.station)
            .finish_non_exhaustive()
    }
}

impl Vault {
    /// `station`'s access, with its key.
    pub fn new(
        registry: Arc<dyn RegistryBackend>,
        station: impl Into<String>,
        key: Arc<SealingKey>,
    ) -> Self {
        Self {
            registry,
            station: station.into(),
            key,
        }
    }

    /// The Station.
    pub fn station(&self) -> &str {
        &self.station
    }

    /// The Registry it reads.
    pub fn registry(&self) -> &dyn RegistryBackend {
        self.registry.as_ref()
    }

    /// Opens `name` with this Station's key.
    ///
    /// # Errors
    ///
    /// [`VaultError::NotFound`], [`VaultError::NotNamed`], [`VaultError::Unreadable`], or the
    /// Registry's.
    pub fn open(&self, name: &str) -> Result<Opened, VaultError> {
        let record = self
            .registry
            .sealed_value(name)?
            .ok_or(VaultError::NotFound)?;
        let copy = record
            .copies
            .get(&self.station)
            .ok_or(VaultError::NotNamed)?;
        let sealed = from_base64(&copy.sealed).ok_or(VaultError::Unreadable)?;
        let plain = self.key.open(&sealed).ok_or(VaultError::Unreadable)?;
        let envelope: Envelope =
            serde_json::from_slice(&plain).map_err(|_| VaultError::Unreadable)?;
        Ok(Opened {
            value: Zeroizing::new(envelope.value.clone()),
            revision: record.revision,
            stations: envelope.recipients.keys().cloned().collect(),
            recipients: envelope.recipients.clone(),
        })
    }

    /// Seals `value` to `stations`, as `expect` allows; the caller need not be one of them.
    ///
    /// # Errors
    ///
    /// [`VaultError::NoKey`] or [`VaultError::Unverified`] for a Station, [`VaultError::Conflict`],
    /// or the Registry's.
    pub fn seal_to(
        &self,
        name: &str,
        value: &str,
        stations: &[Recipient],
        expect: SealedExpect,
        now: f64,
    ) -> Result<u64, VaultError> {
        if stations.is_empty() {
            return Err(VaultError::Sealing);
        }
        let mut recipients = BTreeMap::new();
        for recipient in stations {
            recipients.insert(
                recipient.station.clone(),
                self.published_key(recipient)?.to_text(),
            );
        }
        self.write(name, value, &recipients, expect, now)
    }

    /// Replaces `name` with `value`, sealed to the Stations it names, when it is at `expect`.
    /// Only a Station it is sealed to can.
    ///
    /// # Errors
    ///
    /// [`VaultError::Conflict`] when it is at another revision; those of [`open`](Self::open).
    pub fn replace(
        &self,
        name: &str,
        value: &str,
        expect: u64,
        now: f64,
    ) -> Result<u64, VaultError> {
        let current = self.open(name)?;
        if current.revision != expect {
            return Err(VaultError::Conflict {
                held: Some(current.revision),
            });
        }
        self.write(
            name,
            value,
            &current.recipients,
            SealedExpect::Revision(expect),
            now,
        )
    }

    /// Seals `name` to one more Station, as of the revision this Station can open. Only a Station
    /// it is sealed to can.
    ///
    /// # Errors
    ///
    /// Those of [`seal_to`](Self::seal_to) and [`open`](Self::open).
    pub fn add_station(
        &self,
        name: &str,
        station: &Recipient,
        now: f64,
    ) -> Result<u64, VaultError> {
        let current = self.open(name)?;
        let mut recipients = current.recipients.clone();
        recipients.insert(
            station.station.clone(),
            self.published_key(station)?.to_text(),
        );
        self.write(
            name,
            &current.value,
            &recipients,
            SealedExpect::Revision(current.revision),
            now,
        )
    }

    /// Stops sealing `name` to `station` from the next revision on: what that Station already
    /// opened stays with it, and a login it held should be replaced. Only a Station it is sealed
    /// to can, and the last Station cannot be removed.
    ///
    /// # Errors
    ///
    /// Those of [`open`](Self::open), and [`VaultError::Sealing`] for the last Station.
    pub fn remove_station(&self, name: &str, station: &str, now: f64) -> Result<u64, VaultError> {
        let current = self.open(name)?;
        let mut recipients = current.recipients.clone();
        recipients.remove(station);
        if recipients.is_empty() {
            return Err(VaultError::Sealing);
        }
        self.write(
            name,
            &current.value,
            &recipients,
            SealedExpect::Revision(current.revision),
            now,
        )
    }

    fn published_key(&self, recipient: &Recipient) -> Result<SealingPublicKey, VaultError> {
        let station = &recipient.station;
        let record = self
            .registry
            .sealing_key(station)?
            .ok_or_else(|| VaultError::NoKey(station.clone()))?;
        check_published(&record, recipient.fingerprint.as_deref())
            .map_err(|()| VaultError::Unverified(station.clone()))?;
        SealingPublicKey::parse(&record.public_key)
            .ok_or_else(|| VaultError::NoKey(station.clone()))
    }

    fn write(
        &self,
        name: &str,
        value: &str,
        recipients: &BTreeMap<String, String>,
        expect: SealedExpect,
        now: f64,
    ) -> Result<u64, VaultError> {
        let envelope = Envelope {
            recipients: recipients.clone(),
            value: value.to_owned(),
        };
        let plain = Zeroizing::new(serde_json::to_vec(&envelope).map_err(|_| VaultError::Sealing)?);
        let mut copies = BTreeMap::new();
        for (station, key) in recipients {
            let public =
                SealingPublicKey::parse(key).ok_or_else(|| VaultError::NoKey(station.clone()))?;
            copies.insert(
                station.clone(),
                SealedCopy {
                    key: key.clone(),
                    sealed: base64_text(&public.seal(&plain)?),
                },
            );
        }
        Ok(self
            .registry
            .store_sealed(name, &copies, &self.station, expect, now)?)
    }
}

/// Whether a published key is what a seal may trust: signed by the identity `fingerprint` names
/// when one is named, and a signature it carries must hold either way.
#[cfg(feature = "station-tls")]
fn check_published(
    record: &crate::directorates::registry::sealed::StationSealingKey,
    fingerprint: Option<&str>,
) -> Result<(), ()> {
    crate::station::identity::check_sealing_key(record, fingerprint)
}

/// A build with no Station identity cannot check a signature, so it cannot honour a named one.
#[cfg(not(feature = "station-tls"))]
fn check_published(
    _record: &crate::directorates::registry::sealed::StationSealingKey,
    fingerprint: Option<&str>,
) -> Result<(), ()> {
    if fingerprint.is_some() {
        Err(())
    } else {
        Ok(())
    }
}

/// What one sealed secret names besides `key`.
#[derive(ToValue, FromValue, Schema, Default)]
#[map(crate = ::guatiao, deny_unknown_fields)]
pub struct RegistrySecretFields {
    /// The Stations a new value is sealed to. A value that exists keeps the Stations it names;
    /// another Station is added by one that holds it.
    #[schema(title = "Stations a new value is sealed to")]
    pub stations: Option<Vec<String>>,
}

/// `registry`: values sealed in the Registry to named Stations.
#[derive(Debug)]
pub struct RegistrySecrets {
    vault: Arc<Vault>,
}

impl RegistrySecrets {
    /// The provider over `vault`.
    pub fn new(vault: Arc<Vault>) -> Self {
        Self { vault }
    }

    fn now() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_secs_f64())
    }
}

fn provider_error(error: &VaultError) -> ProviderError {
    let status = match error {
        VaultError::NotFound => Status::GUATIAO_ERR_NOT_FOUND,
        VaultError::Conflict { .. } => Status::GUATIAO_ERR_BUSY,
        _ => Status::GUATIAO_ERR_BAD_VALUE,
    };
    ProviderError::new(status, &error.to_string())
}

fn bad_value(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

impl SecretProvider for RegistrySecrets {
    fn read(&self, ask: &Value) -> Result<Value, ProviderError> {
        let ask = SecretAsk::from_value(ask).map_err(|error| bad_value(&error.to_string()))?;
        let opened = self
            .vault
            .open(&ask.key)
            .map_err(|error| provider_error(&error))?;
        SecretRead {
            value: Secret::new(opened.value.as_str().to_owned()),
            revision: Some(opened.revision.to_string()),
        }
        .to_value_in(Alloc::wiping())
        .map_err(|error| bad_value(&error.to_string()))
    }

    fn fields(&self) -> Result<Value, ProviderError> {
        RegistrySecretFields::schema()
            .map_err(|error| ProviderError::new(Status::GUATIAO_ERR_INTERNAL, &error.to_string()))
    }

    fn store(&self, request: &Value) -> Result<Value, ProviderError> {
        let request =
            SecretStore::from_value(request).map_err(|error| bad_value(&error.to_string()))?;
        let fields = RegistrySecretFields::from_value(&Value::from(
            request
                .fields
                .clone_in(Alloc::wiping())
                .map_err(|_| bad_value("the fields could not be read"))?,
        ))
        .map_err(|error| bad_value(&error.to_string()))?;
        let expect = request
            .expect
            .as_deref()
            .map(|revision| {
                revision
                    .parse::<u64>()
                    .map_err(|_| bad_value("the expected revision is not this provider's"))
            })
            .transpose()?;
        let now = Self::now();
        let value = request.value.expose();
        let revision = match expect {
            Some(expect) => self.vault.replace(&request.key, value, expect, now),
            None => match self.vault.open(&request.key) {
                Ok(current) => self
                    .vault
                    .replace(&request.key, value, current.revision, now),
                Err(VaultError::NotFound) => {
                    let stations: Vec<Recipient> = fields
                        .stations
                        .unwrap_or_else(|| vec![self.vault.station.clone()])
                        .into_iter()
                        .map(Recipient::named)
                        .collect();
                    self.vault
                        .seal_to(&request.key, value, &stations, SealedExpect::Absent, now)
                }
                Err(error) => Err(error),
            },
        }
        .map_err(|error| provider_error(&error))?;
        SecretStored {
            revision: Some(revision.to_string()),
        }
        .to_value_in(Alloc::wiping())
        .map_err(|error| bad_value(&error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::q::secret_kind::{SecretFields, SecretKeeper, SecretStoreError};
    use crate::directorates::registry::sealed::StationSealingKey;
    use crate::directorates::registry::store::FileRegistry;

    const NOW: f64 = 1_790_000_000.0;
    const PLANTED: &str = "planted-secret-0451";

    /// A Station with its key published in `registry`.
    fn station(registry: &Arc<dyn RegistryBackend>, name: &str) -> Vault {
        let key = Arc::new(SealingKey::generate());
        registry
            .publish_sealing_key(&StationSealingKey::new(name, key.public_key().to_text()))
            .unwrap();
        Vault::new(registry.clone(), name, key)
    }

    fn registry(dir: &Path) -> Arc<dyn RegistryBackend> {
        Arc::new(FileRegistry::open(dir))
    }

    fn named(stations: &[&str]) -> Vec<Recipient> {
        stations
            .iter()
            .map(|name| Recipient::named(*name))
            .collect()
    }

    #[test]
    fn only_the_stations_named_can_open_a_value() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path());
        let (a, b, c) = (
            station(&registry, "a"),
            station(&registry, "b"),
            station(&registry, "c"),
        );
        a.seal_to("v", PLANTED, &named(&["a", "b"]), SealedExpect::Absent, NOW)
            .unwrap();
        assert_eq!(a.open("v").unwrap().value.as_str(), PLANTED);
        assert_eq!(b.open("v").unwrap().value.as_str(), PLANTED);
        assert!(matches!(c.open("v"), Err(VaultError::NotNamed)));
        assert!(matches!(a.open("other"), Err(VaultError::NotFound)));
    }

    #[test]
    fn a_station_written_into_the_record_gets_nothing_at_the_next_write() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path());
        let (a, thief) = (station(&registry, "a"), station(&registry, "thief"));
        a.seal_to("v", "one", &named(&["a"]), SealedExpect::Absent, NOW)
            .unwrap();
        let mut record = registry.sealed_value("v").unwrap().unwrap();
        record.copies.insert(
            "thief".to_owned(),
            SealedCopy {
                key: thief.key.public_key().to_text(),
                sealed: record.copies["a"].sealed.clone(),
            },
        );
        registry
            .store_sealed("v", &record.copies, "a", SealedExpect::Any, NOW)
            .unwrap();
        // Its copy of another Station's box does not open; the next write leaves it out.
        assert!(matches!(thief.open("v"), Err(VaultError::Unreadable)));
        let held = a.open("v").unwrap();
        assert_eq!(held.stations, BTreeSet::from(["a".to_owned()]));
        a.replace("v", "two", held.revision, NOW).unwrap();
        let record = registry.sealed_value("v").unwrap().unwrap();
        assert_eq!(record.copies.keys().collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn a_key_replaced_in_the_registry_does_not_redirect_a_later_write() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path());
        let (a, b) = (station(&registry, "a"), station(&registry, "b"));
        a.seal_to("v", "one", &named(&["a", "b"]), SealedExpect::Absent, NOW)
            .unwrap();
        let other = SealingKey::generate();
        registry
            .publish_sealing_key(&StationSealingKey::new("b", other.public_key().to_text()))
            .unwrap();
        a.replace("v", "two", 1, NOW).unwrap();
        assert_eq!(b.open("v").unwrap().value.as_str(), "two");
    }

    #[test]
    fn a_station_is_added_and_removed_only_by_one_that_holds_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path());
        let (a, b, c) = (
            station(&registry, "a"),
            station(&registry, "b"),
            station(&registry, "c"),
        );
        a.seal_to("v", PLANTED, &named(&["a"]), SealedExpect::Absent, NOW)
            .unwrap();
        assert!(matches!(
            c.add_station("v", &Recipient::named("c"), NOW),
            Err(VaultError::NotNamed)
        ));
        a.add_station("v", &Recipient::named("b"), NOW).unwrap();
        assert_eq!(b.open("v").unwrap().value.as_str(), PLANTED);
        a.remove_station("v", "b", NOW).unwrap();
        assert!(matches!(b.open("v"), Err(VaultError::NotNamed)));
        assert!(matches!(
            a.remove_station("v", "a", NOW),
            Err(VaultError::Sealing)
        ));
        assert!(matches!(
            a.add_station("v", &Recipient::named("nobody"), NOW),
            Err(VaultError::NoKey(_))
        ));
    }

    #[test]
    fn the_key_file_is_made_once_and_read_back_and_a_bad_one_is_refused_by_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SEALING_KEY_FILE);
        let made = SealingKey::open_or_create(&path).unwrap();
        let again = SealingKey::open_or_create(&path).unwrap();
        assert_eq!(made.public_key(), again.public_key());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!format!("{made:?}").contains(text.trim()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::write(&path, "not a key\n").unwrap();
        let refused = SealingKey::open_or_create(&path).unwrap_err().to_string();
        assert!(refused.contains(SEALING_KEY_FILE), "{refused}");
        assert!(!refused.contains("not a key"), "{refused}");
    }

    #[test]
    fn the_provider_reads_a_revision_and_refuses_an_older_store_with_busy() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path());
        let a = Arc::new(station(&registry, "a"));
        let b = station(&registry, "b");
        let keeper = SecretKeeper::new(Box::new(RegistrySecrets::new(a))).unwrap();
        assert_eq!(keeper.fields(), ["stations"]);
        let fields = SecretFields::new(
            serde_json::json!({"stations": ["a", "b"]})
                .as_object()
                .unwrap()
                .clone(),
        );
        keeper.check_fields(&fields).unwrap();
        let stored = keeper.store("v", &fields, "one", None).unwrap();
        assert_eq!(stored.revision.as_deref(), Some("1"));
        let read = keeper.read("v", &SecretFields::default()).unwrap();
        assert_eq!(read.value.expose(), "one");
        assert_eq!(read.revision.as_deref(), Some("1"));
        assert_eq!(b.open("v").unwrap().value.as_str(), "one");
        keeper
            .store("v", &SecretFields::default(), "two", Some("1"))
            .unwrap();
        assert!(matches!(
            keeper.store("v", &SecretFields::default(), "old", Some("1")),
            Err(SecretStoreError::Conflict)
        ));
        assert_eq!(
            keeper
                .read("v", &SecretFields::default())
                .unwrap()
                .value
                .expose(),
            "two"
        );
    }

    #[test]
    fn a_planted_value_appears_in_no_record_debug_or_error() {
        let dir = tempfile::tempdir().unwrap();
        let registry = registry(dir.path());
        let (a, c) = (station(&registry, "a"), station(&registry, "c"));
        a.seal_to("v", PLANTED, &named(&["a"]), SealedExpect::Absent, NOW)
            .unwrap();
        let mut seen = String::new();
        for entry in walk(dir.path()) {
            seen.push_str(&String::from_utf8_lossy(&std::fs::read(entry).unwrap()));
        }
        let record = registry.sealed_value("v").unwrap().unwrap();
        seen.push_str(&format!("{record:?}"));
        seen.push_str(&serde_json::to_string(&record).unwrap());
        // What a copy holds is a sealed box, not an encoding of the value.
        let bytes = from_base64(&record.copies["a"].sealed).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_err());
        seen.push_str(&String::from_utf8_lossy(&bytes));
        let opened = a.open("v").unwrap();
        seen.push_str(&format!("{opened:?} {a:?}"));
        seen.push_str(&c.open("v").unwrap_err().to_string());
        assert_eq!(opened.value.as_str(), PLANTED, "it was there to be found");
        assert!(!seen.contains(PLANTED), "{seen}");
        assert!(!seen.contains(&base64_text(PLANTED.as_bytes())), "{seen}");
    }

    pub(crate) fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(walk(&path));
            } else {
                files.push(path);
            }
        }
        files
    }
}
