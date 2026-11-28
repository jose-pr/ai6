# `cabuya` — API header

Header-file-style reference for the `cabuya` crate: every public item with its signature, contract and gotchas, so the crate can be used without reading its source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/cabuya>.

| Feature | Adds | Default |
| --- | --- | --- |
| `sealed` | `auth` and `pairing`; `coqui-peer`, `coqui-peer-noise` and `coqui-peer-webcrypto` (each with `pin`), `coqui-builder` | no |
| `forward` | `forward`; `socket2`, `coqui-net-acl` | no |
| `cli` | the `cabuya` program (`sealed`, `forward`, `clap`); no library item | no |

MSRV Rust 1.89. Version `0.0.0-alpha.0`, not published. A default build depends on `coqui-transport` (by revision, no default features; its `Channel` and `Failure` are what every link and stream is generic over) and `serde_json`. Nothing is re-exported at the root but `Error`; the public modules are `mux`, `link` and `peers`, with `auth` and `pairing` under `sealed` and `forward` under `forward`. No `unsafe` code.

## `cabuya::mux`

```rust
pub const MAX_PROTOCOL: usize = 64;
pub fn valid_protocol(protocol: &str) -> bool      // printable ASCII, no space, comma or equals; goes in a DNS-SD record as it is
pub fn starts_mux(first_message: &[u8]) -> bool    // true for a link with streams; JSON never starts with a zero byte
pub const MAX_REASON: usize = 256;
pub struct Limits { pub max_streams: usize, pub max_per_protocol: usize, pub window: u32, pub max_window: u32, pub auto_window: bool, pub max_message: usize, pub max_frame: usize, pub max_metadata: usize, pub max_pending_opens: usize, pub max_opens_per_minute: u32, pub send_wait: Duration, pub open_wait: Duration, pub heartbeat: Option<Heartbeat> }  // non_exhaustive, Default
pub struct Heartbeat { pub interval: Duration, pub misses: u32 }    // non_exhaustive; Heartbeat::new(interval, misses)
pub enum Side { Dialed, Answered }
pub enum Frame { Open { stream, protocol, metadata }, Data { stream, body }, More { stream, body }, Credit { stream, bytes }, Finish { stream }, Close { stream, reason, text }, Ping, Pong }  // non_exhaustive
pub enum CloseReason { Done, Refused, Broken, Superseded, Revoked }  // non_exhaustive
pub enum StreamEnd { PeerClosed, Refused(String), Broken, Superseded(String), Revoked(String), LinkLost(Failure) }  // non_exhaustive
pub enum LinkState { Up, Closed, Lost(Failure, SystemTime) }  // non_exhaustive
pub struct Incoming { pub protocol: String, pub metadata: Vec<u8>, pub stream: Stream<'c, C> }  // non_exhaustive
pub fn Frame::encode(&self) -> Vec<u8>
pub fn Frame::decode(bytes: &[u8]) -> Option<Frame>

pub fn Mux::run<R>(channel: &C, side: Side, limits: Limits, first: Option<Vec<u8>>, body: impl FnOnce(&Mux<'c, C>) -> R) -> R
pub fn Mux::open(&self, protocol: &str) -> Result<Stream<'c, C>, Failure>
pub fn Mux::open_with(&self, protocol: &str, metadata: &[u8]) -> Result<Stream<'c, C>, Failure>
pub fn Mux::accept(&self, timeout: Option<Duration>) -> Result<Option<Incoming<'c, C>>, Failure>
pub fn Mux::link_state(&self) -> LinkState
impl Channel for Stream<'_, C>   // send, receive, close; closes on drop
pub enum StreamLife { Open, Draining, Gone }   // Draining: the peer closed in order, what it sent can still be read
pub trait LinkStream: Channel { fn life(&self) -> StreamLife; fn finish(&self) -> Result<(), Failure>; fn finished_by_peer(&self) -> bool /* default false */; fn grant(&self, bytes: u32) -> Result<u32, Failure> /* default Err(Unsupported) */; fn close_with(&self, reason: CloseReason, text: &str); }   // what a service is given; implemented by Stream
pub fn Stream::try_send(&self, message: &[u8]) -> Result<(), Failure>    // NoProgress at once when full, uncharged
pub fn Stream::finish(&self) -> Result<(), Failure>                      // sends nothing more, goes on reading; the peer's receive answers None
pub fn Stream::close_with(&self, reason: CloseReason, text: &str)        // refused, superseded or revoked, saying why
pub fn Stream::end(&self) -> Option<StreamEnd>
pub fn Stream::window(&self) -> u32                                       // what this side lets the peer have in flight: `Limits::window` at first
pub fn Stream::grant(&self, bytes: u32) -> Result<u32, Failure>          // widens it by `bytes`, capped at the larger of `max_window` and `window`; one credit frame; answers the new window; the stream's end as Err once it has ended
pub fn Stream::finished_by_peer(&self) -> bool                            // the peer sent `finish`; also on LinkStream (default false)
```

