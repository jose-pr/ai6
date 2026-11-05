//! Values sealed to named Stations, and the public keys they are sealed with.
//!
//! The Registry holds only ciphertext and who may open it: no key that opens one, and nothing a
//! reader can use. A value's revision is the truth about which copy is newest: a write that names
//! the revision it replaces is refused when the Registry holds another, in the same lock or
//! transaction that serializes every other write, so an older value never replaces a newer one.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::limits::AccountLimits;
use super::store::{FileRegistry, invalid_data};

/// One Station's copy of a sealed value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedCopy {
    /// The public key it is sealed to, base64.
    pub key: String,
    /// The sealed box, base64.
    pub sealed: String,
}

impl fmt::Debug for SealedCopy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SealedCopy")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

/// A sealed value as the Registry holds it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedRecord {
    /// The value's name.
    pub name: String,
    /// Raised by one at each write; the first write is 1.
    pub revision: u64,
    /// Each Station's copy, by Station ID.
    pub copies: BTreeMap<String, SealedCopy>,
    /// The Station or person that wrote this revision.
    pub written_by: String,
    /// When, as Unix seconds.
    pub written_at: f64,
}

/// A Station's public sealing key, as it published it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationSealingKey {
    /// The Station.
    pub station: String,
    /// Its public key, base64.
    pub public_key: String,
    /// Its identity's certificate (DER, base64), when its build has an identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<String>,
    /// The signature scheme of `signature`, as TLS numbers it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<u16>,
    /// The identity's signature over the Station and this key, base64, so that a person who holds
    /// the Station's fingerprint can tell its key from one written into the Registry by another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl StationSealingKey {
    /// The key `public_key` of `station`, with no signature.
    pub fn new(station: impl Into<String>, public_key: impl Into<String>) -> Self {
        Self {
            station: station.into(),
            public_key: public_key.into(),
            certificate: None,
            scheme: None,
            signature: None,
        }
    }

    /// Adds the identity's certificate and signature.
    #[must_use]
    pub fn signed(mut self, certificate: String, scheme: u16, signature: String) -> Self {
        self.certificate = Some(certificate);
        self.scheme = Some(scheme);
        self.signature = Some(signature);
        self
    }
}

/// Why a sealed value was not written.
#[non_exhaustive]
#[derive(Debug)]
pub enum SealedWriteError {
    /// The Registry holds another revision than the write named; `None` when it holds none.
    Conflict {
        /// What it holds.
        held: Option<u64>,
    },
    /// The Registry refused or failed the write.
    Io(io::Error),
}

impl fmt::Display for SealedWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict { held: Some(held) } => {
                write!(f, "the sealed value is at revision {held} now")
            }
            Self::Conflict { held: None } => f.write_str("the sealed value does not exist"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SealedWriteError {}

impl From<io::Error> for SealedWriteError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// What a write of a sealed value says about the revision it replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealedExpect {
    /// It replaces whatever is there.
    Any,
    /// There must be no value yet.
    Absent,
    /// It replaces this revision and no other.
    Revision(u64),
}

/// Sealed values and Stations' public sealing keys.
pub trait SealedStore {
    /// Records `key`, replacing what its Station published before.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name or when the Registry cannot be written.
    fn publish_sealing_key(&self, key: &StationSealingKey) -> io::Result<()>;

    /// The key `station` published.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn sealing_key(&self, station: &str) -> io::Result<Option<StationSealingKey>>;

    /// The sealed value `name`.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn sealed_value(&self, name: &str) -> io::Result<Option<SealedRecord>>;

    /// Every sealed value's revision, by name: what a Station polls.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be read.
    fn sealed_revisions(&self) -> io::Result<BTreeMap<String, u64>>;

    /// Writes `copies` as the next revision of `name` and returns that revision. `now` is Unix
    /// seconds.
    ///
    /// # Errors
    ///
    /// [`SealedWriteError::Conflict`] when `expect` does not hold, [`SealedWriteError::Io`] for an
    /// invalid name, a record past `[registry.limits.accounts] sealed_bytes`, or a failed write.
    fn store_sealed(
        &self,
        name: &str,
        copies: &BTreeMap<String, SealedCopy>,
        writer: &str,
        expect: SealedExpect,
        now: f64,
    ) -> Result<u64, SealedWriteError>;

    /// Removes the sealed value `name`; whether there was one.
    ///
    /// # Errors
    ///
    /// Returns an error when the Registry cannot be written.
    fn remove_sealed(&self, name: &str) -> io::Result<bool>;
}

/// Checks a value's or Station's name before a path or row is made from it.
pub(super) fn validate_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name.len() > 256 || name.contains(char::is_control) {
        return Err(invalid_data("invalid sealed value or Station name"));
    }
    Ok(())
}

