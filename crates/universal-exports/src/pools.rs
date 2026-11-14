//! The account pool board: the pool accounts an Operation may draw from, who uses them, how
//! much provider quota they have left and until when they are limited.
//!
//! It is read from the accounts Stations record in the Registry. It never carries a
//! credential, the secret holding one, or the directory of a login.

use std::collections::BTreeMap;

use ai6::{AccountRef, AccountState, AuditRecord};
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    ApiError, Gateway, GatewayError, authorize_operation_action, bearer, resolve_user,
    unix_timestamp,
};

/// One quota window as the board shows it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardWindow {
    /// The window's name (`five_hour`, `weekly`, ...).
    pub name: String,
    /// Thousandths used, when the provider reported a figure.
    pub used_permille: Option<u16>,
    /// When it resets, as Unix seconds.
    pub resets_at: Option<f64>,
    /// The provider's status for it, when reported.
    pub status: Option<String>,
    /// The models it covers, when the provider names them; empty for every model.
    pub models: Vec<String>,
    /// When it was last read, as Unix seconds.
    pub observed_at: f64,
}

/// One Run using an account.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardClaim {
    /// The Station running it.
    pub station_id: String,
    /// Its Mission.
    pub mission_id: String,
    /// Since when, as Unix seconds.
    pub since: f64,
}

/// What the board shows of a login kept sealed in the Registry: never the login.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardLogin {
    /// The Stations it is sealed to.
    pub stations: Vec<String>,
    /// When it ends, as Unix seconds, when its files say.
    pub expires_at: Option<f64>,
    /// When a renewed copy last reached the Registry, as Unix seconds.
    pub renewed_at: Option<f64>,
    /// The Station holding its renewal lease now.
    pub renewal_held_by: Option<String>,
    /// When that lease lapses unless it is renewed, as Unix seconds.
    pub renewal_held_until: Option<f64>,
    /// How the last renewal ended: `renewed`, `unchanged`, `failed: <why>` or `added`.
    pub last_result: Option<String>,
    /// When it ended, as Unix seconds.
    pub last_attempt_at: Option<f64>,
    /// Renewals that failed since the last that did not.
    pub failures: u32,
    /// Why a person is needed, when one is.
    pub needs_person: Option<String>,
    /// What to run on a host where the CLI is signed in, when a person is needed.
    pub needs_person_command: Option<String>,
    /// Changes a person asked for that wait for a Station holding the login.
    pub requests: Vec<BoardLoginRequest>,
}

/// A change to a login waiting for a Station that holds it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardLoginRequest {
    /// `add_station`, `remove_station` or `sign_out_and_forget`.
    pub action: String,
    /// The Station it is about, for the first two.
    pub station: Option<String>,
    /// Who asked.
    pub by: String,
    /// When, as Unix seconds.
    pub at: f64,
}

/// The login as the board shows it: what is kept, or that an account declared to keep one has none.
fn board_login(
    login: Option<&ai6::LoginState>,
    sealed: bool,
    account: &AccountRef,
    handler: &str,
    now: f64,
) -> Option<BoardLogin> {
    let fix = || ai6::directorates::q::login_standing::add_login_command(account, handler);
    let Some(login) = login else {
        return sealed.then(|| BoardLogin {
            stations: Vec::new(),
            expires_at: None,
            renewed_at: None,
            renewal_held_by: None,
            renewal_held_until: None,
            last_result: None,
            last_attempt_at: None,
            failures: 0,
            needs_person: Some("no login is kept for this account".to_owned()),
            needs_person_command: Some(fix()),
            requests: Vec::new(),
        });
    };
    let lease = login.leased_at(now);
    Some(BoardLogin {
        stations: login.stations.clone(),
        expires_at: login.expires_at,
        renewed_at: login.renewed_at,
        renewal_held_by: lease.map(|lease| lease.station.clone()),
        renewal_held_until: lease.map(|lease| lease.until),
        last_result: login.last_result.clone(),
        last_attempt_at: login.last_attempt_at,
        failures: login.failures,
        needs_person: login.needs_person.clone(),
        needs_person_command: login.needs_person.is_some().then(fix),
        requests: login
            .requests
            .iter()
            .map(|asked| {
                let (action, station) = match &asked.action {
                    ai6::LoginRequestAction::AddStation { station } => {
                        ("add_station", Some(station.clone()))
                    }
                    ai6::LoginRequestAction::RemoveStation { station } => {
                        ("remove_station", Some(station.clone()))
                    }
                    ai6::LoginRequestAction::SignOutAndForget => ("sign_out_and_forget", None),
                    _ => ("change", None),
                };
                BoardLoginRequest {
                    action: action.to_owned(),
                    station,
                    by: asked.by.clone(),
                    at: asked.at,
                }
            })
            .collect(),
    })
}

