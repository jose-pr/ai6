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
}

/// One Session using an account.
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
    /// The most Sessions that may use it at once.
    pub max_concurrent: Option<usize>,
    /// Labels configured for it.
    pub labels: BTreeMap<String, String>,
    /// This Operation's Sessions using it now.
    pub active: Vec<BoardClaim>,
    /// How many Sessions of other Operations use it now: who they are is theirs to see.
    pub other_sessions: usize,
    /// The quota windows last reported.
    pub windows: Vec<BoardWindow>,
    /// Until when its provider refuses work, as Unix seconds, while it does.
    pub limited_until: Option<f64>,
    /// When a Session last took it, as Unix seconds.
    pub last_used: Option<f64>,
    /// Tokens its Sessions used today (UTC).
    pub tokens_today: u64,
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
    Some(BoardAccount {
        tokens_today: state.tokens_on_day(now as i64),
        active: own
            .into_iter()
            .map(|claim| BoardClaim {
                station_id: claim.station.clone(),
                mission_id: claim.mission.clone(),
                since: claim.since,
            })
            .collect(),
        other_sessions: others.len(),
        windows: state
            .windows
            .iter()
            .map(|window| BoardWindow {
                name: window.name.clone(),
                used_permille: window.used_permille,
                resets_at: window.resets_at,
                status: window.status.clone(),
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
        actor: person.id,
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

    /// Declares `team/alpha` (two Sessions at once) for G and `team/beta` for L only.
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
            alpha["other_sessions"], 1,
            "another Operation's Session is only counted"
        );
        assert_eq!(alpha["windows"][0]["used_permille"], 640);
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
                "max_concurrent",
                "other_sessions",
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
