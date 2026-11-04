//! `ai6 config rm` on every kind of configuration provider: a directory of files, SQLite and,
//! when `AI6_TEST_POSTGRES_URL` names a server whose user may create schemas, PostgreSQL.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use ai6::directorates::registry::backend::open as open_registry;

const POSTGRES_URL_ENV: &str = "AI6_TEST_POSTGRES_URL";

/// A roster where `forge` is named by a loadout, `007` by another Agent and its loadout, and
/// `spare`, `unused` and the secret `path` by nothing, though the Operation has a `path`.
const AGENTS: &str = "# who works here\n\
    [agent.007]\nhandler = \"mock-agent\"\nroles = [\"case-officer\"]\n\n\
    # the backup\n[agent.008]\nextends = \"007\"\n\n\
    # kept for later\n[agent.spare]\nhandler = \"mock-agent\"\n\n\
    [role.case-officer]\npurpose = \"Runs it.\"\npolicy = \"Report.\"\n\n\
    [role.unused]\npurpose = \"Nothing.\"\npolicy = \"Nothing.\"\n";
const OPERATIONS: &str = "[operation.G]\nroster = [\"007\", \"008\"]\npath = \".\"\n";
const LOADOUTS: &str = "[loadout.007]\nsecrets = [\"forge\"]\n\n[secret.forge]\nenv = \"GH_TOKEN\"\n\n\
     [secret.path]\nenv = \"DEPLOY_KEY\"\n";

fn ai6() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ai6"));
    command.env_remove("AI6_CONFIG");
    command
}

/// A configuration provider of one kind, with the roster above in it.
struct Deployment {
    file: PathBuf,
    registry: String,
    _root: tempfile::TempDir,
}

impl Deployment {
    /// With its provider at `location`; relative is under the root. `None`: the root's own files.
    fn new(location: Option<String>, registry: Option<String>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let state = dir.join("state");
        let quoted = |path: &Path| path.display().to_string().replace('\\', "/");
        let (provider, seeded) = match &location {
            Some(location) => (location.clone(), dir.join("seed")),
            None => (quoted(&dir.join("config")), dir.join("config")),
        };
        std::fs::create_dir_all(&seeded).unwrap();
        std::fs::write(seeded.join("agents.toml"), AGENTS).unwrap();
        std::fs::write(seeded.join("operations.toml"), OPERATIONS).unwrap();
        std::fs::write(seeded.join("loadouts.toml"), LOADOUTS).unwrap();
        let registry = registry.unwrap_or_else(|| quoted(&dir.join("registry")));
        let file = dir.join("ai6.cfg");
        std::fs::write(
            &file,
            format!(
                "[provider]\nlocation = \"{provider}\"\n[registry]\nlocation = \"{registry}\"\n\
                 [station]\nstate_dir = \"{}\"\n",
                quoted(&state)
            ),
        )
        .unwrap();
        let deployment = Self {
            file,
            registry,
            _root: root,
        };
        if location.is_some() {
            let imported = deployment.config(&["import", seeded.to_str().unwrap()]);
            assert!(imported.status.success(), "{imported:?}");
        }
        deployment
    }

    fn config(&self, args: &[&str]) -> Output {
        ai6()
            .arg("config")
            .args(args)
            .arg("--config")
            .arg(&self.file)
            .output()
            .unwrap()
    }

    fn has(&self, family: &str, key: &str) -> bool {
        self.config(&["show", family, key]).status.success()
    }

