//! Codex as a provider of the `acp_harness` kind.
//!
//! Each connection serves a [`CodexAgent`] over the channel, and the agent starts `codex app-server` through the
//! spawner the host handed over, so Codex runs wherever the host's spawner runs it.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;
use std::ffi::OsString;
use std::path::PathBuf;

use serde_json::Value;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Provider, Schema, Secret, Status, ToValue};
use coqui_acp_api::{
    AccountCatalog, AccountGrant, AccountRef, AccountsLink, AcpHarness, Channel, ChannelSink,
    Environment, ExportPage, HarnessInfo, ImportReport, Narrowing, ProcessSpawner, Requirement,
    SessionExport, SessionImport, StartRequest,
};
use coqui_acp_harness::account::{Accounts, Files, Hold};
use coqui_acp_harness::acp::Harness;
use coqui_acp_harness::provider::{Serving, shared, takes_of};
use coqui_acp_harness::{ConnectionSettings, PlainSpawn, Spawn, ThroughSpawner};

use crate::account_provider::ACCOUNT_PROVIDER;
use crate::accounts;
use crate::{CodexAgent, CodexLaunch, SessionSettings};

/// How the Codex provider is configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct ProviderSettings {
    /// The Codex program: a path, or a name looked up on the `PATH` where the spawner runs it.
    #[map(default = String::from("codex"))]
    pub executable: String,
    /// The model every session starts on; Codex's own when absent.
    pub model: Option<String>,
    /// The reasoning effort every session starts at; Codex's own when absent.
    pub effort: Option<String>,
    /// The approval policy every session starts with: `untrusted`, `on-request` or `never`. With the sandbox it is the
    /// loosest mode a client may choose; Codex's own when absent.
    pub approval_policy: Option<String>,
    /// The sandbox every session starts in: `read-only`, `workspace-write` or `danger-full-access`. Codex's own when
    /// absent.
    pub sandbox: Option<String>,
    /// What a connection holds to and offers. Its `access` is turned into Codex's approval policy, sandbox and the
    /// workspace sandbox's network setting (`approval_policy` and `sandbox` above decide where it is absent); `modes`
    /// widen what the approval policy and sandbox amount to and every stricter mode.
    #[map(flatten)]
    pub connection: ConnectionSettings,
    /// Whether a client may give a session MCP servers, which start programs.
    #[map(default = true)]
    pub client_mcp_servers: bool,
    /// The longest, in milliseconds, a command Codex is asked to run to save an account's key or token (`login`) may
    /// take; 0 ends it at once.
    #[map(default = 60_000)]
    pub command_timeout_ms: u64,
    /// The Codex version an imported history's rollout names as its writer; the newest version this crate was tested
    /// with when absent.
    pub import_version: Option<String>,
    /// The model provider an imported history's rollout names; `openai` when absent.
    pub import_model_provider: Option<String>,
    /// Codex's own configuration for every session, as `codex -c key=value` takes it: a dotted key to a value written
    /// as JSON (`true`, `30`, `["a"]`) or, when that is not JSON, as the text itself, passed as `--config key=value`.
    /// Nothing is set by this provider for `shell_environment_policy`, so Codex's default decides which of the
    /// host's variables reach the commands the model runs; `shell_environment_policy.inherit` and its `set`,
    /// `exclude` and `include_only` change that. A key Codex does not read is refused when the session starts.
    #[map(default)]
    pub config: BTreeMap<String, String>,
    /// How each session reads the app server's output, waits for it and ends it.
    #[map(flatten)]
    pub session: SessionSettings,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            executable: "codex".into(),
            model: None,
            effort: None,
            approval_policy: None,
            sandbox: None,
            connection: ConnectionSettings::default(),
            client_mcp_servers: true,
            command_timeout_ms: 60_000,
            import_version: None,
            import_model_provider: None,
            config: BTreeMap::new(),
            session: SessionSettings::default(),
        }
    }
}

/// Codex, as a provider of `acp_harness` under the id `codex`.
///
/// Register it beside a host's own providers with `Registry::register_local`, or build it into a library with
/// `guatiao::providers!`.
#[derive(Provider, Debug)]
#[provider(
    AcpHarness,
    id = "codex",
    config = ProviderSettings,
    new = CodexProvider::unconfigured,
    crate = ::coqui_acp_api::guatiao
)]
pub struct CodexProvider {
    settings: ProviderSettings,
    /// What the provider knows of the files it placed for its accounts.
    accounts: Arc<Accounts>,
    /// For each home a key or token was saved in, a fingerprint of it.
    saved: accounts::Saved,
    /// The ids a host seeded with no history, and the thread Codex gave each once started.
    seeds: crate::Seeds,
}

