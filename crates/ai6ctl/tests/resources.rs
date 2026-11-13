//! `ai6ctl get|apply|edit|set|unset|delete|api-resources` against a real gateway over an empty
//! SQL configuration provider, as a first install has it.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use ai6::directorates::registry::backend::*;
use ai6::{
    Attempt, Authenticator, ConfigProvider, ConfigRecord, ConfigScope, Directory, FileRegistry,
    Lookup, Person, SqlProvider, Verdict, guatiao::library::ProviderError,
};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use universal_exports::{AccessProviders, Gateway, GatewayConfig, GatewayLimits, LiveAccess};

const ORIGIN: &str = "https://exports.example";

/// `boss` is the bootstrap's chief; `guest` is nobody.
struct Accounts;

impl Authenticator for Accounts {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError> {
        Ok(match (attempt.name, attempt.secret) {
            (Some(name), Some(secret)) if secret == "secret" => Verdict::Valid(Lookup {
                by: "name".to_owned(),
                value: name,
            }),
            _ => Verdict::Invalid,
        })
    }
}

impl Directory for Accounts {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError> {
        let name = lookup.value.trim_end_matches("-id").to_owned();
        Ok(Some(Person {
            id: format!("{name}-id"),
            name,
            ..Person::default()
        }))
    }
}

struct Deployment {
    server: String,
    boss: String,
    guest: String,
    registry: FileRegistry,
    /// A Station's own handle on the shared database.
    station: SqlProvider,
    stop: Option<oneshot::Sender<()>>,
    directory: tempfile::TempDir,
}

