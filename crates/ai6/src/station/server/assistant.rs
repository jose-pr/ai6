//! Moneypenny on a Station: the assistant Sessions of Universal Exports conversations.
//!
//! Universal Exports queues an [`AssistantJob`] for a Station each time a person writes to
//! Moneypenny. The desk keeps one harness per conversation, started with the roster's
//! assistant Agent (its handler, model, effort and the model credentials its environment
//! carries) in [`AssistantMode`]: no built-in tool and exactly one MCP server, Universal
//! Exports' endpoint, reached with the job's token. A running harness takes the next message
//! as it is; one idle for `[assistant.desk] idle` is closed, and the next message resumes its
//! provider
//! Session. What the harness says is reported back as [`AssistantReport`]s. A harness that
//! fails or exits is reported `failed`, with its handler's reason, and closed at once, which
//! frees its slot; so is one that writes nothing for that long while answering. A resume that
//! fails, when asked or before the resumed harness confirms its Session, goes to the
//! Station's log and the conversation starts afresh with its recap. A route that reports a
//! limit before the turn showed text or called a tool hands the same message to the next route
//! with room ([`relay`]); after either, the turn ends and the next message takes another route.
//!
//! An assistant Session is not a Mission: it has no Operation, and is never on the board. A
//! job that grants the person Cable actions in some Operations gives the harness a further
//! MCP server, this Station's Cable endpoint for the conversation
//! ([`super::cables_mcp::AssistantEndpoint`]), which holds every Cable she sends until the
//! person confirms it, and then sends it as the assistant for the person. A harness that closes
//! leaves what is held with the desk: the person's confirmation needs no harness, and the Cable
//! waits its hour whether or not one runs.
//! It works in a directory of its own, named for its conversation inside
//! one the desk made readable by the Station's user only: the same directory each time the
//! conversation's harness starts, because Claude Code finds a Session to resume only from the
//! directory it was recorded in. The directory goes when the harness closes. A job that names
//! a workspace fills it with the person's repositories and saves her work after each answer
//! ([`workspace`]); one that gives skills has them installed there from this Station's own
//! catalog, each checked against its pin ([`skills`]).
//!
//! A Station running elevated (root, or an administrator's elevated token) refuses every job
//! unless the roster's `assistant.allow_elevated` is set, which is logged as a weakening each
//! time a harness starts. So does a Station that also runs Missions on its own host, unless
//! `assistant.allow_beside_local_missions` is set: nothing separates the OS user of her harness,
//! whose environment holds her tokens, from theirs.

mod relay;
mod skills;
mod workspace;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use ureq::tls::TlsConfig;
use url::Url;

use super::accounts::{self, Holder, Site};
#[cfg(feature = "cables-mcp")]
use super::cables_mcp::{AssistantEndpoint, AssistantIdentity, CableGate, Standing};
use super::routes::{self, OnRoute};
use super::{DaemonState, credentials, medical, station_terminal, unix_now, vetting};
use crate::bounds::TimeLimit;
use crate::directorates::m::assistant::{AssistantDeskLimits, AssistantLimits, AssistantSettings};
use crate::directorates::m::identity::{MissionId, SessionId};
use crate::directorates::m::roster::{Agent, RouteRef};
use crate::directorates::medical::usage::UsageTracker;
use crate::directorates::q::terminal::TerminalBridge;
use crate::directorates::registry::contexts::ContextOwner;
use crate::directorates::section_00::agent::{
    AgentHandler, AgentResume, AgentStart, AssistantMode, SessionRecordFile, SessionRecordQuery,
    checked_session_id,
};
use crate::directorates::section_00::assistant::{
    ASSISTANT_MCP_PATH, ASSISTANT_MCP_TOKEN_ENV, ASSISTANT_MCP_URL_ENV, AssistantEvent,
    AssistantJob, AssistantPageFile, AssistantReach, AssistantReport, AssistantRun,
    AssistantSharedBase, EVENT_ACCEPTED, EVENT_CABLE_PROPOSAL, EVENT_CABLE_SENT, EVENT_CLOSED,
    EVENT_COMPLETED, EVENT_FAILED, EVENT_INTERRUPTED, EVENT_LIMIT, EVENT_NOTICE, EVENT_REJECTED,
    EVENT_TEXT, EVENT_TOOL, page_call_digest,
};
use crate::directorates::section_00::content::{CONTENT_PREFIX, MessageContent, MessageImage};
use crate::guatiao::library::Object;
use crate::station::publisher::gateway_agent;
use coqui_acp_provider::{
    AgentEvent, AgentMessage, AgentSession, CANCELLED_EVENT, DENIED_EVENT, LIMIT_EVENT,
    TURN_CANCELLED,
};
use workspace::{Houses, PERSONAL, SCRATCH, Workspace, sha256_hex};

/// How long one read of a harness's events waits, so a message never waits long for the lock.
const READ_SLICE_MS: i64 = 250;
/// The prefix of the directory that holds the assistants' working directories.
const AREA_PREFIX: &str = "ai6-assistant-";
/// How long a stopping desk waits between looks at its queue.
const STOP_CHECK: Duration = Duration::from_millis(250);

/// The `[assistant]` tables the desk works by, read afresh each time they are asked for:
/// `[assistant.limits]` and `[assistant.desk]`.
type Tables = Arc<dyn Fn() -> (AssistantLimits, AssistantDeskLimits) + Send + Sync>;

/// [`Tables`] read from the Station's roster as it is now.
fn roster_tables(state: &DaemonState) -> Tables {
    let state = state.clone();
    Arc::new(move || {
        let current = state.current();
        let settings = current.roster.assistant();
        (settings.limits().clone(), settings.desk().clone())
    })
}

/// A harness, shared by the desk (to send it messages) and its pump (to read it).
// A read lock serves the pump while it waits on the harness's output: an exclusive one, taken
// again the moment it is let go, keeps a message or a Stop waiting for as long as the harness
// is silent. Only replacing the harness takes the write lock.
type SharedSession = Arc<RwLock<Object<dyn AgentSession>>>;

/// What became of a report the desk's link sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Delivery {
    /// Universal Exports took it.
    Taken,
    /// Universal Exports refused it (403, 404 or 409): it no longer waits for it from here.
    Refused,
    /// It did not reach Universal Exports, or Universal Exports failed it otherwise.
    Lost,
}

/// Told what became of a report once the link sent it; dropped untold when it never was.
pub(super) type Answered = tokio::sync::oneshot::Sender<Delivery>;

/// Queues one report for Universal Exports behind every report queued before it; it must not
/// block for long. A job's answer comes with an [`Answered`].
pub(super) type Reporter = Arc<dyn Fn(AssistantReport, Option<Answered>) + Send + Sync>;

/// Whether `report` carries what must reach Universal Exports, so no queue drops it for room:
/// a job's answer, a Cable held for the person or a Cable sent.
fn must_deliver(report: &AssistantReport) -> bool {
    report.events.iter().any(|event| {
        matches!(
            event.kind.as_str(),
            EVENT_ACCEPTED | EVENT_REJECTED | EVENT_CABLE_PROPOSAL | EVENT_CABLE_SENT
        )
    })
}

/// A report waiting for the desk's link to Universal Exports.
type Queued = (AssistantReport, Option<Answered>);

/// The desk's reports in the order Universal Exports relies on: from before a harness gets a
/// message until Universal Exports has its `accepted`, the conversation's reports wait, and
/// once that `accepted` is refused none is sent but the Cables the harness sent, which
/// Universal Exports audits. Ordinary reports waiting
/// for their message's answer are bounded by `[assistant.desk] report_queue`, past which the
/// newest are dropped and the gateway sees a gap; a report [`must_deliver`] marks is never
/// dropped for room.
#[derive(Clone)]
struct Outbox {
    report: Reporter,
    held: Arc<Mutex<BTreeMap<String, Held>>>,
    tables: Tables,
}

/// A conversation's reports while its message's answer is unsettled, or after it was refused.
enum Held {
    Waiting(Vec<AssistantReport>),
    /// Universal Exports refused the message's `accepted`: the harness is closing, and nothing
    /// it says is sent but its `cable_sent` events.
    Refused,
}

/// `report` with only its events of `kind`, when it has any.
fn only(report: &AssistantReport, kind: &str) -> Option<AssistantReport> {
    let events: Vec<AssistantEvent> = report
        .events
        .iter()
        .filter(|event| event.kind == kind)
        .cloned()
        .collect();
    (!events.is_empty()).then(|| AssistantReport {
        events,
        session_id: None,
        ..report.clone()
    })
}

impl Outbox {
    fn new(report: Reporter) -> Self {
        Self {
            report,
            held: Arc::default(),
            tables: Arc::new(Default::default),
        }
    }

    /// Works by `tables` in place of the defaults.
    fn with_tables(mut self, tables: Tables) -> Self {
        self.tables = tables;
        self
    }

    /// `[assistant.desk]` as it is now.
    fn desk(&self) -> AssistantDeskLimits {
        (self.tables)().1
    }

    /// Hands `report` to the link, its texts cut to `[assistant.limits] event_bytes`.
    fn emit(&self, mut report: AssistantReport, answered: Option<Answered>) {
        report.cut_texts(&(self.tables)().0);
        (self.report)(report, answered);
    }

    /// Sends `report`, or keeps it while its conversation's message is unsettled.
    fn send(&self, report: AssistantReport) {
        let Ok(mut held) = self.held.lock() else {
            return self.deliver(report);
        };
        match held.get_mut(&report.conversation_id) {
            None => self.deliver(report),
            Some(Held::Refused) => self.refused(&report),
            Some(Held::Waiting(waiting))
                if self.desk().report_queue.allows(waiting.len() + 1) || must_deliver(&report) =>
            {
                waiting.push(report);
            }
            Some(Held::Waiting(_)) => eprintln!(
                "ai6: an assistant report was dropped: too many wait for their message's answer"
            ),
        }
    }

    /// What is sent of a report of a harness whose `accepted` was refused: a Cable it sent, for
    /// the audit log.
    fn refused(&self, report: &AssistantReport) {
        if let Some(sent) = only(report, EVENT_CABLE_SENT) {
            self.deliver(sent);
        }
    }

    /// Hands `report` to the link.
    fn deliver(&self, report: AssistantReport) {
        self.emit(report, None);
    }

    /// Keeps `conversation`'s reports until [`Outbox::answer`] settles its message.
    fn hold(&self, conversation: &str) {
        if let Ok(mut held) = self.held.lock() {
            held.insert(conversation.to_owned(), Held::Waiting(Vec::new()));
        }
    }

    /// Sends at once the reports waiting for `conversation`'s message that only say a harness
    /// closed: called with no harness running, so that harness is the one the message replaces.
    /// Told after the message's `accepted` they would read as her closing before she answered.
    fn release_closed(&self, conversation: &str) {
        let closed = |report: &AssistantReport| {
            !report.events.is_empty()
                && report.events.iter().all(|event| event.kind == EVENT_CLOSED)
        };
        let released: Vec<AssistantReport> = match self.held.lock() {
            Ok(mut held) => match held.get_mut(conversation) {
                Some(Held::Waiting(waiting)) => {
                    let (released, kept) = std::mem::take(waiting).into_iter().partition(closed);
                    *waiting = kept;
                    released
                }
                _ => Vec::new(),
            },
            Err(_) => Vec::new(),
        };
        for report in released {
            self.deliver(report);
        }
    }

    /// Sends a job's answer behind every report queued before it and waits until it was sent;
    /// answers whether Universal Exports refused it. For a `message` job the reports held
    /// meanwhile follow it, or, when it was a refused `accepted`, are dropped with all later
    /// ones until the harness has closed ([`Outbox::closed`]), but for what
    /// [`Outbox::refused`] keeps.
    async fn answer(&self, report: AssistantReport, message: bool) -> bool {
        let conversation = report.conversation_id.clone();
        let accepted = report
            .events
            .first()
            .is_some_and(|event| event.kind == EVENT_ACCEPTED);
        let (answered, delivery) = tokio::sync::oneshot::channel();
        self.emit(report, Some(answered));
        // An answer that never reached Universal Exports is not a refusal.
        let delivery = delivery.await;
        let refused = delivery == Ok(Delivery::Refused);
        if message && let Ok(mut held) = self.held.lock() {
            if accepted && refused {
                if let Some(Held::Waiting(waiting)) = held.insert(conversation, Held::Refused) {
                    for report in &waiting {
                        self.refused(report);
                    }
                }
                return refused;
            }
            if let Some(Held::Waiting(waiting)) = held.remove(&conversation) {
                // Still under the lock, so a report sent meanwhile goes after these.
                for report in waiting {
                    self.deliver(report);
                }
            }
        }
        refused
    }

    /// Forgets that `conversation`'s `accepted` was refused, once its harness has closed.
    fn closed(&self, conversation: &str) {
        if let Ok(mut held) = self.held.lock()
            && matches!(held.get(conversation), Some(Held::Refused))
        {
            held.remove(conversation);
        }
    }
}

/// Whether `job` carries a message for the harness, rather than a control or a commit.
fn carries_message(job: &AssistantJob) -> bool {
    !(job.close
        || job.interrupt
        || job.commit.is_some()
        || job.cable_confirm.is_some()
        || job.cable_discard.is_some())
}

/// Tells whether the Station runs elevated: [`crate::station::elevation::is_elevated`], or a
/// test's.
type ElevationProbe = Arc<dyn Fn() -> std::io::Result<bool> + Send + Sync>;

/// Writes one line to the Station's log, `ai6: ` added.
type Logger = Arc<dyn Fn(&str) + Send + Sync>;

/// A conversation's workspace, shared by the desk (a turn starts, a commit arrives) and its
/// pump (a turn ends, the harness closes).
type SharedWorkspace = Arc<Mutex<Workspace>>;

/// What the desk asks of Universal Exports while a conversation's harness runs.
pub(super) trait Exports: Send + Sync {
    /// Whether the person of `conversation` holds `cable.send` and `cable.read` on `operation`
    /// now, asked before every Cable call.
    #[cfg(feature = "cables-mcp")]
    fn standing(&self, conversation: &str, operation: &str) -> Result<Standing, String>;
    /// Uploads one file of the page `publish_page` is publishing for `conversation`, the call
    /// being named by `digest` ([`page_call_digest`]).
    fn upload_page_file(
        &self,
        _conversation: &str,
        _digest: &str,
        _path: &str,
        _bytes: Vec<u8>,
    ) -> Result<(), String> {
        Err("no Universal Exports to upload to".to_owned())
    }
    /// Ends that upload with the manifest of what was sent, or the reason nothing was.
    fn finish_page(
        &self,
        _conversation: &str,
        _digest: &str,
        _manifest: Result<Vec<AssistantPageFile>, String>,
    ) -> Result<(), String> {
        Err("no Universal Exports to upload to".to_owned())
    }
    /// The bytes of the image named `sha256` that the person of `conversation` sent, at most
    /// `most` of them.
    fn image(&self, _conversation: &str, _sha256: &str, _most: u64) -> Result<Vec<u8>, String> {
        Err("no Universal Exports to read the image from".to_owned())
    }
}

/// No Universal Exports to ask: every answer is an error, which refuses the Cable call.
struct Unreachable;

impl Exports for Unreachable {
    #[cfg(feature = "cables-mcp")]
    fn standing(&self, _conversation: &str, _operation: &str) -> Result<Standing, String> {
        Err("no Universal Exports to ask".to_owned())
    }
}

/// A conversation's Cable endpoint's view of [`Exports`]: no answer refuses the call.
#[cfg(feature = "cables-mcp")]
struct ExportsGate(Arc<dyn Exports>);

#[cfg(feature = "cables-mcp")]
impl CableGate for ExportsGate {
    fn standing(&self, conversation: &str, operation: &str) -> Option<Standing> {
        self.0.standing(conversation, operation).ok()
    }
}

/// A running harness: its Session, flags, token expiry, what it started under
/// ([`Desk::started_under`]), and its workspace.
type Running = (
    SharedSession,
    Arc<Flags>,
    f64,
    String,
    Option<SharedWorkspace>,
);

/// The settings a handler starts with: its route's, or its Agent's.
type HandlerSettings = serde_json::Map<String, serde_json::Value>;

/// Starts and resumes harnesses: the Station's own handlers and terminal, or a test's.
pub(super) trait Harness: Send + Sync {
    /// Whether `handler` can be started here.
    fn available(&self, handler: &str) -> bool;
    /// Starts a harness with `handler` and its `settings`.
    fn start(
        &self,
        handler: &str,
        settings: &HandlerSettings,
        request: AgentStart,
    ) -> Result<Object<dyn AgentSession>, String>;
    /// Resumes a provider Session with `handler` and its `settings`.
    fn resume(
        &self,
        handler: &str,
        settings: &HandlerSettings,
        request: AgentResume,
    ) -> Result<Object<dyn AgentSession>, String>;
    /// Why `handler`'s harness, started with `environment`, cannot run scripts in its own
    /// sandbox here; `None` when it can.
    fn scripts(&self, handler: &str, environment: &BTreeMap<String, String>) -> Option<String> {
        let _ = (handler, environment);
        Some("this Station's harness runs no scripts".to_owned())
    }
    /// Why `handler`'s harness cannot load exactly an assistant's skills; `None` when it can.
    fn skills(&self, handler: &str) -> Option<String> {
        let _ = handler;
        None
    }
    /// Why `handler`'s harness cannot be shown images; `None` when it reads a message in the
    /// content form.
    fn images(&self, handler: &str) -> Option<String> {
        let _ = handler;
        Some("this Station's harness takes no image".to_owned())
    }
    /// Where `handler`'s harness keeps its record of the Session `query` names, if it says.
    fn record(
        &self,
        handler: &str,
        settings: &HandlerSettings,
        query: SessionRecordQuery,
    ) -> Option<SessionRecordFile> {
        let _ = (handler, settings, query);
        None
    }
}

/// The Station's handlers, each on the terminal provider the Station chooses for it.
struct StationHarness(DaemonState);

/// A handler, the terminal its harness is to start in, and that terminal's provider.
type Launch = (
    Box<dyn AgentHandler>,
    Object<dyn crate::directorates::q::terminal::AgentTerminal>,
    String,
);

/// Logs which handler and terminal provider an assistant harness got; `session` is the
/// `assistant--<conversation digest>` its Session is started under.
fn log_start(session: &MissionId, how: &str, handler: &str, terminal: &str) {
    let conversation = session.as_str().trim_start_matches("assistant--");
    super::log_harness_start(
        &format!("assistant conversation {conversation}"),
        how,
        handler,
        terminal,
    );
}

impl StationHarness {
    /// `handler` with `settings`, and a terminal that suits what it declares.
    fn handler(&self, handler: &str, settings: &HandlerSettings) -> Result<Launch, String> {
        let handler = self
            .0
            .plugins
            .handler_with(handler, settings)
            .map_err(|error| error.to_string())?;
        let stdio = handler.stdio().map_err(|error| error.to_string())?;
        let provider = station_terminal(&self.0, stdio)?;
        let terminal = self
            .0
            .plugins
            .terminal(&provider, None)
            .map_err(|error| error.to_string())?;
        Ok((handler, TerminalBridge::agent_terminal(terminal), provider))
    }
}

impl Harness for StationHarness {
    fn available(&self, handler: &str) -> bool {
        super::available_handlers(&self.0)
            .iter()
            .any(|id| id == handler)
    }

    fn start(
        &self,
        handler: &str,
        settings: &HandlerSettings,
        request: AgentStart,
    ) -> Result<Object<dyn AgentSession>, String> {
        let (launch, terminal, provider) = self.handler(handler, settings)?;
        let session = request.mission_id.clone();
        let started = crate::station::acp::start(launch, terminal, request)
            .map_err(|error| error.to_string())?;
        log_start(&session, "started", handler, &provider);
        Ok(started)
    }

    fn resume(
        &self,
        handler: &str,
        settings: &HandlerSettings,
        request: AgentResume,
    ) -> Result<Object<dyn AgentSession>, String> {
        let (launch, terminal, provider) = self.handler(handler, settings)?;
        let session = request.mission_id.clone();
        let resumed = crate::station::acp::resume(launch, terminal, request)
            .map_err(|error| error.to_string())?;
        log_start(&session, "resumed", handler, &provider);
        Ok(resumed)
    }

    fn scripts(&self, handler: &str, environment: &BTreeMap<String, String>) -> Option<String> {
        let handler = match self.0.plugins.handler(handler, None) {
            Ok(handler) => handler,
            Err(error) => return Some(error.to_string()),
        };
        handler
            .assistant_scripts(environment.clone())
            .unwrap_or_else(|error| Some(error.to_string()))
    }

    fn skills(&self, handler: &str) -> Option<String> {
        match self.0.plugins.handler(handler, None) {
            Ok(handler) => handler
                .assistant_skills()
                .unwrap_or_else(|error| Some(error.to_string())),
            Err(error) => Some(error.to_string()),
        }
    }

    fn record(
        &self,
        handler: &str,
        settings: &HandlerSettings,
        query: SessionRecordQuery,
    ) -> Option<SessionRecordFile> {
        let handler = self.0.plugins.handler_with(handler, settings).ok()?;
        handler.session_record(query).ok().flatten()
    }

    fn images(&self, handler: &str) -> Option<String> {
        match self.0.plugins.handler(handler, None) {
            Ok(handler) => handler
                .image_input()
                .unwrap_or_else(|error| Some(error.to_string())),
            Err(error) => Some(error.to_string()),
        }
    }
}

/// What a running harness's pump and the desk share.
struct Flags {
    /// The harness is answering a message.
    busy: AtomicBool,
    /// The pump is reading the harness; cleared when it ends.
    alive: AtomicBool,
    /// Asks the pump to close the harness.
    stop: AtomicBool,
    /// The person stopped the answer: the harness's next `completed` is reported as
    /// `interrupted`.
    interrupting: AtomicBool,
    last_active: Mutex<Instant>,
    /// The handler that started the harness.
    handler: String,
    /// The message the turn under way answers, while only that one has reached the harness: what
    /// a limit before the turn did anything sends to another route.
    turn: Mutex<Option<AssistantJob>>,
    /// The pump is moving the turn's message to another route; a message waits for that.
    moving: AtomicBool,
}

struct Live {
    session: SharedSession,
    token_expires_at: f64,
    flags: Arc<Flags>,
    /// What the running harness started under ([`Desk::started_under`]): a later job under
    /// anything else restarts the harness rather than reach it warm with what it began with.
    started: String,
    workspace: Option<SharedWorkspace>,
    /// The conversation's Cable endpoint, while its harness runs.
    #[cfg(feature = "cables-mcp")]
    cables: Option<Arc<AssistantEndpoint>>,
}

/// The endpoints of closed harnesses whose Cables still wait for their person, by conversation.
#[cfg(feature = "cables-mcp")]
type LeftHeld = Arc<Mutex<BTreeMap<String, Arc<AssistantEndpoint>>>>;

/// Why a Station that also runs Missions on its own host refuses her jobs.
const BESIDE_MISSIONS: &str = "this Station also runs Missions on its own host, as the same OS user \
     as Moneypenny's harness: a command in one can read her tokens from her harness's environment \
     and act as the person she serves. So she does not run here. Run her on a Station with no local \
     Theater, or one without the `execution` role, or set assistant.allow_beside_local_missions to \
     accept that";

/// The provider Session each conversation last ran, and on which route, by conversation.
type Known = Arc<Mutex<BTreeMap<String, (String, Option<RouteRef>)>>>;

/// Frees what was issued to a harness unless it comes to run, when its pump frees it instead.
struct Issued {
    state: DaemonState,
    session: String,
    running: bool,
}

impl Drop for Issued {
    fn drop(&mut self) {
        if !self.running {
            credentials::release(&self.state, &self.session);
        }
    }
}

/// The name a conversation's harness, its claims and its leases go under.
fn session_name(conversation: &str) -> String {
    format!("assistant--{}", conversation_digest(conversation))
}

/// The assistant Sessions of one Station.
pub(super) struct Desk {
    state: DaemonState,
    harness: Arc<dyn Harness>,
    outbox: Outbox,
    mcp_url: String,
    /// `[assistant.limits]` and `[assistant.desk]`, read at each job and harness start.
    tables: Tables,
    live: Arc<Mutex<BTreeMap<String, Live>>>,
    #[cfg(feature = "cables-mcp")]
    left_held: LeftHeld,
    /// Harnesses being started, not yet in `live`.
    starting: AtomicUsize,
    known: Known,
    /// Where the conversations' working directories go, made at the first start.
    areas: Mutex<Option<Root>>,
    /// Opens the safe houses of the workspaces' repositories.
    houses: Houses,
    elevated: ElevationProbe,
    /// Whether this Station also runs Missions on its own host; asked at each job, as its roster
    /// can change.
    beside_missions: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The probe's answer, asked once; an error counts as elevated.
    elevation: OnceLock<bool>,
    log: Logger,
    /// The conversation's standing at Universal Exports, and where pages are uploaded.
    exports: Arc<dyn Exports>,
    /// The conversations whose message is moving to another route after a limit.
    moves: Arc<relay::Moves>,
}

impl Desk {
    pub(super) fn new(
        state: DaemonState,
        harness: Arc<dyn Harness>,
        report: Reporter,
        mcp_url: String,
    ) -> Self {
        let plugins = state.plugins.clone();
        let tables = roster_tables(&state);
        let beside_missions: Arc<dyn Fn() -> bool + Send + Sync> = {
            let state = state.clone();
            Arc::new(move || runs_local_missions(&state.current()))
        };
        Self {
            state,
            harness,
            outbox: Outbox::new(report).with_tables(tables.clone()),
            mcp_url,
            tables,
            live: Arc::default(),
            starting: AtomicUsize::new(0),
            known: Arc::default(),
            areas: Mutex::default(),
            houses: Arc::new(move || {
                plugins
                    .safehouse(workspace::PROVIDER, None)
                    .map_err(|error| error.to_string())
            }),
            elevated: Arc::new(crate::station::elevation::is_elevated),
            beside_missions,
            elevation: OnceLock::new(),
            log: Arc::new(|line| eprintln!("ai6: {line}")),
            exports: Arc::new(Unreachable),
            moves: Arc::default(),
            #[cfg(feature = "cables-mcp")]
            left_held: Arc::default(),
        }
    }

    /// Works by `tables` in place of the roster's.
    #[cfg(test)]
    fn with_tables(mut self, tables: Tables) -> Self {
        self.outbox = self.outbox.clone().with_tables(tables.clone());
        self.tables = tables;
        self
    }

    /// Closes a harness idle for `idle` and runs at most `max_live`, the rest of the
    /// roster's tables kept.
    #[cfg(test)]
    fn with_limits(self, idle: Duration, max_live: u64) -> Self {
        let tables = self.tables.clone();
        self.with_tables(Arc::new(move || {
            let (limits, mut desk) = tables();
            desk.idle = TimeLimit::at(idle);
            desk.live = crate::bounds::Limit::at(max_live);
            (limits, desk)
        }))
    }

    #[cfg(test)]
    fn with_beside_missions(mut self, beside: bool) -> Self {
        self.beside_missions = Arc::new(move || beside);
        self
    }

    #[cfg(test)]
    fn with_elevation(mut self, probe: ElevationProbe, log: Logger) -> Self {
        self.elevated = probe;
        self.log = log;
        self
    }

    /// Asks Universal Exports through `exports`.
    fn with_exports(mut self, exports: Arc<dyn Exports>) -> Self {
        self.exports = exports;
        self
    }

    /// Whether the Station runs elevated, asked once; an error counts as elevated.
    fn elevated(&self) -> bool {
        *self
            .elevation
            .get_or_init(|| (self.elevated)().unwrap_or(true))
    }

    /// Refuses every job on an elevated Station unless `assistant.allow_elevated` is set.
    fn check_elevation(&self) -> Result<(), String> {
        if !self.elevated() || self.state.current().roster.assistant().allow_elevated() {
            return Ok(());
        }
        Err(
            "this Station runs elevated (as root or with an administrator's elevated token) \
             and assistant.allow_elevated is false, so Moneypenny does not run here"
                .to_owned(),
        )
    }

    /// Refuses every job on a Station that also runs Missions on its own host, unless
    /// `assistant.allow_beside_local_missions` is set.
    fn check_beside_missions(&self) -> Result<(), String> {
        if !(self.beside_missions)()
            || self
                .state
                .current()
                .roster
                .assistant()
                .allow_beside_local_missions()
        {
            return Ok(());
        }
        Err(BESIDE_MISSIONS.to_owned())
    }

    /// Refuses a job naming a repository this Station's own roster does not give: the person's
    /// personal repository filled in for `person_id`, the shared repositories of the groups the
    /// job names (`groups`, none when it names none) and the organisation's, and a commit only
    /// to a shared one under the group it names. Universal Exports' word is not taken for where a
    /// person's work goes.
    fn check_repositories(&self, job: &AssistantJob) -> Result<(), String> {
        let state = self.state.current();
        let settings = state.roster.assistant();
        let person = job.person_id.as_deref().unwrap_or_default();
        let groups = job.groups.as_deref().unwrap_or_default();
        let personal = || -> Result<Option<String>, String> { settings.personal_repo_for(person) };
        let shared = |name: &str| {
            settings
                .shared_named(name, groups)
                .map(|repo| repo.repo().to_owned())
        };
        let refused = |what: &str| {
            Err(format!(
                "{what} is not one this Station's roster ([assistant]) gives, so nothing was done"
            ))
        };
        if let Some(workspace) = &job.workspace {
            if personal()?.as_deref() != Some(workspace.personal.as_str()) {
                return refused("the person's repository");
            }
            for (name, repo) in &workspace.shared {
                if shared(name).as_deref() != Some(repo.as_str()) {
                    return refused(&format!("shared repository {name:?}"));
                }
            }
        }
        if let Some(commit) = &job.commit
            && shared(&commit.group).as_deref() != Some(commit.repo.as_str())
        {
            return refused("the commit's repository");
        }
        Ok(())
    }

    /// Refuses a job giving a skill this Station's own roster does not give its person: one
    /// outside the catalog (`[assistant.skills]`), or one a table of the person or of a group
    /// the job names leaves out ([`AssistantSettings::skills_for`]); a job with no `person_id`
    /// is given none. Universal Exports' word is not taken for what she may load.
    ///
    /// [`AssistantSettings::skills_for`]: crate::directorates::m::assistant::AssistantSettings::skills_for
    fn check_skills(&self, job: &AssistantJob) -> Result<(), String> {
        if job.skills.is_empty() {
            return Ok(());
        }
        let state = self.state.current();
        let settings = state.roster.assistant();
        let given = match job.person_id.as_deref() {
            Some(person) => settings.skills_for(person, job_groups(job, settings)?),
            None => Vec::new(),
        };
        match job.skills.iter().find(|skill| !given.contains(skill)) {
            Some(skill) => Err(format!(
                "skill {skill:?} is not one this Station's roster ([assistant.skills]) gives the \
                 person, so nothing was done"
            )),
            None => Ok(()),
        }
    }

    /// Takes one job and reports whether the harness has the message, or how a commit went.
    /// Nothing the harness says about a message is reported before Universal Exports has
    /// its `accepted`, and nothing at all once Universal Exports refused that `accepted`: it
    /// no longer waits for the job here (it moved to another Station or was given up), so the
    /// harness closes.
    pub(super) async fn handle(self: &Arc<Self>, job: AssistantJob) {
        let message = carries_message(&job);
        if message {
            self.outbox.hold(&job.conversation_id);
        }
        let outcome = self.take(&job).await;
        let accepted = outcome.is_ok();
        let event = match outcome {
            Ok(text) => AssistantEvent::new(EVENT_ACCEPTED, text),
            Err(reason) => AssistantEvent::new(EVENT_REJECTED, Some(reason)),
        };
        let session_id = self.known_session(&job.conversation_id);
        let route = self.known_route(&job.conversation_id);
        let refused = self
            .outbox
            .answer(
                AssistantReport {
                    route,
                    run: None,
                    station_id: self.state.station_id.clone(),
                    conversation_id: job.conversation_id.clone(),
                    job_id: Some(job.job_id),
                    session_id,
                    events: vec![event],
                },
                message,
            )
            .await;
        if accepted && refused {
            self.close(&job.conversation_id);
        }
    }

