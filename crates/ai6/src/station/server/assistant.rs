//! Moneypenny on a Station: the assistant Sessions of Universal Exports conversations.
//!
//! Universal Exports queues an [`AssistantJob`] for a Station each time a person writes to
//! Moneypenny. The desk keeps one harness per conversation, started with the roster's
//! assistant Agent (its handler, model, effort and the model credentials its environment
//! carries) in [`AssistantMode`]: no built-in tool and exactly one MCP server, Universal
//! Exports' endpoint, reached with the job's token. A running harness takes the next message
//! as it is; one idle for [`IDLE`] is closed, and the next message resumes its provider
//! Session. What the harness says is reported back as [`AssistantReport`]s. A harness that
//! fails or exits is reported `failed`, with its handler's reason, and closed at once, which
//! frees its slot; so is one that writes nothing for [`IDLE`] while answering. A resume that
//! fails, when asked or before the resumed harness confirms its Session, goes to the
//! Station's log and the conversation starts afresh with its recap.
//!
//! An assistant Session is not a Mission: it has no Operation, no safe house and no Cable
//! endpoint, and is never on the board. It works in a directory of its own, named for its
//! conversation inside one the desk made readable by the Station's user only: the same
//! directory each time the conversation's harness starts, because Claude Code finds a Session
//! to resume only from the directory it was recorded in. The directory goes when the harness
//! closes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use url::Url;

use super::{DaemonState, medical, station_terminal, unix_now, vetting};
use crate::directorates::m::identity::{MissionId, SessionId};
use crate::directorates::q::terminal::TerminalBridge;
use crate::directorates::section_00::agent::{
    AgentEvent, AgentMessage, AgentResume, AgentSession, AgentStart, AssistantMode, LIMIT_EVENT,
};
use crate::directorates::section_00::assistant::{
    ASSISTANT_MCP_PATH, ASSISTANT_MCP_TOKEN_ENV, ASSISTANT_MCP_URL_ENV, AssistantEvent,
    AssistantJob, AssistantReport, EVENT_ACCEPTED, EVENT_CLOSED, EVENT_COMPLETED, EVENT_FAILED,
    EVENT_LIMIT, EVENT_REJECTED, EVENT_TEXT, EVENT_TOOL,
};
use crate::guatiao::library::Object;

/// How long a harness may wait for the next message before the desk closes it.
pub(super) const IDLE: Duration = Duration::from_secs(10 * 60);
/// The most assistant harnesses a Station runs at once.
pub(super) const MAX_LIVE: usize = 4;
/// A harness whose token expires within this many seconds is restarted with the job's.
const TOKEN_MARGIN_SECS: f64 = 120.0;
/// How long one read of a harness's events waits, so a message never waits long for the lock.
const READ_SLICE_MS: i64 = 250;
/// The prefix of the directory that holds the assistants' working directories.
const AREA_PREFIX: &str = "ai6-assistant-";
/// How long Universal Exports holds a job poll open.
const JOB_WAIT_MS: u64 = 1_000;
/// Reports waiting to be sent; past it the newest are dropped, and the gateway sees a gap.
const REPORT_QUEUE: usize = 256;
/// How long a stopping desk waits between looks at its queue.
const STOP_CHECK: Duration = Duration::from_millis(250);
/// Conversations whose provider Session the desk remembers for a resume.
const MAX_KNOWN: usize = 1_024;
/// How long removing a closed harness's working directory is tried again.
const REMOVE_RETRY: Duration = Duration::from_secs(2);

/// A harness, shared by the desk (to send it messages) and its pump (to read it).
type SharedSession = Arc<Mutex<Object<dyn AgentSession>>>;

/// Sends one report to Universal Exports; it must not block for long.
pub(super) type Reporter = Arc<dyn Fn(AssistantReport) + Send + Sync>;

