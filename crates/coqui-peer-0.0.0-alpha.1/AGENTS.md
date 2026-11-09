# `coqui-peer` — API header

Header-file-style reference for the `coqui-peer` crate: every public item with its signature,
contract and gotchas, so the crate can be used without reading its source. It ships inside the
package and is self-contained. Development documentation lives with the source at
<https://github.com/Boriken-Dev/coqui-peer>.

| Feature | Adds | Needed for | Default |
| --- | --- | --- | --- |
| `serde` | `serde`: `Serialize`/`Deserialize` for `PeerId` and `Digest` as their text | text formats the consumer picks | off |
| `tokio` | `tokio` (`io-util`, `time`): module `asynch` | async drivers and frame functions | off |
| `pin` | `spake2`, `hkdf`, `hmac`, `rand_core` (`getrandom`) | PIN pairing (`Pairing::pin`), `Random::os` | off |
| `coqui-transport` | `coqui-transport`: module `transport` | `PeerFrame`, and a `coqui_transport::Channel` as a `MessageTransport` | off |

MSRV Rust 1.89. A default build depends on `data-encoding`, `guatiao` (`derive`), `guatiao-serde` (`json`),
`sha1`, `sha2`, `subtle` and `zeroize`. The settings types below are guatiao values in every build: every
key is optional (an absent one is its default), an unknown key is refused, and `guatiao_serde::text`
(toml, json, yaml) reads and writes them; a duration reads as seconds, `[secs, nanos]` or `{secs, nanos}`. Version `0.0.0-alpha.1` is a pre-release; the API may change in any release before
1.0.

Imports: the crate root holds the ids, the store and its queue, `KeyMaterial`, `Clock`,
`Random`, `NonDefault`, the `Handshake`/`Method` traits, the blocking drivers and frame functions,
and `Error`. Public modules: `pair` (pairing), `asynch` (feature `tokio`), `file` and `pem`.

No `unsafe` code (`#![forbid(unsafe_code)]`). Process-wide state: one counter that keeps
temporary file names unique. Invitation secrets, PINs and the PIN exchange's keys are zeroed
when dropped (`zeroize`); a `Channel`'s `Debug` hides its binding. No TLS, no crypto transport;
nothing reads a file or an environment variable unless the caller passes a directory.
Everything is built in code; `serde` is the only way text formats come in, and the consumer
picks the format.

## Ids (`coqui_peer::{PeerId, Digest}`)

```rust
#[non_exhaustive] pub enum Digest { Sha256, Sha384, Sha512, Sha1 }
impl Digest { pub const fn name(self) -> &'static str; pub const fn size(self) -> usize;
              pub fn of(self, data: &[u8]) -> PeerId; }     // FromStr: "sha256", ...; "md5" is an error
pub struct PeerId;      // Copy, Eq (constant time), Ord, Hash, Display = canonical, FromStr
impl PeerId {
    pub fn of(data: &[u8]) -> PeerId;                // SHA-256 of a public key or certificate encoding
    pub fn from_bytes(bytes: [u8; 32]) -> PeerId;    // SHA-256
    pub fn digest(&self) -> Digest;  pub fn as_bytes(&self) -> &[u8];
    pub fn hex(&self) -> String;                     // 64 lowercase hex for sha256
    pub fn canonical(&self) -> String;               // "<alg>:<hex>", also Display
    pub fn to_base32_grouped(&self) -> String;       // sha256 only: 13 groups of 4, '-' separated
}
```

Canonical text is `<alg>:<hash>`: `sha256:<64 lowercase hex>`. `FromStr` accepts `sha256:<hex>`
(also `sha384:`, `sha512:`, `sha1:`), bare hex of 64, 96, 128 or 40 characters, colon-separated
hex pairs (`AA:BB:...`, with or without the prefix), and 52 base32 characters for SHA-256
(groups optional). An MD5 digest (name `md5` or 32 hex characters) is `Error::WeakDigest`.
Which digests a method accepts is its own allow-list of `Digest` values. Compare ids through
`==`, not through strings.

## Key material (`coqui_peer::KeyMaterial`)

```rust
pub struct KeyMaterial { pub method: String, pub label: String, pub bytes: Vec<u8> }
impl KeyMaterial {
    pub fn new(method: impl Into<String>, label: impl Into<String>, bytes: impl Into<Vec<u8>>)
        -> Result<KeyMaterial, Error>;     // checks the fields
    pub fn id(&self) -> PeerId;            // SHA-256 of bytes
}
pub const MAX_KEY_BYTES: usize = 65536;
```

`method`: `[a-z0-9-]`, 1 to 32 characters, the `<method>` of `coqui-peer-<method>`. `label`: PEM label,
`[A-Z0-9 ]`, 1 to 64 characters (`CERTIFICATE` for `tls`). `bytes`: 1 to `MAX_KEY_BYTES`.

## Trust store (`coqui_peer::TrustStore`)

