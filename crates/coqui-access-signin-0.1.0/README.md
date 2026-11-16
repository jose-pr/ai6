# coqui-access-signin

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_access_signin/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

**Sign-in rules**: ordered rules over where and how a sign-in is made decide which methods it may
use and what it still needs. A rule says when it applies (purpose, client network, kind of client,
service, who) and which chains of methods it allows; a chain is methods that must all succeed and
name one account. The `coqui_signin_rules` provider of the `coqui_sign_in_policy` kind.

## Features

- **Code first** - a builder, or plain-data `SignInConfig`; it never parses a format.
- **Order is the tool** - the first rule that applies decides once someone is named; an address that
  is not known never satisfies a network, so a stricter rule placed last is where it falls.
- **Step-up** - `["password", "totp"]` in a chain asks for a second factor; a method that needs an
  account (`totp`, `recovery_code`) is never offered before a step has named someone.
- **Bounded** - at most 1 024 rules, 64 chains and 1 024 networks per rule, 8 methods per chain,
  each a limit the caller changes or lifts.
- **Loadable** - `coqui-access-signin-plugin` is the `coqui_signin_rules` Guatiao provider.
- **Shared vectors** - the `test-support` feature carries the cases every consumer runs.

## Installation

```bash
cargo add coqui-access-signin --git https://github.com/Boriken-Dev/coqui-access
```

## Quick start

```rust
use coqui_access::{SignInContext, SignInPolicy, purposes};
use coqui_access_signin::{RuleDef, SignInRules};

// A password or a certificate inside 10.0.0.0/8; the certificate alone anywhere else.
let rules = SignInRules::builder()
    .rule(RuleDef::new([["password"], ["certificate"]]).networks(["10.0.0.0/8"]))
    .rule(RuleDef::new([["certificate"]]))
    .build()
    .unwrap();
let decision = rules
    .decide(SignInContext {
        purpose: purposes::SIGN_IN.to_owned(),
        address: Some("203.0.113.7".to_owned()),
        ..SignInContext::default()
    })
    .unwrap();
assert_eq!(decision.methods[0].method, "certificate");
```

The plugin (provider id `coqui_signin_rules`) takes the same rules as data:
`{ "rule": [ { "networks": ["10.0.0.0/8"], "allow": [["password"], ["certificate"]] }, { "allow": [["certificate"]] } ] }`.
A deployment with no policy uses `SignInRules::single_factor(methods)`.

## API overview

| Item | Purpose |
| --- | --- |
| `SignInRules` | The provider: `builder()`, `new(config)`, `single_factor(methods)`; implements `SignInPolicy`. |
| `SignInRulesBuilder` | `rule`, `limits`, `build`. |
| `SignInConfig`, `RuleDef` | The rules as plain data and provider configuration; `Describe`. |
| `SignInLimits` | The bounds, each a `coqui_access::Limit` the caller changes or lifts. |
| `SignInRulesError` | An invalid rule set. |

## Features (Cargo)

| Feature | Adds |
| --- | --- |
| `test-support` | `vectors::{POLICY, FLOWS}`: the shared JSON vectors. |

## Development

```bash
cargo test -p coqui-access-signin --all-features
cargo clippy -p coqui-access-signin --all-targets --all-features -- -D warnings
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` triggers the release workflow of the repository.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