/// `name` as one path component: what is not plain is `%XX`, so two names never share a file.
fn file_name(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len() + 5);
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded.push_str(".json");
    encoded
}

/// The next revision of a value now at `held`, when `expect` allows it.
pub(super) fn next_revision(
    held: Option<u64>,
    expect: SealedExpect,
) -> Result<u64, SealedWriteError> {
    let allowed = match expect {
        SealedExpect::Any => true,
        SealedExpect::Absent => held.is_none(),
        SealedExpect::Revision(revision) => held == Some(revision),
    };
    if allowed {
        Ok(held.unwrap_or(0) + 1)
    } else {
        Err(SealedWriteError::Conflict { held })
    }
}

/// A sealed value as stored, refused past the size limit.
pub(super) fn sealed_bytes(record: &SealedRecord, limits: &AccountLimits) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
    if !limits.sealed_bytes.allows(bytes.len()) {
        return Err(invalid_data("sealed value exceeds the size limit"));
    }
    Ok(bytes)
}

/// A stored sealed value, refused past the size limit.
pub(super) fn parse_sealed(bytes: &[u8], limits: &AccountLimits) -> io::Result<SealedRecord> {
    if !limits.sealed_bytes.allows(bytes.len()) {
        return Err(invalid_data("sealed value exceeds the size limit"));
    }
    serde_json::from_slice(bytes)
        .map_err(|_| invalid_data("a sealed value's record cannot be read"))
}

impl FileRegistry {
    fn sealed_dir(&self) -> PathBuf {
        self.root.join("sealed")
    }

    fn sealing_key_path(&self, station: &str) -> PathBuf {
        self.root.join("sealing-keys").join(file_name(station))
    }

