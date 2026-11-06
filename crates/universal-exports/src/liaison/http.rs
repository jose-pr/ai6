//! A2A's standard HTTP JSON-RPC binding on the gateway: `POST /v1/a2a`, and the card at
//! `/v1/a2a/.well-known/agent-card.json`. A caller signs in as for any route, with the gateway's
//! bearer token; the person it is must be a configured peer's principal, and is that peer.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header::CONTENT_TYPE};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use coqui_a2a::error::{A2aError, ErrorKind};
use coqui_a2a::model::{
    AgentInterface, CancelTaskRequest, GetTaskRequest, ListTasksRequest, SendMessageRequest,
    SubscribeToTaskRequest, params_as,
};
use coqui_a2a::wire::{Method, Request, Response as RpcResponse};
use coqui_a2a::{A2A_VERSION, Events};
use serde_json::{Value, json};

use super::{Calling, Liaison, LiaisonPeer, Open};
use crate::Gateway;

/// The path of the binding's endpoint.
pub const PATH: &str = "/v1/a2a";

pub(crate) fn routes() -> Router<Gateway> {
    Router::new()
        .route(PATH, post(rpc))
        .route("/v1/a2a/", post(rpc))
        .route("/v1/a2a/.well-known/agent-card.json", get(card))
}

/// Liaison and the peer the signed-in person is. No Liaison is 404, no sign-in 401, and a person
/// who is no peer's principal 403.
async fn caller(
    gateway: &Gateway,
    headers: &HeaderMap,
) -> Result<(Liaison, LiaisonPeer), Box<Response>> {
    let liaison = gateway
        .liaison()
        .ok_or_else(|| Box::new(StatusCode::NOT_FOUND.into_response()))?;
    let token = crate::bearer(headers).map_err(|error| Box::new(error.into_response()))?;
    let person = crate::resolve_user(gateway, token)
        .await
        .map_err(|error| Box::new(error.into_response()))?;
    let peer = liaison
        .peer_for(&person)
        .cloned()
        .ok_or_else(|| Box::new(StatusCode::FORBIDDEN.into_response()))?;
    Ok((liaison, peer))
}

async fn card(State(gateway): State<Gateway>, headers: HeaderMap) -> Response {
    let (liaison, peer) = match caller(&gateway, &headers).await {
        Ok(found) => found,
        Err(response) => return *response,
    };
    let origin = gateway
        .config
        .public_origin
        .trim_end_matches('/')
        .to_owned();
    let mut card = blocking(move || liaison.card(Some(&peer))).await;
    card.supported_interfaces = vec![AgentInterface {
        url: format!("{origin}{PATH}"),
        protocol_binding: "JSONRPC".to_owned(),
        protocol_version: A2A_VERSION.to_owned(),
        ..AgentInterface::default()
    }];
    card.security_schemes = json!({
        "bearer": {"httpAuthSecurityScheme": {"scheme": "Bearer"}}
    })
    .as_object()
    .cloned();
    axum::Json(card).into_response()
}

/// Clears its flag when dropped: the handler's future goes when its client does.
struct Present(Arc<AtomicBool>);

