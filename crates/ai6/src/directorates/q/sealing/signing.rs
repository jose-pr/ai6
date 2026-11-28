// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Who may write the Registry's secret records: Ed25519 signatures that a Station checks against a
//! root it pinned.
//!
//! A root a person trusts signs a [`Certificate`] for M's key, and for a requester's key (the
//! gateway's). M signs every scope-key record; a requester signs every secret request. A Station
//! verifies the certificate against its pinned roots, then the signature, and refuses what is
//! unsigned, expired, signed by a key no root certified, or altered. A record's signature carries
//! the Headquarters election term and the Registry revision, so a Station that remembers the newest
//! pair (see [`Marks`](super::Marks)) refuses a rollback and a former Headquarters.
//!
//! The pinned roots live in [`TrustFile`], a file in the Station's own directory: a Registry writer
//! cannot reach it, so it cannot weaken it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use ring::signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::envelope::{hex, put, unhex};
use super::{VaultError, base64_text, from_base64};
use crate::directorates::registry::sealed::SealedCopy;

/// The file beside a Station's sealing key that holds its trust.
pub const TRUST_FILE: &str = "sealing.trust";

const CERTIFICATE_TAG: &[u8] = b"ai6-certificate-v1";
const RECORD_TAG: &[u8] = b"ai6-scope-key-record-v1";
const REQUEST_TAG: &[u8] = b"ai6-secret-request-v1";
/// The copy label that carries a signature, and the one that carries the signer's certificate.
pub(super) const SIGNATURE: &str = "s:sig";
pub(super) const CERTIFICATE: &str = "s:cert";
/// How long a request is valid when its asker names no other.
pub const DEFAULT_REQUEST_LIFETIME: f64 = 3600.0;

/// A public Ed25519 key, written `ed25519:` and 64 hexadecimal digits.
#[derive(Clone, PartialEq, Eq)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    /// The key `text` spells, or `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = unhex(text.strip_prefix("ed25519:")?)?;
        Some(Self(bytes.try_into().ok()?))
    }

    /// The key as text.
    pub fn to_text(&self) -> String {
        format!("ed25519:{}", hex(&self.0))
    }

    fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        UnparsedPublicKey::new(&ED25519, &self.0)
            .verify(message, signature)
            .is_ok()
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PublicKey").field(&self.to_text()).finish()
    }
}

/// A private Ed25519 key. `Debug` shows only the public half.
pub struct SigningKey {
    pair: Ed25519KeyPair,
    pkcs8: Zeroizing<Vec<u8>>,
}

impl SigningKey {
    /// A new key.
    pub fn generate() -> Self {
        let document = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .expect("the operating system's random generator failed");
        Self::from_pkcs8(document.as_ref()).expect("a key ring made is one ring reads")
    }

    /// The key in PKCS#8 `der`.
    pub fn from_pkcs8(der: &[u8]) -> Option<Self> {
        Some(Self {
            pair: Ed25519KeyPair::from_pkcs8(der).ok()?,
            pkcs8: Zeroizing::new(der.to_vec()),
        })
    }

    /// The key `text` spells: its PKCS#8 in base64.
    pub fn from_text(text: &str) -> Option<Self> {
        let der = Zeroizing::new(from_base64(text.trim())?);
        Self::from_pkcs8(&der)
    }

    /// The key as text, for a file or a secret provider.
    pub fn to_text(&self) -> Zeroizing<String> {
        Zeroizing::new(base64_text(&self.pkcs8))
    }

    /// The public half.
    pub fn public_key(&self) -> PublicKey {
        PublicKey(
            self.pair
                .public_key()
                .as_ref()
                .try_into()
                .expect("an Ed25519 public key is 32 bytes"),
        )
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.pair.sign(message).as_ref().to_vec()
    }
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SigningKey")
            .field(&self.public_key())
            .finish()
    }
}

/// What a certified key may sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Purpose {
    /// Scope-key records: M's key.
    ScopeKeyRecords,
    /// Secret requests: M's key, and the gateway's.
    SecretRequests,
}

impl Purpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::ScopeKeyRecords => "scope-key-records",
            Self::SecretRequests => "secret-requests",
        }
    }
}

