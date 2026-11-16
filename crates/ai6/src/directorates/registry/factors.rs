//! Documents of the sign-in factors: pending sign-ins and the one-time code accounts and
//! enrolments, shared by every gateway on one Registry.
//!
//! A document is text a family's owner writes and reads back; the Registry keeps it under a
//! family and a key and runs each change under the same lock (file Registry) or transaction (SQL
//! Registry) that serializes every other write, so a change made from what was read is never
//! made from something already replaced, whichever gateway made it.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::store::{FileRegistry, hex_digest, invalid_data, validate_registry_key};

/// What a change made to a document comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FactorChange {
    /// Leave the document as it is.
    Keep,
    /// Write this text under the key.
    Put(String),
    /// Remove the document.
    Remove,
}

/// A change to one document: sees the text held under the key, if any, and says what to do.
pub type FactorEdit<'a> = dyn FnMut(Option<&str>) -> io::Result<FactorChange> + 'a;

/// Whether to keep a document: sees its key and text.
pub type FactorKeep<'a> = dyn FnMut(&str, &str) -> bool + 'a;

/// Documents the sign-in factors keep in the Registry.
pub trait FactorStore {
    /// Every document of `family` as its key and text.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a family that is not letters, digits, `.`, `_` or `-`; the store's
    /// own errors.
    fn factor_documents(&self, family: &str) -> io::Result<Vec<(String, String)>>;

    /// Changes the document under `key` in `family`, from what is held there now, as one step.
    /// `edit` may run more than once where the store retries; it must not keep state between
    /// runs.
    ///
    /// # Errors
    ///
    /// What `edit` returns, `InvalidData` for a document past the size limit, and the
    /// store's own errors.
    fn factor_update(
        &self,
        family: &str,
        key: &str,
        edit: &mut FactorEdit<'_>,
    ) -> io::Result<()>;

    /// Removes every document of `family` that `keep` refuses, and answers how many went.
    ///
    /// # Errors
    ///
    /// As [`factor_documents`](Self::factor_documents).
    fn factor_retain(&self, family: &str, keep: &mut FactorKeep<'_>) -> io::Result<usize>;
}

/// The most bytes in a document's key.
pub(super) const KEY_BYTES: usize = 512;

/// Whether `key` may name a document: not empty, no control characters, within [`KEY_BYTES`].
pub(super) fn validate_factor_key(key: &str) -> io::Result<()> {
    if key.is_empty() || key.len() > KEY_BYTES || key.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a factor document key must be 1 to 512 bytes with no control characters",
        ));
    }
    Ok(())
}

/// A document as the file Registry writes it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    key: String,
    text: String,
}

impl FileRegistry {
    fn factors_root(&self, family: &str) -> PathBuf {
        self.root().join("factors").join(family)
    }

    fn factor_path(&self, family: &str, key: &str) -> PathBuf {
        let digest = hex_digest(&Sha256::digest(key.as_bytes()));
        self.factors_root(family).join(format!("{digest}.json"))
    }

    fn read_factor(&self, path: &std::path::Path) -> io::Result<Option<Stored>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        // A file that is not a document this Registry wrote is not one.
        Ok(serde_json::from_slice::<Stored>(&bytes).ok())
    }
}

impl FactorStore for FileRegistry {
    fn factor_documents(&self, family: &str) -> io::Result<Vec<(String, String)>> {
        validate_registry_key(family)?;
        let _lock = self.lock()?;
        let entries = match fs::read_dir(self.factors_root(family)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut found = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if let Some(stored) = self.read_factor(&path)?
                && self.factor_path(family, &stored.key) == path
            {
                found.push((stored.key, stored.text));
            }
        }
        found.sort();
        Ok(found)
    }

