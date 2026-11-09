# coqui-peer

[![Version](https://img.shields.io/crates/v/coqui-peer.svg)](https://crates.io/crates/coqui-peer)
[![Rust version](https://img.shields.io/crates/msrv/coqui-peer.svg)](https://crates.io/crates/coqui-peer)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-peer/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/coqui-peer)](https://docs.rs/coqui-peer)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-peer/test.yml)](https://github.com/Boriken-Dev/coqui-peer/actions/workflows/test.yml)

**Who a peer is, and whether we trust it**, with no TLS and no cryptographic transport. This is
the core every authentication method shares: the peer id, a trust store with pairing and key
rotation, and a sans-IO handshake trait. A method lives in a crate of its own;
[`coqui-peer-tls`](https://crates.io/crates/coqui-peer-tls) is the TLS one, and
[`coqui-peer-webcrypto`](https://github.com/Boriken-Dev/coqui-peer-webcrypto) the one for browsers. See the
[documentation](https://boriken-dev.github.io/coqui-peer/).

> **Status: alpha.** `0.0.0-alpha.1` is a pre-release: the API may change in any release
> before 1.0.

## Features

- **`PeerId`**: a peer's id as `<alg>:<hash>` text (`sha256:3f15...`), parsed from the forms
  people paste (bare hex, colon-separated hex, grouped base32), compared in constant time.
- **`TrustStore`**: accepted and waiting peers on disk. A peer is recorded as waiting only
  after it proved its key, bounded per source (IPv6 grouped by /64) and in total; an operator
  accepts or removes it by id. `WaitingQueue` is the same bounding for a host that stores
  waiting entries elsewhere.
- **Invitations and key rotation**: one-time pairing secrets the store keeps only as a hash,
  redeemed atomically; a successor key announced under the current one, promoted once proven,
  the old key retiring after a window you choose.
- **Pairing** (module `pair`): a joiner and a responder accept each other's keys by an
  invitation, a short PIN checked by SPAKE2 (feature `pin`), or an RFC 8628-style device flow.
  Sans-IO; codes, pending joiners and decisions are data your app shows and acts on, and every
  limit is an option with a default.
- **A sans-IO `Handshake`**: feed received bytes or messages in, get bytes or messages out, over
  a byte stream, WebSocket messages or datagrams. Blocking drivers, with deadlines; async ones
  with the `tokio` feature. One length-prefixed frame format for every message.
- **Files and PEM**: the store's atomic, no-replace writes and its PEM format, for method crates.
- Plain data and no process-wide state; nothing reads a file or an environment variable unless
  you name a directory.

## Installation

```bash
cargo add coqui-peer@0.0.0-alpha.1
```

| Feature | Adds | Needed for | Default |
| --- | --- | --- | --- |
| `serde` | `serde` | reading and writing `PeerId` and `Digest` as text, in the format you choose | off |
| `tokio` | `tokio` (`io-util`, `time`) | the `asynch` drivers and frame functions | off |
| `pin` | `spake2`, `hkdf`, `hmac`, `rand_core` | PIN pairing (`Pairing::pin`), `Random::os` | off |
| `coqui-transport` | `coqui-transport` | module `transport`: `PeerFrame`, and a `coqui-transport` channel as a `MessageTransport` | off |

Rust 1.89 or later. A default build depends on `data-encoding`, `guatiao`, `guatiao-serde`, `sha1`, `sha2`, `subtle` and
`zeroize`.

## Quick start

Open a store, record a peer that has proved its key, and let the operator accept it:

```rust,no_run
use coqui_peer::{KeyMaterial, PeerId, TrustStore};

let store = TrustStore::open(std::path::Path::new("/var/lib/app"))?;

// A method hands the store the peer's public key; the id is its SHA-256.
let key = KeyMaterial::new("example", "PUBLIC KEY", vec![0u8; 32])?;
store.record_waiting(&key, Some("192.0.2.7:4000".parse()?)); // only after the key proof
store.accept(&key.id())?;
assert!(store.is_accepted(&key.id()));

// An id from text, in any accepted spelling.
let pinned: PeerId =
    "sha256:630dcd2966c4336691125448bbb25b4ff412a49c732db2c8abc1b8581bd710dd".parse()?;
assert_eq!(pinned, key.id());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Settings go in a builder, by name, and a settings type fills them whole from a file:

```rust,no_run
use std::time::Duration;
use coqui_peer::{TrustStore, TrustStoreSettings, WaitingLimits, WhenFull};

let mut limits = WaitingLimits::default();
limits.per_source = 4;
let store = TrustStore::builder(std::path::Path::new("/var/lib/app"))
    .limits(limits)
    .when_full(WhenFull::ReplaceOldest)
    .record_interval(Duration::from_millis(500))
    .build()?;
// What differs from the defaults, named by key path, for the application to show.
for setting in store.non_defaults() {
    println!("{} = {} (default {})", setting.setting, setting.value, setting.default);
}
# let _ = TrustStoreSettings::default();
# Ok::<(), Box<dyn std::error::Error>>(())
```

`TrustStoreSettings`, `WaitingSettings`, `pair::PairingDeskSettings` and the option types they hold are
guatiao values: `guatiao_serde::text::toml` (or json, yaml) reads them, an absent key is its default and an
unknown key is refused. `builder(..).settings(s)` takes one whole.

Running a method's handshake over a stream, with a deadline:

```rust,ignore (needs a method crate, such as coqui-peer-tls)
let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
let established = coqui_peer::drive_stream_until(method.initiate()?, &mut socket, deadline)?;
let peer: PeerId = established.peer;        // verified
let channel = established.session;          // what the method produced
```

## API overview

| Module | Purpose |
| --- | --- |
| `coqui_peer` | `PeerId`, `Digest`; `TrustStore`, `WaitingQueue`, invitations and rotation; `KeyMaterial`; `Clock`, `Random`, `NonDefault`; the `Handshake` and `Method` traits, the drivers and the frame functions; `Error`. |
| `coqui_peer::pair` | Pairing by invitation, PIN or device flow: `Pairing`, `PairingJoiner`, `PairingResponder`, `PairingDesk` and their options. |
| `coqui_peer::asynch` | The drivers and frame functions over tokio streams and async message transports (`tokio`). |
| `coqui_peer::file` | Atomic writes: `write_new` never replaces, `replace` does; owner-only on request. |
| `coqui_peer::pem` | The PEM block format the store writes, with its header lines. |

## The key-proof contract

A peer is recorded as waiting only after it proved, inside the handshake, that it holds the
private key of the key it presented. A presented key proves nothing: public keys and
certificates are copied freely, and recording them unproven would let a stranger queue
somebody else's identity. Every method crate meets this contract; it is documented on
`TrustStore`.

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
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-peer/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` runs the release workflow: the tests and a strict documentation
build gate a GitHub release, marked as a pre-release when the version has a `-` suffix. The
crate is published to crates.io with `cargo publish` by a maintainer.

## License

MPL-2.0; see [LICENSE](https://github.com/Boriken-Dev/coqui-peer/blob/main/LICENSE).
