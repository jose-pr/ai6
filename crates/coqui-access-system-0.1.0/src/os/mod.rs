//! The platform's account database, behind one small surface.

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{FIRST_PERSON_UID, by_id, by_name, check_password};

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{by_id, by_name, check_password};

#[cfg(not(any(unix, windows)))]
mod unsupported;
#[cfg(not(any(unix, windows)))]
pub(crate) use unsupported::{by_id, by_name, check_password};

/// Whether this build reaches an account database.
pub(crate) const SUPPORTED: bool = cfg!(any(unix, windows));
#[cfg(not(unix))]
pub(crate) const FIRST_PERSON_UID: u32 = 0;

/// One account as the OS describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Account {
    /// The uid as decimal text, or the SID string on Windows.
    pub(crate) id: String,
    /// The canonical name, as the OS spells it.
    pub(crate) name: String,
    /// The GECOS name, or the Windows full name.
    pub(crate) display_name: Option<String>,
    /// Group names, the primary group first on Unix.
    pub(crate) groups: Vec<String>,
    /// The uid, on Unix.
    pub(crate) uid: Option<u32>,
    /// Disabled by the OS (Windows `UF_ACCOUNTDISABLE`).
    pub(crate) disabled: bool,
}
