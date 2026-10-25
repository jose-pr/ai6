//! An in-process SSH server for tests: public-key login, exec and pty channels that run
//! `sh -c`, an SFTP subsystem over the real filesystem, and reverse forwards.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use ai6::TransportConfig;
use ai6::guatiao::{ToValue, Value};
use russh::keys::ssh_key::{LineEnding, PublicKey};
use russh::keys::{Algorithm, HashAlg, PrivateKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, Server as _, Session, VerifiedPublicKey};
use russh::{Channel, ChannelId, Sig};
use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

/// The user name the fake server accepts.
pub const USER: &str = "tester";

/// What the fake server saw clients ask for.
#[derive(Default, Debug)]
pub struct Seen {
    /// `(columns, rows)` of each pty request.
    pub ptys: Vec<(u32, u32)>,
    /// `(columns, rows)` of each window change.
    pub window_changes: Vec<(u32, u32)>,
    /// Signal names clients sent.
    pub signals: Vec<String>,
    /// Commands clients asked to execute.
    pub commands: Vec<String>,
    /// How many connections were accepted.
    pub connections: usize,
}

pub struct FakeSsh {
    pub port: u16,
    pub seen: Arc<Mutex<Seen>>,
    pub dir: tempfile::TempDir,
    host_key: PrivateKey,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    handles: Arc<Mutex<Vec<russh::server::Handle>>>,
}

/// Whether the commands the fake server runs have a shell to run in.
pub fn shell_available() -> bool {
    std::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .status()
        .is_ok_and(|status| status.success())
}

impl FakeSsh {
    /// Starts a server on a free loopback port. The client key is written to
    /// `<dir>/client_key`.
    pub fn start() -> Self {
        Self::start_with(Algorithm::Ed25519)
    }

    pub fn start_with(host_algorithm: Algorithm) -> Self {
        Self::launch(0, host_algorithm, None).expect("a loopback port is free")
    }

    /// Starts a server on `port` that accepts the key in `client_key`, or answers `None`
    /// when the port cannot be bound.
    pub fn start_on(port: u16, client_key: &Path) -> Option<Self> {
        Self::launch(port, Algorithm::Ed25519, Some(client_key))
    }

