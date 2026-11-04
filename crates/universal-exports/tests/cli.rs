use std::process::Command;

fn gateway() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_universal-exports"));
    command.env_remove("AI6_CONFIG");
    command
}

#[test]
fn help_names_the_bootstrap_file_and_its_overrides() {
    let output = gateway().arg("--help").output().unwrap();

    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for named in [
        "ai6.cfg",
        "AI6_CONFIG",
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
        &["--", "roster"].concat(),
        &["--plugin", "-pins"].concat(),
        "--authenticator",
        "unex.cfg",
        "UNEX_CONFIG",
    ] {
        assert!(!help.contains(removed), "{removed}");
    }
}

#[test]
fn a_missing_station_token_fails_without_echoing_a_value() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("ai6.cfg");
    std::fs::write(
        &config,
        "[unex]\npublic_origin = \"https://localhost\"\nstate_dir = \"state\"\nstation_token = { provider = \"file\", key = \"token\" }\n",
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
        "[unex]\npublic_origin = \"https://localhost\"\nstation_token = { provider = \"inline\", value = \"inline-secret-value\" }\nport = 1\n",
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
fn ai6_cfg_and_the_provider_alone_start_the_gateway_on_the_listener_it_names() {
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(dir.path().join("token"), "t".repeat(48)).unwrap();
    std::fs::write(
        dir.path().join("ai6.cfg"),
        format!(
            "[unex]\nlisten = \"127.0.0.1:{port}\"\npublic_origin = \"https://localhost\"\n\
             state_dir = \"state\"\nstation_token = {{ provider = \"file\", key = \"token\" }}\n"
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("unex.toml"), "assistant = false\n").unwrap();
    let mut gateway = gateway()
        .arg("--config")
        .arg(dir.path().join("ai6.cfg"))
        .env(["AI6", "EXPORTS", "PORT"].join("_"), "1")
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
        "the gateway answers on [unex] listen whatever the old port variable says: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Writes the one file every program starts from in `dir`, with the addresses it needs.
fn one_file(dir: &std::path::Path, port: u16) -> std::path::PathBuf {
    std::fs::create_dir_all(dir.join("config")).unwrap();
    std::fs::write(dir.join("config").join("unex.toml"), "assistant = false\n").unwrap();
    std::fs::write(dir.join("station-token"), "t".repeat(48)).unwrap();
    let text = include_str!("../../ai6/tests/one_file.cfg")
        .replace("UNEX_URL", "http://127.0.0.1:9")
        .replace("GATEWAY_LISTEN", &format!("127.0.0.1:{port}"))
        .replace("STATION_LISTEN", "127.0.0.1:9");
    let path = dir.join("ai6.cfg");
    std::fs::write(&path, text).unwrap();
    path
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// What a program said as it stopped: its exit code and standard error. A program that goes on
/// serving past the deadline is killed, and has no exit code to report.
fn stopped(command: &mut Command) -> (Option<i32>, String) {
    let mut child = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while child.try_wait().unwrap().is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let code = child.try_wait().unwrap().and_then(|status| status.code());
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    (code, String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn the_gateway_starts_from_the_one_file_every_program_shares() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let config = one_file(dir.path(), port);
    let mut gateway = gateway()
        .arg("--config")
        .arg(&config)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let mut listening = false;
    while std::time::Instant::now() < deadline && gateway.try_wait().unwrap().is_none() {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            listening = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = gateway.kill();
    let output = gateway.wait_with_output().unwrap();
    assert!(
        listening,
        "the gateway listens on [unex] listen: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !dir.path().join("station-state").exists(),
        "the Station's directory is not the gateway's"
    );
}

#[test]
fn the_gateway_refuses_a_key_nobody_knows_and_does_not_repeat_its_value() {
    let dir = tempfile::tempdir().unwrap();
    let config = one_file(dir.path(), free_port());
    let text = std::fs::read_to_string(&config).unwrap();
    for (document, key) in [
        (
            format!("nobody_knows = \"pl4nted\"\n{text}"),
            "nobody_knows",
        ),
        (
            text.replace("[station]", "[station]\nnobody = \"pl4nted\""),
            "station.nobody",
        ),
        (
            text.replace("[unex]", "[unex]\nnobody = \"pl4nted\""),
            "unex.nobody",
        ),
    ] {
        std::fs::write(&config, document).unwrap();
        let (code, said) = stopped(gateway().arg("--config").arg(&config));
        assert_eq!(code, Some(1), "{said}");
        assert!(said.contains(key), "names {key}: {said}");
        assert!(!said.contains("pl4nted"), "{said}");
    }
}

#[test]
fn the_gateway_names_each_key_it_needs_and_the_file_does_not_hold() {
    let dir = tempfile::tempdir().unwrap();
    let config = one_file(dir.path(), free_port());
    let text = std::fs::read_to_string(&config).unwrap();
    for (document, key) in [
        (
            text.replace("public_origin = \"https://localhost\"\n", ""),
            "public_origin",
        ),
        (
            text.replace(
                "station_token = { provider = \"file\", key = \"station-token\" }\n",
                "",
            ),
            "station_token",
        ),
    ] {
        std::fs::write(&config, document).unwrap();
        let (code, said) = stopped(gateway().arg("--config").arg(&config));
        assert_eq!(code, Some(1), "{said}");
        assert!(said.contains(key), "names {key}: {said}");
    }
}
