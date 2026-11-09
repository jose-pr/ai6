# coqui-access-system

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_access_system/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

**Signs people in with the operating system's own accounts** and resolves them as a
directory: PAM and NSS on Linux, FreeBSD and macOS (so SSSD or LDAP accounts the machine
knows appear too), `LogonUserW` and the NetUser API on Windows. It is read only: accounts are
managed with the OS's own tools.

## Features

- **Authenticator** — a name and a password, checked by PAM (`pam_authenticate`, then
  `pam_acct_mgmt`, so an expired or locked account is refused) or by a Windows network logon.
- **Directory** — `Person { id, name, display_name, groups }`: the uid, or the SID on
  Windows; the GECOS or full name; the account's groups (local groups on Windows).
- **Narrowing** — a lowest uid (by default the platform's first person uid, so root and
  service accounts stay out) and a list of groups an account must be in.

## Installation

```bash
cargo add coqui-access-system --git https://github.com/Boriken-Dev/coqui-access
```

Build `coqui-access-system-plugin` for the loadable library. No Cargo features.

## Quick start

Provider id `coqui_access_system`; every key is optional:

```toml
pam_service = "coqui-access"   # /etc/pam.d/coqui-access on Unix
min_uid = 1000                 # the platform's first person uid by default; 0 admits root
groups = ["staff"]             # empty admits any account
domain = "."                   # Windows: the machine's own accounts
```

In code:

```rust,no_run
use coqui_access::{Attempt, Authenticator, Verdict};
use coqui_access_system::SystemAccounts;

let accounts = SystemAccounts::builder().groups(["staff"]).build()?;
let verdict = accounts.authenticate(Attempt {
    name: Some("alice".into()),
    secret: Some("correct horse".into()),
    ..Attempt::default()
})?;
assert!(matches!(verdict, Verdict::Valid(_) | Verdict::Invalid | Verdict::Unknown));
# Ok::<(), coqui_access::guatiao::library::ProviderError>(())
```

## API overview

| Item | Purpose |
| --- | --- |
| `SystemAccounts` | The provider: `builder()`, `new(config)`, `config()`, `Authenticator` and `Directory`. |
| `SystemAccountsConfig` | The Guatiao configuration; `non_defaults()` reports what it admits beyond the defaults. |
| `NonDefault` | `AdmitsRoot`, `AdmitsServiceAccounts`. |
| `SUPPORTED`, `DEFAULT_*` | Whether this build reaches an account database, and the defaults. |

## PAM service

The provider checks passwords under the PAM service `pam_service` (default `coqui-access`).
Give it a file in `/etc/pam.d` that does what the machine's login does, for example on
Fedora:

```text
auth     include  system-auth
account  include  system-auth
```

and on FreeBSD `auth include system` / `account include system`. On macOS the stock
`chkpasswd` service does the job: set `pam_service = "chkpasswd"`. With no such file,
Linux-PAM and OpenPAM use `other`, which refuses everyone on Fedora and macOS. Under
`pam_unix` (Linux, FreeBSD) checking another account's password needs root; macOS's
`pam_opendirectory` does not.

## Development

```bash
cargo test -p coqui-access-system
cargo clippy -p coqui-access-system --all-targets -- -D warnings
```

Building on Unix links the system's `libpam` (Debian and Ubuntu: `libpam0g-dev`; Fedora:
`pam-devel`). The real sign-in tests are ignored by default; `tests/real_login.rs` says how to
run them against a throwaway account.

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` triggers the release workflow of the repository.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
