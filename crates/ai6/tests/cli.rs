use std::process::Command;

fn ai6() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ai6"))
}

#[test]
fn serve_help_lists_watchdog_configuration() {
    let output = ai6()
        .args(["serve", "--help"])
        .output()
        .expect("the AI6 command should start");

    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("help should be UTF-8");
    for flag in [
        "--stall-timeout",
        "--loop-timeout",
        "--loop-min-lines",
        "--blocked-after",
        "--max-runtime",
        "--history",
        "--normalize-digits",
    ] {
        assert!(help.contains(flag), "serve help should include {flag}");
    }
}

#[test]
fn negative_watchdog_timeout_is_an_invalid_invocation() {
    let output = ai6()
        .args(["serve", "--stall-timeout", "-1"])
        .output()
        .expect("the AI6 command should start");

    assert_eq!(output.status.code(), Some(2));
    let error = String::from_utf8(output.stderr).expect("CLI error should be UTF-8");
    assert!(error.contains("invalid value"));
}

#[test]
fn sleepers_are_listed_authorized_and_revoked() {
    let config = tempfile::tempdir().unwrap();
    let registry = tempfile::tempdir().unwrap();
    std::fs::write(
        config.path().join("operations.toml"),
        "[operation.G]\n[operation.G.sleepers.deps]\nschedule = \"0 3 * * *\"\ntitle = \"Update dependencies\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        let output = ai6()
            .args(["sleeper", "--config-dir"])
            .arg(config.path())
            .args(args)
            .env("AI6_REGISTRY", registry.path())
            .output()
            .expect("the AI6 command should start");
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
        )
    };

    assert!(run(&["list"]).1.contains("waiting for authorization"));
    let (ok, text) = run(&["authorize", "G", "deps"]);
    assert!(ok && text.contains("Authorized sleeper deps"), "{text}");
    assert!(run(&["list"]).1.contains("authorized by"));
    assert!(!run(&["authorize", "G", "missing"]).0);
    assert!(run(&["revoke", "G", "deps"]).1.contains("Revoked"));
    assert!(run(&["list"]).1.contains("waiting for authorization"));
}
