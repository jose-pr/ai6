//! Operator controls: typed Mission and Session actions, queued for the Station that holds
//! the Mission and reported back when it has acted.
//!
//! The gateway decides who may ask and which Station is asked; the Station decides whether
//! the Mission can take the control, because only it holds the Mission's book and lease.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use ai6::directorates::registry::backend::*;
use ai6::{AuditRecord, StationSettings};
use ai6_diptel::{
    ControlAction, ControlId, MissionControl, MissionId, OperationId, SessionId, control_frames,
};
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, Gateway, GatewayError, authorize_operation_action, bearer, resolve_user,
    unix_timestamp,
};

/// Controls retained for status reads and idempotent replays; the oldest finished go first.
pub(crate) const MAX_CONTROLS: usize = 1_024;
/// Controls waiting for one Station before new ones are refused.
const MAX_PENDING_PER_STATION: usize = 64;
/// A control no Station took, or took and never reported, within this is marked expired.
pub(crate) const CONTROL_TTL: Duration = Duration::from_secs(120);
const DEFAULT_POLL_WAIT: Duration = Duration::from_millis(1_000);
const MAX_POLL_WAIT: Duration = Duration::from_secs(25);
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;
const MAX_REASON_CHARS: usize = 512;
/// The Mission states a Station may report after a control.
const MISSION_STATES: [&str; 10] = [
    "queued",
    "working",
    "blocked",
    "stalled",
    "parked",
    "quarantined",
    "done",
    "failed",
    "blown",
    "abandoned",
];

/// One operator control and what became of it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ControlStatus {
    /// Identity of this control, also the key of its status route.
    pub control_id: String,
    /// The Operation the requester was authorized for.
    pub operation_id: String,
    /// The Mission the control acts on.
    pub mission_id: String,
    /// The provider Session expected to be the Mission's current one, when known.
    pub session_id: Option<String>,
    /// `cancel`, `retry`, `resume` or `interrupt`.
    pub action: String,
    /// The Agent a retry is given to, when one was named.
    pub agent: Option<String>,
    /// The Station asked to act.
    pub station_id: String,
    /// `queued`, `delivered` (the Station took it), `applied`, `rejected` or `expired`.
    pub status: String,
    /// The Mission's state the Station reported afterwards.
    pub state: Option<String>,
    /// Why the Station rejected the control, in AI6's own words.
    pub reason: Option<String>,
    /// The person who asked.
    pub requested_by: String,
    /// When the control was asked for, as Unix seconds.
    pub requested_at: f64,
    /// When the control was applied, rejected or expired, as Unix seconds.
    pub finished_at: Option<f64>,
}

