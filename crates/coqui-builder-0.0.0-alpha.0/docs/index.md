# coqui-builder

A **crate to describe, fill, check and build any struct field by field: builders, partials and
profiles, with no value model and no dependencies**. Settings are the main example. A struct
derives `Describe` once; from that one description it is
filled from any source, a missing key is named, a value is compared with its defaults, and a
profile says whether a value is what is expected, with every secret hidden. The
[Rust API reference](https://boriken-dev.github.io/coqui-builder/rust/index.html) lists every item.

## Installation

```bash
cargo add coqui-builder --features derive
```

| Feature | Adds | Needed for |
| --- | --- | --- |
| `std` (default) | `PathBuf`, `HashMap` and the `Env` source | the environment as a source |
| `derive` | `coqui-builder-derive` (`syn`, `quote`) | `#[derive(Describe)]` |
| `zeroize` | `zeroize` | `Secret<T>`; sensitive datums, source texts for sensitive places and `Env` values are wiped on drop |
| `serde` | `serde` (and its `serde_core`) | `Deserialize` for `Partial<T>`: any serde format fills a `Partial` |

## 30-second tour

Describe a type once:

```rust
use coqui_builder::Describe;

#[derive(Describe, Debug, PartialEq)]
struct Limits {
    /// How many sources may wait at once.
    #[map(default = 16)]
    waiting: u32,
    #[schema(sensitive)]
    #[map(default)]
    secret: String,
}
```

Fill it a key at a time, from text or a source such as the environment; the last write
of a key wins, and `build` names every key still missing:

```rust
let mut partial = Partial::<Limits>::new();
partial.set("waiting", "64")?;
let limits = partial.build()?;
```

Say what differs from the defaults, as `{setting, value, default}`, each a type and its text,
with a secret as `String(***)`:

```rust
for found in non_defaults(&limits) {
    println!("{found}"); // waiting = u32(64) (default u32(16))
}
```

Say what a value is expected to be, and read why it is not:

```rust
let profile = Profile::new().expecting("waiting", Expect::Equal("16".into()))?;
let report = profile.verify(&limits);
```

## A builder

A parts struct holds the settings type beside the inputs that are no text, and
`#[derive(Builder)]` writes the builder. Import the settings' setters trait
(`<Name>Setters`, from the crate that defines the settings) to call them; a builder in the
same crate as its settings needs no import when the settings say `#[builder(setters_on = XBuilder)]`.

```rust
#[derive(Builder)]
#[builder(build = Server)]
struct ServerParts {
    settings: ServerSettings,
    #[builder(input)]
    identity: Identity,
}

let server = Server::builder()
    .fill(file)           // layers: a Partial, or fill_from(&mut source)
    .port(9000)           // a typed setter of the settings; a primitive takes a bare literal
    .identity(identity)   // an input
    .build()?;            // names every missing setting and input at once
```

A second way to finish a builder (an `async fn build_async`, a `configs()` that gives two
values) is written by hand over `parts()`, which gives the parts struct with the settings
built, the required inputs present and the optional ones as given, or the same `Missing` that `build` gives:

```rust
impl ServerBuilder {
    async fn build_async(self) -> Result<Server, Missing> {
        let parts = self.parts()?;
        // ... await whatever the parts need, then convert them
        Ok(Server::from(parts))
    }
}
```

A builder is not `Clone`: an input need not be.

A key below the top level is set with `Partial::put("limits.total", 5u32)` through
`partial_mut()` (a method of `coqui_builder::builder::Holds`, so import it). The built type's `Configured` view gives `non_defaults()` and lets a `Profile`
check a builder as it checks a built value.

## A value written as text or as a table

An enum marked `#[map(untagged)]` holds any number of arms that are a single value (unit names,
`Name(bool)`, `Name(u16)`) and at most one structured arm (a group, a list, a map or a tagged
enum). A source that answers a value at the enum's key means a text arm; one that answers
entries, or any key below it, means the structured arm.

```rust
#[derive(Describe, Debug, PartialEq)]
#[map(untagged)]
enum CacheSetting {
    Enabled(bool),
    Tuned(CacheOptions),
}
```

`cache = true`, `cache = false` and `cache = { ttl = "30s" }` all read. In the environment,
`APP_CACHE=false` is the text arm and `APP_CACHE__TTL=30s` the structured one; setting both
is an error. Setting a key below the enum switches a `Partial` to the structured arm, and
setting the enum's text switches it back, dropping what the other side held. A second
structured arm fails the build of the enum: tell the arms apart with `#[map(tag = "..")]`.

## Learn more

- [Rust API reference](https://boriken-dev.github.io/coqui-builder/rust/index.html)
- [Changelog](https://github.com/Boriken-Dev/coqui-builder/blob/main/CHANGELOG.md)
- [Source](https://github.com/Boriken-Dev/coqui-builder)