    async fn take(self: &Arc<Self>, job: &AssistantJob) -> Result<Option<String>, String> {
        let (bounds, desk) = (self.tables)();
        job.validate(&bounds)?;
        self.check_elevation()?;
        self.check_beside_missions()?;
        self.check_repositories(job)?;
        self.check_skills(job)?;
        if job.close {
            self.close(&job.conversation_id);
            return Ok(None);
        }
        if job.interrupt {
            return self.interrupt(&job.conversation_id).await.map(|()| None);
        }
        if job.commit.is_some() {
            return self.commit(job).await.map(Some);
        }
        if job.cable_confirm.is_some() || job.cable_discard.is_some() {
            return self.decide_cable(job).await.map(|()| None);
        }
        // What this job may run and reach, capped by the Station's roster; a job the roster
        // narrows (a group or person table, or a changed deployment), or under which anything
        // else the harness started with changed, restarts it rather than reach it warm.
        let reach = self.capped_reach(job)?;
        let started = self.started_under(job, &reach);
        loop {
            // A message that comes while the turn's message moves to another route follows it.
            self.wait_moved(&job.conversation_id).await;
            let running = self.running(&job.conversation_id);
            if let Some((session, flags, expires, started_under, workspace)) = running {
                if expires > unix_now() + desk.token_margin.0.as_secs_f64()
                    && started_under == started
                {
                    #[cfg(feature = "cables-mcp")]
                    if let Some(cables) = self.cables_of(&job.conversation_id) {
                        cables.update(&job.cables);
                    }
                    // A message that reaches a harness still answering (Universal Exports gave the
                    // answer up) joins the turn in progress, which keeps the note of `personal/` it
                    // started with.
                    let Some(answering) = Self::join_turn(&flags, job)? else {
                        continue;
                    };
                    *flags
                        .last_active
                        .lock()
                        .map_err(|_| "assistant lock failed".to_owned())? = Instant::now();
                    let text = self.message_for(job, &flags.handler).await?;
                    return tokio::task::spawn_blocking(move || {
                        // Noted before the harness can read the message.
                        if let Some(workspace) = &workspace {
                            let mut workspace = workspace
                                .lock()
                                .map_err(|_| "assistant workspace lock failed".to_owned())?;
                            if !answering {
                                workspace.begin_turn();
                            }
                        }
                        session
                            .read()
                            .map_err(|_| "assistant Session lock failed".to_owned())?
                            .send_message(AgentMessage { text })
                            .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(|error| error.to_string())?
                    .map(|()| None);
                }
                // Its token is about to expire, or what it started under changed: the harness
                // starts again.
                self.close(&job.conversation_id);
            }
            break;
        }
        // A harness that has stopped may still be removing the directory the next one uses.
        self.wait_closed(&job.conversation_id, desk.close_wait.0)
            .await;
        if self
            .live
            .lock()
            .map_err(|_| "assistant lock failed")?
            .contains_key(&job.conversation_id)
        {
            return Err("the conversation's previous harness is still closing".to_owned());
        }
        self.outbox.release_closed(&job.conversation_id);
        self.begin(job, Vec::new(), Vec::new(), None)
            .await
            .map(|()| None)
    }

    /// What `handler`'s harness is sent for the job: its message, or, with images, the message
    /// in the content form with the images read from Universal Exports. A handler that takes no
    /// images refuses a job that carries some, saying why; a message whose own text begins as
    /// the form does goes in the form to a handler that reads it, so it is read as text.
    async fn message_for(&self, job: &AssistantJob, handler: &str) -> Result<String, String> {
        let refusal = self.harness.images(handler);
        if job.images.is_empty() {
            return Ok(match (refusal, job.message.starts_with(CONTENT_PREFIX)) {
                (None, true) => MessageContent {
                    text: job.message.clone(),
                    images: Vec::new(),
                }
                .encode(),
                _ => job.message.clone(),
            });
        }
        if let Some(reason) = refusal {
            return Err(format!(
                "the message carries images, and the assistant's harness cannot be shown them: \
                 {reason}"
            ));
        }
        let most = (self.tables)().0.image_bytes.get().unwrap_or(u64::MAX);
        let (exports, conversation, images) = (
            self.exports.clone(),
            job.conversation_id.clone(),
            job.images.clone(),
        );
        let images = tokio::task::spawn_blocking(move || {
            use base64::Engine as _;
            images
                .iter()
                .map(|image| {
                    let bytes = exports.image(&conversation, &image.sha256, most)?;
                    if sha256_hex(&bytes) != image.sha256 {
                        return Err("an image did not arrive as it was sent".to_owned());
                    }
                    Ok(MessageImage {
                        media_type: image.media_type.clone(),
                        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    })
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|reason| format!("an image of the message could not be read: {reason}"))?;
        Ok(MessageContent {
            text: job.message.clone(),
            images,
        }
        .encode())
    }

    /// Makes a job's commit in a clone of its own. Answers where it landed.
    async fn commit(&self, job: &AssistantJob) -> Result<String, String> {
        let commit = job.commit.clone().ok_or("the job has no commit")?;
        let houses = self.houses.clone();
        let digest = conversation_digest(&job.conversation_id);
        let limits = workspace::CloneLimits::of(&(self.tables)().1);
        tokio::task::spawn_blocking(move || {
            workspace::commit_once(&houses, &commit, &digest, limits)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    /// Starts or resumes the conversation's harness with the job's message: on the next route with
    /// room when a route's harness does not start (its login gone, say), so one source's trouble
    /// does not stop her. Each route is tried once, none of `failed` at all. `lead` tells the
    /// person why the message is where it is; a `walk` that is stopped starts nothing more.
    async fn begin(
        self: &Arc<Self>,
        job: &AssistantJob,
        mut failed: Vec<(RouteRef, String)>,
        lead: Vec<String>,
        walk: Option<&relay::Walk>,
    ) -> Result<(), String> {
        loop {
            if walk.is_some_and(relay::Walk::stopped) {
                return Err("the person stopped the move to another route".to_owned());
            }
            let mut failing = None;
            match self.begin_on(job, &failed, &lead, &mut failing).await {
                Ok(()) => return Ok(()),
                Err(reason) => match failing {
                    Some(route) => {
                        (self.log)(&format!(
                            "assistant conversation {}: route {route} did not start: {reason}",
                            conversation_digest(&job.conversation_id)
                        ));
                        failed.push((route, reason));
                    }
                    None => return Err(reason),
                },
            }
        }
    }

    /// [`Desk::begin`] on one route, none of `failed`. A harness on a route that does not start
    /// names the route in `failing`.
    async fn begin_on(
        self: &Arc<Self>,
        job: &AssistantJob,
        failed: &[(RouteRef, String)],
        lead: &[String],
        failing: &mut Option<RouteRef>,
    ) -> Result<(), String> {
        let state = self.state.current();
        let agent = state
            .roster
            .agent(None, &job.agent_id)
            .map_err(|error| format!("the assistant Agent is not on this roster: {error}"))?;
        if medical::is_quarantined(&state, &agent.id, None) {
            return Err(format!("Agent {:?} is quarantined", agent.id));
        }
        let earlier = job
            .route
            .clone()
            .or_else(|| self.known_route(&job.conversation_id));
        let (agent, on, same_route) = self.route_for(&state, agent, earlier.as_ref(), failed)?;
        let handler = agent
            .handler
            .clone()
            .ok_or_else(|| format!("Agent {:?} has no handler", agent.id))?;
        if !self.harness.available(&handler) {
            return Err(format!("this Station has no {handler:?} handler"));
        }
        let handler_settings = agent.handler_settings.clone();
        let message = self.message_for(job, &handler).await?;
        let desk = (self.tables)().1;
        let retry = desk.remove_retry.0;
        // Conversations start at once, so one starting holds its place until it runs.
        let _starting = {
            let live = self.live.lock().map_err(|_| "assistant lock failed")?;
            if !desk
                .live
                .allows(live.len() + self.starting.load(Ordering::Acquire) + 1)
            {
                return Err("this Station runs as many assistant Sessions as it may".to_owned());
            }
            self.starting.fetch_add(1, Ordering::AcqRel);
            Starting(&self.starting)
        };
        let area = self.area(&job.conversation_id)?;
        if self.elevated() {
            (self.log)(
                "weakening: assistant.allow_elevated = true: Moneypenny's harness starts on \
                 this elevated Station",
            );
        }
        if (self.beside_missions)() {
            (self.log)(
                "weakening: assistant.allow_beside_local_missions = true: Moneypenny's harness \
                 starts on a Station that also runs Missions on its own host, whose commands can \
                 read her tokens",
            );
        }
        // A harness that cannot keep to the catalog gets none of it, rather than every skill
        // its own configuration finds.
        let no_skills = if job.skills.is_empty() {
            None
        } else {
            self.harness.skills(&handler)
        };
        let installed = match &no_skills {
            Some(reason) => {
                (self.log)(&format!(
                    "assistant conversation {}: skills are unavailable with handler \
                     {handler:?}: {reason}",
                    conversation_digest(&job.conversation_id)
                ));
                Ok(skills::Installed::default())
            }
            None => self.install_skills(job, &area).await,
        };
        let installed = match installed {
            Ok(installed) => installed,
            Err(reason) => {
                remove_area(&area, retry);
                return Err(reason);
            }
        };
        let (mut mode, workspace, mut notices) =
            match self.workspace(job, &area, &installed.names).await {
                Ok(prepared) => prepared,
                Err(reason) => {
                    remove_area(&area, retry);
                    return Err(reason);
                }
            };
        notices.extend(installed.notices);
        notices.extend(lead.iter().cloned());
        if let (Some(earlier), Some(on), false) = (&earlier, &on, same_route) {
            notices.push(format!(
                "Moneypenny moved from {earlier} to {}: she has this conversation's recap, not \
                 its full history.",
                on.route.reference()
            ));
        }
        if let Some(dir) = installed.dir {
            mode.skills = installed.names;
            mode.skills_dir = Some(dir.to_string_lossy().into_owned());
            mode.skill_commands = state.roster.assistant().skills_allow_tools();
        }
        let reach = match self.capped_reach(job) {
            Ok(reach) => reach,
            Err(reason) => {
                remove_area(&area, retry);
                return Err(reason);
            }
        };
        // As a Chief of Staff review, the harness keeps only what reaches its model.
        let mut environment = vetting::review_environment(&credentials::session_environment(
            &state, &agent.id, &agent.env,
        ));
        // What a route sets is how it reaches its model, whatever its name.
        if let Some(on) = &on {
            environment.extend(on.route.env.values().clone());
        }
        let session_key = session_name(&job.conversation_id);
        // Before her own directories below: an account's variables make way for no one else's.
        let site = Site {
            resuming: false,
            on_station: true,
        };
        if let Err(reason) = credentials::issue_for_assistant(
            &state,
            Holder {
                id: &session_key,
                operation: None,
            },
            &agent,
            on.as_ref(),
            site,
            &mut environment,
        ) {
            remove_area(&area, retry);
            return Err(reason);
        }
        let mut issued = Issued {
            state: state.clone(),
            session: session_key.clone(),
            running: false,
        };
        environment.insert(ASSISTANT_MCP_URL_ENV.to_owned(), self.mcp_url.clone());
        environment.insert(ASSISTANT_MCP_TOKEN_ENV.to_owned(), job.mcp_token.clone());
        // Her own Claude Code configuration directory, beside the working directory: the
        // Agent's (a shared login's `.credentials.json`, every person's transcripts) stays out
        // of the harness, and each conversation keeps its own. The sandbox denies reading it.
        if let Some(config) = config_dir(Path::new(&area)) {
            if let Err(error) = std::fs::create_dir_all(&config) {
                remove_area(&area, retry);
                return Err(format!(
                    "assistant configuration directory not made: {error}"
                ));
            }
            environment.insert(
                "CLAUDE_CONFIG_DIR".to_owned(),
                config.to_string_lossy().into_owned(),
            );
        }
        // Her own temporary directory, so her scripts' `$TMPDIR` is not the per-user one every
        // conversation of the Station's user shares. Claude Code 2.1.310 derives the sandbox's
        // `$TMPDIR` from the harness's own `TMPDIR` (`os.tmpdir()`), and keeps its own temp
        // files under `CLAUDE_CODE_TMPDIR`, so both point here.
        if let Some(temp) = temp_dir(Path::new(&area)) {
            if let Err(error) = std::fs::create_dir_all(&temp) {
                remove_area(&area, retry);
                return Err(format!("assistant temporary directory not made: {error}"));
            }
            let temp = temp.to_string_lossy().into_owned();
            environment.insert("TMPDIR".to_owned(), temp.clone());
            environment.insert("CLAUDE_CODE_TMPDIR".to_owned(), temp);
        }
        let flags = Arc::new(Flags {
            busy: AtomicBool::new(true),
            alive: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            interrupting: AtomicBool::new(false),
            last_active: Mutex::new(Instant::now()),
            handler: handler.clone(),
            turn: Mutex::new(Some(job.clone())),
            moving: AtomicBool::new(false),
        });
        #[cfg(feature = "cables-mcp")]
        let cables = match self
            .open_cables(job, &agent.id, &mut mode, &mut environment, &flags)
            .await
        {
            Ok(cables) => cables,
            Err(reason) => {
                remove_area(&area, retry);
                return Err(reason);
            }
        };
        let unavailable = self.arm(&mut mode, job, &reach, &handler, &environment);
        let mission_id = MissionId::try_new(session_key.clone())
            .map_err(|_| "the conversation cannot name a Session".to_owned())?;
        // A provider Session belongs to the source it ran on.
        let previous = same_route
            .then(|| {
                job.session_id
                    .clone()
                    .or_else(|| self.known_session(&job.conversation_id))
            })
            .flatten();
        let mut initial_context: Vec<String> = Some(agent.context.clone())
            .filter(|context| !context.trim().is_empty())
            .into_iter()
            .collect();
        initial_context.extend(job.context.iter().cloned());
        initial_context.extend(unavailable);
        initial_context.extend(no_skills.map(|reason| {
            format!(
                "Skills are unavailable in this conversation ({reason}), so you load none: say \
                 so when the person asks for one."
            )
        }));
        // A resumed harness has the conversation; only one started afresh is given its recap.
        let mut fresh_context = initial_context.clone();
        fresh_context.extend(job.recap.iter().cloned());
        let start = AgentStart {
            mission_id: mission_id.clone(),
            agent_id: agent.id.clone(),
            prompt: message.clone(),
            initial_context: fresh_context,
            working_directory: area.clone(),
            clearance: "desk".to_owned(),
            environment: environment.clone(),
            model: agent.model.clone(),
            effort: agent.effort.clone(),
            max_cost_micro_usd: None,
            project_settings: false,
            project_mcp: false,
            answer_only: false,
            assistant: Some(mode.clone()),
            overlays: Default::default(),
        };
        let resume = previous
            .and_then(|previous| SessionId::try_new(previous).ok())
            .map(|session_id| AgentResume {
                mission_id,
                session_id,
                message: Some(message.clone()),
                working_directory: area.clone(),
                clearance: "desk".to_owned(),
                environment,
                model: agent.model.clone(),
                effort: agent.effort.clone(),
                max_cost_micro_usd: None,
                project_settings: false,
                project_mcp: false,
                answer_only: false,
                assistant: Some(mode),
                initial_context,
                overlays: Default::default(),
            });
        let launched = Launched {
            harness: self.harness.clone(),
            handler: handler.clone(),
            settings: handler_settings.clone(),
            environment: start.environment.clone(),
        };
        let harness = self.harness.clone();
        let conversation = job.conversation_id.clone();
        let started = tokio::task::spawn_blocking(move || {
            let Some(resume) = resume else {
                return harness
                    .start(&handler, &handler_settings, start)
                    .map(|session| (session, None));
            };
            let resumed = resume.session_id.as_str().to_owned();
            match harness.resume(&handler, &handler_settings, resume) {
                // A resumed harness can still fail before it confirms the Session (Claude Code
                // reads the message first): the pump then starts afresh with `start`.
                Ok(session) => Ok((
                    session,
                    Some(Fallback {
                        harness,
                        handler,
                        settings: handler_settings,
                        start,
                        resumed,
                    }),
                )),
                // A provider Session that cannot be resumed (its transcript gone) starts afresh.
                Err(error) => {
                    report_resume_failure(&conversation, &resumed, &error);
                    harness
                        .start(&handler, &handler_settings, start)
                        .map(|session| (session, None))
                }
            }
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|started| started);
        let (session, fallback) = match started {
            Ok(started) => started,
            Err(error) => {
                remove_area(&area, retry);
                *failing = on.as_ref().map(|on| on.route.reference());
                return Err(format!("the assistant harness did not start: {error}"));
            }
        };
        let session = Arc::new(RwLock::new(session));
        self.live
            .lock()
            .map_err(|_| "assistant lock failed")?
            .insert(
                job.conversation_id.clone(),
                Live {
                    session: session.clone(),
                    token_expires_at: job.token_expires_at,
                    flags: flags.clone(),
                    started: self.started_under(job, &reach),
                    workspace: workspace.clone(),
                    #[cfg(feature = "cables-mcp")]
                    cables,
                },
            );
        let pump = Pump {
            #[cfg(feature = "cables-mcp")]
            left_held: self.left_held.clone(),
            state: state.clone(),
            session_name: session_key,
            route: on.map(|on| on.route.reference()),
            usage: UsageTracker::default(),
            unreported_tokens: std::cell::Cell::new(0),
            model: agent.model.clone(),
            conversation_id: job.conversation_id.clone(),
            station_id: state.station_id.clone(),
            session,
            flags,
            area,
            idle: desk.idle,
            remove_retry: retry,
            outbox: self.outbox.clone(),
            live: self.live.clone(),
            known: self.known.clone(),
            stopping: state.stopping.clone(),
            fallback,
            workspace,
            notices,
            exports: self.exports.clone(),
            desk: Arc::downgrade(self),
            runtime: tokio::runtime::Handle::current(),
            tried: failed.to_vec(),
            touched: false,
            moving: None,
            launched,
            tape: super::contexts::EventTape::new(state.intake.limits().contexts.context_bytes),
        };
        std::thread::Builder::new()
            .name("ai6-assistant".to_owned())
            .spawn(move || pump.run())
            .map_err(|error| format!("the assistant pump did not start: {error}"))?;
        issued.running = true;
        Ok(())
    }

    /// The route a job of the routed `agent` runs on, and `agent` as it works there; `agent` as it
    /// is for one handler. Whether it stays on `earlier`, the route the conversation last ran on,
    /// and so can resume its provider Session. A route counts when its handler can be started
    /// here and its source has room.
    ///
    /// # Errors
    ///
    /// Why no route can take the job, naming each: Universal Exports then offers it elsewhere.
    fn route_for(
        &self,
        state: &DaemonState,
        agent: Agent,
        earlier: Option<&RouteRef>,
        failed: &[(RouteRef, String)],
    ) -> Result<(Agent, Option<OnRoute>, bool), String> {
        if !agent.is_routed() {
            return Ok((agent, None, true));
        }
        let harness = self.harness.clone();
        let refuses = move |route: &crate::directorates::m::roster::AgentRoute| {
            let named = route.reference();
            if let Some((_, reason)) = failed.iter().find(|(tried, _)| *tried == named) {
                return Some(reason.clone());
            }
            (!harness.available(&route.handler))
                .then(|| format!("this Station has no {:?} handler", route.handler))
        };
        let place = routes::Place {
            refuses: &refuses,
            clearance: None,
            review: false,
        };
        let (on, same) = routes::for_conversation(state, &agent, earlier, &place)?;
        let agent = agent.on_route(&on.route);
        Ok((agent, Some(on), same))
    }

    /// What this Station's roster lets `job`'s person run and reach, the deployment capped by
    /// the person's own table and every group table the job names
    /// ([`AssistantSettings::reach_for`]); `Err` when a group table exists but the job carries
    /// no group list to apply it against (the gateway did not resolve the person's groups), so
    /// the Station refuses rather than skip a narrowing and widen. A job with no `person_id`
    /// takes the strictest reach: nothing runs, nothing is reached.
    ///
    /// [`AssistantSettings::reach_for`]: crate::directorates::m::assistant::AssistantSettings::reach_for
    fn capped_reach(&self, job: &AssistantJob) -> Result<AssistantReach, String> {
        let settings = self.state.current().roster.assistant().clone();
        let job_reach = job.reach.capped_by(&settings.deployment_reach());
        let Some(person) = job.person_id.as_deref() else {
            // No person: the strictest reach, whatever the roster allows anyone.
            return Ok(job_reach.capped_by(&AssistantReach::default()));
        };
        Ok(job_reach.capped_by(&settings.reach_for(person, job_groups(job, &settings)?)))
    }

    /// A digest of what a harness for `job` starts under, past its token's expiry: the tool
    /// token it is handed, `reach`, the repositories of its workspace, its skills with the path
    /// and pin this Station's catalog gives each, and the catalog settings that decide how they
    /// load (`skills_allow_tools`, `skills_unpinned`). A running harness started under another
    /// is restarted, so a narrowed or withdrawn permission never outlives its message.
    fn started_under(&self, job: &AssistantJob, reach: &AssistantReach) -> String {
        let state = self.state.current();
        let settings = state.roster.assistant();
        let skills: Vec<_> = job
            .skills
            .iter()
            .map(|name| {
                let entry = settings.skills().get(name);
                (
                    name,
                    entry.map(|skill| skill.path().to_owned()),
                    entry
                        .and_then(|skill| skill.pin())
                        .map(|pin| pin.to_string()),
                )
            })
            .collect();
        let under = serde_json::json!({
            "token": workspace::sha256_hex(job.mcp_token.as_bytes()),
            "reach": reach,
            "workspace": job.workspace.as_ref().map(|workspace| (&workspace.personal, &workspace.shared)),
            "skills": skills,
            "skills_allow_tools": settings.skills_allow_tools(),
            "skills_unpinned": settings.skills_unpinned(),
            "routes": state.roster.agent(None, &job.agent_id).ok().map(|agent| {
                agent
                    .route_list()
                    .iter()
                    .map(|route| route.reference().to_string())
                    .collect::<Vec<_>>()
            }),
        });
        workspace::sha256_hex(under.to_string().as_bytes())
    }

    /// Gives `mode` what `reach` lets her run and reach: the network policy, and the
    /// interpreters when `handler`'s harness can sandbox them here, or without a sandbox when
    /// `reach.unsandboxed_scripts` allows it (a weakening logged, and loud: every page she
    /// fetches and every person she serves effectively gets a shell as the Station user).
    /// Answers the line that tells her scripts are unavailable, when they are, which the
    /// Station's log also gets with the reason.
    fn arm(
        &self,
        mode: &mut AssistantMode,
        job: &AssistantJob,
        reach: &AssistantReach,
        handler: &str,
        environment: &BTreeMap<String, String>,
    ) -> Option<String> {
        let digest = conversation_digest(&job.conversation_id);
        if reach.network.open() {
            (self.log)(
                "weakening: assistant.network = \"open\": Moneypenny's web tools and scripts \
                 reach every host the Station reaches, the Station's own machine and its network \
                 included, but what assistant.denied_networks names",
            );
        }
        mode.network = reach.network.clone();
        if reach.interpreters.is_empty() {
            return None;
        }
        // A shell interpreter runs anything in the sandbox; name it so the operator sees it.
        if let Some(shell) = reach
            .interpreters
            .iter()
            .find(|name| matches!(name.as_str(), "bash" | "sh" | "dash" | "zsh" | "env"))
        {
            (self.log)(&format!(
                "weakening: assistant.interpreters includes {shell:?}: a shell runs any program \
                 the sandbox allows, not only {shell:?}"
            ));
        }
        if let Some(reason) = self.harness.scripts(handler, environment) {
            if !reach.unsandboxed_scripts {
                (self.log)(&format!(
                    "assistant conversation {digest}: scripts are unavailable on this Station: \
                     {reason}"
                ));
                return Some(format!(
                    "Scripts are unavailable on this Station ({reason}), so you have no shell \
                     and run nothing: say so when the person asks for a script."
                ));
            }
            (self.log)(
                "weakening: assistant scripts run unsandboxed: no file or network limit is \
                 enforced for them, so every page she fetches and every person she serves \
                 effectively gets a shell as the Station's user, able to read its credentials, \
                 other conversations and the Station",
            );
            mode.unsandboxed = true;
        }
        mode.interpreters = reach.interpreters.clone();
        if !mode.files.is_empty() {
            mode.read_only = vec![
                workspace::SHARED.to_owned(),
                "CLAUDE.md".to_owned(),
                "AGENTS.md".to_owned(),
            ];
        }
        None
    }

    /// With Cable grants, serves the conversation its Cable endpoint, as a further server of
    /// `mode` reached through `environment`; `None` without grants.
    #[cfg(feature = "cables-mcp")]
    async fn open_cables(
        &self,
        job: &AssistantJob,
        agent: &str,
        mode: &mut AssistantMode,
        environment: &mut BTreeMap<String, String>,
        flags: &Arc<Flags>,
    ) -> Result<Option<Arc<AssistantEndpoint>>, String> {
        let Some(person) = job.person_id.clone().filter(|_| !job.cables.is_empty()) else {
            return Ok(None);
        };
        let (outbox, station, conversation) = (
            self.outbox.clone(),
            self.state.station_id.clone(),
            job.conversation_id.clone(),
        );
        let reporter = Arc::new(move |event: AssistantEvent| {
            outbox.send(AssistantReport {
                route: None,
                run: None,
                station_id: station.clone(),
                conversation_id: conversation.clone(),
                job_id: None,
                session_id: None,
                events: vec![event],
            });
        });
        let endpoint = AssistantEndpoint::start(
            self.state.cables.clone(),
            self.state.intake.clone(),
            AssistantIdentity {
                agent: agent.to_owned(),
                station: self.state.station_id.clone(),
                conversation: job.conversation_id.clone(),
                session: format!("assistant-{}", conversation_digest(&job.conversation_id)),
                person,
            },
            job.cables.clone(),
            &(self.tables)().1,
            reporter,
            Arc::new(ExportsGate(self.exports.clone())),
            // A job with a new token starts the harness again, and this with it; a turn that
            // outlasts the job's token keeps it until the turn ends or the person stops it.
            job.token_expires_at,
            {
                let flags = flags.clone();
                Arc::new(move || {
                    flags.busy.load(Ordering::Acquire)
                        && !flags.interrupting.load(Ordering::Acquire)
                })
            },
        )
        .await
        .map_err(|error| format!("the conversation's Cable endpoint did not start: {error}"))?;
        endpoint.add_environment(environment);
        *mode = std::mem::replace(mode, AssistantMode::moneypenny()).with_cables();
        if let Some(earlier) = self
            .left_held
            .lock()
            .ok()
            .and_then(|mut left| left.remove(&job.conversation_id))
        {
            endpoint.adopt_held(&earlier);
        }
        Ok(Some(Arc::new(endpoint)))
    }

    #[cfg(feature = "cables-mcp")]
    fn cables_of(&self, conversation: &str) -> Option<Arc<AssistantEndpoint>> {
        let running = self
            .live
            .lock()
            .ok()?
            .get(conversation)
            .and_then(|live| live.cables.clone());
        running.or_else(|| self.left_held.lock().ok()?.get(conversation).cloned())
    }

    /// Forgets the endpoints with nothing left to decide; each is otherwise kept for the Cables'
    /// time limit, however long no job comes.
    #[cfg(feature = "cables-mcp")]
    fn sweep_left_held(&self) {
        if let Ok(mut left) = self.left_held.lock() {
            left.retain(|_, endpoint| endpoint.holds_cables());
        }
    }

    /// Sends or drops the Cable the conversation's endpoint holds for the person; a confirm
    /// that sends nothing is refused with the reason.
    async fn decide_cable(&self, job: &AssistantJob) -> Result<(), String> {
        #[cfg(feature = "cables-mcp")]
        {
            let cables = self.cables_of(&job.conversation_id);
            if let Some(decision) = job.cable_discard.clone() {
                if let Some(cables) = cables {
                    cables.discard(&decision.id);
                }
                self.sweep_left_held();
                return Ok(());
            }
            let (Some(cables), Some(decision)) = (cables, job.cable_confirm.clone()) else {
                return Err(
                    "nothing is held here: the Cable was sent, dropped or expired, or the \
                     Station restarted, which drops what it holds; nothing was sent"
                        .to_owned(),
                );
            };
            // Universal Exports gives up on a confirmation that waited this long.
            if job.token_expires_at <= unix_now() {
                return Err("the confirmation came too late; nothing was sent".to_owned());
            }
            let sent = tokio::task::spawn_blocking(move || cables.confirm(&decision))
                .await
                .map_err(|error| error.to_string())?
                .map(|_| ());
            self.sweep_left_held();
            sent
        }
        #[cfg(not(feature = "cables-mcp"))]
        {
            if job.cable_discard.is_some() {
                return Ok(());
            }
            Err("this Station serves no Cables".to_owned())
        }
    }

    fn running(&self, conversation: &str) -> Option<Running> {
        let live = self.live.lock().ok()?;
        let entry = live.get(conversation)?;
        entry.flags.alive.load(Ordering::Acquire).then(|| {
            (
                entry.session.clone(),
                entry.flags.clone(),
                entry.token_expires_at,
                entry.started.clone(),
                entry.workspace.clone(),
            )
        })
    }

    /// Installs the skills `job` gives her in `area` ([`skills::install`]).
    async fn install_skills(
        &self,
        job: &AssistantJob,
        area: &str,
    ) -> Result<skills::Installed, String> {
        if job.skills.is_empty() {
            return Ok(skills::Installed::default());
        }
        let (state, log, area, wanted) = (
            self.state.clone(),
            self.log.clone(),
            PathBuf::from(area),
            job.skills.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let current = state.current();
            skills::install(&state, &log, &area, current.roster.assistant(), &wanted)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|reason| format!("the assistant's skills could not be installed: {reason}"))
    }

    /// The mode a job's harness starts in, and its workspace with the notices preparing it
    /// left: Moneypenny's own tools only without one; with one, the repositories cloned into
    /// The Station's own directories, which an assistant's interpreters may never read.
    fn station_paths(&self) -> Vec<String> {
        self.state
            .station
            .protected_paths()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }

    /// `area` and the file tools for `personal/` and `scratch/`, its standing text naming
    /// `skills`, those installed.
    async fn workspace(
        &self,
        job: &AssistantJob,
        area: &str,
        skills: &[String],
    ) -> Result<(AssistantMode, Option<SharedWorkspace>, Vec<String>), String> {
        let Some(wanted) = job.workspace.clone() else {
            let mode = AssistantMode {
                station_paths: self.station_paths(),
                ..AssistantMode::moneypenny()
            };
            return Ok((mode, None, Vec::new()));
        };
        let area = PathBuf::from(area);
        let (context, repos) = context_file(&area)
            .zip(repos_dir(&area))
            .ok_or("the conversation's directory has no parent")?;
        let houses = self.houses.clone();
        let conversation = job.conversation_id.clone();
        let file = context.clone();
        let skills = skills.to_vec();
        let (bounds, limits) = (self.tables)();
        let (workspace, notices) = tokio::task::spawn_blocking(move || {
            Workspace::prepare(
                &houses,
                &area,
                &repos,
                &file,
                &conversation,
                &wanted,
                &skills,
                &limits,
                &bounds,
            )
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|reason| format!("the assistant's workspace could not be prepared: {reason}"))?;
        for notice in &notices {
            (self.log)(&format!(
                "assistant conversation {}: {notice}",
                conversation_digest(&job.conversation_id)
            ));
        }
        let mode = AssistantMode {
            files: vec![PERSONAL.to_owned(), SCRATCH.to_owned()],
            context_file: Some(context.to_string_lossy().into_owned()),
            station_paths: self.station_paths(),
            ..AssistantMode::moneypenny()
        };
        Ok((mode, Some(Arc::new(Mutex::new(workspace))), notices))
    }

    /// Stops the answer the conversation's harness is giving, through its handler's
    /// interrupt; its pump reports `interrupted` when the turn ends. A harness that is not
    /// answering, or not running here, is left as it is.
    async fn interrupt(&self, conversation: &str) -> Result<(), String> {
        // A message moving to another route is stopped where it is; the walk owes the Stop to
        // the harness it starts.
        if self.moves.interrupt(conversation) {
            return Ok(());
        }
        let Some((session, flags, _, _, _)) = self.running(conversation) else {
            return Ok(());
        };
        if !flags.busy.load(Ordering::Acquire) {
            return Ok(());
        }
        flags.interrupting.store(true, Ordering::Release);
        let interrupted = tokio::task::spawn_blocking(move || {
            session
                .read()
                .map_err(|_| "assistant Session lock failed".to_owned())?
                .interrupt()
                .map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|interrupted| interrupted);
        if interrupted.is_err() {
            flags.interrupting.store(false, Ordering::Release);
        }
        interrupted.map_err(|error| format!("the harness could not be interrupted: {error}"))
    }

    /// Asks the conversation's harness to close; its pump reports `closed`.
    fn close(&self, conversation: &str) {
        self.moves.close(conversation);
        if let Ok(live) = self.live.lock()
            && let Some(entry) = live.get(conversation)
        {
            entry.flags.stop.store(true, Ordering::Release);
        }
    }

    /// Records `job` as the message of `flags`' turn and tells whether that turn was already being
    /// answered, in which case it is not sent again at a limit; `None` while the turn's message
    /// moves to another route.
    fn join_turn(flags: &Flags, job: &AssistantJob) -> Result<Option<bool>, String> {
        let mut turn = flags
            .turn
            .lock()
            .map_err(|_| "assistant lock failed".to_owned())?;
        if flags.moving.load(Ordering::Acquire) {
            return Ok(None);
        }
        flags.interrupting.store(false, Ordering::Release);
        let answering = flags.busy.swap(true, Ordering::AcqRel);
        *turn = (!answering).then(|| job.clone());
        Ok(Some(answering))
    }

    /// Waits while `conversation`'s message moves to another route, which ends when a route
    /// took it or none could.
    async fn wait_moved(&self, conversation: &str) {
        while self.moves.under_way(conversation) && !self.state.stopping.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Waits up to `wait` for `conversation`'s harness to have closed.
    async fn wait_closed(&self, conversation: &str, wait: Duration) {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline
            && self
                .live
                .lock()
                .is_ok_and(|live| live.contains_key(conversation))
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Closes every harness, for a stopping Station.
    fn close_all(&self) {
        if let Ok(live) = self.live.lock() {
            for entry in live.values() {
                entry.flags.stop.store(true, Ordering::Release);
            }
        }
    }

    fn known_session(&self, conversation: &str) -> Option<String> {
        self.known
            .lock()
            .ok()?
            .get(conversation)
            .map(|(session, _)| session.clone())
    }

    fn known_route(&self, conversation: &str) -> Option<RouteRef> {
        self.known
            .lock()
            .ok()?
            .get(conversation)
            .and_then(|(_, route)| route.clone())
    }

    /// The conversation's working directory, made empty-if-new; the same path on every start.
    fn area(&self, conversation: &str) -> Result<String, String> {
        let mut areas = self
            .areas
            .lock()
            .map_err(|_| "assistant lock failed".to_owned())?;
        let root = match &*areas {
            Some(root) => root.path.clone(),
            None => areas.insert(new_root()?).path.clone(),
        };
        let area = root.join(conversation_digest(conversation));
        std::fs::create_dir_all(&area)
            .map_err(|error| format!("assistant working directory not made: {error}"))?;
        Ok(area.to_string_lossy().into_owned())
    }
}

/// A harness being started, counted against `[assistant.desk] live` until it is dropped.
struct Starting<'a>(&'a AtomicUsize);

impl Drop for Starting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Reads one harness's events until it ends, idles out or is closed.
struct Pump {
    #[cfg(feature = "cables-mcp")]
    left_held: LeftHeld,
    state: DaemonState,
    /// What its account claim and leases go under.
    session_name: String,
    /// The route it runs on, when its Agent has routes.
    route: Option<RouteRef>,
    /// What the running harness has reported so far.
    usage: UsageTracker,
    /// Tokens reported since the last report to Universal Exports.
    unreported_tokens: std::cell::Cell<u64>,
    /// The model the harness runs, as its route or Agent names it.
    model: Option<String>,
    conversation_id: String,
    station_id: String,
    session: SharedSession,
    flags: Arc<Flags>,
    area: String,
    /// How long it may wait for a message, or write nothing while answering.
    idle: TimeLimit,
    /// How long removing its directory is tried again.
    remove_retry: Duration,
    outbox: Outbox,
    live: Arc<Mutex<BTreeMap<String, Live>>>,
    known: Known,
    stopping: Arc<AtomicBool>,
    /// For a resumed harness, how to start afresh should it fail before confirming the
    /// provider Session.
    fallback: Option<Fallback>,
    /// The conversation's workspace, whose turn ends with each `completed` and at close.
    workspace: Option<SharedWorkspace>,
    /// What preparing the workspace left to tell, sent with the first event.
    notices: Vec<String>,
    /// Where the pages she publishes are uploaded.
    exports: Arc<dyn Exports>,
    /// The desk, for a limit that moves the turn's message to another route.
    desk: std::sync::Weak<Desk>,
    runtime: tokio::runtime::Handle,
    /// The routes already refused or limited for the message that started this harness.
    tried: Vec<(RouteRef, String)>,
    /// The turn under way has shown the person text or called a tool.
    touched: bool,
    /// Set when a limit sends the turn's message to another route.
    moving: Option<relay::Step>,
    /// How the harness was launched, for its handler to say where it keeps the Session.
    launched: Launched,
    /// The events it reported, the context of a harness that keeps no record of its own.
    tape: super::contexts::EventTape,
}

/// The handler, its settings and the environment a conversation's harness started with.
struct Launched {
    harness: Arc<dyn Harness>,
    handler: String,
    settings: HandlerSettings,
    environment: BTreeMap<String, String>,
}

/// A fresh start for a conversation whose harness was resumed.
struct Fallback {
    harness: Arc<dyn Harness>,
    handler: String,
    settings: HandlerSettings,
    start: AgentStart,
    /// The provider Session the harness was asked to resume.
    resumed: String,
}

impl Pump {
    fn run(mut self) {
        let mut session_id = None;
        let mut heard = Instant::now();
        let reason = loop {
            if self.flags.stop.load(Ordering::Acquire) {
                break None;
            }
            if self.stopping.load(Ordering::Acquire) {
                break Some("the Station is stopping".to_owned());
            }
            let read = match self.session.read() {
                Ok(session) => {
                    let reported = if session_id.is_none() {
                        checked_session_id(session.session_id().ok().flatten())
                    } else {
                        Ok(None)
                    };
                    match reported {
                        Ok(reported) => {
                            if let Some(id) = reported {
                                self.remember(id.as_str());
                                session_id = Some(id);
                            }
                            session.next_event(READ_SLICE_MS)
                        }
                        // A Session whose ID cannot be recorded could not be resumed: it fails.
                        Err(why) => Ok(Some(AgentEvent::new("failed", Some(why)))),
                    }
                }
                Err(_) => break Some("assistant Session lock failed".to_owned()),
            };
            // A resumed harness that fails before confirming its Session never had it.
            let failure = match &read {
                Ok(Some(event)) if event.kind == "failed" => {
                    Some(event.text.clone().unwrap_or_default())
                }
                Err(error) => Some(error.to_string()),
                _ => None,
            };
            if let Some(failure) = failure
                && session_id.is_none()
                && let Some(fallback) = self.fallback.take()
            {
                match self.start_afresh(fallback, &failure) {
                    Ok(()) => {
                        heard = Instant::now();
                        continue;
                    }
                    Err(error) => {
                        self.send(None, vec![AssistantEvent::new(EVENT_FAILED, Some(error))]);
                        break Some("the harness failed".to_owned());
                    }
                }
            }
            match read {
                Ok(Some(event)) => {
                    heard = Instant::now();
                    self.tape.record(&event);
                    let limited = self.count(&event);
                    self.touched |= shows_the_person(&event);
                    let prepared = self.notices.len();
                    let mut shared = Vec::new();
                    if !event.tools.is_empty() {
                        shared = self.shared_bases(&event);
                        self.publish_pages(&event);
                    }
                    let ended = matches!(event.kind.as_str(), "failed" | LIMIT_EVENT);
                    // An agent that ends a turn itself sends no `completed`, and the person
                    // would wait for one until the harness is closed for silence.
                    let cancelled = event.kind == CANCELLED_EVENT
                        || (event.kind == "blocked"
                            && event.text.as_deref() == Some(TURN_CANCELLED));
                    let mut stopped = false;
                    let mut events: Vec<AssistantEvent> = std::mem::take(&mut self.notices)
                        .into_iter()
                        .chain(limited)
                        .map(|notice| AssistantEvent::new(EVENT_NOTICE, Some(notice)))
                        .collect();
                    // The Antigravity CLI ends a turn at a refused tool, and says success: without
                    // this the person sees an empty answer, and nobody sees why.
                    if event.kind == DENIED_EVENT {
                        let tools = event.text.as_deref().unwrap_or("a tool");
                        eprintln!(
                            "ai6: assistant conversation {}: its harness refused calls of {tools}",
                            conversation_digest(&self.conversation_id)
                        );
                        events.push(AssistantEvent::new(
                            EVENT_NOTICE,
                            Some(format!(
                                "Moneypenny's agent was refused {tools}, which may have ended \
                                 this answer early."
                            )),
                        ));
                    }
                    if event.kind == "completed" || cancelled {
                        // The work is saved before the turn is reported over, so the next
                        // message's turn starts from it.
                        events.extend(self.end_turn(false));
                        self.file_context(session_id.as_ref());
                        self.touched = false;
                        self.flags.busy.store(false, Ordering::Release);
                        if let Ok(mut last) = self.flags.last_active.lock() {
                            *last = Instant::now();
                        }
                        stopped = self.flags.interrupting.swap(false, Ordering::AcqRel);
                    }
                    let shown = if stopped {
                        Some(AssistantEvent::new(EVENT_INTERRUPTED, None))
                    } else if cancelled {
                        events.push(AssistantEvent::new(
                            EVENT_NOTICE,
                            Some(
                                "Moneypenny's agent ended this answer early, as some agents do \
                                 when a tool they asked for is refused."
                                    .to_owned(),
                            ),
                        ));
                        Some(AssistantEvent::new(EVENT_COMPLETED, None))
                    } else {
                        translate(&event)
                    };
                    events.extend(shown.map(|mut shown| {
                        if shown.kind == EVENT_TOOL {
                            shown.shared = shared;
                        }
                        shown
                    }));
                    if event.kind == LIMIT_EVENT
                        && let Some(mut step) = self.plan_move(session_id.as_ref())
                    {
                        // Told only if no route can take the message.
                        step.held = events.split_off(prepared.min(events.len()));
                        self.moving = Some(step);
                    }
                    if !events.is_empty() {
                        self.send(session_id.as_ref(), events);
                    }
                    if ended {
                        break Some("the harness stopped".to_owned());
                    }
                }
                Ok(None) => {
                    let idle = self.idle.get();
                    let past = |since: Instant| idle.is_some_and(|idle| since.elapsed() >= idle);
                    let quiet = self.flags.last_active.lock().is_ok_and(|last| past(*last));
                    if !self.flags.busy.load(Ordering::Acquire) {
                        if quiet {
                            break Some("idle".to_owned());
                        }
                    } else if quiet && past(heard) {
                        // A harness that hangs while answering would hold its slot for good.
                        let reason = format!(
                            "the assistant harness wrote nothing for {} while answering; it \
                             was closed",
                            self.idle
                        );
                        self.send(
                            session_id.as_ref(),
                            vec![AssistantEvent::new(EVENT_FAILED, Some(reason))],
                        );
                        break Some("the harness stopped answering".to_owned());
                    }
                }
                Err(error) => {
                    self.send(
                        session_id.as_ref(),
                        vec![AssistantEvent::new(EVENT_FAILED, Some(error.to_string()))],
                    );
                    break Some("the harness failed".to_owned());
                }
            }
        };
        self.flags.alive.store(false, Ordering::Release);
        if let Ok(session) = self.session.read() {
            let _ = session.close();
        }
        // Before it leaves `live`: the next harness of this conversation claims under this name.
        credentials::release(&self.state, &self.session_name);
        // What the closed harness left is saved before its directory goes, and later commits
        // go to clones of their own.
        let saved = self.end_turn(true);
        if !saved.is_empty() {
            self.send(session_id.as_ref(), saved);
        }
        // Before its directory goes: the harness may keep its record there.
        self.file_context(session_id.as_ref());
        remove_area(&self.area, self.remove_retry);
        // Before it leaves `live`: a message waiting for this harness to go then finds this report.
        // A conversation whose message moves on is not closed.
        if self.moving.is_none() {
            self.send(
                session_id.as_ref(),
                vec![AssistantEvent::new(EVENT_CLOSED, reason)],
            );
        }
        if let Ok(mut live) = self.live.lock()
            && live
                .get(&self.conversation_id)
                .is_some_and(|entry| Arc::ptr_eq(&entry.flags, &self.flags))
        {
            let _closed = live.remove(&self.conversation_id);
            #[cfg(feature = "cables-mcp")]
            if let Some(cables) = _closed.and_then(|closed| closed.cables)
                && cables.holds_cables()
            {
                cables.stop_serving();
                if let Ok(mut left) = self.left_held.lock() {
                    left.insert(self.conversation_id.clone(), cables);
                }
            }
        }
        self.outbox.closed(&self.conversation_id);
        if let Some(step) = self.moving.take() {
            step.spawn();
        }
    }

    /// Takes the turn's message from the harness that met a limit to send it to the next route,
    /// when the turn showed the person nothing, called no tool, and no Stop or close is asked:
    /// then no tool acted and no part of an answer was seen. `None` leaves the limit as it is.
    fn plan_move(&self, session_id: Option<&SessionId>) -> Option<relay::Step> {
        let route = self.route.clone()?;
        if self.touched
            || self.flags.stop.load(Ordering::Acquire)
            || self.flags.interrupting.load(Ordering::Acquire)
            || self.stopping.load(Ordering::Acquire)
        {
            return None;
        }
        let desk = self.desk.upgrade()?;
        let walk = desk.moves.enter(&self.conversation_id);
        if walk.stopped() {
            return None;
        }
        // Taken with the flag set, so a message either joined the turn first (and nothing is
        // sent again) or waits for the move.
        let job = {
            let mut turn = self.flags.turn.lock().ok()?;
            let job = turn.take()?;
            self.flags.moving.store(true, Ordering::Release);
            job
        };
        Some(relay::Step {
            desk,
            runtime: self.runtime.clone(),
            job,
            failed: self.tried.clone(),
            route,
            held: Vec::new(),
            session_id: session_id.map(|id| id.as_str().to_owned()),
            walk,
        })
    }

    /// Counts what `event` reports against the account the harness holds, and at a provider's
    /// limit marks it, so another route is taken. What the person should know of that when the
    /// turn ends there, for a routed Agent.
    fn count(&mut self, event: &AgentEvent) -> Option<String> {
        accounts::observe_quota(&self.state, &self.session_name, &event.quota);
        if let Some(reported) = &event.usage {
            let delta = self.usage.observe(reported);
            self.unreported_tokens
                .set(self.unreported_tokens.get().saturating_add(delta.tokens()));
            accounts::record_usage(&self.state, &self.session_name, delta.tokens());
        }
        if event.kind != LIMIT_EVENT {
            return None;
        }
        let holder = Holder {
            id: &self.session_name,
            operation: None,
        };
        let fallback = self.outbox.desk().limit_fallback.0.as_secs();
        let resets_at = event.limit.as_ref().and_then(|limit| limit.resets_at);
        let (account, _, until) = accounts::mark_limited(&self.state, holder, resets_at, fallback)?;
        eprintln!(
            "ai6: assistant conversation {}: account {account} is limited until {until} (Unix \
             seconds): {}",
            conversation_digest(&self.conversation_id),
            event.text.as_deref().unwrap_or_default()
        );
        let route = self.route.as_ref()?;
        Some(format!(
            "Moneypenny's route {route} is out of quota: your next message takes another route if \
             one has room."
        ))
    }

    /// Replaces a resumed harness that failed with `failure` by one started afresh (with the
    /// conversation's recap), after closing it. The failure goes to the Station's log; `Err`
    /// says why neither runs, for the conversation.
    fn start_afresh(&mut self, fallback: Fallback, failure: &str) -> Result<(), String> {
        report_resume_failure(&self.conversation_id, &fallback.resumed, failure);
        let mut session = self
            .session
            .write()
            .map_err(|_| "assistant Session lock failed".to_owned())?;
        let _ = session.close();
        match fallback
            .harness
            .start(&fallback.handler, &fallback.settings, fallback.start)
        {
            Ok(started) => {
                *session = started;
                self.usage = UsageTracker::default();
                Ok(())
            }
            Err(error) => Err(format!(
                "resuming the conversation's Session failed ({failure}), and starting afresh \
                 failed too: {error}"
            )),
        }
    }

    /// Files the context of provider Session `session_id` as it stands: the harness's record
    /// where its handler names one, else the events this harness reported.
    fn file_context(&self, session_id: Option<&SessionId>) {
        let Some(session_id) = session_id else {
            return;
        };
        let launched = &self.launched;
        let record = launched.harness.record(
            &launched.handler,
            &launched.settings,
            SessionRecordQuery {
                session_id: session_id.as_str().to_owned(),
                working_directory: self.area.clone(),
                environment: launched.environment.clone(),
            },
        );
        super::contexts::file(
            &self.state,
            ContextOwner::Conversation {
                conversation: self.conversation_id.clone(),
            },
            session_id.as_str(),
            &launched.handler,
            record,
            &self.tape,
        );
    }

    /// Ends the workspace's turn, and with `closing` the workspace: the events saying what should
    /// be known.
    fn end_turn(&self, closing: bool) -> Vec<AssistantEvent> {
        let Some(workspace) = &self.workspace else {
            return Vec::new();
        };
        let Ok(mut workspace) = workspace.lock() else {
            return vec![AssistantEvent::new(
                EVENT_NOTICE,
                Some("Moneypenny's workspace could not be saved: its lock failed.".to_owned()),
            )];
        };
        let notices = if closing {
            workspace.close()
        } else {
            workspace.end_turn()
        };
        notices
            .into_iter()
            .map(|notice| AssistantEvent::new(EVENT_NOTICE, Some(notice)))
            .collect()
    }

    /// Uploads the files of each `publish_page` call of `event` that names a `path`, to
    /// Universal Exports, which holds the page; a call that cannot be read is refused there
    /// with the reason. The upload runs on its own thread: reading and sending a page does not
    /// hold up the harness's events.
    fn publish_pages(&self, event: &AgentEvent) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        for path in Workspace::page_calls(event.data.as_deref()) {
            let (workspace, exports) = (workspace.clone(), self.exports.clone());
            let conversation = self.conversation_id.clone();
            let digest = page_call_digest(&path);
            let spawned = std::thread::Builder::new()
                .name("ai6-assistant-page".to_owned())
                .spawn(move || {
                    let files = match workspace.lock() {
                        Ok(workspace) => workspace.page_files(&path),
                        Err(_) => Err("the workspace is not available".to_owned()),
                    };
                    let manifest = files.and_then(|files| {
                        let mut manifest = Vec::new();
                        for (file, bytes) in files {
                            exports.upload_page_file(&conversation, &digest, &file.path, bytes)?;
                            manifest.push(file);
                        }
                        Ok(manifest)
                    });
                    if let Err(reason) = exports.finish_page(&conversation, &digest, manifest) {
                        eprintln!(
                            "ai6: assistant conversation {}: a page was not published: {reason}",
                            conversation_digest(&conversation)
                        );
                    }
                });
            if let Err(error) = spawned {
                eprintln!("ai6: assistant page upload could not start: {error}");
            }
        }
    }

    /// What each shared change `event` calls was made from, for its [`EVENT_TOOL`] event to
    /// carry; none without a workspace.
    fn shared_bases(&self, event: &AgentEvent) -> Vec<AssistantSharedBase> {
        self.workspace
            .as_ref()
            .and_then(|workspace| workspace.lock().ok())
            .map(|workspace| workspace.shared_bases(event.data.as_deref()))
            .unwrap_or_default()
    }

    fn remember(&self, session_id: &str) {
        if let Ok(mut known) = self.known.lock() {
            let room = self.outbox.desk().known;
            if !known.contains_key(&self.conversation_id) {
                // Forget arbitrary ones; a forgotten conversation starts afresh.
                while !room.allows(known.len() + 1)
                    && let Some(first) = known.keys().next().cloned()
                {
                    known.remove(&first);
                }
                if !room.allows(known.len() + 1) {
                    return;
                }
            }
            known.insert(
                self.conversation_id.clone(),
                (session_id.to_owned(), self.route.clone()),
            );
        }
    }

    fn send(&self, session_id: Option<&SessionId>, events: Vec<AssistantEvent>) {
        let run = AssistantRun {
            account: accounts::held(&self.state, &self.session_name),
            model: self.model.clone(),
            tokens: Some(self.unreported_tokens.take()).filter(|tokens| *tokens > 0),
        };
        self.outbox.send(AssistantReport {
            route: session_id.and(self.route.clone()),
            run: (run != AssistantRun::default()).then_some(run),
            station_id: self.station_id.clone(),
            conversation_id: self.conversation_id.clone(),
            job_id: None,
            session_id: session_id.map(|id| id.as_str().to_owned()),
            events,
        });
    }
}

/// Whether `event` shows the person part of an answer or calls a tool, which may have acted.
fn shows_the_person(event: &AgentEvent) -> bool {
    match event.kind.as_str() {
        "text" => event
            .text
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty()),
        "tool" | "blocked" => true,
        _ => !event.tools.is_empty(),
    }
}

/// The provider-neutral event for a handler's event, or `None` for one the conversation
/// does not show (a harness's own bookkeeping).
fn translate(event: &AgentEvent) -> Option<AssistantEvent> {
    let kind = match event.kind.as_str() {
        "text" => EVENT_TEXT,
        "tool" => EVENT_TOOL,
        "completed" => EVENT_COMPLETED,
        "failed" => EVENT_FAILED,
        LIMIT_EVENT => EVENT_LIMIT,
        _ => return None,
    };
    let text = event.text.clone().filter(|text| !text.trim().is_empty());
    if kind == EVENT_TEXT && text.is_none() {
        return None;
    }
    let mut translated = AssistantEvent::new(kind, text);
    translated.tools = event.tools.clone();
    Some(translated)
}

/// Logs that the conversation's provider Session `resumed` could not be resumed and that its
/// harness starts afresh with the conversation's recap.
fn report_resume_failure(conversation: &str, resumed: &str, error: &str) {
    eprintln!(
        "ai6: assistant conversation {}: resuming provider Session {resumed} failed, so it \
         starts afresh with its recap: {error}",
        conversation_digest(conversation)
    );
}

/// The directory groups `job`'s person is capped by under `settings`: the job's own list, or
/// none when the roster has no group table; `Err` when it has one and the job carries no list
/// (the gateway did not resolve the person's groups), so the Station refuses rather than skip
/// a narrowing and widen.
fn job_groups<'a>(
    job: &'a AssistantJob,
    settings: &AssistantSettings,
) -> Result<&'a [String], String> {
    match (&job.groups, settings.has_group_narrowing()) {
        (Some(groups), _) => Ok(groups.as_slice()),
        (None, false) => Ok(&[]),
        (None, true) => Err(
            "this Station's roster narrows scripts, network and skills by directory group, but \
             the job carries no group membership, so nothing was done"
                .to_owned(),
        ),
    }
}

/// A file-safe name for a conversation, the same on every Station.
fn conversation_digest(conversation: &str) -> String {
    sha256_hex(conversation.as_bytes())[..32].to_owned()
}

impl Drop for Desk {
    /// Removes the directory of working directories once its harnesses have removed theirs,
    /// then its lock; a harness still closing leaves both behind, for [`sweep_stale_roots`].
    fn drop(&mut self) {
        if let Ok(mut areas) = self.areas.lock()
            && let Some(root) = areas.take()
            && std::fs::remove_dir(&root.path).is_ok()
        {
            let lock = root.lock.clone();
            drop(root);
            let _ = std::fs::remove_file(lock);
        }
    }
}

/// A desk's directory of working directories, and the lock beside it that says the desk
/// still runs.
struct Root {
    path: PathBuf,
    lock: PathBuf,
    _held: std::fs::File,
}

/// The lock file of the directory of working directories at `root`: beside it, so it is held
/// before the directory exists.
fn root_lock(root: &Path) -> PathBuf {
    let mut name = root.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    root.with_file_name(name)
}

/// A new empty directory under the temporary directory, named at random, readable by the
/// Station's user only and locked, for the conversations' working directories.
fn new_root() -> Result<Root, String> {
    new_root_in(&std::env::temp_dir())
}

/// [`new_root`] in `temp`. The lock file is made and locked first and the directory after, so
/// no sweep ever finds the directory with its lock free.
fn new_root_in(temp: &Path) -> Result<Root, String> {
    use fs2::FileExt;
    for _ in 0..16 {
        let (held, lock) = tempfile::Builder::new()
            .prefix(AREA_PREFIX)
            .suffix(".lock")
            .tempfile_in(temp)
            .and_then(|file| file.keep().map_err(|error| error.error))
            .map_err(|error| format!("assistant working directory not locked: {error}"))?;
        held.try_lock_exclusive()
            .map_err(|error| format!("assistant working directory not locked: {error}"))?;
        let name = lock
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".lock"))
            .ok_or("assistant working directory has no name")?
            .to_owned();
        let path = temp.join(name);
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(&path) {
            Ok(()) => {
                return Ok(Root {
                    path,
                    lock,
                    _held: held,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                drop(held);
                let _ = std::fs::remove_file(&lock);
            }
            Err(error) => {
                drop(held);
                let _ = std::fs::remove_file(&lock);
                return Err(format!("assistant working directory not made: {error}"));
            }
        }
    }
    Err("assistant working directory not made: every name was taken".to_owned())
}

/// Removes what desks that are gone left in `temp`: their directories of working directories,
/// with the conversations' directories, context files (her memory, inlined) and git
/// directories in them, and their lock files. A directory whose lock another desk holds is left
/// alone, and so is one without a lock file, or a lock file without its directory, younger than
/// `stale`. Answers how many directories were removed.
fn sweep_stale_roots(temp: &Path, stale: Duration) -> usize {
    use fs2::FileExt;
    let Ok(entries) = std::fs::read_dir(temp) else {
        return 0;
    };
    let old = |path: &Path| {
        std::fs::symlink_metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= stale)
    };
    let mut swept = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !name.starts_with(AREA_PREFIX) {
            continue;
        }
        let (root, lock) = match name.strip_suffix(".lock") {
            Some(root) if kind.is_file() => (temp.join(root), entry.path()),
            None if kind.is_dir() => (entry.path(), root_lock(&entry.path())),
            _ => continue,
        };
        let held = std::fs::OpenOptions::new().write(true).open(&lock);
        if kind.is_file() {
            // A lock left without its directory: a desk that died, or one between the two.
            if !root.exists()
                && old(&lock)
                && held.is_ok_and(|held| held.try_lock_exclusive().is_ok())
            {
                let _ = std::fs::remove_file(&lock);
            }
            continue;
        }
        let stale = match held {
            Ok(held) => held.try_lock_exclusive().is_ok().then_some(held),
            Err(_) => None,
        };
        let gone = stale.is_some() || (!lock.exists() && old(&root));
        if gone && std::fs::remove_dir_all(&root).is_ok() {
            swept += 1;
            drop(stale);
            let _ = std::fs::remove_file(&lock);
        }
    }
    swept
}

/// The file beside a conversation's directory that holds its harness's standing context:
/// outside the directory, so nothing the harness writes there reaches its system prompt.
fn context_file(area: &Path) -> Option<PathBuf> {
    let name = area.file_name()?.to_str()?;
    Some(area.parent()?.join(format!("{name}.context.md")))
}

/// The directory beside a conversation's directory that holds its repositories' git
/// directories: outside the directory, so the harness can reach no repository's
/// configuration or hooks.
fn repos_dir(area: &Path) -> Option<PathBuf> {
    let name = area.file_name()?.to_str()?;
    Some(area.parent()?.join(format!("{name}.git")))
}

/// The temporary directory beside a conversation's directory, which Claude Code's own temp
/// files and its sandboxed scripts' `$TMPDIR` live under (`CLAUDE_CODE_TMPDIR`, to which it
/// appends `claude-<uid>`): one per conversation, so a script never reads another
/// conversation's temp files, as it can in the per-user default shared by every conversation
/// of the Station's user. Kept short, since a long one makes Claude Code fall back to that
/// shared default for sandboxed scripts.
fn temp_dir(area: &Path) -> Option<PathBuf> {
    let name = area.file_name()?.to_str()?;
    Some(area.parent()?.join(format!("{}.t", name.get(..12)?)))
}

/// The Claude Code configuration directory beside a conversation's directory: outside it, so a
/// sandboxed script never reaches its transcripts or any stored login, and its own, so the
/// Agent's configuration directory (a shared login, every person's transcripts) stays out and
/// each conversation keeps its own.
fn config_dir(area: &Path) -> Option<PathBuf> {
    let name = area.file_name()?.to_str()?;
    Some(area.parent()?.join(format!("{name}.config")))
}

/// Removes a conversation's directory [`Desk::area`] made, its context file and its git
/// directories, and nothing else, each tried again for up to `retry`.
fn remove_area(path: &str, retry: Duration) {
    let path = Path::new(path);
    let root = path.parent();
    let made_here = root.and_then(Path::parent) == Some(std::env::temp_dir().as_path())
        && root
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(AREA_PREFIX))
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit()));
    if !made_here {
        return;
    }
    if let Some(context) = context_file(path) {
        let _ = std::fs::remove_file(context);
    }
    if let Some(repos) = repos_dir(path) {
        remove_tree(&repos, retry);
    }
    if let Some(config) = config_dir(path) {
        remove_tree(&config, retry);
    }
    if let Some(temp) = temp_dir(path) {
        remove_tree(&temp, retry);
    }
    remove_tree(path, retry);
}

/// Removes `path` and everything under it, trying again for up to `retry` while Windows still
/// holds it.
fn remove_tree(path: &Path, retry: Duration) {
    // Windows refuses to remove a directory a process still has open (os error 32), which a
    // harness's processes do for a moment after they have exited, so it is tried again.
    let deadline = Instant::now() + retry;
    let mut delay = Duration::from_millis(5);
    loop {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(100));
            }
            Err(error) => {
                eprintln!(
                    "ai6: assistant working directory {} not removed: {error}",
                    path.display()
                );
                return;
            }
        }
    }
}

