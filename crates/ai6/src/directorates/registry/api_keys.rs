//! Personal API keys: who made each, what it is narrowed to, and when it was last used.
//!
//! The key itself, and whatever proves it, is the key provider's (`coqui_access_key`): the
//! Registry holds no secret and no hash, only the record that says whose key a provider account
//! is. A record's ID is the provider's account ID, so a presented key finds its record by the
//! ID the provider answers with. Files: `api-keys/ID.json`.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::limits::ApiKeyLimits;
use super::store::{FileRegistry, invalid_data, validate_registry_key};

/// A personal API key, as the Registry keeps it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyRecord {
    /// The key provider's account ID, which names the key everywhere.
    pub id: String,
    /// The key provider's account name, for removing the account.
    pub account: String,
    /// The ID of the person the key acts as.
    pub owner: String,
    /// The person's name, for a listing.
    pub owner_name: String,
    /// What the maker called the key.
    pub name: String,
    /// What it may do, each as `ACTION@SCOPE`.
    pub scopes: Vec<String>,
    /// When it was made, as Unix seconds.
    pub created_at: f64,
    /// When it stops working, as Unix seconds; `None` never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<f64>,
    /// When it was last used, as Unix seconds, to the access cache's precision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<f64>,
}

impl ApiKeyRecord {
    /// A key `id` (the provider's account `account`) of person `owner`, named `name`, allowed
    /// `scopes`, made at `created_at`.
    pub fn new(
        id: impl Into<String>,
        account: impl Into<String>,
        owner: impl Into<String>,
        owner_name: impl Into<String>,
        name: impl Into<String>,
        scopes: Vec<String>,
        created_at: f64,
    ) -> Self {
        Self {
            id: id.into(),
            account: account.into(),
            owner: owner.into(),
            owner_name: owner_name.into(),
            name: name.into(),
            scopes,
            created_at,
            expires_at: None,
            last_used_at: None,
        }
    }

    /// Sets when the key stops working.
    #[must_use]
    pub fn expiring_at(mut self, expires_at: f64) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Whether the key has stopped working at `now`.
    pub fn expired_at(&self, now: f64) -> bool {
        self.expires_at.is_some_and(|end| end <= now)
    }

    /// Checks the record's bounds.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an ID that is not a Registry key, an empty or oversized name, owner
    /// or scope, too many scopes, or a time that is not finite and positive.
    pub fn validate(&self, limits: &ApiKeyLimits) -> io::Result<()> {
        validate_registry_key(&self.id)?;
        let bounded = |text: &str| {
            !text.trim().is_empty()
                && limits.text_bytes.allows(text.len())
                && !text.contains(char::is_control)
        };
        let time = |at: f64| at.is_finite() && at > 0.0;
        if !bounded(&self.account)
            || !bounded(&self.owner)
            || !bounded(&self.owner_name)
            || !bounded(&self.name)
            || self.scopes.is_empty()
            || !limits.scopes.allows(self.scopes.len())
            || !self.scopes.iter().all(|scope| bounded(scope))
            || !time(self.created_at)
            || !self.expires_at.is_none_or(time)
            || !self.last_used_at.is_none_or(time)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an API key needs a name, an owner and scopes, each in bounds",
            ));
        }
        Ok(())
    }
}

/// Personal API keys.
pub trait ApiKeyStore {
    /// Stores a key's record, replacing the one with its ID.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a record out of bounds ([`ApiKeyRecord::validate`]); `InvalidData`
    /// past `[registry.limits.api_keys] keys`; I/O errors otherwise.
    fn put_api_key(&self, key: &ApiKeyRecord) -> io::Result<()>;

    /// The key `id`, or `None` when there is none.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an ID that is not a Registry key; I/O errors otherwise.
    fn api_key(&self, id: &str) -> io::Result<Option<ApiKeyRecord>>;

    /// Every key, or those of person `owner`, oldest first. Unreadable records are skipped.
    ///
    /// # Errors
    ///
    /// I/O errors.
    fn api_keys(&self, owner: Option<&str>) -> io::Result<Vec<ApiKeyRecord>>;

    /// Removes the key `id` and returns its record, or `None` when there was none.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an ID that is not a Registry key; I/O errors otherwise.
    fn remove_api_key(&self, id: &str) -> io::Result<Option<ApiKeyRecord>>;

    /// Records that key `id` was used at `at`, unless its last use is at most `within` seconds
    /// before; true when it wrote. A key with no record is not an error.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an ID that is not a Registry key; I/O errors otherwise.
    fn touch_api_key(&self, id: &str, at: f64, within: f64) -> io::Result<bool>;
}

impl FileRegistry {
    fn api_keys_root(&self) -> PathBuf {
        self.root.join("api-keys")
    }

    fn api_key_path(&self, id: &str) -> PathBuf {
        self.api_keys_root().join(format!("{id}.json"))
    }

