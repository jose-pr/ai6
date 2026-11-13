//! A provider of the `acp_harness` kind for coding agents that speak ACP on their own standard input and output: Grok
//! (`grok agent stdio`) and Antigravity (`agy acp`).
//!
//! The host's messages go to the program a line each and its lines come back as messages; what the
//! provider adds is the account: it removes every credential variable of the harness from what it starts, sets the chosen
//! kind's, places the login file in the account's directory, watches it and tells the host's keeper when the harness
//! renews or loses the login, renews it with no model turn where the harness can, and runs the harness's own sign-in.
//! The kinds of each harness are tables in [`accounts`].
//!
//! A session's id must be a UUID, as the kind says: the provider does not translate the program's ids.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Provider, Schema, Secret, Status, ToValue};
use coqui_acp_api::{
    AccountCatalog, AccountGrant, AccountKeeper, AccountRef, AcpHarness, Channel, ChannelSink,
    Environment, ExportPage, HarnessInfo, HistoryLoss, ImportReport, LoginOutcome, LoginPrompter,
    LoginRequest, ModelListing, ModelsRequest, Narrowing, ProcessSpawner, RenewReport,
    RenewRequest, Requirement, SeedPoint, SessionExport, SessionImport, SpawnSpec, StartRequest,
    UsageQuery, UsageReport, WorkingRoots,
};
use coqui_acp_history::replay::{Replay, ReplaySettings};
use coqui_acp_history::{Document, HistoryError, RedactionSettings, Redactor};

use std::ffi::OsString;

use crate::ThroughSpawner;
use crate::account::{Accounts, DEFAULT_POLL_MS, Files, Hold, Watched};
use crate::cli;
use crate::process::Launch;

pub mod accounts;
mod login;
mod relay;
mod replay;

use accounts::{Applied, HarnessDef};

/// The harness a provider fronts. Written as `grok` and `antigravity`.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[map(crate = ::coqui_acp_api::guatiao, rename_all = "snake_case")]
#[non_exhaustive]
pub enum StdioHarness {
    /// Grok, started as `grok agent stdio`.
    Grok,
    /// Antigravity, started as `agy acp`.
    Antigravity,
}

impl StdioHarness {
    fn def(self) -> &'static HarnessDef {
        match self {
            Self::Grok => &accounts::grok::DEF,
            Self::Antigravity => &accounts::antigravity::DEF,
        }
    }
}

/// How the stdio provider is configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct StdioSettings {
    /// The harness: `grok` or `antigravity`.
    pub harness: StdioHarness,
    /// The program: a path, or a name looked up on the `PATH` where the spawner runs it. The harness's own (`grok`, `agy`)
    /// when absent.
    pub executable: Option<String>,
    /// The highest ACP version the harness is said to answer: 1 or 2.
    #[map(default = 1)]
    pub max_acp_version: u64,
    /// How often the account's login file is looked at while a connection runs, in milliseconds; 0 looks only at calls and
    /// at close.
    #[map(default = 30_000)]
    pub renew_poll_ms: u64,
    /// How long a command of the harness's own (a renewal) may run before it is ended, in milliseconds.
    #[map(default = 60_000)]
    pub command_timeout_ms: u64,
    /// Requests one connection may have in hand at once: a client that sends more is held back until the sink has taken
    /// answers.
    #[map(default = 32)]
    pub max_requests: u64,
    /// The largest message a connection takes, in bytes; one more ends it.
    #[map(default = 67_108_864)]
    pub max_message_bytes: u64,
    /// How many bytes of an imported history the first prompt of a session carries, as context the agent reads. 0, the
    /// default, imports nothing: these agents keep no transcript the provider can write, and a replay sends the history to
    /// the agent's model, so it is a choice. A history over it keeps its newest turns and the import reports what it
    /// cut.
    #[map(default = 0)]
    pub replay_max_bytes: u64,
    /// The most bytes of one message, tool input or tool output in a replay; one over it is cut and reported.
    #[map(default = 4096)]
    pub replay_max_item_bytes: u64,
    /// Regular expressions whose matches are removed from the history before it is replayed (`[redacted]` in their place).
    /// A replay carries tool output and messages verbatim, so a secret a tool once printed would go to the agent's
    /// model: list the patterns of the secrets your sessions can hold.
    #[map(default)]
    pub replay_redact_patterns: Vec<String>,
    /// How long an imported history waits for a connection to open its session before it is forgotten, in seconds (an
    /// hour by default; 0 forgets it at once).
    #[map(default = 3600)]
    pub replay_waiting_secs: u64,
    /// How many session ids the agent reported are remembered, so that no import takes one (4096 by default; 0 remembers
    /// none).
    #[map(default = 4096)]
    pub replay_remembered_ids: u64,
    /// How many imported histories may wait to be opened at once (64 by default; 0 refuses every import).
    #[map(default = 64)]
    pub replay_waiting_max: u64,
}