    /// The audit records, newest first, as `action target`.
    fn audited(&self) -> Vec<(String, String)> {
        open_registry(&self.registry)
            .unwrap()
            .audit_records()
            .unwrap()
            .into_iter()
            .map(|record| {
                assert!(record.actor.starts_with("cli:"), "{}", record.actor);
                assert_eq!(record.resource.as_deref(), Some("config:roster"));
                (record.action, record.target)
            })
            .collect()
    }
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// What every provider must do.
fn removes_what_nothing_names_and_refuses_the_rest(deployment: &Deployment) {
    // Nothing names it.
    let removed = deployment.config(&["rm", "agent", "spare"]);
    assert!(removed.status.success(), "{}", said(&removed));
    assert!(
        said(&removed).contains("Removed agent spare"),
        "{}",
        said(&removed)
    );
    assert!(!deployment.has("agent", "spare"));
    assert!(deployment.has("agent", "007"), "only that record goes");
    let audited = deployment.audited();
    assert!(
        audited
            .iter()
            .any(|(action, target)| action == "roster.remove" && target.contains("agent spare")),
        "{audited:?}"
    );

    // A secret is named only where a record takes a secret: an Operation's `path` is not one.
    let removed = deployment.config(&["rm", "secret", "path"]);
    assert!(removed.status.success(), "{}", said(&removed));
    assert!(!deployment.has("secret", "path"));
    assert!(deployment.has("operation", "G"));

    // Named: refused, listing what names it, and nothing goes.
    let refused = deployment.config(&["rm", "secret", "forge"]);
    assert!(!refused.status.success());
    let text = said(&refused);
    assert!(text.contains("loadout 007 (global): secrets"), "{text}");
    assert!(deployment.has("secret", "forge"));
    assert!(
        deployment
            .audited()
            .iter()
            .any(|(action, target)| action == "roster.refused" && target.contains("secret forge")),
        "a refusal is audited as an accepted edit is"
    );

    // --force goes through, says what still names it, and the audit record does too.
    let forced = deployment.config(&["rm", "secret", "forge", "--force"]);
    assert!(forced.status.success(), "{}", said(&forced));
    let text = said(&forced);
    assert!(text.contains("by --force"), "{text}");
    assert!(text.contains("loadout 007 (global): secrets"), "{text}");
    assert!(
        String::from_utf8_lossy(&forced.stderr).contains("weakening"),
        "{text}"
    );
    assert!(!deployment.has("secret", "forge"));
    assert!(
        deployment
            .audited()
            .iter()
            .any(|(action, target)| action == "roster.remove"
                && target.contains("secret forge")
                && target.contains("forced")
                && target.contains("loadout 007")),
        "{:?}",
        deployment.audited()
    );

    // An Agent is named by another's `extends`, an Operation's roster and its own loadout.
    let refused = deployment.config(&["rm", "agent", "007"]);
    assert!(!refused.status.success());
    let text = said(&refused);
    for place in [
        "agent 008 (global): extends",
        "operation G (global): roster",
        "loadout 007 (global)",
    ] {
        assert!(text.contains(place), "{place} in {text}");
    }
    // The provider's own check is not something --force can pass: 008 would extend nobody.
    let refused = deployment.config(&["rm", "agent", "007", "--force"]);
    assert!(!refused.status.success(), "{}", said(&refused));
    assert!(deployment.has("agent", "007"));
    assert!(
        deployment
            .audited()
            .iter()
            .any(|(action, target)| action == "roster.refused" && target.contains("agent 007")),
        "{:?}",
        deployment.audited()
    );

    // Nothing to remove, no family, a bad scope.
    for args in [
        &["rm", "agent", "nobody"][..],
        &["rm", "agents", "007"][..],
        &["rm", "agent", "007", "--scope", "nowhere"][..],
    ] {
        assert!(!deployment.config(args).status.success(), "{args:?}");
    }

    // A role nothing holds.
    assert!(deployment.has("role", "unused"));
    let removed = deployment.config(&["rm", "role", "unused"]);
    assert!(removed.status.success(), "{}", said(&removed));
    assert!(!deployment.has("role", "unused"));
}

#[test]
fn on_a_directory_of_files() {
    let deployment = Deployment::new(None, None);
    removes_what_nothing_names_and_refuses_the_rest(&deployment);
    let dir = deployment.file.parent().unwrap().join("config");
    let agents = std::fs::read_to_string(dir.join("agents.toml")).unwrap();
    assert!(
        !agents.contains("spare") && !agents.contains("unused"),
        "{agents}"
    );
    for kept in [
        "# who works here",
        "# the backup",
        "[agent.007]",
        "[role.case-officer]",
    ] {
        assert!(agents.contains(kept), "{kept} in {agents}");
    }
}

#[test]
fn on_a_directory_a_file_in_another_format_loses_only_its_record() {
    let deployment = Deployment::new(None, None);
    let dir = deployment.file.parent().unwrap().join("config");
    std::fs::remove_file(dir.join("agents.toml")).unwrap();
    std::fs::write(
        dir.join("agents.json"),
        r#"{"agent": {"007": {"handler": "mock-agent"}, "008": {"handler": "mock-agent"},
            "spare": {"handler": "mock-agent"}}}"#,
    )
    .unwrap();
    std::fs::write(dir.join("operations.toml"), "[operation.G]\n").unwrap();
    std::fs::write(
        dir.join("loadouts.yaml"),
        "loadout:\n  \"007\": {}\nsecret:\n  forge:\n    env: GH_TOKEN\n",
    )
    .unwrap();
    std::fs::remove_file(dir.join("loadouts.toml")).unwrap();

    let removed = deployment.config(&["rm", "agent", "spare"]);
    assert!(removed.status.success(), "{}", said(&removed));
    let agents: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("agents.json")).unwrap()).unwrap();
    assert!(agents["agent"].get("spare").is_none(), "{agents}");
    assert!(agents["agent"].get("007").is_some(), "{agents}");

    let removed = deployment.config(&["rm", "secret", "forge"]);
    assert!(removed.status.success(), "{}", said(&removed));
    assert!(!deployment.has("secret", "forge"));
}

