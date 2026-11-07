//! Session records: the shared, fenced record of every Agent Session.
//!
//! A Station's own Mission book dies with the Station; this record lives in the Registry so
//! another Station can see what was running and where. Whoever acts on a Session must hold
//! its lease, and every write presents the fencing token the lease was granted with. A token
//! is taken from the Headquarters election term and always grows past the previous holder's,
//! so a Station that lost its lease while paused cannot write after a new holder took over.

use std::fmt;
use std::fs;
use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::directorates::m::deliverables::Deliverable;
use crate::directorates::m::placement::ExecutionMode;
use crate::directorates::medical::exposure::Exposure;
use crate::directorates::q::safehouse::SafeHouseCheckpoint;

use super::accounts::AccountRef;

use super::limits::SessionLimits;
use super::mission_ids::{new_uid, run_handle};
use super::products::CUT_MARKER;
use super::store::{FileRegistry, invalid_data, read_headquarters, unix_time};
use crate::bounds::Limit;

/// Where a Session stands, as far as the Registry knows.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// The holder is running the provider Session.
    Active,
    /// A provider limit paused the Session until `parked_until`.
    Parked,
    /// The Session finished; it can no longer be taken or written.
    Ended,
    /// Its Station went dark and nothing could re-attach to the harness; a later attempt
    /// replaces it. Closed like `Ended`.
    Lost,
}

impl SessionStatus {
    /// Whether the Session can no longer be taken or written.
    pub fn is_closed(self) -> bool {
        matches!(self, Self::Ended | Self::Lost)
    }
}

/// How a Session's harness was launched: what another Station needs to re-attach to it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLaunch {
    /// The AgentHandler provider.
    pub handler: String,
    /// The terminal provider the harness runs in.
    pub terminal: String,
    /// The working directory, on the Theater.
    pub working_directory: String,
    /// The model, when the Agent names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The effort, when the Agent names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The pool its route names, when it names one; with the handler and model it says which
    /// route ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    /// Whether the harness runs on the Theater or acts on it from the Station.
    #[serde(default)]
    pub mode: ExecutionMode,
    /// The key of the Session on the Theater's runner, when a runner supervises the harness;
    /// only such a Session can be re-attached by another Station.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    /// The clearance the harness was launched at, which fixed its tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance: Option<String>,
    /// The Agent codename the harness ran as. A scorecard groups Sessions by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The overlays it was launched with, each at its digest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<crate::directorates::q::overlays::IssuedDigest>,
}

impl SessionLaunch {
    /// A launch of `handler` in `terminal`, working in `working_directory`.
    pub fn new(
        handler: impl Into<String>,
        terminal: impl Into<String>,
        working_directory: impl Into<String>,
    ) -> Self {
        Self {
            handler: handler.into(),
            terminal: terminal.into(),
            working_directory: working_directory.into(),
            model: None,
            effort: None,
            pool: None,
            mode: ExecutionMode::default(),
            runner: None,
            clearance: None,
            agent: None,
            overlays: Vec::new(),
        }
    }

    /// Records the Agent codename the harness ran as.
    pub fn with_agent(mut self, agent: Option<String>) -> Self {
        self.agent = agent;
        self
    }

    /// Records the model and effort.
    pub fn with_model(mut self, model: Option<String>, effort: Option<String>) -> Self {
        self.model = model;
        self.effort = effort;
        self
    }

    /// Records the pool its route names.
    pub fn with_pool(mut self, pool: Option<String>) -> Self {
        self.pool = pool;
        self
    }

    /// Records the execution mode.
    pub fn with_mode(mut self, mode: ExecutionMode) -> Self {
        self.mode = mode;
        self
    }

    /// Records the overlays the harness was launched with.
    pub fn with_overlays(
        mut self,
        overlays: Vec<crate::directorates::q::overlays::IssuedDigest>,
    ) -> Self {
        self.overlays = overlays;
        self
    }

    /// Records the clearance the harness was launched at.
    pub fn with_clearance(mut self, clearance: Option<String>) -> Self {
        self.clearance = clearance;
        self
    }

    /// Records the runner's key for the Session.
    pub fn with_runner(mut self, key: impl Into<String>) -> Self {
        self.runner = Some(key.into());
        self
    }
}

/// One Agent Session as stored in the shared Registry.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    /// AI6's identifier for this Session, distinct from the provider's: the run's UID.
    pub session_id: String,
    /// The UID of the Mission the Session works for.
    pub mission_id: String,
    /// The run's handle, `<mission handle>.<attempt>`, from the handle the Mission had when the
    /// run opened; a rename of the Mission later leaves it as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    /// The Operation the Mission belongs to, when the Station that opened the Session knew it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// The provider-issued Session ID, once the provider has created it.
    pub provider_session_id: Option<String>,
    /// The Station that holds the lease and supervises the Session.
    pub station: String,
    /// The Theater the harness runs in, when known.
    pub theater: Option<String>,
    /// Which try at the Mission this is, starting at 1.
    pub attempt: u32,
    /// The AI6 ID of the Session this one replaced, when it is a later attempt.
    pub predecessor: Option<String>,
    /// Current status.
    pub status: SessionStatus,
    /// When a parked Session may resume, as Unix seconds.
    pub parked_until: Option<f64>,
    /// When the holder's lease lapses, as Unix seconds.
    pub lease_expires: f64,
    /// The fencing token of the current lease; it only grows.
    pub fence: u64,
    /// The untrusted input the Session has read; it only grows and a later attempt at the
    /// Mission inherits it.
    pub exposure: Vec<Exposure>,
    /// How the harness was launched, which another Station needs to re-attach to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<SessionLaunch>,
    /// What the holder last saw the Session do, oldest first and bounded: the hand-off a
    /// later attempt is briefed with when this one is lost.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<String>,
    /// The last checkpoint of the Session's safe house.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SafeHouseCheckpoint>,
    /// When the last checkpoint was saved, as Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpointed_at: Option<f64>,
    /// Why the Session was lost; set only with [`SessionStatus::Lost`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<String>,
    /// The pool account whose credential the Session runs on, when its Agent draws from a
    /// pool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountRef>,
    /// When the Session was recorded, as Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<f64>,
    /// When it became `Ended` or `Lost`, as Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<f64>,
    /// How its Mission stood when the holder last wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<SessionResult>,
    /// The window (`20261025T0300Z`) a standing Sleeper's run answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// What a person found when they checked how the attempt ended; the latest one stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<MissionVerdict>,
    /// What its Mission was to deliver, when the attempt ended done and went to be checked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliverables: Vec<Deliverable>,
    /// What the Station's probes found of those, oldest first and bounded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
    /// When a person asked for the deliverables to be probed, as Unix seconds, until the Station
    /// has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_requested: Option<f64>,
}

/// What a probe found of one deliverable.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProbeResult {
    /// It is there.
    Found,
    /// The place it should be was read, and it is not there.
    Missing,
    /// This Station has no means to look, as for work on a Theater it cannot reach.
    Unprobed,
    /// The look failed or ran out of time, so nothing is known.
    Failed,
}

impl ProbeResult {
    /// The spelling on the wire and in reports.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Found => "found",
            Self::Missing => "missing",
            Self::Unprobed => "unprobed",
            Self::Failed => "failed",
        }
    }
}

/// What one probe of one deliverable found, kept on the attempt it was run for.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    /// What was probed.
    pub deliverable: Deliverable,
    /// What was found.
    pub result: ProbeResult,
    /// What was read, in a line: the name that is missing, the address that answered. It may quote
    /// what a tracker or a server said, so it is data.
    pub detail: String,
    /// When the probe ran, as Unix seconds.
    pub at: f64,
}

