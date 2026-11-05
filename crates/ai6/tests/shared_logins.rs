//! Two real `ai6 serve` Stations sharing a login sealed in their Registry: the Headquarters
//! Station's round renews it through a real command, a fake harness that rotates its login, one
//! Station at a time; when that Station goes dark the round carries on at the other; and nothing a
//! Station wrote, on disk or in its log, holds the login.
//!
//! SQLite always runs; PostgreSQL when `AI6_TEST_POSTGRES_URL` names a server whose user may create
//! schemas. The file Registry and every other case are in the library's own suite.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ai6::directorates::m::config::{ConfigProvider, ConfigRecord, ConfigScope, SqlProvider};
use ai6::directorates::q::logins::add_login;
use ai6::directorates::q::sealing::{Recipient, SealingKey, Vault};
use ai6::directorates::registry::sql::SqlRegistry;
use ai6::{AccountRef, LoginState, RegistryBackend};
use serde_json::json;

const POSTGRES_URL_ENV: &str = "AI6_TEST_POSTGRES_URL";
const PROVIDER_ENV: &str = "AI6_FAKE_HARNESS_PROVIDER";
const FILE: &str = ".fake/auth.json";
/// In the login file only: it must reach nothing else.
const PLANTED: &str = "planted-refresh-0451";
const PATIENCE: Duration = Duration::from_secs(120);

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// The provider a fake harness refreshes through: one good refresh token, rotated at each refresh.
fn provider_state(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn set_provider(path: &Path, refresh: &str, refreshes: u64) {
    std::fs::write(
        path,
        json!({"refresh": refresh, "refreshes": refreshes, "refused": 0}).to_string(),
    )
    .unwrap();
}

/// The fake harness: this test binary, run again by a Station as an account's `renew_command`. A
/// refused refresh removes its file and exits with success, as Grok 1.0.70 does.
#[test]
fn fake_harness() {
    let Some(provider) = std::env::var_os(PROVIDER_ENV) else {
        return;
    };
    let provider = PathBuf::from(provider);
    let path = PathBuf::from(std::env::var_os("HOME").expect("a home")).join(FILE);
    let held: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut state = provider_state(&provider);
    if held["refresh_token"] != state["refresh"] {
        state["refused"] = json!(state["refused"].as_u64().unwrap() + 1);
        std::fs::write(&provider, state.to_string()).unwrap();
        std::fs::remove_file(&path).unwrap();
        return;
    }
    let count = state["refreshes"].as_u64().unwrap() + 1;
    let refresh = format!("rotated-{count}");
    state["refreshes"] = json!(count);
    state["refresh"] = json!(refresh);
    std::fs::write(&provider, state.to_string()).unwrap();
    std::fs::write(
        &path,
        json!({
            "access_token": format!("access-{count}"),
            "refresh_token": refresh,
            "expires_at": now() + 86_400.0,
        })
        .to_string(),
    )
    .unwrap();
}

/// A running `ai6 serve`, stopped when dropped.
struct Station {
    name: &'static str,
    process: Child,
}

impl Station {
    fn start(name: &'static str, location: &str, root: &Path) -> Self {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let bootstrap = dir.join("ai6.cfg");
        std::fs::write(
            &bootstrap,
            format!(
                "[station]\nid = '{name}'\nstate_dir = '.'\n[provider]\nlocation = '{location}'\n\
                 poll = 1\n[registry]\nheadquarters_lease = 3\n"
            ),
        )
        .unwrap();
        let log = File::create(root.join(format!("{name}.log"))).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_ai6"));
        command
            .args(["serve", "--listen", "127.0.0.1:0"])
            .arg("--config")
            .arg(&bootstrap)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        command.env_remove("AI6_CONFIG");
        Self {
            name,
            process: command.spawn().expect("ai6 serve starts"),
        }
    }

    fn kill(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Drop for Station {
    fn drop(&mut self) {
        self.kill();
    }
}

fn until<T>(root: &Path, what: &str, mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(value) = ready() {
            return value;
        }
        if Instant::now() > deadline {
            let logs = ["station-a", "station-b"]
                .map(|name| {
                    let log = std::fs::read_to_string(root.join(format!("{name}.log")));
                    format!("--- {name}\n{}", log.unwrap_or_default())
                })
                .join("\n");
            panic!("timed out waiting for {what}\n{logs}");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn account() -> AccountRef {
    AccountRef::new("fake", "shared")
}

fn login_of(registry: &dyn RegistryBackend) -> Option<LoginState> {
    registry
        .accounts()
        .unwrap()
        .into_iter()
        .find(|state| state.account == account())
        .and_then(|state| state.login)
}

fn seed(seeder: &Vault, root: &Path, refresh: &str) {
    let dir = root.join("logged-in");
    std::fs::create_dir_all(dir.join(".fake")).unwrap();
    std::fs::write(
        dir.join(FILE),
        json!({"access_token": "access-seed", "refresh_token": refresh, "expires_at": now() + 600.0})
            .to_string(),
    )
    .unwrap();
    add_login(
        seeder,
        &account(),
        &dir,
        &[FILE.to_owned()],
        &[Recipient::named("station-a"), Recipient::named("station-b")],
        now(),
    )
    .unwrap();
}

/// Every file under `dir`, as text as far as it reads.
fn all_text(dir: &Path) -> String {
    let mut text = String::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                text.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
    }
    text
}

fn a_login_renews_on_one_station_then_on_the_other(location: &str) {
    let root = tempfile::tempdir().unwrap();
    let provider_file = root.path().join("provider.json");
    let harness = std::env::current_exe().unwrap();
    let config = SqlProvider::open(location).unwrap();
    config
        .put(
            ConfigRecord::Station,
            &ConfigScope::Global,
            ConfigRecord::SINGLETON_KEY,
            &json!({
                "roles": ["control"],
                "scheduler": {"check_in": 1, "interval": 1},
                "accounts": {"renew_every": 1, "renew_within": 3600, "renew_lease": 30},
            }),
        )
        .unwrap();
    config
        .put(
            ConfigRecord::Pool,
            &ConfigScope::Global,
            "fake",
            &json!({
                "handler": "mock-agent",
                "account": {"shared": {
                    "kind": "harness-login",
                    "files": [FILE],
                    "renew_command": [
                        harness.to_str().unwrap(), "--exact", "fake_harness", "--test-threads=1",
                    ],
                    "renew_env": {PROVIDER_ENV: provider_file.to_str().unwrap()},
                }},
            }),
        )
        .unwrap();

    let mut stations = [
        Station::start("station-a", location, root.path()),
        Station::start("station-b", location, root.path()),
    ];
    let registry: Arc<dyn RegistryBackend> = Arc::new(SqlRegistry::open(location).unwrap());
    until(root.path(), "both Stations' sealing keys", || {
        (registry.sealing_key("station-a").unwrap().is_some()
            && registry.sealing_key("station-b").unwrap().is_some())
        .then_some(())
    });
    let published = registry.sealing_key("station-a").unwrap().unwrap();
    assert!(published.signature.is_some(), "its identity vouches for it");
    let elected = until(root.path(), "a Headquarters", || {
        registry
            .headquarters()
            .unwrap()
            .filter(|lease| lease.expires() >= now())
    });

    // A person's login on a host, near its end, sealed to both.
    let seeder = Vault::new(
        registry.clone(),
        "a-person-on-a-host",
        Arc::new(SealingKey::generate()),
    );
    set_provider(&provider_file, PLANTED, 0);
    seed(&seeder, root.path(), PLANTED);
    let first = until(root.path(), "the round to renew the login", || {
        login_of(registry.as_ref()).filter(|login| login.revision >= 2 && login.lease.is_none())
    });
    assert_eq!(provider_state(&provider_file)["refreshes"], 1);
    assert_eq!(provider_state(&provider_file)["refused"], 0);
    assert!(first.expires_at.unwrap() > now() + 3_600.0);
    assert_eq!(first.needs_person, None);

    // The Headquarters Station goes dark; a login near its end again is the survivor's to renew.
    let (dark, survivor) = if elected.station() == "station-a" {
        (0, 1)
    } else {
        (1, 0)
    };
    stations[dark].kill();
    let taken = until(root.path(), "the survivor to take over", || {
        registry
            .headquarters()
            .unwrap()
            .filter(|lease| lease.expires() >= now() && lease.station() == stations[survivor].name)
    });
    assert!(taken.term() > elected.term());
    let before = registry
        .sealed_value(&format!("login/{}/{}", account().pool, account().account))
        .unwrap()
        .unwrap()
        .revision;
    set_provider(&provider_file, "second-login", 1);
    seed(&seeder, root.path(), "second-login");
    until(root.path(), "the round to carry on at the survivor", || {
        login_of(registry.as_ref())
            .filter(|login| login.revision >= before + 2 && login.lease.is_none())
    });
    let state = provider_state(&provider_file);
    assert_eq!(state["refreshes"], 2);
    assert_eq!(state["refused"], 0);
    stations[survivor].kill();

    // Nothing either Station left, its log and state directory, holds what was sealed.
    for name in ["station-a", "station-b"] {
        let dir = root.path().join(name);
        assert!(dir.join("sealing.key").is_file());
        let log = std::fs::read_to_string(root.path().join(format!("{name}.log"))).unwrap();
        let text = all_text(&dir) + &log;
        for secret in [
            PLANTED,
            "second-login",
            "rotated-1",
            "rotated-2",
            "access-seed",
            "access-1",
        ] {
            assert!(!text.contains(secret), "{secret} reached {name}: {log}");
        }
        let scratch = dir.join("accounts");
        let left: Vec<_> = std::fs::read_dir(&scratch)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        assert!(left.is_empty(), "a throwaway home was left: {left:?}");
    }
    let dumped =
        serde_json::to_string(&registry.sealed_value("login/fake/shared").unwrap()).unwrap();
    for secret in [PLANTED, "second-login", "rotated-1", "rotated-2"] {
        assert!(!dumped.contains(secret), "{secret} is in the record");
    }
    let limits = registry.limits();
    assert!(limits.accounts.sealed_bytes.allows(dumped.len()));
}

#[test]
fn a_login_renews_on_one_station_then_on_the_other_on_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let location = format!("sqlite:{}", dir.path().join("ai6.db").display());
    a_login_renews_on_one_station_then_on_the_other(&location);
    let database = all_text(dir.path());
    assert!(
        database.contains("login/fake/shared"),
        "the record is there"
    );
    for secret in [PLANTED, "second-login", "rotated-1", "rotated-2"] {
        assert!(
            !database.contains(secret),
            "{secret} is in the database file"
        );
    }
}

#[test]
fn a_login_renews_on_one_station_then_on_the_other_on_postgres() {
    let Some(server) = std::env::var(POSTGRES_URL_ENV)
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("skipped the shared login PostgreSQL test: {POSTGRES_URL_ENV} is not set");
        return;
    };
    let name = format!("ai6_logins_{}", uuid::Uuid::new_v4().simple());
    let admin = |sql: String| {
        let server = server.clone();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let (client, connection) = tokio_postgres::connect(&server, tokio_postgres::NoTls)
                    .await
                    .unwrap();
                let driver = tokio::spawn(connection);
                client.batch_execute(&sql).await.unwrap();
                drop(client);
                let _ = driver.await;
            });
    };
    admin(format!("CREATE SCHEMA {name}"));
    let separator = if server.contains('?') { '&' } else { '?' };
    let location = format!("{server}{separator}options=-c%20search_path%3D{name}");
    a_login_renews_on_one_station_then_on_the_other(&location);
    admin(format!("DROP SCHEMA {name} CASCADE"));
}
