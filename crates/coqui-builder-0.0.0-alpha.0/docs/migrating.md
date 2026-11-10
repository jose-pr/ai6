# Migrating to coqui-builder

How to move a settings type, a hand-written builder, a hand-kept "non-default" report and a
timeout that waits forever from their usual forms onto `#[derive(Describe)]`,
`#[derive(Builder)]` and `Configured`. Nothing here is needed to start fresh: it is for a crate
that already has such types.

Every step keeps a type's keys, defaults and written forms unless a section says otherwise.

## Dependency

```toml
[dependencies]
coqui-builder = { git = "https://github.com/Boriken-Dev/coqui-builder", rev = "<full commit>", features = ["derive"] }
```

Add the other features only where they are used:

| Feature | When |
| --- | --- |
| `std` (default) | `PathBuf`, `HashMap`, the `Env` source |
| `zeroize` | a `Secret<T>` field, and wiping the texts of sensitive places |
| `serde` | reading a `Partial` from TOML, JSON, YAML or any serde format |

A crate that is `no_std` turns `std` off with `default-features = false`.

## From a derive that writes a value, a schema and a reader

A type that derived three traits to be read from a value, written to one and described by a
schema derives one trait here:

```rust
// before
#[derive(Debug, Clone, PartialEq, ToValue, FromValue, Schema)]
#[map(deny_unknown_fields)]
pub struct Limits { /* ... */ }

// after
#[derive(Debug, Clone, PartialEq, coqui_builder::Describe)]
#[map(deny_unknown_fields)]
pub struct Limits { /* ... */ }
```

`Describe` gives the description (what each key is), the fill (`Partial<Limits>`) and the
reader of a key (`Inspect`). A struct is read from a file by filling a `Partial` from a source
(`Partial::fill_from`, `Env`) or, with the `serde` feature, from any serde format
(`toml::from_str::<Partial<Limits>>(text)?`), and then `build()`.

### Attributes

These keep their spelling and meaning on a field, a variant or a type:

| Attribute | Notes |
| --- | --- |
| `#[map(rename = "x")]`, `#[map(rename_all = "..")]` | `lowercase`, `snake_case`, `kebab-case`, `SCREAMING_SNAKE_CASE` |
| `#[map(default)]`, `#[map(default = expr)]` | the key is then not required |
| `#[map(skip)]`, `#[map(flatten)]`, `#[map(transparent)]` | `flatten` needs a struct of named fields |
| `#[map(deny_unknown_fields)]` | on a struct |
| `#[map(tag = "k")]`, `#[map(untagged)]`, `#[map(other)]` | see "A setting written as a flag or a table" |
| `#[map(as_text)]` | `Display` out, `FromStr` in |
| `#[schema(title, description, sensitive, ..)]` | and a doc comment is the description |
| `#[schema(min_length, max_length, multiple_of, min_items, max_items)]` | bounds checked when a key is set |
| `#[leaf(display)]` | on a unit-only enum that has its own `Display` and `FromStr` |
| `#[builder(stricter = "higher")]`, `#[builder(setter = "name")]` | the order a profile reads; the setter's Rust name |
| `#[map(crate = path)]` | read and ignored; `#[builder(crate = path)]` names where `coqui_builder` is re-exported |

What changes:

- **A field is one text.** A setting is read from, and written to, one text by its type's
  parser. `bool`, the integers, `f32`, `f64`, `char`, `String`, `Duration`, the `core::net`
  addresses and (`std`) `PathBuf` have one. Any other type gets one with `#[derive(Leaf)]` (from
  `Display` and `FromStr`, or from the variant names of a unit-only enum), with
  `#[map(as_text)]` on the field, or with `#[map(with = module)]`. A type that has no text form
  is not a setting: it is an input of a builder.
- **A `with` module is three items**, not a value converter:

  ```rust
  mod name_bytes {
      use coqui_builder::describe::LeafError;
      pub const TYPE: &str = "Name";                       // the name a datum shows
      pub fn to_text(name: &[u8]) -> String { /* ... */ }
      pub fn from_text(text: &str) -> Result<Vec<u8>, LeafError> { /* ... */ }
  }
  // #[map(default = DEFAULT.to_vec(), with = name_bytes)]
  ```

  The text must read back as the value (`from_text(&to_text(&x))` is `x`), for every value the
  field can hold: `to_text` cannot fail, so a byte string writes the bytes that are not
  printable ASCII escaped (`%FF`) rather than losing them. The module cannot use
  a generic parameter of the struct it sits in, and it declares no order.
