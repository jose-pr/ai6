# Access and sign-in

Who may do what is three questions, each a Guatiao provider kind from the shared
[coqui-access](https://boriken-dev.github.io/coqui-access/) crates:

| Question | Kind | Providers |
| --- | --- | --- |
| Are these credentials good? | Authenticator | `coqui_access_password` (passwords), `coqui_access_key` (bearer keys), `coqui_access_ldap`, `coqui_access_proxy` (a trusted reverse proxy), `coqui_access_mtls` (client certificates: Station certificates, or a CA's) |
| Who is this? | Directory | `coqui_access_password`, `coqui_access_key`, `coqui_access_ldap`, `coqui_access_mtls` |
| May they do this here? | Authorizer | `coqui_roles`, or the `access.toml` policy Universal Exports reads itself |

`password`, `key` and `mtls` can also manage their own accounts (the `Accounts` kind). The
providers are libraries: point `--plugin-path` at the directory that holds them.

## Providers

Each provider takes a JSON object; every key is optional.

| Provider | Config |
| --- | --- |
| `coqui_access_password` | `source`, `max_accounts`, `argon2` (`memory_kib`, `iterations`, `parallelism`), `max_password_bytes`, `record_limits` |
| `coqui_access_key` | `source`, `prefix` (default `ck_`), `key_hash` (`{ "algorithm": "sha256" }`, or `"argon2id"` with its costs beside it), `max_keys`, `record_limits` |
| `coqui_access_mtls` | `source`, `waiting`, `max_records`, `waiting_per_source`, `waiting_total`, `waiting_rerecord_secs`, `trust` (`pinned`, `ca`, `pinned-or-ca`), `ca_file`, `subject`, `san`, `ip_in_dns`, `account`, `max_chain`, `record_limits` |
| `coqui_access_ldap` | `url`, `start_tls`, `ca_file`, `base`, `bind`, `bind_password`, `user_filter`, `name_attribute`, `group_attribute`, `group_filter`, `group_base`, `nested_groups`, `timeout_seconds`, `cache` |
| `coqui_access_proxy` | `trusted`, `user_header`, `certificate_header`, `certificate_encoding` (`pem-url`, `der-base64`, `xfcc`), `subject_header`, `account`, `max_certificate_bytes` |
| `coqui_roles` | `scope_kinds`, `roles`, `rules`, `limits` |

`source` says where records live: `{ "file": { "path": "accounts.toml", "format": "toml" } }`,
`format` being `toml` or `json`. With no `source` they live in memory and are gone at exit.

Universal Exports keeps what its Directory and Authorizer answer for a few seconds, to absorb
bursts: a person found or an action granted 5 s, "no such person" or a denial 1 s, at most
10 000 of each (the `unex` record's `access_cache_ttl`, `access_cache_negative_ttl` and
`access_cache_entries`, [Configuration](configuration.md); `0` turns one off). A removed account or a withdrawn
grant can pass for up to the first; passwords and keys are always checked at the provider.
A longer lifetime is reported at start.

## The access policy

`access.toml` binds people to roles at a scope. A scope is `*` or `station:<id>`,
`operation:<id>`, `theater:<id>`, `config:roster` (the roster every Station reads) or
`config:assistant` (Moneypenny's repositories); the
first rule that matches decides, and no match denies. The actions Universal Exports checks
are `agent.configure`, `agent.read`, `approval.class`, `approval.decide`,
`assistant.memory.admin`, `audit.read`,
`cable.read`, `cable.send`, `mission.control`, `mission.create`, `mission.read`,
`pool.manage`, `pool.read`,
`roster.manage`, `roster.read`, `session.input`, `session.read`, `station.configure` and
`station.read`. `roster.read` and `roster.manage` at `config:roster` list and edit the
roster's Operations, Agents, roles and covers ([Changing the configuration of running
Stations](operations.md#changing-the-configuration-of-running-stations)); `audit.read` there
shows those edits and the refused ones. `pool.read` shows the [account pools](agents.md#account-pools) an Operation
may draw from, never a credential; `pool.manage` lifts an account's limit. `mission.control`
(cancel, retry, resume and interrupt a Mission) is separate from `session.input`: typing into
a Session does not grant stopping it. `approval.decide` signs or refuses an Agent's request
for [green ink](missions.md#green-ink) and `approval.class` manages class authorisations;
neither comes with `mission.create` or `mission.control`. `cable.send` and `cable.read` on
`operation:<id>` are what [Moneypenny](assistant.md#cables) may do with Cables there for
the person: send and reply, and read what is sent to her. The Station's `cables.toml` still
has to let her Agent do it. A person reads and changes [Moneypenny's memory of
them](assistant.md#your-memory-in-the-app) with no action at all, and nobody else's;
`assistant.memory.admin` on `config:assistant` lets an administrator read anyone's, each read
audited `assistant.memory.read`, and change none.

```toml
[role.viewer]
actions = ["mission.read", "session.read"]

[role.memory-admin]
actions = ["assistant.memory.admin"]

[[rule]]
subject = "group:field"
scope = "operation:G"
role = "viewer"

[[rule]]
subject = "group:security"
scope = "config:assistant"
role = "memory-admin"
```

## Login limits

Universal Exports counts every password or key try against the client address and against
the account name before an Authenticator is asked. A try past either count is refused with
`429 Too Many Requests` and a `Retry-After`, even if its secret is right. The defaults are 5 a
minute per address and 10 in ten minutes per name; the `unex` record's
`login_tries_per_address`, `login_tries_per_name` and `client_address_header` change them. The gateway listens on
loopback behind a reverse proxy, so name the header the proxy fills with the client address
(`x-forwarded-for` or `x-real-ip`) or all clients share one count.