impl Drop for Deployment {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn login(server: &str, name: &str) -> String {
    let answer: Value = reqwest::Client::new()
        .post(format!("{server}/v1/auth/login"))
        .header("origin", ORIGIN)
        .header("x-forwarded-proto", "https")
        .json(&json!({"name": name, "secret": "secret"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    answer["token"].as_str().unwrap().to_owned()
}

/// The services started on an empty database: nothing is stored but what the bootstrap's `chief`
/// seeds.
async fn first_install() -> Deployment {
    let directory = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", directory.path().join("config.db").display());
    let provider = Arc::new(SqlProvider::open(&location).unwrap());
    let registry = FileRegistry::open(directory.path().join("registry"));
    let shared: Arc<dyn RegistryBackend> = Arc::new(registry.clone());
    let live =
        Arc::new(LiveAccess::new(provider.clone(), vec!["account:boss".to_owned()], None).unwrap());
    let source: Arc<dyn ConfigProvider> = provider.clone();
    live.seed(&source.load("").unwrap(), &shared).unwrap();
    live.apply(&source.load("").unwrap());
    let accounts = Arc::new(Accounts);
    let gateway = Gateway::with_access(
        GatewayConfig::new(
            ORIGIN,
            "exports-test-credential-with-more-than-32-bytes",
            GatewayLimits::default(),
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap(),
        shared,
        AccessProviders::new(accounts.clone(), accounts, live.clone()),
    )
    .with_access_admin(live)
    .with_roster(provider);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    tokio::spawn(async move {
        let _ = axum::serve(listener, gateway.router())
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await;
    });
    Deployment {
        boss: login(&server, "boss").await,
        guest: login(&server, "guest").await,
        server,
        registry,
        station: SqlProvider::open(&location).unwrap(),
        stop: Some(stop),
        directory,
    }
}

struct Ran {
    ok: bool,
    out: String,
    err: String,
}

async fn ai6ctl(
    deployment: &Deployment,
    token: &str,
    args: &[&str],
    stdin: Option<&str>,
    envs: &[(&str, String)],
) -> Ran {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ai6ctl"));
    command
        .arg("--timeout")
        .arg("120")
        .arg("--server")
        .arg(&deployment.server)
        .args(args)
        .env("UNEX_ACCESS_TOKEN", token)
        .envs(envs.iter().map(|(name, value)| (*name, value)))
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let stdin = stdin.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        let mut child = command.spawn().unwrap();
        if let Some(text) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        Ran {
            ok: output.status.success(),
            out: String::from_utf8_lossy(&output.stdout).replace('\r', ""),
            err: String::from_utf8_lossy(&output.stderr).replace('\r', ""),
        }
    })
    .await
    .unwrap()
}

async fn boss(deployment: &Deployment, args: &[&str]) -> String {
    let ran = ai6ctl(deployment, &deployment.boss, args, None, &[]).await;
    assert!(ran.ok, "{args:?}: {}", ran.err);
    ran.out
}

fn audits(deployment: &Deployment, action: &str) -> Vec<ai6::AuditRecord> {
    deployment
        .registry
        .list_audit(500)
        .unwrap()
        .into_iter()
        .filter(|record| record.action == action)
        .collect()
}

const FIRST: &str = "\
[operation.G]
roster = [\"007\"]

[agent.007]
handler = \"mock-agent\"
clearance = \"00\"

[charge.\"operation:G\"]
agent = \"007\"
";

fn charge(deployment: &Deployment) -> Option<ai6::directorates::m::charges::ChargeDefinition> {
    deployment
        .station
        .load("")
        .unwrap()
        .roster
        .charges()
        .get("operation:G")
        .map(|charge| charge.settings.clone())
}

#[tokio::test]
async fn a_first_install_is_loaded_by_the_chief_with_one_apply_and_nobody_else_may() {
    let deployment = first_install().await;
    assert!(
        deployment
            .station
            .definitions(ConfigRecord::Agent, &ConfigScope::Global)
            .unwrap()
            .records
            .is_empty()
    );
    let file = deployment.directory.path().join("first.cfg");
    std::fs::write(&file, FIRST).unwrap();
    let file = file.to_str().unwrap();

    let refused = ai6ctl(
        &deployment,
        &deployment.guest,
        &["apply", "-f", file],
        None,
        &[],
    )
    .await;
    assert!(!refused.ok);
    assert!(
        refused.err.contains("needs") && refused.err.contains("config:"),
        "the refusal names the family: {}",
        refused.err
    );
    assert!(
        deployment
            .station
            .definitions(ConfigRecord::Agent, &ConfigScope::Global)
            .unwrap()
            .records
            .is_empty()
    );

    let applied = boss(&deployment, &["apply", "-f", file]).await;
    for line in [
        "operation/G created",
        "agent/007 created",
        "charge/operation:G created",
    ] {
        assert!(applied.contains(line), "{applied}");
    }
    assert!(applied.contains("revision\t"));
    let revision = deployment.station.revision("").unwrap().get().to_string();
    assert!(
        applied.contains(&format!("revision\t{revision}")),
        "{applied}"
    );
    assert_eq!(charge(&deployment).unwrap().agent, "007");
    let updates = audits(&deployment, "roster.update");
    assert_eq!(updates.len(), 3, "one audit record per record stored");
    assert!(
        updates
            .iter()
            .any(|record| record.target == "charge operation:G")
    );

    let again = boss(&deployment, &["apply", "-f", file]).await;
    assert!(again.contains("charge/operation:G unchanged"), "{again}");
    assert_eq!(audits(&deployment, "roster.update").len(), 3);
    assert_eq!(
        deployment.station.revision("").unwrap().get().to_string(),
        revision,
        "an apply that changes nothing stores nothing"
    );

    let kinds = boss(&deployment, &["api-resources"]).await;
    assert!(
        kinds.contains("charge\tcharges\tconfig.read\tconfig.manage\tconfig:charge\tfalse\tfalse\ttrue\tconfig.read,config.manage"),
        "{kinds}"
    );
    assert!(kinds.contains("agent\tagents\troster.read\troster.manage\tconfig:roster"));
    let ran = ai6ctl(
        &deployment,
        &deployment.guest,
        &["api-resources"],
        None,
        &[],
    )
    .await;
    assert!(
        ran.ok && ran.out.lines().skip(1).all(|line| line.ends_with('\t')),
        "{}",
        ran.out
    );
}

#[tokio::test]
async fn apply_reads_cfg_yaml_and_standard_input_and_stores_all_or_none() {
    let deployment = first_install().await;
    let dir = deployment.directory.path().join("docs");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("a.cfg"),
        "[operation.G]\nroster = [\"007\"]\n\n[agent.007]\nhandler = \"mock-agent\"\nclearance = \"00\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("b.cfg"),
        "#!yaml\ncharge:\n  \"operation:G\":\n    agent: \"007\"\n    max_turns_per_hour: 3\n",
    )
    .unwrap();
    std::fs::write(dir.join("ignored.txt"), "not a document").unwrap();
    let before = deployment.station.revision("").unwrap();
    let dry = boss(
        &deployment,
        &["apply", "--dry-run", "-f", dir.to_str().unwrap()],
    )
    .await;
    assert!(dry.contains("charge/operation:G would be created"), "{dry}");
    assert!(dry.contains("dry run: nothing stored"), "{dry}");
    assert_eq!(deployment.station.revision("").unwrap(), before);
    assert!(audits(&deployment, "roster.update").is_empty());

    let applied = boss(&deployment, &["apply", "-f", dir.to_str().unwrap()]).await;
    assert!(applied.contains("agent/007 created"), "{applied}");
    assert_eq!(
        charge(&deployment).unwrap().max_turns_per_hour,
        ai6::Limit::at(3)
    );

    // One bad record keeps the good ones out too.
    let bad = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "-f", "-"],
        Some("[charge.\"operation:G\"]\nagent = \"007\"\nmax_turns_per_hour = 9\n\n[agent.008]\nextends = \"nobody\"\n"),
        &[],
    )
    .await;
    assert!(!bad.ok);
    assert!(bad.err.contains("refused"), "{}", bad.err);
    assert_eq!(
        charge(&deployment).unwrap().max_turns_per_hour,
        ai6::Limit::at(3)
    );
    assert!(!audits(&deployment, "roster.refused").is_empty());

    // The same document on standard input stores the same way, plain and with --format.
    let plain = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "-f", "-"],
        Some("[charge.\"operation:G\"]\nagent = \"007\"\nmax_turns_per_hour = 5\n"),
        &[],
    )
    .await;
    assert!(plain.ok, "{}", plain.err);
    assert_eq!(
        charge(&deployment).unwrap().max_turns_per_hour,
        ai6::Limit::at(5)
    );
    let json = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "--format", "json", "-f", "-"],
        Some(r#"{"charge": {"operation:G": {"agent": "007", "max_turns_per_hour": 6}}}"#),
        &[],
    )
    .await;
    assert!(json.ok, "{}", json.err);
    assert_eq!(
        charge(&deployment).unwrap().max_turns_per_hour,
        ai6::Limit::at(6)
    );

    let unknown = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "-f", "-"],
        Some("[nothing.x]\ny = 1\n"),
        &[],
    )
    .await;
    assert!(
        !unknown.ok && unknown.err.contains("unknown table `nothing`"),
        "{}",
        unknown.err
    );
}

