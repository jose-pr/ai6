//! The accounts Claude Code takes: the catalog the provider lists, and what applying a grant does to Claude Code's
//! environment and configuration directory.
//!
//! Measured on Claude Code 2.1.311. A `rotates` nobody measured is `true`, a `history_portable` nobody measured
//! `false`.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::ClaudeLaunch;
use std::ffi::OsString;

use coqui_acp_api::guatiao::library::ProviderError;
use coqui_acp_api::guatiao::{Secret, Status, Wipe};
use coqui_acp_api::standard::{ANTHROPIC_API, CLAUDE_CODE};
use coqui_acp_api::{
    ASK, AUTH, Accepted, AcceptedSlot, AccountCatalog, AccountGrant, AccountKindInfo, AccountOffer,
    AccountRef, Credentials, EndpointSlot, HARNESS_LOGIN, LoginMethodInfo, MODEL_API, OPEN_URL,
    PASTE_CODE, PROXY,
};
use coqui_acp_harness::account::{Accounts, Files, Hold, Watched, rfc3339_ms, same_name};
use coqui_acp_harness::provider::{credential, resolve, schema, to_json};
use serde_json::{Value, json};

/// The kind of a login made by `claude auth login --claudeai`, kept in `.credentials.json`.
pub const LOGIN: &str = "claude-login";
/// The kind of a login made by `claude auth login --console`.
pub const CONSOLE: &str = "claude-console";
/// The kind of a long-lived token in `CLAUDE_CODE_OAUTH_TOKEN`.
pub const OAUTH_TOKEN: &str = "claude-oauth-token";
/// The kind of a key in `ANTHROPIC_API_KEY`.
pub const API_KEY: &str = "anthropic-api-key";
/// The kind of a token in `ANTHROPIC_AUTH_TOKEN`, sent as a bearer token to a gateway.
pub const AUTH_TOKEN: &str = "anthropic-auth-token";

/// The login method that makes [`LOGIN`] by `claude auth login --claudeai`.
pub const SUBSCRIPTION: &str = "subscription";
/// The login method that makes [`CONSOLE`] by `claude auth login --console`.
pub const CONSOLE_METHOD: &str = "console";
/// The login method that takes a key and makes [`API_KEY`].
pub const API_KEY_METHOD: &str = "api-key";
/// The login method that takes a token and makes [`AUTH_TOKEN`].
pub const AUTH_TOKEN_METHOD: &str = "auth-token";
/// The login method `claude setup-token` would make, which this provider cannot run.
pub const LONG_LIVED_TOKEN: &str = "long-lived-token";

/// The file in Claude Code's configuration directory that holds a login.
pub const CREDENTIALS_FILE: &str = ".credentials.json";
/// The credential key that holds that file's text.
pub const FILE_KEY: &str = "credentials_file";

/// The model endpoint Claude Code uses without `ANTHROPIC_BASE_URL`.
const API: &str = "https://api.anthropic.com";

/// Every variable Claude Code reads for a credential, for where its requests go, or for which provider it signs in to,
/// so a start on an account removes them all and sets only the kind's.
///
/// Measured (`references/account_kinds.md`): the credential variables, `CLAUDE_CODE_USE_BEDROCK` and the six
/// `*_BASE_URL` endpoint variables of its documentation. Named but not run on 2.1.311: the other provider switches and
/// the cloud credentials (`AWS_*`, `GOOGLE_APPLICATION_CREDENTIALS`, the Vertex and Foundry settings).
pub(crate) const VARIABLES: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_AWS_API_KEY",
    "AWS_BEARER_TOKEN_BEDROCK",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
    "ANTHROPIC_FEDERATION_RULE_ID",
    "ANTHROPIC_ORGANIZATION_ID",
    "ANTHROPIC_PROFILE",
    "CLAUDE_CODE_CLIENT_KEY",
    "CLAUDE_CODE_CLIENT_KEY_PASSPHRASE",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_MANTLE",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AWS_BASE_URL",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_VERTEX_BASE_URL",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
    "ANTHROPIC_FOUNDRY_RESOURCE",
];