impl Evidence {
    /// `result` for `deliverable`, as found at `at`.
    pub fn new(
        deliverable: Deliverable,
        result: ProbeResult,
        detail: impl Into<String>,
        at: f64,
    ) -> Self {
        Self {
            deliverable,
            result,
            detail: detail.into(),
            at,
        }
    }
}

/// What a person found when they checked a Mission's outcome.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutcomeVerdict {
    /// The work is what the Agent said it was.
    Confirmed,
    /// The person does not accept the Agent's account of it.
    Disputed,
    /// The person checked, and the work does not hold up.
    FailedVerification,
    /// The work did not deliver what it was to: the Mission goes to its next attempt, told why.
    Redispatch,
}

impl OutcomeVerdict {
    /// Every verdict.
    pub const ALL: [Self; 4] = [
        Self::Confirmed,
        Self::Disputed,
        Self::FailedVerification,
        Self::Redispatch,
    ];

    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Disputed => "disputed",
            Self::FailedVerification => "failed-verification",
            Self::Redispatch => "redispatch",
        }
    }

    /// The verdict its wire spelling names.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|verdict| verdict.as_str() == name)
    }

    /// Whether the outcome counts as a failure of the Agent that ran it.
    pub const fn is_fault(self) -> bool {
        matches!(
            self,
            Self::Disputed | Self::FailedVerification | Self::Redispatch
        )
    }
}

/// A person's verdict on how an attempt at a Mission ended.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionVerdict {
    /// What they found.
    pub verdict: OutcomeVerdict,
    /// Why, in their words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// For a redispatch, what the next attempt is told beyond its brief and the reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amendment: Option<String>,
    /// The authenticated person.
    pub by: String,
    /// When, as Unix seconds.
    pub at: f64,
}

impl MissionVerdict {
    /// `by`'s `verdict` at `at`, for the reason in `note`.
    pub fn new(
        verdict: OutcomeVerdict,
        by: impl Into<String>,
        at: f64,
        note: Option<String>,
    ) -> Self {
        Self {
            verdict,
            note,
            amendment: None,
            by: by.into(),
            at,
        }
    }

    /// This verdict with what a redispatch tells the next attempt.
    #[must_use]
    pub fn with_amendment(mut self, amendment: Option<String>) -> Self {
        self.amendment = amendment;
        self
    }
}

/// The last message an Agent ended a turn with on a Session: its own account of the work, as data.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalReport {
    /// AI6's identifier for the Session.
    pub session_id: String,
    /// The UID of the Mission it worked for.
    pub mission_id: String,
    /// The message, within `[registry.limits.sessions] final_report_bytes` and marked if cut.
    pub text: String,
    /// When it was filed, as Unix seconds.
    pub at: f64,
}

impl FinalReport {
    /// Session `session_id`'s report of `text`, cut to `limit` bytes with the cut marked.
    pub fn new(
        session_id: impl Into<String>,
        mission_id: impl Into<String>,
        text: &str,
        at: f64,
        limit: Limit,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            mission_id: mission_id.into(),
            text: cut_report(text, limit),
            at,
        }
    }
}

/// `text` within `limit` bytes, the marker included, cut on a character boundary.
fn cut_report(text: &str, limit: Limit) -> String {
    if limit.allows(text.len()) {
        return text.to_owned();
    }
    let most = limit.or_max();
    let Some(room) = most.checked_sub(CUT_MARKER.len()) else {
        return String::new();
    };
    let mut end = room;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{CUT_MARKER}", &text[..end])
}

/// How a Mission stood at its Session's last write: what a scorecard judges the attempt by.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionResult {
    /// The Mission's state, in its wire spelling (`working`, `blocked`, `stalled`, `done`,
    /// `failed`, `blown`, `abandoned`, `parked`, `quarantined`).
    pub state: String,
    /// Why it stands there when AI6 knows: a blocked Mission's reason (`green-ink`, `budget`,
    /// `interrupted`), or `handed-off` for an attempt another replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// How many tools its harness refused in its last turn.
    #[serde(default)]
    pub denied_tools: u32,
}

impl SessionResult {
    /// The reason of an attempt another replaced after a limit: no outcome of its Agent's.
    pub const HANDED_OFF: &'static str = "handed-off";

    /// A result in `state`.
    pub fn new(state: impl Into<String>) -> Self {
        Self {
            state: state.into(),
            reason: None,
            denied_tools: 0,
        }
    }

    /// Names why the Mission stands there.
    #[must_use]
    pub fn with_reason(mut self, reason: Option<String>) -> Self {
        self.reason = reason;
        self
    }

    /// Counts the tools its harness refused.
    #[must_use]
    pub fn with_denied_tools(mut self, denied: usize) -> Self {
        self.denied_tools = u32::try_from(denied).unwrap_or(u32::MAX);
        self
    }
}

impl SessionRecord {
    /// Whether a person may give a verdict on the attempt: it ended done or held, or is being
    /// verified.
    ///
    /// # Errors
    ///
    /// `Unsettled`, naming how it stands instead.
    pub fn takes_verdict(&self) -> Result<(), SessionError> {
        if self.status == SessionStatus::Lost {
            return Err(SessionError::Unsettled("lost".to_owned()));
        }
        let state = self
            .result
            .as_ref()
            .map_or("unrecorded", |result| result.state.as_str());
        if matches!(state, "done" | "held" | "verifying") {
            Ok(())
        } else {
            Err(SessionError::Unsettled(state.to_owned()))
        }
    }

    /// Whether the lease is still live at `now`, in Unix seconds.
    pub fn leased_at(&self, now: f64) -> bool {
        !self.status.is_closed() && self.lease_expires >= now
    }
}

/// A Session to record, built with defaults for the optional parts.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSession {
    pub(super) session_id: String,
    pub(super) mission_id: String,
    mission_handle: Option<String>,
    operation: Option<String>,
    station: String,
    provider_session_id: Option<String>,
    theater: Option<String>,
    attempt: u32,
    predecessor: Option<String>,
    exposure: Vec<Exposure>,
    launch: Option<SessionLaunch>,
    account: Option<AccountRef>,
    window: Option<String>,
}

impl NewSession {
    /// Starts a first attempt held by `station`.
    pub fn new(
        session_id: impl Into<String>,
        mission_id: impl Into<String>,
        station: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            mission_id: mission_id.into(),
            mission_handle: None,
            operation: None,
            station: station.into(),
            provider_session_id: None,
            theater: None,
            attempt: 1,
            predecessor: None,
            exposure: Vec::new(),
            launch: None,
            account: None,
            window: None,
        }
    }

    /// Records the standing Sleeper's window the run answers.
    pub fn with_window(mut self, window: impl Into<String>) -> Self {
        self.window = Some(window.into());
        self
    }

    /// Names the run by the Mission's handle now: the record's handle is
    /// `<mission_handle>.<attempt>`.
    pub fn with_handle(mut self, mission_handle: impl Into<String>) -> Self {
        self.mission_handle = Some(mission_handle.into());
        self
    }

    /// The attempt this Session would be.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Records the Operation the Mission belongs to.
    pub fn with_operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = Some(operation.into());
        self
    }

    /// Records the pool account the Session runs on.
    pub fn with_account(mut self, account: AccountRef) -> Self {
        self.account = Some(account);
        self
    }

    /// Records how the harness was launched.
    pub fn with_launch(mut self, launch: SessionLaunch) -> Self {
        self.launch = Some(launch);
        self
    }

    /// Records the exposure the Session starts with, inherited from its Mission.
    pub fn with_exposure(mut self, exposure: Vec<Exposure>) -> Self {
        self.exposure = exposure;
        self
    }

    /// Records the provider's Session ID when it is already known.
    pub fn with_provider_session(mut self, id: impl Into<String>) -> Self {
        self.provider_session_id = Some(id.into());
        self
    }

    /// Records the Theater the harness runs in.
    pub fn with_theater(mut self, theater: impl Into<String>) -> Self {
        self.theater = Some(theater.into());
        self
    }

    /// Marks this as a later attempt that replaces the Session `predecessor`.
    pub fn with_attempt(mut self, attempt: u32, predecessor: Option<String>) -> Self {
        self.attempt = attempt.max(1);
        self.predecessor = predecessor;
        self
    }

    /// This Session as the attempt after it, with a new UID, replacing the record this one
    /// would have been; `None` past the last attempt a record can count.
    pub fn next_attempt(mut self) -> Option<Self> {
        self.attempt = self.attempt.checked_add(1)?;
        self.predecessor = Some(std::mem::replace(&mut self.session_id, new_uid()));
        Some(self)
    }
}

