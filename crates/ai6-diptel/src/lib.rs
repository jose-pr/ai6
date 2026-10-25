#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]
#![warn(missing_docs)]

use core::fmt;
use guatiao::schema::{FieldBuilder, KindBuilder, SchemaBuilder};
use guatiao::value::convert::{FromValue, ToValue, TryAsRef};
use guatiao::value::wire::channel::{self, Received, Receiver};
use guatiao::{Alloc, Bytes, Map, Value};

/// The largest accepted identity, in UTF-8 bytes.
pub const MAX_ID_BYTES: usize = 256;
/// The largest terminal data frame accepted by this protocol.
pub const MAX_TERMINAL_BYTES: usize = 1024 * 1024;
/// The largest operator message a control carries, in UTF-8 bytes.
pub const MAX_CONTROL_MESSAGE_BYTES: usize = 16 * 1024;
const LIFECYCLE_CHANNEL: u64 = 1;
const TERMINAL_CHANNEL: u64 = 2;
const RESIZE_CHANNEL: u64 = 3;
const CONTROL_CHANNEL: u64 = 4;

/// An invalid or over-limit protocol identity.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InvalidIdentity;

impl fmt::Display for InvalidIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("identity must be non-empty, bounded, and contain no control characters")
    }
}

impl core::error::Error for InvalidIdentity {}

/// A validated Operation identifier. An Operation is a project scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId(String);

/// A validated Mission identifier grouping one unit of work.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MissionId(String);

/// A provider-issued identifier for one resumable agent operation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(String);

macro_rules! identity {
    ($name:ident) => {
        impl $name {
            /// Validates and stores an identifier.
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidIdentity> {
                let value = value.into();
                if value.is_empty()
                    || value.len() > MAX_ID_BYTES
                    || value.chars().any(char::is_control)
                {
                    return Err(InvalidIdentity);
                }
                Ok(Self(value))
            }

            /// Returns the identifier text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

/// A validated identifier for one operator control request.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ControlId(String);

identity!(OperationId);
identity!(MissionId);
identity!(SessionId);
identity!(ControlId);

/// The lifecycle state shown for an agent Session: the state of the Mission it serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionState {
    /// The Mission is queued for another attempt; this Session will not run again.
    Queued,
    /// The agent operation is running.
    Working,
    /// The operation is waiting on an external condition.
    Blocked,
    /// The operation stopped reporting progress.
    Stalled,
    /// A provider usage limit paused the operation until its quota resets.
    Parked,
    /// Medical stopped the operation because its Agent is quarantined; only an operator
    /// releases it.
    Quarantined,
    /// The operation finished.
    Done,
    /// The agent operation exited unsuccessfully.
    Failed,
    /// An operator or AI6 withdrew the Mission.
    Abandoned,
}

impl SessionState {
    /// The wire spelling of the state.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Stalled => "stalled",
            Self::Parked => "parked",
            Self::Quarantined => "quarantined",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Abandoned => "abandoned",
        }
    }

    /// Parses a wire spelling; `None` for an unknown state.
    pub fn parse(state: &str) -> Option<Self> {
        Some(match state {
            "queued" => Self::Queued,
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "stalled" => Self::Stalled,
            "parked" => Self::Parked,
            "quarantined" => Self::Quarantined,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "abandoned" => Self::Abandoned,
            _ => return None,
        })
    }
}

/// What an operator asks of a Mission or of its live Session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ControlAction {
    /// Stop the live Session, if any, and abandon the Mission; its safe house is finished
    /// with the work kept.
    Cancel,
    /// Queue a stalled or finished Mission again as a new attempt, optionally for another
    /// qualified Agent.
    Retry,
    /// Resume a parked, blocked or stalled Mission now, optionally with a message.
    Resume,
    /// Stop the Session's current turn; the Mission stays, blocked, until resumed.
    Interrupt,
}

impl ControlAction {
    /// Every action, in display order.
    pub const ALL: [Self; 4] = [Self::Cancel, Self::Retry, Self::Resume, Self::Interrupt];

    /// The wire spelling of the action.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cancel => "cancel",
            Self::Retry => "retry",
            Self::Resume => "resume",
            Self::Interrupt => "interrupt",
        }
    }

    /// Parses a wire spelling; `None` for an unknown action.
    pub fn parse(action: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == action)
    }
}

