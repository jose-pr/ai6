//! `forward`: TCP over a link, listening here for what the far side was asked to reach and letting it reach what is named here.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use coqui_agentic_peer::Error;
use coqui_agentic_peer::auth::{self, Sealed, Trust};
use coqui_agentic_peer::forward::{self, Target};
use coqui_agentic_peer::link::{self, Services};
use coqui_agentic_peer::mux::{self, Side};
use coqui_agentic_peer::peers::{NoEvents, PeerRecord, PeerStore};
use coqui_peer::TrustStore;
use coqui_transport::Channel;
use coqui_transport_web::websocket;
use coqui_transport_web::websocket::Frames;
use coqui_transport_web::{Dialer as _, Kind};
use url::Url;

use crate::common::{
    DIAL_PROOF, MAX_LINE, MethodArgs, SessionArgs, built, in_band_tls, peers, proof_identity,
    proof_of, say_proof, store_of, unproven_limits,
};

/// The arguments of `forward`: where to connect, which ports to listen on here and which targets the far side may reach.
#[derive(clap::Args, Clone, Debug)]
pub struct Forward {
    /// The URL to connect to: wss://, or ws:// on loopback.
    pub url: Url,
    /// This side's identity. See `keygen`.
    #[arg(long, required_unless_present = "paired")]
    pub identity: Option<PathBuf>,
    /// Use this user's identity, and prove the remote against whoever this user has paired with.
    #[arg(long, conflicts_with = "identity")]
    pub paired: bool,
    /// A peer id the remote must prove with `--identity`, and who `--allow-target` is for. Repeat for more.
    #[arg(long = "peer", required_unless_present = "paired")]
    pub peers: Vec<String>,
    /// A PEM bundle of certificate authorities the remote certificate must chain to, with wss://.
    #[arg(long)]
    pub ca_file: Option<PathBuf>,
    /// Listen here and reach a target from the remote, as LOCAL=TARGET. LOCAL has to be a loopback address, and any account on
    /// this machine that connects to it reaches the target as the remote allows. Repeat for more. The remote decides from its own
    /// grants.
    #[arg(long = "local", value_name = "LOCAL=TARGET")]
    pub locals: Vec<String>,
    /// Let the remote reach this target from here, once it listens for it with `serve --reverse`. Repeat for more. With none, it
    /// reaches nothing.
    #[arg(long = "allow-target", value_name = "HOST:PORT")]
    pub allow_targets: Vec<String>,
    /// The proof's method.
    #[command(flatten)]
    pub method: MethodArgs,
    /// The session's settings.
    #[command(flatten)]
    pub session: SessionArgs,
    /// The directory of the identity, trust store and grants, when the program that embeds this keeps them somewhere else than
    /// `coqui_agentic_peer::home` does. Not a command-line argument.
    #[arg(skip)]
    pub home: Option<PathBuf>,
    /// With `--paired`, the trust store the remote is proved against, in place of this user's own: a host that keeps the keys
    /// it dials apart from the keys it serves names it. Not a command-line argument.
    #[arg(skip)]
    pub paired_store: Option<PathBuf>,
}

pub(crate) fn local_address(text: &str) -> Result<SocketAddr, String> {
    let address: SocketAddr = text
        .parse()
        .map_err(|_| format!("{text} is not an address and port"))?;
    if !forward::is_loopback(address.ip()) {
        return Err(format!(
            "{text} is not loopback: anyone who can reach it gets what this peer was granted"
        ));
    }
    Ok(address)
}

pub(crate) fn parse_local(text: &str) -> Result<(SocketAddr, Target), String> {
    let (local, target) = text.split_once('=').ok_or("--local is LOCAL=TARGET")?;
    Ok((
        local_address(local)?,
        target
            .parse()
            .map_err(|error: forward::BadTarget| error.to_string())?,
    ))
}

