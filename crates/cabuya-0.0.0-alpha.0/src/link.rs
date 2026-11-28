//! Serving a link: what each stream a peer opens runs, under the grants of the entry its proof named. A proof admits a peer to the
//! link. It grants no protocol, so each open is decided here, and again while the stream runs.

use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use coqui_transport::{Channel, Failure};
use serde_json::{Value, json};

use crate::mux::{CloseReason, Incoming, Limits, LinkStream, Mux, Side, Stream};
use crate::peers::{Event, Events, PeerRecord, PeerStore};

/// The stream that says what a link offers. Any admitted peer may open it, and it answers with what that peer may open.
pub const LINK_PROTOCOL: &str = "link/1";
/// How long a capability document may be.
const MAX_DOCUMENT: usize = 64 * 1024;
const ACCEPT_POLL: Duration = Duration::from_millis(100);

/// Random for this process: a peer seeing another after a drop knows this side restarted and lost what it held, as against a blip.
#[must_use]
pub fn instance() -> &'static str {
    static INSTANCE: OnceLock<String> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        // An instance id is no secret: it only tells restarts apart. The standard library seeds each hasher from the system's
        // randomness, and the time is mixed in so a missing source still tells restarts apart.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let word = |index: usize| {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u128(now);
            hasher.write_usize(index);
            hasher.finish()
        };
        format!("{:016x}{:016x}", word(0), word(1))
    })
}

/// A stream a service was given, and who it is for.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Accepted {
    /// The peer, as its proof showed.
    pub peer: String,
    /// What the host set about that peer: derive who is speaking from these, and not from anything the peer says.
    pub claims: BTreeMap<String, String>,
    /// The protocol it opened.
    pub protocol: String,
    /// What it sent with the stream: opaque to the link, and not a credential of the link's.
    pub metadata: Vec<u8>,
}

/// What serves one protocol. It runs on a thread of its own, until it returns or its stream ends.
pub type Handler = Box<dyn Fn(&dyn LinkStream, &Accepted) + Send + Sync>;

/// What a protocol decides beyond its own grant, from what was opened with: whether this record allows what this stream is for. It
/// is asked before the handler runs and again while the stream does, so it must be quick and must not touch anything outside.
pub type Admission = Box<dyn Fn(&PeerRecord, &[u8]) -> Result<(), &'static str> + Send + Sync>;

struct Service {
    handler: Handler,
    admission: Option<Admission>,
}

/// The protocols this host serves.
#[derive(Default)]
pub struct Services(BTreeMap<String, Service>);

impl core::fmt::Debug for Services {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

impl Services {
    /// Serves `protocol` with `handler`.
    #[must_use]
    pub fn with(mut self, protocol: &str, handler: Handler) -> Self {
        self.0.insert(
            protocol.to_owned(),
            Service {
                handler,
                admission: None,
            },
        );
        self
    }

    /// Serves `protocol` with `handler`, under `admission` as well as the protocol's own grant.
    #[must_use]
    pub fn with_admission(
        mut self,
        protocol: &str,
        handler: Handler,
        admission: Admission,
    ) -> Self {
        self.0.insert(
            protocol.to_owned(),
            Service {
                handler,
                admission: Some(admission),
            },
        );
        self
    }

    /// Whether anything serves it.
    #[must_use]
    pub fn offers(&self, protocol: &str) -> bool {
        self.0.contains_key(protocol)
    }

    /// The tokens served.
    pub fn tokens(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

/// How a link is served.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Options {
    /// What the mux will hold.
    pub limits: Limits,
    /// How often a running stream's grant is asked about again. A grant taken away ends the stream within this.
    pub recheck: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            recheck: Duration::from_secs(1),
        }
    }
}

/// Why a protocol is not this peer's. Not being able to tell counts: a store that cannot be read must not let anything in.
fn decide(
    store: &dyn PeerStore,
    services: &Services,
    peer: &str,
    protocol: &str,
    metadata: &[u8],
) -> Result<PeerRecord, &'static str> {
    let Some(service) = services.0.get(protocol) else {
        return Err("nobody serves that here");
    };
    let record = match store.get(peer) {
        Ok(Some(record)) if record.grants.contains(protocol) => record,
        Ok(_) => return Err("this peer may not open that"),
        Err(_) => return Err("the host cannot say who may open that"),
    };
    if let Some(admission) = &service.admission {
        admission(&record, metadata)?;
    }
    Ok(record)
}

