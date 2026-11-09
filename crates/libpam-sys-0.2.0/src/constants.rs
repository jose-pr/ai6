//! All of `libpam`'s constants.
//!
//! These constants are tested on a per-platform basis by `libpam-sys-test`'s
//! `test_constants.rs`.

#![allow(non_camel_case_types)]

/// Macro to make defining a bunch of constants way easier.
macro_rules! define {
    ($(#[$attr:meta])* $($name:ident = $value:expr);+$(;)?) => {
        define!(
            @meta { $(#[$attr])* }
            $(pub const $name: i32 = $value;)+
        );
    };
    (@meta $m:tt $($i:item)+) => { define!(@expand $($m $i)+); };
    (@expand $({ $(#[$m:meta])* } $i:item)+) => {$($(#[$m])* $i)+};
}

/// Macro to make defining C-style enums way easier.
macro_rules! c_enum {
    ($(#[$attr:meta])* $($name:ident $(= $value:expr)?,)*) => {
        c_enum!(
            (0)
            $(#[$attr])*
            $($name $(= $value)?,)*
        );
    };
    (($n:expr) $(#[$attr:meta])* $name:ident, $($rest:ident $(= $rv:expr)?,)*) => {
        $(#[$attr])* pub const $name: i32 = $n;
        c_enum!(($n + 1) $(#[$attr])* $($rest $(= $rv)?,)*);
    };
    (($n:expr) $(#[$attr:meta])* $name:ident = $value:expr, $($rest:ident $(= $rv:expr)?,)*) => {
        $(#[$attr])* pub const $name: i32 = $value;
        c_enum!(($value + 1) $(#[$attr])* $($rest $(= $rv)?,)*);
    };
    (($n:expr) $(#[$attr:meta])*) => {};
}

// There are a few truly universal constants.
// They are defined here directly.
/// The successful return code.
pub const PAM_SUCCESS: i32 = 0;

c_enum!(
    /// An item type.
    PAM_SERVICE = 1,
    PAM_USER,
    PAM_TTY,
    PAM_RHOST,
    PAM_CONV,
    PAM_AUTHTOK,
    PAM_OLDAUTHTOK,
    PAM_RUSER,
    PAM_USER_PROMPT,
);

c_enum!(
    /// A message style.
    PAM_PROMPT_ECHO_OFF = 1,
    PAM_PROMPT_ECHO_ON,
    PAM_ERROR_MSG,
    PAM_TEXT_INFO,
);

define!(
    /// Maximum size of PAM conversation elements (suggested).
    PAM_MAX_NUM_MSG = 32;
    PAM_MAX_MSG_SIZE = 512;
    PAM_MAX_RESP_SIZE = 512;
);

/// A flag for `pam_authenticate`.
pub const PAM_DISALLOW_NULL_AUTHTOK: i32 = 0x1;

#[cfg(pam_impl = "LinuxPam")]
pub use linux_pam::*;
#[cfg(pam_impl = "LinuxPam")]
mod linux_pam {
    c_enum!(
        /// An error return code.
        PAM_OPEN_ERR = 1,
        PAM_SYMBOL_ERR,
        PAM_SERVICE_ERR,
        PAM_SYSTEM_ERR,
        PAM_BUF_ERR,
        PAM_PERM_DENIED,
        PAM_AUTH_ERR,
        PAM_CRED_INSUFFICIENT,
        PAM_AUTHINFO_UNAVAIL,
        PAM_USER_UNKNOWN,
        PAM_MAXTRIES,
        PAM_NEW_AUTHTOK_REQD,
        PAM_ACCT_EXPIRED,
        PAM_SESSION_ERR,
        PAM_CRED_UNAVAIL,
        PAM_CRED_EXPIRED,
        PAM_CRED_ERR,
        PAM_NO_MODULE_DATA,
        PAM_CONV_ERR,
        PAM_AUTHTOK_ERR,
        PAM_AUTHTOK_RECOVERY_ERR,
        PAM_AUTHTOK_LOCK_BUSY,
        PAM_AUTHTOK_DISABLE_AGING,
        PAM_TRY_AGAIN,
        PAM_IGNORE,
        PAM_ABORT,
        PAM_AUTHTOK_EXPIRED,
        PAM_MODULE_UNKNOWN,
        PAM_BAD_ITEM,
        PAM_CONV_AGAIN,
        PAM_INCOMPLETE,
        _PAM_RETURN_VALUES,
    );

    define!(
        /// A flag value.
        PAM_SILENT = 0x8000;
        PAM_ESTABLISH_CRED = 0x0002;
        PAM_DELETE_CRED = 0x0004;
        PAM_REINITIALIZE_CRED = 0x0008;
        PAM_REFRESH_CRED = 0x0010;

        PAM_CHANGE_EXPIRED_AUTHTOK = 0x0020;

        PAM_PRELIM_CHECK = 0x4000;
        PAM_UPDATE_AUTHTOK = 0x2000;
        PAM_DATA_REPLACE = 0x20000000;
    );

    c_enum!(
        /// An item type (Linux-only).
        PAM_FAIL_DELAY = 10,
        PAM_XDISPLAY,
        PAM_XAUTHDATA,
        PAM_AUTHTOK_TYPE,
    );

    /// To suppress messages in the item cleanup function.
    pub const PAM_DATA_SILENT: i32 = 0x40000000;

    // Message styles
    define!(
        /// A message style.
        PAM_RADIO_TYPE = 5;
        PAM_BINARY_PROMPT = 7;
    );

    pub const PAM_MODUTIL_NGROUPS: i32 = 64;

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    #[repr(i32)]
    pub enum pam_modutil_redirect_fd {
        PAM_MODUTIL_IGNORE_FD,
        PAM_MODUTIL_PIPE_FD,
        PAM_MODUTIL_NULL_FD,
    }

    impl From<pam_modutil_redirect_fd> for i32 {
        fn from(value: pam_modutil_redirect_fd) -> Self {
            value as Self
        }
    }

    impl TryFrom<i32> for pam_modutil_redirect_fd {
        type Error = i32;
        fn try_from(value: i32) -> Result<Self, Self::Error> {
            match value {
                0..=2 => Ok(unsafe { *(&value as *const i32).cast() }),
                other => Err(other),
            }
        }
    }

    #[doc(inline)]
    pub use pam_modutil_redirect_fd::*;
}

#[cfg(any(pam_impl = "OpenPam", pam_impl = "Sun", pam_impl = "XSso"))]
pub use xsso_shared::*;
#[cfg(any(pam_impl = "OpenPam", pam_impl = "Sun", pam_impl = "XSso"))]
mod xsso_shared {
    c_enum!(
        /// An error return code.
        PAM_OPEN_ERR = 1,
        PAM_SYMBOL_ERR,
        PAM_SERVICE_ERR,
        PAM_SYSTEM_ERR,
        PAM_BUF_ERR,
        PAM_CONV_ERR,
        PAM_PERM_DENIED,
        PAM_MAXTRIES,
        PAM_AUTH_ERR,
        PAM_NEW_AUTHTOK_REQD,
        PAM_CRED_INSUFFICIENT,
        PAM_AUTHINFO_UNAVAIL,
        PAM_USER_UNKNOWN,
        PAM_CRED_UNAVAIL,
        PAM_CRED_EXPIRED,
        PAM_CRED_ERR,
        PAM_ACCT_EXPIRED,
        PAM_AUTHTOK_EXPIRED,
        PAM_SESSION_ERR,
        PAM_AUTHTOK_ERR,
        PAM_AUTHTOK_RECOVERY_ERR,
        PAM_AUTHTOK_LOCK_BUSY,
        PAM_AUTHTOK_DISABLE_AGING,
        PAM_NO_MODULE_DATA,
        PAM_IGNORE,
        PAM_ABORT,
        PAM_TRY_AGAIN,
    );
    // While `PAM_MODULE_UNKNOWN` and `PAM_DOMAIN_UNKNOWN` are in X/SSO,
    // Sun doesn't use them so we're omitting them here.

    /// A general flag for PAM operations.
    pub const PAM_SILENT: i32 = 0x80000000u32 as i32;

    define!(
        /// A flag for `pam_setcred`.
        PAM_ESTABLISH_CRED = 0b0001;
        PAM_DELETE_CRED = 0b0010;
        PAM_REINITIALIZE_CRED = 0b0100;
        PAM_REFRESH_CRED = 0b1000;
    );

    define!(
        /// A flag for `pam_sm_chauthtok`.
        PAM_PRELIM_CHECK = 0b0001;
        PAM_UPDATE_AUTHTOK = 0b0010;
        PAM_CHANGE_EXPIRED_AUTHTOK = 0b0100;
    );
}

#[cfg(pam_impl = "OpenPam")]
pub use openpam::*;
#[cfg(pam_impl = "OpenPam")]
mod openpam {
    c_enum!(
        /// An error return code.
        PAM_MODULE_UNKNOWN = 28,
        PAM_DOMAIN_UNKNOWN,
        PAM_BAD_HANDLE,
        PAM_BAD_ITEM,
        PAM_BAD_FEATURE,
        PAM_BAD_CONSTANT,
    );
    /// The total number of PAM error codes (including success).
    pub const PAM_NUM_ERRORS: i32 = 34;

    c_enum!(
        /// An item type.
        PAM_REPOSITORY = 10,
        PAM_AUTHTOK_PROMPT,
        PAM_OLDAUTHTOK_PROMPT,
        PAM_HOST,
    );
    /// The total number of PAM items.
    pub const PAM_NUM_ITEMS: i32 = 14;

    c_enum!(
        /// An optional OpenPAM feature.
        OPENPAM_RESTRICT_SERVICE_NAME,
        OPENPAM_VERIFY_POLICY_FILE,
        OPENPAM_RESTRICT_MODULE_NAME,
        OPENPAM_VERIFY_MODULE_FILE,
        OPENPAM_FALLBACK_TO_OTHER,
    );
    /// The number of optional OpenPAM features.
    pub const OPENPAM_NUM_FEATURES: i32 = 5;

    c_enum!(
        /// Log level.
        PAM_LOG_LIBDEBUG = -1,
        PAM_LOG_DEBUG,
        PAM_LOG_VERBOSE,
        PAM_LOG_NOTICE,
        PAM_LOG_ERROR,
    );

    c_enum!(
        /// PAM primitives.
        PAM_SM_AUTHENTICATE,
        PAM_SM_SETCRED,
        PAM_SM_ACCT_MGMT,
        PAM_SM_OPEN_SESSION,
        PAM_SM_CLOSE_SESSION,
        PAM_SM_CHAUTHTOK,
    );
    /// The number of PAM primitives.
    pub const PAM_NUM_PRIMITIVES: i32 = 6;
}

/// Constants exclusive to Illumos.
#[cfg(pam_impl = "Sun")]
pub use sun::*;
#[cfg(pam_impl = "Sun")]
mod sun {
    /// The total number of PAM error codes.
    pub const PAM_TOTAL_ERRNUM: i32 = 28;

    c_enum!(
        /// An item type.
        PAM_REPOSITORY = 10,
        PAM_RESOURCE,
        PAM_AUSER,
    );

    /// A flag for `pam_chauthtok`.
    pub const PAM_NO_AUTHTOK_CHECK: i32 = 0b1000;

    define!(
        /// A flag for `__pam_get_authtok`.
        PAM_PROMPT = 1;
        PAM_HANDLE = 2;
    );
}
