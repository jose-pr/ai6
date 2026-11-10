//! Codex as a provider of the `acp_harness` kind.
//!
//! Each connection serves a [`CodexAgent`] over the channel, and the agent starts `codex app-server` through the
//! spawner the host handed over, so Codex runs wherever the host's spawner runs it.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::ffi::OsString;
use std::path::PathBuf;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Provider, Schema, Secret, Status};
use coqui_acp_api::{
    AccountCatalog, AccountGrant, AccountKeeper, AccountRef, AcpHarness, Channel, ChannelSink,
    Environment, ExportPage, HarnessInfo, ImportReport, LoginOutcome, LoginPrompter, LoginRequest,
    ModelInfo, ModelListing, ModelsRequest, Narrowing, ProcessSpawner, RenewReport, RenewRequest,
    Requirement, SessionExport, SessionImport, StartRequest, UsageQuery, UsageReport,
};
use coqui_acp_harness::acp::Harness;
use coqui_acp_harness::provider::{Reports, Serving, account_refusal, shared, takes_of};
use coqui_acp_harness::{Settings, ThroughSpawner};

use crate::accounts;
use crate::{CodexAgent, CodexLaunch, CodexSession, CodexSettings};

/// How the Codex provider is configured.
#[derive(FromValue, Schema, Debug, Clone)]
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
    /// Whether the Bypass mode may be chosen: never ask, no sandbox.
    #[map(default = false)]
    pub allow_bypass: bool,
    /// The highest ACP version `initialize` answers: 1 or 2.
    #[map(default = 2)]
    pub max_acp_version: u64,
    /// Sessions one connection may hold.
    #[map(default = 8)]
    pub max_sessions: u64,
    /// Requests one connection works on at once; a request past it is refused, and a client that sends faster than the
    /// answers are taken is held back.
    #[map(default = 32)]
    pub max_requests: u64,
    /// The largest message a connection takes, in bytes; one more ends it.
    #[map(default = 67_108_864)]
    pub max_message_bytes: u64,
    /// Whether a client may give a session MCP servers, which start programs.
    #[map(default = true)]
    pub client_mcp_servers: bool,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            executable: "codex".into(),
            model: None,
            effort: None,
            approval_policy: None,
            sandbox: None,
            allow_bypass: false,
            max_acp_version: 2,
            max_sessions: 8,
            max_requests: 32,
            max_message_bytes: 64 * 1024 * 1024,
            client_mcp_servers: true,
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
    /// What the account's readings and the connections learned of its usage.
    reports: Arc<Reports>,
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
            reports: Arc::new(Reports::new()),
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
        launch.allow_bypass = self.settings.allow_bypass;
        for (name, value) in environment.iter() {
            launch.env.insert(name.into(), value.into());
        }
        let refusal = narrow(requirements)?;
        Ok((launch, refusal))
    }

    fn support(&self) -> coqui_acp_api::UsageSupport {
        CodexAgent::new().usage()
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

/// Why a call that applies `account` cannot: `None` when it can.
fn account_check(account: Option<&AccountRef>) -> Result<Option<String>, ProviderError> {
    match account {
        Some(account) => account_refusal(&accounts::catalog(), account),
        None => Ok(None),
    }
}

impl AcpHarness for CodexProvider {
    fn describe(&self) -> Result<HarnessInfo, ProviderError> {
        let agent = CodexAgent::new();
        let versions = if self.settings.max_acp_version >= 2 {
            vec![1, 2]
        } else {
            vec![1]
        };
        let mut info = HarnessInfo::new("Codex", versions);
        info.takes = takes_of(&agent.takes());
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
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<Object<dyn Channel>, ProviderError> {
        drop(keeper);
        let (launch, refusal) = self.launch(request.environment.expose(), &request.requirements)?;
        if let Some(why) = refusal {
            return Err(unsupported(&why));
        }
        let account = request.account.as_ref().map(AccountGrant::reference);
        if let Some(why) = account_check(account.as_ref())? {
            return Err(unsupported(&why));
        }
        let account_id = account.map(|account| account.id);
        let mut settings = Settings::new();
        settings.max_sessions = usize::try_from(self.settings.max_sessions).unwrap_or(usize::MAX);
        settings.max_requests = usize::try_from(self.settings.max_requests).unwrap_or(usize::MAX);
        settings.max_version = if self.settings.max_acp_version >= 2 {
            2
        } else {
            1
        };
        settings.client_mcp_servers = self.settings.client_mcp_servers;
        settings.working_roots = request.working_roots.iter().map(PathBuf::from).collect();
        settings.account.clone_from(&account_id);
        let reports = Arc::clone(&self.reports);
        let support = self.support();
        settings.on_usage = Some(Arc::new(move |report: &UsageReport| {
            reports.observe(&support, account_id.as_deref(), report);
        }));
        let limit = usize::try_from(self.settings.max_message_bytes).unwrap_or(usize::MAX);
        Serving::new(settings, limit).open(spawner, sink, move |spawn| {
            Ok(shared(CodexAgent::new().defaults(launch).spawn(spawn)))
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
        account_check(account.as_ref())
    }

    fn program(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.settings.executable.clone()))
    }

    fn import_session(
        &self,
        session: SessionImport,
        _spawner: Object<dyn ProcessSpawner>,
        _keeper: Object<dyn AccountKeeper>,
    ) -> Result<ImportReport, ProviderError> {
        let _ = session;
        Err(unsupported(
            "this provider does not import a session into Codex yet",
        ))
    }

    fn export_session(
        &self,
        export: SessionExport,
        _spawner: Object<dyn ProcessSpawner>,
        _keeper: Object<dyn AccountKeeper>,
    ) -> Result<ExportPage, ProviderError> {
        let _ = export;
        Err(unsupported(
            "this provider does not export a Codex thread yet",
        ))
    }

    fn models(
        &self,
        request: ModelsRequest,
        spawner: Object<dyn ProcessSpawner>,
        _keeper: Object<dyn AccountKeeper>,
    ) -> Result<ModelListing, ProviderError> {
        if let Some(why) = account_check(
            request
                .account
                .as_ref()
                .map(AccountGrant::reference)
                .as_ref(),
        )? {
            return Err(unsupported(&why));
        }
        let (launch, _) = self.launch(request.environment.expose(), &[])?;
        let spawn = ThroughSpawner::new(spawner);
        Ok(
            match CodexSession::read_models(&launch, &spawn, CodexSettings::new()) {
                Ok(Some(models)) => ModelListing::of(
                    models
                        .into_iter()
                        .map(|model| {
                            let mut info = ModelInfo::new(model.id);
                            info.name = Some(model.name);
                            info.description = model.description;
                            info
                        })
                        .collect(),
                ),
                Ok(None) => ModelListing::unknown("Codex listed no model"),
                Err(error) => ModelListing::unknown(format!("Codex could not be asked: {error}")),
            },
        )
    }

    fn usage(
        &self,
        query: UsageQuery,
        spawner: Object<dyn ProcessSpawner>,
        _keeper: Object<dyn AccountKeeper>,
    ) -> Result<UsageReport, ProviderError> {
        let account = query.account.as_ref().map(AccountGrant::reference);
        if let Some(why) = account_check(account.as_ref())? {
            return Err(unsupported(&why));
        }
        let id = account.as_ref().map(|account| account.id.as_str());
        let (launch, _) = self.launch(query.environment.expose(), &[])?;
        let spawn = ThroughSpawner::new(spawner);
        // Codex answers with no turn: ask it. A login with no allowances answers with an error, and nothing is reported.
        Ok(
            match CodexSession::read_account_usage(&launch, &spawn, CodexSettings::new()) {
                Ok(buckets) => self.reports.read(&self.support(), id, buckets),
                Err(_) => self.reports.report(&self.support(), id),
            },
        )
    }

    fn login(
        &self,
        _request: LoginRequest,
        _spawner: Object<dyn ProcessSpawner>,
        _prompter: Object<dyn LoginPrompter>,
    ) -> Result<LoginOutcome, ProviderError> {
        Err(unsupported(
            "this provider offers no way to sign in to Codex yet",
        ))
    }

    fn renew(
        &self,
        request: RenewRequest,
        _spawner: Object<dyn ProcessSpawner>,
    ) -> Result<RenewReport, ProviderError> {
        account_check(Some(&request.account.reference()))?;
        Err(unsupported("this provider renews no Codex login yet"))
    }
}
