# `anycfg` — API header

Header-file-style reference for the `anycfg` crate: every public item with its signature,
contract and gotchas, so the crate can be used without reading its source. It ships inside the
package and is self-contained. Development documentation lives with the source at
<https://github.com/jose-pr/anycfg-rs>.

The crate has no Cargo features. MSRV Rust 1.89. A default build depends on `serde` (`std`) and
`serde_path_to_error`. Version `0.1.0-alpha.0` is a pre-release and is not published; the API may
change in any release before 1.0.

Imports: everything is at the crate root; there are no public modules.

No `unsafe` code (`#![forbid(unsafe_code)]`). No process-wide state, logger or runtime. No built-in
format, includes, interpolation, command execution or URLs. Only `Search::find`, `user_config_path`
and `system_config_path` read the environment.

## Formats (`anycfg::{Formats, Codec, Callbacks, CodecError}`)

```rust
pub trait Codec<V> {
    fn decode(&self, input: &[u8]) -> Result<V, CodecError>;    // no BOM, no format line
    fn encode(&self, value: &V) -> Result<Vec<u8>, CodecError>;
}
pub struct Callbacks<V, D, E>;  // Codec when D: Fn(&[u8]) -> Result<V, CodecError>,
                                //            E: Fn(&V) -> Result<Vec<u8>, CodecError>
impl<V, D, E> Callbacks<V, D, E> { pub fn new(decode: D, encode: E) -> Self; }

#[non_exhaustive]
pub struct CodecError {          // Clone, Default, Eq, Display, Error
    pub line: Option<usize>,     // one-based
    pub column: Option<usize>,   // one-based
    pub message: Option<String>, // holds no text from the document
}
impl CodecError {
    pub fn new() -> Self;
    pub fn at_offset(input: &[u8], offset: usize) -> Self; // line and column of a byte offset
    pub fn line(self, line: usize) -> Self;
    pub fn column(self, column: usize) -> Self;
    pub fn message(self, message: impl Into<String>) -> Self;
}
```

`CodecError::message` is copied into `Error::Parse`, `Error::Serialize` and, from `Loader::load_as`,
`Error::Conversion`, unchecked. **The codec is responsible for it holding no document text**: pass on
a parser's message only when that parser is known not to quote. What was measured with `toml` 0.9,
`serde_json` 1 and `serde-saphyr` 2, each deserializing into a value type:

- `toml`: `error.message()` and `error.span()` hold no document text; `Display` prints the line.
- `serde_json`: `Display` is a fixed message and the position, when the target accepts every value.
- `serde-saphyr`: `Display` prints the source lines, and several variants' text quotes a scalar. Use
  `error.location()`, and of `error.without_snippet()` only the text of named variants;
  `ExternalMessage`'s `msg` is the parser's fixed strings.

`examples/three_formats.rs` in the repository has a failure function for each.

