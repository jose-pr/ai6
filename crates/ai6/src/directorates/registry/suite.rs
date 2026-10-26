//! The Registry test suite's backends: each case is written once against [`Store`] and
//! [`backend_tests!`] runs it on the file Registry, on SQLite (feature `sqlite`) and on
//! PostgreSQL (feature `postgres`, when `AI6_TEST_POSTGRES_URL` names a server).
//!
//! A PostgreSQL case runs in a schema of its own, dropped afterwards. Without the variable
//! the cases pass without running, and `postgres_cases_skipped` in each test module prints
//! how many were skipped and why (`cargo test -- --nocapture` shows it).

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::backend::RegistryBackend;
use super::sessions::{NewSession, SessionError, SessionRecord, SessionUpdate};
use super::store::{Brief, FileRegistry, Lease};

/// Names the PostgreSQL server the suite runs against: a `postgres://` URL whose user may
/// create schemas in its database.
pub(crate) const POSTGRES_URL_ENV: &str = "AI6_TEST_POSTGRES_URL";

/// A Registry with the clock-explicit Session hooks the Session cases drive.
pub(crate) trait TestRegistry: RegistryBackend {
    fn open_session_at(
        &self,
        new: NewSession,
        ttl: Duration,
        now: f64,
    ) -> Result<SessionRecord, SessionError>;

    fn take_session_at(
        &self,
        id: &str,
        station: &str,
        ttl: Duration,
        now: f64,
    ) -> Result<SessionRecord, SessionError>;

    fn write_fenced(
        &self,
        id: &str,
        station: &str,
        fence: u64,
        now: f64,
        renew: Option<Duration>,
        update: SessionUpdate,
    ) -> Result<SessionRecord, SessionError>;

    fn lose_session_at(
        &self,
        id: &str,
        reason: &str,
        now: f64,
    ) -> Result<SessionRecord, SessionError>;
}

macro_rules! clocked_sessions {
    ($registry:ty) => {
        impl TestRegistry for $registry {
            fn open_session_at(
                &self,
                new: NewSession,
                ttl: Duration,
                now: f64,
            ) -> Result<SessionRecord, SessionError> {
                <$registry>::open_session_at(self, new, ttl, now)
            }

            fn take_session_at(
                &self,
                id: &str,
                station: &str,
                ttl: Duration,
                now: f64,
            ) -> Result<SessionRecord, SessionError> {
                <$registry>::take_session_at(self, id, station, ttl, now)
            }

            fn write_fenced(
                &self,
                id: &str,
                station: &str,
                fence: u64,
                now: f64,
                renew: Option<Duration>,
                update: SessionUpdate,
            ) -> Result<SessionRecord, SessionError> {
                <$registry>::write_fenced(self, id, station, fence, now, renew, update)
            }

            fn lose_session_at(
                &self,
                id: &str,
                reason: &str,
                now: f64,
            ) -> Result<SessionRecord, SessionError> {
                <$registry>::lose_session_at(self, id, reason, now)
            }
        }
    };
}

clocked_sessions!(FileRegistry);
#[cfg(any(feature = "sqlite", feature = "postgres"))]
clocked_sessions!(super::sql::SqlRegistry);

/// One test's Registry.
pub(crate) trait Store {
    /// A new handle on the Registry, as another Station would open it.
    fn handle(&self) -> Arc<dyn TestRegistry>;
}

/// A file Registry in a temporary directory.
pub(crate) struct FileStore(tempfile::TempDir);

impl FileStore {
    pub(crate) fn new() -> Self {
        Self(tempfile::tempdir().expect("a temporary directory"))
    }
}

impl Store for FileStore {
    fn handle(&self) -> Arc<dyn TestRegistry> {
        Arc::new(FileRegistry::open(self.0.path()))
    }
}

/// A SQLite Registry in a temporary directory.
#[cfg(feature = "sqlite")]
pub(crate) struct SqliteStore(tempfile::TempDir);

#[cfg(feature = "sqlite")]
impl SqliteStore {
    pub(crate) fn new() -> Self {
        Self(tempfile::tempdir().expect("a temporary directory"))
    }

    pub(crate) fn location(&self) -> String {
        format!("sqlite:{}", self.0.path().join("registry.db").display())
    }
}

#[cfg(feature = "sqlite")]
impl Store for SqliteStore {
    fn handle(&self) -> Arc<dyn TestRegistry> {
        Arc::new(super::sql::SqlRegistry::open(&self.location()).expect("a SQLite Registry"))
    }
}

