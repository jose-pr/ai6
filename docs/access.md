# Access and sign-in

Who may do what is three questions, each a Guatiao provider kind from the shared
[coqui-access](https://boriken-dev.github.io/coqui-access/) crates:

| Question | Kind | Providers |
| --- | --- | --- |
| Are these credentials good? | Authenticator | `coqui_access_local` (passwords), `coqui_access_key` (bearer keys), `coqui_access_ldap`, `coqui_access_proxy` (a trusted reverse proxy), `coqui_access_peer` (Station certificates) |
| Who is this? | Directory | `coqui_access_local`, `coqui_access_key`, `coqui_access_ldap`, `coqui_access_peer` |
| May they do this here? | Authorizer | `coqui_roles`, or the `access.toml` policy Universal Exports reads itself |

`local`, `key` and `peer` can also manage their own accounts (the `Accounts` kind). The
providers are libraries: point `--plugin-path` at the directory that holds them.

## Providers

Each provider takes a JSON object; every key is optional.

| Provider | Config |
| --- | --- |
| `coqui_access_local` | `source`, `max_accounts`, `argon2` (`memory_kib`, `iterations`, `parallelism`) |
| `coqui_access_key` | `source`, `prefix` (default `ck_`), `key_hash` (`sha256` or `argon2id`), `argon2`, `max_keys` |
| `coqui_access_peer` | `source`, `waiting`, `max_peers`, `waiting_places`, `seen_every_secs` |
| `coqui_access_ldap` | `url`, `start_tls`, `ca_file`, `base`, `bind`, `bind_password`, `user_filter`, `name_attribute`, `group_attribute`, `group_filter`, `group_base`, `nested_groups`, `timeout_seconds` |
| `coqui_access_proxy` | `trusted`, `user_header`, `subject_header`, `subject_field` |
| `coqui_roles` | `scope_kinds`, `roles`, `rules` |

`source` says where records live: `{ "file": { "path": "accounts.toml", "format": "toml" } }`.
With no `source` they live in memory and are gone at exit.

## The access policy

`access.toml` binds people to roles at a scope. A scope is `*` or `station:<id>`,
`operation:<id>`, `theater:<id>` or `config:roster` (the roster every Station reads); the
first rule that matches decides, and no match denies. The actions Universal Exports checks
are `agent.configure`, `agent.read`, `approval.class`, `approval.decide`, `audit.read`,
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
has to let her Agent do it.

```toml
[role.viewer]
actions = ["mission.read", "session.read"]

[[rule]]
subject = "group:field"
scope = "operation:G"
role = "viewer"
```

## Login limits

Universal Exports counts every password or key try against the client address and against
the account name before an Authenticator is asked. A try past either count is refused with
`429 Too Many Requests` and a `Retry-After`, even if its secret is right. The defaults are 5 a
minute per address and 10 in ten minutes per name; `--login-tries-per-address`,
`--login-tries-per-name` and `--client-address-header` change them. The gateway listens on
loopback behind a reverse proxy, so name the header the proxy fills with the client address
(`x-forwarded-for` or `x-real-ip`) or all clients share one count.
