//! The `account_provider` kind: a provider that owns accounts of its kinds. It signs a person in to
//! make one, renews its credentials, watches them for a change nobody told the host of, and reports
//! how much of the account's plan is used.
//!
//! An account provider is not an ACP harness. A harness **accepts** the standards it can start on
//! ([`AccountCatalog::accepts`]); an account provider **provides** them
//! ([`AccountKindInfo::provides`]). A host lists an account for a harness exactly when the two meet.

use alloc::string::String;
use alloc::vec::Vec;

use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, Secret, ToValue};

use crate::account::{
    AccountGrant, AccountKeeper, AccountKindInfo, LoginMethodInfo, LoginOutcome, LoginPrompter,
    LoginRequest, RenewReport, RenewRequest,
};
use crate::environment::Environment;
use crate::spawner::ProcessSpawner;
use crate::usage::{Bucket, Source, UsageReport};

/// Who an account provider is, from [`AccountProvider::describe`], as configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct AccountProviderInfo {
    /// Its name for people, such as `Claude accounts`.
    pub title: String,
    /// What it is and what it keeps, in a sentence or two for people.
    #[map(default)]
    pub description: String,
}

impl AccountProviderInfo {
    /// A provider named `title` for people.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            description: String::new(),
        }
    }
}

/// What an account provider offers, from [`AccountProvider::offers`], as configured.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct AccountOffer {
    /// The kinds of account it makes and keeps. Each says which standards an account of it
    /// provides, whether it rotates its login, keeps files, and renews with no model turn.
    #[map(default)]
    pub kinds: Vec<AccountKindInfo>,
    /// The ways to sign in. Each names the kind it produces, which is listed in `kinds`.
    #[map(default)]
    pub logins: Vec<LoginMethodInfo>,
    /// The kinds whose accounts report their plan's usage through [`AccountProvider::usage`]: ids
    /// listed in `kinds`. A kind not here answers `usage` with "not offered".
    #[map(default)]
    pub usage: Vec<String>,
}

impl AccountOffer {
    /// An offer of nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The kind with `id`.
    #[must_use]
    pub fn kind(&self, id: &str) -> Option<&AccountKindInfo> {
        self.kinds.iter().find(|kind| kind.id == id)
    }

    /// Whether accounts of kind `id` report usage.
    #[must_use]
    pub fn reports_usage(&self, id: &str) -> bool {
        self.usage.iter().any(|kind| kind == id)
    }
}

/// What a host asks [`AccountProvider::watch`]: refuses a key it does not declare.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct WatchRequest {
    /// The account to watch: its `state_dir` holds the files the provider reads, its `revision` is
    /// the version of the credentials the host holds.
    pub account: AccountGrant,
}

impl WatchRequest {
    /// A request to watch `account`.
    #[must_use]
    pub const fn new(account: AccountGrant) -> Self {
        Self { account }
    }
}

/// What a host asks [`AccountProvider::usage`]: refuses a key it does not declare.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct AccountUsageQuery {
    /// The account to report for; its `id` is echoed into the report and never interpreted.
    pub account: AccountGrant,
    /// Variables for what the provider runs or reaches to ask (a proxy, say), over its
    /// configuration's. A provider shows none of them in an answer, a refusal, an error or a log.
    #[map(default)]
    pub environment: Secret<Environment>,
}

impl AccountUsageQuery {
    /// A query for `account`, adding nothing to the provider's configuration.
    #[must_use]
    pub fn new(account: AccountGrant) -> Self {
        Self {
            account,
            environment: Secret::default(),
        }
    }

    /// The variables for what the provider runs.
    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = Secret::new(environment);
        self
    }
}

