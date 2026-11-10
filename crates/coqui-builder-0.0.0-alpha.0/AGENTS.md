# `coqui-builder` — API header

Header-file-style reference for the `coqui-builder` crate, which describes, fills, checks and
builds any struct field by field: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-builder>.

| Feature | Adds | Default |
| --- | --- | --- |
| `std` | `PathBuf` and `HashMap` as settings, and `Env` | yes |
| `derive` | `#[derive(Describe)]`, `#[derive(Leaf)]`, `#[derive(Builder)]` and `#[derive(Configured)]` (`coqui-builder-derive`: `syn`, `quote`, `proc-macro2`) | no |
| `zeroize` | `Secret<T>`; a sensitive `Datum`, the text a source hands over for a sensitive place and the values of an `Env` are wiped on drop (`zeroize`) | no |
| `serde` | `Deserialize` for `Partial<T>` (`serde`, with its dependency-free `serde_core`) | no |

MSRV 1.89, edition 2024. `#![no_std]` with `alloc`. A default build has no dependencies.
The crate holds no `unsafe` (`#![forbid(unsafe_code)]`), no `static`, no logger, and never
reads or sets an environment variable except `Env::new`, which reads the process
environment once.

The root re-exports the common task: `Describe` and `Leaf` (traits, and the derives with
`derive`), `Description`, `Fields`, `Datum`, `Shown`, `KeyPath`, `Segment`, `PathError`,
`Fill`, `FillError`, `Partial`, `Missing`, `Source`, `SourceError`, `Unknown`, `Entries`,
`Env` (`std`), `Secret` (`zeroize`), `Inspect`, `Report`, `Finding`, `Reason`, `Check`, `Checks`, `Profile`,
`Expect`, `BindError`, `NonDefault`, `non_defaults`, `differences`, `Configured` (and, with
`derive`, `Builder`). The modules `describe`, `fill`, `path`, `datum`, `partial`, `source`,
`env`, `inspect`, `report`, `check`, `profile`, `non_default`, `builder` and (`zeroize`) `secret` are public; the
others' items are reached there (`builder::Holds`, `builder::BuildError`).

## The idea

