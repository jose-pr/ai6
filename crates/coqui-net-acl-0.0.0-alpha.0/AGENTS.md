# `coqui-net-acl` — API header

Header-file-style reference for the `coqui-net-acl` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-net-acl>.

| Feature | Adds | Needed for | Default |
| --- | --- | --- | --- |
| `serde` | `serde` (`derive`, `std`): `Serialize`/`Deserialize` for `Acl`, `Rule`, `Action`, `IpNet`, `PortRange` | the text form | off |
| `describe` | `coqui-builder` (`derive`, no default features): `Describe` for `Acl` and `Rule`, `Leaf` for `Action` | reading a list into a `coqui_builder::Partial`, and `coqui_builder::non_defaults` | off |

MSRV Rust 1.89. A default build has no dependencies. Version `0.0.0-alpha.0` is a pre-release;
the API may change in any release before 1.0.

Everything is at the crate root, `coqui_net_acl::*`; there are no public modules.

No `unsafe` code (`#![forbid(unsafe_code)]`), and no process-wide state. The crate decides
nothing about what is public or private: a host chooses what to listen on and writes the rules.

## Values (`coqui_net_acl`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Action { Permit, Deny }                         // Default: Deny; Display: "permit" / "deny"

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IpNet { Any, V4(u32, u8), V6(u128, u8) }        // Default: Any; address bits, prefix length
impl IpNet { pub fn contains(&self, address: IpAddr) -> bool; }
impl FromStr for IpNet { type Err = ParseError; }        // "any", "10.0.0.1", "10.0.0.0/8", "::1", "fd00::/8"
impl Display for IpNet {}                                 // "any", "10.0.0.1" (a /32 or /128 has no suffix), "10.0.0.0/8"

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortRange { pub start: u16, pub end: u16 }    // inclusive
impl FromStr for PortRange { type Err = ParseError; }    // "443", "8000-8100" (spaces around '-' allowed)
impl Display for PortRange {}                            // "443", "8000-8100"

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Rule {
    pub action: Action,
    pub source: IpNet,                  // the connecting address
    pub destination: Option<IpNet>,     // the local address connected to; None = any
    pub port: Option<PortRange>,        // the local port; None = any
    pub fingerprint: Option<String>,    // canonical peer fingerprint text; decided after the handshake
    pub log: bool,                      // a hint returned in the decision
}
impl Rule { pub fn new(action: Action, source: IpNet) -> Rule; }   // no destination, port, fingerprint; log false

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Acl { pub rules: Vec<Rule>, pub default: Action }    // Default: no rules, Deny
```

- `IpNet::contains` is true for `Any`, and for an address of the same family whose first
  `prefix` bits equal the network's; a variant built with a prefix above the family's width
  (`IpNet::V4(bits, 40)`) matches as the full width, a single host. An IPv4-mapped IPv6 address (`::ffff:10.0.0.9`) is the IPv4
  address it maps, in rules and in connections: `"::ffff:10.0.0.0/120"` parses to
  `IpNet::V4(10.0.0.0, 24)`.
- `IpNet` parsing refuses a prefix above 32 (IPv4) or 128 (IPv6), and anything that is not an
  address, a CIDR network or `any` (any case). `PortRange` parsing refuses a port above 65535 and
  a start after its end.
- Host bits beyond the prefix are kept as written: `"10.0.0.5/8"` parses, displays as
  `10.0.0.5/8`, matches `10.0.0.0/8`'s addresses, and is not `==` to `"10.0.0.0/8"`.

## Evaluation (`coqui_net_acl::Acl`)

```rust
impl Acl {
    pub fn push(&mut self, rule: Rule);          // append, before the default
    pub fn prepend(&mut self, rule: Rule);       // insert at the top
    pub fn decide(&self, source: SocketAddr, local: SocketAddr) -> AclDecision;
    pub fn decide_peer(&self, source: SocketAddr, local: SocketAddr, fingerprint: &str) -> AclDecision;
    pub fn decide_anonymous(&self, source: SocketAddr, local: SocketAddr) -> AclDecision;
    pub fn canonicalize_fingerprints<E>(&mut self, canonical: impl FnMut(usize, &str) -> Result<String, E>)
        -> Result<(), E>;
    pub fn permits_bind(&self, local: SocketAddr) -> BindDecision;
}
impl Display for Acl {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Outcome { Permit, Deny, Deferred }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AclDecision { pub outcome: Outcome, pub rule_index: Option<usize>, pub log: bool }
impl AclDecision { pub fn proceeds(&self) -> bool; }          // outcome != Deny

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindDecision { pub permitted: bool, pub rule_index: Option<usize> }
```

- Rules are read top to bottom; the first whose `source`, `destination` and `port` match
  decides, with `rule_index: Some(index)` and that rule's `log`. With none, `default` applies:
  `rule_index: None`, `log: false`.
- `decide` is for accept time, before any byte is read. A first-matching rule that names a
  `fingerprint` cannot be decided yet: `Outcome::Deferred`, and the connection may proceed to
  the handshake (`proceeds()` is true).
- `decide_peer` finishes the decision with the verified fingerprint, compared as text,
  ignoring ASCII case, against each rule's (two spellings of one fingerprint do not match); a rule with a fingerprint matches only that
  fingerprint, and one without matches any. Never `Deferred`.
- `decide_anonymous` decides a peer that proves no identity (plaintext, or TLS without a
  client certificate) by address alone: rules that name a fingerprint are passed over, the
  first other matching rule decides, then the default. Never `Deferred`.
- `canonicalize_fingerprints` rewrites each rule's fingerprint into the text `canonical`
  returns (it gets the rule index and the text as written), so rules in any spelling the host
  parses match the canonical fingerprint it passes to `decide_peer`. All or nothing: on the
  first `Err` the list is unchanged and that error is returned. The crate knows no
  fingerprint format; `coqui-peer-tls` does this with `PeerId` when it builds a policy's ACL.
- `permits_bind` is advisory and enforces nothing. Walking the rules whose `destination` and
  `port` cover `local` (a wildcard address, `0.0.0.0` or `::`, is covered by every rule): a
  `Permit` permits it whatever its source (a routed setup may listen on an address that clients
  on other networks reach), a `Deny` with source `any` and no fingerprint refuses it, and any
  other rule is passed over. With none, `permitted` is `default == Permit` and `rule_index`
  `None`.
- `Display` prints a router-style listing, one rule per line numbered in steps of ten, then the
  default: `10 permit 10.6.0.0/16 destination 10.6.20.5 port 7000-7100 fingerprint sha256:ab
  log`, `default deny`, each line ending in `\n`.

## Text form (`serde` feature)

```toml
default = "deny"            # optional; "permit" or "deny"; default "deny"

[[rule]]
action = "permit"           # required
source = "10.9.0.0/16"      # optional; default "any"
destination = "10.6.20.5"   # optional
port = "7000-7100"          # optional
fingerprint = "sha256:..."  # optional
log = true                  # optional; default false
```

`Acl`, `Rule`, `IpNet` and `PortRange` serialize as above, any serde format; absent optional
keys are skipped when written. Unknown keys are errors, and so are networks or ports that do
not parse. Inside a
`coqui-peer-tls` `Policy` the same table is `[acl]` with `[[acl.rule]]` entries.

## Description (`describe` feature)

`Acl` and `Rule` derive `coqui_builder::Describe` and `Action` derives `coqui_builder::Leaf` (`permit` / `deny`), so a host
reads a list with `toml::from_str::<Partial<Acl>>(text)?.build()` (coqui-builder's `serde` feature) or fills it key by key
(`rule[0].action`, `default`), and describes it with `Acl::description()`. The document is the one above: `default`,
then the rules under the key `rule`; `Rule`'s `source`, `destination` and `port` are their text forms, and every key of a rule
but `action` is optional. Unknown keys are refused. `IpNet` and `PortRange` carry no impl of their own: they cross as text
wherever a field holds them (`#[map(as_text)]`).

The derive also writes two public setter traits, `AclSetters` (`rules`, `default`) and `RuleSetters` (`action`, `source`,
`destination`, `port`, `fingerprint`, `log`), one `fn field(self, impl Into<FieldType>) -> Self` per setting (`log` takes a `bool` itself), for a
builder that holds an `Acl` or a `Rule` (`coqui_builder::builder::Holds`). A setter takes the Rust field name; the key of
`rules` is `rule`.

What differs from the defaults is `coqui_builder::non_defaults(&acl)`: a `permit` default is the line `default`
(`Action(permit)` against `Action(deny)`), and a list with rules lists each as one place, `rule[0]`, `rule[1]`.


## Errors

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError;          // opaque; Display + core::error::Error
```

Returned by `IpNet::from_str` and `PortRange::from_str`. `Display` says what was expected (for
example "not a port or a port range") and never quotes the input.

## Gotchas

- `Outcome` is `#[non_exhaustive]`: a `match` on it needs a `_` arm.
- Fingerprints are compared as text: a rule written as grouped base32 never matches a
  `sha256:<hex>` fingerprint until `canonicalize_fingerprints` rewrites it.
- A fingerprint rule is only decided by `decide_peer`; calling `decide` alone leaves it
  `Deferred`, which `proceeds()`. A host must call `decide_peer` after the handshake.
- `permits_bind` passes over a narrower `Deny` (a source other than `any`, or a fingerprint):
  it answers whether anyone could be permitted, not whether everyone is.
