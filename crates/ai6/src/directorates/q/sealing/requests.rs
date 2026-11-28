// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Secret writes asked of the Stations: a gateway holds no sealing key, so it cannot encrypt a
//! value under a scope key. It seals the request to the Stations that hold the key and leaves it
//! in the Registry; a Station that holds the key opens it, makes the change with its own
//! [`Vault`], and answers in plain, with nothing of the value in the answer.
//!
//! A request is `secret-request/<id>` (one sealed copy for each Station asked), the Station that
//! takes it first claims `secret-claim/<id>`, and the answer is `secret-result/<id>`, read by
//! [`status`]. A request is signed by a key a root certified for requests (M's, or the gateway's),
//! and a Station refuses one that is unsigned, expired, altered or signed by another key before it
//! acts, unless it was told to accept them unsigned.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::envelope::{Scope, ScopeKeyInfo};
use super::{Recipient, Signer, Vault, VaultError, base64_text, from_base64, published_key_of};
use crate::directorates::registry::backend::RegistryBackend;
use crate::directorates::registry::sealed::{
    SealedCopy, SealedExpect, SealedRecord, SealedWriteError,
};

const REQUEST_PREFIX: &str = "secret-request/";
const CLAIM_PREFIX: &str = "secret-claim/";
const RESULT_PREFIX: &str = "secret-result/";

/// What a person asks of the Stations about a secret. A value is in the request only until a
/// Station opens it.
#[derive(Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SecretRequest {
    /// Writes `value` as `name`, in `scope` (the scope it is in, else `global`). A scope with no
    /// key gets one, wrapped for `stations` (this Station when none), each pinned by `pins`.
    Put {
        /// The value's name.
        name: String,
        /// The scope, `global`, `operation:<id>` or `compartment:<id>`.
        scope: Option<String>,
        /// The value.
        value: String,
        /// The revision it replaces; any when none.
        expect: Option<u64>,
        /// The Stations a new scope key is wrapped for.
        #[serde(default)]
        stations: Vec<String>,
        /// The fingerprint each Station's sealing key must be signed by, by Station.
        #[serde(default)]
        pins: BTreeMap<String, String>,
    },
    /// Writes a new value of one that exists, in its own scope, under the scope's key.
    Rotate {
        /// The value's name.
        name: String,
        /// The new value.
        value: String,
        /// The revision it replaces; any when none.
        expect: Option<u64>,
    },
    /// Replaces the scope's key and encrypts every value of the scope again.
    RotateKey {
        /// The scope.
        scope: String,
    },
    /// Wraps the scope's key for one more Station.
    Grant {
        /// The scope.
        scope: String,
        /// The Station.
        station: String,
        /// The fingerprint its sealing key must be signed by.
        pin: Option<String>,
    },
    /// Takes a Station out of the scope: its wrap goes and the key is replaced.
    Revoke {
        /// The scope.
        scope: String,
        /// The Station.
        station: String,
    },
    /// Removes a value of `scope`.
    Remove {
        /// The value's name.
        name: String,
        /// The scope it must be in.
        scope: String,
    },
}

impl SecretRequest {
    /// Whether doing this writes a scope key record, which only a Station holding M's key can.
    fn changes_key_record(&self, vault: &Vault) -> bool {
        match self {
            Self::Put { name, scope, .. } => {
                let scope = match scope {
                    Some(text) => Scope::parse(text).ok(),
                    None => vault
                        .value_info(name)
                        .ok()
                        .flatten()
                        .map(|held| held.scope)
                        .or_else(|| Some(Scope::global())),
                };
                scope.is_none_or(|scope| !matches!(vault.scope_key_info(&scope), Ok(Some(_))))
            }
            Self::Rotate { .. } | Self::Remove { .. } => false,
            Self::RotateKey { .. } | Self::Grant { .. } | Self::Revoke { .. } => true,
        }
    }
}

impl Drop for SecretRequest {
    fn drop(&mut self) {
        match self {
            Self::Put { value, .. } | Self::Rotate { value, .. } => value.zeroize(),
            _ => {}
        }
    }
}

impl fmt::Debug for SecretRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = match self {
            Self::Put { .. } => "Put",
            Self::Rotate { .. } => "Rotate",
            Self::RotateKey { .. } => "RotateKey",
            Self::Grant { .. } => "Grant",
            Self::Revoke { .. } => "Revoke",
            Self::Remove { .. } => "Remove",
        };
        f.debug_struct("SecretRequest")
            .field("verb", &verb)
            .finish_non_exhaustive()
    }
}