/// The part of [`VARIABLES`] whose value is itself a secret: a key, a token, a password or a private key. Not an endpoint,
/// a switch that picks a cloud provider, a project, a profile or the path of a file, which a person sets on purpose and
/// which a start still clears.
pub(crate) const SECRETS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_AWS_API_KEY",
    "AWS_BEARER_TOKEN_BEDROCK",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
    "CLAUDE_CODE_CLIENT_KEY",
    "CLAUDE_CODE_CLIENT_KEY_PASSPHRASE",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
];

fn settings(secret: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            secret: credential(),
            "base_url": {"type": "string", "format": "uri"},
        },
        "required": [secret],
        "additionalProperties": false,
    })
}

fn login_settings() -> Value {
    json!({
        "type": "object",
        "properties": {
            FILE_KEY: credential(),
            "base_url": {"type": "string", "format": "uri"},
        },
        "required": [FILE_KEY],
        "additionalProperties": false,
    })
}

fn kind(id: &str, title: &str, description: &str, settings: &Value) -> AccountKindInfo {
    let mut kind = AccountKindInfo::new(id, title);
    kind.description = description.to_string();
    kind.settings = schema(settings);
    kind
}

fn login(id: &str, title: &str, description: &str, auth: &str) -> AccountKindInfo {
    let mut kind = kind(id, title, description, &login_settings());
    kind.endpoints = vec![
        EndpointSlot::new(MODEL_API)
            .key("base_url")
            .default_url(API),
        EndpointSlot::new(AUTH).default_url(auth),
    ];
    kind.rotates = true;
    kind.state_dir = true;
    kind.renews = true;
    kind.notices =
        vec!["the login is kept in a file that what the agent runs can read".to_string()];
    kind
}

fn environment(
    id: &str,
    title: &str,
    description: &str,
    secret: &str,
    purpose: &str,
) -> AccountKindInfo {
    let mut kind = kind(id, title, description, &settings(secret));
    let mut slot = EndpointSlot::new(purpose).key("base_url");
    if purpose == MODEL_API {
        slot = slot.default_url(API);
    }
    kind.endpoints = vec![slot];
    kind.history_portable = true;
    kind.notices = vec![
        "the credential is in the environment of every program the agent runs".to_string(),
        "the person's own user settings (apiKeyHelper, an env block) are not loaded, as a stored login in their configuration directory could outrank the credential; the entry's setting_sources says otherwise".to_string(),
    ];
    kind
}

fn method(id: &str, title: &str, description: &str, produces: &str) -> LoginMethodInfo {
    let mut method = LoginMethodInfo::new(id, title, produces);
    method.description = description.to_string();
    method
}

fn web_method(id: &str, title: &str, description: &str, produces: &str) -> LoginMethodInfo {
    let mut method = method(id, title, description, produces);
    method.inputs = schema(&json!({
        "type": "object",
        "properties": {"email": {"type": "string", "description": "An address to fill in on the sign-in page."}},
        "additionalProperties": false,
    }));
    method.steps = vec![OPEN_URL.to_string(), PASTE_CODE.to_string()];
    // The page shows the code to copy back, so the person's browser need not be on the machine where Claude Code runs.
    method.remote = true;
    method
}

fn typed_method(id: &str, title: &str, description: &str, produces: &str) -> LoginMethodInfo {
    let mut method = method(id, title, description, produces);
    method.steps = vec![ASK.to_string()];
    method.remote = true;
    method
}

