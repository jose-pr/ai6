# coqui-access-ldap

[![Version](https://img.shields.io/badge/version-0.1.0--dev-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md#unreleased)
[![Rust version](https://img.shields.io/badge/rust-1.89%2B-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/Cargo.toml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://boriken-dev.github.io/coqui-access/rust/coqui_access_ldap/)
[![CI](https://img.shields.io/github/actions/workflow/status/Boriken-Dev/coqui-access/test.yml)](https://github.com/Boriken-Dev/coqui-access/actions/workflows/test.yml)

**Authenticates against LDAP or Active Directory** and uses the same directory for names and
groups. A password is sent only over TLS, StartTLS, a sealed Kerberos connection or loopback
unless the operator allows otherwise; search filters escape user-supplied values. No connection is made until a request is checked.

## Features

- **Search, then bind** — finds the account with a service account, refuses disabled ones,
  then binds as the account with the supplied password.
- **Groups** — from `memberOf`, from a group search, or nested with Active Directory's
  matching rule; an Active Directory account's primary group (`primaryGroupID`, normally
  Domain Users), which `memberOf` leaves out, is added.
- **Private CA** — an optional PEM file as the only trusted roots.
- **Bind methods** — the search account binds with a password, Kerberos (`gssapi`), NTLM
  (`ntlm`) or a TLS client certificate or `ldapi://` socket (`external`); a person's password is
  checked by a simple bind or by NTLM.

## Installation

```bash
cargo add coqui-access-ldap --git https://github.com/Boriken-Dev/coqui-access
```

Build `coqui-access-ldap-plugin` for the loadable library.

## Quick start

Provider id `coqui_access_ldap`, configuration:

```toml
url = "ldaps://directory.example.org"
base = "DC=example,DC=org"
bind = "CN=search,DC=example,DC=org"
bind_password = "..."
# user_filter = "(sAMAccountName={name})"
# nested_groups = true
```

## Bind methods

```toml
# Kerberos: no stored password; the process's own credentials (SSPI on Windows, the ticket
# cache or keytab libgssapi finds on Unix). Sealed over plain ldap://.
url = "ldap://dc.example.org"
bind_method = "gssapi"
# server_fqdn = "dc.example.org"   # the name of ldap/<name>; the URL's host by default

# A client certificate: no stored password.
# url = "ldaps://directory.example.org"
# bind_method = "external"
# ca_file = "ca.pem"
# client_cert = "client.pem"
# client_key = "client.key"
```

`gssapi` and `ntlm` are Cargo features, off by default; on Unix `gssapi` links `libgssapi_krb5` and
needs the Kerberos development files and libclang to build. A password over plain `ldap://` to
another machine is refused unless `allow_cleartext_password` is on, which `non_defaults()` reports.
[Binding to a directory](https://boriken-dev.github.io/coqui-access/ldap/) has each method's
requirements and costs.

## API overview

| Item | Purpose |
| --- | --- |
| `LdapAccounts` | The provider: `new(config)`, `Authenticator` and `Directory`. |
| `LdapAccountsConfig` | The Guatiao configuration. |
| `BindMethod`, `UserBind` | How the search account and a person bind. |
| `NonDefault` | A setting that admits more than the defaults do: a TLS option past the default policy, a clear-text password, NTLM without TLS. |
| `search` | Filter and result helpers that need no directory. |

The live tests in `tests/live_directory.rs` and `tests/live_domain.rs` are ignored unless run against
a disposable directory or a domain controller, and `tests/container_directory.rs` builds a Kerberos and
OpenLDAP container when `COQUI_TEST_CONTAINER` names podman or docker; see `AGENTS.md`.

## Development

```bash
cargo test -p coqui-access-ldap --all-features
cargo clippy -p coqui-access-ldap --all-targets --all-features -- -D warnings
```

### Releasing

This project follows [Semantic Versioning](https://semver.org/) and keeps a
[`CHANGELOG.md`](https://github.com/Boriken-Dev/coqui-access/blob/main/CHANGELOG.md).
Pushing a tag matching `v*` triggers the release workflow of the repository.

## License

MPL-2.0 — see [LICENSE](https://github.com/Boriken-Dev/coqui-access/blob/main/LICENSE).
