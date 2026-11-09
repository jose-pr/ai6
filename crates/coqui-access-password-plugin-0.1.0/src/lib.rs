//! `coqui-access-password` as a library a host loads: the Guatiao entry point and the config that
//! builds the provider.
//!
//! The config names where accounts come from in `source`, a file in `toml` or `json`:
//!
//! ```json
//! { "source": { "file": { "path": "accounts.toml", "format": "toml" } },
//!   "max_accounts": 10000,
//!   "argon2": { "memory_kib": 19456, "iterations": 2, "parallelism": 1 } }
//! ```
//!
//! Every key is optional. With no `source` the accounts live in memory.

mod format;

use std::sync::Arc;

use coqui_access::Limit;
use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, Provider, Schema, ToValue};
use coqui_access::{
    AccountInfo, AccountPut, Accounts, Attempt, Authenticator, Description, Directory, Lookup,
    Person, PutOutcome, Verdict,
};
use coqui_access_kit::file::Source;
use coqui_access_kit::{
    Argon2Params, DEFAULT_MAX_PASSWORD_BYTES, MemoryTable, RecordLimits, Table,
};
use coqui_access_password::{AccountRecord, DEFAULT_MAX_ACCOUNTS, PasswordAccounts};

pub use format::Format;

/// Guatiao configuration for the password accounts provider.
#[derive(Debug, Clone, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao)]
pub struct PasswordAccountsConfig {
    /// Where the accounts are kept; memory when absent.
    pub source: Option<Source<Format>>,
    /// The most accounts the table may hold.
    #[map(default = DEFAULT_MAX_ACCOUNTS as u32)]
    pub max_accounts: u32,
    /// How large a record's fields may be: `groups`, `field_bytes`, `email_bytes`,
    /// `display_name_bytes`, each a number or `null` for no bound.
    #[map(default)]
    pub record_limits: RecordLimits,
    /// The longest password hashed or checked, in bytes; `null` for no bound.
    #[map(default = DEFAULT_MAX_PASSWORD_BYTES)]
    pub max_password_bytes: Limit,
    /// Argon2id costs for new passwords; the library default for each one absent.
    #[map(default)]
    pub argon2: Argon2Params,
}

impl Default for PasswordAccountsConfig {
    /// What an empty configuration reads as: every default the schema declares.
    fn default() -> Self {
        Self::from_value(&coqui_access::guatiao::Map::new().into())
            .expect("an empty configuration reads, every key having a default")
    }
}

/// The password accounts provider a host loads.
#[derive(Debug, Provider)]
#[provider(
    Authenticator,
    Directory,
    Accounts,
    id = "coqui_access_password",
    name = "Password accounts",
    config = PasswordAccountsConfig,
    crate = ::coqui_access::guatiao
)]
pub struct PasswordAccountsProvider(PasswordAccounts);

impl TryFrom<PasswordAccountsConfig> for PasswordAccountsProvider {
    type Error = ProviderError;

    fn try_from(config: PasswordAccountsConfig) -> Result<Self, Self::Error> {
        let table: Arc<dyn Table<AccountRecord>> = match &config.source {
            Some(source) => source
                .open("accounts")
                .map_err(|reason| reason.writing("account table"))?,
            None => Arc::new(MemoryTable::new()),
        };
        PasswordAccounts::builder()
            .table(table)
            .max_accounts(config.max_accounts as usize)
            .record_limits(config.record_limits)
            .max_password_bytes(config.max_password_bytes)
            .argon2(config.argon2)
            .build()
            .map(Self)
            .map_err(|reason| reason.writing("account table"))
    }
}

impl Authenticator for PasswordAccountsProvider {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        self.0.authenticate(attempt)
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Authenticator::describe(&self.0)
    }
}

impl Directory for PasswordAccountsProvider {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        self.0.lookup(lookup)
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Directory::describe(&self.0)
    }
}

impl Accounts for PasswordAccountsProvider {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError> {
        self.0.accounts()
    }

    fn put(&self, account: AccountPut) -> Result<PutOutcome, ProviderError> {
        self.0.put(account)
    }

    fn remove(&self, name: String) -> Result<(), ProviderError> {
        self.0.remove(name)
    }
}

coqui_access::guatiao::providers!(PasswordAccountsProvider);

#[cfg(test)]
mod tests {
    use coqui_access_kit::PasswordHasher;

    use super::*;

    fn cheap() -> Argon2Params {
        Argon2Params::new(64, 1, 1).unwrap()
    }

    fn config(path: &std::path::Path) -> PasswordAccountsConfig {
        config_in(path, Format::Toml)
    }

    fn config_in(path: &std::path::Path, format: Format) -> PasswordAccountsConfig {
        PasswordAccountsConfig {
            source: Some(Source::file(path.to_str().unwrap(), format)),
            argon2: cheap(),
            ..PasswordAccountsConfig::default()
        }
    }

    fn attempt(name: &str, password: &str) -> Attempt {
        Attempt {
            name: Some(name.to_owned()),
            secret: Some(password.to_owned()),
            ..Attempt::default()
        }
    }