    fn launch(port: u16, host_algorithm: Algorithm, existing: Option<&Path>) -> Option<Self> {
        let dir = tempfile::tempdir().unwrap();
        let client = match existing {
            Some(path) => russh::keys::load_secret_key(path, None).unwrap(),
            None => PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap(),
        };
        client
            .write_openssh_file(dir.path().join("client_key"), LineEnding::LF)
            .unwrap();
        let host_key = PrivateKey::random(&mut rand::rng(), host_algorithm).unwrap();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (shutdown, stop) = oneshot::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();

        let config = Arc::new(russh::server::Config {
            auth_rejection_time: std::time::Duration::from_millis(10),
            auth_rejection_time_initial: Some(std::time::Duration::ZERO),
            keys: vec![host_key.clone()],
            inactivity_timeout: None,
            ..russh::server::Config::default()
        });
        let authorized = client.public_key().clone();
        let thread_seen = seen.clone();
        let handles = Arc::new(Mutex::new(Vec::new()));
        let thread_handles = handles.clone();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await else {
                    ready_tx.send(None).unwrap();
                    return;
                };
                ready_tx.send(listener.local_addr().ok()).unwrap();
                let mut server = Server {
                    authorized,
                    seen: thread_seen,
                    handles: thread_handles,
                };
                tokio::select! {
                    _ = stop => {}
                    _ = async {
                        loop {
                            let Ok((socket, peer)) = listener.accept().await else { break };
                            let handler = server.new_client(Some(peer));
                            let config = config.clone();
                            tokio::spawn(async move {
                                if let Ok(session) = russh::server::run_stream(config, socket, handler).await {
                                    let _ = session.await;
                                }
                            });
                        }
                    } => {}
                }
            });
            runtime.shutdown_background();
        });
        let Some(address) = ready_rx.recv().unwrap() else {
            let _ = thread.join();
            return None;
        };
        Some(Self {
            port: address.port(),
            seen,
            dir,
            host_key,
            shutdown: Some(shutdown),
            thread: Some(thread),
            handles,
        })
    }

    /// Closes every open connection from the server's side.
    pub fn disconnect_all(&self) {
        let handles: Vec<_> = self.handles.lock().unwrap().drain(..).collect();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for handle in handles {
            let _ = runtime.block_on(handle.disconnect(
                russh::Disconnect::ByApplication,
                "test".to_owned(),
                "en".to_owned(),
            ));
        }
    }

    /// The server's host key as `sha256:HEX`.
    pub fn pin(&self) -> String {
        let fingerprint = self.host_key.public_key().fingerprint(HashAlg::Sha256);
        let mut pin = String::from("sha256:");
        for byte in fingerprint.as_bytes() {
            pin.push_str(&format!("{byte:02x}"));
        }
        pin
    }

    /// The path of the private key the server accepts.
    pub fn identity(&self) -> PathBuf {
        self.dir.path().join("client_key")
    }

    /// Transport settings that pin the host key; `extra` lines are appended.
    pub fn settings(&self, extra: &str) -> String {
        format!(
            "host = \"127.0.0.1\"\nport = {}\nuser = \"{USER}\"\nidentity = {:?}\nhost_key = \"{}\"\n{extra}",
            self.port,
            self.identity().to_string_lossy(),
            self.pin(),
        )
    }

    /// Transport settings with no pin; `extra` lines are appended.
    pub fn unpinned_settings(&self, extra: &str) -> String {
        format!(
            "host = \"127.0.0.1\"\nport = {}\nuser = \"{USER}\"\nidentity = {:?}\n{extra}",
            self.port,
            self.identity().to_string_lossy(),
        )
    }

    /// The same, as the value the plugin host hands a transport.
    pub fn config(&self, settings_toml: String) -> Value {
        TransportConfig { settings_toml }.to_value().unwrap()
    }
}

impl Drop for FakeSsh {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
struct Server {
    authorized: PublicKey,
    seen: Arc<Mutex<Seen>>,
    handles: Arc<Mutex<Vec<russh::server::Handle>>>,
}

impl russh::server::Server for Server {
    type Handler = Connection;

    fn new_client(&mut self, _: Option<SocketAddr>) -> Connection {
        self.seen.lock().unwrap().connections += 1;
        Connection {
            handles: self.handles.clone(),
            authorized: self.authorized.clone(),
            seen: self.seen.clone(),
            channels: HashMap::new(),
            ptys: HashSet::new(),
            stdin: HashMap::new(),
            kill: HashMap::new(),
            forwards: HashMap::new(),
        }
    }
}

struct Connection {
    handles: Arc<Mutex<Vec<russh::server::Handle>>>,
    authorized: PublicKey,
    seen: Arc<Mutex<Seen>>,
    channels: HashMap<ChannelId, Channel<Msg>>,
    ptys: HashSet<ChannelId>,
    stdin: HashMap<ChannelId, ChildStdin>,
    kill: HashMap<ChannelId, oneshot::Sender<()>>,
    forwards: HashMap<u32, tokio::task::JoinHandle<()>>,
}

/// A connection's forwards stop listening when it ends, as an OpenSSH server's do.
impl Drop for Connection {
    fn drop(&mut self) {
        for accept in self.forwards.values() {
            accept.abort();
        }
    }
}

impl russh::server::Handler for Connection {
    type Error = russh::Error;

