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
fn plugins_pin_prints_an_entry_for_a_provider_library_and_refuses_anything_else() {
    // The access plugins are dev-dependencies, built beside this test binary.
    let deps = std::env::current_exe().unwrap();
    let library = std::fs::read_dir(deps.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == std::env::consts::DLL_EXTENSION)
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().contains("coqui_roles_plugin"))
        })
        .expect("the roles plugin is built beside the test binary");
    let output = ai6()
        .args(["plugins", "pin"])
        .arg(&library)
        .output()
        .expect("the AI6 command should start");
    assert!(output.status.success(), "{output:?}");
    let entry = String::from_utf8(output.stdout).unwrap();
    assert!(entry.starts_with("[[library]]\n"), "{entry}");
    assert!(entry.contains("pin = \"sha256:"), "{entry}");

    let output = ai6()
        .args(["plugins", "pin"])
        .arg(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("the AI6 command should start");
    assert!(!output.status.success());
}

#[test]
fn a_named_pin_set_that_does_not_exist_stops_the_station_and_the_runner() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("absent-plugins.toml");
    for arguments in [
        vec!["serve", "--listen", "127.0.0.1:0"],
        vec!["runner", "serve"],
    ] {
        let output = ai6()
            .args(&arguments)
            .arg("--plugin-pins")
            .arg(&missing)
            .env("AI6_STATE", directory.path())
            .env("AI6_CONFIG_DIR", directory.path())
            .env("AI6_REGISTRY", directory.path().join("registry"))
            .output()
            .expect("the AI6 command should start");
        assert!(!output.status.success(), "{arguments:?}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(
            error.contains("absent-plugins.toml") && error.contains("does not exist"),
            "{arguments:?}: {error}"
        );
    }
}

#[test]
fn harness_pin_prints_the_manifest_and_its_pin() {
    let checkout = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(checkout.path().join(".claude")).unwrap();
    std::fs::write(checkout.path().join(".claude/settings.json"), "{}").unwrap();
    let output = ai6()
        .args(["harness", "pin"])
        .arg(checkout.path())
        .output()
        .expect("the AI6 command should start");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(".claude/settings.json"), "{text}");
    assert!(
        text.lines().last().unwrap().starts_with("pin = \"sha256:"),
        "{text}"
    );
}

