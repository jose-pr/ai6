//! `serve` and the listening side of `pair`: accept connections, take a pairing or a proof from each, and serve the link of a proven
//! peer what its record grants.

use std::net::{SocketAddr, TcpListener};
use std::thread;
use std::time::{Duration, SystemTime};

use cabuya::auth::{Identity, Proof, Sealed, Trust};
use cabuya::link::{self, Host, Options, Services};
use cabuya::mux::Side;
use cabuya::pairing::{self, Answered, PairingHost};
use cabuya::peers::{Event, Events, FilePeers, PeerRecord, PeerStore as _};
use coqui_peer::pair::PairingDesk;
use coqui_peer::{PeerId, TrustStore};
use coqui_transport::Channel as _;

use crate::grants;
use crate::home::Home;
use crate::wire;

/// Where `serve` listens when it is not told.
pub const SERVE_ADDRESS: &str = "0.0.0.0:7711";
/// Where `pair` listens when it is not told.
pub const PAIR_ADDRESS: &str = "0.0.0.0:7710";

/// Writes what happens on a link to standard error: kinds and peer ids, never a message.
struct Log;

impl Events for Log {
    fn event(&self, event: &Event) {
        eprintln!("cabuya: {event:?}");
    }
}

/// What a listener holds that every connection shares.
struct Listening<'a> {
    identity: &'a Identity,
    served: &'a TrustStore,
    grants: &'a FilePeers,
    desk: &'a PairingDesk,
    pairing: bool,
    on_pairing: &'a [String],
}

/// The services a peer may open, each decided against its record.
pub fn services() -> Services {
    cabuya::forward::serving(Services::default())
}

/// Shows the desk's PIN where the operator reads it, and says how long it holds.
fn show_pin(desk: &PairingDesk) -> Result<(), String> {
    let shown = desk.show_pin().map_err(|error| error.to_string())?;
    let minutes = shown
        .expires
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO)
        .as_secs()
        / 60;
    println!("pin {}", shown.pin.as_str());
    println!("pin-valid-minutes {minutes}");
    Ok(())
}

fn announce(listener: &TcpListener, identity: &Identity) -> Result<(), String> {
    let address = listener.local_addr().map_err(|error| error.to_string())?;
    println!("listening {address}");
    println!("peer {}", identity.peer_id());
    flush();
    Ok(())
}

fn flush() {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
}

fn bind(address: &str) -> Result<TcpListener, String> {
    let address: SocketAddr = address
        .parse()
        .map_err(|_| format!("{address} is not an address and port"))?;
    TcpListener::bind(address).map_err(|error| format!("cannot listen on {address}: {error}"))
}

/// Records what a pairing grants the peer it admitted.
fn grant_paired(grants: &FilePeers, peer: PeerId, tokens: &[String]) -> Result<(), String> {
    if tokens.is_empty() {
        return Ok(());
    }
    grants::add(grants, peer, tokens)
}

/// Answers one connection; the link of a proven peer runs here until it ends.
fn handle(connection: std::net::TcpStream, proof: &Proof, listening: &Listening<'_>) -> Outcome {
    let source = connection.peer_addr().ok();
    let channel = match wire::channel(connection) {
        Ok(channel) => channel,
        Err(why) => return Outcome::Failed(why),
    };
    let host = listening.pairing.then_some(PairingHost {
        identity: listening.identity,
        store: listening.served,
        desk: listening.desk,
    });
    let outcome = match pairing::answer(
        &channel,
        proof,
        wire::PROOF_MESSAGE,
        host,
        source,
        wire::PROOF_WITHIN,
    ) {
        Ok(Answered::Paired(paired)) => {
            let peer = paired.peer.id();
            match grant_paired(listening.grants, peer, listening.on_pairing) {
                Ok(()) => Outcome::Paired(peer),
                Err(why) => Outcome::Failed(why),
            }
        }
        Ok(Answered::Proven(done)) => {
            let (peer, accepted) = (done.peer, listening.served);
            let sealed =
                Sealed::new(&channel, done.session).with_check(move || accepted.is_accepted(&peer));
            let host = Host {
                store: listening.grants,
                services: &services(),
                events: &Log,
            };
            link::serve(
                &sealed,
                Side::Answered,
                Options::default(),
                None,
                &peer.to_string(),
                &host,
            );
            Outcome::Served
        }
        Ok(_) => Outcome::Failed("an answer this program does not know".to_owned()),
        Err(unanswered) => Outcome::Refused {
            pairing: unanswered.pairing,
            why: unanswered.to_string(),
        },
    };
    channel.close();
    outcome
}

