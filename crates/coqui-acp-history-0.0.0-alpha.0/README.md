# coqui-acp-history

[![Version](https://img.shields.io/crates/v/coqui-acp-history.svg)](https://crates.io/crates/coqui-acp-history)
[![Rust version](https://img.shields.io/crates/msrv/coqui-acp-history.svg)](https://crates.io/crates/coqui-acp-history)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/coqui-acp-history)](https://docs.rs/coqui-acp-history)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-acp/test.yml)](https://github.com/Boriken-Dev/coqui-acp/actions/workflows/test.yml)

**One file format for the history of an [Agent Client Protocol](https://agentclientprotocol.com) session**, so a session can move
between machines and between harnesses. JSON Lines in ACP's own vocabulary: a header, then the prompts, updates, cancels and
permission answers in order, with turn boundaries, stop reasons and compaction summaries. The format is specified in
[`docs/session-history.md`](https://github.com/Boriken-Dev/coqui-acp/blob/main/docs/session-history.md); this crate is its
reference implementation, and `vectors/session_history/` holds the cases a reader in any language runs.

## Features

- `Reader`: a bounded, streaming reader. It holds one line at a time (a line costs a bounded multiple of its bytes: at most
  `max_values` values, and about 4 times its size for text, up to 60 for tiny objects), refuses an unknown line kind, version,
  key, repeated key, out-of-order line, header text with a control character or malformed input with the line and the JSON
  pointer of the place (never the content), and checks the header's digest at the end. Lines are handed over before that
  check: `Reader::check`, or reading a `Document`, is how a consumer acts only on a verified history.
- `Writer`: checks every line the way the reader does, so it cannot write a file the reader refuses.
- `Document`: a whole history in memory, and `Document::seed`, the latest compaction summary and the lines after it, with
  `Seed::fit` for the latest whole turns that fit a budget.
- `Redactor`: removes values and patterns, only when asked, and reports what it removed by rule position, never by text.
- `tools`: the table that names each harness's built-in tools once (`read`, `write`, `edit`, `patch`, `shell`, `grep`, `glob`,
  `fetch`), with the key renames each direction.
- `record::SessionRecorder`: records any ACP agent's session from the messages its client exchanges with it (version 1 and 2
  updates as the agent sent them, turns, cancels, permissions, modes, compactions) into a `Document`, so a session of an agent
  with no files of its own can be exported.
- `replay::Replay`: for an agent that cannot import a history, the newest turns that fit a size budget as one quoted block for the
  first prompt, and a loss report that says it was replayed and what was cut. The text is delimited and escaped as data, not
  instructions.
- `LossReport`: what an import could not carry (thinking dropped, tools turned into notes, attachments known only by digest).
- Settings as Guatiao values: `Bounds`, `ReaderSettings`, `WriterSettings`, `RedactionSettings`, each with defaults, unknown keys
  refused, and `non_defaults()`.
- `coqui-acp-history`, a command (feature `cli`): `list`, `show` (both print what a history says with control and
  bidirectional characters escaped), `convert` (checked, and its lines copied as they are unless redaction removes something or
  `--seal` is asked for), and `export` and `import`, which need a harness's codec and say so.

## Installation

Not published: `publish = false` until a release is chosen. `cargo build -p coqui-acp-history` builds the library;
`--features cli` adds the command.

## Quick start

```rust
use coqui_acp_history::{Document, Header, Line, Origin, Stop, StopReason};

let mut doc = Document::new(Header::new("0b2e9a4c-7a35-4d2f-9f0e-0f4f3f1f2a11", Origin::new("claude")));
doc.lines.push(Line::text_prompt("hello"));
doc.lines.push(Line::turn_end(Stop::Acp(StopReason::EndTurn)));

let text = doc.to_string()?;
assert_eq!(text.parse::<Document>()?, doc);
# Ok::<(), coqui_acp_history::HistoryError>(())
```

## Development

```sh
cargo fmt --all --check
cargo clippy -p coqui-acp-history --all-targets --all-features -- -D warnings
cargo test -p coqui-acp-history --all-features
RUSTDOCFLAGS="-D warnings" cargo doc -p coqui-acp-history --no-deps --all-features
```

## Licence

MPL-2.0.
