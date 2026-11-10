# coqui-builder

[![Version](https://img.shields.io/crates/v/coqui-builder.svg)](https://crates.io/crates/coqui-builder)
[![Rust version](https://img.shields.io/crates/msrv/coqui-builder.svg)](https://crates.io/crates/coqui-builder)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-builder/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-builder/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-builder/test.yml)](https://github.com/Boriken-Dev/coqui-builder/actions/workflows/test.yml)

**Describe, fill, check and build any struct field by field: builders, partials and profiles,
with no value model and no dependencies.** Settings are the main example. One derive says what
each key is; from that one description a type is filled from any
source, a missing key is named, a value is compared with its defaults, and a profile says
whether a value meets what is expected, with every secret hidden. See the
[documentation site](https://boriken-dev.github.io/coqui-builder/).

## Features

- **One description** — `#[derive(Describe)]` writes what a settings type is: keys, kinds,
  defaults, bounds, sections, secrets. Nothing is described twice. A leaf is one text: it
  parses and prints its own, and `#[derive(Leaf)]` makes a type one from `Display` and `FromStr`.
- **Fill from anywhere** — set a key by path (`waiting.per_source`, `peers[1].host`), from
  text, or from any `Source` such as the environment; the last write of a key wins, and a
  list is replaced whole.
- **Fails once, completely** — `Partial::build` lists every key still missing, not the first.
- **A builder from the description** — `#[derive(Builder)]` on a parts struct writes typed
  setters, layers (`fill`), and a `build` that names every missing setting and input at once;
  `parts()` gives what `build` converts, for a terminal written by hand.
- **Non-defaults as data** — `non_defaults` lists each setting that differs from its default
  as `{setting, value, default}`, each a type and its text (`u32(64)`); a secret is `String(***)`.
  Values are compared as typed values, so `30s` equals `30000ms`.
- **Profiles** — expect an exact value, a range, a set, a stricter-or-looser order, an
  optional group present or absent, or an arm of a variant; check a value and read why not.
- **No dependencies** — `no_std` with `alloc`; the derive, `zeroize` and `serde` are features.

## Installation

```bash
cargo add coqui-builder --features derive
```

| Feature | Adds | Needed for |
| --- | --- | --- |
| `std` (default) | `PathBuf`, `HashMap` and the `Env` source | the environment as a source |
| `derive` | `coqui-builder-derive` (`syn`, `quote`) | `#[derive(Describe)]` |
| `zeroize` | `zeroize` | `Secret<T>`; sensitive datums, source texts for sensitive places and `Env` values are wiped on drop |
| `serde` | `serde` (and its `serde_core`) | `Deserialize` for `Partial<T>`: any serde format fills a `Partial` |

## Quick start

```rust
use coqui_builder::{Check, Describe, Expect, Partial, Profile, non_defaults};

#[derive(Describe, Debug, PartialEq)]
struct Limits {
    /// How many sources may wait at once.
    #[map(default = 16)]
    waiting: u32,
    /// The key to the other side.
    #[schema(sensitive)]
    #[map(default)]
    secret: String,
}

// Fill a key at a time, from any source, and build.
let mut partial = Partial::<Limits>::new();
partial.set("waiting", "64")?;
partial.set("secret", "hunter2")?;
let limits = partial.build()?;
assert_eq!(limits.waiting, 64);

// Say what differs from the defaults; a secret is never shown.
let found = non_defaults(&limits);
assert_eq!(found[0].to_string(), "secret = String(***) (default String(***))");
assert_eq!(found[1].to_string(), "waiting = u32(64) (default u32(16))");

// Say what is expected, and read why a value does not meet it.
let profile = Profile::new().expecting(
    "waiting",
    Expect::Within {
        min: Some("1".into()),
        max: Some("32".into()),
    },
)?;
let report = profile.verify(&limits);
assert!(!report.is_met());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## A builder

A settings type is described once; a parts struct holds it beside the inputs that are no text,
and derives the builder.

```rust
use coqui_builder::{Builder, Describe, Partial};

#[derive(Describe)]
struct ServerSettings {
    host: String,
    #[map(default = 8080)]
    port: u16,
}

struct Server {
    settings: ServerSettings,
    identity: String,
}

#[derive(Builder)]
#[builder(build = Server)]
struct ServerParts {
    settings: ServerSettings,
    #[builder(input)]
    identity: String,
}

impl From<ServerParts> for Server {
    fn from(parts: ServerParts) -> Server {
        Server { settings: parts.settings, identity: parts.identity }
    }
}

// Layers apply in order (a file, the environment, flags, then code): the last write wins.
let mut file = Partial::<ServerSettings>::new();
file.set("host", "example")?;
let server = Server::builder().fill(file).port(9000u16).identity("srv").build()?;
assert_eq!(server.settings.port, 9000);

// A build that lacks something names all of it.
let missing = Server::builder().build().err().unwrap();
assert_eq!(missing.to_string(), "missing settings: host; missing inputs: identity");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## API overview

| Module | Purpose |
| --- | --- |
| `describe` | `Describe`, `Description`, `Kind`, `Field`, `Leaf`: what a type is; `Codec` for `with` and `as_text` |
| `datum` | `Datum`: a type name and a text, `u32(64)`; `Shown`: what a finding holds |
| `path` | `KeyPath` and `Segment`: `agent[1].name` |
| `fill` | `Fill` and the slots a partly filled type is made of |
| `partial` | `Partial`: a type with some keys set, and `Missing` |
| `builder` | `Holds`, `Configured`, `BuildError`: what a generated builder shares |
| `source` | `Source`, `Entries`, `FillError`: filling from anything; `env::Env` reads variables |
| `inspect` | `Inspect`: reading a key of a value |
| `report` | `Report`, `Finding`, `Reason`: what a check found |
| `check` | `Check`: anything that judges a value |
| `profile` | `Profile` and `Expect`: what a value is expected to be |
| `non_default` | `NonDefault`, `non_defaults`, `differences` |

## Development

```bash
cargo test --workspace --all-features
cargo test --workspace --no-default-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo check -p coqui-builder --no-default-features --target wasm32-unknown-unknown
```

The committed public API is `tools/public-api.txt`; regenerate it in the commit that changes
the API:

```bash
cargo +nightly-2026-09-21 public-api -p coqui-builder --all-features -sss --color never > tools/public-api.txt
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](CHANGELOG.md). Pushing a tag matching `v*` triggers the release
workflow: test gate → a strict docs build as a gate → GitHub release. The release workflow
never deploys the docs site itself: for a final release its last job dispatches the docs
workflow at the tag, which owns every Pages deploy.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-builder/blob/main/LICENSE).