/// Every kind and sign-in method Claude Code's accounts have, as the account provider lists them and the harness
/// takes them.
fn full() -> AccountCatalog {
    let mut catalog = AccountCatalog::new();
    catalog.kinds.extend([
        login(
            LOGIN,
            "Claude subscription login",
            "A login made by `claude auth login --claudeai`, kept in `.credentials.json` of Claude Code's configuration directory.",
            "https://claude.com/cai/oauth/authorize",
        ),
        login(
            CONSOLE,
            "Claude Console login",
            "A login made by `claude auth login --console`, billed to an API organization.",
            "https://platform.claude.com/oauth/authorize",
        ),
        environment(
            OAUTH_TOKEN,
            "Claude long-lived token",
            "A token made by `claude setup-token`, read from `CLAUDE_CODE_OAUTH_TOKEN`.",
            "token",
            MODEL_API,
        ),
        environment(
            API_KEY,
            "Anthropic API key",
            "A key read from `ANTHROPIC_API_KEY`; it takes the place of a subscription login.",
            "api_key",
            MODEL_API,
        ),
        environment(
            AUTH_TOKEN,
            "Anthropic gateway token",
            "A token read from `ANTHROPIC_AUTH_TOKEN` and sent as a bearer token, to the gateway `base_url` names.",
            "auth_token",
            PROXY,
        ),
    ]);
    catalog.logins = vec![
        web_method(
            SUBSCRIPTION,
            "Sign in with a Claude subscription",
            "Runs `claude auth login --claudeai`: a page to open, and the code it shows to paste back.",
            LOGIN,
        ),
        web_method(
            CONSOLE_METHOD,
            "Sign in to the Anthropic Console",
            "Runs `claude auth login --console`: a page to open, and the code it shows to paste back.",
            CONSOLE,
        ),
        typed_method(
            API_KEY_METHOD,
            "Use an Anthropic API key",
            "Takes a key and checks with `claude auth status` that Claude Code accepts it; no model is called.",
            API_KEY,
        ),
        typed_method(
            AUTH_TOKEN_METHOD,
            "Use a gateway token",
            "Takes a bearer token and checks with `claude auth status` that Claude Code accepts it; no model is called.",
            AUTH_TOKEN,
        ),
    ];
    for kind in &mut catalog.kinds {
        let standard = match kind.id.as_str() {
            API_KEY | AUTH_TOKEN => ANTHROPIC_API,
            _ => CLAUDE_CODE,
        };
        kind.provides = vec![standard.to_string()];
    }
    catalog.accepts = accepts();
    catalog.credential_variables = VARIABLES
        .iter()
        .map(|v| (*v).to_string())
        .collect::<Vec<String>>();
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
    offer.usage = [LOGIN, OAUTH_TOKEN, API_KEY, AUTH_TOKEN]
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

/// The standards Claude Code starts on, and how each is applied: a Claude Code login (a configuration directory
/// holding `.credentials.json`, or a long-lived token in `CLAUDE_CODE_OAUTH_TOKEN`), or the Anthropic API (a key in
/// `ANTHROPIC_API_KEY` or a bearer token in `ANTHROPIC_AUTH_TOKEN`), whose endpoint `ANTHROPIC_BASE_URL` moves.
fn accepts() -> Vec<Accepted> {
    let url = |slot: AcceptedSlot| slot.base_url("base_url", Some("ANTHROPIC_BASE_URL"));
    vec![
        Accepted::new(CLAUDE_CODE)
            .slot(url(AcceptedSlot::new(FILE_KEY, LOGIN)
                .file(CREDENTIALS_FILE)
                .dir_variable("CLAUDE_CONFIG_DIR")))
            .slot(url(
                AcceptedSlot::new("token", OAUTH_TOKEN).variable("CLAUDE_CODE_OAUTH_TOKEN")
            )),
        Accepted::new(ANTHROPIC_API)
            .slot(url(
                AcceptedSlot::new("api_key", API_KEY).variable("ANTHROPIC_API_KEY")
            ))
            .slot(url(
                AcceptedSlot::new("auth_token", AUTH_TOKEN).variable("ANTHROPIC_AUTH_TOKEN")
            )),
    ]
}

fn bad(why: &str) -> ProviderError {
    ProviderError::new(Status::GUATIAO_ERR_BAD_VALUE, why)
}

/// The text setting `key` of `settings`: absent, or a plain `http(s)` address.
pub(crate) fn base_url(
    settings: &coqui_acp_api::guatiao::Value,
) -> Result<Option<String>, ProviderError> {
    let settings = guatiao_json(settings);
    let Some(object) = settings.as_object() else {
        return Err(bad("the account's settings are not an object"));
    };
    if object.keys().any(|key| key != "base_url") {
        return Err(bad("the account's settings name something the kind lacks"));
    }
    match object.get("base_url") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(url))
            if (url.starts_with("https://") || url.starts_with("http://"))
                && url.len() > 8
                && !url.chars().any(|c| c.is_control() || c.is_whitespace()) =>
        {
            Ok(Some(url.clone()))
        }
        Some(_) => Err(bad("base_url is not an address")),
    }
}

