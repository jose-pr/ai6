# `coqui-access-ldap` — API header

Header-file-style reference for the `coqui-access-ldap` crate: every public item with its
signature, contract and gotchas, so the crate can be used without reading its source. It
ships inside the package and is self-contained. Development documentation lives with the
source at <https://github.com/Boriken-Dev/coqui-access>.

MSRV Rust 1.89. Cargo features (off by default): `gssapi` (Kerberos binds; Unix links the system `libgssapi_krb5` and
builds its bindings with bindgen, so the build needs the Kerberos development files, e.g. `krb5-devel`, and libclang;
Windows uses SSPI and needs nothing) and `ntlm` (pure Rust; holds `picky` at `=7.0.0-rc.17` and `picky-krb` at
`=0.11.1`, the last that `sspi` 0.16.1 compiles with). Depends on `coqui-access`, `coqui-builder` (feature `zeroize`), `guatiao-builder`, `coqui-access-kit` (feature
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
    pub bind_method: String,              // default "simple"; "gssapi" | "ntlm" | "external" (see Bind methods)
    pub server_fqdn: Option<String>,      // gssapi: the name of ldap/<name>; default the URL's host, which must not be an address
    pub client_cert: Option<String>,      // PEM chain presented to the directory; with client_key; needs ldaps/start_tls and ca_file
    pub client_key: Option<String>,       // PEM key of client_cert
    pub user_bind: String,                // default "simple"; "ntlm" checks a person by NTLM
    pub allow_cleartext_password: bool,   // default false; lifts the refusal of a password over plain ldap://
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
    pub fn non_defaults(&self) -> &[NonDefault];               // what the settings admit past the defaults, as data; empty by default
    pub fn cache(&self) -> coqui_access_kit::CacheOptions;     // how lookups are kept
    pub fn bind_method(&self) -> BindMethod;                   // the search account's method
    pub fn user_bind(&self) -> UserBind;                       // how a person's password is checked
}
pub enum BindMethod { Simple, Gssapi, Ntlm, External }   // #[non_exhaustive]; parse(&str), as_str(), feature(), built()
pub enum UserBind { Simple, Ntlm }                        // #[non_exhaustive]; parse, as_str, built
pub enum NonDefault { Tls(coqui_peer_tls::NonDefault), CleartextPassword, UnprotectedNtlm } // #[non_exhaustive]; Display
// module bind: ntlm_name(&str) -> Result<(), String>, url_host(&str), is_address(&str), is_socket(&str)
impl Authenticator for LdapAccounts { /* takes name + secret */ }
impl Directory for LdapAccounts { /* by "name", "id" and (when allowed) "filter" */ }
```

## Behaviour

- `new` refuses a URL a password would cross in the clear (`ldap://` outside loopback without `start_tls`) unless
  `allow_cleartext_password`, an empty `base`, a `user_filter` without `{name}`, and `bind` without `bind_password` or the
  reverse, and a `ca_file` that cannot be read or holds no certificate (`ca_file <path>: <reason>`). Other values that cannot
  work are errors too: an unknown `bind_method` or `user_bind` (the error lists the choices), a method whose feature is not built
  (names the feature), `bind`/`bind_password` with `gssapi` or `external`, `ntlm` without both or with a name that is not
  `user`, `DOMAIN\user` or `user@domain` (ldap3 would panic on it), `gssapi` with a URL that names an address and no `server_fqdn`,
  `external` without `client_cert` and `client_key` over TLS or an `ldapi://` URL, `client_cert` without `client_key` or
  without TLS and `ca_file`.
- With a `ca_file`, the client configuration is coqui-peer-tls's under `Policy::default()`
  (intermediate: TLS 1.2 with extended master secret, and 1.3; resumption off); `non_defaults()`
  reports what it allows past that default as `Tls(..)`, nothing as built. Without one, ldap3's own rustls
  defaults apply and nothing is reported.
- **Bind methods.** The search account (`bind_method`): `simple` (`bind` + `bind_password`, or neither: anonymous), `gssapi` (SASL
  GSSAPI as the process's own credentials; the provider takes no keytab or ticket-cache path because libgssapi reads them from the
  environment: `KRB5CCNAME`, `KRB5_CLIENT_KTNAME`, `KRB5_CONFIG`; Windows uses the process's account; over plain `ldap://` the
  connection is sealed and the bind blocks on the KDC; over TLS the certificate hash is sent as the channel binding), `ntlm`
  (`bind` as `DOMAIN\user` or `user@domain`, SASL GSS-SPNEGO; cannot seal) and `external` (SASL EXTERNAL: the TLS client certificate
  or an `ldapi://` socket's peer credentials). A person (`user_bind`): `simple` (their DN, on the search account's connection, so a
  sealed layer carries the password) or `ntlm` (the name typed, on a connection of its own; a name `ntlm_name` refuses is `Invalid`).
- **Other `non_defaults()` items.** `CleartextPassword` when `allow_cleartext_password` is on, `UnprotectedNtlm` when NTLM binds with no TLS
  (`ldap://` without `start_tls`). Nothing is logged. A password is "in the clear" when a simple search-account bind carries one,
  or a person is checked by a simple bind on a connection that is not sealed; a `gssapi` search account seals plain `ldap://`.
