//! Windows: passwords through `LogonUserW`, accounts through the NetUser API and the
//! account-name lookups, identities as SID strings.
//!
//! The only file of this crate that holds `unsafe`: each block is one Win32 call and the
//! buffer it hands back.

use core::ptr;
use core::slice;

use coqui_access::error;
use coqui_access::guatiao::library::ProviderError;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCOUNT_DISABLED, ERROR_ACCOUNT_EXPIRED, ERROR_ACCOUNT_LOCKED_OUT,
    ERROR_ACCOUNT_RESTRICTION, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_LOGON_HOURS,
    ERROR_INVALID_WORKSTATION, ERROR_LOGON_FAILURE, ERROR_LOGON_TYPE_NOT_GRANTED,
    ERROR_NONE_MAPPED, ERROR_PASSWORD_EXPIRED, ERROR_PASSWORD_MUST_CHANGE, GetLastError, HANDLE,
    LocalFree,
};
use windows_sys::Win32::NetworkManagement::NetManagement::{
    LG_INCLUDE_INDIRECT, LOCALGROUP_USERS_INFO_0, MAX_PREFERRED_LENGTH, NERR_Success,
    NERR_UserNotFound, NetApiBufferFree, NetUserGetInfo, NetUserGetLocalGroups, UF_ACCOUNTDISABLE,
    USER_INFO_23,
};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW};
use windows_sys::Win32::Security::{
    LOGON32_LOGON_NETWORK, LOGON32_PROVIDER_DEFAULT, LogonUserW, LookupAccountNameW,
    LookupAccountSidW, PSID, SID_NAME_USE, SidTypeUser,
};
use windows_sys::core::PWSTR;

use super::Account;
use crate::{DEFAULT_DOMAIN, SystemAccountsConfig};

/// The UTF-16 text Win32 takes, NUL-terminated.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// The text of a NUL-terminated UTF-16 string Win32 handed back; `None` for null or text
/// that is not UTF-16.
///
/// # Safety
///
/// `text` is null or points at a NUL-terminated UTF-16 string valid for the call.
unsafe fn text_of(text: *const u16) -> Option<String> {
    if text.is_null() {
        return None;
    }
    let mut length = 0;
    // SAFETY: the caller promises a NUL-terminated string; the walk stops at its NUL.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` units before the NUL were just read.
    String::from_utf16(unsafe { slice::from_raw_parts(text, length) }).ok()
}

/// The text form (`S-1-5-..`) of a SID Win32 handed back.
///
/// # Safety
///
/// `sid` points at a valid SID for the call.
unsafe fn sid_text(sid: PSID) -> Result<String, ProviderError> {
    let mut text: PWSTR = ptr::null_mut();
    // SAFETY: `sid` is valid (the caller's promise); `text` receives a LocalAlloc'd string.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(failed("ConvertSidToStringSidW"));
    }
    // SAFETY: on success `text` is a NUL-terminated string, freed right after it is copied.
    let copied = unsafe { text_of(text) };
    // SAFETY: `text` came from ConvertSidToStringSidW, which documents LocalFree.
    unsafe { LocalFree(text.cast()) };
    copied.ok_or_else(|| failed("ConvertSidToStringSidW"))
}

fn failed(call: &str) -> ProviderError {
    // SAFETY: reads the calling thread's last error; no memory is involved.
    let code = unsafe { GetLastError() };
    error::internal(&format!("{call} failed with Windows error {code}"))
}

fn net_failed(call: &str, status: u32) -> ProviderError {
    error::internal(&format!("{call} failed with Windows error {status}"))
}

fn local(config: &SystemAccountsConfig) -> bool {
    config.domain == DEFAULT_DOMAIN
}

/// `domain\name`, or `name` for the machine's own accounts.
fn qualified(name: &str, config: &SystemAccountsConfig) -> String {
    if local(config) {
        name.to_owned()
    } else {
        format!("{}\\{name}", config.domain)
    }
}

pub(crate) fn by_name(
    name: &str,
    config: &SystemAccountsConfig,
) -> Result<Option<Account>, ProviderError> {
    if name.contains('\\') || name.contains('@') {
        return Ok(None);
    }
    if local(config) {
        return local_account(name);
    }
    let Some((sid, canonical, _)) = account_sid(&qualified(name, config))? else {
        return Ok(None);
    };
    let groups = local_groups(&qualified(&canonical, config))?;
    Ok(Some(Account {
        id: sid,
        name: canonical,
        display_name: None,
        groups,
        uid: None,
        disabled: false,
    }))
}

pub(crate) fn by_id(
    id: &str,
    config: &SystemAccountsConfig,
) -> Result<Option<Account>, ProviderError> {
    if !id.starts_with("S-") {
        return Ok(None);
    }
    let Some((name, domain)) = sid_account(id)? else {
        return Ok(None);
    };
    if !local(config) && !domain.eq_ignore_ascii_case(&config.domain) {
        return Ok(None);
    }
    // A domain account's SID resolves on a member machine too: only the configured
    // database's accounts answer, and a lookup by name checks that.
    Ok(by_name(&name, config)?.filter(|account| account.id == id))
}

