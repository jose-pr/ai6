//! The PostgreSQL side of a SQL Registry or roster: what a `postgres://` location asks for,
//! TLS, the session's timeouts, and a client whose connection runs on a runtime of its own.
//!
//! Callers wait on the client with [`block_on`], a parking executor that never enters a
//! Tokio runtime, so a Registry call works on any thread, an async worker included, and
//! dropping a handle never blocks on a runtime.

use std::collections::BTreeSet;
use std::future::Future;
use std::io;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Wake, Waker};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_postgres::config::SslMode;
use tokio_postgres::tls::{ChannelBinding, MakeTlsConnect, TlsConnect, TlsStream};
use tokio_postgres::types::{IsNull, ToSql, Type};

use super::db::{Row, Value};
use super::{SqlOptions, TlsMode, invalid_input, parameters, tls_mode};

/// A `postgres://` location, read.
pub(crate) struct Location {
    config: tokio_postgres::Config,
    tls: TlsMode,
    root_cert: Option<String>,
    /// The location's own `options`: server settings AI6 leaves as given.
    options: String,
}

/// Reads a location, applying `options` where the location sets nothing itself.
pub(crate) fn parse(url: &str, options: &SqlOptions) -> io::Result<Location> {
    let tls = tls_mode(url)?;
    let (base, pairs) = parameters(url)?;
    let mut root_cert = None;
    let mut password_file = None;
    let mut given = BTreeSet::new();
    let mut kept = Vec::new();
    for parameter in pairs {
        match parameter.key.as_str() {
            "sslrootcert" => root_cert = Some(parameter.value),
            "password_file" => password_file = Some(parameter.value),
            "sslmode" => {}
            _ => {
                given.insert(parameter.key);
                kept.push(parameter.written);
            }
        }
    }
    let rebuilt = if kept.is_empty() {
        base.to_owned()
    } else {
        format!("{base}?{}", kept.join("&"))
    };
    let mut config: tokio_postgres::Config = rebuilt
        .parse()
        .map_err(|error| invalid_input(format!("invalid PostgreSQL location: {error}")))?;
    config.ssl_mode(match tls {
        TlsMode::Disable => SslMode::Disable,
        TlsMode::Require | TlsMode::VerifyFull => SslMode::Require,
    });
    if let Some(path) = password_file {
        let secret = zeroize::Zeroizing::new(std::fs::read_to_string(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("password_file {path}: {error}"))
        })?);
        config.password(secret.trim_end_matches(['\r', '\n']));
    } else if config.get_password().is_none()
        && let Some(password) = options.password()
    {
        config.password(password);
    }
    if !given.contains("connect_timeout") {
        config.connect_timeout(options.connect_timeout());
    }
    if !given.contains("keepalives_idle") {
        config.keepalives_idle(options.keepalives_idle());
    }
    if !given.contains("tcp_user_timeout") {
        config.tcp_user_timeout(options.tcp_user_timeout());
    }
    let own_options = config.get_options().unwrap_or_default().to_owned();
    Ok(Location {
        config,
        tls,
        root_cert,
        options: own_options,
    })
}

/// The `SET` statements a new session runs: the server-side timeouts the location's own
/// `options` does not set.
fn session_settings(location: &Location, options: &SqlOptions) -> String {
    [
        ("statement_timeout", options.statement_timeout()),
        ("lock_timeout", options.lock_timeout()),
        (
            "idle_in_transaction_session_timeout",
            options.idle_in_transaction_timeout(),
        ),
    ]
    .into_iter()
    .filter(|(name, _)| !location.options.contains(name))
    .map(|(name, value)| format!("SET {name} = {}; ", value.as_millis()))
    .collect()
}

