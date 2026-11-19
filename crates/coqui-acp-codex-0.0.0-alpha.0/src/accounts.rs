//! The accounts Codex takes: the catalog the provider lists, and what applying a grant does to Codex's environment and
//! home directory.
//!
//! Measured on Codex 0.166.0. A `rotates` nobody measured is `true` (except where a kind says why not), a
//! `history_portable` nobody measured `false`.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use std::ffi::OsString;
use std::sync::{Mutex, PoisonError};

use coqui_acp_api::guatiao::library::ProviderError;
use coqui_acp_api::guatiao::{Secret, Status, Wipe};
use coqui_acp_api::standard::{CODEX, OPENAI_API};
use coqui_acp_api::{
    ASK, AUTH, Accepted, AcceptedSlot, AccountCatalog, AccountGrant, AccountKindInfo, AccountOffer,
    AccountRef, Credentials, EndpointSlot, HARNESS_LOGIN, LoginMethodInfo, MODEL_API, OPEN_URL,
    PASTE_CODE, USER_CODE,
};
use coqui_acp_harness::ThroughSpawner;
use coqui_acp_harness::account::{
    Accounts, Files, Hold, Watched, fingerprint, jwt_expiry, same_name,
};
use coqui_acp_harness::provider::{credential, resolve, schema, to_json};
use serde_json::{Value, json};

use crate::{CodexLaunch, signin};

/// The kind of a ChatGPT login made by `codex login`, kept in `auth.json` of `CODEX_HOME`.
pub const CHATGPT: &str = "codex-chatgpt";
/// The kind of an API key, made by `codex login --with-api-key`.
pub const API_KEY: &str = "codex-api-key";
/// The kind of an access token, made by `codex login --with-access-token`.
pub const ACCESS_TOKEN: &str = "codex-access-token";

/// The login method that makes [`CHATGPT`] by `codex login`, the page's callback coming back to this machine.
pub const CHATGPT_METHOD: &str = "chatgpt";
/// The login method that makes [`CHATGPT`] by `codex login --device-auth`.
pub const DEVICE_METHOD: &str = "chatgpt-device";
/// The login method that takes a key and makes [`API_KEY`].
pub const API_KEY_METHOD: &str = "api-key";
/// The login method that takes a token and makes [`ACCESS_TOKEN`].
pub const ACCESS_TOKEN_METHOD: &str = "access-token";

/// The file in Codex's home directory that holds a login or a key.
pub const AUTH_FILE: &str = "auth.json";
/// The credential key that holds [`AUTH_FILE`]'s text for [`CHATGPT`].
pub const FILE_KEY: &str = "auth_file";
/// Where Codex is told to keep its credentials, written into its home so every command on it agrees.
pub(crate) const CONFIG_FILE: &str = "config.toml";
/// What [`CONFIG_FILE`] holds when this provider makes it.
pub(crate) const CONFIG_TEXT: &str = "cli_auth_credentials_store = \"file\"\n";

/// Every variable Codex reads for a credential or for where its requests and token refreshes go, so a start on an
/// account removes them all and sets only the kind's.
///
/// Measured (strings of the binary, `references/account_kinds.md`): the credential variables, the three
/// `CODEX_*_URL_OVERRIDE`/`CODEX_APP_SERVER_LOGIN_ISSUER` levers and `CODEX_SQLITE_HOME`, which moves Codex's state out
/// of its home. `OPENAI_BASE_URL` is not measured: it is the base address OpenAI's own libraries read.
pub(crate) const VARIABLES: &[&str] = &[
    "CODEX_API_KEY",
    "OPENAI_API_KEY",
    "CODEX_ACCESS_TOKEN",
    "CODEX_REFRESH_TOKEN_URL_OVERRIDE",
    "CODEX_APP_SERVER_LOGIN_ISSUER",
    "CODEX_REVOKE_TOKEN_URL_OVERRIDE",
    "CODEX_SQLITE_HOME",
    "OPENAI_BASE_URL",
];

