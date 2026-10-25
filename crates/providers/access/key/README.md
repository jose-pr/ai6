# ai6-access-key

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_access_key/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

Bearer keys for unattended clients, kept as SHA-256 digests in a bounded TOML file.
The crate implements AI6 `Authenticator` and `Directory` and contains its own Guatiao
provider entry point.

Generate a fresh 256-bit secret with `key()`, deliver it once, and store only
`hash_of(secret)` in the key table. The provider has no key-creation endpoint; manage
the file through a trusted operator workflow. Records can be disabled by editing the
table. Invalid updates are rejected while the last valid snapshot remains active.

The public contract and file format are in [AGENTS.md](https://github.com/jose-pr/ai6/blob/main/crates/providers/access/key/AGENTS.md).

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
