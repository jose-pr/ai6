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
use coqui_access_kit::{Keyed, Revision, Snapshot, SourceError, Table, Update, UpdateOne};
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

/// Why a stored record gave nothing.
enum Unread {
    /// This one record cannot be read or opened; the others may be fine.
    Record(SourceError),
    /// Nothing can be opened: the whole table is refused.
    Table(SourceError),
}

impl Unread {
    fn into_error(self) -> SourceError {
        match self {
            Self::Record(error) | Self::Table(error) => error,
        }
    }
}

impl<T: Walk + FromWalk + Sealable + Clone> RegistryTable<T> {
    /// The record `text` holds, with its secret opened. A record that cannot be is logged by
    /// name; the error it returns does not carry the name.
    fn decode(&self, text: &str) -> Result<T, Unread> {
        let mut record = from_deserializer::<T, _>(
            &mut serde_json::Deserializer::from_str(text),
            Presentation::new(),
        )
        .map_err(|_| Unread::Record(SourceError::invalid("a stored record cannot be read")))?;
        let context = self.context(record.name());
        match self.sealer.open(&context, record.secret_mut()) {
            Ok(opened) => {
                *record.secret_mut() = opened;
                Ok(record)
            }
            Err(SealError::NoKey) => Err(Unread::Table(sealing(SealError::NoKey))),
            Err(error) => {
                eprintln!(
                    "universal-exports: the one-time code record of {} cannot be opened: {error}",
                    record.name()
                );
                Err(Unread::Record(sealing(error)))
            }
        }
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

    /// Every record that can be read, and the keys of those that cannot: a whole-table read
    /// leaves a bad record out so only its own person is affected.
    fn read_all(
        &self,
        documents: &[(String, String)],
    ) -> Result<(Vec<T>, Vec<String>), SourceError> {
        let mut records = Vec::with_capacity(documents.len());
        let mut unreadable = Vec::new();
        for (key, text) in documents {
            match self.decode(text) {
                Ok(record) => records.push(record),
                Err(Unread::Record(_)) => unreadable.push(key.clone()),
                Err(Unread::Table(error)) => return Err(error),
            }
        }
        Ok((records, unreadable))
    }

    fn documents(&self) -> Result<Vec<(String, String)>, SourceError> {
        self.registry
            .factor_documents(self.family)
            .map_err(unavailable)
    }

    /// Writes `record` in place of `read` (what it was decided from); `false` when the
    /// document changed meanwhile and nothing was written.
    fn replace(&self, record: &T, read: Option<&str>) -> Result<bool, SourceError>
    where
        T: Keyed,
    {
        let text = self.encode(record)?;
        let key: Cow<'_, str> = record.key();
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
        Ok(!raced)
    }
}

impl<T> Table<T> for RegistryTable<T>
where
    T: Keyed + Clone + Send + Walk + FromWalk + Sealable,
{
    fn load(&self) -> Result<Snapshot<T>, SourceError> {
        let documents = self.documents()?;
        let mut digest = Sha256::new();
        for (key, text) in &documents {
            digest.update(key.as_bytes());
            digest.update([0]);
            digest.update(text.as_bytes());
            digest.update([0]);
        }
        let (records, _) = self.read_all(&documents)?;
        let head: [u8; 8] = digest.finalize()[..8]
            .try_into()
            .map_err(|_| SourceError::invalid("a revision cannot be made"))?;
        Ok(Snapshot::new(
            Revision::new(u64::from_be_bytes(head)),
            records,
        ))
    }

    /// Reads the one document under `key`: a record that cannot be read fails this call only.
    fn get(&self, key: &str) -> Result<Option<T>, SourceError> {
        self.registry
            .factor_get(self.family, key)
            .map_err(unavailable)?
            .map(|text| self.decode(&text).map_err(Unread::into_error))
            .transpose()
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
    /// A record that cannot be opened is never written over.
    fn update(&self, make: &mut Update<'_, T>) -> Result<(), SourceError> {
        loop {
            let documents = self.documents()?;
            let (records, unreadable) = self.read_all(&documents)?;
            let Some(record) = make(&records)? else {
                return Ok(());
            };
            if unreadable.iter().any(|key| *key == record.key()) {
                return Err(SourceError::invalid(
                    "a record that cannot be opened is not rewritten",
                ));
            }
            let read = documents
                .iter()
                .find(|(held, _)| held.as_str() == record.key().as_ref())
                .map(|(_, text)| text.as_str());
            if self.replace(&record, read)? {
                return Ok(());
            }
        }
    }

    /// Reads and writes the one document under `key`, as [`update`](Table::update) does.
    fn update_one(&self, key: &str, make: &mut UpdateOne<'_, T>) -> Result<(), SourceError> {
        loop {
            let text = self
                .registry
                .factor_get(self.family, key)
                .map_err(unavailable)?;
            let current = text
                .as_deref()
                .map(|text| self.decode(text).map_err(Unread::into_error))
                .transpose()?;
            let Some(record) = make(current.as_ref())? else {
                return Ok(());
            };
            if self.replace(&record, text.as_deref())? {
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
    Arc::new(RegistryTable::new(
        registry.clone(),
        ACCOUNTS,
        sealer.clone(),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use ai6::FileRegistry;

    fn record(name: &str) -> TotpRecord {
        let text = format!(
            r#"{{"name":"{name}","secret":"JBSWY3DPEHPK3PXP","algorithm":"sha1","digits":6,"period":30}}"#
        );
        from_deserializer(
            &mut serde_json::Deserializer::from_str(&text),
            Presentation::new(),
        )
        .unwrap()
    }

    fn table(dir: &std::path::Path) -> (Arc<dyn RegistryBackend>, RegistryTable<TotpRecord>) {
        let registry: Arc<dyn RegistryBackend> = Arc::new(FileRegistry::open(dir));
        let sealer =
            Arc::new(FactorSealer::new(Some("a sealing key of enough bytes....."), 8).unwrap());
        let table = RegistryTable::new(registry.clone(), ACCOUNTS, sealer);
        (registry, table)
    }

    #[test]
    fn a_read_and_an_update_touch_only_their_own_record() {
        let dir = tempfile::tempdir().unwrap();
        let (registry, table) = table(dir.path());
        table.put(record("alice")).unwrap();
        // Bob's document is garbage: any read of it fails, so a success never read it.
        registry
            .factor_update(ACCOUNTS, "bob", &mut |_| {
                Ok(FactorChange::Put("not a record".to_owned()))
            })
            .unwrap();

        assert_eq!(table.get("alice").unwrap().unwrap().name, "alice");
        assert!(table.get("carol").unwrap().is_none());
        assert!(table.get("bob").is_err());

        table
            .update_one("alice", &mut |current| {
                let mut next = current.unwrap().clone();
                next.failures = 2;
                Ok(Some(next))
            })
            .unwrap();
        assert_eq!(table.get("alice").unwrap().unwrap().failures, 2);
        assert!(table.update_one("bob", &mut |_| Ok(None)).is_err());
    }

    #[test]
    fn a_whole_table_read_leaves_a_bad_record_out_and_never_writes_over_it() {
        let dir = tempfile::tempdir().unwrap();
        let (registry, table) = table(dir.path());
        table.put(record("alice")).unwrap();
        registry
            .factor_update(ACCOUNTS, "bob", &mut |_| {
                Ok(FactorChange::Put("not a record".to_owned()))
            })
            .unwrap();

        let names: Vec<_> = table
            .load()
            .unwrap()
            .records
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(names, ["alice"]);
        assert!(table.update(&mut |_| Ok(Some(record("bob")))).is_err());
        assert_eq!(
            registry.factor_get(ACCOUNTS, "bob").unwrap().as_deref(),
            Some("not a record")
        );
    }
}
