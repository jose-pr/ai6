# coqui-roles

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_roles/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

A **first-match role authorizer**: named roles list actions, ordered rules bind an account,
person or group to a role at a scope, and anything not allowed is denied. Scope kinds such as
`site` or `project` are configured by the deployment; the crate has no vocabulary of its own.

## Features

- **Code first** - a builder, or a plain-data `RolePolicy`; it never parses a format.
- **Configurable scopes** - `*` or `<kind>:<id>` for the kinds you allow, or any kind.
- **Hierarchies** - a request names the scopes its resource sits under (`ancestors`), and a
  grant on a parent reaches its children unless the rule sets `propagate = false`.
- **Bounded** - at most 256 roles, 256 actions per role, 10 000 rules, 64 scope kinds and
  64 ancestors per request, each a limit the caller changes or lifts.
- **Loadable** - `coqui-roles-plugin` is the `coqui_roles` Guatiao provider, configured with
  structured lists of roles and rules.

## Installation

```bash
cargo add coqui-roles --git https://github.com/Boriken-Dev/coqui-access
```

## Quick start

```rust
use coqui_roles::{RoleAuthorizer, Rule};

let authorizer = RoleAuthorizer::builder()
    .scope_kinds(["site"])
    .role("viewer", ["document.read"])
    .rule(Rule::new("group:staff", "site:north", "viewer"))
    .build()
    .unwrap();
```

`RolePolicy` holds the same data (`scope_kinds`, `roles`, `rules`) for a caller that keeps
its policy elsewhere: `RoleAuthorizer::new(policy)`. It derives guatiao's value traits, so
any format carries it, and it is the configuration of the `coqui_roles` provider
`RoleAuthorizer` derives.

The plugin takes `{ "scope_kinds": ["site"], "roles": [{ "name": "viewer", "actions":
["document.read"] }], "rules": [{ "subject": "group:staff", "scope": "site:north", "role":
"viewer" }] }`.

## API overview

| Item | Purpose |
| --- | --- |
| `RoleAuthorizer` | `builder()`, `new(policy)`; implements `Authorizer`. |
| `RoleAuthorizerBuilder` | `scope_kinds`, `role`, `rule`, `build`. |
| `RolePolicy`, `Role`, `Rule` | The policy as plain data and provider configuration; guatiao value traits. |
| `RolePolicyError` | An invalid policy. |
| `RoleLimits` | The bounds, each a `coqui_access::Limit` the caller changes or lifts. |

## Development

```bash
cargo test -p coqui-roles --all-features
cargo clippy -p coqui-roles --all-targets --all-features -- -D warnings
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` triggers the release workflow of the repository.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
