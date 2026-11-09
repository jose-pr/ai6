//! Hashed bearer keys for clients that have nobody to type a password, kept in any [`Table`].
//!
//! A key is 256 random bits made here, said once, and kept only as a self-describing hash
//! (`sha256:<hex>` by default, or Argon2id). Each key can be disabled or removed alone and
//! may have a time it stops being taken. Build the provider in code with [`Keys::builder`].
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, Schema, ToValue};
use coqui_access::{
    AccountInfo, AccountPut, Accounts, Attempt, Authenticator, Description, Directory, Lookup,
    Person, PutOutcome, Verdict, error, takes,
};
use coqui_access_kit::{
    KeyHash, Keyed, MemoryTable, RecordLimits, Reloading, SourceError, Table, check_keys,
    generate_key, is_key_hash, secret, unix_now, verify_key,
};

/// The most keys a table may hold unless the builder says otherwise.
pub const DEFAULT_MAX_KEYS: usize = 10_000;

/// The prefix of keys made when the builder names none.
pub const DEFAULT_PREFIX: &str = "ck_";

/// One key account as a [`Table`] keeps it, keyed by [`name`](KeyRecord::name).
///
/// It derives guatiao's `ToValue`, `FromValue` and `Schema`, so a [`Table`] may keep it in
/// any format.
#[derive(Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct KeyRecord {
    /// Stable identity; what a [`Verdict::Valid`] lookup names.
    pub id: String,
    /// The account name; unique in the table.
    pub name: String,
    /// What is kept of the key: `sha256:<hex>` or an Argon2id hash, as [`KeyHash::hash`]
    /// writes it. One table may mix both.
    pub hash: String,
    /// Email address.
    pub email: Option<String>,
    /// Directory groups.
    #[map(default, skip_serializing_if = Vec::is_empty)]
    pub groups: Vec<String>,
    /// A disabled key is not taken.
    #[map(default, skip_serializing_if = core::ops::Not::not)]
    pub disabled: bool,
    /// When it was made, in seconds since the Unix epoch.
    pub made_unix: Option<i64>,
    /// When it stops being taken, in seconds since the Unix epoch.
    pub expires_unix: Option<i64>,
}

impl KeyRecord {
    /// An enabled key account with no groups. `hash` comes from [`KeyHash::hash`].
    pub fn new(id: impl Into<String>, name: impl Into<String>, hash: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            hash: hash.into(),
            email: None,
            groups: Vec::new(),
            disabled: false,
            made_unix: None,
            expires_unix: None,
        }
    }

    /// Sets the directory groups.
    #[must_use]
    pub fn groups(mut self, groups: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.groups = groups.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the email address.
    #[must_use]
    pub fn email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// Sets whether the key is disabled.
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets when the key stops being taken.
    #[must_use]
    pub fn expires_unix(mut self, expires: i64) -> Self {
        self.expires_unix = Some(expires);
        self
    }

    /// Whether it is taken now: not disabled and not past its time.
    fn taken(&self) -> bool {
        !self.disabled && self.expires_unix.is_none_or(|expires| unix_now() < expires)
    }
}

impl Keyed for KeyRecord {
    fn key(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.name)
    }
}

impl fmt::Debug for KeyRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyRecord")
            .field("id", &"[REDACTED]")
            .field("name", &"[REDACTED]")
            .field("hash", &"[REDACTED]")
            .field("email", &self.email.as_ref().map(|_| "[REDACTED]"))
            .field("groups", &self.groups.len())
            .field("disabled", &self.disabled)
            .finish()
    }
}

/// Options for [`Keys`]; every one has a default.
pub struct KeysBuilder {
    table: Option<Arc<dyn Table<KeyRecord>>>,
    prefix: String,
    hash: KeyHash,
    max_keys: usize,
    limits: RecordLimits,
}

impl fmt::Debug for KeysBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeysBuilder")
            .field("prefix", &self.prefix)
            .field("hash", &self.hash)
            .field("max_keys", &self.max_keys)
            .finish_non_exhaustive()
    }
}

impl KeysBuilder {
    /// Keeps the keys in `table` instead of in memory.
    #[must_use]
    pub fn table(mut self, table: Arc<dyn Table<KeyRecord>>) -> Self {
        self.table = Some(table);
        self
    }

    /// What every key made here begins with, so a key is recognized where it turns up and
    /// nothing else is looked up among them. Default [`DEFAULT_PREFIX`].
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// How keys made from now on are kept. Default [`KeyHash::Sha256`]. Keys already in the
    /// table keep the algorithm their stored string names.
    #[must_use]
    pub fn key_hash(mut self, hash: KeyHash) -> Self {
        self.hash = hash;
        self
    }

    /// The most keys the table may hold. Default [`DEFAULT_MAX_KEYS`].
    #[must_use]
    pub fn max_keys(mut self, max: usize) -> Self {
        self.max_keys = max;
        self
    }