    async fn auth_publickey(
        &mut self,
        user: &str,
        key: VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        if user == USER && *key.public_key == self.authorized {
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }

    async fn auth_succeeded(&mut self, session: &mut Session) -> Result<(), Self::Error> {
        self.handles.lock().unwrap().push(session.handle());
        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.ptys.insert(channel);
        self.seen.lock().unwrap().ptys.push((col_width, row_height));
        session.channel_success(channel)
    }

    async fn window_change_request(
        &mut self,
        _channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.seen
            .lock()
            .unwrap()
            .window_changes
            .push((col_width, row_height));
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let command = String::from_utf8_lossy(data).into_owned();
        self.seen.lock().unwrap().commands.push(command.clone());
        let spawned = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = spawned else {
            return session.channel_failure(channel);
        };
        session.channel_success(channel)?;
        if let Some(stdin) = child.stdin.take() {
            self.stdin.insert(channel, stdin);
        }
        let (kill, killed) = oneshot::channel();
        self.kill.insert(channel, kill);
        let merge = self.ptys.contains(&channel);
        tokio::spawn(supervise(session.handle(), channel, child, merge, killed));
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(stdin) = self.stdin.get_mut(&channel) {
            let _ = stdin.write_all(data).await;
        }
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.stdin.remove(&channel);
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.stdin.remove(&channel);
        if let Some(kill) = self.kill.remove(&channel) {
            let _ = kill.send(());
        }
        Ok(())
    }

    async fn signal(
        &mut self,
        channel: ChannelId,
        signal: Sig,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.seen
            .lock()
            .unwrap()
            .signals
            .push(format!("{signal:?}"));
        if let Some(kill) = self.kill.remove(&channel) {
            let _ = kill.send(());
        }
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            return session.channel_failure(channel);
        }
        let Some(stream) = self.channels.remove(&channel) else {
            return session.channel_failure(channel);
        };
        session.channel_success(channel)?;
        tokio::spawn(russh_sftp::server::run(
            stream.into_stream(),
            Sftp::default(),
        ));
        Ok(())
    }

    async fn tcpip_forward(
        &mut self,
        address: &str,
        port: &mut u32,
        session: &mut Session,
    ) -> Result<bool, Self::Error> {
        let Ok(listener) = TcpListener::bind((address, u16::try_from(*port).unwrap_or(0))).await
        else {
            return Ok(false);
        };
        let bound = u32::from(listener.local_addr().unwrap().port());
        *port = bound;
        let (handle, address) = (session.handle(), address.to_owned());
        let accept = tokio::spawn(async move {
            while let Ok((socket, peer)) = listener.accept().await {
                let opened = handle
                    .channel_open_forwarded_tcpip(
                        address.clone(),
                        bound,
                        peer.ip().to_string(),
                        u32::from(peer.port()),
                    )
                    .await;
                if let Ok(channel) = opened {
                    tokio::spawn(async move {
                        let (mut socket, mut stream) = (socket, channel.into_stream());
                        let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                    });
                }
            }
        });
        self.forwards.insert(bound, accept);
        Ok(true)
    }

