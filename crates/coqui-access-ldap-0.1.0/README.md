# coqui-access-ldap

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_access_ldap/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

**Authenticates against LDAP or Active Directory** and uses the same directory for names and
groups. Passwords are sent only over TLS, StartTLS or loopback; search filters escape
user-supplied values. No connection is made until a request is checked.

## Features

- **Search, then bind** — finds the account with a service account, refuses disabled ones,
  then binds as the account with the supplied password.
- **Groups** — from `memberOf`, from a group search, or nested with Active Directory's
  matching rule.
- **Private CA** — an optional PEM file as the only trusted roots.

## Installation

```bash
cargo add coqui-access-ldap --git https://github.com/Boriken-Dev/coqui-access
```

Build `coqui-access-ldap-plugin` for the loadable library.

## Quick start

Provider id `coqui_access_ldap`, configuration:

```toml
url = "ldaps://directory.example.org"
base = "DC=example,DC=org"
bind = "CN=search,DC=example,DC=org"
bind_password = "..."
# user_filter = "(sAMAccountName={name})"
# nested_groups = true
```

## API overview

| Item | Purpose |
| --- | --- |
| `LdapAccounts` | The provider: `new(config)`, `Authenticator` and `Directory`. |
| `LdapAccountsConfig` | The Guatiao configuration. |
| `search` | Filter and result helpers that need no directory. |

The live test in `tests/live_directory.rs` is ignored unless run against a disposable
directory; see `AGENTS.md`.

## Development

```bash
cargo test -p coqui-access-ldap --all-features
cargo clippy -p coqui-access-ldap --all-targets --all-features -- -D warnings
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` triggers the release workflow of the repository.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
