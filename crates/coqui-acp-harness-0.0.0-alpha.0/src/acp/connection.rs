//! One client connection: JSON-RPC over lines, the version spoken, and the sessions it opened.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use core::time::Duration;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;

use coqui_acp_api::usage::{Bucket, LimitHit, Spend};
use coqui_acp_api::{USAGE_CAPABILITY, USAGE_METHOD, USAGE_NOTIFICATION, USAGE_VERSION};
use serde_json::{Value, json};

use super::session::{Item, Session};
use super::usage::{UsageTracker, now};
use super::{EventSink, Harness, HarnessError, Open, OpenRequest, Settings, parse, shape};
use crate::lines::{self, Line};

pub(crate) const PARSE_ERROR: i64 = -32700;
pub(crate) const INVALID_REQUEST: i64 = -32600;
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;
pub(crate) const INVALID_PARAMS: i64 = -32602;
pub(crate) const INTERNAL_ERROR: i64 = -32603;

/// Serves `harness` to one client over `input` and `output`, one JSON-RPC message per line, on the calling thread
/// until the input ends. Then every session it opened is closed.
///
/// `initialize` answers the version the client asks for, up to [`Settings::max_version`]: version 2's shapes to a
/// client that asks for 2 or more, version 1's otherwise.
///
/// # Errors
///
/// Reading the input failed, or a line was longer than [`Settings::max_line`].
pub fn serve(
    harness: Arc<dyn Harness>,
    settings: Settings,
    input: impl Read,
    output: impl Write + Send + 'static,
) -> io::Result<()> {
    let usage = UsageTracker::new(harness.usage(), settings.account.clone());
    let shared = Arc::new(Shared {
        harness,
        settings,
        usage: Mutex::new(usage),
        out: Mutex::new(Box::new(output)),
        version: Mutex::new(None),
        sessions: Mutex::new(Sessions::default()),
        pending: Mutex::new(BTreeMap::new()),
        next_id: AtomicU64::new(1),
        busy: Arc::new(AtomicUsize::new(0)),
        ended: AtomicBool::new(false),
    });
    let mut too_long = false;
    let read = lines::read_lines(input, shared.settings.max_line, |line| {
        match line {
            Line::Json(message) => shared.receive(message),
            Line::Text(_) => {
                shared.respond_error(&Value::Null, PARSE_ERROR, "not a JSON-RPC message")
            }
            Line::TooLong(_) => {
                too_long = true;
                return false;
            }
        }
        true
    });
    shared.ended.store(true, Ordering::SeqCst);
    let waiting: Vec<_> = core::mem::take(&mut *shared.pending())
        .into_values()
        .collect();
    for waiter in waiting {
        let _ = waiter.send(Err("the client is gone".into()));
    }
    let sessions: Vec<Arc<Session>> = core::mem::take(&mut shared.sessions().open)
        .into_values()
        .collect();
    for session in sessions {
        session.close(&shared);
    }
    read?;
    if too_long {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a line was longer than the limit",
        ));
    }
    Ok(())
}

/// The client's answers awaited, by the id of our request.
type Pending = BTreeMap<u64, mpsc::Sender<Result<Value, String>>>;

/// The sessions open, and how many are being opened, which count against the limit too.
#[derive(Default)]
struct Sessions {
    open: BTreeMap<String, Arc<Session>>,
    opening: usize,
}

