//! LDAP and Active Directory authentication and directory lookup.
#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

mod primary;
pub mod search;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use coqui_access::guatiao::Provider;
use coqui_access::guatiao::library::ProviderError;
use coqui_access::{Attempt, Authenticator, Directory, Lookup, Person, Verdict};
use coqui_access_kit::{CacheOptions, CacheSetting, Cached};
use coqui_builder::{Describe, Secret};
use coqui_peer_tls::{ClientBuilder, NonDefault, Policy, Roots, Trust};
use guatiao_builder::Described;
use ldap3::{LdapConn, LdapConnSettings, Scope, SearchEntry};

/// The account filter a filter lookup is ANDed with unless the configuration says another:
/// people who are users, which leaves out Active Directory's computer accounts.
pub const DEFAULT_OBJECT_FILTER: &str = "(&(objectCategory=person)(objectClass=user))";

/// LDAP result code for credentials rejected by a directory.
const INVALID_CREDENTIALS: u32 = 49;

/// LDAP authentication and query settings.
#[derive(Clone, Describe)]
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
    /// Service-account password; wiped when dropped and never shown.
    pub bind_password: Option<Secret<String>>,
    /// LDAP filter with `{name}` for the escaped account name.
    #[map(default = String::from("(sAMAccountName={name})"))]
    pub user_filter: String,
    /// The filter every account a filter lookup may find matches, which the lookup's filter
    /// is AND-ed with so it can only narrow it. It must carry every restriction an account
    /// must meet to sign in here (people, not computer or service accounts; an organisational
    /// unit; a group): `user_filter` is not applied to a filter lookup, as it is built around
    /// a name. The default is people, not Active Directory computer accounts.
    #[map(default = String::from(DEFAULT_OBJECT_FILTER))]
    pub object_filter: String,
    /// Whether a lookup `by: "filter"` is answered. Off by default: a filter is built only by
    /// a matcher's rule, with its values escaped, and a directory with no such rule has no
    /// reason to take one. Anything that can make a lookup `by: "filter"` with its own text
    /// can search the directory, so turn this on only where the lookups come from a matcher.
    #[map(default = false)]
    pub allow_filter_lookups: bool,
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
    /// Lookups kept for a few seconds, to absorb bursts: `true` (5 s, 1 s for "not found",
    /// 10 000 entries), `false`, or `{ "ttl", "negative_ttl", "max_entries" }` (durations read
    /// `5`, `1.5`, `5s` or `250ms`). Passwords are always checked at the directory.
    #[map(default)]
    pub cache: CacheSetting,
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
            .field("object_filter", &self.object_filter)
            .field("allow_filter_lookups", &self.allow_filter_lookups)
            .field("name_attribute", &self.name_attribute)
            .field("group_attribute", &self.group_attribute)
            .field("group_filter", &self.group_filter)
            .field("group_base", &self.group_base)
            .field("nested_groups", &self.nested_groups)
            .field("timeout_seconds", &self.timeout_seconds)
            .finish()
    }
}

/// LDAP accounts as authentication and directory providers, with lookups kept as the
/// configuration's `cache` says.
#[derive(Provider)]
#[provider(
    Authenticator,
    Directory,
    id = "coqui_access_ldap",
    name = "LDAP accounts",
    config = Described<LdapAccountsConfig>,
    crate = ::coqui_access::guatiao
)]
pub struct LdapAccounts {
    directory: Cached<Ldap>,
}

impl fmt::Debug for LdapAccounts {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LdapAccounts")
            .field("directory", &self.directory.inner())
            .field("cache", &self.directory)
            .finish()
    }
}

impl LdapAccounts {
    /// Validates the LDAP configuration without opening a network connection.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe transport, incomplete credentials, invalid filters, or a
    /// `ca_file` that cannot be read.
    pub fn new(config: LdapAccountsConfig) -> Result<Self, String> {
        let cache = config.cache.options();
        Ok(Self {
            directory: Cached::new(Arc::new(Ldap::new(config)?), cache),
        })
    }

    /// What the TLS configuration for `ca_file` allows beyond coqui-peer-tls's default policy
    /// (TLS 1.2 and 1.3), as data for the host to act on. Empty without a `ca_file`, where
    /// ldap3's own rustls defaults apply.
    pub fn non_defaults(&self) -> &[NonDefault] {
        &self.directory.inner().non_defaults
    }

    /// How lookups are kept.
    pub fn cache(&self) -> CacheOptions {
        self.directory.options()
    }
}

impl Authenticator for LdapAccounts {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        self.directory.inner().authenticate(attempt)
    }
}

impl Directory for LdapAccounts {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        self.directory.lookup(lookup)
    }
}

