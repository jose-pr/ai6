//! Antigravity's account, plan and quota read from its stored login, with no `agy` process.
//!
//! agy keeps its Google sign-in in the operating system's keyring (service `gemini`, user `antigravity`) or, where the keyring
//! cannot be used, in a file under `~/.gemini`. [`AgyLogin`] reads it, refreshes an expired access token in memory (never
//! writing it back), and asks Google's Cloud Code service what agy asks: `loadCodeAssist` for the plan and
//! `retrieveUserQuotaSummary` for the groups of models and each group's weekly and five-hour windows, as `agy`'s `/usage`
//! lists them. Where the service gives no summary, `fetchAvailableModels` gives each model's remaining fraction and reset
//! time, as buckets of the kind [`super::AgyQuota`] makes from a session's own status.
//!
//! The OAuth client a refresh needs is Antigravity's, read from its installed program at run time or given by the caller.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::time::Duration;
use std::io;
use std::path::PathBuf;

use coqui_acp_api::usage::Bucket;
use serde::Deserialize;
use serde_json::{Value, json};

use super::AcpModel;
use super::status::{self, AgyAccount};

mod keyring;
mod program;
mod stored;

pub use stored::Secret;

/// How long the models the service listed are kept, in seconds.
const LISTED_FOR: i64 = 600;

/// The keyring service agy keeps its login under.
pub const KEYRING_SERVICE: &str = "gemini";
/// The keyring user agy keeps its login under.
pub const KEYRING_USER: &str = "antigravity";
/// The variable agy reads its Cloud Code address from, which overrides the one its sign-in picks.
pub const SERVICE_VARIABLE: &str = "CLOUD_CODE_URL";

/// Google's token endpoint, which refreshes an access token.
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Google's userinfo endpoint: the account's email and name.
pub const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
/// The Cloud Code service for a personal Google account (`auth_method` `consumer`, and any other but `gcp`).
pub const CONSUMER_SERVICE: &str = "https://daily-cloudcode-pa.googleapis.com";
/// The Cloud Code service for a Google Cloud sign-in (`auth_method` `gcp`).
pub const GCP_SERVICE: &str = "https://cloudcode-pa.googleapis.com";

/// The `User-Agent` of agy's command line. The service lists the groups of models by the client that asks: a client that does not
/// name itself `antigravity/cli/<version>` is given only the groups agy's older clients know.
pub const USER_AGENT: &str = "antigravity/cli/1.5.15";

/// An access token this close to its expiry counts as expired, as Go's `oauth2` package counts it.
const EXPIRY_MARGIN: i64 = 10;

/// A function that answers for the keyring: given the service and the user, the stored bytes, `None` for no entry.
pub type KeyringLookup = dyn Fn(&str, &str) -> io::Result<Option<Vec<u8>>> + Send + Sync;

/// A call to a service with an access token.
type Call<'a> = dyn Fn(&ureq::Agent, &str) -> Result<Vec<u8>, LoginError> + 'a;

/// Where the login is looked for in the operating system's keyring.
#[derive(Clone, Default)]
#[non_exhaustive]
pub enum Keyring {
    /// The operating system's own: Windows' Credential Manager (target `gemini:antigravity`), the macOS Keychain, or the
    /// Secret Service.
    #[default]
    Os,
    /// Not looked for: only the files are read.
    Off,
    /// A function that answers for the keyring: given the service and the user, the stored bytes, `None` for no entry.
    Given(Arc<KeyringLookup>),
}

impl fmt::Debug for Keyring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Os => "Os",
            Self::Off => "Off",
            Self::Given(_) => "Given",
        })
    }
}

/// Where the OAuth client of a refresh comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClientSource {
    /// Antigravity's installed program, found as `agy` on `PATH` (and on Windows at `%LOCALAPPDATA%\agy\bin\agy.exe`).
    #[default]
    Find,
    /// Antigravity's program at this path.
    Program(PathBuf),
    /// This client.
    Given(OAuthClient),
    /// None: an expired login is not refreshed.
    Off,
}