```rust
pub struct Formats<V>;  // Send + Sync, Default, Debug
impl<V> Formats<V> {
    pub fn new() -> Self;

    pub fn register<C, I, S>(&mut self, name: &str, extensions: I, codec: C) -> Result<(), Error>
      where C: Codec<V> + Send + Sync + 'static, I: IntoIterator<Item = S>, S: AsRef<str>;
    pub fn register_extension(&mut self, extension: &str, format: &str) -> Result<(), Error>;
    pub fn register_auto<I, S>(&mut self, extensions: I, default_format: &str) -> Result<(), Error>
      where I: IntoIterator<Item = S>, S: AsRef<str>;
    pub fn register_marker(&mut self, name: &str, marker: &[u8]) -> Result<(), Error>;
    pub fn set_default(&mut self, name: &str) -> Result<(), Error>;

    pub fn detect<'a>(&'a self, input: &'a [u8], options: Options<'_>) -> Result<Detection<'a>, Error>;

    pub fn from_slice(&self, input: &[u8], options: Options<'_>) -> Result<V, Error>;
    pub fn from_str(&self, input: &str, options: Options<'_>) -> Result<V, Error>;
    pub fn from_reader(&self, input: impl Read, options: Options<'_>) -> Result<V, Error>;
    pub fn load(&self, path: impl AsRef<Path>, options: Options<'_>) -> Result<V, Error>;

    // Each with V: for<'de> Deserializer<'de>, T: DeserializeOwned.
    pub fn from_slice_typed<T>(&self, input: &[u8], options: Options<'_>) -> Result<T, Error>;
    pub fn from_str_typed<T>(&self, input: &str, options: Options<'_>) -> Result<T, Error>;
    pub fn from_reader_typed<T>(&self, input: impl Read, options: Options<'_>) -> Result<T, Error>;
    pub fn load_typed<T>(&self, path: impl AsRef<Path>, options: Options<'_>) -> Result<T, Error>;

    pub fn to_vec(&self, value: &V, name: &str) -> Result<Vec<u8>, Error>;
    pub fn to_string(&self, value: &V, name: &str) -> Result<String, Error>;
    pub fn to_writer(&self, writer: impl Write, value: &V, name: &str) -> Result<(), Error>;
    pub fn to_vec_typed<T, S>(&self, value: &T, name: &str, serializer: S) -> Result<Vec<u8>, Error>
      where T: Serialize + ?Sized, S: Serializer<Ok = V>;
    pub fn to_auto_vec(&self, value: &V, name: &str, extension: &str) -> Result<Vec<u8>, Error>;
    pub fn to_auto_vec_typed<T, S>(&self, value: &T, name: &str, extension: &str, serializer: S)
        -> Result<Vec<u8>, Error>
      where T: Serialize + ?Sized, S: Serializer<Ok = V>;
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    pub format: Option<&'a str>,         // force a format; the first line is then not read
    pub name: Option<&'a str>,           // a file name, used only for its extension
    pub default_format: Option<&'a str>,
    pub max_bytes: usize,                // default DEFAULT_MAX_SOURCE_BYTES, 8 MiB; no floor
    pub max_depth: usize,                // typed reads; default DEFAULT_MAX_DEPTH, 64; no floor
}
impl<'a> Options<'a> {
    pub fn new() -> Self;
    pub fn name(self, name: &'a str) -> Self;
    pub fn format(self, format: &'a str) -> Self;
    pub fn default_format(self, format: &'a str) -> Self;
    pub fn max_bytes(self, max_bytes: usize) -> Self;
    pub fn max_depth(self, max_depth: usize) -> Self;
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub struct Detection<'a> {
    pub format: &'a str,        // the format's registered name
    pub payload: &'a [u8],      // what the codec is given
    pub lines_removed: usize,   // 1 after a format line, else 0
}

pub const DEFAULT_MAX_SOURCE_BYTES: usize; // 8 MiB
pub const DEFAULT_MAX_DEPTH: usize;        // 64
```

**Registering.** `register` fixes its extensions to the format. A name or extension is ASCII
letters, digits, `-` or `_`, compared in lower case; an extension may be written with its leading
dot. A second registration of a name or extension is `InvalidRegistration`, except `cfg`, which may
be overridden. A failed registration changes nothing. `register_marker` refuses an empty or
duplicate marker; the longest matching marker wins.

**A format answers to** its name and to each extension fixed to it, ignoring case and leading dots.
That holds wherever a format is named: a first line, `Options::format`, `default_format`,
`register_extension`, `to_vec`.

**Selection** (`detect`), after one leading UTF-8 byte-order mark is dropped; nothing is guessed from
content:

1. `Options::format`. The first line is not read, so a `#!` line reaches the codec.
2. The extension of `Options::name`, when fixed to a format. The first line is not read.
3. An unregistered extension is `UnsupportedExtension`. A name with no extension (`app`, `.app`,
   `app.`) counts as unnamed.
4. For `.cfg` (auto-detect from the start), an extension `register_auto` made auto-detect, or an
   unnamed input: a first line starting `#!` names the format by its last word, after any `/` or `\`
   (`#!json`, `#!/usr/bin/env yaml`, `#!.TOML`), and is removed. A line no format answers to, with no
   word, or not UTF-8, is `UnsupportedFormat`.
5. A registered marker. It stays in the payload.
6. The extension's `register_auto` default, then `Options::default_format`, then `set_default`, else
   `NoDefaultFormat`.

A parse error's `line` counts from the document's first line, the removed one included.

**Typed reads** read `V` through `deserialize_any`, into a private copy, then deserialize `T` from
that. So `V` must be self-describing, and `T`'s `Deserialize` sees the document's order, integers as
`i64`/`u64`, an enum as a string or a map of one key. `V`'s own error is discarded. A depth beyond
`max_depth` is `TooDeep`: the root value is at depth 0, and a value's children are one deeper.

