//! Accounts: what a harness accepts as an account, how a person signs in to make one, and how the
//! host hears of a credential the harness renewed or lost.
//!
//! The provider owns every fact about how its harness takes an account (kinds, variables, files,
//! login commands, endpoints). The host owns which accounts exist, their secrets at rest, who holds
//! one and for how long. A credential crosses only as [`Secret<Credentials>`](guatiao::Secret).

use alloc::collections::BTreeMap;
use alloc::collections::btree_map;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use guatiao::library::ProviderError;
use guatiao::{FromValue, Map, Schema, Secret, ToValue, Value, Wipe};

use crate::environment::Environment;

/// The kind every provider lists: the harness's own login, where it runs. Nothing is handed over
/// and nothing is cleared; the account exists to count use and limits.
pub const HARNESS_LOGIN: &str = "harness-login";

/// The longest [`AccountGrant::id`], in bytes.
pub const ACCOUNT_ID_MAX: usize = 128;

/// [`EndpointSlot::purpose`]: the URL model requests go to.
pub const MODEL_API: &str = "model_api";
/// [`EndpointSlot::purpose`]: the URL a sign-in or token refresh talks to.
pub const AUTH: &str = "auth";
/// [`EndpointSlot::purpose`]: a gateway or proxy in front of the model API.
pub const PROXY: &str = "proxy";

/// [`LoginMethodInfo::steps`]: the person opens a URL.
pub const OPEN_URL: &str = "open_url";
/// [`LoginMethodInfo::steps`]: the person confirms a one-time code on a page.
pub const USER_CODE: &str = "user_code";
/// [`LoginMethodInfo::steps`]: the person fills a form.
pub const ASK: &str = "ask";
/// [`LoginMethodInfo::steps`]: the person copies a code from a page back.
pub const PASTE_CODE: &str = "paste_code";

/// [`LoginRequest::timeout_ms`] when a host gives none: ten minutes.
pub const DEFAULT_LOGIN_TIMEOUT_MS: u64 = 600_000;

/// Whether `text` can be an account id: 1 to [`ACCOUNT_ID_MAX`] bytes and no control character.
///
/// An id is the host's label and nothing more: a provider never puts it in a path or an argument
/// vector, so it needs no more than this.
#[must_use]
pub fn is_account_id(text: &str) -> bool {
    !text.is_empty() && text.len() <= ACCOUNT_ID_MAX && !text.chars().any(char::is_control)
}

/// Credentials of an account, by the key path of the sensitive setting each fills. A file a login
/// wrote (`auth.json`) is one entry holding the file's text.
///
/// `Debug` names the keys and never shows a value. Wiping it overwrites every key and value with
/// zeros, as [`guatiao::Secret`] does when it drops one.
#[derive(ToValue, FromValue, Schema, Clone, Default, PartialEq, Eq)]
#[map(crate = ::guatiao, transparent)]
pub struct Credentials(BTreeMap<String, String>);

impl Credentials {
    /// No credentials.
    #[must_use]
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Sets `key` to `value`, answering the value it replaces.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> Option<String> {
        self.0.insert(key.into(), value.into())
    }

    /// The value of `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// Every credential, by key.
    pub fn iter(&self) -> btree_map::Iter<'_, String, String> {
        self.0.iter()
    }

    /// The keys, without the values.
    pub fn keys(&self) -> btree_map::Keys<'_, String, String> {
        self.0.keys()
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<BTreeMap<String, String>> for Credentials {
    fn from(credentials: BTreeMap<String, String>) -> Self {
        Self(credentials)
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Credentials {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        )
    }
}