- `Mux::run` reads the link for as long as `body` runs, on a thread of its own, and does not close the channel. `first` is a message already read, as when `starts_mux` looked at it.
- `Stream::send` waits up to `send_wait` for credit, then answers `Failure::NoProgress`, uncharged, so a caller may try again. A stream refused, superseded or revoked answers `Failure::Permission`, one that broke its bounds `Failure::Protocol`, and a lost link the link's own failure; `Stream::end` says which.
- `Mux::open_with` (and `open`) waits up to `open_wait` (default 250 ms; zero refuses at once) for a stream to end when this side already holds `max_streams`, or `max_per_protocol` of that protocol, then answers `Failure::NoProgress`. Rust only: Dart and JavaScript refuse at once. Opens keep their order while one waits.
- `receive` answers `None` for a peer that called `finish` and for a stream or link closed in order alike. `finished_by_peer` tells them apart: `false` after a `None` means what arrived may be part of what the peer meant to send. In JavaScript it is `stream.finishedByPeer`, in Dart `MuxStream.finishedByPeer`.
- Opens leave in id order whatever the thread count, and an open whose frame could not be sent leaves nothing in the stream table. In JavaScript and Dart `open` is one at a time for the same reason.
- A long message is cut into frames of `max_frame`, so another stream's message passes between its parts. Only the first part can be taken back: past that, a link that will not take the rest is lost.
- A stream delivers in order or fails, and never skips. Nothing is ordered across streams. Nothing replays: to survive a drop, reopen with your own cursor in `metadata`. Either side opens streams, whoever connected.
- `Limits::heartbeat` is off by default, and a zero interval or zero misses turns it off too. It loses a link only when nothing at all is heard, so a stream silent for hours is fine. `link_state` says when a link went; what that means is the host's to decide.
- `Limits::window` is the credit both sides assume at a stream's start (not negotiated), so keep it the same on both. A receiver that wants more on one stream grants it with `Stream::grant`: credit past what was read is already accepted by every sender (Rust, Dart, JavaScript, `agentic-peer`), so no peer needs to change. `max_window` bounds it (below `window` means `window`, so nothing grows by default). `auto_window` (off) grows a stream by itself: while data arrives it sends one `ping` at a time, and a stream that received over two thirds of its window before the `pong` doubles, up to `max_window`; it never shrinks. A grown stream can hold `max_window` unread, so set it to what each stream may cost in memory. Dart and JavaScript have `grant` and `maxWindow` (`stream.window`), not `auto_window`.
- What one stream does wrong ends that stream. What no peer may send ends the link.
- A channel whose `receive` answers `Failure::Permission` (what `Sealed::with_check` does) says its peer may no longer be on the link: the mux closes every stream still open with `CloseReason::Revoked` before the link is lost, so the peer reads `StreamEnd::Revoked` and not a lost link. The owner closes the channel afterwards.
- A proof admits a peer to the link and grants no protocol. Deciding who may open what is the caller's: `accept`, then serve, or `close_with(CloseReason::Refused, why)`. `metadata` is kept nowhere, and is not a credential of the link's.

