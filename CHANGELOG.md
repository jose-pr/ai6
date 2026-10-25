# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- A Station's identity defaults to `<host>-<random>`, created on first start and kept in
  `station-id` in the AI6 state directory, so a restarted daemon stays the same Station;
  `--station` or `AI6_STATION_ID` override it.
- Theaters: a Station reads `theaters.toml` (`[theater.<id>]` with `access = "local"` or an
  `ssh` host, plus labels), always reaches its own host as the implicit `local` Theater and
  publishes its Theaters at check-in. A Session's launch record carries `station_id` and
  `theater_id` separately (older records still load). `GET /v1/stations` lists each
  Station's Theaters, and Moneypenny shows them and the Station and Theater of each Session.
  An SSH Theater is recorded and shown but not yet chosen for Sessions.
- Placement is now M's decision (`ai6::directorates::m::placement`, extracted from the
  Station scheduler). It filters by Station role, health, capacity, labels, checkout, Theater
  and available handler, orders by load then Agent priority, and gives a reason for every
  rejection. An Operation may list the Stations allowed to run it (`stations = [..]`); a
  Mission's brief may narrow the list but never widen it. Stations also publish their
  handlers, Theaters and each Operation's placement policy at check-in.
- Universal Exports `GET /v1/user/operations/{id}/placement-preview` (needs `mission.read`)
  returns the eligible Station, Theater and Agent choices and the reason for each rejection,
  and Moneypenny's Stations view has a placement preview panel.
- Recovery. Every Session now has a record in the shared Registry (`sessions/`): AI6 Session
  ID, provider Session ID, Station, Theater, attempt and predecessor, status. A Station acts
  on a Session only while it holds the record's lease and presents its fencing token, which
  is drawn from the new Headquarters election term (`HeadquartersLease::term`) and always
  grows; a stale token is refused and the Station closes its harness and abandons the
  Mission. Records and leases from before this change still load. Moving a Session to
  another Station or Theater is not implemented.
- Provider usage limits. The Claude and Codex handlers report a usage or rate limit, with
  its reset time when given, as an `AgentEvent` of kind `limit` (`AgentEvent::limit`,
  `UsageLimit`, `LIMIT_EVENT`). The Mission becomes `parked` with `parked_until` and the same
  provider Session resumes when the quota resets (also across a daemon restart). An
  Operation sets `on_limit = "wait"` (default) or `"backup"` (post the next qualified Agent
  on another handler as a new attempt in the same safe house) and `limit_fallback_secs`
  (default 1800, used when no reset time is reported). `parked` is a Session state in
  `ai6-diptel`, the gateway and Moneypenny.

- `ai6`: the Station daemon and AI6's public contracts in one Rust crate. Directorates
  under `ai6::directorates` (M, the Registry, Q Branch, 00 Section, Medical) and the Station
  runtime under `ai6::station` (server, provider host, safe houses, publisher, identity).
  The `ai6` command provides `serve`, `identity` and `peer waiting|accept`.
- Station daemon: a private loopback API for providers and the Mission lifecycle;
  Registry-leased intake with lease renewal and recovery; Mission records persisted in
  `missions.json` and reattached to provider Sessions after a restart; Station check-ins
  with roles (`control`, `execution`), labels and capacity; Headquarters Station election.
- Agents and roles: an Agent (`[agent.<codename>]`) owns its handler, model and effort and
  lists the roles it is qualified for; a role (`[role.<id>]`) is a purpose and a policy.
  M posts the highest-priority qualified Agent whose handler is available, so the next
  qualified Agent is the backup. Operators can override roles per Operation.
- The Station picks the terminal provider automatically, preferring `pty` when several
  are available; `ai6 serve --terminal ID` pins one. Agents do not choose a terminal.
- Safe houses: an isolated workspace per Mission, removed when untouched and kept with
  a report when it holds work. A safe house is made by a provider of the new
  `ai6_safehouse` Guatiao kind (`ai6::directorates::q::safehouse`); `ai6-safehouse-git`
  is the `git` provider (a worktree on its own branch). An Operation chooses one with
  `workspace = { provider, source, base, branch, isolate }`; the provider defaults to `git`
  for a Git checkout. The daemon runs no Git commands itself. `ai6-safehouse-copy` is the
  `copy` provider: a plain directory copy with no history.
