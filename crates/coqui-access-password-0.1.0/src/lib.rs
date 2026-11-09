//! Password accounts with Argon2id password hashes, kept in any [`Table`].
//!
//! Build one in code with [`PasswordAccounts::builder`]; the default table is in memory. The
//! provider authenticates accounts, resolves the same records as a directory, and manages
//! them through [`Accounts`].
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
    AccountInfo, AccountPut, Accounts, Attempt, Authenticator, Description, Directory, Limit,
    Lookup, Person, PutOutcome, Verdict, error, takes,
};
use coqui_access_kit::{
    Argon2Params, DEFAULT_MAX_PASSWORD_BYTES, Keyed, MemoryTable, PasswordHasher, RecordLimits,
    Reloading, SourceError, Table, check_keys, is_password_hash, secret, unix_now,
};

/// The most accounts a table may hold unless the builder says otherwise.
pub const DEFAULT_MAX_ACCOUNTS: usize = 10_000;

/// One password account as a [`Table`] keeps it, keyed by [`name`](AccountRecord::name).
///
/// It derives guatiao's `ToValue`, `FromValue` and `Schema`, so a [`Table`] may keep it in
/// any format.
#[derive(Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct AccountRecord {
    /// Stable identity; what a [`Verdict::Valid`] lookup names.
    pub id: String,
    /// The name the person signs in with; unique in the table.
    pub name: String,
    /// The Argon2id hash of the password, as [`PasswordHasher::hash`] makes it.
    pub password_hash: String,
    /// Human-readable name.
    pub display_name: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// Directory groups.
    #[map(default, skip_serializing_if = Vec::is_empty)]
    pub groups: Vec<String>,
    /// A disabled account neither authenticates nor resolves.
    #[map(default, skip_serializing_if = core::ops::Not::not)]
    pub disabled: bool,
    /// When it was made, in seconds since the Unix epoch.
    pub made_unix: Option<i64>,
}

impl AccountRecord {
    /// An enabled account with no groups. `password_hash` comes from [`PasswordHasher::hash`].
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        password_hash: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            password_hash: password_hash.into(),
            display_name: None,
            email: None,
            groups: Vec::new(),
            disabled: false,
            made_unix: None,
        }
    }

    /// Sets the directory groups.
    #[must_use]
    pub fn groups(mut self, groups: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.groups = groups.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the human-readable name.
    #[must_use]
    pub fn display_name(mut self, display_name: impl Into<String>) -> Self {
        self.display_name = Some(display_name.into());
        self
    }

    /// Sets the email address.
    #[must_use]
    pub fn email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// Sets whether the account is disabled.
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Keyed for AccountRecord {
    fn key(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.name)
    }
}

impl fmt::Debug for AccountRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountRecord")
            .field("id", &"[REDACTED]")
            .field("name", &"[REDACTED]")
            .field("password_hash", &"[REDACTED]")
            .field(
                "display_name",
                &self.display_name.as_ref().map(|_| "[REDACTED]"),
            )
            .field("email", &self.email.as_ref().map(|_| "[REDACTED]"))
            .field("groups", &self.groups.len())
            .field("disabled", &self.disabled)
            .finish()
    }
}

/// Options for [`PasswordAccounts`]; every one has a default.
pub struct PasswordAccountsBuilder {
    table: Option<Arc<dyn Table<AccountRecord>>>,
    max_accounts: usize,
    argon2: Argon2Params,
    limits: RecordLimits,
    max_password_bytes: Limit,
}

impl fmt::Debug for PasswordAccountsBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PasswordAccountsBuilder")
            .field("max_accounts", &self.max_accounts)
            .finish_non_exhaustive()
    }
}

impl PasswordAccountsBuilder {
    /// Keeps the accounts in `table` instead of in memory.
    #[must_use]
    pub fn table(mut self, table: Arc<dyn Table<AccountRecord>>) -> Self {
        self.table = Some(table);
        self
    }

    /// The most accounts the table may hold. Default [`DEFAULT_MAX_ACCOUNTS`].
    #[must_use]
    pub fn max_accounts(mut self, max: usize) -> Self {
        self.max_accounts = max;
        self
    }

    /// How large an account's fields may be. Default [`RecordLimits::DEFAULT`].
    #[must_use]
    pub fn record_limits(mut self, limits: RecordLimits) -> Self {
        self.limits = limits;
        self
    }

    /// The longest password hashed or checked. Default [`DEFAULT_MAX_PASSWORD_BYTES`].
    #[must_use]
    pub fn max_password_bytes(mut self, most: Limit) -> Self {
        self.max_password_bytes = most;
        self
    }

