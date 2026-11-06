//! The Cable MCP endpoint a runner hands a harness, and the Station's side of it.
//!
//! The runner listens on a loopback port of the Theater with a token of its own, and carries
//! each request to the Station attached to the Session over that Session's ACP connection, as
//! [`REQUEST`] notifications. The Station answers from its own Cable endpoint for the Session,
//! with that endpoint's bearer, in [`ANSWER`] notifications: the head, the body in pieces, the
//! end. The bearer never leaves the Station, and the Theater needs no forward. The harness keeps
//! one URL and token while Stations come and go; with none attached it is told so and may try
//! again.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Request as HttpRequest, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::station::limits::RunnerLimits;

/// The environment variable that carries the Cable MCP endpoint's URL to a harness.
pub(crate) const URL_KEY: &str = "AI6_CABLE_MCP_URL";
/// The environment variable that carries the Cable MCP endpoint's bearer token to a harness.
pub(crate) const TOKEN_KEY: &str = "AI6_CABLE_MCP_TOKEN";

/// Runner to Station: one HTTP request of the harness, a [`Request`].
pub(crate) const REQUEST: &str = "_ai6/cable_request";
/// Station to runner: part of the answer to a request, an [`Answer`].
pub(crate) const ANSWER: &str = "_ai6/cable_answer";
/// Runner to Station: the harness stopped reading the answer to a request.
pub(crate) const CANCEL: &str = "_ai6/cable_cancel";

/// Request headers MCP's streamable HTTP transport depends on; nothing else is relayed.
const REQUEST_HEADERS: [&str; 5] = [
    "content-type",
    "accept",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
];
const RESPONSE_HEADERS: [&str; 3] = ["content-type", "mcp-session-id", "cache-control"];
/// The methods MCP's streamable HTTP transport uses.
const METHODS: [&str; 3] = ["GET", "POST", "DELETE"];

/// Bytes of the answer read from the endpoint and sent on at once.
const CHUNK_BYTES: usize = 16 * 1024;

fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode(text: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

/// One request of the harness, as the runner relays it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    /// The runner's name for the Session.
    pub(crate) session_id: String,
    /// Unique among this Session's requests.
    pub(crate) id: u64,
    pub(crate) method: String,
    /// The headers of [`REQUEST_HEADERS`] the harness sent.
    #[serde(default)]
    pub(crate) headers: Vec<(String, String)>,
    /// The body, in base64.
    #[serde(default)]
    pub(crate) body: String,
}

/// Part of the answer to one [`Request`]: first the head (`status`, `headers`), then pieces of
/// the body (`data`, in base64), then `end`; or an `error` instead, at any point.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Answer {
    pub(crate) session_id: String,
    pub(crate) id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) headers: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) end: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

impl Answer {
    /// Whether nothing follows this part.
    pub(crate) fn last(&self) -> bool {
        self.end || self.error.is_some()
    }
}

/// A request on its way to the attached Station: the answers as they arrive, and how to tell
/// the Station the harness no longer reads them.
pub(super) struct Opened {
    pub(super) answers: UnboundedReceiver<Answer>,
    pub(super) cancel: Box<dyn FnOnce() + Send>,
}

/// How a runner's relay reaches the Station attached to its Session now.
pub(super) trait Route: Send + Sync {
    /// Sends `request` to the attached Station; `None` when none is attached.
    fn open(&self, request: &Request) -> Option<Opened>;
}

#[derive(Clone)]
struct Shared {
    key: Arc<str>,
    token: Arc<str>,
    route: Arc<dyn Route>,
    next: Arc<AtomicU64>,
    /// The largest request body relayed; MCP requests are small JSON documents.
    request_bytes: usize,
}

/// A running relay, stopped when dropped.
pub(super) struct CableProxy {
    url: String,
    token: String,
    server: JoinHandle<()>,
}

