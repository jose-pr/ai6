# Architecture

AI6 has two independent shapes: the **work** (what is being done) and the
**infrastructure** (where it runs). Neither contains the other.

```text
Work            Operation ──► Mission ──► Session
                (global)      (brief)     (one provider run, provider-issued Session ID)
                                             │ placed on
Infrastructure  Station ──────────────────► Theater
                (node running `ai6`)         (the host or environment the harness runs in)
```

- An **Operation** is a global project and its policy: checkout, roster, clearance,
  cover, budget and a `station_labels` of Station labels. Stations serve Operations; no
  Station owns one.
- A **Mission** is one brief under an Operation. Its **Mission ID** may group several
  **Sessions**; a Session ID comes from the provider and survives a resume.
- A **Station** is a node running the `ai6` daemon, with the `control` role, the
  `execution` role, or both. Control Stations elect the **Headquarters Station (HQS)**,
  which holds M's authority through a lease. An execution-only Station is an
  *outstation*.
- A **Theater** is where a Session actually works, reached through a **transport** (`local`,
  `ssh`, or another provider that declares what it can do). A harness runs *on* the Theater
  in field mode, which today means the Station's own host in its terminal provider, or on the
  Station in remote-hands mode, acting on the Theater through the transport.

For orientation only, the closest Kubernetes concepts are: Station ≈ node and kubelet,
HQS ≈ elected control plane, Registry ≈ etcd, Operation ≈ namespace with a placement
policy, Mission ≈ Job, Session ≈ a Pod's run, Theater ≈ the container runtime or sandbox
host, Universal Exports ≈ the API server's front, `ai6ctl` ≈ `kubectl`. These are not
AI6 names.

## Runtime

```text
 operators ── Outer Office / ai6ctl ──── HTTPS/WSS ──► Universal Exports (public API)
                                                         │ Station bearer, DipTel frames
 people, trackers ── briefs ──► Registry                ▼
                                   ▲         ┌──────────────────────────────┐
                                   └─leases──┤ Station: ai6 serve           │
                                             │  directorates: M I Q 00 Med. │
                                             │  station: server, plugins,   │
                                             │   safe houses, publisher     │
                                             └──────────────┬───────────────┘
                                                            │ Guatiao providers
                                    AgentHandler (Claude, Codex), terminal (pty)
```

**The Station** (`ai6` crate) runs the directorates' decisions. Its private loopback API
serves provider discovery and the Mission lifecycle; the scheduler leases briefs from the
Registry, selects an Agent, creates a safe house and starts a Session. Mission records
persist, so a restarted Station reattaches to provider Sessions by their Session ID.

**Universal Exports** (`universal-exports` crate) is the only public entry point. It
authenticates users (Authenticator and Directory providers, or a trusted reverse-proxy
identity), applies the authorization policy per Operation, issues attach grants and
write leases, fans out DipTels with bounded replay, and records metadata-only audit.
Browsers and `ai6ctl` never connect to a Station or a provider.

**DipTels** (`ai6-diptel` crate) are the wire contract between a Station, Universal
Exports and Outer Office: Guatiao values for Session lifecycle, terminal bytes and
geometry, used natively and from WebAssembly.

**Outer Office** (`clients/outer-office`, short name OO) is the Flutter desktop and web
operator app. [Moneypenny](assistant.md), the operator's AI assistant, works inside it.
It bundles its fonts, icons and web engine, and its web build serves the Noto fallback
fonts for other scripts and emoji (34.4 MiB, fetched only as text needs them), so by
default it makes no request to any origin but its own; a web deployment opts in to
Google Fonts or Flutter's CDN with
`[assets] cdn = "fonts"` or `"all"` in the `OO.cfg` beside its `index.html` (the app's
README has the details).

## How the directorates decouple

1. **00 Section owns the harness boundary.** `AgentHandler`
   (`ai6::directorates::section_00`) starts or resumes a provider Session, delivers initial
   context, accepts later messages and decodes events. Claude and Codex handlers are
   Guatiao providers in `crates/providers/agents/`.
2. **Q Branch owns gear.** Terminals (`ai6::directorates::q::terminal`, provider `pty`),
   validated loadouts, Mission-scoped secret leases, Claude MCP configuration,
   bubblewrap argument translation and Theater provisioning. A Session's loadout secrets
   are leased when it starts and handed to the harness in its environment only. Secrets
   never enter a worktree, event log or terminal stream.
3. **The Registry owns the shared record.** The file-backed Registry holds briefs,
   leases, Station check-ins, the Headquarters lease, audit, role overrides and
   access-token digests; the Cable relay delivers agent-to-agent messages.