/// A control field was empty, too long or held control characters.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InvalidControl;

impl fmt::Display for InvalidControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("control fields must be non-empty, bounded and free of control characters")
    }
}

impl core::error::Error for InvalidControl {}

/// One operator control, as Universal Exports hands it to the Station holding the Mission.
///
/// The Station is the authority: it checks the Mission, the Operation and its own Session
/// lease before acting, and reports the outcome back.
#[derive(Clone, PartialEq, Eq)]
pub struct MissionControl {
    control_id: ControlId,
    operation_id: OperationId,
    mission_id: MissionId,
    session_id: Option<SessionId>,
    action: ControlAction,
    agent: Option<String>,
    message: Option<String>,
}

impl fmt::Debug for MissionControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MissionControl")
            .field("control_id", &self.control_id)
            .field("operation_id", &self.operation_id)
            .field("mission_id", &self.mission_id)
            .field("session_id", &self.session_id)
            .field("action", &self.action)
            .field("agent", &self.agent)
            .field(
                "message",
                &self
                    .message
                    .as_ref()
                    .map(|message| format!("[REDACTED; {} bytes]", message.len())),
            )
            .finish()
    }
}

impl MissionControl {
    /// A control with no Session, Agent or message.
    pub fn new(
        control_id: ControlId,
        operation_id: OperationId,
        mission_id: MissionId,
        action: ControlAction,
    ) -> Self {
        Self {
            control_id,
            operation_id,
            mission_id,
            session_id: None,
            action,
            agent: None,
            message: None,
        }
    }

    /// Names the provider Session the control expects to be the Mission's current one.
    #[must_use]
    pub fn with_session(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Names the Agent a retry is given to.
    ///
    /// # Errors
    ///
    /// [`InvalidControl`] for an empty codename, one over [`MAX_ID_BYTES`] or one with
    /// control characters.
    pub fn with_agent(mut self, agent: impl Into<String>) -> Result<Self, InvalidControl> {
        let agent = agent.into();
        if agent.trim().is_empty() || agent.len() > MAX_ID_BYTES || agent.contains(char::is_control)
        {
            return Err(InvalidControl);
        }
        self.agent = Some(agent);
        Ok(self)
    }

    /// Adds the message a resume sends to the Agent.
    ///
    /// # Errors
    ///
    /// [`InvalidControl`] for a blank message or one over [`MAX_CONTROL_MESSAGE_BYTES`].
    pub fn with_message(mut self, message: impl Into<String>) -> Result<Self, InvalidControl> {
        let message = message.into();
        if message.trim().is_empty() || message.len() > MAX_CONTROL_MESSAGE_BYTES {
            return Err(InvalidControl);
        }
        self.message = Some(message);
        Ok(self)
    }

    /// The control's identity, unique per request.
    pub fn control_id(&self) -> &ControlId {
        &self.control_id
    }

    /// The Operation the requester was authorized for.
    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    /// The Mission the control acts on.
    pub fn mission_id(&self) -> &MissionId {
        &self.mission_id
    }

    /// The provider Session expected to be the Mission's current one, when known.
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    /// What the operator asked for.
    pub const fn action(&self) -> ControlAction {
        self.action
    }

    /// The Agent a retry is given to, when the operator chose one.
    pub fn agent(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    /// The message a resume sends to the Agent; operator text, never logged.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

/// A session's identity and current board state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    /// Project that owns this work.
    pub operation_id: OperationId,
    /// Unit of work; a Mission can group multiple Sessions.
    pub mission_id: MissionId,
    /// Provider-issued resumable agent operation.
    pub session_id: SessionId,
    /// Current board state.
    pub state: SessionState,
    /// Monotonically increasing event cursor for this Session.
    pub sequence: u64,
}

/// A lifecycle update for one agent Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleEvent {
    /// Operation containing the Mission.
    pub operation_id: OperationId,
    /// Unit of work that may group multiple Sessions.
    pub mission_id: MissionId,
    /// Provider-issued resumable operation identifier.
    pub session_id: SessionId,
    /// Updated board state.
    pub state: SessionState,
    /// Monotonically increasing event cursor for this Session.
    pub sequence: u64,
}

/// Terminal input or output associated with a provider Session.
#[derive(Clone, PartialEq, Eq)]
pub struct TerminalData {
    /// Provider-issued resumable operation identifier.
    session_id: SessionId,
    /// Monotonically increasing event cursor for this Session.
    sequence: u64,
    /// Whether these bytes go to the viewer or to the agent's terminal.
    direction: TerminalDirection,
    /// Raw terminal bytes, which need not be UTF-8.
    bytes: Vec<u8>,
}

impl core::fmt::Debug for TerminalData {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TerminalData")
            .field("session_id", &self.session_id)
            .field("sequence", &self.sequence)
            .field("direction", &self.direction)
            .field(
                "bytes",
                &format_args!("[REDACTED; {} bytes]", self.bytes.len()),
            )
            .finish()
    }
}

