# `coqui-peer-webcrypto` — API header

Header-file-style reference for the `coqui-peer-webcrypto` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It ships inside
the package and is self-contained. Development documentation lives with the source at
<https://github.com/Boriken-Dev/coqui-peer-webcrypto>.

| Feature | Adds | Needed for | Default |
| --- | --- | --- | --- |
| `pin` | `coqui-peer/pin` (SPAKE2, HKDF, HMAC) | PIN pairing (`pair::Pairing::pin`) | off |

MSRV Rust 1.89. A default build depends on `coqui-peer` (by git revision, `984db3d`), `coqui-builder` (by revision), `p256`,
`aes-gcm`, `hkdf`, `sha2`, `data-encoding`, `serde`, `serde_json`, `getrandom` and `zeroize`. This crate
is unreleased, with no independent security review: depend on a git revision, and name the same
`coqui-peer` revision if you use it directly; the wire and API may change at any commit. This is the method named `webcrypto` of `coqui-peer` (`PeerId`, `TrustStore`, the sans-IO
`Handshake`, `pair`); its API header ships in that crate. `PROTOCOL.md` ships here and is the wire.

Imports: everything is at the crate root, including re-exports of `coqui-peer`'s `Established`,
`KeyMaterial`, `MessageTransport`, `Method`, `PeerId`, `Random`, `TrustStore` and its module `pair`, and
of `coqui-builder`'s `NonDefault`: its `value` and `default` print as `Type(text)` (`usize(4096)`,
`Random(caller's)`). `prelude` holds the settings' setter traits.

No `unsafe` code (`#![forbid(unsafe_code)]`), no process-wide state. Nothing reads a file or an
environment variable unless the caller names a path. Ephemeral scalars, traffic keys, the export secret,
long-term keys and PEM text are held on the heap and zeroed when dropped; copies a move leaves on the
stack, and the stack state of `p256`, `hkdf`, `sha2` and `aes-gcm`, are not.

## Identity (`coqui_peer_webcrypto::Identity`)

```rust
pub struct Identity;                       // Clone (one shared copy); Debug shows only the id
impl Identity {
    pub fn generate() -> Result<Identity, Error>;                 // the OS generator
    pub fn generate_with(random: &Random) -> Result<Identity, Error>;
    pub fn from_pkcs8_der(der: &[u8]) -> Result<Identity, Error>;
    pub fn from_pkcs8_pem(text: &str) -> Result<Identity, Error>; // -----BEGIN PRIVATE KEY-----
    pub fn to_pkcs8_pem(&self) -> Result<Zeroizing<String>, Error>;
    pub fn read(path: &Path) -> Result<Option<Identity>, Error>;  // None when absent
    pub fn write_new(&self, path: &Path) -> Result<(), Error>;    // never replaces; 0600 on Unix; creates parents
    pub fn open_or_create(path: &Path) -> Result<Identity, Error>;
    pub fn peer_id(&self) -> PeerId;                              // SHA-256 of spki_der
    pub fn spki_der(&self) -> &[u8; 91];
    pub fn key_material(&self) -> KeyMaterial;                    // method "webcrypto", label "PUBLIC KEY"
}
```

On Windows a written file inherits its directory's ACL. Errors: `Error::Key` for what is not a P-256
PKCS#8 key (SEC1 `EC PRIVATE KEY` is not), `Error::Io` for the file, `Error::Random`.

## Trust (`coqui_peer_webcrypto::Trust`)

```rust
#[non_exhaustive]
pub enum Trust { Pinned(BTreeSet<PeerId>), Store(TrustStore) }
impl Trust { pub fn pinned(ids: impl IntoIterator<Item = PeerId>) -> Trust; }
```

Decided after the peer proved its key, never before.

- `Pinned`: fixed at build; an empty set admits nobody; a pin of a digest other than SHA-256 matches
  nothing.
- `Store`: read at each handshake, so a key removed or retired is refused from the next handshake on.
  The stored key must be of this method. A responder records a proved stranger with
  `record_waiting`, under the `source` given to `respond`; an initiator
  records nobody. The proved pending successor of an accepted key gets `prove_successor`, then
  `promote_successor` unless `RotationOptions::require_approval`, in which case it is refused until the
  caller promotes it.
