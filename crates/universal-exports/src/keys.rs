//! API keys: the key provider's place in sign-in, and the routes with which a person makes,
//! lists and revokes their own keys (`/v1/user/keys`).
//!
//! A key is presented as a request's bearer. The key provider says whose it is; a personal key
//! then acts as the person who made it, narrowed to the scopes named when it was made and
//! judged again at every use, so a grant the person loses the key loses too. A key without a
//! record is a service account's: it acts as that account, with the account's own grants. The
//! key's value is shown once, when it is made, and is in no log, audit record or error.

use std::sync::Arc;

use ai6::{
    ACTIONS, AccountPut, Accounts, ApiKeyRecord, Attempt, Authenticator, CONFIG_ACCOUNTS,
    Directory, KeyScope, Person, Verdict,
};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::access_admin::AdminError;
use crate::{
    ApiError, Gateway, actor_of, bearer, person_by_id, resolve_signed_in, resource_action_allowed,
    unix_timestamp,
};

/// The start of the name of the key provider's account behind a personal key. Accounts made
/// through the account routes may not take it, so no service account is mistaken for a key.
pub(crate) const PERSONAL_PREFIX: &str = "uk-";

/// The key provider as the gateway uses it: it recognizes a key, finds the account a key
/// belongs to, and makes and removes the accounts behind personal keys.
#[derive(Clone)]
pub struct KeyProvider {
    authenticator: Arc<dyn Authenticator>,
    directory: Arc<dyn Directory>,
    accounts: Arc<dyn Accounts>,
    /// Seconds a personal key lives when its maker names none; 0 never ends.
    lifetime: f64,
    /// Seconds within which a second use of a key is not written down again.
    precision: f64,
}

impl KeyProvider {
    /// The key provider `authenticator`, whose accounts `directory` finds and `accounts` keeps.
    pub fn new(
        authenticator: Arc<dyn Authenticator>,
        directory: Arc<dyn Directory>,
        accounts: Arc<dyn Accounts>,
    ) -> Self {
        Self {
            authenticator,
            directory,
            accounts,
            lifetime: DEFAULT_KEY_TTL,
            precision: 5.0,
        }
    }

    /// Sets how long a personal key lives when its maker names no expiry, in seconds (the
    /// record's `key_ttl`; 0 means until revoked). Default: 90 days.
    #[must_use]
    pub fn with_lifetime(mut self, seconds: Option<f64>) -> Self {
        self.lifetime = seconds.map_or(DEFAULT_KEY_TTL, |seconds| seconds.max(0.0));
        self
    }

    /// Sets how closely a key's last use is kept, in seconds (the access cache's lifetime, the
    /// record's `access_cache_ttl`). Default: 5.
    #[must_use]
    pub fn with_use_precision(mut self, seconds: Option<f64>) -> Self {
        self.precision = seconds.map_or(5.0, |seconds| seconds.max(0.0));
        self
    }
}

/// The person a presented key acts as; `None` when no key provider claims the value, or it is
/// refused (unknown, disabled, expired, revoked, or its person gone).
pub(crate) async fn resolve(gateway: &Gateway, token: &str) -> Result<Option<Person>, ApiError> {
    let Some(keys) = gateway
        .access
        .as_ref()
        .and_then(|access| access.keys.clone())
    else {
        return Ok(None);
    };
    let attempt = Run {
        secret: Some(token.to_owned()),
        secure: Some(true),
        ..Attempt::default()
    };
    let authenticator = keys.authenticator.clone();
    let verdict = tokio::task::spawn_blocking(move || authenticator.authenticate(attempt))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    let Verdict::Valid(lookup) = verdict else {
        return Ok(None);
    };
    let now = unix_timestamp()?;
    let record = match gateway.registry.clone() {
        Some(registry) if lookup.by == "id" => {
            let id = lookup.value.clone();
            tokio::task::spawn_blocking(move || registry.api_key(&id))
                .await
                .map_err(|_| ApiError::internal())?
                .map_err(|_| ApiError::internal())?
        }
        _ => None,
    };
    let Some(record) = record else {
        let directory = keys.directory.clone();
        let account = tokio::task::spawn_blocking(move || directory.lookup(lookup))
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|_| ApiError::internal())?;
        // An account behind a personal key whose record is gone has nobody to act as.
        return Ok(account.filter(|account| !account.name.starts_with(PERSONAL_PREFIX)));
    };
    if record.expired_at(now) {
        return Ok(None);
    }
    let Ok(mut person) = person_by_id(gateway, record.owner.clone()).await else {
        return Ok(None);
    };
    let scopes: Vec<KeyScope> = record
        .scopes
        .iter()
        .filter_map(|text| text.parse().ok())
        .collect();
    ai6::narrow_to_key(&mut person, &record.id, &scopes);
    touch(gateway, keys.precision, &record.id, now).await;
    Ok(Some(person))
}