impl TerminalData {
    /// Creates a terminal event after enforcing the protocol frame bound.
    pub fn new(
        session_id: SessionId,
        sequence: u64,
        direction: TerminalDirection,
        bytes: Vec<u8>,
    ) -> Result<Self, FrameTooLarge> {
        if bytes.len() > MAX_TERMINAL_BYTES {
            return Err(FrameTooLarge);
        }
        Ok(Self {
            session_id,
            sequence,
            direction,
            bytes,
        })
    }

    /// The provider-issued Session ID.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The event sequence number.
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// The terminal data direction.
    pub const fn direction(&self) -> TerminalDirection {
        self.direction
    }

    /// The raw terminal bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Direction of terminal bytes at the gateway boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TerminalDirection {
    /// Bytes emitted by the agent terminal for viewers.
    Output,
    /// Input bytes authorized for the agent terminal.
    Input,
}

impl TerminalDirection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Input => "input",
        }
    }
}

/// Creates a guatiao schema announcement and value frame for a lifecycle event.
pub fn lifecycle_frames(
    event: &LifecycleEvent,
) -> Result<(Vec<u8>, Vec<u8>), guatiao::value::ValueError> {
    let schema = SchemaBuilder::new()
        .field(FieldBuilder::new("operation_id", KindBuilder::string()).required())
        .field(FieldBuilder::new("mission_id", KindBuilder::string()).required())
        .field(FieldBuilder::new("session_id", KindBuilder::string()).required())
        .field(FieldBuilder::new("state", KindBuilder::string()).required())
        .field(FieldBuilder::new("sequence", KindBuilder::int()).required())
        .finish()?;
    let mut value = Map::new();
    value.set("operation_id", event.operation_id.as_str());
    value.set("mission_id", event.mission_id.as_str());
    value.set("session_id", event.session_id.as_str());
    value.set("state", event.state.as_str());
    value.set("sequence", event.sequence);
    frames(LIFECYCLE_CHANNEL, &schema, &value.into())
}

/// Creates a guatiao schema announcement and value frame for terminal bytes.
pub fn terminal_frames(
    event: &TerminalData,
) -> Result<(Vec<u8>, Vec<u8>), guatiao::value::ValueError> {
    let schema = SchemaBuilder::new()
        .field(FieldBuilder::new("session_id", KindBuilder::string()).required())
        .field(FieldBuilder::new("sequence", KindBuilder::int()).required())
        .field(FieldBuilder::new("direction", KindBuilder::string()).required())
        .field(FieldBuilder::new("bytes", KindBuilder::bytes()).required())
        .finish()?;
    let mut value = Map::new();
    value.set("session_id", event.session_id.as_str());
    value.set("sequence", event.sequence);
    value.set("direction", event.direction.as_str());
    value.set(
        "bytes",
        Bytes::from(event.bytes.clone()).to_value_in(Alloc::rust())?,
    );
    frames(TERMINAL_CHANNEL, &schema, &value.into())
}

/// Creates a guatiao schema announcement and value frame for a terminal resize.
pub fn resize_frames(
    event: &TerminalResize,
) -> Result<(Vec<u8>, Vec<u8>), guatiao::value::ValueError> {
    let schema = SchemaBuilder::new()
        .field(FieldBuilder::new("session_id", KindBuilder::string()).required())
        .field(FieldBuilder::new("sequence", KindBuilder::int()).required())
        .field(FieldBuilder::new("columns", KindBuilder::int()).required())
        .field(FieldBuilder::new("rows", KindBuilder::int()).required())
        .finish()?;
    let mut value = Map::new();
    value.set("session_id", event.session_id.as_str());
    value.set("sequence", event.sequence);
    value.set("columns", u64::from(event.columns));
    value.set("rows", u64::from(event.rows));
    frames(RESIZE_CHANNEL, &schema, &value.into())
}

