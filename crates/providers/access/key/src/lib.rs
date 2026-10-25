//! Hashed bearer keys for AI6 unattended clients.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

ai6::guatiao::providers!(Keys);

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use ai6::{Attempt, Authenticator, Directory, Lookup, Person, Verdict};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_KEYS: usize = 10_000;
const MAX_FIELD_BYTES: usize = 1024;

/// Prefix identifying secrets created by this provider.
pub const PREFIX: &str = "ai6_";

/// Guatiao configuration for the key table.
#[derive(Debug, Clone, FromValue, Schema, ToValue)]
#[map(crate = ::ai6::guatiao)]
#[non_exhaustive]
pub struct KeysConfig {
    /// TOML file containing key hashes and account metadata.
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[serde(default)]
    keys: Vec<KeyRecord>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyRecord {
    id: String,
    name: String,
    hash: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    disabled: bool,
}

impl fmt::Debug for KeyRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyRecord")
            .field("id", &"[REDACTED]")
            .field("name", &"[REDACTED]")
            .field("hash", &"[REDACTED]")
            .field("email", &self.email.as_ref().map(|_| "[REDACTED]"))
            .field("groups", &self.groups.len())
            .field("disabled", &self.disabled)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    length: u64,
}

#[derive(Debug)]
struct Snapshot {
    document: Document,
    stamp: FileStamp,
}

/// Local key table as an AI6 Authenticator and Directory provider.
#[derive(Provider)]
#[provider(
    Authenticator,
    Directory,
    id = "ai6_auth_key",
    name = "Bearer keys",
    config = KeysConfig,
    crate = ::ai6::guatiao
)]
pub struct Keys {
    path: PathBuf,
    snapshot: Mutex<Snapshot>,
}

impl fmt::Debug for Keys {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self
            .snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .document
            .keys
            .len();
        formatter
            .debug_struct("Keys")
            .field("path", &self.path)
            .field("keys", &count)
            .field("secrets", &"[REDACTED]")
            .finish()
    }
}

impl Keys {
    /// Opens the key table; a missing file means no keys are configured.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is unreadable, malformed or out of bounds.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        let (document, stamp) = read_document(&path)?;
        Ok(Self {
            path,
            snapshot: Mutex::new(Snapshot { document, stamp }),
        })
    }

    fn current(&self) -> Result<std::sync::MutexGuard<'_, Snapshot>, ProviderError> {
        let mut snapshot = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        let stamp =
            file_stamp(&self.path).map_err(|_| provider_error("key table cannot be read"))?;
        if stamp != snapshot.stamp {
            let (document, stamp) = read_document(&self.path)
                .map_err(|_| provider_error("key table has invalid data"))?;
            *snapshot = Snapshot { document, stamp };
        }
        Ok(snapshot)
    }
}

impl TryFrom<KeysConfig> for Keys {
    type Error = ProviderError;

    fn try_from(config: KeysConfig) -> Result<Self, Self::Error> {
        if config.path.trim().is_empty() {
            return Err(provider_error("path must name the key table"));
        }
        Self::open(config.path).map_err(|_| provider_error("key table is invalid"))
    }
}

impl Authenticator for Keys {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let Some(presented) = attempt.secret.filter(|key| key.starts_with(PREFIX)) else {
            return Ok(Verdict::Unknown);
        };
        let digest = hash_of(&presented);
        let snapshot = self.current()?;
        let found = snapshot
            .document
            .keys
            .iter()
            .find(|record| bool::from(record.hash.as_bytes().ct_eq(digest.as_bytes())));
        Ok(match found {
            None => Verdict::Unknown,
            Some(record) if record.disabled => Verdict::Invalid,
            Some(record) => Verdict::Valid(Lookup {
                by: "id".to_owned(),
                value: record.id.clone(),
            }),
        })
    }
}

impl Directory for Keys {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        let snapshot = self.current()?;
        let record = snapshot.document.keys.iter().find(|record| {
            !record.disabled
                && match lookup.by.as_str() {
                    "id" => record.id == lookup.value,
                    "name" => record.name == lookup.value,
                    "email" => record.email.as_deref() == Some(&lookup.value),
                    _ => false,
                }
        });
        Ok(record.map(|record| Person {
            id: record.id.clone(),
            name: record.name.clone(),
            display_name: None,
            email: record.email.clone(),
            groups: record.groups.clone(),
            attributes: BTreeMap::new(),
        }))
    }
}

