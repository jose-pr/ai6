//! Claude Code as a provider of the `acp_harness` kind.
//!
//! Each connection serves a [`ClaudeAgent`] over the channel, and the agent starts `claude` through the spawner the
//! host handed over, so Claude Code runs wherever the host's spawner runs it.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::ffi::OsString;
use std::path::PathBuf;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Provider, Schema, Secret, Status, ToValue};
use coqui_acp_api::{
    AccountCatalog, AccountGrant, AccountRef, AccountsLink, AcpHarness, CacheLifetime, Channel,
    ChannelSink, Environment, ExportPage, HarnessInfo, ImportReport, Narrowing, ProcessSpawner,
    Requirement, SessionExport, SessionImport, StartRequest,
};
use coqui_acp_harness::ConnectionSettings;
use coqui_acp_harness::account::{Accounts, Files, Hold};
use coqui_acp_harness::acp::Harness;
use coqui_acp_harness::provider::{Serving, shared, takes_of};

use crate::account_provider::ACCOUNT_PROVIDER;
use crate::accounts;
use crate::{ClaudeAgent, ClaudeLaunch, SessionSettings};

/// What a connection with `cache_lifetime` short sets `CLAUDE_CODE_PROMPT_CACHE_TTL` and its sub-agent twin to.
const SHORT_TTL: &str = "5m";
/// The same for `long`.
const LONG_TTL: &str = "1h";
/// The variable that turns Claude Code's prompt cache off for a connection with `cache_lifetime` off.
const CACHE_OFF_VARIABLE: &str = "DISABLE_PROMPT_CACHING";
/// Claude Code's variables for how long its prompt cache lives, for the conversation and for its sub-agents.
const TTL_VARIABLES: [&str; 2] = [
    "CLAUDE_CODE_PROMPT_CACHE_TTL",
    "CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL",
];

/// A place Claude Code reads settings from: the person's own, the project's, or the project's untracked ones. Written
/// as `user`, `project` and `local`.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[map(crate = ::coqui_acp_api::guatiao, rename_all = "snake_case")]
#[non_exhaustive]
pub enum SettingSource {
    /// The person's own settings.
    User,
    /// The project's shared settings.
    Project,
    /// The project's settings that stay out of version control.
    Local,
}

impl SettingSource {
    /// The name `--setting-sources` takes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
}

/// How the Claude Code provider is configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct ProviderSettings {
    /// The Claude Code program: a path, or a name looked up on the `PATH` where the spawner runs it.
    #[map(default = String::from("claude"))]
    pub executable: String,
    /// The model every session starts on; Claude Code's own when absent.
    pub model: Option<String>,
    /// The effort every session starts at (`low`, `medium`, `high`, `xhigh`, `max`); Claude Code's own when absent.
    pub effort: Option<String>,
    /// The permission mode every session starts in, and the loosest a client may choose: `default`, `acceptEdits`,
    /// `auto`, `plan` or `bypassPermissions`. Claude Code's own default, which is `default`, when absent.
    pub permission_mode: Option<String>,
    /// What a connection holds to and offers. Its `access` is turned into a permission mode (when `permission_mode` is
    /// absent) and Claude Code's sandbox settings; `allow_bypass` starts Claude Code with
    /// `--allow-dangerously-skip-permissions`; `modes` widen the permission mode's own and every stricter one.
    #[map(flatten)]
    pub connection: ConnectionSettings,
    /// The setting sources Claude Code loads, any of `user`, `project` and `local`; an empty list loads none. Claude Code's
    /// own default, every source, when absent.
    pub setting_sources: Option<Vec<SettingSource>>,
    /// Whether a client may load plugins through `_meta.claudeCode.options.plugins`. Their hooks and MCP servers run
    /// with no permission asked.
    #[map(default = false)]
    pub allow_client_plugins: bool,
    /// Whether a client may give a session MCP servers, which start programs.
    #[map(default = true)]
    pub client_mcp_servers: bool,
    /// The Claude Code version an imported history's transcript names as its writer; the newest version this crate was
    /// tested with when absent.
    pub import_version: Option<String>,
    /// How each session reads Claude Code's output, waits for it and ends it.
    #[map(flatten)]
    pub session: SessionSettings,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            executable: "claude".into(),
            model: None,
            effort: None,
            permission_mode: None,
            connection: ConnectionSettings::default(),
            setting_sources: None,
            allow_client_plugins: false,
            client_mcp_servers: true,
            import_version: None,
            session: SessionSettings::default(),
        }
    }
}