fn document(store: &dyn PeerStore, peer: &str, services: &Services, limits: &Limits) -> Vec<u8> {
    let offers: Vec<&str> = match store.get(peer) {
        Ok(Some(record)) => services
            .tokens()
            .filter(|token| record.grants.contains(*token))
            .collect(),
        _ => Vec::new(),
    };
    json!({
        "version": 1,
        "instance": instance(),
        "peer": peer,
        "offers": offers,
        "window": limits.window,
        "max_frame": limits.max_frame,
    })
    .to_string()
    .into_bytes()
}

/// What a host brings to every link it serves.
#[derive(Clone, Copy)]
pub struct Host<'a> {
    /// Who may open what.
    pub store: &'a dyn PeerStore,
    /// What runs.
    pub services: &'a Services,
    /// Where events go.
    pub events: &'a dyn Events,
}

impl core::fmt::Debug for Host<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Host")
            .field("services", self.services)
            .finish_non_exhaustive()
    }
}

/// A stream a handler is running, for the loop that re-checks its grant.
struct Live<'c, C: Channel> {
    stream: Arc<Stream<'c, C>>,
    protocol: String,
    metadata: Vec<u8>,
    done: Arc<AtomicBool>,
}

/// Serves a link until it ends: `channel` has already proved `peer`. `first`, if given, is a message already read from it.
pub fn serve<C: Channel>(
    channel: &C,
    side: Side,
    options: Options,
    first: Option<Vec<u8>>,
    peer: &str,
    host: &Host<'_>,
) {
    serve_with(channel, side, options, first, peer, host, |_| {});
}

/// As [`serve`], with `opener` running on a thread of its own, for a host that also opens streams to this peer. The link is served
/// until it ends whether or not `opener` has returned, so `opener` watches [`Mux::link_state`] rather than ending the link.
pub fn serve_with<'c, C: Channel>(
    channel: &'c C,
    side: Side,
    options: Options,
    first: Option<Vec<u8>>,
    peer: &str,
    host: &Host<'_>,
    opener: impl FnOnce(&Mux<'c, C>) + Send,
) {
    serve_with_terminal(channel, side, options, first, peer, host, opener, || {});
}