```rust
pub struct TrustStore;                     // Clone
// Builds the store: limits(WaitingLimits), ipv6_prefix(u8), when_full(WhenFull), record_interval(Duration),
// rotation(RotationOptions), clock(Clock), settings(TrustStoreSettings) (replaces all but the clock), build()
// -> Result<TrustStore, Error>. The last setter wins.
pub struct TrustStoreBuilder;
#[non_exhaustive] pub struct TrustStoreSettings { pub waiting: WaitingSettings, pub rotation: RotationOptions }
#[non_exhaustive] pub struct WaitingLimits { pub per_source: usize, pub total: usize, pub rerecord: Duration }   // Default: 16, 1000, 60 s
#[non_exhaustive] pub struct WaitingEntry { pub id: PeerId, pub source: Option<SocketAddr> }
pub const FORMAT_VERSION: u32 = 1;
impl TrustStore {
    pub fn open(state_dir: &Path) -> Result<TrustStore, Error>;        // creates <state_dir>/peers/{accepted,waiting}
    pub fn builder(state_dir: &Path) -> TrustStoreBuilder;           // the same, with settings
    pub fn settings(&self) -> TrustStoreSettings;  pub fn non_defaults(&self) -> Vec<NonDefault>;
    pub fn rotation(&self) -> RotationOptions;   pub fn clock(&self) -> &Clock;
    pub fn waiting_queue(self, WaitingSettings) -> TrustStore;        // a new, empty waiting queue; rotation and the clock stay
    pub fn waiting(&self) -> Result<Vec<WaitingEntry>, Error>;
    pub fn accepted(&self) -> Result<Vec<PeerId>, Error>;             // retired keys not listed
    pub fn accepted_key(&self, id: &PeerId) -> Option<KeyMaterial>;   // None once retired
    pub fn waiting_key(&self, id: &PeerId) -> Option<KeyMaterial>;
    pub fn accept(&self, id: &PeerId) -> Result<(), Error>;             // waiting -> accepted
    pub fn accept_key(&self, key: &KeyMaterial) -> Result<PeerId, Error>;   // a retired key: current again
    pub fn remove(&self, id: &PeerId) -> Result<bool, Error>;         // also its pending successor
    pub fn is_accepted(&self, id: &PeerId) -> bool;                   // false from Retires on
    pub fn record_waiting(&self, key: &KeyMaterial, source: Option<SocketAddr>);
    // invitations
    pub fn invite(&self, random: &Random, options: InvitationOptions, label: &str) -> Result<Invitation, Error>;
    pub fn redeem(&self, secret: &[u8; 32]) -> Result<Invitation, Error>;   // uses one; atomic
    pub fn invitations(&self) -> Result<Vec<InvitationEntry>, Error>;
    pub fn revoke_invitation(&self, id: &InvitationId) -> Result<bool, Error>;
    // rotation
    pub fn announce_successor(&self, old: &PeerId, new: &KeyMaterial) -> Result<(), Error>;
    pub fn successor_key(&self, new: &PeerId) -> Option<KeyMaterial>;
    pub fn prove_successor(&self, new: &PeerId) -> Result<(), Error>;        // records Proven:
    pub fn promote_successor(&self, new: &PeerId) -> Result<PeerId, Error>; // returns old; needs a proof
    pub fn successors(&self) -> Result<Vec<Successor>, Error>;
    pub fn retiring(&self) -> Result<Vec<Retiring>, Error>;
    pub fn take_rotation_events(&self) -> Vec<RotationEvent>;
}
pub struct InvitationOptions { pub valid_for: Duration, pub uses: u32 }        // Default: 24 h, 1
pub struct Invitation { pub secret: [u8; 32], pub expires: SystemTime, pub uses: u32, pub label: String }
impl Invitation { pub fn id(&self) -> InvitationId; }                           // Debug hides the secret
pub struct InvitationId;   // Copy, Ord, Display/FromStr = 64 hex; InvitationId::of(&secret) = SHA-256
pub struct InvitationEntry { pub id: InvitationId, pub expires: SystemTime, pub uses: u32, pub label: String }
pub const MAX_LABEL: usize = 128;
pub struct RotationOptions { pub retire_after: Duration, pub require_approval: bool, pub allow_unproven: bool }  // 24 h, false, false
pub struct Successor { pub old: PeerId, pub new: PeerId, pub proven: Option<SystemTime> }
pub struct Retiring { pub id: PeerId, pub successor: Option<PeerId>, pub retires: SystemTime }
#[non_exhaustive] pub enum RotationEvent { Announced { old, new }, Promoted { old, new, retires } }
pub struct Clock;    // Clone; Clock::system(), Clock::from_fn(|| SystemTime), now()
pub struct Random;   // Clone; Random::from_fn(|&mut [u8]| -> Result<(), Error>), fill(); feature pin: Random::os()
```

**Key-proof contract (every method must meet it).** `record_waiting` is called only after the
peer proved, inside the handshake, that it holds the private key of the key it presented. A
presented key proves nothing: public keys and certificates are copied freely, and recording
them unproven lets a stranger queue somebody else's identity. A failed proof records nothing.

Pairing: a server turns away an untrusted, key-proven peer and records it; the operator lists
`waiting`, then `accept`s or `remove`s by id; the next handshake finds `is_accepted`.
`record_waiting` is bounded: invalid keys ignored, at most `per_source` entries per source (a
newcomer is refused, unless `WhenFull::ReplaceOldest`, which lets peers sharing a source flush each
other), one new peer per source every `record_interval`, the same peer from the same source at
most every `rerecord`, `total` in all. A flood can fill a source, so nobody new is recorded from it
until an operator clears it; it cannot remove anyone. A source is an IPv4 address (an IPv4-mapped IPv6 address counts as
its IPv4 address) or an IPv6 prefix of `ipv6_prefix` bits, /64 by default, since one
holder controls every address of a /64; 128 counts each IPv6 address alone. The full address
is still stored with the entry. `None` sources share one bucket. `is_accepted` and the `*_key`
readers check that the stored bytes hash to the id.

**The queue without the store.** `WaitingQueue` is `record_waiting`'s bounding with no
storage, for a host that keeps waiting entries elsewhere (a table, a database):