    /// The Argon2id costs new passwords are hashed with, and the dummy hash is made with.
    /// Default: [`Argon2Params::default`] (19 MiB, 2 passes, 1 lane). Stored hashes keep the
    /// costs they were made with.
    #[must_use]
    pub fn argon2(mut self, params: Argon2Params) -> Self {
        self.argon2 = params;
        self
    }

    /// Loads the table and makes the provider.
    ///
    /// # Errors
    ///
    /// Returns an error when the table cannot be loaded or holds an invalid or duplicated
    /// account.
    pub fn build(self) -> Result<PasswordAccounts, SourceError> {
        let table = self
            .table
            .unwrap_or_else(|| Arc::new(MemoryTable::<AccountRecord>::new()));
        let (max, limits) = (self.max_accounts, self.limits);
        Ok(PasswordAccounts {
            accounts: Reloading::new(table, move |records| validate(records, max, &limits))?,
            hasher: PasswordHasher::new(self.argon2)
                .map_err(SourceError::invalid)?
                .max_password_bytes(self.max_password_bytes),
            limits,
        })
    }
}

/// Password accounts as an Authenticator, Directory and Accounts implementation.
///
/// The table is read again whenever it reports a change. A name that is not kept is
/// verified against a dummy hash, so an unknown name costs the same time as a wrong password.
///
/// ```
/// use coqui_access::{AccountPut, Accounts, Attempt, Authenticator, Verdict};
/// use coqui_access_password::PasswordAccounts;
///
/// let accounts = PasswordAccounts::builder().build().unwrap();
/// accounts
///     .put(AccountPut {
///         name: "alice".into(),
///         secret: Some("correct horse".into()),
///         ..AccountPut::default()
///     })
///     .unwrap();
/// let verdict = accounts
///     .authenticate(Attempt {
///         name: Some("alice".into()),
///         secret: Some("correct horse".into()),
///         ..Attempt::default()
///     })
///     .unwrap();
/// assert!(matches!(verdict, Verdict::Valid(_)));
/// ```
#[derive(Debug)]
pub struct PasswordAccounts {
    accounts: Reloading<AccountRecord>,
    hasher: PasswordHasher,
    limits: RecordLimits,
}

impl PasswordAccounts {
    /// Options for password accounts, kept in memory unless
    /// [`table`](PasswordAccountsBuilder::table) says otherwise.
    pub fn builder() -> PasswordAccountsBuilder {
        PasswordAccountsBuilder {
            table: None,
            max_accounts: DEFAULT_MAX_ACCOUNTS,
            argon2: Argon2Params::default(),
            limits: RecordLimits::DEFAULT,
            max_password_bytes: DEFAULT_MAX_PASSWORD_BYTES,
        }
    }
}

impl Authenticator for PasswordAccounts {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let (Some(name), Some(password)) = (attempt.name, attempt.secret) else {
            return Ok(Verdict::Unknown);
        };
        let account = self
            .accounts
            .read(|records| records.iter().find(|account| account.name == name).cloned())
            .map_err(|reason| reason.reading("account table"))?;
        let verified = self.hasher.verify(
            account
                .as_ref()
                .map(|account| account.password_hash.as_str()),
            &password,
        );
        Ok(match account {
            Some(account) if verified && !account.disabled => Verdict::Valid(Lookup {
                by: "id".to_owned(),
                value: account.id,
            }),
            _ => Verdict::Invalid,
        })
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description {
            name: Some("Password accounts".to_owned()),
            takes: Some(takes::PASSWORD.to_owned()),
        })
    }
}

impl Directory for PasswordAccounts {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        self.accounts
            .read(|records| {
                records
                    .iter()
                    .find(|account| {
                        !account.disabled
                            && match lookup.by.as_str() {
                                "id" => account.id == lookup.value,
                                "name" => account.name == lookup.value,
                                "email" => account.email.as_deref() == Some(&lookup.value),
                                _ => false,
                            }
                    })
                    .map(|account| Person {
                        id: account.id.clone(),
                        name: account.name.clone(),
                        display_name: account.display_name.clone(),
                        email: account.email.clone(),
                        groups: account.groups.clone(),
                        attributes: Default::default(),
                    })
            })
            .map_err(|reason| reason.reading("account table"))
    }
}

impl Accounts for PasswordAccounts {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError> {
        self.accounts
            .read(|records| {
                records
                    .iter()
                    .map(|account| AccountInfo {
                        id: account.id.clone(),
                        name: account.name.clone(),
                        groups: account.groups.clone(),
                        disabled: Some(account.disabled),
                        made_unix: account.made_unix,
                        expires_unix: None,
                    })
                    .collect()
            })
            .map_err(|reason| reason.reading("account table"))
    }