- Cover checks read the safe house's changes through the provider. A provider that keeps
  no history makes the cover report say `checkable = false` instead of passing.
- Cover policies (`covert`, `partial`, `overt`, or `covers.toml`) with an Operation
  author identity; Mission commits are audited and a violation marks the Mission `blown`
  when the Operation sets `on_blown = "fail"`.
- Session watchdog: stall, repeated-output loop and blocked-prompt detection, with daemon
  options and per-Agent `[agent.<id>.limits]` overrides.
- Cables: a permission-scoped agent-to-agent relay (Agent unicast; Station, Theater,
  Global and named-channel broadcasts) with deliver/hold/refuse receiver policies, quotas,
  metadata-only audit, and per-Session MCP tools (`cables-mcp` feature).
- 00 coordination: a `00`-clearance Session can propose child Missions, which wait until
  an operator approves them.
- Q Branch: bounded loadout catalogs, Mission-scoped secret leases, Claude MCP
  configuration and bubblewrap argument translation.
- Station identities (`station-tls` feature): persistent certificates, SHA-256
  fingerprints, pinned-peer or CA mutual TLS and a waiting/accepted peer store, on the shared
  `coqui-peer-tls` and `coqui-peer` crates. The strict policy is the default (TLS 1.3, SHA-2
  pins); the waiting list is bounded per source address, in total and by re-record time, a peer
  is listed only after it proved its key, and `ai6 peer waiting` shows each source address.
- Access kinds: Authenticator, Directory, Authorizer and Accounts as Guatiao kinds from the shared
  `coqui-access` crate, the first-match `RoleAuthorizer` from `coqui-roles`, and the providers
  `coqui_access_local`, `_key`, `_ldap`, `_proxy`, `_peer` and `coqui_roles`, loaded as plugin
  libraries from `--plugin-path`.
- Universal Exports limits login tries: 5 a minute per client address and 10 in ten minutes per
  account name by default, refused with `429` and `Retry-After` before any Authenticator is
  asked. `--login-tries-per-address`, `--login-tries-per-name` and `--client-address-header`
  (or `AI6_EXPORTS_LOGIN_TRIES_PER_ADDRESS`, `_PER_NAME`, `AI6_EXPORTS_CLIENT_ADDRESS_HEADER`)
  change them; `LoginLimits` and `GatewayConfig::with_login_limits` do the same in code.
- Agent handlers `ai6-agent-claude` (Claude Code stream-json) and `ai6-agent-codex` (Codex
  app-server), and the `ai6-terminal-pty` terminal provider (openpty on Unix, ConPTY with
  Job Objects on Windows).
- `universal-exports`: the authenticated public API gateway: login through an
  Authenticator or a trusted reverse-proxy identity, revocable user tokens shared through
  the Registry, per-Operation authorization, Station, Mission and Session views, Mission
  creation and proposal review, role administration, Station capacity settings,
  metadata-only audit, attach grants with read-first WebSocket terminals and expiring
  write leases. Configured with `AI6_EXPORTS_*` variables.
- Sources: the `Source` Guatiao kind (`ai6_source`) polls a tracker or CI system for work
  and posts each Mission's outcome (the CX report) back. The Station holding the
  Headquarters lease polls each `sources.<provider>` of every Operation on an interval,
  keeps a cursor per Operation and provider in the Registry, and queues items as briefs
  keyed `source_<provider>--<item>`: a changed item updates its queued brief, and a claimed
  or finished one is never queued again. Tracker text is fenced in the brief as untrusted
  data. Reports are worded by the Operation's cover policy.
- Sleepers: standing orders under `operation.<id>.sleepers.<name>` (a five-field UTC cron
  `schedule`, a `title` and a `prompt`) that queue a brief per scheduled window. A sleeper
  fires only after `ai6 sleeper authorize <operation> <name>` approves its exact definition
  (editing it withdraws the approval), and the brief's key is derived from the window, so
  a Headquarters failover cannot queue it twice. `ai6 sleeper list|authorize|revoke`.
