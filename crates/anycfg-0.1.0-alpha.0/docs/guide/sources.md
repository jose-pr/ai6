# Sources and streams

Every load accepts ordered `Source` values. A source can borrow bytes or text, consume a reader, or open a local path at load time.

## Supported inputs

| Constructor | Input | Behavior |
| --- | --- | --- |
| `Source::bytes(&[u8])` | Borrowed bytes | No copy or I/O before parsing |
| `Source::text(&str)` | Borrowed UTF-8 text | Delegates to bytes |
| `Source::reader(Read)` | Any synchronous Rust reader | Reads with byte budget; consumes reader |
| `Source::path(&Path)` | Filesystem path | Opens at load time; filename provides extension hint |

To read one file directly, without a loader, use `Formats::load` or `Formats::load_typed`. A failure from any of these names the file (`Error::File`).

## Format hints

A name is only a hint, not a file to open:

~~~rust
use anycfg::Source;

let source = Source::bytes(b"name = 'demo'")
    .name("settings.toml");
let forced = Source::bytes(b"hello").format("plain");
# let _ = (source, forced);
~~~

`Source::format` takes precedence over the source's name, registered markers and the loader default. Because it forces a format, the loader does not remove format directives.

Unnamed readers and byte slices inherit the default from `Loader::new(&formats, "name")` unless a registered format header or native marker selects another format.

## Several sources

~~~rust
# use anycfg::{Callbacks, CodecError, Formats, Loader, Merge, ReplaceValues, Source};
# fn main() -> Result<(), anycfg::Error> {
# let mut formats = Formats::<String>::new();
# formats.register("text", ["txt"], Callbacks::new(
#     |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| CodecError::default()),
#     |value: &String| Ok(value.as_bytes().to_vec()),
# ))?;
let sources = [
    Source::text("base"),
    Source::reader(std::io::Cursor::new(b"user")),
];
let result = Loader::new(&formats, "text")
    .merge(Merge::Replace)
    .load(sources, &ReplaceValues)?;
assert_eq!(result, "user");
# Ok(())
# }
~~~

Each source is parsed separately; the merge adapter decides how later values change earlier ones.

## Optional sources

`Loader::load` accepts optional `Source` entries and skips `None`:

~~~rust
# use anycfg::{Callbacks, CodecError, Formats, Loader, Merge, ReplaceValues, Source};
# fn main() -> Result<(), anycfg::Error> {
# let mut formats = Formats::<String>::new();
# formats.register("text", ["txt"], Callbacks::new(
#     |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| CodecError::default()),
#     |value: &String| Ok(value.as_bytes().to_vec()),
# ))?;
let layers = [
    None,
    Some(Source::text("present")),
];
let value = Loader::new(&formats, "text")
    .merge(Merge::Replace)
    .load(layers, &ReplaceValues)?;
assert_eq!(value, "present");
# Ok(())
# }
~~~

If all entries are absent, `load` returns `Error::NoSources`. Use `load_or(sources, &adapter, fallback_value)` to supply a fallback value instead.

## Resource budgets

The loader limits each source to **8 MiB** and all sources to **64 MiB** by default:

~~~rust
# use anycfg::{Callbacks, CodecError, Formats, Loader};
# fn main() -> Result<(), anycfg::Error> {
# let mut formats = Formats::<String>::new();
# formats.register("text", ["txt"], Callbacks::new(
#     |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| CodecError::default()),
#     |value: &String| Ok(value.as_bytes().to_vec()),
# ))?;
let loader = Loader::new(&formats, "text")
    .max_source_bytes(1024 * 1024)
    .max_total_bytes(4 * 1024 * 1024);
# let _ = loader;
# Ok(())
# }
~~~

Each is a setting with a default and no floor. A bounded reader may consume one extra byte to detect overflow. Parser recursion, expansion and custom source access remain the responsibility of the chosen codec.

See [Configuration locations](paths.md) and [Merging layers](merging.md).
