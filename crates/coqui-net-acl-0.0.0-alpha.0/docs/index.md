# coqui-net-acl

An ordered, first-match access control list over source networks, for any listener, with or
without TLS. Rules are read top to bottom, the first match wins, and a default action (deny
unless set) applies when none does. It can be evaluated before the first byte is read and again
after a handshake with the verified peer fingerprint.

!!! note "Alpha"
    `0.0.0-alpha.0` is a pre-release: the API may change in any release before 1.0.

```toml
coqui-net-acl = "0.0.0-alpha.0"
```

The API reference is on [docs.rs](https://docs.rs/coqui-net-acl) and under
[Rust API](https://boriken-dev.github.io/coqui-net-acl/rust/index.html) for `main`. The source,
the API header (`AGENTS.md`) and the changelog are in the
[repository](https://github.com/Boriken-Dev/coqui-net-acl).
