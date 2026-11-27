//! Claude Code's accounts as a provider of the `account_provider` kind.
//!
//! It makes the accounts Claude Code takes: it signs a person in with Claude Code's own commands, renews a login with
//! no model turn, watches a login's file for a change nobody told the host of, and reads the plan limits of an account.
//! It runs no agent: the harness ([`ClaudeProvider`](crate::ClaudeProvider)) applies an account the host hands it.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::time::Duration;

use std::ffi::OsString;
use std::sync::{Mutex, PoisonError};

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use coqui_acp_api::standard::CLAUDE_CODE;
use coqui_acp_api::{
    AccountGrant, AccountKeeper, AccountModelsQuery, AccountOffer, AccountProvider,
    AccountProviderInfo, AccountRef, AccountUsage, AccountUsageQuery, AccountWatch, Credentials,
    HARNESS_LOGIN, LoginOutcome, LoginPrompter, LoginRequest, ModelInfo, ModelListing,
    ProcessSpawner, RenewReport, RenewRequest, SignedInCheck, SignedInQuery, UnknownKind,
    WatchRequest,
};
use coqui_acp_harness::ThroughSpawner;
use coqui_acp_harness::account::{Accounts, Files, Watching};
use coqui_acp_harness::provider::provides_of;

use crate::account_usage::{ReadOptions, read_limits};
use crate::{ClaudeLaunch, ClaudeSession, ClaudeSettings, SessionSettings, accounts, signin};

/// The id the account provider is registered under, which the harness's description names as its link.
pub(crate) const ACCOUNT_PROVIDER: &str = "claude_accounts";

/// How the Claude Code account provider is configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct AccountSettings {
    /// The Claude Code program it signs in and renews with: a path, or a name looked up on the `PATH` where the
    /// spawner runs it.
    #[map(default = String::from("claude"))]
    pub executable: String,
    /// How often, in milliseconds, a watch looks at the file an account keeps its login in; 0 looks only when the watch
    /// begins and ends, and when a call is made on the account.
    #[map(default = 30_000)]
    pub renew_poll_ms: u64,
    /// The longest, in milliseconds, a command Claude Code is asked to run for an account (`auth status`) may take; 0
    /// ends it at once.
    #[map(default = 60_000)]
    pub command_timeout_ms: u64,
    /// The address plan limits are read from, in place of `https://api.anthropic.com`: the OAuth API a subscription
    /// login's token is sent to, and the model API an API key or gateway token with no `base_url` of its own is read
    /// at. Absent, Anthropic's.
    pub usage_endpoint: Option<String>,
    /// The longest, in milliseconds, a plan limits read may take in all.
    #[map(default = 10_000)]
    pub usage_timeout_ms: u64,
    /// The longest, in milliseconds, the macOS Keychain may take to say whether Claude Code's login item is there before the
    /// answer is unknown; 0 does not wait for it. No other system asks a keyring.
    #[map(default = 5_000)]
    pub signed_in_timeout_ms: u64,
    /// How the session that lists an account's models reads Claude Code's output, waits for it and ends it.
    #[map(flatten)]
    pub session: SessionSettings,
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            executable: "claude".into(),
            renew_poll_ms: 30_000,
            command_timeout_ms: 60_000,
            usage_endpoint: None,
            usage_timeout_ms: 10_000,
            signed_in_timeout_ms: 5_000,
            session: SessionSettings::default(),
        }
    }
}

/// The accounts of Claude Code, as a provider of `account_provider` under the id `claude_accounts`.
///
/// Register it beside a host's own providers with `Registry::register_local`, or build it into a library with
/// `guatiao::providers!`.
#[derive(Provider, Debug)]
#[provider(
    AccountProvider,
    id = "claude_accounts",
    config = AccountSettings,
    new = ClaudeAccounts::unconfigured,
    crate = ::coqui_acp_api::guatiao
)]
pub struct ClaudeAccounts {
    settings: AccountSettings,
    /// What it knows of its accounts' files, and who hears of them.
    accounts: Arc<Accounts>,
    /// What each account's usage last read as, and the revision it was told at.
    reports: Mutex<BTreeMap<String, (AccountUsage, u64)>>,
}

impl From<AccountSettings> for ClaudeAccounts {
    fn from(settings: AccountSettings) -> Self {
        Self::new(settings)
    }
}

impl ClaudeAccounts {
    /// A provider configured as `settings` say.
    #[must_use]
    pub fn new(settings: AccountSettings) -> Self {
        Self {
            settings,
            accounts: Arc::new(Accounts::new()),
            reports: Mutex::new(BTreeMap::new()),
        }
    }

    fn unconfigured() -> Self {
        Self::new(AccountSettings::default())
    }

    fn executable(&self) -> OsString {
        OsString::from(&self.settings.executable)
    }

    fn limit(&self) -> Duration {
        Duration::from_millis(self.settings.command_timeout_ms)
    }

