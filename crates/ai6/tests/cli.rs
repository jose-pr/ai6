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

#[test]
fn identity_and_peers_work_on_a_state_directory_with_the_source_address_shown() {
    use ai6::station::identity::{Fingerprint, Identity, TrustStore, TrustStoreExt};

    let state = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let output = ai6()
            .args(args)
            .env("AI6_STATE", state.path())
            .output()
            .expect("the AI6 command should start");
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
        )
    };

    let (ok, shown) = run(&["identity"]);
    assert!(ok);
    assert!(shown.starts_with("sha256:"), "{shown}");
    let own = shown.trim().parse::<Fingerprint>().unwrap();
    assert_eq!(run(&["identity"]).1, shown, "the identity is kept");
    assert_eq!(
        Identity::read(&state.path().join("identity.pem"))
            .unwrap()
            .unwrap()
            .fingerprint(),
        own
    );

    let (ok, none) = run(&["peer", "waiting"]);
    assert!(ok && none.contains("No Stations are waiting"));

    let stranger = Identity::generate().unwrap();
    let source: std::net::SocketAddr = "192.0.2.7:4000".parse().unwrap();
    TrustStore::open(state.path())
        .unwrap()
        .record_waiting_certificate(stranger.certificate_der(), Some(source));
    let (ok, waiting) = run(&["peer", "waiting"]);
    assert!(ok);
    assert!(
        waiting.contains(&stranger.fingerprint().to_string()),
        "{waiting}"
    );
    assert!(waiting.contains("192.0.2.7:4000"), "{waiting}");

    // Any spelling of the fingerprint is accepted, including the grouped base32 that earlier
    // versions printed.
    let (ok, accepted) = run(&[
        "peer",
        "accept",
        &stranger.fingerprint().to_base32_grouped(),
    ]);
    assert!(ok && accepted.contains("Accepted station"), "{accepted}");
    assert!(
        run(&["peer", "waiting"])
            .1
            .contains("No Stations are waiting")
    );
    assert!(!run(&["peer", "accept", &stranger.fingerprint().to_string()]).0);
}

#[test]
fn agents_are_quarantined_listed_and_released() {
    let registry = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let output = ai6()
            .arg("quarantine")
            .args(args)
            .env("AI6_REGISTRY", registry.path())
            .output()
            .expect("the AI6 command should start");
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
        )
    };

    assert!(run(&["list"]).1.contains("No Agents are quarantined"));
    let (ok, text) = run(&["add", "007", "looping", "--operation", "G"]);
    assert!(ok && text.contains("Quarantined 007 in G"), "{text}");
    let listed = run(&["list"]).1;
    assert!(
        listed.contains("007\tG\t") && listed.contains("looping"),
        "{listed}"
    );
    assert!(
        !run(&["release", "007"]).0,
        "a release names an Operation or --all"
    );
    assert!(
        run(&["release", "007", "--all"])
            .1
            .contains("not quarantined")
    );
    assert!(
        run(&["release", "007", "--operation", "G"])
            .1
            .contains("Released 007")
    );
    assert!(run(&["list"]).1.contains("No Agents are quarantined"));
}
