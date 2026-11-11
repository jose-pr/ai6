//! The accounts Codex takes: the catalog the provider lists, and what applying a grant does to Codex's environment and
//! home directory.
//!
//! Measured on Codex 0.166.0. A `rotates` nobody measured is `true` (except where a kind says why not), a
//! `history_portable` nobody measured `false`.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use coqui_acp_api::guatiao::library::ProviderError;
use coqui_acp_api::guatiao::{Status, Wipe};
use coqui_acp_api::{
    ASK, AUTH, AccountCatalog, AccountGrant, AccountKindInfo, AccountRef, EndpointSlot,
    HARNESS_LOGIN, LoginMethodInfo, MODEL_API, OPEN_URL, USER_CODE,
};
use coqui_acp_harness::account::{Watched, jwt_expiry};
use coqui_acp_harness::provider::{credential, schema, to_json};
use serde_json::{Value, json};

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

/// What the provider takes as an account.
#[must_use]
pub fn catalog() -> AccountCatalog {
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
        "Runs `codex login`: a page to open, whose callback comes back to the machine where Codex runs.",
        CHATGPT,
    );
    web.steps = vec![OPEN_URL.to_string()];
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
    catalog.credential_variables = VARIABLES.iter().map(|v| (*v).to_string()).collect();
    catalog
}

fn bad(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, why)
}

/// The endpoint setting `key` of `settings`: absent, or a plain `http(s)` address.
fn endpoint(
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
pub(crate) struct Applied {
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
    account.check_id()?;
    let catalog = catalog();
    let Some(info) = catalog.kind(&account.kind) else {
        return Err(bad("this harness takes no account of that kind"));
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

/// What applying `grant` does; `None` for `harness-login`, which hands nothing over and clears nothing.
///
/// # Errors
///
/// `BAD_VALUE`: an id that is not one, a kind not listed, settings the kind refuses, a credential missing, empty or
/// one the kind does not take, a missing directory.
pub(crate) fn plan(grant: &AccountGrant) -> Result<Option<Applied>, ProviderError> {
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
        .filter(|value| !value.is_empty() && credentials.keys().all(|k| k == key))
        .ok_or_else(|| bad("the credential is missing or not the kind's"))?;
    let mut applied = Applied {
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
            applied.watched = vec![
                Watched::new(AUTH_FILE, FILE_KEY)
                    .usable(usable)
                    .expires(expires)
                    .identity(identity),
            ];
        }
        API_KEY => {
            applied.login = Some(StdinLogin {
                flag: "--with-api-key",
                key: "api_key",
                secret: secret.to_string(),
            });
        }
        _ => {
            applied.login = Some(StdinLogin {
                flag: "--with-access-token",
                key: "access_token",
                secret: secret.to_string(),
            });
        }
    }
    Ok(Some(applied))
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

impl core::fmt::Debug for Applied {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The variables set may hold a credential: only their names are shown.
        f.debug_struct("Applied")
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

impl Drop for Applied {
    fn drop(&mut self) {
        for (_, value) in &mut self.set {
            value.wipe();
        }
    }
}