impl CableProxy {
    /// Listens on a loopback port and relays Session `key`'s requests along `route`, within
    /// the `cable_*` keys of `limits`.
    pub(super) async fn start(
        key: &str,
        route: Arc<dyn Route>,
        limits: &RunnerLimits,
    ) -> std::io::Result<Self> {
        let token = uuid::Uuid::new_v4().to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let router = axum::Router::new()
            .route("/mcp", axum::routing::any(relay))
            .with_state(Shared {
                key: key.into(),
                token: token.clone().into(),
                route,
                next: Arc::new(AtomicU64::new(1)),
                request_bytes: limits.cable_request_bytes.or_max(),
            });
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self {
            url: format!("http://{address}/mcp"),
            token,
            server,
        })
    }

    /// Points the harness's Cable endpoint at the relay.
    pub(super) fn add_environment(&self, environment: &mut BTreeMap<String, String>) {
        environment.insert(URL_KEY.to_owned(), self.url.clone());
        environment.insert(TOKEN_KEY.to_owned(), self.token.clone());
    }
}

impl Drop for CableProxy {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn refuse(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
}

/// Tells the Station the harness stopped reading, unless the answer was read to its end.
struct Reading {
    cancel: Option<Box<dyn FnOnce() + Send>>,
}

impl Reading {
    fn finished(&mut self) {
        self.cancel = None;
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}

async fn relay(State(shared): State<Shared>, request: HttpRequest) -> Response {
    // The body is read before any refusal: answering an unread request and closing makes
    // Windows abort the client's send instead of delivering the status.
    let (parts, body) = request.into_parts();
    let Ok(body) = axum::body::to_bytes(body, shared.request_bytes).await else {
        return refuse(StatusCode::PAYLOAD_TOO_LARGE, "the request is too large");
    };
    let presented = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !bool::from(presented.as_bytes().ct_eq(shared.token.as_bytes())) {
        return refuse(StatusCode::UNAUTHORIZED, "a valid bearer token is required");
    }
    let mut headers = Vec::new();
    for name in REQUEST_HEADERS {
        for value in parts.headers.get_all(name) {
            if let Ok(value) = value.to_str() {
                headers.push((name.to_owned(), value.to_owned()));
            }
        }
    }
    let request = Request {
        session_id: shared.key.to_string(),
        id: shared.next.fetch_add(1, Ordering::Relaxed),
        method: parts.method.as_str().to_owned(),
        headers,
        body: encode(&body),
    };
    let Some(Opened {
        mut answers,
        cancel,
    }) = shared.route.open(&request)
    else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "no Station is attached to this Session now; try again later",
        );
    };
    let mut reading = Reading {
        cancel: Some(cancel),
    };
    let head = match answers.recv().await {
        Some(Answer {
            status: Some(status),
            headers,
            ..
        }) => (status, headers),
        Some(Answer {
            error: Some(error), ..
        }) => {
            reading.finished();
            return refuse(StatusCode::BAD_GATEWAY, &error);
        }
        _ => {
            reading.finished();
            return refuse(
                StatusCode::BAD_GATEWAY,
                "the Station's connection ended before it answered",
            );
        }
    };
    // The answer may be a stream that lasts as long as the MCP session: it is handed on piece
    // by piece, and a harness that stops reading tells the Station so.
    let stream = futures_util::stream::unfold(
        (answers, reading),
        |(mut answers, mut reading)| async move {
            loop {
                let answer = answers.recv().await?;
                if answer.last() {
                    reading.finished();
                }
                if let Some(error) = answer.error {
                    return Some((Err(std::io::Error::other(error)), (answers, reading)));
                }
                if let Some(data) = answer.data.as_deref().and_then(decode) {
                    return Some((Ok(Bytes::from(data)), (answers, reading)));
                }
                if answer.end {
                    return None;
                }
            }
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::from_u16(head.0).unwrap_or(StatusCode::BAD_GATEWAY);
    for (name, value) in head.1 {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::from_str(&value))
            && RESPONSE_HEADERS.contains(&name.as_str())
        {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

/// The requests a Session's connection carries to the Station and has no full answer to yet.
#[derive(Default)]
pub(super) struct Pending {
    open: Mutex<BTreeMap<u64, UnboundedSender<Answer>>>,
}

impl Pending {
    /// Waits for the answers to request `id`.
    pub(super) fn add(&self, id: u64) -> UnboundedReceiver<Answer> {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        super::lock_of(&self.open).insert(id, sender);
        receiver
    }

    /// Stops waiting for request `id`.
    pub(super) fn remove(&self, id: u64) {
        super::lock_of(&self.open).remove(&id);
    }

    /// Hands `answer` to whoever waits for it.
    pub(super) fn deliver(&self, answer: Answer) {
        let mut open = super::lock_of(&self.open);
        let last = answer.last();
        let id = answer.id;
        if let Some(waiting) = open.get(&id)
            && (waiting.send(answer).is_err() || last)
        {
            open.remove(&id);
        }
    }

    /// Ends every request: the connection that carried them is gone.
    pub(super) fn clear(&self) {
        super::lock_of(&self.open).clear();
    }
}

/// The Station's side of a runner Session's Cable relay: answers each relayed request from the
/// Station's own Cable endpoint for that Session, with its bearer.
#[cfg_attr(
    not(feature = "cables-mcp"),
    allow(
        dead_code,
        reason = "only a Station with Cable tools answers the relay"
    )
)]
pub(crate) struct CableTunnel {
    shared: Arc<TunnelShared>,
}

struct TunnelShared {
    key: String,
    url: String,
    token: String,
    request_bytes: usize,
    connect_timeout: Duration,
    streams: usize,
    open: AtomicUsize,
    /// The requests being answered, each with whether the harness stopped reading it.
    reading: Mutex<BTreeMap<u64, Arc<AtomicBool>>>,
}

#[cfg_attr(
    not(feature = "cables-mcp"),
    allow(
        dead_code,
        reason = "only a Station with Cable tools answers the relay"
    )
)]
impl CableTunnel {
    /// Answers Session `key`'s relayed requests from the endpoint at `url`, which takes
    /// `token`, within the `cable_*` keys of `limits`.
    pub(crate) fn new(key: &str, url: &str, token: &str, limits: &RunnerLimits) -> Self {
        Self {
            shared: Arc::new(TunnelShared {
                key: key.to_owned(),
                url: url.to_owned(),
                token: token.to_owned(),
                request_bytes: limits.cable_request_bytes.or_max(),
                connect_timeout: limits.cable_connect_timeout.0,
                streams: limits.cable_streams.or_max(),
                open: AtomicUsize::new(0),
                reading: Mutex::new(BTreeMap::new()),
            }),
        }
    }
}