/// How much of an account's plan is used: its allowances, each with its window, as the account's
/// own provider reads them. **Never a session's or a run's tokens**: those come from ACP, as the
/// session's usage updates, and never cross this kind.
///
/// `supported: false` is a real answer: this kind of account reports no plan usage. `supported`
/// with no buckets is a different one, nothing reported yet.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct AccountUsage {
    /// Whether the account's provider reports usage for it at all.
    pub supported: bool,
    /// Why not, when `supported` is false.
    pub unsupported_because: Option<String>,
    /// Which kinds of figure it can give at all.
    #[map(default)]
    pub sources: Vec<Source>,
    /// The host's id for the account, echoed and never interpreted.
    pub account: Option<String>,
    /// Grows by one with each change the provider instance sees of the account, so a reader that
    /// polls keeps the newest and an unchanged account costs nothing.
    pub revision: u64,
    /// The newest bucket's `observed_at`, in Unix seconds; `0` with no bucket.
    pub observed_at: i64,
    /// The allowances, each by its id: its window (`refill`, with `resets_at`), what is used or
    /// left, the models it covers (`models`: all, or only these) and its status.
    #[map(default)]
    pub buckets: Vec<Bucket>,
}

impl AccountUsage {
    /// A supported account's report with nothing reported yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            supported: true,
            unsupported_because: None,
            sources: Vec::new(),
            account: None,
            revision: 0,
            observed_at: 0,
            buckets: Vec::new(),
        }
    }

    /// The answer of an account whose provider reports no usage for it, and why.
    #[must_use]
    pub fn unsupported(reason: impl Into<String>) -> Self {
        Self {
            supported: false,
            unsupported_because: Some(reason.into()),
            ..Self::new()
        }
    }

    /// How much of `bucket` is used, in thousandths and at most 1000, as
    /// [`UsageReport::fraction_permille`] reads it.
    #[must_use]
    pub fn fraction_permille(bucket: &Bucket) -> Option<u16> {
        UsageReport::fraction_permille(bucket)
    }
}

impl Default for AccountUsage {
    fn default() -> Self {
        Self::new()
    }
}

/// Owns accounts: the kinds it makes, the sign-in that makes one, the renewal and watching of its
/// credentials, and the usage of its plan.
///
/// An account provider does not run an agent. A harness (`acp_harness`) applies an account the
/// host hands it when the account's `provides` meets what the harness accepts, and never asks how
/// the account was made. The host owns which accounts exist, their secrets at rest and who holds
/// one; the provider owns every fact about how its kinds sign in, renew, rotate and report.
///
/// Every method answers for the provider as configured: a host builds an instance from the
/// provider's settings and asks it. Every method is synchronous; a host calls them from a thread
/// of its own. Every object a method is handed (`spawner`, `prompter`, `keeper`) is the provider's
/// from then on, and it destroys each, even when it refuses.
///
/// A credential crosses only as a secret, and nothing a provider answers carries one: not an
/// answer, a refusal, an error, a log line or an argument vector.
#[guatiao::kind(name = "account_provider")]
pub trait AccountProvider: Send + Sync {
    /// Who the provider is, for a host listing its providers. Cheap, and the same on every call.
    fn describe(&self) -> Result<AccountProviderInfo, ProviderError>;

    /// The kinds of account it makes, the ways to sign in, and which kinds report usage. Cheap,
    /// the same on every call, and answered as configured.
    fn offers(&self) -> Result<AccountOffer, ProviderError>;

    /// Signs in to make an account of the kind the method `request.method` produces. The provider
    /// runs the sign-in in `request.state_dir` (a program on a terminal where it needs one)
    /// through `spawner`, reads what it prints and relays each step through `prompter`, and at the
    /// end reads the files it wrote into the outcome's credentials. `spawner` and `prompter` are
    /// handed over: the provider destroys both before it answers, a timeout excepted (see below).
    ///
    /// The sign-in is one call that ends in the outcome, a refusal or a cancel. Its steps reach the
    /// person through `prompter`: `say` and `open_url` show them what the sign-in prints, `ask`
    /// takes back what they type or paste, and `cancelled` is how the person ends it.
    ///
    /// When the person cancels (`prompter.cancelled()` true, or a prompter call answering
    /// `GUATIAO_ERR_GONE`), or `request.timeout_ms` passes, the provider kills the program's whole
    /// tree and answers `GUATIAO_ERR_GONE` with `login cancelled` or `login timed out` within two
    /// seconds. It runs a watchdog of its own: at the deadline it kills the tree and answers even
    /// if a prompter call has not returned (the call is then left to end by itself, and the
    /// prompter destroyed when it does), so a host's `ask` should return by `timeout_ms`. It also
    /// checks the deadline after every prompter call and before it answers an outcome. A key the
    /// program refused is `GUATIAO_ERR_BAD_VALUE` with `login refused: ` and its reason; any other
    /// failure `GUATIAO_ERR_INTERNAL` with a reason that holds no input value and no URL query. A
    /// method the provider does not list is `GUATIAO_ERR_BAD_VALUE`; a provider with no login
    /// method answers `GUATIAO_ERR_UNSUPPORTED` with the reason.
    fn login(
        &self,
        request: LoginRequest,
        spawner: Object<dyn ProcessSpawner>,
        prompter: Object<dyn LoginPrompter>,
    ) -> Result<LoginOutcome, ProviderError>;

