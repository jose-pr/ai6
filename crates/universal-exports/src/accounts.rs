//! Account administration: the accounts an account provider keeps, through the gateway.
//!
//! The provider is the `unex` record's `accounts`, else the authenticator's provider when it
//! offers the accounts kind. Every route needs `account.manage` at `config:accounts`; an
//! account is never deleted through the API, only disabled; a secret is shown once and is
//! never audited or logged.

use std::sync::Arc;

use ai6::{AccountInfo, AccountPut, Accounts, CONFIG_ACCESS, CONFIG_ACCOUNTS, Editor, Person};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::access_admin::{AdminError, live};
use crate::{ApiError, Gateway, authorize_resource_action, bearer, resolve_user, unix_timestamp};

/// An account as the routes show it: never with what proves it.
#[derive(Serialize)]
pub(crate) struct AccountView {
    id: String,
    name: String,
    groups: Vec<String>,
    disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    made_unix: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_unix: Option<i64>,
}

impl From<AccountInfo> for AccountView {
    fn from(info: AccountInfo) -> Self {
        Self {
            id: info.id,
            name: info.name,
            groups: info.groups,
            disabled: info.disabled.unwrap_or(false),
            made_unix: info.made_unix,
            expires_unix: info.expires_unix,
        }
    }
}

/// What a change of an account came to. `secret` is the password made for it, shown once.
#[derive(Serialize)]
pub(crate) struct AccountChanged {
    account: AccountView,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<String>,
}

/// The change a `PUT` asks for; an absent key is left as it is.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountBody {
    groups: Option<Vec<String>>,
    disabled: Option<bool>,
    expires_unix: Option<i64>,
}

fn provider(gateway: &Gateway) -> Result<Arc<dyn Accounts>, AdminError> {
    gateway
        .accounts
        .clone()
        .ok_or_else(|| AdminError::Conflict("no account provider".to_owned()))
}

async fn audit(
    gateway: &Gateway,
    person: &Person,
    action: &str,
    target: &str,
) -> Result<(), AdminError> {
    let registry = gateway
        .registry
        .clone()
        .ok_or_else(|| AdminError::Api(ApiError::unavailable()))?;
    let record = ai6::AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: action.to_owned(),
        operation: String::new(),
        resource: Some(CONFIG_ACCOUNTS.to_owned()),
        target: crate::roster::bounded(target),
        at: unix_timestamp()?,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&record))
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|_| AdminError::Api(ApiError::internal()))
}

async fn listed(accounts: Arc<dyn Accounts>) -> Result<Vec<AccountInfo>, AdminError> {
    tokio::task::spawn_blocking(move || accounts.accounts())
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|_| AdminError::Api(ApiError::unavailable()))
}

/// `GET /v1/user/accounts`: needs `account.manage`.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<Vec<AccountView>>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(
        &gateway,
        person,
        CONFIG_ACCOUNTS.to_owned(),
        "account.manage",
    )
    .await?;
    let accounts = provider(&gateway)?;
    Ok(Json(
        listed(accounts)
            .await?
            .into_iter()
            .map(AccountView::from)
            .collect(),
    ))
}

/// A password for an account: 48 hex characters from the generator the access tokens use.
fn new_secret() -> String {
    let mut secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    secret.truncate(48);
    secret
}

/// The person an account is: its ID and name, in the groups it has.
fn person_of(info: &AccountInfo, groups: &[String]) -> Person {
    Person {
        id: info.id.clone(),
        name: info.name.clone(),
        groups: groups.to_vec(),
        ..Person::default()
    }
}

/// Refuses a change that touches a chief unless a chief makes it: the account is a chief
/// subject now, or would be after the change.
fn chief_guard(
    effective: &ai6::EffectiveAccess,
    editor: &Editor,
    before: Option<&AccountInfo>,
    name: &str,
    groups_after: &[String],
) -> Result<(), AdminError> {
    if editor.chief {
        return Ok(());
    }
    let now = before.map_or_else(
        || Person {
            name: name.to_owned(),
            ..Person::default()
        },
        |info| person_of(info, &info.groups),
    );
    let after = before.map_or_else(
        || Person {
            name: name.to_owned(),
            groups: groups_after.to_vec(),
            ..Person::default()
        },
        |info| person_of(info, groups_after),
    );
    if effective.is_chief(&now) || effective.is_chief(&after) {
        return Err(AdminError::Refused(
            "only a chief changes an account that is chief".to_owned(),
        ));
    }
    Ok(())
}

