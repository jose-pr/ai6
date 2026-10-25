# Stations and failover

A **Station** is a node running the `ai6` daemon. It checks in to the shared Registry
with its roles, labels, capacity, active work and the Operation checkouts it has. One
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
| Capacity | its capacity is 0, or it already holds that many leased Missions |
| Theater binding | the Operation's `theater` names a different Theater |
| Theater reachable | the Station cannot reach the Theater now (no transport provider, or the last contact failed), or has not reported what its transport can do |
| Theater capabilities | neither mode below is possible: field mode needs a route to the provider's API and either a runner on the Theater (reached with `run` on another host), `terminal` and `files` on the Station's own host, or, for the detached wrapper on another host, `run`, `files`, a POSIX host and a declared harness; remote-hands mode needs the transport's `run`. `agents = "local"` or `"remote-hands"` allows only that mode, and the reason names the missing capability |
| Labels | `station_labels` is not satisfied. Patterns are globs matched against the Station's labels overlaid by the Theater's |
| Checkout | in field mode on the Station's own host, the Operation has a local `path` and the Station does not hold it. On another host, and in remote-hands mode, the checkout is on the Theater, so the Operation must be bound to the Theater or the Theater must set a `workdir` |
| Handler | the Agent's handler is not available on the Station, so the next qualified Agent is the backup. A Station that did not report its handlers is not excluded |

Among what remains, the least-loaded Station comes first (the deciding Station wins a tie,
then Station ID), the `local` Theater before other Theaters, then the Agent with the
highest priority. There is no scoring beyond that. Each choice also says its mode:
`field` runs the harness on the Theater, `remote-hands` runs it on the Station and acts on the
Theater through its transport (see [Theaters](theaters.md#two-ways-to-work-on-a-theater)).

Ask before launching: `GET /v1/user/operations/{id}/placement-preview` on Universal
Exports (needs `mission.read` on the Operation) returns the eligible choices, best first,
and a reason for every rejection. Moneypenny's Stations view has a **Placement preview**
panel for it. The query parameters `agent`, `role`, `stations` and `tags` narrow it the
way a Mission would. The preview uses the Operation's policy as the Headquarters Station
published it at its last check-in, and it lists every Station in the Registry, not only
those hosting the Operation.

## Terminal

The Station, not the Agent, decides which terminal provider harnesses run in. It uses
an available provider, preferring `pty`; `ai6 serve --terminal ID` pins one. See
[Terminals](terminals.md).

## Shared Registry

Stations sharing a file Registry (`AI6_REGISTRY`) coordinate check-ins, leases and
Headquarters election. Lease expiry uses wall-clock time, so Stations need synchronized
clocks. Check-ins expire after 45 seconds; a Station whose check-ins lapse has *gone dark*
and its leased briefs are recovered by the Headquarters Station: a Session a runner still
supervises is adopted by another Station, any other is lost and tried again
([Recovery](missions.md#recovery)). The Registry is file-backed; a SQL
backend is not supported.

## Capacity and visibility

Operators granted `station.configure` can change a Station's maximum concurrent Missions
(1–1,024) in Moneypenny's Administration view; the change applies to new Missions and is
audited. `GET /v1/stations` on Universal Exports returns the signed-in user's authorized
view of Station health, roles, capacity, the Operations each Station serves, the Theaters
each Station reaches and the current Headquarters Station; host and process details are
omitted.

## Station identity

With the `station-tls` feature, each Station has a persistent certificate:
`ai6 identity` prints its fingerprint (`sha256:` and 64 hex digits), `ai6 peer waiting` lists
peers a listener refused together with the address each came from, and
`ai6 peer accept FINGERPRINT` pins one. Any spelling of a fingerprint is read, including the
grouped base32 earlier versions printed.

Stations speak the `ai6-station/1` protocol over mutual TLS with the strict policy by default
(TLS 1.3, SHA-2 pins). A peer is listed as waiting only after it has proved it holds its key,
and the list is bounded per source address, in total, and by how often one peer from one
address is written down again. A Station's `identity.pem` and `peers/` directory from an
earlier version are read as they are. The mechanism is the shared
[coqui-peer-tls](https://boriken-dev.github.io/coqui-peer-tls/) crate.
