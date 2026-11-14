//! The Station's link to Universal Exports for attached people: it polls for what clients ask
//! of the Runs this Station drives, and delivers what those Runs show them.
//!
//! One thread polls and hands each request to a worker of its client, which applies them in the
//! order they came, so a slow Run (or a refusal that has to be delivered) holds up only the client
//! it is for. Each attached client also has a thread that delivers its replay and then its live
//! frames. A client Universal Exports no longer knows (its socket closed) is detached.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::json;
use ureq::tls::TlsConfig;
use url::Url;

use super::super::{DaemonState, unix_now};
use crate::directorates::section_00::hub::{
    Attachment, HubCommand, HubDelivery, HubError, HubFrame, HubOp, RunHub, RunTarget,
};
use crate::station::limits::AttachLimits;
use crate::station::publisher::gateway_agent;

/// What became of a delivery.
#[derive(Debug, PartialEq, Eq)]
enum Delivered {
    Taken,
    /// Universal Exports has no such client any more.
    Gone,
    /// It could not be delivered after every try.
    Lost,
}

/// The workers of the clients whose requests are being applied, by client.
type Workers = Arc<Mutex<HashMap<String, Sender<HubCommand>>>>;

#[derive(Clone)]
struct Link {
    origin: Url,
    token: String,
    station: String,
    limits: AttachLimits,
    /// The agent of the poll, whose calls may wait as long as `command_wait`.
    polling: ureq::Agent,
    /// The agent of every delivery.
    delivering: ureq::Agent,
}

impl Link {
    fn new(
        origin: Url,
        token: String,
        tls: &TlsConfig,
        station: String,
        limits: AttachLimits,
    ) -> Self {
        Self {
            polling: gateway_agent(tls, limits.call_margin.0 + limits.command_wait.0),
            delivering: gateway_agent(tls, limits.call_margin.0),
            origin,
            token,
            station,
            limits,
        }
    }
}

/// Starts the link when the Station publishes to Universal Exports. The thread ends within one
/// poll of the Station stopping.
pub(in super::super) fn spawn(state: &DaemonState) -> std::io::Result<()> {
    let Some(publisher) = &state.publisher else {
        return Ok(());
    };
    let (origin, token, tls) = publisher.link();
    let link = Link::new(
        origin,
        token,
        &tls,
        state.station_id.clone(),
        state.station.limits().attach.clone(),
    );
    let hub: Arc<dyn RunHub> = state.hub.clone();
    let stopping = state.stopping.clone();
    std::thread::Builder::new()
        .name("ai6-attach".to_owned())
        .spawn(move || link.take_commands(&hub, &stopping))
        .map(drop)
}

impl Link {
    fn pause(&self, stopping: &AtomicBool) {
        let until = Instant::now() + self.limits.retry_delay.0;
        while !stopping.load(Ordering::Acquire) && Instant::now() < until {
            std::thread::sleep(self.limits.pause_slice.0.min(self.limits.retry_delay.0));
        }
    }

    /// Polls for requests and hands them to their clients' workers, until the Station stops.
    fn take_commands(&self, hub: &Arc<dyn RunHub>, stopping: &Arc<AtomicBool>) {
        let Ok(mut endpoint) = self.origin.join("v1/station/hub/next") else {
            return;
        };
        endpoint
            .query_pairs_mut()
            .append_pair("station", &self.station)
            .append_pair(
                "wait_ms",
                &self.limits.command_wait.0.as_millis().to_string(),
            );
        let workers = Workers::default();
        while !stopping.load(Ordering::Acquire) {
            let response = self
                .polling
                .get(endpoint.as_str())
                .header("Authorization", &format!("Bearer {}", self.token))
                .call();
            match response {
                Ok(mut response) if response.status().as_u16() == 200 => {
                    match response.body_mut().read_json::<Vec<HubCommand>>() {
                        Ok(commands) => {
                            for command in commands {
                                self.hand_over(hub, stopping, &workers, command);
                            }
                        }
                        Err(error) => eprintln!(
                            "ai6: attach: a list of requests from Universal Exports was not \
                             understood and was dropped: {error}"
                        ),
                    }
                }
                Ok(response) if response.status().as_u16() == 204 => {}
                _ => self.pause(stopping),
            }
        }
    }

