# coqui-acp-pool

[![Version](https://img.shields.io/crates/v/coqui-acp-pool.svg)](https://crates.io/crates/coqui-acp-pool)
[![Rust version](https://img.shields.io/crates/msrv/coqui-acp-pool.svg)](https://crates.io/crates/coqui-acp-pool)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-acp/blob/main/LICENSE)
[![Docs](https://img.shields.io/docsrs/coqui-acp-pool)](https://docs.rs/coqui-acp-pool)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-acp/test.yml)](https://github.com/Boriken-Dev/coqui-acp/actions/workflows/test.yml)

**The policy that picks which agent, account and model serves a session or a task**, and moves a running session on when its
usage runs out. A pool is an ordered list of members; the host says what it knows of each (usage, runs, holds, whether it is
installed and signed in) and the library answers with the member that takes the turn, or with why none can. It does no I/O, reads no
file, holds no secret, has no clock (every time is an argument, in Unix seconds) and names no harness, account kind or standard.

## Features

- `Strategy`: `priority` (the default: highest priority first, equal ones in the listed order), `headroom` (least used first, use not
  reported counting as 500 permille; then fewest runs, least recently used, priority) and `round_robin` (the member after the last
  that took a turn, kept while the session's prompt cache is warm).
- Per member: `priority`, `max_concurrent`, `stop_permille`, an opaque `group` (the harness) and labels. Every bound is a `Settings`
  value with a default and no floor, and `Settings::non_defaults` shows a changed one.
- `decide`: a member that is unavailable, at its cap of runs, refused by a limit, over a bucket that covers the model (exhausted, or at
  its stop point, with the window still open) or held is passed over, each with a `Reason`. Usage that is not reported leaves a member
  usable and marked `usage_known = false`; `Usage::Pending` says a report is still to come, and the library never waits.
- `Switchboard`: assigns a new session (a member that fails to start is left out and the next is tried) and, at a turn's end, moves a
  running session when its member ran out: the same group first, never in the middle of a turn, the history carried by the host's
  `Starter::carry`, and the continuation sent.
- `clean`: the carried session's last turn, stripped of the limit error or refusal, the text that names the limit and what follows it,
  tool calls left open and the host's own notices. A pure function over `coqui-acp-history`'s `Document`, with phrases the host passes in.
- `resume`: the role's continuation text, else one default line; the person's own prompt again when the interrupted turn did no work.
- `accounts`, `check_pool`, `fits`, `Cache`, `Memory`: account pools' grouping and checks, a capability check, a time-to-live map and the
  round-robin cursor, all behind plain values.
- `TurnRecord` (defined in `coqui-acp-history`): what a `Ledger` receives for each served turn: harness, agent, account, model, effort,
  role, member, profile, tokens, cost, duration, outcome and an open map of tags namespaced by who set them.

## Installation

Not published: `publish = false` until a release is chosen. The `std` feature (default) adds `SharedMemory` and `SharedCache`;
without it the crate is `no_std` plus `alloc`.

## Quick start

```rust
use coqui_acp_pool::{Ask, Member, Memory, Pool, Standing, Strategy, decide};

let pool = Pool::new("planner")
    .strategy(Strategy::Priority)
    .member(Member::new("claude", ()).group("claude"))
    .member(Member::new("codex", ()).group("codex"));
// One standing for each member: the host's view of it now.
let standings = [Standing::new().unavailable("not signed in"), Standing::new()];
let chosen = decide(&pool, &mut Memory::new(), &standings, &Ask::new(1_800_000_000))?;
assert_eq!(chosen.member, "codex");
# Ok::<(), coqui_acp_pool::NoneLeft>(())
```

## Development

```sh
cargo fmt --all --check
cargo clippy -p coqui-acp-pool --all-targets --all-features -- -D warnings
cargo test -p coqui-acp-pool
RUSTDOCFLAGS="-D warnings" cargo doc -p coqui-acp-pool --no-deps --all-features
```

## Licence

MPL-2.0.
