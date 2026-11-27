//! Whether Codex has a login, without starting it, asking its service or showing what it holds.
//!
//! Codex keeps its login where `cli_auth_credentials_store` of its `config.toml` says: `file` (the default) is `auth.json` in
//! its home (`CODEX_HOME`, else `.codex` below the user's home), `keyring` is the operating system's keyring, and `auto` is the
//! keyring with the file as the fallback. A login is also there when `CODEX_API_KEY` or `OPENAI_API_KEY` is set and not empty.
//!
//! The keyring entry is asked for by presence and its secret is never read. Its name (service `Codex Auth`, user `cli|` and the
//! first sixteen digits of the SHA-256 of the home's canonical path) was read from Codex's source and has not been run against
//! a keyring-kept login, so an entry not found where Codex keeps its login in the keyring is unknown, not signed out.
//!
//! The store setting is read from the top of `config.toml` only: a line `cli_auth_credentials_store = "..."` before the first
//! table. A setting given to Codex another way (`-c`) is not seen.

use core::time::Duration;
use std::path::{Path, PathBuf};

use coqui_acp_api::{Environment, SignedInCheck};
use coqui_acp_harness::keyring::{self, Presence};
use coqui_acp_harness::signed_in::{self, FileState};

use crate::accounts::AUTH_FILE;

/// The variables Codex signs in by itself with.
pub const SIGN_IN_VARIABLES: &[&str] = &["CODEX_API_KEY", "OPENAI_API_KEY"];

/// The keyring service Codex keeps its login under.
pub const KEYRING_SERVICE: &str = "Codex Auth";

/// Where Codex keeps its login, from `cli_auth_credentials_store`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    /// `auth.json`.
    File,
    /// The operating system's keyring.
    Keyring,
    /// The keyring, with `auth.json` where the keyring cannot be used.
    Auto,
}

/// How the keyring is asked: the service, the user and the longest to wait.
pub type Keyring<'a> = &'a dyn Fn(&str, &str, Duration) -> Presence;

/// The folder Codex keeps its login in: `CODEX_HOME` of the call's environment, then of this process, then `.codex` below the
/// user's home.
#[must_use]
pub fn home_of(environment: &Environment) -> Option<PathBuf> {
    signed_in::value(environment, "CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| signed_in::home(environment).map(|home| home.join(".codex")))
}

/// The store `config.toml` in `home` names, `File` when it names none, is missing or names one Codex does not have.
#[must_use]
pub fn store_of(home: &Path) -> Store {
    let Ok(text) = std::fs::read_to_string(home.join("config.toml")) else {
        return Store::File;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "cli_auth_credentials_store" {
            continue;
        }
        let value = value.split('#').next().unwrap_or("").trim();
        return match value.trim_matches(['"', '\'']) {
            "keyring" => Store::Keyring,
            "auto" => Store::Auto,
            _ => Store::File,
        };
    }
    Store::File
}

/// The keyring user Codex keeps the login of the folder `home` under.
#[must_use]
pub fn keyring_user(home: &Path) -> String {
    let canonical = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    format!(
        "cli|{}",
        keyring::digest_prefix(&canonical.to_string_lossy(), 16)
    )
}

/// Whether Codex has a login. `keyring` is how the keyring is asked.
#[must_use]
pub fn check(environment: &Environment, wait: Duration, keyring: Keyring<'_>) -> SignedInCheck {
    if let Some(found) = signed_in::by_variable(environment, SIGN_IN_VARIABLES) {
        return found;
    }
    let Some(home) = home_of(environment) else {
        return SignedInCheck::unknown(
            "the folder Codex keeps its login in could not be worked out",
        );
    };
    let store = store_of(&home);
    let file = || signed_in::file_state(&home.join(AUTH_FILE));
    let asked = || keyring(KEYRING_SERVICE, &keyring_user(&home), wait);
    let variables = signed_in::variables_note(SIGN_IN_VARIABLES);
    match store {
        Store::File => match file() {
            FileState::Present => SignedInCheck::signed_in(),
            FileState::Missing => {
                SignedInCheck::signed_out(format!("Codex's auth.json is not there{variables}"))
            }
            FileState::Empty => {
                SignedInCheck::signed_out(format!("Codex's auth.json holds no login{variables}"))
            }
            FileState::Unreadable => SignedInCheck::unknown("Codex's auth.json could not be read"),
        },
        Store::Keyring => from_keyring(asked(), "Codex keeps its login in the keyring"),
        Store::Auto => {
            if file() == FileState::Present {
                return SignedInCheck::signed_in();
            }
            from_keyring(
                asked(),
                "Codex keeps its login in the keyring when it can, and auth.json holds none",
            )
        }
    }
}

/// What the keyring's answer says: an entry is a login, and none, where the name was not measured, is not an answer.
fn from_keyring(presence: Presence, where_it_is: &str) -> SignedInCheck {
    match presence {
        Presence::Present => SignedInCheck::signed_in(),
        Presence::Absent => SignedInCheck::unknown(format!(
            "{where_it_is}, and the keyring has no entry under the name Codex's source gives, which was not run against a keyring-kept login"
        )),
        Presence::Unknown(why) => SignedInCheck::unknown(format!(
            "{where_it_is}, and the keyring could not be asked: {why}"
        )),
    }
}

/// The operating system's keyring, asked for presence.
pub fn real_keyring() -> Keyring<'static> {
    &|service: &str, user: &str, wait: Duration| keyring::present(service, user, None, wait)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coqui_acp_api::SignedInState;

    #[test]
    fn the_store_is_the_top_level_setting_and_defaults_to_the_file() {
        let dir =
            std::env::temp_dir().join(format!("coqui-acp-codex-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let with = |text: &str| {
            std::fs::write(dir.join("config.toml"), text).unwrap();
            store_of(&dir)
        };
        assert_eq!(store_of(&dir), Store::File, "no file");
        assert_eq!(with(""), Store::File);
        assert_eq!(
            with("cli_auth_credentials_store = \"keyring\"\n"),
            Store::Keyring
        );
        assert_eq!(
            with("model = \"x\"\ncli_auth_credentials_store='auto' # note\n"),
            Store::Auto
        );
        assert_eq!(with("cli_auth_credentials_store = \"file\""), Store::File);
        assert_eq!(with("cli_auth_credentials_store = \"other\""), Store::File);
        assert_eq!(
            with("[profiles.p]\ncli_auth_credentials_store = \"keyring\"\n"),
            Store::File,
            "a table's own key is not the top level's"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_state_is_never_signed_out_while_a_keyring_login_may_be_there() {
        let absent = from_keyring(Presence::Absent, "kept in the keyring");
        assert_eq!(absent.state, SignedInState::Unknown);
    }
}
