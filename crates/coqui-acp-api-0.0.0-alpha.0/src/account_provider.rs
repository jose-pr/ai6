//! The `account_provider` kind: a provider that owns accounts of its kinds. It signs a person in to
//! make one, renews its credentials, watches them for a change nobody told the host of, reports
//! how much of the account's plan is used, and lists the models the account may use.
//!
//! An account provider is not an ACP harness. A harness **accepts** the standards it can start on
//! ([`AccountCatalog::accepts`]); an account provider **provides** them
//! ([`AccountKindInfo::provides`]). A host lists an account for a harness exactly when the two meet.

use alloc::string::String;
use alloc::vec::Vec;

use guatiao::library::{Object, ProviderError};
use guatiao::{FromValue, Schema, Secret, ToValue};

use crate::account::{
    AccountGrant, AccountKeeper, AccountKindInfo, AccountRef, LoginMethodInfo, LoginOutcome,
    LoginPrompter, LoginRequest, RenewReport, RenewRequest,
};
use crate::environment::Environment;
use crate::models::ModelListing;
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
    /// listed in `kinds`, or [`HARNESS_LOGIN`](crate::HARNESS_LOGIN) when `local` is true. A kind
    /// not here answers `usage` with "not offered".
    #[map(default)]
    pub usage: Vec<String>,
    /// Whether the provider's program has a login of its own that the provider offers as an
    /// automatic account named [`LOCAL_ACCOUNT`](crate::LOCAL_ACCOUNT), of kind
    /// [`HARNESS_LOGIN`](crate::HARNESS_LOGIN): the program run on the configuration as it stands,
    /// nothing applied. A host lists it without anyone adding it, and `usage` and `models` answer
    /// for a grant of that kind. A provider that offers none refuses such a grant as a kind it
    /// does not list.
    #[map(default)]
    pub local: bool,
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

    /// The offer of a provider whose program has a login of its own: this offer, with the automatic
    /// `local` account.
    #[must_use]
    pub const fn with_local(mut self) -> Self {
        self.local = true;
        self
    }

    /// Whether `id` is a kind a grant may carry to `usage` and `models`: a listed kind, or the
    /// login of the program itself when the provider offers it.
    #[must_use]
    pub fn takes(&self, id: &str) -> bool {
        self.kind(id).is_some() || (self.local && id == crate::HARNESS_LOGIN)
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

/// What a host asks [`AccountProvider::models`]: refuses a key it does not declare.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct AccountModelsQuery {
    /// The account to list for: one of the kinds the provider lists, or, when it offers the
    /// automatic `local` account, [`HARNESS_LOGIN`](crate::HARNESS_LOGIN), for which nothing is applied.
    pub account: AccountGrant,
    /// Variables for what the provider runs to ask, over its configuration's. A provider shows
    /// none of them in an answer, a refusal, an error or a log.
    #[map(default)]
    pub environment: Secret<Environment>,
}

impl AccountModelsQuery {
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

/// What a host asks [`AccountProvider::signed_in`]: refuses a key it does not declare.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct SignedInQuery {
    /// The variables the program's own login is read with, over this process's: the ones that place
    /// its home (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `HOME`) and the ones that sign it in by
    /// themselves (an API key). A provider shows none of them in an answer, a refusal, an error or
    /// a log.
    #[map(default)]
    pub environment: Secret<Environment>,
}

impl SignedInQuery {
    /// A query that adds nothing to this process's environment.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The variables the login is read with.
    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = Secret::new(environment);
        self
    }
}

/// What [`AccountProvider::signed_in`] found.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Copy, PartialEq, Eq)]
#[map(crate = ::guatiao, rename_all = "snake_case")]
#[non_exhaustive]
pub enum SignedInState {
    /// A login is there: a file with something in it, a keyring entry, or a variable that signs the
    /// program in. It may be expired or revoked; only the service knows.
    SignedIn,
    /// Every place the program keeps a login was looked at, and none holds one.
    SignedOut,
    /// The provider could not tell: a place it cannot look at here, or one that did not answer in
    /// time. The reason says which.
    Unknown,
}

/// The answer of [`AccountProvider::signed_in`]: whether the program's own login exists, and never the login.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct SignedInCheck {
    /// What was found.
    pub state: SignedInState,
    /// Why, for people: what was looked at when `state` is not `signed_in`. Holds no value of the
    /// environment and nothing read from a login.
    pub reason: Option<String>,
    /// The name, never the value, of the environment variable that signs the program in, when that
    /// is what was found.
    pub variable: Option<String>,
}

impl SignedInCheck {
    /// A login is there.
    #[must_use]
    pub const fn signed_in() -> Self {
        Self {
            state: SignedInState::SignedIn,
            reason: None,
            variable: None,
        }
    }

    /// A login is there because the variable `name` is set and not empty.
    #[must_use]
    pub fn by_variable(name: impl Into<String>) -> Self {
        Self {
            variable: Some(name.into()),
            ..Self::signed_in()
        }
    }

    /// No login is anywhere the program keeps one, with what was looked at.
    #[must_use]
    pub fn signed_out(reason: impl Into<String>) -> Self {
        Self {
            state: SignedInState::SignedOut,
            reason: Some(reason.into()),
            variable: None,
        }
    }