impl crate::directorates::section_00::acp::ExtensionHandler for CableTunnel {
    fn handle(
        &self,
        method: &str,
        params: &serde_json::Value,
        reply: crate::directorates::section_00::acp::Notifier,
    ) {
        let shared = &self.shared;
        let named = params.get("sessionId").and_then(serde_json::Value::as_str);
        if named != Some(shared.key.as_str()) {
            return;
        }
        match method {
            CANCEL => {
                if let Some(id) = params.get("id").and_then(serde_json::Value::as_u64)
                    && let Some(flag) = super::lock_of(&shared.reading).get(&id)
                {
                    flag.store(true, Ordering::Release);
                }
            }
            REQUEST => {
                let Ok(request) = serde_json::from_value::<Request>(params.clone()) else {
                    return;
                };
                let fail = |id: u64, error: String| {
                    send(
                        &reply,
                        &Answer {
                            session_id: shared.key.clone(),
                            id,
                            error: Some(error),
                            ..Answer::default()
                        },
                    );
                };
                if !METHODS.contains(&request.method.as_str()) {
                    return fail(
                        request.id,
                        format!("the Cable endpoint takes no {} request", request.method),
                    );
                }
                let Some(body) =
                    decode(&request.body).filter(|body| body.len() <= shared.request_bytes)
                else {
                    return fail(
                        request.id,
                        "the request is too large or unreadable".to_owned(),
                    );
                };
                if shared.open.fetch_add(1, Ordering::AcqRel) >= shared.streams {
                    shared.open.fetch_sub(1, Ordering::AcqRel);
                    return fail(
                        request.id,
                        format!(
                            "{} Cable requests are open for this Session; try again once one ends",
                            shared.streams
                        ),
                    );
                }
                let id = request.id;
                let stopped = Arc::new(AtomicBool::new(false));
                super::lock_of(&shared.reading).insert(id, stopped.clone());
                let worker = shared.clone();
                let spawned = std::thread::Builder::new()
                    .name("ai6-station-cable".to_owned())
                    .spawn(move || {
                        answer(&worker, request, body, &reply, &stopped);
                        super::lock_of(&worker.reading).remove(&id);
                        worker.open.fetch_sub(1, Ordering::AcqRel);
                    });
                if spawned.is_err() {
                    super::lock_of(&shared.reading).remove(&id);
                    shared.open.fetch_sub(1, Ordering::AcqRel);
                }
            }
            _ => {}
        }
    }
}

