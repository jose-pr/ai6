//! Codex as a provider of the `acp_harness` kind.
//!
//! Each connection serves a [`CodexAgent`] over the channel, and the agent starts `codex app-server` through the
//! spawner the host handed over, so Codex runs wherever the host's spawner runs it.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;
use std::ffi::OsString;
use std::path::PathBuf;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Provider, Schema, Secret, Status, ToValue};
use coqui_acp_api::{
    AccountCatalog, AccountGrant, AccountRef, AcpHarness, Channel, ChannelSink, Environment,
    ExportPage, HarnessInfo, ImportReport, ModelInfo, ModelListing, ModelsRequest, Narrowing,
    ProcessSpawner, Requirement, SessionExport, SessionImport, StartRequest, UnknownKind,
    UsageQuery, UsageReport,
};
use coqui_acp_harness::account::{Accounts, Files, Hold};
use coqui_acp_harness::acp::Harness;
use coqui_acp_harness::provider::{Reports, Serving, shared, takes_of};
use coqui_acp_harness::{Access, Mode};
use coqui_acp_harness::{Settings, ThroughSpawner};

use crate::accounts;
use crate::{CodexAgent, CodexLaunch, CodexSession, CodexSettings, ModelRead};

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
    /// The access every session starts with, in the words every harness shares: the sandbox, the network and the approvals,
    /// turned into Codex's approval policy, sandbox and the workspace sandbox's network setting. Absent, only `approval_policy`
    /// and `sandbox` above decide, and Codex's own configuration where they are absent too.
    pub access: Option<Access>,
    /// Whether a client may give a session its own access through `_meta` (`dev.boriken.coqui.acp/access`).
    #[map(default = false)]
    pub allow_client_access: bool,
    /// Whether the Bypass mode may be chosen: never ask, no sandbox.
    #[map(default = false)]
    pub allow_bypass: bool,
    /// The modes a client may choose, written by id (`manual`, `accept_edits`, `auto`, `plan`, `bypass`): these widen what
    /// the approval policy and sandbox amount to and every stricter mode. Bypass is offered only with `allow_bypass`,
    /// whatever this names. Absent, a client chooses that mode and every stricter one.
    pub modes: Option<Vec<Mode>>,
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
    /// The longest, in milliseconds, a command Codex is asked to run to save an account's key or token (`login`) may
    /// take; 0 ends it at once.
    #[map(default = 60_000)]
    pub command_timeout_ms: u64,
}

impl Default for ProviderSettings {
    fn default() -> Self {
        Self {
            executable: "codex".into(),
            model: None,
            effort: None,
            approval_policy: None,
            sandbox: None,
            access: None,
            allow_client_access: false,
            allow_bypass: false,
            modes: None,
            max_acp_version: 2,
            max_sessions: 8,
            max_requests: 32,
            max_message_bytes: 64 * 1024 * 1024,
            client_mcp_servers: true,
            command_timeout_ms: 60_000,
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
            reports: Arc::new(Reports::new()),
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
        launch.access = self.settings.access;
        launch.allow_bypass = self.settings.allow_bypass;
        launch.modes.clone_from(&self.settings.modes);
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

/// What a call that applies an account holds: the account's files and who hears of them.
struct Applying {
    hold: Option<Hold>,
}

impl CodexProvider {
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
    ) -> Result<Object<dyn Channel>, ProviderError> {
        let (mut launch, refusal) =
            self.launch(request.environment.expose(), &request.requirements)?;
        if let Some(why) = refusal {
            return Err(unsupported(&why));
        }
        let spawner = Arc::new(spawner);
        let applying = self.apply(request.account.as_ref(), &spawner, &mut launch)?;
        let account_id = request.account.as_ref().map(|account| account.id.clone());
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
        let mut serving = Serving::new(settings, limit);
        if let Some(hold) = applying.hold {
            serving = serving.ending(move || hold.finish());
        }
        let seeds = Arc::clone(&self.seeds);
        let client_access = self.settings.allow_client_access;
        serving.open_shared(spawner, sink, move |spawn| {
            Ok(shared(
                CodexAgent::new()
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
            crate::provider_history::import(&session, &spawner, &self.seeds)
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

    fn models(
        &self,
        request: ModelsRequest,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ModelListing, ProviderError> {
        let (mut launch, _) = self.launch(request.environment.expose(), &[])?;
        let spawner = Arc::new(spawner);
        let applying = self.apply(request.account.as_ref(), &spawner, &mut launch)?;
        let spawn = ThroughSpawner::shared(spawner);
        let listing = match CodexSession::read_models(&launch, &spawn, CodexSettings::new()) {
            Ok(ModelRead {
                models: Some(models),
                signed_out,
            }) => {
                let listing = ModelListing::of(
                    models
                        .into_iter()
                        .map(|model| {
                            let mut info = ModelInfo::new(model.id);
                            info.name = Some(model.name);
                            info.description = model.description;
                            info.efforts = model.efforts;
                            info.default_effort = model.default_effort;
                            info.is_default = model.is_default;
                            info
                        })
                        .collect(),
                );
                if signed_out {
                    listing.signed_out()
                } else {
                    listing
                }
            }
            Ok(_) => ModelListing::unknown("Codex listed no model"),
            Err(error) => ModelListing::unavailable(
                match &error {
                    crate::Error::Spawn(why) if why.kind() == std::io::ErrorKind::NotFound => {
                        UnknownKind::NotInstalled
                    }
                    _ => UnknownKind::Failed,
                },
                format!("Codex could not be asked: {error}"),
            ),
        };
        if let Some(hold) = applying.hold {
            hold.finish();
        }
        Ok(listing)
    }

    fn usage(
        &self,
        query: UsageQuery,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<UsageReport, ProviderError> {
        let (mut launch, _) = self.launch(query.environment.expose(), &[])?;
        let spawner = Arc::new(spawner);
        let applying = self.apply(query.account.as_ref(), &spawner, &mut launch)?;
        let id = query.account.as_ref().map(|account| account.id.as_str());
        let spawn = ThroughSpawner::shared(spawner);
        // Codex answers with no turn: ask it. A login with no allowances answers with an error, and nothing is reported.
        let report = match CodexSession::read_account_usage(&launch, &spawn, CodexSettings::new()) {
            Ok(buckets) => self.reports.read(&self.support(), id, buckets),
            Err(_) => self.reports.report(&self.support(), id),
        };
        if let Some(hold) = applying.hold {
            hold.finish();
        }
        Ok(report)
    }
}
