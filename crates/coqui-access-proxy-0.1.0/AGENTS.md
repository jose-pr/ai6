# `coqui-access-proxy` — API header

Header-file-style reference for the `coqui-access-proxy` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

MSRV Rust 1.89. No Cargo features. Depends on `coqui-access`, `coqui-access-kit` (picks), `coqui-net-acl` (address lists),
`coqui-peer-tls` (feature `fields`: certificate fields), `data-encoding`, `percent-encoding` and
`rustls-pki-types` (PEM). No `unsafe` code. The
loadable library is the separate crate `coqui-access-proxy-plugin`.

```rust
pub struct ProxyWordConfig {
    pub trusted: Vec<String>,            // addresses and CIDR networks (coqui-net-acl IpNet); empty trusts nobody; `any` refused
    pub user_header: Option<String>,     // account name header
    pub certificate_header: Option<String>,          // the client certificate the proxy verified
    pub certificate_encoding: CertificateEncoding,   // default PemUrl
    pub subject_header: Option<String>,  // client-certificate subject header
    pub account: Vec<Pick>,              // default default_account(): subject.cn by name
    pub max_certificate_bytes: Limit,    // default 16 384
}                                        // non_exhaustive; Default = an empty configuration
pub enum CertificateEncoding { PemUrl /* default */, DerBase64, Xfcc }   // "pem-url" | "der-base64" | "xfcc"
pub const DEFAULT_MAX_CERTIFICATE_BYTES: Limit;
pub fn default_account() -> Vec<Pick>;
pub struct ProxyWord;                    // provider id "coqui_access_proxy", kind Authenticator
impl ProxyWord { pub fn new(config: ProxyWordConfig) -> Result<Self, String>; }
pub fn field_of(subject: &str, field: &str) -> Option<String>;
```

## Behaviour

- `new` returns an invalid network entry verbatim as the error.
- `authenticate` looks at `Attempt::peer` (an address or `address:port`): not inside a
  trusted network gives `Unknown`. For a trusted peer the account comes from `user_header`
  (default `x-remote-user` unless another header is set), as `Lookup { by: "name" }`; else from
  `certificate_header`; else from `subject_header`. A header present more than once, in any
  case, is `Invalid`. Header names match case-insensitively over the ordered `headers` pairs.
- A certificate header longer than `max_certificate_bytes` or that does not decode is
  `Invalid`. Decoded, the leaf's fields (coqui-peer-tls `CertificateFields::to_map`) go
  through `coqui_access_kit::pick::pick(.., account)`: `Found { by, value }` is
  `Valid(Lookup { by, value })`, `Ambiguous` is `Invalid`, `Absent` falls through to the
  subject header. The proxy verified the certificate; this provider trusts it by the
  proxy's address and checks no CA.
- `pem-url` is URL-encoded PEM (leaf first); `der-base64` comma-separated base64 DER; `xfcc`
  the `Cert` (URL-encoded PEM, quoted or not) of the last comma-separated element, the one
  the nearest proxy wrote.
- A subject header becomes the fields `subject` (the whole text) and `subject.<attribute>` in
  lower case (`emailAddress` as `subject.email`), read by the same picks. No usable identity
  is `Unknown`.
- An IP address a certificate carries as a DNS name counts as an IP for a `san.ip` pick
  (`Pick::ip_in_dns`, on by default), compared in one spelling.
- IPv4-mapped IPv6 peers match IPv4 networks, and a mapped network (`::ffff:10.0.0.0/120`)
  is the IPv4 network it maps (`10.0.0.0/24`). `any` is refused: trusting every address is
  not a reverse proxy. Groups are never read from headers.
- `field_of` honours escaped commas and also reads slash-separated subjects.