/// An OAuth client: its id and secret. Its `Debug` shows neither.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OAuthClient {
    /// The client id (`…apps.googleusercontent.com`).
    pub id: String,
    /// The client secret.
    pub secret: Secret,
}

impl OAuthClient {
    /// A client with `id` and `secret`.
    #[must_use]
    pub fn new(id: impl Into<String>, secret: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            secret: Secret::new(secret),
        }
    }
}

impl fmt::Debug for OAuthClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OAuthClient(..)")
    }
}

/// Where and how a reading goes. Every address is a setting and every limit has a default and no floor.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct LoginSettings {
    /// The home whose `.gemini` folder holds the login files. Default: `USERPROFILE` on Windows, else `HOME`.
    pub home: Option<PathBuf>,
    /// The `.gemini` folder itself, which replaces `<home>/.gemini`.
    pub gemini_dir: Option<PathBuf>,
    /// Where the keyring is read. Default the operating system's.
    pub keyring: Keyring,
    /// How long the keyring may take to answer before the files are read instead. Default 5 s.
    pub keyring_timeout: Duration,
    /// Where a refresh's OAuth client comes from. Default Antigravity's program, found.
    pub client: ClientSource,
    /// The token endpoint. Default [`TOKEN_URL`].
    pub token_url: String,
    /// The userinfo endpoint. Default [`USERINFO_URL`].
    pub userinfo_url: String,
    /// The Cloud Code service, whatever the sign-in. Default none: `CLOUD_CODE_URL` when it is set, else the sign-in's.
    pub service: Option<String>,
    /// The Cloud Code service of a personal account. Default [`CONSUMER_SERVICE`].
    pub consumer_service: String,
    /// The Cloud Code service of a Google Cloud sign-in. Default [`GCP_SERVICE`].
    pub gcp_service: String,
    /// The `User-Agent` of each request. Default [`USER_AGENT`].
    pub user_agent: String,
    /// How long each request may take, connection included. Default 15 s.
    pub timeout: Duration,
    /// The most of an answer's body that is read, in bytes; an answer past it is malformed. Default 4 MiB.
    pub body: usize,
}

impl Default for LoginSettings {
    fn default() -> Self {
        Self {
            home: None,
            gemini_dir: None,
            keyring: Keyring::Os,
            keyring_timeout: Duration::from_secs(5),
            client: ClientSource::Find,
            token_url: TOKEN_URL.into(),
            userinfo_url: USERINFO_URL.into(),
            service: None,
            consumer_service: CONSUMER_SERVICE.into(),
            gcp_service: GCP_SERVICE.into(),
            user_agent: USER_AGENT.into(),
            timeout: Duration::from_secs(15),
            body: 4 * 1024 * 1024,
        }
    }
}