```rust
pub struct WaitingQueue;   // Clone, Debug
// Builds the queue: limits(WaitingLimits), ipv6_prefix(u8; above 128 is 128), when_full(WhenFull),
// record_interval(Duration; zero is no limit), settings(WaitingSettings) (replaces all), build() -> WaitingQueue.
pub struct WaitingQueueBuilder;
#[non_exhaustive] pub struct WaitingSettings { pub limits: WaitingLimits, pub ipv6_prefix: u8, pub when_full: WhenFull,
                                               pub record_interval: Duration }   // Default: limits default, 64, Refuse, 1 s
impl WaitingQueue {
    pub fn builder(limits: WaitingLimits) -> WaitingQueueBuilder;   // IPv6 grouped by /64, Refuse, 1 s
    pub fn new(limits: WaitingLimits) -> WaitingQueue;          // builder(limits).build()
    pub fn settings(&self) -> WaitingSettings;
    pub fn when_full(&self) -> WhenFull;  pub fn record_interval(&self) -> Duration;
    pub fn limits(&self) -> WaitingLimits;  pub fn ipv6_prefix(&self) -> u8;
    pub fn admit(&mut self, id: PeerId, source: Option<SocketAddr>, stored: usize, now: Instant) -> Admission;
    pub fn remember(&mut self, id: PeerId, source: Option<SocketAddr>, at: Instant) -> Vec<PeerId>;
    pub fn forget(&mut self, id: &PeerId);
    pub fn prune(&mut self, now: Instant, stored: impl Fn(&PeerId) -> bool);
    pub fn len(&self) -> usize;  pub fn is_empty(&self) -> bool;
}
#[non_exhaustive] pub struct Admission { pub record: bool, pub evict: Vec<PeerId> }
#[non_exhaustive] pub enum WhenFull { Refuse, ReplaceOldest }  // Copy, Default = Refuse; text "refuse", "replace-oldest"
pub const DEFAULT_RECORD_INTERVAL: Duration;                   // 1 s
```