/// The part of [`VARIABLES`] whose value is itself a secret: a key or a token. Not an endpoint, an issuer or where
/// Codex keeps its state, which a person sets on purpose and which a start still clears.
pub(crate) const SECRETS: &[&str] = &["CODEX_API_KEY", "OPENAI_API_KEY", "CODEX_ACCESS_TOKEN"];

fn settings(secret: &str, endpoint: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            secret: credential(),
            endpoint: {"type": "string", "format": "uri"},
        },
        "required": [secret],
        "additionalProperties": false,
    })
}

fn kind(id: &str, title: &str, description: &str, secret: &str, endpoint: &str) -> AccountKindInfo {
    let mut kind = AccountKindInfo::new(id, title);
    kind.description = description.to_string();
    kind.settings = schema(&settings(secret, endpoint));
    kind.state_dir = true;
    kind
}

fn method(id: &str, title: &str, description: &str, produces: &str) -> LoginMethodInfo {
    let mut method = LoginMethodInfo::new(id, title, produces);
    method.description = description.to_string();
    method
}

/// Every kind and sign-in method Codex's accounts have, as the account provider lists them and the harness takes them.
fn full() -> AccountCatalog {
    let mut catalog = AccountCatalog::new();

    let mut chatgpt = kind(
        CHATGPT,
        "ChatGPT login",
        "A login made by `codex login`, kept in `auth.json` of Codex's home directory.",
        FILE_KEY,
        "chatgpt_base_url",
    );
    chatgpt.endpoints = vec![
        EndpointSlot::new(MODEL_API)
            .key("chatgpt_base_url")
            .default_url("https://chatgpt.com/backend-api/codex"),
        EndpointSlot::new(AUTH).default_url("https://auth.openai.com"),
    ];
    chatgpt.rotates = true;
    chatgpt.renews = true;
    chatgpt.notices =
        vec!["the login is kept in a file that what the agent runs can read".to_string()];

    let mut key = kind(
        API_KEY,
        "OpenAI API key",
        "A key saved by `codex login --with-api-key` in `auth.json` of Codex's home directory.",
        "api_key",
        "openai_base_url",
    );
    key.endpoints = vec![
        EndpointSlot::new(MODEL_API)
            .key("openai_base_url")
            .default_url("https://api.openai.com/v1"),
    ];
    key.notices = vec!["the key is kept in a file that what the agent runs can read".to_string()];

    let mut token = kind(
        ACCESS_TOKEN,
        "Codex access token",
        "A token Codex checks offline, saved by `codex login --with-access-token` in `auth.json` of its home directory.",
        "access_token",
        "chatgpt_base_url",
    );
    token.endpoints = vec![
        EndpointSlot::new(MODEL_API)
            .key("chatgpt_base_url")
            .default_url("https://chatgpt.com/backend-api/codex"),
    ];
    // Codex checks the token's format offline and no refresh credential comes with it, so nothing of it is renewed
    // behind the host's back: the success path was not run, and this is what its failure path shows.
    token.notices = vec![
        "the token is kept in a file that what the agent runs can read".to_string(),
        "its success path was not measured: it is taken not to rotate".to_string(),
    ];

    catalog.kinds.extend([chatgpt, key, token]);
    let mut web = method(
        CHATGPT_METHOD,
        "Sign in with ChatGPT",
        "Runs `codex login`: a page to open, whose callback comes back to the machine where Codex runs. From another machine, paste the address the browser ends on.",
        CHATGPT,
    );
    web.steps = vec![OPEN_URL.to_string(), PASTE_CODE.to_string()];
    web.remote = true;
    let mut device = method(
        DEVICE_METHOD,
        "Sign in with ChatGPT by a code",
        "Runs `codex login --device-auth`: a page to open and a one-time code to confirm there.",
        CHATGPT,
    );
    device.steps = vec![OPEN_URL.to_string(), USER_CODE.to_string()];
    device.remote = true;
    let mut api = method(
        API_KEY_METHOD,
        "Use an OpenAI API key",
        "Takes a key and saves it with `codex login --with-api-key`, the key on standard input.",
        API_KEY,
    );
    api.steps = vec![ASK.to_string()];
    api.remote = true;
    let mut access = method(
        ACCESS_TOKEN_METHOD,
        "Use an access token",
        "Takes a token and saves it with `codex login --with-access-token`, the token on standard input.",
        ACCESS_TOKEN,
    );
    access.steps = vec![ASK.to_string()];
    access.remote = true;
    catalog.logins = vec![web, device, api, access];
    for kind in &mut catalog.kinds {
        let standard = if kind.id == API_KEY {
            OPENAI_API
        } else {
            CODEX
        };
        kind.provides = vec![standard.to_string()];
    }
    catalog.accepts = accepts();
    catalog.credential_variables = VARIABLES.iter().map(|v| (*v).to_string()).collect();
    catalog.secret_variables = SECRETS.iter().map(|v| (*v).to_string()).collect();
    catalog
}

