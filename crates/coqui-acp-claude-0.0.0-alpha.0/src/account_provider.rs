//! Claude Code's accounts as a provider of the `account_provider` kind.
//!
//! It makes the accounts Claude Code takes: it signs a person in with Claude Code's own commands, renews a login with
//! no model turn, watches a login's file for a change nobody told the host of, and reads the plan limits of an account.
//! It runs no agent: the harness ([`ClaudeProvider`](crate::ClaudeProvider)) applies an account the host hands it.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use core::time::Duration;

use std::ffi::OsString;
use std::sync::{Mutex, PoisonError};

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use coqui_acp_api::{
    AccountKeeper, AccountOffer, AccountProvider, AccountProviderInfo, AccountUsage,
    AccountUsageQuery, AccountWatch, LoginOutcome, LoginPrompter, LoginRequest, ProcessSpawner,
    RenewReport, RenewRequest, WatchRequest,
};
use coqui_acp_harness::account::{Accounts, Files, Watching};

use crate::account_usage::{ReadOptions, read_limits};
use crate::{accounts, signin};

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
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            executable: "claude".into(),
            renew_poll_ms: 30_000,
            command_timeout_ms: 60_000,
            usage_endpoint: None,
            usage_timeout_ms: 10_000,
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

    fn usage(
        &self,
        query: AccountUsageQuery,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<AccountUsage, ProviderError> {
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
}
