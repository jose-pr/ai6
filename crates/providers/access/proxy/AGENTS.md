# `ai6-access-proxy` — API header

Public API and provider contract for `ai6-access-proxy`. This header ships beside
the crate README.

## Package

MSRV: Rust 1.89. No feature flags. The same crate builds as `rlib` and `cdylib` and
contains its Guatiao entry point. It depends on AI6 and serde. Unsafe code is
forbidden.

## API

```rust
pub struct ProxyWordConfig {
    pub trusted: Vec<String>,
    pub user_header: Option<String>,
    pub subject_header: Option<String>,
    pub subject_field: String,
}
pub struct ProxyWord { /* parsed network allowlist and header names */ }

impl ProxyWord {
    pub fn new(config: ProxyWordConfig) -> Result<Self, String>;
}

impl Authenticator for ProxyWord {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError>;
}

pub fn field_of(subject: &str, field: &str) -> Option<String>;
```

`ProxyWord` is provider `ai6_auth_proxy` for the `ai6_authenticator` kind. Empty
`trusted` trusts nobody. Entries are IPv4/IPv6 addresses or CIDR networks. `user_header`
defaults to `x-remote-user` unless `subject_header` is set; subject fallback reads the
first configured subject field, default `CN`. Header names are case-insensitive.

An absent or untrusted peer returns `Unknown`. A trusted peer without a usable identity
also returns `Unknown`; with one it returns a `Lookup` by account name for the configured
Directory. Only configure headers that the proxy overwrites. Groups are resolved by the
Directory and authorizer; this provider does not accept group claims from headers.

`field_of` reads a `CN=name,O=station` subject, honors escaped commas, and also accepts
slash-separated `/O=station/CN=name` subjects.
