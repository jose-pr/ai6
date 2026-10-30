//! The `adb` transport against a stand-in for the adb command line that runs `adb shell`
//! scripts with the host's own `sh`.

use std::process::Command;
use std::sync::{Arc, Mutex};

use ai6::directorates::q::transport::contract::{self, Host};
use ai6::directorates::q::transport::kit::{ProcessInput, run_process};
use ai6::guatiao::library::ProviderError;
use ai6::{Capabilities, ForwardSpec, Output, RunSpec, TransportClient};
use ai6_transport_adb::{Adb, Runner};

/// What the stand-in device was asked, besides running shell scripts.
#[derive(Default)]
struct Seen {
    reverses: Vec<String>,
    removed: Vec<String>,
}

struct FakeDevice {
    seen: Arc<Mutex<Seen>>,
    state: &'static str,
}

impl Runner for FakeDevice {
    fn adb(&self, args: &[String], input: &ProcessInput<'_>) -> Result<Output, ProviderError> {
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match words.as_slice() {
            ["get-state"] => Ok(text(self.state)),
            ["shell", "getprop ro.product.cpu.abi"] => Ok(text("arm64-v8a\n")),
            ["shell", script] => run_process(
                Command::new("sh").arg("-c").arg(script),
                input,
                std::time::Duration::from_secs(1),
            ),
            ["reverse", "--remove", port] => {
                self.seen.lock().unwrap().removed.push((*port).to_owned());
                Ok(text(""))
            }
            ["reverse", remote, local] => {
                self.seen
                    .lock()
                    .unwrap()
                    .reverses
                    .push(format!("{remote} {local}"));
                Ok(text(if *remote == "tcp:0" { "43210\n" } else { "" }))
            }
            other => panic!("unexpected adb command {other:?}"),
        }
    }
}

fn text(output: &str) -> Output {
    Output {
        code: Some(0),
        stdout: ai6::guatiao::Bytes(output.as_bytes().to_vec()),
        ..Output::default()
    }
}

fn device(state: &'static str) -> (Adb, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let adb = Adb::with_runner(Arc::new(FakeDevice {
        seen: seen.clone(),
        state,
    }));
    (adb, seen)
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
            eprintln!("SKIPPED: no `sh` on PATH for the stand-in device to run scripts");
            return;
        }
    };
}

#[test]
fn the_adb_transport_passes_the_contract_suite() {
    need_shell!();
    let scratch = tempfile::tempdir().unwrap();
    let (adb, _) = device("device\n");
    contract::check(
        Box::new(adb),
        &Host {
            shell: &["sh", "-c"],
            expand: |name| format!("${name}"),
            scratch: &scratch.path().to_string_lossy(),
        },
    );
}

#[test]
fn it_declares_what_an_android_device_offers() {
    let (adb, _) = device("device\n");
    let client = TransportClient::new(Box::new(adb));
    assert_eq!(
        client.capabilities().unwrap(),
        Capabilities {
            terminal: false,
            run: true,
            files: true,
            forward: true,
            os: Some("android".to_owned()),
            arch: None,
            notes: Vec::new(),
        }
    );
    let probed = client.probe().unwrap();
    assert_eq!(probed.arch.as_deref(), Some("aarch64"));
    assert_eq!(
        client.capabilities().unwrap().arch.as_deref(),
        Some("aarch64")
    );
}

#[test]
fn a_device_that_is_not_ready_fails_the_probe_with_its_state() {
    let (adb, _) = device("offline\n");
    let error = TransportClient::new(Box::new(adb)).probe().unwrap_err();
    assert!(ai6::describe(&error).contains("offline"), "{error:?}");
}

#[test]
fn a_reverse_forward_reaches_the_stations_loopback_and_is_removed_on_close() {
    let (adb, seen) = device("device\n");
    let client = TransportClient::new(Box::new(adb));
    let forward = client
        .forward(&ForwardSpec {
            remote_host: "127.0.0.1".to_owned(),
            remote_port: 0,
            target: "127.0.0.1:8123".to_owned(),
        })
        .unwrap();
    assert_eq!(forward.info().remote_port, 43210);
    forward.close().unwrap();
    forward.close().unwrap();
    drop(forward);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.reverses, ["tcp:0 tcp:8123"]);
    assert_eq!(seen.removed, ["tcp:43210"], "removed once");

    let refused = client.forward(&ForwardSpec {
        remote_host: "127.0.0.1".to_owned(),
        remote_port: 0,
        target: "10.0.0.5:8123".to_owned(),
    });
    assert!(refused.is_err_and(|e| ai6::describe(&e).contains("loopback")));
}

#[test]
fn the_command_line_sent_to_the_device_is_quoted_word_by_word() {
    need_shell!();
    let (adb, _) = device("device\n");
    let output = TransportClient::new(Box::new(adb))
        .run(&RunSpec {
            argv: ["printf", "%s|", "a b", "it's", "$HOME", "*"]
                .map(str::to_owned)
                .to_vec(),
            ..RunSpec::default()
        })
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout.0),
        "a b|it's|$HOME|*|"
    );
}
