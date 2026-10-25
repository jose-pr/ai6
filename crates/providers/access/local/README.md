# ai6-access-local

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_access_local/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

Local AI6 accounts stored in a bounded TOML file. The provider implements both
`Authenticator` and `Directory`; each account has a stable person ID, a name, an
Argon2id password hash and optional groups. Passwords are never stored or printed.

The library is also the Guatiao `cdylib` loaded by AI6, so its provider entry point
lives alongside the implementation. No forwarding-only plugin crate is needed.

## Configuration

Configure the same provider ID for the gateway's Authenticator and Directory. The
file may initially be absent, which represents an empty account list. It is reread
when its modification time or length changes.

```toml
[[accounts]]
id = "person-007"
name = "007"
display_name = "James Bond"
groups = ["field"]
password_hash = "$argon2id$v=19$..."
```

Call `hash_password` from a trusted Rust management tool to generate each hash;
passwords must be 1–4096 bytes. The provider deliberately has no account-creation
endpoint. A malformed, oversized, duplicate or unreadable account file is rejected.

## API

The shipped public contract is in [AGENTS.md](https://github.com/jose-pr/ai6/blob/main/crates/providers/access/local/AGENTS.md).

## Development

From the repository root, run `cargo test -p ai6-access-local` and include the crate
in workspace format, Clippy, documentation and leak checks.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