/// The directory itself, asked every time.
struct Ldap {
    config: LdapAccountsConfig,
    trusted: Option<Arc<rustls::ClientConfig>>,
    non_defaults: Vec<NonDefault>,
    primary_groups: primary::GroupCache,
}

impl fmt::Debug for Ldap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ldap")
            .field("config", &"[REDACTED]")
            .field("custom_ca", &self.trusted.is_some())
            .finish()
    }
}

struct Found {
    dn: String,
    name: Option<String>,
    display_name: Option<String>,
    email: Option<String>,
    groups: Vec<String>,
    disabled: bool,
}

impl Ldap {
    fn new(config: LdapAccountsConfig) -> Result<Self, String> {
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
        if !search::one_group(&config.object_filter) {
            return Err("object_filter must be one parenthesised filter".to_owned());
        }
        if config.bind.is_some() != config.bind_password.is_some() {
            return Err("bind and bind_password must be configured together".to_owned());
        }
        let (trusted, non_defaults) = match config.ca_file.as_deref() {
            Some(file) => {
                let built = ClientBuilder::new()
                    .server(Trust::Roots(Roots::bundle_file(file)))
                    .policy(Policy::default())
                    .build()
                    .map_err(|reason| format!("ca_file {file}: {reason}"))?;
                let non_defaults = built.non_defaults();
                (Some(Arc::new(built.config)), non_defaults)
            }
            None => (None, Vec::new()),
        };
        Ok(Self {
            primary_groups: primary::GroupCache::new(config.cache.options()),
            config,
            trusted,
            non_defaults,
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
                coqui_access::error::internal(&format!("directory connection failed: {error}"))
            })?;
        connection.with_timeout(timeout);
        if let (Some(bind), Some(password)) = (&self.config.bind, &self.config.bind_password) {
            let bound = connection
                .simple_bind(bind, password.expose())
                .map_err(|error| {
                    coqui_access::error::internal(&format!("directory bind failed: {error}"))
                })?;
            bound.success().map_err(|_| {
                coqui_access::error::internal("directory refused its configured search account")
            })?;
        }
        Ok(connection)
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(u64::from(self.config.timeout_seconds.max(1)))
    }

    fn found(
        &self,
        connection: &mut dyn Entries,
        name: &str,
    ) -> Result<Option<Found>, ProviderError> {
        let filter = search::filter(&self.config.user_filter, "name", name);
        self.found_by(connection, &filter, false)
    }

    fn attributes(&self) -> Vec<&str> {
        let mut attributes = vec![
            self.config.name_attribute.as_str(),
            "userAccountControl",
            "displayName",
            "givenName",
            "initials",
            "sn",
            "mail",
            "primaryGroupID",
            "objectSid",
        ];
        if !self.config.group_attribute.is_empty() {
            attributes.push(&self.config.group_attribute);
        }
        attributes
    }

    /// The one account `filter` finds. Several are nobody for a name (as ever), and
    /// `ambiguous` for a caller's filter, which must be able to tell the two.
    fn found_by(
        &self,
        connection: &mut dyn Entries,
        filter: &str,
        refuse_several: bool,
    ) -> Result<Option<Found>, ProviderError> {
        let entries = connection.search(
            &self.config.base,
            Scope::Subtree,
            filter,
            &self.attributes(),
        )?;
        let mut entries = entries.into_iter();
        let (first_entry, second_entry) = (entries.next(), entries.next());
        if refuse_several && second_entry.is_some() {
            return Err(coqui_access::error::ambiguous());
        }
        let (Some(entry), None) = (first_entry, second_entry) else {
            return Ok(None);
        };
        self.describe(connection, entry).map(Some)
    }

    /// The account whose distinguished name is `dn`, read directly (base scope) and only when
    /// it lies below the configured base and meets the account filter.
    fn found_at(
        &self,
        connection: &mut dyn Entries,
        dn: &str,
    ) -> Result<Option<Found>, ProviderError> {
        if !search::below(dn, &self.config.base) {
            return Ok(None);
        }
        let entries = connection.search(
            dn,
            Scope::Base,
            &self.config.object_filter,
            &self.attributes(),
        )?;
        let Some(entry) = entries.into_iter().next() else {
            return Ok(None);
        };
        // The server's own spelling of the name must still be inside the base.
        if !search::below(&entry.dn, &self.config.base) {
            return Ok(None);
        }
        self.describe(connection, entry).map(Some)
    }

    fn group_base(&self) -> &str {
        self.config
            .group_base
            .as_deref()
            .unwrap_or(&self.config.base)
    }

