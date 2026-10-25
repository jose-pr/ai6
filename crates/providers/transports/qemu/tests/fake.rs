//! The `qemu` transport against an in-process guest agent that runs `guest-exec` with the
//! host's own processes.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ai6::directorates::q::transport::contract::{self, Host};
use ai6::{RunSpec, Transport, TransportClient, TransportConfig};
use ai6_transport_qemu::Qemu;
use data_encoding::BASE64;
use serde_json::{Value, json};

#[derive(Default)]
struct Guest {
    connections: AtomicU64,
    /// Hang up on the next request that is not a sync.
    hang_up_once: AtomicBool,
    /// `guest-exec-status` answers "still running" this many times first.
    busy_polls: AtomicU64,
    osinfo: Mutex<Value>,
    finished: Mutex<HashMap<u64, Value>>,
    next_pid: AtomicU64,
    requests: Mutex<Vec<String>>,
}

struct FakeAgent {
    port: u16,
    guest: Arc<Guest>,
}

impl FakeAgent {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let guest = Arc::new(Guest {
            osinfo: Mutex::new(json!({"id": "ubuntu", "machine": "aarch64"})),
            next_pid: AtomicU64::new(100),
            ..Guest::default()
        });
        let shared = guest.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                shared.connections.fetch_add(1, Ordering::Relaxed);
                let guest = shared.clone();
                std::thread::spawn(move || serve(stream, &guest));
            }
        });
        Self { port, guest }
    }

    fn settings(&self) -> String {
        format!(
            "address = \"127.0.0.1:{}\"\nreply_timeout_secs = 10\n",
            self.port
        )
    }

    fn client(&self) -> TransportClient {
        TransportClient::new(Box::new(
            Qemu::try_from(TransportConfig {
                settings_toml: self.settings(),
            })
            .unwrap(),
        ))
    }
}

fn serve(stream: TcpStream, guest: &Guest) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = Vec::new();
        if reader.read_until(b'\n', &mut line).unwrap_or(0) == 0 {
            return;
        }
        let start = line
            .iter()
            .position(|&byte| byte != 0xFF)
            .unwrap_or(line.len());
        let text = String::from_utf8_lossy(&line[start..]);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let request: Value = serde_json::from_str(text).unwrap();
        let command = request["execute"].as_str().unwrap().to_owned();
        let arguments = request["arguments"].clone();
        let is_sync = command.starts_with("guest-sync");
        if !is_sync {
            guest.requests.lock().unwrap().push(command.clone());
            if guest.hang_up_once.swap(false, Ordering::AcqRel) {
                return;
            }
        }
        let reply = match command.as_str() {
            "guest-sync-delimited" => {
                writer.write_all(&[0xFF]).unwrap();
                json!({"return": arguments["id"]})
            }
            "guest-sync" => json!({"return": arguments["id"]}),
            "guest-ping" => json!({"return": {}}),
            "guest-get-osinfo" => json!({"return": *guest.osinfo.lock().unwrap()}),
            "guest-exec" => match exec(guest, &arguments) {
                Ok(pid) => json!({"return": {"pid": pid}}),
                Err(why) => json!({"error": {"class": "GenericError", "desc": why}}),
            },
            "guest-exec-status" => {
                let pid = arguments["pid"].as_u64().unwrap();
                let busy = guest.busy_polls.load(Ordering::Relaxed);
                if busy > 0 {
                    guest.busy_polls.store(busy - 1, Ordering::Relaxed);
                    json!({"return": {"exited": false}})
                } else {
                    let status = guest.finished.lock().unwrap().get(&pid).cloned();
                    json!({"return": status.expect("a known pid")})
                }
            }
            other => json!({"error": {"class": "CommandNotFound", "desc": format!("no {other}")}}),
        };
        writeln!(writer, "{reply}").unwrap();
    }
}

