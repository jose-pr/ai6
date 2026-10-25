//! Bounded, best-effort publication of AI6 Session events to Universal Exports.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

use crate::MissionId;
use crate::directorates::m::identity::SessionId;
use crate::directorates::q::terminal::TerminalInputTarget;
use ai6_diptel::{
    FrameReceiver, IncomingFrame, MissionId as GatewayMissionId, OperationId,
    SessionId as GatewaySessionId, TerminalData, TerminalDirection, TerminalResize, resize_frames,
    terminal_frames,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const PUBLISH_QUEUE_LIMIT: usize = 64;
const MAX_TERMINAL_BYTES: usize = 1024 * 1024;
const MAX_PRE_BIND_TERMINAL_BYTES: usize = MAX_TERMINAL_BYTES;
const INPUT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Universal Exports gateway credentials and origin for the daemon's event publisher.
pub struct Config {
    origin: Url,
    token: String,
}

impl Config {
    /// Validates an HTTPS origin, or an HTTP loopback origin, and its bearer token.
    pub fn try_new(origin: &str, token: impl Into<String>) -> Result<Self, PublisherError> {
        let authority_has_userinfo = origin
            .split_once("://")
            .and_then(|(_, rest)| rest.split('/').next())
            .is_some_and(|authority| authority.contains('@'));
        let origin = Url::parse(origin).map_err(|_| PublisherError::InvalidOrigin)?;
        if !matches!(origin.scheme(), "http" | "https")
            || !origin.username().is_empty()
            || origin.password().is_some()
            || authority_has_userinfo
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.host_str().is_none()
        {
            return Err(PublisherError::InvalidOrigin);
        }
        if origin.scheme() == "http" && !origin.host().is_some_and(is_loopback) {
            return Err(PublisherError::InvalidOrigin);
        }
        let token = token.into();
        if token.is_empty() || token.chars().any(char::is_control) {
            return Err(PublisherError::InvalidToken);
        }
        Ok(Self { origin, token })
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Config")
            .field("origin", &self.origin)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

/// Configuration errors for the optional Universal Exports publisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PublisherError {
    /// The gateway URL is not a bare HTTPS origin or a loopback HTTP origin.
    InvalidOrigin,
    /// The bearer token is empty or contains control characters.
    InvalidToken,
}

impl fmt::Display for PublisherError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidOrigin => "Universal Exports origin must be HTTPS or loopback HTTP",
            Self::InvalidToken => "Universal Exports token must be non-empty text without controls",
        })
    }
}

impl std::error::Error for PublisherError {}

