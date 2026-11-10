# `coqui-peer-tls` — API header

Header-file-style reference for the `coqui-peer-tls` crate: every public item with its signature,
contract and gotchas, so the crate can be used without reading its source. It ships inside the
package and is self-contained. Development documentation lives with the source at
<https://github.com/Boriken-Dev/coqui-peer-tls>.

| Feature | Adds | Needed for | Default |
| --- | --- | --- | --- |
| `generate` | `rcgen`, `time` | `Identity::generate`, `Identity::builder`, `IdentityBuilder`, `open_or_create` | off |
| `fields` | `x509-parser` | `CertificateFields`, `Name`, `San` | off |
| `ca` | `fields`, `globset` | `Trust::Ca`, `Trust::PinnedOrCa`, `CaPolicy` | off |
| `tokio` | `tokio` (`net`, `io-util`, `time`), `tokio-rustls`, `coqui-peer/tokio` | module `asynch` | off |
| `pin` | `coqui-peer/pin` | PIN pairing (`Pairing::pin`) | off |
| `serde` | `serde`, `coqui-builder/serde`, `coqui-peer/serde` | `Policy` read through any serde format (`Policy::deserialize_over`), the settings types filled from one into a `coqui_builder::Partial`, `PeerId` as text | off |
| `acl` | `coqui-net-acl` | `Policy::acl` and its enforcement, `coqui_peer_tls::acl` | off |
| `platform-roots` | `rustls-platform-verifier` | `Roots::platform` | off |
| `aws-lc` | rustls `aws_lc_rs` (AWS-LC, built from C) | `Crypto::AwsLc`: ML-KEM groups, ML-DSA, `cnsa-2.0` | off |
| `fips` | `aws-lc`, rustls `fips` (AWS-LC's FIPS module; global, see Gotchas) | `CipherProfile::Fips1403Validated` | off |

MSRV Rust 1.89. A default build depends on `coqui-peer`, `coqui-builder` (`derive`: the settings
types are described by it in every build), `rustls` (0.23.27 or later, ring provider, `tls12`) and
`rustls-webpki` (which rustls already uses). Version `0.0.0-alpha.1` is
a pre-release; the API may change in any release before 1.0. This is the TLS method of `coqui-peer` (the core: `PeerId`, `TrustStore`, the sans-IO
`Handshake`); its API header ships in that crate.

Imports: everything is at the crate root, including re-exports of `coqui-peer`'s `Clock`,
`Digest`, `Invitation`, `InvitationOptions`, `KeyMaterial`, `PeerId`, `Random`,
`ReadTimeout`, `WriteTimeout`, `RotationEvent`, `RotationOptions`, `TrustStore`,
`WaitingEntry` and `WaitingLimits`, and `coqui_builder::NonDefault`. Public modules: `prelude` (the settings types' setter traits), `asynch` (feature `tokio`), `pair` (`coqui_peer::pair`
re-exported) and `acl` (`coqui_net_acl` re-exported, feature `acl`).

No `unsafe` code (`#![forbid(unsafe_code)]`). Process-wide state: one thread-local holding the
addresses of the handshake call in progress (`with_addresses`). Nothing installs
a process-wide crypto provider, and nothing reads a file or an environment variable unless the
caller passes a path. The default policy offers TLS 1.2 and 1.3 (two peers that both speak 1.3
negotiate it); `Policy::modern()` offers 1.3 only. TLS 1.2 is always compiled in (rustls's own
code, no dependency): a policy chooses whether it is offered.

## Ids (`coqui_peer_tls::{PeerId, Digest}`, from `coqui-peer`)

```rust
pub use coqui_peer::{Digest, PeerId, KeyMaterial, TrustStore, WaitingEntry, WaitingLimits};
// PeerId: Copy, Eq (constant time), Ord, Hash, Display = "sha256:<hex>", FromStr
```

The id of a certificate is `PeerId::of(der)` (SHA-256) or `Digest::Sha384.of(der)`. Text
forms, `canonical()`, `hex()`, `to_base32_grouped()` and the parsing rules (`sha256:<hex>`, bare
hex, colon pairs, grouped base32; MD5 is `PeerError::WeakDigest`) are the core crate's. Whether a
policy lets a pin use a digest is a `Policy` matter.

## Identity (`coqui_peer_tls::{Identity, PeerIdentity}`)

```rust
pub struct Identity;                       // Clone; Debug shows only the fingerprint
impl Identity {
    pub fn from_pem(text: &str) -> Result<Identity, PeerError>;       // exactly one certificate + its key
    pub fn from_pem_chain(text: &str) -> Result<Identity, PeerError>; // leaf, its intermediates in order, the leaf's key
    pub fn to_pem(&self) -> Result<String, PeerError>;                // certificate, intermediates, then PKCS#8 key
    pub fn read(path: &Path) -> Result<Option<Identity>, PeerError>;  // None when absent; a chain is read
    pub fn write_new(&self, path: &Path) -> Result<(), PeerError>;    // never replaces; 0600; creates parents
    pub fn fingerprint(&self) -> PeerId;                               // the leaf's
    pub fn certificate_der(&self) -> &[u8];                           // the leaf
    pub fn intermediates_der(&self) -> impl ExactSizeIterator<Item = &[u8]>;   // empty unless from_pem_chain
    pub fn peer_identity(&self) -> PeerIdentity;
    pub fn certificate_chain(&self) -> Vec<CertificateDer<'static>>;  // leaf, then intermediates
    pub fn private_key_der(&self) -> PrivateKeyDer<'static>;          // a copy; Debug elides it
    // feature generate: self-signed ECDSA P-256, CN "coqui-peer", DNS SAN SERVER_NAME,
    // subjectKeyIdentifier, basicConstraints CA:FALSE
    pub fn generate() -> Result<Identity, PeerError>;                 // rcgen's default validity (decades)
    pub fn builder() -> IdentityBuilder;                               // a validity window of your own
    pub fn open_or_create(path: &Path) -> Result<Identity, PeerError>;
}
#[must_use] pub struct IdentityBuilder;   // Clone, Debug, Default
impl IdentityBuilder {
    pub fn not_before(self, SystemTime) -> Self;     // default: rcgen's
    pub fn not_after(self, SystemTime) -> Self;      // the end of the validity, as valid_for: the last one called wins
    pub fn valid_for(self, Duration) -> Self;        // from not_before, from now when unset
    pub fn generate(self) -> Result<Identity, PeerError>;
}
pub struct PeerIdentity { pub fingerprint: PeerId, pub certificate: Vec<u8> }   // DER
```

- **Chains.** `from_pem_chain` takes a CA-issued leaf with its intermediates (a web-PKI server a
  browser reaches over `wss://`); every configuration built from the identity presents the
  whole chain. The key must be the first certificate's, or `PeerError::Certificate`. The
  fingerprint is the leaf's. A peer that pins it refuses the chain (`Refusal::ChainWhenPinned`)
  unless its policy sets `allow_chain_when_pinned`; `AnyWithKeyProof` takes it. `from_pem`
  refuses a chain; `read` and `open_or_create` read a file holding one (leaf first), so a
  chained identity written with `write_new` reads back as it was.
- **Short-lived identities.** `Identity::builder().valid_for(d)` is valid from now to now + `d`
  (from `not_before` when set); `.not_before(a).not_after(b)` names both ends. `valid_for` and
  `not_after` set one end between them: the last called wins. Times are truncated to whole seconds; a window under one
  second, an end before the start, or a time before 1970 is `PeerError::Certificate`. Any
  positive length is accepted. WebTransport's `serverCertificateHashes` accepts only a
  certificate whose whole validity period is at most 14 days: pass at most
  `Duration::from_secs(14 * 86_400)` and replace it before it expires; a peer whose clock runs
  behind sees it as not yet valid until its start.
- **Extensions.** Every generated certificate carries a subjectAltName (`SERVER_NAME`), a
  subjectKeyIdentifier and basicConstraints `CA:FALSE`. Chrome's `serverCertificateHashes`
  verifier refuses a certificate with no extensions. Pins name the certificate as made, so
  pinning is unaffected.

## Trust (`coqui_peer_tls::{Trust, Pins, CaPolicy, Roots}`)

```rust
#[non_exhaustive]
pub enum Trust { Pinned(Pins), Ca(CaPolicy), PinnedOrCa { pins: Pins, ca: CaPolicy }, AnyWithKeyProof,
                 Roots(Roots), PinnedOrRoots { pins: Pins, roots: Roots } }
impl Pins { pub fn fixed(impl IntoIterator<Item = PeerId>) -> Pins; pub fn store(TrustStore) -> Pins;
            pub fn matches(&self, der: &[u8]) -> bool; }
pub struct CaPolicy { pub ca_file: PathBuf, pub subject: Vec<String>, pub san: Vec<String>, pub ip_in_dns: bool }  // feature ca
impl CaPolicy { pub fn new(ca_file: impl Into<PathBuf>) -> CaPolicy; }   // no globs, ip_in_dns true
pub struct Roots;    // Clone, Debug (a PEM bundle shows only its length)
impl Roots {
    pub fn bundle_pem(text: impl Into<String>) -> Roots;     // every CERTIFICATE block is a root
    pub fn bundle_file(path: impl Into<PathBuf>) -> Roots;   // read when the verifier is built
    pub fn platform() -> Roots;                              // feature platform-roots
}
impl PeerServerVerifier { pub fn new(&Trust, &Policy) -> Result<Arc<PeerServerVerifier>, PeerError>; }
impl PeerClientVerifier { pub fn builder(&Trust) -> PeerClientVerifierBuilder; }
#[must_use] pub struct PeerClientVerifierBuilder;   // Clone, Debug
impl PeerClientVerifierBuilder {
    pub fn policy(self, Policy) -> Self;                             // default Policy::default()
    pub fn addresses(self, source: SocketAddr, local: SocketAddr) -> Self;   // for one connection (quinn)
    pub fn build(self) -> Result<Arc<dyn ClientCertVerifier>, PeerError>;    // a policy with an acl and no addresses: PeerError::Policy
}
```

`PeerClientVerifier` implements rustls `ClientCertVerifier`, `PeerServerVerifier` implements
`ServerCertVerifier`; hand them to any rustls-based stack. Every policy requires the peer to
prove its key. Names are not checked on the pinned path; roots always check them.

- A fixed pin matches by hashing the certificate with the pin's digest; every pin is compared
  in constant time. A store holds SHA-256 pins.
- After a pin matches: no intermediates unless `allow_chain_when_pinned`, then validity dates
  unless `check_validity` is false. The pin check comes first; certificates are parsed only
  after.
- `Ca` and `PinnedOrCa` apply to servers (client certificates); `PeerServerVerifier::new`
  returns `PeerError::Unsupported` for them. CA trust checks the chain and the globs.
- `Roots` and `PinnedOrRoots` apply to clients (the server's certificate);
  `PeerClientVerifier::builder(..).build()` returns `PeerError::Unsupported` for them. This is the web PKI, a
  different trust model from pinning: any server a root issued a certificate for the dialled
  name is trusted. The chain, every certificate's dates (always, whatever `check_validity`
  says) and the server name are checked, by rustls's `WebPkiServerVerifier` over the bundle,
  or by the operating system's verifier with `platform()` (which may also check revocation).
  Nothing skips the name check. Dial with the host's name: `dial_named`,
  `asynch::dial_named`, `TlsHandshake::client_named`, or rustls / quinn with a `ServerName`;
  `dial` names `SERVER_NAME`, which no root issues, so it is refused (`NameMismatch`).
- `PinnedOrRoots` takes a pinned certificate as `Pinned` does, whatever its name; any other
  certificate must pass the roots. A wrong name is `Refusal::NameMismatch`; an expired or
  not-yet-valid one rustls's `Expired*` / `NotValidYet*`, and `Revoked` and `BadEncoding` stay
  rustls's own too; any other chain failure (unknown issuer, bad signature, a leaf not for
  server authentication, a platform's own reason) is `Refusal::NotTrusted`.
- A bundle (and a `CaPolicy::ca_file`) is read when the verifier is built
  (`PeerServerVerifier::new`, `ClientBuilder::build`; `PeerClientVerifier::builder(..).build()`, `ServerBuilder::build`):
  every failure is `PeerError::CaFile { path, certificate, read }` naming the file (`None` for
  `bundle_pem`), and the certificate that is malformed or cannot be a trust anchor by its
  position from 0, or `read` with the I/O error when the file cannot be read; neither when it
  holds no certificate. `Display`: `CA file <path>: certificate 1 is malformed or cannot be a
  trust anchor`, `... cannot be read`, `... holds no certificate`. `platform()` failing to start (no system roots on Linux) is
  `PeerError::Certificate`. On Android the host initializes rustls-platform-verifier from its
  JVM first (`rustls_platform_verifier::android::init_*`), or a handshake panics.
- `AnyWithKeyProof` takes any certificate whose key is proven; the caller decides from
  `PeerIdentity` afterwards.
- With a store-backed pin set, alone or beside a CA policy, an untrusted client is recorded
  as waiting only after its handshake signature verifies (TLS 1.3 and 1.2), then refused:
  every waiting entry is key-proven. `verify_client_cert` runs before that signature, so it
  records nothing. A stolen public certificate fails the handshake and leaves no entry.
- The recorded source is the address the handshake came from. `accept` (sync and `asynch`,
  `accept_with`) requires `PeerAddr`, implemented for `std::net::TcpStream` and
  `tokio::net::TcpStream`; any other stream is wrapped, `Addressed::new(io, source)` with
  `.local(local)` for the local address. `with_addresses(source, local, f)` scopes the
  addresses for a host that drives rustls itself; outside it, entries have no source.
  Caps are per source: an IPv4 address, or an IPv6 /64 (`TrustStore::with_ipv6_prefix`); the
  full `SocketAddr` is stored.
- A pin whose digest the policy does not list is `PeerError::Policy` when the verifier is built.
- A refusal is `rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(Arc<Refusal>)))`
  (alert `certificate_unknown`); a pinned certificate outside its dates is rustls's `Expired` /
  `NotValidYet`, one whose dates cannot be read `BadEncoding`. See Errors.
- The verifiers are not called on a resumed session. The configurations built here turn
  resumption off (see Policy); a stack that takes only the verifiers must turn it off itself
  (server `session_storage = NoServerSessionStorage`, `send_tls13_tickets = 0`; client
  `resumption = Resumption::disabled()`), or a peer that lost trust can resume.

- **CA name globs.** `subject` globs match the subject as x509-parser displays it (`CN=a, O=b`).
  `san` globs: one SAN must match one glob. An unprefixed glob matches a DNS, email, URI or IP
  name; `dns:`, `ip:`, `email:`, `uri:` or `upn:` limits it to that kind. Addresses are matched
  in canonical text (IPv6 compressed, an IPv4-mapped address as IPv4), and a glob that is an IP
  literal is made canonical too, so `ip:2001:db8::7` matches `2001:db8:0:0:0:0:0:7`. With
  `ip_in_dns` (the default), a `dNSName` holding an IP literal also matches an `ip:` or
  unprefixed glob as that address; `false` is strict: `ip:` globs see `iPAddress` names only.
  An unprefixed glob always matches a DNS name as text.

```rust
impl Trust {
    pub fn verify_client_chain(&self, &Policy, leaf: &[u8], intermediates: &[&[u8]]) -> Result<(), PeerError>;
}
```

`verify_client_chain` runs the client verifier's checks on a chain without a handshake:
`Pinned` (the pin, then no chain unless allowed and the dates), `Ca` (the chain to the roots,
the subject and SAN globs), `PinnedOrCa` (pin first). **No key proof**: only for a certificate a
handshake already proved (one a proxy or the host hands over). A store-backed pin set records
nothing as waiting. A refused chain is `PeerError::Io` (kind `InvalidData`) wrapping the
rustls error, so `refusal()` gives `NotPinned`, `NotAccepted`, `NotTrusted` or
`ChainWhenPinned`; a pinned certificate outside its dates is rustls's `Expired` /
`NotValidYet`. Other trusts are `PeerError::Unsupported`.

## Certificate fields (`coqui_peer_tls::{CertificateFields, Name, San}`, feature `fields`)

```rust
#[non_exhaustive] pub struct CertificateFields {   // Clone, Eq; Debug shows only the fingerprint
    pub subject: Name, pub issuer: Name, pub sans: Vec<San>, pub serial: Vec<u8>,
    pub fingerprint: PeerId, pub not_before: i64, pub not_after: i64,      // Unix seconds
}
impl CertificateFields {
    pub fn parse(der: &[u8]) -> Result<CertificateFields, PeerError>;     // malformed: PeerError::Certificate
    pub fn to_map(&self) -> BTreeMap<String, Vec<String>>;
}
#[non_exhaustive] pub struct Name { pub text: String, pub attributes: Vec<(String, String)> }
#[non_exhaustive] pub enum San { Dns(String), Email(String), Uri(String), Ip(IpAddr), Upn(String), Other { oid: String, der: Vec<u8> } }
impl San { pub fn ip(&self) -> Option<IpAddr>; }   // Ip, or a Dns name that is an IP literal; canonical
```

`parse` reads exactly one certificate and verifies nothing. `Name::attributes` are in
certificate order by short name (`cn`, `o`, `ou`, `c`, `st`, `l`, `dc`, `uid`, `email`,
`serialnumber`, else the dotted OID); a value that is not a string type is `#` and its hex.
`Upn` is the `otherName` 1.3.6.1.4.1.311.20.2.3 holding a UTF8String; `Other::der` is the
value with its `[0]` wrapper. `to_map` keys, values in certificate order: `subject`, `issuer`,
`subject.<attr>`, `issuer.<attr>`, `san.dns`, `san.email`, `san.uri`, `san.upn`, `san.ip`
(canonical), `san.dns.ip` (each DNS name that is an IP literal, canonical: a consumer may treat
it as an address), `san.other.<oid>` (hex DER), `serial` (lowercase hex, leading zero bytes
dropped, `0` for zero), `fingerprint` (`sha256:<hex>`), `not_before`, `not_after`.

## Trust store and certificates (`coqui_peer_tls::{TrustStore, TrustStoreExt}`)

The store, `WaitingEntry { id, source }`, `WaitingLimits`, the pairing flow and the rule that a
peer is recorded only after it proved its key are `coqui-peer`'s; see its API header. This
crate keeps certificates in it as `KeyMaterial` with method `tls` and label `CERTIFICATE`:

```rust
pub const METHOD: &str = "tls";
pub fn certificate_key(der: &[u8]) -> Result<KeyMaterial, PeerError>;
pub trait TrustStoreExt {
    fn accept_certificate(&self, der: &[u8]) -> Result<PeerId, PeerError>;
    fn record_waiting_certificate(&self, der: &[u8], source: Option<SocketAddr>);
}                                                  // implemented for TrustStore
```

A store-backed `Pins` counts a peer as accepted only when its entry names method `tls`.
`record_waiting` is bounded as in the core: per source IP, the rerecord interval, a total.

## Policy (`coqui_peer_tls::Policy`)

```rust
#[non_exhaustive] pub struct Policy {        // start from a preset and assign fields
    pub tls_versions: Vec<TlsVersion>,         // #[non_exhaustive] enum TlsVersion { V1_2, V1_3 }
    pub cipher_suites: Option<Vec<String>>,    // IANA names; None = all for the versions
    pub kx_groups: Option<Vec<String>>,        // e.g. "X25519"; None = all
    pub cipher_profiles: Vec<CipherProfile>,   // allowed algorithm profiles; [Mozilla] in every preset
    pub crypto: Crypto,                        // the provider; Crypto::Ring in every preset
    pub pin_digests: Vec<Digest>,
    pub check_validity: bool,
    pub allow_chain_when_pinned: bool,
    pub allow_resumption: bool,                // a resumed session skips the verifiers
    pub require_ems: bool,                     // TLS 1.2: the extended master secret (RFC 7627)
    pub waiting: WaitingLimits,                // per_source 16, total 1000, rerecord 60 s
    pub acl: Option<coqui_net_acl::Acl>,       // feature acl; applied by every server configuration
}
impl Default for Policy { /* Policy::intermediate() */ }
impl Policy {
    pub fn modern() -> Policy;  pub fn intermediate() -> Policy;  pub fn legacy() -> Policy;
    pub fn preset(name: &str) -> Result<Policy, PeerError>;
    pub fn non_defaults(&self) -> Vec<NonDefault>;   // names are key paths: "waiting.per_source"
    pub fn non_defaults_against(&self, baseline: &Policy) -> Vec<NonDefault>;
    pub fn check_pin(&self, fingerprint: &PeerId) -> Result<(), PeerError>;
    pub fn check(&self) -> Result<(), PeerError>;   // what a configuration build checks, without one
    pub fn crypto_provider(&self) -> Result<Arc<CryptoProvider>, PeerError>;
    pub fn supported_cipher_suites(&self) -> Result<Vec<String>, PeerError>;   // of the provider `crypto` selects
    pub fn supported_kx_groups(&self) -> Result<Vec<String>, PeerError>;
    pub fn deserialize_over<'de, D: Deserializer<'de>>(base: Policy, d: D) -> Result<Policy, D::Error>;  // feature serde; merges key by key, ACL fingerprints made canonical
}
pub use coqui_builder::NonDefault;    // { setting: String, value: Shown, default: Shown }; prints as Type(text): bool(true), [TlsVersion(1.3)], unset
impl TlsVersion { pub const fn as_str(self) -> &'static str; }   // "1.2", "1.3"; also Display; FromStr refuses others
```

| Field | modern | intermediate (default) | legacy |
| --- | --- | --- | --- |
| `tls_versions` | 1.3 | 1.2, 1.3 | 1.2, 1.3 |
| `pin_digests` | sha256, sha384, sha512 | same | plus sha1 |
| `check_validity` | true | true | true |
| `allow_chain_when_pinned` | false | false | false |
| `allow_resumption` | false | false | false |
| `require_ems` | true | true | false |
| `waiting` (`per_source`, `total`, `rerecord`) | 16, 1000, 60 s | same | same |
| `cipher_suites`, `kx_groups`, `acl` | none | none | none |
| `cipher_profiles`, `crypto` | `[mozilla]`, `ring` | same | same |

`check` runs the checks a configuration build runs (versions, suites and groups against the
provider and this build; with `acl`, that every rule's fingerprint is a SHA-256 peer id) and
builds nothing: for a host that builds its configurations later, one per connection.

`crypto_provider` is the provider `crypto` names narrowed to what the policy offers (next
section); an unknown suite or group name is `PeerError::Policy` whose message lists the
provider's names, and so is a policy that leaves no version or no group. `preset` takes `modern`, `intermediate` or `legacy`. `non_defaults` is the places where
the policy's value departs from the defaults (`coqui_builder::non_defaults`), one
`NonDefault` each, `setting` its key path (`tls_versions`, `waiting.per_source`) and `value` /
`default` shown as their type and text (`bool(true)`, `Duration(60s)`, `[TlsVersion(1.3)]`; `unset` for a
missing side), in key order. A looser and a stricter
value are alike differences: empty for `intermediate`, `tls_versions` for `modern`, `pin_digests`
and `require_ems` for `legacy`. With `acl`, a list present is one entry, `acl` (`coqui_builder::non_defaults` over the list reports its own).
`non_defaults_against(&base)` compares with `base`'s value instead of the defaults.

`allow_resumption` false (every preset) builds configurations with resumption off at both ends:
the server keeps no session cache and sends no TLS 1.3 tickets, the client keeps no sessions, so
every connection runs the verifiers and a peer removed from the store or no longer pinned is
refused at its next connection. True keeps rustls's defaults (a 256-session cache, two TLS 1.3
tickets) and lets a peer that lost trust reconnect on a session it holds.

### Cipher profiles and the provider (`coqui_peer_tls::{CipherProfile, Crypto}`)

```rust
#[non_exhaustive] pub enum CipherProfile { Mozilla, Fips1403Compliant, Fips1402Compliant, Fips1403Validated, Cnsa1, Cnsa2 }
impl CipherProfile {
    pub const ALL: &'static [CipherProfile];
    pub const FIPS_VALIDATED_AVAILABLE: bool;            // feature fips
    pub const fn as_str(self) -> &'static str;           // also Display; FromStr lists the known names
}
#[non_exhaustive] pub enum Crypto { Ring /* default */, AwsLc }
impl Crypto { pub const AWS_LC_AVAILABLE: bool; pub const fn as_str(self) -> &'static str; }  // "ring", "aws-lc"
```

A policy lists the profiles it allows. Offered = the union of the allowed profiles (preference
order from the first listed) ∩ `tls_versions` ∩ the provider ∩ `cipher_suites` / `kx_groups`
when set. A TLS version left with no suite is dropped (a `cnsa-2.0`-only policy under the default
versions is TLS 1.3); none left, an empty `cipher_profiles`, or no group is `PeerError::Policy`.
Signature schemes come only from the profiles (and they bound certificate chains as well as
handshake signatures). Profiles and the provider are choices, not non-default settings.

| Text | Claim (quoted from the docs) | Suites | Groups | Signatures |
| --- | --- | --- | --- | --- |
| `mozilla` (default) | Mozilla server-side TLS guideline 6.0, intermediate (holds modern's) | 1.3 AES-128-GCM, AES-256-GCM, ChaCha20; 1.2 ECDHE-ECDSA/RSA with the same, Mozilla's order | X25519MLKEM768 (aws-lc), X25519, P-256, P-384 | every scheme the provider verifies |
| `fips-140-3-compliant` | "algorithm selection only; not a FIPS 140-3 validated module; makes no validation claim" (SP 800-52r2, SP 800-131A r2) | 1.3 AES-128/256-GCM; 1.2 ECDHE-ECDSA/RSA AES-GCM | P-256, P-384; aws-lc: SecP256r1MLKEM768, X25519MLKEM768, MLKEM768, MLKEM1024 | RSA-PSS, RSA PKCS#1 (SHA-256/384/512), ECDSA P-256/SHA-256, P-384/SHA-384; no Ed25519, no ML-DSA |
| `fips-140-2-compliant` | algorithm selection only, no validation claim | as 140-3 | P-256, P-384 (no ML-KEM) | as 140-3 |
| `fips-140-3-validated` | "AWS-LC's FIPS module; validated only as its CMVP certificate and tested operating environments state" | as 140-3, approved only | as 140-3, approved only | as 140-3 |
| `cnsa-1.0` | CNSSP 15 / RFC 9151 as far as rustls goes | 1.3 AES-256-GCM-SHA384; 1.2 ECDHE-ECDSA/RSA AES-256-GCM-SHA384 | secp384r1 | ECDSA P-384/SHA-384, RSA-PSS SHA-384, RSA PKCS#1 SHA-384 (3072+ bits) |
| `cnsa-2.0` | CNSA 2.0 in the shape of draft-becker-cnsa2-tls-profile-05 | 1.3 AES-256-GCM-SHA384 | MLKEM1024 | ML-DSA-87 |

- **`fips-140-3-validated`**, alone or with others, builds on rustls's FIPS provider whatever
  `crypto` says, keeps only what the module reports approved (other listed profiles lose
  ChaCha20 and X25519), and every builder refuses (`PeerError::Policy`, "fips-140-3-validated:
  rustls does not report this configuration FIPS ...") unless `ClientConfig::fips()` /
  `ServerConfig::fips()` is true, which also needs `require_ems`. Without the `fips` feature it
  is `PeerError::Policy("this build has no FIPS module: feature fips")`.
- **`crypto = Crypto::AwsLc`** without the `aws-lc` feature is `PeerError::Policy`.
  `cnsa-2.0` with `ring` is `PeerError::Policy("cnsa-2.0 needs crypto = aws-lc: ring has no
  ML-KEM or ML-DSA")`.
- **Not met, stated**: `cnsa-1.0` lacks RFC 9151's static-RSA and DHE suites and ffdhe groups
  (absent from rustls; SP 800-131A disallows static RSA) and the 3072-bit floor for RSA-PSS keys;
  `cnsa-2.0` leaves out the ML-KEM-768 hybrids (category 3) and has no SecP384r1MLKEM1024 (not in
  rustls; the draft does not use it); whether AWS-LC's certificate covers ML-KEM hybrids and
  ML-DSA is unconfirmed, so a `fips-140-3-validated` + `cnsa-2.0` build the module does not
  report FIPS is refused as usual.
- **Identities**: generated ones are ECDSA P-256 made with ring, outside any FIPS module, and
  neither CNSA profile accepts them: bring a P-384 or RSA-3072 identity for `cnsa-1.0`, an
  ML-DSA-87 one (with its ML-DSA-87 CA chain) for `cnsa-2.0`, and a validated deployment brings
  its own. `Identity::from_pem` loads a key ring cannot (ML-DSA) when `aws-lc` is on.

### Settings as data

Build a `Policy` in code (constructors and public fields), or read it. `TlsSettings`, `Policy`
and `PairConfig` derive `coqui_builder::Describe`: `T::description()` describes each key with its
default, and with coqui-builder's `serde` feature `toml::from_str::<Partial<T>>(text)?.build()` (or
JSON, or any serde format) reads the text. They are never written. Every key is optional and takes the
intermediate preset's value; an unknown key is an error.

```rust
#[non_exhaustive] pub struct TlsSettings { pub alpn: Vec<Alpn>, pub policy: Policy }   // Default; Describe

/// An application protocol name of 1 to 255 bytes, held as text; a name that is not UTF-8 cannot be held.
pub struct Alpn;   // Debug, Clone, PartialEq, Eq, Hash, Display, FromStr (Err PeerError::Policy), coqui_builder::Leaf
impl Alpn { pub fn as_bytes(&self) -> &[u8]; pub fn as_str(&self) -> &str; }
impl TryFrom<&str> for Alpn { type Error = PeerError; }     // refuses empty and over 255 bytes
impl TryFrom<&[u8]> for Alpn { type Error = PeerError; }   // the same, and non-UTF-8 bytes
```

`Policy` keys: `tls_versions` (`"1.2"`, `"1.3"`), `cipher_suites`,
`kx_groups`, `cipher_profiles` (the profiles' text names; an unknown one is an error listing the
known), `crypto` (`"ring"`, `"aws-lc"`), `pin_digests` (`"sha256"`, `"sha384"`, `"sha512"`,
`"sha1"`), `check_validity`, `allow_chain_when_pinned`, `allow_resumption`, `require_ems`,
`waiting` (a table: `per_source`, `total`, `rerecord`) and `acl` (see `coqui-net-acl`). A duration
(`rerecord`, `timeout`) is read as `60`, `30s`, `250ms` or `1.5h` (a number is seconds); `[secs, nanos]` is not read.
`PairConfig` keys: `policy`, `alpn` (text of 1 to 255 bytes), `timeout` (a duration, or `never`).
`tests/vectors/policy/{modern,intermediate,legacy,acl,profiles}.{toml,json}` are read by the tests.

A consumer with its own default profile reads over it with `Policy::deserialize_over` (`serde`), or by
overlaying a `Partial<Policy>` on `Partial::from(base)`. With `serde`, `Policy` implements `Deserialize`
(over the intermediate preset, the ACL fingerprints made canonical); nothing is written.

## Access control (`Policy::acl`, feature `acl`)

Every server configuration built under a policy with `acl: Some(..)` applies it:
`ServerBuilder::build`, `ServerBuilder::configs`,
`ServerConfigs::for_connection` and `pair_server_config`.

- **By address, at the `ClientHello`**: the certificate resolver is wrapped and answers only an
  admitted connection. When every client must present a certificate, `Acl::decide` (a rule
  naming a fingerprint defers to the certificate); otherwise `Acl::decide_anonymous`: server TLS
  (`ClientAuth::None`), and a host verifier whose
  `client_auth_mandatory()` is false, since its client may send no certificate and the
  fingerprint check would never run. So under optional client authentication a rule naming a
  fingerprint never admits a client. A denial is `Refusal::AddressDenied` and no certificate is
  sent.
- **By fingerprint, in the client verifier**: `Acl::decide_peer` with the certificate's
  canonical `sha256:<hex>`, before pins, CA policy or trust store are consulted, so a denied
  client is never recorded as waiting. A denial is `Refusal::PeerDenied`.
- **Fingerprints are canonical.** Rules may be written in any spelling `PeerId` parses (hex,
  colon pairs, grouped base32); they are rewritten to `sha256:<hex>` when a configuration is
  built, and `Policy::check` reports one that cannot be; reading a policy through serde canonicalizes them too. A rule naming something that is not a SHA-256 peer id
  is `PeerError::Policy` there ("acl rule N: ...").
- **Addresses.** A configuration shared by connections reads them from the scope the accept
  helpers set: `accept` takes both from a `PeerAddr` (`TcpStream`, tokio's too, or an
  `Addressed` stream with the source and optionally the local address), `TlsHandshake::server(.., source)`
  the source and `.local(local)` the local address, `with_addresses` both for a host
  driving rustls itself. Without a source, or without a local address when a rule names a
  destination or port, the connection is refused (`Refusal::AddressUnknown`); a missing local
  address is fine when no rule reads it. A configuration for one connection
  (`ServerConfigs::for_connection`, `PeerClientVerifier::builder(..).addresses(..)`) holds its addresses and
  needs no scope: for quinn, which runs the verifier on its own threads.
- `Policy::admits_address(source, local)` / `admits_anonymous` give the address decision
  without a handshake, for a host that refuses a connection before it (a quinn `Incoming`).
- Session resumption skips the fingerprint check, as it skips the verifiers; the address check at
  the `ClientHello` still runs (rustls resolves the certificate before it resumes). Resumption
  is off unless `allow_resumption`, which `non_defaults()` reports.

```rust
pub fn with_addresses<R>(source: Option<SocketAddr>, local: Option<SocketAddr>, work: impl FnOnce() -> R) -> R;
pub trait PeerAddr { fn peer_addr(&self) -> Option<SocketAddr>; fn local_addr(&self) -> Option<SocketAddr> { None } }
// accept(config, Addressed::new(io, source).local(local)): both addresses for a stream that names none
impl TlsHandshake { pub fn local(self, local: SocketAddr) -> TlsHandshake; }
impl Policy {   // feature acl
    pub fn admits_address(&self, source: SocketAddr, local: SocketAddr) -> Result<(), Refusal>;
    pub fn admits_anonymous(&self, source: SocketAddr, local: SocketAddr) -> Result<(), Refusal>;
}
```

A `PeerClientVerifier::builder(..).addresses(source, local)` verifier records waiting entries
under its `source` (no scope needed) and, with an ACL, refuses a denied fingerprint before the
pins; it does not apply the address rules, which `ServerConfigs::for_connection` (both) or
`Policy::admits_address` (before the handshake) do. Without `addresses` the builder refuses a
policy with an ACL (`PeerError::Policy`) rather than ignore it.

### Configurations per connection (`coqui_peer_tls::{ServerConfigs, ConnectionConfig}`)

```rust
pub struct ServerConfigs;   // Clone (shares the session state), Debug; made by ServerBuilder::configs
impl ServerConfigs {
    pub fn non_defaults(&self) -> &[NonDefault];
    pub fn for_connection(&self, source: SocketAddr, local: SocketAddr) -> Result<ConnectionConfig, PeerError>;
}
pub struct ConnectionConfig;   // Clone, Debug
impl ConnectionConfig {
    pub fn config(&self) -> Arc<ServerConfig>;   // quinn: QuicServerConfig::try_from(config)
    pub fn refusal(&self) -> Option<Refusal>;    // the first refusal of this connection's handshake
}
```

For a host that builds a configuration per connection (quinn's `Incoming::accept_with`).
`ServerBuilder::configs` builds what `build` builds, and checks the identity and policy once
(its errors are `build`'s); `non_defaults` is what `Built::non_defaults` would hold.
`for_connection` then gives one connection's configuration: waiting entries recorded under
`source`, the ACL decided from `source` and `local` (by address at the `ClientHello`, by
fingerprint before the store), and every configuration from one `ServerConfigs` sharing one
session cache and ticketer, so with `allow_resumption` a client resumes across connections
(separate `ServerConfigs` never share sessions). quinn reports a failed server handshake only by
its alert; `refusal()` gives this side's typed reason: `AddressDenied` / `AddressUnknown` at the
hello, `PeerDenied` and every trust refusal in the verifier (a host verifier's too, when it
returns a `Refusal`). Check `Policy::admits_address` before `accept_with` to refuse without a
handshake.

## Configurations and streams (`coqui_peer_tls`, `coqui_peer_tls::asynch`)

```rust
#[derive(Configured)] #[non_exhaustive] pub struct Built<T> { pub config: T, #[configured] pub policy: Policy }
impl<T> Built<T> { pub fn non_defaults(&self) -> Vec<NonDefault>; }   // the policy's own keys: "tls_versions"
#[non_exhaustive] pub enum ClientAuth {          // Clone, Debug
    None,                                        // server TLS: the client presents no certificate
    Required(Trust),                             // a client certificate this trust accepts
    Verifier(Arc<dyn ClientCertVerifier>),       // a verifier of the host's own
}
// Derived by coqui-builder from ServerParts<'a> { settings: TlsSettings, input identity: &'a Identity, input client_auth: ClientAuth }.
// Debug, not Clone; ServerBuilder::new() is the entry.
pub struct ServerBuilder<'a>;
impl<'a> ServerBuilder<'a> {
    pub fn new() -> Self;                                  // nothing set; also Default
    pub fn identity(self, &'a Identity) -> Self;           // required input
    pub fn client_auth(self, ClientAuth) -> Self;          // required input
    pub fn alpn(self, impl Into<Vec<Alpn>>) -> Self;       // the TlsSettings setters: alpn, policy (last write wins)
    pub fn policy(self, impl Into<Policy>) -> Self;
    pub fn fill(self, Partial<TlsSettings>) -> Self;       // overlay: only the keys set
    pub fn fill_from(self, &mut impl Source) -> Result<Self, FillError>;
    pub fn non_defaults(&self) -> Vec<NonDefault>;         // over TlsSettings: "policy.tls_versions", and "alpn" when set
    pub fn parts(self) -> Result<ServerParts<'a>, Missing>;
    pub fn build(self) -> Result<Built<ServerConfig>, BuildError<PeerError>>;   // Missing names the inputs not given
    pub fn configs(self) -> Result<ServerConfigs, BuildError<PeerError>>;       // one configuration per connection
}
#[non_exhaustive] pub enum ServerAuth {          // Clone, Debug; From<Trust>
    Trust(Trust),                                // pins, roots, pinned-or-roots, any with key proof
    Verifier(Arc<dyn ServerCertVerifier>),       // a verifier of the host's own
}
// Derived from ClientParts<'a> { settings: TlsSettings, input server: ServerAuth, input identity: Option<&'a Identity> }.
pub struct ClientBuilder<'a>;   // Debug, not Clone
impl<'a> ClientBuilder<'a> {
    pub fn new() -> Self;
    pub fn server(self, impl Into<ServerAuth>) -> Self;   // required input; a Trust converts
    pub fn identity(self, &'a Identity) -> Self;           // optional; presented with its whole chain
    pub fn alpn(self, impl Into<Vec<Alpn>>) -> Self;
    pub fn policy(self, impl Into<Policy>) -> Self;
    pub fn fill(self, Partial<TlsSettings>) -> Self;
    pub fn fill_from(self, &mut impl Source) -> Result<Self, FillError>;
    pub fn non_defaults(&self) -> Vec<NonDefault>;         // over TlsSettings, as the server builder's
    pub fn parts(self) -> Result<ClientParts<'a>, Missing>;
    pub fn build(self) -> Result<Built<ClientConfig>, BuildError<PeerError>>;
}
pub fn crypto_provider() -> Arc<CryptoProvider>;       // default ring provider
pub const SERVER_NAME: &str = "coqui-peer.invalid";
pub fn default_server_name() -> ServerName<'static>;   // SERVER_NAME as rustls takes it
pub fn dial<S: Read + Write>(Arc<ClientConfig>, S) -> Result<(StreamOwned<ClientConnection, S>, PeerIdentity), PeerError>;
pub fn dial_named<S: Read + Write>(Arc<ClientConfig>, ServerName<'static>, S) -> Result<(StreamOwned<ClientConnection, S>, PeerIdentity), PeerError>;
pub fn accept<S: Read + Write + PeerAddr>(Arc<ServerConfig>, S) -> Result<(StreamOwned<ServerConnection, S>, PeerIdentity), PeerError>;
pub trait PeerAddr { fn peer_addr(&self) -> Option<SocketAddr>; fn local_addr(&self) -> Option<SocketAddr> { None } }   // std and tokio TcpStream
pub struct Addressed<S>;     // Debug; Read, Write, ReadTimeout, WriteTimeout, and with `tokio` AsyncRead, AsyncWrite, over S
impl<S> Addressed<S> {
    pub fn new(io: S, source: SocketAddr) -> Self;   // PeerAddr: Some(source), local None
    pub fn local(self, SocketAddr) -> Self;          // the address connected to (an ACL's destination and port rules)
    pub fn get_ref(&self) -> &S;  pub fn get_mut(&mut self) -> &mut S;  pub fn into_inner(self) -> S;
}
pub fn with_addresses<R>(source: Option<SocketAddr>, local: Option<SocketAddr>, impl FnOnce() -> R) -> R;
// feature tokio, module asynch:
pub async fn dial<S>(Arc<ClientConfig>, S) -> Result<(client::TlsStream<S>, PeerIdentity), PeerError>;
pub async fn dial_named<S>(Arc<ClientConfig>, ServerName<'static>, S) -> Result<(client::TlsStream<S>, PeerIdentity), PeerError>;
pub async fn accept<S: PeerAddr>(Arc<ServerConfig>, S) -> Result<(server::TlsStream<S>, PeerIdentity), PeerError>;
pub async fn accept_with<S: PeerAddr>(S, select: impl FnOnce(&ClientHello<'_>) -> Arc<ServerConfig>) -> Result<(server::TlsStream<S>, PeerIdentity), PeerError>;
```

`ClientAuth::Verifier` and `ServerAuth::Verifier` build exactly what the trust builds (the
policy's versions, suites, groups, extended master secret and resumption, the identity's whole
chain, ALPN) around a verifier the host supplies, typically one of this crate's wrapped; `Built::non_defaults` is the policy's. A host
building a rustls or quinn configuration by hand takes the identity's `certificate_chain()` and
`private_key_der()`.

Both complete the handshake before returning, so a refused certificate is an error and the
returned `PeerIdentity` is verified. `dial` sends `SERVER_NAME`, which pinned trust ignores
(trust by fingerprint); `dial_named` sends the name given, which `Roots` check. The returned
peer's fingerprint is the leaf certificate's however the server was verified. `accept_with`
chooses the server configuration from the ClientHello, for example by ALPN, so one port serves
peers and other clients.

### Server TLS

The server proves itself by certificate; the client presents none and **must be
authenticated by the application inside the encrypted channel** (a key proof, a token) before
it is trusted. For browsers (`wss://`, WebTransport), which cannot present a chosen client
certificate, and hosts without an identity of their own.

- `ClientAuth::None` handles versions, suites, groups, ALPN and resumption as `Required` does
  (resumption off unless `allow_resumption`) and requests no client certificate. `Built::non_defaults` is the policy's: a server that asks for no client certificate lists
  nothing for it.
- A `ClientBuilder` with no `identity` verifies the server by `Trust` as with one
  (pins or a store; roots; `AnyWithKeyProof`; CA is `Unsupported`) and presents no
  certificate. A mutual-TLS server refuses such a client. Its `non_defaults` is the policy's.
- The server finishes with `TlsHandshake::finish_unauthenticated` (below), or any rustls /
  tokio-rustls acceptor: there is no client verifier, so `with_addresses` has nothing to scope.
  `accept` and the `asynch` accepts return a verified client, so with this
  configuration they fail with `PeerError::Certificate` after the handshake. `dial` and
  `TlsHandshake::finish` on the client side work as in mutual TLS.

## The method (`coqui_peer_tls::{Tls, TlsHandshake}`)

```rust
pub struct Tls;      // Clone, Default
impl Tls { pub fn new() -> Tls; pub fn client(self, Arc<ClientConfig>) -> Tls; pub fn server(self, Arc<ServerConfig>) -> Tls; }
impl coqui_peer::Method for Tls { /* name() = "tls"; initiate(); respond(source: Option<SocketAddr>) */ }
pub struct TlsHandshake;   // client(Arc<ClientConfig>), server(Arc<ServerConfig>, Option<SocketAddr>)
impl TlsHandshake {
    pub fn client_named(Arc<ClientConfig>, ServerName<'static>) -> Result<TlsHandshake, PeerError>;   // client() sends SERVER_NAME
    pub fn finish_unauthenticated(self) -> Result<(rustls::Connection, Option<PeerIdentity>), PeerError>;
}
impl coqui_peer::Handshake for TlsHandshake { /* Output = rustls::Connection, Framing::Stream */ }
```

`receive` feeds rustls (`read_tls`, `process_new_packets`); `transmit` drains `write_tls`;
`is_complete` is `!is_handshaking()`. `finish` gives `Established { peer, key: Some(certificate),
session: Connection }`; the connection keeps application data that arrived with the last
handshake bytes. The server handshake scopes its `source` around each `receive`, so waiting
entries carry the address exactly as `accept` does. A role without a configuration is
`PeerError::Unsupported`. Run it with `coqui_peer::drive_stream` / `drive_messages` (the
latter needs a reliable, ordered message channel) or the `tokio` forms.

`finish` needs a peer certificate (`PeerError::Certificate` without one), so a server-TLS server
drives its handshake with `coqui_peer::complete_stream` / `complete_messages` (or the
`receive`/`transmit` loop itself) and calls `finish_unauthenticated`: the connection, and
`Some` verified peer only when the client sent a certificate. With a configuration from
`ClientAuth::None` it is always `None`, meaning not authenticated. Before
completion it is `PeerError::Tls`.

## Pairing and rotation over TLS (`coqui_peer_tls`, `coqui_peer_tls::pair`)

The `coqui-peer` pairing exchange (re-exported as `coqui_peer_tls::pair`; its API header has the
modes, options, desk, throttles, their costs and the security model) on a connection of its
own, and key rotation over an authenticated one. TLS settings are the device's own `Policy`;
nothing here sets a version or suite.

```rust
pub use coqui_peer::pair;                    // Pairing, PairingDesk, PairingOptions, DeskLimits, Pin, ...
pub use coqui_peer::{Clock, Random, Invitation, InvitationOptions, RotationOptions, RotationEvent};
pub const PAIR_ALPN: &[u8] = b"coqui-pair/1";
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-coqui-pair";
#[non_exhaustive] pub struct PairConfig { pub policy: Policy, pub alpn: Alpn, pub timeout: Option<Duration> }  // Default: Policy::default(), PAIR_ALPN, Some(15 min); plain data, Describe; None is the text "never"
pub use coqui_peer::{ReadTimeout, WriteTimeout};   // set_read_timeout / set_write_timeout(&self, Option<Duration>); TcpStream, &TcpStream
#[non_exhaustive] pub enum ResponderTrust { Unverified, Verified { trust: Box<Trust>, name: Option<ServerName<'static>> } }
impl ResponderTrust { pub fn pinned(id: PeerId) -> ResponderTrust; }
pub fn random() -> Random;                   // the TLS provider's secure random source
pub fn pair_server_config(&Identity, &PairConfig) -> Result<ServerConfig, PeerError>;
pub fn pair_client_config(&Identity, &Pairing, &ResponderTrust, &PairConfig) -> Result<(ClientConfig, ServerName<'static>), PeerError>;
pub fn pair_dial<S: Read + Write + ReadTimeout + WriteTimeout>(S, &Identity, Pairing, &ResponderTrust, &PairConfig) -> Result<PairDial<S>, PeerError>;
impl<S> PairDial<S> { pub fn shown(&self) -> Option<&Shown>; pub fn finish(self, &TrustStore) -> Result<Paired, PeerError>; }
pub fn pair_accept<S: Read + Write + PeerAddr + ReadTimeout + WriteTimeout>(S, &Identity, &TrustStore, &PairingDesk, &PairConfig) -> Result<Paired, PeerError>;
pub fn announce(&mut impl Write, successor: &Identity) -> Result<(), PeerError>;
pub fn receive_announcement(&mut impl Read, &TrustStore, sender: &PeerIdentity) -> Result<PeerId, PeerError>;
impl Identity { pub fn renew_before(&self, margin: Duration) -> bool; pub fn renew_before_at(&self, Duration, SystemTime) -> bool; }
// feature tokio, module asynch: pair_dial -> asynch::PairDial (shown, async finish), pair_accept,
// pair_respond(server::TlsStream<S>, Option<SocketAddr>, &Identity, &TrustStore,
// &PairingDesk, &PairConfig), announce, receive_announcement
```

**Pairing connection.** `pair_server_config` admits any client certificate whose key is proven,
under `PairConfig::alpn` only; the app's own configurations stay as strict as they are, so an
unknown certificate offering the app's ALPN is still refused. Both ends check that the pairing
ALPN was negotiated (`PeerError::Tls` otherwise), then run the exchange with both certificates as
`Channel::local`/`remote` and the exporter (`EXPORTER_LABEL`, 32 bytes, empty context) as the
binding. Messages are `coqui_peer` frames (`write_frame` / `read_frame`: a four-byte big-endian
length, at most `coqui_peer::MAX_MESSAGE`).
The connection carries only the exchange and then closes; both sides reconnect with the app's
ALPN and their stores. One port can serve both: `asynch::accept_with` choosing
`pair_server_config` when the ClientHello offers the pairing ALPN, then `asynch::pair_respond`.

**Joiner.** `pair_dial` handshakes and runs the exchange until there is something to show
(`shown()`: its PIN with the slot, or the device-flow user code) or the end; `finish` runs the rest
(for a shown code, until the operator acts at the responder), `accept_key`s the responder's
certificate in the joiner's store, and closes. The responder's certificate is checked by:
an invitation, a pin of the inviter whatever `ResponderTrust` says; a PIN, `ResponderTrust` (the
PIN authenticates the responder, so `Unverified` is enough); the device flow, `Verified` trust
(pins, a store, or roots with the host's `name`), and `Unverified` or `Trust::AnyWithKeyProof` is
`PeerError::Pairing(PairingError::Unverified)` before connecting unless
`Pairing::allow_unverified(true)`, at the cost stated in `coqui-peer`'s header. The channel
handed to the exchange is `verified` exactly when the inviter was pinned or the trust was
`Verified` with anything but `AnyWithKeyProof`.

**Responder.** `pair_accept` handshakes, then runs the responder until done: redeeming an
invitation, taking the shown PIN, or waiting for the operator through the desk. The blocking form
blocks in `PairingDesk::wait` (rechecks expiry every second on the system clock); the async form
awaits `PairingDesk::decided` and sweeps the desk every second meanwhile. On success the joiner's
certificate is accepted in the store before the joiner is told. The address of the stream is the
source the desk throttles by (an `Addressed` stream for one that names none).

**Timeouts.** One side's whole exchange, its TLS handshake included, takes at most
`PairConfig::timeout` (15 minutes by default, longer than the default `device_ttl`;
`None`, the text `never`, waits forever): the joiner's `pair_dial` and `finish` together, the responder's
`pair_accept` (or `asynch::pair_respond` from its call). Each responder read also waits at most
until the responder's own deadline (`PairingResponder::time_left`: `pin_ttl` or `device_ttl` from
the joiner's first message); past it the joiner is told `Expired` and its slot, code and pending
place are freed. A stalled transport ends with `PeerError::Io` (`TimedOut` or `WouldBlock`). The
blocking forms set read and write timeouts to what is left through `ReadTimeout` and
`WriteTimeout`, implemented for `TcpStream` and `&TcpStream`; a stream that cannot time out
implements them as no-ops and is bounded by its caller. The async forms bound every read and
write with `tokio::time::timeout`. A frame's buffer grows as its bytes arrive, so a declared length (at
most `coqui_peer::MAX_MESSAGE`) costs nothing until it is sent.

**What each TLS version gives.** TLS 1.3 (what two default peers negotiate, and all `modern`
allows): the exporter is unique to the connection, so a relay between two connections fails the
PIN confirmation. TLS 1.2 (a policy that allows it): the exporter is unique only with the
extended master secret, which `modern` and `intermediate` require (`Policy::require_ems`,
applied to every configuration built here, pairing's included); a policy that turns it off
(`legacy` does) is reported by `non_defaults()`, and then the
confirmation still covers both certificates, which a relay cannot replace with its own without
failing it.

**Rotation.** A peer sends `announce(stream, &successor)` over a connection its current
certificate authenticated; the other side calls `receive_announcement(stream, store, &peer)`,
which records the successor (`TrustStore::announce_successor`; a peer that is not accepted, or
is retiring, is `PeerError::Pairing(PairingError::Local(coqui_peer::Error::NotAccepted))`). A
store-backed `Pins` then admits the successor: the verifier lets it through presentation
(after `check_pinned`'s chain and date checks), and once its handshake signature verifies,
records the proof (`prove_successor`) and promotes it (`promote_successor`), so the
predecessor retires after `RotationOptions::retire_after`. With
`RotationOptions::require_approval` the handshake is refused (`NotAccepted`) after the proof
is recorded, and the caller promotes it when it approves (`promote_successor` refuses a
successor nobody proved unless `allow_unproven`); it is not recorded as waiting. This
works both ways: a server verifying clients by its store, and a client pinning its server by
its store. `take_rotation_events` returns what happened. `Identity::renew_before(margin)` says
when the certificate ends within `margin` (an unreadable one is due).

## On-disk formats

For peers in other languages. All files are UTF-8 with LF line endings.

- **Identity file** — one PEM file: the leaf's `CERTIFICATE` block, the intermediates'
  `CERTIFICATE` blocks in order (a chained identity only), then a PKCS#8 `PRIVATE KEY` block;
  64-character base64 lines. Mode `0600` on Unix. `read` and `open_or_create` read either
  shape; `from_pem` takes the single-certificate one only.
- **Certificate id** (`PeerId`) — SHA-256 of the certificate's DER encoding. Canonical text
  `sha256:<64 lowercase hex>` (`openssl x509 -outform DER | sha256sum` gives the hex).
  Grouped base32 (RFC 4648 alphabet, no padding, 52 characters, groups of four joined by `-`)
  is a display form.
- **Trust store** — `coqui-peer`'s format (see its header): certificates are kept as
  `peers/{accepted,waiting}/<64 hex>.pem` with header lines `Method: tls` and (waiting)
  `Source: <ip>:<port>`, then one `CERTIFICATE` block whose DER hashes to the name. The store
  vectors live in that crate.
- **Vectors** — `tests/vectors/identity-{a,b}.pem` and `identity-{a,b}.fingerprint` (three
  lines: canonical, bare hex, grouped base32) are checked by `tests/vectors.rs`; the policy
  vectors under `tests/vectors/policy/` are byte-exact text forms (TOML and JSON, read in
  tests through dev-dependencies only).

## Errors

```rust
#[non_exhaustive] pub enum PeerError { InvalidId, Certificate, Tls, WeakDigest, Policy(String),
    TrustPattern, NotWaiting, AlreadyAccepted, Path, Unsupported, Pairing(pair::PairingError), Io(io::Error),
    CaFile { path: Option<PathBuf>, certificate: Option<usize>, read: Option<io::Error> } }
impl PeerError { pub fn refusal(&self) -> Option<Refusal>; pub fn pair_refusal(&self) -> Option<pair::PairRefusal>; }
#[non_exhaustive] pub enum Refusal {      // Copy, Eq, Hash, Display, std::error::Error
    NotPinned,         // matches no fixed pin
    NotAccepted,       // not accepted in the trust store (a store-backed server records it as waiting)
    NotTrusted,        // no pin, and the CA policy or the roots do not trust its chain
    ChainWhenPinned,   // a pinned peer sent a chain and allow_chain_when_pinned is false
    NameMismatch,      // the roots trust the server's chain, but not for the name dialled
    AddressDenied { rule: Option<usize> },   // the policy's ACL denies the addresses (rule index; None = default)
    PeerDenied { rule: Option<usize> },      // the policy's ACL denies the client certificate from these addresses
    AddressUnknown,    // the policy has an ACL and the source, or a local address a rule needs, is unknown
}
impl Refusal { pub fn from_rustls(error: &rustls::Error) -> Option<Refusal>; }
```

What each refusal looks like (server side unless stated; the refused side never gets a
`Refusal`, only the alert):

| Refusal | Decided | Server (`accept*`, `asynch::accept*`, `TlsHandshake::receive`) | Alert sent | Client sees |
| --- | --- | --- | --- | --- |
| `AddressDenied { rule }` | at the `ClientHello`, before any certificate | `PeerError::Io`, kind `PermissionDenied`; `refusal()` is it | `access_denied` | `PeerError::Io` with `rustls::Error::AlertReceived(AccessDenied)` |
| `AddressUnknown`, at the hello | at the `ClientHello` | as `AddressDenied` | `access_denied` | as `AddressDenied` |
| `PeerDenied { rule }` | in the client verifier, before pins, CA or store; nothing recorded as waiting | `PeerError::Io`, kind `InvalidData`, wrapping `rustls::Error::InvalidCertificate(CertificateError::Other(..))`; `refusal()` is it | `certificate_unknown` | `PeerError::Io` with `AlertReceived(CertificateUnknown)` |
| `AddressUnknown`, in a verifier | in the client verifier | as `PeerDenied` | `certificate_unknown` | as `PeerDenied` |
| `NotPinned`, `NotAccepted`, `NotTrusted`, `ChainWhenPinned` | in the verifier | as `PeerDenied` | `certificate_unknown` | as `PeerDenied` |
| `NameMismatch` (a client refusing a server) | in the client's server verifier | the client: as `PeerDenied` | `certificate_unknown` | the server: `AlertReceived(CertificateUnknown)` |
| `PeerDenied`, `AddressUnknown` with `ServerConfigs::for_connection` / `PeerClientVerifier::builder(..).addresses(..)` (quinn) | in the verifier, from the addresses the configuration or verifier holds | the host's rustls error `InvalidCertificate(CertificateError::Other(..))`; `Refusal::from_rustls` is it, and `ConnectionConfig::refusal()` | `certificate_unknown` | as `PeerDenied` |
| `AddressDenied` with `ServerConfigs::for_connection` | at the `ClientHello`, from the configuration's addresses | rustls's own `Error::General`; `ConnectionConfig::refusal()` is it; check `Policy::admits_address` first | `access_denied` | as `AddressDenied` |
| no `Refusal`: an unusable CA or roots file | when the configuration or verifier is built, before any connection | `PeerError::CaFile { path, certificate, read }` | none | nothing |

A host driving rustls or quinn itself sees the verifier's refusals as rustls errors
(`Refusal::from_rustls` takes them out), and an address refusal at the hello as rustls's own
`Error::General` with `access_denied` sent. With `ServerConfigs::for_connection`,
`ConnectionConfig::refusal()` gives either; a host can also check `Policy::admits_address`
before the handshake, to refuse before reading a byte.

A failed handshake in `dial`, `accept` and `asynch` is `PeerError::Io` wrapping the rustls error;
`refusal()` digs the `Refusal` out of it (`TlsHandshake` errors too). Only the refusing side has
one: the refused side sees the TLS alert, so its `refusal()` is `None`. A host driving rustls
itself calls `Refusal::from_rustls`. `Display` of `PeerError` names the category only and never
key material; only `CaFile` names a path, the file it could not use.

## Gotchas

- TLS 1.0 and 1.1 and non-AEAD suites cannot be offered; asking is `PeerError::Policy`.
- Some issuers put an IP address in a `dNSName` SAN because some clients do not read IP SANs.
  `San::Dns` keeps it a DNS name; `San::ip()` and the map's `san.dns.ip` give it as an address,
  and CA globs match it as one unless `CaPolicy::ip_in_dns` is false.
- A CA policy applies to servers only, `Roots` to clients only. Pairing (waiting entries) is
  a server-side feature.
- `Roots` with `dial` (or `TlsHandshake::client`) is always refused: they send
  `SERVER_NAME`. Use the `_named` entries.
- A server-TLS server has no client verifier: no pins, no CA, no waiting entries. A client
  pinning its server works the same in both modes.
- `PeerId` equality is constant time; compare through it, not through `to_string()`.
- `PeerError`, `Refusal`, `Trust`, `TlsVersion`, `CipherProfile`, `Crypto` and `ResponderTrust`
  are `#[non_exhaustive]`: a `match` needs a `_` arm.
- With a policy ACL, a connection whose addresses the configuration cannot learn is refused
  (`Refusal::AddressUnknown`): accept through `accept` (an `Addressed` stream when it names none), `TlsHandshake::local`
  or `with_addresses`, or build the configuration for the connection
  (`ServerConfigs::for_connection`). A `PeerAddr` implementation that does not override
  `local_addr` gives none.
- `PeerClientVerifier::builder(..)` without `addresses` refuses a policy with an ACL; the
  configurations built here apply it.
- Under optional client authentication (a host verifier whose `client_auth_mandatory()` is
  false) a client is decided by address as one that proves no identity: a rule naming a
  fingerprint never admits it, even when it then presents that certificate.
- Session resumption skips the verifiers and the ACL's fingerprint rules (its address rules
  still apply): leave `allow_resumption` off where a peer can lose its access.
- The `fips` feature has global effect: rustls's `fips` changes defaults for every crate in the
  build (aws-lc-rs builds as the FIPS module, its default provider drops ChaCha20 and X25519).
  It needs CMake and Go (and on Windows bindgen with libclang, NASM and Ninja). If bindgen fails
  with `ParseQuote is not implemented for syn::File`, cargo paired bindgen 0.73 with
  prettyplease 0.3: `cargo update -p prettyplease --precise 0.2.37`.
