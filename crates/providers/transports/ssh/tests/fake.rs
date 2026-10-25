//! The `ssh` transport against an in-process SSH server: the contract suite, host-key
//! policy, reconnecting, terminals and forwards.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use ai6::directorates::q::transport::contract::{self, Host};
use ai6::guatiao::{FromValue, ToValue};
use ai6::{
    Capabilities, ExitInfo, ForwardSpec, READ_TIMED_OUT, RunSpec, Spec, TransportClient,
    TransportConfig,
};
use ai6_transport_ssh::Ssh;

mod support;
use support::{FakeSsh, shell_available};

fn transport(settings: String) -> Result<Ssh, ai6::guatiao::library::ProviderError> {
    Ssh::try_from(TransportConfig {
        settings_toml: settings,
    })
}

fn client(settings: String) -> TransportClient {
    TransportClient::new(Box::new(transport(settings).unwrap()))
}

/// Skips a test, with the reason on the record, when the fake server has no shell to run.
macro_rules! need_shell {
    () => {
        if !shell_available() {
            eprintln!("SKIPPED: no `sh` on PATH for the in-process SSH server to run commands");
            return;
        }
    };
}

/// Waits for something the server does on its own thread after the client has returned.
fn eventually(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "the server never did it");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn first_error(client: &TransportClient) -> String {
    let error = client
        .run(&RunSpec {
            argv: vec!["true".to_owned()],
            ..RunSpec::default()
        })
        .expect_err("the connection should be refused");
    ai6::describe(&error)
}

#[test]
fn the_ssh_transport_passes_the_contract_suite_against_the_fake_server() {
    need_shell!();
    let server = FakeSsh::start();
    let scratch = tempfile::tempdir().unwrap();
    contract::check(
        Box::new(transport(server.settings("")).unwrap()),
        &Host {
            shell: &["sh", "-c"],
            expand: |name| format!("${name}"),
            scratch: &scratch.path().to_string_lossy(),
        },
    );
    assert_eq!(
        server.seen.lock().unwrap().connections,
        1,
        "one connection is reused"
    );
}

#[test]
fn a_pinned_host_key_is_accepted_and_a_different_one_is_refused() {
    need_shell!();
    let server = FakeSsh::start();
    let ok = client(server.settings(""));
    assert_eq!(
        ok.run(&RunSpec {
            argv: vec!["true".to_owned()],
            ..RunSpec::default()
        })
        .unwrap()
        .code,
        Some(0)
    );

    let wrong = format!("sha256:{}", "00".repeat(32));
    let settings = server.unpinned_settings(&format!("host_key = \"{wrong}\"\n"));
    let message = first_error(&client(settings));
    assert!(message.contains("not the pinned"), "{message}");
    assert!(
        message.contains(&server.pin()) && message.contains(&wrong),
        "{message}"
    );
    assert_eq!(
        server.seen.lock().unwrap().commands.len(),
        1,
        "nothing ran on the host that presented the wrong key"
    );
}

#[test]
fn first_contact_without_a_pin_is_refused_and_names_the_key_to_pin() {
    let server = FakeSsh::start();
    let message = first_error(&client(server.unpinned_settings("")));
    assert!(message.contains("no host key is pinned"), "{message}");
    assert!(message.contains(&server.pin()), "{message}");
    assert!(message.contains("trust_on_first_use"), "{message}");
    assert!(server.seen.lock().unwrap().commands.is_empty());
}

#[test]
fn trust_on_first_use_accepts_the_first_key_and_shows_it_for_pinning() {
    need_shell!();
    let server = FakeSsh::start();
    let ssh = client(server.unpinned_settings("trust_on_first_use = true\n"));
    assert!(
        ssh.capabilities().unwrap().notes.is_empty(),
        "nothing trusted yet"
    );
    assert_eq!(
        ssh.run(&RunSpec {
            argv: vec!["true".to_owned()],
            ..RunSpec::default()
        })
        .unwrap()
        .code,
        Some(0)
    );
    let notes = ssh.capabilities().unwrap().notes;
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains(&server.pin()) && notes[0].contains("host_key"));
}

#[test]
fn a_key_trusted_on_first_use_must_be_the_one_presented_on_reconnect() {
    need_shell!();
    let first = FakeSsh::start();
    let port = first.port;
    // The second server must accept the same client key, which outlives the first one.
    let keep = tempfile::tempdir().unwrap();
    let key = keep.path().join("client_key");
    std::fs::copy(first.identity(), &key).unwrap();
    let settings = format!(
        "host = \"127.0.0.1\"\nport = {port}\nuser = \"tester\"\nidentity = {:?}\ntrust_on_first_use = true\n",
        key.to_string_lossy()
    );
    let ssh = client(settings);
    let argv = vec!["true".to_owned()];
    let spec = RunSpec {
        argv,
        ..RunSpec::default()
    };
    assert!(ssh.run(&spec).is_ok());
    drop(first);

    // A different server takes the address: its key is not the one first trusted.
    let Some(second) = FakeSsh::start_on(port, &key) else {
        eprintln!("SKIPPED: the port could not be reused for a second server");
        return;
    };
    let message = ai6::describe(&ssh.run(&spec).expect_err("the new key must be refused"));
    assert!(message.contains("trusted at first contact"), "{message}");
    assert!(second.seen.lock().unwrap().commands.is_empty());
}