/// One listener for the life of a link. A port someone else holds is said once and tried again: the rest of the link is still good,
/// and whoever holds it would otherwise be given this forward's clients for good.
pub(crate) fn listen_for<C: Channel>(mux: &mux::Mux<'_, C>, local: SocketAddr, target: &Target) {
    let mut said = false;
    let listener = loop {
        match std::net::TcpListener::bind(local) {
            Ok(listener) => break listener,
            Err(error) => {
                if !said {
                    eprintln!("could not listen on {local}, trying again: {error}");
                    said = true;
                }
                for _ in 0..50 {
                    if mux.link_state() != mux::LinkState::Up {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    };
    eprintln!("{local} forwards to {target}");
    if let Err(error) = forward::listen(mux, &listener, target) {
        eprintln!("stopped listening on {local}: {error}");
    }
}

/// What the remote may reach from here, for the peers named: another key this user paired with answering at the same address gets
/// nothing.
struct AllowedTargets {
    peers: BTreeSet<String>,
    grants: BTreeSet<String>,
}

impl PeerStore for AllowedTargets {
    fn get(&self, peer: &str) -> Result<Option<PeerRecord>, Error> {
        let mut record = PeerRecord::new(peer);
        if self.peers.contains(peer) {
            record.grants.clone_from(&self.grants);
        }
        Ok(Some(record))
    }

    fn put(&self, _record: &PeerRecord) -> Result<(), Error> {
        Err(Error::Identity(
            "these grants are fixed by the command line".to_owned(),
        ))
    }

    fn remove(&self, _peer: &str) -> Result<(), Error> {
        Err(Error::Identity(
            "these grants are fixed by the command line".to_owned(),
        ))
    }
}

/// Runs `forward` until stopped, connecting again when the link goes.
///
/// # Errors
///
/// The arguments do not fit together, or the remote proved the wrong key.
pub fn run(args: &Forward) -> Result<u8, String> {
    let locals = args
        .locals
        .iter()
        .map(|text| parse_local(text))
        .collect::<Result<Vec<_>, _>>()?;
    let mut grants = BTreeSet::new();
    for text in &args.allow_targets {
        let target: Target = text
            .parse()
            .map_err(|error: forward::BadTarget| error.to_string())?;
        grants.insert(forward::TCP_PROTOCOL.to_owned());
        grants.insert(target.grant_token());
    }
    if locals.is_empty() && grants.is_empty() {
        return Err("give --local or --allow-target: otherwise this forwards nothing".to_owned());
    }
    if !grants.is_empty() && args.peers.is_empty() {
        return Err("--allow-target needs --peer: it says who may reach it".to_owned());
    }
    let (method, store) = if args.paired {
        let identity = proof_identity(args.home.as_deref(), None, args.method, true)?;
        let store = Arc::new(store_of(
            args.home.as_deref(),
            args.paired_store.as_deref(),
        )?);
        (
            built(
                proof_of(identity, args.session)
                    .responders(Trust::Store(TrustStore::clone(&store))),
            )?,
            Some(store),
        )
    } else {
        let path = args
            .identity
            .as_deref()
            .ok_or("give --identity or --paired")?;
        let identity = proof_identity(args.home.as_deref(), Some(path), args.method, true)?;
        (
            built(proof_of(identity, args.session).responders(Trust::pinned(peers(&args.peers)?)))?,
            None,
        )
    };
    let mut dialer = websocket::Dialer::builder()
        .limits(unproven_limits())
        .frames(Frames::Binary);
    if args.url.scheme() == "wss" {
        dialer = dialer.tls(in_band_tls(args.ca_file.as_ref())?);
    }
    let mut dialer = dialer.build().map_err(|error| error.to_string())?;
    let allowed = AllowedTargets {
        peers: args.peers.iter().cloned().collect(),
        grants,
    };
    let services = forward::serving(Services::default());
    let mut options = link::Options::default();
    options.limits.max_message = MAX_LINE;
    say_proof(&method);
    let longest = Duration::from_secs(60);
    let mut wait = Duration::from_secs(1);
    loop {
        match dialer.dial(Kind::WebSocket, &args.url, Duration::from_secs(10)) {
            Ok(channel) => match auth::prove_out(
                &method,
                &channel,
                DIAL_PROOF,
                method.method().record_length(MAX_LINE),
            ) {
                Ok(established) => {
                    wait = Duration::from_secs(1);
                    let peer = established.peer;
                    eprintln!("connected to {peer}");
                    let sealed = Sealed::new(&channel, established.session);
                    let sealed = match &store {
                        Some(store) => {
                            let store = Arc::clone(store);
                            let proven = peer;
                            sealed.with_check(move || store.is_accepted(&proven))
                        }
                        None => sealed,
                    };
                    let host = link::Host {
                        store: &allowed,
                        services: &services,
                        events: &NoEvents,
                    };
                    link::serve_with(
                        &sealed,
                        Side::Dialed,
                        options,
                        None,
                        &peer.to_string(),
                        &host,
                        |mux| {
                            // Says this is a link, which the far side otherwise waits to hear before it opens anything.
                            if let Err(failure) = link::capabilities(mux, Duration::from_secs(10)) {
                                eprintln!(
                                    "the remote said nothing about what it offers: {failure}"
                                );
                            }
                            thread::scope(|scope| {
                                for (local, target) in &locals {
                                    scope.spawn(move || listen_for(mux, *local, target));
                                }
                            });
                        },
                    );
                    sealed.close();
                }
                Err(Error::Proof(why)) => {
                    return Err(format!("the remote did not prove itself: {why}"));
                }
                Err(error) => eprintln!("the proof did not finish: {error}"),
            },
            Err(failure) => eprintln!("could not connect: {failure}"),
        }
        thread::sleep(wait);
        wait = (wait * 2).min(longest);
    }
}
