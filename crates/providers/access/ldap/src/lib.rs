//! LDAP and Active Directory authentication for AI6.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

ai6::guatiao::providers!(LdapAccounts);

pub mod search;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use ai6::{Attempt, Authenticator, Directory, Lookup, Person, Verdict};
use ldap3::{LdapConn, LdapConnSettings, Scope, SearchEntry};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

/// LDAP result code for credentials rejected by a directory.
const INVALID_CREDENTIALS: u32 = 49;

/// LDAP authentication and query settings.
#[derive(Clone, FromValue, Schema, ToValue)]
#[map(crate = ::ai6::guatiao)]
#[non_exhaustive]
pub struct LdapAccountsConfig {
    /// LDAP or LDAPS server URL.
    pub url: String,
    /// Start TLS on an LDAP connection before sending credentials.
    #[map(default = false)]
    pub start_tls: bool,
    /// Optional PEM file with the only trusted server roots.
    pub ca_file: Option<String>,
    /// Base distinguished name used for searches.
    pub base: String,
    /// Service-account distinguished name used for searches.
    pub bind: Option<String>,
    /// Service-account password; do not include it in logs.
    #[schema(sensitive)]
    pub bind_password: Option<String>,
    /// LDAP filter with `{name}` for the escaped account name.
    #[map(default = String::from("(sAMAccountName={name})"))]
    pub user_filter: String,
    /// Attribute containing the directory's canonical account name.
    #[map(default = String::from("sAMAccountName"))]
    pub name_attribute: String,
    /// Attribute containing group distinguished names; empty disables direct groups.
    #[map(default = String::from("memberOf"))]
    pub group_attribute: String,
    /// Optional filter template with `{dn}` for groups containing the account.
    pub group_filter: Option<String>,
    /// Optional base for group searches; defaults to `base`.
    pub group_base: Option<String>,
    /// Read nested groups with Active Directory's matching rule.
    #[map(default = false)]
    pub nested_groups: bool,
    /// Connection and operation timeout in seconds; values below one become one.
    #[map(default = 10)]
    pub timeout_seconds: u32,
}

impl fmt::Debug for LdapAccountsConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LdapAccountsConfig")
            .field("url", &"[REDACTED]")
            .field("start_tls", &self.start_tls)
            .field("ca_file", &self.ca_file)
            .field("base", &self.base)
            .field("bind", &self.bind.as_ref().map(|_| "[REDACTED]"))
            .field(
                "bind_password",
                &self.bind_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("user_filter", &self.user_filter)
            .field("name_attribute", &self.name_attribute)
            .field("group_attribute", &self.group_attribute)
            .field("group_filter", &self.group_filter)
            .field("group_base", &self.group_base)
            .field("nested_groups", &self.nested_groups)
            .field("timeout_seconds", &self.timeout_seconds)
            .finish()
    }
}

/// LDAP accounts as AI6 authentication and directory providers.
#[derive(Provider)]
#[provider(
    Authenticator,
    Directory,
    id = "ai6_auth_ldap",
    name = "LDAP accounts",
    config = LdapAccountsConfig,
    crate = ::ai6::guatiao
)]
pub struct LdapAccounts {
    config: LdapAccountsConfig,
    trusted: Option<Arc<rustls::ClientConfig>>,
}

impl fmt::Debug for LdapAccounts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LdapAccounts")
            .field("config", &"[REDACTED]")
            .field("custom_ca", &self.trusted.is_some())
            .finish()
    }
}

struct Found {
    dn: String,
    name: Option<String>,
    groups: Vec<String>,
    disabled: bool,
}

impl LdapAccounts {
    /// Validates the LDAP configuration without opening a network connection.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe transport, incomplete credentials or invalid filters.
    pub fn new(config: LdapAccountsConfig) -> Result<Self, String> {
        if search::in_the_clear(&config.url, config.start_tls) {
            return Err(
                "a password would cross the network in the clear: use ldaps:// or ldap:// with start_tls=true"
                    .to_owned(),
            );
        }
        if config.base.trim().is_empty() {
            return Err("base must name the account search root".to_owned());
        }
        if !config.user_filter.contains("{name}") {
            return Err("user_filter must contain {name}".to_owned());
        }
        if config.bind.is_some() != config.bind_password.is_some() {
            return Err("bind and bind_password must be configured together".to_owned());
        }
        let trusted = config.ca_file.as_deref().map(trusting).transpose()?;
        Ok(Self {
            config,
            trusted: trusted.map(Arc::new),
        })
    }