/// A client on a connection driven by a runtime of its own, reconnected when it closes.
pub(crate) struct Connection {
    config: tokio_postgres::Config,
    tls: Option<Arc<ClientConfig>>,
    settings: String,
    client: Mutex<Option<tokio_postgres::Client>>,
    runtime: tokio::runtime::Handle,
    /// Dropping it ends the runtime's thread.
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl Connection {
    /// Reads `url`, starts the runtime and connects, so a wrong location fails at once.
    pub(crate) fn open(url: &str, options: &SqlOptions) -> io::Result<Self> {
        let location = parse(url, options)?;
        let tls = tls_config(&location)?;
        let settings = session_settings(&location, options);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let handle = runtime.handle().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        std::thread::Builder::new()
            .name("ai6-postgres".to_owned())
            .spawn(move || {
                runtime.block_on(async {
                    let _ = stopped.await;
                });
            })?;
        let connection = Self {
            config: location.config,
            tls,
            settings,
            client: Mutex::new(None),
            runtime: handle,
            _stop: stop,
        };
        let mut slot = connection.lock();
        connection.connected(&mut slot)?;
        drop(slot);
        Ok(connection)
    }

    /// The client, held for one transaction at a time.
    pub(crate) fn lock(&self) -> MutexGuard<'_, Option<tokio_postgres::Client>> {
        self.client.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The live client in `slot`, connecting a new one when there is none or it closed.
    pub(crate) fn connected<'a>(
        &self,
        slot: &'a mut Option<tokio_postgres::Client>,
    ) -> io::Result<&'a mut tokio_postgres::Client> {
        if slot.as_ref().is_some_and(|client| !client.is_closed()) {
            return Ok(slot.as_mut().expect("checked above"));
        }
        *slot = None;
        let config = self.config.clone();
        let task = match &self.tls {
            None => self.runtime.spawn(connect(config, tokio_postgres::NoTls)),
            Some(tls) => self
                .runtime
                .spawn(connect(config, RustlsConnect(tls.clone()))),
        };
        let client = block_on(task).map_err(io::Error::other)??;
        if !self.settings.is_empty() {
            block_on(client.batch_execute(&self.settings)).map_err(error)?;
        }
        Ok(slot.insert(client))
    }
}

async fn connect<T>(config: tokio_postgres::Config, tls: T) -> io::Result<tokio_postgres::Client>
where
    T: MakeTlsConnect<tokio_postgres::Socket> + Send + 'static,
    T::Stream: Send + 'static,
{
    let (client, connection) = config.connect(tls).await.map_err(error)?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

/// Runs `sql` on its own session at `url`; for tests that make and drop schemas.
#[cfg(test)]
pub(crate) fn admin(url: &str, sql: &str) -> io::Result<()> {
    let connection = Connection::open(url, &SqlOptions::default())?;
    let mut slot = connection.lock();
    let client = connection.connected(&mut slot)?;
    block_on(client.batch_execute(sql)).map_err(error)
}

struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `future` to completion on this thread, parking it between polls; the connection's
/// own runtime does the I/O.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::park();
    }
}

/// The rustls client configuration a location's TLS mode needs; `None` without TLS.
fn tls_config(location: &Location) -> io::Result<Option<Arc<ClientConfig>>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?;
    let config = match location.tls {
        TlsMode::Disable => return Ok(None),
        TlsMode::Require => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Unverified(provider)))
            .with_no_client_auth(),
        TlsMode::VerifyFull => {
            let mut roots = RootCertStore::empty();
            match location.root_cert.as_deref() {
                // As libpq: a root certificate file is the only trust, unless it says
                // `system`.
                Some(path) if path != "system" => {
                    for certificate in CertificateDer::pem_file_iter(path)
                        .map_err(|reason| invalid_input(format!("sslrootcert {path}: {reason}")))?
                    {
                        let certificate = certificate.map_err(|reason| {
                            invalid_input(format!("sslrootcert {path}: {reason}"))
                        })?;
                        roots.add(certificate).map_err(|reason| {
                            invalid_input(format!("sslrootcert {path}: {reason}"))
                        })?;
                    }
                }
                _ => {
                    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
                }
            }
            if roots.is_empty() {
                return Err(invalid_input(
                    "sslmode verify-full has no trusted root certificate: set sslrootcert",
                ));
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
    };
    Ok(Some(Arc::new(config)))
}