    /// A new account needs a password; an existing one keeps its password unless one is
    /// given.
    fn put(&self, account: AccountPut) -> Result<PutOutcome, ProviderError> {
        if !self.limits.allows_field(&account.name) {
            return Err(error::bad_value("an account has a name"));
        }
        if account.expires_unix.is_some() {
            return Err(error::bad_value("password accounts do not expire"));
        }
        let password = account
            .secret
            .as_deref()
            .map(|password| self.hasher.hash(password))
            .transpose()
            .map_err(|reason| error::bad_value(&reason))?;
        self.accounts
            .put_with(|records| {
                let mut entry = match records.iter().find(|entry| entry.name == account.name) {
                    Some(entry) => entry.clone(),
                    None => {
                        let Some(password) = &password else {
                            return Err(SourceError::invalid("a new account needs a password"));
                        };
                        let mut entry = AccountRecord::new(
                            secret::identifier().map_err(SourceError::unavailable)?,
                            account.name.clone(),
                            password.clone(),
                        );
                        entry.made_unix = Some(unix_now());
                        entry
                    }
                };
                if let Some(password) = password {
                    entry.password_hash = password;
                }
                if let Some(groups) = account.groups {
                    entry.groups = groups;
                }
                if let Some(disabled) = account.disabled {
                    entry.disabled = disabled;
                }
                Ok((entry, ()))
            })
            .map(|()| PutOutcome::default())
            .map_err(|reason| reason.writing("account table"))
    }

    fn remove(&self, name: String) -> Result<(), ProviderError> {
        self.accounts
            .remove(&name)
            .map_err(|reason| reason.writing("account table"))
            .and_then(|removed| {
                removed
                    .then_some(())
                    .ok_or_else(|| error::not_found("there is no such account"))
            })
    }
}