    fn searching(&self) -> Result<LdapConn, ProviderError> {
        let timeout = self.timeout();
        let mut settings = LdapConnSettings::new()
            .set_conn_timeout(timeout)
            .set_starttls(self.config.start_tls);
        if let Some(trusted) = &self.trusted {
            settings = settings.set_config(Arc::clone(trusted));
        }
        let mut connection =
            LdapConn::with_settings(settings, &self.config.url).map_err(|error| {
                provider_error(
                    Status::GUATIAO_ERR_INTERNAL,
                    &format!("directory connection failed: {error}"),
                )
            })?;
        connection.with_timeout(timeout);
        if let (Some(bind), Some(password)) = (&self.config.bind, &self.config.bind_password) {
            let bound = connection.simple_bind(bind, password).map_err(|error| {
                provider_error(
                    Status::GUATIAO_ERR_INTERNAL,
                    &format!("directory bind failed: {error}"),
                )
            })?;
            bound.success().map_err(|_| {
                provider_error(
                    Status::GUATIAO_ERR_INTERNAL,
                    "directory refused its configured search account",
                )
            })?;
        }
        Ok(connection)
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(u64::from(self.config.timeout_seconds.max(1)))
    }

    fn found(&self, connection: &mut LdapConn, name: &str) -> Result<Option<Found>, ProviderError> {
        let filter = search::filter(&self.config.user_filter, "name", name);
        let mut attributes = vec![self.config.name_attribute.as_str(), "userAccountControl"];
        if !self.config.group_attribute.is_empty() {
            attributes.push(&self.config.group_attribute);
        }
        let result = connection
            .search(&self.config.base, Scope::Subtree, &filter, attributes)
            .map_err(|error| {
                provider_error(
                    Status::GUATIAO_ERR_INTERNAL,
                    &format!("directory search failed: {error}"),
                )
            })?;
        let (entries, _) = result.success().map_err(|error| {
            provider_error(
                Status::GUATIAO_ERR_INTERNAL,
                &format!("directory search failed: {error}"),
            )
        })?;
        let mut entries = entries.into_iter().map(SearchEntry::construct);
        let (Some(entry), None) = (entries.next(), entries.next()) else {
            return Ok(None);
        };
        let first = |attribute: &str| {
            entry
                .attrs
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(attribute))
                .and_then(|(_, values)| values.first())
                .cloned()
        };
        let mut groups: Vec<String> = entry
            .attrs
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(&self.config.group_attribute))
            .flat_map(|(_, values)| values.iter().map(|group| search::group_name(group)))
            .collect();
        let group_filter = self
            .config
            .group_filter
            .as_deref()
            .or_else(|| self.config.nested_groups.then_some(search::IN_CHAIN));
        if let Some(template) = group_filter {
            let filter = search::filter(template, "dn", &entry.dn);
            let base = self
                .config
                .group_base
                .as_deref()
                .unwrap_or(&self.config.base);
            let result = connection
                .search(base, Scope::Subtree, &filter, vec!["cn"])
                .map_err(|error| {
                    provider_error(
                        Status::GUATIAO_ERR_INTERNAL,
                        &format!("group search failed: {error}"),
                    )
                })?;
            let (entries, _) = result.success().map_err(|error| {
                provider_error(
                    Status::GUATIAO_ERR_INTERNAL,
                    &format!("group search failed: {error}"),
                )
            })?;
            for group in entries.into_iter().map(SearchEntry::construct) {
                let name = search::group_name(&group.dn);
                if !groups.contains(&name) {
                    groups.push(name);
                }
            }
        }
        Ok(Some(Found {
            name: first(&self.config.name_attribute),
            disabled: search::disabled(first("userAccountControl").as_deref()),
            dn: entry.dn,
            groups,
        }))
    }
}

