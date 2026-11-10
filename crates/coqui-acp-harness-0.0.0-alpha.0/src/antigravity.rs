//! Antigravity's quota as usage buckets.
//!
//! `agy acp` serves its own language server on two random loopback ports once a session is open, one
//! plain HTTP and one HTTPS with a self-signed certificate, and answers `GetUserStatus` on both with
//! the signed-in account's remaining fraction and reset time for each model. [`AgyQuota`] finds those
//! ports among the processes it is told are the agent's, asks, and turns the answer into
//! `reported` [`Bucket`]s: one for each distinct reset time, in permille.
//!
//! The request carries no secret and the answer's account details are never kept, so the only proof
//! that a port is agy's is that its owner is one of the agent's own processes, which is checked
//! again before every reading. Plain HTTP is asked first; HTTPS is asked with certificate
//! verification off, for a literal loopback address only.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::net::SocketAddr;
use core::time::Duration;
use std::io;
use std::time::Instant;

use coqui_acp_api::usage::Bucket;

mod ports;
mod status;

pub use ports::loopback_listeners;

/// What every bucket id starts with, so a host replaces this reader's buckets and no other's.
pub const BUCKET_PREFIX: &str = "agy:";

/// The path of the status call, on the language server's service.
const STATUS_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";

/// A model as the ACP agent lists it in `session/new`: how a reading's models are matched to the
/// ids a client uses.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AcpModel {
    /// Its id (`modelId`).
    pub id: String,
    /// Its name, which agy's own label for the model equals.
    pub name: String,
}

impl AcpModel {
    /// A model with `id` and `name`.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

/// How far a reading may go. Every limit has a default and no floor: a caller decides what is
/// too little.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadLimits {
    /// The most of an answer's body that is read, in bytes. An answer past it is malformed.
    /// Default 256 KiB (an answer is about 23 KB).
    pub body: usize,
    /// How long a connection may take to open. Default 1 s.
    pub connect: Duration,
    /// How long a whole reading may take, every attempt together. Default 3 s.
    pub total: Duration,
    /// The shortest time between two requests: a reading sooner than this after the last that
    /// succeeded answers that one's buckets. Default none.
    pub min_interval: Duration,
}

impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            body: 256 * 1024,
            connect: Duration::from_secs(1),
            total: Duration::from_secs(3),
            min_interval: Duration::ZERO,
        }
    }
}

impl ReadLimits {
    /// The defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the most of a body that is read.
    #[must_use]
    pub const fn body(mut self, bytes: usize) -> Self {
        self.body = bytes;
        self
    }

    /// Sets how long a connection may take to open.
    #[must_use]
    pub const fn connect(mut self, limit: Duration) -> Self {
        self.connect = limit;
        self
    }

    /// Sets how long a whole reading may take.
    #[must_use]
    pub const fn total(mut self, limit: Duration) -> Self {
        self.total = limit;
        self
    }

    /// Sets the shortest time between two requests.
    #[must_use]
    pub const fn min_interval(mut self, interval: Duration) -> Self {
        self.min_interval = interval;
        self
    }
}

/// Why a reading gave nothing. None of them holds any of the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReadError {
    /// The processes have no loopback listener: no session has started the server yet, or the
    /// agent has ended.
    NoPort,
    /// The port last read no longer belongs to the processes, and no other of theirs answers.
    NotOwned,
    /// A port of theirs answered with a status other than 200 (signed out, or a redirect).
    Status(u16),
    /// A port answered 200 with a body that is not agy's status, or too large.
    Malformed,
    /// The ports could not be listed, or none could be reached or read within the limits.
    Io(io::ErrorKind),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPort => f.write_str("the agent has no loopback port"),
            Self::NotOwned => f.write_str("the port is no longer the agent's"),
            Self::Status(code) => write!(f, "the agent answered status {code}"),
            Self::Malformed => f.write_str("the agent's answer is not its status"),
            Self::Io(kind) => write!(f, "the agent could not be read: {kind}"),
        }
    }
}

impl core::error::Error for ReadError {}

/// How a port was last reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scheme {
    Http,
    Https,
}

