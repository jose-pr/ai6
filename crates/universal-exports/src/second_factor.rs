//! The steps of a sign-in after its first, and the setting up of a second factor.
//!
//! A sign-in whose policy decision offers more steps is kept in the Registry as a *pending*
//! sign-in, by the digest of the value its person holds, so any gateway sharing the Registry
//! continues it. Every try at its next step is counted twice: against the account in the login
//! guard and against the pending sign-in's own cap. Enrolling a factor is decided under the
//! sign-in policy (`enrol`, or `enrol_first` before any session exists), audited before it is
//! stored, and never audits or logs a code, a recovery code or a provisioning link.

use std::net::SocketAddr;
use std::sync::Arc;

use ai6::directorates::m::access::{
    Person, SignInContext, SignInDecision, methods as method_words, purposes,
};
use ai6::guatiao::Status;
use ai6::{
    AccountPut, Accounts, Attempt, CONFIG_ACCOUNTS, Lookup, ProviderConfig, RegistryBackend,
    SignInFactor, Verdict,
};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::header::{CACHE_CONTROL, ORIGIN};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use coqui_access_totp::{Algorithm, Lockout, TotpAccounts, TotpBuilder};
use serde::{Deserialize, Serialize};
use serde_json::json;

use ai6::directorates::registry::factors::FactorChange;

use crate::access_admin::{AdminError, audit};
use crate::factor_seal::FactorSealer;
use crate::{
    AccessProviders, ApiError, Gateway, LoginRequest, Signer, access_token_digest, bearer,
    complete_login, new_secret, require_origin, resolve_user, sign_in, unix_timestamp,
};

/// The ID the one-time code provider is listed under in `authenticators` and in a policy.
pub const TOTP: &str = "coqui_access_totp";

/// The header a page names a pending sign-in in when it asks what comes next.
pub(crate) const PENDING_HEADER: &str = "x-ai6-pending";

const PENDING: &str = "pending";

/// A sign-in that has passed some steps and waits for the next, as the Registry keeps it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    /// The ID of the person its steps named.
    person: String,
    factors: Vec<SignInFactor>,
    /// Unix seconds.
    expires: f64,
    /// Tries made at its next step.
    tries: u32,
    /// Whether a browser began it: only a browser continues it.
    browser: bool,
    ttl_seconds: Option<u64>,
    session_cookie: Option<bool>,
}

fn read_pending(text: &str, now: f64) -> Option<Pending> {
    serde_json::from_str::<Pending>(text)
        .ok()
        .filter(|pending| pending.expires > now)
}

/// What a try at a pending sign-in's next step finds.
enum Tried {
    /// There is no such sign-in, or its time is up.
    Gone,
    /// Its tries are used up; it has ended.
    Spent,
    /// The sign-in, with this try counted.
    Held,
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::unavailable())
}

fn registry(gateway: &Gateway) -> Result<Arc<dyn RegistryBackend>, ApiError> {
    gateway.registry.clone().ok_or_else(ApiError::unavailable)
}

async fn begin_pending(
    registry: Arc<dyn RegistryBackend>,
    pending: Pending,
    sweep: bool,
) -> Result<String, ApiError> {
    let token = new_secret();
    let key = access_token_digest(&token);
    let text = serde_json::to_string(&pending).map_err(|_| ApiError::internal())?;
    let now = unix_timestamp()?;
    blocking(move || {
        if sweep {
            registry.factor_retain(PENDING, &mut |_, text| read_pending(text, now).is_some())?;
        }
        registry.factor_update(PENDING, &key, &mut |_| Ok(FactorChange::Put(text.clone())))
    })
    .await?;
    Ok(token)
}

async fn find_pending(
    registry: Arc<dyn RegistryBackend>,
    key: String,
    now: f64,
) -> Result<Option<Pending>, ApiError> {
    // A read writes nothing, so a request that names no sign-in costs the shared store nothing.
    blocking(move || {
        Ok(registry
            .factor_get(PENDING, &key)?
            .and_then(|text| read_pending(&text, now)))
    })
    .await
}

async fn count_try(
    registry: Arc<dyn RegistryBackend>,
    key: String,
    cap: u32,
    now: f64,
) -> Result<Tried, ApiError> {
    blocking(move || {
        let mut tried = Tried::Gone;
        registry.factor_update(PENDING, &key, &mut |held| {
            let Some(mut pending) = held.and_then(|text| read_pending(text, now)) else {
                tried = Tried::Gone;
                return Ok(if held.is_some() {
                    FactorChange::Remove
                } else {
                    FactorChange::Keep
                });
            };
            if pending.tries >= cap {
                tried = Tried::Spent;
                return Ok(FactorChange::Remove);
            }
            pending.tries += 1;
            let text = serde_json::to_string(&pending).map_err(std::io::Error::other)?;
            tried = Tried::Held;
            Ok(FactorChange::Put(text))
        })?;
        Ok(tried)
    })
    .await
}