    /// How large a key account's fields may be. Default [`RecordLimits::DEFAULT`].
    #[must_use]
    pub fn record_limits(mut self, limits: RecordLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Loads the table and makes the provider.
    ///
    /// # Errors
    ///
    /// Returns an error when the prefix is empty or not printable, or the table cannot be
    /// loaded or holds an invalid or duplicated key.
    pub fn build(self) -> Result<Keys, SourceError> {
        if self.prefix.is_empty() || self.prefix.chars().any(char::is_control) {
            return Err(SourceError::invalid(
                "prefix must be a non-empty printable string",
            ));
        }
        let table = self
            .table
            .unwrap_or_else(|| Arc::new(MemoryTable::<KeyRecord>::new()));
        let (max, limits) = (self.max_keys, self.limits);
        Ok(Keys {
            prefix: self.prefix,
            hash: self.hash,
            limits,
            keys: Reloading::new(table, move |records| validate(records, max, &limits))?,
        })
    }
}

/// A key table as an Authenticator, Directory and Accounts implementation.
///
/// The table is read again whenever it reports a change.
///
/// ```
/// use coqui_access::{AccountPut, Accounts, Attempt, Authenticator, Verdict};
/// use coqui_access_key::Keys;
///
/// let keys = Keys::builder().build().unwrap();
/// let made = keys
///     .put(AccountPut { name: "relay".into(), ..AccountPut::default() })
///     .unwrap();
/// let verdict = keys
///     .authenticate(Attempt { secret: made.secret, ..Attempt::default() })
///     .unwrap();
/// assert!(matches!(verdict, Verdict::Valid(_)));
/// ```
#[derive(Debug)]
pub struct Keys {
    limits: RecordLimits,
    prefix: String,
    hash: KeyHash,
    keys: Reloading<KeyRecord>,
}

impl Keys {
    /// Options for a key table, kept in memory unless [`table`](KeysBuilder::table) says
    /// otherwise.
    pub fn builder() -> KeysBuilder {
        KeysBuilder {
            table: None,
            prefix: DEFAULT_PREFIX.to_owned(),
            hash: KeyHash::default(),
            max_keys: DEFAULT_MAX_KEYS,
            limits: RecordLimits::DEFAULT,
        }
    }

    /// Makes a fresh 256-bit key with this table's prefix.
    ///
    /// # Errors
    ///
    /// Returns an error when the operating system random source is unavailable.
    pub fn new_key(&self) -> Result<String, String> {
        generate_key(&self.prefix)
    }
}

impl Authenticator for Keys {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let Some(presented) = attempt.secret.filter(|key| key.starts_with(&self.prefix)) else {
            return Ok(Verdict::Unknown);
        };
        let found = self
            .keys
            .read(|records| {
                // Every record is checked, none stopping the others.
                let mut found = None;
                for record in records {
                    if verify_key(&record.hash, presented.as_bytes()) {
                        found = Some(record.clone());
                    }
                }
                found
            })
            .map_err(|reason| reason.reading("key table"))?;
        Ok(match found {
            None => Verdict::Unknown,
            Some(record) if record.taken() => Verdict::Valid(Lookup {
                by: "id".to_owned(),
                value: record.id,
            }),
            Some(_) => Verdict::Invalid,
        })
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description {
            name: Some("Bearer keys".to_owned()),
            takes: Some(takes::KEY.to_owned()),
        })
    }
}

impl Directory for Keys {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        self.keys
            .read(|records| {
                records
                    .iter()
                    .find(|record| {
                        record.taken()
                            && match lookup.by.as_str() {
                                "id" => record.id == lookup.value,
                                "name" => record.name == lookup.value,
                                "email" => record.email.as_deref() == Some(&lookup.value),
                                _ => false,
                            }
                    })
                    .map(|record| Person {
                        id: record.id.clone(),
                        name: record.name.clone(),
                        display_name: None,
                        email: record.email.clone(),
                        groups: record.groups.clone(),
                        attributes: Default::default(),
                    })
            })
            .map_err(|reason| reason.reading("key table"))
    }
}

impl Accounts for Keys {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError> {
        self.keys
            .read(|records| {
                records
                    .iter()
                    .map(|record| AccountInfo {
                        id: record.id.clone(),
                        name: record.name.clone(),
                        groups: record.groups.clone(),
                        disabled: Some(record.disabled),
                        made_unix: record.made_unix,
                        expires_unix: record.expires_unix,
                    })
                    .collect()
            })
            .map_err(|reason| reason.reading("key table"))
    }