fn is_loopback(host: Host<&str>) -> bool {
    match host {
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => address.is_loopback(),
        Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Health {
    pub(crate) enabled: bool,
    pub(crate) healthy: bool,
    pub(crate) error: Option<&'static str>,
}

enum Command {
    Register {
        operation_id: String,
        mission_id: String,
        session_id: SessionId,
        mission: Option<Box<MissionPublication>>,
    },
    Lifecycle {
        operation_id: String,
        mission_id: String,
        session_id: SessionId,
        state: &'static str,
    },
    Terminal {
        session_id: SessionId,
        bytes: Vec<u8>,
    },
    Wake,
}

/// One daemon-owned publisher thread, bounded so gateway outages cannot stall a Session.
pub(crate) struct Publisher {
    sender: SyncSender<Command>,
    running: Arc<AtomicBool>,
    healthy: Arc<AtomicBool>,
    error: Arc<Mutex<Option<&'static str>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    input_worker: Mutex<Option<JoinHandle<()>>>,
    input_targets: Arc<Mutex<BTreeMap<SessionId, InputTargetRegistration>>>,
}

struct InputTargetRegistration {
    operation_id: String,
    mission_id: String,
    target: Weak<dyn TerminalInputTarget>,
}

impl Publisher {
    pub(crate) fn start(config: Config) -> std::io::Result<Arc<Self>> {
        let input_origin = config.origin.clone();
        let input_token = config.token.clone();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .max_redirects(0)
            .build()
            .into();
        let (sender, receiver) = mpsc::sync_channel(PUBLISH_QUEUE_LIMIT);
        let running = Arc::new(AtomicBool::new(true));
        let healthy = Arc::new(AtomicBool::new(true));
        let error = Arc::new(Mutex::new(None));
        let input_targets = Arc::new(Mutex::new(BTreeMap::new()));
        let worker_running = running.clone();
        let worker_healthy = healthy.clone();
        let worker_error = error.clone();
        let worker = thread::Builder::new()
            .name("ai6-publisher".to_owned())
            .spawn(move || {
                publish_loop(
                    config,
                    agent,
                    receiver,
                    worker_running,
                    worker_healthy,
                    worker_error,
                )
            })?;
        let input_agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .max_redirects(0)
            .build()
            .into();
        let input_running = running.clone();
        let input_targets_for_worker = input_targets.clone();
        let input_worker = match thread::Builder::new()
            .name("ai6-publisher-input".to_owned())
            .spawn(move || {
                receive_inputs(
                    input_origin,
                    input_token,
                    input_agent,
                    input_running,
                    input_targets_for_worker,
                )
            }) {
            Ok(worker) => worker,
            Err(error) => {
                running.store(false, Ordering::Release);
                let _ = sender.try_send(Command::Wake);
                let _ = worker.join();
                return Err(error);
            }
        };
        Ok(Arc::new(Self {
            sender,
            running,
            healthy,
            error,
            worker: Mutex::new(Some(worker)),
            input_worker: Mutex::new(Some(input_worker)),
            input_targets,
        }))
    }

    pub(crate) fn session(
        self: &Arc<Self>,
        operation_id: String,
        mission_id: MissionId,
    ) -> Arc<SessionTelemetry> {
        Arc::new(SessionTelemetry {
            publisher: self.clone(),
            operation_id,
            mission_id,
            state: Mutex::new(SessionTelemetryState::default()),
        })
    }

    fn send(&self, command: Command) -> bool {
        if !self.healthy.load(Ordering::Acquire) {
            return false;
        }
        match self.sender.try_send(command) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.fail("publication queue is full");
                false
            }
            Err(TrySendError::Disconnected(_)) => {
                self.fail("publication worker stopped");
                false
            }
        }
    }

    fn fail(&self, reason: &'static str) {
        if self.healthy.swap(false, Ordering::AcqRel)
            && let Ok(mut error) = self.error.lock()
        {
            *error = Some(reason);
        }
    }

    fn register_input_target(
        &self,
        session_id: SessionId,
        operation_id: &str,
        mission_id: &str,
        target: &Arc<dyn TerminalInputTarget>,
    ) {
        let Ok(mut targets) = self.input_targets.lock() else {
            self.fail("Universal Exports input target lock failed");
            return;
        };
        targets.insert(
            session_id,
            InputTargetRegistration {
                operation_id: operation_id.to_owned(),
                mission_id: mission_id.to_owned(),
                target: Arc::downgrade(target),
            },
        );
    }

    fn unregister_input_target(
        &self,
        session_id: &SessionId,
        target: &Arc<dyn TerminalInputTarget>,
    ) {
        let Ok(mut targets) = self.input_targets.lock() else {
            return;
        };
        if targets
            .get(session_id)
            .is_some_and(|registration| Weak::ptr_eq(&registration.target, &Arc::downgrade(target)))
        {
            targets.remove(session_id);
        }
    }

    pub(crate) fn health(&self) -> Health {
        Health {
            enabled: true,
            healthy: self.healthy.load(Ordering::Acquire),
            error: self.error.lock().ok().and_then(|error| *error),
        }
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        let _ = self.sender.try_send(Command::Wake);
        if let Ok(worker) = self.worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
        if let Ok(worker) = self.input_worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

/// Session-scoped bridge shared by a handler's terminal and lifecycle event pump.
pub(crate) struct SessionTelemetry {
    publisher: Arc<Publisher>,
    operation_id: String,
    mission_id: MissionId,
    state: Mutex<SessionTelemetryState>,
}

#[derive(Default)]
struct SessionTelemetryState {
    session_id: Option<SessionId>,
    terminal_target: Option<Arc<dyn TerminalInputTarget>>,
    pending_terminal: VecDeque<Vec<u8>>,
    pending_terminal_bytes: usize,
    mission: Option<MissionPublication>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MissionPublication {
    pub(crate) prompt: String,
    pub(crate) agent_id: Option<String>,
    pub(crate) role_id: Option<String>,
    pub(crate) handler_provider: String,
    pub(crate) terminal_provider: String,
    pub(crate) working_directory: String,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    pub(crate) environment_keys: Vec<String>,
    pub(crate) station_id: Option<String>,
    pub(crate) theater_id: Option<String>,
    pub(crate) mode: crate::directorates::m::placement::ExecutionMode,
}

impl SessionTelemetry {
    pub(crate) fn bind(&self, session_id: SessionId) {
        let Ok(mut state) = self.state.lock() else {
            self.publisher.fail("Session telemetry lock failed");
            return;
        };
        if state.session_id.as_ref() == Some(&session_id) {
            return;
        }
        if state.session_id.is_some() {
            self.publisher.fail("provider changed its Session ID");
            return;
        }
        if self.publisher.send(Command::Register {
            operation_id: self.operation_id.clone(),
            mission_id: self.mission_id.to_string(),
            session_id: session_id.clone(),
            mission: state.mission.clone().map(Box::new),
        }) {
            state.session_id = Some(session_id.clone());
            if let Some(target) = &state.terminal_target {
                self.publisher.register_input_target(
                    session_id.clone(),
                    &self.operation_id,
                    self.mission_id.as_str(),
                    target,
                );
            }
            state.pending_terminal_bytes = 0;
            while let Some(bytes) = state.pending_terminal.pop_front() {
                if !self.publisher.send(Command::Terminal {
                    session_id: session_id.clone(),
                    bytes,
                }) {
                    state.pending_terminal.clear();
                    break;
                }
            }
        }
    }

    pub(crate) fn set_mission_publication(&self, mission: MissionPublication) {
        if let Ok(mut state) = self.state.lock() {
            state.mission = Some(mission);
        } else {
            self.publisher.fail("Session telemetry lock failed");
        }
    }

    pub(crate) fn attach_terminal_target(&self, target: Arc<dyn TerminalInputTarget>) {
        let Ok(mut state) = self.state.lock() else {
            self.publisher.fail("Session telemetry lock failed");
            return;
        };
        state.terminal_target = Some(target.clone());
        if let Some(session_id) = &state.session_id {
            self.publisher.register_input_target(
                session_id.clone(),
                &self.operation_id,
                self.mission_id.as_str(),
                &target,
            );
        }
    }

    pub(crate) fn publish_lifecycle(&self, state: &'static str) {
        let Ok(telemetry) = self.state.lock() else {
            self.publisher.fail("Session telemetry lock failed");
            return;
        };
        if let Some(session_id) = &telemetry.session_id {
            self.publisher.send(Command::Lifecycle {
                operation_id: self.operation_id.clone(),
                mission_id: self.mission_id.to_string(),
                session_id: session_id.clone(),
                state,
            });
        }
    }

    pub(crate) fn publish_terminal(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            self.publisher.fail("Session telemetry lock failed");
            return;
        };
        for chunk in bytes.chunks(MAX_TERMINAL_BYTES) {
            if let Some(session_id) = &state.session_id {
                if !self.publisher.send(Command::Terminal {
                    session_id: session_id.clone(),
                    bytes: chunk.to_vec(),
                }) {
                    break;
                }
            } else if state.pending_terminal_bytes.saturating_add(chunk.len())
                <= MAX_PRE_BIND_TERMINAL_BYTES
            {
                state.pending_terminal.push_back(chunk.to_vec());
                state.pending_terminal_bytes += chunk.len();
            } else {
                self.publisher
                    .fail("pre-registration terminal buffer is full");
                state.pending_terminal.clear();
                state.pending_terminal_bytes = 0;
                break;
            }
        }
    }
}

impl Drop for SessionTelemetry {
    fn drop(&mut self) {
        let Ok(state) = self.state.get_mut() else {
            return;
        };
        if let (Some(session_id), Some(target)) = (&state.session_id, &state.terminal_target) {
            self.publisher.unregister_input_target(session_id, target);
        }
    }
}

fn receive_inputs(
    origin: Url,
    token: String,
    agent: ureq::Agent,
    running: Arc<AtomicBool>,
    targets: Arc<Mutex<BTreeMap<SessionId, InputTargetRegistration>>>,
) {
    let Ok(endpoint) = origin.join("v1/station/inputs/next") else {
        return;
    };
    while running.load(Ordering::Acquire) {
        let response = agent
            .get(endpoint.as_str())
            .header("Authorization", &format!("Bearer {token}"))
            .call();
        let Ok(mut response) = response else {
            thread::sleep(INPUT_RETRY_DELAY);
            continue;
        };
        if response.status() != 200 {
            thread::sleep(INPUT_RETRY_DELAY);
            continue;
        }
        let operation_id = response
            .headers()
            .get("x-ai6-operation-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| OperationId::new(value).ok());
        let mission_id = response
            .headers()
            .get("x-ai6-mission-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| GatewayMissionId::new(value).ok());
        let frame = response
            .body_mut()
            .with_config()
            .limit((MAX_TERMINAL_BYTES + 64 * 1024) as u64)
            .read_to_vec();
        let (Some(operation_id), Some(mission_id), Ok(frame)) = (operation_id, mission_id, frame)
        else {
            thread::sleep(INPUT_RETRY_DELAY);
            continue;
        };
        if let Some((session_id, _sequence, event)) = decode_input_frame(&frame) {
            dispatch_input(&targets, &operation_id, &mission_id, &session_id, event);
        }
    }
}

enum TerminalInput {
    Write(Vec<u8>),
    Resize { rows: u16, columns: u16 },
}

fn decode_input_frame(frame: &[u8]) -> Option<(SessionId, u64, TerminalInput)> {
    let template_session = GatewaySessionId::new("input-schema").ok()?;
    let terminal_template = TerminalData::new(
        template_session.clone(),
        1,
        TerminalDirection::Input,
        Vec::new(),
    )
    .ok()?;
    let resize_template = TerminalResize::new(template_session, 1, 80, 24).ok()?;
    let (terminal_schema, _) = terminal_frames(&terminal_template).ok()?;
    let (resize_schema, _) = resize_frames(&resize_template).ok()?;
    let mut receiver = FrameReceiver::new();
    receiver.accept(&terminal_schema).ok()?;
    receiver.accept(&resize_schema).ok()?;
    match receiver.accept(frame).ok()?? {
        IncomingFrame::Terminal(data)
            if data.direction() == TerminalDirection::Input && data.sequence() > 0 =>
        {
            Some((
                SessionId::try_new(data.session_id().as_str()).ok()?,
                data.sequence(),
                TerminalInput::Write(data.bytes().to_vec()),
            ))
        }
        IncomingFrame::Resize(resize) if resize.sequence() > 0 => Some((
            SessionId::try_new(resize.session_id().as_str()).ok()?,
            resize.sequence(),
            TerminalInput::Resize {
                rows: resize.rows(),
                columns: resize.columns(),
            },
        )),
        IncomingFrame::Lifecycle(_)
        | IncomingFrame::Terminal(_)
        | IncomingFrame::Resize(_)
        | IncomingFrame::Closed(_) => None,
    }
}

fn dispatch_input(
    targets: &Arc<Mutex<BTreeMap<SessionId, InputTargetRegistration>>>,
    operation_id: &OperationId,
    mission_id: &GatewayMissionId,
    session_id: &SessionId,
    input: TerminalInput,
) {
    let target = {
        let Ok(mut targets) = targets.lock() else {
            return;
        };
        targets.retain(|_, registration| registration.target.strong_count() > 0);
        let Some(registration) = targets.get_mut(session_id) else {
            return;
        };
        if registration.operation_id != operation_id.as_str()
            || registration.mission_id != mission_id.as_str()
        {
            return;
        }
        let Some(target) = registration.target.upgrade() else {
            return;
        };
        target
    };
    let result = match input {
        TerminalInput::Write(bytes) => target.write(&bytes),
        TerminalInput::Resize { rows, columns } => {
            target.resize(i64::from(rows), i64::from(columns))
        }
    };
    if result.is_err()
        && let Ok(mut targets) = targets.lock()
        && targets.get(session_id).is_some_and(|registration| {
            Weak::ptr_eq(&registration.target, &Arc::downgrade(&target))
        })
    {
        targets.remove(session_id);
    }
}

fn publish_loop(
    config: Config,
    agent: ureq::Agent,
    receiver: Receiver<Command>,
    running: Arc<AtomicBool>,
    healthy: Arc<AtomicBool>,
    error: Arc<Mutex<Option<&'static str>>>,
) {
    let mut sequences = BTreeMap::new();
    while running.load(Ordering::Acquire) && healthy.load(Ordering::Acquire) {
        let command = match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let result = match command {
            Command::Register {
                operation_id,
                mission_id,
                session_id,
                mission,
            } => register(
                &agent,
                &config,
                &operation_id,
                &mission_id,
                &session_id,
                mission.as_deref(),
            )
            .map(|sequence| {
                sequences.insert(session_id, sequence);
            }),
            Command::Lifecycle {
                operation_id,
                mission_id,
                session_id,
                state,
            } => publish_lifecycle(
                &agent,
                &config,
                &operation_id,
                &mission_id,
                &session_id,
                state,
                &mut sequences,
            ),
            Command::Terminal { session_id, bytes } => {
                publish_terminal(&agent, &config, &session_id, &bytes, &mut sequences)
            }
            Command::Wake => continue,
        };
        if let Err(reason) = result {
            if healthy.swap(false, Ordering::AcqRel)
                && let Ok(mut current_error) = error.lock()
            {
                *current_error = Some(reason);
            }
            break;
        }
    }
}

#[derive(Serialize)]
struct Registration<'a> {
    operation_id: &'a str,
    mission_id: &'a str,
    session_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    mission: Option<&'a MissionPublication>,
}

#[derive(Deserialize)]
struct RegistrationResponse {
    next_sequence: u64,
}

#[derive(Serialize)]
struct Lifecycle<'a> {
    operation_id: &'a str,
    mission_id: &'a str,
    state: &'a str,
}

fn register(
    agent: &ureq::Agent,
    config: &Config,
    operation_id: &str,
    mission_id: &str,
    session_id: &SessionId,
    mission: Option<&MissionPublication>,
) -> Result<u64, &'static str> {
    let url = config
        .origin
        .join("v1/station/sessions")
        .map_err(|_| "invalid registration endpoint")?;
    let mut response = agent
        .post(url.as_str())
        .header("Authorization", &format!("Bearer {}", config.token))
        .send_json(Registration {
            operation_id,
            mission_id,
            session_id: session_id.as_str(),
            mission,
        })
        .map_err(|_| "registration request failed")?;
    if response.status() != 201 {
        return Err("gateway rejected Session registration");
    }
    let registration: RegistrationResponse = response
        .body_mut()
        .read_json()
        .map_err(|_| "gateway returned an invalid registration response")?;
    if registration.next_sequence == 0 {
        return Err("gateway returned an invalid event sequence");
    }
    Ok(registration.next_sequence)
}

fn publish_lifecycle(
    agent: &ureq::Agent,
    config: &Config,
    operation_id: &str,
    mission_id: &str,
    session_id: &SessionId,
    state: &str,
    sequences: &mut BTreeMap<SessionId, u64>,
) -> Result<(), &'static str> {
    let sequence = take_sequence(sequences, session_id)?;
    let url = session_endpoint(&config.origin, session_id, "lifecycle", sequence)?;
    let mut response = agent
        .post(url.as_str())
        .header("Authorization", &format!("Bearer {}", config.token))
        .send_json(Lifecycle {
            operation_id,
            mission_id,
            state,
        })
        .map_err(|_| "lifecycle request failed")?;
    if response.status() != 202 {
        return Err("gateway rejected a lifecycle event");
    }
    response
        .body_mut()
        .read_to_vec()
        .map_err(|_| "gateway response could not be read")?;
    Ok(())
}

