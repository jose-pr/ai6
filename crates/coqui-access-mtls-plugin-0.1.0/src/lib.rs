//! `coqui-access-mtls` as a library a host loads: the Guatiao entry point and the config that
//! builds the provider.
//!
//! The config names where peers come from in `source`, a file in `toml` or `json`:
//!
//! ```json
//! { "source": { "file": { "path": "peers.toml", "format": "toml" } },
//!   "waiting": { "file": { "path": "peers.toml.waiting", "format": "toml" } },
//!   "max_records": 10000,
//!   "waiting_per_source": 1, "waiting_total": 16, "waiting_rerecord_secs": 60 }
//! ```
//!
//! Every key is optional. With no `source` the peers live in memory. `waiting`, where the
//! fingerprints nobody accepted are kept, defaults to a file beside a file source
//! (`<path>.waiting`) and to memory otherwise.
//!
//! Certificates a CA signed are trusted with `"trust": "ca"` or `"pinned-or-ca"`, a `ca_file`
//! and optional `subject` and `san` globs; `account` picks the field that names the account:
//!
//! ```json
//! { "trust": "ca", "ca_file": "clients-ca.pem", "san": ["email:*@example.org"],
//!   "account": [ { "field": "san.email", "by": "email" } ] }
//! ```

mod format;

use std::sync::Arc;
use std::time::Duration;

use coqui_access::Limit;
use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, FromWalk, Provider, Schema, ToValue, Walk};
use coqui_access::{
    AccountInfo, AccountPut, Accounts, Attempt, Authenticator, Description, Directory, Lookup,
    Person, PutOutcome, Verdict, Waiting,
};
use coqui_access_kit::file::Source;
use coqui_access_kit::pick::Pick;
use coqui_access_kit::{Keyed, MemoryTable, RecordLimits, Table};
use coqui_access_mtls::{
    DEFAULT_MAX_CHAIN, DEFAULT_MAX_RECORDS, DEFAULT_WAITING_PER_SOURCE, DEFAULT_WAITING_RERECORD,
    DEFAULT_WAITING_TOTAL, MtlsAccounts, PeerRecord, TrustMode, Unaccepted, default_account,
};
use coqui_peer::WaitingLimits;
use coqui_peer_tls::CaPolicy;

pub use format::Format;

/// Guatiao configuration for the verified peer provider.
#[derive(Debug, Clone, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao)]
pub struct MtlsAccountsConfig {
    /// Where the accepted peers are kept; memory when absent.
    pub source: Option<Source<Format>>,
    /// Where the fingerprints waiting to be accepted are kept.
    pub waiting: Option<Source<Format>>,
    /// The most peers the table may hold.
    #[map(default = DEFAULT_MAX_RECORDS as u32)]
    pub max_records: u32,
    /// How large a record's fields may be: `groups`, `field_bytes`, `email_bytes`,
    /// `display_name_bytes`, each a number or `null` for no bound.
    #[map(default)]
    pub record_limits: RecordLimits,
    /// Waiting places per source: an IPv4 address, or an IPv6 /64.
    #[map(default = DEFAULT_WAITING_PER_SOURCE as u32)]
    pub waiting_per_source: u32,
    /// Waiting places in all; past them a newcomer is not written down.
    #[map(default = DEFAULT_WAITING_TOTAL as u32)]
    pub waiting_total: u32,
    /// Seconds before one fingerprint from one source is written down again.
    #[map(default = DEFAULT_WAITING_RERECORD.as_secs())]
    pub waiting_rerecord_secs: u64,
    /// How a certificate is trusted: `pinned` (a record of the table), `ca`, or
    /// `pinned-or-ca`.
    #[map(default)]
    pub trust: TrustMode,
    /// The PEM file of CA certificates `ca` and `pinned-or-ca` trust.
    pub ca_file: Option<String>,
    /// Globs the certificate's subject must match; empty matches any.
    #[map(default)]
    pub subject: Vec<String>,
    /// Globs one of its subject alternative names must match (`dns:`, `ip:`, `email:`,
    /// `uri:`, `upn:` limit one to a kind); empty matches any.
    #[map(default)]
    pub san: Vec<String>,
    /// Whether an IP address written as a DNS name matches an `ip:` glob. Default `true`.
    #[map(default = true)]
    pub ip_in_dns: bool,
    /// How a CA-trusted certificate names its account: picks tried in order. Default its
    /// `fingerprint`, by `certificate`.
    #[map(default = default_account())]
    pub account: Vec<Pick>,
    /// The most certificates a client may send, leaf included; `null` for no bound.
    #[map(default = DEFAULT_MAX_CHAIN)]
    pub max_chain: Limit,
}