/// Changes a lease holder may make to a Session record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionUpdate {
    provider_session_id: Option<String>,
    status: Option<SessionStatus>,
    parked_until: Option<f64>,
    exposure: Vec<Exposure>,
    launch: Option<SessionLaunch>,
    recent: Option<Vec<String>>,
    checkpoint: Option<(SafeHouseCheckpoint, f64)>,
    account: Option<AccountRef>,
    result: Option<SessionResult>,
    deliverables: Option<Vec<Deliverable>>,
    evidence: Vec<Evidence>,
    probed: bool,
}

impl SessionUpdate {
    /// Records what the Mission was to deliver, replacing the earlier list.
    pub fn deliverables(mut self, deliverables: Vec<Deliverable>) -> Self {
        self.deliverables = Some(deliverables);
        self
    }

    /// Adds what probes found; the oldest entries beyond `[registry.limits.sessions]
    /// evidence_entries` go first.
    pub fn evidence(mut self, evidence: Vec<Evidence>) -> Self {
        self.evidence = evidence;
        self
    }

    /// Answers the request for a probe: it has been run.
    pub fn probed(mut self) -> Self {
        self.probed = true;
        self
    }

    /// Records how the Mission stands, replacing the earlier result.
    pub fn result(mut self, result: SessionResult) -> Self {
        self.result = Some(result);
        self
    }

    /// Records the pool account the Session runs on now, replacing the one recorded.
    pub fn account(mut self, account: AccountRef) -> Self {
        self.account = Some(account);
        self
    }

    /// Records how the harness was launched, replacing what was recorded.
    pub fn launch(mut self, launch: SessionLaunch) -> Self {
        self.launch = Some(launch);
        self
    }

    /// Replaces the recent events with the last `[registry.limits.sessions] recent_events` of
    /// `recent`, each cut to `recent_event_chars` characters.
    pub fn recent(mut self, recent: Vec<String>) -> Self {
        self.recent = Some(recent);
        self
    }

    /// Records a checkpoint saved at `at`, in Unix seconds.
    pub fn checkpoint(mut self, checkpoint: SafeHouseCheckpoint, at: f64) -> Self {
        self.checkpoint = Some((checkpoint, at));
        self
    }

    /// Adds exposures the record does not have yet; an exposure is never removed.
    pub fn exposed(mut self, exposure: Vec<Exposure>) -> Self {
        self.exposure = exposure;
        self
    }

    /// Records the provider's Session ID.
    pub fn provider_session(mut self, id: impl Into<String>) -> Self {
        self.provider_session_id = Some(id.into());
        self
    }

    /// Sets the status. `Active` clears `parked_until`.
    pub fn status(mut self, status: SessionStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Sets when a parked Session may resume, in Unix seconds.
    pub fn parked_until(mut self, until: f64) -> Self {
        self.parked_until = Some(until);
        self
    }
}

/// Why a Session record could not be read or written by a Station.
#[non_exhaustive]
#[derive(Debug)]
pub enum SessionError {
    /// No record with this ID exists.
    NotFound,
    /// A record with this ID already exists.
    Exists,
    /// The Session finished and is closed to every Station.
    Ended,
    /// The attempt has not ended done or held, so there is no outcome to give a verdict on; the
    /// state it stands in.
    Unsettled(String),
    /// Another Station holds a live lease on the Session.
    HeldBy(String),
    /// The Station's lease or fencing token is no longer the current one.
    Stale,
    /// The Registry could not be read or written.
    Io(io::Error),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("no such Session record"),
            Self::Exists => f.write_str("a Session record with this ID already exists"),
            Self::Ended => f.write_str("the Session has ended"),
            Self::Unsettled(state) => {
                write!(
                    f,
                    "the latest attempt has no outcome to judge: it is {state}"
                )
            }
            Self::HeldBy(station) => write!(f, "Station {station:?} holds the Session lease"),
            Self::Stale => f.write_str("the Session lease or fencing token is stale"),
            Self::Io(error) => write!(f, "Session record: {error}"),
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for SessionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub(super) fn validate_session_id(id: &str, limits: &SessionLimits) -> io::Result<()> {
    let valid = !id.is_empty()
        && limits.id_bytes.allows(id.len())
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Session ID must be 1-{} letters, digits, '-', '_' or '.' and not start with '.'",
                limits.id_bytes
            ),
        ))
    }
}

impl FileRegistry {
    fn session_path(&self, id: &str) -> std::path::PathBuf {
        self.root.join("sessions").join(format!("{id}.json"))
    }

    fn headquarters_term(&self) -> io::Result<u64> {
        let path = self.root.join("headquarters").join("lease.json");
        Ok(read_headquarters(&path, self.limits.stations.record_bytes)?
            .map_or(0, |lease| lease.term()))
    }

    pub(crate) fn open_session_at(
        &self,
        new: NewSession,
        ttl: Duration,
        now: f64,
    ) -> Result<SessionRecord, SessionError> {
        validate_new_session(&new, &self.limits.sessions)?;
        let _lock = self.lock()?;
        let path = self.session_path(&new.session_id);
        if path.exists()
            || self
                .sessions_locked()?
                .iter()
                .any(|record| same_run(record, &new))
        {
            return Err(SessionError::Exists);
        }
        if fs::read_dir(self.root.join("sessions"))
            .is_ok_and(|entries| !self.limits.sessions.records.allows(entries.count() + 1))
        {
            return Err(io::Error::other("too many Session records").into());
        }
        let record = opened(new, ttl, now, self.headquarters_term()?);
        self.write_session(&path, &record)?;
        Ok(record)
    }

    pub(crate) fn take_session_at(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
        now: f64,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(id, &self.limits.sessions)?;
        super::store::validate_station_id(station)?;
        let _lock = self.lock()?;
        let path = self.session_path(id);
        let mut record =
            read_session(&path, &self.limits.sessions)?.ok_or(SessionError::NotFound)?;
        take(&mut record, station, ttl, now, || self.headquarters_term())?;
        self.write_session(&path, &record)?;
        Ok(record)
    }

    pub(crate) fn write_fenced(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        now: f64,
        renew: Option<Duration>,
        update: SessionUpdate,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(id, &self.limits.sessions)?;
        let _lock = self.lock()?;
        let path = self.session_path(id);
        let mut record =
            read_session(&path, &self.limits.sessions)?.ok_or(SessionError::NotFound)?;
        apply_fenced(
            &mut record,
            station,
            fence,
            now,
            renew,
            update,
            &self.limits.sessions,
        )?;
        self.write_session(&path, &record)?;
        Ok(record)
    }

