//! `serve`: wait for peers, prove each, and give it an agent of its own, with the other protocols a link can carry.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write as _;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use cabuya::Error;
use cabuya::auth::{self, Method, Proof, Sealed, Trust};
use cabuya::forward::{self, Target};
use cabuya::link::{self, Accepted, Prefilled, Services};
use cabuya::mux::{self, LinkStream, Side};
use cabuya::peers::{FilePeers, NoEvents, PeerRecord, PeerStore};
use coqui_agentic_peer::discovery::{self, Advertised, Advertisement};
use coqui_agentic_peer::mcp_offer;
use coqui_peer::{PeerId, TrustStore};
use coqui_peer_tls as tls;
use coqui_transport::{Channel, Failure};
use coqui_transport_web::websocket::Frames;
use coqui_transport_web::{websocket, webtransport};

use crate::agent::Agent;
use crate::common::{
    Entered, Gate, MAX_LINE, MethodArgs, SessionArgs, admission, built, grants_of, limits, peers,
    proof_identity, proof_of, read_tls_identity, say_proof, server_tls, server_tls_of, split_agent,
    store_of, unproven_limits,
};
use crate::forward::{listen_for, local_address};
use crate::helper::Helper;
use crate::linger::{self, Keeper, Settings};
use crate::relay::{LinkEnd, relay};
use crate::standing;

/// Connections that have not proved themselves yet, each a thread for at most the handshake and proof limits.
const MAX_UNPROVEN: usize = 64;
/// At most what a browser pins, with a margin.
const SHORT_LIVED_VALIDITY: Duration = Duration::from_secs(14 * 86_400 - 60);
const SHORT_LIVED_RUN: Duration = Duration::from_secs(13 * 86_400);
/// The status `serve` ends the process with when its short-lived certificate is about to end.
pub const EXIT_CERTIFICATE_ENDING: i32 = 3;

/// A rule a host adds to who may be served: given a proven peer, whether it is let in.
#[derive(Clone)]
pub struct Admit(Arc<dyn Fn(&PeerId) -> bool + Send + Sync>);

impl Admit {
    /// A rule that lets in the peers `rule` says yes to.
    pub fn new(rule: impl Fn(&PeerId) -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(rule))
    }

    fn allows(&self, peer: &PeerId) -> bool {
        (self.0)(peer)
    }
}

impl core::fmt::Debug for Admit {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Admit")
    }
}