impl Default for MtlsAccountsConfig {
    /// What an empty configuration reads as: every default the schema declares.
    fn default() -> Self {
        Self::from_value(&coqui_access::guatiao::Map::new().into())
            .expect("an empty configuration reads, every key having a default")
    }
}

/// The table `source` names, or one in memory.
fn open<T>(
    source: Option<&Source<Format>>,
    section: &str,
) -> Result<Arc<dyn Table<T>>, ProviderError>
where
    T: Keyed + Clone + Send + Walk + FromWalk + 'static,
{
    match source {
        Some(source) => source
            .open(section)
            .map_err(|reason| reason.writing("peer table")),
        None => Ok(Arc::new(MemoryTable::new())),
    }
}

/// The peer provider a host loads.
#[derive(Debug, Provider)]
#[provider(
    Authenticator,
    Directory,
    Accounts,
    id = "coqui_access_mtls",
    name = "Client certificates",
    config = MtlsAccountsConfig,
    crate = ::coqui_access::guatiao
)]
pub struct MtlsAccountsProvider(MtlsAccounts);

impl TryFrom<MtlsAccountsConfig> for MtlsAccountsProvider {
    type Error = ProviderError;

    fn try_from(config: MtlsAccountsConfig) -> Result<Self, Self::Error> {
        let waiting_source = config.waiting.clone().or_else(|| {
            config
                .source
                .as_ref()
                .map(|source| source.sibling("waiting"))
        });
        let mut builder = MtlsAccounts::builder()
            .table(open::<PeerRecord>(config.source.as_ref(), "peers")?)
            .waiting(open::<Unaccepted>(waiting_source.as_ref(), "waiting")?);
        builder = builder
            .max_records(config.max_records as usize)
            .record_limits(config.record_limits);
        let mut limits = WaitingLimits::default();
        limits.per_source = config.waiting_per_source as usize;
        limits.total = config.waiting_total as usize;
        limits.rerecord = Duration::from_secs(config.waiting_rerecord_secs);
        builder = builder.waiting_limits(limits);
        builder = builder
            .trust(config.trust)
            .account(config.account)
            .max_chain(config.max_chain);
        if let Some(file) = config.ca_file {
            let mut ca = CaPolicy::new(file);
            ca.subject = config.subject;
            ca.san = config.san;
            ca.ip_in_dns = config.ip_in_dns;
            builder = builder.ca(ca);
        }
        builder
            .build()
            .map(Self)
            .map_err(|reason| reason.writing("peer table"))
    }
}

impl Authenticator for MtlsAccountsProvider {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        self.0.authenticate(attempt)
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Authenticator::describe(&self.0)
    }
}

impl Directory for MtlsAccountsProvider {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        self.0.lookup(lookup)
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Directory::describe(&self.0)
    }
}

impl Accounts for MtlsAccountsProvider {
    fn accounts(&self) -> Result<Vec<AccountInfo>, ProviderError> {
        self.0.accounts()
    }

    fn put(&self, account: AccountPut) -> Result<PutOutcome, ProviderError> {
        self.0.put(account)
    }

    fn remove(&self, name: String) -> Result<(), ProviderError> {
        self.0.remove(name)
    }

    fn waiting(&self) -> Result<Vec<Waiting>, ProviderError> {
        self.0.waiting()
    }
}

coqui_access::guatiao::providers!(MtlsAccountsProvider);

#[cfg(test)]
mod tests {
    use coqui_peer_tls::Identity;

    use super::*;

    fn config(path: &std::path::Path) -> MtlsAccountsConfig {
        MtlsAccountsConfig {
            source: Some(Source::file(path.to_str().unwrap(), Format::Toml)),
            ..MtlsAccountsConfig::default()
        }
    }

    fn shown(certificate: &str) -> Attempt {
        Attempt {
            certificate: Some(certificate.to_owned()),
            peer: Some("192.0.2.7:4000".to_owned()),
            ..Attempt::default()
        }
    }

    #[test]
    fn a_file_source_keeps_peers_and_the_waiting_list_beside_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("peers.toml");
        let provider = MtlsAccountsProvider::try_from(config(&path)).unwrap();
        let certificate = Identity::generate().unwrap().fingerprint();
        let text = certificate.to_string();
        assert_eq!(
            provider.authenticate(shown(&text)).unwrap(),
            Verdict::Unknown
        );
        let waiting_file = directory.path().join("peers.toml.waiting");
        assert!(
            std::fs::read_to_string(&waiting_file)
                .unwrap()
                .contains("[[waiting]]")
        );
        let waiting = provider.waiting().unwrap();
        assert_eq!(waiting.len(), 1);

