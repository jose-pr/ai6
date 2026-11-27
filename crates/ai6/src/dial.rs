//! `ai6 theater dial`: a local stdio ACP agent offered over the agent link a gateway serves.
//! The relay passes the agent's lines unread; what the agent may do stays with the agent.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use agentic_peer::home;
use cabuya::Error;
use cabuya::auth::{self, Method, Proof, Sealed, Trust};
use cabuya::mux::{Heartbeat, Limits as MuxLimits, Mux, Side};
use coqui_acp_bridge::agent::Agent;
use coqui_acp_bridge::relay::{LinkEnd, relay};
use coqui_transport::{Channel as _, Failure};
use coqui_transport_web::websocket::{Dialer, Frames};
use coqui_transport_web::{Dialer as _, Kind, Limits};
use serde_json::json;
use url::Url;

/// The stream token the agent's JSON-RPC lines travel on.
const STREAM: &str = "acp/1";
/// The longest line the agent may write, as the bridge's own `dial` allows.
const AGENT_LINE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, clap::Args)]
pub(crate) struct DialArgs {
    /// The agent link to dial: wss://, or ws:// on loopback.
    #[arg(long)]
    url: Url,
    /// A code the person's page showed: carried once, on the first link, to bind this key.
    #[arg(long)]
    code: Option<String>,
    /// A name for this agent, sent with the code.
    #[arg(long, requires = "code")]
    label: Option<String>,
    /// This side's identity file, whose key's method is the one it proves by (default: this
    /// user's, in the link home, made the first time).
    #[arg(long)]
    identity: Option<PathBuf>,
    /// The method of a new identity: noise (the default) or webcrypto. An existing key keeps its
    /// own, and naming another is refused.
    #[arg(long)]
    method: Option<Method>,
    /// The gateway's peer id, pinned, repeatable. Required unless --any-paired-peer is given.
    #[arg(long = "peer", required_unless_present = "any_paired_peer")]
    peers: Vec<coqui_peer_webcrypto::PeerId>,
    /// Accepts every peer paired in this user's trust store as the gateway.
    #[arg(long, conflicts_with = "peers")]
    any_paired_peer: bool,
    /// A PEM bundle of authorities a wss:// certificate must chain to. Without it the in-band
    /// proof alone says who the remote is.
    #[arg(long)]
    ca_file: Option<PathBuf>,
    /// Seconds to wait for each connection to open.
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
    /// Seconds to wait before dialing again after a link ends; doubles on each failure.
    #[arg(long, value_name = "SECONDS", default_value_t = 1)]
    backoff_start_seconds: u64,
    /// The longest wait between attempts to reconnect, in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = 60)]
    max_backoff_seconds: u64,
    /// Seconds a line waits for the agent to read it before the agent is taken as stuck
    /// (default: the bridge's).
    #[arg(long, value_name = "SECONDS")]
    agent_write_seconds: Option<u64>,
    /// The longest line the agent may write, in bytes.
    #[arg(long, value_name = "BYTES", default_value_t = AGENT_LINE_BYTES)]
    max_line_bytes: usize,
    /// The longest message the link carries, in bytes (default: the link's own).
    #[arg(long, value_name = "BYTES")]
    max_message_bytes: Option<usize>,
    /// The agent program, then its arguments.
    #[arg(last = true, required = true)]
    agent: Vec<OsString>,
}

/// Why `run` stopped without the agent's status.
struct Stop(String);

/// Runs the command: the agent's status when it ends, 2 when the remote is not accepted or the
/// arguments do not fit.
pub(crate) fn run(args: &DialArgs) -> ExitCode {
    match dial(args) {
        Ok(code) => ExitCode::from(code),
        Err(Stop(why)) => {
            eprintln!("ai6: {why}");
            ExitCode::from(2)
        }
    }
}