impl LoginSettings {
    /// The defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the home whose `.gemini` folder is read.
    #[must_use]
    pub fn home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    /// Sets the `.gemini` folder.
    #[must_use]
    pub fn gemini_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.gemini_dir = Some(dir.into());
        self
    }

    /// Sets where the keyring is read.
    #[must_use]
    pub fn keyring(mut self, keyring: Keyring) -> Self {
        self.keyring = keyring;
        self
    }

    /// Sets how long the keyring may take.
    #[must_use]
    pub const fn keyring_timeout(mut self, limit: Duration) -> Self {
        self.keyring_timeout = limit;
        self
    }

    /// Sets where a refresh's OAuth client comes from.
    #[must_use]
    pub fn client(mut self, client: ClientSource) -> Self {
        self.client = client;
        self
    }

    /// Sets the token endpoint.
    #[must_use]
    pub fn token_url(mut self, url: impl Into<String>) -> Self {
        self.token_url = url.into();
        self
    }

    /// Sets the userinfo endpoint.
    #[must_use]
    pub fn userinfo_url(mut self, url: impl Into<String>) -> Self {
        self.userinfo_url = url.into();
        self
    }

    /// Sets the Cloud Code service, whatever the sign-in.
    #[must_use]
    pub fn service(mut self, url: impl Into<String>) -> Self {
        self.service = Some(url.into());
        self
    }

    /// Sets the Cloud Code service of a personal account.
    #[must_use]
    pub fn consumer_service(mut self, url: impl Into<String>) -> Self {
        self.consumer_service = url.into();
        self
    }

    /// Sets the Cloud Code service of a Google Cloud sign-in.
    #[must_use]
    pub fn gcp_service(mut self, url: impl Into<String>) -> Self {
        self.gcp_service = url.into();
        self
    }

    /// Sets the `User-Agent`.
    #[must_use]
    pub fn user_agent(mut self, agent: impl Into<String>) -> Self {
        self.user_agent = agent.into();
        self
    }

    /// Sets how long each request may take.
    #[must_use]
    pub const fn timeout(mut self, limit: Duration) -> Self {
        self.timeout = limit;
        self
    }

    /// Sets the most of a body that is read.
    #[must_use]
    pub const fn body(mut self, bytes: usize) -> Self {
        self.body = bytes;
        self
    }

    /// The `.gemini` folder: the one set, else the home's.
    fn gemini(&self) -> Option<PathBuf> {
        if let Some(dir) = &self.gemini_dir {
            return Some(dir.clone());
        }
        let home = self.home.clone().or_else(|| {
            let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
            std::env::var_os(variable)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })?;
        Some(home.join(".gemini"))
    }
}

/// The step of a reading that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Step {
    /// Reading the operating system's keyring.
    Keyring,
    /// Reading a login file, or the login's JSON.
    LoginFile,
    /// Reading Antigravity's program for its OAuth client.
    Program,
    /// Refreshing the access token.
    Refresh,
    /// Asking userinfo for the email and name.
    UserInfo,
    /// Asking `loadCodeAssist` for the plan.
    LoadCodeAssist,
    /// Asking `retrieveUserQuotaSummary` for the groups of models and their windows.
    QuotaSummary,
    /// Asking `fetchAvailableModels` for each model's quota.
    FetchAvailableModels,
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Keyring => "reading the keyring",
            Self::LoginFile => "reading the login file",
            Self::Program => "reading Antigravity's program",
            Self::Refresh => "refreshing the login",
            Self::UserInfo => "userinfo",
            Self::LoadCodeAssist => "loadCodeAssist",
            Self::QuotaSummary => "retrieveUserQuotaSummary",
            Self::FetchAvailableModels => "fetchAvailableModels",
        })
    }
}

/// Why a step failed. None of them holds a token, an email or any of an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cause {
    /// The access token has expired and there is no OAuth client to refresh it with: Antigravity has to refresh its own
    /// login (it does when it next runs), or the caller gives the client.
    NoClient,
    /// Every OAuth client tried was refused: the login may have been revoked, so Antigravity has to sign in again.
    Refused,
    /// An answer with a status other than 200.
    Status(u16),
    /// An answer, a login or a stored value that is not what it should be, or a body past the limit.
    Malformed,
    /// Nothing could be read or reached in time.
    Io(io::ErrorKind),
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoClient => f.write_str(
                "the login has expired and no OAuth client is known to refresh it; Antigravity refreshes it when it next runs",
            ),
            Self::Refused => f.write_str("the login was refused; sign in to Antigravity again"),
            Self::Status(code) => write!(f, "status {code}"),
            Self::Malformed => f.write_str("the answer is not what was expected"),
            Self::Io(kind) => write!(f, "{kind}"),
        }
    }
}

/// A failed step and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LoginError {
    /// The step.
    pub step: Step,
    /// Why.
    pub cause: Cause,
}

impl LoginError {
    /// `step` failed for `cause`.
    #[must_use]
    pub const fn new(step: Step, cause: Cause) -> Self {
        Self { step, cause }
    }
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Antigravity's account: {} failed: {}",
            self.step, self.cause
        )
    }
}

impl core::error::Error for LoginError {}

/// What a reading found.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoginReading {
    /// No login is stored, or the one stored is not a whole sign-in.
    NotSignedIn,
    /// A login is stored.
    SignedIn(SignedIn),
}