## `cabuya::link`

```rust
pub const LINK_PROTOCOL: &str = "link/1";
pub fn instance() -> &'static str                // random per process
pub struct Accepted { pub peer: String, pub claims: BTreeMap<String, String>, pub protocol: String, pub metadata: Vec<u8> }  // non_exhaustive
pub type Handler = Box<dyn Fn(&dyn LinkStream, &Accepted) + Send + Sync>;
pub fn Services::with(self, protocol: &str, handler: Handler) -> Services   // Default
pub fn Services::offers(&self, protocol: &str) -> bool
pub fn Services::tokens(&self) -> impl Iterator<Item = &str>
pub struct Options { pub limits: Limits, pub recheck: Duration }  // non_exhaustive, Default: a second
pub type Admission = Box<dyn Fn(&PeerRecord, &[u8]) -> Result<(), &'static str> + Send + Sync>;   // asked at open and while running
Services::with_admission(self, protocol: &str, handler: Handler, admission: Admission) -> Self
pub fn serve_with<'c, C: Channel>(channel: &'c C, side: Side, options: Options, first: Option<Vec<u8>>, peer: &str, host: &Host<'_>, opener: impl FnOnce(&Mux<'c, C>) + Send)
pub fn serve_with_terminal<'c, C: Channel>(channel: &'c C, side: Side, options: Options, first: Option<Vec<u8>>, peer: &str, host: &Host<'_>, opener: impl FnOnce(&Mux<'c, C>) + Send, terminal: impl FnOnce())   // serve_with, and terminal() once when the mux is terminal, before the wait for running handlers
pub struct Host<'a> { pub store: &'a dyn PeerStore, pub services: &'a Services, pub events: &'a dyn Events }
pub fn serve<C: Channel>(channel: &C, side: Side, options: Options, first: Option<Vec<u8>>, peer: &str, host: &Host<'_>)
pub struct Capabilities { pub instance: String, pub offers: Vec<String>, pub window: Option<u32>, pub max_frame: Option<usize> }  // non_exhaustive; window and max_frame are the other side's `Limits`, None from an older peer: a host reports a difference in `window`
pub fn capabilities<C: Channel>(mux: &Mux<'_, C>, wait: Duration) -> Result<Capabilities, Failure>
pub struct Prefilled<'a, C>;  // impl Channel; Prefilled::new(channel, first)
```

`serve` runs one handler thread per stream, so a slow service holds up nothing, up to `Limits::max_streams`. Every open is decided against `PeerStore::get`, and every running stream again each `recheck`. A store error refuses, so nothing fails open. A taken-away grant closes the stream with `CloseReason::Revoked`, whose reason reaches the peer first. Call it after the proof, and close `channel` yourself afterwards.

## `cabuya::peers`

```rust
pub struct PeerRecord { pub peer: String, pub claims: BTreeMap<String, String>, pub grants: BTreeSet<String> }  // non_exhaustive, Default
pub fn PeerRecord::new(peer: &str) -> PeerRecord; pub fn PeerRecord::granting(self, protocol: &str) -> PeerRecord; pub fn PeerRecord::claiming(self, name: &str, value: &str) -> PeerRecord
pub trait PeerStore: Send + Sync { fn get(&self, peer: &str) -> Result<Option<PeerRecord>, Error>; fn put(&self, record: &PeerRecord) -> Result<(), Error>; fn remove(&self, peer: &str) -> Result<(), Error>; }
pub struct MemoryPeers;                              // Default; impl PeerStore
pub fn FilePeers::new(path: impl Into<PathBuf>) -> FilePeers   // impl PeerStore; read on every question, so a grant from another process counts at once
pub enum Event { LinkUp { peer }, LinkDown { peer }, Opened { peer, protocol }, Refused { peer, protocol }, Revoked { peer, protocol } }  // non_exhaustive; no payload, never a token or metadata
pub trait Events: Send + Sync { fn event(&self, event: &Event); }
pub struct NoEvents;                                 // impl Events
```

