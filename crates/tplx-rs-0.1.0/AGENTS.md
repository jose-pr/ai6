# `tplx-rs` — API header

Header-file-style reference for the `tplx-rs` crate: every public item with
its signature, contract and gotchas, so the crate can be used without
reading its source. It ships inside the package and is self-contained.
Development documentation lives with the source at
<https://github.com/jose-pr/tplx-rs>.

| Feature | Adds | Default |
| --- | --- | --- |
| `std` | `std::error::Error` for the error types; without it they implement `core::error::Error` | yes |

MSRV 1.81. The crate is `#![no_std]` with `alloc`, holds no `unsafe` (`#![forbid(unsafe_code)]`), does no I/O, keeps
no global state, and builds for `wasm32-unknown-unknown` with `--no-default-features`. Its dependencies are
`serde_json` and `thiserror`.

Everything is imported from the crate root: `render`, `Engine`, `render_raw`, `render_envsubst`, `render_mustache`,
`Options`, `Limits`, `Resolver`, `EnvsubstOptions`, `Dialect`, `Pattern`, `Feature`, `Features`, `Undefined`,
`MustacheOptions`, `Escape`, `Error`, `SyntaxKind`, `Limit`, `Value` (`serde_json::Value`). There are no public
modules. An engine is limited by its options, never rewritten: each is the language as it is written elsewhere.

## Rendering by name (`tplx_rs`)

```rust
pub fn render(engine: &str, text: &str, data: &Value, options: &Options<'_>) -> Result<String, Error>
```

Renders `text` with the engine called `engine`. `Err(Error::UnknownEngine)` when no engine has the name.

```rust
pub enum Engine { Raw, Envsubst, Mustache }                // #[non_exhaustive]
Engine::ALL: &'static [Engine]
Engine::name(self) -> &'static str                         // "raw" "envsubst" "mustache"
Engine::suffixes(self) -> &'static [&'static str]          // no dot: Envsubst ["envsubst","template"], Mustache ["mustache","mst"]
Engine::by_name(&str) -> Option<Engine>                    // exact lowercase name
Engine::by_suffix(&str) -> Option<Engine>                  // leading dot optional, any case
Engine::render(self, text, &Value, &Options) -> Result<String, Error>
```

`Engine` implements `Display` (the name) and `FromStr` (`Error::UnknownEngine`). `"Mustache"` is unknown.

## Options (`tplx_rs::Options`)

```rust
Options::new() -> Options<'a>                  // also Default; Clone + Copy
.limits(Limits) -> Self
.resolver(&'a dyn Resolver) -> Self            // Mustache partials
.envsubst(EnvsubstOptions) -> Self
.mustache(MustacheOptions) -> Self
```

```rust
pub struct Limits { pub max_output: usize, pub max_depth: usize, pub max_iterations: usize }  // #[non_exhaustive]
```

Defaults 1,000,000 bytes, 100 levels, 100,000 iterations. `#[non_exhaustive]`: start from `Limits::default()` and
assign fields. Output over `max_output` bytes is `Error::Limit(Limit::Output)` (exactly `max_output` is fine), in
every engine. `max_depth` bounds Mustache section nesting (the template is level 1: a limit of 3 allows two nested
sections), partial expansion, envsubst nested words and the flattening of data. `max_iterations` counts Mustache
section items, and bounds envsubst glob matching (ten steps per iteration).

```rust
pub trait Resolver { fn read(&self, name: &str) -> Option<String>; }
```

Implemented for `Fn(&str) -> Option<String>` and `BTreeMap<String, String>`. A Mustache partial is asked for by its
name as written; what the resolver lacks (or no resolver) renders as nothing.

## Engines

```rust
pub fn render_raw(text: &str, options: &Options<'_>) -> Result<String, Error>
```

The text unchanged, still held to `max_output`.

```rust
pub fn render_envsubst(text: &str, data: &Value, options: &Options<'_>) -> Result<String, Error>
pub struct EnvsubstOptions { pub dialect: Dialect, pub pattern: Option<Pattern>, pub features: Option<Features>, pub undefined: Undefined }
EnvsubstOptions::default()  // Gnu, dialect's pattern and features, Undefined::Strict
EnvsubstOptions::syntax(&self) -> (Pattern, Features)   // what is in force
```

`$NAME` and `${NAME}` as tplx's envsubst engine reads them. Names are `[A-Za-z_][A-Za-z0-9_]*`, matched
**case-insensitively**. The data is flattened: only an object has names; a path joins with `_` and is upper-cased
(`{"a": {"b": 1}}` is `A_B`), a list item is keyed by its index (`A_0`), `null` is empty, a boolean `true`/`false`,
a number as JSON writes it. A value is inserted as it is and never read again. An expression the dialect does not
have is text, never an error: its `$` is written and scanning goes on inside it (`${U:-$B}` keeps `${U:-`).

