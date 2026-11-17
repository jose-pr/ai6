//! What every command shares: the proof's method and settings, the identity a proof runs under, TLS configurations, who may connect, and
//! a count of what is running.
#![allow(clippy::std_instead_of_core, clippy::std_instead_of_alloc)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use coqui_agentic_peer::auth::{self, Identity, Method, Proof, ProofBuilder};
use coqui_agentic_peer::{Error, home};
use coqui_net_acl::{Acl, Action, IpNet, Rule};
use coqui_peer::PeerId;
use coqui_peer_tls as tls;
use coqui_transport_web::{Limits, websocket};

/// The longest line of ACP a link carries.
pub(crate) const MAX_LINE: usize = 16 * 1024 * 1024;
/// How long a remote may take to prove itself to `dial`.
pub(crate) const DIAL_PROOF: Duration = Duration::from_secs(15);
/// Messages an unproven peer may have waiting.
const UNPROVEN_PENDING: usize = 8;

/// Which proof an endpoint runs. It is chosen here and never negotiated on a connection.
#[derive(clap::Args, Debug, Clone, Copy)]
pub struct MethodArgs {
    /// The proof's method: `noise`, or `webcrypto` for a browser whose WebCrypto has no X25519. An identity keeps the method of its
    /// key, and naming another is refused. Without it, the identity's own; a new identity gets the default method.
    #[arg(long, value_name = "METHOD")]
    pub method: Option<Method>,
}

/// The session's settings, each with its method's default and no floor. One looser than the default is said on standard error.
#[derive(clap::Args, Debug, Clone, Copy)]
pub struct SessionArgs {
    /// What one key may carry in each direction, in bytes. Default 2^38, below where AES-GCM's margin thins.
    #[arg(long, value_name = "BYTES")]
    pub key_limit: Option<u64>,
    /// Under Noise, end the session at the key limit instead of changing keys. WebCrypto has no key change, so it always ends.
    #[arg(long)]
    pub no_rekey: bool,
}

pub(crate) fn peers(list: &[String]) -> Result<BTreeSet<PeerId>, String> {
    list.iter()
        .map(|text| {
            text.parse()
                .map_err(|_| "a peer id is malformed".to_owned())
        })
        .collect()
}

/// The identity a proof runs under: this user's without `path`; with it, the file's, made when absent if `create`. Its key's
/// method is its method, and naming another is refused.
pub(crate) fn proof_identity(
    home: Option<&Path>,
    path: Option<&Path>,
    method: MethodArgs,
    create: bool,
) -> Result<Identity, String> {
    let identity = match (path, home) {
        (Some(path), _) if !create => {
            auth::read_identity(path).and_then(|identity| match method.method {
                Some(named) => identity.of_method(named),
                None => Ok(identity),
            })
        }
        (None, Some(dir)) => identity_in(dir, method.method),
        (path, _) => home::identity(path, method.method),
    };
    identity.map_err(|error| error.to_string())
}

/// This user's identity of `method`, in `dir`: with no method, the one the directory records, else the one it holds, else a new one
/// of the default method, which it then records. The same as `coqui_agentic_peer::home::identity` for a directory it does not
/// resolve.
fn identity_in(dir: &Path, method: Option<Method>) -> Result<Identity, Error> {
    let recorded = match std::fs::read_to_string(dir.join(home::METHOD_FILE)) {
        Ok(text) => Some(text.trim().parse::<Method>().map_err(|_| {
            Error::Identity(format!(
                "{} names no method: write noise or webcrypto",
                home::METHOD_FILE
            ))
        })?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(Error::Identity(format!(
                "{} could not be read: {error}",
                home::METHOD_FILE
            )));
        }
    };
    let chosen = match (method, recorded) {
        (Some(method), _) | (None, Some(method)) => method,
        (None, None) => {
            let held: Vec<Method> = Method::ALL
                .into_iter()
                .filter(|method| dir.join(home::identity_file(*method)).exists())
                .collect();
            match held.as_slice() {
                [] => Method::DEFAULT,
                [only] => *only,
                _ => {
                    return Err(Error::Identity(format!(
                        "this user has an identity of each method: name the one to use, or write it in {}",
                        home::METHOD_FILE
                    )));
                }
            }
        }
    };
    let identity = Identity::open_or_create(&dir.join(home::identity_file(chosen)), chosen)?;
    if recorded != Some(chosen) {
        std::fs::create_dir_all(dir)
            .and_then(|()| {
                std::fs::write(
                    dir.join(home::METHOD_FILE),
                    format!(
                        "{chosen}
"
                    ),
                )
            })
            .map_err(|error| {
                Error::Identity(format!(
                    "{} could not be written: {error}",
                    home::METHOD_FILE
                ))
            })?;
    }
    Ok(identity)
}

