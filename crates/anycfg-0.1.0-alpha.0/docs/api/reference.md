# API reference

The full Rust API reference is generated from the crate's source documentation using `cargo doc` and deployed with this site.

**[Open rustdoc: anycfg API](https://jose-pr.github.io/anycfg-rs/rust/anycfg/index.html)**

For a compact, checked-in contract covering public names, defaults and caveats, see [AGENTS.md](https://github.com/jose-pr/anycfg-rs/blob/main/AGENTS.md).

## Format registration

| Public API | Purpose |
| --- | --- |
| `Formats<V>` | Named codec registry with fixed and auto-detect extension rules |
| `Codec<V>` | Format-specific `decode` and `encode` implementations |
| `Callbacks<V, D, E>` | Wrap decoder and encoder functions or closures |
| `CodecError` | A codec's failure: a position and a message with no document text |
| `Options<'a>` | Explicit format, logical name, default format, byte budget and depth |
| `Detection<'a>` | Selected format and bytes to pass to the codec |
| `Formats::register` | Register a codec and its fixed extensions |
| `Formats::register_auto` | Register an auto-detect extension with a default |
| `Formats::register_extension` | Assign a fixed codec to an extension |
| `Formats::register_marker` | Map a native byte prefix to a codec |
| `Formats::set_default` | Registry fallback for direct reads |
| `Formats::detect` | Detect without invoking a parser |

## Reading and writing

| Public API | Purpose |
| --- | --- |
| `Formats::from_slice` / `from_str` / `from_reader` | Decode documents into `V` |
| `Formats::load` | Read a file and decode it into `V` |
| `Formats::from_slice_typed` / `from_str_typed` / `from_reader_typed` / `load_typed` | Decode `V` into `T: DeserializeOwned`, naming a refused key |
| `Formats::to_vec` / `to_string` / `to_writer` | Serialize a `V` via a registered codec |
| `Formats::to_vec_typed` | Serialize a typed `T` through a caller-supplied Serde serializer |
| `Formats::to_auto_vec` / `to_auto_vec_typed` | Write a self-describing document for an automatic extension |

## Sources, loader and merging

| Public API | Purpose |
| --- | --- |
| `Source::bytes` / `text` / `reader` / `path` | Construct a source |
| `Source::name` / `format` | Add a hint or an explicit format |
| `Loader::new(&formats, "name")` | Construct with required default format |
| `Loader::default_format` | Replace the loader's default |
| `Loader::max_source_bytes` / `max_total_bytes` / `max_depth` | Set resource budgets |
| `Loader::merge` | Select `Merge::Replace`, `Shallow` or `Deep` |
| `Loader::load` / `load_or` | Load and merge, or supply an absent-source fallback |
| `Loader::load_typed` / `load_as` | Convert a merged value into a target type |
| `MergeValues<V>` | Caller-owned merging and structure validation |
| `ReplaceValues` | Built-in whole-value replacement adapter |

## Platform locations

| Public API | Purpose |
| --- | --- |
| `user_config_path` | Resolve a standard user location |
| `system_config_path` | Resolve the machine-wide location |
| `default_configs` | Existing system and user files, in override order |
| `Search` | The one file to read: explicit path, named variable, user's, system's |

## Error model

`Error` is a non-exhaustive enum covering input selection, invalid registration, I/O, byte budgets, parser failures, conversion failures, merge rejection, and unavailable/missing configuration paths. No message quotes a value from a document; `Error::Parse` and `Error::Conversion` carry a message saying what was wrong. For exact variants, see the generated [`anycfg::Error`](https://jose-pr.github.io/anycfg-rs/rust/anycfg/enum.Error.html) reference.

## Development reference build

~~~bash
cargo doc --no-deps --all-features
~~~

The generated pages will be in `target/doc/anycfg/`. The GitHub Pages workflow copies the Rust reference under `/rust/` alongside the MkDocs guides.
