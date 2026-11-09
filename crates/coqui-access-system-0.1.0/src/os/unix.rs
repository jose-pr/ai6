//! Unix: accounts and groups through NSS (so SSSD and LDAP accounts appear), passwords through
//! PAM.
#![forbid(unsafe_code)]

use std::ffi::{OsStr, OsString};

use coqui_access::error;
use coqui_access::guatiao::library::ProviderError;
use nonstick::{
    AuthnFlags, ConversationAdapter, ErrorCode, Result as PamResult, Transaction,
    TransactionBuilder,
};
use uzers::User;
use uzers::os::unix::UserExt;

use super::Account;
use crate::SystemAccountsConfig;

/// The first uid the platform's own tools give a person.
#[cfg(target_os = "freebsd")]
pub(crate) const FIRST_PERSON_UID: u32 = 1001;
#[cfg(target_os = "macos")]
pub(crate) const FIRST_PERSON_UID: u32 = 501;
#[cfg(not(any(target_os = "freebsd", target_os = "macos")))]
pub(crate) const FIRST_PERSON_UID: u32 = 1000;

pub(crate) fn by_name(
    name: &str,
    _config: &SystemAccountsConfig,
) -> Result<Option<Account>, ProviderError> {
    Ok(uzers::get_user_by_name(name).and_then(account))
}

pub(crate) fn by_id(
    id: &str,
    _config: &SystemAccountsConfig,
) -> Result<Option<Account>, ProviderError> {
    // Digits only, so `+5` or ` 5` is not uid 5.
    if !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    Ok(id
        .parse()
        .ok()
        .and_then(uzers::get_user_by_uid)
        .and_then(account))
}

/// The account, or `None` for a name that is not UTF-8.
fn account(user: User) -> Option<Account> {
    let name = user.name().to_str()?.to_owned();
    let display_name = user
        .gecos()
        .to_str()
        .and_then(|gecos| gecos.split(',').next())
        .map(str::trim)
        .filter(|full| !full.is_empty())
        .map(str::to_owned);
    let groups = uzers::get_user_groups(user.name(), user.primary_group_id())
        .unwrap_or_default()
        .iter()
        .filter_map(|group| group.name().to_str().map(str::to_owned))
        .collect();
    Some(Account {
        id: user.uid().to_string(),
        name,
        display_name,
        groups,
        uid: Some(user.uid()),
        disabled: false,
    })
}

/// Answers the masked prompt with the password and refuses every other prompt.
struct Answer {
    password: String,
}

impl ConversationAdapter for Answer {
    fn prompt(&self, _request: impl AsRef<OsStr>) -> PamResult<OsString> {
        Err(ErrorCode::ConversationError)
    }

    fn masked_prompt(&self, _request: impl AsRef<OsStr>) -> PamResult<OsString> {
        Ok(OsString::from(&self.password))
    }

    fn error_msg(&self, _message: impl AsRef<OsStr>) {}

    fn info_msg(&self, _message: impl AsRef<OsStr>) {}
}

/// `pam_authenticate`, then `pam_acct_mgmt`, under the configured service.
pub(crate) fn check_password(
    name: &str,
    password: &str,
    config: &SystemAccountsConfig,
) -> Result<bool, ProviderError> {
    let answer = Answer {
        password: password.to_owned(),
    };
    let checked = TransactionBuilder::new_with_service(&config.pam_service)
        .username(name)
        .build(answer.into_conversation())
        .and_then(|mut transaction| {
            transaction.authenticate(AuthnFlags::empty())?;
            transaction.account_management(AuthnFlags::empty())
        });
    match checked {
        Ok(()) => Ok(true),
        Err(code) if is_refusal(code) => Ok(false),
        Err(code) => Err(error::internal(&format!(
            "PAM could not check the password: {code}"
        ))),
    }
}

/// A PAM answer that means "not this password, or not this account now", as opposed to
/// "could not find out".
fn is_refusal(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::AuthenticationError
            | ErrorCode::UserUnknown
            | ErrorCode::MaxTries
            | ErrorCode::PermissionDenied
            | ErrorCode::AccountExpired
            | ErrorCode::NewAuthTokRequired
            | ErrorCode::AuthTokExpired
            | ErrorCode::CredentialsExpired
            | ErrorCode::CredentialsInsufficient
            | ErrorCode::ConversationError
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uid_is_digits_only() {
        let config = SystemAccountsConfig::default();
        for id in ["", "+0", " 0", "0x0", "-1", "4294967296"] {
            assert_eq!(by_id(id, &config).unwrap(), None, "{id:?}");
        }
    }

    #[test]
    fn root_is_found_by_name_and_by_id() {
        let config = SystemAccountsConfig::default();
        let root = by_name("root", &config).unwrap().unwrap();
        assert_eq!(root.id, "0");
        assert_eq!(root.uid, Some(0));
        assert_eq!(by_id("0", &config).unwrap().unwrap().name, "root");
        assert!(!root.groups.is_empty());
        assert_eq!(by_name("no-such-account-x", &config).unwrap(), None);
    }

    #[test]
    fn the_conversation_answers_only_the_masked_prompt() {
        let answer = Answer {
            password: "pw".to_owned(),
        };
        assert_eq!(answer.masked_prompt("Password: ").unwrap(), "pw");
        assert_eq!(answer.prompt("login: "), Err(ErrorCode::ConversationError));
    }
}
