//! The models each account and route may use, as the Stations last reported them.
//!
//! Each Station looks through its handlers at its pool accounts and at the routes no pool serves
//! (`[accounts] models_every`) and reports what it found at check-in. This reads those reports
//! for a person who may read the roster: it is what a route's form can offer, and it makes no
//! route. A handler that cannot list says so (`known` is `false`, with why); nothing is guessed.
//! No credential is in a report.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::Serialize;

use crate::{ApiError, Gateway, bearer, resolve_user, resource_action_allowed};

/// What one Station found of one account or route.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StationModels {
    /// The Station that looked.
    pub station: String,
    /// What it found: `source`, `pool` and `account` or `agent` and `route`, `handler`, `listing`
    /// (`known`, `unknown_because`, `models`) and `at`.
    #[serde(flatten)]
    pub found: serde_json::Value,
}

/// `GET /v1/user/models`: what every Station last reported, by Station, newest look first.
/// Needs `roster.read`.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
) -> Result<Json<Vec<StationModels>>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    if !resource_action_allowed(
        &gateway,
        person,
        crate::roster::ROSTER_RESOURCE.to_owned(),
        "roster.read",
    )
    .await?
    {
        return Err(ApiError::forbidden());
    }
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let reports = tokio::task::spawn_blocking(move || {
        let stations = registry
            .stations(Duration::MAX)
            .map_err(|_| ApiError::internal())?;
        let mut reports = Vec::new();
        for station in stations {
            let Some(found) = station
                .info()
                .get("models")
                .and_then(serde_json::Value::as_array)
            else {
                continue;
            };
            for found in found {
                reports.push(StationModels {
                    station: station.station().to_owned(),
                    found: found.clone(),
                });
            }
        }
        reports.sort_by(|left, right| {
            let at = |report: &StationModels| report.found["at"].as_f64().unwrap_or_default();
            at(right)
                .total_cmp(&at(left))
                .then_with(|| left.station.cmp(&right.station))
        });
        Ok::<_, ApiError>(reports)
    })
    .await
    .map_err(|_| ApiError::internal())??;
    Ok(Json(reports))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ai6::directorates::registry::backend::*;
    use axum::http::StatusCode;

    use crate::controls::tests::served_with;

    const READERS: &str = r#"
        [role.roster-reader]
        actions = ["roster.read"]

        [role.agent-reader]
        actions = ["agent.read"]

        [[rule]]
        subject = "person:operator-007"
        scope = "config:roster"
        role = "roster-reader"

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "agent-reader"
    "#;

    async fn models(base: &str, token: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(format!("{base}/v1/user/models"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn what_the_stations_found_reaches_who_may_read_the_roster_and_nobody_else() {
        let served = served_with(READERS).await;
        let found = |source: &str, at: f64, known: bool| {
            serde_json::json!({
                "source": source, "pool": "team", "account": "alpha", "handler": "claude", "at": at,
                "listing": {
                    "known": known,
                    "unknown_because": (!known).then_some("this handler has no way to list models"),
                    "models": if known { serde_json::json!([{"id": "opus", "name": "Opus"}]) } else { serde_json::json!([]) },
                }
            })
        };
        served
            .registry
            .check_in(
                "station-a",
                BTreeMap::from([(
                    "models".to_owned(),
                    serde_json::json!([
                        found("team/alpha", 10.0, true),
                        found("team/beta", 20.0, false)
                    ]),
                )]),
            )
            .unwrap();
        served
            .registry
            .check_in(
                "station-b",
                BTreeMap::from([("roles".to_owned(), serde_json::json!(["execution"]))]),
            )
            .unwrap();

        assert_eq!(
            models(&served.base, &served.viewer).await.status(),
            StatusCode::FORBIDDEN
        );
        let all: serde_json::Value = models(&served.base, &served.operator)
            .await
            .json()
            .await
            .unwrap();
        let all = all.as_array().unwrap();
        assert_eq!(all.len(), 2, "a Station that reports none adds none");
        assert_eq!(all[0]["source"], "team/beta", "newest look first");
        assert_eq!(all[0]["station"], "station-a");
        assert_eq!(all[0]["listing"]["known"], false);
        assert_eq!(
            all[0]["listing"]["unknown_because"],
            "this handler has no way to list models"
        );
        assert_eq!(all[1]["listing"]["models"][0]["id"], "opus");
    }
}