/// As [`serve_with`], with `terminal` called once when the mux reaches a terminal state (the link has ended or been lost), before
/// this call waits for the stream handlers still running.
///
/// A handler that blocks on something other than its stream hears the link end only through `terminal`, so a host wakes it there
/// and the wait for handlers is bounded. It runs on the thread of this call, so it should return promptly. `serve_with` is this with
/// a `terminal` that does nothing.
#[allow(clippy::too_many_arguments)]
pub fn serve_with_terminal<'c, C: Channel>(
    channel: &'c C,
    side: Side,
    options: Options,
    first: Option<Vec<u8>>,
    peer: &str,
    host: &Host<'_>,
    opener: impl FnOnce(&Mux<'c, C>) + Send,
    terminal: impl FnOnce(),
) {
    let Host {
        store,
        services,
        events,
    } = *host;
    events.event(&Event::LinkUp {
        peer: peer.to_owned(),
    });
    let limits = options.limits;
    Mux::run(channel, side, limits, first, |mux| {
        thread::scope(|scope| {
            scope.spawn(|| opener(mux));
            let live: Mutex<Vec<Live<'_, C>>> = Mutex::new(Vec::new());
            let mut checked = Instant::now();
            loop {
                // Here rather than after an accept, so what a peer opens cannot put off its own revocation.
                if checked.elapsed() >= options.recheck {
                    checked = Instant::now();
                    let mut held = live.lock().unwrap_or_else(PoisonError::into_inner);
                    held.retain(|running| {
                        if running.done.load(Ordering::SeqCst) {
                            return false;
                        }
                        if decide(store, services, peer, &running.protocol, &running.metadata)
                            .is_ok()
                        {
                            return true;
                        }
                        // Telling a peer whose link is full can take as long as the link allows, and this loop has others to watch.
                        let stream = Arc::clone(&running.stream);
                        scope.spawn(move || {
                            stream.close_with(
                                CloseReason::Revoked,
                                "this peer may no longer use that",
                            );
                        });
                        events.event(&Event::Revoked {
                            peer: peer.to_owned(),
                            protocol: running.protocol.clone(),
                        });
                        false
                    });
                }
                match mux.accept(Some(ACCEPT_POLL)) {
                    Ok(Some(incoming)) => {
                        // A slow service holds up nothing here: each runs on its own thread, and this goes back to waiting.
                        let Incoming {
                            protocol,
                            metadata,
                            stream,
                            ..
                        } = incoming;
                        if protocol == LINK_PROTOCOL {
                            scope.spawn(move || {
                                let _ = stream.send(&document(store, peer, services, &limits));
                                let _ = stream.finish();
                                let _ = stream.receive(Some(Duration::from_secs(5)));
                            });
                            continue;
                        }
                        match decide(store, services, peer, &protocol, &metadata) {
                            Err(why) => {
                                stream.close_with(CloseReason::Refused, why);
                                events.event(&Event::Refused {
                                    peer: peer.to_owned(),
                                    protocol,
                                });
                            }
                            Ok(record) => {
                                events.event(&Event::Opened {
                                    peer: peer.to_owned(),
                                    protocol: protocol.clone(),
                                });
                                let stream = Arc::new(stream);
                                let done = Arc::new(AtomicBool::new(false));
                                live.lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .push(Live {
                                        stream: Arc::clone(&stream),
                                        protocol: protocol.clone(),
                                        metadata: metadata.clone(),
                                        done: Arc::clone(&done),
                                    });
                                let accepted = Accepted {
                                    peer: peer.to_owned(),
                                    claims: record.claims,
                                    protocol: protocol.clone(),
                                    metadata,
                                };
                                scope.spawn(move || {
                                    if let Some(service) = services.0.get(&protocol) {
                                        (service.handler)(&*stream, &accepted);
                                    }
                                    done.store(true, Ordering::SeqCst);
                                    Channel::close(&*stream);
                                });
                            }
                        }
                    }
                    Err(Failure::Timeout) => {}
                    Ok(None) | Err(_) => break,
                }
            }
            // The link has gone. What is still running hears that from its stream, and this scope waits for it.
            terminal();
            drop(live);
        });
    });
    events.event(&Event::LinkDown {
        peer: peer.to_owned(),
    });
}

/// What a peer's capability document said. It is authoritative for this peer, where a DNS-SD record is only a hint.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Capabilities {
    /// That side's process: a different one after a drop means it restarted and lost what it held.
    pub instance: String,
    /// What this peer may open there.
    pub offers: Vec<String>,
    /// The credit that side assumes at a stream's start (its `Limits::window`), when it said. Both sides must start from the
    /// same value: a difference is for the host to report. Absent from an older peer.
    pub window: Option<u32>,
    /// The largest part of a message that side puts in one frame (its `Limits::max_frame`), when it said.
    pub max_frame: Option<usize>,
}

/// Asks the other side what this side may open.
///
/// # Errors
///
/// [`Failure::Protocol`] for a document that is not one, and the stream's own failure.
pub fn capabilities<C: Channel>(mux: &Mux<'_, C>, wait: Duration) -> Result<Capabilities, Failure> {
    let stream = mux.open(LINK_PROTOCOL)?;
    let message = stream.receive(Some(wait))?.ok_or(Failure::Protocol)?;
    if message.len() > MAX_DOCUMENT {
        return Err(Failure::Protocol);
    }
    let value: Value = serde_json::from_slice(&message).map_err(|_| Failure::Protocol)?;
    let instance = value
        .get("instance")
        .and_then(Value::as_str)
        .ok_or(Failure::Protocol)?
        .to_owned();
    let offers = value
        .get("offers")
        .and_then(Value::as_array)
        .ok_or(Failure::Protocol)?
        .iter()
        .filter_map(|offer| offer.as_str().map(str::to_owned))
        .collect();
    let window = value
        .get("window")
        .and_then(Value::as_u64)
        .and_then(|window| u32::try_from(window).ok());
    let max_frame = value
        .get("max_frame")
        .and_then(Value::as_u64)
        .and_then(|frame| usize::try_from(frame).ok());
    Ok(Capabilities {
        instance,
        offers,
        window,
        max_frame,
    })
}

