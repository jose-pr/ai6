# ai6-access-ldap

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_access_ldap/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

Authenticates accounts against LDAP or Active Directory and uses the same directory
for names and groups. Passwords are sent only over TLS, StartTLS, or loopback for local
testing. Search filters escape user-supplied values; the provider refuses a user filter
that omits `{name}` or a remote cleartext LDAP URL.

The same crate implements Guatiao `Authenticator` and `Directory` providers. It makes
no directory connection until a request is checked. See [AGENTS.md](https://github.com/jose-pr/ai6/blob/main/crates/providers/access/ldap/AGENTS.md) for the
configuration and public contract.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
