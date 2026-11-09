//! Functions and types useful for implementing a PAM module.

// Temporarily allowed until we get the actual conversation functions hooked up.
#![allow(dead_code)]

use crate::_doc::{guide, linklist, stdlinks};
use crate::constants::{
    AuthnFlags, AuthtokAction, AuthtokFlags, BaseFlags, CredAction, ErrorCode, Result,
};
use crate::handle::ModuleClient;
use std::ffi::CStr;

macro_rules! sm_refs {
    ($sym:ident: $guide:literal) => {
        concat!(
            linklist!($sym: mwg, _std),
            "\n\n",
            guide!(mwg: $guide),
            "\n",
            stdlinks!(3 $sym),
        )
    }
}

/// A trait for a PAM module to implement.
///
/// The default implementations of all these hooks tell PAM to ignore them
/// (i.e., behave as if this module does not exist) by returning [`ErrorCode::Ignore`].
/// Override any functions you wish to handle in your module.
/// After implementing this trait, use the [`pam_export!`](crate::pam_export!) macro
/// to make the functions available to PAM.
///
/// For more information about how to write a module, see "[What is expected
/// of a module?][what]" in the Module Writers' Guide.
#[doc = ""]
#[doc = guide!(what: "mwg-expected-of-module-overview.html")]
#[allow(unused_variables)]
pub trait PamModule<T: ModuleClient> {
    // Functions for auth modules.

    /// Authenticate the user.
    ///
    /// This is probably the first thing you want to implement.
    /// In most cases, you will want to get the user and password,
    /// using [`PamShared::username`](crate::PamShared::username)
    /// and [`ModuleClient::authtok`],
    /// and verify them against something.
    ///
    /// # Returns
    ///
    /// If the password check was successful, return `Ok(())`.
    ///
    /// Sensible error codes to return include:
    ///
    /// - [`ErrorCode::AuthenticationError`]: Generic authentication error
    ///   (like an incorrect password).
    /// - [`ErrorCode::CredentialsInsufficient`]: The application does not have
    ///   sufficient credentials to authenticate the user.
    /// - [`ErrorCode::AuthInfoUnavailable`]: The module was not able to access
    ///   the authentication information, for instance due to a network failure.
    /// - [`ErrorCode::UserUnknown`]: The supplied username is not known by this service.
    /// - [`ErrorCode::MaxTries`]: The user has tried authenticating too many times.
    ///   They should not try again.
    ///
    /// # References
    #[doc = sm_refs!(pam_sm_authenticate: "mwg-expected-of-module-auth.html#mwg-pam_sm_authenticate")]
    fn authenticate(handle: &mut T, args: Vec<&CStr>, flags: AuthnFlags) -> Result<()> {
        Err(ErrorCode::Ignore)
    }

    /// Perform "account management".
    ///
    /// When PAM calls this function, the user has already been authenticated
    /// by an authentication module (either this one or some other module).
    /// This hook can check for other things, for instance:
    ///
    /// - Date/time (keep your kids off the computer at night)
    /// - Remote host (only let employees log in from the office)
    ///
    /// You can also check things like, e.g., password expiration,
    /// and alert that the user change it before continuing,
    /// or really do whatever you want.
    ///
    /// # Returns
    ///
    /// If the user should be allowed to log in, return `Ok(())`.
    ///
    /// Sensible error codes to return include:
    ///
    /// - [`ErrorCode::AccountExpired`]: The user's account has expired.
    /// - [`ErrorCode::AuthenticationError`]: Generic authentication error.
    /// - [`ErrorCode::NewAuthTokRequired`]: The user's authentication token has expired.
    ///   PAM will ask the user to set a new authentication token, which may be handled by
    ///   this module in [`Self::change_authtok`].
    /// - [`ErrorCode::PermissionDenied`]: This one is pretty self-explanatory.
    /// - [`ErrorCode::UserUnknown`]: The supplied username is not known by this service.
    ///
    /// # References
    #[doc = sm_refs!(pam_sm_acct_mgmt: "mwg-expected-of-module-acct.html#mwg-pam_sm_acct_mgmt")]
    fn account_management(handle: &mut T, args: Vec<&CStr>, flags: AuthnFlags) -> Result<()> {
        Err(ErrorCode::Ignore)
    }

