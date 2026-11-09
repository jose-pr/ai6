# anycfg

**A format-neutral configuration library for Rust, with Serde, pluggable formats and ordered source layering.**

`anycfg` manages *where* configuration comes from, *which registered codec* reads it, *how sources are combined*, and *what a failure says*: it names the key and never quotes a value. Your application supplies the document representation and the format parsers. It depends on `serde` and `serde_path_to_error`.

## Why anycfg?

- **Bring your own formats.** Register codecs or callback functions. JSON, TOML and YAML are examples, not built-in dependencies.
- **Failures that say something.** A typed read names an unknown, missing or duplicate field and the path of a wrong type. Values and parser excerpts stay out of every message.
- **Automatic format selection.** Use registered file extensions and content markers. A `.cfg` file can name its format on its first line (`#!json`, `#!/usr/bin/env yaml`); otherwise it uses your default. Nothing is guessed from content.
- **Explicit defaults.** `Loader::new(&formats, "toml")` sets the fallback for unnamed byte streams and unpinned automatic extensions.
- **Layer configuration.** Read bytes, text, streams and paths in order; a caller-owned `MergeValues<V>` policy decides what replacement, shallow and deep merging mean.
- **Serde types.** Convert a parsed or merged value into a Serde-derived Rust struct, and serialize typed values through consumer-selected serializers.
- **Standard locations.** `Search` finds the one file to read: an explicit path, a named variable's path, the user's file, the system's. The working directory is never searched.

## Install

The crate is not published on crates.io yet. Reference the repository directly:

~~~toml
[dependencies]
anycfg = { git = "https://github.com/jose-pr/anycfg-rs" }
serde = { version = "1", features = ["derive"] }

# Only if your application chooses these parsers:
serde_json = "1"
toml = "0.9"
serde-saphyr = "2"
~~~

## A minimal loader

~~~rust
use anycfg::{Callbacks, CodecError, Formats, Loader, Merge, ReplaceValues, Source};

# fn main() -> Result<(), anycfg::Error> {
let mut formats = Formats::<String>::new();
formats.register("text", ["txt"], Callbacks::new(
    |input: &[u8]| String::from_utf8(input.to_vec())
        .map_err(|_| CodecError::default()),
    |value: &String| Ok(value.as_bytes().to_vec()),
))?;

let value = Loader::new(&formats, "text")
    .merge(Merge::Replace)
    .load([Source::text("system"), Source::text("user")], &ReplaceValues)?;

assert_eq!(value, "user");
# Ok(())
# }
~~~

This example deliberately uses a text codec. Structured formats use the same registration interface, with their own value type and merge adapter.

## Continue reading

- [Getting started](getting-started.md) — install the crate, register a codec and load a value
- [Automatic format selection](guide/automatic-selection.md) — defaults, headers and extension policies
- [Serde and typed configuration](guide/serde.md) — TOML, YAML and JSON using consumer dependencies
- [Merging layers](guide/merging.md) — replace, shallow and deep strategies
- [API reference](api/reference.md) — public types and generated rustdoc

The [Python anycfg project](https://github.com/jose-pr/anycfg) is a separate implementation with a different API.
