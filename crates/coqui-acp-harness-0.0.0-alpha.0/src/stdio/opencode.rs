//! OpenCode as a provider of its own: the stdio provider and its account provider with the harness fixed, under the ids
//! `opencode` and `opencode_accounts`, so a host that runs several agents through `stdio` can still say which one an entry
//! and its accounts are. Reading a session out of OpenCode (`opencode export`) and writing one into its store
//! (`opencode import`) are here too.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;
use std::ffi::OsString;
use std::path::PathBuf;

use coqui_acp_api::guatiao::Secret;
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use coqui_acp_api::{
    AccountCatalog, AccountKeeper, AccountOffer, AccountProvider, AccountProviderInfo, AccountRef,
    AccountUsage, AccountUsageQuery, AccountWatch, AcpHarness, Channel, ChannelSink, Environment,
    ExportPage, HarnessInfo, HistoryLoss, ImportReport, LINE_TOO_LARGE, LoginOutcome,
    LoginPrompter, LoginRequest, ModelListing, ModelsRequest, ProcessSpawner, RenewReport,
    RenewRequest, Requirement, SeedPoint, SessionExport, SessionImport, StartRequest, UsageQuery,
    UsageReport, WatchRequest,
};
use coqui_acp_history::paging::{PageError, page};
use coqui_acp_history::{Document, Header, HistoryError, Origin, RedactionSettings, Redactor};

use super::accounts;
use super::{
    StdioAccountSettings, StdioAccounts, StdioHarness, StdioProvider, StdioSettings,
    opencode_history, opencode_import,
};
use crate::ThroughSpawner;
use crate::account::Files;
use crate::cli;
use crate::process::Launch;

/// The longest `opencode export` may run, and the most it may print.
const EXPORT_LIMIT: Duration = Duration::from_secs(60);
const EXPORT_MAX_BYTES: usize = 64 << 20;

/// How OpenCode's provider is configured: [`StdioSettings`] with the harness fixed and the ACP version OpenCode speaks.
#[derive(ToValue, FromValue, Schema, Debug, Clone)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct OpenCodeSettings {
    /// The program: a path, or a name looked up on the `PATH` where the spawner runs it. `opencode` when absent.
    pub executable: Option<String>,
    /// Requests one connection may have in hand at once: a client that sends more is held back until the sink has taken
    /// answers.
    #[map(default = 32)]
    pub max_requests: u64,
    /// The largest message a connection takes, in bytes; one more ends it.
    #[map(default = 67_108_864)]
    pub max_message_bytes: u64,
    /// How many bytes of an imported history the first prompt of a session carries, as context the agent reads, when the
    /// history cannot be written into OpenCode's store (no private files in the account's folder, or a spawner that runs programs elsewhere). 0, the
    /// default, replays nothing: a replay sends the history to the agent's model, so it is a choice.
    #[map(default = 0)]
    pub replay_max_bytes: u64,
    /// The most bytes of one message, tool input or tool output in a replay; one over it is cut and reported.
    #[map(default = 4096)]
    pub replay_max_item_bytes: u64,
    /// Regular expressions whose matches are removed from the history before it is replayed.
    #[map(default)]
    pub replay_redact_patterns: Vec<String>,
    /// How long an imported history waits for a connection to open its session before it is forgotten, in seconds.
    #[map(default = 3600)]
    pub replay_waiting_secs: u64,
    /// How many session ids the agent reported are remembered, so that no import takes one.
    #[map(default = 4096)]
    pub replay_remembered_ids: u64,
    /// How many imported histories may wait to be opened at once.
    #[map(default = 64)]
    pub replay_waiting_max: u64,
}

impl OpenCodeSettings {
    /// The defaults.
    #[must_use]
    pub fn new() -> Self {
        let own = StdioSettings::new(StdioHarness::OpenCode);
        Self {
            executable: None,
            max_requests: own.max_requests,
            max_message_bytes: own.max_message_bytes,
            replay_max_bytes: own.replay_max_bytes,
            replay_max_item_bytes: own.replay_max_item_bytes,
            replay_redact_patterns: Vec::new(),
            replay_waiting_secs: own.replay_waiting_secs,
            replay_remembered_ids: own.replay_remembered_ids,
            replay_waiting_max: own.replay_waiting_max,
        }
    }
}

impl Default for OpenCodeSettings {
    fn default() -> Self {
        Self::new()
    }
}

