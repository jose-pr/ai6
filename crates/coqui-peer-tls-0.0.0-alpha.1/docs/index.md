# coqui-peer-tls

Mutual-TLS identity between peers, with no RPC layer. Each side has a self-issued certificate
and tells the other only its fingerprint. You get back an authenticated, encrypted byte stream
and the verified peer identity, with pinned, CA and key-proof trust (and, for a client,
servers verified by the web PKI), a trust store with a waiting queue for pairing, policy
presets (`modern`, `intermediate`, `legacy`) and, with the `acl` feature, an access control list
every server configuration applies.

!!! note "Alpha"
    `0.0.0-alpha.1` is a pre-release: the API may change in any release before 1.0.

```toml
coqui-peer-tls = "0.0.0-alpha.1"
```

Two devices that have never met can pair without reading fingerprints: `pair_dial` and
`pair_accept` run [coqui-peer's pairing](https://boriken-dev.github.io/coqui-peer/pairing/)
(an invitation, a PIN checked by SPAKE2, or a device flow) on a TLS connection of its own,
under your device's policy and bound to that connection. A paired peer can later rotate its
certificate: it announces the successor, which is accepted once proven while the old one
retires.

The API reference is on [docs.rs](https://docs.rs/coqui-peer-tls) and under
[Rust API](https://boriken-dev.github.io/coqui-peer-tls/rust/index.html) for `main`. The source,
the API header (`AGENTS.md`) and the changelog are in the
[repository](https://github.com/Boriken-Dev/coqui-peer-tls).