- **Server side.** `gssapi` needs the service principal `ldap/<server_fqdn>` and a client with a ticket or keytab; OpenLDAP also needs
  `authz-regexp` rules from the principal to an entry. `external` needs the certificate subject (or the socket's peer uid) mapped to
  an entry. A server that requires signing (Active Directory's default for simple binds without TLS, or `security simple_bind=128`)
  answers a plain simple bind with `confidentiality required` (13); `gssapi` is what passes.
- `authenticate`: unknown account `Unknown`; disabled account (AD `userAccountControl` bit 2),
  empty password or rejected credentials `Invalid`; success is `Lookup { by: "name", value:
  canonical name }`. Other directory failures are errors.
- `lookup` returns `Person { id: DN, name, display_name, email, groups }`; disabled accounts are `None`. `by: "name"` uses
  `user_filter`. `by: "id"` takes the DN a lookup returned and reads that one entry (base scope) under
  `object_filter`; a DN not strictly below `base`, a missing one or a disabled one is `None`, and nothing
  outside `base` is read. `display_name` is `displayName` when set, else `givenName`, `initials` and `sn`
  joined by spaces (those present; `None` when none); `email` is `mail`. `by: "filter"` is answered only when `allow_filter_lookups` is true (else
  `GUATIAO_ERR_UNSUPPORTED`); it takes an LDAP filter that is one parenthesised group (else
  `GUATIAO_ERR_BAD_VALUE`) and searches `base`, subtree, with `(&<object_filter><filter>)`: no
  account, or a disabled one, is `None`; more than one is `error::ambiguous()`. The filter is the
  caller's (a matcher's, whose values are escaped), so anything that can make a lookup `by: "filter"`
  with its own text can search the directory: turn the setting on only where filter lookups come
  from a matcher's rules.
- **The primary group.** Active Directory leaves an account's primary group out of `memberOf`. When the account has a numeric
  `primaryGroupID` and a binary `objectSid`, the group is the account's domain SID (its `objectSid` without the last RID)
  plus that RID, searched as `(&(objectClass=group)(objectSid=<bytes, each \xx>))` under `group_base` (else `base`) and
  named as the other groups are; with `nested_groups` on, the groups it is in follow (`group_filter`, else the matching
  rule). Resolutions are kept per group SID, so per domain, as long as the lookup `cache` keeps lookups (found for `ttl`, not
  found for `negative_ttl`, at most `max_entries`). A missing or malformed `primaryGroupID` or `objectSid`, no such group or
  several, or a directory error on that search adds nothing and refuses nothing. "Everyone" and "Authenticated Users" are
  not synthesised.
- **`object_filter` must carry every restriction an account must meet to sign in** (people, an
  organisational unit, a group): `user_filter` is built around a name and is not applied to a filter
  lookup. The default, `(&(objectCategory=person)(objectClass=user))`, is people, which leaves out
  Active Directory's computer accounts. Any other `by` is `None`.
- Debug output redacts the URL and bind password.

## `search` module

`IN_CHAIN`, `below(dn, base)`, `display_name(display, given, initials, surname)`, `escaped(value)` (coqui-access-kit's `escape_filter_value`: metacharacters, controls and
every byte of a non-ASCII character as `\xx`), `filter(template, field, value)`, `all_of(account_filter,
given)`, `one_group(filter)`, `group_name(dn)`, `disabled(flags)`, `in_the_clear(url, start_tls)`.

## Tests

`tests/container_directory.rs` builds `tests/container/` (Fedora, MIT KDC and OpenLDAP, packages installed inside the image) when
`COQUI_TEST_CONTAINER=podman` (or `docker`) and binds `simple`, `external` (ldaps, StartTLS, `ldapi://`) and, with `--features
gssapi` on Unix, `gssapi`; it sets `KRB5_*` in the process, so it is the only test of its binary. Rootless containers die with
the `wsl.exe` session that started them: run the whole `cargo test` in one session. Without the variable it passes without running.
`tests/live_domain.rs` (`--features gssapi,ntlm`, `--ignored`) binds to a domain controller read-only: `COQUI_TEST_DC=1`,
`COQUI_TEST_DC_URL`, `_BASE`, `_USER`, optional `_CA` and `_START_TLS=1`; its NTLM test also needs `COQUI_TEST_DC_NTLM_PASSWORD`
and `COQUI_TEST_DC_NTLM_WRONG_PASSWORD` (a wrong password counts against the account's lockout).

### Live test

`tests/live_directory.rs` is ignored by default. Against a disposable server set
`COQUI_TEST_LDAP=1`, `COQUI_TEST_LDAP_URL`, `_BASE`, `_BIND`, `_BIND_PASSWORD`, `_USER`,
`_PASSWORD`, `_WRONG_PASSWORD`, then `cargo test -p coqui-access-ldap --test live_directory --
--ignored`. The directory needs the user and an `operators` group containing them.
