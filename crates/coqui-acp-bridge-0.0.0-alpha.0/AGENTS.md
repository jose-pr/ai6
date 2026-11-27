# `coqui-acp-bridge` — API header

Header-file-style reference for the `coqui-acp-bridge` crate: the commands it offers as library modules, and its small library, so it can be used without reading the source. It ships inside the package and is self-contained. Development documentation lives with the source at <https://github.com/Boriken-Dev/coqui-acp>.

No Cargo features. MSRV Rust 1.89. Version `0.0.0-alpha.0`, not published. It depends on `coqui-transport` (its `Channel` and `Failure` are what `relay` and `Arrival::serve` take) and `coqui-transport-web` (the WebSocket and WebTransport drivers), `coqui-peer`, `cabuya` (features `sealed` and `forward`; it brings `coqui-peer-noise`), `agentic-peer`, `coqui-peer-webcrypto`, `coqui-peer-tls`, `coqui-net-acl`, `clap`, `rustls`, `mdns-sd`, `serde_json` and `url`, with `rustix` on Unix and `win32job` on Windows for ending an agent and what it started.

No `unsafe` code (`#![forbid(unsafe_code)]`). The crate builds no program: a program that people run is its host's, and a host exposes these commands. Public modules: `agent`, `linger`, `relay`, and one per command: `serve`, `dial`, `forward`, `pairing`, `grant`, `discover`, `keygen`, `mcp_shim`, `standing`, with `helper` (below) and `MethodArgs`/`SessionArgs` at the root. Proof, pairing, the link's streams and peers, and `tcp/1` are `cabuya`'s; the home, discovery, the companion and MCP offers are `agentic-peer`'s; see their `AGENTS.md`.

## Commands as modules

Each command's arguments are one `clap::Args` type with public fields, which a host flattens into its own command line or builds by hand; its `run` takes them and returns `Result<u8, String>`: `Ok` is the status to end with (0, or an agent's), `Err` the message to say, which a host ends with status 2 on.

```rust
serve::Serve, serve::run(&Serve)              // wait for peers, prove each, give each an agent; ends only on a failure to start
dial::Dial, dial::run(&Dial)                  // connect out, reconnect, keep the agent running; ends with the agent's status
forward::Forward, forward::run(&Forward)      // TCP over a link
pairing::PairAccept, pairing::accept(&PairAccept)   // show a PIN, pair one peer
pairing::PairDial, pairing::join(&PairDial)         // join a waiting accept with its PIN
grant::run(home: Option<&Path>, peer, protocol, revoke)
discover::run(seconds)                        // one JSON line per candidate on standard output
keygen::create(path, method) -> peer id
mcp_shim::run(peer: Option<&str>, home: Option<&Path>)   // the stdio MCP server
mcp_shim::config(&Helper, home: Option<&Path>, peer, name) -> the `mcpServers` JSON
helper::Helper { program, args }              // the program that runs `agent-guard PID`, `mcp-shim` and `mcp-shim --peer PEER`
agent::watch(pid, stdin)                      // what `agent-guard` runs
```

More settings that are not command-line arguments: `Serve::admit`, an `Admit` rule (`Admit::new(|peer| ...)`) asked once a peer has proved itself and again whenever its link asks the trust store (and for a kept agent), for a host whose own rules limit some paired keys; `Dial::paired_store` and `Forward::paired_store`, the trust store `--paired` proves the remote against, in place of this user's own, for a host that keeps the keys it dials apart from the keys it serves. `valid_origin(text)` says whether an `--origin` is `scheme://host[:port]` (never `null`, no path); `serve` and `pair-accept` refuse one that is not. Two settings are not command-line arguments: `Serve::helper`, the `Helper` a linger guard (Unix) and `--mcp-offers` start, without which `--mcp-offers` is refused and a kept agent has no guard; and `home` on `Serve`, `Dial`, `Forward`, `PairAccept` and `PairDial` (and `grant::run`, `mcp_shim::config`), the directory of the identity (`identity`, `identity-noise`, `method`), trust store (`peers`) and grants (`grants.json`) of a host that keeps them somewhere other than `agentic_peer::home`. A command's flags, as a host spells them (`serve`, `dial`, `forward`, `pair-accept`, `pair-dial`, `grant`, `discover`, `keygen`, `mcp-config`, `mcp-shim`, `agent-guard`), are those of its argument type:

