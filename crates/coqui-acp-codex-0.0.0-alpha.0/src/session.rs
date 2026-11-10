//! A running Codex app server: its process, its JSON-RPC stream, and one thread on it.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;

use crate::mode::Shown;
use coqui_acp_api::usage::Bucket;
use coqui_acp_harness::event::{Content, Event, Severity, context_block};
use coqui_acp_harness::lines::{self, Tail};
use coqui_acp_harness::mode::Mode;
use coqui_acp_harness::stream::Waiting;
use coqui_acp_harness::{LineHook, Process, Spawn, Step, Stream, exit_text, line_writer, version};
use serde_json::{Map, Value, json};

use crate::launch::{CodexLaunch, ThreadStart};
use crate::parse::{Parser, ParserLimits, Writer, id_text};
use crate::{Error, TESTED_VERSIONS};

/// The bounds and waits of a session. Every one is a setting with a default, and any value is taken.
#[derive(Clone)]
#[non_exhaustive]
pub struct CodexSettings {
    /// The longest line of the app server's output read; past it the session ends. Default 64 MiB.
    pub max_line: u64,
    /// How many of the last lines that were not protocol a failure quotes. Default 8.
    pub tail_lines: usize,
    /// The bytes of each of those lines. Default 512.
    pub tail_line_bytes: usize,
    /// How long the app server may take to answer `initialize`. Default 20 s.
    pub initialize_timeout: Duration,
    /// How long it may take to answer any other request: `thread/start`, `thread/resume`, `turn/start`,
    /// `model/list`. Default 30 s.
    pub request_timeout: Duration,
    /// How long the end of the output waits for the exit status. Default 1 s.
    pub exit_wait: Duration,
    /// The items kept for their deltas and results. Default 4096.
    pub max_tools: usize,
    /// The pages of `model/list` asked for. Default 10.
    pub max_model_pages: usize,
    /// The earlier turns a load replays, the latest kept. Default 1000.
    pub max_history_turns: usize,
    /// What the parser keeps and cuts.
    pub parser: ParserLimits,
    /// Whether to ask for the models when starting. Off, or when Codex does not answer, they are unknown. Default on.
    pub list_models: bool,
    /// How often Codex is asked whether it has exited while its output stays open. Default 100 ms.
    pub exit_poll: Duration,
    /// How long a close waits for it to have ended, as the caller may remove its directory next. Default 2 s.
    pub close_wait: Duration,
    /// The lines waiting to be written to it; past it a write fails, rather than wait for a process that has stopped
    /// reading. Default 1024.
    pub max_queued_writes: usize,
    /// The bytes of a session's title, from its first prompt. Default 80.
    pub title_bytes: usize,
    /// How long `codex --version` may take; 0 does not ask. Default 10 s.
    pub version_wait: Duration,
    /// How much of its answer is read. Default 4096.
    pub version_bytes: u64,
    /// The host's check of each line Codex writes, before anything reads it; `false` ends Codex.
    pub on_line: Option<LineHook>,
    /// When set, `config/read` is asked for before the thread opens and its answer given to this; a refusal ends
    /// Codex and the start fails.
    pub check_config: Option<AnswerCheck>,
    /// When set, the answer that opened the thread is given to this; a refusal ends Codex and the start fails.
    pub check_thread: Option<AnswerCheck>,
}

/// A host's check of one of Codex's answers: its refusal says why.
pub type AnswerCheck = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;

impl fmt::Debug for CodexSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexSettings")
            .field("max_line", &self.max_line)
            .field("tail_lines", &self.tail_lines)
            .field("tail_line_bytes", &self.tail_line_bytes)
            .field("initialize_timeout", &self.initialize_timeout)
            .field("request_timeout", &self.request_timeout)
            .field("exit_wait", &self.exit_wait)
            .field("max_tools", &self.max_tools)
            .field("max_model_pages", &self.max_model_pages)
            .field("max_history_turns", &self.max_history_turns)
            .field("parser", &self.parser)
            .field("list_models", &self.list_models)
            .field("exit_poll", &self.exit_poll)
            .field("close_wait", &self.close_wait)
            .field("max_queued_writes", &self.max_queued_writes)
            .field("title_bytes", &self.title_bytes)
            .field("version_wait", &self.version_wait)
            .field("version_bytes", &self.version_bytes)
            .field("on_line", &self.on_line.is_some())
            .field("check_config", &self.check_config.is_some())
            .field("check_thread", &self.check_thread.is_some())
            .finish()
    }
}

