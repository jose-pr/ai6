use std::ffi::{c_char, c_int, c_uint, c_void};
use std::fmt;
use std::marker::{PhantomData, PhantomPinned};

/// An opaque structure that PAM uses to communicate.
///
/// This is only ever returned in pointer form and cannot be constructed.
#[repr(C)]
pub struct pam_handle {
    _value: (),
    _marker: PhantomData<(PhantomPinned, *mut c_void)>,
}

impl fmt::Debug for pam_handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pam_handle({self:p}")
    }
}

/// Used by PAM to communicate between the module and the application.
#[repr(C)]
#[derive(Debug)]
pub struct pam_conv {
    pub conv: unsafe extern "C" fn(
        num_msg: c_int,
        msg: *const *const pam_message,
        resp: *mut *mut pam_response,
        appdata: *mut c_void,
    ) -> c_int,
    pub appdata_ptr: *mut c_void,
}

/// A message sent into a PAM conversation.
#[repr(C)]
#[derive(Debug)]
pub struct pam_message {
    pub msg_style: c_int,
    pub msg: *const c_char,
}

/// A response returned from a PAM conversation.
#[repr(C)]
#[derive(Debug)]
pub struct pam_response {
    pub resp: *mut c_char,
    /// Completely unused.
    pub resp_retcode: c_int,
}

/// Definition of the PAM_XAUTHDATA item. Compatible with `xcb_auth_info_t`.
#[cfg(pam_impl = "LinuxPam")]
#[repr(C)]
pub struct pam_xauth_data {
    pub namelen: c_int,
    pub name: *mut c_char,
    pub datalen: c_int,
    pub data: *mut c_char,
}

#[cfg(pam_impl = "LinuxPam")]
#[derive(Debug)]
#[repr(C)]
pub struct pam_modutil_privs {
    pub grplist: *mut libc::gid_t,
    pub number_of_groups: c_int,
    pub allocated: c_int,
    pub old_gid: libc::gid_t,
    pub old_uid: libc::uid_t,
    pub is_dropped: c_int,
}

