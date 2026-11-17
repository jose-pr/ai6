//! `dial`: connect out to a peer, for an agent behind NAT; reconnect and keep the agent running.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use coqui_agentic_peer::Error;
use coqui_agentic_peer::auth::{self, Proof, Sealed, Trust};
use coqui_peer::TrustStore;
use coqui_peer_tls as tls;
use coqui_transport::{Channel as _, Failure};
use coqui_transport_web::websocket::Frames;
use coqui_transport_web::{Dialer as _, Kind, websocket};
use url::Url;

use crate::agent::Agent;
use crate::common::{
    DIAL_PROOF, MAX_LINE, MethodArgs, SessionArgs, built, in_band_tls, limits, peers,
    proof_identity, proof_of, say_proof, split_agent, status_code, store_of, unproven_limits,
};
use crate::relay::{LinkEnd, relay};

/// The arguments of `dial`: where to connect, what to prove and check, and the agent to keep running.
#[derive(clap::Args, Clone, Debug)]
pub struct Dial {
    /// The URL to connect to: wss://, or ws:// on loopback for the in-band proof.
    pub url: Url,
    /// Another endpoint to try when the one before cannot be reached, such as a second gateway. Repeat for more. Each has to
    /// prove the same thing, since nothing else about it is checked differently.
    #[arg(long = "fallback", value_name = "URL")]
    pub fallbacks: Vec<Url>,
    /// This side's identity, for the in-band proof, which works through a reverse proxy. Created if absent. Everything after
    /// the proof is inside its session.
    #[arg(long)]
    pub identity: Option<PathBuf>,
    /// Prove the remote against whoever this user has paired with, under this user's own identity: one pairing serves every
    /// command. Takes the place of `--identity` and `--trust-store`.
    #[arg(long, conflicts_with_all = ["identity", "trust_store", "peers", "tls_identity"])]
    pub paired: bool,
    /// A peer id the remote must prove, with `--identity`. Repeat for more.
    #[arg(long = "peer")]
    pub peers: Vec<String>,
    /// A trust store whose accepted peers the remote may prove, with `--identity`, as `pair-dial` records them.
    #[arg(long)]
    pub trust_store: Option<PathBuf>,
    /// A TLS identity (certificate and key, PEM) to present over mutual TLS, in place of the in-band proof. Created if absent.
    /// A reverse proxy must vouch for the certificate. With neither this nor `--identity`, TLS checks only the server, by
    /// `--expect` or `--ca-file`.
    #[arg(long)]
    pub tls_identity: Option<PathBuf>,
    /// Check nothing about the remote, for a ws:// on loopback.
    #[arg(long)]
    pub no_peer_proof: bool,
    /// A fingerprint the remote certificate may have, with `--tls-identity`. Repeat for more. Needs no certificate authority.
    /// With `--ca-file`, either a listed fingerprint or a certificate from that authority is enough.
    #[arg(long = "expect")]
    pub expect: Vec<String>,
    /// A PEM bundle of certificate authorities the remote certificate must chain to, with its name checked. With the in-band
    /// proof TLS authenticates nothing, so without this any certificate is taken.
    #[arg(long)]
    pub ca_file: Option<PathBuf>,
    /// A file holding a token to send as `Authorization: Bearer`. It needs a checked certificate, and it is never an argument,
    /// since arguments are visible to other users.
    #[arg(long)]
    pub bearer_file: Option<PathBuf>,
    /// The longest wait between attempts to reconnect, in seconds.
    #[arg(long, default_value_t = 60)]
    pub max_backoff_seconds: u64,
    /// The proof's method.
    #[command(flatten)]
    pub method: MethodArgs,
    /// The session's settings.
    #[command(flatten)]
    pub session: SessionArgs,
    /// The agent program, then its arguments.
    #[arg(last = true, required = true)]
    pub agent: Vec<OsString>,
    /// The directory of the identity, trust store and grants, when the program that embeds this keeps them somewhere else than
    /// `coqui_agentic_peer::home` does. Not a command-line argument.
    #[arg(skip)]
    pub home: Option<PathBuf>,
    /// With `--paired`, the trust store the remote is proved against, in place of this user's own: a host that keeps the keys
    /// it dials apart from the keys it serves names it. Not a command-line argument.
    #[arg(skip)]
    pub paired_store: Option<PathBuf>,
}

