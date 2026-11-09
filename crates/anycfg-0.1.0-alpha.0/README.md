# anycfg

[![Version](https://img.shields.io/crates/v/anycfg.svg)](https://crates.io/crates/anycfg)
[![Rust version](https://img.shields.io/crates/msrv/anycfg.svg)](https://crates.io/crates/anycfg)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/anycfg-rs/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/anycfg)](https://docs.rs/anycfg)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/anycfg-rs/test.yml)](https://github.com/jose-pr/anycfg-rs/actions/workflows/test.yml)

**Find a configuration file, read it in the format it names, and refuse it in words that name the
key and never quote a value.** `anycfg` has no format of its own: an application registers its
parsers, chooses its value type and decides how layers merge. See the
[documentation](https://jose-pr.github.io/anycfg-rs/).

> **Status:** not published. `0.1.0-alpha.0` is a pre-release; the API may change in any release
> before 1.0, and the crate is taken from its repository until then.

## Features

- **Failures that say something, with no document content.** A typed read names the refused key:
  unknown, missing and duplicate fields by name, a wrong type by the key's path, and what was
  expected. Values are never quoted, and neither is a parser's excerpt of the source.
- **Formats you register.** A `Codec<V>` or two closures gives a format a name, extensions and,
  if it has one, a marker such as `%YAML`. `V` is your value type; nothing goes through a tree of
  this crate's.
- **`.cfg` files that name their format.** `#!json`, `#!/usr/bin/env yaml` or a registered marker
  on the first line select the format; anything else is the default. Nothing is guessed from
  content, and a name no format answers to is an error listing the registered formats. One leading
  byte-order mark is ignored.
- **A search for the one file to read.** An explicit path, then a named environment variable's
  path, each an error if the file is not there; then the user's file and the system's. None found
  is `Ok(None)`. The working directory is never searched.
- **Layers.** A `Loader` reads bytes, text, streams and paths in order and hands them to your merge
  policy.
- **Limits you set.** Bytes per source, bytes in total and nesting each have a default and no floor.
- **Nothing runs.** No includes, interpolation, commands or URLs, and no process-wide state.

## Installation

```bash
cargo add anycfg --git https://github.com/jose-pr/anycfg-rs
```

The crate has no Cargo features. Rust 1.89 or later. It depends on `serde` and
`serde_path_to_error`; your application adds the parsers it registers.

## Quick start

Register a format, read a typed value, and see what a refusal says:

```rust
use anycfg::{Callbacks, CodecError, Formats, Options};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Server {
    host: String,
    port: u16,
}

let mut formats = Formats::<Value>::new();
formats.register(
    "json",
    ["json"],
    Callbacks::new(
        |bytes: &[u8]| {
            serde_json::from_slice(bytes).map_err(|error| {
                CodecError::new().line(error.line()).column(error.column())
            })
        },
        |value: &Value| serde_json::to_vec(value).map_err(|_| CodecError::new()),
    ),
)?;

let json = Options::new().name("app.json");
let server: Server = formats.from_str_typed(r#"{"host":"localhost","port":8080}"#, json)?;
assert_eq!(server.host, "localhost");
assert_eq!(server.port, 8080);

let refused = formats
    .from_str_typed::<Server>(r#"{"host":"localhost","port":"hunter2"}"#, json)
    .unwrap_err();
assert_eq!(
    refused.to_string(),
    "invalid configuration at `port`: expected u16, found a string"
);
# Ok::<(), anycfg::Error>(())
```

Find the file a program should read, then read it:

```rust,no_run
use anycfg::{Formats, Options, Search};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

#[derive(Deserialize)]
struct Settings {}

fn read(formats: &Formats<Value>, flag: Option<&Path>) -> Result<Option<Settings>, anycfg::Error> {
    let found = Search::new("app/app.cfg")
        .explicit(flag)
        .variable("APP_CONFIG")
        .find()?;
    found
        .map(|path| formats.load_typed(path, Options::new()))
        .transpose()
}
```

Layer several sources, in any mix of formats, through your merge policy:

```rust
use anycfg::{Callbacks, CodecError, Formats, Loader, Merge, ReplaceValues, Source};

let mut formats = Formats::<String>::new();
formats.register(
    "plain",
    ["txt"],
    Callbacks::new(
        |input: &[u8]| String::from_utf8(input.to_vec()).map_err(|_| CodecError::new()),
        |value: &String| Ok(value.as_bytes().to_vec()),
    ),
)?;

let text = Loader::new(&formats, "plain")
    .merge(Merge::Replace)
    .load([Source::text("old"), Source::text("new")], &ReplaceValues)?;
assert_eq!(text, "new");
# Ok::<(), anycfg::Error>(())
```

[`examples/three_formats.rs`](https://github.com/jose-pr/anycfg-rs/blob/main/examples/three_formats.rs)
registers TOML, JSON and YAML over one value type, with a failure function for each parser. Start
there when you register a parser: `toml`'s and `serde-saphyr`'s `Display` print the source line.

## API overview

Everything is at the crate root; there are no public modules.

| Item | Purpose |
| --- | --- |
| `Formats<V>` | The registry: register formats, select one, read and write |
| `Codec<V>`, `Callbacks` | A format's decode and encode, as a trait or as two closures |
| `CodecError` | A codec's failure: a position and a message with no document text |
| `Options`, `Detection` | Hints and limits for one read; what selection chose |
| `Loader`, `Source` | Ordered sources, limits, and the merged or typed result |
| `MergeValues`, `Merge`, `ReplaceValues` | Your merge policy, and the strategy it is given |
| `Search` | The one file a program reads |
| `user_config_path`, `system_config_path`, `default_configs` | The platform's configuration directories |
| `Error` | Every failure |

## Choosing a format

For one input, the first of these that applies:

1. A format the caller names (`Options::format`, `Source::format`).
2. An extension fixed to a format (`register`, `register_extension`). The first line is not read.
3. The first line, for an auto-detect extension (`.cfg` is one from the start; `register_auto`
   makes others) or an unnamed input. `#!` names the format by the line's last word, after any `/`
   or `\`, ignoring case and leading dots: `#!json`, `#!/usr/bin/env yaml`, `#!.TOML`. A format
   answers to its name and to each extension fixed to it, so a format registered with `.yml`
   answers to `#!yml`. The line is not part of what the codec is given, and a parse error's line
   number still counts it.
4. A registered marker, such as `%YAML`, which stays in what the codec is given.
5. The extension's own default (`register_auto`), then the loader's or `Options::default_format`,
   then `Formats::set_default`.

Nothing is guessed from the content. A first line no format answers to is
`Error::UnsupportedFormat`, which lists the registered formats and does not repeat the line. An
extension that is not registered is `Error::UnsupportedExtension`, which lists those registered.

## What a failure says

| Failure | `Display` |
| --- | --- |
| Unknown field | ``invalid configuration at `server.colour`: unknown field `colour`, expected one of `host`, `port` `` |
| Missing field | ``invalid configuration at `server`: missing field `port` `` |
| Duplicate field | ``invalid configuration at `server`: duplicate field `port` `` |
| Wrong type | ``invalid configuration at `server.port`: expected u16, found a string`` |
| Value out of range | ``invalid configuration at `server.port`: invalid value, expected u16, found an integer`` |
| Unknown enum variant | ``invalid configuration at `level`: unknown variant, expected one of `info`, `warn` `` |
| A type's own refusal (`custom`) | ``invalid configuration at `token`: refused by its type`` |
| Malformed document | ``could not parse toml configuration at line 3, column 12: string values must be quoted, expected literal string`` |
| In a file | ``/etc/app/app.cfg: invalid configuration at `server.port`: expected u16, found a string`` |
| Named file missing | ``no configuration file at /x/app.cfg, which APP_CONFIG names`` |
| First line names nothing | ``no configuration format answers to that name (registered: json, toml, yaml)`` |

A typed read does not show its value type's own errors, which quote the value, or any `custom`
message, which may hold anything. A key is the one document text an error repeats, because a
refused key must be named. A codec's message reaches the error as written, so a codec passes on
only what its parser is known not to quote from the document.

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --no-default-features -- -D warnings
cargo test --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features
cargo +nightly-2026-09-21 public-api --all-features -sss --color never | diff -u tools/public-api.txt -
cargo package --list --allow-dirty
```

`tools/public-api.txt` is the committed public API, regenerated in the commit that changes it.
[`AGENTS.md`](https://github.com/jose-pr/anycfg-rs/blob/main/AGENTS.md) is the API header that ships
in the package. CI runs on demand: by workflow dispatch, or on a throwaway `ci-*` tag.

### Releasing

A `v*` tag runs the tests and a strict docs build, then creates a source release from the tag's
`CHANGELOG.md` section. The manifest says `publish = false`, so nothing reaches crates.io.

## License

[MPL-2.0](https://github.com/jose-pr/anycfg-rs/blob/main/LICENSE).
