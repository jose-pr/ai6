# `coqui-access-mtls` — API header

Header-file-style reference for the `coqui-access-mtls` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

Client certificates as accounts: pinned in a table by fingerprint or by public key (the peers a
service pairs with), signed by a CA, or either. MSRV Rust 1.89. No Cargo features. Depends on
`coqui-access`, `coqui-access-kit`, `coqui-builder` (`TrustMode`), `coqui-peer` (the waiting queue), `coqui-peer-tls`
(feature `ca`: fingerprints, chain checks, certificate fields), `x509-parser` (a certificate's public key), `sha2`
and `data-encoding`. The record types derive
guatiao's `ToValue`, `FromValue` and `Schema`, so any table keeps them. No `unsafe` code. It
reads no environment variable, and no file but a configured `ca_file`. The loadable library
is the separate crate `coqui-access-mtls-plugin` (id `coqui_access_mtls`).

```rust
pub const DEFAULT_MAX_RECORDS: usize = 10_000;
pub const DEFAULT_MAX_PINS: usize = 4;      // fingerprints and keys together, per account
pub const DEFAULT_MAX_CHAIN: Limit = Limit::at(8);
pub enum TrustMode { Pinned /* default */, Ca, PinnedOrCa }    // coqui_builder::Leaf: "pinned" | "ca" | "pinned-or-ca"
pub fn default_account() -> Vec<Pick>;                        // empty: a CA-trusted certificate is answered by its claims
pub fn default_waiting_limits() -> coqui_peer::WaitingLimits;   // per_source 1, total 16, rerecord 60 s
pub const DEFAULT_WAITING_PER_SOURCE: usize = 1;  pub const DEFAULT_WAITING_TOTAL: usize = 16;  pub const DEFAULT_WAITING_RERECORD: Duration = 60 s;
pub struct PeerRecord { pub id, pub name, pub certificate: Option<PeerId>, pub pins: Vec<Pin>,
    pub email: Option<String>, pub groups: Vec<String>, pub disabled: bool, pub made_unix: Option<i64> }
                                          // non_exhaustive; Keyed by name; Debug redacts
impl PeerRecord { pub fn new(id: impl Into<String>, name: impl Into<String>, certificate: &PeerId) -> Self;
                  pub fn keyed(id: impl Into<String>, name: impl Into<String>, pin: Pin) -> Self;
                  pub fn groups(self, ..) -> Self; pub fn email(self, ..) -> Self; pub fn disabled(self, bool) -> Self; }
pub struct Pin { pub kind: String, pub value: String }     // non_exhaustive; Clone, Eq, Hash; Debug hides the value
impl Pin {
    pub const FINGERPRINT: &'static str = "fingerprint"; pub const KEY: &'static str = "key";
    pub fn fingerprint(&PeerId) -> Self;                   // the whole certificate
    pub fn key_of_certificate(der: &[u8]) -> Option<Self>; // its SubjectPublicKeyInfo; None when it does not parse
    pub fn key_of_public_key_info(spki_der: &[u8]) -> Self;
    pub fn parse(text: &str) -> Result<Self, PinError>;    // "sha256:<hex>" or "key:sha256:<hex>"
    pub fn text(&self) -> String;                          // what parse reads
}
pub struct PinError;                                     // non_exhaustive, Error; never quotes the text
pub struct Unaccepted { pub fingerprint: PeerId, pub peer: Option<String>, pub seen_unix: i64 }
                                          // non_exhaustive; Keyed by fingerprint
impl Unaccepted { pub fn new(fingerprint: &PeerId, peer: Option<String>, seen_unix: i64) -> Self; }
pub struct MtlsAccountsBuilder;                  // Debug
impl MtlsAccountsBuilder {
    pub fn table(self, table: Arc<dyn Table<PeerRecord>>) -> Self;     // default: MemoryTable
    pub fn waiting(self, table: Arc<dyn Table<Unaccepted>>) -> Self;   // default: MemoryTable
    pub fn max_records(self, max: usize) -> Self;                        // default 10 000
    pub fn max_pins(self, max: usize) -> Self;                           // default DEFAULT_MAX_PINS
    pub fn record_limits(self, limits: RecordLimits) -> Self;         // default RecordLimits::DEFAULT
    pub fn waiting_limits(self, limits: WaitingLimits) -> Self;        // default default_waiting_limits()
    pub fn trust(self, trust: TrustMode) -> Self;                      // default Pinned
    pub fn ca(self, ca: coqui_peer_tls::CaPolicy) -> Self;             // needed by Ca and PinnedOrCa
    pub fn account(self, picks: Vec<Pick>) -> Self;                    // default default_account()
    pub fn max_chain(self, most: Limit) -> Self;                       // default DEFAULT_MAX_CHAIN
    pub fn policy(self, policy: coqui_peer_tls::Policy) -> Self;       // default Policy::default()
    pub fn build(self) -> Result<MtlsAccounts, SourceError>;
}
pub struct MtlsAccounts;
impl MtlsAccounts { pub fn builder() -> MtlsAccountsBuilder; }
impl Authenticator for MtlsAccounts { /* takes::REQUEST */ }
impl Directory for MtlsAccounts { /* by "id", "name", "certificate", "email" */ }
impl Accounts for MtlsAccounts { /* accounts, put, remove, waiting */ }
```

## Behaviour

- `build` checks the peer table: at most `max_records` records, unique names and ids, each record
  holding between one and `max_pins` pins, every pin valid and held by one record once.
- `build` refuses `Ca` or `PinnedOrCa` without `ca`.
- `authenticate`: more than `max_chain` certificates is `Unknown`. The fingerprint is
  `Attempt::certificate` in any accepted spelling, else the SHA-256 of the first of
  `Attempt::certificates`; anything but SHA-256 is no fingerprint. The key pin is the SHA-256 of
  the SubjectPublicKeyInfo of the first of `Attempt::certificates`, and exists only when
  `secure == Some(true)` and `Attempt::certificate`, if given, is that certificate's fingerprint.
  - Pinned (`Pinned`, and first under `PinnedOrCa`): a record holding the fingerprint, else one
    holding the key pin, gives `Lookup { by: "id", value: id }` when enabled and `Invalid` when
    disabled. A fingerprint pin outranks a key pin if two accounts hold them.
  - CA (`Ca`, and then under `PinnedOrCa`): only for `secure == Some(true)` with
    `certificates`. The chain is checked with coqui-peer-tls's `Trust::verify_client_chain`
    under `policy` (CA, subject and SAN globs, dates; no key proof, which the transport
    made); a refused chain is not trusted. The leaf's `CertificateFields::to_map()` goes
    through `coqui_access_kit::pick::pick(.., account)`: `Found { by, value }` is
    `Valid(Lookup { by, value })`, `Ambiguous` is `Invalid`, `Absent` is `Unknown`. A CA
    file that cannot be read is `GUATIAO_ERR_INTERNAL`, never a denial.
  - Otherwise `Unknown`; under a pinning trust the fingerprint is recorded as waiting with
    `Attempt::peer`.
- An IP address a certificate carries as a DNS name counts as an IP name, both for an `ip:`
  SAN glob (`CaPolicy::ip_in_dns`) and for a `san.ip` pick (`Pick::ip_in_dns`), each on by
  default and compared in one spelling (IPv6 compressed, IPv4-mapped as IPv4).
- Records hold pins two ways. `certificate` (optional) is one fingerprint as `PeerId`
  (`#[map(as_text)]`): written canonical (`sha256:<hex>`), read in any spelling coqui-peer-tls
  reads, and the only form files written before pins existed use. `pins` (skipped when empty) lists
  more, each naming its kind: `{ kind = "fingerprint", value = "sha256:<hex>" }` or
  `{ kind = "key", value = "sha256:<hex of the SubjectPublicKeyInfo>" }`. Both together are what
  the account is known by. A pin that is not valid fails the read. `Unaccepted` is keyed by the
  canonical text.
- Waiting list: coqui-peer's `WaitingQueue` decides, under `waiting_limits`. A source is an
  IPv4 address (IPv4-mapped included) or an IPv6 /64, and an unknown address is one shared
  source; a source holds `per_source` entries (past it the newcomer is not written down), the list holds `total`
  (past it the newcomer is not written down, nobody is pushed out), and one certificate from
  one source is written down again only after `rerecord`. A fingerprint already waiting stays
  as it is. The decision is made inside the waiting table's `update` (a file table is locked for it), from a queue built
  from the entries it holds at that moment, so entries another provider or process wrote count against their
  source and the total, and two racing providers cannot both take the last place. `Accounts::waiting` lists them newest first with
  `shown` in canonical `sha256:<hex>` form. A waiting table that cannot be read or written
  means nobody waits; the peer is never told.
- `put`: `secret` is a pin and is required for a new peer: a SHA-256 certificate fingerprint (any
  spelling) or `key:sha256:<hex>` for a public key. For an existing name it is one more pin (up
  to `max_pins`; saying one the peer holds changes nothing). `remove:` before a pin takes that
  pin away alone; the last pin is never removed (remove the peer instead). A pin already held
  by another peer is refused. `groups` and `disabled` change those fields; `expires_unix` is
  refused. Putting a fingerprint removes it from the waiting list. A read-only peer table answers
  `GUATIAO_ERR_UNSUPPORTED`; a refused pin change is `GUATIAO_ERR_BAD_VALUE`.

## Gotchas

- The CA check reads `ca_file` on every CA-trusted attempt.
- A key pin follows the key through renewals, so it also accepts a certificate someone else
  issued on that key; pin a key only where the key, not the certificate, is what is trusted (a
  smartcard's).
- The directory's `certificate` lookup resolves fingerprint pins only.
- A pick whose `by` is `filter` or `claims` is refused by `build` (`SourceError::Invalid`): a pick names a
  directory attribute; only a matcher rule makes a filter. The claims are built by
  `coqui-access-claims`, which `coqui-access-proxy` uses too.
- `account` defaults to no picks (`default_account()` is empty). A CA-trusted certificate is then
  answered `Valid(Lookup { by: "claims", value: "sha256:<fingerprint>", claims })`, for the consumer's
  matcher (`coqui_access_kit::matcher`) to tie to a directory account; this provider's own directory
  does not resolve it. With picks the answer is as picked. Every `Valid` for a certificate carries
  `claims` when the chain was presented (a fingerprint alone has none): `subject.dn`, `subject.cn` and
  the other subject attributes, `issuer.dn` and its attributes, `serial.hex`, `serial.reversed`,
  `san.upn`, `san.email`, `san.dns`, `san.uri`, `san.ip`, `fingerprint`, `spki`, `not_before`,
  `not_after`, and the leaf as DER in `Claims::certificate`. The directory, not the certificate, decides
  the account and its groups.
- `MtlsAccounts` is a `coqui_access_kit::pins::PinSource`: `check(person, pins)` holds when an enabled
  record named `person.name` holds the fingerprint or the key in `pins` (the global table of pins).

## Errors

A table that cannot be read at call time is `GUATIAO_ERR_INTERNAL`; a refused change is
`GUATIAO_ERR_BAD_VALUE`.