/// Creates a guatiao schema announcement and value frame for an operator control.
pub fn control_frames(
    control: &MissionControl,
) -> Result<(Vec<u8>, Vec<u8>), guatiao::value::ValueError> {
    let schema = SchemaBuilder::new()
        .field(FieldBuilder::new("control_id", KindBuilder::string()).required())
        .field(FieldBuilder::new("operation_id", KindBuilder::string()).required())
        .field(FieldBuilder::new("mission_id", KindBuilder::string()).required())
        .field(FieldBuilder::new("session_id", KindBuilder::string()))
        .field(FieldBuilder::new("action", KindBuilder::string()).required())
        .field(FieldBuilder::new("agent", KindBuilder::string()))
        .field(FieldBuilder::new("message", KindBuilder::string()))
        .finish()?;
    let mut value = Map::new();
    value.set("control_id", control.control_id.as_str());
    value.set("operation_id", control.operation_id.as_str());
    value.set("mission_id", control.mission_id.as_str());
    if let Some(session_id) = &control.session_id {
        value.set("session_id", session_id.as_str());
    }
    value.set("action", control.action.as_str());
    if let Some(agent) = &control.agent {
        value.set("agent", agent.as_str());
    }
    if let Some(message) = &control.message {
        value.set("message", message.as_str());
    }
    frames(CONTROL_CHANNEL, &schema, &value.into())
}

/// One validated lifecycle, terminal or control frame received from a gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncomingFrame {
    /// A Mission or provider Session changed lifecycle state.
    Lifecycle(LifecycleEvent),
    /// Terminal output or input bytes for one provider Session.
    Terminal(TerminalData),
    /// A terminal geometry change from an attached viewer.
    Resize(TerminalResize),
    /// An operator control for the Station holding a Mission.
    Control(MissionControl),
    /// The remote side closed a channel.
    Closed(u64),
}

/// A malformed or unsupported incoming Universal Exports frame.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FrameDecodeError(String);

impl fmt::Display for FrameDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::error::Error for FrameDecodeError {}

/// Decodes gateway frames while retaining Guatiao channel schemas.
#[derive(Debug, Default)]
pub struct FrameReceiver {
    receiver: Receiver,
}

impl FrameReceiver {
    /// Creates a receiver without announced channels.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts one Guatiao frame. Schema announcements return `None`.
    pub fn accept(&mut self, bytes: &[u8]) -> Result<Option<IncomingFrame>, FrameDecodeError> {
        match self
            .receiver
            .accept(bytes)
            .map_err(|error| FrameDecodeError(error.to_string()))?
        {
            Received::Schema(_) => Ok(None),
            Received::Closed(channel) => Ok(Some(IncomingFrame::Closed(channel))),
            Received::Value { channel, value } if channel == LIFECYCLE_CHANNEL => {
                let map = Map::try_from(value)
                    .map_err(|_| FrameDecodeError("lifecycle frame must be a map".into()))?;
                let state = SessionState::parse(&text(&map, "state")?)
                    .ok_or_else(|| FrameDecodeError("unknown lifecycle state".into()))?;
                let event = LifecycleEvent {
                    operation_id: OperationId::new(text(&map, "operation_id")?)
                        .map_err(|error| FrameDecodeError(error.to_string()))?,
                    mission_id: MissionId::new(text(&map, "mission_id")?)
                        .map_err(|error| FrameDecodeError(error.to_string()))?,
                    session_id: SessionId::new(text(&map, "session_id")?)
                        .map_err(|error| FrameDecodeError(error.to_string()))?,
                    state,
                    sequence: number(&map, "sequence")?,
                };
                Ok(Some(IncomingFrame::Lifecycle(event)))
            }
            Received::Value { channel, value } if channel == TERMINAL_CHANNEL => {
                let map = Map::try_from(value)
                    .map_err(|_| FrameDecodeError("terminal frame must be a map".into()))?;
                let session_id = SessionId::new(text(&map, "session_id")?)
                    .map_err(|error| FrameDecodeError(error.to_string()))?;
                let direction = match text(&map, "direction")?.as_str() {
                    "input" => TerminalDirection::Input,
                    "output" => TerminalDirection::Output,
                    _ => return Err(FrameDecodeError("unknown terminal direction".into())),
                };
                let value = map
                    .get("bytes")
                    .ok_or_else(|| FrameDecodeError("missing terminal bytes".into()))?;
                let bytes = TryAsRef::<[u8]>::try_as_ref(value)
                    .ok_or_else(|| FrameDecodeError("terminal bytes have the wrong type".into()))?
                    .to_vec();
                let data =
                    TerminalData::new(session_id, number(&map, "sequence")?, direction, bytes)
                        .map_err(|error| FrameDecodeError(error.to_string()))?;
                Ok(Some(IncomingFrame::Terminal(data)))
            }
            Received::Value { channel, value } if channel == RESIZE_CHANNEL => {
                let map = Map::try_from(value)
                    .map_err(|_| FrameDecodeError("resize frame must be a map".into()))?;
                let event = TerminalResize::new(
                    SessionId::new(text(&map, "session_id")?)
                        .map_err(|error| FrameDecodeError(error.to_string()))?,
                    number(&map, "sequence")?,
                    u16::try_from(number(&map, "columns")?)
                        .map_err(|_| FrameDecodeError("columns are out of range".into()))?,
                    u16::try_from(number(&map, "rows")?)
                        .map_err(|_| FrameDecodeError("rows are out of range".into()))?,
                )
                .map_err(|error| FrameDecodeError(error.to_string()))?;
                Ok(Some(IncomingFrame::Resize(event)))
            }
            Received::Value { channel, value } if channel == CONTROL_CHANNEL => {
                let map = Map::try_from(value)
                    .map_err(|_| FrameDecodeError("control frame must be a map".into()))?;
                Ok(Some(IncomingFrame::Control(decode_control(&map)?)))
            }
            Received::Value { channel, .. } => Err(FrameDecodeError(format!(
                "unsupported Universal Exports channel {channel}"
            ))),
        }
    }
}