impl From<OpenCodeSettings> for StdioSettings {
    fn from(settings: OpenCodeSettings) -> Self {
        let mut own = Self::new(StdioHarness::OpenCode);
        own.executable = settings.executable;
        own.max_requests = settings.max_requests;
        own.max_message_bytes = settings.max_message_bytes;
        own.replay_max_bytes = settings.replay_max_bytes;
        own.replay_max_item_bytes = settings.replay_max_item_bytes;
        own.replay_redact_patterns = settings.replay_redact_patterns;
        own.replay_waiting_secs = settings.replay_waiting_secs;
        own.replay_remembered_ids = settings.replay_remembered_ids;
        own.replay_waiting_max = settings.replay_waiting_max;
        own
    }
}

/// OpenCode, as a provider of `acp_harness` under the id `opencode`: `opencode acp` through the stdio provider.
#[derive(Provider, Debug)]
#[provider(
    AcpHarness,
    id = "opencode",
    config = OpenCodeSettings,
    new = OpenCodeProvider::unconfigured,
    crate = ::coqui_acp_api::guatiao
)]
pub struct OpenCodeProvider {
    inner: StdioProvider,
}

impl From<OpenCodeSettings> for OpenCodeProvider {
    fn from(settings: OpenCodeSettings) -> Self {
        Self {
            inner: StdioProvider::new(settings.into()),
        }
    }
}

impl OpenCodeProvider {
    /// A provider configured as `settings` say.
    #[must_use]
    pub fn new(settings: OpenCodeSettings) -> Self {
        settings.into()
    }

    fn unconfigured() -> Self {
        Self::new(OpenCodeSettings::new())
    }
}

impl AcpHarness for OpenCodeProvider {
    fn describe(&self) -> Result<HarnessInfo, ProviderError> {
        self.inner.describe()
    }

    fn accounts(&self) -> Result<AccountCatalog, ProviderError> {
        self.inner.accounts()
    }

    fn start(
        &self,
        request: StartRequest,
        spawner: Object<dyn ProcessSpawner>,
        sink: Object<dyn ChannelSink>,
    ) -> Result<Object<dyn Channel>, ProviderError> {
        self.inner.start(request, spawner, sink)
    }

    fn refusal(
        &self,
        requirements: Vec<Requirement>,
        environment: Secret<Environment>,
        account: Option<AccountRef>,
    ) -> Result<Option<String>, ProviderError> {
        self.inner.refusal(requirements, environment, account)
    }

    fn program(&self) -> Result<Option<String>, ProviderError> {
        self.inner.program()
    }

    fn import_session(
        &self,
        session: SessionImport,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ImportReport, ProviderError> {
        self.inner.import_session(session, spawner)
    }

    fn export_session(
        &self,
        export: SessionExport,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ExportPage, ProviderError> {
        self.inner.export_session(export, spawner)
    }

    fn models(
        &self,
        request: ModelsRequest,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ModelListing, ProviderError> {
        self.inner.models(request, spawner)
    }

    fn usage(
        &self,
        query: UsageQuery,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<UsageReport, ProviderError> {
        self.inner.usage(query, spawner)
    }
}

/// How OpenCode's account provider is configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct OpenCodeAccountSettings {
    /// The program it signs in and checks with: a path, or a name looked up on the `PATH` where the spawner runs it.
    /// `opencode` when absent.
    pub executable: Option<String>,
    /// How often, in milliseconds, a watch looks at the file an account keeps its sign-in in; 0 looks only when the watch
    /// begins and ends, and when a call is made on the account.
    #[map(default = 30_000)]
    pub renew_poll_ms: u64,
    /// The longest, in milliseconds, a command of the harness's own may run; 0 ends it at once.
    #[map(default = 60_000)]
    pub command_timeout_ms: u64,
}

/// OpenCode's accounts, as a provider of `account_provider` under the id `opencode_accounts`: the sign-in kept in
/// `auth.json` and a key for one provider.
#[derive(Provider, Debug)]
#[provider(
    AccountProvider,
    id = "opencode_accounts",
    config = OpenCodeAccountSettings,
    new = OpenCodeAccounts::unconfigured,
    crate = ::coqui_acp_api::guatiao
)]
pub struct OpenCodeAccounts {
    inner: StdioAccounts,
}

impl From<OpenCodeAccountSettings> for OpenCodeAccounts {
    fn from(settings: OpenCodeAccountSettings) -> Self {
        let mut own = StdioAccountSettings::new(StdioHarness::OpenCode);
        own.executable = settings.executable;
        own.renew_poll_ms = settings.renew_poll_ms;
        own.command_timeout_ms = settings.command_timeout_ms;
        Self {
            inner: StdioAccounts::new(own),
        }
    }
}

impl OpenCodeAccounts {
    /// A provider configured as `settings` say.
    #[must_use]
    pub fn new(settings: OpenCodeAccountSettings) -> Self {
        settings.into()
    }

