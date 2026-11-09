# Merging layers

`anycfg` loads sources from left to right and delegates value merging to the application. It does not prescribe how maps, lists, nulls, special numbers or custom value types behave.

## Strategies

`Merge` provides three strategy names for your adapter:

| Strategy | Conventional meaning | Who implements it? |
| --- | --- | --- |
| `Merge::Replace` | Later source replaces the whole document | Your adapter, or `ReplaceValues` |
| `Merge::Shallow` | Merge top-level entries; replace nested values | Your adapter |
| `Merge::Deep` (default) | Recursively merge maps; define array/scalar rules | Your adapter |

The strategy values alone do not manipulate configuration. Use `Loader::merge` to choose one, then provide an implementation of `MergeValues<V>`.

## Replacement strategy

Replacement works for any value representation:

~~~rust
# use anycfg::{Callbacks, CodecError, Formats, Loader, Merge, ReplaceValues, Source};
# fn main() -> Result<(), anycfg::Error> {
# let mut formats = Formats::<String>::new();
# formats.register("text", ["txt"], Callbacks::new(
#     |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| CodecError::default()),
#     |value: &String| Ok(value.as_bytes().to_vec()),
# ))?;
let value = Loader::new(&formats, "text")
    .merge(Merge::Replace)
    .load([Source::text("base"), Source::text("override")], &ReplaceValues)?;
assert_eq!(value, "override");
# Ok(())
# }
~~~

## Custom merging with a callback

For flat models, pass a closure matching the `MergeValues` callback signature. This example concatenates sources rather than replacing them:

~~~rust
# use anycfg::{Callbacks, CodecError, Error, Formats, Loader, Merge, Source};
# fn main() -> Result<(), Error> {
# let mut formats = Formats::<String>::new();
# formats.register("text", ["txt"], Callbacks::new(
#     |bytes: &[u8]| String::from_utf8(bytes.to_vec()).map_err(|_| CodecError::default()),
#     |value: &String| Ok(value.as_bytes().to_vec()),
# ))?;
let value = Loader::new(&formats, "text").load(
    [Source::text("alpha"), Source::text("beta")],
    &|before: &mut String, next: String, _strategy, _limit| {
        before.push_str(&next);
        Ok(())
    },
)?;
assert_eq!(value, "alphabeta");
# Ok(())
# }
~~~

A callback adapter's default depth validation is a no-op. Use a custom `MergeValues<V>` implementation for recursively nested values.

## Structural validation

The loader passes a **64-level** default depth budget to the adapter, a setting with no floor. The adapter's `validate` method runs on each decoded document. Your implementation must enforce that limit for a recursive `V`; the library cannot inspect arbitrary user-defined representations. A typed load also enforces it on the merged value.

The merge callback receives `max_depth` and should enforce it during recursion as well. Registered format parsers must have their own recursion and expansion bounds.

## Typed result after merging

When `V` is a Serde deserializer, `Loader::load_typed` merges documents first and then deserializes the final result as your struct. For non-Serde representations, `load_as` accepts a conversion callback.

For an example that defines `MergeValues<serde_json::Value>` and combines TOML, YAML and JSON, see [Serde integration](serde.md) or the [full example](https://github.com/jose-pr/anycfg-rs/blob/main/examples/three_formats.rs).