#[tokio::test]
async fn get_set_unset_and_delete_work_on_the_stored_record_by_its_schema() {
    let deployment = first_install().await;
    let file = deployment.directory.path().join("first.cfg");
    std::fs::write(&file, FIRST).unwrap();
    boss(&deployment, &["apply", "-f", file.to_str().unwrap()]).await;

    let shown = boss(&deployment, &["get", "charges"]).await;
    assert!(shown.contains("[charge.\"operation:G\"]"), "{shown}");
    assert_eq!(
        boss(&deployment, &["get", "charge", "operation:G", "agent"]).await,
        "007\n"
    );
    let json: Value = serde_json::from_str(
        &boss(&deployment, &["get", "charge", "operation:G", "-o", "json"]).await,
    )
    .unwrap();
    assert_eq!(json["charge"]["operation:G"]["agent"], "007");
    let yaml = boss(&deployment, &["get", "charge", "-o", "yaml"]).await;
    assert!(
        yaml.contains("agent: '007'") || yaml.contains("agent: \"007\""),
        "{yaml}"
    );
    let missing = ai6ctl(
        &deployment,
        &deployment.boss,
        &["get", "charge", "operation:Z"],
        None,
        &[],
    )
    .await;
    assert!(
        !missing.ok && missing.err.contains("no charge record"),
        "{}",
        missing.err
    );

    let set = boss(
        &deployment,
        &[
            "set",
            "charge",
            "operation:G",
            "max_turns_per_hour=4",
            "wake_every=3600",
        ],
    )
    .await;
    assert!(set.contains("charge/operation:G configured"), "{set}");
    let held = charge(&deployment).unwrap();
    assert_eq!(held.max_turns_per_hour, ai6::Limit::at(4));
    assert_eq!(held.agent, "007", "the fields not named stay as stored");
    assert!(set.contains(&format!(
        "revision\t{}",
        deployment.station.revision("").unwrap().get()
    )));
    boss(
        &deployment,
        &["set", "charge", "operation:G", "max_turns_per_hour=none"],
    )
    .await;
    assert_eq!(
        charge(&deployment).unwrap().max_turns_per_hour,
        ai6::Limit::UNBOUNDED
    );
    let wrong = ai6ctl(
        &deployment,
        &deployment.boss,
        &["set", "charge", "operation:G", "nope=1"],
        None,
        &[],
    )
    .await;
    assert!(
        !wrong.ok && wrong.err.contains("no field nope"),
        "{}",
        wrong.err
    );

    boss(
        &deployment,
        &["unset", "charge", "operation:G", "max_turns_per_hour"],
    )
    .await;
    assert_eq!(
        charge(&deployment).unwrap().max_turns_per_hour,
        ai6::directorates::m::charges::ChargeDefinition::DEFAULTS.max_turns_per_hour
    );
    let absent = ai6ctl(
        &deployment,
        &deployment.boss,
        &["unset", "charge", "operation:G", "lease"],
        None,
        &[],
    )
    .await;
    assert!(
        !absent.ok && absent.err.contains("does not set lease"),
        "{}",
        absent.err
    );

    // A change over a newer one is refused.
    let revisions = boss(&deployment, &["get", "charge", "--revisions"]).await;
    let revision = revisions.trim().split('\t').nth(1).unwrap().to_owned();
    boss(
        &deployment,
        &["set", "charge", "operation:G", "turn_timeout=60"],
    )
    .await;
    let stale = ai6ctl(
        &deployment,
        &deployment.boss,
        &[
            "set",
            "charge",
            "operation:G",
            "turn_timeout=90",
            "--if-revision",
            &revision,
        ],
        None,
        &[],
    )
    .await;
    assert!(
        !stale.ok && stale.err.contains("changed meanwhile"),
        "{}",
        stale.err
    );

    // Another record names the Agent, so it stays.
    let named = ai6ctl(
        &deployment,
        &deployment.boss,
        &["delete", "agent", "007"],
        None,
        &[],
    )
    .await;
    assert!(
        !named.ok && named.err.contains("still named by"),
        "{}",
        named.err
    );
    assert!(!audits(&deployment, "roster.refused").is_empty());
    let removed = boss(&deployment, &["delete", "charge", "operation:G"]).await;
    assert!(removed.contains("charge/operation:G deleted"), "{removed}");
    assert!(charge(&deployment).is_none());
    let gone = ai6ctl(
        &deployment,
        &deployment.boss,
        &["delete", "charge", "operation:G"],
        None,
        &[],
    )
    .await;
    assert!(!gone.ok);
    assert_eq!(audits(&deployment, "roster.remove").len(), 1);

    // --force goes anyway, and the audit record says what named it.
    let forced = boss(&deployment, &["delete", "agent", "007", "--force"]).await;
    assert!(forced.contains("agent/007 deleted"), "{forced}");
    assert!(
        audits(&deployment, "roster.remove")
            .iter()
            .any(|record| record.target.contains("forced"))
    );
}