- There is no way to admit anyone here: a stranger's session is a `PairingSession`.

How strangers wait is the store's: a source at `WaitingLimits::per_source` refuses a newcomer and keeps
those waiting, and records one new peer per `TrustStoreBuilder::record_interval`. A flood can fill a source;
it cannot remove anyone. Behind a reverse proxy every peer shares the proxy's source, unless the caller
passes the client's address from a header it trusts.

## The method (`coqui_peer_webcrypto::{WebCrypto, WebCryptoHandshake, METHOD, random}`)

```rust
pub const METHOD: &str = "webcrypto";
pub const PROFILE: &str = "dev.boriken.coqui.peer-webcrypto/v1";
pub const DEFAULT_MAX_MESSAGE: usize = 3072;
pub fn random() -> Random;                    // the OS generator
pub struct WebCrypto;                         // Clone (shared); Debug shows no secret
impl WebCrypto {
    pub fn builder() -> WebCryptoBuilder;     // takes no arguments; the identity is an input; neither role until responders or initiators
    pub fn identity(&self) -> &Identity;
    pub fn settings(&self) -> WebCryptoSettings;
    pub fn non_defaults(&self) -> Vec<NonDefault>;  // every WebCryptoSettings key that differs, by key path (`max_message`, `session_limit`), and `random` when replaced
}
// Derived by coqui-builder; not Clone. Each write replaces the last, whichever way it was made:
pub struct WebCryptoBuilder;                  // Debug
impl WebCryptoBuilder {
    pub fn identity(self, identity: impl Into<Identity>) -> Self;  // required
    pub fn responders(self, trust: impl Into<Trust>) -> Self;     // whom initiate() admits
    pub fn initiators(self, trust: impl Into<Trust>) -> Self;     // whom respond() admits
    pub fn random(self, random: impl Into<Random>) -> Self;       // secure, or for test vectors only
    pub fn max_message(self, bytes: usize) -> Self;                // before parsing; no floor
    pub fn session_limit(self, bytes: u64) -> Self;                // per direction; DEFAULT_SESSION_LIMIT; no floor
    pub fn fill(self, settings: Partial<WebCryptoSettings>) -> Self;   // only the keys the partial sets
    pub fn fill_from(self, source: &mut impl Source) -> Result<Self, FillError>;
    pub fn build(self) -> Result<WebCrypto, Missing>;              // Missing names `identity`
    pub fn parts(self) -> Result<WebCryptoParts, Missing>;         // the collected parts, for a second terminal
}
pub struct WebCryptoParts;                    // Debug; what the builder collects (settings and inputs)
// Describe (coqui-builder): every key optional, unknown keys refused; read as `Partial<WebCryptoSettings>`.
// The setters are methods of WebCryptoBuilder; `prelude::WebCryptoSettingsSetters` is the trait for a builder in another crate.
#[non_exhaustive] pub struct WebCryptoSettings { pub max_message: usize, pub session_limit: u64 }   // Default: DEFAULT_MAX_MESSAGE, DEFAULT_SESSION_LIMIT
impl coqui_peer::Method for WebCrypto { /* name() = "webcrypto"; initiate(); respond(source) */ }
pub struct WebCryptoHandshake;
impl WebCryptoHandshake {
    pub fn bind(self, binding: &[u8]) -> Result<WebCryptoHandshake, Error>;   // Error::State after the first receive
}
pub struct FramedStream<S>;                   // Debug
impl<S> FramedStream<S> {
    pub const fn new(stream: S, max_message: usize) -> FramedStream<S>;
    pub const fn set_max_message(&mut self, max_message: usize);
    pub fn into_inner(self) -> S;
}
// MessageTransport for S: Read + Write; ReadTimeout and WriteTimeout as S has them
impl coqui_peer::Handshake for WebCryptoHandshake { /* Output = Session, Framing::Message */ }
```