```text
keygen PATH [--method noise|webcrypto]
serve --identity FILE (--allow PEER... | --allow-store DIR) [--listen ADDR] [--tls-identity FILE] [--webtransport-port PORT]
      [--origin URL...] [--permit NET...] [--deny NET...] [--proof-seconds N] [--max-peers N]
      [--linger SECONDS [--linger-per-peer N] [--linger-total N] [--linger-buffer-bytes BYTES] [--linger-settle-ms MS]] -- PROGRAM ARGS...
serve --tls-identity FILE (--allow-cert FP... | --allow-cert-store DIR | --client-ca FILE) [--listen ADDR] [--origin URL...] [--permit NET...] [--deny NET...] [--max-peers N] -- PROGRAM ARGS...
serve --no-peer-proof [--tls-identity FILE] [--listen ADDR] [--origin URL...] [--permit NET...] [--deny NET...] [--max-peers N] -- PROGRAM ARGS...
serve --paired [--listen ADDR] [--tls-identity FILE | --tls-short-lived] ... -- PROGRAM ARGS...
dial URL --identity FILE (--peer ID... | --trust-store DIR) [--ca-file FILE [--bearer-file FILE]] [--max-backoff-seconds N] -- PROGRAM ARGS...
dial URL --tls-identity FILE (--expect FP... | --ca-file FILE) [--bearer-file FILE] [--max-backoff-seconds N] -- PROGRAM ARGS...
dial wss://... (--expect FP... | --ca-file FILE) [--bearer-file FILE] [--max-backoff-seconds N] -- PROGRAM ARGS...
dial ws://loopback --no-peer-proof [--max-backoff-seconds N] -- PROGRAM ARGS...
dial URL --paired [--ca-file FILE [--bearer-file FILE]] [--max-backoff-seconds N] -- PROGRAM ARGS...
pair-accept --listen ADDR [--identity FILE] [--store DIR] [--tls-identity FILE] [--origin URL...] [--timeout-seconds N] [--run-seconds N] [--permit NET...] [--deny NET...]
pair-dial URL [--identity FILE] [--store DIR] [--pin-file FILE] [--ca-file FILE]
discover [--seconds N]
grant PEER TOKEN [--revoke]
mcp-config --peer PEER [--name NAME]
mcp-shim --peer PEER
forward URL (--paired | --identity KEY --peer ID) [--local LOCAL=TARGET]... [--allow-target HOST:PORT]...
```

The crate's tests drive them through `examples/bridge.rs`, a command line over exactly these modules.

Every `serve` form also takes `--advertise LABEL` and `--advertise-host NAME`. With the in-band proof it also takes `--service TOKEN=PROGRAM...` and `--grant PEER=TOKEN...`, and `dial` takes `--fallback URL...`. With the in-band proof, `--tls-short-lived` replaces `--tls-identity`. Every in-band form of `serve`, `dial` and `forward`, and `pair-accept` and `pair-dial`, takes `--method noise|webcrypto`; `serve`, `dial` and `forward` also take `--key-limit BYTES` and `--no-rekey`.

