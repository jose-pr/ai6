//! `pair-accept` and `pair-dial`: pair two peers with a PIN, one side waiting and the other joining.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cabuya::pairing;
use coqui_peer::pair::{CodeAlphabet, Pin};
use coqui_transport::{Channel as _, Failure};
use coqui_transport_web::websocket::Frames;
use coqui_transport_web::{Dialer as _, Kind, websocket};
use url::Url;

use crate::common::{
    Gate, MethodArgs, admission, in_band_tls, proof_identity, server_tls, server_tls_of, store_of,
    unproven_limits,
};

/// Connections to one `pair-accept` at once.
const MAX_PAIRING_RUNS: usize = 16;
/// No browser sends this as its `Origin`, so a listener allowing only it takes only clients that send none: native programs.
const NO_BROWSER: &str = "-";

/// The arguments of `pair-accept`: where to wait, and who may use the PIN.
#[derive(clap::Args, Clone, Debug)]
pub struct PairAccept {
    /// The address to wait on, for a WebSocket. On loopback it is plain, for a reverse proxy in front; any other address
    /// needs `--tls-identity`. The PIN authenticates either way.
    #[arg(long)]
    pub listen: SocketAddr,
    /// This side's identity, whose key is the one paired. Created if absent. Without it, this user's own.
    #[arg(long)]
    pub identity: Option<PathBuf>,
    /// A TLS identity for wss://, when this address is not loopback.
    #[arg(long)]
    pub tls_identity: Option<PathBuf>,
    /// The directory of the trust store that records the peer. Without it, this user's own, which `serve --paired` reads.
    #[arg(long)]
    pub store: Option<PathBuf>,
    /// How long the PIN waits for someone to use it, in seconds: at most the ten minutes the PIN lives.
    #[arg(long, default_value_t = 600)]
    pub timeout_seconds: u64,
    /// How long one connection may take to pair, in seconds, from its handshake to the end of the exchange. A browser's
    /// extension waits as long by default.
    #[arg(long, default_value_t = 120)]
    pub run_seconds: u64,
    /// A browser origin allowed to pair, such as chrome-extension://ID for the extension. Repeat for more. With none listed, a
    /// browser is refused before its handshake, so a page cannot spend the PIN; a native client sends no origin.
    #[arg(long = "origin")]
    pub origins: Vec<String>,
    /// The proof's method.
    #[command(flatten)]
    pub method: MethodArgs,
    /// A source network allowed to connect at all. Repeat for more. With any listed, everything else is dropped first.
    #[arg(long = "permit")]
    pub permit: Vec<String>,
    /// A source network dropped before its handshake. Repeat for more. Takes precedence over `--permit`.
    #[arg(long = "deny")]
    pub deny: Vec<String>,
    /// The directory of the identity, trust store and grants, when the program that embeds this keeps them somewhere else than
    /// `coqui_agentic_peer::home` does. Not a command-line argument.
    #[arg(skip)]
    pub home: Option<PathBuf>,
}

/// The arguments of `pair-dial`: the waiting side's URL, and where the PIN comes from.
#[derive(clap::Args, Clone, Debug)]
pub struct PairDial {
    /// The URL of the `pair-accept`, any path, such as wss://host/v1/agent/pair. ws:// is for loopback only.
    pub url: Url,
    /// This side's identity, whose key is the one paired. Created if absent. Without it, this user's own.
    #[arg(long)]
    pub identity: Option<PathBuf>,
    /// The directory of the trust store that records the peer. Without it, this user's own, which `dial --paired` reads.
    #[arg(long)]
    pub store: Option<PathBuf>,
    /// A file holding the PIN; without it, standard input. A PIN is never an argument, since arguments are visible to
    /// other users.
    #[arg(long)]
    pub pin_file: Option<PathBuf>,
    /// A PEM bundle the server's certificate must chain to. Without it any certificate is taken for this exchange, since the
    /// PIN authenticates the peer, not TLS.
    #[arg(long)]
    pub ca_file: Option<PathBuf>,
    /// How long the exchange may take, in seconds: at most the ten minutes a PIN lives.
    #[arg(long, default_value_t = 600)]
    pub timeout_seconds: u64,
    /// The proof's method.
    #[command(flatten)]
    pub method: MethodArgs,
    /// The directory of the identity, trust store and grants, when the program that embeds this keeps them somewhere else than
    /// `coqui_agentic_peer::home` does. Not a command-line argument.
    #[arg(skip)]
    pub home: Option<PathBuf>,
}

