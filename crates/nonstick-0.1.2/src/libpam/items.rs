use crate::constants::ErrorCode;
use crate::constants::Result;
use crate::items::{Items, ItemsMut};
use crate::libpam::handle::LibPamHandle;
use crate::libpam::memory;
use std::ffi::{c_int, OsStr, OsString};
use std::ptr;

memory::num_enum! {
    /// Identifies what is being gotten or set with `pam_get_item`
    /// or `pam_set_item`.
    #[non_exhaustive]
    pub enum ItemType {
        /// The PAM service name.
        Service = libpam_sys::PAM_SERVICE,
        /// The user's login name.
        User = libpam_sys::PAM_USER,
        /// The TTY name.
        Tty = libpam_sys::PAM_TTY,
        /// The remote host (if applicable).
        RemoteHost = libpam_sys::PAM_RHOST,
        /// The conversation struct (not a CStr-based item).
        Conversation = libpam_sys::PAM_CONV,
        /// The authentication token (password).
        AuthTok = libpam_sys::PAM_AUTHTOK,
        /// The old authentication token (when changing passwords).
        OldAuthTok = libpam_sys::PAM_OLDAUTHTOK,
        /// The remote user's name.
        RemoteUser = libpam_sys::PAM_RUSER,
        /// The prompt shown when requesting a username.
        UserPrompt = libpam_sys::PAM_USER_PROMPT,
        #[cfg(feature = "linux-pam-ext")]
        /// App-supplied function to override failure delays.
        FailDelay = libpam_sys::PAM_FAIL_DELAY,
        #[cfg(feature = "linux-pam-ext")]
        /// X display name.
        XDisplay = libpam_sys::PAM_XDISPLAY,
        #[cfg(feature = "linux-pam-ext")]
        /// X server authentication data.
        XAuthData = libpam_sys::PAM_XAUTHDATA,
        #[cfg(feature = "linux-pam-ext")]
        /// The type of `pam_get_authtok`.
        AuthTokType = libpam_sys::PAM_AUTHTOK_TYPE,
    }
}

pub struct LibPamItems<'a>(pub &'a LibPamHandle);
pub struct LibPamItemsMut<'a>(pub &'a mut LibPamHandle);

/// Macro to implement getting/setting a CStr-based item.
macro_rules! cstr_item {
    (get = $getter:ident, item = $item_type:path) => {
        fn $getter(&self) -> Result<Option<OsString>> {
            unsafe { get_cstr_item(&self.0, $item_type) }
        }
    };
    (set = $setter:ident, item = $item_type:path) => {
        fn $setter(&mut self, value: Option<&OsStr>) -> Result<()> {
            unsafe { set_cstr_item(&mut self.0, $item_type, value) }
        }
    };
}

impl Items<'_> for LibPamItems<'_> {
    cstr_item!(get = user, item = ItemType::User);
    cstr_item!(get = service, item = ItemType::Service);
    cstr_item!(get = user_prompt, item = ItemType::UserPrompt);
    cstr_item!(get = tty_name, item = ItemType::Tty);
    cstr_item!(get = remote_user, item = ItemType::RemoteUser);
    cstr_item!(get = remote_host, item = ItemType::RemoteHost);
}

impl Items<'_> for LibPamItemsMut<'_> {
    cstr_item!(get = user, item = ItemType::User);
    cstr_item!(get = service, item = ItemType::Service);
    cstr_item!(get = user_prompt, item = ItemType::UserPrompt);
    cstr_item!(get = tty_name, item = ItemType::Tty);
    cstr_item!(get = remote_user, item = ItemType::RemoteUser);
    cstr_item!(get = remote_host, item = ItemType::RemoteHost);
}

impl ItemsMut<'_> for LibPamItemsMut<'_> {
    cstr_item!(set = set_user, item = ItemType::User);
    cstr_item!(set = set_service, item = ItemType::Service);
    cstr_item!(set = set_user_prompt, item = ItemType::UserPrompt);
    cstr_item!(set = set_tty_name, item = ItemType::Tty);
    cstr_item!(set = set_remote_user, item = ItemType::RemoteUser);
    cstr_item!(set = set_remote_host, item = ItemType::RemoteHost);
    cstr_item!(set = set_authtok, item = ItemType::AuthTok);
    cstr_item!(set = set_old_authtok, item = ItemType::OldAuthTok);
}

/// Gets a C string item.
///
/// # Safety
///
/// You better be requesting an item which is a C string.
pub unsafe fn get_cstr_item(hdl: &LibPamHandle, item_type: ItemType) -> Result<Option<OsString>> {
    let mut output = ptr::null();
    let ret = unsafe { libpam_sys::pam_get_item(hdl.inner(), item_type as c_int, &mut output) };
    ErrorCode::result_from(ret)?;
    Ok(memory::copy_pam_string(output.cast()))
}

/// Sets a C string item.
///
/// # Safety
///
/// You better be setting an item which is a C string.
pub unsafe fn set_cstr_item(
    hdl: &mut LibPamHandle,
    item_type: ItemType,
    data: Option<&OsStr>,
) -> Result<()> {
    let data_str = memory::option_cstr_os(data);
    let ret = unsafe {
        libpam_sys::pam_set_item(
            hdl.inner_mut(),
            item_type as c_int,
            memory::prompt_ptr(data_str.as_deref()).cast(),
        )
    };
    ErrorCode::result_from(ret)
}
