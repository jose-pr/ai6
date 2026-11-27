# Contributing to cabuya

## Licence

This project is distributed under the **Mozilla Public License 2.0** (`LICENSE`). Its copyleft is per file: using the
crate imposes nothing on your code, and a modified file must be published under the MPL 2.0.

## Contributor licence agreement

A first pull request needs a one-time signature. The bot will comment with the exact sentence to reply with; read
[`CLA.md`](CLA.md) first. You keep the copyright in everything you write.

## Before you open a pull request

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
(cd bindings/dart/cabuya && dart analyze && dart test)
(cd bindings/js && npm test)
```

A change in how a peer behaves on the wire changes `vectors/mux.json`, which every language runs, and must stay compatible with
`coqui-agentic-peer`. `AGENTS.md` is the API header and ships in the package; keep it current in the same commit. Files are LF.