#[test]
fn a_dropped_connection_is_remade_for_the_next_operation() {
    need_shell!();
    let server = FakeSsh::start();
    let ssh = client(server.settings(""));
    let spec = RunSpec {
        argv: vec!["true".to_owned()],
        ..RunSpec::default()
    };
    assert!(ssh.run(&spec).is_ok());
    assert_eq!(server.seen.lock().unwrap().connections, 1);

    server.disconnect_all();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(ssh.run(&spec).unwrap().code, Some(0));
    assert_eq!(server.seen.lock().unwrap().connections, 2);
}

#[test]
fn login_failures_say_what_failed() {
    let server = FakeSsh::start();
    // A key the server does not know.
    let other = tempfile::tempdir().unwrap();
    let key =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let path = other.path().join("stranger");
    key.write_openssh_file(&path, russh::keys::ssh_key::LineEnding::LF)
        .unwrap();
    let settings = format!(
        "host = \"127.0.0.1\"\nport = {}\nuser = \"tester\"\nidentity = {:?}\nhost_key = \"{}\"\n",
        server.port,
        path.to_string_lossy(),
        server.pin()
    );
    assert!(first_error(&client(settings)).contains("refused the key"));

    // A key file that does not exist.
    let settings = server.settings("").replace("client_key", "no_such_key");
    assert!(first_error(&client(settings)).contains("no_such_key"));

    // Nothing listening.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let settings = server
        .settings("")
        .replace(&server.port.to_string(), &port.to_string());
    assert!(first_error(&client(settings)).starts_with(&format!("127.0.0.1:{port}")));
}

#[test]
fn settings_are_validated_when_the_transport_is_built() {
    for bad in [
        "",
        "host = \"h\"\nunknown = 1\n",
        "host = \"h\"\nhost_key = \"sha256:short\"\n",
        "host = \"h\"\nport = 0\n",
    ] {
        assert!(transport(bad.to_owned()).is_err(), "{bad}");
    }
}

#[test]
fn a_probe_learns_the_hosts_platform_and_declares_files_and_forwards() {
    need_shell!();
    let server = FakeSsh::start();
    let ssh = client(server.settings(""));
    let declared = ssh.capabilities().unwrap();
    assert!(declared.terminal && declared.run && declared.files && declared.forward);
    assert!(declared.os.is_none());
    let probed = ssh.probe().unwrap();
    assert!(probed.os.is_some() && probed.arch.is_some(), "{probed:?}");
    assert!(probed.files);
    assert_eq!(ssh.capabilities().unwrap().os, probed.os);
}

#[test]
fn a_run_reports_a_timeout_and_a_signal_free_exit() {
    need_shell!();
    let server = FakeSsh::start();
    let ssh = client(server.settings(""));
    let started = Instant::now();
    let output = ssh
        .run(&RunSpec {
            argv: vec!["sleep".to_owned(), "30".to_owned()],
            timeout_ms: 300,
            ..RunSpec::default()
        })
        .unwrap();
    assert!(output.timed_out && output.code.is_none());
    assert!(started.elapsed() < Duration::from_secs(10));
    eventually(|| {
        server
            .seen
            .lock()
            .unwrap()
            .signals
            .iter()
            .any(|s| s == "KILL")
    });
}

#[test]
fn standard_input_reaches_the_remote_program() {
    need_shell!();
    let server = FakeSsh::start();
    let output = client(server.settings(""))
        .run(&RunSpec {
            argv: vec!["cat".to_owned()],
            stdin: Some(ai6::guatiao::Bytes(b"over the wire\n".to_vec())),
            ..RunSpec::default()
        })
        .unwrap();
    assert_eq!(output.stdout.0, b"over the wire\n");
    // The command line sent is one quoted word per argument.
    assert_eq!(server.seen.lock().unwrap().commands, ["exec 'cat'"]);
}