`admit` decides one peer: with `stored` (the host's count of waiting entries) at `total` or
above, `record` is false and nothing is evicted (the newcomer waits); the same id from the same
source within `rerecord` is not recorded again; a source that recorded anyone within
`record_interval` records nobody new; a source at `per_source` refuses the newcomer, or under
`WhenFull::ReplaceOldest` records it with `evict` listing that source's oldest entries, which the host
removes. Otherwise `record` is true and `evict` is empty. The host skips `admit`
for an id it already stores. `remember` seeds the queue from stored entries (placed by time;
returns the ones past `per_source`), `forget` drops an id the host accepted or removed.
`prune` forgets every entry `stored` says the host no longer holds once its `rerecord` time has
passed (within it, the entry still throttles that peer from that source): for a host whose
entries another process may accept or remove, whose queue otherwise grows with every source it
has seen. `len` counts the entries remembered. Sources are bucketed as in the store. The queue
keeps what it admitted in memory only; `TrustStore::record_waiting` prunes it against the
`waiting/` directory before each decision.

**Invitations.** `invite` draws a 32-byte secret from `random` and writes only its SHA-256
(the `InvitationId`), its expiry, uses and label: the caller hands the secret to the joiner
with the inviter's id, and the store cannot leak it. `redeem` claims the file by renaming it,
so of concurrent redemptions of a single-use invitation exactly one wins; it returns the uses
left after it and removes the file at 0. Errors: `NotInvited` (unknown, used up, revoked),
`Expired` (the file is removed). `InvalidOptions` only for meaningless values: zero uses or
validity, a label that is not one line of at most `MAX_LABEL` bytes without surrounding
spaces. `valid_for` is counted in whole seconds; longer ones and more uses are the caller's
choice and only widen who may redeem it.

**Rotation.** `announce_successor(old, new)` is for a key the holder of `old` sent over a
session `old` authenticated; `old` must be accepted and not retiring (`NotAccepted`), `new`
not accepted (`AlreadyAccepted`), not `old` (`Material`), and not the pending successor of
another key that is still accepted (`SuccessorTaken`: otherwise a peer that learned someone
else's successor could claim it, and promoting it would retire the wrong key); a later
announcement replaces the pending one. A method calls `prove_successor(new)` once the peer
proves `new` in a handshake (a `Proven:` line; the first proof's time is kept), then
`promote_successor(new)` unless `RotationOptions::require_approval` (then the caller calls it
to approve). `promote_successor` refuses a successor nobody proved (`NotProven`) unless
`allow_unproven`, which lets the old key's holder alone vouch for a key that may not be its
own. Promotion accepts `new` and gives `old` `Retires: now + retire_after`; from then on
`is_accepted(old)`, `accepted_key(old)` and `accepted()` drop it. `retire_after` zero cuts the
old key off at once (connections still on it fail their next handshake); a longer one keeps a
possibly stolen old key usable that much longer. `successor_key(new)` is `Some` only while `old`
is accepted, not retiring and names `new`. `remove(old)` deletes its pending successor. Events
are returned, not reported: `take_rotation_events` drains this process's queue (at most 1024,
oldest dropped); `successors()` and `retiring()` read the state on disk.

## Files and PEM (`coqui_peer::file`, `coqui_peer::pem`)

The writes and the block format the store uses, for method crates that keep files of their own
(an identity, a configuration).

```rust
#[non_exhaustive] pub enum Access { Default, OwnerOnly }      // Default: Default; OwnerOnly = 0600 on Unix
pub fn file::write_new(path: &Path, contents: &[u8], access: Access) -> io::Result<()>;
pub fn file::replace(path: &Path, contents: &[u8], access: Access) -> io::Result<()>;

#[non_exhaustive] pub struct pem::Block<'a> { pub headers: Vec<&'a str>, pub label: String, pub bytes: Vec<u8> }
pub fn pem::encode(label: &str, bytes: &[u8]) -> String;      // 64-character lines, LF, ends in "\n"
pub fn pem::decode(text: &str) -> Option<pem::Block<'_>>;     // the first block and the lines before it
```

- Both writes create parent directories, write a temporary file beside `path` (named by the
  process id, the time and a per-process counter, so writes in one instant never share one),
  sync it, then give it the target's name. `write_new` hard-links it, which fails with
  `AlreadyExists` when `path` exists: an existing file is never replaced, and of concurrent
  writers exactly one wins. `replace` renames it over the target, so a reader sees the old
  contents or the new. The temporary file is removed on every path.
- `Access::OwnerOnly` is mode `0600` on Unix; on Windows the file inherits its directory's ACL.
- `pem::decode` returns `None` when no block is complete or its body is not base64; header
  lines are trimmed.

## Handshakes and drivers (`coqui_peer`, `coqui_peer::asynch`)

```rust
pub enum Framing { Stream, Message }               // Copy, Eq, Hash
pub struct Established<S> { pub peer: PeerId, pub key: Option<KeyMaterial>, pub session: S }
pub trait Handshake {
    type Output; type Error: std::error::Error + Send + Sync + 'static;
    fn framing(&self) -> Framing;
    fn receive(&mut self, input: &[u8]) -> Result<(), Self::Error>;   // bytes (Stream) or one message (Message)
    fn transmit(&mut self) -> Result<Option<Vec<u8>>, Self::Error>;   // next thing to send; call until None
    fn is_complete(&self) -> bool;
    fn finish(self) -> Result<Established<Self::Output>, Self::Error>;
}
pub trait Method {
    type Handshake: Handshake;
    fn name(&self) -> &'static str;
    fn initiate(&self) -> Result<Self::Handshake, <Self::Handshake as Handshake>::Error>;
    fn respond(&self, source: Option<SocketAddr>) -> Result<Self::Handshake, <Self::Handshake as Handshake>::Error>;
}
pub trait MessageTransport { fn send(&mut self, &[u8]) -> io::Result<()>; fn recv(&mut self) -> io::Result<Vec<u8>>; }
pub fn drive_stream<H: Handshake, T: Read + Write>(H, &mut T) -> Result<Established<H::Output>, DriveError<H::Error>>;
pub fn drive_messages<H: Handshake, T: MessageTransport>(H, &mut T) -> Result<Established<H::Output>, DriveError<H::Error>>;
pub fn complete_stream<H: Handshake, T: Read + Write>(H, &mut T) -> Result<H, DriveError<H::Error>>;
pub fn complete_messages<H: Handshake, T: MessageTransport>(H, &mut T) -> Result<H, DriveError<H::Error>>;
pub trait ReadTimeout { fn set_read_timeout(&self, Option<Duration>) -> io::Result<()>; }     // TcpStream, &TcpStream
pub trait WriteTimeout { fn set_write_timeout(&self, Option<Duration>) -> io::Result<()>; }   // TcpStream, &TcpStream
pub fn drive_stream_until<H: Handshake, T: Read + Write + ReadTimeout + WriteTimeout>(H, &mut T, deadline: Instant) -> Result<Established<H::Output>, DriveError<H::Error>>;
pub fn complete_stream_until<H: Handshake, T: Read + Write + ReadTimeout + WriteTimeout>(H, &mut T, deadline: Instant) -> Result<H, DriveError<H::Error>>;
pub fn drive_messages_until<H: Handshake, T: MessageTransport + ReadTimeout + WriteTimeout>(H, &mut T, deadline: Instant) -> Result<Established<H::Output>, DriveError<H::Error>>;
pub fn complete_messages_until<H: Handshake, T: MessageTransport + ReadTimeout + WriteTimeout>(H, &mut T, deadline: Instant) -> Result<H, DriveError<H::Error>>;
#[non_exhaustive] pub enum DriveError<E> { Io(io::Error), Handshake(E), TooLong, TimedOut }
pub const MAX_MESSAGE: usize = 1 << 20;
pub fn write_frame<W: Write + ?Sized>(io: &mut W, message: &[u8]) -> io::Result<()>;   // length, message, flush
pub fn read_frame<R: Read + ?Sized>(io: &mut R) -> io::Result<Vec<u8>>;
// feature tokio, module asynch: MessageTransport (async fns), drive_stream, drive_messages,
// complete_stream, complete_messages, their _until forms (no timeout bounds: tokio's timer),
// write_frame, read_frame (AsyncWrite / AsyncRead + Unpin)
```

**Deadlines.** The `_until` forms run the same loops and end at `deadline` (a
`std::time::Instant`) with `DriveError::TimedOut`. The blocking ones set the transport's read
timeout before every read and its write timeout before every write and flush to what is left
(`ReadTimeout`, `WriteTimeout`, both implemented for `TcpStream` and `&TcpStream`, so a socket
shared with a thread that may shut it down is driven through a reference), map a transport
error after the deadline, or a `TimedOut` / `WouldBlock` one, to `TimedOut`, and clear both
timeouts (`None`) before returning. A transport that cannot time out implements the traits as
no-ops and is then bounded only between reads and writes. The async ones wrap the loop in
`tokio::time::timeout_at`, so the runtime needs its timer enabled (tokio panics otherwise).

**Frames.** One format for every length-prefixed message: four big-endian length bytes, then
the message, at most `MAX_MESSAGE`. `write_frame` refuses a longer message with
`InvalidInput` and writes nothing; `read_frame` refuses a longer declared length with
`InvalidData` before reading it, and a stream that ends inside a frame with `UnexpectedEof`.
The reader's buffer grows as bytes arrive, so a declared length costs nothing until it is
sent. The drivers use them for `Framing::Message` over a stream and report a frame above the
limit as `DriveError::TooLong`.

**Over `coqui-transport` (feature `coqui-transport`, module `coqui_peer::transport`).**

```rust
pub struct PeerFrame;   // Clone, Copy, Debug, Default, Eq, Hash; Codec of coqui_transport::frame, Decoder = LengthDecoder
impl PeerFrame { pub const fn prefix() -> LengthPrefix; }   // LengthPrefix::u32_be(): empty messages allowed
pub struct ChannelTransport<C: Channel>;    // Debug
impl<C: Channel> ChannelTransport<C> { pub const fn new(C) -> Self; pub const fn channel(&self) -> &C; pub fn into_inner(self) -> C; }
impl<C: Channel> MessageTransport for ChannelTransport<C>;   // also for C = Box<dyn Channel>
impl<C> ReadTimeout for ChannelTransport<C>;  impl<C> WriteTimeout for ChannelTransport<C>;
```

`PeerFrame` writes and reads the bytes of `write_frame` and `read_frame` (the vectors in
`tests/vectors/peer_frame.json` hold all three to the same cases), so a `StreamChannel` on it and
`drive_stream` over a socket talk to each other. The message bound is the channel's, not
`MAX_MESSAGE`. `ChannelTransport` drives a handshake over any channel: `send` is `send_timeout`
and `recv` is `receive`, each bounded by the timeout the `_until` drivers set (none otherwise); a
message over `MAX_MESSAGE` either way, or a `Failure::Protocol`, is `InvalidInput` going out and
`InvalidData` coming in (`DriveError::Io`, as for any message transport); an orderly end is `UnexpectedEof`; `Timeout` and `NoProgress` are `TimedOut`, `PathLost`
`ConnectionReset`, the certificate, proof and permission failures `PermissionDenied`, and the
`Failure` is the error's source. The channel is not an authenticated peer: the handshake proves
it. `coqui-transport` never depends on `coqui-peer`.

The handshake does no I/O; the caller owns the transport. `Stream` handshakes treat input as
bytes (chunks may split or join); `Message` handshakes get exactly one message per `receive`.
`drive_stream` length-prefixes `Message` handshakes with four big-endian bytes (at most
`MAX_MESSAGE`). `drive_messages` hands transport messages over whole for either framing, so
use it only with a reliable, ordered transport. `finish` before `is_complete` is an error;
bytes received past the end of the handshake stay in `Output`. `Output` is `()` for a method
that only authenticates, session keys, or an encrypted channel.

`complete_stream` / `complete_messages` run the same loop and return the handshake once
`is_complete`, everything it queued sent, without calling `finish`: for a method with a finish
of its own, such as a TLS server that does not authenticate its client at that layer and so
has no `PeerId` to put in `Established` (`coqui-peer-tls`'s `TlsHandshake::finish_unauthenticated`).
`drive_*` is `complete_*` followed by `finish`. `is_complete` says the exchange is over;
`finish` succeeds when the method verified a peer.

## Pairing (`coqui_peer::pair`)

How a joiner and a responder come to accept each other's keys, over any reliable, ordered
channel: TLS (`coqui-peer-tls` runs it), or one that proves nothing. Sans-IO and message-framed:
each side's `transmit` gives whole messages (call until `None`), `receive` takes one. Every
message starts with the version byte 1 and a tag. The crate shows nothing and asks nobody:
what a person sees or decides is returned as data, and the caller acts.

```rust
pub struct Channel { pub local: KeyMaterial, pub remote: Option<KeyMaterial>, pub verified: bool,
                     pub binding: Option<Vec<u8>> }
pub struct Pairing;            // Clone; Debug hides secrets
impl Pairing {
    pub fn invitation(inviter: PeerId, secret: [u8; 32]) -> Pairing;
    pub fn pin(pin: Pin, role: PinRole) -> Pairing;                 // feature pin
    pub fn device(label: &str) -> Pairing;
    pub fn allow_unverified(self, bool) -> Pairing;  pub fn allows_unverified(&self) -> bool;
    pub fn random(self, Random) -> Pairing;                         // feature pin; default Random::os()
    pub fn mode(&self) -> Mode;  pub fn inviter(&self) -> Option<PeerId>;
    pub fn join(self, Channel) -> Result<PairingJoiner, PairingError>;
}
pub enum PinRole { Typed, Shown }
pub struct PairingJoiner;      // transmit() -> Option<Vec<u8>>, receive(&[u8]) -> Result<(), PairingError>,
                               // shown() -> Option<&Shown>, is_complete(), finish() -> Result<Paired, PairingError>
pub struct PairingResponder;   // new(Channel, &TrustStore, &PairingDesk, source: Option<SocketAddr>);
                               // transmit, receive, awaiting() -> Option<Ticket>, resume(Decision),
                               // joiner() -> Option<&KeyMaterial>, deadline() -> Option<SystemTime>,
                               // time_left() -> Option<Duration>, check_deadline(), is_complete(), finish()
#[non_exhaustive] pub enum Shown { Pin(PairingCode), UserCode { code: UserCode, expires: SystemTime } }
#[non_exhaustive] pub struct Paired { pub peer: KeyMaterial, pub mode: Mode }
#[non_exhaustive] pub enum Mode { Invitation, Pin, Device }
#[non_exhaustive] pub enum PairRefusal { WrongCode, Expired, Exhausted, Throttled, Denied, Disabled }
#[non_exhaustive] pub enum PairingError { Refused(PairRefusal), Protocol, Unverified, Local(Error) }

#[non_exhaustive] pub struct CodeAlphabet { pub chars: String, pub len: usize }   // pin(): "0123456789" x 6; user_code(): "BCDFGHJKLMNPQRSTVWXZ" x 8
impl CodeAlphabet { pub fn new(chars: impl Into<String>, len: usize) -> Self; pub fn combinations(&self) -> u128; pub fn check(&self) -> Result<(), Error>; }
pub struct Pin;       // generate(&CodeAlphabet, &Random), parse(&CodeAlphabet, &str), as_str(); Debug hides it
pub struct UserCode;  // the same
pub struct PairingCode { pub slot: Option<u8>, pub pin: Pin }  // plain data

pub struct PairingDesk;        // Clone, shared by every connection and the app
impl PairingDesk {
    pub fn builder(random: Random) -> PairingDeskBuilder;   // options(PairingOptions), limits(DeskLimits), clock(Clock),
                                                            // settings(PairingDeskSettings), build() -> Result<_, Error> (InvalidOptions)
    pub fn new(random: Random) -> PairingDesk;               // builder(random).build() with the defaults
    pub fn settings(&self) -> PairingDeskSettings;  pub fn non_defaults(&self) -> Vec<NonDefault>;
    pub fn options(&self) -> PairingOptions;  pub fn limits(&self) -> DeskLimits;
    pub fn show_pin(&self) -> Result<ShownPin, Error>;    // replaces the shown one
    pub fn shown_pin(&self) -> Option<ShownPin>;  pub fn clear_pin(&self);
    pub fn slots(&self) -> Vec<PendingSlot>;
    pub fn submit_code(&self, &PairingCode) -> Result<(), PairingError>;
    pub fn find_device(&self, approver: &str, code: &str) -> Result<PendingDevice, PairingError>;
    pub fn approve(&self, &PendingDevice) -> Result<(), PairingError>;
    pub fn deny(&self, &PendingDevice) -> Result<(), PairingError>;
    pub fn sweep(&self);
    pub fn wait(&self, Ticket) -> Decision;      // blocking
    pub fn decided(&self, Ticket) -> Decided;    // Future<Output = Decision>
}
#[non_exhaustive] pub struct PairingOptions { pub invitation: bool, pub pin: bool, pub pin_shown: bool, pub device: bool,
    pub pin_alphabet: CodeAlphabet, pub pin_ttl: Duration, pub pin_attempts: u32,
    pub user_code_alphabet: CodeAlphabet, pub device_ttl: Duration }   // Default: all on, 10 min, 1, 10 min
#[non_exhaustive] pub struct DeskLimits { pub pending: usize, pub pending_per_source: usize, pub pin_runs_per_source: usize,
    pub wrong_codes_per_approver: usize, pub approver_window: Duration, pub wrong_codes_total: usize,
    pub ipv6_prefix: u8 }                       // Default: 32, 4, 8, 5, 60 s, 60, 64
#[non_exhaustive] pub struct PairingDeskSettings { pub options: PairingOptions, pub limits: DeskLimits }
impl PairingOptions { pub fn check(&self) -> Result<(), Error>; pub fn non_defaults(&self) -> Vec<NonDefault>; }
impl DeskLimits { pub fn non_defaults(&self) -> Vec<NonDefault>; }
// NonDefault is the crate root's: coqui_peer::NonDefault { setting, value, default }, NonDefault::new(setting, value, default);
// NonDefault::of_settings(&T) lists every key of a guatiao settings type that differs from its schema's default, named
// by key path (`waiting.limits.per_source`), and of_settings_against(&T, &T) against another value;
// NonDefault::from_difference(&guatiao::schema::Difference) renders one difference (text as text, else compact JSON, a
// missing side `unset`).
#[non_exhaustive] pub struct ShownPin { pub pin: Pin, pub expires: SystemTime }
#[non_exhaustive] pub struct PendingSlot { pub slot: u8, pub peer: PeerId, pub source: Option<SocketAddr>, pub expires: SystemTime }
#[non_exhaustive] pub struct PendingDevice { pub peer: PeerId, pub source: Option<SocketAddr>, pub label: String, pub expires: SystemTime }
#[non_exhaustive] pub enum Decision { Code(Pin), Approved, Denied, Expired }

pub struct Rotation;  // announce(&KeyMaterial) -> Vec<u8>; parse(&[u8]) -> Result<KeyMaterial, PairingError>
```

**Modes.** A responder turns each off in `PairingOptions`; a joiner of a disabled mode gets
`Disabled`.

- *Invitation.* The joiner holds `secret` and the inviter's id (from a link or QR code the
  caller formats). `join` refuses (`Unverified`) unless `channel.remote` is the inviter's key,
  because the secret is sent as is: the channel must be confidential and authenticate the
  inviter (TLS pinning it). The responder `redeem`s it; unknown or used up is `WrongCode`,
  expired `Expired`.
- *PIN* (feature `pin`). `Typed`: the responder `show_pin`s, a person types it at the joiner.
  `Shown`: the joiner shows its own PIN with the slot (0 to 99) the responder assigns (from
  `shown()`); the operator's app passes both to `submit_code`. Both sides run SPAKE2 (Ed25519,
  identities = both `PeerId`s' canonical text), derive one HMAC-SHA256 key per direction by
  HKDF-SHA256, and confirm a transcript of both keys (method, label, bytes), the slot, both
  PAKE messages and the binding; the responder confirms first, tags are compared in constant
  time. A run that reached the first PAKE message uses one of `pin_attempts` (default 1:
  burned whatever the outcome; then `Exhausted`). Retrying needs a fresh PIN.
- *Device flow.* The joiner sends a `label`; the responder issues a `UserCode`, which the
  joiner returns in `shown()` and keeps the connection open. The operator enters it; the app
  calls `find_device(approver, code)`, gets the joiner's `PeerId`, source and label (the app may
  show them before deciding; the label proves nothing), then `approve` or `deny`. A label or
  code with a control or bidirectional formatting character is refused when sent and when
  received (`Protocol`), so what an app shows reads as what was sent. The joiner
  learns `Paired`, `Denied` or `Expired`. `join` refuses (`Unverified`) unless
  `channel.verified` (and `remote` is set), or `allow_unverified(true)`. `verified` means the
  channel checked that `remote` is the peer the joiner meant (a pin, or roots for the name
  dialled); a proven key alone, as `Trust::AnyWithKeyProof` gives, is not that. coqui-peer-tls
  sets it from the joiner's trust. **Allowing an unverified responder lets a relaying attacker
  pair in place of both sides**: the operator approves the attacker's code and the joiner trusts
  the attacker. Use PIN mode when the responder cannot be verified.

On success the responder has `accept_key`ed the joiner; the joiner's caller stores
`Paired::peer`.

**Security model** (what an attacker on the network can and cannot do, with default options):

| Mode | Cannot | Can |
| --- | --- | --- |
| Invitation | learn the secret (sent only over a channel that proved the inviter) | pair, if it holds the secret before it expires or runs out of uses |
| PIN | learn the PIN by watching or relaying; swap a key or relay between two bound channels without failing the confirmation | guess once per run: success `pin_attempts / combinations` per shown PIN (1 in 10^6), and the PIN is burned |
| Device flow (verified responder) | see the user code; approve itself | guess codes at the approver's entry, up to the wrong-entry throttles per window |
| Device flow (`allow_unverified`) | — | relay between joiner and responder and pair in place of both |
| All | exceed `pending` exchanges or `pin_runs_per_source` per window | hold up to `pending` exchanges open until their TTL |

A channel without `remote` proves no key: the PIN confirmation still binds both keys, but
neither side proved it holds its private key. Without a binding, a relay between two channels
is caught only through the keys it cannot replace.

**Channels.** `verified` is the caller's statement that `remote` is the peer it meant (see the
device flow); only the device flow reads it. `remote: Some` means the channel proved the other
side holds that key (a TLS handshake does); every key the exchange carries must then equal it
(`Protocol` otherwise).
`None` takes the key from the exchange: the PIN confirmation still covers both keys, so a
man in the middle cannot swap them without the PIN, but nothing proves either side holds its
private key. `binding: Some` (coqui-peer-tls passes the RFC 8446 / RFC 5705 exporter, label
`EXPORTER-coqui-pair`, 32 bytes, empty context) ties a PIN run to this channel: a relay between
two channels fails with `WrongCode` on both sides. Without a binding, a relay that forwards
messages unchanged between two proven channels still fails when the keys differ. A TLS 1.3
exporter is unique to the connection; a TLS 1.2 one only with the extended master secret (RFC
7627), which rustls negotiates when the peer offers it and insists on only with `require_ems`.