    pub(crate) fn lose_session_at(
        &self,
        id: &str,
        reason: &str,
        now: f64,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(id, &self.limits.sessions)?;
        let _lock = self.lock()?;
        let path = self.session_path(id);
        let mut record =
            read_session(&path, &self.limits.sessions)?.ok_or(SessionError::NotFound)?;
        lose(
            &mut record,
            reason,
            now,
            || self.headquarters_term(),
            &self.limits.sessions,
        )?;
        self.write_session(&path, &record)?;
        Ok(record)
    }
}

/// Whether `record` is the run `new` would be: the same attempt at the same Mission, which has
/// one record whatever its UID.
pub(super) fn same_run(record: &SessionRecord, new: &NewSession) -> bool {
    record.mission_id == new.mission_id && record.attempt == new.attempt
}

/// Checks the identities of a Session to record.
pub(super) fn validate_new_session(new: &NewSession, limits: &SessionLimits) -> io::Result<()> {
    validate_session_id(&new.session_id, limits)?;
    super::store::validate_station_id(&new.station)
}

/// A new Session's record: active, held by its Station from `now`, fenced at the
/// Headquarters `term` (at least 1).
pub(super) fn opened(new: NewSession, ttl: Duration, now: f64, term: u64) -> SessionRecord {
    SessionRecord {
        handle: new
            .mission_handle
            .map(|handle| run_handle(&handle, new.attempt)),
        session_id: new.session_id,
        mission_id: new.mission_id,
        operation: new.operation,
        provider_session_id: new.provider_session_id,
        station: new.station,
        theater: new.theater,
        attempt: new.attempt,
        predecessor: new.predecessor,
        status: SessionStatus::Active,
        parked_until: None,
        lease_expires: now + ttl.as_secs_f64(),
        fence: term.max(1),
        exposure: new.exposure,
        launch: new.launch,
        recent: Vec::new(),
        checkpoint: None,
        checkpointed_at: None,
        lost: None,
        account: new.account,
        opened_at: Some(now),
        closed_at: None,
        result: None,
        window: new.window,
        verdict: None,
        deliverables: Vec::new(),
        evidence: Vec::new(),
        probe_requested: None,
    }
}

/// Gives `station` the lease on `record` until `now + ttl`. A lapsed lease of another
/// Station is taken over with a fence above both its own and the Headquarters `term`.
pub(super) fn take(
    record: &mut SessionRecord,
    station: &str,
    ttl: Duration,
    now: f64,
    term: impl FnOnce() -> io::Result<u64>,
) -> Result<(), SessionError> {
    if record.status.is_closed() {
        return Err(SessionError::Ended);
    }
    if record.station != station {
        if record.leased_at(now) {
            return Err(SessionError::HeldBy(record.station.clone()));
        }
        record.station = station.to_owned();
        record.fence = term()?.max(record.fence + 1);
    }
    record.lease_expires = now + ttl.as_secs_f64();
    Ok(())
}

/// Applies `update` (and a renewal) to `record` if `station` holds it with `fence`.
pub(super) fn apply_fenced(
    record: &mut SessionRecord,
    station: &str,
    fence: u64,
    now: f64,
    renew: Option<Duration>,
    update: SessionUpdate,
    limits: &SessionLimits,
) -> Result<(), SessionError> {
    if record.station != station || record.fence != fence {
        return Err(SessionError::Stale);
    }
    if record.status.is_closed() {
        return Err(SessionError::Ended);
    }
    if let Some(ttl) = renew {
        record.lease_expires = now + ttl.as_secs_f64();
    }
    if let Some(provider) = update.provider_session_id {
        record.provider_session_id = Some(provider);
    }
    if let Some(launch) = update.launch {
        record.launch = Some(launch);
    }
    if let Some(account) = update.account {
        record.account = Some(account);
    }
    if let Some(recent) = update.recent {
        let skip = recent.len().saturating_sub(limits.recent_events.or_max());
        record.recent = recent
            .into_iter()
            .skip(skip)
            .map(|line| {
                line.chars()
                    .take(limits.recent_event_chars.or_max())
                    .collect()
            })
            .collect();
    }
    if let Some((checkpoint, at)) = update.checkpoint {
        record.checkpoint = Some(checkpoint);
        record.checkpointed_at = Some(at);
    }
    for exposure in update.exposure {
        if limits.exposures.allows(record.exposure.len() + 1)
            && !record
                .exposure
                .iter()
                .any(|known| known.source == exposure.source)
        {
            record.exposure.push(exposure);
        }
    }
    if let Some(deliverables) = update.deliverables {
        let skip = deliverables
            .len()
            .saturating_sub(limits.deliverables.or_max());
        record.deliverables = deliverables.into_iter().skip(skip).collect();
    }
    if !update.evidence.is_empty() {
        let detail = limits.evidence_detail_chars.or_max();
        record
            .evidence
            .extend(update.evidence.into_iter().map(|mut found| {
                found.detail = found.detail.chars().take(detail).collect();
                found
            }));
        let skip = record
            .evidence
            .len()
            .saturating_sub(limits.evidence_entries.or_max());
        record.evidence.drain(..skip);
    }
    if update.probed {
        record.probe_requested = None;
    }
    if let Some(result) = update.result {
        let cut = |text: String| {
            text.chars()
                .take(limits.recent_event_chars.or_max())
                .collect()
        };
        record.result = Some(SessionResult {
            state: cut(result.state),
            reason: result.reason.map(cut),
            denied_tools: result.denied_tools,
        });
    }
    if let Some(status) = update.status {
        if status.is_closed() {
            record.closed_at = Some(now);
        }
        record.status = status;
        record.parked_until = match status {
            SessionStatus::Parked => update.parked_until.or(record.parked_until),
            _ => None,
        };
    }
    Ok(())
}

/// Marks `record` lost for `reason` once its lease lapsed, moving its fence past the
/// holder's and the Headquarters `term`.
pub(super) fn lose(
    record: &mut SessionRecord,
    reason: &str,
    now: f64,
    term: impl FnOnce() -> io::Result<u64>,
    limits: &SessionLimits,
) -> Result<(), SessionError> {
    if record.status.is_closed() {
        return Err(SessionError::Ended);
    }
    if record.leased_at(now) {
        return Err(SessionError::HeldBy(record.station.clone()));
    }
    record.status = SessionStatus::Lost;
    record.closed_at = Some(now);
    record.parked_until = None;
    record.lost = Some(
        reason
            .chars()
            .take(limits.recent_event_chars.or_max())
            .collect(),
    );
    record.fence = term()?.max(record.fence + 1);
    Ok(())
}

/// Checks a verdict's bounds before it is stored.
pub(super) fn validate_verdict(verdict: &MissionVerdict, limits: &SessionLimits) -> io::Result<()> {
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    if verdict.by.trim().is_empty() || !verdict.at.is_finite() || verdict.at <= 0.0 {
        return Err(invalid(
            "a verdict needs the person who gave it and a positive time".to_owned(),
        ));
    }
    if verdict.note.as_deref().is_some_and(|note| {
        note.trim().is_empty()
            || !limits.verdict_note_bytes.allows(note.len())
            || note
                .chars()
                .any(|character| character.is_control() && !"\n\t".contains(character))
    }) {
        return Err(invalid(format!(
            "the note must be 1 to {} bytes without control characters other than newlines",
            limits.verdict_note_bytes
        )));
    }
    if let Some(amendment) = &verdict.amendment
        && (verdict.verdict != OutcomeVerdict::Redispatch
            || amendment.trim().is_empty()
            || !limits.amendment_bytes.allows(amendment.len())
            || amendment.chars().any(|character| {
                character.is_control()
                    && !"
	"
                    .contains(character)
            }))
    {
        return Err(invalid(format!(
            "an amendment goes only with a redispatch, within {} bytes and without control              characters other than newlines",
            limits.amendment_bytes
        )));
    }
    Ok(())
}