/// What the harness takes as an account: the kinds it applies and the standards it accepts. It signs no one in and
/// renews nothing: the account provider ([`offer`]) does.
#[must_use]
pub fn catalog() -> AccountCatalog {
    let mut catalog = full();
    catalog.logins.clear();
    for kind in &mut catalog.kinds {
        kind.renews = false;
    }
    catalog
}

/// What the account provider offers: the kinds of account it makes (the harness's own login, which it cannot sign in
/// to, left out), the ways to sign in, and the kinds whose plan usage it reads.
#[must_use]
pub fn offer() -> AccountOffer {
    let full = full();
    let mut offer = AccountOffer::new();
    offer.kinds = full
        .kinds
        .into_iter()
        .filter(|kind| kind.id != HARNESS_LOGIN)
        .collect();
    offer.logins = full.logins;
    offer.usage = [CHATGPT, ACCESS_TOKEN, API_KEY]
        .iter()
        .map(|id| (*id).to_string())
        .collect();
    offer
}

/// Why an account is not one the account provider keeps: `BAD_VALUE` for an id that is not one or a kind it does not
/// list. Nothing of another standard is taken: a harness applies those, the provider makes its own.
pub(crate) fn own(grant: &AccountGrant) -> Result<(), ProviderError> {
    grant.check_id()?;
    if offer().kind(&grant.kind).is_none() {
        return Err(bad("this provider keeps no account of that kind"));
    }
    Ok(())
}

/// The standards Codex starts on, and how each is applied: a Codex login (`auth.json` in `CODEX_HOME`, or an access token
/// saved there by `codex login --with-access-token`), or an OpenAI API key (saved there by `codex login --with-api-key`,
/// its endpoint the `openai_base_url` configuration key). Codex's home is the grant's private directory in each.
fn accepts() -> Vec<Accepted> {
    vec![
        Accepted::new(CODEX)
            .slot(
                AcceptedSlot::new(FILE_KEY, CHATGPT)
                    .file(AUTH_FILE)
                    .dir_variable("CODEX_HOME")
                    .base_url("chatgpt_base_url", None),
            )
            .slot(
                AcceptedSlot::new("access_token", ACCESS_TOKEN)
                    .dir_variable("CODEX_HOME")
                    .base_url("chatgpt_base_url", None),
            ),
        Accepted::new(OPENAI_API).slot(
            AcceptedSlot::new("api_key", API_KEY)
                .dir_variable("CODEX_HOME")
                .base_url("openai_base_url", None),
        ),
    ]
}

/// The grant of Codex's own kind that applies `grant`, when `grant` is an account of an accepted standard.
pub(crate) fn resolved(grant: &AccountGrant) -> Result<AccountGrant, ProviderError> {
    resolve(&catalog(), grant)
}

