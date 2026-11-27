//! Codex's accounts as a provider of the `account_provider` kind.
//!
//! It makes the accounts Codex takes: it signs a person in with `codex login`, renews a login with no model turn,
//! watches a login's file for a change nobody told the host of, and reads the plan limits of an account. It runs no
//! agent: the harness ([`CodexProvider`](crate::CodexProvider)) applies an account the host hands it.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::time::Duration;

use std::ffi::OsString;
use std::sync::{Mutex, PoisonError};

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use coqui_acp_api::standard::CODEX;
use coqui_acp_api::{
    AccountKeeper, AccountModelsQuery, AccountOffer, AccountProvider, AccountProviderInfo,
    AccountRef, AccountUsage, AccountUsageQuery, AccountWatch, HARNESS_LOGIN, LoginOutcome,
    LoginPrompter, LoginRequest, ModelInfo, ModelListing, ProcessSpawner, RenewReport,
    RenewRequest, SignedInCheck, SignedInQuery, Source, UnknownKind, WatchRequest,
};
use coqui_acp_harness::ThroughSpawner;
use coqui_acp_harness::account::{Accounts, Files, Watching};
use coqui_acp_harness::provider::provides_of;

use crate::account_usage::{ReadOptions, read_limits};
use crate::stored_login::{self, Stored};
use crate::{
    CodexLaunch, CodexSession, CodexSettings, ModelRead, SessionSettings, accounts, signin,
};

/// The id the account provider is registered under, which the harness's description names as its link.
pub(crate) const ACCOUNT_PROVIDER: &str = "codex_accounts";

/// How the Codex account provider is configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct AccountSettings {
    /// The Codex program it signs in and renews with: a path, or a name looked up on the `PATH` where the spawner runs
    /// it.
    #[map(default = String::from("codex"))]
    pub executable: String,
    /// How often, in milliseconds, a watch looks at the file an account keeps its login in; 0 looks only when the watch
    /// begins and ends, and when a call is made on the account.
    #[map(default = 30_000)]
    pub renew_poll_ms: u64,
    /// The longest, in milliseconds, a command Codex is asked to run for an account (`login`) may take; 0 ends it at
    /// once.
    #[map(default = 60_000)]
    pub command_timeout_ms: u64,
    /// The address plan limits are read from, in place of the account's own `chatgpt_base_url` and ChatGPT's: the
    /// backend a login's token is sent to, and the model API an API key with no `openai_base_url` is read at. Absent,
    /// the account's, then OpenAI's.
    pub usage_endpoint: Option<String>,
    /// The longest, in milliseconds, a plan limits read may take in all.
    #[map(default = 10_000)]
    pub usage_timeout_ms: u64,
    /// The most, in bytes, of the login file (`auth.json`) that is read to answer the local account's usage with no
    /// process; a larger file is left to Codex.
    #[map(default = 1_048_576)]
    pub login_file_limit: u64,
    /// The longest, in milliseconds, the operating system's keyring may take to say whether Codex's login is there before the
    /// answer is unknown; 0 does not wait for it.
    #[map(default = 5_000)]
    pub signed_in_timeout_ms: u64,
    /// How the sessions that list an account's models, read its allowances and refresh its login read the app server's
    /// output, wait for it and end it.
    #[map(flatten)]
    pub session: SessionSettings,
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            executable: "codex".into(),
            renew_poll_ms: 30_000,
            command_timeout_ms: 60_000,
            usage_endpoint: None,
            usage_timeout_ms: 10_000,
            login_file_limit: 1 << 20,
            signed_in_timeout_ms: 5_000,
            session: SessionSettings::default(),
        }
    }
}

/// The accounts of Codex, as a provider of `account_provider` under the id `codex_accounts`.
///
/// Register it beside a host's own providers with `Registry::register_local`, or build it into a library with
/// `guatiao::providers!`.
#[derive(Provider, Debug)]
#[provider(
    AccountProvider,
    id = "codex_accounts",
    config = AccountSettings,
    new = CodexAccounts::unconfigured,
    crate = ::coqui_acp_api::guatiao
)]
pub struct CodexAccounts {
    settings: AccountSettings,
    /// What it knows of its accounts' files, and who hears of them.
    accounts: Arc<Accounts>,
    /// What each account's usage last read as, and the revision it was told at.
    reports: Mutex<BTreeMap<String, (AccountUsage, u64)>>,
    /// For each home a key or token was saved in, a fingerprint of it.
    saved: accounts::Saved,
}