/// What a Station did, in words with no value in them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretOutcome {
    /// The verb.
    pub verb: String,
    /// The value, for a verb about one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// The value's revision after a write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    /// The scope key's ID now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,
    /// The Stations the scope key is wrapped for now.
    #[serde(default)]
    pub stations: Vec<String>,
    /// What the person must still know.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Where a request stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestStatus {
    /// No Station has answered yet.
    Pending,
    /// A Station did it.
    Done(SecretOutcome),
    /// A Station refused or failed, with its words.
    Failed(String),
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Seals `request` to each of `stations`, each checked against its pin, signs it with `signer`
/// (valid for `lifetime` seconds from `now`; unsigned when none), and leaves it in the Registry as
/// `id` for the first of them that takes it. The gateway holds no sealing key and needs none.
///
/// # Errors
///
/// [`VaultError::Sealing`] for no Station or an `id` that is no name, [`VaultError::NoKey`] or
/// [`VaultError::Unverified`] for a Station, [`VaultError::Conflict`] when `id` is taken.
pub fn ask(
    registry: &dyn RegistryBackend,
    id: &str,
    request: &SecretRequest,
    stations: &[Recipient],
    signer: Option<(&Signer, f64)>,
    now: f64,
) -> Result<(), VaultError> {
    if stations.is_empty() || !valid_id(id) {
        return Err(VaultError::Sealing);
    }
    let plain = Zeroizing::new(serde_json::to_vec(request).map_err(|_| VaultError::Sealing)?);
    let mut copies = BTreeMap::new();
    for recipient in stations {
        let public = published_key_of(registry, recipient)?;
        copies.insert(
            recipient.station().to_owned(),
            SealedCopy {
                key: public.to_text(),
                sealed: base64_text(&public.seal(&plain)?),
            },
        );
    }
    if let Some((signer, lifetime)) = signer {
        let names: Vec<String> = copies.keys().cloned().collect();
        copies.extend(signer.sign_request(id, (now, now + lifetime), &plain, &names));
    }
    registry.store_sealed(
        &format!("{REQUEST_PREFIX}{id}"),
        &copies,
        "gateway",
        SealedExpect::Absent,
        now,
    )?;
    Ok(())
}

/// Where request `id` stands.
///
/// # Errors
///
/// [`VaultError::NotFound`] for no such request; the Registry's.
pub fn status(registry: &dyn RegistryBackend, id: &str) -> Result<RequestStatus, VaultError> {
    if !valid_id(id) {
        return Err(VaultError::NotFound);
    }
    if let Some(record) = registry.sealed_value(&format!("{RESULT_PREFIX}{id}"))? {
        let sealed_text = |label: &str| {
            record
                .copies
                .get(label)
                .and_then(|copy| from_base64(&copy.sealed))
        };
        if let Some(bytes) = sealed_text("ok") {
            return serde_json::from_slice(&bytes)
                .map(RequestStatus::Done)
                .map_err(|_| VaultError::Unreadable);
        }
        if let Some(bytes) = sealed_text("error") {
            return Ok(RequestStatus::Failed(
                String::from_utf8_lossy(&bytes).into_owned(),
            ));
        }
        return Err(VaultError::Unreadable);
    }
    registry
        .sealed_value(&format!("{REQUEST_PREFIX}{id}"))?
        .map(|_| RequestStatus::Pending)
        .ok_or(VaultError::NotFound)
}

fn plain_copy(label: &str, text: &[u8]) -> BTreeMap<String, SealedCopy> {
    BTreeMap::from([(
        label.to_owned(),
        SealedCopy {
            key: String::new(),
            sealed: base64_text(text),
        },
    )])
}

fn outcome(verb: &str, name: Option<&str>, info: &ScopeKeyInfo) -> SecretOutcome {
    SecretOutcome {
        verb: verb.to_owned(),
        name: name.map(str::to_owned),
        scope: Some(info.scope.to_string()),
        revision: None,
        key_id: Some(info.key_id.clone()),
        stations: info.stations.iter().cloned().collect(),
        notes: Vec::new(),
    }
}

impl Vault {
    /// Takes up each request addressed to this Station that no Station has taken, does it, and
    /// answers. Returns the IDs it answered. A request that fails is answered with the failure.
    ///
    /// # Errors
    ///
    /// The Registry's, when the requests cannot be listed.
    pub fn serve_requests(&self, now: f64) -> Result<Vec<String>, VaultError> {
        let mut answered = Vec::new();
        for name in self.registry.sealed_revisions()?.into_keys() {
            let Some(id) = name.strip_prefix(REQUEST_PREFIX) else {
                continue;
            };
            let Some(record) = self.registry.sealed_value(&name)? else {
                continue;
            };
            let Some(copy) = record.copies.get(&self.station) else {
                continue;
            };
            let claim = format!("{CLAIM_PREFIX}{id}");
            let result = format!("{RESULT_PREFIX}{id}");
            if self.registry.sealed_value(&result)?.is_some() {
                continue;
            }
            let opened = self.open_request(id, &record, copy, now);
            // A change to a scope key record needs M's key: leave it for a Station that holds it.
            if let Ok(request) = &opened
                && !self.can_sign()
                && request.changes_key_record(self)
            {
                continue;
            }
            match self.registry.store_sealed(
                &claim,
                &plain_copy(&self.station, b""),
                &self.station,
                SealedExpect::Absent,
                now,
            ) {
                Ok(_) => {}
                Err(SealedWriteError::Conflict { .. }) => continue,
                Err(error) => return Err(error.into()),
            }
            let answer = match opened.and_then(|request| self.run_request(&request, now)) {
                Ok(done) => (
                    "ok",
                    serde_json::to_vec(&done).map_err(|_| VaultError::Sealing)?,
                ),
                Err(error) => ("error", error.to_string().into_bytes()),
            };
            self.registry.store_sealed(
                &result,
                &plain_copy(answer.0, &answer.1),
                &self.station,
                SealedExpect::Absent,
                now,
            )?;
            self.registry.remove_sealed(&name)?;
            self.registry.remove_sealed(&claim)?;
            answered.push(id.to_owned());
        }
        Ok(answered)
    }

