//! Local AI6 accounts backed by a bounded TOML file.
//!
//! Passwords are stored only as Argon2id hashes. The provider authenticates
//! accounts and resolves the same records through AI6's Directory contract.

#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

ai6::guatiao::providers!(LocalAccounts);

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use ai6::{Attempt, Authenticator, Directory, Lookup, Person, Verdict};
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use serde::{Deserialize, Serialize};

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_ACCOUNTS: usize = 10_000;
const MAX_PASSWORD_BYTES: usize = 4096;
const MAX_NAME_BYTES: usize = 256;

/// Guatiao configuration for the local account file.
#[derive(Debug, Clone, FromValue, Schema, ToValue)]
#[map(crate = ::ai6::guatiao)]
#[non_exhaustive]
pub struct LocalAccountsConfig {
    /// TOML file containing account IDs, names, password hashes and groups.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[serde(default)]
    accounts: Vec<AccountRecord>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountRecord {
    id: String,
    name: String,
    password_hash: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    disabled: bool,
}

impl fmt::Debug for AccountRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountRecord")
            .field("id", &"[REDACTED]")
            .field("name", &"[REDACTED]")
            .field("password_hash", &"[REDACTED]")
            .field(
                "display_name",
                &self.display_name.as_ref().map(|_| "[REDACTED]"),
            )
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

/// Local accounts as an AI6 Authenticator and Directory provider.
#[derive(Provider)]
#[provider(
    Authenticator,
    Directory,
    id = "ai6_auth_local",
    name = "Local accounts",
    config = LocalAccountsConfig,
    crate = ::ai6::guatiao
)]
pub struct LocalAccounts {
    path: PathBuf,
    snapshot: Mutex<Snapshot>,
    dummy_hash: String,
}

impl fmt::Debug for LocalAccounts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self
            .snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .document
            .accounts
            .len();
        formatter
            .debug_struct("LocalAccounts")
            .field("path", &self.path)
            .field("accounts", &count)
            .field("passwords", &"[REDACTED]")
            .finish()
    }
}

impl LocalAccounts {
    /// Opens the account file; a missing file means that no accounts are configured.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is unreadable, malformed or exceeds its bounds.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        let (document, stamp) = read_document(&path)?;
        let dummy_hash = hash_password("AI6 unknown account password")?;
        Ok(Self {
            path,
            snapshot: Mutex::new(Snapshot { document, stamp }),
            dummy_hash,
        })
    }

    fn current(&self) -> Result<std::sync::MutexGuard<'_, Snapshot>, ProviderError> {
        let mut snapshot = self.snapshot.lock().unwrap_or_else(PoisonError::into_inner);
        let stamp =
            file_stamp(&self.path).map_err(|_| provider_error("account file cannot be read"))?;
        if stamp != snapshot.stamp {
            let (document, stamp) = read_document(&self.path)
                .map_err(|_| provider_error("account file has invalid data"))?;
            *snapshot = Snapshot { document, stamp };
        }
        Ok(snapshot)
    }
}

impl TryFrom<LocalAccountsConfig> for LocalAccounts {
    type Error = ProviderError;

    fn try_from(config: LocalAccountsConfig) -> Result<Self, Self::Error> {
        if config.path.trim().is_empty() {
            return Err(provider_error("path must name the local account file"));
        }
        Self::open(&config.path).map_err(|_| provider_error("account file is invalid"))
    }
}

impl Authenticator for LocalAccounts {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let (Some(name), Some(password)) = (attempt.name, attempt.secret) else {
            return Ok(Verdict::Unknown);
        };
        let account = self
            .current()?
            .document
            .accounts
            .iter()
            .find(|account| account.name == name)
            .cloned();
        let hash = account
            .as_ref()
            .map_or(self.dummy_hash.as_str(), |account| {
                account.password_hash.as_str()
            });
        let valid =
            verify_password(&password, hash) && account.as_ref().is_some_and(|a| !a.disabled);
        Ok(if valid {
            Verdict::Valid(Lookup {
                by: "id".to_owned(),
                value: account.expect("valid accounts are present").id,
            })
        } else {
            Verdict::Invalid
        })
    }
}

impl Directory for LocalAccounts {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        let snapshot = self.current()?;
        let account = snapshot.document.accounts.iter().find(|account| {
            !account.disabled
                && match lookup.by.as_str() {
                    "id" => account.id == lookup.value,
                    "name" => account.name == lookup.value,
                    "email" => account.email.as_deref() == Some(&lookup.value),
                    _ => false,
                }
        });
        Ok(account.map(|account| Person {
            id: account.id.clone(),
            name: account.name.clone(),
            display_name: account.display_name.clone(),
            email: account.email.clone(),
            groups: account.groups.clone(),
            attributes: BTreeMap::new(),
        }))
    }
}