- **Several protocols on one link:** `--service mcp=PROGRAM` serves another protocol beside the agent after `--`, which is `acp/1`. A client opens streams on one proven link, and the first message tells a link from a plain ACP client, which keeps working. A proof admits a peer and grants nothing: `--grant PEER=TOKEN` names what it may open, with everything for an allowed peer when none is listed. Under `--paired`, a peer gets `acp/1` for being paired, and `grant PEER TOKEN` adds or takes away anything else, counted at once on a live link, with no second pairing. A store that cannot be read refuses. `--advertise` then says what is offered, as a hint.
- **A peer's own tools:** `serve --mcp-offers` serves `mcp-offer/1`. A peer opens it, with a server name in its metadata, and is answered with one stdio `mcpServers` entry running the host's `mcp-shim` (the `Helper`), with a loopback address and the path of an owner-only token file in its environment, to declare in `session/new`. It has to run as the bridge's user. The relay still reads no ACP. A paired peer has it with `acp/1`, since it could already declare any command, so `grant --revoke` refuses either rather than saying it worked; `--grant` names it like anything else. The wire is in `agentic-peer`'s `docs/link.md`.
- **A program no peer declared to:** with `serve --mcp-offers`, an MCP client on this machine, such as an IDE or a CLI agent, can reach a peer's tools with no ACP session. `mcp-config --peer PEER` prints the `mcpServers` entry to paste, named `canoa-browser` unless `--name` says otherwise. It holds no port and no secret, so it stays good across restarts of either side. It runs `mcp-shim --peer PEER`, which reaches only that peer: another peer of this bridge has a file of its own.
- **Its wire:** the peer opens `mcp-offer/1` with the metadata `standing/1`, which an older bridge refuses as no server name. It is answered `{"standing":1}`, and `{"claimed":true}` when a program takes the stream; from then on each message is MCP's own, carried as lines to that program, and the stream ends when the program does. One stream carries one program, so the peer opens the next when it hears one was taken, and several programs share the peer's one grant. The peer's first stream makes a loopback port with a random token and an answer, in `standing-<hex>.json` in `offers/` under this user's home, and its last stream ends them. The shim shows the token, and checks the answer before saying anything, so something else on a port a dead bridge left learns only a dead token. This is not HTTP, so a web page firing at the port gets nothing, and something connected has two seconds in all to show the token.
- **Nothing connected:** the shim says why on standard error, answers its client's first message with a JSON-RPC error saying the same, and exits with status 1. A stream ending, which is the peer going away or turning this off, ends the shim, which its client sees as a stdio server closing. A client has to start the server again to come back, from the same entry. A peer gone without a word, over WebTransport, can take that link's idle timeout to be noticed (about 25 seconds, measured), and a program starting meanwhile gets in and hears nothing.
- **Same user:** any program running as this user can read that file and use what the peer has granted. Another user cannot, on Unix by the file's and directory's modes, which the shim also checks, and on Windows by an ACL for its owner alone with nothing inherited, which `icacls` sets before anything is written, or there is no door. An offer's `.token` files take their directory's permissions, which on Windows are `%LOCALAPPDATA%`'s.
- **Forwarding TCP:** `serve --forward` serves `tcp/1`, and grants nothing: a peer needs `--grant PEER=tcp/1:HOST:PORT` or `grant PEER tcp/1:HOST:PORT`, exact, with no wildcards. `forward --local` listens here and reaches a target from the remote. `serve --reverse LOCAL=PEER=TARGET` and `forward --allow-target` are the other direction, with the dialing side deciding. `--allow-target` needs `--peer`, which says who may reach it. A listener has to be on loopback, and anyone on this machine who can reach it, a web page included, gets what this peer was granted. One peer holds at most 8 links, and everyone together 256; a proven peer has a minute to say which it is. `--service` cannot name `tcp/1`, `link/1` or `acp/1`, and revoking `tcp/1` also clears its targets.
- **Keeping an agent through a reconnect:** `serve --linger SECONDS`, with the in-band proof only, keeps a link's `acp/1` agent that long when the link ends, orderly or cut, and gives it to the next connection proving the same key, whether a plain client or a stream of a link; `--service` programs are not kept. A peer that broke the wire format, and an agent that stopped reading or closed its output, leave nothing kept. Off by default, and with 0. The bridge still reads no ACP, so a client asks with `session/resume`, and an agent that has gone answers in its own words from a fresh one. An agent gets a second `initialize` this way.
  - A peer no longer accepted gets nothing back: with `--allow-store` or `--paired`, the store is asked about once a second while an agent is kept and again before it is given back, and a kept agent whose peer was removed ends, turn and all. Accepting the key again later gives it a fresh agent.
  - The key is all that is checked. Under `--paired` every tool of this user on the client machine proves the same identity, so any of them can get another's kept agent, its session and what it wrote; give tools that must stay apart identities of their own.
  - An agent is running while its output is open, whoever holds it: a launcher that exits after starting the real program, such as `cmd /c start /b`, still has its agent kept and given back.
  - Limits: `--linger-per-peer` 1. A peer has two agents only when it connected while an earlier link was still up, as when that link's peer vanished without closing; past the bound the one started last ends, since the earlier holds the session that link was in, and the earliest comes back first. Above 1 a peer can be given the wrong agent, since nothing here can tell which holds its session. `--linger-total` 4, past which the one kept longest, of anyone's, ends. Kept agents take no place among `--max-peers`, and getting one back needs a place.
  - A connection while another of the same peer is up gets an agent of its own, or what is kept: one stdio pipe has one reader. It first waits up to `--linger-settle-ms` (500) for that link to end, so a reload faster than the old link's end still finds its agent; through the library, a `settle` too long for the clock waits until that link ends. A link whose peer vanished without closing lasts until the channel notices, up to 2 minutes, so a reconnect sooner than that gets a fresh agent.
  - Output while nobody is connected: `--linger-buffer-bytes` (16 MiB, at most 256 MiB) of the newest lines per agent, each counted with what holding it takes. Past it the oldest are dropped, since the newest hold the answer that ends a turn, and the next link's first message is a JSON-RPC notification, method `_dev.boriken.coqui.acp/output_lost`, params `{"lines": N, "bytes": N}`. That is the one message the bridge writes itself. Whichever connection of that peer comes next gets all of this. An agent that ends while kept, having written something meanwhile, leaves what it wrote: the next connection gets that first, then a fresh agent.
  - What this does not cover: what a link had accepted and never delivered when it was cut is lost with no notice, since nothing acknowledges it, so a client treats any reconnect as a possible gap. What the agent answers on a new link may be to a request of the old one, with its id. A request the agent is waiting on an answer to stays open while nobody is connected.
  - Agent input while nobody is connected: nothing new is written and its standard input stays open. Lines already queued to it are written; one still waiting when the link ended is dropped, so what a peer sent last has an unknown outcome.
  - When the bridge goes: on Windows every agent, with or without `--linger`, is in a job object of its own, which the system ends with everything in it when the bridge's handle closes, however the bridge ends. A process the agent starts in the moment before it is put there is not in it. On Unix, under `--linger`, each agent has a helper, the host's `agent-guard PID` (the `Helper`), that ends the agent's process group when the bridge goes. That is cleanup, not containment: a process that leaves the group (`setsid`, `setpgid`) escapes it, a signal to every process of this program's name (`killall`) ends the helpers too, and the agent runs as the same user, so it can end or dismiss its helper. Without `--linger`, on Unix, an agent is told only by its input closing.
