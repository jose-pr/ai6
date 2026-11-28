//! `connect` and `forward`: dial a paired host, then carry one named stream over standard input and output, or map local ports onto
//! targets the host can reach.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use cabuya::forward::{self, Target};
use cabuya::mux::{Limits, LinkState, Mux, Stream, StreamEnd};
use coqui_transport::{Channel, Failure, StreamChannel};

use crate::home::{Home, Host};
use crate::wire;

/// What one `connect` carries.
pub enum Wanted {
    /// A TCP connection to a target the host can reach, as `tcp/1`.
    Target(Target),
    /// Any stream by protocol name, with the metadata its service reads.
    Stream { protocol: String, metadata: Vec<u8> },
}

/// How far a chunk read from standard input is cut before it is sent.
const CHUNK: usize = 16 * 1024;
/// How often the sending side looks at whether the stream ended.
const POLL: Duration = Duration::from_millis(100);

/// Dials `host` as this user and runs `body` over the link, closing it afterwards.
fn linked<R>(
    home: &Home,
    method: Option<cabuya::auth::Method>,
    host: &Host,
    body: impl FnOnce(&Mux<'_, cabuya::auth::Sealed<'_, StreamChannel>>) -> R,
) -> Result<R, String> {
    let identity = home.identity(method)?;
    let (channel, session) = wire::prove_peer(&host.address, &identity, host.peer)?;
    let sealed = wire::sealed(&channel, session);
    let result = Mux::run(
        &sealed,
        cabuya::mux::Side::Dialed,
        Limits::default(),
        None,
        body,
    );
    channel.close();
    Ok(result)
}

/// Why a stream ended, when it did not end in order.
fn ended(end: Option<StreamEnd>, failure: Failure) -> String {
    match end {
        Some(StreamEnd::Refused(why)) => format!("the host refused the stream: {why}"),
        Some(StreamEnd::Revoked(why)) => format!("the host took the permission away: {why}"),
        Some(StreamEnd::Superseded(why)) => format!("the stream was superseded: {why}"),
        Some(StreamEnd::LinkLost(lost)) => format!("the link was lost: {lost}"),
        Some(StreamEnd::Broken) => "the stream broke the link's rules".to_owned(),
        Some(StreamEnd::PeerClosed) | Some(_) | None => format!("the stream failed: {failure}"),
    }
}

/// Moves standard input to `stream` and the stream to standard output, until the stream ends. Standard input ending finishes the
/// stream and goes on reading; the stream ending in order ends this. Standard input is read by a thread of its own, since a
/// blocked read cannot be taken back.
fn pipe<C: Channel>(stream: &Stream<'_, C>) -> Result<(), String> {
    let (chunks, input) = mpsc::sync_channel::<Option<Vec<u8>>>(4);
    thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buffer = vec![0_u8; CHUNK];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => {
                    let _ = chunks.send(None);
                    return;
                }
                Ok(read) => {
                    if chunks.send(Some(buffer[..read].to_vec())).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let done = AtomicBool::new(false);
    thread::scope(|scope| {
        let flag = &done;
        scope.spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match input.recv_timeout(POLL) {
                    Ok(Some(chunk)) => {
                        let sent = loop {
                            match stream.send(&chunk) {
                                Err(Failure::NoProgress) if !flag.load(Ordering::SeqCst) => {}
                                other => break other,
                            }
                        };
                        if sent.is_err() {
                            return;
                        }
                    }
                    Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = stream.finish();
                        return;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });
        let mut stdout = std::io::stdout().lock();
        let result = loop {
            match stream.receive(None) {
                Ok(Some(message)) => {
                    if let Err(error) = stdout.write_all(&message).and_then(|()| stdout.flush()) {
                        break Err(format!("standard output: {error}"));
                    }
                }
                Ok(None) => match stream.end() {
                    None | Some(StreamEnd::PeerClosed) => break Ok(()),
                    end => break Err(ended(end, Failure::PathLost)),
                },
                Err(failure) => break Err(ended(stream.end(), failure)),
            }
        };
        done.store(true, Ordering::SeqCst);
        result
    })
}

/// `cabuya connect`: one stream to a paired host, on standard input and output.
pub fn connect(
    home: &Home,
    method: Option<cabuya::auth::Method>,
    name: &str,
    wanted: &Wanted,
) -> Result<u8, String> {
    let host = home.host(name)?;
    linked(home, method, &host, |mux| {
        let stream = match wanted {
            Wanted::Target(target) => forward::connect(mux, target),
            Wanted::Stream { protocol, metadata } => mux.open_with(protocol, metadata),
        }
        .map_err(|failure| format!("the stream could not be opened: {failure}"))?;
        pipe(&stream)
    })?
    .map(|()| 0)
}

/// One local port mapped onto a target the host reaches.
#[derive(Debug, Clone)]
pub struct Mapping {
    pub bind: String,
    pub target: Target,
}

impl std::str::FromStr for Mapping {
    type Err = String;

    /// `[ADDRESS:]PORT=HOST:PORT`: the address defaults to loopback.
    fn from_str(text: &str) -> Result<Self, String> {
        let (local, target) = text
            .split_once('=')
            .ok_or_else(|| format!("{text}: expected [ADDRESS:]PORT=HOST:PORT"))?;
        let bind = if local.contains(':') {
            local.to_owned()
        } else {
            format!("127.0.0.1:{local}")
        };
        let target = target
            .parse()
            .map_err(|_| format!("{target}: not a HOST:PORT"))?;
        Ok(Self { bind, target })
    }
}

/// `cabuya forward`: each mapping listens here and opens a `tcp/1` stream per connection, until the link ends.
pub fn forward(
    home: &Home,
    method: Option<cabuya::auth::Method>,
    name: &str,
    mappings: &[Mapping],
) -> Result<u8, String> {
    let host = home.host(name)?;
    let mut listeners = Vec::new();
    for mapping in mappings {
        let listener = TcpListener::bind(&mapping.bind)
            .map_err(|error| format!("cannot listen on {}: {error}", mapping.bind))?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        if !forward::is_loopback(address.ip()) {
            eprintln!(
                "cabuya: {address} is not loopback: whoever connects to it gets what this key was granted"
            );
        }
        println!("listening {address} {}", mapping.target);
        listeners.push((listener, &mapping.target));
    }
    let _ = std::io::stdout().flush();
    let ended = linked(home, method, &host, |mux| {
        thread::scope(|scope| {
            for (listener, target) in &listeners {
                scope.spawn(move || {
                    let _ = forward::listen(mux, listener, target);
                });
            }
        });
        mux.link_state()
    })?;
    match ended {
        LinkState::Up | LinkState::Closed => Ok(0),
        state => Err(format!("the link ended: {state:?}")),
    }
}