**Matching and codes.** Codes are matched exactly over their alphabet: no case folding, no
dashes or spaces; anything else is `Error::InvalidCode`. Normalising typed input and grouping
for display are the caller's. `find_device` compares against every pending code in constant
time.

**Options cost what they change.** Nothing is refused for being non-default; `non_defaults()` lists every
option that differs from its default, and these are the costs of the looser ones:

- fewer `combinations()` of `pin_alphabet`: an attacker's one guess per run succeeds with
  probability `pin_attempts / combinations` per shown PIN; 10^4 codes is 1 in 10 000;
- `pin_attempts` above 1: that many guesses per shown PIN;
- longer `pin_ttl` / `device_ttl`: a code stays guessable or approvable longer; `pin_ttl` is
  also the window of `pin_runs_per_source`;
- fewer `combinations()` of `user_code_alphabet`: guessing a pending code by its entry, up to
  the wrong-entry throttles below per window;
- larger `pending`, `pin_runs_per_source`, `wrong_codes_*`, shorter `approver_window`: more
  attempts, or more held connections, per window; `ipv6_prefix` above 64 lets one subnet's
  holder count as many sources.

Only meaningless values are errors (`InvalidOptions`): an empty alphabet, a repeated or
control character, length 0, zero TTL or attempts. A TTL of `Duration::MAX` means never; times
saturate instead of overflowing.

