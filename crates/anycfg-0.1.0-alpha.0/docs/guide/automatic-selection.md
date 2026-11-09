# Automatic format selection

`anycfg` resolves a document format using caller-registered rules. It does not try multiple parsers to see which one succeeds.

## Establish a loader default

~~~rust
# use anycfg::{Callbacks, CodecError, Formats, Loader};
# fn main() -> Result<(), anycfg::Error> {
let mut formats = Formats::<String>::new();
formats.register("toml", ["toml"], Callbacks::new(
    |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| CodecError::default()),
    |value: &String| Ok(value.as_bytes().to_vec()),
))?;
let loader = Loader::new(&formats, "toml");
# let _ = loader;
# Ok(())
# }
~~~

The example uses a placeholder text codec under a registered name. Real applications register a TOML decoder for that name.

A loader **requires** its default as the second argument to `Loader::new`. This default is used for unnamed bytes/readers, and for auto-detect extensions without a pinned default, when no header or marker selects another codec.

## File extension policies

| Input | Registered policy | Selection |
| --- | --- | --- |
| `settings.toml` | Fixed extension | TOML codec; no inspection |
| `settings.cfg` | Automatic by default | Header or marker, else loader default |
| `settings.conf` | `register_auto(["conf"], "yaml")` | Header or marker, else YAML |
| Unnamed bytes or stream | No filename | Header or marker, else loader default |
| Unknown extension | No mapping | `UnsupportedExtension`, listing those registered |

`.cfg` starts registered in auto-detect mode; it is not itself a format or parser. You may change its behavior:

~~~rust
# use anycfg::{Callbacks, CodecError, Formats};
# fn main() -> Result<(), anycfg::Error> {
let mut formats = Formats::<String>::new();
for name in ["toml", "yaml", "json"] {
    formats.register(name, [name], Callbacks::new(
        |bytes: &[u8]| String::from_utf8(bytes.to_vec())
            .map_err(|_| CodecError::default()),
        |value: &String| Ok(value.as_bytes().to_vec()),
    ))?;
}

formats.register_auto(["cfg"], "yaml")?; // Auto-detect, otherwise YAML
formats.register_extension("cfg", "json")?; // Instead make .cfg fixed JSON
# Ok(())
# }
~~~

For other extensions, `register_auto` installs an automatic policy with a pinned fallback. Fixed mappings from `Formats::register` or `register_extension` do not inspect content.

## Selection precedence

1. Explicit `Source::format("json")` or `Options::format("json")`.
2. Fixed extension, if one is registered.
3. For auto-detect extensions or unnamed data, a first line starting `#!`. The format is the line's last word, after any `/` or `\`, ignoring case and leading dots: `#!json`, `#!/usr/bin/env yaml`, `#!.TOML`.
4. A registered native byte prefix, via `Formats::register_marker`.
5. A pinned auto-detect extension default (if configured).
6. A loader or per-read default, then the registry fallback for direct reads.

A format line is removed before the codec receives the remaining bytes, and a parse error's line number still counts it. A line no format answers to is an error listing the registered formats (`UnsupportedFormat`); it never falls through to the default, and the error does not repeat the line. Registered native prefixes stay in the payload for the selected codec to interpret. With a forced format or fixed extension, headers remain untouched. One leading UTF-8 byte-order mark is ignored in every case.

Nothing is guessed from content: a `.cfg` file starting `{` under a TOML default is TOML, and fails as TOML.

## Direct bytes and readers

A direct registry read has no loader, so supply the default in `Options`:

~~~rust
# use anycfg::{Callbacks, CodecError, Formats, Options};
# fn main() -> Result<(), anycfg::Error> {
let mut formats = Formats::<String>::new();
formats.register("plain", ["txt"], Callbacks::new(
    |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| CodecError::default()),
    |value: &String| Ok(value.as_bytes().to_vec()),
))?;

let value = formats.from_reader(
    std::io::Cursor::new(b"hello"),
    Options::new().default_format("plain"),
)?;
assert_eq!(value, "hello");
# Ok(())
# }
~~~

## Self-describing output

`Formats::to_auto_vec(&value, "yaml", "cfg")` writes bytes for a registered auto-detect extension. An unpinned extension always adds a `#!yaml` header to make the output independent of loader defaults; a pinned extension omits the header only if the selected format equals its fallback. Typed values can use `to_auto_vec_typed`.

See [Format registration](formats.md) and [Sources](sources.md).