/// A machine account by name, through `NetUserGetInfo` level 23 (name, full name, flags, SID).
fn local_account(name: &str) -> Result<Option<Account>, ProviderError> {
    let user = wide(name);
    let mut buffer: *mut u8 = ptr::null_mut();
    // SAFETY: `user` is NUL-terminated; on success `buffer` holds one USER_INFO_23 that
    // NetApiBufferFree releases below.
    let status = unsafe { NetUserGetInfo(ptr::null(), user.as_ptr(), 23, &mut buffer) };
    if status == NERR_UserNotFound {
        return Ok(None);
    }
    if status != NERR_Success {
        return Err(net_failed("NetUserGetInfo", status));
    }
    // SAFETY: level 23 fills `buffer` with a USER_INFO_23 whose strings and SID live in the
    // same buffer, read before it is freed.
    let read = unsafe {
        let info = &*buffer.cast::<USER_INFO_23>();
        let read = (
            text_of(info.usri23_name),
            text_of(info.usri23_full_name),
            info.usri23_flags & UF_ACCOUNTDISABLE != 0,
            sid_text(info.usri23_user_sid),
        );
        NetApiBufferFree(buffer.cast());
        read
    };
    let (Some(canonical), full_name, disabled, sid) = read else {
        return Ok(None);
    };
    let groups = local_groups(&canonical)?;
    Ok(Some(Account {
        id: sid?,
        name: canonical,
        display_name: full_name.filter(|full| !full.is_empty()),
        groups,
        uid: None,
        disabled,
    }))
}

/// The local groups `name` belongs to, directly or through a global group.
fn local_groups(name: &str) -> Result<Vec<String>, ProviderError> {
    let user = wide(name);
    let mut buffer: *mut u8 = ptr::null_mut();
    let (mut read, mut total) = (0u32, 0u32);
    // SAFETY: `user` is NUL-terminated; on success `buffer` holds `read` entries that
    // NetApiBufferFree releases below.
    let status = unsafe {
        NetUserGetLocalGroups(
            ptr::null(),
            user.as_ptr(),
            0,
            LG_INCLUDE_INDIRECT,
            &mut buffer,
            MAX_PREFERRED_LENGTH,
            &mut read,
            &mut total,
        )
    };
    if status == NERR_UserNotFound {
        return Ok(Vec::new());
    }
    if status != NERR_Success {
        return Err(net_failed("NetUserGetLocalGroups", status));
    }
    if buffer.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: level 0 fills `buffer` with `read` LOCALGROUP_USERS_INFO_0 entries whose
    // names live in the same buffer, copied before it is freed.
    let groups = unsafe {
        let entries =
            slice::from_raw_parts(buffer.cast::<LOCALGROUP_USERS_INFO_0>(), read as usize);
        let groups = entries
            .iter()
            .filter_map(|entry| text_of(entry.lgrui0_name))
            .collect();
        NetApiBufferFree(buffer.cast());
        groups
    };
    Ok(groups)
}

/// `LookupAccountNameW`: the SID text, the canonical name and the domain of a user account.
fn account_sid(qualified: &str) -> Result<Option<(String, String, String)>, ProviderError> {
    let account = wide(qualified);
    let (mut sid_bytes, mut domain_chars) = (0u32, 0u32);
    let mut kind: SID_NAME_USE = 0;
    // SAFETY: a sizing call: null buffers with zero sizes, which the call fills in.
    let sized = unsafe {
        LookupAccountNameW(
            ptr::null(),
            account.as_ptr(),
            ptr::null_mut(),
            &mut sid_bytes,
            ptr::null_mut(),
            &mut domain_chars,
            &mut kind,
        )
    };
    if sized == 0 {
        // SAFETY: reads the calling thread's last error.
        match unsafe { GetLastError() } {
            ERROR_INSUFFICIENT_BUFFER => {}
            ERROR_NONE_MAPPED => return Ok(None),
            _ => return Err(failed("LookupAccountNameW")),
        }
    }
    // u32 elements: a SID's sub-authorities are DWORDs and are read aligned.
    let mut sid = vec![0u32; (sid_bytes as usize).div_ceil(4)];
    let mut domain = vec![0u16; domain_chars as usize];
    // SAFETY: both buffers have the sizes the sizing call asked for.
    let found = unsafe {
        LookupAccountNameW(
            ptr::null(),
            account.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_bytes,
            domain.as_mut_ptr(),
            &mut domain_chars,
            &mut kind,
        )
    };
    if found == 0 {
        return Err(failed("LookupAccountNameW"));
    }
    if kind != SidTypeUser {
        return Ok(None);
    }
    // SAFETY: the call wrote a valid SID into `sid`.
    let text = unsafe { sid_text(sid.as_mut_ptr().cast())? };
    let domain = String::from_utf16_lossy(&domain[..domain_chars as usize]);
    let name = qualified
        .rsplit('\\')
        .next()
        .unwrap_or(qualified)
        .to_owned();
    Ok(Some((text, name, domain)))
}