impl CodexSettings {
    /// The defaults.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_line: 64 * 1024 * 1024,
            tail_lines: 8,
            tail_line_bytes: 512,
            initialize_timeout: Duration::from_secs(20),
            request_timeout: Duration::from_secs(30),
            exit_wait: Duration::from_secs(1),
            max_tools: 4096,
            max_model_pages: 10,
            max_history_turns: 1000,
            parser: ParserLimits::new(),
            list_models: true,
            exit_poll: Duration::from_millis(100),
            close_wait: Duration::from_secs(2),
            max_queued_writes: 1024,
            title_bytes: 80,
            version_wait: Duration::from_secs(10),
            version_bytes: 4096,
            on_line: None,
            check_config: None,
            check_thread: None,
        }
    }
}

impl Default for CodexSettings {
    fn default() -> Self {
        Self::new()
    }
}

/// A model Codex offers, from `model/list`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Model {
    /// The id a turn names.
    pub id: String,
    /// Its name for people.
    pub name: String,
    /// What it is.
    pub description: Option<String>,
    /// The reasoning efforts it takes.
    pub efforts: Vec<String>,
    /// Its own effort when none is named.
    pub default_effort: Option<String>,
    /// Whether Codex uses it when none is named.
    pub is_default: bool,
}

/// The models of a `model/list` result, those Codex hides left out; `None` when it lists none at all, as distinct from an
/// empty list.
#[must_use]
pub fn models_of(result: &Value) -> Option<Vec<Model>> {
    let listed = result.get("data")?.as_array()?;
    Some(
        listed
            .iter()
            .filter(|model| model.get("hidden") != Some(&Value::Bool(true)))
            .filter_map(|model| {
                let id = model
                    .get("model")
                    .and_then(Value::as_str)
                    .or_else(|| model.get("id").and_then(Value::as_str))?;
                Some(Model {
                    id: id.to_string(),
                    name: model
                        .get("displayName")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_string(),
                    description: model
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    efforts: model
                        .get("supportedReasoningEfforts")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|effort| {
                            effort
                                .get("reasoningEffort")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                        .collect(),
                    default_effort: model
                        .get("defaultReasoningEffort")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    is_default: model.get("isDefault").and_then(Value::as_bool) == Some(true),
                })
            })
            .collect(),
    )
}

/// Codex's version in its `initialize` answer's `userAgent` (`<client>/<version> (…)`).
#[must_use]
pub fn version_of(answer: &Value) -> Option<String> {
    let agent = answer.get("userAgent")?.as_str()?;
    let (_, rest) = agent.split_once('/')?;
    Some(rest.split_whitespace().next()?.to_string())
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running Codex app server with one thread open. Its events go to the sink given to [`CodexSession::start`], from
/// a thread of its own; every method may be called from any thread.
pub struct CodexSession {
    thread: String,
    writer: Writer,
    process: Arc<dyn Process>,
    parser: Arc<Mutex<Parser>>,
    waiting: Waiting,
    ended: Arc<OnceLock<String>>,
    requests: AtomicU64,
    settings: CodexSettings,
    closed: Arc<AtomicBool>,
    version: Option<String>,
    models: Option<Vec<Model>>,
    choice: Mutex<(Option<String>, Option<String>)>,
    /// The mode in use and whether a client chose it: until one has, a turn names none of a mode's settings, and the
    /// thread's own configuration holds.
    mode: Mutex<(Option<Shown>, bool)>,
    /// The effort the thread answered with, and the developer instructions it was given: a collaboration mode takes the
    /// place of both, so each turn in a mode names them again.
    thread_effort: Option<String>,
    instructions: Option<String>,
    /// Whether the person has been told that a Plan turn's instructions are the session's own.
    plan_told: AtomicBool,
    /// The model the thread answered with, which a collaboration mode has to name.
    thread_model: Option<String>,
    /// The thread's `sandbox_workspace_write` table.
    workspace_write: Map<String, Value>,
    titled: AtomicBool,
    sink: Arc<dyn Fn(Event) -> bool + Send + Sync>,
}

impl fmt::Debug for CodexSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodexSession")
            .field("thread", &self.thread)
            .finish_non_exhaustive()
    }
}