impl Scheme {
    const fn name(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

/// Reads Antigravity's quota. One reader serves one set of processes; it remembers which port and
/// scheme answered, and each bucket's window length.
#[derive(Debug)]
pub struct AgyQuota {
    limits: ReadLimits,
    endpoint: Option<(SocketAddr, Scheme)>,
    windows: status::Windows,
    unmatched: u64,
    last: Option<(Instant, Vec<Bucket>)>,
}

impl AgyQuota {
    /// A reader within `limits`.
    #[must_use]
    pub fn new(limits: ReadLimits) -> Self {
        Self {
            limits,
            endpoint: None,
            windows: status::Windows::default(),
            unmatched: 0,
            last: None,
        }
    }

    /// How many times a model agy listed matched none of `models` (by id, then by label), so that
    /// its bucket's coverage was left unknown.
    #[must_use]
    pub const fn unmatched(&self) -> u64 {
        self.unmatched
    }

    /// Reads the quota from a port that one of `owners` (process ids) listens on, as of now.
    ///
    /// `models` are the ACP agent's models: a bucket covers the ones agy's models match by id,
    /// else by label, and covers an unknown set if any does not match.
    ///
    /// # Errors
    ///
    /// [`ReadError`]: the processes have no port, the port is no longer theirs, nothing answered
    /// status 200 with agy's status, or the ports could not be listed.
    pub fn read(&mut self, owners: &[u32], models: &[AcpModel]) -> Result<Vec<Bucket>, ReadError> {
        self.read_at(owners, models, crate::acp::now())
    }

    pub(crate) fn read_at(
        &mut self,
        owners: &[u32],
        models: &[AcpModel],
        now: i64,
    ) -> Result<Vec<Bucket>, ReadError> {
        if let Some((at, held)) = &self.last
            && at.elapsed() < self.limits.min_interval
        {
            return Ok(held.clone());
        }
        let deadline = Instant::now().checked_add(self.limits.total);
        let listeners = ports::listeners_before(owners, deadline)
            .map_err(|error| ReadError::Io(error.kind()))?;
        let stale = self
            .endpoint
            .is_some_and(|(address, _)| !listeners.contains(&address));
        if stale {
            self.endpoint = None;
        }
        let mut failure: Option<ReadError> = None;
        for (address, scheme) in self.attempts(&listeners) {
            match self.fetch(address, scheme, deadline) {
                Ok(models_read) => {
                    self.endpoint = Some((address, scheme));
                    let buckets = status::buckets(
                        &models_read,
                        models,
                        now,
                        &mut self.windows,
                        &mut self.unmatched,
                    );
                    self.last = Some((Instant::now(), buckets.clone()));
                    return Ok(buckets);
                }
                Err(error) => {
                    // The next reading looks at every port again.
                    self.endpoint = None;
                    if failure
                        .as_ref()
                        .is_none_or(|held| rank(&error) > rank(held))
                    {
                        failure = Some(error);
                    }
                }
            }
        }
        Err(failure.unwrap_or(if stale {
            ReadError::NotOwned
        } else {
            ReadError::NoPort
        }))
    }

    /// What to try, in order: the remembered port by the remembered scheme, then every listener by
    /// plain HTTP and by HTTPS.
    fn attempts(&self, listeners: &[SocketAddr]) -> Vec<(SocketAddr, Scheme)> {
        if let Some(known) = self.endpoint {
            return alloc::vec![known];
        }
        listeners
            .iter()
            .flat_map(|address| [(*address, Scheme::Http), (*address, Scheme::Https)])
            .collect()
    }

    fn fetch(
        &self,
        address: SocketAddr,
        scheme: Scheme,
        deadline: Option<Instant>,
    ) -> Result<Vec<status::Model>, ReadError> {
        // The request never leaves this machine: the verification switched off below is safe only
        // because of this.
        if !address.ip().to_canonical().is_loopback() {
            return Err(ReadError::Io(io::ErrorKind::InvalidInput));
        }
        let remaining = deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .ok_or(ReadError::Io(io::ErrorKind::TimedOut))?;
        if remaining.is_zero() {
            return Err(ReadError::Io(io::ErrorKind::TimedOut));
        }
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .proxy(None)
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .disable_verification(true)
                    .build(),
            )
            .timeout_global(Some(remaining))
            .timeout_connect(Some(self.limits.connect.min(remaining)))
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let url = alloc::format!("{}://{address}{STATUS_PATH}", scheme.name());
        let mut response = agent
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send("{}")
            .map_err(from_ureq)?;
        let code = response.status().as_u16();
        if code != 200 {
            return Err(ReadError::Status(code));
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(self.limits.body as u64)
            .read_to_vec()
            .map_err(from_ureq)?;
        status::parse(&body)
    }
}

/// Which failure says more: a body that was not the status, a refusing status, a failed connection.
fn rank(error: &ReadError) -> u8 {
    match error {
        ReadError::Malformed => 3,
        ReadError::Status(_) => 2,
        _ => 1,
    }
}

fn from_ureq(error: ureq::Error) -> ReadError {
    match error {
        ureq::Error::Io(error) => ReadError::Io(error.kind()),
        ureq::Error::Timeout(_) => ReadError::Io(io::ErrorKind::TimedOut),
        ureq::Error::BodyExceedsLimit(_) | ureq::Error::Protocol(_) | ureq::Error::Http(_) => {
            ReadError::Malformed
        }
        _ => ReadError::Io(io::ErrorKind::Other),
    }
}