/// One pool account on the board.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardAccount {
    /// The pool ID.
    pub pool: String,
    /// The account's name in the pool.
    pub account: String,
    /// The AgentHandler the pool serves.
    pub handler: String,
    /// How the harness reads its credential (`claude-oauth-token`, ...).
    pub kind: String,
    /// Its priority.
    pub priority: i64,
    /// The most Runs that may use it at once.
    pub max_concurrent: Option<usize>,
    /// Labels configured for it.
    pub labels: BTreeMap<String, String>,
    /// This Operation's Runs using it now.
    pub active: Vec<BoardClaim>,
    /// How many Runs of other Operations use it now: who they are is theirs to see.
    pub other_runs: usize,
    /// The quota windows last reported.
    pub windows: Vec<BoardWindow>,
    /// Until when its provider refuses work, as Unix seconds, while it does.
    pub limited_until: Option<f64>,
    /// When a Run last took it, as Unix seconds.
    pub last_used: Option<f64>,
    /// Tokens its Runs used today (UTC).
    pub tokens_today: u64,
    /// Its login, when one is kept sealed in the Registry.
    pub login: Option<BoardLogin>,
}

/// The board for one Operation.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PoolBoard {
    /// The Operation.
    pub operation_id: String,
    /// Its accounts, by pool and name.
    pub accounts: Vec<BoardAccount>,
}

/// The board entry for `state` at `now`, if a Station declared the account for `operation`.
fn board_account(state: AccountState, operation: &str, now: f64) -> Option<BoardAccount> {
    let profile = state.profile()?;
    if !profile.operations.iter().any(|id| id == operation) {
        return None;
    }
    let (own, others): (Vec<_>, Vec<_>) = state
        .live_claims(now)
        .partition(|claim| claim.operation.as_deref() == Some(operation));
    let login = board_login(
        state.login.as_ref(),
        profile.sealed_login,
        &state.account,
        &profile.handler,
        now,
    );
    Some(BoardAccount {
        login,
        tokens_today: state.tokens_on_day(now as i64),
        active: own
            .into_iter()
            .map(|claim| BoardClaim {
                station_id: claim.station.clone(),
                mission_id: claim.mission.clone(),
                since: claim.since,
            })
            .collect(),
        other_runs: others.len(),
        windows: state
            .windows
            .iter()
            .map(|window| BoardWindow {
                name: window.name.clone(),
                used_permille: window.used_permille,
                resets_at: window.resets_at,
                status: window.status.clone(),
                models: window.models.clone(),
                observed_at: window.observed_at,
            })
            .collect(),
        limited_until: state.limited_until.filter(|until| *until > now),
        last_used: state.last_used,
        pool: state.account.pool,
        account: state.account.account,
        handler: profile.handler,
        kind: profile.kind,
        priority: profile.priority,
        max_concurrent: profile.max_concurrent,
        labels: profile.labels,
    })
}

/// `GET /v1/user/operations/{operation_id}/pools`: needs `pool.read` on the Operation.
pub(crate) async fn operation_pools(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<PoolBoard>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person, &operation, "pool.read").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let now = unix_timestamp()?;
    let operation = operation.to_string();
    let states = tokio::task::spawn_blocking(move || registry.accounts())
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(Json(PoolBoard {
        accounts: states
            .into_iter()
            .filter_map(|state| board_account(state, &operation, now))
            .collect(),
        operation_id: operation,
    }))
}