    /// Renews the credentials of `request.account` with no model turn. Answers the new
    /// credentials, or why nothing changed. `GUATIAO_ERR_UNSUPPORTED`, with the reason, for a
    /// kind whose entry says `renews` false; `GUATIAO_ERR_BAD_VALUE` for a kind it does not list,
    /// an id that is not 1 to 128 bytes without a control character, or a credential that is
    /// missing. `spawner` is handed over as `login`'s is. A refused renewal that lost the login is
    /// `GUATIAO_ERR_GONE` with the reason, and the blank copy is never answered.
    fn renew(
        &self,
        request: RenewRequest,
        spawner: Object<dyn ProcessSpawner>,
    ) -> Result<RenewReport, ProviderError>;

    /// Starts watching the account `request.account` names for a credential its program renewed or
    /// lost behind the host's back: the files in its `state_dir`, read through `spawner`. The
    /// watch tells `keeper` of each change, from threads of its own, and ends when the returned
    /// object is stopped or destroyed.
    ///
    /// A changed credential is `renewed` with the new credentials and the `revision` of the
    /// grant it replaces; a blank or deleted one is `lost`, with a reason that holds no
    /// credential. Each change is told once, with a `sequence` that grows with every event of the
    /// account (renewals and losses together) and that every live watch of the same account in the
    /// same provider instance hears, so a host deduplicates by (account, sequence) and may hand a
    /// keeper that keeps nothing. A watch reports what it finds when it begins, so a login that is
    /// already gone is told at once.
    ///
    /// Refuses `GUATIAO_ERR_UNSUPPORTED`, with the reason, a kind whose entry does not say both
    /// `rotates` and `state_dir`, or a spawner without files; `GUATIAO_ERR_BAD_VALUE` for a kind
    /// it does not list, an id that is not 1 to 128 bytes without a control character, or a
    /// credential that is missing. `spawner` and `keeper` are handed over to the watch: it
    /// destroys both when it is stopped, and at once when it refuses.
    fn watch(
        &self,
        request: WatchRequest,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<Object<dyn AccountWatch>, ProviderError>;

    /// What the plan behind `query.account` has used and has left, with no session and no model
    /// turn: the account's windows, how much of each is used, when each resets and which models it
    /// covers. Cheap, so a host may poll it as often as it likes. A kind the provider lists in
    /// `offers().usage` answers `supported`; any other kind it lists answers "not offered",
    /// [`AccountUsage::unsupported`], which is a true answer. A kind it does not
    /// list is `GUATIAO_ERR_BAD_VALUE`, as is an id that is not 1 to 128 bytes without a control
    /// character.
    ///
    /// **Plan usage only.** The tokens a session or a run spent are not an account's: ACP reports
    /// them with the session, and no field of the answer holds one. Never carries a value of the
    /// environment or the credentials. A provider that runs a program to ask runs it through
    /// `spawner`, on the account's directory, and destroys it; `keeper` is handed over as
    /// `watch`'s is, for a login the call sees renewed or lost, and is destroyed before the call
    /// answers.
    fn usage(
        &self,
        query: AccountUsageQuery,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<AccountUsage, ProviderError>;
}

/// A watch of one account, from [`AccountProvider::watch`]. Destroying it stops it, as
/// [`stop`](Self::stop) does.
#[guatiao::kind(object, name = "account_watch")]
pub trait AccountWatch: Send + Sync {
    /// Ends the watch: no keeper call starts after it answers, one under way has finished, and the
    /// spawner and the keeper it was handed are destroyed. Stopping it twice is no error.
    fn stop(&self) -> Result<(), ProviderError>;
}
