//! Principals: the people the gateway has seen sign in, by the name their directory gave.
//!
//! The directory behind sign-in answers only for the person asking, so a roster of who has
//! been seen is what lets an audit record, an access rule or a page name a person who is not
//! asking now. The name is for display only: nothing here decides access. A person never
//! seen has no record, and callers show their ID. Files: `principals/DIGEST.json`, the digest
//! of the person ID, which may hold characters a Registry key does not.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::limits::PrincipalLimits;
use super::store::{FileRegistry, hex_digest, invalid_data};

/// A person the gateway has seen sign in.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalRecord {
    /// The directory's person ID (a user ID, a directory SID), as access rules name it.
    pub id: String,
    /// The account name.
    pub name: String,
    /// The name to show, when the directory has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The groups the person was in when last seen.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// When first seen, as Unix seconds.
    pub first_seen: f64,
    /// When last seen, as Unix seconds, to the refresh interval's precision.
    pub last_seen: f64,
}

impl PrincipalRecord {
    /// A person `id`, account `name`, seen at `at`.
    pub fn new(id: impl Into<String>, name: impl Into<String>, at: f64) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            display_name: None,
            groups: Vec::new(),
            first_seen: at,
            last_seen: at,
        }
    }

    /// Sets the name to show.
    #[must_use]
    pub fn displayed_as(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = Some(display_name.into());
        self
    }

    /// Sets the groups.
    #[must_use]
    pub fn in_groups(mut self, groups: Vec<String>) -> Self {
        self.groups = groups;
        self
    }

    /// What to show for the person: the display name, else the account name.
    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.name)
    }

    /// The Registry key of the record for person `id`.
    pub fn key(id: &str) -> String {
        hex_digest(&Sha256::digest(id.as_bytes()))[..32].to_owned()
    }

    /// Whether `seen` says something this record does not, or this one is more than
    /// `refresh` seconds older: the reason to write it again.
    pub fn outdated_by(&self, seen: &Self, refresh: f64) -> bool {
        self.name != seen.name
            || self.display_name != seen.display_name
            || self.groups != seen.groups
            || seen.last_seen - self.last_seen > refresh
    }

    /// Checks the record's bounds.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an empty or oversized ID, name or group, too many groups, or a time
    /// that is not finite and positive.
    pub fn validate(&self, limits: &PrincipalLimits) -> io::Result<()> {
        let bounded = |text: &str| {
            !text.trim().is_empty()
                && limits.text_bytes.allows(text.len())
                && !text.contains(char::is_control)
        };
        let time = |at: f64| at.is_finite() && at > 0.0;
        if !bounded(&self.id)
            || !bounded(&self.name)
            || !self.display_name.as_deref().is_none_or(bounded)
            || !limits.groups.allows(self.groups.len())
            || !self.groups.iter().all(|group| bounded(group))
            || !time(self.first_seen)
            || !time(self.last_seen)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a principal needs an ID and a name, each in bounds",
            ));
        }
        Ok(())
    }
}

/// Principals: the people seen signing in.
pub trait PrincipalStore {
    /// Notes that `seen` was seen: stores it when there is no record of that ID, when its
    /// name or groups differ from the record's, or when the record's last sighting is more than
    /// `[registry.limits.principals] refresh` older; otherwise leaves the record alone. The
    /// record keeps the first sighting it had. True when it wrote.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a record out of bounds ([`PrincipalRecord::validate`]);
    /// `InvalidData` past `[registry.limits.principals] principals`; I/O errors otherwise.
    fn note_principal(&self, seen: &PrincipalRecord) -> io::Result<bool>;

    /// The person `id`, or `None` when never seen.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn principal(&self, id: &str) -> io::Result<Option<PrincipalRecord>>;

    /// Every person seen, by label then ID. Unreadable records are skipped.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn principals(&self) -> io::Result<Vec<PrincipalRecord>>;
}

/// The record to write for `seen` given `existing`, or `None` when nothing is to be written.
pub(super) fn merged(
    existing: Option<PrincipalRecord>,
    seen: &PrincipalRecord,
    limits: &PrincipalLimits,
) -> Option<PrincipalRecord> {
    match existing {
        None => Some(seen.clone()),
        Some(old) => {
            if !old.outdated_by(seen, limits.refresh.as_secs_f64()) {
                return None;
            }
            let mut next = seen.clone();
            next.first_seen = old.first_seen.min(seen.first_seen);
            Some(next)
        }
    }
}

