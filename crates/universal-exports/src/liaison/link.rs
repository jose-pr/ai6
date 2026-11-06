//! Liaison's link listener: a WebSocket a peer dials, the in-band proof that names it, and the
//! link that carries its A2A streams.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use coqui_a2a::{PROTOCOL, Server};
use coqui_agentic_peer::auth::{self, Identity, Method, Proof, Sealed, Trust};
use coqui_agentic_peer::link::{self, Host, Options, Services};
use coqui_agentic_peer::mux::Side;
use coqui_agentic_peer::peers::{NoEvents, PeerRecord, PeerStore};
use coqui_web_channel::Channel as _;
use coqui_web_channel::websocket::{Frames, Listener};

use super::outbound::Outbound;
use super::{Liaison, LiaisonConfig, LiaisonError};

/// The time a dialer has to finish its proof.
const PROOF_WAIT: Duration = Duration::from_secs(10);

/// How often the accept loop looks at whether it should stop.
const ACCEPT_POLL: Duration = Duration::from_millis(100);

/// The peers the configuration pairs, each granted `a2a/1` unless it says otherwise. It is read
/// only: a peer is paired by writing the configuration, never over a link.
struct ConfiguredPeers(Arc<LiaisonConfig>);

impl PeerStore for ConfiguredPeers {
    fn get(&self, peer: &str) -> Result<Option<PeerRecord>, coqui_agentic_peer::Error> {
        Ok(self
            .0
            .peers
            .values()
            .find(|entry| entry.id.as_deref() == Some(peer))
            .map(|entry| {
                let record = PeerRecord::new(peer).claiming("name", &entry.name);
                if entry.a2a {
                    record.granting(PROTOCOL)
                } else {
                    record
                }
            }))
    }

    fn put(&self, _record: &PeerRecord) -> Result<(), coqui_agentic_peer::Error> {
        Err(coqui_agentic_peer::Error::Identity(
            "Liaison's peers are configured, not stored".to_owned(),
        ))
    }

    fn remove(&self, _peer: &str) -> Result<(), coqui_agentic_peer::Error> {
        Err(coqui_agentic_peer::Error::Identity(
            "Liaison's peers are configured, not stored".to_owned(),
        ))
    }
}

/// Liaison's running link listener; dropping it stops it, and the links it serves end.
pub struct LinkListener {
    address: SocketAddr,
    peer_id: String,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
    dialer: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for LinkListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkListener")
            .field("address", &self.address)
            .field("peer_id", &self.peer_id)
            .finish_non_exhaustive()
    }
}

