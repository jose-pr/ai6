use std::time::{Duration, Instant};

use ai6::directorates::q::transport::contract::{self, Host};
use ai6::guatiao::Bytes;
use ai6::{RunSpec, Transport, TransportClient};
use ai6_transport_local::Local;

#[cfg(windows)]
const SHELL: &[&str] = &["cmd", "/C"];
#[cfg(not(windows))]
const SHELL: &[&str] = &["sh", "-c"];

fn expand(name: &str) -> String {
    if cfg!(windows) {
        format!("%{name}%")
    } else {
        format!("${name}")
    }
}

fn client() -> TransportClient {
    TransportClient::new(Box::new(Local::default()))
}

#[test]
fn the_local_transport_passes_the_contract_suite() {
    let scratch = tempfile::tempdir().unwrap();
    contract::check(
        Box::new(Local::default()),
        &Host {
            shell: SHELL,
            expand,
            scratch: &scratch.path().to_string_lossy(),
        },
    );
}

#[test]
fn it_claims_exactly_what_it_does() {
    let capabilities = client().capabilities().unwrap();
    assert!(capabilities.run && capabilities.files);
    assert!(!capabilities.terminal && !capabilities.forward);
    assert_eq!(capabilities.os.as_deref(), Some(std::env::consts::OS));
    assert_eq!(capabilities.arch.as_deref(), Some(std::env::consts::ARCH));
    assert!(Local::default().probe().is_ok());
}

#[test]
fn standard_input_reaches_the_program() {
    let argv: Vec<String> = if cfg!(windows) {
        vec!["findstr".to_owned(), "^".to_owned()]
    } else {
        vec!["cat".to_owned()]
    };
    let output = client()
        .run(&RunSpec {
            argv,
            stdin: Some(Bytes(b"piped in\n".to_vec())),
            ..RunSpec::default()
        })
        .unwrap();
    assert_eq!(output.code, Some(0));
    assert!(String::from_utf8_lossy(&output.stdout.0).contains("piped in"));
}

#[test]
fn a_program_that_outlives_its_timeout_is_killed() {
    let argv: Vec<String> = if cfg!(windows) {
        ["ping", "-n", "30", "127.0.0.1"]
            .map(str::to_owned)
            .to_vec()
    } else {
        ["sleep", "30"].map(str::to_owned).to_vec()
    };
    let started = Instant::now();
    let output = client()
        .run(&RunSpec {
            argv,
            timeout_ms: 200,
            ..RunSpec::default()
        })
        .unwrap();
    assert!(output.timed_out);
    assert_eq!(output.code, None);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn an_empty_argv_is_refused() {
    assert!(client().run(&RunSpec::default()).is_err());
}

#[test]
fn the_reader_grace_is_a_config_key_with_a_default() {
    use ai6::guatiao::{FromValue, ToValue};
    use ai6_transport_local::LocalConfig;
    let empty = std::collections::BTreeMap::<String, u64>::new()
        .to_value()
        .unwrap();
    assert_eq!(
        LocalConfig::from_value(&empty).unwrap(),
        LocalConfig::default()
    );
    let set = std::collections::BTreeMap::from([("reader_grace_ms".to_owned(), 5_u64)])
        .to_value()
        .unwrap();
    assert_eq!(LocalConfig::from_value(&set).unwrap().reader_grace_ms, 5);
}