/// A stored login's account and usage.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SignedIn {
    /// Who the account is: the stored email and plan, and what the service says when it was reached.
    pub account: AgyAccount,
    /// The quota's buckets, or the step that kept them from being read.
    pub usage: Result<Vec<Bucket>, LoginError>,
}

/// A token refreshed in this process, for the stored refresh token it came from.
struct Refreshed {
    from: Secret,
    access: Secret,
    expires_at: Option<i64>,
}

/// The OAuth clients read from a program, for the file as it was then.
struct ProgramClients {
    key: program::FileKey,
    clients: Vec<OAuthClient>,
}

/// Reads Antigravity's account and quota from its stored login. One reader keeps what it learns between readings: a refreshed
/// access token (in memory only), the OAuth client that refreshed it, and each bucket's window length.
pub struct AgyLogin {
    settings: LoginSettings,
    refreshed: Option<Refreshed>,
    program: Option<ProgramClients>,
    working: BTreeMap<String, OAuthClient>,
    windows: status::Windows,
    unmatched: u64,
    /// The models the service listed when no session gave any, and when (Unix seconds).
    listed: Option<(i64, Vec<AcpModel>)>,
}

impl fmt::Debug for AgyLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgyLogin")
            .field("settings", &self.settings)
            .field("refreshed", &self.refreshed.is_some())
            .field("unmatched", &self.unmatched)
            .finish_non_exhaustive()
    }
}

/// `loadCodeAssist`'s answer: the tiers and the project.
#[derive(Deserialize)]
struct Assist {
    #[serde(rename = "paidTier")]
    paid: Option<Tier>,
    #[serde(rename = "currentTier")]
    current: Option<Tier>,
    #[serde(rename = "cloudaicompanionProject")]
    project: Option<Value>,
}

#[derive(Deserialize)]
struct Tier {
    name: Option<String>,
}

/// userinfo's answer.
#[derive(Deserialize)]
struct UserInfo {
    email: Option<String>,
    name: Option<String>,
}

/// The token endpoint's answer.
#[derive(Deserialize)]
struct TokenAnswer {
    access_token: Option<String>,
    expires_in: Option<i64>,
}

/// Whether a whole sign-in is stored, in the keyring or in a file, with no reading of its account or its usage and no
/// request: the check a host makes before it lists the account. A keyring that does not answer within
/// [`LoginSettings::keyring_timeout`] and no file to fall back on is the error; an expired login still counts.
///
/// # Errors
///
/// The step that could not be read, when nothing else was found.
pub fn has_stored_login(settings: &LoginSettings) -> Result<bool, LoginError> {
    Ok(stored::load(settings)?.is_some())
}

impl AgyLogin {
    /// A reader with `settings`.
    #[must_use]
    pub fn new(settings: LoginSettings) -> Self {
        Self {
            settings,
            refreshed: None,
            program: None,
            working: BTreeMap::new(),
            windows: status::Windows::default(),
            unmatched: 0,
            listed: None,
        }
    }

    /// How many times a model the service listed matched none of the `models` a reading was given.
    #[must_use]
    pub const fn unmatched(&self) -> u64 {
        self.unmatched
    }

    /// Reads the stored login and, when it can, the plan and the quota, as of now. Starts no process.
    ///
    /// `models` are the ACP agent's models, as for [`super::AgyQuota::read`]; empty leaves every bucket's coverage unknown.
    ///
    /// # Errors
    ///
    /// [`LoginError`] when no login could be read: the keyring failed and no file holds one, or a login is not agy's JSON. A
    /// failure after the login was read is the reading's [`SignedIn::usage`].
    pub fn read(&mut self, models: &[AcpModel]) -> Result<LoginReading, LoginError> {
        let Some(login) = stored::load(&self.settings)? else {
            return Ok(LoginReading::NotSignedIn);
        };
        let mut account = AgyAccount {
            email: login.email.clone(),
            name: None,
            plan: login.tier_name.clone(),
        };
        let usage = self.usage(&login, &mut account, models, crate::acp::now());
        Ok(LoginReading::SignedIn(SignedIn { account, usage }))
    }

