//! A stdio harness's accounts as a provider of the `account_provider` kind.
//!
//! It makes the accounts [`accounts`](super::accounts) lists for the harness: it signs a person in with the harness's own
//! command, renews a login with no model turn where the harness can, and watches a login's file for a change nobody told
//! the host of. It runs no agent: the harness ([`StdioProvider`](super::StdioProvider)) applies an account the host hands
//! it.

use alloc::string::{String, ToString};
use core::time::Duration;

use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::guatiao::{FromValue, Schema, Status, ToValue};
use coqui_acp_api::{
    AccountKeeper, AccountModelsQuery, AccountOffer, AccountProvider, AccountProviderInfo,
    AccountRef, AccountUsage, AccountUsageQuery, AccountWatch, LoginOutcome, LoginPrompter,
    LoginRequest, ModelListing, ProcessSpawner, RenewReport, RenewRequest, SignedInCheck,
    SignedInQuery, WatchRequest,
};

use super::StdioDef;
use super::accounts::Keeping;
use crate::account::DEFAULT_POLL_MS;

/// How a stdio harness's account provider is configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone)]
#[map(crate = ::coqui_acp_api::guatiao)]
pub struct StdioAccountSettings {
    /// The program it signs in and renews with: a path, or a name looked up on the `PATH` where the spawner runs it. The
    /// harness's own when absent.
    pub executable: Option<String>,
    /// How often, in milliseconds, a watch looks at the file an account keeps its login in; 0 looks only when the watch
    /// begins and ends, and when a call is made on the account.
    #[map(default = 30_000)]
    pub renew_poll_ms: u64,
    /// The longest, in milliseconds, a command of the harness's own (a renewal) may run; 0 ends it at once.
    #[map(default = 60_000)]
    pub command_timeout_ms: u64,
    /// The longest, in milliseconds, a look at where the harness keeps its login (its keyring, for one that uses it) may take
    /// before the answer is unknown; 0 does not wait for it.
    #[map(default = 5_000)]
    pub signed_in_timeout_ms: u64,
}

impl StdioAccountSettings {
    /// The defaults.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            executable: None,
            renew_poll_ms: DEFAULT_POLL_MS,
            command_timeout_ms: 60_000,
            signed_in_timeout_ms: 5_000,
        }
    }
}

impl Default for StdioAccountSettings {
    fn default() -> Self {
        Self::new()
    }
}

/// The accounts of one harness, described by its [`StdioDef`], as a provider of `account_provider`.
///
/// It is no registered provider itself: a vendor's accounts provider holds one (see
/// [`stdio_vendor!`](crate::stdio_vendor)).
#[derive(Debug)]
pub struct StdioAccounts {
    def: &'static StdioDef,
    settings: StdioAccountSettings,
    keeping: Keeping,
}

impl StdioAccounts {
    /// The accounts provider of the harness `def` configured as `settings` say.
    #[must_use]
    pub fn new(def: &'static StdioDef, settings: StdioAccountSettings) -> Self {
        let keeping = Keeping::new(def.accounts);
        Self {
            def,
            settings,
            keeping,
        }
    }

    fn program_name(&self) -> String {
        self.settings
            .executable
            .clone()
            .unwrap_or_else(|| self.def.accounts.program.to_string())
    }
}

impl AccountProvider for StdioAccounts {
    fn describe(&self) -> Result<AccountProviderInfo, ProviderError> {
        let mut info =
            AccountProviderInfo::new(alloc::format!("{} accounts", self.def.accounts.name));
        info.description = self.def.accounts_description.to_string();
        Ok(info)
    }

    fn offers(&self) -> Result<AccountOffer, ProviderError> {
        Ok(self.keeping.offer((self.def.logins)()))
    }

    fn provides(&self, account: AccountRef) -> Result<Vec<String>, ProviderError> {
        self.keeping.provides(&account)
    }

    fn login(
        &self,
        request: LoginRequest,
        spawner: Object<dyn ProcessSpawner>,
        prompter: Object<dyn LoginPrompter>,
    ) -> Result<LoginOutcome, ProviderError> {
        match self.def.sign_in {
            Some(sign_in) => sign_in(self.program_name(), request, spawner, prompter),
            None => Err(ProviderError::new(
                Status::GUATIAO_ERR_UNSUPPORTED,
                self.def.accounts.login_unsupported,
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

    fn models(
        &self,
        query: AccountModelsQuery,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<ModelListing, ProviderError> {
        drop((spawner, keeper));
        self.keeping.models(
            &query,
            "the agent lists its models in its session, not before it",
        )
    }

    fn signed_in(&self, query: SignedInQuery) -> Result<SignedInCheck, ProviderError> {
        Ok((self.def.signed_in)(
            query.environment.expose(),
            Duration::from_millis(self.settings.signed_in_timeout_ms),
        ))
    }
}
