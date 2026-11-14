//! The Station's link to Universal Exports for attached people: it polls for what clients ask
//! of the Runs this Station drives, and delivers what those Runs show them.
//!
//! One thread polls and applies each request to the [`RunHub`]; each attached client has a thread
//! that delivers its replay and then its live frames. A client Universal Exports no longer knows
//! (its socket closed) is detached.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use serde_json::json;
use ureq::tls::TlsConfig;
use url::Url;

use super::super::{DaemonState, unix_now};
use crate::directorates::section_00::hub::{
    Attachment, HubCommand, HubDelivery, HubError, HubFrame, HubOp, RunHub, RunTarget,
};
use crate::station::limits::AttachLimits;
use crate::station::publisher::gateway_agent;

/// How long a client's thread waits for a frame before it looks at whether the Station stops.
const IDLE_SLICE: Duration = Duration::from_millis(250);

/// What became of a delivery.
#[derive(Debug, PartialEq, Eq)]
enum Delivered {
    Taken,
    /// Universal Exports has no such client any more.
    Gone,
    /// It could not be delivered after every try.
    Lost,
}

#[derive(Clone)]
struct Link {
    origin: Url,
    token: String,
    tls: TlsConfig,
    station: String,
    limits: AttachLimits,
}

/// Starts the link when the Station publishes to Universal Exports. The thread ends within one
/// poll of the Station stopping.
pub(in super::super) fn spawn(state: &DaemonState) -> std::io::Result<()> {
    let Some(publisher) = &state.publisher else {
        return Ok(());
    };
    let (origin, token, tls) = publisher.link();
    let link = Link {
        origin,
        token,
        tls,
        station: state.station_id.clone(),
        limits: state.station.limits().attach.clone(),
    };
    let hub: Arc<dyn RunHub> = state.hub.clone();
    let stopping = state.stopping.clone();
    std::thread::Builder::new()
        .name("ai6-attach".to_owned())
        .spawn(move || link.take_commands(&hub, &stopping))
        .map(drop)
}

impl Link {
    fn agent(&self, wait: Duration) -> ureq::Agent {
        gateway_agent(&self.tls, self.limits.call_margin.0 + wait)
    }