#[test]
fn on_a_directory_a_table_written_inline_still_goes() {
    let deployment = Deployment::new(None, None);
    let dir = deployment.file.parent().unwrap().join("config");
    std::fs::write(
        dir.join("agents.toml"),
        "agent.spare = { handler = \"mock-agent\" }\n[agent.007]\nhandler = \"mock-agent\"\n\
         [agent.008]\nhandler = \"mock-agent\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("operations.toml"), "").unwrap();
    std::fs::write(dir.join("loadouts.toml"), "").unwrap();
    let removed = deployment.config(&["rm", "agent", "spare"]);
    assert!(removed.status.success(), "{}", said(&removed));
    assert!(!deployment.has("agent", "spare"));
    assert!(deployment.has("agent", "007"));
}

#[test]
fn on_a_directory_a_family_file_that_cannot_be_read_removes_nothing() {
    let deployment = Deployment::new(None, None);
    let dir = deployment.file.parent().unwrap().join("config");
    std::fs::write(dir.join("theaters.toml"), "this is not = = toml\n").unwrap();
    let before = std::fs::read(dir.join("agents.toml")).unwrap();
    for args in [
        &["rm", "agent", "spare"][..],
        &["rm", "agent", "spare", "--force"][..],
    ] {
        let refused = deployment.config(args);
        assert!(!refused.status.success(), "{}", said(&refused));
        assert!(said(&refused).contains("theaters"), "{}", said(&refused));
    }
    assert_eq!(std::fs::read(dir.join("agents.toml")).unwrap(), before);
}

#[test]
fn on_a_directory_a_station_s_own_record_goes_and_the_global_one_stays() {
    let deployment = Deployment::new(None, None);
    let dir = deployment.file.parent().unwrap().join("config");
    let host = "access = { ssh = { host = \"box.example\" } }\n";
    std::fs::write(dir.join("theaters.toml"), format!("[theater.box]\n{host}")).unwrap();
    let own = dir.join("stations").join("hq");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("theaters.toml"), format!("[theater.box]\n{host}")).unwrap();
    let removed = deployment.config(&["rm", "theater", "box", "--scope", "station:hq"]);
    assert!(removed.status.success(), "{}", said(&removed));
    assert!(
        !std::fs::read_to_string(own.join("theaters.toml"))
            .unwrap()
            .contains("box")
    );
    assert!(deployment.has("theater", "box"));
}

#[test]
fn on_sqlite() {
    let deployment = Deployment::new(Some("sqlite:config.db".to_owned()), None);
    removes_what_nothing_names_and_refuses_the_rest(&deployment);
}

#[test]
fn on_postgresql() {
    let Some(server) = std::env::var(POSTGRES_URL_ENV)
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("skipped the PostgreSQL run: {POSTGRES_URL_ENV} is not set");
        return;
    };
    let schema = Schema::new(&server);
    let deployment = Deployment::new(Some(schema.location()), Some(schema.location()));
    removes_what_nothing_names_and_refuses_the_rest(&deployment);
}

/// A schema of its own on the test server, dropped afterwards.
struct Schema {
    server: String,
    name: String,
}

/// Runs `sql` on the test server, which takes a connection without TLS.
fn admin(server: &str, sql: &str) -> Result<(), tokio_postgres::Error> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    runtime.block_on(async {
        let (client, connection) = tokio_postgres::connect(server, tokio_postgres::NoTls).await?;
        let driver = tokio::spawn(connection);
        let result = client.batch_execute(sql).await;
        drop(client);
        let _ = driver.await;
        result
    })
}

impl Schema {
    fn new(server: &str) -> Self {
        let name = format!("ai6_rm_{}", uuid::Uuid::new_v4().simple());
        admin(server, &format!("CREATE SCHEMA {name}"))
            .unwrap_or_else(|error| panic!("{POSTGRES_URL_ENV}: {error}"));
        Self {
            server: server.to_owned(),
            name,
        }
    }

    fn location(&self) -> String {
        let separator = if self.server.contains('?') { '&' } else { '?' };
        format!(
            "{}{separator}options=-c%20search_path%3D{}",
            self.server, self.name
        )
    }
}

impl Drop for Schema {
    fn drop(&mut self) {
        let _ = admin(
            &self.server,
            &format!("DROP SCHEMA IF EXISTS {} CASCADE", self.name),
        );
    }
}
