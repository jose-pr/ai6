//! The account pool board: the pool accounts an Operation may draw from, who uses them, how
//! much provider quota they have left and until when they are limited.
//!
//! It is read from the accounts Stations record in the Registry. It never carries a
//! credential, the secret holding one, or the directory of a login.

use std::collections::BTreeMap;

use ai6::AccountState;
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::Serialize;

use crate::{ApiError, Gateway, authorize_operation_action, bearer, resolve_user, unix_timestamp};

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
    /// The Sessions using it now.
    pub active: Vec<BoardClaim>,
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
    let profile = state.profile.clone()?;
    if !profile.operations.iter().any(|id| id == operation) {
        return None;
    }
    Some(BoardAccount {
        tokens_today: state.tokens_on_day(now as i64),
        active: state
            .live_claims(now)
            .map(|claim| BoardClaim {
                station_id: claim.station.clone(),
                mission_id: claim.mission.clone(),
                since: claim.since,
            })
            .collect(),
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
    "#;

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
            .as_secs_f64();
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
                    &AccountProfile::of(
                        &pool,
                        name,
                        operations.into_iter().map(Into::into).collect(),
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let alpha = AccountRef::new("team", "alpha");
        served
            .registry
            .claim_account(
                &AccountClaimRequest::new(&pool, "worker", "mission-g", Duration::from_secs(60))
                    .only("alpha"),
                now,
            )
            .unwrap()
            .unwrap();
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
        assert_eq!(alpha["active"][0]["mission_id"], "mission-g");
        assert_eq!(alpha["active"][0]["station_id"], "worker");
        assert_eq!(alpha["windows"][0]["used_permille"], 640);
        assert_eq!(alpha["limited_until"], now + 600.0);
        assert_eq!(alpha["tokens_today"], 1_234);
        assert_eq!(
            alpha
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            [
                "account",
                "active",
                "handler",
                "kind",
                "labels",
                "last_used",
                "limited_until",
                "max_concurrent",
                "pool",
                "priority",
                "tokens_today",
                "windows",
            ],
            "no field carries a secret name or a login directory"
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
            board(&served, &served.operator, "G").await.status(),
            StatusCode::FORBIDDEN,
            "the operator has no pool.read here"
        );
        let anonymous = reqwest::Client::new()
            .get(format!("{}/v1/user/operations/G/pools", served.base))
            .send()
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    }
}