4. **M owns decisions.** The roster, Agent selection, clearance, cover and the Mission
   lifecycle (`ai6::directorates::m`), and the access kinds whose policy Universal Exports
   enforces.
5. **Medical watches health.** The watchdog detects stalls, loops and blocked prompts.
6. **The Station runs it all** (`ai6::station`): server, provider host, safe houses,
   publisher and mTLS identity.

Frames are [guatiao](https://github.com/Boriken-Dev/guatiao) values declared once in
`ai6-diptel`; no endpoint hand-rolls a codec. Attach is authorized per Operation,
Mission, Session and mode (`read` or `write`) with a short-lived token; WebSockets are
same-origin and TLS-only, and operator input counts as watchdog activity.

## Not implemented yet

- Remote safe houses, and a terminal over the container, VM and Android transports. A
  remote Theater hosts a harness only through a runner there or the SSH-style detached
  wrapper; otherwise it works in remote-hands mode.
- Central placement: Stations sharing a Registry each lease the briefs they can run
  themselves; the Headquarters lease does not yet drive placement.
- Of a loadout, only its secrets are applied when a Session starts; its sandbox profile
  and MCP servers are parsed and validated but not yet applied.
- External A2A interoperability (see below).

## Agent Cables

Agent-to-agent messages are **Cables**; terminal output, guatiao wire frames and live
stream events remain **DipTels**. The `ai6::directorates::registry::cables` relay stores durable Cable envelopes,
resolves audiences and enforces explicit grants. The Rust AI6 daemon starts per-Session
MCP endpoints for managed Claude and Codex handlers when `cables-mcp` is enabled.

A Cable has a relay-generated ID and timestamp, a sender principal supplied by M's trusted
adapter from the managed Session, an explicit typed recipient, bounded plain-text body, idempotency key,
delivery state and TTL. It may carry the enclosing Mission ID, the provider-issued
Session ID, and conversation/reply IDs for correlation. The Session ID is descriptive,
never proof of identity. Callers cannot choose their sender identity or attach files;
file and resource access require separate authorization.

An `agent` recipient is unicast. `station`, `theater`, `global` and named `channel`
recipients are broadcasts with bounded audiences resolved by the relay at acceptance.
Station delivery requires Station membership and a send grant. Theater and custom
channel delivery require explicit grants for each sender and receiver; Global requires
an exceptional explicit grant and is never a default. The relay rechecks receiver policy
at delivery. Each receiver may `deliver`, `hold`, or `refuse`. Held Cables remain hidden
from recipient inboxes. An approver needs an explicit `release` grant for the Cable's exact
audience scope to list body-free held metadata and release it; the relay checks each
recipient's current receive grant again, refusing any whose grant was revoked. Hidden
recipients are not disclosed by denial errors.

The lifecycle is `queued`, `delivered`, `held`, `refused`, `acked`, `expired` or
`failed`. Retries preserve the Cable ID and are idempotent. The default TTL is 15 minutes
and the maximum is 24 hours; broadcast membership is snapshotted when accepted. A live
recipient is bound to its current Session; a known offline Agent gets an Agent-level
queued delivery that any of its Sessions may read before expiry. The relay uses shared
Registry token buckets by sender Agent and target scope: defaults are 2 sends/second with a
burst of 20 per Agent, and 10 sends/second with a burst of 50 per scope. Identical retries
reuse the original audience snapshot and do not consume another token. Bounded fan-out, a
one-hop limit and body-size caps control storms and replay. M emits metadata-only audit
callbacks for accepted/denied sends, retries, delivery, acknowledgement and release. Each record
contains Agent, Operation, Station, Mission/Session IDs, target scope, decision and timestamp; it
never contains the Cable body. The embedding site chooses its audit sink. Incoming text is
untrusted and cannot grant tools, clearance, secrets or authority; each receiving agent's
own permissions still apply.

The MCP tools expose authorized discovery, unicast, broadcast, inbox reads,
held-Cable approvals, acknowledgements and replies. `list_held_cables` returns sender,
target, recipient and expiry metadata but never a held message body. `release_cable` requires
the same explicit scope grant, rechecks recipient receive grants and moves authorized
recipients into their inboxes. Rust AI6's optional `cables-mcp` feature creates a separate
loopback listener per managed Agent Session, returns no tools until the provider-issued
Session ID is bound, and closes the listener when the Session ends. Claude receives an
environment-substituted `--mcp-config`; Codex app-server receives per-process config
overrides. Both read bearer credentials from the launch environment, never from argv or
persisted configuration. Remote SSH tunneling is not implemented.
MCP is the agent-to-tool interface. [A2A](https://a2a-protocol.org/latest/) remains a
future cross-harness interoperability boundary, while AI6's relay remains authoritative
for local identity and scope permissions. [Claude Code's cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging)
is a reference for delivered/held/refused outcomes and receiver-side controls, not an AI6
transport dependency. See the [A2A coding-harness analysis](https://shashikantjagtap.net/a2a-for-coding-harnesses-the-protocol-most-teams-still-build-privately/)
for the interoperability motivation.

**Protocol decision:** AI6 will not expose A2A yet. Its current MCP Cable surface serves
managed Agents through M's local relay, while no authenticated cross-harness directory or
peer trust exchange exists. Revisit A2A when a deployment needs discovery and message/task
exchange with agents outside its AI6 Registry; an adapter must preserve relay authorization
and map no external identity into AI6 authority. An A2A `Task ID` remains an external
protocol identifier: it is not AI6's **Mission ID** (a unit of work that can group multiple
Sessions) or a provider-issued **Session ID** (one resumable agent run). Inbox
delivery currently uses polling through `read_cables` at agent-selected checkpoints; it
does not wake an idle provider session.

## Terminal handshakes

A shell under a pty, and ConPTY at start-up, asks where the cursor is (`ESC[6n`) and
waits. With no emulator attached that looks exactly like a hung agent, so on Windows the
`pty` terminal provider answers every query itself, always with row 1, column 1, and removes
it from the stream so an attached viewer's emulator does not answer a second time.

## Prior art

| Project | What we take from it |
| --- | --- |
| [BloopAI/vibe-kanban](https://github.com/BloopAI/vibe-kanban) | A Kanban board where a card is an agent run; one git worktree per task so agents work in parallel without colliding; process state shown on the card |
| [OrchestratorInc/agent-orchestrator](https://github.com/OrchestratorInc/agent-orchestrator) | Multi-agent task routing and lifecycle tracking on a shared board |
| [tsl0922/ttyd](https://github.com/tsl0922/ttyd) | One terminal, many browser viewers over a thin WebSocket protocol rendered by xterm.js; per-client auth; read-only by default |
| agent-tmux-web, webmux | tmux as the session layer under a web terminal, so SSH and browser attachments coexist |
| herdr | Many processes multiplexed into one view with their states visible at a glance |
| [tmux](https://github.com/tmux/tmux), [systemd-run](https://www.freedesktop.org/software/systemd/man/latest/systemd-run.html) | Session persistence and detach; transient units with cgroup limits |
| [Model Context Protocol](https://modelcontextprotocol.io/) | The tool interface Q arms agents with |
| [Boriken-Dev/guatiao](https://github.com/Boriken-Dev/guatiao) | Not prior art but a dependency: the provider ABI and the DipTel wire format |

## Repository layout

| Path | Language | What |
| --- | --- | --- |
| `crates/ai6/` | Rust | The Station daemon and AI6 contracts: directorates and `station` modules, `ai6` command |
| `crates/universal-exports/` | Rust | The authenticated public API gateway and its command |
| `crates/ai6-diptel/` | Rust | The DipTel wire contract, native and WebAssembly |
| `crates/ai6ctl/` | Rust | The operator command |
| `crates/providers/agents/` | Rust | Claude and Codex AgentHandler providers |
| `crates/providers/terminals/pty/` | Rust | The `pty` terminal provider (openpty / ConPTY) |
| `crates/providers/terminals/pipe/` | Rust | The `pipe` terminal provider: plain pipes, no terminal, for JSON harnesses |
| `crates/providers/transports/` | Rust | The `local`, `ssh`, `adb`, `container` and `qemu` transport providers: how a Station reaches a Theater |
| `crates/providers/theaters/` | Rust | The `ephemeral-container` Theater provider: a fresh container per Mission |
| `crates/providers/safehouses/` | Rust | The `git` (worktree) and `copy` (plain directory) safe-house providers |
| `coqui-access` (shared, git dependency) | Rust | Access kinds, the role authorizer and the key, LDAP, local, peer-certificate and reverse-proxy access providers, loaded as plugin libraries |
| `coqui-peer-tls`, `coqui-peer` (shared, crates.io) | Rust | Station identity, fingerprints, trust store and mutual TLS |
| `clients/outer-office/` | Dart/Flutter | Outer Office, the operator app for desktop and web |
| `examples/config/` | TOML | A runnable Operation and Agent roster |