async fn advance_pending(
    registry: Arc<dyn RegistryBackend>,
    key: String,
    factors: Vec<SignInFactor>,
    now: f64,
) -> Result<(), ApiError> {
    blocking(move || {
        registry.factor_update(PENDING, &key, &mut |held| {
            let Some(mut pending) = held.and_then(|text| read_pending(text, now)) else {
                return Ok(FactorChange::Keep);
            };
            pending.factors = factors.clone();
            Ok(FactorChange::Put(
                serde_json::to_string(&pending).map_err(std::io::Error::other)?,
            ))
        })
    })
    .await
}

/// Takes the pending sign-in, so that exactly one request completes it.
async fn take_pending(
    registry: Arc<dyn RegistryBackend>,
    key: String,
    now: f64,
) -> Result<Option<Pending>, ApiError> {
    blocking(move || {
        let mut taken = None;
        registry.factor_update(PENDING, &key, &mut |held| {
            taken = held.and_then(|text| read_pending(text, now));
            Ok(if held.is_some() {
                FactorChange::Remove
            } else {
                FactorChange::Keep
            })
        })?;
        Ok(taken)
    })
    .await
}

async fn end_pending(registry: Arc<dyn RegistryBackend>, key: String) {
    let _ =
        blocking(move || registry.factor_update(PENDING, &key, &mut |_| Ok(FactorChange::Remove)))
            .await;
}

/// The method words of `decision`, once each.
fn method_names(decision: &SignInDecision) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for offer in &decision.methods {
        if !names.contains(&offer.method) {
            names.push(offer.method.clone());
        }
    }
    names
}