/// How one connection ended.
enum Outcome {
    Paired(PeerId),
    Served,
    /// A pairing or proof that failed; `pairing` says which.
    Refused {
        pairing: bool,
        why: String,
    },
    Failed(String),
}

/// `cabuya serve`: listens until it is stopped.
pub fn serve(
    home: &Home,
    method: Option<cabuya::auth::Method>,
    listen: &str,
    grants_file: Option<std::path::PathBuf>,
    pair: bool,
    on_pairing: &[String],
) -> Result<u8, String> {
    let identity = home.identity(method)?;
    let served = home.served()?;
    let grants = home.grants(grants_file);
    let desk = pairing::desk().map_err(|error| error.to_string())?;
    let proof = Proof::builder(identity.clone())
        .initiators(Trust::Store(served.clone()))
        .build()
        .map_err(|error| error.to_string())?;
    let listener = bind(listen)?;
    announce(&listener, &identity)?;
    if pair {
        show_pin(&desk)?;
        flush();
    }
    let listening = Listening {
        identity: &identity,
        served: &served,
        grants: &grants,
        desk: &desk,
        pairing: pair,
        on_pairing,
    };
    thread::scope(|scope| {
        for connection in listener.incoming() {
            let Ok(connection) = connection else {
                continue;
            };
            let (proof, listening) = (&proof, &listening);
            scope.spawn(move || match handle(connection, proof, listening) {
                Outcome::Paired(peer) => {
                    println!("paired {peer}");
                    flush();
                }
                Outcome::Served => {}
                Outcome::Refused { pairing, why } => {
                    let what = if pairing { "pairing" } else { "proof" };
                    eprintln!("cabuya: a {what} was refused: {why}");
                }
                Outcome::Failed(why) => eprintln!("cabuya: {why}"),
            });
        }
    });
    Ok(0)
}

/// `cabuya pair`: shows a PIN and waits for one peer to pair with it, then ends.
pub fn pair(
    home: &Home,
    method: Option<cabuya::auth::Method>,
    listen: &str,
    grants_file: Option<std::path::PathBuf>,
    on_pairing: &[String],
) -> Result<u8, String> {
    let identity = home.identity(method)?;
    let served = home.served()?;
    let grants = home.grants(grants_file);
    let desk = pairing::desk().map_err(|error| error.to_string())?;
    // This listener only pairs: a peer that proves itself is turned away, since the proof trusts nobody.
    let proof = Proof::builder(identity.clone())
        .initiators(Trust::pinned(Vec::<PeerId>::new()))
        .build()
        .map_err(|error| error.to_string())?;
    let listener = bind(listen)?;
    announce(&listener, &identity)?;
    show_pin(&desk)?;
    flush();
    let listening = Listening {
        identity: &identity,
        served: &served,
        grants: &grants,
        desk: &desk,
        pairing: true,
        on_pairing,
    };
    for connection in listener.incoming() {
        let Ok(connection) = connection else {
            continue;
        };
        match handle(connection, &proof, &listening) {
            Outcome::Paired(peer) => {
                println!("paired {peer}");
                flush();
                return Ok(0);
            }
            Outcome::Refused { pairing, why } => {
                eprintln!("cabuya: a connection was refused: {why}");
                if pairing && desk.shown_pin().is_none() {
                    return Err("the PIN is spent: run `cabuya pair` again".to_owned());
                }
            }
            Outcome::Failed(why) => eprintln!("cabuya: {why}"),
            Outcome::Served => {}
        }
    }
    Ok(1)
}

/// The record of `peer`, or an empty one.
pub fn record_of(grants: &FilePeers, peer: &str) -> Result<PeerRecord, String> {
    Ok(grants
        .get(peer)
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| PeerRecord::new(peer)))
}