    fn agent(&self) -> ureq::Agent {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .timeout_global(Some(self.settings.timeout))
            .user_agent(self.settings.user_agent.as_str())
            .build();
        ureq::Agent::new_with_config(config)
    }

    fn usage(
        &mut self,
        login: &stored::Login,
        account: &mut AgyAccount,
        models: &[AcpModel],
        now: i64,
    ) -> Result<Vec<Bucket>, LoginError> {
        let agent = self.agent();
        let (mut token, mut fresh) = self.token(&agent, login, now)?;
        // A stored token the service refuses before its expiry is refreshed once and the call asked again.
        let mut authorized = |this: &mut Self, call: &Call<'_>| -> Result<Vec<u8>, LoginError> {
            match call(&agent, token.expose()) {
                Err(error) if error.cause == Cause::Status(401) && !fresh => {
                    token = this.refresh(&agent, login, now)?;
                    fresh = true;
                    call(&agent, token.expose())
                }
                other => other,
            }
        };

        if account.email.is_none() || account.name.is_none() {
            let url = self.settings.userinfo_url.clone();
            let limit = self.settings.body;
            let read = authorized(self, &|agent, token| {
                get(agent, &url, token, limit, Step::UserInfo)
            })
            .and_then(|body| {
                serde_json::from_slice::<UserInfo>(&body)
                    .map_err(|_| LoginError::new(Step::UserInfo, Cause::Malformed))
            });
            match read {
                Ok(info) => {
                    let text = |value: Option<String>| value.filter(|text| !text.is_empty());
                    account.email = account.email.take().or_else(|| text(info.email));
                    account.name = text(info.name);
                }
                // The stored email already says who the account is; the name is not worth the reading.
                Err(_) if account.email.is_some() => {}
                Err(error) => return Err(error),
            }
        }

        let base = self.base(login);
        let limit = self.settings.body;
        let mut request = json!({"metadata": {"ideType": "ANTIGRAVITY", "pluginType": "GEMINI"}});
        if let Some(project) = &login.project {
            request["cloudaicompanionProject"] = json!(project);
        }
        let url = endpoint(&base, "loadCodeAssist");
        let body = authorized(self, &|agent, token| {
            post(agent, &url, token, &request, limit, Step::LoadCodeAssist)
        })?;
        let assist: Assist = serde_json::from_slice(&body)
            .map_err(|_| LoginError::new(Step::LoadCodeAssist, Cause::Malformed))?;
        let tier = |tier: Option<Tier>| {
            tier.and_then(|tier| tier.name)
                .filter(|name| !name.is_empty())
        };
        if let Some(plan) = tier(assist.paid).or_else(|| tier(assist.current)) {
            account.plan = Some(plan);
        }
        let project = login.project.clone().or_else(|| match assist.project {
            Some(Value::String(id)) if !id.is_empty() => Some(id),
            Some(Value::Object(fields)) => fields
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(ToString::to_string),
            _ => None,
        });