/// Records a use of key `id`, as precisely as the access cache keeps an answer.
async fn touch(gateway: &Gateway, within: f64, id: &str, now: f64) {
    let Some(registry) = gateway.registry.clone() else {
        return;
    };
    let id = id.to_owned();
    // A use that cannot be written down does not refuse the request.
    let _ = tokio::task::spawn_blocking(move || registry.touch_api_key(&id, now, within)).await;
}

/// A key as the routes show it: never its value.
#[derive(Serialize)]
pub(crate) struct KeyView {
    id: String,
    name: String,
    scopes: Vec<String>,
    created_at: f64,
    expires_at: Option<f64>,
    last_used_at: Option<f64>,
    owner_id: String,
    owner_name: String,
}

impl From<ApiKeyRecord> for KeyView {
    fn from(record: ApiKeyRecord) -> Self {
        Self {
            id: record.id,
            name: record.name,
            scopes: record.scopes,
            created_at: record.created_at,
            expires_at: record.expires_at,
            last_used_at: record.last_used_at,
            owner_id: record.owner,
            owner_name: record.owner_name,
        }
    }
}

fn key_provider(gateway: &Gateway) -> Result<KeyProvider, AdminError> {
    gateway
        .access
        .as_ref()
        .and_then(|access| access.keys.clone())
        .ok_or_else(|| AdminError::Conflict("no key provider is configured".to_owned()))
}

fn registry(
    gateway: &Gateway,
) -> Result<Arc<dyn ai6::directorates::registry::backend::RegistryBackend>, AdminError> {
    gateway
        .registry
        .clone()
        .ok_or_else(|| AdminError::Api(ApiError::unavailable()))
}

async fn is_administrator(gateway: &Gateway, person: &Person) -> Result<bool, AdminError> {
    Ok(resource_action_allowed(
        gateway,
        person.clone(),
        CONFIG_ACCOUNTS.to_owned(),
        "account.manage",
    )
    .await?)
}

async fn audit(
    gateway: &Gateway,
    person: &Person,
    action: &str,
    target: &str,
) -> Result<(), AdminError> {
    let registry = registry(gateway)?;
    let record = ai6::AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: actor_of(person),
        action: action.to_owned(),
        operation: String::new(),
        resource: Some(CONFIG_ACCOUNTS.to_owned()),
        target: crate::roster::bounded(target, gateway.config.unex().text.roster_target_bytes),
        at: unix_timestamp()?,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|_| AdminError::Api(ApiError::internal()))
}

#[derive(Deserialize)]
pub(crate) struct ListQuery {
    #[serde(default)]
    all: bool,
}

/// `GET /v1/user/keys`: the signed-in person's keys; with `?all=true`, everyone's, which needs
/// `account.manage` at `config:accounts`.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<KeyView>>, AdminError> {
    let person = resolve_signed_in(&gateway, bearer(&headers)?).await?;
    if query.all && !is_administrator(&gateway, &person).await? {
        return Err(ApiError::forbidden().into());
    }
    let registry = registry(&gateway)?;
    let owner = (!query.all).then(|| person.id.clone());
    let keys = tokio::task::spawn_blocking(move || registry.api_keys(owner.as_deref()))
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|_| AdminError::Api(ApiError::internal()))?;
    Ok(Json(keys.into_iter().map(KeyView::from).collect()))
}

/// What a person asks for a key.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateBody {
    name: String,
    scopes: Vec<String>,
    /// Seconds the key lives; absent takes `[unex] key_ttl`, 0 never expires.
    #[serde(default)]
    expires_in: Option<f64>,
}

/// A new key: its record, and its value, which is shown only here.
#[derive(Serialize)]
pub(crate) struct KeyCreated {
    key: KeyView,
    secret: String,
}

/// The default life of a key whose maker names none, in seconds.
const DEFAULT_KEY_TTL: f64 = 90.0 * 24.0 * 3600.0;

