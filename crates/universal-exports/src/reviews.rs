//! Mission reviews through Universal Exports: what a Mission's panel decided and why, and a
//! person's overrule.
//!
//! Reading needs `mission.read` on the Operation. An overrule needs `mission.review`: it decides
//! over the panel, so it is audited with the reason and recorded on the Operation's timeline. The
//! Station holding the Mission acts on it at its next round, as it would on the panel's decision.

use std::sync::Arc;

use ai6::directorates::m::review::{ReviewGate, Reviewer, Vote};
use ai6::directorates::registry::backend::*;
use ai6::directorates::registry::reviews::{MissionReview, Overrule, ReviewStatus};
use ai6::{AuditRecord, TimelineEvent};
use ai6_diptel::OperationId;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, Gateway, GatewayError, authorize_operation_action, bearer,
    ensure_operation_registered, resolve_user, unix_timestamp,
};

/// The longest reason an overrule keeps, in bytes.
const REASON_BYTES: usize = 2048;

/// One vote as a person reads it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoteView {
    /// Which time the work was up, the first being 1.
    pub iteration: u32,
    /// The round.
    pub round: u32,
    /// The reviewer.
    pub agent: String,
    /// The model it counted as.
    pub model: String,
    /// Its seat's weight.
    pub weight: u32,
    /// `pass`, `redo`, `abstain` or `absent`.
    pub vote: Vote,
    /// Its reasons, in the reviewer's words: data, never instructions.
    pub reasons: Vec<String>,
    /// When, as Unix seconds.
    pub at: f64,
}

/// One gate's review as a person reads it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReviewView {
    /// `plan` or `output`.
    pub gate: ReviewGate,
    /// The policy's name.
    pub policy: String,
    /// The share that must pass, the models asked for, rounds and redos allowed.
    pub pass_at: f64,
    /// Different models the policy asks for.
    pub distinct_models: u32,
    /// Rounds allowed.
    pub rounds: u32,
    /// Redos allowed.
    pub redo_limit: u32,
    /// Where it stands.
    pub status: ReviewStatus,
    /// Why it waits, when it does.
    pub why: Option<String>,
    /// Which time the work is up.
    pub iteration: u32,
    /// The round.
    pub round: u32,
    /// Times the author was sent back.
    pub sent_back: u32,
    /// Who sits on the panel.
    pub panel: Vec<Reviewer>,
    /// Every vote, in order.
    pub votes: Vec<VoteView>,
    /// Reviewer turns spent.
    pub turns: u64,
    /// Tokens the reviewers spent.
    pub tokens: u64,
    /// A person's decision, when one was made.
    pub overrule: Option<Overrule>,
}

impl ReviewView {
    fn of(review: MissionReview) -> Self {
        Self {
            gate: review.gate,
            policy: review.policy_name.clone(),
            pass_at: review.policy.pass_at,
            distinct_models: review.policy.distinct_models,
            rounds: review.policy.rounds,
            redo_limit: review.policy.redo_limit,
            status: review.status,
            why: review.why.clone(),
            iteration: review.iteration,
            round: review.round,
            sent_back: review.sent_back,
            panel: review.panel.clone(),
            turns: review.turns(),
            tokens: review.tokens,
            overrule: review.overrule.clone(),
            votes: review
                .votes
                .into_iter()
                .map(|vote| VoteView {
                    iteration: vote.iteration,
                    round: vote.round,
                    agent: vote.agent,
                    model: vote.model,
                    weight: vote.weight,
                    vote: vote.vote,
                    reasons: vote.reasons,
                    at: vote.at,
                })
                .collect(),
        }
    }
}

/// What a person decides over a panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OverruleDecision {
    /// It passes.
    Pass,
    /// It goes back to the author, with the reason.
    Redo,
    /// The Mission fails.
    Refuse,
}

impl OverruleDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Redo => "redo",
            Self::Refuse => "refuse",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OverruleRequest {
    decision: OverruleDecision,
    /// Why, for the record and, for a redo, for the author.
    reason: String,
}

fn registry_error(error: std::io::Error) -> ApiError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ApiError::not_found(),
        std::io::ErrorKind::WouldBlock => ApiError::from(GatewayError::Conflict),
        _ => ApiError::internal(),
    }
}