fn publish_terminal(
    agent: &ureq::Agent,
    config: &Config,
    session_id: &SessionId,
    bytes: &[u8],
    sequences: &mut BTreeMap<SessionId, u64>,
) -> Result<(), &'static str> {
    let sequence = take_sequence(sequences, session_id)?;
    let url = session_endpoint(&config.origin, session_id, "terminal", sequence)?;
    let mut response = agent
        .post(url.as_str())
        .header("Authorization", &format!("Bearer {}", config.token))
        .content_type("application/octet-stream")
        .send(bytes)
        .map_err(|_| "terminal output request failed")?;
    if response.status() != 202 {
        return Err("gateway rejected terminal output");
    }
    response
        .body_mut()
        .read_to_vec()
        .map_err(|_| "gateway response could not be read")?;
    Ok(())
}

fn take_sequence(
    sequences: &mut BTreeMap<SessionId, u64>,
    session_id: &SessionId,
) -> Result<u64, &'static str> {
    let sequence = sequences
        .get_mut(session_id)
        .ok_or("Session was not registered")?;
    let current = *sequence;
    *sequence = sequence
        .checked_add(1)
        .ok_or("Session event sequence is exhausted")?;
    Ok(current)
}

fn session_endpoint(
    origin: &Url,
    session_id: &SessionId,
    kind: &str,
    sequence: u64,
) -> Result<Url, &'static str> {
    let mut url = origin
        .join("v1/station/sessions/")
        .map_err(|_| "invalid Session endpoint")?;
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| "invalid Session endpoint")?;
    segments
        .pop_if_empty()
        .push(session_id.as_str())
        .push(kind)
        .push(&sequence.to_string());
    drop(segments);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::q::terminal::{
        ExitInfo, READ_TIMED_OUT, Spec, Terminal, TerminalBridge, TerminalSession,
    };
    use guatiao::ToValue;
    use guatiao::library::{Object, ProviderError};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    #[derive(Debug, PartialEq, Eq)]
    enum RecordedInput {
        Write(Vec<u8>),
        Resize(i64, i64),
    }

    struct RecordingTerminal(mpsc::Sender<RecordedInput>);

    impl Terminal for RecordingTerminal {
        fn spawn(
            &self,
            _spec: &guatiao::Value,
        ) -> Result<Object<dyn TerminalSession>, ProviderError> {
            Ok(RecordingTerminalSession(self.0.clone()).into_object())
        }
    }

    struct RecordingTerminalSession(mpsc::Sender<RecordedInput>);

    impl TerminalSession for RecordingTerminalSession {
        fn pid(&self) -> i64 {
            18
        }

        fn read(&self, _dst: &mut [u8], _timeout_ms: i64) -> Result<i64, ProviderError> {
            Ok(READ_TIMED_OUT)
        }

        fn write(&self, data: &[u8]) -> Result<(), ProviderError> {
            self.0
                .send(RecordedInput::Write(data.to_vec()))
                .map_err(|error| {
                    ProviderError::new(guatiao::Status::GUATIAO_ERR_INTERNAL, &error.to_string())
                })
        }

        fn resize(&self, rows: i64, columns: i64) -> Result<(), ProviderError> {
            self.0
                .send(RecordedInput::Resize(rows, columns))
                .map_err(|error| {
                    ProviderError::new(guatiao::Status::GUATIAO_ERR_INTERNAL, &error.to_string())
                })
        }

        fn kill(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn exit_status(&self, _timeout_ms: i64) -> Result<guatiao::Value, ProviderError> {
            ExitInfo::default().to_value().map_err(|error| {
                ProviderError::new(guatiao::Status::GUATIAO_ERR_INTERNAL, &error.to_string())
            })
        }
    }

    struct RawOutputTerminal;

    impl Terminal for RawOutputTerminal {
        fn spawn(
            &self,
            _spec: &guatiao::Value,
        ) -> Result<Object<dyn TerminalSession>, ProviderError> {
            Ok(RawOutputSession(Mutex::new(false)).into_object())
        }
    }

    struct RawOutputSession(Mutex<bool>);

    impl TerminalSession for RawOutputSession {
        fn pid(&self) -> i64 {
            17
        }

        fn read(&self, dst: &mut [u8], _timeout_ms: i64) -> Result<i64, ProviderError> {
            let mut emitted = self.0.lock().unwrap();
            if *emitted {
                return Ok(0);
            }
            let output = [0, 255, 10];
            dst[..output.len()].copy_from_slice(&output);
            *emitted = true;
            Ok(output.len() as i64)
        }

        fn write(&self, _data: &[u8]) -> Result<(), ProviderError> {
            Ok(())
        }

        fn resize(&self, _rows: i64, _cols: i64) -> Result<(), ProviderError> {
            Ok(())
        }

        fn kill(&self) -> Result<(), ProviderError> {
            Ok(())
        }

        fn exit_status(&self, _timeout_ms: i64) -> Result<guatiao::Value, ProviderError> {
            ExitInfo::default().to_value().map_err(|error| {
                ProviderError::new(guatiao::Status::GUATIAO_ERR_INTERNAL, &error.to_string())
            })
        }
    }

    struct Request {
        path: String,
        headers: String,
        body: Vec<u8>,
    }

    fn gateway() -> (String, Receiver<Request>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut outbound_count = 0;
            let mut received_input_poll = false;
            while outbound_count < 3 || !received_input_poll {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    headers.push_str(&line);
                }
                let body_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or_default();
                let mut body = vec![0; body_length];
                reader.read_exact(&mut body).unwrap();
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                let response = if path == "/v1/station/inputs/next" {
                    received_input_poll = true;
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    outbound_count += 1;
                    let response = if path == "/v1/station/sessions" {
                        "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"next_sequence\":41}"
                    } else {
                        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    };
                    sender
                        .send(Request {
                            path,
                            headers,
                            body,
                        })
                        .unwrap();
                    response
                };
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://{address}"), receiver, worker)
    }

    #[test]
    fn origin_requires_https_or_loopback_http() {
        assert!(Config::try_new("https://gateway.example", "secret").is_ok());
        assert!(Config::try_new("http://127.0.0.1:7082", "secret").is_ok());
        assert!(Config::try_new("http://[::1]:7082", "secret").is_ok());
        assert!(Config::try_new("http://gateway.example", "secret").is_err());
        assert!(Config::try_new("https://user@gateway.example", "secret").is_err());
        assert!(Config::try_new("https://@gateway.example", "secret").is_err());
        assert!(Config::try_new("https://gateway.example/api", "secret").is_err());
        assert!(Config::try_new("https://gateway.example", "").is_err());
        assert!(Config::try_new("https://gateway.example", "bad\ntoken").is_err());
    }

    #[test]
    fn debug_output_redacts_the_token() {
        let config = Config::try_new("https://gateway.example", "private-token").unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("private-token"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn publisher_registers_then_sequences_lifecycle_and_binary_output() {
        let (origin, requests, gateway) = gateway();
        let config = Config::try_new(&origin, "test-token").unwrap();
        let publisher = Publisher::start(config).unwrap();
        let telemetry = publisher.session(
            "operation-a".to_owned(),
            MissionId::try_new("mission-a").unwrap(),
        );
        telemetry.set_mission_publication(MissionPublication {
            prompt: "Inspect this change".to_owned(),
            agent_id: Some("analyst".to_owned()),
            role_id: Some("case-officer".to_owned()),
            handler_provider: "claude".to_owned(),
            terminal_provider: "pty".to_owned(),
            working_directory: "C:/operations/a".to_owned(),
            model: Some("sonnet".to_owned()),
            effort: Some("high".to_owned()),
            environment_keys: vec!["API_TOKEN".to_owned()],
            station_id: Some("hq".to_owned()),
            theater_id: Some("local".to_owned()),
            mode: crate::directorates::m::placement::ExecutionMode::Field,
        });
        let terminal = TerminalBridge::agent_terminal_with_watchdog_and_telemetry(
            Box::new(RawOutputTerminal),
            telemetry.clone(),
            crate::directorates::medical::watchdog::Watchdog::default(),
        );
        let spec = Spec {
            argv: vec!["mock".to_owned()],
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        }
        .to_value()
        .unwrap();
        let session = terminal.spawn(&spec).unwrap();
        let mut output = [0; 8];
        assert_eq!(session.read(&mut output, 100).unwrap(), 3);
        assert_eq!(&output[..3], &[0, 255, 10]);
        telemetry.bind(SessionId::try_new("session/a").unwrap());
        telemetry.publish_lifecycle("working");

        let registration = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        let terminal = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        let lifecycle = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        gateway.join().unwrap();

        assert_eq!(registration.path, "/v1/station/sessions");
        assert!(registration.headers.contains("Bearer test-token"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&registration.body).unwrap(),
            serde_json::json!({
                "operation_id": "operation-a",
                "mission_id": "mission-a",
                "session_id": "session/a",
                "mission": {
                    "prompt": "Inspect this change",
                    "agent_id": "analyst",
                    "role_id": "case-officer",
                    "handler_provider": "claude",
                    "terminal_provider": "pty",
                    "working_directory": "C:/operations/a",
                    "model": "sonnet",
                    "effort": "high",
                    "environment_keys": ["API_TOKEN"],
                    "station_id": "hq",
                    "theater_id": "local",
                    "mode": "field"
                }
            })
        );
        assert_eq!(
            terminal.path,
            "/v1/station/sessions/session%2Fa/terminal/41"
        );
        assert_eq!(terminal.body, [0, 255, 10]);
        assert_eq!(
            lifecycle.path,
            "/v1/station/sessions/session%2Fa/lifecycle/42"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&lifecycle.body).unwrap(),
            serde_json::json!({
                "operation_id": "operation-a",
                "mission_id": "mission-a",
                "state": "working"
            })
        );
        assert!(publisher.health().healthy);
    }

    #[test]
    fn authenticated_gateway_input_reaches_the_bound_provider_terminal() {
        let session_id = GatewaySessionId::new("session-input").unwrap();
        let (_, terminal_frame) = terminal_frames(
            &TerminalData::new(
                session_id.clone(),
                1,
                TerminalDirection::Input,
                vec![0, 255, 10],
            )
            .unwrap(),
        )
        .unwrap();
        let (_, resize_frame) =
            resize_frames(&TerminalResize::new(session_id, 2, 120, 40).unwrap()).unwrap();
        let frames = [
            (terminal_frame.clone(), "operation-other"),
            (terminal_frame, "operation-a"),
            (resize_frame, "operation-a"),
        ];
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        let gateway_ready = ready.clone();
        let gateway = thread::spawn(move || {
            let mut input_index = 0;
            while input_index < frames.len() {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    headers.push_str(&line);
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or_default();
                if path == "/v1/station/sessions" {
                    let body_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    let mut body = vec![0; body_length];
                    reader.read_exact(&mut body).unwrap();
                    stream
                        .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"next_sequence\":41}")
                        .unwrap();
                    gateway_ready.store(true, Ordering::Release);
                } else if path == "/v1/station/inputs/next" && gateway_ready.load(Ordering::Acquire)
                {
                    assert!(headers.contains("Bearer test-token"));
                    let (frame, operation_id) = &frames[input_index];
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.ai6.guatiao\r\nX-AI6-Operation-Id: {operation_id}\r\nX-AI6-Mission-Id: mission-a\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        frame.len(),
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                    stream.write_all(frame).unwrap();
                    input_index += 1;
                } else {
                    stream
                        .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .unwrap();
                }
            }
        });

        let config = Config::try_new(&format!("http://{address}"), "test-token").unwrap();
        let publisher = Publisher::start(config).unwrap();
        let telemetry = publisher.session(
            "operation-a".to_owned(),
            MissionId::try_new("mission-a").unwrap(),
        );
        let (sender, inputs) = mpsc::channel();
        let terminal = TerminalBridge::agent_terminal_with_watchdog_and_telemetry(
            Box::new(RecordingTerminal(sender)),
            telemetry.clone(),
            crate::directorates::medical::watchdog::Watchdog::default(),
        );
        let spec = Spec {
            argv: vec!["mock".to_owned()],
            cwd: None,
            env: None,
            rows: 24,
            cols: 80,
        }
        .to_value()
        .unwrap();
        let _session = terminal.spawn(&spec).unwrap();
        telemetry.bind(SessionId::try_new("session-input").unwrap());

        assert_eq!(
            inputs.recv_timeout(Duration::from_secs(3)).unwrap(),
            RecordedInput::Write(vec![0, 255, 10])
        );
        assert_eq!(
            inputs.recv_timeout(Duration::from_secs(3)).unwrap(),
            RecordedInput::Resize(40, 120)
        );
        gateway.join().unwrap();
    }

    #[test]
    fn unbound_terminal_buffer_fails_closed_at_its_limit() {
        let config = Config::try_new("http://127.0.0.1:7082", "test-token").unwrap();
        let publisher = Publisher::start(config).unwrap();
        let telemetry = publisher.session(
            "operation-a".to_owned(),
            MissionId::try_new("mission-a").unwrap(),
        );
        telemetry.publish_terminal(&vec![0; MAX_PRE_BIND_TERMINAL_BYTES + 1]);

        let health = publisher.health();
        assert!(!health.healthy);
        assert_eq!(
            health.error,
            Some("pre-registration terminal buffer is full")
        );
    }
}