**Throttles** (`DeskLimits`). At most `pending` exchanges at once, and `pending_per_source`
from one source, in every mode (`Throttled`); at most
`pin_runs_per_source` PIN runs per source (IPv4 address, or IPv6 prefix of `ipv6_prefix` bits)
per `pin_ttl`; at most `wrong_codes_per_approver` wrong user codes per `approver` (any text the
app uses: an account, a session) per `approver_window`, and `wrong_codes_total` from all per
`device_ttl`. A throttled `find_device` is refused before matching. A code not of the alphabet
is not counted. A desk with fewer free codes than pending device joiners refuses
(`Throttled`).

**Deadlines.** From the joiner's first message a responder has a `deadline()` on the desk's
clock: `pin_ttl` for a PIN, `device_ttl` for the device flow (an invitation ends with that
message). `receive` and `resume` refuse past it, and `check_deadline()` ends a stalled exchange
(`Expired` to the joiner, its slot or code released). A transport reads with a timeout up to the
deadline and calls `check_deadline` when it fires (`time_left()` is that timeout); before the
first message the transport's own timeout applies (coqui-peer-tls: `PairConfig::timeout`).

**Waiting.** A responder that waits for the operator returns its `Ticket` from `awaiting()`;
drive it with `desk.wait(ticket)` (rechecks expiry every second on the system clock) or
`desk.decided(ticket).await` (woken by a decision or by `sweep`; an async driver sweeps now and
then), then `resume(decision)`. Expiry follows the desk's `Clock`; under an injected clock
call `sweep` after moving it. Dropping a responder releases its pending slot or code. A slot or
code that was decided (or expired) is listed and decided no more, also after its responder took
the decision.