impl<'a> IntoIterator for &'a Credentials {
    type Item = (&'a String, &'a String);
    type IntoIter = btree_map::Iter<'a, String, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Keys only: a value is a credential.
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

impl Wipe for Credentials {
    fn wipe(&mut self) {
        for (mut key, mut value) in core::mem::take(&mut self.0) {
            key.wipe();
            value.wipe();
        }
    }
}

fn empty_settings() -> Value {
    Value::from(Map::new())
}

/// The account a call runs on, with its credentials: what a host hands every call that runs the
/// harness (`start`, `import_session`, `export_session`, `models`, `usage`, `renew`).
///
/// A provider that applies it (sets variables, writes files into `state_dir`) clears every name
/// in its catalog's `credential_variables` first, then sets the chosen kind's, and shows no
/// credential in an answer, a refusal, an error, a log line or an argument vector.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct AccountGrant {
    /// The host's id for the account: text the host assigns, 1 to [`ACCOUNT_ID_MAX`] bytes with no
    /// control character, opaque to the provider, echoed (into usage reports) and never
    /// interpreted.
    pub id: String,
    /// One of the catalog's kinds.
    pub kind: String,
    /// The kind's non-secret settings, an object its catalog schema accepts.
    #[map(default = empty_settings())]
    pub settings: Value,
    /// The kind's secrets, by the key path of the sensitive setting each fills.
    #[map(default)]
    pub credentials: Secret<Credentials>,
    /// A private directory where the harness runs, made by the host, for a kind whose catalog
    /// entry says `state_dir`. The provider writes the account there and starts the harness on it.
    pub state_dir: Option<String>,
    /// Counts the host's stored copies of the credentials, so a renewal says which it renews.
    #[map(default)]
    pub revision: u64,
}

impl AccountGrant {
    /// A grant of `kind` for the account `id`, with no settings, credentials or directory, at
    /// revision 0.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            settings: empty_settings(),
            credentials: Secret::default(),
            state_dir: None,
            revision: 0,
        }
    }

    /// The same grant, with these settings.
    #[must_use]
    pub fn settings(mut self, settings: Value) -> Self {
        self.settings = settings;
        self
    }

    /// The same grant, with these credentials.
    #[must_use]
    pub fn credentials(mut self, credentials: Credentials) -> Self {
        self.credentials = Secret::new(credentials);
        self
    }

    /// The same grant, with this private directory.
    #[must_use]
    pub fn state_dir(mut self, state_dir: impl Into<String>) -> Self {
        self.state_dir = Some(state_dir.into());
        self
    }

    /// The same grant, at this revision.
    #[must_use]
    pub const fn revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }

    /// The grant without its credentials.
    #[must_use]
    pub fn reference(&self) -> AccountRef {
        AccountRef {
            id: self.id.clone(),
            kind: self.kind.clone(),
            settings: self.settings.clone(),
            state_dir: self.state_dir.clone(),
            revision: self.revision,
        }
    }

    /// Refuses `GUATIAO_ERR_BAD_VALUE` an id that is not [`is_account_id`].
    ///
    /// # Errors
    ///
    /// The refusal, which names no value.
    pub fn check_id(&self) -> Result<(), ProviderError> {
        check_id(&self.id)
    }
}

/// An [`AccountGrant`] without the credentials: for a question that needs only which account.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct AccountRef {
    /// As [`AccountGrant::id`].
    pub id: String,
    /// As [`AccountGrant::kind`].
    pub kind: String,
    /// As [`AccountGrant::settings`].
    #[map(default = empty_settings())]
    pub settings: Value,
    /// As [`AccountGrant::state_dir`].
    pub state_dir: Option<String>,
    /// As [`AccountGrant::revision`].
    #[map(default)]
    pub revision: u64,
}

impl AccountRef {
    /// A reference to the account `id` of `kind`, with no settings or directory, at revision 0.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            settings: empty_settings(),
            state_dir: None,
            revision: 0,
        }
    }

    /// Refuses `GUATIAO_ERR_BAD_VALUE` an id that is not [`is_account_id`].
    ///
    /// # Errors
    ///
    /// The refusal, which names no value.
    pub fn check_id(&self) -> Result<(), ProviderError> {
        check_id(&self.id)
    }
}

fn check_id(id: &str) -> Result<(), ProviderError> {
    if is_account_id(id) {
        Ok(())
    } else {
        Err(ProviderError::new(
            guatiao::Status::GUATIAO_ERR_BAD_VALUE,
            "an account id is 1 to 128 bytes with no control character",
        ))
    }
}