fn remote_trust(args: &Dial) -> Result<tls::Trust, String> {
    let pins = tls::Pins::fixed(peers(&args.expect)?);
    Ok(match &args.ca_file {
        Some(file) if !args.expect.is_empty() => tls::Trust::PinnedOrRoots {
            pins,
            roots: tls::Roots::bundle_file(file),
        },
        Some(file) => tls::Trust::Roots(tls::Roots::bundle_file(file)),
        None => tls::Trust::Pinned(pins),
    })
}

fn mtls_client(path: &std::path::Path, args: &Dial) -> Result<Arc<rustls::ClientConfig>, String> {
    let identity = tls::Identity::open_or_create(path).map_err(|error| error.to_string())?;
    let trust = remote_trust(args)?;
    // This builder turns resumption off, so a peer that lost trust cannot return on a session.
    let built = tls::ClientBuilder::new()
        .server(trust)
        .identity(&identity)
        .build()
        .map_err(|error| error.to_string())?;
    Ok(Arc::new(built.config))
}

/// How `dial` knows its peer. Without an in-band proof, whatever TLS checked is all there is.
enum DialProof {
    InBand(Proof, Option<Arc<TrustStore>>),
    Transport,
}

fn in_band_dialer(args: &Dial) -> Result<websocket::DialerBuilder, String> {
    let mut dialer = websocket::Dialer::builder()
        .limits(unproven_limits())
        .frames(Frames::Binary);
    if args.url.scheme() == "wss" {
        dialer = dialer.tls(in_band_tls(args.ca_file.as_ref())?);
    }
    Ok(dialer)
}

fn dial_setup(args: &Dial) -> Result<(DialProof, websocket::DialerBuilder), String> {
    if args.paired {
        if !args.expect.is_empty() {
            return Err("--expect belongs to --tls-identity".to_owned());
        }
        if args.bearer_file.is_some() && args.ca_file.is_none() {
            return Err(
                "--bearer-file needs --ca-file, so the token goes only to a checked server"
                    .to_owned(),
            );
        }
        let identity = proof_identity(args.home.as_deref(), None, args.method, true)?;
        let store = Arc::new(store_of(
            args.home.as_deref(),
            args.paired_store.as_deref(),
        )?);
        let method = built(
            proof_of(identity, args.session).responders(Trust::Store(TrustStore::clone(&store))),
        )?;
        return Ok((
            DialProof::InBand(method, Some(store)),
            in_band_dialer(args)?,
        ));
    }
    match (&args.identity, &args.tls_identity) {
        (Some(path), None) => {
            if !args.expect.is_empty() {
                return Err("--expect belongs to --tls-identity; use --peer".to_owned());
            }
            if args.bearer_file.is_some() && args.ca_file.is_none() {
                return Err(
                    "--bearer-file needs --ca-file, so the token goes only to a checked server"
                        .to_owned(),
                );
            }
            let identity = proof_identity(args.home.as_deref(), Some(path), args.method, true)?;
            let store = args
                .trust_store
                .as_deref()
                .map(TrustStore::open)
                .transpose()
                .map_err(|error| error.to_string())?
                .map(Arc::new);
            let trust = match (&store, args.peers.is_empty()) {
                (Some(_), false) | (None, true) => {
                    return Err("give exactly one of --peer and --trust-store".to_owned());
                }
                (Some(store), true) => Trust::Store(TrustStore::clone(store)),
                (None, false) => Trust::pinned(peers(&args.peers)?),
            };
            let method = built(proof_of(identity, args.session).responders(trust))?;
            Ok((DialProof::InBand(method, store), in_band_dialer(args)?))
        }
        (None, Some(path)) => {
            if !args.peers.is_empty() || args.trust_store.is_some() {
                return Err("--peer and --trust-store belong to --identity".to_owned());
            }
            if args.expect.is_empty() && args.ca_file.is_none() {
                return Err("--tls-identity needs --expect or --ca-file".to_owned());
            }
            if args.url.scheme() != "wss" {
                return Err("mutual TLS needs a wss:// URL".to_owned());
            }
            let dialer = websocket::Dialer::builder()
                .tls(mtls_client(path, args)?)
                .limits(limits());
            Ok((DialProof::Transport, dialer))
        }
        (None, None) if args.no_peer_proof => {
            if args.url.scheme() != "ws" || !args.expect.is_empty() || args.ca_file.is_some() {
                return Err("--no-peer-proof is for a ws:// on loopback".to_owned());
            }
            let dialer = websocket::Dialer::builder().limits(limits());
            Ok((DialProof::Transport, dialer))
        }
        (None, None) => {
            if args.url.scheme() != "wss" || (args.expect.is_empty() && args.ca_file.is_none()) {
                return Err(
                    "give --identity, --tls-identity, or a wss:// URL with --expect or --ca-file"
                        .to_owned(),
                );
            }
            if !args.peers.is_empty() || args.trust_store.is_some() {
                return Err("--peer and --trust-store belong to --identity".to_owned());
            }
            let built = tls::ClientBuilder::new()
                .server(remote_trust(args)?)
                .build()
                .map_err(|error| error.to_string())?;
            let dialer = websocket::Dialer::builder()
                .tls(Arc::new(built.config))
                .limits(limits());
            Ok((DialProof::Transport, dialer))
        }
        _ => Err("give --identity or --tls-identity, not both".to_owned()),
    }
}