    fn read_sealed(&self, name: &str) -> io::Result<Option<SealedRecord>> {
        match fs::read(self.sealed_dir().join(file_name(name))) {
            Ok(bytes) => parse_sealed(&bytes, &self.limits.accounts).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl SealedStore for FileRegistry {
    fn publish_sealing_key(&self, key: &StationSealingKey) -> io::Result<()> {
        validate_name(&key.station)?;
        let bytes = serde_json::to_vec(key).map_err(io::Error::other)?;
        let _lock = self.lock()?;
        self.write_atomic(&self.sealing_key_path(&key.station), &bytes)
    }

    fn sealing_key(&self, station: &str) -> io::Result<Option<StationSealingKey>> {
        validate_name(station)?;
        match fs::read(self.sealing_key_path(station)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| invalid_data("a sealing key's record cannot be read")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn sealed_value(&self, name: &str) -> io::Result<Option<SealedRecord>> {
        validate_name(name)?;
        self.read_sealed(name)
    }

    fn sealed_revisions(&self) -> io::Result<BTreeMap<String, u64>> {
        let entries = match fs::read_dir(self.sealed_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(error) => return Err(error),
        };
        let mut revisions = BTreeMap::new();
        for entry in entries {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                let record = parse_sealed(&fs::read(&path)?, &self.limits.accounts)?;
                revisions.insert(record.name, record.revision);
            }
        }
        Ok(revisions)
    }

    fn store_sealed(
        &self,
        name: &str,
        copies: &BTreeMap<String, SealedCopy>,
        writer: &str,
        expect: SealedExpect,
        now: f64,
    ) -> Result<u64, SealedWriteError> {
        validate_name(name)?;
        let _lock = self.lock()?;
        let held = self.read_sealed(name)?.map(|record| record.revision);
        let revision = next_revision(held, expect)?;
        let record = SealedRecord {
            name: name.to_owned(),
            revision,
            copies: copies.clone(),
            written_by: writer.to_owned(),
            written_at: now,
        };
        self.write_atomic(
            &self.sealed_dir().join(file_name(name)),
            &sealed_bytes(&record, &self.limits.accounts)?,
        )?;
        Ok(revision)
    }

    fn remove_sealed(&self, name: &str) -> io::Result<bool> {
        validate_name(name)?;
        let _lock = self.lock()?;
        match fs::remove_file(self.sealed_dir().join(file_name(name))) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    const NOW: f64 = 1_790_000_000.0;

    fn copies(tag: &str) -> BTreeMap<String, SealedCopy> {
        BTreeMap::from([(
            "station-a".to_owned(),
            SealedCopy {
                key: "00".to_owned(),
                sealed: tag.to_owned(),
            },
        )])
    }

    backend_tests!(
        an_older_write_never_replaces_a_newer_one,
        two_writers_of_one_revision_have_one_winner,
        names_that_differ_only_in_their_separators_are_separate_values,
        a_sealing_key_is_replaced_by_its_station_and_read_back,
        a_record_past_its_limit_is_refused,
    );

    fn an_older_write_never_replaces_a_newer_one(store: &dyn Store) {
        let registry = store.handle();
        let first = registry
            .store_sealed("login/p/a", &copies("one"), "a", SealedExpect::Absent, NOW)
            .unwrap();
        assert_eq!(first, 1);
        assert!(matches!(
            registry.store_sealed("login/p/a", &copies("x"), "a", SealedExpect::Absent, NOW),
            Err(SealedWriteError::Conflict { held: Some(1) })
        ));
        let second = registry
            .store_sealed(
                "login/p/a",
                &copies("two"),
                "b",
                SealedExpect::Revision(1),
                NOW + 1.0,
            )
            .unwrap();
        assert_eq!(second, 2);
        assert!(matches!(
            registry.store_sealed(
                "login/p/a",
                &copies("stale"),
                "a",
                SealedExpect::Revision(1),
                NOW + 2.0
            ),
            Err(SealedWriteError::Conflict { held: Some(2) })
        ));
        let held = registry.sealed_value("login/p/a").unwrap().unwrap();
        assert_eq!(held.revision, 2);
        assert_eq!(held.written_by, "b");
        assert_eq!(held.copies["station-a"].sealed, "two");
        assert_eq!(
            registry.sealed_revisions().unwrap(),
            BTreeMap::from([("login/p/a".to_owned(), 2)])
        );
        assert!(registry.remove_sealed("login/p/a").unwrap());
        assert!(!registry.remove_sealed("login/p/a").unwrap());
        assert!(registry.sealed_value("login/p/a").unwrap().is_none());
        assert!(matches!(
            registry.store_sealed(
                "login/p/a",
                &copies("x"),
                "a",
                SealedExpect::Revision(2),
                NOW
            ),
            Err(SealedWriteError::Conflict { held: None })
        ));
    }

    fn two_writers_of_one_revision_have_one_winner(store: &dyn Store) {
        let registry = store.handle();
        registry
            .store_sealed("v", &copies("base"), "a", SealedExpect::Absent, NOW)
            .unwrap();
        let barrier = Arc::new(Barrier::new(8));
        let writers: Vec<_> = (0..8)
            .map(|index| {
                let (registry, barrier) = (store.handle(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    registry
                        .store_sealed(
                            "v",
                            &copies(&format!("writer-{index}")),
                            "w",
                            SealedExpect::Revision(1),
                            NOW,
                        )
                        .is_ok()
                })
            })
            .collect();
        let won = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(won, 1);
        assert_eq!(registry.sealed_value("v").unwrap().unwrap().revision, 2);
    }

    fn names_that_differ_only_in_their_separators_are_separate_values(store: &dyn Store) {
        let registry = store.handle();
        for (name, tag) in [
            ("login/a-b/c", "one"),
            ("login/a/b-c", "two"),
            ("login/a.b/c", "three"),
        ] {
            registry
                .store_sealed(name, &copies(tag), "a", SealedExpect::Absent, NOW)
                .unwrap();
        }
        assert_eq!(registry.sealed_revisions().unwrap().len(), 3);
        assert_eq!(
            registry
                .sealed_value("login/a/b-c")
                .unwrap()
                .unwrap()
                .copies["station-a"]
                .sealed,
            "two"
        );
        assert!(registry.sealed_value("\nbad").is_err());
    }

    fn a_sealing_key_is_replaced_by_its_station_and_read_back(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(registry.sealing_key("station-a").unwrap(), None);
        registry
            .publish_sealing_key(&StationSealingKey::new("station-a", "aa"))
            .unwrap();
        let signed = StationSealingKey::new("station-a", "bb").signed(
            "cert".to_owned(),
            1027,
            "sig".to_owned(),
        );
        registry.publish_sealing_key(&signed).unwrap();
        assert_eq!(registry.sealing_key("station-a").unwrap(), Some(signed));
        assert_eq!(registry.sealing_key("station-b").unwrap(), None);
    }

    fn a_record_past_its_limit_is_refused(store: &dyn Store) {
        let registry = store.handle();
        let huge = "x".repeat(5 * 1024 * 1024);
        let refused = registry
            .store_sealed("v", &copies(&huge), "a", SealedExpect::Any, NOW)
            .unwrap_err();
        let SealedWriteError::Io(error) = refused else {
            panic!("{refused}");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(!error.to_string().contains(&huge[..16]));
    }

    #[test]
    fn debug_shows_who_a_copy_is_for_and_not_the_box() {
        let copy = SealedCopy {
            key: "aa".to_owned(),
            sealed: "ciphertext-0451".to_owned(),
        };
        assert!(!format!("{copy:?}").contains("ciphertext-0451"));
    }
}