- **A list is its items.** A `with` module that turned a whole `Vec<T>` into one value is gone:
  a list of values with a text form is `#[map(as_text)] Vec<T>` (per item), a list of types
  that are leaves needs no attribute at all, and a bound on each item is the item type's parser
  (a newtype whose `FromStr` refuses the empty name and the name over 255 bytes).
- **No tuple struct of two or more fields**, no generic enum, no lifetime on a settings type.
  A newtype is its field's setting.
- **`#[map(skip_serializing_if = ..)]` is not an attribute.** Nothing is serialized here.
- **An `Option` field with no `default`** is absent unless given; one with
  `#[map(default = Some(..), none = "never")]` is the next section.

### A timeout that waits forever

A `Duration::MAX` that meant "no bound" becomes an `Option` with a written `None`:

```rust
// before
/// The bound on each read; `Duration::MAX` waits forever.
#[map(default = DEFAULT_TIMEOUT)]
pub read_timeout: Duration,

// after
/// The bound on each read; `never` waits forever.
#[map(default = Some(DEFAULT_TIMEOUT), none = "never")]
pub read_timeout: Option<Duration>,
```

Absent is the default (`Some`), the text `never` is `None`, and any duration text is `Some`.
`none` needs `default = expr`, and the inner type must not itself read the `none` text. A reader
of the field matches on the `Option` where it compared with `Duration::MAX`. An option whose
`Default` was `Some(..)` but whose document left the key out used to read `None`; it now reads
the default.

A duration's text is a decimal number and an optional unit (`ns`, `us`, `ms`, `s`, `m`, `h`; no
unit is seconds): `30`, `0.25`, `250ms`, `1.5h`. It is written as whole seconds, else
milliseconds, microseconds or nanoseconds (`30s`, `250ms`). A `[secs, nanos]` pair is not read.

### A setting written as a flag or as a table

`#[map(untagged)]` on an enum holds any number of text arms (unit names, `Name(bool)`,
`Name(u16)`) and at most one structured arm (a group, a list, a map or a tagged enum):

```rust
#[derive(Describe)]
#[map(untagged)]
pub enum CacheSetting {
    Enabled(bool),
    Tuned(CacheOptions),
}
```

`cache = false` and `cache = { ttl = "30s" }` both read; in the environment `APP_CACHE=false` is
the text arm and `APP_CACHE__TTL=30s` the structured one. A second structured arm fails the build
of the enum.

### What a report shows

A setting is shown as its type and its text: `bool(true)`, `Duration(30s)`, `usize(32)`,
a secret as `String(***)`, an absent value as
`unset`, a list as `[TlsVersion(1.3)]`. Code and tests that compared the earlier plain text
(`"true"`, `"[30,0]"`) compare these. The order is by key text. A list of groups, or an optional
group, present on one side only is one place at its path, not one per key: read
`non_defaults(&inner)` for the inner type's own list.

## From a hand-written builder

A builder that held the settings beside the code it also needed becomes a parts struct:

```rust
// before
pub struct ServerBuilder<'a> { identity: &'a Identity, auth: ClientAuth, alpn: Vec<Vec<u8>>, policy: Policy }
impl<'a> ServerBuilder<'a> {
    pub fn new(identity: &'a Identity, auth: ClientAuth) -> Self { /* ... */ }
    pub fn alpn(mut self, ..) -> Self { /* ... */ }
    pub fn policy(mut self, policy: Policy) -> Self { /* ... */ }
    pub fn settings(mut self, settings: TlsSettings) -> Self { /* ... */ }
    pub fn build(self) -> Result<Built<ServerConfig>, Error> { /* ... */ }
}

// after
#[derive(Builder, Debug)]
#[builder(build = Built<ServerConfig>, name = "ServerBuilder", entry = "new", fallible)]
pub struct ServerParts<'a> {
    settings: TlsSettings,                 // the one Describe field
    #[builder(input)] identity: &'a Identity,
    #[builder(input)] client_auth: ClientAuth,
}

impl TryFrom<ServerParts<'_>> for Built<ServerConfig> {
    type Error = Error;
    fn try_from(parts: ServerParts<'_>) -> Result<Self, Error> { /* ... */ }
}
```

- **Settings** are the one `Describe` field; their setters come from `<Name>Setters`, a trait the
  settings type's derive writes (`TlsSettingsSetters`), which the builder's user imports. A
  setter takes `impl Into<FieldType>`; an `Option` setting takes the inner value or an `Option`. A
  setting (or input) written as a primitive scalar (`bool`, `char`, the integers, `f32`, `f64`)
  takes that type itself, so `.max(8)` needs no suffix; a caller that passed another type through
  `Into` (a `u8` into a `u16` field) now converts it first.