    #[test]
    fn a_file_source_is_read_written_and_followed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts.toml");
        let hash = PasswordHasher::new(cheap())
            .unwrap()
            .hash("correct horse")
            .unwrap();
        std::fs::write(
            &path,
            format!(
                "[[accounts]]\nid='person-a'\nname='alice'\npassword_hash='{hash}'\ngroups=['field']\n"
            ),
        )
        .unwrap();
        let provider = PasswordAccountsProvider::try_from(config(&path)).unwrap();
        let Verdict::Valid(lookup) = provider
            .authenticate(attempt("alice", "correct horse"))
            .unwrap()
        else {
            panic!("the account in the file should authenticate");
        };
        assert_eq!(provider.lookup(lookup).unwrap().unwrap().groups, ["field"]);

        // A put goes to the file, and another provider over the file sees it.
        provider
            .put(AccountPut {
                name: "bob".to_owned(),
                secret: Some("pw".to_owned()),
                ..AccountPut::default()
            })
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("name = \"bob\"") && !text.contains("\"pw\""));
        let other = PasswordAccountsProvider::try_from(config(&path)).unwrap();
        assert!(matches!(
            other.authenticate(attempt("bob", "pw")).unwrap(),
            Verdict::Valid(_)
        ));
        other.remove("bob".to_owned()).unwrap();
        assert_eq!(provider.accounts().unwrap().len(), 1);
    }

    #[test]
    fn a_bad_file_or_source_is_refused_and_no_source_is_memory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts.toml");
        std::fs::write(&path, "accounts = [ { unknown = true } ]").unwrap();
        assert!(PasswordAccountsProvider::try_from(config(&path)).is_err());
        std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).unwrap();
        assert!(PasswordAccountsProvider::try_from(config(&path)).is_err());
        let empty = PasswordAccountsConfig {
            source: Some(Source { file: None }),
            ..PasswordAccountsConfig::default()
        };
        assert!(PasswordAccountsProvider::try_from(empty).is_err());
        let memory = PasswordAccountsProvider::try_from(PasswordAccountsConfig {
            argon2: cheap(),
            ..PasswordAccountsConfig::default()
        })
        .unwrap();
        assert!(memory.accounts().unwrap().is_empty());
    }

    #[test]
    fn the_config_reads_from_a_guatiao_value() {
        let config = PasswordAccountsConfig::from_value(
            &PasswordAccountsConfig {
                source: Some(Source::file("accounts.toml", Format::Toml)),
                max_accounts: 5,
                ..PasswordAccountsConfig::default()
            }
            .to_value()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(config.max_accounts, 5);
        assert_eq!(config.source.unwrap().file.unwrap().path, "accounts.toml");
    }

    #[test]
    fn a_json_source_holds_the_same_accounts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("accounts.json");
        let provider = PasswordAccountsProvider::try_from(config_in(&path, Format::Json)).unwrap();
        provider
            .put(AccountPut {
                name: "carol".to_owned(),
                secret: Some("pw".to_owned()),
                groups: Some(vec!["staff".to_owned()]),
                ..AccountPut::default()
            })
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.trim_start().starts_with('{') && text.contains("\"carol\""));
        let other = PasswordAccountsProvider::try_from(config_in(&path, Format::Json)).unwrap();
        assert!(matches!(
            other.authenticate(attempt("carol", "pw")).unwrap(),
            Verdict::Valid(_)
        ));
        assert!(
            PasswordAccountsProvider::try_from(config(&path)).is_err(),
            "JSON is not TOML"
        );
    }
    fn with_password(name: &str) -> AccountPut {
        AccountPut {
            name: name.to_owned(),
            secret: Some("pw".to_owned()),
            ..AccountPut::default()
        }
    }

    /// The `default` a schema declares for one of its properties.
    fn default_in(schema: &coqui_access::guatiao::Value, field: &str) -> Option<u64> {
        use coqui_access::guatiao::Map;
        let schema = <&Map>::try_from(schema).ok()?;
        let properties = <&Map>::try_from(schema.get("properties")?).ok()?;
        let property = <&Map>::try_from(properties.get(field)?).ok()?;
        u64::from_value(property.get("default")?).ok()
    }

    #[test]
    fn the_schema_declares_the_defaults_an_empty_config_reads_as() {
        let schema = PasswordAccountsConfig::schema().unwrap();
        assert_eq!(default_in(&schema, "max_accounts"), Some(10_000));
        assert_eq!(default_in(&schema, "max_password_bytes"), Some(4096));
        let empty = PasswordAccountsConfig::default();
        assert_eq!(empty.max_accounts, 10_000);
        assert_eq!(empty.argon2, Argon2Params::default());
    }

    #[test]
    fn record_limits_read_from_configuration_and_bind_the_records() {
        let mut limits = coqui_access::guatiao::Map::new();
        limits.set("field_bytes", 40u32);
        let mut config = coqui_access::guatiao::Map::new();
        config.set("record_limits", limits);
        let read = PasswordAccountsConfig::from_value(&config.into()).unwrap();
        assert_eq!(read.record_limits.field_bytes, coqui_access::Limit::at(40));
        assert_eq!(read.record_limits.groups, coqui_access::Limit::at(256));
        let provider = PasswordAccountsProvider::try_from(PasswordAccountsConfig {
            argon2: cheap(),
            ..read
        })
        .unwrap();
        assert!(
            provider.put(with_password(&"x".repeat(41))).is_err(),
            "a name past field_bytes"
        );
        assert!(provider.put(with_password("abc")).is_ok());
    }
}