    fn read_api_key(&self, id: &str) -> io::Result<Option<ApiKeyRecord>> {
        match fs::read(self.api_key_path(id)) {
            Ok(bytes) => parse_api_key(&bytes, &self.limits.api_keys).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl ApiKeyStore for FileRegistry {
    fn put_api_key(&self, key: &ApiKeyRecord) -> io::Result<()> {
        let limits = &self.limits.api_keys;
        key.validate(limits)?;
        let bytes = api_key_bytes(key, limits)?;
        let _lock = self.lock()?;
        let path = self.api_key_path(&key.id);
        if !path.exists() && !limits.keys.allows(count_files(&self.api_keys_root())? + 1) {
            return Err(invalid_data("too many API keys"));
        }
        self.write_atomic(&path, &bytes)
    }

    fn api_key(&self, id: &str) -> io::Result<Option<ApiKeyRecord>> {
        validate_registry_key(id)?;
        self.read_api_key(id)
    }

    fn api_keys(&self, owner: Option<&str>) -> io::Result<Vec<ApiKeyRecord>> {
        let entries = match fs::read_dir(self.api_keys_root()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut keys = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if let Ok(bytes) = fs::read(&path)
                && let Ok(key) = parse_api_key(&bytes, &self.limits.api_keys)
                && path.file_stem().and_then(|stem| stem.to_str()) == Some(key.id.as_str())
                && owner.is_none_or(|owner| key.owner == owner)
            {
                keys.push(key);
            }
        }
        keys.sort_by(|left, right| {
            left.created_at
                .total_cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(keys)
    }

    fn remove_api_key(&self, id: &str) -> io::Result<Option<ApiKeyRecord>> {
        validate_registry_key(id)?;
        let _lock = self.lock()?;
        let Some(key) = self.read_api_key(id)? else {
            return Ok(None);
        };
        fs::remove_file(self.api_key_path(id))?;
        Ok(Some(key))
    }

    fn touch_api_key(&self, id: &str, at: f64, within: f64) -> io::Result<bool> {
        validate_registry_key(id)?;
        let _lock = self.lock()?;
        let Some(mut key) = self.read_api_key(id)? else {
            return Ok(false);
        };
        if !touches(&key, at, within) {
            return Ok(false);
        }
        key.last_used_at = Some(at);
        let bytes = api_key_bytes(&key, &self.limits.api_keys)?;
        self.write_atomic(&self.api_key_path(id), &bytes)?;
        Ok(true)
    }
}

/// Whether a use at `at` is worth writing: the last one was more than `within` seconds ago.
pub(super) fn touches(key: &ApiKeyRecord, at: f64, within: f64) -> bool {
    key.last_used_at.is_none_or(|last| at - last > within)
}

fn count_files(root: &std::path::Path) -> io::Result<usize> {
    match fs::read_dir(root) {
        Ok(entries) => Ok(entries.count()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

/// The stored form of a key, refused past the size limit.
pub(super) fn api_key_bytes(key: &ApiKeyRecord, limits: &ApiKeyLimits) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(key).map_err(io::Error::other)?;
    if !limits.record_bytes.allows(bytes.len()) {
        return Err(invalid_data("the API key exceeds the size limit"));
    }
    Ok(bytes)
}

/// A stored key, checked against the size limit and its bounds.
pub(super) fn parse_api_key(bytes: &[u8], limits: &ApiKeyLimits) -> io::Result<ApiKeyRecord> {
    if !limits.record_bytes.allows(bytes.len()) {
        return Err(invalid_data("the API key exceeds the size limit"));
    }
    let key: ApiKeyRecord = serde_json::from_slice(bytes)
        .map_err(|error| invalid_data(format!("invalid API key: {error}")))?;
    key.validate(limits)
        .map_err(|error| invalid_data(error.to_string()))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(a_key_is_kept_listed_by_owner_touched_and_removed);

    fn key(id: &str, owner: &str, at: f64) -> ApiKeyRecord {
        ApiKeyRecord::new(
            id,
            format!("uk-{id}"),
            owner,
            format!("{owner} Name"),
            "deploy",
            vec!["config.manage@config:plugin".to_owned()],
            at,
        )
    }

    fn a_key_is_kept_listed_by_owner_touched_and_removed(store: &dyn Store) {
        let registry = store.handle();
        assert!(registry.api_keys(None).unwrap().is_empty());
        registry.put_api_key(&key("k1", "alice", 10.0)).unwrap();
        registry
            .put_api_key(&key("k2", "bob", 11.0).expiring_at(99.0))
            .unwrap();
        registry.put_api_key(&key("k3", "alice", 12.0)).unwrap();

        let all = registry.api_keys(None).unwrap();
        assert_eq!(
            all.iter().map(|key| key.id.as_str()).collect::<Vec<_>>(),
            ["k1", "k2", "k3"],
            "oldest first"
        );
        let alices = registry.api_keys(Some("alice")).unwrap();
        assert_eq!(alices.len(), 2);
        assert!(alices.iter().all(|key| key.owner == "alice"));
        let bobs = registry.api_key("k2").unwrap().unwrap();
        assert!(bobs.expired_at(99.0) && !bobs.expired_at(98.0));
        assert_eq!(registry.api_key("nobody").unwrap(), None);

        // A use is written when none was, and again only past `within`.
        assert!(registry.touch_api_key("k1", 100.0, 5.0).unwrap());
        assert!(!registry.touch_api_key("k1", 103.0, 5.0).unwrap());
        assert!(registry.touch_api_key("k1", 106.0, 5.0).unwrap());
        assert_eq!(
            registry.api_key("k1").unwrap().unwrap().last_used_at,
            Some(106.0)
        );
        assert!(!registry.touch_api_key("nobody", 1.0, 0.0).unwrap());

        assert_eq!(
            registry.remove_api_key("k1").unwrap().unwrap().owner,
            "alice"
        );
        assert_eq!(registry.remove_api_key("k1").unwrap(), None);
        assert_eq!(registry.api_keys(Some("alice")).unwrap().len(), 1);

        let mut nameless = key("k4", "alice", 13.0);
        nameless.name = " ".to_owned();
        let mut scopeless = key("k5", "alice", 13.0);
        scopeless.scopes.clear();
        for bad in [
            nameless,
            scopeless,
            key("k6", "alice", f64::NAN),
            key("../x", "alice", 1.0),
        ] {
            assert!(registry.put_api_key(&bad).is_err(), "{bad:?}");
        }
    }
}