A host keeps peers in its own registry by implementing `PeerStore`, with no file or prompt involved. `claims` are set by the host and never interpreted here: derive who is speaking from them, not from anything a peer says. Only keys, peers and grants need to survive a restart.

## `cabuya::auth` (feature `sealed`)

```rust
pub const MAX_PROOF_MESSAGE: usize = 3072;
pub enum Method { WebCrypto, Noise }   // Copy, Eq, Hash, Display, FromStr ("webcrypto" | "noise", exactly); non_exhaustive
pub const Method::DEFAULT: Method;     // Noise: what a new identity gets when nothing names a method
pub const Method::ALL: [Method; 2];
pub const fn Method::name(self) -> &'static str
pub const fn Method::record_length(self, message: usize) -> usize   // the longest record: WebCrypto message + 17; Noise message + 38 per 65,518 bytes or part
pub fn Method::of_key(material: &KeyMaterial) -> Option<Method>

pub enum Identity { WebCrypto(coqui_peer_webcrypto::Identity), Noise(coqui_peer_noise::Identity) }   // Clone, From both; Debug shows method and id; non_exhaustive
pub fn Identity::generate(method: Method) -> Result<Identity, Error>
pub fn Identity::read(path: &Path) -> Result<Option<Identity>, Error>      // the key's type is its method
pub fn Identity::write_new(&self, path: &Path) -> Result<(), Error>       // never replaces; 0600 on Unix
pub fn Identity::open_or_create(path: &Path, method: Method) -> Result<Identity, Error>   // a key of another method there is refused
pub fn Identity::of_method(self, method: Method) -> Result<Identity, Error>
pub const fn Identity::method(&self) -> Method
pub fn Identity::peer_id(&self) -> PeerId
pub fn Identity::key_material(&self) -> KeyMaterial
pub fn read_identity(path: &Path) -> Result<Identity, Error>
pub fn create_identity(path: &Path, method: Method) -> Result<PeerId, Error>

pub enum Trust { Pinned(BTreeSet<PeerId>), Store(TrustStore), Asked(Arc<Asking>) }   // Clone; Trust::pinned(ids), Trust::asked(f); non_exhaustive
pub type Asking = dyn Fn(&KeyMaterial, bool, Option<SocketAddr>) -> bool + Send + Sync;   // the proven key, whether this side answered, the connection's source; true admits
pub enum Proof { WebCrypto(WebCrypto), Noise(Noise) }            // Clone, From both; impl coqui_peer::Method: Handshake = ProofHandshake, Output = Session, Error = ProofError
pub fn Proof::builder(identity: Identity) -> ProofBuilder        // identity's method, neither role yet
impl ProofBuilder {                                              // Debug, not Clone; must_use
    pub fn responders(self, trust: Trust) -> ProofBuilder          // whom prove_out admits
    pub fn initiators(self, trust: Trust) -> ProofBuilder          // whom prove admits
    pub fn key_limit(self, bytes: u64) -> ProofBuilder             // WebCrypto's session limit, Noise's key limit; 2^38 by default, no floor
    pub fn rekey(self, rekey: bool) -> ProofBuilder                // Noise: on by default; WebCrypto refuses true at build, having no key change
    pub fn max_message(self, bytes: usize) -> ProofBuilder         // WebCrypto's handshake message limit; refused at build under Noise, whose messages have fixed lengths
    pub fn build(self) -> Result<Proof, Error>                     // Error::Proof for a setting the method does not have
}
pub const fn Proof::method(&self) -> Method
pub fn Proof::peer_id(&self) -> PeerId
pub fn Proof::non_defaults(&self) -> Vec<NonDefault>             // coqui_builder::NonDefault: value and default are Shown (prints `bool(false)`, `Duration(5s)`)
pub enum Session { WebCrypto(..), Noise(..) }                    // From both; method(), id(), export(label, out)
pub enum ProofError { WebCrypto(..), Noise(..) }                 // the method's own error

pub fn prove<M: coqui_peer::Method, C: Channel>(method: &M, channel: &C, limit: Duration, max_message: usize) -> Result<Established<<M::Handshake as Handshake>::Output>, Error>
pub fn prove_out<M: coqui_peer::Method, C: Channel>(method: &M, channel: &C, limit: Duration, max_message: usize) -> Result<Established<<M::Handshake as Handshake>::Output>, Error>
pub struct Sealed<'a, C>;   // impl Channel
pub fn Sealed::new(channel: &'a C, session: impl Into<Session>) -> Sealed<'a, C>   // a session of either method
pub const fn Sealed::method(&self) -> Method
pub fn Sealed::with_check(self, allowed: impl Fn() -> bool + Send + Sync + 'a) -> Sealed<'a, C>   // a check that cannot tell must say no; a failed check answers `Permission` from `receive` once with the channel still open, so a mux tells each stream `Revoked`, and closes it on the next call
pub const fn Sealed::with_check_every(self, recheck: Duration) -> Sealed<'a, C>                    // a second unless set
```