fn trusting(file: &str) -> Result<rustls::ClientConfig, String> {
    let error = |reason: &dyn fmt::Display| format!("ca_file {file}: {reason}");
    let mut roots = rustls::RootCertStore::empty();
    let certificates = CertificateDer::pem_file_iter(file).map_err(|reason| error(&reason))?;
    for certificate in certificates {
        roots
            .add(certificate.map_err(|reason| error(&reason))?)
            .map_err(|reason| error(&reason))?;
    }
    if roots.is_empty() {
        return Err(error(&"no certificate was found"));
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|reason| error(&reason))?;
    Ok(config.with_root_certificates(roots).with_no_client_auth())
}

impl TryFrom<LdapAccountsConfig> for LdapAccounts {
    type Error = ProviderError;

    fn try_from(config: LdapAccountsConfig) -> Result<Self, Self::Error> {
        Self::new(config).map_err(|_| {
            provider_error(
                Status::GUATIAO_ERR_BAD_VALUE,
                "LDAP configuration is invalid",
            )
        })
    }
}

impl Authenticator for LdapAccounts {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let (Some(name), Some(password)) = (attempt.name, attempt.secret) else {
            return Ok(Verdict::Unknown);
        };
        let mut connection = self.searching()?;
        let Some(found) = self.found(&mut connection, &name)? else {
            let _ = connection.unbind();
            return Ok(Verdict::Unknown);
        };
        if password.is_empty() || found.disabled {
            let _ = connection.unbind();
            return Ok(Verdict::Invalid);
        }
        connection.with_timeout(self.timeout());
        let result = connection
            .simple_bind(&found.dn, &password)
            .map_err(|error| {
                provider_error(
                    Status::GUATIAO_ERR_INTERNAL,
                    &format!("account bind failed: {error}"),
                )
            })?;
        let _ = connection.unbind();
        match result.rc {
            0 => Ok(Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: found.name.unwrap_or(name),
            })),
            INVALID_CREDENTIALS => Ok(Verdict::Invalid),
            _ => Err(provider_error(
                Status::GUATIAO_ERR_INTERNAL,
                "directory could not verify the account",
            )),
        }
    }
}

impl Directory for LdapAccounts {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        if lookup.by != "name" {
            return Ok(None);
        }
        let mut connection = self.searching()?;
        let found = self.found(&mut connection, &lookup.value)?;
        let _ = connection.unbind();
        Ok(found.filter(|found| !found.disabled).map(|found| Person {
            id: found.dn,
            name: found.name.unwrap_or(lookup.value),
            display_name: None,
            email: None,
            groups: found.groups,
            attributes: Default::default(),
        }))
    }
}

fn provider_error(status: Status, message: &str) -> ProviderError {
    ProviderError::new(status, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(url: &str) -> LdapAccountsConfig {
        LdapAccountsConfig {
            url: url.to_owned(),
            start_tls: false,
            ca_file: None,
            base: "DC=example,DC=org".to_owned(),
            bind: None,
            bind_password: None,
            user_filter: "(sAMAccountName={name})".to_owned(),
            name_attribute: "sAMAccountName".to_owned(),
            group_attribute: "memberOf".to_owned(),
            group_filter: None,
            group_base: None,
            nested_groups: false,
            timeout_seconds: 10,
        }
    }

    #[test]
    fn refuses_cleartext_remote_urls_and_incomplete_configuration() {
        assert!(
            LdapAccounts::new(config("ldap://directory.example.org"))
                .unwrap_err()
                .contains("clear")
        );
        assert!(LdapAccounts::new(config("ldaps://directory.example.org")).is_ok());
        let mut start_tls = config("ldap://directory.example.org");
        start_tls.start_tls = true;
        assert!(LdapAccounts::new(start_tls).is_ok());
        let mut filter = config("ldaps://directory.example.org");
        filter.user_filter = "(objectClass=person)".to_owned();
        assert!(LdapAccounts::new(filter).unwrap_err().contains("{name}"));
        let mut bind = config("ldaps://directory.example.org");
        bind.bind = Some("CN=search".to_owned());
        assert!(LdapAccounts::new(bind).unwrap_err().contains("together"));
    }

    #[test]
    fn debug_output_redacts_directory_credentials_and_url() {
        let mut config = config("ldaps://user:secret@directory.example.org");
        config.bind = Some("CN=search".to_owned());
        config.bind_password = Some("very-secret".to_owned());
        let debug = format!("{:?}", config);
        assert!(!debug.contains("very-secret"));
        assert!(!debug.contains("user:secret"));
        assert!(debug.contains("[REDACTED]"));
    }
}