    /// Queues `command` for its client's worker, starting one for a client it has none.
    fn hand_over(
        &self,
        hub: &Arc<dyn RunHub>,
        stopping: &Arc<AtomicBool>,
        workers: &Workers,
        command: HubCommand,
    ) {
        let client = command.client.clone();
        let mut held = workers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(worker) = held.get(&client)
            && worker.send(command.clone()).is_ok()
        {
            return;
        }
        let (sender, commands) = channel::<HubCommand>();
        let _ = sender.send(command);
        held.insert(client.clone(), sender);
        let (link, hub, stopping, workers) =
            (self.clone(), hub.clone(), stopping.clone(), workers.clone());
        let spawned = std::thread::Builder::new()
            .name("ai6-attach-worker".to_owned())
            .spawn(move || {
                for command in commands {
                    let leaving = command.op == HubOp::Detach;
                    link.apply(&hub, &stopping, command);
                    if leaving {
                        break;
                    }
                }
                workers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&client);
            });
        if spawned.is_err() {
            eprintln!("ai6: attach: no thread to serve a client's requests");
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
        let batch = self.limits.delivery_frames;
        let Attachment { replay, live, .. } = attachment;
        let mut replay = replay.into_iter().map(|frame| frame.to(client)).peekable();
        while replay.peek().is_some() {
            let mut chunk: Vec<HubFrame> = Vec::new();
            while batch.allows(chunk.len() + 1) || chunk.is_empty() {
                let Some(frame) = replay.next() else { break };
                chunk.push(frame);
            }
            if self.deliver(target, chunk, stopping) != Delivered::Taken {
                hub.detach(target, client);
                return;
            }
        }
        while !stopping.load(Ordering::Acquire) {
            let first = match live.recv_timeout(self.limits.poll_slice.0) {
                Ok(frame) => frame,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    // Let go (too far behind) or the Run closed: the gateway is told, so the person
                    // is not left attached to nothing. A client detached on its own is unknown
                    // there already.
                    let ended = HubFrame::new(
                        "closed",
                        unix_now(),
                        json!({"reason": "the attachment ended"}),
                    )
                    .to(client);
                    let _ = self.deliver(target, vec![ended], stopping);
                    return;
                }
            };
            let mut chunk = vec![first.to(client)];
            let mut ended = false;
            while batch.allows(chunk.len() + 1) {
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
        let delivery = HubDelivery::new(self.station.clone(), target.clone(), frames);
        let mut tried = 0_usize;
        while self.limits.delivery_attempts.allows(tried + 1) {
            tried += 1;
            let sent = self
                .delivering
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
    use std::time::{Duration, Instant};

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
    struct Recording(
        Mutex<Vec<(String, String)>>,
        /// The prompt `slow` was taken.
        std::sync::atomic::AtomicBool,
    );

    impl RunPort for Recording {
        fn prompt(&self, person: &str, text: &str, accepted: &dyn Fn()) -> Result<(), HubError> {
            if text == "slow" {
                std::thread::sleep(Duration::from_secs(2));
                self.1.store(true, Ordering::Release);
            }
            self.0
                .lock()
                .unwrap()
                .push((person.to_owned(), text.to_owned()));
            accepted();
            Ok(())
        }

        fn interrupt(&self, _: &str) -> Result<bool, HubError> {
            Ok(true)
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

        let link = Link::new(
            origin,
            "station-token".to_owned(),
            &TlsConfig::default(),
            "hq".to_owned(),
            AttachLimits {
                command_wait: Seconds::new(1),
                retry_delay: Seconds::new(1),
                ..AttachLimits::default()
            },
        );
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_slow_to_take_a_prompt_holds_up_only_its_own_client() {
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
        hub.open(&target, port.clone());
        let link = Link::new(
            origin,
            "station-token".to_owned(),
            &TlsConfig::default(),
            "hq".to_owned(),
            AttachLimits {
                command_wait: Seconds::new(1),
                ..AttachLimits::default()
            },
        );
        let stopping = Arc::new(AtomicBool::new(false));
        let polling = std::thread::spawn({
            let (hub, stopping): (Arc<dyn RunHub>, _) = (hub.clone(), stopping.clone());
            move || link.take_commands(&hub, &stopping)
        });
        let ask = |client: &str, op: HubOp| {
            stub.lock()
                .unwrap()
                .commands
                .push_back(vec![HubCommand::new(target.clone(), client, "alice", op)]);
        };
        ask("c1", HubOp::Attach { after: None });
        until("the first client", || {
            delivered(&stub, "c1").iter().any(|f| f.kind == "ready")
        })
        .await;
        // The first client's prompt takes two seconds; a second client attaches meanwhile.
        ask(
            "c1",
            HubOp::Prompt {
                text: "slow".to_owned(),
            },
        );
        ask("c2", HubOp::Attach { after: None });
        until("the second client", || {
            delivered(&stub, "c2").iter().any(|f| f.kind == "ready")
        })
        .await;
        assert!(
            !port.1.load(Ordering::Acquire),
            "the second client was served while the first prompt was still being taken"
        );
        until("the slow prompt", || port.1.load(Ordering::Acquire)).await;

        stopping.store(true, Ordering::Release);
        tokio::task::spawn_blocking(move || polling.join().unwrap())
            .await
            .unwrap();
        served.abort();
    }
}
