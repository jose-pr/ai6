//! Claude Code as an ACP agent.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};

use coqui_acp_api::{Source, UsageSupport};
use coqui_acp_harness::acp::absolute_path;
use coqui_acp_harness::{
    AgentInfo, ConfigOption, Conversation, Event, EventSink, Harness, HarnessError, Launch, Open,
    OpenRequest, OptionValue, PlainSpawn, Spawn, Takes,
};
use serde_json::Value;

use crate::launch::{ClaudeLaunch, SessionStart, valid_session_id};
use crate::session::{ClaudeSession, ClaudeSettings, Model};
use crate::{Error, replay};
use coqui_acp_harness::mode::{self, Mode};

/// The efforts Claude Code 2.1.318 takes, offered when a model does not list its own.
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The value of the model and effort options that leaves Claude Code's own choice.
pub const DEFAULT: &str = "default";

/// Claude Code served as an ACP agent: each session a `claude` process in stream-json mode.
///
/// A session's `_meta.claudeCode.options.plugins` (`[{"type": "local", "path": "<absolute directory>"}]`) loads those
/// plugins with `--plugin-dir`, as the Claude Agent SDK's `plugins` option does, so their skills are named
/// `<plugin>:<skill>`, once a host has allowed that with [`ClaudeAgent::client_plugins`].
#[derive(Clone)]
pub struct ClaudeAgent {
    defaults: ClaudeLaunch,
    spawn: Arc<dyn Spawn>,
    settings: ClaudeSettings,
    version: Arc<OnceLock<Option<String>>>,
    client_plugins: bool,
    client_access: bool,
    fresh: FreshSessions,
}

/// The ids a host seeded empty: a session of one is Claude Code's to start new under that id, when it has no
/// transcript yet.
pub type FreshSessions = Arc<std::sync::Mutex<alloc::collections::BTreeSet<String>>>;