/// Starts and resumes harnesses: the Station's own handlers and terminal, or a test's.
pub(super) trait Harness: Send + Sync {
    /// Whether `handler` can be started here.
    fn available(&self, handler: &str) -> bool;
    /// Starts a harness with `handler`.
    fn start(&self, handler: &str, request: AgentStart)
    -> Result<Object<dyn AgentSession>, String>;
    /// Resumes a provider Session with `handler`.
    fn resume(
        &self,
        handler: &str,
        request: AgentResume,
    ) -> Result<Object<dyn AgentSession>, String>;
}

/// The Station's handlers, each on the terminal provider the Station uses.
struct StationHarness(DaemonState);

impl StationHarness {
    fn terminal(
        &self,
    ) -> Result<Object<dyn crate::directorates::q::terminal::AgentTerminal>, String> {
        let provider = station_terminal(&self.0)?;
        let terminal = self
            .0
            .plugins
            .terminal(&provider, None)
            .map_err(|error| error.to_string())?;
        Ok(TerminalBridge::agent_terminal(terminal))
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
        request: AgentStart,
    ) -> Result<Object<dyn AgentSession>, String> {
        let terminal = self.terminal()?;
        self.0
            .plugins
            .handler(handler, None)
            .map_err(|error| error.to_string())?
            .start(request, terminal)
            .map_err(|error| error.to_string())
    }

    fn resume(
        &self,
        handler: &str,
        request: AgentResume,
    ) -> Result<Object<dyn AgentSession>, String> {
        let terminal = self.terminal()?;
        self.0
            .plugins
            .handler(handler, None)
            .map_err(|error| error.to_string())?
            .resume(request, terminal)
            .map_err(|error| error.to_string())
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
    last_active: Mutex<Instant>,
}

struct Live {
    session: SharedSession,
    token_expires_at: f64,
    flags: Arc<Flags>,
}

/// The assistant Sessions of one Station.
pub(super) struct Desk {
    state: DaemonState,
    harness: Arc<dyn Harness>,
    report: Reporter,
    mcp_url: String,
    idle: Duration,
    max_live: usize,
    live: Arc<Mutex<BTreeMap<String, Live>>>,
    known: Arc<Mutex<BTreeMap<String, String>>>,
    /// Where the conversations' working directories go, made at the first start.
    areas: Mutex<Option<PathBuf>>,
}

impl Desk {
    pub(super) fn new(
        state: DaemonState,
        harness: Arc<dyn Harness>,
        report: Reporter,
        mcp_url: String,
    ) -> Self {
        Self {
            state,
            harness,
            report,
            mcp_url,
            idle: IDLE,
            max_live: MAX_LIVE,
            live: Arc::default(),
            known: Arc::default(),
            areas: Mutex::default(),
        }
    }

    #[cfg(test)]
    fn with_limits(mut self, idle: Duration, max_live: usize) -> Self {
        self.idle = idle;
        self.max_live = max_live;
        self
    }

    /// Takes one job and reports whether the harness has the message.
    pub(super) async fn handle(&self, job: AssistantJob) {
        let outcome = self.take(&job).await;
        let event = match outcome {
            Ok(()) => AssistantEvent::new(EVENT_ACCEPTED, None),
            Err(reason) => AssistantEvent::new(EVENT_REJECTED, Some(reason)),
        };
        let session_id = self.known_session(&job.conversation_id);
        (self.report)(AssistantReport {
            station_id: self.state.station_id.clone(),
            conversation_id: job.conversation_id,
            job_id: Some(job.job_id),
            session_id,
            events: vec![event],
        });
    }

