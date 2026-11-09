//! Constants and enum values from the PAM library.

use crate::_doc::{linklist, man7, manbsd, mansun, xsso};
use bitflags::bitflags;
use std::error::Error;
use std::ffi::c_int;
use std::fmt;
use std::result::Result as StdResult;

macro_rules! wrapper {
    (
        $(#[$m:meta])*
        $viz:vis $name:ident($wraps:ty);
    ) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(transparent)]
        $viz struct $name($wraps);

        impl From<$wraps> for $name {
            fn from(value: $wraps) -> Self {
                Self(value)
            }
        }
        impl From<$name> for $wraps {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    }
}

wrapper! {
    /// Type of the flags that PAM passes to us (or that we pass to PAM).
    pub RawFlags(c_int);
}
wrapper! {
    /// The error code that we return to PAM.
    pub ReturnCode(c_int);
}

impl ReturnCode {
    /// A successful return.
    pub const SUCCESS: Self = Self(0);
}

macro_rules! pam_flags {
    (
        $(#[$m:meta])*
        $name:ident {
            $(
                $(#[$m_ident:ident $($m_arg:tt)*])*
                const $item_name:ident = (link = $value_value:expr, else = $other_value:expr);
            )*
        }
    ) => {
        bitflags! {
            #[derive(Clone, Copy, Debug, Default, PartialEq)]
            $(#[$m])*
            pub struct $name: u16 {
                $(
                    $(#[$m_ident $($m_arg)*])*
                    const $item_name = $other_value;
                )*
            }
        }

        #[cfg(feature = "link")]
        impl From<RawFlags> for $name {
            #[allow(unused_doc_comments)]
            fn from(value: RawFlags) -> Self {
                let value: c_int = value.into();
                let result = Self::empty();
                $(
                    $(#[$m_ident $($m_arg)*])*
                    let result = result | if value & $value_value == 0 {
                        Self::empty()
                    } else {
                        Self::$item_name
                    };
                )*
                result
            }
        }

        #[cfg(feature = "link")]
        impl From<$name> for RawFlags {
            #[allow(unused_doc_comments)]
            fn from(value: $name) -> Self {
                let result = 0;
                $(
                    $(#[$m_ident $($m_arg)*])*
                    let result = result | if value.contains($name::$item_name) {
                        $value_value
                    } else {
                        0
                    };
                )*
                Self(result)
            }
        }
    }
}

pam_flags! {
    /// Flags for authentication and account management.
    AuthnFlags {
        /// The PAM module should not generate any messages.
        const SILENT = (link = libpam_sys::PAM_SILENT, else = 0x8000);

        /// The module should return [AuthError](ErrorCode::AuthError)
        /// if the user has an empty authentication token, rather than
        /// allowing them to log in.
        const DISALLOW_NULL_AUTHTOK = (link = libpam_sys::PAM_DISALLOW_NULL_AUTHTOK, else = 0b1);
    }
}

pam_flags! {
    /// Flags for changing the authentication token.
    AuthtokFlags {
        /// The PAM module should not generate any messages.
        const SILENT = (link = libpam_sys::PAM_SILENT, else = 0x8000);

        /// Indicates that the user's authentication token should
        /// only be changed if it is expired. If not passed,
        /// the authentication token should be changed unconditionally.
        const CHANGE_EXPIRED_AUTHTOK = (link = libpam_sys::PAM_CHANGE_EXPIRED_AUTHTOK, else = 0b10);

        /// Don't check if the password is any good (Sun only).
        #[cfg(feature = "sun-ext")]
        const NO_AUTHTOK_CHECK = (link = libpam_sys::PAM_NO_AUTHTOK_CHECK, else = 0b100);
    }
}

pam_flags! {
    /// Common flag(s) shared by all PAM actions.
    BaseFlags {
        /// The PAM module should not generate any messages.
        const SILENT = (link = libpam_sys::PAM_SILENT, else = 0x8000);
    }
}

macro_rules! flag_enum {
    (
        $(#[$m:meta])*
        $name:ident {
            $(
                $(#[$item_m:meta])*
                $item_name:ident = $item_value:path,
            )*
        }
    ) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub enum $name {
            $(
                $(#[$item_m])*
                $item_name,
            )*
        }

        #[cfg(feature = "link")]
        impl TryFrom<RawFlags> for $name {
            type Error = ErrorCode;
            fn try_from(value: RawFlags) -> Result<$name> {
                match value.0 {
                    $(
                        $item_value => Ok(Self::$item_name),
                    )*
                    _ => Err(ErrorCode::BAD_CONST),
                }
            }
        }

        #[cfg(feature = "link")]
        impl From<$name> for RawFlags {
            fn from(value: $name) -> Self {
                match value {
                    $(
                        $name::$item_name => $item_value.into(),
                    )*
                }
            }
        }

        #[cfg(feature = "link")]
        impl $name {
            const ALL_VALUES: c_int = 0 $( | $item_value)*;

            fn split(value: RawFlags) -> Result<(Option<Self>, RawFlags)> {
                let me = value.0 & Self::ALL_VALUES;
                let them = (value.0 & !Self::ALL_VALUES).into();
                let me = match RawFlags(me) {
                    RawFlags(0) => None,
                    other => Some(Self::try_from(other).map_err(|_| ErrorCode::BAD_CONST)?),
                };
                Ok((me, them))
            }
        }
    }
}

flag_enum! {
    /// The credential management action that should take place.
    CredAction {
        /// Set the user's credentials from this module. Default if unspecified.
        Establish = libpam_sys::PAM_ESTABLISH_CRED,
        /// Revoke the user's credentials established by this module.
        Delete = libpam_sys::PAM_DELETE_CRED,
        /// Fully reinitialize the user's credentials from this module.
        Reinitialize = libpam_sys::PAM_REINITIALIZE_CRED,
        /// Extend the lifetime of the user's credentials from this module.
        Refresh = libpam_sys::PAM_REFRESH_CRED,
    }
}

#[cfg(feature = "link")]
impl CredAction {
    /// Separates this enum from the remaining [`BaseFlags`].
    pub(crate) fn extract(value: RawFlags) -> Result<(Self, BaseFlags)> {
        Self::split(value).map(|(act, rest)| (act.unwrap_or_default(), BaseFlags::from(rest)))
    }
}

impl Default for CredAction {
    fn default() -> Self {
        Self::Establish
    }
}

flag_enum! {
    /// The action that a module should take during a [`change_authtok`] call.
    ///
    /// [`change_authtok`]: crate::PamModule::change_authtok
    AuthtokAction {
        /// On this call, just validate that the password is acceptable
        /// and that you have all the resources you need to change it.
        ///
        /// This corresponds to the constant `PAM_PRELIM_CHECK`.
        Validate = libpam_sys::PAM_PRELIM_CHECK,
        /// Actually perform the update.
        ///
        /// This corresponds to the constant `PAM_UPDATE_AUTHTOK`.
        Update = libpam_sys::PAM_UPDATE_AUTHTOK,
    }
}

#[cfg(feature = "link")]
impl AuthtokAction {
    /// Separates this enum from the remaining [`AuthtokFlags`].
    pub(crate) fn extract(value: RawFlags) -> Result<(Self, AuthtokFlags)> {
        match Self::split(value)? {
            (Some(act), rest) => Ok((act, AuthtokFlags::from(rest))),
            (None, _) => Err(ErrorCode::BAD_CONST),
        }
    }
}

/// Constructs an enum which has the values if it's linked
macro_rules! linky_enum {
    (
        $(#[$om:meta])*
        pub enum $name:ident($wrap:ty) {
            $(
                $(#[$im:meta])*
                $key:ident = $value:path,
            )*
        }
    ) => {
        $(#[$om])*
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $(
                $(#[$im])*
                $key,
            )*
        }

        #[cfg(feature = "link")]
        impl TryFrom<$wrap> for $name {
            type Error = ErrorCode;
            fn try_from(value: $wrap) -> Result<Self> {
                match value.into() {
                    $(
                        $(#[$im])*
                        $value => Ok(Self::$key),
                    )*
                    _ => Err(ErrorCode::BAD_CONST),
                }
            }
        }

        #[cfg(feature = "link")]
        impl From<$name> for $wrap {
            fn from(value: $name) -> Self {
                match value {
                    $(
                        $(#[$im])*
                        $name::$key => $value.into(),
                    )*
                }
            }
        }
    }
}

linky_enum! {
    /// The PAM error return codes.
    ///
    /// These are returned by most PAM functions if an error of some kind occurs.
    ///
    /// Instead of being an error code, success is represented by an Ok [`Result`].
    ///
    /// **Do not depend upon the numerical value of these error codes,
    /// or the enum's representation type.
    /// The available codes and their values will vary depending upon
    /// PAM implementation.**
    ///
    /// # References
    ///
    #[doc = linklist!(pam: man7, manbsd, mansun)]
    /// - [X/SSO error code specification][xsso]
    ///
    #[doc = man7!(3 pam "RETURN_VALUES")]
    #[doc = manbsd!(3 pam "RETURN%20VALUES")]
    #[doc = mansun!([3 "pam"] pam "return-values")]
    #[doc = xsso!("chap5.htm#tagcjh_06_02")]
    #[allow(non_camel_case_types, dead_code)]
    #[non_exhaustive] // Different PAMs have different error code sets.
    pub enum ErrorCode(ReturnCode) {
        OpenError = libpam_sys::PAM_OPEN_ERR,
        SymbolError = libpam_sys::PAM_SYMBOL_ERR,
        ServiceError = libpam_sys::PAM_SERVICE_ERR,
        SystemError = libpam_sys::PAM_SYSTEM_ERR,
        BufferError = libpam_sys::PAM_BUF_ERR,
        PermissionDenied = libpam_sys::PAM_PERM_DENIED,
        AuthenticationError = libpam_sys::PAM_AUTH_ERR,
        CredentialsInsufficient = libpam_sys::PAM_CRED_INSUFFICIENT,
        AuthInfoUnavailable = libpam_sys::PAM_AUTHINFO_UNAVAIL,
        UserUnknown = libpam_sys::PAM_USER_UNKNOWN,
        MaxTries = libpam_sys::PAM_MAXTRIES,
        NewAuthTokRequired = libpam_sys::PAM_NEW_AUTHTOK_REQD,
        AccountExpired = libpam_sys::PAM_ACCT_EXPIRED,
        SessionError = libpam_sys::PAM_SESSION_ERR,
        CredentialsUnavailable = libpam_sys::PAM_CRED_UNAVAIL,
        CredentialsExpired = libpam_sys::PAM_CRED_EXPIRED,
        CredentialsError = libpam_sys::PAM_CRED_ERR,
        NoModuleData = libpam_sys::PAM_NO_MODULE_DATA,
        ConversationError = libpam_sys::PAM_CONV_ERR,
        AuthTokError = libpam_sys::PAM_AUTHTOK_ERR,
        AuthTokRecoveryError = libpam_sys::PAM_AUTHTOK_RECOVERY_ERR,
        AuthTokLockBusy = libpam_sys::PAM_AUTHTOK_LOCK_BUSY,
        AuthTokDisableAging = libpam_sys::PAM_AUTHTOK_DISABLE_AGING,
        TryAgain = libpam_sys::PAM_TRY_AGAIN,
        Ignore = libpam_sys::PAM_IGNORE,
        Abort = libpam_sys::PAM_ABORT,
        AuthTokExpired = libpam_sys::PAM_AUTHTOK_EXPIRED,
        #[cfg(feature = "basic-ext")]
        ModuleUnknown = libpam_sys::PAM_MODULE_UNKNOWN,
        #[cfg(feature = "basic-ext")]
        BadItem = libpam_sys::PAM_BAD_ITEM,
        #[cfg(feature = "linux-pam-ext")]
        ConversationAgain = libpam_sys::PAM_CONV_AGAIN,
        #[cfg(feature = "linux-pam-ext")]
        Incomplete = libpam_sys::PAM_INCOMPLETE,
        #[cfg(feature = "openpam-ext")]
        DomainUnknown = libpam_sys::PAM_DOMAIN_UNKNOWN,
        #[cfg(feature = "openpam-ext")]
        BadHandle = libpam_sys::PAM_BAD_HANDLE,
        #[cfg(feature = "openpam-ext")]
        BadFeature = libpam_sys::PAM_BAD_FEATURE,
        #[cfg(feature = "openpam-ext")]
        BadConstant = libpam_sys::PAM_BAD_CONSTANT,
    }
}

/// A PAM-specific Result type with an [ErrorCode] error.
pub type Result<T> = StdResult<T, ErrorCode>;

#[cfg(feature = "link")]
impl fmt::Display for ErrorCode {
    #[cfg(any(pam_impl = "LinuxPam", pam_impl = "OpenPam", pam_impl = "Sun"))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use std::ffi::CStr;
        use std::ptr;
        let retcode: ReturnCode = (*self).into();
        // SAFETY: PAM impls don't care about the PAM handle and always return
        // static strings.
        let got = unsafe { libpam_sys::pam_strerror(ptr::null(), retcode.into()) };
        if got.is_null() {
            // This shouldn't happen.
            write!(f, "PAM error: {self:?} ({:?})", retcode)
        } else {
            // SAFETY: We just got this back from PAM and we checked if it's null.
            f.write_str(&unsafe { CStr::from_ptr(got) }.to_string_lossy())
        }
    }
    #[cfg(not(any(pam_impl = "LinuxPam", pam_impl = "OpenPam", pam_impl = "Sun")))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[cfg(not(feature = "link"))]
impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl Error for ErrorCode {}

#[cfg(feature = "link")]
impl ErrorCode {
    /// Returned when an invalid constant is used.
    #[cfg(feature = "openpam-ext")]
    pub const BAD_CONST: ErrorCode = ErrorCode::BadConstant;
    /// Returned when an invalid constant is used.
    #[cfg(not(feature = "openpam-ext"))]
    pub const BAD_CONST: ErrorCode = ErrorCode::SystemError;

    pub(crate) fn result_from(ret: c_int) -> Result<()> {
        match ret {
            0 => Ok(()),
            value => Err(ReturnCode(value).try_into().unwrap_or(Self::BAD_CONST)),
        }
    }
}

#[cfg(feature = "link")]
impl<T> From<Result<T>> for ReturnCode {
    fn from(value: Result<T>) -> Self {
        match value {
            Ok(_) => ReturnCode::SUCCESS,
            Err(otherwise) => otherwise.into(),
        }
    }
}

#[cfg(all(test, feature = "link"))]
mod tests {
    use super::*;

    #[test]
    fn test_enums() {
        assert_eq!(Ok(()), ErrorCode::result_from(0));
        assert_eq!(
            ReturnCode(libpam_sys::PAM_SESSION_ERR),
            Result::<()>::Err(ErrorCode::SessionError).into()
        );
        assert_eq!(
            Result::<()>::Err(ErrorCode::Abort),
            ErrorCode::result_from(libpam_sys::PAM_ABORT)
        );
        assert_eq!(Err(ErrorCode::BAD_CONST), ErrorCode::result_from(423));
    }

    #[test]
    fn test_flags() {
        assert_eq!(
            AuthtokFlags::CHANGE_EXPIRED_AUTHTOK | AuthtokFlags::SILENT,
            AuthtokFlags::from(RawFlags(
                libpam_sys::PAM_SILENT | libpam_sys::PAM_CHANGE_EXPIRED_AUTHTOK
            ))
        );
        assert_eq!(
            RawFlags(libpam_sys::PAM_DISALLOW_NULL_AUTHTOK),
            AuthnFlags::DISALLOW_NULL_AUTHTOK.into()
        );
        assert_eq!(
            RawFlags(libpam_sys::PAM_SILENT | libpam_sys::PAM_CHANGE_EXPIRED_AUTHTOK),
            (AuthtokFlags::SILENT | AuthtokFlags::CHANGE_EXPIRED_AUTHTOK).into()
        );
    }

    #[test]
    #[cfg(feature = "sun-ext")]
    fn test_flags_sun() {
        assert_eq!(
            AuthtokFlags::NO_AUTHTOK_CHECK,
            AuthtokFlags::from(RawFlags(libpam_sys::PAM_NO_AUTHTOK_CHECK))
        );
        assert_eq!(
            RawFlags(
                libpam_sys::PAM_SILENT
                    | libpam_sys::PAM_CHANGE_EXPIRED_AUTHTOK
                    | libpam_sys::PAM_NO_AUTHTOK_CHECK
            ),
            (AuthtokFlags::SILENT
                | AuthtokFlags::CHANGE_EXPIRED_AUTHTOK
                | AuthtokFlags::NO_AUTHTOK_CHECK)
                .into()
        );
    }

    #[test]
    fn test_flag_enums() {
        AuthtokAction::extract((-1).into()).expect_err("too many set");
        AuthtokAction::extract(0.into()).expect_err("too few set");
        assert_eq!(
            Ok((AuthtokAction::Update, AuthtokFlags::SILENT,)),
            AuthtokAction::extract(
                (libpam_sys::PAM_SILENT | libpam_sys::PAM_UPDATE_AUTHTOK).into()
            )
        );
        CredAction::extract(0xffff.into()).expect_err("too many set");
        assert_eq!(
            Ok((CredAction::Establish, BaseFlags::empty())),
            CredAction::extract(0.into())
        );
        assert_eq!(
            Ok((CredAction::Delete, BaseFlags::SILENT)),
            CredAction::extract((libpam_sys::PAM_SILENT | libpam_sys::PAM_DELETE_CRED).into())
        );
    }
}