fn text(map: &Map, key: &str) -> Result<String, FrameDecodeError> {
    let value = map
        .get(key)
        .ok_or_else(|| FrameDecodeError(format!("missing {key}")))?;
    String::from_value(value).map_err(|_| FrameDecodeError(format!("{key} must be text")))
}

fn number(map: &Map, key: &str) -> Result<u64, FrameDecodeError> {
    let value = map
        .get(key)
        .ok_or_else(|| FrameDecodeError(format!("missing {key}")))?;
    u64::from_value(value).map_err(|_| FrameDecodeError(format!("{key} must be a number")))
}

fn optional_text(map: &Map, key: &str) -> Result<Option<String>, FrameDecodeError> {
    map.get(key)
        .map(|value| {
            String::from_value(value).map_err(|_| FrameDecodeError(format!("{key} must be text")))
        })
        .transpose()
}

fn decode_control(map: &Map) -> Result<MissionControl, FrameDecodeError> {
    let invalid = |error: InvalidIdentity| FrameDecodeError(error.to_string());
    let action = ControlAction::parse(&text(map, "action")?)
        .ok_or_else(|| FrameDecodeError("unknown control action".into()))?;
    let mut control = MissionControl::new(
        ControlId::new(text(map, "control_id")?).map_err(invalid)?,
        OperationId::new(text(map, "operation_id")?).map_err(invalid)?,
        MissionId::new(text(map, "mission_id")?).map_err(invalid)?,
        action,
    );
    if let Some(session_id) = optional_text(map, "session_id")? {
        control = control.with_session(SessionId::new(session_id).map_err(invalid)?);
    }
    let invalid_control = |error: InvalidControl| FrameDecodeError(error.to_string());
    if let Some(agent) = optional_text(map, "agent")? {
        control = control.with_agent(agent).map_err(invalid_control)?;
    }
    if let Some(message) = optional_text(map, "message")? {
        control = control.with_message(message).map_err(invalid_control)?;
    }
    Ok(control)
}

fn frames(
    channel_id: u64,
    schema: &Value,
    value: &Value,
) -> Result<(Vec<u8>, Vec<u8>), guatiao::value::ValueError> {
    Ok((
        channel::announce(channel_id, schema)?,
        channel::send(channel_id, value)?,
    ))
}

