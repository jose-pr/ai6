# coqui-access-password

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_access_password/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

**Password accounts**: names and Argon2id password hashes, groups, and the account-management
kind to make, change and remove them. An authenticator, a directory and an `Accounts` provider
over one `Table` of account records. The table is any source you give it (memory by default,
a file, a SQL database, an HTTP API) and is read again whenever it reports a change.

## Features

- **Code first** - `PasswordAccounts::builder()` works out of the box with an in-memory table.
- **Any source** - hand it an `Arc<dyn Table<AccountRecord>>` from `coqui-access-kit`.
- **Argon2id** hashes only, with the costs configurable; a name that is not kept costs the
  same time as a wrong password.
- **Account management** - `put` makes an account (a password is required for a new one),
  changes groups or the disabled flag, or replaces the password.

## Installation

```bash
cargo add coqui-access-password --git https://github.com/Boriken-Dev/coqui-access
```

Build `coqui-access-password-plugin` for the loadable library.

## Quick start

```rust
use coqui_access::{AccountPut, Accounts, Attempt, Authenticator, Verdict};
use coqui_access_password::PasswordAccounts;

let accounts = PasswordAccounts::builder().build().unwrap();
accounts
    .put(AccountPut {
        name: "alice".into(),
        secret: Some("correct horse".into()),
        ..AccountPut::default()
    })
    .unwrap();
let verdict = accounts
    .authenticate(Attempt {
        name: Some("alice".into()),
        secret: Some("correct horse".into()),
        ..Attempt::default()
    })
    .unwrap();
assert!(matches!(verdict, Verdict::Valid(_)));
```

Records of your own come from a table:
`PasswordAccounts::builder().table(table).max_accounts(500).argon2(params).build()?`, where
`table` is a `MemoryTable::from_records([AccountRecord::new(id, name, hash).groups(["staff"])])`
or your own `Table<AccountRecord>`. Hashes come from `coqui_access_kit::PasswordHasher`, which
you can use with your own data too. `AccountRecord` derives guatiao's value traits, so any
table keeps it.

The loadable plugin (provider id `coqui_access_password`) is configured with a `source`, for
example `{ "source": { "file": { "path": "accounts.toml", "format": "toml" } } }` (`format` is
`toml` or `json`), `max_accounts`, `argon2`, `record_limits` and `max_password_bytes`; with no `source` the accounts live in memory.

## API overview

| Item | Purpose |
| --- | --- |
| `PasswordAccounts` | The provider, and the `Authenticator`, `Directory` and `Accounts` kinds. `builder()` makes one. |
| `PasswordAccountsBuilder` | Options: `table`, `max_accounts`, `argon2`, `record_limits`, `max_password_bytes`; `build`. |
| `AccountRecord` | One account as a table keeps it; guatiao value traits. |
| `DEFAULT_MAX_ACCOUNTS` | 10 000. |

## Development

```bash
cargo test -p coqui-access-password --all-features
cargo clippy -p coqui-access-password --all-targets --all-features -- -D warnings
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` triggers the release workflow of the repository.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
