//! Authentication of transport-verified AI6 station peers.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

ai6::guatiao::providers!(Peers);

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use ai6::station::identity::StationFingerprint;
use ai6::{Attempt, Authenticator, Directory, Lookup, Person, Verdict};
use serde::{Deserialize, Serialize};

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_PEERS: usize = 10_000;
const MAX_FIELD_BYTES: usize = 1024;

/// Guatiao configuration for the verified peer table.
#[derive(Debug, Clone, FromValue, Schema, ToValue)]
#[map(crate = ::ai6::guatiao)]
#[non_exhaustive]
pub struct PeersConfig {
    /// TOML file mapping verified StationFingerprints to Directory people.
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[serde(default)]
    peers: Vec<PeerRecord>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerRecord {
    id: String,
    name: String,
    certificate: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    disabled: bool,
}

impl fmt::Debug for PeerRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerRecord")
            .field("id", &"[REDACTED]")
            .field("name", &"[REDACTED]")
            .field("certificate", &"[REDACTED]")
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

/// Mapping from an authenticated station certificate to an AI6 person.
#[derive(Provider)]
#[provider(
    Authenticator,
    Directory,
    id = "ai6_auth_peer",
    name = "Verified station peers",
    config = PeersConfig,
    crate = ::ai6::guatiao
)]
pub struct Peers {
    path: PathBuf,
    snapshot: Mutex<Snapshot>,
}

impl fmt::Debug for Peers {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self
            .snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .document
            .peers
            .len();
        formatter
            .debug_struct("Peers")
            .field("path", &self.path)
            .field("peers", &count)
            .field("identities", &"[REDACTED]")
            .finish()
    }
}

impl Peers {
    /// Opens the peer table; a missing file means no peers are configured.
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
            file_stamp(&self.path).map_err(|_| provider_error("peer table cannot be read"))?;
        if stamp != snapshot.stamp {
            let (document, stamp) = read_document(&self.path)
                .map_err(|_| provider_error("peer table has invalid data"))?;
            *snapshot = Snapshot { document, stamp };
        }
        Ok(snapshot)
    }
}

impl TryFrom<PeersConfig> for Peers {
    type Error = ProviderError;

    fn try_from(config: PeersConfig) -> Result<Self, Self::Error> {
        if config.path.trim().is_empty() {
            return Err(provider_error("path must name the peer table"));
        }
        Self::open(config.path).map_err(|_| provider_error("peer table is invalid"))
    }
}

impl Authenticator for Peers {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let Some(certificate) = attempt.certificate else {
            return Ok(Verdict::Unknown);
        };
        let Ok(certificate) = certificate.parse::<StationFingerprint>() else {
            return Ok(Verdict::Unknown);
        };
        let certificate = certificate.to_string();
        let snapshot = self.current()?;
        let peer = snapshot
            .document
            .peers
            .iter()
            .find(|peer| peer.certificate == certificate);
        Ok(match peer {
            None => Verdict::Unknown,
            Some(peer) if peer.disabled => Verdict::Invalid,
            Some(peer) => Verdict::Valid(Lookup {
                by: "id".to_owned(),
                value: peer.id.clone(),
            }),
        })
    }
}

impl Directory for Peers {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        let snapshot = self.current()?;
        let peer = snapshot.document.peers.iter().find(|peer| {
            !peer.disabled
                && match lookup.by.as_str() {
                    "id" => peer.id == lookup.value,
                    "name" => peer.name == lookup.value,
                    "certificate" => peer.certificate == lookup.value,
                    "email" => peer.email.as_deref() == Some(&lookup.value),
                    _ => false,
                }
        });
        Ok(peer.map(|peer| Person {
            id: peer.id.clone(),
            name: peer.name.clone(),
            display_name: None,
            email: peer.email.clone(),
            groups: peer.groups.clone(),
            attributes: BTreeMap::new(),
        }))
    }
}

