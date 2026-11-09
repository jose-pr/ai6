//! The wrapper types and traits for handles into the PAM library.

use crate::_doc::{guide, linklist, man7, manbsd, stdlinks};
use crate::constants::{AuthnFlags, AuthtokFlags, Result};
use crate::conv::Conversation;
use crate::environ::{EnvironMap, EnvironMapMut};
use crate::items::{getter, Items, ItemsMut};
use crate::logging::Logger;
use std::ffi::{OsStr, OsString};

/// Functionality for both PAM applications and PAM modules.
///
/// This base trait includes features of a PAM handle that are available
/// to both applications and modules.
///
/// You probably want [`LibPamTransaction`](crate::libpam::LibPamTransaction).
/// This trait is intended to allow creating mock PAM handle types
/// to test PAM modules and applications.
pub trait PamShared: Logger {
    /// The contents of the environment to set for the logged-in user.
    ///
    /// # References
    #[doc = linklist!(pam_getenv: adg, mwg, _std)]
    ///
    #[doc = stdlinks!(3 pam_getenv)]
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_getenv")]
    #[doc = guide!(mwg: "mwg-expected-by-module-item.html#adg-pam_getenv")]
    fn environ(&self) -> impl EnvironMap;

    /// A writable map of the environment to set for the logged-in user.
    ///
    /// # References
    #[doc = linklist!(pam_putenv: adg, mwg, _std)]
    ///
    #[doc = stdlinks!(3 pam_putenv)]
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_putenv")]
    #[doc = guide!(mwg: "mwg-expected-by-module-item.html#adg-pam_putenv")]
    fn environ_mut(&mut self) -> impl EnvironMapMut;

    /// Gets Items, data shared by PAM, the application, and modules.
    ///
    /// Certain Items should not be accessed by a PAM application;
    /// those are available directly on [`ModuleClient`] for use
    /// by PAM modules only.
    ///
    /// # References
    #[doc = linklist!(pam_get_item: mwg, adg, _std)]
    ///
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_get_item")]
    #[doc = guide!(mwg: "mwg-expected-by-module-item.html#mwg-pam_get_item")]
    #[doc = stdlinks!(3 pam_get_item)]
    fn items(&self) -> impl Items;

    /// Read-write access to PAM Items.
    ///
    /// # References
    #[doc = linklist!(pam_set_item: mwg, adg, _std)]
    ///
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_set_item")]
    #[doc = guide!(mwg: "mwg-expected-by-module-item.html#mwg-pam_set_item")]
    #[doc = stdlinks!(3 pam_set_item)]
    fn items_mut(&mut self) -> impl ItemsMut;
}

/// Functionality of a PAM handle that can be expected by a PAM application.
///
/// If you are not writing a PAM client application (e.g., you are writing
/// a module), you should not use the functionality exposed by this trait.
///
/// Like [`PamShared`], this is intended to allow creating mock implementations
/// of PAM for testing PAM applications.
pub trait Transaction: PamShared {
    /// Starts the authentication process for the user.
    ///
    /// The application calls this to find out who the user is, and verify that
    /// they are really that person. If authentication is successful,
    /// this will return an `Ok(())` [`Result`].
    ///
    /// A PAM module may change the caller's [username](PamShared::username)
    /// as part of the login process, so be sure to check it after making
    /// any PAM application call.
    ///
    /// # References
    #[doc = linklist!(pam_authenticate: adg, _std)]
    ///
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_authenticate")]
    #[doc = stdlinks!(3 pam_authenticate)]
    fn authenticate(&mut self, flags: AuthnFlags) -> Result<()>;

    /// Verifies the validity of the user's account (and other stuff).
    ///
    /// After [authentication](Self::authenticate), an application should call
    /// this to ensure that the user's account is still valid. This may check
    /// for token expiration or that the user's account is not locked.
    ///
    /// # References
    #[doc = linklist!(pam_acct_mgmt: adg, _std)]
    ///
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_acct_mgmt")]
    #[doc = stdlinks!(3 pam_acct_mgmt)]
    fn account_management(&mut self, flags: AuthnFlags) -> Result<()>;

    /// Changes the authentication token.
    ///
    /// # References
    #[doc = linklist!(pam_chauthtok: adg, _std)]
    ///
    #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_chauthtok")]
    #[doc = stdlinks!(3 pam_chauthtok)]
    fn change_authtok(&mut self, flags: AuthtokFlags) -> Result<()>;
}

