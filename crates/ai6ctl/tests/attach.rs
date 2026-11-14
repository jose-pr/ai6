//! `ai6ctl charge attach` and `ai6ctl mission attach` against the in-process gateway, with a
//! stand-in Station that polls for what the people attached type and delivers what the Run shows.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::directorates::section_00::hub::{HubCommand, HubDelivery, HubFrame, HubOp, RunTarget};
use ai6::{
    Attempt, Authenticator, FileRegistry, Lookup, Person, Verdict, access_from_toml,
    guatiao::library::ProviderError,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use universal_exports::{AccessProviders, Gateway, GatewayConfig, GatewayLimits};

const STATION_TOKEN: &str = "m-token-that-is-long-enough-for-testing";
const ORIGIN: &str = "https://exports.example";
const WAIT: Duration = Duration::from_secs(30);

/// `operator-007` may talk to Operation G's charge; `viewer-007` may only read it.
const CONVERSE: &str = r#"
    [role.converser]
    actions = ["charge.converse"]

    [role.viewer]
    actions = ["mission.read"]

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "converser"

    [[rule]]
    subject = "person:viewer-007"
    scope = "operation:G"
    role = "viewer"
"#;

/// `operator-007` may also interrupt: that is control of G's Missions, not conversation.
const BOTH: &str = r#"
    [role.both]
    actions = ["charge.converse", "mission.control"]

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "both"
"#;

/// `operator-007` may control G's Missions.
const CONTROL: &str = r#"
    [role.controller]
    actions = ["mission.control"]

    [[rule]]
    subject = "person:operator-007"
    scope = "operation:G"
    role = "controller"
"#;

struct People;

impl Authenticator for People {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        Ok(match (attempt.name.as_deref(), attempt.secret.as_deref()) {
            (Some(name @ ("operator" | "viewer")), Some("secret")) => Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: format!("{name}-007"),
            }),
            _ => Verdict::Invalid,
        })
    }
}

impl ai6::Directory for People {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        Ok(
            matches!(lookup.value.as_str(), "operator-007" | "viewer-007").then(|| Person {
                id: lookup.value.clone(),
                name: lookup.value,
                display_name: None,
                email: None,
                groups: Vec::new(),
                attributes: BTreeMap::new(),
            }),
        )
    }
}

