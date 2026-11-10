# coqui-builder-derive

[![Version](https://img.shields.io/crates/v/coqui-builder-derive.svg)](https://crates.io/crates/coqui-builder-derive)
[![Rust version](https://img.shields.io/crates/msrv/coqui-builder-derive.svg)](https://crates.io/crates/coqui-builder-derive)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-builder/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-builder/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-builder/test.yml)](https://github.com/Boriken-Dev/coqui-builder/actions/workflows/test.yml)

The **derive macro of `coqui-builder`**: `#[derive(Describe)]` writes what a settings type is,
how it is filled one key at a time and how a key of a value is read, from one declaration.
It is reached through `coqui-builder`'s `derive` feature and is never named directly.

## Features

- **`#[derive(Describe)]`** — a struct of named fields, a newtype, an enum of unit variants,
  a tagged enum, and an untagged enum of text arms with at most one structured arm.
- **`#[derive(Leaf)]`** — a type that is one setting as one text: a unit-only enum by its
  variant names, any other type by `Display` and `FromStr`.
- **The spellings guatiao's derives read** — `#[map(..)]`, `#[schema(..)]`, so a type can move
  between the two; vectors in the repository hold both to the same meaning.
- **A refusal is a sentence** — an unsupported shape or a contradictory pair of attributes is
  a compile error on the declaration that says what to do instead.

## Installation

```bash
cargo add coqui-builder --features derive
```

## Quick start

```rust
use coqui_builder::Describe;

#[derive(Describe)]
struct Limits {
    #[map(default = 16)]
    waiting: u32,
}
```

## API overview

| Item | Purpose |
| --- | --- |
| `Describe` | the derive; attributes `map`, `schema` and `builder` |
| `Builder` | the builder of a parts struct; `#[builder(build, name, fallible, entry, crate)]`, `#[builder(input, setter)]` |
| `Configured` | the settings view of a built type; `#[configured]` on its one settings field |

## Development

```bash
cargo test -p coqui-builder-derive
cargo test -p coqui-builder --all-features
```

### Releasing

Released with `coqui-builder`, under the same version; see the
[repository](https://github.com/Boriken-Dev/coqui-builder).

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-builder/blob/main/LICENSE).