/// Whether this Station runs elevated, asked once for its life; an error counts as elevated.
fn station_elevated() -> bool {
    static ELEVATED: OnceLock<bool> = OnceLock::new();
    *ELEVATED.get_or_init(|| crate::station::elevation::is_elevated().unwrap_or(true))
}

/// Whether a Mission's harness can run on this Station's own host, as the Station's OS user: it has
/// the role that takes Missions, and an Operation whose Theater is reached with local access.
fn runs_local_missions(state: &DaemonState) -> bool {
    state.station.roles.contains(&super::StationRole::Execution)
        && state
            .roster
            .operations()
            .any(|operation| super::local_theater_of(&state.station.theaters, operation).is_some())
}

/// Each way the roster's `[assistant]` settings are looser than the default, and this Station
/// running elevated when they let Moneypenny run on it, as the check-in reports them.
pub(super) fn weakenings(roster: &crate::directorates::m::roster::Roster) -> Vec<String> {
    let mut weakened = roster.assistant().weakenings();
    if roster.assistant().allow_elevated() && station_elevated() {
        weakened.push(
            "this Station runs elevated, and assistant.allow_elevated lets Moneypenny's harness \
             run on it"
                .to_owned(),
        );
    }
    weakened
}

/// Lists [`weakenings`] for the operator.
pub(super) fn log_weakenings(roster: &crate::directorates::m::roster::Roster) {
    for weakened in weakenings(roster) {
        eprintln!("ai6: weakening: {weakened}");
    }
}