/// A root's statement that `key` may sign for `purposes` between two times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Certificate {
    /// Whose key it is, in words (`m`, `gateway`).
    pub subject: String,
    /// The certified key.
    pub key: String,
    /// What it may sign.
    pub purposes: BTreeSet<Purpose>,
    /// Raised each time a root certifies a subject's next key; a Station refuses a lower one than
    /// it accepted, which retires the key the new one replaced before it expires.
    pub serial: u64,
    /// The first moment it holds, Unix seconds.
    pub not_before: f64,
    /// The first moment it no longer holds, Unix seconds.
    pub not_after: f64,
    /// The root that signed it.
    pub issuer: String,
    /// The root's signature, hex.
    pub signature: String,
}

impl Certificate {
    /// `key` certified by `root` for `purposes`.
    pub fn issue(
        root: &SigningKey,
        subject: &str,
        key: &PublicKey,
        purposes: BTreeSet<Purpose>,
        serial: u64,
        (not_before, not_after): (f64, f64),
    ) -> Self {
        let mut certificate = Self {
            subject: subject.to_owned(),
            key: key.to_text(),
            purposes,
            serial,
            not_before,
            not_after,
            issuer: root.public_key().to_text(),
            signature: String::new(),
        };
        certificate.signature = hex(&root.sign(&certificate.message()));
        certificate
    }

    fn message(&self) -> Vec<u8> {
        let mut out = CERTIFICATE_TAG.to_vec();
        put(&mut out, self.subject.as_bytes());
        put(&mut out, self.key.as_bytes());
        for purpose in &self.purposes {
            put(&mut out, purpose.as_str().as_bytes());
        }
        out.extend_from_slice(&self.serial.to_be_bytes());
        out.extend_from_slice(&self.not_before.to_bits().to_be_bytes());
        out.extend_from_slice(&self.not_after.to_bits().to_be_bytes());
        put(&mut out, self.issuer.as_bytes());
        out
    }

    /// The certificate as JSON text.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a certificate is JSON")
    }

    /// The certificate `text` holds, or `None`.
    pub fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }
}

/// The roots a Station trusts, and whether it accepts what no root vouches for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trust {
    roots: Vec<PublicKey>,
    unsigned: bool,
}

impl Trust {
    /// Trusts what a certificate from one of `roots` vouches for, and nothing else. With no root
    /// it refuses every signed record.
    pub fn strict(roots: Vec<PublicKey>) -> Self {
        Self {
            roots,
            unsigned: false,
        }
    }

    /// Accepts records and requests unsigned, and signs none. The weakening the operator chooses.
    pub fn unsigned() -> Self {
        Self {
            roots: Vec::new(),
            unsigned: true,
        }
    }

    /// Whether this Station accepts what is unsigned.
    pub fn accepts_unsigned(&self) -> bool {
        self.unsigned
    }

    /// What a Station reports at start when it trusts less than it should.
    pub fn weakening(&self) -> Option<String> {
        if self.unsigned {
            Some(
                "scope-key records and secret requests are accepted unsigned: a Station that can \
                 write the Registry can forge or roll one back"
                    .to_owned(),
            )
        } else if self.roots.is_empty() {
            Some(
                "this Station pins no root, so it refuses every scope-key record and secret \
                 request: add one to sealing.trust"
                    .to_owned(),
            )
        } else {
            None
        }
    }

    /// The key `certificate` certifies for `purpose` at `now`, or why it does not.
    fn certified(
        &self,
        certificate: &Certificate,
        purpose: Purpose,
        now: f64,
    ) -> Result<PublicKey, String> {
        let signature =
            unhex(&certificate.signature).ok_or("the certificate's signature is not hex")?;
        let message = certificate.message();
        let vouched = self
            .roots
            .iter()
            .any(|root| root.to_text() == certificate.issuer && root.verify(&message, &signature));
        if !vouched {
            return Err("its certificate is not signed by a root this Station pins".to_owned());
        }
        if !certificate.purposes.contains(&purpose) {
            return Err(format!(
                "its certificate does not allow {}",
                purpose.as_str()
            ));
        }
        if now < certificate.not_before {
            return Err("its certificate is not valid yet".to_owned());
        }
        if now >= certificate.not_after {
            return Err("its certificate has expired".to_owned());
        }
        PublicKey::parse(&certificate.key).ok_or_else(|| "its certified key is no key".to_owned())
    }
}

