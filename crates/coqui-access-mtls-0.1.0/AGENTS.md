# `coqui-access-mtls` — API header

Header-file-style reference for the `coqui-access-mtls` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

Client certificates as accounts: pinned by fingerprint in a table (the peers a service
pairs with), signed by a CA, or either. MSRV Rust 1.89. No Cargo features. Depends on
`coqui-access`, `coqui-access-kit`, `coqui-builder` (`TrustMode`), `coqui-peer` (the waiting queue) and `coqui-peer-tls`
(feature `ca`: fingerprints, chain checks, certificate fields). The record types derive
guatiao's `ToValue`, `FromValue` and `Schema`, so any table keeps them. No `unsafe` code. It
reads no environment variable, and no file but a configured `ca_file`. The loadable library
is the separate crate `coqui-access-mtls-plugin` (id `coqui_access_mtls`).

```rust
pub const DEFAULT_MAX_RECORDS: usize = 10_000;
pub const DEFAULT_MAX_CHAIN: Limit = Limit::at(8);
pub enum TrustMode { Pinned /* default */, Ca, PinnedOrCa }    // coqui_builder::Leaf: "pinned" | "ca" | "pinned-or-ca"
pub fn default_account() -> Vec<Pick>;                        // [fingerprint by certificate]
pub fn default_waiting_limits() -> coqui_peer::WaitingLimits;   // per_source 1, total 16, rerecord 60 s
pub const DEFAULT_WAITING_PER_SOURCE: usize = 1;  pub const DEFAULT_WAITING_TOTAL: usize = 16;  pub const DEFAULT_WAITING_RERECORD: Duration = 60 s;
pub struct PeerRecord { pub id, pub name, pub certificate: PeerId, pub email: Option<String>,
    pub groups: Vec<String>, pub disabled: bool, pub made_unix: Option<i64> }
                                          // non_exhaustive; Keyed by name; Debug redacts
impl PeerRecord { pub fn new(id: impl Into<String>, name: impl Into<String>, certificate: &PeerId) -> Self;
                  pub fn groups(self, ..) -> Self; pub fn email(self, ..) -> Self; pub fn disabled(self, bool) -> Self; }
pub struct Unaccepted { pub fingerprint: PeerId, pub peer: Option<String>, pub seen_unix: i64 }
                                          // non_exhaustive; Keyed by fingerprint
impl Unaccepted { pub fn new(fingerprint: &PeerId, peer: Option<String>, seen_unix: i64) -> Self; }
pub struct MtlsAccountsBuilder;                  // Debug
impl MtlsAccountsBuilder {
    pub fn table(self, table: Arc<dyn Table<PeerRecord>>) -> Self;     // default: MemoryTable
    pub fn waiting(self, table: Arc<dyn Table<Unaccepted>>) -> Self;   // default: MemoryTable
    pub fn max_records(self, max: usize) -> Self;                        // default 10 000
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

- `build` checks the peer table: at most `max_records` records, unique names and ids, certificates
  that are distinct SHA-256 fingerprints.
- `build` refuses `Ca` or `PinnedOrCa` without `ca`.
- `authenticate`: more than `max_chain` certificates is `Unknown`. The fingerprint is
  `Attempt::certificate` in any accepted spelling, else the SHA-256 of the first of
  `Attempt::certificates`; anything but SHA-256 is no fingerprint.
  - Pinned (`Pinned`, and first under `PinnedOrCa`): a known, enabled record gives
    `Lookup { by: "id", value: id }`; a known, disabled one `Invalid`.
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
- Records hold fingerprints as `PeerId` (`#[map(as_text)]`): written canonical
  (`sha256:<hex>`), read in any spelling coqui-peer-tls reads; text that is not one fails the
  read. `Unaccepted` is keyed by the canonical text.
- Waiting list: coqui-peer's `WaitingQueue` decides, under `waiting_limits`. A source is an
  IPv4 address (IPv4-mapped included) or an IPv6 /64, and an unknown address is one shared
  source; a source holds `per_source` entries (past it the newcomer is not written down), the list holds `total`
  (past it the newcomer is not written down, nobody is pushed out), and one certificate from
  one source is written down again only after `rerecord`. A fingerprint already waiting stays
  as it is. A provider built over a waiting table counts what is there. `Accounts::waiting` lists them newest first with
  `shown` in canonical `sha256:<hex>` form. A waiting table that cannot be read or written
  means nobody waits; the peer is never told.
- `put`: `secret` is the certificate fingerprint (SHA-256, any spelling) and is required for a
  new peer; giving it for an existing name moves the account to that certificate; a
  fingerprint already used by another peer is refused. `groups` and `disabled` change those
  fields; `expires_unix` is refused. Putting a fingerprint removes it from the waiting list.
  A read-only peer table answers `GUATIAO_ERR_UNSUPPORTED`.

## Gotchas

- The CA check reads `ca_file` on every CA-trusted attempt.
- The default `account` answers `Lookup { by: "certificate", value: "sha256:<hex>" }`, which
  this provider's own directory resolves only for pinned records: with `Ca`, configure picks
  for the directory the host asks.

## Errors

A table that cannot be read at call time is `GUATIAO_ERR_INTERNAL`; a refused change is
`GUATIAO_ERR_BAD_VALUE`.