impl CodexSession {
    /// Starts `codex app-server` as `launch` says, through `spawn`; initializes it; lists its models; sets the skill
    /// roots of `launch`'s directories; and starts or resumes its thread, checking that a resumed thread is the one
    /// asked for. For [`ThreadStart::Load`] the thread's earlier turns go to `sink` as history first. A version Codex
    /// was not tested with is an [`Event::Notice`], never a refusal. Events reach `sink` until it answers `false`;
    /// the output's end is one [`Event::Exited`].
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] for a `--config` key that cannot be written; [`Error::Spawn`] when it could not start; [`Error::Timeout`] or [`Error::Harness`] when a request failed, and
    /// the app server has been ended.
    pub fn start(
        launch: &CodexLaunch,
        spawn: &dyn Spawn,
        settings: CodexSettings,
        sink: impl Fn(Event) -> bool + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        let mut session = Self::connect(launch, spawn, settings, sink)?;
        if let Err(error) = session.open(launch) {
            session.close();
            return Err(error);
        }
        Ok(session)
    }

    /// Reads the account's allowances with no thread and no turn: starts `codex app-server` as `launch` says,
    /// initializes it, asks `account/rateLimits/read`, and ends it. This is what a poll with no session open costs.
    ///
    /// # Errors
    ///
    /// As [`CodexSession::start`], and as [`CodexSession::read_usage`].
    pub fn read_account_usage(
        launch: &CodexLaunch,
        spawn: &dyn Spawn,
        settings: CodexSettings,
    ) -> Result<Vec<Bucket>, Error> {
        let mut session = Self::connect(launch, spawn, settings, |_| true)?;
        let read = session
            .handshake(launch)
            .and_then(|()| session.read_usage());
        session.close();
        read
    }

    /// Starts the process and its readers, with no request made yet.
    fn connect(
        launch: &CodexLaunch,
        spawn: &dyn Spawn,
        settings: CodexSettings,
        sink: impl Fn(Event) -> bool + Send + Sync + 'static,
    ) -> Result<Self, Error> {
        if let Some((key, _)) = launch
            .config_overrides
            .iter()
            .find(|(key, _)| !crate::valid_config_key(key))
        {
            return Err(Error::Refused(format!(
                "{key:?} cannot be written as a --config key"
            )));
        }
        let sink: Arc<dyn Fn(Event) -> bool + Send + Sync> = Arc::new(sink);
        let spawned = spawn.spawn(&launch.launch()).map_err(Error::Spawn)?;
        let process: Arc<dyn Process> = Arc::from(spawned.process);
        // Past here the app server runs: a failure ends it, since no session exists yet to do so on drop.
        let ended_with = |error: io::Error| {
            let _ = process.kill();
            Error::Spawn(error)
        };
        let writer: Writer = line_writer(
            spawned.stdin,
            settings.max_queued_writes,
            "coqui-acp-codex-stdin",
        )
        .map_err(&ended_with)?;
        let parser = Arc::new(Mutex::new(Parser::with_limits(
            Arc::clone(&writer),
            settings.max_tools,
            settings.parser,
        )));
        let tail = Arc::new(Mutex::new(Tail::new(
            settings.tail_lines,
            settings.tail_line_bytes,
        )));
        let closed = Arc::new(AtomicBool::new(false));
        if let Some(stderr) = spawned.stderr {
            let tail = Arc::clone(&tail);
            let max_line = settings.max_line;
            thread::Builder::new()
                .name("coqui-acp-codex-stderr".into())
                .spawn(move || lines::read_diagnostics(stderr, max_line, &tail))
                .map_err(&ended_with)?;
        }
        let mut stream = Stream::new(
            "Codex app-server",
            Arc::clone(&process),
            tail,
            Arc::clone(&closed),
            Arc::clone(&sink),
        );
        stream.max_line = settings.max_line;
        stream.exit_poll = settings.exit_poll;
        stream.exit_wait = settings.exit_wait;
        stream.on_line.clone_from(&settings.on_line);
        let waiting = Arc::clone(&stream.waiting);
        let ended = Arc::clone(&stream.ended);
        {
            let parser = Arc::clone(&parser);
            stream
                .read(
                    spawned.stdout,
                    "coqui-acp-codex-stdout",
                    move |value| {
                        let mut parser = locked(&parser);
                        let events = parser.line(value);
                        Step::new(events, parser.answers())
                    },
                    Vec::new,
                )
                .map_err(&ended_with)?;
        }
        Ok(Self {
            thread: String::new(),
            writer,
            process,
            parser,
            waiting,
            ended,
            requests: AtomicU64::new(0),
            settings,
            closed,
            version: None,
            models: None,
            choice: Mutex::new((launch.model.clone(), launch.effort.clone())),
            mode: Mutex::new((None, false)),
            thread_model: None,
            thread_effort: None,
            instructions: launch.developer_instructions.clone(),
            plan_told: AtomicBool::new(false),
            workspace_write: launch
                .config()
                .get("sandbox_workspace_write")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            titled: AtomicBool::new(false),
            sink,
        })
    }

    /// Initializes the app server and tells it so.
    fn handshake(&mut self, launch: &CodexLaunch) -> Result<(), Error> {
        let initialized = self.request(
            "initialize",
            json!({"clientInfo": {"name": launch.client_info.name, "title": launch.client_info.title,
                                  "version": launch.client_info.version},
                   "capabilities": {}}),
            self.settings.initialize_timeout,
        )?;
        self.version = version_of(&initialized);
        self.notify("initialized", json!({}))?;
        let found = self.version.clone().unwrap_or_default();
        if let Some(warning) = version::untested("Codex", &found, TESTED_VERSIONS) {
            (self.sink)(Event::Notice {
                severity: Severity::Warning,
                title: warning,
            });
        }
        Ok(())
    }

    fn open(&mut self, launch: &CodexLaunch) -> Result<(), Error> {
        self.handshake(launch)?;
        if self.settings.list_models {
            self.list_models();
        }
        if let Some(check) = self.settings.check_config.clone() {
            let config = self.request(
                "config/read",
                json!({"cwd": launch.cwd.to_string_lossy()}),
                self.settings.request_timeout,
            )?;
            check(&config).map_err(Error::Refused)?;
        }
        let roots = launch.skill_roots();
        if !roots.is_empty() {
            self.request(
                "skills/extraRoots/set",
                json!({"extraRoots": roots}),
                self.settings.request_timeout,
            )?;
        }
        let (method, wanted) = match &launch.thread {
            ThreadStart::New => ("thread/start", None),
            ThreadStart::Resume(id) | ThreadStart::Load(id) => ("thread/resume", Some(id.clone())),
        };
        let opened = self.request(
            method,
            launch.thread_params(),
            self.settings.request_timeout,
        )?;
        let thread = opened
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Harness("Codex named no thread".into()))?;
        if let Some(wanted) = wanted {
            if wanted != thread {
                return Err(Error::Harness(format!(
                    "Codex resumed thread {thread}, not the one requested"
                )));
            }
            if !launch.working_roots.is_empty()
                && !opened
                    .pointer("/thread/cwd")
                    .and_then(Value::as_str)
                    .is_some_and(|cwd| {
                        coqui_acp_harness::acp::within_roots(Path::new(cwd), &launch.working_roots)
                    })
            {
                return Err(Error::Refused(
                    "that thread's directory is outside the directories this agent may work in"
                        .into(),
                ));
            }
        }
        if let Some(check) = self.settings.check_thread.clone() {
            check(&opened).map_err(Error::Refused)?;
        }
        self.thread = thread.to_string();
        self.thread_model = opened
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string);
        self.thread_effort = opened
            .get("reasoningEffort")
            .and_then(Value::as_str)
            .map(str::to_string);
        locked(&self.mode).0 = crate::mode::of_thread(&opened);
        locked(&self.parser).set_thread(thread);
        if matches!(launch.thread, ThreadStart::Load(_)) {
            let turns = opened
                .pointer("/thread/turns")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let skip = turns.len().saturating_sub(self.settings.max_history_turns);
            let writer: Writer =
                Arc::new(|_| Err(io::Error::other("a replayed turn asks nothing")));
            let mut history =
                Parser::with_limits(writer, self.settings.max_tools, self.settings.parser);
            if skip > 0 {
                (self.sink)(Event::Notice {
                    severity: Severity::Warning,
                    title: format!(
                        "Only the last {} turns of this thread were replayed",
                        self.settings.max_history_turns
                    ),
                });
            }
            for turn in turns.iter().skip(skip) {
                for event in history.history(turn) {
                    (self.sink)(event);
                }
            }
        }
        Ok(())
    }

    /// Reads the account's allowances with `account/rateLimits/read`, which needs no turn, and answers them as buckets.
    /// The answer becomes the account's last full report, which later sparse updates are merged into.
    ///
    /// # Errors
    ///
    /// [`Error::Harness`] when Codex refused (it is not signed in with an account that has allowances), or
    /// [`Error::Timeout`] when it did not answer.
    pub fn read_usage(&self) -> Result<Vec<Bucket>, Error> {
        let number = self.next_request();
        locked(&self.parser).expect_rate_limits_read(&json!(number));
        self.request_numbered(
            number,
            "account/rateLimits/read",
            json!({}),
            self.settings.request_timeout,
        )?;
        // The answer was applied where it was read, so an update read after it is kept.
        Ok(locked(&self.parser).rate_limits())
    }

    /// Asks for the models. Their being unknown stops nothing: a notice says so.
    fn list_models(&mut self) {
        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..self.settings.max_model_pages {
            let mut params = json!({});
            if let Some(cursor) = &cursor {
                params["cursor"] = json!(cursor);
            }
            let page = match self.request("model/list", params, self.settings.request_timeout) {
                Ok(page) => page,
                Err(error) => {
                    (self.sink)(Event::Notice {
                        severity: Severity::Warning,
                        title: format!("Codex's models are not known: {error}"),
                    });
                    return;
                }
            };
            let Some(listed) = models_of(&page) else {
                return;
            };
            models.extend(listed);
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|next| !next.is_empty())
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        self.models = Some(models);
    }

    fn write(&self, message: &Value) -> Result<(), Error> {
        (self.writer)(message)
            .map_err(|error| Error::Harness(format!("Codex could not be written to: {error}")))
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), Error> {
        self.write(&json!({"method": method, "params": params}))
    }

    /// Sends one request of the app server's protocol and waits up to `limit` for its answer.
    ///
    /// # Errors
    ///
    /// [`Error::Harness`] when Codex refused or could not be written to, [`Error::Timeout`] when it did not answer.
    pub fn request(&self, method: &str, params: Value, limit: Duration) -> Result<Value, Error> {
        self.request_numbered(self.next_request(), method, params, limit)
    }

    fn next_request(&self) -> u64 {
        self.requests.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn request_numbered(
        &self,
        number: u64,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value, Error> {
        let id = id_text(&json!(number));
        let (answer, answered) = mpsc::channel();
        locked(&self.waiting).insert(id.clone(), answer);
        if let Some(reason) = self.ended.get() {
            locked(&self.waiting).remove(&id);
            return Err(Error::Harness(reason.clone()));
        }
        if let Err(error) = self.write(&json!({"id": number, "method": method, "params": params})) {
            locked(&self.waiting).remove(&id);
            return Err(error);
        }
        let result = answered.recv_timeout(limit);
        locked(&self.waiting).remove(&id);
        match result {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(refusal)) => Err(Error::Harness(format!("Codex refused {method}: {refusal}"))),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Error::Timeout {
                what: "a request",
                limit,
            }),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(Error::Harness("Codex ended before it answered".into()))
            }
        }
    }

    /// The thread's id.
    #[must_use]
    pub fn thread(&self) -> &str {
        &self.thread
    }

    /// Codex's version, from its answer to `initialize`.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// The models Codex offers; `None` when they are not known.
    #[must_use]
    pub fn models(&self) -> Option<&[Model]> {
        self.models.as_deref()
    }

    /// The model and effort the next turn names, when chosen.
    #[must_use]
    pub fn choice(&self) -> (Option<String>, Option<String>) {
        locked(&self.choice).clone()
    }

    /// The mode the thread is in, `None` while its approval policy and sandbox are a combination no mode names.
    #[must_use]
    pub fn mode(&self) -> Option<Shown> {
        locked(&self.mode).0.clone()
    }

    /// Puts the turns that follow in `mode`: each names its approval policy, sandbox, reviewer and collaboration mode.
    pub fn set_mode(&self, mode: Mode) {
        *locked(&self.mode) = (Some(Shown::Known(mode)), true);
    }

    /// Sets the model the turns that follow name; `None` leaves Codex's own.
    pub fn set_model(&self, model: Option<&str>) {
        locked(&self.choice).0 = model.map(str::to_string);
    }

    /// Sets the reasoning effort the turns that follow name; `None` leaves the model's own.
    pub fn set_effort(&self, effort: Option<&str>) {
        locked(&self.choice).1 = effort.map(str::to_string);
    }

    /// Starts a turn with a message of the person's: text, images, links and resources, with the model and effort
    /// chosen. Returns once Codex has taken it.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] for a resource Codex cannot read; [`Error::Harness`] or [`Error::Timeout`] when Codex did
    /// not take the turn.
    pub fn prompt(&self, content: &[Content]) -> Result<(), Error> {
        let input = user_input(content)?;
        let (model, effort) = self.choice();
        let mut params = json!({"threadId": self.thread, "input": input});
        if let Some(model) = model {
            params["model"] = json!(model);
        }
        if let Some(effort) = &effort {
            params["effort"] = json!(effort);
        }
        let (mode, chosen) = locked(&self.mode).clone();
        if let (Some(Shown::Known(mode)), true) = (mode, chosen) {
            // The model named for the turn, else the thread's, else the one Codex lists as its own.
            let named = params["model"]
                .as_str()
                .map(str::to_string)
                .or_else(|| self.thread_model.clone())
                .or_else(|| {
                    self.models
                        .as_deref()
                        .and_then(|models| models.iter().find(|model| model.is_default))
                        .map(|model| model.id.clone())
                });
            if mode == Mode::Plan
                && self.instructions.is_some()
                && !self.plan_told.swap(true, Ordering::SeqCst)
            {
                (self.sink)(Event::Notice {
                    severity: Severity::Info,
                    title: "This session's developer instructions replace Codex's built-in Plan instructions in Plan turns"
                        .into(),
                });
            }
            if named.is_none() {
                (self.sink)(Event::Notice {
                    severity: Severity::Warning,
                    title: "Codex named no model, so this turn is in the mode's approval and sandbox only".into(),
                });
            }
            for (key, value) in crate::mode::turn_params(
                mode,
                named.as_deref(),
                effort.as_deref().or(self.thread_effort.as_deref()),
                self.instructions.as_deref(),
                &self.workspace_write,
            ) {
                params[key] = value;
            }
        }
        locked(&self.parser).prompting();
        let number = self.next_request();
        let started = self
            .request_numbered(number, "turn/start", params, self.settings.request_timeout)
            .inspect_err(|error| {
                // No answer does not mean no turn: this one has failed, and must not run on unwatched.
                if matches!(error, Error::Timeout { .. }) {
                    locked(&self.parser).abandon(&id_text(&json!(number)));
                }
            })?;
        if let Some(turn) = started.pointer("/turn/id").and_then(Value::as_str) {
            let mut parser = locked(&self.parser);
            if parser.turn().is_none() {
                parser.started(turn);
            }
        }
        if !self.titled.swap(true, Ordering::SeqCst)
            && let Some(title) = content.iter().find_map(|block| match block {
                Content::Text(text) => text.lines().find(|line| !line.trim().is_empty()),
                _ => None,
            })
        {
            (self.sink)(Event::Title(
                coqui_acp_harness::lines::cut(title.trim(), self.settings.title_bytes).to_string(),
            ));
        }
        Ok(())
    }

    /// Interrupts the running turn (`turn/interrupt`), which then ends as cancelled; nothing when none runs. Its answer
    /// is not awaited.
    ///
    /// # Errors
    ///
    /// [`Error::Harness`] when it could not be written.
    pub fn interrupt(&self) -> Result<(), Error> {
        let turn = {
            let mut parser = locked(&self.parser);
            let Some(turn) = parser.turn().map(str::to_string) else {
                return Ok(());
            };
            parser.interrupting();
            turn
        };
        let number = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
        self.write(&json!({"id": number, "method": "turn/interrupt", "params": {"threadId": self.thread, "turnId": turn}}))
    }

    /// Adds to a running turn (`turn/steer`), as after a refusal that should not end it: `turn` is the turn it is meant
    /// for, as a question's `turnId` names it, and Codex refuses it for any other. Its answer is not awaited.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] for a resource Codex cannot read; [`Error::Harness`] when it could not be written.
    pub fn steer(&self, turn: &str, content: &[Content]) -> Result<(), Error> {
        let input = user_input(content)?;
        let number = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
        self.write(&json!({"id": number, "method": "turn/steer", "params": {
            "threadId": self.thread, "expectedTurnId": turn, "input": input}}))
    }

    /// Ends the app server and everything it started, and waits up to `close_wait` for it to have gone.
    pub fn close(&self) {
        if !self.closed.swap(true, Ordering::SeqCst) {
            let _ = self.process.kill();
            let _ = exit_text(self.process.as_ref(), self.settings.close_wait);
        }
    }
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        self.close();
    }
}