pub(super) fn sorted(mut people: Vec<PrincipalRecord>) -> Vec<PrincipalRecord> {
    people.sort_by(|left, right| {
        left.label()
            .to_lowercase()
            .cmp(&right.label().to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    people
}

impl FileRegistry {
    fn principals_root(&self) -> PathBuf {
        self.root.join("principals")
    }

    fn principal_path(&self, id: &str) -> PathBuf {
        self.principals_root()
            .join(format!("{}.json", PrincipalRecord::key(id)))
    }

    fn read_principal(&self, id: &str) -> io::Result<Option<PrincipalRecord>> {
        match fs::read(self.principal_path(id)) {
            Ok(bytes) => parse_principal(&bytes, &self.limits.principals).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl PrincipalStore for FileRegistry {
    fn note_principal(&self, seen: &PrincipalRecord) -> io::Result<bool> {
        let limits = &self.limits.principals;
        seen.validate(limits)?;
        let _lock = self.lock()?;
        let existing = self.read_principal(&seen.id)?;
        let fresh = existing.is_none();
        let Some(next) = merged(existing, seen, limits) else {
            return Ok(false);
        };
        if fresh
            && !limits
                .principals
                .allows(count_files(&self.principals_root())? + 1)
        {
            return Err(invalid_data("too many principals"));
        }
        let bytes = principal_bytes(&next, limits)?;
        self.write_atomic(&self.principal_path(&seen.id), &bytes)?;
        Ok(true)
    }

    fn principal(&self, id: &str) -> io::Result<Option<PrincipalRecord>> {
        self.read_principal(id)
    }

    fn principals(&self) -> io::Result<Vec<PrincipalRecord>> {
        let entries = match fs::read_dir(self.principals_root()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut people = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if let Ok(bytes) = fs::read(&path)
                && let Ok(person) = parse_principal(&bytes, &self.limits.principals)
                && path.file_stem().and_then(|stem| stem.to_str())
                    == Some(PrincipalRecord::key(&person.id).as_str())
            {
                people.push(person);
            }
        }
        Ok(sorted(people))
    }
}

fn count_files(root: &std::path::Path) -> io::Result<usize> {
    match fs::read_dir(root) {
        Ok(entries) => Ok(entries.count()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

/// The stored form of a principal, refused past the size limit.
pub(super) fn principal_bytes(
    person: &PrincipalRecord,
    limits: &PrincipalLimits,
) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(person).map_err(io::Error::other)?;
    if !limits.record_bytes.allows(bytes.len()) {
        return Err(invalid_data("the principal exceeds the size limit"));
    }
    Ok(bytes)
}

/// A stored principal, checked against the size limit and its bounds.
pub(super) fn parse_principal(
    bytes: &[u8],
    limits: &PrincipalLimits,
) -> io::Result<PrincipalRecord> {
    if !limits.record_bytes.allows(bytes.len()) {
        return Err(invalid_data("the principal exceeds the size limit"));
    }
    let person: PrincipalRecord = serde_json::from_slice(bytes)
        .map_err(|error| invalid_data(format!("invalid principal: {error}")))?;
    person
        .validate(limits)
        .map_err(|error| invalid_data(error.to_string()))?;
    Ok(person)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(a_person_seen_is_kept_updated_when_it_changes_and_listed_by_name);

    fn person(id: &str, name: &str, at: f64) -> PrincipalRecord {
        PrincipalRecord::new(id, name, at)
    }

    fn a_person_seen_is_kept_updated_when_it_changes_and_listed_by_name(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.principals().unwrap().is_empty());
        assert_eq!(registry.principal("person:1000").unwrap(), None);

        // An ID with characters a Registry key lacks is kept all the same.
        let ada = person("S-1-5-21-1-2-3-1000", "ada", 10.0)
            .displayed_as("Ada Lovelace")
            .in_groups(vec!["analysts".to_owned()]);
        assert!(registry.note_principal(&ada).unwrap());
        assert!(
            registry
                .note_principal(&person("1001", "bob", 11.0))
                .unwrap()
        );

        // Seen again soon with nothing new: not written.
        let mut again = ada.clone();
        again.last_seen = 11.0;
        assert!(!registry.note_principal(&again).unwrap());
        assert_eq!(
            registry.principal(&ada.id).unwrap().unwrap().last_seen,
            10.0
        );

        // A new group is written, and the first sighting is kept.
        let mut regrouped = ada.clone().in_groups(vec!["admins".to_owned()]);
        regrouped.first_seen = 50.0;
        regrouped.last_seen = 50.0;
        assert!(registry.note_principal(&regrouped).unwrap());
        let stored = registry.principal(&ada.id).unwrap().unwrap();
        assert_eq!(stored.groups, ["admins"]);
        assert_eq!((stored.first_seen, stored.last_seen), (10.0, 50.0));

        // Long after, the sighting alone moves.
        let mut later = regrouped.clone();
        later.last_seen = 10_000_000.0;
        assert!(registry.note_principal(&later).unwrap());

        let labels = registry
            .principals()
            .unwrap()
            .iter()
            .map(|person| person.label().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["Ada Lovelace", "bob"]);

        for bad in [
            person(" ", "x", 1.0),
            person("2000", "", 1.0),
            person("2000", "x", f64::NAN),
        ] {
            assert!(registry.note_principal(&bad).is_err(), "{bad:?}");
        }
    }
}
