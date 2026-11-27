//! Whether Claude Code has a login, without starting it, asking its service or showing what it holds.
//!
//! A login is there when one of the variables Claude Code signs in by is set and not empty, when `.credentials.json` of its
//! configuration directory (`CLAUDE_CONFIG_DIR`, else `.claude` below the home) holds something, or, on macOS, when the
//! Keychain has Claude Code's item. The Keychain is asked whether the item exists and its secret is not loaded. The item's
//! name is `Claude Code-credentials`, with `-` and the first eight digits of the SHA-256 of `CLAUDE_CONFIG_DIR` after it when
//! that is set, and its account is the user; it was read from Claude Code's source and has not been run against a Mac, so an
//! item not found there is unknown, not signed out.

use alloc::collections::BTreeMap;
use core::time::Duration;
use std::ffi::OsString;

use coqui_acp_api::{Environment, SignedInCheck, SignedInState};
use coqui_acp_harness::keyring::{self, Presence};
use coqui_acp_harness::signed_in;

use crate::accounts::CREDENTIALS_FILE;
use crate::config_dir;

/// The variables Claude Code signs in by itself with: an API key, a long-lived OAuth token from `claude setup-token`, and a
/// bearer token for a gateway.
pub const SIGN_IN_VARIABLES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
];

/// The Keychain service Claude Code keeps its login under.
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// The Keychain service for the configuration directory `CLAUDE_CONFIG_DIR` names, or the default one when it is not set.
#[must_use]
pub fn keychain_service(config_dir_variable: Option<&str>) -> String {
    match config_dir_variable.filter(|text| !text.is_empty()) {
        Some(dir) => format!("{KEYCHAIN_SERVICE}-{}", keyring::digest_prefix(dir, 8)),
        None => KEYCHAIN_SERVICE.to_string(),
    }
}

/// How the Keychain is asked: the service, the account and the longest to wait.
pub type Keychain<'a> = &'a dyn Fn(&str, &str, Duration) -> Presence;

/// Whether Claude Code has a login. `keychain` is how the Keychain is asked where there is one (macOS), and `None` elsewhere.
#[must_use]
pub fn check(
    environment: &Environment,
    wait: Duration,
    keychain: Option<Keychain<'_>>,
) -> SignedInCheck {
    let variables: BTreeMap<OsString, OsString> = environment
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect();
    let file = config_dir(&variables, false).map(|dir| dir.join(CREDENTIALS_FILE));
    let answer = signed_in::answer_for_file(
        environment,
        SIGN_IN_VARIABLES,
        file.as_deref(),
        "Claude Code's credentials file",
    );
    let Some(keychain) = keychain else {
        return answer;
    };
    if answer.state == SignedInState::SignedIn {
        return answer;
    }
    let Some(user) = ["USER", "LOGNAME"]
        .iter()
        .find_map(|name| signed_in::value(environment, name))
    else {
        return SignedInCheck::unknown(
            "the user the Keychain item is kept for could not be worked out, and no credentials file holds a login",
        );
    };
    let service = keychain_service(signed_in::value(environment, "CLAUDE_CONFIG_DIR").as_deref());
    match keychain(&service, &user, wait) {
        Presence::Present => SignedInCheck::signed_in(),
        Presence::Absent => SignedInCheck::unknown(
            "no credentials file holds a login and the Keychain has no item under the name Claude Code's source gives, which was not run on a Mac",
        ),
        Presence::Unknown(why) => SignedInCheck::unknown(format!(
            "no credentials file holds a login, and the Keychain could not be asked: {why}"
        )),
    }
}

/// The macOS Keychain's lookup, and `None` on every other system.
#[must_use]
pub fn real_keychain() -> Option<Keychain<'static>> {
    if cfg!(target_os = "macos") {
        Some(&|service: &str, user: &str, wait: Duration| {
            keyring::present(service, user, None, wait)
        })
    } else {
        None
    }
}
