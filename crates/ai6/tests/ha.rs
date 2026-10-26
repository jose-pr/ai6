//! Two Stations sharing one PostgreSQL database: they elect one Headquarters, read one
//! roster, and the survivor takes over when the Headquarters goes dark.
//!
//! Runs when `AI6_TEST_POSTGRES_URL` names a `postgres://` server whose user may create
//! schemas; otherwise it says why it skipped and passes.

use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use ai6::directorates::m::config::{ConfigRecord, SqlConfigSource};
use ai6::directorates::registry::sql::SqlRegistry;
use ai6::{GONE_DARK_AFTER, HeadquartersLease, StationStore};
use serde_json::json;

const POSTGRES_URL_ENV: &str = "AI6_TEST_POSTGRES_URL";
/// Short, so the takeover comes quickly, and still several scheduler ticks long.
const HEADQUARTERS_LEASE_SECS: &str = "3";
/// Generous: two Stations start and settle on a slow CI runner.
const PATIENCE: Duration = Duration::from_secs(120);

/// A schema of its own on the test server, dropped afterwards.
struct Schema {
    server: String,
    name: String,
}

impl Schema {
    fn new(server: &str) -> Self {
        let name = format!("ai6_ha_{}", uuid::Uuid::new_v4().simple());
        postgres::Client::connect(server, postgres::NoTls)
            .and_then(|mut admin| admin.batch_execute(&format!("CREATE SCHEMA {name}")))
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
        if let Ok(mut admin) = postgres::Client::connect(&self.server, postgres::NoTls) {
            let _ = admin.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.name));
        }
    }
}

/// A running `ai6 serve`, stopped when dropped.
struct Station {
    name: &'static str,
    process: Child,
}

impl Station {
    fn start(name: &'static str, location: &str, root: &Path) -> Self {
        let state = root.join(name);
        let config_dir = root.join("config");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();
        let log = File::create(root.join(format!("{name}.log"))).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_ai6"));
        command
            .args(["serve", "--listen", "127.0.0.1:0", "--role", "control"])
            .args([
                "--station",
                name,
                "--registry",
                location,
                "--config",
                location,
            ])
            .arg("--config-dir")
            .arg(&config_dir)
            .args(["--headquarters-lease", HEADQUARTERS_LEASE_SECS])
            .env("AI6_STATE", &state)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        for inherited in [
            "AI6_REGISTRY",
            "AI6_HOME",
            "AI6_CONFIG_DIR",
            "AI6_STATION_ID",
            "AI6_EXPORTS_URL",
            "AI6_EXPORTS_STATION_TOKEN",
        ] {
            command.env_remove(inherited);
        }
        Self {
            name,
            process: command.spawn().expect("ai6 serve starts"),
        }
    }

    /// Stops the Station as a crash would: no shutdown, no last write.
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

/// Waits for `ready`, failing with the Stations' logs after [`PATIENCE`].
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

fn live_headquarters(registry: &SqlRegistry) -> Option<HeadquartersLease> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    registry
        .headquarters()
        .unwrap()
        .filter(|lease| lease.expires() >= now)
}

#[test]
fn two_stations_on_one_database_elect_one_headquarters_share_the_roster_and_fail_over() {
    let Some(server) = std::env::var(POSTGRES_URL_ENV)
        .ok()
        .filter(|url| !url.trim().is_empty())
    else {
        eprintln!("skipped the two-Station PostgreSQL test: {POSTGRES_URL_ENV} is not set");
        return;
    };
    let schema = Schema::new(&server);
    let location = schema.location();
    let root = tempfile::tempdir().unwrap();

    // The roster lives only in the database.
    let roster = SqlConfigSource::open(&location).unwrap();
    roster
        .put(
            ConfigRecord::Operation,
            "G",
            &json!({"description": "Shared through PostgreSQL", "roster": ["007"]}),
        )
        .unwrap();
    roster
        .put(
            ConfigRecord::Role,
            "case-officer",
            &json!({"purpose": "Runs a Mission end to end.", "policy": "Report to M."}),
        )
        .unwrap();
    roster
        .put(
            ConfigRecord::Agent,
            "007",
            &json!({"handler": "mock-agent", "roles": ["case-officer"]}),
        )
        .unwrap();

    let mut stations = [
        Station::start("station-a", &location, root.path()),
        Station::start("station-b", &location, root.path()),
    ];
    let registry = SqlRegistry::open(&location).unwrap();

    // Both Stations check in with the roster they read from the database.
    until(
        root.path(),
        "both Stations to check in with the shared roster",
        || {
            let records = registry.stations(GONE_DARK_AFTER).unwrap();
            let shared = |name: &str| {
                records.iter().any(|record| {
                    record.station() == name
                        && record.info()["agent_roles"][0]["id"] == "case-officer"
                        && record.info()["operation_policies"].get("G").is_some()
                })
            };
            (shared("station-a") && shared("station-b")).then_some(())
        },
    );

    // One of them holds the Headquarters lease, and keeps it while it renews.
    let elected = until(root.path(), "a Headquarters", || {
        live_headquarters(&registry)
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let current = until(root.path(), "the live Headquarters lease", || {
            live_headquarters(&registry)
        });
        assert_eq!(
            (current.station(), current.term()),
            (elected.station(), elected.term()),
            "one Headquarters while it renews"
        );
        thread::sleep(Duration::from_millis(250));
    }

    // The Headquarters goes dark; the other Station takes over in the next term.
    let (dark, survivor) = if elected.station() == "station-a" {
        (0, 1)
    } else {
        (1, 0)
    };
    stations[dark].kill();
    let taken = until(root.path(), "the survivor to take over", || {
        live_headquarters(&registry).filter(|lease| lease.station() == stations[survivor].name)
    });
    assert!(taken.term() > elected.term());
}
