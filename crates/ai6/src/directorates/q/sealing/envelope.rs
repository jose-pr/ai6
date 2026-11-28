// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Envelope encryption for secrets shared across Stations: a value is encrypted under a
//! symmetric scope key, and the scope key is wrapped once per Station that may use it.
//!
//! A scope is `global`, `operation:<id>` or `compartment:<id>`. One write of a value reaches every
//! Station holding a wrap of its scope's key. Granting a Station wraps the key for it; revoking
//! one rotates the key, re-encrypts every value of the scope under the new key and wraps it for
//! the Stations that remain. A Station revoked keeps only what it already opened.
//!
//! Both kinds of record live in the sealed store, so every Registry backend holds them:
//! `scope-key/<scope>` (copies `w:<station>` are the current wraps, `p:<station>` the previous
//! key's wraps while a rotation is unfinished, `i:<id>` and `j:<id>` the current and previous key
//! IDs) and `value/<name>` (one copy keyed by its scope: the key ID, and the nonce followed by
//! the ciphertext, base64).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use zeroize::Zeroizing;

use super::{
    Certificate, Recipient, SealingPublicKey, Vault, VaultError, base64_text, from_base64,
};
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::sealed::{SealedCopy, SealedExpect, SealedRecord};

const WRAP_TAG: &[u8] = b"ai6-scope-key-v1";
const VALUE_TAG: &[u8] = b"ai6-secret-v1";
const KEY_ID_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
const KEY_PREFIX: &str = "scope-key/";
const VALUE_PREFIX: &str = "value/";
/// How many times a re-encryption retries a value another writer replaced meanwhile.
const REENCRYPT_ATTEMPTS: usize = 8;
/// The term a value record carries: values are not signed, their key is what protects them.
const VALUE_TERM: u64 = 0;

/// Who a secret's key belongs to.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scope(String);

impl Scope {
    /// Station-level secrets.
    pub fn global() -> Self {
        Self("global".to_owned())
    }

    /// The Operation `id`'s secrets.
    ///
    /// # Errors
    ///
    /// [`VaultError::BadScope`] for an empty or control-laden ID.
    pub fn operation(id: &str) -> Result<Self, VaultError> {
        Self::parse(&format!("operation:{id}"))
    }

    /// Secrets shared across Operations by the compartment `id`.
    ///
    /// # Errors
    ///
    /// [`VaultError::BadScope`] for an empty or control-laden ID.
    pub fn compartment(id: &str) -> Result<Self, VaultError> {
        Self::parse(&format!("compartment:{id}"))
    }

    /// The scope `text` spells: `global`, `operation:<id>` or `compartment:<id>`.
    ///
    /// # Errors
    ///
    /// [`VaultError::BadScope`] for anything else.
    pub fn parse(text: &str) -> Result<Self, VaultError> {
        let bad = || VaultError::BadScope(text.chars().take(64).collect());
        let named = |id: &str| !id.is_empty() && id.len() <= 200 && !id.contains(char::is_control);
        match text.split_once(':') {
            None if text == "global" => Ok(Self::global()),
            Some(("operation" | "compartment", id)) if named(id) => Ok(Self(text.to_owned())),
            _ => Err(bad()),
        }
    }

    /// The scope as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The Operation or compartment ID, and which, or `None` for `global`.
    pub fn owner(&self) -> Option<(&str, &str)> {
        self.0.split_once(':')
    }
}

impl fmt::Debug for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Scope").field(&self.0).finish()
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a scope key record says about itself. Holds no key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeKeyInfo {
    /// The scope.
    pub scope: Scope,
    /// The record's revision: raised by a grant, a revoke and a rotation.
    pub revision: u64,
    /// The current key's ID, hex.
    pub key_id: String,
    /// The Stations holding a wrap of the current key.
    pub stations: BTreeSet<String>,
    /// Whether a rotation is unfinished: some values may still be under the previous key.
    pub rotating: bool,
}

/// A value this Station opened. `Debug` shows everything but the value.
pub struct OpenedValue {
    /// The value.
    pub value: Zeroizing<String>,
    /// The revision it was read at.
    pub revision: u64,
    /// The scope whose key encrypts it.
    pub scope: Scope,
    /// The ID of the key it was encrypted under, hex.
    pub key_id: String,
}

impl fmt::Debug for OpenedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenedValue")
            .field("revision", &self.revision)
            .field("scope", &self.scope)
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

/// A value's record, not its plaintext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueInfo {
    /// The value's name.
    pub name: String,
    /// The scope whose key encrypts it.
    pub scope: Scope,
    /// The ID of the key it is under, hex.
    pub key_id: String,
    /// Its revision.
    pub revision: u64,
}

/// An opened scope key. Wiped on drop; `Debug` shows only its ID.
struct ScopeKey {
    id: [u8; KEY_ID_LEN],
    key: Zeroizing<[u8; KEY_LEN]>,
}

impl fmt::Debug for ScopeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ScopeKey").field(&hex(&self.id)).finish()
    }
}

