# coqui-access-kit

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_access_kit/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

The **small, boring parts every access provider needs**, written once: typed tables a
provider reads its records through, key and password hashing, generated secrets, and a table
kept in one private file. It names no format: a file table is written in the serde format
the caller hands it.

## Features

- **Tables** - `Table<T>` is the source of a provider's records. `MemoryTable` is built in
  code; a file with a codec, a SQL database or an HTTP API are tables the caller implements.
  `Reloading` caches the latest snapshot and follows `changes_since`.
- **Hashing** - `sha256:<hex>` or Argon2id, self-describing, constant-time verification.
- **Secrets** - a prefix plus 256 random bits, SHA-256 with optional domain separation,
  constant-time comparison.
- **Caches** (feature `cache`) - `Cached` in front of a directory or an authorizer keeps a
  found person or a grant 5 s and "not found" or a denial 1 s by default, to absorb bursts;
  credentials and errors are never kept.
- **File tables** (feature `file`) - `FileTable` keeps a table in one private file, written
  whole beside the target and renamed into place (`0600` on Unix), in any serde format the
  caller supplies as a `Codec`. Records cross through their guatiao derives.

## Installation

```bash
cargo add coqui-access-kit --git https://github.com/Boriken-Dev/coqui-access
```

| Feature | Adds | Needed for | Default |
| --- | --- | --- | --- |
| `file` | `serde`, `guatiao-serde` (no format) | `FileTable`, `Codec`, `Source`, private writes, bounded reads, stamps | no |
| `cache` | `moka` (`sync`) | `Cached`, `CacheOptions`, `CacheSetting`: lookups and decisions kept a few seconds | no |

## Quick start

```rust,ignore
use std::sync::Arc;
use coqui_access_kit::{MemoryTable, Reloading, SourceError, check_keys};

// Records are keyed by name; a table built in code works out of the box.
let table = Arc::new(MemoryTable::from_records(my_records));
let records = Reloading::new(table, |all| check_keys(all, 10_000))?;
let n = records.read(|all| all.len())?;
```

## Use these with your own data

The hashing and verification are plain functions and types, so a provider of your own over
your own storage does not need ours.

```rust
use coqui_access_kit::{Argon2Params, KeyHash, PasswordHasher, generate_key, verify_key};

// Bearer keys: made, said once, kept as a self-describing string.
let key = generate_key("ck_").unwrap();                 // ck_ + 43 URL-safe characters
let fast = KeyHash::Sha256.hash(&key).unwrap();         // "sha256:<64 hex>"  (the default)
let slow = KeyHash::Argon2id(Argon2Params::new(64, 1, 1).unwrap()).hash(&key).unwrap();
assert!(verify_key(&fast, key.as_bytes()) && verify_key(&slow, key.as_bytes()));
assert!(!verify_key(&fast, b"ck_other"));                // a table may mix both

// Passwords: Argon2id, with a dummy hash so an unknown name costs the same time.
let hasher = PasswordHasher::new(Argon2Params::new(64, 1, 1).unwrap()).unwrap();
let stored = hasher.hash("correct horse").unwrap();      // "$argon2id$v=19$m=64,t=1,p=1$..."
assert!(hasher.verify(Some(&stored), "correct horse"));
assert!(!hasher.verify(None, "correct horse"));          // no such account
```

`KeyHash::Sha256` is the default: it is fast and enough for a key of 256 random bits.
`Argon2id` costs one verification per Argon2id record a presented key is checked against, and
is for keys a person might have chosen. `verify_key` branches only on the algorithm prefix of
the stored string; a string naming no known algorithm never matches. `Argon2Params::default()`
is 19 MiB, 2 passes, 1 lane. As configuration, `Argon2Params` is a map of `memory_kib`,
`iterations` and `parallelism`, each defaulting to that value; reading it refuses a
combination Argon2 would.

## API overview

| Module | Purpose |
| --- | --- |
| `table` | `Table<T>`, `Keyed`, `Snapshot`, `Revision`, `SourceError`, `MemoryTable`, `Reloading`, `check_keys`. |
| `hash` | `KeyHash`, `Argon2Params`, `verify_key`, `is_key_hash`, `PasswordHasher`, `verify_password`, `is_password_hash`, `generate_key`. |
| `pick` | `Pick`, `Picked`, `pick`: which account a credential's fields name. |
| `secret` | `generate`, `identifier`, `hash`, `hash_bytes`, `same`, `is_hash`. |
| `cache` | Feature `cache`. `Cached`, `CacheOptions`, `CacheSetting`, `DEFAULT_TTL`, `DEFAULT_NEGATIVE_TTL`, `DEFAULT_MAX_ENTRIES`. |
| `file` | Feature `file`. `FileTable`, `Codec`, `CodecError`, `Source`, `FileSource`, `DEFAULT_MAX_BYTES`, `write_private`, `read_bounded`, `stamp`, `Stamp`. |

## A table in a file

`FileTable<T, C>` keeps records of a type deriving guatiao's `ToValue` and `FromValue` under
one key of a document, written with the codec `C`. The kit depends on no format crate: the
caller implements `Codec` over the one it picks (`toml`, `serde_json`, …), and records cross
through guatiao-serde's bridge with no tree in between.

```rust,ignore
let table = FileTable::<AccountRecord, _>::new("accounts.toml", "accounts", MyToml);
```

A missing file is an empty table; a file that is unreadable, larger than `max_bytes` (1 MiB by
default) or invalid is an error that quotes none of its contents.

## Development

```bash
cargo test -p coqui-access-kit --all-features
cargo clippy -p coqui-access-kit --all-targets --all-features -- -D warnings
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` triggers the release workflow of the repository.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