impl StdioSettings {
    /// The settings for `harness`, every other value its default.
    #[must_use]
    pub const fn new(harness: StdioHarness) -> Self {
        Self {
            harness,
            executable: None,
            max_acp_version: 1,
            renew_poll_ms: DEFAULT_POLL_MS,
            command_timeout_ms: 60_000,
            max_requests: 32,
            max_message_bytes: 64 * 1024 * 1024,
            replay_max_bytes: 0,
            replay_max_item_bytes: 4096,
            replay_redact_patterns: Vec::new(),
            replay_waiting_secs: replay::WAITING_SECS,
            replay_remembered_ids: replay::REMEMBERED_IDS,
            replay_waiting_max: replay::WAITING_MAX,
        }
    }
}

/// Grok or Antigravity, as a provider of `acp_harness` under the id `stdio`.
///
/// Register it beside a host's own providers with `Registry::register_local`, or build it into a library with
/// `guatiao::providers!`.
#[derive(Provider, Debug)]
#[provider(
    AcpHarness,
    id = "stdio",
    config = StdioSettings,
    new = StdioProvider::unconfigured,
    crate = ::coqui_acp_api::guatiao
)]
pub struct StdioProvider {
    settings: StdioSettings,
    accounts: Arc<Accounts>,
    /// Sessions imported by replay, shared by every connection.
    replays: Arc<replay::Replays>,
}

impl From<StdioSettings> for StdioProvider {
    fn from(settings: StdioSettings) -> Self {
        Self::new(settings)
    }
}

fn unsupported(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, why)
}

/// The requirements the provider holds: none, so why the first cannot be, `None` for no requirement. A requirement that
/// cannot be read is `GUATIAO_ERR_BAD_VALUE`.
fn refuse_requirements(requirements: &[Requirement]) -> Result<Option<String>, ProviderError> {
    let narrowings = match Narrowing::read_all(requirements) {
        Ok(narrowings) => narrowings,
        Err(error) if error.status() == Status::GUATIAO_ERR_UNSUPPORTED => {
            return Ok(Some(error.to_string()));
        }
        Err(error) => return Err(error.into()),
    };
    Ok(narrowings.into_iter().next().map(|narrowing| {
        let name = Requirement::from(narrowing).name;
        alloc::format!(
            "this provider cannot hold a connection to `{name}`: it starts the agent as it is"
        )
    }))
}

impl StdioProvider {
    /// A provider configured as `settings` say.
    #[must_use]
    pub fn new(settings: StdioSettings) -> Self {
        let replays = replay::Replays::limited(
            Duration::from_secs(settings.replay_waiting_secs),
            usize::try_from(settings.replay_remembered_ids).unwrap_or(usize::MAX),
            usize::try_from(settings.replay_waiting_max).unwrap_or(usize::MAX),
        );
        Self {
            settings,
            accounts: Arc::new(Accounts::new()),
            replays: Arc::new(replays),
        }
    }

    fn unconfigured() -> Self {
        Self::new(StdioSettings::new(StdioHarness::Grok))
    }