    /// Makes an account and its key, or changes what is said of one there is. An account
    /// that is there and has nothing said to change is given another key, and the one it had
    /// stops being taken. A key is never given: it is made here and answered once.
    fn put(&self, account: AccountPut) -> Result<PutOutcome, ProviderError> {
        if !self.limits.allows_field(&account.name) {
            return Err(error::bad_value("an account has a name"));
        }
        if account.secret.is_some() {
            return Err(error::bad_value("a key is made here, not given"));
        }
        let unchanged = account.groups.is_none()
            && account.disabled.is_none()
            && account.expires_unix.is_none();
        self.keys
            .put_with(|records| {
                let existing = records.iter().find(|record| record.name == account.name);
                let mut record = match existing {
                    Some(record) => record.clone(),
                    None => KeyRecord::new(
                        secret::identifier().map_err(SourceError::unavailable)?,
                        account.name.clone(),
                        "",
                    ),
                };
                let mut made = None;
                if existing.is_none() || unchanged {
                    let key = generate_key(&self.prefix).map_err(SourceError::unavailable)?;
                    record.hash = self.hash.hash(&key).map_err(SourceError::unavailable)?;
                    record.made_unix = Some(unix_now());
                    made = Some(key);
                }
                if let Some(groups) = account.groups {
                    record.groups = groups;
                }
                if let Some(disabled) = account.disabled {
                    record.disabled = disabled;
                }
                if let Some(expires) = account.expires_unix {
                    record.expires_unix = Some(expires);
                }
                Ok((record, PutOutcome { secret: made }))
            })
            .map_err(|reason| reason.writing("key table"))
    }

    fn remove(&self, name: String) -> Result<(), ProviderError> {
        self.keys
            .remove(&name)
            .map_err(|reason| reason.writing("key table"))
            .and_then(|removed| {
                removed
                    .then_some(())
                    .ok_or_else(|| error::not_found("there is no such account"))
            })
    }
}

