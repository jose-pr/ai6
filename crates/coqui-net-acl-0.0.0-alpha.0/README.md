# coqui-net-acl

[![Version](https://img.shields.io/crates/v/coqui-net-acl.svg)](https://crates.io/crates/coqui-net-acl)
[![Rust version](https://img.shields.io/crates/msrv/coqui-net-acl.svg)](https://crates.io/crates/coqui-net-acl)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-net-acl/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/coqui-net-acl)](https://docs.rs/coqui-net-acl)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-net-acl/test.yml)](https://github.com/Boriken-Dev/coqui-net-acl/actions/workflows/test.yml)

An **ordered, first-match access control list** like a router ACL: rules are read top to
bottom, the first match wins, and a default action applies when none does (deny, unless you set
it). It has no TLS dependency and decides on any listener, before the first byte is read. See
the [documentation](https://boriken-dev.github.io/coqui-net-acl/).

> **Status: alpha.** `0.0.0-alpha.0` is a pre-release: the API may change in any release
> before 1.0.

## Features

- **Source and destination rules**: a source network, and optionally the local address and
  port connected to.
- **Two evaluation points**: at accept time from the addresses alone, and after a handshake
  with the verified peer fingerprint.
- **Router-style listing**: `Display` prints `10 permit 10.6.0.0/16`, numbered in steps of ten.
- **A plain text form** (TOML, JSON) with the `serde` feature, and as a guatiao value with a schema with the `guatiao` feature.
- No dependencies in a default build, no `unsafe` code, no process-wide state.

## Installation

```bash
cargo add coqui-net-acl@0.0.0-alpha.0
```

| Feature | Adds | Needed for | Default |
| --- | --- | --- | --- |
| `serde` | `serde` | reading and writing an ACL in its text form | off |
| `guatiao` | `guatiao` (`derive`) | `Acl`, `Rule` and `Action` as guatiao values with a schema | off |

Rust 1.89 or later.

## Quick start

```rust
use coqui_net_acl::{Acl, Action, Outcome, Rule};

let mut acl = Acl::default(); // no rules, default deny
acl.push(Rule::new(Action::Permit, "10.6.0.0/16".parse()?));

let decision = acl.decide("10.6.1.2:5000".parse()?, "10.6.0.1:443".parse()?);
assert_eq!(decision.outcome, Outcome::Permit);
assert_eq!(decision.rule_index, Some(0));

let outside = acl.decide("192.0.2.7:5000".parse()?, "10.6.0.1:443".parse()?);
assert_eq!(outside.outcome, Outcome::Deny); // the default
assert_eq!(acl.to_string(), "10 permit 10.6.0.0/16\ndefault deny\n");
# Ok::<(), Box<dyn std::error::Error>>(())
```

With the `serde` feature the same list reads from TOML or JSON, through your own `toml` or
`serde_json`:

```toml
default = "deny"

[[rule]]
action = "permit"
source = "10.9.0.0/16"
destination = "10.6.20.5"     # optional: the local address, or a CIDR
port = "7000-7100"            # optional: a port or an inclusive range

[[rule]]
action = "deny"
source = "any"
```

## API overview

Everything is at the crate root.

| Item | Purpose |
| --- | --- |
| `Acl` | The rules and the default; `push`, `prepend`, `decide`, `decide_peer`, `decide_anonymous`, `canonicalize_fingerprints`, `permits_bind`, `non_defaults`. |
| `Rule` | `action`, `source`, and the optional `destination`, `port` and `fingerprint`; a `log` hint. |
| `IpNet`, `PortRange` | Networks (`any`, a host, CIDR) and ports, parsed from and printed as text. |
| `Outcome`, `AclDecision`, `BindDecision` | What was decided, and by which rule. |
| `Action`, `NonDefault`, `ParseError` | `permit` / `deny`; a setting that differs from its default; a malformed network or port. |

## Evaluation

1. `Acl::decide` runs at accept time, from the connecting and local socket addresses alone. A
   rule that names a peer fingerprint cannot be decided yet: when it is the first to match, the
   outcome is `Outcome::Deferred` and the connection proceeds to its handshake.
2. `Acl::decide_peer` runs after the handshake with the verified fingerprint, which finishes
   the decision; it is never deferred.

A peer that proves no identity (plaintext, or TLS without a client certificate) is decided by
`Acl::decide_anonymous`: rules that name a fingerprint cannot match it and are passed over.
Fingerprints are compared as text; a host whose rules may be written in several spellings
rewrites them into its canonical one with `Acl::canonicalize_fingerprints` when it builds the
list, refusing any that does not parse.

An IPv4-mapped IPv6 address (`::ffff:10.0.0.9`) is the IPv4 address it maps, in rules and in
connections. `Acl::permits_bind` is advisory: whether any rule could permit traffic to a local
address. Nothing enforces it; what to listen on is the host's choice. `Acl::non_defaults` lists a
`permit` default, as data, so a host can report it.

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo +1.89 clippy --all-targets --all-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cargo +nightly-2026-09-21 public-api --all-features -sss --color never | diff -u tools/public-api.txt -
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-net-acl/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` runs the release workflow: the tests and a strict documentation
build gate a GitHub release, marked as a pre-release when the version has a `-` suffix. The
crate is published to crates.io with `cargo publish` by a maintainer.

## License

MPL-2.0; see [LICENSE](https://github.com/Boriken-Dev/coqui-net-acl/blob/main/LICENSE).
