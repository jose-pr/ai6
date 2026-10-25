# `ai6-diptel` — API header

Header-file-style reference for the `ai6-diptel` crate: every public item
with its signature, contract and gotchas, so the crate can be used without
reading its source. It ships inside the package and is self-contained.
Development documentation lives with the source at
<https://github.com/jose-pr/ai6>.

## Features, MSRV and dependencies

| Feature | Adds | Default |
| --- | --- | --- |
| (none) | The shared protocol API. | Yes |

MSRV: Rust 1.89. The default build depends on the AI6-pinned `guatiao` crate
for values, schemas and channel frames. There are no optional dependencies.

## Imports and modules

Every public item is imported from the crate root. The crate has no public
modules. There are no feature-gated items.

## Unsafe code

The crate forbids unsafe code with `#![forbid(unsafe_code)]`.

## Identities

```rust
pub struct OperationId(String);
pub struct MissionId(String);
pub struct SessionId(String);
```

Each `new(value: impl Into<String>) -> Result<Self, InvalidIdentity>` rejects
empty values, control characters and identifiers longer than `MAX_ID_BYTES`
(256 UTF-8 bytes). `as_str(&self) -> &str` borrows the validated spelling.
`Display` returns that spelling. Fields are private so callers cannot bypass
validation. An Operation identifies a project; a Mission ID groups one unit of
work; a provider Session ID identifies one resumable agent operation.

## Board events

```rust
pub enum SessionState { Working, Blocked, Stalled, Parked, Done, Failed }
pub struct SessionSnapshot {
    pub operation_id: OperationId,
    pub mission_id: MissionId,
    pub session_id: SessionId,
    pub state: SessionState,
    pub sequence: u64,
}
pub struct LifecycleEvent {
    pub operation_id: OperationId,
    pub mission_id: MissionId,
    pub session_id: SessionId,
    pub state: SessionState,
    pub sequence: u64,
}
```

All fields above are public. `SessionState` is non-exhaustive. `sequence` is a
monotonic cursor for events in one provider Session. A Mission may have multiple
Sessions; these types do not derive one identity from another.

## Terminal data

```rust
pub enum TerminalDirection { Output, Input }
pub struct TerminalData { /* fields are private */ }
impl TerminalData {
    pub fn new(
        session_id: SessionId,
        sequence: u64,
        direction: TerminalDirection,
        bytes: Vec<u8>,
    ) -> Result<Self, FrameTooLarge>;
    pub fn session_id(&self) -> &SessionId;
    pub const fn sequence(&self) -> u64;
    pub const fn direction(&self) -> TerminalDirection;
    pub fn bytes(&self) -> &[u8];
}
```

`new` refuses more than `MAX_TERMINAL_BYTES` (1 MiB). The bytes are never
decoded as UTF-8. Its `Debug` output redacts the bytes and includes only their
length.

```rust
pub struct TerminalResize { /* fields are private */ }
impl TerminalResize {
    pub fn new(session_id: SessionId, sequence: u64, columns: u16, rows: u16)
        -> Result<Self, InvalidSize>;
    pub fn session_id(&self) -> &SessionId;
    pub const fn sequence(&self) -> u64;
    pub const fn columns(&self) -> u16;
    pub const fn rows(&self) -> u16;
}
```

Both dimensions must be non-zero.

## Guatiao frames

```rust
pub fn lifecycle_frames(
    event: &LifecycleEvent,
) -> Result<(Vec<u8>, Vec<u8>), guatiao::value::ValueError>;

pub fn terminal_frames(
    event: &TerminalData,
) -> Result<(Vec<u8>, Vec<u8>), guatiao::value::ValueError>;

pub fn resize_frames(
    event: &TerminalResize,
) -> Result<(Vec<u8>, Vec<u8>), guatiao::value::ValueError>;

pub struct FrameReceiver;
pub enum IncomingFrame {
    Lifecycle(LifecycleEvent), Terminal(TerminalData), Resize(TerminalResize), Closed(u64)
}
impl FrameReceiver {
    pub fn new() -> Self;
    pub fn accept(&mut self, frame: &[u8]) -> Result<Option<IncomingFrame>, FrameDecodeError>;
}
```

Each function returns a schema announcement followed by a schema-checked value
frame on a stable channel. A transport supplies message boundaries; WebSocket
uses one guatiao frame per binary message. Channel IDs are `1` for lifecycle
events, `2` for terminal data and `3` for terminal resize. Terminal data includes
raw bytes and an `input` or `output` direction; resize values carry bounded,
non-zero columns and rows.

`FrameReceiver` retains channel schemas between frames, validates received values,
and decodes lifecycle records and bounded terminal bytes. Schema announcements
return `Ok(None)`; unknown channels, malformed values and invalid identities fail.

## Constants

```rust
pub const MAX_ID_BYTES: usize = 256;
pub const MAX_TERMINAL_BYTES: usize = 1_048_576;
```

These limits apply to the constructors documented above.

## Errors

`InvalidIdentity`, `FrameTooLarge`, `InvalidSize` and `FrameDecodeError` are
non-exhaustive error types implementing `core::error::Error`. They do not include
rejected payload bytes. Frame construction may also return guatiao's `ValueError`.

## Gotchas

- A2A Task IDs are external data and are not Mission IDs or provider Session IDs.
- `TerminalData::Debug` redacts its contents. Do not add terminal bytes to logs.
- Schema and value frames are separate. Announce a channel schema before sending
  its values to `guatiao::value::wire::channel::Receiver`.