impl From<ProviderSettings> for CodexProvider {
    fn from(settings: ProviderSettings) -> Self {
        Self::new(settings)
    }
}

impl CodexProvider {
    /// A provider configured as `settings` say.
    #[must_use]
    pub fn new(settings: ProviderSettings) -> Self {
        Self {
            settings,
            accounts: Arc::new(Accounts::new()),
            saved: std::sync::Mutex::new(alloc::collections::BTreeMap::new()),
            seeds: crate::Seeds::default(),
        }
    }

    fn unconfigured() -> Self {
        Self::new(ProviderSettings::default())
    }

    /// What a session starts with: the settings, the host's environment and the requirements; the reason for the
    /// first requirement it cannot hold.
    ///
    /// # Errors
    ///
    /// A requirement the kind does not read.
    fn launch(
        &self,
        environment: &Environment,
        requirements: &[Requirement],
    ) -> Result<(CodexLaunch, Option<String>), ProviderError> {
        let mut launch = CodexLaunch::new(".");
        launch.executable = OsString::from(&self.settings.executable);
        launch.model.clone_from(&self.settings.model);
        launch.effort.clone_from(&self.settings.effort);
        launch
            .approval_policy
            .clone_from(&self.settings.approval_policy);
        launch.sandbox.clone_from(&self.settings.sandbox);
        launch.access = self.settings.connection.access;
        launch.allow_bypass = self.settings.connection.allow_bypass;
        launch.modes.clone_from(&self.settings.connection.modes);
        for (key, text) in &self.settings.config {
            let value = serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()));
            launch.config_overrides.push((key.clone(), value));
        }
        for (name, value) in environment.iter() {
            launch.env.insert(name.into(), value.into());
        }
        let refusal = narrow(requirements)?;
        Ok((launch, refusal))
    }
}

/// The reason for the first requirement Codex cannot be held to; none is held.
///
/// `read_only` is not the read-only sandbox alone: an approval policy that asks lets Codex escalate, Bypass mode is
/// still offered, Plan mode applies `on-request` again, and the MCP servers of the person's own configuration start
/// outside the sandbox. Nothing else has a Codex setting that takes it away.
fn narrow(requirements: &[Requirement]) -> Result<Option<String>, ProviderError> {
    let narrowings = match Narrowing::read_all(requirements) {
        Ok(narrowings) => narrowings,
        Err(error) if error.status() == Status::GUATIAO_ERR_UNSUPPORTED => {
            return Ok(Some(error.to_string()));
        }
        Err(error) => return Err(error.into()),
    };
    Ok(narrowings.into_iter().next().map(|narrowing| {
        let name = Requirement::from(narrowing).name;
        format!("Codex cannot be held to `{name}`")
    }))
}

fn unsupported(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, why)
}

/// What a call that applies an account holds: the account's files and who hears of them.
struct Applying {
    hold: Option<Hold>,
}

impl CodexProvider {
    /// The agent a connection started with `environment` and nothing else would run, over `spawn`: the one whose app
    /// servers [`AcpHarness::prewarm`] starts and [`warm_ready`](Self::warm_ready) counts.
    fn warming(
        &self,
        environment: &Environment,
        spawn: Arc<dyn Spawn>,
    ) -> Result<CodexAgent, ProviderError> {
        let (launch, _) = self.launch(environment, &[])?;
        Ok(CodexAgent::new()
            .settings(self.settings.session.settings())
            .client_access(self.settings.connection.allow_client_access)
            .seeded_sessions(Arc::clone(&self.seeds))
            .defaults(launch)
            .spawn(spawn))
    }

    /// How many started `codex app-server` processes wait in this process for a connection started with `environment`
    /// and nothing else: those [`AcpHarness::prewarm`] and earlier sessions started, not the starting nor the exited.
    #[must_use]
    pub fn warm_ready(&self, environment: &Environment) -> usize {
        self.warming(environment, Arc::new(PlainSpawn))
            .map_or(0, |agent| agent.warm_ready())
    }

    fn command_limit(&self) -> Duration {
        Duration::from_millis(self.settings.command_timeout_ms)
    }