/// Puts `verdict` on the attempt that ended done or held; a verdict already there is replaced.
pub(super) fn apply_verdict(
    record: &mut SessionRecord,
    verdict: MissionVerdict,
) -> Result<(), SessionError> {
    record.takes_verdict()?;
    record.verdict = Some(verdict);
    Ok(())
}

/// Marks `record` as asked to be probed; only an attempt being verified takes the request.
pub(super) fn apply_probe_request(record: &mut SessionRecord, at: f64) -> Result<(), SessionError> {
    let state = record
        .result
        .as_ref()
        .map_or("unrecorded", |result| result.state.as_str());
    if state != "verifying" || record.status.is_closed() {
        return Err(SessionError::Unsettled(state.to_owned()));
    }
    record.probe_requested = Some(at);
    Ok(())
}

/// Checks a final report's bounds before it is stored.
pub(super) fn validate_final_report(
    report: &FinalReport,
    limits: &SessionLimits,
) -> io::Result<()> {
    validate_session_id(&report.session_id, limits)?;
    if !limits.final_report_bytes.allows(report.text.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "a final report is at most {} bytes",
                limits.final_report_bytes
            ),
        ));
    }
    Ok(())
}

/// Every exposure of `records` (one Mission's Sessions, in ID order), earliest first, one
/// entry per source.
pub(super) fn merged_exposure(records: impl IntoIterator<Item = SessionRecord>) -> Vec<Exposure> {
    let mut merged: Vec<Exposure> = Vec::new();
    for record in records {
        for exposure in record.exposure {
            match merged
                .iter_mut()
                .find(|known| known.source == exposure.source)
            {
                Some(known) => known.at = known.at.min(exposure.at),
                None => merged.push(exposure),
            }
        }
    }
    merged.sort_by(|left, right| left.at.total_cmp(&right.at));
    merged
}

/// The highest attempt among `records`.
pub(super) fn latest(records: impl IntoIterator<Item = SessionRecord>) -> Option<SessionRecord> {
    records.into_iter().max_by(|left, right| {
        left.attempt
            .cmp(&right.attempt)
            .then(left.lease_expires.total_cmp(&right.lease_expires))
    })
}

/// Agent Session records with their fenced leases.
pub trait SessionStore {
    /// Records a new Session held by its Station and returns it with its fencing token.
    ///
    /// # Errors
    ///
    /// `Exists` when the ID is taken or the Mission already has a record of that attempt; `Io`
    /// for an invalid ID, record or Registry failure.
    fn open_session(&self, new: NewSession, ttl: Duration) -> Result<SessionRecord, SessionError>;

    /// Reads one Session record.
    fn session(&self, id: &str) -> io::Result<Option<SessionRecord>>;

    /// Every way any Session of a Mission was exposed, earliest first, one entry per source.
    ///
    /// A later attempt inherits this, so exposure cannot be shed by starting again.
    ///
    /// # Errors
    ///
    /// Returns an error when the Session records cannot be read.
    fn mission_exposure(&self, mission_id: &str) -> io::Result<Vec<Exposure>>;

    /// Lists every Session record in stable ID order.
    fn sessions(&self) -> io::Result<Vec<SessionRecord>>;

    /// Takes the lease on a Session for `station`, or renews it when `station` already holds it.
    ///
    /// A different holder's live lease is respected. A lapsed one is taken over with a fencing
    /// token above the previous holder's, which makes that holder's writes stale.
    ///
    /// # Errors
    ///
    /// `NotFound`, `Ended`, `HeldBy` while another Station's lease is live, or `Io`.
    fn take_session(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
    ) -> Result<SessionRecord, SessionError>;

    /// Extends the lease `station` holds on a Session, if it still holds it with `fence`.
    ///
    /// # Errors
    ///
    /// `Stale` when another Station took the Session or `fence` is not the current token.
    fn renew_session(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        ttl: Duration,
    ) -> Result<SessionRecord, SessionError>;

    /// Applies `update` to a Session if `station` still holds it with `fence`.
    ///
    /// # Errors
    ///
    /// `Stale` when another Station took the Session or `fence` is not the current token.
    fn update_session(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        update: SessionUpdate,
    ) -> Result<SessionRecord, SessionError>;

    /// Marks a Session whose holder stopped renewing its lease as lost, giving the reason.
    ///
    /// The fencing token moves on, so the holder's writes are refused if it comes back.
    ///
    /// # Errors
    ///
    /// `NotFound`, `Ended` for a closed Session, `HeldBy` while its lease is live, or `Io`.
    fn lose_session(&self, id: &str, reason: &str) -> Result<SessionRecord, SessionError>;

    /// The latest attempt at a Mission: the Session record with the highest attempt number.
    ///
    /// # Errors
    ///
    /// Returns an error when the Session records cannot be read.
    fn latest_session(&self, mission_id: &str) -> io::Result<Option<SessionRecord>>;

    /// Records a person's verdict on the latest attempt at a Mission of `operation`, replacing any
    /// it has. It needs no lease: a person's word, not a holder's write.
    ///
    /// # Errors
    ///
    /// `NotFound` when the Operation has no attempt at the Mission; `Unsettled` when the latest
    /// has not ended done or held; `Io` for a verdict out of bounds or a Registry failure.
    fn record_verdict(
        &self,
        operation: &str,
        mission_id: &str,
        verdict: &MissionVerdict,
    ) -> Result<SessionRecord, SessionError>;

    /// Asks for the latest attempt at a Mission of `operation` to have its deliverables probed;
    /// the Station that holds it does so on its next pass and clears the request. It needs no
    /// lease: a person's word, not a holder's write.
    ///
    /// # Errors
    ///
    /// `NotFound` when the Operation has no attempt at the Mission; `Unsettled` when the latest
    /// is not being verified; `Io` for a Registry failure.
    fn request_probe(
        &self,
        operation: &str,
        mission_id: &str,
        at: f64,
    ) -> Result<SessionRecord, SessionError>;

    /// Keeps a Session's final report, replacing the one it has.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an invalid Session ID or a report over its limit; I/O errors otherwise.
    fn file_final_report(&self, report: &FinalReport) -> io::Result<()>;

    /// A Session's final report, if it filed one.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for an invalid Session ID; I/O errors otherwise.
    fn final_report(&self, session_id: &str) -> io::Result<Option<FinalReport>>;
}

impl SessionStore for FileRegistry {
    fn open_session(&self, new: NewSession, ttl: Duration) -> Result<SessionRecord, SessionError> {
        self.open_session_at(new, ttl, unix_time()?)
    }

    fn session(&self, id: &str) -> io::Result<Option<SessionRecord>> {
        validate_session_id(id, &self.limits.sessions)?;
        let _lock = self.lock()?;
        read_session(&self.session_path(id), &self.limits.sessions)
    }

    fn mission_exposure(&self, mission_id: &str) -> io::Result<Vec<Exposure>> {
        Ok(merged_exposure(
            self.sessions()?
                .into_iter()
                .filter(|record| record.mission_id == mission_id),
        ))
    }

    fn sessions(&self) -> io::Result<Vec<SessionRecord>> {
        let _lock = self.lock()?;
        self.sessions_locked()
    }

    fn take_session(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
    ) -> Result<SessionRecord, SessionError> {
        self.take_session_at(id, station, ttl, unix_time()?)
    }

    fn renew_session(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        ttl: Duration,
    ) -> Result<SessionRecord, SessionError> {
        self.write_fenced(
            id,
            station,
            fence,
            unix_time()?,
            Some(ttl),
            SessionUpdate::default(),
        )
    }

    fn update_session(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        update: SessionUpdate,
    ) -> Result<SessionRecord, SessionError> {
        self.write_fenced(id, station, fence, unix_time()?, None, update)
    }

