# Stations and failover

A **Station** is a node running the `ai6` daemon. It checks in to the shared Registry
with its roles, labels, capacity, active work, the Operation checkouts it has and its
host's CPU and memory (`resources`). One
control Station holds the Headquarters lease at a time and is the **Headquarters Station
(HQS)**; M's authority follows that lease.

A Station's roles and labels are its `station` record ([Configuration](configuration.md)):
`stations/<id>/station.toml` in a configuration directory, or the `station:<id>` row of a
SQL provider, over the global `station` record. Start a Station with both roles, the
default:

```console
ai6 serve --station hq-1
```

A standby control Station takes over the Headquarters lease if it expires
(*relocation* to a *fallback Station*). The lease carries an election **term** that grows
each time a different Station takes it; Session leases draw their fencing tokens from it
([Recovery](missions.md#recovery)):

```toml
# stations/hq-2/station.toml
roles = ["control"]
```

An execution-only Station (an *outstation*) runs work but cannot hold the lease:

```toml
# stations/lab-gpu/station.toml
roles = ["execution"]
labels = { gpu = "yes" }
```

The default roles are `control` and `execution`. Every Station is also labeled with its
Station ID, host and operating system; the record's `labels` adds more.

The record's tables bound and pace the Station's own work: how often it checks in
(`[scheduler] check_in`), how much it holds for Universal Exports (`[publisher]`), how it
probes Theaters and re-attaches to Sessions, how long its secret leases last, and so on, each
key with a default ([Station runtime limits](configuration.md#station-runtime-limits)). Its
`[providers.<id>]` tables configure the handlers, terminals and safe houses it builds:

```toml
# stations/hq-1/station.toml
[scheduler]
check_in = 5

[providers.pty]
scrollback = 2000
```

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
| Health | its last check-in is older than `[registry.limits.stations] gone_dark_after`, 45 seconds by default (gone dark) |
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
each with its `score`, and a reason for every rejection. Outer Office's Stations view has a
**Placement preview** panel for it, and each Station card shows the host's CPUs, CPU busy,
load, free memory and when they were sampled (`unknown` for a figure not reported); a
Theater's tooltip shows its own and how many Sessions run there. `ai6ctl stations` prints
the same figures as columns. The query parameters `agent`, `role`, `stations` and `tags` narrow it the
way a Mission would. The preview uses the Operation's policy as the Headquarters Station
published it at its last check-in, and it lists every Station in the Registry, not only
those hosting the Operation.

## Terminal

The Station, not the Agent, decides which terminal provider harnesses run in. It uses
an available provider that suits what the handler declares: one without a terminal,
preferring `pipe`, for a harness that speaks JSON (Claude Code, Codex), else preferring
`pty`. The `station` record's `terminal` pins one, and a Mission whose harness the pin does not suit
is refused at start. See [Terminals](terminals.md).

## Shared Registry

Stations sharing a Registry coordinate check-ins, leases and Headquarters election. The
Registry is a directory every Station mounts, a SQLite file for a single Station, or a
PostgreSQL database for Stations that share no filesystem. The bootstrap file `ai6.cfg`
names it in `[registry] location` (or `ai6 serve --registry LOCATION`): a directory,
`sqlite:PATH` or a `postgres://` URL. Without one the Station uses its configuration
provider's database when that is SQL, else `registry` in its state directory; the `ai6`
command reads the same file, and so does Universal Exports. One SQL
location therefore configures a whole HA deployment:

```toml
# /etc/ai6/ai6.cfg
[provider]
location = "postgres://ai6@db.internal/ai6?sslrootcert=/etc/ai6/db-ca.pem"
password = { provider = "file", key = "db-password" }   # relative to this file
```

In a directory Registry lease expiry uses each Station's clock, so Stations need
synchronized clocks; a SQL Registry uses the database's clock. Check-ins expire after 45
seconds; a Station whose check-ins lapse has *gone dark* and its leased briefs are
recovered by the Headquarters Station: a Session a runner still supervises is adopted by
another Station, any other is lost and tried again ([Recovery](missions.md#recovery)). The
Headquarters lease lasts 60 seconds between renewals; `ai6.cfg`'s `[registry]
headquarters_lease` sets how long a standby control Station waits before taking over from one
that went dark.

In a SQL Registry each kind of record has its own table. Each open compares the tables and
indexes this build needs with what the database's catalog holds, under the same lock two Stations
opening together wait on, and makes only what is missing, so a new table needs no script. Nothing
runs when nothing is missing, so a role with no `CREATE` right starts on a database someone else
keeps up to date. A role that cannot make what is missing stops at start, naming itself, the schema
and what is missing: start that build once as a role that may, or create them from
`crates/ai6/src/directorates/registry/sql/schema/postgres.sql`, and grant them to it. A script still
does what moves data. A database whose schema version is newer than the Station knows is refused,
when it is opened and on every write.

A column added to a table that exists is a schema step: a name and its statements in
`crates/ai6/src/directorates/registry/sql/steps.rs`. After making what is missing, each open runs
the steps the database's `ai6_schema_steps` table does not record, one transaction each and in
order, and records each with the time. A step whose column is there already, made by this build or
added by hand, is recorded without running. The first, `mission_ids.handle`, adds the handle
book's `handle` column to a database from before Mission handles; a handle book with rows refuses
it, naming the step, since those Missions need UIDs. On PostgreSQL only the table's owner may run
a step, so another role stops at start naming itself and the step. Then every table is compared with the columns this
build makes it with: one that lacks a column, such as one dropped by hand, stops the Station naming
table and column, while a column the build does not know, as a later build leaves after a
rollback, is allowed.

Every write first takes the Registry's row in `ai6_clock`, so writers queue as they do on the directory's lock, and leases, the Headquarters term and Session
fencing behave the same on every backend. A Station that stalls inside a write (a paused
VM, a half-open connection) loses its session after a quarter of the Headquarters lease,
so it cannot hold the others back from taking over: the statement, lock and idle
timeouts, and the connection's own, are derived from the Headquarters lease, and a
location that sets one itself keeps it. No secret is stored there: access tokens are kept
by digest, and credentials stay with Q.

PostgreSQL connections use TLS and verify the server: against the platform's trusted
roots, or only against `sslrootcert=PATH` when the location names a CA file. A location
may weaken that with `sslmode=require` (encrypted, server not verified) or
`sslmode=disable` (no TLS); `ai6 serve` and Universal Exports then print a warning at
start. Keep the password out of the location, where a process listing shows it: `ai6.cfg`'s
`[provider]` and `[registry] password` take a [secret reference](configuration.md#secrets):
`{ provider = "file", key = "PATH" }`, `{ provider = "env", key = "NAME" }` or `{ provider =
"inline", value = "…" }`; the last two are printed as a warning and reported at check-in. Messages print locations with the password hidden.

Stations and Universal Exports reach the Registry only through the `RegistryBackend`
traits (one per kind of record: briefs, Stations, Sessions, Cables, approvals and so on),
which the directory (`FileRegistry`) and the database (`SqlRegistry`) implement; a program
embedding the Station can pass another store with `ServeConfig::with_registry`.

## Capacity and visibility

Operators granted `station.configure` can change a Station's maximum concurrent Missions
(1 to `[registry.limits.stations] mission_capacity`, 1,024 by default) in Outer Office under
Stations → Settings; the change applies to new Missions and is
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
  Operation there, and on the Station's card in Outer Office), and is sent as a Cable to the
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

The same report is also kept as a [Report](reports.md) of the Station (kind `station`, linked to
the round's Mission), read with `station.read` on `station:ID`; when it cannot be stored, the logbook
entry stands and the answer says why.

Every call checks again that its Mission is still a live round of this Station; a round that is over
is refused. A Session of any other role has none of the Station's tools, and its `file_report` files
a Report of its own Operation, not a round's report.

### Safe-house hygiene: fixed limits

Kept safe houses (the working directories of Missions that held work when they ended) grow
with their build caches. `tidy_safe_house { mission, action }` tidies them within limits the
Station enforces in its own code, whatever the prompt or the Operation's policy says:

| What | When |
| --- | --- |
| Read anything `station_report` shows | always |
| `caches`: remove the cache directories (`target`, `node_modules`, `.dart_tool`; the `station` record's `safe_house_caches` replaces the list); in a Git safe house only those Git ignores whole | unattended, for a Mission that is over whose safe house has been left alone for 7 days (`safe_house_cache_age`) |
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

Stations do not connect to each other: they coordinate through the Registry, and each publishes to
Universal Exports. With the `station-tls` feature a Station still has an identity of its own, a
persistent certificate kept in `identity.pem` in its state directory, with the peers it trusts under
`peers/`, and beside it the [sealing key](agents.md#logins-kept-in-the-registry) logins are sealed to, published
in the Registry and signed by this identity. `ai6 identity` prints its fingerprint (`sha256:` and 64 hex digits), `ai6 peer waiting` lists
what is waiting in that store, and `ai6 peer accept FINGERPRINT` pins one; any spelling of a
fingerprint is read, grouped base32 included.

`ai6::station::identity` also holds the mutual TLS configurations for a link between Stations, the
`ai6-station/1` protocol with the modern policy by default (TLS 1.3, SHA-2 pins), on the shared
[coqui-peer-tls](https://boriken-dev.github.io/coqui-peer-tls/) crate. Nothing in `ai6 serve` dials or
listens on it today, so no peer comes to wait through a Station, and accepting one changes nothing
yet.

## When Universal Exports goes away

A Station publishes each Session's states and terminal output to Universal Exports. A gateway that
stops answering costs the Session nothing: the harness runs on, and the Station tries again with
growing pauses (`[publisher] publish_retry_delay`, `retry_max_delay`). Its `/internal/publisher`
reports it unhealthy, with the reason, until an event gets through.

Universal Exports holds its Sessions in memory, so one that comes back has forgotten them. When it
answers, and every `refresh_every` seconds in case it restarted during a quiet spell, the Station
registers each live Session again with its Mission's brief and sends its latest state, and its
sequence goes on from where it was, so a viewer's cursor stays good. Operator controls and terminal
input resume by themselves, since each is a poll the Station keeps making.

What is lost while it is away:

- **Terminal output past the queue.** `[publisher] queue` frames wait, of up to `terminal_bytes`
  each, across the Station's Sessions; past that the oldest go. The Station counts them
  (`lost_terminal_bytes` in `/internal/publisher`), and the Session's sequence skips a number there.
- **States in between.** When the queue is full, a Session's newer state replaces its older one, so
  the gateway's timeline can skip a state the Session passed through. Its latest state always
  arrives.
- **Whatever the gateway held, if it restarted**: every Session's scrollback, and any Session that
  ended before the gateway came back and has no state still waiting. The Registry keeps the
  Missions themselves.
- **Input typed into a gateway that went away**, and controls it had queued and not handed over,
  since it holds them in memory too.

The Session itself loses nothing: its harness and its record of the work are on the Station.

A viewer is told when output is missing: a sequence that skips a number, or a position older than
what the gateway holds, [reaches its terminal as a notice](terminals.md#a-viewer-reconnects-from-what-it-has).
A state skipped in the timeline is not announced.

## Provider libraries: pins and publishers

A Station runs every provider library it loads with its own privileges, so it checks them
first (bona fides). The **pin set**, the `plugin` records of its configuration provider
(`plugins.toml` in a configuration directory, global or in `stations/<id>/`), names the
libraries it accepts:

```toml
[[library]]
file = "ai6_agent_claude.dll"
pin = "sha256:..."               # the library's own digest

[[library]]
file = "ai6_agent_codex.dll"
publisher = "ed25519:..."        # or: whoever holds this key signed it
# require = "both"               # with pin and publisher together: both must hold
```

### Signing libraries

Signing is the normal path. A digest pin changes with every build; a signature travels with
the build, so a rebuild changes no record and one entry serves every architecture and both
sides of a rolling upgrade, which a digest cannot:

```console
ai6 plugins keygen --out publisher.key              # once, on the host that builds
ai6 plugins pin --publisher publisher.key lib/*.dll # once for each file name: the plugin records
ai6 plugins sign --key publisher.key lib/*.dll      # in every build: FILE.sig beside each library
ai6 plugins check --config ai6.cfg lib/             # before a restart
```

`keygen` prints the `publisher = "ed25519:..."` line and nothing of the key, and refuses a file
that exists. `pin --publisher` takes that `ed25519:...` text or the key's file. `sign` reads the
key only from its file. `check` says what a `strict` scan would say about each library against
the `plugin` records of this host's Station, printing its path, digest and verdict; it loads
nothing and writes no copy, and exits 1 on any refusal or when it found no library to check.
A pin set's `plugin` family applies at restart, so run it after importing and before restarting.

Copy `FILE.sig` with its library wherever libraries are copied, field runners included. A new
library name still takes one record, once: naming what may load is the policy.

### Storing pins

`pin` prints entries for a `plugins.toml`. With a configuration provider that takes changes
(SQL), `--import` stores them as its `plugin` records instead, which suits digest pins, renewed
with every build:

```console
ai6 plugins pin --import --config ai6.cfg lib/*.dll                     # the global records
ai6 plugins pin --import --scope station:s1 --config ai6.cfg lib/*.dll  # one Station's own
```

Each entry replaces the record of its file name, and records for other names stay as they are.
It prints a line per library, `added`, `changed` (with the old entry and the new) or
`unchanged`, and how many records it wrote; the change is checked and kept whole, as `ai6
config import` does. A directory of family files takes no changes, so `--import` is refused
there and writes nothing: add the printed entries to its `plugins.toml`. A Station applies
`plugin` records at restart.

### What a publisher entry gives up

**A publisher entry is weaker than a digest pin, and the compromise of its key matters more.**
Against a digest pin it gives up:

- **Exact bytes.** Any file that key signed for that name loads, an older vulnerable build
  included: nothing in a signature binds a version, so a signed old build can be put back.
- **Revocation.** A stolen key signs anything until the record changes and each host restarts.
  There is no key server and no revocation list: removing the `publisher` line, or the entry,
  is how a key or a library stops being accepted.
- **Holding no secret.** The key sits on whatever signs, where a digest pin needs none.

So each publisher whose signature alone is enough is listed as a weakening,
`publisher ed25519:... alone admits N libraries: anything it signed loads, older builds
included`, printed at start, in `GET /internal/plugins` and in the check-in's
`plugins_weakened`. An entry with a pin and a publisher accepts either one, so it is listed
too, unless it says `require = "both"`. With `require = "both"` the library must be those exact
bytes and carry the signature: it gives none of this up and is not listed, and every rebuild
is a change to its record, as with a digest pin.

Keep the key on the host that builds, never on a Station, which needs only the public half. On
Unix `keygen` writes it `0600`; on Windows the file takes its directory's access list, so put it
in a directory only you can read. A CI system should get it only for release builds, as a
repository secret. To rotate, sign everything with the new key, copy the signatures out and
change the records together, then restart; one key per entry is the whole model, with no list
of keys to overlap them, so a host restarted between the two steps refuses what does not match.

For development `profile = "report"` is the setting: everything loads from where it was found
and needs no signature, and each library no entry accepts is listed.

### The signature's format

What `sign` writes, for anything that signs without `ai6`: Ed25519 over the tag
`ai6-provider-library-v1`, a NUL, the library's file name, a NUL and the library's 32-byte
SHA-256, shipped as the 64 bytes of `FILE.sig`. The name is part of what is signed, so a library
signed for one slot does not load in another. With OpenSSL and a POSIX shell:

```console
{ printf 'ai6-provider-library-v1\0%s\0' ai6_agent_codex.dll
  openssl dgst -sha256 -binary ai6_agent_codex.dll; } > message.bin
openssl pkeyutl -sign -rawin -inkey publisher.pem -in message.bin -out ai6_agent_codex.dll.sig
openssl pkey -in publisher.pem -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \n'
```

The last line prints the key's 64 hex digits for `publisher = "ed25519:..."`. `ai6 plugins sign`
and `pin --publisher` read such a key too.

### Profiles

The **profile** (`--plugin-profile`, else `ai6.cfg`'s `[station.plugins] profile`; each host's own,
never the shared provider's) decides what loads:

| Profile | Default when | What loads |
| --- | --- | --- |
| `strict` | a pin set exists | only libraries a pin or publisher accepts. Each is read once, checked, and loaded from a copy written to the state directory's `plugins/verified/sha256-HEX.EXT`, never from where it was found. A refusal names the expected and found digests or the publisher. |
| `report` | there is no pin set | every library, from where it was found; each one no entry accepts is a weakening |
| `off` | never | every library, unchecked (itself a weakening) |

Every weakening is printed when the Station starts, listed by `GET /internal/plugins` with
each library's digest and verdict, and published in the Station's check-in
(`plugins_weakened`). Only the libraries that declare an AI6 provider kind are checked.

After a `strict` scan the copies in `plugins/verified/` it did not load are removed, keeping the
newest `[station.plugins] keep_verified` files by when each was written (default 8; `0` keeps none of
them; `"none"` means no limit, so every copy stays). A process keeps a shared lock on each copy it
loaded for as long as it runs, so another process's scan leaves that copy, and a scan holds
the directory's `.lock` from before its first copy until its last load, so no clearing removes a
copy between the two: clearing happens only when no other scan is under way, and otherwise waits
for the next one. A copy that cannot be removed is left and listed (`verified_held` in
`GET /internal/plugins`); it never fails the scan. On a file system that takes no locks nothing is
removed. `verified_removed` lists what went.

Every command that loads providers checks them the same way, with the same defaults:
`ai6 theater provision`, Universal Exports for its access providers (the provider's `plugin`
records, `[unex.plugins] profile` or `--plugin-profile`), and the field runner
(`ai6 runner serve --pins PATH --plugin-profile P --keep-verified N`, whose verified copies go
under its own state directory; a Station starting one on its own host writes the pins it
resolved into the runner's state directory and passes them with its own `keep_verified`). Each
prints what it refused and every weakening when it starts. A Station hands every runner it starts its record's `[runner]` and
`[providers.<id>]` tables (`--settings`, [The field runner](theaters.md#the-field-runner)).

What a pin covers is the library file. A library that imports another non-system library
gets it from wherever the operating system's loader finds it; on Windows that includes
the directory of the executable, the current directory and `PATH`, which no pin covers.
AI6's own providers import only system libraries.
