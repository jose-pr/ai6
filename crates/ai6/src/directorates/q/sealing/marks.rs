// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a Station has accepted of the Registry's secret records: the newest `(term, revision)`
//! of each scope key record and each value, and the key ID of the key record.
//!
//! The Registry's records are not authenticated, and every Station writes it, so a Station that
//! was revoked or never was in a scope can put an older record back. A Station that remembers the
//! newest it accepted refuses the older one. The term is the election term a signed record carries;
//! while records are unsigned it is always 0, and ordering is by revision alone.
//!
//! The marks are the Station's own, in its state directory beside its sealing key. Without the file
//! (a new Station, a wiped state directory) a Station accepts what it first reads; restoring the
//! Registry from a backup makes every Station refuse until its marks are removed.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use super::VaultError;

/// The file beside a Station's identity that holds its marks.
pub const MARKS_FILE: &str = "sealing.marks";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mark {
    term: u64,
    revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
    /// Key IDs this record has had before the current one. The Registry hands a record's revision
    /// out itself, so an old record put back gets a newer revision; its key ID gives it away.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    retired: BTreeSet<String>,
}

/// The newest record of each name a Station has accepted.
#[derive(Debug, Default)]
pub struct Marks {
    path: Option<PathBuf>,
    held: Mutex<BTreeMap<String, Mark>>,
}

impl Marks {
    /// Marks kept in memory only: they last as long as the [`Vault`](super::Vault) holding them.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// The marks in `path`, kept there from now on; none yet when the file does not exist.
    ///
    /// # Errors
    ///
    /// When the file cannot be read or holds no marks; the message names the path.
    pub fn open(path: &Path) -> io::Result<Self> {
        let held = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} holds no sealing marks", path.display()),
                )
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            path: Some(path.to_path_buf()),
            held: Mutex::new(held),
        })
    }

    /// Whether `accept` would take the record, changing nothing: a signer checks before it writes.
    ///
    /// # Errors
    ///
    /// [`VaultError::Rollback`].
    pub(super) fn check(
        &self,
        label: &str,
        term: u64,
        revision: u64,
        key_id: Option<&str>,
    ) -> Result<(), VaultError> {
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(mark) = held.get(label) else {
            return Ok(());
        };
        let known = (mark.term, mark.revision);
        let older = (term, revision) < known;
        let retired = key_id.is_some_and(|id| mark.retired.contains(id));
        let other = (term, revision) == known && mark.key_id.as_deref() != key_id;
        if older || retired || other {
            return Err(VaultError::Rollback(label.to_owned()));
        }
        Ok(())
    }

    /// Accepts the record `label` at `(term, revision)` with `key_id`, or refuses it as older than
    /// one already accepted, or as another key at the same place.
    ///
    /// # Errors
    ///
    /// [`VaultError::Rollback`], or [`VaultError::Registry`] when the marks cannot be kept.
    pub(super) fn accept(
        &self,
        label: &str,
        term: u64,
        revision: u64,
        key_id: Option<&str>,
    ) -> Result<(), VaultError> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let mut retired = BTreeSet::new();
        if let Some(mark) = held.get(label) {
            let known = (mark.term, mark.revision);
            if (term, revision) < known || key_id.is_some_and(|id| mark.retired.contains(id)) {
                return Err(VaultError::Rollback(label.to_owned()));
            }
            if (term, revision) == known {
                if mark.key_id.as_deref() != key_id {
                    return Err(VaultError::Rollback(label.to_owned()));
                }
                return Ok(());
            }
            retired.clone_from(&mark.retired);
            if let Some(before) = &mark.key_id
                && key_id != Some(before.as_str())
            {
                retired.insert(before.clone());
            }
        }
        held.insert(
            label.to_owned(),
            Mark {
                term,
                revision,
                key_id: key_id.map(str::to_owned),
                retired,
            },
        );
        if let Some(path) = &self.path {
            let bytes = serde_json::to_vec(&*held).map_err(io::Error::other)?;
            let temporary = path.with_extension("tmp");
            std::fs::write(&temporary, bytes)?;
            std::fs::rename(&temporary, path)?;
        }
        Ok(())
    }
}