    /// Set credentials on this session.
    ///
    /// If an authentication module knows more about the user than just
    /// their authentication token, then it uses this function to provide
    /// that information to the application. It should only be called after
    /// authentication but before a session is established.
    ///
    /// The module should perform the specified `action`.
    ///
    /// # Returns
    ///
    /// If credentials were set successfully, return `Ok(())`.
    ///
    /// Sensible error codes to return include:
    ///
    /// - [`ErrorCode::CredentialsUnavailable`]: The credentials cannot be retrieved.
    /// - [`ErrorCode::CredentialsExpired`]: The credentials have expired.
    /// - [`ErrorCode::CredentialsError`]: Some other error occurred when setting credentials.
    /// - [`ErrorCode::UserUnknown`]: The supplied username is not known by this service.
    ///
    /// # References
    #[doc = sm_refs!(pam_sm_setcred: "mwg-expected-of-module-auth.html#mwg-pam_sm_setcred")]
    fn set_credentials(
        handle: &mut T,
        args: Vec<&CStr>,
        action: CredAction,
        flags: BaseFlags,
    ) -> Result<()> {
        Err(ErrorCode::Ignore)
    }

    // Function for chauthtok modules.

    /// Called to set or reset the user's authentication token.
    ///
    /// PAM calls this function twice in succession.
    ///  1. The first time, the `action` will be
    ///     [`AuthtokAction::Validate`].
    ///     If the new token is acceptable, return success;
    ///     if not, return [`ErrorCode::TryAgain`] to re-prompt the user.
    ///  2. After the preliminary check succeeds, you will be called again
    ///     with the same password and [`AuthtokAction::Update`].
    ///     When this happens, actually change the authentication token.
    ///
    /// The new authentication token will be available in
    /// [`authtok`](ModuleClient::authtok),
    /// and the previous authentication token will be available in
    /// [`old_authtok`](ModuleClient::old_authtok).
    ///
    /// # Returns
    ///
    /// If the authentication token was changed successfully
    /// (or the check passed), return `Ok(())`.
    ///
    /// Sensible error codes to return include:
    ///
    /// - [`ErrorCode::AuthTokError`]: The service could not get the authentication token.
    /// - [`ErrorCode::AuthTokRecoveryError`]: The service could not get the old token.
    /// - [`ErrorCode::AuthTokLockBusy`]: The password cannot be changed because
    ///   the authentication token is currently locked.
    /// - [`ErrorCode::AuthTokDisableAging`]: Aging (expiration) is disabled.
    /// - [`ErrorCode::PermissionDenied`]: What it says on the tin.
    /// - [`ErrorCode::TryAgain`]: When the preliminary check is unsuccessful,
    ///   ask the user for a new authentication token.
    /// - [`ErrorCode::UserUnknown`]: The supplied username is not known by this service.
    ///
    /// # References
    #[doc = sm_refs!(pam_sm_chauthtok: "mwg-expected-of-module-acct.html#mwg-pam_sm_chauthtok")]
    fn change_authtok(
        handle: &mut T,
        args: Vec<&CStr>,
        action: AuthtokAction,
        flags: AuthtokFlags,
    ) -> Result<()> {
        Err(ErrorCode::Ignore)
    }

    // Functions for session modules.

    /// Called when a session is opened.
    ///
    /// # Returns
    ///
    /// If the session was opened successfully, return `Ok(())`.
    ///
    /// A sensible error code to return is:
    ///
    /// - [`ErrorCode::SessionError`]: Cannot make an entry for this session.
    ///
    /// # References
    #[doc = sm_refs!(pam_sm_open_session: "mwg-expected-of-module-session.html#mwg-pam_sm_open_session")]
    fn open_session(handle: &mut T, args: Vec<&CStr>, flags: BaseFlags) -> Result<()> {
        Err(ErrorCode::Ignore)
    }

    /// Called when a session is being terminated.
    ///
    /// # Returns
    ///
    /// If the session was closed successfully, return `Ok(())`.
    ///
    /// A sensible error code to return is:
    ///
    /// - [`ErrorCode::SessionError`]: Cannot remove an entry for this session.
    ///
    /// # References
    #[doc = sm_refs!(pam_sm_close_session: "mwg-expected-of-module-session.html#mwg-pam_sm_close_session")]
    fn close_session(handle: &mut T, args: Vec<&CStr>, flags: BaseFlags) -> Result<()> {
        Err(ErrorCode::Ignore)
    }
}
