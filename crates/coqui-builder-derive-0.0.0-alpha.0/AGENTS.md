# `coqui-builder-derive` — API header

Header-file-style reference for the `coqui-builder-derive` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-builder>.

Derives for `coqui-builder`: describe, fill and build any struct field by field.

A proc-macro crate with no features; its dependencies are `syn` 2 (default features),
`quote` 1 and `proc-macro2` 1. MSRV 1.89. It holds no `unsafe`. It exports two macros and is
reached through `coqui-builder`'s `derive` feature, which re-exports it beside the trait of
the same name.

## `#[derive(Describe)]`

```rust
#[proc_macro_derive(Describe, attributes(map, schema, builder, leaf))]
#[proc_macro_derive(Leaf, attributes(leaf, map, schema))]
```

Writes, inside an anonymous `const` so nothing leaks into the caller's namespace:
`Describe` (with the hidden `Fill` the `Partial` holds), `Inspect`, and for a struct of named
fields `Fields`; for an enum of unit variants or an untagged enum of unit variants, `Leaf` (a
type that is one text); for an untagged enum that holds a value, `Describe` and `Inspect` alone
(see below). The expansion
names `::coqui_builder` (or the path of `#[builder(crate = path)]`), `::core` and the
hidden `coqui_builder::__private`.

| Declaration | Setting |
| --- | --- |
| struct with named fields | a group, one key per field; may be generic over types |
| newtype struct, or `#[map(transparent)]` struct | the field's own setting |
| enum of unit variants | a `Leaf` with choices; `#[map(other)] Other(String)` keeps an unknown name |
| enum with `#[map(tag = "k")]` | arms: unit, named-field or `Name(T)` (the keys of `T`, a struct with named fields, sit beside the tag) |
| enum of unit variants with `#[map(untagged)]` | a `Leaf` of its names |
| enum with `#[map(untagged)]` and a `Name(T)` variant | text arms and at most one structured arm (below) |

Refused with a sentence: a tuple struct of two or more fields, a unit struct, a union, a
generic enum, a lifetime or const parameter, an untagged variant with named fields, a
variant that carries fields in an enum without a tag, and a field whose type names the type
it is in; the derive's header in `src/lib.rs` lists every refusal, by group.

### Untagged enums that hold a value

Each `Name(T)` needs `T: Describe + Inspect`. The unit names and every `T` that is one text
(`Describe::LEAF`: a `Leaf`, or an untagged enum of texts) are the **text arms**: one leaf
whose parser tries the unit names, then each text arm in declaration order. The one `T` that
is not (a group, list, map or tagged enum) is the **structured arm**. A source answering
`value` at the enum's key means a text arm; `entries`, or any key below, means the structured
arm. Both is a `SourceError`. Without a structured arm the description is a leaf, with one it
is `Kind::Union`.

The derive cannot see whether `T` is one text, so it emits a `const` assertion over
`Describe::LEAF`. A second structured arm, or an arm that is itself an untagged enum holding a
structured arm, fails the build of the enum, a compile error (E0080) whose message is
``Either: an untagged enum holds at most one arm that is not a single value; tell the others
apart with a tag`` (and, for the nested case, ``the structured arm of an untagged enum is a
group, a list, a map or a tagged enum, not another untagged enum that holds a structured
arm``). An arm that names the enum itself is refused like any self-reference. It is never a
run-time error. Two more fail the same way: an arm that is an `Option` (``an untagged enum's
arm holds a value; make the field that holds the enum optional instead``) and an enum with no
unit variant and no single-value arm (``an untagged enum with one arm is that arm: use a
newtype``). Choices are the unit names when the enum has no other text arm.

### `#[derive(Leaf)]`