impl core::fmt::Debug for ClaudeAgent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ClaudeAgent")
            .field("defaults", &self.defaults)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl Default for ClaudeAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeAgent {
    /// Runs `claude` from `PATH` with its own defaults, as a plain process.
    #[must_use]
    pub fn new() -> Self {
        Self {
            defaults: ClaudeLaunch::new("."),
            spawn: Arc::new(PlainSpawn),
            settings: ClaudeSettings::new(),
            version: Arc::new(OnceLock::new()),
            client_plugins: false,
            client_access: false,
            fresh: FreshSessions::default(),
        }
    }

    /// What every session starts with; a session's own parts (its directory, servers, prompt, directories and
    /// plugins) are added to it.
    #[must_use]
    pub fn defaults(mut self, defaults: ClaudeLaunch) -> Self {
        self.defaults = defaults;
        self
    }

    /// How processes are started.
    #[must_use]
    pub fn spawn(mut self, spawn: Arc<dyn Spawn>) -> Self {
        self.spawn = spawn;
        self
    }

    /// Whether a client may load plugins through `_meta.claudeCode.options.plugins`. A plugin's hooks and MCP servers run
    /// with no permission asked, so this is the host's to allow, and each path is held to the same working roots and
    /// network rule as a session's directories. Default false.
    #[must_use]
    pub const fn client_plugins(mut self, allowed: bool) -> Self {
        self.client_plugins = allowed;
        self
    }

    /// Whether a client may give a session its own access through `_meta` ([`coqui_acp_harness::ACCESS_META`]), which
    /// replaces the launch's. Default false: the access is the host's.
    #[must_use]
    pub const fn client_access(mut self, allowed: bool) -> Self {
        self.client_access = allowed;
        self
    }

    /// The bounds and waits of each session.
    #[must_use]
    pub fn settings(mut self, settings: ClaudeSettings) -> Self {
        self.settings = settings;
        self
    }

    /// The ids a host seeded with no history. Opening one of them that has no transcript starts a new Claude Code
    /// session under that id (`--session-id`), where any other id is resumed (`--resume`).
    #[must_use]
    pub fn fresh_sessions(mut self, fresh: FreshSessions) -> Self {
        self.fresh = fresh;
        self
    }

    /// Claude Code's version, from `claude --version`, asked once; `None` when it does not answer within
    /// [`ClaudeSettings::version_wait`].
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
                    .name("coqui-acp-claude-version".into())
                    .spawn(move || {
                        let mut text = String::new();
                        let _ = stdout.by_ref().take(limit).read_to_string(&mut text);
                        let _ = sender.send(text);
                    });
                let text = received.recv_timeout(self.settings.version_wait).ok();
                let _ = spawned.process.kill();
                text?.split_whitespace().next().map(str::to_string)
            })
            .clone()
    }

    fn launch_for(&self, request: &OpenRequest) -> Result<ClaudeLaunch, HarnessError> {
        let mut launch = self.defaults.clone();
        launch.cwd.clone_from(&request.cwd);
        let config = replay::config_dir(&launch.env, launch.clear_env);
        launch.session = match &request.open {
            Open::New => SessionStart::New(uuid::Uuid::new_v4().to_string()),
            Open::Resume(id) | Open::Load(id) => {
                if !valid_session_id(id) {
                    return Err(HarnessError::Refused(
                        "that is not a Claude Code session id".into(),
                    ));
                }
                let seeded = self.fresh.lock().is_ok_and(|fresh| fresh.contains(id))
                    && config
                        .as_ref()
                        .is_none_or(|config| replay::transcript(config, &request.cwd, id).is_err());
                if seeded {
                    SessionStart::New(id.clone())
                } else {
                    SessionStart::Resume(id.clone())
                }
            }
            _ => {
                return Err(HarnessError::Refused(
                    "that kind of opening is not supported".into(),
                ));
            }
        };
        if let Some(prompt) = &request.system_prompt {
            launch.append_system_prompt = Some(match launch.append_system_prompt.take() {
                Some(own) => format!("{own}\n\n{prompt}"),
                None => prompt.clone(),
            });
        }
        if launch.mcp_config_file.is_some() && !request.mcp_servers.is_empty() {
            return Err(HarnessError::Refused(
                "this agent's MCP configuration is the host's own file: it takes no servers from a client".into(),
            ));
        }
        if let Some(taken) = request
            .mcp_servers
            .iter()
            .find(|server| launch.mcp_servers.iter().any(|own| own.name == server.name))
        {
            return Err(HarnessError::Refused(format!(
                "an MCP server named {} is the host's own",
                taken.name
            )));
        }
        launch
            .mcp_servers
            .extend(request.mcp_servers.iter().cloned());
        launch
            .add_dirs
            .extend(request.additional_directories.iter().cloned());
        let plugins = plugins(&request.meta)?;
        if !plugins.is_empty() && !self.client_plugins {
            return Err(HarnessError::Refused(
                "this agent takes no plugins from a client".into(),
            ));
        }
        for plugin in &plugins {
            request
                .path_admitted(plugin)
                .map_err(HarnessError::Refused)?;
        }
        launch.plugin_dirs.extend(plugins);
        let asked = if self.client_access {
            coqui_acp_harness::Access::from_meta(&request.meta).map_err(HarnessError::Refused)?
        } else {
            None
        };
        if asked.is_some() {
            launch.access = asked;
        }
        Ok(launch)
    }
}

/// The local plugins of `_meta.claudeCode.options.plugins`, each an absolute directory.
fn plugins(meta: &serde_json::Map<String, Value>) -> Result<Vec<PathBuf>, HarnessError> {
    let Some(plugins) = meta
        .get("claudeCode")
        .and_then(|code| code.pointer("/options/plugins"))
    else {
        return Ok(Vec::new());
    };
    let plugins = plugins
        .as_array()
        .ok_or_else(|| HarnessError::Refused("claudeCode.options.plugins is not a list".into()))?;
    plugins
        .iter()
        .map(|plugin| {
            if plugin.get("type").and_then(Value::as_str) != Some("local") {
                return Err(HarnessError::Refused("only local plugins are taken".into()));
            }
            let path = plugin
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| HarnessError::Refused("a plugin has no path".into()))?;
            absolute_path(path, "a plugin's path").map_err(HarnessError::Refused)
        })
        .collect()
}

impl Harness for ClaudeAgent {
    fn info(&self) -> AgentInfo {
        let title = match self.cli_version() {
            Some(version) => format!("Claude Code {version}"),
            None => "Claude Code".to_string(),
        };
        AgentInfo::new("coqui-acp-claude", title, env!("CARGO_PKG_VERSION"))
    }