/// Starts the desk when the Station publishes to Universal Exports; `None` otherwise.
pub(super) fn spawn(state: &DaemonState) -> std::io::Result<Option<JoinHandle<()>>> {
    let Some(publisher) = &state.publisher else {
        return Ok(None);
    };
    if runs_local_missions(state) && !state.roster.assistant().allow_beside_local_missions() {
        eprintln!("ai6: {BESIDE_MISSIONS}");
    }
    let tables = roster_tables(state);
    let swept = sweep_stale_roots(&std::env::temp_dir(), tables().1.unlocked_stale.0);
    if swept > 0 {
        eprintln!("ai6: removed {swept} assistant directories desks that are gone left behind");
    }
    let (origin, token, tls) = publisher.link();
    let mcp_url = origin
        .join(ASSISTANT_MCP_PATH)
        .map_err(|error| std::io::Error::other(format!("assistant endpoint URL: {error}")))?
        .to_string();
    let (reports, queued) = std::sync::mpsc::channel::<Queued>();
    let ordinary = Arc::new(AtomicUsize::new(0));
    let (jobs, mut receiver) = mpsc::channel::<AssistantJob>(8);
    let link = Link {
        origin,
        token,
        tls,
        station: state.station_id.clone(),
        tables: tables.clone(),
    };
    let running = state.stopping.clone();
    let poll_link = link.clone();
    let exports: Arc<dyn Exports> = Arc::new(link.clone());
    std::thread::Builder::new()
        .name("ai6-assistant-jobs".to_owned())
        .spawn(move || poll_link.take_jobs(&running, &jobs))?;
    std::thread::Builder::new()
        .name("ai6-assistant-reports".to_owned())
        .spawn({
            let ordinary = ordinary.clone();
            move || link.send_reports(&queued, &ordinary)
        })?;
    let report: Reporter = Arc::new(move |report, answered| {
        // What must reach Universal Exports always gets in line; an ordinary report only while
        // fewer than `[assistant.desk] report_queue` wait.
        if is_ordinary(&report, answered.as_ref())
            && !tables()
                .1
                .report_queue
                .allows(ordinary.fetch_add(1, Ordering::AcqRel) + 1)
        {
            ordinary.fetch_sub(1, Ordering::AcqRel);
            eprintln!(
                "ai6: an assistant report was dropped: the queue to Universal Exports is full"
            );
            return;
        }
        let _ = reports.send((report, answered));
    });
    let desk = Desk::new(
        state.clone(),
        Arc::new(StationHarness(state.clone())),
        report,
        mcp_url,
    )
    .with_exports(exports);
    let stopping = state.stopping.clone();
    let mut dispatch = Dispatch::new(Arc::new(desk));
    Ok(Some(tokio::spawn(async move {
        while !stopping.load(Ordering::Acquire) {
            tokio::select! {
                job = receiver.recv() => match job {
                    Some(job) => dispatch.send(job),
                    None => break,
                },
                () = tokio::time::sleep(STOP_CHECK) => {
                    #[cfg(feature = "cables-mcp")]
                    dispatch.desk.sweep_left_held();
                }
            }
        }
        dispatch.desk.close_all();
    })))
}

/// Hands each job to the desk on a task of its own: a conversation's jobs one after another in
/// the order they came, other conversations' meanwhile, so one person's slow clone or harness
/// start holds up no one else.
struct Dispatch {
    desk: Arc<Desk>,
    /// Told when each conversation's last job sent is done.
    tails: std::collections::HashMap<String, tokio::sync::oneshot::Receiver<()>>,
}

impl Dispatch {
    fn new(desk: Arc<Desk>) -> Self {
        Self {
            desk,
            tails: std::collections::HashMap::new(),
        }
    }

    /// Starts `job` once its conversation's job before it, if any, is done.
    fn send(&mut self, job: AssistantJob) {
        // A conversation whose last job is done (or whose task ended) waits for nothing.
        self.tails.retain(|_, tail| {
            matches!(
                tail.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            )
        });
        let (done, tail) = tokio::sync::oneshot::channel();
        let before = self.tails.insert(job.conversation_id.clone(), tail);
        let desk = self.desk.clone();
        tokio::spawn(async move {
            if let Some(before) = before {
                let _ = before.await;
            }
            desk.handle(job).await;
            let _ = done.send(());
        });
    }
}

/// The desk's own link to Universal Exports, with the Station's credential.
#[derive(Clone)]
struct Link {
    origin: Url,
    token: String,
    tls: TlsConfig,
    station: String,
    /// `[assistant.desk]` paces its polls and retries.
    tables: Tables,
}