/// The arguments of `serve`: where to listen, which peers to let in and by what proof, and what each is given.
#[derive(clap::Args, Clone, Debug)]
pub struct Serve {
    /// The address to listen on. Plain WebSocket is refused beyond loopback.
    #[arg(long, default_value = "127.0.0.1:8765")]
    pub listen: SocketAddr,
    /// This bridge's identity (see `keygen`), for `--allow` and `--allow-store`.
    #[arg(long)]
    pub identity: Option<PathBuf>,
    /// A peer id allowed in, proved in-band. Repeat for more. With nobody allowed, the bridge refuses to start: nothing
    /// is trusted on first use.
    #[arg(long = "allow")]
    pub allow: Vec<String>,
    /// Allow in whoever this user has paired with, proved in-band, under this user's own identity: one pairing serves every
    /// command. Takes the place of `--identity` and `--allow-store`.
    #[arg(long, conflicts_with_all = ["identity", "allow", "allow_store"])]
    pub paired: bool,
    /// A trust store whose accepted peers are allowed in, proved in-band, read on each connection and while it lasts, so a
    /// peer paired or removed later takes effect at once.
    #[arg(long)]
    pub allow_store: Option<PathBuf>,
    /// A TLS identity (certificate and key, PEM) for wss:// and WebTransport. Without it, plain ws:// on loopback only, where
    /// a reverse proxy may stand in front: the in-band proof and its session do not depend on TLS.
    #[arg(long, conflicts_with = "tls_short_lived")]
    pub tls_identity: Option<PathBuf>,
    /// Make a certificate for this run and keep it in memory, for a browser to pin over WebTransport with no certificate
    /// authority. Browsers take only one valid for 14 days at most, so `serve` ends with status 3 after 13: restart it, and a
    /// client finds the new hash by looking for it again. Needs the in-band proof, which is what authenticates.
    #[arg(long)]
    pub tls_short_lived: bool,
    /// A client certificate fingerprint allowed in over mutual TLS, in place of the in-band proof. Repeat for more. Needs
    /// `--tls-identity`. The stream is then plain ACP protected only by that TLS, so it ends where TLS ends.
    #[arg(long = "allow-cert")]
    pub allow_cert: Vec<String>,
    /// A trust store of client certificates allowed in over mutual TLS, read on each connection. Needs `--tls-identity`.
    #[arg(long)]
    pub allow_cert_store: Option<PathBuf>,
    /// A PEM bundle of certificate authorities a client certificate may chain to, over mutual TLS. Needs `--tls-identity`.
    #[arg(long)]
    pub client_ca: Option<PathBuf>,
    /// Prove nothing about the peer: whoever can connect gets the agent, any account on this machine included. Only on loopback, or with `--permit` naming who can
    /// reach it, and a browser needs its `--origin` listed, or any page it opens could drive the agent. With `--tls-identity` a
    /// client can still pin this server.
    #[arg(long)]
    pub no_peer_proof: bool,
    /// Serve WebTransport on this UDP port too; needs `--tls-identity`, and the in-band proof.
    #[arg(long)]
    pub webtransport_port: Option<u16>,
    /// A browser origin allowed in, as `scheme://host[:port]`. Repeat for more. A non-browser client sends none. With none listed, no origin is checked.
    #[arg(long = "origin")]
    pub origins: Vec<String>,
    /// A source network allowed to connect at all, like 10.6.0.0/16. Repeat for more. With any listed, everything else is
    /// dropped before its handshake. Checked first, in the order given, against `--deny`.
    #[arg(long = "permit")]
    pub permit: Vec<String>,
    /// A source network dropped before its handshake. Repeat for more. Takes precedence over `--permit`.
    #[arg(long = "deny")]
    pub deny: Vec<String>,
    /// How long a peer may take to prove itself in-band, in seconds. Default 15.
    #[arg(long)]
    pub proof_seconds: Option<u64>,
    /// Proven peers served at once, each with an agent of its own. Another is turned away while this many run.
    #[arg(long, default_value_t = 4)]
    pub max_peers: usize,
    /// When a link ends, keep its agent this many seconds and give it back to the same proven peer on a new connection, so ACP's
    /// `session/resume` reaches a live session. Needs the in-band proof. Without it, or with 0, the agent ends with its link.
    #[arg(long, value_name = "SECONDS")]
    pub linger: Option<u64>,
    /// Agents kept for one peer, past which the one of them started last ends: a peer has two only when it connected while an earlier
    /// link was still up, and the earlier holds that link's session. The bridge reads no ACP, so it cannot tell which agent holds the
    /// session a peer wants: above 1, a peer can be given the wrong one.
    #[arg(long, requires = "linger", value_name = "N", default_value = "1")]
    pub linger_per_peer: NonZeroUsize,
    /// Agents kept for everyone together, past which the one kept longest ends. These take no place among --max-peers.
    #[arg(long, requires = "linger", value_name = "N", default_value = "4")]
    pub linger_total: NonZeroUsize,
    /// Bytes of a kept agent's output to hold while nobody is connected, each line counted with what holding it takes, at most 256 MiB.
    /// Past this the oldest lines are dropped, and the next link is told first, in a notification of its own.
    #[arg(long, requires = "linger", value_name = "BYTES", default_value_t = 16 * 1024 * 1024, value_parser = buffer_bytes)]
    pub linger_buffer_bytes: usize,
    /// How long a new connection waits for a link of the same peer to end, so a reload faster than the old link's end finds its agent.
    #[arg(long, requires = "linger", value_name = "MS", default_value_t = 500)]
    pub linger_settle_ms: u64,
    /// Serve another protocol on the same link, as TOKEN=PROGRAM, such as mcp=my-server. Repeat for more. The agent after `--` is
    /// `acp/1`. A client opens streams on one proven link, with no second proof and no second pairing.
    #[arg(long = "service", value_name = "TOKEN=PROGRAM")]
    pub services: Vec<String>,
    /// What a peer may open, as PEER=TOKEN. Repeat for more. With any listed, a peer may open only what names it. With none, every
    /// peer allowed in may open everything; `--paired` keeps its grants with `grant` instead.
    #[arg(long = "grant", value_name = "PEER=TOKEN")]
    pub grants: Vec<String>,
    /// Serve `tcp/1`, forwarding to the targets a peer is granted and to no others. Nothing is granted by this. Name each with
    /// `--grant PEER=tcp/1:HOST:PORT` or, with `--paired`, the `grant` command.
    #[arg(long)]
    pub forward: bool,
    /// Listen on this machine and reach a target from a peer that forwards for this one, as LOCAL=PEER=TARGET. LOCAL has to be a
    /// loopback address, and any account on this machine that connects to it gets what the peer was granted. Repeat for more.
    /// That peer decides from its own grants.
    #[arg(long = "reverse", value_name = "LOCAL=PEER=TARGET")]
    pub reverses: Vec<String>,
    /// Serve `mcp-offer/1`, so a peer can give its agent the peer's own MCP tools. A peer allowed `acp/1` can already declare any
    /// command to its agent, so this adds a way to reach the peer's tools and nothing else.
    #[arg(long)]
    pub mcp_offers: bool,
    /// Say this agent exists, under this name. On loopback a file only this user's programs read; otherwise a DNS-SD record
    /// everyone on the link can see, with no secret in it. Connecting still needs the proof.
    #[arg(long, value_name = "LABEL")]
    pub advertise: Option<String>,
    /// A host name to publish in that record, for a certificate a browser accepts. Without it, clients get an address.
    #[arg(long, requires = "advertise", value_name = "NAME")]
    pub advertise_host: Option<String>,
    /// The proof's method.
    #[command(flatten)]
    pub method: MethodArgs,
    /// The session's settings.
    #[command(flatten)]
    pub session: SessionArgs,
    /// The agent program, then its arguments.
    #[arg(last = true)]
    pub agent: Vec<OsString>,
    /// The program that runs the helper subcommands a linger or an offer needs. Not a command-line argument: the program that
    /// embeds this names itself.
    #[arg(skip)]
    pub helper: Option<Helper>,
    /// The directory of the identity, trust store and grants, when the program that embeds this keeps them somewhere else than
    /// `coqui_agentic_peer::home` does. Not a command-line argument.
    #[arg(skip)]
    pub home: Option<PathBuf>,
    /// Whether a proven peer may be served at all, asked once it has proved itself and again, for a link that lasts, whenever the
    /// trust store is asked. For a host whose own rules limit some keys. Not a command-line argument.
    #[arg(skip)]
    pub admit: Option<Admit>,
}