`Trust::Asked` leaves the decision to the host, under either method: the closure runs once per handshake, only for a key whose proof held, and records nothing itself, so a host keeps its own accepted list and queues a proven stranger in the closure. `prove` passes `None` as the source.

An endpoint proves by one method, chosen when it is built: nothing on the wire names it and nothing negotiates it. A peer of the other method fails the handshake; the responder's `Error::OtherMethod` names both methods, from the shape of the first message (WebCrypto's is JSON, Noise's 32 bytes), and the side that connected sees only the connection end, as `Error::Channel`. A peer id is the hash of a key of one type, so only its own method proves it: store the method with the id, and never fall back on a failure.

`create_identity` never replaces a file. `prove` answers and `prove_out` connects, within `limit`, refusing a message over 3072 bytes, and set the channel's limit to `max_message` before the proof's last message goes out: size it with `Method::record_length`. `Sealed::set_max_message(n)` sets the channel's to its method's `record_length(n)`. Under Noise that is the worst case at any key limit the sender runs: 19 bytes per part of 65,518 bytes, and 19 more per part for a change of key in front of it (9,766 at 16 MiB), since a receiver cannot know the sender's limit.

## `cabuya::pairing` (feature `sealed`)

```rust
pub fn desk() -> Result<PairingDesk, Error>   // PIN runs only, coqui-peer's defaults
pub fn desk_with(options: PairingOptions, limits: DeskLimits) -> Result<PairingDesk, Error>   // PIN lifetime, attempts, alphabet, pending bounds; invitation and device stay off
pub fn respond<C: Channel>(channel: &C, identity: &Identity, store: &TrustStore, desk: &PairingDesk, source: Option<SocketAddr>, limit: Duration) -> Result<Paired, Error>
pub fn join<C: Channel>(channel: &C, identity: &Identity, pin: Pin, store: &TrustStore, limit: Duration) -> Result<Paired, Error>
pub const MARKER: &[u8];                  // what a joiner sends first on a listener that also takes proofs: not 32 bytes (Noise's first message), not JSON (WebCrypto's)
pub fn starts_pairing(first_message: &[u8]) -> bool   // first_message == MARKER
pub fn join_shared<C: Channel>(channel: &C, identity: &Identity, pin: Pin, store: &TrustStore, limit: Duration) -> Result<Paired, Error>   // MARKER, then join
pub struct PairingHost<'a> { pub identity: &'a Identity, pub store: &'a TrustStore, pub desk: &'a PairingDesk }   // Clone, Copy
pub enum Answered<T> { Paired(Paired), Proven(Established<T>) }   // non_exhaustive
pub struct Unanswered { pub pairing: bool, pub error: Error }   // non_exhaustive, Display, Error: pairing is true when the joiner sent MARKER, so a refusal of a pairing is never read as a failed proof
pub fn answer<M: coqui_peer::Method, C: Channel>(channel: &C, method: &M, max_message: usize, pairing: Option<PairingHost<'_>>, source: Option<SocketAddr>, limit: Duration) -> Result<Answered<..>, Unanswered>   // reads the first message: MARKER runs respond (None: the joiner is sent away unanswered), anything else prove(method, ..) with that message kept; limit bounds both
```

