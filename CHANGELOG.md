# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Green ink: a person's recorded approval of one irreversible step. An Agent calls the
  `request_approval` tool on its Session's AI6 endpoint with the step's kind (`push`,
  `merge`, `release`, `delete`, `clearance` or one its Operation adds) and target. The
  Operation's `[operation.ID.green_ink]` policy (preset `strict` by default, or `licensed`,
  where `00` pushes and merges unattended; every key overridable) answers `not_required`,
  a live class authorisation answers `approved`, and otherwise the request is `pending`:
  the Mission is `blocked` with reason `green-ink`, its Watchdog paused, until an operator
  with the new `approval.decide` permission approves it for a chosen time or rejects it
  (`GET /v1/user/operations/{id}/green-ink`, `POST .../{request}/approve|reject`). The
  Station tells the Agent the decision and the Mission works again; a request nobody
  decides expires as refused after 24 hours. Class authorisations (`approval.class`,
  `/v1/user/operations/{id}/class-authorizations`) approve a kind of step on a target glob
  for named Agents or roles, up to a use limit, and are revoked there. Records live in the
  Registry (`ai6::ApprovalRequest`, `Approval`, `ClassAuthorization`); decisions are audited
  and requests appear on the timeline. Moneypenny has Green ink and Class authorisations
  tabs under Administration. AI6 does not yet stop a step taken without green ink.
- The DipTel lifecycle frame carries an optional `reason` (`green-ink`, `budget`), shown on
  the board beside the Session's state.

- Operator controls. With the new `mission.control` permission an operator cancels,
  retries, resumes or interrupts a Mission through
  `POST /v1/user/operations/{operation_id}/missions/{mission_id}/actions` (idempotent by
  key, audited as `mission.cancel`, `mission.retry`, `mission.resume`,
  `session.interrupt`). Universal Exports queues a typed DipTel control
  (`ai6_diptel::MissionControl`) for the Station holding the Mission, which takes it from
  `/v1/station/controls/next`, checks the Mission, its Operation and its fenced Session
  lease, applies it and reports the outcome, read back from the control's status route.
  A retry queues the Mission as a new attempt, optionally for another qualified Agent
  (`MissionStore::retry`). `ai6ctl mission cancel|retry|resume` and `ai6ctl session
  interrupt` wait for the outcome; Moneypenny offers the controls in each Mission card's
  menu with the reason any is unavailable. DipTel gains the `queued` and `abandoned`
  states, and board Sessions carry `updated_at`, so a client finds a Mission's current
  Session.
- An Operation activity timeline. `GET /v1/user/operations/{operation_id}/timeline`
  (`audit.read`) pages newest first through lifecycle transitions, control outcomes,
  Cables sent and Sleeper authorizations from a new Registry activity log
  (`ai6::TimelineEvent`, the newest 10,000 kept) merged with the Operation's audit records,
  correlated by Station, Mission and Session, filterable by Mission and continued by an
  opaque cursor. Items are metadata only. Moneypenny shows it in a Timeline view.

- A Station's identity defaults to `<host>-<random>`, created on first start and kept in
  `station-id` in the AI6 state directory, so a restarted daemon stays the same Station;
  `--station` or `AI6_STATION_ID` override it.
- Theaters: a Station reads `theaters.toml` (`[theater.<id>]` with `access = "local"` or an
  `ssh` host, plus labels), always reaches its own host as the implicit `local` Theater and
  publishes its Theaters at check-in. A Session's launch record carries `station_id` and
  `theater_id` separately (older records still load). `GET /v1/stations` lists each
  Station's Theaters, and Moneypenny shows them and the Station and Theater of each Session.
- Transports. A Station reaches a Theater through a transport, a new provider kind
  (`ai6_transport`, `ai6::directorates::q::transport`) with optional, capability-gated
  operations: open a terminal, run a command, read, write and list files, and forward a port
  from the host back to the Station. Each transport declares its `Capabilities`
  (`terminal`, `run`, `files`, `forward`, `os`, `arch`); a contract suite beside the kind
  checks any transport against what it declares. Theaters gain `agents` (`auto`, `local` or
  `remote-hands`), `egress` (`direct`, a proxy, `station` or `none`), `harnesses`, `workdir`
  and `access = { transport = { provider, config } }` for any other transport provider.
  Stations publish each Theater's capabilities and declared facts, and `GET /v1/stations`
  and Moneypenny's Theater tooltips show them; a Theater that cannot be reached is shown
  with the reason and ruled out of placement.
