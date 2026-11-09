//! `coqui-access-key` as a library a host loads: the Guatiao entry point and the config that
//! builds the provider.
//!
//! The config names where keys come from in `source`, a file in `toml` or `json`:
//!
//! ```json
//! { "source": { "file": { "path": "keys.toml", "format": "toml" } },
//!   "prefix": "ck_",
//!   "key_hash": { "algorithm": "argon2id", "memory_kib": 19456, "iterations": 2 },
//!   "max_keys": 10000 }
//! ```
//!
//! Every key is optional. With no `source` the keys live in memory. `key_hash` is how keys
//! made from now on are kept: `{ "algorithm": "sha256" }` (the default) or `argon2id` with
//! its costs beside it, each defaulting. Keys already in the table keep the algorithm their
//! stored string names.

mod format;

use std::sync::Arc;

use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, Provider, Schema, ToValue};
use coqui_access::{
    AccountInfo, AccountPut, Accounts, Attempt, Authenticator, Description, Directory, Lookup,
    Person, PutOutcome, Verdict,
};
use coqui_access_key::{DEFAULT_MAX_KEYS, DEFAULT_PREFIX, KeyRecord, Keys};
use coqui_access_kit::file::Source;
use coqui_access_kit::{KeyHash, MemoryTable, RecordLimits, Table};

pub use format::Format;

/// Guatiao configuration for the key provider.
#[derive(Debug, Clone, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao)]
pub struct KeysConfig {
    /// Where the keys are kept; memory when absent.
    pub source: Option<Source<Format>>,
    /// What every key made here begins with.
    #[map(default = String::from(DEFAULT_PREFIX))]
    pub prefix: String,
    /// How keys made from now on are kept; SHA-256 by default.
    #[map(default)]
    pub key_hash: KeyHash,
    /// The most keys the table may hold.
    #[map(default = DEFAULT_MAX_KEYS as u32)]
    pub max_keys: u32,
    /// How large a record's fields may be: `groups`, `field_bytes`, `email_bytes`,
    /// `display_name_bytes`, each a number or `null` for no bound.
    #[map(default)]
    pub record_limits: RecordLimits,
}

impl Default for KeysConfig {
    /// What an empty configuration reads as: every default the schema declares.
    fn default() -> Self {
        Self::from_value(&coqui_access::guatiao::Map::new().into())
            .expect("an empty configuration reads, every key having a default")
    }
}

/// The key provider a host loads.
#[derive(Debug, Provider)]
#[provider(
    Authenticator,
    Directory,
    Accounts,
    id = "coqui_access_key",
    name = "Bearer keys",
    config = KeysConfig,
    crate = ::coqui_access::guatiao
)]
pub struct KeysProvider(Keys);

impl TryFrom<KeysConfig> for KeysProvider {
    type Error = ProviderError;

    fn try_from(config: KeysConfig) -> Result<Self, Self::Error> {
        let table: Arc<dyn Table<KeyRecord>> = match &config.source {
            Some(source) => source
                .open("keys")
                .map_err(|reason| reason.writing("key table"))?,
            None => Arc::new(MemoryTable::new()),
        };
        let builder = Keys::builder()
            .table(table)
            .prefix(config.prefix)
            .key_hash(config.key_hash);
        builder
            .max_keys(config.max_keys as usize)
            .record_limits(config.record_limits)
            .build()
            .map(Self)
            .map_err(|reason| reason.writing("key table"))
    }
}

impl Authenticator for KeysProvider {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        self.0.authenticate(attempt)
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Authenticator::describe(&self.0)
    }
}

impl Directory for KeysProvider {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        self.0.lookup(lookup)
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Directory::describe(&self.0)
    }
}

impl Accounts for KeysProvider {
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

coqui_access::guatiao::providers!(KeysProvider);

#[cfg(test)]
mod tests {
    use super::*;

    fn config(path: &std::path::Path) -> KeysConfig {
        KeysConfig {
            source: Some(Source::file(path.to_str().unwrap(), Format::Toml)),
            prefix: DEFAULT_PREFIX.to_owned(),
            ..KeysConfig::default()
        }
    }

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

