# tplx-rs

[![Version](https://img.shields.io/crates/v/tplx-rs.svg)](https://crates.io/crates/tplx-rs)
[![Rust version](https://img.shields.io/crates/msrv/tplx-rs.svg)](https://crates.io/crates/tplx-rs)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/jose-pr/tplx-rs/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/tplx-rs)](https://docs.rs/tplx-rs)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/tplx-rs/test.yaml)](https://github.com/jose-pr/tplx-rs/actions/workflows/test.yaml)

Template languages as they are, behind one call: the Rust counterpart of the Python library
[tplx](https://github.com/jose-pr/tplx). **Text and data go in, text comes out, and the core does no I/O**:
Mustache partials come from a resolver you supply, every render is bounded by output, depth and iteration limits,
and the crate builds for `wasm32-unknown-unknown`. An engine is limited by its options, never rewritten.

## Features

- **`envsubst`**: `$NAME` and `${NAME}` in three dialects, as tplx has them: `gnu` (GNU gettext, the default),
  `a8m` (defaults, alternates, `$$`) and `drone` (braced only, slices, replacements, trims, case, length). Options
  for the dialect, the pattern, the features and what an undefined name does (`strict`, the default as in tplx, `lenient` or `debug`).
- **`mustache`**: the official Mustache specification (variables, sections, inverted sections, comments,
  delimiters, partials); HTML-escaping `{{x}}` by default, or none.
- **`raw`**: the text as it is.
- **One call by name**: `render("mustache", text, &data, &options)`, a registry of lowercase names and suffixes.
- **Sandbox limits** on output bytes, nesting depth and iterations, and an `Error` that tells an unknown engine,
  a refused template, an undefined name and a reached limit apart.

## Installation

```bash
cargo add tplx-rs
```

| Feature | Adds | Default |
| --- | --- | --- |
| `std` | `std::error::Error` for the error types (without it they implement `core::error::Error`) | yes |

The crate is `no_std` with `alloc`. Its only dependencies are `serde_json` (the data type) and `thiserror`.

## Quick start

```rust
use tplx_rs::{render, Options};

let data = serde_json::json!({"name": "world", "items": ["a", "b"]});
let out = render("mustache", "hello {{name}}:{{#items}} {{.}}{{/items}}", &data, &Options::new())?;
assert_eq!(out, "hello world: a b");
# Ok::<(), tplx_rs::Error>(())
```

envsubst reads names case-insensitively, flattens nested data (`{"target": {"host": "h"}}` is `$TARGET_HOST`) and
follows the dialect you pick:

```rust
use tplx_rs::{render, Dialect, EnvsubstOptions, Options, Undefined};

let data = serde_json::json!({"HOST": "web01"});
// GNU envsubst: no defaults, so `${PORT:-80}` is text; a name with no value is an error...
assert_eq!(render("envsubst", "$HOST ${HOST}/${PORT:-80}", &data, &Options::new())?, "web01 web01/${PORT:-80}");
assert!(render("envsubst", "$NOPE", &data, &Options::new()).is_err());

// ...unless the mode says it is empty, as GNU envsubst does.
let mut settings = EnvsubstOptions::default();
settings.undefined = Undefined::Lenient;
assert_eq!(render("envsubst", "[$NOPE]", &data, &Options::new().envsubst(settings))?, "[]");

settings.dialect = Dialect::A8m;
settings.undefined = Undefined::Debug; // keep what has no value, as an nginx template needs
let options = Options::new().envsubst(settings);
assert_eq!(render("envsubst", "${PORT:-80} $request_uri", &data, &options)?, "80 $request_uri");
# Ok::<(), tplx_rs::Error>(())
```

Limits bound a render, and a resolver gives Mustache its partials:

```rust
use std::collections::BTreeMap;
use tplx_rs::{render, Error, Limit, Limits, Options};

let mut partials = BTreeMap::new();
partials.insert("row".to_string(), "<{{.}}>".to_string());
let data = serde_json::json!({"xs": [1, 2]});
let options = Options::new().resolver(&partials);
assert_eq!(render("mustache", "{{#xs}}{{>row}}{{/xs}}", &data, &options)?, "<1><2>");

let mut limits = Limits::default();
limits.max_output = 4;
assert_eq!(render("raw", "too long", &data, &Options::new().limits(limits)), Err(Error::Limit(Limit::Output)));
# Ok::<(), Error>(())
```

## API overview

| Item | Purpose |
| --- | --- |
| `render`, `Engine` | render by engine name; the registry, `Engine::by_name` and `Engine::by_suffix` |
| `render_envsubst`, `render_mustache`, `render_raw` | one function per engine |
| `Options`, `Limits`, `Resolver` | per-render settings, sandbox limits, and the source of Mustache partials |
| `EnvsubstOptions`, `Dialect`, `Pattern`, `Feature`, `Features`, `Undefined` | the envsubst engine's settings |
| `MustacheOptions`, `Escape` | the Mustache engine's settings |
| `Error`, `SyntaxKind`, `Limit` | the error and what it tells apart |
| `Value` | the data type, `serde_json::Value` |

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cargo check --target wasm32-unknown-unknown --no-default-features
```

The Mustache specification's tests are kept as JSON under `tests/data/mustache-spec/`. The envsubst tests compare
with GNU gettext's envsubst (`tests/data/gnu-envsubst.json`) and with tplx's engine of the same name
(`tests/data/tplx-envsubst.json`).

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/jose-pr/tplx-rs/blob/main/CHANGELOG.md). Pushing a tag matching `v*` triggers
the release workflow: test gate, package, GitHub release, publish.

## License

MIT, see [LICENSE](https://github.com/jose-pr/tplx-rs/blob/main/LICENSE).