impl ScopeKey {
    fn generate() -> Self {
        let mut id = [0; KEY_ID_LEN];
        let mut key = Zeroizing::new([0; KEY_LEN]);
        // The OS's generator failing leaves nothing safe to do.
        SystemRandom::new()
            .fill(&mut id)
            .and_then(|()| SystemRandom::new().fill(key.as_mut()))
            .expect("the operating system's random generator failed");
        Self { id, key }
    }

    /// ring keeps its own copy of the key inside the cipher and offers no way to wipe it; the copy
    /// lives for one call, and this is a known limit of the library.
    fn cipher(&self) -> Result<LessSafeKey, VaultError> {
        UnboundKey::new(&CHACHA20_POLY1305, self.key.as_ref())
            .map(LessSafeKey::new)
            .map_err(|_| VaultError::Sealing)
    }

    fn wrap_plain(&self, scope: &Scope) -> Zeroizing<Vec<u8>> {
        let mut plain = Zeroizing::new(Vec::new());
        plain.extend_from_slice(WRAP_TAG);
        put(&mut plain, scope.as_str().as_bytes());
        plain.extend_from_slice(&self.id);
        plain.extend_from_slice(self.key.as_ref());
        plain
    }

    fn from_wrap(plain: &[u8], scope: &Scope) -> Option<Self> {
        let rest = plain.strip_prefix(WRAP_TAG)?;
        let (named, rest) = take(rest)?;
        if named != scope.as_str().as_bytes() || rest.len() != KEY_ID_LEN + KEY_LEN {
            return None;
        }
        let (id, key) = rest.split_at(KEY_ID_LEN);
        Some(Self {
            id: id.try_into().ok()?,
            key: Zeroizing::new(key.try_into().ok()?),
        })
    }
}

/// `bytes` with its length in front, so two fields never run together.
pub(super) fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn take(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let (len, rest) = bytes.split_first_chunk::<4>()?;
    let len = usize::try_from(u32::from_be_bytes(*len)).ok()?;
    (rest.len() >= len).then(|| rest.split_at(len))
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 == 1 || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok())
        .collect()
}

/// What a value's ciphertext is bound to.
fn aad(name: &str, scope: &Scope, key_id: &str, revision: u64) -> Vec<u8> {
    let mut out = VALUE_TAG.to_vec();
    put(&mut out, name.as_bytes());
    put(&mut out, scope.as_str().as_bytes());
    put(&mut out, key_id.as_bytes());
    out.extend_from_slice(&revision.to_be_bytes());
    out
}

fn encrypt(
    key: &ScopeKey,
    name: &str,
    scope: &Scope,
    revision: u64,
    plain: &[u8],
) -> Result<String, VaultError> {
    let mut nonce = [0; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| VaultError::Sealing)?;
    // Room for the tag, so the plaintext is never copied to a bigger buffer left unwiped.
    let mut sealed = Zeroizing::new(Vec::with_capacity(
        plain.len() + CHACHA20_POLY1305.tag_len(),
    ));
    sealed.extend_from_slice(plain);
    let buffer: &mut Vec<u8> = &mut sealed;
    key.cipher()?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad(name, scope, &hex(&key.id), revision)),
            buffer,
        )
        .map_err(|_| VaultError::Sealing)?;
    let mut joined = nonce.to_vec();
    joined.extend_from_slice(&sealed);
    Ok(base64_text(&joined))
}

fn decrypt(
    key: &ScopeKey,
    name: &str,
    stored: &StoredValue,
) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let joined = from_base64(&stored.sealed).ok_or(VaultError::Unreadable)?;
    let (nonce, sealed) = joined
        .split_first_chunk::<NONCE_LEN>()
        .ok_or(VaultError::Unreadable)?;
    let mut buffer = Zeroizing::new(sealed.to_vec());
    let opened = key
        .cipher()?
        .open_in_place(
            Nonce::assume_unique_for_key(*nonce),
            Aad::from(aad(name, &stored.scope, &stored.key_id, stored.revision)),
            &mut buffer,
        )
        .map_err(|_| VaultError::Unreadable)?
        .len();
    buffer.truncate(opened);
    Ok(buffer)
}

/// A value's record, parsed.
struct StoredValue {
    scope: Scope,
    key_id: String,
    revision: u64,
    sealed: String,
}

impl StoredValue {
    fn parse(record: &SealedRecord) -> Result<Self, VaultError> {
        let mut copies = record.copies.iter();
        let (Some((scope, copy)), None) = (copies.next(), copies.next()) else {
            return Err(VaultError::Unreadable);
        };
        Ok(Self {
            scope: Scope::parse(scope).map_err(|_| VaultError::Unreadable)?,
            key_id: copy.key.clone(),
            revision: record.revision,
            sealed: copy.sealed.clone(),
        })
    }
}

/// A scope key's record, parsed: wraps by Station and the key IDs.
struct StoredKey {
    revision: u64,
    id: String,
    previous_id: Option<String>,
    current: BTreeMap<String, SealedCopy>,
    previous: BTreeMap<String, SealedCopy>,
    /// The certificate fingerprint each pinned Station's sealing key must be signed by.
    pins: BTreeMap<String, String>,
}