/// An editor that changes one number in the file it is given.
fn editor(directory: &std::path::Path) -> String {
    #[cfg(windows)]
    {
        let path = directory.join("editor.cmd");
        std::fs::write(
            &path,
            "@echo off\r\npowershell -NoProfile -Command \"(Get-Content -Raw '%1').Replace('max_turns_per_hour = 2', 'max_turns_per_hour = 8') | Set-Content -NoNewline '%1'\"\r\n",
        )
        .unwrap();
        path.display().to_string()
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let path = directory.join("editor.sh");
        std::fs::write(
            &path,
            "#!/bin/sh\nsed -i.bak 's/max_turns_per_hour = 2/max_turns_per_hour = 8/' \"$1\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }
}

#[tokio::test]
async fn edit_stores_what_the_editor_saved_and_nothing_when_it_changed_nothing() {
    let deployment = first_install().await;
    let file = deployment.directory.path().join("first.cfg");
    std::fs::write(&file, FIRST).unwrap();
    boss(&deployment, &["apply", "-f", file.to_str().unwrap()]).await;
    boss(
        &deployment,
        &["set", "charge", "operation:G", "max_turns_per_hour=2"],
    )
    .await;

    let before = deployment.station.revision("").unwrap();
    let kept = ai6ctl(
        &deployment,
        &deployment.boss,
        &["edit", "charge", "operation:G"],
        None,
        &[(
            "EDITOR",
            if cfg!(windows) {
                "cmd /c rem".to_owned()
            } else {
                "true".to_owned()
            },
        )],
    )
    .await;
    assert!(
        kept.ok && kept.out.contains("unchanged"),
        "{}{}",
        kept.out,
        kept.err
    );
    assert_eq!(deployment.station.revision("").unwrap(), before);

    let edited = ai6ctl(
        &deployment,
        &deployment.boss,
        &["edit", "charge", "operation:G"],
        None,
        &[("EDITOR", editor(deployment.directory.path()))],
    )
    .await;
    assert!(edited.ok, "{}", edited.err);
    assert!(
        edited.out.contains("charge/operation:G configured"),
        "{}",
        edited.out
    );
    assert_eq!(
        charge(&deployment).unwrap().max_turns_per_hour,
        ai6::Limit::at(8)
    );
}

#[tokio::test]
async fn a_family_the_person_holds_no_action_on_is_refused_naming_it() {
    let deployment = first_install().await;
    let file = deployment.directory.path().join("first.cfg");
    std::fs::write(&file, FIRST).unwrap();
    boss(&deployment, &["apply", "-f", file.to_str().unwrap()]).await;
    for args in [
        vec!["get", "charge"],
        vec!["set", "charge", "operation:G", "turn_timeout=90"],
        vec!["delete", "charge", "operation:G"],
    ] {
        let ran = ai6ctl(&deployment, &deployment.guest, &args, None, &[]).await;
        assert!(!ran.ok, "{args:?}");
        assert!(
            ran.err.contains("the charge family needs config.")
                && ran.err.contains("config:charge"),
            "{args:?}: {}",
            ran.err
        );
    }
    assert!(charge(&deployment).is_some());
}

#[tokio::test]
async fn the_loadout_families_have_their_own_action_and_a_station_s_own_records() {
    let deployment = first_install().await;
    let first = deployment.directory.path().join("first.cfg");
    std::fs::write(&first, FIRST).unwrap();
    boss(&deployment, &["apply", "-f", first.to_str().unwrap()]).await;
    let loadouts = "[secret.forge]\nenv = \"GH_TOKEN\"\n\n[loadout.007]\nsecrets = [\"forge\"]\n";

    let denied = ai6ctl(
        &deployment,
        &deployment.guest,
        &["apply", "-f", "-"],
        Some(loadouts),
        &[],
    )
    .await;
    assert!(
        !denied.ok
            && denied
                .err
                .contains("the secret family needs config.manage on config:secret"),
        "{}",
        denied.err
    );
    let applied = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "-f", "-"],
        Some(loadouts),
        &[],
    )
    .await;
    assert!(applied.ok, "{}", applied.err);
    assert!(
        applied.out.contains("secret/forge created") && applied.out.contains("loadout/007 created")
    );
    let kinds = boss(&deployment, &["api-resources"]).await;
    for family in [
        "theater",
        "loadout",
        "sandbox",
        "mcp",
        "overlay",
        "secret",
        "secret_provider",
        "pool",
    ] {
        assert!(
            kinds.contains(&format!(
                "{family}\t{family}s\tconfig.read\tconfig.manage\tconfig:{family}\ttrue"
            )) || kinds.contains(&format!(
                "{family}\t{family}es\tconfig.read\tconfig.manage\tconfig:{family}\ttrue"
            )),
            "{family}: {kinds}"
        );
    }

    // The loadout has a form: a text field takes a quoted value as the text inside the quotes,
    // and a codename stays text.
    boss(
        &deployment,
        &[
            "set",
            "loadout",
            "007",
            "env.PATH=/opt/bin",
            "env.LEVEL=\"3\"",
            "env.PLAIN=3",
            "env.CODE=007",
        ],
    )
    .await;
    let shown: Value =
        serde_json::from_str(&boss(&deployment, &["get", "loadouts", "007", "-o", "json"]).await)
            .unwrap();
    assert_eq!(
        shown["loadout"]["007"]["env"],
        json!({"PATH": "/opt/bin", "LEVEL": "3", "PLAIN": "3", "CODE": "007"})
    );
    assert_eq!(shown["loadout"]["007"]["secrets"], json!(["forge"]));

    // A secret is named only by the records that take its name, and a Station keeps its own.
    let named = ai6ctl(
        &deployment,
        &deployment.boss,
        &["delete", "secret", "forge"],
        None,
        &[],
    )
    .await;
    assert!(
        !named.ok && named.err.contains("loadout 007"),
        "{}",
        named.err
    );
    let own = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "--station", "s1", "-f", "-"],
        Some("[loadout.007]\nsecrets = [\"forge\"]\nenv = { MODE = \"s1\" }\n"),
        &[],
    )
    .await;
    assert!(own.ok, "{}", own.err);
    assert_eq!(
        boss(
            &deployment,
            &["get", "loadout", "007", "env.MODE", "--station", "s1"]
        )
        .await,
        "s1\n"
    );
    let global = boss(&deployment, &["get", "loadout", "007", "-o", "json"]).await;
    assert!(
        !global.contains("MODE"),
        "the global record is untouched: {global}"
    );
    let elsewhere = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "--station", "s1", "-f", "-"],
        Some("[agent.009]\nhandler = \"mock-agent\"\n"),
        &[],
    )
    .await;
    assert!(
        !elsewhere.ok && elsewhere.err.contains("no records of a Station's own"),
        "{}",
        elsewhere.err
    );
}