#[test]
fn a_terminal_runs_on_a_pty_and_can_be_written_resized_and_killed() {
    need_shell!();
    let server = FakeSsh::start();
    let ssh = transport(server.settings("")).unwrap();
    let spec = Spec {
        argv: vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "read line; echo got:$line".to_owned(),
        ],
        cwd: None,
        env: None,
        rows: 30,
        cols: 100,
    };
    use ai6::Transport;
    let session = ssh.open_terminal(&spec.to_value().unwrap()).unwrap();
    assert_eq!(server.seen.lock().unwrap().ptys, [(100, 30)]);

    let info = |timeout: i64| ExitInfo::from_value(&session.exit_status(timeout).unwrap()).unwrap();
    assert!(info(0).running);
    session.resize(40, 120).unwrap();
    session.write(b"hello\n").unwrap();
    let mut seen = Vec::new();
    let mut buffer = [0u8; 256];
    let deadline = Instant::now() + Duration::from_secs(10);
    while !String::from_utf8_lossy(&seen).contains("got:hello") && Instant::now() < deadline {
        let count = session.read(&mut buffer, 200).unwrap();
        if count > 0 {
            seen.extend_from_slice(&buffer[..count as usize]);
        } else if count == 0 {
            break;
        }
    }
    assert!(
        String::from_utf8_lossy(&seen).contains("got:hello"),
        "{seen:?}"
    );
    assert_eq!(server.seen.lock().unwrap().window_changes, [(120, 40)]);
    assert_eq!(info(5000).code, Some(0));
    // Once the program is gone and its output is read, there is nothing more to read.
    let mut tail = 1;
    for _ in 0..50 {
        tail = session.read(&mut buffer, 100).unwrap();
        if tail == 0 {
            break;
        }
    }
    assert_eq!(tail, 0);

    // A program that never exits is killed.
    let spec = Spec {
        argv: vec!["sleep".to_owned(), "30".to_owned()],
        ..spec
    };
    let sleeper = ssh.open_terminal(&spec.to_value().unwrap()).unwrap();
    assert_eq!(sleeper.read(&mut buffer, 50).unwrap(), READ_TIMED_OUT);
    sleeper.kill().unwrap();
    sleeper.kill().unwrap();
    let ended = ExitInfo::from_value(&sleeper.exit_status(5000).unwrap()).unwrap();
    assert!(!ended.running);
    eventually(|| {
        server
            .seen
            .lock()
            .unwrap()
            .signals
            .iter()
            .any(|s| s == "KILL")
    });
}

#[test]
fn a_reverse_forward_carries_a_connection_from_the_host_to_the_station() {
    let server = FakeSsh::start();
    let ssh = client(server.settings(""));

    // The Station side: a service the host reaches through the forward.
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = service.local_addr().unwrap().to_string();
    let echo = std::thread::spawn(move || {
        let (mut stream, _) = service.accept().unwrap();
        let mut buffer = [0u8; 64];
        let count = stream.read(&mut buffer).unwrap();
        stream.write_all(&buffer[..count]).unwrap();
    });

    let forward = ssh
        .forward(&ForwardSpec {
            remote_host: "127.0.0.1".to_owned(),
            remote_port: 0,
            target,
        })
        .unwrap();
    let port = forward.info().remote_port;
    assert_ne!(port, 0);

    let mut host_side = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    host_side
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    host_side.write_all(b"through the tunnel").unwrap();
    let mut reply = [0u8; 64];
    let count = host_side.read(&mut reply).unwrap();
    assert_eq!(&reply[..count], b"through the tunnel");
    echo.join().unwrap();

    forward.close().unwrap();
    forward.close().unwrap();
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "the host stops listening once the forward is closed"
    );
}

#[test]
fn a_reverse_forward_listens_again_on_its_port_when_the_connection_is_remade() {
    need_shell!();
    let server = FakeSsh::start();
    let ssh = client(server.settings(""));
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = service.local_addr().unwrap().to_string();
    let echo = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = service.accept().unwrap();
            let mut buffer = [0u8; 64];
            let count = stream.read(&mut buffer).unwrap();
            stream.write_all(&buffer[..count]).unwrap();
        }
    });
    let forward = ssh
        .forward(&ForwardSpec {
            remote_host: "127.0.0.1".to_owned(),
            remote_port: 0,
            target,
        })
        .unwrap();
    let port = forward.info().remote_port;
    let through = |text: &[u8]| {
        let mut host_side = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        host_side
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        host_side.write_all(text).unwrap();
        let mut reply = [0u8; 64];
        let count = host_side.read(&mut reply).unwrap();
        reply[..count].to_vec()
    };
    assert_eq!(through(b"before"), b"before");

    // The link drops; the host stops listening with it.
    server.disconnect_all();
    eventually(|| std::net::TcpStream::connect(("127.0.0.1", port)).is_err());

    // The next operation reconnects, and the forward is back on the same port.
    let spec = RunSpec {
        argv: vec!["true".to_owned()],
        ..RunSpec::default()
    };
    assert_eq!(ssh.run(&spec).unwrap().code, Some(0));
    assert_eq!(server.seen.lock().unwrap().connections, 2);
    assert_eq!(through(b"after"), b"after");
    echo.join().unwrap();

    forward.close().unwrap();
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "closing the forward stops the host listening on the new connection"
    );
}

#[test]
fn capabilities_roundtrip_as_published() {
    let server = FakeSsh::start();
    let capabilities = client(server.settings("")).capabilities().unwrap();
    let value = capabilities.to_value().unwrap();
    assert_eq!(Capabilities::from_value(&value).unwrap(), capabilities);
}