/// A Station's trust as it is kept in [`TRUST_FILE`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustFile {
    /// The roots, `ed25519:` and hex.
    #[serde(default)]
    pub roots: Vec<String>,
    /// Accepts unsigned records and requests, and signs none.
    #[serde(default)]
    pub unsigned: bool,
    /// A file holding M's key (PKCS#8 base64) and, beside it as `<file>.cert`, its certificate:
    /// what a vault or HSM agent writes. Without it a Station opens M's key from the Registry,
    /// where it is sealed to the Headquarters-eligible Stations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer_file: Option<String>,
}

impl TrustFile {
    /// The file at `path`; the strict default with no root when it does not exist.
    ///
    /// # Errors
    ///
    /// When the file cannot be read or holds no trust; the message names the path.
    pub fn open(path: &Path) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} holds no sealing trust", path.display()),
                )
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    /// Writes the file at `path`, replacing it.
    ///
    /// # Errors
    ///
    /// When it cannot be written.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(&temporary, path)
    }

    /// The [`Trust`] it says.
    ///
    /// # Errors
    ///
    /// When a root is no key; the message names it.
    pub fn trust(&self) -> io::Result<Trust> {
        if self.unsigned {
            return Ok(Trust::unsigned());
        }
        let roots = self
            .roots
            .iter()
            .map(|text| {
                PublicKey::parse(text).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("root {text:?} is no ed25519 key"),
                    )
                })
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Trust::strict(roots))
    }
}

/// A key with its certificate, and where the election term comes from.
#[derive(Clone)]
pub struct Signer {
    key: Arc<SigningKey>,
    certificate: Certificate,
    term: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
}

impl fmt::Debug for Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signer")
            .field("subject", &self.certificate.subject)
            .field("key", &self.key.public_key())
            .finish_non_exhaustive()
    }
}

impl Signer {
    /// `key` with its `certificate`, at term 0 (a requester has none).
    pub fn new(key: Arc<SigningKey>, certificate: Certificate) -> Self {
        Self {
            key,
            certificate,
            term: Arc::new(|| Some(0)),
        }
    }

    /// Takes the election term from `term` at each signing; `None` there is no live Headquarters
    /// and nothing is signed.
    #[must_use]
    pub fn with_term(mut self, term: impl Fn() -> Option<u64> + Send + Sync + 'static) -> Self {
        self.term = Arc::new(term);
        self
    }

    /// The term to sign in now.
    pub(super) fn term(&self) -> Result<u64, VaultError> {
        (self.term)().ok_or(VaultError::NoTerm)
    }

    /// The signature and certificate copies that make `copies` a signed record of `scope` at
    /// `revision` in `term`.
    pub(super) fn sign_record(
        &self,
        scope: &str,
        term: u64,
        revision: u64,
        copies: &BTreeMap<String, SealedCopy>,
    ) -> BTreeMap<String, SealedCopy> {
        let signature = self
            .key
            .sign(&record_message(scope, term, revision, copies));
        self.signature_copies(term.to_string(), &signature)
    }

    fn signature_copies(&self, note: String, signature: &[u8]) -> BTreeMap<String, SealedCopy> {
        BTreeMap::from([
            (
                SIGNATURE.to_owned(),
                SealedCopy {
                    key: note,
                    sealed: base64_text(signature),
                },
            ),
            (
                CERTIFICATE.to_owned(),
                SealedCopy {
                    key: String::new(),
                    sealed: base64_text(self.certificate.to_json().as_bytes()),
                },
            ),
        ])
    }

    /// The copies that sign a secret request `id` for `stations`, valid from `issued` to `expires`.
    pub(super) fn sign_request(
        &self,
        id: &str,
        (issued, expires): (f64, f64),
        plain: &[u8],
        stations: &[String],
    ) -> BTreeMap<String, SealedCopy> {
        let signature = self
            .key
            .sign(&request_message(id, (issued, expires), plain, stations));
        self.signature_copies(format!("{issued:?}/{expires:?}"), &signature)
    }
}

