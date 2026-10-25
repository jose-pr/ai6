# `ai6-access-local` — API header

Header-file-style reference for the `ai6-access-local` crate. It ships beside the
crate README and describes its public Rust API and provider contract.

## Package

MSRV: Rust 1.89. Features: none. The crate builds as `rlib` and `cdylib` from the
same source. It depends on AI6, Argon2, serde, TOML and UUID. It forbids unsafe code.

## API

```rust
pub struct LocalAccountsConfig { pub path: String }
pub struct LocalAccounts { /* account file snapshot and dummy password hash */ }

impl LocalAccounts {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String>;
}

impl Authenticator for LocalAccounts {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError>;
}

impl Directory for LocalAccounts {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError>;
}

pub fn hash_password(password: &str) -> Result<String, String>;
```

`LocalAccountsConfig` is the Guatiao provider config, and `LocalAccounts` is provider
`ai6_auth_local` for both `ai6_authenticator` and `ai6_directory`. `hash_password`
returns an Argon2id PHC string with a fresh random salt. Passwords outside 1–4096
bytes are rejected.

The TOML document contains `[[accounts]]` records with `id`, `name`, `password_hash`,
optional `display_name`, `email`, `groups` and `disabled`. The maximum file size is
1 MiB and the maximum account count is 10,000. IDs and names are unique. Names match
exactly; lookup supports `id`, `name` and `email`. Disabled accounts cannot
authenticate or resolve through the Directory. Unknown names are verified against a
dummy Argon2id hash to reduce account-enumeration timing differences.

The file is read at provider construction and reloaded when its modification time or
length changes. A missing file is an empty account list. Malformed, unreadable or
oversized files are rejected and do not replace the last valid snapshot.

## Debug and errors

`LocalAccounts` reports only the configured path and account count. Account names,
emails, IDs, password hashes and credentials are omitted from its debug output. The
Guatiao provider returns fixed errors for invalid or unreadable account files.
