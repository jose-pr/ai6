//! The one-time code provider's tables, kept in the Registry so every gateway sharing it sees
//! the same accounts, enrolments, spent steps and recovery codes.
//!
//! The secret of a record is sealed ([`FactorSealer`]) before it is written and opened when it is
//! read; with no key a record is neither written nor read.
//!
//! A record is one document under its name; a change made from what was read is written only
//! if the document is still what was read, so two gateways never both accept one code.

use std::borrow::Cow;
use std::io;
use std::marker::PhantomData;
use std::sync::Arc;

use ai6::RegistryBackend;
use ai6::directorates::registry::factors::FactorChange;
use coqui_access_kit::{Keyed, Revision, Snapshot, SourceError, Table, Update};
use coqui_access_totp::{PendingTotp, TotpRecord};
use guatiao::{FromWalk, Walk};
use guatiao_serde::{Presentation, Walked, from_deserializer};
use sha2::{Digest, Sha256};

use crate::factor_seal::{FactorSealer, SealError};

/// The family of enrolled accounts.
pub(crate) const ACCOUNTS: &str = "totp";
/// The family of enrolments waiting for a first code.
pub(crate) const ENROLMENTS: &str = "totp_enrolling";

/// A record that keeps a secret under a name.
pub(crate) trait Sealable {
    fn name(&self) -> &str;
    fn secret_mut(&mut self) -> &mut String;
}

impl Sealable for TotpRecord {
    fn name(&self) -> &str {
        &self.name
    }
    fn secret_mut(&mut self) -> &mut String {
        &mut self.secret
    }
}

impl Sealable for PendingTotp {
    fn name(&self) -> &str {
        &self.name
    }
    fn secret_mut(&mut self) -> &mut String {
        &mut self.secret
    }
}

/// The records of one family of the Registry, as a [`Table`].
pub(crate) struct RegistryTable<T> {
    registry: Arc<dyn RegistryBackend>,
    family: &'static str,
    sealer: Arc<FactorSealer>,
    _records: PhantomData<fn() -> T>,
}

impl<T> RegistryTable<T> {
    pub(crate) fn new(
        registry: Arc<dyn RegistryBackend>,
        family: &'static str,
        sealer: Arc<FactorSealer>,
    ) -> Self {
        Self {
            registry,
            family,
            sealer,
            _records: PhantomData,
        }
    }

    /// What a secret is sealed to: the family and the account's name.
    fn context(&self, name: &str) -> String {
        format!("{}\0{name}", self.family)
    }
}

fn sealing(error: SealError) -> SourceError {
    SourceError::invalid(error.to_string())
}

/// What a Registry failure says to a provider: never the document.
fn unavailable(_: io::Error) -> SourceError {
    SourceError::unavailable("the Registry could not be read or written")
}

impl<T: Walk + FromWalk + Sealable + Clone> RegistryTable<T> {
    fn decode(&self, text: &str) -> Result<T, SourceError> {
        let mut record = from_deserializer::<T, _>(
            &mut serde_json::Deserializer::from_str(text),
            Presentation::new(),
        )
        .map_err(|_| SourceError::invalid("a stored record cannot be read"))?;
        let context = self.context(record.name());
        let opened = self
            .sealer
            .open(&context, record.secret_mut())
            .map_err(sealing)?;
        *record.secret_mut() = opened;
        Ok(record)
    }

    fn encode(&self, record: &T) -> Result<String, SourceError> {
        let mut sealed = record.clone();
        let context = self.context(sealed.name());
        let text = self
            .sealer
            .seal(&context, sealed.secret_mut())
            .map_err(sealing)?;
        *sealed.secret_mut() = text;
        serde_json::to_string(&Walked::new(&sealed, Presentation::new()))
            .map_err(|_| SourceError::invalid("a record cannot be encoded"))
    }

    fn documents(&self) -> Result<Vec<(String, String)>, SourceError> {
        self.registry
            .factor_documents(self.family)
            .map_err(unavailable)
    }
}

impl<T> Table<T> for RegistryTable<T>
where
    T: Keyed + Clone + Send + Walk + FromWalk + Sealable,
{
    fn load(&self) -> Result<Snapshot<T>, SourceError> {
        let documents = self.documents()?;
        let mut digest = Sha256::new();
        let mut records = Vec::with_capacity(documents.len());
        for (key, text) in &documents {
            digest.update(key.as_bytes());
            digest.update([0]);
            digest.update(text.as_bytes());
            digest.update([0]);
            records.push(self.decode(text)?);
        }
        let head: [u8; 8] = digest.finalize()[..8]
            .try_into()
            .map_err(|_| SourceError::invalid("a revision cannot be made"))?;
        Ok(Snapshot::new(
            Revision::new(u64::from_be_bytes(head)),
            records,
        ))
    }

    fn put(&self, record: T) -> Result<(), SourceError> {
        let text = self.encode(&record)?;
        let key = record.key().into_owned();
        self.registry
            .factor_update(self.family, &key, &mut |_| {
                Ok(FactorChange::Put(text.clone()))
            })
            .map_err(unavailable)
    }

    fn remove(&self, key: &str) -> Result<bool, SourceError> {
        let mut found = false;
        self.registry
            .factor_update(self.family, key, &mut |held| {
                found = held.is_some();
                Ok(FactorChange::Remove)
            })
            .map_err(unavailable)?;
        Ok(found)
    }

    /// Decides from the records read and writes the result only while the document it
    /// replaces is still what was read; otherwise it decides again from what is there now.
    fn update(&self, make: &mut Update<'_, T>) -> Result<(), SourceError> {
        loop {
            let documents = self.documents()?;
            let records = documents
                .iter()
                .map(|(_, text)| self.decode(text))
                .collect::<Result<Vec<_>, _>>()?;
            let Some(record) = make(&records)? else {
                return Ok(());
            };
            let text = self.encode(&record)?;
            let key: Cow<'_, str> = record.key();
            let read = documents
                .iter()
                .find(|(held, _)| held.as_str() == key.as_ref())
                .map(|(_, text)| text.as_str());
            let mut raced = false;
            self.registry
                .factor_update(self.family, &key, &mut |held| {
                    if held == read {
                        raced = false;
                        Ok(FactorChange::Put(text.clone()))
                    } else {
                        raced = true;
                        Ok(FactorChange::Keep)
                    }
                })
                .map_err(unavailable)?;
            if !raced {
                return Ok(());
            }
        }
    }
}

/// The accounts table over `registry`.
pub(crate) fn accounts(
    registry: &Arc<dyn RegistryBackend>,
    sealer: &Arc<FactorSealer>,
) -> Arc<dyn Table<TotpRecord>> {
    Arc::new(RegistryTable::new(registry.clone(), ACCOUNTS, sealer.clone()))
}

/// The enrolments table over `registry`.
pub(crate) fn enrolments(
    registry: &Arc<dyn RegistryBackend>,
    sealer: &Arc<FactorSealer>,
) -> Arc<dyn Table<PendingTotp>> {
    Arc::new(RegistryTable::new(
        registry.clone(),
        ENROLMENTS,
        sealer.clone(),
    ))
}