impl ControlStatus {
    fn is_open(&self) -> bool {
        matches!(self.status.as_str(), "queued" | "delivered")
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct IdempotencyKey {
    actor: String,
    operation: String,
    mission: String,
    key: String,
}

/// What two requests with the same idempotency key must agree on.
#[derive(Clone, PartialEq, Eq)]
struct Asked {
    action: ControlAction,
    agent: Option<String>,
    message: Option<String>,
    session: Option<String>,
}

struct Entry {
    status: ControlStatus,
    control: MissionControl,
    asked: Asked,
    key: IdempotencyKey,
    created: Instant,
}

/// The gateway's bounded memory of controls: one queue per Station, plus every control's status.
#[derive(Default)]
pub(crate) struct ControlBook {
    entries: HashMap<String, Entry>,
    order: VecDeque<String>,
    pending: HashMap<String, VecDeque<String>>,
    keys: HashMap<IdempotencyKey, String>,
}

impl ControlBook {
    /// Marks controls no Station finished in time as expired.
    fn expire(&mut self, now: Instant, wall: f64) {
        for entry in self.entries.values_mut() {
            if entry.status.is_open() && now.duration_since(entry.created) > CONTROL_TTL {
                entry.status.status = "expired".to_owned();
                entry.status.finished_at = Some(wall);
            }
        }
        for queue in self.pending.values_mut() {
            queue.retain(|id| {
                self.entries
                    .get(id)
                    .is_some_and(|entry| entry.status.status == "queued")
            });
        }
    }

    fn insert(&mut self, entry: Entry) -> Result<ControlStatus, GatewayError> {
        let station = entry.status.station_id.clone();
        if self
            .pending
            .get(&station)
            .is_some_and(|queue| queue.len() >= MAX_PENDING_PER_STATION)
        {
            return Err(GatewayError::Backpressure);
        }
        while self.entries.len() >= MAX_CONTROLS {
            let finished = self.order.iter().position(|id| {
                self.entries
                    .get(id)
                    .is_none_or(|entry| !entry.status.is_open())
            });
            let Some(index) = finished else {
                return Err(GatewayError::Backpressure);
            };
            if let Some(id) = self.order.remove(index)
                && let Some(old) = self.entries.remove(&id)
            {
                self.keys.remove(&old.key);
            }
        }
        let id = entry.status.control_id.clone();
        let status = entry.status.clone();
        self.keys.insert(entry.key.clone(), id.clone());
        self.order.push_back(id.clone());
        self.pending
            .entry(station)
            .or_default()
            .push_back(id.clone());
        self.entries.insert(id, entry);
        Ok(status)
    }

    /// Hands the oldest queued control for `station` over and marks it delivered.
    fn take(&mut self, station: &str) -> Option<MissionControl> {
        let queue = self.pending.get_mut(station)?;
        while let Some(id) = queue.pop_front() {
            if let Some(entry) = self.entries.get_mut(&id)
                && entry.status.status == "queued"
            {
                entry.status.status = "delivered".to_owned();
                return Some(entry.control.clone());
            }
        }
        None
    }

    fn finish(
        &mut self,
        control_id: &str,
        station: &str,
        report: &ControlReport,
        wall: f64,
    ) -> Result<ControlStatus, GatewayError> {
        let entry = self
            .entries
            .get_mut(control_id)
            .ok_or(GatewayError::NotFound)?;
        if entry.status.station_id != station {
            return Err(GatewayError::Forbidden);
        }
        if !entry.status.is_open() {
            return Err(GatewayError::Conflict);
        }
        entry.status.status = report.outcome.clone();
        entry.status.state = report.state.clone();
        entry.status.reason = report.reason.clone();
        entry.status.finished_at = Some(wall);
        if let Some(queue) = self.pending.get_mut(station) {
            queue.retain(|id| id != control_id);
        }
        Ok(entry.status.clone())
    }
}

impl Gateway {
    fn with_controls<T>(&self, act: impl FnOnce(&mut ControlBook) -> T) -> Result<T, ApiError> {
        let now = Instant::now();
        let wall = unix_timestamp()?;
        let mut book = self.controls.lock().map_err(|_| ApiError::internal())?;
        book.expire(now, wall);
        Ok(act(&mut book))
    }

    /// Waits up to `wait` for a control queued for `station`, and hands it over.
    pub(crate) async fn next_control(
        &self,
        station: &str,
        wait: Duration,
    ) -> Result<Option<MissionControl>, ApiError> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.control_ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(control) = self.with_controls(|book| book.take(station))? {
                return Ok(Some(control));
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(None);
            }
        }
    }
}

/// The body of `POST /v1/user/operations/{operation_id}/missions/{mission_id}/actions`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlRequest {
    action: String,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    idempotency_key: String,
}

/// Where a control goes: the Station holding the Mission, and its current provider Session.
struct Target {
    station: String,
    session: Option<String>,
}

/// Where a control for a Mission goes.
enum Routing {
    To(Target),
    /// Nothing in the Registry or on the board knows the Mission.
    Unknown,
    /// The Mission is known, but no Station that could act on it is checking in.
    NoStation,
}

/// The Station that should act on a Mission: the one holding its latest Session record, else
/// the one holding its brief's lease, else the Headquarters Station. A Station that has gone
/// dark is passed over. The Mission is known only when something names `operation` as its
/// Operation (a Session record, its lease, its queued brief, or the board: `on_board`) and
/// nothing names another; otherwise it is unknown, so nothing about a Mission of another
/// Operation is routed to, counted or told.
fn control_target(
    registry: &dyn RegistryBackend,
    operation: &str,
    mission_id: &str,
    on_board: bool,
) -> std::io::Result<Routing> {
    let live = registry
        .stations(registry.limits().stations.gone_dark_after.0)?
        .into_iter()
        .map(|record| record.station().to_owned())
        .collect::<Vec<_>>();
    let records: Vec<_> = registry
        .sessions()?
        .into_iter()
        .filter(|record| record.mission_id == mission_id)
        .collect();
    let leased = registry.leases()?.into_iter().find(|lease| {
        lease
            .meta()
            .get("mission_id")
            .and_then(serde_json::Value::as_str)
            == Some(mission_id)
    });
    let leased_for = leased.as_ref().and_then(|lease| {
        lease
            .meta()
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    });
    // A queued brief's Mission ID is its key until a Station places it.
    let briefed_for: Vec<Option<String>> = registry
        .list_new()?
        .into_iter()
        .filter(|brief| brief.key() == mission_id)
        .map(|brief| brief.operation)
        .collect();
    let named: Vec<&str> = records
        .iter()
        .filter_map(|record| record.operation.as_deref())
        .chain(leased_for.as_deref())
        .chain(briefed_for.iter().flatten().map(String::as_str))
        .collect();
    if !(on_board || named.contains(&operation)) || named.iter().any(|named| *named != operation) {
        return Ok(Routing::Unknown);
    }
    let latest = records.into_iter().max_by(|left, right| {
        left.attempt
            .cmp(&right.attempt)
            .then(left.lease_expires.total_cmp(&right.lease_expires))
    });
    let session = latest
        .as_ref()
        .and_then(|record| record.provider_session_id.clone());
    if let Some(record) = &latest
        && live.contains(&record.station)
    {
        return Ok(Routing::To(Target {
            station: record.station.clone(),
            session,
        }));
    }
    if let Some(lease) = &leased
        && live.iter().any(|station| station == lease.station())
    {
        return Ok(Routing::To(Target {
            station: lease.station().to_owned(),
            session,
        }));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64());
    Ok(registry
        .headquarters()?
        .filter(|lease| lease.expires() > now && live.iter().any(|s| s == lease.station()))
        .map_or(Routing::NoStation, |lease| {
            Routing::To(Target {
                station: lease.station().to_owned(),
                session,
            })
        }))
}