impl Exports for Link {
    fn upload_page_file(
        &self,
        conversation: &str,
        digest: &str,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        let endpoint = self.page_endpoint(conversation, digest, Some(path))?;
        self.agent((self.tables)().1.page_upload_timeout.0)
            .put(endpoint.as_str())
            .header("Authorization", &format!("Bearer {}", self.token))
            .send(&bytes[..])
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn finish_page(
        &self,
        conversation: &str,
        digest: &str,
        manifest: Result<Vec<AssistantPageFile>, String>,
    ) -> Result<(), String> {
        let endpoint = self.page_endpoint(conversation, digest, None)?;
        let body = match manifest {
            Ok(files) => serde_json::json!({"files": files}),
            Err(reason) => serde_json::json!({"refused": reason}),
        };
        self.agent(Duration::ZERO)
            .post(endpoint.as_str())
            .header("Authorization", &format!("Bearer {}", self.token))
            .send_json(body)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// `GET /v1/station/assistant/images/{sha256}?station=&conversation=`.
    fn image(&self, conversation: &str, sha256: &str, most: u64) -> Result<Vec<u8>, String> {
        let mut endpoint = self
            .origin
            .join("v1/station/assistant/images/")
            .map_err(|error| error.to_string())?;
        endpoint
            .path_segments_mut()
            .map_err(|()| "the origin cannot hold a path".to_owned())?
            .pop_if_empty()
            .push(sha256);
        endpoint
            .query_pairs_mut()
            .append_pair("station", &self.station)
            .append_pair("conversation", conversation);
        self.agent(Duration::ZERO)
            .get(endpoint.as_str())
            .header("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|error| error.to_string())?
            .body_mut()
            .with_config()
            .limit(most)
            .read_to_vec()
            .map_err(|error| error.to_string())
    }

    /// `GET /v1/station/assistant/cables`.
    #[cfg(feature = "cables-mcp")]
    fn standing(&self, conversation: &str, operation: &str) -> Result<Standing, String> {
        let mut endpoint = self
            .origin
            .join("v1/station/assistant/cables")
            .map_err(|error| error.to_string())?;
        endpoint
            .query_pairs_mut()
            .append_pair("station", &self.station)
            .append_pair("conversation", conversation)
            .append_pair("operation", operation);
        let answer: serde_json::Value = self
            .agent(Duration::ZERO)
            .get(endpoint.as_str())
            .header("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|error| error.to_string())?
            .body_mut()
            .read_json()
            .map_err(|error| error.to_string())?;
        let flag = |name: &str| {
            answer[name]
                .as_bool()
                .ok_or_else(|| "Universal Exports did not say".to_owned())
        };
        Ok(Standing {
            send: flag("send")?,
            read: flag("read")?,
        })
    }
}

impl Link {
    /// `{pages endpoint}/{digest}[/{path}]` with the Station and conversation as its query.
    fn page_endpoint(
        &self,
        conversation: &str,
        digest: &str,
        path: Option<&str>,
    ) -> Result<Url, String> {
        let mut endpoint = self
            .origin
            .join("v1/station/assistant/pages/")
            .map_err(|error| error.to_string())?;
        {
            let mut segments = endpoint
                .path_segments_mut()
                .map_err(|()| "the origin cannot hold a path".to_owned())?;
            segments.pop_if_empty().push(digest);
            match path {
                Some(path) => segments.extend(path.split('/')),
                None => segments.push("done"),
            };
        }
        endpoint
            .query_pairs_mut()
            .append_pair("station", &self.station)
            .append_pair("conversation", conversation);
        Ok(endpoint)
    }

    /// An agent whose requests may take `wait` and `[assistant.desk] call_margin` more.
    fn agent(&self, wait: Duration) -> ureq::Agent {
        gateway_agent(&self.tls, (self.tables)().1.call_margin.0 + wait)
    }

    fn take_jobs(&self, stopping: &AtomicBool, jobs: &mpsc::Sender<AssistantJob>) {
        let Ok(base) = self.origin.join("v1/station/assistant/next") else {
            return;
        };
        while !stopping.load(Ordering::Acquire) {
            let desk = (self.tables)().1;
            let wait = desk.job_wait.0;
            let started = Instant::now();
            let mut endpoint = base.clone();
            endpoint
                .query_pairs_mut()
                .append_pair("station", &self.station)
                .append_pair("wait_ms", &wait.as_millis().to_string());
            let response = self
                .agent(wait)
                .get(endpoint.as_str())
                .header("Authorization", &format!("Bearer {}", self.token))
                .call();
            let job = match response {
                Ok(mut response) if response.status().as_u16() == 200 => response
                    .body_mut()
                    .with_config()
                    .limit(desk.job_bytes.get().unwrap_or(u64::MAX))
                    .read_to_vec()
                    .ok()
                    .and_then(|body| serde_json::from_slice::<AssistantJob>(&body).ok()),
                Ok(response) if response.status().as_u16() == 204 => {
                    // The gateway may answer before `wait`, which it caps at its own `max_wait`.
                    // At least `job_retry` passes between polls, and no more, so a new message
                    // is never left waiting for a poll.
                    std::thread::sleep(desk.job_retry.0.saturating_sub(started.elapsed()));
                    continue;
                }
                _ => {
                    std::thread::sleep(desk.job_retry.0);
                    continue;
                }
            };
            if let Some(job) = job
                && jobs.blocking_send(job).is_err()
            {
                return;
            }
        }
    }

    /// Sends reports in the order they were queued, and tells each one queued with an
    /// [`Answered`] what became of it. `ordinary` counts the ordinary reports waiting
    /// ([`is_ordinary`]), so the reporter can bound them.
    fn send_reports(&self, queued: &std::sync::mpsc::Receiver<Queued>, ordinary: &AtomicUsize) {
        let Ok(endpoint) = self.origin.join("v1/station/assistant/reports") else {
            return;
        };
        while let Ok((report, answered)) = queued.recv() {
            if is_ordinary(&report, answered.as_ref()) {
                ordinary.fetch_sub(1, Ordering::AcqRel);
            }
            let desk = (self.tables)().1;
            let agent = self.agent(Duration::ZERO);
            let mut delivery = Delivery::Lost;
            for _ in 0..desk.report_attempts {
                let sent = agent
                    .post(endpoint.as_str())
                    .header("Authorization", &format!("Bearer {}", self.token))
                    .send_json(&report);
                // A refused report (an unknown conversation) is not retried.
                match sent {
                    Err(ureq::Error::StatusCode(403 | 404 | 409)) => {
                        delivery = Delivery::Refused;
                        break;
                    }
                    Ok(_) => {
                        delivery = Delivery::Taken;
                        break;
                    }
                    Err(ureq::Error::StatusCode(_)) => break,
                    Err(_) => std::thread::sleep(desk.report_retry.0),
                }
            }
            if let Some(answered) = answered {
                let _ = answered.send(delivery);
            }
        }
    }
}

/// Whether a report queued with `answered` is ordinary: one the reporter may drop for room.
fn is_ordinary(report: &AssistantReport, answered: Option<&Answered>) -> bool {
    answered.is_none() && !must_deliver(report)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::Path;

    use super::super::tests::test_state;
    use super::*;
    use crate::directorates::registry::contexts::{
        ContextSource, EVENTS_FORMAT as CONTEXT_EVENTS_FORMAT,
    };

    /// `[assistant.desk]`'s default `idle` and `report_queue`.
    const IDLE: Duration = Duration::from_secs(600);
    const REPORT_QUEUE: usize = 256;
    use crate::directorates::section_00::assistant::{
        AssistantChange, AssistantCommit, AssistantWorkspace,
    };
    use crate::guatiao::library::ProviderError;

    /// Each start and resume, as the harness was asked for it.
    #[derive(Default)]
    struct Asked {
        starts: Vec<AgentStart>,
        resumes: Vec<AgentResume>,
        messages: Vec<String>,
        interrupts: usize,
        /// A read of [`Script::ReadsLong`] ran out with no output.
        read_ran_out: bool,
        /// What the desk asked of the harness's record.
        records: Vec<SessionRecordQuery>,
    }

    /// What a scripted harness does with a message.
    #[derive(Clone, Copy, PartialEq)]
    enum Script {
        /// Answers `answer: <message>` and completes.
        Answers,
        /// As `Answers`, with each read waiting on the harness's output for as long as it takes.
        ReadsLong,
        /// Ends the turn as cancelled, as the Grok CLI does when a tool is refused.
        CancelsTurn,
        /// Reports a refused tool and completes with no text, as the Antigravity CLI ends a turn.
        Refused,
        /// Exits, as its handler reports it.
        Dies,
        /// Never writes anything.
        Hangs,
        /// Starts answering and completes only when interrupted, as Claude Code and Codex end
        /// an interrupted turn.
        Works,
        /// Writes a note and a memory in `personal/`, then answers and completes.
        Writes,
        /// Calls a file tool (see [`ScriptedSession::answer`]), writes a memory, then answers
        /// and completes.
        Reads,
        /// Writes `personal/calc.py`, runs it with `Bash`, which writes `personal/out.txt`,
        /// then answers and completes.
        Computes,
    }

    /// Universal Exports as a test keeps it: the person may send and read Cables everywhere.
    #[derive(Default)]
    struct Recorded;

    impl Exports for Recorded {
        #[cfg(feature = "cables-mcp")]
        fn standing(&self, _conversation: &str, _operation: &str) -> Result<Standing, String> {
            Ok(Standing {
                send: true,
                read: true,
            })
        }

        fn image(&self, _conversation: &str, sha256: &str, most: u64) -> Result<Vec<u8>, String> {
            if sha256 != sha256_hex(TEST_IMAGE) {
                return Err("no such image".to_owned());
            }
            if TEST_IMAGE.len() as u64 > most {
                return Err("too long".to_owned());
            }
            Ok(TEST_IMAGE.to_vec())
        }
    }

    /// The image Universal Exports keeps for the tests.
    const TEST_IMAGE: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    /// `message` with [`TEST_IMAGE`].
    fn with_image(conversation: &str, message: &str) -> AssistantJob {
        let mut job = job(conversation, message);
        job.images = vec![crate::AssistantImage {
            sha256: sha256_hex(TEST_IMAGE),
            media_type: "image/png".to_owned(),
            bytes: TEST_IMAGE.len() as u64,
        }];
        job
    }

    #[tokio::test]
    async fn an_image_reaches_a_harness_that_takes_images_with_its_message() {
        let fixture = fixture(true, Duration::from_secs(60));
        *fixture.no_images.lock().unwrap() = None;
        fixture
            .desk
            .handle(with_image("c-image", "What is wrong here?"))
            .await;
        assert!(kinds(&fixture, "c-image").contains(&EVENT_ACCEPTED.to_owned()));
        let prompt = fixture.asked.lock().unwrap().starts[0].prompt.clone();
        let content = MessageContent::decode(&prompt).expect("the content form");
        assert_eq!(content.text, "What is wrong here?");
        use base64::Engine as _;
        assert_eq!(
            content.images,
            [MessageImage {
                media_type: "image/png".to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(TEST_IMAGE),
            }]
        );
        // A later message on the running harness carries its image the same way; one without
        // images is plain text.
        fixture.desk.handle(job("c-image", "And this?")).await;
        fixture
            .desk
            .handle(with_image("c-image", "And this one?"))
            .await;
        let messages = fixture.asked.lock().unwrap().messages.clone();
        assert!(
            messages.iter().any(|message| message == "And this?"),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .filter_map(|message| MessageContent::decode(message))
                .any(|content| content.text == "And this one?" && content.images.len() == 1),
            "{messages:?}"
        );
    }

    #[tokio::test]
    async fn a_harness_that_takes_no_image_refuses_the_message_and_says_why() {
        let fixture = fixture(true, Duration::from_secs(60));
        fixture
            .desk
            .handle(with_image("c-no-image", "What is wrong here?"))
            .await;
        let reason = text_of(&fixture, "c-no-image", EVENT_REJECTED).unwrap();
        assert!(reason.contains("it is shown no image"), "{reason}");
        assert!(fixture.asked.lock().unwrap().starts.is_empty());
        // A person's own text that looks like the form stays text for such a harness.
        let forged = format!("{CONTENT_PREFIX}{{\"text\":\"x\",\"images\":[]}}");
        let mut plain = job("c-no-image", &forged);
        plain.job_id = "job-forged".to_owned();
        fixture.desk.handle(plain).await;
        assert_eq!(fixture.asked.lock().unwrap().starts[0].prompt, forged);
    }

    /// What [`Script::Writes`] puts in her memory.
    /// The script [`Script::Computes`] writes.
    const CALC: &str = "open('personal/out.txt', 'w').write(f'{6 * 7}\\n')\n";

    /// A harness that does what `script` says with every message; a resumed one does what
    /// `resumed` says, and one that dies when resumed never confirms its Session. Its scripts
    /// run in a sandbox unless `no_sandbox` says why not, and it loads skills unless
    /// `no_skills` says why not.
    struct Scripted {
        asked: Arc<Mutex<Asked>>,
        resumable: bool,
        script: Script,
        resumed: Script,
        no_sandbox: Arc<Mutex<Option<String>>>,
        no_skills: Arc<Mutex<Option<String>>>,
        no_images: Arc<Mutex<Option<String>>>,
        /// Where the harness says it keeps its record of a Session; `None`, it keeps none.
        record: Arc<Mutex<Option<SessionRecordFile>>>,
    }

    struct ScriptedSession {
        session_id: Option<SessionId>,
        events: Mutex<VecDeque<AgentEvent>>,
        asked: Arc<Mutex<Asked>>,
        script: Script,
        /// The working directory it was started in.
        dir: PathBuf,
    }

    /// The `failed` text a dying scripted harness reports.
    const DIED: &str = "Claude Code exited with code 1 before completing the Session; its last \
                        output:\nError: the stored login has expired";

    fn event(kind: &str, text: Option<&str>) -> AgentEvent {
        AgentEvent {
            kind: kind.to_owned(),
            text: text.map(str::to_owned),
            data: None,
            limit: None,
            usage: None,
            tools: Vec::new(),
            quota: Vec::new(),
        }
    }

    impl ScriptedSession {
        fn answer(&self, message: &str) {
            let mut events = self.events.lock().unwrap();
            match self.script {
                Script::Answers | Script::ReadsLong => {
                    events.push_back(event("system", Some("init")));
                    events.push_back(event(EVENT_TEXT, Some(&format!("answer: {message}"))));
                    events.push_back(event("completed", Some(&format!("answer: {message}"))));
                }
                Script::CancelsTurn => {
                    events.push_back(event(CANCELLED_EVENT, Some(TURN_CANCELLED)));
                }
                Script::Refused => {
                    events.push_back(event(DENIED_EVENT, Some("read_file")));
                    events.push_back(event("completed", Some("")));
                }
                Script::Dies => {
                    events.push_back(event("output", Some("Error: the stored login has expired")));
                    events.push_back(event("failed", Some(DIED)));
                }
                Script::Hangs => {}
                Script::Works => {
                    events.push_back(event(EVENT_TEXT, Some(&format!("working on: {message}"))));
                }
                Script::Writes => {
                    let personal = self.dir.join(PERSONAL);
                    std::fs::create_dir_all(personal.join("notes")).unwrap();
                    std::fs::write(personal.join("notes").join("a.md"), message).unwrap();
                    events.push_back(event(EVENT_TEXT, Some("Noted.")));
                    events.push_back(event("completed", Some("Noted.")));
                }
                Script::Reads => {
                    // `<Tool> <input>` calls that tool, `{area}` in the input being her
                    // working directory; anything else is a `Read` whose line is not given.
                    let mut read = event("tool", None);
                    read.tools = vec!["Read".to_owned()];
                    if let Some((tool, input)) = message.split_once(' ')
                        && input.starts_with('{')
                    {
                        let area = self.dir.to_string_lossy().replace('\\', "/");
                        let input: serde_json::Value =
                            serde_json::from_str(&input.replace("{area}", &area)).unwrap();
                        // `codex:<tool>` calls a tool of her server as Codex announces it.
                        let line = match tool.strip_prefix("codex:") {
                            Some(tool) => serde_json::json!({
                                "method": "item/started",
                                "params": { "item": {
                                    "type": "mcpToolCall", "id": "exec-1",
                                    "server": "moneypenny", "tool": tool,
                                    "status": "inProgress", "arguments": input
                                } }
                            }),
                            None => serde_json::json!({
                                "type": "assistant",
                                "message": { "content": [
                                    { "type": "tool_use", "id": "toolu_1", "name": tool,
                                      "input": input }
                                ] }
                            }),
                        };
                        read.tools = vec![tool.trim_start_matches("codex:").to_owned()];
                        read.data = Some(line.to_string().into_bytes());
                    }
                    events.push_back(read);
                    events.push_back(event("completed", Some("Read it.")));
                }
                Script::Computes => {
                    let personal = self.dir.join(PERSONAL);
                    std::fs::write(personal.join("calc.py"), CALC).unwrap();
                    let mut wrote = event("tool", None);
                    wrote.tools = vec!["Write".to_owned()];
                    events.push_back(wrote);
                    // What `python3 personal/calc.py` leaves behind.
                    std::fs::write(personal.join("out.txt"), "42\n").unwrap();
                    let mut ran = event("tool", None);
                    ran.tools = vec!["Bash".to_owned()];
                    events.push_back(ran);
                    events.push_back(event(EVENT_TEXT, Some("It is 42.")));
                    events.push_back(event("completed", Some("It is 42.")));
                }
            }
        }
    }

    impl AgentSession for ScriptedSession {
        fn session_id(&self) -> Result<Option<String>, ProviderError> {
            Ok(self.session_id.clone().map(String::from))
        }

        fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
            self.asked
                .lock()
                .unwrap()
                .messages
                .push(message.text.clone());
            self.answer(&message.text);
            Ok(())
        }

        fn next_event(&self, _timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let next = self.events.lock().unwrap().pop_front();
                if next.is_some() {
                    return Ok(next);
                }
                std::thread::sleep(Duration::from_millis(5));
                if self.script != Script::ReadsLong {
                    return Ok(None);
                }
                if Instant::now() >= deadline {
                    self.asked.lock().unwrap().read_ran_out = true;
                    return Ok(None);
                }
            }
        }

        fn interrupt(&self) -> Result<(), ProviderError> {
            self.asked.lock().unwrap().interrupts += 1;
            if self.script == Script::Works {
                self.events
                    .lock()
                    .unwrap()
                    .push_back(event("completed", Some("[Request interrupted by user]")));
            }
            Ok(())
        }

        fn close(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    impl Harness for Scripted {
        fn available(&self, handler: &str) -> bool {
            handler == "mock-assistant"
        }

        fn start(
            &self,
            _handler: &str,
            _settings: &HandlerSettings,
            request: AgentStart,
        ) -> Result<Object<dyn AgentSession>, String> {
            let message = request.prompt.clone();
            let dir = PathBuf::from(&request.working_directory);
            self.asked.lock().unwrap().starts.push(request);
            let session = ScriptedSession {
                session_id: Some(SessionId::try_new("provider-assistant-1").unwrap()),
                events: Mutex::default(),
                asked: self.asked.clone(),
                script: self.script,
                dir,
            };
            session.answer(&message);
            Ok(session.into_object())
        }

        fn resume(
            &self,
            _handler: &str,
            _settings: &HandlerSettings,
            request: AgentResume,
        ) -> Result<Object<dyn AgentSession>, String> {
            if !self.resumable {
                return Err("no such provider Session".to_owned());
            }
            let message = request.message.clone().unwrap_or_default();
            let session_id = request.session_id.clone();
            let dir = PathBuf::from(&request.working_directory);
            self.asked.lock().unwrap().resumes.push(request);
            let session = ScriptedSession {
                session_id: (self.resumed != Script::Dies).then_some(session_id),
                events: Mutex::default(),
                asked: self.asked.clone(),
                script: self.resumed,
                dir,
            };
            session.answer(&message);
            Ok(session.into_object())
        }

        fn scripts(
            &self,
            _handler: &str,
            _environment: &BTreeMap<String, String>,
        ) -> Option<String> {
            self.no_sandbox.lock().unwrap().clone()
        }

        fn skills(&self, _handler: &str) -> Option<String> {
            self.no_skills.lock().unwrap().clone()
        }

        fn images(&self, _handler: &str) -> Option<String> {
            self.no_images.lock().unwrap().clone()
        }

        fn record(
            &self,
            _handler: &str,
            _settings: &HandlerSettings,
            query: SessionRecordQuery,
        ) -> Option<SessionRecordFile> {
            self.asked.lock().unwrap().records.push(query);
            self.record.lock().unwrap().clone()
        }
    }

    const AGENTS: &str = "[agent.moneypenny]\nhandler = \"mock-assistant\"\nmodel = \"sonnet\"\n\
                          effort = \"low\"\ncontext = \"You serve the operator.\"\n\
                          env = { CLAUDE_CODE_OAUTH_TOKEN = \"subscription\", LOADOUT_SECRET = \"no\" }\n\
                          [agent.007]\nhandler = \"mock-agent\"\n";

    struct Fixture {
        desk: Arc<Desk>,
        asked: Arc<Mutex<Asked>>,
        reports: Arc<Mutex<Vec<AssistantReport>>>,
        /// What the desk wrote to the Station's log.
        logged: Arc<Mutex<Vec<String>>>,
        /// Why the harness's scripts cannot be sandboxed here; `None`, they can.
        no_sandbox: Arc<Mutex<Option<String>>>,
        /// Why the harness cannot keep to her skills; `None`, it can.
        no_skills: Arc<Mutex<Option<String>>>,
        /// Why the harness cannot be shown images; `None`, it can.
        no_images: Arc<Mutex<Option<String>>>,
        /// Where the harness keeps its record of a Session; `None`, it keeps none.
        record: Arc<Mutex<Option<SessionRecordFile>>>,
        _root: tempfile::TempDir,
    }

    /// The roster's agents file for a Station whose `[assistant]` keeps personal repositories
    /// as `<people>/{person}.git` and shares `<people>/ops` (`ops`) and a repository that does
    /// not exist (`gone`).
    fn station_agents(people: &Path) -> String {
        station_agents_with(people, "")
    }

    /// [`station_agents`] with the further `[assistant]` keys in `keys`.
    fn station_agents_with(people: &Path, keys: &str) -> String {
        format!(
            "{AGENTS}[assistant]\npersonal_repo = '{}'\n{keys}[assistant.shared.ops]\n\
             repo = '{}'\n[assistant.shared.gone]\nrepo = '{}'\n",
            people.join("{person}.git").display(),
            people.join("ops").display(),
            people.join("gone").join("missing.git").display()
        )
    }

    /// A desk with the real `git` safe-house provider on a Station whose roster keeps
    /// personal repositories under `people` ([`station_agents`]).
    fn git_fixture(people: &Path, script: Script) -> Fixture {
        let mut fixture = on_station(
            &station_agents(people),
            Ok(false),
            true,
            IDLE,
            script,
            Script::Answers,
        );
        fixture.desk_mut().houses = git_houses();
        fixture
    }

    impl Fixture {
        /// The desk, to rewire before anything else holds it.
        fn desk_mut(&mut self) -> &mut Desk {
            Arc::get_mut(&mut self.desk).expect("the desk is not shared yet")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.desk.close_all();
            if let Some(root) = self.desk.areas.lock().unwrap().take() {
                let path = root.path.clone();
                drop(root);
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }

    fn fixture(resumable: bool, idle: Duration) -> Fixture {
        scripted(resumable, idle, Script::Answers, Script::Answers)
    }

    fn scripted(resumable: bool, idle: Duration, script: Script, resumed: Script) -> Fixture {
        on_station(AGENTS, Ok(false), resumable, idle, script, resumed)
    }

    /// A desk on a Station whose roster's agents file is `agents` and whose elevation probe
    /// answers `elevated` (the CI hosts' own answer differs by platform).
    fn on_station(
        agents: &str,
        elevated: std::io::Result<bool>,
        resumable: bool,
        idle: Duration,
        script: Script,
        resumed: Script,
    ) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", agents);
        state.station = state.station.clone().with_loadouts(
            "[loadout.moneypenny]
             env = { PATH = \"/opt/moneypenny/bin\", LOADOUT_TOOL_MODE = \"fast\" }
"
            .parse()
            .unwrap(),
        );
        let asked = Arc::new(Mutex::new(Asked::default()));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = reports.clone();
        let logged = Arc::new(Mutex::new(Vec::new()));
        let log = logged.clone();
        let elevated = Arc::new(Mutex::new(Some(elevated)));
        let no_sandbox = Arc::new(Mutex::new(None));
        let no_skills = Arc::new(Mutex::new(None));
        let no_images = Arc::new(Mutex::new(Some("it is shown no image".to_owned())));
        let record = Arc::new(Mutex::new(None));
        let desk = Desk::new(
            state,
            Arc::new(Scripted {
                asked: asked.clone(),
                resumable,
                script,
                resumed,
                no_sandbox: no_sandbox.clone(),
                no_skills: no_skills.clone(),
                no_images: no_images.clone(),
                record: record.clone(),
            }),
            Arc::new(move |report, answered: Option<Answered>| {
                sink.lock().unwrap().push(report);
                if let Some(answered) = answered {
                    let _ = answered.send(Delivery::Taken);
                }
            }),
            "https://exports.example/v1/moneypenny/mcp".to_owned(),
        )
        .with_limits(idle, 2)
        .with_beside_missions(false)
        .with_elevation(
            Arc::new(move || elevated.lock().unwrap().take().expect("the desk asks once")),
            Arc::new(move |line: &str| log.lock().unwrap().push(line.to_owned())),
        )
        .with_exports(Arc::new(Recorded));
        Fixture {
            desk: Arc::new(desk),
            asked,
            reports,
            logged,
            no_sandbox,
            no_skills,
            no_images,
            record,
            _root: root,
        }
    }

    /// The `git` safe-house provider built beside the test binary, loaded from a copy of its
    /// own: a library a test loads stays mapped, and locked on Windows, for the process.
    fn git_houses() -> Houses {
        static PLUGINS: OnceLock<Arc<crate::station::plugins::Plugins>> = OnceLock::new();
        let plugins = PLUGINS
            .get_or_init(|| {
                let deps = std::env::current_exe()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .to_path_buf();
                let library = std::fs::read_dir(&deps)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == std::env::consts::DLL_EXTENSION)
                            && path.file_name().is_some_and(|name| {
                                name.to_string_lossy().contains("ai6_safehouse_git")
                            })
                    })
                    .max_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
                    .expect("the git safe-house provider is built beside the test binary");
                let parent = deps.join("plugin-scratch");
                std::fs::create_dir_all(&parent).unwrap();
                let scratch = tempfile::Builder::new()
                    .prefix("assistant-git-")
                    .tempdir_in(parent)
                    .unwrap()
                    .keep();
                std::fs::copy(&library, scratch.join(library.file_name().unwrap())).unwrap();
                Arc::new(crate::station::plugins::Plugins::scan(&[scratch]))
            })
            .clone();
        Arc::new(move || {
            plugins
                .safehouse(workspace::PROVIDER, None)
                .map_err(|error| error.to_string())
        })
    }

    /// Runs `git` in `cwd` and answers what it printed.
    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn workspace(personal: &Path) -> AssistantWorkspace {
        AssistantWorkspace {
            personal: personal.to_string_lossy().into_owned(),
            shared: BTreeMap::new(),
            author_name: "Eve Moneypenny".to_owned(),
            author_email: "eve@example.invalid".to_owned(),
        }
    }

    /// A message job of a conversation whose workspace is `workspace`, for a person in the
    /// groups `ops` and `gone` ([`station_agents`]).
    fn working(conversation: &str, message: &str, workspace: AssistantWorkspace) -> AssistantJob {
        AssistantJob {
            workspace: Some(workspace),
            person_id: Some("eve".to_owned()),
            groups: Some(vec!["ops".to_owned(), "gone".to_owned()]),
            ..job(conversation, message)
        }
    }

    /// A job that commits `change` to `repo` as `ops`'s repository, as the person, a member of it.
    fn committing(conversation: &str, repo: &Path, change: AssistantChange) -> AssistantJob {
        AssistantJob {
            job_id: format!("commit-{}", &change.digest[..8]),
            message: String::new(),
            context: Vec::new(),
            recap: None,
            groups: Some(vec!["ops".to_owned()]),
            commit: Some(AssistantCommit {
                repo: repo.to_string_lossy().into_owned(),
                group: "ops".to_owned(),
                change,
                message: format!(
                    "moneypenny: remember\n\nVia: Moneypenny (conversation {conversation})"
                ),
                author_name: "Eve Moneypenny".to_owned(),
                author_email: "eve@example.invalid".to_owned(),
            }),
            person_id: Some("eve".to_owned()),
            ..job(conversation, "unused")
        }
    }

    /// The events reported for `conversation`, oldest first.
    fn events(fixture: &Fixture, conversation: &str) -> Vec<AssistantEvent> {
        fixture
            .reports
            .lock()
            .unwrap()
            .iter()
            .filter(|report| report.conversation_id == conversation)
            .flat_map(|report| report.events.clone())
            .collect()
    }

    fn job(conversation: &str, message: &str) -> AssistantJob {
        AssistantJob {
            route: None,
            job_id: format!("job-{message}"),
            conversation_id: conversation.to_owned(),
            agent_id: "moneypenny".to_owned(),
            message: message.to_owned(),
            context: vec!["You are Moneypenny.".to_owned()],
            recap: Some("Earlier in this conversation: hello.".to_owned()),
            mcp_token: "conversation-token-".repeat(3),
            token_expires_at: unix_now() + 3_600.0,
            session_id: None,
            close: false,
            interrupt: false,
            person_id: None,
            workspace: None,
            commit: None,
            cables: Vec::new(),
            cable_confirm: None,
            cable_discard: None,
            reach: Default::default(),
            skills: Vec::new(),
            groups: None,
            images: Vec::new(),
        }
    }

    /// A job that stops the conversation's answer.
    fn interruption(conversation: &str, id: &str) -> AssistantJob {
        AssistantJob {
            job_id: format!("interrupt-{id}"),
            message: String::new(),
            context: Vec::new(),
            recap: None,
            interrupt: true,
            ..job(conversation, "unused")
        }
    }

    fn kinds(fixture: &Fixture, conversation: &str) -> Vec<String> {
        fixture
            .reports
            .lock()
            .unwrap()
            .iter()
            .filter(|report| report.conversation_id == conversation)
            .flat_map(|report| report.events.iter().map(|event| event.kind.clone()))
            .collect()
    }

    /// Waits for `ready`; generous, since CI clocks are slow.
    async fn until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !ready() {
            assert!(Instant::now() < deadline, "timed out waiting");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Connects to a Cable endpoint as her harness would; retried until the deadline, since a
    /// launch is recorded before the endpoint it names is listening.
    #[cfg(feature = "cables-mcp")]
    async fn cable_client(
        url: &str,
        token: &str,
    ) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
        use rmcp::ServiceExt;
        use rmcp::transport::StreamableHttpClientTransport;
        use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let transport = StreamableHttpClientTransport::from_config(
                StreamableHttpClientTransportConfig::with_uri(url.to_owned())
                    .auth_header(token.to_owned()),
            );
            match ().serve(transport).await {
                Ok(client) => return client,
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "Cable endpoint never answered: {error}"
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    }

    /// Her routes in the owner's order: free first, then a gateway through Claude Code.
    const HER_ROUTES: &str = r#"
[agent.moneypenny]
balance = "priority"
context = "You serve the operator."

[[agent.moneypenny.route]]
handler = "mock-grok"
model = "grok-4.7"
priority = 2
[agent.moneypenny.route.handler_settings]
command = "grok"
args = ["agent", "stdio"]

[[agent.moneypenny.route]]
handler = "mock-claude"
model = "gemini-free"
effort = "low"
priority = 1
secrets = ["litellm"]
env = { ANTHROPIC_BASE_URL = "https://llm.example", ANTHROPIC_SMALL_FAST_MODEL = "gemini-free" }
"#;
    /// As Grok CLI 1.0.70 worded a real limit, through the acp handler: no reset time.
    const GROK_LIMIT: &str = "Rate limited: API error (status 429 Too Many Requests): \
                              subscription:free-usage-exhausted: You've used all the included free \
                              usage for model grok-4.7 for now. Usage resets over a rolling 24-hour \
                              window — tokens (actual/limit): 609418/600000. Upgrade to a Grok \
                              subscription for higher limits: https://grok.com/super";

    /// What a routed harness was asked to do.
    #[derive(Clone)]
    enum RoutedAsk {
        Start(String, HandlerSettings, AgentStart),
        Resume(String, HandlerSettings, AgentResume),
    }

    /// A harness with several handlers: its provider Session ID comes only once a turn runs, as a
    /// real stream reports it, a handler in `limited` stops at a limit in Grok's words, and one in
    /// `logged_out` does not start.
    #[derive(Clone, Default)]
    struct RoutedHarness {
        handlers: Arc<Mutex<Vec<String>>>,
        limited: Arc<Mutex<Vec<String>>>,
        logged_out: Arc<Mutex<Vec<String>>>,
        /// An event a handler in `limited` shows before its limit: text, or a tool call.
        before_limit: Arc<Mutex<Option<AgentEvent>>>,
        /// Handlers whose start waits until `release` is set.
        slow: Arc<Mutex<Vec<String>>>,
        release: Arc<AtomicBool>,
        /// Starts waiting for `release`.
        waiting: Arc<AtomicUsize>,
        asked: Arc<Mutex<Vec<RoutedAsk>>>,
        started: Arc<AtomicUsize>,
        /// Each message a harness was sent after it started, by handler.
        sent: Arc<Mutex<Vec<(String, String)>>>,
        interrupts: Arc<AtomicUsize>,
    }

    struct RoutedSession {
        id: SessionId,
        handler: String,
        events: Mutex<VecDeque<AgentEvent>>,
        sent: Arc<Mutex<Vec<(String, String)>>>,
        interrupts: Arc<AtomicUsize>,
    }

    impl AgentSession for RoutedSession {
        fn session_id(&self) -> Result<Option<String>, ProviderError> {
            Ok(Some(self.id.to_string()))
        }

        fn send_message(&self, message: AgentMessage) -> Result<(), ProviderError> {
            self.sent
                .lock()
                .unwrap()
                .push((self.handler.clone(), message.text));
            Ok(())
        }

        fn next_event(&self, timeout_ms: i64) -> Result<Option<AgentEvent>, ProviderError> {
            let next = self.events.lock().unwrap().pop_front();
            if next.is_none() {
                std::thread::sleep(Duration::from_millis(timeout_ms.min(20) as u64));
            }
            Ok(next)
        }

        fn interrupt(&self) -> Result<(), ProviderError> {
            self.interrupts.fetch_add(1, Ordering::SeqCst);
            self.events
                .lock()
                .unwrap()
                .push_back(event("completed", Some("[Request interrupted by user]")));
            Ok(())
        }

        fn close(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    impl RoutedHarness {
        fn session(&self, handler: &str, id: String) -> Object<dyn AgentSession> {
            let mut events = VecDeque::new();
            if self
                .limited
                .lock()
                .unwrap()
                .iter()
                .any(|name| name == handler)
            {
                events.extend(self.before_limit.lock().unwrap().clone());
                let mut limit = event(LIMIT_EVENT, Some(GROK_LIMIT));
                limit.limit = Some(coqui_acp_provider::UsageLimit {
                    resets_at: None,
                    window: None,
                });
                events.push_back(limit);
            } else {
                events.push_back(event(EVENT_TEXT, Some("an answer")));
                events.push_back(event("completed", Some("an answer")));
            }
            RoutedSession {
                id: SessionId::try_new(id).unwrap(),
                handler: handler.to_owned(),
                events: Mutex::new(events),
                sent: self.sent.clone(),
                interrupts: self.interrupts.clone(),
            }
            .into_object()
        }
    }

    impl Harness for RoutedHarness {
        fn available(&self, handler: &str) -> bool {
            self.handlers
                .lock()
                .unwrap()
                .iter()
                .any(|name| name == handler)
        }

        fn start(
            &self,
            handler: &str,
            settings: &HandlerSettings,
            request: AgentStart,
        ) -> Result<Object<dyn AgentSession>, String> {
            if self.slow.lock().unwrap().iter().any(|name| name == handler) {
                self.waiting.fetch_add(1, Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(60);
                while !self.release.load(Ordering::SeqCst) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            if self
                .logged_out
                .lock()
                .unwrap()
                .iter()
                .any(|name| name == handler)
            {
                return Err("session/new: Authentication required".to_owned());
            }
            self.asked.lock().unwrap().push(RoutedAsk::Start(
                handler.to_owned(),
                settings.clone(),
                request,
            ));
            let started = self.started.fetch_add(1, Ordering::SeqCst);
            Ok(self.session(handler, format!("provider-{handler}-{started}")))
        }

        fn resume(
            &self,
            handler: &str,
            settings: &HandlerSettings,
            request: AgentResume,
        ) -> Result<Object<dyn AgentSession>, String> {
            let id = request.session_id.as_str().to_owned();
            self.asked.lock().unwrap().push(RoutedAsk::Resume(
                handler.to_owned(),
                settings.clone(),
                request,
            ));
            Ok(self.session(handler, id))
        }
    }

    struct RoutedFixture {
        desk: Arc<Desk>,
        harness: RoutedHarness,
        state: DaemonState,
        reports: Arc<Mutex<Vec<AssistantReport>>>,
        _root: tempfile::TempDir,
    }

    impl Drop for RoutedFixture {
        fn drop(&mut self) {
            self.desk.close_all();
            if let Some(root) = self.desk.areas.lock().unwrap().take() {
                let path = root.path.clone();
                drop(root);
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }

    fn routed_fixture(agents: &str, loadouts: &str, handlers: &[&str]) -> RoutedFixture {
        let root = tempfile::tempdir().unwrap();
        let mut state = test_state(root.path(), "", agents);
        state.station = state.station.clone().with_loadouts(
            loadouts
                .parse::<crate::directorates::q::LoadoutCatalog>()
                .unwrap()
                .with_default_secret_provider("fake-secrets"),
        );
        let harness = RoutedHarness::default();
        *harness.handlers.lock().unwrap() = handlers.iter().map(|h| (*h).to_owned()).collect();
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = reports.clone();
        let desk = Desk::new(
            state.clone(),
            Arc::new(harness.clone()),
            Arc::new(move |report, answered: Option<Answered>| {
                sink.lock().unwrap().push(report);
                if let Some(answered) = answered {
                    let _ = answered.send(Delivery::Taken);
                }
            }),
            "https://exports.example/v1/moneypenny/mcp".to_owned(),
        )
        .with_limits(IDLE, 4)
        .with_beside_missions(false)
        .with_elevation(Arc::new(|| Ok(false)), Arc::new(|_: &str| {}))
        .with_exports(Arc::new(Recorded));
        RoutedFixture {
            desk: Arc::new(desk),
            harness,
            state,
            reports,
            _root: root,
        }
    }

    impl RoutedFixture {
        fn kinds(&self, conversation: &str) -> Vec<String> {
            self.reports
                .lock()
                .unwrap()
                .iter()
                .filter(|report| report.conversation_id == conversation)
                .flat_map(|report| report.events.iter().map(|event| event.kind.clone()))
                .collect()
        }

        fn texts(&self, conversation: &str, kind: &str) -> Vec<String> {
            self.reports
                .lock()
                .unwrap()
                .iter()
                .filter(|report| report.conversation_id == conversation)
                .flat_map(|report| report.events.iter())
                .filter(|event| event.kind == kind)
                .filter_map(|event| event.text.clone())
                .collect()
        }

        fn last_route(&self, conversation: &str) -> Option<RouteRef> {
            self.reports
                .lock()
                .unwrap()
                .iter()
                .filter(|report| report.conversation_id == conversation)
                .filter_map(|report| report.route.clone())
                .next_back()
        }

        /// Waits until the conversation's harness has closed.
        async fn closed(&self, conversation: &str) {
            self.desk.close(conversation);
            until(|| self.kinds(conversation).contains(&EVENT_CLOSED.to_owned())).await;
        }
    }

    const LITELLM: &str = "[secret.litellm]\nenv = \"ANTHROPIC_AUTH_TOKEN\"\n";

    /// Her three routes, free first, then a gateway through Claude Code, then a last one.
    const THREE_ROUTES: &str = r#"
[agent.moneypenny]
balance = "priority"
context = "You serve the operator."

[[agent.moneypenny.route]]
handler = "mock-grok"
model = "grok-4.7"
priority = 3

[[agent.moneypenny.route]]
handler = "mock-claude"
model = "gemini-free"
priority = 2

[[agent.moneypenny.route]]
handler = "mock-codex"
model = "gpt-last"
priority = 1
"#;

    const ALL_THREE: &[&str] = &["mock-grok", "mock-claude", "mock-codex"];

    impl RoutedFixture {
        fn limit(&self, handlers: &[&str]) {
            *self.harness.limited.lock().unwrap() =
                handlers.iter().map(|name| (*name).to_owned()).collect();
        }

        /// The handlers started so far, in order.
        fn started(&self) -> Vec<String> {
            self.harness
                .asked
                .lock()
                .unwrap()
                .iter()
                .map(|ask| match ask {
                    RoutedAsk::Start(handler, ..) | RoutedAsk::Resume(handler, ..) => {
                        handler.clone()
                    }
                })
                .collect()
        }
    }

    #[tokio::test]
    async fn a_limit_before_the_turn_did_anything_sends_the_same_message_to_her_next_route() {
        let fixture = routed_fixture(HER_ROUTES, LITELLM, &["mock-grok", "mock-claude"]);
        fixture.limit(&["mock-grok"]);

        fixture.desk.handle(job("c-limit", "first")).await;
        until(|| {
            fixture
                .kinds("c-limit")
                .contains(&EVENT_COMPLETED.to_owned())
        })
        .await;
        let asked = fixture.harness.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 2, "the first route, then the next");
        let RoutedAsk::Start(handler, settings, start) = asked[0].clone() else {
            panic!("a start");
        };
        assert_eq!(handler, "mock-grok", "the first in order");
        assert_eq!(settings["command"], "grok", "the route's own settings");
        assert!(!start.environment.contains_key("ANTHROPIC_BASE_URL"));
        let RoutedAsk::Start(handler, settings, start) = asked[1].clone() else {
            panic!("a provider Session of another source is not resumed");
        };
        assert_eq!(handler, "mock-claude");
        assert!(settings.is_empty());
        assert_eq!(start.prompt, "first", "the message that met the limit");
        assert_eq!(start.model.as_deref(), Some("gemini-free"));
        assert_eq!(start.effort.as_deref(), Some("low"));
        assert_eq!(
            start.environment["ANTHROPIC_BASE_URL"], "https://llm.example",
            "the route's env passes what her environment otherwise keeps out"
        );
        assert_eq!(
            start.environment["ANTHROPIC_AUTH_TOKEN"], "fake-litellm-0451",
            "the route's secret"
        );
        assert!(
            start
                .initial_context
                .iter()
                .any(|line| line == "Earlier in this conversation: hello."),
            "afresh, with its recap: {:?}",
            start.initial_context
        );
        let kinds = fixture.kinds("c-limit");
        assert!(
            !kinds.contains(&EVENT_LIMIT.to_owned()) && !kinds.contains(&EVENT_CLOSED.to_owned()),
            "the conversation neither ends nor shows an error: {kinds:?}"
        );
        let told = fixture.texts("c-limit", EVENT_NOTICE);
        assert!(
            told.iter()
                .any(|text| text.contains("mock-grok grok-4.7") && text.contains("out of quota")),
            "{told:?}"
        );
        assert!(
            told.iter()
                .any(|text| text.contains("moved from mock-grok grok-4.7 to mock-claude")),
            "the person is told which route took over: {told:?}"
        );
        assert_eq!(
            fixture.last_route("c-limit"),
            Some(RouteRef::new(
                "mock-claude",
                Some("gemini-free".to_owned()),
                None
            )),
            "Universal Exports learns the new route with the new Session"
        );
        let marked = fixture
            .state
            .intake
            .accounts()
            .unwrap()
            .into_iter()
            .find(|row| row.limited_until.is_some())
            .expect("the route is marked for every Station");
        assert!(routes::is_implicit(&marked.account.pool));
        assert!(marked.active.is_empty(), "its claim is freed");
    }

    #[tokio::test]
    async fn she_walks_her_routes_once_each_while_they_report_limits() {
        let fixture = routed_fixture(THREE_ROUTES, LITELLM, ALL_THREE);
        fixture.limit(&["mock-grok", "mock-claude"]);

        fixture.desk.handle(job("c-walk", "first")).await;
        until(|| {
            fixture
                .kinds("c-walk")
                .contains(&EVENT_COMPLETED.to_owned())
        })
        .await;
        assert_eq!(
            fixture.started(),
            ["mock-grok", "mock-claude", "mock-codex"]
        );
        assert_eq!(fixture.last_route("c-walk").unwrap().handler, "mock-codex");
        assert!(!fixture.kinds("c-walk").contains(&EVENT_LIMIT.to_owned()));
    }

    #[tokio::test]
    async fn with_no_route_left_a_limit_ends_the_turn_with_the_notice_as_before() {
        let fixture = routed_fixture(THREE_ROUTES, LITELLM, ALL_THREE);
        fixture.limit(ALL_THREE);

        fixture.desk.handle(job("c-spent", "first")).await;
        until(|| fixture.kinds("c-spent").contains(&EVENT_CLOSED.to_owned())).await;
        assert_eq!(
            fixture.started(),
            ["mock-grok", "mock-claude", "mock-codex"],
            "each route once, and no more"
        );
        let kinds = fixture.kinds("c-spent");
        assert_eq!(
            kinds.iter().filter(|kind| *kind == EVENT_LIMIT).count(),
            1,
            "the last limit is shown: {kinds:?}"
        );
        let told = fixture.texts("c-spent", EVENT_NOTICE);
        assert!(
            told.iter()
                .any(|text| text.contains("next message takes another route")),
            "{told:?}"
        );
    }

    #[tokio::test]
    async fn a_limit_after_text_or_a_tool_call_is_never_sent_again() {
        for (conversation, shown) in [
            ("c-text", event(EVENT_TEXT, Some("Here is the beginning"))),
            ("c-tool", {
                let mut call = event(EVENT_TOOL, None);
                call.tools = vec!["publish_page".to_owned()];
                call
            }),
        ] {
            let fixture = routed_fixture(HER_ROUTES, LITELLM, &["mock-grok", "mock-claude"]);
            fixture.limit(&["mock-grok"]);
            *fixture.harness.before_limit.lock().unwrap() = Some(shown);

            fixture.desk.handle(job(conversation, "first")).await;
            until(|| {
                fixture
                    .kinds(conversation)
                    .contains(&EVENT_CLOSED.to_owned())
            })
            .await;
            assert_eq!(
                fixture.started(),
                ["mock-grok"],
                "{conversation}: nothing resent"
            );
            let kinds = fixture.kinds(conversation);
            assert!(kinds.contains(&EVENT_LIMIT.to_owned()), "{kinds:?}");
            assert!(
                fixture
                    .texts(conversation, EVENT_NOTICE)
                    .iter()
                    .any(|text| text.contains("next message takes another route")),
                "{conversation}: the notice, as before"
            );
        }
    }

    #[tokio::test]
    async fn a_message_that_comes_while_hers_moves_follows_it_to_the_new_route() {
        let fixture = routed_fixture(HER_ROUTES, LITELLM, &["mock-grok", "mock-claude"]);
        fixture.limit(&["mock-grok"]);
        *fixture.harness.slow.lock().unwrap() = vec!["mock-claude".to_owned()];

        fixture.desk.handle(job("c-follow", "first")).await;
        until(|| fixture.harness.waiting.load(Ordering::SeqCst) == 1).await;
        let desk = fixture.desk.clone();
        let second = tokio::spawn(async move { desk.handle(job("c-follow", "second")).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            fixture.harness.sent.lock().unwrap().is_empty() && !second.is_finished(),
            "it waits for the move rather than start a harness of its own"
        );
        fixture.harness.release.store(true, Ordering::SeqCst);
        second.await.unwrap();
        until(|| !fixture.harness.sent.lock().unwrap().is_empty()).await;
        assert_eq!(
            fixture.harness.sent.lock().unwrap().clone(),
            [("mock-claude".to_owned(), "second".to_owned())],
            "the route that took over has it"
        );
        assert_eq!(fixture.started(), ["mock-grok", "mock-claude"]);
        fixture.closed("c-follow").await;
    }

    #[tokio::test]
    async fn a_stop_during_the_move_ends_the_walk_before_another_route_starts() {
        let fixture = routed_fixture(THREE_ROUTES, LITELLM, ALL_THREE);
        fixture.limit(&["mock-grok"]);
        *fixture.harness.slow.lock().unwrap() = vec!["mock-claude".to_owned()];
        *fixture.harness.logged_out.lock().unwrap() = vec!["mock-claude".to_owned()];

        fixture.desk.handle(job("c-stop", "first")).await;
        until(|| fixture.harness.waiting.load(Ordering::SeqCst) == 1).await;
        fixture.desk.handle(interruption("c-stop", "1")).await;
        fixture.harness.release.store(true, Ordering::SeqCst);
        until(|| fixture.kinds("c-stop").contains(&EVENT_CLOSED.to_owned())).await;
        let kinds = fixture.kinds("c-stop");
        assert!(kinds.contains(&EVENT_INTERRUPTED.to_owned()), "{kinds:?}");
        assert_eq!(
            fixture.started(),
            ["mock-grok"],
            "the route that failed to start did not hand the message to the third"
        );
    }

    #[tokio::test]
    async fn a_stop_that_comes_as_the_new_route_starts_stops_its_answer() {
        let fixture = routed_fixture(HER_ROUTES, LITELLM, &["mock-grok", "mock-claude"]);
        fixture.limit(&["mock-grok"]);
        *fixture.harness.slow.lock().unwrap() = vec!["mock-claude".to_owned()];

        fixture.desk.handle(job("c-late", "first")).await;
        until(|| fixture.harness.waiting.load(Ordering::SeqCst) == 1).await;
        fixture.desk.handle(interruption("c-late", "1")).await;
        fixture.harness.release.store(true, Ordering::SeqCst);
        until(|| {
            fixture
                .kinds("c-late")
                .contains(&EVENT_INTERRUPTED.to_owned())
        })
        .await;
        assert_eq!(
            fixture.harness.interrupts.load(Ordering::SeqCst),
            1,
            "the harness that took over was stopped"
        );
        fixture.closed("c-late").await;
    }

    /// The close of the harness a message replaces is not told after the message's `accepted`,
    /// where it would read as her closing before she answered.
    #[test]
    fn a_replaced_harness_s_close_goes_before_the_messages_answer() {
        let sent: Arc<Mutex<Vec<AssistantReport>>> = Arc::default();
        let sink = sent.clone();
        let outbox = Outbox::new(Arc::new(move |report, _answered: Option<Answered>| {
            sink.lock().unwrap().push(report);
        }));
        let report = |kind: &str| AssistantReport {
            route: None,
            run: None,
            station_id: "hq".to_owned(),
            conversation_id: "c".to_owned(),
            job_id: None,
            session_id: None,
            events: vec![AssistantEvent::new(kind, None)],
        };
        outbox.hold("c");
        outbox.send(report(EVENT_CLOSED));
        outbox.send(report(EVENT_TEXT));
        assert!(sent.lock().unwrap().is_empty(), "held for the message");
        outbox.release_closed("c");
        let kinds: Vec<String> = sent
            .lock()
            .unwrap()
            .iter()
            .flat_map(|report| report.events.iter().map(|event| event.kind.clone()))
            .collect();
        assert_eq!(kinds, [EVENT_CLOSED], "the rest waits for its answer");
        outbox.release_closed("nothing-held");
    }

    #[tokio::test]
    async fn a_route_whose_harness_does_not_start_gives_way_to_the_next_and_with_none_left_she_is_refused()
     {
        let fixture = routed_fixture(HER_ROUTES, LITELLM, &["mock-grok", "mock-claude"]);
        *fixture.harness.logged_out.lock().unwrap() = vec!["mock-grok".to_owned()];
        fixture.desk.handle(job("c-out", "hello")).await;
        until(|| fixture.kinds("c-out").contains(&EVENT_COMPLETED.to_owned())).await;
        assert_eq!(fixture.last_route("c-out").unwrap().handler, "mock-claude");
        assert!(
            fixture.kinds("c-out").contains(&EVENT_ACCEPTED.to_owned()),
            "the person never sees the first route's trouble"
        );

        *fixture.harness.logged_out.lock().unwrap() =
            vec!["mock-grok".to_owned(), "mock-claude".to_owned()];
        fixture.desk.handle(job("c-out-both", "hello")).await;
        until(|| {
            fixture
                .kinds("c-out-both")
                .contains(&EVENT_REJECTED.to_owned())
        })
        .await;
        let refused = fixture.texts("c-out-both", EVENT_REJECTED).remove(0);
        for expected in [
            "mock-grok grok-4.7",
            "mock-claude gemini-free",
            "Authentication required",
        ] {
            assert!(refused.contains(expected), "{refused}");
        }
        assert!(
            fixture
                .state
                .intake
                .accounts()
                .unwrap()
                .iter()
                .all(|row| row
                    .active
                    .iter()
                    .all(|claim| claim.mission != session_name("c-out-both"))),
            "a harness that did not start holds nothing"
        );
    }

    #[tokio::test]
    async fn her_conversation_resumes_on_the_same_route_with_its_settings() {
        let fixture = routed_fixture(HER_ROUTES, LITELLM, &["mock-grok", "mock-claude"]);
        let resume = AssistantJob {
            session_id: Some("provider-earlier".to_owned()),
            route: Some(RouteRef::new(
                "mock-claude",
                Some("gemini-free".to_owned()),
                None,
            )),
            ..job("c-same", "again")
        };
        fixture.desk.handle(resume).await;
        until(|| {
            fixture
                .kinds("c-same")
                .contains(&EVENT_COMPLETED.to_owned())
        })
        .await;
        let asked = fixture.harness.asked.lock().unwrap().clone();
        let RoutedAsk::Resume(handler, settings, resume) = &asked[0] else {
            panic!("the Session it ran resumes, though another route comes first in order");
        };
        assert_eq!(handler, "mock-claude");
        assert!(settings.is_empty(), "this route's, not the first's");
        assert_eq!(resume.session_id.as_str(), "provider-earlier");
        assert_eq!(
            resume.environment["ANTHROPIC_AUTH_TOKEN"],
            "fake-litellm-0451"
        );
        assert!(
            !resume
                .initial_context
                .iter()
                .any(|line| line == "Earlier in this conversation: hello."),
            "a resumed harness has the conversation"
        );
        assert!(
            fixture.texts("c-same", EVENT_NOTICE).is_empty(),
            "nothing moved"
        );
    }

    #[tokio::test]
    async fn a_station_with_no_handler_of_hers_refuses_so_another_station_can_take_her() {
        let fixture = routed_fixture(HER_ROUTES, LITELLM, &[]);
        fixture.desk.handle(job("c-none", "hello")).await;
        until(|| fixture.kinds("c-none").contains(&EVENT_REJECTED.to_owned())).await;
        let refused = fixture.texts("c-none", EVENT_REJECTED).remove(0);
        for expected in [
            "mock-grok grok-4.7",
            "mock-claude gemini-free",
            "no \"mock-grok\" handler",
        ] {
            assert!(refused.contains(expected), "{refused}");
        }
        assert!(fixture.harness.asked.lock().unwrap().is_empty());

        // One handler here: she runs on it, though it comes second.
        *fixture.harness.handlers.lock().unwrap() = vec!["mock-claude".to_owned()];
        fixture.desk.handle(job("c-second", "hello")).await;
        until(|| {
            fixture
                .kinds("c-second")
                .contains(&EVENT_COMPLETED.to_owned())
        })
        .await;
        assert_eq!(
            fixture.last_route("c-second").unwrap().handler,
            "mock-claude"
        );
    }

    #[tokio::test]
    async fn with_every_route_out_of_quota_she_is_refused_naming_each_and_a_claim_is_freed_at_close()
     {
        let agents = r#"
[agent.moneypenny]
balance = "priority"
[[agent.moneypenny.route]]
handler = "mock-grok"
model = "grok-4.7"
pool = "grok-free"
[[agent.moneypenny.route]]
handler = "mock-claude"
model = "gemini-free"
"#;
        let loadouts = r#"
[pool.grok-free]
handler = "mock-grok"
named_only = true
[pool.grok-free.account.free]
kind = "harness-login"
max_concurrent = 1
"#;
        let fixture = routed_fixture(agents, loadouts, &["mock-grok", "mock-claude"]);
        let first = crate::directorates::registry::accounts::AccountRef::new("grok-free", "free");

        // Its one place taken, the next conversation goes to the next route.
        fixture.desk.handle(job("c-one", "hello")).await;
        until(|| fixture.kinds("c-one").contains(&EVENT_COMPLETED.to_owned())).await;
        assert_eq!(fixture.last_route("c-one").unwrap().handler, "mock-grok");
        fixture.desk.handle(job("c-two", "hello")).await;
        until(|| fixture.kinds("c-two").contains(&EVENT_COMPLETED.to_owned())).await;
        assert_eq!(fixture.last_route("c-two").unwrap().handler, "mock-claude");

        fixture.closed("c-one").await;
        let row = |state: &DaemonState| {
            state
                .intake
                .accounts()
                .unwrap()
                .into_iter()
                .find(|row| row.account == first)
                .unwrap()
        };
        assert!(row(&fixture.state).active.is_empty(), "freed at close");
        fixture.desk.handle(job("c-three", "hello")).await;
        until(|| {
            fixture
                .kinds("c-three")
                .contains(&EVENT_COMPLETED.to_owned())
        })
        .await;
        assert_eq!(fixture.last_route("c-three").unwrap().handler, "mock-grok");
        fixture.closed("c-two").await;
        fixture.closed("c-three").await;

        fixture
            .state
            .intake
            .limit_account(&first, unix_now() + 3_600.0)
            .unwrap();
        let second = routes::implicit_pool(
            "moneypenny",
            &fixture
                .state
                .roster
                .agent(None, "moneypenny")
                .unwrap()
                .route_list()[1],
        );
        fixture
            .state
            .intake
            .limit_account(
                &crate::directorates::registry::accounts::AccountRef::new(second.id(), "login"),
                unix_now() + 3_600.0,
            )
            .unwrap();
        fixture.desk.handle(job("c-four", "hello")).await;
        until(|| fixture.kinds("c-four").contains(&EVENT_REJECTED.to_owned())).await;
        let refused = fixture.texts("c-four", EVENT_REJECTED).remove(0);
        assert!(refused.contains("grok-4.7 (pool grok-free)"), "{refused}");
        assert!(
            refused.contains("gemini-free: its limit resets"),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn the_plain_form_draws_a_pool_account_as_a_mission_does() {
        let agents = "[agent.moneypenny]\nhandler = \"mock-claude\"\n";
        let loadouts = r#"
[pool.gateway]
handler = "mock-claude"
[pool.gateway.account.key]
kind = "anthropic-auth-token"
secret = "gateway-key"
"#;
        let fixture = routed_fixture(agents, loadouts, &["mock-claude"]);
        fixture.desk.handle(job("c-plain", "hello")).await;
        until(|| {
            fixture
                .kinds("c-plain")
                .contains(&EVENT_COMPLETED.to_owned())
        })
        .await;
        let RoutedAsk::Start(_, _, start) = fixture.harness.asked.lock().unwrap()[0].clone() else {
            panic!("a start");
        };
        assert_eq!(
            start.environment["ANTHROPIC_AUTH_TOKEN"],
            "fake-gateway-key-0451"
        );
        assert_eq!(
            fixture.last_route("c-plain"),
            None,
            "one handler has no route to name"
        );
        let held = fixture.state.intake.accounts().unwrap();
        assert_eq!(held[0].active.len(), 1, "her harness holds it");
        fixture.closed("c-plain").await;
        assert!(
            fixture.state.intake.accounts().unwrap()[0]
                .active
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_harness_record_is_filed_as_the_conversation_s_context_after_each_turn() {
        let fixture = fixture(true, IDLE);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider-assistant-1.jsonl");
        std::fs::write(&path, "{\"type\":\"user\"}\n{\"type\":\"assistant\"}\n").unwrap();
        *fixture.record.lock().unwrap() = Some(SessionRecordFile {
            path: path.to_string_lossy().into_owned(),
            format: "claude-code-jsonl".to_owned(),
        });
        fixture
            .desk
            .handle(job("c-ctx", "Which Stations are busy?"))
            .await;
        until(|| kinds(&fixture, "c-ctx").contains(&EVENT_COMPLETED.to_owned())).await;
        let registry = fixture.desk.state.intake.clone();
        let context = registry
            .session_context("provider-assistant-1")
            .unwrap()
            .expect("filed at the turn's end");
        assert_eq!(context.conversation(), Some("c-ctx"));
        assert_eq!(context.source, ContextSource::Harness);
        assert_eq!(context.format, "claude-code-jsonl");
        assert_eq!(context.handler, "mock-assistant");
        assert!(context.content.contains("\"assistant\""));
        assert_eq!(context.cut_from, None);
        let asked = fixture.asked.lock().unwrap().records[0].clone();
        assert_eq!(asked.session_id, "provider-assistant-1");
        assert!(asked.environment.contains_key("CLAUDE_CONFIG_DIR"));

        // A record past the cap keeps its newest lines and says how large it was.
        let cap = registry.limits().contexts.context_bytes.or_max();
        let line = format!("{{\"pad\":\"{}\"}}\n", "x".repeat(1_000));
        let mut long = line.repeat(cap / line.len() + 2);
        long.push_str("{\"type\":\"last\"}\n");
        std::fs::write(&path, &long).unwrap();
        fixture.desk.handle(job("c-ctx", "And now?")).await;
        until(|| {
            registry
                .session_context("provider-assistant-1")
                .unwrap()
                .is_some_and(|context| context.cut_from.is_some())
        })
        .await;
        let cut = registry
            .session_context("provider-assistant-1")
            .unwrap()
            .unwrap();
        assert_eq!(cut.cut_from, Some(long.len() as u64));
        assert!(cut.content.len() <= cap);
        assert!(cut.content.ends_with("{\"type\":\"last\"}\n"));
        assert!(cut.content.starts_with("{\"pad\""), "from a line start");
    }

    #[tokio::test]
    async fn a_harness_with_no_record_has_its_events_filed_and_said_so() {
        let fixture = fixture(true, IDLE);
        fixture
            .desk
            .handle(job("c-acp", "Which Stations are busy?"))
            .await;
        until(|| kinds(&fixture, "c-acp").contains(&EVENT_COMPLETED.to_owned())).await;
        let context = fixture
            .desk
            .state
            .intake
            .session_context("provider-assistant-1")
            .unwrap()
            .expect("filed at the turn's end");
        assert_eq!(context.source, ContextSource::Events);
        assert_eq!(context.format, CONTEXT_EVENTS_FORMAT);
        let kinds: Vec<String> = context
            .content
            .lines()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line).unwrap()["kind"].to_string()
            })
            .collect();
        assert!(kinds.contains(&"\"completed\"".to_owned()), "{kinds:?}");
    }

    #[tokio::test]
    async fn a_message_starts_a_restricted_harness_and_the_next_one_reaches_it_warm() {
        let fixture = fixture(true, IDLE);
        fixture
            .desk
            .handle(job("c-1", "Which Stations are busy?"))
            .await;
        until(|| kinds(&fixture, "c-1").contains(&EVENT_COMPLETED.to_owned())).await;
        assert_eq!(
            kinds(&fixture, "c-1"),
            [EVENT_ACCEPTED, EVENT_TEXT, EVENT_COMPLETED],
            "the harness's own bookkeeping is not shown"
        );
        let start = fixture.asked.lock().unwrap().starts[0].clone();
        assert_eq!(
            start.assistant,
            Some(AssistantMode::moneypenny()),
            "only Moneypenny's MCP server"
        );
        assert!(!start.answer_only && !start.project_settings && !start.project_mcp);
        assert_eq!(start.clearance, "desk");
        assert_eq!(
            (start.model.as_deref(), start.effort.as_deref()),
            (Some("sonnet"), Some("low"))
        );
        assert_eq!(
            start.initial_context,
            [
                "You serve the operator.",
                "You are Moneypenny.",
                "Earlier in this conversation: hello."
            ],
            "a harness started afresh gets the recap"
        );
        assert_eq!(
            start
                .environment
                .get(ASSISTANT_MCP_URL_ENV)
                .map(String::as_str),
            Some("https://exports.example/v1/moneypenny/mcp")
        );
        assert!(start.environment.contains_key(ASSISTANT_MCP_TOKEN_ENV));
        assert!(start.environment.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(
            !start.environment.contains_key("LOADOUT_SECRET"),
            "nothing else of the Agent's environment"
        );
        assert_eq!(
            start.environment.get("PATH").map(String::as_str),
            Some("/opt/moneypenny/bin"),
            "her harness finds its program on what her loadout sets"
        );
        assert!(
            !start.environment.contains_key("LOADOUT_TOOL_MODE"),
            "of her loadout's env too, only what reaches the model"
        );
        let area = Path::new(&start.working_directory);
        assert!(area.is_dir());
        assert_eq!(
            std::fs::read_dir(area).unwrap().count(),
            0,
            "it starts empty"
        );

        fixture.desk.handle(job("c-1", "And now?")).await;
        until(|| {
            kinds(&fixture, "c-1")
                .iter()
                .filter(|kind| *kind == EVENT_COMPLETED)
                .count()
                == 2
        })
        .await;
        {
            let asked = fixture.asked.lock().unwrap();
            assert_eq!(asked.starts.len(), 1, "the running harness took it");
            assert_eq!(asked.messages, ["And now?"]);
        }
        let session_ids = fixture
            .reports
            .lock()
            .unwrap()
            .iter()
            .filter(|report| report.job_id.is_none())
            .all(|report| report.session_id.as_deref() == Some("provider-assistant-1"));
        assert!(session_ids);
        fixture.desk.close_all();
        until(|| kinds(&fixture, "c-1").contains(&EVENT_CLOSED.to_owned())).await;
        until(|| !area.exists()).await;
    }

    #[tokio::test]
    async fn a_station_running_missions_on_its_own_host_refuses_her_unless_consented() {
        let fixture = fixture(true, IDLE);
        let mut desk = Desk::new(
            fixture.desk.state.clone(),
            fixture.desk.harness.clone(),
            Arc::new(|_, _| {}),
            "https://unused.example/mcp".to_owned(),
        );
        assert!(
            (desk.beside_missions)(),
            "by default a Station has the execution role and a local Theater"
        );
        desk = desk.with_beside_missions(true);
        let refused = desk.check_beside_missions().unwrap_err();
        assert!(
            refused.contains("read her tokens")
                && refused.contains("assistant.allow_beside_local_missions"),
            "{refused}"
        );

        let mut control = fixture.desk.state.clone();
        control.station = crate::station::server::StationConfig::default()
            .with_roles([crate::station::server::StationRole::Control]);
        assert!(!runs_local_missions(&control), "no Mission is placed here");

        let consent = format!("{AGENTS}[assistant]\nallow_beside_local_missions = true\n");
        let allowed = on_station(
            &consent,
            Ok(false),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );
        assert!(allowed.desk.check_beside_missions().is_ok());
        let roster = allowed.desk.state.current().roster.clone();
        assert!(
            weakenings(&roster)
                .iter()
                .any(|line| line.contains("assistant.allow_beside_local_missions")),
            "the check-in names it as a weakening"
        );
        allowed.desk.close_all();
    }

    #[tokio::test]
    async fn a_refused_job_starts_no_harness_and_says_why() {
        let mut fixture = fixture(true, IDLE);
        fixture.desk_mut().beside_missions = Arc::new(|| true);
        fixture.desk.handle(job("c-beside", "hello")).await;
        let rejected = fixture
            .reports
            .lock()
            .unwrap()
            .iter()
            .flat_map(|report| report.events.iter())
            .find(|event| event.kind == EVENT_REJECTED)
            .and_then(|event| event.text.clone())
            .expect("the person is told");
        assert!(rejected.contains("read her tokens"), "{rejected}");
        assert!(fixture.asked.lock().unwrap().starts.is_empty());
    }

    #[tokio::test]
    async fn an_idle_harness_closes_and_the_next_message_resumes_its_provider_session() {
        let fixture = fixture(true, Duration::from_millis(50));
        fixture.desk.handle(job("c-2", "first")).await;
        until(|| kinds(&fixture, "c-2").contains(&EVENT_CLOSED.to_owned())).await;
        assert!(fixture.desk.live.lock().unwrap().is_empty());
        fixture.desk.handle(job("c-2", "second")).await;
        until(|| {
            kinds(&fixture, "c-2")
                .iter()
                .filter(|kind| *kind == EVENT_COMPLETED)
                .count()
                == 2
        })
        .await;
        let asked = fixture.asked.lock().unwrap();
        assert_eq!(asked.starts.len(), 1);
        assert_eq!(asked.resumes.len(), 1);
        let resume = &asked.resumes[0];
        assert_eq!(resume.session_id.as_str(), "provider-assistant-1");
        assert_eq!(resume.message.as_deref(), Some("second"));
        assert_eq!(resume.assistant, Some(AssistantMode::moneypenny()));
        assert_eq!(
            resume.working_directory, asked.starts[0].working_directory,
            "Claude Code resumes a Session only from the directory that recorded it"
        );
        assert_eq!(
            resume.initial_context,
            ["You serve the operator.", "You are Moneypenny."],
            "the instructions come again, never the recap of a conversation it has"
        );
        assert!(Path::new(&resume.working_directory).is_dir());
    }

    #[tokio::test]
    async fn each_conversation_has_its_own_directory() {
        let fixture = fixture(true, IDLE);
        fixture.desk.handle(job("c-a", "one")).await;
        fixture.desk.handle(job("c-b", "two")).await;
        until(|| fixture.asked.lock().unwrap().starts.len() == 2).await;
        let asked = fixture.asked.lock().unwrap();
        let (a, b) = (
            Path::new(&asked.starts[0].working_directory),
            Path::new(&asked.starts[1].working_directory),
        );
        assert_ne!(a, b);
        assert_eq!(a.parent(), b.parent());
        drop(asked);
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn a_turn_the_agent_cancels_ends_and_the_next_message_reaches_it_warm() {
        let fixture = scripted(true, IDLE, Script::CancelsTurn, Script::CancelsTurn);
        fixture.desk.handle(job("c-1", "Run this")).await;
        until(|| kinds(&fixture, "c-1").contains(&EVENT_COMPLETED.to_owned())).await;
        assert_eq!(
            kinds(&fixture, "c-1"),
            [EVENT_ACCEPTED, EVENT_NOTICE, EVENT_COMPLETED]
        );
        fixture.desk.handle(job("c-1", "And now?")).await;
        assert_eq!(fixture.asked.lock().unwrap().messages, ["And now?"]);
        assert_eq!(fixture.asked.lock().unwrap().starts.len(), 1);
    }

    #[tokio::test]
    async fn a_refused_tool_is_told_to_the_person_since_the_answer_may_be_empty() {
        let fixture = scripted(true, IDLE, Script::Refused, Script::Refused);
        fixture.desk.handle(job("c-1", "Read that")).await;
        until(|| kinds(&fixture, "c-1").contains(&EVENT_COMPLETED.to_owned())).await;
        assert_eq!(
            kinds(&fixture, "c-1"),
            [EVENT_ACCEPTED, EVENT_NOTICE, EVENT_COMPLETED]
        );
        let reports = fixture.reports.lock().unwrap();
        let notice = reports
            .iter()
            .flat_map(|report| report.events.iter())
            .find(|event| event.kind == EVENT_NOTICE)
            .and_then(|event| event.text.clone())
            .unwrap();
        assert!(notice.contains("refused read_file"), "{notice}");
    }

    #[tokio::test]
    async fn a_message_reaches_a_harness_whose_output_is_being_waited_on() {
        let fixture = scripted(true, IDLE, Script::ReadsLong, Script::ReadsLong);
        fixture.desk.handle(job("c-1", "First")).await;
        let completed = |want: usize| {
            kinds(&fixture, "c-1")
                .iter()
                .filter(|kind| *kind == EVENT_COMPLETED)
                .count()
                == want
        };
        until(|| completed(1)).await;
        fixture.desk.handle(job("c-1", "Second")).await;
        until(|| completed(2)).await;
        assert!(
            !fixture.asked.lock().unwrap().read_ran_out,
            "the message waited for the read to give up"
        );
    }

    #[test]
    fn a_resumed_harness_gets_its_agents_handler_settings() {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(
            root.path(),
            "",
            "[agent.moneypenny]\nhandler = \"mock-denied\"\n\
             [agent.moneypenny.handler_settings]\ncommand = \"the-agent\"\n",
        );
        let settings = state
            .roster
            .agent(None, "moneypenny")
            .unwrap()
            .handler_settings;
        let session = StationHarness(state)
            .resume(
                "mock-denied",
                &settings,
                AgentResume {
                    mission_id: MissionId::try_new("assistant--a").unwrap(),
                    session_id: SessionId::try_new("provider-session").unwrap(),
                    message: None,
                    working_directory: root.path().to_string_lossy().into_owned(),
                    clearance: "desk".to_owned(),
                    environment: BTreeMap::new(),
                    model: None,
                    effort: None,
                    max_cost_micro_usd: None,
                    project_settings: false,
                    project_mcp: false,
                    answer_only: false,
                    assistant: None,
                    initial_context: Vec::new(),
                    overlays: Default::default(),
                },
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let told = loop {
            assert!(Instant::now() < deadline, "the handler said nothing");
            if let Some(event) = session.next_event(100).unwrap() {
                break event;
            }
        };
        assert_eq!(told.kind, "notice");
        assert!(told.text.unwrap().contains("the-agent"));
    }

    #[tokio::test]
    async fn a_session_that_cannot_resume_starts_afresh_and_bad_jobs_are_rejected() {
        let fixture = fixture(false, IDLE);
        let mut resumed = job("c-3", "hello again");
        resumed.session_id = Some("gone".to_owned());
        fixture.desk.handle(resumed).await;
        until(|| kinds(&fixture, "c-3").contains(&EVENT_COMPLETED.to_owned())).await;
        assert_eq!(fixture.asked.lock().unwrap().starts.len(), 1);

        let mut stranger = job("c-4", "hi");
        stranger.agent_id = "nobody".to_owned();
        fixture.desk.handle(stranger).await;
        let mut wrong_handler = job("c-5", "hi");
        wrong_handler.agent_id = "007".to_owned();
        fixture.desk.handle(wrong_handler).await;
        let mut bad = job("c-6", "hi");
        bad.mcp_token = "short".to_owned();
        fixture.desk.handle(bad).await;
        fixture
            .desk
            .handle(job("c-7", "fills the second slot"))
            .await;
        fixture.desk.handle(job("c-8", "one too many")).await;
        for conversation in ["c-4", "c-5", "c-6", "c-8"] {
            assert_eq!(
                kinds(&fixture, conversation),
                [EVENT_REJECTED],
                "{conversation}"
            );
        }
        let reports = fixture.reports.lock().unwrap();
        let reason = |conversation: &str| {
            reports
                .iter()
                .find(|report| report.conversation_id == conversation)
                .and_then(|report| report.events[0].text.clone())
                .unwrap()
        };
        assert!(reason("c-5").contains("mock-agent"), "{}", reason("c-5"));
        assert!(reason("c-8").contains("as many"), "{}", reason("c-8"));
        drop(reports);
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn the_rosters_assistant_limits_bound_each_job_and_report() {
        let agents = format!(
            "{AGENTS}[assistant.limits]
message_bytes = 8
event_bytes = 7
"
        );
        let fixture = on_station(
            &agents,
            Ok(false),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );
        fixture.desk.handle(job("c-long", "123456789")).await;
        assert_eq!(kinds(&fixture, "c-long"), [EVENT_REJECTED]);
        fixture.desk.handle(job("c-short", "12345678")).await;
        until(|| kinds(&fixture, "c-short").contains(&EVENT_COMPLETED.to_owned())).await;
        let reports = fixture.reports.lock().unwrap();
        let texts: Vec<&str> = reports
            .iter()
            .flat_map(|report| report.events.iter())
            .filter_map(|event| event.text.as_deref())
            .collect();
        assert!(texts.contains(&"message"), "the refusal, cut: {texts:?}");
        assert!(texts.iter().all(|text| text.len() <= 7), "{texts:?}");
        drop(reports);
        fixture.desk.close_all();
    }

    #[test]
    fn empty_assistant_polls_are_paced_even_when_the_gateway_answers_immediately() {
        const PACE: Duration = Duration::from_millis(400);

        use std::io::{BufRead, BufReader, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let served = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut polls = Vec::new();
            while polls.len() < 2 && Instant::now() < deadline {
                let (stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                polls.push(Instant::now());
                if polls.len() == 2 {
                    stop.store(true, Ordering::Release);
                }
                write!(
                    reader.get_mut(),
                    "HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n"
                )
                .unwrap();
            }
            stop.store(true, Ordering::Release);
            polls
        });
        let link = Link {
            origin,
            token: "station-token".to_owned(),
            station: "hq".to_owned(),
            tls: TlsConfig::default(),
            tables: Arc::new(|| {
                let mut desk = AssistantDeskLimits::default();
                // Past the server's deadline: pacing by this would lose the second poll.
                desk.job_wait.0 = Duration::from_secs(30);
                desk.job_retry.0 = PACE;
                (AssistantLimits::default(), desk)
            }),
        };
        let (jobs, _receiver) = mpsc::channel(1);
        let stop = stopping.clone();
        let began = Instant::now();
        let polling = std::thread::spawn(move || link.take_jobs(&stop, &jobs));
        let polls = served.join();
        stopping.store(true, Ordering::Release);
        polling.join().unwrap();
        let polls = polls.unwrap();
        assert_eq!(polls.len(), 2);
        // Pacing runs from a poll's start, so a late first arrival would shrink the gap between
        // arrivals on a loaded machine; the second can never arrive sooner than this after the start.
        assert!(polls[1].duration_since(began) >= PACE);
    }

    /// A gateway that answers every report with `status` and counts them.
    fn refusing_gateway(status: u16) -> (Url, std::thread::JoinHandle<usize>) {
        use std::io::{BufRead, BufReader, Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let served = std::thread::spawn(move || {
            let mut answered = 0;
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let reason = if status == 409 { "Conflict" } else { "OK" };
                write!(
                    reader.get_mut(),
                    "HTTP/1.1 {status} {reason}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                )
                .unwrap();
                answered += 1;
            }
            answered
        });
        (origin, served)
    }

    #[test]
    fn the_desk_reaches_an_enterprise_gateway_only_with_its_ca() {
        use crate::station::publisher::tls_gateway;
        use crate::tls::trusting;

        let gateway = tls_gateway::start("127.0.0.1");
        let link = |tls| Link {
            origin: Url::parse(&gateway.origin).unwrap(),
            token: "station-token".to_owned(),
            tls,
            station: "hq".to_owned(),
            tables: Arc::new(Default::default),
        };
        let asked = |link: Link| link.finish_page("c", "d", Ok(Vec::new()));
        assert!(asked(link(TlsConfig::default())).is_err());
        assert_eq!(
            gateway
                .paths
                .recv_timeout(std::time::Duration::from_millis(200))
                .ok(),
            None,
            "no request crosses a handshake the desk does not trust"
        );
        assert!(asked(link(trusting(&gateway.ca))).is_err(), "a 503");
        assert!(
            gateway
                .paths
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .starts_with("/v1/station/assistant/")
        );
    }

    #[test]
    fn the_link_tells_a_job_answer_whether_the_gateway_refused_it() {
        for (status, delivery) in [
            (409, Delivery::Refused),
            (204, Delivery::Taken),
            (500, Delivery::Lost),
        ] {
            let (origin, served) = refusing_gateway(status);
            let link = Link {
                origin,
                token: "station-token".to_owned(),
                tls: TlsConfig::default(),
                station: "hq".to_owned(),
                tables: Arc::new(Default::default),
            };
            let (reports, queued) = std::sync::mpsc::sync_channel(4);
            let report = |job: Option<&str>, kind: &str| AssistantReport {
                route: None,
                run: None,
                station_id: "hq".to_owned(),
                conversation_id: "c-late".to_owned(),
                job_id: job.map(str::to_owned),
                session_id: None,
                events: vec![AssistantEvent::new(kind, None)],
            };
            let (answered, mut told) = tokio::sync::oneshot::channel();
            reports
                .send((report(Some("job-1"), EVENT_ACCEPTED), Some(answered)))
                .unwrap();
            reports.send((report(None, EVENT_TEXT), None)).unwrap();
            drop(reports);
            let ordinary = AtomicUsize::new(1);
            link.send_reports(&queued, &ordinary);
            assert_eq!(served.join().unwrap(), 2);
            assert_eq!(told.try_recv(), Ok(delivery), "at {status}");
            assert_eq!(
                ordinary.load(Ordering::Acquire),
                0,
                "the ordinary one was counted out"
            );
        }
    }

    /// Makes the fixture's gateway take a job's answer only once its conversation holds
    /// `holding` reports back (or two seconds passed), refusing an `accepted` when `refuse`;
    /// each answer is recorded with how many were held when it arrived.
    fn slow_gateway(
        fixture: &mut Fixture,
        holding: usize,
        refuse: bool,
    ) -> Arc<Mutex<Vec<(AssistantReport, usize)>>> {
        let sink = fixture.reports.clone();
        let answers = Arc::new(Mutex::new(Vec::new()));
        let seen = answers.clone();
        let held: Arc<Mutex<BTreeMap<String, Held>>> = Arc::default();
        let waiting = held.clone();
        let report: Reporter = Arc::new(move |report, answered: Option<Answered>| {
            let Some(answered) = answered else {
                sink.lock().unwrap().push(report);
                return;
            };
            let (sink, seen, waiting) = (sink.clone(), seen.clone(), waiting.clone());
            std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(2);
                let held = loop {
                    let held = match waiting.lock().unwrap().get(&report.conversation_id) {
                        Some(Held::Waiting(held)) => held.len(),
                        _ => 0,
                    };
                    if held >= holding || Instant::now() >= deadline {
                        break held;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                };
                let accepted = report.events[0].kind == EVENT_ACCEPTED;
                seen.lock().unwrap().push((report.clone(), held));
                sink.lock().unwrap().push(report);
                let _ = answered.send(if refuse && accepted {
                    Delivery::Refused
                } else {
                    Delivery::Taken
                });
            });
        });
        fixture.desk_mut().outbox = Outbox {
            report,
            held,
            tables: Arc::new(Default::default),
        };
        answers
    }

    #[tokio::test]
    async fn no_event_of_a_turn_is_reported_before_its_slow_accepted() {
        let mut fixture = fixture(true, IDLE);
        // Each turn reports `text` and `completed`.
        let answers = slow_gateway(&mut fixture, 2, false);
        fixture.desk.handle(job("c-slow", "first")).await;
        assert_eq!(
            kinds(&fixture, "c-slow").first().map(String::as_str),
            Some(EVENT_ACCEPTED),
            "the desk waits for the gateway to take its answer"
        );
        until(|| kinds(&fixture, "c-slow").contains(&EVENT_COMPLETED.to_owned())).await;
        // The warm harness's turn too.
        fixture.desk.handle(job("c-slow", "second")).await;
        until(|| {
            kinds(&fixture, "c-slow")
                .iter()
                .filter(|kind| *kind == EVENT_COMPLETED)
                .count()
                == 2
        })
        .await;
        assert_eq!(
            kinds(&fixture, "c-slow"),
            [
                EVENT_ACCEPTED,
                EVENT_TEXT,
                EVENT_COMPLETED,
                EVENT_ACCEPTED,
                EVENT_TEXT,
                EVENT_COMPLETED
            ]
        );
        let held: Vec<usize> = answers.lock().unwrap().iter().map(|(_, n)| *n).collect();
        assert_eq!(
            held,
            [2, 2],
            "each turn was over while its accepted was on its way"
        );
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn a_turn_whose_accepted_is_refused_reports_nothing_and_its_harness_closes() {
        let mut fixture = fixture(true, IDLE);
        let answers = slow_gateway(&mut fixture, 2, true);
        fixture.desk.handle(job("c-moved", "hello")).await;
        until(|| fixture.desk.live.lock().unwrap().is_empty()).await;
        until(|| fixture.desk.outbox.held.lock().unwrap().is_empty()).await;
        assert_eq!(
            kinds(&fixture, "c-moved"),
            [EVENT_ACCEPTED],
            "neither the turn nor the harness's closing"
        );
        assert_eq!(answers.lock().unwrap()[0].1, 2, "the turn was held back");
        assert_eq!(fixture.asked.lock().unwrap().starts.len(), 1);
    }

    /// An outbox whose link answers every job answer `refused`, and the reports it sent.
    fn refusing_outbox() -> (Outbox, Arc<Mutex<Vec<AssistantReport>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let seen = sent.clone();
        let outbox = Outbox::new(Arc::new(
            move |report: AssistantReport, answered: Option<Answered>| match answered {
                Some(answered) => {
                    let _ = answered.send(Delivery::Refused);
                }
                None => seen.lock().unwrap().push(report),
            },
        ));
        (outbox, sent)
    }

    fn reported(conversation: &str, kinds: &[&str]) -> AssistantReport {
        AssistantReport {
            route: None,
            run: None,
            station_id: "test".to_owned(),
            conversation_id: conversation.to_owned(),
            job_id: None,
            session_id: None,
            events: kinds
                .iter()
                .map(|kind| AssistantEvent::new(kind, None))
                .collect(),
        }
    }

    #[tokio::test]
    async fn a_refused_accept_still_reports_the_cables_its_harness_sent() {
        let (outbox, sent) = refusing_outbox();
        outbox.hold("c-late");
        outbox.send(reported("c-late", &[EVENT_TEXT, EVENT_CABLE_SENT]));
        outbox.send(reported("c-late", &[EVENT_TEXT]));
        let accepted = AssistantReport {
            job_id: Some("job-late".to_owned()),
            ..reported("c-late", &[EVENT_ACCEPTED])
        };
        assert!(outbox.answer(accepted, true).await, "refused");
        outbox.send(reported("c-late", &[EVENT_CABLE_SENT, EVENT_CLOSED]));
        outbox.send(reported("c-late", &[EVENT_COMPLETED]));
        let kinds: Vec<Vec<String>> = sent
            .lock()
            .unwrap()
            .iter()
            .map(|report| {
                report
                    .events
                    .iter()
                    .map(|event| event.kind.clone())
                    .collect()
            })
            .collect();
        assert_eq!(
            kinds,
            [[EVENT_CABLE_SENT.to_owned()], [EVENT_CABLE_SENT.to_owned()]]
        );
    }

    /// An outbox whose link answers every report `Taken`, and the reports it sent.
    fn taking_outbox() -> (Outbox, Arc<Mutex<Vec<AssistantReport>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let seen = sent.clone();
        let outbox = Outbox::new(Arc::new(
            move |report: AssistantReport, answered: Option<Answered>| {
                seen.lock().unwrap().push(report);
                if let Some(answered) = answered {
                    let _ = answered.send(Delivery::Taken);
                }
            },
        ));
        (outbox, sent)
    }

    fn kinds_sent(sent: &Mutex<Vec<AssistantReport>>, kind: &str) -> usize {
        sent.lock()
            .unwrap()
            .iter()
            .filter(|report| report.events.iter().any(|event| event.kind == kind))
            .count()
    }

    #[tokio::test]
    async fn a_full_queue_drops_no_cable_or_answer() {
        let (outbox, sent) = taking_outbox();
        outbox.hold("c-full");
        for _ in 0..REPORT_QUEUE + 10 {
            outbox.send(reported("c-full", &[EVENT_TEXT]));
        }
        outbox.send(reported("c-full", &[EVENT_CABLE_PROPOSAL]));
        outbox.send(reported("c-full", &[EVENT_CABLE_SENT]));
        let accepted = AssistantReport {
            job_id: Some("job-full".to_owned()),
            ..reported("c-full", &[EVENT_ACCEPTED])
        };
        assert!(!outbox.answer(accepted, true).await);
        assert_eq!(
            kinds_sent(&sent, EVENT_TEXT),
            REPORT_QUEUE,
            "ordinary ones are bounded"
        );
        assert_eq!(kinds_sent(&sent, EVENT_CABLE_PROPOSAL), 1);
        assert_eq!(kinds_sent(&sent, EVENT_CABLE_SENT), 1);
    }

    #[tokio::test]
    async fn a_warm_harness_restarts_when_the_job_carries_a_new_token() {
        let fixture = fixture(true, IDLE);
        // A long way from the margin either way: only the token itself differs.
        fixture.desk.handle(job("c-t", "first")).await;
        until(|| kinds(&fixture, "c-t").contains(&EVENT_COMPLETED.to_owned())).await;
        fixture.desk.handle(job("c-t", "second")).await;
        until(|| fixture.asked.lock().unwrap().messages.len() == 1).await;
        assert!(
            fixture.asked.lock().unwrap().resumes.is_empty(),
            "the same token reaches it warm"
        );

        let mut third = job("c-t", "third");
        third.mcp_token = "fresh-conversation-token-".repeat(2);
        fixture.desk.handle(third).await;
        until(|| fixture.asked.lock().unwrap().resumes.len() == 1).await;
        let asked = fixture.asked.lock().unwrap();
        assert_eq!(
            asked.resumes[0]
                .environment
                .get(ASSISTANT_MCP_TOKEN_ENV)
                .map(String::as_str),
            Some("fresh-conversation-token-fresh-conversation-token-")
        );
        assert_eq!(asked.messages.len(), 1, "not sent to the old harness");
        drop(asked);
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn a_token_about_to_expire_restarts_the_harness_with_the_new_one() {
        let fixture = fixture(true, IDLE);
        let mut first = job("c-9", "first");
        first.token_expires_at = unix_now() + 30.0;
        fixture.desk.handle(first).await;
        until(|| kinds(&fixture, "c-9").contains(&EVENT_COMPLETED.to_owned())).await;
        let mut second = job("c-9", "second");
        second.mcp_token = "fresh-conversation-token-".repeat(2);
        fixture.desk.handle(second).await;
        until(|| fixture.asked.lock().unwrap().resumes.len() == 1).await;
        let asked = fixture.asked.lock().unwrap();
        assert_eq!(
            asked.resumes[0]
                .environment
                .get(ASSISTANT_MCP_TOKEN_ENV)
                .map(String::as_str),
            Some("fresh-conversation-token-fresh-conversation-token-")
        );
        assert!(asked.messages.is_empty(), "not sent to the old harness");
        drop(asked);
        fixture.desk.close_all();
    }

    /// The text of the conversation's first event of `kind`.
    fn text_of(fixture: &Fixture, conversation: &str, kind: &str) -> Option<String> {
        fixture
            .reports
            .lock()
            .unwrap()
            .iter()
            .filter(|report| report.conversation_id == conversation)
            .flat_map(|report| report.events.iter())
            .find(|event| event.kind == kind)
            .and_then(|event| event.text.clone())
    }

    /// `failed` comes before `closed`, and nothing else but `accepted` is reported.
    fn failed_then_closed(fixture: &Fixture, conversation: &str) {
        let mut kinds = kinds(fixture, conversation);
        kinds.retain(|kind| kind != EVENT_ACCEPTED);
        assert_eq!(kinds, [EVENT_FAILED, EVENT_CLOSED]);
    }

    #[tokio::test]
    async fn a_harness_that_dies_is_reported_with_its_reason_and_frees_its_slot() {
        let fixture = scripted(true, IDLE, Script::Dies, Script::Answers);
        fixture.desk.handle(job("c-dead", "hello")).await;
        until(|| kinds(&fixture, "c-dead").contains(&EVENT_CLOSED.to_owned())).await;
        failed_then_closed(&fixture, "c-dead");
        let reason = text_of(&fixture, "c-dead", EVENT_FAILED).unwrap();
        assert!(reason.contains("the stored login has expired"), "{reason}");
        assert!(
            fixture.desk.live.lock().unwrap().is_empty(),
            "its slot is free"
        );
        let area = fixture.asked.lock().unwrap().starts[0]
            .working_directory
            .clone();
        until(|| !Path::new(&area).exists()).await;
    }

    #[tokio::test]
    async fn a_busy_harness_that_writes_nothing_is_closed_and_the_conversation_told() {
        let fixture = scripted(
            true,
            Duration::from_millis(50),
            Script::Hangs,
            Script::Answers,
        );
        fixture.desk.handle(job("c-hung", "hello")).await;
        until(|| kinds(&fixture, "c-hung").contains(&EVENT_CLOSED.to_owned())).await;
        failed_then_closed(&fixture, "c-hung");
        let reason = text_of(&fixture, "c-hung", EVENT_FAILED).unwrap();
        assert!(reason.contains("wrote nothing"), "{reason}");
        assert!(fixture.desk.live.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_interrupt_stops_the_answer_through_the_handler_and_keeps_the_harness() {
        let fixture = scripted(true, IDLE, Script::Works, Script::Answers);
        fixture.desk.handle(interruption("c-stop", "early")).await;
        assert_eq!(
            kinds(&fixture, "c-stop"),
            [EVENT_ACCEPTED],
            "with no harness running it is a no-op"
        );

        fixture.desk.handle(job("c-stop", "a long answer")).await;
        until(|| kinds(&fixture, "c-stop").contains(&EVENT_TEXT.to_owned())).await;
        fixture.desk.handle(interruption("c-stop", "now")).await;
        until(|| kinds(&fixture, "c-stop").contains(&EVENT_INTERRUPTED.to_owned())).await;
        assert_eq!(fixture.asked.lock().unwrap().interrupts, 1);
        let mut reported = kinds(&fixture, "c-stop");
        reported.retain(|kind| kind != EVENT_ACCEPTED);
        assert_eq!(
            reported,
            [EVENT_TEXT, EVENT_INTERRUPTED],
            "the turn the handler ended is reported as interrupted, not completed"
        );

        // The harness is idle now: another interrupt does nothing, and it takes the next
        // message itself.
        fixture.desk.handle(interruption("c-stop", "again")).await;
        fixture.desk.handle(job("c-stop", "next")).await;
        until(|| fixture.asked.lock().unwrap().messages == ["next"]).await;
        let asked = fixture.asked.lock().unwrap();
        assert_eq!(asked.interrupts, 1, "an idle harness is not interrupted");
        assert_eq!(asked.starts.len(), 1, "the harness kept running");
        drop(asked);
        assert!(!kinds(&fixture, "c-stop").contains(&EVENT_CLOSED.to_owned()));
        fixture.desk.close_all();
    }

    /// Claude Code reads the message before it says whether the Session it was asked to
    /// resume exists, so a resume can fail after the harness started.
    #[tokio::test]
    async fn a_resumed_harness_that_fails_before_confirming_its_session_starts_afresh() {
        let fixture = scripted(true, IDLE, Script::Answers, Script::Dies);
        let mut resumed = job("c-gone", "hello again");
        resumed.session_id = Some("gone".to_owned());
        fixture.desk.handle(resumed).await;
        until(|| kinds(&fixture, "c-gone").contains(&EVENT_COMPLETED.to_owned())).await;
        assert!(
            !kinds(&fixture, "c-gone").contains(&EVENT_FAILED.to_owned()),
            "the failed resume is the Station's to report, not the conversation's"
        );
        let asked = fixture.asked.lock().unwrap();
        assert_eq!((asked.resumes.len(), asked.starts.len()), (1, 1));
        assert_eq!(asked.starts[0].prompt, "hello again");
        assert!(
            asked.starts[0]
                .initial_context
                .contains(&"Earlier in this conversation: hello.".to_owned()),
            "started afresh with the recap"
        );
        drop(asked);
        fixture.desk.close_all();
    }

    #[cfg(feature = "cables-mcp")]
    /// A desk whose harnesses close after `idle` on a Station where `moneypenny` may send to `008`.
    #[cfg(feature = "cables-mcp")]
    fn cable_fixture(idle: Duration) -> Fixture {
        use crate::directorates::registry::cables::{
            CableAction, CableEndpoint, CableGrant, CablePolicy, CableRelay, CableScope,
            CableTarget,
        };

        let mut fixture = fixture(true, idle);
        let registry = fixture.desk.state.intake.clone();
        let station = CableTarget::try_new(CableScope::Station, Some("test".to_owned())).unwrap();
        let grants = ["moneypenny", "008"]
            .into_iter()
            .flat_map(|agent| {
                [CableAction::Send, CableAction::Receive].map(|action| {
                    CableGrant::try_new(agent.to_owned(), action, station.clone()).unwrap()
                })
            })
            .collect();
        let audit = registry.clone();
        fixture.desk_mut().state.cables = Arc::new(
            CableRelay::new(
                registry.clone(),
                CablePolicy::try_new(grants, BTreeMap::new()).unwrap(),
                || {
                    vec![
                        CableEndpoint::try_new(
                            "008".to_owned(),
                            "G".to_owned(),
                            None,
                            "test".to_owned(),
                            Some("session-8".to_owned()),
                            Some("mission-1".to_owned()),
                        )
                        .unwrap(),
                    ]
                },
                BTreeMap::new(),
            )
            .with_durable_audit(move |record| {
                super::super::write_cable_audit(audit.as_ref(), "test", record)
            }),
        );

        fixture
    }

    /// The harness of conversation `conversation` holds one Cable for the person, as Claude Code
    /// would call the tool; what the person is shown, and how they would confirm it.
    #[cfg(feature = "cables-mcp")]
    async fn hold_a_cable(fixture: &Fixture, conversation: &str, key: &str) -> AssistantJob {
        use rmcp::model::CallToolRequestParams;

        use crate::directorates::section_00::assistant::{
            ASSISTANT_CABLE_MCP_TOKEN_ENV, ASSISTANT_CABLE_MCP_URL_ENV, AssistantCableDecision,
            AssistantCableGrant,
        };

        let mut granted = job(conversation, "Ask 008 whether the build is green.");
        granted.person_id = Some("person-007".to_owned());
        granted.cables = vec![AssistantCableGrant {
            operation: "G".to_owned(),
            send: true,
            read: true,
        }];
        let launches = || {
            let asked = fixture.asked.lock().unwrap();
            asked.starts.len() + asked.resumes.len()
        };
        let before = launches();
        fixture.desk.handle(granted).await;
        until(|| launches() > before).await;
        let environment = {
            let asked = fixture.asked.lock().unwrap();
            match asked.resumes.last() {
                Some(resume) => resume.environment.clone(),
                None => asked.starts[0].environment.clone(),
            }
        };
        let client = cable_client(
            &environment[ASSISTANT_CABLE_MCP_URL_ENV],
            &environment[ASSISTANT_CABLE_MCP_TOKEN_ENV],
        )
        .await;
        let arguments = serde_json::json!({
            "operation": "G", "scope": "agent", "target": "008",
            "body": format!("Is the build green? {key}"), "idempotency_key": key
        });
        client
            .call_tool(
                CallToolRequestParams::new("send_cable")
                    .with_arguments(serde_json::from_value(arguments).unwrap()),
            )
            .await
            .unwrap();
        client.cancel().await.unwrap();
        let proposal = || {
            fixture
                .reports
                .lock()
                .unwrap()
                .iter()
                .flat_map(|report| report.events.iter())
                .filter(|event| event.kind == EVENT_CABLE_PROPOSAL)
                .filter_map(|event| event.cable.clone())
                .find(|cable| cable.body.as_deref().is_some_and(|body| body.contains(key)))
        };
        until(|| proposal().is_some()).await;
        let proposal = proposal().unwrap();
        let mut confirm = job(conversation, "unused");
        confirm.job_id = format!("confirm-{key}");
        confirm.message = String::new();
        confirm.cable_confirm = Some(AssistantCableDecision {
            id: proposal.id,
            digest: proposal.digest,
        });
        confirm
    }

    #[cfg(feature = "cables-mcp")]
    fn job_answer(fixture: &Fixture, job_id: &str) -> Option<(String, Option<String>)> {
        fixture
            .reports
            .lock()
            .unwrap()
            .iter()
            .find(|report| report.job_id.as_deref() == Some(job_id))
            .map(|report| (report.events[0].kind.clone(), report.events[0].text.clone()))
    }

    #[cfg(feature = "cables-mcp")]
    #[tokio::test]
    async fn what_she_held_goes_on_waiting_when_the_next_message_starts_her_again() {
        let fixture = cable_fixture(Duration::from_millis(100));
        let first = hold_a_cable(&fixture, "c-next", "k-first").await;
        until(|| kinds(&fixture, "c-next").contains(&EVENT_CLOSED.to_owned())).await;
        until(|| fixture.desk.live.lock().unwrap().is_empty()).await;

        // She resumes, holds a second, and both stay for the person, across the two harnesses.
        let second = hold_a_cable(&fixture, "c-next", "k-second").await;
        fixture.desk.handle(second).await;
        let answer = job_answer(&fixture, "confirm-k-second").unwrap();
        assert_eq!(answer.0, EVENT_ACCEPTED, "{answer:?}");
        fixture.desk.handle(first).await;
        let answer = job_answer(&fixture, "confirm-k-first").unwrap();
        assert_eq!(answer.0, EVENT_ACCEPTED, "{answer:?}");
        fixture.desk.close_all();
    }

    #[cfg(feature = "cables-mcp")]
    #[tokio::test]
    async fn a_cable_cancelled_after_her_harness_closed_is_gone_and_so_is_its_endpoint() {
        use crate::directorates::section_00::assistant::AssistantCableDecision;

        let fixture = cable_fixture(Duration::from_millis(100));
        let confirm = hold_a_cable(&fixture, "c-gone", "k-gone").await;
        until(|| kinds(&fixture, "c-gone").contains(&EVENT_CLOSED.to_owned())).await;
        until(|| !fixture.desk.left_held.lock().unwrap().is_empty()).await;
        let decision: &AssistantCableDecision = confirm.cable_confirm.as_ref().unwrap();
        let mut cancel = job("c-gone", "unused");
        cancel.job_id = "cancel-k-gone".to_owned();
        cancel.message = String::new();
        cancel.cable_discard = Some(decision.clone());
        fixture.desk.handle(cancel).await;
        assert!(
            fixture.desk.left_held.lock().unwrap().is_empty(),
            "nothing is kept for a conversation with nothing left to decide"
        );
        fixture.desk.handle(confirm).await;
        let answer = job_answer(&fixture, "confirm-k-gone").unwrap();
        assert_eq!(answer.0, EVENT_REJECTED);
        assert!(answer.1.unwrap().contains("nothing was sent"));
        assert!(!kinds(&fixture, "c-gone").contains(&EVENT_CABLE_SENT.to_owned()));
    }

    #[cfg(feature = "cables-mcp")]
    #[tokio::test]
    async fn a_person_confirms_a_cable_after_her_idle_harness_has_closed() {
        let fixture = cable_fixture(Duration::from_millis(100));
        let confirm = hold_a_cable(&fixture, "c-idle", "k-idle").await;
        until(|| kinds(&fixture, "c-idle").contains(&EVENT_CLOSED.to_owned())).await;
        assert!(
            fixture.desk.live.lock().unwrap().is_empty(),
            "no harness runs"
        );

        fixture.desk.handle(confirm.clone()).await;
        let answer = job_answer(&fixture, "confirm-k-idle").unwrap();
        assert_eq!(answer.0, EVENT_ACCEPTED, "{answer:?}");
        assert!(kinds(&fixture, "c-idle").contains(&EVENT_CABLE_SENT.to_owned()));
        assert_eq!(
            fixture
                .desk
                .state
                .intake
                .audit_records()
                .unwrap()
                .iter()
                .filter(|record| record.action == "cable.send" && record.actor == "person-007")
                .count(),
            1
        );

        // It went once, and the same holds with nothing left held.
        let mut again = confirm;
        again.job_id = "confirm-again".to_owned();
        fixture.desk.handle(again).await;
        let answer = job_answer(&fixture, "confirm-again").unwrap();
        assert_eq!(answer.0, EVENT_REJECTED);
        assert!(
            answer.1.unwrap().contains("sent, dropped or expired"),
            "the person is told plainly"
        );
    }

    #[tokio::test]
    async fn cable_grants_give_her_a_cable_server_that_sends_for_the_person() {
        use rmcp::model::{CallToolRequestParams, ContentBlock};

        use crate::directorates::section_00::assistant::{
            ASSISTANT_CABLE_MCP_TOKEN_ENV, ASSISTANT_CABLE_MCP_URL_ENV, AssistantCableDecision,
            AssistantCableGrant,
        };

        let fixture = cable_fixture(IDLE);
        let registry = fixture.desk.state.intake.clone();
        let mut granted = job("c-cable", "Ask 008 whether the build is green.");
        granted.person_id = Some("person-007".to_owned());
        granted.cables = vec![AssistantCableGrant {
            operation: "G".to_owned(),
            send: true,
            read: true,
        }];
        fixture.desk.handle(granted.clone()).await;
        until(|| kinds(&fixture, "c-cable").contains(&EVENT_COMPLETED.to_owned())).await;
        let start = fixture.asked.lock().unwrap().starts[0].clone();
        assert_eq!(
            start.assistant,
            Some(AssistantMode::moneypenny().with_cables()),
            "her own server and the Station's Cable server"
        );
        let url = start.environment[ASSISTANT_CABLE_MCP_URL_ENV].clone();
        let token = start.environment[ASSISTANT_CABLE_MCP_TOKEN_ENV].clone();

        // The harness calls the Cable server, as Claude Code would.
        let client = cable_client(&url, &token).await;
        let send = |operation: &str, key: &str| {
            let arguments = serde_json::json!({
                "operation": operation, "scope": "agent", "target": "008",
                "body": "Is the build green?", "idempotency_key": key
            });
            client.call_tool(
                CallToolRequestParams::new("send_cable")
                    .with_arguments(serde_json::from_value(arguments).unwrap()),
            )
        };
        let text = |result: rmcp::model::CallToolResult| match result.content.first() {
            Some(ContentBlock::Text(text)) => text.text.clone(),
            _ => panic!("the tool answers in text"),
        };
        assert_eq!(
            text(send("H", "k0").await.unwrap()),
            "Cable target is unavailable or not authorized",
            "no grant in Operation H"
        );
        // A send waits for the person, and nothing has left.
        let held = text(send("G", "k1").await.unwrap());
        assert!(
            held.starts_with("Held for the person's confirmation"),
            "{held}"
        );
        until(|| kinds(&fixture, "c-cable").contains(&EVENT_CABLE_PROPOSAL.to_owned())).await;
        assert!(!kinds(&fixture, "c-cable").contains(&EVENT_CABLE_SENT.to_owned()));
        let proposal = fixture
            .reports
            .lock()
            .unwrap()
            .iter()
            .flat_map(|report| report.events.iter())
            .find_map(|event| {
                event
                    .cable
                    .clone()
                    .filter(|_| event.kind == EVENT_CABLE_PROPOSAL)
            })
            .unwrap();
        let mut confirm = job("c-cable", "unused");
        confirm.job_id = "confirm-1".to_owned();
        confirm.message = String::new();
        confirm.cable_confirm = Some(AssistantCableDecision {
            id: proposal.id.clone(),
            digest: proposal.digest.clone(),
        });
        let answer = |job_id: &str| {
            fixture
                .reports
                .lock()
                .unwrap()
                .iter()
                .find(|report| report.job_id.as_deref() == Some(job_id))
                .map(|report| report.events[0].kind.clone())
        };
        // A confirmation past its time limit sends nothing; the Cable stays held.
        let mut late = confirm.clone();
        late.job_id = "confirm-late".to_owned();
        late.token_expires_at = unix_now() - 1.0;
        fixture.desk.handle(late).await;
        assert_eq!(answer("confirm-late").as_deref(), Some(EVENT_REJECTED));
        fixture.desk.handle(confirm.clone()).await;
        assert_eq!(answer("confirm-1").as_deref(), Some(EVENT_ACCEPTED));
        let confirmed_sends: Vec<_> = fixture
            .reports
            .lock()
            .unwrap()
            .iter()
            .flat_map(|report| report.events.iter())
            .filter(|event| event.kind == EVENT_CABLE_SENT)
            .filter_map(|event| event.cable.clone())
            .filter(|cable| cable.id == proposal.id)
            .collect();
        assert_eq!(confirmed_sends.len(), 1);
        let cable = registry
            .get_cable(confirmed_sends[0].cable_id.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(cable.on_behalf_of(), Some("person-007"));
        assert!(
            registry
                .audit_records()
                .unwrap()
                .iter()
                .any(|record| record.action == "cable.send" && record.actor == "person-007")
        );
        confirm.job_id = "confirm-2".to_owned();
        fixture.desk.handle(confirm).await;
        assert_eq!(
            answer("confirm-2").as_deref(),
            Some(EVENT_REJECTED),
            "a held Cable is sent once"
        );
        client.cancel().await.unwrap();
        fixture.desk.close_all();
    }

    #[test]
    fn a_working_directory_still_in_use_for_a_moment_is_removed_once_free() {
        let root = new_root().unwrap();
        let area = root.path.join("0123456789abcdef0123456789abcdef");
        std::fs::create_dir_all(&area).unwrap();
        #[cfg(windows)]
        let mut holder = std::process::Command::new("ping")
            .args(["-n", "2", "127.0.0.1"])
            .current_dir(&area)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        #[cfg(unix)]
        let mut holder = std::process::Command::new("sleep")
            .arg("1")
            .current_dir(&area)
            .spawn()
            .unwrap();
        remove_area(&area.to_string_lossy(), Duration::from_secs(2));
        assert!(!area.exists());
        holder.wait().unwrap();
        let path = root.path.clone();
        drop(root);
        std::fs::remove_dir_all(path).unwrap();
    }

    /// `[assistant.desk]`'s default `unlocked_stale`.
    const STALE: Duration = Duration::from_secs(3_600);

    #[test]
    fn a_starting_desk_sweeps_what_gone_desks_left_and_nothing_a_live_one_holds() {
        use fs2::FileExt;
        let temp = tempfile::tempdir().unwrap();
        let root = |name: &str| {
            let path = temp.path().join(format!("{AREA_PREFIX}{name}"));
            let area = path.join("0123456789abcdef0123456789abcdef");
            std::fs::create_dir_all(&area).unwrap();
            std::fs::write(
                path.join("0123456789abcdef0123456789abcdef.context.md"),
                "her memory",
            )
            .unwrap();
            path
        };
        let gone = root("gone");
        std::fs::File::create(root_lock(&gone)).unwrap();
        let live = root("live");
        let held = std::fs::File::create(root_lock(&live)).unwrap();
        held.try_lock_exclusive().unwrap();
        let unlocked = root("young");
        let other = temp.path().join("someone-else");
        std::fs::create_dir_all(&other).unwrap();

        assert_eq!(sweep_stale_roots(temp.path(), STALE), 1);
        assert!(!gone.exists(), "its context file went with it");
        assert!(!root_lock(&gone).exists(), "and its lock");
        assert!(
            live.join("0123456789abcdef0123456789abcdef.context.md")
                .exists()
        );
        assert!(unlocked.exists(), "too young to judge without a lock");
        assert!(other.exists(), "not a desk's");
        drop(held);
        // A process another test forks holds a copy of the lock's descriptor until it execs,
        // so the lock can stay taken for a moment after the desk lets go.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while live.exists() && std::time::Instant::now() < deadline {
            if sweep_stale_roots(temp.path(), STALE) == 0 {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        assert!(!live.exists(), "once its desk lets go");
    }

    #[test]
    fn a_root_being_made_is_never_swept() {
        let temp = tempfile::tempdir().unwrap();
        // The moment between making the lock file and locking it, before any directory: no
        // sweep takes the lock file.
        let making = root_lock(&temp.path().join(format!("{AREA_PREFIX}abc")));
        std::fs::File::create(&making).unwrap();
        assert_eq!(sweep_stale_roots(temp.path(), STALE), 0);
        assert!(making.exists(), "too young, whoever holds it");
        std::fs::remove_file(&making).unwrap();

        // Desks starting while others sweep keep every directory they made.
        let stop = Arc::new(AtomicBool::new(false));
        let sweeper = {
            let stop = stop.clone();
            let temp = temp.path().to_path_buf();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    sweep_stale_roots(&temp, STALE);
                }
            })
        };
        let roots: Vec<Root> = (0..50).map(|_| new_root_in(temp.path()).unwrap()).collect();
        stop.store(true, Ordering::Release);
        sweeper.join().unwrap();
        for root in &roots {
            assert!(
                root.path.is_dir() && root.lock.is_file(),
                "{}",
                root.path.display()
            );
            let name = root
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            assert!(name.starts_with(AREA_PREFIX), "{name}");
        }
        assert_eq!(sweep_stale_roots(temp.path(), STALE), 0, "all held");
        drop(roots);
        // A process another test forks holds a copy of each lock's descriptor until it execs, so
        // some locks can stay taken for a moment after their desks let go.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut swept = sweep_stale_roots(temp.path(), STALE);
        while swept < 50 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
            swept += sweep_stale_roots(temp.path(), STALE);
        }
        assert_eq!(swept, 50, "once their desks let go");
        assert_eq!(
            std::fs::read_dir(temp.path()).unwrap().count(),
            0,
            "locks too"
        );
    }

    #[tokio::test]
    async fn a_turn_commits_her_work_to_main() {
        let people = tempfile::tempdir().unwrap();
        let fixture = git_fixture(people.path(), Script::Writes);
        let bare = people.path().join("eve.git");
        fixture
            .desk
            .handle(working("c-w", "Note this.", workspace(&bare)))
            .await;
        until(|| kinds(&fixture, "c-w").contains(&EVENT_COMPLETED.to_owned())).await;
        let shown = kinds(&fixture, "c-w");
        assert_eq!(
            shown,
            [EVENT_ACCEPTED, EVENT_TEXT, EVENT_COMPLETED],
            "the work is saved before the turn is reported over"
        );

        let start = fixture.asked.lock().unwrap().starts[0].clone();
        let mode = start.assistant.clone().unwrap();
        assert_eq!(mode.files, ["personal", "scratch"]);
        let area = PathBuf::from(&start.working_directory);
        let context = PathBuf::from(mode.context_file.unwrap());
        assert_eq!(
            context.parent(),
            area.parent(),
            "beside, not inside, the directory"
        );
        assert!(area.join("scratch").is_dir());
        let claude = std::fs::read_to_string(area.join("CLAUDE.md")).unwrap();
        assert!(
            claude.contains("is the person's personal repository"),
            "{claude}"
        );
        assert!(
            !claude.contains("memory/"),
            "her memory is not in a file: {claude}"
        );
        assert!(
            !area.join("personal").join(".git").exists(),
            "her repository's git directory is outside her workspace"
        );
        let repos = repos_dir(&area).unwrap();
        assert!(repos.join("personal").join("HEAD").exists());

        // Her note is on main, as the person, with the trailer, and nothing else is.
        assert!(
            bare.join("HEAD").exists(),
            "the repository was made on first use"
        );
        let files = git(&bare, &["ls-tree", "-r", "--name-only", "main"]);
        assert_eq!(files.trim(), "notes/a.md", "{files}");
        let head = git(&bare, &["log", "-1", "--format=%an <%ae>%n%B", "main"]);
        assert!(
            head.starts_with("Eve Moneypenny <eve@example.invalid>\nmoneypenny: update notes/a.md"),
            "{head}"
        );
        assert!(
            head.contains("Via: Moneypenny (conversation c-w)"),
            "{head}"
        );

        // Closed, its directory and context file go.
        fixture.desk.close_all();
        until(|| kinds(&fixture, "c-w").contains(&EVENT_CLOSED.to_owned())).await;
        until(|| !area.exists() && !context.exists() && !repos.exists()).await;
    }

    /// Writes an executable hook script that records it ran in `marker`.
    fn hook(path: &Path, marker: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!(
                "#!/bin/sh\necho ran > '{}'\nexit 0\n",
                marker.to_string_lossy().replace('\\', "/")
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[tokio::test]
    async fn hooks_and_a_monitor_in_her_repository_never_run_when_the_station_commits() {
        let people = tempfile::tempdir().unwrap();
        let fixture = git_fixture(people.path(), Script::Works);
        let bare = people.path().join("eve.git");
        fixture
            .desk
            .handle(working("c-h", "Take notes.", workspace(&bare)))
            .await;
        until(|| kinds(&fixture, "c-h").contains(&EVENT_TEXT.to_owned())).await;
        let start = fixture.asked.lock().unwrap().starts[0].clone();
        let area = PathBuf::from(&start.working_directory);
        let git_dir = repos_dir(&area).unwrap().join("personal");
        let marker = people.path().join("hook-ran");
        let hooks = people.path().join("planted");
        hook(&hooks.join("pre-commit"), &marker);
        hook(&hooks.join("post-commit"), &marker);
        hook(&hooks.join("monitor"), &marker);
        let set = |key: &str, value: &Path| {
            git(
                people.path(),
                &[
                    &format!("--git-dir={}", git_dir.to_string_lossy()),
                    "config",
                    key,
                    &value.to_string_lossy().replace('\\', "/"),
                ],
            );
        };
        set("core.hooksPath", &hooks);
        set("core.fsmonitor", &hooks.join("monitor"));
        std::fs::write(area.join("personal").join("notes.md"), "notes\n").unwrap();
        fixture.desk.handle(interruption("c-h", "done")).await;
        until(|| kinds(&fixture, "c-h").contains(&EVENT_INTERRUPTED.to_owned())).await;
        assert_eq!(git(&bare, &["show", "main:notes.md"]), "notes\n");
        assert!(!marker.exists(), "nothing the repository names ran");
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn the_station_takes_only_repositories_its_own_roster_gives() {
        let people = tempfile::tempdir().unwrap();
        let fixture = git_fixture(people.path(), Script::Answers);
        let elsewhere = tempfile::tempdir().unwrap();
        let theirs = elsewhere.path().join("eve.git");
        let mine = people.path().join("eve.git");
        let reason = |conversation: &str| {
            events(&fixture, conversation)
                .first()
                .filter(|event| event.kind == EVENT_REJECTED)
                .and_then(|event| event.text.clone())
                .unwrap_or_default()
        };

        // Another repository, or another person's, for this person.
        fixture
            .desk
            .handle(working("c-1", "hi", workspace(&theirs)))
            .await;
        assert!(reason("c-1").contains("roster"), "{}", reason("c-1"));
        let mut other = working("c-2", "hi", workspace(&people.path().join("mallory.git")));
        other.person_id = Some("eve".to_owned());
        fixture.desk.handle(other).await;
        assert!(reason("c-2").contains("roster"), "{}", reason("c-2"));
        // A shared repository under a name the roster gives another.
        let mut wanted = workspace(&mine);
        wanted.shared.insert(
            "ops".to_owned(),
            elsewhere.path().join("ops").to_string_lossy().into_owned(),
        );
        fixture.desk.handle(working("c-3", "hi", wanted)).await;
        assert!(
            reason("c-3").contains("shared repository"),
            "{}",
            reason("c-3")
        );
        // Commits to repositories the roster does not give.
        let shared = AssistantChange::new(BTreeMap::from([(
            "handbook.md".to_owned(),
            Some("x".to_owned()),
        )]));
        fixture
            .desk
            .handle(committing("c-5", &theirs, shared))
            .await;
        assert!(reason("c-5").contains("commit"), "{}", reason("c-5"));
        // The roster's own repository of a group, for a person the job does not put in it.
        let ops = people.path().join("ops");
        let mut outsider = workspace(&mine);
        outsider
            .shared
            .insert("ops".to_owned(), ops.to_string_lossy().into_owned());
        let mut not_member = working("c-7", "hi", outsider);
        not_member.groups = Some(vec!["gone".to_owned()]);
        fixture.desk.handle(not_member).await;
        assert!(
            reason("c-7").contains("shared repository"),
            "{}",
            reason("c-7")
        );
        let edit = AssistantChange::new(BTreeMap::from([(
            "handbook.md".to_owned(),
            Some("y".to_owned()),
        )]));
        let mut not_member = committing("c-8", &ops, edit.clone());
        not_member.groups = None;
        fixture.desk.handle(not_member).await;
        assert!(reason("c-8").contains("commit"), "{}", reason("c-8"));
        // One group's repository under another's name.
        let mut renamed = committing("c-9", &ops, edit);
        renamed.groups = Some(vec!["ops".to_owned(), "gone".to_owned()]);
        if let Some(commit) = renamed.commit.as_mut() {
            commit.group = "gone".to_owned();
        }
        fixture.desk.handle(renamed).await;
        assert!(reason("c-9").contains("commit"), "{}", reason("c-9"));
        assert!(fixture.asked.lock().unwrap().starts.is_empty());
        assert!(!theirs.exists(), "nothing was made where it was told to");

        // The person's own, as the roster fills it in, is taken.
        fixture
            .desk
            .handle(working("c-6", "hi", workspace(&mine)))
            .await;
        until(|| kinds(&fixture, "c-6").contains(&EVENT_COMPLETED.to_owned())).await;
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn a_stalled_clone_holds_up_only_its_own_conversation() {
        let people = tempfile::tempdir().unwrap();
        let mut fixture = git_fixture(people.path(), Script::Answers);
        // The first clone stalls until released, as a remote that never answers would.
        let (release, held) = std::sync::mpsc::channel::<()>();
        let held = Arc::new(Mutex::new(Some(held)));
        let houses = fixture.desk.houses.clone();
        fixture.desk_mut().houses = Arc::new(move || {
            let waiting = held.lock().unwrap().take();
            if let Some(held) = waiting {
                let _ = held.recv_timeout(Duration::from_secs(30));
            }
            houses()
        });
        let completed = |conversation: &str| {
            kinds(&fixture, conversation)
                .iter()
                .filter(|kind| *kind == EVENT_COMPLETED)
                .count()
        };
        let mut dispatch = Dispatch::new(fixture.desk.clone());
        let mine = people.path().join("eve.git");
        dispatch.send(working("c-slow", "first", workspace(&mine)));
        dispatch.send(working("c-slow", "second", workspace(&mine)));
        dispatch.send(job("c-other", "meanwhile"));
        until(|| completed("c-other") == 1).await;
        assert_eq!(completed("c-slow"), 0, "still cloning");
        release.send(()).unwrap();
        until(|| completed("c-slow") == 2).await;
        // Its own messages kept their order.
        let prompts: Vec<String> = fixture
            .asked
            .lock()
            .unwrap()
            .starts
            .iter()
            .map(|start| start.prompt.clone())
            .collect();
        assert_eq!(prompts, ["meanwhile", "first"], "second reached it warm");
        drop(dispatch);
    }

    #[tokio::test]
    async fn hidden_files_in_her_repository_are_never_saved() {
        let people = tempfile::tempdir().unwrap();
        let fixture = git_fixture(people.path(), Script::Works);
        let bare = people.path().join("eve.git");
        fixture
            .desk
            .handle(working("c-d", "Take notes.", workspace(&bare)))
            .await;
        until(|| kinds(&fixture, "c-d").contains(&EVENT_TEXT.to_owned())).await;
        let start = fixture.asked.lock().unwrap().starts[0].clone();
        let personal = PathBuf::from(&start.working_directory).join("personal");
        std::fs::write(personal.join(".gitattributes"), "* filter=lfs diff=lfs\n").unwrap();
        std::fs::write(personal.join(".gitmodules"), "[submodule \"x\"]\n").unwrap();
        std::fs::create_dir_all(personal.join("notes")).unwrap();
        std::fs::write(personal.join("notes").join(".hidden"), "hidden").unwrap();
        std::fs::write(personal.join("notes").join("a.md"), "a\n").unwrap();
        fixture.desk.handle(interruption("c-d", "done")).await;
        until(|| kinds(&fixture, "c-d").contains(&EVENT_INTERRUPTED.to_owned())).await;
        let files = git(&bare, &["ls-tree", "-r", "--name-only", "main"]);
        assert!(files.contains("notes/a.md"), "{files}");
        assert!(!files.contains("/.") && !files.starts_with('.'), "{files}");
        fixture.desk.close_all();
    }

    /// A shared repository `ops` under `root` whose `main` holds `handbook.md`, and which
    /// takes pushes to it.
    fn shared_ops(root: &Path) -> PathBuf {
        let ops = root.join("ops");
        std::fs::create_dir_all(&ops).unwrap();
        git(&ops, &["init", "--quiet", "--initial-branch=main"]);
        commit_ops(&ops, "# Handbook\n");
        git(
            &ops,
            &["config", "receive.denyCurrentBranch", "updateInstead"],
        );
        ops
    }

    /// Commits `text` as `handbook.md` of `ops`, as someone else.
    fn commit_ops(ops: &Path, text: &str) {
        std::fs::write(ops.join("handbook.md"), text).unwrap();
        git(ops, &["add", "handbook.md"]);
        git(
            ops,
            &[
                "-c",
                "user.name=Q",
                "-c",
                "user.email=q@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "edit",
            ],
        );
    }

    #[tokio::test]
    async fn shared_repositories_are_checked_out_and_a_shared_commit_lands_on_main() {
        let repos = tempfile::tempdir().unwrap();
        let fixture = git_fixture(repos.path(), Script::Answers);
        let ops = shared_ops(repos.path());
        let mut wanted = workspace(&repos.path().join("eve.git"));
        wanted
            .shared
            .insert("ops".to_owned(), ops.to_string_lossy().into_owned());
        wanted.shared.insert(
            "gone".to_owned(),
            repos
                .path()
                .join("gone")
                .join("missing.git")
                .to_string_lossy()
                .into_owned(),
        );
        fixture.desk.handle(working("c-s", "hello", wanted)).await;
        until(|| kinds(&fixture, "c-s").contains(&EVENT_COMPLETED.to_owned())).await;
        let start = fixture.asked.lock().unwrap().starts[0].clone();
        let area = Path::new(&start.working_directory);
        assert!(area.join("shared").join("ops").join("handbook.md").exists());
        assert!(!area.join("shared").join("gone").exists());
        let notices: Vec<String> = events(&fixture, "c-s")
            .into_iter()
            .filter(|event| event.kind == EVENT_NOTICE)
            .filter_map(|event| event.text)
            .collect();
        assert!(
            notices.iter().any(|notice| notice.contains("\"gone\"")),
            "{notices:?}"
        );
        assert!(
            !repos.path().join("gone").join("missing.git").exists(),
            "a shared one is never made"
        );

        let change = AssistantChange::new(BTreeMap::from([(
            "handbook.md".to_owned(),
            Some("# Handbook\n\nOn call: Q.\n".to_owned()),
        )]));
        fixture.desk.handle(committing("c-s", &ops, change)).await;
        let landed = events(&fixture, "c-s")
            .into_iter()
            .rfind(|event| event.kind == EVENT_ACCEPTED)
            .and_then(|event| event.text)
            .unwrap();
        assert!(landed.starts_with("committed "), "{landed}");
        assert!(git(&ops, &["show", "main:handbook.md"]).contains("On call: Q."));
        assert_eq!(
            git(&ops, &["log", "-1", "--format=%an <%ae>", "main"]).trim(),
            "Eve Moneypenny <eve@example.invalid>"
        );
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn a_shared_change_confirmed_after_its_file_changed_on_main_is_refused() {
        let repos = tempfile::tempdir().unwrap();
        let fixture = git_fixture(repos.path(), Script::Reads);
        let ops = shared_ops(repos.path());
        let mut wanted = workspace(&repos.path().join("eve.git"));
        wanted
            .shared
            .insert("ops".to_owned(), ops.to_string_lossy().into_owned());
        let call = |tool: &str, group: &str, path: &str| {
            format!(
                "{tool} {{\"group\": \"{group}\", \"path\": \
                 \"{path}\", \"content\": \"On call: Q.\\n\", \"message\": \"Name who is on \
                 call\"}}"
            )
        };
        let claude = "mcp__moneypenny__propose_shared_change";
        // Codex announces the same call as an MCP tool call item of her server.
        let codex = "codex:propose_shared_change";
        for (n, (tool, group, path)) in [
            (claude, "ops", "handbook.md"),
            (claude, "ops", "rota.md"),
            (claude, "gone", "handbook.md"),
            (codex, "ops", "handbook.md"),
        ]
        .into_iter()
        .enumerate()
        {
            fixture
                .desk
                .handle(working("c-s", &call(tool, group, path), wanted.clone()))
                .await;
            until(|| {
                kinds(&fixture, "c-s")
                    .iter()
                    .filter(|kind| *kind == EVENT_COMPLETED)
                    .count()
                    > n
            })
            .await;
        }
        let reported: Vec<AssistantSharedBase> = events(&fixture, "c-s")
            .into_iter()
            .filter(|event| event.kind == EVENT_TOOL)
            .flat_map(|event| event.shared)
            .collect();
        let content = workspace::sha256_hex(b"On call: Q.\n");
        assert_eq!(
            reported,
            [
                AssistantSharedBase {
                    group: "ops".to_owned(),
                    path: "handbook.md".to_owned(),
                    content: content.clone(),
                    base: Some(workspace::sha256_hex(b"# Handbook\n")),
                },
                AssistantSharedBase {
                    group: "ops".to_owned(),
                    path: "rota.md".to_owned(),
                    content: content.clone(),
                    base: None,
                },
                AssistantSharedBase {
                    group: "ops".to_owned(),
                    path: "handbook.md".to_owned(),
                    content,
                    base: Some(workspace::sha256_hex(b"# Handbook\n")),
                },
            ],
            "a repository not checked out gives no base; Codex's call gives one as Claude's"
        );
        // The change as Universal Exports makes it from her call and its base.
        let change = |base: &AssistantSharedBase| {
            AssistantChange::based(
                BTreeMap::from([(base.path.clone(), Some("On call: Q.\n".to_owned()))]),
                BTreeMap::from([(base.path.clone(), base.base.clone())]),
            )
        };

        // Meanwhile someone else changed the handbook on `main`.
        commit_ops(&ops, "# Handbook\n\nOn call: M.\n");
        fixture
            .desk
            .handle(committing("c-s", &ops, change(&reported[0])))
            .await;
        let refused = events(&fixture, "c-s")
            .into_iter()
            .rfind(|event| event.kind == EVENT_REJECTED)
            .and_then(|event| event.text)
            .unwrap_or_default();
        assert!(refused.contains("handbook.md changed on main"), "{refused}");
        assert_eq!(
            git(&ops, &["show", "main:handbook.md"]),
            "# Handbook\n\nOn call: M.\n"
        );

        // A file nobody has made since lands.
        fixture
            .desk
            .handle(committing("c-s", &ops, change(&reported[1])))
            .await;
        let landed = events(&fixture, "c-s")
            .into_iter()
            .rfind(|event| event.kind == EVENT_ACCEPTED)
            .and_then(|event| event.text)
            .unwrap_or_default();
        assert!(landed.starts_with("committed "), "{landed}");
        assert_eq!(git(&ops, &["show", "main:rota.md"]), "On call: Q.\n");
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn an_elevated_station_refuses_her_unless_the_roster_allows_it() {
        for probe in [Ok(true), Err(std::io::Error::other("whoami did not run"))] {
            let fixture = on_station(AGENTS, probe, true, IDLE, Script::Answers, Script::Answers);
            fixture.desk.handle(job("c-e", "hello")).await;
            assert_eq!(kinds(&fixture, "c-e"), [EVENT_REJECTED]);
            let reason = events(&fixture, "c-e")[0].text.clone().unwrap();
            assert!(reason.contains("assistant.allow_elevated"), "{reason}");
            assert!(fixture.asked.lock().unwrap().starts.is_empty());
        }

        let allowed = format!("{AGENTS}[assistant]\nallow_elevated = true\n");
        let fixture = on_station(
            &allowed,
            Ok(true),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );
        fixture.desk.handle(job("c-e", "hello")).await;
        until(|| kinds(&fixture, "c-e").contains(&EVENT_COMPLETED.to_owned())).await;
        let logged = fixture.logged.lock().unwrap().clone();
        assert!(
            logged
                .iter()
                .any(|line| line.starts_with("weakening: assistant.allow_elevated = true")),
            "{logged:?}"
        );
        let roster = fixture.desk.state.current().roster.clone();
        assert!(
            weakenings(&roster)
                .iter()
                .any(|line| line.contains("assistant.allow_elevated")),
            "the check-in names it"
        );
        fixture.desk.close_all();

        let plain = on_station(
            AGENTS,
            Ok(false),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );
        plain.desk.handle(job("c-e", "hello")).await;
        until(|| kinds(&plain, "c-e").contains(&EVENT_COMPLETED.to_owned())).await;
        assert!(plain.logged.lock().unwrap().is_empty(), "nothing weakened");
        plain.desk.close_all();
    }

    /// A message job that asks for `reach`.
    fn reaching(
        job: AssistantJob,
        interpreters: &[&str],
        policy: &str,
        hosts: &[&str],
    ) -> AssistantJob {
        AssistantJob {
            reach: crate::directorates::section_00::assistant::AssistantReach {
                interpreters: interpreters.iter().map(|name| (*name).to_owned()).collect(),
                network: crate::directorates::section_00::agent::AssistantNetwork {
                    policy: policy.to_owned(),
                    allowed_hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
                    ..Default::default()
                },
                unsandboxed_scripts: true,
            },
            // A reach is for a named person; with none the Station takes the strictest.
            person_id: job.person_id.or_else(|| Some("eve".to_owned())),
            ..job
        }
    }

    #[tokio::test]
    async fn a_script_she_runs_is_saved_by_the_turn_and_never_reaches_past_the_stations_roster() {
        let people = tempfile::tempdir().unwrap();
        let agents = station_agents_with(
            people.path(),
            "interpreters = ['python3', 'node']\nallowed_hosts = ['docs.python.org', 'pypi.org']\n",
        );
        let mut fixture = on_station(
            &agents,
            Ok(false),
            true,
            IDLE,
            Script::Computes,
            Script::Answers,
        );
        fixture.desk_mut().houses = git_houses();
        let bare = people.path().join("eve.git");
        // Universal Exports asks for more than this Station's roster gives.
        let asked = reaching(
            working("c-s", "What is six times seven?", workspace(&bare)),
            &["python3", "perl"],
            "open",
            &[],
        );
        fixture.desk.handle(asked).await;
        until(|| kinds(&fixture, "c-s").contains(&EVENT_COMPLETED.to_owned())).await;

        let start = fixture.asked.lock().unwrap().starts[0].clone();
        let mode = start.assistant.unwrap();
        assert_eq!(mode.interpreters, ["python3"], "only what both name");
        assert_eq!(
            mode.network.policy, "limited",
            "never wider than the roster"
        );
        assert_eq!(mode.network.allowed_hosts, ["docs.python.org", "pypi.org"]);
        assert!(!mode.unsandboxed, "the harness has a sandbox here");
        assert_eq!(mode.read_only, ["shared", "CLAUDE.md", "AGENTS.md"]);
        // Her own configuration directory, beside the working directory, keeps the Agent's out.
        let config = start
            .environment
            .get("CLAUDE_CONFIG_DIR")
            .expect("own config dir");
        let area = PathBuf::from(&start.working_directory);
        assert_eq!(
            PathBuf::from(config).parent(),
            area.parent(),
            "beside, not inside, the working directory"
        );
        // Her own temporary directory too, so her scripts' TMPDIR is not the shared per-user
        // one; the sandbox derives it from the harness's TMPDIR.
        let temp = start.environment.get("TMPDIR").expect("own TMPDIR");
        assert_eq!(start.environment.get("CLAUDE_CODE_TMPDIR"), Some(temp));
        assert_eq!(PathBuf::from(temp).parent(), area.parent());
        assert!(temp.len() < area.to_string_lossy().len(), "kept short");
        assert!(
            !start
                .initial_context
                .iter()
                .any(|line| line.contains("Scripts are unavailable"))
        );
        assert!(
            fixture
                .logged
                .lock()
                .unwrap()
                .iter()
                .all(|line| !line.contains("weakening")),
            "nothing weakened"
        );

        let files = git(&bare, &["ls-tree", "-r", "--name-only", "main"]);
        assert!(
            files.contains("calc.py") && files.contains("out.txt"),
            "the script and what it wrote are on main: {files}"
        );
        assert_eq!(git(&bare, &["show", "main:out.txt"]), "42\n");
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn where_the_harness_has_no_sandbox_she_runs_no_script_unless_the_roster_allows_it() {
        let windows = "Claude Code runs commands unsandboxed on native Windows; its sandbox runs \
                       on macOS, Linux and WSL2";
        let fixture = fixture(true, IDLE);
        *fixture.no_sandbox.lock().unwrap() = Some(windows.to_owned());
        let asked = reaching(job("c-n", "Run a script."), &["python3"], "denied", &[]);
        fixture.desk.handle(asked.clone()).await;
        until(|| kinds(&fixture, "c-n").contains(&EVENT_COMPLETED.to_owned())).await;
        let start = fixture.asked.lock().unwrap().starts[0].clone();
        let mode = start.assistant.unwrap();
        assert!(mode.interpreters.is_empty(), "no Bash: {mode:?}");
        assert!(!mode.unsandboxed);
        assert!(
            start.initial_context.iter().any(|line| line
                .starts_with("Scripts are unavailable on this Station")
                && line.contains("native Windows")),
            "{:?}",
            start.initial_context
        );
        let logged = fixture.logged.lock().unwrap().clone();
        assert!(
            logged.iter().any(
                |line| line.contains("scripts are unavailable on this Station")
                    && line.contains("native Windows")
            ),
            "{logged:?}"
        );
        assert!(
            !logged.iter().any(|line| line.contains("weakening")),
            "{logged:?}"
        );
        fixture.desk.close_all();

        let allowed =
            format!("{AGENTS}[assistant]\nunsandboxed_scripts = true\nnetwork = 'open'\n");
        let loose = on_station(
            &allowed,
            Ok(false),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );
        *loose.no_sandbox.lock().unwrap() = Some(windows.to_owned());
        loose
            .desk
            .handle(reaching(
                job("c-u", "Run a script."),
                &["python3"],
                "open",
                &[],
            ))
            .await;
        until(|| kinds(&loose, "c-u").contains(&EVENT_COMPLETED.to_owned())).await;
        let mode = loose.asked.lock().unwrap().starts[0]
            .assistant
            .clone()
            .unwrap();
        assert_eq!(mode.interpreters, ["python3"]);
        assert!(mode.unsandboxed);
        assert_eq!(mode.network.policy, "open");
        let logged = loose.logged.lock().unwrap().clone();
        assert!(
            logged.iter().any(|line| line
                .starts_with("weakening: assistant scripts run unsandboxed:")
                && line.contains("effectively gets a shell as the Station's user")),
            "the weakening says it loudly: {logged:?}"
        );
        assert!(
            logged
                .iter()
                .any(|line| line.starts_with("weakening: assistant.network = \"open\"")),
            "{logged:?}"
        );
        let roster = loose.desk.state.current().roster.clone();
        let reported = weakenings(&roster);
        assert!(
            reported
                .iter()
                .any(|line| line.starts_with("assistant.network = \"open\""))
                && reported
                    .iter()
                    .any(|line| line.starts_with("assistant.unsandboxed_scripts = true")),
            "the check-in names both: {reported:?}"
        );
        loose.desk.close_all();
    }

    #[tokio::test]
    async fn a_group_table_narrows_the_station_cap_and_a_missing_group_list_is_refused() {
        let people = tempfile::tempdir().unwrap();
        let agents = station_agents_with(
            people.path(),
            "interpreters = ['python3', 'node']\nnetwork = 'open'\n\
             [assistant.group.contractors]\ninterpreters = ['python3']\nnetwork = 'denied'\n",
        );
        let fixture = on_station(
            &agents,
            Ok(false),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );

        // A job for a contractor: the group table narrows interpreters and the network.
        let contractor = AssistantJob {
            person_id: Some("eve".to_owned()),
            groups: Some(vec!["contractors".to_owned()]),
            reach: crate::directorates::section_00::assistant::AssistantReach {
                interpreters: vec!["python3".to_owned(), "node".to_owned()],
                network: crate::directorates::section_00::agent::AssistantNetwork {
                    policy: "open".to_owned(),
                    allowed_hosts: Vec::new(),
                    ..Default::default()
                },
                unsandboxed_scripts: false,
            },
            ..job("c-g", "Hello.")
        };
        fixture.desk.handle(contractor).await;
        until(|| kinds(&fixture, "c-g").contains(&EVENT_COMPLETED.to_owned())).await;
        let mode = fixture.asked.lock().unwrap().starts[0]
            .assistant
            .clone()
            .unwrap();
        assert_eq!(
            mode.interpreters,
            ["python3"],
            "the group narrows interpreters"
        );
        assert_eq!(
            mode.network.policy, "denied",
            "the group denies the network"
        );
        fixture.desk.close_all();

        // A job with no group list, while the roster narrows by group: refused, not skipped.
        let no_groups = AssistantJob {
            person_id: Some("eve".to_owned()),
            groups: None,
            images: Vec::new(),
            ..job("c-h", "Hello.")
        };
        fixture.desk.handle(no_groups).await;
        until(|| kinds(&fixture, "c-h").contains(&EVENT_REJECTED.to_owned())).await;
        let text = text_of(&fixture, "c-h", EVENT_REJECTED).unwrap();
        assert!(text.contains("directory group"), "{text}");
    }

    #[tokio::test]
    async fn a_job_without_a_person_takes_the_strictest_reach() {
        let people = tempfile::tempdir().unwrap();
        let agents = station_agents_with(
            people.path(),
            "interpreters = ['python3']\nnetwork = 'open'\n",
        );
        let fixture = on_station(
            &agents,
            Ok(false),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );
        // The job asks for everything but names no person.
        let anonymous = AssistantJob {
            person_id: None,
            reach: crate::directorates::section_00::assistant::AssistantReach {
                interpreters: vec!["python3".to_owned()],
                network: crate::directorates::section_00::agent::AssistantNetwork {
                    policy: "open".to_owned(),
                    allowed_hosts: Vec::new(),
                    ..Default::default()
                },
                unsandboxed_scripts: false,
            },
            ..job("c-a", "Hello.")
        };
        fixture.desk.handle(anonymous).await;
        until(|| kinds(&fixture, "c-a").contains(&EVENT_COMPLETED.to_owned())).await;
        let mode = fixture.asked.lock().unwrap().starts[0]
            .assistant
            .clone()
            .unwrap();
        assert!(mode.interpreters.is_empty(), "no person, no scripts");
        assert!(mode.network.reaches_nothing(), "no person, no network");
    }

    #[tokio::test]
    async fn a_narrower_reach_restarts_the_running_harness_rather_than_reaching_it_warm() {
        let people = tempfile::tempdir().unwrap();
        let agents = station_agents_with(
            people.path(),
            "interpreters = ['python3']\nnetwork = 'open'\n",
        );
        let fixture = on_station(
            &agents,
            Ok(false),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );
        let open = |conversation: &str| AssistantJob {
            person_id: Some("eve".to_owned()),
            reach: crate::directorates::section_00::assistant::AssistantReach {
                interpreters: vec!["python3".to_owned()],
                network: crate::directorates::section_00::agent::AssistantNetwork {
                    policy: "open".to_owned(),
                    allowed_hosts: Vec::new(),
                    ..Default::default()
                },
                unsandboxed_scripts: false,
            },
            ..job(conversation, "first")
        };
        fixture.desk.handle(open("c-r")).await;
        until(|| kinds(&fixture, "c-r").contains(&EVENT_COMPLETED.to_owned())).await;
        assert_eq!(fixture.asked.lock().unwrap().starts.len(), 1);

        // The next message carries a narrower reach: the harness restarts (a second start, or
        // a resume), it is not reached warm with the open reach it began under.
        let narrowed = AssistantJob {
            person_id: Some("eve".to_owned()),
            reach: crate::directorates::section_00::assistant::AssistantReach {
                interpreters: Vec::new(),
                network: crate::directorates::section_00::agent::AssistantNetwork {
                    policy: "denied".to_owned(),
                    allowed_hosts: Vec::new(),
                    ..Default::default()
                },
                unsandboxed_scripts: false,
            },
            ..job("c-r", "second")
        };
        fixture.desk.handle(narrowed).await;
        until(|| {
            fixture
                .asked
                .lock()
                .unwrap()
                .starts
                .iter()
                .chain(std::iter::empty())
                .any(|start| start.prompt == "second")
                || !fixture.asked.lock().unwrap().resumes.is_empty()
        })
        .await;
        let asked = fixture.asked.lock().unwrap();
        let restarted = asked.starts.len() > 1 || !asked.resumes.is_empty();
        assert!(restarted, "the harness restarted on the narrower reach");
        let mode = asked
            .resumes
            .first()
            .and_then(|resume| resume.assistant.clone())
            .or_else(|| {
                asked
                    .starts
                    .get(1)
                    .and_then(|start| start.assistant.clone())
            })
            .unwrap();
        assert!(
            mode.interpreters.is_empty(),
            "the restarted harness runs no script"
        );
        assert_eq!(mode.network.policy, "denied");
        drop(asked);
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn a_narrowed_shared_set_or_a_new_token_restarts_the_running_harness() {
        let people = tempfile::tempdir().unwrap();
        let fixture = git_fixture(people.path(), Script::Answers);
        let mine = people.path().join("eve.git");
        let launched = || {
            let asked = fixture.asked.lock().unwrap();
            asked.starts.len() + asked.resumes.len()
        };
        let answered = |count: usize| {
            kinds(&fixture, "c-n")
                .iter()
                .filter(|kind| *kind == EVENT_COMPLETED)
                .count()
                >= count
        };
        let with_ops = || {
            let mut wanted = workspace(&mine);
            wanted.shared.insert(
                "ops".to_owned(),
                people.path().join("ops").to_string_lossy().into_owned(),
            );
            wanted
        };
        fixture
            .desk
            .handle(working("c-n", "first", with_ops()))
            .await;
        until(|| answered(1)).await;
        assert_eq!(launched(), 1);
        // The same again reaches it warm.
        fixture
            .desk
            .handle(working("c-n", "second", with_ops()))
            .await;
        until(|| answered(2)).await;
        assert_eq!(launched(), 1, "reached warm");
        // The person left `ops`: the harness that has `shared/ops/` starts again without it.
        let mut left = working("c-n", "third", workspace(&mine));
        left.groups = Some(vec!["gone".to_owned()]);
        fixture.desk.handle(left).await;
        until(|| launched() == 2).await;
        until(|| answered(3)).await;
        // A message with another token (the old one withdrawn) never reaches the old harness.
        let mut renewed = working("c-n", "fourth", workspace(&mine));
        renewed.groups = Some(vec!["gone".to_owned()]);
        renewed.mcp_token = "renewed-token-".repeat(4);
        fixture.desk.handle(renewed).await;
        until(|| launched() == 3).await;
        fixture.desk.close_all();
    }

    /// A catalog of two skills under `catalog`, each pinned as `ai6 assistant pin-skill`
    /// prints it: the roster's agents file.
    fn skills_catalog(catalog: &Path) -> String {
        let mut agents = AGENTS.to_owned();
        for name in ["charts", "report-writer"] {
            let dir = catalog.join(name);
            std::fs::create_dir_all(dir.join("scripts")).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!(
                    "---\nname: {name}\ndescription: Makes {name}.\n---\n\nSee reference.md.\n"
                ),
            )
            .unwrap();
            std::fs::write(dir.join("reference.md"), "How it is done.\n").unwrap();
            std::fs::write(dir.join("scripts").join("run.py"), "print('done')\n").unwrap();
            let pin = crate::ProjectManifest::read_tree(&dir).unwrap().digest();
            agents.push_str(&format!(
                "[assistant.skills.{name}]\npath = '{}'\npin = '{pin}'\n",
                dir.display()
            ));
        }
        agents
    }

    #[tokio::test]
    async fn a_pinned_skill_is_installed_for_her_and_a_changed_one_is_left_out_and_reported() {
        use crate::directorates::registry::logbook::StationFindingKind;

        let catalog = tempfile::tempdir().unwrap();
        let fixture = on_station(
            &skills_catalog(catalog.path()),
            Ok(false),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );
        // One byte of report-writer changes after the roster pinned it.
        std::fs::write(
            catalog.path().join("report-writer").join("reference.md"),
            "How it is done!\n",
        )
        .unwrap();
        let given = AssistantJob {
            skills: vec!["charts".to_owned(), "report-writer".to_owned()],
            // Skills are given to a person.
            person_id: Some("eve".to_owned()),
            ..job("c-k", "Chart the week.")
        };
        fixture.desk.handle(given).await;
        until(|| kinds(&fixture, "c-k").contains(&EVENT_COMPLETED.to_owned())).await;

        let start = fixture.asked.lock().unwrap().starts[0].clone();
        let mode = start.assistant.clone().unwrap();
        let area = Path::new(&start.working_directory);
        let plugin = area.join(".claude");
        assert_eq!(mode.skills, ["charts"]);
        assert_eq!(
            mode.skills_dir.as_deref(),
            Some(plugin.to_string_lossy().as_ref())
        );
        assert!(!mode.skill_commands);
        let installed = plugin.join("skills").join("charts");
        for file in ["SKILL.md", "reference.md", "scripts/run.py"] {
            assert!(installed.join(file).is_file(), "{file}");
        }
        assert_eq!(
            crate::ProjectManifest::read_tree(&installed)
                .unwrap()
                .digest(),
            crate::ProjectManifest::read_tree(&catalog.path().join("charts"))
                .unwrap()
                .digest(),
            "the copy is what was pinned"
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(plugin.join(".claude-plugin").join("plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["name"], "ai6");
        assert!(!plugin.join("skills").join("report-writer").exists());

        let logged = fixture.logged.lock().unwrap().clone();
        assert!(
            logged
                .iter()
                .any(|line| line == "assistant skill report-writer does not match its pin"),
            "{logged:?}"
        );
        let logbook = fixture.desk.state.intake.logbook("test").unwrap();
        let finding = &logbook.last().unwrap().findings[0];
        assert_eq!(finding.kind, StationFindingKind::Provider);
        assert_eq!(
            finding.text,
            "assistant skill report-writer does not match its pin"
        );
        let notices: Vec<String> = events(&fixture, "c-k")
            .into_iter()
            .filter(|event| event.kind == EVENT_NOTICE)
            .filter_map(|event| event.text)
            .collect();
        assert!(
            notices
                .iter()
                .any(|notice| notice.contains("\"report-writer\"")),
            "{notices:?}"
        );
        fixture.desk.close_all();

        // A skill the Station's own catalog lacks is refused, and nothing starts.
        let unknown = AssistantJob {
            skills: vec!["lockpicking".to_owned()],
            person_id: Some("eve".to_owned()),
            ..job("c-u", "Open it.")
        };
        fixture.desk.handle(unknown).await;
        assert_eq!(kinds(&fixture, "c-u"), [EVENT_REJECTED]);
        let reason = events(&fixture, "c-u")[0].text.clone().unwrap();
        assert!(reason.contains("\"lockpicking\""), "{reason}");
        assert_eq!(fixture.asked.lock().unwrap().starts.len(), 1);

        // Without skills she starts as before: no Skill tool, no plugin directory.
        fixture.desk.handle(job("c-n", "Hello.")).await;
        until(|| kinds(&fixture, "c-n").contains(&EVENT_COMPLETED.to_owned())).await;
        let start = fixture.asked.lock().unwrap().starts[1].clone();
        let mode = start.assistant.unwrap();
        assert!(mode.skills.is_empty() && mode.skills_dir.is_none());
        assert!(!Path::new(&start.working_directory).join(".claude").exists());
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn a_harness_that_cannot_keep_to_her_skills_gets_none_and_she_is_told() {
        let catalog = tempfile::tempdir().unwrap();
        let fixture = on_station(
            &skills_catalog(catalog.path()),
            Ok(false),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );
        *fixture.no_skills.lock().unwrap() = Some("it keeps no list of skills".to_owned());
        let given = AssistantJob {
            skills: vec!["charts".to_owned()],
            person_id: Some("eve".to_owned()),
            ..job("c-ns", "Chart the week.")
        };
        fixture.desk.handle(given).await;
        until(|| kinds(&fixture, "c-ns").contains(&EVENT_COMPLETED.to_owned())).await;
        let start = fixture.asked.lock().unwrap().starts[0].clone();
        let mode = start.assistant.clone().unwrap();
        assert!(
            mode.skills.is_empty() && mode.skills_dir.is_none(),
            "{mode:?}"
        );
        assert!(!Path::new(&start.working_directory).join(".claude").exists());
        assert!(
            start.initial_context.iter().any(|line| line.starts_with(
                "Skills are unavailable in this conversation (it \
                                               keeps no list of skills)"
            )),
            "{:?}",
            start.initial_context
        );
        let logged = fixture.logged.lock().unwrap().clone();
        assert!(
            logged.iter().any(|line| line.ends_with(
                "skills are unavailable with handler \"mock-assistant\": it keeps no list of \
                 skills"
            )),
            "{logged:?}"
        );
        fixture.desk.close_all();
    }

    #[tokio::test]
    async fn a_skill_the_stations_tables_leave_out_is_refused_and_a_changed_list_restarts_her() {
        let catalog = tempfile::tempdir().unwrap();
        let agents = format!(
            "{}[assistant.group.contractors]\nskills = ['charts']\n",
            skills_catalog(catalog.path())
        );
        let fixture = on_station(
            &agents,
            Ok(false),
            true,
            IDLE,
            Script::Answers,
            Script::Answers,
        );
        let given = |conversation: &str, message: &str, skills: &[&str]| AssistantJob {
            skills: skills.iter().map(|name| (*name).to_owned()).collect(),
            person_id: Some("eve".to_owned()),
            groups: Some(vec!["contractors".to_owned()]),
            ..job(conversation, message)
        };

        // The gateway gave a contractor a skill the Station's group table leaves out.
        fixture
            .desk
            .handle(given("c-w", "Write it up.", &["report-writer"]))
            .await;
        assert_eq!(kinds(&fixture, "c-w"), [EVENT_REJECTED]);
        let reason = events(&fixture, "c-w")[0].text.clone().unwrap();
        assert!(reason.contains("\"report-writer\""), "{reason}");

        // With no group list the Station cannot apply the table: refused, not skipped.
        let unresolved = AssistantJob {
            groups: None,
            images: Vec::new(),
            ..given("c-v", "Chart it.", &["charts"])
        };
        fixture.desk.handle(unresolved).await;
        assert_eq!(kinds(&fixture, "c-v"), [EVENT_REJECTED]);
        let reason = events(&fixture, "c-v")[0].text.clone().unwrap();
        assert!(reason.contains("directory group"), "{reason}");

        // No person, no skills.
        let anonymous = AssistantJob {
            person_id: None,
            groups: None,
            images: Vec::new(),
            ..given("c-z", "Chart it.", &["charts"])
        };
        fixture.desk.handle(anonymous).await;
        assert_eq!(kinds(&fixture, "c-z"), [EVENT_REJECTED]);
        assert!(fixture.asked.lock().unwrap().starts.is_empty());

        // A skill it gives starts her with it; the next message without it restarts her.
        fixture
            .desk
            .handle(given("c-c", "first", &["charts"]))
            .await;
        until(|| kinds(&fixture, "c-c").contains(&EVENT_COMPLETED.to_owned())).await;
        let mode = fixture.asked.lock().unwrap().starts[0]
            .assistant
            .clone()
            .unwrap();
        assert_eq!(mode.skills, ["charts"]);
        fixture.desk.handle(given("c-c", "second", &[])).await;
        until(|| {
            let asked = fixture.asked.lock().unwrap();
            asked.starts.len() > 1 || !asked.resumes.is_empty()
        })
        .await;
        let asked = fixture.asked.lock().unwrap();
        let mode = asked
            .resumes
            .first()
            .and_then(|resume| resume.assistant.clone())
            .or_else(|| {
                asked
                    .starts
                    .get(1)
                    .and_then(|start| start.assistant.clone())
            })
            .unwrap();
        assert!(mode.skills.is_empty(), "restarted without the skill");
        drop(asked);
        fixture.desk.close_all();
    }
}
