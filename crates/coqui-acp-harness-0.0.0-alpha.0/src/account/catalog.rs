//! What a harness with accounts and its account provider build the same way from one table of kinds: the kind and sign-in
//! entries, the catalog the harness takes, the offer the provider makes, and the reads of a grant that every harness makes.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use coqui_acp_api::guatiao::library::ProviderError;
use coqui_acp_api::guatiao::{FromValue, Schema, Status, ToValue, Value};
use coqui_acp_api::{
    ASK, AccountCatalog, AccountKindInfo, AccountOffer, Credentials, HARNESS_LOGIN, LoginMethodInfo,
};

use crate::provider::settings_as;

/// A kind of account that takes no settings: the harness's own login.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq)]
#[map(crate = ::coqui_acp_api::guatiao, deny_unknown_fields)]
pub struct NoSettings {}

/// `BAD_VALUE` with `why`.
#[must_use]
pub fn bad(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, why)
}

/// Whether `settings` is the empty map the harness's own login takes.
///
/// # Errors
///
/// `BAD_VALUE` for any setting.
pub fn takes_no_settings(settings: &Value) -> Result<(), ProviderError> {
    settings_as::<NoSettings>(settings)
        .map(drop)
        .map_err(|_| bad("that kind takes no settings"))
}

/// A kind `id` named `title`, with the `settings` schema its accounts hold.
#[must_use]
pub fn kind_info(id: &str, title: &str, description: &str, settings: Value) -> AccountKindInfo {
    let mut kind = AccountKindInfo::new(id, title);
    kind.description = description.to_string();
    kind.settings = settings;
    kind
}

/// A way to sign in that makes a `produces` account.
#[must_use]
pub fn method_info(id: &str, title: &str, description: &str, produces: &str) -> LoginMethodInfo {
    let mut method = LoginMethodInfo::new(id, title, produces);
    method.description = description.to_string();
    method
}

/// A way to sign in whose one step is a form the person fills, which a remote person can: a key or a token.
#[must_use]
pub fn typed_method(id: &str, title: &str, description: &str, produces: &str) -> LoginMethodInfo {
    let mut method = method_info(id, title, description, produces);
    method.steps = vec![ASK.to_string()];
    method.remote = true;
    method
}

/// Sets what each kind of `catalog` provides to the standard `standard_of` names for its id.
pub fn provide_standards<'a>(catalog: &mut AccountCatalog, standard_of: impl Fn(&str) -> &'a str) {
    for kind in &mut catalog.kinds {
        kind.provides = vec![standard_of(&kind.id).to_string()];
    }
}

/// What the harness takes as an account, from the `full` catalog: the kinds it applies and the standards it accepts. It
/// signs no one in and renews nothing: the account provider ([`provider_offer`]) does.
#[must_use]
pub fn harness_catalog(mut full: AccountCatalog) -> AccountCatalog {
    full.logins.clear();
    for kind in &mut full.kinds {
        kind.renews = false;
    }
    full
}

/// What the account provider offers from the `full` catalog: the kinds it makes (the harness's own login, which it cannot
/// sign in to, left out and offered as the automatic `local` account), the ways to sign in, and the kinds in `usage`,
/// whose plan usage it reads.
#[must_use]
pub fn provider_offer(full: AccountCatalog, usage: &[&str]) -> AccountOffer {
    let mut offer = AccountOffer::new();
    offer.kinds = full
        .kinds
        .into_iter()
        .filter(|kind| kind.id != HARNESS_LOGIN)
        .collect();
    offer.logins = full.logins;
    offer.local = true;
    offer.usage = usage
        .iter()
        .map(|id| (*id).to_string())
        .collect::<Vec<String>>();
    offer
}

/// The text of the credential `key` when it is the only one `credentials` holds and is not empty.
#[must_use]
pub fn sole_credential<'a>(credentials: &'a Credentials, key: &str) -> Option<&'a str> {
    credentials
        .get(key)
        .filter(|text| !text.is_empty() && credentials.keys().all(|each| each == key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{schema, to_json};
    use coqui_acp_api::AccountKindInfo;
    use serde_json::json;

    fn full() -> AccountCatalog {
        let mut catalog = AccountCatalog::new();
        let mut login = AccountKindInfo::new("a-login", "A login");
        login.renews = true;
        catalog.kinds.push(login);
        catalog
            .logins
            .push(typed_method("key", "A key", "Asks.", "a-login"));
        catalog
    }

    #[test]
    fn the_harness_takes_kinds_that_renew_nothing_and_lists_no_sign_in() {
        let catalog = harness_catalog(full());
        assert!(catalog.logins.is_empty());
        assert!(catalog.kinds.iter().all(|kind| !kind.renews));
    }

    #[test]
    fn the_provider_offers_every_kind_but_the_own_login_and_says_which_it_reads_usage_of() {
        let offer = provider_offer(full(), &["a-login", HARNESS_LOGIN]);
        assert!(offer.kind(HARNESS_LOGIN).is_none());
        assert!(offer.kind("a-login").is_some_and(|kind| kind.renews));
        assert_eq!(offer.logins.len(), 1);
        assert!(offer.local);
        assert_eq!(offer.usage, ["a-login", HARNESS_LOGIN]);
    }

    #[test]
    fn a_typed_method_asks_and_a_remote_person_can_answer() {
        let method = typed_method("key", "A key", "Asks.", "a-login");
        assert_eq!(method.steps, [ASK]);
        assert!(method.remote);
    }

    #[test]
    fn a_credential_is_the_sole_one_only_when_it_is_the_only_key_and_not_empty() {
        let only = Credentials::from_iter([("k", "v")]);
        assert_eq!(sole_credential(&only, "k"), Some("v"));
        assert_eq!(sole_credential(&only, "other"), None);
        let two = Credentials::from_iter([("k", "v"), ("x", "y")]);
        assert_eq!(sole_credential(&two, "k"), None);
        let blank = Credentials::from_iter([("k", "")]);
        assert_eq!(sole_credential(&blank, "k"), None);
    }

    #[test]
    fn the_own_login_takes_no_setting_at_all() {
        assert!(takes_no_settings(&schema(&json!({}))).is_ok());
        for settings in [json!({"base_url": "https://x.example"}), json!(["x"])] {
            assert!(takes_no_settings(&schema(&settings)).is_err(), "{settings}");
        }
        assert_eq!(
            to_json(&crate::provider::schema_of::<NoSettings>())["additionalProperties"],
            false
        );
    }
}
