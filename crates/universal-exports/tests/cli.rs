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
        "AI6_EXPORTS_PORT",
        "AI6_EXPORTS_PUBLIC_ORIGIN",
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