/// A certificate browsers will pin, starting a little early for a client whose clock runs behind.
fn short_lived_identity() -> Result<tls::Identity, String> {
    let not_before = SystemTime::now() - Duration::from_secs(3600);
    tls::Identity::builder()
        .not_before(not_before)
        .not_after(not_before + SHORT_LIVED_VALIDITY)
        .generate()
        .map_err(|error| error.to_string())
}

/// New connections could not open after the certificate ends, and that must not look like a bridge still working. By the clock
/// on the wall: a machine that sleeps does not count that towards a sleep. Agents read their end of the pipe closing.
fn end_with_the_certificate(advertised: Option<Advertised>) {
    let until = SystemTime::now() + SHORT_LIVED_RUN;
    thread::spawn(move || {
        while SystemTime::now() < until {
            thread::sleep(Duration::from_secs(60));
        }
        drop(advertised);
        eprintln!(
            "coqui-acp-bridge: the short-lived certificate is ending: restart to make another"
        );
        std::process::exit(EXIT_CERTIFICATE_ENDING);
    });
}

/// What every connection of a `serve` shares.
/// `serve --reverse`: a listener here, for as long as this peer's link is up.
struct Reverse {
    local: SocketAddr,
    peer: String,
    target: Target,
}

fn parse_reverse(text: &str) -> Result<Reverse, String> {
    let mut parts = text.splitn(3, '=');
    let (Some(local), Some(peer), Some(target)) = (parts.next(), parts.next(), parts.next()) else {
        return Err("--reverse is LOCAL=PEER=TARGET".to_owned());
    };
    Ok(Reverse {
        local: local_address(local)?,
        peer: peer.to_owned(),
        target: target
            .parse()
            .map_err(|error: forward::BadTarget| error.to_string())?,
    })
}

struct Service {
    admit: Option<Admit>,
    helper: Option<Helper>,
    method: Option<Proof>,
    store: Option<Arc<TrustStore>>,
    proof: Duration,
    agent: Vec<OsString>,
    unproven: Gate,
    peers: Gate,
    keeper: Option<Keeper>,
    links: Option<Links>,
    reverses: Vec<Reverse>,
}

/// What a link can run beside the one agent, and who may open what.
struct Links {
    services: Services,
    grants: Box<dyn PeerStore>,
    open: LinkCounts,
}

/// How many links are up. A stream's limits are per link, so without this one key could open as many of everything as it liked.
#[derive(Default)]
struct LinkCounts(Mutex<BTreeMap<String, usize>>);

/// Links from one peer. Each can hold its own streams, so this also bounds what forwarding a peer can have open.
const MAX_LINKS_PER_PEER: usize = 8;
/// Links from everyone together.
const MAX_LINKS: usize = 256;
/// How long a proven peer may take to say whether it is a link or a plain client.
const FIRST_MESSAGE: Duration = Duration::from_secs(60);

impl LinkCounts {
    fn enter<'a>(&'a self, peer: &str) -> Option<LinkEntered<'a>> {
        let mut open = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let total: usize = open.values().sum();
        let mine = open.entry(peer.to_owned()).or_default();
        if *mine >= MAX_LINKS_PER_PEER || total >= MAX_LINKS {
            return None;
        }
        *mine += 1;
        Some(LinkEntered {
            counts: self,
            peer: peer.to_owned(),
        })
    }
}

struct LinkEntered<'a> {
    counts: &'a LinkCounts,
    peer: String,
}

impl Drop for LinkEntered<'_> {
    fn drop(&mut self) {
        let mut open = self.counts.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(mine) = open.get_mut(&self.peer) {
            *mine -= 1;
            if *mine == 0 {
                open.remove(&self.peer);
            }
        }
    }
}

/// The token of the agent named after `--`.
pub(crate) const ACP: &str = "acp/1";

/// `--grant`, or everything for whoever is allowed in: the proof already decided who that is.
struct FixedGrants {
    every: Vec<String>,
    named: Option<BTreeMap<String, BTreeSet<String>>>,
}