    fn usage(&self) -> UsageSupport {
        UsageSupport::new(alloc::vec![Source::Reported, Source::Inferred], true).needs_turn(true)
    }

    fn takes(&self) -> Takes {
        Takes::new()
            .images(true)
            .embedded_context(true)
            .mcp_http(true)
            .mcp_sse(true)
            .load(true)
            .additional_directories(true)
            .system_prompt(true)
    }

    fn open(
        &self,
        request: OpenRequest,
        events: EventSink,
    ) -> Result<Box<dyn Conversation>, HarnessError> {
        let launch = self.launch_for(&request)?;
        if let (Open::Load(id), SessionStart::Resume(_)) = (&request.open, &launch.session) {
            let config = replay::config_dir(&launch.env, launch.clear_env).ok_or_else(|| {
                HarnessError::Failed("Claude Code's configuration directory is not known".into())
            })?;
            let transcript =
                replay::transcript(&config, &launch.cwd, id).map_err(HarnessError::from)?;
            let replaying = events.clone();
            replay::replay(
                &transcript,
                self.settings.max_transcript_bytes,
                self.settings.max_tools,
                self.settings.parser,
                &move |event| replaying.send(event),
            )
            .map_err(HarnessError::from)?;
        }
        let sink = events.clone();
        let session = ClaudeSession::start(
            &launch,
            self.spawn.as_ref(),
            self.settings.clone(),
            move |event| sink.send(event),
        )
        .map_err(HarnessError::from)?;
        let model = launch.model.clone().unwrap_or_else(|| DEFAULT.to_string());
        let effort = launch.effort.clone().unwrap_or_else(|| DEFAULT.to_string());
        let state = Arc::new(State {
            models: session.models().unwrap_or_default(),
            chosen: Mutex::new((model, effort)),
            mode: Mutex::new(Current::None),
            offered: offered_for(&launch),
        });
        // Claude Code's mode is its own: told by its `system` lines and by an "allow always" that switches it, so what
        // the client shows follows it.
        let configured = launch
            .effective_permission_mode()
            .unwrap_or_else(|| "default".to_string());
        state.observe(&session.permission_mode().unwrap_or(configured));
        let allowed = Arc::clone(&state);
        session.permit_modes(move |raw| {
            mode_of(raw).is_some_and(|mode| allowed.offered.contains(&mode))
        });
        let told = Arc::clone(&state);
        let sink = events.clone();
        session.on_permission_mode(move |raw| {
            if told.observe(raw) {
                sink.send(Event::Options(told.options()));
            }
        });
        if let Some(raw) = session.permission_mode()
            && state.observe(&raw)
        {
            events.send(Event::Options(state.options()));
        }
        Ok(Box::new(ClaudeConversation { session, state }))
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

struct ClaudeConversation {
    session: ClaudeSession,
    state: Arc<State>,
}

/// What a conversation shows, shared with the thread that is told Claude Code's mode changes.
struct State {
    models: Vec<Model>,
    /// The model and effort in use.
    chosen: Mutex<(String, String)>,
    /// The mode Claude Code is in.
    mode: Mutex<Current>,
    /// The modes that can be switched to.
    offered: Vec<Mode>,
}

/// The mode Claude Code is in as far as it has said.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Current {
    /// It has not said.
    None,
    /// One of the modes.
    Known(Mode),
    /// A `permissionMode` no mode names, such as `dontAsk`: shown as it is called.
    Custom(String),
}

/// Claude Code's `permissionMode` for each mode, the one table both directions read.
const MODES: [(Mode, &str); 5] = [
    (Mode::Manual, "default"),
    (Mode::AcceptEdits, "acceptEdits"),
    (Mode::Auto, "auto"),
    (Mode::Plan, "plan"),
    (Mode::Bypass, "bypassPermissions"),
];

fn mode_of(permission_mode: &str) -> Option<Mode> {
    MODES
        .iter()
        .find(|(_, name)| {
            *name == permission_mode || (permission_mode == "manual" && *name == "default")
        })
        .map(|(mode, _)| *mode)
}

fn permission_mode_of(mode: Mode) -> &'static str {
    MODES
        .iter()
        .find(|(known, _)| *known == mode)
        .map_or("default", |(_, name)| name)
}