- **Stopping an agent:** ends what it started too, by process group on Unix and job object on Windows. An agent runs in its own group, so a terminal's Ctrl-C reaches the bridge and not it directly; the bridge ends it.
- **Failover:** `dial --fallback URL` tries the next endpoint when one cannot be reached. Each has to prove what the first would, and they share a scheme.
- **One pairing:** `pair-accept` and `pair-dial` default to this user's own identity and trust store (`agentic_peer::home`), and `serve --paired` and `dial --paired` read them, so one pairing serves every command, with nothing to name.
- **In-band proof (`--identity`):** the identity's method's handshake, Noise (`Noise_XX_25519_AESGCM_SHA256`) or WebCrypto (P-256), then every message inside its session. It does not depend on TLS, so it works through a reverse proxy that ends TLS, and the proxy can neither read nor inject. Under WebCrypto the handshake may be text or binary frames; Noise's is binary. Each record after it is one binary frame.
- **The method:** an identity file holds a key of one method, and its type is the method: `--method` naming another is refused (status 2, "this identity proves by X, not Y"). A new identity gets `--method`, or the default method, Noise. This user's home keeps each method's identity in a file of its own (`identity` for WebCrypto, `identity-noise` for Noise) and records in `method` the one named last, or the first one made; with no `--method`, a command uses that one. Only a home from before the record, holding both, refuses to choose until one is named. Nothing on the wire names the method and nothing negotiates it: a peer of the other method fails the handshake, the side answering logs "the peer proves by X, and this endpoint only by Y", and the side connecting sees the connection end. `serve`, `dial` and `forward` print `proving by METHOD` on standard error, after `listening on` for `serve`, then a `non-default:` line for each setting other than its default.
- **Moving to Noise:** a home or identity file made before keeps WebCrypto, and so do the peers paired with it. To move, a person runs `pair-accept --method noise` (which makes the Noise identity and records Noise as the home's method, so every command naming none then proves by it), pairs each peer again, and serves `serve --paired`. A WebCrypto peer still to be served needs `serve --paired --method webcrypto` on another port, which records WebCrypto again, so name `--method noise` on the Noise one too. A paired WebCrypto key is never carried over: a peer id names a key of one type.
- **Session settings:** `--key-limit BYTES` is what one key carries in each direction (default 2^38 for both methods, no floor); `--no-rekey` makes Noise end a session at the limit instead of changing keys, as WebCrypto, which has no key change, always does. A limit above the default has to be raised at both ends: a receiver holds a sender to the larger of its own limit and the default.
- **Mutual TLS:** the client's certificate by pin, trust store or CA. The stream is plain ACP protected by that TLS, so it ends where TLS ends and a proxy must vouch for the certificate.
- **TLS checking only the server:** `dial` with `--expect` or `--ca-file` and nothing else. `serve --no-peer-proof --tls-identity` is its other end.
- **No proof (`--no-peer-proof`):** whoever can connect gets the agent, which can run what the agent runs. `serve` allows it only on loopback, or with `--permit`, and refuses a browser unless its `--origin` is listed, since any page could otherwise drive it. `dial` allows it only for `ws://` on loopback.
- `serve` with the in-band proof prints its peer id, one line, on stdout. Each proven connection gets an agent process of its own, up to `--max-peers` (default 4), and another is turned away while they run. Handshakes and proofs run apart, so a slow peer holds up nobody; past 64 unproven connections a new one is dropped. An unproven peer may send at most 3072 bytes in a message. Plain `ws` is refused beyond loopback, where it is for a proxy in front. A flag that cannot apply to the mode is refused.
- An initiator under Noise finishes its handshake before the responder decides, so a refused `dial` or client sees its link end rather than a failed proof, and `dial` reconnects as for any lost link.
- `--allow-store` is read at each handshake and every second of a live link, so a peer removed from the store loses its link within about a second.
- `dial` keeps the agent across reconnects, ends with the agent's status, and stops on a wrong certificate, a remote that proves the wrong key, or a remote removed from `--trust-store`. A remote it cannot reach is tried again.
- `pair-accept` and `pair-dial` pair by their identity's method, saying `pairing by METHOD` on standard error; a joiner of the other method fails the handshake, and the stored key names its method, so the two then connect by it. `pair-accept` prints the PIN, then its peer id, and waits until someone uses the PIN or it expires; connections run apart, so one that says nothing costs neither the PIN nor the wait, and one wrong try ends it. `pair-dial` reads the PIN from `--pin-file` or standard input and prints the other side's peer id. Both record the peer in `--store`. A pairing connection never serves an agent: reconnect pinned.
- An identity file holds a key of one method, Noise's X25519 or WebCrypto's P-256, created by `keygen`, and by `dial`, `forward`, `pair-accept` and `pair-dial` when absent, of `--method` or else the default. A TLS identity is a separate PEM certificate and key.
- `serve` says `listening on` on standard error only once it can also be found. `--advertise LABEL` is off by default. On loopback it writes a descriptor; otherwise it publishes `_acp-agent._tcp` on every interface the listener is bound to, with no secret in it. `--advertise-host` publishes a host name for a certificate a browser accepts; without it, clients get an address. A killed `serve` leaves a descriptor, which the next search removes. The DNS-SD record is not withdrawn on a kill, and expires.
- `serve --tls-short-lived` makes a certificate for this run, in memory, which a browser can pin over WebTransport with no certificate authority, and advertises its hash as `wth`. Browsers take one valid for 14 days at most, so `serve` ends with status 3 after 13 days: restart it, and a client finds the new hash by searching again. It needs the in-band proof, which is what authenticates.
- `discover` prints one JSON object per line: `label`, `url`, `peer`, `method`, `source` (`dns-sd` or `local`), `webtransport`, `webtransportHash`, `acpVersions`. It is what they claim. `serve --advertise` publishes the method its identity proves by, and a record naming none is WebCrypto's.
- Exit status: 3 when a short-lived certificate is ending; the agent's when it ended the bridge; 2 for a bad configuration, a failed pairing, or a remote that failed its proof.

## `coqui_acp_bridge::relay`

```rust
pub fn relay<C: Channel>(channel: &C, agent: &Agent) -> LinkEnd
pub enum LinkEnd { AgentEnded, AgentStuck, PeerClosed, Failed(Failure), Protocol }  // non_exhaustive
```

Passes complete messages until one side ends, reading none of them. A message holding a raw newline ends the link as `Protocol`, since it would forge a second line. A slow side feels back-pressure; a line that waits past the limit ends the link, as `AgentStuck` when the agent stopped reading. Nothing is replayed.

## `coqui_acp_bridge::linger`

```rust
pub const OUTPUT_LOST: &str;   // "_dev.boriken.coqui.acp/output_lost"
pub const MAX_BUFFER: usize;   // 256 MiB
pub struct Settings { pub keep: Duration, pub per_peer: NonZeroUsize, pub total: NonZeroUsize, pub buffer: usize, pub settle: Duration }   // non_exhaustive
pub const fn Settings::new(keep: Duration) -> Settings   // per_peer 1, total 4, buffer 16 MiB, settle 500 ms
pub fn Keeper::new(settings: Settings, accepted: impl Fn(&str) -> bool + Send + Sync + 'static, report: impl Fn(&str) + Send + Sync + 'static) -> Keeper   // Clone: clones share
pub fn Keeper::arrive(&self, peer: &str) -> Arrival
pub fn Arrival::serve<C: Channel + ?Sized>(&self, channel: &C, start: impl FnOnce() -> io::Result<Agent>) -> io::Result<LinkEnd>
```

`arrive` waits up to `settle` while a link of this peer is up, and with a `settle` too long for the clock, until it ends. Hold the `Arrival` until the connection's agent has been dealt with, and drop anything limiting links first. `serve` relays with the agent kept for this peer, or else one from `start` after what a kept agent that ended wrote, then keeps it or ends it by how the link ended. `peer` has to be what a proof showed. `accepted` is asked about once a second for each kept agent and again before giving one back; a peer it refuses has its agents ended. It must not block for long. `report` hears what became of each agent, in a sentence. `buffer` above `MAX_BUFFER` holds `MAX_BUFFER`.

## `coqui_acp_bridge::agent`

```rust
pub fn Agent::spawn(program: &OsString, args: &[OsString], max_line: usize) -> io::Result<Agent>
pub fn Agent::spawn_with(spawner: &dyn Spawner, program: &OsStr, args: &[OsString], max_line: usize) -> io::Result<Agent>
pub const fn Agent::with_write_limit(self, limit: Duration) -> Agent   // default 30 s
pub fn Agent::guarded(self, program: &OsStr, args: &[&str]) -> io::Result<Agent>
pub fn Agent::pid(&self) -> u32
pub fn Agent::guard_pid(&self) -> Option<u32>
pub fn Agent::try_status(&self) -> io::Result<Option<ExitStatus>>
pub fn Agent::stop(&self) -> io::Result<ExitStatus>
pub fn watch(agent: u32, from_bridge: impl Read)

pub trait Spawner: Send + Sync { fn spawn(&self, program: &OsStr, args: &[OsString]) -> io::Result<Spawned>; }
pub struct Spawned { pub process: Box<dyn Process>, pub input: Box<dyn Write + Send>, pub output: Box<dyn Read + Send> }
pub trait Process: Send + Debug {   // id, peek, end_group, try_wait, kill, wait
    fn id(&self) -> u32;
    fn peek(&mut self) -> io::Result<Option<ExitStatus>>;
    fn end_group(&mut self);
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<ExitStatus>;
}
pub struct Local;   // the Spawner `Agent::spawn` uses
```

Standard error stays on this process's. `stop`, and dropping an `Agent`, end the process and what it started: its process group on Unix, its job object on Windows, where `spawn` fails, with the system's own error text, if it cannot make one or put the process in it (a process that had already ended has nothing to put, and is no failure). `try_status` does not collect the process, so its id and its group's stay its own until `stop`. `guarded`, on Unix, starts `program args... PID`, which has to call `watch` with that id and its standard input: a pipe ending with nothing said ends that process group, as cleanup and not containment (see `--linger`). `stop` dismisses it after ending the group and before collecting the agent. On Windows `guarded` starts nothing and `guard_pid` is `None`. `watch` does nothing for an id of 0 or 1.

A host that cannot wait on its own children (one inside an application whose process handling collects every child that ends, so `try_wait` reports no child) starts agents with `Agent::spawn_with` and a `Spawner` of its own; `Agent::spawn` is `spawn_with(&Local, ..)`, this process's own children as before. A `Spawner` starts `program args...` in a process group of its own with standard input and output piped and standard error inherited, and returns a `Process` that keeps what the trait says: `peek` sees an ended process and leaves it uncollected, `end_group` ends the process group (the job object on Windows) and does nothing for a process already collected, `try_wait` and `wait` collect. `stop` calls `end_group` once, before `try_wait`, `kill` and `wait`. `guarded` starts its helper as this process's own child, whatever the spawner.

## Gotchas

- A browser takes a self-signed certificate only for WebTransport with `serverCertificateHashes`, so a `wss://` address `discover` gives opens in a browser only with a certificate it accepts. `--tls-short-lived` is the way without one.
- On Windows, the first time this program listens or joins multicast the firewall may ask, and an answer of no leaves an inbound block rule for that path. `discover` then sees agents on this machine and none on others.
- mDNS stays on the link: it does not cross a router or a site-to-site VPN. `discover` does not check that an answer came from the link, so anyone who can reach UDP 5353 here can send a record. A record is a claim either way.
- `serve --advertise` beyond loopback needs the in-band proof, since a client has to be able to check the key a record names; a record naming no key is ignored.
- A browser pairs by PIN under Noise with `coqui-peer-noise`'s JavaScript module against `pair-accept --method noise --origin chrome-extension://ID`, its extension's origin; with no `--origin`, `pair-accept` refuses every browser (403) before the handshake, so a page cannot spend the PIN, and takes native clients, which send none. `--timeout-seconds` (600, the PIN's own life) bounds the wait, `--run-seconds` (120, as long as the extension waits) each connection; WebCrypto's module has no PIN exchange, so a WebCrypto browser is accepted by its key.
- Behind a reverse proxy every peer has the proxy's address, so `--permit`, `--deny` and the store's per-source limit on waiting entries see the proxy. Bind to loopback and filter at the proxy.
- `dial` keeps the agent across links, so lines not yet sent go to the next link, which may be another allowed peer. `serve --linger` never does: an agent goes back only to the key it served, and under `--paired` every tool of a user shares that key.
- A key file's permissions are set only on Unix; elsewhere it takes its directory's. Nothing checks them on reading. Keep keys in a directory only their owner can read.
- Removing a peer from a store does not affect `--allow-cert`, which has no store.