    fn pause(&self, stopping: &AtomicBool) {
        let until = std::time::Instant::now() + self.limits.retry_delay.0;
        while !stopping.load(Ordering::Acquire) && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50).min(self.limits.retry_delay.0));
        }
    }

    /// Polls for requests and applies them, until the Station stops.
    fn take_commands(&self, hub: &Arc<dyn RunHub>, stopping: &Arc<AtomicBool>) {
        let Ok(mut endpoint) = self.origin.join("v1/station/hub/next") else {
            return;
        };
        let wait = self.limits.command_wait.0;
        endpoint
            .query_pairs_mut()
            .append_pair("station", &self.station)
            .append_pair("wait_ms", &wait.as_millis().to_string());
        let agent = self.agent(wait);
        while !stopping.load(Ordering::Acquire) {
            let response = agent
                .get(endpoint.as_str())
                .header("Authorization", &format!("Bearer {}", self.token))
                .call();
            match response {
                Ok(mut response) if response.status().as_u16() == 200 => {
                    let commands = response
                        .body_mut()
                        .read_json::<Vec<HubCommand>>()
                        .unwrap_or_default();
                    for command in commands {
                        self.apply(hub, stopping, command);
                    }
                }
                Ok(response) if response.status().as_u16() == 204 => {}
                _ => self.pause(stopping),
            }
        }
    }

    fn apply(&self, hub: &Arc<dyn RunHub>, stopping: &Arc<AtomicBool>, command: HubCommand) {
        let HubCommand {
            target,
            client,
            person,
            op,
        } = command;
        let refused = match op {
            HubOp::Attach { after } => match hub.attach(&target, &client, &person, after) {
                Ok(attachment) => {
                    let (link, forwarding, stopped) = (self.clone(), hub.clone(), stopping.clone());
                    let (watched, named) = (target.clone(), client.clone());
                    let spawned = std::thread::Builder::new()
                        .name("ai6-attach-client".to_owned())
                        .spawn(move || {
                            link.forward(&forwarding, &stopped, &watched, &named, attachment);
                        });
                    if spawned.is_err() {
                        hub.detach(&target, &client);
                        Some((
                            "attach",
                            HubError::Failed("no thread to serve it".to_owned()),
                        ))
                    } else {
                        None
                    }
                }
                Err(error) => Some(("attach", error)),
            },
            HubOp::Detach => {
                hub.detach(&target, &client);
                None
            }
            HubOp::Prompt { text } => hub
                .prompt(&target, &client, &person, &text)
                .err()
                .map(|error| ("prompt", error)),
            HubOp::Answer { request, option } => hub
                .answer(&target, &client, &person, request, &option)
                .err()
                .map(|error| ("answer", error)),
            HubOp::Interrupt => hub
                .interrupt(&target, &client, &person)
                .err()
                .map(|error| ("interrupt", error)),
        };
        if let Some((what, error)) = refused {
            let mut frames = vec![
                HubFrame::new(
                    "error",
                    unix_now(),
                    json!({"reason": error.to_string(), "refused": what}),
                )
                .to(&client),
            ];
            // A client that cannot attach has nothing to wait for.
            if what == "attach" {
                frames.push(
                    HubFrame::new("closed", unix_now(), json!({"reason": error.to_string()}))
                        .to(&client),
                );
            }
            let _ = self.deliver(&target, frames, stopping);
        }
    }

    /// Delivers `attachment`'s replay, then each live frame, to `client`, until it is detached,
    /// the Run closes or Universal Exports lets the client go.
    fn forward(
        &self,
        hub: &Arc<dyn RunHub>,
        stopping: &AtomicBool,
        target: &RunTarget,
        client: &str,
        attachment: Attachment,
    ) {
        let batch = self.limits.delivery_frames.or_max().max(1);
        let Attachment { replay, live, .. } = attachment;
        let mut replay = replay.into_iter().map(|frame| frame.to(client)).peekable();
        while replay.peek().is_some() {
            let chunk: Vec<HubFrame> = replay.by_ref().take(batch).collect();
            if self.deliver(target, chunk, stopping) != Delivered::Taken {
                hub.detach(target, client);
                return;
            }
        }
        while !stopping.load(Ordering::Acquire) {
            let first = match live.recv_timeout(IDLE_SLICE) {
                Ok(frame) => frame,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let mut chunk = vec![first.to(client)];
            let mut ended = false;
            while chunk.len() < batch {
                match live.try_recv() {
                    Ok(frame) => chunk.push(frame.to(client)),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        ended = true;
                        break;
                    }
                }
            }
            if self.deliver(target, chunk, stopping) != Delivered::Taken {
                hub.detach(target, client);
                return;
            }
            if ended {
                return;
            }
        }
    }

    fn deliver(
        &self,
        target: &RunTarget,
        frames: Vec<HubFrame>,
        stopping: &AtomicBool,
    ) -> Delivered {
        let Ok(endpoint) = self.origin.join("v1/station/hub/frames") else {
            return Delivered::Lost;
        };
        let delivery = HubDelivery {
            station: self.station.clone(),
            target: target.clone(),
            frames,
        };
        let agent = self.agent(Duration::ZERO);
        let attempts = self.limits.delivery_attempts.or_max().max(1);
        for _ in 0..attempts {
            let sent = agent
                .post(endpoint.as_str())
                .header("Authorization", &format!("Bearer {}", self.token))
                .send_json(&delivery);
            match sent {
                Ok(_) => return Delivered::Taken,
                Err(ureq::Error::StatusCode(403 | 404 | 409)) => return Delivered::Gone,
                Err(_) => {
                    if stopping.load(Ordering::Acquire) {
                        return Delivered::Lost;
                    }
                    self.pause(stopping);
                }
            }
        }
        Delivered::Lost
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Instant;

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};

    use super::super::{LocalHub, RunPort};
    use super::*;
    use crate::bounds::Seconds;

    /// What a stand-in Universal Exports hands the Station and was handed back.
    #[derive(Default)]
    struct Stub {
        commands: VecDeque<Vec<HubCommand>>,
        deliveries: Vec<HubDelivery>,
    }

    type Shared = Arc<Mutex<Stub>>;

    async fn next(State(stub): State<Shared>) -> Result<Json<Vec<HubCommand>>, StatusCode> {
        let batch = stub.lock().unwrap().commands.pop_front();
        match batch {
            Some(batch) => Ok(Json(batch)),
            None => {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Err(StatusCode::NO_CONTENT)
            }
        }
    }

    async fn frames(State(stub): State<Shared>, Json(delivery): Json<HubDelivery>) -> StatusCode {
        stub.lock().unwrap().deliveries.push(delivery);
        StatusCode::NO_CONTENT
    }

    /// A port that records what people typed into it.
    #[derive(Default)]
    struct Recording(Mutex<Vec<(String, String)>>);

    impl RunPort for Recording {
        fn prompt(&self, person: &str, text: &str) -> Result<(), HubError> {
            self.0
                .lock()
                .unwrap()
                .push((person.to_owned(), text.to_owned()));
            Ok(())
        }

        fn interrupt(&self, _: &str) -> Result<(), HubError> {
            Ok(())
        }
    }

    fn delivered(stub: &Shared, client: &str) -> Vec<HubFrame> {
        stub.lock()
            .unwrap()
            .deliveries
            .iter()
            .flat_map(|delivery| delivery.frames.iter().cloned())
            .filter(|frame| frame.client.as_deref() == Some(client))
            .collect()
    }

    async fn until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !done() {
            assert!(Instant::now() < deadline, "never happened: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_link_applies_what_the_gateway_hands_it_and_delivers_what_the_run_shows() {
        let stub = Shared::default();
        let app = Router::new()
            .route("/v1/station/hub/next", get(next))
            .route("/v1/station/hub/frames", post(frames))
            .with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let served = tokio::spawn(async move { axum::serve(listener, app).await });

        let hub = Arc::new(LocalHub::default());
        let target = RunTarget::Charge {
            scope: "operation:G".to_owned(),
        };
        let port = Arc::new(Recording::default());
        let room = hub.open(&target, port.clone());
        room.publish_error("older");

        let link = Link {
            origin,
            token: "station-token".to_owned(),
            tls: TlsConfig::default(),
            station: "hq".to_owned(),
            limits: AttachLimits {
                command_wait: Seconds::new(1),
                retry_delay: Seconds::new(1),
                ..AttachLimits::default()
            },
        };
        let stopping = Arc::new(AtomicBool::new(false));
        let polling = std::thread::spawn({
            let (hub, stopping): (Arc<dyn RunHub>, _) = (hub.clone(), stopping.clone());
            move || link.take_commands(&hub, &stopping)
        });
        let ask = |op: HubOp, target: &RunTarget| {
            stub.lock()
                .unwrap()
                .commands
                .push_back(vec![HubCommand::new(target.clone(), "c1", "alice", op)]);
        };

        // Attaching delivers the history, then the end of it, to that client alone.
        ask(HubOp::Attach { after: None }, &target);
        until("the replay", || {
            delivered(&stub, "c1").iter().any(|f| f.kind == "ready")
        })
        .await;
        let kinds: Vec<String> = delivered(&stub, "c1").into_iter().map(|f| f.kind).collect();
        assert_eq!(kinds[..2], ["error", "ready"]);
        // Then what the Run shows follows.
        room.publish_event(&coqui_acp_provider::AgentEvent::new(
            "text",
            Some("hello".to_owned()),
        ));
        until("a live frame", || {
            delivered(&stub, "c1")
                .iter()
                .any(|f| f.kind == "event" && f.data["text"] == "hello")
        })
        .await;

        // A person's prompt reaches the Run, and everyone is shown it.
        ask(
            HubOp::Prompt {
                text: "how goes it".to_owned(),
            },
            &target,
        );
        until("the prompt", || !port.0.lock().unwrap().is_empty()).await;
        assert_eq!(
            port.0.lock().unwrap()[0],
            ("alice".to_owned(), "how goes it".to_owned())
        );
        until("the prompt shown", || {
            delivered(&stub, "c1")
                .iter()
                .any(|f| f.kind == "prompt" && f.data["from"] == "alice")
        })
        .await;

        // What this Station does not run is refused to the person who asked.
        let elsewhere = RunTarget::Mission {
            operation: "G".to_owned(),
            mission: "not-here".to_owned(),
        };
        ask(
            HubOp::Prompt {
                text: "anyone".to_owned(),
            },
            &elsewhere,
        );
        until("the refusal", || {
            delivered(&stub, "c1")
                .iter()
                .any(|f| f.kind == "error" && f.data["refused"] == "prompt")
        })
        .await;

        // Detaching lets the client go; the Run is untouched.
        ask(HubOp::Detach, &target);
        until("the detach", || room.attached() == 0).await;
        assert!(hub.room(&target).is_some());

        stopping.store(true, Ordering::Release);
        tokio::task::spawn_blocking(move || polling.join().unwrap())
            .await
            .unwrap();
        served.abort();
    }
}