/// A PostgreSQL Registry in a schema of its own on the server `AI6_TEST_POSTGRES_URL`
/// names; the schema is dropped with the store.
#[cfg(feature = "postgres")]
pub(crate) struct PostgresStore {
    server: String,
    schema: String,
}

#[cfg(feature = "postgres")]
impl PostgresStore {
    /// A new schema, or `None` when no server is named.
    pub(crate) fn new() -> Option<Self> {
        let server = postgres_server()?;
        let schema = format!("ai6_test_{}", uuid::Uuid::new_v4().simple());
        super::sql::postgres_admin(&server, &format!("CREATE SCHEMA {schema}"))
            .unwrap_or_else(|error| panic!("{POSTGRES_URL_ENV} cannot be reached: {error}"));
        Some(Self { server, schema })
    }

    /// The location of the Registry: the server, with this store's schema first on the
    /// search path.
    pub(crate) fn location(&self) -> String {
        self.location_with(&[])
    }

    /// [`location`](Self::location) with more server settings for each session.
    pub(crate) fn location_with(&self, settings: &[(&str, &str)]) -> String {
        let separator = if self.server.contains('?') { '&' } else { '?' };
        let mut options = format!("-c%20search_path%3D{}", self.schema);
        for (name, value) in settings {
            options.push_str(&format!(
                "%20-c%20{name}%3D{}",
                value.replace(' ', "%5C%20")
            ));
        }
        format!("{}{separator}options={options}", self.server)
    }
}

#[cfg(feature = "postgres")]
impl Store for PostgresStore {
    fn handle(&self) -> Arc<dyn TestRegistry> {
        Arc::new(super::sql::SqlRegistry::open(&self.location()).expect("a PostgreSQL Registry"))
    }
}

#[cfg(feature = "postgres")]
impl Drop for PostgresStore {
    fn drop(&mut self) {
        let _ = super::sql::postgres_admin(
            &self.server,
            &format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema),
        );
    }
}

/// The server `AI6_TEST_POSTGRES_URL` names, if any.
#[cfg(feature = "postgres")]
pub(crate) fn postgres_server() -> Option<String> {
    std::env::var(POSTGRES_URL_ENV)
        .ok()
        .filter(|url| !url.trim().is_empty())
}

/// Prints how many PostgreSQL cases of `module` did not run, when none could.
#[cfg(feature = "postgres")]
pub(crate) fn report_skips(module: &str, cases: usize) {
    if postgres_server().is_none() {
        eprintln!(
            "skipped {cases} PostgreSQL Registry cases in {module}: {POSTGRES_URL_ENV} is not set"
        );
    }
}

/// A brief with this ID from the `manual` source: its key is `manual--ID`.
pub(crate) fn brief(id: &str) -> Brief {
    serde_json::from_value(serde_json::json!({"title": "Inspect", "id": id})).expect("a brief")
}

/// Lets the lease on `key` lapse: its holder renews it for a nanosecond, then the backend's
/// own clock passes that. SQL backends judge expiry by the database's clock, which SQLite
/// keeps in milliseconds, so the test process's clock is not the one to wait on.
pub(crate) fn lapse(registry: &dyn TestRegistry, key: &str, station: &str) -> Lease {
    assert!(
        registry
            .renew(key, station, Duration::from_nanos(1))
            .unwrap()
    );
    until(|| {
        registry
            .expired()
            .unwrap()
            .into_iter()
            .find(|lease| lease.key() == key)
    })
}

/// Waits for `ready` to return a value; generous, since CI clocks and disks are slow.
pub(crate) fn until<T>(mut ready: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(value) = ready() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting");
        thread::sleep(Duration::from_millis(2));
    }
}

/// Runs each case, a function taking `&dyn Store`, once per Registry backend.
macro_rules! backend_tests {
    ($($case:ident),+ $(,)?) => {
        mod on_file {
            $(
                #[test]
                fn $case() {
                    super::$case(&$crate::directorates::registry::suite::FileStore::new());
                }
            )+
        }

        #[cfg(feature = "sqlite")]
        mod on_sqlite {
            $(
                #[test]
                fn $case() {
                    super::$case(&$crate::directorates::registry::suite::SqliteStore::new());
                }
            )+
        }

        #[cfg(feature = "postgres")]
        mod on_postgres {
            $(
                #[test]
                fn $case() {
                    if let Some(store) = $crate::directorates::registry::suite::PostgresStore::new() {
                        super::$case(&store);
                    }
                }
            )+

            #[test]
            fn postgres_cases_skipped() {
                $crate::directorates::registry::suite::report_skips(
                    module_path!(),
                    [$(stringify!($case)),+].len(),
                );
            }
        }
    };
}

pub(crate) use backend_tests;