`load` and `load_typed` read through `from_reader`, so `max_bytes` bounds them, and use the path as
`Options::name` unless one is given. `from_reader` reads at most one byte past the limit.

`to_auto_vec` writes for an auto-detect extension: a `#!name` line, unless the format is that
extension's `register_auto` default. A fixed or unknown extension is `UnsupportedExtension`.
`to_vec_typed` converts through `serializer` first; its error is discarded (`Conversion`).
`to_writer` encodes completely before it writes.

## Layers (`anycfg::{Loader, Source, MergeValues, Merge, ReplaceValues}`)

```rust
pub struct Source<'a>;
impl<'a> Source<'a> {
    pub fn bytes(bytes: &'a [u8]) -> Self;
    pub fn text(text: &'a str) -> Self;
    pub fn reader(reader: impl Read + 'a) -> Self;
    pub fn path(path: &'a Path) -> Self;       // opened while loading
    pub fn name(self, name: &'a str) -> Self;  // used only for its extension
    pub fn format(self, format: &'a str) -> Self;
}

pub struct Loader<'f, V>;
impl<'f, V> Loader<'f, V> {
    pub fn new(formats: &'f Formats<V>, default_format: impl Into<String>) -> Self;
    pub fn merge(self, strategy: Merge) -> Self;                 // default Merge::Deep
    pub fn default_format(self, format: impl Into<String>) -> Self;
    pub fn max_source_bytes(self, max_bytes: usize) -> Self;     // default 8 MiB; no floor
    pub fn max_total_bytes(self, max_bytes: usize) -> Self;      // default 64 MiB; no floor
    pub fn max_depth(self, max_depth: usize) -> Self;            // default 64; no floor

    // Each with I: IntoIterator<Item = S>, S: Into<Option<Source<'a>>>, M: MergeValues<V>.
    pub fn load<'a, I, S, M>(&self, sources: I, adapter: &M) -> Result<V, Error>;
    pub fn load_or<'a, I, S, M>(&self, sources: I, adapter: &M, fallback: V) -> Result<V, Error>;
    pub fn load_as<'a, I, S, M, T, F>(&self, sources: I, adapter: &M, convert: F) -> Result<T, Error>
      where F: FnOnce(V) -> Result<T, CodecError>;
    pub fn load_typed<'a, I, S, M, T>(&self, sources: I, adapter: &M) -> Result<T, Error>
      where V: for<'de> Deserializer<'de>, T: DeserializeOwned;
}

#[non_exhaustive] pub enum Merge { Replace, Shallow, #[default] Deep }
pub trait MergeValues<V> {
    fn validate(&self, _value: &V, _max_depth: usize) -> Result<(), Error> { Ok(()) } // each document
    fn merge(&self, previous: &mut V, next: V, strategy: Merge, max_depth: usize) -> Result<(), Error>;
}
pub struct ReplaceValues;                 // MergeValues<V>; MergeShape unless Merge::Replace
impl<V, F> MergeValues<V> for F           // a closure, with validate the default no-op
  where F: Fn(&mut V, V, Merge, usize) -> Result<(), Error>;

pub const DEFAULT_MAX_TOTAL_BYTES: usize; // 64 MiB
```

Sources are read in order, and the later ones merged into the earlier by the adapter; `merge` is not
called for a single source. `None` is skipped. All absent is `NoSources`, or `fallback` from `load_or`.
The strategy is only a name given to the adapter. The crate walks no `V`, so a recursive value model
enforces `max_depth` in `validate`; a typed load also enforces it on the merged value.