/// What a provider accepts as an account, from
/// [`AcpHarness::accounts`](crate::AcpHarness::accounts): cheap, and the same on every call.
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct AccountCatalog {
    /// The kinds, [`HARNESS_LOGIN`] among them.
    #[map(default)]
    pub kinds: Vec<AccountKindInfo>,
    /// The ways a person signs in to make an account.
    #[map(default)]
    pub logins: Vec<LoginMethodInfo>,
    /// Every variable any of its kinds reads: applying an account clears all of them, then sets the
    /// chosen kind's.
    #[map(default)]
    pub credential_variables: Vec<String>,
}

impl AccountCatalog {
    /// A catalog with only [`HARNESS_LOGIN`], no login method and no variable.
    #[must_use]
    pub fn new() -> Self {
        Self {
            kinds: alloc::vec![AccountKindInfo::harness_login()],
            logins: Vec::new(),
            credential_variables: Vec::new(),
        }
    }

    /// The kind `id`.
    #[must_use]
    pub fn kind(&self, id: &str) -> Option<&AccountKindInfo> {
        self.kinds.iter().find(|kind| kind.id == id)
    }
}

/// One kind of account a harness takes.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct AccountKindInfo {
    /// Its id, such as `claude-login`.
    pub id: String,
    /// Its name for people.
    pub title: String,
    /// What it is, for people.
    #[map(default)]
    pub description: String,
    /// A guatiao schema (JSON Schema, `type: object`) for the kind's settings. A credential is a
    /// property marked `writeOnly` and `x-sensitive`, and its key is the key in
    /// [`Credentials`]. A host renders its form from this.
    #[map(default = empty_settings())]
    pub settings: Value,
    /// The URLs the kind talks to, for the host's egress policy.
    #[map(default)]
    pub endpoints: Vec<EndpointSlot>,
    /// A renewal replaces a credential so that an older copy stops working. The host holds such an
    /// account exclusively. A kind whose rotation is unmeasured says `true`.
    #[map(default)]
    pub rotates: bool,
    /// A session started on one account of this kind resumes on another account of the same
    /// provider directly.
    #[map(default)]
    pub history_portable: bool,
    /// The kind needs a private directory where the harness runs ([`AccountGrant::state_dir`]).
    #[map(default)]
    pub state_dir: bool,
    /// [`renew`](crate::AcpHarness::renew) can renew it with no model turn.
    #[map(default)]
    pub renews: bool,
    /// What using it weakens, shown when the account is added and logged at start.
    #[map(default)]
    pub notices: Vec<String>,
}

impl AccountKindInfo {
    /// A kind `id` named `title`, with an empty settings object, no endpoint, none of the flags and
    /// no notice.
    #[must_use]
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: String::new(),
            settings: empty_settings(),
            endpoints: Vec::new(),
            rotates: false,
            history_portable: false,
            state_dir: false,
            renews: false,
            notices: Vec::new(),
        }
    }

    /// The kind every provider lists: the harness's own login, where it runs. It rotates, since
    /// whether it does is unmeasured, and it is not held exclusively because the host cannot see the
    /// person's own terminal on it.
    #[must_use]
    pub fn harness_login() -> Self {
        let mut kind = Self::new(HARNESS_LOGIN, "The harness's own login");
        kind.description = String::from(
            "Nothing is handed over and nothing is cleared: the harness uses the login it already has \
             where it runs. The account counts use and limits.",
        );
        kind.rotates = true;
        kind
    }
}

/// A URL a kind talks to.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct EndpointSlot {
    /// The kind's text setting holding an absolute URL; absent when the harness fixes it.
    pub key: Option<String>,
    /// What it is for: [`MODEL_API`], [`AUTH`] or [`PROXY`].
    pub purpose: String,
    /// The URL used when the setting is absent.
    pub default: Option<String>,
}

impl EndpointSlot {
    /// A slot for `purpose`, fixed by the harness and with no known default.
    #[must_use]
    pub fn new(purpose: impl Into<String>) -> Self {
        Self {
            key: None,
            purpose: purpose.into(),
            default: None,
        }
    }