    #[test]
    fn a_file_source_keeps_keys_and_a_hand_written_file_is_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.toml");
        let provider = KeysProvider::try_from(config(&path)).unwrap();
        let key = provider.put(named("cabin")).unwrap().secret.unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[keys]]") && text.contains("hash = \"sha256:"));
        assert!(!text.contains(key.trim_start_matches("ck_")));
        // A second provider over the same file knows the key, and a hand-written
        // record is taken too.
        let other = KeysProvider::try_from(config(&path)).unwrap();
        assert!(matches!(
            other.authenticate(presented(&key)).unwrap(),
            Verdict::Valid(_)
        ));
        let relay = other.0.new_key().unwrap();
        let digest = KeyHash::Sha256.hash(&relay).unwrap();
        let mut kept = std::fs::read_to_string(&path).unwrap();
        kept.push_str(&format!(
            "\n[[keys]]\nid = 'robot-1'\nname = 'relay'\nhash = '{digest}'\n"
        ));
        std::fs::write(&path, kept).unwrap();
        assert!(matches!(
            provider.authenticate(presented(&relay)).unwrap(),
            Verdict::Valid(_)
        ));
    }

    #[test]
    fn argon2id_keys_are_chosen_in_the_config_and_mix_with_sha256() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.toml");
        let fast = KeysProvider::try_from(config(&path)).unwrap();
        let slow = KeysProvider::try_from(KeysConfig {
            key_hash: KeyHash::Argon2id(coqui_access_kit::Argon2Params::new(64, 1, 1).unwrap()),
            ..config(&path)
        })
        .unwrap();
        let sha_key = fast.put(named("fast")).unwrap().secret.unwrap();
        let argon_key = slow.put(named("slow")).unwrap().secret.unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("sha256:") && text.contains("$argon2id$v=19$m=64,t=1,p=1$"));
        for key in [&sha_key, &argon_key] {
            assert!(matches!(
                fast.authenticate(presented(key)).unwrap(),
                Verdict::Valid(_)
            ));
        }
    }

    #[test]
    fn a_bad_file_prefix_or_costs_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("keys.toml");
        std::fs::write(&path, "[[keys]]\nid='a'\nname='a'\nhash='short'\n").unwrap();
        assert!(KeysProvider::try_from(config(&path)).is_err());
        let blank = KeysConfig {
            prefix: String::new(),
            ..KeysConfig::default()
        };
        assert!(KeysProvider::try_from(blank).is_err(), "an empty prefix");
        assert!(
            config_from(r#"{"key_hash": {"algorithm": "argon2id", "iterations": 0}}"#).is_err(),
            "Argon2 refuses the costs"
        );
    }

    fn config_from(json: &str) -> Result<KeysConfig, coqui_access::guatiao::MapError> {
        let value = guatiao_serde::from_serde(&mut serde_json::Deserializer::from_str(json));
        KeysConfig::from_value(&value.unwrap())
    }

    #[test]
    fn the_key_hash_is_tagged_and_defaults_to_sha256() {
        assert_eq!(config_from("{}").unwrap().key_hash, KeyHash::Sha256);
        let tagged = r#"{"key_hash": {"algorithm": "argon2id", "memory_kib": 65536}}"#;
        let KeyHash::Argon2id(costs) = config_from(tagged).unwrap().key_hash else {
            panic!("an argon2id key hash");
        };
        assert_eq!(costs.memory_kib(), 65536);
        assert_eq!(
            costs.iterations(),
            coqui_access_kit::Argon2Params::default().iterations()
        );
        assert!(
            config_from(r#"{"key_hash": "sha256"}"#).is_err(),
            "a map, not a name"
        );
    }

    #[test]
    fn no_source_is_memory_and_the_prefix_defaults_when_read() {
        let provider = KeysProvider::try_from(KeysConfig {
            prefix: DEFAULT_PREFIX.to_owned(),
            ..KeysConfig::default()
        })
        .unwrap();
        let key = provider.put(named("cabin")).unwrap().secret.unwrap();
        assert!(key.starts_with("ck_"));
        let bare = coqui_access::guatiao::Map::new();
        assert!(KeysConfig::from_value(&bare.into()).is_ok_and(|c| c.prefix == "ck_"));
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
        let schema = KeysConfig::schema().unwrap();
        assert_eq!(default_in(&schema, "max_keys"), Some(10_000));
        let empty = KeysConfig::default();
        assert_eq!(
            (empty.max_keys, empty.prefix.as_str()),
            (10_000, DEFAULT_PREFIX)
        );
    }

    #[test]
    fn record_limits_read_from_configuration_and_bind_the_records() {
        let mut limits = coqui_access::guatiao::Map::new();
        limits.set("field_bytes", 40u32);
        let mut config = coqui_access::guatiao::Map::new();
        config.set("record_limits", limits);
        let read = KeysConfig::from_value(&config.into()).unwrap();
        assert_eq!(read.record_limits.field_bytes, coqui_access::Limit::at(40));
        assert_eq!(read.record_limits.groups, coqui_access::Limit::at(256));
        let provider = KeysProvider::try_from(read).unwrap();
        assert!(
            provider.put(named(&"x".repeat(41))).is_err(),
            "a name past field_bytes"
        );
        assert!(provider.put(named("abc")).is_ok());
    }
}