fn validate(
    accounts: &[AccountRecord],
    max: usize,
    limits: &RecordLimits,
) -> Result<(), SourceError> {
    check_keys(accounts, max)?;
    let mut ids = BTreeSet::new();
    for account in accounts {
        if !limits.allows_field(&account.id)
            || !limits.allows_field(&account.name)
            || account.name.trim().is_empty()
            || account.password_hash.len() > 1024
            || !is_password_hash(&account.password_hash)
            || !ids.insert(&account.id)
            || !limits.allows_groups(&account.groups)
            || account
                .display_name
                .as_ref()
                .is_some_and(|value| !limits.allows_display_name(value))
            || account
                .email
                .as_ref()
                .is_some_and(|value| !limits.allows_email(value))
        {
            return Err(SourceError::invalid(
                "an account entry is invalid or duplicated",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cheap() -> Argon2Params {
        Argon2Params::new(64, 1, 1).unwrap()
    }

    fn hash_of(password: &str) -> Result<String, String> {
        PasswordHasher::new(cheap())?.hash(password)
    }

    fn attempt(name: &str, password: &str) -> Attempt {
        Attempt {
            name: Some(name.to_owned()),
            secret: Some(password.to_owned()),
            ..Attempt::default()
        }
    }

    fn named(name: &str) -> AccountPut {
        AccountPut {
            name: name.to_owned(),
            ..AccountPut::default()
        }
    }

    fn with_accounts(
        records: impl IntoIterator<Item = AccountRecord>,
    ) -> (Arc<MemoryTable<AccountRecord>>, PasswordAccounts) {
        let table = Arc::new(MemoryTable::from_records(records));
        let accounts = PasswordAccounts::builder()
            .table(table.clone())
            .argon2(cheap())
            .build()
            .unwrap();
        (table, accounts)
    }

    #[test]
    fn authenticates_and_resolves_accounts_without_exposing_password_hashes() {
        let password_hash = hash_of("correct horse battery staple").unwrap();
        let (_, accounts) =
            with_accounts([
                AccountRecord::new("person-a", "alice", password_hash.clone()).groups(["field"]),
            ]);
        let Verdict::Valid(lookup) = accounts
            .authenticate(attempt("alice", "correct horse battery staple"))
            .unwrap()
        else {
            panic!("password should be valid");
        };
        assert_eq!(lookup.by, "id");
        let person = accounts.lookup(lookup).unwrap().unwrap();
        assert_eq!(person.id, "person-a");
        assert_eq!(person.groups, vec!["field".to_owned()]);
        assert!(!format!("{accounts:?}").contains(&password_hash));
    }

    #[test]
    fn wrong_unknown_and_disabled_credentials_do_not_authenticate() {
        let password_hash = hash_of("password").unwrap();
        let (_, accounts) =
            with_accounts([AccountRecord::new("person-a", "a", password_hash).disabled(true)]);
        assert!(
            accounts
                .lookup(Lookup {
                    by: "id".to_owned(),
                    value: "person-a".to_owned(),
                })
                .unwrap()
                .is_none()
        );
        for name in ["a", "missing"] {
            assert_eq!(
                accounts.authenticate(attempt(name, "password")).unwrap(),
                Verdict::Invalid
            );
        }
        assert_eq!(
            accounts.authenticate(Attempt::default()).unwrap(),
            Verdict::Unknown
        );
    }

    #[test]
    fn an_unknown_name_costs_a_hash_check_like_a_wrong_password() {
        let (_, accounts) = with_accounts([]);
        assert!(!accounts.hasher.verify(None, "anything"));
        assert_eq!(
            accounts
                .authenticate(attempt("nobody", "anything"))
                .unwrap(),
            Verdict::Invalid
        );
    }

    #[test]
    fn invalid_duplicated_and_oversized_tables_are_refused() {
        let hash = hash_of("pw").unwrap();
        let build = |records: Vec<AccountRecord>, max: usize| {
            PasswordAccounts::builder()
                .table(Arc::new(MemoryTable::from_records(records)))
                .max_accounts(max)
                .build()
        };
        assert!(build(vec![AccountRecord::new("1", "a", "not a hash")], 10).is_err());
        assert!(
            build(
                vec![
                    AccountRecord::new("1", "a", hash.clone()),
                    AccountRecord::new("1", "b", hash.clone()),
                ],
                10
            )
            .is_err(),
            "an id is not shared"
        );
        assert!(
            build(
                vec![
                    AccountRecord::new("1", "a", hash.clone()),
                    AccountRecord::new("2", "b", hash.clone()),
                ],
                1
            )
            .is_err()
        );
        assert!(build(vec![AccountRecord::new("1", "a", hash)], 1).is_ok());
    }

    #[test]
    fn accounts_are_made_changed_and_removed_through_the_kind() {
        let (table, accounts) = with_accounts([]);
        assert!(
            accounts.put(named("alice")).is_err(),
            "a new account needs a password"
        );
        accounts
            .put(AccountPut {
                secret: Some("pw one".to_owned()),
                groups: Some(vec!["staff".to_owned()]),
                ..named("alice")
            })
            .unwrap();
        assert!(matches!(
            accounts.authenticate(attempt("alice", "pw one")).unwrap(),
            Verdict::Valid(_)
        ));
        // Changing groups leaves the password; giving a password replaces it.
        accounts
            .put(AccountPut {
                groups: Some(vec!["admin".to_owned()]),
                ..named("alice")
            })
            .unwrap();
        assert!(matches!(
            accounts.authenticate(attempt("alice", "pw one")).unwrap(),
            Verdict::Valid(_)
        ));
        accounts
            .put(AccountPut {
                secret: Some("pw two".to_owned()),
                ..named("alice")
            })
            .unwrap();
        assert_eq!(
            accounts.authenticate(attempt("alice", "pw one")).unwrap(),
            Verdict::Invalid
        );
        let listed = accounts.accounts().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].groups, ["admin"]);
        let kept = table.load().unwrap().records;
        assert!(
            kept[0]
                .password_hash
                .starts_with("$argon2id$v=19$m=64,t=1,p=1$")
        );
        assert!(
            accounts
                .put(AccountPut {
                    expires_unix: Some(1),
                    ..named("alice")
                })
                .is_err()
        );
        accounts.remove("alice".to_owned()).unwrap();
        assert!(accounts.remove("alice".to_owned()).is_err());
    }

    #[test]
    fn a_write_through_another_handle_is_seen_without_rebuilding() {
        let (table, accounts) = with_accounts([]);
        let other = PasswordAccounts::builder()
            .table(table)
            .argon2(cheap())
            .build()
            .unwrap();
        other
            .put(AccountPut {
                secret: Some("x".to_owned()),
                ..named("bob")
            })
            .unwrap();
        assert_eq!(accounts.accounts().unwrap().len(), 1);
    }

    #[test]
    fn a_read_only_table_refuses_changes_but_authenticates() {
        struct Fixed(Vec<AccountRecord>);
        impl Table<AccountRecord> for Fixed {
            fn load(&self) -> Result<coqui_access_kit::Snapshot<AccountRecord>, SourceError> {
                Ok(coqui_access_kit::Snapshot::new(
                    coqui_access_kit::Revision::default(),
                    self.0.clone(),
                ))
            }
        }
        let hash = hash_of("pw").unwrap();
        let accounts = PasswordAccounts::builder()
            .table(Arc::new(Fixed(vec![AccountRecord::new("1", "a", hash)])))
            .build()
            .unwrap();
        assert!(matches!(
            accounts.authenticate(attempt("a", "pw")).unwrap(),
            Verdict::Valid(_)
        ));
        assert!(
            accounts
                .put(AccountPut {
                    secret: Some("new".to_owned()),
                    ..named("b")
                })
                .is_err()
        );
        assert!(accounts.remove("a".to_owned()).is_err());
    }
}
