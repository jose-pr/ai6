# coqui-access-key

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_access_key/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

**Bearer keys** for clients that have nobody to type a password: a relay, a script, a device.
A key is 256 random bits made here and shown once; only a self-describing hash of it is kept
(`sha256:<hex>` by default, or Argon2id). Each key can be disabled, expire or be removed alone.
An authenticator, directory and `Accounts` provider over one `Table` of key records, read
again whenever the table reports a change.

## Features

- **Code first** - `Keys::builder()` works out of the box with an in-memory table.
- **Any source** - hand it an `Arc<dyn Table<KeyRecord>>` from `coqui-access-kit`.
- **Hash only** - the table holds digests; a leaked table presents nothing.
- **A choice of hash** - SHA-256 (the default) or Argon2id, per provider; records name their
  own algorithm, so one table mixes both.
- **Your prefix** - keys begin with a configurable prefix (default `ck_`), so a key is
  recognized where it turns up.
- **Rotation** - `put` with nothing else said makes the account another key and the old one
  stops being taken.

## Installation

```bash
cargo add coqui-access-key --git https://github.com/Boriken-Dev/coqui-access
```

Build `coqui-access-key-plugin` for the loadable library.

## Quick start

```rust
use coqui_access::{AccountPut, Accounts, Attempt, Authenticator, Verdict};
use coqui_access_key::Keys;

let keys = Keys::builder().prefix("ck_").build().unwrap();
let made = keys
    .put(AccountPut { name: "relay".into(), ..AccountPut::default() })
    .unwrap();
let verdict = keys
    .authenticate(Attempt { secret: made.secret, ..Attempt::default() })
    .unwrap();
assert!(matches!(verdict, Verdict::Valid(_)));
```

Records of your own come from a table:
`Keys::builder().table(table).key_hash(KeyHash::argon2id()).max_keys(500).build()?`, where
`table` is a `MemoryTable::from_records([KeyRecord::new(id, name, hash)])` or your own
`Table<KeyRecord>`. The hash comes from `coqui_access_kit::KeyHash::hash`, and
`verify_key` checks one, with or without this provider. `KeyRecord` derives guatiao's value
traits, so any table keeps it.

The loadable plugin (provider id `coqui_access_key`) is configured with a `source`, for example
`{ "source": { "file": { "path": "keys.toml", "format": "toml" } }, "prefix": "ck_" }` (`format`
is `toml` or `json`);
with no `source` the keys live in memory.

## API overview

| Item | Purpose |
| --- | --- |
| `Keys` | The provider: `builder()`, `new_key`, and the three kinds. |
| `KeysBuilder` | Options: `table`, `prefix`, `key_hash`, `max_keys`, `record_limits`; `build`. |
| `KeyRecord` | One key account as a table keeps it; guatiao value traits. |
| `DEFAULT_PREFIX`, `DEFAULT_MAX_KEYS` | `"ck_"`, 10 000. |

## Development

```bash
cargo test -p coqui-access-key --all-features
cargo clippy -p coqui-access-key --all-targets --all-features -- -D warnings
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` triggers the release workflow of the repository.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