impl StoredKey {
    fn parse(record: &SealedRecord) -> Result<Self, VaultError> {
        let mut parsed = Self {
            revision: record.revision,
            id: String::new(),
            previous_id: None,
            current: BTreeMap::new(),
            previous: BTreeMap::new(),
            pins: BTreeMap::new(),
        };
        for (label, copy) in &record.copies {
            match label.split_at_checked(2) {
                Some(("w:", station)) => {
                    parsed.current.insert(station.to_owned(), copy.clone());
                }
                Some(("p:", station)) => {
                    parsed.previous.insert(station.to_owned(), copy.clone());
                }
                Some(("f:", station)) => {
                    parsed.pins.insert(station.to_owned(), copy.key.clone());
                }
                Some(("i:", id)) => parsed.id = id.to_owned(),
                Some(("j:", id)) => parsed.previous_id = Some(id.to_owned()),
                Some(("s:", _)) => {}
                _ => return Err(VaultError::Unreadable),
            }
        }
        if parsed.id.is_empty() || parsed.current.is_empty() {
            return Err(VaultError::Unreadable);
        }
        Ok(parsed)
    }

    fn rotating(&self) -> bool {
        self.previous_id.is_some()
    }

    fn copies(&self) -> BTreeMap<String, SealedCopy> {
        let mut copies = BTreeMap::new();
        for (station, copy) in &self.current {
            copies.insert(format!("w:{station}"), copy.clone());
        }
        for (station, copy) in &self.previous {
            copies.insert(format!("p:{station}"), copy.clone());
        }
        for (station, fingerprint) in &self.pins {
            copies.insert(
                format!("f:{station}"),
                SealedCopy {
                    key: fingerprint.clone(),
                    sealed: String::new(),
                },
            );
        }
        copies.insert(format!("i:{}", self.id), marker());
        if let Some(id) = &self.previous_id {
            copies.insert(format!("j:{id}"), marker());
        }
        copies
    }

    fn info(&self, scope: &Scope) -> ScopeKeyInfo {
        ScopeKeyInfo {
            scope: scope.clone(),
            revision: self.revision,
            key_id: self.id.clone(),
            stations: self.current.keys().cloned().collect(),
            rotating: self.rotating(),
        }
    }
}

/// The empty copy that carries a key ID in a label.
fn marker() -> SealedCopy {
    SealedCopy {
        key: String::new(),
        sealed: String::new(),
    }
}

fn wrap(public: &str, key: &ScopeKey, scope: &Scope) -> Result<SealedCopy, VaultError> {
    let target = SealingPublicKey::parse(public).ok_or(VaultError::Sealing)?;
    Ok(SealedCopy {
        key: public.to_owned(),
        sealed: base64_text(&target.seal(&key.wrap_plain(scope))?),
    })
}

fn key_name(scope: &Scope) -> String {
    format!("{KEY_PREFIX}{scope}")
}

fn value_name(name: &str) -> String {
    format!("{VALUE_PREFIX}{name}")
}

impl Vault {
    fn load_key(&self, scope: &Scope) -> Result<StoredKey, VaultError> {
        let label = key_name(scope);
        let record = self
            .registry
            .sealed_value(&label)?
            .ok_or_else(|| VaultError::NoScopeKey(scope.to_string()))?;
        let term = self.verify_key_record(&label, scope, &record)?;
        let stored = StoredKey::parse(&record)?;
        self.marks
            .accept(&label, term, stored.revision, Some(&stored.id))?;
        Ok(stored)
    }

    /// The election term `record` was signed in, once M's signature on it holds; 0 for a Station
    /// that accepts records unsigned.
    fn verify_key_record(
        &self,
        label: &str,
        scope: &Scope,
        record: &SealedRecord,
    ) -> Result<u64, VaultError> {
        if self.trust.accepts_unsigned() {
            return Ok(0);
        }
        let verified = self.trust.verify_record(
            label,
            scope.as_str(),
            record.revision,
            &record.copies,
            self.trust_time(),
        )?;
        self.accept_certificate(&verified.certificate)?;
        Ok(verified.term)
    }

    /// Remembers the newest certificate of its subject: a lower serial is a key the root has since
    /// replaced, and its signatures are refused from then on.
    pub(super) fn accept_certificate(&self, certificate: &Certificate) -> Result<(), VaultError> {
        self.marks.accept(
            &format!("certificate/{}", certificate.subject),
            0,
            certificate.serial,
            Some(&certificate.key),
        )
    }

    /// Signs every scope key record again with this Station's signer, as it must after the root
    /// certifies M's next key. Every record is read before any is written, since a Station that
    /// has accepted the new certificate refuses the ones the old key signed.
    ///
    /// # Errors
    ///
    /// [`VaultError::NoSigner`], and the Registry's.
    pub fn resign_scope_keys(&self, now: f64) -> Result<usize, VaultError> {
        let mut held = Vec::new();
        for stored_name in self.registry.sealed_revisions()?.into_keys() {
            let Some(text) = stored_name.strip_prefix(KEY_PREFIX) else {
                continue;
            };
            let scope = Scope::parse(text).map_err(|_| VaultError::Unreadable)?;
            held.push((self.load_key(&scope)?, scope));
        }
        for (stored, scope) in &held {
            self.store_key(scope, stored, SealedExpect::Revision(stored.revision), now)?;
        }
        Ok(held.len())
    }

