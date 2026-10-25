# `ai6-access-peer` — API header

Public API and provider contract for `ai6-access-peer`. This header ships beside the
crate README.

## Package

MSRV: Rust 1.89. No feature flags. The same crate builds as `rlib` and `cdylib` and
contains its Guatiao entry point. It depends on AI6 with `station-tls`, serde and TOML.
Unsafe code is forbidden.

## API

```rust
pub struct PeersConfig { pub path: String }
pub struct Peers { /* verified StationFingerprint to Directory identity mapping */ }

impl Peers {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String>;
}

impl Authenticator for Peers {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError>;
}

impl Directory for Peers {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError>;
}
```

`Peers` is provider `ai6_auth_peer` for both `ai6_authenticator` and `ai6_directory`.
The TOML file contains `[[peers]]` records with `id`, `name`, `certificate` (the canonical
`StationFingerprint`), optional `email`, `groups` and `disabled`. The file is bounded to 1 MiB and
10,000 records. It reloads valid file changes.

This provider maps an already verified TLS client certificate to a Directory identity.
It does not verify TLS or decide whether the StationFingerprint is pinned; establish those
transport checks with AI6 `PeerTrustStore` before exposing the ID as `Attempt.certificate`.
Disabled records cannot authenticate or resolve through the Directory.

Debug output includes only the configured path and record count. Certificate IDs and
person fields are redacted.
