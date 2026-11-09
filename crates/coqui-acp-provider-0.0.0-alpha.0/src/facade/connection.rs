//! One client: its requests, its sessions and what watches them.
//!
//! Handlers run on the connection's executor and never call a provider: every provider call has a thread of
//! its own, so a provider that blocks cannot stop the connection answering.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::path::{Component, Path};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use agent_client_protocol::{Client, ConnectionTo, Error as RpcError, Responder, UntypedMessage};
use futures::channel::oneshot;
use guatiao::Alloc;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::operation::{Launch, Operation};
use super::options::{self, OptionTarget};
use super::{
    AcpFacade, EVENTS_CAPABILITY, FacadeAuth, LAUNCH_META_KEY, SESSION_OPTIONS_CAPABILITY,
    SESSION_OPTIONS_METHOD, Settings, updates,
};
use crate::kind::AgentProvider;

/// The longest session identifier a client may name: it reaches the provider.
const MAX_SESSION_ID: usize = 4096;

/// How often deadlines are checked, at most.
const TICK: Duration = Duration::from_millis(20);

/// What a client may put under [`LAUNCH_META_KEY`].
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LaunchMeta {
    context: Vec<String>,
    environment: BTreeMap<String, String>,
    model: Option<String>,
    effort: Option<String>,
    meta: Option<Value>,
}

pub(super) struct Connection {
    pub(super) provider: Arc<dyn AgentProvider>,
    pub(super) settings: Settings,
    auth: Option<FacadeAuth>,
    name: String,
    version: String,
    authenticated: AtomicBool,
    auth_failures: AtomicU32,
    stop: Mutex<Option<oneshot::Sender<()>>>,
    sessions: Mutex<HashMap<String, Arc<Operation>>>,
    /// The provider identities every connection of the facade holds.
    held: Arc<Mutex<BTreeSet<String>>>,
    /// What this connection has sent, against [`Settings::max_connection_bytes`].
    sent: AtomicUsize,
    wake: Mutex<Option<Sender<()>>>,
}

fn invalid(what: &str) -> RpcError {
    RpcError::invalid_params().data(what.to_string())
}

fn session_id(params: &Value) -> Result<&str, RpcError> {
    params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("the request names no sessionId"))
}

impl Connection {
    pub(super) fn new(facade: &AcpFacade, stop: oneshot::Sender<()>) -> Arc<Self> {
        Arc::new(Self {
            provider: facade.provider.clone(),
            settings: facade.settings.clone(),
            authenticated: AtomicBool::new(facade.auth.is_none()),
            auth: facade.auth.clone(),
            name: facade.name.clone(),
            version: facade.version.clone(),
            auth_failures: AtomicU32::new(0),
            stop: Mutex::new(Some(stop)),
            sessions: Mutex::new(HashMap::new()),
            held: facade.held.clone(),
            sent: AtomicUsize::new(0),
            wake: Mutex::new(None),
        })
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Operation>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The thread that holds every session to its time limits.
    pub(super) fn supervise(self: &Arc<Self>) -> std::io::Result<JoinHandle<()>> {
        let (wake, woken) = mpsc::channel::<()>();
        *self.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(wake);
        let this = self.clone();
        thread::Builder::new()
            .name("coqui-acp-supervisor".to_string())
            .spawn(move || {
                while matches!(woken.recv_timeout(TICK), Err(RecvTimeoutError::Timeout)) {
                    let now = Instant::now();
                    let sessions: Vec<_> = this.sessions().values().cloned().collect();
                    for session in sessions {
                        session.check(now);
                    }
                }
            })
    }

    /// Ends every session and waits for their providers, up to the close limit. How many are still in a
    /// provider's hands.
    pub(super) fn shut_down(&self) -> usize {
        let sessions: Vec<_> = self.sessions().values().cloned().collect();
        for session in sessions {
            session.begin_close();
        }
        // A limit too large for the clock waits for as long as it takes.
        let deadline = Instant::now().checked_add(self.settings.close_timeout.saturating_add(TICK));
        while !self.sessions().is_empty()
            && deadline.is_none_or(|deadline| Instant::now() < deadline)
        {
            thread::sleep(TICK);
        }
        *self.wake.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.sessions().len()
    }

    /// Takes a finished session out, freeing its place: this one, not a later one under its identity.
    pub(super) fn remove(&self, operation: &Operation) {
        let mut sessions = self.sessions();
        if sessions
            .get(&operation.id)
            .is_some_and(|held| core::ptr::eq(&**held, operation))
        {
            sessions.remove(&operation.id);
            let held = operation
                .held
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(id) = held {
                self.held
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
            }
        }
    }

    /// Holds a provider identity for one session of the facade; `false` when one already holds it.
    pub(super) fn hold(&self, id: &str) -> bool {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_string())
    }