fn bad(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, why)
}

/// The endpoint setting `key` of `settings`: absent, or a plain `http(s)` address.
pub(crate) fn endpoint(
    settings: &coqui_acp_api::guatiao::Value,
    key: &str,
) -> Result<Option<String>, ProviderError> {
    let settings = to_json(settings);
    let Some(object) = settings.as_object() else {
        return Err(bad("the account's settings are not an object"));
    };
    if object.keys().any(|name| name != key) {
        return Err(bad("the account's settings name something the kind lacks"));
    }
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(url))
            if (url.starts_with("https://") || url.starts_with("http://"))
                && url.len() > 8
                && !url.chars().any(|c| c.is_control() || c.is_whitespace()) =>
        {
            Ok(Some(url.clone()))
        }
        Some(_) => Err(bad("the endpoint is not an address")),
    }
}

/// The text holds a login or a key: not blank, and when it is Codex's JSON, a token or key in it.
pub(crate) fn usable(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(object)) => {
            let text = |value: Option<&Value>| {
                value
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
            };
            let tokens = object.get("tokens");
            text(object.get("OPENAI_API_KEY"))
                || text(tokens.and_then(|t| t.get("access_token")))
                || text(tokens.and_then(|t| t.get("refresh_token")))
        }
        Ok(_) => false,
        // Not the JSON Codex writes: whatever it is, it is not blank.
        Err(_) => true,
    }
}

/// How a key or token reaches Codex: by `codex login`, with the secret on its standard input.
#[derive(Clone)]
#[non_exhaustive]
pub(crate) struct StdinLogin {
    /// The flag: `--with-api-key` or `--with-access-token`.
    pub flag: &'static str,
    /// The key of the credentials that holds the secret.
    pub key: &'static str,
    /// The secret.
    pub secret: String,
}

/// What applying a grant does.
#[non_exhaustive]
pub(crate) struct Plan {
    /// The variables removed before, every credential variable.
    pub remove: Vec<String>,
    /// The variables Codex is given.
    pub set: Vec<(String, String)>,
    /// `--config` overrides of the app server.
    pub config: Vec<(String, Value)>,
    /// Codex's home directory.
    pub dir: String,
    /// The files that hold a login Codex may refresh.
    pub watched: Vec<Watched>,
    /// The login to run for a key or token.
    pub login: Option<StdinLogin>,
}

fn settings_key(kind: &str) -> &'static str {
    match kind {
        API_KEY => "openai_base_url",
        _ => "chatgpt_base_url",
    }
}

/// Why a reference to an account is not one the provider takes.
pub(crate) fn check(account: &AccountRef) -> Result<(), ProviderError> {
    let catalog = catalog();
    catalog.accepts_reference(account)?;
    let Some(info) = catalog.kind(&account.kind) else {
        // An account of an accepted standard: `plan` reads its credential and settings.
        return Ok(());
    };
    if info.id == HARNESS_LOGIN {
        if !to_json(&account.settings)
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            return Err(bad("that kind takes no settings"));
        }
        return Ok(());
    }
    endpoint(&account.settings, settings_key(&info.id))?;
    if account.state_dir.as_deref().is_none_or(str::is_empty) {
        return Err(bad("that kind needs a directory to run in"));
    }
    Ok(())
}

/// The variables applying `grant` sets in Codex's environment that hold no credential, by name: its home
/// (`CODEX_HOME`, for every kind but `harness-login`, when the grant has a directory). A host reads them to tell its
/// profile's layers what an account sets before a session starts; an endpoint is Codex's configuration, not a
/// variable, and the credentials' own are never listed. A grant this provider cannot apply lists none.
#[must_use]
pub fn variables(grant: &AccountGrant) -> Vec<(String, String)> {
    let Ok(grant) = resolved(grant) else {
        return Vec::new();
    };
    match grant.state_dir.as_deref().filter(|dir| !dir.is_empty()) {
        Some(dir) if grant.kind != HARNESS_LOGIN => {
            vec![("CODEX_HOME".to_string(), dir.to_string())]
        }
        _ => Vec::new(),
    }
}