struct Served {
    base: String,
    operator: String,
    viewer: String,
    server: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// A gateway under `roles` whose Operation G's charge is kept by Station `worker`, which also
/// runs Mission `mission-g`.
async fn served(roles: &str) -> Served {
    let directory = tempfile::tempdir().unwrap();
    let registry = FileRegistry::open(directory.path());
    registry
        .check_in(
            "worker",
            BTreeMap::from([("operations".to_owned(), json!(["G"]))]),
        )
        .unwrap();
    registry
        .claim_charge("operation:G", "agent", "worker", Duration::from_secs(600))
        .unwrap();
    registry
        .open_run(
            ai6::NewRun::new("record-g", "mission-g", "worker")
                .with_operation("G")
                .with_provider_id("run-g"),
            Duration::from_secs(60),
        )
        .unwrap();
    let authorizer = access_from_toml(roles, ai6::Limit::UNBOUNDED).unwrap();
    let gateway = Gateway::with_access(
        GatewayConfig::new(
            ORIGIN,
            STATION_TOKEN,
            GatewayLimits::default(),
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap(),
        Arc::new(registry),
        AccessProviders::new(Arc::new(People), Arc::new(People), Arc::new(authorizer)),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            gateway
                .router()
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    let operator = login(&base, "operator").await;
    let viewer = login(&base, "viewer").await;
    Served {
        base,
        operator,
        viewer,
        server,
        _directory: directory,
    }
}

async fn login(base: &str, name: &str) -> String {
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/auth/login"))
        .header("origin", ORIGIN)
        .header("x-forwarded-proto", "https")
        .json(&json!({"name": name, "secret": "secret"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    response.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// What the Station is shown of the people attached: its poll, as the Station makes it.
async fn poll(served: &Served, wait_ms: u64) -> Vec<HubCommand> {
    let response = reqwest::Client::new()
        .get(format!(
            "{}/v1/station/hub/next?station=worker&wait_ms={wait_ms}",
            served.base
        ))
        .bearer_auth(STATION_TOKEN)
        .send()
        .await
        .unwrap();
    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Vec::new();
    }
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response.json().await.unwrap()
}

/// At least `count` commands, polled until they arrive.
async fn commands(served: &Served, count: usize) -> Vec<HubCommand> {
    let mut all = Vec::new();
    let started = std::time::Instant::now();
    while all.len() < count {
        assert!(started.elapsed() < WAIT, "only {all:?}");
        all.extend(poll(served, 1_000).await);
    }
    all
}

async fn deliver(served: &Served, target: &RunTarget, frames: Vec<HubFrame>) {
    let response = reqwest::Client::new()
        .post(format!("{}/v1/station/hub/frames", served.base))
        .bearer_auth(STATION_TOKEN)
        .json(&HubDelivery::new("worker", target.clone(), frames))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
}

fn charge() -> RunTarget {
    RunTarget::Charge {
        scope: "operation:G".to_owned(),
    }
}

fn frame(seq: u64, kind: &str, data: Value) -> HubFrame {
    let mut frame = HubFrame::new(kind, 1.0, data);
    frame.seq = seq;
    frame
}

/// A running `ai6ctl` attached to a Run, its standard streams read and written by the test.
struct Attached {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: UnboundedReceiver<String>,
    seen: Vec<String>,
    errors: Arc<Mutex<String>>,
}

impl Attached {
    fn start(server: &str, token: &str, args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ai6ctl"))
            .arg("--server")
            .arg(server)
            .args(args)
            .env("UNEX_ACCESS_TOKEN", token)
            .env_remove("UNEX_CA_FILE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (sender, lines) = unbounded_channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let errors = Arc::new(Mutex::new(String::new()));
        let stderr = child.stderr.take().unwrap();
        let sink = errors.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut sink = sink.lock().unwrap();
                sink.push_str(&line);
                sink.push('\n');
            }
        });
        let stdin = child.stdin.take();
        Self {
            child,
            stdin,
            lines,
            seen: Vec::new(),
            errors,
        }
    }

    /// The next line of output that contains `needle`.
    async fn line(&mut self, needle: &str) -> String {
        loop {
            let line = tokio::time::timeout(WAIT, self.lines.recv())
                .await
                .unwrap_or_else(|_| panic!("no {needle:?} in {:?}", self.seen))
                .unwrap_or_else(|| panic!("output ended without {needle:?}: {:?}", self.seen));
            self.seen.push(line.clone());
            if line.contains(needle) {
                return line;
            }
        }
    }

    /// The client's ID, from the `attached as` line.
    async fn client(&mut self) -> String {
        self.line("attached as ")
            .await
            .strip_prefix("attached as ")
            .unwrap()
            .to_owned()
    }

    fn say(&mut self, text: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{text}").unwrap();
        stdin.flush().unwrap();
    }

    fn end_of_input(&mut self) {
        self.stdin = None;
    }

    /// Waits for the process to exit: whether it succeeded, all its output, and its errors.
    async fn finish(mut self) -> (bool, String, String) {
        let mut child = self.child;
        let status = tokio::task::spawn_blocking(move || child.wait().unwrap());
        let status = tokio::time::timeout(WAIT, status)
            .await
            .expect("ai6ctl exits")
            .unwrap();
        while let Ok(line) = self.lines.try_recv() {
            self.seen.push(line);
        }
        // The reader threads end with the pipes; let them drain.
        tokio::time::sleep(Duration::from_millis(200)).await;
        while let Ok(line) = self.lines.try_recv() {
            self.seen.push(line);
        }
        let errors = self.errors.lock().unwrap().clone();
        (status.success(), self.seen.join("\n"), errors)
    }
}

#[tokio::test]
async fn frames_are_printed_and_typed_lines_reach_the_station_as_the_person() {
    let served = served(BOTH).await;
    let mut attached = Attached::start(
        &served.base,
        &served.operator,
        &["charge", "attach", "operation:G", "--after", "2"],
    );
    let client = attached.client().await;
    let command = commands(&served, 1).await.remove(0);
    assert_eq!(command.target, charge());
    assert_eq!(command.person, "operator-007");
    assert_eq!(command.client, client);
    assert_eq!(command.op, HubOp::Attach { after: Some(2) });

    // The replay, addressed to the client, then live frames for everybody.
    deliver(
        &served,
        &charge(),
        vec![
            frame(3, "prompt", json!({"from": "station", "text": "wake"})).to(&client),
            frame(4, "event", json!({"kind": "text", "text": "noted"})).to(&client),
            frame(5, "ready", json!({"busy": false, "attached": 1, "last": 5})).to(&client),
        ],
    )
    .await;
    deliver(
        &served,
        &charge(),
        vec![
            frame(6, "turn", json!({"state": "begun", "by": "station"})),
            frame(7, "event", json!({"kind": "tool", "text": "Run a command"})),
            frame(8, "event", json!({"kind": "plan", "text": "1. look"})),
            frame(
                9,
                "permission",
                json!({"request": 4, "tool": "Run a command", "detail": "ls",
                       "options": [{"id": "allow", "label": "Allow"}, {"id": "refuse", "label": "Refuse"}]}),
            ),
        ],
    )
    .await;
    assert_eq!(attached.line("[3]").await, "[3] prompt from station: wake");
    attached.line("[4] text: noted").await;
    attached
        .line("[5] ready: busy false, attached 1, last 5")
        .await;
    attached.line("[6] turn begun by station").await;
    attached.line("[7] tool: Run a command").await;
    attached.line("[8] plan: 1. look").await;
    attached
        .line("[9] permission 4: Run a command (ls) options: allow (Allow), refuse (Refuse)")
        .await;

    // What is typed reaches the Station as the person's command, in order.
    attached.say("what is the state of G-1?");
    attached.say("/allow 4");
    attached.say("/answer 5 once");
    attached.say("/refuse 6");
    attached.say("/interrupt");
    attached.say("//not a command");
    let heard = commands(&served, 6).await;
    let heard: Vec<HubOp> = heard
        .into_iter()
        .inspect(|command| {
            assert_eq!(command.person, "operator-007");
            assert_eq!(command.client, client);
        })
        .map(|command| command.op)
        .collect();
    assert_eq!(
        heard,
        [
            HubOp::Prompt {
                text: "what is the state of G-1?".to_owned()
            },
            HubOp::Answer {
                request: 4,
                option: "allow".to_owned()
            },
            HubOp::Answer {
                request: 5,
                option: "once".to_owned()
            },
            HubOp::Answer {
                request: 6,
                option: "refuse".to_owned()
            },
            HubOp::Interrupt,
            HubOp::Prompt {
                text: "/not a command".to_owned()
            },
        ]
    );

    // A typo is told on standard error and sends nothing; the attachment stays.
    attached.say("/allow");
    attached.say("/detach");
    let (ok, out, err) = attached.finish().await;
    assert!(ok, "{err}");
    assert!(err.contains("/answer N OPTION"), "{err}");
    for text in [&out, &err] {
        assert!(!text.contains(&served.operator), "the token is never shown");
    }
    // Leaving tells the Station; it is not an interrupt or a cancel.
    let left = commands(&served, 1).await;
    assert_eq!(left[0].op, HubOp::Detach);
}

#[tokio::test]
async fn the_end_of_input_detaches_and_a_closed_run_ends_the_attachment() {
    let served = served(CONVERSE).await;
    let mut attached = Attached::start(
        &served.base,
        &served.operator,
        &["charge", "attach", "operation:G"],
    );
    attached.client().await;
    assert_eq!(
        commands(&served, 1).await[0].op,
        HubOp::Attach { after: None }
    );
    // Interrupting is control, which conversing is not: the refusal is told on standard error
    // and the person stays attached.
    attached.say("/interrupt");
    let started = std::time::Instant::now();
    while !attached.errors.lock().unwrap().contains("forbidden") {
        assert!(started.elapsed() < WAIT, "no refusal");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(poll(&served, 0).await.is_empty());
    attached.end_of_input();
    let (ok, _, err) = attached.finish().await;
    assert!(ok, "{err}");
    assert_eq!(commands(&served, 1).await[0].op, HubOp::Detach);

    // The Run ending ends the attachment, successfully.
    let mut attached = Attached::start(
        &served.base,
        &served.operator,
        &["charge", "attach", "operation:G"],
    );
    attached.client().await;
    commands(&served, 1).await;
    deliver(
        &served,
        &charge(),
        vec![frame(1, "closed", json!({"reason": "the Run ended"}))],
    )
    .await;
    attached.line("[1] closed: the Run ended").await;
    let (ok, _, err) = attached.finish().await;
    assert!(ok, "{err}");
}

#[tokio::test]
async fn a_person_without_the_action_is_refused_without_a_secret_in_the_output() {
    let served = served(CONVERSE).await;
    for (token, scope) in [
        (&served.viewer, "operation:G"),
        (&served.operator, "operation:L"),
        (&served.operator, "m"),
    ] {
        let attached = Attached::start(&served.base, token, &["charge", "attach", scope]);
        let (ok, out, err) = attached.finish().await;
        assert!(!ok, "{scope}");
        assert!(err.contains("forbidden"), "{err}");
        assert!(out.is_empty(), "{out}");
        for text in [&out, &err] {
            assert!(!text.contains(token.as_str()), "the token is never shown");
        }
    }
    // A token that is nobody's is refused too, and nothing reached the Station.
    let attached = Attached::start(
        &served.base,
        "not-a-token",
        &["charge", "attach", "operation:G"],
    );
    let (ok, _, err) = attached.finish().await;
    assert!(!ok && err.contains("unauthorized"), "{err}");
    assert!(poll(&served, 0).await.is_empty());
}

#[tokio::test]
async fn a_missions_run_is_attached_under_mission_control() {
    // Conversing with the charge is not controlling the Mission.
    let converse = served(CONVERSE).await;
    let attached = Attached::start(
        &converse.base,
        &converse.operator,
        &["mission", "attach", "mission-g", "--operation", "G"],
    );
    let (ok, _, err) = attached.finish().await;
    assert!(!ok && err.contains("forbidden"), "{err}");

    let served = served(CONTROL).await;
    let target = RunTarget::Mission {
        operation: "G".to_owned(),
        mission: "mission-g".to_owned(),
    };
    let mut attached = Attached::start(
        &served.base,
        &served.operator,
        &["mission", "attach", "mission-g", "--operation", "G"],
    );
    let client = attached.client().await;
    let command = commands(&served, 1).await.remove(0);
    assert_eq!(command.target, target);
    assert_eq!(command.person, "operator-007");
    assert_eq!(command.op, HubOp::Attach { after: None });
    deliver(
        &served,
        &target,
        vec![frame(1, "event", json!({"kind": "text", "text": "working"})).to(&client)],
    )
    .await;
    attached.line("[1] text: working").await;
    attached.say("carry on");
    attached.say("/interrupt");
    let heard = commands(&served, 2).await;
    assert_eq!(
        heard
            .iter()
            .map(|command| command.op.clone())
            .collect::<Vec<_>>(),
        [
            HubOp::Prompt {
                text: "carry on".to_owned()
            },
            HubOp::Interrupt
        ]
    );
    attached.say("/detach");
    let (ok, _, err) = attached.finish().await;
    assert!(ok, "{err}");

    // A Mission no Station runs a Run of is not attachable.
    let attached = Attached::start(
        &served.base,
        &served.operator,
        &["mission", "attach", "mission-none", "--operation", "G"],
    );
    let (ok, _, err) = attached.finish().await;
    assert!(!ok && err.contains("no_run"), "{err}");
}

/// A TLS WebSocket server standing in for the gateway, with a certificate for `127.0.0.1` from a
/// new authority: its origin, the authority's PEM and what it was sent (the hello, then the
/// next frame).
async fn tls_gateway() -> (String, String, tokio::sync::oneshot::Receiver<Vec<Value>>) {
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_pem = ca.self_signed(&ca_key).unwrap().pem();
    let issuer = rcgen::Issuer::new(ca, ca_key);
    let key = rcgen::KeyPair::generate().unwrap();
    let leaf = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()])
        .unwrap()
        .signed_by(&key, &issuer)
        .unwrap();
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("https://{}", listener.local_addr().unwrap());
    let (done, received) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        // A client that does not trust the authority fails the handshake; the next one is served.
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let Ok(tls) = acceptor.accept(tcp).await else {
                continue;
            };
            let mut socket = tokio_tungstenite::accept_async(tls).await.unwrap();
            let mut heard = Vec::new();
            let hello = socket.next().await.unwrap().unwrap();
            heard.push(serde_json::from_str::<Value>(hello.to_text().unwrap()).unwrap());
            for reply in [
                json!({"type": "attached", "client": "c-1", "target": {}}),
                json!({"type": "frame", "frame": {"seq": 1, "at": 1.0, "kind": "event",
                       "data": {"kind": "text", "text": "over TLS"}}}),
            ] {
                socket.send(reply.to_string().into()).await.unwrap();
            }
            let prompt = socket.next().await.unwrap().unwrap();
            heard.push(serde_json::from_str::<Value>(prompt.to_text().unwrap()).unwrap());
            let _ = done.send(heard);
            let _ = socket.close(None).await;
            return;
        }
    });
    (origin, ca_pem, received)
}

