//! The `container` transport against a stand-in for the engine's command line that runs
//! `exec` with the host's own processes.

use std::process::Command;
use std::sync::{Arc, Mutex};

use ai6::directorates::q::transport::contract::{self, Host};
use ai6::directorates::q::transport::kit::{ProcessInput, run_process};
use ai6::guatiao::library::ProviderError;
use ai6::{Output, RunSpec, TransportClient};
use ai6_transport_container::{Container, Runner};

/// The `exec` calls the stand-in engine saw.
type Calls = Arc<Mutex<Vec<Vec<String>>>>;

struct FakeEngine {
    calls: Calls,
    running: &'static str,
}

impl Runner for FakeEngine {
    fn engine(&self, args: &[String], input: &ProcessInput<'_>) -> Result<Output, ProviderError> {
        let words: Vec<&str> = args.iter().map(String::as_str).collect();
        match words.as_slice() {
            ["inspect", "-f", _, "box"] => Ok(Output {
                code: Some(0),
                stdout: ai6::guatiao::Bytes(self.running.as_bytes().to_vec()),
                ..Output::default()
            }),
            ["exec", rest @ ..] => {
                self.calls.lock().unwrap().push(args.to_vec());
                // exec [-i] [-u U] [-w DIR] [-e K=V]... CONTAINER COMMAND...
                let mut command: Option<Command> = None;
                let mut directory = None;
                let mut env = Vec::new();
                let mut iter = rest.iter();
                while let Some(word) = iter.next() {
                    match *word {
                        "-i" => {}
                        "-u" => {
                            iter.next();
                        }
                        "-w" => directory = iter.next().map(|dir| (*dir).to_owned()),
                        "-e" => env.push(iter.next().unwrap().to_string()),
                        "box" => {
                            let program = iter.next().expect("a command");
                            let mut built = Command::new(program);
                            built.args(iter.by_ref());
                            command = Some(built);
                        }
                        other => panic!("unexpected exec word {other:?}"),
                    }
                }
                let mut command = command.expect("the container name");
                if let Some(directory) = directory {
                    command.current_dir(directory);
                }
                for pair in env {
                    let (name, value) = pair.split_once('=').unwrap();
                    command.env(name, value);
                }
                run_process(&mut command, input, std::time::Duration::from_secs(1))
            }
            other => panic!("unexpected engine command {other:?}"),
        }
    }
}

fn engine_with(running: &'static str) -> (Container, Calls) {
    let calls = Calls::default();
    let engine = FakeEngine {
        calls: calls.clone(),
        running,
    };
    (
        Container::with_runner(Arc::new(engine), "box", Some("1000".to_owned())),
        calls,
    )
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
            eprintln!("SKIPPED: no `sh` on PATH for the stand-in engine to run commands");
            return;
        }
    };
}

#[test]
fn the_container_transport_passes_the_contract_suite() {
    need_shell!();
    let scratch = tempfile::tempdir().unwrap();
    let (container, _) = engine_with("true\n");
    contract::check(
        Box::new(container),
        &Host {
            shell: &["sh", "-c"],
            expand: |name| format!("${name}"),
            scratch: &scratch.path().to_string_lossy(),
        },
    );
}

#[test]
fn a_command_is_run_directly_with_the_containers_own_cwd_env_and_user() {
    need_shell!();
    let (container, calls) = engine_with("true\n");
    let scratch = tempfile::tempdir().unwrap();
    let output = TransportClient::new(Box::new(container))
        .run(&RunSpec {
            argv: ["sh", "-c", "echo \"$AI6_PROBE\"; pwd"]
                .map(str::to_owned)
                .to_vec(),
            cwd: Some(scratch.path().to_string_lossy().into_owned()),
            env: Some([("AI6_PROBE".to_owned(), "x y".to_owned())].into()),
            ..RunSpec::default()
        })
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout.0).starts_with("x y\n"));
    let calls = calls.lock().unwrap();
    let expected_dir = scratch.path().to_string_lossy().into_owned();
    assert_eq!(
        calls[0],
        [
            "exec",
            "-u",
            "1000",
            "-w",
            expected_dir.as_str(),
            "-e",
            "AI6_PROBE=x y",
            "box",
            "sh",
            "-c",
            "echo \"$AI6_PROBE\"; pwd"
        ]
    );
}

#[test]
fn a_probe_needs_a_running_container_and_learns_its_platform() {
    need_shell!();
    let (container, _) = engine_with("true\n");
    let probed = TransportClient::new(Box::new(container)).probe().unwrap();
    assert!(probed.os.is_some() && probed.arch.is_some() && probed.files);
    assert!(!probed.terminal && !probed.forward);

    let (stopped, _) = engine_with("false\n");
    let error = TransportClient::new(Box::new(stopped)).probe().unwrap_err();
    assert!(
        ai6::describe(&error).contains("is not running"),
        "{error:?}"
    );
}

#[test]
fn an_environment_name_the_engine_would_misread_is_refused() {
    let (container, calls) = engine_with("true\n");
    let error = TransportClient::new(Box::new(container))
        .run(&RunSpec {
            argv: vec!["true".to_owned()],
            env: Some([("A B".to_owned(), "v".to_owned())].into()),
            ..RunSpec::default()
        })
        .unwrap_err();
    assert!(ai6::describe(&error).contains("environment variable"));
    assert!(calls.lock().unwrap().is_empty());
}
