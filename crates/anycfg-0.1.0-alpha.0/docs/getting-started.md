# Getting started

The crate is named `anycfg` and requires Rust **1.89+** (edition 2024). It provides orchestration and Serde support, but does not select a concrete parser or data model.

## Add dependencies

~~~toml
[dependencies]
anycfg = { git = "https://github.com/jose-pr/anycfg-rs" }
serde = { version = "1", features = ["derive"] }
~~~

To read JSON, TOML or YAML, add the corresponding format crates to your application's manifest. The library needs only `serde` and `serde_path_to_error`.

## Register a codec

A codec maps a document to and from a type chosen by your application. Here, the value type is `String`:

~~~rust
use anycfg::{Callbacks, CodecError, Formats};

# fn main() -> Result<(), anycfg::Error> {
let mut formats = Formats::<String>::new();

formats.register("text", ["txt"], Callbacks::new(
    |bytes: &[u8]| String::from_utf8(bytes.to_vec())
        .map_err(|_| CodecError::default()),
    |value: &String| Ok(value.as_bytes().to_vec()),
))?;
# Ok(())
# }
~~~

You may implement `Codec<V>` directly instead of providing closures. The registry does not implicitly enable any document parser.

## Load a source

A loader always receives a default format in its constructor:

~~~rust
# use anycfg::{Callbacks, CodecError, Formats, Loader, Merge, ReplaceValues, Source};
# fn main() -> Result<(), anycfg::Error> {
# let mut formats = Formats::<String>::new();
# formats.register("text", ["txt"], Callbacks::new(
#     |input: &[u8]| String::from_utf8(input.to_vec()).map_err(|_| CodecError::default()),
#     |value: &String| Ok(value.as_bytes().to_vec()),
# ))?;
let loader = Loader::new(&formats, "text").merge(Merge::Replace);
let value = loader.load([Source::bytes(b"hello")], &ReplaceValues)?;
assert_eq!(value, "hello");
# Ok(())
# }
~~~

The string `"text"` must identify a registered codec. It is used for unnamed bytes and streams when format detection cannot choose something more specific.

## Layer multiple sources

Sources are processed in the order supplied, with later sources taking precedence according to the merge adapter:

~~~rust
# use anycfg::{Callbacks, CodecError, Formats, Loader, Merge, ReplaceValues, Source};
# fn main() -> Result<(), anycfg::Error> {
# let mut formats = Formats::<String>::new();
# formats.register("text", ["txt"], Callbacks::new(
#     |input: &[u8]| String::from_utf8(input.to_vec()).map_err(|_| CodecError::default()),
#     |value: &String| Ok(value.as_bytes().to_vec()),
# ))?;
let value = Loader::new(&formats, "text")
    .merge(Merge::Replace)
    .load([
        Source::text("base"),
        Source::text("personal"),
    ], &ReplaceValues)?;
assert_eq!(value, "personal");
# Ok(())
# }
~~~

For mapping-aware `Merge::Deep` or `Merge::Shallow`, implement a merge adapter. The library does not assume every value is a JSON-like object.

## Next steps

Read [Register formats](guide/formats.md) to plug in parsers, [Sources and streams](guide/sources.md) for file/byte inputs, or [Serde integration](guide/serde.md) for typed Rust structs.
