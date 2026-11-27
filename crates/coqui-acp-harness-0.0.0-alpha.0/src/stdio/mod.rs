//! A provider of the `acp_harness` kind for coding agents that speak ACP on their own standard input and output.
//!
//! The harness is data, a [`StdioDef`]: how its program starts and the kinds of account it takes and how they are applied
//! ([`HarnessDef`]), its sign-in, and how it keeps its sessions. [`StdioProvider`] runs any one, and
//! [`stdio_vendor!`](crate::stdio_vendor) makes the two providers a host registers for it, a harness and its accounts,
//! each under an id of its own. A harness with a table of its own is a `HarnessDef` and one such call; the ones this crate
//! carries are in [`vendors`](crate::vendors).
//!
//! The host's messages go to the program a line each and its lines come back as messages; what the
//! provider adds is the account: it removes every credential variable of the harness from what it starts, sets the chosen
//! kind's and places the login file in the account's directory. Signing in, renewing and watching the login are the
//! account provider's ([`StdioAccounts`]).
//!
//! A session's id must be a UUID, as the kind says: the provider does not translate the program's ids, except for a
//! harness whose [`SessionIds`] say how it spells the id of a session written into its store, which
//! the relay turns into the UUID for the host and back.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Schema, Secret, Status, ToValue};
use coqui_acp_api::{
    AccountCatalog, AccountGrant, AccountRef, AcpHarness, Channel, ChannelSink, Environment,
    ExportPage, HarnessInfo, HistoryLoss, ImportReport, Narrowing, ProcessSpawner, Requirement,
    SeedPoint, SessionExport, SessionImport, SpawnSpec, StartRequest, WorkingRoots,
};
use coqui_acp_history::replay::{Replay, ReplaySettings};
use coqui_acp_history::{Document, HistoryError, RedactionSettings, Redactor};

use crate::account::{Accounts, Files, Hold, Watched};

mod account_provider;
pub mod accounts;
mod def;
pub mod login;
mod macros;
mod relay;
mod replay;

pub use account_provider::{StdioAccountSettings, StdioAccounts};
pub use def::{ExportSession, ImportSession, LoginCheck, SessionIds, SignIn, StdioDef, no_logins};

use accounts::{Applied, HarnessDef};

/// How the stdio provider is configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct StdioSettings {
    /// The program: a path, or a name looked up on the `PATH` where the spawner runs it. The harness's own when absent.
    pub executable: Option<String>,
    /// The highest ACP version the harness is said to answer: 1 or 2.
    #[map(default = 1)]
    pub max_acp_version: u64,
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
    /// The defaults.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            executable: None,
            max_acp_version: 1,
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

impl Default for StdioSettings {
    fn default() -> Self {
        Self::new()
    }
}

/// One harness, described by its [`StdioDef`], as a provider of `acp_harness`.
///
/// It is no registered provider itself: a vendor's provider holds one (see [`stdio_vendor!`](crate::stdio_vendor)).
#[derive(Debug)]
pub struct StdioProvider {
    def: &'static StdioDef,
    settings: StdioSettings,
    /// What the provider knows of the files it placed for its accounts.
    accounts: Arc<Accounts>,
    /// Sessions imported by replay, shared by every connection.
    replays: Arc<replay::Replays>,
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
    /// A provider of the harness `def` configured as `settings` say.
    #[must_use]
    pub fn new(def: &'static StdioDef, settings: StdioSettings) -> Self {
        let replays = replay::Replays::limited(
            Duration::from_secs(settings.replay_waiting_secs),
            usize::try_from(settings.replay_remembered_ids).unwrap_or(usize::MAX),
            usize::try_from(settings.replay_waiting_max).unwrap_or(usize::MAX),
        );
        Self {
            def,
            settings,
            accounts: Arc::new(Accounts::new()),
            replays: Arc::new(replays),
        }
    }