fn guatiao_json(value: &coqui_acp_api::guatiao::Value) -> Value {
    to_json(value)
}

/// What applying a grant does.
#[non_exhaustive]
pub(crate) struct Plan {
    /// The variables Claude Code is given.
    pub set: Vec<(String, String)>,
    /// The variables removed before, every credential variable; empty for `harness-login`, which clears nothing.
    pub remove: Vec<String>,
    /// The configuration directory, for a kind that keeps its login in one.
    pub dir: Option<String>,
    /// The files that hold the login.
    pub watched: Vec<Watched>,
}

/// The file's text holds a login: not blank, and when it is Claude Code's JSON, a token in it.
pub(crate) fn usable(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(object)) => object
            .get("claudeAiOauth")
            .and_then(Value::as_object)
            .is_some_and(|oauth| {
                ["accessToken", "refreshToken"].iter().any(|key| {
                    oauth
                        .get(*key)
                        .and_then(Value::as_str)
                        .is_some_and(|token| !token.is_empty())
                })
            }),
        Ok(_) => false,
        // Not the JSON Claude Code writes: whatever it is, it is not blank.
        Err(_) => true,
    }
}

/// When the login in the file's text stops working, as an RFC 3339 time.
pub(crate) fn expires(text: &str) -> Option<String> {
    let millis = expires_ms(text)?;
    Some(rfc3339_ms(millis))
}

/// When it stops working, in milliseconds since the Unix epoch.
pub(crate) fn expires_ms(text: &str) -> Option<i64> {
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("claudeAiOauth")?
        .get("expiresAt")?
        .as_i64()
}

/// Why a reference to an account is not one the provider takes: `BAD_VALUE` for an id, kind or settings it refuses.
pub(crate) fn check(account: &AccountRef) -> Result<(), ProviderError> {
    let catalog = catalog();
    catalog.accepts_reference(account)?;
    let Some(info) = catalog.kind(&account.kind) else {
        // An account of an accepted standard: `plan` reads its credential and settings.
        return Ok(());
    };
    if info.id == HARNESS_LOGIN {
        if !guatiao_json(&account.settings)
            .as_object()
            .is_some_and(serde_json::Map::is_empty)
        {
            return Err(bad("that kind takes no settings"));
        }
        return Ok(());
    }
    base_url(&account.settings)?;
    if info.state_dir && account.state_dir.as_deref().is_none_or(str::is_empty) {
        return Err(bad("that kind needs a directory to run in"));
    }
    Ok(())
}

/// The variables applying `grant` sets in Claude Code's environment that hold no credential, by name: its configuration
/// directory (`CLAUDE_CONFIG_DIR`, for a kind that keeps a login there, when the grant has a directory) and the endpoint
/// (`ANTHROPIC_BASE_URL`, when the account has one). A host reads them to tell its profile's layers what an account
/// sets before a session starts; the credentials' own variables are never listed. A grant this provider cannot apply
/// lists none.
#[must_use]
pub fn variables(grant: &AccountGrant) -> Vec<(String, String)> {
    let Ok(grant) = resolved(grant) else {
        return Vec::new();
    };
    let mut set = Vec::new();
    if matches!(grant.kind.as_str(), LOGIN | CONSOLE)
        && let Some(dir) = grant.state_dir.as_deref().filter(|dir| !dir.is_empty())
    {
        set.push(("CLAUDE_CONFIG_DIR".to_string(), dir.to_string()));
    }
    if grant.kind != HARNESS_LOGIN
        && let Ok(Some(url)) = base_url(&grant.settings)
    {
        set.push(("ANTHROPIC_BASE_URL".to_string(), url));
    }
    set
}

