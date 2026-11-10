# `coqui-access-system` — API header

Header-file-style reference for the `coqui-access-system` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

MSRV Rust 1.89. No Cargo features. Depends on `coqui-access`, `coqui-builder` and `guatiao-builder`; on Unix on `uzers` (NSS) and
`nonstick` (PAM, linking the system `libpam`), on Windows on `windows-sys`. Everything is at
the root. `unsafe` lives in `src/os/windows.rs` alone (the Win32 calls), checked by
`tests/unsafe_stays_in_windows.rs`; the root is `#![deny(unsafe_code)]`. The loadable library
is the separate crate `coqui-access-system-plugin`.

```rust
pub const DEFAULT_PAM_SERVICE: &str = "coqui-access";
pub const DEFAULT_MIN_UID: u32;          // Linux 1000, FreeBSD 1001, macOS 501, other Unix 1000, Windows 0
pub const DEFAULT_DOMAIN: &str = ".";
pub const SUPPORTED: bool;               // Unix and Windows

#[non_exhaustive]                        // coqui_builder::Describe; deny_unknown_fields
pub struct SystemAccountsConfig {
    pub pam_service: String,             // default DEFAULT_PAM_SERVICE (Unix)
    pub min_uid: u32,                    // default DEFAULT_MIN_UID (Unix); 0 admits root
    pub groups: Vec<String>,             // default empty = any account
    pub domain: String,                  // default "." (Windows): the machine's own accounts
}
impl Default for SystemAccountsConfig;   // the defaults above
impl SystemAccountsConfig { pub fn non_defaults(&self) -> Vec<NonDefault>; }

#[non_exhaustive]
pub enum NonDefault { AdmitsRoot, AdmitsServiceAccounts }   // Display; Copy, Eq

pub struct SystemAccountsBuilder;        // Debug, Clone, Default, must_use
impl SystemAccountsBuilder {
    pub fn pam_service(self, service: impl Into<String>) -> Self;
    pub fn min_uid(self, uid: u32) -> Self;
    pub fn groups(self, groups: impl IntoIterator<Item = impl Into<String>>) -> Self;
    pub fn domain(self, domain: impl Into<String>) -> Self;
    pub fn build(self) -> Result<SystemAccounts, ProviderError>;
}

pub struct SystemAccounts;               // provider id "coqui_access_system", config Described<SystemAccountsConfig>
impl SystemAccounts {
    pub fn builder() -> SystemAccountsBuilder;
    pub fn new(config: SystemAccountsConfig) -> Result<Self, ProviderError>;
    pub fn config(&self) -> &SystemAccountsConfig;
}
impl TryFrom<SystemAccountsConfig> for SystemAccounts;      // Error = ProviderError
impl TryFrom<guatiao_builder::Described<SystemAccountsConfig>> for SystemAccounts;   // the provider config
impl Authenticator for SystemAccounts;   // takes "password"
impl Directory for SystemAccounts;       // by "id" or "name"
```

## Behaviour

- `build` performs no lookup. It is `GUATIAO_ERR_UNSUPPORTED` where `SUPPORTED` is false, and
  `GUATIAO_ERR_BAD_VALUE` for an empty `pam_service` or one holding `/`, an empty `domain`, an
  empty group, or a control character in any of them.
- An account is **admitted** when its uid is at least `min_uid` (Unix) and, with `groups`
  set, it belongs to one of them. `non_defaults()` reports `AdmitsRoot` for `min_uid` 0 and
  `AdmitsServiceAccounts` for one below `DEFAULT_MIN_UID`, on Unix; nothing is refused for it.
  Windows accounts carry no uid, so there a `min_uid` other than `DEFAULT_MIN_UID` is `GUATIAO_ERR_BAD_VALUE`.
- `authenticate`: no name or no secret, a name the OS does not have, or one not admitted, is
  `Unknown`; an empty password, a disabled account, a wrong password, or an account the OS
  refuses now (expired, locked, password to change, outside its hours) is `Invalid`; success
  is `Valid(Lookup { by: "id", value })` with the uid as decimal text, or the SID string
  (`S-1-5-21-…`) on Windows. Anything else the OS reports is `GUATIAO_ERR_INTERNAL` ("could
  not find out").
- Unix: accounts and groups come from NSS (`getpwnam`, `getpwuid`, `getgrouplist`), so SSSD
  and LDAP accounts the machine knows appear. The password is checked with
  `pam_authenticate` then `pam_acct_mgmt` under `pam_service`, the user name set; the
  conversation answers the masked prompt with the password and refuses any other prompt
  (an OTP module asking an echoed question makes the attempt `Invalid`).
- Windows: `LogonUserW(name, domain, password, LOGON32_LOGON_NETWORK)`; the token is closed
  at once. With `domain` `.`, accounts come from `NetUserGetInfo` level 23 (name, full name,
  `UF_ACCOUNTDISABLE`, SID) on this machine; with another domain, from
  `LookupAccountNameW("domain\name")`, with no display name or disabled flag. Groups are the
  account's local groups, `NetUserGetLocalGroups` with `LG_INCLUDE_INDIRECT`. A name holding
  `\` or `@` is not looked up.
- `lookup` by `id` (uid text of digits only, or a SID string naming a user) or `name`; any
  other `by`, an empty value or one holding NUL, an unadmitted or disabled account is `None`.
  `Person { id, name, display_name, groups }`: the GECOS name's first field or the Windows
  full name; groups by name, the primary group first on Unix.

## Gotchas

- **Checking another account's password needs root under `pam_unix`** (Linux, FreeBSD): run
  unprivileged, it can check only the caller's own account (through `unix_chkpwd` on Linux)
  and answers `Invalid` for the rest, right password or not. macOS's `pam_opendirectory`
  needs no root.
- **The PAM service needs a file**: with no `/etc/pam.d/<pam_service>`, Linux-PAM and OpenPAM
  use `other`, which refuses everyone on Fedora and macOS, so every password is `Invalid`.
  See the README for one. On macOS the stock `chkpasswd` service checks any local account's
  password without root.
- **Every call blocks**: PAM may sleep seconds after a wrong password (`pam_faildelay`), and a
  directory behind NSS may be remote. Call it from a blocking thread.
- **An unknown name is `Unknown`, sooner than a wrong password is `Invalid`**: the time
  tells whether an account exists. Most systems publish their account list anyway.
- Building on Unix links `libpam` (`libpam0g-dev` on Debian and Ubuntu, `pam-devel` on Fedora).
