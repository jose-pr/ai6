# ai6-access-peer

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_access_peer/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

Maps a TLS-verified AI6 station certificate fingerprint to a stable Directory person.
The provider accepts only `Attempt.certificate`, which must come from a client
certificate that the transport has already authenticated and allowed. It does not
replace AI6's `PeerTrustStore`.

The provider table is bounded TOML. Each active record grants the mapped account's
Directory groups; authorization policy still decides which actions that identity may
perform. Its Guatiao entry point lives in this crate.

The public contract is in [AGENTS.md](https://github.com/jose-pr/ai6/blob/main/crates/providers/access/peer/AGENTS.md).

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