/// What applying `grant` does.
///
/// # Errors
///
/// `BAD_VALUE`: an id that is not one, a kind not listed, settings the kind refuses, a credential missing, empty or
/// one the kind does not take, a missing directory.
pub(crate) fn plan(grant: &AccountGrant) -> Result<Plan, ProviderError> {
    plan_as(grant, false)
}

/// [`plan`], where a login kind with no file in the grant (`in_place`) is the directory as it is.
fn plan_as(grant: &AccountGrant, in_place: bool) -> Result<Plan, ProviderError> {
    let grant = &resolved(grant)?;
    check(&grant.reference())?;
    let credentials: &Credentials = grant.credentials.expose();
    if grant.kind == HARNESS_LOGIN {
        if !credentials.is_empty() {
            return Err(bad("that kind takes no credential"));
        }
        return Ok(Plan {
            set: Vec::new(),
            remove: Vec::new(),
            dir: None,
            watched: Vec::new(),
        });
    }
    let url = base_url(&grant.settings)?;
    let mut set = Vec::new();
    let mut applied = Plan {
        set: Vec::new(),
        remove: VARIABLES.iter().map(|v| (*v).to_string()).collect(),
        dir: None,
        watched: Vec::new(),
    };
    if matches!(grant.kind.as_str(), LOGIN | CONSOLE) {
        let only = credentials.keys().all(|key| key == FILE_KEY);
        let text = credentials.get(FILE_KEY).filter(|text| !text.is_empty());
        let there = in_place && credentials.is_empty();
        if !there && (!only || text.is_none()) {
            return Err(bad("the login's credentials are missing or not the kind's"));
        }
        let dir = grant.state_dir.clone().unwrap_or_default();
        set.push(("CLAUDE_CONFIG_DIR".to_string(), dir.clone()));
        applied.dir = Some(dir);
        if !there {
            applied.watched = vec![
                Watched::new(CREDENTIALS_FILE, FILE_KEY)
                    .usable(usable)
                    .expires(expires),
            ];
        }
    } else {
        let key = match grant.kind.as_str() {
            OAUTH_TOKEN => "token",
            API_KEY => "api_key",
            _ => "auth_token",
        };
        let variable = secret_key_variable(&grant.kind);
        let only = credentials.keys().all(|k| k == key);
        let value = credentials.get(key).filter(|value| !value.is_empty());
        let Some(value) = value.filter(|_| only) else {
            return Err(bad("the credential is missing or not the kind's"));
        };
        set.push((variable.to_string(), value.to_string()));
    }
    if let Some(url) = url {
        set.push(("ANTHROPIC_BASE_URL".to_string(), url));
    }
    applied.set = set;
    Ok(applied)
}

/// What is left to do for an account [`apply`] has applied to a launch: the files of a login the account keeps.
///
/// It holds no credential a log or a `Debug` would show.
#[must_use = "a login kept in files is placed only by `hold`"]
pub struct Applied {
    id: String,
    revision: u64,
    credentials: Secret<Credentials>,
    dir: Option<String>,
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
    /// Claude Code's configuration directory, for a kind that keeps a login in one.
    #[must_use]
    pub fn dir(&self) -> Option<&str> {
        self.dir.as_deref()
    }

    /// Whether the account keeps a login in files that [`hold`](Self::hold) places and watches.
    #[must_use]
    pub fn keeps_files(&self) -> bool {
        !self.watched.is_empty()
    }

    /// Places the files of a login in the account's directory through `files` and holds them in `accounts`, which sees
    /// to it that the grant's copy replaces only one that is blank, older or not a file, and tells a renewal or a loss
    /// to the keepers of a live hold. `None` for an account that keeps no file: nothing is written. The hold lasts
    /// until [`Hold::finish`] or its drop; keep it as long as Claude Code runs.
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
                self.dir.as_deref().unwrap_or_default(),
                self.revision,
                self.credentials.expose(),
                self.watched,
            )
            .map(Some)
    }
}

