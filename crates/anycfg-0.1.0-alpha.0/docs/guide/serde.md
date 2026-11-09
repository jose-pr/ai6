# Serde and typed configuration

Applications deserialize a registered format into a Serde-compatible value `V`, merge those values, and then read the result into any compatible Rust type. The library does not depend on `serde_json`, `toml` or `serde-saphyr`: those are **application-selected codecs**, present as development dependencies for the example and the tests.

## Example dependencies

~~~toml
[dependencies]
anycfg = { git = "https://github.com/jose-pr/anycfg-rs" }
serde = { version = "1", features = ["derive"] }

# Choose your own format implementations:
serde_json = "1"
toml = "0.9"
serde-saphyr = "2"
~~~

## Read into a struct

Choose a `V` that can act as a Serde deserializer:

~~~rust
use anycfg::{Callbacks, CodecError, Formats, Options};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Server {
    host: String,
    port: u16,
}

fn main() -> Result<(), anycfg::Error> {
    let mut formats = Formats::<Value>::new();
    formats.register("json", ["json"], Callbacks::new(
        |bytes: &[u8]| serde_json::from_slice::<Value>(bytes)
            .map_err(|error| CodecError::new().line(error.line()).column(error.column())),
        |value: &Value| serde_json::to_vec(value)
            .map_err(|_| CodecError::new().message("the value has no JSON form")),
    ))?;

    let server: Server = formats.from_slice_typed(
        br#"{"host":"localhost","port":8080}"#,
        Options::new().name("server.json"),
    )?;
    assert_eq!(server.port, 8080);

    // The caller supplies the Serde serializer used to create the same V.
    let encoded = formats.to_vec_typed(&server, "json", serde_json::value::Serializer)?;
    let decoded: Server = formats.from_slice_typed(&encoded, Options::new().format("json"))?;
    assert_eq!(server, decoded);
    Ok(())
}
~~~

The codec selects how bytes become `V`; Serde then selects how `V` becomes `Server`. `Formats::load_typed` does the same from a file. `V` must be self-describing, because it is read through `deserialize_any`.

## What a refusal says

A typed read names the key and never quotes a value. Serde's own text for a wrong type, `invalid type: string "hunter2", expected u16`, does not reach an error; nor does anything a `custom` call was given, which may hold anything.

| Failure | `Error::Conversion` displayed |
| --- | --- |
| Unknown field | ``invalid configuration at `server.colour`: unknown field `colour`, expected one of `host`, `port` `` |
| Missing field | ``invalid configuration at `server`: missing field `port` `` |
| Duplicate field | ``invalid configuration at `server`: duplicate field `port` `` |
| Wrong type | ``invalid configuration at `server.port`: expected u16, found a string`` |
| Out of range | ``invalid configuration at `server.port`: invalid value, expected u16, found an integer`` |
| Unknown enum variant | ``invalid configuration at `level`: unknown variant, expected one of `info`, `warn` `` |
| A type's own refusal | ``invalid configuration at `token`: refused by its type`` |

`path` and `message` are also fields of the variant. To say more about a value a type refuses, validate after the read, where the message can be written knowing what it holds.

## Register TOML, YAML and JSON

Each format supplies its own parser and serializer, and all three return the application's chosen value:

~~~rust
# use anycfg::{Callbacks, CodecError, Formats};
# use serde_json::Value;
# fn formats() -> Result<Formats<Value>, anycfg::Error> {
# let decode_toml = |_: &[u8]| Ok::<Value, CodecError>(Value::Null);
# let decode_yaml = decode_toml;
# let decode_json = decode_toml;
# let encode = |_: &Value| Ok::<Vec<u8>, CodecError>(Vec::new());
let mut formats = Formats::<Value>::new();
formats.register("toml", ["toml"], Callbacks::new(decode_toml, encode))?;
formats.register("json", ["json"], Callbacks::new(decode_json, encode))?;
formats.register("yaml", ["yaml", "yml"], Callbacks::new(decode_yaml, encode))?;
formats.register_auto(["cfg"], "toml")?;
formats.register_marker("yaml", b"%YAML")?;
# Ok(formats)
# }
~~~

The extensions `.toml`, `.json`, `.yaml` and `.yml` select their codecs directly. A `.cfg` file names its format on its first line (`#!json`, `#!yaml` or `#!yml`, `#!toml`), or starts with `%YAML`, and is otherwise TOML.

The decoders are where a secret can leak, because the parsers' own `Display` prints the source line. [`examples/three_formats.rs`](https://github.com/jose-pr/anycfg-rs/blob/main/examples/three_formats.rs) has the three in full, with what each passes on:

- `toml`: `error.message()` and `error.span()`, which hold no document text.
- `serde_json`: its message without the position, with a target that refuses a repeated key, which `Value` would otherwise resolve to the last one.
- `serde-saphyr`: `error.location()`, and the text of only the variants known to hold none.

Copy them, and keep a test planting a secret in malformed documents as `tests/failures.rs` does.

## Merge formats, then deserialize

Once codecs and an application-specific `MergeValues<Value>` adapter are registered, the loader can merge several different document formats:

~~~rust
# use anycfg::{Loader, Source, MergeValues, Error, Formats};
# use serde::Deserialize;
# use serde_json::Value;
# #[derive(Deserialize)]
# struct Config { name: String, server: Server }
# #[derive(Deserialize)]
# struct Server { host: String, port: u16 }
# fn example<M: MergeValues<Value>>(formats: &Formats<Value>, merge_adapter: &M) -> Result<(), Error> {
let config: Config = Loader::new(formats, "toml").load_typed(
    [
        Source::text("name = 'demo'\n[server]\nhost = 'localhost'\nport = 8000\n")
            .name("base.toml"),
        Source::text("server:\n  port: 8080\n")
            .name("override.yaml"),
        Source::text("#!json\n{\"server\":{\"host\":\"example.org\"}}")
            .name("local.cfg"),
    ],
    merge_adapter,
)?;

assert_eq!(config.server.host, "example.org");
assert_eq!(config.server.port, 8080);
# Ok(())
# }
~~~

## Data fidelity

A shared value model is a choice by the application. `serde_json::Value`, with its `preserve_order` feature, keeps a document's order and is convenient for mappings, arrays and scalars, but it has no datetime, YAML tag or non-string map key. The example turns a TOML datetime into text: deserialized straight into `Value` it would be a map under `toml`'s private key.

For applications requiring that information, choose a more expressive `V`, with matching codecs and a matching `MergeValues<V>`. A typed read copies `V` through `deserialize_any` once, so what `T` sees is what `V` reports there, in `V`'s order.