/// The trust store at `store`, else in `home`, else this user's own.
pub(crate) fn store_of(
    home: Option<&Path>,
    store: Option<&Path>,
) -> Result<coqui_peer::TrustStore, String> {
    let path = store
        .map(Path::to_path_buf)
        .or_else(|| home.map(|dir| dir.join("peers")));
    home::store(path.as_deref()).map_err(|error| error.to_string())
}

/// The file of this user's grants.
pub(crate) fn grants_of(home: Option<&Path>) -> Result<PathBuf, String> {
    match home {
        Some(dir) => Ok(dir.join("grants.json")),
        None => home::grants_path().map_err(|error| error.to_string()),
    }
}

/// `identity`'s proof with the session's settings.
pub(crate) fn proof_of(identity: Identity, session: SessionArgs) -> ProofBuilder {
    let mut proof = Proof::builder(identity);
    if let Some(bytes) = session.key_limit {
        proof = proof.key_limit(bytes);
    }
    if session.no_rekey {
        proof = proof.rekey(false);
    }
    proof
}

/// The proof a builder describes, or why its settings do not fit the identity's method.
pub(crate) fn built(builder: ProofBuilder) -> Result<Proof, String> {
    builder.build().map_err(|error| error.to_string())
}

/// Says on standard error which method a proof runs, and each of its settings other than the default. A parent that stopped
/// reading standard error does not end the bridge for it.
pub(crate) fn say_proof(proof: &Proof) {
    let mut said = std::io::stderr().lock();
    let _ = writeln!(said, "proving by {}", proof.method());
    for setting in proof.non_defaults() {
        let _ = writeln!(
            said,
            "non-default: {} is {}, default {}",
            setting.setting, setting.value, setting.default
        );
    }
}

pub(crate) fn split_agent(agent: &[OsString]) -> Result<(&OsString, &[OsString]), String> {
    agent
        .split_first()
        .ok_or_else(|| "no agent program".to_owned())
}

pub(crate) fn read_tls_identity(path: &std::path::Path) -> Result<tls::Identity, String> {
    tls::Identity::read(path)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "the TLS identity file does not exist".to_owned())
}

pub(crate) fn server_tls(path: &std::path::Path) -> Result<Arc<rustls::ServerConfig>, String> {
    let identity = read_tls_identity(path)?;
    let built = tls::ServerBuilder::new()
        .identity(&identity)
        .client_auth(tls::ClientAuth::None)
        .build()
        .map_err(|error| error.to_string())?;
    Ok(Arc::new(built.config))
}

/// TLS for a connection whose peer the in-band proof authenticates.
pub(crate) fn in_band_tls(ca_file: Option<&PathBuf>) -> Result<Arc<rustls::ClientConfig>, String> {
    let trust = match ca_file {
        Some(file) => tls::Trust::Roots(tls::Roots::bundle_file(file)),
        None => tls::Trust::AnyWithKeyProof,
    };
    let built = tls::ClientBuilder::new()
        .server(trust)
        .build()
        .map_err(|error| error.to_string())?;
    Ok(Arc::new(built.config))
}

