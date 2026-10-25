//! The connection to one host: host-key policy, login, and reconnecting.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ai6::guatiao::Status;
use ai6::guatiao::library::ProviderError;
use russh::Channel;
use russh::client::{self, ChannelOpenHandle, Handle, Msg};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh_sftp::client::SftpSession;
use tokio::net::TcpStream;

use crate::refuse;
use crate::settings::Settings;

const KEEPALIVE: Duration = Duration::from_secs(30);
const KEEPALIVE_MAX: usize = 3;

/// What the host's key must be for the connection to go on.
#[derive(Debug, Clone)]
enum Policy {
    /// The key must hash to this `sha256:HEX`.
    Pinned(String),
    /// The first key seen is trusted, and every later connection must present the same.
    FirstUse,
    /// No key is acceptable; the refusal says which one was presented.
    Refuse,
}

/// State the handler and the session share across reconnects.
#[derive(Default)]
pub(crate) struct HostKey {
    /// The key presented in the latest handshake.
    seen: Mutex<Option<String>>,
    /// The key trusted on first use, once there is one.
    trusted: Mutex<Option<String>>,
    /// Why the latest handshake was refused.
    refusal: Mutex<Option<String>>,
}

impl HostKey {
    /// The key trusted on first use, if that policy accepted one.
    pub(crate) fn trusted(&self) -> Option<String> {
        lock(&self.trusted).clone()
    }
}

/// Reverse forwards currently registered: the host's listening port to the Station address.
#[derive(Default)]
pub(crate) struct Forwards {
    targets: Mutex<BTreeMap<u32, String>>,
}

impl Forwards {
    pub(crate) fn add(&self, port: u32, target: String) {
        lock(&self.targets).insert(port, target);
    }

    pub(crate) fn remove(&self, port: u32) {
        lock(&self.targets).remove(&port);
    }

    fn target(&self, port: u32) -> Option<String> {
        lock(&self.targets).get(&port).cloned()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) struct Client {
    policy: Policy,
    host_key: Arc<HostKey>,
    forwards: Arc<Forwards>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let presented = fingerprint(server_public_key);
        *lock(&self.host_key.seen) = Some(presented.clone());
        let verdict = match &self.policy {
            Policy::Pinned(pin) if *pin == presented => Ok(()),
            Policy::Pinned(pin) => Err(format!(
                "the host presented key {presented}, which is not the pinned {pin}"
            )),
            Policy::FirstUse => {
                let mut trusted = lock(&self.host_key.trusted);
                match trusted.as_deref() {
                    None => {
                        *trusted = Some(presented.clone());
                        Ok(())
                    }
                    Some(known) if known == presented => Ok(()),
                    Some(known) => Err(format!(
                        "the host presented key {presented}, which is not the {known} trusted at first contact"
                    )),
                }
            }
            Policy::Refuse => Err(format!(
                "no host key is pinned and the host presented {presented}; set `host_key` to pin it, or `trust_on_first_use = true` to accept the first key seen"
            )),
        };
        match verdict {
            Ok(()) => Ok(true),
            Err(reason) => {
                *lock(&self.host_key.refusal) = Some(reason);
                Ok(false)
            }
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        // Dropping the reply refuses the channel.
        let Some(target) = self.forwards.target(connected_port) else {
            return Ok(());
        };
        reply.accept().await;
        tokio::spawn(async move {
            if let Ok(mut station) = TcpStream::connect(&target).await {
                let mut remote = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut remote, &mut station).await;
            }
        });
        Ok(())
    }
}