| `Dialect` | `Pattern` | `Feature`s |
| --- | --- | --- |
| `Gnu` (default) | `All` (`$NAME`, `${NAME}`) | none: no defaults, `$$` is not an escape |
| `A8m` | `All` | `Escape` `Default` `Assign` `Alternate` `Nested` |
| `Drone` | `Braced` | `Escape` `Default` `Assign` `Nested` `Slice` `Replace` `Trim` `Case` `Length` |

`pattern` and `features`, when `Some`, replace the dialect's. `Pattern`: `Braced`, `Unbraced`, `All`. `Features` is a
set: `Features::none()`, `Features::of(&[..])`, `.with(Feature)`, `.contains(Feature)`. The features:

| `Feature` | Syntax |
| --- | --- |
| `Escape` | `$$` is `$` |
| `Default` | `${A-w}` (unset), `${A:-w}` (unset or empty) |
| `Assign` | `${A=w}`, `${A:=w}`, read as `-` and `:-` |
| `Alternate` | `${A+w}` (set), `${A:+w}` (set and not empty), else empty |
| `Nested` | a word is itself substituted (`${A:-${B}}`) |
| `Unquote` | one layer of matching quotes is stripped from a word |
| `Slice` | `${A:1}`, `${A:1:2}`, `${A: -2}`, `${A:(-2)}`, as bash |
| `Replace` | `${A/p/t}`, `${A//p/t}`, `${A/#p/t}`, `${A/%p/t}`; `p` is a bash glob (`*` `?` `[..]` `[!..]` `\x`) |
| `Trim` | `${A#p}`, `${A##p}`, `${A%p}`, `${A%%p}` |
| `Case` | `${A^}`, `${A^^}`, `${A,}`, `${A,,}` |
| `Length` | `${#A}` |

`Undefined` (what a name with no value does; the default operators never trigger it): `Strict` (default, as tplx's
loader: `Error::Undefined { line, column }`), `Lenient` (empty, as GNU envsubst does), `Debug` (the expression is
written back as it was, so `$request_uri` of an nginx template survives). From GNU on POSIX, names are
case-insensitive.

```rust
pub fn render_mustache(text: &str, data: &Value, options: &Options<'_>) -> Result<String, Error>
pub struct MustacheOptions { pub escape: Escape }     // #[non_exhaustive]; Default: Escape::Html
pub enum Escape { Html, None }                        // #[non_exhaustive]
```

Mustache as the official specification defines it (it passes the specification's comments, delimiters,
interpolation, inverted, partials and sections modules): `{{x}}` escapes `& < > " '` under `Escape::Html` and
writes the value as it is under `Escape::None`; `{{{x}}}` and `{{&x}}` never escape. Dotted names (the first part
from the innermost context that has the key, later parts must exist), `{{.}}`, sections over lists and values,
inverted sections, comments, `{{=<% %>=}}`, standalone-line removal, partials from the resolver (indented as the
specification says; delimiters reset in a partial). Truthiness is JSON's: missing, `null`, `false` and an empty list
are false, `0`, `""` and `{}` are true. Numbers print as JSON; lists, objects and `null` print nothing. A name no
context has renders as nothing (no undefined error). Lambdas, inheritance (`{{<`, `{{$`) and dynamic names are not
supported; the first two are `SyntaxKind::UnsupportedTag`.

## Errors

```rust
pub enum Error {                                                       // #[non_exhaustive]; Clone, PartialEq
    UnknownEngine,
    Syntax { line: usize, column: usize, kind: SyntaxKind },           // Mustache refuses the template
    Undefined { line: usize, column: usize },                          // envsubst, Undefined::Strict
    Limit(Limit),                                                      // Limit::{Output, Depth, Iterations}
}
```

`SyntaxKind`: `UnclosedTag`, `InvalidName`, `UnsupportedTag`, `InvalidDelimiters`, `UnclosedSection`, `StrayClose`,
`MismatchedClose`. All three enums are `#[non_exhaustive]` and implement `Display` and `Error`. Positions are
1-based, columns in characters; an `Undefined` inside a nested word points at the expression that holds it.
Messages are one lowercase clause and never quote the template or the data.

## Gotchas

- A recursive Mustache partial finds a key through the parent contexts, so a leaf must hold the key it recurses on
  (an empty list); otherwise it recurses until `max_depth`.
- The default envsubst is GNU's, so `${A:-x}` is text: choose `Dialect::A8m` or `Dialect::Drone` for defaults.
- The default undefined mode is `Strict`: `$NAME` with no value is an error. Set `Undefined::Lenient` for GNU's empty.
- Under `Debug`, a `${A:-x}` of a dialect that has defaults is never kept: the default applies.
- Tests compare with GNU envsubst's output and with tplx's engine; a case where they differ is the case-insensitive
  name match.