    fn open_wrap(&self, scope: &Scope, copy: &SealedCopy) -> Result<ScopeKey, VaultError> {
        // A wrap says whose key it was made for; the record cannot redirect a later rotation.
        if copy.key != self.key.public_key().to_text() {
            return Err(VaultError::Unreadable);
        }
        let sealed = from_base64(&copy.sealed).ok_or(VaultError::Unreadable)?;
        let plain = self.key.open(&sealed).ok_or(VaultError::Unreadable)?;
        ScopeKey::from_wrap(&plain, scope).ok_or(VaultError::Unreadable)
    }

    /// This Station's wrap of the current key.
    fn current_key(&self, scope: &Scope, stored: &StoredKey) -> Result<ScopeKey, VaultError> {
        let copy = stored
            .current
            .get(&self.station)
            .ok_or(VaultError::NotNamed)?;
        let key = self.open_wrap(scope, copy)?;
        if hex(&key.id) != stored.id {
            return Err(VaultError::Unreadable);
        }
        Ok(key)
    }

    fn previous_key(&self, scope: &Scope, stored: &StoredKey) -> Result<ScopeKey, VaultError> {
        let copy = stored
            .previous
            .get(&self.station)
            .ok_or(VaultError::NotNamed)?;
        let key = self.open_wrap(scope, copy)?;
        if Some(hex(&key.id)) != stored.previous_id {
            return Err(VaultError::Unreadable);
        }
        Ok(key)
    }

    fn store_key(
        &self,
        scope: &Scope,
        stored: &StoredKey,
        expect: SealedExpect,
        now: f64,
    ) -> Result<u64, VaultError> {
        let label = key_name(scope);
        let mut copies = stored.copies();
        let mut term = 0;
        if !self.trust.accepts_unsigned() {
            let signer = self.signer.as_ref().ok_or(VaultError::NoSigner)?;
            // Registry revisions go up by one, so the revision this write gets is known now.
            let revision = match expect {
                SealedExpect::Revision(held) => held + 1,
                _ => 1,
            };
            term = signer.term()?;
            // A signer in an older term than this Station accepted is refused before it writes.
            self.marks.check(&label, term, revision, Some(&stored.id))?;
            let signature = signer.sign_record(scope.as_str(), term, revision, &copies);
            copies.extend(signature);
        }
        let revision = self
            .registry
            .store_sealed(&label, &copies, &self.station, expect, now)?;
        self.marks
            .accept(&label, term, revision, Some(&stored.id))?;
        Ok(revision)
    }

    /// Makes the scope's key and wraps it for `stations`; the caller need not be one of them.
    ///
    /// # Errors
    ///
    /// [`VaultError::NoKey`] or [`VaultError::Unverified`] for a Station, [`VaultError::Sealing`]
    /// when none is named, [`VaultError::Conflict`] when the scope already has a key.
    pub fn put_scope_key(
        &self,
        scope: &Scope,
        stations: &[Recipient],
        now: f64,
    ) -> Result<ScopeKeyInfo, VaultError> {
        if stations.is_empty() {
            return Err(VaultError::Sealing);
        }
        let key = ScopeKey::generate();
        let mut current = BTreeMap::new();
        let mut pins = BTreeMap::new();
        for recipient in stations {
            let public = self.published_key(recipient)?.to_text();
            current.insert(recipient.station.clone(), wrap(&public, &key, scope)?);
            if let Some(fingerprint) = &recipient.fingerprint {
                pins.insert(recipient.station.clone(), fingerprint.clone());
            }
        }
        let stored = StoredKey {
            revision: 0,
            id: hex(&key.id),
            previous_id: None,
            current,
            previous: BTreeMap::new(),
            pins,
        };
        let revision = self.store_key(scope, &stored, SealedExpect::Absent, now)?;
        Ok(StoredKey { revision, ..stored }.info(scope))
    }