    fn lose_session(&self, id: &str, reason: &str) -> Result<SessionRecord, SessionError> {
        self.lose_session_at(id, reason, unix_time()?)
    }

    fn latest_session(&self, mission_id: &str) -> io::Result<Option<SessionRecord>> {
        Ok(latest(
            self.sessions()?
                .into_iter()
                .filter(|record| record.mission_id == mission_id),
        ))
    }

    fn record_verdict(
        &self,
        operation: &str,
        mission_id: &str,
        verdict: &MissionVerdict,
    ) -> Result<SessionRecord, SessionError> {
        validate_verdict(verdict, &self.limits.sessions)?;
        let _lock = self.lock()?;
        let mut record = latest(self.sessions_locked()?.into_iter().filter(|record| {
            record.mission_id == mission_id && record.operation.as_deref() == Some(operation)
        }))
        .ok_or(SessionError::NotFound)?;
        apply_verdict(&mut record, verdict.clone())?;
        self.write_session(&self.session_path(&record.session_id), &record)?;
        Ok(record)
    }

    fn request_probe(
        &self,
        operation: &str,
        mission_id: &str,
        at: f64,
    ) -> Result<SessionRecord, SessionError> {
        let _lock = self.lock()?;
        let mut record = latest(self.sessions_locked()?.into_iter().filter(|record| {
            record.mission_id == mission_id && record.operation.as_deref() == Some(operation)
        }))
        .ok_or(SessionError::NotFound)?;
        apply_probe_request(&mut record, at)?;
        self.write_session(&self.session_path(&record.session_id), &record)?;
        Ok(record)
    }

    fn file_final_report(&self, report: &FinalReport) -> io::Result<()> {
        validate_final_report(report, &self.limits.sessions)?;
        let bytes = serde_json::to_vec(report).map_err(io::Error::other)?;
        let _lock = self.lock()?;
        self.write_atomic(&self.final_report_path(&report.session_id), &bytes)
    }

