//! Codex as an ACP agent.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::io::Read;
use std::sync::OnceLock;

use coqui_acp_api::{Bucket, Source, UsageSupport};
use coqui_acp_harness::{
    AgentInfo, ConfigOption, Conversation, EventSink, Harness, HarnessError, Launch, McpTransport,
    Open, OpenRequest, OptionValue, PlainSpawn, Spawn, Takes,
};

use crate::Error;
use crate::launch::{CodexLaunch, ThreadStart, valid_thread_id};
use crate::session::{CodexSession, CodexSettings};
use coqui_acp_harness::mode::{self, Mode};

/// The value of the model and effort options that leaves Codex's own choice.
pub const DEFAULT: &str = "default";

/// Codex served as an ACP agent: each session a `codex app-server` process with one thread.
///
/// A session's `additionalDirectories` are each a root of skills (the Agent Skills directory inside it) and writable
/// in a `workspace-write` sandbox, as Canoa gives Codex a profile's skills.
#[derive(Clone)]
pub struct CodexAgent {
    defaults: CodexLaunch,
    spawn: Arc<dyn Spawn>,
    settings: CodexSettings,
    version: Arc<OnceLock<Option<String>>>,
    seeds: Seeds,
}

/// The ids a host seeded with no history, each with the thread Codex assigned once one was started under it.
pub type Seeds = Arc<std::sync::Mutex<alloc::collections::BTreeMap<String, Option<String>>>>;

impl core::fmt::Debug for CodexAgent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CodexAgent")
            .field("defaults", &self.defaults)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl Default for CodexAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexAgent {
    /// Runs `codex` from `PATH` with its own defaults, as a plain process.
    #[must_use]
    pub fn new() -> Self {
        Self {
            defaults: CodexLaunch::new("."),
            spawn: Arc::new(PlainSpawn),
            settings: CodexSettings::new(),
            version: Arc::new(OnceLock::new()),
            seeds: Seeds::default(),
        }
    }

    /// What every session starts with; a session's own parts are added to it.
    #[must_use]
    pub fn defaults(mut self, defaults: CodexLaunch) -> Self {
        self.defaults = defaults;
        self
    }

    /// How processes are started.
    #[must_use]
    pub fn spawn(mut self, spawn: Arc<dyn Spawn>) -> Self {
        self.spawn = spawn;
        self
    }

    /// The bounds and waits of each session.
    #[must_use]
    pub fn settings(mut self, settings: CodexSettings) -> Self {
        self.settings = settings;
        self
    }

    /// The ids a host seeded with no history. Codex starts a thread under an id of its own, so opening one of these
    /// starts a thread, keeps which, and answers to the host's id from then on; later opens reach that thread.
    #[must_use]
    pub fn seeded_sessions(mut self, seeds: Seeds) -> Self {
        self.seeds = seeds;
        self
    }

    /// Starts the session `request` asks for.
    fn begin(
        &self,
        request: &OpenRequest,
        events: &EventSink,
    ) -> Result<(CodexLaunch, CodexSession), HarnessError> {
        let launch = self.launch_for(request)?;
        let sink = events.clone();
        let session = CodexSession::start(
            &launch,
            self.spawn.as_ref(),
            self.settings.clone(),
            move |event| sink.send(event),
        )
        .map_err(HarnessError::from)?;
        Ok((launch, session))
    }

    /// Codex's version, from `codex --version`, asked once; `None` when it does not answer within
    /// [`CodexSettings::version_wait`].
    #[must_use]
    pub fn cli_version(&self) -> Option<String> {
        self.version
            .get_or_init(|| {
                if self.settings.version_wait.is_zero() {
                    return None;
                }
                let mut launch = Launch::new(self.defaults.executable.clone())
                    .arg("--version")
                    .clear_env(self.defaults.clear_env);
                launch.env.clone_from(&self.defaults.env);
                launch.remove_env.clone_from(&self.defaults.remove_env);
                let spawned = self.spawn.spawn(&launch).ok()?;
                let (sender, received) = std::sync::mpsc::channel();
                let mut stdout = spawned.stdout;
                let limit = self.settings.version_bytes;
                let _ = std::thread::Builder::new()
                    .name("coqui-acp-codex-version".into())
                    .spawn(move || {
                        let mut text = String::new();
                        let _ = stdout.by_ref().take(limit).read_to_string(&mut text);
                        let _ = sender.send(text);
                    });
                let text = received.recv_timeout(self.settings.version_wait).ok();
                let _ = spawned.process.kill();
                // `codex-cli 0.164.0`
                text?.split_whitespace().nth(1).map(str::to_string)
            })
            .clone()
    }

    fn launch_for(&self, request: &OpenRequest) -> Result<CodexLaunch, HarnessError> {
        let mut launch = self.defaults.clone();
        launch.cwd.clone_from(&request.cwd);
        launch
            .working_roots
            .extend(request.working_roots.iter().cloned());
        launch.thread = match &request.open {
            Open::New => ThreadStart::New,
            Open::Resume(id) | Open::Load(id) if !valid_thread_id(id) => {
                return Err(HarnessError::Refused(
                    "that is not a Codex thread id".into(),
                ));
            }
            Open::Resume(id) => ThreadStart::Resume(id.clone()),
            Open::Load(id) => ThreadStart::Load(id.clone()),
            _ => {
                return Err(HarnessError::Refused(
                    "that kind of opening is not supported".into(),
                ));
            }
        };
        if let Some(prompt) = &request.system_prompt {
            launch.developer_instructions = Some(match launch.developer_instructions.take() {
                Some(own) => format!("{own}\n\n{prompt}"),
                None => prompt.clone(),
            });
        }
        for server in &request.mcp_servers {
            if matches!(server.transport, McpTransport::Sse { .. }) {
                return Err(HarnessError::Refused(
                    "Codex has no SSE transport for MCP servers".into(),
                ));
            }
            if launch.has_server(&server.name) {
                return Err(HarnessError::Refused(format!(
                    "an MCP server named {} is the host's own",
                    server.name
                )));
            }
            launch.mcp_servers.push(server.clone());
        }
        launch
            .additional_directories
            .extend(request.additional_directories.iter().cloned());
        Ok(launch)
    }
}