/// `POST /v1/user/keys`: makes a key for the signed-in person, narrowed to `scopes`, each of
/// which the person holds now. A key cannot make a key.
pub(crate) async fn create(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<KeyCreated>), AdminError> {
    let person = resolve_signed_in(&gateway, bearer(&headers)?).await?;
    let keys = key_provider(&gateway)?;
    let registry = registry(&gateway)?;
    let text = &gateway.config.unex().text;
    if body.name.trim().is_empty()
        || !text.access_name_bytes.allows(body.name.len())
        || body.name.chars().any(char::is_control)
    {
        return Err(AdminError::Refused("name the key".to_owned()));
    }
    let mut scopes: Vec<KeyScope> = Vec::new();
    for text in &body.scopes {
        let scope: KeyScope = text
            .parse()
            .map_err(|error: ai6::KeyScopeError| AdminError::Refused(error.to_string()))?;
        if scope.scope == "*" {
            return Err(AdminError::Refused(
                "name the resource a scope covers, not *".to_owned(),
            ));
        }
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    if scopes.is_empty() {
        return Err(AdminError::Refused(
            "name at least one scope, ACTION@SCOPE".to_owned(),
        ));
    }
    for scope in &scopes {
        let action = ACTIONS
            .iter()
            .find(|action| **action == scope.action)
            .copied()
            .ok_or_else(|| AdminError::Refused(format!("{:?} is not an action", scope.action)))?;
        if !resource_action_allowed(&gateway, person.clone(), scope.scope.clone(), action).await? {
            return Err(AdminError::Refused(format!(
                "you do not hold {scope}, so a key cannot"
            )));
        }
    }
    let now = unix_timestamp()?;
    let lifetime = match body.expires_in {
        Some(seconds) if seconds.is_nan() || seconds < 0.0 => {
            return Err(AdminError::Refused(
                "expires_in must be seconds, not negative".to_owned(),
            ));
        }
        Some(seconds) => seconds,
        None => keys.lifetime,
    };
    let expires_at = (lifetime > 0.0).then_some(now + lifetime);
    let account = format!("{PERSONAL_PREFIX}{}", Uuid::new_v4().simple());
    let put = AccountPut {
        name: account.clone(),
        expires_unix: expires_at.map(|at| at.ceil() as i64),
        ..AccountPut::default()
    };
    let (id, secret) = {
        let accounts = keys.accounts.clone();
        let account = account.clone();
        tokio::task::spawn_blocking(move || {
            let secret = accounts.put(put)?.secret;
            let id = accounts
                .accounts()?
                .into_iter()
                .find(|info| info.name == account)
                .map(|info| info.id);
            Ok::<_, ai6::guatiao::library::ProviderError>((id, secret))
        })
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|error| AdminError::Refused(error.to_string()))?
    };
    let (Some(id), Some(secret)) = (id, secret) else {
        remove_account(&keys, &account).await;
        return Err(AdminError::Api(ApiError::internal()));
    };
    let mut record = ApiKeyRecord::new(
        id,
        account.clone(),
        person.id.clone(),
        person
            .display_name
            .clone()
            .unwrap_or_else(|| person.name.clone()),
        body.name.trim(),
        scopes.iter().map(ToString::to_string).collect(),
        now,
    );
    record.expires_at = expires_at;
    let stored = {
        let (registry, record) = (registry.clone(), record.clone());
        tokio::task::spawn_blocking(move || registry.put_api_key(&record))
            .await
            .map_err(|_| AdminError::Api(ApiError::internal()))?
    };
    if stored.is_err() {
        remove_account(&keys, &account).await;
        return Err(AdminError::Api(ApiError::internal()));
    }
    audit(
        &gateway,
        &person,
        "key.create",
        &format!(
            "{} {} ({})",
            record.id,
            record.name,
            record.scopes.join(", ")
        ),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(KeyCreated {
            key: record.into(),
            secret,
        }),
    ))
}

/// Removes the key provider's account `account`, as far as it can be.
async fn remove_account(keys: &KeyProvider, account: &str) {
    let (accounts, account) = (keys.accounts.clone(), account.to_owned());
    let _ = tokio::task::spawn_blocking(move || accounts.remove(account)).await;
}

#[derive(Serialize)]
pub(crate) struct Revoked {
    revoked: bool,
}

/// `DELETE /v1/user/keys/{id}`: ends a key now. A person revokes their own; an administrator
/// (`account.manage` at `config:accounts`) anyone's. Another person's key is not found.
pub(crate) async fn revoke(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Revoked>, AdminError> {
    let person = resolve_signed_in(&gateway, bearer(&headers)?).await?;
    let keys = key_provider(&gateway)?;
    let registry = registry(&gateway)?;
    let found = {
        let (registry, id) = (registry.clone(), id.clone());
        tokio::task::spawn_blocking(move || registry.api_key(&id))
            .await
            .map_err(|_| AdminError::Api(ApiError::internal()))?
            .map_err(|_| AdminError::Api(ApiError::not_found()))?
    };
    let Some(record) = found else {
        return Err(ApiError::not_found().into());
    };
    if record.owner != person.id && !is_administrator(&gateway, &person).await? {
        return Err(ApiError::not_found().into());
    }
    // The account goes first: from that moment the provider refuses the key.
    let (accounts, account) = (keys.accounts.clone(), record.account.clone());
    tokio::task::spawn_blocking(move || accounts.remove(account))
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .or_else(|error| {
            if error.status == ai6::guatiao::Status::GUATIAO_ERR_NOT_FOUND {
                Ok(())
            } else {
                Err(AdminError::Api(ApiError::unavailable()))
            }
        })?;
    {
        let (registry, id) = (registry.clone(), id.clone());
        tokio::task::spawn_blocking(move || registry.remove_api_key(&id))
            .await
            .map_err(|_| AdminError::Api(ApiError::internal()))?
            .map_err(|_| AdminError::Api(ApiError::internal()))?;
    }
    audit(
        &gateway,
        &person,
        "key.revoke",
        &format!("{} {} of {}", record.id, record.name, record.owner),
    )
    .await?;
    Ok(Json(Revoked { revoked: true }))
}

impl std::fmt::Debug for KeyProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("KeyProvider([REDACTED])")
    }
}