    /// The same slot, set by the text setting `key`.
    #[must_use]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// The same slot, defaulting to `url`.
    #[must_use]
    pub fn default_url(mut self, url: impl Into<String>) -> Self {
        self.default = Some(url.into());
        self
    }
}

/// One way a person signs in to make an account.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct LoginMethodInfo {
    /// Its id, such as `subscription`.
    pub id: String,
    /// Its name for people.
    pub title: String,
    /// What it does, for people.
    #[map(default)]
    pub description: String,
    /// The kind of account it makes: one of the catalog's kinds.
    pub produces: String,
    /// A schema of what is asked before it runs (an email, an issuer); an empty object asks nothing.
    #[map(default = empty_settings())]
    pub inputs: Value,
    /// The steps it relays, in the order they can come: [`OPEN_URL`], [`USER_CODE`], [`ASK`],
    /// [`PASTE_CODE`].
    #[map(default)]
    pub steps: Vec<String>,
    /// Whether it works through a spawner that is not local. A loopback callback needs the person's
    /// browser on the machine where the CLI runs, so it is `false` unless the CLI offers a code to
    /// paste or a device code.
    #[map(default)]
    pub remote: bool,
}

impl LoginMethodInfo {
    /// A method `id` named `title`, making `produces`, asking nothing first, relaying no step and
    /// not remote.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        produces: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: String::new(),
            produces: produces.into(),
            inputs: empty_settings(),
            steps: Vec::new(),
            remote: false,
        }
    }
}

/// A sign-in to run, for [`AcpHarness::login`](crate::AcpHarness::login).
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct LoginRequest {
    /// One of the catalog's [`LoginMethodInfo::id`].
    pub method: String,
    /// The answers to the method's `inputs` schema.
    #[map(default = empty_settings())]
    pub inputs: Value,
    /// A private directory where the harness runs, made by the host, where the CLI runs and writes
    /// what it signs in with. The host removes it however the login ended.
    pub state_dir: String,
    /// The most the whole login may take.
    #[map(default = DEFAULT_LOGIN_TIMEOUT_MS)]
    pub timeout_ms: u64,
}

impl LoginRequest {
    /// Runs `method` in `state_dir`, asking no inputs, for at most [`DEFAULT_LOGIN_TIMEOUT_MS`].
    #[must_use]
    pub fn new(method: impl Into<String>, state_dir: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            inputs: empty_settings(),
            state_dir: state_dir.into(),
            timeout_ms: DEFAULT_LOGIN_TIMEOUT_MS,
        }
    }

    /// The same request, with these inputs.
    #[must_use]
    pub fn inputs(mut self, inputs: Value) -> Self {
        self.inputs = inputs;
        self
    }

    /// The same request, ending after `ms`.
    #[must_use]
    pub const fn timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }
}

/// The account a sign-in made, from [`AcpHarness::login`](crate::AcpHarness::login). The host
/// stores it as an account of `kind`.
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct LoginOutcome {
    /// The kind made: the method's `produces`.
    pub kind: String,
    /// Its non-secret settings.
    #[map(default = empty_settings())]
    pub settings: Value,
    /// What the login wrote or was given, read from the files it wrote.
    #[map(default)]
    pub credentials: Secret<Credentials>,
    /// A name for people the provider learned (an email address), when it did.
    pub label: Option<String>,
    /// When the credentials stop working, as an RFC 3339 time, when known.
    pub expires_at: Option<String>,
    /// Whether the provider verified the login with a call that spends nothing.
    #[map(default)]
    pub checked: bool,
}

impl LoginOutcome {
    /// An account of `kind` with these credentials, no settings or label, not checked.
    #[must_use]
    pub fn new(kind: impl Into<String>, credentials: Credentials) -> Self {
        Self {
            kind: kind.into(),
            settings: empty_settings(),
            credentials: Secret::new(credentials),
            label: None,
            expires_at: None,
            checked: false,
        }
    }
}