/// The SHA-256 of the public key blob, as `sha256:HEX`.
fn fingerprint(key: &PublicKeyOrCertificate) -> String {
    let fingerprint = key.public_key().fingerprint(HashAlg::Sha256);
    let mut text = String::from("sha256:");
    for byte in fingerprint.as_bytes() {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// One authenticated connection, shared by every operation until it closes.
pub(crate) struct Conn {
    pub(crate) handle: Handle<Client>,
    pub(crate) forwards: Arc<Forwards>,
    sftp: tokio::sync::Mutex<Option<Arc<SftpSession>>>,
}

impl Conn {
    pub(crate) fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    pub(crate) async fn channel(&self) -> Result<Channel<Msg>, russh::Error> {
        self.handle.channel_open_session().await
    }

    /// The SFTP session on this connection, opened on first use.
    pub(crate) async fn sftp(&self) -> Result<Arc<SftpSession>, ProviderError> {
        let mut cached = self.sftp.lock().await;
        if let Some(session) = cached.as_ref() {
            return Ok(session.clone());
        }
        let channel = self
            .channel()
            .await
            .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| refuse(Status::GUATIAO_ERR_UNSUPPORTED, format!("sftp: {error}")))?;
        let session = Arc::new(
            SftpSession::new(channel.into_stream())
                .await
                .map_err(|error| {
                    refuse(Status::GUATIAO_ERR_UNSUPPORTED, format!("sftp: {error}"))
                })?,
        );
        *cached = Some(session.clone());
        Ok(session)
    }

    /// Forgets the SFTP session after it failed, so the next use opens a new one.
    pub(crate) async fn drop_sftp(&self) {
        *self.sftp.lock().await = None;
    }
}

/// Everything one transport instance needs to reach its host.
pub(crate) struct Link {
    settings: Settings,
    pub(crate) host_key: Arc<HostKey>,
    forwards: Arc<Forwards>,
    conn: tokio::sync::Mutex<Option<Arc<Conn>>>,
}

impl Link {
    pub(crate) fn new(settings: Settings) -> Self {
        Self {
            settings,
            host_key: Arc::new(HostKey::default()),
            forwards: Arc::new(Forwards::default()),
            conn: tokio::sync::Mutex::new(None),
        }
    }

    pub(crate) fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The live connection, made (or remade) when there is none.
    pub(crate) async fn conn(&self) -> Result<Arc<Conn>, ProviderError> {
        let mut slot = self.conn.lock().await;
        if let Some(conn) = slot.as_ref()
            && !conn.is_closed()
        {
            return Ok(conn.clone());
        }
        let conn = Arc::new(self.connect().await?);
        *slot = Some(conn.clone());
        Ok(conn)
    }

    /// Drops `conn` if it is still the current one, so the next use reconnects.
    pub(crate) async fn reset(&self, conn: &Arc<Conn>) {
        let mut slot = self.conn.lock().await;
        if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, conn))
        {
            *slot = None;
        }
    }

    /// A new session channel, reconnecting once when the connection turns out to be dead.
    pub(crate) async fn channel(&self) -> Result<(Arc<Conn>, Channel<Msg>), ProviderError> {
        let conn = self.conn().await?;
        match conn.channel().await {
            Ok(channel) => Ok((conn, channel)),
            Err(_) => {
                self.reset(&conn).await;
                let conn = self.conn().await?;
                let channel = conn
                    .channel()
                    .await
                    .map_err(|error| refuse(Status::GUATIAO_ERR_INTERNAL, error))?;
                Ok((conn, channel))
            }
        }
    }

    async fn connect(&self) -> Result<Conn, ProviderError> {
        let settings = &self.settings;
        let policy = match settings.pin() {
            Some(pin) => Policy::Pinned(pin),
            None if settings.trust_on_first_use => Policy::FirstUse,
            None => Policy::Refuse,
        };
        let key = self.load_key()?;
        let config = Arc::new(client::Config {
            keepalive_interval: Some(KEEPALIVE),
            keepalive_max: KEEPALIVE_MAX,
            ..client::Config::default()
        });
        let handler = Client {
            policy,
            host_key: self.host_key.clone(),
            forwards: self.forwards.clone(),
        };
        *lock(&self.host_key.refusal) = None;
        let address = (settings.host.clone(), settings.port());
        let timeout = settings.connect_timeout();
        let connected = tokio::time::timeout(timeout, async {
            let mut handle = client::connect(config, address, handler).await?;
            let hash = handle.best_supported_rsa_hash().await?.flatten();
            let login = handle
                .authenticate_publickey(
                    settings.user(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await?;
            Ok::<_, russh::Error>((handle, login.success()))
        })
        .await;
        let target = format!("{}:{}", settings.host, settings.port());
        match connected {
            Err(_) => Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!("{target}: no answer within {} seconds", timeout.as_secs()),
            )),
            Ok(Err(error)) => {
                let refusal = lock(&self.host_key.refusal).clone();
                Err(refuse(
                    Status::GUATIAO_ERR_INTERNAL,
                    refusal.map_or_else(
                        || format!("{target}: {error}"),
                        |why| format!("{target}: {why}"),
                    ),
                ))
            }
            Ok(Ok((_, false))) => Err(refuse(
                Status::GUATIAO_ERR_INTERNAL,
                format!(
                    "{target}: the host refused the key for user {:?}",
                    settings.user()
                ),
            )),
            Ok(Ok((handle, true))) => Ok(Conn {
                handle,
                forwards: self.forwards.clone(),
                sftp: tokio::sync::Mutex::new(None),
            }),
        }
    }

    fn load_key(&self) -> Result<russh::keys::PrivateKey, ProviderError> {
        let path = self
            .settings
            .identity_path()
            .map_err(|why| refuse(Status::GUATIAO_ERR_BAD_VALUE, why))?;
        load_secret_key(&path, None).map_err(|error| match error {
            russh::keys::Error::KeyIsEncrypted => refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!(
                    "{}: the key is passphrase-protected; the transport never holds a passphrase, so use a key without one",
                    path.display()
                ),
            ),
            other => refuse(
                Status::GUATIAO_ERR_BAD_VALUE,
                format!("{}: {other}", path.display()),
            ),
        })
    }
}