/// Makes a fresh 256-bit random bearer key with the `ai6_` prefix.
///
/// # Errors
///
/// Returns an error when the operating system random source is unavailable.
pub fn key() -> Result<String, String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut secret = [0; 32];
    fill(&mut secret).map_err(|_| "system random source is unavailable".to_owned())?;
    let mut encoded = String::with_capacity(PREFIX.len() + secret.len() * 2);
    encoded.push_str(PREFIX);
    for byte in secret {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(encoded)
}

/// Hashes a bearer key as lowercase SHA-256 hexadecimal for the table.
pub fn hash_of(key: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(key.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn read_document(path: &Path) -> Result<(Document, FileStamp), String> {
    let before = file_stamp(path)?;
    let bytes = match fs::read(path) {
        Ok(bytes) if bytes.len() <= MAX_FILE_BYTES => bytes,
        Ok(_) => return Err("key table exceeds the 1 MiB limit".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Document::default(), before));
        }
        Err(_) => return Err("key table cannot be read".to_owned()),
    };
    let source = std::str::from_utf8(&bytes).map_err(|_| "key table is not UTF-8".to_owned())?;
    let document: Document =
        toml::from_str(source).map_err(|_| "key table is invalid TOML".to_owned())?;
    validate_document(&document)?;
    let after = file_stamp(path)?;
    if before != after {
        return Err("key table changed while it was read".to_owned());
    }
    Ok((document, after))
}

fn validate_document(document: &Document) -> Result<(), String> {
    if document.keys.len() > MAX_KEYS {
        return Err("key table has too many entries".to_owned());
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut hashes = BTreeSet::new();
    for record in &document.keys {
        if !valid_field(&record.id, MAX_FIELD_BYTES)
            || !valid_field(&record.name, MAX_FIELD_BYTES)
            || record.hash.len() != 64
            || !record
                .hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || !ids.insert(&record.id)
            || !names.insert(&record.name)
            || !hashes.insert(&record.hash)
            || record.groups.len() > 256
            || record
                .groups
                .iter()
                .any(|group| !valid_field(group, MAX_FIELD_BYTES))
            || record
                .email
                .as_ref()
                .is_some_and(|email| !valid_field(email, 320))
        {
            return Err("key entry is invalid or duplicated".to_owned());
        }
    }
    Ok(())
}

fn valid_field(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn file_stamp(path: &Path) -> Result<FileStamp, String> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(FileStamp {
            modified: metadata.modified().ok(),
            length: metadata.len(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileStamp {
            modified: None,
            length: 0,
        }),
        Err(_) => Err("key table metadata is unavailable".to_owned()),
    }
}

fn provider_error(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(secret: &str) -> Attempt {
        Attempt {
            name: None,
            secret: Some(secret.to_owned()),
            headers: BTreeMap::new(),
            peer: None,
            secure: Some(true),
            certificate: None,
        }
    }

    #[test]
    fn generated_key_authenticates_and_resolves_without_storing_the_secret() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.toml");
        let secret = key().unwrap();
        let digest = hash_of(&secret);
        fs::write(
            &path,
            format!("[[keys]]\nid='robot-1'\nname='relay'\nhash='{digest}'\ngroups=['station']\n"),
        )
        .unwrap();
        let keys = Keys::open(&path).unwrap();
        let Verdict::Valid(lookup) = keys.authenticate(attempt(&secret)).unwrap() else {
            panic!("valid key was rejected");
        };
        let person = keys.lookup(lookup).unwrap().unwrap();
        assert_eq!(person.id, "robot-1");
        assert_eq!(person.groups, vec!["station"]);
        assert!(!format!("{keys:?}").contains(&secret));
        assert!(!fs::read_to_string(path).unwrap().contains(&secret));
    }

    #[test]
    fn unknown_and_disabled_keys_do_not_authenticate_or_resolve() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.toml");
        let secret = key().unwrap();
        let digest = hash_of(&secret);
        fs::write(
            &path,
            format!("[[keys]]\nid='robot-1'\nname='relay'\nhash='{digest}'\ndisabled=true\n"),
        )
        .unwrap();
        let keys = Keys::open(&path).unwrap();
        assert_eq!(
            keys.authenticate(attempt(&key().unwrap())).unwrap(),
            Verdict::Unknown
        );
        assert_eq!(
            keys.authenticate(attempt(&secret)).unwrap(),
            Verdict::Invalid
        );
        assert!(
            keys.lookup(Lookup {
                by: "id".to_owned(),
                value: "robot-1".to_owned()
            })
            .unwrap()
            .is_none()
        );
        assert_eq!(
            keys.authenticate(attempt("password")).unwrap(),
            Verdict::Unknown
        );
    }
}