fn validate(records: &[KeyRecord], max: usize, limits: &RecordLimits) -> Result<(), SourceError> {
    check_keys(records, max)?;
    let mut ids = BTreeSet::new();
    let mut hashes = BTreeSet::new();
    for record in records {
        if !limits.allows_field(&record.id)
            || !limits.allows_field(&record.name)
            || !is_key_hash(&record.hash)
            || !ids.insert(&record.id)
            || !hashes.insert(&record.hash)
            || !limits.allows_groups(&record.groups)
            || record
                .email
                .as_ref()
                .is_some_and(|email| !limits.allows_email(email))
        {
            return Err(SourceError::invalid("a key entry is invalid or duplicated"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use coqui_access_kit::Argon2Params;

    use super::*;

    fn presented(key: &str) -> Attempt {
        Attempt {
            secret: Some(key.to_owned()),
            ..Attempt::default()
        }
    }

    fn named(name: &str) -> AccountPut {
        AccountPut {
            name: name.to_owned(),
            ..AccountPut::default()
        }
    }

    fn keys_over(table: Arc<MemoryTable<KeyRecord>>) -> Keys {
        Keys::builder().table(table).build().unwrap()
    }

    #[test]
    fn a_table_built_in_code_authenticates_and_resolves() {
        let table = Arc::new(MemoryTable::new());
        let keys = keys_over(table.clone());
        let secret = keys.new_key().unwrap();
        table
            .put(
                KeyRecord::new("robot-1", "relay", KeyHash::Sha256.hash(&secret).unwrap())
                    .groups(["station"]),
            )
            .unwrap();
        let Verdict::Valid(lookup) = keys.authenticate(presented(&secret)).unwrap() else {
            panic!("valid key was rejected");
        };
        let person = keys.lookup(lookup).unwrap().unwrap();
        assert_eq!(person.id, "robot-1");
        assert_eq!(person.groups, vec!["station"]);
        assert!(!format!("{keys:?}").contains(&secret));
    }

    #[test]
    fn unknown_disabled_and_foreign_keys_are_told_apart() {
        let keys = Keys::builder().build().unwrap();
        let key = keys.put(named("relay")).unwrap().secret.unwrap();
        keys.put(AccountPut {
            disabled: Some(true),
            ..named("relay")
        })
        .unwrap();
        assert_eq!(
            keys.authenticate(presented(&key)).unwrap(),
            Verdict::Invalid
        );
        assert_eq!(
            keys.authenticate(presented(&keys.new_key().unwrap()))
                .unwrap(),
            Verdict::Unknown
        );
        assert_eq!(
            keys.authenticate(presented("password")).unwrap(),
            Verdict::Unknown
        );
        assert!(
            keys.lookup(Lookup {
                by: "name".to_owned(),
                value: "relay".to_owned()
            })
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn keys_are_made_rotated_expired_and_removed() {
        let table = Arc::new(MemoryTable::new());
        let keys = keys_over(table.clone());
        let first = keys.put(named("cabin")).unwrap().secret.unwrap();
        assert!(first.starts_with("ck_"));
        let kept = table.load().unwrap().records;
        assert!(!kept[0].hash.contains(first.trim_start_matches("ck_")));
        assert_eq!(kept[0].hash, KeyHash::Sha256.hash(&first).unwrap());
        // Changing what is said of it keeps the key; saying nothing makes another.
        let moved = keys
            .put(AccountPut {
                groups: Some(vec!["guests".to_owned()]),
                ..named("cabin")
            })
            .unwrap();
        assert_eq!(moved.secret, None);
        assert!(matches!(
            keys.authenticate(presented(&first)).unwrap(),
            Verdict::Valid(_)
        ));
        let second = keys.put(named("cabin")).unwrap().secret.unwrap();
        assert_ne!(first, second);
        assert_eq!(
            keys.authenticate(presented(&first)).unwrap(),
            Verdict::Unknown
        );
        assert!(
            keys.put(AccountPut {
                secret: Some("ck_mine".to_owned()),
                ..named("attic")
            })
            .is_err()
        );
        keys.put(AccountPut {
            expires_unix: Some(unix_now() - 1),
            ..named("cabin")
        })
        .unwrap();
        assert_eq!(
            keys.authenticate(presented(&second)).unwrap(),
            Verdict::Invalid
        );
        assert!(keys.accounts().unwrap()[0].expires_unix.is_some());
        keys.remove("cabin".to_owned()).unwrap();
        assert!(keys.remove("cabin".to_owned()).is_err());
        assert_eq!(
            keys.authenticate(presented(&second)).unwrap(),
            Verdict::Unknown
        );
    }

    #[test]
    fn a_key_made_through_another_handle_is_known_without_rebuilding() {
        let table = Arc::new(MemoryTable::new());
        let daemon = keys_over(table.clone());
        let command = keys_over(table);
        let key = command.put(named("cabin")).unwrap().secret.unwrap();
        assert!(matches!(
            daemon.authenticate(presented(&key)).unwrap(),
            Verdict::Valid(_)
        ));
    }

    #[test]
    fn one_table_mixes_sha256_and_argon2id_keys() {
        let cheap = KeyHash::Argon2id(Argon2Params::new(64, 1, 1).unwrap());
        let table = Arc::new(MemoryTable::new());
        let sha = Keys::builder().table(table.clone()).build().unwrap();
        let slow = Keys::builder()
            .table(table.clone())
            .key_hash(cheap)
            .build()
            .unwrap();
        let fast_key = sha.put(named("fast")).unwrap().secret.unwrap();
        let slow_key = slow.put(named("slow")).unwrap().secret.unwrap();
        let kept = table.load().unwrap().records;
        assert!(kept[0].hash.starts_with("sha256:"));
        assert!(kept[1].hash.starts_with("$argon2id$"));
        for keys in [&sha, &slow] {
            for (key, name) in [(&fast_key, "fast"), (&slow_key, "slow")] {
                let Verdict::Valid(lookup) = keys.authenticate(presented(key)).unwrap() else {
                    panic!("{name} was refused");
                };
                assert_eq!(keys.lookup(lookup).unwrap().unwrap().name, name);
            }
            // A key of the right prefix that is in neither is not known.
            assert_eq!(
                keys.authenticate(presented(&keys.new_key().unwrap()))
                    .unwrap(),
                Verdict::Unknown
            );
        }
    }

    #[test]
    fn a_malformed_table_or_prefix_is_refused() {
        assert!(Keys::builder().prefix("").build().is_err());
        assert!(Keys::builder().prefix("a\nb").build().is_err());
        assert!(Keys::builder().prefix("rk_").build().is_ok());
        let build = |records: Vec<KeyRecord>, max: usize| {
            Keys::builder()
                .table(Arc::new(MemoryTable::from_records(records)))
                .max_keys(max)
                .build()
        };
        assert!(build(vec![KeyRecord::new("a", "a", "short")], 10).is_err());
        // A bare digest names no algorithm.
        assert!(build(vec![KeyRecord::new("a", "a", secret::hash("k", None))], 10).is_err());
        let one = KeyHash::Sha256.hash("k").unwrap();
        let two = KeyHash::Sha256.hash("l").unwrap();
        assert!(
            build(
                vec![
                    KeyRecord::new("a", "a", one.clone()),
                    KeyRecord::new("b", "b", two.clone())
                ],
                10
            )
            .is_ok()
        );
        assert!(
            build(
                vec![
                    KeyRecord::new("a", "a", one.clone()),
                    KeyRecord::new("b", "b", one.clone())
                ],
                10
            )
            .is_err(),
            "two accounts cannot share a key"
        );
        assert!(
            build(
                vec![KeyRecord::new("a", "a", one), KeyRecord::new("b", "b", two)],
                1
            )
            .is_err()
        );
    }
}