/// Runs the command to completion and files its status under a new pid.
fn exec(guest: &Guest, arguments: &Value) -> Result<u64, String> {
    let path = arguments["path"].as_str().unwrap();
    let args: Vec<&str> = arguments["arg"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    // A guest has /bin/sh; this host may keep its shell elsewhere.
    let mut command = Command::new(if path == "/bin/sh" { "sh" } else { path });
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for pair in arguments["env"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let (name, value) = pair.split_once('=').unwrap();
        command.env(name, value);
    }
    let input = arguments["input-data"]
        .as_str()
        .map(|data| BASE64.decode(data.as_bytes()).unwrap())
        .unwrap_or_default();
    let mut child = command
        .spawn()
        .map_err(|error| format!("Guest agent command failed, error was '{error}'"))?;
    let mut stdin = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let output = child.wait_with_output().unwrap();
    let pid = guest.next_pid.fetch_add(1, Ordering::Relaxed);
    guest.finished.lock().unwrap().insert(
        pid,
        json!({
            "exited": true,
            "exitcode": output.status.code(),
            "out-data": BASE64.encode(&output.stdout),
            "err-data": BASE64.encode(&output.stderr),
        }),
    );
    Ok(pid)
}

fn shell_available() -> bool {
    Command::new("sh")
        .args(["-c", "exit 0"])
        .status()
        .is_ok_and(|status| status.success())
}

macro_rules! need_shell {
    () => {
        if !shell_available() {
            eprintln!("SKIPPED: no `sh` on PATH for the in-process guest to run commands");
            return;
        }
    };
}

#[test]
fn the_qemu_transport_passes_the_contract_suite() {
    need_shell!();
    let agent = FakeAgent::start();
    let scratch = tempfile::tempdir().unwrap();
    contract::check(
        Box::new(
            Qemu::try_from(TransportConfig {
                settings_toml: agent.settings(),
            })
            .unwrap(),
        ),
        &Host {
            shell: &["sh", "-c"],
            expand: |name| format!("${name}"),
            scratch: &scratch.path().to_string_lossy(),
        },
    );
    assert_eq!(
        agent.guest.connections.load(Ordering::Relaxed),
        1,
        "one connection, synchronized once"
    );
}

#[test]
fn a_command_is_polled_until_it_exits_and_its_environment_reaches_it() {
    need_shell!();
    let agent = FakeAgent::start();
    agent.guest.busy_polls.store(3, Ordering::Relaxed);
    let output = agent
        .client()
        .run(&RunSpec {
            argv: ["sh", "-c", "echo \"$AI6_PROBE\"; exit 4"]
                .map(str::to_owned)
                .to_vec(),
            env: Some([("AI6_PROBE".to_owned(), "from the host".to_owned())].into()),
            ..RunSpec::default()
        })
        .unwrap();
    assert_eq!(output.code, Some(4));
    assert_eq!(String::from_utf8_lossy(&output.stdout.0), "from the host\n");
    assert_eq!(agent.guest.busy_polls.load(Ordering::Relaxed), 0);
}

#[test]
fn a_command_that_outlives_its_timeout_reports_it_without_waiting() {
    need_shell!();
    let agent = FakeAgent::start();
    agent.guest.busy_polls.store(u64::MAX, Ordering::Relaxed);
    let started = std::time::Instant::now();
    let output = agent
        .client()
        .run(&RunSpec {
            argv: vec!["true".to_owned()],
            timeout_ms: 300,
            ..RunSpec::default()
        })
        .unwrap();
    assert!(output.timed_out && output.code.is_none());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn a_dropped_connection_fails_one_call_and_the_next_starts_over() {
    let agent = FakeAgent::start();
    let client = agent.client();
    client.probe().unwrap();
    assert_eq!(agent.guest.connections.load(Ordering::Relaxed), 1);

    agent.guest.hang_up_once.store(true, Ordering::Release);
    assert!(client.probe().is_err(), "the call in flight fails");
    client.probe().unwrap();
    assert_eq!(agent.guest.connections.load(Ordering::Relaxed), 2);
}

#[test]
fn a_probe_reads_the_guests_platform_and_a_windows_guest_has_no_files() {
    let agent = FakeAgent::start();
    let client = agent.client();
    let probed = client.probe().unwrap();
    assert_eq!(probed.os.as_deref(), Some("linux"));
    assert_eq!(probed.arch.as_deref(), Some("aarch64"));
    assert!(probed.run && probed.files && !probed.terminal && !probed.forward);

    *agent.guest.osinfo.lock().unwrap() = json!({"id": "mswindows", "machine": "x86_64"});
    let windows = agent.client().probe().unwrap();
    assert_eq!(windows.os.as_deref(), Some("windows"));
    assert!(windows.run && !windows.files);
    assert_eq!(windows.notes.len(), 1);
}

#[test]
fn an_unreachable_agent_is_an_error_naming_the_address() {
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let qemu = Qemu::try_from(TransportConfig {
        settings_toml: format!("address = \"127.0.0.1:{port}\"\n"),
    })
    .unwrap();
    let error = qemu.probe().expect_err("nothing listens");
    assert!(ai6::describe(&error).contains(&port.to_string()));
}

#[test]
fn settings_are_validated_when_the_transport_is_built() {
    for bad in [
        "",
        "address = \"nope\"\n",
        "address = \"h:1\"\nunknown = 1\n",
    ] {
        assert!(
            Qemu::try_from(TransportConfig {
                settings_toml: bad.to_owned()
            })
            .is_err(),
            "{bad}"
        );
    }
}