/// `sslmode=require`: any certificate is accepted, while the handshake signatures are still
/// checked, so the connection is encrypted but the server is not authenticated.
#[derive(Debug)]
struct Unverified(Arc<CryptoProvider>);

impl ServerCertVerifier for Unverified {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            certificate,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            certificate,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[derive(Clone)]
struct RustlsConnect(Arc<ClientConfig>);

impl<S> MakeTlsConnect<S> for RustlsConnect
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = RustlsStream<S>;
    type TlsConnect = RustlsConnector;
    type Error = io::Error;

    fn make_tls_connect(&mut self, domain: &str) -> io::Result<RustlsConnector> {
        let name = ServerName::try_from(domain.to_owned())
            .map_err(|reason| invalid_input(format!("PostgreSQL host {domain:?}: {reason}")))?;
        Ok(RustlsConnector {
            config: self.0.clone(),
            name,
        })
    }
}

struct RustlsConnector {
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
}

impl<S> TlsConnect<S> for RustlsConnector
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Stream = RustlsStream<S>;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<RustlsStream<S>>> + Send>>;

    fn connect(self, stream: S) -> Self::Future {
        Box::pin(async move {
            tokio_rustls::TlsConnector::from(self.config)
                .connect(self.name, stream)
                .await
                .map(RustlsStream)
        })
    }
}

struct RustlsStream<S>(tokio_rustls::client::TlsStream<S>);

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for RustlsStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(context, buffer)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for RustlsStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().0).poll_write(context, buffer)
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(context)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> TlsStream for RustlsStream<S> {
    fn channel_binding(&self) -> ChannelBinding {
        ChannelBinding::none()
    }
}

impl ToSql for Value {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut tokio_postgres::types::private::BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match self {
            Self::Null => Ok(IsNull::Yes),
            Self::Int(value) if *ty == Type::FLOAT8 => (*value as f64).to_sql(ty, out),
            Self::Int(value) if *ty == Type::INT4 => i32::try_from(*value)?.to_sql(ty, out),
            Self::Int(value) => value.to_sql(ty, out),
            Self::Real(value) => value.to_sql(ty, out),
            Self::Text(value) => value.as_str().to_sql(ty, out),
            Self::Bytes(value) => value.as_slice().to_sql(ty, out),
        }
    }

    fn accepts(ty: &Type) -> bool {
        [
            Type::INT4,
            Type::INT8,
            Type::FLOAT8,
            Type::TEXT,
            Type::VARCHAR,
            Type::BYTEA,
        ]
        .contains(ty)
    }

    tokio_postgres::types::to_sql_checked!();
}

/// The parameters of a statement, as the client takes them.
pub(crate) fn params(values: &[Value]) -> Vec<&(dyn ToSql + Sync)> {
    values
        .iter()
        .map(|value| value as &(dyn ToSql + Sync))
        .collect()
}

pub(crate) fn row(row: &tokio_postgres::Row) -> io::Result<Row> {
    let mut values = Vec::with_capacity(row.len());
    for (index, column) in row.columns().iter().enumerate() {
        let ty = column.type_();
        let value = if *ty == Type::INT8 {
            row.try_get::<_, Option<i64>>(index)
                .map(|value| value.map(Value::Int))
        } else if *ty == Type::INT4 {
            row.try_get::<_, Option<i32>>(index)
                .map(|value| value.map(|value| Value::Int(i64::from(value))))
        } else if *ty == Type::FLOAT8 {
            row.try_get::<_, Option<f64>>(index)
                .map(|value| value.map(Value::Real))
        } else if *ty == Type::BYTEA {
            row.try_get::<_, Option<Vec<u8>>>(index)
                .map(|value| value.map(Value::Bytes))
        } else {
            row.try_get::<_, Option<String>>(index)
                .map(|value| value.map(Value::Text))
        };
        values.push(value.map_err(error)?.unwrap_or(Value::Null));
    }
    Ok(Row(values))
}