A pairing and a proof begin with the same handshake message under both methods, so one listener tells them apart only by `MARKER`, sent first by `join_shared`; an unmarked `join` is for a listener that only pairs. A desk with no PIN shown refuses a joiner as one with none shown, and a wrong guess burns the PIN. PIN pairing over any channel in binary frames, with the PIN shown by the responder, by `identity`'s method: a joiner of the other method fails the handshake, and the responder's error names it. The key stored names its method, so the two then connect by it. A refusal's error says which: a wrong PIN, an expired one, a mode not offered. Under Noise, a joiner that proves itself and then says nothing is sent away as expired within `limit`, or Noise's own minute if that is sooner.

## `cabuya::forward` (feature `forward`)

`tcp/1`: one TCP connection per stream, in either direction. The widest capability there is, so a peer needs `tcp/1` and a grant `tcp/1:RULE` that matches the target: a token naming one target (an exact rule, as before) or a `Rule` pattern the operator wrote. A refusal happens before anything connects, and removing either grant ends running streams within `Options::recheck`.

```rust
pub const TCP_PROTOCOL: &str = "tcp/1";
pub struct BadTarget;                                 // the error of Target's FromStr; Display, Error
pub struct Target;                                    // FromStr, Display: HOST:PORT in one spelling; "localhost:80" is not "127.0.0.1:80"
Target::connect_within(&self, timeout: Duration) -> io::Result<TcpStream>   // each resolved address up to `timeout`; Err for zero
pub struct BadRule;                                   // the error of Rule's FromStr; Display, Error
pub struct Rule;                                      // Clone, Eq, Hash, FromStr, Display, From<&Target>: HOST:PORT, HOST = name | *.suffix | * | address | CIDR (IPv6 in brackets), PORT = N | A-B | *
Rule::matches(&self, target: &Target) -> bool         // nothing resolves: an address/network rule matches address targets only, a name rule name targets only, * both
Rule::grant_token(&self) -> String                    // "tcp/1:RULE"
Rule::from_grant_token(token: &str) -> Option<Rule>   // a target's token is that target's rule
pub fn grant_rule(record: PeerRecord, rule: &Rule) -> PeerRecord
pub struct Options { pub connect_timeout: Duration /*10 s*/, pub chunk: usize /*16 KiB*/, pub stalled: Duration /*30 s*/, pub idle_min: Duration /*1 ms*/, pub idle_max: Duration /*10 ms*/, pub watch: Duration /*50 ms*/, pub listen_poll: Duration /*50 ms*/ }   // non_exhaustive, Default = what pump, listen and serving use
Options::validate(&self) -> Result<(), BadOptions>    // zero durations or chunk, and idle_max < idle_min, are refused; nothing else is limited
pub struct BadOptions;                                // Display names the option; Error
pub fn serving_with(services: Services, options: Options) -> Result<Services, BadOptions>
pub fn pump_with<S: LinkStream + ?Sized>(stream: &S, connection: &TcpStream, options: &Options)   // invalid options refuse the stream
pub fn listen_with<C: Channel>(mux: &Mux<'_, C>, listener: &TcpListener, target: &Target, options: &Options) -> io::Result<()>   // InvalidInput for invalid options
Target::grant_token(&self) -> String                  // "tcp/1:HOST:PORT"
Target::from_grant_token(token: &str) -> Option<Target>
pub fn grant(record: PeerRecord, target: &Target) -> PeerRecord
pub fn serving(services: Services) -> Services        // adds tcp/1
pub fn connect<'c, C: Channel>(mux: &Mux<'c, C>, target: &Target) -> Result<Stream<'c, C>, Failure>
pub fn pump<S: LinkStream + ?Sized>(stream: &S, connection: &TcpStream)   // until both directions end; FIN maps to finish
pub fn listen<C: Channel>(mux: &Mux<'_, C>, listener: &TcpListener, target: &Target) -> io::Result<()>   // until the link ends
pub fn is_loopback(address: IpAddr) -> bool
```