/// Claude Code, as a provider of `acp_harness` under the id `claude`.
///
/// Register it beside a host's own providers with `Registry::register_local`, or build it into a library with
/// `guatiao::providers!`.
#[derive(Provider, Debug)]
#[provider(
    AcpHarness,
    id = "claude",
    config = ProviderSettings,
    new = ClaudeProvider::unconfigured,
    crate = ::coqui_acp_api::guatiao
)]
pub struct ClaudeProvider {
    settings: ProviderSettings,
    /// What the provider knows of the files it placed for its accounts.
    accounts: Arc<Accounts>,
    /// The ids a host seeded with no history, which their first start makes sessions.
    fresh: crate::FreshSessions,
}

impl From<ProviderSettings> for ClaudeProvider {
    fn from(settings: ProviderSettings) -> Self {
        Self::new(settings)
    }
}

impl ClaudeProvider {
    /// A provider configured as `settings` say.
    #[must_use]
    pub fn new(settings: ProviderSettings) -> Self {
        Self {
            settings,
            accounts: Arc::new(Accounts::new()),
            fresh: crate::FreshSessions::default(),
        }
    }

    fn unconfigured() -> Self {
        Self::new(ProviderSettings::default())
    }

    /// What a session starts with: the settings, the host's environment, the cache hint and the requirements.
    ///
    /// # Errors
    ///
    /// A requirement the kind does not read.
    fn launch(
        &self,
        environment: &Environment,
        cache_lifetime: CacheLifetime,
        requirements: &[Requirement],
    ) -> Result<(ClaudeLaunch, Option<String>), ProviderError> {
        let mut launch = ClaudeLaunch::new(".");
        launch.executable = OsString::from(&self.settings.executable);
        launch.model.clone_from(&self.settings.model);
        launch.effort.clone_from(&self.settings.effort);
        launch
            .permission_mode
            .clone_from(&self.settings.permission_mode);
        launch.access = self.settings.connection.access;
        launch.allow_bypass = self.settings.connection.allow_bypass;
        launch.modes.clone_from(&self.settings.connection.modes);
        launch.setting_sources = self.settings.setting_sources.as_ref().map(|sources| {
            sources
                .iter()
                .map(|source| source.as_str().to_string())
                .collect()
        });
        for (name, value) in environment.iter() {
            launch.env.insert(name.into(), value.into());
        }
        let ttl = match cache_lifetime {
            CacheLifetime::Short => Some(SHORT_TTL),
            CacheLifetime::Long => Some(LONG_TTL),
            _ => None,
        };
        if cache_lifetime == CacheLifetime::Off {
            launch.env.insert(CACHE_OFF_VARIABLE.into(), "1".into());
        }
        if let Some(ttl) = ttl {
            for name in TTL_VARIABLES {
                launch.env.insert(name.into(), ttl.into());
            }
        }
        let refusal = narrow(requirements, &mut launch)?;
        Ok((launch, refusal))
    }
}

/// Holds the requirements a launch can, changing it to; the reason for the first it cannot.
///
/// `no_project_mcp` is `--strict-mcp-config`, which starts the MCP servers `--mcp-config` names and no others, so also
/// none the user's own configuration declares. No other name is held: Claude Code's flags do not leave the instruction
/// files loaded while they drop a directory's settings, and what the rest take away is not measured.
fn narrow(
    requirements: &[Requirement],
    launch: &mut ClaudeLaunch,
) -> Result<Option<String>, ProviderError> {
    let narrowings = match Narrowing::read_all(requirements) {
        Ok(narrowings) => narrowings,
        Err(error) if error.status() == Status::GUATIAO_ERR_UNSUPPORTED => {
            return Ok(Some(error.to_string()));
        }
        Err(error) => return Err(error.into()),
    };
    for narrowing in narrowings {
        match narrowing {
            Narrowing::NoProjectMcp => launch.strict_mcp_config = true,
            other => {
                let name = Requirement::from(other).name;
                return Ok(Some(format!("Claude Code cannot be held to `{name}`")));
            }
        }
    }
    Ok(None)
}

fn unsupported(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, why)
}

/// What a call that applies an account holds: the account's files and who hears of them.
struct Applying {
    hold: Option<Hold>,
}

impl ClaudeProvider {
    /// Applies `account` to `launch` as [`accounts::apply`] does, and places and holds the files of a login in the
    /// account's directory through `files`. What happens to the files afterwards is for the account provider's watch
    /// to tell, and this provider tells nothing.
    ///
    /// # Errors
    ///
    /// `BAD_VALUE` for a grant the kind refuses; `UNSUPPORTED` for a kind that needs files from a spawner with none.
    fn apply(
        &self,
        account: Option<&AccountGrant>,
        files: &Files,
        launch: &mut ClaudeLaunch,
    ) -> Result<Applying, ProviderError> {
        let Some(grant) = account else {
            return Ok(Applying { hold: None });
        };
        let applied = accounts::apply_as(grant, launch, files, false)?;
        Ok(Applying {
            hold: applied.hold(&self.accounts, files)?,
        })
    }
}