- Five transport providers. `ai6-transport-local` runs commands and reads and writes
  files on the Station's own host. `ai6-transport-adb` reaches an Android device through
  `adb` (commands, files built from shell commands, `adb reverse` forwards),
  `ai6-transport-container` a running Docker or Podman container through `exec`, and
  `ai6-transport-qemu` a QEMU guest through its guest agent; the shell-based file operations
  they share are in `ai6::directorates::q::transport::kit`. `ai6-transport-ssh` reaches a host over SSH with a
  built-in pure-Rust client (`russh`, `ring`), so it needs no installed `ssh` and behaves the
  same on every OS: commands, pty terminals, SFTP files and reverse forwards, reconnecting
  on its own. The host key is pinned (`host_key = "sha256:HEX"`); first contact without a pin
  is refused, naming the key to pin, unless `trust_on_first_use = true`, which the Station
  logs and `GET /v1/stations` reports as a weakening. Keys stay in files: only a key path is
  configured, and a passphrase-protected key is refused.
- Remote-hands mode. Placement now chooses field mode (the harness runs on the Theater) or
  remote-hands mode (the harness runs on the Station and acts on the Theater through its
  transport) from the Theater's capabilities and its `agents` and `egress` settings, and
  every rejection names the missing capability. In remote-hands mode the harness's own shell
  and file tools are switched off (Claude Code `--disallowedTools`; Codex runs in a read-only
  sandbox that asks nothing) and four Theater tools (`theater_run`, `theater_read`,
  `theater_write`, `theater_list`) on a per-Session authenticated MCP endpoint act on the
  Theater instead. Placement choices, launch records, the published Mission brief and
  Moneypenny carry the mode. Field mode on a remote Theater still needs a runner there and is
  not implemented.
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

- Medical: usage and budgets. The Claude and Codex handlers report tokens and cost from the
  provider's own events (`AgentEvent::usage`, `TokenUsage`); the Station keeps a per-Mission
  ledger in the shared Registry (`usage/`), returned as `usage` by the Mission detail of
  Universal Exports. `[operation.ID.budget]` and `[agent.ID.budget]` set `session_tokens`,
  `mission_tokens`, `mission_cost_usd`, `operation_cost_usd_per_day` and `warn_at`; nothing is
  limited unless set or `preset = "strict"` is chosen. Crossing `warn_at` emits a `budget`
  event and an audit record; reaching a limit blocks the Mission with `blocked_reason`
  `budget` (resumable once the limit is raised). A Mission cost limit also becomes Claude
  Code's `--max-budget-usd`.
- Medical: exposure. A Session is marked when its Mission came from a Source item, it calls a
  tool the Operation lists (`WebFetch` and `WebSearch` by default), or it reads a Cable from
  another Operation or an exposed Session. The mark is kept in the Session record, inherited
  by later attempts, shown on the board (`exposure`) and as an `EXPOSED` chip in Moneypenny.
  `[operation.ID.exposure]` picks `strict` (default: an exposed Session sends Cables only as
  replies, and its Mission proposals carry its exposure) or `observe`, and can choose
  restrictions and tools individually.
- Medical: quarantine. An Agent that stalls or loops three times, hits a budget stop twice, or
  blows its cover (`[operation.ID.quarantine]`, 24 hours, each trigger can be set to 0) is
  quarantined in the Registry: its Sessions stop on every Station, its Missions become
  `quarantined` (safe houses kept), its Cables are held and the roster skips it. An operator
  quarantines or releases with `ai6 quarantine list|add|release` or the Universal Exports
  `quarantine` routes (`agent.configure`); a released Mission returns to the queue with the
  next qualified Agent. `quarantined` is a Session state in `ai6-diptel`, the gateway,
  `ai6ctl` and Moneypenny, and `ai6ctl` lists a Session's exposure.

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

### Fixed

- Terminal input queued at Universal Exports reaches the Session again: the gateway named
  the input's Operation in `x-ai6-station-id` while Stations read `x-ai6-operation-id`, so
  every input was dropped.

[Unreleased]: https://github.com/jose-pr/ai6/commits/main