/// A terminal resize request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalResize {
    /// Provider-issued resumable operation identifier.
    session_id: SessionId,
    /// Monotonically increasing event cursor for this Session.
    sequence: u64,
    /// Number of character columns, greater than zero.
    columns: u16,
    /// Number of character rows, greater than zero.
    rows: u16,
}

impl TerminalResize {
    /// Creates a resize request, rejecting zero-sized terminals.
    pub fn new(
        session_id: SessionId,
        sequence: u64,
        columns: u16,
        rows: u16,
    ) -> Result<Self, InvalidSize> {
        if columns == 0 || rows == 0 {
            return Err(InvalidSize);
        }
        Ok(Self {
            session_id,
            sequence,
            columns,
            rows,
        })
    }

    /// The provider-issued Session ID.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The event sequence number.
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Number of terminal columns.
    pub const fn columns(&self) -> u16 {
        self.columns
    }

    /// Number of terminal rows.
    pub const fn rows(&self) -> u16 {
        self.rows
    }
}

/// A terminal frame exceeded the protocol limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FrameTooLarge;

impl fmt::Display for FrameTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("terminal frame exceeds the protocol limit")
    }
}

impl core::error::Error for FrameTooLarge {}

/// A terminal resize dimension was zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct InvalidSize;

impl fmt::Display for InvalidSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("terminal dimensions must be greater than zero")
    }
}

impl core::error::Error for InvalidSize {}

#[cfg(test)]
mod tests {
    use super::*;
    use guatiao::TryAsRef;

    #[test]
    fn mission_and_provider_session_are_separate_identities() {
        let snapshot = SessionSnapshot {
            operation_id: OperationId::new("ai6").unwrap(),
            mission_id: MissionId::new("mission-42").unwrap(),
            session_id: SessionId::new("provider-session-abc").unwrap(),
            state: SessionState::Working,
            sequence: 1,
        };
        assert_eq!(snapshot.mission_id.as_str(), "mission-42");
        assert_eq!(snapshot.session_id.as_str(), "provider-session-abc");
    }

    #[test]
    fn identities_reject_controls_and_unbounded_values() {
        assert!(MissionId::new("mission\n42").is_err());
        assert!(SessionId::new("x".repeat(MAX_ID_BYTES + 1)).is_err());
    }

    #[test]
    fn terminal_data_preserves_binary_and_enforces_bound() {
        let session = SessionId::new("provider-session").unwrap();
        let data = TerminalData::new(
            session.clone(),
            1,
            TerminalDirection::Output,
            vec![0xff, 0x00],
        )
        .unwrap();
        assert_eq!(data.bytes(), [0xff, 0x00]);
        assert!(format!("{data:?}").contains("[REDACTED; 2 bytes]"));
        assert!(!format!("{data:?}").contains("ff00"));
        assert!(
            TerminalData::new(
                session,
                2,
                TerminalDirection::Output,
                vec![0; MAX_TERMINAL_BYTES + 1]
            )
            .is_err()
        );
    }

    #[test]
    fn resize_dimensions_must_be_nonzero() {
        let session = SessionId::new("provider-session").unwrap();
        assert!(TerminalResize::new(session.clone(), 1, 0, 24).is_err());
        assert!(TerminalResize::new(session, 1, 80, 24).is_ok());
    }

    #[test]
    fn guatiao_frames_validate_lifecycle_and_keep_mission_distinct() {
        use guatiao::value::wire::channel::{Received, Receiver};

        let event = LifecycleEvent {
            operation_id: OperationId::new("station-a").unwrap(),
            mission_id: MissionId::new("mission-17").unwrap(),
            session_id: SessionId::new("provider-run-3").unwrap(),
            state: SessionState::Working,
            sequence: 4,
        };
        let (schema, value) = lifecycle_frames(&event).unwrap();
        let mut receiver = Receiver::new();
        assert!(matches!(
            receiver.accept(&schema).unwrap(),
            Received::Schema(1)
        ));
        let Received::Value { channel: 1, value } = receiver.accept(&value).unwrap() else {
            panic!("lifecycle channel must carry a schema-checked value");
        };
        let map: &Map = value.try_as_ref().unwrap();
        assert_eq!(
            <&str>::try_from(map.required("mission_id").unwrap()).unwrap(),
            "mission-17"
        );
        assert_eq!(
            <&str>::try_from(map.required("session_id").unwrap()).unwrap(),
            "provider-run-3"
        );
    }