impl PeerStore for FixedGrants {
    fn get(&self, peer: &str) -> Result<Option<PeerRecord>, Error> {
        let mut record = PeerRecord::new(peer);
        match &self.named {
            Some(named) => record.grants = named.get(peer).cloned().unwrap_or_default(),
            None => record.grants = self.every.iter().cloned().collect(),
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

/// This user's grants, and the agent a peer was paired for: pairing gave it that, and anything else takes a `grant`.
struct PairedGrants(FilePeers);

impl PeerStore for PairedGrants {
    fn get(&self, peer: &str) -> Result<Option<PeerRecord>, Error> {
        let record = self.0.get(peer)?.unwrap_or_else(|| PeerRecord::new(peer));
        // Each comes with pairing: it is served only with `--mcp-offers`, and serving its own tools to its own agent adds nothing to `acp/1`.
        Ok(Some(
            record.granting(ACP).granting(mcp_offer::OFFER_PROTOCOL),
        ))
    }

    fn put(&self, record: &PeerRecord) -> Result<(), Error> {
        self.0.put(record)
    }

    fn remove(&self, peer: &str) -> Result<(), Error> {
        self.0.remove(peer)
    }
}

/// An agent for `peer` until this link ends, and `None` if it got none. With a keeper it may be one kept from an earlier link, and may be
/// kept again. The slot among --max-peers is held only while a link is up, and is free again before the arrival says so.
fn serve_agent<C: Channel + ?Sized>(
    helper: Option<&Helper>,
    program: &[OsString],
    gate: &Gate,
    keeper: Option<&Keeper>,
    channel: &C,
    peer: &str,
) -> Option<LinkEnd> {
    let arrival = keeper.map(|keeper| keeper.arrive(peer));
    let Some(_running) = gate.enter() else {
        eprintln!("{peer} was turned away: --max-peers are running");
        return None;
    };
    let start = || {
        let (program, args) = split_agent(program).map_err(std::io::Error::other)?;
        Agent::spawn(program, args, MAX_LINE)
    };
    let ended = match &arrival {
        Some(arrival) => arrival.serve(channel, || {
            let started = start()?;
            match helper {
                Some(helper) => {
                    let args = helper.command(&["agent-guard"]);
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    started.guarded(helper.program.as_os_str(), &args)
                }
                None => Ok(started),
            }
        }),
        None => start().map(|running| {
            let end = relay(channel, &running);
            let _ = running.stop();
            end
        }),
    };
    ended
        .map_err(|error| eprintln!("the agent could not start: {error}"))
        .ok()
}

fn agent_handler(
    program: Vec<OsString>,
    gate: Gate,
    keeper: Option<Keeper>,
    helper: Option<Helper>,
) -> link::Handler {
    Box::new(move |stream: &dyn LinkStream, accepted: &Accepted| {
        if let Some(end) = serve_agent(
            helper.as_ref(),
            &program,
            &gate,
            keeper.as_ref(),
            stream,
            &accepted.peer,
        ) {
            eprintln!("{} left {}: {end:?}", accepted.peer, accepted.protocol);
        }
    })
}

fn parse_service(text: &str) -> Result<(String, OsString), String> {
    let (token, program) = text.split_once('=').ok_or("a service is TOKEN=PROGRAM")?;
    if !mux::valid_protocol(token) {
        return Err(
            "a protocol token is printable, with no spaces, commas or equals signs".to_owned(),
        );
    }
    if token.starts_with(forward::TCP_PROTOCOL)
        || token == link::LINK_PROTOCOL
        || token == ACP
        || token == mcp_offer::OFFER_PROTOCOL
    {
        return Err(format!(
            "{token} is not for --service: it means something on every link"
        ));
    }
    if program.is_empty() {
        return Err("a service is TOKEN=PROGRAM".to_owned());
    }
    Ok((token.to_owned(), OsString::from(program)))
}

/// What `--service` and `--grant` ask for, if anything.
fn links(args: &Serve, gate: &Gate, keeper: Option<&Keeper>) -> Result<Option<Links>, String> {
    if args.services.is_empty() && !args.forward && args.reverses.is_empty() && !args.mcp_offers {
        return Ok(None);
    }
    let mut services = Services::default().with(
        ACP,
        agent_handler(
            args.agent.clone(),
            gate.clone(),
            keeper.cloned(),
            args.helper.clone(),
        ),
    );
    let mut every = vec![ACP.to_owned()];
    for text in &args.services {
        let (token, program) = parse_service(text)?;
        services = services.with(
            &token,
            agent_handler(vec![program], gate.clone(), None, None),
        );
        every.push(token);
    }
    if args.forward {
        services = forward::serving(services);
    }
    if args.mcp_offers {
        let helper = args
            .helper
            .as_ref()
            .ok_or("--mcp-offers needs a program that runs mcp-shim: this embedding named none")?;
        let offers = standing::dir(args.home.as_deref())?;
        let tokens = offers.clone();
        services = services.with(
            mcp_offer::OFFER_PROTOCOL,
            standing::handler(
                mcp_offer::handler(
                    helper.program.clone(),
                    helper.command(&["mcp-shim"]),
                    tokens,
                ),
                offers,
            ),
        );
        every.push(mcp_offer::OFFER_PROTOCOL.to_owned());
    }
    let grants: Box<dyn PeerStore> = if args.paired {
        if !args.grants.is_empty() {
            return Err("--paired keeps its grants with the grant command, not --grant".to_owned());
        }
        Box::new(PairedGrants(FilePeers::new(grants_of(
            args.home.as_deref(),
        )?)))
    } else {
        let mut named: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for text in &args.grants {
            let (peer, token) = text.split_once('=').ok_or("a grant is PEER=TOKEN")?;
            let entry = named.entry(peer.to_owned()).or_default();
            if let Some(target) = token.strip_prefix("tcp/1:") {
                if !args.forward {
                    return Err("a target needs --forward".to_owned());
                }
                let target: Target = target
                    .parse()
                    .map_err(|error: forward::BadTarget| error.to_string())?;
                entry.insert(forward::TCP_PROTOCOL.to_owned());
                entry.insert(target.grant_token());
            } else if every.iter().any(|offered| offered == token) {
                entry.insert(token.to_owned());
            } else {
                return Err("a grant names a protocol nothing here serves".to_owned());
            }
        }
        Box::new(FixedGrants {
            every,
            named: (!named.is_empty()).then_some(named),
        })
    };
    Ok(Some(Links {
        services,
        grants,
        open: LinkCounts::default(),
    }))
}

impl Service {
    /// Proves the peer unless TLS already did, gives it a fresh agent, and relays until either ends.
    fn run<C: Channel>(&self, channel: &C, unproven: Entered) {
        let Some(method) = &self.method else {
            drop(unproven);
            self.give_agent(channel, "a certificate peer");
            return channel.close();
        };
        match auth::prove(
            method,
            channel,
            self.proof,
            method.method().record_length(MAX_LINE),
        ) {
            Ok(established) => {
                drop(unproven);
                let peer = established.peer;
                let label = peer.to_string();
                if !self.admit.as_ref().is_none_or(|admit| admit.allows(&peer)) {
                    eprintln!("{label} was turned away: this host's own rules do not admit it");
                    return channel.close();
                }
                let sealed = Sealed::new(channel, established.session);
                let admit = self.admit.clone();
                let sealed = match &self.store {
                    Some(store) => {
                        let store = Arc::clone(store);
                        sealed.with_check(move || {
                            store.is_accepted(&peer)
                                && admit.as_ref().is_none_or(|admit| admit.allows(&peer))
                        })
                    }
                    None => sealed,
                };
                self.serve_peer(&sealed, &label);
                sealed.close();
            }
            Err(error) => eprintln!("a connection was refused: {error}"),
        }
        channel.close();
    }

    /// One protocol, or a link of several: a first message of streams says which, so nothing has to be agreed first. A plain ACP
    /// client speaks first, and with no services the program may be the one to, so this waits only where there is a choice.
    fn serve_peer<C: Channel>(&self, channel: &C, peer: &str) {
        let Some(links) = &self.links else {
            return self.give_agent(channel, peer);
        };
        eprintln!("{peer} proven");
        let mut options = link::Options::default();
        options.limits.max_message = MAX_LINE;
        let Some(_entered) = links.open.enter(peer) else {
            return eprintln!("{peer} was turned away: it has as many links as it may");
        };
        match channel.receive(Some(FIRST_MESSAGE)) {
            Ok(Some(first)) if mux::starts_mux(&first) => {
                let host = link::Host {
                    store: &*links.grants,
                    services: &links.services,
                    events: &NoEvents,
                };
                let reverses: Vec<&Reverse> = self
                    .reverses
                    .iter()
                    .filter(|reverse| reverse.peer == peer)
                    .collect();
                link::serve_with(
                    channel,
                    Side::Answered,
                    options,
                    Some(first),
                    peer,
                    &host,
                    |mux| {
                        thread::scope(|scope| {
                            for reverse in reverses {
                                scope
                                    .spawn(move || listen_for(mux, reverse.local, &reverse.target));
                            }
                        });
                    },
                );
            }
            Ok(Some(first)) => {
                let granted = links
                    .grants
                    .get(peer)
                    .is_ok_and(|record| record.is_some_and(|record| record.grants.contains(ACP)));
                if granted {
                    self.give_agent(&Prefilled::new(channel, first), peer);
                } else {
                    eprintln!("{peer} may not open {ACP}");
                }
            }
            Ok(None) | Err(_) => {}
        }
    }

    fn give_agent<C: Channel>(&self, channel: &C, peer: &str) {
        eprintln!("{peer} proven");
        let end = serve_agent(
            self.helper.as_ref(),
            &self.agent,
            &self.peers,
            self.keeper.as_ref(),
            channel,
            peer,
        );
        if let Some(end) = end {
            eprintln!("{peer} left: {end:?}");
        }
    }
}

/// What `--linger` asks for, if anything. Only a key a proof named can be told apart from another, so nothing else has it.
fn linger(args: &Serve, no_key: bool) -> Result<Option<Settings>, String> {
    let Some(seconds) = args.linger.filter(|seconds| *seconds > 0) else {
        return Ok(None);
    };
    if no_key {
        return Err(
            "--linger needs the in-band proof: an agent goes back only to the key that proved itself"
                .to_owned(),
        );
    }
    let keep = Duration::from_secs(seconds);
    if Instant::now().checked_add(keep).is_none() {
        return Err("--linger is longer than this machine's clock can count".to_owned());
    }
    let mut settings = Settings::new(keep);
    settings.per_peer = args.linger_per_peer;
    settings.total = args.linger_total;
    settings.buffer = args.linger_buffer_bytes;
    settings.settle = Duration::from_millis(args.linger_settle_ms);
    Ok(Some(settings))
}

/// Keeps agents by `settings` for peers `store` still accepts, or with no store, for every peer the command line allowed.
fn keeper(settings: Settings, store: Option<&Arc<TrustStore>>, admit: Option<Admit>) -> Keeper {
    let report = |said: &str| eprintln!("{said}");
    match store {
        Some(store) => {
            let store = Arc::clone(store);
            let accepted = move |peer: &str| {
                peer.parse::<PeerId>().is_ok_and(|peer| {
                    store.is_accepted(&peer)
                        && admit.as_ref().is_none_or(|admit| admit.allows(&peer))
                })
            };
            Keeper::new(settings, accepted, report)
        }
        None => Keeper::new(settings, |_: &str| true, report),
    }
}

/// `--linger-buffer-bytes`, which holds at most [`linger::MAX_BUFFER`] for each agent.
fn buffer_bytes(text: &str) -> Result<usize, String> {
    let bytes: usize = text.parse().map_err(|error| format!("{error}"))?;
    if bytes > linger::MAX_BUFFER {
        return Err(format!("at most {} bytes", linger::MAX_BUFFER));
    }
    Ok(bytes)
}

/// Takes connections for ever, each handshake and proof on a thread of its own, so one slow peer holds up nobody. Past the
/// ceiling of unproven connections a new one is dropped: that many at once is an attack, or the ceiling needs raising.
fn accept_loop<P, C>(
    next: impl Fn() -> Result<P, Failure>,
    handshake: impl Fn(P) -> Result<C, Failure> + Clone + Send + 'static,
    service: &Arc<Service>,
) -> !
where
    P: Send + 'static,
    C: Channel + 'static,
{
    loop {
        let pending = match next() {
            Ok(pending) => pending,
            Err(failure) => {
                eprintln!("a connection failed: {failure}");
                continue;
            }
        };
        let Some(entered) = service.unproven.enter() else {
            eprintln!("a connection was dropped: too many are unproven");
            continue;
        };
        let service = Arc::clone(service);
        let handshake = handshake.clone();
        let spawned = thread::Builder::new()
            .name("coqui-acp-bridge-peer".to_owned())
            .spawn(move || match handshake(pending) {
                Ok(channel) => service.run(&channel, entered),
                Err(failure) => eprintln!("a connection failed: {failure}"),
            });
        if let Err(error) = spawned {
            eprintln!("a connection was dropped: {error}");
        }
    }
}

/// Publishes what `--advertise` asked for. The result has to live as long as the listener.
fn advertise(
    args: &Serve,
    bound: SocketAddr,
    peer: Option<(String, Method)>,
    webtransport: Option<u16>,
    webtransport_hash: Option<String>,
) -> Result<Option<Advertised>, String> {
    let Some(label) = &args.advertise else {
        return Ok(None);
    };
    let text = |error: coqui_agentic_peer::Error| error.to_string();
    if !bound.ip().is_loopback() && peer.is_none() {
        return Err(
            "--advertise on the network needs the in-band proof: a client has to be able to check the key a record names"
                .to_owned(),
        );
    }
    if bound.ip().is_loopback() && args.advertise_host.is_some() {
        return Err(
            "--advertise-host names a host on the network, and this listens on loopback".to_owned(),
        );
    }
    let offers: Vec<String> = if args.services.is_empty() && !args.forward && !args.mcp_offers {
        Vec::new()
    } else {
        core::iter::once(ACP.to_owned())
            .chain(
                args.services
                    .iter()
                    .filter_map(|text| parse_service(text).ok().map(|(token, _)| token)),
            )
            .chain(args.forward.then(|| forward::TCP_PROTOCOL.to_owned()))
            .chain(
                args.mcp_offers
                    .then(|| mcp_offer::OFFER_PROTOCOL.to_owned()),
            )
            .collect()
    };
    let advertisement =
        Advertisement::new(label, args.tls_identity.is_some() || args.tls_short_lived)
            .map_err(text)?
            .with_peer(peer.as_ref().map(|(id, _)| id.clone()))
            .with_method(peer.map_or(Method::WebCrypto, |(_, method)| method))
            .with_webtransport(webtransport)
            .with_webtransport_hash(webtransport_hash)
            .with_offers(&offers)
            .with_host(args.advertise_host.clone())
            .map_err(text)?;
    discovery::advertise(&advertisement, bound)
        .map(Some)
        .map_err(text)
}

/// Runs `serve` until the process ends: only a failure to start returns.
///
/// # Errors
///
/// The arguments do not fit together, a key or certificate could not be read, or a listener could not be bound.
pub fn run(args: &Serve) -> Result<u8, String> {
    crate::common::check_origins(&args.origins)?;
    let in_band = !args.allow.is_empty() || args.allow_store.is_some() || args.paired;
    let certificates =
        !args.allow_cert.is_empty() || args.allow_cert_store.is_some() || args.client_ca.is_some();
    let modes = [in_band, certificates, args.no_peer_proof];
    if modes.iter().filter(|mode| **mode).count() != 1
        || (!args.allow.is_empty() && args.allow_store.is_some())
    {
        return Err(
            "give one proof: --allow or --allow-store; client certificates; or --no-peer-proof"
                .to_owned(),
        );
    }
    let admit = admission(&args.permit, &args.deny)?;
    let unproven = Gate::new(MAX_UNPROVEN);
    let running = Gate::new(args.max_peers);
    if !args.grants.is_empty() && args.services.is_empty() && !args.forward && !args.mcp_offers {
        return Err(
            "--grant needs --service, --forward or --mcp-offers: otherwise there is nothing to grant".to_owned(),
        );
    }
    if (!args.services.is_empty() || args.forward || args.mcp_offers || !args.reverses.is_empty())
        && (args.no_peer_proof || certificates)
    {
        return Err("--service, --forward, --mcp-offers and --reverse need the in-band proof: streams are for a link a proof named a peer on".to_owned());
    }
    if args.tls_short_lived && (args.no_peer_proof || certificates) {
        return Err("--tls-short-lived needs the in-band proof: a client cannot pin this certificate for long".to_owned());
    }
    let linger = linger(args, args.no_peer_proof || certificates)?;
    if args.no_peer_proof {
        return serve_unproven(args, admit, unproven, running);
    }
    if certificates {
        // Silently dropping these would leave an operator believing they apply.
        if args.identity.is_some()
            || args.proof_seconds.is_some()
            || args.method.method.is_some()
            || args.session.key_limit.is_some()
            || args.session.no_rekey
        {
            return Err("--identity, --method, --key-limit, --no-rekey and --proof-seconds belong to the in-band proof".to_owned());
        }
        if args.webtransport_port.is_some() {
            return Err(
                "WebTransport has no client certificate here: it needs the in-band proof"
                    .to_owned(),
            );
        }
        let path = args
            .tls_identity
            .as_deref()
            .ok_or("mutual TLS needs --tls-identity")?;
        let pins = match &args.allow_cert_store {
            Some(_) if !args.allow_cert.is_empty() => {
                return Err("give --allow-cert or --allow-cert-store, not both".to_owned());
            }
            Some(store) => {
                tls::Pins::store(TrustStore::open(store).map_err(|error| error.to_string())?)
            }
            None => tls::Pins::fixed(peers(&args.allow_cert)?),
        };
        let pinned = !args.allow_cert.is_empty() || args.allow_cert_store.is_some();
        let trust = match &args.client_ca {
            Some(file) if pinned => tls::Trust::PinnedOrCa {
                pins,
                ca: tls::CaPolicy::new(file),
            },
            Some(file) => tls::Trust::Ca(tls::CaPolicy::new(file)),
            None => tls::Trust::Pinned(pins),
        };
        // This builder turns resumption off, so a peer that lost trust cannot return on a session.
        let identity = read_tls_identity(path)?;
        let built = tls::ServerBuilder::new()
            .identity(&identity)
            .client_auth(tls::ClientAuth::Required(trust))
            .build()
            .map_err(|error| error.to_string())?;
        let listener = websocket::Listener::builder(args.listen, Arc::new(built.config))
            .limits(limits())
            .origins(args.origins.clone())
            .admission(admit)
            .build()
            .map_err(|failure| failure.to_string())?;
        let bound = listener
            .local_addr()
            .map_err(|failure| failure.to_string())?;
        let _advertised = advertise(args, bound, None, None, None)?;
        eprintln!("listening on {bound}");
        let service = Arc::new(Service {
            admit: args.admit.clone(),
            helper: args.helper.clone(),
            method: None,
            store: None,
            proof: Duration::ZERO,
            agent: args.agent.clone(),
            unproven,
            peers: running,
            keeper: None,
            links: None,
            reverses: Vec::new(),
        });
        accept_loop(
            || listener.next(None),
            websocket::Pending::handshake,
            &service,
        );
    }
    let identity = if args.paired {
        proof_identity(args.home.as_deref(), None, args.method, true)?
    } else {
        let identity = args
            .identity
            .as_deref()
            .ok_or("the in-band proof needs --identity (see keygen), or --paired")?;
        proof_identity(args.home.as_deref(), Some(identity), args.method, false)?
    };
    let peer_id = identity.peer_id();
    let store = if args.paired {
        Some(Arc::new(store_of(args.home.as_deref(), None)?))
    } else {
        args.allow_store
            .as_deref()
            .map(TrustStore::open)
            .transpose()
            .map_err(|error| error.to_string())?
            .map(Arc::new)
    };
    let trust = match &store {
        Some(store) => Trust::Store(TrustStore::clone(store)),
        None => Trust::pinned(peers(&args.allow)?),
    };
    let method = built(proof_of(identity, args.session).initiators(trust))?;
    println!("{peer_id}");
    let _ = std::io::stdout().flush();
    let proof = Duration::from_secs(args.proof_seconds.unwrap_or(15));
    let short_lived = args
        .tls_short_lived
        .then(short_lived_identity)
        .transpose()?;
    let tls_config = match &short_lived {
        Some(identity) => {
            let built = tls::ServerBuilder::new()
                .identity(identity)
                .client_auth(tls::ClientAuth::None)
                .build()
                .map_err(|error| error.to_string())?;
            Some(Arc::new(built.config))
        }
        None => args.tls_identity.as_deref().map(server_tls).transpose()?,
    };
    let plain = websocket::Listener::builder(args.listen, server_tls_of(tls_config.clone()))
        .limits(unproven_limits())
        .frames(Frames::Binary)
        .origins(args.origins.clone())
        .admission(admit.clone())
        .build()
        .map_err(|failure| failure.to_string())?;
    // Every listener is bound before anything says this agent exists, so what is announced is there.
    let quic = args
        .webtransport_port
        .map(|port| {
            let tls_config = tls_config.ok_or("WebTransport needs --tls-identity")?;
            let address = SocketAddr::new(args.listen.ip(), port);
            let listener = webtransport::Listener::builder(address, tls_config)
                .limits(unproven_limits())
                .origins(args.origins.clone())
                .admission(admit)
                .build()
                .map_err(|failure| failure.to_string())?;
            Ok::<_, String>(listener)
        })
        .transpose()?;
    let bound = plain.local_addr().map_err(|failure| failure.to_string())?;
    let hash = short_lived
        .as_ref()
        .map(|identity| identity.fingerprint().to_string());
    if let Some(hash) = &hash {
        eprintln!("certificate {hash}, for 13 days");
    }
    let short_lived_run = hash.is_some();
    let advertised = advertise(
        args,
        bound,
        Some((peer_id.to_string(), method.method())),
        args.webtransport_port,
        hash,
    )?;
    let _advertised = if short_lived_run {
        end_with_the_certificate(advertised);
        None
    } else {
        advertised
    };
    eprintln!("listening on {bound}");
    say_proof(&method);
    let keeper = linger.map(|settings| keeper(settings, store.as_ref(), args.admit.clone()));
    let links = links(args, &running, keeper.as_ref())?;
    let reverses = args
        .reverses
        .iter()
        .map(|text| parse_reverse(text))
        .collect::<Result<Vec<_>, _>>()?;
    let service = Arc::new(Service {
        admit: args.admit.clone(),
        helper: args.helper.clone(),
        method: Some(method),
        store,
        proof,
        agent: args.agent.clone(),
        unproven,
        peers: running,
        keeper,
        links,
        reverses,
    });
    if let Some(quic) = quic {
        let service = Arc::clone(&service);
        thread::spawn(move || {
            accept_loop(
                || quic.next(None),
                webtransport::Pending::handshake,
                &service,
            )
        });
    }
    accept_loop(|| plain.next(None), websocket::Pending::handshake, &service)
}

/// `serve --no-peer-proof`: the agent for whoever can connect, so who can connect has to be limited.
fn serve_unproven(
    args: &Serve,
    admit: impl Fn(SocketAddr) -> bool + Clone + Send + Sync + 'static,
    unproven: Gate,
    running: Gate,
) -> Result<u8, String> {
    if args.identity.is_some() || args.proof_seconds.is_some() || args.webtransport_port.is_some() {
        return Err("--identity, --proof-seconds and --webtransport-port need a proof".to_owned());
    }
    if !args.listen.ip().is_loopback() && args.permit.is_empty() {
        return Err(
            "--no-peer-proof beyond loopback needs --permit, to say who can reach it".to_owned(),
        );
    }
    let tls_config = args.tls_identity.as_deref().map(server_tls).transpose()?;
    // An empty list checks nothing, and a page on any site could then drive an agent on this machine.
    let origins = if args.origins.is_empty() {
        vec!["none".to_owned()]
    } else {
        args.origins.clone()
    };
    let listener = websocket::Listener::builder(args.listen, server_tls_of(tls_config))
        .limits(limits())
        .origins(origins)
        .admission(admit)
        .build()
        .map_err(|failure| failure.to_string())?;
    let bound = listener
        .local_addr()
        .map_err(|failure| failure.to_string())?;
    let _advertised = advertise(args, bound, None, None, None)?;
    eprintln!(
        "listening on {bound}, with no proof of the peer: whoever can connect gets the agent"
    );
    let service = Arc::new(Service {
        admit: args.admit.clone(),
        helper: args.helper.clone(),
        method: None,
        store: None,
        proof: Duration::ZERO,
        agent: args.agent.clone(),
        unproven,
        peers: running,
        keeper: None,
        links: None,
        reverses: Vec::new(),
    });
    accept_loop(
        || listener.next(None),
        websocket::Pending::handshake,
        &service,
    )
}
