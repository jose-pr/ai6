//! Convenience aliases for complex types in PAM.

use super::{pam_conv, pam_handle, pam_message, pam_response, pam_set_data};
use std::ffi::{c_int, c_void};

/// The type of [`pam_conv::conv`].
///
/// The exact format of `messages` varies between Linux-PAM and other
/// implementations. See `libpam_sys_helpers::PtrPtrVec` for details
/// (and a workaround).
///
/// ```no_run
/// use libpam_sys::aliases::ConversationCallback;
/// use libpam_sys::pam_conv;
/// fn convo() -> ConversationCallback {
///     // ...
/// #    unimplemented!()
/// }
/// let conv = pam_conv {
///     conv: convo(),
///     appdata_ptr: std::ptr::null_mut(),
/// };
/// ```
pub type ConversationCallback = unsafe extern "C" fn(
    num_msg: c_int,
    msg: *const *const pam_message,
    resp: *mut *mut pam_response,
    appdata: *mut c_void,
) -> c_int;

/// Alias for the callback to [`pam_set_data`].
///
/// ```no_run
/// # use std::ffi::CString;
/// use libpam_sys::aliases::CleanupCallback;
/// use libpam_sys::pam_set_data;
/// # use libpam_sys::pam_handle;
/// # let handle: *mut pam_handle = std::ptr::null_mut();
/// # let mut my_data = 100;
/// # let data_ptr = &mut my_data as *mut i32;
/// fn cleanup() -> CleanupCallback {
///     // ...
/// #    unimplemented!()
/// }
/// let name = CString::new("name").unwrap();
/// unsafe {
///     pam_set_data(handle, name.as_ptr().cast_mut(), data_ptr.cast(), cleanup());
/// }
/// ```
pub type CleanupCallback =
    unsafe extern "C" fn(pamh: *mut pam_handle, data: *mut c_void, pam_end_status: c_int);
