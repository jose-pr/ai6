//! The Cable MCP endpoint a runner hands a harness: a loopback address with a token of its
//! own that relays every request to the Cable endpoint of whichever Station is attached now.
//! The harness keeps one URL and token while Stations come and go; with none attached it is
//! told so and may try again.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::task::JoinHandle;

/// The environment variable that carries the Cable MCP endpoint's URL to a harness.
pub(crate) const URL_KEY: &str = "AI6_CABLE_MCP_URL";
/// The environment variable that carries the Cable MCP endpoint's bearer token to a harness.
pub(crate) const TOKEN_KEY: &str = "AI6_CABLE_MCP_TOKEN";

/// The largest request body relayed; MCP requests are small JSON documents.
const MAX_REQUEST: usize = 4 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Request headers MCP's streamable HTTP transport depends on; nothing else is relayed.
const REQUEST_HEADERS: [&str; 5] = [
    "content-type",
    "accept",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
];
const RESPONSE_HEADERS: [&str; 3] = ["content-type", "mcp-session-id", "cache-control"];

/// Where the attached Station serves its Cable endpoint, as reachable from the runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Upstream {
    pub(crate) url: String,
    pub(crate) token: String,
}

impl Upstream {
    /// The endpoint a Station put in a harness environment, if it put one there.
    pub(crate) fn from_environment(environment: &BTreeMap<String, String>) -> Option<Self> {
        Some(Self {
            url: environment.get(URL_KEY)?.clone(),
            token: environment.get(TOKEN_KEY)?.clone(),
        })
    }
}

#[derive(Clone)]
struct Shared {
    token: Arc<str>,
    upstream: Arc<RwLock<Option<Upstream>>>,
}

/// A running relay, stopped when dropped.
pub(super) struct CableProxy {
    url: String,
    token: String,
    upstream: Arc<RwLock<Option<Upstream>>>,
    server: JoinHandle<()>,
}

impl CableProxy {
    /// Listens on a loopback port and relays to `upstream` until another is set.
    pub(super) async fn start(upstream: Option<Upstream>) -> std::io::Result<Self> {
        let token = uuid::Uuid::new_v4().to_string();
        let upstream = Arc::new(RwLock::new(upstream));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let router = axum::Router::new()
            .route("/mcp", axum::routing::any(relay))
            .with_state(Shared {
                token: token.clone().into(),
                upstream: upstream.clone(),
            });
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self {
            url: format!("http://{address}/mcp"),
            token,
            upstream,
            server,
        })
    }

    /// Points the harness's Cable endpoint at the relay instead of a Station.
    pub(super) fn add_environment(&self, environment: &mut BTreeMap<String, String>) {
        environment.insert(URL_KEY.to_owned(), self.url.clone());
        environment.insert(TOKEN_KEY.to_owned(), self.token.clone());
    }

    /// Relays to `upstream` from now on.
    pub(super) fn set_upstream(&self, upstream: Upstream) {
        if let Ok(mut current) = self.upstream.write() {
            *current = Some(upstream);
        }
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

async fn relay(State(shared): State<Shared>, request: Request) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !bool::from(presented.as_bytes().ct_eq(shared.token.as_bytes())) {
        return refuse(StatusCode::UNAUTHORIZED, "a valid bearer token is required");
    }
    let Some(upstream) = shared.upstream.read().ok().and_then(|up| up.clone()) else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "no Station is attached to this Session now; try again later",
        );
    };
    let (parts, body) = request.into_parts();
    let Ok(body) = axum::body::to_bytes(body, MAX_REQUEST).await else {
        return refuse(StatusCode::PAYLOAD_TOO_LARGE, "the request is too large");
    };
    let mut forward = ureq::http::Request::builder()
        .method(parts.method.as_str())
        .uri(upstream.url.as_str())
        .header("Authorization", format!("Bearer {}", upstream.token));
    for name in REQUEST_HEADERS {
        for value in parts.headers.get_all(name) {
            forward = forward.header(name, value.as_bytes());
        }
    }
    let (head_sender, head) = tokio::sync::oneshot::channel();
    let (chunk_sender, chunks) = tokio::sync::mpsc::channel::<Bytes>(16);
    // The upstream answer may be a stream that lasts as long as the MCP session, so it is
    // read on a thread of its own and handed over chunk by chunk.
    let spawned = std::thread::Builder::new()
        .name("ai6-runner-cable".to_owned())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .timeout_connect(Some(CONNECT_TIMEOUT))
                .build()
                .into();
            let response = if body.is_empty() {
                forward.body(()).map(|request| agent.run(request))
            } else {
                forward
                    .body(body.to_vec())
                    .map(|request| agent.run(request))
            };
            let response = match response {
                Ok(Ok(response)) => response,
                _ => {
                    let _ = head_sender.send(None);
                    return;
                }
            };
            let status = response.status().as_u16();
            let headers: Vec<(String, Vec<u8>)> = RESPONSE_HEADERS
                .iter()
                .filter_map(|name| {
                    response
                        .headers()
                        .get(*name)
                        .map(|value| ((*name).to_owned(), value.as_bytes().to_vec()))
                })
                .collect();
            if head_sender.send(Some((status, headers))).is_err() {
                return;
            }
            let mut reader = response.into_body().into_reader();
            let mut buffer = vec![0u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        if chunk_sender
                            .blocking_send(Bytes::copy_from_slice(&buffer[..count]))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
    if spawned.is_err() {
        return refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the relay could not start",
        );
    }
    let Ok(Some((status, headers))) = head.await else {
        return refuse(
            StatusCode::BAD_GATEWAY,
            "the attached Station's Cable endpoint did not answer",
        );
    };
    let stream = futures_util::stream::unfold(chunks, |mut chunks| async move {
        chunks
            .recv()
            .await
            .map(|chunk| (Ok::<_, std::io::Error>(chunk), chunks))
    });
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    for (name, value) in headers {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::from_bytes(&value))
        {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in Station Cable endpoint that echoes what it was sent.
    async fn echo_station(token: &'static str, name: &'static str) -> Upstream {
        async fn echo(
            State((token, name)): State<(&'static str, &'static str)>,
            request: Request,
        ) -> Response {
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
            let body = axum::body::to_bytes(request.into_body(), 1024)
                .await
                .unwrap();
            let mut response = axum::Json(serde_json::json!({
                "station": name,
                "session": session,
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
            .with_state((token, name));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Upstream {
            url: format!("http://{address}/mcp"),
            token: token.to_owned(),
        }
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
        let first = echo_station("first-token", "first").await;
        let proxy = CableProxy::start(None).await.unwrap();
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

        proxy.set_upstream(first);
        let (status, session, body) = call(token.clone(), r#"{"id":1}"#).await.unwrap();
        assert_eq!(status, 200);
        assert_eq!(body["station"], "first");
        assert_eq!(body["session"], "harness-1");
        assert_eq!(body["body"], r#"{"id":1}"#);
        assert_eq!(session.as_deref(), Some("upstream-1"));

        proxy.set_upstream(echo_station("second-token", "second").await);
        let (status, _, body) = call(token, r#"{"id":2}"#).await.unwrap();
        assert_eq!(status, 200);
        assert_eq!(body["station"], "second");
    }
}
