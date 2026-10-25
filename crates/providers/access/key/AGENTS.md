# `ai6-access-key` — API header

Public API and provider contract for the `ai6-access-key` crate. This header ships
beside the crate README.

## Package

MSRV: Rust 1.89. No feature flags. The same crate builds as `rlib` and `cdylib` and
contains its Guatiao entry point. It depends on AI6, serde, SHA-256, constant-time
comparison, TOML and UUID. Unsafe code is forbidden.

## API

```rust
pub struct KeysConfig { pub path: String }
pub struct Keys { /* hashes and account metadata; no bearer secrets */ }

impl Keys {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String>;
}

impl Authenticator for Keys {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError>;
}

impl Directory for Keys {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError>;
}

pub const PREFIX: &str = "ai6_";
pub fn key() -> Result<String, String>;
pub fn hash_of(key: &str) -> String;
```

`Keys` is provider `ai6_auth_key` for both `ai6_authenticator` and `ai6_directory`.
`key` returns a one-time 256-bit random bearer secret; persist only `hash_of(key)`.
It returns an error if the operating system random source is unavailable.
The TOML file stores `[[keys]]` records with `id`, `name`, `hash`, optional `email`,
`groups` and `disabled`. File size is capped at 1 MiB and the table at 10,000 entries.
The provider reloads a valid file when its modification time or length changes.

Attempts without the `ai6_` prefix are unknown. A prefixed key that is not in the
table is unknown; a matching disabled key is invalid. Matching hashes use constant-
time comparison. Directory lookups accept `id`, `name` and `email`; disabled keys do
not resolve.

`Keys` debug output includes only the configured path and record count. It omits key
hashes and account fields.