/// The pin `ai6 assistant pin-skill` prints is the one a roster takes and a Station checks its
/// copy against: the digest of [`ai6::ProjectManifest::read_tree`].
#[test]
fn assistant_pin_skill_prints_the_pin_the_roster_takes() {
    let config = tempfile::tempdir().unwrap();
    let skill = config.path().join("skills").join("charts");
    std::fs::create_dir_all(skill.join("scripts")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: charts\ndescription: Draws charts.\n---\nDraw.\n",
    )
    .unwrap();
    std::fs::write(skill.join("scripts").join("plot.py"), "print(1)\n").unwrap();
    let output = ai6()
        .args(["assistant", "pin-skill"])
        .arg(&skill)
        .output()
        .expect("the AI6 command should start");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("  SKILL.md") && text.contains("  scripts/plot.py"),
        "{text}"
    );
    let pin = text
        .lines()
        .last()
        .and_then(|line| line.strip_prefix("pin = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap()
        .to_owned();
    assert_eq!(
        pin,
        ai6::ProjectManifest::read_tree(&skill)
            .unwrap()
            .digest()
            .to_string()
    );
    std::fs::write(
        config.path().join("agents.toml"),
        format!("[assistant.skills.charts]\npath = \"skills/charts\"\npin = \"{pin}\"\n"),
    )
    .unwrap();
    let roster = ai6::Roster::load(config.path()).unwrap();
    assert_eq!(
        roster.assistant().skills()["charts"]
            .pin()
            .unwrap()
            .to_string(),
        pin
    );

    let missing = ai6()
        .args(["assistant", "pin-skill"])
        .arg(config.path().join("missing"))
        .output()
        .expect("the AI6 command should start");
    assert!(!missing.status.success());
}

#[test]
fn a_safe_house_cache_that_is_not_a_plain_directory_name_stops_the_station() {
    let directory = tempfile::tempdir().unwrap();
    let output = ai6()
        .args([
            "serve",
            "--listen",
            "127.0.0.1:0",
            "--safe-house-cache",
            "../outside",
        ])
        .env("AI6_STATE", directory.path())
        .env("AI6_CONFIG_DIR", directory.path())
        .env("AI6_REGISTRY", directory.path().join("registry"))
        .output()
        .expect("the AI6 command should start");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("--safe-house-cache") && error.contains("../outside"),
        "{error}"
    );
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
fn theater_provision_names_what_is_missing_before_contacting_anything() {
    let config = tempfile::tempdir().unwrap();
    std::fs::write(
        config.path().join("theaters.toml"),
        "[theater.box]\naccess = { ssh = { host = \"b.example\" } }\n",
    )
    .unwrap();
    let run = |theater: &str| {
        let output = ai6()
            .args(["theater", "provision", theater, "--check", "--config-dir"])
            .arg(config.path())
            .output()
            .expect("the AI6 command should start");
        (
            output.status.success(),
            String::from_utf8(output.stderr).unwrap(),
        )
    };
    let (ok, error) = run("absent");
    assert!(!ok && error.contains("theaters.toml declares no Theater \"absent\""));
    let (ok, error) = run("box");
    assert!(
        !ok && error.contains("no provider `ssh` is loaded"),
        "{error}"
    );
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
    use ai6::station::identity::{Identity, PeerId, TrustStore, TrustStoreExt};

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
    let own = shown.trim().parse::<PeerId>().unwrap();
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

    // Any spelling of the fingerprint is accepted, grouped base32 included.
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

#[test]
fn a_detached_runner_answers_calls_until_it_stops() {
    use std::io::Write;
    use std::process::Stdio;

    let state = tempfile::tempdir().unwrap();
    let mut starter = ai6()
        .args(["runner", "serve", "--detach", "--state-dir"])
        .arg(state.path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the AI6 command should start");
    // Only the first line is read: on Windows the runner inherits the starter's output pipe,
    // so the pipe stays open for as long as the runner runs.
    let mut line = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(starter.stdout.take().unwrap()),
        &mut line,
    )
    .unwrap();
    assert!(starter.wait().unwrap().success());
    let info: serde_json::Value = serde_json::from_str(&line).unwrap();
    let pid = info["pid"].as_u64().unwrap();
    assert!(
        info["url"]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );

    let call = |method: &str, path: &str, body: Option<&str>| {
        let mut child = ai6()
            .args(["runner", "call", "--state-dir"])
            .arg(state.path())
            .args([method, path])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        if let Some(body) = body {
            stdin.write_all(body.as_bytes()).unwrap();
        }
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        let answer = serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .unwrap_or(serde_json::Value::Null);
        (output.status.code(), answer)
    };

    let (code, health) = call("GET", "/v1/health", None);
    assert_eq!(code, Some(0));
    assert_eq!(health["status"], 200);
    assert_eq!(health["body"]["pid"], pid);

    // A request body comes on standard input; the runner's refusal is relayed, not hidden.
    let (code, refused) = call(
        "POST",
        "/v1/sessions",
        Some(
            r#"{"key":"k1","handler":"absent","launch":{"start":{"mission_id":"m-1","agent_id":"007","prompt":"p","initial_context":[],"working_directory":".","clearance":"field","environment":{},"model":null,"effort":null,"max_cost_micro_usd":null}}}"#,
        ),
    );
    assert_eq!(code, Some(0));
    assert_eq!(refused["status"], 409, "{refused}");
    assert!(refused["body"]["error"].as_str().is_some());

    let stopped = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
    } else {
        Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
    }
    .unwrap();
    assert!(stopped.status.success());
    let mut code = Some(0);
    for _ in 0..200 {
        code = call("GET", "/v1/health", None).0;
        if code != Some(0) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(code, Some(3), "a call with no runner to answer exits 3");
}

/// Runs `ai6 green-ink gate` as Claude Code runs a `PreToolUse` hook: the hook's JSON on
/// standard input, no Cable endpoint in the environment.
fn gate(command: &str) -> std::process::Output {
    use std::io::Write;
    let mut child = ai6()
        .args(["green-ink", "gate"])
        .env_remove("AI6_CABLE_MCP_URL")
        .env_remove("AI6_CABLE_MCP_TOKEN")
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the AI6 command should start");
    let input = serde_json::json!({
        "session_id": "abc123",
        "cwd": std::env::temp_dir(),
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": command, "description": "run it" },
    });
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn the_green_ink_hook_denies_a_governed_step_and_stays_silent_otherwise() {
    let held = gate("gh release create v1.2.0");
    assert!(held.status.success(), "a hook decision exits 0");
    let decision: serde_json::Value = serde_json::from_slice(&held.stdout).unwrap();
    let output = &decision["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], "PreToolUse");
    assert_eq!(output["permissionDecision"], "deny");
    let reason = output["permissionDecisionReason"].as_str().unwrap();
    assert!(
        reason.contains("release tag v1.2.0") && reason.contains("no Station"),
        "{reason}"
    );

    let quiet = gate("cargo test --workspace");
    assert!(quiet.status.success());
    assert!(
        quiet.stdout.is_empty(),
        "nothing printed lets the call go on"
    );

    // Input the hook cannot read is denied, not let through.
    let mut child = ai6()
        .args(["green-ink", "gate", "--cable-url", "http://127.0.0.1:9/mcp"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the AI6 command should start");
    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"not json").unwrap();
    }
    let garbled = child.wait_with_output().unwrap();
    assert!(garbled.status.success());
    let decision: serde_json::Value = serde_json::from_slice(&garbled.stdout).unwrap();
    assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny");
}