Describe, fill, check and build any struct field by field: builders, partials and profiles,
with no dependencies. Settings are the main example. A struct (a settings type, say) derives
`Describe`. That gives an owned `Description` (what each key is), a
`Fill` (the type partly filled, held by a `Partial`) and an `Inspect` impl (read a key of a
value). There is no value model: a setting is one text, read and written by its leaf's own
parser (a `Datum` is that text with its type's name), and nothing is filled from a whole group.

## Paths (`coqui_builder::path`)

```rust
pub enum Segment { Key(String), Index(usize) }
pub struct KeyPath;           // FromStr, Display, Default (the root), Hash, Ord
impl KeyPath {
    pub fn root() -> KeyPath;
    pub fn new(segments: Vec<Segment>) -> KeyPath;
    pub fn segments(&self) -> &[Segment];
    pub fn len(&self) -> usize;  pub fn is_empty(&self) -> bool;
    pub fn push(&mut self, segment: impl Into<Segment>);  pub fn pop(&mut self) -> Option<Segment>;
    pub fn child(&self, segment: impl Into<Segment>) -> KeyPath;
    pub fn starts_with(&self, prefix: &[Segment]) -> bool;
}
impl Segment { pub fn key(..) -> Segment; pub fn name(&self) -> Cow<'_, str>;
               pub fn as_key(&self) -> Option<&str>; pub fn as_index(&self) -> Option<usize>; }
pub enum PathError { Empty, EmptyField{at}, EmptyIndex{at}, UnclosedIndex{at}, UnclosedQuote{at},
                     BadEscape{at}, UnexpectedClose{at}, TrailingQuote{at}, MissingSeparator{at} } // non_exhaustive
```

Grammar: `a.b`, `a[0]` (a position), `a[name]` (a map key), `a["x.y"]` (a quoted key, `\"`
and `\\` escaped). Rootless; `a[0]b` and `a.[0]` are refused. Digits without a leading zero
are a position (`[01]` is the key `01`). An open map reads a position as the key of its
decimal text. `Display` writes the spelling that parses back: `waiting.per_source`,
`env["a.b"]`, `["1"]` for the key `1`.

## Data (`coqui_builder::datum`)

```rust
pub struct Datum;   // a type name and a text; Debug, Clone, PartialEq, Display
impl Datum {
    pub fn new(ty: &'static str, text: impl Into<String>) -> Datum;
    pub fn ty(&self) -> &'static str;   pub fn text(&self) -> &str;
    pub fn is_sensitive(&self) -> bool; pub fn sensitive(self) -> Datum;   // mark as a secret
}
pub enum Shown { Unset, One(Datum), Items(Vec<Datum>), Fields(Vec<(String, Datum)>) }
impl Shown { pub fn sensitive(self) -> Shown; }   // what a finding or non-default holds
```

A datum is never a list or a record: every leaf parses and prints its own text. `Display` is
`Type(text)` (`u32(64)`, `Duration(30s)`); a secret is `Type(***)`, in `Display` and in
`Debug`. `Shown` prints `unset`, `Type(text)`, `[Type(a), Type(b)]` or, for the structured arm
of an untagged enum between differing sides, `{ttl: Duration(30s), size: u32(8)}`.

## Describing (`coqui_builder::describe`)

```rust
pub trait Describe: Sized {
    type Fill: Fill<Self>;
    const OPTIONAL: bool = false;           // true for Option<_>
    const LEAF: bool = false;               // one text: a Leaf, or an untagged enum of texts
    fn description() -> Description;        // owned, built on each call
}
pub struct Description;  // kind(), kind_mut(), notes(), notes_mut(), resolve(&[Segment]),
                         // resolve_chosen(&[Segment], &dyn Fn(&[Segment]) -> Option<String>),
                         // knows(..), default_at(..), check_text(&str) -> Result<(), Bound>,
                         // check_items(usize), group_fields(), into_group_fields();
                         // built by new(Kind), leaf::<T>(), leaf_of::<T, C>(), list(..),
                         // map(..), group(fields), variant(..), union(text, structured)
pub enum Kind { Leaf(LeafInfo), List(Box<Description>), Map(Box<Description>), Group(Group),
                Variant(Variant), Union(Union) }                            // non_exhaustive
pub struct Union;      // text() -> &LeafInfo, structured() -> &Description,
                       // default_is_structured(); built by new(text, structured)
pub struct LeafInfo;   // ty(), choices(), labels(), label(&str), is_ordered(), check(&str),
                       // eq(&str, &str), order(&str, &str); of::<T>(), of_codec::<T, C>()
pub struct Field { key, required, optional, default: Option<String>,
                   default_items: Option<Vec<String>>, none: Option<String>, description }
pub struct Notes { title, description, sensitive, read_only, deprecated, examples, section, order,
                   advanced, stricter: Option<Stricter>, min_length, max_length, pattern, format,
                   multiple_of, min_items, max_items }                       // non_exhaustive
pub enum Stricter { Higher, Lower }
```

A `Union` is an untagged enum with a text side (the unit names and the single-value arms, one
leaf named after the enum) and one structured arm (a group, list, map or tagged variant), both
at the enum's own path. A key below the enum is in the structured arm; `resolve` reports a
`Gate` with `is_union()` for it.

A `Group` holds `fields` and `deny_unknown_fields`. A `Variant` holds a `tag`, `arms` (each an
`Arm` with its own `fields`) and a default arm; the tag is itself a key, a `String` leaf.
`resolve` answers a `Resolved`: the description at a place, its field, its default text or
default items, and the `Gate`s (optional groups and variant arms) above it. A key several
arms share reads as the default arm's field; `resolve_chosen` reads it from the arm its
callback names for the variant's tag (a `Partial` passes the arm it holds). `Field::new`,
`Field::default_value` and `Field::default_from` build a field by hand. Defaults are
texts, so `Field::default` is `"30s"`, not a value.

`Leaf` is a type that is one setting, as one text:

```rust
pub trait Leaf: Sized {
    const TYPE: &'static str;                     // the name a Datum shows
    fn to_text(&self) -> String;
    fn from_text(text: &str) -> Result<Self, LeafError>;
    const ORDERED: bool = false;                  // order() answers Some
    fn order(a: &Self, b: &Self) -> Option<Ordering> { None }
    fn same(a: &Self, b: &Self) -> bool;          // default: canonical texts are equal
    fn choices() -> Option<&'static [&'static str]> { None }   // a unit-only enum's names
    fn labels() -> &'static [(&'static str, &'static str)] { &[] }   // (text, label) for forms
}
pub fn assert_round_trip<T: Leaf + Debug>(values: &[T]);
pub trait Codec<T> { const TYPE; fn to_text(&T); fn from_text(&str); .. }   // for `with`, `as_text`
```

**Contract**: `from_text(&x.to_text())` is `x`. **Equality** is parsed, then compared by
canonical text: two texts are equal when both parse into the type and the two values write
the same text (`30s` equals `30000ms`, `0.5` equals `0.50`); the raw input is never compared.
A type whose equal values can write different texts overrides `same` (`f32` and `f64` do, so
`-0` equals `0`). Every `Leaf` is `Describe` and `Inspect`; `#[derive(Leaf)]` writes the impl.

Implemented for `bool`, the integers up to `u128`/`i128`/`isize`/`usize`, `f32`, `f64`,
`String`, `char`, `Duration`, the `core::net` addresses and (`std`) `PathBuf`. There is no
bytes leaf: hold bytes as text with `#[map(with = ..)]`.

Reading rules: an integer is held to its type's range and reads no fraction; a float is
finite; a boolean's text is `1 true yes on` and `0 false no off`, any case; a duration reads
`30`, `0.25`, `30s`, `250ms`, `1.5h` (units `h m s ms us ns`, a number without a unit is
seconds) exactly, refuses a negative, a non-number and anything finer than a nanosecond, and
is written exactly as seconds if whole, else milliseconds, microseconds, nanoseconds
(`30s`, `250ms`). `[secs, nanos]` is not read. `LeafError` never quotes its input.

## Filling (`coqui_builder::fill`, `partial`)

```rust
pub trait Fill<T>: Default + Inspect {
    fn set(&mut self, at: &[Segment], text: &str) -> Result<(), FillError>;
    fn set_items(&mut self, at: &[Segment], items: &[String]) -> Result<(), FillError>;
    fn clear(&mut self, at: &[Segment]) -> Result<(), FillError>;
    fn set_default(&mut self, value: T);
    fn build(self, at: &KeyPath, missing: &mut Vec<KeyPath>) -> Option<T>;
    fn from_value(value: T) -> Self;
}
```

Slots: `LeafSlot<T, C = Native>`, `OptionSlot<T, F>` (`.none(ty, text)` sets the text that
writes `None`), `ListSlot<T, F>`, `MapSlot<M, V, F>` (`M: TextMap<V>`: `BTreeMap`, `HashMap`),
`Wrapped<T: Wrap, F>`. `FillError` has a relative `at()` path and a `kind()` (`FillErrorKind`:
`UnknownKey`, `Path`, `Leaf`, `Bound`, `Unsupported`, `Source`, `UnknownKeys`); a slot reports
an unknown key and the caller prepends its own segment with `under`.

```rust
pub struct Partial<T: Describe>;
impl<T: Describe> Partial<T> {
    pub fn new() -> Self;                                       // Default
    pub fn set(&mut self, key: &str, text: &str) -> Result<(), FillError>;
    pub fn set_items<S: AsRef<str>>(&mut self, key: &str, items: impl IntoIterator<Item = S>)
        -> Result<(), FillError>;                               // replaces a whole list
    pub fn clear(&mut self, key: &str) -> Result<(), FillError>;
    pub fn set_at / set_items_at / clear_at(.., at: &[Segment], ..);   // the same, split
    pub fn overlay(&mut self, later: Partial<T>);               // only the keys `later` set
    pub fn build(self) -> Result<T, Missing>;                   // every missing key at once
    pub fn description(&self) -> &Description;
    pub fn fill_from(&mut self, source: &mut impl Source) -> Result<Unknown, FillError>;
}
impl<T: Describe> From<T> for Partial<T>;                       // every key set
impl<T: Describe> Inspect for Partial<T>;                       // set keys, else the defaults
```

`set` is text for every leaf; the leaf's own parser reads it, so a duration key takes `30s`.
At an untagged enum, `set` with a path of its own sets the text side; a key below it, or
`set_items` (an empty list chooses a group), switches to the structured arm and drops a text,
and setting the text again forgets the structured keys (a default given to that arm stays).
`clear` of the enum forgets both sides.
It also checks the bounds the description states (`min_length`, `max_length`, `min_items`,
`max_items`, `multiple_of`). A failed `set` changes nothing. A list position more than 1024
past the end is refused. `clear` on an optional group forgets that it was given; an
`overlay`ed list replaces the list; an open map is merged key by key.

`Partial::put<V: Describe + 'static>(&mut self, key, value: V)` (and `put_at`) sets a typed
value at any path, through a hidden typed `Fill` method: `put("waiting.total", 5u32)`,
`put("peers[1]", peer)`, an untagged enum's arm by the type given, an `Option` place by the
value or an `Option` of it. A value of another type, or an unknown key, is a `FillError` and
changes nothing; the text bounds of the description (`min_length`, `multiple_of`, ..) are not
checked, as no text is written. `Fill::put(value)` sets every key from a value and keeps the
defaults the fill was given. Because `put` skips the bounds, `overlay` (and so `fill`) re-sets
each leaf through its text and drops one that breaks a bound: a `Partial` filled by `put` past a
bound builds as it is but is not carried whole by `fill`.

## Builders (`coqui_builder::builder`, `#[derive(Builder)]`)

```rust
pub trait Holds<S: Describe> { fn partial_mut(&mut self) -> &mut Partial<S>; }
pub enum BuildError<E> { Missing(Missing), Failed(E) }   // non_exhaustive; Display is the inner's
impl Missing { pub fn new(keys, inputs: Vec<&'static str>) -> Missing; pub fn keys(&self);
               pub fn inputs(&self) -> &[&'static str]; pub fn is_empty(&self) -> bool; }
```

`Missing` displays `missing settings: a, b.c; missing inputs: identity` (an input is named by
its field, not by a renamed setter); a part with nothing
omitted is left out, and only names are written. `BuildError::source` is the conversion's own
source, so a chain does not print the error twice.

**Settings setters.** `#[derive(Describe)]` on a struct writes `pub trait <Name>Setters` (the
struct's visibility): one `fn <setter>(self, value: impl Into<FieldType>) -> Self` per setting (a field written as a
primitive scalar, `bool`, `char`, the integers, `f32`, `f64`, takes `FieldType` itself so a literal
infers; an alias of one keeps `impl Into`), the last call winning, implemented for every `B: Holds<Name>`. Import it to call them (not needed on a builder the settings name with `setters_on`), from any
crate (a settings crate re-exports it in its prelude). A setter that is `build`, `parts`,
`fill`, `fill_from`, `builder`, `new`, `configured`, `non_defaults`, `partial_mut`, `into` or `try_into`
is refused, since an inherent method silently wins over a trait method; rename it with `#[builder(setter = "..")]`
(the key stays the field's). A key below the top level has no setter: a hand-written method
calls `partial_mut().put("a.b", value)`.

**The builder.** On a parts struct, `#[derive(Builder)]` with `#[builder(build = X[, name =
".."][, fallible][, entry = "new"][, crate = path])]` writes `XBuilder` (`name` replaces that
name and is required for an `X` with generic arguments; a foreign `X` needs `name` when
generic and `entry = "new"`):

```rust
X::builder() / XBuilder::new()                          // nothing set
fn <input>(self, impl Into<I>) -> Self                  // `Option<J>` inputs take impl Into<J> (J itself if primitive); an input
                                                        // that is a type parameter C or a primitive scalar
                                                        // takes its type itself
fn fill(self, Partial<S>) -> Self                       // Partial::overlay: the last write wins
fn fill_from(self, &mut impl Source) -> Result<Self, FillError>   // drops `Unknown`
fn build(self) -> Result<X, Missing>                    // fallible: Result<X, BuildError<E>>
fn parts(self) -> Result<Parts, Missing>                //  what build converts; the same Missing
impl From<Partial<S>> for XBuilder                      // equals X::builder().fill(p)
```

`fill_from` drops the keys the source holds that the settings lack; to see them, fill a
`Partial` with `Partial::fill_from` and pass it to `fill`. `build` lists every missing key and
every unset required input at once, then calls `From<Parts>` (or `TryFrom`); it is `parts()`
followed by that conversion. A second terminal (an `async fn build_async`, a `configs()` giving
two values) is written by hand over `parts()`, which gives the parts struct with the settings
built, the required inputs present and the optional ones as given, or the same `Missing`. A
builder is not `Clone`: its inputs need not be. Layers run in the
order called: a file, the environment, flags, then setters. The builder has the parts struct's
visibility: a `pub` built type whose builder other crates must name needs a `pub` parts struct.
`entry = "new"` also writes `Default`. `Check::verify` is not available on a
builder (it is not `Describe`): make `Report::new(&S::description())` and call `check`.

## Configured (`coqui_builder::Configured`)

```rust
pub trait Configured {
    type Settings: Describe;
    fn configured(&self) -> &dyn Inspect;
    fn non_defaults(&self) -> Vec<NonDefault> { .. }
}
impl Profile { pub fn judge(&self, description: &Description, subject: &dyn Inspect, report: &mut Report); }
```

A builder is `Configured` (its view is the `Partial`, so an unset key reads as its default); so
is a built type that derives `Configured` with `#[configured]` on its settings field. Each
derive also writes `impl Check<Self> for Profile` (through `judge`; a blanket impl would overlap
`Check<T: Describe + Inspect>`, and so would a built type that is itself `Describe + Inspect`: it
gets E0119, derive `Configured` only on a type that is not) and an inherent `non_defaults()` so no import is needed. A
builder's `non_defaults()` and its built value's are equal.

## Sources (`coqui_builder::source`, `env`)

```rust
pub trait Source {
    fn value(&mut self, at: &KeyPath, leaf: &LeafInfo) -> Result<Option<String>, SourceError>;
    fn entries(&mut self, at: &KeyPath) -> Result<Option<Entries>, SourceError>;
    fn unknown(&mut self, known: &dyn Fn(&KeyPath) -> bool) -> Vec<KeyPath>;
}
pub enum Entries { Len(usize), Keys(Vec<String>) }              // non_exhaustive
```

`fill_from` walks the description: `value` per leaf, the tag before an arm (only the chosen
arm's fields are asked for), `entries` per list (`Len`) and open map (`Keys`), then each
element by its own path. At an untagged enum with a structured arm it asks `entries` and `value`
at the enum's key: a value is the text side, entries the structured side (an empty table still
chooses it), both is a `SourceError` naming the key, and neither walks the structured arm for
keys below. A source holding a table there answers `entries`; its refusal of `value` on a table
is not a failure. A list the source answers by `Len` is replaced whole. Keys the
source holds that the description lacks are returned in `Unknown`, and refused
(`FillErrorKind::UnknownKeys`, after everything else was filled) under a group with
`deny_unknown_fields`. The first `SourceError` or leaf refusal stops the fill. Precedence is
the order of the calls: the last write of a key wins.

`Env::new("APP")` and `Env::from_vars("APP", iter)` (`std`): a path maps to `APP_A__B`:
segments upper-cased with `-` as `_`, `__` between segments, positions as digits, open-map
keys as found (`APP_CACHE=false` is an untagged enum's text, `APP_CACHE__TTL=30s` its
structured arm; both set is refused). The prefix is matched as written; the rest of a name in any case. An empty
variable is unset. A list position of 65535 or more is ignored, and a gap of more than 1024
past the end of a list refuses the fill. A variable whose leaf was read is never reported as
unknown, whatever its spelling. `Env::name(&KeyPath)` gives the variable of a leaf. A list
whose positions are not all given (`0` and `2` but no `1`) is refused. `Debug` shows the
prefix and the count, never a value.

## Secrets (`coqui_builder::secret`, feature `zeroize`)

```rust
pub struct Secret<T: Zeroize>;   // Clone, PartialEq, Eq where T is; Debug is `Secret(***)`; no Display
impl<T: Zeroize> Secret<T> { pub fn new(T) -> Self; pub fn expose(&self) -> &T; }   // From<T>
pub struct SecretCodec<T>;       // the Codec a Secret<T> is filled through (`SENSITIVE = true`)
```

For `T: Leaf + Zeroize` a `Secret<T>` is a setting of its own (it implements `Describe` and
`Inspect` by hand, so it is not a `Leaf`): one text, the type of `T`, sensitive in the
description, and every datum read from it is marked sensitive (`String(***)`). `Codec` has a
`SENSITIVE` constant for the same effect on a hand-written `LeafSlot<T, C>` with
`Description::leaf_of`; a field converted by `#[map(with = ..)]` needs `#[schema(sensitive)]`. `Secret` exists only with the
feature, so a `Secret` field does not compile without it; `#[schema(sensitive)]` needs no
dependency and masks only.

**What is wiped**, with `zeroize`, when the owner drops: a `Secret`'s value; the text of a
sensitive `Datum`; the text a `Source` handed over for a place the description marks
sensitive (or below one), as soon as `fill_from` has parsed it; the values an `Env` holds; the
texts of a `Partial` read through serde. **What is not**: a plain `String` under
`#[schema(sensitive)]` (hold a `Secret<String>`), the text a caller passes to `Partial::set`,
a copy taken with `Datum::text` or `Secret::expose`, the copies of a value's text that
`non_defaults`, `differences`, a `Profile` and its `Expect` texts make while comparing, a
default kept as text in a description, the process environment itself, the
buffers a file format's parser keeps, and a buffer a growing `String` left behind when it
reallocated. Without the feature everything is masked, nothing is wiped. `PartialEq` is not
constant-time.

## From serde (feature `serde`)

`impl<'de, T: Describe> Deserialize<'de> for Partial<T>`: the input is collected into a
private tree, and a source over it runs `Partial::fill_from`, so lists (replaced whole),
untagged enums, a tag written after its arm's keys, `none` texts and unknown keys follow
"Sources" exactly as they do for `Env`. Keys are matched exactly, with no case folding, and an
empty string is a value (unlike an empty variable).

- A scalar is its text: a string as it is, a boolean `true`/`false`, an integer its decimal
  digits, a float the shortest text that reads back (`0.25`; `30.0` is `30`). A format that
  hands a wider integer than 64 bits as a float loses digits first: write it as a string. A
  unit or `None` (a JSON `null`) is a key that is not given.
- Shapes are checked before the fill: a group, variant or open map takes a table (`expected a
  table at waiting`), a list takes a list or a table of positions (`expected a list at ports`),
  an untagged enum takes a value or a table, and a leaf takes a value (`expected a single value
  at waiting.total`; `at the root` for the root). The root follows the same rules. A TOML
  datetime is not a scalar here: write a date as a string.
- Unknown keys are skipped, or refused under `deny_unknown_fields` (the error lists them).
- Every error this impl writes is `Error::custom` with the `FillError` text or a sentence like
  the above, and never quotes the input, a sensitive value included. A format's own syntax
  error is the format's, and may show the line it failed on.
- A builder takes the result: `Builder::from(toml::from_str::<Partial<S>>(text)?)` or
  `.fill(partial)`.

## Reading a value (`coqui_builder::inspect`)

```rust
pub trait Inspect {
    fn at(&self, at: &[Segment]) -> Option<Datum>;             // a leaf
    fn entries(&self, at: &[Segment]) -> Option<Entries>;      // a list's Len, a map's or group's Keys
}
```

Implemented for every `Leaf`, `Option`, `Vec`, `BTreeMap<String, _>`, `HashMap<String, _>`,
a derived type, a `Fill` and a `Partial`. `at` answers only a leaf; a group, list or map
answers through `entries`. A `None` is absent for both. A group's keys are the ones set.
Custom checks read values through this trait.

## Reports and checks (`coqui_builder::report`, `check`, `non_default`)

```rust
pub struct Report;    // new(&Description), push(Finding), findings(), unmet(), is_met(), len()
pub struct Finding;   // new(key, met, value: Shown, expected: Expect, reason); accessors
pub enum Reason { Differs, Missing, Present, Below, Above, NotOneOf, Looser, NotSubset,
                  ArmNotAllowed, UnknownKey, NoOrdering, NotApplied, Custom(String) } // non_exhaustive
pub trait Check<T: ?Sized> {
    fn check(&self, subject: &T, report: &mut Report);
    fn verify(&self, subject: &T) -> Report where T: Describe;
}
pub struct Checks<T: ?Sized>;   // push(impl Check<T> + 'static), len(), is_empty()
```

`Report::push` marks as secret, and masks the expectation of, every finding at a place the
description marks sensitive (through list items, open-map values and arms). `Check` is
implemented for `Profile`, `Fn(&T, &mut Report)`, `[C]` and `Vec<C>` of checks and `Checks<T>`.

```rust
pub fn non_defaults<T: Describe + Inspect>(value: &T) -> Vec<NonDefault>;
pub fn differences(description: &Description, value: &dyn Inspect,
                   baseline: Option<&dyn Inspect>) -> Report;
pub struct NonDefault { pub setting: String, pub value: Shown, pub default: Shown }
```

`differences` walks a group's fields, an open map's entries and a variant's fields (when
both sides are the same arm). An untagged enum is its text leaf when both sides hold text, its
structured arm when both hold it, and one place at the enum otherwise (the text as
`Type(text)`, the structured side as `Shown::Fields`, expecting `Equal(text)` or, from a
structured baseline, `Required`). A leaf is compared as typed values. A list of leaves is one
place, shown whole as `Shown::Items`. A switched arm is one place at its tag; a group present
on one side only is one place at the group with no value. A key left out reads as its default.
Findings come in the order of their path segments as text.

## Profiles (`coqui_builder::profile`)

```rust
pub enum Expect { Equal(String), Within{min: Option<String>, max: Option<String>}, OneOf(Vec<String>),
                  AsStrictAs(String), NoStricterThan(String), SubsetOf(Vec<String>),
                  ItemsEqual(Vec<String>), Required, Absent, Arms(Vec<String>), Any }  // non_exhaustive
pub struct Profile;   // new(), expecting(key, Expect) -> Result<Profile, PathError>,
                      // insert/remove/get/entries/len/is_empty, bind::<T>() -> Result<(), BindError>,
                      // defaults::<T>(), against(&T), merge(&Profile) -> Profile
```

An expectation holds texts, read by the field's own parser when the profile is checked, so
`Equal("30s")` meets a duration of 30000 ms. A text the field refuses is a `Reason::Custom`
finding, and `bind::<T>()` refuses it up front with a `BindError` (key and reason). `Equal`
and `OneOf` are for a leaf, `ItemsEqual` and `SubsetOf` for a list of leaves, `Arms` for a
variant. Keys are unique. `AsStrictAs` and `NoStricterThan` follow the setting's `stricter`
order (`#[builder(stricter = "higher" | "lower")]`) and rank an unset optional (or its `none`
text) loosest; a setting with no order is `NoOrdering`. `Required` and `Absent` judge a leaf
or an optional group. An expectation on an optional group or a variant gates the keys below
it. A key the type lacks is `Reason::UnknownKey`. `merge` takes the later per key, keeps the
rest, and `Any` clears. `defaults` and `against` write one expectation per leaf (`ItemsEqual`
per list of leaves); a sensitive leaf is `Required` and a sensitive list is `Required`, never
a copy, and everything below a sensitive group, map or list is too, so a profile holds no
secret. `against` writes an `Equal` per entry of an open map; `defaults` writes nothing for
one. The text that writes an optional's `None` (`none = ".."`) is a value `Equal`, `OneOf`
and `against` understand. `Arms` names must be arms of the variant it is bound to.

`Expect` and `Profile` are `Describe` and `Inspect` by hand, held equal to a derived twin by
the tests, so a profile can be read from a file or a form like any other settings type.

## The derive (`#[derive(Describe)]`, `#[derive(Leaf)]`, feature `derive`)

A struct of named fields is a group; a newtype or `#[map(transparent)]` struct is its field's
setting; an enum of unit variants is a `Leaf` with choices; `#[map(tag = "k")]` makes a variant
of arms; `#[map(untagged)]` text arms (unit names, then each single-value `Name(T)` in order) and at
most one structured `Name(T)` (a group, list, map or tagged enum): see the derive's header. A
second structured arm fails the build of the enum.
A struct may be generic over types; a generic enum, a lifetime, a tuple struct of two or
more fields, a unit struct, a union, an untagged variant with named fields and a type that
holds itself are refused with a sentence (the derive's header lists every refusal). In an
untagged enum an `Option` arm, and an enum with no unit variant and no single-value arm,
fail the build of the enum (E0080).

`#[derive(Leaf)]` on a unit-only enum reads and writes the variant names (`rename`,
`rename_all`); on any other type it uses `Display` and `FromStr`. `#[leaf(name = "..")]`
names the type a datum shows, `#[leaf(ordered)]` takes the order from `PartialOrd`,
`#[leaf(display)]` makes a unit-only enum use `Display` and `FromStr` too.

| Attribute | On | Meaning |
| --- | --- | --- |
| `#[map(rename = "x")]`, `#[map(rename_all = "..")]` | field, variant / type | the key; `lowercase`, `snake_case`, `kebab-case`, `SCREAMING_SNAKE_CASE` |
| `#[map(skip)]` | field | not a key; built from `Default` |
| `#[map(default)]`, `#[map(default = expr)]`, `#[schema(default = expr)]` | field | the default; the key is not required |
| `#[map(flatten)]` | field | the field's keys sit beside the others (`Fields` is required) |
| `#[map(transparent)]`, `#[map(deny_unknown_fields)]` | struct | as named |
| `#[map(tag = "k")]`, `#[map(untagged)]`, `#[map(other)]` | enum, variant | arms; held values; the catch-all `Other(String)` |
| `#[map(as_text)]` | field | `Display` out, `FromStr` in; per item for `Vec<T>` and `Option<Vec<T>>` |
| `#[map(with = path)]` | field | a module with `TYPE`, `to_text(&T)`, `from_text(&str)`; the field is a leaf |
| `#[map(none = "text")]` | `Option` field | the text that writes `None`; needs `default = expr`; the inner type must not read that text |
| `#[schema(title, description, section, order, advanced, sensitive, read_only, deprecated, examples(..))]` | field | notes; a doc comment is the description |
| `#[schema(min_length, max_length, pattern, format, multiple_of, min_items, max_items)]` | field | bounds |
| `#[schema(title, description)]`, `#[schema(label)]` | arm, unit variant | as named; a unit variant's `label` (else its doc comment) is in `LeafInfo::labels`, also on an untagged enum's unit variants |
| `#[builder(stricter = "higher" \| "lower")]`, `#[builder(setter = "name")]` | field | the order; the setter's method name |
| `#[builder(crate = path)]` | type | where `coqui_builder` is re-exported |
| `#[builder(setters_on = Type)]`, `#[builder(setters_on = <'a, C: Tr> Type<'a, C>)]` | struct with setters | repeatable; an inherent `impl` on that builder with every setter (same names, docs and argument types as the trait's, each delegating to it), so a caller needs no import. The impl must be in the builder's crate (a foreign builder is E0116); the trait stays for those. Refused on an enum, a newtype, a transparent struct and a generic settings type |

`#[map(crate = ..)]`, guatiao's spelling, is read and ignored. A setting whose setter would
be `build`, `parts`, `fill`, `fill_from`, `builder`, `new`, `configured`, `non_defaults`,
`partial_mut`, `into` or `try_into` is refused; rename the setter with `#[builder(setter =
"..")]` (the key stays).

## Errors

`PathError`, `LeafError`, `FillError`, `Missing`, `SourceError` and `BindError` implement
`Display` (one lowercase clause, never the input; `BindError` prints its reason with
`Debug`) and `core::error::Error`. `Missing` lists
every key a `build` could not make. All are `#[non_exhaustive]` where they may grow.

## Environment variables

Only `Env::new(prefix)` reads any: the variables of this process that start with
`<prefix>_`, once, when it is called. This crate sets none.

## Gotchas

- Indirect recursion (A holds B holds A) is not detected and overflows the stack when the
  description is built; a direct self-reference is refused by the derive.
- `pattern` and `format` are carried in the description for forms and schemas and are never
  enforced here.
- Bounds are checked when a key is set, not when a value is built; a list's `min_items` is
  not seen by setting its items one at a time, only by `set_items`.
- The derive sees the item after `cfg` stripping; a field behind a false `cfg` is no key.
- A `with` module cannot use a generic parameter of the struct it sits in.
- `Partial` caches its type's description when it is made.
- A profile made by `defaults` says nothing about the entries of an open map.
- `Partial::overlay` of a `Partial::from(value)` carries the `None` of an optional: with a
  `none` text it lays that text over, without one it forgets what the target held. A `None`
  at a list position or as a map entry is not carried (clearing a position would shift its
  neighbours, and an entry has nothing to forget).
- The default of an open map or of a list of groups is built but not described: the
  description records defaults as texts of leaves and lists of leaves, so `non_defaults`
  of such a default lists its entries.
- A `with` module is a text converter only; it declares no order, so `Within`,
  `AsStrictAs` and `NoStricterThan` cannot bind to its field.
- A flattened field that repeats a key of the outer struct panics when the description is
  built, in a debug build (a debug assertion naming the key); a release build keeps both.
- `Env` matches a variable to a key by reading its segments in lower case, with `_` as `-`,
  and as found; a field name that mixes `_` and `-` is not matched.