        provider
            .put(AccountPut {
                name: "site-a".to_owned(),
                secret: Some(waiting[0].shown.clone()),
                ..AccountPut::default()
            })
            .unwrap();
        assert!(provider.waiting().unwrap().is_empty());
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("[[peers]]")
        );
        let other = MtlsAccountsProvider::try_from(config(&path)).unwrap();
        assert!(matches!(
            other.authenticate(shown(&text)).unwrap(),
            Verdict::Valid(_)
        ));
    }

    #[test]
    fn a_hand_written_file_is_read_and_a_bad_one_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("peers.toml");
        let certificate = Identity::generate().unwrap().fingerprint().to_string();
        std::fs::write(
            &path,
            format!(
                "[[peers]]\nid='service-1'\nname='site-a'\ncertificate='{certificate}'\ngroups=['site-a']\n"
            ),
        )
        .unwrap();
        let provider = MtlsAccountsProvider::try_from(config(&path)).unwrap();
        assert!(matches!(
            provider.authenticate(shown(&certificate)).unwrap(),
            Verdict::Valid(_)
        ));
        std::fs::write(
            &path,
            "[[peers]]\nid='service-1'\nname='site-a'\ncertificate='bad'\n",
        )
        .unwrap();
        assert!(MtlsAccountsProvider::try_from(config(&path)).is_err());
    }

    #[test]
    fn waiting_can_be_a_separate_source_or_memory() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("peers.toml");
        let apart = directory.path().join("seen.toml");
        let provider = MtlsAccountsProvider::try_from(MtlsAccountsConfig {
            waiting: Some(Source::file(apart.to_str().unwrap(), Format::Toml)),
            ..config(&path)
        })
        .unwrap();
        let text = Identity::generate().unwrap().fingerprint().to_string();
        provider.authenticate(shown(&text)).unwrap();
        assert!(apart.exists());
        assert!(!directory.path().join("peers.toml.waiting").exists());
        // No source at all: both in memory.
        let memory = MtlsAccountsProvider::try_from(MtlsAccountsConfig::default()).unwrap();
        memory.authenticate(shown(&text)).unwrap();
        assert_eq!(memory.waiting().unwrap().len(), 1);
    }

    fn fingerprinted(name: &str) -> AccountPut {
        AccountPut {
            name: name.to_owned(),
            secret: Some(Identity::generate().unwrap().fingerprint().to_string()),
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
    fn the_waiting_limits_default_in_the_schema_and_when_read() {
        let read =
            MtlsAccountsConfig::from_value(&coqui_access::guatiao::Map::new().into()).unwrap();
        assert_eq!(
            (
                read.waiting_per_source,
                read.waiting_total,
                read.waiting_rerecord_secs
            ),
            (1, 16, 60)
        );
        let defaults = MtlsAccountsConfig::default();
        assert_eq!(read.waiting_total, defaults.waiting_total);
        let schema = MtlsAccountsConfig::schema().unwrap();
        for (field, expected) in [
            ("waiting_per_source", 1),
            ("waiting_total", 16),
            ("waiting_rerecord_secs", 60),
            ("max_records", 10_000),
        ] {
            assert_eq!(default_in(&schema, field), Some(expected), "{field}");
        }
    }

    #[test]
    fn record_limits_read_from_configuration_and_bind_the_records() {
        let mut limits = coqui_access::guatiao::Map::new();
        limits.set("field_bytes", 40u32);
        let mut config = coqui_access::guatiao::Map::new();
        config.set("record_limits", limits);
        let read = MtlsAccountsConfig::from_value(&config.into()).unwrap();
        assert_eq!(read.record_limits.field_bytes, coqui_access::Limit::at(40));
        assert_eq!(read.record_limits.groups, coqui_access::Limit::at(256));
        let provider = MtlsAccountsProvider::try_from(read).unwrap();
        assert!(
            provider.put(fingerprinted(&"x".repeat(41))).is_err(),
            "a name past field_bytes"
        );
        assert!(provider.put(fingerprinted("abc")).is_ok());
    }

    #[test]
    fn a_ca_trust_reads_from_configuration_and_needs_its_file() {
        let empty = MtlsAccountsConfig::default();
        assert_eq!(empty.trust, TrustMode::Pinned);
        assert_eq!(empty.account, default_account());
        assert_eq!(empty.max_chain, DEFAULT_MAX_CHAIN);
        assert!(empty.ip_in_dns);
        let mut config = coqui_access::guatiao::Map::new();
        config.set("trust", "pinned-or-ca");
        let read = MtlsAccountsConfig::from_value(&config.into()).unwrap();
        assert_eq!(read.trust, TrustMode::PinnedOrCa);
        assert!(MtlsAccountsProvider::try_from(read).is_err(), "no ca_file");
    }
}
