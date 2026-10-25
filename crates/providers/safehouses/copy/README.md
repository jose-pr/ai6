# ai6-safehouse-copy

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_safehouse_copy/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

The `copy` safe-house provider for [AI6](https://github.com/jose-pr/ai6): **each Mission
works in a plain copy of the Operation's directory**, with no version control needed. It
keeps no history, so a Mission's cover report says its commits were **not checkable**
rather than passing them. The copy is always kept when the Mission ends. See the
[API header](https://github.com/jose-pr/ai6/blob/main/crates/providers/safehouses/copy/AGENTS.md).

## Features

- A copy per Mission under AI6's state directory, never inside the source.
- Works for any directory: no Git, no other tool on the `PATH`.
- Proves the safe-house contract with a provider that has no history.

## Installation

Part of the AI6 workspace; built with it:

```bash
cargo build -p ai6 --features cli -p ai6-safehouse-copy
```

## Quick start

```toml
[operation.G]
path = "~/work/notes"
workspace = { provider = "copy" }
```

## Development

```bash
cargo test -p ai6-safehouse-copy
cargo clippy -p ai6-safehouse-copy --all-targets -- -D warnings
```

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