    fn final_report(&self, session_id: &str) -> io::Result<Option<FinalReport>> {
        validate_session_id(session_id, &self.limits.sessions)?;
        let _lock = self.lock()?;
        match fs::read(self.final_report_path(session_id)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| invalid_data(format!("invalid final report: {error}"))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl FileRegistry {
    fn final_report_path(&self, id: &str) -> std::path::PathBuf {
        self.root.join("final-reports").join(format!("{id}.json"))
    }

    /// Every Session record, in ID order, under the lock the caller holds.
    fn sessions_locked(&self) -> io::Result<Vec<SessionRecord>> {
        let mut records = Vec::new();
        let entries = match fs::read_dir(self.root.join("sessions")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(records),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && let Some(record) = read_session(&path, &self.limits.sessions)?
            {
                records.push(record);
            }
        }
        records.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        Ok(records)
    }
}

fn read_session(
    path: &std::path::Path,
    limits: &SessionLimits,
) -> io::Result<Option<SessionRecord>> {
    match fs::read(path) {
        Ok(bytes) => parse_session(&bytes, limits).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Reads a stored Session record, refusing one out of bounds.
pub(super) fn parse_session(bytes: &[u8], limits: &SessionLimits) -> io::Result<SessionRecord> {
    if !limits.record_bytes.allows(bytes.len()) {
        return Err(invalid_data("Session record exceeds the size limit"));
    }
    let record: SessionRecord = serde_json::from_slice(bytes)
        .map_err(|error| invalid_data(format!("invalid Session record: {error}")))?;
    if !record.lease_expires.is_finite() || record.attempt == 0 {
        return Err(invalid_data("invalid Session record"));
    }
    Ok(record)
}

/// The stored form of a Session record, within the size limit.
pub(super) fn session_json(record: &SessionRecord, limits: &SessionLimits) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
    if !limits.record_bytes.allows(bytes.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Session record exceeds the size limit",
        ));
    }
    Ok(bytes)
}

impl FileRegistry {
    fn write_session(&self, path: &std::path::Path, record: &SessionRecord) -> io::Result<()> {
        self.write_atomic(path, &session_json(record, &self.limits.sessions)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::registry::store::DEFAULT_LEASE;
    use crate::directorates::registry::suite::{Store, backend_tests};

    backend_tests!(
        two_stations_sharing_a_registry_see_the_same_record,
        a_later_attempt_links_to_its_predecessor,
        a_stale_fencing_token_is_refused,
        a_live_lease_cannot_be_taken_but_the_holder_renews_it,
        the_fencing_token_follows_the_headquarters_term_and_only_grows,
        status_changes_are_fenced_and_an_ended_session_is_closed,
        a_session_keeps_its_agent_times_and_the_result_of_its_last_write,
        only_a_lapsed_session_is_lost_and_its_holder_is_fenced_out,
        the_holder_records_launch_recent_events_and_checkpoints_within_bounds,
        the_latest_session_is_the_highest_attempt,
        unsafe_ids_are_rejected_before_touching_the_filesystem,
        a_verdict_lands_on_the_latest_attempt_that_ended,
        a_verdict_is_refused_on_an_attempt_with_no_outcome,
        a_final_report_is_kept_by_its_session_and_cut_when_long,
    );

    const NOW: f64 = 1_800_000_000.0;
    const TTL: Duration = Duration::from_secs(60);

    fn new_session() -> NewSession {
        NewSession::new("s-1", "mission-1", "station-a").with_theater("local")
    }

    fn two_stations_sharing_a_registry_see_the_same_record(store: &dyn Store) {
        let first = store.handle();
        let second = store.handle();
        let opened = first
            .open_session_at(new_session().with_provider_session("provider-1"), TTL, NOW)
            .unwrap();

        let seen = second.session("s-1").unwrap().unwrap();
        assert_eq!(seen, opened);
        assert_eq!(seen.station, "station-a");
        assert_eq!(seen.theater.as_deref(), Some("local"));
        assert_eq!(seen.provider_session_id.as_deref(), Some("provider-1"));
        assert_eq!(seen.attempt, 1);
        assert_eq!(second.sessions().unwrap(), vec![seen]);
        assert!(matches!(
            second.open_session_at(new_session(), TTL, NOW),
            Err(SessionError::Exists)
        ));
    }

    fn a_later_attempt_links_to_its_predecessor(store: &dyn Store) {
        let registry = store.handle();
        registry.open_session_at(new_session(), TTL, NOW).unwrap();
        let next = registry
            .open_session_at(
                NewSession::new("s-2", "mission-1", "station-a")
                    .with_attempt(2, Some("s-1".to_owned())),
                TTL,
                NOW,
            )
            .unwrap();
        assert_eq!(
            (next.attempt, next.predecessor.as_deref()),
            (2, Some("s-1"))
        );
    }

    fn a_stale_fencing_token_is_refused(store: &dyn Store) {
        let registry = store.handle();
        let first = registry.open_session_at(new_session(), TTL, NOW).unwrap();

        // The first holder stops renewing; the second takes the lapsed lease.
        let later = NOW + 120.0;
        let taken = registry
            .take_session_at("s-1", "station-b", TTL, later)
            .unwrap();
        assert_eq!(taken.station, "station-b");
        assert!(taken.fence > first.fence);

        // Its old token, and even the right Station name with the old token, are refused.
        let update = SessionUpdate::default().provider_session("late-write");
        for (station, fence) in [("station-a", first.fence), ("station-b", first.fence)] {
            assert!(matches!(
                registry.write_fenced("s-1", station, fence, later, None, update.clone()),
                Err(SessionError::Stale)
            ));
        }
        assert!(matches!(
            registry.write_fenced("s-1", "station-a", taken.fence, later, None, update.clone()),
            Err(SessionError::Stale)
        ));
        let record = registry.session("s-1").unwrap().unwrap();
        assert_eq!(record.provider_session_id, None);

        // The current holder's token still works.
        let written = registry
            .write_fenced("s-1", "station-b", taken.fence, later, None, update)
            .unwrap();
        assert_eq!(written.provider_session_id.as_deref(), Some("late-write"));
    }

    fn a_live_lease_cannot_be_taken_but_the_holder_renews_it(store: &dyn Store) {
        let registry = store.handle();
        let opened = registry.open_session_at(new_session(), TTL, NOW).unwrap();
        assert!(matches!(
            registry.take_session_at("s-1", "station-b", TTL, NOW + 30.0),
            Err(SessionError::HeldBy(holder)) if holder == "station-a"
        ));
        let renewed = registry
            .take_session_at("s-1", "station-a", TTL, NOW + 30.0)
            .unwrap();
        assert_eq!(renewed.fence, opened.fence);
        assert!(renewed.lease_expires > opened.lease_expires);
        assert!(matches!(
            registry.take_session_at("s-1", "station-b", TTL, NOW + 80.0),
            Err(SessionError::HeldBy(_))
        ));
        assert!(
            registry
                .take_session_at("s-1", "station-b", TTL, NOW + 200.0)
                .is_ok()
        );
    }

    fn the_fencing_token_follows_the_headquarters_term_and_only_grows(store: &dyn Store) {
        let registry = store.handle();
        registry.elect("hq-a", DEFAULT_LEASE).unwrap();
        let first = registry.open_session_at(new_session(), TTL, NOW).unwrap();
        assert_eq!(
            first.fence,
            registry.headquarters().unwrap().unwrap().term()
        );

        // A takeover inside the same election term still gets a larger token.
        let taken = registry
            .take_session_at("s-1", "station-b", TTL, NOW + 500.0)
            .unwrap();
        assert!(taken.fence > first.fence);
        let again = registry
            .take_session_at("s-1", "station-c", TTL, NOW + 1_000.0)
            .unwrap();
        assert!(again.fence > taken.fence);
    }

    /// A Mission of Operation `G` whose attempt `attempt` stands in `state`.
    fn attempt_in(store: &dyn Store, mission: &str, attempt: u32, state: &str) -> SessionRecord {
        let registry = store.handle();
        let new = NewSession::new(format!("{mission}-{attempt}"), mission, "station-a")
            .with_operation("G")
            .with_attempt(attempt, None);
        let opened = registry.open_session_at(new, TTL, NOW).unwrap();
        registry
            .write_fenced(
                &opened.session_id,
                "station-a",
                opened.fence,
                NOW,
                None,
                SessionUpdate::default().result(SessionResult::new(state)),
            )
            .unwrap()
    }

    fn a_verdict_lands_on_the_latest_attempt_that_ended(store: &dyn Store) {
        let registry = store.handle();
        attempt_in(store, "m-1", 1, "failed");
        let last = attempt_in(store, "m-1", 2, "done");
        let verdict = MissionVerdict::new(
            OutcomeVerdict::Disputed,
            "person-1",
            NOW,
            Some("the tag was never pushed".to_owned()),
        );
        let judged = registry.record_verdict("G", "m-1", &verdict).unwrap();
        assert_eq!(judged.session_id, last.session_id);
        assert_eq!(judged.verdict.as_ref(), Some(&verdict));
        assert_eq!(
            registry.session(&last.session_id).unwrap().unwrap().verdict,
            Some(verdict)
        );
        assert_eq!(registry.session("m-1-1").unwrap().unwrap().verdict, None);

        // A person changes their mind: the latest verdict stands.
        let confirmed = MissionVerdict::new(OutcomeVerdict::Confirmed, "person-2", NOW + 1.0, None);
        let judged = registry.record_verdict("G", "m-1", &confirmed).unwrap();
        assert_eq!(judged.verdict, Some(confirmed));
        assert_eq!(judged.result.unwrap().state, "done", "its state stays");

        let held = attempt_in(store, "m-2", 1, "held");
        let judged = registry
            .record_verdict(
                "G",
                "m-2",
                &MissionVerdict::new(OutcomeVerdict::FailedVerification, "p", NOW, None),
            )
            .unwrap();
        assert_eq!(judged.session_id, held.session_id);
    }

    fn a_verdict_is_refused_on_an_attempt_with_no_outcome(store: &dyn Store) {
        let registry = store.handle();
        let verdict = MissionVerdict::new(OutcomeVerdict::Disputed, "p", NOW, None);
        assert!(matches!(
            registry.record_verdict("G", "m-none", &verdict),
            Err(SessionError::NotFound)
        ));
        attempt_in(store, "m-1", 1, "done");
        assert!(
            matches!(
                registry.record_verdict("other", "m-1", &verdict),
                Err(SessionError::NotFound)
            ),
            "another Operation's Mission is not found"
        );
        for state in ["working", "blocked", "failed", "abandoned"] {
            let mission = format!("m-{state}");
            attempt_in(store, &mission, 1, state);
            match registry.record_verdict("G", &mission, &verdict) {
                Err(SessionError::Unsettled(found)) => assert_eq!(found, state),
                other => panic!("{state}: {other:?}"),
            }
        }
        // A later attempt that has not ended stands in the way of the earlier one's outcome.
        attempt_in(store, "m-retried", 1, "done");
        attempt_in(store, "m-retried", 2, "working");
        assert!(matches!(
            registry.record_verdict("G", "m-retried", &verdict),
            Err(SessionError::Unsettled(_))
        ));
        for bad in [
            MissionVerdict::new(OutcomeVerdict::Disputed, " ", NOW, None),
            MissionVerdict::new(OutcomeVerdict::Disputed, "p", 0.0, None),
            MissionVerdict::new(OutcomeVerdict::Disputed, "p", NOW, Some(" ".to_owned())),
            MissionVerdict::new(
                OutcomeVerdict::Disputed,
                "p",
                NOW,
                Some("a\u{7}b".to_owned()),
            ),
            MissionVerdict::new(
                OutcomeVerdict::Disputed,
                "p",
                NOW,
                Some("x".repeat(SessionLimits::default().verdict_note_bytes.or_max() + 1)),
            ),
        ] {
            assert!(
                matches!(
                    registry.record_verdict("G", "m-1", &bad),
                    Err(SessionError::Io(error)) if error.kind() == io::ErrorKind::InvalidInput
                ),
                "{bad:?}"
            );
        }
    }

    fn a_final_report_is_kept_by_its_session_and_cut_when_long(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(registry.final_report("s-1").unwrap(), None);
        let first = FinalReport::new("s-1", "m-1", "first", NOW, Limit::at(64));
        registry.file_final_report(&first).unwrap();
        assert_eq!(registry.final_report("s-1").unwrap().as_ref(), Some(&first));
        let later = FinalReport::new("s-1", "m-1", "later turn", NOW + 1.0, Limit::at(64));
        registry.file_final_report(&later).unwrap();
        assert_eq!(registry.final_report("s-1").unwrap(), Some(later));
        assert_eq!(registry.final_report("s-2").unwrap(), None);

        let limit = Limit::at(32);
        let cut = FinalReport::new("s-2", "m-1", &"\u{e9}".repeat(100), NOW, limit);
        assert!(cut.text.ends_with(CUT_MARKER), "{}", cut.text);
        assert!(limit.allows(cut.text.len()), "{}", cut.text.len());
        let whole = FinalReport::new("s-2", "m-1", &"x".repeat(32), NOW, limit);
        assert_eq!(whole.text.len(), 32, "at the limit is not cut");

        let over = FinalReport::new(
            "s-3",
            "m-1",
            &"x".repeat(SessionLimits::default().final_report_bytes.or_max() + 1),
            NOW,
            Limit::UNBOUNDED,
        );
        assert_eq!(
            registry.file_final_report(&over).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(registry.final_report("../x").is_err());
    }

    fn a_session_keeps_its_agent_times_and_the_result_of_its_last_write(store: &dyn Store) {
        let registry = store.handle();
        let launch = SessionLaunch::new("claude", "pipe", "/w").with_agent(Some("007".to_owned()));
        let opened = registry
            .open_session_at(new_session().with_launch(launch), TTL, NOW)
            .unwrap();
        assert_eq!(opened.opened_at, Some(NOW));
        assert_eq!((opened.closed_at, opened.result.clone()), (None, None));

        let blocked = registry
            .write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW + 10.0,
                None,
                SessionUpdate::default().result(
                    SessionResult::new("blocked")
                        .with_reason(Some("green-ink".to_owned()))
                        .with_denied_tools(2),
                ),
            )
            .unwrap();
        assert_eq!(blocked.closed_at, None, "still open");
        let result = blocked.result.unwrap();
        assert_eq!(
            (
                result.state.as_str(),
                result.reason.as_deref(),
                result.denied_tools
            ),
            ("blocked", Some("green-ink"), 2)
        );

        let ended = registry
            .write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW + 90.0,
                None,
                SessionUpdate::default()
                    .result(SessionResult::new("done"))
                    .status(SessionStatus::Ended),
            )
            .unwrap();
        assert_eq!(ended.closed_at, Some(NOW + 90.0));
        let read = registry.session("s-1").unwrap().unwrap();
        assert_eq!(read, ended);
        assert_eq!(read.result.unwrap().state, "done");
        assert_eq!(
            read.launch.unwrap().agent.as_deref(),
            Some("007"),
            "the Agent is stored with the launch"
        );
    }

    fn status_changes_are_fenced_and_an_ended_session_is_closed(store: &dyn Store) {
        let registry = store.handle();
        let opened = registry.open_session_at(new_session(), TTL, NOW).unwrap();
        let parked = registry
            .write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW,
                None,
                SessionUpdate::default()
                    .status(SessionStatus::Parked)
                    .parked_until(NOW + 3_600.0),
            )
            .unwrap();
        assert_eq!(parked.status, SessionStatus::Parked);
        assert_eq!(parked.parked_until, Some(NOW + 3_600.0));
        let active = registry
            .write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW,
                None,
                SessionUpdate::default().status(SessionStatus::Active),
            )
            .unwrap();
        assert_eq!(active.parked_until, None);
        registry
            .write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW,
                None,
                SessionUpdate::default().status(SessionStatus::Ended),
            )
            .unwrap();
        assert!(matches!(
            registry.take_session_at("s-1", "station-b", TTL, NOW + 1_000.0),
            Err(SessionError::Ended)
        ));
        assert!(matches!(
            registry.write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW,
                None,
                SessionUpdate::default()
            ),
            Err(SessionError::Ended)
        ));
    }

    fn only_a_lapsed_session_is_lost_and_its_holder_is_fenced_out(store: &dyn Store) {
        let registry = store.handle();
        let opened = registry.open_session_at(new_session(), TTL, NOW).unwrap();
        assert!(matches!(
            registry.lose_session_at("s-1", "gone dark", NOW + 30.0),
            Err(SessionError::HeldBy(holder)) if holder == "station-a"
        ));

        let lost = registry
            .lose_session_at("s-1", "Station station-a went dark", NOW + 120.0)
            .unwrap();
        assert_eq!(lost.status, SessionStatus::Lost);
        assert_eq!(lost.lost.as_deref(), Some("Station station-a went dark"));
        assert!(lost.fence > opened.fence);
        assert!(!lost.leased_at(NOW));

        // The old holder is refused, and nobody can take or lose it again.
        assert!(matches!(
            registry.write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW + 130.0,
                Some(TTL),
                SessionUpdate::default()
            ),
            Err(SessionError::Stale)
        ));
        assert!(matches!(
            registry.take_session_at("s-1", "station-b", TTL, NOW + 130.0),
            Err(SessionError::Ended)
        ));
        assert!(matches!(
            registry.lose_session_at("s-1", "again", NOW + 130.0),
            Err(SessionError::Ended)
        ));
    }

    fn the_holder_records_launch_recent_events_and_checkpoints_within_bounds(store: &dyn Store) {
        let registry = store.handle();
        let launch = SessionLaunch::new("claude", "pty", "/srv/work")
            .with_model(Some("sonnet".to_owned()), None)
            .with_mode(ExecutionMode::Field)
            .with_runner("key-1");
        let opened = registry
            .open_session_at(new_session().with_launch(launch.clone()), TTL, NOW)
            .unwrap();
        assert_eq!(opened.launch.as_ref(), Some(&launch));

        let many = (0..20 + 5)
            .map(|index| format!("event {index} {}", "x".repeat(300)))
            .collect::<Vec<_>>();
        let checkpoint = SafeHouseCheckpoint {
            remote: "origin".to_owned(),
            branch: "mission-1".to_owned(),
            commit: "c0ffee".to_owned(),
            base: Some("base".to_owned()),
        };
        let written = registry
            .write_fenced(
                "s-1",
                "station-a",
                opened.fence,
                NOW,
                None,
                SessionUpdate::default()
                    .recent(many)
                    .checkpoint(checkpoint.clone(), NOW + 1.0),
            )
            .unwrap();
        assert_eq!(written.recent.len(), 20);
        assert!(written.recent[0].starts_with("event 5 "));
        assert!(
            written
                .recent
                .iter()
                .all(|line| line.chars().count() == 300)
        );
        assert_eq!(written.checkpoint, Some(checkpoint));
        assert_eq!(written.checkpointed_at, Some(NOW + 1.0));
        assert_eq!(registry.session("s-1").unwrap().unwrap(), written);
    }

    fn the_latest_session_is_the_highest_attempt(store: &dyn Store) {
        let registry = store.handle();
        assert_eq!(registry.latest_session("mission-1").unwrap(), None);
        registry.open_session_at(new_session(), TTL, NOW).unwrap();
        registry
            .open_session_at(
                NewSession::new("s-0", "mission-1", "station-b")
                    .with_attempt(2, Some("s-1".to_owned())),
                TTL,
                NOW,
            )
            .unwrap();
        registry
            .open_session_at(NewSession::new("s-9", "mission-2", "station-b"), TTL, NOW)
            .unwrap();
        let latest = registry.latest_session("mission-1").unwrap().unwrap();
        assert_eq!((latest.session_id.as_str(), latest.attempt), ("s-0", 2));
    }

    #[test]
    fn records_written_before_adoption_still_load() {
        let root = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(root.path());
        registry.open_session_at(new_session(), TTL, NOW).unwrap();
        let path = registry.session_path("s-1");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let object = value.as_object_mut().unwrap();
        for key in ["launch", "recent", "checkpoint", "checkpointed_at", "lost"] {
            object.remove(key);
        }
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let record = registry.session("s-1").unwrap().unwrap();
        assert_eq!((record.launch, record.recent.len()), (None, 0));
    }

    fn unsafe_ids_are_rejected_before_touching_the_filesystem(store: &dyn Store) {
        let registry = store.handle();
        for id in ["", "../escape", "a/b", ".hidden", &"x".repeat(129)] {
            assert!(matches!(
                registry.open_session_at(NewSession::new(id, "m", "station-a"), TTL, NOW),
                Err(SessionError::Io(_))
            ));
        }
        assert!(registry.session("../escape").is_err());
    }
}