/// The `202` answer of a sign-in that waits for its next step.
fn next_step(token: &str, methods: Vec<String>, seconds: f64) -> Response {
    let mut response = (
        StatusCode::ACCEPTED,
        Json(json!({"pending": token, "methods": methods, "expires_in": seconds})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

impl Gateway {
    /// Whether this request is one of the sweeps that drop pending sign-ins gone stale.
    fn sweeps_now(&self) -> bool {
        let every = u64::from(self.config.login.forget_every.max(1));
        self.pending_begun
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .is_multiple_of(every)
    }

    /// Keeps a sign-in that `factors` took as far as they did, to wait for its next step, and
    /// answers `202`.
    pub(crate) async fn wait_for_next_step(
        &self,
        person: &Person,
        factors: Vec<SignInFactor>,
        methods: Vec<String>,
        from_browser: bool,
        request: &LoginRequest,
    ) -> Result<Response, ApiError> {
        let seconds = self.config.login.pending_seconds;
        let pending = Pending {
            person: person.id.clone(),
            factors,
            expires: unix_timestamp()? + seconds.as_secs_f64(),
            tries: 0,
            browser: from_browser,
            ttl_seconds: request.ttl_seconds,
            session_cookie: request.session_cookie,
        };
        let token = begin_pending(registry(self)?, pending, self.sweeps_now()).await?;
        Ok(next_step(&token, methods, seconds.as_secs_f64()))
    }
}

/// The person with ID `id`, as the directory has them now.
async fn person_by_id(providers: &AccessProviders, id: String) -> Result<Option<Person>, ApiError> {
    let directory = providers.directory.clone();
    tokio::task::spawn_blocking(move || {
        directory.lookup(Lookup {
            by: "id".to_owned(),
            value: id,
            ..Default::default()
        })
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())
}

fn address_of(connection: Option<Extension<ConnectInfo<SocketAddr>>>) -> Option<SocketAddr> {
    connection.map(|Extension(ConnectInfo(address))| address)
}

/// What a page that holds a pending sign-in is offered next: the methods, or `None` when the
/// sign-in is gone.
pub(crate) async fn offer_next(
    gateway: &Gateway,
    providers: &AccessProviders,
    headers: &HeaderMap,
    address: Option<SocketAddr>,
    token: &str,
) -> Result<Vec<String>, ApiError> {
    // Counted against the address before the Registry is read, whether or not the sign-in is
    // there: a client that names sign-ins at random is held off.
    let source = gateway.login_guard().source(headers, address);
    if let Some(wait) = gateway.login_guard().tried_address(source) {
        return Err(ApiError::too_many_attempts(wait));
    }
    let now = unix_timestamp()?;
    let pending = find_pending(registry(gateway)?, access_token_digest(token), now)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let person = person_by_id(providers, pending.person.clone())
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let context = SignInContext {
        done: Gateway::passed(&pending.factors),
        person: Some(person),
        ..gateway.sign_in_context(purposes::SIGN_IN, headers, address, sign_in::SERVICE)
    };
    gateway
        .decide_sign_in(providers, context)
        .await
        .map(|decision| method_names(&decision))
        .ok_or_else(ApiError::unauthorized)
}

/// The second-factor providers a method is asked of: those the offer lists, else every one.
fn asked(providers: &AccessProviders, offered: &[String]) -> Vec<Signer> {
    if offered.is_empty() {
        return providers.second.clone();
    }
    offered
        .iter()
        .filter_map(|id| {
            providers
                .second
                .iter()
                .find(|signer| signer.id.as_deref() == Some(id))
                .cloned()
        })
        .collect()
}

/// `POST /v1/auth/login` with a `method` that checks a code, or with a `pending` sign-in:
/// the next step of a sign-in.
pub(crate) async fn continue_login(
    gateway: &Gateway,
    connection: Option<SocketAddr>,
    certificates: Option<Vec<Vec<u8>>>,
    headers: &HeaderMap,
    request: LoginRequest,
) -> Result<Response, ApiError> {
    let from_browser = headers.contains_key(ORIGIN);
    if from_browser {
        require_origin(gateway, headers)?;
    }
    let providers = gateway.access.clone().ok_or_else(ApiError::unavailable)?;
    let method = request.method.clone().ok_or_else(ApiError::bad_request)?;
    let context = gateway.sign_in_context(purposes::SIGN_IN, headers, connection, sign_in::SERVICE);
    let registry = registry(gateway)?;
    let now = unix_timestamp()?;
    let Some(token) = request.pending.as_deref() else {
        // Nothing is pending, so nothing an account's code could follow is offered.
        let offered = gateway
            .decide_sign_in(&providers, context)
            .await
            .is_some_and(|decision| decision.methods.iter().any(|offer| offer.method == method));
        return Err(if offered {
            ApiError::unauthorized()
        } else {
            sign_in::not_accepted()
        });
    };
    // Counted against the address before the Registry is read, whether or not the sign-in is
    // there, so a client naming sign-ins at random is held off and costs the store nothing.
    let source = gateway.login_guard().source(headers, connection);
    if let Some(wait) = gateway.login_guard().tried_address(source) {
        return Err(ApiError::too_many_attempts(wait));
    }
    let key = access_token_digest(token);
    let Some(pending) = find_pending(registry.clone(), key.clone(), now).await? else {
        return Err(ApiError::unauthorized());
    };
    if pending.browser != from_browser {
        return Err(ApiError::unauthorized());
    }
    let Some(person) = person_by_id(&providers, pending.person.clone()).await? else {
        end_pending(registry, key).await;
        return Err(ApiError::unauthorized());
    };
    let context = SignInContext {
        done: Gateway::passed(&pending.factors),
        person: Some(person.clone()),
        ..context
    };
    let offer = gateway
        .decide_sign_in(&providers, context.clone())
        .await
        .and_then(|decision| {
            decision
                .methods
                .into_iter()
                .find(|offer| offer.method == method)
        })
        .ok_or_else(sign_in::not_accepted)?;
    // Counted before anything is asked, under the account, whatever the address: a refusal is
    // not judged, and the right code along with the wrong ones.
    let by_password = method == "password";
    if !by_password && let Some(wait) = gateway.login_guard().tried_name(&person.id) {
        return Err(ApiError::too_many_attempts(wait));
    }
    match count_try(
        registry.clone(),
        key.clone(),
        gateway.config.login.pending_tries,
        now,
    )
    .await?
    {
        Tried::Gone | Tried::Spent => return Err(ApiError::unauthorized()),
        Tried::Held => {}
    }
    if !request.name.is_empty() && !by_password && request.name != person.id {
        // A code is only ever checked for the person the sign-in named.
        return Err(ApiError::unauthorized());
    }
    let step = match method.as_str() {
        method_words::TOTP | method_words::RECOVERY_CODE => {
            second_factor_step(
                &providers,
                &person,
                &method,
                &offer.providers,
                request.secret.clone(),
                source,
            )
            .await?
        }
        // A certificate the gateway's own TLS proved is named by the page authenticator.
        "certificate" if certificates.as_ref().is_some_and(|chain| !chain.is_empty()) => {
            certificate_step(
                &providers,
                &person,
                certificates.unwrap_or_default(),
                source,
            )
            .await?
        }
        // `certificate` is only a chain the gateway's own TLS proved; a header never makes one.
        "certificate" => return Err(ApiError::unauthorized()),
        "proxy" => {
            let named = crate::proxy_person(gateway, &providers, headers)
                .await?
                .ok_or_else(ApiError::unauthorized)?;
            if named.id != person.id {
                return Err(ApiError::unauthorized());
            }
            sign_in::step(&method, None, &named)?
        }
        "password" => {
            let (named, provider) = crate::authenticate_password(
                gateway,
                &providers,
                headers,
                connection,
                request.clone(),
                &offer.providers,
            )
            .await?;
            if named.id != person.id {
                return Err(ApiError::unauthorized());
            }
            sign_in::step(&method, provider.as_deref(), &named)?
        }
        _ => return Err(ApiError::bad_request()),
    };
    let mut factors = pending.factors.clone();
    factors.push(step);
    let decision = gateway
        .decide_sign_in(
            &providers,
            SignInContext {
                done: Gateway::passed(&factors),
                person: Some(person.clone()),
                ..context
            },
        )
        .await;
    match decision {
        Some(decision) if decision.complete => {
            // One request takes the sign-in; a second finds it gone.
            let Some(taken) = take_pending(registry, key, unix_timestamp()?).await? else {
                return Err(ApiError::unauthorized());
            };
            let wants_cookie = from_browser && taken.session_cookie != Some(false);
            let ttl = crate::sign_in_ttl(
                &gateway.config.unex().gateway,
                crate::access_token_ttl(gateway, taken.ttl_seconds)?,
            );
            complete_login(gateway, from_browser, wants_cookie, ttl, person, factors).await
        }
        Some(decision) if !decision.methods.is_empty() => {
            advance_pending(registry, key, factors, now).await?;
            let left = pending.expires - now;
            Ok(next_step(token, method_names(&decision), left))
        }
        _ => {
            end_pending(registry, key).await;
            Err(ApiError::unauthorized())
        }
    }
}

/// The step a certificate the gateway's own TLS proved makes, when the page authenticator names
/// `person` by it.
async fn certificate_step(
    providers: &AccessProviders,
    person: &Person,
    chain: Vec<Vec<u8>>,
    source: std::net::IpAddr,
) -> Result<SignInFactor, ApiError> {
    let authenticator = providers
        .page_authenticator
        .clone()
        .ok_or_else(ApiError::unauthorized)?;
    let attempt = Attempt {
        peer: Some(source.to_string()),
        secure: Some(true),
        certificates: Some(chain.into_iter().map(Into::into).collect()),
        ..Attempt::default()
    };
    let verdict = tokio::task::spawn_blocking(move || authenticator.authenticate(attempt))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    let Verdict::Valid(lookup) = verdict else {
        return Err(ApiError::unauthorized());
    };
    let named = tokio::task::spawn_blocking({
        let directory = providers.directory.clone();
        move || directory.lookup(lookup)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?
    .ok_or_else(ApiError::unauthorized)?;
    if named.id != person.id {
        return Err(ApiError::unauthorized());
    }
    sign_in::step(
        "certificate",
        providers.page_authenticator_id.as_deref(),
        &named,
    )
}

/// Checks a code of `person`'s by the providers the offer lists.
async fn second_factor_step(
    providers: &AccessProviders,
    person: &Person,
    method: &str,
    offered: &[String],
    secret: String,
    source: std::net::IpAddr,
) -> Result<SignInFactor, ApiError> {
    let asked = asked(providers, offered);
    let attempt = Attempt {
        name: Some(person.id.clone()),
        secret: Some(secret),
        peer: Some(source.to_string()),
        secure: Some(true),
        method: Some(method.to_owned()),
        ..Attempt::default()
    };
    let verdict = tokio::task::spawn_blocking(move || {
        for signer in asked {
            match signer.authenticator.authenticate(attempt.clone())? {
                Verdict::Unknown => {}
                decided => return Ok((decided, signer.id)),
            }
        }
        Ok((Verdict::Unknown, None))
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_: ai6::guatiao::library::ProviderError| ApiError::internal())?;
    let (Verdict::Valid(lookup), provider) = verdict else {
        return Err(ApiError::unauthorized());
    };
    // The step must name the same person the sign-in is for.
    if lookup.by != "id" || lookup.value != person.id {
        return Err(ApiError::unauthorized());
    }
    sign_in::step(method, provider.as_deref(), person)
}

/// The provider settings of the one-time code provider as the `unex` record gives them: every
/// key optional, the provider's own default standing for what is left out.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TotpSettings {
    issuer: Option<String>,
    algorithm: Option<String>,
    digits: Option<u32>,
    period: Option<u32>,
    skew: Option<u32>,
    recovery_codes: Option<u32>,
    enrol_seconds: Option<u32>,
    max_records: Option<usize>,
    lockout_after: Option<u32>,
    lockout_seconds: Option<u32>,
    lockout_longest_seconds: Option<u32>,
}

/// The one-time code provider over the Registry's tables, configured by `config`, ready to build.
///
/// # Errors
///
/// Why the configuration is refused.
pub fn totp_builder(
    registry: &Arc<dyn RegistryBackend>,
    config: Option<&ProviderConfig>,
    sealer: &Arc<FactorSealer>,
) -> Result<TotpBuilder, String> {
    let settings: TotpSettings = match config {
        Some(object) => serde_json::from_value(serde_json::Value::Object(object.0.clone()))
            .map_err(|error| error.to_string())?,
        None => TotpSettings::default(),
    };
    let mut builder = TotpAccounts::builder()
        .table(crate::totp_store::accounts(registry, sealer))
        .pending(crate::totp_store::enrolments(registry, sealer));
    if let Some(issuer) = settings.issuer {
        builder = builder.issuer(issuer);
    }
    if let Some(name) = settings.algorithm {
        builder = builder.algorithm(match name.as_str() {
            "sha1" => Algorithm::Sha1,
            "sha256" => Algorithm::Sha256,
            "sha512" => Algorithm::Sha512,
            other => return Err(format!("algorithm {other:?} is not sha1, sha256 or sha512")),
        });
    }
    if let Some(digits) = settings.digits {
        builder = builder.digits(digits);
    }
    if let Some(period) = settings.period {
        builder = builder.period(period);
    }
    if let Some(skew) = settings.skew {
        builder = builder.skew(skew);
    }
    if let Some(count) = settings.recovery_codes {
        builder = builder.recovery_codes(count);
    }
    if let Some(seconds) = settings.enrol_seconds {
        builder = builder.enrol_seconds(seconds);
    }
    if let Some(records) = settings.max_records {
        builder = builder.max_records(records);
    }
    if settings.lockout_after.is_some()
        || settings.lockout_seconds.is_some()
        || settings.lockout_longest_seconds.is_some()
    {
        let default = Lockout::DEFAULT;
        builder = builder.lockout(Lockout::new(
            settings.lockout_after.unwrap_or(default.after),
            settings.lockout_seconds.unwrap_or(default.first_seconds),
            settings
                .lockout_longest_seconds
                .unwrap_or(default.longest_seconds),
        ));
    }
    Ok(builder)
}

/// The one-time code provider over the Registry's tables, configured by `config`.
///
/// # Errors
///
/// Why the configuration is refused.
pub fn totp_provider(
    registry: &Arc<dyn RegistryBackend>,
    config: Option<&ProviderConfig>,
    sealer: &Arc<FactorSealer>,
) -> Result<Arc<TotpAccounts>, String> {
    totp_builder(registry, config, sealer)?
        .build()
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

fn factors_of(gateway: &Gateway) -> Result<Arc<dyn Accounts>, ApiError> {
    gateway
        .access
        .as_ref()
        .and_then(|providers| providers.factors.clone())
        .ok_or_else(ApiError::not_found)
}

/// What beginning an enrolment answers; shown once and kept nowhere as itself.
#[derive(Serialize)]
pub(crate) struct Begun {
    /// The `otpauth://` link, to show as a QR code.
    uri: String,
    recovery_codes: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Code {
    code: String,
    #[serde(default)]
    pending: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Held {
    pending: String,
}

/// The proof a person who already has a factor gives to replace it, besides a fresh sign-in.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Begin {
    /// A code from the factor they have now.
    #[serde(default)]
    code: Option<String>,
}

fn refused_because(reason: &str) -> AdminError {
    AdminError::Api(ApiError::refused(StatusCode::FORBIDDEN, reason.to_owned()))
}

fn refused_here() -> AdminError {
    refused_because("setting up a second factor is not accepted from here")
}

/// Refuses what cannot be a code before anything is counted or recorded.
fn code_shaped(code: &str) -> Result<(), AdminError> {
    if code.is_empty() || code.len() > 64 || code.chars().any(char::is_control) {
        return Err(AdminError::Refused("that is not a code".to_owned()));
    }
    Ok(())
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The person, and the sign-in the presented bearer belongs to.
struct Signed {
    person: Person,
    /// The steps the sign-in was opened with.
    factors: Vec<SignInFactor>,
    /// When the sign-in was made, Unix seconds.
    issued_at: f64,
    /// The IDs of this sign-in's own records, which a revocation of the person's others keeps.
    keep: Vec<String>,
}

async fn bearer_steps(gateway: &Gateway, headers: &HeaderMap) -> Result<Signed, ApiError> {
    let token = bearer(headers)?;
    let person = resolve_user(gateway, token).await?;
    let registry = registry(gateway)?;
    let digest = access_token_digest(token);
    let record = blocking(move || registry.find_access_token(&digest))
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let factors = record.factors().to_vec();
    if sign_in::from_station(&factors) {
        return Err(ApiError::forbidden());
    }
    let mut keep = vec![record.token_id().to_owned()];
    keep.extend(record.sign_in().map(str::to_owned));
    Ok(Signed {
        person,
        factors,
        issued_at: record.issued_at(),
        keep,
    })
}

/// Decides `purpose` for `person` with `factors`; whether the steps are enough.
async fn decided_for(
    gateway: &Gateway,
    headers: &HeaderMap,
    address: Option<SocketAddr>,
    purpose: &str,
    person: &Person,
    factors: &[SignInFactor],
) -> Result<bool, ApiError> {
    let providers = gateway.access.clone().ok_or_else(ApiError::unavailable)?;
    let context = SignInContext {
        done: Gateway::passed(factors),
        person: Some(person.clone()),
        ..gateway.sign_in_context(purpose, headers, address, sign_in::SERVICE)
    };
    Ok(gateway
        .decide_sign_in(&providers, context)
        .await
        .is_some_and(|decision| decision.complete))
}

/// Whether `id` has a factor, and whether one is being set up for them.
async fn held_by(accounts: Arc<dyn Accounts>, id: String) -> Result<(bool, bool), ApiError> {
    tokio::task::spawn_blocking(move || {
        Ok::<_, ai6::guatiao::library::ProviderError>((
            accounts.accounts()?.iter().any(|info| info.id == id),
            accounts
                .waiting()?
                .iter()
                .any(|waiting| waiting.shown == id),
        ))
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::unavailable())
}

async fn begin(
    gateway: &Gateway,
    person: &Person,
    accounts: Arc<dyn Accounts>,
) -> Result<Response, AdminError> {
    let refusal = gateway
        .access
        .as_ref()
        .and_then(|providers| providers.factor_sealer.as_ref())
        .map_or_else(
            || Some("no [unex] factor_key is configured".to_owned()),
            |sealer| sealer.refusal(),
        );
    if let Some(why) = refusal {
        return Err(AdminError::Conflict(format!(
            "one-time codes cannot be set up: {why}"
        )));
    }
    // The record of it comes first: an enrolment that cannot be recorded is not made.
    audit(
        gateway,
        person,
        "account.factor.enrol",
        None,
        CONFIG_ACCOUNTS,
        &person.id,
    )
    .await?;
    let put = AccountPut {
        name: person.id.clone(),
        display_name: Some(person.name.clone()),
        ..AccountPut::default()
    };
    let outcome = tokio::task::spawn_blocking(move || accounts.put(put))
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|_| AdminError::Api(ApiError::unavailable()))?;
    let uri = outcome
        .secret
        .ok_or(AdminError::Api(ApiError::internal()))?;
    Ok(no_store(
        Json(Begun {
            uri,
            recovery_codes: outcome.codes.unwrap_or_default(),
        })
        .into_response(),
    ))
}

async fn confirm(
    gateway: &Gateway,
    person: &Person,
    accounts: Arc<dyn Accounts>,
    code: String,
) -> Result<Response, AdminError> {
    audit(
        gateway,
        person,
        "account.factor.confirm",
        None,
        CONFIG_ACCOUNTS,
        &person.id,
    )
    .await?;
    let put = AccountPut {
        name: person.id.clone(),
        secret: Some(code),
        ..AccountPut::default()
    };
    tokio::task::spawn_blocking(move || accounts.put(put))
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|error| match error.status {
            Status::GUATIAO_ERR_BAD_VALUE => AdminError::Refused(
                "that code is not right, or the enrolment ran out; begin again".to_owned(),
            ),
            Status::GUATIAO_ERR_NOT_FOUND => {
                AdminError::Conflict("no enrolment is waiting; begin one".to_owned())
            }
            _ => AdminError::Api(ApiError::unavailable()),
        })?;
    Ok(no_store(Json(json!({"enrolled": true})).into_response()))
}

/// Ends every sign-in of `person_id` but the ones in `keep`, audited first.
async fn end_logins(
    gateway: &Gateway,
    actor: &Person,
    person_id: &str,
    keep: Vec<String>,
) -> Result<(), AdminError> {
    audit(
        gateway,
        actor,
        "account.factor.revoke",
        None,
        CONFIG_ACCOUNTS,
        person_id,
    )
    .await?;
    let (registry, person_id) = (registry(gateway)?, person_id.to_owned());
    blocking(move || registry.revoke_person_logins(&person_id, &keep)).await?;
    Ok(())
}

/// What `GET /v1/user/factors` says of the signed-in person.
#[derive(Serialize)]
pub(crate) struct FactorState {
    /// Whether a one-time code provider is configured.
    configured: bool,
    /// Whether the person has a confirmed one-time code factor.
    enrolled: bool,
    /// Whether an enrolment waits for its first code.
    enrolling: bool,
    /// Whether the sign-in policy lets this sign-in set up a factor.
    may_enrol: bool,
}

/// `GET /v1/user/factors`.
pub(crate) async fn state(
    State(gateway): State<Gateway>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
) -> Result<Json<FactorState>, ApiError> {
    let signed = bearer_steps(&gateway, &headers).await?;
    let Ok(accounts) = factors_of(&gateway) else {
        return Ok(Json(FactorState {
            configured: false,
            enrolled: false,
            enrolling: false,
            may_enrol: false,
        }));
    };
    let may_enrol = decided_for(
        &gateway,
        &headers,
        address_of(connection),
        purposes::ENROL,
        &signed.person,
        &signed.factors,
    )
    .await?;
    let (enrolled, enrolling) = held_by(accounts, signed.person.id.clone()).await?;
    Ok(Json(FactorState {
        configured: true,
        enrolled,
        enrolling,
        may_enrol,
    }))
}

/// The signed-in person whom the policy lets set up a factor now (`purpose: enrol`), counted
/// against the login guard per person and per address before anything is recorded.
async fn enrolling_person(
    gateway: &Gateway,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: &HeaderMap,
) -> Result<(Signed, Arc<dyn Accounts>, std::net::IpAddr), AdminError> {
    let accounts = factors_of(gateway)?;
    let address = address_of(connection);
    let signed = bearer_steps(gateway, headers).await?;
    if !decided_for(
        gateway,
        headers,
        address,
        purposes::ENROL,
        &signed.person,
        &signed.factors,
    )
    .await?
    {
        return Err(refused_here());
    }
    let source = gateway.login_guard().source(headers, address);
    if let Some(wait) = gateway.login_guard().tried(source, &signed.person.id) {
        return Err(AdminError::Api(ApiError::too_many_attempts(wait)));
    }
    Ok((signed, accounts, source))
}

/// `POST /v1/user/factors/totp`: begins an enrolment, or a rotation. Needs a sign-in the policy
/// lets enrol (`purpose: enrol`). A person who has a factor replaces it only with a code from it
/// (`{"code"}`) or from a sign-in made within `[login] enrol_fresh_seconds`.
pub(crate) async fn begin_enrolment(
    State(gateway): State<Gateway>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    body: Option<Json<Begin>>,
) -> Result<Response, AdminError> {
    let (signed, accounts, source) = enrolling_person(&gateway, connection, &headers).await?;
    let (enrolled, _) = held_by(accounts.clone(), signed.person.id.clone()).await?;
    if enrolled {
        let fresh = unix_timestamp()? - signed.issued_at
            <= gateway.config.login.enrol_fresh_seconds.as_secs_f64();
        if !fresh {
            let code = body.and_then(|Json(begin)| begin.code).ok_or_else(|| {
                refused_because(
                    "replacing your second factor needs a code from it, or a recent sign-in",
                )
            })?;
            code_shaped(&code)?;
            let providers = gateway.access.clone().ok_or_else(ApiError::unavailable)?;
            second_factor_step(
                &providers,
                &signed.person,
                method_words::TOTP,
                &[],
                code,
                source,
            )
            .await
            .map_err(|_| refused_because("that code is not right"))?;
        }
    }
    begin(&gateway, &signed.person, accounts).await
}

/// `POST /v1/user/factors/totp/confirm`: the first code from the app makes the factor. Replacing
/// a factor ends the person's other sign-ins.
pub(crate) async fn confirm_enrolment(
    State(gateway): State<Gateway>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<Code>,
) -> Result<Response, AdminError> {
    code_shaped(&body.code)?;
    let (signed, accounts, _) = enrolling_person(&gateway, connection, &headers).await?;
    let (replacing, _) = held_by(accounts.clone(), signed.person.id.clone()).await?;
    let answered = confirm(&gateway, &signed.person, accounts, body.code).await?;
    if replacing {
        end_logins(&gateway, &signed.person, &signed.person.id, signed.keep).await?;
    }
    Ok(answered)
}

/// The pending sign-in `token` names and its person, when the sign-in policy lets it set up a
/// factor now (`purpose: enrol_first`). The address is counted before the Registry is read, and
/// the person's own count after the person is known.
async fn enrolling_first(
    gateway: &Gateway,
    connection: Option<SocketAddr>,
    headers: &HeaderMap,
    token: &str,
    counted: bool,
) -> Result<(Person, Arc<dyn Accounts>), AdminError> {
    let from_browser = headers.contains_key(ORIGIN);
    if from_browser {
        require_origin(gateway, headers)?;
    }
    let accounts = factors_of(gateway)?;
    let providers = gateway.access.clone().ok_or_else(ApiError::unavailable)?;
    let source = gateway.login_guard().source(headers, connection);
    if let Some(wait) = gateway.login_guard().tried_address(source) {
        return Err(AdminError::Api(ApiError::too_many_attempts(wait)));
    }
    let now = unix_timestamp()?;
    let key = access_token_digest(token);
    let registry = registry(gateway)?;
    let Some(pending) = find_pending(registry.clone(), key.clone(), now).await? else {
        return Err(AdminError::Api(ApiError::unauthorized()));
    };
    if pending.browser != from_browser {
        return Err(AdminError::Api(ApiError::unauthorized()));
    }
    let person = person_by_id(&providers, pending.person.clone())
        .await?
        .ok_or(AdminError::Api(ApiError::unauthorized()))?;
    let context = SignInContext {
        done: Gateway::passed(&pending.factors),
        person: Some(person.clone()),
        ..gateway.sign_in_context(purposes::ENROL_FIRST, headers, connection, sign_in::SERVICE)
    };
    let allowed = gateway
        .decide_sign_in(&providers, context)
        .await
        .is_some_and(|decision| decision.complete);
    if !allowed {
        return Err(refused_here());
    }
    if let Some(wait) = gateway.login_guard().tried_name(&person.id) {
        return Err(AdminError::Api(ApiError::too_many_attempts(wait)));
    }
    // A code tried here is a guess at the factor being made, so it is counted as any is.
    if counted {
        let cap = gateway.config.login.pending_tries;
        match count_try(registry, key, cap, now).await? {
            Tried::Held => {}
            Tried::Gone | Tried::Spent => return Err(AdminError::Api(ApiError::unauthorized())),
        }
    }
    Ok((person, accounts))
}

/// `POST /v1/auth/enrol`: a sign-in that waits for a code begins setting one up, where a rule
/// of the policy names `enrol_first`. Whoever holds only the first factor never replaces a factor
/// that is there or being made: it is refused.
pub(crate) async fn begin_first(
    State(gateway): State<Gateway>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<Held>,
) -> Result<Response, AdminError> {
    let (person, accounts) = enrolling_first(
        &gateway,
        address_of(connection),
        &headers,
        &body.pending,
        false,
    )
    .await?;
    let (enrolled, enrolling) = held_by(accounts.clone(), person.id.clone()).await?;
    if enrolled || enrolling {
        return Err(refused_because(
            "a second factor is already set up or being set up; sign in with it, or ask an \
             administrator to reset it",
        ));
    }
    begin(&gateway, &person, accounts).await
}

/// `POST /v1/auth/enrol/confirm`: the first code from the app. The sign-in then continues with
/// the next code.
pub(crate) async fn confirm_first(
    State(gateway): State<Gateway>,
    connection: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<Code>,
) -> Result<Response, AdminError> {
    code_shaped(&body.code)?;
    let token = body
        .pending
        .clone()
        .ok_or(AdminError::Api(ApiError::bad_request()))?;
    let (person, accounts) =
        enrolling_first(&gateway, address_of(connection), &headers, &token, true).await?;
    confirm(&gateway, &person, accounts, body.code).await
}

/// `DELETE /v1/user/access/factors/{person}`: an administrator's reset, after which the person
/// enrols again and every sign-in they have ends. Needs `account.manage` at `config:accounts`.
pub(crate) async fn reset(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(person_id): Path<String>,
) -> Result<StatusCode, AdminError> {
    let admin = resolve_user(&gateway, bearer(&headers)?).await?;
    crate::authorize_resource_action(
        &gateway,
        admin.clone(),
        CONFIG_ACCOUNTS.to_owned(),
        "account.manage",
    )
    .await?;
    let accounts = factors_of(&gateway)?;
    if person_id.trim().is_empty() || person_id.chars().any(char::is_control) {
        return Err(AdminError::Api(ApiError::bad_request()));
    }
    audit(
        &gateway,
        &admin,
        "account.factor.reset",
        None,
        CONFIG_ACCOUNTS,
        &person_id,
    )
    .await?;
    let removed = person_id.clone();
    tokio::task::spawn_blocking(move || accounts.remove(removed))
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|error| match error.status {
            Status::GUATIAO_ERR_NOT_FOUND => AdminError::Api(ApiError::not_found()),
            _ => AdminError::Api(ApiError::unavailable()),
        })?;
    end_logins(&gateway, &admin, &person_id, Vec::new()).await?;
    Ok(StatusCode::NO_CONTENT)
}