    /// `usage` with the revision it is told at: one more than the last, when what it reads has changed.
    fn revised(&self, mut usage: AccountUsage) -> AccountUsage {
        let Some(id) = usage.account.clone() else {
            return usage;
        };
        usage.revision = 0;
        let mut reports = self.reports.lock().unwrap_or_else(PoisonError::into_inner);
        let revision = match reports.get(&id) {
            Some((last, revision)) if *last == usage => *revision,
            Some((_, revision)) => revision + 1,
            None => 1,
        };
        reports.insert(id, (usage.clone(), revision));
        usage.revision = revision;
        usage
    }

    /// The plan limits of the login Claude Code keeps for itself: the one in its configuration directory
    /// (`CLAUDE_CONFIG_DIR`, else `.claude` in the home), read as an account of the kind [`LOGIN`] is. A login that
    /// has expired is renewed by Claude Code (`auth status`, as the host's own renewal does) and read again once.
    ///
    /// The directory is on this machine's files, so a spawner that runs elsewhere has none to read. The credential is
    /// held only for the read and never shown.
    fn local_usage(
        &self,
        query: &AccountUsageQuery,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<AccountUsage, ProviderError> {
        let none = |why: &str| {
            let mut none = AccountUsage::unsupported(why);
            none.account = Some(query.account.id.clone());
            Ok(none)
        };
        let spawner = Arc::new(spawner);
        if !spawner.describe()?.local {
            return none(
                "Claude Code runs where this machine's files are not, so the login it keeps there cannot be read from here",
            );
        }
        let set: Vec<(String, String)> = query
            .environment
            .expose()
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        let variables: BTreeMap<OsString, OsString> = set
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect();
        let Some(dir) = crate::config_dir(&variables, false) else {
            return none("Claude Code's configuration directory is unknown");
        };
        let mut options =
            ReadOptions::new().timeout(Duration::from_millis(self.settings.usage_timeout_ms));
        if let Some(endpoint) = &self.settings.usage_endpoint {
            options = options.endpoint(endpoint.clone());
        }
        let mut renewed = false;
        loop {
            let Some(text) = login_file(&dir.join(accounts::CREDENTIALS_FILE)) else {
                return none(
                    "Claude Code keeps no login file in its configuration directory (nobody is signed in, or it keeps the login elsewhere)",
                );
            };
            // The kind needs a directory to be read as an account; reading it opens nothing there.
            let grant = AccountGrant::new(query.account.id.clone(), accounts::LOGIN)
                .credentials(
                    [(accounts::FILE_KEY, text.as_str())]
                        .into_iter()
                        .collect::<Credentials>(),
                )
                .state_dir(dir.to_string_lossy());
            match read_limits(&grant, &options) {
                Ok(report) => return Ok(self.revised(AccountUsage::from_report(report))),
                Err(error) if error.status == Status::GUATIAO_ERR_GONE && !renewed => {
                    renewed = true;
                    let spawn = ThroughSpawner::shared(Arc::clone(&spawner));
                    signin::status(&spawn, &self.executable(), &set, self.limit(), None)?;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

fn unsupported(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, why)
}

impl AccountProvider for ClaudeAccounts {
    fn describe(&self) -> Result<AccountProviderInfo, ProviderError> {
        let mut info = AccountProviderInfo::new("Claude accounts");
        info.description = "Signs in to Claude Code, renews and watches its logins, and reads the plan limits of its accounts.".into();
        Ok(info)
    }

    fn offers(&self) -> Result<AccountOffer, ProviderError> {
        Ok(accounts::offer())
    }

    fn provides(&self, account: AccountRef) -> Result<Vec<String>, ProviderError> {
        provides_of(&accounts::offer(), &account, &[CLAUDE_CODE])
    }

    fn login(
        &self,
        request: LoginRequest,
        spawner: Object<dyn ProcessSpawner>,
        prompter: Object<dyn LoginPrompter>,
    ) -> Result<LoginOutcome, ProviderError> {
        signin::login(
            &self.executable(),
            self.limit(),
            &request,
            spawner,
            prompter,
        )
    }

    fn renew(
        &self,
        request: RenewRequest,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<RenewReport, ProviderError> {
        signin::renew(
            &self.accounts,
            &self.executable(),
            self.limit(),
            &request,
            spawner,
        )
    }

    fn watch(
        &self,
        request: WatchRequest,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<Object<dyn AccountWatch>, ProviderError> {
        let grant = &request.account;
        accounts::own(grant)?;
        if !accounts::offer()
            .kind(&grant.kind)
            .is_some_and(|kind| kind.rotates && kind.state_dir)
        {
            return Err(unsupported(
                "this kind of account keeps its credential in no file that a program renews, so there is nothing to watch",
            ));
        }
        let applied = accounts::plan(grant)?;
        let files = Files::new(Arc::new(spawner));
        if !files.available() {
            return Err(unsupported(
                "this spawner serves no private files, which the login is kept in",
            ));
        }
        let dir = applied.dir.clone().unwrap_or_default();
        let hold = self.accounts.observe(
            files,
            Some(keeper),
            &grant.id,
            &dir,
            grant.revision,
            grant.credentials.expose(),
            applied.watched.clone(),
        )?;
        Ok(Watching::begin(
            hold,
            Duration::from_millis(self.settings.renew_poll_ms),
        ))
    }

    fn signed_in(&self, query: SignedInQuery) -> Result<SignedInCheck, ProviderError> {
        Ok(crate::signed_in::check(
            query.environment.expose(),
            Duration::from_millis(self.settings.signed_in_timeout_ms),
            crate::signed_in::real_keychain(),
        ))
    }

    fn usage(
        &self,
        query: AccountUsageQuery,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<AccountUsage, ProviderError> {
        if query.account.kind == HARNESS_LOGIN {
            drop(keeper);
            query.account.check_id()?;
            return self.local_usage(&query, spawner);
        }
        // Plan limits are read over the network with the credential: no program is started and no file read.
        drop((spawner, keeper));
        accounts::own(&query.account)?;
        let mut options =
            ReadOptions::new().timeout(Duration::from_millis(self.settings.usage_timeout_ms));
        if let Some(endpoint) = &self.settings.usage_endpoint {
            options = options.endpoint(endpoint.clone());
        }
        let report = read_limits(&query.account, &options)?;
        Ok(self.revised(AccountUsage::from_report(report)))
    }

    fn models(
        &self,
        query: AccountModelsQuery,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<ModelListing, ProviderError> {
        let grant = &query.account;
        let local = grant.kind == HARNESS_LOGIN;
        if local {
            grant.check_id()?;
        } else {
            accounts::own(grant)?;
        }
        let mut launch = ClaudeLaunch::new(".");
        launch.executable = self.executable();
        for (name, value) in query.environment.expose().iter() {
            launch.env.insert(name.into(), value.into());
        }
        let spawner = Arc::new(spawner);
        let files = Files::new(Arc::clone(&spawner));
        // The login's files are placed for the call and a login the program renews meanwhile is told to the keeper.
        let hold = if local {
            drop(keeper);
            None
        } else {
            accounts::apply_as(grant, &mut launch, &files, false)?.hold_keeping(
                &self.accounts,
                &files,
                Some(keeper),
            )?
        };
        drop(files);
        let listing = listing(
            &launch,
            &ThroughSpawner::shared(spawner),
            self.settings.session.settings(),
        );
        if let Some(hold) = hold {
            hold.finish();
        }
        Ok(listing)
    }
}

/// The models Claude Code lists once started on `launch`, with no turn.
fn listing(
    launch: &ClaudeLaunch,
    spawn: &ThroughSpawner,
    settings: ClaudeSettings,
) -> ModelListing {
    match ClaudeSession::start(launch, spawn, settings, |_| true) {
        Ok(session) => {
            let signed_out = session.signed_out();
            let listing = session.models().map_or_else(
                || {
                    // A Claude Code with nobody signed in can list none; that is the person's next step, not a
                    // list that waits for a session.
                    if signed_out {
                        ModelListing::unknown(
                            "nobody is signed in to Claude Code, which lists its models once someone is",
                        )
                        .signed_out()
                    } else {
                        ModelListing::unavailable(UnknownKind::Failed, "Claude Code listed no model")
                    }
                },
                |models| {
                    ModelListing::of(
                        models
                            .into_iter()
                            .map(|model| {
                                let mut info = ModelInfo::new(model.id.clone());
                                info.name = Some(model.label());
                                info.description = model.description;
                                info.resolved = model.resolved;
                                info.version = model.version;
                                info.efforts = model.efforts;
                                info.is_default = model.id == "default";
                                info
                            })
                            .collect(),
                    )
                },
            );
            session.close();
            if signed_out && listing.known {
                listing.signed_out()
            } else {
                listing
            }
        }
        Err(error) => ModelListing::unavailable(
            match &error {
                crate::Error::Spawn(why) if why.kind() == std::io::ErrorKind::NotFound => {
                    UnknownKind::NotInstalled
                }
                _ => UnknownKind::Failed,
            },
            format!("Claude Code could not be asked: {error}"),
        ),
    }
}

/// The longest login file read: a real one is a few hundred bytes.
const LOGIN_FILE_LIMIT: u64 = 1 << 20;

/// The text of the login file at `path`; `None` when it is not there or is not readable text of a sane length.
fn login_file(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(LOGIN_FILE_LIMIT + 1)
        .read_to_string(&mut text)
        .ok()?;
    (!text.is_empty() && text.len() as u64 <= LOGIN_FILE_LIMIT).then_some(text)
}
