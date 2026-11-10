# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `#[builder(setters_on = XBuilder)]` on a settings struct (repeatable, `<'a, C: Tr> XBuilder<'a, C>` for a
  generic builder) writes the setters as inherent methods of a builder in the same crate, so callers need
  no import of `<Name>Setters`. The trait remains for builders in other crates.
- `Datum`, a type name and a text (`u32(64)`, printed `String(***)` for a secret), and `Shown`
  (unset, one datum or a list of them): what a finding or a non-default holds. `KeyPath` /
  `Segment`: the path grammar `agent[1].name`, `env["a.b"]`, parsed and written so that every
  path round-trips.
- `Leaf`: a type that is one setting, as one text. It names its type, parses and prints its own
  text, may declare an order and its choices, and is held to a round trip (`assert_round_trip`).
  Implemented for `bool`, the integers up to `u128` and `i128`, `f32`, `f64`, `String`, `char`,
  `Duration`, the `core::net` addresses and (`std`) `PathBuf`. `Codec` is the same for
  `with` and `as_text`.
- Durations read `30`, `0.25`, `30s`, `250ms` and `1.5h` exactly, and are written exactly as
  seconds, else milliseconds, microseconds or nanoseconds; a negative, a non-number or anything
  finer than a nanosecond is refused.
- `Describe`, `Description`, `Kind`, `Field` and `Notes`: what a settings type is, as an owned
  tree built on each call.
- `Fill`, its slots and `Partial`: fill a type by path and text, replace a list whole with
  `set_items`, `overlay` one over another, `clear` a key, and `build`, which names every missing
  key at once. Bounds the description states are checked on `set`; a failed `set` leaves no
  trace.
- `Source`, `Entries` and `Partial::fill_from`: fill from anything that answers a text per leaf,
  a length per list and keys per open map; keys a source holds that the description lacks are
  returned, or refused under `deny_unknown_fields`. `Env` (`std`) reads `PREFIX_A__B` variables.
- `Inspect`: read a key of a built value, a fill or a `Partial` as a `Datum`, and `entries`
  (a list's length or the keys that are set).
- `Report`, `Finding`, `Reason`, `Check` and `Checks`: findings with every secret hidden, in the
  value and in the expectation, through list items, open-map values and arms.
- `NonDefault`, `non_defaults` and `differences`: every setting that differs from its default or
  from a second value, compared as typed values, never as text.
- `Profile` and `Expect` (`Equal`, `Within`, `OneOf`, `AsStrictAs`, `NoStricterThan`, `SubsetOf`,
  `ItemsEqual`, `Required`, `Absent`, `Arms`, `Any`), each holding texts that the field's own
  parser reads when the profile is bound (`Profile::bind`): build one by hand, from a type's
  defaults (`Profile::defaults`) or from a value (`Profile::against`), `merge` them, and check a
  value. A secret is expected to be set, never to equal a copy of itself.
- An untagged enum (`#[map(untagged)]`) holds any number of text arms (unit names and
  single-value arms, tried in order) and at most one structured arm (a group, list, map or
  tagged enum): `Kind::Union(Union)` with `text()` and `structured()`, `Describe::LEAF`, and a
  derive that implements `Describe` rather than `Leaf` for an enum that holds a value. A source's
  `value` at the enum's key is the text side and `entries` (or any key below) the structured one,
  both together an error (`APP_CACHE=false` against `APP_CACHE__TTL=30s`); setting a key below
  switches a `Partial` to the structured arm, setting the text switches it back. A second
  structured arm, or one that is itself such an enum, fails the build of the enum. The walks
  (`fill_from`, `overlay`, `non_defaults`, `differences`, profiles, redaction) follow whichever
  side a value holds; a finding between differing sides shows the structured side as
  `Shown::Fields`.
- Typed setters: `#[derive(Describe)]` on a struct also writes `<Name>Setters`, a trait with one
  method per setting taking `impl Into<FieldType>` (the last call wins), implemented for every
  `builder::Holds<Name>`; import it, from any crate, to call the setters. `Fill::put` sets every
  key from a value and keeps the defaults; `Partial::put(key, value)` does it for a typed value
  at any path (`waiting.limits`, `peers[1]`, an untagged enum's arm) and refuses a value of
  another type or an unknown key, changing nothing.
- `#[derive(Builder)]` on a parts struct (one settings field, every other field a
  `#[builder(input)]`) writes the builder: `X::builder()` (`entry = "new"` gives
  `XBuilder::new()`, for a type that is not local), a setter per input, `fill` (overlay),
  `fill_from` (a source; the unknown keys are dropped), `From<Partial<S>>` and `build`, which
  lists every missing setting and input at once (`Missing::inputs`, `Missing::is_empty`) and
  runs `From` over the parts, or with `fallible` `TryFrom` and `BuildError<E>`.
  `name = ".."` names the builder and is required for a built type with generic arguments.
- `Configured`, the link between a builder, what it built and a profile: the settings through
  `Inspect`, `non_defaults`, `Check<Builder> for Profile` and `Profile::judge`; written by
  `#[derive(Builder)]` and, on a built type with one `#[configured]` field, `#[derive(Configured)]`.
- `Secret<T>` (feature `zeroize`): a setting that is sensitive in its description and zeroized
  when it drops. With the feature, a sensitive `Datum`, the text a source hands over for a
  sensitive place and the values of an `Env` are wiped on drop as well; `Codec::SENSITIVE`
  marks a codec's place and its data.
- `Deserialize` for `Partial<T>` (feature `serde`): TOML, JSON or any serde format fills a
  `Partial` with the rules of every other source, and a builder takes it with `From` or `fill`.
  The errors it writes never quote the input.
- `#[derive(Describe)]` and `#[derive(Leaf)]` (feature `derive`) with the attributes of
  guatiao's derives (`rename`, `rename_all`, `skip`, `default`, `flatten`, `transparent`,
  `deny_unknown_fields`, `tag`, `untagged`, `other`, `as_text`, `with`, `#[schema(..)]`), plus
  `none` (the text that writes the `None` of an `Option`) and `#[builder(stricter, setter,
  crate)]`.
- A migration guide (`docs/migrating.md`): from derives that write a value and a schema to
  `Describe`, from a hand-written builder to `#[derive(Builder)]` and `Configured`, and from a
  hand-kept non-default report and a `Duration::MAX` that waits forever to `non_defaults` and
  `none = "never"`. The test `settings_shapes` holds settings types of the shapes the guide covers
  (lists, optional timeouts, units, groups, a flag or a table, byte strings, bounds), with
  builders over several of them.
- `parts()` on a builder `#[derive(Builder)]` writes: the parts struct with its settings built,
  its required inputs present and its optional inputs as given, or the same `Missing` that
  `build` gives. `build` is `parts()` followed by the conversion, and a second terminal is written
  over `parts()`. `parts` joins the names a setting or an input cannot take for its setter.

### Changed

- A setter for a field written as a primitive scalar (`bool`, `char`, the integers, `f32`, `f64`), on the
  settings and on a builder input (an `Option<u32>` input takes the `u32`), takes that type itself, so
  `.max(8)` infers. Every other type keeps `impl Into<T>`. A call that passed another type through `Into`
  (a `u8` into a `u16` field) must convert it first.
- An input whose type is exactly one of the parts struct's type parameters has a setter taking
  that type, not `impl Into<..>`, so setting it infers the parameter and needs no turbofish. Every
  other input still takes `impl Into<T>`, an `Option<C>` among them. A caller that named the
  parameter and passed a value converting into it now converts it first (`.name("x".into())`).