/// What applying `grant` does; `None` for `harness-login`, which hands nothing over and clears nothing.
///
/// # Errors
///
/// `BAD_VALUE`: an id that is not one, a kind not listed, settings the kind refuses, a credential missing, empty or
/// one the kind does not take, a missing directory.
pub(crate) fn plan(grant: &AccountGrant) -> Result<Option<Plan>, ProviderError> {
    plan_as(grant, false)
}

/// [`plan`], where a ChatGPT login with no file in the grant (`in_place`) is the home as it is.
fn plan_as(grant: &AccountGrant, in_place: bool) -> Result<Option<Plan>, ProviderError> {
    let grant = &resolved(grant)?;
    check(&grant.reference())?;
    let credentials = grant.credentials.expose();
    if grant.kind == HARNESS_LOGIN {
        if !credentials.is_empty() {
            return Err(bad("that kind takes no credential"));
        }
        return Ok(None);
    }
    let key = match grant.kind.as_str() {
        CHATGPT => FILE_KEY,
        API_KEY => "api_key",
        _ => "access_token",
    };
    let secret = credentials
        .get(key)
        .filter(|value| !value.is_empty() && credentials.keys().all(|k| k == key));
    let there = in_place && grant.kind == CHATGPT && credentials.is_empty();
    if secret.is_none() && !there {
        return Err(bad("the credential is missing or not the kind's"));
    }
    let mut applied = Plan {
        remove: VARIABLES.iter().map(|v| (*v).to_string()).collect(),
        set: Vec::new(),
        config: Vec::new(),
        dir: grant.state_dir.clone().unwrap_or_default(),
        watched: Vec::new(),
        login: None,
    };
    applied
        .set
        .push(("CODEX_HOME".to_string(), applied.dir.clone()));
    let setting = settings_key(&grant.kind);
    if let Some(url) = endpoint(&grant.settings, setting)? {
        applied.config.push((setting.to_string(), json!(url)));
    }
    match grant.kind.as_str() {
        CHATGPT => {
            if !there {
                applied.watched = vec![
                    Watched::new(AUTH_FILE, FILE_KEY)
                        .usable(usable)
                        .expires(expires)
                        .identity(identity),
                ];
            }
        }
        API_KEY => {
            applied.login = secret.map(|secret| StdinLogin {
                flag: "--with-api-key",
                key: "api_key",
                secret: secret.to_string(),
            });
        }
        _ => {
            applied.login = secret.map(|secret| StdinLogin {
                flag: "--with-access-token",
                key: "access_token",
                secret: secret.to_string(),
            });
        }
    }
    Ok(Some(applied))
}

/// How long [`apply`] lets `codex login` run to save a key or token, in milliseconds.
pub const DEFAULT_LOGIN_TIMEOUT_MS: u64 = 60_000;

/// How [`apply_with`] runs the command that saves a key or token.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ApplyOptions {
    /// The longest `codex login` may take; [`DEFAULT_LOGIN_TIMEOUT_MS`].
    pub timeout: Duration,
    /// What follows the program, before `login`, when it is a wrapper (`wsl` for `wsl codex`); none.
    pub leading: Vec<OsString>,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(DEFAULT_LOGIN_TIMEOUT_MS),
            leading: Vec::new(),
        }
    }
}

impl ApplyOptions {
    /// The defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Lets `codex login` run for at most `timeout`.
    #[must_use]
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Starts the login as `program leading... login <flag>`, `program` being the launch's `executable`.
    #[must_use]
    pub fn leading(mut self, leading: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        self.leading = leading.into_iter().map(Into::into).collect();
        self
    }
}