/// Functionality of a PAM handle that can be expected by a PAM module.
///
/// If you are not writing a PAM module (e.g., you are writing an application),
/// you should not use any of the functionality exposed by this trait.
///
/// Like [`PamShared`], this is intended to allow creating mock implementations
/// of PAM for testing PAM modules.
pub trait ModuleClient: Conversation + PamShared {
    /// Retrieves the name of the user who is authenticating or logging in.
    ///
    /// If the username has previously been obtained, this uses that username;
    /// otherwise it prompts the user with the first of these that is present:
    ///
    ///  1. The prompt string passed to this function.
    ///  2. The string returned by `get_user_prompt_item`.
    ///  3. The default prompt, `login: `.
    ///
    /// # References
    #[doc = linklist!(pam_get_user: mwg, _std)]
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nonstick::ModuleClient;
    /// # fn _doc(handle: &mut impl ModuleClient) -> Result<(), Box<dyn std::error::Error>> {
    /// // Get the username using the default prompt.
    /// let user = handle.username(None)?;
    /// // Get the username using a custom prompt.
    /// // If this were actually called right after the above,
    /// // both user and user_2 would have the same value.
    /// let user_2 = handle.username(Some("who ARE you even???".as_ref()))?;
    /// # Ok(())
    /// # }
    /// ```
    #[doc = stdlinks!(3 pam_get_user)]
    #[doc = guide!(mwg: "mwg-expected-by-module-item.html#mwg-pam_get_user")]
    fn username(&mut self, prompt: Option<&OsStr>) -> Result<OsString>;

    /// Retrieves the authentication token from the user.
    ///
    /// This should only be used by *authentication* and *password-change*
    /// PAM modules.
    ///
    /// With Sun's PAM implementation, this works a little bit differently
    /// than it does everywhere else. Sun's PAM provides for password input
    /// *exclusively* though module stacking with the
    /// [`pam_authtok_get` module][pam_authtok_get]. On Sun, this function
    /// is exactly equivalent to [`Self::authtok_item`], in that it only
    /// retrieves the existing item.
    ///
    /// # References
    #[doc = linklist!(pam_get_authtok: man7, manbsd)]
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nonstick::handle::ModuleClient;
    /// # fn _doc(handle: &mut impl ModuleClient) -> Result<(), Box<dyn std::error::Error>> {
    /// // Get the user's password using the default prompt.
    /// let pass = handle.authtok(None)?;
    /// // Get the user's password using a custom prompt.
    /// let pass = handle.authtok(Some("Reveal your secrets!".as_ref()))?;
    /// Ok(())
    /// # }
    /// ```
    #[doc = man7!(3 pam_get_authtok)]
    #[doc = manbsd!(3 pam_get_authtok)]
    /// [pam_authtok_get]: https://smartos.org/man/7/pam_authtok_get
    fn authtok(&mut self, prompt: Option<&OsStr>) -> Result<OsString>;

    /// Retrieves the user's old authentication token when changing passwords.
    ///
    /// This should only be used by a *password-change* module.
    ///
    /// # References
    #[doc = linklist!(pam_get_authtok: man7, manbsd)]
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nonstick::handle::ModuleClient;
    /// # fn _doc(handle: &mut impl ModuleClient) -> Result<(), Box<dyn std::error::Error>> {
    /// // Get the user's password using the default prompt.
    /// let pass = handle.old_authtok(None)?;
    /// // Get the user's password using a custom prompt.
    /// let pass = handle.old_authtok(Some("Reveal your secrets!".as_ref()))?;
    /// Ok(())
    /// # }
    /// ```
    #[doc = stdlinks!(3 pam_get_authtok)]
    fn old_authtok(&mut self, prompt: Option<&OsStr>) -> Result<OsString>;

    /// Gets an item of module-specific data stored over the transaction.
    ///
    /// This gives you a reference to the data that was earlier set with
    /// [`Self::set_module_data`]. If not present, you get `None`.
    ///
    /// Data is in a module-specific, type-specific namespace.
    ///
    /// ```
    /// # use nonstick::ModuleClient;
    /// # use std::path::PathBuf;
    /// # fn test(client: &impl ModuleClient) {
    /// // These two can coexist and do not overlap.
    /// let str_data: Option<&String> = client.get_module_data("the_key");
    /// let num_data: Option<&u64> = client.get_module_data("the_key");
    /// // ...
    /// let nothing_data: Option<&PathBuf> = client.get_module_data("this does not exist");
    /// # }
    /// ```
    ///
    /// # References
    #[doc = linklist!(pam_get_data: mwg, _std)]
    ///
    #[doc = guide!(mwg: "mwg-expected-by-module-item.html#mwg-pam_get_data")]
    #[doc = stdlinks!(3 pam_get_data)]
    fn get_module_data<T: 'static>(&self, key: &str) -> Option<&T>;

