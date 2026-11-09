//! The operating system's own accounts as an authenticator and a directory.
#![doc = include_str!("../README.md")]
#![deny(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]
#![warn(
    clippy::std_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::alloc_instead_of_core
)]

mod os;

use core::fmt;

use coqui_access::guatiao::library::ProviderError;
use coqui_access::guatiao::{FromValue, Provider, Schema, ToValue};
use coqui_access::{
    Attempt, Authenticator, Description, Directory, Lookup, Person, Verdict, error, takes,
};

/// The PAM service checked unless the configuration names another.
pub const DEFAULT_PAM_SERVICE: &str = "coqui-access";
/// The lowest uid admitted unless the configuration says otherwise: the first uid this
/// platform gives a person (Linux 1000, FreeBSD 1001, macOS 501; 0 elsewhere).
pub const DEFAULT_MIN_UID: u32 = os::FIRST_PERSON_UID;
/// The Windows domain an account belongs to unless the configuration names another: `.`,
/// the machine's own accounts.
pub const DEFAULT_DOMAIN: &str = ".";
/// Whether this build can reach the operating system's accounts: Unix and Windows.
pub const SUPPORTED: bool = os::SUPPORTED;

/// Options of [`SystemAccounts`], as a host's configuration holds them. Every key is optional.
#[derive(Debug, Clone, PartialEq, Eq, FromValue, Schema, ToValue)]
#[map(crate = ::coqui_access::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct SystemAccountsConfig {
    /// The PAM service a password is checked under (Unix). With no file of that name in
    /// `/etc/pam.d`, Linux-PAM uses `other`, which refuses everyone on most distributions.
    #[map(default = String::from(DEFAULT_PAM_SERVICE))]
    pub pam_service: String,
    /// The lowest uid admitted (Unix). `0` admits root.
    #[map(default = DEFAULT_MIN_UID)]
    pub min_uid: u32,
    /// The groups an account must belong to one of; empty admits any.
    #[map(default)]
    pub groups: Vec<String>,
    /// The Windows domain accounts belong to; `.` is the machine's own accounts.
    #[map(default = String::from(DEFAULT_DOMAIN))]
    pub domain: String,
}

impl Default for SystemAccountsConfig {
    fn default() -> Self {
        Self {
            pam_service: DEFAULT_PAM_SERVICE.to_owned(),
            min_uid: DEFAULT_MIN_UID,
            groups: Vec::new(),
            domain: DEFAULT_DOMAIN.to_owned(),
        }
    }
}

/// A setting that admits more than the defaults do. Reported, never refused: the host
/// decides what to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NonDefault {
    /// `min_uid` is 0 on Unix: root can sign in.
    AdmitsRoot,
    /// `min_uid` is below [`DEFAULT_MIN_UID`] but not 0 on Unix: service accounts can sign in.
    AdmitsServiceAccounts,
}

impl fmt::Display for NonDefault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AdmitsRoot => "min_uid 0 admits root",
            Self::AdmitsServiceAccounts => "min_uid admits service accounts",
        })
    }
}

impl SystemAccountsConfig {
    /// What these options admit beyond the defaults, on this platform. Empty with the
    /// defaults.
    pub fn non_defaults(&self) -> Vec<NonDefault> {
        // Windows accounts carry no uid, so `min_uid` admits nothing there.
        #[cfg(unix)]
        match self.min_uid {
            0 => vec![NonDefault::AdmitsRoot],
            uid if uid < DEFAULT_MIN_UID => vec![NonDefault::AdmitsServiceAccounts],
            _ => Vec::new(),
        }
        #[cfg(not(unix))]
        Vec::new()
    }

    fn check(&self) -> Result<(), ProviderError> {
        let plain = |text: &str| !text.is_empty() && !text.chars().any(char::is_control);
        // Windows accounts carry no uid: a threshold there would silently admit everyone.
        #[cfg(not(unix))]
        if self.min_uid != DEFAULT_MIN_UID {
            return Err(error::bad_value(
                "min_uid has no meaning here: Windows accounts carry no uid",
            ));
        }
        if !plain(&self.pam_service) || self.pam_service.contains('/') {
            return Err(error::bad_value(
                "pam_service must be a service name: not empty, no '/' and no control characters",
            ));
        }
        if !plain(&self.domain) {
            return Err(error::bad_value(
                "domain must not be empty or hold control characters",
            ));
        }
        if !self.groups.iter().all(|group| plain(group)) {
            return Err(error::bad_value(
                "a group must not be empty or hold control characters",
            ));
        }
        Ok(())
    }
}

/// Options for [`SystemAccounts`]; every one has a default.
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct SystemAccountsBuilder {
    config: SystemAccountsConfig,
}

impl SystemAccountsBuilder {
    /// The PAM service passwords are checked under. Default [`DEFAULT_PAM_SERVICE`].
    pub fn pam_service(mut self, service: impl Into<String>) -> Self {
        self.config.pam_service = service.into();
        self
    }

    /// The lowest uid admitted. Default [`DEFAULT_MIN_UID`]; `0` admits root.
    pub fn min_uid(mut self, uid: u32) -> Self {
        self.config.min_uid = uid;
        self
    }