A stream ended any way but in order resets the connection, so a cut-short transfer does not read as finished. `Sealed::send` answering `NoProgress` means that message was not taken: send it again.

Gotchas: whoever can connect to a `listen`er gets what this peer was granted, so bind it to loopback. `Limits::max_message` has to allow 16 KiB. A name is resolved by the side connecting, each time, so a grant on a name trusts that name's DNS.

## The `cabuya` program (feature `cli`)

`cargo install --git https://github.com/Boriken-Dev/cabuya --features cli`; the binary adds no library item. Global options: `--home DIR` (else `CABUYA_HOME`, else `%APPDATA%\cabuya` on Windows, `$XDG_CONFIG_HOME/cabuya` or `~/.config/cabuya`), `--method noise|webcrypto` (only for an identity that does not exist yet), `--grants FILE` (for `serve`, `pair`, `grant` and `revoke`; default `grants.json` in the folder). The folder holds `identity`, `served/` and `dialed/` (coqui-peer trust stores), `grants.json` (`FilePeers`) and `hosts.json`.

```text
cabuya id                                           # prints "<peer id> <method>"; creates the identity
cabuya pair [--listen ADDR=0.0.0.0:7710] [--grant TOKEN]...    # prints listening/peer/pin lines, pairs one joiner, "paired <id>", exits 0
cabuya pair --join ADDR --name NAME [--pin PIN] [--at SERVE_ADDR]   # no --pin: one line of stdin; prints "paired NAME <host id>", "this-peer <id>"
cabuya serve [--listen ADDR=0.0.0.0:7711] [--pair [--grant TOKEN]...]   # runs until stopped; events and refusals on stderr
cabuya grant PEER TOKEN...      # TOKEN: a protocol, or tcp/1:HOST:PORT (a Rule); the latter also grants tcp/1
cabuya revoke PEER [TOKEN...]   # no TOKEN: drops the peer's record and its key
cabuya connect HOST (TARGET | --stream PROTOCOL [--metadata TEXT])   # one stream on stdin/stdout; an ssh ProxyCommand
cabuya forward HOST --local [ADDR:]PORT=TARGET ...                   # prints "listening ADDR TARGET"; ends when the link does
cabuya hosts | cabuya forget NAME
```

Gotchas: a link is `u32` little-endian length-prefixed messages over TCP, proved with the pinned key of the named host (`dialed/`), so a host that presents another key is refused. `serve` re-reads the grants file on every question and re-checks running streams each second, and a revoked key ends them (`Revoked`). A PIN lasts ten minutes and one attempt. Exit codes: 0 done, 2 any failure with its reason on stderr. One identity has one method, so a host of the other method needs another `--home`. The program does not reconnect.

## Errors

`cabuya::Error` (`#[non_exhaustive]`): `Channel(Failure)`, `Proof(String)`, `Identity(String)` and, with `sealed`, `OtherMethod { peer: Method, ours: Method }` (a responder seeing the other method's first message: report it as a method mismatch, never as a key to accept). A message never quotes a key file.

## Gotchas

- A frame adds 5 bytes, so the link's own limit has to be at least `max_frame + 5`. `Mux` blocks a thread: on Tokio, run it on `spawn_blocking`.
- A browser takes a self-signed certificate only for WebTransport with `serverCertificateHashes`.
- A key file's permissions are set only on Unix; elsewhere it takes its directory's. Nothing checks them on reading.
- A link of `cabuya` and one of `agentic-peer` talk to each other: the protocol names (`link/1`, `tcp/1`) and every frame byte are the same.