/// A channel with one message already read from it, as when a server looked at what a client sent first.
#[derive(Debug)]
pub struct Prefilled<'a, C: Channel> {
    channel: &'a C,
    first: Mutex<Option<Vec<u8>>>,
}

impl<'a, C: Channel> Prefilled<'a, C> {
    /// `channel`, with `first` to be received before anything else.
    #[must_use]
    pub fn new(channel: &'a C, first: Vec<u8>) -> Self {
        Self {
            channel,
            first: Mutex::new(Some(first)),
        }
    }
}

impl<C: Channel> Channel for Prefilled<'_, C> {
    fn send(&self, message: &[u8]) -> Result<(), Failure> {
        self.channel.send(message)
    }

    fn receive(&self, timeout: Option<Duration>) -> Result<Option<Vec<u8>>, Failure> {
        if let Some(first) = self
            .first
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            return Ok(Some(first));
        }
        self.channel.receive(timeout)
    }

    fn close(&self) {
        self.channel.close();
    }

    fn set_max_message(&self, bytes: usize) {
        self.channel.set_max_message(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peers::{MemoryPeers, NoEvents};
    use std::sync::mpsc::{Receiver, Sender, channel as queue};

    struct Wire {
        out: Sender<Vec<u8>>,
        incoming: Mutex<Receiver<Vec<u8>>>,
    }

    fn wires() -> (Wire, Wire) {
        let (a_out, b_in) = queue();
        let (b_out, a_in) = queue();
        (
            Wire {
                out: a_out,
                incoming: Mutex::new(a_in),
            },
            Wire {
                out: b_out,
                incoming: Mutex::new(b_in),
            },
        )
    }

    impl Channel for Wire {
        fn send(&self, message: &[u8]) -> Result<(), Failure> {
            self.out
                .send(message.to_vec())
                .map_err(|_| Failure::PathLost)
        }

        fn receive(&self, timeout: Option<Duration>) -> Result<Option<Vec<u8>>, Failure> {
            let incoming = self.incoming.lock().unwrap();
            match incoming.recv_timeout(timeout.unwrap_or(Duration::from_secs(5))) {
                Ok(message) => Ok(Some(message)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(Failure::Timeout),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Ok(None),
            }
        }

        fn close(&self) {}
    }

    const WAIT: Option<Duration> = Some(Duration::from_secs(5));
    const TICK: Duration = Duration::from_secs(5);

    fn echo() -> Handler {
        Box::new(|stream, accepted| {
            while let Ok(Some(message)) = stream.receive(WAIT) {
                let mut answer = accepted
                    .claims
                    .get("station_id")
                    .cloned()
                    .unwrap_or_default()
                    .into_bytes();
                answer.extend_from_slice(&message);
                if stream.send(&answer).is_err() {
                    break;
                }
            }
        })
    }

    #[derive(Default)]
    struct Recorded(Mutex<Vec<Event>>);

    impl Events for Recorded {
        fn event(&self, event: &Event) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    /// A host on one end of a link, and `client` on the other, until `client` returns.
    fn with_host(
        store: &dyn PeerStore,
        events: &dyn Events,
        client: impl FnOnce(&Mux<'_, Wire>) + Send,
    ) {
        let (ours, theirs) = wires();
        let services = Services::default()
            .with("acp/1", echo())
            .with("mcp", echo());
        let options = Options {
            recheck: Duration::from_millis(20),
            ..Options::default()
        };
        thread::scope(|scope| {
            let host = scope.spawn(|| {
                serve(
                    &theirs,
                    Side::Answered,
                    options,
                    None,
                    "peer",
                    &Host {
                        store,
                        services: &services,
                        events,
                    },
                )
            });
            Mux::run(&ours, Side::Dialed, Limits::default(), None, client);
            drop(ours);
            host.join().unwrap();
        });
    }

    #[test]
    fn one_entry_two_capabilities_and_claims_reach_the_service() {
        let _deadline = crate::deadline::within(60);
        let store = MemoryPeers::default();
        store
            .put(
                &PeerRecord::new("peer")
                    .granting("acp/1")
                    .granting("mcp")
                    .claiming("station_id", "s1"),
            )
            .unwrap();
        with_host(&store, &NoEvents, |mux| {
            let told = capabilities(mux, TICK).unwrap();
            assert_eq!(told.offers, ["acp/1", "mcp"]);
            assert_eq!(told.instance, instance());
            assert_eq!(told.window, Some(Limits::default().window));
            assert_eq!(told.max_frame, Some(Limits::default().max_frame));
            let acp = mux.open("acp/1").unwrap();
            let mcp = mux.open("mcp").unwrap();
            acp.send(b":acp").unwrap();
            mcp.send(b":mcp").unwrap();
            assert_eq!(acp.receive(WAIT).unwrap().unwrap(), b"s1:acp");
            assert_eq!(mcp.receive(WAIT).unwrap().unwrap(), b"s1:mcp");
        });
    }

    #[test]
    fn being_admitted_grants_nothing_and_a_grant_added_later_needs_no_second_pairing() {
        let _deadline = crate::deadline::within(60);
        let store = MemoryPeers::default();
        store
            .put(&PeerRecord::new("peer").granting("acp/1"))
            .unwrap();
        let events = Recorded::default();
        with_host(&store, &events, |mux| {
            assert_eq!(capabilities(mux, TICK).unwrap().offers, ["acp/1"]);
            let refused = mux.open("mcp").unwrap();
            assert_eq!(refused.receive(WAIT), Err(Failure::Permission));
            store
                .put(&PeerRecord::new("peer").granting("acp/1").granting("mcp"))
                .unwrap();
            assert_eq!(capabilities(mux, TICK).unwrap().offers, ["acp/1", "mcp"]);
            let granted = mux.open("mcp").unwrap();
            granted.send(b"x").unwrap();
            assert_eq!(granted.receive(WAIT).unwrap().unwrap(), b"x");
        });
        let seen = events.0.lock().unwrap();
        assert!(seen.contains(&Event::Refused {
            peer: "peer".to_owned(),
            protocol: "mcp".to_owned()
        }));
        assert!(seen.contains(&Event::Opened {
            peer: "peer".to_owned(),
            protocol: "mcp".to_owned()
        }));
        assert_eq!(
            seen.first(),
            Some(&Event::LinkUp {
                peer: "peer".to_owned()
            })
        );
    }

    #[test]
    fn nothing_unserved_and_no_record_means_refused() {
        let _deadline = crate::deadline::within(60);
        let store = MemoryPeers::default();
        with_host(&store, &NoEvents, |mux| {
            assert_eq!(
                capabilities(mux, TICK).unwrap().offers,
                Vec::<String>::new()
            );
            assert_eq!(
                mux.open("acp/1").unwrap().receive(WAIT),
                Err(Failure::Permission)
            );
        });
        store
            .put(&PeerRecord::new("peer").granting("other/1"))
            .unwrap();
        with_host(&store, &NoEvents, |mux| {
            assert_eq!(
                mux.open("other/1").unwrap().receive(WAIT),
                Err(Failure::Permission)
            );
        });
    }

    #[test]
    fn taking_a_grant_away_ends_that_stream_saying_so_and_leaves_the_other() {
        let _deadline = crate::deadline::within(60);
        let store = MemoryPeers::default();
        store
            .put(&PeerRecord::new("peer").granting("acp/1").granting("mcp"))
            .unwrap();
        let events = Recorded::default();
        with_host(&store, &events, |mux| {
            let acp = mux.open("acp/1").unwrap();
            let mcp = mux.open("mcp").unwrap();
            acp.send(b"a").unwrap();
            acp.receive(WAIT).unwrap().unwrap();
            store
                .put(&PeerRecord::new("peer").granting("acp/1"))
                .unwrap();
            assert_eq!(mcp.receive(WAIT), Err(Failure::Permission));
            assert!(matches!(mcp.end(), Some(crate::mux::StreamEnd::Revoked(_))));
            acp.send(b"b").unwrap();
            assert_eq!(acp.receive(WAIT).unwrap().unwrap(), b"b");
            assert_eq!(
                mux.open("mcp").unwrap().receive(WAIT),
                Err(Failure::Permission),
                "and it cannot be opened again"
            );
        });
        assert!(events.0.lock().unwrap().contains(&Event::Revoked {
            peer: "peer".to_owned(),
            protocol: "mcp".to_owned()
        }));
    }

    /// A store that can be broken, as a database going away would be.
    #[derive(Default)]
    struct Breakable {
        inner: MemoryPeers,
        broken: AtomicBool,
    }

    impl PeerStore for Breakable {
        fn get(&self, peer: &str) -> Result<Option<PeerRecord>, crate::Error> {
            if self.broken.load(Ordering::SeqCst) {
                return Err(crate::Error::Identity("gone".to_owned()));
            }
            self.inner.get(peer)
        }

        fn put(&self, record: &PeerRecord) -> Result<(), crate::Error> {
            self.inner.put(record)
        }

        fn remove(&self, peer: &str) -> Result<(), crate::Error> {
            self.inner.remove(peer)
        }
    }

    #[test]
    fn a_store_that_cannot_be_read_fails_closed() {
        let _deadline = crate::deadline::within(60);
        let store = Breakable::default();
        store
            .put(&PeerRecord::new("peer").granting("acp/1"))
            .unwrap();
        with_host(&store, &NoEvents, |mux| {
            let running = mux.open("acp/1").unwrap();
            running.send(b"a").unwrap();
            running.receive(WAIT).unwrap().unwrap();
            store.broken.store(true, Ordering::SeqCst);
            assert_eq!(
                running.receive(WAIT),
                Err(Failure::Permission),
                "what was running ends"
            );
            assert_eq!(
                mux.open("acp/1").unwrap().receive(WAIT),
                Err(Failure::Permission),
                "and nothing new opens"
            );
            assert_eq!(
                capabilities(mux, TICK).unwrap().offers,
                Vec::<String>::new()
            );
        });
    }

    #[test]
    fn terminal_wakes_a_handler_that_is_not_reading_its_stream() {
        let _deadline = crate::deadline::within(60);
        let store = MemoryPeers::default();
        store
            .put(&PeerRecord::new("peer").granting("held"))
            .unwrap();
        let (ours, theirs) = wires();
        let (entered_tx, entered) = queue::<()>();
        let entered_tx = Mutex::new(entered_tx);
        let woken = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        // The handler waits on a flag of its own, so only the hook can end it.
        let services = Services::default().with(
            "held",
            Box::new({
                let woken = Arc::clone(&woken);
                let ended = Arc::clone(&ended);
                move |_, _| {
                    entered_tx.lock().unwrap().send(()).unwrap();
                    let begun = Instant::now();
                    while !woken.load(Ordering::SeqCst) && begun.elapsed() < Duration::from_secs(20)
                    {
                        thread::sleep(Duration::from_millis(5));
                    }
                    ended.store(woken.load(Ordering::SeqCst), Ordering::SeqCst);
                }
            }),
        );
        let began = Instant::now();
        thread::scope(|scope| {
            let host = scope.spawn(|| {
                serve_with_terminal(
                    &theirs,
                    Side::Answered,
                    Options::default(),
                    None,
                    "peer",
                    &Host {
                        store: &store,
                        services: &services,
                        events: &NoEvents,
                    },
                    |_| {},
                    || woken.store(true, Ordering::SeqCst),
                )
            });
            Mux::run(&ours, Side::Dialed, Limits::default(), None, |mux| {
                let _held = mux.open("held").unwrap();
                entered.recv_timeout(Duration::from_secs(5)).unwrap();
            });
            drop(ours);
            host.join().unwrap();
        });
        assert!(
            ended.load(Ordering::SeqCst),
            "the handler was not woken by terminal"
        );
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "{:?}",
            began.elapsed()
        );
    }

    #[test]
    fn a_slow_service_holds_up_nothing_else() {
        let _deadline = crate::deadline::within(60);
        let store = MemoryPeers::default();
        store
            .put(&PeerRecord::new("peer").granting("slow").granting("fast"))
            .unwrap();
        let (ours, theirs) = wires();
        let services = Services::default()
            .with(
                "slow",
                Box::new(|_, _| thread::sleep(Duration::from_millis(600))),
            )
            .with("fast", echo());
        thread::scope(|scope| {
            let host = scope.spawn(|| {
                serve(
                    &theirs,
                    Side::Answered,
                    Options::default(),
                    None,
                    "peer",
                    &Host {
                        store: &store,
                        services: &services,
                        events: &NoEvents,
                    },
                )
            });
            Mux::run(&ours, Side::Dialed, Limits::default(), None, |mux| {
                let _slow = mux.open("slow").unwrap();
                let begun = Instant::now();
                let fast = mux.open("fast").unwrap();
                fast.send(b"x").unwrap();
                fast.receive(WAIT).unwrap().unwrap();
                assert!(
                    begun.elapsed() < Duration::from_millis(500),
                    "{:?}",
                    begun.elapsed()
                );
            });
            drop(ours);
            host.join().unwrap();
        });
    }

    #[test]
    fn a_refused_stream_is_told_why_before_it_closes() {
        let _deadline = crate::deadline::within(60);
        let store = MemoryPeers::default();
        with_host(&store, &NoEvents, |mux| {
            let stream = mux.open("mcp").unwrap();
            let _ = stream.receive(WAIT);
            assert!(
                matches!(stream.end(), Some(crate::mux::StreamEnd::Refused(why)) if !why.is_empty())
            );
        });
    }

    #[test]
    fn a_prefilled_channel_gives_its_first_message_first() {
        let _deadline = crate::deadline::within(60);
        let (ours, theirs) = wires();
        theirs.out.send(b"second".to_vec()).unwrap();
        let channel = Prefilled::new(&ours, b"first".to_vec());
        assert_eq!(channel.receive(WAIT).unwrap().unwrap(), b"first");
        assert_eq!(channel.receive(WAIT).unwrap().unwrap(), b"second");
    }

    #[test]
    fn opening_as_fast_as_it_can_does_not_put_off_a_revocation() {
        let _deadline = crate::deadline::within(60);
        let store = MemoryPeers::default();
        store
            .put(&PeerRecord::new("peer").granting("acp/1"))
            .unwrap();
        with_host(&store, &NoEvents, |mux| {
            let running = mux.open("acp/1").unwrap();
            running.send(b"a").unwrap();
            running.receive(WAIT).unwrap().unwrap();
            let opening = AtomicBool::new(true);
            thread::scope(|scope| {
                scope.spawn(|| {
                    // Faster than the accept loop's own poll, which is what kept it from ever reaching its check.
                    while opening.load(Ordering::SeqCst) {
                        let _ = mux.open(LINK_PROTOCOL);
                        thread::sleep(Duration::from_millis(20));
                    }
                });
                store.put(&PeerRecord::new("peer")).unwrap();
                let began = Instant::now();
                assert_eq!(running.receive(WAIT), Err(Failure::Permission));
                assert!(
                    began.elapsed() < Duration::from_secs(2),
                    "took {:?}",
                    began.elapsed()
                );
                opening.store(false, Ordering::SeqCst);
            });
        });
    }

    #[test]
    fn a_revocation_is_reported_once() {
        let _deadline = crate::deadline::within(60);
        let store = MemoryPeers::default();
        store
            .put(&PeerRecord::new("peer").granting("acp/1"))
            .unwrap();
        let events = Recorded::default();
        with_host(&store, &events, |mux| {
            let running = mux.open("acp/1").unwrap();
            running.send(b"a").unwrap();
            running.receive(WAIT).unwrap().unwrap();
            store.put(&PeerRecord::new("peer")).unwrap();
            assert_eq!(running.receive(WAIT), Err(Failure::Permission));
            thread::sleep(Duration::from_millis(200));
        });
        let told = events.0.lock().unwrap();
        assert_eq!(
            told.iter()
                .filter(|event| matches!(event, Event::Revoked { .. }))
                .count(),
            1
        );
    }
}