    /// The group `primaryGroupID` names: the account's domain SID with that RID, searched by
    /// its binary `objectSid` under the group base. Active Directory leaves it out of
    /// `memberOf`. A missing or malformed id or SID, no such group, several, or a directory
    /// that will not answer, is no primary group: it adds nothing and refuses nothing.
    fn primary_group(
        &self,
        connection: &mut dyn Entries,
        entry: &SearchEntry,
    ) -> Option<primary::Group> {
        let rid: u32 = entry
            .attrs
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("primaryGroupID"))?
            .1
            .first()?
            .trim()
            .parse()
            .ok()?;
        let sid = entry
            .bin_attrs
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("objectSid"))
            .and_then(|(_, values)| values.first().map(Vec::as_slice))
            .or_else(|| {
                entry
                    .attrs
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("objectSid"))
                    .and_then(|(_, values)| values.first().map(String::as_bytes))
            })?;
        let group_sid = primary::Sid::parse(sid)?.sibling(rid)?;
        let key = group_sid.text();
        if let Some(kept) = self.primary_groups.get(&key) {
            return kept;
        }
        // The filter holds only the group SID's bytes, each as `\xx`; nothing read from the
        // account is text in it.
        let filter = format!(
            "(&(objectClass=group)(objectSid={}))",
            group_sid.filter_value()
        );
        let found = connection
            .search(self.group_base(), Scope::Subtree, &filter, &["cn"])
            .ok()?;
        let group = match found.as_slice() {
            [only] if search::below(&only.dn, self.group_base()) => Some(primary::Group {
                name: search::group_name(&only.dn),
                dn: only.dn.clone(),
            }),
            _ => None,
        };
        self.primary_groups.put(&key, group.clone());
        group
    }

    fn describe(
        &self,
        connection: &mut dyn Entries,
        entry: SearchEntry,
    ) -> Result<Found, ProviderError> {
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
            for group in connection.search(base, Scope::Subtree, &filter, &["cn"])? {
                let name = search::group_name(&group.dn);
                if !groups.contains(&name) {
                    groups.push(name);
                }
            }
        }
        if let Some(primary) = self.primary_group(connection, &entry) {
            let name = primary.name;
            if !groups.contains(&name) {
                groups.push(name);
            }
            // Only the matching rule or filter that says nested groups count reaches the
            // groups the primary group is in.
            let nested = self.config.nested_groups.then(|| {
                self.config
                    .group_filter
                    .as_deref()
                    .unwrap_or(search::IN_CHAIN)
            });
            if let Some(template) = nested {
                let filter = search::filter(template, "dn", &primary.dn);
                // A failed search for these adds nothing, as a primary group not found does.
                let found = connection.search(self.group_base(), Scope::Subtree, &filter, &["cn"]);
                for group in found.unwrap_or_default() {
                    let name = search::group_name(&group.dn);
                    if !groups.contains(&name) {
                        groups.push(name);
                    }
                }
            }
        }
        Ok(Found {
            name: first(&self.config.name_attribute),
            disabled: search::disabled(first("userAccountControl").as_deref()),
            display_name: search::display_name(
                first("displayName").as_deref(),
                first("givenName").as_deref(),
                first("initials").as_deref(),
                first("sn").as_deref(),
            ),
            email: first("mail")
                .map(|mail| mail.trim().to_owned())
                .filter(|mail| !mail.is_empty()),
            dn: entry.dn,
            groups,
        })
    }
}

/// What the account searches need of a directory connection, so a stand-in can answer them.
trait Entries {
    /// The entries a search finds; none for a base-scope read of a name that is not there.
    fn search(
        &mut self,
        base: &str,
        scope: Scope,
        filter: &str,
        attributes: &[&str],
    ) -> Result<Vec<SearchEntry>, ProviderError>;
}

impl Entries for LdapConn {
    fn search(
        &mut self,
        base: &str,
        scope: Scope,
        filter: &str,
        attributes: &[&str],
    ) -> Result<Vec<SearchEntry>, ProviderError> {
        let single = matches!(scope, Scope::Base);
        let result =
            LdapConn::search(self, base, scope, filter, attributes.to_vec()).map_err(|error| {
                coqui_access::error::internal(&format!("directory search failed: {error}"))
            })?;
        match result.success() {
            Ok((entries, _)) => Ok(entries.into_iter().map(SearchEntry::construct).collect()),
            // noSuchObject and invalidDNSyntax on a base read mean "no such account".
            Err(ldap3::LdapError::LdapResult { result })
                if single && matches!(result.rc, 32 | 34) =>
            {
                Ok(Vec::new())
            }
            Err(error) => Err(coqui_access::error::internal(&format!(
                "directory search failed: {error}"
            ))),
        }
    }
}

impl TryFrom<LdapAccountsConfig> for LdapAccounts {
    type Error = ProviderError;

    fn try_from(config: LdapAccountsConfig) -> Result<Self, Self::Error> {
        Self::new(config)
            .map_err(|_| coqui_access::error::bad_value("LDAP configuration is invalid"))
    }
}
impl TryFrom<Described<LdapAccountsConfig>> for LdapAccounts {
    type Error = ProviderError;