    /// Counts what goes out. Past the connection's limit the connection ends, and the answer is `false`.
    pub(super) fn charge(&self, cost: usize) -> bool {
        let sent = self
            .sent
            .fetch_add(cost, Ordering::SeqCst)
            .saturating_add(cost);
        if sent <= self.settings.max_connection_bytes {
            return true;
        }
        self.end();
        false
    }

    /// Ends the connection, as its client closing it would.
    fn end(&self) {
        if let Some(stop) = self
            .stop
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            let _ = stop.send(());
        }
    }

    /// Whether this connection has run out of credentials to try: then it answers nothing more.
    fn locked(&self) -> bool {
        self.auth.is_some()
            && self.auth_failures.load(Ordering::SeqCst) >= self.settings.max_auth_failures
    }

    fn operation(&self, params: &Value) -> Result<Arc<Operation>, RpcError> {
        let id = session_id(params)?;
        self.sessions()
            .get(id)
            .cloned()
            .ok_or_else(|| RpcError::resource_not_found(Some(id.to_string())))
    }

    pub(super) fn request(
        self: &Arc<Self>,
        message: &UntypedMessage,
        responder: Responder<Value>,
        cx: ConnectionTo<Client>,
    ) {
        let params = message.params();
        // Requests already sent keep arriving after the connection is told to end, so each is refused.
        if self.locked() {
            let _ = responder.respond_with_error(
                RpcError::auth_required()
                    .data("this connection tried too many credentials".to_string()),
            );
            return;
        }
        let outcome = match message.method() {
            "initialize" => Ok(self.initialize()),
            "authenticate" => self.authenticate(params),
            _ if !self.authenticated.load(Ordering::SeqCst) => Err(RpcError::auth_required()),
            "session/new" => self.new_session(params, cx),
            "session/set_config_option" => self.set_config_option(params),
            "session/set_mode" => self.set_mode(params),
            "session/list" => self.list(params),
            SESSION_OPTIONS_METHOD => Ok(options::description(&self.settings.options)),
            "session/prompt" => {
                return self.prompt(params, responder);
            }
            "session/resume" => {
                return self.resume(params, responder, cx);
            }
            "session/close" => {
                return match self.operation(params) {
                    Ok(operation) => operation.close(responder),
                    Err(error) => drop(responder.respond_with_error(error)),
                };
            }
            _ => Err(RpcError::method_not_found()),
        };
        let _ = responder.respond_with_result(outcome);
    }

    pub(super) fn notification(&self, message: &UntypedMessage) {
        if message.method() != "session/cancel" || self.locked() {
            return;
        }
        if let Ok(operation) = self.operation(message.params()) {
            operation.cancel();
        }
    }

    fn initialize(&self) -> Value {
        let methods = self
            .auth
            .iter()
            .map(|auth| json!({"id": auth.method, "name": auth.name}))
            .collect::<Vec<_>>();
        json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": false,
                "promptCapabilities": {},
                "mcpCapabilities": {"http": false, "sse": false},
                "sessionCapabilities": {"resume": {}, "close": {}, "list": {}},
                "_meta": {EVENTS_CAPABILITY: true, SESSION_OPTIONS_CAPABILITY: true},
            },
            "authMethods": methods,
            "agentInfo": {"name": self.name, "version": self.version},
        })
    }

    fn authenticate(&self, params: &Value) -> Result<Value, RpcError> {
        let Some(auth) = &self.auth else {
            return Ok(json!({}));
        };
        let method = params.get("methodId").and_then(Value::as_str);
        let meta = params.get("_meta").and_then(Value::as_object);
        if method == Some(auth.method.as_str()) && meta.is_some_and(|meta| (auth.check)(meta)) {
            self.authenticated.store(true, Ordering::SeqCst);
            return Ok(json!({}));
        }
        if self.auth_failures.fetch_add(1, Ordering::SeqCst) + 1 >= self.settings.max_auth_failures
        {
            self.end();
        }
        Err(RpcError::auth_required().data("the credential was not accepted".to_string()))
    }

    /// What `session/new` and `session/resume` ask for, checked before anything is held for it.
    fn launch(&self, params: &Value) -> Result<Launch, RpcError> {
        let cwd = params
            .get("cwd")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("the request names no cwd"))?;
        let path = Path::new(cwd);
        if !path.is_absolute() {
            return Err(invalid("cwd is not an absolute path"));
        }
        // Compared as written: `..` would climb out of a root, and links are the provider's to judge.
        let roots = &self.settings.working_roots;
        if !roots.is_empty()
            && (path
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
                || !roots.iter().any(|root| path.starts_with(root)))
        {
            return Err(invalid(
                "cwd is outside the directories this agent works in",
            ));
        }
        let servers = params.get("mcpServers");
        if servers.is_some_and(|servers| !servers.is_array()) {
            return Err(invalid("mcpServers is not a list"));
        }
        if servers
            .and_then(Value::as_array)
            .is_some_and(|servers| !servers.is_empty())
        {
            return Err(invalid(
                "this agent connects to no MCP server of its client",
            ));
        }
        let meta: LaunchMeta = match params
            .get("_meta")
            .and_then(|meta| meta.get(LAUNCH_META_KEY))
        {
            // Its words would quote what the client sent, which may hold a credential.
            Some(raw) => serde_json::from_value(raw.clone()).map_err(|_| {
                invalid("_meta holds no launch: {context, environment, model, effort, meta}")
            })?,
            None => LaunchMeta::default(),
        };
        let mut environment = self.settings.environment.clone();
        for (name, value) in meta.environment {
            if !self.settings.client_environment.contains(&name) {
                return Err(invalid(&format!(
                    "this agent takes no environment value {name} from its client"
                )));
            }
            environment.insert(name, value);
        }
        let declared = &self.settings.options;
        let mut choices = options::defaults(declared);
        for (target, wanted) in [
            (OptionTarget::Model, &meta.model),
            (OptionTarget::Effort, &meta.effort),
        ] {
            if let (Some(option), Some(wanted)) = (options::for_target(declared, &target), wanted) {
                options::choose(declared, &mut choices, &option.id, wanted)
                    .map_err(|why| invalid(&why))?;
            }
        }
        let opaque = meta
            .meta
            .as_ref()
            .map(|meta| guatiao_serde::adapt::to_value_in(meta, Alloc::rust()))
            .transpose()
            .map_err(|_| invalid("_meta cannot be carried to the provider"))?;
        Ok(Launch {
            working_directory: cwd.to_string(),
            context: meta.context,
            environment,
            model: meta.model,
            effort: meta.effort,
            meta: opaque,
            choices,
        })
    }

    /// A place for a session, or why not.
    fn admit(self: &Arc<Self>, operation: Arc<Operation>) -> Result<(), RpcError> {
        let mut sessions = self.sessions();
        if sessions.len() >= self.settings.max_sessions {
            return Err(RpcError::internal_error()
                .data("this connection holds as many sessions as it may".to_string()));
        }
        if sessions.contains_key(&operation.id) {
            return Err(invalid("that session is already open on this connection"));
        }
        sessions.insert(operation.id.clone(), operation);
        Ok(())
    }

    fn new_session(
        self: &Arc<Self>,
        params: &Value,
        cx: ConnectionTo<Client>,
    ) -> Result<Value, RpcError> {
        let launch = self.launch(params)?;
        let id = uuid::Uuid::new_v4().to_string();
        let mut answer = Map::new();
        answer.insert("sessionId".to_string(), json!(id));
        options::session_fields(&self.settings.options, &launch.choices, &mut answer);
        self.admit(Operation::new(id, launch, self.clone(), cx))?;
        Ok(Value::Object(answer))
    }

    /// `session/list`: this connection's open sessions, in `cwd` when it names one, in one page. Another
    /// connection's are not this client's to see: a provider identity works as a bearer secret.
    fn list(&self, params: &Value) -> Result<Value, RpcError> {
        if params.get("cursor").is_some_and(|cursor| !cursor.is_null()) {
            return Err(invalid("this agent lists every session in one page"));
        }
        let cwd = params.get("cwd").and_then(Value::as_str);
        let mut listed: Vec<(String, String)> = self
            .sessions()
            .values()
            .filter_map(|operation| operation.listed())
            .filter(|(_, directory)| cwd.is_none_or(|cwd| cwd == directory))
            .collect();
        listed.sort();
        let sessions = listed
            .into_iter()
            .map(|(id, directory)| json!({"sessionId": id, "cwd": directory}))
            .collect::<Vec<_>>();
        Ok(json!({"sessions": sessions}))
    }

    /// `session/set_config_option`: a value from the option's own list, before the provider starts.
    fn set_config_option(&self, params: &Value) -> Result<Value, RpcError> {
        let operation = self.operation(params)?;
        let option = params
            .get("configId")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("the request names no configId"))?;
        // Every option here is a select, so only a value id is one of its values.
        let value = params
            .get("value")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(&format!("the option {option} takes a value id")))?;
        let choices = operation.choose(option, value)?;
        Ok(json!({"configOptions": options::config_options(&self.settings.options, &choices)}))
    }

    /// `session/set_mode`: the option of category `mode`, as `session/set_config_option` sets it.
    fn set_mode(&self, params: &Value) -> Result<Value, RpcError> {
        let operation = self.operation(params)?;
        let value = params
            .get("modeId")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("the request names no modeId"))?;
        let Some(option) = options::mode_option(&self.settings.options) else {
            return Err(invalid("this agent has no modes"));
        };
        operation.choose(&option.id, value)?;
        Ok(json!({}))
    }

    fn prompt(&self, params: &Value, responder: Responder<Value>) {
        let operation = match self.operation(params) {
            Ok(operation) => operation,
            Err(error) => {
                let _ = responder.respond_with_error(error);
                return;
            }
        };
        let Some(text) = updates::prompt_text(params) else {
            let _ = responder.respond_with_error(invalid(
                "this agent takes a prompt of text and resource links only",
            ));
            return;
        };
        if text.len() > self.settings.max_prompt_bytes {
            let _ =
                responder.respond_with_error(invalid("the prompt is longer than this agent takes"));
            return;
        }
        operation.prompt(text, responder);
    }

    /// A session the provider issued, under the identity it issued. Its answer waits for the provider, so
    /// a session that no longer exists fails here, in the provider's words.
    fn resume(
        self: &Arc<Self>,
        params: &Value,
        responder: Responder<Value>,
        cx: ConnectionTo<Client>,
    ) {
        let opened = session_id(params).and_then(|id| {
            if id.len() > MAX_SESSION_ID {
                return Err(invalid("that session identifier is too long"));
            }
            let launch = self.launch(params)?;
            // The identity works as a bearer token: one session of the facade at a time may hold it.
            if !self.hold(id) {
                return Err(invalid("that session is already open"));
            }
            let operation = Operation::new(id.to_string(), launch, self.clone(), cx);
            *operation
                .held
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(id.to_string());
            if let Err(error) = self.admit(operation.clone()) {
                *operation
                    .held
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = None;
                self.held
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(id);
                return Err(error);
            }
            Ok(operation)
        });
        match opened {
            Ok(operation) => operation.resume(responder),
            Err(error) => {
                let _ = responder.respond_with_error(error);
            }
        }
    }
}
