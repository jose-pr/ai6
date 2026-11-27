# cabuya

[![Version](https://img.shields.io/crates/v/cabuya.svg)](https://crates.io/crates/cabuya)
[![Rust version](https://img.shields.io/crates/msrv/cabuya.svg)](https://crates.io/crates/cabuya)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/cabuya/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/cabuya)](https://docs.rs/cabuya)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/cabuya/test.yml)](https://github.com/Boriken-Dev/cabuya/actions/workflows/test.yml)

**One proven connection between two peers, and any number of named streams over it.** Pair once, prove who holds a key, then
run whatever protocols the application needs, each as a stream with its own flow control and its own grant. It adds no dialect:
a stream carries the application's own messages. See the [documentation](https://boriken-dev.github.io/cabuya/).

> **Status: alpha.** `0.0.0-alpha.0` is a pre-release: the API may change in any release before 1.0.

## Features

- **Streams** (`mux`): named channels inside one link with credit per stream, metadata on open, a distinct cause for every end,
  and an optional heartbeat. The same wire in Rust, Dart and JavaScript, on one set of vectors.
- **Serving** (`link`, `peers`): who may open what. A proof admits a peer and grants nothing; each open is decided against the
  peer's record, and each running stream again, so a grant taken away ends its stream. A store that cannot be read refuses.
- **Proof and pairing** (`auth`, `pairing`, feature `sealed`): Noise or WebCrypto identities, a sealed channel, and PIN
  pairing over any channel.
- **Forwarding** (`forward`, feature `forward`): one TCP connection per stream, in either direction, each target its own grant, or a rule (name, `*.suffix`, address or network, port range) the operator writes.

## Installation

```sh
cargo add cabuya --git https://github.com/Boriken-Dev/cabuya
```

| Feature | Adds | Default |
| --- | --- | --- |
| `sealed` | `auth` and `pairing`, with `coqui-peer`, `coqui-peer-noise` and `coqui-peer-webcrypto` | no |
| `forward` | `forward`, with `socket2` and `coqui-net-acl` | no |

Without a feature the crate needs only `coqui-transport` and `serde_json`: a streams-only user carries no cryptography.

## Quick start

Two ends of a channel, one stream opened and echoed:

```rust
use std::time::Duration;

use cabuya::mux::{Limits, Mux, Side};
use coqui_transport::frame::LengthPrefix;
use coqui_transport::{Channel, StreamChannelBuilder, memory};

let (one, other) = memory::pipe(1 << 16);
let channel = |stream| {
    StreamChannelBuilder::new()
        .codec(LengthPrefix::u32_le())
        .stream(stream)
        .build()
};
let (near, far) = (channel(one)?, channel(other)?);
let wait = Some(Duration::from_secs(5));

std::thread::scope(|scope| {
    scope.spawn(|| {
        Mux::run(&far, Side::Answered, Limits::default(), None, |mux| {
            if let Ok(Some(incoming)) = mux.accept(wait) {
                let message = incoming.stream.receive(wait).unwrap().unwrap();
                incoming.stream.send(&message).unwrap();
            }
        })
    });
    Mux::run(&near, Side::Dialed, Limits::default(), None, |mux| {
        let stream = mux.open("echo/1").unwrap();
        stream.send(b"hello").unwrap();
        assert_eq!(stream.receive(wait).unwrap().unwrap(), b"hello");
    });
});
# Ok::<(), Box<dyn std::error::Error>>(())
```

## API overview

| Module | Purpose |
| --- | --- |
| `mux` | named streams inside one link: frames, limits, flow control, ends |
| `link` | serving a link under grants, with a capability document and admission |
| `peers` | a peer's claims and grants, behind a store trait, and audit events |
| `auth` (`sealed`) | identities, proof by Noise or WebCrypto, and the sealed channel |
| `pairing` (`sealed`) | PIN pairing over any channel |
| `forward` (`forward`) | `tcp/1`: a TCP connection per stream |

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
(cd bindings/dart/cabuya && dart analyze && dart test)
(cd bindings/js && npm test)
```

`tests/interop.rs` and `tests/vectors.rs` run against `coqui-agentic-peer`, which speaks the same wire.

## License

MPL-2.0; see [LICENSE](https://github.com/Boriken-Dev/cabuya/blob/main/LICENSE).