/// The modes a client may choose: the launch's own mode and every stricter one, and what the launch names besides. Bypass
/// only when the launch allows it, whatever mode it was started in.
fn offered_for(launch: &ClaudeLaunch) -> Vec<Mode> {
    let configured = launch
        .effective_permission_mode()
        .as_deref()
        .map_or(Some(Mode::Manual), mode_of);
    mode::widened(
        &mode::up_to(configured, launch.allow_bypass),
        launch.modes.as_deref().unwrap_or(&[]),
        launch.allow_bypass,
    )
}

impl State {
    fn efforts(&self, model: &str) -> Vec<String> {
        let listed = self
            .models
            .iter()
            .find(|known| known.id == model)
            .map(|known| known.efforts.clone())
            .unwrap_or_default();
        if listed.is_empty() {
            EFFORTS.iter().map(|effort| (*effort).to_string()).collect()
        } else {
            listed
        }
    }

    /// Takes Claude Code's `permissionMode`; whether that changed what is shown.
    fn observe(&self, raw: &str) -> bool {
        let now = mode_of(raw).map_or_else(|| Current::Custom(raw.to_string()), Current::Known);
        let mut held = self.mode.lock().unwrap_or_else(PoisonError::into_inner);
        let changed = *held != now;
        *held = now;
        changed
    }

    fn options(&self) -> Vec<ConfigOption> {
        let (model, effort) = self
            .chosen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut models: Vec<OptionValue> = self
            .models
            .iter()
            .map(|known| {
                OptionValue::new(known.id.clone(), known.label(), known.description.clone())
            })
            .collect();
        if !models.iter().any(|value| value.id == model) {
            models.push(OptionValue::new(model.clone(), model.clone(), None));
        }
        let mut efforts = vec_default();
        efforts.extend(
            self.efforts(&model)
                .into_iter()
                .map(|effort| OptionValue::new(effort.clone(), effort, None)),
        );
        if !efforts.iter().any(|value| value.id == effort) {
            efforts.push(OptionValue::new(effort.clone(), effort.clone(), None));
        }
        let mut options = vec![
            ConfigOption::new("model", "Model", model, models).category("model"),
            ConfigOption::new("effort", "Effort", effort, efforts).category("thought_level"),
        ];
        match self
            .mode
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            Current::Known(current) => options.push(mode::option(current, &self.offered)),
            Current::Custom(label) => options.push(mode::custom_option(&label, &self.offered)),
            Current::None => {}
        }
        options
    }
}

impl Conversation for ClaudeConversation {
    fn session_id(&self) -> String {
        self.session.id().unwrap_or_default()
    }

    fn options(&self) -> Vec<ConfigOption> {
        self.state.options()
    }

    fn prompt(&self, content: Vec<coqui_acp_harness::event::Content>) -> Result<(), HarnessError> {
        self.session.prompt(&content).map_err(HarnessError::from)
    }

    fn interrupt(&self) -> Result<(), HarnessError> {
        self.session.interrupt().map_err(HarnessError::from)
    }

    fn set_option(&self, id: &str, value: &str) -> Result<Vec<ConfigOption>, HarnessError> {
        if id == mode::OPTION_ID {
            let chosen = mode::chosen(value, &self.state.offered).map_err(HarnessError::Refused)?;
            self.session
                .set_permission_mode(permission_mode_of(chosen))
                .map_err(HarnessError::from)?;
            self.state.observe(permission_mode_of(chosen));
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
        match id {
            "model" => {
                self.session.set_model(value).map_err(HarnessError::from)?;
                self.state
                    .chosen
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .0 = value.to_string();
            }
            _ => {
                self.session
                    .set_effort((value != DEFAULT).then_some(value))
                    .map_err(HarnessError::from)?;
                self.state
                    .chosen
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .1 = value.to_string();
            }
        }
        Ok(self.options())
    }

    fn close(&self) {
        self.session.close();
    }
}

fn vec_default() -> Vec<OptionValue> {
    alloc::vec![OptionValue::new(
        DEFAULT,
        "Default",
        Some("Claude Code's own choice".to_string())
    )]
}