/// Applies `grant` to `launch`, as `start` does: every credential variable is removed from what Claude Code starts
/// with, including those of `launch.env`, then the kind's is set (`CLAUDE_CONFIG_DIR` the grant's `state_dir` for a
/// login kind, `--setting-sources=project,local` for an environment kind unless `launch` already says which sources to
/// load). `harness-login` changes nothing.
///
/// A login kind whose grant carries no `credentials_file` and a `state_dir` is applied **in place**: Claude Code runs on
/// that directory as it is, nothing is written and [`Applied::hold`] places nothing. A login is placed only when the
/// grant carries its copy, and only by `hold`. `files` is used only to check that the spawner serves files when a login
/// is to be placed.
///
/// The endpoint variables are cleared and set only from the grant's `base_url`: a host that keeps a person's own
/// choice of endpoint (`ANTHROPIC_BASE_URL`, a cloud switch) puts it back afterwards. They are among the catalog's
/// `credential_variables` and not among its `secret_variables`.
///
/// # Errors
///
/// `BAD_VALUE`: an id that is not one, a kind not listed, settings the kind refuses, a credential missing or not the
/// kind's, a login kind with no `state_dir`; `UNSUPPORTED`: a login to place on a spawner with no files.
pub fn apply(
    grant: &AccountGrant,
    launch: &mut ClaudeLaunch,
    files: &Files,
) -> Result<Applied, ProviderError> {
    apply_as(grant, launch, files, true)
}

/// [`apply`] for `start` and the other calls of the harness, which place every login the grant carries and refuse one
/// that carries none (`in_place` false).
pub(crate) fn apply_as(
    grant: &AccountGrant,
    launch: &mut ClaudeLaunch,
    files: &Files,
    in_place: bool,
) -> Result<Applied, ProviderError> {
    let grant = &resolved(grant)?;
    let mut plan = plan_as(grant, in_place)?;
    let applied = |plan: &mut Plan| Applied {
        id: grant.id.clone(),
        revision: grant.revision,
        credentials: Secret::new(grant.credentials.expose().clone()),
        dir: plan.dir.take(),
        watched: core::mem::take(&mut plan.watched),
    };
    if grant.kind == HARNESS_LOGIN {
        return Ok(applied(&mut plan));
    }
    if !plan.watched.is_empty() && !files.available() {
        return Err(ProviderError::new(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this spawner serves no private files, which the login is kept in",
        ));
    }
    // What the host's environment holds under a credential name does not reach Claude Code beside the account's.
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
    if plan.dir.is_none() && launch.setting_sources.is_none() {
        // The person's own user settings (an apiKeyHelper, an env block) would outrank the credential: they are left
        // out unless the launch says which sources to load.
        launch.setting_sources = Some(vec!["project".to_string(), "local".to_string()]);
    }
    Ok(applied(&mut plan))
}

/// The grant of Claude Code's own kind that applies `grant`, when `grant` is an account of an accepted standard.
pub(crate) fn resolved(grant: &AccountGrant) -> Result<AccountGrant, ProviderError> {
    resolve(&catalog(), grant)
}

