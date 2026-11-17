//! `ai6ctl liaison loopback` against an in-process A2A peer on a proven link: an A2A HTTP client
//! reaches the peer's agent through the loopback endpoint the command serves.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use coqui_a2a::error::A2aError;
use coqui_a2a::model::{AgentCard, Message, Role, SendMessageRequest, SendMessageResponse};
use coqui_a2a::{Agent, Caller, PROTOCOL, Server};
use coqui_agentic_peer::auth::{self, Identity, Method, Proof, Sealed, Trust};
use coqui_agentic_peer::link::{self, Host, Options, Services};
use coqui_agentic_peer::mux::Side;
use coqui_agentic_peer::peers::{MemoryPeers, NoEvents, PeerRecord, PeerStore};
use coqui_transport_web::websocket::{Frames, Listener, ServerTls};
use coqui_transport_web::{Channel as _, Limits};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(60);

/// The agent behind the link: it answers `echo: <text>`.
struct Echo;

impl Agent for Echo {
    fn agent_card(&self, _caller: &Caller<'_>) -> AgentCard {
        AgentCard {
            name: "Echo".to_owned(),
            ..AgentCard::default()
        }
    }

    fn send_message(
        &self,
        _caller: &Caller<'_>,
        request: SendMessageRequest,
    ) -> Result<SendMessageResponse, A2aError> {
        Ok(SendMessageResponse::Message(Message::text(
            Role::Agent,
            "echo-reply",
            format!("echo: {}", request.message.joined_text()),
        )))
    }
}

/// Serves `Echo` on a loopback link to the one dialer `client` names, until `stop`.
fn serve_peer(
    identity: Identity,
    client: &str,
    stop: Arc<AtomicBool>,
) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
    let mut limits = Limits::default();
    limits.max_message = auth::MAX_PROOF_MESSAGE;
    let listener = Listener::builder("127.0.0.1:0".parse().unwrap(), ServerTls::Plain)
        .limits(limits)
        .frames(Frames::Binary)
        .build()
        .unwrap();
    let address = listener.local_addr().unwrap();
    let store = MemoryPeers::default();
    store
        .put(&PeerRecord::new(client).granting(PROTOCOL))
        .unwrap();
    let proof = Proof::builder(identity)
        .initiators(Trust::pinned([client.parse().unwrap()]))
        .build()
        .unwrap();
    let services = Server::new(Echo).serving(Services::default());
    let thread = thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            let Ok(pending) = listener.next(Some(Duration::from_millis(50))) else {
                continue;
            };
            let Ok(channel) = pending.handshake() else {
                continue;
            };
            let options = Options::default();
            if let Ok(established) = auth::prove(
                &proof,
                &channel,
                WAIT,
                proof.method().record_length(options.limits.max_message),
            ) {
                let peer = established.peer.to_string();
                let sealed = Sealed::new(&channel, established.session);
                let host = Host {
                    store: &store,
                    services: &services,
                    events: &NoEvents,
                };
                link::serve(&sealed, Side::Answered, options, None, &peer, &host);
                sealed.close();
            }
            channel.close();
        }
    });
    (address, thread)
}

/// Stops the command however the test ends.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(args: &[&str], url: &str) -> (Running, Vec<String>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ai6ctl"))
        .args(["liaison", "loopback", "--url", url])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let Some(printed) = lines.next() else {
        // The command ended before printing its endpoint: its own error says why.
        let mut said = String::new();
        let _ = std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut said);
        panic!("no endpoint was printed; the command said: {said}");
    };
    // Drained so a full pipe never stalls the command while it runs.
    let mut stderr = child.stderr.take().unwrap();
    thread::spawn(move || std::io::copy(&mut stderr, &mut std::io::sink()));
    (Running(child), vec![printed.unwrap()])
}

async fn until_answered(http: &reqwest::Client, base: &str, token: Option<&str>) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let mut request = http.post(base).json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "SendMessage",
            "params": {"message": {
                "messageId": "loop-1",
                "role": "ROLE_USER",
                "parts": [{"text": "over the loopback"}]
            }}
        }));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Ok(response) = request.send().await
            && let Ok(answer) = response.json::<Value>().await
            && answer["result"]["message"].is_object()
        {
            return answer;
        }
        assert!(Instant::now() < deadline, "the link came up");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_a2a_client_reaches_the_agent_through_the_loopback_command() {
    let keys = tempfile::tempdir().unwrap();
    let peer_key = Identity::open_or_create(&keys.path().join("peer"), Method::DEFAULT).unwrap();
    let own_path = keys.path().join("own");
    let own_key = Identity::open_or_create(&own_path, Method::DEFAULT).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (address, peer_thread) = serve_peer(
        peer_key.clone(),
        &own_key.peer_id().to_string(),
        stop.clone(),
    );
    let token_file = keys.path().join("token");
    std::fs::write(&token_file, "loopback-token-for-the-test\n").unwrap();
    let url = format!("ws://{address}/link");
    let (running, printed) = start(
        &[
            "--peer",
            &peer_key.peer_id().to_string(),
            "--identity",
            own_path.to_str().unwrap(),
            "--token-file",
            token_file.to_str().unwrap(),
        ],
        &url,
    );
    let base = printed[0].clone();
    assert!(base.starts_with("http://127.0.0.1:"), "{base}");
    let http = reqwest::Client::new();

    let answer = until_answered(&http, &base, Some("loopback-token-for-the-test")).await;
    assert_eq!(
        answer["result"]["message"]["parts"][0]["text"], "echo: over the loopback",
        "{answer}"
    );
    let card: Value = http
        .get(format!("{base}.well-known/agent-card.json"))
        .bearer_auth("loopback-token-for-the-test")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(card["name"], "Echo");
    let refused = http.post(&base).json(&json!({})).send().await.unwrap();
    assert_eq!(refused.status(), 401, "a request with no token is refused");

    drop(running);
    stop.store(true, Ordering::SeqCst);
    peer_thread.join().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_remote_that_is_not_the_peer_named_stops_the_command() {
    let keys = tempfile::tempdir().unwrap();
    let peer_key = Identity::open_or_create(&keys.path().join("peer"), Method::DEFAULT).unwrap();
    let stranger =
        Identity::open_or_create(&keys.path().join("stranger"), Method::DEFAULT).unwrap();
    let own_path = keys.path().join("own");
    let own_key = Identity::open_or_create(&own_path, Method::DEFAULT).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (address, peer_thread) = serve_peer(peer_key, &own_key.peer_id().to_string(), stop.clone());
    let (mut running, _) = start(
        &[
            "--peer",
            &stranger.peer_id().to_string(),
            "--identity",
            own_path.to_str().unwrap(),
            "--no-token",
        ],
        &format!("ws://{address}/link"),
    );
    let deadline = Instant::now() + WAIT;
    let status = loop {
        if let Some(status) = running.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the command stopped");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(!status.success());
    stop.store(true, Ordering::SeqCst);
    peer_thread.join().unwrap();
}
