//! Bounded, best-effort publication of AI6 Session events to Universal Exports.
//!
//! A gateway that goes away costs a Session nothing: what it writes waits here, within `[publisher]
//! queue`, and the worker tries again with growing pauses. Universal Exports holds its Sessions in
//! memory, so when it answers again each live Session is registered again and its latest state
//! sent. Output past the queue is dropped, oldest first, and the Session's sequence skips a number
//! there, which tells the gateway and its viewers that something is missing.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ureq::tls::TlsConfig;
use url::{Host, Url};

use crate::MissionId;
use crate::directorates::m::identity::SessionId;
use crate::directorates::q::terminal::TerminalInputTarget;
use crate::station::limits::PublisherLimits;
use ai6_diptel::{
    ControlAction, ControlId, FrameReceiver, IncomingFrame, MAX_CONTROL_MESSAGE_BYTES,
    MissionControl, MissionId as GatewayMissionId, OperationId, SessionId as GatewaySessionId,
    TerminalData, TerminalDirection, TerminalResize, control_frames, resize_frames,
    terminal_frames,
};

/// Room for a frame's header past the terminal bytes it carries.
pub(crate) const FRAME_MARGIN: u64 = 64 * 1024;

/// Universal Exports gateway credentials and origin for the daemon's event publisher.
pub struct Config {
    origin: Url,
    token: String,
    limits: PublisherLimits,
    tls: TlsConfig,
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
        Ok(Self {
            origin,
            token,
            limits: PublisherLimits::default(),
            tls: TlsConfig::default(),
        })
    }

    /// Trusts `certificates` (DER, as [`read_ca_file`](crate::bootstrap::read_ca_file) returns
    /// them) for Universal Exports besides the public roots, on every link of the Station to
    /// it. Without this call only the public roots are trusted.
    #[must_use]
    pub fn with_ca_certificates(mut self, certificates: &[Vec<u8>]) -> Self {
        self.tls = crate::tls::trusting(certificates);
        self
    }

    /// Publishes within `limits`, the `station` record's `[publisher]` table.
    pub(crate) fn with_limits(mut self, limits: PublisherLimits) -> Self {
        self.limits = limits;
        self
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

/// An agent for one link to Universal Exports: no redirects, the Station's trust, and at most
/// `timeout` per request.
pub(crate) fn gateway_agent(tls: &TlsConfig, timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .tls_config(tls.clone())
        .build()
        .into()
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Health {
    pub(crate) enabled: bool,
    pub(crate) healthy: bool,
    pub(crate) error: Option<&'static str>,
    /// Bytes of terminal output dropped since the Station started, because they did not fit
    /// while Universal Exports was away or could not keep up.
    pub(crate) lost_terminal_bytes: u64,
}

enum Command {
    Register {
        operation_id: String,
        mission_id: String,
        session_id: SessionId,
        mission: Option<Box<MissionPublication>>,
    },
    Lifecycle {
        session_id: SessionId,
        state: &'static str,
        reason: Option<&'static str>,
    },
    Terminal {
        session_id: SessionId,
        bytes: Vec<u8>,
    },
    /// A holder of the Session's telemetry has gone: once none is left and what came before this
    /// is out, the Session is not registered again.
    Forget { session_id: SessionId },
}

impl Command {
    fn session_id(&self) -> &SessionId {
        match self {
            Self::Register { session_id, .. }
            | Self::Lifecycle { session_id, .. }
            | Self::Terminal { session_id, .. }
            | Self::Forget { session_id } => session_id,
        }
    }
}

/// What waits for the publish worker. `lost` names the Sessions with output dropped since their
/// last event went out.
#[derive(Default)]
struct Waiting {
    commands: VecDeque<Command>,
    lost: BTreeMap<SessionId, u64>,
}

/// The queue between a Station's Sessions and its publish worker. A Session never waits on it.
struct Outbox {
    limits: PublisherLimits,
    waiting: Mutex<Waiting>,
    ready: Condvar,
    lost_bytes: AtomicU64,
}

impl Outbox {
    fn new(limits: PublisherLimits) -> Self {
        Self {
            limits,
            waiting: Mutex::default(),
            ready: Condvar::new(),
            lost_bytes: AtomicU64::new(0),
        }
    }

    /// Queues `command`. Past `queue` commands, a lifecycle replaces its Session's earlier one and
    /// output gives way oldest first; a registration or lifecycle always gets in, at most one per
    /// Session stays waiting, so a Session's latest state is never what is dropped.
    fn push(&self, command: Command) -> bool {
        let Ok(mut waiting) = self.waiting.lock() else {
            return false;
        };
        let room = self.limits.queue;
        match command {
            Command::Terminal { session_id, bytes } => {
                let frame = self.limits.terminal_bytes;
                // Consecutive reads of one Session travel as one frame.
                if let Some(Command::Terminal {
                    session_id: tail,
                    bytes: held,
                }) = waiting.commands.back_mut()
                    && *tail == session_id
                    && frame.allows(held.len() + bytes.len())
                {
                    held.extend_from_slice(&bytes);
                } else if waiting.commands.len() < room || self.evict_output(&mut waiting) {
                    waiting
                        .commands
                        .push_back(Command::Terminal { session_id, bytes });
                } else {
                    self.note_lost(&mut waiting, session_id, bytes.len());
                }
            }
            command => {
                if waiting.commands.len() >= room {
                    if let Command::Lifecycle { session_id, .. } = &command {
                        let earlier = waiting.commands.iter().position(|queued| {
                            matches!(queued, Command::Lifecycle { .. })
                                && queued.session_id() == session_id
                        });
                        if let Some(earlier) = earlier {
                            waiting.commands.remove(earlier);
                        }
                    }
                    if waiting.commands.len() >= room {
                        self.evict_output(&mut waiting);
                    }
                }
                waiting.commands.push_back(command);
            }
        }
        self.ready.notify_one();
        true
    }

    /// Drops the oldest output waiting; `false` when none does.
    fn evict_output(&self, waiting: &mut Waiting) -> bool {
        let Some(oldest) = waiting
            .commands
            .iter()
            .position(|queued| matches!(queued, Command::Terminal { .. }))
        else {
            return false;
        };
        if let Some(Command::Terminal { session_id, bytes }) = waiting.commands.remove(oldest) {
            self.note_lost(waiting, session_id, bytes.len());
        }
        true
    }

    fn note_lost(&self, waiting: &mut Waiting, session_id: SessionId, bytes: usize) {
        *waiting.lost.entry(session_id).or_default() += bytes as u64;
        self.lost_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// Records output dropped before it was queued.
    fn lost(&self, session_id: SessionId, bytes: usize) {
        if let Ok(mut waiting) = self.waiting.lock() {
            self.note_lost(&mut waiting, session_id, bytes);
        }
    }

    /// The next command, and whether output of its Session was dropped; `None` after `wait` with
    /// nothing.
    fn next(&self, wait: Duration) -> Option<(Command, bool)> {
        let mut waiting = self.waiting.lock().ok()?;
        if waiting.commands.is_empty() {
            waiting = self.ready.wait_timeout(waiting, wait).ok()?.0;
        }
        let command = waiting.commands.pop_front()?;
        let lost = waiting.lost.remove(command.session_id()).is_some();
        Some((command, lost))
    }
}

/// One daemon-owned publisher thread, bounded so gateway outages cannot stall a Session.
pub(crate) struct Publisher {
    origin: Url,
    token: String,
    limits: PublisherLimits,
    tls: TlsConfig,
    outbox: Arc<Outbox>,
    running: Arc<AtomicBool>,
    healthy: Arc<AtomicBool>,
    error: Arc<Mutex<Option<&'static str>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    input_worker: Mutex<Option<JoinHandle<()>>>,
    control_worker: Mutex<Option<JoinHandle<()>>>,
    input_targets: Arc<Mutex<BTreeMap<SessionId, InputTargetRegistration>>>,
}

/// One operator control taken from Universal Exports and the channel its outcome goes back on.
pub(crate) struct ControlDelivery {
    pub(crate) control: MissionControl,
    pub(crate) outcome: tokio::sync::oneshot::Sender<ControlOutcome>,
}

/// What the Station did with a control, as reported to Universal Exports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ControlOutcome {
    /// `applied` or `rejected`.
    pub(crate) outcome: &'static str,
    /// The Mission's state afterwards, when the Station has the Mission.
    pub(crate) state: Option<&'static str>,
    /// Why a control was rejected; AI6's own wording, never provider or Agent text.
    pub(crate) reason: Option<String>,
}

impl ControlOutcome {
    pub(crate) fn applied(state: &'static str) -> Self {
        Self {
            outcome: "applied",
            state: Some(state),
            reason: None,
        }
    }

    pub(crate) fn rejected(state: Option<&'static str>, reason: impl Into<String>) -> Self {
        Self {
            outcome: "rejected",
            state,
            reason: Some(reason.into()),
        }
    }
}

struct InputTargetRegistration {
    operation_id: String,
    mission_id: String,
    target: Weak<dyn TerminalInputTarget>,
}

impl Publisher {
    pub(crate) fn start(config: Config) -> std::io::Result<Arc<Self>> {
        let origin = config.origin.clone();
        let token = config.token.clone();
        let input_origin = config.origin.clone();
        let input_token = config.token.clone();
        let limits = config.limits.clone();
        let tls = config.tls.clone();
        let agent = gateway_agent(&tls, limits.request_timeout.0);
        let outbox = Arc::new(Outbox::new(limits.clone()));
        let running = Arc::new(AtomicBool::new(true));
        let healthy = Arc::new(AtomicBool::new(true));
        let error = Arc::new(Mutex::new(None));
        let input_targets = Arc::new(Mutex::new(BTreeMap::new()));
        let mut publishing = Worker {
            config,
            agent,
            outbox: outbox.clone(),
            running: running.clone(),
            healthy: healthy.clone(),
            error: error.clone(),
            sessions: BTreeMap::new(),
            pause: limits.publish_retry_delay.0,
            away: false,
            next_refresh: Instant::now() + limits.refresh_every.0,
            sent: 0,
        };
        let worker = thread::Builder::new()
            .name("ai6-publisher".to_owned())
            .spawn(move || publishing.run())?;
        let input_agent = gateway_agent(&tls, limits.request_timeout.0 + limits.input_wait.0);
        let input_running = running.clone();
        let input_targets_for_worker = input_targets.clone();
        let input_limits = limits.clone();
        let input_worker = match thread::Builder::new()
            .name("ai6-publisher-input".to_owned())
            .spawn(move || {
                receive_inputs(
                    input_origin,
                    input_token,
                    input_agent,
                    &input_limits,
                    input_running,
                    input_targets_for_worker,
                )
            }) {
            Ok(worker) => worker,
            Err(error) => {
                running.store(false, Ordering::Release);
                outbox.ready.notify_all();
                let _ = worker.join();
                return Err(error);
            }
        };
        Ok(Arc::new(Self {
            origin,
            token,
            limits,
            tls,
            outbox,
            running,
            healthy,
            error,
            worker: Mutex::new(Some(worker)),
            input_worker: Mutex::new(Some(input_worker)),
            control_worker: Mutex::new(None),
            input_targets,
        }))
    }

    /// Starts taking the controls Universal Exports queued for `station_id`.
    ///
    /// Controls are handed over one at a time: the next is taken only after the previous
    /// outcome is reported. Dropping the receiver reports any taken control as rejected.
    pub(crate) fn receive_controls(
        &self,
        station_id: &str,
    ) -> std::io::Result<tokio::sync::mpsc::Receiver<ControlDelivery>> {
        let mut worker = self
            .control_worker
            .lock()
            .map_err(|_| std::io::Error::other("control worker lock was poisoned"))?;
        if worker.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "this Station already takes its controls",
            ));
        }
        let (deliveries, receiver) = tokio::sync::mpsc::channel(self.limits.control_queue);
        let agent = gateway_agent(
            &self.tls,
            self.limits.request_timeout.0 + self.limits.control_wait.0,
        );
        let link = ControlLink {
            origin: self.origin.clone(),
            token: self.token.clone(),
            station: station_id.to_owned(),
            agent,
            limits: self.limits.clone(),
        };
        let running = self.running.clone();
        *worker = Some(
            thread::Builder::new()
                .name("ai6-publisher-controls".to_owned())
                .spawn(move || take_controls(&link, &running, &deliveries))?,
        );
        Ok(receiver)
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

    fn send(&self, command: Command) {
        if !self.outbox.push(command) {
            self.fail("publication queue lock failed");
        }
    }

    /// Reports a fault until the next event gets through.
    fn fail(&self, reason: &'static str) {
        self.healthy.store(false, Ordering::Release);
        if let Ok(mut error) = self.error.lock() {
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

    /// The origin, Station credential and trust of Universal Exports, for another worker of
    /// this Station that talks to it on a link of its own (the assistant desk).
    pub(crate) fn link(&self) -> (Url, String, TlsConfig) {
        (self.origin.clone(), self.token.clone(), self.tls.clone())
    }

    pub(crate) fn health(&self) -> Health {
        Health {
            enabled: true,
            healthy: self.healthy.load(Ordering::Acquire),
            error: self.error.lock().ok().and_then(|error| *error),
            lost_terminal_bytes: self.outbox.lost_bytes.load(Ordering::Relaxed),
        }
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.outbox.ready.notify_all();
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
        if let Ok(worker) = self.control_worker.get_mut()
            && let Some(worker) = worker.take()
        {
            let _ = worker.join();
        }
    }
}

/// Where and as whom the control worker talks to Universal Exports.
struct ControlLink {
    origin: Url,
    token: String,
    station: String,
    agent: ureq::Agent,
    limits: PublisherLimits,
}

fn take_controls(
    link: &ControlLink,
    running: &AtomicBool,
    deliveries: &tokio::sync::mpsc::Sender<ControlDelivery>,
) {
    let Ok(mut endpoint) = link.origin.join("v1/station/controls/next") else {
        return;
    };
    endpoint
        .query_pairs_mut()
        .append_pair("station", &link.station)
        .append_pair(
            "wait_ms",
            &link.limits.control_wait.0.as_millis().to_string(),
        );
    let Some(schema) = control_schema() else {
        return;
    };
    let mut retry = Retry::new(link.limits.input_retry_delay.0, &link.limits);
    while running.load(Ordering::Acquire) {
        let response = link
            .agent
            .get(endpoint.as_str())
            .header("Authorization", &format!("Bearer {}", link.token))
            .call();
        let Ok(mut response) = response else {
            retry.pause(running);
            continue;
        };
        match response.status().as_u16() {
            200 => retry.reset(),
            204 => {
                retry.reset();
                continue;
            }
            _ => {
                retry.pause(running);
                continue;
            }
        }
        let Ok(frame) = response
            .body_mut()
            .with_config()
            .limit(MAX_CONTROL_MESSAGE_BYTES as u64 + FRAME_MARGIN)
            .read_to_vec()
        else {
            continue;
        };
        let Some(control) = decode_control_frame(&schema, &frame) else {
            continue;
        };
        let control_id = control.control_id().clone();
        let (reply, outcome) = tokio::sync::oneshot::channel();
        if deliveries
            .blocking_send(ControlDelivery {
                control,
                outcome: reply,
            })
            .is_err()
        {
            report_control(
                link,
                &control_id,
                &ControlOutcome::rejected(None, "the Station is stopping"),
            );
            return;
        }
        let outcome = outcome
            .blocking_recv()
            .unwrap_or_else(|_| ControlOutcome::rejected(None, "the Station is stopping"));
        report_control(link, &control_id, &outcome);
    }
}

/// The control channel's schema; it does not depend on the control's values.
fn control_schema() -> Option<Vec<u8>> {
    let template = MissionControl::new(
        ControlId::new("schema").ok()?,
        OperationId::new("schema").ok()?,
        GatewayMissionId::new("schema").ok()?,
        ControlAction::Cancel,
    );
    Some(control_frames(&template).ok()?.0)
}

fn decode_control_frame(schema: &[u8], frame: &[u8]) -> Option<MissionControl> {
    let mut receiver = FrameReceiver::new();
    receiver.accept(schema).ok()?;
    match receiver.accept(frame).ok()?? {
        IncomingFrame::Control(control) => Some(control),
        _ => None,
    }
}

#[derive(Serialize)]
struct ControlReport<'a> {
    station: &'a str,
    #[serde(flatten)]
    outcome: &'a ControlOutcome,
}

fn report_control(link: &ControlLink, control_id: &ControlId, outcome: &ControlOutcome) {
    let Ok(mut url) = link.origin.join("v1/station/controls/") else {
        return;
    };
    if let Ok(mut segments) = url.path_segments_mut() {
        segments
            .pop_if_empty()
            .push(control_id.as_str())
            .push("result");
    }
    for _ in 0..link.limits.control_report_attempts {
        let sent = link
            .agent
            .post(url.as_str())
            .header("Authorization", &format!("Bearer {}", link.token))
            .send_json(ControlReport {
                station: &link.station,
                outcome,
            });
        // Universal Exports refuses a report for a control it finished or forgot; retrying
        // cannot help.
        match sent {
            Ok(_) | Err(ureq::Error::StatusCode(_)) => return,
            Err(_) => thread::sleep(link.limits.input_retry_delay.0),
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
    /// Bytes dropped from `pending_terminal`.
    pending_terminal_lost: usize,
    mission: Option<MissionPublication>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct MissionPublication {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provenance: Option<crate::directorates::registry::bona_fides::Provenance>,
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
        self.publisher.send(Command::Register {
            operation_id: self.operation_id.clone(),
            mission_id: self.mission_id.to_string(),
            session_id: session_id.clone(),
            mission: state.mission.clone().map(Box::new),
        });
        state.session_id = Some(session_id.clone());
        if let Some(target) = &state.terminal_target {
            self.publisher.register_input_target(
                session_id.clone(),
                &self.operation_id,
                self.mission_id.as_str(),
                target,
            );
        }
        if state.pending_terminal_lost > 0 {
            self.publisher
                .outbox
                .lost(session_id.clone(), state.pending_terminal_lost);
            state.pending_terminal_lost = 0;
        }
        state.pending_terminal_bytes = 0;
        while let Some(bytes) = state.pending_terminal.pop_front() {
            self.publisher.send(Command::Terminal {
                session_id: session_id.clone(),
                bytes,
            });
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
        self.publish_lifecycle_for(state, None);
    }

    /// Publishes a state with the reason AI6 holds the Session in it, such as `green-ink`.
    pub(crate) fn publish_lifecycle_for(&self, state: &'static str, reason: Option<&'static str>) {
        let Ok(telemetry) = self.state.lock() else {
            self.publisher.fail("Session telemetry lock failed");
            return;
        };
        if let Some(session_id) = &telemetry.session_id {
            self.publisher.send(Command::Lifecycle {
                session_id: session_id.clone(),
                state,
                reason,
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
        let limits = &self.publisher.limits;
        for chunk in bytes.chunks(limits.terminal_bytes.or_max()) {
            if let Some(session_id) = &state.session_id {
                self.publisher.send(Command::Terminal {
                    session_id: session_id.clone(),
                    bytes: chunk.to_vec(),
                });
                continue;
            }
            // Before the Session has an ID, what it wrote last is kept.
            state.pending_terminal.push_back(chunk.to_vec());
            state.pending_terminal_bytes += chunk.len();
            while !limits
                .pre_bind_terminal_bytes
                .allows(state.pending_terminal_bytes)
            {
                let Some(oldest) = state.pending_terminal.pop_front() else {
                    break;
                };
                state.pending_terminal_bytes -= oldest.len();
                state.pending_terminal_lost += oldest.len();
                self.publisher
                    .outbox
                    .lost_bytes
                    .fetch_add(oldest.len() as u64, Ordering::Relaxed);
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
        if let Some(session_id) = state.session_id.take() {
            self.publisher.send(Command::Forget { session_id });
        }
    }
}

/// Pauses between tries that double up to `retry_max_delay`.
struct Retry {
    first: Duration,
    pause: Duration,
    longest: Duration,
}

impl Retry {
    fn new(first: Duration, limits: &PublisherLimits) -> Self {
        Self {
            first,
            pause: first,
            longest: limits.retry_max_delay.0,
        }
    }

    fn reset(&mut self) {
        self.pause = self.first;
    }

    /// Waits the current pause, or until the Station stops, and doubles it.
    fn pause(&mut self, running: &AtomicBool) {
        sleep_while_running(self.pause, running);
        self.pause = self.pause.saturating_mul(2).min(self.longest);
    }
}

/// Sleeps for `pause`, ending early when `running` goes false: a stopping Station waits for this.
fn sleep_while_running(pause: Duration, running: &AtomicBool) {
    let until = Instant::now() + pause;
    while running.load(Ordering::Acquire) {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        thread::sleep(left.min(Duration::from_millis(50)));
    }
}

fn receive_inputs(
    origin: Url,
    token: String,
    agent: ureq::Agent,
    limits: &PublisherLimits,
    running: Arc<AtomicBool>,
    targets: Arc<Mutex<BTreeMap<SessionId, InputTargetRegistration>>>,
) {
    let Ok(mut endpoint) = origin.join("v1/station/inputs/next") else {
        return;
    };
    endpoint
        .query_pairs_mut()
        .append_pair("wait_ms", &limits.input_wait.0.as_millis().to_string());
    let mut retry = Retry::new(limits.input_retry_delay.0, limits);
    while running.load(Ordering::Acquire) {
        let response = agent
            .get(endpoint.as_str())
            .header("Authorization", &format!("Bearer {token}"))
            .call();
        let Ok(mut response) = response else {
            retry.pause(&running);
            continue;
        };
        // The gateway held the poll for its wait and nothing came.
        if response.status() == 204 {
            retry.reset();
            continue;
        }
        if response.status() != 200 {
            retry.pause(&running);
            continue;
        }
        retry.reset();
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
            .limit(
                limits
                    .terminal_bytes
                    .get()
                    .map_or(u64::MAX, |bytes| bytes.saturating_add(FRAME_MARGIN)),
            )
            .read_to_vec();
        let (Some(operation_id), Some(mission_id), Ok(frame)) = (operation_id, mission_id, frame)
        else {
            retry.pause(&running);
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
        | IncomingFrame::Control(_)
        | IncomingFrame::Gap(_)
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

/// Why an event did not go out.
#[derive(Debug)]
enum Failure {
    /// Trying again may work: the gateway did not answer, or is not taking this for now.
    Away(&'static str),
    /// The gateway will not take this event; the next is worth trying.
    Refused(&'static str),
}

/// What the worker knows of one Session.
struct Published {
    operation_id: String,
    mission_id: String,
    mission: Option<Box<MissionPublication>>,
    /// The sequence of the next event, 0 until a gateway has named one.
    next_sequence: u64,
    /// Whether the gateway is believed to hold the Session.
    registered: bool,
    /// The latest state and whether the gateway is yet to hear it.
    lifecycle: Option<(&'static str, Option<&'static str>)>,
    lifecycle_due: bool,
    /// Output was dropped: the next event's sequence skips a number.
    hole: bool,
    /// How many telemetries hold it; a resume can take a Session over before the old one lets go.
    holders: usize,
    /// The most output one event may carry, as the gateway last said or showed.
    terminal_bytes: Option<usize>,
}

/// Publishes one command at a time, holding it until the gateway has it or refuses it.
struct Worker {
    config: Config,
    agent: ureq::Agent,
    outbox: Arc<Outbox>,
    running: Arc<AtomicBool>,
    healthy: Arc<AtomicBool>,
    error: Arc<Mutex<Option<&'static str>>>,
    sessions: BTreeMap<SessionId, Published>,
    /// The pause before the next try.
    pause: Duration,
    /// Whether the last try found the gateway away.
    away: bool,
    next_refresh: Instant,
    /// How much of the command in hand has gone out, across its tries: a burst goes as several
    /// events, each with its own sequence, and those already out are not sent again.
    sent: usize,
}

impl Worker {
    fn run(&mut self) {
        while self.running.load(Ordering::Acquire) {
            if let Some((command, lost)) = self.outbox.next(Duration::from_millis(100)) {
                self.take_in(&command);
                self.sent = 0;
                if lost && let Some(session) = self.sessions.get_mut(command.session_id()) {
                    session.hole = true;
                }
                self.publish(&command);
            }
            if Instant::now() >= self.next_refresh {
                self.refresh();
            }
        }
    }

    /// What `command` changes in the worker's own record, once however many tries it takes.
    fn take_in(&mut self, command: &Command) {
        match command {
            Command::Register {
                operation_id,
                mission_id,
                session_id,
                mission,
            } => {
                let session =
                    self.sessions
                        .entry(session_id.clone())
                        .or_insert_with(|| Published {
                            operation_id: operation_id.clone(),
                            mission_id: mission_id.clone(),
                            mission: None,
                            next_sequence: 0,
                            registered: false,
                            lifecycle: None,
                            lifecycle_due: false,
                            hole: false,
                            holders: 0,
                            terminal_bytes: None,
                        });
                session.holders += 1;
                session.mission.clone_from(mission);
                session.registered = false;
            }
            Command::Lifecycle {
                session_id,
                state,
                reason,
            } => {
                if let Some(session) = self.sessions.get_mut(session_id) {
                    session.lifecycle = Some((*state, *reason));
                    session.lifecycle_due = true;
                }
            }
            Command::Terminal { .. } => {}
            Command::Forget { session_id } => {
                if let Some(session) = self.sessions.get_mut(session_id) {
                    session.holders = session.holders.saturating_sub(1);
                    if session.holders == 0 {
                        self.sessions.remove(session_id);
                    }
                }
            }
        }
    }

    /// Tries `command` until the gateway has it or refuses it, or the Station stops.
    fn publish(&mut self, command: &Command) {
        while self.running.load(Ordering::Acquire) {
            match self.attempt(command) {
                Ok(()) => {
                    self.answered();
                    return;
                }
                Err(Failure::Refused(reason)) => {
                    self.answered();
                    self.report(reason);
                    if let Command::Terminal { session_id, bytes } = command {
                        self.outbox
                            .lost(session_id.clone(), bytes.len().saturating_sub(self.sent));
                    }
                    return;
                }
                Err(Failure::Away(reason)) => {
                    self.went_away(reason);
                    sleep_while_running(self.pause, &self.running);
                    self.lengthen_pause();
                }
            }
        }
    }

    fn attempt(&mut self, command: &Command) -> Result<(), Failure> {
        match command {
            Command::Register { session_id, .. } | Command::Lifecycle { session_id, .. } => {
                self.bring_up_to_date(session_id)
            }
            Command::Terminal { session_id, bytes } => {
                self.bring_up_to_date(session_id)?;
                self.post_output(session_id, bytes)
            }
            Command::Forget { .. } => Ok(()),
        }
    }

    /// Registers the Session if the gateway may not hold it, and sends the state it has not heard.
    fn bring_up_to_date(&mut self, session_id: &SessionId) -> Result<(), Failure> {
        let Some(session) = self.sessions.get(session_id) else {
            return Err(Failure::Refused("Session was not registered"));
        };
        if !session.registered {
            self.register(session_id, true)?;
        }
        let Some(session) = self.sessions.get(session_id) else {
            return Err(Failure::Refused("Session was not registered"));
        };
        if let (true, Some((state, reason))) = (session.lifecycle_due, session.lifecycle) {
            self.post_lifecycle(session_id, state, reason)?;
            if let Some(session) = self.sessions.get_mut(session_id) {
                session.lifecycle_due = false;
            }
        }
        Ok(())
    }

    /// Registers the Session, with its Mission's brief when `brief`. A gateway naming an earlier
    /// sequence than the worker's own has forgotten the Session: the worker's sequence goes on, so
    /// a viewer's cursor stays good, and the gateway is told its state and brief again.
    fn register(&mut self, session_id: &SessionId, brief: bool) -> Result<(), Failure> {
        let session = self
            .sessions
            .get(session_id)
            .ok_or(Failure::Refused("Session was not registered"))?;
        let (next, stated) = register(
            &self.agent,
            &self.config,
            &session.operation_id,
            &session.mission_id,
            session_id,
            brief.then_some(session.mission.as_deref()).flatten(),
        )?;
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(Failure::Refused("Session was not registered"))?;
        let forgotten = next < session.next_sequence;
        session.terminal_bytes = stated;
        session.next_sequence = session.next_sequence.max(next);
        session.registered = true;
        if forgotten {
            session.lifecycle_due = session.lifecycle.is_some();
            if !brief {
                self.register(session_id, true)?;
            }
        }
        Ok(())
    }

    /// The sequence of the Session's next event, past any hole.
    fn sequence(&mut self, session_id: &SessionId) -> Result<u64, Failure> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(Failure::Refused("Session was not registered"))?;
        if session.hole {
            session.next_sequence = session
                .next_sequence
                .checked_add(1)
                .ok_or(Failure::Refused("Session event sequence is exhausted"))?;
            session.hole = false;
        }
        Ok(session.next_sequence)
    }

    /// Moves past `sequence` once the gateway holds it.
    fn advance(&mut self, session_id: &SessionId, sequence: u64) -> Result<(), Failure> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(Failure::Refused("Session was not registered"))?;
        session.next_sequence = sequence
            .checked_add(1)
            .ok_or(Failure::Refused("Session event sequence is exhausted"))?;
        Ok(())
    }

    fn post_lifecycle(
        &mut self,
        session_id: &SessionId,
        state: &'static str,
        reason: Option<&'static str>,
    ) -> Result<(), Failure> {
        let sequence = self.sequence(session_id)?;
        let (operation_id, mission_id) = self
            .sessions
            .get(session_id)
            .map(|session| (session.operation_id.clone(), session.mission_id.clone()))
            .ok_or(Failure::Refused("Session was not registered"))?;
        let sent = publish_lifecycle(
            &self.agent,
            &self.config,
            session_id,
            sequence,
            Lifecycle {
                operation_id: &operation_id,
                mission_id: &mission_id,
                state,
                reason,
            },
        );
        self.settle(session_id, sequence, sent)
    }

    /// Sends what is left of `bytes`, within what this Station and the gateway allow in one event.
    fn post_output(&mut self, session_id: &SessionId, bytes: &[u8]) -> Result<(), Failure> {
        while self.sent < bytes.len() {
            let stated = self
                .sessions
                .get(session_id)
                .and_then(|session| session.terminal_bytes)
                .unwrap_or(usize::MAX);
            let most = stated.min(self.config.limits.terminal_bytes.or_max());
            let left = &bytes[self.sent..];
            let chunk = &left[..left.len().min(most.max(1))];
            let sequence = self.sequence(session_id)?;
            let sent = publish_terminal(&self.agent, &self.config, session_id, sequence, chunk);
            if matches!(sent, Err(Publish::TooLarge)) {
                // Its limit came down since it last said, or it says one it does not keep.
                if chunk.len() == 1 {
                    return Err(Failure::Refused(
                        "gateway refused terminal output as too large",
                    ));
                }
                if let Some(session) = self.sessions.get_mut(session_id) {
                    session.terminal_bytes = Some(chunk.len() / 2);
                }
                continue;
            }
            self.settle(session_id, sequence, sent)?;
            self.sent += chunk.len();
        }
        Ok(())
    }

    /// What the gateway's answer to `sequence` means: a Session it does not hold is registered
    /// again, and one past `sequence` already has this event, from a try whose answer was lost.
    fn settle(
        &mut self,
        session_id: &SessionId,
        sequence: u64,
        sent: Result<(), Publish>,
    ) -> Result<(), Failure> {
        match sent {
            Ok(()) => self.advance(session_id, sequence),
            Err(Publish::Failed(failure)) => Err(failure),
            Err(Publish::TooLarge) => {
                Err(Failure::Refused("gateway refused an event as too large"))
            }
            Err(Publish::Unknown) => {
                for session in self.sessions.values_mut() {
                    session.registered = false;
                }
                Err(Failure::Away("gateway no longer holds the Session"))
            }
            Err(Publish::Conflict) => {
                self.register(session_id, false)?;
                let next = self.sessions.get(session_id).map(|held| held.next_sequence);
                if next.is_some_and(|next| next > sequence) {
                    Ok(())
                } else {
                    Err(Failure::Refused(
                        "gateway refused a Session event's sequence",
                    ))
                }
            }
        }
    }

    /// Registers every Session again, which restores what a gateway restarted since the last
    /// event forgot; nothing else would tell an idle Session's Station it had restarted.
    fn refresh(&mut self) {
        self.next_refresh = Instant::now() + self.config.limits.refresh_every.0;
        let sessions: Vec<SessionId> = self.sessions.keys().cloned().collect();
        for session_id in sessions {
            let refreshed = match self.sessions.get(&session_id) {
                Some(session) if session.registered => self.register(&session_id, false),
                Some(_) => self.register(&session_id, true),
                None => Ok(()),
            }
            .and_then(|()| self.bring_up_to_date(&session_id));
            match refreshed {
                Ok(()) => self.answered(),
                Err(Failure::Refused(reason)) => self.report(reason),
                Err(Failure::Away(reason)) => {
                    self.went_away(reason);
                    self.next_refresh = Instant::now() + self.pause;
                    self.lengthen_pause();
                    return;
                }
            }
        }
    }

    fn went_away(&mut self, reason: &'static str) {
        self.away = true;
        self.report(reason);
    }

    /// The gateway answered: anything held for it can go, and if it was away it may have restarted.
    fn answered(&mut self) {
        if self.away {
            self.away = false;
            for session in self.sessions.values_mut() {
                session.registered = false;
            }
            self.next_refresh = Instant::now();
        }
        self.pause = self.config.limits.publish_retry_delay.0;
        self.healthy.store(true, Ordering::Release);
        if let Ok(mut error) = self.error.lock() {
            *error = None;
        }
    }

    fn lengthen_pause(&mut self) {
        self.pause = self
            .pause
            .saturating_mul(2)
            .min(self.config.limits.retry_max_delay.0);
    }

    fn report(&self, reason: &'static str) {
        self.healthy.store(false, Ordering::Release);
        if let Ok(mut error) = self.error.lock() {
            *error = Some(reason);
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
    /// The most terminal output the gateway takes in one event of this Session.
    terminal_bytes: Option<u64>,
}

#[derive(Serialize)]
struct Lifecycle<'a> {
    operation_id: &'a str,
    mission_id: &'a str,
    state: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

/// What a status the gateway answers means for a registration or an event: its limits and
/// credentials can change, and a server's error passes, so only a malformed or unprocessable
/// request is taken as final.
fn failure_of(error: &ureq::Error, away: &'static str, refused: &'static str) -> Failure {
    match error {
        ureq::Error::StatusCode(400 | 405 | 409 | 413 | 415 | 422) => Failure::Refused(refused),
        _ => Failure::Away(away),
    }
}

fn register(
    agent: &ureq::Agent,
    config: &Config,
    operation_id: &str,
    mission_id: &str,
    session_id: &SessionId,
    mission: Option<&MissionPublication>,
) -> Result<(u64, Option<usize>), Failure> {
    let url = config
        .origin
        .join("v1/station/sessions")
        .map_err(|_| Failure::Refused("invalid registration endpoint"))?;
    let mut response = agent
        .post(url.as_str())
        .header("Authorization", &format!("Bearer {}", config.token))
        .send_json(Registration {
            operation_id,
            mission_id,
            session_id: session_id.as_str(),
            mission,
        })
        .map_err(|error| {
            failure_of(
                &error,
                "registration request failed",
                "gateway rejected Session registration",
            )
        })?;
    if response.status() != 201 {
        return Err(Failure::Away("gateway rejected Session registration"));
    }
    let registration: RegistrationResponse = response
        .body_mut()
        .read_json()
        .map_err(|_| Failure::Away("gateway returned an invalid registration response"))?;
    if registration.next_sequence == 0 {
        return Err(Failure::Away("gateway returned an invalid event sequence"));
    }
    Ok((
        registration.next_sequence,
        registration
            .terminal_bytes
            .and_then(|stated| usize::try_from(stated).ok()),
    ))
}

/// Why one event's request did not land.
#[derive(Debug)]
enum Publish {
    /// The gateway does not hold the Session.
    Unknown,
    /// The gateway holds a sequence at or past this one.
    Conflict,
    /// The event is longer than the gateway takes.
    TooLarge,
    Failed(Failure),
}

/// The outcome of one event's request.
fn published(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    away: &'static str,
    refused: &'static str,
) -> Result<(), Publish> {
    match response {
        Ok(mut response) if response.status() == 202 => {
            // The event is in; a body cut short changes nothing.
            let _ = response.body_mut().read_to_vec();
            Ok(())
        }
        Ok(_) => Err(Publish::Failed(Failure::Away(refused))),
        Err(ureq::Error::StatusCode(404)) => Err(Publish::Unknown),
        Err(ureq::Error::StatusCode(409)) => Err(Publish::Conflict),
        Err(ureq::Error::StatusCode(413)) => Err(Publish::TooLarge),
        Err(error) => Err(Publish::Failed(failure_of(&error, away, refused))),
    }
}

fn publish_lifecycle(
    agent: &ureq::Agent,
    config: &Config,
    session_id: &SessionId,
    sequence: u64,
    lifecycle: Lifecycle<'_>,
) -> Result<(), Publish> {
    let url = session_endpoint(&config.origin, session_id, "lifecycle", sequence)?;
    published(
        agent
            .post(url.as_str())
            .header("Authorization", &format!("Bearer {}", config.token))
            .send_json(lifecycle),
        "lifecycle request failed",
        "gateway rejected a lifecycle event",
    )
}

fn publish_terminal(
    agent: &ureq::Agent,
    config: &Config,
    session_id: &SessionId,
    sequence: u64,
    bytes: &[u8],
) -> Result<(), Publish> {
    let url = session_endpoint(&config.origin, session_id, "terminal", sequence)?;
    published(
        agent
            .post(url.as_str())
            .header("Authorization", &format!("Bearer {}", config.token))
            .content_type("application/octet-stream")
            .send(bytes),
        "terminal output request failed",
        "gateway rejected terminal output",
    )
}

fn session_endpoint(
    origin: &Url,
    session_id: &SessionId,
    kind: &str,
    sequence: u64,
) -> Result<Url, Publish> {
    let invalid = || Publish::Failed(Failure::Refused("invalid Session endpoint"));
    let mut url = origin.join("v1/station/sessions/").map_err(|_| invalid())?;
    let mut segments = url.path_segments_mut().map_err(|_| invalid())?;
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
    use std::sync::mpsc::{self, Receiver};

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
                let response = if path.starts_with("/v1/station/inputs/next") {
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

    /// One Session as a gateway holds it.
    #[derive(Default)]
    struct HeldSession {
        last_sequence: u64,
        brief: bool,
        lifecycle: Option<(String, Option<String>)>,
        output: Vec<u8>,
        sequences: Vec<u64>,
    }

    /// Answers a Station as Universal Exports does, on one port: registration is idempotent and
    /// names the next sequence, an unknown Session is 404, a sequence not past the last is 409. It
    /// holds everything in memory, so it comes back with nothing, as a restarted gateway does.
    struct Restartable {
        port: u16,
        sessions: Arc<Mutex<BTreeMap<String, HeldSession>>>,
        /// Refuses a terminal body longer than this with 413 and states it at registration.
        stated: Arc<Mutex<Option<usize>>>,
        serving: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
    }

    /// Long enough for a loaded machine; a passing test never waits this long.
    const PATIENCE: Duration = Duration::from_secs(120);

    fn until(what: &str, done: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + PATIENCE;
        while !done() {
            assert!(std::time::Instant::now() < deadline, "never: {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    impl Restartable {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let mut gateway = Self {
                port,
                sessions: Arc::default(),
                stated: Arc::default(),
                serving: None,
            };
            gateway.serve(listener);
            gateway
        }

        fn origin(&self) -> String {
            format!("http://127.0.0.1:{}", self.port)
        }

        fn serve(&mut self, listener: TcpListener) {
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let (stopping, sessions, stated) =
                (stop.clone(), self.sessions.clone(), self.stated.clone());
            let serving = thread::spawn(move || {
                while !stopping.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (sessions, stated) = (sessions.clone(), stated.clone());
                            thread::spawn(move || Self::answer(stream, &sessions, &stated));
                        }
                        Err(_) => thread::sleep(Duration::from_millis(5)),
                    }
                }
            });
            self.serving = Some((stop, serving));
        }

        fn answer(
            stream: std::net::TcpStream,
            sessions: &Mutex<BTreeMap<String, HeldSession>>,
            stated: &Mutex<Option<usize>>,
        ) {
            stream.set_nonblocking(false).unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let mut writer = stream.try_clone().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            if reader.read_line(&mut request).is_err() {
                return;
            }
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let mut body = vec![0; length];
            if reader.read_exact(&mut body).is_err() {
                return;
            }
            let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
            let parts: Vec<&str> = path.split('/').collect();
            let (status, answer) = if path == "/v1/station/sessions" {
                let registration: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let id = registration["session_id"].as_str().unwrap().to_owned();
                let mut sessions = sessions.lock().unwrap();
                let held = sessions.entry(id).or_default();
                held.brief |= registration.get("mission").is_some();
                let mut answer = serde_json::json!({"next_sequence": held.last_sequence + 1});
                if let Some(stated) = *stated.lock().unwrap() {
                    answer["terminal_bytes"] = stated.into();
                }
                ("201 Created", answer.to_string())
            } else if path.starts_with("/v1/station/sessions/") && parts.len() == 7 {
                let sequence: u64 = parts[6].parse().unwrap();
                let mut sessions = sessions.lock().unwrap();
                match sessions.get_mut(parts[4]) {
                    None => ("404 Not Found", String::new()),
                    Some(held) if sequence <= held.last_sequence => ("409 Conflict", String::new()),
                    Some(_)
                        if parts[5] == "terminal"
                            && stated.lock().unwrap().is_some_and(|most| body.len() > most) =>
                    {
                        ("413 Payload Too Large", String::new())
                    }
                    Some(held) => {
                        held.last_sequence = sequence;
                        held.sequences.push(sequence);
                        if parts[5] == "terminal" {
                            held.output.extend_from_slice(&body);
                        } else {
                            let event: serde_json::Value = serde_json::from_slice(&body).unwrap();
                            held.lifecycle = Some((
                                event["state"].as_str().unwrap().to_owned(),
                                event["reason"].as_str().map(str::to_owned),
                            ));
                        }
                        ("202 Accepted", String::new())
                    }
                }
            } else {
                // An input or control poll, with nothing waiting.
                thread::sleep(Duration::from_millis(20));
                ("204 No Content", String::new())
            };
            let _ = write!(
                writer,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{answer}",
                answer.len()
            );
        }

        /// Stops listening and forgets everything.
        fn go_away(&mut self) {
            let (stop, serving) = self.serving.take().unwrap();
            stop.store(true, Ordering::Release);
            serving.join().unwrap();
            self.sessions.lock().unwrap().clear();
        }

        /// Listens on the same port again, holding nothing.
        fn come_back(&mut self) {
            let port = self.port;
            let deadline = std::time::Instant::now() + PATIENCE;
            let listener = loop {
                match TcpListener::bind(("127.0.0.1", port)) {
                    Ok(listener) => break listener,
                    Err(error) if std::time::Instant::now() >= deadline => {
                        panic!("the port did not come free: {error}")
                    }
                    Err(_) => thread::sleep(Duration::from_millis(50)),
                }
            };
            self.serve(listener);
        }

        fn lifecycle(&self, session: &str) -> Option<(String, Option<String>)> {
            self.sessions
                .lock()
                .unwrap()
                .get(session)?
                .lifecycle
                .clone()
        }

        fn output(&self, session: &str) -> Vec<u8> {
            self.sessions
                .lock()
                .unwrap()
                .get(session)
                .map_or_else(Vec::new, |held| held.output.clone())
        }

        fn sequences(&self, session: &str) -> Vec<u64> {
            self.sessions
                .lock()
                .unwrap()
                .get(session)
                .map_or_else(Vec::new, |held| held.sequences.clone())
        }

        fn has_brief(&self, session: &str) -> bool {
            self.sessions
                .lock()
                .unwrap()
                .get(session)
                .is_some_and(|held| held.brief)
        }
    }

    impl Drop for Restartable {
        fn drop(&mut self) {
            if let Some((stop, serving)) = self.serving.take() {
                stop.store(true, Ordering::Release);
                let _ = serving.join();
            }
        }
    }

    fn brief() -> MissionPublication {
        MissionPublication {
            name: None,
            prompt: "Inspect this change".to_owned(),
            agent_id: Some("analyst".to_owned()),
            role_id: None,
            handler_provider: "claude".to_owned(),
            terminal_provider: "pipe".to_owned(),
            working_directory: "C:/operations/a".to_owned(),
            model: None,
            effort: None,
            environment_keys: Vec::new(),
            station_id: Some("hq".to_owned()),
            theater_id: Some("local".to_owned()),
            mode: crate::directorates::m::placement::ExecutionMode::Field,
            provenance: None,
        }
    }

    fn telemetry_of(publisher: &Arc<Publisher>, mission: &str) -> Arc<SessionTelemetry> {
        let telemetry = publisher.session(
            "operation-a".to_owned(),
            MissionId::try_new(mission).unwrap(),
        );
        telemetry.set_mission_publication(brief());
        telemetry
    }

    #[test]
    fn a_gateway_that_goes_away_and_comes_back_is_told_the_truth() {
        let mut gateway = Restartable::start();
        let publisher = Publisher::start(
            Config::try_new(&gateway.origin(), "t")
                .unwrap()
                .with_limits(quick()),
        )
        .unwrap();
        let telemetry = telemetry_of(&publisher, "mission-a");
        telemetry.publish_terminal(b"before ");
        telemetry.bind(SessionId::try_new("session-a").unwrap());
        telemetry.publish_lifecycle("working");
        until("the first state arrives", || {
            gateway.lifecycle("session-a").is_some()
        });
        let last_seen = *gateway.sequences("session-a").last().unwrap();

        gateway.go_away();
        telemetry.publish_terminal(b"while away");
        telemetry.publish_lifecycle_for("blocked", Some("green-ink"));
        until("the Station reports it cannot publish", || {
            !publisher.health().healthy
        });

        gateway.come_back();
        until("the gateway shows the Session as it is now", || {
            gateway.lifecycle("session-a")
                == Some(("blocked".to_owned(), Some("green-ink".to_owned())))
        });
        until("what was held while it was away arrives", || {
            gateway.output("session-a") == b"while away"
        });
        assert!(
            gateway.has_brief("session-a"),
            "the Mission's brief is sent again"
        );
        let sequences = gateway.sequences("session-a");
        assert!(
            sequences[0] > last_seen,
            "a viewer's cursor stays good across the restart: {sequences:?} after {last_seen}"
        );
        until("the Station reports it publishes again", || {
            publisher.health().healthy
        });
        telemetry.publish_terminal(b" and after");
        until("publishing goes on", || {
            gateway.output("session-a") == b"while away and after"
        });
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
    fn the_public_roots_stay_trusted_beside_a_ca_file() {
        use ureq::tls::RootCerts;

        let config = Config::try_new("https://gateway.example", "secret").unwrap();
        assert!(
            matches!(config.tls.root_certs(), RootCerts::WebPki),
            "unset, the built-in roots"
        );
        let config = config.with_ca_certificates(&[vec![1, 2, 3]]);
        let RootCerts::Specific(roots) = config.tls.root_certs() else {
            panic!("a list of roots");
        };
        let public = webpki_root_certs::TLS_SERVER_ROOT_CERTS;
        assert_eq!(roots.len(), public.len() + 1);
        assert_eq!(roots[0].der(), public[0].as_ref());
        assert_eq!(roots[public.len()].der(), [1, 2, 3]);
    }

    #[test]
    fn an_enterprise_gateway_is_refused_without_its_ca_and_reached_on_every_link_with_it() {
        let gateway = tls_gateway::start("127.0.0.1");
        let refused = Publisher::start(Config::try_new(&gateway.origin, "t").unwrap()).unwrap();
        let _controls = refused.receive_controls("hq").unwrap();
        refused
            .session(
                "operation-a".to_owned(),
                MissionId::try_new("mission-a").unwrap(),
            )
            .bind(SessionId::try_new("session-a").unwrap());
        assert_eq!(
            gateway.paths.recv_timeout(Duration::from_secs(2)).ok(),
            None,
            "no request crosses a handshake the Station does not trust"
        );
        drop(refused);

        let gateway = tls_gateway::start("127.0.0.1");
        let config = Config::try_new(&gateway.origin, "t").unwrap();
        let reached = Publisher::start(config.with_ca_certificates(&gateway.ca)).unwrap();
        let _controls = reached.receive_controls("hq").unwrap();
        reached
            .session(
                "operation-a".to_owned(),
                MissionId::try_new("mission-a").unwrap(),
            )
            .bind(SessionId::try_new("session-a").unwrap());
        let mut links: Vec<&str> = vec![
            "/v1/station/sessions",
            "/v1/station/inputs/next",
            "/v1/station/controls/next",
        ];
        while !links.is_empty() {
            let path = gateway
                .paths
                .recv_timeout(Duration::from_secs(10))
                .unwrap_or_else(|_| panic!("no request on {links:?}"));
            links.retain(|link| !path.starts_with(link));
        }
    }

    #[test]
    fn a_ca_does_not_vouch_for_a_gateway_of_another_name() {
        let gateway = tls_gateway::start("other.example");
        let config = Config::try_new(&gateway.origin, "t").unwrap();
        let _publisher = Publisher::start(config.with_ca_certificates(&gateway.ca)).unwrap();
        assert_eq!(
            gateway.paths.recv_timeout(Duration::from_secs(2)).ok(),
            None
        );
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
            name: None,
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
            provenance: None,
        });
        let terminal = TerminalBridge::agent_terminal_with_watchdog_and_telemetry(
            Box::new(RawOutputTerminal),
            telemetry.clone(),
            crate::directorates::medical::watchdog::Watchdog::default(),
            true,
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
    fn a_private_commands_output_reaches_the_handler_and_no_gateway() {
        let (origin, requests, gateway) = gateway();
        let publisher = Publisher::start(Config::try_new(&origin, "test-token").unwrap()).unwrap();
        let telemetry = publisher.session(
            "operation-a".to_owned(),
            MissionId::try_new("mission-a").unwrap(),
        );
        let terminal = TerminalBridge::agent_terminal_with_watchdog_and_telemetry(
            Box::new(RawOutputTerminal),
            telemetry.clone(),
            crate::directorates::medical::watchdog::Watchdog::default(),
            true,
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
        let mut output = [0; 8];
        let probe = terminal.spawn_private(&spec).unwrap();
        assert_eq!(
            probe.read(&mut output, 100).unwrap(),
            3,
            "the handler reads it"
        );
        // The Session's own output goes out, so the gateway has seen all it will once this comes.
        let session = terminal.spawn(&spec).unwrap();
        assert_eq!(session.read(&mut output, 100).unwrap(), 3);
        telemetry.bind(SessionId::try_new("session-a").unwrap());
        telemetry.publish_lifecycle("working");

        let registration = requests.recv_timeout(Duration::from_secs(60)).unwrap();
        assert_eq!(registration.path, "/v1/station/sessions");
        let terminal_frame = requests.recv_timeout(Duration::from_secs(60)).unwrap();
        assert_eq!(
            terminal_frame.path,
            "/v1/station/sessions/session-a/terminal/41"
        );
        let lifecycle = requests.recv_timeout(Duration::from_secs(60)).unwrap();
        assert_eq!(
            lifecycle.path, "/v1/station/sessions/session-a/lifecycle/42",
            "one read's output, the Session's, and no other frame came before this"
        );
        gateway.join().unwrap();
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
                } else if path == "/v1/station/inputs/next?wait_ms=20000"
                    && gateway_ready.load(Ordering::Acquire)
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
            true,
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
    fn a_queued_control_reaches_the_station_and_its_outcome_is_reported() {
        let control = MissionControl::new(
            ControlId::new("control-1").unwrap(),
            OperationId::new("operation-a").unwrap(),
            GatewayMissionId::new("mission-a").unwrap(),
            ControlAction::Resume,
        )
        .with_message("carry on")
        .unwrap();
        let (_, frame) = control_frames(&control).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, requests) = mpsc::channel();
        let gateway = thread::spawn(move || {
            let mut served = false;
            loop {
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
                if path.starts_with("/v1/station/controls/next?") && !served {
                    served = true;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.ai6.guatiao\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        frame.len()
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                    stream.write_all(&frame).unwrap();
                    sender
                        .send(Request {
                            path,
                            headers,
                            body,
                        })
                        .unwrap();
                } else if path == "/v1/station/controls/control-1/result" {
                    stream
                        .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                        .unwrap();
                    sender
                        .send(Request {
                            path,
                            headers,
                            body,
                        })
                        .unwrap();
                    break;
                } else if path.starts_with("/v1/station/controls/next?") {
                    stream
                        .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                        .unwrap();
                } else {
                    stream
                        .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .unwrap();
                }
            }
        });

        let config = Config::try_new(&format!("http://{address}"), "test-token").unwrap();
        let publisher = Publisher::start(config).unwrap();
        let mut deliveries = publisher.receive_controls("station-7").unwrap();
        assert!(publisher.receive_controls("station-7").is_err());
        let delivery = deliveries.blocking_recv().unwrap();
        assert_eq!(delivery.control, control);
        delivery
            .outcome
            .send(ControlOutcome::applied("working"))
            .unwrap();

        let poll = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(
            poll.path,
            "/v1/station/controls/next?station=station-7&wait_ms=1000"
        );
        assert!(poll.headers.contains("Bearer test-token"));
        let report = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&report.body).unwrap(),
            serde_json::json!({
                "station": "station-7",
                "outcome": "applied",
                "state": "working",
                "reason": null
            })
        );
        gateway.join().unwrap();
    }

    /// Pauses a test can sit through.
    fn quick() -> PublisherLimits {
        PublisherLimits {
            publish_retry_delay: crate::bounds::Seconds(Duration::from_millis(20)),
            retry_max_delay: crate::bounds::Seconds(Duration::from_millis(100)),
            refresh_every: crate::bounds::Seconds(Duration::from_millis(100)),
            ..PublisherLimits::default()
        }
    }

    #[test]
    fn output_past_the_unbound_buffer_costs_the_oldest_and_says_so_when_it_goes_out() {
        let gateway = Restartable::start();
        let limits = PublisherLimits {
            pre_bind_terminal_bytes: crate::bounds::Limit::at(8),
            ..quick()
        };
        let publisher = Publisher::start(
            Config::try_new(&gateway.origin(), "t")
                .unwrap()
                .with_limits(limits),
        )
        .unwrap();
        let telemetry = telemetry_of(&publisher, "mission-a");
        telemetry.publish_terminal(b"12345");
        telemetry.publish_terminal(b"67890");
        assert!(publisher.health().healthy, "nothing fatal in that");
        assert_eq!(publisher.health().lost_terminal_bytes, 5);
        telemetry.bind(SessionId::try_new("session-a").unwrap());
        until("the newest output goes out", || {
            gateway.output("session-a") == b"67890"
        });
        assert_eq!(
            gateway.sequences("session-a"),
            [2],
            "the number the lost output would have had is skipped"
        );
    }

    #[test]
    fn a_full_queue_costs_the_oldest_output_and_never_the_latest_state() {
        let mut gateway = Restartable::start();
        let limits = PublisherLimits {
            queue: 3,
            terminal_bytes: crate::bounds::Limit::at(4),
            ..quick()
        };
        let publisher = Publisher::start(
            Config::try_new(&gateway.origin(), "t")
                .unwrap()
                .with_limits(limits),
        )
        .unwrap();
        let telemetry = telemetry_of(&publisher, "mission-a");
        telemetry.bind(SessionId::try_new("session-a").unwrap());
        telemetry.publish_lifecycle("working");
        until("the first state arrives", || {
            gateway.lifecycle("session-a").is_some()
        });
        gateway.go_away();
        // The worker holds what it took first; what follows passes through a queue of three.
        telemetry.publish_terminal(b"held");
        until("the Station reports it cannot publish", || {
            !publisher.health().healthy
        });
        telemetry.publish_lifecycle("blocked");
        for chunk in [b"aaaa", b"bbbb", b"cccc", b"dddd", b"eeee"] {
            telemetry.publish_terminal(chunk);
        }
        telemetry.publish_lifecycle("stalled");
        telemetry.publish_lifecycle("working");
        assert!(publisher.health().lost_terminal_bytes > 0);

        gateway.come_back();
        until("the latest state and output arrive", || {
            gateway
                .lifecycle("session-a")
                .map(|state| state.0)
                .as_deref()
                == Some("working")
                && gateway.output("session-a").ends_with(b"eeee")
        });
        let sequences = gateway.sequences("session-a");
        assert!(
            sequences.windows(2).any(|pair| pair[1] > pair[0] + 1),
            "the dropped output leaves a hole: {sequences:?}"
        );
        assert!(publisher.health().healthy);
    }

    #[test]
    fn a_gateway_restarted_while_a_session_was_idle_learns_of_it_unasked() {
        let mut gateway = Restartable::start();
        let publisher = Publisher::start(
            Config::try_new(&gateway.origin(), "t")
                .unwrap()
                .with_limits(quick()),
        )
        .unwrap();
        let telemetry = telemetry_of(&publisher, "mission-a");
        telemetry.bind(SessionId::try_new("session-a").unwrap());
        telemetry.publish_lifecycle_for("blocked", Some("green-ink"));
        until("the state arrives", || {
            gateway.lifecycle("session-a").is_some()
        });
        gateway.go_away();
        gateway.come_back();
        until(
            "the gateway shows the Session again, with no new event",
            || {
                gateway.lifecycle("session-a")
                    == Some(("blocked".to_owned(), Some("green-ink".to_owned())))
                    && gateway.has_brief("session-a")
            },
        );
    }

    #[test]
    fn a_session_that_ended_is_not_registered_again() {
        let mut gateway = Restartable::start();
        let publisher = Publisher::start(
            Config::try_new(&gateway.origin(), "t")
                .unwrap()
                .with_limits(quick()),
        )
        .unwrap();
        let telemetry = telemetry_of(&publisher, "mission-a");
        telemetry.bind(SessionId::try_new("session-a").unwrap());
        telemetry.publish_lifecycle("done");
        until("the last state arrives", || {
            gateway.lifecycle("session-a").is_some()
        });
        drop(telemetry);
        let other = telemetry_of(&publisher, "mission-b");
        other.bind(SessionId::try_new("session-b").unwrap());
        other.publish_lifecycle("working");
        until("what came after the end arrives", || {
            gateway.lifecycle("session-b").is_some()
        });
        gateway.go_away();
        gateway.come_back();
        until("the live Session is back", || {
            gateway.lifecycle("session-b").is_some()
        });
        assert_eq!(gateway.lifecycle("session-a"), None);
    }

    #[test]
    fn a_burst_longer_than_the_gateway_takes_arrives_split_within_what_it_states() {
        let gateway = Restartable::start();
        *gateway.stated.lock().unwrap() = Some(1000);
        let publisher = Publisher::start(
            Config::try_new(&gateway.origin(), "t")
                .unwrap()
                .with_limits(quick()),
        )
        .unwrap();
        let telemetry = telemetry_of(&publisher, "mission-a");
        telemetry.bind(SessionId::try_new("session-a").unwrap());
        let burst: Vec<u8> = (0..4500_u32).map(|at| (at % 251) as u8).collect();
        telemetry.publish_terminal(&burst);
        until("the whole burst arrives", || {
            gateway.output("session-a") == burst
        });
        assert_eq!(gateway.sequences("session-a"), [1, 2, 3, 4, 5]);
        let health = publisher.health();
        assert!(health.healthy, "{:?}", health.error);
        assert_eq!(health.lost_terminal_bytes, 0);
    }

    #[test]
    fn a_gateway_that_lowers_its_limit_unannounced_gets_smaller_frames() {
        let gateway = Restartable::start();
        let publisher = Publisher::start(
            Config::try_new(&gateway.origin(), "t")
                .unwrap()
                .with_limits(quick()),
        )
        .unwrap();
        let telemetry = telemetry_of(&publisher, "mission-a");
        telemetry.bind(SessionId::try_new("session-a").unwrap());
        telemetry.publish_lifecycle("working");
        until("the Session is registered", || {
            gateway.lifecycle("session-a").is_some()
        });
        *gateway.stated.lock().unwrap() = Some(100);
        telemetry.publish_terminal(&[7; 950]);
        until("the output arrives", || {
            gateway.output("session-a") == [7; 950]
        });
        assert_eq!(publisher.health().lost_terminal_bytes, 0);
    }

    #[test]
    fn output_the_gateway_refuses_at_any_size_is_dropped_and_counted() {
        let gateway = Restartable::start();
        *gateway.stated.lock().unwrap() = Some(0);
        let publisher = Publisher::start(
            Config::try_new(&gateway.origin(), "t")
                .unwrap()
                .with_limits(quick()),
        )
        .unwrap();
        let telemetry = telemetry_of(&publisher, "mission-a");
        telemetry.bind(SessionId::try_new("session-a").unwrap());
        telemetry.publish_terminal(b"nothing fits");
        until("the output is written off", || {
            publisher.health().lost_terminal_bytes == 12
        });
        telemetry.publish_lifecycle("working");
        until("what follows still goes out", || {
            gateway.lifecycle("session-a").is_some()
        });
    }

    #[test]
    fn the_default_frame_fits_the_gateways_default_frame() {
        let frame = crate::UnexLimits::default().gateway.frame_bytes;
        let publisher = PublisherLimits::default().terminal_bytes;
        assert_eq!(
            publisher.get().unwrap() + FRAME_MARGIN,
            frame.get().unwrap(),
            "the output of a default frame, with room for its header, is the gateway's default"
        );
    }

    #[test]
    fn a_refused_event_does_not_hold_up_the_next() {
        // A gateway answering 400 to one state: only a malformed request is final.
        assert!(matches!(
            failure_of(&ureq::Error::StatusCode(400), "away", "refused"),
            Failure::Refused("refused")
        ));
        for status in [401, 403, 429, 500, 502, 503, 504] {
            assert!(
                matches!(
                    failure_of(&ureq::Error::StatusCode(status), "away", "refused"),
                    Failure::Away("away")
                ),
                "{status} passes"
            );
        }
    }
}

/// A TLS listener standing in for a gateway behind an enterprise certificate authority.
#[cfg(test)]
pub(crate) mod tls_gateway {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::Duration;

    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    use rustls::pki_types::PrivatePkcs8KeyDer;

    pub(crate) struct TlsGateway {
        pub(crate) origin: String,
        /// The throwaway certificate authority, as `read_ca_file` returns it.
        pub(crate) ca: Vec<Vec<u8>>,
        /// The path of each request that got past the handshake.
        pub(crate) paths: Receiver<String>,
    }

    /// Listens on loopback with a certificate for `name`, from a new certificate authority.
    /// Each request is answered 503, which leaves a publisher polling.
    pub(crate) fn start(name: &str) -> TlsGateway {
        let ca_key = KeyPair::generate().unwrap();
        let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_certificate = ca.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca, ca_key);
        let key = KeyPair::generate().unwrap();
        let leaf = CertificateParams::new(vec![name.to_owned()])
            .unwrap()
            .signed_by(&key, &issuer)
            .unwrap();
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![leaf.der().clone()],
                PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            )
            .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("https://{}", listener.local_addr().unwrap());
        let (sender, paths) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let config = config.clone();
                let sender = sender.clone();
                thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let connection = rustls::ServerConnection::new(config).unwrap();
                    let mut reader = BufReader::new(rustls::StreamOwned::new(connection, stream));
                    let mut request = String::new();
                    if reader.read_line(&mut request).is_err() {
                        return;
                    }
                    let mut length = 0;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() {
                            return;
                        }
                        if let Some(value) =
                            line.to_ascii_lowercase().strip_prefix("content-length:")
                        {
                            length = value.trim().parse().unwrap_or(0);
                        }
                        if line == "\r\n" || line.is_empty() {
                            break;
                        }
                    }
                    let mut body = vec![0; length];
                    if reader.read_exact(&mut body).is_err() {
                        return;
                    }
                    let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                    let _ = sender.send(path);
                    let tls = reader.get_mut();
                    let _ = tls.write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\
                          Connection: close\r\n\r\n",
                    );
                    tls.conn.send_close_notify();
                    let _ = tls.flush();
                });
            }
        });
        TlsGateway {
            origin,
            ca: vec![ca_certificate.der().to_vec()],
            paths,
        }
    }
}
