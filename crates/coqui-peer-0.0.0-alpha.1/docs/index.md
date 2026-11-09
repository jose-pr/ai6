# coqui-peer

Who a peer is and whether we trust it, independent of how the peer authenticates. The crate
has no TLS and no crypto transport. It holds what every authentication method shares: the
`<alg>:<hash>` peer id, a trust store with the pairing flow and key rotation, and a sans-IO
handshake trait that runs over byte streams and message transports alike.

!!! note "Alpha"
    `0.0.0-alpha.1` is a pre-release: the API may change in any release before 1.0.

```toml
coqui-peer = "0.0.0-alpha.1"
```

[Pairing](pairing.md) explains how two devices come to trust each other's keys by an
invitation, a PIN or a device flow, and what an attacker can and cannot do in each.

Methods are crates of their own: [`coqui-peer-tls`](https://crates.io/crates/coqui-peer-tls)
is the TLS one, and [`coqui-peer-webcrypto`](https://github.com/Boriken-Dev/coqui-peer-webcrypto)
the one for browsers.

The API reference is on [docs.rs](https://docs.rs/coqui-peer) and under
[Rust API](https://boriken-dev.github.io/coqui-peer/rust/index.html) for `main`. The source,
the API header (`AGENTS.md`) and the changelog are in the
[repository](https://github.com/Boriken-Dev/coqui-peer).