#[tokio::test]
async fn the_example_configuration_s_documents_load_as_they_are_written() {
    let deployment = first_install().await;
    let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/config");
    let (agents, operations, theaters) = (
        examples.join("agents.toml"),
        examples.join("operations.toml"),
        examples.join("theaters.yaml"),
    );
    let applied = boss(
        &deployment,
        &[
            "apply",
            "-f",
            agents.to_str().unwrap(),
            "-f",
            operations.to_str().unwrap(),
            "-f",
            theaters.to_str().unwrap(),
        ],
    )
    .await;
    assert!(applied.contains("agent/007 created"), "{applied}");
    assert!(applied.contains("operation/G created"), "{applied}");
    let from_files = ai6::FileProvider::new(&examples).load("").unwrap();
    let stored = deployment.station.load("").unwrap();
    assert_eq!(
        stored.roster.agent(Some("G"), "007").unwrap().handler,
        from_files.roster.agent(Some("G"), "007").unwrap().handler
    );
    let shown = boss(&deployment, &["get", "agent", "007"]).await;
    assert!(
        shown.contains("[agent.007]") && shown.contains("handler = \"claude\""),
        "{shown}"
    );
}

#[tokio::test]
async fn a_trust_root_record_that_loosens_waits_for_the_person_to_confirm() {
    let deployment = first_install().await;
    let stored = |record: ConfigRecord| {
        deployment
            .station
            .definitions(record, &ConfigScope::Global)
            .unwrap()
            .records
    };
    let loose = "[cables]\ncross_operation = \"station_grants\"\n";

    let denied = ai6ctl(
        &deployment,
        &deployment.guest,
        &["apply", "-y", "-f", "-"],
        Some(loose),
        &[],
    )
    .await;
    assert!(
        !denied.ok
            && denied
                .err
                .contains("the cables family needs config.manage on config:cables"),
        "{}",
        denied.err
    );

    // Held back: the weakening is printed and nothing is stored, and no one is asked on a pipe.
    let held = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "-f", "-"],
        Some(loose),
        &[],
    )
    .await;
    assert!(!held.ok && held.err.contains("--yes"), "{}", held.err);
    assert!(
        held.out.contains("weakening\t") && held.out.contains("station_grants"),
        "{}",
        held.out
    );
    assert!(stored(ConfigRecord::Cables).is_empty());
    assert!(audits(&deployment, "roster.update").is_empty());

    let confirmed = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "-y", "-f", "-"],
        Some(loose),
        &[],
    )
    .await;
    assert!(confirmed.ok, "{}", confirmed.err);
    assert!(
        confirmed.out.contains("cables/settings created"),
        "{}",
        confirmed.out
    );
    assert!(
        confirmed.out.contains("weakening\t"),
        "the weakening is still reported: {}",
        confirmed.out
    );
    assert_eq!(stored(ConfigRecord::Cables).len(), 1);
    assert_eq!(audits(&deployment, "roster.update").len(), 1);

    // A no-op and a tightening apply at once.
    let same = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "-f", "-"],
        Some(loose),
        &[],
    )
    .await;
    assert!(
        same.ok && same.out.contains("cables/settings unchanged"),
        "{}{}",
        same.out,
        same.err
    );
    let tight = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "-f", "-"],
        Some("[cables]\n"),
        &[],
    )
    .await;
    assert!(
        tight.ok && tight.out.contains("cables/settings changed"),
        "{}{}",
        tight.out,
        tight.err
    );
    assert!(!tight.out.contains("weakening"));

    // `set` is held the same way: a SHA-1 pin loosens, a SHA-256 pin does not.
    let sha1 = format!("pin=sha1:{}", "0".repeat(40));
    let held = ai6ctl(
        &deployment,
        &deployment.boss,
        &["set", "plugin", "a.so", &sha1],
        None,
        &[],
    )
    .await;
    assert!(!held.ok && held.err.contains("--yes"), "{}", held.err);
    assert!(stored(ConfigRecord::Plugin).is_empty());
    let pinned = format!("pin=sha256:{}", "0".repeat(64));
    boss(&deployment, &["set", "plugin", "b.so", &pinned]).await;
    assert_eq!(stored(ConfigRecord::Plugin).len(), 1);
    boss(&deployment, &["set", "plugin", "a.so", &sha1, "-y"]).await;
    assert_eq!(stored(ConfigRecord::Plugin).len(), 2);
}

