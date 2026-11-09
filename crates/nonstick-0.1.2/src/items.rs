use crate::_doc::{guide, linklist, stdlinks};
use crate::constants::Result;
#[cfg(doc)]
use crate::handle::{ModuleClient, PamShared};
use std::ffi::{OsStr, OsString};

macro_rules! getter {
    ($(#[$md:meta])* $getter:ident($item:literal $(, see = $see:path)?)) => {
        $(#[$md])*
        #[doc = ""]
        #[doc = concat!("Gets the `", $item, "` of the PAM handle.")]
        $(
            #[doc = concat!("See [`", stringify!($see), "`].")]
        )?
        fn $getter(&self) -> Result<Option<OsString>>;
    };
}

pub(crate) use getter;
macro_rules! setter {
    ($(#[$md:meta])* $setter:ident($item:literal $(, see = $see:path)?)) => {
        $(#[$md])*
        #[doc = ""]
        #[doc = concat!("Sets the `", $item, "` from the PAM handle.")]
        $(
            #[doc = concat!("See [`", stringify!($see), "`].")]
        )?
        ///
        /// Sets the item's value. PAM copies the string's contents.
        ///
        /// # Panics
        ///
        /// If the string contains a nul byte, this will panic.
        ///
        fn $setter(&mut self, value: Option<&OsStr>) -> Result<()>;
    };
}

/// Provides access to Items, pieces of data shared by the PAM application,
/// modules, and the framework itself.
///
/// # References
#[doc = linklist!(pam_get_item: mwg, adg, _std)]
///
#[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_get_item")]
#[doc = guide!(mwg: "mwg-expected-by-module-item.html#mwg-pam_get_item")]
#[doc = stdlinks!(3 pam_get_item)]
pub trait Items<'a> {
    getter!(
        /// The identity of the user for whom service is being requested.
        ///
        /// Unlike [`username`](PamShared::username), this will simply get
        /// the current state of the user item, and not request the username.
        /// While PAM usually sets this automatically in the `username` call,
        /// it may be changed by a module during the PAM transaction.
        /// Applications should check it after each step of the PAM process.
        user("PAM_USER", see = PamShared::username)
    );

    getter!(
        /// If set, the identity of the remote user logging in.
        ///
        /// This is only as trustworthy as the application calling PAM.
        remote_user("PAM_RUSER", see = Self::remote_host)
    );

    getter!(
        /// If set, the remote location where the user is coming from.
        ///
        /// This is only as trustworthy as the application calling PAM.
        /// This can be combined with [`Self::remote_user`] to identify
        /// the account the user is attempting to log in from,
        /// with `remote_user@remote_host`.
        ///
        /// If unset, "it is unclear where the authentication request
        /// is originating from."
        remote_host("PAM_RHOST", see = Self::remote_user)
    );

    getter!(
        /// The service name, which identifies the PAM stack which is used
        /// to perform authentication.
        service("PAM_SERVICE")
    );

    getter!(
        /// The string used to prompt for a user's name.
        /// By default, this is a localized version of `login: `.
        user_prompt("PAM_USER_PROMPT")
    );

    getter!(
        /// The device path of the TTY being used to log in.
        ///
        /// This is the terminal the user is logging in on,
        /// specified as the full device path (e.g. `/dev/tty0`).
        /// Very old applications may use this instead of `PAM_XDISPLAY`.
        tty_name("PAM_TTY")
    );
}

/// Provides write access to PAM Items, data shared by the PAM application,
/// the framework, and modules.
///
/// # References
#[doc = linklist!(pam_set_item: mwg, adg, _std)]
///
#[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_set_item")]
#[doc = guide!(mwg: "mwg-expected-by-module-item.html#mwg-pam_set_item")]
#[doc = stdlinks!(3 pam_set_item)]
pub trait ItemsMut<'a>: Items<'a> {
    setter!(
        /// Sets the identity of the logging-in user.
        ///
        /// Usually this will be set during the course of
        /// a [`username`](PamShared::username) call, but you may set it manually
        /// or change it during the PAM process.
        set_user("PAM_USER", see = Items::user)
    );

    setter!(
        /// Sets the service name. It's probably a bad idea to change this.
        set_service("PAM_SERVICE", see = Items::service)
    );

    setter!(
        /// Sets the string used to prompt for a user's name.
        set_user_prompt("PAM_USER_PROMPT", see = Items::user_prompt)
    );

    setter!(
        /// Sets the path to the terminal where the user is logging on.
        set_tty_name("PAM_TTY", see = Items::tty_name)
    );

    setter!(
        /// Sets the identity of the remote user logging in.
        ///
        /// This may be set by the application before making calls
        /// into a PAM transaction.
        set_remote_user("PAM_RUSER", see = Items::remote_user)
    );

    setter!(
        /// Sets the location where the user is coming from.
        ///
        /// This may be set by the application before making calls
        /// into a PAM transaction.
        set_remote_host("PAM_RHOST", see = Items::remote_host)
    );

    setter!(
        /// Gets the user's authentication token (e.g., password).
        ///
        /// This is usually set automatically when
        /// [`authtok`](ModuleClient::authtok) is called,
        /// but can be manually set.
        set_authtok("PAM_AUTHTOK", see = ModuleClient::authtok_item)
    );
    setter!(
        /// Sets the user's "old authentication token" when changing passwords.
        ///
        /// This is usually set automatically by PAM when
        /// [`old_authtok`](ModuleClient::old_authtok) is called.
        set_old_authtok("PAM_OLDAUTHTOK", see = ModuleClient::old_authtok_item)
    );
}
