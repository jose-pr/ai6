# Stations and failover

A **Station** is a node running the `ai6` daemon. It checks in to the shared Registry
with its roles, labels, capacity, active work, the Operation checkouts it has and its
host's CPU and memory (`resources`). One
control Station holds the Headquarters lease at a time and is the **Headquarters Station
(HQS)**; M's authority follows that lease.

Start a Station with both roles:

```console
ai6 serve --station hq-1 --role control --role execution
```

A standby control Station takes over the Headquarters lease if it expires
(*relocation* to a *fallback Station*). The lease carries an election **term** that grows
each time a different Station takes it; Session leases draw their fencing tokens from it
([Recovery](missions.md#recovery)):

```console
ai6 serve --station hq-2 --role control
```

An execution-only Station (an *outstation*) runs work but cannot hold the lease:

```console
ai6 serve --station lab-gpu --role execution --label gpu=yes
```

The default roles are `control` and `execution`. Every Station is also labeled with its
Station ID, host and operating system; `--label KEY=VALUE` adds more.

## Serving Operations

Operations are global: they live in `operations.toml`, and no Station owns one. A Station
runs an Operation's Missions when its labels satisfy the Operation's `station_labels` and, for
a local Operation, when it has the checkout. One Station can serve many Operations, and
one Operation can be served by many Stations.

## Placement

The Headquarters Station places each queued brief with M's placement. For one Mission it
considers every Station that has checked in, each Theater that Station reaches (see
[Theaters](theaters.md)) and each Agent posted to the Operation, and keeps the
combinations that pass these checks:

| Check | A Station, Theater or Agent is ruled out when |
| --- | --- |
| Operation `stations` | the Operation lists Stations and this one is not among them |
| Mission `stations` | the brief narrows to Stations and this one is not among them. A Mission can only narrow the Operation's list, never widen it |
| Role | the Station has no `execution` role |
| Health | its last check-in is older than 45 seconds (gone dark; `GONE_DARK_AFTER`) |
| Capacity | its capacity is 0, or it already holds that many Missions: its leases or the Sessions it reported running, whichever is more |
| Resources | it reported less free memory than `min_free_memory_mb` (default 512) or a CPU busier than `max_cpu_percent` (default 95). The same limits rule out a Theater by its own sample. A figure that was not reported never rules anything out |
| Theater binding | the Operation's `theater` names a different Theater |
| Theater reachable | the Station cannot reach the Theater now (no transport provider, or the last contact failed), or has not reported what its transport can do |
| Theater capabilities | neither mode below is possible: field mode needs a route to the provider's API and either a runner on the Theater (reached with `run` on another host), `terminal` and `files` on the Station's own host, or, for the detached wrapper on another host, `run`, `files`, a POSIX host and a declared harness; remote-hands mode needs the transport's `run`. `agents = "local"` or `"remote-hands"` allows only that mode, and the reason names the missing capability |
| Theater requirements | the Theater declares `requires` and the Station's probe has not checked them yet, or its `os`, `tools` or `disk_mb` are unmet (see [Fitting a Theater out](theaters.md#fitting-a-theater-out)) |
| Harness | in field mode, the Theater lists `requires.harnesses` and the Agent's handler is not among them or is not installed there; that Agent is ruled out on that Theater only |
| Labels | `station_labels` is not satisfied. Patterns are globs matched against the Station's labels overlaid by the Theater's |
| Checkout | in field mode on the Station's own host, the Operation has a local `path` and the Station does not hold it. On another host, and in remote-hands mode, the checkout is on the Theater, so the Operation must be bound to the Theater or the Theater must set a `workdir` |
| Handler | the Agent's handler is not available on the Station, so the next qualified Agent is the backup. A Station that did not report its handlers is not excluded |

Each (Station, Theater) that remains gets a load **score** from 0 (idle) to 4, the sum of
four shares from 0 to 1: the Missions the Station holds against its capacity, how busy the
CPU is (the CPU percent, or for a Theater the load average per CPU), the memory in use, and
the Station's Sessions on that Theater against its capacity. A Theater on the Station's own
host is judged by the Station's sample. A figure that was not reported adds nothing, so a
Station that reports no resources scores on its Missions alone. The Operation's `placement`
decides the order: `spread` (the default) puts the lowest score first, `pack` the highest
(see [Operations](operations.md#spreading-or-packing-work)). Equal scores keep the order
without scores: the least-loaded Station first (the deciding Station wins a tie, then Station
ID), the `local` Theater before other Theaters, then the Agent with the highest priority.

The Headquarters Station judges each Station on what that Station published, never on its
own: a brief that names no Agent is offered, on each Station, the posted Agents whose handler
that Station has (so a handler the Headquarters Station lacks is no obstacle), and a Station
at its published capacity is passed over. The first choice whose Agent and Operation are
within their `max_concurrent` is taken; when there is none, the brief waits. Each choice also
says its mode:
`field` runs the harness on the Theater, `remote-hands` runs it on the Station and acts on the
Theater through its transport (see [Theaters](theaters.md#two-ways-to-work-on-a-theater)).

Ask before launching: `GET /v1/user/operations/{id}/placement-preview` on Universal
Exports (needs `mission.read` on the Operation) returns the eligible choices, best first,
each with its `score`, and a reason for every rejection. Moneypenny's Stations view has a
**Placement preview** panel for it, and each Station card shows the host's CPUs, CPU busy,
load, free memory and when they were sampled (`unknown` for a figure not reported); a
Theater's tooltip shows its own and how many Sessions run there. `ai6ctl stations` prints
the same figures as columns. The query parameters `agent`, `role`, `stations` and `tags` narrow it the
way a Mission would. The preview uses the Operation's policy as the Headquarters Station
published it at its last check-in, and it lists every Station in the Registry, not only
those hosting the Operation.

## Terminal

The Station, not the Agent, decides which terminal provider harnesses run in. It uses
an available provider, preferring `pty`; `ai6 serve --terminal ID` pins one. See
[Terminals](terminals.md).

## Shared Registry

Stations sharing a Registry coordinate check-ins, leases and Headquarters election. The
Registry is a directory every Station mounts, a SQLite file for a single Station, or a
PostgreSQL database for Stations that share no filesystem. `ai6 serve --registry LOCATION`
names it: a directory, `sqlite:PATH` or a `postgres://` URL; without it the Station uses
`AI6_REGISTRY` (read the same way, also by the `ai6` command and Universal Exports), else
the state directory.

```console
AI6_REGISTRY_PASSWORD=… AI6_CONFIG_PASSWORD=… ai6 serve \
  --registry "postgres://ai6@db.internal/ai6?sslrootcert=/etc/ai6/db-ca.pem" \
  --config "postgres://ai6@db.internal/ai6?sslrootcert=/etc/ai6/db-ca.pem"
```

In a directory Registry lease expiry uses each Station's clock, so Stations need
synchronized clocks; a SQL Registry uses the database's clock. Check-ins expire after 45
seconds; a Station whose check-ins lapse has *gone dark* and its leased briefs are
recovered by the Headquarters Station: a Session a runner still supervises is adopted by
another Station, any other is lost and tried again ([Recovery](missions.md#recovery)). The
Headquarters lease lasts 60 seconds between renewals; `--headquarters-lease SECS` sets how
long a standby control Station waits before taking over from one that went dark.

In a SQL Registry each kind of record has its own table, created on first use; a database
whose schema version is newer than the Station knows is refused, when it is opened and on
every write. Every write first takes the Registry's row in `ai6_clock`, so writers
queue as they do on the directory's lock, and leases, the Headquarters term and Session
fencing behave the same on every backend. A Station that stalls inside a write (a paused
VM, a half-open connection) loses its session after a quarter of the Headquarters lease,
so it cannot hold the others back from taking over: the statement, lock and idle
timeouts, and the connection's own, are derived from `--headquarters-lease`, and a
location that sets one itself keeps it. No secret is stored there: access tokens are kept
by digest, and credentials stay with Q.

PostgreSQL connections use TLS and verify the server: against the platform's trusted
roots, or only against `sslrootcert=PATH` when the location names a CA file. A location
may weaken that with `sslmode=require` (encrypted, server not verified) or
`sslmode=disable` (no TLS); `ai6 serve` and Universal Exports then print a warning at
start. Keep the password out of the location, where a process listing shows it:
`password_file=PATH`, or `AI6_REGISTRY_PASSWORD` and `AI6_CONFIG_PASSWORD`. Messages print
locations with the password hidden.

Stations and Universal Exports reach the Registry only through the `RegistryBackend`
traits (one per kind of record: briefs, Stations, Sessions, Cables, approvals and so on),
which the directory (`FileRegistry`) and the database (`SqlRegistry`) implement; a program
embedding the Station can pass another store with `ServeConfig::with_registry`.

## Capacity and visibility

Operators granted `station.configure` can change a Station's maximum concurrent Missions
(1–1,024) in Moneypenny's Administration view; the change applies to new Missions and is
audited. The same settings hold the Station's resource limits, `min_free_memory_mb` (default
512; 0 never rules it out) and `max_cpu_percent` (default 95; 100 never does), set in the
same view or through `PUT /v1/user/stations/{id}/settings`; the Station publishes them at check-in and placement
applies them to the Station and each Theater it reaches. `GET /v1/stations` on Universal Exports returns the signed-in user's authorized
view of Station health, roles, capacity, the Operations each Station serves, the Theaters
each Station reaches and the current Headquarters Station; host and process details are
omitted.

## Head of Station

The **Head of Station** keeps each Station fit: its free space, its safe houses, its
Theaters. It is an Agent role, `head-of-station`, not a process of its own: each **round** is
an ordinary Mission that an `each_station` [Sleeper](missions.md#sleepers) queues for every
Station, narrowed to that Station, under an Operation set aside for housekeeping.

```toml
# operations.toml
[operation.housekeeping]
path = "/var/lib/ai6/housekeeping"   # must exist on every Station that keeps rounds
clearance = "field"

[operation.housekeeping.sleepers.rounds]
schedule = "0 * * * *"               # hourly, in UTC
title = "Station round"
prompt = "Look over this Station with station_report, act within its limits and file_report what you found."
role = "head-of-station"
each_station = true

# agents.toml
[role.head-of-station]
purpose = "Keep one Station fit: disk, safe houses and Theaters."
policy = "Act only through the Station's tools and within their limits; report the rest."
```

Qualify an Agent for the role (`roles = [..., "head-of-station"]`) and authorize the rounds:
`ai6 sleeper authorize housekeeping rounds`.

- **A round per Station per window.** Headquarters queues one brief for each Station whose
  last check-in placement accepts for the Operation, keyed by the window and the Station, so
  a failover never doubles a round. A busy Station's round waits for room.
- **Rounds, not one long Session.** A single endless Session would hit context limits, park
  on provider limits and spend tokens idling; each round is a Mission of its own. What one
  round saw reaches the next through the Station's **logbook**: the newest 20 reports, kept
  in the Registry (`logbook/<station>.json`, or the `ai6_logbooks` table) and handed to each
  round as context, fenced as data.
- **Which Mission is a round.** Only this Station's round of an authorized `each_station`
  Sleeper, as the Registry recorded where its brief entered, gets the logbook and the
  Station's tools: its role is `head-of-station`, it names this Station alone, and the Sleeper
  is still configured and authorized exactly as it was when it fired. A brief anyone writes
  with that role and Station is not a round; Universal Exports refuses the role on a brief, an
  Agent may not propose it, and revoking the Sleeper takes the tools away from a round.
- **Untrusted text.** The logbook reaches a round with every line quoted, and
  `station_report` quotes what other Missions' Agents wrote (commit subjects, branch names),
  so a round that reads either is treated as exposed to untrusted input: it sends no Cable
  that is not a reply, and its requests always go to a person.

### Station tools and reports

A round's Cable endpoint also lists two tools that no other Session has:

- `station_report`: the Station's capacity and the Missions not over, free space where its
  state and safe houses are, its resources, each Theater it reaches and why placement would
  not use it, every safe house it keeps (the Mission's state, age, size, cache directories and
  whether its work exists anywhere else, as the safe-house provider tells it), provider
  libraries loaded without a pin, and the limits below. It carries no secret: no environment
  value, token, loadout or brief text.
- `file_report { summary, findings }`: findings are typed (`disk`, `theater`, `safe-house`,
  `provider`, `other`) with a severity (`info`, `warning`, `critical`). The report goes into
  the logbook, is published with the Station's next check-in (`head_of_station` in
  `GET /v1/stations` for people with `station.read` on `station:ID`, since it covers every
  Operation there, and on the Station's card in Moneypenny), and is sent as a Cable to the
  `heads-of-station` channel when `cables.toml` defines it and lets the Agent send there:

```toml
# cables.toml
[[grants]]
agent = "008"
action = "send"
scope = "channel"
name = "heads-of-station"

[channels]
heads-of-station = ["007", "008"]
```

Every call checks again that its Mission is still a live round of this Station; a Session of
any other role, or a round that is over, is refused.

### Safe-house hygiene: fixed limits

Kept safe houses (the working directories of Missions that held work when they ended) grow
with their build caches. `tidy_safe_house { mission, action }` tidies them within limits the
Station enforces in its own code, whatever the prompt or the Operation's policy says:

| What | When |
| --- | --- |
| Read anything `station_report` shows | always |
| `caches`: remove the cache directories (`target`, `node_modules`, `.dart_tool`; `ai6 serve --safe-house-cache NAME` replaces the list); in a Git safe house only those Git ignores whole | unattended, for a Mission that is over whose safe house has been left alone for 7 days (`--safe-house-cache-age SECS`) |
| `remove` a safe house whose work all exists elsewhere (every commit on a remote, checked against the remote itself; nothing uncommitted or untracked; ignored files only inside cache directories) | unattended, for a Mission that is over |
| `remove` a safe house holding anything else (unpushed commits, uncommitted or ignored files such as `.env`, submodules, Git LFS objects), or whose provider cannot tell (the `copy` provider) | green ink, kind `safe-house-delete`, asked of the Operation the safe house belongs to; no class authorisation approves it. The round carries on; a later call, in this round or a later one, removes it once a person approved, spending that approval on that one removal |
| `provision_theater` with `apply`: the steps a Theater's declaration calls for | unattended; anything outside the declaration is a finding |
| Change configuration, the roster or policy; read a secret; act on another Station; touch the safe house of a Mission that is not over | never: the tools cannot, or refuse |

`provision_theater { theater, apply }` checks a Theater this Station reaches against its
declaration, or with `apply` runs exactly the declared steps (see
[Provisioning](theaters.md#fitting-a-theater-out)); that is unattended because the operator
declared them, and a run that changed something is audited (`head_of_station.provisioned`).

A green ink request for a removal is made even when the Operation's green ink policy would let
the Agent delete unattended. A tidy holds the Mission's start reservation while it works, so a
retry or resume of that Mission waits for it, and a tidy of a Mission being started is
refused. Applying a Theater's provisioning is refused while a Mission of this Station works on
it, and a failed step's output is withheld from the round (`ai6 theater provision` shows it). Every removal is audited (`head_of_station.caches_removed`,
`head_of_station.removed`) with the Station as its resource.

## Station identity

With the `station-tls` feature, each Station has a persistent certificate:
`ai6 identity` prints its fingerprint (`sha256:` and 64 hex digits), `ai6 peer waiting` lists
peers a listener refused together with the address each came from, and
`ai6 peer accept FINGERPRINT` pins one. Any spelling of a fingerprint is read, grouped base32
included.

Stations speak the `ai6-station/1` protocol over mutual TLS with the strict policy by default
(TLS 1.3, SHA-2 pins). A peer is listed as waiting only after it has proved it holds its key,
and the list is bounded per source address, in total, and by how often one peer from one
address is written down again. A Station keeps its identity in `identity.pem` and its peers
under `peers/`, in its state directory. The mechanism is the shared
[coqui-peer-tls](https://boriken-dev.github.io/coqui-peer-tls/) crate.

## Provider libraries: pins and publishers

A Station runs every provider library it loads with its own privileges, so it checks them
first (bona fides). A **pin set**, `plugins.toml` in the configuration directory (or
`ai6 serve --plugin-pins PATH`, which must exist), names the libraries it accepts:

```toml
# profile = "strict"   # optional; see below
[[library]]
file = "ai6_agent_claude.dll"
pin = "sha256:..."               # the library's own digest

[[library]]
file = "ai6_agent_codex.dll"
publisher = "ed25519:..."        # or: whoever holds this key signed it
# require = "both"               # with pin and publisher together: both must hold
```

`ai6 plugins pin PATH...` prints the entry that pins each AI6 provider library it is given.
A publisher signs, with Ed25519, the tag `ai6-provider-library-v1`, a NUL, the library's
file name, a NUL and the library's 32-byte SHA-256, and ships the 64-byte signature beside
it as `FILE.sig`. The name is part of what is signed, so a library signed for one slot does
not load in another. With OpenSSL and a POSIX shell:

```console
{ printf 'ai6-provider-library-v1\0%s\0' ai6_agent_codex.dll
  openssl dgst -sha256 -binary ai6_agent_codex.dll; } > message.bin
openssl pkeyutl -sign -rawin -inkey publisher.pem -in message.bin -out ai6_agent_codex.dll.sig
openssl pkey -in publisher.pem -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \n'
```

The last line prints the key's 64 hex digits for `publisher = "ed25519:..."`. A rebuilt
library signed with the same key loads on every Station without re-pinning; removing the
`publisher` line, or the entry, is how a key or a library stops being accepted. There is no
key server and no revocation list.

The **profile** (`--plugin-profile`, else the pin set's `profile`) decides what loads:

| Profile | Default when | What loads |
| --- | --- | --- |
| `strict` | a pin set exists | only libraries a pin or publisher accepts. Each is read once, checked, and loaded from a copy written to the state directory's `plugins/verified/sha256-HEX.EXT`, never from where it was found. A refusal names the expected and found digests or the publisher. |
| `report` | there is no pin set | every library, from where it was found; each one no entry accepts is a weakening |
| `off` | never | every library, unchecked (itself a weakening) |

Every weakening is printed when the Station starts, listed by `GET /internal/plugins` with
each library's digest and verdict, and published in the Station's check-in
(`plugins_weakened`). Only the libraries that declare an AI6 provider kind are checked.

Every command that loads providers checks them the same way and takes the same
`--plugin-pins` and `--plugin-profile`, with the same defaults: the field runner
(`ai6 runner serve`, whose verified copies go under its own state directory),
`ai6 theater provision`, and Universal Exports for its access providers
(`AI6_EXPORTS_PLUGIN_PINS`, `AI6_EXPORTS_PLUGIN_PROFILE`). Each prints what it refused and
every weakening when it starts.

What a pin covers is the library file. A library that imports another non-system library
gets it from wherever the operating system's loader finds it; on Windows that includes
the directory of the executable, the current directory and `PATH`, which no pin covers.
AI6's own providers import only system libraries.
