# coqui-access

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_access/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

The **kinds and models** that every access provider and every host share: authenticate
credentials, resolve a person, authorize an action, and manage accounts. Built on
[Guatiao](https://github.com/Boriken-Dev/guatiao); kind names and the hashes of their required
methods are a pinned ABI. See the [documentation](https://boriken-dev.github.io/coqui-access/).

## Features

- **Authenticator, Directory, Authorizer** — credentials give a `Lookup`, a lookup gives a
  `Person`, a person and an action give an `AuthorizationDecision`.
- **Accounts, optional** — `accounts`, `put`, `remove` and `waiting` for providers that keep
  their own accounts.
- **Redacted by construction** — `Debug` of `Attempt`, `Lookup`, `Person` and `AccountPut`
  never shows a secret or an identity value.
- **One import** — `coqui_access::guatiao` is the Guatiao providers must use.

## Installation

```bash
cargo add coqui-access --git https://github.com/Boriken-Dev/coqui-access
```

| Feature | Adds | Needed for |
| --- | --- | --- |
| none | `guatiao` with `provider` | everything; a host adds `guatiao/load` itself |

## Quick start

A provider implements the kinds it offers and names Guatiao through this crate:

```rust,ignore
use coqui_access::guatiao::{Provider, library::ProviderError};
use coqui_access::{Attempt, Authenticator, Lookup, Verdict};

#[derive(Default, Provider)]
#[provider(Authenticator, crate = ::coqui_access::guatiao)]
struct Always;

impl Authenticator for Always {
    fn authenticate(&self, _: Attempt) -> Result<Verdict, ProviderError> {
        Ok(Verdict::Valid(Lookup { by: "name".into(), value: "alice".into() }))
    }
}
```

## API overview

| Item | Purpose |
| --- | --- |
| `Authenticator`, `Directory`, `Authorizer`, `Accounts` | The four kinds. |
| `Attempt`, `Verdict`, `Lookup`, `Person` | Authentication and lookup models. |
| `Subject` | `account:<name>`, `person:<id>` or `group:<name>`, as policies write who they name. |
| `AuthorizationRequest`, `AuthorizationDecision` | Authorization models. |
| `AccountInfo`, `AccountPut`, `PutOutcome`, `Waiting` | Account management models. |
| `Description`, `takes` | What a provider says it is and takes. |
| `error` | `bad_value`, `not_found`, `unsupported`, `internal` as `ProviderError`s. |

`kinds.json` beside the manifest describes the kinds as data for bindings.

## Development

```bash
cargo test -p coqui-access --all-features
cargo clippy -p coqui-access --all-targets --all-features -- -D warnings
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` triggers the release workflow of the repository.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