- Source providers `ai6-source-github` (labeled issues and pull requests, comment and
  label back) and `ai6-source-jira` (JQL on Cloud or Server/Data Center, comment and
  workflow transition back).
- `ai6-diptel`: the native and WebAssembly wire contract for Session lifecycle, terminal
  bytes and resize frames, with test vectors.
- `ai6ctl`: a read-only operator command for health, Stations, Agent roles, Missions and
  Sessions.
- Moneypenny: the Flutter desktop and web operator app, with Stations and the
  Headquarters Station, the Operation → Mission → Session board, Mission briefs, live
  terminals, administration, audit and an operator menu.
- `examples/config`, the documentation site with the Rust API reference, public API
  snapshots, and test, docs and release workflows.

### Changed

- The project is renamed from coagent to AI6.
- The Python packages are replaced by the Rust workspace; Python is no longer needed at
  runtime.
- Projects are configured as Operations in `operations.toml` (formerly `stations.toml`);
  Agents and roles in `agents.toml`. A Station is now the node running the daemon.

- The guatiao pin moved to the revision the shared crates use.
- Breaking, access kinds. The kinds are `coqui_authenticator`, `coqui_directory`,
  `coqui_authorizer` and `coqui_accounts` (they were `lineupmux_*`), so providers built for
  any host load here. `Attempt::headers` is an ordered `Vec<(String, String)>` instead of a
  map. `ai6::RoleAuthorizer::from_toml` and `load` are `ai6::role_authorizer_from_toml` and
  `load_role_authorizer`; `RoleAuthorizerConfig` and `RoleAuthorizerProvider` are gone. The
  `access.toml` format is unchanged. The `coqui_roles` provider replaces `ai6_roles` and takes
  `scope_kinds`, `roles` and `rules` instead of `policy_toml`.
- Breaking, provider ids and configuration. Pass `--plugin-path` to the directory of the
  `coqui-access` plugin libraries and change the ids and configs:
  - `ai6_auth_local` is `coqui_access_local`: `{"path": P}` is
    `{"source": {"file": {"path": P}}}`. The `[[accounts]]` file is unchanged.
  - `ai6_auth_key` is `coqui_access_key`: `{"path": P}` is
    `{"source": {"file": {"path": P}}, "prefix": "ai6_"}` (the prefix keeps issued keys
    working; new keys use it too). Every `hash` in the `[[keys]]` file gets a `sha256:` label
    (`sha256:<hex>`); a bare digest is refused.
  - `ai6_auth_peer` is `coqui_access_peer`: `{"path": P}` is `{"source": {"file": {"path": P}}}`.
    The `[[peers]]` file is unchanged and its `certificate` may stay in any fingerprint spelling.
  - `ai6_auth_ldap` is `coqui_access_ldap` and `ai6_auth_proxy` is `coqui_access_proxy`; their
    configs are unchanged.
- Breaking, Station TLS. `ai6::station::identity` is built on `coqui-peer-tls`: `StationIdentity`
  is `Identity`, `StationFingerprint` is `Fingerprint`, `PeerTrustStore` is `TrustStore`,
  `TlsTrust` is `Trust`, and `client_config`/`server_config` take a `Policy` and return
  `Built<..>`; none is re-exported at the crate root. Handshakes are TLS 1.3 only unless the
  policy allows 1.2. Fingerprints print as `sha256:<hex>`; every earlier spelling is still read.
  An `identity.pem` and a `peers/` store from before load unchanged.

### Removed

- The crates `ai6-access-key`, `ai6-access-ldap`, `ai6-access-local`, `ai6-access-peer` and
  `ai6-access-proxy`; the `coqui-access` plugin libraries replace them.
- The Python packages `coagent-00`, `coagent-i` and `coagent-m`, their commands and their
  `COAGENT_*` environment variables, and the `q/` placeholder.

[Unreleased]: https://github.com/jose-pr/ai6/commits/main
