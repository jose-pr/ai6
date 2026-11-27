//! `ai6ctl liaison loopback`: Liaison's A2A link served as the standard A2A HTTP binding on a
//! loopback address, for A2A software that knows nothing of links.

use std::io::{self, Read as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use cabuya::Error;
use cabuya::auth::{self, Proof, Sealed, Trust};
use cabuya::mux::{Heartbeat, Limits, Mux, Side};
use clap::Subcommand;
use coqui_a2a::loopback::{Config, Hub, Loopback};
use coqui_agentic_peer::home;
use coqui_peer_webcrypto::{PeerId, TrustStore};
use coqui_transport::{Channel as _, Failure};
use coqui_transport_web::websocket::{Dialer, Frames};
use coqui_transport_web::{Dialer as _, Kind};
use url::Url;

/// The file under the link home that holds the token, unless another is named.
const TOKEN_FILE: &str = "a2a-loopback.token";
const TOKEN_BYTES: usize = 32;
/// More than any token, so a file named by mistake is refused rather than read.
const MAX_TOKEN_FILE: u64 = 4096;

#[derive(Debug, Subcommand)]
pub(crate) enum LiaisonCommand {
    /// Serve a standard A2A HTTP endpoint on a loopback address and carry it over a link to
    /// Liaison.
    ///
    /// Prints the endpoint's URL, then the token file's path unless --no-token, on standard
    /// output, and runs until it is stopped, dialing the link again after it ends. It fails when
    /// the remote does not prove itself as one of the peers named. Does not use the Universal
    /// Exports origin or token; the global --ca-file is the roots a wss:// link's certificate
    /// must chain to.
    Loopback(LoopbackArgs),
}

#[derive(Debug, clap::Args)]
pub(crate) struct LoopbackArgs {
    /// The link to dial: ws:// (loopback only is sensible) or wss://.
    #[arg(long, env = "COQUI_A2A_URL")]
    url: Url,
    /// Where to listen: a loopback address. Port 0 picks a free port, printed on standard output.
    #[arg(long, env = "COQUI_A2A_LISTEN", default_value = "127.0.0.1:0")]
    listen: SocketAddr,
    /// The remote peer's id, pinned, repeatable (the gateway prints its id at start). Required
    /// unless --any-paired-peer is given.
    #[arg(long = "peer", required_unless_present = "any_paired_peer")]
    peers: Vec<PeerId>,
    /// Accepts every peer paired in this user's trust store as the remote, so any of them can
    /// pose as it to whatever uses this endpoint.
    #[arg(long, conflicts_with = "peers")]
    any_paired_peer: bool,
    /// This side's identity file, whose key's method is the one it proves by (default: this
    /// user's, in the link home).
    #[arg(long)]
    identity: Option<PathBuf>,
    /// Lets a page from this Origin use the endpoint, repeatable; by default any request with an
    /// Origin is refused.
    #[arg(long = "origin")]
    origins: Vec<String>,
    /// Passes this header to the remote as a service parameter, repeatable.
    #[arg(long = "forward-header", value_name = "NAME")]
    forwarded: Vec<String>,
    /// How long the remote has to say anything to an operation, in seconds (default: the
    /// library's). SendMessage waits for the task to settle, so a longer task needs more, or
    /// returnImmediately and GetTask.
    #[arg(long, value_name = "SECONDS")]
    answer_timeout: Option<u64>,
    /// A file holding the bearer token every request must present, which only this user may
    /// read (default: this user's own in the link home, made the first time).
    #[arg(long, env = "COQUI_A2A_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    /// Serves with no token: every program of every user on this machine can then do what the
    /// link's remote granted, and read the extended card.
    #[arg(long, conflicts_with = "token_file")]
    no_token: bool,
    /// Seconds to wait for each connection to the link to open.
    #[arg(long, value_name = "SECONDS", default_value_t = 10)]
    dial_timeout_seconds: u64,
    /// Seconds the remote has to finish proving itself once connected.
    #[arg(long, value_name = "SECONDS", default_value_t = 15)]
    proof_timeout_seconds: u64,
    /// Seconds between the heartbeats asked of the remote on an open link.
    #[arg(long, value_name = "SECONDS", default_value_t = 15)]
    heartbeat_seconds: u64,
    /// Heartbeat intervals with nothing heard after which the link is taken as lost.
    #[arg(long, value_name = "COUNT", default_value_t = 3)]
    heartbeat_misses: u32,
    /// Seconds to wait before dialing again after a link fails or ends; doubles on each failure.
    #[arg(long, value_name = "SECONDS", default_value_t = 1)]
    backoff_start_seconds: u64,
    /// Seconds the wait before dialing again grows to at most.
    #[arg(long, value_name = "SECONDS", default_value_t = 30)]
    backoff_max_seconds: u64,
}

/// Runs the command; it returns only with the reason it stopped.
pub(crate) async fn run(command: LiaisonCommand, ca_file: Option<PathBuf>) -> Result<(), String> {
    match command {
        LiaisonCommand::Loopback(args) => {
            tokio::task::spawn_blocking(move || loopback(&args, ca_file.as_deref()))
                .await
                .map_err(|_| "the loopback command failed".to_owned())?
        }
    }
}

fn loopback(args: &LoopbackArgs, ca_file: Option<&Path>) -> Result<(), String> {
    let mut config = Config::new(args.listen).map_err(|error| error.to_string())?;
    for origin in &args.origins {
        config = config.allow_origin(origin.clone());
    }
    for name in &args.forwarded {
        config = config.forward_header(name);
    }
    if let Some(seconds) = args.answer_timeout {
        config = config.with_answer_timeout(Duration::from_secs(seconds));
    }
    let token_file = if args.no_token {
        config = config.without_token();
        None
    } else {
        let file = match &args.token_file {
            Some(file) => file.clone(),
            None => default_token_file()?,
        };
        config = config.with_token(token_from(&file, args.token_file.is_none())?);
        Some(file)
    };
    let identity =
        home::identity(args.identity.as_deref(), None).map_err(|error| error.to_string())?;
    let (trust, store) = if args.peers.is_empty() {
        let store = Arc::new(home::store(None).map_err(|error| error.to_string())?);
        (Trust::Store(TrustStore::clone(&store)), Some(store))
    } else {
        (Trust::pinned(args.peers.iter().copied()), None)
    };
    let method = Proof::builder(identity)
        .responders(trust)
        .build()
        .map_err(|error| error.to_string())?;

    let hub = Hub::new();
    let endpoint = Loopback::start(config, Arc::clone(&hub)).map_err(|error| error.to_string())?;
    println!("http://{}/", endpoint.local_addr());
    if let Some(file) = &token_file {
        println!("{}", file.display());
    }

    let mut dialer = Dialer::builder().frames(Frames::Binary);
    if args.url.scheme() == "wss" {
        dialer = dialer.tls(tls_client(ca_file)?);
    }
    let mut dialer = dialer.build().map_err(|error| error.to_string())?;
    let max_message = Limits::default().max_message;
    let first_wait = Duration::from_secs(args.backoff_start_seconds);
    let longest_wait = Duration::from_secs(args.backoff_max_seconds);
    let mut wait = first_wait;
    loop {
        match dialer.dial(
            Kind::WebSocket,
            &args.url,
            Duration::from_secs(args.dial_timeout_seconds),
        ) {
            Ok(channel) => {
                let proved = auth::prove_out(
                    &method,
                    &channel,
                    Duration::from_secs(args.proof_timeout_seconds),
                    method.method().record_length(max_message),
                );
                match proved {
                    Ok(established) => {
                        wait = first_wait;
                        let peer = established.peer;
                        eprintln!("connected to {peer}");
                        let sealed = Sealed::new(&channel, established.session);
                        let sealed = match &store {
                            Some(store) => {
                                let store = Arc::clone(store);
                                sealed.with_check(move || store.is_accepted(&peer))
                            }
                            None => sealed,
                        };
                        let mut limits = Limits::default();
                        limits.heartbeat = Some(Heartbeat::new(
                            Duration::from_secs(args.heartbeat_seconds),
                            args.heartbeat_misses,
                        ));
                        Mux::run(&sealed, Side::Dialed, limits, None, |mux| hub.attach(mux));
                        sealed.close();
                        eprintln!("the link ended");
                    }
                    // A remote that proved the wrong key will not fix itself.
                    Err(Error::Proof(why)) => {
                        return Err(format!("the remote did not prove itself: {why}"));
                    }
                    Err(error) => eprintln!("the proof did not finish: {error}"),
                }
                channel.close();
            }
            Err(Failure::Certificate) => {
                return Err("the remote certificate is not accepted".to_owned());
            }
            Err(failure) => eprintln!("could not connect: {failure}"),
        }
        thread::sleep(wait);
        wait = (wait * 2).min(longest_wait);
    }
}

fn default_token_file() -> Result<PathBuf, String> {
    home::dir()
        .map(|dir| dir.join(TOKEN_FILE))
        .ok_or_else(|| "no home directory for this user to keep a token in".to_owned())
}

/// The token in `file`. When `make` says so a missing file is made with a new token, and when two
/// starts race, the one that loses reads what the other wrote.
fn token_from(file: &Path, make: bool) -> Result<String, String> {
    let shown = file.display();
    let mut opened = std::fs::File::open(file);
    if make
        && let Err(error) = &opened
        && error.kind() == io::ErrorKind::NotFound
    {
        match new_token().and_then(|token| write_token(file, &token)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(format!("the token file {shown} could not be made: {error}")),
        }
        opened = std::fs::File::open(file);
    }
    let mut opened =
        opened.map_err(|error| format!("the token file {shown} could not be read: {error}"))?;
    private_enough(&opened).map_err(|why| format!("the token file {shown} {why}"))?;
    let mut text = String::new();
    opened
        .read_to_string(&mut text)
        .map_err(|error| format!("the token file {shown} could not be read: {error}"))?;
    let token = text.trim();
    if token.is_empty() {
        return Err(format!("the token file {shown} is empty"));
    }
    Ok(token.to_owned())
}

fn new_token() -> io::Result<String> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    // A guessable token lets anyone on this machine in, so no randomness means no endpoint.
    coqui_peer_webcrypto::random()
        .fill(&mut bytes)
        .map_err(|_| io::Error::other("this machine has no randomness to make one with"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn write_token(file: &Path, token: &str) -> io::Result<()> {
    coqui_peer::file::write_new(file, token.as_bytes(), coqui_peer::file::Access::OwnerOnly)
}

/// Refuses a file others could read the token from. On Windows a new file takes its directory's
/// access list, which is this user's alone under `%LOCALAPPDATA%`, and this does not read it.
fn private_enough(file: &std::fs::File) -> Result<(), String> {
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not be read: {error}"))?;
    if !metadata.is_file() {
        return Err("is not a file".to_owned());
    }
    if metadata.len() > MAX_TOKEN_FILE {
        return Err("is too large to hold a token".to_owned());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = metadata.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(format!(
                "can be reached by other users (mode {:o}): make it 600",
                mode & 0o777
            ));
        }
    }
    Ok(())
}

/// The TLS settings of a wss:// link: the in-band proof, not the certificate, decides who the
/// remote is, unless `ca_file` names roots that must vouch for it.
fn tls_client(ca_file: Option<&Path>) -> Result<Arc<rustls::ClientConfig>, String> {
    let trust = match ca_file {
        Some(file) => coqui_peer_tls::Trust::Roots(coqui_peer_tls::Roots::bundle_file(file)),
        None => coqui_peer_tls::Trust::AnyWithKeyProof,
    };
    let built = coqui_peer_tls::ClientBuilder::new()
        .server(trust)
        .build()
        .map_err(|error| error.to_string())?;
    Ok(Arc::new(built.config))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    #[test]
    fn the_peer_is_named_or_explicitly_left_open() {
        let parse = |extra: &[&str]| {
            let mut line = vec![
                "ai6ctl",
                "liaison",
                "loopback",
                "--url",
                "ws://127.0.0.1:1/x",
            ];
            line.extend_from_slice(extra);
            <crate::Cli as clap::Parser>::try_parse_from(line)
        };
        let error = parse(&[]).unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().contains("--peer"), "{error}");
        assert!(parse(&["--any-paired-peer"]).is_ok());
        crate::Cli::command().debug_assert();
    }
}
