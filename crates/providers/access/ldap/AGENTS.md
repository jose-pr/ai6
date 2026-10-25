# `ai6-access-ldap` — API header

Public API and provider contract for `ai6-access-ldap`. This header ships beside the
crate README.

## Package

MSRV: Rust 1.89. No feature flags. The same crate builds as `rlib` and `cdylib` and
contains its Guatiao entry point. It depends on AI6 with station TLS support, ldap3 with
sync and rustls-ring, rustls and serde. Unsafe code is forbidden.

## API

```rust
pub struct LdapAccountsConfig {
    pub url: String,
    pub start_tls: bool,
    pub ca_file: Option<String>,
    pub base: String,
    pub bind: Option<String>,
    pub bind_password: Option<String>,
    pub user_filter: String,
    pub name_attribute: String,
    pub group_attribute: String,
    pub group_filter: Option<String>,
    pub group_base: Option<String>,
    pub timeout_seconds: u32,
}
pub struct LdapAccounts { /* validated settings; no open connection */ }

impl LdapAccounts {
    pub fn new(config: LdapAccountsConfig) -> Result<Self, String>;
}

impl Authenticator for LdapAccounts {
    fn authenticate(&self, attempt: Attempt) -> Result<Verdict, ProviderError>;
}

impl Directory for LdapAccounts {
    fn lookup(&self, lookup: Lookup) -> Result<Option<Person>, ProviderError>;
}
```

`LdapAccounts` is provider `ai6_auth_ldap` for both provider kinds. `url` must use
`ldaps://`, or `ldap://` with StartTLS; plain loopback is allowed for throwaway tests.
Every name used in an LDAP filter is RFC 4515 escaped. Authentication binds with the
configured search account, uniquely searches the account, refuses disabled accounts,
and then binds as that account with the supplied password. Directory lookup searches
the account and reads its configured name and group attributes. `user_filter` must
contain `{name}`. Search service DN and password must be configured together. Optional
`ca_file` supplies the exclusive set of TLS roots for a private LDAP CA.

The directory is contacted only when authentication or lookup is requested. The
connection and operation timeout is at least one second. Provider errors never contain
the supplied account password. `Debug` output redacts the bind password and URL.

The `search` module exposes `IN_CHAIN`, `escaped`, `filter`, `group_name`, `disabled`,
and `in_the_clear` helpers for filter and transport policy verification.

## Live integration test

`tests/live_directory.rs` is ignored unless run explicitly against a disposable LDAP
server. Set `AI6_TEST_LDAP=1`, `AI6_TEST_LDAP_URL`, `AI6_TEST_LDAP_BASE`,
`AI6_TEST_LDAP_BIND`, `AI6_TEST_LDAP_BIND_PASSWORD`, `AI6_TEST_LDAP_USER`,
`AI6_TEST_LDAP_PASSWORD`, and `AI6_TEST_LDAP_WRONG_PASSWORD`, then run:

```text
cargo test -p ai6-access-ldap --test live_directory -- --ignored
```

The directory must contain the named user and an `operators` group with that user as a
member. The test verifies rejected and accepted passwords and resolves the user's
canonical name, DN and group through the public provider traits.
