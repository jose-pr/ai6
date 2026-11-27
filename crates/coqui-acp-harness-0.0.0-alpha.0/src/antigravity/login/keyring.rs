//! The login's entry in the operating system's keyring, as go-keyring writes it for agy.

use alloc::vec::Vec;
use core::time::Duration;
use std::io;
use std::sync::mpsc;

use super::Keyring;

/// The entry of `service` and `user` in `keyring`, `None` when there is none or no keyring can be reached here (agy then
/// keeps its login in a file). A keyring that does not answer within `timeout` is an error, and the thread asking it is left
/// to end on its own.
pub(super) fn read(
    keyring: &Keyring,
    timeout: Duration,
    service: &str,
    user: &str,
) -> io::Result<Option<Vec<u8>>> {
    match keyring {
        Keyring::Off => Ok(None),
        Keyring::Given(answer) => answer(service, user),
        Keyring::Os => {
            let (send, receive) = mpsc::channel();
            let (service, user) = (
                alloc::string::String::from(service),
                alloc::string::String::from(user),
            );
            std::thread::Builder::new()
                .name("agy-keyring".into())
                .spawn(move || {
                    let _ = send.send(platform(&service, &user));
                })?;
            match receive.recv_timeout(timeout) {
                Ok(answer) => answer,
                Err(mpsc::RecvTimeoutError::Timeout) => Err(io::ErrorKind::TimedOut.into()),
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::ErrorKind::Other.into()),
            }
        }
    }
}

/// go-keyring's names: on Windows the generic credential `service:user`, elsewhere the service and user themselves.
#[cfg(any(windows, unix))]
fn platform(service: &str, user: &str) -> io::Result<Option<Vec<u8>>> {
    use keyring_core::Error;

    #[cfg(windows)]
    let store = windows_native_keyring_store::Store::new();
    #[cfg(target_os = "macos")]
    let store = apple_native_keyring_store::keychain::Store::new();
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
        return Ok(None);
    };
    let target = alloc::format!("{service}:{user}");
    let modifiers: std::collections::HashMap<&str, &str> = if cfg!(windows) {
        std::collections::HashMap::from([("target", target.as_str())])
    } else {
        std::collections::HashMap::new()
    };
    let entry = store
        .build(service, user, Some(&modifiers))
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    match entry.get_secret() {
        Ok(bytes) => Ok(Some(bytes)),
        Err(Error::NoEntry | Error::NoDefaultStore | Error::NotSupportedByStore(_)) => Ok(None),
        Err(Error::NoStorageAccess(_)) => Err(io::ErrorKind::PermissionDenied.into()),
        Err(_) => Err(io::ErrorKind::Other.into()),
    }
}

#[cfg(not(any(windows, unix)))]
fn platform(_service: &str, _user: &str) -> io::Result<Option<Vec<u8>>> {
    Ok(None)
}
