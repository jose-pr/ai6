//! `ai6 theater dial` against an in-process agent-link listener: the relay carries a scripted
//! ACP agent over a proven link, dials again after the link is cut, and stops on a wrong key.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};
use std::thread;
use std::time::Duration;

use coqui_agentic_peer::auth::{self, Identity, Method, Proof, Sealed, Trust};
use coqui_agentic_peer::mux::{Limits as MuxLimits, Mux, Side};
use coqui_transport::Channel as _;
use coqui_transport_web::Limits;
use coqui_transport_web::websocket::{Frames, Listener, ServerTls};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(60);

/// What the listener saw.
enum Seen {
    /// A stream opened: its protocol and metadata.
    Opened(String, Vec<u8>),
    /// The text the agent's reply to a prompt carried, on the link of this index.
    Answered(usize, String),
}

/// The scripted agent: built once with Cargo, which finds the target directory this test runs
/// from, then looked for beside the test binary.
fn fake_agent() -> PathBuf {
    static BUILT: std::sync::Once = std::sync::Once::new();
    const BUILD: &str = "cargo build -p ai6-agent-acp --bin ai6-acp-fake-agent";
    BUILT.call_once(|| {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args([
                "build",
                "-p",
                "ai6-agent-acp",
                "--bin",
                "ai6-acp-fake-agent",
            ])
            .status()
            .unwrap_or_else(|error| panic!("`{BUILD}` could not start: {error}"));
        assert!(status.success(), "`{BUILD}` failed");
    });
    let name = format!("ai6-acp-fake-agent{}", std::env::consts::EXE_SUFFIX);
    let exe = std::env::current_exe().unwrap();
    exe.ancestors()
        .skip(1)
        .take(3)
        .map(|dir| dir.join(&name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("{name} is not beside the test binary; build it with `{BUILD}`"))
}

/// Sends one request over the stream and returns the agent's answer and the text of any
/// `agent_message_chunk` that came before it.
fn ask(
    stream: &dyn coqui_transport::Channel,
    id: u64,
    method: &str,
    params: Value,
) -> (Value, String) {
    let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    stream.send(request.to_string().as_bytes()).unwrap();
    let mut text = String::new();
    loop {
        let line = stream
            .receive(Some(WAIT))
            .unwrap()
            .expect("the stream stayed open");
        let message: Value = serde_json::from_slice(&line).unwrap();
        if message["id"] == id {
            return (message, text);
        }
        if let Some(chunk) = message["params"]["update"]["content"]["text"].as_str() {
            text.push_str(chunk);
        }
    }
}

/// Serves the agent link for the one dialer `client` names. The first link runs the whole
/// exchange and is then cut; the second runs one more prompt, and `done` ends the listener.
fn serve(
    identity: Identity,
    client: &str,
    seen: Sender<Seen>,
    done: Arc<AtomicBool>,
) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
    let mut limits = Limits::default();
    limits.max_message = auth::MAX_PROOF_MESSAGE;
    let listener = Listener::builder("127.0.0.1:0".parse().unwrap(), ServerTls::Plain)
        .limits(limits)
        .frames(Frames::Binary)
        .build()
        .unwrap();
    let address = listener.local_addr().unwrap();
    let proof = Proof::builder(identity)
        .initiators(Trust::pinned([client.parse().unwrap()]))
        .build()
        .unwrap();
    let thread = thread::spawn(move || {
        let mut index = 0;
        while !done.load(Ordering::SeqCst) {
            let Ok(pending) = listener.next(Some(Duration::from_millis(50))) else {
                continue;
            };
            let Ok(channel) = pending.handshake() else {
                continue;
            };
            let limits = MuxLimits::default();
            if let Ok(established) = auth::prove(
                &proof,
                &channel,
                WAIT,
                proof.method().record_length(limits.max_message),
            ) {
                let sealed = Sealed::new(&channel, established.session);
                Mux::run(&sealed, Side::Answered, limits, None, |mux| {
                    let Ok(Some(incoming)) = mux.accept(Some(WAIT)) else {
                        return;
                    };
                    seen.send(Seen::Opened(
                        incoming.protocol.clone(),
                        incoming.metadata.clone(),
                    ))
                    .unwrap();
                    let stream = &incoming.stream;
                    if index == 0 {
                        let (initialized, _) =
                            ask(stream, 1, "initialize", json!({"protocolVersion": 1}));
                        assert_eq!(initialized["result"]["protocolVersion"], 1, "{initialized}");
                        let (session, _) = ask(
                            stream,
                            2,
                            "session/new",
                            json!({"cwd": "/work", "mcpServers": []}),
                        );
                        assert_eq!(session["result"]["sessionId"], "fake-1", "{session}");
                    }
                    let (answer, text) = ask(
                        stream,
                        10 + index as u64,
                        "session/prompt",
                        json!({"sessionId": "fake-1", "prompt": [{"type": "text", "text": format!("hello {index}")}]}),
                    );
                    assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
                    seen.send(Seen::Answered(index, text)).unwrap();
                });
                // The first link is cut here, from the listener's side.
                sealed.close();
                index += 1;
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

fn start(url: &str, extra: &[&str]) -> Running {
    let agent = fake_agent();
    let child = Command::new(env!("CARGO_BIN_EXE_ai6"))
        .args(["theater", "dial", "--url", url])
        .args(["--backoff-start-seconds", "1"])
        .args(extra)
        .arg("--")
        .arg(agent)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    Running(child)
}

fn next(seen: &Receiver<Seen>) -> Seen {
    seen.recv_timeout(WAIT).expect("the listener saw it")
}

/// A key file only its owner may read, as the command makes its own.
#[cfg(unix)]
fn assert_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{} is mode {mode:o}", path.display());
}

#[cfg(not(unix))]
fn assert_private(_path: &std::path::Path) {}

#[test]
fn the_relay_carries_an_agent_and_dials_again_after_the_link_is_cut() {
    let keys = tempfile::tempdir().unwrap();
    let gateway = Identity::open_or_create(&keys.path().join("gateway"), Method::DEFAULT).unwrap();
    let own_path = keys.path().join("own");
    let own = Identity::open_or_create(&own_path, Method::DEFAULT).unwrap();
    assert_private(&own_path);
    assert_private(&keys.path().join("gateway"));
    let (sender, seen) = mpsc::channel();
    let done = Arc::new(AtomicBool::new(false));
    let (address, listener) = serve(
        gateway.clone(),
        &own.peer_id().to_string(),
        sender,
        done.clone(),
    );
    let url = format!("ws://{address}/v1/assistant/agent-link");
    let running = start(
        &url,
        &[
            "--peer",
            &gateway.peer_id().to_string(),
            "--identity",
            own_path.to_str().unwrap(),
            "--code",
            "first-time-code",
            "--label",
            "the laptop",
        ],
    );

    // The first link carries the code, and the agent answers through the relay.
    let Seen::Opened(protocol, metadata) = next(&seen) else {
        panic!("the stream opened first");
    };
    assert_eq!(protocol, "acp/1");
    let metadata: Value = serde_json::from_slice(&metadata).unwrap();
    assert_eq!(
        metadata,
        json!({"code": "first-time-code", "label": "the laptop"})
    );
    let Seen::Answered(0, text) = next(&seen) else {
        panic!("the first prompt was answered");
    };
    assert!(text.contains("prompt:hello 0"), "{text}");

    // The listener cut that link; the relay dials again with the key alone and the same agent
    // answers.
    let Seen::Opened(protocol, metadata) = next(&seen) else {
        panic!("the relay opened a stream again");
    };
    assert_eq!(protocol, "acp/1");
    assert!(
        metadata.is_empty(),
        "the code is carried once: {metadata:?}"
    );
    let Seen::Answered(1, text) = next(&seen) else {
        panic!("the prompt after the reconnect was answered");
    };
    assert!(text.contains("prompt:hello 1"), "{text}");

    drop(running);
    done.store(true, Ordering::SeqCst);
    listener.join().unwrap();
}

#[test]
fn a_remote_that_proves_another_key_ends_the_command_with_status_2() {
    let keys = tempfile::tempdir().unwrap();
    let gateway = Identity::open_or_create(&keys.path().join("gateway"), Method::DEFAULT).unwrap();
    let stranger =
        Identity::open_or_create(&keys.path().join("stranger"), Method::DEFAULT).unwrap();
    let own_path = keys.path().join("own");
    let own = Identity::open_or_create(&own_path, Method::DEFAULT).unwrap();
    let (sender, _seen) = mpsc::channel();
    let done = Arc::new(AtomicBool::new(false));
    let (address, listener) = serve(gateway, &own.peer_id().to_string(), sender, done.clone());
    let mut running = start(
        &format!("ws://{address}/v1/assistant/agent-link"),
        &[
            "--peer",
            &stranger.peer_id().to_string(),
            "--identity",
            own_path.to_str().unwrap(),
        ],
    );
    let status = {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            if let Some(status) = running.0.try_wait().unwrap() {
                break status;
            }
            assert!(std::time::Instant::now() < deadline, "the command stopped");
            thread::sleep(Duration::from_millis(100));
        }
    };
    assert_eq!(status.code(), Some(2));
    done.store(true, Ordering::SeqCst);
    listener.join().unwrap();
}
