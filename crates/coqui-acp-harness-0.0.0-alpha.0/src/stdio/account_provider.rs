//! Grok's accounts, OpenCode's (and Antigravity's, which lists none) as a provider of the `account_provider` kind.
//!
//! It makes the accounts [`accounts`](super::accounts) lists for the harness: it signs a person in with the harness's own
//! command, renews a login with no model turn where the harness can, and watches a login's file for a change nobody told
//! the host of. It runs no agent: the harness ([`StdioProvider`](super::StdioProvider)) applies an account the host hands
//! it.

use alloc::string::{String, ToString};
use core::time::Duration;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Provider, Schema, Status, ToValue};
use coqui_acp_api::{
    AccountKeeper, AccountOffer, AccountProvider, AccountProviderInfo, AccountUsage,
    AccountUsageQuery, AccountWatch, LoginOutcome, LoginPrompter, LoginRequest, ProcessSpawner,
    RenewReport, RenewRequest, WatchRequest,
};

use super::StdioHarness;
use super::accounts::{self, Keeping};
use crate::account::DEFAULT_POLL_MS;

/// How the stdio account provider is configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct StdioAccountSettings {
    /// The harness: `grok`, `antigravity` or `opencode`.
    pub harness: StdioHarness,
    /// The program it signs in and renews with: a path, or a name looked up on the `PATH` where the spawner runs it. The
    /// harness's own (`grok`, `agy`, `opencode`) when absent.
    pub executable: Option<String>,
    /// How often, in milliseconds, a watch looks at the file an account keeps its login in; 0 looks only when the watch
    /// begins and ends, and when a call is made on the account.
    #[map(default = 30_000)]
    pub renew_poll_ms: u64,
    /// The longest, in milliseconds, a command of the harness's own (a renewal) may run; 0 ends it at once.
    #[map(default = 60_000)]
    pub command_timeout_ms: u64,
}

impl StdioAccountSettings {
    /// The settings for `harness`, every other value its default.
    #[must_use]
    pub const fn new(harness: StdioHarness) -> Self {
        Self {
            harness,
            executable: None,
            renew_poll_ms: DEFAULT_POLL_MS,
            command_timeout_ms: 60_000,
        }
    }
}

/// The accounts of Grok, Antigravity or OpenCode, as a provider of `account_provider` under the id `stdio_accounts`.
///
/// Register it beside a host's own providers with `Registry::register_local`, or build it into a library with
/// `guatiao::providers!`.
#[derive(Provider, Debug)]
#[provider(
    AccountProvider,
    id = "stdio_accounts",
    config = StdioAccountSettings,
    new = StdioAccounts::unconfigured,
    crate = ::coqui_acp_api::guatiao
)]
pub struct StdioAccounts {
    settings: StdioAccountSettings,
    keeping: Keeping,
}

impl From<StdioAccountSettings> for StdioAccounts {
    fn from(settings: StdioAccountSettings) -> Self {
        Self::new(settings)
    }
}

impl StdioAccounts {
    /// A provider configured as `settings` say.
    #[must_use]
    pub fn new(settings: StdioAccountSettings) -> Self {
        let keeping = Keeping::new(settings.harness.def());
        Self { settings, keeping }
    }

    fn unconfigured() -> Self {
        Self::new(StdioAccountSettings::new(StdioHarness::Grok))
    }

    fn program_name(&self) -> String {
        self.settings
            .executable
            .clone()
            .unwrap_or_else(|| self.settings.harness.def().program.to_string())
    }

    fn logins(&self) -> alloc::vec::Vec<coqui_acp_api::LoginMethodInfo> {
        match self.settings.harness {
            StdioHarness::Grok => accounts::grok::logins(),
            StdioHarness::Antigravity => alloc::vec::Vec::new(),
            StdioHarness::OpenCode => accounts::opencode::logins(),
        }
    }
}

impl AccountProvider for StdioAccounts {
    fn describe(&self) -> Result<AccountProviderInfo, ProviderError> {
        let name = self.settings.harness.def().name;
        let mut info = AccountProviderInfo::new(alloc::format!("{name} accounts"));
        info.description = match self.settings.harness {
            StdioHarness::Grok => {
                "Signs in to Grok, renews and watches its logins, and says its accounts report no plan usage.".into()
            }
            StdioHarness::Antigravity => {
                "Antigravity's sign-in is unmeasured, so it keeps no account yet.".into()
            }
            StdioHarness::OpenCode => {
                "Makes an OpenCode account from a provider's key, watches the sign-in OpenCode keeps in `auth.json`, and says its accounts report no plan usage.".into()
            }
        };
        Ok(info)
    }

    fn offers(&self) -> Result<AccountOffer, ProviderError> {
        Ok(self.keeping.offer(self.logins()))
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
            StdioHarness::OpenCode => {
                accounts::opencode::sign_in(self.program_name(), request, spawner, prompter)
            }
            StdioHarness::Antigravity => Err(ProviderError::new(
                Status::GUATIAO_ERR_UNSUPPORTED,
                self.settings.harness.def().login_unsupported,
            )),
        }
    }

    fn renew(
        &self,
        request: RenewRequest,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<RenewReport, ProviderError> {
        self.keeping.renew(
            request,
            spawner,
            &self.program_name(),
            Duration::from_millis(self.settings.command_timeout_ms),
        )
    }

    fn watch(
        &self,
        request: WatchRequest,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<Object<dyn AccountWatch>, ProviderError> {
        self.keeping.watch(
            request,
            spawner,
            keeper,
            Duration::from_millis(self.settings.renew_poll_ms),
        )
    }

    fn usage(
        &self,
        query: AccountUsageQuery,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<AccountUsage, ProviderError> {
        drop((spawner, keeper));
        self.keeping
            .usage(&query, "this harness reports no usage outside a turn")
    }
}