pub(crate) fn error(error: tokio_postgres::Error) -> io::Error {
    use tokio_postgres::error::SqlState;
    let kind = match error.code() {
        Some(code) if *code == SqlState::UNIQUE_VIOLATION => io::ErrorKind::AlreadyExists,
        Some(code)
            if *code == SqlState::LOCK_NOT_AVAILABLE || *code == SqlState::QUERY_CANCELED =>
        {
            io::ErrorKind::TimedOut
        }
        _ if error.is_closed() => io::ErrorKind::ConnectionAborted,
        _ => io::ErrorKind::Other,
    };
    // The reason, such as which certificate check failed, is in the source chain.
    let mut message = format!("PostgreSQL: {error}");
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        message.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    io::Error::new(kind, message)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::super::SqlRegistry;
    use super::super::db::{Clock, Tx};
    use super::*;
    use crate::directorates::registry::backend::*;
    use crate::directorates::registry::suite::{POSTGRES_URL_ENV, PostgresStore, until};

    /// Names a server that takes TLS only, with `sslrootcert=` pointing at the CA its
    /// certificate is signed by.
    const TLS_URL_ENV: &str = "AI6_TEST_POSTGRES_TLS_URL";

    fn store() -> Option<PostgresStore> {
        let store = PostgresStore::new();
        if store.is_none() {
            eprintln!("skipped a PostgreSQL test: {POSTGRES_URL_ENV} is not set");
        }
        store
    }

    #[test]
    fn a_station_stalled_in_a_transaction_is_cut_off_and_another_is_elected_within_the_lease() {
        let Some(store) = store() else { return };
        let lease = Duration::from_secs(4);
        let options = SqlOptions::for_lease(lease);
        let frozen = Arc::new(SqlRegistry::open_with(&store.location(), &options).unwrap());
        assert!(frozen.elect("station-a", lease).unwrap());
        let elected_at = Instant::now();

        // Station A freezes inside a write, holding the Registry's clock row, as a paused
        // VM or a half-open connection would.
        let (started, holding) = mpsc::channel();
        let station = frozen.clone();
        std::thread::spawn(move || {
            let _ = station.db.write(Clock::Registry, |_, _| {
                started.send(()).unwrap();
                std::thread::sleep(lease * 3);
                Ok::<_, io::Error>(())
            });
        });
        holding.recv().unwrap();

        let standby = SqlRegistry::open_with(&store.location(), &options).unwrap();
        until(|| standby.elect("station-b", lease).ok().filter(|won| *won));
        let taken = elected_at.elapsed();
        assert!(
            taken < lease + Duration::from_secs(3),
            "the standby took over after {taken:?}"
        );
        assert_eq!(standby.headquarters().unwrap().unwrap().term(), 2);
    }

    fn show(tx: &mut Tx<'_>, setting: &str) -> io::Result<String> {
        tx.query_opt(&format!("SHOW {setting}"), &[])?
            .expect("a setting")
            .text(0)
    }

    #[test]
    fn writes_read_committed_and_reads_one_snapshot_whatever_the_server_default() {
        let Some(store) = store() else { return };
        let location = store.location_with(&[("default_transaction_isolation", "serializable")]);
        let registry = SqlRegistry::open(&location).unwrap();
        assert_eq!(
            registry
                .db
                .write(Clock::Registry, |tx, _| show(tx, "transaction_isolation"))
                .unwrap(),
            "read committed"
        );
        assert_eq!(
            registry
                .db
                .read(|tx| Ok::<_, io::Error>((
                    show(tx, "transaction_isolation")?,
                    show(tx, "transaction_read_only")?
                )))
                .unwrap(),
            ("repeatable read".to_owned(), "on".to_owned())
        );

        // Under the server's serializable default, Stations writing at once would fail one
        // another; read committed behind the clock row lets each wait its turn.
        let writers = (0..6)
            .map(|index| {
                let location = location.clone();
                std::thread::spawn(move || {
                    let station = SqlRegistry::open(&location).unwrap();
                    for _ in 0..5 {
                        station
                            .check_in(&format!("station-{index}"), Default::default())
                            .unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(registry.stations(Duration::from_secs(60)).unwrap().len(), 6);
    }

    fn used_and_dropped_on_this_thread() {
        let Some(store) = store() else { return };
        let registry = SqlRegistry::open(&store.location()).unwrap();
        registry.check_in("station-a", Default::default()).unwrap();
        assert!(registry.elect("station-a", Duration::from_secs(5)).unwrap());
        drop(registry);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_handle_is_used_and_dropped_on_a_single_threaded_runtime() {
        used_and_dropped_on_this_thread();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_handle_is_used_and_dropped_on_an_async_worker() {
        used_and_dropped_on_this_thread();
    }

    #[test]
    fn a_password_comes_from_a_file_or_the_options_instead_of_the_location() {
        let Some(store) = store() else { return };
        let location = store.location();
        let (scheme, rest) = location.split_once("://").unwrap();
        let (userinfo, host) = rest.split_once('@').unwrap();
        let Some((user, password)) = userinfo.split_once(':') else {
            eprintln!("skipped: {POSTGRES_URL_ENV} has no password to move out of it");
            return;
        };
        let bare = format!("{scheme}://{user}@{host}");
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("password");
        std::fs::write(&file, format!("{password}\n")).unwrap();

        let from_file = format!("{bare}&password_file={}", file.display());
        SqlRegistry::open(&from_file)
            .unwrap()
            .check_in("station-a", Default::default())
            .unwrap();
        SqlRegistry::open_with(&bare, &SqlOptions::default().with_password(password))
            .unwrap()
            .check_in("station-b", Default::default())
            .unwrap();
        assert!(SqlRegistry::open(&bare).is_err(), "no password, no session");
    }

    /// `url` without its `sslrootcert` and `sslmode`, with `extra` parameters added.
    fn tls_location(url: &str, extra: &[(&str, &str)]) -> String {
        let (base, query) = url.split_once('?').unwrap_or((url, ""));
        let mut pairs = query
            .split('&')
            .filter(|pair| {
                !pair.is_empty()
                    && !pair.starts_with("sslrootcert=")
                    && !pair.starts_with("sslmode=")
            })
            .map(str::to_owned)
            .collect::<Vec<_>>();
        pairs.extend(extra.iter().map(|(key, value)| format!("{key}={value}")));
        if pairs.is_empty() {
            base.to_owned()
        } else {
            format!("{base}?{}", pairs.join("&"))
        }
    }

    #[test]
    fn tls_verifies_the_server_unless_a_location_says_otherwise() {
        let Some(url) = std::env::var(TLS_URL_ENV)
            .ok()
            .filter(|url| !url.is_empty())
        else {
            eprintln!("skipped the PostgreSQL TLS test: {TLS_URL_ENV} is not set");
            return;
        };
        // The default, verify-full, with the server's own CA: trusted.
        let trusted = SqlRegistry::open(&url).unwrap();
        trusted.check_in("station-a", Default::default()).unwrap();
        drop(trusted);

        let refusal = |location: String| SqlRegistry::open(&location).unwrap_err().to_string();

        // The platform's roots do not include a private CA.
        let untrusted = refusal(tls_location(&url, &[]));
        assert!(untrusted.contains("certificate"), "{untrusted}");

        // Another CA: the server is not the one the location trusts.
        let directory = tempfile::tempdir().unwrap();
        let stranger = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let stranger_file = directory.path().join("stranger.pem");
        std::fs::write(&stranger_file, stranger.cert.pem()).unwrap();
        let stranger_root = stranger_file.display().to_string();
        let stranger = refusal(tls_location(&url, &[("sslrootcert", &stranger_root)]));
        assert!(stranger.contains("certificate"), "{stranger}");

        // require: encrypted, nothing verified, so any certificate passes.
        SqlRegistry::open(&tls_location(&url, &[("sslmode", "require")]))
            .unwrap()
            .check_in("station-b", Default::default())
            .unwrap();

        // disable: a server that takes TLS only refuses it.
        let plaintext = refusal(tls_location(&url, &[("sslmode", "disable")]));
        assert!(plaintext.contains("no encryption"), "{plaintext}");
    }
}