/// What is left to do for an account [`apply`] has applied to a launch: the files of a login the account keeps.
///
/// It holds no credential a log or a `Debug` would show.
#[must_use = "a login kept in files is placed only by `hold`"]
pub struct Applied {
    id: String,
    revision: u64,
    credentials: Secret<Credentials>,
    dir: String,
    watched: Vec<Watched>,
}

impl core::fmt::Debug for Applied {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Applied")
            .field("id", &self.id)
            .field("dir", &self.dir)
            .field("watched", &self.watched.len())
            .finish_non_exhaustive()
    }
}

impl Applied {
    /// Codex's home directory; empty for `harness-login`.
    #[must_use]
    pub fn dir(&self) -> &str {
        &self.dir
    }

    /// Whether the account keeps a ChatGPT login in a file that [`hold`](Self::hold) places and watches.
    #[must_use]
    pub fn keeps_files(&self) -> bool {
        !self.watched.is_empty()
    }

    /// Places the file of a ChatGPT login in the account's home through `files` and holds it in `accounts`, which sees
    /// to it that the grant's copy replaces only one that is blank, older or not a file, and tells a renewal or a loss
    /// to the keepers of a live hold. `None` for an account that keeps no such file (a key or token was saved by
    /// [`apply`] already): nothing is written. The hold lasts until [`Hold::finish`] or its drop; keep it as long as
    /// Codex runs.
    ///
    /// # Errors
    ///
    /// `BAD_VALUE` when the grant carries no copy of the login; the spawner's refusal to read or write a file.
    pub fn hold(
        self,
        accounts: &Arc<Accounts>,
        files: &Files,
    ) -> Result<Option<Hold>, ProviderError> {
        if self.watched.is_empty() {
            return Ok(None);
        }
        accounts
            .hold(
                files.clone(),
                None,
                &self.id,
                &self.dir,
                self.revision,
                self.credentials.expose(),
                self.watched,
            )
            .map(Some)
    }
}

/// [`apply_with`] with the defaults.
///
/// # Errors
///
/// As [`apply_with`].
pub fn apply(
    grant: &AccountGrant,
    launch: &mut CodexLaunch,
    files: &Files,
) -> Result<Applied, ProviderError> {
    apply_with(grant, launch, files, &ApplyOptions::default())
}

/// Applies `grant` to `launch`, as `start` does: every credential variable is removed from what Codex starts with,
/// including those of `launch.env`, `CODEX_HOME` is set to the grant's `state_dir`, the endpoint the account names goes
/// to `launch.config_overrides`, and the home is made to keep its credentials in `auth.json` (a `config.toml` written
/// through `files` when none is there). A key or token is saved there by `launch.executable login --with-api-key` or
/// `--with-access-token`, the secret on its standard input and in no argument, log or error, through `files`' spawner
/// and within `options.timeout`; a key `auth.json` already holds is not saved again. `harness-login` changes nothing.
///
/// A ChatGPT login whose grant carries no `auth_file` and a `state_dir` is applied **in place**: Codex runs on that home
/// as it is and nothing is written, not even `config.toml`. A login is placed only when the grant carries its copy, and
/// only by [`Applied::hold`].
///
/// The endpoint variables are cleared and set only from the grant's settings: a host that keeps a person's own choice
/// (`OPENAI_BASE_URL`) puts it back afterwards. They are among the catalog's `credential_variables` and not among its
/// `secret_variables`.
///
/// # Errors
///
/// `BAD_VALUE`: an id that is not one, a kind not listed, settings the kind refuses, a credential missing or not the
/// kind's, a missing `state_dir`, or a key Codex refuses (`login refused: ...`); `UNSUPPORTED`: a home to write on a
/// spawner with no files; `INTERNAL` when Codex could not be run or did not finish in time.
pub fn apply_with(
    grant: &AccountGrant,
    launch: &mut CodexLaunch,
    files: &Files,
    options: &ApplyOptions,
) -> Result<Applied, ProviderError> {
    apply_as(grant, launch, files, options, true, None)
}