**Rotation message.** `Rotation::announce(&successor)` is the message a peer sends over a
session its current key authenticated; the receiver `parse`s it and calls
`TrustStore::announce_successor(sender, &key)`.

**Vectors.** `tests/vectors/pair/{invitation,device,pin-typed,pin-shown-plain}.txt`: every
message of one exchange (`joiner <hex>` / `responder <hex>` lines), from keys of 32 repeated
bytes (1 for the joiner, 2 for the responder, method `example`, label `PUBLIC KEY`), the clock at
unix 1800000000, and counter random sources; checked by `tests/pair.rs`.

## On-disk store format (version 1)

For peers in other languages. UTF-8, LF. Under `<state_dir>/peers/`: `accepted/`, `waiting/`
and `successors/` hold `<64 lowercase hex>.pem`, the name being the SHA-256 id of the key bytes
(no `sha256:` prefix: `:` is not valid in Windows file names); `invitations/` holds
`<64 lowercase hex>.inv`, named by the SHA-256 of the secret. A revoked invitation leaves
`<64 lowercase hex>.revoked` (empty), which refuses redemptions still in flight. A key file
is header lines, then one PEM block:

```text
Source: 192.0.2.7:4000        waiting files only, optional; IPv6 as [addr]:port
Predecessor: <64 hex>         successors/ only: the accepted key that announced it
Proven: 1800000000            successors/ only, optional: unix seconds of the first proof
Method: tls                   the method name
Successor: <64 hex>           accepted only, optional: announced (pending) or promoted successor
Retires: 1800086400           accepted only, optional: unix seconds; refused from then on
-----BEGIN CERTIFICATE-----   the label the method chose
...base64 of the key bytes, 64-character lines...
-----END CERTIFICATE-----
```