impl Drop for Present {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

async fn rpc(State(gateway): State<Gateway>, headers: HeaderMap, body: Bytes) -> Response {
    let (liaison, peer) = match caller(&gateway, &headers).await {
        Ok(found) => found,
        Err(response) => return *response,
    };
    let request = match Request::decode(&body) {
        Ok(request) => request,
        Err((id, error)) => return answer(RpcResponse::error(id, error)),
    };
    let id = request.id.clone();
    let refuse = |error: A2aError| answer(RpcResponse::error(id.clone(), error));
    if let Some(version) = headers
        .get("a2a-version")
        .and_then(|value| value.to_str().ok())
        && !version
            .split(',')
            .all(|one| one.trim().split('.').next() == Some("1"))
    {
        return refuse(A2aError::standard(ErrorKind::VersionNotSupported));
    }
    let Some(method) = Method::from_name(&request.method) else {
        return refuse(A2aError::standard(ErrorKind::MethodNotFound));
    };
    if method.is_push() {
        return refuse(A2aError::standard(ErrorKind::PushNotificationNotSupported));
    }
    let present = Arc::new(AtomicBool::new(true));
    let guard = Present(present.clone());
    let open: Open = {
        let present = present.clone();
        Arc::new(move || present.load(Ordering::SeqCst))
    };
    let proof = format!("signed-in:{}", peer.principal);
    let params = request.params;
    let outcome: Result<Outcome, A2aError> = blocking(move || {
        let calling = Calling { peer: &peer, proof };
        match method {
            Method::SendMessage => {
                let request: SendMessageRequest = params_as(params)?;
                reject_push(&request)?;
                let response = liaison.send_message(&calling, &request, &open)?;
                Ok(Outcome::One(to_value(&response)?))
            }
            Method::SendStreamingMessage => {
                let request: SendMessageRequest = params_as(params)?;
                reject_push(&request)?;
                Ok(Outcome::Stream(
                    liaison.send_streaming(&calling, &request, open)?,
                ))
            }
            Method::GetTask => {
                let request: GetTaskRequest = params_as(params)?;
                Ok(Outcome::One(to_value(&liaison.get_task(&peer, &request)?)?))
            }
            Method::ListTasks => {
                let request: ListTasksRequest = params_as(params)?;
                Ok(Outcome::One(to_value(
                    &liaison.list_tasks(&peer, &request)?,
                )?))
            }
            Method::CancelTask => {
                let request: CancelTaskRequest = params_as(params)?;
                Ok(Outcome::One(to_value(
                    &liaison.cancel_task(&peer, &request)?,
                )?))
            }
            Method::SubscribeToTask => {
                let request: SubscribeToTaskRequest = params_as(params)?;
                Ok(Outcome::Stream(liaison.subscribe(&peer, &request, open)?))
            }
            Method::GetExtendedAgentCard => Err(A2aError::standard(
                ErrorKind::ExtendedAgentCardNotConfigured,
            )),
            _ => Err(A2aError::standard(ErrorKind::MethodNotFound)),
        }
    })
    .await;
    match outcome {
        Ok(Outcome::One(result)) => answer(RpcResponse::result(id, result)),
        Ok(Outcome::Stream(events)) => stream(id, events, present, guard),
        Err(error) => refuse(error),
    }
}

enum Outcome {
    One(Value),
    Stream(Events),
}

fn reject_push(request: &SendMessageRequest) -> Result<(), A2aError> {
    if request
        .configuration
        .as_ref()
        .is_some_and(|configuration| configuration.task_push_notification_config.is_some())
    {
        return Err(A2aError::standard(ErrorKind::PushNotificationNotSupported));
    }
    Ok(())
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, A2aError> {
    serde_json::to_value(value).map_err(|_| A2aError::standard(ErrorKind::Internal))
}

fn answer(response: RpcResponse) -> Response {
    ([(CONTENT_TYPE, "application/json")], response.encode()).into_response()
}

/// Server-Sent Events, one JSON-RPC response per item, until the stream ends or the client goes.
fn stream(id: Value, events: Events, present: Arc<AtomicBool>, guard: Present) -> Response {
    let (sender, receiver) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    tokio::task::spawn_blocking(move || {
        for item in events {
            let response = match item {
                Ok(item) => match serde_json::to_value(&item) {
                    Ok(value) => RpcResponse::result(id.clone(), value),
                    Err(_) => {
                        RpcResponse::error(id.clone(), A2aError::standard(ErrorKind::Internal))
                    }
                },
                Err(error) => RpcResponse::error(id.clone(), error),
            };
            if sender.blocking_send(response.encode()).is_err() {
                present.store(false, Ordering::SeqCst);
                break;
            }
        }
    });
    // The guard goes with the body, so a client that leaves ends the stream's wait.
    let items =
        futures_util::stream::unfold((receiver, guard), |(mut receiver, guard)| async move {
            let bytes = receiver.recv().await?;
            let event = Event::default().data(String::from_utf8_lossy(&bytes));
            Some((Ok::<_, std::convert::Infallible>(event), (receiver, guard)))
        });
    Sse::new(items).into_response()
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => value,
        Err(error) => std::panic::resume_unwind(error.into_panic()),
    }
}