    async fn cancel_tcpip_forward(
        &mut self,
        _address: &str,
        port: u32,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        match self.forwards.remove(&port) {
            Some(accept) => {
                accept.abort();
                let _ = accept.await;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

/// Relays a child's output to the channel, then its exit status.
async fn supervise(
    handle: russh::server::Handle,
    channel: ChannelId,
    mut child: Child,
    merge: bool,
    mut killed: oneshot::Receiver<()>,
) {
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (mut out_buf, mut err_buf) = ([0u8; 4096], [0u8; 4096]);
    let (mut out_open, mut err_open) = (true, true);
    while out_open || err_open {
        tokio::select! {
            read = stdout.read(&mut out_buf), if out_open => match read {
                Ok(0) | Err(_) => out_open = false,
                Ok(n) => { let _ = handle.data(channel, out_buf[..n].to_vec()).await; }
            },
            read = stderr.read(&mut err_buf), if err_open => match read {
                Ok(0) | Err(_) => err_open = false,
                Ok(n) => {
                    let bytes = err_buf[..n].to_vec();
                    let _ = if merge {
                        handle.data(channel, bytes).await.map_err(|_| ())
                    } else {
                        handle.extended_data(channel, 1, bytes).await.map_err(|_| ())
                    };
                }
            },
            _ = &mut killed => {
                let _ = child.kill().await;
                break;
            }
        }
    }
    let code = match child.wait().await {
        Ok(status) => status.code().map_or(255, |code| code as u32),
        Err(_) => 255,
    };
    let _ = handle.exit_status_request(channel, code).await;
    let _ = handle.eof(channel).await;
    let _ = handle.close(channel).await;
}

enum Open {
    File(std::fs::File),
    Dir(Vec<File>),
}

#[derive(Default)]
struct Sftp {
    handles: HashMap<String, Open>,
    next: u64,
}

fn status(id: u32, code: StatusCode) -> Status {
    Status {
        id,
        status_code: code,
        error_message: code.to_string(),
        language_tag: "en-US".to_owned(),
    }
}

fn code_of(error: &std::io::Error) -> StatusCode {
    match error.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn attributes(metadata: &std::fs::Metadata) -> FileAttributes {
    let mut attrs = FileAttributes {
        size: Some(metadata.len()),
        permissions: Some(0o644),
        ..FileAttributes::default()
    };
    if metadata.is_dir() {
        attrs.set_dir(true);
    } else if metadata.is_symlink() {
        attrs.set_symlink(true);
    } else {
        attrs.set_regular(true);
    }
    attrs
}

impl russh_sftp::server::Handler for Sftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let mut options = OpenOptions::new();
        options
            .read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE))
            .append(pflags.contains(OpenFlags::APPEND))
            .create(pflags.contains(OpenFlags::CREATE))
            .truncate(pflags.contains(OpenFlags::TRUNCATE));
        let file = options.open(&filename).map_err(|e| code_of(&e))?;
        self.next += 1;
        let handle = format!("h{}", self.next);
        self.handles.insert(handle.clone(), Open::File(file));
        Ok(Handle { id, handle })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(status(id, StatusCode::Ok))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let Some(Open::File(file)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| code_of(&e))?;
        let mut data = vec![0u8; len as usize];
        let read = file.read(&mut data).map_err(|e| code_of(&e))?;
        if read == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(read);
        Ok(Data { id, data })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let Some(Open::File(file)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| code_of(&e))?;
        file.write_all(&data).map_err(|e| code_of(&e))?;
        Ok(status(id, StatusCode::Ok))
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let Some(Open::File(file)) = self.handles.get(&handle) else {
            return Err(StatusCode::Failure);
        };
        let metadata = file.metadata().map_err(|e| code_of(&e))?;
        Ok(Attrs {
            id,
            attrs: attributes(&metadata),
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let metadata = std::fs::metadata(&path).map_err(|e| code_of(&e))?;
        Ok(Attrs {
            id,
            attrs: attributes(&metadata),
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| code_of(&e))?;
        Ok(Attrs {
            id,
            attrs: attributes(&metadata),
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(|e| code_of(&e))? {
            let entry = entry.map_err(|e| code_of(&e))?;
            let metadata = std::fs::symlink_metadata(entry.path()).map_err(|e| code_of(&e))?;
            entries.push(File::new(
                entry.file_name().to_string_lossy().into_owned(),
                attributes(&metadata),
            ));
        }
        self.next += 1;
        let handle = format!("d{}", self.next);
        self.handles.insert(handle.clone(), Open::Dir(entries));
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let Some(Open::Dir(entries)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        if entries.is_empty() {
            return Err(StatusCode::Eof);
        }
        Ok(Name {
            id,
            files: std::mem::take(entries),
        })
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        std::fs::create_dir(&path).map_err(|e| code_of(&e))?;
        Ok(status(id, StatusCode::Ok))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let resolved = if Path::new(&path).exists() {
            std::fs::canonicalize(&path).map_or(path.clone(), |p| p.to_string_lossy().into_owned())
        } else {
            path.clone()
        };
        Ok(Name {
            id,
            files: vec![File::dummy(resolved)],
        })
    }
}