impl Harness for CodexAgent {
    fn info(&self) -> AgentInfo {
        let title = match self.cli_version() {
            Some(version) => format!("Codex {version}"),
            None => "Codex".to_string(),
        };
        AgentInfo::new("coqui-acp-codex", title, env!("CARGO_PKG_VERSION"))
    }

    fn usage(&self) -> UsageSupport {
        UsageSupport::new(alloc::vec![Source::Reported, Source::Inferred], true)
    }

    /// With no session open, an app server is started for the read and ended after it, so a host polls as often as it
    /// can afford a process start. Nothing when Codex cannot be started or has no allowances to report.
    fn read_usage(&self) -> Option<Vec<Bucket>> {
        CodexSession::read_account_usage(&self.defaults, self.spawn.as_ref(), self.settings.clone())
            .ok()
    }

    fn takes(&self) -> Takes {
        Takes::new()
            .images(true)
            .embedded_context(true)
            .mcp_http(true)
            .load(true)
            .additional_directories(true)
            .system_prompt(true)
    }

    fn open(
        &self,
        request: OpenRequest,
        events: EventSink,
    ) -> Result<Box<dyn Conversation>, HarnessError> {
        let mut request = request;
        let mut alias = None;
        let mut mapped = None;
        if let Open::Resume(id) | Open::Load(id) = &request.open
            && let Some(known) = self
                .seeds
                .lock()
                .ok()
                .and_then(|seeds| seeds.get(id).cloned())
        {
            alias = Some(id.clone());
            mapped.clone_from(&known);
            request.open = match (&request.open, known) {
                (Open::Load(_), Some(thread)) => Open::Load(thread),
                (_, Some(thread)) => Open::Resume(thread),
                (_, None) => Open::New,
            };
        }
        let (launch, session) = match self.begin(&request, &events) {
            Ok(started) => started,
            // A thread nothing was said in is not kept by Codex: this one starts over.
            Err(_) if mapped.is_some() => {
                request.open = Open::New;
                self.begin(&request, &events)?
            }
            Err(error) => return Err(error),
        };
        if let Some(id) = &alias
            && let Ok(mut seeds) = self.seeds.lock()
        {
            seeds.insert(id.clone(), Some(session.thread().to_string()));
        }
        // What a client may choose: what the host named; else the host's own mode and every stricter one, the host's mode
        // being what its policy and sandbox amount to, or what the thread is really in when the host set neither.
        let loosest = match (&launch.approval_policy, &launch.sandbox) {
            (Some(approval), Some(sandbox)) => crate::mode::configured(approval, sandbox),
            _ => match session.mode() {
                Some(crate::mode::Shown::Known(mode)) => Some(mode),
                _ => None,
            },
        };
        let offered = mode::widened(
            &mode::up_to(loosest, launch.allow_bypass),
            launch.modes.as_deref().unwrap_or(&[]),
            launch.allow_bypass,
        );
        Ok(Box::new(CodexConversation {
            session,
            offered,
            alias,
        }))
    }
}