fn audit_action(action: ControlAction) -> &'static str {
    match action {
        ControlAction::Cancel => "mission.cancel",
        ControlAction::Retry => "mission.retry",
        ControlAction::Resume => "mission.resume",
        ControlAction::Interrupt => "session.interrupt",
        _ => "mission.control",
    }
}

fn valid_key(key: &str) -> bool {
    !key.trim().is_empty()
        && key.len() <= MAX_IDEMPOTENCY_KEY_BYTES
        && !key.contains(char::is_control)
}

pub(crate) async fn request_control(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id)): Path<(String, String)>,
    Json(request): Json<ControlRequest>,
) -> Result<(StatusCode, Json<ControlStatus>), ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    let mission = MissionId::new(mission_id).map_err(|_| ApiError::bad_request())?;
    let action = ControlAction::parse(&request.action).ok_or_else(ApiError::bad_request)?;
    if !valid_key(&request.idempotency_key)
        || (request.agent.is_some() && action != ControlAction::Retry)
        || (request.message.is_some() && action != ControlAction::Resume)
    {
        return Err(ApiError::bad_request());
    }
    let expected_session = request
        .session_id
        .clone()
        .map(SessionId::new)
        .transpose()
        .map_err(|_| ApiError::bad_request())?;
    let asked = Asked {
        action,
        agent: request.agent.clone(),
        message: request.message.clone(),
        session: request.session_id.clone(),
    };
    authorize_operation_action(&gateway, person.clone(), &operation, "mission.control").await?;
    let key = IdempotencyKey {
        actor: person.id.clone(),
        operation: operation.to_string(),
        mission: mission.to_string(),
        key: request.idempotency_key.clone(),
    };
    let replay = gateway.with_controls(|book| {
        book.keys
            .get(&key)
            .and_then(|id| book.entries.get(id))
            .map(|entry| (entry.asked == asked).then(|| entry.status.clone()))
    })?;
    match replay {
        Some(Some(status)) => return Ok((StatusCode::OK, Json(status))),
        Some(None) => return Err(ApiError::from(GatewayError::Conflict)),
        None => {}
    }
    let on_board = {
        let inner = gateway.inner.lock().await;
        let elsewhere = inner
            .sessions
            .values()
            .any(|session| session.mission_id == mission && session.operation_id != operation);
        if elsewhere {
            return Err(ApiError::not_found());
        }
        inner
            .sessions
            .values()
            .any(|session| session.mission_id == mission)
            || inner
                .missions
                .contains_key(&(operation.clone(), mission.clone()))
    };
    let registry = gateway.registry.clone().ok_or_else(ApiError::unavailable)?;
    let lookup = registry.clone();
    let (wanted_operation, wanted) = (operation.to_string(), mission.to_string());
    let routing = tokio::task::spawn_blocking(move || {
        control_target(lookup.as_ref(), &wanted_operation, &wanted, on_board)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    // Refused before anything is audited or queued.
    let target = match routing {
        Routing::To(target) => target,
        Routing::Unknown => return Err(ApiError::not_found()),
        Routing::NoStation => return Err(no_station()),
    };
    let session = match (expected_session, &target.session) {
        (Some(expected), Some(current)) if expected.as_str() != current => {
            return Err(ApiError::from(GatewayError::Conflict));
        }
        (Some(expected), _) => Some(expected),
        (None, Some(current)) => SessionId::new(current.clone()).ok(),
        (None, None) => None,
    };
    let control_id =
        ControlId::new(Uuid::new_v4().to_string()).map_err(|_| ApiError::internal())?;
    let mut control = MissionControl::new(
        control_id.clone(),
        operation.clone(),
        mission.clone(),
        action,
    );
    if let Some(session) = &session {
        control = control.with_session(session.clone());
    }
    if let Some(agent) = &request.agent {
        control = control
            .with_agent(agent.clone())
            .map_err(|_| ApiError::bad_request())?;
    }
    if let Some(message) = &request.message {
        control = control
            .with_message(message.clone())
            .map_err(|_| ApiError::bad_request())?;
    }
    let requested_at = unix_timestamp()?;
    let audit = AuditRecord {
        id: Uuid::new_v4().to_string(),
        actor: person.id.clone(),
        action: audit_action(action).to_owned(),
        operation: operation.to_string(),
        resource: None,
        target: mission.to_string(),
        at: requested_at,
    };
    tokio::task::spawn_blocking(move || registry.record_audit(&audit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    let status = ControlStatus {
        control_id: control_id.to_string(),
        operation_id: operation.to_string(),
        mission_id: mission.to_string(),
        session_id: session.map(|session| session.to_string()),
        action: action.as_str().to_owned(),
        agent: request.agent,
        station_id: target.station,
        status: "queued".to_owned(),
        state: None,
        reason: None,
        requested_by: person.id,
        requested_at,
        finished_at: None,
    };
    let entry = Entry {
        status,
        control,
        asked,
        key,
        created: Instant::now(),
    };
    let status = gateway
        .with_controls(|book| book.insert(entry))?
        .map_err(ApiError::from)?;
    gateway.control_ready.notify_waiters();
    Ok((StatusCode::ACCEPTED, Json(status)))
}

pub(crate) async fn control_status(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path((operation_id, mission_id, control_id)): Path<(String, String, String)>,
) -> Result<Json<ControlStatus>, ApiError> {
    let person = resolve_user(&gateway, bearer(&headers)?).await?;
    let operation = OperationId::new(operation_id).map_err(|_| ApiError::bad_request())?;
    authorize_operation_action(&gateway, person, &operation, "mission.read").await?;
    gateway
        .with_controls(|book| {
            book.entries
                .get(&control_id)
                .map(|entry| entry.status.clone())
        })?
        .filter(|status| {
            status.operation_id == operation.as_str() && status.mission_id == mission_id
        })
        .map(Json)
        .ok_or_else(ApiError::not_found)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlPoll {
    station: String,
    wait_ms: Option<u64>,
}

pub(crate) async fn next_station_control(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Query(poll): Query<ControlPoll>,
) -> Result<Response, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    StationSettings::try_new(poll.station.clone(), 1).map_err(|_| ApiError::bad_request())?;
    let wait = poll
        .wait_ms
        .map_or(DEFAULT_POLL_WAIT, Duration::from_millis)
        .min(MAX_POLL_WAIT);
    let Some(control) = gateway.next_control(&poll.station, wait).await? else {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .map_err(|_| ApiError::internal());
    };
    let (_, frame) = control_frames(&control).map_err(|_| ApiError::internal())?;
    let mut response = Response::new(Body::from(frame));
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static("application/vnd.ai6.guatiao"),
    );
    Ok(response)
}

/// The body a Station reports a control's outcome with.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlReport {
    station: String,
    outcome: String,
    state: Option<String>,
    reason: Option<String>,
}

pub(crate) async fn report_station_control(
    State(gateway): State<Gateway>,
    headers: HeaderMap,
    Path(control_id): Path<String>,
    Json(mut report): Json<ControlReport>,
) -> Result<StatusCode, ApiError> {
    gateway.authenticate_station(bearer(&headers)?)?;
    StationSettings::try_new(report.station.clone(), 1).map_err(|_| ApiError::bad_request())?;
    if !matches!(report.outcome.as_str(), "applied" | "rejected")
        || report
            .state
            .as_deref()
            .is_some_and(|state| !MISSION_STATES.contains(&state))
    {
        return Err(ApiError::bad_request());
    }
    report.reason = report.reason.map(|reason| {
        reason
            .chars()
            .filter(|character| !character.is_control())
            .take(MAX_REASON_CHARS)
            .collect()
    });
    let wall = unix_timestamp()?;
    let station = report.station.clone();
    let finished = gateway
        .with_controls(|book| book.finish(&control_id, &station, &report, wall))?
        .map_err(ApiError::from)?;
    crate::timeline::record_control(&gateway, &finished).await;
    Ok(StatusCode::NO_CONTENT)
}

fn no_station() -> ApiError {
    ApiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        message: "no Station can take the control now",
        retry_after_seconds: Some(15),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use ai6::FileRegistry;
    use ai6::guatiao::library::ProviderError;
    use ai6::{
        Attempt, Authenticator, Directory, Lookup, NewSession, Person, Verdict,
        role_authorizer_from_toml,
    };
    use ai6_diptel::{FrameReceiver, IncomingFrame};

    use super::*;
    use crate::{AccessProviders, GatewayConfig, GatewayLimits, issue_access_token_record};

    pub(crate) const STATION_TOKEN: &str = "m-token-that-is-long-enough-for-testing";

    struct People;

    impl Authenticator for People {
        fn authenticate(&self, _attempt: Attempt) -> Result<Verdict, ProviderError> {
            Ok(Verdict::Invalid)
        }
    }

    impl Directory for People {
        fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
            Ok(["operator-007", "viewer-007"]
                .contains(&lookup.value.as_str())
                .then(|| Person {
                    id: lookup.value.clone(),
                    name: lookup.value,
                    ..Person::default()
                }))
        }
    }

    /// A served gateway over a Registry where Station `worker` holds `mission-g`'s Session
    /// `session-g` in Operation G and `hq` is the Headquarters Station.
    pub(crate) struct Served {
        pub(crate) base: String,
        pub(crate) registry: FileRegistry,
        pub(crate) operator: String,
        pub(crate) viewer: String,
        server: tokio::task::JoinHandle<()>,
        pub(crate) directory: tempfile::TempDir,
    }

    impl Drop for Served {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    /// Operator 007 may read, control and audit Operation G, and audit Station `worker`;
    /// viewer 007 may read and audit G.
    pub(crate) const ROLES: &str = r#"
        [role.viewer]
        actions = ["mission.read", "audit.read"]

        [role.operator]
        actions = ["mission.read", "mission.control", "audit.read"]

        [[rule]]
        subject = "person:viewer-007"
        scope = "operation:G"
        role = "viewer"

        [[rule]]
        subject = "person:operator-007"
        scope = "operation:G"
        role = "operator"

        [role.station-auditor]
        actions = ["audit.read"]

        [[rule]]
        subject = "person:operator-007"
        scope = "station:worker"
        role = "station-auditor"
    "#;

    pub(crate) async fn served() -> Served {
        served_with(ROLES).await
    }

    /// As [`served`], with another role policy for `operator-007` and `viewer-007`.
    pub(crate) async fn served_with(roles: &str) -> Served {
        served_with_roster(roles, None).await
    }

    /// As [`served_with`], serving roster administration from `roster`.
    pub(crate) async fn served_with_roster(
        roles: &str,
        roster: Option<Arc<dyn ai6::ConfigProvider>>,
    ) -> Served {
        let directory = tempfile::tempdir().unwrap();
        let registry = FileRegistry::open(directory.path());
        for (station, operations) in [("hq", vec![]), ("worker", vec!["G", "L"])] {
            registry
                .check_in(
                    station,
                    BTreeMap::from([("operations".to_owned(), serde_json::json!(operations))]),
                )
                .unwrap();
        }
        registry.elect("hq", Duration::from_secs(120)).unwrap();
        registry
            .open_session(
                NewSession::new("record-g", "mission-g", "worker")
                    .with_operation("G")
                    .with_provider_session("session-g"),
                Duration::from_secs(60),
            )
            .unwrap();
        let authorizer = role_authorizer_from_toml(roles, ai6::Limit::UNBOUNDED).unwrap();
        let gateway = Gateway::with_access(
            GatewayConfig::new(
                "https://localhost",
                STATION_TOKEN,
                GatewayLimits::default(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
            Arc::new(registry.clone()),
            AccessProviders::new(Arc::new(People), Arc::new(People), Arc::new(authorizer)),
        );
        let gateway = match roster {
            Some(roster) => gateway.with_roster(roster),
            None => gateway,
        };
        for (operation, mission, session) in [
            ("G", "mission-g", "session-g"),
            ("L", "mission-l", "session-l"),
        ] {
            gateway
                .register_session(
                    STATION_TOKEN,
                    OperationId::new(operation).unwrap(),
                    MissionId::new(mission).unwrap(),
                    SessionId::new(session).unwrap(),
                )
                .await
                .unwrap();
        }
        let operator =
            issue_access_token_record(Arc::new(registry.clone()), "operator-007".to_owned(), 600)
                .await
                .ok()
                .unwrap()
                .token;
        let viewer =
            issue_access_token_record(Arc::new(registry.clone()), "viewer-007".to_owned(), 600)
                .await
                .ok()
                .unwrap()
                .token;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = gateway.router();
        let server = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        Served {
            base,
            registry,
            operator,
            viewer,
            server,
            directory,
        }
    }

    pub(crate) async fn ask(
        served: &Served,
        token: &str,
        operation: &str,
        mission: &str,
        body: serde_json::Value,
    ) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!(
                "{}/v1/user/operations/{operation}/missions/{mission}/actions",
                served.base
            ))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    pub(crate) async fn take(served: &Served, station: &str) -> Option<MissionControl> {
        let response = reqwest::Client::new()
            .get(format!(
                "{}/v1/station/controls/next?station={station}&wait_ms=50",
                served.base
            ))
            .bearer_auth(STATION_TOKEN)
            .send()
            .await
            .unwrap();
        if response.status() == StatusCode::NO_CONTENT {
            return None;
        }
        assert_eq!(response.status(), StatusCode::OK);
        let frame = response.bytes().await.unwrap();
        let template = MissionControl::new(
            ControlId::new("schema").unwrap(),
            OperationId::new("schema").unwrap(),
            MissionId::new("schema").unwrap(),
            ControlAction::Cancel,
        );
        let mut receiver = FrameReceiver::new();
        receiver
            .accept(&control_frames(&template).unwrap().0)
            .unwrap();
        match receiver.accept(&frame).unwrap() {
            Some(IncomingFrame::Control(control)) => Some(control),
            other => panic!("expected a control, got {other:?}"),
        }
    }

    pub(crate) async fn report(
        served: &Served,
        control_id: &str,
        body: serde_json::Value,
    ) -> StatusCode {
        reqwest::Client::new()
            .post(format!(
                "{}/v1/station/controls/{control_id}/result",
                served.base
            ))
            .bearer_auth(STATION_TOKEN)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status()
    }

    async fn status(served: &Served, control_id: &str) -> serde_json::Value {
        reqwest::Client::new()
            .get(format!(
                "{}/v1/user/operations/G/missions/mission-g/actions/{control_id}",
                served.base
            ))
            .bearer_auth(&served.viewer)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_control_reaches_the_station_holding_the_session_and_reports_its_outcome() {
        let served = served().await;
        let asked = ask(
            &served,
            &served.operator,
            "G",
            "mission-g",
            serde_json::json!({"action": "cancel", "idempotency_key": "cancel-1"}),
        )
        .await;
        assert_eq!(asked.status(), StatusCode::ACCEPTED);
        let queued: serde_json::Value = asked.json().await.unwrap();
        assert_eq!(queued["status"], "queued");
        assert_eq!(queued["station_id"], "worker");
        assert_eq!(queued["session_id"], "session-g");
        assert_eq!(queued["requested_by"], "operator-007");
        let control_id = queued["control_id"].as_str().unwrap().to_owned();

        // A retried request returns the same control; a different one under the key is refused.
        let replay = ask(
            &served,
            &served.operator,
            "G",
            "mission-g",
            serde_json::json!({"action": "cancel", "idempotency_key": "cancel-1"}),
        )
        .await;
        assert_eq!(replay.status(), StatusCode::OK);
        let replayed: serde_json::Value = replay.json().await.unwrap();
        assert_eq!(replayed["control_id"], control_id.as_str());
        let reused = ask(
            &served,
            &served.operator,
            "G",
            "mission-g",
            serde_json::json!({"action": "retry", "idempotency_key": "cancel-1"}),
        )
        .await;
        assert_eq!(reused.status(), StatusCode::CONFLICT);

        // Only the Station holding the Session is handed the control, once.
        assert!(take(&served, "hq").await.is_none());
        let control = take(&served, "worker").await.unwrap();
        assert!(take(&served, "worker").await.is_none());
        assert_eq!(control.control_id().as_str(), control_id);
        assert_eq!(control.action(), ControlAction::Cancel);
        assert_eq!(control.operation_id().as_str(), "G");
        assert_eq!(control.mission_id().as_str(), "mission-g");
        assert_eq!(control.session_id().unwrap().as_str(), "session-g");
        assert_eq!(status(&served, &control_id).await["status"], "delivered");

        let wrong_station = report(
            &served,
            &control_id,
            serde_json::json!({"station": "hq", "outcome": "applied", "state": "abandoned", "reason": null}),
        )
        .await;
        assert_eq!(wrong_station, StatusCode::FORBIDDEN);
        let reported = report(
            &served,
            &control_id,
            serde_json::json!({"station": "worker", "outcome": "applied", "state": "abandoned", "reason": null}),
        )
        .await;
        assert_eq!(reported, StatusCode::NO_CONTENT);
        let finished = status(&served, &control_id).await;
        assert_eq!(finished["status"], "applied");
        assert_eq!(finished["state"], "abandoned");
        assert!(finished["finished_at"].as_f64().is_some());
        let again = report(
            &served,
            &control_id,
            serde_json::json!({"station": "worker", "outcome": "rejected", "state": null, "reason": "late"}),
        )
        .await;
        assert_eq!(again, StatusCode::CONFLICT);

        let audit = served.registry.list_audit(10).unwrap();
        assert!(audit.iter().any(|record| record.action == "mission.cancel"
            && record.actor == "operator-007"
            && record.target == "mission-g"
            && record.operation == "G"));
    }

    #[tokio::test]
    async fn controls_need_mission_control_and_stay_in_their_operation() {
        let served = served().await;
        let cancel = serde_json::json!({"action": "cancel", "idempotency_key": "k"});
        let viewer = ask(&served, &served.viewer, "G", "mission-g", cancel.clone()).await;
        assert_eq!(viewer.status(), StatusCode::FORBIDDEN);
        let elsewhere = ask(&served, &served.operator, "L", "mission-l", cancel.clone()).await;
        assert_eq!(elsewhere.status(), StatusCode::FORBIDDEN);
        // A Mission of another Operation is not reachable through one the caller holds.
        let crossed = ask(&served, &served.operator, "G", "mission-l", cancel.clone()).await;
        assert_eq!(crossed.status(), StatusCode::NOT_FOUND);
        let anonymous = reqwest::Client::new()
            .post(format!(
                "{}/v1/user/operations/G/missions/mission-g/actions",
                served.base
            ))
            .json(&cancel)
            .send()
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        for invalid in [
            serde_json::json!({"action": "pause", "idempotency_key": "k"}),
            serde_json::json!({"action": "cancel", "agent": "008", "idempotency_key": "k"}),
            serde_json::json!({"action": "interrupt", "message": "stop", "idempotency_key": "k"}),
            serde_json::json!({"action": "resume", "message": " ", "idempotency_key": "k"}),
            serde_json::json!({"action": "cancel", "idempotency_key": ""}),
        ] {
            let refused = ask(&served, &served.operator, "G", "mission-g", invalid).await;
            assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        }
        let stale = ask(
            &served,
            &served.operator,
            "G",
            "mission-g",
            serde_json::json!({"action": "interrupt", "session_id": "session-old", "idempotency_key": "k"}),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert!(served.registry.list_audit(10).unwrap().is_empty());

        let unauthenticated_station = reqwest::Client::new()
            .get(format!(
                "{}/v1/station/controls/next?station=worker",
                served.base
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(unauthenticated_station.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_mission_without_a_session_goes_to_the_headquarters_station() {
        let served = served().await;
        let brief = ai6::Brief {
            title: "Queued".to_owned(),
            body: "Not placed yet.".to_owned(),
            id: "mission-new".to_owned(),
            source: "universal-exports".to_owned(),
            operation: Some("G".to_owned()),
            agent: None,
            role: None,
            tags: Vec::new(),
            stations: Vec::new(),
            priority: 0,
            url: None,
            data: BTreeMap::new(),
            created: String::new(),
            version: 1,
            provenance: None,
        };
        served.registry.submit_brief(&brief).unwrap();
        let asked = ask(
            &served,
            &served.operator,
            "G",
            &brief.key(),
            serde_json::json!({"action": "retry", "agent": "008", "idempotency_key": "r"}),
        )
        .await;
        assert_eq!(asked.status(), StatusCode::ACCEPTED);
        let queued: serde_json::Value = asked.json().await.unwrap();
        assert_eq!(queued["station_id"], "hq");
        assert_eq!(queued["agent"], "008");
        let control = take(&served, "hq").await.unwrap();
        assert_eq!(control.agent(), Some("008"));
        assert_eq!(control.session_id(), None);

        let empty = tempfile::tempdir().unwrap();
        let alone = Gateway::with_registry(
            GatewayConfig::new(
                "https://localhost",
                STATION_TOKEN,
                GatewayLimits::default(),
                Duration::from_secs(30),
                Duration::from_secs(30),
            )
            .unwrap(),
            Arc::new(FileRegistry::open(empty.path())),
        );
        let registry = alone.registry.as_deref().unwrap();
        assert!(matches!(
            control_target(registry, "G", "mission-g", true).unwrap(),
            Routing::NoStation
        ));
        assert!(matches!(
            control_target(registry, "G", "mission-g", false).unwrap(),
            Routing::Unknown
        ));
    }

    #[tokio::test]
    async fn an_unknown_mission_is_refused_before_it_is_audited_or_queued() {
        let served = served().await;
        for mission in ["mission-nobody-knows", "mission-l"] {
            let asked = ask(
                &served,
                &served.operator,
                "G",
                mission,
                serde_json::json!({"action": "cancel", "idempotency_key": "c"}),
            )
            .await;
            assert_eq!(asked.status(), StatusCode::NOT_FOUND, "{mission}");
        }
        assert!(served.registry.list_audit(10).unwrap().is_empty());
        assert!(take(&served, "hq").await.is_none());
        assert!(take(&served, "worker").await.is_none());
    }

    #[tokio::test]
    async fn a_mission_the_registry_holds_for_another_operation_is_not_found() {
        let served = served().await;
        // Off the board, so only the Registry knows them: one of Operation L, one whose
        // Operation nothing names, and one claimed by G and by L.
        for (record, mission, operation) in [
            ("record-x", "mission-x", Some("L")),
            ("record-y", "mission-y", None),
            ("record-z1", "mission-z", Some("G")),
            ("record-z2", "mission-z", Some("L")),
        ] {
            let mut new = NewSession::new(record, mission, "worker").with_provider_session("p-1");
            if let Some(operation) = operation {
                new = new.with_operation(operation);
            }
            served
                .registry
                .open_session(new, Duration::from_secs(60))
                .unwrap();
        }
        for mission in ["mission-x", "mission-y", "mission-z"] {
            let asked = ask(
                &served,
                &served.operator,
                "G",
                mission,
                serde_json::json!({"action": "cancel", "idempotency_key": "c"}),
            )
            .await;
            assert_eq!(asked.status(), StatusCode::NOT_FOUND, "{mission}");
            let body = asked.text().await.unwrap();
            assert!(
                !body.contains("worker") && !body.contains("p-1"),
                "{mission}: {body}"
            );
        }
        assert!(served.registry.list_audit(10).unwrap().is_empty());
        assert!(take(&served, "worker").await.is_none());

        // The Registry naming the Operation asked about is enough.
        served
            .registry
            .open_session(
                NewSession::new("record-w", "mission-w", "worker").with_operation("G"),
                Duration::from_secs(60),
            )
            .unwrap();
        let asked = ask(
            &served,
            &served.operator,
            "G",
            "mission-w",
            serde_json::json!({"action": "cancel", "idempotency_key": "c"}),
        )
        .await;
        assert_eq!(asked.status(), StatusCode::ACCEPTED);
    }

    #[test]
    fn open_controls_expire_and_the_book_stays_bounded() {
        let mut book = ControlBook::default();
        let entry = |id: usize| Entry {
            status: ControlStatus {
                control_id: format!("control-{id}"),
                operation_id: "G".to_owned(),
                mission_id: "mission-g".to_owned(),
                session_id: None,
                action: "cancel".to_owned(),
                agent: None,
                station_id: format!("station-{}", id % 32),
                status: "queued".to_owned(),
                state: None,
                reason: None,
                requested_by: "operator-007".to_owned(),
                requested_at: 1.0,
                finished_at: None,
            },
            control: MissionControl::new(
                ControlId::new(format!("control-{id}")).unwrap(),
                OperationId::new("G").unwrap(),
                MissionId::new("mission-g").unwrap(),
                ControlAction::Cancel,
            ),
            asked: Asked {
                action: ControlAction::Cancel,
                agent: None,
                message: None,
                session: None,
            },
            key: IdempotencyKey {
                actor: "operator-007".to_owned(),
                operation: "G".to_owned(),
                mission: "mission-g".to_owned(),
                key: format!("key-{id}"),
            },
            created: Instant::now(),
        };
        for id in 0..MAX_CONTROLS {
            book.insert(entry(id)).unwrap();
        }
        // Every control is still open, so none can make room.
        assert_eq!(
            book.insert(entry(MAX_CONTROLS)),
            Err(GatewayError::Backpressure)
        );
        book.expire(Instant::now() + CONTROL_TTL + Duration::from_secs(1), 2.0);
        assert!(
            book.take("station-0").is_none(),
            "expired controls are not handed out"
        );
        assert_eq!(book.entries["control-0"].status.status, "expired");
        book.insert(entry(MAX_CONTROLS)).unwrap();
        assert_eq!(book.entries.len(), MAX_CONTROLS);
        assert!(!book.entries.contains_key("control-0"));
        assert!(!book.keys.values().any(|id| id == "control-0"));
    }
}