impl From<AccountSettings> for CodexAccounts {
    fn from(settings: AccountSettings) -> Self {
        Self::new(settings)
    }
}

impl CodexAccounts {
    /// A provider configured as `settings` say.
    #[must_use]
    pub fn new(settings: AccountSettings) -> Self {
        Self {
            settings,
            accounts: Arc::new(Accounts::new()),
            reports: Mutex::new(BTreeMap::new()),
            saved: Mutex::new(BTreeMap::new()),
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

    /// What Codex is started with for a call: the program and the environment the call adds.
    fn launch(&self, environment: &coqui_acp_api::Environment) -> CodexLaunch {
        let mut launch = CodexLaunch::new(".");
        launch.executable = self.executable();
        for (name, value) in environment.iter() {
            launch.env.insert(name.into(), value.into());
        }
        launch
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
}

fn unsupported(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_UNSUPPORTED, why)
}

impl AccountProvider for CodexAccounts {
    fn describe(&self) -> Result<AccountProviderInfo, ProviderError> {
        let mut info = AccountProviderInfo::new("Codex accounts");
        info.description = "Signs in to Codex, renews and watches its ChatGPT logins, and reads the plan limits of its accounts.".into();
        Ok(info)
    }

    fn offers(&self) -> Result<AccountOffer, ProviderError> {
        Ok(accounts::offer())
    }

    fn provides(&self, account: AccountRef) -> Result<Vec<String>, ProviderError> {
        provides_of(&accounts::offer(), &account, &[CODEX])
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
            &request,
            spawner,
            self.settings.session.settings(),
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
        let Some(applied) = accounts::plan(grant)? else {
            return Err(unsupported("this account has no file to watch"));
        };
        let files = Files::new(Arc::new(spawner));
        if !files.available() {
            return Err(unsupported(
                "this spawner serves no private files, which the login is kept in",
            ));
        }
        let hold = self.accounts.observe(
            files,
            Some(keeper),
            &grant.id,
            &applied.dir,
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
            crate::signed_in::real_keyring(),
        ))
    }

    fn usage(
        &self,
        query: AccountUsageQuery,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<AccountUsage, ProviderError> {
        if query.account.kind == HARNESS_LOGIN {
            // The login Codex has where it runs: its `auth.json` says the allowances to ChatGPT's backend, and only a
            // login that file cannot answer for is read by Codex itself, with no turn.
            drop(keeper);
            query.account.check_id()?;
            let mut options =
                ReadOptions::new().timeout(Duration::from_millis(self.settings.usage_timeout_ms));
            if let Some(endpoint) = &self.settings.usage_endpoint {
                options = options.endpoint(endpoint.clone());
            }
            if let Stored::Read(usage) = stored_login::read(
                &query.account.id,
                query.environment.expose(),
                &options,
                self.settings.login_file_limit,
            ) {
                return Ok(self.revised(usage));
            }
            let launch = self.launch(query.environment.expose());
            let spawn = ThroughSpawner::shared(Arc::new(spawner));
            let mut usage = AccountUsage::new();
            usage.sources = vec![Source::Reported];
            usage.account = Some(query.account.id.clone());
            // A login with no allowances answers with an error, and nothing is reported.
            if let Ok(mut buckets) =
                CodexSession::read_account_usage(&launch, &spawn, self.settings.session.settings())
            {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|since| i64::try_from(since.as_secs()).ok())
                    .unwrap_or(0);
                for bucket in &mut buckets {
                    if bucket.observed_at == 0 {
                        bucket.observed_at = now;
                    }
                }
                usage.observed_at = buckets.iter().map(|b| b.observed_at).max().unwrap_or(0);
                usage.buckets = buckets;
            }
            return Ok(self.revised(usage));
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
        let mut launch = self.launch(query.environment.expose());
        let spawner = Arc::new(spawner);
        // The login's files are placed for the call and a login the program renews meanwhile is told to the keeper.
        let hold = if local {
            drop(keeper);
            None
        } else {
            let files = Files::new(Arc::clone(&spawner));
            let options = accounts::ApplyOptions::new().timeout(self.limit());
            accounts::apply_as(
                grant,
                &mut launch,
                &files,
                &options,
                false,
                Some(&self.saved),
            )?
            .hold_keeping(&self.accounts, &files, Some(keeper))?
        };
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

/// The models Codex lists once started on `launch`, with no thread.
fn listing(launch: &CodexLaunch, spawn: &ThroughSpawner, settings: CodexSettings) -> ModelListing {
    match CodexSession::read_models(launch, spawn, settings) {
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
    }
}