/// `PUT /v1/user/accounts/{name}`: makes the account or changes it. A `groups` change also
/// needs `access.manage` at `config:access`, since groups confer roles. A new account gets a
/// password, shown once.
pub(crate) async fn put_account(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(body): Json<AccountBody>,
) -> Result<Json<AccountChanged>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        CONFIG_ACCOUNTS.to_owned(),
        "account.manage",
    )
    .await?;
    checked_name(&name)?;
    let accounts = provider(&gateway)?;
    let live = live(&gateway)?;
    let effective = live.effective();
    let editor = Editor::new(person.clone(), &effective);
    let existing = listed(accounts.clone())
        .await?
        .into_iter()
        .find(|info| info.name == name);
    if body.groups.is_some()
        && !effective.holds(
            &person,
            "access.manage",
            CONFIG_ACCESS,
            &ai6::Lineage::default(),
        )
    {
        audit(
            &gateway,
            &person,
            "account.refused",
            &format!("{name}: groups need access.manage"),
        )
        .await?;
        return Err(AdminError::Refused(
            "changing an account's groups needs access.manage on the global policy".to_owned(),
        ));
    }
    let groups_after = body
        .groups
        .clone()
        .or_else(|| existing.as_ref().map(|info| info.groups.clone()))
        .unwrap_or_default();
    if let Err(error) = chief_guard(&effective, &editor, existing.as_ref(), &name, &groups_after) {
        audit(
            &gateway,
            &person,
            "account.refused",
            &format!("{name}: chief"),
        )
        .await?;
        return Err(error);
    }
    let creating = existing.is_none();
    let put = AccountPut {
        name: name.clone(),
        groups: body.groups,
        disabled: body.disabled,
        expires_unix: body.expires_unix,
        ..AccountPut::default()
    };
    let secret = {
        let (accounts, name) = (accounts.clone(), name.clone());
        tokio::task::spawn_blocking(move || {
            let made = accounts.put(put)?.secret;
            if !creating || made.is_some() {
                return Ok::<_, ai6::guatiao::library::ProviderError>(made);
            }
            let secret = new_secret();
            accounts.put(AccountPut {
                name,
                secret: Some(secret.clone()),
                ..AccountPut::default()
            })?;
            Ok(Some(secret))
        })
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|error| AdminError::Refused(error.to_string()))?
    };
    audit(
        &gateway,
        &person,
        "account.update",
        &format!("{name}{}", if creating { " (new)" } else { "" }),
    )
    .await?;
    let account = changed_account(accounts, &name).await?;
    Ok(Json(AccountChanged { account, secret }))
}

/// `POST /v1/user/accounts/{name}/secret`: sets a new password for the account (or takes the
/// one its provider makes) and shows it once; it is in no audit record or log.
pub(crate) async fn reset_secret(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<AccountChanged>, AdminError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    authorize_resource_action(
        &gateway,
        person.clone(),
        CONFIG_ACCOUNTS.to_owned(),
        "account.manage",
    )
    .await?;
    checked_name(&name)?;
    let accounts = provider(&gateway)?;
    let live = live(&gateway)?;
    let effective = live.effective();
    let editor = Editor::new(person.clone(), &effective);
    let Some(existing) = listed(accounts.clone())
        .await?
        .into_iter()
        .find(|info| info.name == name)
    else {
        return Err(AdminError::Api(ApiError::not_found()));
    };
    if let Err(error) = chief_guard(
        &effective,
        &editor,
        Some(&existing),
        &name,
        &existing.groups,
    ) {
        audit(
            &gateway,
            &person,
            "account.refused",
            &format!("{name}: chief"),
        )
        .await?;
        return Err(error);
    }
    let secret = {
        let (accounts, name) = (accounts.clone(), name.clone());
        let candidate = new_secret();
        tokio::task::spawn_blocking(move || {
            let outcome = accounts.put(AccountPut {
                name,
                secret: Some(candidate.clone()),
                ..AccountPut::default()
            })?;
            Ok::<_, ai6::guatiao::library::ProviderError>(outcome.secret.unwrap_or(candidate))
        })
        .await
        .map_err(|_| AdminError::Api(ApiError::internal()))?
        .map_err(|error| AdminError::Refused(error.to_string()))?
    };
    audit(&gateway, &person, "account.secret", &name).await?;
    let account = changed_account(accounts, &name).await?;
    Ok(Json(AccountChanged {
        account,
        secret: Some(secret),
    }))
}

async fn changed_account(
    accounts: Arc<dyn Accounts>,
    name: &str,
) -> Result<AccountView, AdminError> {
    listed(accounts)
        .await?
        .into_iter()
        .find(|info| info.name == name)
        .map(AccountView::from)
        .ok_or(AdminError::Api(ApiError::internal()))
}

fn checked_name(name: &str) -> Result<(), AdminError> {
    if name.trim().is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        Err(AdminError::Api(ApiError::bad_request()))
    } else {
        Ok(())
    }
}