    /// What the Registry says of the scope's key, or `None` when it has none.
    ///
    /// # Errors
    ///
    /// The Registry's, or [`VaultError::Unreadable`] for a record that is not a scope key.
    pub fn scope_key_info(&self, scope: &Scope) -> Result<Option<ScopeKeyInfo>, VaultError> {
        match self.load_key(scope) {
            Ok(stored) => Ok(Some(stored.info(scope))),
            Err(VaultError::NoScopeKey(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Wraps the scope's key for one more Station. Only a Station holding it can. The key and
    /// every value stay as they are.
    ///
    /// # Errors
    ///
    /// [`VaultError::NotNamed`] when this Station holds no wrap; those of
    /// [`put_scope_key`](Self::put_scope_key).
    pub fn grant_station(
        &self,
        scope: &Scope,
        station: &Recipient,
        now: f64,
    ) -> Result<ScopeKeyInfo, VaultError> {
        self.finish_rotation(scope, now)?;
        let mut stored = self.load_key(scope)?;
        let key = self.current_key(scope, &stored)?;
        let public = self.published_key(station)?.to_text();
        stored
            .current
            .insert(station.station.clone(), wrap(&public, &key, scope)?);
        match &station.fingerprint {
            Some(fingerprint) => {
                stored
                    .pins
                    .insert(station.station.clone(), fingerprint.clone());
            }
            None => {
                stored.pins.remove(&station.station);
            }
        }
        let revision =
            self.store_key(scope, &stored, SealedExpect::Revision(stored.revision), now)?;
        stored.revision = revision;
        Ok(stored.info(scope))
    }

    /// Takes `station` out of the scope: its wrap is dropped, the key is replaced, every value of
    /// the scope is encrypted again under the new one, and the Stations that remain are wrapped
    /// for it. What `station` already opened stays with it. Only a Station holding the key can,
    /// and the last Station cannot be removed.
    ///
    /// # Errors
    ///
    /// [`VaultError::LastStation`], [`VaultError::NotNamed`] when this Station holds no wrap or
    /// `station` is not in the scope; the Registry's.
    pub fn revoke_station(
        &self,
        scope: &Scope,
        station: &str,
        now: f64,
    ) -> Result<ScopeKeyInfo, VaultError> {
        self.rotate(scope, Some(station), now)
    }

    /// Replaces the scope's key and encrypts every value of the scope again under it, keeping the
    /// Stations. Only a Station holding the key can.
    ///
    /// # Errors
    ///
    /// Those of [`revoke_station`](Self::revoke_station) but the last Station.
    pub fn rotate_scope_key(&self, scope: &Scope, now: f64) -> Result<ScopeKeyInfo, VaultError> {
        self.rotate(scope, None, now)
    }

    fn rotate(
        &self,
        scope: &Scope,
        remove: Option<&str>,
        now: f64,
    ) -> Result<ScopeKeyInfo, VaultError> {
        let (next, old, new) = self.begin_rotation(scope, remove, now)?;
        self.finish_with(scope, next, &old, &new, now)
    }

    /// Step one of a rotation: the new key is the current one and the previous key stays wrapped
    /// for the Stations that remain, so every value is readable whatever happens next.
    fn begin_rotation(
        &self,
        scope: &Scope,
        remove: Option<&str>,
        now: f64,
    ) -> Result<(StoredKey, ScopeKey, ScopeKey), VaultError> {
        self.finish_rotation(scope, now)?;
        let stored = self.load_key(scope)?;
        let old = self.current_key(scope, &stored)?;
        let mut remaining = stored.current.clone();
        if let Some(station) = remove
            && remaining.remove(station).is_none()
        {
            return Err(VaultError::NotNamed);
        }
        if remaining.is_empty() {
            return Err(VaultError::LastStation);
        }
        let new = ScopeKey::generate();
        let mut next = StoredKey {
            revision: stored.revision,
            id: hex(&new.id),
            previous_id: Some(stored.id.clone()),
            current: BTreeMap::new(),
            previous: BTreeMap::new(),
            pins: stored
                .pins
                .iter()
                .filter(|(station, _)| remaining.contains_key(*station))
                .map(|(station, pin)| (station.clone(), pin.clone()))
                .collect(),
        };
        // Wrapped to the key each Station publishes now, checked against its pin as at the grant,
        // never to the key the record names: a Station that changed its sealing key is wrapped to
        // the new one only once it verifies, and the record cannot redirect a rotation.
        for name in remaining.keys() {
            let mut recipient = Recipient::named(name.clone());
            if let Some(pin) = stored.pins.get(name) {
                recipient = recipient.pinned(pin.clone());
            }
            let public = self.published_key(&recipient)?.to_text();
            next.current
                .insert(name.clone(), wrap(&public, &new, scope)?);
            next.previous
                .insert(name.clone(), wrap(&public, &old, scope)?);
        }
        next.revision =
            self.store_key(scope, &next, SealedExpect::Revision(stored.revision), now)?;
        Ok((next, old, new))
    }

    /// Finishes a rotation a crash or a conflict left open: encrypts what is still under the
    /// previous key again and drops the previous wraps. Does nothing when none is open.
    ///
    /// # Errors
    ///
    /// [`VaultError::Rotating`] when one is open and this Station cannot finish it (it holds no
    /// wrap of the previous key), so nothing starts another over it; the Registry's.
    pub fn finish_rotation(&self, scope: &Scope, now: f64) -> Result<(), VaultError> {
        let stored = match self.load_key(scope) {
            Ok(stored) if stored.rotating() => stored,
            Ok(_) | Err(VaultError::NoScopeKey(_)) => return Ok(()),
            Err(error) => return Err(error),
        };
        let new = self.current_key(scope, &stored)?;
        let old = self
            .previous_key(scope, &stored)
            .map_err(|error| match error {
                VaultError::NotNamed | VaultError::Unreadable => {
                    VaultError::Rotating(scope.to_string())
                }
                other => other,
            })?;
        self.finish_with(scope, stored, &old, &new, now).map(drop)
    }

    fn finish_with(
        &self,
        scope: &Scope,
        mut stored: StoredKey,
        old: &ScopeKey,
        new: &ScopeKey,
        now: f64,
    ) -> Result<ScopeKeyInfo, VaultError> {
        self.reencrypt(scope, old, new, now)?;
        stored.previous.clear();
        stored.previous_id = None;
        stored.revision =
            self.store_key(scope, &stored, SealedExpect::Revision(stored.revision), now)?;
        Ok(stored.info(scope))
    }

    fn reencrypt(
        &self,
        scope: &Scope,
        old: &ScopeKey,
        new: &ScopeKey,
        now: f64,
    ) -> Result<(), VaultError> {
        let old_id = hex(&old.id);
        for stored_name in self.registry.sealed_revisions()?.into_keys() {
            let Some(name) = stored_name.strip_prefix(VALUE_PREFIX) else {
                continue;
            };
            for _ in 0..REENCRYPT_ATTEMPTS {
                let Some(record) = self.registry.sealed_value(&stored_name)? else {
                    break;
                };
                self.marks
                    .accept(&stored_name, VALUE_TERM, record.revision, None)?;
                if is_gone(&record) {
                    break;
                }
                let value = StoredValue::parse(&record)?;
                if &value.scope != scope || value.key_id != old_id {
                    break;
                }
                let plain = decrypt(old, name, &value)?;
                match self.store_encrypted(
                    new,
                    name,
                    scope,
                    &plain,
                    SealedExpect::Revision(record.revision),
                    now,
                ) {
                    Ok(revision) => {
                        self.marks
                            .accept(&stored_name, VALUE_TERM, revision, None)?;
                        break;
                    }
                    Err(VaultError::Conflict { .. }) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
    }

    /// Encrypts `plain` under `key` as the next revision `expect` allows, and stores it.
    fn store_encrypted(
        &self,
        key: &ScopeKey,
        name: &str,
        scope: &Scope,
        plain: &[u8],
        expect: SealedExpect,
        now: f64,
    ) -> Result<u64, VaultError> {
        let revision = match expect {
            SealedExpect::Revision(held) => held + 1,
            _ => 1,
        };
        let sealed = encrypt(key, name, scope, revision, plain)?;
        let copies = BTreeMap::from([(
            scope.to_string(),
            SealedCopy {
                key: hex(&key.id),
                sealed,
            },
        )]);
        Ok(self
            .registry
            .store_sealed(&value_name(name), &copies, &self.station, expect, now)?)
    }

    /// Leaves a tombstone at the value's name, so its revisions go on from here and an older record
    /// put back is refused.
    fn tombstone(&self, name: &str, revision: u64, now: f64) -> Result<(), VaultError> {
        let label = value_name(name);
        let next = self.registry.store_sealed(
            &label,
            &BTreeMap::new(),
            &self.station,
            SealedExpect::Revision(revision),
            now,
        )?;
        self.marks.accept(&label, VALUE_TERM, next, None)
    }

    /// Encrypts `value` under the scope's key and writes it as `name`, when the value is at
    /// `expect`. The caller must hold the scope's key; a value that exists stays in its scope.
    /// When a rotation moves the key while this writes, the value is encrypted again under the
    /// new key before this returns, never left under a key only a Station that was dropped holds.
    ///
    /// # Errors
    ///
    /// [`VaultError::NoScopeKey`], [`VaultError::NotNamed`], [`VaultError::WrongScope`] for a
    /// value in another scope, [`VaultError::Conflict`], [`VaultError::Rollback`],
    /// [`VaultError::Stale`] when rotations kept moving the key, or the Registry's.
    pub fn write_value(
        &self,
        name: &str,
        scope: &Scope,
        value: &str,
        expect: SealedExpect,
        now: f64,
    ) -> Result<u64, VaultError> {
        self.write_value_between(name, scope, value, expect, now, &|| {})
    }

    /// [`write_value`](Self::write_value), calling `between` once the key is open and before the
    /// value is stored: where a rotation by another Station may land.
    fn write_value_between(
        &self,
        name: &str,
        scope: &Scope,
        value: &str,
        expect: SealedExpect,
        now: f64,
        between: &dyn Fn(),
    ) -> Result<u64, VaultError> {
        let stored = self.load_key(scope)?;
        let mut key = self.current_key(scope, &stored)?;
        let label = value_name(name);
        let held = self.registry.sealed_value(&label)?;
        let (held_revision, live) = match &held {
            Some(record) => {
                self.marks
                    .accept(&label, VALUE_TERM, record.revision, None)?;
                let live = !is_gone(record);
                if live && StoredValue::parse(record)?.scope != *scope {
                    return Err(VaultError::WrongScope);
                }
                (Some(record.revision), live)
            }
            None => (None, false),
        };
        // A tombstone is the revision to go on from, and no value to conflict with.
        let from_held = || held_revision.map_or(SealedExpect::Absent, SealedExpect::Revision);
        let expect = match expect {
            SealedExpect::Any => from_held(),
            SealedExpect::Absent if live => {
                return Err(VaultError::Conflict {
                    held: held_revision,
                });
            }
            SealedExpect::Absent => from_held(),
            SealedExpect::Revision(_) if !live => return Err(VaultError::Conflict { held: None }),
            SealedExpect::Revision(revision) => SealedExpect::Revision(revision),
        };
        between();
        let mut revision =
            self.store_encrypted(&key, name, scope, value.as_bytes(), expect, now)?;
        self.marks.accept(&label, VALUE_TERM, revision, None)?;
        for _ in 0..REENCRYPT_ATTEMPTS {
            let after = self.load_key(scope)?;
            if after.id == hex(&key.id) {
                return Ok(revision);
            }
            // A rotation moved the key while this wrote.
            match self.current_key(scope, &after) {
                Ok(new) => key = new,
                Err(VaultError::NotNamed) => {
                    // This Station was dropped meanwhile: the value must not stay under a key
                    // only it holds.
                    drop(self.tombstone(name, revision, now));
                    return Err(VaultError::NotNamed);
                }
                Err(error) => return Err(error),
            }
            let Some(record) = self.registry.sealed_value(&label)? else {
                return Err(VaultError::Conflict { held: None });
            };
            if is_gone(&record) {
                return Err(VaultError::Conflict { held: None });
            }
            if StoredValue::parse(&record)?.key_id == after.id {
                // The rotation's own pass, or a newer write, already did.
                return Ok(record.revision);
            }
            match self.store_encrypted(
                &key,
                name,
                scope,
                value.as_bytes(),
                SealedExpect::Revision(record.revision),
                now,
            ) {
                Ok(next) => {
                    self.marks.accept(&label, VALUE_TERM, next, None)?;
                    revision = next;
                }
                Err(VaultError::Conflict { .. }) => {}
                Err(error) => return Err(error),
            }
        }
        Err(VaultError::Stale)
    }

    /// Opens the value `name`: this Station's wrap opens the scope key, and the key opens the
    /// value.
    ///
    /// # Errors
    ///
    /// [`VaultError::NotFound`], [`VaultError::NotNamed`] when this Station holds no wrap,
    /// [`VaultError::Stale`] when a rotation has not reached the value yet,
    /// [`VaultError::Rollback`] for a record older than one accepted,
    /// [`VaultError::Unreadable`] when anything fails to open, the Registry's.
    pub fn read_value(&self, name: &str) -> Result<OpenedValue, VaultError> {
        self.read_value_in(name, None)
    }

    /// [`read_value`](Self::read_value) of a value that must be in `expected`, when one is
    /// named: a value another scope holds under the name is [`VaultError::WrongScope`], and
    /// nothing of it is opened. A reader says which scope it means; the record does not.
    ///
    /// # Errors
    ///
    /// Those of [`read_value`](Self::read_value), and [`VaultError::WrongScope`].
    pub fn read_value_in(
        &self,
        name: &str,
        expected: Option<&Scope>,
    ) -> Result<OpenedValue, VaultError> {
        let label = value_name(name);
        let record = self
            .registry
            .sealed_value(&label)?
            .ok_or(VaultError::NotFound)?;
        self.marks
            .accept(&label, VALUE_TERM, record.revision, None)?;
        if is_gone(&record) {
            return Err(VaultError::NotFound);
        }
        let stored = StoredValue::parse(&record)?;
        if expected.is_some_and(|expected| *expected != stored.scope) {
            return Err(VaultError::WrongScope);
        }
        let keys = match self.load_key(&stored.scope) {
            Ok(keys) => keys,
            Err(VaultError::NoScopeKey(_)) => return Err(VaultError::Unreadable),
            Err(error) => return Err(error),
        };
        let key = if stored.key_id == keys.id {
            self.current_key(&stored.scope, &keys)?
        } else if keys.previous_id.as_deref() == Some(stored.key_id.as_str()) {
            self.previous_key(&stored.scope, &keys)?
        } else {
            // The value is under a key the record no longer names: a rotation is writing.
            self.current_key(&stored.scope, &keys)?;
            return Err(VaultError::Stale);
        };
        let plain = decrypt(&key, name, &stored)?;
        // Borrowed, so a plaintext that is not text is wiped with `plain` and not copied into an
        // error.
        let value = Zeroizing::new(
            std::str::from_utf8(&plain)
                .map_err(|_| VaultError::Unreadable)?
                .to_owned(),
        );
        Ok(OpenedValue {
            value,
            revision: stored.revision,
            scope: stored.scope,
            key_id: stored.key_id,
        })
    }

    /// What the Registry holds of the value `name`, without opening it.
    ///
    /// # Errors
    ///
    /// The Registry's, or [`VaultError::Unreadable`].
    pub fn value_info(&self, name: &str) -> Result<Option<ValueInfo>, VaultError> {
        self.registry
            .sealed_value(&value_name(name))?
            .filter(|record| !is_gone(record))
            .map(|record| {
                let stored = StoredValue::parse(&record)?;
                Ok(ValueInfo {
                    name: name.to_owned(),
                    scope: stored.scope,
                    key_id: stored.key_id,
                    revision: stored.revision,
                })
            })
            .transpose()
    }

    /// The values the Registry holds, by name, with their scope and key. Opens none.
    ///
    /// # Errors
    ///
    /// The Registry's, or [`VaultError::Unreadable`].
    pub fn value_infos(&self) -> Result<Vec<ValueInfo>, VaultError> {
        let mut infos = Vec::new();
        for stored_name in self.registry.sealed_revisions()?.into_keys() {
            if let Some(name) = stored_name.strip_prefix(VALUE_PREFIX)
                && let Some(info) = self.value_info(name)?
            {
                infos.push(info);
            }
        }
        Ok(infos)
    }

    /// Removes the value `name` of scope `expected`, leaving a tombstone so the name's revisions
    /// go on from here; whether there was a value. A value another scope holds is
    /// [`VaultError::WrongScope`] and stays.
    ///
    /// # Errors
    ///
    /// [`VaultError::WrongScope`], [`VaultError::Rollback`] for a record older than one
    /// accepted; the Registry's.
    pub fn remove_value_in(&self, name: &str, expected: &Scope) -> Result<bool, VaultError> {
        let label = value_name(name);
        let Some(record) = self.registry.sealed_value(&label)? else {
            return Ok(false);
        };
        self.marks
            .accept(&label, VALUE_TERM, record.revision, None)?;
        if is_gone(&record) {
            return Ok(false);
        }
        if StoredValue::parse(&record)?.scope != *expected {
            return Err(VaultError::WrongScope);
        }
        self.tombstone(name, record.revision, unix_now())?;
        Ok(true)
    }

    /// Moves each value an earlier build sealed to each Station separately (a sealed record that
    /// is no login and no record of this module's) under the `global` scope key, wrapped for the
    /// Stations it was sealed to, and removes the old record. A value this Station cannot open
    /// stays for a Station that can, as does one this Station cannot wrap the `global` key for.
    /// The old record's name is the new value's. Safe to run again.
    ///
    /// # Errors
    ///
    /// The Registry's, or a Station's published key being unusable; nothing is removed until its
    /// new value is stored.
    pub fn migrate_legacy(&self, now: f64) -> Result<Vec<String>, VaultError> {
        let mut moved = Vec::new();
        for name in self.registry.sealed_revisions()?.into_keys() {
            let reserved = ["login/", KEY_PREFIX, VALUE_PREFIX, "secret-"];
            if reserved.iter().any(|prefix| name.starts_with(prefix)) {
                continue;
            }
            let opened = match self.open(&name) {
                Ok(opened) => opened,
                Err(VaultError::NotNamed | VaultError::Unreadable | VaultError::NotFound) => {
                    continue;
                }
                Err(error) => return Err(error),
            };
            let global = Scope::global();
            match self.scope_key_info(&global)? {
                None => {
                    let stations: Vec<Recipient> = opened
                        .stations
                        .iter()
                        .map(|station| Recipient::named(station.clone()))
                        .collect();
                    self.put_scope_key(&global, &stations, now)?;
                }
                // A Station outside the global scope cannot wrap its key: the value stays as it was.
                Some(info) if !info.stations.contains(&self.station) => continue,
                Some(info) => {
                    for station in opened.stations.difference(&info.stations) {
                        self.grant_station(&global, &Recipient::named(station.clone()), now)?;
                    }
                }
            }
            match self.write_value(&name, &global, &opened.value, SealedExpect::Absent, now) {
                Ok(_) | Err(VaultError::Conflict { .. }) => {}
                Err(error) => return Err(error),
            }
            self.registry.remove_sealed(&name)?;
            moved.push(name);
        }
        Ok(moved)
    }
}

/// The values `registry` holds, by name, with their scope and key. Opens none.
///
/// # Errors
///
/// The Registry's, or [`VaultError::Unreadable`].
pub fn list_values(registry: &dyn RegistryBackend) -> Result<Vec<ValueInfo>, VaultError> {
    let mut infos = Vec::new();
    for stored_name in registry.sealed_revisions()?.into_keys() {
        let Some(name) = stored_name.strip_prefix(VALUE_PREFIX) else {
            continue;
        };
        let Some(record) = registry.sealed_value(&stored_name)? else {
            continue;
        };
        if is_gone(&record) {
            continue;
        }
        let stored = StoredValue::parse(&record)?;
        infos.push(ValueInfo {
            name: name.to_owned(),
            scope: stored.scope,
            key_id: stored.key_id,
            revision: stored.revision,
        });
    }
    Ok(infos)
}

/// The scope keys `registry` holds, with who they are wrapped for. Opens none.
///
/// # Errors
///
/// The Registry's, or [`VaultError::Unreadable`].
pub fn list_scopes(registry: &dyn RegistryBackend) -> Result<Vec<ScopeKeyInfo>, VaultError> {
    let mut infos = Vec::new();
    for stored_name in registry.sealed_revisions()?.into_keys() {
        let Some(text) = stored_name.strip_prefix(KEY_PREFIX) else {
            continue;
        };
        let Some(record) = registry.sealed_value(&stored_name)? else {
            continue;
        };
        let scope = Scope::parse(text).map_err(|_| VaultError::Unreadable)?;
        infos.push(StoredKey::parse(&record)?.info(&scope));
    }
    Ok(infos)
}

/// Now, as Unix seconds.
pub(super) fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// Whether a value's record is the tombstone a removal left.
fn is_gone(record: &SealedRecord) -> bool {
    record.copies.is_empty()
}

#[cfg(test)]
mod tests;