Run it with `coqui_peer::drive_messages` over a reliable, ordered message transport, `drive_messages`
over a `FramedStream` for a byte stream (`coqui_peer::drive_stream` also works, and takes up to 1 MiB
before the message limit applies; `FramedStream` refuses a frame by its declared length, with an
`InvalidData` I/O error), their `_until` forms for a deadline (**the caller bounds the handshake**), or a
`receive`/`transmit` loop. `initiate` has the
hello ready to `transmit`; `Error::Unsupported` without trust for that role. `finish` gives
`Established { peer, key: Some(key), session }`.

`binding` is for an endpoint that really ends a TLS connection itself: pass that connection's
exporter at both ends, and a relay ending TLS on both sides fails with `Error::BadProof`. Never set it
through a reverse proxy, where the two ends read different values. It is never sent.

## The session (`coqui_peer_webcrypto::{Session, Sealer, Opener, DEFAULT_SESSION_LIMIT}`)

```rust
pub const DEFAULT_SESSION_LIMIT: u64 = 1 << 38;   // each record counts its length and 32
pub struct Session;                           // Debug shows nothing
impl Session {
    pub fn id(&self) -> &[u8; 32];            // both ends agree; not secret
    pub fn seal(&mut self, message: &[u8]) -> Result<Vec<u8>, Error>;
    pub fn close(&mut self) -> Result<Vec<u8>, Error>;        // the last record of this direction
    pub fn open(&mut self, record: &[u8]) -> Result<Option<Vec<u8>>, Error>;  // None: the peer's close
    pub fn verify_end(&self) -> Result<(), Error>;            // call when the transport ended
    pub fn split(self) -> (Sealer, Opener);   // a reader and a writer on two threads; export first
    pub fn export(&self, label: &str, out: &mut [u8]) -> Result<(), Error>;   // at most 8160 bytes
}
impl Sealer { pub fn seal(&mut self, message: &[u8]) -> Result<Vec<u8>, Error>;
              pub fn close(&mut self) -> Result<Vec<u8>, Error>; }
impl Opener { pub fn open(&mut self, record: &[u8]) -> Result<Option<Vec<u8>>, Error>;
              pub fn verify_end(&self) -> Result<(), Error>; }
```

Each message is one record, sent whole and in order on a reliable transport; a record is the message
plus 17 bytes, and is not UTF-8, so a WebSocket carries it in binary frames.

**End every stream with `close`, and call `verify_end` when the transport ends.** A relay can cut the
connection at any record boundary; `verify_end` is `Error::Truncated` unless the peer's close record
opened. Each direction closes on its own: after `close`, `seal` is `Error::Closed`, and receiving goes on.

Any failure to open ends the session in both directions, split or not: `Error::Record` on every later
`open`, `seal` and `close`, and the caller closes the connection. At `session_limit`, a longer record
is `Error::LimitReached` in `seal` or `open`, the session stays intact, a close still passes, and a new
handshake carries on: there is no rekeying.

`export` gives unrelated bytes for another label or another length. Keep every application message
inside the session: **a plain message on the same transport comes from whoever carries it**, even after
a successful handshake.

## Pairing (`coqui_peer_webcrypto::{PairingMethod, PairingSession, PairDial}`, `pair`)