## Finding the file (`anycfg::{Search, user_config_path, system_config_path, default_configs}`)

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Search;
impl Search {
    pub fn new(name: impl Into<PathBuf>) -> Self;             // beneath the two directories
    pub fn explicit(self, path: Option<&Path>) -> Self;       // a `--config` flag's value
    pub fn variable(self, name: impl Into<String>) -> Self;   // the caller names it
    pub fn find(&self) -> Result<Option<PathBuf>, Error>;
}
pub fn user_config_path(name: Option<&Path>) -> Result<PathBuf, Error>;
pub fn system_config_path(name: Option<&Path>) -> PathBuf;
pub fn default_configs(name: impl AsRef<Path>) -> Result<[Option<PathBuf>; 2], Error>;
```

`Search::find` returns the first that exists, and opens nothing:

1. the explicit path; not a file is `NotFound` naming it;
2. the variable's path, when set and not empty; not a file is `NotFound` naming it and the variable;
3. `name` in `user_config_path`; a user with no known home directory skips it;
4. `name` in `system_config_path`;
5. otherwise `Ok(None)`, which is not an error.

The working directory is never searched; a relative explicit or variable path is taken against it
because a person chose it. `name` must be relative, not empty and hold only normal components, or
`find` is `InvalidConfigName`.

`user_config_path` is `%APPDATA%` (else `~/AppData/Roaming`) on Windows, an absolute
`$XDG_CONFIG_HOME` (else `~/.config`) elsewhere, macOS included; `HomeUnavailable` when it needs a
home directory and there is none. `system_config_path` is `%PROGRAMDATA%` (else `C:\ProgramData`) on
Windows, `/etc` elsewhere. `default_configs` returns the system file then the user's, for layering,
with `None` for one that is absent; it refuses a name as `Search::find` does.

## Dialect

The format's own rules are its codec's: what a duplicate key does, how a number that does not fit is
kept, whether `NaN` and infinities are accepted, how deep a document may nest while parsing, and
whether expansions such as YAML aliases are bounded. This crate adds: one leading byte-order mark is
ignored, a typed read keeps the value's order, and a typed read refuses nesting beyond `max_depth`.
`serde_json::Value` resolves a repeated key to the last one, so a codec over it refuses repeats itself
if it should (the repository's example does).

## Errors

```rust
#[non_exhaustive]
pub enum Error {
    NoDefaultFormat,
    UnsupportedFormat { registered: Vec<String> },   // the name is not repeated
    UnsupportedExtension { extension: String, registered: Vec<String> },
    InvalidRegistration,
    InvalidUtf8 { offset: usize },                   // to_string: a codec's output
    TooLarge { limit: usize },
    TooDeep { limit: usize },
    NoSources,
    MergeShape,
    Conversion { path: Option<String>, message: Option<String> },
    Parse { format: String, line: Option<usize>, column: Option<usize>, message: Option<String> },
    Serialize { format: String, message: Option<String> },
    Source { index: usize, cause: Box<Error> },
    File { path: PathBuf, cause: Box<Error> },
    NotFound { path: PathBuf, variable: Option<String> },
    HomeUnavailable,
    InvalidConfigName,
    Io(std::io::Error),
}
```

`Display` is one lowercase clause. `source()` gives the cause of `Source`, `File` and `Io`.

A message, field, `Debug` and source chain quote no value. For a typed read, `Conversion.path` is where
the key is (`server.port`, `items[1]`), `None` at the root, and `message` is one of:

```text
unknown field `colour`, expected one of `host`, `port`    (path includes the unknown key)
missing field `port`                                      (path is the map it belongs in)
duplicate field `port`
expected u16, found a string
invalid value, expected u16, found an integer
invalid length 3, expected a tuple of size 2
unknown variant, expected one of `info`, `warn`
refused by its type                                       (a `custom` message, dropped)
```

## Environment variables

Only for finding files; the library holds no setting of its own.

| Variable | Read by | Effect |
| --- | --- | --- |
| The one named to `Search::variable` | `Search::find` | The path of the file; empty counts as unset |
| `APPDATA` (Windows), `XDG_CONFIG_HOME` (elsewhere; absolute only) | `user_config_path` | The user's configuration directory |
| `PROGRAMDATA` (Windows) | `system_config_path` | The system's configuration directory |

The home directory comes from `std::env::home_dir`.

## Gotchas

- `T`'s field names, `expecting` text and a type's `Expected` description appear in messages as
  written. A `Visitor` whose `expecting` prints data it read would pass that on; a derive never does.
- A key is the one document text an error repeats, because a refused key must be named. Control
  characters in it are escaped, and its length is bounded only by the byte limit.
- A `#[serde(try_from)]` or validating type's message is dropped, as `refused by its type` with the
  path. To say more, validate after the read, where a message can be written knowing what it holds.
- `Display` of a wrapper (`Source`, `File`) includes its cause, and `source()` also returns it: a
  printer that walks the chain shows the cause twice.
- `Options::format` and fixed extensions do not read the first line, so a `.json` file starting `#!`
  fails as JSON.
- YAML 1.1 readers take `n`, `y`, `on` and `off` as booleans; `serde-saphyr` does. That reads as a wrong
  type in a string field.
- `#[serde(flatten)]` and `deny_unknown_fields` do not combine in serde itself.