    fn try_from(config: Described<LdapAccountsConfig>) -> Result<Self, Self::Error> {
        Self::try_from(config.0)
    }
}

impl Authenticator for Ldap {
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
                coqui_access::error::internal(&format!("account bind failed: {error}"))
            })?;
        let _ = connection.unbind();
        match result.rc {
            0 => Ok(Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: found.name.unwrap_or(name),
                ..Lookup::default()
            })),
            INVALID_CREDENTIALS => Ok(Verdict::Invalid),
            _ => Err(coqui_access::error::internal(
                "directory could not verify the account",
            )),
        }
    }
}

impl Ldap {
    /// The one enabled account a filter finds under the account filter: `ambiguous` for
    /// several, `None` for none or a disabled one.
    fn lookup_filter(
        &self,
        connection: &mut dyn Entries,
        given: &str,
    ) -> Result<Option<Person>, ProviderError> {
        if !search::one_group(given) {
            return Err(coqui_access::error::bad_value(
                "a filter lookup is one parenthesised filter",
            ));
        }
        let filter = search::all_of(&self.config.object_filter, given);
        let found = self.found_by(connection, &filter, true)?;
        Ok(found.filter(|found| !found.disabled).map(person))
    }

    /// A `name` lookup by `user_filter`, an `id` lookup by the distinguished name a lookup
    /// returned; any other `by` is nobody.
    fn resolve(
        &self,
        connection: &mut dyn Entries,
        lookup: &Lookup,
    ) -> Result<Option<Person>, ProviderError> {
        let found = match lookup.by.as_str() {
            "name" => self.found(connection, &lookup.value)?,
            "id" => self.found_at(connection, &lookup.value)?,
            _ => None,
        };
        Ok(found.filter(|found| !found.disabled).map(person))
    }
}

fn person(found: Found) -> Person {
    Person {
        name: found.name.unwrap_or_else(|| found.dn.clone()),
        id: found.dn,
        display_name: found.display_name,
        email: found.email,
        groups: found.groups,
        attributes: Default::default(),
    }
}

impl Directory for Ldap {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        if lookup.by == "filter" {
            if !self.config.allow_filter_lookups {
                return Err(coqui_access::error::unsupported(
                    "this directory does not take filter lookups",
                ));
            }
            if !search::one_group(&lookup.value) {
                return Err(coqui_access::error::bad_value(
                    "a filter lookup is one parenthesised filter",
                ));
            }
            let mut connection = self.searching()?;
            let found = self.lookup_filter(&mut connection, &lookup.value);
            let _ = connection.unbind();
            return found;
        }
        if lookup.by != "name" && lookup.by != "id" {
            return Ok(None);
        }
        let mut connection = self.searching()?;
        let found = self.resolve(&mut connection, &lookup);
        let _ = connection.unbind();
        found
    }
}

