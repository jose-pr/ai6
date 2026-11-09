# Register formats

A `Formats<V>` registry associates names and filename extensions with `Codec<V>` implementations. The type parameter `V` is an **application-owned representation**, not a library-defined tree.

## Callback-based formats

~~~rust
use anycfg::{Callbacks, CodecError, Formats};

# fn main() -> Result<(), anycfg::Error> {
let mut formats = Formats::<String>::new();
formats.register("plain", ["txt", "text"], Callbacks::new(
    |bytes: &[u8]| String::from_utf8(bytes.to_vec())
        .map_err(|_| CodecError::default()),
    |value: &String| Ok(value.as_bytes().to_vec()),
))?;
# Ok(())
# }
~~~

The decode callback receives the document's bytes after any selected format directive has been removed. The encode callback returns a complete byte representation of the value.

## Custom codecs

For nontrivial implementations, implement the trait:

~~~rust
use anycfg::{Codec, CodecError};

struct Plain;

impl Codec<String> for Plain {
    fn decode(&self, input: &[u8]) -> Result<String, CodecError> {
        String::from_utf8(input.to_vec()).map_err(|_| CodecError::default())
    }

    fn encode(&self, value: &String) -> Result<Vec<u8>, CodecError> {
        Ok(value.as_bytes().to_vec())
    }
}
~~~

A `CodecError` holds an optional line, column and message. The message reaches `Error::Parse` as written, so it must hold no text from the document: pass on a parser's message only when that parser is known not to quote. `toml`'s and `serde-saphyr`'s `Display` print the source line, which is what must not reach an error. The [repository example](https://github.com/jose-pr/anycfg-rs/blob/main/examples/three_formats.rs) has a failure function for each of `toml`, `serde_json` and `serde-saphyr`; `CodecError::at_offset` turns a byte offset into a line and column.

## Format registration versus extension registration

`Formats::register(name, extensions, codec)` registers both a named codec and fixed extensions. Other extension mappings can be added explicitly with `register_extension`. Use `register_auto` when an extension should recognize content headers and markers first.

~~~rust
# use anycfg::{Callbacks, CodecError, Formats};
# fn main() -> Result<(), anycfg::Error> {
# let mut formats = Formats::<String>::new();
# formats.register("plain", ["txt"], Callbacks::new(
#     |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| CodecError::default()),
#     |value: &String| Ok(value.as_bytes().to_vec()),
# ))?;
formats.register_extension("config", "plain")?;
formats.register_auto(["settings"], "plain")?;
# Ok(())
# }
~~~

A fixed extension never interprets a format header. An automatic extension checks registered headers and markers, then uses its configured fallback. A format answers to its name and to each extension fixed to it, so a format registered with `.yml` also answers to `#!yml`.

The initially registered `.cfg` extension is special *only in the registry setup*: it starts in automatic mode, with no pinned fallback. It inherits the loader's explicit default unless an application overrides that extension.

## Choosing the value representation

A codec may return a Serde-compatible dynamic tree, a domain-specific structure, or an opaque value. If several registered codecs feed the same loader, their outputs must all use the registry's `V`.

For examples of TOML, YAML and JSON codecs, see [Serde and typed configuration](serde.md) and the [repository example](https://github.com/jose-pr/anycfg-rs/blob/main/examples/three_formats.rs).

For precedence and output headers, see [Automatic format selection](automatic-selection.md).