/// For each home a key or token was saved in, a fingerprint of it.
pub(crate) type Saved = Mutex<BTreeMap<String, u64>>;

/// [`apply_with`] for `start` and the other calls of the harness, which refuse a login that carries no copy
/// (`in_place` false) and, given `saved`, do not save a token again for a home it was saved in.
pub(crate) fn apply_as(
    grant: &AccountGrant,
    launch: &mut CodexLaunch,
    files: &Files,
    options: &ApplyOptions,
    in_place: bool,
    saved: Option<&Saved>,
) -> Result<Applied, ProviderError> {
    let grant = &resolved(grant)?;
    let Some(mut plan) = plan_as(grant, in_place)? else {
        return Ok(Applied {
            id: grant.id.clone(),
            revision: grant.revision,
            credentials: Secret::new(Credentials::new()),
            dir: String::new(),
            watched: Vec::new(),
        });
    };
    let writes = !plan.watched.is_empty() || plan.login.is_some() || !in_place;
    if writes && !files.available() {
        return Err(ProviderError::new(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this spawner serves no private files, which the credential is kept in",
        ));
    }
    // What the host's environment holds under a credential name does not reach Codex beside the account's.
    launch.env.retain(|name, _| {
        !plan
            .remove
            .iter()
            .any(|v| same_name(&name.to_string_lossy(), v))
    });
    launch.remove_env = plan.remove.iter().map(OsString::from).collect();
    for (name, value) in &plan.set {
        launch.env.insert(name.into(), value.into());
    }
    launch.config_overrides.extend(plan.config.iter().cloned());
    if writes {
        signin::configure(files)?;
    }
    if let Some(login) = &plan.login {
        let spawn = ThroughSpawner::shared(Arc::clone(files.spawner()));
        let print = fingerprint(&login.secret);
        let there = files.info(AUTH_FILE)?.is_some();
        // A key already in `auth.json` is not saved again. A token, which Codex does not show back, is saved once for
        // this home while the file is there when the caller keeps a record: a poll of the account's usage does not run
        // `codex login` each time.
        let done = if login.key == "api_key" {
            files
                .read(AUTH_FILE)?
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|auth| auth["OPENAI_API_KEY"].as_str() == Some(&login.secret))
        } else {
            there
                && saved.is_some_and(|saved| {
                    saved
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get(&plan.dir)
                        == Some(&print)
                })
        };
        if !done {
            signin::save(
                &spawn,
                &launch.executable,
                &options.leading,
                &plan.dir,
                login,
                options.timeout,
                None,
            )?;
            if let Some(saved) = saved {
                saved
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(plan.dir.clone(), print);
            }
        }
    }
    Ok(Applied {
        id: grant.id.clone(),
        revision: grant.revision,
        credentials: Secret::new(grant.credentials.expose().clone()),
        dir: core::mem::take(&mut plan.dir),
        watched: core::mem::take(&mut plan.watched),
    })
}