    fn unconfigured() -> Self {
        Self::new(OpenCodeAccountSettings {
            executable: None,
            renew_poll_ms: 30_000,
            command_timeout_ms: 60_000,
        })
    }
}

impl AccountProvider for OpenCodeAccounts {
    fn describe(&self) -> Result<AccountProviderInfo, ProviderError> {
        self.inner.describe()
    }

    fn offers(&self) -> Result<AccountOffer, ProviderError> {
        self.inner.offers()
    }

    fn login(
        &self,
        request: LoginRequest,
        spawner: Object<dyn ProcessSpawner>,
        prompter: Object<dyn LoginPrompter>,
    ) -> Result<LoginOutcome, ProviderError> {
        self.inner.login(request, spawner, prompter)
    }

    fn renew(
        &self,
        request: RenewRequest,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<RenewReport, ProviderError> {
        self.inner.renew(request, spawner)
    }

    fn watch(
        &self,
        request: WatchRequest,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<Object<dyn AccountWatch>, ProviderError> {
        self.inner.watch(request, spawner, keeper)
    }

    fn usage(
        &self,
        query: AccountUsageQuery,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<AccountUsage, ProviderError> {
        self.inner.usage(query, spawner, keeper)
    }
}

fn error(status: Status, why: &str) -> ProviderError {
    ProviderError::new(status, why)
}

/// The file `opencode import` reads, where it was put, and what takes it away when this is dropped.
struct Placed {
    /// The path the program is given.
    path: String,
    cleanup: Cleanup,
}

enum Cleanup {
    /// A file in the account's folder, removed through the spawner that put it there.
    Account(Files, String),
    /// A folder made for the import alone, removed with what is in it.
    Folder(PathBuf),
}

impl Placed {
    /// `text` at `name` in the account's folder `folder`, through the spawner's private files.
    fn in_account(
        spawner: &Arc<Object<dyn ProcessSpawner>>,
        folder: &str,
        name: &str,
        text: &str,
    ) -> Result<Self, ProviderError> {
        let files = Files::new(Arc::clone(spawner));
        if !files.available() {
            return Err(error(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "this spawner has no private files to put the session's file in",
            ));
        }
        files.write(name, text)?;
        Ok(Self {
            path: alloc::format!("{}/{name}", folder.trim_end_matches(['/', '\\'])),
            cleanup: Cleanup::Account(files, name.to_owned()),
        })
    }

    /// `text` at `name` in a new folder of this machine's temporary files, made for this import and closed to others.
    fn in_own_folder(
        spawner: &Arc<Object<dyn ProcessSpawner>>,
        name: &str,
        text: &str,
    ) -> Result<Self, ProviderError> {
        if !spawner.describe()?.local {
            return Err(error(
                Status::GUATIAO_ERR_UNSUPPORTED,
                "this spawner runs programs on another machine, which cannot read a file made here, and there is no account folder to put the session's file in",
            ));
        }
        let failed = |_| {
            error(
                Status::GUATIAO_ERR_INTERNAL,
                "the session's file could not be written",
            )
        };
        let folder = std::env::temp_dir().join(alloc::format!(
            "coqui-acp-opencode-{}",
            uuid::Uuid::new_v4()
        ));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        // Not recursive: the folder is made here and is new, or the call fails.
        builder.create(&folder).map_err(failed)?;
        // Dropped on a failure from here on, so that the folder never outlives the call.
        let placed = Self {
            path: folder.join(name).to_string_lossy().into_owned(),
            cleanup: Cleanup::Folder(folder.clone()),
        };
        coqui_acp_api::PrivateDir::new(folder).write(name, text.as_bytes(), true)?;
        Ok(placed)
    }
}

impl Drop for Placed {
    fn drop(&mut self) {
        match &self.cleanup {
            Cleanup::Account(files, name) => {
                let _ = files.remove(name);
            }
            Cleanup::Folder(folder) => {
                let _ = std::fs::remove_dir_all(folder);
            }
        }
    }
}

/// The `Imported session: <id>` line `opencode import` prints, and the id.
fn imported_id(output: &str) -> Option<&str> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("Imported session:"))
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

impl StdioProvider {
    /// A launch of the program with `args` on the account's data folder (or the environment's, with no account): the
    /// host's variables, less what is a credential of the harness when an account applies, and the account's folder, which is
    /// all of the account a command that only reads or writes sessions needs: the key is not passed.
    fn opencode_launch(
        &self,
        args: impl IntoIterator<Item = String>,
        environment: &Environment,
        applied: Option<&accounts::Applied>,
    ) -> Launch {
        let def = self.def();
        let mut launch = Launch::new(self.program_name()).args(args);
        for (name, value) in environment.iter() {
            let credential = def
                .variables
                .iter()
                .any(|variable| accounts::same_name(variable, name));
            if applied.is_none() || !credential {
                launch.env.insert(name.into(), value.into());
            }
        }
        if let Some(applied) = applied {
            for (name, value) in applied.environment.expose().iter() {
                if accounts::same_name(name, "XDG_DATA_HOME") {
                    launch.env.insert(name.into(), value.into());
                }
            }
            launch.remove_env = def.variables.iter().map(OsString::from).collect();
            launch
                .remove_env
                .retain(|name| !accounts::same_name(&name.to_string_lossy(), "XDG_DATA_HOME"));
        }
        launch
    }

    /// A session of OpenCode's as a history: `opencode export --pure <id>` on the account's data folder (or the
    /// environment's, with no account), through the spawner, then paged. The id is the one OpenCode made, or the UUID
    /// the host knows an imported session by.
    pub(super) fn export_opencode(
        &self,
        export: &SessionExport,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ExportPage, ProviderError> {
        let def = self.def();
        let native = if opencode_history::is_native_id(&export.session_id) {
            export.session_id.clone()
        } else if let Some(native) = opencode_history::native_id_of(&export.session_id) {
            native
        } else {
            return Err(error(
                Status::GUATIAO_ERR_BAD_VALUE,
                "the session id is not one OpenCode made (`ses_` and letters and digits) or a UUID",
            ));
        };
        let applied = match &export.account {
            Some(grant) => accounts::apply(def, grant)?,
            None => None,
        };
        let launch = self.opencode_launch(
            ["export".to_owned(), "--pure".to_owned(), native.clone()],
            export.environment.expose(),
            applied.as_ref(),
        );
        let done = cli::run(
            &ThroughSpawner::new(spawner),
            &launch,
            None,
            EXPORT_LIMIT,
            EXPORT_MAX_BYTES,
        )
        .map_err(|_| {
            error(
                Status::GUATIAO_ERR_INTERNAL,
                "the program could not be started to export the session",
            )
        })?;
        if done.timed_out {
            return Err(error(
                Status::GUATIAO_ERR_INTERNAL,
                "the program did not export the session in time",
            ));
        }
        if !done.succeeded() {
            let said = cli::plain(&String::from_utf8_lossy(&done.stderr)).to_lowercase();
            return Err(if said.contains("not found") {
                error(Status::GUATIAO_ERR_NOT_FOUND, "no session has this id")
            } else {
                error(
                    Status::GUATIAO_ERR_INTERNAL,
                    "the program did not export the session",
                )
            });
        }
        let output = String::from_utf8_lossy(&done.stdout);
        let host =
            opencode_history::host_id_of(&native).unwrap_or_else(|| export.session_id.clone());
        let history = opencode_history::export(&output, &host)
            .map_err(|why| error(Status::GUATIAO_ERR_BAD_VALUE, &why))?
            .to_string()
            .map_err(|why| error(Status::GUATIAO_ERR_BAD_VALUE, &why.to_string()))?;
        match page(&history, export.after.as_deref(), export.max_bytes) {
            Ok(page) => Ok(ExportPage::new(page.text, page.next)),
            Err(PageError::LineTooLarge(size)) => Err(error(
                Status::GUATIAO_ERR_BAD_VALUE,
                &alloc::format!(
                    "{LINE_TOO_LARGE}{size} bytes, more than max_bytes {}",
                    export.max_bytes
                ),
            )),
            Err(other) => Err(error(Status::GUATIAO_ERR_BAD_VALUE, &other.to_string())),
        }
    }

    /// The history of `session` as a document: an empty one for a session with none, redacted by the provider's patterns
    /// when it has any.
    ///
    /// # Errors
    ///
    /// `GUATIAO_ERR_BAD_VALUE` for an id that is not a UUID, a history this provider cannot read or a pattern that is no
    /// expression; `GUATIAO_ERR_UNSUPPORTED` for a version of the format it does not read.
    pub(super) fn opencode_history(
        &self,
        session: &SessionImport,
    ) -> Result<Document, ProviderError> {
        let bad = |why: &str| error(Status::GUATIAO_ERR_BAD_VALUE, why);
        let uuid = uuid::Uuid::try_parse(&session.session_id)
            .map_err(|_| bad("the session id is not a UUID"))?;
        let mut document = if session.history.trim().is_empty() {
            Document::new(Header::new(
                uuid.hyphenated().to_string(),
                Origin::new(opencode_history::HARNESS),
            ))
        } else {
            Document::from_slice(session.history.as_bytes()).map_err(|why| match why {
                HistoryError::UnknownVersion { .. } | HistoryError::UnknownFormat => error(
                    Status::GUATIAO_ERR_UNSUPPORTED,
                    "this provider does not read that version of the history format",
                ),
                _ => bad("the history is not one this provider can read"),
            })?
        };
        if !self.settings.replay_redact_patterns.is_empty() {
            let mut redaction = RedactionSettings::default();
            redaction
                .patterns
                .clone_from(&self.settings.replay_redact_patterns);
            let mut redactor = Redactor::try_new(&redaction).map_err(|_| {
                bad("a pattern to redact from a history is not a usable expression")
            })?;
            redactor.document(&mut document).map_err(|_| {
                bad("the history cannot be redacted into one this provider can write")
            })?;
        }
        Ok(document)
    }

    /// Writes `session`'s history into OpenCode's store as a session of its own: the file `opencode import` reads is made and
    /// the program is run on it in the session's working directory (the directory it gives the session), and the file is
    /// removed again. The store is the account's data folder when the account keeps one, whose private files the file is put
    /// in through the spawner; with no account, or one that keeps none (the machine's own sign-in), it is the one the
    /// program finds from the environment, and the file is made in a folder for this import alone, closed to others and
    /// removed with the file, which only a spawner that runs on this machine can use. The session is then `ses_` and the
    /// UUID's hex digits ([`opencode_history::native_id_of`]), which the relay turns back into the UUID for the host.
    ///
    /// # Errors
    ///
    /// `GUATIAO_ERR_UNSUPPORTED` with no private files in the account's folder to put the file in, or, with no folder of the
    /// account's, a spawner that runs programs elsewhere; `GUATIAO_ERR_BAD_VALUE` for an id that is not a UUID or a history
    /// this provider cannot read; `GUATIAO_ERR_INTERNAL` when the program does not take the file.
    pub(super) fn import_opencode(
        &self,
        session: &SessionImport,
        history: &Document,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<ImportReport, ProviderError> {
        let def = self.def();
        let account = match &session.account {
            Some(grant) => accounts::apply(def, grant)?.map(|applied| (grant, applied)),
            None => None,
        };
        let imported =
            opencode_import::import(history, &session.session_id, &session.working_directory)
                .map_err(|why| error(Status::GUATIAO_ERR_BAD_VALUE, &why))?;
        let spawner = Arc::new(spawner);
        let name = alloc::format!("imports/{}.json", imported.native_id);
        let placed = match &account {
            Some((grant, _)) => {
                let Some(folder) = grant.state_dir.as_deref() else {
                    return Err(error(
                        Status::GUATIAO_ERR_UNSUPPORTED,
                        "this account has no data folder of its own to write a session into",
                    ));
                };
                Placed::in_account(&spawner, folder, &name, &imported.text)?
            }
            None => Placed::in_own_folder(&spawner, &name, &imported.text)?,
        };
        let launch = self
            .opencode_launch(
                ["import".to_owned(), placed.path.clone()],
                session.environment.expose(),
                account.as_ref().map(|(_, applied)| applied),
            )
            .cwd(&session.working_directory);
        let done = cli::run(
            &ThroughSpawner::shared(Arc::clone(&spawner)),
            &launch,
            None,
            EXPORT_LIMIT,
            EXPORT_MAX_BYTES,
        );
        // The file holds the history: it goes whatever came of the import.
        drop(placed);
        let done = done.map_err(|_| {
            error(
                Status::GUATIAO_ERR_INTERNAL,
                "the program could not be started to import the session",
            )
        })?;
        let said = String::from_utf8_lossy(&done.stdout);
        if done.timed_out
            || !done.succeeded()
            || imported_id(&said) != Some(imported.native_id.as_str())
        {
            return Err(error(
                Status::GUATIAO_ERR_INTERNAL,
                "the program did not take the session's history",
            ));
        }
        let mut report = ImportReport::new();
        report.seeded_from = imported.seeded_from.map(|point| {
            let mut seed = SeedPoint::line(point.line);
            seed.digest = Some(point.digest);
            seed
        });
        report.losses = imported
            .report
            .losses()
            .iter()
            .map(|loss| {
                let mut out = HistoryLoss::new(loss.kind.clone(), loss.count);
                out.detail.clone_from(&loss.detail);
                out
            })
            .collect();
        Ok(report)
    }
}