A successor is pending while `successors/<new>.pem` names the accepted key and that key's
`Successor:` names it back without a `Retires:` line. An invitation file is header lines only,
never the secret:

```text
Expires: 1800086400           unix seconds
Uses: 2                       redemptions left, at least 1
Label: kitchen tablet         optional, one line
```

`Method:` is required, and a file named other than by 64 lowercase hex is not read.
Accepting rewrites a waiting file without `Source:`. Vectors: `tests/vectors/store`,
`*.fingerprint` (canonical, bare hex, grouped base32), `example-key.id`, checked by
`tests/vectors.rs`; the store tree is written at unix time 1800000000 with the invitation
secret `0xa0..=0xbf`.

## Errors

`Error` (non-exhaustive): `InvalidId`, `WeakDigest`, `UnknownDigest(String)`, `Material`,
`NotWaiting`, `AlreadyAccepted`, `NotAccepted`, `NoSuccessor`, `SuccessorTaken`, `NotProven`,
`NotInvited`, `Expired`, `InvalidOptions`, `InvalidCode`, `Random`, `Io(io::Error)`.
`Display` is one lowercase clause and never includes key material, secrets, paths or the input
it refused: `UnknownDigest` carries the name, and its `Display` does not quote it.

`DriveError<E>` (non-exhaustive): `Io(io::Error)` (the transport failed or closed),
`Handshake(E)` (the method refused or failed), `TooLong` (a frame above `MAX_MESSAGE`),
`TimedOut` (an `_until` loop passed its deadline). `read_frame` / `write_frame` return
`io::Error`: `InvalidInput` (writing more than `MAX_MESSAGE`), `InvalidData` (a declared length
above it), `UnexpectedEof` (the stream ended inside a frame).

`pair::PairingError` (non-exhaustive): `Refused(PairRefusal)`, `Protocol`, `Unverified`,
`Local(Error)`; see Pairing.

## Gotchas

- Compare ids with `==` (constant time), never their text: one id has several spellings.
- `record_waiting` is only for a peer that proved its key in the handshake (the key-proof
  contract); calling it for a presented key queues identities anyone can copy.
- The `_until` drivers need `ReadTimeout` and `WriteTimeout` to bound a read or a write; a no-op
  implementation is bounded only between them. The async ones need a tokio runtime with its timer enabled.
- `pair::Decision`, `Error`, `DriveError`, `pair::Mode`, `pair::Shown`, `pair::PairRefusal`,
  `pair::PairingError`, `Digest`, `WhenFull` and `RotationEvent` are `#[non_exhaustive]`: a `match` needs a
  `_` arm.
- On Windows a rename, link or removal that another handle blocks is retried for about a second
  before the store or `file` reports it.