fn secret_key_variable(kind: &str) -> &'static str {
    match kind {
        OAUTH_TOKEN => "CLAUDE_CODE_OAUTH_TOKEN",
        API_KEY => "ANTHROPIC_API_KEY",
        _ => "ANTHROPIC_AUTH_TOKEN",
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
            .field("dir", &self.dir)
            .field("watched", &self.watched)
            .finish()
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

    const CANARY: &str = "sk-canary-61c0";

    fn foreign(standards: &[&str], key: &str) -> AccountGrant {
        AccountGrant::new("a", "someone-elses-kind")
            .provides(standards.iter().copied())
            .credentials(Credentials::from_iter([(key, CANARY)]))
    }

    fn settings(json: &Value) -> GValue {
        coqui_acp_harness::provider::schema(json)
    }

    fn set_of(applied: &Plan, name: &str) -> Option<String> {
        applied
            .set
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    }

    #[test]
    fn every_kind_says_what_it_provides_and_every_standard_is_applied_by_a_listed_kind() {
        let catalog = catalog();
        for id in [LOGIN, CONSOLE, OAUTH_TOKEN, HARNESS_LOGIN] {
            assert_eq!(catalog.kind(id).expect(id).provides, [CLAUDE_CODE], "{id}");
        }
        for id in [API_KEY, AUTH_TOKEN] {
            assert_eq!(
                catalog.kind(id).expect(id).provides,
                [ANTHROPIC_API],
                "{id}"
            );
        }
        let standards: Vec<&str> = catalog
            .accepts
            .iter()
            .map(|a| a.standard.as_str())
            .collect();
        assert_eq!(standards, [CLAUDE_CODE, ANTHROPIC_API]);
        for slot in catalog.accepts.iter().flat_map(|a| &a.slots) {
            assert!(catalog.kind(&slot.kind).is_some(), "{}", slot.kind);
        }
    }

    #[test]
    fn an_account_of_anthropic_api_from_elsewhere_sets_the_key_and_the_endpoint() {
        let grant = foreign(&[ANTHROPIC_API], "api_key")
            .settings(settings(&json!({"base_url": "https://gateway.example/v1"})));
        let applied = plan(&grant).expect("accepted");
        assert_eq!(
            set_of(&applied, "ANTHROPIC_API_KEY").as_deref(),
            Some(CANARY)
        );
        assert_eq!(
            set_of(&applied, "ANTHROPIC_BASE_URL").as_deref(),
            Some("https://gateway.example/v1")
        );
        assert!(applied.remove.contains(&"ANTHROPIC_AUTH_TOKEN".to_string()));
        assert!(applied.dir.is_none());
        let bearer =
            plan(&foreign(&[ANTHROPIC_API, "openai-api"], "auth_token")).expect("accepted");
        assert_eq!(
            set_of(&bearer, "ANTHROPIC_AUTH_TOKEN").as_deref(),
            Some(CANARY)
        );
        assert!(set_of(&bearer, "ANTHROPIC_API_KEY").is_none());
    }

    #[test]
    fn an_account_of_claude_code_from_elsewhere_is_a_token_or_a_login_directory() {
        let token = plan(&foreign(&[CLAUDE_CODE], "token")).expect("accepted");
        assert_eq!(
            set_of(&token, "CLAUDE_CODE_OAUTH_TOKEN").as_deref(),
            Some(CANARY)
        );
        let text = json!({"claudeAiOauth": {"accessToken": "t", "refreshToken": "r"}}).to_string();
        let login = AccountGrant::new("a", "someone-elses-kind")
            .provides([CLAUDE_CODE])
            .credentials(Credentials::from_iter([(FILE_KEY, text.as_str())]))
            .state_dir("/some/dir");
        let applied = plan(&login).expect("accepted");
        assert_eq!(
            set_of(&applied, "CLAUDE_CONFIG_DIR").as_deref(),
            Some("/some/dir")
        );
        assert_eq!(applied.watched.len(), 1);
        assert_eq!(applied.watched[0].path, CREDENTIALS_FILE);
    }

    #[test]
    fn an_account_of_a_standard_it_does_not_accept_is_refused_saying_which() {
        for standards in [&["openai-api"][..], &["codex", "xai-api"][..], &[][..]] {
            let grant = foreign(standards, "api_key");
            let error = plan(&grant).expect_err("not accepted");
            assert_eq!(error.status, Status::GUATIAO_ERR_BAD_VALUE);
            let said = error.message().to_string();
            assert!(
                said.contains("claude-code") && said.contains("anthropic-api"),
                "{said}"
            );
            assert!(!said.contains(CANARY), "{said}");
            assert!(check(&grant.reference()).is_err());
        }
    }

    #[test]
    fn an_account_of_an_accepted_standard_with_the_wrong_credential_or_setting_is_refused() {
        assert!(plan(&foreign(&[ANTHROPIC_API], "token")).is_err());
        assert!(plan(&foreign(&[CLAUDE_CODE], "api_key")).is_err());
        let loose = foreign(&[ANTHROPIC_API], "api_key").settings(settings(&json!({"proxy": "x"})));
        assert!(plan(&loose).is_err());
    }
}