        let request = project.map_or_else(|| json!({}), |project| json!({"project": project}));
        // With no session to say which models the agent lists, the service's own list says which models a group names.
        let listed: Vec<AcpModel>;
        let models = if models.is_empty() {
            listed = match &self.listed {
                Some((at, held)) if now.saturating_sub(*at) < LISTED_FOR => held.clone(),
                _ => {
                    let url = endpoint(&base, "fetchAvailableModels");
                    let read = authorized(self, &|agent, token| {
                        post(
                            agent,
                            &url,
                            token,
                            &request,
                            limit,
                            Step::FetchAvailableModels,
                        )
                    })
                    .ok()
                    .and_then(|body| status::parse_available(&body))
                    .map(|read| status::acp_models(&read))
                    .unwrap_or_default();
                    if !read.is_empty() {
                        self.listed = Some((now, read.clone()));
                    }
                    read
                }
            };
            &listed[..]
        } else {
            models
        };
        // The service's own summary of the quota says the groups of models and each group's windows; accounts and services that
        // do not answer it are read through each model's quota instead.
        let url = endpoint(&base, "retrieveUserQuotaSummary");
        let summary = authorized(self, &|agent, token| {
            post(agent, &url, token, &request, limit, Step::QuotaSummary)
        });
        if let Ok(body) = summary
            && let Some(made) = status::summary_buckets(&body, models, now)
        {
            return Ok(made);
        }
        let url = endpoint(&base, "fetchAvailableModels");
        let body = authorized(self, &|agent, token| {
            post(
                agent,
                &url,
                token,
                &request,
                limit,
                Step::FetchAvailableModels,
            )
        })?;
        let read = status::parse_available(&body).ok_or(LoginError::new(
            Step::FetchAvailableModels,
            Cause::Malformed,
        ))?;
        Ok(status::buckets(
            &read,
            models,
            now,
            &mut self.windows,
            &mut self.unmatched,
        ))
    }

    /// The Cloud Code address: the one set, else `CLOUD_CODE_URL`, else the sign-in's.
    fn base(&self, login: &stored::Login) -> String {
        if let Some(service) = &self.settings.service {
            return service.clone();
        }
        if let Some(service) = std::env::var(SERVICE_VARIABLE)
            .ok()
            .filter(|url| !url.is_empty())
        {
            return service;
        }
        if login.auth_method.as_deref() == Some("gcp") {
            self.settings.gcp_service.clone()
        } else {
            self.settings.consumer_service.clone()
        }
    }

    /// An access token to use: one refreshed earlier from the same login while it lasts, else the stored one while it lasts,
    /// else a new one. The flag says whether it was refreshed just now.
    fn token(
        &mut self,
        agent: &ureq::Agent,
        login: &stored::Login,
        now: i64,
    ) -> Result<(Secret, bool), LoginError> {
        if let Some(held) = &self.refreshed
            && login.refresh.as_ref() == Some(&held.from)
            && !expired(held.expires_at, now)
        {
            return Ok((held.access.clone(), false));
        }
        if let Some(access) = &login.access
            && !expired(login.expiry, now)
        {
            return Ok((access.clone(), false));
        }
        self.refresh(agent, login, now).map(|token| (token, true))
    }

    /// A new access token for the stored refresh token, kept in memory and never written anywhere. The OAuth clients are tried
    /// in turn, the one that worked for this kind of sign-in first; the first the token endpoint accepts is remembered.
    fn refresh(
        &mut self,
        agent: &ureq::Agent,
        login: &stored::Login,
        now: i64,
    ) -> Result<Secret, LoginError> {
        let refresh = login
            .refresh
            .as_ref()
            .ok_or(LoginError::new(Step::Refresh, Cause::Refused))?;
        let method = login.auth_method.clone().unwrap_or_default();
        let mut clients = self.clients()?;
        if let Some(known) = self.working.get(&method)
            && let Some(at) = clients.iter().position(|client| client == known)
        {
            let known = clients.remove(at);
            clients.insert(0, known);
        }
        if clients.is_empty() {
            return Err(LoginError::new(Step::Refresh, Cause::NoClient));
        }
        let limit = self.settings.body;
        for client in clients {
            let response = agent
                .post(&self.settings.token_url)
                .header("Accept", "application/json")
                .send_form([
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh.expose()),
                    ("client_id", client.id.as_str()),
                    ("client_secret", client.secret.expose()),
                ]);
            let mut response = response.map_err(|error| from_ureq(error, Step::Refresh))?;
            let code = response.status().as_u16();
            if code == 400 || code == 401 {
                continue;
            }
            if code != 200 {
                return Err(LoginError::new(Step::Refresh, Cause::Status(code)));
            }
            let body = read_body(&mut response, limit, Step::Refresh)?;
            let answer: TokenAnswer = serde_json::from_slice(&body)
                .map_err(|_| LoginError::new(Step::Refresh, Cause::Malformed))?;
            let access = answer
                .access_token
                .filter(|token| !token.is_empty())
                .map(Secret::new)
                .ok_or(LoginError::new(Step::Refresh, Cause::Malformed))?;
            self.working.insert(method, client);
            self.refreshed = Some(Refreshed {
                from: refresh.clone(),
                access: access.clone(),
                expires_at: answer
                    .expires_in
                    .and_then(|seconds| now.checked_add(seconds)),
            });
            return Ok(access);
        }
        Err(LoginError::new(Step::Refresh, Cause::Refused))
    }

    /// The OAuth clients to try: the one given, or every pairing of the ids and secrets Antigravity's program holds.
    fn clients(&mut self) -> Result<Vec<OAuthClient>, LoginError> {
        let path = match &self.settings.client {
            ClientSource::Off => return Ok(Vec::new()),
            ClientSource::Given(client) => return Ok(alloc::vec![client.clone()]),
            ClientSource::Program(path) => path.clone(),
            ClientSource::Find => match program::find() {
                Some(path) => path,
                None => return Ok(Vec::new()),
            },
        };
        let key = program::FileKey::of(&path)
            .map_err(|error| LoginError::new(Step::Program, Cause::Io(error.kind())))?;
        if let Some(held) = &self.program
            && held.key == key
        {
            return Ok(held.clients.clone());
        }
        let clients = program::clients(&path)
            .map_err(|error| LoginError::new(Step::Program, Cause::Io(error.kind())))?;
        self.program = Some(ProgramClients {
            key,
            clients: clients.clone(),
        });
        Ok(clients)
    }
}

