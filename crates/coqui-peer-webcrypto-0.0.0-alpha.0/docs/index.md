# coqui-peer-webcrypto

Two peers prove their P-256 keys to each other and end up with an encrypted session, in a browser or
natively, through anything that carries their messages. This is a method of
[coqui-peer](https://boriken-dev.github.io/coqui-peer/), which holds the peer id, the trust store and
the sans-IO handshake trait.

!!! warning "Unreleased, not independently reviewed"
    The wire and the API may change at any commit. There are no releases for now: depend on a git
    revision.

```bash
cargo add --git https://github.com/Boriken-Dev/coqui-peer-webcrypto --rev <commit> coqui-peer-webcrypto
```

## Why a session, and not only a proof

A proof alone says who was there when it was made. Anything carrying the connection, such as a reverse
proxy that ends TLS or a relay a user was lured to, can forward a proof unchanged, watch both ends
succeed, and then speak for either. This method therefore ends in keys only the two peers hold, and an
application keeps every later message inside that session. Nothing depends on the TLS beneath it.

A party carrying the connection can forward it, see public keys and record sizes, delay it and cut it,
and a cut is reported. It cannot read or change a record, replace a key, or stand in for a pinned peer. It can, if it also
serves a page's JavaScript, replace that JavaScript, so a browser application ships the module with
itself.

## Pairing

Two strangers run a handshake that accepts any proved key and ends in a type that can only pair, then
[coqui-peer's pairing](https://boriken-dev.github.io/coqui-peer/pairing/) sealed inside it, bound to the
session and not to any TLS. Afterwards the full key is what each handshake proves. The JavaScript
module has no PIN exchange; a browser's id is carried to its peer out of band.

The API reference is under
[Rust API](https://boriken-dev.github.io/coqui-peer-webcrypto/rust/index.html) for `main`. The source, the
wire protocol (`PROTOCOL.md`), the API headers (`AGENTS.md`) and the changelog are in the
[repository](https://github.com/Boriken-Dev/coqui-peer-webcrypto).
