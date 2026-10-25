# ai6-diptel

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/jose-pr/ai6/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/jose-pr/ai6/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/jose-pr/ai6/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://jose-pr.github.io/ai6/rust/ai6_diptel/)
[![CI](https://img.shields.io/github/actions/workflow/status/jose-pr/ai6/test.yml)](https://github.com/jose-pr/ai6/actions/workflows/test.yml)

DipTels: the native and WebAssembly wire contract between a Station, Universal Exports
and the Moneypenny app. **One identity contract and one guatiao frame format** carry
Session lifecycle events, terminal bytes, terminal geometry and operator controls. See the
[crate API header](https://github.com/jose-pr/ai6/blob/main/crates/ai6-diptel/AGENTS.md).

## Features

- Distinct validated Operation ID, Mission ID and provider Session ID types.
- Schema-checked guatiao lifecycle, binary terminal, resize and operator control frames.
- Native and `wasm32-unknown-unknown` support with no unsafe code.
- No process-global state or custom wire codec.

`FrameReceiver` validates gateway frames against their channel schemas and yields
decoded lifecycle events, terminal bytes, resize events, operator controls or channel closure. Schema announcements
are consumed internally and return `None`.

## Installation

This crate is part of the AI6 workspace and is not published separately. From the
repository root, add it as a path dependency:

```console
cargo add ai6-diptel --path crates/ai6-diptel
```

There are no Cargo feature flags.

## Quick start

```rust
use ai6_diptel::{
    LifecycleEvent, MissionId, OperationId, SessionId, SessionState, lifecycle_frames,
};

let event = LifecycleEvent {
    operation_id: OperationId::new("operation-a")?,
    mission_id: MissionId::new("mission-17")?,
    session_id: SessionId::new("provider-run-3")?,
    state: SessionState::Working,
    sequence: 4,
};
let (schema_frame, value_frame) = lifecycle_frames(&event)?;
assert!(!schema_frame.is_empty() && !value_frame.is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
```

A Mission ID identifies one unit of work and may group several Sessions. A
provider-issued Session ID identifies one resumable agent operation and stays
the same when that operation resumes. An A2A Task ID is external protocol data.

## API overview

| Item | Purpose |
| --- | --- |
| `OperationId`, `MissionId`, `SessionId`, `ControlId` | Validated identities for project, work unit, provider operation and operator control. |
| `SessionSnapshot`, `LifecycleEvent`, `SessionState` | Board state keyed by Operation, Mission and Session. |
| `TerminalData`, `TerminalDirection`, `TerminalResize` | Bounded binary input/output and terminal geometry. |
| `ControlAction`, `MissionControl` | An operator control (cancel, retry, resume, interrupt) for the Station holding a Mission. |
| `lifecycle_frames`, `terminal_frames`, `resize_frames`, `control_frames` | guatiao schema announcement and schema-checked value frames. |
| `FrameReceiver`, `IncomingFrame` | Schema-checked gateway frame decoding. |
| `MAX_ID_BYTES`, `MAX_TERMINAL_BYTES`, `MAX_CONTROL_MESSAGE_BYTES` | Protocol bounds applied by constructors. |

## Development

From the repository root, run:

```console
cargo fmt --all --check
cargo test -p ai6-diptel
cargo clippy -p ai6-diptel --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc -p ai6-diptel --no-deps
cargo check -p ai6-diptel --target wasm32-unknown-unknown
```

### Releasing

`ai6-diptel` is an internal workspace crate (`publish = false`). Update
its version with the workspace package when an AI6 release requires it.

## License

MPL-2.0. See [LICENSE](https://github.com/jose-pr/ai6/blob/main/LICENSE).