    async fn take(&self, job: &AssistantJob) -> Result<(), String> {
        job.validate()?;
        if job.close {
            self.close(&job.conversation_id);
            return Ok(());
        }
        let running = self.running(&job.conversation_id);
        if let Some((session, flags, expires)) = running {
            if expires > unix_now() + TOKEN_MARGIN_SECS {
                flags.busy.store(true, Ordering::Release);
                *flags
                    .last_active
                    .lock()
                    .map_err(|_| "assistant lock failed".to_owned())? = Instant::now();
                let text = job.message.clone();
                return tokio::task::spawn_blocking(move || {
                    session
                        .lock()
                        .map_err(|_| "assistant Session lock failed".to_owned())?
                        .send_message(AgentMessage { text })
                        .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())?;
            }
            // Its token is about to expire: the harness starts again with the job's.
            self.close(&job.conversation_id);
        }
        // A harness that has stopped may still be removing the directory the next one uses.
        self.wait_closed(&job.conversation_id).await;
        if self
            .live
            .lock()
            .map_err(|_| "assistant lock failed")?
            .contains_key(&job.conversation_id)
        {
            return Err("the conversation's previous harness is still closing".to_owned());
        }
        self.begin(job).await
    }

    /// Starts or resumes the conversation's harness with the job's message.
    async fn begin(&self, job: &AssistantJob) -> Result<(), String> {
        let state = self.state.current();
        let agent = state
            .roster
            .agent(None, &job.agent_id)
            .map_err(|error| format!("the assistant Agent is not on this roster: {error}"))?;
        if medical::is_quarantined(&state, &agent.id, None) {
            return Err(format!("Agent {:?} is quarantined", agent.id));
        }
        let handler = agent
            .handler
            .clone()
            .ok_or_else(|| format!("Agent {:?} has no handler", agent.id))?;
        if !self.harness.available(&handler) {
            return Err(format!("this Station has no {handler:?} handler"));
        }
        let live = self.live.lock().map_err(|_| "assistant lock failed")?.len();
        if live >= self.max_live {
            return Err("this Station runs as many assistant Sessions as it may".to_owned());
        }
        let area = self.area(&job.conversation_id)?;
        // As a Chief of Staff review, the harness keeps only what reaches its model.
        let mut environment = vetting::review_environment(&agent.env);
        environment.insert(ASSISTANT_MCP_URL_ENV.to_owned(), self.mcp_url.clone());
        environment.insert(ASSISTANT_MCP_TOKEN_ENV.to_owned(), job.mcp_token.clone());
        let mission_id = MissionId::try_new(format!(
            "assistant--{}",
            conversation_digest(&job.conversation_id)
        ))
        .map_err(|_| "the conversation cannot name a Session".to_owned())?;
        let previous = job
            .session_id
            .clone()
            .or_else(|| self.known_session(&job.conversation_id));
        let mut initial_context: Vec<String> = Some(agent.context.clone())
            .filter(|context| !context.trim().is_empty())
            .into_iter()
            .collect();
        initial_context.extend(job.context.iter().cloned());
        // A resumed harness has the conversation; only one started afresh is given its recap.
        let mut fresh_context = initial_context.clone();
        fresh_context.extend(job.recap.iter().cloned());
        let start = AgentStart {
            mission_id: mission_id.clone(),
            agent_id: agent.id.clone(),
            prompt: job.message.clone(),
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
            assistant: Some(AssistantMode::moneypenny()),
        };
        let resume = previous
            .and_then(|previous| SessionId::try_new(previous).ok())
            .map(|session_id| AgentResume {
                mission_id,
                session_id,
                message: Some(job.message.clone()),
                working_directory: area.clone(),
                environment,
                model: agent.model.clone(),
                effort: agent.effort.clone(),
                max_cost_micro_usd: None,
                project_settings: false,
                project_mcp: false,
                answer_only: false,
                assistant: Some(AssistantMode::moneypenny()),
                initial_context,
            });
        let harness = self.harness.clone();
        let conversation = job.conversation_id.clone();
        let started = tokio::task::spawn_blocking(move || {
            let Some(resume) = resume else {
                return harness
                    .start(&handler, start)
                    .map(|session| (session, None));
            };
            let resumed = resume.session_id.as_str().to_owned();
            match harness.resume(&handler, resume) {
                // A resumed harness can still fail before it confirms the Session (Claude Code
                // reads the message first): the pump then starts afresh with `start`.
                Ok(session) => Ok((
                    session,
                    Some(Fallback {
                        harness,
                        handler,
                        start,
                        resumed,
                    }),
                )),
                // A provider Session that cannot be resumed (its transcript gone) starts afresh.
                Err(error) => {
                    report_resume_failure(&conversation, &resumed, &error);
                    harness
                        .start(&handler, start)
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
                remove_area(&area);
                return Err(format!("the assistant harness did not start: {error}"));
            }
        };
        let flags = Arc::new(Flags {
            busy: AtomicBool::new(true),
            alive: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            last_active: Mutex::new(Instant::now()),
        });
        let session = Arc::new(Mutex::new(session));
        self.live
            .lock()
            .map_err(|_| "assistant lock failed")?
            .insert(
                job.conversation_id.clone(),
                Live {
                    session: session.clone(),
                    token_expires_at: job.token_expires_at,
                    flags: flags.clone(),
                },
            );
        let pump = Pump {
            conversation_id: job.conversation_id.clone(),
            station_id: state.station_id.clone(),
            session,
            flags,
            area,
            idle: self.idle,
            report: self.report.clone(),
            live: self.live.clone(),
            known: self.known.clone(),
            stopping: state.stopping.clone(),
            fallback,
        };
        std::thread::Builder::new()
            .name("ai6-assistant".to_owned())
            .spawn(move || pump.run())
            .map_err(|error| format!("the assistant pump did not start: {error}"))?;
        Ok(())
    }

    fn running(&self, conversation: &str) -> Option<(SharedSession, Arc<Flags>, f64)> {
        let live = self.live.lock().ok()?;
        let entry = live.get(conversation)?;
        entry.flags.alive.load(Ordering::Acquire).then(|| {
            (
                entry.session.clone(),
                entry.flags.clone(),
                entry.token_expires_at,
            )
        })
    }

    /// Asks the conversation's harness to close; its pump reports `closed`.
    fn close(&self, conversation: &str) {
        if let Ok(live) = self.live.lock()
            && let Some(entry) = live.get(conversation)
        {
            entry.flags.stop.store(true, Ordering::Release);
        }
    }

    async fn wait_closed(&self, conversation: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
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
        self.known.lock().ok()?.get(conversation).cloned()
    }

    /// The conversation's working directory, made empty-if-new; the same path on every start.
    fn area(&self, conversation: &str) -> Result<String, String> {
        let mut areas = self
            .areas
            .lock()
            .map_err(|_| "assistant lock failed".to_owned())?;
        let root = match &*areas {
            Some(root) => root.clone(),
            None => areas.insert(new_root()?).clone(),
        };
        let area = root.join(conversation_digest(conversation));
        std::fs::create_dir_all(&area)
            .map_err(|error| format!("assistant working directory not made: {error}"))?;
        Ok(area.to_string_lossy().into_owned())
    }
}

/// Reads one harness's events until it ends, idles out or is closed.
struct Pump {
    conversation_id: String,
    station_id: String,
    session: SharedSession,
    flags: Arc<Flags>,
    area: String,
    idle: Duration,
    report: Reporter,
    live: Arc<Mutex<BTreeMap<String, Live>>>,
    known: Arc<Mutex<BTreeMap<String, String>>>,
    stopping: Arc<AtomicBool>,
    /// For a resumed harness, how to start afresh should it fail before confirming the
    /// provider Session.
    fallback: Option<Fallback>,
}

/// A fresh start for a conversation whose harness was resumed.
struct Fallback {
    harness: Arc<dyn Harness>,
    handler: String,
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
            let read = match self.session.lock() {
                Ok(session) => {
                    if session_id.is_none() {
                        session_id = session.session_id().ok().flatten();
                        if let Some(id) = &session_id {
                            self.remember(id.as_str());
                        }
                    }
                    session.next_event(READ_SLICE_MS)
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
                    let ended = matches!(event.kind.as_str(), "failed" | LIMIT_EVENT);
                    if event.kind == "completed" {
                        self.flags.busy.store(false, Ordering::Release);
                        if let Ok(mut last) = self.flags.last_active.lock() {
                            *last = Instant::now();
                        }
                    }
                    if let Some(event) = translate(&event) {
                        self.send(session_id.as_ref(), vec![event]);
                    }
                    if ended {
                        break Some("the harness stopped".to_owned());
                    }
                }
                Ok(None) => {
                    let quiet = self
                        .flags
                        .last_active
                        .lock()
                        .is_ok_and(|last| last.elapsed() >= self.idle);
                    if !self.flags.busy.load(Ordering::Acquire) {
                        if quiet {
                            break Some("idle".to_owned());
                        }
                    } else if quiet && heard.elapsed() >= self.idle {
                        // A harness that hangs while answering would hold its slot for good.
                        let reason = format!(
                            "the assistant harness wrote nothing for {} seconds while \
                             answering; it was closed",
                            self.idle.as_secs()
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
        if let Ok(session) = self.session.lock() {
            let _ = session.close();
        }
        remove_area(&self.area);
        if let Ok(mut live) = self.live.lock()
            && live
                .get(&self.conversation_id)
                .is_some_and(|entry| Arc::ptr_eq(&entry.flags, &self.flags))
        {
            live.remove(&self.conversation_id);
        }
        self.send(
            session_id.as_ref(),
            vec![AssistantEvent::new(EVENT_CLOSED, reason)],
        );
    }

    /// Replaces a resumed harness that failed with `failure` by one started afresh (with the
    /// conversation's recap), after closing it. The failure goes to the Station's log; `Err`
    /// says why neither runs, for the conversation.
    fn start_afresh(&mut self, fallback: Fallback, failure: &str) -> Result<(), String> {
        report_resume_failure(&self.conversation_id, &fallback.resumed, failure);
        let mut session = self
            .session
            .lock()
            .map_err(|_| "assistant Session lock failed".to_owned())?;
        let _ = session.close();
        match fallback.harness.start(&fallback.handler, fallback.start) {
            Ok(started) => {
                *session = started;
                Ok(())
            }
            Err(error) => Err(format!(
                "resuming the conversation's Session failed ({failure}), and starting afresh \
                 failed too: {error}"
            )),
        }
    }

    fn remember(&self, session_id: &str) {
        if let Ok(mut known) = self.known.lock() {
            if known.len() >= MAX_KNOWN && !known.contains_key(&self.conversation_id) {
                // Forget an arbitrary one; a forgotten conversation starts afresh.
                if let Some(first) = known.keys().next().cloned() {
                    known.remove(&first);
                }
            }
            known.insert(self.conversation_id.clone(), session_id.to_owned());
        }
    }

    fn send(&self, session_id: Option<&SessionId>, events: Vec<AssistantEvent>) {
        (self.report)(AssistantReport {
            station_id: self.station_id.clone(),
            conversation_id: self.conversation_id.clone(),
            job_id: None,
            session_id: session_id.map(|id| id.as_str().to_owned()),
            events,
        });
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

/// A file-safe name for a conversation, the same on every Station.
fn conversation_digest(conversation: &str) -> String {
    let digest = Sha256::digest(conversation.as_bytes());
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl Drop for Desk {
    /// Removes the directory of working directories once its harnesses have removed theirs; a
    /// harness still closing leaves it behind.
    fn drop(&mut self) {
        if let Ok(mut areas) = self.areas.lock()
            && let Some(root) = areas.take()
        {
            let _ = std::fs::remove_dir(root);
        }
    }
}

/// A new empty directory under the temporary directory, named at random and readable by the
/// Station's user only, for the conversations' working directories.
fn new_root() -> Result<PathBuf, String> {
    Ok(tempfile::Builder::new()
        .prefix(AREA_PREFIX)
        .tempdir()
        .map_err(|error| format!("assistant working directory not made: {error}"))?
        .keep())
}

/// Removes a conversation's directory [`Desk::area`] made, and nothing else.
fn remove_area(path: &str) {
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
    // Windows refuses to remove a directory a process still has open (os error 32), which a
    // harness's processes do for a moment after they have exited, so it is tried again.
    let deadline = Instant::now() + REMOVE_RETRY;
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

/// Starts the desk when the Station publishes to Universal Exports; `None` otherwise.
pub(super) fn spawn(state: &DaemonState) -> std::io::Result<Option<JoinHandle<()>>> {
    let Some(publisher) = &state.publisher else {
        return Ok(None);
    };
    let (origin, token) = publisher.link();
    let mcp_url = origin
        .join(ASSISTANT_MCP_PATH)
        .map_err(|error| std::io::Error::other(format!("assistant endpoint URL: {error}")))?
        .to_string();
    let (reports, queued) = std::sync::mpsc::sync_channel::<AssistantReport>(REPORT_QUEUE);
    let (jobs, mut receiver) = mpsc::channel::<AssistantJob>(8);
    let link = Link {
        origin,
        token,
        station: state.station_id.clone(),
    };
    let running = state.stopping.clone();
    let poll_link = link.clone();
    std::thread::Builder::new()
        .name("ai6-assistant-jobs".to_owned())
        .spawn(move || poll_link.take_jobs(&running, &jobs))?;
    let (stale_conversations, mut stale) = mpsc::unbounded_channel::<String>();
    std::thread::Builder::new()
        .name("ai6-assistant-reports".to_owned())
        .spawn(move || {
            link.send_reports(&queued, &move |conversation| {
                let _ = stale_conversations.send(conversation);
            });
        })?;
    let report: Reporter = Arc::new(move |report| {
        if reports.try_send(report).is_err() {
            eprintln!(
                "ai6: an assistant report was dropped: the queue to Universal Exports is full"
            );
        }
    });
    let desk = Desk::new(
        state.clone(),
        Arc::new(StationHarness(state.clone())),
        report,
        mcp_url,
    );
    let stopping = state.stopping.clone();
    Ok(Some(tokio::spawn(async move {
        while !stopping.load(Ordering::Acquire) {
            tokio::select! {
                job = receiver.recv() => match job {
                    Some(job) => desk.handle(job).await,
                    None => break,
                },
                Some(conversation) = stale.recv() => desk.close(&conversation),
                () = tokio::time::sleep(STOP_CHECK) => {}
            }
        }
        desk.close_all();
    })))
}

/// The desk's own link to Universal Exports, with the Station's credential.
#[derive(Clone)]
struct Link {
    origin: Url,
    token: String,
    station: String,
}

impl Link {
    fn agent(wait: Duration) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(2) + wait))
            .max_redirects(0)
            .build()
            .into()
    }

    fn take_jobs(&self, stopping: &AtomicBool, jobs: &mpsc::Sender<AssistantJob>) {
        let Ok(mut endpoint) = self.origin.join("v1/station/assistant/next") else {
            return;
        };
        endpoint
            .query_pairs_mut()
            .append_pair("station", &self.station)
            .append_pair("wait_ms", &JOB_WAIT_MS.to_string());
        let agent = Self::agent(Duration::from_millis(JOB_WAIT_MS));
        while !stopping.load(Ordering::Acquire) {
            let response = agent
                .get(endpoint.as_str())
                .header("Authorization", &format!("Bearer {}", self.token))
                .call();
            let job = match response {
                Ok(mut response) if response.status().as_u16() == 200 => response
                    .body_mut()
                    .with_config()
                    .limit(256 * 1024)
                    .read_to_vec()
                    .ok()
                    .and_then(|body| serde_json::from_slice::<AssistantJob>(&body).ok()),
                Ok(response) if response.status().as_u16() == 204 => continue,
                _ => {
                    std::thread::sleep(Duration::from_millis(500));
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

    /// Sends reports in order. When Universal Exports refuses a job's `accepted` report (it
    /// no longer waits for that job here: it moved to another Station or was given up),
    /// `stale` is told the conversation, so the harness this Station started for it closes.
    fn send_reports(
        &self,
        queued: &std::sync::mpsc::Receiver<AssistantReport>,
        stale: &(dyn Fn(String) + Send + Sync),
    ) {
        let Ok(endpoint) = self.origin.join("v1/station/assistant/reports") else {
            return;
        };
        let agent = Self::agent(Duration::ZERO);
        while let Ok(report) = queued.recv() {
            for _ in 0..3 {
                let sent = agent
                    .post(endpoint.as_str())
                    .header("Authorization", &format!("Bearer {}", self.token))
                    .send_json(&report);
                // A refused report (an unknown conversation) is not retried.
                match sent {
                    Err(ureq::Error::StatusCode(403 | 404 | 409))
                        if report.job_id.is_some()
                            && report
                                .events
                                .first()
                                .is_some_and(|event| event.kind == EVENT_ACCEPTED) =>
                    {
                        stale(report.conversation_id.clone());
                        break;
                    }
                    Ok(_) | Err(ureq::Error::StatusCode(_)) => break,
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::Path;

    use super::super::tests::test_state;
    use super::*;
    use crate::guatiao::library::ProviderError;

    /// Each start and resume, as the harness was asked for it.
    #[derive(Default)]
    struct Asked {
        starts: Vec<AgentStart>,
        resumes: Vec<AgentResume>,
        messages: Vec<String>,
    }

    /// What a scripted harness does with a message.
    #[derive(Clone, Copy, PartialEq)]
    enum Script {
        /// Answers `answer: <message>` and completes.
        Answers,
        /// Exits, as its handler reports it.
        Dies,
        /// Never writes anything.
        Hangs,
    }

    /// A harness that does what `script` says with every message; a resumed one does what
    /// `resumed` says, and one that dies when resumed never confirms its Session.
    struct Scripted {
        asked: Arc<Mutex<Asked>>,
        resumable: bool,
        script: Script,
        resumed: Script,
    }

    struct ScriptedSession {
        session_id: Option<SessionId>,
        events: Mutex<VecDeque<AgentEvent>>,
        asked: Arc<Mutex<Asked>>,
        script: Script,
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
                Script::Answers => {
                    events.push_back(event("system", Some("init")));
                    events.push_back(event(EVENT_TEXT, Some(&format!("answer: {message}"))));
                    events.push_back(event("completed", Some(&format!("answer: {message}"))));
                }
                Script::Dies => {
                    events.push_back(event("output", Some("Error: the stored login has expired")));
                    events.push_back(event("failed", Some(DIED)));
                }
                Script::Hangs => {}
            }
        }
    }

    impl AgentSession for ScriptedSession {
        fn session_id(&self) -> Result<Option<SessionId>, ProviderError> {
            Ok(self.session_id.clone())
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
            let next = self.events.lock().unwrap().pop_front();
            if next.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(next)
        }

        fn interrupt(&self) -> Result<(), ProviderError> {
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
            request: AgentStart,
        ) -> Result<Object<dyn AgentSession>, String> {
            let message = request.prompt.clone();
            self.asked.lock().unwrap().starts.push(request);
            let session = ScriptedSession {
                session_id: Some(SessionId::try_new("provider-assistant-1").unwrap()),
                events: Mutex::default(),
                asked: self.asked.clone(),
                script: self.script,
            };
            session.answer(&message);
            Ok(session.into_object())
        }

        fn resume(
            &self,
            _handler: &str,
            request: AgentResume,
        ) -> Result<Object<dyn AgentSession>, String> {
            if !self.resumable {
                return Err("no such provider Session".to_owned());
            }
            let message = request.message.clone().unwrap_or_default();
            let session_id = request.session_id.clone();
            self.asked.lock().unwrap().resumes.push(request);
            let session = ScriptedSession {
                session_id: (self.resumed != Script::Dies).then_some(session_id),
                events: Mutex::default(),
                asked: self.asked.clone(),
                script: self.resumed,
            };
            session.answer(&message);
            Ok(session.into_object())
        }
    }

    const AGENTS: &str = "[agent.moneypenny]\nhandler = \"mock-assistant\"\nmodel = \"sonnet\"\n\
                          effort = \"low\"\ncontext = \"You serve the operator.\"\n\
                          env = { CLAUDE_CODE_OAUTH_TOKEN = \"subscription\", LOADOUT_SECRET = \"no\" }\n\
                          [agent.007]\nhandler = \"mock-agent\"\n";

    struct Fixture {
        desk: Desk,
        asked: Arc<Mutex<Asked>>,
        reports: Arc<Mutex<Vec<AssistantReport>>>,
        _root: tempfile::TempDir,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.desk.close_all();
            if let Some(areas) = self.desk.areas.lock().unwrap().take() {
                let _ = std::fs::remove_dir_all(areas);
            }
        }
    }

    fn fixture(resumable: bool, idle: Duration) -> Fixture {
        scripted(resumable, idle, Script::Answers, Script::Answers)
    }

    fn scripted(resumable: bool, idle: Duration, script: Script, resumed: Script) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let state = test_state(root.path(), "", AGENTS);
        let asked = Arc::new(Mutex::new(Asked::default()));
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = reports.clone();
        let desk = Desk::new(
            state,
            Arc::new(Scripted {
                asked: asked.clone(),
                resumable,
                script,
                resumed,
            }),
            Arc::new(move |report| sink.lock().unwrap().push(report)),
            "https://exports.example/v1/moneypenny/mcp".to_owned(),
        )
        .with_limits(idle, 2);
        Fixture {
            desk,
            asked,
            reports,
            _root: root,
        }
    }

    fn job(conversation: &str, message: &str) -> AssistantJob {
        AssistantJob {
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
    fn a_harness_whose_accept_the_gateway_refused_is_closed() {
        for (status, closes) in [(409, vec!["c-late".to_owned()]), (204, Vec::new())] {
            let (origin, served) = refusing_gateway(status);
            let link = Link {
                origin,
                token: "station-token".to_owned(),
                station: "hq".to_owned(),
            };
            let (reports, queued) = std::sync::mpsc::sync_channel(4);
            let report = |job: Option<&str>, kind: &str| AssistantReport {
                station_id: "hq".to_owned(),
                conversation_id: "c-late".to_owned(),
                job_id: job.map(str::to_owned),
                session_id: None,
                events: vec![AssistantEvent::new(kind, None)],
            };
            reports.send(report(Some("job-1"), EVENT_ACCEPTED)).unwrap();
            reports.send(report(None, EVENT_TEXT)).unwrap();
            drop(reports);
            let closed = Mutex::new(Vec::new());
            link.send_reports(&queued, &|conversation| {
                closed.lock().unwrap().push(conversation);
            });
            assert_eq!(served.join().unwrap(), 2);
            assert_eq!(
                closed.into_inner().unwrap(),
                closes,
                "only a refused accept closes, at {status}"
            );
        }
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

    #[test]
    fn a_working_directory_still_in_use_for_a_moment_is_removed_once_free() {
        let root = new_root().unwrap();
        let area = root.join("0123456789abcdef0123456789abcdef");
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
        remove_area(&area.to_string_lossy());
        assert!(!area.exists());
        holder.wait().unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