pub(crate) fn server_tls_of(config: Option<Arc<rustls::ServerConfig>>) -> websocket::ServerTls {
    config.map_or(websocket::ServerTls::Plain, websocket::ServerTls::Config)
}

pub(crate) fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_message = MAX_LINE;
    limits
}

/// What a channel starts with while its peer has proved nothing; the proof raises it.
pub(crate) fn unproven_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_message = auth::MAX_PROOF_MESSAGE;
    limits.pending = UNPROVEN_PENDING;
    limits
}

/// A count of what is running, with a ceiling.
#[derive(Clone)]
pub(crate) struct Gate(Arc<AtomicUsize>, usize);

pub(crate) struct Entered(Arc<AtomicUsize>);

impl Drop for Entered {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Gate {
    pub(crate) fn new(ceiling: usize) -> Self {
        Self(Arc::new(AtomicUsize::new(0)), ceiling)
    }

    pub(crate) fn enter(&self) -> Option<Entered> {
        let mut running = self.0.load(Ordering::SeqCst);
        while running < self.1 {
            match self
                .0
                .compare_exchange(running, running + 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Some(Entered(Arc::clone(&self.0))),
                Err(now) => running = now,
            }
        }
        None
    }
}

/// What may start a handshake: `--deny` first, then `--permit` if any are listed. A first-match ACL.
pub(crate) fn admission(
    permit: &[String],
    deny: &[String],
) -> Result<impl Fn(SocketAddr) -> bool + Clone + Send + Sync + 'static, String> {
    let parse = |text: &String| {
        text.parse::<IpNet>()
            .map_err(|_| "a network is malformed".to_owned())
    };
    let acl = if permit.is_empty() && deny.is_empty() {
        None
    } else {
        let default = if permit.is_empty() {
            Action::Permit
        } else {
            Action::Deny
        };
        let mut acl = Acl {
            rules: Vec::new(),
            default,
        };
        for network in deny {
            acl.push(Rule::new(Action::Deny, parse(network)?));
        }
        for network in permit {
            acl.push(Rule::new(Action::Permit, parse(network)?));
        }
        Some(acl)
    };
    let acl = Arc::new(acl);
    Ok(move |source: SocketAddr| {
        acl.as_ref()
            .as_ref()
            .is_none_or(|acl| acl.decide(source, source).proceeds())
    })
}

pub(crate) fn status_code(status: std::process::ExitStatus) -> u8 {
    status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}

/// Whether `text` is a browser origin as a listener can match it: `scheme://host` or `scheme://host:port`, with no path, query,
/// user information or spaces. `null` is no origin (a sandboxed page or a local file sends it), so it is refused.
#[must_use]
pub fn valid_origin(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once("://") else {
        return false;
    };
    let scheme_ok = !scheme.is_empty()
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    let host_port_ok = !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '[' | ']' | '_'));
    scheme_ok && host_port_ok && !rest.starts_with(':')
}

/// Refuses an origin list that has a malformed entry.
pub(crate) fn check_origins(origins: &[String]) -> Result<(), String> {
    match origins.iter().find(|origin| !valid_origin(origin)) {
        Some(_) => Err(
            "an --origin is scheme://host or scheme://host:port, with no path (and never null)"
                .to_owned(),
        ),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::valid_origin;

    #[test]
    fn an_origin_is_scheme_host_and_port_and_nothing_else() {
        for good in [
            "https://example.com",
            "http://localhost:8080",
            "chrome-extension://abcdefghijklmnopabcdefghijklmnop",
            "http://[::1]:7800",
        ] {
            assert!(valid_origin(good), "{good}");
        }
        for bad in [
            "null",
            "",
            "example.com",
            "https://",
            "https://example.com/",
            "https://example.com/path",
            "https://user@example.com",
            "https://exa mple.com",
            "https://example.com?x=1",
            "://example.com",
            "1http://example.com",
        ] {
            assert!(!valid_origin(bad), "{bad}");
        }
    }
}