fn dial(args: &DialArgs) -> Result<u8, Stop> {
    let identity =
        home::identity(args.identity.as_deref(), args.method).map_err(|e| Stop(e.to_string()))?;
    let (trust, store) = if args.peers.is_empty() {
        let store = Arc::new(home::store(None).map_err(|e| Stop(e.to_string()))?);
        (Trust::Store(store.as_ref().clone()), Some(store))
    } else {
        (Trust::pinned(args.peers.iter().copied()), None)
    };
    let method = Proof::builder(identity)
        .responders(trust)
        .build()
        .map_err(|e| Stop(e.to_string()))?;
    eprintln!("proving by {}", method.method());

    let max_message = args
        .max_message_bytes
        .unwrap_or_else(|| MuxLimits::default().max_message);
    let (program, rest) = args
        .agent
        .split_first()
        .ok_or_else(|| Stop("no agent program".into()))?;
    let mut agent = Agent::spawn(program, rest, args.max_line_bytes)
        .map_err(|e| Stop(format!("the agent could not start: {e}")))?;
    if let Some(seconds) = args.agent_write_seconds {
        agent = agent.with_write_limit(Duration::from_secs(seconds));
    }

    let mut limits = Limits::default();
    limits.max_message = auth::MAX_PROOF_MESSAGE;
    let mut dialer = Dialer::builder().limits(limits).frames(Frames::Binary);
    if args.url.scheme() == "wss" {
        dialer = dialer.tls(tls_client(args.ca_file.as_deref())?);
    }
    let mut dialer = dialer.build().map_err(|e| Stop(e.to_string()))?;

    let first_wait = Duration::from_secs(args.backoff_start_seconds);
    let longest_wait = Duration::from_secs(args.max_backoff_seconds);
    let mut wait = first_wait;
    // The code is single use: once a link carried it, later links prove the key alone.
    let mut metadata = args.code.as_ref().map(|code| {
        let mut object = json!({ "code": code });
        if let Some(label) = &args.label {
            object["label"] = json!(label);
        }
        object.to_string().into_bytes()
    });
    loop {
        if let Some(status) = agent.try_status().map_err(|e| Stop(e.to_string()))? {
            return Ok(status_code(status));
        }
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
                let end = match proved {
                    Ok(established) => {
                        eprintln!("connected to {}", established.peer);
                        let peer = established.peer;
                        let sealed = Sealed::new(&channel, established.session);
                        let sealed = match &store {
                            Some(store) => {
                                let store = Arc::clone(store);
                                sealed.with_check(move || store.is_accepted(&peer))
                            }
                            None => sealed,
                        };
                        let mut link = MuxLimits::default();
                        link.max_message = max_message;
                        link.heartbeat = Some(Heartbeat::new(
                            Duration::from_secs(args.heartbeat_seconds),
                            args.heartbeat_misses,
                        ));
                        let end = Mux::run(&sealed, Side::Dialed, link, None, |mux| {
                            let opened = match &metadata {
                                Some(bytes) => mux.open_with(STREAM, bytes),
                                None => mux.open(STREAM),
                            };
                            opened.map(|stream| {
                                metadata = None;
                                relay(&stream, &agent)
                            })
                        });
                        sealed.close();
                        Some(end)
                    }
                    // A remote that proved the wrong key will not fix itself.
                    Err(Error::Proof(why)) => {
                        channel.close();
                        return Err(Stop(format!("the remote did not prove itself: {why}")));
                    }
                    Err(error) => {
                        eprintln!("the proof did not finish: {error}");
                        None
                    }
                };
                channel.close();
                if let Some(end) = end {
                    wait = first_wait;
                    match end {
                        Ok(LinkEnd::AgentEnded | LinkEnd::AgentStuck) => {
                            let status = agent.stop().map_err(|e| Stop(e.to_string()))?;
                            return Ok(status_code(status));
                        }
                        // The gateway refused the stream: a code or key it does not accept
                        // will not be accepted later.
                        Ok(LinkEnd::Failed(Failure::Permission)) => {
                            return Err(Stop("the gateway does not accept this agent".into()));
                        }
                        Ok(end) => eprintln!("the link ended: {end:?}"),
                        Err(failure) => eprintln!("the stream did not open: {failure}"),
                    }
                }
            }
            Err(Failure::Certificate) => {
                return Err(Stop("the remote certificate is not accepted".into()));
            }
            Err(failure) => eprintln!("could not connect: {failure}"),
        }
        thread::sleep(wait);
        wait = (wait * 2).min(longest_wait);
    }
}

fn status_code(status: std::process::ExitStatus) -> u8 {
    status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}

/// The TLS settings of a wss:// link: the in-band proof, not the certificate, decides who the
/// remote is, unless `ca_file` names roots that must vouch for it.
fn tls_client(ca_file: Option<&std::path::Path>) -> Result<Arc<rustls::ClientConfig>, Stop> {
    let trust = match ca_file {
        Some(file) => coqui_peer_tls::Trust::Roots(coqui_peer_tls::Roots::bundle_file(file)),
        None => coqui_peer_tls::Trust::AnyWithKeyProof,
    };
    let built = coqui_peer_tls::ClientBuilder::new()
        .server(trust)
        .build()
        .map_err(|e| Stop(e.to_string()))?;
    Ok(Arc::new(built.config))
}
