use std::process::Command;

fn gateway() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_universal-exports"));
    command.env_remove("UNEX_CONFIG");
    command
}

#[test]
fn help_names_the_bootstrap_file_and_its_overrides() {
    let output = gateway().arg("--help").output().unwrap();

    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for named in [
        "unex.cfg",
        "UNEX_CONFIG",
        "station_token",
        "--config",
        "--provider",
        "--registry",
        "--listen",
        "--public-origin",
        "--plugin-path",
        "--plugin-profile",
    ] {
        assert!(help.contains(named), "{named}");
    }
    for removed in [
        "--port",
        "--roster",
        "--plugin-pins",
        "--authenticator",
        "AI6_",
    ] {
        assert!(!help.contains(removed), "{removed}");
    }
}

#[test]
fn a_missing_station_token_fails_without_echoing_a_value() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("unex.cfg");
    std::fs::write(
        &config,
        "public_origin = \"https://localhost\"\nstate_dir = \"state\"\nstation_token = { file = \"token\" }\n",
    )
    .unwrap();
    let run = || {
        let output = gateway().arg("--config").arg(&config).output().unwrap();
        (
            output.status.code(),
            String::from_utf8(output.stderr).unwrap(),
        )
    };

    let (code, error) = run();
    assert_eq!(code, Some(1));
    assert!(
        error.contains("station_token") && error.contains("token"),
        "{error}"
    );

    std::fs::write(
        &config,
        "public_origin = \"https://localhost\"\nstation_token = \"inline-secret-value\"\nport = 1\n",
    )
    .unwrap();
    let (code, error) = run();
    assert_eq!(code, Some(1));
    assert!(error.contains("port"), "an unknown key is named: {error}");
    assert!(!error.contains("inline-secret-value"), "{error}");
}

#[test]
fn a_named_bootstrap_file_that_does_not_exist_fails() {
    let dir = tempfile::tempdir().unwrap();
    let output = gateway()
        .arg("--config")
        .arg(dir.path().join("absent.cfg"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("does not exist")
    );
}

#[test]
fn unex_cfg_and_the_provider_alone_start_the_gateway_on_the_listener_it_names() {
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(dir.path().join("token"), "t".repeat(48)).unwrap();
    std::fs::write(
        dir.path().join("unex.cfg"),
        format!(
            "listen = \"127.0.0.1:{port}\"\npublic_origin = \"https://localhost\"\n\
             state_dir = \"state\"\nstation_token = {{ file = \"token\" }}\n"
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("unex.toml"), "assistant = false\n").unwrap();
    let mut gateway = gateway()
        .arg("--config")
        .arg(dir.path().join("unex.cfg"))
        .env("AI6_EXPORTS_PORT", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut healthy = false;
    while std::time::Instant::now() < deadline {
        if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            use std::io::{Read, Write};
            let _ = stream
                .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            if answer.starts_with("HTTP/1.1 ") {
                healthy = true;
                break;
            }
        }
        if gateway.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    let _ = gateway.kill();
    let output = gateway.wait_with_output().unwrap();
    assert!(
        healthy,
        "the gateway answers on unex.cfg's listen whatever AI6_EXPORTS_PORT says: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