    fn def(&self) -> &'static HarnessDef {
        self.settings.harness.def()
    }

    fn program_name(&self) -> String {
        self.settings
            .executable
            .clone()
            .unwrap_or_else(|| self.def().program.to_string())
    }

    fn poll(&self) -> Duration {
        Duration::from_millis(self.settings.renew_poll_ms)
    }

    /// Reads `grant` against the harness, and answers what applies it; `None` for `harness-login`.
    fn applied(grant: &AccountGrant, def: &HarnessDef) -> Result<Option<Applied>, ProviderError> {
        accounts::apply(def, grant)
    }

    /// Takes the hold on the account's login file for a call or a connection, when the kind has one.
    ///
    /// # Errors
    ///
    /// `GUATIAO_ERR_UNSUPPORTED` when the kind needs files and the spawner has none; the spawner's refusal to place them.
    fn hold(
        &self,
        grant: &AccountGrant,
        applied: &Applied,
        spawner: &Arc<Object<dyn ProcessSpawner>>,
        keeper: Option<Object<dyn AccountKeeper>>,
    ) -> Result<Option<Hold>, ProviderError> {
        let files = Files::new(Arc::clone(spawner));
        if applied.needs_files && !files.available() {
            return Err(unsupported(&alloc::format!(
                "this spawner has no private files, which a {} account needs",
                grant.kind
            )));
        }
        let (Some(watched), Some(dir)) = (applied.watched, grant.state_dir.as_deref()) else {
            return Ok(None);
        };
        self.accounts
            .hold(
                files,
                keeper,
                &grant.id,
                dir,
                grant.revision,
                grant.credentials.expose(),
                vec![Watched::new(watched.path, watched.key).usable(accounts::login_usable)],
            )
            .map(Some)
    }

    /// What a call that runs nothing does for an account: reads it, and looks at its login file so that a renewal or a
    /// loss that happened since is told.
    fn glance(
        &self,
        account: Option<&AccountGrant>,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<(), ProviderError> {
        let Some(grant) = account else {
            return Ok(());
        };
        let Some(applied) = Self::applied(grant, self.def())? else {
            return Ok(());
        };
        let spawner = Arc::new(spawner);
        if let Some(hold) = self.hold(grant, &applied, &spawner, Some(keeper))? {
            let _ = hold.look();
            hold.finish();
        }
        Ok(())
    }

    /// What the program is started with: the host's environment, less the harness's credential variables when an account
    /// is applied, plus the account's.
    fn spec(
        &self,
        environment: &Environment,
        applied: Option<&Applied>,
        args: &[&str],
    ) -> SpawnSpec {
        let def = self.def();
        let mut spec = SpawnSpec::new(self.program_name());
        for arg in args {
            spec = spec.arg(*arg);
        }
        let mut variables = Environment::new();
        for (name, value) in environment.iter() {
            if applied.is_some() && def.variables.iter().any(|v| accounts::same_name(v, name)) {
                continue;
            }
            variables.insert(name.as_str(), value.as_str());
        }
        if let Some(applied) = applied {
            for (name, value) in applied.environment.expose().iter() {
                variables.insert(name.as_str(), value.as_str());
            }
            spec.remove_environment = def.variables.iter().map(|v| (*v).to_string()).collect();
        }
        spec.environment(variables)
    }
}

impl AcpHarness for StdioProvider {
    fn describe(&self) -> Result<HarnessInfo, ProviderError> {
        let versions = if self.settings.max_acp_version >= 2 {
            vec![1, 2]
        } else {
            vec![1]
        };
        // The program answers `initialize` itself: nothing is claimed of what it takes beyond the protocol.
        Ok(HarnessInfo::new(self.def().name, versions))
    }

    fn accounts(&self) -> Result<AccountCatalog, ProviderError> {
        let logins = match self.settings.harness {
            StdioHarness::Grok => accounts::grok::logins(),
            StdioHarness::Antigravity => Vec::new(),
        };
        Ok(accounts::catalog(self.def(), logins))
    }

