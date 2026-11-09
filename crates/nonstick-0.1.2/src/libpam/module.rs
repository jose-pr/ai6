use crate::constants::{ErrorCode, RawFlags, Result};
use crate::libpam::handle::LibPamHandle;
use crate::module::PamModule;
use crate::{AuthnFlags, AuthtokAction, BaseFlags, CredAction};
use std::ffi::{c_char, c_int, c_void, CStr};

/// Generates the dynamic library entry points for a PAM module
///
/// Calling `pam_export!(SomeType)` on a type that implements
/// [`PamModule`] will generate the exported
/// `extern "C"` functions that PAM uses to call into your module.
///
/// ## Examples:
///
/// Here is full example of a PAM module that would authenticate
/// and authorize everybody:
///
/// ```no_run
/// use nonstick::{
///     pam_export, AuthnFlags, ConversationAdapter, LibPamTransaction, ModuleClient, PamModule,
///     Result as PamResult,
/// };
/// use std::ffi::CStr;
/// # fn main() {}
///
/// struct MyPamModule;
/// pam_export!(MyPamModule);
///
/// impl<T: ModuleClient> PamModule<T> for MyPamModule {
///     fn authenticate(handle: &mut T, args: Vec<&CStr>, flags: AuthnFlags) -> PamResult<()> {
///         let password = handle.authtok(Some("what's your password?".as_ref()))?;
///         let response =
///             format!("If you say your password is {password:?}, who am I to disagree?");
///         handle.info_msg(&response);
///         Ok(())
///     }
///
///     fn account_management(
///         handle: &mut T,
///         args: Vec<&CStr>,
///         flags: AuthnFlags,
///     ) -> PamResult<()> {
///         let username = handle.username(None)?;
///         let response = format!("Hello {username:?}! I trust you unconditionally.");
///         handle.info_msg(&response);
///         Ok(())
///     }
/// }
/// ```
#[macro_export]
macro_rules! pam_export {
    ($ident:ident) => {
        mod __pam_export_scope {
            use std::ffi::{c_char, c_int, c_void};
            use $crate::constants::{RawFlags, ReturnCode};
            use $crate::libpam::module;

            macro_rules! export {
                ($func:ident) => {
                    #[no_mangle]
                    unsafe extern "C" fn $func(
                        pamh: *mut c_void,
                        flags: RawFlags,
                        argc: c_int,
                        argv: *const *const c_char,
                    ) -> c_int {
                        let ret: ReturnCode =
                            module::$func::<super::$ident>(pamh, flags, argc, argv).into();
                        ret.into()
                    }
                };
            }

            export!(pam_sm_acct_mgmt);
            export!(pam_sm_authenticate);
            export!(pam_sm_chauthtok);
            export!(pam_sm_close_session);
            export!(pam_sm_open_session);
            export!(pam_sm_setcred);
        }
    };
}

#[doc(hidden)]
pub unsafe fn pam_sm_acct_mgmt<M: PamModule<LibPamHandle>>(
    pamh: *mut c_void,
    flags: RawFlags,
    argc: c_int,
    argv: *const *const c_char,
) -> Result<()> {
    let handle = wrap(pamh)?;
    let args = extract_argv(argc, argv);
    M::account_management(handle, args, AuthnFlags::from(flags))
}

#[doc(hidden)]
pub unsafe fn pam_sm_authenticate<M: PamModule<LibPamHandle>>(
    pamh: *mut c_void,
    flags: RawFlags,
    argc: c_int,
    argv: *const *const c_char,
) -> Result<()> {
    let handle = wrap(pamh)?;
    let args = extract_argv(argc, argv);
    M::authenticate(handle, args, AuthnFlags::from(flags))
}

#[doc(hidden)]
pub unsafe fn pam_sm_chauthtok<M: PamModule<LibPamHandle>>(
    pamh: *mut c_void,
    flags: RawFlags,
    argc: c_int,
    argv: *const *const c_char,
) -> Result<()> {
    let handle = wrap(pamh)?;
    let (action, flags) = AuthtokAction::extract(flags)?;
    let args = extract_argv(argc, argv);
    M::change_authtok(handle, args, action, flags)
}

#[doc(hidden)]
pub unsafe fn pam_sm_close_session<M: PamModule<LibPamHandle>>(
    pamh: *mut c_void,
    flags: RawFlags,
    argc: c_int,
    argv: *const *const c_char,
) -> Result<()> {
    let handle = wrap(pamh)?;
    let args = extract_argv(argc, argv);
    M::close_session(handle, args, BaseFlags::from(flags))
}

#[doc(hidden)]
pub unsafe fn pam_sm_open_session<M: PamModule<LibPamHandle>>(
    pamh: *mut c_void,
    flags: RawFlags,
    argc: c_int,
    argv: *const *const c_char,
) -> Result<()> {
    let handle = wrap(pamh)?;
    let args = extract_argv(argc, argv);
    M::open_session(handle, args, BaseFlags::from(flags))
}

#[doc(hidden)]
pub unsafe fn pam_sm_setcred<M: PamModule<LibPamHandle>>(
    pamh: *mut c_void,
    flags: RawFlags,
    argc: c_int,
    argv: *const *const c_char,
) -> Result<()> {
    let handle = wrap(pamh)?;
    let (action, flags) = CredAction::extract(flags)?;
    let args = extract_argv(argc, argv);
    M::set_credentials(handle, args, action, flags)
}

/// Turns `argc`/`argv` into a [Vec] of [CStr]s.
///
/// # Safety
///
/// We use this only with arguments we get from `libpam`, which we kind of have to trust.
unsafe fn extract_argv<'a>(argc: c_int, argv: *const *const c_char) -> Vec<&'a CStr> {
    (0..argc)
        .map(|o| unsafe { CStr::from_ptr(*argv.offset(o as isize) as *const c_char) })
        .collect()
}

/// Wraps the pointer in a PAM handle, or returns an error if it's null.
///
/// # Safety
///
/// It's up to you to pass a valid handle.
unsafe fn wrap<'a>(handle: *mut c_void) -> Result<&'a mut LibPamHandle> {
    handle
        .cast::<LibPamHandle>()
        .as_mut()
        .ok_or(ErrorCode::SystemError)
}

#[cfg(test)]
mod tests {
    // Compile-time test that the `pam_hooks` macro compiles.
    use crate::{ModuleClient, PamModule};
    struct Foo;
    impl<T: ModuleClient> PamModule<T> for Foo {}

    pam_export!(Foo);
}