#[tokio::test]
async fn a_wss_gateway_is_trusted_through_the_ca_file_and_the_token_is_only_in_the_hello() {
    let (origin, ca, received) = tls_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("ca.pem");
    std::fs::write(&file, &ca).unwrap();

    // Without the authority the connection is refused, and says so.
    let refused = Attached::start(
        &origin,
        "secret-token",
        &["--timeout", "5", "charge", "attach", "m"],
    );
    let (ok, out, err) = refused.finish().await;
    assert!(!ok && err.contains("TLS"), "{err}");
    assert!(!out.contains("secret-token") && !err.contains("secret-token"));

    let mut attached = Attached::start(
        &origin,
        "secret-token",
        &[
            "--ca-file",
            file.to_str().unwrap(),
            "charge",
            "attach",
            "operation:G",
        ],
    );
    assert_eq!(attached.client().await, "c-1");
    attached.line("[1] text: over TLS").await;
    attached.say("hello");
    let heard = tokio::time::timeout(WAIT, received).await.unwrap().unwrap();
    assert_eq!(
        heard,
        [
            json!({"type": "hello", "token": "secret-token", "charge": "operation:G"}),
            json!({"type": "prompt", "text": "hello"}),
        ]
    );
    // The gateway closing the socket without a closed frame is an error, not a detach.
    let (ok, out, err) = attached.finish().await;
    assert!(!ok && err.contains("connection ended"), "{err}");
    assert!(!out.contains("secret-token") && !err.contains("secret-token"));
}