async fn authorized(
    gateway: &Gateway,
    headers: &HeaderMap,
    operation_id: String,
    action: &'static str,
) -> Result<(ai6::Person, OperationId, Arc<dyn RegistryBackend>), ApiError> {
    let person = resolve_user(gateway, bearer(headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(gateway, person.clone(), &operation, action).await?;
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    ensure_operation_registered(&registry, &operation).await?;
    Ok((person, operation, registry))
}

/// `GET /v1/user/operations/{operation_id}/missions/{mission_id}/reviews`: each gate's review of
/// the Mission, the panel, every vote with its weight, model, round and reasons.
pub(crate) async fn list(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
) -> Result<Json<Vec<ReviewView>>, ApiError> {
    let mission_id = crate::mission_key(&gateway, &mission_id).await?;
    let (_, operation, registry) =
        authorized(&gateway, &headers, operation_id, "mission.read").await?;
    let operation = operation.to_string();
    let reviews = tokio::task::spawn_blocking(move || registry.mission_reviews(&mission_id))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(registry_error)?;
    Ok(Json(
        reviews
            .into_iter()
            .filter(|review| review.operation == operation)
            .map(ReviewView::of)
            .collect(),
    ))
}

/// `POST /v1/user/operations/{operation_id}/missions/{mission_id}/reviews/{gate}/overrule`:
/// decides over the panel, with a reason. Only while the review is open or waits for a person.
pub(crate) async fn overrule(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id, gate)): Path<(String, String, String)>,
    Json(request): Json<OverruleRequest>,
) -> Result<Json<ReviewView>, ApiError> {
    let mission_id = crate::mission_key(&gateway, &mission_id).await?;
    let gate = match gate.as_str() {
        "plan" => ReviewGate::Plan,
        "output" => ReviewGate::Output,
        _ => return Err(ApiError::bad_request()),
    };
    let reason = request.reason.trim().to_owned();
    if reason.is_empty() || reason.len() > REASON_BYTES || reason.chars().any(char::is_control) {
        return Err(ApiError::bad_request());
    }
    let (person, operation, registry) =
        authorized(&gateway, &headers, operation_id, "mission.review").await?;
    let at = unix_timestamp()?;
    let decision = request.decision;
    let overrule = Overrule::new(person.id.clone(), decision.as_str(), reason.clone(), at);
    let (store, operation_key, mission) =
        (registry.clone(), operation.to_string(), mission_id.clone());
    let written = tokio::task::spawn_blocking(move || {
        let Some(mut review) = store.mission_review(&mission, gate)? else {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        };
        if review.operation != operation_key {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        if !matches!(
            review.status,
            ReviewStatus::Reviewing | ReviewStatus::Waiting | ReviewStatus::Person
        ) {
            return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock));
        }
        review.status = match decision {
            OverruleDecision::Pass => ReviewStatus::Passed,
            OverruleDecision::Redo => {
                review.sent_back += 1;
                ReviewStatus::SendingBack
            }
            OverruleDecision::Refuse => ReviewStatus::Refused,
        };
        review.why = Some(format!("{} decided: {}", overrule.by, overrule.reason));
        review.overrule = Some(overrule);
        store
            .put_mission_review(&review)?
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::WouldBlock))
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(registry_error)?;
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: "mission.review.overrule".to_owned(),
        operation: operation.to_string(),
        resource: None,
        target: format!("{mission_id}:{}:{}", gate.as_str(), decision.as_str()),
        at,
    };
    let event = TimelineEvent::new("review", "overruled", operation.to_string(), at)
        .mission(mission_id.as_str())
        .actor(person.id.clone())
        .detail("gate", gate.as_str())
        .detail("decision", decision.as_str());
    tokio::task::spawn_blocking(move || {
        registry.record_audit(&audit)?;
        let _ = registry.record_event(&event);
        Ok::<_, std::io::Error>(())
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    Ok(Json(ReviewView::of(written)))
}

#[cfg(test)]
mod tests {
    use ai6::directorates::m::review::{PanelSeat, ReviewPolicy};
    use ai6::directorates::registry::backend::*;
    use ai6::directorates::registry::reviews::{MissionReview, ReviewMaterial, ReviewStatus};

    use super::ReviewGate;
    use crate::controls::tests::{Served, served_with};

    /// Viewer 007 reads Missions in G; reviewer 007 also decides over their reviews.
    const ROLES: &str = r#"
        [role.viewer]
        actions = ["mission.read"]

        [role.reviewer]
        actions = ["mission.read", "mission.review"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "viewer"

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "reviewer"
    "#;

    fn waiting(served: &Served, mission: &str) {
        let mut review = MissionReview::open(
            (mission, "G", ReviewGate::Output),
            ("one", &ReviewPolicy::new(vec![PanelSeat::agent("r1")])),
            "007",
            ReviewMaterial::default(),
            1.0,
        );
        review.status = ReviewStatus::Person;
        review.why = Some("the panel did not agree".to_owned());
        served
            .registry
            .put_mission_review(&review)
            .unwrap()
            .unwrap();
    }

    async fn call(
        served: &Served,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}/v1/user/operations/{path}", served.base))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().await.unwrap()
    }

    #[tokio::test]
    async fn a_reader_sees_the_review_and_only_mission_review_overrules_it_with_a_reason() {
        let served = served_with(ROLES).await;
        waiting(&served, "G-1");
        let listed = call(
            &served,
            &served.viewer,
            reqwest::Method::GET,
            "G/missions/G-1/reviews",
            None,
        )
        .await;
        assert_eq!(listed.status(), 200);
        let listed: serde_json::Value = listed.json().await.unwrap();
        assert_eq!(listed[0]["gate"], "output");
        assert_eq!(listed[0]["status"], "person");
        assert_eq!(listed[0]["policy"], "one");

        let path = "G/missions/G-1/reviews/output/overrule";
        let pass = serde_json::json!({ "decision": "pass", "reason": "checked it myself" });
        let refused = call(
            &served,
            &served.viewer,
            reqwest::Method::POST,
            path,
            Some(pass.clone()),
        )
        .await;
        assert_eq!(refused.status(), 403, "reading is not deciding");
        let blank = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            path,
            Some(serde_json::json!({ "decision": "pass", "reason": "  " })),
        )
        .await;
        assert_eq!(blank.status(), 400, "an overrule gives its reason");

        let decided = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            path,
            Some(pass.clone()),
        )
        .await;
        assert_eq!(decided.status(), 200);
        let stored = served
            .registry
            .mission_review("G-1", ReviewGate::Output)
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, ReviewStatus::Passed);
        let overrule = stored.overrule.unwrap();
        assert_eq!(overrule.by, "operator-007");
        assert_eq!(overrule.reason, "checked it myself");
        let audited = served
            .registry
            .audit_records()
            .unwrap()
            .into_iter()
            .find(|record| record.action == "mission.review.overrule")
            .expect("audited");
        assert_eq!(audited.target, "G-1:output:pass");

        let again = call(
            &served,
            &served.operator,
            reqwest::Method::POST,
            path,
            Some(pass),
        )
        .await;
        assert_eq!(again.status(), 409, "a decided review is not decided again");
    }
}