impl LinkListener {
    /// Binds `address` and serves `liaison` to the configured peers, proving itself with the key
    /// in `identity` (made the first time, by Noise). With `tls` the listener is `wss://`; without
    /// it, it binds beyond loopback only when the configuration says
    /// `plain_beyond_loopback`. It also carries Cables sent to a peer: on the link the peer holds
    /// open, else by dialing its `url`.
    ///
    /// # Errors
    ///
    /// The key cannot be read or made, a peer id is not one, or the address cannot be bound.
    pub fn start(
        liaison: &Liaison,
        address: SocketAddr,
        tls: Option<Arc<rustls::ServerConfig>>,
        identity: &Path,
    ) -> Result<Self, LiaisonError> {
        let config = liaison.inner.config.clone();
        let identity_path = identity;
        if let Some(parent) = identity.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| LiaisonError::new(format!("the link key's directory: {error}")))?;
        }
        let identity = Identity::open_or_create(identity_path, Method::DEFAULT)
            .map_err(|error| LiaisonError::new(format!("the link key: {error}")))?;
        let outbound = Outbound::new(liaison.clone(), identity.clone());
        let peer_id = identity.peer_id().to_string();
        let ids = config
            .peers
            .values()
            .filter_map(|entry| entry.id.as_deref())
            .map(str::parse)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| LiaisonError::new("a peer id is not sha256:<hex> of a key"))?;
        let proof = Arc::new(Proof::new(identity).with_initiators(Trust::pinned(ids)));
        let listener = match tls {
            Some(tls) => Listener::bind(address, Some(tls)),
            None if address.ip().is_loopback() || config.plain_beyond_loopback => {
                Listener::bind_plain(address)
            }
            None => {
                return Err(LiaisonError::new(
                    "[unex.liaison] listen beyond loopback needs [unex.tls], or \
                     plain_beyond_loopback",
                ));
            }
        }
        .map_err(|error| LiaisonError::new(format!("the link listener: {error}")))?
        .with_limits(coqui_web_channel::Limits::default().with_max_message(auth::MAX_PROOF_MESSAGE))
        .with_frames(Frames::Binary);
        let address = listener
            .local_addr()
            .map_err(|error| LiaisonError::new(format!("the link listener: {error}")))?;
        let stop = Arc::new(AtomicBool::new(false));
        let store: Arc<dyn PeerStore> = Arc::new(ConfiguredPeers(config.clone()));
        let services = Arc::new(Server::new(liaison.clone()).serving(Services::default()));
        let links = Arc::new(AtomicUsize::new(0));
        let (dialing, dial_stop) = (outbound.clone(), stop.clone());
        let dialer = thread::Builder::new()
            .name("liaison-dial".to_owned())
            .spawn(move || dialing.dial_loop(&dial_stop))
            .map_err(|error| LiaisonError::new(format!("the outbound thread: {error}")))?;
        let flag = stop.clone();
        let accept = thread::Builder::new()
            .name("liaison-links".to_owned())
            .spawn(move || {
                while !flag.load(Ordering::SeqCst) {
                    let Ok(pending) = listener.next(Some(ACCEPT_POLL)) else {
                        continue;
                    };
                    if !config.max_links.allows(links.load(Ordering::SeqCst) + 1) {
                        drop(pending);
                        continue;
                    }
                    links.fetch_add(1, Ordering::SeqCst);
                    let (proof, store, services, links, config, outbound, stop) = (
                        proof.clone(),
                        store.clone(),
                        services.clone(),
                        links.clone(),
                        config.clone(),
                        outbound.clone(),
                        flag.clone(),
                    );
                    let spawned = thread::Builder::new()
                        .name("liaison-link".to_owned())
                        .spawn(move || {
                            serve_one(
                                pending, &proof, &*store, &services, &config, &outbound, &stop,
                            );
                            links.fetch_sub(1, Ordering::SeqCst);
                        });
                    if spawned.is_err() {
                        eprintln!("universal-exports: Liaison could not start a link's thread");
                    }
                }
            })
            .map_err(|error| LiaisonError::new(format!("the link listener's thread: {error}")))?;
        Ok(Self {
            address,
            peer_id,
            stop,
            accept: Some(accept),
            dialer: Some(dialer),
        })
    }

    /// The address it listens on.
    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    /// The gateway's own peer id, which a peer pins: `sha256:<hex>` of its link key.
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }
}

impl Drop for LinkListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        if let Some(dialer) = self.dialer.take() {
            let _ = dialer.join();
        }
    }
}

/// Proves one dialer and serves its link until it ends. A key nobody configured fails the proof,
/// and nothing of the link is read.
fn serve_one(
    pending: coqui_web_channel::websocket::Pending,
    proof: &Proof,
    store: &dyn PeerStore,
    services: &Services,
    config: &LiaisonConfig,
    outbound: &Outbound,
    stop: &AtomicBool,
) {
    let Ok(channel) = pending.handshake() else {
        return;
    };
    let options = Options::default();
    let max_message = options.limits.max_message;
    match auth::prove(
        proof,
        &channel,
        PROOF_WAIT,
        proof.method().record_length(max_message),
    ) {
        Ok(established) => {
            let peer = established.peer.to_string();
            let sealed = Sealed::new(&channel, established.session);
            let host = Host {
                store,
                services,
                events: &NoEvents,
            };
            let mut options = options;
            options.recheck = config.recheck;
            link::serve_with(
                &sealed,
                Side::Answered,
                options,
                None,
                &peer,
                &host,
                |mux| {
                    outbound.on_link(&peer, mux, stop);
                },
            );
            sealed.close();
        }
        Err(_) => {
            // An unpaired key, or no proof at all: refused before any stream.
        }
    }
    channel.close();
}
