//! Whether a login is in the operating system's keyring or keychain, looked at with no secret read into an answer: Windows'
//! Credential Manager, the macOS Keychain, or the Secret Service.
//!
//! [`present`] asks for the entry's attributes and never its secret where the platform lets it (the Keychain is searched for
//! the item and no data is loaded; Windows and the Secret Service answer attributes). A store that cannot give attributes is
//! asked for the secret, which is dropped in the same call. A store that does not answer within the time given is
//! [`Presence::Unknown`], and the thread that asked it is left to end by itself.

use alloc::string::String;
use core::time::Duration;
use std::sync::mpsc;

/// What the keyring says of one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    /// The entry is there.
    Present,
    /// The keyring answered and has no such entry, or there is no keyring on this machine to hold one.
    Absent,
    /// The keyring could not say: it did not answer in time, refused access, or failed. The text is for people and holds
    /// nothing the keyring returned.
    Unknown(String),
}

/// Whether the entry of `service` and `user` is in the keyring, asked for no longer than `timeout`.
///
/// `target` is the Credential Manager's target name for the entry where it is not the store's own (go-keyring's
/// `service:user`); the other stores ignore it.
#[must_use]
pub fn present(service: &str, user: &str, target: Option<&str>, timeout: Duration) -> Presence {
    let (send, receive) = mpsc::channel();
    let (service, user, target) = (
        String::from(service),
        String::from(user),
        target.map(String::from),
    );
    let spawned = std::thread::Builder::new()
        .name("login-keyring".into())
        .spawn(move || {
            let _ = send.send(platform(&service, &user, target.as_deref()));
        });
    if spawned.is_err() {
        return Presence::Unknown("a thread to ask the keyring could not be started".into());
    }
    match receive.recv_timeout(timeout) {
        Ok(presence) => presence,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Presence::Unknown("the keyring did not answer in time".into())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Presence::Unknown("the keyring failed while it was asked".into())
        }
    }
}

/// The first `digits` hexadecimal digits of the SHA-256 of `text`: how Codex and Claude Code tell the folders their
/// logins are kept for apart in a keyring entry's name.
#[must_use]
pub fn digest_prefix(text: &str, digits: usize) -> String {
    use sha2::{Digest, Sha256};
    let hex = data_encoding::HEXLOWER.encode(&Sha256::digest(text.as_bytes()));
    String::from(hex.get(..digits).unwrap_or(&hex))
}

#[cfg(target_os = "macos")]
fn platform(service: &str, user: &str, _target: Option<&str>) -> Presence {
    use security_framework::item::{ItemClass, ItemSearchOptions};
    // errSecItemNotFound.
    const NOT_FOUND: i32 = -25300;
    let found = ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(service)
        .account(user)
        .load_attributes(true)
        .search();
    match found {
        Ok(items) if items.is_empty() => Presence::Absent,
        Ok(_) => Presence::Present,
        Err(error) if error.code() == NOT_FOUND => Presence::Absent,
        Err(_) => Presence::Unknown("the Keychain could not be searched".into()),
    }
}

#[cfg(all(any(windows, unix), not(target_os = "macos")))]
fn platform(service: &str, user: &str, target: Option<&str>) -> Presence {
    use keyring_core::Error;

    #[cfg(windows)]
    let store = windows_native_keyring_store::Store::new();
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    let store = zbus_secret_service_keyring_store::Store::new();
    #[cfg(any(target_os = "ios", target_os = "android"))]
    let store: Result<alloc::sync::Arc<keyring_core::CredentialStore>, Error> =
        Err(Error::NoDefaultStore);

    let Ok(store) = store.map(|store| store as alloc::sync::Arc<keyring_core::CredentialStore>)
    else {
        return Presence::Absent;
    };
    let modifiers: std::collections::HashMap<&str, &str> = match target {
        Some(target) if cfg!(windows) => std::collections::HashMap::from([("target", target)]),
        _ => std::collections::HashMap::new(),
    };
    let Ok(entry) = store.build(service, user, Some(&modifiers)) else {
        return Presence::Unknown("the keyring has no way to name that entry".into());
    };
    let seen = match entry.get_attributes() {
        Err(Error::NotSupportedByStore(_)) => entry.get_secret().map(|_| ()),
        other => other.map(|_| ()),
    };
    match seen {
        Ok(()) => Presence::Present,
        Err(Error::NoEntry | Error::NoDefaultStore) => Presence::Absent,
        Err(Error::NoStorageAccess(_)) => {
            Presence::Unknown("the keyring refused access to the entry".into())
        }
        Err(_) => Presence::Unknown("the keyring failed while it was asked".into()),
    }
}

#[cfg(not(any(windows, unix)))]
fn platform(_service: &str, _user: &str, _target: Option<&str>) -> Presence {
    Presence::Absent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_prefix_is_the_start_of_the_sha256() {
        // sha256("abc")
        assert_eq!(digest_prefix("abc", 16), "ba7816bf8f01cfea");
        assert_eq!(digest_prefix("abc", 8), "ba7816bf");
    }

    #[test]
    fn an_entry_nobody_made_is_absent_or_unknown_and_never_present() {
        let answer = present(
            "coqui-acp-test-no-such-service",
            "coqui-acp-test-no-such-user",
            None,
            Duration::from_secs(5),
        );
        assert!(
            !matches!(answer, Presence::Present),
            "an entry nobody made was found"
        );
    }
}