pub(crate) struct Shared {
    harness: Arc<dyn Harness>,
    pub(crate) settings: Settings,
    out: Mutex<Box<dyn Write + Send>>,
    version: Mutex<Option<u8>>,
    usage: Mutex<UsageTracker>,
    sessions: Mutex<Sessions>,
    pending: Mutex<Pending>,
    next_id: AtomicU64,
    busy: Arc<AtomicUsize>,
    ended: AtomicBool,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn sessions(&self) -> MutexGuard<'_, Sessions> {
        locked(&self.sessions)
    }

    fn pending(&self) -> MutexGuard<'_, Pending> {
        locked(&self.pending)
    }

    fn version(&self) -> Option<u8> {
        *locked(&self.version)
    }

    /// Tells the host something that is no message to the client.
    pub(crate) fn note(&self, text: &str) {
        if let Some(note) = &self.settings.on_notice {
            note(&crate::lines::one_line(text));
        }
    }

    fn write(&self, mut message: Value) {
        message["jsonrpc"] = json!("2.0");
        let Ok(mut line) = serde_json::to_vec(&message) else {
            return;
        };
        line.push(b'\n');
        let mut out = locked(&self.out);
        // A client that has gone is the reader's to notice.
        let _ = out.write_all(&line).and_then(|()| out.flush());
    }

    pub(crate) fn respond(&self, id: &Value, result: Value) {
        self.write(json!({"id": id, "result": result}));
    }

    pub(crate) fn respond_error(&self, id: &Value, code: i64, message: &str) {
        self.write(json!({"id": id, "error": {"code": code, "message": message}}));
    }

    pub(crate) fn respond_error_data(&self, id: &Value, code: i64, message: &str, data: Value) {
        self.write(json!({"id": id, "error": {"code": code, "message": message, "data": data}}));
    }

    pub(crate) fn notify(&self, method: &str, params: Value) {
        self.write(json!({"method": method, "params": params}));
    }

    /// Asks the client: the request's id, and where its answer comes.
    pub(crate) fn begin_request(
        &self,
        method: &str,
        params: Value,
    ) -> (u64, mpsc::Receiver<Result<Value, String>>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (answer, answered) = mpsc::channel();
        if self.ended.load(Ordering::SeqCst) {
            let _ = answer.send(Err("the client is gone".into()));
            return (id, answered);
        }
        self.pending().insert(id, answer);
        self.write(json!({"id": id, "method": method, "params": params}));
        (id, answered)
    }

    /// Waits for the answer to request `id` up to `limit`.
    pub(crate) fn wait(
        &self,
        id: u64,
        answered: &mpsc::Receiver<Result<Value, String>>,
        limit: Option<Duration>,
    ) -> Result<Value, String> {
        let result = match limit {
            Some(limit) => answered
                .recv_timeout(limit)
                .map_err(|_| "no answer in time".to_string()),
            None => answered
                .recv()
                .map_err(|_| "the client is gone".to_string()),
        };
        self.pending().remove(&id);
        result?
    }

    /// Gives up the requests `ids`: each is answered as withdrawn now, and the client's answer, if it comes, is
    /// dropped.
    pub(crate) fn withdraw(&self, ids: &[u64]) {
        for id in ids {
            if let Some(waiter) = self.pending().remove(id) {
                let _ = waiter.send(Err("withdrawn".into()));
            }
        }
    }

    fn receive(self: &Arc<Self>, message: Value) {
        let Some(method) = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            // An answer to one of our requests.
            let Some(id) = message.get("id").and_then(Value::as_u64) else {
                return;
            };
            if let Some(waiting) = self.pending().remove(&id) {
                let answer = match message.get("error") {
                    Some(error) => Err(error.to_string()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = waiting.send(answer);
            }
            return;
        };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = message.get("id").cloned() else {
            if method == "session/cancel"
                && let Ok(session) = parse::session_id(&params)
                && let Some(session) = self.sessions().open.get(&session).cloned()
            {
                session.cancel(self);
            }
            return;
        };
        match method.as_str() {
            "initialize" => {
                // A harness may take a while to say its name, as a command line asked its version does.
                self.take_version(&params);
                self.work(id, method, params, None);
            }
            "authenticate" => self.respond(&id, json!({})),
            _ if self.version().is_none() => {
                self.respond_error(&id, INVALID_REQUEST, "initialize first")
            }
            "session/prompt" => {
                // The turn is taken here, before the next line is read, so a cancel that follows finds it.
                let session = match self.session(&params) {
                    Ok(session) => session,
                    Err(why) => return self.respond_error(&id, INVALID_PARAMS, &why),
                };
                match session.begin(&id) {
                    Ok(number) => self.work(id, method, params, Some((session, number))),
                    Err((code, why)) => self.respond_error(&id, code, &why),
                }
            }
            "session/new"
            | "session/load"
            | "session/resume"
            | "session/set_config_option"
            | USAGE_METHOD
            | "session/close" => {
                self.work(id, method, params, None);
            }
            _ => self.respond_error(&id, METHOD_NOT_FOUND, "method not found"),
        }
    }

    /// Takes the version the client asked for, on the thread that reads it, so its next line finds it.
    fn take_version(&self, params: &Value) {
        let asked = params
            .get("protocolVersion")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let version: u8 = if asked >= 2 && self.settings.max_version >= 2 {
            2
        } else {
            1
        };
        *locked(&self.version) = Some(version);
        // Version 2 names the client's capabilities as version 1 names the agent's, without the prefix.
        let asked = ["clientCapabilities", "capabilities"]
            .iter()
            .find_map(|key| params.get(*key))
            .and_then(|capabilities| capabilities.get("_meta"))
            .and_then(|meta| meta.get(USAGE_CAPABILITY));
        if let Some(asked) = asked {
            let account = asked
                .get("account")
                .and_then(Value::as_str)
                .map(str::to_string);
            let notify = asked.get("notify").and_then(Value::as_bool) == Some(true);
            locked(&self.usage).client(notify, account);
        }
    }

    /// What `initialize` says of the usage extension: nothing for a harness that reports none.
    fn usage_capability(&self) -> Option<Value> {
        let support = self.harness.usage();
        support.reports().then(|| {
            let mut capability = json!({
                "version": USAGE_VERSION,
                "sources": support.sources.iter().map(coqui_acp_api::Source::as_str).collect::<Vec<_>>(),
                "spend": support.spend,
            });
            if support.needs_turn {
                capability["needs_turn"] = json!(true);
            }
            if support.needs_session {
                capability["needs_session"] = json!(true);
            }
            capability
        })
    }

    /// Applies `change` to the connection's report and, when it changed one, tells a client that asked
    /// to be told: the whole report, for `session` when the change came from one.
    pub(crate) fn usage_changed(
        &self,
        session: Option<&str>,
        change: impl FnOnce(&mut UsageTracker, i64) -> bool,
    ) {
        let at = now();
        let told = {
            let mut usage = locked(&self.usage);
            if !usage.reports() || !change(&mut usage, at) {
                return;
            }
            usage.notifies().then(|| usage.snapshot(session, at))
        };
        if let Some(report) = told
            && let Ok(report) = serde_json::to_value(&report)
        {
            self.notify(
                USAGE_NOTIFICATION,
                json!({"sessionId": session, "report": report}),
            );
        }
    }

    pub(crate) fn usage_buckets(&self, session: &str, buckets: Vec<Bucket>) {
        self.usage_changed(Some(session), |usage, at| usage.buckets(buckets, at));
    }

    pub(crate) fn usage_spend(&self, session: &str, spend: Spend) {
        self.usage_changed(Some(session), |usage, _| usage.spend(session, spend));
    }

    pub(crate) fn usage_limit(&self, session: &str, hit: LimitHit) {
        self.usage_changed(Some(session), |usage, at| usage.limit(hit, at));
    }

    pub(crate) fn usage_support(&self) -> coqui_acp_api::UsageSupport {
        self.harness.usage()
    }

    /// Re-reads the account's usage from the harness before an answer, when the harness can without
    /// a turn: from an open session's running harness, else with none open.
    fn refresh_usage(&self) {
        let support = self.harness.usage();
        if support.needs_turn {
            return;
        }
        // One session is asked, so a read that times out costs one wait, not one for each open session.
        let asked = self
            .sessions()
            .open
            .values()
            .rev()
            .find_map(|session| session.conversation());
        let read = match asked {
            Some(conversation) => conversation.read_usage(),
            // Nothing is started for a harness whose figures come from a session's process.
            None if support.needs_session => None,
            None => self.harness.read_usage(),
        };
        if let Some(buckets) = read {
            self.usage_changed(None, |usage, at| usage.buckets(buckets, at));
        }
    }

    /// `_coqui_acp/usage`: the report, with a session's spend when one is named.
    fn usage_report(&self, id: &Value, params: &Value) {
        if !locked(&self.usage).reports() {
            return self.respond_error(id, METHOD_NOT_FOUND, "method not found");
        }
        let session = match params.get("sessionId").filter(|named| !named.is_null()) {
            None => None,
            Some(named) => match named.as_str() {
                Some(named) if self.sessions().open.contains_key(named) => Some(named.to_string()),
                _ => {
                    return self.respond_error(
                        id,
                        INVALID_PARAMS,
                        "no such session is open on this connection",
                    );
                }
            },
        };
        self.refresh_usage();
        let report = locked(&self.usage).snapshot(session.as_deref(), now());
        match serde_json::to_value(&report) {
            Ok(report) => self.respond(id, report),
            Err(error) => self.respond_error(id, INTERNAL_ERROR, &error.to_string()),
        }
    }

    fn initialize(&self, id: &Value) {
        let version = self.version().unwrap_or(1);
        let info = self.harness.info();
        let takes = self.harness.takes();
        let info = json!({"name": info.name, "title": info.title, "version": info.version});
        let present = |taken: bool| taken.then(|| json!({}));
        if version >= 2 {
            let mut session = json!({"prompt": {}, "mcp": {"stdio": {}}, "close": {}});
            if takes.images {
                session["prompt"]["image"] = json!({});
            }
            if takes.embedded_context {
                session["prompt"]["embeddedContext"] = json!({});
            }
            if takes.mcp_http {
                session["mcp"]["http"] = json!({});
            }
            for (key, taken) in [
                ("additionalDirectories", takes.additional_directories),
                ("systemPrompt", takes.system_prompt),
            ] {
                if let Some(value) = present(taken) {
                    session[key] = value;
                }
            }
            let mut capabilities = json!({"session": session});
            if let Some(usage) = self.usage_capability() {
                capabilities["_meta"] = json!({ USAGE_CAPABILITY: usage });
            }
            self.respond(id, json!({"protocolVersion": 2, "info": info, "capabilities": capabilities, "authMethods": []}));
            return;
        }
        let mut sessions = json!({"resume": {}, "close": {}});
        for (key, taken) in [
            ("additionalDirectories", takes.additional_directories),
            ("systemPrompt", takes.system_prompt),
        ] {
            if let Some(value) = present(taken) {
                sessions[key] = value;
            }
        }
        let mut capabilities = json!({
            "loadSession": takes.load,
            "promptCapabilities": {"image": takes.images, "audio": false, "embeddedContext": takes.embedded_context},
            "mcpCapabilities": {"http": takes.mcp_http, "sse": takes.mcp_sse},
            "sessionCapabilities": sessions,
        });
        if let Some(usage) = self.usage_capability() {
            capabilities["_meta"] = json!({ USAGE_CAPABILITY: usage });
        }
        self.respond(
            id,
            json!({
                "protocolVersion": 1,
                "agentCapabilities": capabilities,
                "agentInfo": info,
                "authMethods": [],
            }),
        );
    }

    /// Works on a request on a thread of its own, within [`Settings::max_requests`].
    fn work(
        self: &Arc<Self>,
        id: Value,
        method: String,
        params: Value,
        turn: Option<(Arc<Session>, u64)>,
    ) {
        if self.busy.fetch_add(1, Ordering::SeqCst) >= self.settings.max_requests {
            self.busy.fetch_sub(1, Ordering::SeqCst);
            if let Some((session, number)) = &turn {
                session.abort(*number);
            }
            self.respond_error(&id, INTERNAL_ERROR, "too many requests at once");
            return;
        }
        let shared = Arc::clone(self);
        let busy = Arc::clone(&self.busy);
        let aborted = turn.clone();
        let answer_to = id.clone();
        let spawned = thread::Builder::new()
            .name("coqui-acp-harness-request".into())
            .spawn(move || {
                match turn {
                    Some((session, number)) => shared.prompt(&id, &params, &session, number),
                    None => shared.handle(&id, &method, &params),
                }
                busy.fetch_sub(1, Ordering::SeqCst);
            });
        if let Err(error) = spawned {
            self.busy.fetch_sub(1, Ordering::SeqCst);
            if let Some((session, number)) = aborted {
                session.abort(number);
            }
            self.respond_error(
                &answer_to,
                INTERNAL_ERROR,
                &format!("the request could not start: {error}"),
            );
        }
    }

    fn prompt(self: &Arc<Self>, id: &Value, params: &Value, session: &Arc<Session>, number: u64) {
        let size = params
            .get("prompt")
            .map_or(0, |prompt| prompt.to_string().len());
        if size > self.settings.max_prompt_bytes {
            session.abort(number);
            return self.respond_error(id, INVALID_PARAMS, "the prompt is larger than the limit");
        }
        match parse::prompt(params, self.harness.takes()) {
            Ok(content) => {
                let blocks = params
                    .get("prompt")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                session.send(self, id, number, &blocks, content);
            }
            Err(why) => {
                session.abort(number);
                self.respond_error(id, INVALID_PARAMS, &why);
            }
        }
    }

    fn handle(self: &Arc<Self>, id: &Value, method: &str, params: &Value) {
        let version = self.version().unwrap_or(1);
        match method {
            "initialize" => self.initialize(id),
            USAGE_METHOD => self.usage_report(id, params),
            "session/new" => self.open(id, params, Open::New, version),
            "session/load" => match parse::session_id(params) {
                Ok(session) => self.open(id, params, Open::Load(session), version),
                Err(why) => self.respond_error(id, INVALID_PARAMS, &why),
            },
            "session/resume" => {
                let session = match parse::session_id(params) {
                    Ok(session) => session,
                    Err(why) => return self.respond_error(id, INVALID_PARAMS, &why),
                };
                let open = match params.get("replayFrom").filter(|from| !from.is_null()) {
                    None => Open::Resume(session),
                    Some(from) if from.get("type").and_then(Value::as_str) == Some("start") => {
                        Open::Load(session)
                    }
                    Some(_) => {
                        return self.respond_error(
                            id,
                            INVALID_PARAMS,
                            "replay may start only from the start",
                        );
                    }
                };
                self.open(id, params, open, version);
            }
            "session/set_config_option" => {
                let session = match self.session(params) {
                    Ok(session) => session,
                    Err(why) => return self.respond_error(id, INVALID_PARAMS, &why),
                };
                let (option, value) = match parse::config_choice(params) {
                    Ok(choice) => choice,
                    Err(why) => return self.respond_error(id, INVALID_PARAMS, &why),
                };
                let Some(conversation) = session.conversation() else {
                    return self.respond_error(id, INVALID_PARAMS, "the session is closed");
                };
                match conversation.set_option(&option, &value) {
                    Ok(options) => self.respond(
                        id,
                        json!({"configOptions": shape::config_options(&options, version)}),
                    ),
                    Err(error) => self.refuse(id, &error),
                }
            }
            "session/close" => {
                let session = match self.session(params) {
                    Ok(session) => session,
                    Err(why) => return self.respond_error(id, INVALID_PARAMS, &why),
                };
                self.sessions().open.remove(session.id());
                session.close(self);
                self.respond(id, json!({}));
            }
            _ => self.respond_error(id, METHOD_NOT_FOUND, "method not found"),
        }
    }

    fn session(&self, params: &Value) -> Result<Arc<Session>, String> {
        let id = parse::session_id(params)?;
        self.sessions()
            .open
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("no session {id} on this connection"))
    }

    fn refuse(&self, id: &Value, error: &HarnessError) {
        match error {
            HarnessError::Refused(why) => self.respond_error(id, INVALID_PARAMS, why),
            HarnessError::Failed(why) => self.respond_error(id, INTERNAL_ERROR, why),
        }
    }

    /// What the host does not let a client choose: MCP servers when they are off, a path outside the working roots,
    /// and a network or device path unless allowed.
    fn admitted(&self, request: &OpenRequest) -> Result<(), String> {
        if !self.settings.client_mcp_servers && !request.mcp_servers.is_empty() {
            return Err("this agent takes no MCP servers from a client".into());
        }
        let mut paths: Vec<&Path> = alloc::vec![request.cwd.as_path()];
        paths.extend(
            request
                .additional_directories
                .iter()
                .map(std::path::PathBuf::as_path),
        );
        for path in &paths {
            super::path_admitted(
                path,
                &self.settings.working_roots,
                self.settings.network_paths,
            )?;
        }
        for server in &request.mcp_servers {
            if let super::McpTransport::Stdio { command, .. } = &server.transport
                && !self.settings.network_paths
                && parse::network(Path::new(command))
            {
                return Err(
                    "an MCP server's command on a network or device path is not taken".into(),
                );
            }
        }
        Ok(())
    }

    fn open(self: &Arc<Self>, id: &Value, params: &Value, open: Open, version: u8) {
        let takes = self.harness.takes();
        if matches!(open, Open::Load(_)) && !takes.load {
            return self.respond_error(id, METHOD_NOT_FOUND, "this agent does not load sessions");
        }
        let named = match &open {
            Open::Resume(session) | Open::Load(session) => Some(session.clone()),
            _ => None,
        };
        let replays = matches!(open, Open::Load(_));
        let new = matches!(open, Open::New);
        let request = match parse::open_request(params, open, takes, version) {
            Ok(mut request) => {
                request
                    .working_roots
                    .clone_from(&self.settings.working_roots);
                request.network_paths = self.settings.network_paths;
                request
            }
            Err(why) => return self.respond_error(id, INVALID_PARAMS, &why),
        };
        if let Err(why) = self.admitted(&request) {
            return self.respond_error(id, INVALID_PARAMS, &why);
        }
        {
            let mut sessions = self.sessions();
            if named
                .as_ref()
                .is_some_and(|named| sessions.open.contains_key(named))
            {
                drop(sessions);
                return self.respond_error(
                    id,
                    INVALID_PARAMS,
                    "that session is already open on this connection",
                );
            }
            if sessions.open.len() + sessions.opening >= self.settings.max_sessions {
                drop(sessions);
                return self.respond_error(
                    id,
                    INTERNAL_ERROR,
                    "this connection holds as many sessions as it may",
                );
            }
            sessions.opening += 1;
        }
        // The pump runs from the start, so a harness that tells much while it opens (a history) is never held up;
        // only a load's history is told before its answer.
        let (sender, items) = mpsc::sync_channel(self.settings.max_queued_events);
        let session = Arc::new(Session::new(named, version, &self.settings));
        {
            let pumped = Arc::clone(&session);
            let shared = Arc::clone(self);
            let spawned = thread::Builder::new()
                .name("coqui-acp-harness-session".into())
                .spawn(move || pumped.pump(&shared, items, replays));
            if let Err(error) = spawned {
                self.sessions().opening -= 1;
                return self.respond_error(
                    id,
                    INTERNAL_ERROR,
                    &format!("the session could not start: {error}"),
                );
            }
        }
        let opened = self.harness.open(request, EventSink(sender.clone()));
        let conversation = match opened {
            Ok(conversation) => conversation,
            Err(error) => {
                self.sessions().opening -= 1;
                return self.refuse(id, &error);
            }
        };
        let session_id = conversation.session_id();
        let options = shape::config_options(&conversation.options(), version);
        session.set(session_id.clone(), conversation);
        {
            let mut sessions = self.sessions();
            sessions.opening -= 1;
            if sessions.open.contains_key(&session_id) || self.ended.load(Ordering::SeqCst) {
                drop(sessions);
                session.close(self);
                return self.respond_error(
                    id,
                    INVALID_PARAMS,
                    "that session is already open on this connection",
                );
            }
            sessions
                .open
                .insert(session_id.clone(), Arc::clone(&session));
        }
        let result = if new {
            json!({"sessionId": session_id, "configOptions": options})
        } else {
            json!({"configOptions": options})
        };
        if replays {
            // After the replayed history, which the harness has queued.
            let _ = sender.send(Item::Reply {
                id: id.clone(),
                result,
            });
        } else {
            self.respond(id, result);
            let _ = sender.send(Item::Release);
        }
        session.read_usage_in_background(self);
    }
}