Makes a type one setting, as one text. A unit-only enum reads and writes its variant names
(`#[map(rename)]`, `#[map(rename_all)]`); any other type uses its `Display` and `FromStr`
(`from_text` refuses without quoting its input). `#[leaf(name = "..")]` names the type a datum
shows (default: the type's name), `#[leaf(ordered)]` takes the order from `PartialOrd`,
`#[leaf(display)]` makes a unit-only enum use `Display` and `FromStr`. Equality is the
canonical text, as `Leaf::same` states.

### Attributes

`#[map(..)]` on a type: `rename_all = "lowercase" | "snake_case" | "kebab-case" |
"SCREAMING_SNAKE_CASE"`, `transparent`, `deny_unknown_fields` (struct); `tag = ".."`,
`untagged`, `rename_all` (enum); `crate = path` (accepted and ignored: guatiao's spelling).

`#[map(..)]` on a field: `rename = ".."`, `skip`, `default`, `default = <expr>`, `flatten`,
`with = path`, `as_text`, `none = ".."`. On a variant: `rename = ".."`, `other`.

`#[schema(..)]` on a field: `title`, `description`, `section`, `order`, `advanced`,
`sensitive`, `default = <expr>`, `min_length`, `max_length`, `pattern`, `format`,
`multiple_of`, `min_items`, `max_items`, `read_only`, `deprecated`, `examples(..)`. A doc
comment is the description unless one is given. On a tagged variant: `title`, `description`;
on a unit variant: `label`.

`#[builder(..)]` on a field: `stricter = "higher" | "lower"`, `setter = "name"`. On a type:
`crate = path`, `setters_on = Type` (repeatable).

`setters_on = XBuilder` (or `<'a, C: Tr> XBuilder<'a, C>`, parsed as `syn::Generics` then `syn::Type`; the
path resolves in the settings' module) writes, beside the trait, `impl XBuilder where Self: Holds<S>` with
one method per setting: the trait method's name, docs, visibility and argument type, its body
`<Self as SSetters>::name(self, value)`. An inherent method wins at the call site, so a caller in the
builder's crate needs no import; an impl on a builder of another crate is E0116, and that crate keeps the
trait. Refused: no type, a non-type value, an enum, a newtype, a `#[map(transparent)]` struct, a settings
type with generic parameters. Two impls of one setter name on a builder (the same target twice, an input
setter named like a setting, a hand-written method) are rustc's E0592.

Contracts:

- A key is required unless the field has a default or is an `Option`.
- `default` on an `Option` is refused unless `none = ".."` is beside it with `default = <expr>`;
  `none` is refused on a field that is not an `Option`, beside `with`, `as_text` or `flatten`,
  and without a default expression. The inner type's parser must refuse the `none` text, or the text would be ambiguous.
- `flatten` stands alone (no rename, default or conversion), takes a type with named fields,
  and is refused beside `deny_unknown_fields`.
- `as_text` applies per item to `Vec<T>` and `Option<Vec<T>>` (`T: Display + FromStr`).
- `with = path` names a module with `const TYPE: &str`, `to_text(&T) -> String` and
  `from_text(&str) -> Result<T, LeafError>`; the field is a leaf of that type. It cannot use a generic parameter of the type it sits in, and is
  refused on an `Option`.
- A setting whose setter name (the field, or `setter = ".."`) is `build`, `parts`,
  `fill`, `fill_from`, `builder`, `new`, `configured`, `non_defaults`, `partial_mut`, `into` or `try_into` is refused.
- `skip_serializing_if` and `alias` are refused: nothing is written, and a description has
  no alias.

## `#[derive(Describe)]` on a struct: typed setters

A struct of named fields also gets, beside the struct and with its visibility,
`pub trait <Name>Setters<..>: Holds<Name<..>> + Sized` with one method per setting that is not
`#[map(skip)]`, named by the field or `#[builder(setter = "..")]` (a keyword is written raw),
`fn name(self, value: impl Into<FieldType>) -> Self`, carrying the field's doc comment. A field
written as a primitive scalar (`bool`, `char`, `u8`..`u128`, `usize`, `i8`..`i128`, `isize`, `f32`,
`f64`; one segment, no generics) takes `FieldType` itself, so `.max(8)` infers; an alias, a
path such as `std::primitive::u32`, an `Option<u32>` and every other type keep `impl Into`. A
`#[map(flatten)]` field has one setter taking its whole type. `impl<B: Holds<Name>> <Name>Setters
for B` is written beside the other impls. Two settings with one setter name are refused. A field
behind a false `cfg` has none.

## `#[derive(Builder)]`

```rust
#[proc_macro_derive(Builder, attributes(builder))]
```

On a parts struct with named fields: exactly one field without `#[builder(input)]` (the
settings, `Describe`) and any number with it (`setter = ".."` renames the method). On the
struct `#[builder(build = X[, name = ".."][, fallible][, entry = "new"][, crate = path])]`:

- `build = X` is the type the parts build, by `From<Parts> for X` or, with `fallible`,
  `TryFrom<Parts> for X`. The builder is named `XBuilder` after the last segment of `X`, with
  the parts struct's visibility; `name = "ServerBuilder"` names it instead and is **required**
  when `X` has generic arguments (`Built<Config>`). A type this crate does not own needs `name`
  (if generic) and `entry = "new"`, since an inherent `X::builder()` is E0116.
- `entry = "new"` makes the builder with `XBuilder::new()` instead of `X::builder()`, and is
  required when the parts struct has generic parameters or lifetimes.
- An input whose type is exactly one of the parts struct's type parameters takes that type itself
  (`fn channel(self, value: C)`), so setting it infers the parameter; every other input takes
  `impl Into<T>`, an `Option<C>`, a `Vec<C>` and a `C::Item` included, and those still need the
  parameters named when nothing else fixes them. An input written as a primitive scalar takes it
  as itself too (the list is the settings setters').
- An input of type `Option<J>` takes `impl Into<J>` (`J` itself when `J` is a primitive scalar) and is never missing; any other input is
  named in `Missing::inputs` while unset.
- An input setter that is one of `build`, `parts`, `fill`, `fill_from`, `builder`, `new`,
  `configured`, `non_defaults`, `partial_mut`, `into` or `try_into` is refused; one equal to a setter of the settings
  fails the build of the builder (E0080); two inputs with one setter are refused.
- The builder has `fill`, `fill_from` (the `Unknown` keys are dropped), `build`, `parts`
  (`Result<Parts, Missing>`: `build` is `parts` and then the conversion), `From<Partial<S>>`,
  `Holds<S>`, `Configured`, `Check<XBuilder> for Profile`, an inherent `non_defaults` and a
  `Debug` that shows which inputs are set and never a value.
- Zero or two settings fields, a missing `build`, an unknown key, a name that is no identifier
  and a generic `X` without `name` are refused with a sentence.

## `#[derive(Configured)]`

```rust
#[proc_macro_derive(Configured, attributes(configured))]
```

On a struct with exactly one `#[configured]` field (the settings, `Describe + Inspect`): writes
`Configured` (the field is the view), `Check<Self> for Profile` and an inherent `non_defaults`.
`#[configured(crate = path)]` on the type says where `coqui_builder` is.

## Gotchas

- The derive sees the item after `cfg` and `cfg_attr` are applied; a field behind a false
  `cfg` is no key.
- Every type parameter of a generic struct is bound by `Describe + Inspect`.
- Indirect recursion (A holds B holds A) is not detected.
- An untagged enum that holds a value is `Describe` but not `Leaf`: it takes no `none = ".."`
  as an `Option` field, and `assert_round_trip` does not apply.
- Setting an untagged enum's text after setting keys of its structured arm forgets those keys;
  a default the structured arm was given stays.