fn send(reply: &crate::directorates::section_00::acp::Notifier, answer: &Answer) -> bool {
    serde_json::to_value(answer).is_ok_and(|params| reply.notify(ANSWER, params))
}

/// Makes `request` of the Station's endpoint and sends the answer back as it is read, until it
/// ends, the harness stops reading, or the connection is gone.
fn answer(
    shared: &TunnelShared,
    request: Request,
    body: Vec<u8>,
    reply: &crate::directorates::section_00::acp::Notifier,
    stopped: &AtomicBool,
) {
    use std::io::Read as _;
    let part = |answer: Answer| {
        send(
            reply,
            &Answer {
                session_id: shared.key.clone(),
                id: request.id,
                ..answer
            },
        )
    };
    let mut forward = ureq::http::Request::builder()
        .method(request.method.as_str())
        .uri(shared.url.as_str())
        .header("Authorization", format!("Bearer {}", shared.token));
    for (name, value) in &request.headers {
        if REQUEST_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
            forward = forward.header(name.as_str(), value.as_str());
        }
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_connect(Some(shared.connect_timeout))
        .build()
        .into();
    let response = if body.is_empty() {
        forward.body(()).map(|built| agent.run(built))
    } else {
        forward.body(body).map(|built| agent.run(built))
    };
    let response = match response {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            part(Answer {
                error: Some(format!(
                    "the Station's Cable endpoint did not answer: {error}"
                )),
                ..Answer::default()
            });
            return;
        }
        Err(error) => {
            part(Answer {
                error: Some(format!("the request could not be relayed: {error}")),
                ..Answer::default()
            });
            return;
        }
    };
    let headers = RESPONSE_HEADERS
        .iter()
        .filter_map(|name| {
            let value = response.headers().get(*name)?.to_str().ok()?;
            Some(((*name).to_owned(), value.to_owned()))
        })
        .collect();
    if !part(Answer {
        status: Some(response.status().as_u16()),
        headers,
        ..Answer::default()
    }) {
        return;
    }
    let mut reader = response.into_body().into_reader();
    let mut buffer = vec![0u8; CHUNK_BYTES];
    loop {
        if stopped.load(Ordering::Acquire) {
            return;
        }
        match reader.read(&mut buffer) {
            Ok(0) => {
                part(Answer {
                    end: true,
                    ..Answer::default()
                });
                return;
            }
            Ok(count) => {
                if stopped.load(Ordering::Acquire)
                    || !part(Answer {
                        data: Some(encode(&buffer[..count])),
                        ..Answer::default()
                    })
                {
                    return;
                }
            }
            Err(error) => {
                part(Answer {
                    error: Some(format!(
                        "the Station's Cable endpoint stopped answering: {error}"
                    )),
                    ..Answer::default()
                });
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directorates::section_00::acp::ExtensionHandler as _;

    /// A stand-in Station Cable endpoint that echoes what it was sent.
    async fn echo_station(token: &'static str) -> String {
        async fn echo(State(token): State<&'static str>, request: HttpRequest) -> Response {
            let authorized = request
                .headers()
                .get(header::AUTHORIZATION)
                .is_some_and(|value| value.as_bytes() == format!("Bearer {token}").as_bytes());
            if !authorized {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let session = request
                .headers()
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("none")
                .to_owned();
            let leaked = request.headers().get("x-extra").is_some();
            let body = axum::body::to_bytes(request.into_body(), 1024)
                .await
                .unwrap();
            let mut response = axum::Json(serde_json::json!({
                "session": session,
                "leaked": leaked,
                "body": String::from_utf8_lossy(&body),
            }))
            .into_response();
            response
                .headers_mut()
                .insert("mcp-session-id", HeaderValue::from_static("upstream-1"));
            response
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = axum::Router::new()
            .route("/mcp", axum::routing::any(echo))
            .with_state(token);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("http://{address}/mcp")
    }

    /// The connection between a runner and a Station, in memory: the Station's tunnel answers
    /// what the runner's relay sends, while one is attached.
    struct Wire {
        station: Mutex<Option<Arc<CableTunnel>>>,
        pending: Arc<Pending>,
        cancelled: Arc<Mutex<Vec<u64>>>,
    }

    struct Back(Arc<Pending>);

    impl Route for Wire {
        fn open(&self, request: &Request) -> Option<Opened> {
            let station = super::super::lock_of(&self.station).clone()?;
            let answers = self.pending.add(request.id);
            let (id, cancelled, station_side) =
                (request.id, self.cancelled.clone(), station.clone());
            let pending = self.pending.clone();
            let notifier = notifier_for(Back(pending.clone()));
            station.handle(
                REQUEST,
                &serde_json::to_value(request).unwrap(),
                notifier.clone(),
            );
            Some(Opened {
                answers,
                cancel: Box::new(move || {
                    pending.remove(id);
                    super::super::lock_of(&cancelled).push(id);
                    station_side.handle(
                        CANCEL,
                        &serde_json::json!({"sessionId": "k1", "id": id}),
                        notifier,
                    );
                }),
            })
        }
    }

    /// A notifier that hands answers straight to the runner's pending requests.
    fn notifier_for(back: Back) -> crate::directorates::section_00::acp::Notifier {
        crate::directorates::section_00::acp::Notifier::new(move |method, params| {
            assert_eq!(method, ANSWER);
            back.0.deliver(serde_json::from_value(params).unwrap());
            true
        })
    }

    fn post(url: &str, token: &str, body: &str) -> (u16, Option<String>, serde_json::Value) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let mut response = agent
            .post(url)
            .header("Authorization", &format!("Bearer {token}"))
            .header("mcp-session-id", "harness-1")
            .header("x-extra", "kept out")
            .content_type("application/json")
            .send(body)
            .unwrap();
        let session = response
            .headers()
            .get("mcp-session-id")
            .map(|value| value.to_str().unwrap().to_owned());
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap();
        (
            status,
            session,
            serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_harness_keeps_its_endpoint_while_the_station_behind_it_changes() {
        let wire = Arc::new(Wire {
            station: Mutex::new(None),
            pending: Arc::default(),
            cancelled: Arc::default(),
        });
        let proxy = CableProxy::start("k1", wire.clone(), &RunnerLimits::default())
            .await
            .unwrap();
        let mut environment = BTreeMap::new();
        proxy.add_environment(&mut environment);
        let (url, token) = (environment[URL_KEY].clone(), environment[TOKEN_KEY].clone());
        let call = |token: String, body: &'static str| {
            let url = url.clone();
            tokio::task::spawn_blocking(move || post(&url, &token, body))
        };

        let (status, _, _) = call(token.clone(), "{}").await.unwrap();
        assert_eq!(status, 503, "no Station is attached yet");
        let (status, _, _) = call("wrong".to_owned(), "{}").await.unwrap();
        assert_eq!(status, 401);

        // The Station answers with its own endpoint's bearer, which the harness never holds.
        let first = echo_station("station-token").await;
        *super::super::lock_of(&wire.station) = Some(Arc::new(CableTunnel::new(
            "k1",
            &first,
            "station-token",
            &RunnerLimits::default(),
        )));
        let (status, session, body) = call(token.clone(), r#"{"id":1}"#).await.unwrap();
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["session"], "harness-1");
        assert_eq!(body["body"], r#"{"id":1}"#);
        assert_eq!(body["leaked"], false, "only MCP's headers are relayed");
        assert_eq!(session.as_deref(), Some("upstream-1"));
        assert!(
            super::super::lock_of(&wire.cancelled).is_empty(),
            "an answer read to its end cancels nothing"
        );

        // Another Station attaches: the same URL and token reach it.
        let second = echo_station("second-token").await;
        *super::super::lock_of(&wire.station) = Some(Arc::new(CableTunnel::new(
            "k1",
            &second,
            "second-token",
            &RunnerLimits::default(),
        )));
        let (status, _, body) = call(token.clone(), r#"{"id":2}"#).await.unwrap();
        assert_eq!(status, 200);
        assert_eq!(body["body"], r#"{"id":2}"#);

        // A Station that cannot reach its endpoint says so.
        *super::super::lock_of(&wire.station) = Some(Arc::new(CableTunnel::new(
            "k1",
            "http://127.0.0.1:1/mcp",
            "x",
            &RunnerLimits::default(),
        )));
        let (status, _, body) = call(token, "{}").await.unwrap();
        assert_eq!(status, 502);
        assert!(
            body["error"].as_str().unwrap().contains("did not answer"),
            "{body}"
        );
    }

    #[test]
    fn the_station_answers_only_its_own_session_within_its_limits() {
        let seen: Arc<Mutex<Vec<Answer>>> = Arc::default();
        let recorder = seen.clone();
        let notifier = crate::directorates::section_00::acp::Notifier::new(move |_, params| {
            super::super::lock_of(&recorder).push(serde_json::from_value(params).unwrap());
            true
        });
        let limits = RunnerLimits {
            cable_streams: crate::Limit::at(0),
            cable_request_bytes: crate::Limit::at(4),
            ..RunnerLimits::default()
        };
        let tunnel = CableTunnel::new("k1", "http://127.0.0.1:1/mcp", "t", &limits);
        let request = |session: &str, id: u64, method: &str, body: &[u8]| {
            serde_json::to_value(Request {
                session_id: session.to_owned(),
                id,
                method: method.to_owned(),
                headers: Vec::new(),
                body: encode(body),
            })
            .unwrap()
        };
        tunnel.handle(
            REQUEST,
            &request("other", 1, "POST", b"{}"),
            notifier.clone(),
        );
        tunnel.handle(REQUEST, &request("k1", 2, "PUT", b"{}"), notifier.clone());
        tunnel.handle(
            REQUEST,
            &request("k1", 3, "POST", b"too large"),
            notifier.clone(),
        );
        tunnel.handle(REQUEST, &request("k1", 4, "POST", b"{}"), notifier);
        let seen = super::super::lock_of(&seen).clone();
        let errors: Vec<_> = seen
            .iter()
            .map(|answer| (answer.id, answer.error.clone().unwrap()))
            .collect();
        assert_eq!(
            errors.len(),
            3,
            "another Session's request is not answered: {errors:?}"
        );
        assert!(errors[0].1.contains("no PUT request"), "{errors:?}");
        assert!(errors[1].1.contains("too large"), "{errors:?}");
        assert!(
            errors[2].1.contains("0 Cable requests are open"),
            "{errors:?}"
        );
    }
}