    /// The provider could not tell, and why.
    #[must_use]
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            state: SignedInState::Unknown,
            reason: Some(reason.into()),
            variable: None,
        }
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

    /// The plan usage a [`UsageReport`] holds: its allowances and windows. What a report keeps of a session (`spend`) or
    /// of a refusal a run met (`limit`) is no account's, and is left out.
    #[must_use]
    pub fn from_report(report: UsageReport) -> Self {
        Self {
            supported: report.supported,
            unsupported_because: report.unsupported_because,
            sources: report.sources,
            account: report.account,
            revision: report.revision,
            observed_at: report.observed_at,
            buckets: report.buckets,
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
/// credentials, the usage of its plan and the models it may use.
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

    /// The standards the account `account` provides: every one it can be used as, the ids a harness
    /// matches against what it accepts. An account names only its kind and settings; this is where
    /// they are read. A kind that lists `provides` answers that list, and a kind whose settings pick
    /// the standards ([`PROVIDES_BY_VALUE`](crate::PROVIDES_BY_VALUE) is the usual way)
    /// answers what the account's settings pick. The answer is the account's, not the credentials':
    /// it reads `account.settings` and no secret, and `account.provides` is no input. Each id is a
    /// [`standard`](crate::standard) id, none twice. May be empty for an account that cannot be
    /// used yet.
    ///
    /// Takes every kind the provider lists in `offers().kinds`, and, when `offers().local`, the
    /// automatic `local` account ([`HARNESS_LOGIN`](crate::HARNESS_LOGIN)), which provides the
    /// standard of the program itself. Any other kind is `GUATIAO_ERR_BAD_VALUE`, as is an id that
    /// is not 1 to 128 bytes without a control character. Cheap, the same on every call for the
    /// same account, and it starts nothing.
    fn provides(&self, account: AccountRef) -> Result<Vec<String>, ProviderError>;

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
    /// [`AccountUsage::unsupported`], which is a true answer. The automatic `local` account (kind
    /// [`HARNESS_LOGIN`](crate::HARNESS_LOGIN), when `offers().local`) is asked of the program on
    /// the configuration as it stands. A kind it neither lists nor offers as `local` is
    /// `GUATIAO_ERR_BAD_VALUE`, as is an id that is not 1 to 128 bytes without a control
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

    /// The models the account behind `query.account` may use, with no model turn and no session:
    /// what a host shows a person choosing one before a session starts. The listing is the
    /// account's, so an account of another kind (another login, another key) may list others. A
    /// provider that cannot list for a kind (the program lists its models only in a session, or
    /// has no way to) answers [`ModelListing::unknown`] with the reason, which is a true answer,
    /// and one that tried and could not answers [`ModelListing::unavailable`]; it never guesses.
    ///
    /// Takes every kind the provider lists in `offers().kinds`, and [`HARNESS_LOGIN`](crate::HARNESS_LOGIN)
    /// (the automatic `local` account) when `offers().local`: the program asked as its
    /// configuration stands, with nothing applied. Any other kind is `GUATIAO_ERR_BAD_VALUE`, as
    /// is an id that is not 1 to 128 bytes without a control character. Never carries a value of
    /// the environment or the credentials.
    ///
    /// A provider that runs a program to ask runs it through `spawner`, on the account's directory,
    /// and destroys it; `keeper` is handed over as `watch`'s is, for a login the call sees renewed
    /// or lost, and is destroyed before the call answers.
    fn models(
        &self,
        query: AccountModelsQuery,
        spawner: Object<dyn ProcessSpawner>,
        keeper: Object<dyn AccountKeeper>,
    ) -> Result<ModelListing, ProviderError>;

    /// Whether the program's own login (the automatic `local` account) exists, asked cheaply enough
    /// for a host to list the account only when it does, with no list of file names of its own.
    ///
    /// The answer is [`SignedInState::SignedIn`] when a login is there, [`SignedInState::SignedOut`]
    /// when every place the program keeps one was looked at and none holds one, and
    /// [`SignedInState::Unknown`] when the provider could not tell (a place it cannot look at here,
    /// a keyring that did not answer in time), each of the last two with a reason. A login counts
    /// where the program finds one: a credentials file that is not empty, an entry of the
    /// operating system's keyring or keychain, or one of the variables the program signs in by
    /// itself with, set and not empty (`variable` names it).
    ///
    /// It starts no program, makes no network call, puts no secret in the answer and is bounded in
    /// time: a keyring that is slow answers unknown instead of waiting. A login that is there is
    /// not checked with the service, so an expired one still answers signed in. It reads this
    /// machine's own files and keyring, which are the program's when the host runs it here. Never
    /// carries a value of the environment or of the login, not in the answer, not in an error.
    ///
    /// A provider that does not offer the automatic `local` account answers
    /// `GUATIAO_ERR_UNSUPPORTED` with the reason.
    fn signed_in(&self, query: SignedInQuery) -> Result<SignedInCheck, ProviderError>;
}

/// A watch of one account, from [`AccountProvider::watch`]. Destroying it stops it, as
/// [`stop`](Self::stop) does.
#[guatiao::kind(object, name = "account_watch")]
pub trait AccountWatch: Send + Sync {
    /// Ends the watch: no keeper call starts after it answers, one under way has finished, and the
    /// spawner and the keeper it was handed are destroyed. Stopping it twice is no error.
    fn stop(&self) -> Result<(), ProviderError>;
}