    #[test]
    fn guatiao_terminal_frames_preserve_non_utf8_bytes() {
        use guatiao::value::wire::channel::{Received, Receiver};

        let event = TerminalData::new(
            SessionId::new("provider-run-3").unwrap(),
            5,
            TerminalDirection::Output,
            vec![0xff, 0x00, 0x80],
        )
        .unwrap();
        let (schema, value) = terminal_frames(&event).unwrap();
        let mut receiver = Receiver::new();
        receiver.accept(&schema).unwrap();
        let Received::Value { channel: 2, value } = receiver.accept(&value).unwrap() else {
            panic!("terminal channel must carry a schema-checked value");
        };
        let map: &Map = value.try_as_ref().unwrap();
        let bytes: &[u8] = map.required("bytes").unwrap().try_into().unwrap();
        assert_eq!(bytes, [0xff, 0x00, 0x80]);
    }

    #[test]
    fn guatiao_receiver_refuses_malformed_and_unknown_frames() {
        use guatiao::value::wire::channel::Receiver;

        let mut receiver = Receiver::new();
        assert!(receiver.accept(&[]).is_err());
        assert!(receiver.accept(&[0xff]).is_err());
    }

    #[test]
    fn protocol_receiver_decodes_lifecycle_terminal_and_resize_frames() {
        let event = LifecycleEvent {
            operation_id: OperationId::new("station-a").unwrap(),
            mission_id: MissionId::new("mission-17").unwrap(),
            session_id: SessionId::new("provider-run-3").unwrap(),
            state: SessionState::Blocked,
            sequence: 9,
        };
        let terminal = TerminalData::new(
            event.session_id.clone(),
            10,
            TerminalDirection::Output,
            vec![0xff, 0x00, 0x80],
        )
        .unwrap();
        let resize = TerminalResize::new(event.session_id.clone(), 11, 120, 40).unwrap();
        let (lifecycle_schema, lifecycle_value) = lifecycle_frames(&event).unwrap();
        let (terminal_schema, terminal_value) = terminal_frames(&terminal).unwrap();
        let (resize_schema, resize_value) = resize_frames(&resize).unwrap();
        let mut receiver = FrameReceiver::new();

        assert_eq!(receiver.accept(&lifecycle_schema).unwrap(), None);
        assert_eq!(
            receiver.accept(&lifecycle_value).unwrap(),
            Some(IncomingFrame::Lifecycle(event))
        );
        assert_eq!(receiver.accept(&terminal_schema).unwrap(), None);
        assert_eq!(
            receiver.accept(&terminal_value).unwrap(),
            Some(IncomingFrame::Terminal(terminal))
        );
        assert_eq!(receiver.accept(&resize_schema).unwrap(), None);
        assert_eq!(
            receiver.accept(&resize_value).unwrap(),
            Some(IncomingFrame::Resize(resize))
        );
    }

    #[test]
    fn a_parked_session_round_trips_through_the_receiver() {
        let event = LifecycleEvent {
            operation_id: OperationId::new("station-a").unwrap(),
            mission_id: MissionId::new("mission-17").unwrap(),
            session_id: SessionId::new("provider-run-3").unwrap(),
            state: SessionState::Parked,
            sequence: 4,
        };
        let (schema, value) = lifecycle_frames(&event).unwrap();
        let mut receiver = FrameReceiver::new();
        assert_eq!(receiver.accept(&schema).unwrap(), None);
        assert_eq!(
            receiver.accept(&value).unwrap(),
            Some(IncomingFrame::Lifecycle(event))
        );
    }

    #[test]
    fn a_quarantined_session_round_trips_and_matches_its_wire_vector() {
        let event = LifecycleEvent {
            operation_id: OperationId::new("station-a").unwrap(),
            mission_id: MissionId::new("mission-17").unwrap(),
            session_id: SessionId::new("provider-run-3").unwrap(),
            state: SessionState::Quarantined,
            sequence: 4,
        };
        let (schema, value) = lifecycle_frames(&event).unwrap();
        let mut receiver = FrameReceiver::new();
        assert_eq!(receiver.accept(&schema).unwrap(), None);
        assert_eq!(
            receiver.accept(&value).unwrap(),
            Some(IncomingFrame::Lifecycle(event))
        );
        let hex = value
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let expected = include_str!("../tests/vectors/frames.txt")
            .lines()
            .find_map(|line| line.strip_prefix("lifecycle_value_quarantined="))
            .unwrap();
        assert_eq!(hex, expected);
    }