/// Waits for one peer to pair with a PIN it shows, then ends.
///
/// # Errors
///
/// The PIN expired or was used up, nobody paired in time, or a listener could not be bound.
pub fn accept(args: &PairAccept) -> Result<u8, String> {
    crate::common::check_origins(&args.origins)?;
    let identity = Arc::new(proof_identity(
        args.home.as_deref(),
        args.identity.as_deref(),
        args.method,
        true,
    )?);
    eprintln!("pairing by {}", identity.method());
    let store = Arc::new(store_of(args.home.as_deref(), args.store.as_deref())?);
    let tls_config = args.tls_identity.as_deref().map(server_tls).transpose()?;
    let listener = websocket::Listener::builder(args.listen, server_tls_of(tls_config))
        .frames(Frames::Binary)
        .limits(unproven_limits())
        .origins(if args.origins.is_empty() {
            vec![NO_BROWSER.to_owned()]
        } else {
            args.origins.clone()
        })
        .admission(admission(&args.permit, &args.deny)?)
        .build()
        .map_err(|failure| failure.to_string())?;
    let run = Duration::from_secs(args.run_seconds);
    let desk = pairing::desk().map_err(|error| error.to_string())?;
    let shown = desk.show_pin().map_err(|error| error.to_string())?;
    println!("{}", shown.pin.as_str());
    println!("{}", identity.peer_id());
    let _ = std::io::stdout().flush();
    let until = Instant::now() + Duration::from_secs(args.timeout_seconds);
    let (report, reports) = mpsc::channel();
    let gate = Gate::new(MAX_PAIRING_RUNS);
    // Each connection runs apart, so a scanner that connects and says nothing costs the person neither the PIN nor the wait.
    // A proxy's address says nothing about the peer, so the desk is not given one.
    loop {
        while let Ok(result) = reports.try_recv() {
            match result {
                Ok(peer) => {
                    eprintln!("paired with {peer}");
                    return Ok(0);
                }
                Err(why) => {
                    eprintln!("a pairing attempt failed: {why}");
                    if desk.shown_pin().is_none() {
                        return Err("the PIN is used up or expired: start again".to_owned());
                    }
                }
            }
        }
        if Instant::now() >= until {
            return Err("nobody paired in time".to_owned());
        }
        let pending = match listener.next(Some(Duration::from_millis(100))) {
            Ok(pending) => pending,
            Err(Failure::Timeout) => continue,
            Err(failure) => return Err(failure.to_string()),
        };
        let Some(entered) = gate.enter() else {
            continue;
        };
        let (identity, store, desk, report) = (
            Arc::clone(&identity),
            Arc::clone(&store),
            desk.clone(),
            report.clone(),
        );
        thread::spawn(move || {
            let _entered = entered;
            let result = pending
                .handshake()
                .map_err(|failure| failure.to_string())
                .and_then(|channel| {
                    let paired = pairing::respond(&channel, &identity, &store, &desk, None, run);
                    channel.close();
                    paired.map_err(|error| error.to_string())
                })
                .map(|paired| paired.peer.id().to_string());
            let _ = report.send(result);
        });
    }
}

/// Pairs with a waiting `accept`, using its PIN.
///
/// # Errors
///
/// The PIN is malformed or wrong, or the other side could not be reached.
pub fn join(args: &PairDial) -> Result<u8, String> {
    let text = match &args.pin_file {
        Some(file) => std::fs::read_to_string(file)
            .map_err(|error| format!("the PIN file could not be read: {error}"))?,
        None => {
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|error| error.to_string())?;
            line
        }
    };
    let pin = Pin::parse(&CodeAlphabet::pin(), text.trim())
        .map_err(|_| "that is not a PIN".to_owned())?;
    let identity = proof_identity(
        args.home.as_deref(),
        args.identity.as_deref(),
        args.method,
        true,
    )?;
    eprintln!("pairing by {}", identity.method());
    let store = store_of(args.home.as_deref(), args.store.as_deref())?;
    let mut dialer = websocket::Dialer::builder()
        .frames(Frames::Binary)
        .limits(unproven_limits());
    if args.url.scheme() == "wss" {
        dialer = dialer.tls(in_band_tls(args.ca_file.as_ref())?);
    }
    let mut dialer = dialer.build().map_err(|error| error.to_string())?;
    let limit = Duration::from_secs(args.timeout_seconds);
    let channel = dialer
        .dial(
            Kind::WebSocket,
            &args.url,
            limit.min(Duration::from_secs(10)),
        )
        .map_err(|failure| failure.to_string())?;
    let paired = pairing::join(&channel, &identity, pin, &store, limit)
        .map_err(|error| error.to_string())?;
    channel.close();
    println!("{}", paired.peer.id());
    Ok(0)
}