#[cfg(pam_impl = "OpenPam")]
pub type pam_func_t = unsafe extern "C" fn(
    handle: *mut pam_handle,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int;

#[cfg(pam_impl = "OpenPam")]
#[derive(Debug)]
#[repr(C)]
pub struct pam_module {
    pub path: *mut c_char,
    pub func: [pam_func_t; 6],
    pub dlh: *mut c_void,
}

#[cfg(any(pam_impl = "OpenPam", pam_impl = "Sun"))]
#[derive(Debug)]
#[repr(C)]
pub struct pam_repository {
    pub type_: *mut c_char,
    pub scope: *mut c_void,
    pub scope_len: usize,
}

// These are the functions specified in X/SSO. Everybody exports them.
extern "C" {
    /// Account validation.
    pub fn pam_acct_mgmt(pamh: *mut pam_handle, flags: c_int) -> c_int;

    /// Authenticate a user.
    pub fn pam_authenticate(pamh: *mut pam_handle, flags: c_int) -> c_int;

    // Nobody implements pam_authenticate_secondary.

    /// Manage authentication tokens.
    pub fn pam_chauthtok(pamh: *mut pam_handle, flags: c_int) -> c_int;

    /// Close an opened user session.
    pub fn pam_close_session(pamh: *mut pam_handle, flags: c_int) -> c_int;

    /// Ends the PAM transaction.
    pub fn pam_end(pamh: *mut pam_handle, flags: c_int) -> c_int;

    /// Gets module-specific data. PAM still owns the data.
    pub fn pam_get_data(
        pamh: *const pam_handle,
        module_data_name: *const c_char,
        data: *mut *const c_void,
    ) -> c_int;

    /// Gets an environment variable.  You own the return value.
    pub fn pam_getenv(pamh: *const pam_handle, name: *const c_char) -> *mut c_char;

    /// Gets all the environment variables.  You own everything it points to.
    pub fn pam_getenvlist(pamh: *const pam_handle) -> *mut *mut c_char;

    /// Get information about the transaction.
    ///
    /// The item is owned by PAM.
    pub fn pam_get_item(
        pamh: *const pam_handle,
        item_type: c_int,
        item: *mut *const c_void,
    ) -> c_int;

    // Nobody implements pam_get_mapped_authtok.
    // Nobody implements pam_get_mapped_username.

    /// Get the username. PAM owns it.
    pub fn pam_get_user(
        pamh: *mut pam_handle,
        user: *mut *const c_char,
        prompt: *const c_char,
    ) -> c_int;

    /// Opens a user session.
    pub fn pam_open_session(pamh: *mut pam_handle, flags: c_int) -> c_int;

    /// Sets the value of an environment variable. `namevalue` is copied.
    pub fn pam_putenv(pamh: *mut pam_handle, namevalue: *const c_char) -> c_int;

    /// Update or delete user credentials.
    pub fn pam_setcred(pamh: *mut pam_handle, flags: c_int) -> c_int;

    /// Set module-specific data. PAM will call `cleanup` when completed.
    pub fn pam_set_data(
        pamh: *mut pam_handle,
        module_data_name: *const c_char,
        data: *mut c_void,
        cleanup: unsafe extern "C" fn(
            pamh: *mut pam_handle,
            data: *mut c_void,
            pam_end_status: c_int,
        ),
    ) -> c_int;

    /// Set information about the transaction.  The `item` is copied.
    pub fn pam_set_item(pamh: *mut pam_handle, item_type: c_int, item: *const c_void) -> c_int;

    // Nobody implements pam_set_mapped_authtok.
    // Nobody implements pam_set_mapped_username.

    // The pam_sm_whatever functions are prototypes for the functions that
    // a PAM module should implement, not symbols provided by PAM.

    /// Starts a PAM transaction.  The `conv` may or may not be copied.
    pub fn pam_start(
        service: *const c_char,
        user: *const c_char,
        pam_conv: *mut pam_conv,
        pamh: *mut *mut pam_handle,
    ) -> c_int;

    /// Gets a statically-allocated error string.
    ///
    /// All implementations of PAM known to this library (Linux-PAM, OpenPAM,
    /// and Sun) ignore `pamh` and will accept a null pointer.
    pub fn pam_strerror(pamh: *const pam_handle, error_number: c_int) -> *mut c_char;
}

#[cfg(any(pam_impl = "LinuxPam", pam_impl = "OpenPam"))]
extern "C" {
    /// Gets `PAM_AUTHTOK`, or asks the user if that is unset.
    pub fn pam_get_authtok(
        pamh: *mut pam_handle,
        item: c_int,
        authtok: *mut *const c_char,
        prompt: *const c_char,
    ) -> c_int;

    pub fn pam_prompt(
        pamh: *const pam_handle,
        style: c_int,
        response: *mut *mut c_char,
        fmt: *const c_char,
        ...
    ) -> c_int;

}

#[cfg(pam_impl = "LinuxPam")]
extern "C" {
    pub fn pam_fail_delay(pamh: *mut pam_handle, musec_delay: c_uint) -> c_int;

    /// Start a PAM transaction based on configuration in the given directory.
    pub fn pam_start_confdir(
        service_name: *const c_char,
        user: *const c_char,
        pam_conversation: *mut pam_conv,
        confdir: *const c_char,
        pamh: *mut *mut pam_handle,
    ) -> c_int;

    // We don't export the v-variants of the formatting functions.

    pub fn pam_syslog(pamh: *const pam_handle, priority: c_int, fmt: *const c_char, ...);

    pub fn pam_get_authtok_noverify(
        pamh: *const pam_handle,
        authtok: *mut *const c_char,
        prompt: *const c_char,
    ) -> c_int;

    pub fn pam_get_authtok_verify(
        pamh: *const pam_handle,
        authtok: *mut *const c_char,
        prompt: *const c_char,
    ) -> c_int;

    // pam_modutil also lives in libpam for Linux.

    pub fn pam_modutil_check_user_in_passwd(
        pamh: *mut pam_handle,
        user_name: *const c_char,
        file_name: *const c_char,
    ) -> c_int;

    pub fn pam_modutil_getpwnam(pamh: *mut pam_handle, user: *const c_char) -> *mut libc::passwd;

    pub fn pam_modutil_getpwuid(pamh: *mut pam_handle, uid: libc::uid_t) -> *mut libc::passwd;

    pub fn pam_modutil_getgrnam(pamh: *mut pam_handle, group: *const c_char) -> *mut libc::group;

    pub fn pam_modutil_getgrgid(pamh: *mut pam_handle, gid: libc::gid_t) -> *mut libc::group;

    pub fn pam_modutil_getspnam(pamh: *mut pam_handle, user: *const c_char) -> *mut libc::spwd;

    pub fn pam_modutil_user_in_group_nam_nam(
        pamh: *mut pam_handle,
        user: *const c_char,
        group: *const c_char,
    ) -> c_int;

    pub fn pam_modutil_user_in_group_nam_gid(
        pamh: *mut pam_handle,
        user: *const c_char,
        group: libc::gid_t,
    ) -> c_int;

    pub fn pam_modutil_user_in_group_uid_nam(
        pamh: *mut pam_handle,
        user: libc::uid_t,
        group: *const c_char,
    ) -> c_int;

    pub fn pam_modutil_user_in_group_uid_gid(
        pamh: *mut pam_handle,
        user: libc::uid_t,
        group: libc::gid_t,
    ) -> c_int;

    pub fn pam_modutil_getlogin(pamh: *mut pam_handle) -> *const c_char;

    pub fn pam_modutil_read(fd: c_int, buffer: *mut c_char, count: c_int) -> c_int;

    pub fn pam_modutil_write(fd: c_int, buffer: *const c_char, count: c_int) -> c_int;

    pub fn pam_modutil_audit_write(
        pamh: *mut pam_handle,
        type_: c_int,
        message: *const c_char,
        retval: c_int,
    ) -> c_int;

    pub fn pam_modutil_drop_priv(
        pamh: *mut pam_handle,
        p: *mut pam_modutil_privs,
        pw: *const libc::passwd,
    ) -> c_int;

    pub fn pam_modutil_regain_priv(pamh: *mut pam_handle, p: *mut pam_modutil_privs) -> c_int;

    pub fn pam_modutil_sanitize_helper_fds(
        pamh: *mut pam_handle,
        redirect_stdin: super::constants::pam_modutil_redirect_fd,
        redirect_stdout: super::constants::pam_modutil_redirect_fd,
        redirect_stderr: super::constants::pam_modutil_redirect_fd,
    ) -> c_int;

    pub fn pam_modutil_search_key(
        pamh: *mut pam_handle,
        file_name: *const c_char,
        key: *const c_char,
    ) -> *mut c_char;
}

#[cfg(pam_impl = "OpenPam")]
extern "C" {
    pub fn openpam_borrow_cred(pamh: *mut pam_handle, passwd: *const libc::passwd) -> c_int;

    pub fn openpam_subst(
        pamh: *const pam_handle,
        buf: *mut c_char,
        _bufsize: *mut usize,
        _template: *const c_char,
    ) -> c_int;

    pub fn openpam_free_data(pamh: *mut pam_handle, data: *mut c_void, status: c_int);

    pub fn openpam_free_envlist(_envlist: *mut *mut c_char);

    pub fn openpam_get_option(_pamh: *mut pam_handle, _option: *const c_char) -> *const c_char;

    pub fn openpam_restore_cred(pamh: *mut pam_handle) -> c_int;

    pub fn openpam_set_option(
        _pamh: *mut pam_handle,
        _option: *const c_char,
        _value: *const c_char,
    ) -> c_int;

    pub fn pam_error(pamh: *const pam_handle, _fmt: *const c_char, ...) -> c_int;

    pub fn pam_info(_pamh: *const pam_handle, _fmt: *const c_char, ...) -> c_int;

    pub fn openpam_readline(
        _f: *mut libc::FILE,
        _lineno: *mut c_int,
        _lenp: *mut usize,
    ) -> *mut c_char;

    pub fn openpam_readlinev(
        _f: *mut libc::FILE,
        _lineno: *mut c_int,
        _lenp: *mut c_int,
    ) -> *mut *mut c_char;

    pub fn openpam_readword(
        _f: *mut libc::FILE,
        _lineno: *mut c_int,
        _lenp: *mut usize,
    ) -> *mut c_char;

    pub fn openpam_straddch(
        _str: *mut *mut c_char,
        _sizep: *mut usize,
        _lenp: *mut usize,
        ch: c_int,
    ) -> c_int;

    pub fn openpam_set_feature(_feature: c_int, _onoff: c_int) -> c_int;

    pub fn openpam_get_feature(_feature: c_int, _onoff: *mut c_int) -> c_int;

    pub fn _openpam_log(_level: c_int, _func: *const c_char, _fmt: *const c_char, ...);

    /// A premade conversation function that talks to the TTY.
    ///
    /// ```no_run
    /// # use std::ffi::CString;
    /// # use std::ptr;
    /// use libpam_sys::*;
    /// # let service = CString::new("whatever").unwrap();
    /// # let user = CString::new("whatever").unwrap();
    /// let mut handle: *mut pam_handle = ptr::null_mut();
    /// let mut conv = pam_conv {
    ///     conv: openpam_ttyconv,
    ///     appdata_ptr: ptr::null_mut(),
    /// };
    /// let result = unsafe { pam_start(service.as_ptr(), user.as_ptr(), &mut conv, &mut handle) };
    /// ```
    pub fn openpam_ttyconv(
        n: c_int,
        _msg: *const *const pam_message,
        _resp: *mut *mut pam_response,
        _data: *mut c_void,
    ) -> c_int;

    pub static mut openpam_ttyconv_timeout: c_int;

    /// A null conversation function.
    ///
    /// ```no_run
    /// # use std::ffi::CString;
    /// # use std::ptr;
    /// use libpam_sys::*;
    /// # let service = CString::new("whatever").unwrap();
    /// # let user = CString::new("whatever").unwrap();
    /// let mut handle: *mut pam_handle = ptr::null_mut();
    /// let mut conv = pam_conv {
    ///     conv: openpam_nullconv,
    ///     appdata_ptr: ptr::null_mut(),
    /// };
    /// let result = unsafe { pam_start(service.as_ptr(), user.as_ptr(), &mut conv, &mut handle) };
    /// ```
    pub fn openpam_nullconv(
        n: c_int,
        _msg: *const *const pam_message,
        _resp: *mut *mut pam_response,
        _data: *mut c_void,
    ) -> c_int;
}

#[cfg(pam_impl = "Sun")]
extern "C" {
    pub fn __pam_get_authtok(
        pamh: *mut pam_handle,
        source: c_int,
        type_: c_int,
        prompt: *const c_char,
        authtok: *mut *mut c_char,
    ) -> c_int;

    pub fn __pam_log(priority: c_int, format: *const c_char, ...);
}