/// What a record or request is signed over: every copy but the signature's own, in label order.
fn copies_message(out: &mut Vec<u8>, copies: &BTreeMap<String, SealedCopy>) {
    let signed: Vec<_> = copies
        .iter()
        .filter(|(label, _)| !label.starts_with("s:"))
        .collect();
    out.extend_from_slice(&(signed.len() as u64).to_be_bytes());
    for (label, copy) in signed {
        put(out, label.as_bytes());
        put(out, copy.key.as_bytes());
        put(out, copy.sealed.as_bytes());
    }
}

fn record_message(
    scope: &str,
    term: u64,
    revision: u64,
    copies: &BTreeMap<String, SealedCopy>,
) -> Vec<u8> {
    let mut out = RECORD_TAG.to_vec();
    put(&mut out, scope.as_bytes());
    out.extend_from_slice(&term.to_be_bytes());
    out.extend_from_slice(&revision.to_be_bytes());
    copies_message(&mut out, copies);
    out
}

fn request_message(
    id: &str,
    (issued, expires): (f64, f64),
    plain: &[u8],
    stations: &[String],
) -> Vec<u8> {
    let mut out = REQUEST_TAG.to_vec();
    put(&mut out, id.as_bytes());
    out.extend_from_slice(&issued.to_bits().to_be_bytes());
    out.extend_from_slice(&expires.to_bits().to_be_bytes());
    out.extend_from_slice(&Sha256::digest(plain));
    let mut stations = stations.to_vec();
    stations.sort();
    for station in &stations {
        put(&mut out, station.as_bytes());
    }
    out
}

/// What a verified signature says of its signer.
pub(super) struct Verified {
    /// The election term the record was signed in.
    pub term: u64,
    /// The signer's certificate.
    pub certificate: Certificate,
}

/// The parts a signature carries, or `None` when there is none.
fn signature_parts(copies: &BTreeMap<String, SealedCopy>) -> Option<(&str, Vec<u8>, Certificate)> {
    let signature = copies.get(SIGNATURE)?;
    let certificate = copies.get(CERTIFICATE)?;
    let certificate =
        Certificate::from_json(&String::from_utf8(from_base64(&certificate.sealed)?).ok()?)?;
    Some((&signature.key, from_base64(&signature.sealed)?, certificate))
}

impl Trust {
    /// Checks that `copies` are `scope`'s key record at `revision`, signed by M.
    pub(super) fn verify_record(
        &self,
        label: &str,
        scope: &str,
        revision: u64,
        copies: &BTreeMap<String, SealedCopy>,
        now: f64,
    ) -> Result<Verified, VaultError> {
        let untrusted = |why: &str| VaultError::Untrusted(label.to_owned(), why.to_owned());
        let (term, signature, certificate) =
            signature_parts(copies).ok_or_else(|| VaultError::Unsigned(label.to_owned()))?;
        let key = self
            .certified(&certificate, Purpose::ScopeKeyRecords, now)
            .map_err(|why| untrusted(&why))?;
        let term: u64 = term
            .parse()
            .map_err(|_| untrusted("its signature names no term"))?;
        if !key.verify(&record_message(scope, term, revision, copies), &signature) {
            return Err(untrusted(
                "its signature does not cover this scope, revision and content",
            ));
        }
        Ok(Verified { term, certificate })
    }

    /// Checks that a request `id` for `stations`, whose opened content is `plain`, is signed by a
    /// key certified for requests and valid at `now`.
    pub(super) fn verify_request(
        &self,
        label: &str,
        id: &str,
        copies: &BTreeMap<String, SealedCopy>,
        plain: &[u8],
        now: f64,
    ) -> Result<Verified, VaultError> {
        let untrusted = |why: &str| VaultError::Untrusted(label.to_owned(), why.to_owned());
        let (window, signature, certificate) =
            signature_parts(copies).ok_or_else(|| VaultError::Unsigned(label.to_owned()))?;
        let key = self
            .certified(&certificate, Purpose::SecretRequests, now)
            .map_err(|why| untrusted(&why))?;
        let (issued, expires) = window
            .split_once('/')
            .and_then(|(issued, expires)| Some((issued.parse().ok()?, expires.parse().ok()?)))
            .ok_or_else(|| untrusted("its signature names no lifetime"))?;
        let stations: Vec<String> = copies
            .keys()
            .filter(|label| !label.starts_with("s:"))
            .cloned()
            .collect();
        if !key.verify(
            &request_message(id, (issued, expires), plain, &stations),
            &signature,
        ) {
            return Err(untrusted(
                "its signature does not cover this request and the Stations asked",
            ));
        }
        if now < issued {
            return Err(untrusted("it is dated in the future"));
        }
        if now >= expires {
            return Err(untrusted("it has expired"));
        }
        Ok(Verified {
            term: 0,
            certificate,
        })
    }
}