- **Setters without an import.** When the settings and the builder live in the same crate, put
  `#[builder(setters_on = XBuilder)]` on the settings (repeat it for each builder that holds them;
  `<'a, C: Trait> XBuilder<'a, C>` for a generic one): the setters become inherent methods of the
  builder and callers import nothing. An inherent `impl` must be in the builder's own crate, so for a
  builder elsewhere (E0116 otherwise) re-export `<Name>Setters` from the crate's prelude.
- **Inputs** are every other field, marked `#[builder(input)]`: a value with no text form,
  a borrowed value, code. An input of type `Option<J>` is optional; every other input is required
  and `build` names it if it is missing, together with every missing setting.
- **The entry.** `X::builder()` is written on the built type. `entry = "new"` writes
  `XBuilder::new()` instead (and `Default`); it is required when the parts struct has a generic
  parameter or a lifetime, and when the built type is not defined in the crate, since an inherent
  `X::builder()` cannot be written on it. `name = ".."` names the builder, and is required when
  the built type has generic arguments. A
  constructor argument becomes an input (`.identity(&id)`) or, when its type has a text form and
  it belongs in a file, a required key of the settings.
- **Generic inputs are inferred.** An input whose type is exactly a type parameter of the
  parts struct (`channel: C`) has a setter taking `C` itself, so
  `ChannelBuilder::new().port(1).channel(Framing)` needs no turbofish. Any other input takes
  `impl Into<T>`, an `Option<C>` among them (name the parameters when nothing else fixes them). A
  caller that named the parameter and passed a value converting into it now converts it first.
- **A second terminal.** `parts()` gives the parts struct with the settings built, or the same
  `Missing` that `build` gives, and `build` is `parts()` followed by the conversion. A
  `build_async()` or a `configs()` that gives two values is a method written over it. A builder is
  not `Clone`.
- **Layers.** `settings(s)` becomes `fill(Partial::from(s))`; a file, the environment and flags
  are `fill(partial)` and `fill_from(&mut source)`; the last write of a key wins, whichever way it
  was written, setters included.
- **Keys below the top level.** There is no setter for a nested key. A method that sets one
  calls `self.partial_mut().put("waiting.limits", limits)` (import `builder::Holds`); the typed
  value must be the type of the key.
- **The built type** derives `Configured` with `#[configured]` on its settings field, which
  gives it `non_defaults()` equal to its builder's, and a `Profile` can check either.
- **Errors.** `fallible` uses `TryFrom` and reports `BuildError<E>`: `Missing(..)`, or `Failed(e)`
  with the crate's own error. `build` without it uses `From` and reports `Missing`.
- **Debug.** A parts struct that is public and part of a crate with
  `missing_debug_implementations` derives `Debug`; an input that is not `Debug` takes a manual
  implementation of the parts struct.

## From a hand-kept non-default report

A type that listed `{setting, value, default}` as text, by walking its own description, is
replaced by the crate's:

```rust
// before
let found: Vec<crate::NonDefault> = crate::NonDefault::of_settings(&settings);
// after
let found: Vec<coqui_builder::NonDefault> = coqui_builder::non_defaults(&settings);
// or, on a built type that derives Configured, or on its builder
let found = built.non_defaults();
```

`NonDefault` holds `setting: String`, and `value` and `default` as a `Shown` (print it with
`to_string()`). Delete the hand-kept type and the code that compared two values through a schema;
`coqui_builder::differences(&Settings::description(), &a, Some(&b))` does that: it returns a
`Report`, and `report.findings().iter().map(NonDefault::from).collect()` is the list. A required
key with no default is always listed (against `unset`). A setting that is not in the description
(a random source replaced by code, say) is added to the list by the caller.

## Checking against a profile

A `Profile` holds an expectation per key (`Equal`, `OneOf`, `Within`, `SubsetOf`, `ItemsEqual`,
`Required`, `Absent`, ..), each a text read by the field's own parser:

```rust
let modern = Profile::new()
    .expecting("tls_versions", Expect::SubsetOf(vec!["1.3".into()]))?;
let report = modern.verify(&policy);   // what is met, and why not
```

A builder is not `Describe`, so `verify` is not available on it: make
`Report::new(&Settings::description())` and call `profile.check(&builder, &mut report)`.