impl From<Error> for HarnessError {
    fn from(error: Error) -> Self {
        match error {
            Error::Refused(why) => Self::Refused(why),
            other => Self::Failed(other.to_string()),
        }
    }
}

struct CodexConversation {
    session: CodexSession,
    /// The host's id for a session it seeded, which every id this conversation reports is.
    alias: Option<String>,
    /// The modes that can be chosen.
    offered: Vec<Mode>,
}

impl CodexConversation {
    fn current_model(&self) -> Option<String> {
        self.session.choice().0.or_else(|| {
            self.session
                .models()
                .unwrap_or_default()
                .iter()
                .find(|model| model.is_default)
                .map(|model| model.id.clone())
        })
    }
}

impl Conversation for CodexConversation {
    fn session_id(&self) -> String {
        self.alias
            .clone()
            .unwrap_or_else(|| self.session.thread().to_string())
    }

    fn options(&self) -> Vec<ConfigOption> {
        let (model, effort) = self.session.choice();
        let model = model.unwrap_or_else(|| DEFAULT.to_string());
        let effort = effort.unwrap_or_else(|| DEFAULT.to_string());
        let mut models = alloc::vec![OptionValue::new(
            DEFAULT,
            "Default",
            Some("Codex's own choice".to_string())
        )];
        models.extend(
            self.session
                .models()
                .unwrap_or_default()
                .iter()
                .map(|known| {
                    OptionValue::new(
                        known.id.clone(),
                        known.name.clone(),
                        known.description.clone(),
                    )
                }),
        );
        if !models.iter().any(|value| value.id == model) {
            models.push(OptionValue::new(model.clone(), model.clone(), None));
        }
        let efforts_of = self
            .current_model()
            .and_then(|current| {
                self.session
                    .models()
                    .unwrap_or_default()
                    .iter()
                    .find(|known| known.id == current)
                    .map(|known| known.efforts.clone())
            })
            .unwrap_or_default();
        let mut efforts = alloc::vec![OptionValue::new(
            DEFAULT,
            "Default",
            Some("The model's own effort".to_string())
        )];
        efforts.extend(
            efforts_of
                .into_iter()
                .map(|effort| OptionValue::new(effort.clone(), effort, None)),
        );
        if !efforts.iter().any(|value| value.id == effort) {
            efforts.push(OptionValue::new(effort.clone(), effort.clone(), None));
        }
        let mut options = alloc::vec![
            ConfigOption::new("model", "Model", model, models).category("model"),
            ConfigOption::new("effort", "Effort", effort, efforts).category("thought_level"),
        ];
        match self.session.mode() {
            Some(crate::mode::Shown::Known(current)) => {
                options.push(mode::option(current, &self.offered));
            }
            Some(crate::mode::Shown::Custom(label)) => {
                options.push(mode::custom_option(&label, &self.offered));
            }
            None => {}
        }
        options
    }

    fn prompt(&self, content: Vec<coqui_acp_harness::event::Content>) -> Result<(), HarnessError> {
        self.session.prompt(&content).map_err(HarnessError::from)
    }

    fn interrupt(&self) -> Result<(), HarnessError> {
        self.session.interrupt().map_err(HarnessError::from)
    }

    fn read_usage(&self) -> Option<Vec<Bucket>> {
        self.session.read_usage().ok()
    }

    fn set_option(&self, id: &str, value: &str) -> Result<Vec<ConfigOption>, HarnessError> {
        if id == mode::OPTION_ID {
            let chosen = mode::chosen(value, &self.offered).map_err(HarnessError::Refused)?;
            self.session.set_mode(chosen);
            return Ok(self.options());
        }
        let offered = self.options();
        let option = offered
            .iter()
            .find(|option| option.id == id)
            .ok_or_else(|| HarnessError::Refused(format!("no option {id}")))?;
        if !option.values.iter().any(|known| known.id == value) {
            return Err(HarnessError::Refused(format!(
                "{value} is not a value of {id}"
            )));
        }
        let chosen = (value != DEFAULT).then_some(value);
        if id == "model" {
            self.session.set_model(chosen);
        } else {
            self.session.set_effort(chosen);
        }
        Ok(self.options())
    }

    fn close(&self) {
        self.session.close();
    }
}