/// `POST /v1/user/operations/{operation_id}/pools/{pool}/{account}/clear-limit`: lifts an
/// account's provider limit, for an account declared for the Operation. Needs `pool.manage`
/// on the Operation and is audited (`pool.limit.clear`, target `pool/account`).
pub(crate) async fn clear_limit(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, pool, account)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    let account = AccountRef::new(pool, account);
    authorize_operation_action(&gateway, person.clone(), &operation, "pool.manage").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let operation_name = operation.to_string();
    let target = account.clone();
    tokio::task::spawn_blocking(move || {
        let declared = registry
            .accounts()
            .map_err(|_| ApiError::internal())?
            .into_iter()
            .find(|state| state.account == target)
            .and_then(|state| state.profile())
            .is_some_and(|profile| profile.operations.contains(&operation_name));
        if !declared {
            return Err(ApiError::from(GatewayError::NotFound));
        }
        registry
            .clear_account_limit(&target)
            .map(|_| ())
            .map_err(|_| ApiError::bad_request())
    })
    .await
    .map_err(|_| ApiError::internal())??;
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: crate::actor_of(&person),
        action: "pool.limit.clear".to_owned(),
        operation: operation.to_string(),
        resource: None,
        target: account.to_string(),
        at: unix_timestamp()?,
    };
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    tokio::task::spawn_blocking(move || registry.record_audit(&audit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

/// The account of `{pool}/{account}` declared for the Operation, for a person who may manage it.
async fn manageable(
    gateway: &Gateway,
    headers: &HeaderMap,
    operation_id: String,
    pool: String,
    account: String,
) -> Result<(ai6::Person, OperationId, AccountRef), ApiError> {
    let person = resolve_user(gateway, bearer(headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    let account = AccountRef::new(pool, account);
    authorize_operation_action(gateway, person.clone(), &operation, "pool.manage").await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let (name, target) = (operation.to_string(), account.clone());
    let declared = tokio::task::spawn_blocking(move || {
        registry.accounts().map(|states| {
            states
                .into_iter()
                .find(|state| state.account == target)
                .and_then(|state| state.profile())
                .is_some_and(|profile| profile.operations.contains(&name))
        })
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    if !declared {
        return Err(ApiError::from(GatewayError::NotFound));
    }
    Ok((person, operation, account))
}

/// Refuses with 404 when `account` holds no login, before anything is audited or queued.
async fn require_login(gateway: &Gateway, account: &AccountRef) -> Result<(), ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let target = account.clone();
    let held = tokio::task::spawn_blocking(move || {
        ai6::directorates::q::login_standing::holds_login(registry.as_ref(), &target)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    if held {
        Ok(())
    } else {
        Err(ApiError::from(GatewayError::NotFound))
    }
}

/// Writes the audit record of a change to a login; the change waits for it.
async fn audit_login_change(
    gateway: &Gateway,
    person: &ai6::Person,
    operation: &OperationId,
    action: &str,
    target: String,
) -> Result<(), ApiError> {
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: crate::actor_of(person),
        action: action.to_owned(),
        operation: operation.to_string(),
        resource: None,
        target,
        at: unix_timestamp()?,
    };
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    tokio::task::spawn_blocking(move || registry.record_audit(&audit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())
}

/// Queues a change a Station holding the login makes: sealing needs its key.
async fn queue_login_change(
    gateway: &Gateway,
    person: &ai6::Person,
    account: AccountRef,
    action: ai6::LoginRequestAction,
) -> Result<StatusCode, ApiError> {
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let (by, now) = (crate::actor_of(person), unix_timestamp()?);
    tokio::task::spawn_blocking(move || {
        ai6::directorates::q::login_standing::request(registry.as_ref(), &account, action, &by, now)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?
    .then_some(StatusCode::ACCEPTED)
    .ok_or_else(|| ApiError::from(GatewayError::NotFound))
}

/// A Station ID as a path or body carries it: not empty, and nothing a log line could be split by.
fn station_name(station: String) -> Result<String, ApiError> {
    if station.is_empty() || station.contains(char::is_control) {
        return Err(ApiError::bad_request());
    }
    Ok(station)
}

/// The body of an add-Station request.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddStation {
    station: String,
}

/// `POST /v1/user/operations/{operation_id}/pools/{pool}/{account}/login/stations`: lets one more
/// Station hold the account's login. A Station that holds it seals it to the new one at its next
/// check-in, so the answer is 202. Needs `pool.manage`; audited (`pool.login.station.add`) before
/// it is queued.
pub(crate) async fn add_login_station(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, pool, account)): Path<(String, String, String)>,
    Json(body): Json<AddStation>,
) -> Result<StatusCode, ApiError> {
    let station = station_name(body.station)?;
    let (person, operation, account) =
        manageable(&gateway, &headers, operation_id, pool, account).await?;
    require_login(&gateway, &account).await?;
    audit_login_change(
        &gateway,
        &person,
        &operation,
        "pool.login.station.add",
        format!("{account} {station}"),
    )
    .await?;
    queue_login_change(
        &gateway,
        &person,
        account,
        ai6::LoginRequestAction::AddStation { station },
    )
    .await
}

/// `DELETE /v1/user/operations/{operation_id}/pools/{pool}/{account}/login/stations/{station}`:
/// stops sealing the login to a Station, which drops its copy at the next check-in of a Station
/// that holds it (202). Needs `pool.manage`; audited (`pool.login.station.remove`) first.
pub(crate) async fn remove_login_station(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, pool, account, station)): Path<(String, String, String, String)>,
) -> Result<StatusCode, ApiError> {
    let station = station_name(station)?;
    let (person, operation, account) =
        manageable(&gateway, &headers, operation_id, pool, account).await?;
    require_login(&gateway, &account).await?;
    audit_login_change(
        &gateway,
        &person,
        &operation,
        "pool.login.station.remove",
        format!("{account} {station}"),
    )
    .await?;
    queue_login_change(
        &gateway,
        &person,
        account,
        ai6::LoginRequestAction::RemoveStation { station },
    )
    .await
}

/// `?sign_out=` of a login removal.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForgetLogin {
    #[serde(default)]
    sign_out: bool,
}

/// `DELETE /v1/user/operations/{operation_id}/pools/{pool}/{account}/login`: forgets the account's
/// login (204). With `sign_out=true` a Station holding it first signs it out at the provider with
/// the account's `logout_command`, so it is queued (202) and kept if that fails. Needs
/// `pool.manage`; audited (`pool.login.forget` or `pool.login.sign_out`) before it is made or
/// queued.
pub(crate) async fn forget_login(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, pool, account)): Path<(String, String, String)>,
    axum::extract::Query(query): axum::extract::Query<ForgetLogin>,
) -> Result<StatusCode, ApiError> {
    let (person, operation, account) =
        manageable(&gateway, &headers, operation_id, pool, account).await?;
    require_login(&gateway, &account).await?;
    if query.sign_out {
        audit_login_change(
            &gateway,
            &person,
            &operation,
            "pool.login.sign_out",
            account.to_string(),
        )
        .await?;
        return queue_login_change(
            &gateway,
            &person,
            account,
            ai6::LoginRequestAction::SignOutAndForget,
        )
        .await;
    }
    audit_login_change(
        &gateway,
        &person,
        &operation,
        "pool.login.forget",
        account.to_string(),
    )
    .await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    tokio::task::spawn_blocking(move || {
        ai6::directorates::q::login_standing::forget(registry.as_ref(), &account)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?
    .then_some(StatusCode::NO_CONTENT)
    .ok_or_else(|| ApiError::from(GatewayError::NotFound))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ai6::directorates::registry::backend::*;
    use ai6::{
        Account, AccountClaimRequest, AccountKind, AccountPool, AccountProfile, AccountRef,
        QuotaWindow,
    };
    use axum::http::StatusCode;

    use crate::controls::tests::{Served, served_with};

    const ROLES: &str = r#"
        [role.pools]
        actions = ["pool.read"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "pools"

        [role.pool-managers]
        actions = ["pool.read", "pool.manage"]

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "pool-managers"
    "#;

    /// Declares `team/alpha` (two Runs at once) for G and `team/beta` for L only.
    fn declare(served: &Served) -> AccountPool {
        let pool = AccountPool::new("team", "claude")
            .account(
                Account::new("alpha", AccountKind::ClaudeOauthToken)
                    .secret("SECRET-NAME")
                    .max_concurrent(2),
            )
            .account(Account::new("beta", AccountKind::ConfigDir).dir("/SECRET-DIR"));
        for (name, operations) in [("alpha", vec!["G"]), ("beta", vec!["L"])] {
            served
                .registry
                .declare_account(
                    &AccountRef::new("team", name),
                    "worker",
                    &AccountProfile::of(
                        &pool,
                        name,
                        operations.into_iter().map(Into::into).collect(),
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        pool
    }

    #[tokio::test]
    async fn the_board_shows_a_login_s_standing_and_nothing_of_the_login() {
        use std::sync::Arc;

        use ai6::directorates::q::logins::add_login;
        use ai6::directorates::q::sealing::{Recipient, SealingKey, Vault};
        use ai6::directorates::registry::sealed::StationSealingKey;

        const PLANTED: &str = "planted-refresh-0451";
        let served = served_with(ROLES).await;
        let pool = AccountPool::new("free", "acp")
            .account(Account::new("shared", AccountKind::HarnessLogin).files([".fake/auth.json"]));
        let account = AccountRef::new("free", "shared");
        served
            .registry
            .declare_account(
                &account,
                "worker",
                &AccountProfile::of(&pool, "shared", vec!["G".to_owned()]).unwrap(),
            )
            .unwrap();
        let key = Arc::new(SealingKey::generate());
        served
            .registry
            .publish_sealing_key(&StationSealingKey::new(
                "worker",
                key.public_key().to_text(),
            ))
            .unwrap();
        let vault = Vault::new(
            Arc::new(ai6::FileRegistry::open(served.registry.root())),
            "worker",
            key,
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".fake")).unwrap();
        std::fs::write(
            dir.path().join(".fake/auth.json"),
            format!(r#"{{"refresh_token":"{PLANTED}","expires_at":1890000000}}"#),
        )
        .unwrap();
        add_login(
            &vault,
            &account,
            dir.path(),
            &[".fake/auth.json".to_owned()],
            &[Recipient::named("worker")],
            1_790_000_000.0,
        )
        .unwrap();

        let response = board(&served, &served.viewer, "G").await;
        assert_eq!(response.status(), StatusCode::OK);
        let text = response.text().await.unwrap();
        assert!(!text.contains(PLANTED), "{text}");
        assert!(!text.contains("auth.json"), "{text}");
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        let login = &body["accounts"][0]["login"];
        assert_eq!(login["stations"], serde_json::json!(["worker"]));
        assert_eq!(login["expires_at"], 1_890_000_000.0);
        assert_eq!(login["renewed_at"], 1_790_000_000.0);
        assert_eq!(login["renewal_held_by"], serde_json::Value::Null);
        assert_eq!(login["needs_person"], serde_json::Value::Null);
    }

    async fn board(served: &Served, token: &str, operation: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(format!(
                "{}/v1/user/operations/{operation}/pools",
                served.base
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_board_shows_an_operations_accounts_and_never_a_credential() {
        let served = served_with(ROLES).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            // Whole seconds: serde_json does not round-trip every f64 exactly.
            .floor();
        let pool = declare(&served);
        let alpha = AccountRef::new("team", "alpha");
        for (mission, operation) in [("mission-g", "G"), ("SECRET-MISSION-L", "L")] {
            served
                .registry
                .claim_account(
                    &AccountClaimRequest::new(&pool, "worker", mission, Duration::from_secs(60))
                        .operation(Some(operation.to_owned()))
                        .only("alpha"),
                    now,
                )
                .unwrap()
                .unwrap();
        }
        served
            .registry
            .report_quota(
                &alpha,
                &[QuotaWindow::new("five_hour", now)
                    .used_permille(640)
                    .resets_at(now + 600.0)],
            )
            .unwrap();
        served.registry.limit_account(&alpha, now + 600.0).unwrap();
        served
            .registry
            .record_account_usage(&alpha, 1_234, now as i64)
            .unwrap();

        let response = board(&served, &served.viewer, "G").await;
        assert_eq!(response.status(), StatusCode::OK);
        let shown: serde_json::Value = response.json().await.unwrap();
        let text = shown.to_string();
        assert!(!text.contains("SECRET"), "{text}");
        assert_eq!(shown["operation_id"], "G");
        let accounts = shown["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 1, "beta is not open to G");
        let alpha = &accounts[0];
        assert_eq!(alpha["pool"], "team");
        assert_eq!(alpha["account"], "alpha");
        assert_eq!(alpha["kind"], "claude-oauth-token");
        assert_eq!(alpha["max_concurrent"], 2);
        assert_eq!(alpha["active"].as_array().unwrap().len(), 1);
        assert_eq!(alpha["active"][0]["mission_id"], "mission-g");
        assert_eq!(alpha["active"][0]["station_id"], "worker");
        assert_eq!(
            alpha["other_runs"], 1,
            "another Operation's Run is only counted"
        );
        assert_eq!(alpha["windows"][0]["used_permille"], 640);
        assert_eq!(alpha["windows"][0]["observed_at"], now);
        assert_eq!(alpha["windows"][0]["models"], serde_json::json!([]));
        assert_eq!(alpha["limited_until"], now + 600.0);
        assert_eq!(alpha["tokens_today"], 1_234);
        // Sorted: the ACP crates turn on serde_json's `preserve_order`.
        let mut keys = alpha
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(
            keys,
            [
                "account",
                "active",
                "handler",
                "kind",
                "labels",
                "last_used",
                "limited_until",
                "login",
                "max_concurrent",
                "other_runs",
                "pool",
                "priority",
                "tokens_today",
                "windows",
            ],
            "no field carries a secret name or a login directory"
        );
    }

    #[tokio::test]
    async fn clearing_a_limit_needs_pool_manage_and_is_audited() {
        let served = served_with(ROLES).await;
        declare(&served);
        let alpha = AccountRef::new("team", "alpha");
        served
            .registry
            .limit_account(&alpha, 4_000_000_000.0)
            .unwrap();
        let clear = |token: String, account: &'static str| {
            let base = served.base.clone();
            async move {
                reqwest::Client::new()
                    .post(format!(
                        "{base}/v1/user/operations/G/pools/team/{account}/clear-limit"
                    ))
                    .bearer_auth(token)
                    .send()
                    .await
                    .unwrap()
                    .status()
            }
        };
        assert_eq!(
            clear(served.viewer.clone(), "alpha").await,
            StatusCode::FORBIDDEN,
            "pool.read is not enough"
        );
        assert_eq!(
            clear(served.operator.clone(), "beta").await,
            StatusCode::NOT_FOUND,
            "beta is not declared for G"
        );
        assert_eq!(
            clear(served.operator.clone(), "alpha").await,
            StatusCode::NO_CONTENT
        );
        let state = served
            .registry
            .accounts()
            .unwrap()
            .into_iter()
            .find(|state| state.account == alpha)
            .unwrap();
        assert_eq!(state.limited_until, None);
        let audit = served.registry.audit_records().unwrap();
        assert!(
            audit
                .iter()
                .any(|record| record.action == "pool.limit.clear"
                    && record.target == "team/alpha"
                    && record.actor == "operator-007"
                    && record.operation == "G")
        );
    }

    /// `free/shared`, a login account declared for G, and `free/other`, declared for L only.
    fn declare_login(served: &Served) -> AccountRef {
        let pool = AccountPool::new("free", "acp")
            .account(Account::new("shared", AccountKind::HarnessLogin).files([".fake/auth.json"]))
            .account(Account::new("other", AccountKind::HarnessLogin).files([".fake/auth.json"]));
        for (name, operation) in [("shared", "G"), ("other", "L")] {
            served
                .registry
                .declare_account(
                    &AccountRef::new("free", name),
                    "worker",
                    &AccountProfile::of(&pool, name, vec![operation.to_owned()]).unwrap(),
                )
                .unwrap();
        }
        AccountRef::new("free", "shared")
    }

    fn login_state(served: &Served, account: &AccountRef) -> Option<ai6::LoginState> {
        served
            .registry
            .accounts()
            .unwrap()
            .into_iter()
            .find(|state| state.account == *account)
            .and_then(|state| state.login)
    }

    /// Gives `account` a login standing, with no sealed value: the gateway holds no key.
    fn give_login(served: &Served, account: &AccountRef, change: impl Fn(&mut ai6::LoginState)) {
        served
            .registry
            .change_login(account, &mut |state| {
                let mut login = ai6::LoginState::default();
                login.stations = vec!["worker".to_owned()];
                login.revision = 1;
                change(&mut login);
                *state = Some(login);
            })
            .unwrap();
    }

    #[tokio::test]
    async fn the_board_says_when_a_login_needs_a_person_and_what_to_run() {
        let served = served_with(ROLES).await;
        let account = declare_login(&served);
        // Declared to keep a login, and none was added.
        let response = board(&served, &served.viewer, "G").await;
        let body: serde_json::Value = response.json().await.unwrap();
        let login = &body["accounts"][0]["login"];
        assert_eq!(login["stations"], serde_json::json!([]));
        assert_eq!(login["needs_person"], "no login is kept for this account");
        assert_eq!(
            login["needs_person_command"],
            "ai6 pools add-login free shared --handler acp --replace"
        );

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            .floor();
        give_login(&served, &account, |login| {
            login.expires_at = Some(now + 600.0);
            login.renewed_at = Some(now - 60.0);
            login.last_result = Some("failed: its harness removed its login file".to_owned());
            login.last_attempt_at = Some(now - 30.0);
            login.failures = 3;
            login.needs_person = Some("3 renewals failed in a row".to_owned());
            login.lease = Some(
                serde_json::from_value(
                    serde_json::json!({"station": "worker", "until": now + 120.0}),
                )
                .unwrap(),
            );
        });
        let body: serde_json::Value = board(&served, &served.viewer, "G")
            .await
            .json()
            .await
            .unwrap();
        let login = &body["accounts"][0]["login"];
        assert_eq!(login["stations"], serde_json::json!(["worker"]));
        assert_eq!(login["renewal_held_by"], "worker");
        assert_eq!(login["renewal_held_until"], now + 120.0);
        assert_eq!(login["last_attempt_at"], now - 30.0);
        assert!(
            login["last_result"]
                .as_str()
                .unwrap()
                .starts_with("failed: ")
        );
        assert_eq!(login["failures"], 3);
        assert_eq!(login["needs_person"], "3 renewals failed in a row");
        assert_eq!(
            login["needs_person_command"],
            "ai6 pools add-login free shared --handler acp --replace"
        );

        // Healthy: no person, no command.
        give_login(&served, &account, |login| {
            login.last_result = Some("renewed".to_owned());
        });
        let body: serde_json::Value = board(&served, &served.viewer, "G")
            .await
            .json()
            .await
            .unwrap();
        let login = &body["accounts"][0]["login"];
        assert_eq!(login["needs_person"], serde_json::Value::Null);
        assert_eq!(login["needs_person_command"], serde_json::Value::Null);
    }

    async fn change(
        served: &Served,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> StatusCode {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}/v1/user/operations/{path}", served.base))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await.unwrap().status()
    }

    #[tokio::test]
    async fn stations_and_the_login_are_changed_with_pool_manage_and_audited_first() {
        use reqwest::Method;
        let served = served_with(ROLES).await;
        let account = declare_login(&served);
        let stations = "G/pools/free/shared/login/stations";
        let add = || serde_json::json!({"station": "host-b"});

        assert_eq!(
            change(&served, &served.viewer, Method::POST, stations, Some(add())).await,
            StatusCode::FORBIDDEN,
            "pool.read is not enough"
        );
        assert_eq!(
            change(
                &served,
                &served.operator,
                Method::POST,
                "G/pools/free/other/login/stations",
                Some(add())
            )
            .await,
            StatusCode::NOT_FOUND,
            "free/other is not declared for G"
        );
        assert_eq!(
            change(
                &served,
                &served.operator,
                Method::POST,
                stations,
                Some(add())
            )
            .await,
            StatusCode::NOT_FOUND,
            "there is no login to change"
        );
        assert!(
            served
                .registry
                .audit_records()
                .unwrap()
                .iter()
                .all(|record| !record.action.starts_with("pool.login")),
            "a refused request leaves no trace beyond the refusal"
        );

        give_login(&served, &account, |_| {});
        assert_eq!(
            change(
                &served,
                &served.operator,
                Method::POST,
                stations,
                Some(add())
            )
            .await,
            StatusCode::ACCEPTED
        );
        assert_eq!(
            change(
                &served,
                &served.operator,
                Method::DELETE,
                &format!("{stations}/worker"),
                None
            )
            .await,
            StatusCode::ACCEPTED
        );
        let login = login_state(&served, &account).unwrap();
        let asked: Vec<_> = login.requests.iter().map(|asked| &asked.action).collect();
        assert_eq!(
            asked,
            [
                &ai6::LoginRequestAction::AddStation {
                    station: "host-b".to_owned()
                },
                &ai6::LoginRequestAction::RemoveStation {
                    station: "worker".to_owned()
                },
            ]
        );
        assert_eq!(
            login.stations,
            ["worker"],
            "a Station with the key makes it"
        );
        let body: serde_json::Value = board(&served, &served.viewer, "G")
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(
            body["accounts"][0]["login"]["requests"][0],
            serde_json::json!({"action": "add_station", "station": "host-b",
                "by": "operator-007", "at": body["accounts"][0]["login"]["requests"][0]["at"]})
        );

        // Signing out waits for a Station; forgetting alone is done at once.
        assert_eq!(
            change(
                &served,
                &served.operator,
                Method::DELETE,
                "G/pools/free/shared/login?sign_out=true",
                None
            )
            .await,
            StatusCode::ACCEPTED
        );
        assert!(login_state(&served, &account).is_some());
        assert_eq!(
            change(
                &served,
                &served.operator,
                Method::DELETE,
                "G/pools/free/shared/login",
                None
            )
            .await,
            StatusCode::NO_CONTENT
        );
        assert!(login_state(&served, &account).is_none());
        assert_eq!(
            change(
                &served,
                &served.operator,
                Method::DELETE,
                "G/pools/free/shared/login",
                None
            )
            .await,
            StatusCode::NOT_FOUND,
            "it holds none now"
        );
        let mut made: Vec<(String, String)> = served
            .registry
            .audit_records()
            .unwrap()
            .into_iter()
            .filter(|record| record.action.starts_with("pool.login"))
            .inspect(|record| {
                assert_eq!(record.actor, "operator-007");
                assert_eq!(record.operation, "G");
            })
            .map(|record| (record.action, record.target))
            .collect();
        made.sort();
        assert_eq!(
            made,
            [
                ("pool.login.forget".to_owned(), "free/shared".to_owned()),
                ("pool.login.sign_out".to_owned(), "free/shared".to_owned()),
                (
                    "pool.login.station.add".to_owned(),
                    "free/shared host-b".to_owned()
                ),
                (
                    "pool.login.station.remove".to_owned(),
                    "free/shared worker".to_owned()
                ),
            ]
        );
    }

    #[tokio::test]
    async fn a_refused_audit_refuses_the_change() {
        use reqwest::Method;
        let served = served_with(ROLES).await;
        let account = declare_login(&served);
        give_login(&served, &account, |_| {});
        // The audit log cannot be written: a file stands where its directory belongs.
        let audit = served.registry.root().join("audit");
        let _ = std::fs::remove_dir_all(&audit);
        std::fs::write(&audit, b"not a directory").unwrap();
        for (method, path, body) in [
            (
                Method::POST,
                "G/pools/free/shared/login/stations",
                Some(serde_json::json!({"station": "host-b"})),
            ),
            (
                Method::DELETE,
                "G/pools/free/shared/login/stations/worker",
                None,
            ),
            (
                Method::DELETE,
                "G/pools/free/shared/login?sign_out=true",
                None,
            ),
            (Method::DELETE, "G/pools/free/shared/login", None),
        ] {
            let status = change(&served, &served.operator, method, path, body).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{path}");
        }
        let login = login_state(&served, &account).expect("the login was not forgotten");
        assert!(login.requests.is_empty(), "nothing was queued");
    }

    #[tokio::test]
    async fn the_board_needs_pool_read_on_the_operation() {
        let served = served_with(ROLES).await;
        assert_eq!(
            board(&served, &served.viewer, "L").await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            board(&served, &served.operator, "L").await.status(),
            StatusCode::FORBIDDEN,
            "the operator has no pool.read in L"
        );
        let anonymous = reqwest::Client::new()
            .get(format!("{}/v1/user/operations/G/pools", served.base))
            .send()
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    }
}