    /// The groups an account must belong to one of. Default: none, which admits any.
    pub fn groups(mut self, groups: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.config.groups = groups.into_iter().map(Into::into).collect();
        self
    }

    /// The Windows domain accounts belong to. Default [`DEFAULT_DOMAIN`].
    pub fn domain(mut self, domain: impl Into<String>) -> Self {
        self.config.domain = domain.into();
        self
    }

    /// Makes the provider. Nothing is looked up yet.
    ///
    /// # Errors
    ///
    /// Unsupported on a platform with no implementation ([`SUPPORTED`] is false); a bad
    /// value for an empty or malformed service, domain or group.
    pub fn build(self) -> Result<SystemAccounts, ProviderError> {
        if !SUPPORTED {
            return Err(error::unsupported(
                "this platform's accounts are not supported",
            ));
        }
        self.config.check()?;
        Ok(SystemAccounts {
            config: self.config,
        })
    }
}

/// The operating system's accounts: who may sign in is who the OS lets sign in, narrowed by
/// [`SystemAccountsConfig::min_uid`] and [`SystemAccountsConfig::groups`].
///
/// Read only: accounts are managed with the OS's own tools. Every call blocks on the OS
/// (PAM may sleep seconds after a wrong password), so a host calls it from a blocking thread.
#[derive(Debug, Provider)]
#[provider(
    Authenticator,
    Directory,
    id = "coqui_access_system",
    name = "System accounts",
    config = SystemAccountsConfig,
    crate = ::coqui_access::guatiao
)]
pub struct SystemAccounts {
    config: SystemAccountsConfig,
}

impl SystemAccounts {
    /// Options for the OS's accounts, each at its default.
    pub fn builder() -> SystemAccountsBuilder {
        SystemAccountsBuilder::default()
    }

    /// The provider these options describe.
    ///
    /// # Errors
    ///
    /// As [`SystemAccountsBuilder::build`].
    pub fn new(config: SystemAccountsConfig) -> Result<Self, ProviderError> {
        SystemAccountsBuilder { config }.build()
    }

    /// The options in force.
    pub fn config(&self) -> &SystemAccountsConfig {
        &self.config
    }

    /// The account `lookup` names, when the OS has it and these options admit it.
    fn admitted(&self, lookup: &Lookup) -> Result<Option<os::Account>, ProviderError> {
        if lookup.value.is_empty() || lookup.value.contains('\0') {
            return Ok(None);
        }
        let found = match lookup.by.as_str() {
            "id" => os::by_id(&lookup.value, &self.config)?,
            "name" => os::by_name(&lookup.value, &self.config)?,
            _ => None,
        };
        Ok(found.filter(|account| account.admitted_by(&self.config)))
    }
}

impl TryFrom<SystemAccountsConfig> for SystemAccounts {
    type Error = ProviderError;

    fn try_from(config: SystemAccountsConfig) -> Result<Self, Self::Error> {
        Self::new(config)
    }
}

impl os::Account {
    fn admitted_by(&self, config: &SystemAccountsConfig) -> bool {
        self.uid.is_none_or(|uid| uid >= config.min_uid)
            && (config.groups.is_empty()
                || self
                    .groups
                    .iter()
                    .any(|group| config.groups.iter().any(|allowed| allowed == group)))
    }

    fn person(self) -> Person {
        Person {
            id: self.id,
            name: self.name,
            display_name: self.display_name,
            email: None,
            groups: self.groups,
            attributes: Default::default(),
        }
    }
}

impl Authenticator for SystemAccounts {
    /// A name the OS does not have, or these options do not admit, is `Unknown`; a wrong
    /// password, or a disabled, locked or expired account, is `Invalid`; success names the
    /// account `by: "id"` (the uid, or the SID on Windows).
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        let (Some(name), Some(password)) = (attempt.name, attempt.secret) else {
            return Ok(Verdict::Unknown);
        };
        let lookup = Lookup {
            by: "name".to_owned(),
            value: name,
        };
        let Some(account) = self.admitted(&lookup)? else {
            return Ok(Verdict::Unknown);
        };
        if account.disabled || password.is_empty() || password.contains('\0') {
            return Ok(Verdict::Invalid);
        }
        Ok(
            if os::check_password(&account.name, &password, &self.config)? {
                Verdict::Valid(Lookup {
                    by: "id".to_owned(),
                    value: account.id,
                })
            } else {
                Verdict::Invalid
            },
        )
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description {
            name: Some("System accounts".to_owned()),
            takes: Some(takes::PASSWORD.to_owned()),
        })
    }
}

impl Directory for SystemAccounts {
    /// By `id` (uid, or SID on Windows) or `name`; a disabled or unadmitted account is
    /// `None`.
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        Ok(self
            .admitted(&lookup)?
            .filter(|account| !account.disabled)
            .map(os::Account::person))
    }

    fn describe(&self) -> Result<Description, ProviderError> {
        Ok(Description {
            name: Some("System accounts".to_owned()),
            takes: None,
        })
    }
}

#[cfg(test)]
mod tests;