impl AcpHarness for ClaudeProvider {
    fn describe(&self) -> Result<HarnessInfo, ProviderError> {
        let agent = ClaudeAgent::new();
        let mut info = HarnessInfo::new("Claude Code", self.settings.connection.versions());
        info.takes = takes_of(&agent.takes());
        if !self.settings.client_mcp_servers {
            info.takes.mcp_http = false;
            info.takes.mcp_sse = false;
        }
        // A session's record is the harness's own, not tied to what a connection was held to.
        info.resumes_wider = true;
        info.usage = agent.usage();
        info.cache_lifetime = Some(format!(
            "sets {} and {} to {SHORT_TTL} for short and {LONG_TTL} for long, sets {CACHE_OFF_VARIABLE} for off, and leaves them as the environment has them for default",
            TTL_VARIABLES[0], TTL_VARIABLES[1]
        ));
        info.cache_choices = vec![
            CacheLifetime::Off,
            CacheLifetime::Short,
            CacheLifetime::Long,
        ];
        info.accounts = Some(AccountsLink::new(ACCOUNT_PROVIDER).sharing(["executable"]));
        Ok(info)
    }

    fn accounts(&self) -> Result<AccountCatalog, ProviderError> {
        Ok(accounts::catalog())
    }

    fn start(
        &self,
        request: StartRequest,
        spawner: Object<dyn ProcessSpawner>,
        sink: Object<dyn ChannelSink>,
    ) -> Result<Object<dyn Channel>, ProviderError> {
        let (mut launch, refusal) = self.launch(
            request.environment.expose(),
            request.cache_lifetime,
            &request.requirements,
        )?;
        if let Some(why) = refusal {
            return Err(unsupported(&why));
        }
        let spawner = Arc::new(spawner);
        let files = Files::new(Arc::clone(&spawner));
        let applying = self.apply(request.account.as_ref(), &files, &mut launch)?;
        drop(files);
        let account_id = request.account.as_ref().map(|account| account.id.clone());
        let mut settings = self.settings.connection.serving();
        settings.client_mcp_servers = self.settings.client_mcp_servers;
        settings.working_roots = request.working_roots.iter().map(PathBuf::from).collect();
        settings.account.clone_from(&account_id);
        let limit = self.settings.connection.message_limit();
        let plugins = self.settings.allow_client_plugins;
        let client_access = self.settings.connection.allow_client_access;
        let session = self.settings.session.settings();
        let mut serving = Serving::new(settings, limit);
        if let Some(hold) = applying.hold {
            serving = serving.ending(move || hold.finish());
        }
        let fresh = Arc::clone(&self.fresh);
        serving.open_shared(spawner, sink, move |spawn| {
            Ok(shared(
                ClaudeAgent::new()
                    .fresh_sessions(fresh)
                    .defaults(launch)
                    .spawn(spawn)
                    .settings(session)
                    .client_plugins(plugins)
                    .client_access(client_access),
            ))
        })
    }

    fn refusal(
        &self,
        requirements: Vec<Requirement>,
        _environment: Secret<Environment>,
        account: Option<AccountRef>,
    ) -> Result<Option<String>, ProviderError> {
        let mut launch = ClaudeLaunch::new(".");
        if let Some(why) = narrow(&requirements, &mut launch)? {
            return Ok(Some(why));
        }
        if let Some(account) = &account {
            accounts::check(account)?;
        }
        Ok(None)
    }

    fn program(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.settings.executable.clone()))
    }

    fn import_session(
        &self,
        session: SessionImport,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ImportReport, ProviderError> {
        if let Some(account) = &session.account {
            accounts::plan(account)?;
        }
        #[cfg(feature = "history")]
        {
            let mut options = crate::history::ImportOptions::default();
            options.version.clone_from(&self.settings.import_version);
            crate::provider_history::import(&session, &spawner, &self.fresh, &options)
        }
        #[cfg(not(feature = "history"))]
        {
            drop(spawner);
            Err(unsupported(
                "this provider does not import a session into Claude Code yet",
            ))
        }
    }

    fn export_session(
        &self,
        export: SessionExport,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ExportPage, ProviderError> {
        if let Some(account) = &export.account {
            accounts::plan(account)?;
        }
        #[cfg(feature = "history")]
        {
            crate::provider_history::export(&export, &spawner, &self.fresh)
        }
        #[cfg(not(feature = "history"))]
        {
            drop(spawner);
            Err(unsupported(
                "this provider does not export a Claude Code session yet",
            ))
        }
    }
}