```rust
pub const PAIR_EXPORT_LABEL: &str = "EXPORTER-coqui-pair";
pub struct PairingMethod;                     // Clone; Debug
impl PairingMethod {
    pub fn builder() -> PairingMethodBuilder;   // no arguments; admits any proved key, in both roles
    pub fn settings(&self) -> PairingSettings;
    pub fn non_defaults(&self) -> Vec<NonDefault>;    // `max_message` when it differs; `random` when replaced (admitting any proved key is what pairing is, not listed)
}
pub struct PairingMethodBuilder;              // Debug; not Clone; build() -> Result<PairingMethod, Missing>
impl PairingMethodBuilder {
    pub fn identity(self, identity: impl Into<Identity>) -> Self;   // required
    pub fn responders(self, trust: impl Into<Trust>) -> Self;       // a joiner verifies the responder
    pub fn random(self, random: impl Into<Random>) -> Self;
    pub fn max_message(self, bytes: usize) -> Self;
    pub fn fill(self, settings: Partial<PairingSettings>) -> Self;
    pub fn fill_from(self, source: &mut impl Source) -> Result<Self, FillError>;
}
pub struct PairingMethodParts;                // Debug
#[non_exhaustive] pub struct PairingSettings { pub max_message: usize }   // Describe; Default: DEFAULT_MAX_MESSAGE
impl coqui_peer::Method for PairingMethod { /* name() = "webcrypto"; initiate(); respond(source) */ }
pub struct PairingHandshake;  // impl Handshake { Output = PairingSession, Framing::Message }
pub struct PairingSession;    // no seal, no open: it runs one exchange
impl PairingSession {
    pub fn id(&self) -> &[u8; 32];
    pub fn peer(&self) -> &KeyMaterial;
    pub fn peer_id(&self) -> PeerId;
    pub fn pair_channel(&self) -> Result<pair::Channel, Error>;  // the proved key; no caller names it
    pub fn join<T: MessageTransport>(self, pairing: Pairing, transport: &mut T) -> Result<PairDial<'_, T>, Error>;
    pub fn respond<T: MessageTransport + ReadTimeout>(self, store: &TrustStore, desk: &PairingDesk,
        source: Option<SocketAddr>, transport: &mut T) -> Result<Paired, Error>;
}
impl<T: MessageTransport> PairDial<'_, T> {
    pub fn shown(&self) -> Option<&Shown>;    // a PIN with its slot, or a device flow's user code
    pub fn finish(self) -> Result<Paired, Error>;
}
```

**A stranger's session is this type, and only this type.** `WebCrypto` cannot admit an unknown key, and
`PairingSession` cannot carry an application message, so what serves the application cannot be reached by
proving just any key. `join` and `respond` seal every message, and end with a close record. `respond`
blocks in `PairingDesk::wait` for the operator; each read waits until the exchange's deadline through
`ReadTimeout`. On success the responder has `accept_key`ed the joiner and the joiner stores `Paired::peer`.
Then both reconnect under `WebCrypto`, and the full key is what the handshake proves.

- **Invitation**: `join` refuses (`Error::Pairing(PairingError::Unverified)`) unless the proved key is the
  inviter's, before anything is sent. Pin the inviter in `responders` too, to stop others at the
  handshake.
- **PIN** (feature `pin`): no pin needed: the PIN authenticates both keys and the session's binding.
- **Device flow**: `join` refuses unless `responders` verified the responder, or
  `Pairing::allow_unverified(true)`, which lets a relay pair in place of both sides.

## Errors

`Error` (non-exhaustive): `Malformed` (not valid here: encoding, member, key, or place in the
exchange), `TooLong` (above `max_message`, or an export above 8160 bytes), `BadProof` (a signature
does not verify), `Untrusted` (proved, and `Trust` does not admit it), `Unsupported` (no trust for that
role), `State` (the handshake is over, `finish` too early, a binding too late), `Random`, `Record`,
`Truncated`, `Closed`, `LimitReached` (see the session), `Key`, `Pairing(PairingError)`, `Io(io::Error)`. `Display` is one lowercase clause and never quotes input, a key or a path. A
handshake is over after an error: nothing queued leaves, and `transmit` is `State`.

## Gotchas

- The handshake alone authenticates nothing that follows it. Use the session, or a relay that forwarded
  the handshake owns the connection.
- A transport ending says nothing by itself: `verify_end` does. A peer that never calls `close` looks cut.
- Signatures are transferable evidence that a key took part in a run, and a `hello` names its sender's key
  to whoever receives it; see `PROTOCOL.md`.
- Each `hello` costs a responder a signature for free: bound connections and time in the host.
- Removing a peer does not stop its running sessions: the host closes them, polling
  `TrustStore::is_accepted` if it must.
- `Error` and `Trust` are `#[non_exhaustive]`: a `match` needs a `_` arm.
