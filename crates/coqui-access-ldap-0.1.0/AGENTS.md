# `coqui-access-ldap` — API header

Header-file-style reference for the `coqui-access-ldap` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

MSRV Rust 1.89. No Cargo features. Depends on `coqui-access`, `coqui-builder` (feature `zeroize`), `guatiao-builder`, `coqui-access-kit` (feature
`cache`, for lookups), `coqui-peer-tls` (for the `ca_file` client configuration), `ldap3` (sync, rustls with ring) and `rustls`. No `unsafe` code. The loadable
library is the separate crate `coqui-access-ldap-plugin`.

```rust
pub struct LdapAccountsConfig {            // coqui_builder::Describe; the provider config is guatiao_builder::Described<LdapAccountsConfig>
    pub url: String,                      // ldaps:// or ldap:// with start_tls
    pub start_tls: bool,                  // default false
    pub cache: CacheSetting,               // `true` (default: lookups kept 5 s, "not found" 1 s), `false`, or `{ ttl, negative_ttl, max_entries }`
    pub ca_file: Option<String>,          // PEM; the only trusted roots, under Policy::default()
    pub base: String,                     // search root
    pub bind: Option<String>,             // service account DN
    pub bind_password: Option<Secret<String>>,   // coqui_builder::Secret: wiped on drop, shown as `***`; set together with bind
    pub user_filter: String,              // default "(sAMAccountName={name})"; must contain {name}
    pub object_filter: String,            // default "(&(objectCategory=person)(objectClass=user))"; one parenthesised group; ANDed into every filter lookup
    pub allow_filter_lookups: bool,       // default false; whether a lookup by "filter" is answered
    pub name_attribute: String,           // default "sAMAccountName"
    pub group_attribute: String,          // default "memberOf"; empty disables direct groups
    pub group_filter: Option<String>,     // template with {dn}
    pub group_base: Option<String>,       // default base
    pub nested_groups: bool,              // default false; AD matching rule
    pub timeout_seconds: u32,             // default 10; at least 1 is used
}
pub struct LdapAccounts;                  // provider id "coqui_access_ldap"
impl LdapAccounts {
    pub fn new(config: LdapAccountsConfig) -> Result<Self, String>;   // also TryFrom<LdapAccountsConfig> and TryFrom<Described<LdapAccountsConfig>>
    pub fn non_defaults(&self) -> &[coqui_peer_tls::NonDefault];   // what the ca_file TLS config allows past the default
    pub fn cache(&self) -> coqui_access_kit::CacheOptions;     // how lookups are kept
}
impl Authenticator for LdapAccounts { /* takes name + secret */ }
impl Directory for LdapAccounts { /* by "name" only */ }
```

## Behaviour

- `new` refuses a cleartext URL outside loopback (`ldap://` without `start_tls`), an empty
  `base`, a `user_filter` without `{name}`, and `bind` without `bind_password` or the reverse,
  and a `ca_file` that cannot be read or holds no certificate (`ca_file <path>: <reason>`).
- With a `ca_file`, the client configuration is coqui-peer-tls's under `Policy::default()`
  (intermediate: TLS 1.2 with extended master secret, and 1.3; resumption off); `non_defaults()`
  reports what it allows past that default, nothing as built. Without one, ldap3's own rustls
  defaults apply and nothing is reported.
- `authenticate`: unknown account `Unknown`; disabled account (AD `userAccountControl` bit 2),
  empty password or rejected credentials `Invalid`; success is `Lookup { by: "name", value:
  canonical name }`. Other directory failures are errors.
- `lookup` returns `Person { id: DN, name, groups }`; disabled accounts are `None`. `by: "name"` uses
  `user_filter`. `by: "filter"` is answered only when `allow_filter_lookups` is true (else
  `GUATIAO_ERR_UNSUPPORTED`); it takes an LDAP filter that is one parenthesised group (else
  `GUATIAO_ERR_BAD_VALUE`) and searches `base`, subtree, with `(&<object_filter><filter>)`: no
  account, or a disabled one, is `None`; more than one is `error::ambiguous()`. The filter is the
  caller's (a matcher's, whose values are escaped), so anything that can make a lookup `by: "filter"`
  with its own text can search the directory: turn the setting on only where filter lookups come
  from a matcher's rules.
- **`object_filter` must carry every restriction an account must meet to sign in** (people, an
  organisational unit, a group): `user_filter` is built around a name and is not applied to a filter
  lookup. The default, `(&(objectCategory=person)(objectClass=user))`, is people, which leaves out
  Active Directory's computer accounts. Any other `by` is `None`.
- Debug output redacts the URL and bind password.

## `search` module

`IN_CHAIN`, `escaped(value)` (coqui-access-kit's `escape_filter_value`: metacharacters, controls and
every byte of a non-ASCII character as `\xx`), `filter(template, field, value)`, `all_of(account_filter,
given)`, `one_group(filter)`, `group_name(dn)`, `disabled(flags)`, `in_the_clear(url, start_tls)`.

## Live test

`tests/live_directory.rs` is ignored by default. Against a disposable server set
`COQUI_TEST_LDAP=1`, `COQUI_TEST_LDAP_URL`, `_BASE`, `_BIND`, `_BIND_PASSWORD`, `_USER`,
`_PASSWORD`, `_WRONG_PASSWORD`, then `cargo test -p coqui-access-ldap --test live_directory --
--ignored`. The directory needs the user and an `operators` group containing them.
