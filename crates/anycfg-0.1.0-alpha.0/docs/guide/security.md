# Security and resource limits

`anycfg` only orchestrates inputs, codec selection, and caller-defined value merging. Treat configuration content as untrusted when it comes from users or external systems.

## What the core does not execute

- Does not execute format directives as commands.
- Does not run shell commands, templates, includes or interpolation.
- Does not follow remote URLs or fetch content.
- Does not automatically resolve a source's logical name to an actual path.
- Does not implicitly register format parsers.

A `#!format` header only selects an already registered codec for automatic formats; it is never executable.

## Limits

| Limit | Default | Where it is enforced |
| --- | --- | --- |
| Per-source bytes | 8 MiB | Input buffering / bytes before decoding |
| Combined source bytes | 64 MiB | Loader before decoding subsequent sources |
| Value depth | 64 levels | A typed read, and passed to the caller's value adapter |

Each is a setting with a default and no floor. Tune budgets with `Loader::max_source_bytes`, `max_total_bytes`, and `max_depth`. Direct registry reads accept `Options::max_bytes` and `Options::max_depth`.

A reader may consume one extra byte to identify an exceeded budget. This is intentional; an unbounded reader is not read in full after reaching the configured limit.

!!! warning "Value depth is consumer-enforced"

    The library cannot inspect an arbitrary `V` until a typed read walks it. The default `MergeValues::validate` is a no-op; implement it for recursive value models, and enforce parser recursion/expansion limits inside the selected codec.

## Safe errors

A failure's message, fields, `Debug` and source chain quote no value from a document.

- **A typed read** discards the value type's own error, which quotes the value, and keeps what serde's structured calls give: a field's name, what was expected, and the kind of value found. A `custom` message is dropped, because it may hold anything.
- **A key** is the one document text an error repeats, because a refused key must be named. A control character in it is escaped.
- **A first line naming no format** is not repeated; the error lists the registered formats.
- **A codec's message** reaches `Error::Parse` as written, and the crate cannot check it. A codec passes on only what its parser is known not to quote: see [Serde and typed configuration](serde.md).

Do not add potentially sensitive source content to custom logging or error types. Filesystem `Error::Io` may still reveal ordinary operating-system diagnostics; assess that separately from document contents.

## Filesystem boundaries

`default_configs` and `Search` reject empty, absolute and traversing names and only return paths of existing files. They do not create directories or open files, and never look in the working directory unless a person named a relative path.

`Source::path`, on the other hand, opens a **caller-selected path** when loading; the library does not provide a general sandbox. If your application loads user-supplied paths, enforce your own allowed roots, symlink rules and access controls.

## Trusted codecs

The application owns all registered codecs and any custom merge adapter. Only register parsers you intend to trust, with appropriate limits for aliases, nesting, expression evaluation, and other format-specific features. Do not assume that a parser is safe merely because the core never executes directives.

See [Sources and streams](sources.md), [Format registration](formats.md), and [Merging layers](merging.md).