fn read_document(path: &Path) -> Result<(Document, FileStamp), String> {
    let before = file_stamp(path)?;
    let bytes = match fs::read(path) {
        Ok(bytes) if bytes.len() <= MAX_FILE_BYTES => bytes,
        Ok(_) => return Err("peer table exceeds the 1 MiB limit".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Document::default(), before));
        }
        Err(_) => return Err("peer table cannot be read".to_owned()),
    };
    let source = std::str::from_utf8(&bytes).map_err(|_| "peer table is not UTF-8".to_owned())?;
    let document: Document =
        toml::from_str(source).map_err(|_| "peer table is invalid TOML".to_owned())?;
    validate_document(&document)?;
    let after = file_stamp(path)?;
    if before != after {
        return Err("peer table changed while it was read".to_owned());
    }
    Ok((document, after))
}

fn validate_document(document: &Document) -> Result<(), String> {
    if document.peers.len() > MAX_PEERS {
        return Err("peer table has too many entries".to_owned());
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut certificates = BTreeSet::new();
    for peer in &document.peers {
        if !valid_field(&peer.id, MAX_FIELD_BYTES)
            || !valid_field(&peer.name, MAX_FIELD_BYTES)
            || peer
                .certificate
                .parse::<StationFingerprint>()
                .is_ok_and(|id| id.to_string() != peer.certificate)
            || peer.certificate.parse::<StationFingerprint>().is_err()
            || !ids.insert(&peer.id)
            || !names.insert(&peer.name)
            || !certificates.insert(&peer.certificate)
            || peer.groups.len() > 256
            || peer
                .groups
                .iter()
                .any(|group| !valid_field(group, MAX_FIELD_BYTES))
            || peer
                .email
                .as_ref()
                .is_some_and(|email| !valid_field(email, 320))
        {
            return Err("peer entry is invalid or duplicated".to_owned());
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
        Err(_) => Err("peer table metadata is unavailable".to_owned()),
    }
}

fn provider_error(message: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn attempt(certificate: Option<&str>) -> Attempt {
        Attempt {
            name: Some("ignored".to_owned()),
            secret: Some("ignored".to_owned()),
            headers: BTreeMap::new(),
            peer: None,
            secure: Some(true),
            certificate: certificate.map(str::to_owned),
        }
    }

    #[test]
    fn authenticates_only_the_verified_office_certificate_and_resolves_groups() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("peers.toml");
        let identity = ai6::StationIdentity::generate().unwrap();
        let certificate = identity.id().to_string();
        fs::write(
            &path,
            format!("[[peers]]\nid='service-1'\nname='station-g'\ncertificate='{certificate}'\ngroups=['station-g']\n"),
        )
        .unwrap();
        let peers = Peers::open(path).unwrap();
        let Verdict::Valid(lookup) = peers.authenticate(attempt(Some(&certificate))).unwrap()
        else {
            panic!("known verified station certificate was rejected");
        };
        let person = peers.lookup(lookup).unwrap().unwrap();
        assert_eq!(person.id, "service-1");
        assert_eq!(person.groups, vec!["station-g"]);
        assert_eq!(peers.authenticate(attempt(None)).unwrap(), Verdict::Unknown);
        assert_eq!(
            peers
                .authenticate(attempt(Some("not-a-fingerprint")))
                .unwrap(),
            Verdict::Unknown
        );
        assert!(!format!("{peers:?}").contains(&certificate));
    }

    #[test]
    fn rejects_disabled_peers_and_malformed_certificates() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("peers.toml");
        let identity = ai6::StationIdentity::generate().unwrap();
        let certificate = identity.id().to_string();
        fs::write(
            &path,
            format!("[[peers]]\nid='service-1'\nname='station-g'\ncertificate='{certificate}'\ndisabled=true\n"),
        )
        .unwrap();
        let peers = Peers::open(&path).unwrap();
        assert_eq!(
            peers.authenticate(attempt(Some(&certificate))).unwrap(),
            Verdict::Invalid
        );
        assert!(
            peers
                .lookup(Lookup {
                    by: "id".to_owned(),
                    value: "service-1".to_owned()
                })
                .unwrap()
                .is_none()
        );
        fs::write(
            &path,
            "[[peers]]\nid='service-1'\nname='station-g'\ncertificate='bad'\n",
        )
        .unwrap();
        assert!(Peers::open(&path).is_err());
    }
}