#[tokio::test]
async fn adding_a_sign_in_provider_is_stored_without_a_confirmation() {
    let deployment = first_install().await;
    let stored = || {
        deployment
            .station
            .definitions(ConfigRecord::Unex, &ConfigScope::Global)
            .unwrap()
            .records
    };
    let one = "[unex]\nauthenticators = [{ provider = \"coqui_access_system\" }]\n";
    let two = "[unex]\nauthenticators = [{ provider = \"coqui_access_system\" }, { provider = \"coqui_access_key\", config = { prefix = \"ck_\" } }]\n";
    for document in [one, two] {
        // No --yes: nothing is loosened, so nothing waits to be confirmed.
        let applied = ai6ctl(
            &deployment,
            &deployment.boss,
            &["apply", "-f", "-"],
            Some(document),
            &[],
        )
        .await;
        assert!(applied.ok, "{}{}", applied.out, applied.err);
        assert!(!applied.out.contains("weakening"), "{}", applied.out);
    }
    let records = stored();
    assert_eq!(records.len(), 1);
    let providers: Vec<&str> = records[0].1["authenticators"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["provider"].as_str().unwrap())
        .collect();
    assert_eq!(providers, ["coqui_access_system", "coqui_access_key"]);
    assert_eq!(audits(&deployment, "roster.update").len(), 2);

    // The old single field is not read.
    let old = "[unex]\nauthenticator = \"coqui_access_system\"\n";
    let refused = ai6ctl(
        &deployment,
        &deployment.boss,
        &["apply", "-y", "-f", "-"],
        Some(old),
        &[],
    )
    .await;
    assert!(
        !refused.ok && refused.err.contains("authenticator"),
        "{}",
        refused.err
    );
}
