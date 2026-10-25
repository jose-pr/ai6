# ai6-access-proxy

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_access_proxy/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

An AI6 Authenticator that trusts identity headers only when the request peer matches a
configured proxy address or CIDR network. It accepts a direct account header or reads a
field from a client-certificate subject header. Resolved identities and groups remain
the Directory's responsibility.

The proxy must overwrite every identity header this provider reads; forwarded client
headers are not proof of identity. The Guatiao entry point is included in this crate.

The public contract is in [AGENTS.md](https://github.com/jose-pr/ai6/blob/main/crates/providers/access/proxy/AGENTS.md).

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
