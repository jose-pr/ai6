# coqui-access-totp

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_access_totp/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

**One-time codes** (TOTP, RFC 6238) as a second factor that works in every client: a browser, a
desktop app, a command line. Enrolment, rotation and reset go through the `Accounts` kind; ten
one-time recovery codes are made at enrolment and presented as the separate method
`recovery_code`. An authenticator and `Accounts` provider over a `Table` of records.

## Features

- **RFC 6238** - SHA-1 (every app reads it), SHA-256 or SHA-512, 6 to 8 digits, any period; checked
  against the RFC's Appendix B values.
- **Replay protection** - the step a code was accepted for is written before the answer; that step
  and earlier ones are refused, and a write that fails is an error. A file table is locked around the
  read and the write, so another process over the same file cannot use a code twice either.
- **A delay under wrong codes** - five in a row (default) and the account takes no code for 30 s,
  doubling to an hour; a caller lifts it with `Lockout::OFF`.
- **Skew** - one step either side by default; every step in range is compared in constant time.
- **Enrolment you prove** - a secret is not a factor until a code from the app confirms it;
  rotation keeps the old secret working until then.
- **Recovery codes** - ten, kept as digests, each good once, a method the policy must name.
- **Any source** - an `Arc<dyn Table<TotpRecord>>` and an `Arc<dyn Table<PendingTotp>>`.

## Installation

```bash
cargo add coqui-access-totp --git https://github.com/Boriken-Dev/coqui-access
```

Build `coqui-access-totp-plugin` for the loadable library.

## Quick start

```rust
use coqui_access::{AccountPut, Accounts};
use coqui_access_totp::TotpAccounts;

let totp = TotpAccounts::builder().issuer("Example").build().unwrap();
let begun = totp
    .put(AccountPut { name: "id-alice".into(), ..AccountPut::default() })
    .unwrap();
// Show `begun.secret` (an otpauth:// link) as a QR code and `begun.codes` once; then confirm
// with the code the app shows: `put(AccountPut { name, secret: Some(code), .. })`.
assert!(begun.secret.unwrap().starts_with("otpauth://totp/Example:"));
```

`name` is the account key: the `Person::id` an earlier sign-in step named. The provider does not
throttle; the consumer's throttle and pending-sign-in try cap bound guessing. The secrets are kept
readable in the table, which a file source writes privately.

The loadable plugin (provider id `coqui_access_totp`) takes a `source`, for example
`{ "source": { "file": { "path": "totp.toml", "format": "toml" } }, "issuer": "Example" }`;
unconfirmed enrolments go beside it in `totp.toml.pending`.

## API overview

| Item | Purpose |
| --- | --- |
| `TotpAccounts` | The provider: `builder()`, `Authenticator` and `Accounts`. |
| `TotpBuilder` | `table`, `pending`, `issuer`, `algorithm`, `digits`, `period`, `skew`, `recovery_codes`, `enrol_seconds`, `max_records`, `record_limits`, `clock`; `build`. |
| `TotpRecord`, `PendingTotp` | The records the two tables keep; guatiao value traits. |
| `Algorithm` | `Sha1`, `Sha256`, `Sha512`. |

## Development

```bash
cargo test -p coqui-access-totp --all-features
cargo clippy -p coqui-access-totp --all-targets --all-features -- -D warnings
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` triggers the release workflow of the repository.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
