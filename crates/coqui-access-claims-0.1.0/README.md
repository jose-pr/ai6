# coqui-access-claims

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_access_claims/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

The **claims a client certificate carries**: subject and issuer names, serial, the subject
alternative names (UPN, email, DNS, URI, IP), the fingerprint, the hash of the public key and
the DER itself. The TLS provider (`coqui-access-mtls`) and the one behind a reverse proxy that
forwards the certificate (`coqui-access-proxy`) both build them here, so a matcher sees the
same names whichever way the certificate arrived.

## Installation

```bash
cargo add coqui-access-claims --git https://github.com/Boriken-Dev/coqui-access
```

## Quick start

```rust
let der: Vec<u8> = Vec::new(); // a DER certificate
assert!(coqui_access_claims::of_certificate(&der).is_none());
```

## API overview

| Item | Purpose |
| --- | --- |
| `of_certificate(der)` | The `Claims` of a leaf certificate; `None` when it does not parse. |
| `public_key_hash(der)` | `sha256:<hex>` of the SubjectPublicKeyInfo. |
| `public_key_info_hash(info)` | The same for a DER `SubjectPublicKeyInfo`. |

## Development

```bash
cargo test -p coqui-access-claims
cargo clippy -p coqui-access-claims --all-targets -- -D warnings
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