    fn factor_update(
        &self,
        family: &str,
        key: &str,
        edit: &mut FactorEdit<'_>,
    ) -> io::Result<()> {
        validate_registry_key(family)?;
        validate_factor_key(key)?;
        let _lock = self.lock()?;
        let path = self.factor_path(family, key);
        let held = self.read_factor(&path)?.filter(|stored| stored.key == key);
        match edit(held.as_ref().map(|stored| stored.text.as_str()))? {
            FactorChange::Keep => Ok(()),
            FactorChange::Put(text) => {
                let bytes = serde_json::to_vec(&Stored {
                    key: key.to_owned(),
                    text,
                })
                .map_err(io::Error::other)?;
                if !self
                    .current_limits()
                    .tokens
                    .record_bytes
                    .allows(bytes.len())
                {
                    return Err(invalid_data("a factor document exceeds the size limit"));
                }
                self.write_atomic(&path, &bytes)
            }
            FactorChange::Remove => match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        }
    }

    fn factor_retain(&self, family: &str, keep: &mut FactorKeep<'_>) -> io::Result<usize> {
        validate_registry_key(family)?;
        let _lock = self.lock()?;
        let entries = match fs::read_dir(self.factors_root(family)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        let mut removed = 0;
        for entry in entries {
            let path = entry?.path();
            let Some(stored) = self.read_factor(&path)? else {
                continue;
            };
            if self.factor_path(family, &stored.key) == path && !keep(&stored.key, &stored.text) {
                match fs::remove_file(&path) {
                    Ok(()) => removed += 1,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        a_document_is_changed_from_what_is_held_and_removed,
        documents_are_kept_per_family_and_swept_by_what_they_say
    );

    fn put(registry: &dyn FactorStore, family: &str, key: &str, text: &str) {
        registry
            .factor_update(family, key, &mut |_| Ok(FactorChange::Put(text.to_owned())))
            .unwrap();
    }

    fn a_document_is_changed_from_what_is_held_and_removed(store: &dyn Store) {
        let registry = store.handle();
        let mut seen = Vec::new();
        registry
            .factor_update("totp", "id-alice", &mut |held| {
                seen.push(held.map(str::to_owned));
                Ok(FactorChange::Put("one".to_owned()))
            })
            .unwrap();
        // A second handle, as another gateway's, changes it from what the first wrote.
        store
            .handle()
            .factor_update("totp", "id-alice", &mut |held| {
                seen.push(held.map(str::to_owned));
                Ok(FactorChange::Put(format!("{}-two", held.unwrap())))
            })
            .unwrap();
        assert_eq!(seen, [None, Some("one".to_owned())]);
        let held = registry.factor_documents("totp").unwrap();
        assert_eq!(held, [("id-alice".to_owned(), "one-two".to_owned())]);
        // Keep changes nothing and an error from the edit changes nothing.
        registry
            .factor_update("totp", "id-alice", &mut |_| Ok(FactorChange::Keep))
            .unwrap();
        assert!(
            registry
                .factor_update("totp", "id-alice", &mut |_| Err(io::Error::other("no")))
                .is_err()
        );
        assert_eq!(registry.factor_documents("totp").unwrap(), held);
        registry
            .factor_update("totp", "id-alice", &mut |_| Ok(FactorChange::Remove))
            .unwrap();
        registry
            .factor_update("totp", "id-alice", &mut |_| Ok(FactorChange::Remove))
            .unwrap();
        assert!(registry.factor_documents("totp").unwrap().is_empty());
        for bad in ["", "a\nb"] {
            assert!(
                registry
                    .factor_update("totp", bad, &mut |_| Ok(FactorChange::Keep))
                    .is_err(),
                "{bad:?}"
            );
        }
        assert!(registry.factor_documents("../x").is_err());
    }

    fn documents_are_kept_per_family_and_swept_by_what_they_say(store: &dyn Store) {
        let registry = store.handle();
        put(registry.as_ref(), "pending", "a", "old");
        put(registry.as_ref(), "pending", "b/\u{e9}", "new");
        put(registry.as_ref(), "totp", "a", "old");
        assert_eq!(
            registry.factor_documents("pending").unwrap(),
            [
                ("a".to_owned(), "old".to_owned()),
                ("b/\u{e9}".to_owned(), "new".to_owned())
            ]
        );
        let removed = registry
            .factor_retain("pending", &mut |_, text| text != "old")
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(registry.factor_documents("pending").unwrap().len(), 1);
        assert_eq!(registry.factor_documents("totp").unwrap().len(), 1, "its own family");
        assert_eq!(
            registry.factor_retain("none", &mut |_, _| false).unwrap(),
            0
        );
    }
}