    /// Sets module-specific data.
    ///
    /// A PAM module may need to store data across multiple invocations within
    /// the same PAM transaction. For instance, a module that stores credentials
    /// would need to know where those credentials were stored in order to
    /// update or destroy them later. Also see [`Self::get_module_data`].
    ///
    /// Module-specific data gives a module a way to store such data.
    /// Data are stored in a module-specific, type-specific namespace.
    ///
    /// PAM takes ownership of the data passed in. See the **Cleanup** section
    /// below for details on how cleanup is handled.
    ///
    /// # Examples
    ///
    /// Each type of data is in a separate namespace:
    ///
    /// ```
    /// # use nonstick::{ModuleClient, Result};
    /// # fn test(client: &mut impl ModuleClient) -> Result<()> {
    /// client.set_module_data("count", 999i32)?;
    ///
    /// let count_int: Option<&i32> = client.get_module_data("count");
    /// // count_int = Some(&999i32)
    /// let count_string: Option<&String> = client.get_module_data("count");
    /// // count_string = None
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Data persist across invocations of the same module:
    ///
    /// ```
    /// # use nonstick::{ModuleClient, Result};
    /// // In a pam_authenticate call, this function is called:
    /// fn authenticate(client: &mut impl ModuleClient) -> Result<()> {
    ///     client.set_module_data::<u64>("TOKEN_ID", 0x0fa1afe10000beef)?;
    ///     Ok(())
    /// }
    ///
    /// // Later, in a pam_session_start call:
    /// fn start_session(client: &mut impl ModuleClient) -> Result<()> {
    ///     match client.get_module_data::<u64>("TOKEN_ID") {
    ///         Some(&tid) => {
    ///             // This will execute and tid will be 0x0fa1afe10000beef.
    ///         }
    ///         None => { /* This will not execute. */ }
    ///     }
    ///     Ok(())
    /// }
    /// ```
    ///
    /// Each module has its own set of data:
    ///
    /// ```
    /// # use nonstick::{ModuleClient, Result};
    /// // This function is called somewhere in pam_module_a.so.
    /// fn in_pam_module_a(client: &mut impl ModuleClient) -> Result<()> {
    ///     client.set_module_data("value", String::from("pam_module_a data"))?;
    ///     Ok(())
    /// }
    ///
    /// // This function is called later in pam_module_b.so.
    /// fn in_pam_module_b(client: &mut impl ModuleClient) -> Result<()> {
    ///     match client.get_module_data::<String>("value") {
    ///         Some(value) => {
    ///             // This will match, because pam_module_a's data
    ///             // is completely unrelated to pam_module_b's data.
    ///         }
    ///         None => {
    ///             // This branch will execute.
    ///         }
    ///     }
    ///     // ...
    /// #    Ok(())
    /// }
    /// ```
    ///
    /// # Cleanup
    ///
    /// PAM modules should be careful about cleaning up data outside their own
    /// address space, because PAM applications may `fork()`:
    ///
    /// ```plain
    /// ┃ let tx = start_pam_transaction();
    /// ┃
    /// ┃ tx.authenticate();
    /// ┃ │ // PAM calls into your module where you set data:
    /// ┃ │ handle.set_module_data("key", the_data);
    /// ┃
    /// ┃ fork();
    /// ┣━━━━━━━━━━━━━━━━━━━━━━━━━━┓
    /// Parent process             Child process
    /// ┃ wait(child);             ┃ setuid(user_to_login);
    /// ┃ ┆                        ┃ // ... do other stuff ...
    /// ┃ ┆                        ┃ drop(tx);
    /// ┃ ┆                        ┃ │ // PAM cleans up your data.
    /// ┃ ┆                        ┃ │ drop(the_data);
    /// ┃ ┆                        ┗ exec(user's shell)
    /// ┃ ┆                          ┃ // user does stuff over their session
    /// ┃ ┆                          ┃ // ...
    /// ┃ ┆                          X
    /// ┃
    /// ┃ drop(tx);
    /// ┃ │ // Parent PAM cleans up your data.
    /// ┃ │ drop(the_data);  // Called again, but in this process instead!
    /// ```
    ///
    /// While LibPAM offers a way to customize the action taken on cleanup,
    /// we do not (yet) offer this.
    ///
    /// # References
    #[doc = linklist!(pam_set_data: mwg, _std)]
    ///
    #[doc = guide!(mwg: "mwg-expected-by-module-item.html#mwg-pam_set_data")]
    #[doc = stdlinks!(3 pam_set_data)]
    fn set_module_data<T: 'static>(&mut self, key: &str, data: T) -> Result<()>;

    getter!(
        /// Gets the user's authentication token (e.g., password).
        ///
        /// This is normally set automatically by PAM through [`Self::authtok`],
        /// but this will get its value (if set) without prompting the user.
        ///
        /// Like `authtok`, this should only ever be called
        /// by *authentication* and *password-change* PAM modules.
        ///
        /// # References
        ///
        #[doc = linklist!(pam_set_item: mwg, adg, _std)]
        ///
        #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_set_item")]
        #[doc = guide!(mwg: "mwg-expected-by-module-item.html#mwg-pam_set_item")]
        #[doc = stdlinks!(3 pam_set_item)]
        authtok_item("PAM_AUTHTOK", see = Self::authtok)
    );

    getter!(
        /// Gets the user's old authentication token when changing passwords.
        ///
        /// This is normally set automatically by PAM through
        /// [`Self::old_authtok`], but this will get its value (if set)
        /// without prompting the user.
        ///
        /// This should only ever be called by *password-change* PAM modules.
        ///
        /// # References
        ///
        #[doc = linklist!(pam_set_item: mwg, adg, _std)]
        ///
        #[doc = guide!(adg: "adg-interface-by-app-expected.html#adg-pam_set_item")]
        #[doc = guide!(mwg: "mwg-expected-by-module-item.html#mwg-pam_set_item")]
        #[doc = stdlinks!(3 pam_set_item)]
        old_authtok_item("PAM_OLDAUTHTOK", see = ItemsMut::set_old_authtok)
    );
}