    /// Opens this Station's copy of request `id` and checks its signature before anything of it is
    /// read.
    fn open_request(
        &self,
        id: &str,
        record: &SealedRecord,
        copy: &SealedCopy,
        now: f64,
    ) -> Result<SecretRequest, VaultError> {
        let sealed = from_base64(&copy.sealed).ok_or(VaultError::Unreadable)?;
        let plain = self.key.open(&sealed).ok_or(VaultError::Unreadable)?;
        if !self.trust.accepts_unsigned() {
            let verified = self.trust.verify_request(
                &format!("{REQUEST_PREFIX}{id}"),
                id,
                &record.copies,
                &plain,
                now,
            )?;
            self.accept_certificate(&verified.certificate)?;
        }
        serde_json::from_slice(&plain).map_err(|_| VaultError::Unreadable)
    }

    fn run_request(&self, request: &SecretRequest, now: f64) -> Result<SecretOutcome, VaultError> {
        match request {
            SecretRequest::Put {
                name,
                scope,
                value,
                expect,
                stations,
                pins,
            } => {
                let scope = match scope {
                    Some(text) => Scope::parse(text)?,
                    None => self
                        .value_info(name)?
                        .map_or_else(Scope::global, |held| held.scope),
                };
                let info = match self.scope_key_info(&scope)? {
                    Some(info) => info,
                    None => {
                        let recipients: Vec<Recipient> = if stations.is_empty() {
                            vec![Recipient::named(self.station.clone())]
                        } else {
                            stations
                                .iter()
                                .map(|station| match pins.get(station) {
                                    Some(pin) => Recipient::named(station.clone()).pinned(pin),
                                    None => Recipient::named(station.clone()),
                                })
                                .collect()
                        };
                        self.put_scope_key(&scope, &recipients, now)?
                    }
                };
                let revision = self.write_value(
                    name,
                    &scope,
                    value,
                    expect.map_or(SealedExpect::Any, SealedExpect::Revision),
                    now,
                )?;
                let mut done = outcome("put", Some(name), &info);
                done.revision = Some(revision);
                Ok(done)
            }
            SecretRequest::Rotate {
                name,
                value,
                expect,
            } => {
                let held = self.value_info(name)?.ok_or(VaultError::NotFound)?;
                let info = self
                    .scope_key_info(&held.scope)?
                    .ok_or_else(|| VaultError::NoScopeKey(held.scope.to_string()))?;
                let revision = self.write_value(
                    name,
                    &held.scope,
                    value,
                    expect.map_or(SealedExpect::Any, SealedExpect::Revision),
                    now,
                )?;
                let mut done = outcome("rotate", Some(name), &info);
                done.revision = Some(revision);
                Ok(done)
            }
            SecretRequest::RotateKey { scope } => {
                let info = self.rotate_scope_key(&Scope::parse(scope)?, now)?;
                Ok(outcome("rotate-key", None, &info))
            }
            SecretRequest::Grant {
                scope,
                station,
                pin,
            } => {
                let mut recipient = Recipient::named(station.clone());
                if let Some(pin) = pin {
                    recipient = recipient.pinned(pin);
                }
                let info = self.grant_station(&Scope::parse(scope)?, &recipient, now)?;
                Ok(outcome("grant", None, &info))
            }
            SecretRequest::Revoke { scope, station } => {
                let info = self.revoke_station(&Scope::parse(scope)?, station, now)?;
                let mut done = outcome("revoke", None, &info);
                done.notes = vec![format!(
                    "{station} keeps every value of the scope it could open before: replace \
                     the secrets themselves at their source"
                )];
                Ok(done)
            }
            SecretRequest::Remove { name, scope } => {
                let scope = Scope::parse(scope)?;
                let removed = self.remove_value_in(name, &scope)?;
                let info = self
                    .scope_key_info(&scope)?
                    .ok_or_else(|| VaultError::NoScopeKey(scope.to_string()))?;
                let mut done = outcome("remove", Some(name), &info);
                if !removed {
                    done.notes = vec![format!("there was no value {name:?}")];
                }
                Ok(done)
            }
        }
    }
}