    /// The harness this provider runs.
    #[must_use]
    pub const fn def(&self) -> &'static StdioDef {
        self.def
    }

    /// The settings it was configured with.
    #[must_use]
    pub const fn settings(&self) -> &StdioSettings {
        &self.settings
    }

    /// The program it starts: the settings' `executable`, else the harness's own.
    #[must_use]
    pub fn program_name(&self) -> String {
        self.settings
            .executable
            .clone()
            .unwrap_or_else(|| self.def.accounts.program.to_string())
    }

    /// Reads `grant` against the harness, and answers what applies it; `None` for `harness-login`.
    fn applied(grant: &AccountGrant, def: &HarnessDef) -> Result<Option<Applied>, ProviderError> {
        accounts::apply(def, grant)
    }

    /// Takes the hold on the account's login file for a connection, when the kind has one: the grant's copy is placed
    /// there. What happens to the file afterwards is for the account provider's watch to tell.
    ///
    /// # Errors
    ///
    /// `GUATIAO_ERR_UNSUPPORTED` when the kind needs files and the spawner has none; the spawner's refusal to place them.
    fn hold(
        &self,
        grant: &AccountGrant,
        applied: &Applied,
        spawner: &Arc<Object<dyn ProcessSpawner>>,
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
                None,
                &grant.id,
                dir,
                grant.revision,
                grant.credentials.expose(),
                vec![Watched::new(watched.path, watched.key).usable(accounts::login_usable)],
            )
            .map(Some)
    }

    /// What the program is started with: the host's environment, less the harness's credential variables when an account
    /// is applied, plus the account's.
    fn spec(
        &self,
        environment: &Environment,
        applied: Option<&Applied>,
        args: &[&str],
    ) -> SpawnSpec {
        let def = self.def.accounts;
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
        Ok(HarnessInfo::new(self.def.accounts.name, versions))
    }

    fn accounts(&self) -> Result<AccountCatalog, ProviderError> {
        Ok(accounts::catalog(self.def.accounts))
    }

    fn start(
        &self,
        request: StartRequest,
        spawner: Object<dyn ProcessSpawner>,
        sink: Object<dyn ChannelSink>,
    ) -> Result<Object<dyn Channel>, ProviderError> {
        if let Some(why) = refuse_requirements(&request.requirements)? {
            return Err(unsupported(&why));
        }
        let applied = match &request.account {
            Some(grant) => Self::applied(grant, self.def.accounts)?,
            None => None,
        };
        let spawner = Arc::new(spawner);
        let hold = match (&request.account, &applied) {
            (Some(grant), Some(applied)) => self.hold(grant, applied, &spawner)?,
            _ => None,
        };
        let spec = self.spec(
            request.environment.expose(),
            applied.as_ref(),
            self.def.accounts.args,
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
            session_ids: self.def.session_ids,
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
            accounts::known(self.def.accounts, &account)?;
        }
        Ok(None)
    }

    fn program(&self) -> Result<Option<String>, ProviderError> {
        Ok(Some(self.program_name()))
    }

    fn import_session(
        &self,
        session: SessionImport,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ImportReport, ProviderError> {
        if let Some(grant) = &session.account {
            accounts::known(self.def.accounts, &grant.reference())?;
        }
        if let Some(import) = self.def.import {
            // A harness that takes a history into its own store; replaying it as context is what is left when it cannot.
            match import(self, &session, spawner) {
                Ok(report) => return Ok(report),
                Err(error)
                    if self.settings.replay_max_bytes > 0
                        && [
                            u32::from(Status::GUATIAO_ERR_UNSUPPORTED),
                            u32::from(Status::GUATIAO_ERR_INTERNAL),
                        ]
                        .contains(&u32::from(error.status)) => {}
                Err(error) => return Err(error),
            }
        }
        if self.settings.replay_max_bytes == 0 {
            return Err(unsupported(self.def.import_unsupported));
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
        export_request: SessionExport,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ExportPage, ProviderError> {
        if let Some(grant) = &export_request.account {
            accounts::known(self.def.accounts, &grant.reference())?;
        }
        match self.def.export {
            Some(export) => export(self, &export_request, spawner),
            None => Err(unsupported(self.def.export_unsupported)),
        }
    }
}