const M_KEY: &str = "secret-m-key";
const M_CERTIFICATE: &str = "secret-m-certificate";

impl Signer {
    /// The key in `path` (PKCS#8, base64) with its certificate in `<path>.cert`: what a vault or
    /// HSM agent writes for a Station that holds M's key outside the Registry.
    ///
    /// # Errors
    ///
    /// When either file cannot be read or holds no key or certificate; the message names the file.
    pub fn from_file(path: &Path) -> io::Result<Self> {
        let bad = |what: &Path| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} holds no signing key or certificate", what.display()),
            )
        };
        let text = Zeroizing::new(std::fs::read_to_string(path)?);
        let key = SigningKey::from_text(&text).ok_or_else(|| bad(path))?;
        let mut certificate_path = path.as_os_str().to_owned();
        certificate_path.push(".cert");
        let certificate_path = std::path::PathBuf::from(certificate_path);
        let certificate = Certificate::from_json(&std::fs::read_to_string(&certificate_path)?)
            .ok_or_else(|| bad(&certificate_path))?;
        Ok(Self::new(Arc::new(key), certificate))
    }
}

impl super::Vault {
    /// Seals M's key to the Headquarters-eligible Stations `eligible` in the Registry, with its
    /// certificate beside it. This is the fallback when no vault or HSM holds the key: every
    /// Station named can sign, so any of them leaking it is the weakening, and removing one means
    /// sealing it again to the rest under a key the root certifies next.
    ///
    /// # Errors
    ///
    /// Those of [`seal_to`](Self::seal_to).
    pub fn seal_m_key(
        &self,
        key: &SigningKey,
        certificate: &Certificate,
        eligible: &[super::Recipient],
        now: f64,
    ) -> Result<(), VaultError> {
        use crate::directorates::registry::sealed::SealedExpect;
        let expect = match self.registry.sealed_value(M_KEY)? {
            Some(held) => SealedExpect::Revision(held.revision),
            None => SealedExpect::Absent,
        };
        self.seal_to(M_KEY, &key.to_text(), eligible, expect, now)?;
        let copies = BTreeMap::from([(
            "cert".to_owned(),
            SealedCopy {
                key: String::new(),
                sealed: base64_text(certificate.to_json().as_bytes()),
            },
        )]);
        self.registry.store_sealed(
            M_CERTIFICATE,
            &copies,
            &self.station,
            SealedExpect::Any,
            now,
        )?;
        Ok(())
    }

    /// M's signer, when this Station is one of those M's key is sealed to; `None` when it is not or
    /// none is sealed. Its term comes from `term` at each signing.
    ///
    /// # Errors
    ///
    /// [`VaultError::Unreadable`] for a key or certificate that is no key or certificate, or a
    /// certificate for another key; the Registry's.
    pub fn open_m_signer(
        &self,
        term: impl Fn() -> Option<u64> + Send + Sync + 'static,
    ) -> Result<Option<Signer>, VaultError> {
        let opened = match self.open(M_KEY) {
            Ok(opened) => opened,
            Err(VaultError::NotFound | VaultError::NotNamed) => return Ok(None),
            Err(error) => return Err(error),
        };
        let key = SigningKey::from_text(&opened.value).ok_or(VaultError::Unreadable)?;
        let certificate = self
            .registry
            .sealed_value(M_CERTIFICATE)?
            .and_then(|record| record.copies.get("cert").cloned())
            .and_then(|copy| String::from_utf8(from_base64(&copy.sealed)?).ok())
            .and_then(|text| Certificate::from_json(&text))
            .filter(|certificate| certificate.key == key.public_key().to_text())
            .ok_or(VaultError::Unreadable)?;
        Ok(Some(
            Signer::new(Arc::new(key), certificate).with_term(term),
        ))
    }
}

#[cfg(test)]
mod tests;