/// Hashes a password with a fresh random Argon2id salt for an account record.
///
/// # Errors
///
/// Returns an error when the password is empty, exceeds 4096 bytes, or hashing fails.
pub fn hash_password(password: &str) -> Result<String, String> {
    if password.is_empty() || password.len() > MAX_PASSWORD_BYTES {
        return Err("password must be between 1 and 4096 bytes".to_owned());
    }
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|_| "password hash could not be created".to_owned())
}

fn verify_password(password: &str, encoded: &str) -> bool {
    if password.len() > MAX_PASSWORD_BYTES {
        return false;
    }
    PasswordHash::new(encoded).ok().is_some_and(|hash| {
        Argon2::default()
            .verify_password(password.as_bytes(), &hash)
            .is_ok()
    })
}

fn read_document(path: &Path) -> Result<(Document, FileStamp), String> {
    let before = file_stamp(path)?;
    let bytes = match fs::read(path) {
        Ok(bytes) if bytes.len() <= MAX_FILE_BYTES => bytes,
        Ok(_) => return Err("account file exceeds the 1 MiB limit".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((
                Document {
                    accounts: Vec::new(),
                },
                before,
            ));
        }
        Err(_) => return Err("account file cannot be read".to_owned()),
    };
    let source = std::str::from_utf8(&bytes).map_err(|_| "account file is not UTF-8".to_owned())?;
    let document: Document =
        toml::from_str(source).map_err(|_| "account file is invalid TOML".to_owned())?;
    validate_document(&document)?;
    let after = file_stamp(path)?;
    if before != after {
        return Err("account file changed while it was read".to_owned());
    }
    Ok((document, after))
}

fn validate_document(document: &Document) -> Result<(), String> {
    if document.accounts.len() > MAX_ACCOUNTS {
        return Err("account file has too many accounts".to_owned());
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for account in &document.accounts {
        if !valid_field(&account.id, MAX_NAME_BYTES)
            || !valid_field(&account.name, MAX_NAME_BYTES)
            || account.name.trim().is_empty()
            || account.password_hash.len() > 1024
            || PasswordHash::new(&account.password_hash).is_err()
            || !ids.insert(&account.id)
            || !names.insert(&account.name)
            || account.groups.len() > 256
            || account
                .groups
                .iter()
                .any(|group| !valid_field(group, MAX_NAME_BYTES))
            || account
                .display_name
                .as_ref()
                .is_some_and(|value| !valid_field(value, 1024))
            || account
                .email
                .as_ref()
                .is_some_and(|value| !valid_field(value, 320))
        {
            return Err("account entry is invalid or duplicated".to_owned());
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
        Err(_) => Err("account file metadata is unavailable".to_owned()),
    }
}

fn provider_error(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn authenticates_and_resolves_accounts_without_exposing_password_hashes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts.toml");
        let password_hash = hash_password("correct horse battery staple").unwrap();
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "[[accounts]]\nid='person-007'\nname='007'\npassword_hash='{password_hash}'\ngroups=['field']").unwrap();
        let accounts = LocalAccounts::open(&path).unwrap();
        let verdict = accounts
            .authenticate(Attempt {
                name: Some("007".to_owned()),
                secret: Some("correct horse battery staple".to_owned()),
                headers: BTreeMap::new(),
                peer: None,
                secure: Some(true),
                certificate: None,
            })
            .unwrap();
        let Verdict::Valid(lookup) = verdict else {
            panic!("password should be valid");
        };
        assert_eq!(lookup.by, "id");
        let person = accounts.lookup(lookup).unwrap().unwrap();
        assert_eq!(person.id, "person-007");
        assert_eq!(person.groups, vec!["field".to_owned()]);
        assert!(!format!("{accounts:?}").contains(&password_hash));
    }

    #[test]
    fn wrong_unknown_and_disabled_credentials_do_not_authenticate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts.toml");
        let password_hash = hash_password("password").unwrap();
        fs::write(&path, format!("[[accounts]]\nid='person-a'\nname='a'\npassword_hash='{password_hash}'\ndisabled=true\n")).unwrap();
        let accounts = LocalAccounts::open(&path).unwrap();
        assert!(
            accounts
                .lookup(Lookup {
                    by: "id".to_owned(),
                    value: "person-a".to_owned(),
                })
                .unwrap()
                .is_none()
        );
        for name in ["a", "missing"] {
            let verdict = accounts
                .authenticate(Attempt {
                    name: Some(name.to_owned()),
                    secret: Some("password".to_owned()),
                    headers: BTreeMap::new(),
                    peer: None,
                    secure: None,
                    certificate: None,
                })
                .unwrap();
            assert_eq!(verdict, Verdict::Invalid);
        }
    }

    #[test]
    fn rejects_oversized_and_malformed_account_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts.toml");
        fs::write(&path, "accounts = [ { unknown = true } ]").unwrap();
        assert!(LocalAccounts::open(&path).is_err());
        fs::write(&path, vec![b' '; MAX_FILE_BYTES + 1]).unwrap();
        assert!(LocalAccounts::open(&path).is_err());
    }
}