    /// Applies `account` to `launch` as [`accounts::apply_with`] does, and places and holds the files of a login in the
    /// account's home through `files`. The keeper is let go of: what happens to the files afterwards is for the
    /// account provider's watch to tell, and this provider tells nothing.
    ///
    /// # Errors
    ///
    /// `BAD_VALUE` for a grant the kind refuses or a key Codex refuses; `UNSUPPORTED` for a kind that needs files from
    /// a spawner with none.
    fn apply(
        &self,
        account: Option<&AccountGrant>,
        spawner: &Arc<Object<dyn ProcessSpawner>>,
        launch: &mut CodexLaunch,
    ) -> Result<Applying, ProviderError> {
        let Some(grant) = account else {
            return Ok(Applying { hold: None });
        };
        let files = Files::new(Arc::clone(spawner));
        let options = accounts::ApplyOptions::new().timeout(self.command_limit());
        let applied =
            accounts::apply_as(grant, launch, &files, &options, false, Some(&self.saved))?;
        Ok(Applying {
            hold: applied.hold(&self.accounts, &files)?,
        })
    }
}

impl AcpHarness for CodexProvider {
    fn describe(&self) -> Result<HarnessInfo, ProviderError> {
        let agent = CodexAgent::new();
        let mut info = HarnessInfo::new("Codex", self.settings.connection.versions());
        info.takes = takes_of(&agent.takes());
        info.accounts = Some(AccountsLink::new(ACCOUNT_PROVIDER).sharing(["executable"]));
        if !self.settings.client_mcp_servers {
            info.takes.mcp_http = false;
            info.takes.mcp_sse = false;
        }
        // A thread's record is the harness's own, not tied to what a connection was held to.
        info.resumes_wider = true;
        info.usage = agent.usage();
        info.cache_lifetime = Some(
            "ignored: Codex's app server has no setting for how long its prompt cache lives (Codex 0.166.0)".to_string(),
        );
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
        let (mut launch, refusal) =
            self.launch(request.environment.expose(), &request.requirements)?;
        if let Some(why) = refusal {
            return Err(unsupported(&why));
        }
        let spawner = Arc::new(spawner);
        let applying = self.apply(request.account.as_ref(), &spawner, &mut launch)?;
        let account_id = request.account.as_ref().map(|account| account.id.clone());
        let mut settings = self.settings.connection.serving();
        settings.client_mcp_servers = self.settings.client_mcp_servers;
        settings.working_roots = request.working_roots.iter().map(PathBuf::from).collect();
        settings.account.clone_from(&account_id);
        let limit = self.settings.connection.message_limit();
        let mut serving = Serving::new(settings, limit);
        if let Some(hold) = applying.hold {
            serving = serving.ending(move || hold.finish());
        }
        let seeds = Arc::clone(&self.seeds);
        let client_access = self.settings.connection.allow_client_access;
        let session = self.settings.session.settings();
        serving.open_shared(spawner, sink, move |spawn| {
            Ok(shared(
                CodexAgent::new()
                    .settings(session)
                    .client_access(client_access)
                    .seeded_sessions(seeds)
                    .defaults(launch)
                    .spawn(spawn),
            ))
        })
    }

    fn refusal(
        &self,
        requirements: Vec<Requirement>,
        _environment: Secret<Environment>,
        account: Option<AccountRef>,
    ) -> Result<Option<String>, ProviderError> {
        if let Some(why) = narrow(&requirements)? {
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
            options
                .model_provider
                .clone_from(&self.settings.import_model_provider);
            crate::provider_history::import(&session, &spawner, &self.seeds, &options)
        }
        #[cfg(not(feature = "history"))]
        {
            drop(spawner);
            Err(unsupported(
                "this provider does not import a session into Codex yet",
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
            crate::provider_history::export(&export, &spawner, &self.seeds)
        }
        #[cfg(not(feature = "history"))]
        {
            drop(spawner);
            Err(unsupported(
                "this provider does not export a Codex thread yet",
            ))
        }
    }

    fn prewarm(
        &self,
        _cwd: &str,
        environment: Secret<Environment>,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<(), ProviderError> {
        // An app server serves a thread in any directory, so `cwd` is no part of what is started.
        let spawn: Arc<dyn Spawn> = Arc::new(ThroughSpawner::new(spawner));
        self.warming(environment.expose(), spawn)?.prewarm();
        Ok(())
    }
}
