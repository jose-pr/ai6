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
pub struct ControlId(String);
```

Each `new(value: impl Into<String>) -> Result<Self, InvalidIdentity>` rejects
empty values, control characters and identifiers longer than `MAX_ID_BYTES`
(256 UTF-8 bytes). `as_str(&self) -> &str` borrows the validated spelling.
`Display` returns that spelling. Fields are private so callers cannot bypass
validation. An Operation identifies a project; a Mission ID groups one unit of
work; a provider Session ID identifies one resumable agent operation; a control ID
identifies one operator control request.

## Board events

```rust
pub enum SessionState {
    Queued, Working, Blocked, Stalled, Parked, Quarantined, Done, Failed, Abandoned,
}
impl SessionState {
    pub const fn as_str(self) -> &'static str;   // "queued", "working", ...
    pub fn parse(state: &str) -> Option<Self>;   // None for an unknown spelling
}
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
    pub reason: Option<String>,   // why AI6 holds this state: "green-ink", "budget"
    pub sequence: u64,
}
pub const MAX_REASON_BYTES: usize = 64;
pub fn valid_reason(reason: &str) -> bool;   // 1..=64 bytes of a-z 0-9 - _ .
```

All fields above are public. `SessionState` is non-exhaustive and carries the state
of the Mission the Session serves: `Queued` means the Mission waits for another
attempt (this Session will not run again) and `Abandoned` that an operator or AI6
withdrew it. `reason` is an optional field of the lifecycle frame: `green-ink` on a
`Blocked` Session whose Agent waits for a person's approval of an irreversible step.
The receiver refuses a reason that `valid_reason` rejects. `sequence` is a monotonic cursor for events in one provider Session. A
Mission may have multiple Sessions; these types do not derive one identity from
another.

## Operator controls

```rust
pub enum ControlAction { Cancel, Retry, Resume, Interrupt }
impl ControlAction {
    pub const ALL: [Self; 4];
    pub const fn as_str(self) -> &'static str;   // "cancel", "retry", "resume", "interrupt"
    pub fn parse(action: &str) -> Option<Self>;
}

pub struct MissionControl { /* fields are private */ }
impl MissionControl {
    pub fn new(control_id: ControlId, operation_id: OperationId, mission_id: MissionId,
               action: ControlAction) -> Self;
    pub fn with_session(self, session_id: SessionId) -> Self;
    pub fn with_agent(self, agent: impl Into<String>) -> Result<Self, InvalidControl>;
    pub fn with_message(self, message: impl Into<String>) -> Result<Self, InvalidControl>;
    pub fn control_id(&self) -> &ControlId;
    pub fn operation_id(&self) -> &OperationId;
    pub fn mission_id(&self) -> &MissionId;
    pub fn session_id(&self) -> Option<&SessionId>;
    pub const fn action(&self) -> ControlAction;
    pub fn agent(&self) -> Option<&str>;
    pub fn message(&self) -> Option<&str>;
}
```

A control is what Universal Exports hands the Station holding a Mission. `Cancel` stops
the live Session and abandons the Mission, `Retry` queues a stalled or finished Mission
as a new attempt (optionally for `agent`), `Resume` resumes a parked, blocked or stalled
Mission (optionally with `message`) and `Interrupt` stops the current turn. `session_id`
names the provider Session the requester expects to be current. `with_agent` refuses a
blank codename, one over `MAX_ID_BYTES` or one with control characters; `with_message`
refuses a blank message or one over `MAX_CONTROL_MESSAGE_BYTES` (16 KiB). `Debug`
redacts the message to its length. `ControlAction` is non-exhaustive.

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

pub fn control_frames(
    control: &MissionControl,
) -> Result<(Vec<u8>, Vec<u8>), guatiao::value::ValueError>;

pub struct FrameReceiver;
pub enum IncomingFrame {
    Lifecycle(LifecycleEvent), Terminal(TerminalData), Resize(TerminalResize),
    Control(MissionControl), Closed(u64)
}
impl FrameReceiver {
    pub fn new() -> Self;
    pub fn accept(&mut self, frame: &[u8]) -> Result<Option<IncomingFrame>, FrameDecodeError>;
}
```

Each function returns a schema announcement followed by a schema-checked value
frame on a stable channel. A transport supplies message boundaries; WebSocket
uses one guatiao frame per binary message. Channel IDs are `1` for lifecycle
events, `2` for terminal data, `3` for terminal resize and `4` for operator
controls; a control's schema does not depend on its values, so a receiver may build
it from any control. Terminal data includes
raw bytes and an `input` or `output` direction; resize values carry bounded,
non-zero columns and rows.

`FrameReceiver` retains channel schemas between frames, validates received values,
and decodes lifecycle records and bounded terminal bytes. Schema announcements
return `Ok(None)`; unknown channels, malformed values and invalid identities fail.

## Constants

```rust
pub const MAX_ID_BYTES: usize = 256;
pub const MAX_TERMINAL_BYTES: usize = 1_048_576;
pub const MAX_CONTROL_MESSAGE_BYTES: usize = 16_384;
```

These limits apply to the constructors documented above.

## Errors

`InvalidIdentity`, `InvalidControl`, `FrameTooLarge`, `InvalidSize` and `FrameDecodeError` are
non-exhaustive error types implementing `core::error::Error`. They do not include
rejected payload bytes. Frame construction may also return guatiao's `ValueError`.

## Gotchas

- A2A Task IDs are external data and are not Mission IDs or provider Session IDs.
- `TerminalData::Debug` redacts its contents. Do not add terminal bytes to logs.
- `MissionControl::Debug` redacts a resume's message; it is operator text for the Agent.
- Schema and value frames are separate. Announce a channel schema before sending
  its values to `guatiao::value::wire::channel::Receiver`.