/// An account to renew, for [`AcpHarness::renew`](crate::AcpHarness::renew).
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct RenewRequest {
    /// The account, with the credentials the host holds.
    pub account: AccountGrant,
}

impl RenewRequest {
    /// A request to renew `account`.
    #[must_use]
    pub const fn new(account: AccountGrant) -> Self {
        Self { account }
    }
}

/// What a renewal did, from [`AcpHarness::renew`](crate::AcpHarness::renew).
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct RenewReport {
    /// The credentials after the renewal, when it made new ones; absent when nothing changed.
    pub credentials: Option<Secret<Credentials>>,
    /// When the credentials now stop working, as an RFC 3339 time, when known.
    pub expires_at: Option<String>,
    /// Why nothing was renewed (not within the harness's renewal window, say), when so. It shows no
    /// credential.
    pub not_renewed_because: Option<String>,
}

impl RenewReport {
    /// A renewal that made `credentials`.
    #[must_use]
    pub fn renewed(credentials: Credentials) -> Self {
        Self {
            credentials: Some(Secret::new(credentials)),
            expires_at: None,
            not_renewed_because: None,
        }
    }

    /// A renewal that changed nothing, for `reason`.
    #[must_use]
    pub fn unchanged(reason: impl Into<String>) -> Self {
        Self {
            credentials: None,
            expires_at: None,
            not_renewed_because: Some(reason.into()),
        }
    }
}

/// The models an account may use, for [`AcpHarness::models`](crate::AcpHarness::models).
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq)]
#[map(crate = ::guatiao, deny_unknown_fields)]
#[non_exhaustive]
pub struct ModelsRequest {
    /// Variables for what the harness runs to ask, over its configuration's. A provider shows none
    /// of them in an answer, a refusal, an error or a log.
    #[map(default)]
    pub environment: Secret<Environment>,
    /// The account to ask for; absent asks as the configuration stands.
    pub account: Option<AccountGrant>,
}

impl ModelsRequest {
    /// A request adding nothing to the provider's configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The variables for what the harness runs.
    #[must_use]
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = Secret::new(environment);
        self
    }

    /// The account to ask for.
    #[must_use]
    pub fn account(mut self, account: AccountGrant) -> Self {
        self.account = Some(account);
        self
    }
}

/// Credentials a harness renewed, reported through [`AccountKeeper::renewed`].
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct AccountRenewal {
    /// The account's id.
    pub id: String,
    /// The revision of the grant the renewal replaces.
    pub from_revision: u64,
    /// The credentials now, whole: every key of the grant.
    pub credentials: Secret<Credentials>,
    /// When they stop working, as an RFC 3339 time, when known.
    pub expires_at: Option<String>,
    /// Counts what happened to this account, renewals and losses together, from 1, without a gap
    /// or a repeat for as long as the provider lives, so the host orders chained rotations by it.
    /// The same event reaches every keeper that is live for the account and carries the same
    /// sequence: the host deduplicates by (account, sequence).
    pub sequence: u64,
}

impl AccountRenewal {
    /// A renewal of the account `id`, which replaces `from_revision`.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        from_revision: u64,
        sequence: u64,
        credentials: Credentials,
    ) -> Self {
        Self {
            id: id.into(),
            from_revision,
            credentials: Secret::new(credentials),
            expires_at: None,
            sequence,
        }
    }
}

/// A login a harness lost, reported through [`AccountKeeper::lost`].
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct AccountLoss {
    /// The account's id.
    pub id: String,
    /// Why, for people; it shows no credential.
    pub reason: String,
    /// As [`AccountRenewal::sequence`]: one count for the account's renewals and losses.
    pub sequence: u64,
}

impl AccountLoss {
    /// The account `id` is lost, for `reason`.
    #[must_use]
    pub fn new(id: impl Into<String>, sequence: u64, reason: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            reason: reason.into(),
            sequence,
        }
    }
}