/// A prompt as Codex's turn input: text, an image as a data URL, a link as a mention, and a text resource in a
/// `<context>` block. A resource's bytes are taken only for an image.
///
/// # Errors
///
/// [`Error::Refused`] for a resource of bytes that is not an image.
pub fn user_input(content: &[Content]) -> Result<Vec<Value>, Error> {
    let text = |text: String| json!({"type": "text", "text": text, "text_elements": []});
    let mut input = Vec::new();
    for block in content {
        input.push(match block {
            Content::Text(said) => text(said.clone()),
            Content::Image { mime_type, data } => {
                json!({"type": "image", "url": format!("data:{mime_type};base64,{data}")})
            }
            Content::ResourceLink { uri, name } => {
                let name = if name.is_empty() { uri } else { name };
                text(format!("[@{name}]({uri})"))
            }
            Content::Resource {
                uri,
                text: Some(said),
                ..
            } => text(context_block(uri, said)),
            Content::Resource {
                mime_type: Some(mime_type),
                blob: Some(data),
                ..
            } if mime_type.starts_with("image/") => {
                json!({"type": "image", "url": format!("data:{mime_type};base64,{data}")})
            }
            Content::Resource { .. } => {
                return Err(Error::Refused(
                    "Codex reads a resource's bytes only for an image".into(),
                ));
            }
            _ => return Err(Error::Refused("Codex does not take that content".into())),
        });
    }
    Ok(input)
}