/// Whom the login in the file's text is for: its `tokens.account_id`.
pub(crate) fn identity(text: &str) -> Option<String> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("tokens")?
        .get("account_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// When the login in the file's text stops working: the `exp` of its access token.
pub(crate) fn expires(text: &str) -> Option<String> {
    let file = serde_json::from_str::<Value>(text).ok()?;
    jwt_expiry(file.get("tokens")?.get("access_token")?.as_str()?)
}

impl core::fmt::Debug for StdinLogin {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The secret is not shown.
        f.debug_struct("StdinLogin")
            .field("flag", &self.flag)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl core::fmt::Debug for Plan {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The variables set may hold a credential: only their names are shown.
        f.debug_struct("Plan")
            .field(
                "set",
                &self.set.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .field("remove", &self.remove.len())
            .field(
                "config",
                &self.config.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            )
            .field("dir", &self.dir)
            .field("watched", &self.watched)
            .field("login", &self.login)
            .finish()
    }
}

impl Drop for StdinLogin {
    fn drop(&mut self) {
        self.secret.wipe();
    }
}

impl Drop for Plan {
    fn drop(&mut self) {
        for (_, value) in &mut self.set {
            value.wipe();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coqui_acp_api::guatiao::Value as GValue;

    const CANARY: &str = "sk-canary-9d2e";

    fn foreign(standards: &[&str], key: &str) -> AccountGrant {
        AccountGrant::new("a", "someone-elses-kind")
            .provides(standards.iter().copied())
            .credentials(coqui_acp_api::Credentials::from_iter([(key, CANARY)]))
            .state_dir("/some/dir")
    }

    fn settings(json: &Value) -> GValue {
        coqui_acp_harness::provider::schema(json)
    }

    #[test]
    fn every_kind_says_what_it_provides_and_every_standard_is_applied_by_a_listed_kind() {
        let catalog = catalog();
        for id in [CHATGPT, ACCESS_TOKEN, HARNESS_LOGIN] {
            assert_eq!(catalog.kind(id).expect(id).provides, [CODEX], "{id}");
        }
        assert_eq!(catalog.kind(API_KEY).expect("key").provides, [OPENAI_API]);
        let standards: Vec<&str> = catalog
            .accepts
            .iter()
            .map(|a| a.standard.as_str())
            .collect();
        assert_eq!(standards, [CODEX, OPENAI_API]);
        for slot in catalog.accepts.iter().flat_map(|a| &a.slots) {
            assert!(catalog.kind(&slot.kind).is_some(), "{}", slot.kind);
        }
    }

    #[test]
    fn an_openai_api_key_from_elsewhere_is_saved_by_login_into_codexs_home() {
        let grant = foreign(&[OPENAI_API], "api_key")
            .settings(settings(&json!({"base_url": "https://gateway.example/v1"})));
        let applied = plan(&grant).expect("accepted").expect("something to apply");
        assert!(
            applied
                .set
                .iter()
                .any(|(n, v)| n == "CODEX_HOME" && v == "/some/dir")
        );
        let login = applied.login.as_ref().expect("a login by standard input");
        assert_eq!(login.flag, "--with-api-key");
        assert_eq!(login.secret, CANARY);
        assert_eq!(
            applied.config,
            [(
                "openai_base_url".to_string(),
                json!("https://gateway.example/v1")
            )]
        );
        assert!(applied.remove.contains(&"OPENAI_API_KEY".to_string()));
    }

    #[test]
    fn a_codex_login_or_token_from_elsewhere_is_applied_as_codexs_own() {
        let token = plan(&foreign(&[CODEX], "access_token"))
            .expect("accepted")
            .expect("apply");
        assert_eq!(
            token.login.as_ref().expect("login").flag,
            "--with-access-token"
        );
        let file = plan(&foreign(&[CODEX], FILE_KEY))
            .expect("accepted")
            .expect("apply");
        assert_eq!(file.watched.len(), 1);
        assert_eq!(file.watched[0].path, AUTH_FILE);
    }

    #[test]
    fn an_account_of_a_standard_it_does_not_accept_is_refused_saying_which() {
        for standards in [&["anthropic-api"][..], &["claude-code"][..], &[][..]] {
            let grant = foreign(standards, "api_key");
            let error = plan(&grant).expect_err("not accepted");
            let said = error.message().to_string();
            assert!(
                said.contains("codex") && said.contains("openai-api"),
                "{said}"
            );
            assert!(!said.contains(CANARY), "{said}");
            assert!(check(&grant.reference()).is_err());
        }
        assert!(plan(&foreign(&[OPENAI_API], "access_token")).is_err());
    }
}
