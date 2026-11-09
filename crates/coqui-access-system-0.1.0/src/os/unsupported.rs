//! Targets with no account database this crate reaches: every call is unsupported.
#![forbid(unsafe_code)]

use coqui_access::error;
use coqui_access::guatiao::library::ProviderError;

use super::Account;
use crate::SystemAccountsConfig;

fn unsupported() -> ProviderError {
    error::unsupported("this platform's accounts are not supported")
}

pub(crate) fn by_name(
    _name: &str,
    _config: &SystemAccountsConfig,
) -> Result<Option<Account>, ProviderError> {
    Err(unsupported())
}

pub(crate) fn by_id(
    _id: &str,
    _config: &SystemAccountsConfig,
) -> Result<Option<Account>, ProviderError> {
    Err(unsupported())
}

pub(crate) fn check_password(
    _name: &str,
    _password: &str,
    _config: &SystemAccountsConfig,
) -> Result<bool, ProviderError> {
    Err(unsupported())
}