    fn start(
        &self,
        request: StartRequest,
        spawner: Object<dyn ProcessSpawner>,
        sink: Object<dyn ChannelSink>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<Object<dyn Channel>, ProviderError> {
        if let Some(why) = refuse_requirements(&request.requirements)? {
            return Err(unsupported(&why));
        }
        let applied = match &request.account {
            Some(grant) => Self::applied(grant, self.def())?,
            None => None,
        };
        let spawner = Arc::new(spawner);
        let hold = match (&request.account, &applied) {
            (Some(grant), Some(applied)) => self
                .hold(grant, applied, &spawner, Some(keeper))?
                .map(|hold| (hold, self.poll())),
            _ => None,
        };
        let spec = self.spec(
            request.environment.expose(),
            applied.as_ref(),
            self.def().args,
        );
        relay::open(relay::Params {
            spawner,
            spec,
            sink,
            max_message_bytes: usize::try_from(self.settings.max_message_bytes)
                .unwrap_or(usize::MAX),
            max_requests: usize::try_from(self.settings.max_requests).unwrap_or(usize::MAX),
            roots: WorkingRoots::new(request.working_roots.clone()),
            hold,
            replays: Arc::clone(&self.replays),
        })
    }

    fn refusal(
        &self,
        requirements: Vec<Requirement>,
        _environment: Secret<Environment>,
        account: Option<AccountRef>,
    ) -> Result<Option<String>, ProviderError> {
        if let Some(why) = refuse_requirements(&requirements)? {
            return Ok(Some(why));
        }
        if let Some(account) = account {
            accounts::known(self.def(), &account)?;
        }
        Ok(None)
    }

    fn program(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.program_name()))
    }

    fn import_session(
        &self,
        session: SessionImport,
        _spawner: Object<dyn ProcessSpawner>,
        _keeper: Object<dyn AccountKeeper>,
    ) -> Result<ImportReport, ProviderError> {
        if let Some(grant) = &session.account {
            accounts::known(self.def(), &grant.reference())?;
        }
        if self.settings.replay_max_bytes == 0 {
            return Err(unsupported(
                "this provider does not seed a session in an agent it does not keep the sessions of; set replay_max_bytes to replay the history as context in the first prompt",
            ));
        }
        let bad = |why: &str| ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, why);
        if uuid::Uuid::parse_str(&session.session_id).is_err() {
            return Err(bad("the session id is not a UUID"));
        }
        let replay = if session.history.trim().is_empty() {
            None
        } else {
            let document =
                Document::from_slice(session.history.as_bytes()).map_err(|error| match error {
                    HistoryError::UnknownVersion { .. } | HistoryError::UnknownFormat => {
                        unsupported(
                            "this provider does not read that version of the history format",
                        )
                    }
                    _ => bad("the history is not one this provider can read"),
                })?;
            let mut settings = ReplaySettings::default();
            settings.max_bytes = self.settings.replay_max_bytes;
            settings.max_item_bytes = self.settings.replay_max_item_bytes;
            if self.settings.replay_redact_patterns.is_empty() {
                Some(Replay::of(&document, &settings))
            } else {
                let mut redaction = RedactionSettings::default();
                redaction
                    .patterns
                    .clone_from(&self.settings.replay_redact_patterns);
                let mut redactor = Redactor::try_new(&redaction).map_err(|_| {
                    bad("a pattern to redact from a replay is not a usable expression")
                })?;
                let redacted =
                    Replay::of_redacted(&document, &settings, &mut redactor).map_err(|_| {
                        bad("the history cannot be redacted into one this provider can read")
                    })?;
                Some(redacted)
            }
        };
        let context = replay.as_ref().and_then(Replay::block);
        self.replays
            .add(&session.session_id, context)
            .map_err(|refusal| match refusal {
                replay::Refusal::Named => bad("the id already names an imported session"),
                replay::Refusal::Live => bad(
                    "the id names a session the agent holds: import only at a UUID the host minted",
                ),
                replay::Refusal::Full => ProviderError::new(
                    Status::GUATIAO_ERR_BUSY,
                    "too many imported sessions wait to be opened",
                ),
            })?;
        let mut report = ImportReport::new();
        if let Some(replay) = replay {
            report.seeded_from = replay.seeded_from().map(|point| {
                let mut seed = SeedPoint::line(point.line);
                seed.digest = Some(point.digest.clone());
                seed
            });
            report.losses = replay
                .report()
                .losses()
                .iter()
                .map(|loss| {
                    let mut out = HistoryLoss::new(loss.kind.clone(), loss.count);
                    out.detail.clone_from(&loss.detail);
                    out
                })
                .collect();
            report.compacted = replay.cut_items() > 0;
        }
        Ok(report)
    }

    fn export_session(
        &self,
        export: SessionExport,
        _spawner: Object<dyn ProcessSpawner>,
        _keeper: Object<dyn AccountKeeper>,
    ) -> Result<ExportPage, ProviderError> {
        if let Some(grant) = &export.account {
            accounts::known(self.def(), &grant.reference())?;
        }
        Err(unsupported(
            "this provider does not read a session out of an agent it does not keep the sessions of",
        ))
    }

    fn models(
        &self,
        request: ModelsRequest,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<ModelListing, ProviderError> {
        self.glance(request.account.as_ref(), spawner, keeper)?;
        Ok(ModelListing::unknown(
            "the agent lists its models in its session, not before it",
        ))
    }

    fn usage(
        &self,
        query: UsageQuery,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<UsageReport, ProviderError> {
        self.glance(query.account.as_ref(), spawner, keeper)?;
        Ok(UsageReport::unsupported(
            "this harness reports no usage outside a turn",
        ))
    }

    fn login(
        &self,
        request: LoginRequest,
        spawner: Object<dyn ProcessSpawner>,
        prompter: Object<dyn LoginPrompter>,
    ) -> Result<LoginOutcome, ProviderError> {
        match self.settings.harness {
            StdioHarness::Grok => {
                accounts::grok::sign_in(self.program_name(), request, spawner, prompter)
            }
            StdioHarness::Antigravity => Err(unsupported(self.def().login_unsupported)),
        }
    }

    fn renew(
        &self,
        request: RenewRequest,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<RenewReport, ProviderError> {
        let def = self.def();
        let grant = request.account;
        let Some(applied) = Self::applied(&grant, def)? else {
            return Err(unsupported(
                "the harness's own login is the harness's to renew",
            ));
        };
        let kind = def.kind(&grant.kind).ok_or_else(|| {
            ProviderError::new(
                Status::GUATIAO_ERR_BAD_VALUE,
                "this harness takes no account of that kind",
            )
        })?;
        let args = match (kind.renews, def.renew_args) {
            (true, Some(args)) => args,
            _ => return Err(unsupported(kind.renew_unsupported)),
        };
        let spawner = Arc::new(spawner);
        let Some(hold) = self.hold(&grant, &applied, &spawner, None)? else {
            return Err(unsupported(kind.renew_unsupported));
        };
        let renewed = self.renew_with(&hold, &spawner, &grant, &applied, args);
        hold.finish();
        renewed
    }
}

impl StdioProvider {
    fn renew_with(
        &self,
        hold: &Hold,
        spawner: &Arc<Object<dyn ProcessSpawner>>,
        grant: &AccountGrant,
        applied: &Applied,
        args: &[&str],
    ) -> Result<RenewReport, ProviderError> {
        let gone = |why: String| ProviderError::new(Status::GUATIAO_ERR_GONE, &why);
        let lost = |hold: &Hold| -> Option<String> {
            let found = hold.look().ok()?;
            found.lost.into_iter().next().or_else(|| hold.lost())
        };
        if let Some(why) = lost(hold) {
            return Err(gone(why));
        }
        let mut launch = Launch::new(self.program_name()).args(args.iter().copied());
        for (name, value) in applied.environment.expose().iter() {
            launch = launch.env(name.as_str(), value.as_str());
        }
        launch.remove_env = self.def().variables.iter().map(OsString::from).collect();
        let done = cli::run(
            &ThroughSpawner::shared(Arc::clone(spawner)),
            &launch,
            None,
            Duration::from_millis(self.settings.command_timeout_ms),
            4096,
        )
        .map_err(|_| {
            ProviderError::new(
                Status::GUATIAO_ERR_INTERNAL,
                "the renewal's program could not be started",
            )
        })?;
        if !done.succeeded() {
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_INTERNAL,
                if done.timed_out {
                    "the renewal's program did not finish in time"
                } else {
                    "the renewal's program did not succeed"
                },
            ));
        }
        self.refuse_another_account(spawner, grant, applied)?;
        let found = hold.look()?;
        if let Some(why) = found.lost.into_iter().next().or_else(|| hold.lost()) {
            return Err(gone(why));
        }
        match found.renewed.into_iter().next_back() {
            Some((credentials, expires)) => {
                let mut report = RenewReport::default();
                report.credentials = Some(credentials);
                report.expires_at = expires;
                Ok(report)
            }
            None => Ok(RenewReport::unchanged(
                "the login is not near its end, so the harness left it as it was",
            )),
        }
    }

    /// A renewal that left the login of another account in the file is undone: the grant's copy is put back, and the
    /// renewal is refused. Only a file that says whose login it is can be told apart.
    fn refuse_another_account(
        &self,
        spawner: &Arc<Object<dyn ProcessSpawner>>,
        grant: &AccountGrant,
        applied: &Applied,
    ) -> Result<(), ProviderError> {
        let Some(watched) = applied.watched else {
            return Ok(());
        };
        let Some(mine) = grant
            .credentials
            .expose()
            .get(watched.key)
            .and_then(accounts::identity_of)
        else {
            return Ok(());
        };
        let files = Files::new(Arc::clone(spawner));
        let Ok(Some(now)) = files.read(watched.path) else {
            return Ok(());
        };
        if accounts::identity_of(&now).is_some_and(|theirs| theirs != mine) {
            if let Some(old) = grant.credentials.expose().get(watched.key) {
                let _ = files.write(watched.path, old);
            }
            return Err(ProviderError::new(
                Status::GUATIAO_ERR_BAD_VALUE,
                "the renewed login belongs to another account",
            ));
        }
        Ok(())
    }
}