/// `LookupAccountSidW`: the name and domain of the user account a SID string names.
fn sid_account(id: &str) -> Result<Option<(String, String)>, ProviderError> {
    let text = wide(id);
    let mut sid: PSID = ptr::null_mut();
    // SAFETY: `text` is NUL-terminated; on success `sid` is LocalAlloc'd, freed below.
    if unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) } == 0 {
        return Ok(None);
    }
    let (mut name_chars, mut domain_chars) = (0u32, 0u32);
    let mut kind: SID_NAME_USE = 0;
    // SAFETY: a sizing call over the valid `sid`.
    unsafe {
        LookupAccountSidW(
            ptr::null(),
            sid,
            ptr::null_mut(),
            &mut name_chars,
            ptr::null_mut(),
            &mut domain_chars,
            &mut kind,
        )
    };
    let mut name = vec![0u16; name_chars.max(1) as usize];
    let mut domain = vec![0u16; domain_chars.max(1) as usize];
    // SAFETY: buffers of the sizes the sizing call asked for; `sid` is still valid.
    let found = unsafe {
        LookupAccountSidW(
            ptr::null(),
            sid,
            name.as_mut_ptr(),
            &mut name_chars,
            domain.as_mut_ptr(),
            &mut domain_chars,
            &mut kind,
        )
    };
    // SAFETY: `sid` came from ConvertStringSidToSidW, which documents LocalFree.
    unsafe { LocalFree(sid.cast()) };
    if found == 0 || kind != SidTypeUser {
        return Ok(None);
    }
    Ok(Some((
        String::from_utf16_lossy(&name[..name_chars as usize]),
        String::from_utf16_lossy(&domain[..domain_chars as usize]),
    )))
}

/// `LogonUserW` with a network logon, which proves the password without loading a profile.
pub(crate) fn check_password(
    name: &str,
    password: &str,
    config: &SystemAccountsConfig,
) -> Result<bool, ProviderError> {
    let user = wide(name);
    let domain = wide(&config.domain);
    let mut secret = wide(password);
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: three NUL-terminated strings; on success `token` is a handle closed below.
    let accepted = unsafe {
        LogonUserW(
            user.as_ptr(),
            domain.as_ptr(),
            secret.as_ptr(),
            LOGON32_LOGON_NETWORK,
            LOGON32_PROVIDER_DEFAULT,
            &mut token,
        )
    } != 0;
    // SAFETY: reads the calling thread's last error, before any other call can reset it.
    let code = unsafe { GetLastError() };
    secret.fill(0);
    core::hint::black_box(&secret);
    if accepted {
        // SAFETY: `token` is the handle LogonUserW opened.
        unsafe { CloseHandle(token) };
        return Ok(true);
    }
    match code {
        ERROR_LOGON_FAILURE
        | ERROR_ACCOUNT_RESTRICTION
        | ERROR_INVALID_LOGON_HOURS
        | ERROR_INVALID_WORKSTATION
        | ERROR_PASSWORD_EXPIRED
        | ERROR_ACCOUNT_DISABLED
        | ERROR_LOGON_TYPE_NOT_GRANTED
        | ERROR_ACCOUNT_EXPIRED
        | ERROR_PASSWORD_MUST_CHANGE
        | ERROR_ACCOUNT_LOCKED_OUT => Ok(false),
        other => Err(error::internal(&format!(
            "LogonUserW failed with Windows error {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_sids_the_machine_does_not_have_are_none() {
        let config = SystemAccountsConfig::default();
        assert_eq!(by_name("no-such-account-x", &config).unwrap(), None);
        assert_eq!(by_id("S-1-5-21-0-0-0-1", &config).unwrap(), None);
        assert_eq!(by_id("not a sid", &config).unwrap(), None);
        // A well-known group's SID is not a user.
        assert_eq!(by_id("S-1-5-32-544", &config).unwrap(), None);
    }

    #[test]
    fn a_logon_as_an_account_nobody_has_is_refused_not_an_error() {
        let config = SystemAccountsConfig::default();
        let checked = check_password("coqui-no-such-account-x", "not a password", &config);
        assert!(matches!(checked, Ok(false)), "{checked:?}");
    }

    #[test]
    fn names_carrying_a_domain_are_not_looked_up() {
        let config = SystemAccountsConfig::default();
        assert_eq!(by_name("DOMAIN\\someone", &config).unwrap(), None);
        assert_eq!(by_name("someone@domain", &config).unwrap(), None);
    }
}