#[cfg(test)]
mod tests {
    use coqui_builder::Partial;

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
            object_filter: DEFAULT_OBJECT_FILTER.to_owned(),
            allow_filter_lookups: true,
            name_attribute: "sAMAccountName".to_owned(),
            group_attribute: "memberOf".to_owned(),
            group_filter: None,
            group_base: None,
            nested_groups: false,
            timeout_seconds: 10,
            cache: CacheSetting::default(),
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
        config.bind_password = Some(Secret::new("very-secret".to_owned()));
        let debug = format!("{:?}", config);
        assert!(!debug.contains("very-secret"));
        assert!(!debug.contains("user:secret"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn a_ca_file_trusts_its_roots_under_the_default_policy() {
        let directory = tempfile::tempdir().unwrap();
        let ca = rcgen::generate_simple_self_signed(["directory.example.org".to_owned()]).unwrap();
        let file = directory.path().join("ca.pem");
        std::fs::write(&file, ca.cert.pem()).unwrap();
        let mut trusting = config("ldaps://directory.example.org");
        trusting.ca_file = Some(file.to_str().unwrap().to_owned());
        let accounts = LdapAccounts::new(trusting).unwrap();
        assert!(accounts.directory.inner().trusted.is_some());
        assert!(
            accounts.non_defaults().is_empty(),
            "the default policy lists nothing"
        );
        assert!(
            LdapAccounts::new(config("ldaps://directory.example.org"))
                .unwrap()
                .non_defaults()
                .is_empty()
        );
        for bad in ["missing.pem", "empty.pem"] {
            let path = directory.path().join(bad);
            if bad == "empty.pem" {
                std::fs::write(&path, "").unwrap();
            }
            let mut refused = config("ldaps://directory.example.org");
            refused.ca_file = Some(path.to_str().unwrap().to_owned());
            assert!(
                LdapAccounts::new(refused)
                    .unwrap_err()
                    .starts_with("ca_file")
            );
        }
    }

    #[test]
    fn filter_lookups_are_off_unless_the_directory_is_set_to_take_them() {
        let mut off = config("ldaps://directory.example.org");
        off.allow_filter_lookups = false;
        let accounts = LdapAccounts::new(off).unwrap();
        let refused = accounts
            .lookup(Lookup {
                by: "filter".to_owned(),
                value: "(uid=*)".to_owned(),
                ..Lookup::default()
            })
            .unwrap_err();
        assert_eq!(
            refused.status,
            coqui_access::guatiao::Status::GUATIAO_ERR_UNSUPPORTED
        );
        // A configuration that says nothing of it is off, and its account filter is people.
        let mut partial = Partial::<LdapAccountsConfig>::new();
        partial.set("url", "ldaps://directory.example.org").unwrap();
        partial.set("base", "DC=example,DC=org").unwrap();
        let said_nothing = partial.build().unwrap();
        assert!(!said_nothing.allow_filter_lookups);
        assert_eq!(
            said_nothing.object_filter,
            "(&(objectCategory=person)(objectClass=user))"
        );
    }

    #[test]
    fn a_filter_lookup_is_refused_unless_it_is_one_filter_and_other_kinds_are_not_found() {
        let accounts = LdapAccounts::new(config("ldaps://directory.example.org")).unwrap();
        for bad in ["uid=1", "(a=1)(b=2)", "(a=1", ""] {
            let refused = accounts
                .lookup(Lookup {
                    by: "filter".to_owned(),
                    value: bad.to_owned(),
                    ..Lookup::default()
                })
                .unwrap_err();
            assert_eq!(
                refused.status,
                coqui_access::guatiao::Status::GUATIAO_ERR_BAD_VALUE,
                "{bad}"
            );
            assert!(!coqui_access::error::is_ambiguous(&refused), "{bad}");
        }
        let other = accounts
            .lookup(Lookup {
                by: "email".to_owned(),
                value: "x".to_owned(),
                ..Lookup::default()
            })
            .unwrap();
        assert!(other.is_none());
        let mut object = config("ldaps://directory.example.org");
        object.object_filter = "objectClass=user".to_owned();
        assert!(
            LdapAccounts::new(object)
                .unwrap_err()
                .contains("object_filter")
        );
    }

    #[test]
    fn lookups_are_cached_by_default_and_the_setting_reads_as_a_flag_or_options() {
        let accounts = LdapAccounts::new(config("ldaps://directory.example.org")).unwrap();
        assert_eq!(accounts.cache(), CacheOptions::default());
        let mut off = config("ldaps://directory.example.org");
        off.cache = CacheSetting::Enabled(false);
        assert_eq!(LdapAccounts::new(off).unwrap().cache(), CacheOptions::off());
        let read = |key: &str, text: &str| {
            let mut partial = Partial::from(config("ldaps://directory.example.org"));
            partial.set(key, text).unwrap();
            partial.build().unwrap().cache.options()
        };
        assert_eq!(read("cache", "false"), CacheOptions::off());
        assert_eq!(read("cache", "true"), CacheOptions::default());
        assert_eq!(read("cache.ttl", "1").ttl, Duration::from_secs(1));
        assert_eq!(read("cache.ttl", "250ms").ttl, Duration::from_millis(250));
    }

    /// A directory held in memory: it answers a search by scope and the one attribute a test's
    /// filters name, and records what was asked.
    struct StandIn {
        entries: Vec<SearchEntry>,
        asked: Vec<(String, bool)>,
        filters: Vec<String>,
    }

    fn entry(dn: &str, attrs: &[(&str, &str)]) -> SearchEntry {
        SearchEntry {
            dn: dn.to_owned(),
            attrs: attrs
                .iter()
                .map(|(name, value)| ((*name).to_owned(), vec![(*value).to_owned()]))
                .collect(),
            bin_attrs: Default::default(),
        }
    }

    impl Entries for StandIn {
        fn search(
            &mut self,
            base: &str,
            scope: Scope,
            filter: &str,
            _: &[&str],
        ) -> Result<Vec<SearchEntry>, ProviderError> {
            let base_scope = matches!(scope, Scope::Base);
            self.asked.push((base.to_owned(), base_scope));
            self.filters.push(filter.to_owned());
            let wanted_sid = filter.split("(objectSid=").nth(1).map(|rest| {
                let hex = rest.split(')').next().unwrap_or_default();
                hex.split('\\')
                    .filter(|byte| !byte.is_empty())
                    .map(|byte| u8::from_str_radix(byte, 16).unwrap())
                    .collect::<Vec<u8>>()
            });
            let chain_of = filter
                .split("1.2.840.113556.1.4.1941:=")
                .nth(1)
                .map(|rest| rest.trim_end_matches(')').to_owned());
            let wanted = filter
                .split("sAMAccountName=")
                .nth(1)
                .map(|rest| rest.trim_end_matches(')'));
            Ok(self
                .entries
                .iter()
                .filter(|candidate| {
                    if base_scope {
                        // A server compares names by their components, not their spelling.
                        let squeezed = |dn: &str| dn.replace(' ', "").to_lowercase();
                        squeezed(&candidate.dn) == squeezed(base)
                    } else if !below_or_at(&candidate.dn, base) {
                        false
                    } else if let Some(sid) = &wanted_sid {
                        candidate
                            .bin_attrs
                            .get("objectSid")
                            .is_some_and(|values| values[0] == *sid)
                    } else if let Some(group) = &chain_of {
                        candidate
                            .attrs
                            .get("member")
                            .is_some_and(|members| members.contains(group))
                    } else {
                        wanted.is_none_or(|name| {
                            candidate
                                .attrs
                                .get("sAMAccountName")
                                .is_some_and(|v| v[0] == name)
                        })
                    }
                })
                .cloned()
                .collect())
        }
    }

    fn stand_in() -> StandIn {
        StandIn {
            entries: vec![
                entry(
                    "CN=Ana,OU=People,DC=example,DC=org",
                    &[
                        ("sAMAccountName", "ana"),
                        ("displayName", "Ana Q. Rivera"),
                        ("mail", "ana@example.org"),
                    ],
                ),
                entry(
                    "CN=Bo,OU=People,DC=example,DC=org",
                    &[
                        ("sAMAccountName", "bo"),
                        ("givenName", "Bo"),
                        ("initials", "J"),
                        ("sn", "Diaz"),
                    ],
                ),
                entry(
                    "CN=Cy,OU=People,DC=example,DC=org",
                    &[("sAMAccountName", "cy"), ("userAccountControl", "514")],
                ),
                entry(
                    "CN=Dee,OU=People,DC=example,DC=org",
                    &[("sAMAccountName", "dee")],
                ),
                entry("CN=Outside,DC=other,DC=org", &[("sAMAccountName", "out")]),
            ],
            asked: Vec::new(),
            filters: Vec::new(),
        }
    }

    fn below_or_at(dn: &str, base: &str) -> bool {
        search::below(dn, base) || dn.eq_ignore_ascii_case(base)
    }

    fn lookup(directory: &mut StandIn, by: &str, value: &str) -> Option<Person> {
        lookup_with(
            directory,
            config("ldaps://directory.example.org"),
            by,
            value,
        )
    }

    fn lookup_with(
        directory: &mut StandIn,
        config: LdapAccountsConfig,
        by: &str,
        value: &str,
    ) -> Option<Person> {
        let ldap = Ldap::new(config).unwrap();
        ldap.resolve(
            directory,
            &Lookup {
                by: by.to_owned(),
                value: value.to_owned(),
                ..Lookup::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn an_id_lookup_answers_the_person_a_name_lookup_gave() {
        let mut directory = stand_in();
        let by_name = lookup(&mut directory, "name", "ana").unwrap();
        assert_eq!(by_name.id, "CN=Ana,OU=People,DC=example,DC=org");
        let by_id = lookup(&mut directory, "id", &by_name.id).unwrap();
        assert_eq!(by_id, by_name);
        // The DN's case and spacing do not matter to which entry it is.
        let spaced = lookup(
            &mut directory,
            "id",
            "cn=Ana, ou=People, dc=EXAMPLE, dc=org",
        );
        assert_eq!(spaced.map(|person| person.name), Some("ana".to_owned()));
    }

    #[test]
    fn an_id_lookup_of_a_missing_disabled_or_outside_name_is_nobody() {
        let mut directory = stand_in();
        assert!(
            lookup(
                &mut directory,
                "id",
                "CN=Nobody,OU=People,DC=example,DC=org"
            )
            .is_none()
        );
        assert!(lookup(&mut directory, "id", "CN=Cy,OU=People,DC=example,DC=org").is_none());
        directory.asked.clear();
        for outside in [
            "CN=Outside,DC=other,DC=org",
            "DC=example,DC=org",
            "DC=org",
            "CN=Ana,OU=People,DC=example,DC=org,DC=evil",
            "",
            "not a name",
        ] {
            assert!(lookup(&mut directory, "id", outside).is_none(), "{outside}");
        }
        assert!(
            directory.asked.is_empty(),
            "nothing outside the base is read"
        );
    }

    #[test]
    fn an_id_lookup_reads_one_entry_and_a_name_lookup_searches_the_base() {
        let mut directory = stand_in();
        lookup(&mut directory, "id", "CN=Ana,OU=People,DC=example,DC=org");
        assert_eq!(
            directory.asked,
            [("CN=Ana,OU=People,DC=example,DC=org".to_owned(), true)]
        );
        directory.asked.clear();
        lookup(&mut directory, "name", "ana");
        assert_eq!(directory.asked, [("DC=example,DC=org".to_owned(), false)]);
    }

    #[test]
    fn the_person_carries_display_name_and_email() {
        let mut directory = stand_in();
        let ana = lookup(&mut directory, "name", "ana").unwrap();
        assert_eq!(ana.display_name.as_deref(), Some("Ana Q. Rivera"));
        assert_eq!(ana.email.as_deref(), Some("ana@example.org"));
        let bo = lookup(&mut directory, "name", "bo").unwrap();
        assert_eq!(bo.display_name.as_deref(), Some("Bo J Diaz"));
        assert_eq!(bo.email, None);
        let dee = lookup(&mut directory, "name", "dee").unwrap();
        assert_eq!((dee.display_name, dee.email), (None, None));
    }

    const DOMAIN: [u32; 4] = [21, 11, 22, 33];

    fn sid_in(domain: &[u32], rid: u32) -> Vec<u8> {
        let mut subs = domain.to_vec();
        subs.push(rid);
        let mut bytes = vec![1, subs.len() as u8, 0, 0, 0, 0, 0, 5];
        for sub in subs {
            bytes.extend_from_slice(&sub.to_le_bytes());
        }
        bytes
    }

    fn sid(rid: u32) -> Vec<u8> {
        sid_in(&DOMAIN, rid)
    }

    fn with_sid(mut found: SearchEntry, bytes: Vec<u8>) -> SearchEntry {
        found.bin_attrs.insert("objectSid".to_owned(), vec![bytes]);
        found
    }

    fn with_attr(mut found: SearchEntry, name: &str, values: &[&str]) -> SearchEntry {
        found.attrs.insert(
            name.to_owned(),
            values.iter().map(|value| (*value).to_owned()).collect(),
        );
        found
    }

    /// A user whose `memberOf` is empty, the domain's groups, and a group of another domain
    /// with the same RID. `Domain Users` is RID 513; `Staff` has it as a member.
    fn domain_directory(primary: Option<&str>) -> StandIn {
        let mut user = with_sid(
            entry(
                "CN=Eve,OU=People,DC=example,DC=org",
                &[("sAMAccountName", "eve")],
            ),
            sid(1104),
        );
        if let Some(primary) = primary {
            user = with_attr(user, "primaryGroupID", &[primary]);
        }
        StandIn {
            entries: vec![
                user,
                with_sid(
                    entry("CN=Domain Users,CN=Users,DC=example,DC=org", &[]),
                    sid(513),
                ),
                with_attr(
                    with_sid(
                        entry("CN=Staff,OU=Groups,DC=example,DC=org", &[]),
                        sid(2001),
                    ),
                    "member",
                    &["CN=Domain Users,CN=Users,DC=example,DC=org"],
                ),
                with_sid(
                    entry("CN=Interns,CN=Users,DC=example,DC=org", &[]),
                    sid(2002),
                ),
                with_sid(
                    entry("CN=Domain Users,CN=Users,DC=example,DC=org", &[]),
                    sid_in(&[21, 99, 22, 33], 513),
                ),
            ],
            asked: Vec::new(),
            filters: Vec::new(),
        }
    }

    fn eve(primary: Option<&str>, tweak: impl FnOnce(&mut LdapAccountsConfig)) -> Person {
        let mut directory = domain_directory(primary);
        let mut settings = config("ldaps://directory.example.org");
        tweak(&mut settings);
        lookup_with(&mut directory, settings, "name", "eve").unwrap()
    }

    fn sid_searches(directory: &StandIn) -> usize {
        directory
            .filters
            .iter()
            .filter(|filter| filter.contains("objectSid="))
            .count()
    }

    #[test]
    fn a_user_whose_only_group_is_the_primary_one_gets_it() {
        assert_eq!(eve(Some("513"), |_| {}).groups, ["Domain Users"]);
    }

    #[test]
    fn another_primary_group_is_the_one_added() {
        assert_eq!(eve(Some("2002"), |_| {}).groups, ["Interns"]);
    }

    #[test]
    fn a_missing_or_unresolvable_primary_group_adds_nothing_and_refuses_nothing() {
        for primary in [
            None,
            Some("999"),
            Some(""),
            Some("-1"),
            Some("513)(objectClass=*"),
            Some("99999999999999"),
        ] {
            let person = eve(primary, |_| {});
            assert_eq!(person.name, "eve", "{primary:?}");
            assert!(person.groups.is_empty(), "{primary:?}");
        }
        // Without a readable objectSid there is no domain to name a group in.
        let mut directory = domain_directory(Some("513"));
        directory.entries[0].bin_attrs.clear();
        let person = lookup(&mut directory, "name", "eve").unwrap();
        assert!(person.groups.is_empty());
        assert_eq!(sid_searches(&directory), 0);
    }

    #[test]
    fn the_primary_group_is_not_listed_twice_and_follows_direct_groups() {
        let mut directory = domain_directory(Some("513"));
        directory.entries[0] = with_attr(
            directory.entries[0].clone(),
            "memberOf",
            &[
                "CN=Domain Users,CN=Users,DC=example,DC=org",
                "CN=Ops,OU=Groups,DC=example,DC=org",
            ],
        );
        let person = lookup(&mut directory, "name", "eve").unwrap();
        assert_eq!(person.groups, ["Domain Users", "Ops"]);
    }

    #[test]
    fn nested_groups_of_the_primary_group_follow_when_nested_groups_is_on() {
        assert_eq!(eve(Some("513"), |_| {}).groups, ["Domain Users"]);
        let on = eve(Some("513"), |settings| settings.nested_groups = true);
        assert_eq!(on.groups, ["Domain Users", "Staff"]);
    }

    #[test]
    fn the_primary_group_is_searched_by_its_binary_sid_under_the_group_base_only() {
        let mut directory = domain_directory(Some("513"));
        let mut settings = config("ldaps://directory.example.org");
        settings.group_base = Some("CN=Users,DC=example,DC=org".to_owned());
        let person = lookup_with(&mut directory, settings, "name", "eve").unwrap();
        assert_eq!(person.groups, ["Domain Users"]);
        let at = directory
            .filters
            .iter()
            .position(|filter| filter.contains("objectSid="))
            .unwrap();
        assert_eq!(
            directory.asked[at],
            ("CN=Users,DC=example,DC=org".to_owned(), false)
        );
        assert_eq!(
            directory.filters[at],
            "(&(objectClass=group)(objectSid=\\01\\05\\00\\00\\00\\00\\00\\05\\15\\00\\00\\00\\0b\\00\\00\\00\\16\\00\\00\\00\\21\\00\\00\\00\\01\\02\\00\\00))"
        );
        // A group base that excludes the group leaves the user without it.
        let mut elsewhere = domain_directory(Some("513"));
        let mut narrow = config("ldaps://directory.example.org");
        narrow.group_base = Some("OU=Groups,DC=example,DC=org".to_owned());
        let person = lookup_with(&mut elsewhere, narrow, "name", "eve").unwrap();
        assert!(person.groups.is_empty());
    }

    #[test]
    fn a_crafted_object_sid_or_group_id_cannot_widen_the_search() {
        // A group id that is not a number is never searched for.
        for primary in ["513)(objectClass=*", "*", "513\\00"] {
            let mut directory = domain_directory(Some(primary));
            lookup(&mut directory, "name", "eve").unwrap();
            assert_eq!(sid_searches(&directory), 0, "{primary}");
        }
        // A SID that is not a SID is never searched for either; a real one is written as
        // bytes, so no value read from the directory reaches the filter as text.
        let mut directory = domain_directory(Some("513"));
        directory.entries[0]
            .bin_attrs
            .insert("objectSid".to_owned(), vec![b"*)(objectClass=*".to_vec()]);
        let person = lookup(&mut directory, "name", "eve").unwrap();
        assert!(person.groups.is_empty());
        assert_eq!(sid_searches(&directory), 0);
    }

    #[test]
    fn the_rid_to_group_answer_is_kept_per_domain_as_the_cache_says() {
        let ldap = Ldap::new(config("ldaps://directory.example.org")).unwrap();
        let mut directory = domain_directory(Some("513"));
        let first = ldap.found(&mut directory, "eve").unwrap().unwrap();
        let second = ldap.found(&mut directory, "eve").unwrap().unwrap();
        assert_eq!(first.groups, ["Domain Users"]);
        assert_eq!(second.groups, first.groups);
        assert_eq!(
            sid_searches(&directory),
            1,
            "the second read hits the cache"
        );
        // Another domain's RID 513 is its own entry.
        directory.entries[0].bin_attrs.insert(
            "objectSid".to_owned(),
            vec![sid_in(&[21, 99, 22, 33], 1104)],
        );
        let other = ldap.found(&mut directory, "eve").unwrap().unwrap();
        assert_eq!(other.groups, ["Domain Users"]);
        assert_eq!(sid_searches(&directory), 2);
        // With the cache off, every read asks.
        let mut off = config("ldaps://directory.example.org");
        off.cache = CacheSetting::Enabled(false);
        let ldap = Ldap::new(off).unwrap();
        let mut directory = domain_directory(Some("513"));
        ldap.found(&mut directory, "eve").unwrap();
        ldap.found(&mut directory, "eve").unwrap();
        assert_eq!(sid_searches(&directory), 2);
    }
}
