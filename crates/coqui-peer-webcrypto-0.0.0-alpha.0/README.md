# coqui-peer-webcrypto

[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-peer-webcrypto/blob/main/LICENSE)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-peer-webcrypto/test.yml)](https://github.com/Boriken-Dev/coqui-peer-webcrypto/actions/workflows/test.yml)

**Two peers prove their P-256 keys to each other and end up with an encrypted session, in a browser
or natively, through anything that carries their messages.** This is a method of
[`coqui-peer`](https://github.com/Boriken-Dev/coqui-peer), which holds the peer id, the trust store and the
sans-IO handshake trait. It depends on no TLS session beneath it, so it works behind a
TLS-terminating reverse proxy, and such a proxy can connect the two peers and nothing more. See the
[documentation](https://boriken-dev.github.io/coqui-peer-webcrypto/) and the
[wire protocol](https://github.com/Boriken-Dev/coqui-peer-webcrypto/blob/main/PROTOCOL.md).

> **Status: unreleased, and not independently reviewed.** The wire and the API may change at any
> commit. There are no releases for now: depend on a git revision.

## Features

- **Mutual key proof** over P-256 ECDSA: each side's signature names both keys, both nonces and both
  ephemeral keys, in a role of its own, so a proof cannot be replayed, reflected or redirected.
- **A session**: ephemeral ECDH gives an id both ends agree on, per-direction AES-256-GCM records, a
  close record that makes a cut stream visible, a usage limit and exports. A party carrying the
  connection cannot read or change what goes through it, or end it unseen.
- **Trust from `coqui-peer`**: fixed pins, or a live `TrustStore` read at each handshake, with waiting
  entries for proved strangers, refused rather than evicted, and rotation to a proved successor.
- **Pairing on a session of its own**, sealed and bound to the session and not to any TLS. A stranger's
  session is a different type, which carries the exchange and nothing else (PIN needs feature `pin`).
- **Both roles in both languages**: Rust here, and a dependency-free JavaScript module in
  [`bindings/js/coqui-peer-webcrypto`](https://github.com/Boriken-Dev/coqui-peer-webcrypto/tree/main/bindings/js/coqui-peer-webcrypto)
  that keeps its key non-extractable, tested against each other and in a real browser.
- **Exactly one encoding** of every message and key, refused otherwise, with shared test vectors.

## Installation

```bash
cargo add --git https://github.com/Boriken-Dev/coqui-peer-webcrypto --rev <commit> coqui-peer-webcrypto
```

This crate names `coqui-peer` by git revision too. A consumer that uses `coqui-peer` directly names the
same revision, as `Cargo.toml` here does, so both see one `PeerId` type.

| Feature | Adds | Needed for | Default |
| --- | --- | --- | --- |
| `pin` | `coqui-peer/pin` (SPAKE2, HKDF, HMAC) | PIN pairing (`pair::Pairing::pin`) | off |

Rust 1.89 or later. A default build depends on `coqui-peer`, `p256`, `aes-gcm`, `hkdf`, `sha2`,
`data-encoding`, `serde`, `serde_json`, `getrandom` and `zeroize`.

## Quick start

Prove both keys over any message transport, then keep every message inside the session:

```rust
use std::sync::mpsc::{Receiver, Sender, channel};
use std::{io, thread};

use coqui_peer::{MessageTransport, Method, drive_messages};
use coqui_peer_webcrypto::{Identity, Trust, WebCrypto};

struct Pipe(Sender<Vec<u8>>, Receiver<Vec<u8>>);

impl MessageTransport for Pipe {
    fn send(&mut self, message: &[u8]) -> io::Result<()> {
        self.0.send(message.to_vec()).map_err(|_| io::ErrorKind::BrokenPipe.into())
    }
    fn recv(&mut self) -> io::Result<Vec<u8>> {
        self.1.recv().map_err(|_| io::ErrorKind::UnexpectedEof.into())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (dialer, listener) = (Identity::generate()?, Identity::generate()?);
    let client = WebCrypto::builder().identity(dialer.clone()).responders(Trust::pinned([listener.peer_id()])).build()?;
    let server = WebCrypto::builder().identity(listener).initiators(Trust::pinned([dialer.peer_id()])).build()?;

    let ((to_server, from_client), (to_client, from_server)) = (channel(), channel());
    let serving = thread::spawn(move || {
        let mut pipe = Pipe(to_client, from_client);
        drive_messages(server.respond(None).expect("a role with trust"), &mut pipe)
    });
    let mut pipe = Pipe(to_server, from_server);
    let at_client = drive_messages(client.initiate()?, &mut pipe)?;
    let at_server = serving.join().expect("the thread ends")?;

    assert_eq!(at_server.peer, dialer.peer_id());
    let (mut near, mut far) = (at_client.session, at_server.session);
    assert_eq!(near.id(), far.id());
    let record = near.seal(b"anything the application says")?;
    assert_eq!(far.open(&record)?.as_deref(), Some(&b"anything the application says"[..]));
    // The last record says the stream was not cut.
    assert_eq!(far.open(&near.close()?)?, None);
    far.verify_end()?;
    Ok(())
}
```

Identities live in files, as PKCS#8 PEM that `openssl genpkey` can also write:

```rust,no_run
use std::path::Path;

use coqui_peer_webcrypto::Identity;

# fn main() -> Result<(), coqui_peer_webcrypto::Error> {
let identity = Identity::open_or_create(Path::new("identity.pem"))?;
println!("tell your peer this id: {}", identity.peer_id());
# Ok(())
# }
```

## API overview

Everything is at the crate root; the shipped `AGENTS.md` is the full header.

| Item | Purpose |
| --- | --- |
| `Identity` | a long-term P-256 key: generate, read, write, id |
| `Trust` | whom a role admits: pins or a store |
| `WebCrypto`, `WebCryptoHandshake` | the `coqui_peer::Method` and its `Handshake` |
| `Session`, `Sealer`, `Opener` | the id, records, close and exports a handshake ends in |
| `PairingMethod`, `PairingHandshake`, `PairingSession`, `PairDial` | a stranger's handshake, and the sealed pairing drivers it ends in |
| `FramedStream` | a byte stream as a message transport that refuses a frame by its declared length |
| `Error` | why a handshake, a record, a pairing or an identity failed |
| `pair` | `coqui_peer::pair`, which runs on a `PairingSession` |

## Behind a reverse proxy

The handshake and session use nothing of the connection they run over. A proxy that ends TLS is a
relay: it can forward, see public keys and record sizes, delay and cut, and a cut is reported. It cannot
read or change a record, replace a key, or stand in for a pinned peer. This protection covers **only
what goes through the session**: a plain message sent after the handshake comes from whoever carries the connection. A
proxy that also serves a page's JavaScript can replace that JavaScript. The full table is in
[PROTOCOL.md](https://github.com/Boriken-Dev/coqui-peer-webcrypto/blob/main/PROTOCOL.md#through-a-relay).

An endpoint that really ends TLS itself may also bind the handshake to that connection's exporter
with `WebCryptoHandshake::bind`, which refuses a relay ending TLS on both sides.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --no-default-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cd bindings/js/coqui-peer-webcrypto && npm test
```

The Node tests run the Rust example in both roles and a real browser against it (Chrome or Edge, or
`COQUI_TEST_BROWSER`), and report a skip where cargo or a browser is absent;
`COQUI_PEER_WEBCRYPTO_REQUIRE_PEERS=1`, as in CI, makes that a failure.

### Releasing

There are no releases for now, and dependents pin a git revision. When there are,
`.github/workflows/release.yml` runs on a `v*` tag: it gates on the tests and a strict docs build, then
creates the release from `CHANGELOG.md`. Every tag needs the maintainer's explicit go-ahead.

## License

[MPL-2.0](https://github.com/Boriken-Dev/coqui-peer-webcrypto/blob/main/LICENSE).