/// Whether a token that expires at `at` (Unix seconds; none for never) is expired at `now`.
fn expired(at: Option<i64>, now: i64) -> bool {
    at.is_some_and(|at| at.saturating_sub(EXPIRY_MARGIN) <= now)
}

/// `{base}/v1internal:{method}`.
fn endpoint(base: &str, method: &str) -> String {
    alloc::format!("{}/v1internal:{method}", base.trim_end_matches('/'))
}

fn post(
    agent: &ureq::Agent,
    url: &str,
    token: &str,
    body: &Value,
    limit: usize,
    step: Step,
) -> Result<Vec<u8>, LoginError> {
    let text = body.to_string();
    let mut response = agent
        .post(url)
        .header("Authorization", alloc::format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send(text.as_bytes())
        .map_err(|error| from_ureq(error, step))?;
    answered(&mut response, limit, step)
}

fn get(
    agent: &ureq::Agent,
    url: &str,
    token: &str,
    limit: usize,
    step: Step,
) -> Result<Vec<u8>, LoginError> {
    let mut response = agent
        .get(url)
        .header("Authorization", alloc::format!("Bearer {token}"))
        .header("Accept", "application/json")
        .call()
        .map_err(|error| from_ureq(error, step))?;
    answered(&mut response, limit, step)
}

fn answered(
    response: &mut ureq::http::Response<ureq::Body>,
    limit: usize,
    step: Step,
) -> Result<Vec<u8>, LoginError> {
    let code = response.status().as_u16();
    if code != 200 {
        return Err(LoginError::new(step, Cause::Status(code)));
    }
    read_body(response, limit, step)
}

fn read_body(
    response: &mut ureq::http::Response<ureq::Body>,
    limit: usize,
    step: Step,
) -> Result<Vec<u8>, LoginError> {
    response
        .body_mut()
        .with_config()
        .limit(limit as u64)
        .read_to_vec()
        .map_err(|error| from_ureq(error, step))
}

fn from_ureq(error: ureq::Error, step: Step) -> LoginError {
    let cause = match error {
        ureq::Error::Io(error) => Cause::Io(error.kind()),
        ureq::Error::Timeout(_) => Cause::Io(io::ErrorKind::TimedOut),
        ureq::Error::BodyExceedsLimit(_) | ureq::Error::Protocol(_) | ureq::Error::Http(_) => {
            Cause::Malformed
        }
        _ => Cause::Io(io::ErrorKind::Other),
    };
    LoginError::new(step, cause)
}