    fn sample_control() -> MissionControl {
        MissionControl::new(
            ControlId::new("control-1").unwrap(),
            OperationId::new("station-a").unwrap(),
            MissionId::new("mission-17").unwrap(),
            ControlAction::Retry,
        )
        .with_session(SessionId::new("provider-run-3").unwrap())
        .with_agent("008")
        .unwrap()
    }

    #[test]
    fn a_control_round_trips_and_its_message_stays_out_of_debug_output() {
        let control = sample_control();
        let resume = MissionControl::new(
            ControlId::new("control-2").unwrap(),
            OperationId::new("station-a").unwrap(),
            MissionId::new("mission-17").unwrap(),
            ControlAction::Resume,
        )
        .with_message("carry on with the review")
        .unwrap();
        let mut receiver = FrameReceiver::new();
        for expected in [control, resume.clone()] {
            let (schema, value) = control_frames(&expected).unwrap();
            assert_eq!(receiver.accept(&schema).unwrap(), None);
            assert_eq!(
                receiver.accept(&value).unwrap(),
                Some(IncomingFrame::Control(expected))
            );
        }
        let debug = format!("{resume:?}");
        assert!(debug.contains("[REDACTED; 24 bytes]"));
        assert!(!debug.contains("carry on"));
    }

    #[test]
    fn control_fields_are_bounded() {
        let control = sample_control();
        assert!(control.clone().with_agent("").is_err());
        assert!(control.clone().with_agent("00\n7").is_err());
        assert!(control.clone().with_message("  ").is_err());
        assert!(
            control
                .with_message("x".repeat(MAX_CONTROL_MESSAGE_BYTES + 1))
                .is_err()
        );
        assert_eq!(ControlAction::parse("pause"), None);
        for action in ControlAction::ALL {
            assert_eq!(ControlAction::parse(action.as_str()), Some(action));
        }
    }

    #[test]
    fn every_session_state_round_trips_its_wire_spelling() {
        for state in [
            SessionState::Queued,
            SessionState::Working,
            SessionState::Blocked,
            SessionState::Stalled,
            SessionState::Parked,
            SessionState::Quarantined,
            SessionState::Done,
            SessionState::Failed,
            SessionState::Abandoned,
        ] {
            assert_eq!(SessionState::parse(state.as_str()), Some(state));
        }
        assert_eq!(SessionState::parse("paused"), None);
    }

    #[test]
    fn committed_wire_vectors_match_the_rust_encoder() {
        let event = LifecycleEvent {
            operation_id: OperationId::new("station-a").unwrap(),
            mission_id: MissionId::new("mission-17").unwrap(),
            session_id: SessionId::new("provider-run-3").unwrap(),
            state: SessionState::Working,
            sequence: 4,
        };
        let terminal = TerminalData::new(
            SessionId::new("provider-run-3").unwrap(),
            5,
            TerminalDirection::Output,
            vec![0xff, 0x00, 0x80],
        )
        .unwrap();
        let resize =
            TerminalResize::new(SessionId::new("provider-run-3").unwrap(), 6, 120, 40).unwrap();
        let lifecycle_frames = lifecycle_frames(&event).unwrap();
        let terminal_frames = terminal_frames(&terminal).unwrap();
        let resize_frames = resize_frames(&resize).unwrap();
        let control_frames = control_frames(&sample_control()).unwrap();
        let expected = include_str!("../tests/vectors/frames.txt");
        for line in expected.lines() {
            let (name, encoded) = line.split_once('=').unwrap();
            let actual = match name {
                "lifecycle_schema" => &lifecycle_frames.0,
                "lifecycle_value" => &lifecycle_frames.1,
                "terminal_schema" => &terminal_frames.0,
                "terminal_value" => &terminal_frames.1,
                "resize_schema" => &resize_frames.0,
                "resize_value" => &resize_frames.1,
                "control_schema" => &control_frames.0,
                "control_value" => &control_frames.1,
                "lifecycle_value_quarantined" => continue,
                _ => panic!("unknown vector name"),
            };
            let actual = actual
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(actual, encoded, "{name} must match the shared vector");
        }
    }
}