/// One link's relay. An error here ends `dial`: a remote that proved the wrong key will not fix itself.
fn dial_link(
    proof: &DialProof,
    channel: &websocket::Channel,
    agent: &Agent,
) -> Result<Option<LinkEnd>, String> {
    let DialProof::InBand(method, store) = proof else {
        return Ok(Some(relay(channel, agent)));
    };
    match auth::prove_out(
        method,
        channel,
        DIAL_PROOF,
        method.method().record_length(MAX_LINE),
    ) {
        Ok(established) => {
            let peer = established.peer;
            let sealed = Sealed::new(channel, established.session);
            let sealed = match store {
                Some(store) => {
                    let store = Arc::clone(store);
                    sealed.with_check(move || store.is_accepted(&peer))
                }
                None => sealed,
            };
            let end = relay(&sealed, agent);
            sealed.close();
            Ok(Some(end))
        }
        Err(Error::Proof(why)) => Err(format!("the remote did not prove itself: {why}")),
        Err(error) => {
            eprintln!("the proof did not finish: {error}");
            Ok(None)
        }
    }
}

/// Runs `dial` until the agent ends, or a remote proves the wrong key.
///
/// # Errors
///
/// The arguments do not fit together, or the remote is not the one expected.
pub fn run(args: &Dial) -> Result<u8, String> {
    let (proof, mut dialer) = dial_setup(args)?;
    if let DialProof::InBand(method, _) = &proof {
        say_proof(method);
    }
    let (program, rest) = split_agent(&args.agent)?;
    let agent = Agent::spawn(program, rest, MAX_LINE)
        .map_err(|error| format!("the agent could not start: {error}"))?;
    if let Some(file) = &args.bearer_file {
        let token = std::fs::read_to_string(file)
            .map_err(|error| format!("the bearer file could not be read: {error}"))?;
        dialer = dialer.headers(vec![(
            "authorization".to_owned(),
            format!("Bearer {}", token.trim()),
        )]);
    }
    let mut dialer = dialer.build().map_err(|error| error.to_string())?;
    let mut wait = Duration::from_secs(1);
    let longest = Duration::from_secs(args.max_backoff_seconds);
    if args
        .fallbacks
        .iter()
        .any(|url| url.scheme() != args.url.scheme())
    {
        return Err(
            "a fallback uses the same scheme as the URL: the same checks have to apply".to_owned(),
        );
    }
    let endpoints: Vec<&Url> = core::iter::once(&args.url).chain(&args.fallbacks).collect();
    let mut endpoint = 0;
    loop {
        if let Some(status) = agent.try_status().map_err(|error| error.to_string())? {
            return Ok(status_code(status));
        }
        match dialer.dial(
            Kind::WebSocket,
            endpoints[endpoint],
            Duration::from_secs(10),
        ) {
            Ok(channel) => {
                eprintln!("connected");
                let end = dial_link(&proof, &channel, &agent);
                channel.close();
                let end = end?;
                if let Some(end) = end {
                    wait = Duration::from_secs(1);
                    eprintln!("the link ended: {end:?}");
                    if matches!(end, LinkEnd::AgentEnded | LinkEnd::AgentStuck) {
                        let status = agent.stop().map_err(|error| error.to_string())?;
                        return Ok(status_code(status));
                    }
                    if end == LinkEnd::Failed(Failure::Permission) {
                        return Err("the remote peer is no longer accepted".to_owned());
                    }
                }
            }
            // A rejected certificate will not fix itself: stop rather than knock forever.
            Err(Failure::Certificate) => {
                return Err("the remote certificate is not the expected one".to_owned());
            }
            Err(failure) => {
                eprintln!("could not connect: {failure}");
                endpoint = (endpoint + 1) % endpoints.len();
                // There is another to try, so nothing is gained by waiting first.
                if endpoint != 0 {
                    continue;
                }
            }
        }
        thread::sleep(wait);
        wait = (wait * 2).min(longest);
    }
}