/// Where a provider reports what happens to an account's credentials behind the host's back. The
/// host implements it and hands one to every call that runs the harness; the provider owns it from
/// then on, and destroys it by the time it calls the sink's `closed` (or when the call returns).
///
/// **Every live keeper of the account hears each event**, not the first to ask: a connection's
/// keeper hears a rotation a `models` call noticed, and the other way round. An event carries a
/// sequence the provider counts per account; **the host deduplicates by (account, sequence)** and
/// orders chained rotations by it, so a keeper that hears an event twice, or two keepers of one
/// host that hear it once each, store it once.
///
/// A provider of a kind with files watches them through the spawner while the connection runs
/// (every `renew_poll_ms`, 30000 by default) and after each import, export, models call and close,
/// and reports a changed file here. A file deleted or blanked is [`lost`](Self::lost), never
/// [`renewed`](Self::renewed).
#[guatiao::kind(object, name = "account_keeper")]
pub trait AccountKeeper: Send + Sync {
    /// The harness renewed the credentials. The host stores them only when they are newer.
    fn renewed(&self, renewal: AccountRenewal) -> Result<(), ProviderError>;

    /// The harness lost the login (a refused refresh blanked or deleted its file). The blank copy
    /// is never reported as a renewal.
    fn lost(&self, loss: AccountLoss) -> Result<(), ProviderError>;
}

/// A page the person is asked to open, for [`LoginPrompter::open_url`].
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq, Eq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct OpenUrl {
    /// The page, as the CLI printed it.
    pub url: String,
    /// The one-time code the person confirms there, when the CLI printed one.
    pub user_code: Option<String>,
    /// How long the page and code stay good, in milliseconds, when known.
    pub expires_ms: Option<u64>,
}

impl OpenUrl {
    /// Open `url`, with no code.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            user_code: None,
            expires_ms: None,
        }
    }
}

/// A form the person fills, for [`LoginPrompter::ask`].
#[derive(ToValue, FromValue, Schema, Debug, Clone, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct LoginForm {
    /// What it asks, for people.
    pub title: String,
    /// A schema of the fields. A secret is a property marked `writeOnly` and `x-sensitive`, and the
    /// host answers it in [`LoginAnswer::secrets`].
    #[map(default = empty_settings())]
    pub schema: Value,
}

impl LoginForm {
    /// A form titled `title` for `schema`.
    #[must_use]
    pub fn new(title: impl Into<String>, schema: Value) -> Self {
        Self {
            title: title.into(),
            schema,
        }
    }
}

/// What the person answered, from [`LoginPrompter::ask`].
#[derive(ToValue, FromValue, Schema, Debug, Clone, Default, PartialEq)]
#[map(crate = ::guatiao)]
#[non_exhaustive]
pub struct LoginAnswer {
    /// The values of the fields that are not secrets, an object.
    #[map(default = empty_settings())]
    pub values: Value,
    /// The values of the sensitive fields, by property name.
    #[map(default)]
    pub secrets: Secret<Credentials>,
}

impl LoginAnswer {
    /// An answer with these values and secrets.
    #[must_use]
    pub fn new(values: Value, secrets: Credentials) -> Self {
        Self {
            values,
            secrets: Secret::new(secrets),
        }
    }
}

/// How a login relays its steps to the person. The host implements it, hands one to
/// [`login`](crate::AcpHarness::login) and renders each call; the provider owns it from then on and
/// destroys it when the login ends. `ask` goes sealed from the person's frontend to where the
/// provider runs, never through a model, a channel the agent reads or a log.
///
/// When the person cancels, every call answers `GUATIAO_ERR_GONE` and `cancelled` is true.
#[guatiao::kind(object, name = "login_prompter")]
pub trait LoginPrompter: Send + Sync {
    /// Shows `text`, a line the CLI printed that the person should read.
    fn say(&self, text: &str) -> Result<(), ProviderError>;

    /// Asks the person to open a page, and to confirm a code there.
    fn open_url(&self, open: OpenUrl) -> Result<(), ProviderError>;

    /// Asks the person to fill `form` and waits for the answer.
    fn ask(&self, form: LoginForm) -> Result<LoginAnswer, ProviderError>;

    /// Whether the person cancelled the login.
    fn cancelled(&self) -> Result<bool, ProviderError>;
}
